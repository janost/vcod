//! `--net-probe`, the headless spectate client. Lives in the client crate
//! because cue resolution needs the audio alias tables.

use std::time::{Duration, Instant};
use vcod_common::net::{self, NetClient, NetEvent, NetState, SnapshotCapture, UdpTransport};

/// Committed fixtures are absolute (the common crate's parser tests read
/// them); scratch paths are cwd-relative.
const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../common/tests/fixtures/net/gamestate.bin"
);
const SCRATCH_PATH: &str = "tmp/gamestate.bin";
const SNAP_FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../common/tests/fixtures/net/snapshots.bin"
);
const SNAP_SCRATCH_PATH: &str = "tmp/snapshots.bin";
/// How many snapshot-bearing messages to capture before a fixture run stops.
const SNAP_CAPTURE_TARGET: usize = 24;

/// Connects, drives the client to `Active`, prints a summary per second and
/// nudges forward now and then to prove the server applies moves. With `fs`
/// it resolves drained events into sound cues; weapon-file cues (fire,
/// reload) are not resolved, the probe loads no weapon files.
///
/// `save_playerstate` turns it into the stage 4 capture instead: it sends
/// `begin`, answers the stock team and weapon menus, writes the fixture once
/// the spawn has settled and exits. It sends no nudge, which would walk the
/// capture off the spawn point.
pub fn probe(
    addr: &str,
    save_fixture: bool,
    save_snapshots: bool,
    save_configstrings: bool,
    save_playerstate: bool,
    secs: u64,
    fs: Option<&vcod_common::pk3::Pk3Fs>,
) -> anyhow::Result<()> {
    let mut client = NetClient::connect(addr)?;
    if save_fixture || save_snapshots {
        client.enable_capture();
    }
    let mut quick_chat = crate::quick_chat::QuickChat::new(0x51ee);

    let start = Instant::now();
    let mut last_summary = start;
    let mut reached_active: Option<Instant> = None;
    let mut baseline_origin: Option<[f32; 3]> = None;
    let mut watch = ProbeWatch::default();
    let mut join = JoinProbe::default();
    let mut wrote_playerstate = false;
    // The full table; the map's loadspec filters it at gamestate.
    let aliases_all = fs.map(crate::audio::alias::AliasTable::load);

    loop {
        let now = Instant::now();
        for e in client.pump_at(now) {
            match e {
                NetEvent::GamestateReady => {
                    let gs = client.gamestate().unwrap();
                    println!("systeminfo: {}", gs.configstrings[1]);
                    println!(
                        "gamestate ready: clientNum {}, serverId {}, {} configstrings, {} baselines",
                        gs.client_num,
                        net::server_id_from_gamestate(gs),
                        gs.configstrings.iter().filter(|s| !s.is_empty()).count(),
                        gs.baselines.len(),
                    );
                    // The research configstring tables were taken from this dump.
                    for (i, cs) in client.configstrings().iter().enumerate() {
                        if !cs.is_empty() {
                            log::debug!("cs[{i}] = {cs:?}");
                        }
                    }
                    if save_configstrings {
                        write_configstrings_fixture(client.configstrings())?;
                    }
                    for (i, m) in gs.configstrings.iter().enumerate().skip(269).take(255) {
                        if !m.is_empty() {
                            println!("model {}: {m}", i - 268);
                        }
                    }
                    // docs/research/cod11-sound-system.md, section 9.
                    println!("cs 3 (ambient): {:?}", gs.configstrings.get(3));
                    let aliases: Vec<(usize, &String)> = gs
                        .configstrings
                        .iter()
                        .enumerate()
                        .skip(CS_SOUNDS)
                        .take(CS_SOUNDS_COUNT)
                        .filter(|(_, s)| !s.is_empty())
                        .collect();
                    println!(
                        "sound alias configstrings ({CS_SOUNDS}..{}): {} set",
                        CS_SOUNDS + CS_SOUNDS_COUNT,
                        aliases.len()
                    );
                    for (i, s) in &aliases {
                        println!("  cs {i} (alias {}): {s}", i - CS_SOUNDS);
                    }
                    if let Some(all) = &aliases_all {
                        let map = net::info_value_for_key(&gs.configstrings[0], "mapname")
                            .unwrap_or_default()
                            .to_string();
                        watch.aliases = all.for_map(&map);
                        println!(
                            "sound aliases: {} of {} apply on {map:?}",
                            watch.aliases.len(),
                            all.len()
                        );
                    }
                    if save_playerstate {
                        // `begin` is what releases `Callback_PlayerConnect`'s
                        // `waittill`; the join menus follow from it.
                        client.send_reliable("begin");
                    } else {
                        client.send_reliable("say hello from vcod");
                    }
                }
                NetEvent::Chat { text, .. } => println!("chat: {}", net::strip_colors(&text)),
                NetEvent::Print(t) => print!("print: {t}"),
                NetEvent::Dropped(r) => {
                    println!("dropped: {r}");
                    return finish_probe(client, save_fixture, save_snapshots);
                }
                NetEvent::ConfigstringChanged(i) => {
                    println!("configstring {i} changed");
                    if i == 3 {
                        // `n\\<alias>\\t\\<fade end time>`.
                        println!("cs 3 (ambient): {:?}", client.configstrings().get(3));
                    } else if (CS_SOUNDS..CS_SOUNDS + CS_SOUNDS_COUNT).contains(&i) {
                        println!(
                            "cs {i} (alias {}): {:?}",
                            i - CS_SOUNDS,
                            client.configstrings().get(i)
                        );
                    }
                }
                NetEvent::DownloadComplete(n) => println!("downloaded {n}"),
                NetEvent::ServerCommand(tokens) => {
                    // `b` is the scoreboard, one long line per second at round end.
                    if tokens.first().map(String::as_str) == Some("b") {
                        continue;
                    }
                    println!("serverCommand: {tokens:?}");
                    if save_playerstate {
                        join.on_server_command(&tokens, &mut client, now);
                    }
                    // `s <idx>` is playLocalSound (sound doc, section 9).
                    if tokens.first().map(String::as_str) == Some("s") {
                        match tokens.get(1).and_then(|t| t.parse::<i32>().ok()) {
                            Some(idx) => match sound_cs_alias(client.configstrings(), idx) {
                                Some((cs, name)) => {
                                    println!("  playLocalSound: cs {cs} = {name:?}")
                                }
                                None => println!("  playLocalSound: index {idx}: no alias"),
                            },
                            None => {
                                println!("  playLocalSound: <unparsed index {:?}>", tokens.get(1))
                            }
                        }
                    }
                    // Quick chat (`j/k/l`): resolve the category against the
                    // `.voice` tables when they exist; stock installs ship none.
                    if let Some(fs) = fs {
                        let newest = client.snapshots().newest();
                        let protocol = &net::protocol::PROTOCOL_V1;
                        let handled = quick_chat.on_server_command(fs, &tokens, |num| {
                            newest
                                .and_then(|s| s.clients.get(&num))
                                .map(|c| c.field_i32(protocol, "team"))
                        });
                        if handled {
                            while let Some(line) = quick_chat.drain(f32::INFINITY) {
                                println!(
                                    "  quickchat: {:?} -> {:?} ({line_text})",
                                    line.cue.source,
                                    line.cue.alias,
                                    line_text = line.text
                                );
                            }
                        }
                    }
                }
            }
        }

        if save_playerstate {
            for cmd in client.take_server_commands() {
                println!("JOIN cmd: {cmd}");
                join.commands.push(cmd);
            }
        }

        // 5 s after going active push forward for 2 s, again every 30 s, so a
        // long run shows whether moves still apply after a map_restart.
        let mut cmd = net::msg::UserCmd::default();
        if client.state() == NetState::Active && !save_playerstate {
            let active_at = *reached_active.get_or_insert(now);
            let dt = now.duration_since(active_at).as_secs() % 30;
            if (5..7).contains(&dt) {
                cmd.forward = 127;
            }
        }
        client.send_frame(&cmd);

        // Every iteration, not once a second; the event rings hold four slots
        // and `loopSound` can come and go between summaries.
        if let Some(s) = client.snapshots().newest() {
            watch.check_sounds(s, client.configstrings());
        }

        if now.duration_since(last_summary) >= Duration::from_secs(1) {
            last_summary = now;
            if let Some(s) = client.snapshots().newest() {
                let o = s.ps.origin(&net::protocol::PROTOCOL_V1);
                let base = *baseline_origin.get_or_insert(o);
                let moved = ((o[0] - base[0]).powi(2)
                    + (o[1] - base[1]).powi(2)
                    + (o[2] - base[2]).powi(2))
                .sqrt();
                // `cmdLag` is serverTime minus the echoed ps.commandTime: how
                // far behind the server's execution of our usercmds runs. A
                // healthy live connection stays under ~100 ms.
                println!(
                    "[{:>3}s] {:?} sid={} snap #{} delta={} t={} cmdLag={} ps.origin=[{:.0},{:.0},{:.0}] moved {:.0}u, {} entities",
                    now.duration_since(start).as_secs(),
                    client.state(),
                    client.server_id(),
                    s.message_num,
                    s.delta_num,
                    s.server_time,
                    s.server_time - s.ps.field_i32(&net::protocol::PROTOCOL_V1, "commandTime"),
                    o[0],
                    o[1],
                    o[2],
                    moved,
                    s.entities.len(),
                );
                watch.check(s, client.configstrings());
            } else {
                println!(
                    "[{:>3}s] {:?}, no snapshot yet",
                    now.duration_since(start).as_secs(),
                    client.state()
                );
            }
        }

        // A refused weapon reopens the same menu, which the probe answers
        // once and then ignores, so a sent answer is not an accepted one; the
        // playerstate is what tells a spawn from a still-spectating client.
        if join.settled(now) {
            if let Some(s) = client.snapshots().newest() {
                if s.ps.field_i32(&net::protocol::PROTOCOL_V1, "pm_type") == PM_NORMAL {
                    write_playerstate_fixture(s, client.configstrings(), &join)?;
                    wrote_playerstate = true;
                    break;
                }
            }
        }
        if let Some(count) = client.capture_count() {
            if count >= SNAP_CAPTURE_TARGET {
                break;
            }
        }
        if now.duration_since(start) >= Duration::from_secs(secs) {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }

    if save_playerstate && !wrote_playerstate {
        let pm_type = client
            .snapshots()
            .newest()
            .map(|s| s.ps.field_i32(&net::protocol::PROTOCOL_V1, "pm_type"));
        println!(
            "no playerstate fixture: the join never completed (menus answered: {:?}, pm_type {pm_type:?}); \
             a spectator pm_type means the weapon answer was refused",
            join.answered
        );
    }
    finish_probe(client, save_fixture, save_snapshots)
}

/// Per-second checks over the newest snapshot, for chasing wrong-model and
/// sound-path reports headlessly.
#[derive(Default)]
struct ProbeWatch {
    client_models: std::collections::HashMap<u32, i32>,
    /// Live eType-2 corpses: entity -> (clientNum, first-seen serverTime).
    corpses: std::collections::HashMap<u32, (i32, i32)>,
    prop_origins: std::collections::HashMap<u32, (String, [f32; 3])>,
    loop_sounds: std::collections::HashMap<u32, i32>,
    events: vcod_common::net::events::EventTracker,
    /// Empty until the gamestate names the map, or when there is no game
    /// data; the audio line stays quiet then.
    aliases: crate::audio::alias::AliasTable,
    /// Since the last audio line.
    cues: u64,
    misses: u64,
    first_miss: Option<String>,
}

impl ProbeWatch {
    fn check(&mut self, s: &net::snapshot::Snapshot, configstrings: &[String]) {
        use crate::entities::{resolve_visual, EntityVisual, ET_GENERAL};
        let p = &net::protocol::PROTOCOL_V1;
        if !self.aliases.is_empty() {
            println!(
                "audio: {} cues, {} alias misses{}",
                self.cues,
                self.misses,
                match self.first_miss.take() {
                    Some(m) => format!(" (first: {m})"),
                    None => String::new(),
                }
            );
            self.cues = 0;
            self.misses = 0;
        }
        for (&num, cl) in &s.clients {
            let mi = cl.field_i32(p, "modelindex");
            if let Some(old) = self.client_models.insert(num, mi) {
                if old != mi {
                    let name = |i: i32| {
                        configstrings
                            .get(268 + i as usize)
                            .cloned()
                            .unwrap_or_default()
                    };
                    println!(
                        "client {num}: modelindex {old} ({}) -> {mi} ({})",
                        name(old),
                        name(mi)
                    );
                }
            }
        }
        // Corpse lifecycle (1 Hz, so times are +-1 s): a corpse resolves
        // through the dead client's roster entry, so log appear/vanish with
        // that entry's modelindex - the evidence for how long the wire keeps
        // bodies and whether the roster clears first.
        let mi_of = |cn: i32| {
            s.clients
                .get(&(cn as u32))
                .map_or(-1, |c| c.field_i32(p, "modelindex"))
        };
        self.corpses.retain(|&num, &mut (cn, t0)| {
            if s.entities.contains_key(&num) {
                return true;
            }
            println!(
                "corpse {num} gone after {} ms (clientNum {cn} modelindex {})",
                s.server_time - t0,
                mi_of(cn)
            );
            false
        });
        for (&num, ent) in &s.entities {
            if ent.field_i32(p, "eType") == crate::entities::ET_CORPSE {
                let cn = ent.field_i32(p, "clientNum");
                self.corpses.entry(num).or_insert_with(|| {
                    println!(
                        "corpse {num} appeared clientNum {cn} modelindex {}",
                        mi_of(cn)
                    );
                    (cn, s.server_time)
                });
                // The trajectory decides whether a closed-form evaluation sinks
                // the body: TR_GRAVITY with a frozen trTime drops z by
                // 400*age^2 per second.
                let tr = net::trajectory::Trajectory::read(ent, p, "pos");
                let eval = tr.evaluate(s.server_time);
                println!(
                    "corpse {num} trType {} trTime {} (age {} ms) base z {:.0} delta {:?} eval z {:.0}",
                    tr.tr_type,
                    tr.tr_time,
                    s.server_time - tr.tr_time,
                    tr.base.z,
                    tr.delta,
                    eval.z
                );
            }
        }
        for (&num, ent) in &s.entities {
            let etype = ent.field_i32(p, "eType");
            match resolve_visual(ent, &s.clients, configstrings, p) {
                EntityVisual::Player { body, .. } if !body.contains("playerbody") => {
                    println!(
                        "entity {num} eType {etype} clientNum {} index {} resolves to {body}",
                        ent.field_i32(p, "clientNum"),
                        ent.field_i32(p, "index")
                    );
                }
                EntityVisual::Model(m) if etype == ET_GENERAL => {
                    let o = ent.origin(p);
                    if let Some((old_m, old_o)) = self.prop_origins.insert(num, (m.clone(), o)) {
                        let d = ((o[0] - old_o[0]).powi(2)
                            + (o[1] - old_o[1]).powi(2)
                            + (o[2] - old_o[2]).powi(2))
                        .sqrt();
                        if old_m != m || d > 8.0 {
                            println!(
                                "entity {num} ET_GENERAL index {}: {old_m} @[{:.0},{:.0},{:.0}] -> {m} @[{:.0},{:.0},{:.0}]",
                                ent.field_i32(p, "index"), old_o[0], old_o[1], old_o[2], o[0], o[1], o[2]
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// `EventTracker::drain` is idempotent per `message_num`, so re-reading
    /// the same snapshot is free.
    fn check_sounds(&mut self, s: &net::snapshot::Snapshot, configstrings: &[String]) {
        let p = &net::protocol::PROTOCOL_V1;
        for (&num, ent) in &s.entities {
            let ls = ent.field_i32(p, "loopSound");
            let old = self.loop_sounds.insert(num, ls);
            // Starts and stops only; most entities sit at 0.
            if old != Some(ls) && (ls != 0 || old.is_some_and(|o| o != 0)) {
                let o = ent.origin(p);
                println!(
                    "entity {num} loopSound {} -> {ls} {} at [{:.0},{:.0},{:.0}]",
                    old.unwrap_or(0),
                    sound_cs_alias_paren(configstrings, ls),
                    o[0],
                    o[1],
                    o[2]
                );
            }
        }
        // Forget entities that left, so a reused slot reports again.
        self.loop_sounds.retain(|n, _| s.entities.contains_key(n));

        let ps_client = s.ps.field_i32(p, "clientNum");
        // No weapon cache or drawn entities headlessly; fire, reload and
        // whizby cues resolve to nothing.
        let no_weapons = Default::default();
        let no_muzzles = Default::default();
        let ctx = crate::audio::cues::CueCtx {
            configstrings,
            weapon_sounds: &no_weapons,
            muzzles: &no_muzzles,
            listener_pos: glam::Vec3::from(s.ps.origin(p)),
            ps_entity: crate::audio::cues::ps_entity(ps_client),
        };
        for ev in self.events.drain(s, p) {
            for cue in crate::audio::cues::resolve(&ev, &ctx) {
                self.cues += 1;
                if self.aliases.get(&cue.alias).is_none() {
                    self.misses += 1;
                    self.first_miss.get_or_insert(cue.alias);
                }
            }
            if ev.event != crate::fx::registry::EV_SOUND_ALIAS {
                continue;
            }
            let who = if ev.entity_num == u32::MAX {
                "playerState ring (the followed player)".to_string()
            } else {
                format!("entity {}", ev.entity_num)
            };
            println!(
                "EV_SOUND_ALIAS {who} clientNum {} parm {} {} at [{:.0},{:.0},{:.0}]",
                ev.client_num,
                ev.parm,
                sound_cs_alias_paren(configstrings, ev.parm),
                ev.pos[0],
                ev.pos[1],
                ev.pos[2]
            );
        }
    }
}

/// `CS_SOUNDS` (sound doc, section 9); index 0 in the block is "no sound".
const CS_SOUNDS: usize = 524;
/// `G_SoundAliasIndex`'s hard limit.
const CS_SOUNDS_COUNT: usize = 256;

/// `None` when the index names no alias (0 is "no sound", a garbled command
/// can carry anything), so callers print the raw index.
fn sound_cs_alias(configstrings: &[String], idx: i32) -> Option<(usize, &str)> {
    if idx <= 0 || idx as usize >= CS_SOUNDS_COUNT {
        return None;
    }
    let cs = CS_SOUNDS + idx as usize;
    Some((cs, configstrings.get(cs).map_or("<unset>", String::as_str)))
}

fn sound_cs_alias_paren(configstrings: &[String], idx: i32) -> String {
    match sound_cs_alias(configstrings, idx) {
        Some((cs, name)) => format!("(cs {cs}) = {name:?}"),
        None => format!("(index {idx}: no alias)"),
    }
}

/// `save_fixture` writes gamestate.bin; `save_snapshots` writes snapshots.bin
/// only and sends the gamestate capture to tmp/, so the pinned gamestate is
/// safe to refresh snapshots against.
fn finish_probe(
    mut client: NetClient<UdpTransport>,
    save_fixture: bool,
    save_snapshots: bool,
) -> anyhow::Result<()> {
    client.disconnect();
    let Some(cap): Option<SnapshotCapture> = client.take_capture() else {
        return Ok(());
    };
    println!(
        "captured {} snapshot message(s); serverTime {:?}..{:?}",
        cap.count,
        cap.times.first(),
        cap.times.last()
    );
    if let Some(gs_bytes) = client.captured_gamestate() {
        let path = if save_fixture {
            FIXTURE_PATH
        } else {
            SCRATCH_PATH
        };
        write_fixture(path, gs_bytes)?;
        println!("gamestate: {} bytes -> {path}", gs_bytes.len());
    }
    if cap.count == 0 {
        println!("no snapshots captured");
        return Ok(());
    }
    let path = if save_fixture || save_snapshots {
        SNAP_FIXTURE_PATH
    } else {
        SNAP_SCRATCH_PATH
    };
    write_fixture(path, &cap.triples)?;
    println!(
        "snapshots: {} message(s), {} bytes -> {path}",
        cap.count,
        cap.triples.len()
    );
    if !save_fixture && !save_snapshots {
        println!("(pass --save-snapshots to replace the committed snapshot fixture)");
    }
    Ok(())
}

fn write_fixture(path: &str, data: &[u8]) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, data)?;
    Ok(())
}

/// Directory `crates/server/tests/configstrings_ab.rs` reads its fixtures
/// from.
const CONFIGSTRINGS_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/configstrings"
);

/// Writes the non-empty configstring table to
/// `<map>-<gametype>.txt`. The map, gametype, client count and pureness come
/// out of cs 0 (serverinfo) itself, so nothing here is hand-typed.
fn write_configstrings_fixture(configstrings: &[String]) -> anyhow::Result<()> {
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
    let map = key("mapname");
    let gametype = key("g_gametype");
    let max_clients = key("sv_maxclients");
    let pure = key("sv_pure");

    // Values are raw: a configstring cannot contain a newline, so the
    // `<index> <value>` line format needs no escaping.
    let mut out = String::new();
    out.push_str("# Retail CoD 1.1d dedicated server configstring table at gamestate.\n");
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, sv_maxclients {max_clients}, sv_pure {pure}, dedicated 1, stock scr_* defaults.\n"
    ));
    out.push_str("# Captured with tools/run_server.sh and --net-probe at debug level.\n");
    out.push_str("# One '<index> <value>' line per non-empty slot, ascending. Values are raw;\n");
    out.push_str("# a configstring cannot contain a newline, so no escaping is needed.\n");
    out.push_str("# 0 (serverinfo) and 1 (systeminfo) are recorded for provenance and excluded\n");
    out.push_str(
        "# from the diff: they carry pak checksums and server config, not script output.\n",
    );
    let mut written = 0;
    for (i, cs) in configstrings.iter().enumerate() {
        if !cs.is_empty() {
            out.push_str(&format!("{i} {cs}\n"));
            written += 1;
        }
    }

    let path = format!("{CONFIGSTRINGS_FIXTURE_DIR}/{map}-{gametype}.txt");
    std::fs::write(&path, out)?;
    println!("configstrings: {written} non-empty slots -> {path}");
    Ok(())
}

/// Directory `crates/server/tests/playerstate_ab.rs` reads its fixtures from.
const PLAYERSTATE_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/playerstate"
);

/// The team `--save-playerstate` joins; the stage gate is a client joining
/// allies through the real menu.
const JOIN_TEAM: &str = "allies";

/// How long after the weapon answer the capture is taken, so the spawn has
/// landed and the drop to the floor has finished.
const SPAWN_SETTLE: Duration = Duration::from_secs(3);

/// `pmove_t`'s first entry, the state a spawned player is in; a client still
/// on the menus sits at 4, the spectator's.
const PM_NORMAL: i32 = 0;

/// Drives the stock team/weapon menu handshake under `--save-playerstate` and
/// keeps what the fixture header needs.
#[derive(Default)]
struct JoinProbe {
    /// The menu retail last named in `v g_scriptMainMenu`; the `t` that follows
    /// opens it.
    main_menu: String,
    /// Menu indices already answered, so a reopened menu is not answered twice.
    answered: Vec<i32>,
    weapon: String,
    /// When the weapon answer went out.
    answered_weapon: Option<Instant>,
    /// Every serverCommand since the gamestate, verbatim and in order.
    commands: Vec<String>,
}

impl JoinProbe {
    /// `v g_scriptMainMenu <menu>` names the menu, `t <index>` opens it, and
    /// `mr <serverId> <index> <response>` answers the index `t` named
    /// (docs/research/cod11-hud-protocol.md, section 0.1).
    fn on_server_command(
        &mut self,
        tokens: &[String],
        client: &mut NetClient<UdpTransport>,
        now: Instant,
    ) {
        match tokens.first().map(String::as_str) {
            Some("v") if tokens.get(1).map(String::as_str) == Some("g_scriptMainMenu") => {
                self.main_menu = tokens.get(2).cloned().unwrap_or_default();
            }
            Some("t") => {
                let Some(idx) = tokens.get(1).and_then(|t| t.parse::<i32>().ok()) else {
                    return;
                };
                if self.answered.contains(&idx) {
                    return;
                }
                let Some(reply) = menu_reply(&self.main_menu) else {
                    println!(
                        "JOIN: menu {idx} ({:?}) has no scripted reply",
                        self.main_menu
                    );
                    return;
                };
                println!(
                    "JOIN: answering menu {idx} ({}) with {reply}",
                    self.main_menu
                );
                client.send_reliable(&format!("mr {} {idx} {reply}", client.server_id()));
                self.answered.push(idx);
                if self.main_menu.starts_with("weapon_") {
                    self.weapon = reply.to_string();
                    self.answered_weapon = Some(now);
                }
            }
            _ => {}
        }
    }

    fn settled(&self, now: Instant) -> bool {
        self.answered_weapon
            .is_some_and(|t| now.duration_since(t) >= SPAWN_SETTLE)
    }
}

/// The team menu takes a team; the weapon menu takes a weapon `_teams::restrict`
/// allows for that menu's nationality, which is why one weapon literal cannot
/// serve both gate maps. All four are on under the stock `scr_allow_*` defaults.
fn menu_reply(menu: &str) -> Option<&'static str> {
    if menu.starts_with("team_") {
        return Some(JOIN_TEAM);
    }
    match menu.strip_prefix("weapon_")? {
        "american" => Some("m1carbine_mp"),
        "british" => Some("enfield_mp"),
        "russian" => Some("mosin_nagant_mp"),
        "german" => Some("kar98k_mp"),
        _ => None,
    }
}

/// Writes the spawned player's wire state to `<map>-<gametype>.txt`. The map
/// and gametype come out of cs 0 (serverinfo), so nothing here is hand-typed.
fn write_playerstate_fixture(
    snap: &net::snapshot::Snapshot,
    configstrings: &[String],
    join: &JoinProbe,
) -> anyhow::Result<()> {
    let p = &net::protocol::PROTOCOL_V1;
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
    let map = key("mapname");
    let gametype = key("g_gametype");

    let mut out = String::new();
    out.push_str("# Retail CoD 1.1d dedicated server state after a stock menu join.\n");
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {JOIN_TEAM}, weapon {}, dedicated 1,\n",
        join.weapon
    ));
    out.push_str("# sv_maxclients 8, sv_pure 0, stock scr_* defaults, one client on the server.\n");
    out.push_str("# Captured with tools/run_server.sh and --net-probe --save-playerstate,\n");
    out.push_str(&format!(
        "# {} s after the weapon menu was answered; snapshot #{}, serverTime {}.\n",
        SPAWN_SETTLE.as_secs(),
        snap.message_num,
        snap.server_time
    ));
    out.push_str("# Values are the raw i32 wire words, floats as their bit patterns: the gate\n");
    out.push_str("# compares bits and a rendered float loses them.\n");
    out.push_str("# [playerstate] is Protocol::player_fields order; [entity] is the probe's\n");
    out.push_str("# own client entity in Protocol::entity_fields order, or a comment saying so\n");
    out.push_str("# when retail sent none; [servercommands] is every serverCommand retail sent\n");
    out.push_str("# after the gamestate, verbatim and in order.\n");

    out.push_str("[playerstate]\n");
    for (f, v) in p.player_fields.iter().zip(&snap.ps.fields) {
        out.push_str(&format!("{} {v}\n", f.name));
    }

    out.push_str("[entity]\n");
    let self_num = snap.ps.field_i32(p, "clientNum") as u32;
    let self_ent = snap.entities.get(&self_num);
    match self_ent {
        Some(ent) => {
            for (f, v) in p.entity_fields.iter().zip(&ent.fields) {
                out.push_str(&format!("{} {v}\n", f.name));
            }
        }
        None => out.push_str("# retail sent no self-entity\n"),
    }

    out.push_str("[servercommands]\n");
    for cmd in &join.commands {
        out.push_str(cmd);
        out.push('\n');
    }

    let path = format!("{PLAYERSTATE_FIXTURE_DIR}/{map}-{gametype}.txt");
    std::fs::create_dir_all(PLAYERSTATE_FIXTURE_DIR)?;
    std::fs::write(&path, out)?;
    println!(
        "playerstate: {} fields, {}, {} serverCommands -> {path}",
        snap.ps.fields.len(),
        match self_ent {
            Some(_) => "self-entity present",
            None => "no self-entity",
        },
        join.commands.len(),
    );
    Ok(())
}
