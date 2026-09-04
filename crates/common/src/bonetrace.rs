//! The locational trace: a ray against the per-bone hit boxes of a posed
//! skeleton, which is where a hit's 0..18 location index comes from.
//! `docs/research/cod11-combat.md` section 3.

use crate::skeleton::{PoseBuffer, Skeleton};
use glam::Vec3;

/// The winning bone, in the frame the ray was given in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneHit {
    /// Along `start..end`; 0 when the ray started inside a box.
    pub fraction: f32,
    pub hit_location: u8,
    /// Skeleton bone index.
    pub bone: usize,
    /// The ray started inside this bone's box, which retail reports as
    /// start-solid and returns on at once.
    pub start_solid: bool,
}

/// The priority the trace ranks candidates by, one byte per hit location.
/// `bulletPriorityMap` and `riflePriorityMap` (combat doc, 2.3).
pub type PriorityMap = [u8; 19];

/// Where `bestPriority` starts, so `gun` (priority 0) and `none` (1) are
/// never hit under either shipped map.
const MIN_PRIORITY: u8 = 2;

/// Entry fraction of a segment through an AABB, or `None` when it misses.
/// The bool is set when the segment starts inside.
fn slab(start: Vec3, dir: Vec3, lo: Vec3, hi: Vec3) -> Option<(f32, bool)> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let mut inside = true;
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
        }
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, inside))
}

/// The bone the segment `start..end` scores on `pose`, both in the skeleton's
/// own frame (the entity's origin and angles are the caller's to undo).
///
/// The rules are retail's, in its order (combat doc, section 3): a bone with
/// a zero-size box is skipped, a bone whose priority is below the best found
/// so far is skipped before any geometry runs, a higher priority takes the
/// hit **regardless of distance**, an equal priority takes it only when it is
/// nearer, and a segment that starts inside a box returns at once.
pub fn bone_trace(
    skel: &Skeleton,
    pose: &PoseBuffer,
    start: Vec3,
    end: Vec3,
    priority: &PriorityMap,
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
        let Some((t, inside)) = slab(ls, le - ls, b.hit_mins, b.hit_maxs) else {
            continue;
        };
        if inside {
            return Some(BoneHit {
                fraction: 0.0,
                hit_location: b.hit_location,
                bone: bi,
                start_solid: true,
            });
        }
        if prio == best_priority && best.is_some_and(|h| h.fraction <= t) {
            continue;
        }
        best_priority = prio;
        best = Some(BoneHit {
            fraction: t,
            hit_location: b.hit_location,
            bone: bi,
            start_solid: false,
        });
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xmodel::{Bone, XModel};
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
        let m = XModel {
            lod: "t".into(),
            surfaces: vec![],
            materials: vec![],
            bones,
            collision: Vec::new(),
        };
        let skel = Skeleton::build(&[&m]);
        let pose = PoseBuffer::new(&skel);
        (skel, pose)
    }

    fn shoot(skel: &Skeleton, pose: &PoseBuffer, from: Vec3) -> Option<BoneHit> {
        bone_trace(skel, pose, from, from + Vec3::X * 100.0, &RIFLE)
    }

    #[test]
    fn priority_beats_distance() {
        // an arm (6) in front of the head (9): retail scores the head
        let (skel, pose) = rig(vec![
            bone("arm", Vec3::new(20.0, 0.0, 0.0), Vec3::splat(2.0), 6),
            bone("head", Vec3::new(50.0, 0.0, 0.0), Vec3::splat(2.0), 2),
        ]);
        let hit = shoot(&skel, &pose, Vec3::ZERO).unwrap();
        assert_eq!(hit.hit_location, 2);
        assert!((hit.fraction - 0.48).abs() < 1e-3, "{}", hit.fraction);
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

    #[test]
    fn a_segment_starting_inside_a_box_is_start_solid() {
        let (skel, pose) = rig(vec![
            bone("torso", Vec3::new(0.0, 0.0, 0.0), Vec3::splat(5.0), 4),
            bone("head", Vec3::new(50.0, 0.0, 0.0), Vec3::splat(2.0), 2),
        ]);
        let hit = shoot(&skel, &pose, Vec3::ZERO).unwrap();
        assert!(hit.start_solid);
        assert_eq!(hit.fraction, 0.0);
        assert_eq!(hit.hit_location, 4); // returns at once, never reaches the head
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
        let hit = bone_trace(&skel, &pose, Vec3::ZERO, Vec3::Y * 100.0, &RIFLE).unwrap();
        assert_eq!(hit.hit_location, 2);
    }
}
