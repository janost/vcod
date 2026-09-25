//! The client's predictor (`vcod_common::pmove::predict`) against the two
//! things it copies: the server's own per-cmd step, and retail's mover.
//!
//! `predictor_matches_the_server_step` runs a scripted cmd stream through
//! `ClientSim` the way `replay_moves` does and, every fifth cmd, rebuilds the
//! predictor from the playerstate the server would send, round-tripped
//! through the wire codec, then runs the next cmds on both and compares them
//! field for field. Same code on the same inputs, so everything is exact
//! except what the wire itself narrows. The event ring's half,
//! `predictor_ring_matches_the_server_step`, is ignored until `bobCycle`
//! travels.
//!
//! `predictor_replays_retail_slope_runs` is `playerstate_slope_ab.rs`'s
//! rebased replay with the predictor in place of the bare mover: retail's
//! snapshot rebuilt through `from_wire`, retail's cmds run through `run_cmd`.
//! Both need `COD_DIR`; without the paks they return early.

use glam::Vec3;
use std::collections::{BTreeMap, HashMap};
use vcod_common::collision::CollisionWorld;
use vcod_common::net::huffman::Huffman;
use vcod_common::net::msg::{
    self, UserCmd, BUTTON_ADS, BUTTON_ATTACK, NULL_USERCMD, WBUTTON_CROUCH, WBUTTON_PRONE,
    WBUTTON_RELOAD,
};
use vcod_common::net::protocol::{Protocol, ENTITYNUM_WORLD, PROTOCOL_V1};
use vcod_common::pmove::predict::{self, Predicted};
use vcod_common::pmove::{self as pm, weapon::WEAPON_READY};
use vcod_common::weapon::WeaponDef;
use vcod_server::spectate::ClientSim;

const P: &Protocol = &PROTOCOL_V1;
const ANGLE2SHORT: f32 = 65536.0 / 360.0;
/// `replay_moves`' arrears cap, `crates/server/src/server.rs`.
const MAX_PMOVE_ARREARS_MS: i32 = 1000;

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
/// entity string for the spawn and `unlink_gameobjects`.
fn load_world(fs: &vcod_common::pk3::Pk3Fs, map: &str) -> (CollisionWorld, String) {
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("parse");
    let world = CollisionWorld::build(&bsp, &vcod_common::props::collision_tris(fs, &bsp.entities));
    (world, bsp.entities.clone())
}

/// What a client decodes: the playerstate written as a delta from the null
/// state and read back, so every field arrives at its netfield width.
fn over_the_wire(w: &msg::PlayerState) -> msg::PlayerState {
    // Built once: the tree costs a debug build over 200 ms.
    static HUFF: std::sync::OnceLock<Huffman> = std::sync::OnceLock::new();
    let huff = HUFF.get_or_init(Huffman::new);
    let null = msg::PlayerState::null(P);
    let mut writer = msg::MsgWriter::new(huff);
    msg::write_delta_playerstate(&mut writer, P, &null, w);
    let data = writer.finish();
    let mut reader = msg::MsgReader::new(&data, huff);
    let out = msg::read_delta_playerstate(&mut reader, P, &null);
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
    sim: ClientSim,
    pred: Predicted,
    world: &'a CollisionWorld,
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
            let w = over_the_wire(&self.sim.to_wire(P, 0, self.st));
            self.pred = predict::from_wire(P, &w, Some(&self.last_cmd));
            self.rebuilds += 1;
            self.rebuilds_dropping +=
                usize::from(self.pred.ps.weaponstate == pm::weapon::WEAPON_DROPPING);
            self.rebuilds_mid_lerp += usize::from(self.pred.view_lerp_start != 0);
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
        predict::run_cmd(&mut self.pred, &cmd, self.world, self.weapons);
        self.compare();
        self.last_cmd = cmd;
        self.cmds += 1;
    }

    /// `replay_moves`' per-cmd body for one client, minus what reaches past
    /// the sim (attacks, touches, anims).
    fn server_step(&mut self, cmd: &UserCmd) {
        let dt_ms = cmd.server_time.wrapping_sub(self.st);
        if dt_ms <= 0 {
            return;
        }
        let mut base = self.st;
        if dt_ms > MAX_PMOVE_ARREARS_MS {
            base = cmd.server_time - MAX_PMOVE_ARREARS_MS;
        }
        self.sim.update_aim(dt_ms, cmd.server_time, self.weapons);
        while base != cmd.server_time {
            let msec = (cmd.server_time - base).min(pm::MAX_FRAME_MS as i32);
            base += msec;
            let step = UserCmd {
                server_time: base,
                ..*cmd
            };
            self.sim
                .step(&step, msec as f32 / 1000.0, Some(self.world), self.weapons);
        }
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
        if !self.ring {
            return self.record(bad);
        }
        // `eventSequence` and the slots are 8 bits on the wire.
        check(
            "event_sequence",
            (ring.seq & 0xff).to_string(),
            self.pred.event_sequence.to_string(),
        );
        check(
            "events",
            format!("{:?}", ring.events.map(|e| e & 0xff)),
            format!("{:?}", self.pred.events),
        );
        check(
            "event_parms",
            format!("{:?}", ring.parms.map(|e| e & 0xff)),
            format!("{:?}", self.pred.event_parms),
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
/// `from_wire` reads but `ClientSim::to_wire` does not send, so a predictor
/// rebuilt from our wire restarts the phase at 0 and lays its footsteps on
/// other cmds than the server does.
#[test]
#[ignore = "bobCycle is not on vcod-server's wire yet (ClientSim::to_wire); lands with the post-mounted-mg dedupe"]
fn predictor_ring_matches_the_server_step() {
    step_side_by_side(true);
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
    let (origin, yaw) = vcod_common::bsp::find_spawn(&entities).expect("a spawn");

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
    let mut run = Run {
        pred: predict::from_wire(P, &sim.to_wire(P, 0, t0), Some(&first)),
        sim,
        world: &world,
        weapons: &weapons,
        st: t0,
        last_cmd: first,
        view: [0; 2],
        cmds: 0,
        ring,
        rebuilds: 0,
        rebuilds_dropping: 0,
        rebuilds_mid_lerp: 0,
        prone_corrections: 0,
        mismatches: Vec::new(),
        counts: BTreeMap::new(),
    };

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
// Retail's slope runs through the predictor. Parser, gameobject rule and
// tolerances are `playerstate_slope_ab.rs`'s.

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
}

enum Line {
    Cmd(UserCmd),
    Snap(Snap),
}

fn parse_vec3(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().expect("a float")).collect();
    Vec3::new(v[0], v[1], v[2])
}

fn parse_fixture(text: &str) -> Vec<Line> {
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
        let ints = |k: &str| -> Vec<i32> { kv[k].split(',').map(|x| x.parse().unwrap()).collect() };
        match kind {
            "!cmd" => {
                let a = ints("angles");
                out.push(Line::Cmd(UserCmd {
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
                let view: Vec<f32> = kv["view"].split(',').map(|x| x.parse().unwrap()).collect();
                let da = ints("da");
                out.push(Line::Snap(Snap {
                    ct: int("ct"),
                    origin: parse_vec3(kv["origin"]),
                    velocity: parse_vec3(kv["vel"]),
                    ground: int("ground"),
                    view: [view[0], view[1], view[2]],
                    delta_angles: [da[0], da[1], da[2]],
                    frac: kv["frac"].parse().unwrap(),
                    pm_flags: int("pm_flags"),
                }));
            }
            _ => {}
        }
    }
    out
}

/// Copy of `playerstate_slope_ab.rs`'s: what the stock gametype script's
/// `_gameobjects::main` deletes before a client walks.
fn unlink_gameobjects(world: &CollisionWorld, entities: &str, gametype: &str) {
    let allowed: &[&str] = match gametype {
        "sd" => &["sd", "bombzone", "blocker"],
        g => &[g][..],
    };
    for block in vcod_common::bsp::entity_blocks(entities) {
        let Some(name) = block.get("script_gameobjectname") else {
            continue;
        };
        if allowed.contains(&name.as_str()) {
            continue;
        }
        if let Some(n) = block
            .get("model")
            .and_then(|m| m.strip_prefix('*'))
            .and_then(|n| n.parse::<usize>().ok())
        {
            world.set_model_linked(n, false);
        }
    }
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
    set("fWeaponPosFrac", snap.frac.to_bits() as i32);
    set("viewHeightCurrent", pm::VIEW_STAND.to_bits() as i32);
    set("weapon", i32::from(weapon));
    let held = 1u64 << weapon;
    set("weapons[0]", held as u32 as i32);
    set("weapons[1]", (held >> 32) as u32 as i32);
    w
}

#[derive(Clone, Copy, Default)]
struct Delta {
    dz: f32,
    dxy: f32,
    ground_disagrees: bool,
}

struct Stats {
    max_dz: f32,
    max_dxy: f32,
    p95_dz: f32,
    p95_dxy: f32,
    p99_dz: f32,
    p99_dxy: f32,
    outliers: usize,
    ground_disagreements: usize,
}

fn stats(rows: &[Delta]) -> Stats {
    let mut dz: Vec<f32> = rows.iter().map(|r| r.dz.abs()).collect();
    let mut dxy: Vec<f32> = rows.iter().map(|r| r.dxy).collect();
    dz.sort_by(f32::total_cmp);
    dxy.sort_by(f32::total_cmp);
    let at = |v: &[f32], q: f32| v.get((q * v.len() as f32) as usize).copied().unwrap_or(0.0);
    Stats {
        max_dz: dz.last().copied().unwrap_or(0.0),
        max_dxy: dxy.last().copied().unwrap_or(0.0),
        p95_dz: at(&dz, 0.95),
        p95_dxy: at(&dxy, 0.95),
        p99_dz: at(&dz, 0.99),
        p99_dxy: at(&dxy, 0.99),
        outliers: rows
            .iter()
            .filter(|r| r.dz.abs() > 1.0 || r.dxy > 1.0)
            .count(),
        ground_disagreements: rows.iter().filter(|r| r.ground_disagrees).count(),
    }
}

/// `playerstate_slope_ab.rs`'s rebased tolerances.
const P95_TOLERANCE_Z: f32 = 0.02;
const P95_TOLERANCE_XY: f32 = 0.05;
const P99_TOLERANCE_Z: f32 = 0.05;
const P99_TOLERANCE_XY: f32 = 0.6;
const MAX_TOLERANCE_Z: f32 = 0.5;
const OUTLIER_SHARE: f32 = 0.015;

/// Each snapshot rebuilt through `from_wire` and the cmds up to the next
/// snapshot's `commandTime` run through `run_cmd`, as a predicting client
/// does; one row per snapshot, the predicted origin against retail's.
fn replay(lines: &[Line], world: &CollisionWorld, weapons: &[Option<WeaponDef>]) -> Vec<Delta> {
    let cmds: Vec<UserCmd> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Cmd(c) => Some(*c),
            _ => None,
        })
        .collect();
    let weapon = cmds.first().map_or(0, |c| c.weapon);
    let by_time: HashMap<i32, UserCmd> = cmds.iter().map(|c| (c.server_time, *c)).collect();
    let mut snaps = lines.iter().filter_map(|l| match l {
        Line::Snap(s) => Some(s),
        _ => None,
    });
    let first = snaps.next().expect("a snapshot before the first cmd");
    let rebuild = |s: &Snap| predict::from_wire(P, &wire_from(s, weapon), by_time.get(&s.ct));
    let mut pred = rebuild(first);
    let mut last_ct = first.ct;
    let mut next_cmd = 0usize;
    let mut rows = Vec::new();
    for s in snaps {
        if s.ct <= last_ct {
            continue;
        }
        while next_cmd < cmds.len() && cmds[next_cmd].server_time <= s.ct {
            predict::run_cmd(&mut pred, &cmds[next_cmd], world, weapons);
            next_cmd += 1;
        }
        let d = pred.ps.origin - s.origin;
        rows.push(Delta {
            dz: d.z,
            dxy: d.truncate().length(),
            ground_disagrees: pred.ps.on_ground != (s.ground == ENTITYNUM_WORLD as i32),
        });
        pred = rebuild(s);
        last_ct = s.ct;
    }
    rows
}

#[test]
fn predictor_replays_retail_slope_runs() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let weapons = vcod_common::weapon_table::from_configstring(&fs, retail_cs7());
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/playerstate");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("the fixture directory")
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.contains("-slope-") && name.ends_with("ms.txt")
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no slope fixtures in {dir}");
    let mut worlds: HashMap<(String, String), CollisionWorld> = HashMap::new();
    let mut failures = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // `<map>-<gametype>-slope-<ms>ms.txt`; map names carry no hyphen.
        let mut parts = name.split('-');
        let (map, gametype) = (parts.next().unwrap(), parts.next().unwrap());
        let world = worlds
            .entry((map.to_owned(), gametype.to_owned()))
            .or_insert_with(|| {
                let (world, entities) = load_world(&fs, map);
                unlink_gameobjects(&world, &entities, gametype);
                world
            });
        let text = std::fs::read_to_string(path).unwrap();
        let rows = replay(&parse_fixture(&text), world, &weapons);
        assert!(rows.len() > 100, "{name}: only {} snapshots", rows.len());
        let s = stats(&rows);
        println!(
            "{name}: {} snapshots, p95 dz {:.3} dxy {:.3}, p99 dz {:.3} dxy {:.3}, \
             max dz {:.3} dxy {:.3}, {} past a unit, {} ground disagreements",
            rows.len(),
            s.p95_dz,
            s.p95_dxy,
            s.p99_dz,
            s.p99_dxy,
            s.max_dz,
            s.max_dxy,
            s.outliers,
            s.ground_disagreements,
        );
        if s.p95_dz > P95_TOLERANCE_Z || s.p95_dxy > P95_TOLERANCE_XY {
            failures.push(format!("{name}: p95 past the tolerance"));
        }
        if s.p99_dz > P99_TOLERANCE_Z || s.p99_dxy > P99_TOLERANCE_XY {
            failures.push(format!("{name}: p99 past the tolerance"));
        }
        if s.max_dz > MAX_TOLERANCE_Z {
            failures.push(format!("{name}: max dz past the tolerance"));
        }
        if s.outliers as f32 > OUTLIER_SHARE * rows.len() as f32 {
            failures.push(format!(
                "{name}: {} of {} snapshots past a unit",
                s.outliers,
                rows.len()
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
