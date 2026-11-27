//! A map's dynamic entities on the client: the physics props of the clip
//! map's `DynEntityDef` lists, which iw3mp.exe keeps in its own arrays
//! (0x14099b4..0x14099d4), draws, pushes with blasts (0x48e670) and
//! bullets (0x48e0f0) on physics world 0, and breaks (0x48de30). Nothing
//! of them is on the wire. Retail's model and where this one differs:
//! cod4-world.md 13.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Quat, Vec3};
use vcod_common::clipmap::ClipTrace;
use vcod_common::rng::MsvcRand;

use crate::assets::Assets;
use crate::assets::model::{XModel, XModelCollSurf};
use crate::ode::collide::{self, Impact, JitterRegion};
use crate::ode::math::M3;
use crate::ode::obj::{PhysModel, Presets};
use crate::ode::phys::{self, Clip, PhysWorld, WorldId};
use crate::ode::world::{BodyId, PhysDvars};
use crate::ragdoll::Explosion;

/// `DynEntityType`s (the table at 0x6bdb70): `clutter` and `destruct`.
pub const CLUTTER: u32 = 1;
pub const DESTRUCT: u32 = 2;

/// The contents a blast's and a round's dynent walks take (0x48e592,
/// 0x48e1d0).
pub const MASK: u32 = 0x80_2013;

/// `player_meleeRange`, `..Width` and `..Height` at their 1.7 defaults
/// (64, 10, 10; the dvars at 0x7323ec, 0x734b28 and 0x7322a0, read by
/// the swing's rays at 0x48e439..0x48e4d5).
const MELEE_RANGE: f32 = 64.0;
const MELEE_WIDTH: f32 = 10.0;
const MELEE_HEIGHT: f32 = 10.0;
/// Where the swing's five rays end, in widths right and heights up: the
/// rodata pairs at 0x6d3cc8 (the server's at 0x82173e0 hold the same).
const MELEE_OFFSETS: [[f32; 2]; 5] = [
    [0.0, 0.0],
    [1.0, 1.0],
    [-1.0, 1.0],
    [1.0, -1.0],
    [-1.0, -1.0],
];
/// The jitter's candidate box half size over its radius (the double at
/// 0x70ae78, a float's `sqrt 2`).
const JITTER_REACH: f64 = 1.414_213_538_169_860_8;

/// One step of the shared MSVC LCG on a bare state; returns the new state.
fn step_state(state: &mut u32) -> u32 {
    *state = MsvcRand::seed(*state).step();
    *state
}

/// What the type table (0x6bdb70, 8 bytes a type) says of `kind`: whether
/// it starts live for the walks (byte 4, client flag 1, 0x48d088), takes
/// pushes (byte 6, 0x48e967, 0x48e2f7) and takes damage (byte 7,
/// 0x48eaac, 0x48e359).
fn traits(kind: u32) -> (bool, bool, bool) {
    match kind {
        CLUTTER => (true, true, false),
        DESTRUCT => (true, true, true),
        _ => (false, false, false),
    }
}

/// One `DynEntityDef` (0x60) as the client uses it.
#[derive(Clone, Debug)]
pub struct DynDef {
    /// `type` (+0).
    pub kind: u32,
    /// `xModel` (+0x20), as the zone names it.
    pub model: String,
    /// `pose` (+4): a quaternion (x, y, z, w) and the origin.
    pub rot: Quat,
    pub origin: Vec3,
    /// The model's `radius` (`XModel` +0xa8): the pose's cube's half size
    /// the walks test (0x48c9d0).
    pub radius: f32,
    /// The model's collision a round meets (0x490020), `None` when its
    /// `collLod` is below 0.
    pub coll: Option<Arc<Vec<XModelCollSurf>>>,
    /// `destroyFx` (+0x28) by name.
    pub destroy_fx: Option<String>,
    /// `health` (+0x34).
    pub health: i32,
    /// `contents` (+0x5c).
    pub contents: u32,
    /// The model and `physPreset` (+0x30) its body is made with; `None`
    /// when it names no preset any zone holds.
    pub body: Option<Arc<PhysModel>>,
}

impl DynDef {
    /// The preset's `explosiveForceScale` (+0x18), 0 with none.
    pub fn explosive_force_scale(&self) -> f32 {
        self.body
            .as_ref()
            .map_or(0.0, |m| m.preset.explosive_force_scale)
    }
}

/// Every dynamic entity of `map`'s clip map, list 0 (the model ones): the
/// presets and models it names resolved through `map` and then `refs`. A
/// stock map's list 1 (the brush ones) is empty.
pub fn defs(map: &Assets, refs: &[&Assets]) -> Vec<DynDef> {
    let Some(clip) = map
        .clip_maps_pvs
        .first()
        .or(map.clip_maps.first().map(|c| &c.0))
    else {
        return Vec::new();
    };
    if !clip.dyn_ents[1].is_empty() {
        log::warn!(
            "cod4: {} brush dynamic entities left out",
            clip.dyn_ents[1].len()
        );
    }
    let zones: Vec<&Assets> = std::iter::once(map).chain(refs.iter().copied()).collect();
    let presets = Presets::of(&zones);
    // A zone holding only a stub (`,name`) of a model leaves its body to
    // the next.
    let whole = |name: &str| -> Option<&XModel> {
        zones
            .iter()
            .find_map(|z| z.xmodels.iter().find(|x| x.name == name))
    };
    let mut models: HashMap<(String, String), Arc<PhysModel>> = HashMap::new();
    let mut colls: HashMap<String, Option<Arc<Vec<XModelCollSurf>>>> = HashMap::new();
    let mut out = Vec::new();
    for d in &clip.dyn_ents[0] {
        let f = |o: usize| f32::from_le_bytes(d.raw[o..o + 4].try_into().unwrap());
        let u = |o: usize| u32::from_le_bytes(d.raw[o..o + 4].try_into().unwrap());
        let Some(m) = d.model else { continue };
        let name = map.xmodels[m.index()]
            .name
            .trim_start_matches(',')
            .to_string();
        let Some(x) = whole(&name) else {
            log::debug!("cod4: dynent model {name}: no zone holds it");
            continue;
        };
        let body = d.preset.and_then(|p| {
            let pname = &map.phys_presets[p.index()].name;
            let preset = presets.get(pname)?;
            let key = (name.clone(), pname.to_ascii_lowercase());
            Some(
                models
                    .entry(key)
                    .or_insert_with(|| Arc::new(PhysModel::new(x, *preset)))
                    .clone(),
            )
        });
        let coll = colls
            .entry(name.clone())
            .or_insert_with(|| (x.coll_lod >= 0).then(|| Arc::new(x.coll.clone())))
            .clone();
        out.push(DynDef {
            kind: u(0),
            model: name,
            rot: Quat::from_xyzw(f(4), f(8), f(12), f(16)),
            origin: Vec3::new(f(20), f(24), f(28)),
            radius: x.radius,
            coll,
            destroy_fx: d.destroy_fx.map(|i| map.fx[i.index()].name.clone()),
            health: u(0x34) as i32,
            contents: u(0x5c),
            body,
        });
    }
    out
}

/// The model names of `a`'s dynamic entities (list 0), each once.
pub fn model_names(a: &Assets) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in a
        .clip_maps_pvs
        .iter()
        .chain(a.clip_maps.iter().map(|c| &c.0))
    {
        for m in c.dyn_ents[0].iter().filter_map(|d| d.model) {
            let name = a.xmodels[m.index()].name.trim_start_matches(',');
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The `dynEnt_*` dvars at their 1.7 defaults (0x48c1e0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dvars {
    /// `dynEnt_active` (1).
    pub active: bool,
    /// `dynEnt_bulletForce` (1000): a round's push speed.
    pub bullet_force: f32,
    /// `dynEnt_explodeForce` (12500).
    pub explode_force: f32,
    /// `dynEnt_explodeUpbias` (0.5).
    pub explode_upbias: f32,
    /// `dynEnt_explodeSpinScale` (3): the push point's random offset off
    /// the centre of mass, per axis.
    pub explode_spin_scale: f32,
    /// `dynEnt_explodeMinForce` (40).
    pub explode_min_force: f32,
    /// `dynEnt_explodeMaxEnts` (20).
    pub explode_max_ents: usize,
}

impl Default for Dvars {
    fn default() -> Self {
        Dvars {
            active: true,
            bullet_force: 1000.0,
            explode_force: 12500.0,
            explode_upbias: 0.5,
            explode_spin_scale: 3.0,
            explode_min_force: 40.0,
            explode_max_ents: 20,
        }
    }
}

/// One dynamic entity's state (`DynEntityPose`, 0x20, and
/// `DynEntityClient`, 0xC).
#[derive(Clone, Debug)]
pub struct DynEnt {
    pub rot: Quat,
    pub origin: Vec3,
    /// Client flag 1: the walks see it.
    pub live: bool,
    /// Client flag 2: it draws.
    pub drawn: bool,
    pub health: i32,
    /// Its body in world 0, made on its first push and dropped once it
    /// sleeps (0x48d176).
    pub body: Option<BodyId>,
}

/// A dynamic entity broken: its `destroyFx` to play at its pose (0x48dec7).
#[derive(Clone, Debug, PartialEq)]
pub struct Broken {
    pub index: usize,
    pub fx: Option<String>,
    pub origin: Vec3,
    /// The pose's axis (0x561100).
    pub axis: [Vec3; 3],
}

/// Where a round met a dynamic entity's model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Struck {
    pub index: usize,
    pub at: Vec3,
    pub normal: Vec3,
    pub surface_flags: u32,
}

/// A map's dynamic entities and physics world 0, which steps their bodies.
pub struct DynEnts {
    pub defs: Arc<Vec<DynDef>>,
    pub ents: Vec<DynEnt>,
    pub dvars: Dvars,
    pub phys: PhysDvars,
    pub contact: collide::Dvars,
    /// The blast walk's random state (0x7209a8), which retail shares with
    /// other callers.
    pub rand: u32,
    pub pw: PhysWorld,
    /// Whether world 0's clock runs ([`PhysWorld::set_time`]).
    started: bool,
    /// The impact sounds world 0's steps played since the owner last took
    /// them.
    pub impacts: Vec<Impact>,
}

/// A pose quaternion's axes (0x561100), the phys object's frame.
fn quat_axes(q: Quat) -> M3 {
    let m = glam::Mat3::from_quat(q);
    [m.x_axis.into(), m.y_axis.into(), m.z_axis.into()]
}

impl DynEnts {
    /// `DynEntCl_InitEntities` (0x48d000): each at its def's pose, live
    /// and drawn by its type, at its def's health.
    pub fn new(defs: Arc<Vec<DynDef>>) -> DynEnts {
        let ents = defs
            .iter()
            .map(|d| DynEnt {
                rot: d.rot,
                origin: d.origin,
                live: traits(d.kind).0,
                drawn: true,
                health: d.health,
                body: None,
            })
            .collect();
        DynEnts {
            defs,
            ents,
            dvars: Dvars::default(),
            phys: PhysDvars::default(),
            contact: collide::Dvars::default(),
            rand: 0,
            pw: PhysWorld::new(WorldId::Dynent),
            started: false,
            impacts: Vec::new(),
        }
    }

    fn start(&mut self, now: i32) {
        if !self.started {
            self.pw.set_time(now);
            self.started = true;
        }
    }

    /// `DynEntCl_CreatePhysObj` (0x48dca0): a body in world 0 at the pose,
    /// at rest, with its model's collision (0x490240). Whether it has one.
    fn make_body(&mut self, i: usize, now: i32) -> bool {
        if self.ents[i].body.is_some() {
            return true;
        }
        let Some(m) = self.defs[i].body.clone() else {
            return false;
        };
        self.start(now);
        let e = &self.ents[i];
        let (pos, axes) = (e.origin.to_array(), quat_axes(e.rot));
        let id = self.pw.create_from(&m, pos, &axes, true, &self.phys);
        if id.is_none() {
            log::debug!("DynEntCl_CreatePhysObj: Unable to create physic object.");
        }
        self.ents[i].body = id;
        id.is_some()
    }

    /// Squared distance from `p` to entity `i`'s cube, level for a
    /// cylinder (0x48fd50, 0x48fe80).
    fn dist_sq(&self, i: usize, p: Vec3, level: bool) -> f32 {
        let (o, r) = (self.ents[i].origin, self.defs[i].radius);
        let mut d = Vec3::ZERO;
        for k in 0..3 {
            d[k] = (o[k] - r - p[k]).max(0.0) + (p[k] - o[k] - r).max(0.0);
        }
        if level {
            d.z = 0.0;
        }
        d.length_squared()
    }

    /// One step of the shared MSVC LCG on `rand`; returns the new state.
    fn step_rand(&mut self) -> u32 {
        step_state(&mut self.rand)
    }

    fn next_rand(&mut self) -> f32 {
        // Bits 17..31 of the state, as retail's `holdrand` draws.
        (self.step_rand() >> 17) as f32 * 2.0 / 32768.0 - 1.0
    }

    /// `DynEntCl_ExplosionEvent` (0x48e670): `e` pushes every live entity
    /// whose cube it reaches, the nearest `dynEnt_explodeMaxEnts` when more
    /// do, and damages the destructible ones from `damage[0]` (inner) to
    /// `damage[1]` (outer) at `now`. Returns those it broke.
    pub fn explode(&mut self, e: &Explosion, damage: [i32; 2], now: i32) -> Vec<Broken> {
        let mut broken = Vec::new();
        if !self.dvars.active || e.outer == 0.0 {
            return broken;
        }
        let (outer_sq, inner_sq) = (e.outer * e.outer, e.inner * e.inner);
        let inv_band = if inner_sq < outer_sq {
            1.0 / (e.inner - e.outer)
        } else {
            0.0
        };
        let mut hit: Vec<(usize, f32)> = (0..self.ents.len())
            .filter(|&i| self.ents[i].live && self.defs[i].contents & MASK != 0)
            .map(|i| (i, self.dist_sq(i, e.origin, e.cylinder)))
            .filter(|&(_, d)| d < outer_sq)
            .collect();
        if hit.len() > self.dvars.explode_max_ents {
            hit.sort_by(|a, b| a.1.total_cmp(&b.1));
            hit.truncate(self.dvars.explode_max_ents);
        }
        for (i, d_sq) in hit {
            let mut scale = e.scale;
            if inner_sq < d_sq {
                scale = (d_sq.sqrt() - e.outer) * inv_band * e.scale;
            }
            let force = self.defs[i].explosive_force_scale() * scale * self.dvars.explode_force;
            let dir = if e.impulse == Vec3::ZERO {
                if force < self.dvars.explode_min_force {
                    continue;
                }
                let mut d = self.ents[i].origin - e.origin;
                if e.cylinder {
                    d.z = 0.0;
                }
                let mut d = d.normalize_or_zero();
                d.z += self.dvars.explode_upbias;
                d.normalize_or_zero()
            } else {
                e.impulse
            };
            let (pushes, breaks) = {
                let t = traits(self.defs[i].kind);
                (t.1, t.2)
            };
            if pushes && self.make_body(i, now) {
                let r = Vec3::new(self.next_rand(), self.next_rand(), self.next_rand());
                let spin = self.dvars.explode_spin_scale;
                let msec = self.pw.clock.msec_step;
                let b = self.pw.world.body_mut(self.ents[i].body.unwrap());
                let at = Vec3::from(b.pos) + r * spin;
                // World 0's `msecStep` (`esi` 0 into 0x5a5900 at 0x48eaa1).
                phys::obj_add_force(b, msec, at.into(), (dir * force).into());
            }
            if breaks {
                let dmg = ((damage[0] - damage[1]) as f32 * scale + damage[1] as f32) as i32;
                if dmg != 0 {
                    broken.extend(self.damage(i, dmg));
                }
            }
        }
        broken
    }

    /// The live entity a round from `start` to `end` meets first
    /// (`DynEntCl_ClipEntities` through 0x48d6c0 under [`MASK`]): its
    /// cube's sphere first, then its model's collision in its frame.
    pub fn trace(&self, start: Vec3, end: Vec3) -> Option<Struck> {
        let mut tr = ClipTrace {
            fraction: 1.0,
            normal: Vec3::ZERO,
            surface_flags: 0,
            contents: 0,
            material: None,
            allsolid: false,
            startsolid: false,
            walkable: false,
        };
        let mut best = None;
        let seg = end - start;
        for (i, (e, d)) in self.ents.iter().zip(self.defs.iter()).enumerate() {
            if !e.live || d.contents & MASK == 0 {
                continue;
            }
            let Some(coll) = &d.coll else { continue };
            // The ray against the sphere about its origin (0x4f6e70).
            let to = e.origin - start;
            let t = (to.dot(seg) / seg.length_squared().max(1e-6)).clamp(0.0, tr.fraction);
            if (start + seg * t).distance_squared(e.origin) > d.radius * d.radius {
                continue;
            }
            let inv = e.rot.inverse();
            let local = |p: Vec3| inv * (p - e.origin);
            if crate::collision::trace_line(coll, local(start), local(end), MASK, &mut tr) {
                tr.normal = e.rot * tr.normal;
                best = Some(Struck {
                    index: i,
                    at: start + seg * tr.fraction,
                    normal: tr.normal,
                    surface_flags: tr.surface_flags,
                });
            }
        }
        best
    }

    /// `DynEntCl_EntityImpactEvent` (0x48e0f0): a round from `start` to
    /// `end` of a weapon doing `damage` pushes the first entity it meets
    /// by `Phys_ObjBulletImpact` at `dynEnt_bulletForce` and the preset's
    /// `bulletForceScale`, and damages a destructible one. Returns what it
    /// met and what broke.
    pub fn bullet(
        &mut self,
        start: Vec3,
        end: Vec3,
        damage: i32,
        now: i32,
    ) -> (Option<Struck>, Vec<Broken>) {
        if !self.dvars.active {
            return (None, Vec::new());
        }
        let Some(s) = self.trace(start, end) else {
            return (None, Vec::new());
        };
        let i = s.index;
        let (_, pushes, breaks) = traits(self.defs[i].kind);
        if pushes && self.make_body(i, now) {
            let scale = self.defs[i]
                .body
                .as_ref()
                .map_or(0.0, |m| m.preset.bullet_force_scale);
            let dir = phys::normalize((end - start).into()).0;
            let force = self.dvars.bullet_force;
            let msec = self.pw.clock.msec_step;
            let b = self.pw.world.body_mut(self.ents[i].body.unwrap());
            // World 0 (the `push 0` into 0x5a5a20 at 0x48e34f).
            phys::obj_bullet_impact(b, msec, s.at.into(), dir, force, scale, &self.phys);
        }
        let mut broken = Vec::new();
        if breaks && damage != 0 {
            broken.extend(self.damage(i, damage));
        }
        (Some(s), broken)
    }

    /// The knife's swing on the dynamic entities (0x48e3a0, from
    /// `EV_FIRE_MELEE` at 0x4367a9): five rays from the swinger's `eye`
    /// along `view` (forward, right, up) to `player_meleeRange` ahead and
    /// the corners of a `..Width` by `..Height` box, each through
    /// [`Self::bullet`] with the weapon's `meleeDamage` until one meets an
    /// entity. Its contact plays `melee_knife_hit_other` there (0x48e2c8)
    /// in place of a round's impact.
    pub fn melee(
        &mut self,
        eye: Vec3,
        [forward, right, up]: [Vec3; 3],
        damage: i32,
        now: i32,
    ) -> (Option<Struck>, Vec<Broken>) {
        let reach = eye + forward * MELEE_RANGE;
        for [x, y] in MELEE_OFFSETS {
            let end = reach + right * (x * MELEE_WIDTH) + up * (y * MELEE_HEIGHT);
            let (s, b) = self.bullet(eye, end, damage, now);
            if s.is_some() {
                return (s, b);
            }
        }
        (None, Vec::new())
    }

    /// `EV_PHYS_JITTER` (0x48eb50, from `CG_EntityEvent` at 0x4373ab): a
    /// body for every live entity that takes pushes and whose cube meets
    /// the box `origin +- outer sqrt 2`, unbounded in z, the nearest
    /// `dynEnt_explodeMaxEnts` of them when more do (0x48db30, 0x48ef40);
    /// then the region held for world 0's next step
    /// ([`PhysWorld::add_jitter`], 0x5a7730).
    pub fn jitter(&mut self, j: JitterRegion, now: i32) {
        if !self.dvars.active || j.outer == 0.0 {
            return;
        }
        let o = Vec3::from(j.origin);
        let reach = (f64::from(j.outer) * JITTER_REACH) as f32;
        let meets =
            |p: Vec3, r: f32| (p.x - o.x).abs() <= r + reach && (p.y - o.y).abs() <= r + reach;
        let mut hit: Vec<(usize, f32)> = (0..self.ents.len())
            .filter(|&i| self.ents[i].live && self.defs[i].contents & MASK != 0)
            .filter(|&i| meets(self.ents[i].origin, self.defs[i].radius))
            .map(|i| (i, self.dist_sq(i, o, true)))
            .collect();
        if hit.len() > self.dvars.explode_max_ents {
            hit.sort_by(|a, b| a.1.total_cmp(&b.1));
            hit.truncate(self.dvars.explode_max_ents);
        }
        for (i, _) in hit {
            if traits(self.defs[i].kind).1 {
                self.make_body(i, now);
            }
        }
        self.pw.add_jitter(j);
    }

    /// `DynEntCl_EntityDamage` (0x48de30): `dmg` off entity `i`'s health;
    /// at 0 or below it stops drawing and meeting anything, its body goes,
    /// and its `destroyFx` plays at its pose.
    fn damage(&mut self, i: usize, dmg: i32) -> Option<Broken> {
        let e = &mut self.ents[i];
        e.health -= dmg;
        if e.health > 0 {
            return None;
        }
        e.live = false;
        e.drawn = false;
        if let Some(id) = e.body.take() {
            self.pw.destroy_body(id);
        }
        let e = &self.ents[i];
        // 0x48de98: neither an effect nor pieces, nothing more.
        let d = &self.defs[i];
        d.destroy_fx.as_ref()?;
        let m = glam::Mat3::from_quat(e.rot);
        Some(Broken {
            index: i,
            fx: d.destroy_fx.clone(),
            origin: e.origin,
            axis: [m.x_axis, m.y_axis, m.z_axis],
        })
    }

    /// `DynEntCl_ProcessEntities` (0x48d110): each entity with a body
    /// posed at its body's drawn state, the body dropped once it sleeps,
    /// then world 0 run to `time` against `clip` (`Phys_RunToTime`), its
    /// jitter drawing on `rand`, the CRT's stream.
    pub fn advance(&mut self, time: i32, clip: &dyn Clip, rand: &mut MsvcRand) {
        for e in &mut self.ents {
            let Some(id) = e.body else { continue };
            if let Some((origin, rot)) = self.pw.pose(id) {
                (e.origin, e.rot) = (origin, rot);
            }
            if self.pw.asleep(id) {
                self.pw.destroy_body(id);
                e.body = None;
            }
        }
        self.start(time);
        std::mem::swap(&mut self.pw.rand, rand);
        let (phys, contact) = (self.phys, self.contact);
        self.pw
            .run_to_time(time, &phys, &contact, clip, &mut |_| {});
        std::mem::swap(&mut self.pw.rand, rand);
        self.impacts.append(&mut self.pw.impacts);
    }

    /// World 0's `msecStep`, which a push naming world 0 spreads over.
    pub fn msec_step(&self) -> i32 {
        self.pw.clock.msec_step
    }

    /// How many bodies are awake.
    pub fn awake(&self) -> usize {
        self.ents.iter().filter(|e| e.body.is_some()).count()
    }

    /// Every entity that draws: its def's model and its pose.
    pub fn drawn(&self) -> impl Iterator<Item = (&DynDef, Vec3, Quat)> + '_ {
        self.ents
            .iter()
            .zip(self.defs.iter())
            .filter(|(e, _)| e.drawn)
            .map(|(e, d)| (d, e.origin, e.rot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ode::collide::tests::{Brushes, cuboid};
    use crate::ode::obj::{ObjPreset, Preset};

    /// A clutter can at `at`: a 4-unit box, 1 unit of mass, explosive
    /// force scale 0.01 (125 at full force).
    fn can(at: Vec3) -> DynDef {
        let model = XModel {
            name: "can".into(),
            mins: [-2.0; 3],
            maxs: [2.0; 3],
            ..XModel::default()
        };
        let preset = Preset {
            obj: ObjPreset {
                sound: 0,
                mass: 1.0,
                bounce: 0.1,
                friction: 0.5,
                cylinder: false,
            },
            bullet_force_scale: 0.5,
            explosive_force_scale: 0.01,
        };
        DynDef {
            kind: CLUTTER,
            model: "can".into(),
            rot: Quat::IDENTITY,
            origin: at,
            radius: 2.0,
            coll: None,
            destroy_fx: None,
            health: 0,
            contents: 1,
            body: Some(Arc::new(PhysModel::new(&model, preset))),
        }
    }

    fn blast(outer: f32, inner: f32) -> Explosion {
        Explosion {
            origin: Vec3::ZERO,
            inner,
            outer,
            cylinder: false,
            impulse: Vec3::ZERO,
            scale: 1.0,
        }
    }

    /// Entity `i`'s body.
    fn body(d: &DynEnts, i: usize) -> &crate::ode::Body {
        d.pw.world.body(d.ents[i].body.unwrap())
    }

    /// A floor brush, its top at z 0.
    fn floor() -> Brushes {
        Brushes(vec![cuboid(
            [-1024.0, -1024.0, -32.0],
            [1024.0, 1024.0, 0.0],
        )])
    }

    #[test]
    fn a_blast_falls_off_from_inner_to_outer_and_tips_up() {
        // 50 off the edge of a 0..200 blast: falloff 150/200 of 125.
        let defs = vec![can(Vec3::X * 52.0), can(Vec3::X * 300.0)];
        let mut d = DynEnts::new(Arc::new(defs));
        d.dvars.explode_spin_scale = 0.0;
        assert!(d.explode(&blast(200.0, 0.0), [0, 0], 0).is_empty());
        assert_eq!(d.awake(), 1, "the far can woke");
        // Each push a force at world 0's 17 ms (`Phys_ObjAddForce`).
        let spent = |d: &DynEnts| Vec3::from(body(d, 0).facc) * 0.017;
        let v = spent(&d);
        let want = Vec3::new(1.0, 0.0, 0.5).normalize() * (125.0 * 150.0 / 200.0);
        assert!(v.distance(want) < 1e-3, "{v} {want}");
        // Inside the inner radius the whole force.
        let mut d = DynEnts::new(Arc::new(vec![can(Vec3::X * 52.0)]));
        d.dvars.explode_spin_scale = 0.0;
        d.explode(&blast(200.0, 100.0), [0, 0], 0);
        let v = spent(&d);
        assert!((v.length() - 125.0).abs() < 1e-3, "{v}");
    }

    #[test]
    fn a_blast_wakes_the_nearest_twenty_and_none_under_the_minimum_force() {
        let defs: Vec<DynDef> = (0..30)
            .map(|i| can(Vec3::X * (10.0 + 4.0 * i as f32)))
            .collect();
        let mut d = DynEnts::new(Arc::new(defs));
        d.explode(&blast(400.0, 0.0), [0, 0], 0);
        assert_eq!(d.awake(), 20);
        assert_eq!(d.pw.bodies(), 20);
        assert!(d.ents[..20].iter().all(|e| e.body.is_some()));
        // 125 times a falloff under 40/125 pushes nothing.
        let mut d = DynEnts::new(Arc::new(vec![can(Vec3::X * 150.0)]));
        d.explode(&blast(200.0, 0.0), [0, 0], 0);
        assert_eq!(d.awake(), 0);
        // A jolt's impulse is its direction, whatever the force.
        let mut e = blast(200.0, 0.0);
        e.impulse = Vec3::Z;
        d.explode(&e, [0, 0], 0);
        assert_eq!(d.awake(), 1);
    }

    #[test]
    fn a_destructible_takes_its_falloff_damage_and_breaks_into_its_effect() {
        let mut plate = can(Vec3::X * 52.0);
        plate.kind = DESTRUCT;
        plate.health = 100;
        plate.destroy_fx = Some("props/plate".into());
        let mut d = DynEnts::new(Arc::new(vec![plate]));
        // 150/200 of the way from 20 to 120: 95, standing at 5.
        assert!(d.explode(&blast(200.0, 0.0), [120, 20], 0).is_empty());
        assert_eq!(d.ents[0].health, 5);
        let b = d.explode(&blast(200.0, 0.0), [120, 20], 0);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].fx.as_deref(), Some("props/plate"));
        assert!(!d.ents[0].drawn && !d.ents[0].live && d.ents[0].body.is_none());
        assert_eq!(d.pw.bodies(), 0, "its body went with it");
        // Gone, a third meets nothing.
        assert!(d.explode(&blast(200.0, 0.0), [120, 20], 0).is_empty());
    }

    /// [`can`] with a face at x -2 in its frame, facing -x, that a round
    /// along +x meets.
    fn walled_can(at: Vec3) -> DynDef {
        let face = [
            -1.0,
            0.0,
            0.0,
            2.0,
            0.0,
            1.0 / 32.0,
            0.0,
            -0.25,
            0.0,
            0.0,
            1.0 / 32.0,
            -0.25,
        ];
        DynDef {
            kind: DESTRUCT,
            health: 100,
            coll: Some(Arc::new(vec![XModelCollSurf {
                tris: vec![face],
                mins: [-3.0, -9.0, -9.0],
                maxs: [-1.0, 25.0, 25.0],
                bone: 0,
                contents: 1,
                surface_flags: 0,
            }])),
            ..can(at)
        }
    }

    #[test]
    fn a_swing_tries_its_five_rays_until_one_meets_a_prop() {
        let view = [Vec3::X, Vec3::Y, Vec3::Z];
        // 40 ahead, the upper right ray's line (64, 10, 10) passes the
        // can's origin; the centre ray passes 8.8 off it.
        let mut d = DynEnts::new(Arc::new(vec![walled_can(Vec3::new(40.0, 6.25, 6.25))]));
        let (s, broken) = d.melee(Vec3::ZERO, view, 135, 0);
        let s = s.expect("the corner ray meets the can");
        assert_eq!(s.index, 0);
        // At the face, less the kernels' skin.
        assert!(s.at.x > 37.8 && s.at.x < 38.0, "{}", s.at);
        // `meleeDamage` 135 breaks it; a push woke it first.
        assert!(broken.is_empty(), "no destroyFx to play");
        assert!(!d.ents[0].live);
        // Past `player_meleeRange` nothing.
        let mut d = DynEnts::new(Arc::new(vec![walled_can(Vec3::X * 70.0)]));
        assert_eq!(d.melee(Vec3::ZERO, view, 135, 0).0, None);
        // Straight ahead the centre ray pushes it along +x and damages it.
        let mut d = DynEnts::new(Arc::new(vec![walled_can(Vec3::X * 40.0)]));
        let (s, _) = d.melee(Vec3::ZERO, view, 50, 0);
        assert!(s.is_some());
        assert_eq!(d.ents[0].health, 50);
        assert!(body(&d, 0).facc[0] > 0.0, "{:?}", body(&d, 0).facc);
    }

    #[test]
    fn a_jitter_lifts_the_props_on_the_ground_in_its_radius() {
        // On the floor, its bottom half a unit into it.
        let near = can(Vec3::new(10.0, 0.0, 1.5));
        let far = can(Vec3::new(500.0, 0.0, 1.5));
        let mut d = DynEnts::new(Arc::new(vec![near, far]));
        let j = JitterRegion::new([0.0; 3], 0.0, 100.0, 1.0, 1.0);
        let mut rand = MsvcRand::default();
        d.jitter(j, 0);
        assert!(d.ents[0].body.is_some(), "a body for the near can");
        assert!(d.ents[1].body.is_none(), "none past the box");
        d.advance(17, &floor(), &mut rand);
        // Each floor contact of a 1-unit mass takes about
        // (1 - 1/200^3) 1000 / 17 straight up, the falloff 0.9 at 10 of
        // 100, as a force at 17 ms: the can leaves the floor.
        let v = body(&d, 0).lvel;
        assert!(v[2] > 50.0, "{v:?}");
        assert!(d.pw.jitter.is_empty(), "the region lasts one step");
        assert_ne!(rand, MsvcRand::default(), "the push drew on the stream");
        // An outer radius of 0 does nothing.
        let mut d = DynEnts::new(Arc::new(vec![can(Vec3::new(10.0, 0.0, 1.5))]));
        d.jitter(JitterRegion { outer: 0.0, ..j }, 0);
        assert_eq!(d.awake(), 0);
        // Without the region the can only settles.
        let mut d = DynEnts::new(Arc::new(vec![can(Vec3::new(10.0, 0.0, 1.5))]));
        d.make_body(0, 0);
        d.advance(17, &floor(), &mut MsvcRand::default());
        assert!(body(&d, 0).lvel[2] < 25.0, "{:?}", body(&d, 0).lvel);
    }

    #[test]
    fn a_body_that_comes_to_rest_is_dropped_where_it_lies() {
        let mut d = DynEnts::new(Arc::new(vec![can(Vec3::Z * 2.0)]));
        d.dvars.explode_spin_scale = 0.0;
        let mut e = blast(200.0, 0.0);
        e.origin = Vec3::new(-30.0, 0.0, 2.0);
        d.explode(&e, [0, 0], 0);
        let (clip, mut rand) = (floor(), MsvcRand::default());
        let mut t = 0;
        while d.awake() > 0 && t < 10_000 {
            t += 16;
            d.advance(t, &clip, &mut rand);
        }
        assert_eq!(d.awake(), 0, "awake at {t}");
        assert_eq!(d.pw.bodies(), 0);
        let e = &d.ents[0];
        assert!(e.origin.x > 10.0, "{}", e.origin);
        // `phys_csl` lets it sink about a unit.
        assert!((0.8..2.0).contains(&e.origin.z), "{}", e.origin);
        assert!(e.drawn && e.live);
    }
}
