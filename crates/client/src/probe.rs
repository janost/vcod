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
/// `save_combat` joins the same way and runs the weapon script in
/// [`combat_script`], recording every snapshot rather than a settled sample:
/// a shot moves `weaponstate`, `weapAnim` and the four-slot event ring, and
/// the ring overwrites, so a settled sample cannot hold one. It is also the
/// one mode with the stall response on, so a walking step gets past the first
/// wall it meets.
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
    pub combat: bool,
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
        combat: save_combat,
        entities: save_entities,
    } = save;
    // The fixture is the route's output, so the capture drives the same walk.
    let pvs = pvs || save_entities;
    // Every mode that needs a spawned player drives the same stock-menu join;
    // `--probe-team` alone joins and then just watches the roster.
    let joining = save_playerstate || save_motion || save_combat || pvs || team.is_some();
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
    let mut combat = CombatProbe::default();
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
        } else if save_combat && combat.running() {
            cmd = combat.cmd();
            if let Some(o) = client
                .snapshots()
                .newest()
                .map(|s| s.ps.origin(&net::protocol::PROTOCOL_V1))
            {
                combat.stall.apply(&mut cmd, now, o);
            }
            hold_view_yaw(&mut cmd, &client, &mut combat.spawn_delta_yaw);
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
                    } else if save_combat {
                        // The join names the weapon; the reload step is sized
                        // off its `reloadTime` rather than one rifle's number.
                        combat.use_weapon(fs, &join.weapon);
                        if combat.step(now, s) {
                            write_combat_fixture(client.configstrings(), &join, &combat)?;
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

    if save_combat && !wrote_playerstate {
        println!(
            "no combat fixture: the run ended on step {} of {} after {secs}s; raise --probe-secs",
            combat.idx + 1,
            combat.steps.len()
        );
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
    /// At the first snapshot back on the ground, which is where the landing
    /// anim is. An event anim expires, so a settled sample cannot hold one.
    Grounded(Duration),
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
        held(
            "run_back",
            net::msg::UserCmd {
                forward: -127,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "strafe_left",
            net::msg::UserCmd {
                right: -127,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "strafe_right",
            net::msg::UserCmd {
                right: 127,
                ..NULL_USERCMD
            },
            2000,
        ),
        // Ads standing: the `weapon_position ads` clauses are the only ones a
        // rifleman can otherwise never reach.
        held(
            "ads_stand",
            net::msg::UserCmd {
                buttons: 0x10,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "crouch_run",
            net::msg::UserCmd {
                wbuttons: WBUTTON_CROUCH,
                up: -127,
                forward: 127,
                ..NULL_USERCMD
            },
            2500,
        ),
        held("stand_after_crouch", NULL_USERCMD, 1500),
        // Turning on the spot. Nothing derives the turn movetypes yet, so
        // this is evidence rather than a gate: it records whether retail
        // leaves the idle anim alone while the view sweeps.
        held(
            "turn_left",
            net::msg::UserCmd {
                angles: [0, -60 * 65536 / 360, 0],
                ..NULL_USERCMD
            },
            1500,
        ),
        held(
            "turn_right",
            net::msg::UserCmd {
                angles: [0, 60 * 65536 / 360, 0],
                ..NULL_USERCMD
            },
            1500,
        ),
        MotionStep {
            label: "jump_takeoff",
            cmd: net::msg::UserCmd {
                up: 127,
                ..NULL_USERCMD
            },
            until: Until::Airborne(Duration::from_millis(1500)),
        },
        MotionStep {
            label: "land",
            cmd: NULL_USERCMD,
            until: Until::Grounded(Duration::from_millis(1500)),
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
                "  trace {} +{:>4}ms st={} vh={:>5.1} proneDir={:>8.2} dirPitch={:>7.2} torso={:>7.2} origin=[{:>8.1},{:>8.1}] deltaYaw={} eFlags={} pm=0x{:x} legsAnim={} torsoAnim={} weapAnim={}",
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
                snap.ps.field_i32(p, "legsAnim"),
                snap.ps.field_i32(p, "torsoAnim"),
                snap.ps.field_i32(p, "weapAnim"),
            );
        }
        let take = match step.until {
            Until::Held(d) => elapsed >= d,
            Until::Airborne(limit) => {
                let airborne = snap.ps.field_i32(p, "groundEntityNum")
                    != net::protocol::ENTITYNUM_WORLD as i32;
                (airborne && elapsed >= Duration::from_millis(50)) || elapsed >= limit
            }
            Until::Grounded(limit) => {
                let grounded = snap.ps.field_i32(p, "groundEntityNum")
                    == net::protocol::ENTITYNUM_WORLD as i32;
                (grounded && elapsed >= Duration::from_millis(50)) || elapsed >= limit
            }
        };
        if !take {
            return false;
        }
        println!(
            "MOTION {}: leanf={} viewHeightCurrent={} viewHeightTarget={} viewHeightLerp[target={} time={} down={} posAdj={}] \
bobCycle={} groundEntityNum={} pm_flags=0x{:x} pm_time={} jumpTime={} velocity_z={} eventSequence={} events=[{},{},{},{}] \
legsAnim={} torsoAnim={} weapAnim={}",
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
            snap.ps.field_i32(p, "legsAnim"),
            snap.ps.field_i32(p, "torsoAnim"),
            snap.ps.field_i32(p, "weapAnim"),
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
                Until::Grounded(_) => "grounded",
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

/// `buttons` bit 0, fire (docs/protocol-1.1.md, "Usercmd input bits").
const BUTTON_ATTACK: u8 = 0x01;

/// `weaponstate` the capture keys off: the weapon is ready to take an input,
/// and it is firing. 2 is the reload, seen but not waited on.
const WEAPONSTATE_READY: i32 = 0;
const WEAPONSTATE_FIRING: i32 = 3;

/// A key a step taps rather than holds. The stock rifle is semi-automatic, so
/// a held fire bit fires once and then nothing: every shot needs its own
/// release edge, and reload is a press for the same reason.
#[derive(Clone, Copy, Default)]
struct Pulse {
    buttons: u8,
    wbuttons: u8,
    /// Taps the step sends; after the last one it holds its base input alone.
    count: u32,
}

/// How long a tap is held down, and the floor on how often one starts: two
/// frames down, six up at the probe's 60 Hz send rate, which is what fires
/// cleanly on retail. The gap is raised to clear the weapon file's `fireTime`
/// when that is longer, so a tap edge never races the shot before it;
/// m1carbine_mp's is 0.135 s, past the floor.
const PULSE_HOLD: Duration = Duration::from_millis(32);
const PULSE_PERIOD: Duration = Duration::from_millis(128);
const PULSE_MARGIN: Duration = Duration::from_millis(48);

/// One labelled step of the combat script: `base` held for `dur`, with
/// `pulse`'s bits tapped on top of it.
struct CombatStep {
    label: &'static str,
    base: net::msg::UserCmd,
    pulse: Pulse,
    dur: Duration,
    /// Whether the step walks. A walking step is steered by [`StallTurn`], so
    /// where it ends up is not reproducible and a gate cannot replay it.
    walks: bool,
    /// Whether the step's clock waits for `weaponstate` to read ready. A step
    /// that taps has to: the weapon it inherits may still be busy, and a
    /// reload that outlives its step recorded a reload under a fire label.
    wait_ready: bool,
    /// Set to the weapon file's `reloadTime` plus a margin once the join names
    /// the weapon, so the hold is not a guess about one rifle's timings.
    from_reload_time: bool,
    /// How often a tap starts; [`PULSE_PERIOD`] until the weapon file raises it.
    period: Duration,
}

impl CombatStep {
    /// What the step sends `elapsed` into itself: its held input, with the
    /// pulsed bits down for the first [`PULSE_HOLD`] of each `period` until
    /// `count` taps have gone out.
    fn cmd_at(&self, elapsed: Duration) -> net::msg::UserCmd {
        let mut cmd = self.base;
        let ms = elapsed.as_millis();
        if ms / self.period.as_millis() < self.pulse.count as u128
            && ms % self.period.as_millis() < PULSE_HOLD.as_millis()
        {
            cmd.buttons |= self.pulse.buttons;
            cmd.wbuttons |= self.pulse.wbuttons;
        }
        cmd
    }
}

/// The steps, each starting from the one before. The two idles bracket the
/// shooting so the capture shows what the channels return to, and the stances
/// are separate steps because the animscript's fire clauses are per stance.
fn combat_script() -> Vec<CombatStep> {
    use net::msg::{NULL_USERCMD, WBUTTON_CROUCH, WBUTTON_PRONE, WBUTTON_RELOAD};
    let hold = |label, base, ms| CombatStep {
        label,
        base,
        pulse: Pulse::default(),
        dur: Duration::from_millis(ms),
        walks: false,
        wait_ready: false,
        from_reload_time: false,
        period: PULSE_PERIOD,
    };
    let fire = |label, base, shots, ms| CombatStep {
        label,
        base,
        pulse: Pulse {
            buttons: BUTTON_ATTACK,
            wbuttons: 0,
            count: shots,
        },
        dur: Duration::from_millis(ms),
        walks: false,
        wait_ready: true,
        from_reload_time: false,
        period: PULSE_PERIOD,
    };
    vec![
        // First, so two probes spawned across a map close some of the distance
        // between them before anyone shoots. Not a gate case: the stall
        // response steers it.
        CombatStep {
            label: "advance",
            base: net::msg::UserCmd {
                forward: 127,
                ..NULL_USERCMD
            },
            pulse: Pulse::default(),
            dur: Duration::from_millis(6000),
            walks: true,
            wait_ready: false,
            from_reload_time: false,
            period: PULSE_PERIOD,
        },
        hold("idle", NULL_USERCMD, 1500),
        fire("single_shot", NULL_USERCMD, 1, 1500),
        // More shots than the ring has slots, which is what shows how it wraps
        // and what `eventSequence` does across the wrap.
        fire("sustained_fire", NULL_USERCMD, 6, 2500),
        CombatStep {
            label: "reload",
            base: NULL_USERCMD,
            pulse: Pulse {
                buttons: 0,
                wbuttons: WBUTTON_RELOAD,
                count: 1,
            },
            dur: RELOAD_HOLD_FALLBACK,
            walks: false,
            wait_ready: true,
            from_reload_time: true,
            period: PULSE_PERIOD,
        },
        fire(
            "crouch_fire",
            net::msg::UserCmd {
                wbuttons: WBUTTON_CROUCH,
                up: -127,
                ..NULL_USERCMD
            },
            3,
            2500,
        ),
        // Standing between the two lowered stances: a prone taken straight out
        // of a crouch was refused in one motion capture and taken in another.
        hold("stand_between", NULL_USERCMD, 1500),
        fire(
            "prone_fire",
            net::msg::UserCmd {
                wbuttons: WBUTTON_PRONE,
                up: -127,
                ..NULL_USERCMD
            },
            3,
            3000,
        ),
        hold("idle_after", NULL_USERCMD, 3000),
    ]
}

/// One snapshot's worth of the channels a shot moves.
struct CombatSample {
    label: &'static str,
    elapsed_ms: u128,
    server_time: i32,
    /// The input in flight when the snapshot arrived, so a trace line can be
    /// lined up with the tap that produced it.
    buttons: u8,
    wbuttons: u8,
    weaponstate: i32,
    weap_anim: i32,
    legs_anim: i32,
    torso_anim: i32,
    event_sequence: i32,
    events: [i32; 4],
}

/// What a step turned out to be: kept beside the playerstate it ended at, so
/// the fixture can say on its face whether a step labelled fire ever fired.
struct StepResult {
    label: &'static str,
    /// The step ever observed `weaponstate` firing. False on a fire step is a
    /// broken capture, not a fact about retail.
    fired: bool,
    /// Snapshots where `weaponstate` rose into firing. A shot shorter than the
    /// gap between snapshots can slip through, which is what `seq_delta` is
    /// the check on.
    shots_seen: u32,
    /// `eventSequence` gained over the step. 8 bits on the wire, so it wraps.
    seq_delta: i32,
    /// How long the step waited for the weapon to read ready before its clock
    /// started.
    waited_ms: u128,
    /// The step was restarted because the weapon went busy under it.
    retried: bool,
    /// The playerstate the step ended at, for the fields a trace line omits.
    fields: Vec<i32>,
}

/// Walks [`combat_script`] and keeps every snapshot along the way. A settled
/// sample cannot hold a shot -- the event ring has four slots and overwrites
/// as it goes -- so this mode records the trace itself and the settled
/// playerstate only as the step's tail.
struct CombatProbe {
    steps: Vec<CombatStep>,
    idx: usize,
    /// Set when the current step's clock starts, which a `wait_ready` step
    /// defers until the weapon reads ready.
    started: Option<Instant>,
    /// When the current step began waiting for that, and since when the weapon
    /// has read ready without interruption.
    waiting_since: Option<Instant>,
    ready_since: Option<Instant>,
    /// Steps already restarted once, so a weapon that goes busy every time
    /// costs one retry and not the run.
    retried: Vec<&'static str>,
    trace: Vec<CombatSample>,
    results: Vec<StepResult>,
    done: bool,
    /// Newest snapshot already recorded, so the trace runs per snapshot rather
    /// than per loop iteration.
    traced: Option<u32>,
    /// The weapon file has been read once; a second look would reread it every
    /// frame.
    weapon_read: bool,
    spawn_delta_yaw: Option<i32>,
    stall: StallTurn,
}

impl Default for CombatProbe {
    fn default() -> Self {
        Self {
            steps: combat_script(),
            idx: 0,
            started: None,
            waiting_since: None,
            ready_since: None,
            retried: Vec::new(),
            trace: Vec::new(),
            results: Vec::new(),
            done: false,
            traced: None,
            weapon_read: false,
            spawn_delta_yaw: None,
            stall: StallTurn::default(),
        }
    }
}

impl CombatProbe {
    fn running(&self) -> bool {
        !self.done
    }

    /// Holds the step's base input, and taps only once its clock runs: a step
    /// waiting for the weapon to read ready must not be tapping while it waits.
    fn cmd(&self) -> net::msg::UserCmd {
        match (self.steps.get(self.idx), self.started) {
            (Some(step), Some(t)) => step.cmd_at(t.elapsed()),
            (Some(step), None) => step.base,
            (None, _) => net::msg::NULL_USERCMD,
        }
    }

    /// Sizes the reload step off the weapon the join was given, once. A hold
    /// shorter than `reloadTime` leaves the reload running into the steps after
    /// it, which recorded a reload under two fire labels before this existed.
    fn use_weapon(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, name: &str) {
        if self.weapon_read || name.is_empty() {
            return;
        }
        self.weapon_read = true;
        let Some(fs) = fs else {
            println!("COMBAT: no game data, reload holds the {RELOAD_HOLD_FALLBACK:?} fallback");
            return;
        };
        let def = match vcod_common::weapon::load(fs, name) {
            Ok(def) => def,
            Err(e) => {
                println!("COMBAT: cannot read weapon {name} ({e:#}); reload holds the fallback");
                return;
            }
        };
        let dur = Duration::from_secs_f32(def.reload_time) + RELOAD_MARGIN;
        let period = (Duration::from_secs_f32(def.fire_time) + PULSE_MARGIN).max(PULSE_PERIOD);
        for step in &mut self.steps {
            if step.from_reload_time {
                step.dur = dur;
            }
            step.period = period;
        }
        println!(
            "COMBAT: {name} reloadTime {:.2}s fireTime {:.3}s, reload step holds {dur:?}, taps every {period:?}",
            def.reload_time, def.fire_time
        );
    }

    /// Feeds the newest snapshot in. Returns true once the last step is done
    /// and the fixture is ready to write.
    fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let Some(step) = self.steps.get(self.idx) else {
            self.done = true;
            return true;
        };
        let (label, dur, wait_ready, fires) =
            (step.label, step.dur, step.wait_ready, step_fires(step));
        let p = &net::protocol::PROTOCOL_V1;
        let weaponstate = snap.ps.field_i32(p, "weaponstate");
        // A step that taps starts from a weapon that can take the input. The
        // wait is on the observed state, not a duration, so a weapon with other
        // timings than the stock rifle's does not silently break the capture.
        if wait_ready && self.started.is_none() {
            let since = *self.waiting_since.get_or_insert(now);
            let waited = now.duration_since(since);
            let steady = if weaponstate == WEAPONSTATE_READY {
                now.duration_since(*self.ready_since.get_or_insert(now))
            } else {
                self.ready_since = None;
                Duration::ZERO
            };
            if steady < READY_STABLE && waited < READY_TIMEOUT {
                return false;
            }
            if steady < READY_STABLE {
                println!(
                    "COMBAT {label}: weaponstate still {weaponstate} after {READY_TIMEOUT:?}, starting anyway"
                );
            } else if waited > READY_STABLE {
                println!(
                    "COMBAT {label}: weapon ready after {}ms",
                    waited.as_millis()
                );
            }
        }
        let started = *self.started.get_or_insert(now);
        let elapsed = now.duration_since(started);
        let cmd = self.cmd();
        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            let s = CombatSample {
                label,
                elapsed_ms: elapsed.as_millis(),
                server_time: snap.server_time,
                buttons: cmd.buttons,
                wbuttons: cmd.wbuttons,
                weaponstate,
                weap_anim: snap.ps.field_i32(p, "weapAnim"),
                legs_anim: snap.ps.field_i32(p, "legsAnim"),
                torso_anim: snap.ps.field_i32(p, "torsoAnim"),
                event_sequence: snap.ps.field_i32(p, "eventSequence"),
                events: [
                    snap.ps.field_i32(p, "events[0]"),
                    snap.ps.field_i32(p, "events[1]"),
                    snap.ps.field_i32(p, "events[2]"),
                    snap.ps.field_i32(p, "events[3]"),
                ],
            };
            println!(
                "  trace {label} +{:>5}ms st={} in={:02x}/{:02x} weaponstate={} weapAnim={} legsAnim={} torsoAnim={} evSeq={} events=[{},{},{},{}]",
                s.elapsed_ms,
                s.server_time,
                s.buttons,
                s.wbuttons,
                s.weaponstate,
                s.weap_anim,
                s.legs_anim,
                s.torso_anim,
                s.event_sequence,
                s.events[0],
                s.events[1],
                s.events[2],
                s.events[3],
            );
            self.trace.push(s);
        }
        // The weapon went busy under a fire step before it fired: the taps
        // spent so far bought nothing, so the step starts over rather than
        // recording a reload under a firing label.
        if fires
            && weaponstate == WEAPONSTATE_RELOADING
            && !self.retried.contains(&label)
            && !self
                .trace
                .iter()
                .any(|s| s.label == label && s.weaponstate == WEAPONSTATE_FIRING)
        {
            println!(
                "COMBAT {label}: weaponstate {weaponstate} +{}ms into a fire step, restarting it once",
                elapsed.as_millis()
            );
            self.retried.push(label);
            self.trace.retain(|s| s.label != label);
            self.started = None;
            self.waiting_since = None;
            self.ready_since = None;
            return false;
        }
        if elapsed < dur {
            return false;
        }
        let waited_ms = self
            .waiting_since
            .map_or(0, |t| started.duration_since(t).as_millis());
        let mine: Vec<&CombatSample> = self.trace.iter().filter(|s| s.label == label).collect();
        let set = |f: fn(&CombatSample) -> i32| {
            mine.iter()
                .map(|s| f(s))
                .collect::<std::collections::BTreeSet<i32>>()
        };
        let mut shots_seen = 0;
        let mut was_firing = false;
        for s in &mine {
            let firing = s.weaponstate == WEAPONSTATE_FIRING;
            shots_seen += u32::from(firing && !was_firing);
            was_firing = firing;
        }
        // `eventSequence` is 8 bits on the wire, so a run spanning the wrap
        // reads as a negative difference without this.
        let seq_delta = match (mine.first(), mine.last()) {
            (Some(a), Some(b)) => (b.event_sequence - a.event_sequence).rem_euclid(256),
            _ => 0,
        };
        println!(
            "COMBAT {label}: {} snapshots, {shots_seen} shot(s) seen, eventSequence +{seq_delta}, weaponstate {:?}, weapAnim {:?}, legsAnim {:?}, torsoAnim {:?}",
            mine.len(),
            set(|s| s.weaponstate),
            set(|s| s.weap_anim),
            set(|s| s.legs_anim),
            set(|s| s.torso_anim),
        );
        let fired = mine.iter().any(|s| s.weaponstate == WEAPONSTATE_FIRING);
        if fires && !fired {
            println!(
                "COMBAT WARNING {label}: a fire step that never saw weaponstate {WEAPONSTATE_FIRING}. \
                 The capture is broken, not the server: something left the weapon busy."
            );
        }
        self.results.push(StepResult {
            label,
            fired,
            shots_seen,
            seq_delta,
            waited_ms,
            retried: self.retried.contains(&label),
            fields: snap.ps.fields.clone(),
        });
        self.idx += 1;
        self.started = None;
        self.waiting_since = None;
        self.ready_since = None;
        if self.idx >= self.steps.len() {
            self.done = true;
            return true;
        }
        false
    }
}

/// Whether the step taps the trigger, which is what makes a step that never
/// observed a shot a defect rather than a fact.
fn step_fires(step: &CombatStep) -> bool {
    step.pulse.buttons & BUTTON_ATTACK != 0 && step.pulse.count > 0
}

/// Added to the weapon file's `reloadTime` so the reload has finished before
/// the next step starts, and used whole when no weapon file can be read.
/// m1carbine_mp's `reloadTime` is 2.65 s; a step shorter than that ran the
/// reload on into the two stance steps and recorded it under their labels.
const RELOAD_MARGIN: Duration = Duration::from_millis(1200);
const RELOAD_HOLD_FALLBACK: Duration = Duration::from_millis(6000);

/// How long `weaponstate` has to read ready before a step's clock starts. One
/// ready snapshot is not enough: in the capture that motivated this, a second
/// reload began 50 ms into `crouch_fire`, and `crouch_fire` had started on a
/// weapon that read ready.
const READY_STABLE: Duration = Duration::from_millis(500);

/// How long a step waits for that before it gives up and starts anyway, so a
/// weapon that never settles costs one step's worth of capture rather than the
/// whole run.
const READY_TIMEOUT: Duration = Duration::from_secs(8);

/// `weaponstate` while reloading. A fire step that meets it before it has seen
/// a shot restarts once: the weapon went busy under it, and the taps it has
/// already spent bought nothing.
const WEAPONSTATE_RELOADING: i32 = 2;

/// Ground covered below this inside [`STALL_WINDOW`] while walking forward
/// means the probe is standing against geometry. Retail's run speed is
/// ~190 u/s, so a clear walk covers ~280u per window.
const STALL_UNITS: f32 = 60.0;
const STALL_WINDOW: Duration = Duration::from_millis(1500);
const STALL_TURN_DEG: i32 = 45;

/// A wall follower, near enough: a probe walking into geometry stops there and
/// stays, ~135u from a spawn, so a stalled walk turns and keeps going. Opt-in,
/// because every capture that holds an exact input depends on not wandering.
#[derive(Default)]
struct StallTurn {
    /// Added to the walk's heading; it accumulates until the walk moves again.
    detour_deg: i32,
    /// Time and position the current stall window opened at.
    last_progress: Option<(Instant, [f32; 3])>,
}

impl StallTurn {
    /// Turns `cmd` by the accumulated detour, and turns further when a forward
    /// walk has covered no ground. Only the horizontal distance counts: a
    /// probe sliding down a slope is not making progress.
    fn apply(&mut self, cmd: &mut net::msg::UserCmd, now: Instant, origin: [f32; 3]) {
        if cmd.forward <= 0 {
            self.last_progress = None;
            return;
        }
        let (t0, o0) = *self.last_progress.get_or_insert((now, origin));
        if now.duration_since(t0) >= STALL_WINDOW {
            let moved = ((origin[0] - o0[0]).powi(2) + (origin[1] - o0[1]).powi(2)).sqrt();
            if moved < STALL_UNITS {
                self.detour_deg = (self.detour_deg + STALL_TURN_DEG).rem_euclid(360);
                println!(
                    "STALL: only {moved:.0}u in {}ms, turning to detour {} deg",
                    STALL_WINDOW.as_millis(),
                    self.detour_deg,
                );
            }
            self.last_progress = Some((now, origin));
        }
        cmd.angles[1] += self.detour_deg * 65536 / 360;
    }
}

/// Writes one section per step to `<map>-<gametype>-combat.txt`. Each carries
/// the input that produced it on an `!input` line, so the gate replays the
/// capture's own script rather than a copy of it, one `!trace` line per
/// snapshot, and the playerstate the step ended at.
fn write_combat_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    combat: &CombatProbe,
) -> anyhow::Result<()> {
    let p = &net::protocol::PROTOCOL_V1;
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
    let map = key("mapname");
    let gametype = key("g_gametype");

    let mut out = String::new();
    out.push_str("# Retail CoD 1.1d dedicated server playerstate under weapon input.\n");
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
    ));
    out.push_str("# sv_maxclients 8, sv_pure 0, stock scr_* defaults, one client on the server.\n");
    out.push_str("# Captured with tools/run_server.sh and --net-probe --save-combat.\n");
    out.push_str("# The fire bit is tapped, not held: the stock rifle is semi-automatic and a\n");
    out.push_str("# held bit fires one shot and then nothing. !input carries the held input,\n");
    out.push_str("# the tapped bits, how many taps and the tap timing, all in ms.\n");
    out.push_str("# One !trace line per snapshot, because the event ring holds four slots and\n");
    out.push_str("# overwrites: a shot is a transient no settled sample can hold. The field\n");
    out.push_str("# lines after a step's traces are the playerstate it ended at.\n");
    out.push_str("# !observed carries what the step turned out to be: whether a fire step ever\n");
    out.push_str("# saw weaponstate 3, how many shots and how much eventSequence it gained, and\n");
    out.push_str("# the distinct values each channel took. A fire step with fired=0 is a broken\n");
    out.push_str("# capture and is flagged above its section as well.\n");
    out.push_str("# Values are the raw i32 wire words, floats as their bit patterns.\n");
    let broken: Vec<&str> = combat
        .results
        .iter()
        .filter(|r| {
            !r.fired
                && combat
                    .steps
                    .iter()
                    .any(|s| s.label == r.label && step_fires(s))
        })
        .map(|r| r.label)
        .collect();
    if !broken.is_empty() {
        out.push_str(&format!(
            "# BROKEN: fire step(s) {} never observed weaponstate {WEAPONSTATE_FIRING}; \
recapture before gating anything on this file.\n",
            broken.join(", ")
        ));
    }
    for r in &combat.results {
        let Some(step) = combat.steps.iter().find(|s| s.label == r.label) else {
            continue;
        };
        let mine: Vec<&CombatSample> = combat.trace.iter().filter(|s| s.label == r.label).collect();
        let set = |f: fn(&CombatSample) -> i32| {
            mine.iter()
                .map(|s| f(s))
                .map(|v| v.to_string())
                .collect::<std::collections::BTreeSet<String>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",")
        };
        if step_fires(step) && !r.fired {
            out.push_str(&format!(
                "# BROKEN: {} never observed weaponstate {WEAPONSTATE_FIRING}; it captured no shot.\n",
                r.label
            ));
        }
        out.push_str(&format!("[step {}]\n", r.label));
        out.push_str(&format!(
            "!input buttons={} wbuttons={} up={} forward={} right={} yaw={} \
pulse_buttons={} pulse_wbuttons={} pulses={} pulse_hold_ms={} pulse_period_ms={} \
hold_ms={} walks={} wait_ready={}\n",
            step.base.buttons,
            step.base.wbuttons,
            step.base.up,
            step.base.forward,
            step.base.right,
            step.base.angles[1],
            step.pulse.buttons,
            step.pulse.wbuttons,
            step.pulse.count,
            PULSE_HOLD.as_millis(),
            step.period.as_millis(),
            step.dur.as_millis(),
            step.walks as i32,
            step.wait_ready as i32,
        ));
        out.push_str(&format!(
            "!observed fire_step={} expected_shots={} fired={} shots_seen={} seq_delta={} \
waited_ready_ms={} retried={} snapshots={} weaponstate={} legsAnim={} torsoAnim={} weapAnim={}\n",
            step_fires(step) as i32,
            if step_fires(step) {
                step.pulse.count
            } else {
                0
            },
            r.fired as i32,
            r.shots_seen,
            r.seq_delta,
            r.waited_ms,
            r.retried as i32,
            mine.len(),
            set(|s| s.weaponstate),
            set(|s| s.legs_anim),
            set(|s| s.torso_anim),
            set(|s| s.weap_anim),
        ));
        for s in &mine {
            out.push_str(&format!(
                "!trace ms={} serverTime={} buttons={} wbuttons={} weaponstate={} weapAnim={} \
legsAnim={} torsoAnim={} eventSequence={} events[0]={} events[1]={} events[2]={} events[3]={}\n",
                s.elapsed_ms,
                s.server_time,
                s.buttons,
                s.wbuttons,
                s.weaponstate,
                s.weap_anim,
                s.legs_anim,
                s.torso_anim,
                s.event_sequence,
                s.events[0],
                s.events[1],
                s.events[2],
                s.events[3],
            ));
        }
        for (f, v) in p.player_fields.iter().zip(&r.fields) {
            out.push_str(&format!("{} {v}\n", f.name));
        }
    }

    let path = format!("{PLAYERSTATE_FIXTURE_DIR}/{map}-{gametype}-combat.txt");
    std::fs::create_dir_all(PLAYERSTATE_FIXTURE_DIR)?;
    std::fs::write(&path, out)?;
    println!(
        "combat: {} steps, {} traced snapshots -> {path}",
        combat.results.len(),
        combat.trace.len(),
    );
    if !broken.is_empty() {
        println!(
            "combat: BROKEN fire step(s) {}: no shot captured",
            broken.join(", ")
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock rifle is semi-automatic: a held fire bit is one shot and then
    /// nothing, so every shot needs its own release edge. Counted at the
    /// probe's send rate rather than read off the pattern by eye.
    #[test]
    fn a_fire_step_taps_the_attack_bit_once_per_shot() {
        let script = combat_script();
        let step = script
            .iter()
            .find(|s| s.label == "sustained_fire")
            .expect("the script fires a burst");
        let (mut edges, mut frames_down, mut was_down) = (0, 0, false);
        for frame in 0..step.dur.as_millis() / 16 {
            let down = step
                .cmd_at(Duration::from_millis(frame as u64 * 16))
                .buttons
                & BUTTON_ATTACK
                != 0;
            edges += u32::from(down && !was_down);
            frames_down += u32::from(down);
            was_down = down;
        }
        assert_eq!(edges, step.pulse.count);
        assert!(
            frames_down <= 3 * step.pulse.count,
            "{frames_down} frames down for {} taps: the bit is being held",
            step.pulse.count
        );
    }

    /// A step that taps has to start from a weapon that can take the input.
    /// A reload that outlived its step ran on into both stance steps and was
    /// recorded under their labels, with no shot in either.
    #[test]
    fn a_tapping_step_starts_from_a_ready_weapon() {
        for step in combat_script().iter().filter(|s| s.pulse.count > 0) {
            assert!(
                step.wait_ready,
                "{} taps without waiting for the weapon",
                step.label
            );
        }
        let reload = combat_script()
            .into_iter()
            .find(|s| s.from_reload_time)
            .expect("the script reloads");
        // m1carbine_mp's `reloadTime`, the shortest hold the fallback has to
        // outlast when no weapon file can be read.
        assert!(reload.dur > Duration::from_millis(2650));
    }
}
