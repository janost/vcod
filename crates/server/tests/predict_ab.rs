//! The client's predictor (`vcod_common::pmove::predict`) against the
//! server's sim and against retail's mover.
//!
//! `predictor_matches_the_server_step` runs a scripted cmd stream through
//! `ClientSim` the way `replay_moves` does and, every fifth cmd, rebuilds the
//! predictor from the playerstate the server would send, round-tripped
//! through the wire codec, then runs the next cmds on both and compares the
//! fields prediction draws from. Both run `pmove::cmd`'s step, so what this
//! pins is `from_wire` against `to_wire` and the predictor's wrapping against
//! the server's: everything is exact except what the wire itself narrows. The event ring's half,
//! `predictor_ring_matches_the_server_step`, needed `bobCycle` on the wire
//! before it could pass; it does now.
//!
//! `predictor_matches_the_uo_server_step` and
//! `predictor_matches_the_1_5_server_step` run the same comparison on a
//! protocol 22 and a protocol 6 sim, fed retail's own cmd streams (the
//! `moves_ab.rs` slope captures: UO's sprint, bipod, automatic fire, mounts
//! and cook, its tank and jeep seats, a mounted MG, 1.5's moves and turns). The UO one runs
//! at a UO install, the 1.5 one at a 1.5 or UO install.
//!
//! `predictor_replays_retail_slope_runs` (and its `_1_5` and `_uo` twins)
//! replays every 1.1d (1.5b, UO 1.51b) slope capture that logs the whole
//! playerstate through the predictor under its version's rules: each
//! retail snapshot rebuilt through `from_wire`, retail's cmds run through
//! `run_cmd`, the next snapshot's origin matched to the bit, but where the
//! server moved the body and the rows `REPLAY_GAPS` names.
//!
//! `predictor_rides_retail_movers` predicts the ride capture's player from
//! one snapshot to the next while the slab under him moves: the slab clipped
//! where the snapshot's trajectory has it, the idle cmds run, the carry to
//! the next snapshot's time (docs/research/cod11-movers.md, section 14).
//! All three need `COD_DIR`; without the paks they return early.

use glam::Vec3;
use std::collections::{BTreeMap, HashMap};
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::{Body, CONTENTS_BODY, MoveWorld};
use vcod_common::net::huffman::Huffman;
use vcod_common::net::msg::{
    self, BUTTON_ADS, BUTTON_ATTACK, NULL_USERCMD, UserCmd, WBUTTON_CROUCH, WBUTTON_PRONE,
    WBUTTON_RELOAD,
};
use vcod_common::net::protocol::{
    ENTITYNUM_WORLD, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol,
};
use vcod_common::net::trajectory::Trajectory;
use vcod_common::pmove::movers::SnapshotMovers;
use vcod_common::pmove::predict::{self, Predicted};
use vcod_common::pmove::{self as pm, weapon::WEAPON_READY};
use vcod_common::weapon::WeaponDef;
use vcod_server::spectate::ClientSim;

const P: &Protocol = &PROTOCOL_V1;
const ANGLE2SHORT: f32 = pm::cmd::ANGLE2SHORT;

/// Retail's configstring 7 on mp_carentan dm.
fn retail_cs7() -> &'static str {
    include_str!("fixtures/configstrings/mp_carentan-dm.txt")
        .lines()
        .find_map(|l| l.strip_prefix("7 "))
        .expect("configstring 7 in the fixture")
}

/// A weapon's configstring 7 index, which is what `ps.weapon` and the
/// usercmd's weapon byte carry.
fn weapon_index(cs7: &str, name: &str) -> u8 {
    cs7.split(' ').position(|n| n == name).expect(name) as u8 + 1
}

/// A map's collision world the way `World::from_bsp` builds it, and the
/// entity string for the spawn and `unlink_script_brushes`.
fn load_world(fs: &vcod_common::pk3::Pk3Fs, map: &str) -> (CollisionWorld, String) {
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("parse");
    let world = CollisionWorld::build(&bsp, &vcod_common::props::collision_tris(fs, &bsp.entities));
    (world, bsp.entities.clone())
}

/// What a client decodes: the playerstate written as a delta from the null
/// state and read back, so every field arrives at its netfield width.
fn over_the_wire(p: &Protocol, w: &msg::PlayerState) -> msg::PlayerState {
    // Built once: the tree costs a debug build over 200 ms.
    static HUFF: std::sync::OnceLock<Huffman> = std::sync::OnceLock::new();
    let huff = HUFF.get_or_init(Huffman::new);
    let null = msg::PlayerState::null(p);
    let mut writer = msg::MsgWriter::new(huff);
    msg::write_delta_playerstate(&mut writer, p, &null, w);
    let data = writer.finish();
    let mut reader = msg::MsgReader::new(&data, huff);
    let out = msg::read_delta_playerstate(&mut reader, p, &null);
    assert!(
        !reader.is_overflowed(),
        "the playerstate did not round-trip"
    );
    out
}

// ---------------------------------------------------------------------------
// The server's step against the predictor's.

/// The server sim and the predictor, fed the same cmds.
struct Run<'a> {
    /// The wire both sides speak.
    p: &'static Protocol,
    sim: ClientSim,
    pred: Predicted,
    world: &'a CollisionWorld,
    /// The other players, the same on both sides; the sim is client 0.
    bodies: Vec<Body>,
    weapons: &'a [Option<WeaponDef>],
    /// `c.last_processed_st`: the server's clock for this client.
    st: i32,
    last_cmd: UserCmd,
    /// Current view in ANGLE2SHORT units, pitch and yaw.
    view: [i32; 2],
    cmds: usize,
    /// Whether the event ring is compared too; see `the_ring_matches`.
    ring: bool,
    rebuilds: usize,
    /// Rebuilds that landed inside a putaway, a stance's eye lerp, and cmds
    /// whose prone correction moved `delta_angles`: the script's cover.
    rebuilds_dropping: usize,
    rebuilds_mid_lerp: usize,
    prone_corrections: usize,
    /// Cmds run while the sim's `pm_type` is one the client does not predict.
    unpredicted: usize,
    mismatches: Vec<String>,
    /// Per field, how many cmds read it wrong.
    counts: BTreeMap<&'static str, usize>,
}

impl Run<'_> {
    /// One cmd `dt_ms` after the last, on both sides. A template weapon byte
    /// of 0 sends the weapon the server says is held, as a retail client
    /// does.
    fn send(&mut self, template: UserCmd, dt_ms: i32) {
        if self.cmds.is_multiple_of(5) {
            self.rebuild();
        }
        let da = self.sim.delta_angles();
        let cmd = UserCmd {
            server_time: self.st + dt_ms,
            weapon: if template.weapon == 0 {
                self.sim.ps.weapon
            } else {
                template.weapon
            },
            angles: [self.view[0], self.view[1], 0],
            ..template
        };
        self.server_step(&cmd);
        self.prone_corrections += usize::from(self.sim.delta_angles() != da);
        // The client predicts every state but a linked dead body's.
        if !predict::predictable(self.sim.wire_pm_type()) {
            self.last_cmd = cmd;
            self.cmds += 1;
            self.unpredicted += 1;
            return;
        }
        predict::run_cmd(
            &mut self.pred,
            &cmd,
            &MoveWorld::new(self.world, &self.bodies, 0),
            self.weapons,
        );
        self.compare();
        self.last_cmd = cmd;
        self.cmds += 1;
    }

    /// The predictor rebuilt from the snapshot the server would send now.
    fn rebuild(&mut self) {
        // The snapshot's own map into the protocol's numbering (events,
        // `eFlags`), as `Server`'s snapshot pass runs it.
        let mut w = self.sim.to_wire(self.p, 0, self.st);
        vcod_server::game::wire::ps_to_protocol(self.p, &mut w);
        let w = over_the_wire(self.p, &w);
        self.pred = predict::from_wire(self.p, &w, Some(&self.last_cmd));
        // The vehicle's angles come off its entity, as the client's
        // `seat_angles` do.
        if let (Some(seat), Some(on)) = (&mut self.pred.ps.seat, self.sim.seat) {
            seat.angles = on.angles;
        }
        self.rebuilds += 1;
        self.rebuilds_dropping +=
            usize::from(self.pred.ps.weaponstate == pm::weapon::WEAPON_DROPPING);
        self.rebuilds_mid_lerp += usize::from(self.pred.view_lerp_start != 0);
    }

    /// `replay_moves`' per-cmd body for one client, minus what reaches past
    /// the sim (attacks, touches, anims).
    fn server_step(&mut self, cmd: &UserCmd) {
        let dt_ms = cmd.server_time.wrapping_sub(self.st);
        if dt_ms <= 0 {
            return;
        }
        self.sim.update_aim(dt_ms, cmd.server_time, self.weapons);
        pm::cmd::chop(self.st, cmd, |step, dt| {
            self.sim.step(
                step,
                dt,
                Some(MoveWorld::new(self.world, &self.bodies, 0)),
                self.weapons,
            );
            self.sim.ps.jump_latched
        });
        self.st = cmd.server_time;
    }

    fn compare(&mut self) {
        let (s, p) = (&self.sim.ps, &self.pred.ps);
        let ring = self.sim.ring;
        let sda = self.sim.delta_angles();
        let mut bad: Vec<(&'static str, String, String)> = Vec::new();
        let mut check = |name: &'static str, a: String, b: String| {
            if a != b {
                bad.push((name, a, b));
            }
        };
        let bits = |v: Vec3| v.to_array().map(f32::to_bits);
        if bits(s.origin) != bits(p.origin) {
            check(
                "origin",
                format!("{:?}", s.origin),
                format!("{:?}", p.origin),
            );
        }
        if bits(s.velocity) != bits(p.velocity) {
            check(
                "velocity",
                format!("{:?}", s.velocity),
                format!("{:?}", p.velocity),
            );
        }
        check(
            "stance",
            format!("{:?}", s.stance),
            format!("{:?}", p.stance),
        );
        check(
            "on_ground",
            s.on_ground.to_string(),
            p.on_ground.to_string(),
        );
        // 16 bits on the wire; the server keeps its sum unmasked.
        check(
            "delta_angles",
            format!("{:?}", sda.map(|v| v & 0xffff)),
            format!("{:?}", self.pred.delta_angles),
        );
        check("weapon", s.weapon.to_string(), p.weapon.to_string());
        check(
            "weaponstate",
            s.weaponstate.to_string(),
            p.weaponstate.to_string(),
        );
        check(
            "weapon_time_ms",
            s.weapon_time_ms.to_string(),
            p.weapon_time_ms.to_string(),
        );
        check(
            "weapon_delay_ms",
            s.weapon_delay_ms.to_string(),
            p.weapon_delay_ms.to_string(),
        );
        check(
            "weap_anim",
            s.weap_anim.to_string(),
            p.weap_anim.to_string(),
        );
        if s.ammoclip != p.ammoclip {
            let diff: Vec<_> = (0..s.ammoclip.len())
                .filter(|&i| s.ammoclip[i] != p.ammoclip[i])
                .map(|i| format!("[{i}] {} vs {}", s.ammoclip[i], p.ammoclip[i]))
                .collect();
            check("ammoclip", diff.join(", "), String::new());
        }
        check(
            "weapon_pos_frac",
            s.weapon_pos_frac.to_bits().to_string(),
            p.weapon_pos_frac.to_bits().to_string(),
        );
        check(
            "view_height",
            format!("{:?}", s.view_height()),
            format!("{:?}", p.view_height()),
        );
        // The rest of what a rebuild restores, bit for bit: a field the wire
        // drops reads its spawn value here on the cmd after a rebuild.
        let fbits = |v: f32| v.to_bits().to_string();
        check(
            "aim_spread_scale",
            fbits(s.aim_spread_scale),
            fbits(p.aim_spread_scale),
        );
        check("lean", fbits(s.lean), fbits(p.lean));
        check(
            "prone_direction",
            fbits(s.prone_direction),
            fbits(p.prone_direction),
        );
        check(
            "prone_direction_pitch",
            fbits(s.prone_direction_pitch),
            fbits(p.prone_direction_pitch),
        );
        check(
            "prone_torso_pitch",
            fbits(s.prone_torso_pitch),
            fbits(p.prone_torso_pitch),
        );
        check(
            "movement_dir",
            s.movement_dir.to_string(),
            p.movement_dir.to_string(),
        );
        check(
            "bob_cycle",
            s.bob_cycle.to_string(),
            p.bob_cycle.to_string(),
        );
        check(
            "jump_origin_z",
            fbits(s.jump_origin_z),
            fbits(p.jump_origin_z),
        );
        check(
            "ads_active",
            s.ads_active.to_string(),
            p.ads_active.to_string(),
        );
        check(
            "grenade_time_left_ms",
            s.grenade_time_left_ms.to_string(),
            p.grenade_time_left_ms.to_string(),
        );
        check(
            "weapon_rechamber",
            s.weapon_rechamber.to_string(),
            p.weapon_rechamber.to_string(),
        );
        check("ammo", format!("{:?}", s.ammo), format!("{:?}", p.ammo));
        check(
            "knockback",
            format!("{} {}", s.knockback_ms, s.knockback_flags),
            format!("{} {}", p.knockback_ms, p.knockback_flags),
        );
        check(
            "mounted",
            format!("{:?}", s.mounted),
            format!("{:?}", p.mounted),
        );
        check(
            "jump_penalty",
            s.jump_penalty.to_string(),
            p.jump_penalty.to_string(),
        );
        // Unread under 1.1's rules, and by a dead body's step: a corpse's
        // goes out as `deadViewHeight` from its second snapshot on, as
        // retail's does.
        if s.rules.v15() && self.sim.wire_pm_type() != vcod_common::net::flags::PM_DEAD {
            check("view_target", fbits(s.view_target), fbits(p.view_target));
        }
        check(
            "sprint",
            format!("{} {}", s.sprinting, s.winded),
            format!("{} {}", p.sprinting, p.winded),
        );
        check("fatigue", fbits(s.fatigue), fbits(p.fatigue));
        check(
            "last_sprint_time",
            s.last_sprint_time.to_string(),
            p.last_sprint_time.to_string(),
        );
        check(
            "fatigue_sound_time",
            s.fatigue_sound_time.to_string(),
            p.fatigue_sound_time.to_string(),
        );
        check(
            "mount_hint",
            format!("{:?}", s.mount_hint),
            format!("{:?}", p.mount_hint),
        );
        if !self.ring {
            return self.record(bad);
        }
        // `eventSequence` and the slots are 8 bits on the wire.
        check(
            "event_sequence",
            ring.seq.to_string(),
            self.pred.ring.seq.to_string(),
        );
        // The server's slot as the client reads it back: the protocol's id,
        // 8 bits of it, in 1.1's numbering again.
        let p = self.p;
        check(
            "events",
            format!(
                "{:?}",
                ring.events
                    .map(|e| (p.event_to_v1)((p.event_from_v1)(e) & 0xff))
            ),
            format!("{:?}", self.pred.ring.events),
        );
        check(
            "event_parms",
            format!("{:?}", ring.parms.map(|e| e & 0xff)),
            format!("{:?}", self.pred.ring.parms),
        );
        self.record(bad);
    }

    fn record(&mut self, bad: Vec<(&'static str, String, String)>) {
        for (name, a, b) in bad {
            *self.counts.entry(name).or_default() += 1;
            self.mismatches.push(format!(
                "cmd {} (st {}, {} since rebuild): {name}: server {a} predictor {b}",
                self.cmds,
                self.st,
                self.cmds % 5,
            ));
        }
    }

    fn turn(&mut self, pitch_deg: f32, yaw_deg: f32) {
        self.view[0] += (pitch_deg * ANGLE2SHORT) as i32;
        self.view[1] += (yaw_deg * ANGLE2SHORT) as i32;
    }

    /// `n` cmds at 8 ms, the view turned by `yaw_per_cmd` degrees each.
    fn hold(&mut self, template: UserCmd, n: usize, yaw_per_cmd: f32) {
        for _ in 0..n {
            self.turn(0.0, yaw_per_cmd);
            self.send(template, 8);
        }
    }

    /// Until the weapon machine is idle again, capped.
    fn until_ready(&mut self, template: UserCmd, cap: usize) {
        for _ in 0..cap {
            if self.sim.ps.weaponstate == WEAPON_READY && self.sim.ps.weapon_time_ms == 0 {
                return;
            }
            self.send(template, 8);
        }
        panic!(
            "weapon still busy after {cap} cmds: state {} time {}",
            self.sim.ps.weaponstate, self.sim.ps.weapon_time_ms
        );
    }
}

fn input(f: impl FnOnce(&mut UserCmd)) -> UserCmd {
    let mut c = NULL_USERCMD;
    f(&mut c);
    c
}

#[test]
fn predictor_matches_the_server_step() {
    step_side_by_side(false);
}

/// The event ring under the same script. Footsteps fire off `bobCycle`, which
/// `to_wire` now carries, so a predictor rebuilt from the wire resumes the
/// phase where the server left it instead of restarting at 0.
#[test]
fn predictor_ring_matches_the_server_step() {
    step_side_by_side(true);
}

/// A carbine and colt player at mp_carentan's first spawn, on both sides.
fn start<'a>(
    world: &'a CollisionWorld,
    entities: &str,
    weapons: &'a [Option<WeaponDef>],
    ring: bool,
) -> Run<'a> {
    let cs7 = retail_cs7();
    let (origin, yaw) = vcod_common::bsp::find_spawn(entities).expect("a spawn");
    let carbine = weapon_index(cs7, "m1carbine_mp");
    let colt = weapon_index(cs7, "colt_mp");
    let def = |i: u8| weapons[i as usize].as_ref().expect("weapon loads");
    let t0 = 10_000;
    let mut sim = ClientSim::spectator(origin, yaw, [0; 3]);
    sim.become_player(origin, yaw, [0; 3]);
    // The stock loadout: primary in slot 1, pistol in slot 3 (the
    // `weaponSlot` table, `crates/server/src/weapons.rs`).
    pm::weapon::give(&mut sim.ps, carbine, 1);
    pm::weapon::give(&mut sim.ps, colt, 3);
    sim.ps.weapon = carbine;
    for (w, reserve) in [(carbine, 60), (colt, 56)] {
        let d = def(w);
        sim.ps.ammoclip[d.clip_index] = d.clip_size as i16;
        sim.ps.ammo[d.ammo_index] = reserve;
    }
    let first = UserCmd {
        server_time: t0,
        weapon: carbine,
        ..NULL_USERCMD
    };
    Run {
        p: P,
        pred: predict::from_wire(P, &sim.to_wire(P, 0, t0), Some(&first)),
        sim,
        world,
        bodies: Vec::new(),
        weapons,
        st: t0,
        last_cmd: first,
        view: [0; 2],
        cmds: 0,
        ring,
        rebuilds: 0,
        rebuilds_dropping: 0,
        rebuilds_mid_lerp: 0,
        prone_corrections: 0,
        unpredicted: 0,
        mismatches: Vec::new(),
        counts: BTreeMap::new(),
    }
}

/// A standing player 60 units ahead, handed to both sides: the run into it,
/// a strafe along it, a jump at it and a crouched push.
#[test]
fn server_and_predictor_agree_beside_a_body() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let weapons = vcod_common::weapon_table::from_configstring(&fs, retail_cs7());
    let (world, entities) = load_world(&fs, "mp_carentan");
    let mut run = start(&world, &entities, &weapons, true);
    run.hold(input(|_| {}), 40, 0.0);
    let yaw = run.sim.view_angles()[1].to_radians();
    let at = run.sim.ps.origin + Vec3::new(yaw.cos(), yaw.sin(), 0.0) * 60.0;
    run.bodies = vec![Body::from_solid(1, at, 6684943, CONTENTS_BODY)];

    let mut closest = f32::MAX;
    let mut push = |run: &mut Run, template: UserCmd, n: usize| {
        for _ in 0..n {
            run.send(template, 8);
            closest = closest.min((run.sim.ps.origin - at).truncate().length());
        }
    };
    push(&mut run, input(|c| c.forward = 127), 120);
    push(
        &mut run,
        input(|c| {
            c.forward = 127;
            c.right = 127
        }),
        30,
    );
    push(&mut run, input(|c| c.forward = -127), 60);
    push(
        &mut run,
        input(|c| {
            c.forward = 127;
            c.up = 127
        }),
        3,
    );
    push(&mut run, input(|c| c.forward = 127), 90);
    push(
        &mut run,
        input(|c| {
            c.forward = 127;
            c.wbuttons = WBUTTON_CROUCH
        }),
        60,
    );

    println!(
        "beside a body: {} cmds, closest {closest:.3}, {} mismatching reads {:?}",
        run.cmds,
        run.mismatches.len(),
        run.counts
    );
    for m in run.mismatches.iter().take(40) {
        println!("  {m}");
    }
    assert!(
        (30.0..31.0).contains(&closest),
        "the body did not stop the run: closest {closest}"
    );
    assert!(
        run.mismatches.is_empty(),
        "the predictor diverged from the server's step: {:?}",
        run.counts
    );
}

/// The script both tests run: every movement mode, a hitch past the chop, a
/// prone turn past the cone, a weapon switch, taps, a reload and the sight.
fn step_side_by_side(ring: bool) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let cs7 = retail_cs7();
    let weapons = vcod_common::weapon_table::from_configstring(&fs, cs7);
    let (world, entities) = load_world(&fs, "mp_carentan");
    let colt = weapon_index(cs7, "colt_mp");
    let def = |i: u8| weapons[i as usize].as_ref().expect("weapon loads");
    let mut run = start(&world, &entities, &weapons, ring);

    // Land, then run forward with a slow turn.
    run.hold(input(|_| {}), 40, 0.0);
    run.hold(input(|c| c.forward = 127), 250, 0.2);
    run.hold(input(|c| c.right = 127), 60, 0.0);
    run.hold(input(|c| c.right = -127), 30, 0.0);
    // A jump off a run, and the landing.
    run.hold(
        input(|c| {
            c.forward = 127;
            c.up = 127
        }),
        3,
        0.0,
    );
    run.hold(input(|c| c.forward = 127), 90, 0.0);
    // A crouched walk, and a hitch past the 66 ms chop in the middle of it.
    run.hold(
        input(|c| {
            c.forward = 127;
            c.wbuttons = WBUTTON_CROUCH
        }),
        40,
        0.0,
    );
    run.send(
        input(|c| {
            c.forward = 127;
            c.wbuttons = WBUTTON_CROUCH
        }),
        150,
    );
    run.hold(input(|c| c.wbuttons = WBUTTON_CROUCH), 30, 0.0);
    // Prone, then a crawl turned far past the prone cone so the correction
    // pushes `delta_angles`, and back the other way.
    let prone = input(|c| c.wbuttons = WBUTTON_PRONE);
    run.hold(prone, 130, 0.0);
    run.hold(
        input(|c| {
            c.forward = 127;
            c.wbuttons = WBUTTON_PRONE
        }),
        60,
        2.0,
    );
    run.hold(prone, 40, -3.0);
    // Stand up from prone.
    run.hold(input(|_| {}), 110, 0.0);
    // The switch to the pistol, the byte held until the playerstate has it.
    let to_colt = input(|c| c.weapon = colt);
    for _ in 0..200 {
        if run.sim.ps.weapon == colt {
            break;
        }
        run.send(to_colt, 8);
    }
    assert_eq!(run.sim.ps.weapon, colt, "the switch never landed");
    run.until_ready(input(|_| {}), 200);
    // Three taps.
    let colt_clip = def(colt).clip_index;
    let full = run.sim.ps.ammoclip[colt_clip];
    for _ in 0..3 {
        run.send(input(|c| c.buttons = BUTTON_ATTACK), 8);
        run.hold(input(|_| {}), 5, 0.0);
        run.until_ready(input(|_| {}), 200);
    }
    assert_eq!(run.sim.ps.ammoclip[colt_clip], full - 3, "three shots");
    // A reload, the key held a few cmds.
    run.hold(input(|c| c.wbuttons = WBUTTON_RELOAD), 3, 0.0);
    run.until_ready(input(|_| {}), 600);
    assert_eq!(run.sim.ps.ammoclip[colt_clip], full, "the reload");
    // The sight: raised, a shot through it, a walk under it, lowered.
    let ads = input(|c| c.buttons = BUTTON_ADS);
    run.hold(ads, 60, 0.0);
    run.send(input(|c| c.buttons = BUTTON_ADS | BUTTON_ATTACK), 8);
    run.until_ready(ads, 200);
    assert_eq!(run.sim.ps.weapon_pos_frac, 1.0, "the sight is up");
    run.hold(
        input(|c| {
            c.buttons = BUTTON_ADS;
            c.forward = 127
        }),
        60,
        0.0,
    );
    run.hold(input(|_| {}), 60, 0.0);
    // Mounted on a stand gun: the step holds the player still whatever it
    // asks for, and the predictor has to read that off eFlags.
    let view = run.sim.view_angles();
    vcod_server::game::turret::mount_sim(
        &mut run.sim,
        vcod_server::game::turret::AimRule::V1,
        298,
        vcod_server::game::turret::TurretStance::Stand,
        view,
    );
    // The client learns of the mount from the next snapshot, not from a cmd.
    let w = over_the_wire(P, &run.sim.to_wire(P, 0, run.st));
    run.pred = predict::from_wire(P, &w, Some(&run.last_cmd));
    run.hold(
        input(|c| {
            c.forward = 127;
            c.right = 127;
            c.wbuttons = WBUTTON_CROUCH
        }),
        40,
        0.5,
    );

    println!(
        "predictor vs server{}: {} cmds, {} rebuilds ({} mid-putaway, {} mid eye lerp), \
         {} prone corrections, {} mismatching reads {:?}",
        if ring { " with the ring" } else { "" },
        run.cmds,
        run.rebuilds,
        run.rebuilds_dropping,
        run.rebuilds_mid_lerp,
        run.prone_corrections,
        run.mismatches.len(),
        run.counts
    );
    for m in run.mismatches.iter().take(40) {
        println!("  {m}");
    }
    assert!(run.rebuilds_dropping > 0, "no rebuild inside the putaway");
    assert!(run.rebuilds_mid_lerp > 0, "no rebuild inside an eye lerp");
    assert!(
        run.prone_corrections > 0,
        "the prone cone never pushed the view"
    );
    assert!(
        run.mismatches.is_empty(),
        "the predictor diverged from the server's step: {:?}",
        run.counts
    );
}

// ---------------------------------------------------------------------------
// The same side-by-side step under 1.5's and United Offensive's wires and
// rules, driven by retail's own cmd streams.

/// UO's slope captures and their maps (`moves_ab.rs`): the seats, the sprint
/// to exhaustion and the bipod, automatic fire, the mounts and the cook, and
/// a death, a spectator spell and a link (`follow_state`), and a mounted MG
/// (`follow_turret`).
const UO_RUNS: &[(&str, &str)] = &[
    ("mp_cassino-tdm-slope-8ms-tank-seats-uo.txt", "mp_cassino"),
    ("mp_cassino-tdm-slope-8ms-tank-drive-uo.txt", "mp_cassino"),
    ("mp_peaks-tdm-slope-8ms-jeep-seats-uo.txt", "mp_peaks"),
    ("mp_peaks-tdm-slope-8ms-jeep-drive-uo.txt", "mp_peaks"),
    ("mp_peaks-tdm-slope-8ms-jeep-gun-uo.txt", "mp_peaks"),
    ("mp_peaks-tdm-slope-8ms-jeep-rider-uo.txt", "mp_peaks"),
    ("mp_carentan-dm-slope-8ms-sprint-uo.txt", "mp_carentan"),
    ("mp_carentan-dm-slope-8ms-autofire-uo.txt", "mp_carentan"),
    ("mp_foy-dm-slope-8ms-uofoot-stand.txt", "mp_foy"),
    ("mp_foy-dm-slope-8ms-uofoot-crouch.txt", "mp_foy"),
    ("mp_foy-dm-slope-8ms-uofoot-mount.txt", "mp_foy"),
    ("mp_carentan-dm-slope-8ms-uostates-uo.txt", "mp_carentan"),
    ("mp_carentan-dm-slope-8ms-deadlink-uo.txt", "mp_carentan"),
    ("mp_carentan-dm-slope-8ms-uoturret-uo.txt", "mp_carentan"),
    ("mp_carentan-dm-slope-8ms-speedgrav-uo.txt", "mp_carentan"),
];

enum CaptureLine {
    Cmd(UserCmd),
    /// With `vang`, the seat's vehicle's angles, on a seated capture.
    Snap(Box<msg::PlayerState>, Option<[f32; 3]>),
}

/// A slope capture's cmds and decoded playerstates (`moves_ab.rs`' parser,
/// less what this gate does not read).
fn parse_capture(p: &Protocol, text: &str) -> Vec<CaptureLine> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((kind, rest)) = line.split_once(' ') else {
            continue;
        };
        let kv: BTreeMap<&str, &str> = rest
            .split_whitespace()
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let int = |k: &str| -> i32 { kv[k].parse().unwrap_or_else(|_| panic!("{k} in {line}")) };
        let pairs = |k: &str| -> Vec<(&str, i32)> {
            kv.get(k)
                .into_iter()
                .flat_map(|v| v.split(','))
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let (a, b) = s.rsplit_once(':').expect("name:value");
                    (a, b.parse().expect("an int"))
                })
                .collect()
        };
        match kind {
            "!cmd" => {
                let a: Vec<i32> = kv["angles"]
                    .split(',')
                    .map(|x| x.parse().unwrap())
                    .collect();
                out.push(CaptureLine::Cmd(UserCmd {
                    server_time: int("st"),
                    buttons: int("buttons") as u8,
                    wbuttons: int("wbuttons") as u8,
                    weapon: int("weapon") as u8,
                    angles: [a[0], a[1], a[2]],
                    forward: int("forward") as i8,
                    right: int("right") as i8,
                    up: int("up") as i8,
                    ..NULL_USERCMD
                }));
            }
            "!snap" => {
                let mut w = msg::PlayerState::null(p);
                for (name, v) in pairs("wire") {
                    let i = msg::PlayerState::field_index(p, name).expect(name);
                    w.fields[i] = v;
                }
                for (k, v) in pairs("ammo") {
                    w.arrays.ammo[k.parse::<usize>().unwrap()] = v as i16;
                }
                for (k, v) in pairs("clip") {
                    w.arrays.ammoclip[k.parse::<usize>().unwrap()] = v as i16;
                }
                let vang = kv.get("vang").map(|v| {
                    let a: Vec<f32> = v.split(',').map(|x| x.parse().unwrap()).collect();
                    [a[0], a[1], a[2]]
                });
                out.push(CaptureLine::Snap(Box::new(w), vang));
            }
            _ => {}
        }
    }
    out
}

/// Retail's seat at a seated capture's snapshot, put on the sim the way the
/// server's vehicle puts it there: the boarding and the exit are script and
/// the vehicle's step, which this harness has neither of, and in a seat the
/// end frame places the body at the tag (coduo-vehicles.md 3). So the sim
/// takes retail's origin and the vehicle's angles, and the cmds between
/// snapshots run seated on both sides. Whether the sim rides now.
fn follow_seat(
    sim: &mut ClientSim,
    p: &Protocol,
    w: &msg::PlayerState,
    vang: Option<[f32; 3]>,
) -> bool {
    use vcod_common::net::flags::{EF_HULL_VIEW_V22, EF_IN_VEHICLE_V22};
    let Some(vang) = vang else {
        return false;
    };
    let eflags = w.field_i32(p, "eFlags");
    let origin = w.origin(p);
    if eflags & EF_IN_VEHICLE_V22 == 0 {
        if sim.seat.is_some() {
            sim.leave_seat(None, None);
            sim.ps.origin = Vec3::from(origin);
            sim.ps.velocity = Vec3::ZERO;
        }
        return false;
    }
    let seat = vcod_server::spectate::Seat {
        vehicle: w.field_i32(p, "viewlocked_entNum") as u32,
        pos: w.field_i32(p, "vehPos"),
        kind: w.field_i32(p, "vehType"),
        motion: w.field_i32(p, "vehMotion"),
        angles: vang,
        hull_view: eflags & EF_HULL_VIEW_V22 != 0,
        overheated: false,
        // Not on the wire; only the entity pass reads it.
        link_offset: [0.0; 3],
    };
    match &mut sim.seat {
        Some(on) if on.pos == seat.pos && on.kind == seat.kind => on.angles = vang,
        _ => sim.enter_seat(seat, origin, w.viewangles(p)),
    }
    sim.ps.origin = Vec3::from(origin);
    true
}

/// Retail's mounted MG at a capture's snapshot, put on the sim the way the
/// gun's use and its turret pass put it there (`client-probes/probe_turret`,
/// a `--probe-uoturret` capture): the use tap and its release are the
/// game's, and the gunner's origin is the turret pass's `place_gunner`, so
/// the sim mounts and lets go where retail's playerstate does and takes
/// retail's origin while on the gun, as `follow_seat` does a seat's. The
/// capture mounts standing, and its release carries 1.1's
/// `EV_STANCE_FORCE_STAND` in the ring. Whether the snapshot's body was on a
/// gun.
fn follow_turret(sim: &mut ClientSim, p: &Protocol, w: &msg::PlayerState) -> bool {
    use vcod_common::net::flags::{
        EF_MOUNTED, EF_MOUNTED_DUCK, EF_MOUNTED_PRONE, EF_MOUNTED_STAND,
    };
    use vcod_server::game::turret::{self, AimRule, TurretStance};
    let origin = w.origin(p);
    let stance = match w.field_i32(p, "eFlags") & EF_MOUNTED {
        EF_MOUNTED_STAND => TurretStance::Stand,
        EF_MOUNTED_DUCK => TurretStance::Duck,
        EF_MOUNTED_PRONE => TurretStance::Prone,
        _ => {
            if sim.mounted_on.is_some() {
                turret::release_sim(sim, 0, origin, Some(pm::Stance::Stand));
            }
            return false;
        }
    };
    if sim.mounted_on.is_none() {
        let gun = w.field_i32(p, "viewlocked_entNum") as u32;
        turret::mount_sim(sim, AimRule::of(p), gun, stance, w.viewangles(p));
    }
    sim.ps.origin = Vec3::from(origin);
    true
}

/// Retail's `pm_type` at a capture's snapshot, put on the sim the way the
/// script put it there (`client-probes/probe_uostates`, `probe_states`): a
/// death, a `spawnPlayer`, `spawnSpectator` or `spawnIntermission` reseeded
/// from retail's playerstate,
/// a `linkTo` held at retail's origin, the mover being the script's. The
/// seat captures' links are `follow_seat`'s. Whether the snapshot's body
/// was dead, spectating or linked off a seat.
fn follow_state(run: &mut Run<'_>, w: &msg::PlayerState) -> bool {
    use vcod_common::net::flags::{
        EF_IN_VEHICLE_V22, PM_DEAD, PM_INTERMISSION, PM_NORMAL, PM_NORMAL_LINKED, PM_SPECTATOR,
    };
    let p = run.p;
    let pm = w.field_i32(p, "pm_type");
    let ours = run.sim.wire_pm_type();
    let origin = w.origin(p);
    let yaw = w.viewangles(p)[1];
    let angles = run.last_cmd.angles;
    let reseed = |run: &mut Run<'_>| {
        run.sim.ps = predict::from_wire(p, w, Some(&run.last_cmd)).ps;
    };
    match pm {
        PM_DEAD if ours != PM_DEAD => {
            run.sim.die();
            run.sim.end_frame(run.st);
        }
        PM_SPECTATOR if ours != PM_SPECTATOR => {
            run.sim.become_spectator(origin, yaw, angles);
            reseed(run);
        }
        PM_INTERMISSION if ours != PM_INTERMISSION => {
            run.sim.become_intermission(origin, yaw, angles, run.st);
            reseed(run);
        }
        PM_NORMAL if matches!(ours, PM_DEAD | PM_SPECTATOR) => {
            run.sim.become_player(origin, yaw, angles);
            reseed(run);
        }
        PM_NORMAL_LINKED if w.field_i32(p, "eFlags") & EF_IN_VEHICLE_V22 == 0 => {
            run.sim.link_to.get_or_insert(vcod_server::spectate::Link {
                parent: vcod_gsc::EntId(1, 0),
                offset: [0.0; 3],
                velocity: [0.0; 3],
            });
            run.sim.ps.origin = Vec3::from(origin);
        }
        PM_NORMAL if run.sim.link_to.is_some() && run.sim.seat.is_none() => {
            run.sim.link_to = None;
        }
        _ => {}
    }
    pm == PM_DEAD
        || pm == PM_SPECTATOR
        || pm == PM_INTERMISSION
        || (pm == PM_NORMAL_LINKED && w.field_i32(p, "eFlags") & EF_IN_VEHICLE_V22 == 0)
}

/// One capture's cmds run on a server sim speaking `p` and on a predictor
/// rebuilt from that sim's playerstate, mapped and round-tripped through the
/// wire codec as a snapshot is, at every snapshot the capture holds and every
/// fifth cmd between. The sim starts from retail's first snapshot and takes
/// retail's `serverCursorHint` at each one, so a bipod deploys where retail's
/// did. A field `to_wire` leaves out, or `from_wire` misreads, shows on the
/// cmds after a rebuild (`iFatigueSoundTime` was one, coduo-movement.md 15).
/// `Err` names the fields that differed.
fn capture_side_by_side(
    p: &'static Protocol,
    fs: &vcod_common::pk3::Pk3Fs,
    file: &str,
    map: &str,
) -> Result<(), String> {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/playerstate/{file}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    // The 1.5 captures predate the header; 1.5b sends 1.1's list.
    let cs7 = text
        .lines()
        .find_map(|l| l.strip_prefix("# cs7 "))
        .unwrap_or(retail_cs7());
    let weapons = vcod_common::weapon_table::from_configstring(fs, cs7);
    let (world, entities) = load_world(fs, map);
    world.unlink_script_brushes(&entities, "dm");
    world.set_arith(vcod_common::patch::Arith::for_protocol(p));
    let lines = parse_capture(p, &text);
    let first = lines
        .iter()
        .find_map(|l| match l {
            CaptureLine::Snap(w, _) => Some(&**w),
            CaptureLine::Cmd(_) => None,
        })
        .expect("a snapshot");
    // The cmd that ran to it, as `moves_ab.rs`' `base_cmd` builds it.
    let view = first.viewangles(p);
    let cmd_angles = [0, 1, 2].map(|i| {
        let da = first.field_i32(p, &format!("delta_angles[{i}]"));
        ((view[i] * ANGLE2SHORT).round() as i32 - da) & 0xffff
    });
    let last_cmd = UserCmd {
        server_time: first.field_i32(p, "commandTime"),
        angles: cmd_angles,
        weapon: first.field_i32(p, "weapon") as u8,
        ..NULL_USERCMD
    };
    let seed = predict::from_wire(p, first, Some(&last_cmd));
    let mut sim = ClientSim::spectator(first.origin(p), view[1], cmd_angles);
    sim.ps.rules = pm::MoveRules::for_protocol(p);
    sim.become_player(first.origin(p), view[1], cmd_angles);
    sim.ps = seed.ps;
    // What the capture's server stored at each end frame: its `g_speed` or
    // `maxspeed`, and its `g_gravity`, off a playing first snapshot.
    if first.field_i32(p, "pm_type") == vcod_common::net::flags::PM_NORMAL {
        sim.max_speed = seed.ps.speed as i32;
        sim.gravity = seed.ps.gravity as i32;
    }
    let t0 = seed.command_time;
    let mut run = Run {
        p,
        pred: seed,
        sim,
        world: &world,
        bodies: Vec::new(),
        weapons: &weapons,
        st: t0,
        last_cmd,
        view: [0; 2],
        cmds: 0,
        ring: true,
        rebuilds: 0,
        rebuilds_dropping: 0,
        rebuilds_mid_lerp: 0,
        prone_corrections: 0,
        unpredicted: 0,
        mismatches: Vec::new(),
        counts: BTreeMap::new(),
    };
    run.rebuild();
    let (mut sprinted, mut deployed, mut cooked, mut seated, mut states, mut manned) =
        (0, 0, 0, 0, 0, 0);
    for line in &lines {
        match line {
            CaptureLine::Snap(w, vang) => {
                run.sim.set_cursor_hint(
                    w.field_i32(p, "serverCursorHint"),
                    w.field_i32(p, "serverCursorHintVal"),
                );
                seated += usize::from(follow_seat(&mut run.sim, p, w, *vang));
                states += usize::from(follow_state(&mut run, w));
                manned += usize::from(follow_turret(&mut run.sim, p, w));
                run.rebuild();
            }
            CaptureLine::Cmd(c) if c.server_time > run.st => {
                // The capture's angles are absolute; `send` takes the view.
                run.view = [c.angles[0], c.angles[1]];
                run.send(*c, c.server_time - run.st);
                sprinted += usize::from(run.sim.ps.sprinting);
                deployed += usize::from(run.sim.ps.deployed());
                cooked += usize::from(run.sim.ps.grenade_time_left_ms != 0);
            }
            CaptureLine::Cmd(_) => {}
        }
    }
    println!(
        "{file}: {} cmds, {} rebuilds, sprinting {sprinted}, deployed {deployed}, \
         fuse armed {cooked}, seated snapshots {seated}, dead, spectating or linked \
         snapshots {states}, mounted snapshots {manned}, {} unpredicted cmds, {} mismatching reads {:?}",
        run.cmds,
        run.rebuilds,
        run.unpredicted,
        run.mismatches.len(),
        run.counts
    );
    for m in run.mismatches.iter().take(20) {
        println!("  {m}");
    }
    assert!(run.cmds > 1000, "{file}: only {} cmds", run.cmds);
    if run.mismatches.is_empty() {
        Ok(())
    } else {
        Err(format!("{file}: {:?}", run.counts))
    }
}

/// United Offensive's sprint, bipod, automatic fire, mounts, cook and MG through
/// the protocol 22 round trip. Needs `COD_DIR` at a United Offensive install.
#[test]
fn predictor_matches_the_uo_server_step() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let failures: Vec<String> = UO_RUNS
        .iter()
        .filter_map(|&(file, map)| capture_side_by_side(&PROTOCOL_V22, &fs, file, map).err())
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// 1.1d's spectator spell, corpse and intermission (`probe_states`) and its
/// linked corpse (`probe_deadlink`) through the protocol 1 round trip. Needs `COD_DIR` at a 1.1 install.
#[test]
fn predictor_matches_the_1_1_server_step() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let failures: Vec<String> = [
        "mp_carentan-dm-slope-8ms-states-1.1.txt",
        "mp_carentan-dm-slope-8ms-deadlink-1.1.txt",
    ]
    .iter()
    .filter_map(|file| capture_side_by_side(&PROTOCOL_V1, &fs, file, "mp_carentan").err())
    .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// 1.5's jump penalty, stepped eye and prone turns through the protocol 6
/// round trip. Needs `COD_DIR` at a 1.5 install, or a UO one (its `main/` is
/// 1.5's).
#[test]
fn predictor_matches_the_1_5_server_step() {
    let Some(fs) = fs_15() else {
        return;
    };
    let failures: Vec<String> = [
        "mp_carentan-dm-slope-8ms-moves-1.5.txt",
        "mp_carentan-dm-slope-8ms-turns-1.5.txt",
        "mp_carentan-dm-slope-8ms-speedgrav-1.5.txt",
        "mp_carentan-dm-slope-8ms-states-1.5.txt",
        "mp_carentan-dm-slope-8ms-deadlink-1.5.txt",
    ]
    .iter()
    .filter_map(|file| capture_side_by_side(&PROTOCOL_V6, &fs, file, "mp_carentan").err())
    .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// One state's snapshots of a retail capture through the predictor: each
/// snapshot whose `pm_type` is `pm_type`, followed by another in it, rebuilt
/// through `from_wire`, the capture's cmds up to the next one's
/// `commandTime` run as a predicting client runs them, and the origin,
/// velocity and eye compared with retail's, bit for bit. Retail's cgame
/// predicts every own `pm_type` (cod11-player-clip.md 1.1). Panics on a
/// difference, and when fewer than `min_compared` pairs, or `min_moved`
/// that moved, were compared.
fn retail_state_rows(
    p: &Protocol,
    fs: &vcod_common::pk3::Pk3Fs,
    file: &str,
    pm_type: i32,
    min_compared: usize,
    min_moved: usize,
) {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/playerstate/{file}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let cs7 = text
        .lines()
        .find_map(|l| l.strip_prefix("# cs7 "))
        .unwrap_or(retail_cs7());
    let weapons = vcod_common::weapon_table::from_configstring(fs, cs7);
    let (world, entities) = load_world(fs, "mp_carentan");
    world.unlink_script_brushes(&entities, "dm");
    world.set_arith(vcod_common::patch::Arith::for_protocol(p));
    let mw = MoveWorld::bare(&world);
    let lines = parse_capture(p, &text);
    let cmds: Vec<UserCmd> = lines
        .iter()
        .filter_map(|l| match l {
            CaptureLine::Cmd(c) => Some(*c),
            CaptureLine::Snap(..) => None,
        })
        .collect();
    // Each snapshot with the cmds sent after it arrived, up to the next.
    let mut snaps: Vec<(&msg::PlayerState, Vec<UserCmd>)> = Vec::new();
    for l in &lines {
        match l {
            CaptureLine::Snap(w, _) => snaps.push((&**w, Vec::new())),
            CaptureLine::Cmd(c) => {
                if let Some((_, after)) = snaps.last_mut() {
                    after.push(*c);
                }
            }
        }
    }
    let ct = |w: &msg::PlayerState| w.field_i32(p, "commandTime");
    // The intermission's think runs no cmd, so its `commandTime` holds.
    let frozen = pm_type == vcod_common::net::flags::PM_INTERMISSION;
    let (mut compared, mut moved) = (0, 0);
    let mut bad = Vec::new();
    for pair in snaps.windows(2) {
        let ((s, after), (n, _)) = (&pair[0], &pair[1]);
        let (s, n) = (*s, *n);
        if s.field_i32(p, "pm_type") != pm_type
            || n.field_i32(p, "pm_type") != pm_type
            || (ct(n) <= ct(s) && !frozen)
        {
            continue;
        }
        // Of two cmds stamped alike the server runs the first.
        let last = cmds.iter().find(|c| c.server_time == ct(s));
        let mut pred = predict::from_wire(p, s, last);
        if frozen {
            for c in after {
                predict::run_cmd(&mut pred, c, &mw, &weapons);
            }
        }
        let mut st = ct(s);
        for c in cmds.iter().filter(|_| !frozen) {
            if c.server_time > st && c.server_time <= ct(n) {
                predict::run_cmd(&mut pred, c, &mw, &weapons);
                st = c.server_time;
            }
        }
        let bits = |v: [f32; 3]| v.map(f32::to_bits);
        let vel =
            |w: &msg::PlayerState| [0, 1, 2].map(|i| w.field_f32(p, &format!("velocity[{i}]")));
        let eye = |w: &msg::PlayerState| w.field_f32(p, "viewHeightCurrent");
        compared += 1;
        moved += usize::from(n.origin(p) != s.origin(p));
        // `PmoveSingle`'s head runs for a corpse too: the cone on the held
        // weapon and the backpedal flag (cod11-player-clip.md 1.1).
        let dead = matches!(
            pm_type,
            vcod_common::net::flags::PM_DEAD | vcod_common::net::flags::PM_DEAD_LINKED
        );
        let head_differs = dead
            && (pred.ps.aim_spread_scale.to_bits() != n.field_f32(p, "aimSpreadScale").to_bits()
                || pred.ps.backwards_run
                    != (n.field_i32(p, "pm_flags") & vcod_common::net::flags::PMF_BACKWARDS_RUN
                        != 0));
        if bits(pred.ps.origin.to_array()) != bits(n.origin(p))
            || bits(pred.ps.velocity.to_array()) != bits(vel(n))
            || pred.ps.view_height().to_bits() != eye(n).to_bits()
            || head_differs
        {
            bad.push(format!(
                "ct {}: predicted {} {} eye {} cone {} back {} retail {:?} {:?} eye {} cone {} pm_flags {:#x}",
                ct(n),
                pred.ps.origin,
                pred.ps.velocity,
                pred.ps.view_height(),
                pred.ps.aim_spread_scale,
                pred.ps.backwards_run,
                n.origin(p),
                vel(n),
                eye(n),
                n.field_f32(p, "aimSpreadScale"),
                n.field_i32(p, "pm_flags")
            ));
        }
    }
    println!(
        "{file} pm_type {pm_type}: {compared} snapshots, {moved} moved, {} differ",
        bad.len()
    );
    assert!(compared >= min_compared, "only {compared} snapshots");
    assert!(moved >= min_moved, "only {moved} moved");
    assert!(
        bad.is_empty(),
        "{} of {compared} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// The spectator spell of retail UO's `probe_uostates` capture: 1.5's and
/// UO's stored fly move (cod15-movement.md 5, "The fly move"). Needs
/// `COD_DIR` at a United Offensive install.
#[test]
fn predictor_flies_retail_spectator() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V22,
        &fs,
        "mp_carentan-dm-slope-8ms-uostates-uo.txt",
        vcod_common::net::flags::PM_SPECTATOR,
        60,
        30,
    );
}

/// The 22 s spectator spell of retail 1.1d's `probe_states` capture: 1.1's
/// unstored fly move, its lean unconditional, flown into roofs and walls and
/// dived onto the street. Needs `COD_DIR` at a 1.1 install.
#[test]
fn predictor_flies_retail_spectator_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V1,
        &fs,
        "mp_carentan-dm-slope-8ms-states-1.1.txt",
        vcod_common::net::flags::PM_SPECTATOR,
        400,
        200,
    );
}

/// The corpse of retail 1.1d's `probe_states` capture, killed running: its
/// slide, the drop the ladder check gives it every frame and the eye sinking
/// to `deadViewHeight`. Needs `COD_DIR` at a 1.1 install.
#[test]
fn predictor_drops_retail_corpse_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V1,
        &fs,
        "mp_carentan-dm-slope-8ms-states-1.1.txt",
        vcod_common::net::flags::PM_DEAD,
        70,
        30,
    );
}

/// The linked corpse of retail 1.1d's `probe_deadlink` capture, killed
/// linked in a diagonal run: `pm_type` 7 takes the linked arm, so the
/// velocity holds, `groundEntityNum` reads 1023 and only the eye sinks.
/// Needs `COD_DIR` at a 1.1 install.
#[test]
fn predictor_holds_retail_linked_corpse_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V1,
        &fs,
        "mp_carentan-dm-slope-8ms-deadlink-1.1.txt",
        vcod_common::net::flags::PM_DEAD_LINKED,
        400,
        0,
    );
}

/// The linked corpse of retail 1.5b's `probe_deadlink` capture: 1.5's
/// dispatch sends `pm_type` 7 to the linked arm too. Needs `COD_DIR` at a
/// 1.5 install, or a UO one (its `main/` is 1.5's).
#[test]
fn predictor_holds_retail_linked_corpse_1_5() {
    let Some(fs) = fs_15() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V6,
        &fs,
        "mp_carentan-dm-slope-8ms-deadlink-1.5.txt",
        vcod_common::net::flags::PM_DEAD_LINKED,
        400,
        0,
    );
}

/// The linked corpse of retail UO's `probe_deadlink` capture. Needs
/// `COD_DIR` at a United Offensive install.
#[test]
fn predictor_holds_retail_linked_corpse_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V22,
        &fs,
        "mp_carentan-dm-slope-8ms-deadlink-uo.txt",
        vcod_common::net::flags::PM_DEAD_LINKED,
        400,
        0,
    );
}

/// The corpse of retail UO's `probe_uostates` capture. Needs `COD_DIR` at a
/// United Offensive install.
#[test]
fn predictor_drops_retail_corpse_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V22,
        &fs,
        "mp_carentan-dm-slope-8ms-uostates-uo.txt",
        vcod_common::net::flags::PM_DEAD,
        40,
        0,
    );
}

/// The intermission of retail 1.1d's `probe_states` capture: its cmds move
/// nothing. Needs `COD_DIR` at a 1.1 install.
#[test]
fn predictor_holds_retail_intermission_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V1,
        &fs,
        "mp_carentan-dm-slope-8ms-states-1.1.txt",
        vcod_common::net::flags::PM_INTERMISSION,
        600,
        0,
    );
}

/// A 1.5 install's paks, or a UO one's `main/` (1.5's).
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    vcod_common::testing::game_fs().filter(|fs| {
        vcod_common::net::protocol::install_protocol(fs)
            .transport
            .version
            == 6
    })
}

/// The 22 s spectator spell of retail 1.5b's `probe_states` capture: 1.5's
/// stored fly move (cod15-movement.md 5, "The fly move"). Needs `COD_DIR` at
/// a 1.5 install, or a UO one.
#[test]
fn predictor_flies_retail_spectator_1_5() {
    let Some(fs) = fs_15() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V6,
        &fs,
        "mp_carentan-dm-slope-8ms-states-1.5.txt",
        vcod_common::net::flags::PM_SPECTATOR,
        400,
        200,
    );
}

/// The corpse of retail 1.5b's `probe_states` capture, killed 1475 ms into
/// a stance change's `pm_time`: the ladder check skipped only on a knockback
/// or stun timer, so it drops anyway (cod11-player-clip.md 1.1). Needs
/// `COD_DIR` at a 1.5 install, or a UO one.
#[test]
fn predictor_drops_retail_corpse_1_5() {
    let Some(fs) = fs_15() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V6,
        &fs,
        "mp_carentan-dm-slope-8ms-states-1.5.txt",
        vcod_common::net::flags::PM_DEAD,
        70,
        30,
    );
}

/// The intermission of retail 1.5b's `probe_states` capture. Needs `COD_DIR`
/// at a 1.5 install, or a UO one.
#[test]
fn predictor_holds_retail_intermission_1_5() {
    let Some(fs) = fs_15() else {
        return;
    };
    retail_state_rows(
        &PROTOCOL_V6,
        &fs,
        "mp_carentan-dm-slope-8ms-states-1.5.txt",
        vcod_common::net::flags::PM_INTERMISSION,
        600,
        0,
    );
}

// ---------------------------------------------------------------------------
// Retail's slope runs through the predictor, and the playerstate fields a
// `RIDE` line logs (`wire_from`).

#[derive(Clone, Debug)]
struct Snap {
    ct: i32,
    origin: Vec3,
    velocity: Vec3,
    ground: i32,
    /// Wire order: pitch, yaw, roll, in degrees.
    view: [f32; 3],
    delta_angles: [i32; 3],
    frac: f32,
    pm_flags: i32,
    /// The wire's `ps.speed`, `g_speed` on these captures.
    speed: i32,
    /// The wire's `ps.gravity`, `g_gravity`.
    gravity: i32,
}

fn parse_vec3(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().expect("a float")).collect();
    Vec3::new(v[0], v[1], v[2])
}

/// Retail's snapshot as a wire playerstate: the fields the fixture carries,
/// a standing eye, and the weapon the cmds hold.
fn wire_from(snap: &Snap, weapon: u8) -> msg::PlayerState {
    let mut w = msg::PlayerState::null(P);
    let mut set = |name: &str, v: i32| {
        w.fields[msg::PlayerState::field_index(P, name).expect(name)] = v;
    };
    set("commandTime", snap.ct);
    for i in 0..3 {
        set(&format!("origin[{i}]"), snap.origin[i].to_bits() as i32);
        set(&format!("velocity[{i}]"), snap.velocity[i].to_bits() as i32);
        set(&format!("viewangles[{i}]"), snap.view[i].to_bits() as i32);
        set(&format!("delta_angles[{i}]"), snap.delta_angles[i]);
    }
    set("groundEntityNum", snap.ground);
    set("pm_flags", snap.pm_flags);
    set("speed", snap.speed);
    set("gravity", snap.gravity);
    set("fWeaponPosFrac", snap.frac.to_bits() as i32);
    set("viewHeightCurrent", pm::VIEW_STAND.to_bits() as i32);
    set("weapon", i32::from(weapon));
    let held = 1u64 << weapon;
    set("weapons[0]", held as u32 as i32);
    set("weapons[1]", (held >> 32) as u32 as i32);
    w
}

/// A snapshot's predicted origin against retail's.
struct Delta {
    ct: i32,
    d: Vec3,
    ground_disagrees: bool,
    /// Moved by the server between snapshots, which no prediction follows:
    /// a spawn's or teleport's `eFlags` 0x8 flip, the probe gsc's
    /// `setOrigin` (its `PROBE place <time>` line at the snapshot's time),
    /// or a body that moved while carried ([`carried`]) at either end.
    relocated: bool,
}

/// A body whose origin the server sets each frame and whose pmove moves
/// nothing: linked (`pm_type` 1 or 7, the parent's carry), in a UO vehicle
/// seat (placed at the seat tag, coduo-vehicles.md 3) or on a mounted MG
/// (placed by the turret pass, cod11-turrets.md).
fn carried(p: &Protocol, w: &msg::PlayerState) -> bool {
    use vcod_common::net::flags::{
        EF_IN_VEHICLE_V22, EF_MOUNTED, PM_DEAD_LINKED, PM_NORMAL_LINKED,
    };
    let pm = w.field_i32(p, "pm_type");
    let ef = w.field_i32(p, "eFlags");
    pm == PM_NORMAL_LINKED
        || pm == PM_DEAD_LINKED
        || ef & EF_MOUNTED != 0
        || (p.transport.version == PROTOCOL_V22.transport.version && ef & EF_IN_VEHICLE_V22 != 0)
}

/// The serverTimes of the gsc's `PROBE place <time> (x, y, z)` lines.
fn placements(text: &str) -> Vec<i32> {
    text.lines()
        .filter_map(|l| {
            l.strip_prefix("PROBE place ")?
                .split(' ')
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

/// The serverTime (`t=`) of each `!snap` line, in order.
fn snap_times(text: &str) -> Vec<i32> {
    text.lines()
        .filter(|l| l.starts_with("!snap "))
        .map(|l| {
            let t = l.split_whitespace().find_map(|kv| kv.strip_prefix("t="));
            t.expect("a t= field").parse().unwrap()
        })
        .collect()
}

/// Each snapshot rebuilt through `from_wire` from the whole playerstate it
/// logs and the cmds up to the next snapshot's `commandTime` run through
/// `run_cmd`, as a predicting client does; one row per snapshot, the
/// predicted origin against retail's. `PREDICT_TRACE=<commandTime>` prints
/// that snapshot's prediction cmd by cmd.
fn replay(
    p: &Protocol,
    text: &str,
    world: &CollisionWorld,
    weapons: &[Option<WeaponDef>],
) -> Vec<Delta> {
    let lines = parse_capture(p, text);
    let cmds: Vec<UserCmd> = lines
        .iter()
        .filter_map(|l| match l {
            CaptureLine::Cmd(c) => Some(*c),
            CaptureLine::Snap(..) => None,
        })
        .collect();
    // Of two cmds stamped alike the server runs the first (`SV_UserMove`).
    let mut by_time: HashMap<i32, UserCmd> = HashMap::new();
    for c in &cmds {
        by_time.entry(c.server_time).or_insert(*c);
    }
    let places = placements(text);
    let mut snaps = lines
        .iter()
        .filter_map(|l| match l {
            CaptureLine::Snap(w, _) => Some(&**w),
            CaptureLine::Cmd(_) => None,
        })
        .zip(snap_times(text));
    let ct = |w: &msg::PlayerState| w.field_i32(p, "commandTime");
    let eflags = |w: &msg::PlayerState| w.field_i32(p, "eFlags");
    let (first, _) = snaps.next().expect("a snapshot before the first cmd");
    let rebuild = |w: &msg::PlayerState| predict::from_wire(p, w, by_time.get(&ct(w)));
    let mut pred = rebuild(first);
    let (mut last_ct, mut last_eflags) = (ct(first), eflags(first));
    let mut last = first;
    let mut next_cmd = 0usize;
    let mut rows = Vec::new();
    let mw = MoveWorld::bare(world);
    let trace = std::env::var("PREDICT_TRACE")
        .ok()
        .and_then(|v| v.parse::<i32>().ok());
    for (w, t) in snaps {
        let s_ct = ct(w);
        if s_ct <= last_ct {
            continue;
        }
        while next_cmd < cmds.len() && cmds[next_cmd].server_time <= s_ct {
            // The cmds the capture logged before its first snapshot ran
            // before it.
            if cmds[next_cmd].server_time > last_ct {
                predict::run_cmd(&mut pred, &cmds[next_cmd], &mw, weapons);
                if trace == Some(s_ct) {
                    let ps = &pred.ps;
                    println!(
                        "st {} origin {:?} z {:#x} vel {:?} ground {} maxs {:?}",
                        cmds[next_cmd].server_time,
                        ps.origin,
                        ps.origin.z.to_bits(),
                        ps.velocity,
                        ps.on_ground,
                        ps.maxs()
                    );
                }
            }
            next_cmd += 1;
        }
        rows.push(Delta {
            ct: s_ct,
            d: pred.ps.origin - Vec3::from(w.origin(p)),
            ground_disagrees: pred.ps.on_ground
                != (w.field_i32(p, "groundEntityNum") == ENTITYNUM_WORLD as i32),
            relocated: (eflags(w) ^ last_eflags) & 0x8 != 0
                || places.contains(&t)
                || ((carried(p, w) || carried(p, last)) && w.origin(p) != last.origin(p)),
        });
        pred = rebuild(w);
        (last_ct, last_eflags, last) = (s_ct, eflags(w), w);
    }
    rows
}

/// Every slope capture off one retail server (`server`, its header's name)
/// whose `!snap` lines log the whole playerstate (`wire=`), replayed under
/// `p` exactly: a snapshot off retail's origin by any amount, or on the other
/// side of the ground, fails unless the server moved the body
/// ([`Delta::relocated`]). Each capture's weapons are its own `# cs7`
/// header's, or mp_carentan's 1.1 list where it has none.
/// Rows the replay cannot match, `(file, commandTime)`, each with its reason.
/// A listed row that matches fails the gate, so the list cannot go stale.
const REPLAY_GAPS: &[(&str, i32)] = &[
    // The dead driver's corpse is pushed out of the jeep's hull by
    // `ClientThink_real` ahead of `Pmove` (`vehicle_overlap`, coduo-vehicles.md
    // 11.6, 11.7): the game's velocity write, which no predictor runs, at
    // the two snapshots after the death.
    ("mp_peaks-tdm-slope-8ms-jeep-die-uo.txt", 18743),
    ("mp_peaks-tdm-slope-8ms-jeep-die-uo.txt", 18793),
];

fn replay_retail_runs(p: &Protocol, fs: &vcod_common::pk3::Pk3Fs, server: &str, min_runs: usize) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/playerstate");
    let mut runs: Vec<(String, String)> = std::fs::read_dir(dir)
        .expect("the fixture directory")
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains("-slope-"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&p).unwrap())
        })
        .filter(|(_, text)| {
            text.lines()
                .next()
                .is_some_and(|l| l.contains(&format!("CoD {server} ")))
                && text.contains(" wire=")
        })
        .collect();
    runs.sort();
    assert!(
        runs.len() >= min_runs,
        "only {} {server} slope fixtures in {dir}",
        runs.len()
    );
    let mut worlds: HashMap<(String, String), CollisionWorld> = HashMap::new();
    let mut failures = Vec::new();
    for (name, text) in &runs {
        // `<map>-<gametype>-slope-...`; map names carry no hyphen.
        let mut parts = name.split('-');
        let (map, gametype) = (parts.next().unwrap(), parts.next().unwrap());
        let world = worlds
            .entry((map.to_owned(), gametype.to_owned()))
            .or_insert_with(|| {
                let (world, entities) = load_world(fs, map);
                world.unlink_script_brushes(&entities, gametype);
                world.set_arith(vcod_common::patch::Arith::for_protocol(p));
                world
            });
        let cs7 = text
            .lines()
            .find_map(|l| l.strip_prefix("# cs7 "))
            .unwrap_or(retail_cs7());
        let weapons = vcod_common::weapon_table::from_configstring(fs, cs7);
        let rows = replay(p, text, world, &weapons);
        assert!(rows.len() > 100, "{name}: only {} snapshots", rows.len());
        let relocated = rows.iter().filter(|r| r.relocated).count();
        let gap = |r: &Delta| REPLAY_GAPS.contains(&(name.as_str(), r.ct));
        let is_off = |r: &Delta| !r.relocated && (r.d != Vec3::ZERO || r.ground_disagrees);
        for r in rows.iter().filter(|r| gap(r) && !is_off(r)) {
            failures.push(format!(
                "{name}: ct {} matches; drop it from REPLAY_GAPS",
                r.ct
            ));
        }
        let off: Vec<_> = rows.iter().filter(|r| is_off(r) && !gap(r)).collect();
        println!(
            "{name}: {} snapshots, {relocated} moved by the server, {} off retail",
            rows.len(),
            off.len()
        );
        for r in off.iter().take(5) {
            failures.push(format!(
                "{name}: ct {} off by {:?}, ground disagrees {}",
                r.ct, r.d, r.ground_disagrees
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The retail 1.1d slope captures.
/// `mp_carentan-dm-slope-8ms-prone-{mound,street}.txt` predate the `wire=`
/// field (no eye leg, speed or gravity to rebuild from); their `-1.1`
/// recaptures stand in for them here. Needs `COD_DIR` at a 1.1 install.
#[test]
fn predictor_replays_retail_slope_runs() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    replay_retail_runs(&PROTOCOL_V1, &fs, "1.1d", 15);
}

/// The retail 1.5b slope captures under 1.5's rules. Needs `COD_DIR` at a
/// 1.5 install, or a UO one (its `main/` is 1.5's).
#[test]
fn predictor_replays_retail_slope_runs_1_5() {
    let Some(fs) = fs_15() else {
        return;
    };
    replay_retail_runs(&PROTOCOL_V6, &fs, "1.5b", 10);
}

/// The retail United Offensive 1.51b slope captures under UO's rules. Needs
/// `COD_DIR` at a United Offensive install.
#[test]
fn predictor_replays_retail_slope_runs_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    replay_retail_runs(&PROTOCOL_V22, &fs, "UO 1.51b", 30);
}
// ---------------------------------------------------------------------------
// Riding a retail mover.

const RIDE_SERVER: &str = "tests/fixtures/movers/mp_carentan-dm-ride.txt";
const RIDE_WIRE: &str = "tests/fixtures/movers/mp_carentan-dm-ride-wire.txt";
/// The slab's entity in the ride capture, and its inline model.
const SLAB: u32 = 177;
const SLAB_MODEL: i32 = 5;

/// Phases whose rider retail's own client cannot predict, with the reason.
/// The carry is the ground mover's `pos` alone, and the slab of the two yaw
/// phases turns in place about the world origin, so the prediction holds the
/// rider still while the server swings him round (movers doc, section 14):
/// 2 degrees a second at 2473 units is 4.3 units a snapshot.
const RIDE_GAPS: &[(&str, f32)] = &[("ride_yaw", 4.4), ("ride_yaw_back", 4.4)];

/// A `RIDE` line: the snapshot's time and the playerstate fields it logs.
struct Ride {
    t: i32,
    snap: Snap,
}

/// `RIDE` lines, the first of each serverTime, and entity `num`'s
/// trajectories as the change lines put them, `(serverTime, pos, apos)`.
fn parse_ride(text: &str, num: u32) -> (Vec<Ride>, Vec<(i32, Trajectory, Trajectory)>) {
    let mut rides: Vec<Ride> = Vec::new();
    let mut slab = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("RIDE ") {
            let kv: BTreeMap<&str, &str> = rest
                .split_whitespace()
                .filter_map(|kv| kv.split_once('='))
                .collect();
            let int = |k: &str| -> i32 {
                let v = kv[k];
                match v.strip_prefix("0x") {
                    Some(h) => i32::from_str_radix(h, 16).unwrap(),
                    None => v.parse().unwrap(),
                }
            };
            let t = int("t");
            if rides.last().is_some_and(|r| r.t == t) {
                continue;
            }
            rides.push(Ride {
                t,
                snap: Snap {
                    ct: int("ct"),
                    origin: parse_vec3(kv["origin"]),
                    velocity: parse_vec3(kv["vel"]),
                    ground: int("ground"),
                    view: [0.0; 3],
                    delta_angles: [0, int("delta_yaw"), 0],
                    frac: 0.0,
                    pm_flags: int("pm_flags"),
                    // The probe logs no speed or gravity; the capture's
                    // server ran the stock `g_speed` and `g_gravity`.
                    speed: 190,
                    gravity: 800,
                },
            });
        } else if let Some(rest) = line.strip_prefix(&format!("entity {num} serverTime ")) {
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            let v = |t: &str| parse_vec3(t.trim_matches(|c| c == '[' || c == ']'));
            let tr = |at: usize| Trajectory {
                tr_type: tokens[at + 1].parse().unwrap(),
                tr_time: tokens[at + 3].parse().unwrap(),
                tr_duration: tokens[at + 5].parse().unwrap(),
                base: v(tokens[at + 7]),
                delta: v(tokens[at + 9]),
            };
            // `<t> eType 8 pos trType .. delta [..] apos trType ..`
            slab.push((tokens[0].parse().unwrap(), tr(4), tr(15)));
        }
    }
    (rides, slab)
}

/// A brush model as a snapshot entity, entity `num` with inline model
/// `index`, with the trajectories it had at `t`.
fn brush_entity(
    num: u32,
    index: i32,
    changes: &[(i32, Trajectory, Trajectory)],
    t: i32,
) -> msg::EntityState {
    let (pos, apos) = changes
        .iter()
        .take_while(|c| c.0 <= t)
        .last()
        .map_or((Trajectory::default(), Trajectory::default()), |c| {
            (c.1, c.2)
        });
    let mut e = msg::EntityState::null(P);
    e.number = num;
    let mut set = |name: &str, v: i32| {
        e.fields[msg::EntityState::field_index(P, name).expect(name)] = v;
    };
    set("eType", 8);
    set("solid", 0xff_ffff);
    set("index", index);
    for (group, tr) in [("pos", pos), ("apos", apos)] {
        set(&format!("{group}.trType"), tr.tr_type);
        set(&format!("{group}.trTime"), tr.tr_time);
        set(&format!("{group}.trDuration"), tr.tr_duration);
        for i in 0..3 {
            set(&format!("{group}.trBase[{i}]"), tr.base[i].to_bits() as i32);
            set(
                &format!("{group}.trDelta[{i}]"),
                tr.delta[i].to_bits() as i32,
            );
        }
    }
    e
}

#[test]
fn predictor_rides_retail_movers() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let cs7 = retail_cs7();
    let weapons = vcod_common::weapon_table::from_configstring(&fs, cs7);
    let weapon = weapon_index(cs7, "m1carbine_mp");
    let (world, _) = load_world(&fs, "mp_carentan");
    let mw = MoveWorld::bare(&world);
    let (rides, slab) = parse_ride(&std::fs::read_to_string(RIDE_WIRE).unwrap(), SLAB);
    let starts: Vec<(String, i32)> = std::fs::read_to_string(RIDE_SERVER)
        .unwrap()
        .lines()
        .filter_map(|l| {
            let rest = &l[l.find("PROBE at ")? + 9..];
            let (phase, t) = rest.split_once(' ')?;
            Some((phase.to_owned(), t.trim().parse().ok()?))
        })
        .collect();
    let phase_at = |t: i32| {
        starts
            .iter()
            .rfind(|(_, s)| *s <= t)
            .map_or("", |(p, _)| p.as_str())
    };

    let mut bad = Vec::new();
    let mut compared = 0;
    // A jump past this between two snapshots is the probe's `setorigin`.
    let teleport = |a: &Ride, b: &Ride| (a.snap.origin - b.snap.origin).length() > 16.0;
    for w in rides.windows(3) {
        let (prev, s, n) = (&w[0], &w[1], &w[2]);
        if n.t != s.t + 50
            || s.snap.ground != SLAB as i32
            || n.snap.ct <= s.snap.ct
            || teleport(prev, s)
            || teleport(s, n)
        {
            continue;
        }
        let phase = phase_at(s.t);
        let ents = BTreeMap::from([(SLAB, brush_entity(SLAB, SLAB_MODEL, &slab, s.t))]);
        let movers = SnapshotMovers::from_entities(P, &ents);
        movers.place(&world, s.t);
        let mut pred = predict::from_wire(P, &wire_from(&s.snap, weapon), None);
        let mut st = s.snap.ct;
        while st < n.snap.ct {
            st = (st + 8).min(n.snap.ct);
            let cmd = UserCmd {
                server_time: st,
                weapon,
                ..NULL_USERCMD
            };
            predict::run_cmd(&mut pred, &cmd, &mw, &weapons);
        }
        let ground = pred.ps.ground_entity_num();
        let at = movers.carry(pred.ps.origin, ground as i32, s.t, n.t);
        let d = (at - n.snap.origin).abs().max_element();
        compared += 1;
        let tol = RIDE_GAPS
            .iter()
            .find(|g| g.0 == phase)
            .map_or(0.05, |g| g.1);
        if ground != SLAB || d > tol {
            bad.push(format!(
                "{phase} t={}: ground {ground}, predicted {at} retail {} ({d:.3})",
                n.t, n.snap.origin
            ));
        }
    }
    assert!(compared > 200, "only {compared} riding snapshots");
    assert!(
        bad.is_empty(),
        "{} of {compared} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

const WORLDCOPY_WIRE: &str = "tests/fixtures/movers/mp_carentan-dm-worldcopy-wire.txt";
/// The script-spawned `script_brushmodel` of the world-copy capture: index
/// 0, spawned stationary at (0, 0, -64) on the 11750 snapshot.
const COPY: u32 = 171;

/// Retail's player riding a moving world copy, predicted from each snapshot
/// to the next as `predictor_rides_retail_movers` does the slab's rider:
/// the copy clips as the whole world where its trajectory has it
/// (docs/research/cod11-movers.md 20).
#[test]
fn predictor_rides_a_retail_world_copy() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let cs7 = retail_cs7();
    let weapons = vcod_common::weapon_table::from_configstring(&fs, cs7);
    let weapon = weapon_index(cs7, "m1carbine_mp");
    let (world, _) = load_world(&fs, "mp_carentan");
    let mw = MoveWorld::bare(&world);
    let (rides, mut copy) = parse_ride(&std::fs::read_to_string(WORLDCOPY_WIRE).unwrap(), COPY);
    let spawned = Trajectory {
        base: glam::Vec3::new(0.0, 0.0, -64.0),
        ..Trajectory::default()
    };
    copy.insert(0, (11750, spawned, Trajectory::default()));
    let mut bad = Vec::new();
    let mut compared = 0;
    for w in rides.windows(2) {
        let (s, n) = (&w[0], &w[1]);
        // Both on the copy, and no push jitter between them: the server's
        // sideways shove off an obstacle is not the prediction's to make.
        let jitter = (n.snap.origin - s.snap.origin).truncate().length() > 3.0;
        if n.t != s.t + 50
            || s.snap.ground != COPY as i32
            || n.snap.ground != COPY as i32
            || n.snap.ct <= s.snap.ct
            || jitter
        {
            continue;
        }
        let ents = BTreeMap::from([(COPY, brush_entity(COPY, 0, &copy, s.t))]);
        let movers = SnapshotMovers::from_entities(P, &ents);
        movers.place(&world, s.t);
        let mut pred = predict::from_wire(P, &wire_from(&s.snap, weapon), None);
        let mut st = s.snap.ct;
        while st < n.snap.ct {
            st = (st + 8).min(n.snap.ct);
            let cmd = UserCmd {
                server_time: st,
                weapon,
                ..NULL_USERCMD
            };
            predict::run_cmd(&mut pred, &cmd, &mw, &weapons);
        }
        let ground = pred.ps.ground_entity_num();
        let at = movers.carry(pred.ps.origin, ground as i32, s.t, n.t);
        let d = (at - n.snap.origin).abs().max_element();
        compared += 1;
        if ground != COPY || d > 0.05 {
            bad.push(format!(
                "t={}: ground {ground}, predicted {at} retail {} ({d:.3})",
                n.t, n.snap.origin
            ));
        }
    }
    SnapshotMovers::default().place(&world, 0);
    assert!(compared > 80, "only {compared} riding snapshots");
    assert!(
        bad.is_empty(),
        "{} of {compared} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// Where retail UO's `probe_uostates` unlink left its player, 6.8 units
/// into mp_carentan's street terrain: the ground trace starts in solid
/// without being all solid (a terrain clip never reads all solid), and so
/// does its retrace from a thousandth under the origin, so the body stays
/// off the ground under every version's rules (cod11-mantle.md, "A start
/// in solid on the ground trace"). It used to stand there and walk off at
/// full speed. Needs `COD_DIR` at any install.
#[test]
fn a_body_sunk_in_terrain_finds_no_ground() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let (world, entities) = load_world(&fs, "mp_carentan");
    world.unlink_script_brushes(&entities, "dm");
    for p in [&PROTOCOL_V1, &PROTOCOL_V6, &PROTOCOL_V22] {
        world.set_arith(vcod_common::patch::Arith::for_protocol(p));
        let mw = MoveWorld::bare(&world);
        let mut ps = pm::PlayerState::spawn(Vec3::new(902.2071, 2019.3634, -43.596127), 90.0);
        ps.rules = pm::MoveRules::for_protocol(p);
        let input = pm::PmInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..6 {
            pm::pmove(&mut ps, &input, &mw, 0.008, &[]);
            assert!(
                !ps.on_ground,
                "protocol {}: {:?}",
                p.transport.version, ps.origin
            );
        }
    }
}
