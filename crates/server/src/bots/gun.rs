//! What CoD2's and CoD4's bots on a mounted gun share ([`super::cod2`]'s and
//! [`super::cod4`]'s `gun`): the gun's arc, the spot behind it and the
//! straight walk there, the barrel's turn and the enemy in the arc. Which
//! gun a bot takes and when it lets go stay each game's own.

use glam::Vec3;
use vcod_common::collision::CollisionWorld;

use crate::game::cod2_turret::GunInfo;

/// The most a bot's feet and the spot's may differ in height for the
/// straight walk.
pub(crate) const GUN_STEP: f32 = 40.0;
/// An enemy nearer the gun than this is fought by hand.
pub(crate) const MIN_RANGE: f32 = 250.0;
/// The barrel's turn per tick, degrees.
pub(crate) const TURN_PER_TICK: f32 = 12.0;
/// The walk's box: the player's, the step height up.
const WALK_MINS: Vec3 = Vec3::new(-15.0, -15.0, 18.0);
const WALK_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// `a` folded into (-180, 180].
pub(crate) fn angle_180(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// `at`'s yaw off the gun's own, folded into (-180, 180].
fn arc_yaw(gun: &GunInfo, at: Vec3) -> f32 {
    let d = at - gun.origin;
    angle_180(d.y.atan2(d.x).to_degrees() - gun.yaw)
}

/// Whether `at` is inside the gun's yaw arc.
pub(crate) fn in_arc(gun: &GunInfo, at: Vec3) -> bool {
    let y = arc_yaw(gun, at);
    y >= gun.yaw_range[0] && y <= gun.yaw_range[1]
}

/// The spot `back` behind the gun along its yaw a bot mounts from, at the
/// gun's height.
pub(crate) fn mount_spot(gun: &GunInfo, back: f32) -> Vec3 {
    let (s, c) = gun.yaw.to_radians().sin_cos();
    gun.origin - Vec3::new(c, s, 0.0) * back
}

/// The view from `eye` onto `at`: pitch (down positive), yaw.
pub(crate) fn look_at(eye: Vec3, at: Vec3) -> [f32; 2] {
    let d = at - eye;
    let flat = d.truncate().length();
    [-d.z.atan2(flat).to_degrees(), d.y.atan2(d.x).to_degrees()]
}

/// `from` turned toward `to` by at most `step` degrees an axis.
pub(crate) fn turn(from: [f32; 2], to: [f32; 2], step: f32) -> [f32; 2] {
    std::array::from_fn(|i| from[i] + angle_180(to[i] - from[i]).clamp(-step, step))
}

/// The gunner's view this tick, turned from `cur` toward `target` seen from
/// `eye`, and whether it is within `cone` degrees of it to fire; held with
/// no target.
pub(crate) fn aim_barrel(
    cur: [f32; 2],
    eye: Vec3,
    target: Option<Vec3>,
    cone: f32,
) -> ([f32; 2], bool) {
    let Some(at) = target else {
        return (cur, false);
    };
    let want = look_at(eye, at);
    let aim = turn(cur, want, TURN_PER_TICK);
    let off = (0..2).map(|i| angle_180(want[i] - aim[i]).abs());
    (aim, off.fold(0.0, f32::max) <= cone)
}

/// The floor under `spot`, traced 32 up to 96 down against `mask`; `None`
/// with none there.
pub(crate) fn floor_spot(w: &CollisionWorld, spot: Vec3, mask: u32) -> Option<Vec3> {
    let floor = w.move_trace(
        spot + Vec3::Z * 32.0,
        spot - Vec3::Z * 96.0,
        Vec3::ZERO,
        Vec3::ZERO,
        mask,
    );
    (floor.fraction < 1.0).then_some(floor.endpos)
}

/// Whether feet at `feet` walk straight to `spot`: within `reach` across,
/// [`GUN_STEP`] up or down, and unblocked against `mask`.
pub(crate) fn straight_walk(
    w: &CollisionWorld,
    feet: Vec3,
    spot: Vec3,
    reach: f32,
    mask: u32,
) -> bool {
    if (spot - feet).truncate().length() > reach || (spot.z - feet.z).abs() > GUN_STEP {
        return false;
    }
    w.move_trace(feet, spot, WALK_MINS, WALK_MAXS, mask)
        .fraction
        >= 1.0
}

/// The chest of the nearest of `foes` (slot, chest) in `gun`'s arc within
/// [`super::SHOOT_RANGE`] of `eye` that `clear` says the eye line reaches,
/// by slot on a tie.
pub(crate) fn arc_enemy(
    gun: &GunInfo,
    eye: Vec3,
    foes: impl IntoIterator<Item = (usize, Vec3)>,
    clear: impl Fn(Vec3) -> bool,
) -> Option<Vec3> {
    let mut seen: Vec<(f32, usize, Vec3)> = foes
        .into_iter()
        .map(|(i, chest)| (chest.distance(eye), i, chest))
        .filter(|&(d, _, chest)| d <= super::SHOOT_RANGE && in_arc(gun, chest))
        .collect();
    seen.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    seen.into_iter()
        .find(|&(_, _, chest)| clear(chest))
        .map(|(_, _, chest)| chest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_gsc::EntId;

    fn gun() -> GunInfo {
        GunInfo {
            id: EntId(295, 0),
            origin: Vec3::new(100.0, 0.0, 0.0),
            yaw: 90.0,
            yaw_range: [-45.0, 45.0],
            pitch_range: [-15.0, 15.0],
            owner: None,
        }
    }

    #[test]
    fn the_arc_is_the_yaw_range_off_the_gun() {
        let g = gun();
        assert!(in_arc(&g, Vec3::new(100.0, 500.0, 0.0)));
        assert!(in_arc(&g, Vec3::new(400.0, 400.0, 0.0)));
        assert!(!in_arc(&g, Vec3::new(600.0, 400.0, 0.0)));
        assert!(!in_arc(&g, Vec3::new(100.0, -500.0, 0.0)));
        let spot = mount_spot(&g, 40.0);
        assert!((spot - Vec3::new(100.0, -40.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn the_barrel_turns_a_step_at_a_time_the_short_way() {
        assert_eq!(turn([0.0, 170.0], [5.0, -170.0], 12.0), [5.0, 182.0]);
        assert_eq!(turn([0.0, 0.0], [30.0, 0.0], 12.0), [12.0, 0.0]);
        let v = look_at(Vec3::ZERO, Vec3::new(100.0, 100.0, -100.0 * 2f32.sqrt()));
        assert!((v[0] - 45.0).abs() < 1e-3 && (v[1] - 45.0).abs() < 1e-3);
    }

    /// The nearest in the arc the eye line reaches.
    #[test]
    fn the_gunner_takes_the_nearest_clear_foe_in_the_arc() {
        let g = gun();
        let eye = Vec3::new(100.0, -40.0, 60.0);
        let at = |y: f32| Vec3::new(100.0, y, 60.0);
        let foes = [
            (4, at(600.0)),
            (2, at(600.0)),
            (1, at(300.0)),
            (3, at(-500.0)),
        ];
        let clear = |c: Vec3| c.y != 300.0;
        assert_eq!(arc_enemy(&g, eye, foes, clear), Some(at(600.0)));
        let wide = [(5, Vec3::new(1000.0, 0.0, 60.0))];
        assert_eq!(arc_enemy(&g, eye, wide, |_| true), None, "out of the arc");
    }
}
