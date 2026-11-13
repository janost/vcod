//! One test per CoD2 rule, each against the value the decompile's
//! arithmetic gives (docs/research/cod2-movement.md).

use glam::Vec3;
use vcod_common::collision::{CollisionWorld, ramp_test_world, test_world};
use vcod_common::movetrace::MoveWorld;
use vcod_common::patch::Arith;

use super::*;

fn world(extra: &[(Vec3, Vec3)]) -> CollisionWorld {
    let w = test_world(extra);
    w.set_arith(Arith::Stored);
    w
}

fn cmd(time: i32, buttons: i32, forward: i8, right: i8) -> UserCmd2 {
    UserCmd2 {
        server_time: time,
        buttons,
        forward_move: forward,
        right_move: right,
        ..Default::default()
    }
}

/// Runs `n` 50 ms cmds of `buttons`/`forward` from `ps`'s clock.
fn run(ps: &mut PlayerState, w: &CollisionWorld, n: usize, buttons: i32, forward: i8) {
    let mut old = cmd(ps.command_time, 0, 0, 0);
    for _ in 0..n {
        let c = cmd(ps.command_time + 50, buttons, forward, 0);
        player_step(ps, &c, &mut old, MoveWorld::bare(w), Rules::default(), 0.0);
    }
}

fn standing(w: &CollisionWorld) -> PlayerState {
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0, 1000);
    run(&mut ps, w, 2, 0, 0);
    assert!(ps.on_ground());
    ps
}

#[test]
fn the_clip_backs_off_a_thousandth_of_the_dot_either_way() {
    let n = Vec3::Z;
    let into = clip_velocity(Vec3::new(100.0, 0.0, -100.0), n);
    // backoff = -100 - |-100| * 0.001f, then in + n * -backoff, stored.
    let k = f64::from(f32::from_bits(0x3a83_126f));
    let back = (-(-100.0 - k * 100.0)) as f32;
    assert_eq!(into.z, (-100.0 + f64::from(back)) as f32);
    let out = clip_velocity(Vec3::new(0.0, 0.0, 100.0), n);
    let back = (-(100.0 - k * 100.0)) as f32;
    assert_eq!(out.z, (100.0 + f64::from(back)) as f32);
    // Q3's 1.001 overclip divides out of a plane; CoD2 multiplies by 0.999.
    assert!(out.z > 0.0999 && (out.z - 0.1).abs() < 1e-4, "{}", out.z);
}

#[test]
fn the_planes_sort_by_their_dot_a_tie_ahead_of_the_earlier() {
    let v = Vec3::new(1.0, 0.0, 0.0);
    let planes = [
        Vec3::new(0.5, 0.0, 0.0),
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(0.5, 0.0, 0.0),
        Vec3::new(-1.0, 0.0, 0.0),
    ];
    let (order, least) = slide::permute_planes(v, &planes);
    assert_eq!(&order[..4], &[3, 1, 2, 0]);
    assert_eq!(least, -1.0);
}

#[test]
fn a_turn_past_the_inertia_angle_accelerates_at_most_50() {
    let rules = Rules::default();
    let mut ps = PlayerState {
        velocity: Vec3::new(100.0, 0.0, 0.0),
        old_velocity: [100.0, 0.0],
        ..Default::default()
    };
    let back = Vec3::new(-1.0, 0.0, 0.0);
    // (100 - 150) points back: past 90 degrees, cut to `inertiaMax`.
    assert_eq!(moves::inertia_clamp(&ps, 150.0, back, &rules), 50.0);
    // (100 - 80) still points forward.
    assert_eq!(moves::inertia_clamp(&ps, 80.0, back, &rules), 80.0);
    assert_eq!(moves::inertia_clamp(&ps, 40.0, back, &rules), 40.0);
    ps.pm_type = pm_type::NOCLIP;
    assert_eq!(moves::inertia_clamp(&ps, 150.0, back, &rules), 150.0);
    ps.pm_type = pm_type::NORMAL;
    ps.old_velocity = [0.005, 0.0];
    assert_eq!(moves::inertia_clamp(&ps, 150.0, back, &rules), 150.0);
}

#[test]
fn old_velocity_eases_toward_the_velocity_by_the_frame() {
    let w = world(&[]);
    let mut ps = standing(&w);
    run(&mut ps, &w, 40, 0, 127);
    assert_eq!(ps.velocity.x, 190.0);
    ps.old_velocity = [0.0, 0.0];
    run(&mut ps, &w, 1, 0, 127);
    // old += (v - old) * min(frametime, 1), frametime 0.001f * 50 stored.
    let ft = (f64::from(f32::from_bits(0x3a83_126f)) * 50.0) as f32;
    assert!(
        (ps.old_velocity[0] - 190.0 * ft).abs() < 1e-3,
        "{:?}",
        ps.old_velocity
    );
}

#[test]
fn the_jump_is_the_edge_of_the_key_against_oldcmd() {
    let w = world(&[]);
    let mut ps = standing(&w);
    let mut old = cmd(ps.command_time, button::JUMP, 0, 0);
    let c = cmd(ps.command_time + 50, button::JUMP, 0, 0);
    player_step(
        &mut ps,
        &c,
        &mut old,
        MoveWorld::bare(&w),
        Rules::default(),
        0.0,
    );
    assert!(ps.on_ground(), "a held key does not jump");
    let mut old = cmd(ps.command_time, 0, 0, 0);
    let c = cmd(ps.command_time + 50, button::JUMP, 0, 0);
    let t0 = c.server_time;
    player_step(
        &mut ps,
        &c,
        &mut old,
        MoveWorld::bare(&w),
        Rules::default(),
        0.0,
    );
    assert!(!ps.on_ground());
    assert_ne!(ps.pm_flags & pmf::JUMPING, 0);
    assert_eq!(ps.jump_origin_z, 0.125);
    assert_eq!(ps.jump_time, t0);
    assert_eq!(ps.aim_spread_scale, 64.0);
}

#[test]
fn a_free_jump_peaks_at_the_jump_height() {
    let w = world(&[]);
    let mut ps = standing(&w);
    let z0 = ps.origin.z;
    let mut old = cmd(ps.command_time, 0, 0, 0);
    let mut top = z0;
    for i in 0..30 {
        let b = if i == 0 { button::JUMP } else { 0 };
        let c = cmd(ps.command_time + 50, b, 0, 0);
        player_step(
            &mut ps,
            &c,
            &mut old,
            MoveWorld::bare(&w),
            Rules::default(),
            0.0,
        );
        top = top.max(ps.origin.z);
    }
    // vz = sqrt(2 * 800 * 39) under the slide's averaged gravity.
    assert!((top - z0 - 39.0).abs() < 0.05, "{}", top - z0);
    assert!(ps.on_ground());
    // The landing under 18 above the start runs the 1800 ms slowdown.
    assert_ne!(ps.pm_flags & pmf::JUMPING, 0);
}

#[test]
fn the_jump_step_never_reaches_past_the_jump_height() {
    // A 36-unit block met while rising: the airborne step climbs at most to
    // the jump's start plus `jump_height`, which clears it.
    let w = world(&[(Vec3::new(250.0, -64.0, 0.0), Vec3::new(600.0, 64.0, 36.0))]);
    let mut ps = standing(&w);
    let z0 = ps.origin.z;
    let mut old = cmd(ps.command_time, 0, 0, 0);
    let mut top = z0;
    let mut jumped = false;
    for _ in 0..60 {
        let b = if !jumped && ps.origin.x > 212.0 {
            button::JUMP
        } else {
            0
        };
        jumped |= b != 0;
        let c = cmd(ps.command_time + 50, b, 127, 0);
        player_step(
            &mut ps,
            &c,
            &mut old,
            MoveWorld::bare(&w),
            Rules::default(),
            0.0,
        );
        top = top.max(ps.origin.z);
    }
    assert!(jumped);
    assert!(top <= z0 + 39.0, "{}", top - z0);
    assert!(ps.origin.z >= 36.0 && ps.origin.x > 250.0, "{ps:?}");
}

#[test]
fn the_step_down_refuses_a_plane_under_0_3() {
    // A grounded body on a face rising +x, sliding 1 unit off it down the
    // slope: the down pass lands on the face. At 75 degrees (normal z 0.26)
    // the plain slide stands; at 60 (0.5) the body is snapped onto it.
    let slide_off = |deg: f32| {
        let w = ramp_test_world(deg, 0.0, 100.0);
        w.set_arith(Arith::Stored);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0, 1000);
        let mw = MoveWorld::bare(&w);
        let (top, bottom) = (Vec3::new(50.0, 0.0, 1000.0), Vec3::new(50.0, 0.0, 0.0));
        let t = mw.box_trace(top, bottom, ps.mins, ps.maxs, MASK_PLAYERSOLID);
        assert!(t.fraction < 1.0 && t.normal.z < 0.7, "{deg}: {t:?}");
        ps.origin = fpu::lerp(top, bottom, t.fraction) + Vec3::Z * 0.01;
        ps.velocity = Vec3::new(-20.0, 0.0, 0.0);
        // As a walk over the face would leave it.
        ps.ground_entity_num = ENTITYNUM_WORLD as i32;
        let mut pml = Pml {
            frametime: 0.05,
            msec: 50,
            ground_plane: true,
            ..Default::default()
        };
        pml.ground_trace.normal = t.normal;
        let mut pm = Pmove::new(
            &mut ps,
            cmd(1050, 0, 0, 0),
            cmd(1000, 0, 0, 0),
            mw,
            Rules::default(),
        );
        let z = pm.ps.origin.z;
        slide::step_slide_move(&mut pm, &mut pml, false);
        z - pm.ps.origin.z
    };
    assert_eq!(slide_off(75.0), 0.0);
    let dropped = slide_off(60.0);
    assert!(dropped > 1.0, "{dropped}");
}

#[test]
fn the_pitch_holds_at_85_degrees() {
    let w = world(&[]);
    let mut ps = standing(&w);
    let mut old = cmd(ps.command_time, 0, 0, 0);
    let mut c = cmd(ps.command_time + 50, 0, 0, 0);
    c.angles[0] = fpu::shorts(89.0);
    player_step(
        &mut ps,
        &c,
        &mut old,
        MoveWorld::bare(&w),
        Rules::default(),
        0.0,
    );
    assert_eq!(ps.view_angles[0], 15473.0 * 360.0 / 65536.0);
    assert_eq!(ps.delta_angles[0], 15473 - c.angles[0]);
    let mut c = cmd(ps.command_time + 50, 0, 0, 0);
    c.angles[0] = -fpu::shorts(89.0);
    ps.delta_angles = [0; 3];
    player_step(
        &mut ps,
        &c,
        &mut old,
        MoveWorld::bare(&w),
        Rules::default(),
        0.0,
    );
    assert_eq!(ps.view_angles[0], -15473.0 * 360.0 / 65536.0);
}

/// The cmd's roll lands in the view as its pitch and yaw do, so the
/// client's view kick roll, sent in the cmd, is drawn off the predicted
/// view (cod2-weapons.md, "The recoil").
#[test]
fn the_cmd_roll_rides_into_the_view() {
    let w = world(&[]);
    let mut ps = standing(&w);
    let mut old = cmd(ps.command_time, 0, 0, 0);
    let mut c = cmd(ps.command_time + 50, 0, 0, 0);
    c.angles[2] = fpu::shorts(-0.75);
    player_step(
        &mut ps,
        &c,
        &mut old,
        MoveWorld::bare(&w),
        Rules::default(),
        0.0,
    );
    assert_eq!(
        ps.view_angles[2],
        f32::from(c.angles[2] as i16) * 360.0 / 65536.0
    );
    assert!(ps.view_angles[2] < -0.7);
}

#[test]
fn the_stances_take_their_box_and_eye() {
    let w = world(&[]);
    let mut ps = standing(&w);
    run(&mut ps, &w, 10, button::CROUCH, 0);
    assert_eq!(ps.view_height_current, 40.0);
    assert_eq!(ps.effective_stance(), Stance::Crouch);
    assert_eq!(ps.e_flags & (ef::CROUCH | ef::PRONE), ef::CROUCH);
    run(&mut ps, &w, 12, button::PRONE, 0);
    assert_eq!(ps.view_height_current, 11.0);
    assert_eq!(ps.pm_flags & (pmf::PRONE | pmf::DUCKED), pmf::PRONE);
    assert_eq!(ps.e_flags & (ef::CROUCH | ef::PRONE), ef::PRONE);
    // Going prone started the jump slowdown's 1800 ms.
    assert_ne!(ps.pm_flags & pmf::JUMPING, 0);
    run(&mut ps, &w, 20, 0, 0);
    assert_eq!(ps.view_height_current, 60.0);
    assert_eq!(ps.effective_stance(), Stance::Stand);
}

#[test]
fn prone_needs_66_units_behind_the_view() {
    // A wall 50 units behind the body: CoD1's 54-unit body would lie down
    // short of it on a block that passes its sill test; CoD2's 66 sweep is
    // blocked under the 64.8 sill and refused.
    let w = world(&[(Vec3::new(-80.0, -64.0, 0.0), Vec3::new(-50.0, 64.0, 128.0))]);
    let mut ps = standing(&w);
    run(&mut ps, &w, 12, button::PRONE, 0);
    assert_eq!(ps.pm_flags & pmf::PRONE, 0, "{ps:?}");
    assert_eq!(
        ps.events[((ps.event_sequence - 1) & 3) as usize],
        ev::STANCE_FORCE_STAND
    );
    // Facing away from it there is room.
    let w = world(&[(Vec3::new(50.0, -64.0, 0.0), Vec3::new(80.0, 64.0, 128.0))]);
    let mut ps = standing(&w);
    run(&mut ps, &w, 12, button::PRONE, 0);
    assert_ne!(ps.pm_flags & pmf::PRONE, 0);
}

#[test]
fn the_run_speeds_follow_the_stance() {
    let w = world(&[]);
    let speed = |buttons: i32| {
        let mut ps = standing(&w);
        run(&mut ps, &w, 12, buttons, 0);
        run(&mut ps, &w, 40, buttons, 127);
        ps.velocity.x
    };
    assert_eq!(speed(0), 190.0);
    // 123.5 and 28.5 as the stored scales leave them (190 / 127 per key
    // unit, times the floats 0.64999998 and 0.15000001, times 127), snapped
    // by `rint`.
    assert_eq!(speed(button::CROUCH), 123.0);
    assert_eq!(speed(button::PRONE), 28.0);
}

#[test]
fn a_spectator_flies_at_its_speed_times_the_spectate_scale() {
    let w = world(&[]);
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 100.0), 0.0, 1000);
    ps.pm_type = pm_type::SPECTATOR;
    run(&mut ps, &w, 40, 0, 127);
    assert!(
        (ps.velocity.length() - 380.0).abs() < 0.5,
        "{:?}",
        ps.velocity
    );
    assert_eq!(ps.view_height_current, 0.0);
    // The lean-right bit rises.
    let z = ps.origin.z;
    run(&mut ps, &w, 5, button::LEAN_RIGHT, 0);
    assert!(ps.origin.z > z);
}

#[test]
fn the_cmd_is_chopped_at_66_and_oldcmd_comes_back_as_sent() {
    let w = world(&[]);
    let mut ps = standing(&w);
    let t = ps.command_time;
    let c = cmd(t + 150, button::JUMP, 0, 0);
    let mut old = cmd(t, 0, 0, 0);
    player_step(
        &mut ps,
        &c,
        &mut old,
        MoveWorld::bare(&w),
        Rules::default(),
        0.0,
    );
    assert_eq!(ps.command_time, t + 150);
    // The caller's oldcmd is the cmd as sent: the jump the step cleared off
    // its own copy stays held for the next cmd's edge test.
    assert_eq!(old, c);
    // The jump fired on the first 66 ms step only.
    assert_eq!(ps.jump_time, t + 66);
}

#[test]
fn the_lean_ramps_to_half_standing() {
    let w = world(&[]);
    let mut ps = standing(&w);
    run(&mut ps, &w, 10, button::LEAN_RIGHT, 0);
    assert_eq!(ps.leanf, 0.5);
    run(&mut ps, &w, 10, 0, 0);
    assert_eq!(ps.leanf, 0.0);
}

#[test]
fn the_flat_cmd_scale_is_speed_over_127() {
    let ps = PlayerState::default();
    let s = moves::cmd_scale_flat(&ps, 127, 0, &Rules::default());
    assert_eq!(s, (127.0f64 * 190.0 / (127.0 * 127.0)) as f32);
    assert_eq!(moves::jump_curve(&ps, &Rules::default()), 1.0);
}

/// A gunner looking past the arcs is held on their edge: the view at
/// `base ± range`, the excess in `delta_angles`, so the next cmd reads the
/// edge off the short grid and is left alone (cod2-turrets.md 5).
#[test]
fn a_gunners_view_is_held_inside_the_turret_clamp() {
    let w = world(&[]);
    let mut ps = standing(&w);
    ps.e_flags |= ef::TURRET;
    ps.view_angle_clamp_base = [0.0, 233.1];
    ps.view_angle_clamp_range = [15.0, 45.0];
    let short = |deg: f32| (deg * 65536.0 / 360.0) as i32 & 0xffff;
    let mut old = cmd(ps.command_time, 0, 0, 0);
    let mut look = |ps: &mut PlayerState, pitch: f32, yaw: f32| {
        let mut c = cmd(ps.command_time + 50, 0, 0, 0);
        c.angles = [short(pitch), short(yaw), 0];
        player_step(ps, &c, &mut old, MoveWorld::bare(&w), Rules::default(), 0.0);
    };
    look(&mut ps, 40.0, 233.1 + 70.0);
    assert_eq!(ps.view_angles[0], 15.0);
    assert!(
        (ps.view_angles[1] - (233.1 + 45.0)).abs() < 1e-3,
        "{:?}",
        ps.view_angles
    );
    let held = ps.delta_angles;
    look(&mut ps, 40.0, 233.1 + 70.0);
    assert_eq!(
        ps.delta_angles, held,
        "the edge reads back inside the clamp"
    );
    assert!(
        (ps.view_angles[0] - 15.0).abs() < 0.01,
        "{:?}",
        ps.view_angles
    );
    // Inside the arcs nothing moves.
    let before = ps.delta_angles;
    look(&mut ps, 30.0, 233.1 + 50.0);
    assert_eq!(ps.delta_angles, before);
    assert!(ps.view_angles[0] < 15.0);
}

/// A prone body put on a ramp eases its pitches toward the ramp's at 70 a
/// second (`PM_UpdatePronePitch` 0x80e7c00): 3.5 a 50 ms cmd, then held.
#[test]
fn the_prone_pitches_ease_onto_the_ground_at_70_a_second() {
    let w = ramp_test_world(10.0, 0.0, 600.0);
    w.set_arith(Arith::Stored);
    let mut ps = PlayerState::spawn(Vec3::new(-300.0, 0.0, 0.125), 0.0, 1000);
    run(&mut ps, &w, 14, button::PRONE, 0);
    assert_ne!(ps.pm_flags & pmf::PRONE, 0, "{ps:?}");
    assert_eq!((ps.prone_direction_pitch, ps.prone_torso_pitch), (0.0, 0.0));
    let x = 300.0f32;
    ps.origin = Vec3::new(x, 0.0, x * 10f32.to_radians().tan() + 0.5);
    let n = Vec3::new(-(10f32.to_radians().sin()), 0.0, 10f32.to_radians().cos());
    // `PitchForYawOnNormal` answers in 0..360; uphill is nose up.
    let want = vcod_common::pmove::pitch_for_yaw_on_normal(0.0, n) - 360.0;
    assert!(want < -9.0 && want > -11.0, "{want}");
    run(&mut ps, &w, 1, button::PRONE, 0);
    assert!(ps.on_ground());
    assert!((ps.prone_direction_pitch + 3.5).abs() < 1e-5, "{ps:?}");
    assert_eq!(ps.prone_torso_pitch, ps.prone_direction_pitch);
    run(&mut ps, &w, 4, button::PRONE, 0);
    assert!(
        (ps.prone_direction_pitch - want).abs() < 0.02,
        "{} against {want}",
        ps.prone_direction_pitch
    );
    // Turned side-on, the torso eases back toward level; the body keeps
    // its heading's pitch.
    let body = ps.prone_direction_pitch;
    let mut p = ps.clone();
    p.view_angles[1] = 90.0;
    let pml = Pml {
        frametime: 0.05,
        ground_plane: true,
        ground_trace: Tr {
            normal: n,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pm = Pmove::new(
        &mut p,
        UserCmd2::default(),
        UserCmd2::default(),
        MoveWorld::bare(&w),
        Rules::default(),
    );
    pm.ps.ground_entity_num = 0;
    stance::update_prone_pitch(&mut pm, &pml);
    assert_eq!(p.prone_direction_pitch, body);
    assert!((p.prone_torso_pitch - (body + 3.5)).abs() < 1e-4, "{p:?}");
}

/// On ground steeper than a 0.7 normal a prone body raises
/// `EV_STANCE_FORCE_CROUCH` every frame (0x8155144).
#[test]
fn a_prone_body_on_a_steep_plane_is_forced_to_crouch() {
    let w = world(&[]);
    let mut ps = standing(&w);
    ps.pm_flags |= pmf::PRONE;
    let steep = Vec3::new(-(50f32.to_radians().sin()), 0.0, 50f32.to_radians().cos());
    for (n, forced) in [(steep, true), (Vec3::Z, false)] {
        let mut p = ps.clone();
        let seq = p.event_sequence;
        let pml = Pml {
            frametime: 0.05,
            ground_plane: true,
            ground_trace: Tr {
                normal: n,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pm = Pmove::new(
            &mut p,
            UserCmd2::default(),
            UserCmd2::default(),
            MoveWorld::bare(&w),
            Rules::default(),
        );
        stance::update_prone_pitch(&mut pm, &pml);
        assert_eq!(p.event_sequence - seq, i32::from(forced));
        if forced {
            assert_eq!(p.events[(seq & 3) as usize], ev::STANCE_FORCE_CROUCH);
            assert_eq!(p.pm_flags & pmf::REFUSED, 0);
        }
    }
}

/// A world clip whose foliage (contents 2) fills `lo..hi`; nothing else.
struct Bush {
    lo: Vec3,
    hi: Vec3,
}

impl vcod_common::collision::WorldClip for Bush {
    fn trace(
        &self,
        start: Vec3,
        _end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> vcod_common::collision::WorldHit {
        let inside = mask & crate::collision::CONTENTS_FOLIAGE != 0
            && (start + mins).cmplt(self.hi).all()
            && (start + maxs).cmpgt(self.lo).all();
        vcod_common::collision::WorldHit {
            fraction: if inside { 0.0 } else { 1.0 },
            normal: Vec3::ZERO,
            surface_flags: 0,
            contents: if inside { 2 } else { 0 },
            startsolid: inside,
            allsolid: inside,
        }
    }

    fn point_contents(&self, _p: Vec3) -> u32 {
        0
    }
}

/// `PM_FoliageSounds` (0x80e6784): a full run through foliage raises
/// `EV_FOLIAGE_SOUND` once past each 500 ms; standing still clears the
/// timer after 500; outside foliage nothing.
#[test]
fn a_run_through_foliage_rustles_every_half_second() {
    let mut w = world(&[]);
    w.set_world_clip(std::sync::Arc::new(Bush {
        lo: Vec3::new(-64.0, -512.0, 0.0),
        hi: Vec3::new(4096.0, 512.0, 64.0),
    }));
    let mut ps = standing(&w);
    let mut old = cmd(ps.command_time, 0, 0, 0);
    let mut rustles = Vec::new();
    for _ in 0..40 {
        let seq = ps.event_sequence;
        let c = cmd(ps.command_time + 50, 0, 127, 0);
        player_step(
            &mut ps,
            &c,
            &mut old,
            MoveWorld::bare(&w),
            Rules::default(),
            1.0,
        );
        if (seq..ps.event_sequence).any(|i| ps.events[(i & 3) as usize] == ev::FOLIAGE_SOUND) {
            assert_eq!(ps.foliage_sound_time, ps.command_time);
            rustles.push(ps.command_time);
        }
    }
    assert!(rustles.len() >= 3, "{rustles:?}");
    // At full run speed the interval is 500: the next cmd past it.
    for p in rustles.windows(2).skip(1) {
        assert_eq!(p[1] - p[0], 550, "{rustles:?}");
    }
    // Stopped: the timer survives 500 ms, then goes to 0.
    let last = ps.foliage_sound_time;
    for _ in 0..16 {
        run(&mut ps, &w, 1, 0, 0);
        let want = if ps.command_time > last + 500 {
            0
        } else {
            last
        };
        assert_eq!(ps.foliage_sound_time, want, "at {}", ps.command_time);
    }
    // Out of the bush nothing rustles.
    let bare = world(&[]);
    let mut ps = standing(&bare);
    let mut old = cmd(ps.command_time, 0, 0, 0);
    for _ in 0..20 {
        let c = cmd(ps.command_time + 50, 0, 127, 0);
        player_step(
            &mut ps,
            &c,
            &mut old,
            MoveWorld::bare(&bare),
            Rules::default(),
            1.0,
        );
    }
    assert_eq!(ps.foliage_sound_time, 0);
}

#[test]
fn a_prone_move_leaves_the_sight_unless_the_weapon_is_busy() {
    use weapon::{ev as wev, state};
    let w = world(&[]);
    let mut prone = standing(&w);
    run(&mut prone, &w, 14, button::PRONE, 0);
    assert_ne!(prone.pm_flags & pmf::PRONE, 0);
    // One cmd that starts the forward key, from `weaponstate` `s`.
    let step = |s: i32| {
        let mut ps = prone.clone();
        ps.weapon_state = s;
        ps.pm_flags |= pmf::SIGHT | pmf::PRONE_SIGHT;
        let seq = ps.event_sequence;
        let mut old = cmd(ps.command_time, button::PRONE, 0, 0);
        let c = cmd(ps.command_time + 50, button::PRONE, 127, 0);
        player_step(
            &mut ps,
            &c,
            &mut old,
            MoveWorld::bare(&w),
            Rules::default(),
            0.0,
        );
        let evs: Vec<i32> = (seq..ps.event_sequence)
            .map(|i| ps.events[(i & 3) as usize])
            .collect();
        (ps, evs)
    };
    // Ready: `PM_ExitAimDownSight`'s event, both sight flags off.
    let (ps, evs) = step(state::READY);
    assert_eq!(evs, [wev::RESET_ADS]);
    assert_eq!(ps.pm_flags & (pmf::SIGHT | pmf::PRONE_SIGHT), 0);
    assert!(!ps.weapon_interrupt);
    // Firing keeps the sight and raises nothing.
    let (ps, evs) = step(state::FIRING);
    assert!(evs.is_empty());
    assert_ne!(ps.pm_flags & pmf::PRONE_SIGHT, 0);
    // The binoculars are cut off first, the weapon's half owed.
    let (ps, evs) = step(state::BINOCULARS_HOLD);
    assert_eq!(evs, [wev::RESET_ADS, wev::BINOCULAR_EXIT, wev::RESET_ADS]);
    assert_eq!((ps.weapon_state, ps.weapon_interrupt), (state::READY, true));
}
