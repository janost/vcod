//! iw3mp.exe 1.7's ragdolls on the ODE port: the 32 slots and their states
//! (`Ragdoll_Update` 0x5beca0, the handlers at 0x71e658), the build
//! (bodies 0x5bc860, joints 0x5bcb30, the placement 0x5bc670, velocities
//! 0x5be060, the tunnel test 0x5be3a0) and the self collision hook world
//! 2 steps with (`ode::phys`); cod4-player-anims.md 10.7.

use glam::{Mat3, Quat, Vec3};
use vcod_common::skeleton::Skeleton;

use super::iw::{self, IwQuat};
use super::{Dvars, Explosion, JointKind, Limit, RagdollDef, Reach};
use crate::ode::body::DISABLED;
use crate::ode::collide::selfcol::{self, bone_segment};
use crate::ode::collide::{self, Obj};
use crate::ode::joint::amotor::{AMotor, Rel};
use crate::ode::joint::ball::Ball;
use crate::ode::joint::hinge::Hinge;
use crate::ode::joint::limot::{LimitMotor, Param};
use crate::ode::math::{M3, V3, mul0_331};
use crate::ode::phys::{self, Geom, PhysObj, PhysWorld, WorldId, axes_of};
use crate::ode::world::PhysDvars;
use crate::ode::{BodyId, Joint, JointId, Kind, Mass, World};

pub use crate::ode::phys::Clip;

/// `ragdollPool[32]` (0x9d4 bytes each from 0xcc78a00).
pub const SLOTS: usize = 32;
/// The wait for the DObj gives up past this many frames (0x5beb90).
const DOBJ_FRAMES: i32 = 3;
/// The joints' motor force at the build, times the limit's friction (the
/// double at 0x70b588).
const BUILD_FMAX: f64 = 15.0;
/// A limit motor's fudge factor (0x70b264).
const FUDGE: f32 = 0.9;
/// A base lerp bone further than this from its ragdoll bone takes its
/// base pose at once (0x70b398).
const LERP_JUMP: f32 = 400.0;

/// A slot's state (+0x24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Free = 0,
    /// Waiting for the corpse's DObj, at most 3 frames.
    WaitDObj = 1,
    /// Taking two pose snapshots a frame apart.
    Snapshots = 2,
    Build = 3,
    Simulating = 4,
    /// The pose held, the ODE objects freed; a blast rebuilds them.
    Idle = 5,
}

/// One bone in a pose snapshot (0x20 bytes): position and IW quaternion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Record {
    pub pos: V3,
    pub q: IwQuat,
}

/// What a corpse's DObj shows the ragdoll on a frame.
pub struct DObj<'a> {
    pub skel: &'a Skeleton,
    /// Each skeleton bone's base pose in the model (its `XModel` baseMat),
    /// which the bodies and joints are built in.
    pub base: &'a [(Vec3, Quat)],
    /// The entity's origin and turn (`cpose`).
    pub origin: Vec3,
    pub turn: Quat,
    /// Each bone in the world now, as drawn.
    pub world: &'a [(Vec3, Quat)],
}

/// A slot's hold on its corpse's DObj: the def bones' and base lerp bones'
/// skeleton indices (0x5be940), the base pose and the last pose fed.
#[derive(Clone, Debug)]
struct Fed {
    p0: Vec<usize>,
    p1: Vec<Option<usize>>,
    lerp: Vec<usize>,
    parents: Vec<i32>,
    base: Vec<(Vec3, Quat)>,
    origin: Vec3,
    turn: Quat,
    world: Vec<(Vec3, Quat)>,
    /// Each bone's world pose under its parent's, kept from the last feed:
    /// what a bone the ragdoll does not set is drawn with.
    local: Vec<(Vec3, Quat)>,
    fresh: bool,
}

/// One def bone of a slot (0x1c bytes from +0x38).
#[derive(Clone, Debug)]
struct SlotBone {
    parent: Option<usize>,
    body: Option<BodyId>,
    /// `|p1 - p0|` in the base pose (+0xc).
    len: f32,
    /// The phys object's offset (its +0): `(-len / 2, 0, 0)`.
    offset: V3,
}

/// One base lerp bone: its ragdoll bone, msec, and its pose under that
/// bone now and in the base pose (+0x8b4).
#[derive(Clone, Copy, Debug)]
struct LerpBone {
    bone: usize,
    msec: i32,
    base: (Vec3, Quat),
}

#[derive(Clone, Debug)]
struct Slot {
    state: State,
    msec: i32,
    frames: i32,
    /// +0x30: the time between the two snapshots.
    dt: i32,
    bones: Vec<SlotBone>,
    joints: Vec<[Option<JointId>; 2]>,
    /// The def's joint limits as the build left them (a joint with none
    /// takes the default z axis, written into the def at 0x5bcbd4).
    limits: Vec<Vec<Limit>>,
    lerps: Vec<LerpBone>,
    /// The two pose snapshots (+0x2f4) and the latest (+0x2f0): the def
    /// bones' then the base lerp bones'.
    snaps: [Vec<Record>; 2],
    cur: usize,
    fed: Option<Fed>,
    /// The pose drawn last, every skeleton bone in the world.
    drawn: Vec<(Vec3, Quat)>,
    /// Bumped whenever [`Ragdolls::pose`] may change.
    version: u64,
}

impl Slot {
    fn new() -> Slot {
        Slot {
            state: State::Free,
            msec: 0,
            frames: 0,
            dt: 0,
            bones: Vec::new(),
            joints: Vec::new(),
            limits: Vec::new(),
            lerps: Vec::new(),
            snaps: [Vec::new(), Vec::new()],
            cur: 0,
            fed: None,
            drawn: Vec::new(),
            version: 0,
        }
    }
}

/// A slot's handle, with the generation it was taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId {
    index: usize,
    generation: u64,
}

/// The ragdoll pool and world 2.
#[derive(Clone, Debug)]
pub struct Ragdolls {
    pub def: std::sync::Arc<RagdollDef>,
    pub dvars: Dvars,
    pub phys: PhysDvars,
    pub contact: collide::Dvars,
    pw: PhysWorld,
    slots: Vec<Slot>,
    generations: Vec<u64>,
    /// The ragdoll clock (0xd5ec534).
    time: i32,
}

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

fn v3(v: Vec3) -> V3 {
    v.to_array()
}

fn iwq(q: Quat) -> IwQuat {
    q.to_array()
}

impl Ragdolls {
    /// An empty pool over `def`, world 2 as `Phys_Init` leaves it: the
    /// reorder's seed at 0x89abcdef.
    pub fn new(def: std::sync::Arc<RagdollDef>, dvars: Dvars) -> Ragdolls {
        Ragdolls {
            def,
            dvars,
            phys: PhysDvars::default(),
            contact: collide::Dvars::default(),
            pw: PhysWorld::new(WorldId::Ragdoll),
            slots: (0..SLOTS).map(|_| Slot::new()).collect(),
            generations: vec![0; SLOTS],
            time: 0,
        }
    }

    fn slot(&self, id: SlotId) -> Option<&Slot> {
        (self.generations[id.index] == id.generation).then(|| &self.slots[id.index])
    }

    /// 0x5ba1f0: the slots building or simulating.
    fn simulating(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s.state, State::Build | State::Simulating))
            .count()
    }

    /// `Ragdoll_CreateRagdollForDObj` (0x5ba270): a free slot waiting for
    /// its DObj, `None` once `ragdoll_max_simulating` build or simulate or
    /// every slot is taken.
    pub fn create(&mut self) -> Option<SlotId> {
        if self.simulating() >= self.dvars.max_simulating {
            return None;
        }
        let index = self.slots.iter().position(|s| s.state == State::Free)?;
        self.generations[index] += 1;
        self.slots[index] = Slot::new();
        self.transition(index, State::WaitDObj, None);
        Some(SlotId {
            index,
            generation: self.generations[index],
        })
    }

    /// The corpse is gone: its slot goes free (0x4404d2).
    pub fn release(&mut self, id: SlotId) {
        if self.slot(id).is_some() {
            self.transition(id.index, State::Free, None);
            self.generations[id.index] += 1;
        }
    }

    pub fn state(&self, id: SlotId) -> Option<State> {
        self.slot(id).map(|s| s.state)
    }

    /// Whether the slot still reads its corpse's animated pose: until it
    /// simulates.
    pub fn wants_pose(&self, id: SlotId) -> bool {
        self.slot(id)
            .is_some_and(|s| matches!(s.state, State::WaitDObj | State::Snapshots | State::Build))
    }

    /// The corpse's DObj this frame; [`Self::update`] reads it.
    pub fn feed(&mut self, id: SlotId, d: &DObj) {
        if self.slot(id).is_none() {
            return;
        }
        let def = self.def.clone();
        let s = &mut self.slots[id.index];
        let n = d.skel.bone_count();
        if d.world.len() != n || d.base.len() != n {
            s.fed = None;
            return;
        }
        let parents: Vec<i32> = d.skel.bones().iter().map(|b| b.parent).collect();
        let local = (0..n)
            .map(|b| match parents[b] {
                p if p >= 0 => {
                    let (pp, pr) = d.world[p as usize];
                    let inv = pr.inverse();
                    (inv * (d.world[b].0 - pp), (inv * d.world[b].1).normalize())
                }
                _ => d.world[b],
            })
            .collect();
        let find = |name: &str| d.skel.bone_index(name);
        let p0: Option<Vec<usize>> = def.bones.iter().map(|b| find(&b.p0)).collect();
        let p1: Vec<Option<usize>> = def
            .bones
            .iter()
            .map(|b| {
                (!b.p1.eq_ignore_ascii_case("none"))
                    .then(|| find(&b.p1))
                    .flatten()
            })
            .collect();
        let lerp: Option<Vec<usize>> = def.base_lerp.iter().map(|l| find(&l.bone)).collect();
        // 0x5bc730: every name resolves, else the slot waits.
        let (Some(p0), Some(lerp)) = (p0, lerp) else {
            s.fed = None;
            return;
        };
        if def
            .bones
            .iter()
            .zip(&p1)
            .any(|(b, p)| !b.p1.eq_ignore_ascii_case("none") && p.is_none())
        {
            s.fed = None;
            return;
        }
        s.fed = Some(Fed {
            p0,
            p1,
            lerp,
            parents,
            base: d.base.to_vec(),
            origin: d.origin,
            turn: d.turn,
            world: d.world.to_vec(),
            local,
            fresh: true,
        });
    }

    /// 0x5bec30: the exit of the old state, the new one written, its
    /// entry; a refused exit or entry leaves the old one.
    fn transition(&mut self, i: usize, to: State, clip: Option<&dyn Clip>) -> bool {
        let from = self.slots[i].state;
        if from == to {
            return true;
        }
        if !self.exit(i, from, to) {
            return false;
        }
        self.slots[i].state = to;
        if !self.enter(i, from, to, clip) {
            if self.slots[i].state == to {
                self.slots[i].state = from;
            }
            return false;
        }
        let s = &mut self.slots[i];
        s.frames = 0;
        s.msec = 0;
        s.version += 1;
        true
    }

    fn exit(&mut self, i: usize, from: State, to: State) -> bool {
        match from {
            State::Free => {
                let def = self.def.clone();
                let s = &mut self.slots[i];
                s.bones = def
                    .bones
                    .iter()
                    .map(|b| SlotBone {
                        parent: b.parent,
                        body: None,
                        len: 0.0,
                        offset: [0.0; 3],
                    })
                    .collect();
                s.joints = vec![[None, None]; def.joints.len()];
                true
            }
            State::WaitDObj => to == State::Free || self.slots[i].fed.is_some(),
            State::Idle if !matches!(to, State::Free | State::WaitDObj) => {
                // 0x5beaa0: the bones there, room to simulate, the bodies and
                // joints rebuilt, placed at the pose held.
                if self.slots[i].fed.is_none() || self.simulating() >= self.dvars.max_simulating {
                    return false;
                }
                if !self.build_bodies(i) {
                    self.destroy(i);
                    return false;
                }
                self.build_joints(i);
                let cur = self.slots[i].cur;
                let rec = self.slots[i].snaps[cur].clone();
                self.place(i, &rec);
                true
            }
            _ => true,
        }
    }

    fn enter(&mut self, i: usize, from: State, to: State, clip: Option<&dyn Clip>) -> bool {
        match to {
            State::Free => {
                self.destroy(i);
                let s = &mut self.slots[i];
                let keep = s.version;
                *s = Slot::new();
                s.version = keep + 1;
                true
            }
            State::Build => {
                self.build(i, clip);
                true
            }
            State::Simulating => {
                // 0x5beb60: the count holds this slot already.
                if self.simulating() < self.dvars.max_simulating {
                    return true;
                }
                let back = if from == State::Idle {
                    State::Idle
                } else {
                    State::Free
                };
                self.transition(i, back, clip);
                false
            }
            State::Idle => {
                self.destroy(i);
                true
            }
            _ => true,
        }
    }

    /// 0x5bce20 and 0x5bce80: the joints, then the bodies and their geoms.
    fn destroy(&mut self, i: usize) {
        for j in std::mem::take(&mut self.slots[i].joints) {
            for id in j.into_iter().flatten() {
                self.pw.world.destroy_joint(id);
            }
        }
        self.slots[i].joints = vec![[None, None]; self.def.joints.len()];
        for b in 0..self.slots[i].bones.len() {
            if let Some(id) = self.slots[i].bones[b].body.take() {
                self.pw.destroy_body(id);
            }
        }
    }

    /// `Ragdoll_Update` (0x5beca0): the clock on by `msec`, every slot's
    /// handlers until its state holds, then world 2 stepped to the clock.
    /// `ragdoll_enable` off steps nothing (0x5becb0).
    pub fn update(&mut self, msec: i32, clip: &dyn Clip) {
        if !self.dvars.enable {
            return;
        }
        self.time = self.time.wrapping_add(msec);
        for i in 0..SLOTS {
            if self.slots[i].state == State::Free {
                continue;
            }
            loop {
                let s = self.slots[i].state;
                self.handle(i, clip);
                if self.slots[i].state == s {
                    break;
                }
            }
            let s = &mut self.slots[i];
            if s.state != State::Free {
                s.frames += 1;
                s.msec += msec;
            }
            if let Some(f) = &mut s.fed {
                f.fresh = false;
            }
        }
        let (slots, def, contact) = (&self.slots, &*self.def, &self.contact);
        self.pw
            .run_to_time(self.time, &self.phys, contact, clip, &mut |pw| {
                self_collide(slots, def, contact, pw)
            });
    }

    fn handle(&mut self, i: usize, clip: &dyn Clip) {
        match self.slots[i].state {
            State::WaitDObj => {
                if self.slots[i].frames > DOBJ_FRAMES {
                    log::debug!("Ragdoll activation timed out waiting for dobj");
                    self.transition(i, State::Free, None);
                } else if self.slots[i].fed.as_ref().is_some_and(|f| f.fresh) {
                    self.transition(i, State::Snapshots, None);
                }
            }
            State::Snapshots => {
                // 0x5be720: a snapshot each frame, the build on the second.
                if self.slots[i].fed.as_ref().is_none_or(|f| !f.fresh) {
                    self.transition(i, State::Free, None);
                    return;
                }
                let s = &mut self.slots[i];
                s.cur ^= 1;
                let rec = snapshot(s.fed.as_ref().expect("fed"));
                let cur = s.cur;
                s.snaps[cur] = rec;
                if s.frames >= 1 {
                    s.dt = s.msec;
                    self.transition(i, State::Build, Some(clip));
                }
            }
            State::Simulating => {
                if self.idle(i) {
                    self.transition(i, State::Idle, None);
                    return;
                }
                self.save_pose(i);
                self.update_friction(i);
            }
            _ => {}
        }
    }

    /// 0x5be890: past `ragdoll_max_life`, or every body asleep.
    fn idle(&self, i: usize) -> bool {
        let s = &self.slots[i];
        s.msec > self.dvars.max_life_ms
            || s.bones.iter().all(|b| {
                b.body
                    .is_none_or(|id| self.pw.world.body(id).flags & DISABLED != 0)
            })
    }

    /// State 3's entry (0x5be600): bodies and joints in the base pose,
    /// placed at the latest snapshot and set moving off the two, the base
    /// lerp bones' first pose, the tunnel test; then simulating, or freed.
    fn build(&mut self, i: usize, clip: Option<&dyn Clip>) {
        if !self.build_bodies(i) {
            self.destroy(i);
            self.transition(i, State::Free, None);
            return;
        }
        self.build_joints(i);
        let s = &self.slots[i];
        let (cur, prev, dt) = (s.snaps[s.cur].clone(), s.snaps[s.cur ^ 1].clone(), s.dt);
        self.place(i, &cur);
        self.set_velocities(i, &cur, &prev, dt);
        let clear = clip.is_none_or(|c| self.tunnel_test(i, &cur, c));
        self.start_lerps(i);
        let next = if clear {
            State::Simulating
        } else {
            State::Free
        };
        self.transition(i, next, clip);
    }

    /// 0x5bcdc0: a capsule body per def bone (0x5bc860) in the base pose
    /// under the entity, through 0x5a4d90 and 0x5a56a0.
    fn build_bodies(&mut self, i: usize) -> bool {
        let def = self.def.clone();
        let Some(f) = self.slots[i].fed.clone() else {
            return false;
        };
        let m = Mat3::from_quat(f.turn);
        let qent = iwq(f.turn);
        let at = |t: Vec3| -> V3 {
            std::array::from_fn(|k| {
                let c = |j: usize| w(m.col(j)[k]);
                (((c(0) * w(t.x) + c(1) * w(t.y)) + c(2) * w(t.z)) as f32 as f64 + w(f.origin[k]))
                    as f32
            })
        };
        for (b, d) in def.bones.iter().enumerate() {
            let (t0, q0) = f.base[f.p0[b]];
            let p0 = at(t0);
            let mut q = iw::qmul(iwq(q0), qent);
            if d.mirror {
                q = iw::flip(q);
            }
            // a p1 at DObj bone 0 reads as none (0x5bc947)
            let p1 = f.p1[b].filter(|&k| k != 0).map_or(p0, |k| at(f.base[k].0));
            // a full body pool fails the build
            let Some(id) = self.pw.create_body() else {
                return false;
            };
            let body = self.pw.world.body_mut(id);
            body.flags &= !3;
            body.set_finite_rotation(true);
            body.pos = p0;
            body.set_rotation(&iw::axes_to_r(&iw::axes(q)));
            // 0x5a4d90's sphere of the bone's mass, which the capsule's
            // replaces
            let mut m = Mass::unit();
            m.mass = d.mass;
            let d2: [f32; 3] = std::array::from_fn(|k| (w(p1[k]) - w(p0[k])) as f32);
            let len = w(((w(d2[0]) * w(d2[0]) + w(d2[1]) * w(d2[1])) + w(d2[2]) * w(d2[2])) as f32)
                .sqrt() as f32;
            let h = (w(len) * 0.5) as f32;
            // 0x5a47c0: the body moved from p0 by `R (h, 0, 0)`, the object
            // point left at p0
            let r = body.r;
            body.pos = std::array::from_fn(|k| {
                let t = ((0.0 * w(r[k][1]) + w(h) * w(r[k][0])) + 0.0 * w(r[k][2])) as f32;
                (w(p0[k]) + w(t)) as f32
            });
            let mut mass = Mass::capped_cylinder(1.0, 1, d.radius, h);
            mass.adjust(m.mass);
            body.set_mass(mass);
            let pos = body.pos;
            let axes = axes_of(&body.r);
            let sb = &mut self.slots[i].bones[b];
            sb.body = Some(id);
            sb.len = len;
            sb.offset = [-h, 0.0, 0.0];
            self.pw.add_obj(
                id,
                PhysObj {
                    offset: [-h, 0.0, 0.0],
                    // the bone block's sound row (+0x68) is not read; nothing plays it
                    sound: 0,
                    obj: Obj {
                        friction: d.friction,
                        bounce: 0.0,
                        state: 0,
                    },
                    prev: (pos, axes),
                    geoms: vec![Geom::Capsule {
                        radius: d.radius,
                        half_len: h,
                    }],
                },
            );
        }
        true
    }

    /// 0x5bcd60: each def joint (0x5bcb30), the child's body first, at the
    /// base pose: a hinge (0x5a7150), or a ball (0x5a7250) and an Euler
    /// angular motor (0x5a7510), anchored at the child bone's root.
    fn build_joints(&mut self, i: usize) {
        let def = self.def.clone();
        let params = self.pw.world.params;
        self.slots[i].limits = def.joints.iter().map(|j| j.limits.clone()).collect();
        for (j, jd) in def.joints.iter().enumerate() {
            let sb = &self.slots[i].bones[jd.bone];
            let Some(b1) = sb.body else { continue };
            let b2 = sb.parent.and_then(|p| self.slots[i].bones[p].body);
            let offset = sb.offset;
            let body1 = self.pw.world.body(b1);
            let r1 = body1.r;
            let anchor = iw::offset(body1.pos, &axes_of(&r1), offset);
            let lims = &mut self.slots[i].limits[j];
            if lims.is_empty() {
                lims.push(Limit {
                    axis: [0.0, 0.0, 1.0],
                    lo: -1.570_796_4,
                    hi: 1.570_796_4,
                    friction: 0.0,
                });
            }
            let lims = lims.clone();
            let lim = |k: usize| lims.get(k).copied();
            let axis = |k: usize| lim(k).map_or([0.0; 3], |l| mul0_331(&r1, l.axis));
            let fmax = |k: usize| lim(k).map_or(0.0, |l| (w(l.friction) * BUILD_FMAX) as f32);
            let stops = |k: usize| lim(k).map_or((0.0, 0.0), |l| (l.lo, l.hi));
            match jd.kind {
                JointKind::None => {}
                JointKind::Hinge => {
                    let id = self
                        .pw
                        .world
                        .create_joint(Joint::new(Kind::Hinge(Hinge::new(&params))));
                    self.pw.world.attach(id, Some(b1), b2);
                    self.pw.world.with_joint(id, |jt, b| {
                        if let Kind::Hinge(h) = &mut jt.kind {
                            h.set_anchor(b, anchor);
                            h.set_axis(b, axis(0));
                            set_limot(&mut h.limot, fmax(0), stops(0));
                        }
                    });
                    self.slots[i].joints[j] = [Some(id), None];
                }
                JointKind::Swivel => {
                    let ball = self
                        .pw
                        .world
                        .create_joint(Joint::new(Kind::Ball(Ball::default())));
                    self.pw.world.attach(ball, Some(b1), b2);
                    self.pw.world.with_joint(ball, |jt, b| {
                        if let Kind::Ball(x) = &mut jt.kind {
                            x.set_anchor(b, anchor);
                        }
                    });
                    let am = self
                        .pw
                        .world
                        .create_joint(Joint::new(Kind::AMotor(AMotor::euler(&params))));
                    self.pw.world.attach(am, Some(b1), b2);
                    self.pw.world.with_joint(am, |jt, b| {
                        if let Kind::AMotor(a) = &mut jt.kind {
                            let rel2 = if b2.is_some() {
                                Rel::Body2
                            } else {
                                Rel::Global
                            };
                            a.set_axis(b, 2, rel2, axis(2));
                            a.set_axis(b, 0, Rel::Body1, axis(0));
                            for k in 0..3 {
                                set_limot(&mut a.limot[k], fmax(k), stops(k));
                            }
                        }
                    });
                    self.slots[i].joints[j] = [Some(ball), Some(am)];
                }
            }
        }
    }

    /// 0x5bc670: each body at its bone's snapshot through 0x5a5090: the
    /// turn the bone's (half round z for a mirrored one), the centre off
    /// the bone's root by the phys object's offset turned.
    fn place(&mut self, i: usize, rec: &[Record]) {
        let def = self.def.clone();
        for b in 0..self.slots[i].bones.len() {
            let sb = &self.slots[i].bones[b];
            let (Some(id), Some(r)) = (sb.body, rec.get(b)) else {
                continue;
            };
            let q = if def.bones[b].mirror {
                iw::flip(r.q)
            } else {
                r.q
            };
            let a = iw::axes(q);
            let o = sb.offset;
            let pos: V3 = std::array::from_fn(|k| {
                let t =
                    ((w(o[0]) * w(a[0][k]) + w(a[1][k]) * w(o[1])) + w(a[2][k]) * w(o[2])) as f32;
                (w(r.pos[k]) - w(t)) as f32
            });
            let body = self.pw.world.body_mut(id);
            body.pos = pos;
            body.set_rotation(&iw::axes_to_r(&a));
            if let Some(o) = self.pw.obj_mut(id) {
                o.prev = (pos, a);
            }
        }
    }

    /// 0x5be060: each body's velocities off the two snapshots `dt` ms
    /// apart: its bone root's move, and the turn between the quaternions
    /// as a rotation vector times `ragdoll_rotvel_scale`.
    fn set_velocities(&mut self, i: usize, cur: &[Record], prev: &[Record], dt: i32) {
        if dt == 0 {
            return;
        }
        let inv = (1000.0 / dt as f64) as f32;
        let sc = (w(inv) * w(self.dvars.rotvel_scale)) as f32;
        for b in 0..self.slots[i].bones.len() {
            let (Some(id), Some(c), Some(p)) =
                (self.slots[i].bones[b].body, cur.get(b), prev.get(b))
            else {
                continue;
            };
            let d: V3 = std::array::from_fn(|k| (w(c.pos[k]) - w(p.pos[k])) as f32);
            let (cq, pq) = (c.q, p.q);
            let dot = (((w(cq[1]) * w(pq[1]) + w(cq[0]) * w(pq[0])) + w(pq[2]) * w(cq[2]))
                + w(cq[3]) * w(pq[3])) as f32;
            let pq = if dot < 0.0 { pq.map(|x| -x) } else { pq };
            let (cq, pq) = (iw::normalize(cq), iw::normalize(pq));
            let dq = iw::normalize(iw::qmul(iw::conj(pq), cq));
            let rv = iw::rotation_vector(dq);
            // 0x5bb8e0: `(c (x) c*) (x) rv`, the identity but for rounding
            let t = iw::qmul(iw::conj(cq), cq);
            let av = iw::qmul([rv[0], rv[1], rv[2], 0.0], t);
            let body = self.pw.world.body_mut(id);
            body.avel = std::array::from_fn(|k| (w(av[k]) * w(sc)) as f32);
            body.lvel = std::array::from_fn(|k| (w(d[k]) * w(inv)) as f32);
        }
    }

    /// `Ragdoll_TunnelTest` (0x5be3a0) on the latest snapshot: each bone
    /// along its own x for its length, and a bone with more than one child
    /// from its middle to each child's root, through `Ragdoll_BoneTrace`.
    fn tunnel_test(&self, i: usize, cur: &[Record], clip: &dyn Clip) -> bool {
        let def = &self.def;
        let s = &self.slots[i];
        let bone_trace = |a: Vec3, b: Vec3| {
            let z = Vec3::ZERO;
            let f = clip.sweep(a, b, z, z);
            if f.startsolid {
                !clip.sweep(b, a, z, z).startsolid
            } else {
                f.fraction == 1.0 || clip.sweep(b, a, z, z).fraction == 1.0
            }
        };
        for (b, sb) in s.bones.iter().enumerate() {
            if sb.body.is_none() {
                continue;
            }
            let Some(r) = cur.get(b) else { continue };
            let q = if def.bones[b].mirror {
                iw::flip(r.q)
            } else {
                r.q
            };
            let ax = Vec3::from(iw::axes(q)[0]);
            let p0 = Vec3::from(r.pos);
            let children: Vec<usize> = (0..s.bones.len())
                .filter(|&c| s.bones[c].parent == Some(b))
                .take(6)
                .collect();
            if children.len() > 1 {
                let mid = p0 + ax * (w(sb.len) * 0.5) as f32;
                if children.iter().any(|&c| {
                    cur.get(c)
                        .is_some_and(|rc| !bone_trace(mid, Vec3::from(rc.pos)))
                }) {
                    return false;
                }
            }
            if !bone_trace(p0, p0 + ax * sb.len) {
                return false;
            }
        }
        true
    }

    /// 0x5bcef0 and the build's 0x5bbf90: each base lerp bone's base pose
    /// under its ragdoll bone, and its first pose, the DObj's.
    fn start_lerps(&mut self, i: usize) {
        let def = self.def.clone();
        let Some(f) = self.slots[i].fed.clone() else {
            return;
        };
        let s = &mut self.slots[i];
        s.lerps = def
            .base_lerp
            .iter()
            .enumerate()
            .filter(|(_, l)| l.ragdoll_bone < def.bones.len())
            .map(|(k, l)| {
                let (pb, qb) = f.base[f.p0[l.ragdoll_bone]];
                let (pl, ql) = f.base[f.lerp[k]];
                let inv = qb.inverse();
                LerpBone {
                    bone: l.ragdoll_bone,
                    msec: l.msec,
                    base: (inv * (pl - pb), (inv * ql).normalize()),
                }
            })
            .collect();
        s.drawn = f.world.clone();
        let cur = s.cur;
        let mut rec = std::mem::take(&mut s.snaps[cur]);
        self.lerp_bones(i, &mut rec);
        let s = &mut self.slots[i];
        s.snaps[cur] = rec;
        s.drawn = compose(s);
    }

    /// 0x5bbf90: each base lerp bone eased from where it was drawn last,
    /// under its ragdoll bone, towards its base pose by `msec / its
    /// msec`, and set under that bone's pose in `rec`.
    fn lerp_bones(&self, i: usize, rec: &mut Vec<Record>) {
        let s = &self.slots[i];
        let Some(f) = &s.fed else { return };
        let n = s.bones.len();
        rec.truncate(n);
        for (k, l) in s.lerps.iter().enumerate() {
            let wgt = if l.msec > 0 {
                ((s.msec as f64 / l.msec as f64) as f32).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let (ai, bi) = (f.lerp[k], f.p0[l.bone]);
            let (Some(&(ap, aq)), Some(&(bp, bq))) = (s.drawn.get(ai), s.drawn.get(bi)) else {
                return;
            };
            if ai == bi {
                return;
            }
            let inv = bq.inverse();
            let (rt, rq) = (inv * (ap - bp), (inv * aq).normalize());
            let (t, q) = if rt.length() > LERP_JUMP {
                l.base
            } else {
                let q = Quat::from_vec4(
                    glam::Vec4::from(rq)
                        + (glam::Vec4::from(l.base.1) - glam::Vec4::from(rq)) * wgt,
                );
                (rt + (l.base.0 - rt) * wgt, q.normalize())
            };
            let Some(b) = rec.get(l.bone).copied() else {
                return;
            };
            let bq = Quat::from_array(b.q);
            rec.push(Record {
                pos: v3(Vec3::from(b.pos) + bq * t),
                q: iwq(bq * q),
            });
        }
    }

    /// State 4's pose (0x5bc4f0): each body's bone root and turn lerped
    /// between where the last frame's steps began and ended
    /// (`Phys_ObjGetInterpolatedState` 0x5a6ec0, `timeNowLerpFrac`), then
    /// the base lerp bones.
    fn save_pose(&mut self, i: usize) {
        let def = self.def.clone();
        let frac = self.pw.clock.frac;
        let mut rec = Vec::with_capacity(def.bones.len() + def.base_lerp.len());
        for (b, sb) in self.slots[i].bones.iter().enumerate() {
            let Some(id) = sb.body else { return };
            let body = self.pw.world.body(id);
            let prev = self.pw.obj(id).expect("a body's object").prev;
            let a = axes_of(&body.r);
            let o0 = Vec3::from(iw::offset(prev.0, &prev.1, sb.offset));
            let o1 = Vec3::from(iw::offset(body.pos, &a, sb.offset));
            let quat =
                |m: &M3| Quat::from_mat3(&Mat3::from_cols(m[0].into(), m[1].into(), m[2].into()));
            let (q0, mut q1) = (quat(&prev.1), quat(&a));
            if q0.dot(q1) < 0.0 {
                q1 = -q1;
            }
            let q = Quat::from_vec4(
                glam::Vec4::from(q0) + (glam::Vec4::from(q1) - glam::Vec4::from(q0)) * frac,
            )
            .normalize();
            let q = if def.bones[b].mirror {
                iw::flip(iwq(q))
            } else {
                iwq(q)
            };
            rec.push(Record {
                pos: v3(o0 + (o1 - o0) * frac),
                q,
            });
        }
        self.lerp_bones(i, &mut rec);
        let s = &mut self.slots[i];
        let cur = s.cur;
        s.snaps[cur] = rec;
        s.drawn = compose(s);
        s.version += 1;
    }

    /// `Ragdoll_UpdateFriction` (0x5be780): every joint's motor force its
    /// friction times `0.9 (1 - t)^2 + 0.1`, `t` the time simulated over
    /// `ragdoll_jointlerp_time`, unclamped.
    fn update_friction(&mut self, i: usize) {
        let s = &self.slots[i];
        let u = (1.0 - s.msec as f64 / self.dvars.jointlerp_ms as f64) as f32;
        let k = (w((w(u) * w(u)) as f32) * 0.9 + 0.1) as f32;
        for (j, ids) in s.joints.iter().enumerate() {
            let lims = &s.limits[j];
            let fmax = |n: usize| lims.get(n).map_or(0.0, |l| (w(l.friction) * w(k)) as f32);
            let stops = |n: usize| lims.get(n).map_or((0.0, 0.0), |l| (l.lo, l.hi));
            match ids {
                [Some(h), None] => {
                    if let Kind::Hinge(h) = &mut self.pw.world.joint_mut(*h).kind {
                        set_limot(&mut h.limot, fmax(0), stops(0));
                    }
                }
                [_, Some(a)] => {
                    if let Kind::AMotor(a) = &mut self.pw.world.joint_mut(*a).kind {
                        for n in 0..3 {
                            set_limot(&mut a.limot[n], fmax(n), stops(n));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// `Ragdoll_ExplosionEvent` (0x5bdc10, cod4-player-anims.md 10.1): each
    /// simulating or idle ragdoll whose root is inside the outer radius
    /// wakes (an idle one rebuilt, 0x5beaa0), its clock restarts, and each
    /// bone whose jittered centre is inside takes its share of
    /// `ragdoll_explode_force` at that centre ([`phys::obj_add_force`]
    /// with world 2's `msecStep`, 0x5bdfe0). How many it reached.
    pub fn explode(&mut self, e: &Explosion, rand: &mut dyn FnMut() -> f32) -> usize {
        let Some(reach) = Reach::of(e) else { return 0 };
        let mut hit = 0;
        let msec_step = self.pw.clock.msec_step;
        for i in 0..SLOTS {
            let s = &self.slots[i];
            if !matches!(s.state, State::Simulating | State::Idle) {
                continue;
            }
            let Some(root) = s.snaps[s.cur].first() else {
                continue;
            };
            if reach
                .level(Vec3::from(root.pos) - e.origin)
                .length_squared()
                >= reach.outer_sq
            {
                continue;
            }
            if s.state == State::Idle && !self.transition(i, State::Simulating, None) {
                continue;
            }
            hit += 1;
            let s = &mut self.slots[i];
            s.msec = 0;
            s.version += 1;
            let share = 1.0 / s.bones.len() as f32;
            for sb in &s.bones {
                let Some(id) = sb.body else { continue };
                let body = self.pw.world.body_mut(id);
                // the jitter's three draws come before the bone's range test
                let jitter = Vec3::new(rand(), rand(), rand());
                let at = Vec3::from(body.pos) + jitter;
                let off = at - e.origin;
                let Some(falloff) = reach.falloff(off.length_squared()) else {
                    continue;
                };
                let mut heading = if e.impulse == Vec3::ZERO {
                    reach.level(off).normalize_or_zero()
                } else {
                    e.impulse
                };
                heading.z += self.dvars.explode_upbias;
                let force = e.scale * falloff * self.dvars.explode_force * share;
                let f = v3(heading.normalize_or_zero() * force);
                phys::obj_add_force(body, msec_step, v3(at), f);
            }
        }
        hit
    }

    /// The slot's corpse as drawn: every skeleton bone in the world, once
    /// it simulates or holds its pose.
    pub fn pose(&self, id: SlotId) -> Option<&[(Vec3, Quat)]> {
        let s = self.slot(id)?;
        (matches!(s.state, State::Simulating | State::Idle) && !s.drawn.is_empty())
            .then_some(s.drawn.as_slice())
    }

    /// Changes whenever [`Self::pose`] may.
    pub fn version(&self, id: SlotId) -> u64 {
        self.slot(id).map_or(0, |s| s.version)
    }

    /// The ms the slot has spent in its state.
    pub fn msec(&self, id: SlotId) -> i32 {
        self.slot(id).map_or(0, |s| s.msec)
    }

    /// The slot's bodies in def order: `None` for one it has not (or no
    /// longer) built.
    pub fn bodies(&self, id: SlotId) -> Vec<Option<&crate::ode::Body>> {
        self.slot(id).map_or_else(Vec::new, |s| {
            s.bones
                .iter()
                .map(|b| b.body.map(|id| self.pw.world.body(id)))
                .collect()
        })
    }

    /// Each def bone's capsule as it stands: the segment's ends (the bone's
    /// root and its end along the body's x) and its radius.
    pub fn capsules(&self, id: SlotId) -> Vec<(Vec3, Vec3, f32)> {
        let Some(s) = self.slot(id) else {
            return Vec::new();
        };
        s.bones
            .iter()
            .enumerate()
            .filter_map(|(k, b)| {
                let body = self.pw.world.body(b.body?);
                let [a, e] = bone_segment(body, b.offset, b.len);
                Some((Vec3::from(a), Vec3::from(e), self.def.bones[k].radius))
            })
            .collect()
    }

    /// Each def bone's capsule ends and its body's turn (IW's frame, its
    /// x along the bone), in def order.
    pub fn frames(&self, id: SlotId) -> Vec<(Vec3, Vec3, Quat)> {
        let Some(s) = self.slot(id) else {
            return Vec::new();
        };
        s.bones
            .iter()
            .filter_map(|b| {
                let body = self.pw.world.body(b.body?);
                let [a, e] = bone_segment(body, b.offset, b.len);
                let [qw, qx, qy, qz] = body.q;
                Some((
                    Vec3::from(a),
                    Vec3::from(e),
                    Quat::from_xyzw(qx, qy, qz, qw),
                ))
            })
            .collect()
    }

    pub fn world(&self) -> &World {
        &self.pw.world
    }

    /// A slot built straight off two snapshots `dt` ms apart, the latest
    /// first (each def bone's, then the base lerp bones'), over `base` (the
    /// skeleton's base pose, the entity at the origin unturned): what the
    /// capture's gate starts from.
    #[cfg(any(test, feature = "testing"))]
    pub fn start_from(
        &mut self,
        skel: &Skeleton,
        base: &[(Vec3, Quat)],
        snaps: [Vec<Record>; 2],
        dt: i32,
        clip: &dyn Clip,
    ) -> Option<SlotId> {
        let id = self.create()?;
        let world: Vec<(Vec3, Quat)> = base.to_vec();
        self.feed(
            id,
            &DObj {
                skel,
                base,
                origin: Vec3::ZERO,
                turn: Quat::IDENTITY,
                world: &world,
            },
        );
        self.slots[id.index].fed.as_ref()?;
        let [cur, prev] = snaps;
        let s = &mut self.slots[id.index];
        s.cur = 0;
        s.snaps = [cur, prev];
        s.dt = dt;
        s.state = State::Snapshots;
        self.transition(id.index, State::Build, Some(clip));
        Some(id)
    }

    /// World 2's reorder seed, as a capture had it.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_seed(&mut self, seed: u32) {
        self.pw.world.seed = seed;
    }

    /// A body of the slot, to set it as a capture had it.
    #[cfg(any(test, feature = "testing"))]
    pub fn body_mut(&mut self, id: SlotId, bone: usize) -> Option<&mut crate::ode::Body> {
        let b = self.slot(id)?.bones.get(bone)?.body?;
        Some(self.pw.world.body_mut(b))
    }
}

/// World 2's hook (0x5bd9c0, run by `Phys_RunFrame` before the space's
/// collide): each building or simulating slot's self pairs.
fn self_collide(slots: &[Slot], def: &RagdollDef, contact: &collide::Dvars, pw: &mut PhysWorld) {
    for s in slots {
        if !matches!(s.state, State::Build | State::Simulating) {
            continue;
        }
        for &(a, b) in &def.self_pairs {
            let (Some(ba), Some(bb)) = (s.bones[a].body, s.bones[b].body) else {
                continue;
            };
            let bone = |k: usize, id: BodyId| {
                let sb = &s.bones[k];
                selfcol::Bone {
                    body: id,
                    seg: bone_segment(pw.world.body(id), sb.offset, sb.len),
                    radius: def.bones[k].radius,
                    friction: def.bones[k].friction,
                }
            };
            let (bone_a, bone_b) = (bone(a, ba), bone(b, bb));
            let obj = |id: BodyId| pw.obj(id).expect("a body's object").obj;
            let (mut oa, mut ob) = (obj(ba), obj(bb));
            selfcol::self_pair(&mut pw.world, &bone_a, &mut oa, &bone_b, &mut ob, contact);
            pw.obj_mut(ba).expect("a body's object").obj = oa;
            pw.obj_mut(bb).expect("a body's object").obj = ob;
        }
    }
}

/// 0x5bc5b0: each def bone's and base lerp bone's pose now, its bone's
/// origin and raw quaternion.
fn snapshot(f: &Fed) -> Vec<Record> {
    f.p0.iter()
        .chain(&f.lerp)
        .map(|&b| Record {
            pos: v3(f.world[b].0),
            q: iwq(f.world[b].1),
        })
        .collect()
}

/// Every skeleton bone in the world: the def bones' and base lerp bones'
/// as the slot's latest pose has them (0x5bb580), the rest under their
/// parents as the corpse's DObj last had them.
fn compose(s: &Slot) -> Vec<(Vec3, Quat)> {
    let Some(f) = &s.fed else { return Vec::new() };
    let rec = &s.snaps[s.cur];
    let mut set: Vec<Option<(Vec3, Quat)>> = vec![None; f.world.len()];
    for (k, &b) in f.p0.iter().chain(&f.lerp).enumerate() {
        if let Some(r) = rec.get(k) {
            set[b] = Some((Vec3::from(r.pos), Quat::from_array(r.q).normalize()));
        }
    }
    let mut out: Vec<(Vec3, Quat)> = Vec::with_capacity(f.world.len());
    for b in 0..f.world.len() {
        let p = match set[b] {
            Some(p) => p,
            None => match f.parents[b] {
                p if p >= 0 => {
                    let (pp, pr) = out[p as usize];
                    let (lp, lr) = f.local[b];
                    (pp + pr * lp, (pr * lr).normalize())
                }
                _ => f.world[b],
            },
        };
        out.push(p);
    }
    out
}

/// A limit motor as 0x5a70c0 and 0x5a72f0 set it: the stops, speed 0, the
/// force, fudge 0.9 and `phys_joint_cfm`, `_stop_cfm`, `_stop_erp`.
fn set_limot(l: &mut LimitMotor, fmax: f32, (lo, hi): (f32, f32)) {
    l.set(Param::LoStop, lo);
    l.set(Param::HiStop, hi);
    l.set(Param::Vel, 0.0);
    l.set(Param::FMax, fmax);
    l.set(Param::FudgeFactor, FUDGE);
    l.set(Param::Cfm, JOINT_CFM);
    l.set(Param::StopCfm, JOINT_STOP_CFM);
    l.set(Param::StopErp, JOINT_STOP_ERP);
}

/// `phys_joint_cfm`, `phys_joint_stop_cfm` and `phys_joint_stop_erp`'s
/// registered defaults (0x5a3d2c..0x5a3f3e).
const JOINT_CFM: f32 = 1e-4;
const JOINT_STOP_CFM: f32 = 1e-4;
const JOINT_STOP_ERP: f32 = 0.8;
