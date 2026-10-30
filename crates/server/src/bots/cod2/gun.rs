//! CoD2 bots on the mounted guns (cod2-turrets.md 17). Retail has no
//! bots and vcod's CoD1 brain mans no gun, so the rule is vcod's own: in
//! `dm` and `tdm` a bot that sees an enemy in a free gun's arc, past
//! [`MIN_RANGE`] from it, with a clear walk to the spot behind it, walks
//! there and taps use once the cursor hint names the gun. On the gun it
//! turns the barrel onto the nearest enemy in the arc it can see and holds
//! the trigger once on target; [`IDLE_RELEASE_MS`] without one, it taps
//! use again and lets go.

use glam::Vec3;
use vcod_cod2::net::UserCmd2;
use vcod_cod2::pmove::{button, ef};
use vcod_gsc::EntId;

use super::{Cod2Bots, eye, playing};
use crate::bots::BotView;
use crate::game::cod2::Cod2Level;
use crate::game::cod2_turret::GunInfo;
use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};

/// How far from the spot behind a gun a bot sets off for it.
const GUN_REACH: f32 = 320.0;
/// The most a bot's feet and the spot's may differ in height.
const GUN_STEP: f32 = 40.0;
/// An enemy nearer the gun than this is fought by hand.
const MIN_RANGE: f32 = 250.0;
/// The walk to a gun gives up after this, ms.
const APPROACH_MS: i32 = 5000;
/// A gunner with no enemy in the arc this long lets go, ms.
const IDLE_RELEASE_MS: i32 = 4000;
/// The barrel's turn per tick, degrees, and how near the aim it fires.
const TURN_PER_TICK: f32 = 12.0;
const FIRE_CONE: f32 = 3.0;
/// The spot a bot mounts from, behind the gun along its yaw: where the
/// stock probes stand (cod2-turrets.md 10).
const BACK: f32 = 40.0;
/// `+activate` in a CoD2 cmd (`to_cmd2`'s map of `BUTTON_USE`).
pub(super) const USE: i32 = 0x8;
/// The walk's box: the player's, the step height up.
const WALK_MINS: Vec3 = Vec3::new(-15.0, -15.0, 18.0);
const WALK_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// One bot's business with a gun.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GunTask {
    gun: Option<EntId>,
    /// When the walk began, and when the gunner last had a target.
    since: i32,
    seen: i32,
    /// The last cmd held use: a press is its rising edge.
    pressed: bool,
    /// On the gun, not walking to it.
    manning: bool,
}

/// What the gun step makes of the brain's cmd.
pub(super) enum GunCmd {
    /// Walking to the gun: use on this cmd or not.
    Approach { press: bool },
    /// On the gun: the view, the trigger, a use press to let go.
    Man {
        view: [f32; 2],
        fire: bool,
        release: bool,
    },
}

/// `at`'s yaw off the gun's own, folded into (-180, 180].
fn arc_yaw(gun: &GunInfo, at: Vec3) -> f32 {
    let d = at - gun.origin;
    angle_180(d.y.atan2(d.x).to_degrees() - gun.yaw)
}

fn angle_180(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// Whether `at` is inside the gun's yaw arc.
pub(super) fn in_arc(gun: &GunInfo, at: Vec3) -> bool {
    let y = arc_yaw(gun, at);
    y >= gun.yaw_range[0] && y <= gun.yaw_range[1]
}

/// The spot behind the gun a bot mounts from, at the gun's height.
pub(super) fn mount_spot(gun: &GunInfo) -> Vec3 {
    let (s, c) = gun.yaw.to_radians().sin_cos();
    gun.origin - Vec3::new(c, s, 0.0) * BACK
}

/// The view from `eye` onto `at`: pitch (down positive), yaw.
fn look_at(eye: Vec3, at: Vec3) -> [f32; 2] {
    let d = at - eye;
    let flat = d.truncate().length();
    [-d.z.atan2(flat).to_degrees(), d.y.atan2(d.x).to_degrees()]
}

/// `from` turned toward `to` by at most `step` degrees an axis.
fn turn(from: [f32; 2], to: [f32; 2], step: f32) -> [f32; 2] {
    std::array::from_fn(|i| from[i] + angle_180(to[i] - from[i]).clamp(-step, step))
}

/// The cmd's angle for view angle `a` over `delta` (`ANGLE2SHORT` less
/// `delta_angles`).
fn cmd_angle(a: f32, delta: i32) -> i32 {
    (((a * 65536.0 / 360.0) as i32) - delta) & 0xffff
}

impl Cod2Bots {
    /// The gun step for the bot in `slot` this tick, after its route:
    /// a walk to a gun sets `view`'s waypoint. `guns` is empty where the
    /// gametype has objectives.
    pub(super) fn gun_step(
        &mut self,
        level: &Cod2Level,
        slot: usize,
        view: &mut BotView,
        guns: &[GunInfo],
        teams: &[i32],
        dm: bool,
    ) -> Option<GunCmd> {
        let now = level.level_time();
        let p = level.players.get(slot)?.as_ref()?;
        if guns.is_empty() || !playing(level, slot) {
            self.guns.remove(&slot);
            return None;
        }
        let mut task = self.guns.get(&slot).copied().unwrap_or_default();
        let mounted = p.ps.e_flags & ef::TURRET != 0;
        let out = if mounted {
            let gun = guns.iter().find(|g| g.owner == Some(slot))?;
            let my_eye = eye(&p.ps);
            let target = self.arc_enemy(level, slot, gun, my_eye, teams, dm);
            if target.is_some() || !task.manning || task.gun != Some(gun.id) {
                task.seen = now;
            }
            task.gun = Some(gun.id);
            task.manning = true;
            let cur = [p.ps.view_angles[0], p.ps.view_angles[1]];
            let (aim, fire) = match target {
                Some(at) => {
                    let want = look_at(my_eye, at);
                    let aim = turn(cur, want, TURN_PER_TICK);
                    let off = (0..2).map(|i| angle_180(want[i] - aim[i]).abs());
                    (aim, off.fold(0.0, f32::max) <= FIRE_CONE)
                }
                None => (cur, false),
            };
            let release = now - task.seen >= IDLE_RELEASE_MS && !task.pressed;
            task.pressed = release;
            Some(GunCmd::Man {
                view: aim,
                fire,
                release,
            })
        } else {
            if let Some(id) = task.gun {
                let gun = guns.iter().find(|g| g.id == id);
                let lost = gun.is_none_or(|g| g.owner.is_some());
                if task.manning || lost || now - task.since > APPROACH_MS {
                    task = GunTask::default();
                }
            }
            if task.gun.is_none()
                && let Some(gun) = self.pick_gun(level, slot, view, guns)
            {
                task = GunTask {
                    gun: Some(gun),
                    since: now,
                    ..GunTask::default()
                };
            }
            match task.gun.and_then(|id| guns.iter().find(|g| g.id == id)) {
                Some(gun) => {
                    view.waypoint = Some(mount_spot(gun).to_array());
                    view.stop = false;
                    let press = p.gunner.hint_ent == Some(gun.id) && !task.pressed;
                    task.pressed = press;
                    Some(GunCmd::Approach { press })
                }
                None => None,
            }
        };
        if task.gun.is_some() {
            self.guns.insert(slot, task);
        } else {
            self.guns.remove(&slot);
        }
        out
    }

    /// The nearest free gun a bot in `slot` should go for: an enemy it
    /// sees in the gun's arc and past [`MIN_RANGE`] from it, the spot
    /// behind within reach, level with its feet and walked to unblocked.
    fn pick_gun(
        &self,
        level: &Cod2Level,
        slot: usize,
        view: &BotView,
        guns: &[GunInfo],
    ) -> Option<EntId> {
        let enemy = Vec3::from(view.enemy?.origin);
        let feet = Vec3::from(view.origin);
        let taken: Vec<EntId> = (self.guns.iter())
            .filter(|(s, _)| **s != slot)
            .filter_map(|(_, t)| t.gun)
            .collect();
        let w = &level.world.collision;
        guns.iter()
            .filter(|g| g.owner.is_none() && !taken.contains(&g.id))
            .filter(|g| in_arc(g, enemy) && g.origin.distance(enemy) > MIN_RANGE)
            .filter_map(|g| {
                let spot = mount_spot(g);
                let floor = w.move_trace(
                    spot + Vec3::Z * 32.0,
                    spot - Vec3::Z * 96.0,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    vcod_cod2::pmove::MASK_PLAYERSOLID,
                );
                let spot = floor.endpos;
                let d = (spot - feet).truncate().length();
                if floor.fraction >= 1.0 || d > GUN_REACH || (spot.z - feet.z).abs() > GUN_STEP {
                    return None;
                }
                let walk = w.move_trace(
                    feet,
                    spot,
                    WALK_MINS,
                    WALK_MAXS,
                    vcod_cod2::pmove::MASK_PLAYERSOLID,
                );
                (walk.fraction >= 1.0).then_some((d, g.id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    /// The nearest foe whose chest is in `gun`'s arc, in range and in
    /// clear sight of `my_eye`.
    fn arc_enemy(
        &self,
        level: &Cod2Level,
        slot: usize,
        gun: &GunInfo,
        my_eye: Vec3,
        teams: &[i32],
        dm: bool,
    ) -> Option<Vec3> {
        if !self.shoot {
            return None;
        }
        let mine = teams.get(slot).copied().unwrap_or(0);
        let w = &level.world.collision;
        (level.players.iter().enumerate())
            .filter(|(i, _)| *i != slot && playing(level, *i))
            .filter_map(|(i, p)| {
                let p = p.as_ref()?;
                let theirs = teams.get(i).copied().unwrap_or(0);
                let foe = dm
                    || !(mine == TEAM_AXIS || mine == TEAM_ALLIES)
                    || (theirs != mine && (theirs == TEAM_AXIS || theirs == TEAM_ALLIES));
                let chest = Vec3::from(crate::bots::chest(
                    p.ps.origin.into(),
                    p.ps.view_height_current,
                ));
                let d = chest.distance(my_eye);
                (foe && d <= crate::bots::SHOOT_RANGE && in_arc(gun, chest)).then_some((d, chest))
            })
            .filter(|(_, chest)| w.shot_trace(my_eye, *chest).fraction >= 1.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, chest)| chest)
    }
}

/// The gun step's cmd over the brain's: on the gun nothing but the view,
/// the trigger and the release's use; on the way a use press added.
pub(super) fn apply(c: &mut UserCmd2, step: &GunCmd, delta: [i32; 3]) {
    match *step {
        GunCmd::Approach { press } => {
            c.buttons &= !USE;
            if press {
                c.buttons |= USE;
            }
        }
        GunCmd::Man {
            view,
            fire,
            release,
        } => {
            c.buttons = 0;
            if fire {
                c.buttons |= button::ATTACK;
            }
            if release {
                c.buttons |= USE;
            }
            c.forward_move = 0;
            c.right_move = 0;
            c.angles[0] = cmd_angle(view[0], delta[0]);
            c.angles[1] = cmd_angle(view[1], delta[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let spot = mount_spot(&g);
        assert!((spot - Vec3::new(100.0, -40.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn the_barrel_turns_a_step_at_a_time_the_short_way() {
        assert_eq!(turn([0.0, 170.0], [5.0, -170.0], 12.0), [5.0, 182.0]);
        assert_eq!(turn([0.0, 0.0], [30.0, 0.0], 12.0), [12.0, 0.0]);
        let v = look_at(Vec3::ZERO, Vec3::new(100.0, 100.0, -100.0 * 2f32.sqrt()));
        assert!((v[0] - 45.0).abs() < 1e-3 && (v[1] - 45.0).abs() < 1e-3);
    }

    #[test]
    fn a_gunner_cmd_holds_still_and_fires_or_lets_go() {
        let mut c = UserCmd2 {
            buttons: 0x200 | USE,
            forward_move: 127,
            ..Default::default()
        };
        let step = GunCmd::Man {
            view: [90.0, 0.0],
            fire: true,
            release: false,
        };
        apply(&mut c, &step, [0x4000, 0, 0]);
        assert_eq!(c.buttons, button::ATTACK);
        assert_eq!((c.forward_move, c.angles[0]), (0, 0));
    }
}
