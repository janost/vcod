//! Pins for the penetration loop, the melee and the damage playerstate on
//! mp_crash `war`, in process: two allies and an axis shooter placed by
//! `setviewpos` around the room whose north wall is a metal plate
//! (cod4-combat.md 3.1), rounds and swings fired straight through the
//! level, and what each victim lost and carries afterwards. Needs
//! `COD4_DIR`.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use glam::Vec3;

use super::wire_ab::Probe;
use super::*;
use crate::game::cod4::combat::Damage4;
use crate::game::cod4::level::Cod4Level;

/// Slots: two allies and the axis shooter.
const A: usize = 0;
const B: usize = 1;
const SHOOTER: usize = 2;

struct Trio {
    sv: Cod4Server,
    probes: Vec<Probe>,
    now: Instant,
    /// The yaw each slot's cmds hold once placed.
    yaws: [Option<f32>; 3],
}

impl Trio {
    /// mp_crash `war` under `cvars` with the three joined and spawned, past
    /// the prematch.
    fn new(cvars: &[(&str, &str)]) -> Option<Trio> {
        let install = vcod_cod4::testing::cod4_install()?;
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 8,
            },
            7,
        );
        sv.set_cvar("sv_pure", "0");
        sv.set_cvar("sv_cheats", "1");
        for (k, v) in cvars {
            sv.set_cvar(k, v);
        }
        sv.load(Some(&install)).unwrap();
        let class = "mr {sid} 15 assault_mp,0";
        let probes = vec![
            Probe::new(
                "a",
                0,
                28981,
                5_600,
                &[(1_000, "mr {sid} 8 allies"), (2_500, class)],
            ),
            Probe::new(
                "b",
                1,
                28982,
                5_700,
                &[(1_000, "mr {sid} 8 allies"), (2_500, class)],
            ),
            Probe::new(
                "shooter",
                2,
                28983,
                5_800,
                &[(1_000, "mr {sid} 8 axis"), (2_500, class)],
            ),
        ];
        let mut t = Trio {
            sv,
            probes,
            now: Instant::now(),
            yaws: [None; 3],
        };
        t.run_to(25_000);
        for s in [A, B, SHOOTER] {
            assert_eq!(
                t.level().pm_state(s).map(|p| p.pm_type),
                Some(0),
                "slot {s} plays"
            );
        }
        Some(t)
    }

    fn level(&mut self) -> &mut Cod4Level {
        self.sv.game.level.as_mut().unwrap()
    }

    fn run_to(&mut self, until: i32) {
        while self.sv.core.sv_time_ms < until {
            let svs = self.sv.core.sv_time_ms;
            for (slot, yaw) in self.yaws.into_iter().enumerate() {
                let (Some(yaw), Some(ps)) = (yaw, self.level().pm_state(slot)) else {
                    continue;
                };
                let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
                let c = UserCmd {
                    server_time: svs + 10,
                    weapon: ps.weapon as u8,
                    offhand: ps.off_hand_index as u8,
                    angles: [
                        0u16.wrapping_sub(short(ps.delta_angles[0])),
                        short(yaw).wrapping_sub(short(ps.delta_angles[1])),
                        0u16.wrapping_sub(short(ps.delta_angles[2])),
                    ],
                    ..Default::default()
                };
                let p = &mut self.probes[slot];
                p.replay.clear();
                p.replay.push_back((svs, vec![c]));
            }
            for p in self.probes.iter_mut() {
                if !p.joined && svs >= p.connect_at {
                    p.join(&mut self.sv, self.now);
                }
                if p.joined {
                    let pkt = p.packet(svs);
                    self.sv.handle_packet(p.addr, &pkt, self.now);
                }
            }
            self.now += Duration::from_millis(FRAME_MS as u64);
            self.sv.tick_frames(self.now, 1);
            let svs = self.sv.core.sv_time_ms;
            for (to, pkt) in self.sv.take_outgoing() {
                if let Some(p) = self.probes.iter_mut().find(|p| p.addr == to) {
                    p.receive(svs, &pkt);
                }
            }
        }
    }

    /// Each slot's eye to its spot (`setviewpos`) and its cmds' view to
    /// the yaw, then a second to settle.
    fn place(&mut self, spots: [([f32; 3], f32); 3]) {
        for (s, (o, yaw)) in spots.into_iter().enumerate() {
            self.level().set_view_pos(s, o, [0.0, yaw, 0.0]);
            self.yaws[s] = Some(yaw);
        }
        let t = self.sv.core.sv_time_ms + 1_000;
        self.run_to(t);
        for (s, (_, yaw)) in spots.into_iter().enumerate() {
            let view = self.level().pm_state(s).unwrap().view_angles[1];
            assert!(
                (view - yaw).abs() < 0.01,
                "slot {s} views {view}, not {yaw}"
            );
        }
    }

    fn heal(&mut self) {
        for s in [A, B, SHOOTER] {
            self.level().heal_for_test(s);
        }
    }

    /// What `act` took off each slot's health, one line.
    fn losses(&mut self, act: impl FnOnce(&mut Cod4Level) -> String) -> String {
        self.heal();
        let level = self.level();
        let before: Vec<i32> = (0..3).map(|s| level.rt.client_vitals(s).health).collect();
        let hits = level.combat_hits().len();
        let said = act(level);
        let lost: Vec<i32> = (0..3)
            .map(|s| before[s] - level.rt.client_vitals(s).health)
            .collect();
        let new_hits = &level.combat_hits()[hits..];
        let line = format!("lost {lost:?} hits {new_hits:?}{said}");
        let t = self.sv.core.sv_time_ms + 500;
        self.run_to(t);
        line
    }
}

/// The room's floor at 236, the yard north of the plate at about 232 and
/// 225 (eyes 60 over them).
const IN_ROOM: [f32; 3] = [207.0, 2250.0, 296.0];
const A_SPOT: [f32; 3] = [207.0, 2380.0, 292.0];
const B_SPOT: [f32; 3] = [207.0, 2500.0, 285.0];

/// Rounds north along x 207 through the plate, A's body, B's body and
/// the yard's far concrete wall, and the variants: perks, weapons,
/// penetration off, a leg begun past the plate.
#[test]
fn rounds_through_the_plate_and_two_bodies() {
    let Some(mut t) = Trio::new(&[("scr_player_maxhealth", "100")]) else {
        return;
    };
    t.place([(A_SPOT, -90.0), (B_SPOT, -90.0), (IN_ROOM, 90.0)]);
    let mut log = String::new();
    let north = |z: f32| (Vec3::new(207.0, 2250.0, z), Vec3::new(207.0, 3200.0, z));
    for (name, weapon, perks, (start, end)) in [
        ("m16 chest", "m16_mp", 0, north(270.0)),
        ("m16 head", "m16_mp", 0, north(286.0)),
        ("m16 deep", "m16_mp", 0x20, north(270.0)),
        ("m40a3", "m40a3_mp", 0, north(270.0)),
        ("m40a3 deep", "m40a3_mp", 0x20, north(270.0)),
        ("beretta", "beretta_mp", 0, north(270.0)),
        ("mp5", "mp5_mp", 0, north(270.0)),
        (
            "m16 past plate",
            "m16_mp",
            0,
            (
                Vec3::new(207.0, 2340.0, 270.0),
                Vec3::new(207.0, 3200.0, 270.0),
            ),
        ),
        (
            "m16 short",
            "m16_mp",
            0,
            (
                Vec3::new(207.0, 2250.0, 270.0),
                Vec3::new(207.0, 2390.0, 270.0),
            ),
        ),
        (
            "m16 into plate",
            "m16_mp",
            0,
            (
                Vec3::new(207.0, 2250.0, 270.0),
                Vec3::new(207.0, 2334.0, 270.0),
            ),
        ),
        (
            "m16 ceiling",
            "m16_mp",
            0,
            (
                Vec3::new(207.0, 2300.0, 296.0),
                Vec3::new(207.0, 2300.0, 900.0),
            ),
        ),
        (
            "m16 graze",
            "m16_mp",
            0,
            (
                Vec3::new(207.0, 2331.0, 296.0),
                Vec3::new(1207.0, 2431.0, 296.0),
            ),
        ),
        (
            "m16 south",
            "m16_mp",
            0,
            (
                Vec3::new(207.0, 1860.0, 296.0),
                Vec3::new(207.0, 1500.0, 296.0),
            ),
        ),
    ] {
        let line = t.losses(|l| {
            l.rt.host.cod4_mut().clients[SHOOTER].perks = perks;
            l.fire_round_for_test(SHOOTER, weapon, start, end);
            l.rt.host.cod4_mut().clients[SHOOTER].perks = 0;
            String::new()
        });
        writeln!(log, "{name}: {line}").unwrap();
    }
    t.level()
        .rt
        .host
        .cvars
        .set("bullet_penetrationEnabled", "0");
    for (name, (start, end)) in [
        ("off m16", north(270.0)),
        (
            "off m16 past plate",
            (
                Vec3::new(207.0, 2340.0, 270.0),
                Vec3::new(207.0, 3200.0, 270.0),
            ),
        ),
    ] {
        let line = t.losses(|l| {
            l.fire_round_for_test(SHOOTER, "m16_mp", start, end);
            String::new()
        });
        writeln!(log, "{name}: {line}").unwrap();
    }
    // A teammate in the way under no friendly fire stops an extended
    // round: B (allies) fires south through A.
    let line = t.losses(|l| {
        l.fire_round_for_test(
            B,
            "m16_mp",
            Vec3::new(207.0, 2490.0, 270.0),
            Vec3::new(207.0, 2340.0, 270.0),
        );
        String::new()
    });
    writeln!(log, "off team: {line}").unwrap();
    eprintln!("{log}");
    assert_eq!(log, ROUNDS);
}

const ROUNDS: &str = "\
m16 chest: lost [31, 17, 0] hits [(2, 0, 5), (2, 1, 5)]
m16 head: lost [31, 26, 0] hits [(2, 0, 3), (2, 1, 2)]
m16 deep: lost [35, 28, 0] hits [(2, 0, 5), (2, 1, 5)]
m40a3: lost [60, 35, 0] hits [(2, 0, 5), (2, 1, 5)]
m40a3 deep: lost [68, 56, 0] hits [(2, 0, 5), (2, 1, 5)]
beretta: lost [25, 4, 0] hits [(2, 0, 5), (2, 1, 5)]
mp5: lost [25, 4, 0] hits [(2, 0, 5), (2, 1, 5)]
m16 past plate: lost [40, 26, 0] hits [(2, 0, 5), (2, 1, 5)]
m16 short: lost [31, 0, 0] hits [(2, 0, 5)]
m16 into plate: lost [0, 0, 0] hits []
m16 ceiling: lost [0, 0, 0] hits []
m16 graze: lost [0, 0, 0] hits []
m16 south: lost [0, 0, 0] hits []
off m16: lost [0, 0, 0] hits []
off m16 past plate: lost [40, 20, 0] hits [(2, 0, 5), (2, 1, 5)]
off team: lost [0, 0, 0] hits [(1, 0, 5)]
";

/// Melee swings from the room: a body ahead, off to the side, behind, the
/// plate alone in reach, nothing in reach.
#[test]
fn melee_swings_hit_miss_and_pass_behind() {
    let Some(mut t) = Trio::new(&[]) else {
        return;
    };
    let mut log = String::new();
    let far = ([207.0, 2100.0, 296.0], 0.0);
    for (name, a, shooter) in [
        ("ahead", ([207.0, 2290.0, 296.0], -90.0), (IN_ROOM, 90.0)),
        ("side", ([222.0, 2300.0, 296.0], -90.0), (IN_ROOM, 90.0)),
        ("edge", ([229.0, 2305.0, 296.0], -90.0), (IN_ROOM, 90.0)),
        (
            "edge near",
            ([226.0, 2290.0, 296.0], -90.0),
            (IN_ROOM, 90.0),
        ),
        (
            "far right",
            ([233.0, 2300.0, 296.0], -90.0),
            (IN_ROOM, 90.0),
        ),
        (
            "far ahead",
            ([207.0, 2318.0, 296.0], -90.0),
            (IN_ROOM, 90.0),
        ),
        (
            "reach 2322",
            ([207.0, 2322.0, 296.0], -90.0),
            (IN_ROOM, 90.0),
        ),
        (
            "reach 2326",
            ([207.0, 2326.0, 296.0], -90.0),
            (IN_ROOM, 90.0),
        ),
        (
            "reach right",
            ([219.0, 2316.0, 296.0], -90.0),
            (IN_ROOM, 90.0),
        ),
        // Only the lower cross trace, corner 3 to 4, meets the arm.
        ("cross", ([217.0, 2324.0, 296.0], -90.0), (IN_ROOM, 90.0)),
        ("turned", ([207.0, 2290.0, 296.0], 0.0), (IN_ROOM, 90.0)),
        (
            "behind",
            ([207.0, 2240.0, 296.0], 90.0),
            ([207.0, 2280.0, 296.0], 90.0),
        ),
        ("plate", far, ([207.0, 2280.0, 296.0], 90.0)),
        ("nothing", far, (IN_ROOM, 90.0)),
    ] {
        t.place([a, ([150.0, 2150.0, 296.0], 0.0), shooter]);
        let temps_before: Vec<u32> = t.level().rt.temp_entities().map(|(n, _)| n).collect();
        let line = t.losses(|l| {
            l.fire_melee_for_test(SHOOTER);
            let mut s = String::new();
            for (n, te) in l.rt.temp_entities() {
                if temps_before.contains(&n) || !matches!(te.event, 35 | 36) {
                    continue;
                }
                write!(
                    s,
                    " te {} parm {} other {} weapon {} at {:?}",
                    te.event, te.parm, te.other, te.weapon, te.origin
                )
                .unwrap();
            }
            let ps = l.pm_state(SHOOTER).unwrap();
            write!(s, " events {:?} parms {:?}", ps.events, ps.event_parms).unwrap();
            s
        });
        writeln!(log, "{name}: {line}").unwrap();
    }
    eprintln!("{log}");
    assert_eq!(log, MELEE);
}

const MELEE: &str = "\
ahead: lost [135, 0, 0] hits [(2, 0, 2)] te 35 parm 1 other 0 weapon 27 at [207.0, 2281.4976, 297.0] events [37, 0, 0, 0] parms [0, 0, 0, 0]
side: lost [138, 0, 0] hits [(2, 0, 6)] te 35 parm 1 other 0 weapon 27 at [215.01053, 2301.2673, 288.9895] events [37, 37, 0, 0] parms [0, 0, 0, 0]
edge: lost [0, 0, 0] hits [] events [37, 37, 0, 0] parms [0, 0, 0, 0]
edge near: lost [0, 0, 0] hits [] events [37, 37, 0, 0] parms [0, 0, 0, 0]
far right: lost [0, 0, 0] hits [] events [37, 37, 0, 0] parms [0, 0, 0, 0]
far ahead: lost [136, 0, 0] hits [(2, 0, 2)] te 35 parm 1 other 0 weapon 27 at [207.0, 2310.835, 297.0] events [37, 37, 37, 0] parms [0, 0, 0, 0]
reach 2322: lost [138, 0, 0] hits [(2, 0, 2)] te 35 parm 1 other 0 weapon 27 at [207.0, 2312.0305, 297.0] events [37, 37, 37, 37] parms [0, 0, 0, 0]
reach 2326: lost [0, 0, 0] hits [] events [37, 37, 37, 37] parms [0, 0, 0, 0]
reach right: lost [0, 0, 0] hits [] events [37, 37, 37, 37] parms [0, 0, 0, 0]
cross: lost [0, 0, 0] hits [] events [37, 37, 37, 37] parms [0, 0, 0, 0]
turned: lost [138, 0, 0] hits [(2, 0, 2)] te 35 parm 1 other 0 weapon 27 at [207.0, 2286.1614, 297.0] events [37, 37, 37, 37] parms [0, 0, 0, 0]
behind: lost [0, 0, 0] hits [] te 36 parm 1 other 1022 weapon 27 at [207.0, 2331.875, 297.0] events [37, 37, 37, 37] parms [0, 0, 0, 0]
plate: lost [0, 0, 0] hits [] te 36 parm 1 other 1022 weapon 27 at [207.0, 2331.875, 297.0] events [37, 37, 37, 37] parms [0, 0, 0, 0]
nothing: lost [0, 0, 0] hits [] events [37, 37, 37, 37] parms [0, 0, 0, 0]
";

/// The playerstate half of a hit and the end frame's feedback on A, for
/// each stance, a knockback-proof `eFlags`, a running `pm_time`, no
/// knockback, a kill, no direction, and `g_knockback` off its default.
#[test]
fn damage_playerstate_by_stance_and_flags() {
    let Some(mut t) = Trio::new(&[]) else {
        return;
    };
    t.place([(A_SPOT, -90.0), (B_SPOT, -90.0), (IN_ROOM, 90.0)]);
    let mut log = String::new();
    let dir = Some([0.6f32, 0.8, 0.0]);
    use vcod_cod4::pmove::pmf;
    for (name, damage, dflags, flags, e_flags, pm_time, fatal, dir, knock) in [
        ("stand", 40, 0, 0, 0, 0, false, dir, ""),
        ("stand big", 300, 0, 0, 0, 0, false, dir, ""),
        ("stand small", 3, 0, 0, 0, 0, false, dir, ""),
        ("crouch", 40, 0, pmf::DUCKED, 0, 0, false, dir, ""),
        ("prone", 40, 0, pmf::PRONE, 0, 0, false, dir, ""),
        ("prone big", 300, 0, pmf::PRONE, 0, 0, false, dir, ""),
        ("eflags 0x100", 40, 0, 0, 0x100, 0, false, dir, ""),
        ("eflags 0x200", 40, 0, 0, 0x200, 0, false, dir, ""),
        ("pm_time", 40, 0, 0, 0, 77, false, dir, ""),
        ("no knockback", 40, 4, 0, 0, 0, false, dir, ""),
        ("no dir", 40, 0, 0, 0, 0, false, None, ""),
        ("g_knockback 500", 40, 0, 0, 0, 0, false, dir, "500"),
        ("g_knockback empty", 40, 0, 0, 0, 0, false, dir, "-"),
        (
            "down dir",
            25,
            0,
            0,
            0,
            0,
            false,
            Some([0.0, -0.6, -0.8]),
            "",
        ),
        ("fatal", 40, 0, 0, 0, 0, true, dir, ""),
    ] {
        let l = t.level();
        l.heal_for_test(A);
        match knock {
            "" => l.rt.host.cvars.set("g_knockback", "1000"),
            "-" => l.rt.host.cvars.set("g_knockback", ""),
            k => l.rt.host.cvars.set("g_knockback", k),
        }
        {
            let p = l.players[A].as_mut().unwrap();
            set(&mut p.ps, "velocity[0]", 0);
            set(&mut p.ps, "velocity[1]", 0);
            set(&mut p.ps, "velocity[2]", 0);
            set(&mut p.ps, "pm_flags", flags);
            set(&mut p.ps, "eFlags", e_flags);
            set(&mut p.ps, "pm_time", pm_time);
            set(&mut p.ps, "damageTimer", 0);
        }
        l.rt.host.cod4_mut().damage_ops.push(Damage4 {
            slot: A,
            damage,
            dflags,
            dir,
            fatal,
            attacker: Some(SHOOTER),
            death: None,
            dead_yaw: None,
        });
        l.apply_damage_ops_for_test();
        let ps = &l.players[A].as_ref().unwrap().ps;
        let w = |n: &str| word(ps, n);
        let f = |n: &str| f32::from_bits(word(ps, n));
        let mut line = format!(
            "vel [{}, {}, {}] pm_time {} pm_flags {:#x} timer {} duration {} pm_type {} weapon {} health {}",
            f("velocity[0]"),
            f("velocity[1]"),
            f("velocity[2]"),
            w("pm_time"),
            w("pm_flags"),
            w("damageTimer"),
            w("damageDuration"),
            w("pm_type"),
            w("weapon"),
            ps.stats[0],
        );
        // A dead one keeps its pm_type for the end frame, the feedback skips it.
        l.damage_feedback_for_test(A);
        let ps = &l.players[A].as_ref().unwrap().ps;
        let w = |n: &str| word(ps, n);
        write!(
            line,
            " | count {} pitch {} yaw {} event {} spread {}",
            w("damageCount"),
            w("damagePitch"),
            w("damageYaw"),
            w("damageEvent"),
            f32::from_bits(w("aimSpreadScale")),
        )
        .unwrap();
        writeln!(log, "{name}: {line}").unwrap();
        // Back to playing for the next row.
        let p = l.players[A].as_mut().unwrap();
        set(&mut p.ps, "pm_type", 0);
        set(&mut p.ps, "pm_flags", 0);
        set(&mut p.ps, "eFlags", 0);
        set(&mut p.ps, "aimSpreadScale", 0);
        let t2 = t.sv.core.sv_time_ms + 600;
        t.run_to(t2);
    }
    t.level().rt.host.cvars.set("g_knockback", "1000");
    eprintln!("{log}");
    assert_eq!(log, PLAYERSTATE);
}

const PLAYERSTATE: &str = "\
stand: vel [28.800001, 38.4, 0] pm_time 50 pm_flags 0x100 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 1 spread 3
stand big: vel [144, 192, 0] pm_time 120 pm_flags 0x100 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 27 pitch 0 yaw 37 event 2 spread 27
stand small: vel [0, 0, 0] pm_time 0 pm_flags 0x0 timer 300 duration 300 pm_type 0 weapon 27 health 1100 | count 0 pitch 0 yaw 37 event 3 spread 0
crouch: vel [14.400001, 19.2, 0] pm_time 50 pm_flags 0x102 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 4 spread 3
prone: vel [0, 0, 0] pm_time 0 pm_flags 0x1 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 5 spread 3
prone big: vel [14.400001, 19.2, 0] pm_time 50 pm_flags 0x101 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 27 pitch 0 yaw 37 event 6 spread 27
eflags 0x100: vel [0, 0, 0] pm_time 0 pm_flags 0x0 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 7 spread 3
eflags 0x200: vel [0, 0, 0] pm_time 0 pm_flags 0x0 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 8 spread 3
pm_time: vel [28.800001, 38.4, 0] pm_time 77 pm_flags 0x0 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 9 spread 3
no knockback: vel [0, 0, 0] pm_time 0 pm_flags 0x0 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 10 spread 3
no dir: vel [0, 0, 0] pm_time 50 pm_flags 0x100 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 255 yaw 255 event 11 spread 3
g_knockback 500: vel [14.400001, 19.2, 0] pm_time 50 pm_flags 0x100 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 12 spread 3
g_knockback empty: vel [28.800001, 38.4, 0] pm_time 50 pm_flags 0x100 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 3 pitch 0 yaw 37 event 13 spread 3
down dir: vel [0, -16.800001, -22.4] pm_time 50 pm_flags 0x100 timer 750 duration 750 pm_type 0 weapon 27 health 1100 | count 2 pitch 37 yaw 192 event 14 spread 2
fatal: vel [28.800001, 38.4, 0] pm_time 50 pm_flags 0x100 timer 750 duration 750 pm_type 7 weapon 0 health 0 | count 0 pitch 37 yaw 192 event 14 spread 0
";

/// A blow along yaw -1.2 from 0.7 degrees below packs as 255 both, the
/// bytes' none, and the level still has its direction for the bots.
#[test]
fn a_direction_that_packs_as_none_keeps_its_direction() {
    let Some(mut t) = Trio::new(&[]) else {
        return;
    };
    let l = t.level();
    let dir = [0.9997134f32, -0.02088902, 0.01170162];
    for (from, want) in [(Some(dir), Some(Vec3::from(dir))), (None, None)] {
        l.heal_for_test(A);
        l.rt.host.cod4_mut().damage_ops.push(Damage4 {
            slot: A,
            damage: 40,
            dflags: 0,
            dir: from,
            fatal: false,
            attacker: Some(SHOOTER),
            death: None,
            dead_yaw: None,
        });
        l.apply_damage_ops_for_test();
        l.damage_feedback_for_test(A);
        let ps = &l.players[A].as_ref().unwrap().ps;
        assert_eq!((word(ps, "damagePitch"), word(ps, "damageYaw")), (255, 255));
        let got = l.hit_dir(A);
        assert_eq!(got.is_some(), want.is_some());
        if let (Some(got), Some(want)) = (got, want) {
            assert!(got.dot(want) > 0.9999, "{got} against {want}");
        }
    }
}

/// A playerstate word by its field name, and its write.
fn word(ps: &vcod_cod4::net::delta::PlayerState, name: &str) -> u32 {
    ps.get(name).unwrap_or_else(|| panic!("no field {name}"))
}

fn set(ps: &mut vcod_cod4::net::delta::PlayerState, name: &str, v: u32) {
    let f = vcod_cod4::net::fields::PLAYER_STATE
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name}"));
    ps.words[f.word()] = v;
}
