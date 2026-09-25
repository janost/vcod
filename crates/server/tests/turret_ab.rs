//! The two-client rig for the mounted MG on carentan: an allied gunner
//! placed 40 units behind the gun and an axis target 300 units in front of
//! it, both by `probe_turret.gsc`'s `watch_teleports` under
//! `probe_teleport 1`. Tasks 8-11 add their own tests against this rig as
//! the mount, the arc clamp, the fire path and the release each land; task
//! 13 replays a retail fixture on top of it.
//!
//! Stock turret facts (global constraints): `mg42_bipod_stand_mp`, arcs
//! ±45 yaw and -40..+40 pitch, 60 damage, 50 ms `fireTime`, stance 0
//! (stand), hint 6 (`HINT_MG42`).
//!
//! Needs `COD_DIR`; without the paks the smoke test returns early.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{holding, ClientEnd, Queues, CMD_MS};
use vcod_common::net::msg::{UserCmd, BUTTON_ATTACK, BUTTON_USE};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::NetClient;
use vcod_common::pmove::aim::angle_subtract;
use vcod_server::Server;

const MAP: &str = "mp_carentan";
const PROBE_PATH: &str = "maps/mp/gametypes/probe_turret";
const PROBE_SRC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../gsc/tests/fixtures/semantics/client-probes/probe_turret.gsc"
);
/// `misc_mg42`'s wire `eType`.
const ET_MG42: i32 = 11;
/// `docs/research/cod11-events-and-fx.md` section 1.
const EV_FIRE_WEAPON_MG42: i32 = 168;
const EV_SOUND_ALIAS: i32 = 172;
/// The spot the probe script places its clients around (its own comment).
const GUN_NEAR: [f32; 2] = [1712.0, 1830.0];

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn dist_xy(o: [f32; 3], at: [f32; 2]) -> f32 {
    let dx = o[0] - at[0];
    let dy = o[1] - at[1];
    (dx * dx + dy * dy).sqrt()
}

/// `ANGLE2SHORT`: the wire's 16-bit angle, masked the way a usercmd carries
/// it.
fn deg_to_short(deg: f32) -> i32 {
    (deg * 65536.0 / 360.0) as i32 & 0xffff
}

/// As [`deg_to_short`], rounded: a view read back off the wire is already a
/// short, and truncating it would walk the view down a short per frame.
fn deg_to_short_rounded(deg: f32) -> i32 {
    (deg * 65536.0 / 360.0).round() as i32 & 0xffff
}

struct Rig {
    sv: Server,
    qg: Rc<RefCell<Queues>>,
    qt: Rc<RefCell<Queues>>,
    now: Instant,
    /// Allies, placed behind the gun by `probe_turret`.
    gunner: NetClient<ClientEnd>,
    /// Axis, placed 300 units in front, facing back at the gunner.
    target: NetClient<ClientEnd>,
    /// The turret's own entity number, read off the gunner's snapshot once
    /// the placement has settled.
    gun: u32,
}

// Tasks 8-11 read the fields the smoke test does not.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
struct Sample {
    viewlocked: i32,
    viewlocked_ent: i32,
    e_flags: i32,
    pm_type: i32,
    legs_anim: i32,
    hint: i32,
    origin: [f32; 3],
    viewangles: [f32; 3],
    turret_angles2: [f32; 3],
    turret_loop: i32,
    turret_e_flags: i32,
    /// The gun's own ring, oldest first, as far as `eventSequence` has filled it.
    turret_events: Vec<i32>,
}

/// `probe_turret` as the gametype, both clients through the stock menus,
/// then held until the script's teleport has placed them and settled.
fn rig_with(cvars: &[(&str, &str)]) -> Option<Rig> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = Server::new(common::cfg(MAP, "probe_turret"), now);
    sv.overlay_script(PROBE_PATH, &read(PROBE_SRC));
    sv.set_cvar("probe_teleport", "1");
    for (name, value) in cvars {
        sv.set_cvar(name, value);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    let qg = Rc::new(RefCell::new(Queues::default()));
    let qt = Rc::new(RefCell::new(Queues::default()));
    let (gunner, target) = common::join_pair(
        &mut sv,
        &qg,
        &qt,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("axis", "kar98k_mp"),
    );
    let mut rig = Rig {
        sv,
        qg,
        qt,
        now,
        gunner,
        target,
        gun: 0,
    };
    for _ in 0..40 {
        let h = rig.still();
        rig.frame([h, h]);
    }
    assert!(
        rig.sv.script_aborts().is_empty(),
        "{:?}",
        rig.sv.script_aborts()
    );
    let p = &PROTOCOL_V1;
    let gun = rig
        .gunner
        .snapshots()
        .newest()
        .expect("a snapshot once the placement has settled")
        .entities
        .iter()
        .filter(|(_, e)| e.field_i32(p, "eType") == ET_MG42)
        .min_by(|(_, a), (_, b)| {
            dist_xy(a.origin(p), GUN_NEAR)
                .partial_cmp(&dist_xy(b.origin(p), GUN_NEAR))
                .unwrap()
        })
        .map(|(&n, _)| n)
        .expect("the turret entity in the gunner's snapshot");
    rig.gun = gun;
    // The joins' `holding` cmds turned the view back to yaw 0 after the
    // script's `setPlayerAngles`; face the gun the way the retail probe did.
    let (me, at) = (rig.sample().origin, rig.gun_origin());
    let yaw = (at[1] - me[1]).atan2(at[0] - me[0]).to_degrees();
    rig.look([0.0, yaw]);
    rig.hold(1);
    Some(rig)
}

// Tasks 8-11 fill in the tests that use `hold`, `tap`, `look` and `command`;
// this rig lands ahead of them, so the impl carries dead code until then.
#[allow(dead_code)]
impl Rig {
    /// One server frame: two gunner cmds (`CMD_MS` apart), the target
    /// holding.
    fn frame(&mut self, cmds: [UserCmd; 2]) -> Sample {
        for cmd in cmds {
            self.now += Duration::from_millis(CMD_MS as u64);
            self.gunner.pump_at(self.now);
            self.gunner.send_frame(&cmd);
        }
        self.target.pump_at(self.now);
        self.target.send_frame(&holding(&self.target));
        common::step_pair(
            &mut self.sv,
            (&self.qg, &mut self.gunner),
            (&self.qt, &mut self.target),
            self.now,
        );
        self.sample()
    }

    /// The gunner's `holding` cmd with the mouse left where it is: the
    /// angles are the newest snapshot's view, since `holding`'s zero angles
    /// are an absolute turn to yaw 0 and would undo `setPlayerAngles` and a
    /// mount's view snap.
    fn still(&self) -> UserCmd {
        let mut h = holding(&self.gunner);
        if let Some(s) = self.gunner.snapshots().newest() {
            let v = s.ps.viewangles(&PROTOCOL_V1);
            h.angles = [deg_to_short_rounded(v[0]), deg_to_short_rounded(v[1]), 0];
        }
        h
    }

    /// `n` frames of [`Rig::still`] cmds.
    fn hold(&mut self, n: usize) -> Sample {
        let mut s = Sample::default();
        for _ in 0..n {
            let h = self.still();
            s = self.frame([h, h]);
        }
        s
    }

    /// One frame whose first cmd adds `buttons`, then one holding frame.
    fn tap(&mut self, buttons: u8) -> Sample {
        let h = self.still();
        let tapped = UserCmd {
            buttons: h.buttons | buttons,
            ..h
        };
        self.frame([tapped, h]);
        self.hold(1)
    }

    /// The gunner's view turned to `angles` (absolute, degrees) for one
    /// frame; `hold` and `tap` keep it. `NetClient::send_frame` already subtracts `delta_angles`
    /// before it puts a cmd on the wire, so this passes the absolute shorts
    /// and lets it.
    fn look(&mut self, angles: [f32; 2]) -> Sample {
        let mut h = holding(&self.gunner);
        h.angles = [deg_to_short(angles[0]), deg_to_short(angles[1]), 0];
        self.frame([h, h])
    }

    /// A reliable client command from the gunner, then two frames.
    fn command(&mut self, cmd: &str) -> Sample {
        self.gunner.send_reliable(cmd);
        self.hold(2)
    }

    /// The turret's `pos.trBase` from the gunner's newest snapshot.
    fn gun_origin(&self) -> [f32; 3] {
        let p = &PROTOCOL_V1;
        self.gunner
            .snapshots()
            .newest()
            .and_then(|s| s.entities.get(&self.gun))
            .map_or([0.0; 3], |e| e.origin(p))
    }

    /// The turret's `apos.trBase[1]` from the gunner's newest snapshot.
    fn gun_yaw(&self) -> f32 {
        let p = &PROTOCOL_V1;
        self.gunner
            .snapshots()
            .newest()
            .and_then(|s| s.entities.get(&self.gun))
            .map_or(0.0, |e| e.field_f32(p, "apos.trBase[1]"))
    }

    fn sample(&self) -> Sample {
        let p = &PROTOCOL_V1;
        let Some(s) = self.gunner.snapshots().newest() else {
            return Sample::default();
        };
        let ps = |name: &str| s.ps.field_i32(p, name);
        let gun = s.entities.get(&self.gun);
        Sample {
            viewlocked: ps("viewlocked"),
            viewlocked_ent: ps("viewlocked_entNum"),
            e_flags: ps("eFlags"),
            pm_type: ps("pm_type"),
            legs_anim: ps("legsAnim"),
            hint: ps("serverCursorHint"),
            origin: s.ps.origin(p),
            viewangles: s.ps.viewangles(p),
            turret_angles2: [0, 1, 2]
                .map(|i| gun.map_or(0.0, |e| e.field_f32(p, &format!("angles2[{i}]")))),
            turret_loop: gun.map_or(0, |e| e.field_i32(p, "loopSound")),
            turret_e_flags: gun.map_or(0, |e| e.field_i32(p, "eFlags")),
            turret_events: gun.map_or(Vec::new(), |e| {
                let seq = e.field_i32(p, "eventSequence");
                ((seq - 4).max(0)..seq)
                    .map(|i| e.field_i32(p, &format!("events[{}]", i & 3)))
                    .collect()
            }),
        }
    }
}

/// Pins the rig itself: the gunner spawns unmounted, within 60 units of the
/// gun the script placed it 40 units behind.
#[test]
fn the_rig_places_the_gunner_behind_the_gun() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    let s = rig.hold(1);
    assert_eq!(s.viewlocked, 0);
    assert_eq!(s.pm_type, 0);
    let gun = rig.gun_origin();
    let d = ((s.origin[0] - gun[0]).powi(2) + (s.origin[1] - gun[1]).powi(2)).sqrt();
    assert!(d < 60.0, "gunner {:?} gun {:?}", s.origin, gun);
}

/// Behind the gun and facing it the hint is `HINT_MG42`; the use key mounts
/// it, and the mounted frames read the lock, the stand bits and no hint
/// (`docs/research/cod11-turrets.md` 12.1 and 12.10).
#[test]
fn standing_behind_the_gun_shows_hint_mg42_and_use_mounts_it() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    assert_eq!(rig.hold(1).hint, 6);
    rig.tap(BUTTON_USE);
    let s = rig.hold(2);
    assert_eq!(s.viewlocked, 1);
    assert_eq!(s.viewlocked_ent, rig.gun as i32);
    assert_eq!(s.e_flags & 0xC000, 0xC000);
    assert_eq!(s.hint, 0);
}

/// A view turned past the arc drags the barrel to the edge, 15 degrees a
/// frame, and holds it and the view there (`docs/research/cod11-turrets.md`
/// 6.2 and 12.3).
#[test]
fn a_manned_gun_follows_the_view_and_stops_at_the_edge() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    let yaw = rig.gun_yaw();
    for _ in 0..10 {
        rig.look([0.0, yaw + 90.0]);
    }
    let s = rig.hold(1);
    assert!(
        (s.turret_angles2[1] - 45.0).abs() < 0.01,
        "{:?}",
        s.turret_angles2
    );
    assert!(
        (angle_subtract(s.viewangles[1], yaw) - 45.0).abs() < 0.1,
        "view {:?} gun yaw {yaw}",
        s.viewangles
    );
}

/// A held trigger fires on every frame, the frame reads `viewlocked` 2 and
/// 0x400 on both the gun and the gunner, and the loop stays one frame past
/// the last shot before the cooldown alias ends it (turrets doc 6.3, 6.4 and
/// 12.4).
#[test]
fn holding_attack_fires_and_the_loop_sound_follows() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    let h = rig.still();
    let fire = UserCmd {
        buttons: h.buttons | BUTTON_ATTACK,
        ..h
    };
    let s = rig.frame([fire, fire]);
    assert_ne!(s.turret_loop, 0);
    assert_eq!(s.turret_e_flags & 0x400, 0x400);
    assert_eq!(s.e_flags & 0x400, 0x400);
    assert_eq!(s.viewlocked, 2);
    assert_eq!(s.turret_events.last(), Some(&EV_FIRE_WEAPON_MG42));
    let s = rig.frame([fire, fire]);
    assert_eq!(s.viewlocked, 2, "a second shot on the next frame");
    let s = rig.hold(1);
    assert_ne!(
        s.turret_loop, 0,
        "the loop outlasts the last shot by a frame"
    );
    assert_eq!(
        (s.viewlocked, s.e_flags & 0x400, s.turret_e_flags & 0x400),
        (1, 0, 0)
    );
    let s = rig.hold(1);
    assert_eq!(s.turret_loop, 0);
    assert_eq!(s.turret_events.last(), Some(&EV_SOUND_ALIAS));
    let s = rig.hold(5);
    assert_eq!(s.turret_loop, 0);
}

/// A round on the target takes its health on the snapshot of the frame it
/// was fired on, not the next one (turrets doc 12.5).
#[test]
fn a_round_on_the_target_lands_in_the_frame_it_was_fired() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    let p = &PROTOCOL_V1;
    let gun = rig.gun_origin();
    let target = rig
        .target
        .snapshots()
        .newest()
        .map(|s| s.ps.origin(p))
        .expect("the target's snapshot");
    // From about `tag_player`'s height to the target's chest.
    let (dx, dy, dz) = (
        target[0] - gun[0],
        target[1] - gun[1],
        target[2] + 40.0 - (gun[2] + 21.0),
    );
    let yaw = dy.atan2(dx).to_degrees();
    let pitch = -dz.atan2(dx.hypot(dy)).to_degrees();
    for _ in 0..4 {
        rig.look([pitch, yaw]);
    }
    let before = rig.target.snapshots().newest().unwrap().ps.health();
    assert_eq!(before, 100);
    let h = rig.still();
    let fire = UserCmd {
        buttons: h.buttons | BUTTON_ATTACK,
        ..h
    };
    let s = rig.frame([fire, fire]);
    assert_eq!(s.viewlocked, 2);
    let after = rig.target.snapshots().newest().unwrap().ps.health();
    assert!(after < before, "health {after} on the shot's own snapshot");
}
