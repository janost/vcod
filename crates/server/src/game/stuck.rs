//! `StuckInClient`: retail's push that separates two overlapping players
//! (docs/research/cod11-player-clip.md, "Plan-phase reads" 2).

use glam::{Vec2, Vec3};
use vcod_common::movetrace::{CONTENTS_BODY, CONTENTS_CORPSE};

/// One slot's view into the scan: everything `StuckInClient` reads off a
/// client, built fresh per caller so a slot scanned earlier in the same end
/// frame is seen with its post-push contents.
#[derive(Clone, Copy, Debug)]
pub struct StuckView {
    /// `pm_flags & PMF_OWN_VIEW`: set for a connected client in sessionState
    /// 0 or 1 (playing or dead), clear for a spectator, an intermission
    /// client or one still on the connect menu. A clear `own_view` anywhere
    /// in the scan order aborts the whole thing.
    pub own_view: bool,
    /// sessionState 0: playing, not dead.
    pub playing: bool,
    pub health: i32,
    pub contents: u32,
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub vel_xy: Vec2,
    pub speed: f32,
}

pub struct Push {
    pub other: usize,
    pub self_vel: Vec2,
    pub other_vel: Vec2,
}

/// `-1 - rand()/2^30`, retail's jitter term (rodata 0x72d30 read as an
/// overflowed `RAND_MAX + 1`); `rand` is the caller's 31-bit `rand()`.
fn jitter(rand: &mut impl FnMut() -> u32) -> f32 {
    -1.0 - rand() as f32 / (1u32 << 30) as f32
}

/// Inclusive AABB overlap on the absolute boxes, then the capsule radius
/// test on x/y.
fn overlaps(me: &StuckView, other: &StuckView) -> bool {
    let (my_min, my_max) = (me.origin + me.mins, me.origin + me.maxs);
    let (o_min, o_max) = (other.origin + other.mins, other.origin + other.maxs);
    if my_min.x > o_max.x
        || my_max.x < o_min.x
        || my_min.y > o_max.y
        || my_max.y < o_min.y
        || my_min.z > o_max.z
        || my_max.z < o_min.z
    {
        return false;
    }
    let d = (other.origin - me.origin).truncate();
    let r = me.maxs.x + other.maxs.x;
    d.length_squared() <= r * r
}

/// `StuckInClient` (game.mp 0x40934) for `me`; `None` in a slot is a free
/// slot.
pub fn stuck_in_client(
    me: usize,
    views: &[Option<StuckView>],
    mut rand: impl FnMut() -> u32,
) -> Option<Push> {
    let my = views.get(me).copied().flatten()?;
    let self_body = my.contents == CONTENTS_BODY || my.contents == CONTENTS_CORPSE;
    if !my.own_view || !my.playing || !self_body {
        return None;
    }
    for (i, slot) in views.iter().enumerate() {
        let Some(other) = slot else { continue };
        if !other.own_view {
            return None;
        }
        let other_body = other.contents == CONTENTS_BODY || other.contents == CONTENTS_CORPSE;
        if !other.playing || i == me || other.health <= 0 || !other_body {
            continue;
        }
        if !overlaps(&my, other) {
            continue;
        }
        let j = Vec2::new(jitter(&mut rand), jitter(&mut rand));
        let dir = ((other.origin - my.origin).truncate() + j).normalize_or_zero();
        let mut s_me = if my.vel_xy.length_squared() > 0.0 {
            my.speed
        } else {
            0.0
        };
        let mut s_other = if other.vel_xy.length_squared() > 0.0 {
            other.speed
        } else {
            0.0
        };
        if s_me < 1e-4 && s_other < 1e-4 {
            s_me = my.speed;
            s_other = other.speed;
        }
        return Some(Push {
            other: i,
            self_vel: -dir * s_me,
            other_vel: dir * s_other,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(x: f32, vx: f32) -> StuckView {
        StuckView {
            own_view: true,
            playing: true,
            health: 100,
            contents: CONTENTS_BODY,
            origin: Vec3::new(x, 0.0, 0.0),
            mins: Vec3::new(-15.0, -15.0, 0.0),
            maxs: Vec3::new(15.0, 15.0, 70.0),
            vel_xy: Vec2::new(vx, 0.0),
            speed: 190.0,
        }
    }
    const MID: u32 = 1 << 30; // j = -1 - 2^30 / 2^30 = -2

    #[test]
    fn both_still_both_take_speed() {
        let views = [Some(view(0.0, 0.0)), Some(view(10.0, 0.0))];
        let p = stuck_in_client(0, &views, || MID).unwrap();
        assert_eq!(p.other, 1);
        // dir = normalize((10, 0) + (-2, -2)) = normalize(8, -2)
        let d = Vec2::new(8.0, -2.0).normalize();
        assert!(
            (p.other_vel - d * 190.0).length() < 1e-3 && (p.self_vel + d * 190.0).length() < 1e-3
        );
    }

    #[test]
    fn only_the_mover_is_pushed() {
        let views = [Some(view(0.0, 100.0)), Some(view(10.0, 0.0))];
        let p = stuck_in_client(0, &views, || MID).unwrap();
        assert_eq!(p.other_vel, Vec2::ZERO);
        assert!((p.self_vel.length() - 190.0).abs() < 1e-3);
    }

    #[test]
    fn apart_is_not_stuck() {
        let views = [Some(view(0.0, 0.0)), Some(view(31.0, 0.0))];
        assert!(stuck_in_client(0, &views, || MID).is_none());
    }

    #[test]
    fn a_slot_without_own_view_ends_the_scan() {
        let spec = StuckView {
            own_view: false,
            ..view(500.0, 0.0)
        };
        let views = [Some(spec), Some(view(0.0, 0.0)), Some(view(10.0, 0.0))];
        assert!(stuck_in_client(1, &views, || MID).is_none());
        let views = [Some(view(0.0, 0.0)), Some(view(10.0, 0.0)), Some(spec)];
        assert!(
            stuck_in_client(0, &views, || MID).is_some(),
            "the hit in slot 1 returns before slot 2"
        );
    }

    #[test]
    fn a_corpse_marked_player_still_counts() {
        let views = [
            Some(view(0.0, 0.0)),
            Some(StuckView {
                contents: CONTENTS_CORPSE,
                ..view(10.0, 0.0)
            }),
        ];
        assert!(stuck_in_client(0, &views, || MID).is_some());
    }
}
