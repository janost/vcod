//! The locational trace: a ray against the per-bone hit boxes of a posed
//! skeleton, which is where a hit's 0..18 location index comes from.
//! `docs/research/cod11-combat.md` section 3.

use crate::patch::Arith;
use crate::skeleton::{PoseBuffer, Skeleton, axes_stored};
use glam::Vec3;

/// The winning bone, in the frame the ray was given in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneHit {
    /// Along `start..end`; 0 when the ray started inside a box.
    pub fraction: f32,
    pub hit_location: u8,
    /// Skeleton bone index.
    pub bone: usize,
    /// The segment lies inside this bone's box end to end, which retail
    /// reports as all-solid at fraction 0 and returns on at once.
    pub start_solid: bool,
    /// The struck face's outward normal, a signed axis of the bone's frame
    /// turned into the ray's (combat doc 3.1); zero on a start-solid.
    pub normal: Vec3,
}

/// The priority the trace ranks candidates by, one byte per hit location.
/// `bulletPriorityMap` and `riflePriorityMap` (combat doc, 2.3).
pub type PriorityMap = [u8; 19];

/// Where `bestPriority` starts, so `gun` (priority 0) and `none` (1) are
/// never hit under either shipped map.
const MIN_PRIORITY: u8 = 2;

/// Entry fraction of a segment through an AABB and the entered face's
/// outward normal, or `None` when it misses. The bool is set when the
/// segment starts inside.
fn slab(start: Vec3, dir: Vec3, lo: Vec3, hi: Vec3) -> Option<(f32, bool, Vec3)> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let mut inside = true;
    let mut normal = Vec3::ZERO;
    for axis in 0..3 {
        if dir[axis].abs() < 1e-6 {
            if start[axis] < lo[axis] || start[axis] > hi[axis] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / dir[axis];
        let (mut a, mut b) = (
            (lo[axis] - start[axis]) * inv,
            (hi[axis] - start[axis]) * inv,
        );
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        if a > t0 {
            t0 = a;
            inside = false;
            normal = Vec3::ZERO;
            normal[axis] = -dir[axis].signum();
        }
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, inside, normal))
}

/// The bone the segment `start..end` scores on `pose`, both in the skeleton's
/// own frame (the entity's origin and angles are the caller's to undo).
///
/// The rules are retail's, in its order (combat doc, section 3): a bone with
/// a zero-size box is skipped, a bone whose priority is below the best found
/// so far is skipped before any geometry runs, a box is clipped to the
/// fraction found so far (`limit` before any), so it scores only where the
/// segment enters it no farther, a higher priority then takes the hit, an
/// equal one only when nearer. A segment inside a box end to end returns
/// at once; one that starts inside a box and leaves it scores nothing on
/// that bone and the walk goes on (1.1d `cod_lnxded` 0x80c4bc5, UO
/// 0x80c7a8a). A higher priority beats distance only in bone order.
pub fn bone_trace(
    skel: &Skeleton,
    pose: &PoseBuffer,
    start: Vec3,
    end: Vec3,
    priority: &PriorityMap,
    limit: f32,
) -> Option<BoneHit> {
    let worlds = pose.bone_worlds(skel);
    let mut best: Option<BoneHit> = None;
    let mut best_priority = MIN_PRIORITY;
    for (bi, b) in skel.bones().iter().enumerate() {
        if !b.has_hit_box() {
            continue;
        }
        let prio = priority.get(b.hit_location as usize).copied().unwrap_or(0);
        if prio < best_priority {
            continue;
        }
        // Into the bone's own frame, where the box is an AABB.
        let (bp, br) = worlds[bi];
        let inv = br.inverse();
        let (ls, le) = (inv * (start - bp), inv * (end - bp));
        let Some((t, inside, n)) = slab(ls, le - ls, b.hit_mins, b.hit_maxs) else {
            continue;
        };
        let ends_in = (0..3).all(|a| le[a] >= b.hit_mins[a] && le[a] <= b.hit_maxs[a]);
        if inside && !ends_in {
            continue;
        }
        if inside {
            return Some(BoneHit {
                fraction: 0.0,
                hit_location: b.hit_location,
                bone: bi,
                start_solid: true,
                normal: Vec3::ZERO,
            });
        }
        let bound = best.map_or(limit, |h| h.fraction);
        if t > bound || (prio == best_priority && best.is_some() && t >= bound) {
            continue;
        }
        best_priority = prio;
        best = Some(BoneHit {
            fraction: t,
            hit_location: b.hit_location,
            bone: bi,
            start_solid: false,
            normal: br * n,
        });
    }
    best
}

/// [`bone_trace`] in `arith`'s build. 1.1d's keeps its floats on the
/// stack, which [`bone_trace`] stands in for; 1.5b's and United
/// Offensive's (`SV_LocationalTraceModel`, 1.5b `cod_lnxded` 0x80d2000, UO
/// `coduo_lnxded` 0x80c7394) store every assignment: each point moved into
/// a bone's frame, each slab distance, each denominator and each fraction
/// (cod11-combat.md 3.1, "The arithmetic on 1.5 and UO").
pub fn bone_trace_in(
    skel: &Skeleton,
    pose: &PoseBuffer,
    start: Vec3,
    end: Vec3,
    priority: &PriorityMap,
    limit: f32,
    arith: Arith,
) -> Option<BoneHit> {
    match arith {
        Arith::Stored => bone_trace_stored(skel, pose, start, end, priority, limit),
        Arith::Kept | Arith::Msvc => bone_trace(skel, pose, start, end, priority, limit),
    }
}

/// A float variable the -O0 build stores.
fn st(x: f64) -> f32 {
    x as f32
}

/// UO 0x80c72ea (1.5b 0x80d1f56): `p` into the frame whose axes are
/// `rows`, at `origin`: each difference stored, each dot product rounded
/// once.
pub fn into_frame_stored(p: Vec3, rows: &[Vec3; 3], origin: Vec3) -> Vec3 {
    let d = (p - origin).as_dvec3();
    let dot = |r: Vec3| st(d.x * f64::from(r.x) + d.y * f64::from(r.y) + d.z * f64::from(r.z));
    Vec3::new(dot(rows[0]), dot(rows[1]), dot(rows[2]))
}

/// 1.5b's and UO's slab test, store for store (UO 0x80c788a..0x80c7a8a).
fn bone_trace_stored(
    skel: &Skeleton,
    pose: &PoseBuffer,
    start: Vec3,
    end: Vec3,
    priority: &PriorityMap,
    limit: f32,
) -> Option<BoneHit> {
    // Retail's build off the engine-side records where the pose kept them.
    let worlds: Vec<(Vec3, [Vec3; 3])> = match pose.stored_worlds(skel) {
        Some(w) => w.iter().map(|b| (b.origin, b.axes)).collect(),
        None => pose
            .bone_worlds(skel)
            .iter()
            .map(|&(p, q)| (p, axes_stored(q.to_array())))
            .collect(),
    };
    let mut best: Option<BoneHit> = None;
    let mut fraction = limit;
    let mut best_priority = MIN_PRIORITY;
    'bones: for (bi, b) in skel.bones().iter().enumerate() {
        if !b.has_hit_box() {
            continue;
        }
        let prio = priority.get(b.hit_location as usize).copied().unwrap_or(0);
        if prio < best_priority {
            continue;
        }
        let (bp, rows) = worlds[bi];
        let (ls, le) = (
            into_frame_stored(start, &rows, bp),
            into_frame_stored(end, &rows, bp),
        );
        let (mut enter, mut exit) = (0.0f32, fraction);
        let (mut starts_in, mut ends_in) = (true, true);
        let mut face: Option<(usize, f32)> = None;
        for (sign, planes) in [(-1.0f32, b.hit_mins), (1.0, b.hit_maxs)] {
            for axis in 0..3 {
                let d1 = st((f64::from(ls[axis]) - f64::from(planes[axis])) * f64::from(sign));
                let d2 = st((f64::from(le[axis]) - f64::from(planes[axis])) * f64::from(sign));
                if d1 > 0.0 {
                    if d2 > 0.0 {
                        continue 'bones;
                    }
                    starts_in = false;
                    let den = st(f64::from(d1) - f64::from(d2));
                    if f64::from(d1) > f64::from(enter) * f64::from(den) {
                        enter = st(f64::from(d1) / f64::from(den));
                        if enter >= exit {
                            continue 'bones;
                        }
                        face = Some((axis, sign));
                    }
                } else if d2 > 0.0 {
                    ends_in = false;
                    let den = st(f64::from(d1) - f64::from(d2));
                    if f64::from(d1) > f64::from(exit) * f64::from(den) {
                        exit = st(f64::from(d1) / f64::from(den));
                        if enter >= exit {
                            continue 'bones;
                        }
                    }
                }
            }
        }
        // One that starts inside and leaves flags the trace start-solid,
        // which nothing reads, and scores nothing here (UO 0x80c7a8a).
        if starts_in && !ends_in {
            continue;
        }
        if starts_in {
            return Some(BoneHit {
                fraction: 0.0,
                hit_location: b.hit_location,
                bone: bi,
                start_solid: true,
                normal: Vec3::ZERO,
            });
        }
        if prio == best_priority && best.is_some() && enter >= fraction {
            continue;
        }
        best_priority = prio;
        fraction = enter;
        let normal = face.map_or(Vec3::ZERO, |(axis, sign)| rows[axis] * sign);
        best = Some(BoneHit {
            fraction: enter,
            hit_location: b.hit_location,
            bone: bi,
            start_solid: false,
            normal,
        });
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bone, Model};
    use glam::Quat;

    /// `riflePriorityMap` (combat doc, 2.3).
    const RIFLE: PriorityMap = [1, 9, 9, 9, 8, 7, 6, 6, 6, 6, 5, 5, 4, 4, 4, 4, 3, 3, 0];

    fn bone(name: &str, pos: Vec3, half: Vec3, hit_location: u8) -> Bone {
        Bone {
            name: name.into(),
            parent: -1,
            pos,
            rot: Quat::IDENTITY,
            local_pos: pos,
            local_rot: Quat::IDENTITY,
            hit_mins: -half,
            hit_maxs: half,
            hit_location,
        }
    }

    /// Unparented bones at the positions given, so a ray down +X meets their
    /// boxes in order and only the trace's ranking is under test.
    fn rig(bones: Vec<Bone>) -> (Skeleton, PoseBuffer) {
        let m = Model::rig("t", bones);
        let skel = Skeleton::build(&[&m]);
        let pose = PoseBuffer::new(&skel);
        (skel, pose)
    }

    fn shoot(skel: &Skeleton, pose: &PoseBuffer, from: Vec3) -> Option<BoneHit> {
        bone_trace(skel, pose, from, from + Vec3::X * 100.0, &RIFLE, 1.0)
    }

    #[test]
    fn priority_beats_distance_only_in_bone_order() {
        // an arm (6) in front of the head (9): the head scores when it is
        // listed first, the arm when it is, which clips the head's box
        let arm = bone("arm", Vec3::new(20.0, 0.0, 0.0), Vec3::splat(2.0), 6);
        let head = bone("head", Vec3::new(50.0, 0.0, 0.0), Vec3::splat(2.0), 2);
        let (skel, pose) = rig(vec![head.clone(), arm.clone()]);
        let hit = shoot(&skel, &pose, Vec3::ZERO).unwrap();
        assert_eq!(hit.hit_location, 2);
        assert!((hit.fraction - 0.48).abs() < 1e-3, "{}", hit.fraction);
        assert_eq!(hit.normal, -Vec3::X);
        let (skel, pose) = rig(vec![arm, head]);
        let hit = shoot(&skel, &pose, Vec3::ZERO).unwrap();
        assert_eq!(hit.hit_location, 6);
        assert!((hit.fraction - 0.18).abs() < 1e-3, "{}", hit.fraction);
    }

    /// The stored build ranks bones as the kept one does; its fractions sit
    /// within float steps of the kept ones.
    #[test]
    fn the_stored_build_ranks_alike() {
        let arm = bone("arm", Vec3::new(20.0, 0.0, 0.0), Vec3::splat(2.0), 6);
        let head = bone("head", Vec3::new(50.0, 0.0, 0.0), Vec3::splat(2.0), 2);
        for listed in [vec![head.clone(), arm.clone()], vec![arm, head]] {
            let (skel, pose) = rig(listed);
            let to = Vec3::X * 100.0;
            let kept = bone_trace_in(&skel, &pose, Vec3::ZERO, to, &RIFLE, 1.0, Arith::Kept);
            let stored = bone_trace_in(&skel, &pose, Vec3::ZERO, to, &RIFLE, 1.0, Arith::Stored);
            let (kept, stored) = (kept.unwrap(), stored.unwrap());
            assert_eq!(stored.hit_location, kept.hit_location);
            assert_eq!(stored.normal, kept.normal);
            assert!((stored.fraction - kept.fraction).abs() < 1e-6);
        }
    }

    /// UO's matrix off a quarter turn about z: its axes to the float step.
    #[test]
    fn stored_axes_follow_the_quaternion() {
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let [x, y, z] = axes_stored(q.to_array());
        assert!((x - Vec3::Y).abs().max_element() < 1e-6, "{x}");
        assert!((y + Vec3::X).abs().max_element() < 1e-6, "{y}");
        assert_eq!(z, Vec3::Z);
        assert_eq!(
            axes_stored(Quat::IDENTITY.to_array()),
            [Vec3::X, Vec3::Y, Vec3::Z]
        );
    }

    #[test]
    fn a_box_past_the_limit_never_scores() {
        let (skel, pose) = rig(vec![bone(
            "head",
            Vec3::new(50.0, 0.0, 0.0),
            Vec3::splat(2.0),
            2,
        )]);
        let from = Vec3::ZERO;
        assert!(bone_trace(&skel, &pose, from, from + Vec3::X * 100.0, &RIFLE, 0.4).is_none());
    }

    #[test]
    fn equal_priority_goes_to_the_nearer_hit() {
        // two upper legs, same priority; the near one wins whichever order
        // the bones are listed in
        for order in [[0usize, 1], [1, 0]] {
            let far = bone("far", Vec3::new(60.0, 0.0, 0.0), Vec3::splat(2.0), 12);
            let near = bone("near", Vec3::new(20.0, 0.0, 0.0), Vec3::splat(2.0), 13);
            let mut listed = vec![far, near];
            if order == [1, 0] {
                listed.swap(0, 1);
            }
            let (skel, pose) = rig(listed);
            let hit = shoot(&skel, &pose, Vec3::ZERO).unwrap();
            assert_eq!(hit.hit_location, 13, "order {order:?}");
        }
    }

    #[test]
    fn a_zero_size_box_is_skipped() {
        let (skel, pose) = rig(vec![
            bone("off", Vec3::new(20.0, 0.0, 0.0), Vec3::ZERO, 2),
            bone("torso", Vec3::new(50.0, 0.0, 0.0), Vec3::splat(2.0), 4),
        ]);
        assert_eq!(shoot(&skel, &pose, Vec3::ZERO).unwrap().hit_location, 4);
    }

    #[test]
    fn gun_and_none_bones_are_never_hit() {
        let (skel, pose) = rig(vec![
            bone(
                "tag_weapon_right",
                Vec3::new(20.0, 0.0, 0.0),
                Vec3::splat(2.0),
                18,
            ),
            bone("tag_origin", Vec3::new(30.0, 0.0, 0.0), Vec3::splat(2.0), 0),
        ]);
        assert!(shoot(&skel, &pose, Vec3::ZERO).is_none());
    }

    /// A segment that starts inside a box and leaves it scores nothing on
    /// that bone: the walk goes on to the head behind it, on both builds.
    #[test]
    fn a_segment_leaving_a_box_it_starts_in_goes_on() {
        let (skel, pose) = rig(vec![
            bone("torso", Vec3::new(0.0, 0.0, 0.0), Vec3::splat(5.0), 4),
            bone("head", Vec3::new(50.0, 0.0, 0.0), Vec3::splat(2.0), 2),
        ]);
        for arith in [Arith::Kept, Arith::Stored] {
            let to = Vec3::X * 100.0;
            let hit = bone_trace_in(&skel, &pose, Vec3::ZERO, to, &RIFLE, 1.0, arith).unwrap();
            assert!(!hit.start_solid);
            assert_eq!(hit.hit_location, 2);
            assert!((hit.fraction - 0.48).abs() < 1e-3, "{}", hit.fraction);
        }
    }

    /// A segment inside a box end to end is all-solid at fraction 0, and the
    /// walk stops there.
    #[test]
    fn a_segment_inside_a_box_is_all_solid() {
        let (skel, pose) = rig(vec![
            bone("torso", Vec3::new(0.0, 0.0, 0.0), Vec3::splat(5.0), 4),
            bone("head", Vec3::new(3.0, 0.0, 0.0), Vec3::splat(2.0), 2),
        ]);
        for arith in [Arith::Kept, Arith::Stored] {
            let to = Vec3::X * 2.0;
            let hit = bone_trace_in(&skel, &pose, Vec3::ZERO, to, &RIFLE, 1.0, arith).unwrap();
            assert!(hit.start_solid);
            assert_eq!((hit.fraction, hit.hit_location), (0.0, 4));
        }
    }

    /// The link box is only a broad phase: a ray can cross the column a
    /// player occupies and hit no bone at all.
    #[test]
    fn a_ray_through_the_column_that_meets_no_box_misses() {
        let (skel, pose) = rig(vec![bone(
            "torso",
            Vec3::new(50.0, 0.0, 0.0),
            Vec3::splat(2.0),
            4,
        )]);
        assert!(shoot(&skel, &pose, Vec3::new(0.0, 0.0, 10.0)).is_none());
    }

    /// The box is in the bone's own frame, so posing the bone moves it.
    #[test]
    fn the_box_follows_the_posed_bone() {
        let head = Bone {
            parent: 0,
            local_pos: Vec3::new(50.0, 0.0, 0.0),
            ..bone("head", Vec3::new(50.0, 0.0, 0.0), Vec3::splat(2.0), 2)
        };
        let (skel, mut pose) = rig(vec![bone("root", Vec3::ZERO, Vec3::ZERO, 0), head]);
        assert_eq!(shoot(&skel, &pose, Vec3::ZERO).unwrap().hit_location, 2);

        // a quarter turn on the root swings the head box onto +Y
        pose.set_local_rot(0, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        assert!(shoot(&skel, &pose, Vec3::ZERO).is_none());
        let hit = bone_trace(&skel, &pose, Vec3::ZERO, Vec3::Y * 100.0, &RIFLE, 1.0).unwrap();
        assert_eq!(hit.hit_location, 2);
        // the face met is the box's -X side, turned onto -Y with the bone
        assert!((hit.normal + Vec3::Y).length() < 1e-5, "{}", hit.normal);
    }
}
