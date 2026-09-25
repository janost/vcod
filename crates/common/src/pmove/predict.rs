//! Client-side prediction's shared half: the sim playerstate rebuilt from a
//! snapshot's wire playerstate, and the per-cmd step the server runs on it.
//! Both are copies of server code (`crates/server/src/spectate.rs`,
//! `crates/server/src/server.rs`); keep them in step until the dedupe.

use super::{weapon, PlayerState, PmInput, Stance};
use crate::movetrace::MoveWorld;
use crate::net::msg::{self, UserCmd};
use crate::net::protocol::{Protocol, ENTITYNUM_NONE};
use crate::weapon::WeaponDef;
use glam::Vec3;

/// ANGLE2SHORT units per degree.
const ANGLE2SHORT: f32 = 65536.0 / 360.0;
/// What `Pmove` keeps of a long cmd; the rest is dropped.
const MAX_PMOVE_ARREARS_MS: i32 = 1000;

const PM_NORMAL: i32 = 0;
const PM_NORMAL_LINKED: i32 = 1;
const PM_DEAD_LINKED: i32 = 7;

const EF_CROUCH: i32 = 0x20;
const EF_PRONE: i32 = 0x40;
/// The mounted-gun bits, one value per gun stance (docs/research/cod11-turrets.md 4.4).
const EF_MOUNTED: i32 = 0xC000;
const PMF_DUCKED: i32 = 0x2;
const PMF_JUMP_HELD: i32 = 0x8;
const PMF_BACKWARDS_RUN: i32 = 0x40;

/// The sim playerstate plus the wire fields the step reads and writes that
/// `PlayerState` does not carry.
#[derive(Clone, Copy, Debug)]
pub struct Predicted {
    pub ps: PlayerState,
    pub pm_type: i32,
    /// Raw 16-bit wire values.
    pub delta_angles: [i32; 3],
    pub command_time: i32,
    /// `viewHeightLerpTime`; 0 while the eye is settled.
    pub view_lerp_start: i32,
    /// `eventSequence`, kept in the wire's 8 bits.
    pub event_sequence: i32,
    pub events: [i32; 4],
    pub event_parms: [i32; 4],
}

/// Whether a `pm_type` is one the client predicts: normal and linked.
pub fn predictable(pm_type: i32) -> bool {
    matches!(pm_type, PM_NORMAL | PM_NORMAL_LINKED)
}

/// The inverse of `ClientSim::to_wire`. `last_cmd` is the client's own cmd
/// whose `server_time` equals `commandTime`, which is the only source for the
/// few sim fields the wire does not carry; `None` leaves them at their spawn
/// values.
pub fn from_wire(p: &Protocol, w: &msg::PlayerState, last_cmd: Option<&UserCmd>) -> Predicted {
    let int = |name: &str| w.field_i32(p, name);
    let float = |name: &str| w.field_f32(p, name);
    // The wire reads signed fields back unsigned.
    let s16 = |name: &str| i32::from(int(name) as u16 as i16);
    let s8 = |name: &str| i32::from(int(name) as u8 as i8);
    let vec3 = |name: &str| {
        Vec3::new(
            float(&format!("{name}[0]")),
            float(&format!("{name}[1]")),
            float(&format!("{name}[2]")),
        )
    };
    let u64_pair = |lo: &str, hi: &str| u64::from(int(lo) as u32) | u64::from(int(hi) as u32) << 32;

    let pm_type = int("pm_type");
    let command_time = int("commandTime");
    let view_lerp_start = int("viewHeightLerpTime");
    let view = w.viewangles(p);
    let mut ps = PlayerState::spawn(Vec3::from(w.origin(p)), view[1]);
    ps.pitch = (-view[0]).to_radians();
    ps.velocity = vec3("velocity");

    let eflags = int("eFlags");
    ps.stance = if eflags & EF_PRONE != 0 {
        Stance::Prone
    } else if eflags & EF_CROUCH != 0 {
        Stance::Crouch
    } else {
        Stance::Stand
    };
    ps.mounted = match eflags & EF_MOUNTED {
        0xC000 => Some(Stance::Stand),
        0x8000 => Some(Stance::Crouch),
        0x4000 => Some(Stance::Prone),
        _ => None,
    };
    let ground = int("groundEntityNum") as u32;
    ps.on_ground = ground != ENTITYNUM_NONE;
    if ps.on_ground {
        ps.ground_entity = ground;
    }
    ps.lean = float("leanf") * super::LEAN_MAX;
    ps.prone_direction = float("proneDirection");
    ps.movement_dir = s8("movementDir");
    ps.bob_cycle = int("bobCycle") as u8;

    let pm_flags = int("pm_flags");
    ps.ducked = pm_flags & PMF_DUCKED != 0;
    ps.jump_latched = pm_flags & PMF_JUMP_HELD != 0;
    ps.backwards_run = pm_flags & PMF_BACKWARDS_RUN != 0;
    ps.ads_active = pm_flags & weapon::PMF_ADS != 0;

    ps.view_lerp_target = s8("viewHeightLerpTarget") as f32;
    ps.view_lerp_down = int("viewHeightLerpDown") != 0;
    ps.view_height_cur = float("viewHeightCurrent");
    if view_lerp_start != 0 {
        ps.view_height_speed = lerp_speed(
            ps.stance.view_height(),
            ps.view_height_cur,
            ps.ducked,
            command_time - view_lerp_start,
        );
    }

    ps.weapon = int("weapon") as u8;
    ps.weapons_held = u64_pair("weapons[0]", "weapons[1]");
    let lo = int("weaponslots[0]").to_le_bytes();
    let hi = int("weaponslots[4]").to_le_bytes();
    ps.weapon_slots[..4].copy_from_slice(&lo);
    ps.weapon_slots[4..].copy_from_slice(&hi);
    ps.weaponstate = int("weaponstate") as u8;
    ps.weapon_time_ms = s16("weaponTime");
    ps.weapon_delay_ms = s16("weaponDelay");
    ps.weap_anim = int("weapAnim");
    ps.ammo = w.arrays.ammo;
    ps.ammoclip = w.arrays.ammoclip;
    ps.weapon_rechamber = u64_pair("weaponrechamber[0]", "weaponrechamber[1]");
    ps.weapon_pos_frac = float("fWeaponPosFrac");
    ps.aim_spread_scale = float("aimSpreadScale");
    ps.grenade_time_left_ms = s16("grenadeTimeLeft");
    ps.linked = pm_type == PM_NORMAL_LINKED || pm_type == PM_DEAD_LINKED;

    if let Some(cmd) = last_cmd {
        ps.last_cmd_angles = [cmd.angles[0], cmd.angles[1]];
        ps.last_cmd_ads = cmd.buttons & msg::BUTTON_ADS != 0;
        ps.melee_latched = cmd.buttons & msg::BUTTON_MELEE != 0;
        if ps.weaponstate == weapon::WEAPON_DROPPING {
            ps.pending_weapon = cmd.weapon;
        }
    }

    // `pml.previous_origin` and the landing's sampled fall speed.
    ps.move_start = ps.origin;
    ps.air_speed_peak = if ps.on_ground {
        0.0
    } else {
        (-ps.velocity.z).max(0.0)
    };

    Predicted {
        ps,
        pm_type,
        delta_angles: ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"]
            .map(|n| int(n) & 0xffff),
        command_time,
        view_lerp_start,
        event_sequence: int("eventSequence") & 0xff,
        events: ["events[0]", "events[1]", "events[2]", "events[3]"].map(int),
        event_parms: [
            "eventParms[0]",
            "eventParms[1]",
            "eventParms[2]",
            "eventParms[3]",
        ]
        .map(int),
    }
}

/// The eye lerp's pace, which the wire does not carry: `update_stance` fixes
/// it at a stance change as the whole gap from the stance it left over the
/// transition's lerp time. The stance it left is not on the wire either. Into
/// prone, `ducked` names it: entering a crouch sets the flag and entering prone
/// leaves it. Otherwise each stance the eye lies between it and the target is
/// tried, and the one whose lerp puts the eye here after `elapsed_ms` wins; the
/// stamp trails the change by one slice (at most 66 ms), and the two sources a
/// standing target can have are 200 ms or more apart. So a single transition
/// comes out exact; a stance changed again mid-lerp started from a height no
/// stance has and comes out approximate.
fn lerp_speed(target: f32, cur: f32, ducked: bool, elapsed_ms: i32) -> f32 {
    let pace = |from: f32| {
        let ms = if from == super::VIEW_PRONE || target == super::VIEW_PRONE {
            super::VIEW_LERP_PRONE_MS
        } else {
            super::VIEW_LERP_MS
        };
        (target - from).abs() / ms * 1000.0
    };
    if target == super::VIEW_PRONE {
        return pace(if ducked {
            super::VIEW_CROUCH
        } else {
            super::VIEW_STAND
        });
    }
    [super::VIEW_STAND, super::VIEW_CROUCH, super::VIEW_PRONE]
        .into_iter()
        .filter(|&from| from != target && (from - cur) * (target - cur) <= 0.0)
        .map(|from| {
            let speed = pace(from);
            let implied_ms = (from - cur).abs() / speed * 1000.0;
            (speed, (implied_ms - elapsed_ms as f32).abs())
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0.0, |(speed, _)| speed)
}

/// Copy of `pm_input`, `crates/server/src/spectate.rs`; keep in step until
/// the dedupe.
pub fn pm_input(cmd: &UserCmd) -> PmInput {
    PmInput {
        forward: f32::from(cmd.forward) / 127.0,
        right: f32::from(cmd.right) / 127.0,
        jump: cmd.up > 0,
        crouch: cmd.wbuttons & msg::WBUTTON_CROUCH != 0,
        prone: cmd.wbuttons & msg::WBUTTON_PRONE != 0,
        walk_slow: false,
        lean_left: cmd.wbuttons & msg::WBUTTON_LEAN_LEFT != 0,
        lean_right: cmd.wbuttons & msg::WBUTTON_LEAN_RIGHT != 0,
        attack: cmd.buttons & msg::BUTTON_ATTACK != 0,
        melee: cmd.buttons & msg::BUTTON_MELEE != 0,
        reload: cmd.wbuttons & msg::WBUTTON_RELOAD != 0,
        ads: cmd.buttons & msg::BUTTON_ADS != 0,
        use_button: cmd.buttons & msg::BUTTON_USE != 0,
        weapon: cmd.weapon,
        angles: [cmd.angles[0], cmd.angles[1]],
    }
}

fn short_deg(v: i32) -> f32 {
    let deg = v as f32 / ANGLE2SHORT;
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// Copy of `view_angles`, `crates/server/src/spectate.rs`: degrees, wire
/// convention (pitch positive down).
pub fn view_angles(cmd_angles: [i32; 3], delta_angles: [i32; 3]) -> [f32; 3] {
    [
        short_deg(cmd_angles[0] + delta_angles[0]),
        short_deg(cmd_angles[1] + delta_angles[1]),
        short_deg(cmd_angles[2] + delta_angles[2]),
    ]
}

/// One usercmd, the way `replay_moves` (`crates/server/src/server.rs`) runs
/// it: a cmd already run is skipped, arrears past a second are dropped, and
/// the rest is chopped into `MAX_FRAME_MS` steps.
pub fn run_cmd(
    pred: &mut Predicted,
    cmd: &UserCmd,
    world: &MoveWorld,
    weapons: &[Option<WeaponDef>],
) {
    let dt_ms = cmd.server_time.wrapping_sub(pred.command_time);
    if dt_ms <= 0 {
        return;
    }
    let mut base = pred.command_time;
    if dt_ms > MAX_PMOVE_ARREARS_MS {
        base = cmd.server_time - MAX_PMOVE_ARREARS_MS;
    }
    while base != cmd.server_time {
        let msec = (cmd.server_time - base).min(super::MAX_FRAME_MS as i32);
        base += msec;
        let slice = UserCmd {
            server_time: base,
            ..*cmd
        };
        step(pred, &slice, msec as f32 / 1000.0, world, weapons);
    }
    pred.command_time = cmd.server_time;
}

/// Copy of `ClientSim::step`'s player arm, `crates/server/src/spectate.rs`,
/// minus what only the server does; keep in step until the dedupe.
fn step(
    pred: &mut Predicted,
    cmd: &UserCmd,
    dt: f32,
    world: &MoveWorld,
    weapons: &[Option<WeaponDef>],
) {
    // Dead, spectator and intermission states are drawn from the snapshot.
    if !predictable(pred.pm_type) {
        return;
    }
    let view = view_angles(cmd.angles, pred.delta_angles);
    pred.ps.yaw = view[1].to_radians();
    pred.ps.pitch = (-view[0]).to_radians();
    pred.ps.linked = pred.pm_type == PM_NORMAL_LINKED;
    let events = super::pmove(&mut pred.ps, &pm_input(cmd), world, dt, weapons);
    // The prone cone's push on the view (docs/research/cod11-mantle.md, "Prone").
    if pred.ps.view_yaw_correction != 0.0 {
        pred.delta_angles[1] =
            (pred.delta_angles[1] + (pred.ps.view_yaw_correction * ANGLE2SHORT) as i32) & 0xffff;
    }
    pred.view_lerp_start = if pred.ps.view_height_settled() {
        0
    } else if pred.view_lerp_start == 0 {
        cmd.server_time
    } else {
        pred.view_lerp_start
    };
    for e in &events {
        // The fire parm is vcod's internal fuse channel, never sent.
        let parm = match e.event {
            weapon::EV_FIRE_WEAPON | weapon::EV_FIRE_WEAPON_LASTSHOT => 0,
            _ => e.parm,
        };
        let slot = (pred.event_sequence & 3) as usize;
        pred.events[slot] = e.event;
        pred.event_parms[slot] = parm;
        pred.event_sequence = (pred.event_sequence + 1) & 0xff;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_world;
    use crate::net::protocol::{ENTITYNUM_WORLD, PROTOCOL_V1};

    fn set(w: &mut msg::PlayerState, name: &str, v: i32) {
        w.fields[msg::PlayerState::field_index(&PROTOCOL_V1, name).unwrap()] = v;
    }

    fn setf(w: &mut msg::PlayerState, name: &str, v: f32) {
        set(w, name, v.to_bits() as i32);
    }

    /// A player standing on `test_world`'s floor at the origin at `time`.
    fn standing(time: i32) -> msg::PlayerState {
        let mut w = msg::PlayerState::null(&PROTOCOL_V1);
        set(&mut w, "eFlags", 16);
        set(&mut w, "groundEntityNum", ENTITYNUM_WORLD as i32);
        setf(&mut w, "viewHeightCurrent", super::super::VIEW_STAND);
        set(&mut w, "commandTime", time);
        w
    }

    fn cmd(server_time: i32) -> UserCmd {
        UserCmd {
            server_time,
            ..Default::default()
        }
    }

    /// The part of a `Predicted` `to_wire` would send for these tests.
    fn to_wire(pred: &Predicted) -> msg::PlayerState {
        let ps = &pred.ps;
        let mut w = standing(pred.command_time);
        for i in 0..3 {
            setf(&mut w, &format!("origin[{i}]"), ps.origin[i]);
            setf(&mut w, &format!("velocity[{i}]"), ps.velocity[i]);
        }
        let stance = match ps.stance {
            Stance::Stand => 0,
            Stance::Crouch => EF_CROUCH,
            Stance::Prone => EF_PRONE,
        };
        set(&mut w, "eFlags", 16 | stance);
        set(&mut w, "groundEntityNum", ps.ground_entity_num() as i32);
        set(&mut w, "pm_flags", if ps.ducked { PMF_DUCKED } else { 0 });
        setf(&mut w, "viewHeightCurrent", ps.view_height());
        set(&mut w, "viewHeightLerpTarget", ps.view_lerp_target as i32);
        set(&mut w, "viewHeightLerpDown", i32::from(ps.view_lerp_down));
        set(&mut w, "viewHeightLerpTime", pred.view_lerp_start);
        setf(&mut w, "proneDirection", ps.prone_direction);
        for i in 0..3 {
            set(&mut w, &format!("delta_angles[{i}]"), pred.delta_angles[i]);
        }
        w
    }

    #[test]
    fn from_wire_reads_the_mounted_stance_off_eflags() {
        let p = &PROTOCOL_V1;
        for (bits, want) in [
            (0, None),
            (0xC000, Some(Stance::Stand)),
            (0x8000, Some(Stance::Crouch)),
            (0x4000, Some(Stance::Prone)),
        ] {
            let mut w = msg::PlayerState::null(p);
            set(&mut w, "eFlags", 16 | bits);
            assert_eq!(from_wire(p, &w, None).ps.mounted, want, "eFlags {bits:#x}");
        }
    }

    #[test]
    fn from_wire_reads_the_movement_fields() {
        let p = &PROTOCOL_V1;
        let mut w = msg::PlayerState::null(p);
        set(&mut w, "origin[0]", 100.0f32.to_bits() as i32);
        set(&mut w, "eFlags", 16 | 0x40);
        set(&mut w, "groundEntityNum", 1023);
        set(&mut w, "weapon", 10);
        set(&mut w, "weaponslots[0]", i32::from_le_bytes([0, 10, 0, 3]));
        set(&mut w, "delta_angles[1]", 1234);
        set(&mut w, "commandTime", 5000);
        // Signed 16-bit fields arrive unsigned.
        set(&mut w, "weaponTime", 0xffff);
        set(&mut w, "movementDir", 0xd3);
        set(&mut w, "bobCycle", 200);
        let pred = from_wire(p, &w, None);
        assert_eq!(pred.ps.origin.x, 100.0);
        assert_eq!(pred.ps.stance, Stance::Prone);
        assert!(!pred.ps.on_ground);
        assert_eq!(pred.ps.weapon, 10);
        assert_eq!(pred.ps.weapon_slots[1], 10);
        assert_eq!(pred.ps.weapon_slots[3], 3);
        assert_eq!(pred.delta_angles[1], 1234);
        assert_eq!(pred.command_time, 5000);
        assert_eq!(pred.ps.weapon_time_ms, -1);
        assert_eq!(pred.ps.movement_dir, -45);
        assert_eq!(pred.ps.bob_cycle, 200);
    }

    /// A putaway in flight raises the weapon the cmd at `commandTime` asked
    /// for, not weapon 0.
    #[test]
    fn the_cmd_at_command_time_seeds_what_the_wire_lacks() {
        let mut w = standing(5000);
        set(&mut w, "weaponstate", i32::from(weapon::WEAPON_DROPPING));
        let last = UserCmd {
            server_time: 5000,
            buttons: msg::BUTTON_ADS | msg::BUTTON_MELEE,
            weapon: 6,
            angles: [10, 20, 30],
            ..Default::default()
        };
        let pred = from_wire(&PROTOCOL_V1, &w, Some(&last));
        assert_eq!(pred.ps.pending_weapon, 6);
        assert_eq!(pred.ps.last_cmd_angles, [10, 20]);
        assert!(pred.ps.last_cmd_ads);
        assert!(pred.ps.melee_latched);
        set(&mut w, "weaponstate", i32::from(weapon::WEAPON_READY));
        assert_eq!(
            from_wire(&PROTOCOL_V1, &w, Some(&last)).ps.pending_weapon,
            0
        );
    }

    #[test]
    fn a_cmd_already_run_does_nothing() {
        let world = test_world(&[]);
        let world = MoveWorld::bare(&world);
        let before = from_wire(&PROTOCOL_V1, &standing(5000), None);
        for t in [5000, 4990] {
            let mut pred = before;
            let mut c = cmd(t);
            c.forward = 127;
            run_cmd(&mut pred, &c, &world, &[]);
            assert_eq!(format!("{pred:?}"), format!("{before:?}"));
        }
    }

    #[test]
    fn a_long_cmd_is_chopped_and_arrears_dropped() {
        let world = test_world(&[]);
        let world = MoveWorld::bare(&world);
        let travel = |dt: i32| {
            let mut pred = from_wire(&PROTOCOL_V1, &standing(5000), None);
            let mut c = cmd(5000 + dt);
            c.forward = 127;
            run_cmd(&mut pred, &c, &world, &[]);
            assert_eq!(pred.command_time, 5000 + dt);
            pred.ps.origin.length()
        };
        let (second, long) = (travel(1000), travel(2500));
        assert!(second > 100.0, "{second}");
        assert!((long - second).abs() < 1e-3, "{long} vs {second}");
    }

    /// Rebuilding mid-lerp from what the wire carries continues the eye lerp
    /// the uninterrupted run is on.
    fn lerp_continues(first: &[u8], second: u8, rebuild_after_ms: i32) {
        let world = test_world(&[]);
        let world = MoveWorld::bare(&world);
        let mut pred = from_wire(&PROTOCOL_V1, &standing(1000), None);
        let mut t = 1000;
        let run = |pred: &mut Predicted, t: &mut i32, wbuttons: u8, ms: i32| {
            for _ in 0..ms / 8 {
                *t += 8;
                let c = UserCmd {
                    wbuttons,
                    ..cmd(*t)
                };
                run_cmd(pred, &c, &world, &[]);
            }
        };
        for &b in first {
            run(&mut pred, &mut t, b, 600);
        }
        run(&mut pred, &mut t, second, rebuild_after_ms);
        assert_ne!(
            pred.view_lerp_start, 0,
            "the lerp settled before the rebuild"
        );
        let mut rebuilt = from_wire(&PROTOCOL_V1, &to_wire(&pred), None);
        for _ in 0..60 {
            let mut t2 = t;
            run(&mut pred, &mut t, second, 8);
            run(&mut rebuilt, &mut t2, second, 8);
            assert!(
                (pred.ps.view_height() - rebuilt.ps.view_height()).abs() < 0.01,
                "t {t}: {} vs {}",
                pred.ps.view_height(),
                rebuilt.ps.view_height()
            );
            assert_eq!(pred.view_lerp_start, rebuilt.view_lerp_start);
        }
    }

    #[test]
    fn a_rebuild_mid_crouch_keeps_the_eye_lerp() {
        lerp_continues(&[0], msg::WBUTTON_CROUCH, 96);
        lerp_continues(&[0], msg::WBUTTON_CROUCH, 184);
    }

    /// Prone to standing crosses the crouch height, where the eye alone
    /// cannot tell it from crouch to standing.
    #[test]
    fn a_rebuild_mid_stand_up_from_prone_keeps_the_eye_lerp() {
        lerp_continues(&[msg::WBUTTON_PRONE], 0, 96);
        lerp_continues(&[msg::WBUTTON_PRONE], 0, 304);
        lerp_continues(&[0], msg::WBUTTON_PRONE, 304);
        lerp_continues(&[0], msg::WBUTTON_PRONE, 376);
        lerp_continues(&[0], msg::WBUTTON_PRONE, 384);
        lerp_continues(&[0], msg::WBUTTON_PRONE, 392);
        lerp_continues(&[msg::WBUTTON_CROUCH], msg::WBUTTON_PRONE, 304);
    }

    /// Turning a prone view past the cone pushes `delta_angles`.
    #[test]
    fn a_prone_turn_past_the_cone_moves_delta_angles() {
        let world = test_world(&[]);
        let world = MoveWorld::bare(&world);
        let mut pred = from_wire(&PROTOCOL_V1, &standing(1000), None);
        let mut t = 1000;
        for _ in 0..80 {
            t += 8;
            let c = UserCmd {
                wbuttons: msg::WBUTTON_PRONE,
                ..cmd(t)
            };
            run_cmd(&mut pred, &c, &world, &[]);
        }
        assert_eq!(pred.ps.stance, Stance::Prone);
        let before = pred.delta_angles[1];
        t += 8;
        let c = UserCmd {
            wbuttons: msg::WBUTTON_PRONE,
            angles: [0, (150.0 * ANGLE2SHORT) as i32, 0],
            ..cmd(t)
        };
        run_cmd(&mut pred, &c, &world, &[]);
        assert_ne!(pred.delta_angles[1], before);
        assert_eq!(pred.delta_angles[1] & !0xffff, 0);
    }

    /// The grenade's fire event goes into the ring with parm 0, where the
    /// step's own event carries the fuse.
    #[test]
    fn events_fill_the_ring_and_the_fuse_stays_off_it() {
        let world = test_world(&[]);
        let world = MoveWorld::bare(&world);
        let frag = WeaponDef {
            clip_size: 3,
            fire_time: 1.0,
            drop_time: 0.25,
            raise_time: 0.5,
            weapon_type: "grenade".into(),
            fuse_time: 4.0,
            hold_fire_time: 0.6,
            clip_only: true,
            ammo_index: 1,
            clip_index: 1,
            ..WeaponDef::default()
        };
        let weapons = [None, Some(frag)];
        let mut w = standing(1000);
        set(&mut w, "weapon", 1);
        set(&mut w, "weapons[0]", 1 << 1);
        w.set_clip(1, 3);
        set(&mut w, "eventSequence", 254);
        let mut pred = from_wire(&PROTOCOL_V1, &w, None);
        let mut t = 1000;
        for i in 0..100 {
            t += 8;
            let c = UserCmd {
                buttons: if i < 90 { msg::BUTTON_ATTACK } else { 0 },
                weapon: 1,
                ..cmd(t)
            };
            run_cmd(&mut pred, &c, &world, &weapons);
        }
        assert_eq!(pred.event_sequence, 0, "pullback and throw, wrapped");
        assert_eq!(pred.events[2], weapon::EV_PULLBACK_WEAPON);
        assert_eq!(pred.events[3], weapon::EV_FIRE_WEAPON);
        assert_eq!(pred.event_parms[3], 0);
        assert_eq!(pred.ps.ammoclip[1], 2);
    }
}
