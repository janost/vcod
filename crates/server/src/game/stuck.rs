//! `StuckInClient`: retail's push that separates two overlapping players
//! (docs/research/cod11-player-clip.md, "StuckInClient").

use glam::{Vec2, Vec3};
use vcod_common::movetrace::{CONTENTS_BODY, CONTENTS_CORPSE};

/// One slot's view into the scan: everything `StuckInClient` reads off a
/// client, built fresh per caller so a slot scanned earlier in the same end
/// frame is seen with its post-push contents.
#[derive(Clone, Copy, Debug)]
pub struct StuckView {
    /// `pm_flags & PMF_OWN_VIEW`; a clear one anywhere in the scan aborts it.
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
    /// UO's `EF_IN_VEHICLE`: a seated player is no partner (game.mp.uo
    /// 0x5258c).
    pub seated: bool,
}

/// Which module's scan: 1.1's gives up on any slot without its own view,
/// 1.5's and UO's pass over it (cod11-player-clip.md 6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StuckRules {
    V1_1,
    V1_5,
}

pub struct Push {
    pub other: usize,
    pub self_vel: Vec2,
    pub other_vel: Vec2,
}

/// Retail's jitter term off the caller's 31-bit `rand()`: 1.1's `-1 -
/// r/2^30`, 1.5's and UO's `r/2^31 + r/2^31 - 1` (cod11-player-clip.md 6.4,
/// 13.4). Both are exact in an `f64`, as on the x87 stack.
fn jitter(rules: StuckRules, rand: &mut impl FnMut() -> u32) -> f64 {
    let r = rand() as f64;
    match rules {
        StuckRules::V1_1 => -1.0 - r / 1073741824.0,
        StuckRules::V1_5 => r / 2147483648.0 * 2.0 - 1.0,
    }
}

/// The push direction: the separation plus the jitter, summed on the x87
/// stack and stored, then each module's `VectorNormalize2D`. 1.1d's keeps
/// the squared length, the reciprocal and the products on the stack and
/// stores only the length; 1.5b's and UO's store the squared length and
/// the reciprocal as floats (player-clip doc 13.4).
fn push_dir(rules: StuckRules, d: Vec2, jx: f64, jy: f64) -> Vec2 {
    let x = (d.x as f64 + jx) as f32;
    let y = (d.y as f64 + jy) as f32;
    let (xd, yd) = (x as f64, y as f64);
    match rules {
        StuckRules::V1_1 => {
            let len = (xd * xd + yd * yd).sqrt() as f32;
            if len == 0.0 {
                return Vec2::new(x, y);
            }
            let il = 1.0 / len as f64;
            Vec2::new((xd * il) as f32, (yd * il) as f32)
        }
        StuckRules::V1_5 => {
            let len = ((xd * xd + yd * yd) as f32).sqrt();
            if len == 0.0 {
                return Vec2::new(x, y);
            }
            let il = 1.0 / len;
            Vec2::new(x * il, y * il)
        }
    }
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
    rules: StuckRules,
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
            if rules == StuckRules::V1_1 {
                return None;
            }
            continue;
        }
        let other_body = other.contents == CONTENTS_BODY || other.contents == CONTENTS_CORPSE;
        if !other.playing || i == me || other.health <= 0 || !other_body || other.seated {
            continue;
        }
        if !overlaps(&my, other) {
            continue;
        }
        let d = (other.origin - my.origin).truncate();
        let jx = jitter(rules, &mut rand);
        let jy = jitter(rules, &mut rand);
        let dir = push_dir(rules, d, jx, jy);
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

/// One CoD2 client as its `StuckInClient` (lnxded 0x80f6e9e) reads it:
/// the linked entity's box and the gclient's playerstate.
#[derive(Clone, Copy, Debug)]
pub struct Cod2StuckView {
    /// `pm_flags` 0x800000 (`ClientEndFrame`'s mark on a playing client)
    /// with `sessionState` 0.
    pub playing: bool,
    pub health: i32,
    /// `r.contents`: only 0x2000000 and 0x4000000 take part.
    pub contents: u32,
    /// `r.currentOrigin`, at the last link.
    pub origin: Vec3,
    /// `r.mins` and `r.maxs`: `r.absmin`/`r.absmax` are these about the
    /// origin grown a unit (`SV_LinkEntity` 0x809c63a), and the radius test
    /// sums the two `r.maxs[0]`.
    pub mins: Vec3,
    pub maxs: Vec3,
    pub vel_xy: Vec2,
    /// `ps.speed`.
    pub speed: f32,
}

/// CoD2's `StuckInClient` (lnxded 0x80f6e9e) for `me`, slots in order:
/// the first other playing client with health whose grown box meets
/// `me`'s inclusively and whose origin lies within the two `maxs[0]` in
/// x/y is pushed along the separation plus a `G_crandom()` an axis
/// (normalized), `me` the other way, each at `eject`
/// (`g_playerCollisionEjectSpeed`) when its x/y velocity is not 0, both at
/// their `ps.speed` when neither moves (cod2-turrets.md 14.2).
pub fn stuck_in_client_cod2(
    me: usize,
    views: &[Option<Cod2StuckView>],
    eject: f32,
    mut rand: impl FnMut() -> u32,
) -> Option<Push> {
    let body = |v: &Cod2StuckView| v.contents == CONTENTS_BODY || v.contents == CONTENTS_CORPSE;
    let my = views.get(me).copied().flatten()?;
    if !my.playing || !body(&my) {
        return None;
    }
    let grown = |v: &Cod2StuckView| {
        let lo = v.origin + v.mins;
        let hi = v.origin + v.maxs;
        (lo - Vec3::ONE, hi + Vec3::ONE)
    };
    let (my_lo, my_hi) = grown(&my);
    for (i, slot) in views.iter().enumerate() {
        let Some(other) = slot else { continue };
        if i == me || !other.playing || other.health <= 0 || !body(other) {
            continue;
        }
        let (o_lo, o_hi) = grown(other);
        if !(o_lo.cmple(my_hi).all() && my_lo.cmple(o_hi).all()) {
            continue;
        }
        let d = (other.origin - my.origin).truncate();
        let r = f64::from(my.maxs.x) + f64::from(other.maxs.x);
        let (dx, dy) = (f64::from(d.x), f64::from(d.y));
        if dx * dx + dy * dy > r * r {
            continue;
        }
        // `G_crandom` (0x80a3792): `2 * rand() / 2^31 - 1`, each sum stored.
        let jx = jitter(StuckRules::V1_5, &mut rand);
        let jy = jitter(StuckRules::V1_5, &mut rand);
        let x = (f64::from(d.x) + jx) as f32;
        let y = (f64::from(d.y) + jy) as f32;
        // `Vec2Normalize` (0x80a44c2): the squared length stored for
        // `I_sqrt`, the length stored, the reciprocal kept on the stack.
        let len = f64::from((f64::from(x) * f64::from(x) + f64::from(y) * f64::from(y)) as f32)
            .sqrt() as f32;
        let dir = if len == 0.0 {
            Vec2::new(x, y)
        } else {
            let il = 1.0 / f64::from(len);
            Vec2::new((f64::from(x) * il) as f32, (f64::from(y) * il) as f32)
        };
        let moving = |v: &Cod2StuckView| {
            if v.vel_xy.length_squared() > 0.0 {
                eject
            } else {
                0.0
            }
        };
        let (mut s_me, mut s_other) = (moving(&my), moving(other));
        // rodata 0x8157aac.
        if s_other < 1e-4 && s_me < 1e-4 {
            s_other = other.speed;
            s_me = my.speed;
        }
        return Some(Push {
            other: i,
            self_vel: dir * -s_me,
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
            seated: false,
        }
    }
    const MID: u32 = 1 << 30; // j = -1 - 2^30 / 2^30 = -2

    #[test]
    fn both_still_both_take_speed() {
        let views = [Some(view(0.0, 0.0)), Some(view(10.0, 0.0))];
        let p = stuck_in_client(0, &views, StuckRules::V1_1, || MID).unwrap();
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
        let p = stuck_in_client(0, &views, StuckRules::V1_1, || MID).unwrap();
        assert_eq!(p.other_vel, Vec2::ZERO);
        assert!((p.self_vel.length() - 190.0).abs() < 1e-3);
    }

    #[test]
    fn apart_is_not_stuck() {
        let views = [Some(view(0.0, 0.0)), Some(view(31.0, 0.0))];
        assert!(stuck_in_client(0, &views, StuckRules::V1_1, || MID).is_none());
    }

    #[test]
    fn a_slot_without_own_view_ends_the_scan() {
        let spec = StuckView {
            own_view: false,
            ..view(500.0, 0.0)
        };
        let views = [Some(spec), Some(view(0.0, 0.0)), Some(view(10.0, 0.0))];
        assert!(stuck_in_client(1, &views, StuckRules::V1_1, || MID).is_none());
        let views = [Some(view(0.0, 0.0)), Some(view(10.0, 0.0)), Some(spec)];
        assert!(
            stuck_in_client(0, &views, StuckRules::V1_1, || MID).is_some(),
            "the hit in slot 1 returns before slot 2"
        );
    }

    #[test]
    fn on_15_a_slot_without_own_view_is_passed_over() {
        let spec = StuckView {
            own_view: false,
            ..view(500.0, 0.0)
        };
        let views = [Some(spec), Some(view(0.0, 0.0)), Some(view(0.0, 0.0))];
        // r = 2^30 is a jitter of 0 on 1.5; r = 2^30 * 3/2 is 0.5.
        let mut r = [1u32 << 30, 3 << 29].into_iter();
        let p = stuck_in_client(1, &views, StuckRules::V1_5, || r.next().unwrap()).unwrap();
        assert_eq!(p.other, 2);
        assert_eq!(p.other_vel, Vec2::new(0.0, 190.0));
        let seated = StuckView {
            seated: true,
            ..view(0.0, 0.0)
        };
        let views = [Some(view(0.0, 0.0)), Some(seated)];
        assert!(stuck_in_client(0, &views, StuckRules::V1_5, || MID).is_none());
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
        assert!(stuck_in_client(0, &views, StuckRules::V1_1, || MID).is_some());
    }

    /// One end frame of a capture's pair: each slot's origin and whether it
    /// moved, and the higher slot's velocity after its push.
    struct CapturedPush {
        lo: [f32; 3],
        hi: [f32; 3],
        lo_moving: bool,
        hi_moving: bool,
        hi_vel: [f32; 2],
        /// The origins are a capture's three-decimal print, so the push
        /// reads to that, not to the bit.
        rounded: bool,
    }

    /// Replays a capture's push frames on glibc's `rand()` from `seed`
    /// after `skip` draws: per frame the lower slot scans and draws, then
    /// the higher one, whose push both velocities keep.
    fn replay(rules: StuckRules, seed: u32, skip: usize, lo_slot: usize, frames: &[CapturedPush]) {
        let mut g = vcod_common::rng::GlibcRand::new(seed);
        for _ in 0..skip {
            g.next_u31();
        }
        for (n, f) in frames.iter().enumerate() {
            let at = |o: [f32; 3], moving: bool| StuckView {
                origin: Vec3::from(o),
                vel_xy: if moving { Vec2::X } else { Vec2::ZERO },
                ..view(0.0, 0.0)
            };
            let spec = StuckView {
                own_view: false,
                ..view(500.0, 0.0)
            };
            let mut views = vec![Some(spec); lo_slot];
            views.push(Some(at(f.lo, f.lo_moving)));
            views.push(Some(at(f.hi, f.hi_moving)));
            stuck_in_client(lo_slot, &views, rules, || g.next_u31()).expect("the lower slot");
            let p = stuck_in_client(lo_slot + 1, &views, rules, || g.next_u31())
                .expect("the higher slot");
            let want = Vec2::from(f.hi_vel);
            if f.rounded {
                assert!((p.self_vel - want).length() < 0.05, "push {n}: {p:?}");
            } else {
                assert_eq!(p.self_vel, want, "push {n}");
                let lo_want = if f.lo_moving == f.hi_moving {
                    -want
                } else {
                    Vec2::ZERO
                };
                assert_eq!(p.other_vel, lo_want, "push {n}");
            }
        }
    }

    impl std::fmt::Debug for Push {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} {:?} {:?}", self.other, self.self_vel, self.other_vel)
        }
    }

    /// 1.1d's push velocities in `mp_carentan-dm-bump-overlap-*` (target
    /// slot 0, walker slot 1), to the bit, off `srand(631)` past six
    /// draws: both still, then pushed, then the walker walking into a
    /// still target four frames running (player-clip doc 13.4).
    #[test]
    fn the_11_overlap_capture_replays_from_its_seed() {
        let still = |hi: [f32; 3], hi_vel: [f32; 2]| CapturedPush {
            lo: [1145.3181, -330.0834, -151.64139],
            hi,
            lo_moving: false,
            hi_moving: true,
            hi_vel,
            rounded: false,
        };
        let frames = [
            CapturedPush {
                lo: [932.0, -376.0, -150.875],
                hi: [932.0, -376.0, -151.875],
                lo_moving: false,
                hi_moving: false,
                hi_vel: [126.06007, 142.15787],
                rounded: false,
            },
            CapturedPush {
                lo: [925.825, -382.9607, -151.8514],
                hi: [937.8682, -369.41397, -151.875],
                lo_moving: true,
                hi_moving: true,
                hi_vel: [128.05652, 140.36215],
                rounded: false,
            },
            CapturedPush {
                lo: [1145.3181, -330.0834, -150.875],
                ..still([1145.3181, -330.0834, -151.875], [79.05528, 172.7723])
            },
            still([1149.8934, -321.8941, -151.875], [99.19997, 162.04741]),
            still([1155.3969, -314.23483, -151.875], [111.7774, 153.64183]),
            still([1161.658, -306.8345, -151.875], [111.84219, 153.59467]),
        ];
        replay(StuckRules::V1_1, 631, 6, 0, &frames);
    }

    /// The `RIDE` pushes of `mp_carentan-dm-pushcand-1.5.txt` and `-uo.txt`
    /// (a spectator in slot 0, `p1` and `p2` in 1 and 2), off `srand(428)`
    /// and `srand(439)` past eight draws each: the stacked pair, its next
    /// frame (origins printed to three decimals) and the half pair.
    #[test]
    fn the_15_and_uo_pushcand_captures_replay_from_their_seeds() {
        let push = |lo: [f32; 3], hi: [f32; 3], vel: [f32; 2], rounded: bool| CapturedPush {
            lo,
            hi,
            lo_moving: rounded,
            hi_moving: rounded,
            // The capture logs p1's, the lower slot's: the higher's is
            // its reverse.
            hi_vel: [-vel[0], -vel[1]],
            rounded,
        };
        let stack = [-215.0, 2430.0, -30.0];
        let half = [-200.0, 2430.0, -30.0];
        replay(
            StuckRules::V1_5,
            428,
            8,
            1,
            &[
                push(stack, stack, [-170.53241, -83.77766], false),
                push(
                    [-223.542, 2425.804, -30.99],
                    [-203.551, 2435.624, -31.792],
                    [-166.98854, -90.635666],
                    true,
                ),
                push(stack, half, [-189.8796, -6.762784], false),
            ],
        );
        replay(
            StuckRules::V1_5,
            439,
            8,
            1,
            &[
                push(stack, stack, [179.16396, 63.247704], false),
                push(
                    [-206.047, 2433.155, -30.99],
                    [-229.684, 2424.825, -31.874],
                    [178.51088, 65.068146],
                    true,
                ),
                push(stack, half, [-189.5686, -12.796347], false),
            ],
        );
    }

    /// The draw's ends against the binaries' constants: 1.1d's `-1 -
    /// r/2^30` spans (-3, -1], 1.5b's `2r/2^31 - 1` [-1, 1).
    #[test]
    fn the_jitter_spans_each_modules_band() {
        let j = |rules, r: u32| jitter(rules, &mut || r);
        assert_eq!(j(StuckRules::V1_1, 0), -1.0);
        assert_eq!(j(StuckRules::V1_1, 1 << 30), -2.0);
        assert_eq!(j(StuckRules::V1_1, i32::MAX as u32), -3.0 + 2f64.powi(-30));
        assert_eq!(j(StuckRules::V1_5, 0), -1.0);
        assert_eq!(j(StuckRules::V1_5, 1 << 30), 0.0);
        assert_eq!(j(StuckRules::V1_5, i32::MAX as u32), 1.0 - 2f64.powi(-30));
    }

    fn cod2_view(x: f32, vx: f32) -> Cod2StuckView {
        Cod2StuckView {
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

    /// CoD2's push: the eject speed for a mover, `ps.speed` for both when
    /// neither moves, apart along the separation; out of reach or a
    /// non-body partner, nothing (lnxded 0x80f6e9e).
    #[test]
    fn cod2_pushes_overlapping_players_apart_at_the_eject_speed() {
        const MID: u32 = 1 << 30;
        let still = [Some(cod2_view(0.0, 0.0)), Some(cod2_view(20.0, 0.0))];
        let p = stuck_in_client_cod2(0, &still, 25.0, || MID).unwrap();
        assert_eq!(p.other, 1);
        assert_eq!(p.other_vel, Vec2::new(190.0, 0.0));
        assert_eq!(p.self_vel, Vec2::new(-190.0, 0.0));
        let moving = [Some(cod2_view(0.0, 0.0)), Some(cod2_view(20.0, 50.0))];
        let p = stuck_in_client_cod2(0, &moving, 25.0, || MID).unwrap();
        assert_eq!(
            (p.self_vel, p.other_vel),
            (Vec2::ZERO, Vec2::new(25.0, 0.0))
        );
        // 30 apart is the two radii, inclusive; 31 is out.
        let edge = [Some(cod2_view(0.0, 0.0)), Some(cod2_view(30.0, 0.0))];
        assert!(stuck_in_client_cod2(0, &edge, 25.0, || MID).is_some());
        let out = [Some(cod2_view(0.0, 0.0)), Some(cod2_view(31.0, 0.0))];
        assert!(stuck_in_client_cod2(0, &out, 25.0, || MID).is_none());
        // The grown boxes meet two units past the boxes in z.
        let mut above = cod2_view(0.0, 0.0);
        above.origin.z = 71.5;
        assert!(
            stuck_in_client_cod2(0, &[Some(cod2_view(0.0, 0.0)), Some(above)], 25.0, || MID)
                .is_some()
        );
        above.origin.z = 72.5;
        assert!(
            stuck_in_client_cod2(0, &[Some(cod2_view(0.0, 0.0)), Some(above)], 25.0, || MID)
                .is_none()
        );
        let mut gone = cod2_view(20.0, 0.0);
        gone.contents = 0;
        assert!(
            stuck_in_client_cod2(0, &[Some(cod2_view(0.0, 0.0)), Some(gone)], 25.0, || MID)
                .is_none()
        );
    }
}
