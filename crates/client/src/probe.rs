//! `--net-probe`, the headless spectate client. Lives in the client crate
//! because cue resolution needs the audio alias tables.

use std::collections::BTreeMap;
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
/// `save_playerstate` turns it into the stage 4 capture instead: it answers
/// the stock team and weapon menus, writes the fixture once the spawn has
/// settled and exits. It sends no nudge, which would walk the capture off the
/// spawn point.
///
/// `save_motion` joins the same way, then holds each movement input in turn
/// and captures the playerstate settled under each, which is what a standing
/// capture cannot show: every field the client predicts reads zero while the
/// player stands still.
///
/// `pvs` joins the same way again, walks the route in [`pvs_route`] and prints
/// the snapshot entity list at each station plus every add and removal along
/// the way, answering what the server sends from where. `save.entities` walks
/// the same route and writes the trace as the stage 5 fixture; `pvs` alone
/// writes nothing.
/// Which captures a probe run overwrites. Each is a separate flag because
/// each pins a different thing; the flag docs in `main.rs` say why.
#[derive(Clone, Copy, Default)]
pub struct Save {
    pub fixture: bool,
    pub snapshots: bool,
    pub configstrings: bool,
    pub playerstate: bool,
    pub motion: bool,
    pub entities: bool,
}

pub fn probe(
    addr: &str,
    save: Save,
    tag: Option<String>,
    pvs: bool,
    team: Option<&str>,
    secs: u64,
    fs: Option<&vcod_common::pk3::Pk3Fs>,
) -> anyhow::Result<()> {
    let Save {
        fixture: save_fixture,
        snapshots: save_snapshots,
        configstrings: save_configstrings,
        playerstate: save_playerstate,
        motion: save_motion,
        entities: save_entities,
    } = save;
    // The fixture is the route's output, so the capture drives the same walk.
    let pvs = pvs || save_entities;
    // Every mode that needs a spawned player drives the same stock-menu join;
    // `--probe-team` alone joins and then just watches the roster.
    let joining = save_playerstate || save_motion || pvs || team.is_some();
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
    let mut join = JoinProbe::new(team);
    let mut wrote_playerstate = false;
    let mut motion = MotionProbe::default();
    let mut pvs_probe = PvsProbe::default();
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
                    // The join is entered by the first usercmd of the loop
                    // below, the way a retail client enters; nothing is sent
                    // here to start it.
                    if !joining {
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
                    if joining {
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

        if joining {
            for cmd in client.take_server_commands() {
                println!("JOIN cmd: {cmd}");
                join.commands.push(cmd);
            }
        }

        // 5 s after going active push forward for 2 s, again every 30 s, so a
        // long run shows whether moves still apply after a map_restart.
        let mut cmd = net::msg::UserCmd::default();
        if client.state() == NetState::Active && !joining {
            let active_at = *reached_active.get_or_insert(now);
            let dt = now.duration_since(active_at).as_secs() % 30;
            if (5..7).contains(&dt) {
                cmd.forward = 127;
            }
        }
        if save_motion && motion.running() {
            cmd = motion.cmd();
            hold_view_yaw(&mut cmd, &client, &mut motion.spawn_delta_yaw);
        } else if pvs && pvs_probe.running() {
            cmd = pvs_probe.cmd();
            hold_view_yaw(&mut cmd, &client, &mut pvs_probe.spawn_delta_yaw);
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
                if !s.clients.is_empty() {
                    let p = &net::protocol::PROTOCOL_V1;
                    let roster: Vec<String> = s
                        .clients
                        .iter()
                        .map(|(num, c)| {
                            format!("{num}:team={} {:?}", c.field_i32(p, "team"), c.name(p))
                        })
                        .collect();
                    println!("  roster: {}", roster.join("  "));
                }
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
                    if save_motion {
                        if motion.step(now, s) {
                            write_motion_fixture(s, client.configstrings(), &join, &motion)?;
                            wrote_playerstate = true;
                            break;
                        }
                    } else if pvs {
                        if pvs_probe.step(now, s) {
                            pvs_probe.report();
                            if save_entities {
                                write_entities_fixture(
                                    &pvs_probe,
                                    client.configstrings(),
                                    &join,
                                    tag.as_deref(),
                                )?;
                            }
                            break;
                        }
                    } else if save_playerstate {
                        write_playerstate_fixture(s, client.configstrings(), &join)?;
                        wrote_playerstate = true;
                        break;
                    }
                    // `--probe-team` on its own writes nothing: it stays on
                    // and reports the roster, which is what a second probe on
                    // the other team needs to be seen by.
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

/// The team a capture joins when `--probe-team` names none; the stage gates
/// were all taken with a client joining allies through the real menu.
const DEFAULT_JOIN_TEAM: &str = "allies";

/// How long after the weapon answer the capture is taken, so the spawn has
/// landed and the drop to the floor has finished.
const SPAWN_SETTLE: Duration = Duration::from_secs(3);

/// `pmove_t`'s first entry, the state a spawned player is in; a client still
/// on the menus sits at 4, the spectator's.
const PM_NORMAL: i32 = 0;

/// Drives the stock team/weapon menu handshake under `--save-playerstate` and
/// keeps what the fixture header needs.
struct JoinProbe {
    /// What the team menu is answered with: `allies`, `axis`, `autoassign`
    /// or `spectator`, the four the stock gametypes accept.
    team: String,
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
    fn new(team: Option<&str>) -> Self {
        Self {
            team: team.unwrap_or(DEFAULT_JOIN_TEAM).to_string(),
            main_menu: String::new(),
            answered: Vec::new(),
            weapon: String::new(),
            answered_weapon: None,
            commands: Vec::new(),
        }
    }

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
                let Some(reply) = menu_reply(&self.main_menu, &self.team) else {
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
fn menu_reply<'a>(menu: &str, team: &'a str) -> Option<&'a str> {
    if menu.starts_with("team_") {
        return Some(team);
    }
    match menu.strip_prefix("weapon_")? {
        "american" => Some("m1carbine_mp"),
        "british" => Some("enfield_mp"),
        "russian" => Some("mosin_nagant_mp"),
        "german" => Some("kar98k_mp"),
        _ => None,
    }
}

/// One held input and the label the playerstate settled under it is captured
/// under.
struct MotionStep {
    label: &'static str,
    cmd: net::msg::UserCmd,
    until: Until,
}

/// When a step's capture is taken.
enum Until {
    /// After the input has been held this long, so the pose has settled and
    /// the value is a steady state rather than a point on a lerp.
    Held(Duration),
    /// At the first snapshot off the ground, which is what puts the jump's
    /// takeoff velocity in the capture. The duration bounds the wait.
    Airborne(Duration),
}

/// The poses, in an order where each starts from the one before: lean from
/// standing, prone from crouched, run from standing again. Bit names:
/// docs/protocol-1.1.md, "Usercmd input bits".
fn motion_script() -> Vec<MotionStep> {
    use net::msg::{
        NULL_USERCMD, WBUTTON_CROUCH, WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT, WBUTTON_PRONE,
    };
    let held = |label, cmd, ms| MotionStep {
        label,
        cmd,
        until: Until::Held(Duration::from_millis(ms)),
    };
    vec![
        held("stand", NULL_USERCMD, 1500),
        held(
            "lean_left",
            net::msg::UserCmd {
                wbuttons: WBUTTON_LEAN_LEFT,
                ..NULL_USERCMD
            },
            2000,
        ),
        // Between the leans, so the capture shows whether a released lean
        // returns to centre server-side.
        held("center", NULL_USERCMD, 1500),
        held(
            "lean_right",
            net::msg::UserCmd {
                wbuttons: WBUTTON_LEAN_RIGHT,
                ..NULL_USERCMD
            },
            2000,
        ),
        // A crouched or prone client holds `up` at -127 for as long as it is
        // down, so the capture holds it too.
        held(
            "crouch",
            net::msg::UserCmd {
                wbuttons: WBUTTON_CROUCH,
                up: -127,
                ..NULL_USERCMD
            },
            2500,
        ),
        // Standing between the two lowered stances: a prone held straight out
        // of a crouch was refused in one capture and taken in another.
        held("stand_between", NULL_USERCMD, 1500),
        held(
            "prone",
            net::msg::UserCmd {
                wbuttons: WBUTTON_PRONE,
                up: -127,
                ..NULL_USERCMD
            },
            3000,
        ),
        // Turning the view while already prone: this is what separates a
        // `proneDirection` the server writes from one it leaves at zero, and
        // it is where `bg_prone_yawcap` (85 degrees) bites.
        held(
            "prone_yaw_60",
            net::msg::UserCmd {
                angles: [0, 60 * 65536 / 360, 0],
                wbuttons: WBUTTON_PRONE,
                up: -127,
                forward: 0,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "prone_yaw_150",
            net::msg::UserCmd {
                angles: [0, 150 * 65536 / 360, 0],
                wbuttons: WBUTTON_PRONE,
                up: -127,
                forward: 0,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "prone_crawl_150",
            net::msg::UserCmd {
                angles: [0, 150 * 65536 / 360, 0],
                wbuttons: WBUTTON_PRONE,
                up: -127,
                forward: 127,
                ..NULL_USERCMD
            },
            2000,
        ),
        held("stand_up", NULL_USERCMD, 2500),
        held(
            "run_forward",
            net::msg::UserCmd {
                forward: 127,
                ..NULL_USERCMD
            },
            2000,
        ),
        MotionStep {
            label: "jump_takeoff",
            cmd: net::msg::UserCmd {
                up: 127,
                ..NULL_USERCMD
            },
            until: Until::Airborne(Duration::from_millis(1500)),
        },
    ]
}

/// Walks [`motion_script`], holding each input and keeping the playerstate it
/// settles at.
struct MotionProbe {
    steps: Vec<MotionStep>,
    idx: usize,
    started: Option<Instant>,
    captured: Vec<(&'static str, Vec<i32>)>,
    done: bool,
    /// Newest snapshot already traced, so the trace prints per snapshot
    /// rather than per loop iteration.
    traced: Option<u32>,
    /// `delta_angles[1]` at the first pose, which is the spawn yaw every
    /// pose's own yaw is an offset from.
    spawn_delta_yaw: Option<i32>,
}

impl Default for MotionProbe {
    fn default() -> Self {
        Self {
            steps: motion_script(),
            idx: 0,
            started: None,
            captured: Vec::new(),
            done: false,
            traced: None,
            spawn_delta_yaw: None,
        }
    }
}

impl MotionProbe {
    fn running(&self) -> bool {
        !self.done
    }

    fn cmd(&self) -> net::msg::UserCmd {
        self.steps
            .get(self.idx)
            .map(|s| s.cmd)
            .unwrap_or(net::msg::NULL_USERCMD)
    }

    /// Feeds the newest snapshot in. Returns true once the last pose is
    /// captured and the fixture is ready to write.
    fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let Some(step) = self.steps.get(self.idx) else {
            self.done = true;
            return true;
        };
        let started = *self.started.get_or_insert(now);
        let elapsed = now.duration_since(started);
        let p = &net::protocol::PROTOCOL_V1;
        // A settled sample cannot tell a ramp from a constant; the trace can.
        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            println!(
                "  trace {} +{:>4}ms st={} vh={:>5.1} proneDir={:>8.2} dirPitch={:>7.2} torso={:>7.2} origin=[{:>8.1},{:>8.1}] deltaYaw={} eFlags={} pm=0x{:x}",
                self.steps[self.idx].label,
                elapsed.as_millis(),
                snap.server_time,
                f32::from_bits(snap.ps.field_i32(p, "viewHeightCurrent") as u32),
                f32::from_bits(snap.ps.field_i32(p, "proneDirection") as u32),
                f32::from_bits(snap.ps.field_i32(p, "proneDirectionPitch") as u32),
                f32::from_bits(snap.ps.field_i32(p, "proneTorsoPitch") as u32),
                f32::from_bits(snap.ps.field_i32(p, "origin[0]") as u32),
                f32::from_bits(snap.ps.field_i32(p, "origin[1]") as u32),
                snap.ps.field_i32(p, "delta_angles[1]"),
                snap.ps.field_i32(p, "eFlags"),
                snap.ps.field_i32(p, "pm_flags"),
            );
        }
        let take = match step.until {
            Until::Held(d) => elapsed >= d,
            Until::Airborne(limit) => {
                let airborne = snap.ps.field_i32(p, "groundEntityNum")
                    != net::protocol::ENTITYNUM_WORLD as i32;
                (airborne && elapsed >= Duration::from_millis(50)) || elapsed >= limit
            }
        };
        if !take {
            return false;
        }
        println!(
            "MOTION {}: leanf={} viewHeightCurrent={} viewHeightTarget={} viewHeightLerp[target={} time={} down={} posAdj={}] \
bobCycle={} groundEntityNum={} pm_flags=0x{:x} pm_time={} jumpTime={} velocity_z={} eventSequence={} events=[{},{},{},{}]",
            step.label,
            f32::from_bits(snap.ps.field_i32(p, "leanf") as u32),
            f32::from_bits(snap.ps.field_i32(p, "viewHeightCurrent") as u32),
            snap.ps.field_i32(p, "viewHeightTarget"),
            snap.ps.field_i32(p, "viewHeightLerpTarget"),
            snap.ps.field_i32(p, "viewHeightLerpTime"),
            snap.ps.field_i32(p, "viewHeightLerpDown"),
            f32::from_bits(snap.ps.field_i32(p, "viewHeightLerpPosAdj") as u32),
            snap.ps.field_i32(p, "bobCycle"),
            snap.ps.field_i32(p, "groundEntityNum"),
            snap.ps.field_i32(p, "pm_flags"),
            snap.ps.field_i32(p, "pm_time"),
            snap.ps.field_i32(p, "jumpTime"),
            f32::from_bits(snap.ps.field_i32(p, "velocity[2]") as u32),
            snap.ps.field_i32(p, "eventSequence"),
            snap.ps.field_i32(p, "events[0]"),
            snap.ps.field_i32(p, "events[1]"),
            snap.ps.field_i32(p, "events[2]"),
            snap.ps.field_i32(p, "events[3]"),
        );
        self.captured.push((step.label, snap.ps.fields.clone()));
        self.idx += 1;
        self.started = None;
        if self.idx >= self.steps.len() {
            self.done = true;
            return true;
        }
        false
    }
}

/// Writes one section per pose to `<map>-<gametype>-motion.txt`. Each carries
/// the input that produced it on a `!input` line, so the gate replays the
/// capture's own script rather than a copy of it.
fn write_motion_fixture(
    snap: &net::snapshot::Snapshot,
    configstrings: &[String],
    join: &JoinProbe,
    motion: &MotionProbe,
) -> anyhow::Result<()> {
    let p = &net::protocol::PROTOCOL_V1;
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
    let map = key("mapname");
    let gametype = key("g_gametype");

    let mut out = String::new();
    out.push_str("# Retail CoD 1.1d dedicated server playerstate under each movement input.\n");
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
    ));
    out.push_str("# sv_maxclients 8, sv_pure 0, stock scr_* defaults, one client on the server.\n");
    out.push_str("# Captured with tools/run_server.sh and --net-probe --save-motion. Each pose\n");
    out.push_str("# holds one usercmd until the state settles, so the values are steady states\n");
    out.push_str("# rather than points on a lerp; jump_takeoff is the first snapshot off the\n");
    out.push_str("# ground instead, which is where the takeoff velocity is.\n");
    out.push_str("# Values are the raw i32 wire words, floats as their bit patterns.\n");
    for (label, fields) in &motion.captured {
        let cmd = motion
            .steps
            .iter()
            .find(|s| s.label == *label)
            .map(|s| s.cmd)
            .unwrap_or(net::msg::NULL_USERCMD);
        out.push_str(&format!("[pose {label}]\n"));
        let sample = motion
            .steps
            .iter()
            .find(|s| s.label == *label)
            .map(|s| match s.until {
                Until::Held(_) => "settled",
                Until::Airborne(_) => "airborne",
            })
            .unwrap_or("settled");
        out.push_str(&format!(
            "!input buttons={} wbuttons={} up={} forward={} right={} yaw={} sample={sample}\n",
            cmd.buttons, cmd.wbuttons, cmd.up, cmd.forward, cmd.right, cmd.angles[1]
        ));
        for (f, v) in p.player_fields.iter().zip(fields) {
            out.push_str(&format!("{} {v}\n", f.name));
        }
    }
    let path = format!("{PLAYERSTATE_FIXTURE_DIR}/{map}-{gametype}-motion.txt");
    std::fs::write(&path, out)?;
    println!(
        "motion: {} poses ({} fields each) -> {path}",
        motion.captured.len(),
        snap.ps.fields.len()
    );
    Ok(())
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
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
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

/// A scripted yaw is a view direction, not a raw cmd word: the view is
/// `cmd.angles + delta_angles`, and the server pushes `delta_angles` to hold a
/// prone view inside its cone and to face a fresh spawn. A probe that sent the
/// raw word would fight that correction instead of holding a heading under it,
/// the way a retail client does. `spawn` latches the first correction seen, so
/// every scripted yaw is an offset from where the spawn faced.
fn hold_view_yaw(
    cmd: &mut net::msg::UserCmd,
    client: &NetClient<UdpTransport>,
    spawn: &mut Option<i32>,
) {
    let Some(s) = client.snapshots().newest() else {
        return;
    };
    let delta =
        s.ps.field_i32(&net::protocol::PROTOCOL_V1, "delta_angles[1]");
    let spawn = *spawn.get_or_insert(delta);
    cmd.angles[1] = (spawn + cmd.angles[1] - delta) & 0xffff;
}

/// One leg of the `--probe-pvs` route: a heading relative to where the spawn
/// faced, walked for `walk_ms`, then stood still until the station is taken.
struct PvsLeg {
    label: &'static str,
    yaw_deg: i32,
    walk_ms: u64,
}

/// How long a leg stands still before its station is recorded, so a moving
/// entity settles and its motion is not read as the list changing.
const PVS_STAND: Duration = Duration::from_millis(1500);

/// Ground covered below this inside [`PVS_STUCK_WINDOW`] means the walk is
/// against geometry; the leg turns 45 degrees rather than spending itself on a
/// wall. Retail's run speed is ~190 u/s, so a clear leg covers ~280u per window.
const PVS_STUCK_UNITS: f32 = 60.0;
const PVS_STUCK_WINDOW: Duration = Duration::from_millis(1500);

/// A wander: twelve stations, each leg a heading the walk keeps unless geometry
/// turns it. Legs repeat and reverse headings on purpose, so the route crosses
/// its own ground and a set that changes with position is told apart from one
/// that only grows with time.
fn pvs_route() -> Vec<PvsLeg> {
    let leg = |label, yaw_deg| PvsLeg {
        label,
        yaw_deg,
        walk_ms: 5000,
    };
    vec![
        PvsLeg {
            label: "spawn",
            yaw_deg: 0,
            walk_ms: 0,
        },
        leg("out", 0),
        leg("across", 90),
        leg("on", 0),
        leg("back", 180),
        leg("over", 270),
        leg("far", 180),
        leg("side", 90),
        leg("return", 0),
        leg("wide", 90),
        leg("long", 180),
        leg("last", 270),
    ]
}

/// What identifies an entity across snapshots. The slot number alone cannot: a
/// freed slot is reused, so a reappearing number with a different `index` is a
/// different entity, not a visibility change.
#[derive(Clone, Copy, PartialEq)]
struct EntInfo {
    etype: i32,
    index: i32,
    solid: i32,
    client_num: i32,
    origin: [f32; 3],
}

impl EntInfo {
    fn of(p: &net::protocol::Protocol, e: &net::msg::EntityState) -> Self {
        EntInfo {
            etype: e.field_i32(p, "eType"),
            index: e.field_i32(p, "index"),
            solid: e.field_i32(p, "solid"),
            client_num: e.field_i32(p, "clientNum"),
            origin: e.origin(p),
        }
    }
}

/// One recorded stop on the route: where the probe stood, and every entity
/// the server had sent it as of that frame.
struct PvsStation {
    label: &'static str,
    origin: [f32; 3],
    server_time: i32,
    ents: BTreeMap<u32, net::msg::EntityState>,
}

/// Directory `crates/server/tests/entities_ab.rs` reads its fixtures from.
const ENTITIES_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/entities"
);

/// Writes the walked route as `<map>-<gametype>.txt`: one `[sample]` per
/// station, carrying where the probe stood and every entity the server had
/// sent it there.
///
/// The origin is the point of the file. A capture cannot be told to stand
/// anywhere -- `setviewpos` is refused on a dedicated server -- so the gate
/// does not replay the route, it replays each recorded origin
/// (docs/protocol-1.1.md, "Which entities a client is sent").
fn write_entities_fixture(
    probe: &PvsProbe,
    configstrings: &[String],
    join: &JoinProbe,
    tag: Option<&str>,
) -> anyhow::Result<()> {
    let p = &net::protocol::PROTOCOL_V1;
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
    let (map, gametype) = (key("mapname"), key("g_gametype"));

    let mut head = String::new();
    head.push_str("# Retail CoD 1.1d dedicated server entity trace along walked routes.\n");
    head.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
    ));
    head.push_str(&format!(
        "# sv_maxclients {}, sv_pure {}, stock scr_* defaults, one client on the server.\n",
        key("sv_maxclients"),
        key("sv_pure"),
    ));
    head.push_str("# Captured with tools/run_server.sh and --net-probe --save-entities, one run\n");
    head.push_str("# per random spawn: a walk cannot be steered across the map, so coverage is\n");
    head.push_str("# several runs' stations sharing a file. Each run appends and rewrites,\n");
    head.push_str(&format!(
        "# keeping at most {MAX_SAMPLES_PER_SET} samples of any one entity set, since a fourth \
sample of a set\n"
    ));
    head.push_str("# already seen adds a position and no new visibility case.\n");
    head.push_str(
        "# Every run is taken from a settled server: an item's groundEntityNum reads 0\n",
    );
    head.push_str("# for the first minute after a map load and ENTITYNUM_NONE afterwards, so a\n");
    head.push_str("# capture taken straight after +map disagrees with every later one.\n");
    head.push_str(
        "# One [sample] per station, carrying the probe's origin there and every entity\n",
    );
    head.push_str(
        "# the server had sent it as of that frame. The origin is what the gate replays:\n",
    );
    head.push_str("# the route itself is not reproducible and does not need to be.\n");
    head.push_str(
        "# Values are the raw i32 wire words, floats as their bit patterns. Names come\n",
    );
    head.push_str("# from Protocol::entity_fields; a field a block does not list is zero.\n");

    let suffix = tag.map(|t| format!("-{t}")).unwrap_or_default();
    let path = format!("{ENTITIES_FIXTURE_DIR}/{map}-{gametype}{suffix}.txt");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut samples = parse_fixture_samples(&existing);
    let taken = samples.len();
    for st in &probe.stations {
        let mut body = format!("serverTime {}\n", st.server_time);
        for (axis, v) in st.origin.iter().enumerate() {
            body.push_str(&format!("origin[{axis}] {}\n", v.to_bits() as i32));
        }
        for (num, e) in &st.ents {
            body.push_str(&format!("[ent {num}]\n"));
            for (f, v) in p.entity_fields.iter().zip(&e.fields) {
                if *v != 0 {
                    body.push_str(&format!("{} {v}\n", f.name));
                }
            }
        }
        samples.push(FixtureSample {
            label: st.label.to_string(),
            ents: st.ents.keys().copied().collect(),
            body,
        });
    }

    let mut out = head;
    let mut per_set: std::collections::HashMap<Vec<u32>, usize> = std::collections::HashMap::new();
    let mut kept = 0;
    for s in &samples {
        let n = per_set.entry(s.ents.clone()).or_insert(0);
        if *n >= MAX_SAMPLES_PER_SET {
            continue;
        }
        *n += 1;
        out.push_str(&format!("[sample {kept}] {}\n", s.label));
        out.push_str(&s.body);
        kept += 1;
    }

    std::fs::create_dir_all(ENTITIES_FIXTURE_DIR)?;
    std::fs::write(&path, out)?;
    println!(
        "entities: {} stations this run, {} samples in the file ({} read, {} dropped as \
         repeats), {} distinct entity sets -> {path}",
        probe.stations.len(),
        kept,
        taken,
        samples.len() - kept,
        per_set.len(),
    );
    Ok(())
}

/// At most this many samples of one entity set survive in a fixture. A fourth
/// sample of a set already seen adds a position and no new visibility case,
/// and 96 stations of a 12-entity map ran to 230 KB before this cap existed.
const MAX_SAMPLES_PER_SET: usize = 3;

/// A sample as it sits in the fixture. The body is kept verbatim so a reread
/// and rewrite never has to understand the fields it carries.
struct FixtureSample {
    label: String,
    ents: Vec<u32>,
    body: String,
}

/// Splits a fixture into its samples, dropping the header. Tolerant by
/// design: an unreadable file reads as no samples and the run starts the map
/// over rather than failing after the capture is already spent.
fn parse_fixture_samples(text: &str) -> Vec<FixtureSample> {
    let mut out: Vec<FixtureSample> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("[sample ") {
            let label = rest
                .split_once("] ")
                .map(|(_, l)| l)
                .unwrap_or("")
                .to_string();
            out.push(FixtureSample {
                label,
                ents: Vec::new(),
                body: String::new(),
            });
            continue;
        }
        let Some(s) = out.last_mut() else {
            continue;
        };
        if let Some(num) = line
            .strip_prefix("[ent ")
            .and_then(|r| r.strip_suffix(']'))
            .and_then(|n| n.parse::<u32>().ok())
        {
            s.ents.push(num);
        }
        s.body.push_str(line);
        s.body.push('\n');
    }
    out
}

/// Walks [`pvs_route`] and keeps the snapshot entity list at each station,
/// printing every appearance and removal in between with the position it
/// happened at. The question it answers: does the server decide the entity
/// list from where the client stands?
struct PvsProbe {
    legs: Vec<PvsLeg>,
    idx: usize,
    started: Option<Instant>,
    /// Added to every remaining leg's heading once a walk stalls against
    /// geometry. It carries across legs on purpose: a leg that reset it would
    /// walk straight back into the wall the last one escaped.
    detour_deg: i32,
    /// Time and position the current stuck window opened at.
    last_progress: Option<(Instant, [f32; 3])>,
    spawn_delta_yaw: Option<i32>,
    /// The previous snapshot's entities, which the next one is diffed against.
    live: BTreeMap<u32, EntInfo>,
    /// Newest snapshot already diffed, so the diff runs per snapshot rather
    /// than per loop iteration.
    traced: Option<u32>,
    stations: Vec<PvsStation>,
    done: bool,
}

impl Default for PvsProbe {
    fn default() -> Self {
        Self {
            legs: pvs_route(),
            idx: 0,
            started: None,
            detour_deg: 0,
            last_progress: None,
            spawn_delta_yaw: None,
            live: BTreeMap::new(),
            traced: None,
            stations: Vec::new(),
            done: false,
        }
    }
}

impl PvsProbe {
    fn running(&self) -> bool {
        !self.done
    }

    fn cmd(&self) -> net::msg::UserCmd {
        let Some(leg) = self.legs.get(self.idx) else {
            return net::msg::NULL_USERCMD;
        };
        let walking = self
            .started
            .is_some_and(|t| t.elapsed() < Duration::from_millis(leg.walk_ms));
        net::msg::UserCmd {
            angles: [
                0,
                (leg.yaw_deg + self.detour_deg).rem_euclid(360) * 65536 / 360,
                0,
            ],
            forward: if walking { 127 } else { 0 },
            ..net::msg::NULL_USERCMD
        }
    }

    /// Feeds the newest snapshot in. Returns true once the last station is
    /// recorded.
    fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        if self.done {
            return true;
        }
        let Some(leg) = self.legs.get(self.idx) else {
            self.done = true;
            return true;
        };
        let (label, walk) = (leg.label, Duration::from_millis(leg.walk_ms));
        let started = *self.started.get_or_insert(now);
        let elapsed = now.duration_since(started);
        let me = snap.ps.origin(&net::protocol::PROTOCOL_V1);
        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            self.diff(snap, label, elapsed, me);
        }
        if elapsed < walk {
            let (t0, o0) = *self.last_progress.get_or_insert((now, me));
            if now.duration_since(t0) >= PVS_STUCK_WINDOW {
                if dist(o0, me) < PVS_STUCK_UNITS {
                    self.detour_deg += 45;
                    let heading = self.legs[self.idx].yaw_deg + self.detour_deg;
                    println!(
                        "PVS {label}: only {:.0}u in {}ms, turning to {heading} deg",
                        dist(o0, me),
                        PVS_STUCK_WINDOW.as_millis(),
                    );
                }
                self.last_progress = Some((now, me));
            }
            return false;
        }
        if elapsed < walk + PVS_STAND {
            return false;
        }
        println!(
            "PVS station {label}: origin=[{:.0},{:.0},{:.0}], {} entities",
            me[0],
            me[1],
            me[2],
            self.live.len(),
        );
        for (num, e) in &self.live {
            println!(
                "  ent {num:>3} eType={:<3} index={:<4} solid={:<8} clientNum={:<3} @[{:>7.0},{:>7.0},{:>7.0}] d={:.0}",
                e.etype,
                e.index,
                e.solid,
                e.client_num,
                e.origin[0],
                e.origin[1],
                e.origin[2],
                dist(e.origin, me),
            );
        }
        self.stations.push(PvsStation {
            label,
            origin: me,
            server_time: snap.server_time,
            // The frame's own list, not the diff state: what the server sent
            // is what the fixture has to carry.
            ents: snap.entities.clone(),
        });
        self.idx += 1;
        self.started = None;
        self.last_progress = None;
        if self.idx >= self.legs.len() {
            self.done = true;
            return true;
        }
        false
    }

    /// Prints what appeared and what vanished since the previous snapshot. An
    /// entity that only moved is not a visibility change and is not printed.
    fn diff(
        &mut self,
        snap: &net::snapshot::Snapshot,
        label: &str,
        elapsed: Duration,
        me: [f32; 3],
    ) {
        let p = &net::protocol::PROTOCOL_V1;
        let now: BTreeMap<u32, EntInfo> = snap
            .entities
            .iter()
            .map(|(n, e)| (*n, EntInfo::of(p, e)))
            .collect();
        let at = |e: &EntInfo| {
            format!(
                "eType={} index={} clientNum={} @[{:.0},{:.0},{:.0}] d={:.0} (me [{:.0},{:.0},{:.0}], {label} +{}ms)",
                e.etype,
                e.index,
                e.client_num,
                e.origin[0],
                e.origin[1],
                e.origin[2],
                dist(e.origin, me),
                me[0],
                me[1],
                me[2],
                elapsed.as_millis(),
            )
        };
        for (num, e) in &now {
            let fresh = match self.live.get(num) {
                None => true,
                // A reused slot: same number, different entity.
                Some(old) => old.index != e.index || old.etype != e.etype,
            };
            if fresh {
                println!("PVS +ent {num}: {}", at(e));
            }
        }
        for (num, e) in &self.live {
            if !now.contains_key(num) {
                println!("PVS -ent {num}: {}", at(e));
            }
        }
        self.live = now;
    }

    /// The table the run exists for: one row per entity slot seen anywhere,
    /// one column per station.
    fn report(&self) {
        let all: std::collections::BTreeSet<u32> = self
            .stations
            .iter()
            .flat_map(|st| st.ents.keys().copied())
            .collect();
        println!("\nPVS report: {} stations", self.stations.len());
        for st in &self.stations {
            println!(
                "  {:<8} origin=[{:>7.0},{:>7.0},{:>7.0}] {} entities",
                st.label,
                st.origin[0],
                st.origin[1],
                st.origin[2],
                st.ents.len(),
            );
        }
        let head: Vec<&str> = self.stations.iter().map(|st| st.label).collect();
        println!("  ent  {}", head.join(" "));
        for num in &all {
            let row: Vec<String> = self
                .stations
                .iter()
                .zip(&head)
                .map(|(st, h)| {
                    let mark = if st.ents.contains_key(num) { "x" } else { "." };
                    format!("{mark:^width$}", width = h.len())
                })
                .collect();
            println!("  {num:>3}  {}", row.join(" "));
        }
        // The verdict, on the sets alone: same slots everywhere means the
        // route saw no culling, which is not the same as proving there is none.
        let first = self
            .stations
            .first()
            .map(|st| st.ents.keys().copied().collect::<Vec<_>>());
        let same = self
            .stations
            .iter()
            .all(|st| Some(st.ents.keys().copied().collect::<Vec<_>>()) == first);
        if same {
            println!("PVS verdict: every station saw the same entity slots; this route found no position culling");
        } else {
            println!("PVS verdict: the entity list differs between stations; it depends on where the client stands");
        }
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
