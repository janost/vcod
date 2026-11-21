//! The front end's way into and out of a CoD2 game: `connect` from the
//! browser, the console or Start New Server joins on a thread of its own
//! (`play::cod2::Cod2Session::connect_in_background`), loads the map and
//! hands the session to the `Cod2` mode, as `--connect` does at startup (its
//! script menus and Esc are `App::cod2_script_menus` and
//! `App::escape_in_cod2`); `disconnect`, a drop or a failed load end
//! the game for the main menu.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use glam::Vec3;
use vcod_common::mapworld::MapWorld;
use vcod_common::{bsp, mesh};

use crate::camera::FlyCamera;
use crate::play::cod2::{Cod2Session, ConnectOpts, Join, ServerError};
use crate::{App, Mode, World, version_tag};

/// How long a front-end connect waits for the gamestate, as `--connect`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

impl App {
    /// A game is up: a CoD1 connection or a CoD2 session.
    pub(crate) fn in_game(&self) -> bool {
        matches!(self.mode, Mode::Online { .. }) || self.session.live()
    }

    /// `connect <addr>` on a CoD2 install: any game up ends first, the
    /// menus close, and the handshake runs off the main thread until
    /// [`Self::poll_cod2_connect`] sees it through.
    pub(crate) fn connect_cod2(&mut self, addr: String) {
        if self.in_game() || self.session.busy() {
            self.leave_cod2();
            self.unload_world();
            self.mode = Mode::Idle;
        }
        vcod_common::laps::begin_load(format!("cod2 connect {addr}"));
        self.start_cod2_connect(addr);
    }

    /// [`Self::connect_cod2`]'s handshake started, with no game up; a
    /// start-up `--connect` keeps the load profile the process start began.
    pub(crate) fn start_cod2_connect(&mut self, addr: String) {
        log::info!("connecting to {addr}");
        if let Some(c) = self.session.cod2_mut() {
            c.connecting = Some(Cod2Session::connect_in_background(
                &addr,
                connect_opts(&self.shell, &self.fs),
                CONNECT_TIMEOUT,
            ));
        }
        self.ui.close_all();
        self.connect_addr = Some(addr);
        self.set_title(format!("{} — connecting", version_tag()));
        if self.console.open {
            self.toggle_console();
        }
    }

    /// Once a loop: a finished front-end connect loads its map and enters
    /// the game, a failed one shows why over the main menu.
    pub(crate) fn poll_cod2_connect(&mut self) {
        let Some(result) = self
            .session
            .cod2()
            .and_then(|c| c.connecting.as_ref())
            .and_then(|c| c.poll())
        else {
            return;
        };
        let addr = self
            .session
            .cod2_mut()
            .and_then(|c| c.connecting.take())
            .map(|c| c.addr.clone())
            .unwrap_or_default();
        vcod_common::laps::mark(|| "cod2 gamestate".into());
        let entered = result.and_then(|s| self.enter_cod2(s));
        if let Err(e) = entered {
            // The server's own words go to the popup bare, to localize.
            let why = match e.downcast_ref::<ServerError>() {
                Some(ServerError(why)) => why.clone(),
                None if self.session.live() => format!("{e:#}"),
                None => format!("cannot connect to {addr}: {e:#}"),
            };
            self.disconnect(Some(why));
        }
    }

    /// `FS_ConditionalRestart` / `FS_Restart` for a CoD2 session: the
    /// search path opened again over the gamestate's `fs_game` (also set
    /// as the cvar, as `CL_SystemInfoChanged` sets it) and whatever its
    /// downloads landed, before the map loads.
    pub(crate) fn reopen_cod2_fs(&mut self) -> Result<()> {
        let Some(s) = self
            .session
            .cod2_mut()
            .and_then(|c| c.session.as_deref_mut())
        else {
            return Ok(());
        };
        if !s.take_reopen(self.fs_game.as_deref()) {
            return Ok(());
        }
        let game = s.fs_game().map(str::to_string);
        log::info!(
            "cod2: search path opened again, fs_game {:?}",
            game.as_deref().unwrap_or("")
        );
        self.shell.force("fs_game", game.as_deref().unwrap_or(""));
        self.fs_game = game;
        let (base, game) = crate::search_dirs(&self.game_dir, &self.mod_dir, &self.fs_game);
        crate::reopen_fs(
            &base,
            game.as_deref(),
            None,
            &mut self.fs,
            &mut self.localized,
            &mut self.hud,
            &mut self.audio,
            &mut self.fx,
            &mut self.quick_chat,
        )?;
        if let Some(r) = &mut self.renderer {
            r.reopen(&self.fs);
        }
        if let Some(c) = self.session.cod2_mut() {
            c.search_path_reopened();
        }
        Ok(())
    }

    /// The session's map loaded and the camera on its spawn, the session
    /// handed to `Mode::Cod2`. `--team` / `--weapon` answer the menus as
    /// at startup; without them the player picks in the script menus.
    pub(crate) fn enter_cod2(&mut self, s: Cod2Session) -> Result<()> {
        let mut s = Box::new(s);
        if !s.is_demo() {
            s.join = self.team.clone().map(|team| Join {
                team,
                weapon: self.weapon.clone(),
            });
        }
        let map = s.map_name().map(str::to_string);
        if let Some(c) = self.session.cod2_mut() {
            c.session = Some(s);
        }
        let map = map.context("cod2: no mapname in serverinfo")?;
        self.reopen_cod2_fs()?;
        let bsp = self.load_cod2_map(&map)?;
        let cam = match bsp::find_spawn(&bsp.entities) {
            Some((origin, yaw)) => FlyCamera::new(Vec3::from(origin) + Vec3::Z * 60.0, yaw),
            None => {
                let (min, max) = mesh::map_bounds(&bsp);
                FlyCamera::new((Vec3::from(min) + Vec3::from(max)) * 0.5, 0.0)
            }
        };
        self.world = Some(World::new(
            crate::map_base_name(&map),
            bsp,
            self.renderer.is_none(),
            crate::MapSource::Cod2(map.clone()),
        ));
        self.mode = Mode::Cod2(cam);
        self.fx.clear();
        self.audio.on_gamestate(&map);
        self.set_title(format!("{} — {map}", version_tag()));
        self.set_grab(true);
        Ok(())
    }

    /// `maps/mp/<map>.d3dbsp` read and loaded into the renderer, the
    /// session (when there is one) kept alive on its pump meanwhile. The
    /// draw's model handles die with the old map; what it parsed stays.
    pub(crate) fn load_cod2_map(&mut self, map: &str) -> Result<MapWorld> {
        let pump = self
            .session
            .cod2_mut()
            .and_then(|c| c.session.take())
            .map(|s| s.pump());
        let mut predict_world = None;
        let loaded = (|| -> Result<MapWorld> {
            let mut laps = vcod_common::laps::Laps::new(format!("cod2 map {map}"));
            let path = format!("maps/mp/{map}.d3dbsp");
            // `Com_LoadBsp`'s error (Mac 0x11154e), which the popup localizes.
            let data = self
                .fs
                .read(&path)
                .ok_or_else(|| ServerError(format!("EXE_ERR_COULDNT_LOAD\u{15}{path}")))?;
            laps.lap("read");
            // An upload that panics ends the game too, not the client.
            let (file, bsp) = parse_map(&data, &path)?;
            laps.lap("parse");
            // The session's prediction collision builds beside the upload.
            let session = pump.is_some();
            let (file, world) = (&file, &bsp);
            let up = std::thread::scope(|sc| {
                let collision = session.then(|| {
                    sc.spawn(move || {
                        let t0 = std::time::Instant::now();
                        let built = std::panic::catch_unwind(|| {
                            let clip = std::sync::Arc::new(file.clip()?);
                            let cw = crate::play::cod2::prediction_world(world, clip.clone());
                            anyhow::Ok((cw, clip))
                        })
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("the collision build panicked")));
                        (built, t0)
                    })
                });
                let up = match &mut self.renderer {
                    Some(r) => {
                        let fs = &self.fs;
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            r.load_world(world, fs)
                        }))
                    }
                    None => Ok(Ok(())),
                };
                predict_world = collision.map(vcod_common::laps::joined);
                up
            });
            // The snow's `$outdoor` build traces the prediction's clip.
            let clip = match &predict_world {
                Some((Ok((_, clip)), _)) => Some(clip.clone()),
                _ => None,
            };
            if let Some(r) = &mut self.renderer {
                r.set_outdoor_source(clip.map(|clip| {
                    let (mins, maxs) = file.outdoor_bounds();
                    crate::renderer::OutdoorSource {
                        map: map.to_string(),
                        clip,
                        mins,
                        maxs,
                    }
                }));
            }
            match up {
                Ok(res) => res?,
                Err(_) => {
                    if let Some(r) = &mut self.renderer {
                        r.unload_world();
                    }
                    bail!("cannot load {path}: the renderer panicked");
                }
            }
            laps.lap("renderer and collision");
            laps.finish();
            Ok(bsp)
        })();
        if let Some(p) = pump {
            let mut s = p.finish();
            if let Some((built, t0)) = predict_world {
                s.install_world(&self.fs, map, built.map(|(cw, _)| cw), t0);
            }
            if let Some(c) = self.session.cod2_mut() {
                c.session = Some(s);
            }
        }
        if let Some(c) = self.session.cod2_mut() {
            c.draw.next_map();
        }
        loaded
    }

    /// Ends the CoD2 session, or the connect still running
    /// ([`Cod2Client::leave`]).
    pub(crate) fn leave_cod2(&mut self) {
        if let Some(c) = self.session.cod2_mut() {
            c.leave();
        }
    }
}

/// The connect screen's lines while a CoD2 connect runs: the address,
/// then the file downloading, `UI_DisplayDownloadInfo`'s `Downloading:`
/// line with its percentage and its `(<got> of <size> copied)`.
/// A session in the game downloading for its next map shows the second
/// line alone.
pub(crate) fn connect_lines(
    connecting: &Option<crate::play::cod2::Connecting>,
    session: Option<&Cod2Session>,
    localized: &vcod_common::localize::Localized,
) -> Vec<String> {
    let line = |p: &vcod_cod2::net::download::Progress| download_line(p, |k| localized.message(k));
    match (connecting, session) {
        (Some(c), _) => std::iter::once(format!("Connecting to {}...", c.addr))
            .chain(c.progress().as_ref().map(line))
            .collect(),
        (None, Some(s)) => s
            .download_progress()
            .as_ref()
            .map(line)
            .into_iter()
            .collect(),
        (None, None) => Vec::new(),
    }
}

/// The connect's options off the console: the userinfo dvars (`cl_voice`
/// among them), `cl_allowDownload`, and `fs`'s install as the homepath.
pub(crate) fn connect_opts(
    shell: &crate::console::shell::Shell,
    fs: &vcod_common::pk3::Pk3Fs,
) -> ConnectOpts {
    ConnectOpts {
        allow_download: shell.cvar_f32("cl_allowDownload") != 0.0,
        voice: shell.cvar_f32("cl_voice") != 0.0,
        ..ConnectOpts::new("", fs)
    }
    .with_userinfo(&shell.userinfo())
}

/// `Downloading: <file> (<n>%)  (<got> of <size> copied)`, the sizes as
/// `UI_ReadableSize` (Mac 0x151506) writes them; `loc` localizes `EXE_*`.
fn download_line(p: &vcod_cod2::net::download::Progress, loc: impl Fn(&str) -> String) -> String {
    let size = |n: u64| match n {
        0..=1024 => format!("{n} {}", loc("EXE_BYTES")),
        1025..=0x10_0000 => format!("{} {}", n >> 10, loc("EXE_KILOBYTE")),
        _ => format!(
            "{}.{:02} {}",
            n >> 20,
            ((n & 0xf_ffff) * 100) >> 20,
            loc("EXE_MEGABYTE")
        ),
    };
    let mut name = p.remote.clone();
    if let Some(pct) = (p.got * 100).checked_div(p.size) {
        name = format!("{name} ({pct}%)");
    }
    let copied = match p.size {
        0 => format!("({} {})", size(p.got), loc("EXE_COPIED")),
        n => format!(
            "({} {} {} {})",
            size(p.got),
            loc("EXE_OF"),
            size(n),
            loc("EXE_COPIED")
        ),
    };
    format!("{} {name}  {copied}", loc("EXE_DOWNLOADING"))
}

/// A `.d3dbsp` parsed into its file and world. A downloaded map is anyone's
/// bytes: a parse that panics is `path`'s error, not the client's end.
pub(crate) fn parse_map(data: &[u8], path: &str) -> Result<(vcod_cod2::map::MapFile, MapWorld)> {
    std::panic::catch_unwind(|| {
        let file = vcod_cod2::map::MapFile::parse(data)?;
        let bsp = file.world();
        anyhow::Ok((file, bsp))
    })
    .map_err(|_| anyhow::anyhow!("the parser panicked"))
    .and_then(|r| r)
    .with_context(|| format!("cannot parse {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map that is not one fails its load with the path, whatever its
    /// bytes, and the client stays up (TEST-050).
    #[test]
    fn a_hostile_map_fails_its_parse_not_the_client() {
        let mut seed = 0x5eed_u64;
        let junk: Vec<u8> = (0..4096)
            .map(|_| vcod_common::rng::xorshift(&mut seed) as u8)
            .collect();
        let mut head = b"IBSP\x04\0\0\0".to_vec();
        head.extend_from_slice(&junk);
        for bytes in [&[][..], &junk, &head] {
            let e = parse_map(bytes, "maps/mp/x.d3dbsp").err().expect("no map");
            assert!(format!("{e:#}").contains("maps/mp/x.d3dbsp"), "{e:#}");
        }
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let stock = fs.read("maps/mp/mp_toujane.d3dbsp").expect("mp_toujane");
        assert!(parse_map(&stock, "toujane").is_ok());
        for cut in [16, stock.len() / 3, stock.len() - 1] {
            assert!(parse_map(&stock[..cut], "toujane").is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn the_download_line_reads_as_retails() {
        let p = vcod_cod2::net::download::Progress {
            remote: "main/z_dltest.iwd".into(),
            got: 3 << 20,
            size: 12 << 20,
        };
        let en = |k: &str| {
            match k {
                "EXE_DOWNLOADING" => "Downloading:",
                "EXE_OF" => "of",
                "EXE_COPIED" => "copied",
                "EXE_MEGABYTE" => "MB",
                "EXE_KILOBYTE" => "KB",
                _ => "bytes",
            }
            .to_string()
        };
        assert_eq!(
            download_line(&p, en),
            "Downloading: main/z_dltest.iwd (25%)  (3.00 MB of 12.00 MB copied)"
        );
        let early = vcod_cod2::net::download::Progress {
            remote: "main/z.iwd".into(),
            got: 512,
            size: 0,
        };
        assert_eq!(
            download_line(&early, en),
            "Downloading: main/z.iwd  (512 bytes copied)"
        );
    }
}
