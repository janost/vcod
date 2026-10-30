//! `ps.stats[3]` and `stats[4]`, the teammate under a client's crosshair
//! and his health, which the client's crosshair names colour by
//! (lnxded 0x811c148 from `G_RunFrame`'s pass 0x811c382; cod2-hud.md
//! 19.6).

use glam::Vec3;
use vcod_cod2::pmove::PlayerState;
use vcod_common::x87::angle_vectors;

use super::cod2::Cod2Level;
use super::script::TEAM_NONE;

/// The scan's reach (0x46000000 at 0x811c1c4).
const RANGE: f32 = 8192.0;
/// `CONTENTS_SOLID`: the world half of the trace's mask 0x2000001, whose
/// `CONTENTS_BODY` half the bodies' boxes take.
const CONTENTS_SOLID: u32 = 1;
/// `ps.pm_flags` 0x400000: following, which the pass skips.
const PMF_FOLLOW: u32 = 0x40_0000;

/// A body the scan can stop on: its slot, link box, team and health.
struct Body {
    slot: usize,
    lo: Vec3,
    hi: Vec3,
    team: i32,
    health: i32,
}

/// The eye the scan starts from: `G_GetPlayerViewOrigin`, then for a
/// client not playing whose `viewHeightCurrent` is under 8 the difference
/// added to its height (0x811c1f2, rodata 0x815eac8).
fn scan_eye(ps: &PlayerState, level_time: i32, playing: bool) -> Vec3 {
    let mut eye = Cod2Level::player_view_origin(ps, level_time);
    if !playing && ps.view_height_current < 8.0 {
        eye.z += 8.0 - ps.view_height_current;
    }
    eye
}

/// 0x811c148's choice: the nearest body but the viewer's own the segment
/// meets before the world's `fraction`, kept when the viewer is not
/// playing or shares its team.
fn pick(
    eye: Vec3,
    end: Vec3,
    fraction: f32,
    viewer: (usize, bool, i32),
    bodies: &[Body],
) -> Option<(u8, i32)> {
    let (own, playing, team) = viewer;
    let hit = bodies
        .iter()
        .filter(|b| b.slot != own)
        .filter_map(|b| {
            let t = super::combat::ray_box(eye, end, b.lo, b.hi)?;
            (t < fraction).then_some((t, b))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?
        .1;
    (!playing || hit.team == team).then_some((hit.slot as u8, hit.health))
}

impl Cod2Level {
    /// The pass after the end frames (0x811c382): each connected client
    /// not following gets the body its view meets within 8192 units, a
    /// capsule trace of no size under mask 0x2000001 passing its own
    /// entity, when that body is a client's and the viewer is not playing
    /// or is on its team; a playing client in no team (`dm`) gets none.
    /// VERIFIED, the stores. Script models' xmodel collision does not stop
    /// the trace here; the world and its brush models do.
    pub(super) fn identify_clients(&mut self) {
        let now = self.level_time();
        let mut bodies = Vec::new();
        for slot in 0..self.players.len() {
            if !self.playing(slot) {
                continue;
            }
            let Some(p) = self.players[slot].as_ref() else {
                continue;
            };
            let (origin, mins, maxs) = (p.ps.origin, p.ps.mins, p.ps.maxs);
            bodies.push(Body {
                slot,
                lo: origin + mins,
                hi: origin + maxs,
                team: self.rt.client_team(slot),
                health: self.rt.client_vitals(slot).health,
            });
        }
        for slot in 0..self.players.len() {
            let Some(p) = self.players[slot].as_ref() else {
                continue;
            };
            if p.ps.pm_flags & PMF_FOLLOW != 0 {
                continue;
            }
            let ps = p.ps.clone();
            let playing = self.rt.client_field(slot, "sessionstate").as_deref() == Some("playing");
            let team = self.rt.client_team(slot);
            let found = if playing && team == TEAM_NONE {
                None
            } else {
                let eye = scan_eye(&ps, now, playing);
                let forward = angle_vectors(Vec3::from_array(ps.view_angles))[0];
                let end = eye + forward * RANGE;
                let world = self
                    .world
                    .collision
                    .point_trace(eye, end, CONTENTS_SOLID, false);
                pick(eye, end, world.fraction, (slot, playing, team), &bodies)
            };
            self.rt.host.cod2_mut().clients[slot].identify = found;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};

    fn body(slot: usize, x: f32, team: i32, health: i32) -> Body {
        Body {
            slot,
            lo: Vec3::new(x - 15.0, -15.0, 0.0),
            hi: Vec3::new(x + 15.0, 15.0, 70.0),
            team,
            health,
        }
    }

    #[test]
    fn the_scan_names_the_nearest_teammate_before_the_wall() {
        let (eye, end) = (Vec3::new(0.0, 0.0, 60.0), Vec3::new(8192.0, 0.0, 60.0));
        let bodies = [body(2, 400.0, TEAM_AXIS, 80), body(3, 200.0, TEAM_AXIS, 55)];
        assert_eq!(
            pick(eye, end, 1.0, (0, true, TEAM_AXIS), &bodies),
            Some((3, 55))
        );
        // A wall at 100 units hides both.
        assert_eq!(
            pick(eye, end, 100.0 / 8192.0, (0, true, TEAM_AXIS), &bodies),
            None
        );
        // The nearest is an enemy: nothing, the teammate behind him is not
        // reached past him.
        let bodies = [
            body(2, 400.0, TEAM_AXIS, 80),
            body(3, 200.0, TEAM_ALLIES, 55),
        ];
        assert_eq!(pick(eye, end, 1.0, (0, true, TEAM_AXIS), &bodies), None);
        // A viewer not playing is shown either team.
        assert_eq!(
            pick(eye, end, 1.0, (0, false, TEAM_AXIS), &bodies),
            Some((3, 55))
        );
    }

    /// Two clients on mp_toujane, the shooter looking at the other's chest:
    /// `dm` names nobody, one team names the other with his health.
    /// Needs `COD2_DIR`.
    #[test]
    fn a_teammate_under_the_crosshair_rides_stats_3_and_4() {
        let Some(mut f) = crate::game::cod2_fight::fight() else {
            return;
        };
        f.face_off();
        assert_eq!(f.level.rt.host.cod2().clients[0].identify, None, "dm");
        for slot in 0..2 {
            let id = f.level.rt.client_entity(slot).unwrap();
            f.level.rt.with_host(|h, cx| {
                use vcod_gsc::Host;
                let v = vcod_gsc::Value::String(cx.intern_exact("axis"));
                let atom = cx.intern_folded("sessionteam");
                h.set_field(cx, id, atom, v).unwrap();
            });
        }
        f.frame();
        let health = f.health(1);
        assert_eq!(
            f.level.rt.host.cod2().clients[0].identify,
            Some((1, health))
        );
        let w = f.level.wire_playerstate(0, TEAM_AXIS).unwrap();
        assert_eq!((w.stats[3], w.stats[4]), (1, health));
    }
}
