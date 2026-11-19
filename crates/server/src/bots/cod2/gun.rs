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
use crate::bots::gun::{
    MIN_RANGE, aim_barrel, arc_enemy, floor_spot, in_arc, mount_spot, straight_walk,
};
use crate::game::cod2::Cod2Level;
use crate::game::cod2_turret::GunInfo;

/// How far from the spot behind a gun a bot sets off for it.
const GUN_REACH: f32 = 320.0;
/// The walk to a gun gives up after this, ms.
const APPROACH_MS: i32 = 5000;
/// A gunner with no enemy in the arc this long lets go, ms.
const IDLE_RELEASE_MS: i32 = 4000;
/// How near the aim the gunner fires, degrees.
const FIRE_CONE: f32 = 3.0;
/// The spot a bot mounts from, behind the gun along its yaw: where the
/// stock probes stand (cod2-turrets.md 10).
const BACK: f32 = 40.0;
/// What a walk to a gun clips against.
const MASK: u32 = vcod_cod2::pmove::MASK_PLAYERSOLID;
/// `+activate` in a CoD2 cmd (`to_cmd2`'s map of `BUTTON_USE`).
pub(super) const USE: i32 = 0x8;

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
            self.own.guns.remove(&slot);
            return None;
        }
        let mut task = self.own.guns.get(&slot).copied().unwrap_or_default();
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
            let (aim, fire) = aim_barrel(cur, my_eye, target, FIRE_CONE);
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
                    view.waypoint = Some(mount_spot(gun, BACK).to_array());
                    view.stop = false;
                    let press = p.gunner.hint_ent == Some(gun.id) && !task.pressed;
                    task.pressed = press;
                    Some(GunCmd::Approach { press })
                }
                None => None,
            }
        };
        if task.gun.is_some() {
            self.own.guns.insert(slot, task);
        } else {
            self.own.guns.remove(&slot);
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
        let taken: Vec<EntId> = (self.own.guns.iter())
            .filter(|(s, _)| **s != slot)
            .filter_map(|(_, t)| t.gun)
            .collect();
        let w = &level.world.collision;
        guns.iter()
            .filter(|g| g.owner.is_none() && !taken.contains(&g.id))
            .filter(|g| in_arc(g, enemy) && g.origin.distance(enemy) > MIN_RANGE)
            .filter_map(|g| {
                let spot = floor_spot(w, mount_spot(g, BACK), MASK)?;
                let d = (spot - feet).truncate().length();
                straight_walk(w, feet, spot, GUN_REACH, MASK).then_some((d, g.id))
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
        if !self.shoots() {
            return None;
        }
        let mine = teams.get(slot).copied().unwrap_or(0);
        let w = &level.world.collision;
        let foes = (level.players.iter().enumerate())
            .filter(|&(i, _)| i != slot && playing(level, i))
            .filter(|&(i, _)| crate::bots::foe(dm, mine, teams.get(i).copied().unwrap_or(0)))
            .filter_map(|(i, p)| {
                let p = p.as_ref()?;
                let chest = crate::bots::chest(p.ps.origin.into(), p.ps.view_height_current);
                Some((i, Vec3::from(chest)))
            });
        arc_enemy(gun, my_eye, foes, |chest| {
            w.shot_trace(my_eye, chest).fraction >= 1.0
        })
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
