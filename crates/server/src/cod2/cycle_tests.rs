//! The map cycle in process (cod2-map-cycle.md): two vcod CoD2 clients
//! join, then the console's `fast_restart`, `map_restart`, `map` and
//! `map_rotate`, a `tdm` time limit through the intermission to the next
//! map, and an `sd` round's `map_restart(true)`. Each client follows
//! with the commands and gamestates retail's capture shows (section 2).
//! Needs `COD2_DIR`.

use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use glam::Vec3;
use vcod_cod2::net::oob::{self, Reply};
use vcod_cod2::net::{Client, latin1};
use vcod_common::pk3::Pk3Fs;

use super::*;

struct Player {
    addr: SocketAddr,
    cl: Client,
    team: &'static str,
    /// The cmd's buttons.
    buttons: i32,
    /// Every server command and OOB line, in arrival order, gamestates as
    /// `gamestate <serverid>`.
    log: Vec<String>,
    /// Frames a new map's gamestate keeps the client busy loading: it
    /// sends nothing and reads nothing.
    load_frames: usize,
    /// Frames left of the current load.
    loading: usize,
}

struct Rig {
    sv: Cod2Server,
    fs: Rc<Pk3Fs>,
    now: Instant,
    players: Vec<Player>,
}

impl Rig {
    fn new(map: &str, gametype: &str, set: &[(&str, &str)]) -> Option<Rig> {
        Rig::with_slots(map, gametype, set, 4)
    }

    /// [`Rig::new`] with `slots` client slots.
    fn with_slots(map: &str, gametype: &str, set: &[(&str, &str)], slots: usize) -> Option<Rig> {
        let fs = Rc::new(vcod_common::testing::cod2_fs()?);
        let now = Instant::now();
        let mut cfg = super::tests::cfg(map);
        cfg.gametype = gametype.into();
        cfg.max_clients = slots;
        let mut sv = Cod2Server::new(cfg, 7, now);
        sv.exec_default_cfg(&*fs);
        sv.set_cvar("sv_pure", "0");
        for (k, v) in set {
            sv.set_cvar(k, v);
        }
        sv.load(Some(fs.clone())).unwrap();
        let mut rig = Rig {
            sv,
            fs,
            now,
            players: Vec::new(),
        };
        for (i, team) in ["allies", "axis"].into_iter().enumerate() {
            let addr = SocketAddr::from(([127, 0, 0, 1], 28961 + i as u16));
            rig.sv.handle_packet(addr, &oob::getchallenge(None), now);
            let out = rig.sv.take_outgoing();
            let Reply::Challenge(c) = oob::parse_reply(oob::payload(&out[0].1).unwrap()) else {
                panic!("no challenge");
            };
            let qport = 4660 + i as u16;
            let ui = format!("\\name\\p{i}\\rate\\25000\\snaps\\20");
            rig.sv
                .handle_packet(addr, &oob::connect(&ui, c, qport), now);
            rig.sv.take_outgoing();
            rig.players.push(Player {
                addr,
                cl: Client::new(qport, c),
                team,
                buttons: 0,
                log: Vec::new(),
                load_frames: 0,
                loading: 0,
            });
        }
        Some(rig)
    }

    /// `secs` of frames: a cmd a frame from each client, the script menus
    /// answered (serverinfo closed, the team, the class's rifle).
    fn run(&mut self, secs: f32) {
        for _ in 0..(secs * 20.0) as usize {
            for p in self.players.iter_mut() {
                if p.loading > 0 {
                    p.loading -= 1;
                    continue;
                }
                let pkt = if p.cl.parser.gamestate.is_some() {
                    let base = p.cl.parser.snapshot().map_or(0, |s| s.server_time);
                    let mut cmd = p.cl.idle_cmd(base + FRAME_MS);
                    cmd.buttons = p.buttons;
                    p.cl.build_packet(&[cmd]).unwrap()
                } else {
                    p.cl.build_packet(&[]).unwrap()
                };
                self.sv.handle_packet(p.addr, &pkt, self.now);
            }
            self.now += Duration::from_millis(FRAME_MS as u64);
            self.sv.drain_console(self.now);
            self.sv.tick_frames(self.now, 1);
            for (to, pkt) in self.sv.take_outgoing() {
                let Some(p) = self.players.iter_mut().find(|p| p.addr == to) else {
                    continue; // a bot's, which has no address
                };
                if p.loading > 0 {
                    continue;
                }
                if let Some(pl) = oob::payload(&pkt) {
                    p.log.push(format!("oob {}", latin1(pl).replace('\n', " ")));
                    continue;
                }
                let parsed = p.cl.receive(&pkt).expect("a message that parses");
                let Some(parsed) = parsed else { continue };
                if parsed.gamestate {
                    // vcod's client loads the map off a new gamestate.
                    if p.log.iter().any(|l| l.starts_with("gamestate ")) {
                        p.loading = p.load_frames;
                    }
                    let gs = p.cl.parser.gamestate.as_ref().unwrap();
                    p.log.push(format!("gamestate {}", p.cl.server_id()));
                    let map = vcod_common::net::info_value_for_key(gs.configstring(0), "mapname")
                        .unwrap_or("")
                        .to_string();
                    let si = gs.configstring(1).to_string();
                    self.fs.clear_references();
                    self.fs.touch(&format!("maps/mp/{map}.d3dbsp"));
                    let feed = gs.checksum_feed;
                    let cp = vcod_cod2::net::pure::pure_command(&self.fs, feed, &si);
                    p.cl.add_reliable(&cp);
                }
                for (_, text) in &parsed.commands {
                    let text = latin1(text);
                    p.log.push(text.clone());
                    // `CL_SystemInfoChanged`: a `fast_restart`'s new id.
                    if let Some(si) = text.strip_prefix("d 1 ") {
                        p.cl.set_systeminfo(si);
                    }
                    if let Some(n) = text.strip_prefix("t ") {
                        let n: u16 = n.trim().parse().unwrap();
                        let gs = p.cl.parser.gamestate.as_ref().unwrap();
                        let menu = gs.configstring(1246 + n).to_string();
                        let reply = if menu.starts_with("serverinfo_") {
                            "close"
                        } else if menu.starts_with("team_") {
                            p.team
                        } else {
                            match menu.strip_prefix("weapon_") {
                                Some("german") => "kar98k_mp",
                                Some("british") => "enfield_mp",
                                Some("american") => "m1garand_mp",
                                Some(_) => "mosin_nagant_mp",
                                None => continue,
                            }
                        };
                        let mr = format!("mr {} {n} {reply}", p.cl.server_id());
                        p.cl.add_reliable(&mr);
                    }
                }
            }
        }
    }

    /// Puts `slot`'s body on the floor under `at` (the first spot of a few
    /// around it the body fits), still, as a `setOrigin` would.
    fn teleport(&mut self, slot: usize, at: Vec3) {
        let game = self.sv.game.level.as_mut().unwrap();
        let (mins, maxs) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        let spot = [
            (0.0, 0.0),
            (24.0, 0.0),
            (-24.0, 0.0),
            (0.0, 24.0),
            (0.0, -24.0),
            (-40.0, 0.0),
            (0.0, 40.0),
            (0.0, -40.0),
        ]
        .into_iter()
        .find_map(|(x, y)| {
            let top = at + Vec3::new(x, y, 64.0);
            let tr = game
                .world
                .collision
                .box_trace(top, top - Vec3::Z * 320.0, mins, maxs);
            (!tr.startsolid && tr.fraction < 1.0).then_some(tr.endpos + Vec3::Z * 0.125)
        })
        .expect("a floor under the spot");
        let ps = &mut game.players[slot].as_mut().unwrap().ps;
        ps.origin = spot;
        ps.velocity = Vec3::ZERO;
    }

    /// Every use trigger's (team byte, box centre, entity), in entity
    /// order.
    fn use_triggers(&mut self) -> Vec<(u8, Vec3, vcod_gsc::EntId)> {
        let game = self.sv.game.level.as_mut().unwrap();
        game.rt.with_host(|host, cx| {
            let origin = cx.intern_folded("origin");
            let rows: Vec<_> = host
                .triggers
                .iter()
                .filter(|(_, t)| t.kind == crate::game::trigger::TriggerKind::Use)
                .map(|(id, t)| (id, *t))
                .collect();
            rows.into_iter()
                .map(|(id, t)| {
                    let o = match vcod_gsc::Host::get_field(host, cx, id, origin) {
                        vcod_gsc::Value::Vector(v) => Vec3::from(v),
                        _ => Vec3::ZERO,
                    };
                    let c = o + (Vec3::from(t.shape.mins) + Vec3::from(t.shape.maxs)) * 0.5;
                    (t.team, c, id)
                })
                .collect()
        })
    }

    /// The box centre of the trigger named `targetname`.
    fn trigger_named(&mut self, targetname: &str) -> Vec3 {
        let game = self.sv.game.level.as_mut().unwrap();
        game.rt.with_host(|host, cx| {
            let (origin, name) = (cx.intern_folded("origin"), cx.intern_folded("targetname"));
            let rows: Vec<_> = host.triggers.iter().map(|(id, t)| (id, *t)).collect();
            rows.into_iter()
                .find_map(|(id, t)| {
                    let n = match vcod_gsc::Host::get_field(host, cx, id, name) {
                        vcod_gsc::Value::String(s) => cx.resolve(s).to_string(),
                        _ => return None,
                    };
                    if n != targetname {
                        return None;
                    }
                    let o = match vcod_gsc::Host::get_field(host, cx, id, origin) {
                        vcod_gsc::Value::Vector(v) => Vec3::from(v),
                        _ => Vec3::ZERO,
                    };
                    Some(o + (Vec3::from(t.shape.mins) + Vec3::from(t.shape.maxs)) * 0.5)
                })
                .unwrap_or_else(|| panic!("no trigger {targetname}"))
        })
    }

    fn team_scores(&self) -> [i32; 2] {
        self.sv.game.level.as_ref().unwrap().rt.host.team_scores
    }

    fn level_str(&mut self, name: &str) -> String {
        let game = self.sv.game.level.as_mut().unwrap();
        game.rt.level_field_str(name)
    }

    fn console(&mut self, line: &str) {
        for p in &mut self.players {
            p.log.push(format!("-- {line}"));
        }
        self.sv.push_console(line);
    }

    /// Both clients in the world and playing, on the server's id.
    fn assert_playing(&self, what: &str) {
        let game = self.sv.game.level.as_ref().unwrap();
        for (i, p) in self.players.iter().enumerate() {
            assert!(
                game.playing(i),
                "{what}: client {i} not playing\n{:#?}",
                &p.log[p.log.len().saturating_sub(60)..]
            );
            assert_eq!(
                i32::from(p.cl.server_id()),
                self.sv.server_id(),
                "{what}: client {i}"
            );
            let snap = p.cl.parser.snapshot().unwrap();
            assert_eq!(snap.ps.i32("pm_type"), 0, "{what}: client {i}");
        }
        let aborts: Vec<_> = game
            .rt
            .aborts()
            .into_iter()
            .filter(|a| !a.starts_with("maps/mp/_fx::soundfx:"))
            .collect();
        assert!(aborts.is_empty(), "{what}: {}", aborts.join("\n"));
    }

    /// Every bot in `slots` in the world and playing.
    fn bots_playing(&mut self, slots: &[usize], what: &str) {
        let game = self.sv.game.level.as_mut().unwrap();
        for &slot in slots {
            let state = game.rt.client_field(slot, "sessionstate");
            assert!(game.playing(slot), "{what}: bot {slot} {state:?}");
        }
    }

    /// Client 0's log since the last `-- <line>` marker.
    fn since(&self, line: &str) -> Vec<String> {
        let log = &self.players[0].log;
        let at = log
            .iter()
            .rposition(|l| *l == format!("-- {line}"))
            .expect("the marker");
        log[at + 1..].to_vec()
    }
}

fn pos(log: &[String], what: &str) -> usize {
    log.iter()
        .position(|l| l == what || l.starts_with(&format!("{what} ")))
        .unwrap_or_else(|| panic!("no {what:?} in {log:#?}"))
}

/// The console's four level commands on dm, as retail's capture runs
/// them: `fast_restart` sends `B` with no gamestate and the new id as
/// `d 1` after it; `map_restart`, `map` and `map_rotate` send the OOB
/// `loadingnewmap` and a gamestate under an id 16 higher. The ids walk
/// 16, 17, 33, 49, 65 as retail's did, and both clients play again after
/// each.
#[test]
fn the_console_cycles_the_level_and_the_clients_follow() {
    let Some(mut rig) = Rig::new("mp_toujane", "dm", &[]) else {
        return;
    };
    rig.run(4.0);
    rig.assert_playing("joined");
    assert_eq!(rig.sv.server_id(), 16);
    let time = rig.sv.time_ms();

    rig.console("fast_restart");
    rig.run(3.0);
    rig.assert_playing("fast_restart");
    assert_eq!(rig.sv.server_id(), 17);
    // Three 100 ms frames on top of the 60 a 3 s run steps.
    assert_eq!(rig.sv.time_ms(), time + 3000 + 300);
    let log = rig.since("fast_restart");
    assert!(!log.iter().any(|l| l.starts_with("gamestate")), "{log:#?}");
    let b = pos(&log, "B");
    let d1 = log
        .iter()
        .position(|l| l.starts_with("d 1 ") && l.contains("\\sv_serverid\\17"))
        .expect("d 1");
    assert!(b < d1, "{log:#?}");

    rig.console("map_restart");
    rig.run(3.0);
    rig.assert_playing("map_restart");
    assert_eq!(rig.sv.server_id(), 33);
    let log = rig.since("map_restart");
    assert_eq!(log[0], "oob loadingnewmap mp_toujane dm", "{log:#?}");
    assert!(pos(&log, "gamestate 33") > 0);

    rig.console("map mp_carentan");
    rig.run(3.0);
    rig.assert_playing("map mp_carentan");
    assert_eq!(rig.sv.server_id(), 49);
    assert_eq!(rig.sv.game.cfg.map, "mp_carentan");
    let log = rig.since("map mp_carentan");
    assert_eq!(log[0], "oob loadingnewmap mp_carentan dm");

    rig.console("sv_mapRotation \"gametype tdm map mp_toujane\"");
    rig.console("map_rotate");
    rig.run(3.0);
    rig.assert_playing("map_rotate");
    assert_eq!(rig.sv.server_id(), 65);
    assert_eq!(
        (
            rig.sv.game.cfg.map.as_str(),
            rig.sv.game.cfg.gametype.as_str()
        ),
        ("mp_toujane", "tdm")
    );
    let log = rig.since("map_rotate");
    assert_eq!(log[0], "oob loadingnewmap mp_toujane tdm");
    for p in &rig.players {
        assert_eq!(
            p.log.iter().filter(|l| l.starts_with("gamestate")).count(),
            4
        );
    }
}

/// `tdm` with a half-minute time limit: `endMap` puts both clients in the
/// intermission, `exitLevel(false)` runs `map_rotate` 10 s later, and the
/// rotation's next map loads with both clients back in it.
#[test]
fn a_tdm_time_limit_ends_in_the_next_map_of_the_rotation() {
    let Some(mut rig) = Rig::new(
        "mp_toujane",
        "tdm",
        &[
            ("scr_tdm_timelimit", "0.25"),
            ("sv_mapRotation", "gametype dm map mp_carentan"),
        ],
    ) else {
        return;
    };
    rig.run(4.0);
    rig.assert_playing("joined");
    rig.run(13.0);
    let game = rig.sv.game.level.as_mut().unwrap();
    assert_eq!(game.rt.level_field_str("mapended"), "1");
    assert_eq!(
        game.rt.client_field(0, "sessionstate").as_deref(),
        Some("intermission")
    );
    rig.run(12.0);
    assert_eq!(
        (
            rig.sv.game.cfg.map.as_str(),
            rig.sv.game.cfg.gametype.as_str()
        ),
        ("mp_carentan", "dm")
    );
    rig.run(4.0);
    rig.assert_playing("next map");
    let log = &rig.players[0].log;
    assert!(log.iter().any(|l| l == "oob loadingnewmap mp_carentan dm"));
}

/// `sd`: once both teams have a player the match starts with a
/// `map_restart(true)`, which goes out as `n` (`savePersist` 1) with no
/// gamestate; both clients and two bots keep their team through `pers[]`
/// and play the round.
#[test]
fn an_sd_match_start_restarts_in_place_with_n() {
    let Some(mut rig) = Rig::new("mp_toujane", "sd", &[]) else {
        return;
    };
    let bots = rig.sv.add_bots(2, false);
    rig.run(4.0);
    rig.assert_playing("joined");
    let id = rig.sv.server_id();
    rig.run(12.0);
    let log = &rig.players[0].log;
    assert!(log.iter().any(|l| l == "n"), "{log:#?}");
    assert!(!log.iter().any(|l| l == "B"));
    assert_eq!(rig.sv.server_id(), id + 1);
    rig.run(4.0);
    rig.assert_playing("round");
    rig.bots_playing(&bots, "round");
    let game = rig.sv.game.level.as_mut().unwrap();
    assert_eq!(
        game.rt.client_field(0, "sessionteam").as_deref(),
        Some("allies")
    );
    assert_eq!(
        game.rt.client_field(1, "sessionteam").as_deref(),
        Some("axis")
    );
}

/// `sd`'s bomb on mp_toujane: the attacker stands in a bomb zone
/// (`trigger_use_touch`, `setTeamForTrigger` the attackers) holding use,
/// plants in five seconds (`level.bombplanted`), the defender holds use
/// at the bomb for ten and defuses it, and the round ends in the next
/// `map_restart(true)`, an `n`.
#[test]
fn an_sd_bomb_is_planted_and_defused() {
    let Some(mut rig) = Rig::new("mp_toujane", "sd", &[]) else {
        return;
    };
    rig.run(16.0);
    rig.assert_playing("round");
    let zones: Vec<_> = rig
        .use_triggers()
        .into_iter()
        .filter(|z| z.0 != 0)
        .collect();
    assert_eq!(zones.len(), 2, "two bomb zones with the attackers' team");
    // `setTeamForTrigger`: 2 allies, 1 axis; slot 0 is allies.
    let (team, at, _) = zones[0];
    let (planter, defender) = if team == 2 { (0, 1) } else { (1, 0) };
    rig.teleport(planter, at);
    rig.run(1.0);
    rig.players[planter].buttons = 0x28;
    rig.run(6.0);
    rig.players[planter].buttons = 0;
    assert_eq!(rig.level_str("bombplanted"), "1");
    let restarts = rig.players[0].log.iter().filter(|l| *l == "n").count();
    // The defuse trigger stands at the bomb (`bombtrigger.origin`).
    let game = rig.sv.game.level.as_ref().unwrap();
    let bomb = game.players[planter].as_ref().unwrap().ps.origin;
    rig.teleport(defender, bomb + Vec3::new(24.0, 0.0, 0.0));
    rig.run(1.0);
    rig.players[defender].buttons = 0x28;
    rig.run(11.0);
    rig.players[defender].buttons = 0;
    assert_eq!(rig.level_str("bombexploded"), "0");
    rig.run(10.0);
    let after = rig.players[0].log.iter().filter(|l| *l == "n").count();
    assert_eq!(after, restarts + 1, "the round's restart");
}

/// `ctf` on mp_toujane: the allied player walks onto the axis flag
/// (`axis_flag`'s trigger) and takes it, carries it home onto
/// `allied_flag` and scores the capture: allies 1, axis 0.
#[test]
fn a_ctf_flag_is_taken_carried_and_captured() {
    let Some(mut rig) = Rig::new("mp_toujane", "ctf", &[]) else {
        return;
    };
    rig.run(6.0);
    rig.assert_playing("joined");
    let (axis_flag, allied_flag) = (
        rig.trigger_named("axis_flag"),
        rig.trigger_named("allied_flag"),
    );
    rig.teleport(0, axis_flag);
    rig.run(1.0);
    let game = rig.sv.game.level.as_mut().unwrap();
    assert_eq!(
        game.rt.client_field(0, "statusicon").as_deref(),
        Some("compass_flag_german"),
        "the flag's carrier icon"
    );
    rig.teleport(0, allied_flag);
    rig.run(1.0);
    // [axis, allies]
    assert_eq!(rig.team_scores(), [0, 1]);
}

/// `hq` on mp_toujane: 15 s after both teams exist a radio shows; the
/// allied player stands at it until it is captured and the allies score.
#[test]
fn an_hq_radio_is_captured_and_scores() {
    let Some(mut rig) = Rig::new("mp_toujane", "hq", &[]) else {
        return;
    };
    rig.run(25.0);
    rig.assert_playing("joined");
    let game = rig.sv.game.level.as_mut().unwrap();
    let radio = game.rt.with_host(|host, cx| {
        let hidden = cx.intern_folded("hidden");
        let origin = cx.intern_folded("origin");
        let ids: Vec<_> = host.ents.iter_inuse().map(|(id, _)| id).collect();
        ids.into_iter().find_map(|id| {
            matches!(
                vcod_gsc::Host::get_field(host, cx, id, hidden),
                vcod_gsc::Value::Int(0)
            )
            .then(|| match vcod_gsc::Host::get_field(host, cx, id, origin) {
                vcod_gsc::Value::Vector(v) => Vec3::from(v),
                _ => Vec3::ZERO,
            })
        })
    });
    let radio = radio.expect("a shown radio");
    rig.teleport(0, radio + Vec3::new(40.0, 0.0, 0.0));
    rig.run(15.0);
    assert!(rig.team_scores()[1] > 0, "{:?}", rig.team_scores());
}

/// `--gametype-script`: a gametype off disk runs in place of the paks'
/// (`Cod2Config::gametype_script`), as on CoD1, and again after a map
/// change.
#[test]
fn a_gametype_script_from_disk_runs() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let mut cfg = super::tests::cfg("mp_toujane");
    cfg.gametype = "probe_disk".into();
    cfg.gametype_script = Some((
        "maps/mp/gametypes/probe_disk".into(),
        "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
         level.callbackStartGameType = ::start; }\n\
         start() { level.probe = getcvar(\"mapname\"); }\n"
            .into(),
    ));
    let mut sv = Cod2Server::new(cfg, 7, Instant::now());
    sv.load(Some(fs)).unwrap();
    let probe = |sv: &mut Cod2Server| {
        let game = sv.game.level.as_mut().unwrap();
        game.rt.level_field_str("probe")
    };
    assert_eq!(probe(&mut sv), "mp_toujane");
    sv.map_command("mp_carentan", false).unwrap();
    assert_eq!(sv.game.cfg.gametype, "probe_disk");
    assert_eq!(probe(&mut sv), "mp_carentan");
}

/// A map change resumes the gametype's compile from the last level's
/// instead of compiling it again, and lands where compiling afresh does:
/// the compile a first load of the map makes, and the configstrings and
/// baselines of the same map change with nothing carried. A changed
/// gametype script is compiled afresh.
#[test]
fn a_map_change_resumes_the_gametype_compile_where_a_fresh_one_lands() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let compiled = |sv: &Cod2Server| {
        sv.game
            .level
            .as_ref()
            .unwrap()
            .rt
            .compiled()
            .unwrap()
            .clone()
    };
    let now = Instant::now();
    let mut sv = Cod2Server::new(super::tests::cfg("mp_toujane"), 7, now);
    sv.load(Some(fs.clone())).unwrap();
    let first = compiled(&sv);
    sv.map_command("mp_carentan", false).unwrap();
    let changed = compiled(&sv);
    assert!(Rc::ptr_eq(first.gametype(), changed.gametype()));

    let mut fresh = Cod2Server::new(super::tests::cfg("mp_carentan"), 7, now);
    fresh.load(Some(fs.clone())).unwrap();
    let fresh_compile = compiled(&fresh);
    assert!(!Rc::ptr_eq(fresh_compile.gametype(), changed.gametype()));
    assert!(changed.same_compile(&fresh_compile));

    // The same map change with no compile to carry.
    let mut uncached = Cod2Server::new(super::tests::cfg("mp_toujane"), 7, now);
    uncached.load(Some(fs.clone())).unwrap();
    uncached.game.level.as_mut().unwrap().rt.forget_compiled();
    uncached.map_command("mp_carentan", false).unwrap();
    assert!(!Rc::ptr_eq(
        compiled(&uncached).gametype(),
        changed.gametype()
    ));
    let level_cs = |sv: &Cod2Server| sv.game.level.as_ref().unwrap().rt.configstrings().to_vec();
    assert_eq!(level_cs(&sv), level_cs(&uncached));
    assert_eq!(sv.core.configstrings, uncached.core.configstrings);
    assert_eq!(sv.game.baselines, uncached.game.baselines);

    // The same gametype from a script whose text changed: compiled again.
    let mut cfg = super::tests::cfg("mp_toujane");
    cfg.gametype_script = Some((
        "maps/mp/gametypes/dm".into(),
        "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); }\n".into(),
    ));
    let mut over = Cod2Server::new(cfg, 7, Instant::now());
    over.load(Some(fs)).unwrap();
    let overlaid = compiled(&over);
    over.game.level = None;
    over.game.cfg.gametype_script = None;
    over.load_with(
        over.game.fs.clone(),
        crate::game::cod2::Cod2Init {
            carry: crate::game::script::Carry {
                gametype: Some(overlaid.gametype().clone()),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let reloaded = compiled(&over);
    assert!(!Rc::ptr_eq(overlaid.gametype(), reloaded.gametype()));
    assert!(reloaded.same_compile(&first));
}

/// The letters and indexes of `log`'s commands from `first` on, `n` of
/// them: `d 13`, `B`, `H`.
fn heads(log: &[String], first: usize, n: usize) -> Vec<String> {
    log[first..first + n]
        .iter()
        .map(|l| match l.strip_prefix("d ") {
            Some(rest) => format!("d {}", rest.split(' ').next().unwrap()),
            None => l.split(' ').next().unwrap().to_string(),
        })
        .collect()
}

/// A restart's burst in retail's order (cod2-map-cycle.md 2): the
/// configstrings `G_InitGame` wrote, the fog among them though its value
/// is the old one, then the level's own `H 0` / `G 0` on sd, then `B` or
/// `n`, then `d 1` ahead of the connect callbacks' commands. No `d 7`.
#[test]
fn a_restart_sends_retail_s_burst_in_write_order() {
    let Some(mut rig) = Rig::new("mp_toujane", "dm", &[]) else {
        return;
    };
    rig.run(4.0);
    rig.console("fast_restart");
    rig.run(3.0);
    let log = rig.since("fast_restart");
    assert_eq!(
        heads(&log, 0, 5),
        ["d 13", "d 3", "d 12", "B", "d 1"],
        "{log:#?}"
    );
    assert!(log[6].contains("MP_CONNECTED"), "{log:#?}");

    let Some(mut rig) = Rig::new("mp_toujane", "sd", &[]) else {
        return;
    };
    rig.run(16.0);
    let log = &rig.players[0].log;
    let n = pos(log, "n");
    assert_eq!(
        heads(log, n - 5, 7),
        ["d 13", "d 3", "d 12", "H", "G", "n", "d 1"],
        "{log:#?}"
    );
    let gamestate = pos(log, "gamestate");
    assert!(
        !log[gamestate..].iter().any(|l| l.starts_with("d 7 ")),
        "{log:#?}"
    );
}

/// The tdm intermission's configstrings 5 and 6 follow the commands the
/// level end sent before it, as on retail (cod2-map-cycle.md 2).
#[test]
fn the_intermission_scores_follow_the_level_end_s_lines() {
    let Some(mut rig) = Rig::new("mp_toujane", "tdm", &[("scr_tdm_timelimit", "0.25")]) else {
        return;
    };
    rig.run(17.0);
    let log = &rig.players[0].log;
    let limit = pos(log, "f \"MP_TIME_LIMIT_REACHED\"");
    let tie = pos(log, "v cg_objectiveText \"MP_THE_GAME_IS_A_TIE\"");
    let (d5, d6) = (pos(log, "d 5 0"), pos(log, "d 6 0"));
    assert!(limit < tie && tie < d5 && d5 < d6, "{log:#?}");
}

/// `SV_SpawnServer`'s `loadingnewmap` leaves through the socket before the
/// load, then the loop sleeps 250 ms (0x8093520).
#[test]
fn a_map_change_sends_loadingnewmap_then_sleeps() {
    use std::cell::RefCell;
    let Some(mut rig) = Rig::new("mp_toujane", "dm", &[]) else {
        return;
    };
    rig.run(4.0);
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let (sent, slept) = (events.clone(), events.clone());
    rig.sv.set_net_now(NetNow {
        send: Box::new(move |_, pkt| {
            let text = latin1(oob::payload(pkt).unwrap_or_default());
            sent.borrow_mut().push(text.replace('\n', " "));
        }),
        sleep: Box::new(move |d| slept.borrow_mut().push(format!("sleep {}", d.as_millis()))),
    });
    rig.sv.map_command("mp_carentan", false).unwrap();
    assert_eq!(
        *events.borrow(),
        [
            "loadingnewmap mp_carentan dm",
            "loadingnewmap mp_carentan dm",
            "sleep 250"
        ]
    );
    assert!(
        rig.sv
            .take_outgoing()
            .iter()
            .all(|(_, p)| oob::payload(p).is_none())
    );
}

/// Bots join with the humans and play again after each of the console's
/// level commands and a passed `callvote map`: every new level reopens
/// the stock menus under the indices the last one used, and a bot that
/// remembered them as answered stayed a spectator.
#[test]
fn bots_play_again_after_every_level_change() {
    let Some(mut rig) = Rig::new("mp_toujane", "tdm", &[]) else {
        return;
    };
    let bots = rig.sv.add_bots(2, false);
    assert_eq!(bots.len(), 2);
    rig.run(4.0);
    rig.assert_playing("joined");
    rig.bots_playing(&bots, "joined");
    for line in [
        "map mp_carentan",
        "map_restart",
        "fast_restart",
        "sv_mapRotation \"gametype dm map mp_toujane\"",
        "map_rotate",
    ] {
        rig.console(line);
        rig.run(4.0);
        if line.starts_with("sv_") {
            continue;
        }
        rig.assert_playing(line);
        rig.bots_playing(&bots, line);
    }
    // A vote one human calls and both pass.
    rig.players[0].cl.add_reliable("callvote map mp_carentan");
    rig.run(1.0);
    rig.players[1].cl.add_reliable("vote yes");
    rig.run(8.0);
    assert_eq!(rig.sv.game.cfg.map, "mp_carentan");
    rig.assert_playing("callvote map");
    rig.bots_playing(&bots, "callvote map");
}

/// A passed `callvote map` on a full server while both clients take 15 s
/// to load the new map. A client loading sends no acknowledge, and the 50
/// bots' reconnect prints pile up in its ring past the 128 slots. Retail
/// queues a client not `CS_ACTIVE` only its type 1 commands
/// (`SV_AddServerCommand` 0x8094750, cod2-protocol.md 3.4), so neither is
/// dropped with `EXE_SERVERCOMMANDOVERFLOW` and both play on the new map.
#[test]
fn a_voted_map_change_keeps_a_slow_loading_client() {
    let Some(mut rig) = Rig::with_slots("mp_toujane", "tdm", &[], 52) else {
        return;
    };
    rig.run(4.0);
    rig.assert_playing("joined");
    let bots = rig.sv.add_bots(50, false);
    assert_eq!(bots.len(), 50);
    rig.run(4.0);
    rig.assert_playing("bots joined");
    for p in &mut rig.players {
        p.load_frames = 15 * 20;
    }
    rig.players[0].cl.add_reliable("callvote map mp_carentan");
    rig.run(1.0);
    rig.players[1].cl.add_reliable("vote yes");
    rig.run(25.0);
    assert_eq!(rig.sv.game.cfg.map, "mp_carentan");
    for p in &rig.players {
        assert_eq!(
            p.log.iter().filter(|l| l.starts_with("gamestate")).count(),
            2
        );
    }
    rig.assert_playing("callvote map");
}
