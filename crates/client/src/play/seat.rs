//! The view from a United Offensive vehicle seat: where `CG_VehicleView`
//! (uo cgame 0x30040810) puts the camera off the posed vehicle's tags, the
//! gunner's view held on its gun, and a four-wheel's riders turned with it
//! (0x30040580). docs/research/coduo-vehicles.md, 5.

use crate::fx::sim::TagPose;
use glam::{Mat3, Vec3};
use vcod_common::pmove::aim::angles_to_axis_msvc;

/// `centity` +0x88, the vehicle entity's `surfType`: its file's type.
pub const CLASS_FOUR_WHEEL: i32 = 1;
pub const CLASS_TANK: i32 = 2;
pub const CLASS_ARTILLERY: i32 = 5;

/// The seat the view sits in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seat {
    /// `ps.vehPos`: 1 the driver, 2 the gunner, 3.. passengers.
    pub pos: i32,
    /// `ps.vehType`.
    pub kind: i32,
    /// The vehicle entity's `surfType` (`CLASS_*`).
    pub class: i32,
}

impl Seat {
    /// The four-wheel's seat 3, which keeps its own weapon (cgame
    /// 0x30008210).
    pub fn own_weapon(&self) -> bool {
        self.kind == 1 && self.pos == 3
    }

    /// The field of view in a seat that draws no gun (`CG_AddViewWeapon`
    /// 0x30046570 draws one only for the jeep passenger), `None` for that
    /// one (0x3003ffc0): the driver's sight on a
    /// weapon with one closes toward 70 by the sight's fraction; every other
    /// seat but the jeep passenger's keeps `cg_fov` (the passenger zooms
    /// with its own weapon, as on foot).
    pub fn fov(&self, cg_fov: f32, frac: f32, weapon_has_sight: bool) -> Option<f32> {
        if self.own_weapon() {
            return None;
        }
        Some(if self.pos == 1 && weapon_has_sight {
            cg_fov - (cg_fov - 70.0) * frac
        } else {
            cg_fov
        })
    }
}

/// `0x300407c0`: the tag a seat's chase camera turns about.
fn pivot_tag(pos: i32) -> &'static str {
    match pos {
        2 => "tag_secondary_gun",
        3 => "tag_passenger",
        4 => "tag_passenger2",
        5 => "tag_passenger3",
        6 => "tag_passenger4",
        _ => "tag_turret",
    }
}

/// `BG_GetVehiclePosTag` (cgame 0x30008190).
fn seat_tag(pos: i32) -> &'static str {
    match pos {
        1 => "tag_player",
        2 => "tag_secondary_player",
        3 => "tag_passenger",
        4 => "tag_passenger2",
        5 => "tag_passenger3",
        6 => "tag_passenger4",
        _ => "tag_player",
    }
}

/// What the gunner's eye adds on top of its gun (0x30040810's seat-2 arm).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunnerEye {
    /// The turn sway's offset ([`GunnerSway::step`]): `[1]` along the gun's
    /// left, `[2]` down its up.
    pub sway: [f32; 3],
    /// The firing shake's phase ([`shake_phase`]), `None` unless shaking.
    pub shake: Option<f32>,
    /// `cg.time`, the shake's yaw clock.
    pub time_ms: i32,
}

/// The firing shake's phase at `cg.time`: a 100 ms triangle, 0 to 1 and
/// back (`cg.time % 100` folded at 50, times 0.02 at 0x3007c2b8).
pub fn shake_phase(time_ms: i32) -> f32 {
    let m = time_ms % 100;
    let m = if m > 50 { 100 - m } else { m };
    m as f32 * 0.02
}

/// Whether the gunner's view shakes: `cg.snap`'s playerstate fires
/// (`EF_FIRING`, UO's 0x200) and the vehicle's `animMovetype` (es +0xe4)
/// reads 0 (0x30040ae3..0x30040afa). The gunner's think writes 1 there at
/// an overheat and 0 once it cools, over the 2 and 6 the seats leave, so
/// a crewed vehicle shakes only after its gun has overheated once.
pub fn gunner_shakes(ps_eflags: i32, vehicle_anim_movetype: i32) -> bool {
    ps_eflags & vcod_common::net::flags::EF_FIRING != 0 && vehicle_anim_movetype == 0
}

/// `sin(2.334 * 2pi t) * cos(3.198 * 2pi t)` of `t` in seconds (0x3004d200,
/// the constants pushed at 0x30040d08): the shake's yaw wander, a quarter
/// of a degree at most.
fn shake_yaw(time_ms: i32) -> f32 {
    let t = f64::from(time_ms as f32) / 1000.0 * std::f64::consts::TAU;
    let a = f64::from(f32::from_bits(0x4015_6417));
    let b = f64::from(f32::from_bits(0x404c_a9d3));
    ((a * t).sin() * (b * t).cos()) as f32 * 0.25
}

/// The gunner's eye lag: the turn sway (0x30015ca0) run once a frame off a
/// copy of the predicted playerstate holding `thompson_MP`, at scale 2,
/// on state of its own (0x300dc9dc..0x300dca00) that no seat change resets.
#[derive(Default)]
pub struct GunnerSway {
    st: vcod_common::pmove::aim::AimState,
    last_ms: Option<i32>,
}

impl GunnerSway {
    /// Steps the sway to `time_ms` (`cg.frametime` since the last call) and
    /// returns its offset.
    pub fn step(
        &mut self,
        ps: &vcod_common::pmove::PlayerState,
        thompson: &vcod_common::weapon::WeaponDef,
        view: [f32; 3],
        time_ms: i32,
    ) -> [f32; 3] {
        let msec = self.last_ms.map_or(0, |t| time_ms - t);
        self.last_ms = Some(time_ms);
        vcod_common::pmove::aim::step_turn_sway(ps, thompson, &mut self.st, view, msec, 2.0)
    }
}

/// Where the seat puts the view, before the clip trace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeatCamera {
    pub pos: Vec3,
    /// Pitch and roll the refdef takes from the gun instead of the view,
    /// degrees, wire convention (the gunner).
    pub gun_pitch_roll: Option<[f32; 2]>,
    /// The view the client's own angles snap to (trap 0xf7), pitch and
    /// yaw degrees: the gunner's view put back on its gun.
    pub snap: Option<[f32; 2]>,
    /// Degrees the gunner's firing shake adds to the drawn yaw.
    pub yaw_shake: f32,
    /// The clip trace's start: `tag_body`, else the vehicle's origin, 20 up.
    pub trace_from: Vec3,
}

/// `CG_VehicleView` (0x30040810) without its trace. `view` is the
/// predicted view, wire degrees; `frac` the sight's fraction; `tag` poses a
/// tag of the vehicle as drawn this frame; `origin` its drawn origin;
/// `gunner` the gunner's sway and shake, read in seat 2 only.
pub fn camera(
    seat: Seat,
    view: [f32; 3],
    frac: f32,
    origin: Vec3,
    gunner: GunnerEye,
    tag: &mut dyn FnMut(&str) -> Option<TagPose>,
) -> Option<SeatCamera> {
    let trace_from = tag("tag_body").map_or(origin, |t| t.origin) + Vec3::Z * 20.0;
    let out = |pos| {
        Some(SeatCamera {
            pos,
            gun_pitch_roll: None,
            snap: None,
            yaw_shake: 0.0,
            trace_from,
        })
    };
    if seat.class == CLASS_ARTILLERY && frac == 0.0 {
        return out(tag("tag_chasecam")?.origin);
    }
    if seat.pos == 2 {
        let g = tag("tag_secondary_gun")?;
        let [f, l, u] = g.axis;
        let mut eye =
            tag("tag_secondary_view").map_or(f * -20.0 + u * 6.5, |t| t.origin - g.origin);
        // The shake pulls the eye back along the gun, half a unit at most.
        if let Some(t) = gunner.shake {
            eye += f * (t * -0.5);
        }
        let [sf, sl, su] = gunner.sway;
        let pos = g.origin + eye + f * sf + l * sl - u * su;
        let [pitch, yaw, roll] = axis_to_angles(g.axis);
        // 0x30040c3d: off by more than 0.4 degrees of pitch, the client's
        // angles are put on the gun's.
        let snap = (vcod_common::pmove::angle_normalize180(view[0] - pitch).abs() > 0.4)
            .then_some([pitch, yaw]);
        // After the snap the shake lifts the pitch and wanders the yaw
        // (0x30040ce8..0x30040d26).
        let (lift, yaw_shake) = gunner
            .shake
            .map_or((0.0, 0.0), |t| (t * 0.5, shake_yaw(gunner.time_ms)));
        return Some(SeatCamera {
            pos,
            gun_pitch_roll: Some([pitch - lift, roll]),
            snap,
            yaw_shake,
            trace_from,
        });
    }
    let four_wheel = seat.class == CLASS_FOUR_WHEEL;
    if seat.own_weapon() || (frac == 0.0) == four_wheel {
        if four_wheel && let Some(t) = tag(seat_tag(seat.pos)) {
            let [f, _, u] = t.axis;
            return out(t.origin - f * 12.0 + u * 32.0);
        }
        if let Some(t) = tag("tag_aimdownbarrel") {
            return out(t.origin);
        }
        let t = tag("tag_barrel")?;
        let [f, l, u] = t.axis;
        return out(t.origin + f * 30.0 - l * 3.0 + u * 10.0);
    }
    // The chase camera: the chasecam's offset from the pivot, taken into
    // the pivot's frame and turned by the view.
    let t = tag(pivot_tag(seat.pos))?;
    let [tf, tl, tu] = t.axis;
    let offset = match tag("tag_chasecam") {
        Some(c) => {
            let o = c.origin - t.origin;
            if four_wheel { o * 2.5 } else { o }
        }
        None => tf * -200.0 + tu * 32.0,
    };
    let local = Vec3::new(offset.dot(tf), offset.dot(tl), offset.dot(tu));
    let [rf, rl, ru] = angles_to_axis_msvc([view[0], view[1], 0.0]).map(Vec3::from);
    let mut rotated = rf * local.x + rl * local.y + ru * local.z;
    if seat.class == CLASS_TANK {
        rotated.z *= 0.75;
    }
    if four_wheel && frac != 0.0 {
        rotated -= ru * 135.0;
    }
    let len = offset.length();
    let dist = rotated.length();
    if dist > 2.0 * len {
        rotated *= 2.0 * len / dist;
    } else if dist < 0.5 * len && dist > 0.0 {
        rotated *= 0.5 * len / dist;
    }
    out(t.origin + rotated)
}

/// The end of `CG_VehicleView`: a ±8 box from `trace_from` to the camera
/// under mask 0x211, retraced under 0x11 when it started in a 0x200 brush;
/// a hit stops the camera there, `(1 - fraction) * 32` higher, and a second
/// trace from the start brings it back inside.
pub fn clip(cam: &SeatCamera, trace: &mut dyn FnMut(Vec3, Vec3, u32) -> (f32, Vec3, bool)) -> Vec3 {
    let (mut fraction, mut end, startsolid) = trace(cam.trace_from, cam.pos, 0x211);
    if startsolid {
        (fraction, end, _) = trace(cam.trace_from, cam.pos, 0x11);
    }
    if fraction == 1.0 {
        return cam.pos;
    }
    let raised = end + Vec3::Z * ((1.0 - fraction) * 32.0);
    (_, end, _) = trace(cam.trace_from, raised, 0x11);
    end
}

/// The seat the newest snapshot's playerstate sits in, with its vehicle's
/// entity number and angles; `None` on foot or off a UO server.
pub fn of_snapshot(
    p: &vcod_common::net::protocol::Protocol,
    s: &vcod_common::net::snapshot::Snapshot,
) -> Option<(Seat, u32, [f32; 3])> {
    use vcod_common::net::flags::{EF_IN_VEHICLE_V22, ET_VEHICLE_V22};
    let int = |n: &str| s.ps.field_i32(p, n);
    if !p.is_uo() || int("eFlags") & EF_IN_VEHICLE_V22 == 0 {
        return None;
    }
    let num = u32::try_from(int("viewlocked_entNum")).ok()?;
    let ent = s
        .entities
        .get(&num)
        .filter(|e| e.field_i32(p, "eType") == ET_VEHICLE_V22)?;
    let seat = Seat {
        pos: int("vehPos"),
        kind: int("vehType"),
        class: ent.field_i32(p, "surfType"),
    };
    Some((seat, num, ent.angles(p)))
}

/// Whether the body the view rides draws in its seat (cgame 0x3003392b,
/// first person): never in a tank's seat 1 (nobody's is) or its gunner's
/// seat, never in a four-wheel's seats 2 and 3, and in a four-wheel's
/// other seats only with the sight up, which is when the view leaves the
/// seat for the chase camera. `frac` is the predicted `fWeaponPosFrac`.
pub fn own_body_drawn(seat: Seat, frac: f32) -> bool {
    match seat.class {
        CLASS_FOUR_WHEEL => !matches!(seat.pos, 2 | 3) && frac != 0.0,
        CLASS_TANK => !matches!(seat.pos, 1 | 2),
        _ => true,
    }
}

/// Whether the frame adds [`own_body`] to the snapshot pair. The cgame adds
/// `cg.predictedPlayerEntity` under `pm_flags` 0x50000, the own view or a
/// followed client's copied frame (1.1 0x3001bd00), posed and not drawn
/// in first person (0x30028210), so `playFxOnTag` finds its tags
/// (docs/research/cod11-events-and-fx.md, `playFxOnTag`). A UO seat that
/// draws its body adds it too; a followed one in a seat does not.
pub fn adds_view_body(pm_flags: i32, seat_body: bool, following: bool) -> bool {
    use vcod_common::net::flags::{PMF_FOLLOW, PMF_OWN_VIEW};
    if seat_body {
        !following
    } else {
        pm_flags & (PMF_OWN_VIEW | PMF_FOLLOW) != 0
    }
}

/// The entity `BG_PlayerStateToEntityState` (game 0x28004, the cgame's
/// `cg.predictedPlayerEntity`) makes of a playerstate, enough for
/// `entities::build_instances` to pose it: the body, the anims, the
/// weapon, the lean and the body's pitches, and the seat packed into
/// `surfType` with the vehicle in `otherEntityNum`.
pub fn own_body(
    p: &vcod_common::net::protocol::Protocol,
    ps: &vcod_common::net::msg::PlayerState,
) -> vcod_common::net::msg::EntityState {
    use vcod_common::net::flags::{EF_DEAD, ET_PLAYER};
    use vcod_common::net::msg::EntityState;
    use vcod_common::net::trajectory::TR_INTERPOLATE;
    let int = |n: &str| ps.field_i32(p, n);
    let mut e = EntityState::null(p);
    e.number = int("clientNum") as u32;
    let mut set = |name: &str, v: i32| {
        if let Some(i) = EntityState::field_index(p, name) {
            e.fields[i] = v;
        }
    };
    set("eType", ET_PLAYER);
    set("clientNum", int("clientNum"));
    let dead = if int("pm_type") >= 6 { EF_DEAD } else { 0 };
    set("eFlags", int("eFlags") & !EF_DEAD | dead);
    set("pos.trType", TR_INTERPOLATE);
    set("apos.trType", TR_INTERPOLATE);
    for (axis, (o, a)) in ps.origin(p).iter().zip(ps.viewangles(p)).enumerate() {
        set(&format!("pos.trBase[{axis}]"), o.to_bits() as i32);
        set(&format!("apos.trBase[{axis}]"), a.to_bits() as i32);
    }
    for name in [
        "legsAnim",
        "torsoAnim",
        "weapon",
        "groundEntityNum",
        "leanf",
        "fTorsoHeight",
        "fTorsoPitch",
        "fWaistPitch",
    ] {
        set(name, int(name));
    }
    // The wire's byte, signed, as a float (spectate.rs's `to_entity`).
    set(
        "angles2[1]",
        f32::from(int("movementDir") as u8 as i8).to_bits() as i32,
    );
    set("otherEntityNum", int("viewlocked_entNum"));
    set(
        "surfType",
        int("vehPos") & 7 | (int("vehType") & 7) << 3 | (int("vehMotion") & 3) << 6,
    );
    e
}

/// `AxisToAngles`: wire pitch, yaw and roll of a `[forward, left, up]`.
pub fn axis_to_angles(axis: [Vec3; 3]) -> [f32; 3] {
    let [f, l, _] = axis;
    let yaw = f.y.atan2(f.x).to_degrees();
    let pitch = -f.z.atan2((f.x * f.x + f.y * f.y).sqrt()).to_degrees();
    let [_, l0, u0] = angles_to_axis_msvc([pitch, yaw, 0.0]).map(Vec3::from);
    let roll = l.dot(u0).atan2(l.dot(l0)).to_degrees();
    [pitch, yaw, roll]
}

/// 0x30040580: a four-wheel's riders but the gunner and the passenger with
/// its own weapon have their view turned with the vehicle, frame to frame:
/// the pitch while the sight is down, the yaw while `eFlags` 0x200000
/// (the mount's look-along-the-hull flag) is set.
#[derive(Default)]
pub struct FourWheelTurn {
    /// The vehicle's axis last frame.
    prev: Option<Mat3>,
}

impl FourWheelTurn {
    /// Forgets the vehicle: out of a seat, or in one this does not turn.
    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// The view turned by the vehicle's turn since the last call, pitch and
    /// yaw degrees; `None` when nothing is written.
    pub fn step(
        &mut self,
        view: [f32; 3],
        vehicle_angles: [f32; 3],
        sight_down: bool,
        hull_yaw: bool,
    ) -> Option<[f32; 2]> {
        let cur = mat(angles_to_axis_msvc(vehicle_angles).map(Vec3::from));
        let prev = self.prev.replace(cur)?;
        // Rows are the axes: V' = V * prev^T * cur.
        let v = mat(angles_to_axis_msvc(view).map(Vec3::from));
        let turned = v * prev.transpose() * cur;
        let rows = [turned.row(0), turned.row(1), turned.row(2)];
        let [pitch, yaw, _] = axis_to_angles(rows);
        let pitch = if sight_down { view[0] } else { pitch };
        let yaw = if hull_yaw { yaw } else { view[1] };
        Some([pitch, yaw])
    }
}

/// A matrix whose rows are `axis`.
fn mat(axis: [Vec3; 3]) -> Mat3 {
    Mat3::from_cols(axis[0], axis[1], axis[2]).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_body_is_added_in_its_own_view_or_a_copied_follow_frame() {
        use vcod_common::net::flags::{PMF_FOLLOW, PMF_FORCED_FOLLOW, PMF_OWN_VIEW};
        assert!(adds_view_body(PMF_OWN_VIEW, false, false));
        assert!(adds_view_body(PMF_FOLLOW | PMF_FORCED_FOLLOW, false, true));
        // A free spectator's frame carries neither bit.
        assert!(!adds_view_body(0, false, false));
        assert!(!adds_view_body(PMF_FORCED_FOLLOW, false, true));
        // A UO seat that draws its body: ours, never a followed one.
        assert!(adds_view_body(0, true, false));
        assert!(!adds_view_body(PMF_FOLLOW, true, true));
    }

    #[test]
    fn the_view_body_carries_the_lean_and_the_body_pitches() {
        use vcod_common::net::msg::PlayerState;
        use vcod_common::net::protocol::PROTOCOL_V1;
        let p = &PROTOCOL_V1;
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            let i = PlayerState::field_index(p, name).expect(name);
            ps.fields[i] = v;
        };
        set("leanf", (-0.5f32).to_bits() as i32);
        set("fTorsoPitch", 12.5f32.to_bits() as i32);
        set("fWaistPitch", 4.0f32.to_bits() as i32);
        set("fTorsoHeight", 2.0f32.to_bits() as i32);
        // A leftward movement direction travels as its unsigned byte.
        set("movementDir", 0xd3);
        let e = own_body(p, &ps);
        assert_eq!(e.field_f32(p, "leanf"), -0.5);
        assert_eq!(e.field_f32(p, "fTorsoPitch"), 12.5);
        assert_eq!(e.field_f32(p, "fWaistPitch"), 4.0);
        assert_eq!(e.field_f32(p, "fTorsoHeight"), 2.0);
        assert_eq!(e.field_f32(p, "angles2[1]"), -45.0);
    }

    fn pose(origin: Vec3, angles: [f32; 3]) -> TagPose {
        TagPose {
            origin,
            axis: angles_to_axis_msvc(angles).map(Vec3::from),
        }
    }

    #[test]
    fn axis_to_angles_inverts_angles_to_axis() {
        for a in [[10.0, 30.0, 0.0], [-20.0, -150.0, 15.0], [5.0, 90.0, -40.0]] {
            let b = axis_to_angles(angles_to_axis_msvc(a).map(Vec3::from));
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 1e-3, "{a:?} {b:?}");
            }
        }
    }

    /// A tank driver at the hip looks from the chase camera, orbiting
    /// `tag_turret` by the view: turned half round, the camera swings to
    /// the turret's other side.
    #[test]
    fn a_tank_driver_rides_the_chase_camera() {
        let seat = Seat {
            pos: 1,
            kind: 2,
            class: CLASS_TANK,
        };
        let turret = Vec3::new(0.0, 0.0, 100.0);
        let mut tag = |name: &str| match name {
            "tag_turret" => Some(pose(turret, [0.0; 3])),
            "tag_chasecam" => Some(pose(turret + Vec3::new(-200.0, 0.0, 40.0), [0.0; 3])),
            _ => None,
        };
        let ahead = camera(
            seat,
            [0.0, 0.0, 0.0],
            0.0,
            Vec3::ZERO,
            GunnerEye::default(),
            &mut tag,
        )
        .unwrap();
        assert!((ahead.pos - (turret + Vec3::new(-200.0, 0.0, 30.0))).length() < 1e-3);
        let back = camera(
            seat,
            [0.0, 180.0, 0.0],
            0.0,
            Vec3::ZERO,
            GunnerEye::default(),
            &mut tag,
        )
        .unwrap();
        assert!((back.pos - (turret + Vec3::new(200.0, 0.0, 30.0))).length() < 1e-2);
        // Sighted, the driver looks down the barrel.
        let mut tag = |name: &str| match name {
            "tag_aimdownbarrel" => Some(pose(Vec3::new(5.0, 6.0, 7.0), [0.0; 3])),
            _ => None,
        };
        let sighted = camera(
            seat,
            [0.0; 3],
            1.0,
            Vec3::ZERO,
            GunnerEye::default(),
            &mut tag,
        )
        .unwrap();
        assert_eq!(sighted.pos, Vec3::new(5.0, 6.0, 7.0));
    }

    /// The gunner's eye is `tag_secondary_view`, its pitch the gun's, and a
    /// view off the gun's pitch snaps to the gun.
    #[test]
    fn the_gunner_looks_along_its_gun() {
        let seat = Seat {
            pos: 2,
            kind: 2,
            class: CLASS_TANK,
        };
        let mut tag = |name: &str| match name {
            "tag_secondary_gun" => Some(pose(Vec3::ZERO, [-10.0, 45.0, 0.0])),
            "tag_secondary_view" => Some(pose(Vec3::new(1.0, 2.0, 3.0), [0.0; 3])),
            _ => None,
        };
        let on_gun = camera(
            seat,
            [-10.0, 45.0, 0.0],
            0.0,
            Vec3::ZERO,
            GunnerEye::default(),
            &mut tag,
        )
        .unwrap();
        assert_eq!(on_gun.pos, Vec3::new(1.0, 2.0, 3.0));
        assert!(on_gun.snap.is_none());
        let off = camera(
            seat,
            [20.0, 90.0, 0.0],
            0.0,
            Vec3::ZERO,
            GunnerEye::default(),
            &mut tag,
        )
        .unwrap();
        let [p, y] = off.snap.unwrap();
        assert!((p + 10.0).abs() < 1e-3 && (y - 45.0).abs() < 1e-3);
    }

    /// Firing, the gunner's eye backs along the gun by up to half a unit on
    /// a 100 ms triangle and its pitch lifts by up to half a degree; the
    /// sway moves it along the gun's left and down its up.
    #[test]
    fn the_gunner_shakes_and_sways_on_its_gun() {
        assert_eq!(
            [0, 25, 50, 75, 100, 150].map(shake_phase),
            [0.0, 0.5, 1.0, 0.5, 0.0, 1.0]
        );
        let seat = Seat {
            pos: 2,
            kind: 2,
            class: CLASS_TANK,
        };
        let mut tag = |name: &str| match name {
            "tag_secondary_gun" => Some(pose(Vec3::ZERO, [0.0; 3])),
            "tag_secondary_view" => Some(pose(Vec3::new(-10.0, 0.0, 5.0), [0.0; 3])),
            _ => None,
        };
        let eye = GunnerEye {
            sway: [0.0, 1.0, 2.0],
            shake: Some(1.0),
            time_ms: 50,
        };
        let cam = camera(seat, [0.0; 3], 0.0, Vec3::ZERO, eye, &mut tag).unwrap();
        assert!(
            (cam.pos - Vec3::new(-10.5, 1.0, 3.0)).length() < 1e-4,
            "{}",
            cam.pos
        );
        assert_eq!(cam.gun_pitch_roll.unwrap()[0], -0.5);
        assert!(cam.yaw_shake.abs() <= 0.25);
        assert!(gunner_shakes(vcod_common::net::flags::EF_FIRING, 0));
        assert!(!gunner_shakes(vcod_common::net::flags::EF_FIRING, 6));
        assert!(!gunner_shakes(0, 0));
    }

    /// Retail draws a jeep driver's own body only with the sight up (the
    /// chase camera), never a tank driver's or gunner's.
    #[test]
    fn the_own_body_draws_where_the_view_leaves_the_seat() {
        let seat = |pos, class| Seat {
            pos,
            kind: class,
            class,
        };
        assert!(!own_body_drawn(seat(1, CLASS_FOUR_WHEEL), 0.0));
        assert!(own_body_drawn(seat(1, CLASS_FOUR_WHEEL), 1.0));
        assert!(!own_body_drawn(seat(3, CLASS_FOUR_WHEEL), 1.0));
        assert!(own_body_drawn(seat(4, CLASS_FOUR_WHEEL), 0.5));
        assert!(!own_body_drawn(seat(1, CLASS_TANK), 1.0));
        assert!(!own_body_drawn(seat(2, CLASS_TANK), 0.0));
        assert!(own_body_drawn(seat(1, CLASS_ARTILLERY), 0.0));
    }

    /// The jeep capture's driver, rebuilt as the entity other clients see:
    /// seat 1 of a four-wheel in `surfType`, the jeep in `otherEntityNum`.
    #[test]
    fn the_own_body_carries_the_seat() {
        use vcod_common::net::protocol::PROTOCOL_V22;
        let p = &PROTOCOL_V22;
        let frames = vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-uo-jeep.bin",
            "net/snapshots-uo-jeep.bin",
        );
        let gunner_of = |s: &vcod_common::net::snapshot::Snapshot| {
            s.entities
                .values()
                .find(|e| e.field_i32(p, "eType") == 1 && e.field_i32(p, "surfType") & 7 == 2)
                .cloned()
        };
        let s = &frames
            .iter()
            .rev()
            .map(|f| &f.snapshot)
            .find(|s| gunner_of(s).is_some() && of_snapshot(p, s).is_some())
            .expect("a crewed frame");
        let (seat, num, _) = of_snapshot(p, s).unwrap();
        let e = own_body(p, &s.ps);
        assert_eq!(e.number, s.ps.field_i32(p, "clientNum") as u32);
        assert_eq!(e.field_i32(p, "otherEntityNum"), num as i32);
        assert_eq!(e.field_i32(p, "surfType") & 7, seat.pos);
        assert_eq!(e.field_i32(p, "surfType") >> 3 & 7, seat.kind);
        assert_eq!(e.origin(p), s.ps.origin(p));
        // The gunner's entity in the same capture packs the same way.
        let gunner = gunner_of(s).unwrap();
        assert_eq!(gunner.field_i32(p, "otherEntityNum"), num as i32);
    }

    #[test]
    fn a_clipped_camera_rises_by_what_was_cut() {
        let cam = SeatCamera {
            pos: Vec3::new(100.0, 0.0, 0.0),
            gun_pitch_roll: None,
            snap: None,
            yaw_shake: 0.0,
            trace_from: Vec3::ZERO,
        };
        let mut wall = |a: Vec3, b: Vec3, _mask: u32| {
            let f = if b.x > 50.0 { 50.0 / (b.x - a.x) } else { 1.0 };
            (f, a + (b - a) * f, false)
        };
        let at = clip(&cam, &mut wall);
        assert_eq!(at.x, 50.0);
        assert!(at.z > 0.0);
    }

    /// A jeep turning 10 degrees left turns the driver's view with it.
    #[test]
    fn a_four_wheel_turns_its_riders() {
        let mut t = FourWheelTurn::default();
        assert!(
            t.step([0.0, 30.0, 0.0], [0.0, 0.0, 0.0], false, true)
                .is_none()
        );
        let [p, y] = t
            .step([0.0, 30.0, 0.0], [0.0, 10.0, 0.0], false, true)
            .unwrap();
        assert!(p.abs() < 1e-3 && (y - 40.0).abs() < 1e-3, "{p} {y}");
        // Without the hull flag the yaw stays.
        let [_, y] = t
            .step([0.0, 30.0, 0.0], [0.0, 20.0, 0.0], false, false)
            .unwrap();
        assert_eq!(y, 30.0);
    }
}
