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

use crate::assets::Assets;
use crate::assets::model::{XModel, XModelCollSurf};
use crate::physics::{Body, Dvars as PhysDvars, Preset, Shape};
use crate::ragdoll::{Explosion, Sweep};

/// A sweep of a sphere of the given radius from the first point to the
/// second through the world under the given mask.
pub type MaskedTrace<'a> = dyn FnMut(Vec3, Vec3, f32, u32) -> Sweep + 'a;

/// `DynEntityType`s (the table at 0x6bdb70): `clutter` and `destruct`.
pub const CLUTTER: u32 = 1;
pub const DESTRUCT: u32 = 2;

/// The contents a blast's and a round's dynent walks take (0x48e592,
/// 0x48e1d0).
pub const MASK: u32 = 0x80_2013;

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
    /// The shape and `physPreset` (+0x30) its body is made with; `None`
    /// when it names no preset any zone holds.
    pub body: Option<(Arc<Shape>, Preset)>,
    /// The preset's `explosiveForceScale` (+0x18).
    pub explosive_force_scale: f32,
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
    let mut presets: HashMap<String, (Preset, f32)> = HashMap::new();
    for z in &zones {
        for p in z.phys_presets.iter().filter(|p| !p.name.starts_with(',')) {
            let efs = p
                .raw
                .get(24..28)
                .map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap()));
            if let Some(v) = Preset::from_raw(&p.raw) {
                presets
                    .entry(p.name.to_ascii_lowercase())
                    .or_insert((v, efs));
            }
        }
    }
    // A zone holding only a stub (`,name`) of a model leaves its body to
    // the next.
    let whole = |name: &str| -> Option<&XModel> {
        zones
            .iter()
            .find_map(|z| z.xmodels.iter().find(|x| x.name == name))
    };
    let mut shapes: HashMap<String, Arc<Shape>> = HashMap::new();
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
        let preset = d.preset.and_then(|p| {
            let n = map.phys_presets[p.index()].name.trim_start_matches(',');
            presets.get(&n.to_ascii_lowercase()).copied()
        });
        let shape = shapes
            .entry(name.clone())
            .or_insert_with(|| Arc::new(Shape::of_dynent(x)))
            .clone();
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
            body: preset.map(|(p, _)| (shape, p)),
            explosive_force_scale: preset.map_or(0.0, |(_, e)| e),
        });
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
    /// Its body, made on its first push and dropped once it sleeps
    /// (0x48d176).
    pub body: Option<Body>,
    /// The world contents its body passes: those its def's origin lies in
    /// ([`DynEnts::pass_start_contents`]).
    pub pass: u32,
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

/// A map's dynamic entities.
pub struct DynEnts {
    pub defs: Arc<Vec<DynDef>>,
    pub ents: Vec<DynEnt>,
    pub dvars: Dvars,
    pub phys: PhysDvars,
    /// The blast walk's random state (0x7209a8), which retail shares with
    /// other callers.
    pub rand: u32,
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
                pass: 0,
            })
            .collect();
        DynEnts {
            defs,
            ents,
            dvars: Dvars::default(),
            phys: PhysDvars::default(),
            rand: 0,
        }
    }

    /// Lets each body pass the world contents its def's origin lies in
    /// (`contents` a point's): mp_crash's pots stand inside an item clip
    /// brush over their table, in which every point of theirs would start
    /// solid and meet nothing. vcod's; ODE's colliders push a body out.
    pub fn pass_start_contents(&mut self, contents: impl Fn(Vec3) -> u32) {
        for (e, d) in self.ents.iter_mut().zip(self.defs.iter()) {
            e.pass = contents(d.origin) & crate::ragdoll::CLIP_MASK;
        }
    }

    fn make_body(&mut self, i: usize, now: i32) -> bool {
        if self.ents[i].body.is_some() {
            return true;
        }
        let Some((shape, preset)) = &self.defs[i].body else {
            return false;
        };
        let e = &mut self.ents[i];
        e.body = Some(Body::at(e.origin, e.rot, shape, preset, now));
        true
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

    fn next_rand(&mut self) -> f32 {
        self.rand = self.rand.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        (self.rand >> 17) as f32 * 2.0 / 32768.0 - 1.0
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
            let force = self.defs[i].explosive_force_scale * scale * self.dvars.explode_force;
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
                let b = self.ents[i].body.as_mut().unwrap();
                let at = b.pos + r * spin;
                b.push(at, dir * force);
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
                .map_or(0.0, |b| b.1.bullet_force_scale);
            let dir = (end - start).normalize_or_zero();
            let (force, phys) = (self.dvars.bullet_force, self.phys);
            let b = self.ents[i].body.as_mut().unwrap();
            b.launch(s.at, dir * force, scale, &phys);
        }
        let mut broken = Vec::new();
        if breaks && damage != 0 {
            broken.extend(self.damage(i, damage));
        }
        (Some(s), broken)
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
        e.body = None;
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

    /// `DynEntCl_ProcessEntities` (0x48d110) and world 0's
    /// `Phys_RunToTime` at `time`: each body stepped through `trace` (a
    /// sweep under a mask, [`crate::physics::clip_trace_masked`]), its
    /// entity posed where it is drawn, and one asleep dropped where it
    /// lies.
    pub fn advance(&mut self, time: i32, trace: &mut MaskedTrace) {
        for e in &mut self.ents {
            let Some(b) = &mut e.body else { continue };
            let mask = crate::ragdoll::CLIP_MASK & !e.pass;
            let mut t = |a: Vec3, b: Vec3, r: f32| trace(a, b, r, mask);
            b.advance(time, &mut t, &self.phys);
            (e.origin, e.rot) = b.pose_quat(time);
            if b.asleep {
                e.body = None;
            }
        }
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
