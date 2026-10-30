//! A soak of the damage path on [`Cod4Server`]: two `cod4_probe`-shaped
//! clients (`target` allies in slot 0, `shooter` axis in slot 1) on
//! mp_crash `war` under `sv_cheats` run one transition after another
//! (bullet kills both ways, `kill`, every stock class, a team swap,
//! autoassign, spectator and back, a cooked grenade, a melee kill, a
//! firefight, a suicide during the other's killcam, a disconnect and a new
//! join), and after each one each fires at the other's head and the victim
//! must lose health; once with the cmds on the server's clock, once a
//! 120 ms link behind it, so every shot rewinds the bodies
//! (docs/research/cod4-combat.md 10.7). Needs `COD4_DIR`.
//!
//! `SOAK_REPORT=1` prints every step.

use std::time::{Duration, Instant};

use glam::Vec3;

use super::wire_ab::Probe;
use super::*;
use crate::follow::SessionState;

/// Where the checks stand the two: the kill capture's target spot and
/// one 150 units off it (cod4-combat.md 8).
const SPOTS: [[f32; 3]; 2] = [[207.0, 2291.0, 296.0], [140.0, 2146.0, 296.0]];
/// A melee stands the attacker this far from the victim.
const MELEE_DIST: f32 = 40.0;
/// Give up on one step past this: long enough for a match's end, its
/// intermission and the next level's join.
const STEP_MS: i32 = 90_000;

/// One transition, then the two-way check.
#[derive(Clone, Copy, Debug)]
enum Act {
    /// Nothing but the check.
    Check,
    /// `attacker` shoots the other until it dies.
    ShootDead(usize),
    /// `kill` from `slot`.
    Suicide(usize),
    /// The class menu's answer from both, then `kill` so it takes.
    Class(&'static str),
    /// The team menu's answers (target's, shooter's), then the class.
    Teams(&'static str, &'static str),
    /// `slot` to spectator and back to `team`.
    Spectate(usize, &'static str),
    /// `slot` to spectator for `ms`, then back to `team`.
    SpectateFor(usize, &'static str, i32),
    /// `slot` holds the frag key past the fuse.
    Cook(usize),
    /// `attacker` knifes the other until it dies.
    MeleeDead(usize),
    /// Both fire at once until one is down.
    Trade,
    /// `attacker` shoots the other dead, then `kill`s itself while the
    /// other watches its killcam.
    KillInKillcam(usize),
    /// `slot` sends `disconnect` and joins again as a new client on its
    /// team.
    Reconnect(usize, &'static str),
}

const PLAN: &[Act] = &[
    Act::Check,
    Act::ShootDead(1),
    Act::Check,
    Act::ShootDead(0),
    Act::Check,
    Act::Suicide(0),
    Act::Check,
    Act::Suicide(1),
    Act::Check,
    Act::Class("specops_mp,0"),
    Act::Class("heavygunner_mp,0"),
    Act::Class("demolitions_mp,0"),
    Act::Class("sniper_mp,0"),
    Act::Class("assault_mp,0"),
    Act::Teams("axis", "allies"),
    Act::Teams("autoassign", "autoassign"),
    Act::Teams("allies", "axis"),
    Act::Spectate(0, "allies"),
    Act::Spectate(1, "axis"),
    Act::Cook(0),
    Act::Check,
    Act::MeleeDead(1),
    Act::Check,
    Act::MeleeDead(0),
    Act::Check,
    Act::Trade,
    Act::Check,
    Act::KillInKillcam(0),
    Act::Check,
    Act::KillInKillcam(1),
    Act::Check,
    Act::Reconnect(0, "allies"),
    Act::Reconnect(1, "axis"),
];

/// Where one step stands.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// The transition's commands go out.
    Start,
    /// The transition runs, from the time given.
    Act(i32),
    /// Waits for both to stand.
    Settle,
    /// Both are put on their spots; shooting starts at the time.
    Place(i32),
    /// `attacker` fires at the victim, whose health was the value, from
    /// the time.
    Fire {
        attacker: usize,
        health: i32,
        at: i32,
    },
}

struct Soak {
    plan: &'static [Act],
    step: usize,
    phase: Phase,
    step_start: i32,
    /// The check's outcomes: (step, attacker, damaged).
    results: Vec<(usize, usize, bool)>,
    log: Vec<String>,
    /// Keys `slot` holds this tick: (buttons, aim at the other).
    hold: [u32; 2],
    /// Each role's server slot (a client joining again takes a new one).
    slot: [usize; 2],
    /// The attackers still to check this step.
    pending: Vec<usize>,
}

fn playing(level: &mut crate::game::cod4::level::Cod4Level, slot: usize) -> bool {
    let session = level.rt.client_session(slot).map(|s| s.state);
    let Some(ps) = level.pm_state(slot) else {
        return false;
    };
    session == Some(SessionState::Playing)
        && ps.pm_type == 0
        && ps.pm_flags & vcod_cod4::pmove::pmf::FROZEN == 0
        && ps.weap_flags & 0x80 == 0
        && ps.weapon != 0
        && level.rt.client_vitals(slot).health > 0
}

/// The cmd that views `at` from `slot`'s eye with `buttons`.
pub(super) fn aim_cmd(
    level: &mut crate::game::cod4::level::Cod4Level,
    slot: usize,
    at: Option<Vec3>,
    buttons: u32,
    svs: i32,
    lag_ms: i32,
) -> Option<UserCmd> {
    let ps = level.pm_state(slot)?;
    let eye = crate::game::cod4::combat::view_origin(&ps, ps.command_time);
    let view = match at {
        Some(at) => {
            let d = at - eye;
            let flat = (d.x * d.x + d.y * d.y).sqrt();
            [-d.z.atan2(flat).to_degrees(), d.y.atan2(d.x).to_degrees()]
        }
        None => [ps.view_angles[0], ps.view_angles[1]],
    };
    let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
    Some(UserCmd {
        server_time: svs + 10 - lag_ms,
        buttons,
        weapon: ps.weapon as u8,
        offhand: ps.off_hand_index as u8,
        angles: [
            short(view[0]).wrapping_sub(short(ps.delta_angles[0])),
            short(view[1]).wrapping_sub(short(ps.delta_angles[1])),
            0u16.wrapping_sub(short(ps.delta_angles[2])),
        ],
        ..Default::default()
    })
}

impl Soak {
    fn say(&mut self, svs: i32, s: String) {
        if std::env::var_os("SOAK_REPORT").is_some() {
            eprintln!("{svs} step {} {s}", self.step);
        }
        self.log.push(format!("{svs} step {} {s}", self.step));
    }

    fn next_step(&mut self, svs: i32) {
        self.step += 1;
        self.phase = Phase::Start;
        self.step_start = svs;
    }

    /// One tick: the step's commands, its keys and its checks.
    fn tick(&mut self, sv: &mut Cod4Server, probes: &mut [Probe], svs: i32) {
        self.hold = [0; 2];
        let Some(&act) = self.plan.get(self.step) else {
            return;
        };
        if svs - self.step_start > STEP_MS {
            self.say(svs, format!("{act:?} timed out in {:?}", self.phase));
            self.results.push((self.step, 9, false));
            self.next_step(svs);
            return;
        }
        let sl = self.slot;
        let level = sv.game.level.as_mut().unwrap();
        let both = playing(level, sl[0]) && playing(level, sl[1]);
        if svs % 2000 == 0 {
            let mut st = Vec::new();
            for k in sl {
                let ses = level.rt.client_session(k).map(|x| x.state);
                let team = level.rt.client_field(k, "sessionteam");
                let ss = level.rt.client_field(k, "sessionstate");
                let ps = level
                    .pm_state(k)
                    .map(|p| (p.pm_type, p.pm_flags, p.weapon, p.weap_flags));
                st.push(format!(
                    "{k}: {ses:?} {team:?} {ss:?} {ps:?} hp {}",
                    level.rt.client_vitals(k).health
                ));
            }
            let slots: Vec<_> = (0..4)
                .map(|k| {
                    sv.core.clients[k]
                        .as_ref()
                        .map(|c| format!("{:?}", c.state))
                })
                .collect();
            self.say(svs, format!("state {} clients {slots:?}", st.join(" | ")));
        }
        match self.phase {
            Phase::Start => {
                // The last check's burst may still be killing someone.
                if !both || svs - self.step_start < 1000 {
                    return;
                }
                let mut cmd = |slot: usize, t: &str| {
                    probes[slot].command(t);
                };
                match act {
                    Act::Check
                    | Act::ShootDead(_)
                    | Act::Cook(_)
                    | Act::MeleeDead(_)
                    | Act::Trade
                    | Act::KillInKillcam(_) => {}
                    Act::Suicide(s) => cmd(s, "kill"),
                    Act::Class(c) => {
                        for s in 0..2 {
                            cmd(s, &format!("mr {{sid}} 15 {c}"));
                            cmd(s, "kill");
                        }
                    }
                    Act::Teams(a, b) => {
                        cmd(0, &format!("mr {{sid}} 8 {a}"));
                        cmd(1, &format!("mr {{sid}} 8 {b}"));
                    }
                    Act::Spectate(s, _) | Act::SpectateFor(s, _, _) => {
                        cmd(s, "mr {sid} 8 spectator")
                    }
                    Act::Reconnect(s, _) => cmd(s, "disconnect"),
                }
                self.say(svs, format!("{act:?} starts"));
                self.pending = vec![0, 1];
                self.phase = Phase::Act(svs);
            }
            Phase::Act(from) => {
                let t = svs - from;
                let done = match act {
                    Act::ShootDead(a) | Act::MeleeDead(a) => {
                        let v = 1 - a;
                        if !playing(level, sl[v]) {
                            true
                        } else {
                            let melee = matches!(act, Act::MeleeDead(_));
                            if t % 2000 == 0 {
                                level.set_view_pos(sl[v], SPOTS[v], [0.0; 3]);
                                let spot = if melee {
                                    let (v0, a0) = (Vec3::from(SPOTS[v]), Vec3::from(SPOTS[a]));
                                    (v0 + (a0 - v0).normalize() * MELEE_DIST).to_array()
                                } else {
                                    SPOTS[a]
                                };
                                level.set_view_pos(sl[a], spot, [0.0; 3]);
                            }
                            self.hold[a] = if melee {
                                if t % 800 == 300 { 4 } else { 0 }
                            } else if t % 300 == 200 {
                                1
                            } else {
                                0
                            };
                            false
                        }
                    }
                    Act::Cook(s) => {
                        self.hold[s] = vcod_cod4::pmove::button::FRAG;
                        !playing(level, sl[s]) || t > 6000
                    }
                    Act::Teams(..) => {
                        if t == 1000 {
                            for p in probes.iter_mut().take(2) {
                                p.command("mr {sid} 15 assault_mp,0");
                            }
                        }
                        t > 1000
                    }
                    Act::SpectateFor(s, team, ms) => {
                        if t == ms {
                            probes[s].command(&format!("mr {{sid}} 8 {team}"));
                        }
                        if t == ms + 1000 {
                            probes[s].command("mr {sid} 15 assault_mp,0");
                        }
                        t > ms + 1000
                    }
                    Act::Spectate(s, team) => {
                        let spectating = level
                            .rt
                            .client_session(sl[s])
                            .is_some_and(|x| x.state == SessionState::Spectator);
                        // The class a second after the team, as a player
                        // clicks them.
                        if spectating && t == 1500 {
                            probes[s].command(&format!("mr {{sid}} 8 {team}"));
                            self.say(svs, format!("slot {s} spectating, back to {team}"));
                        }
                        if t == 2500 {
                            probes[s].command("mr {sid} 15 assault_mp,0");
                        }
                        t > 2500
                    }
                    Act::Trade => {
                        let (up0, up1) = (playing(level, sl[0]), playing(level, sl[1]));
                        if t % 2000 == 50 {
                            level.set_view_pos(sl[0], SPOTS[0], [0.0; 3]);
                            level.set_view_pos(sl[1], SPOTS[1], [0.0; 3]);
                        }
                        let tap = u32::from(t % 300 == 200);
                        self.hold = [0x800 | tap, 0x800 | tap];
                        !up0 || !up1
                    }
                    Act::KillInKillcam(a) => {
                        let v = 1 - a;
                        let watching = level
                            .rt
                            .client_session(sl[v])
                            .is_some_and(|x| x.state == SessionState::Spectator);
                        if playing(level, sl[v]) {
                            if t % 2000 == 0 {
                                level.set_view_pos(sl[v], SPOTS[v], [0.0; 3]);
                                level.set_view_pos(sl[a], SPOTS[a], [0.0; 3]);
                            }
                            self.hold[a] = 0x800 | u32::from(t % 300 == 200);
                            false
                        } else if watching && playing(level, sl[a]) {
                            probes[a].command("kill");
                            self.say(svs, format!("{a} kills itself while {v} watches"));
                            true
                        } else {
                            false
                        }
                    }
                    Act::Reconnect(s, team) => {
                        if t == 500 {
                            let names = ["target", "shooter"];
                            let ports = [28971, 28972];
                            let answers: &'static [(i32, &'static str)] = match team {
                                "allies" => &[
                                    (1050, "mr {sid} 8 allies"),
                                    (2550, "mr {sid} 15 assault_mp,0"),
                                ],
                                _ => &[
                                    (1050, "mr {sid} 8 axis"),
                                    (2550, "mr {sid} 15 assault_mp,0"),
                                ],
                            };
                            // A new client takes the first slot neither
                            // in use nor a zombie.
                            let free = (0..sv.core.clients.len())
                                .find(|&k| sv.core.clients[k].is_none())
                                .unwrap();
                            self.slot[s] = free;
                            let mut p =
                                Probe::new(names[s], free as u32, ports[s], svs + 4100, answers);
                            p.replay_from = 0;
                            probes[s] = p;
                            self.say(svs, format!("slot {s} joins again"));
                        }
                        t > 500
                    }
                    Act::Check | Act::Suicide(_) | Act::Class(_) => t >= 500,
                };
                if done {
                    self.say(svs, format!("{act:?} done"));
                    self.phase = Phase::Settle;
                }
            }
            Phase::Settle => {
                if both {
                    level.set_view_pos(sl[0], SPOTS[0], [0.0; 3]);
                    level.set_view_pos(sl[1], SPOTS[1], [0.0; 3]);
                    self.phase = Phase::Place(svs + 300);
                }
            }
            Phase::Place(at) => {
                if !both {
                    self.phase = Phase::Settle;
                } else if svs >= at {
                    let attacker = self.pending[0];
                    let health = level.rt.client_vitals(sl[1 - attacker]).health;
                    self.phase = Phase::Fire {
                        attacker,
                        health,
                        at: svs,
                    };
                }
            }
            Phase::Fire {
                attacker,
                health,
                at,
            } => {
                let v = 1 - attacker;
                let now = level.rt.client_vitals(sl[v]).health;
                let dead = !playing(level, sl[v]);
                if now < health || dead {
                    self.say(svs, format!("{attacker} hurt {v}: {health} -> {now}"));
                    self.results.push((self.step, attacker, true));
                } else if !playing(level, sl[attacker]) {
                    // Killed before its turn (the other's burst): again
                    // once both stand.
                    self.phase = Phase::Settle;
                    return;
                } else if svs - at > 5000 {
                    let team = |l: &mut crate::game::cod4::level::Cod4Level, s| {
                        l.rt.client_field(s, "sessionteam").unwrap_or_default()
                    };
                    let (ta, tv) = (team(level, sl[attacker]), team(level, sl[v]));
                    self.say(
                        svs,
                        format!(
                            "{attacker} did NOT hurt {v} (health {now}, teams {ta}/{tv}, hits {:?})",
                            level.combat_hits().iter().rev().take(3).collect::<Vec<_>>()
                        ),
                    );
                    self.results.push((self.step, attacker, false));
                } else {
                    // The sight up, then a tap every 700 ms (an empty
                    // clip reloads in between).
                    self.hold[attacker] = 0x800 | u32::from((svs - at) % 700 == 600);
                    return;
                }
                self.pending.remove(0);
                if self.pending.is_empty() {
                    self.next_step(svs);
                } else {
                    self.phase = Phase::Settle;
                }
            }
        }
    }
}

/// Every transition of [`PLAN`], then a round each way that hurts.
#[test]
fn cod4_damage_survives_every_transition() {
    let run = soak_run(PLAN, &[], 0);
    // Every corpse falls: the team and spectator switches' `suicide`
    // draws a death anim as a kill does (cod4-combat.md 5).
    let standing: Vec<_> = run
        .corpse_anims
        .iter()
        .filter(|n| !n.contains("death"))
        .collect();
    assert!(
        standing.is_empty(),
        "corpses on {standing:?} of {:?}",
        run.corpse_anims
    );
}

/// [`PLAN`]'s steps, the two playing on with the remaining cvars set, and
/// a round each way that hurts after each; the soak's log on failure.
/// Returns the `sv_serverid` of every level the two played on, in order:
/// a map change or a restart answers the team and class menus again.
fn soak(plan: &'static [Act], cvars: &[(&str, &str)], lag_ms: i32) -> Vec<i32> {
    soak_run(plan, cvars, lag_ms).levels
}

/// What one soak saw besides the damage checks.
#[derive(Default)]
struct SoakRun {
    /// Each level's `sv_serverid`, in order.
    levels: Vec<i32>,
    /// Every script error on any level, each once.
    aborts: std::collections::BTreeSet<String>,
    /// Per role: a frame put it in the intermission (`pm_type` 5, the
    /// scoreboard on the client).
    intermission: [bool; 2],
    /// Per role: every server command it read, in order, across levels.
    commands: [Vec<String>; 2],
    /// Every body queue corpse's legs anim, by name.
    corpse_anims: std::collections::BTreeSet<String>,
}

fn soak_run(plan: &'static [Act], cvars: &[(&str, &str)], lag_ms: i32) -> SoakRun {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return SoakRun::default();
    };
    let mut run = SoakRun::default();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    sv.set_cvar("sv_cheats", "1");
    sv.set_cvar("sv_pure", "0");
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load(Some(&install)).unwrap();
    let mut probes = vec![
        Probe::new(
            "target",
            0,
            28961,
            43350,
            &[
                (1050, "mr {sid} 8 allies"),
                (2550, "mr {sid} 15 assault_mp,0"),
            ],
        ),
        Probe::new(
            "shooter",
            1,
            28962,
            44300,
            &[
                (1050, "mr {sid} 8 axis"),
                (2550, "mr {sid} 15 assault_mp,0"),
            ],
        ),
    ];
    for p in probes.iter_mut() {
        p.replay_from = 0;
    }
    let mut soak = Soak {
        plan,
        step: 0,
        phase: Phase::Start,
        step_start: 0,
        results: Vec::new(),
        log: Vec::new(),
        hold: [0; 2],
        pending: Vec::new(),
        slot: [0, 1],
    };
    let mut now = Instant::now();
    let mut started = false;
    let mut levels = vec![sv.server_id()];
    // Per role, once a new level is up: the time its gamestate's id came
    // in, and how many of the two menu answers went.
    let mut rejoin: [Option<(Option<i32>, u8)>; 2] = [None; 2];
    let mut seen = [0usize; 2];
    while soak.step < plan.len() && sv.core.sv_time_ms < 600_000 {
        let svs = sv.core.sv_time_ms;
        for p in probes.iter_mut() {
            if !p.joined && svs >= p.connect_at {
                p.join(&mut sv, now);
            }
        }
        if let Some(level) = sv.game.level.as_ref() {
            run.aborts.extend(level.rt.aborts());
            let names = &level.rt.host.cod4().anim_names;
            let legs = vcod_common::net::msg::EntityState::field_index(
                &crate::game::cod4::COD4_HOST_PROTO,
                "legsAnim",
            )
            .unwrap();
            for (_, e) in level.rt.body_entities(&crate::game::cod4::COD4_HOST_PROTO) {
                let wire = (e.fields[legs] & 511) as usize;
                run.corpse_anims
                    .insert(names.get(wire).cloned().unwrap_or_default());
            }
            for (s, slot) in soak.slot.iter().enumerate() {
                if level.at_intermission(*slot) {
                    run.intermission[s] = true;
                }
            }
        }
        if sv.server_id() != *levels.last().unwrap() {
            levels.push(sv.server_id());
            soak.say(svs, format!("level {} up", sv.server_id()));
            rejoin = [Some((None, 0)); 2];
        }
        for (s, r) in rejoin.iter_mut().enumerate() {
            let Some((at, sent)) = r else { continue };
            if at.is_none() && probes[s].server_id() == Some(sv.server_id()) {
                *at = Some(svs);
            }
            let Some(at) = *at else { continue };
            let team = ["allies", "axis"][s];
            if *sent == 0 && svs >= at + 1500 {
                probes[s].command(&format!("mr {{sid}} 8 {team}"));
                *sent = 1;
            } else if *sent == 1 && svs >= at + 3000 {
                probes[s].command("mr {sid} 15 assault_mp,0");
                *r = None;
            }
        }
        {
            let level = sv.game.level.as_mut().unwrap();
            if !started && playing(level, soak.slot[0]) && playing(level, soak.slot[1]) {
                started = true;
                soak.step_start = svs;
            }
        }
        if started {
            soak.tick(&mut sv, &mut probes, svs);
        }
        let level = sv.game.level.as_mut().unwrap();
        for (s, p) in probes.iter_mut().enumerate() {
            let (me, other) = (soak.slot[s], soak.slot[1 - s]);
            let head = if playing(level, other) {
                level.hit_box_center(other, 2)
            } else {
                None
            };
            if let Some(c) = aim_cmd(level, me, head, soak.hold[s], svs, lag_ms) {
                p.replay.clear();
                p.replay.push_back((svs, vec![c]));
            }
        }
        for p in probes.iter_mut().filter(|p| p.joined) {
            let pkt = p.packet(svs);
            sv.handle_packet(p.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        // `main.rs`'s console ahead of the frame: the script's `exitLevel`
        // and `map_restart` lines.
        sv.drain_console();
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if let Some(p) = probes.iter_mut().find(|p| p.joined && p.addr == to) {
                p.receive(svs, &pkt);
            }
        }
        for (s, p) in probes.iter_mut().enumerate() {
            p.answer_stats(&mut sv, now);
            p.lines.clear();
            let cmds = p.server_commands();
            if cmds.len() < seen[s] {
                seen[s] = 0;
            }
            run.commands[s].extend(cmds[seen[s]..].iter().cloned());
            seen[s] = cmds.len();
        }
    }
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    let failed: Vec<_> = soak.results.iter().filter(|r| !r.2).collect();
    assert!(started, "the two never played");
    assert_eq!(
        soak.step,
        plan.len(),
        "ran out of time (aborts {aborts:?}):\n{}",
        soak.log.join("\n")
    );
    assert!(
        failed.is_empty(),
        "no damage at {failed:?} (aborts {aborts:?}):\n{}",
        soak.log.join("\n")
    );
    run.levels = levels;
    run
}

/// Checks across the end of a one-minute match: `endGame` sets
/// `game["state"]` to `postgame`, which `Callback_PlayerDamage` returns on,
/// and the intermission's `exitLevel(false)` runs `map_rotate`, which
/// spawns the rotation's next map, a new gamestate for both
/// (cod4-combat.md 10.7, cod4-map-cycle.md 1, 3).
#[test]
fn cod4_damage_survives_the_time_limit() {
    let levels = soak(
        &[Act::Check; 12],
        &[
            ("scr_war_timelimit", "1"),
            ("sv_mapRotation", "gametype war map mp_crash"),
        ],
        0,
    );
    if levels.is_empty() {
        return;
    }
    assert!(levels.len() >= 2, "no map change: {levels:?}");
    assert_ne!(levels[1] & 0xf0, levels[0] & 0xf0, "{levels:?}");
}

/// A ranked match's end (`sv_pure 1`, `level.rankedMatch`): `endGame`
/// runs `setXenonRanks` and its `sendRanks` (empty on the PC build), both
/// clients go to the intermission, which the client draws as the
/// scoreboard (`pm_type` 5), the rotation's next map comes up and damage
/// works there, with no script error on any level.
#[test]
fn cod4_ranked_match_ends_with_the_scoreboard_and_rotates() {
    let run = soak_run(
        &[Act::Check; 12],
        &[
            ("sv_pure", "1"),
            ("scr_war_timelimit", "1"),
            ("sv_mapRotation", "gametype war map mp_crash"),
        ],
        0,
    );
    if run.levels.is_empty() {
        return;
    }
    assert!(run.levels.len() >= 2, "no map change: {:?}", run.levels);
    assert_eq!(run.intermission, [true; 2], "no intermission");
    assert!(run.aborts.is_empty(), "script errors:\n{:?}", run.aborts);
    // The leader dialog reaches each client as `playLocalSound`'s `s <n>`,
    // the alias at configstring 1342 + n written by a `d` before it.
    for cmds in &run.commands {
        let played = local_sounds(cmds);
        for line in ["1mc_team_deathmtch", "1mc_timesup"] {
            assert!(
                played.iter().any(|a| a.contains(line)),
                "{line}: {played:?}"
            );
        }
    }
}

/// The aliases a client's `s <n>` commands play, by the `d 1342+n` that
/// last named each slot.
fn local_sounds(cmds: &[String]) -> Vec<String> {
    let mut slots = std::collections::HashMap::new();
    let mut out = Vec::new();
    for c in cmds {
        if let Some(rest) = c.strip_prefix("d ")
            && let Some((i, v)) = rest.split_once(' ')
            && let Ok(i) = i.parse::<usize>()
        {
            slots.insert(i, v.to_string());
        } else if let Some(n) = c.strip_prefix("s ").and_then(|n| n.parse::<usize>().ok())
            && let Some(a) = slots.get(&(1342 + n))
        {
            out.push(a.clone());
        }
    }
    out
}

/// The same with a rotation that names no map (over `server_map.cfg`'s):
/// `map_rotate` falls back to `fast_restart`, the level restarted in place
/// under `B` (cod4-map-cycle.md 2).
#[test]
fn cod4_damage_survives_the_time_limit_restarted() {
    let levels = soak(
        &[Act::Check; 12],
        &[
            ("scr_war_timelimit", "1"),
            ("sv_mapRotation", "gametype war"),
        ],
        0,
    );
    if levels.is_empty() {
        return;
    }
    assert!(levels.len() >= 2, "no restart: {levels:?}");
    assert_eq!(levels[1], super::cycle::restart_server_id(levels[0]));
}

/// [`PLAN`] with each client's cmds a link behind the server's clock, so
/// every shot rewinds the bodies (`g_antilag`).
#[test]
fn cod4_damage_survives_every_transition_on_a_lagged_link() {
    soak(PLAN, &[], 120);
}

/// One side leaves for the spectators past the forfeit's 20 s and comes
/// back: no forfeit ends the match (`updateGameEvents` forfeits only a
/// ranked match).
#[test]
fn cod4_damage_survives_a_long_spectate() {
    soak(
        &[
            Act::Check,
            Act::SpectateFor(0, "allies", 25_000),
            Act::Check,
            Act::Check,
        ],
        &[],
        0,
    );
}

/// On every stock map both teams' bodies pose their head boxes once they
/// play, and each class's: the hit boxes a shot or a knife needs
/// (`SOAK_MAPS` narrows the list).
#[test]
#[ignore]
fn cod4_every_map_poses_both_teams() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let mut maps: Vec<String> = install
        .zones("english")
        .unwrap()
        .iter()
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_string))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .collect();
    if let Ok(only) = std::env::var("SOAK_MAPS") {
        maps.retain(|m| only.split(',').any(|o| o == m));
    }
    let classes = [
        "assault_mp,0",
        "specops_mp,0",
        "heavygunner_mp,0",
        "demolitions_mp,0",
        "sniper_mp,0",
    ];
    let mut bad = Vec::new();
    for map in &maps {
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: map.clone(),
                gametype: "war".into(),
                hostname: "CoD4Host".into(),
                max_clients: 8,
            },
            7,
        );
        sv.set_cvar("sv_cheats", "1");
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(&install)).unwrap();
        let mut probes = [
            Probe::new(
                "target",
                0,
                28961,
                43350,
                &[
                    (1050, "mr {sid} 8 allies"),
                    (2550, "mr {sid} 15 assault_mp,0"),
                ],
            ),
            Probe::new(
                "shooter",
                1,
                28962,
                44300,
                &[
                    (1050, "mr {sid} 8 axis"),
                    (2550, "mr {sid} 15 assault_mp,0"),
                ],
            ),
        ];
        let mut now = Instant::now();
        let mut class = 0;
        let mut seen = Vec::new();
        let start = sv.core.sv_time_ms;
        // The joins answer the first class.
        let mut asked = Some(start);
        while class < classes.len() && sv.core.sv_time_ms < start + 240_000 {
            let svs = sv.core.sv_time_ms;
            for p in probes.iter_mut() {
                if !p.joined && svs >= p.connect_at {
                    p.join(&mut sv, now);
                }
            }
            let level = sv.game.level.as_mut().unwrap();
            if asked.is_none_or(|a| svs - a > 2000) && (0..2).all(|s| playing(level, s)) {
                if asked.is_some() {
                    let heads = [level.hit_box_center(0, 2), level.hit_box_center(1, 2)];
                    let models = [
                        level.rt.client_field(0, "model"),
                        level.rt.client_field(1, "model"),
                    ];
                    if heads.iter().any(Option::is_none) {
                        bad.push(format!("{map} {} {models:?}", classes[class]));
                    }
                    seen.push(models);
                    class += 1;
                }
                if class < classes.len() {
                    for p in probes.iter_mut() {
                        p.command(&format!("mr {{sid}} 15 {}", classes[class]));
                        p.command("kill");
                    }
                }
                asked = Some(svs);
            }
            for p in probes.iter_mut().filter(|p| p.joined) {
                let pkt = p.packet(svs);
                sv.handle_packet(p.addr, &pkt, now);
            }
            now += Duration::from_millis(FRAME_MS as u64);
            sv.tick_frames(now, 1);
            let svs = sv.core.sv_time_ms;
            for (to, pkt) in sv.take_outgoing() {
                if let Some(p) = probes.iter_mut().find(|p| p.joined && p.addr == to) {
                    p.receive(svs, &pkt);
                }
            }
            for p in probes.iter_mut() {
                p.lines.clear();
            }
        }
        eprintln!("{map}: {} classes {seen:?}", class);
        if class < classes.len() {
            bad.push(format!("{map}: only {class} classes played"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
