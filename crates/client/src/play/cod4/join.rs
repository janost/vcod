//! Joining a server: the connects, the missing-file downloads before the
//! join, the server probe, and a drop's or refusal's text in the install's
//! words.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use vcod_cod4::net::conn::{Conn, Cp, Join};
use vcod_cod4::net::download::DownloadProgress;
use vcod_cod4::net::parse::Op;

use super::{CS_SERVERINFO, Cod4Session, ET_PLAYER, Source, clock_now, info_value};
use crate::play::cod4_menus::{Join as MenuJoin, ScriptMenus};

impl Cod4Session {
    /// Joins `addr` and waits for its gamestate (`cp` answered), up to
    /// 30 s; `join` answers the team and class menus on their own.
    #[cfg(test)]
    pub fn connect(addr: SocketAddr, name: &str, cp: Cp, join: MenuJoin) -> Result<Cod4Session> {
        Self::connect_with(addr, name, cp, join, None)
    }

    /// [`Self::connect`], then, with `downloads`, the files the server
    /// references that the install lacks: fetched into it (over HTTP when
    /// the server redirects and `cl_wwwDownload` is on) and the gamestate
    /// `donedl` brings waited for, or with downloads off the join refused
    /// as retail's `CL_InitDownloads` refuses it (cod4-handshake.md 11). A
    /// disconnected HTTP fetch (`sv_wwwDlDisconnected`) joins again once it
    /// lands.
    pub fn connect_with(
        addr: SocketAddr,
        name: &str,
        cp: Cp,
        join: MenuJoin,
        downloads: Option<Fetch<'_>>,
    ) -> Result<Cod4Session> {
        Self::connect_password(addr, name, "", cp, join, downloads)
    }

    /// [`Self::connect_with`] with the userinfo's `password`, as the
    /// browser's join sends it.
    pub fn connect_password(
        addr: SocketAddr,
        name: &str,
        password: &str,
        cp: Cp,
        join: MenuJoin,
        downloads: Option<Fetch<'_>>,
    ) -> Result<Cod4Session> {
        let profile = super::cod4_profile::ProfileStats::load();
        Self::connect_profile(addr, name, password, cp, join, downloads, profile)
    }

    /// [`Self::connect_password`] with `profile`'s stats: the stats
    /// packets carry them and the server's `N`s go back to its file.
    pub(crate) fn connect_profile(
        addr: SocketAddr,
        name: &str,
        password: &str,
        cp: Cp,
        join: MenuJoin,
        mut downloads: Option<Fetch<'_>>,
        mut profile: Option<(
            super::cod4_profile::ProfileStats,
            vcod_cod4::net::stats::Stats,
        )>,
    ) -> Result<Cod4Session> {
        let mut fetched = std::collections::HashSet::new();
        let cancel = downloads.as_ref().and_then(|d| d.cancel.clone());
        let notes = downloads.as_ref().and_then(|d| d.notes.clone());
        // A reconnect a disconnected fetch, each one landing a file.
        for _ in 0..16 {
            // The profile's stats under the server's `fs_game`, as 1.7
            // loads them on `connectResponse` (cod4-front-end.md 6).
            let under: Arc<std::sync::Mutex<Option<_>>> = Arc::new(std::sync::Mutex::new(None));
            let stats_for = profile.as_ref().map(|(p, _)| {
                let (base, slot) = (p.dir().to_path_buf(), under.clone());
                Box::new(move |fs_game: &str| {
                    let (p, stats) = super::cod4_profile::ProfileStats::in_dir(&base, fs_game)?;
                    let sent = stats.clone();
                    *slot.lock().ok()? = Some((p, stats));
                    Some(sent)
                }) as vcod_cod4::net::conn::StatsFor
            });
            let conn = Self::join_conn(
                addr,
                Join {
                    name: name.into(),
                    password: password.into(),
                    cp: cp.clone(),
                    www: downloads.as_ref().is_some_and(|d| d.www),
                    cancel: cancel.clone(),
                    notes: notes.clone(),
                    stats: profile.as_ref().map(|(_, s)| s.clone()),
                    stats_for,
                    ..Join::default()
                },
            )?;
            if let Some(p) = under.lock().ok().and_then(|mut u| u.take()) {
                profile = Some(p);
            }
            let Some(conn) = (match downloads.as_mut() {
                Some(d) => fetch_missing(conn, d, &mut fetched)?,
                None => Some(conn),
            }) else {
                log::info!("cod4: reconnecting to {addr}");
                continue;
            };
            let mut s = Self::new(Source::Live {
                conn: Box::new(conn),
                last_send: None,
                last_msg: clock_now(),
            });
            s.menus = ScriptMenus::new(join);
            if let Some((p, stats)) = profile.take() {
                s.menus.set_stats(stats);
                s.profile = Some(p);
            }
            s.loaded_map = s.map_name();
            return Ok(s);
        }
        bail!("{addr}: still missing files after 16 reconnects")
    }

    /// `connect` to `addr` as `join` says and its first gamestate (`cp`
    /// answered), on a qport of its own, with voice.
    fn join_conn(addr: SocketAddr, join: Join) -> Result<Conn> {
        // A qport of its own, as `Netchan_Init`'s random one, so two clients
        // on one host are two clients; under 0x8000, as retail's stats
        // lookup misses a larger one (it read none on a live server).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let mut conn = Conn::join(
            addr,
            Join {
                qport: (nanos ^ std::process::id()) as u16 & 0x7fff,
                voice: true,
                ..join
            },
        )?;
        let deadline = clock_now() + Duration::from_secs(30);
        while conn.client.gamestate.is_none() {
            if clock_now() > deadline {
                bail!("{addr}: no gamestate");
            }
            conn.send(&[], None)?;
            conn.poll(clock_now() + Duration::from_millis(50))?;
        }
        Ok(conn)
    }
}

/// How [`Cod4Session::connect_with`] fetches what the install lacks.
pub struct Fetch<'a> {
    /// The install root the files land in, at their referenced paths.
    pub root: &'a std::path::Path,
    /// `cl_allowDownload`.
    pub allow: bool,
    /// `cl_wwwDownload`: sent in the userinfo, and a redirect fetched over
    /// HTTP.
    pub www: bool,
    /// Told the file in flight while a download runs, and `None` once
    /// the downloads end, for the connect screen.
    pub progress: Option<&'a mut dyn FnMut(Option<DownloadProgress>)>,
    /// Esc on the connect screen: set, the join's next read fails
    /// (`conn::Cancelled`).
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The connect screen's `print` lines and stages
    /// (`vcod_cod4::net::conn::JoinNote`).
    pub notes: Option<vcod_cod4::net::conn::Notes>,
}

impl Fetch<'_> {
    pub(super) fn report(&mut self, p: Option<DownloadProgress>) {
        if let Some(f) = self.progress.as_mut() {
            f(p);
        }
    }
}

/// `CL_InitDownloads` (iw3mp.exe 0x46ac60) on each gamestate of a live
/// join, as `CL_ParseGamestate` runs it (0x47423e): the referenced files
/// `fetch.root` lacks (`net::download::missing_in`), each fetched over
/// `svc_download` or the HTTP redirect with no usercmds sent meanwhile,
/// then `donedl` and the compare again on the gamestate it brings. A
/// refusal ends the join with its reason; with `allow` off a missing file
/// ends it as retail's `EXE_...`-less text does (0x6d0910). Retail fetches
/// a file that still differs after its download again on every
/// gamestate; vcod ends the join on the second ask (`fetched` holds what
/// this connect landed, across the joins of a disconnected fetch). `None` when a disconnected HTTP fetch landed and
/// the caller joins again.
fn fetch_missing(
    mut conn: Conn,
    fetch: &mut Fetch,
    fetched: &mut std::collections::HashSet<String>,
) -> Result<Option<Conn>> {
    loop {
        let gs = conn.client.gamestate.as_ref().context("gamestate")?;
        let si = String::from_utf8_lossy(gs.configstring(1)).into_owned();
        let map = info_value(gs.configstring(CS_SERVERINFO), "mapname")
            .filter(|m| vcod_cod4::install::is_zone_name(m));
        let wanted = vcod_cod4::net::download::missing_in(fetch.root, &si, map.as_deref());
        if wanted.is_empty() {
            return Ok(Some(conn));
        }
        let names: Vec<&str> = wanted.iter().map(|w| w.remote.as_str()).collect();
        log::info!("cod4: need files: {}", names.join(" "));
        if !fetch.allow {
            bail!(
                "You are missing some files referenced by the server: {}\nGo to the Multiplayer \
                 options menu to allow downloads",
                names.join(" ")
            );
        }
        if let Some(again) = names.iter().find(|n| fetched.contains(**n)) {
            return Err(vcod_cod4::net::conn::Refused(format!(
                "{again} is different from the server after its download"
            ))
            .into());
        }
        match download_round(conn, fetch, fetched, wanted)? {
            Some(c) => conn = c,
            None => return Ok(None),
        }
    }
}

/// `CL_NextDownload` through the list, then `donedl` and the gamestate it
/// brings (or nothing asked for); each file that lands goes in `fetched`.
/// `None` when a disconnected HTTP fetch landed: join again.
fn download_round(
    mut conn: Conn,
    fetch: &mut Fetch,
    fetched: &mut std::collections::HashSet<String>,
    wanted: Vec<vcod_cod4::net::download::Wanted>,
) -> Result<Option<Conn>> {
    use vcod_cod4::net::download::{DownloadEvent, Downloads};
    let mut dls = Downloads::new(fetch.root, wanted).with_www(fetch.www);
    let mut events = dls.next_download();
    let mut waiting = false;
    let mut last_heard = clock_now();
    let mut last_report = clock_now();
    let mut shown: Option<DownloadProgress> = None;
    loop {
        events.extend(dls.poll());
        for e in events.drain(..) {
            match e {
                DownloadEvent::Command(c) if c == "donedl" => {
                    conn.done_downloading();
                    waiting = true;
                }
                DownloadEvent::Command(c) if c == "wwwdl fail" => {
                    conn.www_failed();
                }
                DownloadEvent::Command(c) => {
                    conn.command(c);
                }
                DownloadEvent::Finished(name, p, size) => {
                    log::info!("cod4: downloaded {}", p.display());
                    // The file whole, though a fast fetch can land before
                    // a look ever saw it (or saw its size).
                    shown = None;
                    fetch.report(Some(DownloadProgress {
                        file: name.clone(),
                        bytes: size,
                        total: size,
                    }));
                    fetched.insert(name);
                }
                DownloadEvent::Skipped(why) => log::warn!("cod4: {why}"),
                DownloadEvent::Failed(why) => {
                    fetch.report(None);
                    return Err(vcod_cod4::net::conn::Refused(why).into());
                }
                DownloadEvent::Reconnect => {
                    fetch.report(None);
                    let _ = conn.disconnect();
                    return Ok(None);
                }
            }
        }
        if dls.is_done() && !waiting {
            fetch.report(None);
            return Ok(Some(conn));
        }
        // A disconnected fetch hears nothing from the server it left.
        if clock_now().saturating_duration_since(last_heard) > Duration::from_secs(30)
            && !dls.is_disconnected()
        {
            bail!("{}: the download stalled", conn.server());
        }
        let now = dls.progress();
        if now != shown {
            shown = now.clone();
            fetch.report(now);
        }
        if clock_now().saturating_duration_since(last_report) > Duration::from_secs(1)
            && let Some(p) = &shown
        {
            last_report = clock_now();
            log::info!(
                "cod4: downloading {}: {} of {} bytes",
                p.file,
                p.bytes,
                p.total
            );
        }
        conn.send(&[], None)?;
        for got in conn.poll(clock_now() + Duration::from_millis(50))? {
            last_heard = clock_now();
            for op in &got.parsed.ops {
                match op {
                    Op::Download(m) => events.extend(dls.on_message(m)),
                    Op::Gamestate if waiting => {
                        fetch.report(None);
                        return Ok(Some(conn));
                    }
                    Op::Gamestate => events.extend(dls.on_gamestate()),
                    _ => {}
                }
            }
        }
    }
}

/// `vcod --game cod4 --net-probe ADDR [--team T [--class C]]`: joins a
/// server headless for `secs` through the same session steps as the
/// window (join, `cp`, the menus answered, a cmd a frame, events, the
/// scene), a line a second, and the menus opened and the spawn as they
/// come.
/// With `install`, the files the server references that it lacks are
/// downloaded into it first.
pub fn probe(
    addr: SocketAddr,
    secs: u64,
    name: &str,
    cp: Cp,
    join: MenuJoin,
    install: Option<&std::path::Path>,
) -> Result<()> {
    let fetch = install.map(|root| Fetch {
        root,
        allow: true,
        www: true,
        progress: None,
        cancel: None,
        notes: None,
    });
    let mut s = Cod4Session::connect_with(addr, name, cp, join, fetch)?;
    println!(
        "cod4: joined {addr}, map {}",
        s.map_name().unwrap_or_default()
    );
    let t0 = Instant::now();
    let mut next = 0;
    let (mut menus, mut spawned) = (0, false);
    while t0.elapsed() < Duration::from_secs(secs) {
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        s.step(ms)?;
        for m in &s.menus.opened[menus..] {
            println!("{ms:.0} menu {m}");
        }
        menus = s.menus.opened.len();
        if let Some(sp) = s.spawn().filter(|_| !spawned) {
            spawned = true;
            println!("{ms:.0} spawned {sp}");
        }
        for f in s.take_fired() {
            println!("{ms:.0} event {} entity {:?}", f.name, f.entity);
        }
        for l in s.take_lines() {
            println!("{ms:.0} {l}");
        }
        if t0.elapsed().as_secs() >= next {
            next = t0.elapsed().as_secs() + 1;
            if let Some(sc) = s.scene() {
                let kinds: Vec<u32> = sc.entities.iter().map(|d| d.etype).collect();
                let players: Vec<String> = sc
                    .entities
                    .iter()
                    .filter(|d| d.etype == ET_PLAYER)
                    .map(|d| {
                        format!(
                            "{} ({:.0} {:.0} {:.0})",
                            d.number, d.origin.x, d.origin.y, d.origin.z
                        )
                    })
                    .collect();
                println!(
                    "t={next} snapshots {} time {} eye ({:.0} {:.0} {:.0}) drawn {kinds:?} players {}",
                    s.client().snapshots.len(),
                    sc.server_time,
                    sc.eye.x,
                    sc.eye.y,
                    sc.eye.z,
                    players.join(", ")
                );
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    s.disconnect();
    Ok(())
}

/// `n` or `B`, which a `fast_restart` queues every client in place of a
/// gamestate (cod4-map-cycle.md 2): the cgame runs `CG_MapRestart`.
pub(super) fn is_map_restart(text: &[u8]) -> bool {
    matches!(text.split(|&b| b == b' ').next(), Some(b"n" | b"B"))
}

/// `CL_CGameNeedsServerCommand`'s `w` (`iw3mp.exe` 0x45b1d9): the server
/// dropped us, `w "<reason>"` (`CL_DisconnectError`), or with no reason
/// `EXE_SERVER_DISCONNECTED`; a `PB` third argument is PunkBuster's kick,
/// its reason the same. `None` for any other command.
pub(super) fn drop_reason(text: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(text);
    let args = vcod_cod4::hud::tokenize(&text);
    // The switch reads the first token's first character.
    if !args.first()?.starts_with('w') {
        return None;
    }
    Some(match args.get(1) {
        Some(why) => format!("EXE_SERVERDISCONNECTREASON\u{15}{why}"),
        None => "EXE_SERVER_DISCONNECTED".into(),
    })
}

/// A drop's reason ([`Cod4Session::take_dropped`]) in the install's own
/// words: an `EXE_*` key translated, `EXE_SERVERDISCONNECTREASON` with
/// its `&&1` the translated reason after the `\x15` (`CL_DisconnectError`,
/// `iw3mp.exe` 0x46b4e0); a key the strings lack stays as it is.
pub fn localize_drop(why: &str, install: &std::path::Path) -> String {
    let strings = vcod_cod4::install::Install::at(install)
        .and_then(|i| i.code_strings("english").ok())
        .unwrap_or_default();
    let tr = |k: &str| strings.get(k).cloned().unwrap_or_else(|| k.to_string());
    match why.split_once('\u{15}') {
        Some((outer, arg)) => tr(outer).replace("&&1", &tr(arg)),
        None => tr(why),
    }
}

/// A refused challenge's `EXE_*` reason (`conn::Refused`) in the
/// install's own words, from `localized_code_post_gfx_mp`, its arguments
/// in ([`localize_message`]); any other error as it came.
pub fn localize_refusal(e: anyhow::Error, install: &std::path::Path) -> anyhow::Error {
    let Some(vcod_cod4::net::conn::Refused(why)) = e.downcast_ref() else {
        return e;
    };
    let Some(strings) =
        vcod_cod4::install::Install::at(install).and_then(|i| i.code_strings("english").ok())
    else {
        return e;
    };
    match localize_message(why, |k| strings.get(k).cloned()) {
        Some(t) => anyhow::anyhow!("{t}"),
        None => e,
    }
}

/// `SEH_LocalizeTextMessage`
/// (cod4-front-end.md 7): tokens split by 0x14 (the next one localized,
/// as the first is) and 0x15 (the next one as it stands); each token
/// after the first fills the text's next `&&n` in turn, or goes on its
/// end when none is left. `None` when a localized token has no string,
/// where CoD2's `Localized::message` keeps the token and also escapes `%s`.
pub fn localize_message(msg: &str, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut next = 1;
    let mut localize = true;
    let mut token = String::new();
    let mut tokens = Vec::new();
    for c in msg.chars() {
        match c {
            '\u{14}' | '\u{15}' => {
                tokens.push((std::mem::take(&mut token), localize));
                localize = c == '\u{14}';
            }
            c => token.push(c),
        }
    }
    tokens.push((token, localize));
    for (i, (t, loc)) in tokens.into_iter().enumerate() {
        if t.is_empty() {
            continue;
        }
        let t = if loc { get(&t)? } else { t };
        let slot = format!("&&{next}");
        match out.find(&slot) {
            Some(at) if i > 0 => {
                out.replace_range(at..at + slot.len(), &t);
                next += 1;
            }
            _ => out.push_str(&t),
        }
    }
    Some(out)
}

/// `demo`'s missing file (`timedemo 1` looks for `demos/1.dm_1`) reads
/// in the install's words with the path in, not as its key
/// (cod4-demos.md 1).
#[cfg(test)]
#[test]
fn a_missing_demo_reads_localized() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let t = localize_drop("EXE_ERR_NOT_FOUND\u{15}demos/1.dm_1", install.root());
    assert!(t.contains("demos/1.dm_1") && !t.contains("EXE_"), "{t}");
}

/// A refusal's argument fills its `&&1` (`EXE_SERVER_IS_DIFFERENT_VER`
/// with its 0x15 version, cod4-handshake.md), and a plain key reads as
/// its string.
#[cfg(test)]
#[test]
fn a_refusal_fills_its_arguments_in() {
    let get = |k: &str| match k {
        "EXE_SERVER_IS_DIFFERENT_VER" => Some("Server is a different version:\n&&1".to_string()),
        "EXE_SERVERISFULL" => Some("Server is full.".to_string()),
        _ => None,
    };
    assert_eq!(
        localize_message("EXE_SERVER_IS_DIFFERENT_VER\u{15}1.7", get).as_deref(),
        Some("Server is a different version:\n1.7")
    );
    assert_eq!(
        localize_message("EXE_SERVERISFULL", get).as_deref(),
        Some("Server is full.")
    );
    assert_eq!(localize_message("EXE_NOPE", get), None);
}
