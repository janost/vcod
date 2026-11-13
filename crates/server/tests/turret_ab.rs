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

use crate::common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{CMD_MS, ClientEnd, FRAME_MS, PROBE_CMD_MS, Queues, holding};
use vcod_common::animtree::PlayerAnims;
use vcod_common::net::NetClient;
use vcod_common::net::events::EventTracker;
use vcod_common::net::msg::{BUTTON_ATTACK, BUTTON_USE, NULL_USERCMD, UserCmd, WBUTTON_CROUCH};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::pk3::Pk3Fs;
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
const EV_BULLET_TRACER: i32 = 171;
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
    /// The server's protocol, which every snapshot field is read through.
    p: &'static Protocol,
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
    /// The target's `wbuttons` on every frame's cmd.
    target_wbuttons: u8,
    /// The target's `buttons` on every frame's cmd.
    target_buttons: u8,
    /// The target's absolute view pitch on every frame's cmd.
    target_pitch: f32,
    /// The next frame's target cmd stamps, in ms past the last snapshot's
    /// time, in place of [`TARGET_CMD_MS`]; taken by the frame.
    target_plan: Option<Vec<i32>>,
    /// The last target cmd's `serverTime`.
    target_last: i32,
    /// The retail target's cmd stamps ([`target_cmds`]): with any, every
    /// frame without a plan sends the ones before its time, on a server
    /// whose clock runs on retail's ([`rig_aligned`]).
    target_cmds: Vec<i32>,
    /// The server's clock runs on retail's ([`rig_aligned`]), so each
    /// gunner cmd keeps its own stamp.
    aligned: bool,
}

/// One drained event: the id, its parm and whose it was (see [`who`]).
type Ev = (i32, i32, String);

/// One snapshot of the gunner, ours off its client and retail's off a
/// `!trace` line and the `!turret`, `!event` and `!impact` lines after it.
#[derive(Clone, Debug, Default)]
struct Sample {
    /// Retail's `serverTime`; ours carries the one it pairs with.
    t: i32,
    /// `commandTime`, the stamp of the last cmd the frame ran; retail's
    /// only where the capture logs it (`ct=`).
    ct: Option<i32>,
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
    /// Each impact's `otherEntityNum` and `surfType`, in the same order.
    impact_others: Vec<(u32, i32)>,
    /// The target's entity in the gunner's snapshot, where the capture logs
    /// it (`!victim`).
    victim: Option<Victim>,
    /// Ours only: the target's own playerstate, as a `--probe-fall`
    /// client's `FALL` line reads it ([`target_rows`]).
    target_ps: Option<TargetPs>,
}

/// The fields of a `FALL` line [`target_rows`] compares.
#[derive(Clone, Debug, Default)]
struct TargetPs {
    command_time: i32,
    origin: [f32; 3],
    velocity: [f32; 3],
    pm_flags: i32,
    pm_time: i32,
    health: i32,
}

/// The target's entity as a `!victim` line prints it: `pos.trTime` less
/// the snapshot's time, and the trajectory to whole units.
#[derive(Clone, Debug, Default, PartialEq)]
struct Victim {
    tr_off: i32,
    base: [i32; 3],
    delta: [i32; 3],
    legs: i32,
    torso: i32,
    e_flags: i32,
    solid: i32,
}

/// `probe_turret` as the gametype, both clients through the stock menus,
/// then held until the script's teleport has placed them and settled.
fn rig_with(cvars: &[(&str, &str)]) -> Option<Rig> {
    build(cvars, "m1carbine_mp", false)
}

/// The rig with `cvars` set and the gunner joining with `weapon`; with
/// `gunner_second` the target connects first and takes client 0.
fn build(cvars: &[(&str, &str)], weapon: &str, gunner_second: bool) -> Option<Rig> {
    build_on(
        vcod_common::testing::game_fs_11()?,
        &PROTOCOL_V1,
        cvars,
        weapon,
        gunner_second,
        None,
        0,
        None,
        &[],
        None,
    )
}

/// [`build`] on the paks of `fs`, the server speaking `p`; `gun_info`
/// swaps the map's stand guns for that weapon file. The server runs
/// `alone_ms` before the clients connect, and a non-empty `target_cmds`
/// stamps the target's cmds (`Rig::target_cmds`) from its join on. A
/// second gunner joins as `late` says, or with the target.
#[allow(clippy::too_many_arguments)]
fn build_on(
    fs: Pk3Fs,
    p: &'static Protocol,
    cvars: &[(&str, &str)],
    weapon: &str,
    gunner_second: bool,
    gun_info: Option<&str>,
    alone_ms: i64,
    late: Option<LateGunner>,
    target_cmds: &[i32],
    target_spawn: Option<([f32; 3], [f32; 3])>,
) -> Option<Rig> {
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let mut bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    if let Some(info) = gun_info {
        bsp.entities = bsp.entities.replace("mg42_bipod_stand_mp", info);
    }
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = Server::new(common::cfg(MAP, "probe_turret"), now);
    sv.set_protocol(p);
    sv.overlay_script(PROBE_PATH, &read(PROBE_SRC));
    sv.set_cvar("probe_teleport", "1");
    // Only logs: every hit's point and direction to the last bit.
    sv.set_cvar("probe_hitlog", "1");
    for (name, value) in cvars {
        sv.set_cvar(name, value);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    if let Some(late) = late {
        sv.pin_spawns(vec![vec![target_spawn], vec![Some(late.spawn)]]);
    } else if let Some(pin) = target_spawn {
        sv.pin_spawns(vec![vec![Some(pin)]]);
    }
    common::run_alone(&mut sv, &mut now, alone_ms);
    let qg = Rc::new(RefCell::new(Queues::default()));
    let qt = Rc::new(RefCell::new(Queues::default()));
    assert!(gunner_second || target_cmds.is_empty());
    let (gunner, target) = if gunner_second {
        let (t, g, _, _) = common::join_pair_staggered(
            &mut sv,
            &qt,
            &qg,
            &mut now,
            ("axis", "kar98k_mp"),
            ("allies", weapon),
            target_cmds,
            late.map_or(0, |l| l.after_ms),
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
        p,
        qg,
        qt,
        now,
        gunner,
        target,
        gunner_second,
        events: EventTracker::new(),
        gun: 0,
        target_wbuttons: 0,
        target_buttons: 0,
        target_pitch: 0.0,
        target_plan: None,
        target_last: 0,
        target_cmds: target_cmds.to_vec(),
        aligned: false,
    };
    if !target_cmds.is_empty() {
        let snap = rig.target.snapshots().newest().expect("a target snapshot");
        rig.target_last = snap.ps.field_i32(p, "commandTime");
    }
    // A late gunner's capture starts soon after his spawn.
    let settle = if late.is_some() { 10 } else { 40 };
    for _ in 0..settle {
        let h = rig.still();
        rig.frame([h, h]);
    }
    assert!(
        rig.sv.script_aborts().is_empty(),
        "{:?}",
        rig.sv.script_aborts()
    );
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
            if !self.aligned {
                self.gunner.send_frame(&cmd);
            } else {
                // On retail's clock (`rig_aligned`) a cmd keeps its own
                // stamp, so one that arrived late still runs its own ms.
                let st = cmd.server_time;
                let sent = common::stamped(&self.gunner, &cmd, &[st], i32::MIN, i32::MAX);
                self.gunner.send_cmds(&sent);
            }
        }
        let mut held = UserCmd {
            wbuttons: self.target_wbuttons,
            buttons: self.target_buttons,
            ..holding(&self.target)
        };
        held.angles[0] = deg_to_short(self.target_pitch);
        match self.target_plan.take() {
            Some(plan) => {
                // Stamped by hand: `send_frame` stamps the client's clock and
                // rebases the view, so the rebase is done here too.
                let p = self.p;
                let snap = self.target.snapshots().newest().expect("a target snapshot");
                let base = snap.server_time;
                let da = [0, 1, 2].map(|i| snap.ps.field_i32(p, &format!("delta_angles[{i}]")));
                let mut cmds = Vec::new();
                for off in plan {
                    let st = base + off;
                    if st > self.target_last {
                        let mut c = held;
                        c.server_time = st;
                        c.angles = [0, 1, 2].map(|i| (held.angles[i] - da[i]) & 0xffff);
                        cmds.push(c);
                        self.target_last = st;
                    }
                }
                self.target.send_cmds(&cmds);
            }
            None if !self.target_cmds.is_empty() => {
                let snap = self.target.snapshots().newest().expect("a target snapshot");
                let end = snap.server_time + FRAME_MS as i32;
                let cmds = common::stamped(
                    &self.target,
                    &held,
                    &self.target_cmds,
                    self.target_last,
                    end,
                );
                if let Some(c) = cmds.last() {
                    self.target_last = c.server_time;
                }
                self.target.send_cmds(&cmds);
            }
            None => {
                for ms in TARGET_CMD_MS {
                    self.target.pump_at(start + Duration::from_millis(ms));
                    if let Some(c) = self.target.send_frame(&held) {
                        self.target_last = c.server_time;
                    }
                }
            }
        }
        self.now = start + Duration::from_millis(FRAME_MS as u64);
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
        if let Some(snap) = self.target.snapshots().newest() {
            let p = self.p;
            let i = |n: &str| snap.ps.field_i32(p, n);
            let f = |n: &str| f32::from_bits(i(n) as u32);
            s.target_ps = Some(TargetPs {
                command_time: i("commandTime"),
                origin: [f("origin[0]"), f("origin[1]"), f("origin[2]")],
                velocity: [f("velocity[0]"), f("velocity[1]"), f("velocity[2]")],
                pm_flags: i("pm_flags"),
                pm_time: i("pm_time"),
                health: snap.ps.health(),
            });
        }
        if let Some(snap) = self.gunner.snapshots().newest() {
            for e in self.events.drain(snap, self.p) {
                let event = nearest_v1(e.event);
                if event == EV_BULLET_HIT_SMALL || event == EV_BULLET_HIT_LARGE {
                    s.impacts.push((event, e.pos));
                    s.impact_others.push((e.other_entity_num, e.surf_type));
                }
                s.events
                    .push((event, e.parm, who(e.entity_num as i64, self.gun)));
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
            let v = s.ps.viewangles(self.p);
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
        let p = self.p;
        self.gunner
            .snapshots()
            .newest()
            .and_then(|s| s.entities.get(&self.gun))
            .map_or([0.0; 3], |e| e.origin(p))
    }

    /// The turret's `apos.trBase[1]` from the gunner's newest snapshot.
    fn gun_yaw(&self) -> f32 {
        let p = self.p;
        self.gunner
            .snapshots()
            .newest()
            .and_then(|s| s.entities.get(&self.gun))
            .map_or(0.0, |e| e.field_f32(p, "apos.trBase[1]"))
    }

    fn sample(&self) -> Sample {
        let p = self.p;
        let Some(s) = self.gunner.snapshots().newest() else {
            return Sample::default();
        };
        let ps = |name: &str| s.ps.field_i32(p, name);
        let gun = s.entities.get(&self.gun);
        let g = |name: &str| gun.map_or(0, |e| e.field_i32(p, name));
        let ev = |name: &str| ev_v1(p, g(name));
        Sample {
            t: s.server_time,
            ct: Some(ps("commandTime")),
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
                    .map(|i| ev_v1(p, e.field_i32(p, &format!("events[{}]", i & 3))))
                    .collect()
            }),
            turret_ring: [0, 1, 2, 3].map(|i| ev(&format!("events[{i}]"))),
            turret_parms: [0, 1, 2, 3].map(|i| g(&format!("eventParms[{i}]"))),
            events: Vec::new(),
            impacts: Vec::new(),
            impact_others: Vec::new(),
            victim: s.entities.get(&self.target_num()).map(|e| {
                let i = |name: &str| e.field_i32(p, name);
                let f = |name: &str| e.field_f32(p, name) as i32;
                Victim {
                    tr_off: i("pos.trTime") - s.server_time,
                    base: [0, 1, 2].map(|k| f(&format!("pos.trBase[{k}]"))),
                    delta: [0, 1, 2].map(|k| f(&format!("pos.trDelta[{k}]"))),
                    legs: i("legsAnim"),
                    torso: i("torsoAnim"),
                    e_flags: i("eFlags"),
                    solid: i("solid"),
                }
            }),
            target_ps: None,
        }
    }

    /// The target's client number.
    fn target_num(&self) -> u32 {
        u32::from(!self.gunner_second)
    }
}

/// A raw `events[]` slot in 1.1's numbering, as the probe prints a capture's
/// (`probe::ev_v1`).
fn ev_v1(p: &Protocol, raw: i32) -> i32 {
    nearest_v1((p.event_to_v1)(raw))
}

/// UO's bullet hit as the probe prints it, the small one
/// (`probe::nearest_v1`); no other UO event reaches this rig.
fn nearest_v1(event: i32) -> i32 {
    match event {
        vcod_common::net::event_ids::EV_UO_BULLET_HIT => EV_BULLET_HIT_SMALL,
        e => e,
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

/// A round's world impact names the gun, `Bullet_Fire`'s inflictor
/// (`Bullet_Fire_Extended` 1.1d 0x68b05; the 1.5b gunner capture's hits
/// all name the gun), so a client draws its tracer off the gun's tag. The
/// flesh copy a hit on the target raises names the gunner, the callback's
/// attacker (`finishPlayerDamage` 0x43aee).
#[test]
fn a_turret_rounds_impacts_name_the_gun() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    let h = rig.still();
    let fire = UserCmd {
        buttons: h.buttons | BUTTON_ATTACK,
        ..h
    };
    let mut others = Vec::new();
    for _ in 0..6 {
        others.extend(rig.frame([fire, fire]).impact_others);
    }
    assert!(
        others.iter().any(|&(_, surf)| surf != 7),
        "no round met the world"
    );
    for &(other, surf) in &others {
        let want = if surf == 7 { 0 } else { rig.gun };
        assert_eq!(other, want, "{others:?} gun {}", rig.gun);
    }
}

/// A manned gun's round is gunfire the bots hear, the gunner's own.
#[test]
fn a_turret_round_is_heard() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    rig.tap(BUTTON_USE);
    let h = rig.still();
    assert!(rig.sv.bot_noises().is_empty(), "noise before the trigger");
    let fire = UserCmd {
        buttons: h.buttons | BUTTON_ATTACK,
        ..h
    };
    rig.frame([fire, fire]);
    let heard = rig.sv.bot_noises();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].source, 0, "not the gunner's");
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

/// A round fired from `ClientEndFrame` at `aim_z` over the feet of a target
/// that stands up from a crouch `after` frames before the gun fires; the
/// damage it did.
fn round_at_a_target_standing_up(gunner_second: bool, after: usize, aim_z: f32) -> Option<i32> {
    let mut rig = build(&[], "m1carbine_mp", gunner_second)?;
    rig.tap(BUTTON_USE);
    rig.target_wbuttons = WBUTTON_CROUCH;
    rig.hold(10);
    let p = &PROTOCOL_V1;
    let gun = rig.gun_origin();
    let target = rig.target.snapshots().newest()?.ps.origin(p);
    // At a standing head; the crouched bones end below it.
    let (dx, dy, dz) = (
        target[0] - gun[0],
        target[1] - gun[1],
        target[2] + aim_z - (gun[2] + 21.0),
    );
    let yaw = dy.atan2(dx).to_degrees();
    let pitch = -dz.atan2(dx.hypot(dy)).to_degrees();
    for _ in 0..4 {
        rig.look([pitch, yaw]);
    }
    let before = rig.target.snapshots().newest()?.ps.health();
    assert_eq!(before, 100);
    rig.target_wbuttons = 0;
    rig.hold(after);
    let h = rig.still();
    let fire = UserCmd {
        buttons: h.buttons | BUTTON_ATTACK,
        ..h
    };
    let s = rig.frame_at(&[(CMD_MS, fire)]);
    assert_eq!(s.viewlocked, 2);
    Some(before - rig.target.snapshots().newest()?.ps.health())
}

/// A round meets the target posed as its own end frame last left it when the
/// target's slot is higher, and as this frame's when it is lower (combat doc
/// 16.1, turrets doc 6.1). A stance change is one such end frame, but its
/// clip comes in at weight 0 and blends over 250 ms (movers doc 15), so the
/// round on the stand-up frame meets a crouched body from either slot, and
/// one four frames on meets a standing head from both.
#[test]
fn a_stance_change_blends_in_whatever_the_slot_order() {
    let Some(higher) = round_at_a_target_standing_up(false, 0, 62.0) else {
        return;
    };
    assert_eq!(higher, 0, "the target in slot 1 is still posed crouched");
    let lower = round_at_a_target_standing_up(true, 0, 62.0).unwrap();
    assert_eq!(lower, 0, "the target in slot 0 is still posed crouched");
    for gunner_second in [false, true] {
        let hit = round_at_a_target_standing_up(gunner_second, 4, 62.0).unwrap();
        assert!(
            hit > 0,
            "gunner second {gunner_second}: no hit once stood up"
        );
    }
}

/// `probe_pose`'s hit point for the frame at `t`: where the round met the
/// target, `None` for a frame whose round missed it.
fn pose_hit(rig: &Rig, t: i32) -> Option<[f32; 3]> {
    let tag = format!("PROBE hit {t} ");
    let line = rig.sv.script_log().iter().find_map(|l| {
        let i = l.find(&tag)?;
        Some(l[i + tag.len()..].to_string())
    })?;
    let v = line.split_once('(')?.1.trim_end_matches(')');
    let mut it = v.split(',').map(|x| x.trim().parse::<f32>().unwrap());
    Some([it.next()?, it.next()?, it.next()?])
}

/// `mp_carentan-dm-pitch-ease.txt`: retail's turret rounds against a body
/// flipping its view pitch 0 / 85 every 400 ms (combat doc 16.3).
const PITCH_EASE_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/turret/mp_carentan-dm-pitch-ease.txt"
);
/// The retail gunner's settled view in that capture, wire shorts.
const PITCH_EASE_VIEW: [f32; 2] = [214.0, 41527.0];
/// Frames past a flip a row covers: the first eased frame and the next five.
const EASE_FRAMES: usize = 6;

/// One flip's rounds: the pitch it flipped to and each frame's hit x from
/// the flip's frame on, less the level body's hit x (the frame before the
/// next flip to 85, or before this one), `None` for a miss.
type EaseRow = (f32, Vec<Option<f32>>);

/// The fixture's rows for `run` (`slot0` or `slot1`).
fn retail_ease_rows(run: &str) -> Vec<EaseRow> {
    let text = read(PITCH_EASE_FIXTURE);
    let mut hits: BTreeMap<i32, f32> = BTreeMap::new();
    let mut flips: Vec<(i32, f32)> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("[run ") {
            inside = head.starts_with(run);
        } else if !inside {
        } else if let Some(rest) = line.strip_prefix("PITCH serverTime=") {
            let (t, p) = rest.split_once(" pitch=").unwrap();
            flips.push((t.parse().unwrap(), p.parse().unwrap()));
        } else if let Some(rest) = line.strip_prefix("PROBE hit ") {
            let t = rest.split(' ').next().unwrap().parse().unwrap();
            let x = rest.split_once('(').unwrap().1.split(',').next().unwrap();
            hits.insert(t, x.trim().parse().unwrap());
        }
    }
    let ms = FRAME_MS as i32;
    flips
        .iter()
        .enumerate()
        .filter_map(|(i, &(t, pitch))| {
            let level_at = if pitch == 0.0 {
                flips.get(i + 1)?.0 - ms
            } else {
                t - ms
            };
            let level = *hits.get(&level_at)?;
            let row = (0..EASE_FRAMES as i32)
                .map(|k| hits.get(&(t + k * ms)).map(|x| x - level))
                .collect();
            Some((pitch, row))
        })
        .collect()
}

/// The same rows off ours: the rig with the retail gunner's view firing
/// every frame while the target flips its pitch every eight frames.
fn our_ease_rows(gunner_second: bool) -> Option<Vec<EaseRow>> {
    let mut rig = build(&[("probe_pose", "1")], "m1carbine_mp", gunner_second)?;
    rig.tap(BUTTON_USE);
    let view = PITCH_EASE_VIEW.map(|s| s * 360.0 / 65536.0);
    for _ in 0..4 {
        rig.look(view);
    }
    let fire = |rig: &mut Rig| {
        let h = rig.still();
        let f = UserCmd {
            buttons: h.buttons | BUTTON_ATTACK,
            ..h
        };
        let s = rig.frame([f, f]);
        pose_hit(rig, s.t).map(|h| h[0])
    };
    for _ in 0..20 {
        fire(&mut rig);
    }
    let mut windows: Vec<(f32, Option<f32>, Vec<Option<f32>>)> = Vec::new();
    let mut before = fire(&mut rig);
    for flip in 0..8 {
        rig.target_pitch = if flip % 2 == 0 { 85.0 } else { 0.0 };
        let xs: Vec<Option<f32>> = (0..8).map(|_| fire(&mut rig)).collect();
        windows.push((rig.target_pitch, before, xs.clone()));
        before = xs[7];
    }
    let rows = windows
        .iter()
        .enumerate()
        .filter_map(|(i, (pitch, before, xs))| {
            let level = if *pitch == 0.0 {
                windows.get(i + 1)?.1?
            } else {
                (*before)?
            };
            let row = xs[..EASE_FRAMES]
                .iter()
                .map(|x| x.map(|x| x - level))
                .collect();
            Some((*pitch, row))
        })
        .collect();
    Some(rows)
}

/// Retail, `mp_carentan-dm-pitch-ease.txt` (combat doc 16.3): after a view
/// pitch change the round's point eases over five or six frames, the
/// torso following `BG_PlayerAnimation`'s pitch swing. With the target in
/// the higher slot its flip frame's round meets the pose of the frame
/// before (16.1); in the lower slot it already meets the first eased step.
/// Per flip direction and frame, ours must sit inside the band retail's
/// flips spread over, widened by the turret's own scatter.
#[test]
fn a_round_meets_the_torso_easing_after_a_pitch_flip() {
    const SLACK: f32 = 0.6;
    for (run, gunner_second) in [("slot1", false), ("slot0", true)] {
        let Some(ours) = our_ease_rows(gunner_second) else {
            return;
        };
        let retail = retail_ease_rows(run);
        for pitch in [85.0, 0.0] {
            for k in 0..EASE_FRAMES {
                // GAP: a body posed at (or within a step of) the full
                // 0.6 * 85 is mostly missed by the round retail still lands
                // on the back of the torso: the flip-frame row back to 0 in
                // the higher slot, the last row to 85 in the lower.
                let full = if run == "slot1" {
                    pitch == 0.0 && k == 0
                } else {
                    pitch == 85.0 && k == 5
                };
                if full {
                    continue;
                }
                let column = |rows: &[EaseRow]| -> Vec<f32> {
                    rows.iter()
                        .filter(|(p, _)| *p == pitch)
                        .filter_map(|(_, r)| r[k])
                        .collect()
                };
                let (theirs, mine) = (column(&retail), column(&ours));
                assert!(theirs.len() >= 4 && mine.len() >= 2, "{run} {pitch} k{k}");
                let lo = theirs.iter().copied().fold(f32::MAX, f32::min) - SLACK;
                let hi = theirs.iter().copied().fold(f32::MIN, f32::max) + SLACK;
                let mean = mine.iter().sum::<f32>() / mine.len() as f32;
                assert!(
                    (lo..=hi).contains(&mean),
                    "{run} to {pitch} frame k{k}: ours {mean:.2} {mine:?}, retail {theirs:?}"
                );
            }
        }
        // The higher slot's flip-frame round meets the old pose outright.
        if run == "slot1" {
            for (_, row) in ours.iter().filter(|(p, _)| *p == 85.0) {
                assert!(row[0].is_some_and(|d| d.abs() < 0.2), "{row:?}");
            }
        }
    }
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

/// `TeleportPlayer`'s link (combat doc 14.7): the release files the gunner
/// in the area tree where it put him, in the end frame of the use cmd,
/// ahead of his next cmd's link.
#[test]
fn the_release_links_the_gunner_where_it_put_him() {
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    let slot = rig.gunner_second as u32;
    rig.tap(BUTTON_USE);
    rig.hold(2);
    let mounted = rig.sv.test_area_link(slot).expect("the gunner is linked");
    let h = rig.still();
    let s = rig.frame([
        UserCmd {
            buttons: h.buttons | BUTTON_USE,
            ..h
        },
        h,
    ]);
    assert_eq!(s.viewlocked, 0, "released");
    let (absmin, absmax) = rig.sv.test_area_link(slot).expect("linked again");
    let centre = [0, 1].map(|i| (absmin[i] + absmax[i]) * 0.5);
    assert!(
        (centre[0] - s.origin[0]).abs() < 0.01 && (centre[1] - s.origin[1]).abs() < 0.01,
        "filed at {centre:?}, released at {:?}, mounted box {mounted:?}",
        s.origin
    );
    assert!(
        (absmin[2] - (s.origin[2] - 1.0)).abs() < 0.01,
        "linked at the lifted origin: {absmin:?} for {:?}",
        s.origin
    );
}

/// `turret_think_client`'s `sessionstate` test (turrets doc 8): a gunner
/// killed on the gun lets go that frame, and the gun is free for its next
/// life. The clone happens before the release, but the corpse's fields are
/// re-read from the sim at snapshot build, after the release has run, so it
/// carries no mounted bits; this pins ours where turrets doc 13 only infers
/// that retail's clone runs first and a retail corpse may keep 0xC000, since
/// no capture covers it.
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
        .filter(|&(&n, _)| (64..72).contains(&n))
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
    // `G_FreeTurret`'s `TeleportPlayer` links him where it put him.
    let (absmin, absmax) = rig
        .sv
        .test_area_link(rig.gunner_second as u32)
        .expect("linked");
    let centre = [0, 1].map(|i| (absmin[i] + absmax[i]) * 0.5);
    assert!(
        (centre[0] - s.origin[0]).abs() < 0.01 && (centre[1] - s.origin[1]).abs() < 0.01,
        "filed at {centre:?}, released at {:?}",
        s.origin
    );
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
    rig.gunner.disconnect();
    rig.hold(10);
    let s = target_gun_angles2(&rig).unwrap();
    assert_eq!(s[1], 0.0, "{s:?}");
}

/// A gunner killed by a lower slot's round is dead on its own turn of the
/// end-frame loop, since the callback runs inside the shooter's
/// `ClientEndFrame` (combat doc 16.2): `turret_think_client` lets go instead
/// of firing, so its held trigger raises no shot on the death frame, and the
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
    let other_seq = |rig: &Rig| {
        rig.gunner
            .snapshots()
            .newest()
            .and_then(|s| s.entities.get(&other))
            .map(|e| e.field_i32(p, "eventSequence"))
            .expect("the second gun in the gunner's view")
    };
    rig.target_buttons = BUTTON_ATTACK;
    let mut dead = None;
    let mut fired_alive = false;
    for _ in 0..20 {
        let before = other_seq(&rig);
        rig.frame([fire, fire]);
        let fired = other_seq(&rig) != before;
        let t = rig.target.snapshots().newest().unwrap();
        if t.ps.field_i32(p, "pm_type") == 6 {
            assert!(!fired, "the dead gunner's gun fired on its death frame");
            dead = Some(t.clone());
            break;
        }
        fired_alive |= fired;
    }
    assert!(fired_alive, "the held trigger fires while the target lives");
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
        .find(|&(&n, e)| (64..72).contains(&n) && e.field_i32(p, "clientNum") == 1)
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
    let Some(fs) = vcod_common::testing::game_fs_11() else {
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

/// The use pass runs inside the cmd that pressed it, so the frame's later
/// cmd runs mounted and the first snapshot that reads the lock already has
/// the body on the gun and the mounted anim, as retail's first stand-mount
/// snapshot does (`docs/research/cod11-turrets.md` 12.1).
#[test]
fn the_first_locked_snapshot_is_already_placed() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let anims = PlayerAnims::load(&fs).unwrap();
    let Some(mut rig) = rig_with(&[]) else {
        return;
    };
    let before = rig.hold(1);
    assert_eq!(before.viewlocked, 0);
    let h = rig.still();
    let used = UserCmd {
        buttons: h.buttons | BUTTON_USE,
        ..h
    };
    let s = rig.frame([used, h]);
    assert_eq!(
        s.viewlocked, 1,
        "the use frame's snapshot is the first locked one"
    );
    assert_eq!(s.ground, 1023);
    assert_eq!(
        s.legs_anim & !ANIM_TOGGLEBIT,
        anims.wire_of("standMG42_aim").unwrap()
    );
    let moved = dist_xy(s.origin, [before.origin[0], before.origin[1]]);
    assert!(moved > 1.0, "still at the mount spot: {:?}", s.origin);
    assert!(dist_xy(s.origin, GUN_NEAR) < 60.0, "{:?}", s.origin);
}

// ------------------------------------------------------------ the retail gate
//
// The retail captures replayed on ours. `turret/mp_carentan-dm-turret-<tag>.txt`
// is the gunner's side (every cmd, every snapshot, the gun's entity, every
// drained event and impact) and its `-script.txt` the server's log, both from
// one run (`docs/research/cod11-turrets.md` 12, 13.4). The first captures
// (`-turret.txt`, `-15`, `-uo`, `-victim-15`, `-victim-uo`) logged neither the
// target's spawn time nor its cmds and are not replayed: the `-spawn` captures
// of the same recipe carry both and match on all three servers (13.4). The rig numbers
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
// Where the capture logs each snapshot's `commandTime` (`ct=`), a cmd pairs
// with the first snapshot whose `ct=` reached its stamp instead ([`frame_of`]).

const FIXTURE_FULL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/turret/mp_carentan-dm-turret-full.txt"
);
const FIXTURE_VICTIM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/turret/mp_carentan-dm-turret-victim-ct.txt"
);
const FIXTURE_VICTIM_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/turret/mp_carentan-dm-turret-victim-ct-script.txt"
);
/// The 2026-10-19 run of the same recipe, from before `ct=`: its gunner
/// stood where the strafe's step reads parm 142 (turrets doc 13.3).
const FIXTURE_VICTIM_19: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/turret/mp_carentan-dm-turret-victim.txt"
);
const FIXTURE_VICTIM_19_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/turret/mp_carentan-dm-turret-victim-script.txt"
);
/// When in a frame the target's cmds go out: the net probe sends one every
/// 16 ms of its own clock, three a frame.
const TARGET_CMD_MS: [u64; 3] = [16, 33, FRAME_MS as u64];
const ORIGIN_EPS: f32 = 0.25;
/// Retail's fixture prints angles to 0.1.
const PRINT_EPS: f32 = 0.05;
/// One `ANGLE2SHORT` step.
const SHORT_DEG: f32 = 360.0 / 65536.0;
const ANGLES2_EPS: f32 = 0.01;
/// The largest move the start rebase may make on any axis: retail's own
/// landings from the gsc's placement ended from 2.4 units short of the spot
/// (1.1) to 0.9 past it (1.5b, UO), so ours, 3.3 short on 1.5 and UO, starts
/// up to 4.2 off retail's.
const REBASE_MAX_VICTIM: f32 = 4.5;

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
    /// The target's own cmd stamps, where its client logged them
    /// ([`target_cmds`]); empty otherwise.
    target_cmds: Vec<i32>,
    /// Every snapshot with its phase, `wait` included.
    traces: Vec<(String, Sample)>,
    /// Cvars the replay sets on ours beyond the recipe's.
    cvars: &'static [(&'static str, &'static str)],
    /// Where retail's spawn put the target (`PROBE spawnat`), which ours
    /// pins: a spawn's think on a sloped point leaves him a drift.
    target_spawn: Option<([f32; 3], [f32; 3])>,
    /// The gunner's first origin at full precision (`exact=`), where the
    /// rebase puts ours; the 0.1 print when the capture predates it.
    start: Option<[f32; 3]>,
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
    let mut start = None;
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
            if traces.is_empty() {
                start = m.get("exact").map(|v| floats(v));
            }
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            let [hint, hint_val, hint_string] = {
                let h: Vec<i32> = m["hint"].split(':').map(|v| v.parse().unwrap()).collect();
                [h[0], h[1], h[2]]
            };
            let s = Sample {
                t: i("serverTime"),
                ct: m.get("ct").map(|v| v.parse().unwrap()),
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
        } else if let Some(rest) = line.strip_prefix("!victim ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            let s = &mut traces.last_mut().expect("a !victim after a !trace").1;
            s.victim = Some(Victim {
                tr_off: i("trTime") - s.t,
                base: ints(m["trBase"]),
                delta: ints(m["trDelta"]),
                legs: i("legsAnim"),
                torso: i("torsoAnim"),
                e_flags: i("eFlags"),
                solid: i("solid"),
            });
        } else if let Some(rest) = line.strip_prefix("!impact ") {
            let m = kv(rest);
            let s = &mut traces.last_mut().expect("an !impact after a !trace").1;
            s.impacts
                .push((m["event"].parse().unwrap(), floats(m["origin"])));
        }
    }
    Capture {
        target_cmds: Vec::new(),
        cvars: &[],
        target_spawn: None,
        start,
        weapon,
        gun,
        cmds,
        traces,
    }
}

/// A spawn's origin and angles, as [`Server::pin_spawns`] takes it.
type Spawn = ([f32; 3], [f32; 3]);

/// A gunner who joins after the target, as retail's capture client did: he
/// connects `after_ms` after it and spawns at `spawn`.
#[derive(Clone, Copy)]
struct LateGunner {
    after_ms: i64,
    spawn: Spawn,
}

/// The rig as the capture's recipe left retail, on the paks of `fs`, the
/// server speaking `p`: the gunner joined with the header's weapon, as
/// client 1, standing where retail's did, the start rebased at most
/// `rebase_max` on any axis (`docs/research/cod11-turrets.md` 13.1).
fn rig_on(cap: &Capture, fs: Pk3Fs, p: &'static Protocol, rebase_max: f32) -> Option<Rig> {
    rig_after(cap, fs, p, rebase_max, 0, None)
}

/// [`rig_on`] with the server alone for `alone_ms` before the joins and the
/// gunner joining as `late` says.
fn rig_after(
    cap: &Capture,
    fs: Pk3Fs,
    p: &'static Protocol,
    rebase_max: f32,
    alone_ms: i64,
    late: Option<LateGunner>,
) -> Option<Rig> {
    let mut rig = build_on(
        fs,
        p,
        cap.cvars,
        &cap.weapon,
        true,
        None,
        alone_ms,
        late,
        &cap.target_cmds,
        cap.target_spawn,
    )?;
    let at = cap.traces.first().expect("a snapshot").1.origin;
    let mut ours = rig.sample().origin;
    for i in 0..3 {
        let off = ours[i] - at[i];
        assert!(
            off.abs() <= rebase_max,
            "the placement put ours {off} off retail's spot on axis {i}: {ours:?} against {at:?}"
        );
        if off.abs() > PRINT_EPS {
            ours[i] = at[i];
        }
    }
    if let Some(start) = cap.start {
        for i in 0..3 {
            assert!(
                (start[i] - at[i]).abs() <= PRINT_EPS,
                "{start:?} outside {at:?}'s print"
            );
        }
        ours = start;
    }
    rig.sv.test_set_client_origin(1, ours);
    rig.hold(1);
    Some(rig)
}

/// How long our joins take from the first connect to the target's
/// placement: the first guess [`rig_aligned`] makes, which it corrects once
/// if the joins have since got faster or slower.
const JOIN_TO_PLACE_MS: i64 = 800;

/// The level time of the gsc's `PROBE place` line for client `num`, in
/// our script log or retail's games_mp.log (after its `m:ss` stamp).
fn place_ms<'a>(mut log: impl Iterator<Item = &'a str>, num: u32) -> Option<i32> {
    log.find_map(|l| {
        let (_, rest) = l.trim().split_once("PROBE place ")?;
        (rest.split(' ').next()? == num.to_string())
            .then(|| rest.rsplit(' ').next()?.parse().ok())
            .flatten()
    })
}

/// [`rig_on`] on retail's level clock: the target placed (spawned, the same
/// frame) at `place`, retail's `PROBE place` time for client 0, and the
/// server held until its newest snapshot is the one before retail's first
/// past `wait`, so every replayed frame runs at the level time of the
/// retail snapshot it pairs with. The target's idle runs from its spawn
/// and a movement clip's start phase off the level time (`animclock`), so
/// the pose a round meets is retail's (`cod11-turrets.md` 13.3). With
/// `gunner`, retail's `PROBE place` time for client 1 and the spawn to pin
/// him to, the gunner joins late enough to be placed then too, so his own
/// idle runs from retail's spawn (13.9).
fn rig_aligned(
    cap: &Capture,
    fs: &impl Fn() -> Option<Pk3Fs>,
    p: &'static Protocol,
    rebase_max: f32,
    place: i32,
    gunner: Option<(i32, Spawn)>,
) -> Option<Rig> {
    let gunner_place = gunner.map(|g| g.0);
    let mut alone = place as i64 - JOIN_TO_PLACE_MS;
    let mut after = gunner_place.map_or(0, |g| (g - place) as i64);
    for _ in 0..3 {
        let late = gunner.map(|(_, spawn)| LateGunner {
            after_ms: after,
            spawn,
        });
        let mut rig = rig_after(cap, fs()?, p, rebase_max, alone, late)?;
        let log = rig.sv.script_log();
        let ours = place_ms(log.iter().map(|l| l.as_str()), 0)
            .expect("our PROBE place line for the target");
        let ours_gunner = place_ms(log.iter().map(|l| l.as_str()), 1);
        let gunner_off = gunner_place
            .zip(ours_gunner)
            .map_or(0, |(g, o)| (g - o) as i64);
        if ours != place || gunner_off != 0 {
            alone += (place - ours) as i64;
            after += gunner_off - (place - ours) as i64;
            continue;
        }
        let first = cap
            .traces
            .iter()
            .find(|(phase, _)| phase != "wait")
            .expect("snapshots past wait")
            .1
            .t;
        while rig.sample().t < first - FRAME_MS as i32 {
            rig.hold(1);
        }
        assert_eq!(
            rig.sample().t,
            first - FRAME_MS as i32,
            "our clock against retail's before the replay"
        );
        rig.aligned = true;
        return Some(rig);
    }
    panic!("the placements never landed on retail's {place} and {gunner_place:?}");
}

/// Every `serverTime` a `--probe-fall` client's `CMDS` lines list, in send
/// order.
fn target_cmds(text: &str) -> Vec<i32> {
    let mut out = Vec::new();
    for rest in text.lines().filter_map(|l| l.strip_prefix("CMDS st=")) {
        let (first, steps) = rest.split_once(" d=").expect("CMDS st= d=");
        let mut st: i32 = first.parse().expect("a stamp");
        out.push(st);
        for d in steps.split_whitespace().next().unwrap_or("").split(',') {
            st += d.parse::<i32>().expect("a step");
            out.push(st);
        }
    }
    out
}

/// The retail snapshot a cmd's effect first shows in: the first whose
/// `commandTime` reached its stamp, where the capture logs it. A cmd stamped
/// at or below one already run lands in the same frame and runs on neither
/// server (turrets doc 13.4).
fn frame_of(cap: &Capture, c: &RetailCmd) -> Option<i32> {
    if cap.traces.iter().any(|(_, s)| s.ct.is_some()) {
        return cap
            .traces
            .iter()
            .find(|(_, s)| s.ct.is_some_and(|ct| ct >= c.st))
            .map(|(_, s)| s.t);
    }
    cap.traces.iter().map(|(_, s)| s.t).find(|&t| c.st < t)
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
            .filter(|c| frame_of(cap, c) == Some(t))
            .map(|c| {
                let mut cmd = UserCmd {
                    weapon,
                    server_time: c.st,
                    ..c.cmd
                };
                for i in 0..3 {
                    cmd.angles[i] = (cmd.angles[i] + prev.delta_angles[i]) & 0xffff;
                }
                ((c.st - prev.t).clamp(0, FRAME_MS as i32 - 1) as i64, cmd)
            })
            .collect();
        // The target's cmds end where its entity's `pos.trTime` says its
        // last one did: from its own log where the capture has one, on a
        // clock that runs on retail's level time (`rig_aligned`); else one
        // every 16 ms back to the last frame's.
        if !cap.target_cmds.is_empty() {
            // The rig sends the ones before the frame's time by itself.
            if let Some(v) = &cap.traces[k].1.victim {
                let end = t + v.tr_off;
                rig.target_plan = Some(
                    cap.target_cmds
                        .iter()
                        .filter(|&&st| st <= end)
                        .map(|&st| st - prev.t)
                        .collect(),
                );
            }
        } else if let Some(v) = &cap.traces[k].1.victim {
            let end = v.tr_off + FRAME_MS as i32;
            let floor = prev
                .victim
                .as_ref()
                .map_or(end - FRAME_MS as i32, |p| p.tr_off);
            let mut plan: Vec<i32> = (0..)
                .map(|n| end - n * PROBE_CMD_MS)
                .take_while(|&o| o > floor)
                .collect();
            plan.reverse();
            rig.target_plan = Some(plan);
        }
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
/// `[phase] field t=T: retail .. ours ..`. `delta_angles`, the gunner's
/// `eventSequence` and its `legsAnim` restart toggle are compared as their
/// change from the first pair: the join leaves each side its own offset, the
/// toggle's off the anims its random spawn point's think and landing played.
/// The gunner's ring slots are
/// compared as the drained events, one row per event only one side raised.
fn diff(cap: &Capture, ours: &[Sample], fs: &Pk3Fs) -> Vec<String> {
    let anims = PlayerAnims::load(fs).ok();
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
    let legs0 = (o0.legs_anim ^ r0.legs_anim) & ANIM_TOGGLEBIT;
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
        // Where retail logged it, every frame ran the same last cmd.
        if let Some(ct) = r.ct {
            row("ct", ct.to_string(), format!("{:?}", o.ct.unwrap_or(0)));
        }
        row("legs_anim", anim(r.legs_anim), anim(o.legs_anim ^ legs0));
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
        // The wire carries 16 bits; a change above them decodes as the
        // base with a bit set past the width (`msg::above_width`).
        row(
            "delta_angles",
            format!("{:?}", r.delta_angles.map(|a| a & 0xffff)),
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
        if let Some(rv) = &r.victim {
            let ov = o.victim.clone().unwrap_or_default();
            let strip = |v: &Victim| Victim {
                legs: v.legs & !ANIM_TOGGLEBIT,
                torso: v.torso & !ANIM_TOGGLEBIT,
                ..v.clone()
            };
            if strip(rv) != strip(&ov) {
                rows.push(format!(
                    "[{phase}] victim t={t}: retail {rv:?} {} ours {ov:?} {}",
                    anim(rv.legs),
                    anim(ov.legs)
                ));
            }
        }
        for (side, a, b) in [
            ("retail", &r.impacts, &o.impacts),
            ("ours", &o.impacts, &r.impacts),
        ] {
            for (e, at) in a.iter() {
                // Both sides truncate to whole units (`G_TempEntity`), so
                // equal or not at all.
                let met = b.iter().any(|(f, bt)| e == f && at == bt);
                if !met {
                    rows.push(format!("[{phase}] impact t={t}: {side} only {e}@{at:?}"));
                }
            }
        }
    }
    rows
}

/// The target's own playerstate against its `FALL` lines on the snapshots
/// the capture logs its entity on (`!victim`, the fire and target phases,
/// the kill included): command time, `pm_flags`, `pm_time` and health
/// exactly, origin and velocity to the line's print. The legs are left
/// out: a death anim is a random draw.
fn target_rows(cap: &Capture, log: &str, ours: &[Sample]) -> Vec<String> {
    let ours: BTreeMap<i32, &TargetPs> = ours
        .iter()
        .filter(|s| {
            cap.traces
                .iter()
                .any(|(_, r)| r.t == s.t && r.victim.is_some())
        })
        .filter_map(|s| Some((s.t, s.target_ps.as_ref()?)))
        .collect();
    let mut rows = Vec::new();
    let mut compared = 0;
    for rest in log.lines().filter_map(|l| l.strip_prefix("FALL ")) {
        let k = kv(rest);
        let t: i32 = k["t"].parse().expect("t=");
        let Some(o) = ours.get(&t) else { continue };
        compared += 1;
        let r = TargetPs {
            command_time: k["ct"].parse().expect("ct="),
            origin: floats(k["origin"]),
            velocity: floats(k["vel"]),
            pm_flags: i32::from_str_radix(k["pm_flags"].trim_start_matches("0x"), 16)
                .expect("pm_flags="),
            pm_time: k["pm_time"].parse().expect("pm_time="),
            health: k["health"].parse().expect("health="),
        };
        let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() <= 0.001);
        if r.command_time != o.command_time
            || r.pm_flags != o.pm_flags
            || r.pm_time != o.pm_time
            || r.health != o.health
            || !near(r.origin, o.origin)
            || !near(r.velocity, o.velocity)
        {
            rows.push(format!("[target ps] t={t}: retail {r:?} ours {o:?}"));
        }
    }
    // The wound and the kill each move health and pm_time.
    assert!(
        compared >= 2,
        "{compared} FALL lines on the target's phases"
    );
    rows
}

/// Fails on any row no `gaps` entry names, and on any entry that named no
/// row, so the list cannot outlive what it excuses.
fn finish(rows: Vec<String>, gaps: &[(&str, &str)]) {
    let (gapped, real): (Vec<String>, Vec<String>) = rows
        .into_iter()
        .partition(|r| gaps.iter().any(|(g, _)| r.contains(g)));
    if report() {
        for r in &gapped {
            let why = gaps.iter().find(|(g, _)| r.contains(g)).unwrap().1;
            println!("gap: {r}\n     {why}");
        }
        for r in &real {
            println!("{r}");
        }
    }
    let stale: Vec<&str> = gaps
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

/// The mount, sweep, fire and release recipe (turrets doc 12) with the
/// target's entity logged (`!victim`) on
/// each fire and target snapshot: its `pos.trTime` drives the target's cmd
/// clock and its trajectory, anims and `solid` are diffed, with every
/// other phase. Ours starts at the gunner's `exact=` origin and runs on
/// retail's level clock off the `-script` half's `PROBE place`, so the
/// target's run clip meets the kill round at retail's phase, each gunner
/// cmd on the frame its `ct=` names (`docs/research/cod11-turrets.md` 13.1,
/// 13.3).
#[test]
fn the_victim_capture_matches_retail_on_mp_carentan() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let cap = parse(&read(FIXTURE_VICTIM));
    let place = place_ms(read(FIXTURE_VICTIM_SCRIPT).lines(), 0).expect("retail's PROBE place");
    let Some(mut rig) = rig_aligned(
        &cap,
        &vcod_common::testing::game_fs_11,
        &PROTOCOL_V1,
        REBASE_MAX_VICTIM,
        place,
        None,
    ) else {
        return;
    };
    let ours = replay(&mut rig, &cap);
    let fs = vcod_common::testing::game_fs_11().unwrap();
    finish(diff(&cap, &ours, &fs), &[]);
}

/// The phases after the 2026-10-19 run's dismount, which puts the gunner
/// back on his spot: the 142 step off the sandbags and the slide down the
/// face (turrets doc 13.3). Its sweep logs no `ct=`, so which frame ran a
/// cmd stamped at a snapshot's own time is unknown there; the `-victim-ct`
/// gate above holds every phase.
const AFTER_DISMOUNT: [&str; 5] = ["dismount", "crouch", "uncrouch", "strafe", "refused"];

#[test]
fn the_142_step_off_the_sandbags_matches_retail() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let cap = parse(&read(FIXTURE_VICTIM_19));
    assert!(cap.traces.iter().all(|(_, s)| s.ct.is_none()));
    let place = place_ms(read(FIXTURE_VICTIM_19_SCRIPT).lines(), 0).expect("retail's PROBE place");
    let Some(mut rig) = rig_aligned(
        &cap,
        &vcod_common::testing::game_fs_11,
        &PROTOCOL_V1,
        REBASE_MAX_VICTIM,
        place,
        None,
    ) else {
        return;
    };
    let ours = replay(&mut rig, &cap);
    let fs = vcod_common::testing::game_fs_11().unwrap();
    let rows = diff(&cap, &ours, &fs)
        .into_iter()
        .filter(|r| {
            AFTER_DISMOUNT
                .iter()
                .any(|p| r.starts_with(&format!("[{p}]")))
        })
        .collect();
    finish(rows, &[]);
}

/// The recipe rerun once more with the gsc logging each placement's level
/// time and the target logging its own cmds (`--probe-fall`), as
/// `mp_carentan-dm-turret-<tag>.txt` and its `-script` and `-target`
/// halves: ours replays it on retail's level clock (`rig_aligned`), the
/// target's cmds at their own stamps, and diffs every phase. `fs` is the
/// install's paks, `None` where the capture's server is not the install's.
fn spawn_gate(
    tag: &str,
    fs: impl Fn() -> Option<Pk3Fs>,
    p: &'static Protocol,
    gaps: &[(&str, &str)],
) {
    if fs().is_none() {
        return;
    }
    let path = |half: &str| {
        format!(
            "{}/tests/fixtures/turret/mp_carentan-dm-turret-{tag}{half}.txt",
            env!("CARGO_MANIFEST_DIR")
        )
    };
    let cap = Capture {
        target_cmds: target_cmds(&read(&path("-target"))),
        target_spawn: spawn_at(&read(&path("-script"))).or(Some(PRE_SPAWNAT)),
        ..parse(&read(&path("")))
    };
    let place = place_ms(read(&path("-script")).lines(), 0).expect("retail's PROBE place");
    let Some(mut rig) = rig_aligned(&cap, &fs, p, REBASE_MAX_VICTIM, place, None) else {
        return;
    };
    if p.transport.version == PROTOCOL_V22.transport.version {
        rig.sv.pin_tracer_rolls(tracer_rolls(&cap));
    }
    let ours = replay(&mut rig, &cap);
    let mut rows = diff(&cap, &ours, &fs().unwrap());
    rows.extend(target_rows(&cap, &read(&path("-target")), &ours));
    rows.extend(hitfine_rows(
        read(&path("-script")).lines(),
        rig.sv.script_log().iter().map(String::as_str),
    ));
    rows.extend(credit_rows(&read(&path("-script")), &rig));
    finish(rows, gaps);
}

/// Retail's UO tracer rolls, one per player hit in hit order: a UO round's
/// only server impact is a player hit's, and a roll that came up sends
/// `EV_BULLET_TRACER` in the same snapshot (`coduo-diff.md` 21). Ours draws
/// off its own seed, so the replay takes retail's outcomes.
fn tracer_rolls(cap: &Capture) -> Vec<bool> {
    let mut out = Vec::new();
    for (_, s) in &cap.traces {
        let hits = s.impacts.len();
        let tracers = s.events.iter().filter(|e| e.0 == EV_BULLET_TRACER).count();
        assert!(
            tracers <= hits && (hits <= 1 || tracers % hits == 0),
            "t={}: {tracers} tracers on {hits} hits",
            s.t
        );
        out.extend((0..hits).map(|i| i < tracers));
    }
    out
}

/// The kill credit: ours logs the `D;` and `K;` records retail's server log
/// carries, the wound on `m1carbine_mp` and the kill on the gun (turrets doc
/// 13.1).
fn credit_rows(script: &str, rig: &Rig) -> Vec<String> {
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
    if ours == retail {
        Vec::new()
    } else {
        vec![format!("[credit] retail {retail:?} ours {ours:?}")]
    }
}

/// The hit-grid captures: the `-spawn` recipe with `probe_hitlog` and
/// `probe_hitgrid`, whose grids read the pose each round met
/// (`cod11-turrets.md` 13.6). Ours replays it as [`spawn_gate`] does, with
/// `cvars` set, and is held to the target's playerstate, every hit's point
/// and every grid cell; the gunner's own rows are the `-spawn` gates'.
fn hit_gate(
    tag: &str,
    fs: impl Fn() -> Option<Pk3Fs>,
    p: &'static Protocol,
    cvars: &'static [(&'static str, &'static str)],
    gaps: &[(&str, &str)],
) {
    if fs().is_none() {
        return;
    }
    let path = |half: &str| {
        format!(
            "{}/tests/fixtures/turret/mp_carentan-dm-turret-{tag}{half}.txt",
            env!("CARGO_MANIFEST_DIR")
        )
    };
    let script = read(&path("-script"));
    let cap = Capture {
        target_cmds: target_cmds(&read(&path("-target"))),
        cvars,
        target_spawn: spawn_at(&script).or(Some(PRE_SPAWNAT)),
        ..parse(&read(&path("")))
    };
    let place = place_ms(script.lines(), 0).expect("retail's PROBE place");
    let Some(mut rig) = rig_aligned(&cap, &fs, p, REBASE_MAX_VICTIM, place, None) else {
        return;
    };
    let ours = replay(&mut rig, &cap);
    let mut rows = target_rows(&cap, &read(&path("-target")), &ours);
    let log = rig.sv.script_log();
    rows.extend(hitfine_rows(script.lines(), log.iter().map(String::as_str)));
    rows.extend(hitgrid_rows(script.lines(), log.iter().map(String::as_str)));
    finish(rows, gaps);
}

/// Client 0's first `PROBE spawnat <num> <origin> <angles>` line.
/// The target's spawn for a capture that predates `PROBE spawnat`: the
/// spawn think's drift off this sloped point puts ours on retail's origin
/// to 0.001 on every such capture, where the script's own pick (off a
/// clock-seeded `holdrand`, map-cycle doc 4.8) can leave him a third of a
/// unit off. INFERRED: retail's target spawned here.
const PRE_SPAWNAT: common::SpawnAt = common::CARENTAN_NORTH;

fn spawn_at(script: &str) -> Option<([f32; 3], [f32; 3])> {
    let rest = script
        .lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.split_once("PROBE spawnat 0 ").map(|(_, r)| r))?;
    let v: Vec<f32> = rest
        .split(['(', ')', ','])
        .filter_map(|x| x.trim().parse().ok())
        .collect();
    Some(([v[0], v[1], v[2]], [v[3], v[4], v[5]]))
}

/// A `fine1` number: `<int>:<thousandths>:<millionths of one>`.
fn fine(c: &str) -> f64 {
    let (neg, c) = c.strip_prefix('-').map_or((false, c), |c| (true, c));
    let p: Vec<f64> = c.split(':').map(|x| x.parse().expect("a number")).collect();
    let v = p[0] + p[1] / 1e3 + p[2] / 1e9;
    if neg { -v } else { v }
}

/// A `probe_hitlog` line: time, victim, location and damage, then the point
/// and the direction.
fn hitfine<'a>(log: impl Iterator<Item = &'a str>) -> Vec<(String, [f64; 3], [f64; 3])> {
    log.filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once("PROBE hitfine ").map(|(_, r)| r))
        .map(|r| {
            let f: Vec<&str> = r.split_whitespace().collect();
            let v = |s: &str| {
                let c: Vec<f64> = s.split(',').map(fine).collect();
                [c[0], c[1], c[2]]
            };
            (f[..4].join(" "), v(f[4]), v(f[5]))
        })
        .collect()
}

/// How far a hit's point a gate names in its gaps may sit from retail's,
/// units: what is left on a knockback's first frame, 2e-4 at most
/// (`cod11-turrets.md` 13.6). Every other hit is retail's to the bit.
const HIT_NEAR: f64 = 0.001;
/// How far a hit-grid cell may sit from retail's along its ray, units:
/// 0.0011 at most, on 1.1d's knockback frame (13.6).
const GRID_NEAR: f64 = 0.005;
/// How far a gunner-grid cell may: every cell of both captures sits
/// within 0.0001 of retail's (cod11-turrets.md 13.9).
const GUNGRID_NEAR: f64 = 0.0005;

/// The captures that log `probe_hitlog`: every hit at retail's time, on the
/// same victim, location and damage, its point retail's as `fine1` prints
/// it. A row off it reads `[hit] <time> ...` for a gap to name and
/// `[hit-far]` past [`HIT_NEAR`], which no gap covers. How far each point
/// sits from retail's prints under `TURRET_REPORT=1` (turrets doc 13.5).
fn hitfine_rows<'a>(
    theirs: impl Iterator<Item = &'a str>,
    mine: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    let (theirs, mine) = (hitfine(theirs), hitfine(mine));
    if theirs.is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for (i, (key, at, _)) in theirs.iter().enumerate() {
        match mine.get(i) {
            Some((k, m, _)) if k == key => {
                let d = [m[0] - at[0], m[1] - at[1], m[2] - at[2]];
                if report() {
                    eprintln!("hitfine {key}: ours - retail = {d:?}");
                }
                let off = d.iter().map(|x| x * x).sum::<f64>().sqrt();
                if off > HIT_NEAR {
                    rows.push(format!("[hit-far] {key}: ours - retail = {d:?}"));
                } else if off > 0.0 {
                    rows.push(format!("[hit] {key}: ours - retail = {d:?}"));
                }
            }
            other => rows.push(format!("[hit] {key}: ours {:?}", other.map(|o| &o.0))),
        }
    }
    if mine.len() > theirs.len() {
        rows.push(format!("[hit] ours only {:?}", &mine[theirs.len()..]));
    }
    rows
}

/// Every `probe_hitgrid` row against retail's, cell by cell, each a
/// distance along the round from the server's own hit point moved back 48:
/// both turn into a distance from a common origin through that point's
/// projection on the round, so a cell compares where its ray met the body.
fn hitgrid_rows<'a>(
    theirs: impl Iterator<Item = &'a str> + Clone,
    mine: impl Iterator<Item = &'a str> + Clone,
) -> Vec<String> {
    type Rows = BTreeMap<(i32, i32), Vec<Option<f64>>>;
    let rows_of = |log: &mut dyn Iterator<Item = &'a str>| -> Rows {
        let mut out = Rows::new();
        for r in log
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once("PROBE hitgrid ").map(|(_, r)| r))
        {
            let f: Vec<&str> = r.split_whitespace().collect();
            let cells = f[3..]
                .iter()
                .map(|c| (*c != "-").then(|| fine(c)))
                .collect();
            out.insert((f[0].parse().unwrap(), f[2].parse().unwrap()), cells);
        }
        out
    };
    let along = |log: &mut dyn Iterator<Item = &'a str>| -> BTreeMap<i32, f64> {
        hitfine(log)
            .into_iter()
            .map(|(key, p, d)| {
                let t = key.split(' ').next().unwrap().parse().unwrap();
                (t, p[0] * d[0] + p[1] * d[1] + p[2] * d[2])
            })
            .collect()
    };
    let (rt, ot) = (rows_of(&mut theirs.clone()), rows_of(&mut mine.clone()));
    let (ra, oa) = (along(&mut theirs.clone()), along(&mut mine.clone()));
    let mut rows = Vec::new();
    for ((t, row), cells) in &rt {
        let Some(mine) = ot.get(&(*t, *row)) else {
            rows.push(format!("[grid] t={t} row {row}: retail only"));
            continue;
        };
        for (j, (r, o)) in cells.iter().zip(mine).enumerate() {
            let off = match (r, o) {
                (Some(r), Some(o)) => Some((o + oa[t]) - (r + ra[t])),
                (None, None) => continue,
                _ => None,
            };
            if report() {
                eprintln!("grid t={t} row {row} col {j}: {off:?}");
            }
            if off.is_none_or(|d| d.abs() > GRID_NEAR) {
                rows.push(format!(
                    "[grid] t={t} row {row} col {j}: retail {r:?} ours {o:?}"
                ));
            }
        }
    }
    rows
}

/// Every `probe_gungrid` line on a frame retail's gunner was mounted
/// (`eFlags & 0xc000`) against ours, cell by cell along its ray: the
/// gunner's own pose, his fire leaves included (`cod11-turrets.md` 13.9).
/// A row reads `[gungrid] <phase> t=<time> ...`; a cell past
/// [`GUNGRID_NEAR`] or met by one side only is a row.
fn gungrid_rows<'a>(
    cap: &Capture,
    theirs: impl Iterator<Item = &'a str>,
    mine: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    let lines = |log: &mut dyn Iterator<Item = &'a str>| -> BTreeMap<i32, Vec<Option<f64>>> {
        log.filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once("PROBE gungrid ").map(|(_, r)| r))
            .map(|r| {
                let f: Vec<&str> = r.split_whitespace().collect();
                let cells = f[2..]
                    .iter()
                    .map(|c| (*c != "-").then(|| fine(c)))
                    .collect();
                (f[0].parse().unwrap(), cells)
            })
            .collect()
    };
    let (theirs, mine) = (lines(&mut { theirs }), lines(&mut { mine }));
    let mut rows = Vec::new();
    for (t, cells) in &theirs {
        let Some((phase, _)) = cap
            .traces
            .iter()
            .find(|(_, s)| s.t == *t && s.e_flags & 0xc000 != 0)
        else {
            continue;
        };
        let Some(ours) = mine.get(t) else {
            rows.push(format!("[gungrid] {phase} t={t}: retail only"));
            continue;
        };
        for (j, (r, o)) in cells.iter().zip(ours).enumerate() {
            let off = match (r, o) {
                (None, None) => continue,
                (Some(r), Some(o)) => Some(o - r),
                _ => None,
            };
            if report() {
                eprintln!("gungrid t={t} cell {j}: {off:?}");
            }
            if off.is_none_or(|d| d.abs() > GUNGRID_NEAR) {
                rows.push(format!(
                    "[gungrid] {phase} t={t} cell {j}: retail {r:?} ours {o:?}"
                ));
            }
        }
    }
    rows
}

/// Where the gunner-grid gates spawn the gunner: `probe_turret`'s spot 40
/// behind the gun, on the ground. Retail's capture logs no `spawnat` for
/// him; its first snapshot's `legsAnim` (the idle, toggle set) says his
/// spawn set the idle once, as a spawn on flat ground does, where our
/// random pick spins up a walk first (cod11-turrets.md 13.9).
const GUNNER_SPAWN: Spawn = ([1738.2423, 1860.1884, -23.875], [0.0, 229.0, 0.0]);

/// The gunner-grid captures (`-gungrid-*`): the hit-grid recipe with
/// `probe_gungrid`, replayed as [`hit_gate`] does and held to the gunner's
/// own grid alone; the target's rows are the hit-grid gates'. The gunner
/// joins when retail's did, his idle running from that spawn.
fn gungrid_gate(
    tag: &str,
    fs: impl Fn() -> Option<Pk3Fs>,
    p: &'static Protocol,
    cvars: &'static [(&'static str, &'static str)],
    gaps: &[(&str, &str)],
) {
    if fs().is_none() {
        return;
    }
    let path = |half: &str| {
        format!(
            "{}/tests/fixtures/turret/mp_carentan-dm-turret-{tag}{half}.txt",
            env!("CARGO_MANIFEST_DIR")
        )
    };
    let script = read(&path("-script"));
    let cap = Capture {
        target_cmds: target_cmds(&read(&path("-target"))),
        cvars,
        target_spawn: spawn_at(&script),
        ..parse(&read(&path("")))
    };
    let place = place_ms(script.lines(), 0).expect("retail's PROBE place");
    let gunner = place_ms(script.lines(), 1).expect("retail's PROBE place for the gunner");
    let late = Some((gunner, GUNNER_SPAWN));
    let Some(mut rig) = rig_aligned(&cap, &fs, p, REBASE_MAX_VICTIM, place, late) else {
        return;
    };
    replay(&mut rig, &cap);
    let log = rig.sv.script_log();
    let rows = gungrid_rows(&cap, script.lines(), log.iter().map(String::as_str));
    assert!(
        script.contains("PROBE gungrid ") && log.iter().any(|l| l.contains("PROBE gungrid ")),
        "no gunner grid on one side"
    );
    finish(rows, gaps);
}

#[test]
fn the_spawn_capture_matches_retail_on_mp_carentan() {
    spawn_gate("spawn", vcod_common::testing::game_fs_11, &PROTOCOL_V1, &[]);
}

#[test]
fn the_spawn_capture_matches_15b_on_mp_carentan() {
    spawn_gate(
        "spawn-15",
        || {
            vcod_common::testing::game_fs().filter(|fs| {
                vcod_server::server::install_protocol(fs).transport.version
                    == PROTOCOL_V6.transport.version
            })
        },
        &PROTOCOL_V6,
        &[],
    );
}

#[test]
fn the_spawn_capture_matches_uo_on_mp_carentan() {
    spawn_gate("spawn-uo", vcod_common::testing::uo_fs, &PROTOCOL_V22, &[]);
}

/// The `-spawn-uo` recipe rerun with `probe_hitlog`, whose hits the gate
/// also pairs line by line (turrets doc 13.5).
#[test]
fn the_hitlog_capture_matches_uo_on_mp_carentan() {
    spawn_gate(
        "hitfine-uo",
        vcod_common::testing::uo_fs,
        &PROTOCOL_V22,
        &[("[hit] 30200 0 ", KNOCKBACK_GAP)],
    );
}

/// A kill on the knockback's first frame: the body turned about a
/// hundredth of a degree off retail's, cause not found (`cod11-turrets.md`
/// 13.6).
const KNOCKBACK_GAP: &str = "the knockback's first frame's pose (turrets doc 13.6)";

/// The 1.1d hit-grid capture's head: the capture predates the gsc's
/// `PROBE models` line, and its head cells read the boxes of the `a` axis
/// heads (`xmodelparts/basehead_axis11`) to 0.002 where ours' own draw, a
/// `b` head, reads them 0.12 off (cod11-turrets.md 13.6).
const HEAD_HITGRID: &[(&str, &str)] =
    &[("probe_hitgrid", "1"), ("probe_head", "basehead_axis_a_01")];
/// The other hit-grid captures' cvars.
const HITGRID: &[(&str, &str)] = &[("probe_hitgrid", "1")];
/// The gunner-grid captures' cvars: the hit-grid recipe with
/// `probe_gungrid`, the target wearing the head retail drew him.
const GUNGRID_UO: &[(&str, &str)] = &[
    ("probe_hitgrid", "1"),
    ("probe_gungrid", "1"),
    ("probe_head", "basehead_axis_a_02"),
];
const GUNGRID_15: &[(&str, &str)] = &[
    ("probe_hitgrid", "1"),
    ("probe_gungrid", "1"),
    ("probe_head", "basehead_axis_a_09"),
];

/// A firing gunner's own pose on UO, every frame he mans the gun: his fire
/// leaves on their sync group's node time, and the aim group blending out
/// on its own leaves (cod11-turrets.md 13.9).
#[test]
fn the_gunner_grid_capture_matches_uo_on_mp_carentan() {
    gungrid_gate(
        "gungrid-uo",
        vcod_common::testing::uo_fs,
        &PROTOCOL_V22,
        GUNGRID_UO,
        &[],
    );
}

#[test]
fn the_gunner_grid_capture_matches_15b_on_mp_carentan() {
    gungrid_gate(
        "gungrid-15",
        || {
            vcod_common::testing::game_fs().filter(|fs| {
                vcod_server::server::install_protocol(fs).transport.version
                    == PROTOCOL_V6.transport.version
            })
        },
        &PROTOCOL_V6,
        GUNGRID_15,
        &[],
    );
}

#[test]
fn the_hitgrid_capture_matches_retail_on_mp_carentan() {
    hit_gate(
        "hitgrid",
        vcod_common::testing::game_fs_11,
        &PROTOCOL_V1,
        HEAD_HITGRID,
        &[("[hit] 30450 0 ", KNOCKBACK_GAP)],
    );
}

#[test]
fn the_hitgrid_capture_matches_15b_on_mp_carentan() {
    hit_gate(
        "hitgrid-15",
        || {
            vcod_common::testing::game_fs().filter(|fs| {
                vcod_server::server::install_protocol(fs).transport.version
                    == PROTOCOL_V6.transport.version
            })
        },
        &PROTOCOL_V6,
        HITGRID,
        &[],
    );
}

#[test]
fn the_hitgrid_capture_matches_uo_on_mp_carentan() {
    hit_gate(
        "hitgrid-uo",
        vcod_common::testing::uo_fs,
        &PROTOCOL_V22,
        HITGRID,
        &[("[hit] 30450 0 ", KNOCKBACK_GAP)],
    );
}

/// The UO hit-grid recipe rerun with the target's spawn point logged and
/// pinned, and once more with the target placed on the ground instead of
/// falling onto it: every cell matches either way (turrets doc 13.7).
const HITGRID_SPAWNLOG: &[(&str, &str)] = &[("probe_hitgrid", "1"), ("probe_spawnlog", "1")];
const HITGRID_GROUND: &[(&str, &str)] = &[
    ("probe_hitgrid", "1"),
    ("probe_spawnlog", "1"),
    ("probe_ground", "1"),
];

#[test]
fn the_spawnlog_hitgrid_capture_matches_uo_on_mp_carentan() {
    hit_gate(
        "hitgrid-uo-spawn2",
        vcod_common::testing::uo_fs,
        &PROTOCOL_V22,
        HITGRID_SPAWNLOG,
        &[],
    );
}

#[test]
fn the_grounded_hitgrid_capture_matches_uo_on_mp_carentan() {
    hit_gate(
        "hitgrid-uo-ground",
        vcod_common::testing::uo_fs,
        &PROTOCOL_V22,
        HITGRID_GROUND,
        &[],
    );
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
    // 1.5 and UO put a guid ahead of each client number.
    let fields = if f.len() == 13 {
        [0, 3, 7, 9, 10, 11, 12]
    } else {
        [0, 2, 5, 7, 8, 9, 10]
    };
    Some(fields.map(|i| f.get(i).copied().unwrap_or("?")).join(";"))
}

/// 1.5's `turret_think_client` (0x62305) runs the aim, the gunner's
/// placement (0x61389) and the fire only outside a match timeout (turrets
/// doc 14.3): a gunner moved off his spot while the clock stands stays
/// where he was put, and the placement takes him back once it runs again.
#[test]
fn a_15_match_timeout_holds_the_gunner_where_he_stands() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    let cvars = [
        ("g_timeoutsAllowed", "1"),
        ("g_timeoutLength", "1000"),
        ("g_timeoutRecovery", "500"),
        ("g_timeoutBank", "180000"),
    ];
    let mut rig = build_on(
        fs,
        &PROTOCOL_V6,
        &cvars,
        "m1carbine_mp",
        false,
        None,
        0,
        None,
        &[],
        None,
    )
    .unwrap();
    rig.tap(BUTTON_USE);
    let placed = rig.hold(5);
    assert_eq!(placed.e_flags & 0xC000, 0xC000, "mounted");
    // `dm` puts no one on a side, which the timeout asks for.
    rig.sv.test_set_client_team(0, "allies");
    let frozen = rig.command("matchtimeout");
    let off = [placed.origin[0] + 8.0, placed.origin[1], placed.origin[2]];
    rig.sv.test_set_client_origin(0, off);
    let held = rig.hold(4);
    assert_eq!(held.t, frozen.t, "the clock stands");
    assert!(
        dist(held.origin, off) < 0.01,
        "placed back at {:?} with the clock stood, moved to {off:?}",
        held.origin
    );
    let mut back = held.clone();
    for _ in 0..60 {
        back = rig.hold(1);
        if back.t != held.t {
            break;
        }
    }
    assert_ne!(back.t, held.t, "the clock never ran again");
    assert!(
        dist(back.origin, placed.origin) < 0.01,
        "{:?} with the clock running, placed at {:?}",
        back.origin,
        placed.origin
    );
}

/// `mg42_bipod_stand_mp`'s turret keys, as the stock weapon file carries
/// them (turrets doc 2).
const STAND_FILE: &str = "WEAPONFILE\\weaponClass\\turret\\weaponType\\bullet\\damage\\60\\fireTime\\0.05\\leftArc\\45\\rightArc\\45\\topArc\\40\\bottomArc\\40\\stance\\stand";

/// 1.5b's and UO 1.51b's `--save-turret` captures through the manned aim
/// (turrets doc 14.3, coduo-diff.md 19): on every mounted snapshot the
/// barrel is the snapshot's view off the gun clamped to the arcs, with no
/// 15-degree step and no view forced onto it. The view runs past the arcs
/// with `delta_angles` left alone, and 1.1's rule would have stepped the
/// barrel on the flick and the sweeps' edges. Needs no paks.
#[test]
fn the_1_5_and_uo_barrels_follow_the_clamped_view() {
    use vcod_server::game::turret::{AimRule, TurretDef, TurretKeys, TurretRecord, aim};
    for (file, rule) in [
        ("mp_carentan-dm-turret-15.txt", AimRule::V15),
        ("mp_carentan-dm-turret-uo.txt", AimRule::Uo),
    ] {
        let text = read(&format!(
            "{}/tests/fixtures/turret/{file}",
            env!("CARGO_MANIFEST_DIR")
        ));
        let cap = parse(&text);
        let gun_angles: [f32; 3] = text
            .lines()
            .find_map(|l| l.strip_prefix("# turret "))
            .map(|l| floats(kv(l)["angles"]))
            .expect("the gun's angles in the header");
        let def = TurretDef::parse(STAND_FILE).unwrap();
        let mut ours = TurretRecord::new("mg42_bipod_stand_mp", def, TurretKeys::default(), 0.0);
        let mut v1 = ours.clone();
        let (mut mounted, mut past_arc, mut stepped) = (0, 0, 0);
        let (mut delta, mut prev) = (None, None);
        for (phase, s) in &cap.traces {
            if s.viewlocked_ent != cap.gun as i32 || s.e_flags & 0xC000 == 0 {
                continue;
            }
            mounted += 1;
            let view = s.viewangles;
            assert_eq!(
                aim(&mut ours, rule, view, gun_angles),
                None,
                "{file} [{phase}] t={}: a forced view",
                s.t
            );
            for i in 0..2 {
                let off = angle_off(ours.angles2[i], s.turret_angles2[i]);
                assert!(
                    off <= PRINT_EPS + ANGLES2_EPS,
                    "{file} [{phase}] t={}: view {view:?}, retail's barrel {:?}, ours {:?}",
                    s.t,
                    s.turret_angles2,
                    ours.angles2
                );
            }
            if (0..2)
                .any(|i| angle_off(angle_subtract(view[i], gun_angles[i]), ours.angles2[i]) > 1.0)
            {
                past_arc += 1;
            }
            // 1.1's step from the barrel retail's previous snapshot held.
            v1.angles2 = prev.unwrap_or(s.turret_angles2);
            prev = Some(s.turret_angles2);
            aim(&mut v1, AimRule::V1, view, gun_angles);
            if (0..2).any(|i| angle_off(v1.angles2[i], s.turret_angles2[i]) > 1.0) {
                stepped += 1;
            }
            // Nothing but a remount's `SetClientViewAngle` moves the view.
            if phase != "crouch" && phase != "mount" {
                assert_eq!(
                    *delta.get_or_insert(s.delta_angles),
                    s.delta_angles,
                    "{file} [{phase}] t={}",
                    s.t
                );
            }
        }
        println!("{file}: {mounted} mounted, {past_arc} past an arc, {stepped} 1.1 would step");
        assert!(mounted > 100, "{file}: {mounted} mounted snapshots");
        assert!(past_arc > 10, "{file}: the view never ran past an arc");
        assert!(stepped > 0, "{file}: 1.1's rule never differed");
    }
}

/// The round's pass entity is the gun, not the gunner (turrets doc 6.3), so
/// nothing but the muzzle's lead keeps a gunner out of his own line. At the
/// corners and the middle of the stand, duck and prone guns' arcs he never
/// takes his own round.
#[test]
fn a_gunner_never_meets_his_own_round() {
    for info in [
        "mg42_bipod_stand_mp",
        "mg42_bipod_duck_mp",
        "mg42_bipod_prone_mp",
    ] {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let mut rig = build_on(
            fs,
            &PROTOCOL_V1,
            &[],
            "m1carbine_mp",
            false,
            Some(info),
            0,
            None,
            &[],
            None,
        )
        .unwrap();
        rig.tap(BUTTON_USE);
        assert_eq!(
            rig.hold(1).viewlocked_ent,
            rig.gun as i32,
            "{info}: mounted"
        );
        let yaw = rig.gun_yaw();
        for (pitch, turn) in [
            (0.0, 0.0),
            (-40.0, -45.0),
            (-40.0, 45.0),
            (40.0, -45.0),
            (40.0, 45.0),
        ] {
            for _ in 0..4 {
                rig.look([pitch, yaw + turn]);
            }
            let h = rig.still();
            let fire = UserCmd {
                buttons: h.buttons | BUTTON_ATTACK,
                ..h
            };
            assert_eq!(rig.frame([fire, fire]).viewlocked, 2, "{info}: fired");
            rig.hold(1);
            let health = rig.gunner.snapshots().newest().unwrap().ps.health();
            assert_eq!(health, 100, "{info}: pitch {pitch} turn {turn}");
        }
    }
}

/// The origin and angles a `PROBE seat` line prints, `(x, y, z) (p, y, r)`,
/// out of `log`.
fn seat_read<'a>(mut log: impl Iterator<Item = &'a str>) -> Option<String> {
    let l = log.find(|l| l.starts_with("PROBE seat "))?;
    Some(l[l.find('(')?..].trim_end().to_string())
}

/// [`seat_read`]'s `PROBE seatnext`, the read a frame after the spawn.
fn seatnext_read<'a>(mut log: impl Iterator<Item = &'a str>) -> Option<String> {
    let l = log.find(|l| l.starts_with("PROBE seatnext "))?;
    Some(l[l.find('(')?..].trim_end().to_string())
}

/// `ClientSpawn` lets go of the gun before its think (turrets doc 8.1): a
/// gunner the script spawns off the gun reads the spawn's think origin in
/// the spawning thread, as retail's, and his spawn frame carries the
/// release's teleport pair. Its angles are the think's view
/// (cod11-gametypes-re-bel.md 8.6): 0 on 1.1, the yaw through
/// `SHORT2ANGLE` on UO. A frame later, on the cmds retail's next frame ran,
/// the script reads retail's `PROBE seatnext`.
fn a_gunner_spawned_off_the_gun(fs: Pk3Fs, p: &'static Protocol, fixture: &str) {
    let mut rig = build_on(
        fs,
        p,
        &[("probe_respawn", "1")],
        "m1carbine_mp",
        true,
        None,
        0,
        None,
        &[],
        None,
    )
    .unwrap();
    rig.tap(BUTTON_USE);
    assert_eq!(rig.hold(1).e_flags & 0xC000, 0xC000, "mounted");
    let mut spawned = None;
    for _ in 0..80 {
        let s = rig.hold(1);
        if s.e_flags & 0xC000 == 0 {
            spawned = Some(s);
            break;
        }
    }
    let s = spawned.expect("spawned off the gun");
    let theirs = seat_read(fixture.lines()).expect("retail's PROBE seat");
    let mine = seat_read(rig.sv.script_log().iter().map(String::as_str)).expect("our PROBE seat");
    assert_eq!(mine, theirs, "the script's read after the spawn");
    let retail = fixture
        .lines()
        .filter(|l| l.starts_with("!trace "))
        .find(|l| l.contains(" eFlags=24 "))
        .and_then(|l| l.split(" origin=").nth(1))
        .and_then(|o| o.split(' ').next())
        .map(|o| {
            o.split(',')
                .map(|x| x.parse::<f32>().unwrap())
                .collect::<Vec<_>>()
        })
        .expect("retail's spawn frame");
    assert!(
        (0..3).all(|i| (s.origin[i] - retail[i]).abs() <= 0.051),
        "spawn frame at {:?}, retail's {retail:?}",
        s.origin
    );
    assert_eq!((s.viewlocked, s.viewlocked_ent), (0, 0));
    let temps: Vec<i32> = s
        .events
        .iter()
        .filter(|(_, _, w)| w == "temp")
        .map(|e| e.0)
        .collect();
    assert_eq!(temps, [200, 199], "the release's teleport pair");
    // The next frame on retail's cmds: the ones its `commandTime` says that
    // frame ran, each as far past the spawn as on retail, at its view.
    let traces: Vec<BTreeMap<&str, &str>> = fixture
        .lines()
        .filter_map(|l| l.strip_prefix("!trace "))
        .map(kv)
        .collect();
    let at = |m: &BTreeMap<&str, &str>, k: &str| m[k].parse::<i32>().unwrap();
    let spawn = traces
        .iter()
        .position(|m| m["eFlags"] == "24")
        .expect("retail's spawn frame");
    let (spawn_t, ran) = (
        at(&traces[spawn], "serverTime"),
        at(&traces[spawn + 1], "ct"),
    );
    let weapon = holding(&rig.gunner).weapon;
    let cmds: Vec<UserCmd> = fixture
        .lines()
        .filter_map(|l| l.strip_prefix("!cmd "))
        .map(kv)
        .filter(|m| (spawn_t + 1..=ran).contains(&at(m, "st")))
        .flat_map(|m| {
            let cmd = UserCmd {
                weapon,
                angles: ints(m["view"]),
                ..NULL_USERCMD
            };
            let st = at(&m, "st") - spawn_t + s.t;
            common::stamped(&rig.gunner, &cmd, &[st], i32::MIN, i32::MAX)
        })
        .collect();
    assert!(!cmds.is_empty(), "retail ran cmds in the frame after");
    rig.gunner.send_cmds(&cmds);
    rig.frame_at(&[]);
    let theirs = seatnext_read(fixture.lines()).expect("retail's PROBE seatnext");
    let mine = seatnext_read(rig.sv.script_log().iter().map(String::as_str)).expect("our seatnext");
    assert_eq!(mine, theirs, "the script's read a frame after the spawn");
}

#[test]
fn a_gunner_spawned_off_the_gun_reads_retail_s_origin() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    a_gunner_spawned_off_the_gun(
        fs,
        &PROTOCOL_V1,
        include_str!("fixtures/seatspawn/mp_carentan-turret.txt"),
    );
}

#[test]
fn a_gunner_spawned_off_the_gun_reads_retail_s_origin_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    a_gunner_spawned_off_the_gun(
        fs,
        &PROTOCOL_V22,
        include_str!("fixtures/seatspawn/mp_carentan-turret-uo.txt"),
    );
}

/// `mp_carentan-dm-turret-full.txt`: the recipe once more, its `!trace`
/// lines carrying the gunner's origin and velocity at full precision
/// (`exact=`, `velocity=`). From the strafe off the sandbags to the end
/// (the step onto the 52-degree face, the fall down it and the run on)
/// ours lands on retail's origin to the bit on every snapshot: 1.1d's air
/// move, step scale and slide down a steep face are retail's
/// (cod11-turrets.md 13.3).
#[test]
fn the_fall_off_the_sandbags_matches_retail_to_the_bit() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let text = read(FIXTURE_FULL);
    let exact: BTreeMap<i32, String> = text
        .lines()
        .filter(|l| l.starts_with("!trace "))
        .map(|l| {
            let m = kv(l);
            (m["serverTime"].parse().unwrap(), m["exact"].to_string())
        })
        .collect();
    let cap = parse(&text);
    let strafe = cap
        .traces
        .iter()
        .find(|(phase, _)| phase == "strafe")
        .map(|(_, s)| s.t)
        .unwrap();
    let Some(mut rig) = rig_on(&cap, fs, &PROTOCOL_V1, REBASE_MAX_VICTIM) else {
        return;
    };
    let ours = replay(&mut rig, &cap);
    let mut checked = 0;
    for o in ours.iter().filter(|o| o.t >= strafe) {
        let got = format!("{},{},{}", o.origin[0], o.origin[1], o.origin[2]);
        assert_eq!(got, exact[&o.t], "t={}", o.t);
        checked += 1;
    }
    assert!(checked > 20, "{checked}");
}
