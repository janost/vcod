//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! The world: its bodies and joints, the sleep test (`autoDisableBodies`,
//! iw3mp.exe 0x5b9b70) and the island walk (`dxProcessIslands`, 0x5b9d40)
//! with IW's island split (0x5b9cc0); cod4-player-anims.md 10.3.

use super::body::{AutoDisable, Body, DISABLED, Node};
use super::joint::Joint;
use super::quickstep;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BodyId(pub(crate) u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JointId(pub(crate) u32);

/// The most bodies IW steps in one island (0x5b9cc3).
pub const ISLAND_MAX_BODIES: usize = 14;
/// An island keeps fewer joints than this (0x5b9cd2, 0x5b9d18).
pub const ISLAND_MAX_JOINTS: usize = 74;

/// The world's step parameters (`dxWorld` +0x10..+0x54).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub gravity: [f32; 3],
    pub erp: f32,
    pub cfm: f32,
    pub qs_iterations: u32,
    pub qs_w: f32,
    /// A new body's sleep test, and whether it has one (`adis_flag`).
    pub adis: AutoDisable,
    pub adis_flag: bool,
    /// The contacts' most correcting velocity and surface layer
    /// (`dxContactParameters`).
    pub contact_max_vel: f32,
    pub contact_min_depth: f32,
}

impl Default for Params {
    /// `dWorldCreate`'s (0x5b7350).
    fn default() -> Params {
        Params {
            gravity: [0.0; 3],
            erp: 0.2,
            cfm: 1e-5,
            qs_iterations: 20,
            qs_w: 1.3,
            adis: AutoDisable {
                linear_threshold: (0.001f32 as f64 * 0.001f32 as f64) as f32,
                angular_threshold: (0.001f32 as f64 * 0.001f32 as f64) as f32,
                idle_time: 0.0,
                idle_steps: 10,
            },
            adis_flag: false,
            contact_max_vel: quickstep::INFINITY,
            contact_min_depth: 0.0,
        }
    }
}

/// The reorder's seed at every `Phys_Init` (0x5b7396).
pub const SEED: u32 = 0x89ab_cdef;

#[derive(Clone, Debug)]
pub struct World {
    pub params: Params,
    /// The reorder's LCG state (world +0x30); it runs on across steps.
    pub seed: u32,
    bodies: Vec<Option<Body>>,
    /// `firstbody`'s list, first visited first.
    body_list: Vec<BodyId>,
    joints: Vec<Option<Joint>>,
    joint_list: Vec<JointId>,
}

impl Default for World {
    fn default() -> World {
        World::new()
    }
}

impl World {
    pub fn new() -> World {
        World {
            params: Params::default(),
            seed: SEED,
            bodies: Vec::new(),
            body_list: Vec::new(),
            joints: Vec::new(),
            joint_list: Vec::new(),
        }
    }

    /// `dBodyCreate`: a unit body at the origin, put at the head of the
    /// world's list.
    pub fn create_body(&mut self) -> BodyId {
        let body = Body::new(self.params.adis, self.params.adis_flag);
        let id = match self.bodies.iter().position(Option::is_none) {
            Some(i) => {
                self.bodies[i] = Some(body);
                BodyId(i as u32)
            }
            None => {
                self.bodies.push(Some(body));
                BodyId(self.bodies.len() as u32 - 1)
            }
        };
        self.body_list.insert(0, id);
        id
    }

    /// `dBodyDestroy`: its joints are detached, left in the world.
    pub fn destroy_body(&mut self, id: BodyId) {
        let body = self.bodies[id.0 as usize].take().expect("live body");
        for n in body.nodes {
            if let Some(j) = self.joints[n.joint.0 as usize].as_mut() {
                j.body = [None, None];
            }
            if let Some(other) = n.other.and_then(|o| self.bodies[o.0 as usize].as_mut()) {
                other.nodes.retain(|m| m.joint != n.joint);
            }
        }
        self.body_list.retain(|&b| b != id);
    }

    pub fn body(&self, id: BodyId) -> &Body {
        self.bodies[id.0 as usize].as_ref().expect("live body")
    }

    pub fn body_mut(&mut self, id: BodyId) -> &mut Body {
        self.bodies[id.0 as usize].as_mut().expect("live body")
    }

    /// `dJointCreate*`: a detached joint at the head of the world's list.
    pub fn create_joint(&mut self, joint: Joint) -> JointId {
        let id = match self.joints.iter().position(Option::is_none) {
            Some(i) => {
                self.joints[i] = Some(joint);
                JointId(i as u32)
            }
            None => {
                self.joints.push(Some(joint));
                JointId(self.joints.len() as u32 - 1)
            }
        };
        self.joint_list.insert(0, id);
        id
    }

    pub fn destroy_joint(&mut self, id: JointId) {
        self.detach(id);
        self.joints[id.0 as usize] = None;
        self.joint_list.retain(|&j| j != id);
    }

    pub fn joint(&self, id: JointId) -> &Joint {
        self.joints[id.0 as usize].as_ref().expect("live joint")
    }

    pub fn joint_mut(&mut self, id: JointId) -> &mut Joint {
        self.joints[id.0 as usize].as_mut().expect("live joint")
    }

    fn detach(&mut self, id: JointId) {
        let j = self.joints[id.0 as usize].as_mut().expect("live joint");
        let ends = std::mem::take(&mut j.body);
        for b in ends.into_iter().flatten() {
            if let Some(body) = self.bodies[b.0 as usize].as_mut() {
                body.nodes.retain(|n| n.joint != id);
            }
        }
    }

    /// `dJointAttach`: a world end goes second (`dJOINT_REVERSE`), and
    /// each body takes the joint at the head of its list, body 1 first.
    pub fn attach(&mut self, id: JointId, b1: Option<BodyId>, b2: Option<BodyId>) {
        self.detach(id);
        let (b1, b2, reverse) = match (b1, b2) {
            (None, b) => (b, None, true),
            (a, b) => (a, b, false),
        };
        let j = self.joints[id.0 as usize].as_mut().expect("live joint");
        j.body = [b1, b2];
        j.reverse = reverse;
        if let Some(b) = b1 {
            self.body_mut(b).nodes.insert(
                0,
                Node {
                    joint: id,
                    other: b2,
                },
            );
        }
        if let Some(b) = b2 {
            self.body_mut(b).nodes.insert(
                0,
                Node {
                    joint: id,
                    other: b1,
                },
            );
        }
    }

    /// The world's bodies, first visited first.
    pub fn bodies(&self) -> &[BodyId] {
        &self.body_list
    }

    /// `dWorldQuickStep`: one step of `h` seconds over every awake island.
    pub fn quick_step(&mut self, h: f32) {
        if self.body_list.is_empty() {
            return;
        }
        self.auto_disable(h);
        self.untag();
        for bi in 0..self.body_list.len() {
            let root = self.body_list[bi];
            let b = self.bodies[root.0 as usize].as_mut().expect("live body");
            if b.tag != 0 || b.flags & DISABLED != 0 {
                continue;
            }
            b.tag = 1;
            let (island, jlist) = self.gather(root);
            if island.is_empty() {
                continue;
            }
            quickstep::step(
                &self.params,
                &mut self.seed,
                &mut self.bodies,
                &island,
                &mut self.joints,
                &jlist,
                h,
            );
        }
    }

    fn untag(&mut self) {
        for &b in &self.body_list {
            self.bodies[b.0 as usize].as_mut().expect("live body").tag = 0;
        }
        for &j in &self.joint_list {
            self.joints[j.0 as usize].as_mut().expect("live joint").tag = 0;
        }
    }

    /// `root`'s island, split, its joints' ends numbered by their bodies'
    /// places in it (0x5b9e34..0x5b9ecb).
    fn gather(&mut self, root: BodyId) -> (Vec<BodyId>, Vec<JointId>) {
        let (mut island, mut jlist) = self.walk(root);
        split(&mut self.bodies, &mut self.joints, &mut island, &mut jlist);
        for (i, &b) in island.iter().enumerate() {
            self.bodies[b.0 as usize].as_mut().expect("live body").tag = i as i32;
        }
        for &j in &jlist {
            let joint = self.joints[j.0 as usize].as_mut().expect("live joint");
            joint.index = joint.body.map(|b| {
                b.map_or(-1, |b| {
                    self.bodies[b.0 as usize].as_ref().expect("live body").tag
                })
            });
        }
        for &b in &island {
            self.bodies[b.0 as usize].as_mut().expect("live body").tag = 1;
        }
        (island, jlist)
    }

    #[cfg(test)]
    pub(crate) fn split_island_for_test(&mut self, root: BodyId) -> (Vec<BodyId>, Vec<JointId>) {
        self.untag();
        self.body_mut(root).tag = 1;
        self.gather(root)
    }

    /// Collects `root`'s island, tagging what it takes; a body reached
    /// through a joint wakes (0x5b9dc7..0x5b9e32).
    fn walk(&mut self, root: BodyId) -> (Vec<BodyId>, Vec<JointId>) {
        let mut island = vec![root];
        let mut jlist = Vec::new();
        let mut stack = Vec::new();
        let mut b = root;
        loop {
            let nodes = self.bodies[b.0 as usize]
                .as_ref()
                .expect("live body")
                .nodes
                .clone();
            for n in nodes {
                let j = self.joints[n.joint.0 as usize]
                    .as_mut()
                    .expect("live joint");
                if j.tag != 0 {
                    continue;
                }
                j.tag = 1;
                jlist.push(n.joint);
                if let Some(o) = n.other {
                    let other = self.bodies[o.0 as usize].as_mut().expect("live body");
                    if other.tag == 0 {
                        other.flags &= !DISABLED;
                        stack.push(o);
                        other.tag = 1;
                    }
                }
            }
            match stack.pop() {
                Some(next) => {
                    island.push(next);
                    b = next;
                }
                None => break,
            }
        }
        (island, jlist)
    }

    /// `autoDisableBodies` (0x5b9b70), before the islands: a body idle
    /// past both its steps and its time sleeps.
    fn auto_disable(&mut self, h: f32) {
        for &id in &self.body_list {
            let b = self.bodies[id.0 as usize].as_mut().expect("live body");
            if b.flags & (super::body::AUTO_DISABLE | DISABLED) != super::body::AUTO_DISABLE {
                continue;
            }
            // `thresh < speed2` is the not-idle test (0x5b9bc4, 0x5b9c01):
            // a NaN speed reads idle.
            let lv = super::math::dot(b.lvel, b.lvel);
            let moving =
                |thresh: f32, v2: f32| thresh.partial_cmp(&v2) == Some(std::cmp::Ordering::Less);
            let idle = !moving(b.adis.linear_threshold, lv)
                && !moving(b.adis.angular_threshold, super::math::dot(b.avel, b.avel));
            if idle {
                b.adis_timeleft = (b.adis_timeleft as f64 - h as f64) as f32;
                b.adis_stepsleft -= 1;
            } else {
                b.adis_stepsleft = b.adis.idle_steps;
                b.adis_timeleft = b.adis.idle_time;
            }
            if b.adis_stepsleft < 0 && b.adis_timeleft < 0.0 {
                b.flags |= DISABLED;
            }
        }
    }
}

/// IW's split (0x5b9cc0): past 14 bodies the last ones are dropped, then
/// one more at a time while 74 joints or more remain. A dropped body's
/// joint goes with it when its other end is dropped too or is the world;
/// one to a kept body stays, its dropped end reading index 0
/// (0x5b9c60). Dropped bodies and joints are untagged for a later island.
fn split(
    bodies: &mut [Option<Body>],
    joints: &mut [Option<Joint>],
    island: &mut Vec<BodyId>,
    jlist: &mut Vec<JointId>,
) {
    if island.len() < ISLAND_MAX_BODIES && jlist.len() < ISLAND_MAX_JOINTS {
        return;
    }
    let mut kept = island.len();
    while kept > ISLAND_MAX_BODIES {
        kept -= 1;
        bodies[island[kept].0 as usize]
            .as_mut()
            .expect("live body")
            .tag = 0;
    }
    let mut was = island.len();
    loop {
        if was > kept {
            partition(joints, jlist, &island[kept..was]);
        }
        if jlist.len() < ISLAND_MAX_JOINTS || kept == 0 {
            break;
        }
        was = kept;
        kept -= 1;
        bodies[island[kept].0 as usize]
            .as_mut()
            .expect("live body")
            .tag = 0;
    }
    island.truncate(kept);
}

/// `std::partition` over the joints (0x5b9f20) with 0x5b9c60's test: the
/// kept ones stay in front, swapped in from the back.
fn partition(joints: &mut [Option<Joint>], jlist: &mut Vec<JointId>, dropped: &[BodyId]) {
    let mut keep = |id: JointId| -> bool {
        let j = joints[id.0 as usize].as_mut().expect("live joint");
        let hits = dropped
            .iter()
            .filter(|&&d| j.body[0] == Some(d) || j.body[1] == Some(d))
            .count();
        let drop =
            hits == 2 || (hits == 1 && (j.tag == 0 || j.body[1].is_none() || j.body[0].is_none()));
        if drop {
            j.tag = 0;
        }
        !drop
    };
    let (mut first, mut last) = (0, jlist.len());
    loop {
        while first != last && keep(jlist[first]) {
            first += 1;
        }
        if first == last {
            break;
        }
        loop {
            last -= 1;
            if first == last {
                jlist.truncate(first);
                return;
            }
            if keep(jlist[last]) {
                break;
            }
        }
        jlist.swap(first, last);
        first += 1;
    }
    jlist.truncate(first);
}
