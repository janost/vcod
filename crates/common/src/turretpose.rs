//! The gunner's body placement, `game.mp.i386.so` 0x515a8
//! (docs/research/cod11-turrets.md section 7): the mounted anim's blended
//! root delta, hung off the turret's `tag_weapon`, is where the player
//! stands.

use crate::animtree::PlayerAnims;
use crate::xanim::XAnim;
use glam::{EulerRot, Quat, Vec3};
use std::rc::Rc;

/// Wire angles (pitch, yaw, roll, degrees) as the rotation `AnglesToAxis`
/// builds: its forward, left and up are this rotation's X, Y and Z.
pub fn angles_quat(angles: [f32; 3]) -> Quat {
    let [p, y, r] = angles.map(f32::to_radians);
    Quat::from_euler(EulerRot::ZYX, y, p, r)
}

/// `AxisToAngles`: the inverse of [`angles_quat`], wire degrees.
pub fn quat_angles(q: Quat) -> [f32; 3] {
    let (y, p, r) = q.to_euler(EulerRot::ZYX);
    [p.to_degrees(), y.to_degrees(), r.to_degrees()]
}

/// `tag_weapon` in the gun's model space with the barrel's pitch and yaw on
/// `tag_aim`: the bind positions of the two tags and the wire `angles2`.
pub fn tag_weapon_local(aim: Vec3, weapon: Vec3, angles2: [f32; 3]) -> (Vec3, Quat) {
    let barrel = angles_quat([angles2[0], angles2[1], 0.0]);
    (aim + barrel * (weapon - aim), barrel)
}

/// What 0x515a8 (and the client's 0x300279b0) work out for a gunner.
#[derive(Clone, Debug)]
pub struct GunnerPlacement {
    /// World origin, before the trace down.
    pub origin: Vec3,
    /// The body's world rotation: the blend's root yaw plus `tag_weapon`'s,
    /// carried by the gun's axis.
    pub rot: Quat,
    /// The turret anim's rows the placement weights, in engine child order:
    /// each row's goal weight and its leaves (animtree node ids) with theirs,
    /// the rows' weights and each row's leaves' summing to 1. The tree walk
    /// normalizes each row to its weight before the group sums them
    /// (cod11-turrets.md 13.9, "The rows").
    pub rows: Vec<TurretRow>,
    /// The rows the search's first pass weighted and walked past, before
    /// [`Self::rows`] in engine child order, with that pass's goals: 1 and
    /// the row's column pair.
    pub passed: Vec<TurretRow>,
    /// The row nodes of [`Self::passed`] then [`Self::rows`], in that order:
    /// every row the first pass walked.
    pub walked: Vec<usize>,
    /// Every row of the group with its column leaves, engine child order.
    pub layout: Vec<(usize, Vec<usize>)>,
    /// The turret anim's group node the leaves sit under.
    pub node: usize,
    /// The group's node time step a second at rate 1 those weights give:
    /// the sync node's weighted mean of its rows' and their leaves'
    /// frequencies (1.5b 0x80c639e, UO 0x80bb732; cod11-turrets.md 13.9).
    pub frequency: f32,
    /// The group is `loopsync`: its node time wraps.
    pub looping: bool,
}

/// One row of a turret anim's group: its weight and its leaves' weights.
pub type TurretRow = (f32, Vec<(usize, f32)>);

impl GunnerPlacement {
    /// Every leaf with its row's weight times its own.
    pub fn leaves(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        self.rows
            .iter()
            .flat_map(|(rw, w)| w.iter().map(move |&(l, x)| (l, x * rw)))
    }

    /// The body's world yaw in degrees, `AxisToAngles`' yaw of [`Self::rot`].
    pub fn yaw(&self) -> f32 {
        let f = self.rot * Vec3::X;
        f.y.atan2(f.x).to_degrees()
    }
}

/// One node's goal-weight record in the server's anim info (UO
/// `coduo_lnxded` node `+0x34..+0x3c`, half 1): its weight, the goal it
/// heads for and the seconds left to reach it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct GoalWeight {
    weight: f32,
    goal: f32,
    time: f32,
}

/// 0.001: the engine's goal floor, snap window and raise factor (UO
/// 0x80ea578, 0x80ea444, 0x80ea1d4).
const MIN_WEIGHT: f32 = 0.001;

impl GoalWeight {
    fn live(&self) -> bool {
        self.weight != 0.0 || self.goal != 0.0
    }
}

/// `XAnimSetGoalWeight` on one node, `None` when it holds no record (UO
/// 0x80c1072 into 0x80c05a2, 1.5b 0x80bd674): a goal under 0.001 reads 0,
/// the time left is `|goal - weight| * time`, and under 0.001 the weight
/// takes the goal at once, else a weight of 0 is raised to `0.001 * goal`.
fn set_goal(node: &mut Option<GoalWeight>, goal: f32, time: f32) {
    let goal = if goal < MIN_WEIGHT { 0.0 } else { goal };
    if node.is_none() && goal == 0.0 {
        return;
    }
    let n = node.get_or_insert_with(GoalWeight::default);
    n.goal = goal;
    let left = (f64::from((goal - n.weight).abs()) * f64::from(time)) as f32;
    if left < MIN_WEIGHT {
        n.time = 0.0;
        n.weight = goal;
    } else {
        n.time = left;
        if n.weight == 0.0 {
            n.weight = (f64::from(MIN_WEIGHT) * f64::from(goal)) as f32;
        }
    }
    if !n.live() {
        *node = None;
    }
}

/// The weight step `G_XAnimUpdate`'s `trap_DObjInitServerTime` runs on
/// every node each frame (UO 0x80bcc1e, 1.5b 0x80bb130): within the step
/// plus 0.001 of its time the weight takes the goal, else it moves the
/// step's share of the way there.
fn step_goal(n: &mut GoalWeight, dt: f32) {
    if f64::from(n.time) < f64::from(MIN_WEIGHT) + f64::from(dt) {
        n.weight = n.goal;
        n.time = 0.0;
        return;
    }
    let (w, g) = (f64::from(n.weight), f64::from(n.goal));
    n.weight = (w + (g - w) * f64::from(dt) / f64::from(n.time)) as f32;
    if n.weight < 1e-6 {
        n.weight = (f64::from(MIN_WEIGHT) * g) as f32;
    }
    n.time = (f64::from(n.time) - f64::from(dt)) as f32;
}

/// One turret group's rows and leaves as the server's anim info holds them.
#[derive(Clone, Debug, Default)]
struct GroupWeights {
    node: usize,
    layout: Vec<(usize, Vec<usize>)>,
    nodes: std::collections::HashMap<usize, GoalWeight>,
}

impl GroupWeights {
    fn get(&self, n: usize) -> Option<GoalWeight> {
        self.nodes.get(&n).copied()
    }

    fn weight(&self, n: usize) -> f32 {
        self.get(n).map_or(0.0, |v| v.weight)
    }

    fn put(&mut self, n: usize, v: Option<GoalWeight>) {
        match v.filter(GoalWeight::live) {
            Some(v) => self.nodes.insert(n, v),
            None => self.nodes.remove(&n),
        };
    }

    fn set_goal(&mut self, n: usize, goal: f32, time: f32) {
        let mut v = self.get(n);
        set_goal(&mut v, goal, time);
        self.put(n, v);
    }

    /// `trap_XAnimClearTreeGoalWeightsStrict` under the group at time 0
    /// (UO 0x80bfdca over 0x80bfc22, 0x80bfbdc): a weighted node under
    /// weighted parents with a weighted leaf at or below it heads for 0
    /// now, its weight kept; any other is zeroed. The group itself is the
    /// legs anim, weighted.
    fn clear_strict(&mut self) {
        let cleared = |v: GoalWeight| GoalWeight {
            goal: 0.0,
            time: 0.0,
            ..v
        };
        for (row, cols) in self.layout.clone() {
            let row_active = self.weight(row) != 0.0 && cols.iter().any(|&c| self.weight(c) != 0.0);
            self.put(row, self.get(row).filter(|_| row_active).map(cleared));
            for c in cols {
                let keep = row_active && self.weight(c) != 0.0;
                self.put(c, self.get(c).filter(|_| keep).map(cleared));
            }
        }
    }

    /// The weight step on every row, then on its leaves; under a row left
    /// with neither weight nor goal the leaves take their goals instead
    /// (0x80bca32). A node left with neither is freed.
    fn step(&mut self, dt: f32) {
        for (row, cols) in self.layout.clone() {
            let mut r = self.get(row);
            if let Some(r) = r.as_mut() {
                step_goal(r, dt);
            }
            let row_dead = r.is_none_or(|r| !r.live());
            for c in cols {
                let Some(mut v) = self.get(c) else { continue };
                if row_dead {
                    v.weight = v.goal;
                    v.time = 0.0;
                } else {
                    step_goal(&mut v, dt);
                }
                self.put(c, Some(v));
            }
            self.put(row, r);
        }
    }

    /// The rows as the tree walk meets them: each weighted row with its
    /// weighted leaves, engine child order.
    fn rows(&self) -> Vec<TurretRow> {
        self.layout
            .iter()
            .map(|(r, cols)| {
                let leaves: Vec<(usize, f32)> = cols
                    .iter()
                    .map(|&c| (c, self.weight(c)))
                    .filter(|&(_, w)| w != 0.0)
                    .collect();
                (self.weight(*r), leaves)
            })
            .filter(|(w, l)| *w != 0.0 && !l.is_empty())
            .collect()
    }
}

/// A 1.5b or UO gunner's turret rows and leaves carried across frames as
/// the server's anim info carries them: [`Self::place`] is the placement's
/// two passes (game.mp.uo 0x6ac06), [`Self::step`] `G_XAnimUpdate`'s weight
/// step, and [`Self::rows`] what a pose reads in between. A row or leaf the
/// placement drops keeps its weight until the next step, and one it adds
/// stands at 0.001 times its first pass's goal until then
/// (cod11-turrets.md 13.9, "The weights across frames").
#[derive(Clone, Debug, Default)]
pub struct GunnerTree {
    groups: Vec<GroupWeights>,
}

impl GunnerTree {
    /// The placement on `g`'s group, `frametime_ms` the level's frame: the
    /// strict clear, the first pass's goals at time 1 on every walked row
    /// (1 for the row, its column pair for the leaves), the strict clear
    /// again, then the kept rows' goals over one frame.
    pub fn place(&mut self, g: &GunnerPlacement, frametime_ms: i32) {
        let i = match self.groups.iter().position(|x| x.node == g.node) {
            Some(i) => i,
            None => {
                self.groups.push(GroupWeights {
                    node: g.node,
                    ..Default::default()
                });
                self.groups.len() - 1
            }
        };
        let group = &mut self.groups[i];
        group.layout.clone_from(&g.layout);
        group.clear_strict();
        let first = g.passed.iter().chain(&g.rows).map(|(_, l)| l);
        for (&row, leaves) in g.walked.iter().zip(first) {
            group.set_goal(row, 1.0, 1.0);
            for &(leaf, x) in leaves {
                group.set_goal(leaf, x, 1.0);
            }
        }
        group.clear_strict();
        // 0x6b0e6..0x6b70c: each goal over `1 / (1000 / frametime *
        // |weight - goal|)`, 0 when that rate is 0.
        let over = |group: &GroupWeights, n: usize, goal: f32| -> f32 {
            let d = f64::from((group.weight(n) - goal).abs());
            let rate = (f64::from(1000.0 / frametime_ms as f32) * d) as f32;
            if rate > 0.0 {
                (1.0 / f64::from(rate)) as f32
            } else {
                0.0
            }
        };
        let kept = &g.walked[g.passed.len()..];
        for (&row, (rw, leaves)) in kept.iter().zip(&g.rows) {
            let t = over(group, row, *rw);
            group.set_goal(row, *rw, t);
            for &(leaf, x) in leaves {
                let t = over(group, leaf, x);
                group.set_goal(leaf, x, t);
            }
        }
    }

    /// `G_XAnimUpdate`'s weight step over every group, `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        for g in &mut self.groups {
            g.step(dt);
        }
        self.groups.retain(|g| !g.nodes.is_empty());
    }

    /// The weighted rows under `node`, as a pose reads them now.
    pub fn rows(&self, node: usize) -> Vec<TurretRow> {
        self.groups
            .iter()
            .find(|g| g.node == node)
            .map(GroupWeights::rows)
            .unwrap_or_default()
    }
}

/// Where 0x515a8 puts a gunner before its trace down, and the leaf blend
/// that puts it there. `None` when the legs anim is not a `turretanim` one
/// or a clip under it will not load, which skips the placement as retail's
/// flag test does.
///
/// `tag_weapon` is the tag in the turret's model space with the barrel's
/// `angles2` already on `tag_aim`; `turret` is the gun's world origin and
/// rotation; `player_origin` the gunner's current origin, whose height above
/// the gun picks the pitch row.
pub fn place_gunner(
    anims: &PlayerAnims,
    clip: impl FnMut(&str) -> Option<Rc<XAnim>>,
    legs_anim: i32,
    tag_weapon: (Vec3, Quat),
    turret: (Vec3, Quat),
    player_origin: Vec3,
    rotate_inc: f32,
) -> Option<GunnerPlacement> {
    place_gunner_by(
        GunnerRule::Cod1,
        anims,
        clip,
        legs_anim,
        tag_weapon,
        turret,
        player_origin,
        rotate_inc,
    )
}

/// Which way a game walks the turret group's rows and columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GunnerRule {
    /// CoD1 and CoD2 (0x515a8, lnxded 0x810aa50): the rows from the
    /// file's last, the column at `n / 2 - yaw / inc` over the engine's
    /// (reversed) order.
    Cod1,
    /// CoD4 1.7 (`G_PlayerTurretPositionAndBlend`): the rows from the
    /// file's first, the column at `n / 2 + yaw / inc` over the engine's
    /// order (cod4-gsc.md 9.2).
    Iw3,
}

/// [`place_gunner`] under `rule`.
#[allow(clippy::too_many_arguments)]
pub fn place_gunner_by(
    rule: GunnerRule,
    anims: &PlayerAnims,
    mut clip: impl FnMut(&str) -> Option<Rc<XAnim>>,
    legs_anim: i32,
    tag_weapon: (Vec3, Quat),
    turret: (Vec3, Quat),
    player_origin: Vec3,
    rotate_inc: f32,
) -> Option<GunnerPlacement> {
    let tree = &anims.tree;
    let node = anims.index.node_id(legs_anim)?;
    if !anims.script.is_turret_anim(&tree.nodes[node].name) {
        return None;
    }
    // The engine's child order is the reverse of the file's (research doc
    // player-model-anim-system.md, "Animation indices: the animtree").
    let children = |n: usize| tree.nodes[n].children.iter().rev().copied();
    let forward = tag_weapon.1 * Vec3::X;
    let gun_yaw = if forward.x == 0.0 && forward.y == 0.0 {
        0.0
    } else {
        forward.y.atan2(forward.x).to_degrees()
    };
    let up = turret.1 * Vec3::Z;
    let height = (player_origin - turret.0).dot(up);
    let target = height - tag_weapon.0.z;

    // One row's yaw blend: the two columns either side of the gun's yaw.
    let row_weights = |row: usize| -> Option<Vec<(usize, f32)>> {
        let cols: Vec<usize> = children(row).collect();
        let last = cols.len().checked_sub(1)?;
        let turn = match rule {
            GunnerRule::Cod1 => -gun_yaw,
            GunnerRule::Iw3 => gun_yaw,
        };
        let x = (cols.len() as f32 * 0.5 + turn / rotate_inc).clamp(0.0, last as f32);
        let i = x as usize;
        let f = x - i as f32;
        let mut w = vec![(cols[i], 1.0 - f)];
        if f != 0.0 {
            w.push((cols[i + 1], f));
        }
        Some(w)
    };
    let mut delta = |weights: &[(usize, f32)]| -> Option<(Vec3, f32)> {
        let (mut trans, mut rot) = (Vec3::ZERO, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0));
        for &(leaf, w) in weights {
            let anim = clip(&tree.nodes[leaf].name)?;
            let (t, r) = anim.root.as_ref().map_or((None, None), |r| r.sample(0.0));
            trans += w * t.unwrap_or(Vec3::ZERO);
            rot += r.unwrap_or(Quat::IDENTITY) * w;
        }
        Some((trans, 2.0 * rot.z.atan2(rot.w).to_degrees()))
    };

    // The rows are searched for the first whose delta sits at or above the
    // gun's height and blended with the one before it (turrets doc 7.1).
    // Each weighted row: its weight and its leaves' (engine child order).
    type Row = (f32, Vec<(usize, f32)>);
    let mut below: Option<(Vec<(usize, f32)>, f32)> = None;
    let mut rows: Option<Vec<Row>> = None;
    // The rows the search set and walked past, each at its pass's goals.
    let mut passed: Vec<Row> = Vec::new();
    let mut walked = Vec::new();
    let row_order: Vec<usize> = match rule {
        GunnerRule::Cod1 => children(node).collect(),
        GunnerRule::Iw3 => tree.nodes[node].children.clone(),
    };
    for &row in &row_order {
        let w = row_weights(row)?;
        walked.push(row);
        let z = delta(&w)?.0.z;
        if z >= target {
            rows = Some(match below.take() {
                None => vec![(1.0, w)],
                Some((bw, bz)) => {
                    let t = (target - bz) / (z - bz);
                    vec![(1.0 - t, bw), (t, w)]
                }
            });
            break;
        }
        if let Some((bw, _)) = below.replace((w, z)) {
            passed.push((1.0, bw));
        }
    }
    let layout = children(node).map(|r| (r, children(r).collect())).collect();
    let rows = rows.or(below.map(|(w, _)| vec![(1.0, w)]))?;
    // The flattened blend, the row above first.
    let weights: Vec<(usize, f32)> = rows
        .iter()
        .rev()
        .flat_map(|(rw, w)| w.iter().map(move |&(l, x)| (l, x * rw)))
        .collect();
    let (trans, rot_yaw) = delta(&weights)?;
    let mut leaf_frequency = |leaf: usize| -> Option<f32> {
        let anim = clip(&tree.nodes[leaf].name)?;
        Some(match anim.frame_count.saturating_sub(1) {
            0 => 0.0,
            n => anim.framerate / n as f32,
        })
    };
    let mut row_means = Vec::new();
    for (rw, w) in &rows {
        let mut leaves = Vec::new();
        for &(leaf, x) in w {
            leaves.push((x, leaf_frequency(leaf)?));
        }
        row_means.push((*rw, sync_mean(&leaves)));
    }
    let frequency = sync_mean(&row_means);

    let local = Quat::from_rotation_z(gun_yaw.to_radians()) * trans + tag_weapon.0;
    let local = Vec3::new(local.x, local.y, height);
    Some(GunnerPlacement {
        origin: turret.0 + turret.1 * local,
        rot: turret.1 * Quat::from_rotation_z((rot_yaw + gun_yaw).to_radians()),
        rows,
        passed,
        walked,
        layout,
        node,
        frequency,
        looping: tree.nodes[node].loopsync,
    })
}

/// 1.5b 0x80c639e (UO 0x80bb732) over one node's children, each `(weight,
/// frequency)` in child order at rate 1: the weights and the weighted
/// frequencies summed as stored floats, a child with no weight or no
/// frequency skipped, and their quotient stored; 0 with no child counted.
pub fn sync_mean(children: &[(f32, f32)]) -> f32 {
    let (mut num, mut den) = (0.0f32, 0.0f32);
    for &(w, f) in children {
        if w == 0.0 || f == 0.0 {
            continue;
        }
        den = (f64::from(den) + f64::from(w)) as f32;
        num = (f64::from(num) + f64::from(f) * f64::from(w)) as f32;
    }
    if den == 0.0 {
        0.0
    } else {
        (f64::from(num) / f64::from(den)) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anim info between two placements: what one adds stands at 0.001
    /// times its first goal, what it drops keeps its weight, and the step
    /// lands every node on its goal (cod11-turrets.md 13.9).
    #[test]
    fn the_gunner_tree_carries_the_weights_one_frame() {
        let placed =
            |rows: Vec<TurretRow>, passed: Vec<TurretRow>, walked: Vec<usize>| GunnerPlacement {
                origin: Vec3::ZERO,
                rot: Quat::IDENTITY,
                rows,
                passed,
                walked,
                layout: vec![(1, vec![11, 12, 13]), (2, vec![21, 22, 23])],
                node: 0,
                frequency: 0.0,
                looping: false,
            };
        let mut tree = GunnerTree::default();
        let first = placed(
            vec![(1.0, vec![(21, 0.4), (22, 0.6)])],
            vec![(1.0, vec![(11, 0.5), (12, 0.5)])],
            vec![1, 2],
        );
        tree.place(&first, 50);
        let raised = |x: f32| (f64::from(0.001f32) * f64::from(x)) as f32;
        assert_eq!(
            tree.rows(0),
            vec![
                (0.001, vec![(11, raised(0.5)), (12, raised(0.5))]),
                (0.001, vec![(21, raised(0.4)), (22, raised(0.6))]),
            ]
        );
        tree.step(0.05);
        assert_eq!(tree.rows(0), vec![(1.0, vec![(21, 0.4), (22, 0.6)])]);
        let second = placed(
            vec![(0.3, vec![(11, 1.0)]), (0.7, vec![(22, 0.5), (23, 0.5)])],
            Vec::new(),
            vec![1, 2],
        );
        tree.place(&second, 50);
        assert_eq!(
            tree.rows(0),
            vec![
                (0.001, vec![(11, 0.001)]),
                (1.0, vec![(21, 0.4), (22, 0.6), (23, raised(0.5))]),
            ]
        );
        tree.step(0.05);
        assert_eq!(
            tree.rows(0),
            vec![(0.3, vec![(11, 1.0)]), (0.7, vec![(22, 0.5), (23, 0.5)])]
        );
    }

    #[test]
    fn the_sync_mean_sums_stored_floats_and_skips_idle_children() {
        // Two five-frame fire leaves at 30 fps (6 a second): the stored
        // sums land a float step under 6.
        let w2 = 1.0f32 / 2000.0;
        let w1 = 1.0 - w2;
        assert_eq!(sync_mean(&[(w1, 6.0), (w2, 6.0)]), 5.9999995);
        assert_eq!(sync_mean(&[(0.5, 6.0), (0.5, 6.0)]), 6.0);
        // A child with no weight or no frequency (a one-frame aim leaf)
        // counts for nothing; with none counted the mean is 0.
        assert_eq!(sync_mean(&[(0.25, 6.0), (0.0, 3.0), (0.75, 0.0)]), 6.0);
        assert_eq!(sync_mean(&[(1.0, 0.0)]), 0.0);
    }

    #[test]
    fn quat_angles_inverts_angles_quat() {
        let a = [20.0, -131.0, -10.0];
        let b = quat_angles(angles_quat(a));
        assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3), "{b:?}");
    }

    #[test]
    fn angles_quat_matches_angles_to_axis() {
        let a = [20.0, 229.0, -10.0];
        let axis = crate::pmove::aim::angles_to_axis(a);
        let q = angles_quat(a);
        for (v, want) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().zip(axis) {
            assert!((q * v).abs_diff_eq(Vec3::from(want), 1e-5), "{:?}", q * v);
        }
    }

    /// Three snapshots of the retail capture
    /// (`crates/server/tests/fixtures/turret/mp_carentan-dm-turret.txt`,
    /// turrets doc 12.2): the barrel centred, at the left arc, and pitched
    /// fully down. The gun is at (1712, 1830, 8) facing 229.
    #[test]
    fn the_captured_gunner_origins_replay() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let bones = crate::model::load_bones(&fs, "mg42_bipod").unwrap();
        let tag = |n: &str| bones.iter().find(|b| b.name == n).unwrap().pos;
        let (aim, weapon) = (tag("tag_aim"), tag("tag_weapon"));
        let turret = (
            Vec3::new(1712.0, 1830.0, 8.0),
            angles_quat([0.0, 229.0, 0.0]),
        );
        let legs = anims.wire_of("standMG42_aim").unwrap();
        // (fixture line, angles2, captured origin)
        for (line, a2, want) in [
            (533, [-2.0, 0.0], [1746.5, 1862.0, -23.9]),
            (250, [0.0, 45.0], [1715.4, 1877.3, -23.9]),
            (568, [40.0, 0.0], [1742.2, 1857.1, -23.9]),
        ] {
            let tag_weapon = tag_weapon_local(aim, weapon, [a2[0], a2[1], 0.0]);
            let at = place_gunner(
                &anims,
                |n| crate::xanim::load(&fs, n).ok().map(Rc::new),
                legs,
                tag_weapon,
                turret,
                Vec3::from(want),
                15.0,
            )
            .unwrap_or_else(|| panic!("line {line}: no placement"))
            .origin;
            // Horizontal only: z is the height handed in (turrets doc 7.2).
            let err = (at - Vec3::from(want)).truncate().length();
            assert!(err < 0.25, "line {line}: {at:?} vs {want:?}, off {err}");
        }
    }

    /// The yaw splits each row between the two columns either side of
    /// `n / 2 - yaw / inc` (turrets doc 7.2), and the leaves' root yaw undoes
    /// the barrel's, so the body keeps facing the gun's base and the leaves
    /// swing the arms.
    #[test]
    fn the_barrel_yaw_picks_the_columns_and_the_body_holds_still() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let bones = crate::model::load_bones(&fs, "mg42_bipod").unwrap();
        let tag = |n: &str| bones.iter().find(|b| b.name == n).unwrap().pos;
        let turret = (
            Vec3::new(1712.0, 1830.0, 8.0),
            angles_quat([0.0, 229.0, 0.0]),
        );
        let legs = anims.wire_of("standMG42_aim").unwrap();
        // A height the 40-down barrel puts wholly on the 15up row.
        let player = Vec3::new(1742.2, 1857.1, -23.9);
        for (yaw, cols) in [
            (0.0, ["forward", "15left"]),
            (45.0, ["45right", "30right"]),
            (-40.0, ["45left", "30left"]),
        ] {
            let tag_weapon = tag_weapon_local(tag("tag_aim"), tag("tag_weapon"), [40.0, yaw, 0.0]);
            let g = place_gunner(
                &anims,
                |n| crate::xanim::load(&fs, n).ok().map(Rc::new),
                legs,
                tag_weapon,
                turret,
                player,
                15.0,
            )
            .unwrap();
            let sum: f32 = g.leaves().map(|l| l.1).sum();
            assert!((sum - 1.0).abs() < 1e-4, "yaw {yaw}: weights sum {sum}");
            for (leaf, w) in g.leaves() {
                let name = &anims.tree.nodes[leaf].name;
                assert!(name.ends_with("_15up"), "yaw {yaw}: {name}");
                assert!(
                    cols.iter().any(|c| name.contains(&format!("_aim_{c}_"))),
                    "yaw {yaw}: {name} at {w}"
                );
            }
            let off = angle_delta(g.yaw(), 229.0);
            assert!(
                (-0.1..7.6).contains(&off),
                "yaw {yaw}: body {off} off the base"
            );
        }
    }

    fn angle_delta(a: f32, b: f32) -> f32 {
        (a - b + 540.0).rem_euclid(360.0) - 180.0
    }

    #[test]
    fn a_leaf_anim_without_turretanim_places_nothing() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).unwrap();
        let legs = anims.wire_of("pb_crouch_alert").unwrap();
        let id = (Vec3::ZERO, Quat::IDENTITY);
        let placed = place_gunner(&anims, |_| None, legs, id, id, Vec3::ZERO, 15.0);
        assert!(placed.is_none());
    }
}
