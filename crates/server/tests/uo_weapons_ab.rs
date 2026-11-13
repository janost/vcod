//! United Offensive's special weapons on a protocol 22 server against the
//! retail captures `client-probes/probe_uofx` produced
//! (`crates/common/tests/fixtures/net/*-uo-{arty,smoke,mg}.bin`, a spectator
//! following a `--probe-uo-weapon` shooter on mp_cassino dm; recipe in
//! docs/research/coduo-diff.md 19). Ours runs the same probe gametype with
//! the shooter placed where retail's stood, drives the same step script and
//! records the events its own snapshots carry; each gate compares the event
//! sequence and the carriers' state (docs/research/coduo-diff.md 21).
//!
//! `UOWEAP_REPORT=1` prints both sides' events. Needs `COD_DIR` at a UO
//! install; without one it returns early.

use crate::common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetEvent;
use vcod_common::net::event_ids::{
    EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT, EV_FIRE_WEAPON_MG42, EV_FOREIGN, EV_GRENADE_BOUNCE,
    EV_NOAMMO, EV_UO_REMOVE_WEAPON_ATTACHMENTS,
};
use vcod_common::net::events::{EventTracker, GameEvent};
use vcod_common::net::flags::EF_FIRING;
use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, BUTTON_USE, UserCmd};

/// A mounted gun's `eType` (11 on both protocols).
const ET_TURRET_V22: i32 = 11;
use vcod_common::net::protocol::{PROTOCOL_V22, Protocol};
use vcod_common::net::snapshot::Snapshot;

const MAP: &str = "mp_cassino";
const PROBE: &str = "probe_uofx";

/// One step of `vcod --probe-uo-weapon`'s scripts
/// (`crates/client/src/vehicle_probe.rs`): how long it holds, its buttons
/// and the view offset from the spawn's, ramped from the previous step's.
struct Step {
    ms: i64,
    buttons: u8,
    view: [f32; 2],
}

const fn foot(ms: i64, buttons: u8, view: [f32; 2]) -> Step {
    Step { ms, buttons, view }
}

/// `WeaponScript::Arty`.
const ARTY: &[Step] = &[
    foot(3000, 0, [12.0, 0.0]),
    foot(1500, BUTTON_ADS, [12.0, 0.0]),
    foot(200, BUTTON_ADS | BUTTON_ATTACK, [12.0, 0.0]),
    foot(3000, BUTTON_ADS, [12.0, 0.0]),
    foot(60_000, 0, [12.0, 0.0]),
];

/// `WeaponScript::Smoke`.
const SMOKE: &[Step] = &[
    foot(3000, 0, [-10.0, 0.0]),
    foot(1000, BUTTON_ATTACK, [-10.0, 0.0]),
    foot(2000, 0, [-10.0, 0.0]),
    foot(60_000, 0, [-10.0, 0.0]),
];

/// `WeaponScript::Mg`.
const MG: &[Step] = &[
    foot(3000, 0, [0.0, 0.0]),
    foot(3000, BUTTON_ADS, [0.0, 0.0]),
    foot(3000, BUTTON_ADS | BUTTON_ATTACK, [0.0, 0.0]),
    foot(2000, BUTTON_ADS, [0.0, 0.0]),
    foot(3000, 0, [0.0, 0.0]),
    foot(1000, BUTTON_ATTACK, [0.0, 0.0]),
    foot(60_000, 0, [0.0, 0.0]),
];

/// The cmd buttons and view offset `ms` into a script.
fn script_at(script: &[Step], mut ms: i64) -> (u8, [f32; 2]) {
    let mut from = [0.0f32; 2];
    for s in script {
        if ms < s.ms {
            let f = ms as f32 / s.ms as f32;
            return (
                s.buttons,
                [0, 1].map(|i| from[i] + (s.view[i] - from[i]) * f),
            );
        }
        ms -= s.ms;
        from = s.view;
    }
    let last = script.last().unwrap();
    (last.buttons, last.view)
}

/// One event with the server time of the snapshot that brought it.
pub struct Seen {
    pub t: i32,
    pub ev: GameEvent,
}

/// What one side's snapshots carried: every event, and the last snapshot
/// time each entity number was on the wire.
#[derive(Default)]
struct Run {
    seen: Vec<Seen>,
    last_seen: BTreeMap<u32, i32>,
    /// Every missile (`eType` 4) on each snapshot: the snapshot time, its
    /// number and its trajectory.
    missiles: Vec<(i32, u32, Flight)>,
}

/// A missile's `pos` trajectory and `apos.trBase` as the wire carries them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Flight {
    tr_type: i32,
    tr_time: i32,
    base: [f32; 3],
    delta: [f32; 3],
    /// Degrees, as the 16-bit wire angle decodes.
    apos: [f32; 3],
}

impl Run {
    fn record(&mut self, tracker: &mut EventTracker, s: &Snapshot, p: &Protocol) {
        for ev in tracker.drain(s, p) {
            self.seen.push(Seen {
                t: s.server_time,
                ev,
            });
        }
        for (&n, e) in &s.entities {
            self.last_seen.insert(n, s.server_time);
            if e.field_i32(p, "eType") == 4 {
                let f = |name: &str| e.field_f32(p, name);
                self.missiles.push((
                    s.server_time,
                    n,
                    Flight {
                        tr_type: e.field_i32(p, "pos.trType"),
                        tr_time: e.field_i32(p, "pos.trTime"),
                        base: [0, 1, 2].map(|i| f(&format!("pos.trBase[{i}]"))),
                        delta: [0, 1, 2].map(|i| f(&format!("pos.trDelta[{i}]"))),
                        apos: [0, 1, 2].map(|i| f(&format!("apos.trBase[{i}]"))),
                    },
                ));
            }
        }
    }

    /// The events with this id.
    fn of(&self, event: i32) -> Vec<&Seen> {
        self.seen.iter().filter(|s| s.ev.event == event).collect()
    }

    /// The first event with this id. A spectator's follow replays the
    /// shooter's ring once it lands, so retail's playerstate events can
    /// come twice.
    fn first(&self, event: i32) -> &Seen {
        self.of(event)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no event {event:#x}"))
    }
}

/// The retail capture's events in 1.1's numbering, and where the followed
/// shooter stood and faced on its first playing snapshot.
fn retail(tag: &str) -> (Run, [f32; 3], f32) {
    let p = &PROTOCOL_V22;
    let frames = vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-uo-{tag}.bin"),
        &format!("net/snapshots-uo-{tag}.bin"),
    );
    let mut tracker = EventTracker::new();
    let mut run = Run::default();
    let mut spawn = None;
    for f in &frames {
        let s = &f.snapshot;
        if spawn.is_none() && s.ps.field_i32(p, "pm_type") == 0 && s.ps.field_i32(p, "weapon") != 0
        {
            spawn = Some((s.ps.origin(p), s.ps.viewangles(p)[1]));
        }
        run.record(&mut tracker, s, p);
    }
    let (origin, yaw) = spawn.expect("a playing snapshot");
    (run, origin, yaw)
}

/// The probe on ours: `probe_weapon` set to `weapon`, the shooter placed at
/// `origin` facing `yaw`, driven by `script` from its first playing
/// snapshot for `secs`, every event its snapshots carried.
fn ours(weapon: &str, script: &[Step], origin: [f32; 3], yaw: f32, secs: i64) -> Run {
    ours_with(weapon, script, origin, yaw, secs, |_| {})
}

/// [`ours`] with `setup` run on the server once its scripts are loaded.
fn ours_with(
    weapon: &str,
    script: &[Step],
    origin: [f32; 3],
    yaw: f32,
    secs: i64,
    setup: impl FnOnce(&mut vcod_server::Server),
) -> Run {
    let fs = vcod_common::testing::uo_fs().unwrap();
    let p: &'static Protocol = &PROTOCOL_V22;
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{PROBE}.gsc");
    let probe = std::fs::read_to_string(src).expect("read the probe");
    let bsp_path = fs.resolve_map(MAP).expect("the map in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, PROBE), now);
    sv.set_protocol(p);
    sv.overlay_script(&format!("maps/mp/gametypes/{PROBE}"), &probe);
    sv.set_cvar("probe_weapon", weapon);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    setup(&mut sv);
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut join = common::Join::new("axis", "kar98k_mp");
    let mut placed = false;

    let mut tracker = EventTracker::new();
    let mut out = Run::default();
    let mut selected: Option<u8> = None;
    let mut spawned_ms: Option<i64> = None;
    let mut ms = 0i64;
    let frames = secs * 1000 / common::FRAME_MS;
    for _ in 0..frames {
        let newest = cl.snapshots().newest();
        let playing = newest.is_some_and(|s| s.ps.field_i32(p, "pm_type") == 0);
        if playing && !placed {
            sv.place_client(0, origin, yaw);
            placed = true;
        } else if placed && spawned_ms.is_none() {
            spawned_ms = Some(ms);
        }
        let held = newest.map_or(0, |s| s.ps.field_i32(p, "weapon") as u8);
        // The probe client's own cadence, a cmd a 60 Hz frame: a weapon's
        // `fireTime` is spent in whole cmds.
        let mut at = 0;
        for cmd_ms in [16, 17, 17] {
            let (buttons, off) =
                spawned_ms.map_or((0, [0.0, 0.0]), |t0| script_at(script, ms + at - t0));
            at += cmd_ms;
            let cmd = UserCmd {
                buttons,
                weapon: selected.unwrap_or(held),
                angles: [deg_to_short(off[0]), deg_to_short(yaw + off[1]), 0],
                ..vcod_common::net::msg::NULL_USERCMD
            };
            now += Duration::from_millis(cmd_ms as u64);
            cl.pump_at(now);
            cl.send_frame(&cmd);
        }
        ms += common::FRAME_MS;
        for e in common::step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if tokens.first().map(String::as_str) == Some("a") {
                    selected = tokens.get(1).and_then(|w| w.parse().ok());
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest() {
            out.record(&mut tracker, s, p);
        }
    }
    if std::env::var_os("UOWEAP_REPORT").is_some() {
        for l in sv.script_log() {
            print!("ours log {l}");
        }
    }
    out
}

/// mp_cassino's collision with its props, as the server builds it.
fn cassino_world() -> vcod_common::collision::CollisionWorld {
    let fs = vcod_common::testing::uo_fs().unwrap();
    let bsp_path = fs.resolve_map(MAP).expect("the map in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let props = vcod_common::props::collision_tris(&fs, &bsp.entities);
    vcod_common::collision::CollisionWorld::build(&bsp, &props)
}

/// A 16-bit wire angle as the snapshot decodes it: degrees truncated to
/// 65536ths toward zero (`codec::write_angle_field`), 0 sent as 0.
fn wire_angle(d: f32) -> f32 {
    let short = (f64::from(d) * f64::from(65536.0f32 / 360.0)) as i64 as u16 as i16;
    f32::from(short) * (360.0 / 65536.0)
}

fn deg_to_short(d: f32) -> i32 {
    ((d * 65536.0 / 360.0).round() as i32) & 0xffff
}

fn report(side: &str, run: &Run) {
    if std::env::var_os("UOWEAP_REPORT").is_none() {
        return;
    }
    for s in &run.seen {
        let e = &s.ev;
        let carrier = match e.entity_num {
            u32::MAX => "ps".to_string(),
            n => n.to_string(),
        };
        println!(
            "{side} t {} ev {:#x} on {carrier} eType {} parm {} weapon {} scale {} surf {} eFlags {:#x} pos {:?}",
            s.t,
            e.event,
            e.e_type,
            e.parm,
            e.weapon,
            e.scale,
            e.surf_type,
            e.eflags,
            e.pos.map(|v| v.round() as i32),
        );
    }
}

const EV_DEPLOY: i32 = EV_FOREIGN | 160;
const EV_BREAKDOWN: i32 = EV_FOREIGN | 161;
const EV_LAUNCH: i32 = EV_FOREIGN | 183;
const EV_INCOMING: i32 = EV_FOREIGN | 184;
const EV_EXPLODE: i32 = EV_FOREIGN | 185;
const EV_HIT: i32 = EV_FOREIGN | 178;

/// The binoculars' strike: the sight's deploy and breakdown on the
/// binoculars and no take, the launches from the strike point 250 ms on,
/// the first incoming `projectileDelay` on, every shell's explode on its
/// incoming's spot 1.8 to 2 s later as an `eType` 0 entity with type 4 in
/// `es.scale`, the binoculars' weapon and `eFlags` 0x8000.
#[test]
fn the_artillery_strike_lands_as_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (retail, origin, yaw) = retail("arty");
    let ours = ours("binoculars_artillery_mp", ARTY, origin, yaw, 45);
    report("retail", &retail);
    report("ours", &ours);
    let binoculars = vcod_server::defs::UO
        .weapon_index("binoculars_artillery_mp")
        .unwrap() as i32;
    for (side, run) in [("retail", &retail), ("ours", &ours)] {
        let fire = run.first(EV_FIRE_WEAPON_LASTSHOT);
        assert_eq!(fire.ev.weapon, binoculars, "{side}");
        assert!(
            run.of(EV_DEPLOY).iter().all(|e| e.ev.weapon == binoculars),
            "{side}: the sight deploys the binoculars"
        );
        let breakdown = run
            .of(EV_BREAKDOWN)
            .into_iter()
            .find(|e| e.t > fire.t)
            .unwrap_or_else(|| panic!("{side}: a breakdown after the strike"));
        assert_eq!(
            breakdown.ev.weapon, binoculars,
            "{side}: the binoculars are kept"
        );
        assert!(
            run.of(EV_NOAMMO).is_empty(),
            "{side}: a spent spotter raises no 149"
        );
        let launches = run.of(EV_LAUNCH);
        let incomings = run.of(EV_INCOMING);
        let explodes = run.of(EV_EXPLODE);
        assert_eq!(launches.len(), 15, "{side} launches");
        assert_eq!(explodes.len(), 15, "{side} explodes");
        // A temp event's number reused on the next snapshot is not seen.
        assert!(
            incomings.len() >= 13,
            "{side} incomings {}",
            incomings.len()
        );
        // The cmd runs on the frame before the fire's snapshot.
        let level = fire.t - common::FRAME_MS as i32;
        assert_eq!(launches[0].t, level + 300, "{side}: the first launch");
        assert_eq!(incomings[0].t, level + 6050, "{side}: the first incoming");
        let strike = launches[0].ev.pos;
        for l in &launches {
            assert_eq!((l.ev.parm, l.ev.pos), (4, strike), "{side} launch");
        }
        for i in &incomings {
            assert_eq!(i.ev.parm, 4, "{side} incoming");
            let d = glam::Vec2::new(i.ev.pos[0] - strike[0], i.ev.pos[1] - strike[1]);
            assert!(d.length() < 850.0 * 1.5, "{side}: incoming {:?}", i.ev.pos);
            let shell = explodes
                .iter()
                .find(|e| {
                    (e.ev.pos[0] - i.ev.pos[0]).abs() < 2.0
                        && (e.ev.pos[1] - i.ev.pos[1]).abs() < 2.0
                })
                .unwrap_or_else(|| panic!("{side}: no explode under {:?}", i.ev.pos));
            assert!(
                (1750..=2050).contains(&(shell.t - i.t)),
                "{side}: incoming {} explode {}",
                i.t,
                shell.t
            );
        }
        for e in &explodes {
            assert_eq!(
                (e.ev.e_type, e.ev.scale, e.ev.weapon, e.ev.eflags),
                (0, 4, binoculars, 0x10000),
                "{side} explode at {}",
                e.t
            );
        }
    }
    // The shells land on random spots, so retail's surfaces are checked
    // against our collision on retail's own spots: the face a shell falling
    // straight down meets there under its clipmask (0x2802091, the kerbs
    // included) is the side it reports.
    let world = cassino_world();
    for e in retail.of(EV_EXPLODE) {
        let p = glam::Vec3::from(e.ev.pos);
        let t = world.point_trace(
            p + glam::Vec3::Z * 40.0,
            p - glam::Vec3::Z * 40.0,
            vcod_common::collision::MASK_BLAST,
            true,
        );
        assert_eq!(
            vcod_common::collision::sound_material(t.surface_flags),
            e.ev.surf_type,
            "the shell at {p}: {}",
            t.hit.map(|h| world.describe(h)).unwrap_or_default()
        );
    }
    // Same spot, same view: the strike lands where retail's did.
    let (r, o) = (retail.of(EV_LAUNCH)[0].ev.pos, ours.of(EV_LAUNCH)[0].ev.pos);
    assert!(
        (glam::Vec3::from(r) - glam::Vec3::from(o)).length() < 4.0,
        "retail {r:?} ours {o:?}"
    );
}

/// The smoke grenade: the same two bounces at the same spots and times,
/// the pop 3.95 s after the throw on the grenade itself (`eType` 4, type 1,
/// the floor's normal and surface), and the grenade on the wire 15 s more.
#[test]
fn the_smoke_grenade_pops_as_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (retail, origin, yaw) = retail("smoke");
    let ours = ours("smokegrenade_mp", SMOKE, origin, yaw, 40);
    report("retail", &retail);
    report("ours", &ours);
    let shape = |run: &Run| {
        let throw = run.first(EV_FIRE_WEAPON).t;
        let bounces: Vec<_> = run
            .of(EV_GRENADE_BOUNCE)
            .iter()
            .map(|b| (b.t - throw, b.ev.parm, b.ev.pos.map(|v| v.round() as i32)))
            .collect();
        let pop = run.first(EV_EXPLODE);
        let n = pop.ev.entity_num;
        let gone = run.last_seen[&n] - pop.t;
        (
            bounces,
            (
                pop.t - throw,
                pop.ev.e_type,
                pop.ev.parm,
                pop.ev.scale,
                pop.ev.surf_type,
                pop.ev.pos.map(|v| v.round() as i32),
            ),
            gone,
        )
    };
    let (r, o) = (shape(&retail), shape(&ours));
    assert_eq!(r.0, o.0, "bounces");
    assert_eq!(r.1, o.1, "the pop");
    // Retail's capture ends inside the 15 s; ours rides until the free.
    assert!(r.2 >= 14_000, "retail kept it {} ms", r.2);
    assert_eq!(
        o.2,
        15_000 - common::FRAME_MS as i32,
        "ours kept it {} ms",
        o.2
    );
}

/// The bipod mg34: the sight deploys it, the held trigger fires at retail's
/// cadence, every round's world impact carries the weapon and a reflection,
/// and letting the sight go breaks it down.
#[test]
fn the_bipod_mg_deploys_and_fires_as_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (retail, origin, yaw) = retail("mg");
    let ours = ours("mg34_mp", MG, origin, yaw, 25);
    report("retail", &retail);
    report("ours", &ours);
    let mg34 = vcod_server::defs::UO.weapon_index("mg34_mp").unwrap() as i32;
    let shape = |run: &Run| {
        let deploy = run.of(EV_DEPLOY)[0].t;
        let fires: Vec<i32> = run
            .of(EV_FIRE_WEAPON)
            .iter()
            .map(|f| f.t - deploy)
            .collect();
        let hits = run.of(EV_HIT);
        assert!(
            hits.iter().all(|h| h.ev.weapon == mg34 && h.ev.scale != 0),
            "every impact carries the mg34 and a reflection"
        );
        let breakdown = run.of(EV_BREAKDOWN)[0].t - deploy;
        (fires.len(), fires[0], hits.len(), breakdown)
    };
    assert_eq!(shape(&retail), shape(&ours));
}

const EV_OVERHEATING: i32 = EV_FOREIGN | 208;

/// The mounted gun's events off a run: each round's time and parm, the
/// overheats' times, and the time `animMovetype` 1 first reached the wire.
fn turret_shape(run: &Run, gun: u32) -> (Vec<(i32, i32)>, Vec<i32>) {
    let on_gun = |e: &&Seen| e.ev.entity_num == gun;
    let rounds = run
        .of(EV_FIRE_WEAPON_MG42)
        .into_iter()
        .filter(on_gun)
        .map(|e| (e.t, e.ev.parm))
        .collect();
    let overheats = run
        .of(EV_OVERHEATING)
        .into_iter()
        .filter(on_gun)
        .map(|e| e.t)
        .collect();
    (rounds, overheats)
}

/// UO's mounted MG with the trigger held 10 s (`*-uo-turret.bin`,
/// `client-probes/probe_turret` on mp_carentan): 100 rounds 50 ms apart
/// heat it to `EV_OVERHEATING` 5 s after the first, the gun holds off until
/// the heat is down to half, 2.6 s, with the trigger still down, and the
/// gun keeps `animMovetype` 1 from the overheat on. Ours has no target, so
/// every round carries bit 7 (retail's that met none did too).
#[test]
fn the_mounted_mg_overheats_and_cools_as_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let p: &'static Protocol = &PROTOCOL_V22;
    let frames = vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-uo-turret.bin",
        "net/snapshots-uo-turret.bin",
    );
    let mut tracker = EventTracker::new();
    let mut retail = Run::default();
    let mut retail_mark = None;
    let mut gun = None;
    for f in &frames {
        retail.record(&mut tracker, &f.snapshot, p);
        for (&n, e) in &f.snapshot.entities {
            if e.field_i32(p, "eType") == ET_TURRET_V22 {
                if e.field_i32(p, "eFlags") & EF_FIRING != 0 {
                    gun = Some(n);
                }
                if e.field_i32(p, "animMovetype") == 1 && retail_mark.is_none() {
                    retail_mark = Some(f.snapshot.server_time);
                }
            }
        }
    }
    let (r_rounds, r_heats) = turret_shape(&retail, gun.expect("the fired gun"));

    let map = "mp_carentan";
    let src = "../gsc/tests/fixtures/semantics/client-probes/probe_turret.gsc";
    let probe = std::fs::read_to_string(src).expect("read the probe");
    let bsp_path = fs.resolve_map(map).expect("mp_carentan in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_turret"), now);
    sv.set_protocol(p);
    sv.overlay_script("maps/mp/gametypes/probe_turret", &probe);
    sv.set_cvar("probe_teleport", "1");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut cl, _join) = common::join(&mut sv, &q, &mut now, "allies", "m1garand_mp");
    let mut ours = Run::default();
    let mut tracker = EventTracker::new();
    let mut ours_mark = None;
    let mut our_gun = None;
    // Settle, use, 10 s on the trigger, let go.
    for frame in 0..(2_000 + 300 + 12_000) / common::FRAME_MS {
        let ms = frame * common::FRAME_MS;
        let newest = cl.snapshots().newest();
        let mut view = newest.map_or([0.0; 3], |s| s.ps.viewangles(p));
        let weapon = newest.map_or(0, |s| s.ps.field_i32(p, "weapon") as u8);
        // The joins' cmds turned the view off the script's
        // `setPlayerAngles`; face the gun before the use, as turret_ab does.
        if (1_000..2_000).contains(&ms)
            && let Some(s) = newest
            && let Some(at) = s
                .entities
                .values()
                .filter(|e| e.field_i32(p, "eType") == ET_TURRET_V22)
                .map(|e| e.origin(p))
                .min_by(|a, b| {
                    let d = |o: &[f32; 3]| (o[0] - 1712.0).hypot(o[1] - 1830.0);
                    d(a).total_cmp(&d(b))
                })
        {
            let me = s.ps.origin(p);
            view = [0.0, (at[1] - me[1]).atan2(at[0] - me[0]).to_degrees(), 0.0];
        }
        let buttons = match ms {
            2_000..2_100 => BUTTON_USE,
            2_300..12_300 => BUTTON_ATTACK,
            _ => 0,
        };
        for cmd_ms in [16, 17, 17] {
            now += Duration::from_millis(cmd_ms);
            cl.pump_at(now);
            cl.send_frame(&UserCmd {
                buttons,
                weapon,
                angles: [deg_to_short(view[0]), deg_to_short(view[1]), 0],
                ..vcod_common::net::msg::NULL_USERCMD
            });
        }
        common::step(&mut sv, &q, &mut cl, now);
        if let Some(s) = cl.snapshots().newest() {
            ours.record(&mut tracker, s, p);
            for (&n, e) in &s.entities {
                if e.field_i32(p, "eType") == ET_TURRET_V22 {
                    if e.field_i32(p, "eFlags") & EF_FIRING != 0 {
                        our_gun = Some(n);
                    }
                    if e.field_i32(p, "animMovetype") == 1 && ours_mark.is_none() {
                        ours_mark = Some(s.server_time);
                    }
                }
            }
        }
    }
    let (o_rounds, o_heats) = turret_shape(&ours, our_gun.expect("our gun fired"));
    if std::env::var_os("UOWEAP_REPORT").is_some() {
        println!("retail rounds {r_rounds:?}\nheats {r_heats:?} mark {retail_mark:?}");
        println!("ours rounds {o_rounds:?}\nheats {o_heats:?} mark {ours_mark:?}");
    }
    let shape = |rounds: &[(i32, i32)], heats: &[i32], mark: Option<i32>| {
        let first = rounds[0].0;
        let heat = heats[0];
        let before = rounds.iter().filter(|r| r.0 < heat).count();
        let resume = rounds.iter().find(|r| r.0 > heat).map(|r| r.0 - heat);
        (before, heat - first, resume, mark.map(|m| m - heat))
    };
    let r = shape(&r_rounds, &r_heats, retail_mark);
    let o = shape(&o_rounds, &o_heats, ours_mark);
    assert_eq!(r, o, "(rounds before, overheat after first, resume, mark)");
    assert_eq!(o_heats.len(), 1, "one overheat in the burst");
    assert!(
        o_rounds.iter().all(|r| r.1 == 0x80),
        "every round into the world carries bit 7"
    );
    // Retail's rounds that met nothing carry it too.
    assert!(r_rounds.iter().any(|r| r.1 == 0x80));
}

/// UO's flamethrower damage is the burnt client's own report: `fdc <n>`
/// names whose flame it stands in, and while its cmds carry `wbuttons` 1
/// it takes the flamethrower's 2 four times over every frame for 3 s
/// (`G_CheckFlameDamage`, game.mp.uo 0x3f4d4); without the bit, nothing.
#[test]
fn a_reported_flame_burns_every_frame_while_the_bit_is_held() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let p: &'static Protocol = &PROTOCOL_V22;
    let bsp_path = fs.resolve_map(MAP).unwrap();
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "dm"), now);
    sv.set_protocol(p);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let (qa, qb) = (
        Rc::new(RefCell::new(common::Queues::default())),
        Rc::new(RefCell::new(common::Queues::default())),
    );
    let (mut a, mut b) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("axis", "kar98k_mp"),
        ("allies", "m1garand_mp"),
    );
    let health = |cl: &vcod_common::net::NetClient<common::ClientEnd>| {
        cl.snapshots().newest().map_or(0, |s| s.ps.health())
    };
    let mut frame = |a: &mut vcod_common::net::NetClient<common::ClientEnd>,
                     b: &mut vcod_common::net::NetClient<common::ClientEnd>,
                     now: &mut Instant,
                     wbuttons: u8| {
        *now += Duration::from_millis(common::FRAME_MS as u64);
        for (cl, wb) in [(&mut *a, wbuttons), (&mut *b, 0)] {
            let mut cmd = common::holding(cl);
            cmd.wbuttons = wb;
            cl.send_frame(&cmd);
        }
        common::step_pair(&mut sv, (&qa, a), (&qb, b), *now);
    };
    for _ in 0..10 {
        frame(&mut a, &mut b, &mut now, 0);
    }
    assert_eq!(health(&a), 100);
    a.send_reliable("fdc 1");
    for _ in 0..4 {
        frame(&mut a, &mut b, &mut now, 0);
    }
    assert_eq!(health(&a), 100, "no damage without the bit");
    let damage_event = |cl: &vcod_common::net::NetClient<common::ClientEnd>| {
        cl.snapshots()
            .newest()
            .map_or(0, |s| s.ps.field_i32(p, "damageEvent"))
    };
    let mut seen = vec![(health(&a), damage_event(&a))];
    for _ in 0..6 {
        frame(&mut a, &mut b, &mut now, 1);
        seen.push((health(&a), damage_event(&a)));
    }
    let drops: Vec<i32> = seen[1..].windows(2).map(|w| w[0].0 - w[1].0).collect();
    assert!(
        drops.iter().all(|&d| d == 8),
        "8 a frame from another's flame: {seen:?}"
    );
    // `P_DamageFeedback` runs in the burn's own `ClientEndFrame`: every
    // snapshot that carries a burn carries its feedback too.
    assert!(
        seen.windows(2)
            .all(|w| (w[1].0 < w[0].0) == (w[1].1 != w[0].1)),
        "the feedback rides the burn's frame: {seen:?}"
    );
}

/// A flamethrower pulse fires nothing on the server: UO's `FireWeapon`
/// (game.mp.uo 0x7bd05) skips weapon type 4, `gas`, so a player in front of
/// the nozzle takes no damage until his own client reports the flame.
#[test]
fn a_flamethrower_pulse_deals_no_bullet_damage() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let p: &'static Protocol = &PROTOCOL_V22;
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{PROBE}.gsc");
    let probe = std::fs::read_to_string(src).expect("read the probe");
    let bsp_path = fs.resolve_map(MAP).unwrap();
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, PROBE), now);
    sv.set_protocol(p);
    sv.overlay_script(&format!("maps/mp/gametypes/{PROBE}"), &probe);
    sv.set_cvar("probe_weapon", "flamethrower_mp");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let (qa, qb) = (
        Rc::new(RefCell::new(common::Queues::default())),
        Rc::new(RefCell::new(common::Queues::default())),
    );
    let (mut a, mut b, mut ja, mut jb) = common::join_pair_logged(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("axis", "kar98k_mp"),
        ("allies", "m1garand_mp"),
    );
    let flamer = vcod_server::defs::UO
        .weapon_index("flamethrower_mp")
        .unwrap() as u8;
    // The shooter 100 units behind the target, both facing +x.
    sv.place_client(0, [3072.0, -1896.0, 0.0], 0.0);
    sv.place_client(1, [3172.0, -1896.0, 0.0], 0.0);
    let mut fires = 0;
    let mut health = Vec::new();
    for frame in 0..120 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        let held = a
            .snapshots()
            .newest()
            .map_or(0, |s| s.ps.field_i32(p, "weapon") as u8);
        let attack = held == flamer && frame % 14 < 3;
        let cmd = UserCmd {
            buttons: if attack { BUTTON_ATTACK } else { 0 },
            weapon: if frame > 30 { flamer } else { held },
            ..vcod_common::net::msg::NULL_USERCMD
        };
        a.send_frame(&cmd);
        b.send_frame(&common::holding(&b));
        let (ea, eb) = common::step_pair(&mut sv, (&qa, &mut a), (&qb, &mut b), now);
        for (events, join, cl) in [(ea, &mut ja, &mut a), (eb, &mut jb, &mut b)] {
            for e in events {
                if let NetEvent::ServerCommand(tokens) = e {
                    join.on_server_command(&tokens, cl, now);
                }
            }
        }
        if let Some(s) = a.snapshots().newest()
            && s.ps.field_i32(p, "weaponstate") == 3
        {
            fires += 1;
        }
        health.push(b.snapshots().newest().map_or(0, |s| s.ps.health()));
    }
    assert!(fires > 0, "the shooter never fired the flamethrower");
    assert!(
        health.iter().all(|&h| h == 100),
        "the target took damage from the pulses: {health:?}"
    );
    assert!(
        !sv.script_log()
            .iter()
            .any(|l| l.starts_with("D;") || l.starts_with("K;")),
        "a damage line: {:?}",
        sv.script_log()
    );
}

/// The satchel charge (`*-uo-satchel.bin`, `probe_uofx` with
/// `probe_weapon satchelcharge_mp`, the smoke script): its bounces at the
/// same spots and times with the shootable `eFlags` 0x10 and the surfaces
/// (5, 17, 17: the sides struck, not their brushes' materials), its explode
/// 6 s after the throw as an `eType` 0 entity with type 2 on surface 17,
/// and the thrower's `EV_REMOVE_WEAPON_ATTACHMENTS` with the satchel's
/// index when the spent satchel goes down, as long after the throw.
#[test]
fn the_satchel_charge_bounces_and_blows_as_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (retail, origin, yaw) = retail("satchel");
    let ours = ours("satchelcharge_mp", SMOKE, origin, yaw, 20);
    report("retail", &retail);
    report("ours", &ours);
    let shape = |run: &Run| {
        let throw = run.first(EV_FIRE_WEAPON_LASTSHOT).t;
        let bounces: Vec<_> = run
            .of(EV_GRENADE_BOUNCE)
            .iter()
            .map(|b| {
                (
                    b.t - throw,
                    b.ev.eflags,
                    b.ev.parm,
                    b.ev.pos.map(|v| v.round() as i32),
                )
            })
            .collect();
        let x = run.first(EV_EXPLODE);
        let gone = run.first(EV_UO_REMOVE_WEAPON_ATTACHMENTS);
        (
            bounces,
            (
                x.t - throw,
                x.ev.e_type,
                x.ev.parm,
                x.ev.scale,
                x.ev.eflags,
                x.ev.surf_type,
                x.ev.pos.map(|v| v.round() as i32),
            ),
            (gone.t - throw, gone.ev.parm),
        )
    };
    assert_eq!(shape(&retail), shape(&ours));
}

/// A UO player rocket (`*-uo-bazooka.bin`, `probe_uofx` with `probe_weapon
/// bazooka_mp` and a `--probe-weapsel` shooter from the hip, coduo-diff.md
/// 19). Ours fires from retail's spot along retail's view. The hip spread
/// is a random draw neither side can repeat, so the rocket's direction is
/// each side's own: the gate compares its trajectory on every snapshot of
/// the flight (`TR_LINEAR`, `trTime` 100 ms before the fire's snapshot,
/// the muzzle, the 2800 speed truncated, held still), the explode's carrier (185 on
/// the rocket as `eType` 0, the rocket type in `es.scale`, the weapon,
/// `eFlags`), and, on each side, an explode on the snapshot its flight
/// reaches it. Retail's flight is traced through our world too: its line
/// meets our collision where retail's rocket went off, on its surface.
#[test]
fn the_bazooka_rocket_flies_and_explodes_as_on_uo() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (retail, origin, yaw) = retail("bazooka");
    let p = &PROTOCOL_V22;
    let frames = vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-uo-bazooka.bin",
        "net/snapshots-uo-bazooka.bin",
    );
    let fire_t = retail.first(EV_FIRE_WEAPON_LASTSHOT).t;
    // The view the fire's cmd carried: the shooter's on the snapshot before.
    let view = frames
        .iter()
        .map(|f| &f.snapshot)
        .take_while(|s| s.server_time < fire_t)
        .last()
        .expect("a snapshot before the fire")
        .ps
        .viewangles(p);
    let aim = [view[0], view[1] - yaw];
    let script = [
        foot(5000, 0, aim),
        foot(150, BUTTON_ATTACK, aim),
        foot(60_000, 0, aim),
    ];
    let ours = ours("bazooka_mp", &script, origin, yaw, 15);
    report("retail", &retail);
    report("ours", &ours);
    let flight = |run: &Run| -> (Vec<_>, Flight) {
        let fire = run.first(EV_FIRE_WEAPON_LASTSHOT).t;
        let on: Vec<_> = run.missiles.iter().filter(|(t, ..)| *t >= fire).collect();
        let first = on.first().expect("the rocket on a snapshot").2;
        assert!(
            on.iter().all(|(.., f)| *f == first),
            "the flight changed: {on:?}"
        );
        // `trDelta` goes out truncated to whole units (cod11-combat.md 13.6), so
        // its length sits within a unit or two under the 2800 the draw turns.
        assert!(
            first.delta.iter().all(|v| v.fract() == 0.0),
            "a fractional delta {:?}",
            first.delta
        );
        let speed = glam::Vec3::from(first.delta).length();
        assert!((2797.0..=2800.0).contains(&speed), "speed {speed}");
        // `apos` is `vectoangles` of the truncated `trDelta`, its pitch
        // wrapped into [0, 360) before the wire truncates it: retail's
        // (2799, 22, 43) reads short -161, not the -160 of -0.88 degrees
        // (cod11-combat.md 13.6).
        let angles = vcod_server::game::builtins::math::vectoangles(first.delta);
        assert_eq!(
            first.apos.map(f32::to_bits),
            angles.map(|a| wire_angle(a).to_bits()),
            "apos {:?} off vectoangles {angles:?}",
            first.apos
        );
        let rows = on
            .iter()
            .map(|(t, _, f)| {
                (
                    t - fire,
                    f.tr_type,
                    f.tr_time - fire,
                    f.base.map(|v| (v * 10.0).round() as i32),
                )
            })
            .collect();
        (rows, first)
    };
    let carrier = |run: &Run| {
        let x = run.first(EV_EXPLODE);
        (x.ev.e_type, x.ev.weapon, x.ev.scale, x.ev.eflags)
    };
    let (r_rows, r_flight) = flight(&retail);
    let (o_rows, o_flight) = flight(&ours);
    // The flight's rows up to the shorter one's end: where it goes off
    // rides the draw.
    let n = r_rows.len().min(o_rows.len());
    assert_eq!(r_rows[..n], o_rows[..n], "the flight");
    assert_eq!(carrier(&retail), carrier(&ours), "the explode's carrier");
    // Each side's explode on its own line, on the first snapshot at or past
    // the frame the rocket reaches it, the rocket gone with it.
    for (side, run, f) in [("retail", &retail, r_flight), ("ours", &ours, o_flight)] {
        let x = run.first(EV_EXPLODE);
        let (base, delta) = (glam::Vec3::from(f.base), glam::Vec3::from(f.delta));
        let pos = glam::Vec3::from(x.ev.pos);
        let along = (pos - base).dot(delta.normalize());
        let off = (pos - base - delta.normalize() * along).length();
        assert!(off < 2.0, "{side}: the explode {pos} is {off} off the line");
        let reach = f.tr_time + (along / delta.length() * 1000.0).ceil() as i32;
        assert!(
            (reach..reach + common::FRAME_MS as i32).contains(&x.t),
            "{side}: reaches {pos} at {reach}, goes off at {}",
            x.t
        );
        let fire = run.first(EV_FIRE_WEAPON_LASTSHOT).t;
        let rows = run.missiles.iter().filter(|(t, ..)| *t >= fire).count();
        assert_eq!(
            rows as i32,
            (x.t - fire) / common::FRAME_MS as i32,
            "{side}"
        );
    }
    let world = cassino_world();
    let (base, delta) = (
        glam::Vec3::from(r_flight.base),
        glam::Vec3::from(r_flight.delta),
    );
    let t = world.point_trace(
        base,
        base + delta * 2.0,
        vcod_common::collision::MASK_BLAST,
        true,
    );
    let x = retail.first(EV_EXPLODE);
    assert!(
        (t.endpos - glam::Vec3::from(x.ev.pos)).length() < 2.0,
        "retail's line meets our world at {}, retail's rocket went off at {:?}",
        t.endpos,
        x.ev.pos
    );
    assert_eq!(
        vcod_common::collision::sound_material(t.surface_flags),
        x.ev.surf_type,
        "{}",
        t.hit.map(|h| world.describe(h)).unwrap_or_default()
    );
}

/// The bazooka round of `*-uo-bazooka-draw.bin` (2026-10-21, the
/// `*-uo-bazooka.bin` recipe on port 34531 with the server under gdb):
/// ours fires it from retail's spot along retail's view with the
/// `gunrandom` pair gdb read off retail's `Weapon_RocketLauncher_Fire`
/// (x 0xbea62115, y 0xbf0d3c83), and its `trBase`, `trDelta` and `apos`
/// go out on retail's bits (combat doc 13.6).
#[test]
fn the_bazooka_rocket_leaves_on_retails_bits() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (retail, origin, yaw) = retail("bazooka-draw");
    let p = &PROTOCOL_V22;
    let frames = vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-uo-bazooka-draw.bin",
        "net/snapshots-uo-bazooka-draw.bin",
    );
    let fire_t = retail.first(EV_FIRE_WEAPON_LASTSHOT).t;
    let view = frames
        .iter()
        .map(|f| &f.snapshot)
        .take_while(|s| s.server_time < fire_t)
        .last()
        .expect("a snapshot before the fire")
        .ps
        .viewangles(p);
    let aim = [view[0], view[1] - yaw];
    let script = [
        foot(5000, 0, aim),
        foot(150, BUTTON_ATTACK, aim),
        foot(10_000, 0, aim),
    ];
    let draw = (f32::from_bits(0xbea62115), f32::from_bits(0xbf0d3c83));
    let ours = ours_with("bazooka_mp", &script, origin, yaw, 8, |sv| {
        sv.pin_spread_draws(vec![draw])
    });
    let first = |run: &Run| {
        let fire = run.first(EV_FIRE_WEAPON_LASTSHOT).t;
        let f = run
            .missiles
            .iter()
            .find(|(t, ..)| *t >= fire)
            .expect("the rocket on a snapshot")
            .2;
        (
            f.base.map(f32::to_bits),
            f.delta.map(f32::to_bits),
            f.apos.map(f32::to_bits),
        )
    };
    let r = first(&retail);
    // gdb's `fire_rocket` stores (base, delta) for the same round.
    assert_eq!(
        (r.0, r.1),
        (
            [0xc42bb959, 0xc491015f, 0x4304af98],
            [0x452e7000, 0x42de0000, 0xc33d0000]
        ),
        "the capture's round is gdb's"
    );
    assert_eq!(first(&ours), r);
}

/// `client-probes/probe_satchelshot` on ours, its client driven as the
/// retail capture's `--probe-uo-weapon direct` probe was: every cmd at the
/// newest snapshot's view (the gsc's), its buttons from the `PROBE_IN`
/// lines. The script's log and every event the probe's snapshots carried.
fn ours_direct(probe: &str, src: &str, fixture: &str, secs: i64) -> (Vec<String>, Run) {
    let fs = vcod_common::testing::uo_fs().unwrap();
    let p: &'static Protocol = &PROTOCOL_V22;
    let bsp_path = fs.resolve_map(MAP).expect("the map in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, probe), now);
    sv.set_protocol(p);
    sv.overlay_script(&format!("maps/mp/gametypes/{probe}"), src);
    // The recipe's `+set`s.
    for l in fixture.lines().filter(|l| l.starts_with('#')) {
        let mut t = l.split_whitespace();
        while let Some(w) = t.next() {
            if w == "+set"
                && let (Some(name), Some(value)) = (t.next(), t.next())
            {
                sv.set_cvar(name, value);
            }
        }
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl =
        vcod_common::net::NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = common::Join::new("allies", "thompson_mp");
    let mut hold = common::ProbeHold {
        aim: true,
        ..Default::default()
    };
    let (mut buttons, mut selected) = (0u8, None::<u8>);
    let mut tracker = EventTracker::new();
    let mut out = Run::default();
    let mut seen_time = 0;
    for _ in 0..secs * 1000 / common::FRAME_MS {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += Duration::from_millis(common::FRAME_MS as u64);
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(p, "pm_type") == 0);
        if live {
            let mut cmds = hold.cmds(&cl, buttons);
            if let Some(w) = selected {
                for c in &mut cmds {
                    c.weapon = w;
                }
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&vcod_common::net::msg::UserCmd::default()) {
            hold.last = c.server_time;
        }
        for e in common::step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                match tokens.first().map(String::as_str) {
                    Some("a") => selected = tokens.get(1).and_then(|w| w.parse().ok()),
                    Some("f") => {
                        if let Some(b) = tokens
                            .get(1)
                            .and_then(|l| l.strip_prefix("PROBE_IN "))
                            .and_then(|l| l.split_whitespace().next())
                            .and_then(|b| b.parse().ok())
                        {
                            buttons = b;
                        }
                    }
                    _ => {}
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest()
            && s.server_time != seen_time
        {
            seen_time = s.server_time;
            out.record(&mut tracker, s, p);
        }
    }
    (sv.script_log().to_vec(), out)
}

/// The first `PROBE <what> <time> ...` line's time.
fn probe_time(log: &[String], what: &str) -> Option<i32> {
    let tag = format!("PROBE {what} ");
    log.iter()
        .find_map(|l| l.trim().strip_prefix(tag.as_str()))
        .and_then(|l| l.split(' ').next())
        .and_then(|t| t.parse().ok())
}

/// One `probe_satchelshot` capture against ours: every explode's time
/// after the shot (when there is one) and after the first throw, its
/// carrier's `eType`, parm, type, `eFlags`, surface and origin.
#[allow(clippy::type_complexity)]
fn satchel_explodes(
    tag: &str,
    fixture: &str,
) -> (
    Vec<(Option<i32>, i32, i32, i32, i32, i32, i32, [i32; 3])>,
    Vec<(Option<i32>, i32, i32, i32, i32, i32, i32, [i32; 3])>,
) {
    let log: Vec<String> = fixture.lines().map(str::to_string).collect();
    let (retail, ..) = retail(tag);
    let src =
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_satchelshot.gsc");
    let (ours_log, ours) = ours_direct("probe_satchelshot", src, fixture, 60);
    report("retail", &retail);
    report("ours", &ours);
    if std::env::var_os("UOWEAP_REPORT").is_some() {
        for l in ours_log.iter().filter(|l| !l.contains("PROBE miss")) {
            print!("ours log {l}");
        }
    }
    let shape = |log: &[String], run: &Run| {
        let throw = probe_time(log, "throw").expect("a throw");
        let fire = probe_time(log, "fire");
        run.of(EV_EXPLODE)
            .iter()
            .map(|x| {
                (
                    fire.map(|f| x.t - f),
                    x.t - throw,
                    x.ev.e_type,
                    x.ev.parm,
                    x.ev.scale,
                    x.ev.eflags,
                    x.ev.surf_type,
                    x.ev.pos.map(|v| v.round() as i32),
                )
            })
            .collect::<Vec<_>>()
    };
    (shape(&log, &retail), shape(&ours_log, &ours))
}

/// Two satchels on mp_cassino and a kar98k round into the second
/// (`*-uo-satchelshot.bin`, `client-probes/probe_satchelshot`, recipe in
/// `fixtures/missiles/mp_cassino-satchelshot-uo.txt`): the round's
/// `G_Damage` takes the satchel's 1 health and `G_GrenadeDie` brings its
/// explode 100 ms on, the snapshot after the shot's. Its blast leaves the
/// first satchel, 225 units off, alone: that one goes off on its own fuse,
/// 6 s after its throw (coduo-diff.md 21).
#[test]
fn a_shot_satchel_goes_off_100_ms_on() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (r, o) = satchel_explodes(
        "satchelshot",
        include_str!("fixtures/missiles/mp_cassino-satchelshot-uo.txt"),
    );
    assert_eq!(r.len(), 2, "retail {r:?}");
    assert_eq!(r[0].0, Some(100), "retail: the shot one goes off 100 ms on");
    // The second throw's view rides the cmd timing: two retail runs rested
    // the shot satchel at y -169.92 and -170.02, which truncate apart.
    let near = |a: [i32; 3], b: [i32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() <= 1);
    assert_eq!(r.len(), o.len(), "retail {r:?}\nours {o:?}");
    for (a, b) in r.iter().zip(&o) {
        assert_eq!(
            (a.0, a.1, a.2, a.3, a.4, a.5, a.6),
            (b.0, b.1, b.2, b.3, b.4, b.5, b.6),
            "(since the shot, since the first throw, eType, parm, type, eFlags, surface)"
        );
        assert!(near(a.7, b.7), "retail {r:?}\nours {o:?}");
    }
}

/// Two satchels 137 units apart, no shot (`*-uo-satchelchain.bin`): the
/// first goes off on its fuse and its blast sets the second off 100 ms on;
/// the second's blast reaches the first inside the 300 ms its explode rides
/// the wire, and the first does not go off again (`freeAfterEvent` keeps
/// its think from running, coduo-diff.md 21).
#[test]
fn a_satchel_blast_sets_off_the_next_and_none_goes_off_twice() {
    if vcod_common::testing::uo_fs().is_none() {
        return;
    }
    let (r, o) = satchel_explodes(
        "satchelchain",
        include_str!("fixtures/missiles/mp_cassino-satchelchain-uo.txt"),
    );
    assert_eq!(r.len(), 2, "retail {r:?}");
    assert_eq!(r[1].1 - r[0].1, 100, "retail: the chain's 100 ms");
    assert_eq!(
        r, o,
        "(since the shot, since the first throw, eType, parm, type, eFlags, surface, origin)"
    );
}
