use super::*;

const CFG: &str = "
ragdoll_clear 1
ragdoll_bone 1 j_mainroot j_neck   5.0 0.5 20.0 0.7 -1 0 capsule // torso
ragdoll_bone 1 j_neck     j_head   3.5 0.5  5.0 0.6  0 0 capsule
ragdoll_bone 1 j_shoulder_le j_elbow_le 2.5 0.5 8.0 0.5 0 0 capsule
ragdoll_bone 1 j_shoulder_ri j_elbow_ri 2.5 0.5 8.0 0.5 0 1 capsule
ragdoll_bone 1 j_a j_b 1 1 1 1 9 0 capsule
ragdoll_selfpair 1 2 3
ragdoll_joint 1 1 swivel
ragdoll_joint 1 2 hinge
ragdoll_joint 1 0 hinge
ragdoll_joint 1 3 none
ragdoll_limit 1 0 x 60.0 -50.0 50.0
ragdoll_limit 1 0 -z 60.0 -120.0 200.0
ragdoll_limit 1 1 z 60.0 -120.0 2.0
ragdoll_baselerp_bone 1 j_head 2000
ragdoll_baselerp_bone 1 j_hiptwist_le 50000 3
";

#[test]
fn the_cfg_builds_its_def_and_refuses_bad_lines() {
    let defs = parse_cfg(CFG);
    assert!(defs[0].bones.is_empty());
    let d = &defs[1];
    // `j_a`'s parent 9 does not exist yet: refused.
    assert_eq!(d.bones.len(), 4);
    assert_eq!(d.bones[0].parent, None);
    assert_eq!(d.bones[3].parent, Some(0));
    assert!(d.bones[3].mirror);
    assert_eq!(d.self_pairs, [(2, 3)]);
    // A joint on the root bone has no parent: refused.
    let kinds: Vec<JointKind> = d.joints.iter().map(|j| j.kind).collect();
    assert_eq!(
        kinds,
        [JointKind::Swivel, JointKind::Hinge, JointKind::None]
    );
    // The limits in the cfg's order, `-` negating the axis, the stops in
    // radians clamped to pi.
    let l = &d.joints[0].limits;
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].axis, [1.0, 0.0, 0.0]);
    assert_eq!(l[1].axis, [-0.0, -0.0, -1.0]);
    assert_eq!(l[1].hi, std::f32::consts::PI);
    assert_eq!(l[0].lo, (-50.0f64 * 0.017_453_292_384_743_69) as f32);
    assert_eq!(l[0].friction, 60.0);
    // The msec clamped to 6000, the ragdoll bone 0 when left out.
    let b: Vec<(i32, usize)> = d
        .base_lerp
        .iter()
        .map(|b| (b.msec, b.ragdoll_bone))
        .collect();
    assert_eq!(b, [(2000, 0), (6000, 3)]);
}

/// 1.7's caps: 14 bones, 28 joints, 3 limit axes a joint, 33 self pairs
/// and 9 base lerp bones, each line past one dropped.
#[test]
fn a_def_past_its_caps_stops_growing() {
    let mut cfg = String::from("ragdoll_bone 1 r r 1 1 1 1 -1 0 capsule\n");
    for i in 0..100 {
        let p0 = if i % 2 == 0 { "a" } else { "b" };
        cfg += &format!("ragdoll_bone 1 {p0} c 1 1 1 1 0 {} capsule\n", i % 2);
        cfg += "ragdoll_selfpair 1 0 1\nragdoll_joint 1 1 hinge\n";
        cfg += "ragdoll_limit 1 0 x 1 -1 1\n";
        cfg += "ragdoll_baselerp_bone 1 j_head 1000\n";
    }
    let d = &parse_cfg(&cfg)[1];
    let counts = (
        d.bones.len(),
        d.joints.len(),
        d.joints[0].limits.len(),
        d.self_pairs.len(),
        d.base_lerp.len(),
    );
    assert_eq!(counts, (14, 28, 3, 33, 9));
}

/// The stock `ragdoll.cfg`: MP's def 1, and def 0 beside it.
#[test]
fn the_stock_cfg_builds_the_mp_def() {
    let Some(install) = crate::testing::cod4_install() else {
        return;
    };
    let fs = vcod_common::pk3::Pk3Fs::open_iwd_layers(&install.root().join("main"), &[]).unwrap();
    let text = fs.read("ragdoll.cfg").expect("iw_00.iwd's ragdoll.cfg");
    let defs = parse_cfg(&String::from_utf8_lossy(&text));
    let d = &defs[MP_DEF];
    let counts = (
        d.bones.len(),
        d.joints.len(),
        d.self_pairs.len(),
        d.base_lerp.len(),
    );
    assert_eq!(counts, (12, 11, 33, 9));
    assert_eq!(
        (d.bones[0].p0.as_str(), d.bones[0].mass),
        ("j_mainroot", 20.0)
    );
    assert_eq!(d.bones[11].parent, Some(10));
    let knee = d.joints.iter().find(|j| j.bone == 9).unwrap();
    assert_eq!(knee.kind, JointKind::Hinge);
    let l = knee.limits[0];
    assert_eq!(l.axis, [0.0, 0.0, 1.0]);
    assert_eq!(l.friction, 130.0);
    assert!((l.lo.to_degrees() + 120.0).abs() < 1e-4 && (l.hi.to_degrees() - 3.5).abs() < 1e-4);
    let neck = d.joints.iter().find(|j| j.bone == 3).unwrap();
    assert_eq!(neck.limits.len(), 3);
    let twist: Vec<usize> = d.base_lerp.iter().map(|b| b.ragdoll_bone).collect();
    assert_eq!(twist, [0, 0, 0, 0, 0, 4, 6, 8, 10]);
    assert_eq!(defs[0].bones.len(), 10);
}

mod pool {
    use std::sync::Arc;

    use glam::{Quat, Vec3};
    use vcod_common::model::{Bone, Model};
    use vcod_common::skeleton::Skeleton;

    use crate::collision::{BrushFaces, Piece};
    use crate::ode::collide::Pieces;
    use crate::ode::collide::tests::cuboid;
    use crate::ragdoll::{Clip, DObj, Dvars, Explosion, Ragdolls, State, Sweep, parse_cfg};

    const FIGURE: &str = "
ragdoll_bone 1 j_mainroot    j_neck     5.0 0.5 20.0 0.7 -1 0 capsule
ragdoll_bone 1 j_neck        j_head     3.5 0.5  5.0 0.6  0 0 capsule
ragdoll_bone 1 j_shoulder_le j_elbow_le 2.5 0.5  8.0 0.5  0 0 capsule
ragdoll_bone 1 j_shoulder_ri j_elbow_ri 2.5 0.5  8.0 0.5  0 1 capsule
ragdoll_selfpair 1 2 3
ragdoll_joint 1 1 swivel
ragdoll_joint 1 2 swivel
ragdoll_joint 1 3 swivel
ragdoll_limit 1 0 x 80 -30 30
ragdoll_limit 1 0 y 80 -30 30
ragdoll_limit 1 0 z 80 -30 30
ragdoll_limit 1 1 x 95 -90 90
ragdoll_limit 1 1 y 95 -90 35
ragdoll_limit 1 1 z 95 -110 16
ragdoll_limit 1 2 x 95 -90 90
ragdoll_limit 1 2 y 95 -35 90
ragdoll_limit 1 2 z 95 -110 16
ragdoll_baselerp_bone 1 j_head 1000
";

    /// A bone at `pos` (the model's frame) whose x points along `x`.
    fn bone(name: &str, parent: i32, pos: Vec3, x: Vec3) -> Bone {
        Bone {
            name: name.into(),
            parent,
            pos,
            rot: Quat::from_rotation_arc(Vec3::X, x),
            local_pos: Vec3::ZERO,
            local_rot: Quat::IDENTITY,
            hit_mins: Vec3::ZERO,
            hit_maxs: Vec3::ZERO,
            hit_location: 0,
        }
    }

    /// A standing figure: torso up from the root at 40, the neck and
    /// head, two arms out level; the right arm's bones point back at
    /// their parent, as a mirrored side's do. The skeleton and its base
    /// pose.
    fn figure() -> (Skeleton, Vec<(Vec3, Quat)>) {
        let p = Vec3::new;
        let mut bones = vec![
            bone("tag_origin", -1, Vec3::ZERO, Vec3::X),
            bone("j_mainroot", 0, p(0.0, 0.0, 40.0), Vec3::Z),
            bone("j_neck", 1, p(0.0, 0.0, 60.0), Vec3::Z),
            bone("j_head", 2, p(0.0, 0.0, 68.0), Vec3::Z),
            bone("j_shoulder_le", 2, p(0.0, 8.0, 58.0), Vec3::Y),
            bone("j_elbow_le", 4, p(0.0, 20.0, 58.0), Vec3::Y),
            bone("j_shoulder_ri", 2, p(0.0, -8.0, 58.0), Vec3::Y),
            bone("j_elbow_ri", 6, p(0.0, -20.0, 58.0), Vec3::Y),
        ];
        for i in 1..bones.len() {
            let (pp, pr) = {
                let q = &bones[bones[i].parent as usize];
                (q.pos, q.rot)
            };
            bones[i].local_pos = pr.inverse() * (bones[i].pos - pp);
            bones[i].local_rot = pr.inverse() * bones[i].rot;
        }
        let base = bones.iter().map(|b| (b.pos, b.rot)).collect();
        let skel = Skeleton::build(&[&Model {
            name: "figure".into(),
            bones,
            lods: Vec::new(),
            collision: Vec::new(),
        }]);
        (skel, base)
    }

    /// A floor brush whose top is z 0, and a wall at y 15 a sweep alone
    /// meets when `wall`.
    struct Floor {
        brush: BrushFaces,
        wall: bool,
    }

    impl Floor {
        fn new(wall: bool) -> Floor {
            Floor {
                brush: cuboid([-512.0, -512.0, -32.0], [512.0, 512.0, 0.0]),
                wall,
            }
        }
    }

    impl Pieces for Floor {
        fn pieces(&self, _: Vec3, _: Vec3, _: Vec3, _: u32) -> Vec<Piece> {
            vec![Piece::Brush(0)]
        }
        fn brush(&self, _: usize) -> BrushFaces {
            self.brush.clone()
        }
    }

    impl Clip for Floor {
        fn sweep(&self, a: Vec3, b: Vec3, mins: Vec3, _: Vec3) -> Sweep {
            let e = -mins.z;
            let clear = Sweep {
                fraction: 1.0,
                normal: Vec3::ZERO,
                startsolid: false,
            };
            if self.wall && (a.y - 15.0) * (b.y - 15.0) < 0.0 {
                return Sweep {
                    fraction: 0.5,
                    normal: Vec3::NEG_Y,
                    ..clear
                };
            }
            if a.z - e < 0.0 {
                return Sweep {
                    fraction: 0.0,
                    normal: Vec3::Z,
                    startsolid: true,
                };
            }
            if b.z - e >= 0.0 {
                return clear;
            }
            Sweep {
                fraction: (a.z - e) / (a.z - b.z),
                normal: Vec3::Z,
                startsolid: false,
            }
        }
        fn bounds(&self) -> (Vec3, Vec3) {
            (Vec3::splat(-4096.0), Vec3::splat(4096.0))
        }
    }

    fn pool(dvars: Dvars) -> Ragdolls {
        let mut defs = parse_cfg(FIGURE);
        Ragdolls::new(Arc::new(std::mem::take(&mut defs[1])), dvars)
    }

    /// The figure's base pose moved by `up`, as a DObj shows it.
    fn worlds(base: &[(Vec3, Quat)], up: f32) -> Vec<(Vec3, Quat)> {
        base.iter().map(|&(p, q)| (p + Vec3::Z * up, q)).collect()
    }

    fn feed(
        r: &mut Ragdolls,
        id: crate::ragdoll::SlotId,
        skel: &Skeleton,
        base: &[(Vec3, Quat)],
        world: &[(Vec3, Quat)],
    ) {
        r.feed(
            id,
            &DObj {
                skel,
                base,
                origin: Vec3::ZERO,
                turn: Quat::IDENTITY,
                world,
            },
        );
    }

    /// A slot no DObj answers gives up on its fifth frame (0x5beb90); one
    /// fed takes a snapshot on its first frame, builds off the second on
    /// its next and simulates, one step run: each capsule along its bone,
    /// moving as the two snapshots 17 ms apart moved (1 unit up: 1000/17
    /// u/s, less a step's fall).
    #[test]
    fn a_slot_waits_takes_two_snapshots_and_builds() {
        let (skel, base) = figure();
        let floor = Floor::new(false);
        let mut r = pool(Dvars::default());
        let lost = r.create().unwrap();
        let mut states = Vec::new();
        for _ in 0..5 {
            r.update(17, &floor);
            states.push(r.state(lost).unwrap());
        }
        assert_eq!(states[3], State::WaitDObj);
        assert_eq!(states[4], State::Free);
        let id = r.create().unwrap();
        feed(&mut r, id, &skel, &base, &worlds(&base, 20.0));
        r.update(17, &floor);
        assert_eq!(r.state(id), Some(State::Snapshots));
        feed(&mut r, id, &skel, &base, &worlds(&base, 21.0));
        r.update(17, &floor);
        assert_eq!(r.state(id), Some(State::Simulating));
        let step = Vec3::Z * ((1000.0 / 17.0 - 800.0 * 0.017) * 0.017);
        let caps = r.capsules(id);
        let (a, e, rad) = caps[1];
        assert!(a.distance(Vec3::new(0.0, 0.0, 81.0) + step) < 0.01, "{a}");
        assert!(e.distance(Vec3::new(0.0, 0.0, 89.0) + step) < 0.01, "{e}");
        assert_eq!(rad, 3.5);
        // the right arm's capsule runs out along the bone, its turn
        // flipped
        let (a, e, _) = caps[3];
        assert!(
            a.distance(Vec3::new(0.0, -8.0, 79.0) + step) < 0.01 && e.y < -19.0,
            "{a} {e}"
        );
    }

    /// The figure let go 20 units over the floor falls, comes to rest on
    /// it before `ragdoll_max_life` and goes idle, its bodies let go and
    /// its pose held: the root's capsule lies on the floor, sunk into it
    /// no more than the contacts' surface layer.
    #[test]
    fn a_figure_dropped_on_the_floor_sleeps_on_it() {
        let (skel, base) = figure();
        let floor = Floor::new(false);
        let mut r = pool(Dvars::default());
        let id = r.create().unwrap();
        let mut ms = 0;
        while r.state(id) != Some(State::Idle) && ms < 6000 {
            if r.wants_pose(id) {
                feed(&mut r, id, &skel, &base, &worlds(&base, -20.0));
            }
            r.update(17, &floor);
            ms += 17;
        }
        assert_eq!(r.state(id), Some(State::Idle), "{ms} ms");
        assert!(ms < 4500, "{ms} ms: past its max life");
        assert!(r.bodies(id).iter().all(Option::is_none));
        let pose = r.pose(id).unwrap().to_vec();
        let root = skel.bone_index("j_mainroot").unwrap();
        assert!((3.0..6.0).contains(&pose[root].0.z), "{}", pose[root].0.z);
        r.update(17, &floor);
        assert_eq!(r.pose(id).unwrap(), pose.as_slice());
    }

    /// A bone whose length crosses a wall fails the tunnel test at the
    /// build, and the slot goes free.
    #[test]
    fn a_bone_across_a_wall_fails_the_tunnel_test() {
        let (skel, base) = figure();
        let wall = Floor::new(true);
        let mut r = pool(Dvars::default());
        let id = r.create().unwrap();
        for _ in 0..2 {
            feed(&mut r, id, &skel, &base, &worlds(&base, 20.0));
            r.update(17, &wall);
        }
        assert_eq!(r.state(id), Some(State::Free));
    }

    /// A held figure: a grenade's blast (inner 0, outer 256) 48 units
    /// off its root wakes it, its bodies rebuilt at the pose held, and
    /// throws it away from the blast; one past the outer radius reaches
    /// nothing.
    #[test]
    fn a_blast_wakes_a_held_figure_and_throws_it() {
        let (skel, base) = figure();
        let floor = Floor::new(false);
        let mut r = pool(Dvars::default());
        let id = r.create().unwrap();
        while r.state(id) != Some(State::Idle) {
            if r.wants_pose(id) {
                feed(&mut r, id, &skel, &base, &worlds(&base, -20.0));
            }
            r.update(17, &floor);
        }
        let root = skel.bone_index("j_mainroot").unwrap();
        let rest = r.pose(id).unwrap()[root].0;
        let mut no_jitter = || 0.0;
        let far = Explosion {
            origin: rest - Vec3::X * 300.0,
            inner: 0.0,
            outer: 256.0,
            cylinder: false,
            impulse: Vec3::ZERO,
            scale: 1.0,
        };
        assert_eq!(r.explode(&far, &mut no_jitter), 0);
        assert_eq!(r.state(id), Some(State::Idle));
        let near = Explosion {
            origin: rest - Vec3::X * 48.0,
            ..far
        };
        assert_eq!(r.explode(&near, &mut no_jitter), 1);
        assert_eq!(r.state(id), Some(State::Simulating));
        assert!(r.bodies(id).iter().all(Option::is_some));
        let mut top = rest.z;
        while r.state(id) == Some(State::Simulating) {
            r.update(17, &floor);
            top = top.max(r.pose(id).unwrap()[root].0.z);
        }
        let moved = r.pose(id).unwrap()[root].0 - rest;
        assert!(moved.x > 4.0, "the root moved {moved}");
        assert!(top > rest.z + 1.0, "the root never left the floor ({top})");
    }

    /// `ragdoll_max_simulating` counts the slot entering the simulation
    /// too (0x5beb60): at 2 the first ragdoll simulates and the second,
    /// built, is let go.
    #[test]
    fn the_simulating_cap_counts_the_slot_itself() {
        let (skel, base) = figure();
        let floor = Floor::new(false);
        let mut r = pool(Dvars {
            max_simulating: 2,
            ..Dvars::default()
        });
        let ids = [r.create().unwrap(), r.create().unwrap()];
        for _ in 0..2 {
            for id in ids {
                feed(&mut r, id, &skel, &base, &worlds(&base, 20.0));
            }
            r.update(17, &floor);
        }
        assert_eq!(r.state(ids[0]), Some(State::Simulating));
        assert_eq!(r.state(ids[1]), Some(State::Free));
    }
}
