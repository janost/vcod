//! The two-client rig for the mounted MG on carentan: an allied gunner
//! placed 40 units behind the gun and an axis target 300 units in front of
//! it, both by `probe_turret.gsc`'s `watch_teleports` under
//! `probe_teleport 1`. Tasks 8-11 add their own tests against this rig as
//! the mount, the arc clamp, the fire path and the release each land; task
//! 13 replays a retail fixture on top of it (the gate at the end of this
//! file).
//!
//! Stock turret facts (global constraints): `mg42_bipod_stand_mp`, arcs
//! ±45 yaw and -40..+40 pitch, 60 damage, 50 ms `fireTime`, stance 0
//! (stand), hint 6 (`HINT_MG42`).
//!
//! Needs `COD_DIR`; without the paks the smoke test returns early.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{holding, ClientEnd, Queues, CMD_MS, FRAME_MS};
use vcod_common::animtree::PlayerAnims;
use vcod_common::net::events::EventTracker;
use vcod_common::net::msg::{UserCmd, BUTTON_ATTACK, BUTTON_USE, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::NetClient;
use vcod_common::pmove::aim::angle_subtract;
use vcod_server::Server;

/// `animscript.rs:494`.
const ANIM_TOGGLEBIT: i32 = 512;

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
const EV_BULLET_HIT_SMALL: i32 = 173;
const EV_BULLET_HIT_LARGE: i32 = 174;
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
    /// The gunner joined second, on `ADDR_B`, and is client 1, as in the
    /// retail capture; otherwise it is client 0 on `ADDR`.
    gunner_second: bool,
    /// The gunner's snapshot events, drained once per frame.
    events: EventTracker,
    /// The turret's own entity number, read off the gunner's snapshot once
    /// the placement has settled.
    gun: u32,
}

/// One drained event: the id, its parm and whose it was (see [`who`]).
type Ev = (i32, i32, String);

/// One snapshot of the gunner, ours off its client and retail's off a
/// `!trace` line and the `!turret`, `!event` and `!impact` lines after it.
#[derive(Clone, Debug, Default)]
struct Sample {
    /// Retail's `serverTime`; ours carries the one it pairs with.
    t: i32,
    viewlocked: i32,
    viewlocked_ent: i32,
    e_flags: i32,
    pm_type: i32,
    pm_flags: i32,
    ground: i32,
    legs_anim: i32,
    torso_anim: i32,
    hint: i32,
    hint_val: i32,
    hint_string: i32,
    gunfx: i32,
    weapon: i32,
    event_sequence: i32,
    origin: [f32; 3],
    viewangles: [f32; 3],
    delta_angles: [i32; 3],
    turret_angles2: [f32; 3],
    turret_loop: i32,
    turret_e_flags: i32,
    turret_other: i32,
    turret_event_seq: i32,
    /// The gun's own ring, oldest first, as far as `eventSequence` has filled it.
    turret_events: Vec<i32>,
    /// The gun's four ring slots and parms as they sit.
    turret_ring: [i32; 4],
    turret_parms: [i32; 4],
    /// Every event drained off this snapshot.
    events: Vec<Ev>,
    /// Every `EV_BULLET_HIT_SMALL`/`LARGE` temp entity: the event and origin.
    impacts: Vec<(i32, [f32; 3])>,
}

/// `probe_turret` as the gametype, both clients through the stock menus,
/// then held until the script's teleport has placed them and settled.
fn rig_with(cvars: &[(&str, &str)]) -> Option<Rig> {
    build(cvars, "m1carbine_mp", false)
}

/// The rig with `cvars` set and the gunner joining with `weapon`; with
/// `gunner_second` the target connects first and takes client 0.
fn build(cvars: &[(&str, &str)], weapon: &str, gunner_second: bool) -> Option<Rig> {
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
    let (gunner, target) = if gunner_second {
        let (t, g) = common::join_pair(
            &mut sv,
            &qt,
            &qg,
            &mut now,
            ("axis", "kar98k_mp"),
            ("allies", weapon),
        );
        (g, t)
    } else {
        common::join_pair(
            &mut sv,
            &qg,
            &qt,
            &mut now,
            ("allies", weapon),
            ("axis", "kar98k_mp"),
        )
    };
    let mut rig = Rig {
        sv,
        qg,
        qt,
        now,
        gunner,
        target,
        gunner_second,
        events: EventTracker::new(),
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

impl Rig {
    /// One server frame: two gunner cmds (`CMD_MS` apart), the target
    /// holding.
    fn frame(&mut self, cmds: [UserCmd; 2]) -> Sample {
        self.frame_at(&[(CMD_MS, cmds[0]), (2 * CMD_MS, cmds[1])])
    }

    /// One server frame whose gunner cmds go out `ms` into it, each stamped
    /// that far past the last snapshot; the target holds.
    fn frame_at(&mut self, cmds: &[(i64, UserCmd)]) -> Sample {
        let start = self.now;
        for &(ms, cmd) in cmds {
            self.now = start + Duration::from_millis(ms as u64);
            self.gunner.pump_at(self.now);
            self.gunner.send_frame(&cmd);
        }
        self.now = start + Duration::from_millis(FRAME_MS as u64);
        self.target.pump_at(self.now);
        self.target.send_frame(&holding(&self.target));
        if self.gunner_second {
            common::step_pair(
                &mut self.sv,
                (&self.qt, &mut self.target),
                (&self.qg, &mut self.gunner),
                self.now,
            );
        } else {
            common::step_pair(
                &mut self.sv,
                (&self.qg, &mut self.gunner),
                (&self.qt, &mut self.target),
                self.now,
            );
        }
        let mut s = self.sample();
        if let Some(snap) = self.gunner.snapshots().newest() {
            for e in self.events.drain(snap, &PROTOCOL_V1) {
                if e.event == EV_BULLET_HIT_SMALL || e.event == EV_BULLET_HIT_LARGE {
                    s.impacts.push((e.event, e.pos));
                }
                s.events
                    .push((e.event, e.parm, who(e.entity_num as i64, self.gun)));
            }
        }
        s
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
        let g = |name: &str| gun.map_or(0, |e| e.field_i32(p, name));
        Sample {
            t: s.server_time,
            viewlocked: ps("viewlocked"),
            viewlocked_ent: ps("viewlocked_entNum"),
            e_flags: ps("eFlags"),
            pm_type: ps("pm_type"),
            pm_flags: ps("pm_flags"),
            ground: ps("groundEntityNum"),
            legs_anim: ps("legsAnim"),
            torso_anim: ps("torsoAnim"),
            hint: ps("serverCursorHint"),
            hint_val: ps("serverCursorHintVal"),
            hint_string: ps("serverCursorHintString"),
            gunfx: ps("gunfx"),
            weapon: ps("weapon"),
            event_sequence: ps("eventSequence"),
            origin: s.ps.origin(p),
            viewangles: s.ps.viewangles(p),
            delta_angles: [0, 1, 2].map(|i| ps(&format!("delta_angles[{i}]"))),
            turret_angles2: [0, 1, 2]
                .map(|i| gun.map_or(0.0, |e| e.field_f32(p, &format!("angles2[{i}]")))),
            turret_loop: g("loopSound"),
            turret_e_flags: g("eFlags"),
            turret_other: g("otherEntityNum"),
            turret_event_seq: g("eventSequence"),
            turret_events: gun.map_or(Vec::new(), |e| {
                let seq = e.field_i32(p, "eventSequence");
                ((seq - 4).max(0)..seq)
                    .map(|i| e.field_i32(p, &format!("events[{}]", i & 3)))
                    .collect()
            }),
            turret_ring: [0, 1, 2, 3].map(|i| g(&format!("events[{i}]"))),
            turret_parms: [0, 1, 2, 3].map(|i| g(&format!("eventParms[{i}]"))),
            events: Vec::new(),
            impacts: Vec::new(),
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

/// The gun's `angles2` as the target's newest snapshot carries it: the
/// gunner's own view is gone once it disconnects.
fn target_gun_angles2(rig: &Rig) -> Option<[f32; 3]> {
    let p = &PROTOCOL_V1;
    let gun = rig
        .target
        .snapshots()
        .newest()?
        .entities
        .get(&rig.gun)?
        .clone();
    Some([0, 1, 2].map(|i| gun.field_f32(p, &format!("angles2[{i}]"))))
}

/// The gunner's use press on the gun lets go in the frame of the use cmd:
/// back a unit above where it stood to mount, the lock and the stand bits
/// gone, `viewlocked_entNum` 1023 (turrets doc 12.7); the barrel keeps its
/// place on that snapshot and walks home 10 degrees a frame after it (12.8).
#[test]
fn use_on_the_gun_lets_go_and_the_barrel_walks_home() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    let before = rig.hold(1).origin;
    rig.tap(BUTTON_USE);
    let yaw = rig.gun_yaw();
    for _ in 0..3 {
        rig.look([0.0, yaw + 30.0]);
    }
    let held = rig.hold(1);
    assert_eq!(held.viewlocked, 1);
    let h = rig.still();
    let s = rig.frame([
        UserCmd {
            buttons: h.buttons | BUTTON_USE,
            ..h
        },
        h,
    ]);
    assert_eq!((s.viewlocked, s.viewlocked_ent), (0, 1023));
    assert_eq!(s.e_flags & 0xC000, 0);
    assert!(
        (s.origin[2] - (before[2] + 1.0)).abs() < 0.01
            && (s.origin[0] - before[0]).abs() < 0.01
            && (s.origin[1] - before[1]).abs() < 0.01,
        "released at {:?}, mounted from {before:?}",
        s.origin
    );
    assert_eq!(
        s.turret_angles2[1], held.turret_angles2[1],
        "the release snapshot keeps the barrel"
    );
    let next = rig.hold(1);
    assert!(
        (next.turret_angles2[1] - (held.turret_angles2[1] - 10.0)).abs() < 0.01,
        "{:?} after {:?}",
        next.turret_angles2,
        held.turret_angles2
    );
    let s = rig.hold(20);
    assert_eq!(s.turret_angles2[1], 0.0);
}

/// `turret_think_client`'s `sessionstate` test (turrets doc 8): a gunner
/// killed on the gun lets go that frame, and the gun is free for its next
/// life. The corpse is cloned off a released player, so it carries no
/// mounted bits.
#[test]
fn a_death_releases_the_turret() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    assert_eq!(rig.hold(2).viewlocked, 1);
    // Past the flood window the `u` taps and menu answers opened.
    rig.hold(20);
    let s = rig.command("kill");
    let s = if s.pm_type == 6 { s } else { rig.hold(2) };
    assert_eq!(s.pm_type, 6, "dead");
    assert_eq!(s.viewlocked, 0);
    assert_eq!(s.e_flags & 0xC000, 0);
    assert_eq!(s.turret_loop, 0);
    let p = &PROTOCOL_V1;
    let corpses: Vec<i32> = rig
        .target
        .snapshots()
        .newest()
        .unwrap()
        .entities
        .iter()
        .filter(|(&n, _)| (64..72).contains(&n))
        .map(|(_, e)| e.field_i32(p, "eFlags"))
        .collect();
    assert!(!corpses.is_empty(), "the corpse is in the target's view");
    assert!(corpses.iter().all(|f| f & 0xC000 == 0), "{corpses:?}");
    rig.hold(50); // the death anim and the callback's 2 s wait
                  // `waitRespawnButton` polls `useButtonPressed`, the frame's last cmd,
                  // which a tap has already released.
    let h = rig.still();
    let held = UserCmd {
        buttons: h.buttons | BUTTON_USE,
        ..h
    };
    rig.frame([held, held]);
    let s = rig.hold(40); // the gsc places the gunner again
    assert_eq!(s.pm_type, 0, "respawned");
    rig.tap(BUTTON_USE);
    assert_eq!(
        rig.hold(2).viewlocked,
        1,
        "the gun is free for the next life"
    );
}

/// Both boundaries build the level again, turrets and sims with it: a
/// `map_restart` leaves nobody on the gun.
#[test]
fn a_level_boundary_forgets_every_mount() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    assert_eq!(rig.hold(2).viewlocked, 1);
    rig.sv.push_console("map_restart");
    let s = rig.hold(60);
    assert_eq!(s.viewlocked, 0);
    assert_eq!(s.e_flags & 0xC000, 0);
}

/// `G_FreeEntity` calls `G_FreeTurret` (turrets doc 8): deleting a manned
/// gun lets its gunner go.
#[test]
fn deleting_a_manned_turret_releases_its_gunner() {
    // Counted from map load; the joins, the placement and the mount take
    // about 5.5 s of it.
    let Some(mut rig) = rig_with(&[("probe_delete_after", "10")]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    assert_eq!(rig.hold(2).viewlocked, 1);
    let mut s = rig.hold(1);
    for _ in 0..200 {
        if s.viewlocked == 0 {
            break;
        }
        s = rig.hold(1);
    }
    let gone = rig
        .gunner
        .snapshots()
        .newest()
        .is_some_and(|snap| !snap.entities.contains_key(&rig.gun));
    assert!(gone, "released by the delete, not by anything else");
    assert_eq!(s.viewlocked, 0);
    assert_eq!(s.viewlocked_ent, 1023);
    assert_eq!(s.e_flags & 0xC000, 0);
}

/// A gunner who drops: `G_FreeEntity` on the player clears the gun's owner
/// (turrets doc 8), so the unmanned think takes the barrel home.
#[test]
fn a_gunner_who_disconnects_leaves_the_gun_to_walk_home() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    let yaw = rig.gun_yaw();
    for _ in 0..3 {
        rig.look([0.0, yaw + 30.0]);
    }
    let held = target_gun_angles2(&rig).expect("the gun in the target's view");
    assert!(held[1] > 20.0, "{held:?}");
    rig.sv
        .handle_packet(common::ADDR, b"\xff\xff\xff\xffdisconnect", rig.now);
    rig.hold(10);
    let s = target_gun_angles2(&rig).unwrap();
    assert_eq!(s[1], 0.0, "{s:?}");
}

/// A gunner killed by another gun's round lets go on the frame it dies,
/// though its own `ClientEndFrame` ran before the round was traced, and its
/// corpse lies where it died, not where the release teleports the dead
/// player. Carentan's second gun is out of the first one's arc, so it is
/// moved to put its gunner's body where the target stood, and the target
/// mounts it from 64 units away.
#[test]
fn a_gunner_killed_by_a_turret_round_lets_go_that_frame() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    let p = &PROTOCOL_V1;
    rig.tap(BUTTON_USE);
    let other = rig
        .sv
        .all_entities()
        .into_iter()
        .find(|(n, e)| *n != rig.gun && e.field_i32(p, "eType") == ET_MG42)
        .map(|(n, _)| n)
        .expect("carentan's second gun");
    let stood = rig
        .target
        .snapshots()
        .newest()
        .map(|s| s.ps.origin(p))
        .unwrap();
    // The body sits about 47 units behind a gun and 32 below it (turrets
    // doc 12.2), so the gun goes that far past the target, facing away.
    let gun = rig.gun_origin();
    let away = (stood[1] - gun[1]).atan2(stood[0] - gun[0]);
    let other_at = [
        stood[0] + 47.0 * away.cos(),
        stood[1] + 47.0 * away.sin(),
        stood[2] + 31.9,
    ];
    rig.sv
        .test_place_entity(other, other_at, [0.0, away.to_degrees(), 0.0]);
    let mount_spot = [stood[0], stood[1] + 64.0, stood[2]];
    assert!(rig.sv.test_mount(1, other, mount_spot));
    // The target still faces the first gun, so its barrel swings to the arc
    // and the body with it; four frames settle both.
    rig.hold(4);
    let t = rig.target.snapshots().newest().unwrap();
    assert_eq!(
        t.ps.field_i32(p, "viewlocked"),
        1,
        "the target mans the gun"
    );
    let died_at = t.ps.origin(p);
    assert!(
        dist_xy(died_at, [stood[0], stood[1]]) < 64.0,
        "placed at {died_at:?}, stood at {stood:?}"
    );

    let gun = rig.gun_origin();
    let (dx, dy, dz) = (
        died_at[0] - gun[0],
        died_at[1] - gun[1],
        died_at[2] + 40.0 - (gun[2] + 21.0),
    );
    let yaw = dy.atan2(dx).to_degrees();
    let pitch = -dz.atan2(dx.hypot(dy)).to_degrees();
    for _ in 0..4 {
        rig.look([pitch, yaw]);
    }
    let h = rig.still();
    let fire = UserCmd {
        buttons: h.buttons | BUTTON_ATTACK,
        ..h
    };
    let mut dead = None;
    for _ in 0..20 {
        rig.frame([fire, fire]);
        let t = rig.target.snapshots().newest().unwrap();
        if t.ps.field_i32(p, "pm_type") == 6 {
            dead = Some(t.clone());
            break;
        }
    }
    let t = dead.expect("the rounds kill the target");
    assert_eq!(t.ps.field_i32(p, "viewlocked"), 0);
    assert_eq!(t.ps.field_i32(p, "eFlags") & 0xC000, 0);
    let corpse = rig
        .gunner
        .snapshots()
        .newest()
        .unwrap()
        .entities
        .iter()
        .find(|(&n, e)| (64..72).contains(&n) && e.field_i32(p, "clientNum") == 1)
        .map(|(_, e)| e.clone())
        .expect("the target's corpse in the gunner's view");
    assert_eq!(corpse.field_i32(p, "eFlags") & 0xC000, 0);
    let at = corpse.origin(p);
    assert!(
        dist_xy(at, [died_at[0], died_at[1]]) < 1.0,
        "corpse {at:?}, died at {died_at:?}, mounted from {mount_spot:?}"
    );
}

/// `ClientSpawn` lets go of the gun (turrets doc 8): a live gunner who
/// picks spectator from the team menu is spawned where the spectator spawn
/// is, not teleported back to where it mounted, and the gun is free.
#[test]
fn a_gunner_spawned_as_a_spectator_lets_go() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    let mounted_from = rig.hold(1).origin;
    rig.tap(BUTTON_USE);
    assert_eq!(rig.hold(2).viewlocked, 1);
    let (lo, hi) = vcod_server::configstrings::CsRange::Menu.bounds();
    let team_menu = (lo..=hi)
        .find(|&i| rig.sv.configstring(i).starts_with("team_"))
        .expect("the team menu precached")
        - lo;
    let mr = format!("mr {} {team_menu} spectator", rig.gunner.server_id());
    rig.gunner.send_reliable(&mr);
    let s = rig.hold(3);
    assert_eq!(s.pm_type, 4, "spectating");
    assert_eq!((s.viewlocked, s.e_flags & 0xC000), (0, 0));
    assert!(
        dist_xy(s.origin, [mounted_from[0], mounted_from[1]]) > 64.0,
        "spectator at {:?}, mounted from {mounted_from:?}",
        s.origin
    );
    rig.hold(20);
    assert_eq!(target_gun_angles2(&rig).map(|a| a[1]), Some(0.0));
}

/// A mounted gunner plays the `mounted mg42` clauses off `mp/playeranim.script`:
/// `standMG42_aim` at rest and `standMG42_fire` while the attack bit is held
/// (`docs/research/cod11-turrets.md` 10 and 12.9).
#[test]
fn a_mounted_player_plays_the_mg42_aim_and_fire_clauses() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let anims = PlayerAnims::load(&fs).unwrap();
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    let s = rig.hold(3);
    assert_eq!(
        s.legs_anim & !ANIM_TOGGLEBIT,
        anims.wire_of("standMG42_aim").unwrap()
    );
    let mut fire = rig.still();
    fire.buttons |= BUTTON_ATTACK;
    rig.frame([fire, fire]);
    let s = rig.frame([fire, fire]);
    assert_eq!(
        s.legs_anim & !ANIM_TOGGLEBIT,
        anims.wire_of("standMG42_fire").unwrap()
    );
}

// ------------------------------------------------------------ the retail gate
//
// The retail capture replayed on ours. `turret/mp_carentan-dm-turret.txt` is
// the gunner's side (every cmd, every snapshot, the gun's entity, every drained
// event and impact) and `-turret-script.txt` the server's `D;`/`K;` lines,
// both from one run (`docs/research/cod11-turrets.md` 12). The rig numbers
// the clients as retail did, target 0 and gunner 1, since a victim numbered
// below its gunner takes its pain a frame late (12.5).
//
// Retail's snapshot `T` pairs with our frame that ran the cmds stamped in
// `[T - 50, T)`: a cmd sent on receipt of snapshot `T - 50` carries that
// time and runs in the next frame, and in every snapshot where the asked
// view moves between cmds the view is the last one stamped below `T`. Each
// cmd goes out as the view retail's server built from it, its wire angles
// plus retail's `delta_angles` of snapshot `T - 50`, which is the view ours
// builds too whatever `delta_angles` the join left us.

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/turret/mp_carentan-dm-turret.txt"
);
const SCRIPT_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/turret/mp_carentan-dm-turret-script.txt"
);

/// Cmds that reached retail's server after the frame their stamp puts them
/// in (12.1): the stand mount's use cmd, whose 24750 is still unmounted and
/// 24800 mounted and placed, and 30148, whose pitch clamp at 30150 is built
/// off 30132's wire angles.
const LATE: &[i32] = &[24736, 30148];

/// Rows the diff lets through, each a substring of the rows it excuses and
/// the reason, which `docs/research/cod11-turrets.md` 13 carries too.
const GAPS: &[(&str, &str)] = &[
    ("[mount] ground t=24800", SAME_TICK),
    ("[mount] legs_anim t=24800", SAME_TICK),
    ("[mount] origin t=24800", SAME_TICK),
    (
        "[target] event t=34900: retail only (174, 52, ",
        PASS_THROUGH,
    ),
    (
        "[target] impact t=34900: retail only 174@[1248.0, 1308.0",
        PASS_THROUGH,
    ),
    (
        "[target] event t=34950: retail only (174, 52, ",
        PASS_THROUGH,
    ),
    (
        "[target] impact t=34950: retail only 174@[1248.0, 1312.0",
        PASS_THROUGH,
    ),
    (
        "[target] event t=34950: retail only (187, 47, \"client 0\")",
        END_FRAME,
    ),
    (
        "[target] impact t=34950: retail only 174@[1514.0, 1609.0",
        KILLING_ROUND,
    ),
    ("[target] impact t=34950: ours only 174@[151", KILLING_ROUND),
    ("[uncrouch] ground t=38900", CROUCH_DROP),
    ("[uncrouch] hint t=38900", CROUCH_DROP),
    ("[uncrouch] hint_string t=38900", CROUCH_DROP),
    ("[uncrouch] legs_anim t=38900", CROUCH_DROP),
    ("[strafe] eventSequence t=395", FOOTSTEP),
    ("[strafe] event t=39500: ours only (6, 0, ", FOOTSTEP),
    ("[strafe] event t=39600: retail only (6, 0, ", FOOTSTEP),
    ("[strafe] event t=39800: ", SANDBAG),
    ("[strafe] origin t=398", SANDBAG),
    ("[strafe] origin t=399", SANDBAG),
    ("[strafe] origin t=40000", SANDBAG),
    ("[refused] origin", SANDBAG),
    ("[refused] legs_anim t=40250", SANDBAG),
];

const SAME_TICK: &str = "the cmds after the use cmd in the same tick run unmounted, so the \
    stand mount's first snapshot keeps the stance and the spot it mounted from for a frame; \
    retail does that only on its crouch mount, where the use cmd was the frame's last";
const PASS_THROUGH: &str = "a round stops at the first player it hits: the rifle-bullet \
    pass-through that puts retail's second impact on the world behind the target is not \
    modelled";
const END_FRAME: &str = "entity states are built at snapshot time, and a client dead by then \
    has none: retail copies the victim's state in its own ClientEndFrame, ahead of the \
    gunner's, so the kill snapshot still carries it with the pain the round before raised";
const KILLING_ROUND: &str = "the killing round meets a victim the round before knocked back, \
    and lands about 4 units nearer the gun along the ray than retail's; neither half of the capture \
    carries the victim's origin, so whether the knockback or the pose differs is open";
const CROUCH_DROP: &str = "the crouch release's one-unit drop reads grounded 0.2 above the \
    floor on ours and airborne at the same height on retail, which lands a frame later; the \
    stand release lands on the same frame on both, and the capture holds one of each";
const FOOTSTEP: &str = "footstep phase: bobCycle is not in the capture and the join leaves \
    each side its own, so the strafe's first footstep falls two frames apart";
const SANDBAG: &str = "the strafe slides along the nest wall into the sandbags' 52-degree \
    face at 39800, where retail steps 11 units up it and ours 14, 2.5 units short; unmounted \
    pmove on a steep face, and every origin after it carries the difference";

const ORIGIN_EPS: f32 = 0.25;
/// Retail's fixture prints angles to 0.1.
const PRINT_EPS: f32 = 0.05;
/// One `ANGLE2SHORT` step.
const SHORT_DEG: f32 = 360.0 / 65536.0;
const ANGLES2_EPS: f32 = 0.01;
/// Impact origins reach the wire truncated to whole units, so one step per
/// axis.
const IMPACT_EPS: f32 = 1.0;

fn report() -> bool {
    std::env::var("TURRET_REPORT").is_ok_and(|v| v == "1")
}

/// Whose event it was: the gunner's own ring, the gun, a client, a corpse,
/// or a temp entity, whose number is each server's own free list.
fn who(num: i64, gun: u32) -> String {
    match num {
        n if n == u32::MAX as i64 => "ps".to_string(),
        n if n == gun as i64 => "gun".to_string(),
        0..=63 => format!("client {num}"),
        64..=71 => "body".to_string(),
        _ => "temp".to_string(),
    }
}

fn kv(rest: &str) -> BTreeMap<&str, &str> {
    rest.split_whitespace()
        .filter_map(|t| t.split_once('='))
        .collect()
}

fn floats<const N: usize>(s: &str) -> [f32; N] {
    let mut out = [0.0; N];
    for (i, v) in s.split(',').take(N).enumerate() {
        out[i] = v.parse().unwrap_or_else(|_| panic!("numbers, got {s:?}"));
    }
    out
}

fn ints<const N: usize>(s: &str) -> [i32; N] {
    floats::<N>(s).map(|v| v as i32)
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

struct RetailCmd {
    st: i32,
    /// With the wire angles, as retail's server received them.
    cmd: UserCmd,
}

struct Capture {
    weapon: String,
    gun: u32,
    cmds: Vec<RetailCmd>,
    /// Every snapshot with its phase, `wait` included.
    traces: Vec<(String, Sample)>,
}

fn parse(text: &str) -> Capture {
    let header = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or_else(|| panic!("no {key:?} header"))
    };
    let weapon = kv(header("# gunner "))["weapon"].to_string();
    let gun = header("# turret ")
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .expect("the gun's number in the header");
    let mut phase = String::new();
    let mut cmds = Vec::new();
    let mut traces: Vec<(String, Sample)> = Vec::new();
    // The gun's fields as of the last `!turret`: the line is written only
    // when one moved.
    let mut turret = Sample::default();
    for line in text.lines() {
        if let Some(name) = line
            .strip_prefix("[phase ")
            .and_then(|l| l.strip_suffix(']'))
        {
            phase = name.to_string();
        } else if let Some(rest) = line.strip_prefix("!cmd ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            cmds.push(RetailCmd {
                st: i("st"),
                cmd: UserCmd {
                    buttons: i("buttons") as u8,
                    wbuttons: i("wbuttons") as u8,
                    weapon: i("weapon") as u8,
                    up: i("up") as i8,
                    forward: i("forward") as i8,
                    right: i("right") as i8,
                    angles: ints(m["angles"]),
                    ..NULL_USERCMD
                },
            });
        } else if let Some(rest) = line.strip_prefix("!trace ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            let [hint, hint_val, hint_string] = {
                let h: Vec<i32> = m["hint"].split(':').map(|v| v.parse().unwrap()).collect();
                [h[0], h[1], h[2]]
            };
            let s = Sample {
                t: i("serverTime"),
                viewlocked: i("viewlocked"),
                viewlocked_ent: i("viewlocked_entNum"),
                e_flags: i("eFlags"),
                pm_type: i("pm_type"),
                pm_flags: i("pm_flags"),
                ground: i("groundEntityNum"),
                legs_anim: i("legsAnim"),
                torso_anim: i("torsoAnim"),
                hint,
                hint_val,
                hint_string,
                gunfx: i("gunfx"),
                weapon: i("weapon"),
                event_sequence: i("eventSequence"),
                origin: floats(m["origin"]),
                viewangles: floats(m["viewangles"]),
                delta_angles: ints(m["delta_angles"]),
                ..turret.clone()
            };
            traces.push((phase.clone(), s));
        } else if let Some(rest) = line.strip_prefix("!turret ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            turret.turret_angles2 = floats(m["angles2"]);
            turret.turret_e_flags = i("eFlags");
            turret.turret_loop = i("loopSound");
            turret.turret_other = i("otherEntityNum");
            turret.turret_event_seq = i("eventSequence");
            turret.turret_ring = ints(m["events"]);
            turret.turret_parms = ints(m["eventParms"]);
            let s = &mut traces.last_mut().expect("a !turret after a !trace").1;
            s.turret_angles2 = turret.turret_angles2;
            s.turret_e_flags = turret.turret_e_flags;
            s.turret_loop = turret.turret_loop;
            s.turret_other = turret.turret_other;
            s.turret_event_seq = turret.turret_event_seq;
            s.turret_ring = turret.turret_ring;
            s.turret_parms = turret.turret_parms;
        } else if let Some(rest) = line.strip_prefix("!event ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i64>().unwrap();
            let s = &mut traces.last_mut().expect("an !event after a !trace").1;
            s.events
                .push((i("event") as i32, i("parm") as i32, who(i("entity"), gun)));
        } else if let Some(rest) = line.strip_prefix("!impact ") {
            let m = kv(rest);
            let s = &mut traces.last_mut().expect("an !impact after a !trace").1;
            s.impacts
                .push((m["event"].parse().unwrap(), floats(m["origin"])));
        }
    }
    Capture {
        weapon,
        gun,
        cmds,
        traces,
    }
}

/// The rig as the capture's recipe left retail: the gunner joined with the
/// header's weapon, as client 1, standing where retail's did. Retail's
/// landing from the gsc's placement ended 0.31 units off the spot in y, which
/// no line of the capture explains and ours does not reproduce; every
/// release teleports back to that spot, so ours starts there, to the
/// fixture's print precision (`docs/research/cod11-turrets.md` 13).
fn rig(cap: &Capture) -> Option<Rig> {
    let mut rig = build(&[], &cap.weapon, true)?;
    let at = cap.traces.first().expect("a snapshot").1.origin;
    let mut ours = rig.sample().origin;
    for i in 0..3 {
        if (ours[i] - at[i]).abs() > PRINT_EPS {
            ours[i] = at[i];
        }
    }
    rig.sv.test_set_client_origin(1, ours);
    rig.hold(1);
    Some(rig)
}

/// The retail snapshot a cmd's effect first shows in.
fn frame_of(cap: &Capture, st: i32) -> Option<i32> {
    let t = cap.traces.iter().map(|(_, s)| s.t).find(|&t| st < t)?;
    Some(if LATE.contains(&st) {
        t + FRAME_MS as i32
    } else {
        t
    })
}

/// Every snapshot from `aim` on, each paired with our frame that ran the
/// same cmds, tagged with retail's time. The weapon byte is ours: our
/// configstring 7 need not number the carbine as retail's does.
fn replay(rig: &mut Rig, cap: &Capture) -> Vec<Sample> {
    assert_eq!(
        rig.gun, cap.gun,
        "the gun's entity number, ours against retail's"
    );
    let first = cap
        .traces
        .iter()
        .position(|(phase, _)| phase != "wait")
        .expect("snapshots past wait");
    let mut out = Vec::new();
    for k in first..cap.traces.len() {
        let (prev, t) = (&cap.traces[k - 1].1, cap.traces[k].1.t);
        let weapon = holding(&rig.gunner).weapon;
        let batch: Vec<(i64, UserCmd)> = cap
            .cmds
            .iter()
            .filter(|c| frame_of(cap, c.st) == Some(t))
            .map(|c| {
                let mut cmd = UserCmd { weapon, ..c.cmd };
                for i in 0..3 {
                    cmd.angles[i] = (cmd.angles[i] + prev.delta_angles[i]) & 0xffff;
                }
                ((c.st - prev.t).clamp(0, FRAME_MS as i32 - 1) as i64, cmd)
            })
            .collect();
        let mut s = rig.frame_at(&batch);
        s.t = t;
        out.push(s);
    }
    out
}

fn angle_off(a: f32, b: f32) -> f32 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

/// Every `!trace` and `!turret` field, the drained events and the impacts,
/// snapshot by snapshot, one row per field that differs, reading
/// `[phase] field t=T: retail .. ours ..`. `delta_angles` and the gunner's
/// `eventSequence` are compared as their change from the first pair: the
/// join leaves each side its own offset. The gunner's ring slots are
/// compared as the drained events, one row per event only one side raised.
fn diff(cap: &Capture, ours: &[Sample]) -> Vec<String> {
    let anims = vcod_common::testing::game_fs().and_then(|fs| PlayerAnims::load(&fs).ok());
    let anim = |wire: i32| {
        let name = anims.as_ref().and_then(|a| a.name(wire)).unwrap_or("?");
        format!("{wire} ({name})")
    };
    let mut rows = Vec::new();
    let phase_of: BTreeMap<i32, &str> = cap.traces.iter().map(|(p, s)| (s.t, p.as_str())).collect();
    let retail: BTreeMap<i32, &Sample> = cap.traces.iter().map(|(_, s)| (s.t, s)).collect();
    let Some(o0) = ours.first() else {
        return vec!["no samples of ours".to_string()];
    };
    let r0 = retail[&o0.t];
    let delta0 = [0, 1, 2].map(|i| (o0.delta_angles[i] - r0.delta_angles[i]) & 0xffff);
    let seq0 = (o0.event_sequence - r0.event_sequence) & 0xff;
    for o in ours {
        let r = retail[&o.t];
        let (phase, t) = (phase_of[&o.t], o.t);
        let mut row = |field: &str, a: String, b: String| {
            if a != b {
                rows.push(format!("[{phase}] {field} t={t}: retail {a} ours {b}"));
            }
        };
        macro_rules! exact {
            ($($f:ident),*) => {
                $(row(stringify!($f), format!("{:?}", r.$f), format!("{:?}", o.$f));)*
            };
        }
        exact!(
            pm_type,
            pm_flags,
            e_flags,
            ground,
            viewlocked,
            viewlocked_ent,
            gunfx,
            hint,
            hint_val,
            hint_string,
            weapon,
            turret_e_flags,
            turret_loop,
            turret_other,
            turret_event_seq,
            turret_ring,
            turret_parms
        );
        row("legs_anim", anim(r.legs_anim), anim(o.legs_anim));
        row("torso_anim", anim(r.torso_anim), anim(o.torso_anim));
        if dist(r.origin, o.origin) > ORIGIN_EPS {
            row(
                "origin",
                format!("{:?}", r.origin),
                format!("{:?}", o.origin),
            );
        }
        if (0..3).any(|i| angle_off(r.viewangles[i], o.viewangles[i]) > PRINT_EPS + SHORT_DEG) {
            row(
                "viewangles",
                format!("{:?}", r.viewangles),
                format!("{:?}", o.viewangles),
            );
        }
        if (0..3)
            .any(|i| angle_off(r.turret_angles2[i], o.turret_angles2[i]) > PRINT_EPS + ANGLES2_EPS)
        {
            row(
                "angles2",
                format!("{:?}", r.turret_angles2),
                format!("{:?}", o.turret_angles2),
            );
        }
        let rebased = [0, 1, 2].map(|i| (o.delta_angles[i] - delta0[i]) & 0xffff);
        row(
            "delta_angles",
            format!("{:?}", r.delta_angles),
            format!("{rebased:?}"),
        );
        row(
            "eventSequence",
            r.event_sequence.to_string(),
            ((o.event_sequence - seq0) & 0xff).to_string(),
        );
        for (side, a, b) in [
            ("retail", &r.events, &o.events),
            ("ours", &o.events, &r.events),
        ] {
            let mut left = b.clone();
            for e in a {
                match left.iter().position(|x| x == e) {
                    Some(i) => {
                        left.remove(i);
                    }
                    None => rows.push(format!("[{phase}] event t={t}: {side} only {e:?}")),
                }
            }
        }
        for (side, a, b) in [
            ("retail", &r.impacts, &o.impacts),
            ("ours", &o.impacts, &r.impacts),
        ] {
            for (e, at) in a.iter() {
                let met = b
                    .iter()
                    .any(|(f, bt)| e == f && (0..3).all(|i| (at[i] - bt[i]).abs() <= IMPACT_EPS));
                if !met {
                    rows.push(format!("[{phase}] impact t={t}: {side} only {e}@{at:?}"));
                }
            }
        }
    }
    rows
}

/// Fails on any row no [`GAPS`] entry names, and on any entry that named no
/// row, so the list cannot outlive what it excuses.
fn finish(rows: Vec<String>) {
    let (gapped, real): (Vec<String>, Vec<String>) = rows
        .into_iter()
        .partition(|r| GAPS.iter().any(|(g, _)| r.contains(g)));
    if report() {
        for r in &gapped {
            let why = GAPS.iter().find(|(g, _)| r.contains(g)).unwrap().1;
            println!("gap: {r}\n     {why}");
        }
        for r in &real {
            println!("{r}");
        }
    }
    let stale: Vec<&str> = GAPS
        .iter()
        .map(|(g, _)| *g)
        .filter(|g| !gapped.iter().any(|r| r.contains(g)))
        .collect();
    assert!(real.is_empty(), "{} rows:\n{}", real.len(), real.join("\n"));
    assert!(
        stale.is_empty(),
        "GAPS entries that match no row: {stale:?}"
    );
}

#[test]
fn the_mount_sweep_fire_and_release_match_retail_on_mp_carentan() {
    let cap = parse(&read(FIXTURE));
    let Some(mut rig) = rig(&cap) else { return };
    let ours = replay(&mut rig, &cap);
    finish(diff(&cap, &ours));
}

/// A `D;` or `K;` line's kind, teams, weapon, damage, means of death and hit
/// location: the client numbers and names are each server's own. Retail's
/// lines carry the log's `m:ss` stamp first.
fn damage_key(line: &str) -> Option<String> {
    let body = match line.split_once(' ') {
        Some((stamp, rest)) if stamp.contains(':') && !stamp.contains(';') => rest,
        _ => line,
    };
    if !(body.starts_with("D;") || body.starts_with("K;")) {
        return None;
    }
    let f: Vec<&str> = body.split(';').collect();
    Some(
        [0, 2, 5, 7, 8, 9, 10]
            .map(|i| f.get(i).copied().unwrap_or("?"))
            .join(";"),
    )
}

#[test]
fn the_turret_kill_is_credited_to_the_mg42() {
    let cap = parse(&read(FIXTURE));
    let Some(mut rig) = rig(&cap) else { return };
    replay(&mut rig, &cap);
    let script = read(SCRIPT_FIXTURE);
    let retail: Vec<String> = script
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(damage_key)
        .collect();
    assert_eq!(retail.len(), 2, "{retail:?}");
    let ours: Vec<String> = rig
        .sv
        .script_log()
        .iter()
        .filter_map(|l| damage_key(l))
        .collect();
    assert_eq!(ours, retail);
}
