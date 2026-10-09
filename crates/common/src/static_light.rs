//! Static-model (`misc_model`) vertex lighting, retail's load-time path:
//! each model's centre samples lump 32's light-visibility grid for the
//! lights of its BSP leaf, the eight strongest survive, and every vertex is
//! lit from them with its own normal. Facts and addresses:
//! docs/research/cod11-light-grid-and-leaf-lights.md.

use crate::bsp::{self, Bsp};
use crate::collision::CollisionWorld;
use glam::Vec3;

/// One overbright bit, retail's full-screen default: light colours load
/// pre-halved and the display doubles them back. The renderer rescales to
/// the current `identityLight` (windowed 1).
const IDENTITY_LIGHT: f32 = 0.5;
/// `1 << r_overBrightBits`, the intensity scale the light sort uses.
const OVERBRIGHT_SCALE: f32 = 2.0;
/// `r_diffuseSunSteps` default: a 3x3 fan of sky rays per grid point.
const SUN_STEPS: usize = 3;
/// `r_diffuseSunQuality` default: the sky becomes two lights (above, below).
const SUN_QUALITY: u32 = 2;
/// `r_maxEntLights` default, also GL's light count.
const MAX_ENT_LIGHTS: usize = 8;
/// `r_minEntLightIntensity` default.
const MIN_ENT_LIGHT_INTENSITY: f32 = 0.02;
const LUMA: Vec3 = Vec3::new(0.299, 0.587, 0.114);
const SURF_SKY: u32 = 0x4;
/// The mask every light-visibility trace runs with.
const VIS_MASK: u32 = 0x2001;
/// Mask bit set when any sky ray of the sample got out.
const SKY_BIT: u16 = 0x8000;
/// Grid cells are 32 x 32 x 64 units from this corner.
const GRID_ORIGIN: f64 = -131072.0;
const BUCKETS: usize = 0x2000;
const SLOTS: usize = 32;
/// Lump 32 is exactly this long or ignored.
const VIS_LUMP_LEN: usize = 0x30_0000;
/// Kind of the two synthetic sky lights.
const KIND_SKY: u32 = 8;
/// Spot cutoff meaning "no cone".
const NO_CONE: f32 = 180.0;

/// The runtime light record retail builds from a lump-30 entry
/// (0x88 bytes, `R_LoadLights` 0x4db620), only the fields lighting reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub kind: u32,
    /// Luminance; negative only for the sky's under-light.
    pub intensity: f32,
    pub ambient: Vec3,
    pub diffuse: Vec3,
    /// Position, or for a directional light the unit vector towards it.
    pub origin: Vec3,
    pub directional: bool,
    pub spot_dir: Vec3,
    /// Constant, linear, quadratic: `1 / (c + l d + q d^2)`.
    pub atten: [f32; 3],
    pub spot_exponent: f32,
    /// Degrees; [`NO_CONE`] is a point light.
    pub spot_cutoff: f32,
}

impl Light {
    /// Decodes one lump-30 record. `sun_diffuse` is the worldspawn-derived
    /// colour the sun (kind 1) takes instead of its own.
    fn from_words(w: &[u32; 18], sun_diffuse: Vec3) -> Light {
        let f = |i: usize| f32::from_bits(w[i]);
        let v = |i: usize| Vec3::new(f(i), f(i + 1), f(i + 2));
        let c = v(1) * IDENTITY_LIGHT;
        let mut l = Light {
            kind: w[0],
            intensity: c.dot(LUMA),
            ambient: c * 0.1,
            diffuse: c * 0.8,
            origin: v(4),
            directional: false,
            spot_dir: Vec3::ZERO,
            atten: [0.0; 3],
            spot_exponent: 0.0,
            spot_cutoff: NO_CONE,
        };
        let cone = |cos: f32| cos.clamp(-1.0, 1.0).acos().to_degrees();
        match l.kind {
            1 => {
                l.ambient = Vec3::ZERO;
                l.diffuse = sun_diffuse;
                l.intensity = sun_diffuse.dot(LUMA);
                l.origin = v(7);
                l.directional = true;
                l.atten[0] = 1.0;
            }
            2 => l.atten[2] = 1.0,
            3 => l.atten[1] = f(10),
            4 => l.atten = [f(11), 0.0, f(10)],
            5 => {
                l.atten[2] = 1.0;
                l.spot_dir = -v(7);
                l.spot_cutoff = cone(f(10));
                l.spot_exponent = w[11] as i32 as f32;
            }
            7 => {
                l.atten = [f(11), 0.0, f(10)];
                l.spot_dir = -v(7);
                l.spot_cutoff = cone(f(12));
                l.spot_exponent = w[13] as i32 as f32;
            }
            _ => {}
        }
        l
    }

    /// Sort key before the weight: intensity over attenuation at `p`
    /// (0x4b5e30).
    fn intensity_at(&self, p: Vec3) -> f32 {
        let i = self.intensity * OVERBRIGHT_SCALE;
        if self.directional {
            return i;
        }
        let [c, lin, quad] = self.atten;
        let mut denom = c;
        if lin != 0.0 || quad != 0.0 {
            let d2 = (self.origin - p).length_squared();
            denom = d2 * quad + c;
            if lin != 0.0 {
                return i / (d2.sqrt() * lin + denom);
            }
        }
        i / denom
    }

    /// A scene light as `RE_AddLightToScene` (0x4e9b00) records it: kind 2,
    /// `intensity^2 / 32` as its key intensity, no ambient, falloff
    /// `1 / (0.001 + d^2)`.
    pub fn dynamic(origin: Vec3, color: Vec3, intensity: f32) -> Light {
        let i = intensity * intensity / 32.0;
        Light {
            kind: 2,
            intensity: i,
            ambient: Vec3::ZERO,
            diffuse: color * (IDENTITY_LIGHT * i),
            origin,
            directional: false,
            spot_dir: Vec3::ZERO,
            atten: [0.001, 0.0, 1.0],
            spot_exponent: 0.0,
            spot_cutoff: NO_CONE,
        }
    }

    fn sky(ambient: f32, diffuse: f32, up: f32, sky: Vec3, intensity: f32) -> Light {
        Light {
            kind: KIND_SKY,
            intensity,
            ambient: sky * ambient,
            diffuse: sky * diffuse,
            origin: Vec3::new(0.0, 0.0, up),
            directional: true,
            spot_dir: Vec3::ZERO,
            atten: [0.0; 3],
            spot_exponent: 0.0,
            spot_cutoff: NO_CONE,
        }
    }
}

/// The worldspawn's lighting keys, scaled as `LoadMap`'s entity pass
/// (0x4dbf50) leaves them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct WorldLight {
    ambient: Vec3,
    /// The sun light's diffuse colour.
    sun_diffuse: Vec3,
    /// The sky's share, lit through the sky lights.
    sky: Vec3,
    sky_luma: f32,
}

impl WorldLight {
    fn parse(entities: &str) -> WorldLight {
        let blocks = bsp::entity_blocks(entities);
        let get = |k: &str| {
            blocks.first().and_then(|e| {
                e.iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(k))
                    .map(|(_, v)| v.as_str())
            })
        };
        let float = |k: &str, d: f32| {
            get(k)
                .and_then(|s| s.trim().parse::<f32>().ok())
                .unwrap_or(d)
        };
        let vec = |k: &str| get(k).and_then(bsp::parse_vec3).map(Vec3::from_array);
        let mut ambient = float("ambient", 0.0);
        if ambient > 2.0 {
            // the old 0-255 scale, which retail warns about and rescales
            ambient *= 4.0 / 255.0;
        }
        let color = vec("_color").unwrap_or(Vec3::ZERO).normalize_or_zero();
        let fraction = float("diffuseFraction", 0.5);
        let sun_color = vec("suncolor").unwrap_or(Vec3::ZERO).normalize_or_zero();
        let sky_color = vec("sundiffusecolor").map_or(sun_color, Vec3::normalize_or_zero);
        let sunlight = float("sunlight", 1.0);
        let t = (sunlight - ambient) * IDENTITY_LIGHT;
        let sky = sky_color * (t * fraction);
        WorldLight {
            ambient: if ambient != 0.0 && color != Vec3::ZERO {
                color * (ambient * IDENTITY_LIGHT)
            } else {
                Vec3::ZERO
            },
            sun_diffuse: sun_color * ((1.0 - fraction) * t),
            sky,
            sky_luma: sky.dot(LUMA),
        }
    }
}

/// One light-visibility cache slot. `state` 0 is empty, 1 a sample, 2 a
/// point inside solid; `sun` is twice the sky rays that got out; `mask`
/// has bit `i` for the leaf's `i`th light.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisSlot {
    pub key: u32,
    pub state: u8,
    pub sun: u8,
    pub mask: u16,
}

/// Retail's hash table of grid samples (0x00ca28d0): 8192 buckets of 32
/// slots, seeded from lump 32 and filled by tracing on a miss.
pub struct VisCache {
    slots: Vec<VisSlot>,
}

/// Bit-reverses each byte, keeping byte order (0x4b4de0).
fn reverse_bytes(v: u32) -> u32 {
    u32::from_le_bytes(v.to_le_bytes().map(u8::reverse_bits))
}

impl VisCache {
    /// Lump 32 holds 262144 12-byte slots: key, state, the sun byte for
    /// one to five sky steps, a pad byte, mask. Any other length loads
    /// nothing, as retail's 0x4b7c00 does.
    pub fn from_lump(raw: &[u8]) -> VisCache {
        let mut slots = vec![VisSlot::default(); BUCKETS * SLOTS];
        if raw.len() == VIS_LUMP_LEN {
            for (slot, r) in slots.iter_mut().zip(raw.as_chunks::<12>().0) {
                *slot = VisSlot {
                    key: u32::from_le_bytes([r[0], r[1], r[2], r[3]]),
                    state: r[4],
                    sun: r[4 + SUN_STEPS],
                    mask: u16::from_le_bytes([r[10], r[11]]),
                };
            }
        }
        VisCache { slots }
    }

    pub fn bucket(x: i32, y: i32, z: i32) -> usize {
        let h = reverse_bytes(z as u32)
            .wrapping_add((y as u32).wrapping_mul(0xc41))
            .wrapping_sub((x as u32).wrapping_mul(0xc3d));
        (h & 0x1fff) as usize
    }

    /// The key holds x's low 10 bits, y's low 10 and the cluster's low 12;
    /// z only picks the bucket.
    pub fn key(x: i32, y: i32, cluster: i32) -> u32 {
        ((((x as u32) & 0x3ff) | ((y as u32) << 10)) << 12) | ((cluster as u32) & 0xfff)
    }

    /// The slot for a grid point, filling a missing one from `fill`
    /// (0x4b5420). A full bucket drops its last slot and takes the new
    /// one first.
    fn lookup(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        cluster: i32,
        fill: impl FnOnce() -> VisSlot,
    ) -> VisSlot {
        let key = Self::key(x, y, cluster);
        let base = Self::bucket(x, y, z) * SLOTS;
        let bucket = &mut self.slots[base..base + SLOTS];
        let at = match bucket.iter().position(|s| s.state == 0 || s.key == key) {
            Some(i) if bucket[i].state != 0 => return bucket[i],
            Some(i) => i,
            None => {
                bucket.copy_within(0..SLOTS - 1, 1);
                0
            }
        };
        bucket[at] = VisSlot { key, ..fill() };
        bucket[at]
    }
}

/// Grid cell and the fraction past it along one axis: `cell_size` 32 for
/// x and y, 64 for z. The cell is `fistp(v - 0.5) >> shift`, so an exact
/// odd integer rounds down one.
pub fn grid_axis(v: f32, shift: u32) -> (i32, f32) {
    let off = v as f64 - GRID_ORIGIN;
    let cell = ((off - 0.5).round_ties_even() as i32) >> shift;
    let frac = (off / f64::from(1u32 << shift) - f64::from(cell)) as f32;
    (cell, frac)
}

/// A grid point's world position, where a missing sample traces from.
fn grid_point(x: i32, y: i32, z: i32) -> Vec3 {
    Vec3::new(
        ((x - 0x1000) * 32) as f32,
        ((y - 0x1000) * 32) as f32,
        ((z - 0x800) * 64) as f32,
    )
}

/// The lights one static model keeps, with their weights, and the sky
/// scale its vertices add to the ambient (zero once the sky became
/// lights).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelLights {
    pub lights: Vec<(Light, f32)>,
    pub sky: f32,
}

/// What GL lighting draws an entity model with (0x4d64e0): the light
/// model's ambient and up to eight weighted lights.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityLights {
    pub ambient: Vec3,
    pub lights: Vec<(Light, f32)>,
}

/// A scene light (an fx `Light`) for the entity pick: position, colour,
/// intensity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneLight {
    pub origin: Vec3,
    pub color: Vec3,
    pub intensity: f32,
}

/// What a leaf contributes: its light ids and whether it sees the sky
/// (its list opened with a negative index).
fn leaf_lights(bsp: &LeafTree, leaf: usize) -> (Vec<usize>, bool) {
    let Some(&(first, count)) = bsp.leaf_lights.get(leaf) else {
        return (Vec::new(), false);
    };
    let (mut first, mut count) = (first as usize, count as usize);
    let mut sky = false;
    if count != 0 && bsp.light_indices.get(first).is_some_and(|&i| i < 0) {
        first += 1;
        count -= 1;
        sky = true;
    }
    let ids = bsp
        .light_indices
        .get(first..first + count)
        .unwrap_or(&[])
        .iter()
        .map(|&i| i as u16 as usize)
        .collect();
    (ids, sky)
}

/// The BSP tree and leaf light lists, copied out of the map so the
/// lighting outlives the parse.
struct LeafTree {
    planes: Vec<bsp::Plane>,
    nodes: Vec<bsp::Node>,
    leafs: Vec<bsp::Leaf>,
    light_indices: Vec<i16>,
    leaf_lights: Vec<(u32, u32)>,
}

impl LeafTree {
    fn of(bsp: &Bsp) -> LeafTree {
        LeafTree {
            planes: bsp.planes.clone(),
            nodes: bsp.nodes.clone(),
            leafs: bsp.leafs.clone(),
            light_indices: bsp.light_indices.clone(),
            leaf_lights: bsp.leaf_lights.clone(),
        }
    }
}

/// The leaf `p` falls in, walking from node 0; a point on a plane takes
/// the back child (0x50b390).
fn leaf_index(bsp: &LeafTree, p: Vec3) -> Option<usize> {
    let mut i: i32 = 0;
    for _ in 0..=bsp.nodes.len() {
        if i < 0 {
            return Some((-(i + 1)) as usize);
        }
        let n = bsp.nodes.get(i as usize)?;
        let pl = bsp.planes.get(n.plane as usize)?;
        let d = p.dot(Vec3::from_array(pl.normal)) - pl.dist;
        i = n.children[if d > 0.0 { 0 } else { 1 }];
    }
    None
}

/// Everything model lighting reads from a map, plus the cache it fills as
/// models are lit. Static models light through it once at load, entity
/// models every frame; both share the cache, as in retail.
pub struct StaticLighting {
    bsp: LeafTree,
    world: WorldLight,
    lights: Vec<Light>,
    /// The kind-1 light; the last one wins, as in the loader.
    sun: Option<usize>,
    cache: VisCache,
    /// What a cache miss traces through: world brushes only. `None` on a
    /// map without leaf lights, which never samples.
    collision: Option<CollisionWorld>,
    pub misses: usize,
}

impl StaticLighting {
    pub fn new(bsp: &Bsp) -> Self {
        let world = WorldLight::parse(&bsp.entities);
        let lights: Vec<Light> = bsp
            .lights
            .iter()
            .map(|w| Light::from_words(w, world.sun_diffuse))
            .collect();
        let sun = lights.iter().rposition(|l| l.kind == 1);
        StaticLighting {
            bsp: LeafTree::of(bsp),
            world,
            lights,
            sun,
            cache: VisCache::from_lump(&bsp.light_vis),
            collision: (!bsp.light_indices.is_empty()).then(|| CollisionWorld::build(bsp, &[])),
            misses: 0,
        }
    }

    /// Without leaf light lists every vertex takes `identityLight`.
    pub fn lit(&self) -> bool {
        !self.bsp.light_indices.is_empty()
    }

    /// Samples the grid around a model's bounds centre (0x4b6210), then
    /// picks its lights (0x4b69f0).
    pub fn model_lights(&mut self, center: Vec3) -> ModelLights {
        let (ids, weights, sky) = self.sample(center);
        self.select(center, &ids, &weights, &[], sky)
    }

    /// An entity model's lights for this frame (0x4b7320 -> 0x4b7290): the
    /// grid sample at its lighting origin, then the pick with every scene
    /// light within twice its intensity competing for the eight slots.
    /// A map without leaf lights draws entities unlit at `identityLight`.
    pub fn entity_lights(&mut self, origin: Vec3, scene: &[SceneLight]) -> EntityLights {
        if !self.lit() {
            return EntityLights {
                ambient: Vec3::splat(IDENTITY_LIGHT),
                lights: Vec::new(),
            };
        }
        let near = scene_candidates(origin, scene);
        let (ids, weights, sky) = self.sample(origin);
        let m = self.select(origin, &ids, &weights, &near, sky);
        EntityLights {
            ambient: self.world.sky * m.sky + self.world.ambient,
            lights: m.lights,
        }
    }

    fn sample(&mut self, center: Vec3) -> (Vec<usize>, Vec<f32>, f32) {
        let none = (Vec::new(), Vec::new(), 0.0);
        let Some(leaf) = leaf_index(&self.bsp, center) else {
            return none;
        };
        let Some(cluster) = self.bsp.leafs.get(leaf).map(|l| l.cluster) else {
            return none;
        };
        if cluster < 0 {
            return match self.sun {
                Some(s) => (vec![s], vec![1.0], 1.0),
                None => none,
            };
        }
        let (ids, sees_sky) = leaf_lights(&self.bsp, leaf);
        if ids.is_empty() && !sees_sky {
            return none;
        }
        let ids: Vec<usize> = ids.into_iter().filter(|&i| i < self.lights.len()).collect();
        let mut weights = vec![0.0f32; ids.len()];
        let (cx, fx) = grid_axis(center.x, 5);
        let (cy, fy) = grid_axis(center.y, 5);
        let (cz, fz) = grid_axis(center.z, 6);
        let (wx, wy, wz) = ([1.0 - fx, fx], [1.0 - fy, fy], [1.0 - fz, fz]);
        let sun_scale = 0.5 / (SUN_STEPS * SUN_STEPS) as f32;
        let (mut total, mut sky) = (0.0f32, 0.0f32);
        for corner in 0..8usize {
            let (bx, by, bz) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
            let (x, y, z) = (cx + bx as i32, cy + by as i32, cz + bz as i32);
            let slot = {
                let lights = &self.lights;
                let Some(world) = self.collision.as_ref() else {
                    continue;
                };
                let bsp = &self.bsp;
                let misses = &mut self.misses;
                let ids = &ids;
                self.cache.lookup(x, y, z, cluster, || {
                    *misses += 1;
                    trace_sample(
                        world,
                        bsp,
                        grid_point(x, y, z),
                        center,
                        ids,
                        lights,
                        sees_sky,
                    )
                })
            };
            if slot.state != 1 {
                continue;
            }
            let w = wy[by] * wz[bz] * wx[bx];
            total += w;
            sky += f32::from(slot.sun) * w * sun_scale;
            for (i, weight) in weights.iter_mut().enumerate() {
                if i < 16 && slot.mask & (1 << i) != 0 {
                    *weight += w;
                }
            }
        }
        if total < 0.98 && total != 0.0 {
            let s = 1.0 / total;
            sky *= s;
            for (i, weight) in weights.iter_mut().enumerate() {
                if *weight != 0.0 {
                    *weight = if Some(ids[i]) == self.sun {
                        (1.0 - total) + *weight
                    } else {
                        *weight * s
                    };
                }
            }
        }
        if sees_sky && sky < 0.25 {
            sky = 0.25;
        }
        (ids, weights, sky)
    }

    /// `scene` lights join after the leaf's, weight 1, ahead of the sky's.
    fn select(
        &self,
        center: Vec3,
        ids: &[usize],
        weights: &[f32],
        scene: &[Light],
        sky: f32,
    ) -> ModelLights {
        let mut cands: Vec<(Light, f32)> = ids
            .iter()
            .zip(weights)
            .map(|(&i, &w)| (self.lights[i], w))
            .chain(scene.iter().map(|&l| (l, 1.0)))
            .collect();
        let mut sky_left = sky;
        let world = &self.world;
        if sky != 0.0 && world.sky_luma != 0.0 && SUN_QUALITY != 0 {
            if SUN_QUALITY == 1 || MAX_ENT_LIGHTS < cands.len() {
                cands.push((Light::sky(0.5, 0.5, 1.0, world.sky, world.sky_luma), sky));
            } else {
                cands.push((Light::sky(0.75, 0.25, 1.0, world.sky, world.sky_luma), sky));
                cands.push((
                    Light::sky(0.0, -0.25, -1.0, world.sky, world.sky_luma * -0.25),
                    sky,
                ));
            }
            sky_left = 0.0;
        }
        // strongest first; a new key goes ahead of an equal one
        let mut keys: Vec<f32> = Vec::with_capacity(cands.len());
        let mut order: Vec<usize> = Vec::with_capacity(MAX_ENT_LIGHTS);
        for (i, (l, w)) in cands.iter().enumerate() {
            keys.push(0.0);
            if *w == 0.0 {
                continue;
            }
            let key = if l.intensity < 0.0 {
                1e19
            } else {
                let mut k = l.intensity_at(center) * w;
                if l.kind == 1 || l.kind == KIND_SKY {
                    k += OVERBRIGHT_SCALE * world.sky_luma * sky_left;
                }
                if k < MIN_ENT_LIGHT_INTENSITY {
                    continue;
                }
                k
            };
            keys[i] = key;
            let at = order
                .iter()
                .position(|&o| keys[o] <= key)
                .unwrap_or(order.len());
            if at < MAX_ENT_LIGHTS {
                order.truncate(MAX_ENT_LIGHTS - 1);
                order.insert(at.min(order.len()), i);
            }
        }
        ModelLights {
            lights: order.iter().map(|&i| cands[i]).collect(),
            sky: sky_left,
        }
    }

    /// One vertex's colour (0x4e5210): `p` in world space, `n` unit.
    pub fn vertex_color(&self, m: &ModelLights, p: Vec3, n: Vec3) -> [u8; 4] {
        if !self.lit() {
            let i = identity_byte();
            return [i, i, i, 255];
        }
        shade_vertex(&self.world, m, p, n)
    }
}

/// The scene lights that reach a model at `origin`: within twice their
/// intensity (0x4b69f0, 0x569080 being 4.0).
fn scene_candidates(origin: Vec3, scene: &[SceneLight]) -> Vec<Light> {
    scene
        .iter()
        .filter(|l| {
            l.intensity > 0.0
                && (origin - l.origin).length_squared() <= 4.0 * l.intensity * l.intensity
        })
        .map(|l| Light::dynamic(l.origin, l.color, l.intensity))
        .collect()
}

/// `identityLight` as a byte (`__ftol` truncates).
pub fn identity_byte() -> u8 {
    (IDENTITY_LIGHT * 255.0) as u8
}

/// A colour as `rgbGen lightingPrecalc` / `constLighting` store it:
/// halved by `identityLight`, scaled, truncated (0x504ea7).
pub fn precalc_byte(c: f32) -> u8 {
    (c * IDENTITY_LIGHT * 255.0).clamp(0.0, 255.0) as u8
}

/// Q3's 1024-entry sine table read as a cosine, as the cone test does.
fn table_cos(deg: f32) -> f32 {
    let i = (((deg + 90.0) * (1024.0 / 360.0)) as f64 + 9.313226e-10).round_ties_even() as i64;
    ((i & 0x3ff) as f64 * std::f64::consts::TAU / 1024.0).sin() as f32
}

fn shade_vertex(world: &WorldLight, m: &ModelLights, p: Vec3, n: Vec3) -> [u8; 4] {
    let mut c = world.sky * m.sky + world.ambient;
    for (l, w) in &m.lights {
        let (ndl, atten) = if l.directional {
            (l.origin.dot(n).max(0.0), *w)
        } else {
            let v = l.origin - p;
            let d = v.length();
            let v = if d != 0.0 { v / d } else { v };
            let [k, lin, quad] = l.atten;
            let mut atten = w / ((d * quad + lin) * d + k);
            if l.spot_cutoff != NO_CONE {
                let cs = v.dot(l.spot_dir);
                if cs > table_cos(l.spot_cutoff) {
                    let mut i = 0.0;
                    while i < l.spot_exponent {
                        atten *= cs;
                        i += 1.0;
                    }
                } else {
                    atten = 0.0;
                }
            }
            (v.dot(n).max(0.0), atten)
        };
        c += (l.diffuse * ndl + l.ambient) * atten;
    }
    let b = |v: f32| {
        ((v * 255.0) as f64 + 9.313226e-10)
            .round_ties_even()
            .clamp(0.0, 255.0) as u8
    };
    [b(c.x), b(c.y), b(c.z), 255]
}

/// A grid point no compile sampled (0x4b5190): solid unless the model's
/// centre sees it, then one ray per light and the sky fan.
fn trace_sample(
    world: &CollisionWorld,
    bsp: &LeafTree,
    point: Vec3,
    center: Vec3,
    ids: &[usize],
    lights: &[Light],
    sees_sky: bool,
) -> VisSlot {
    let solid = VisSlot {
        state: 2,
        ..VisSlot::default()
    };
    let in_world = leaf_index(bsp, point)
        .and_then(|l| bsp.leafs.get(l))
        .is_some_and(|l| l.cluster >= 0);
    if !in_world {
        return solid;
    }
    let to_center = (center - point).normalize_or_zero();
    let gate = world.point_trace(point + to_center * 0.01, center, VIS_MASK, false);
    if gate.fraction < 1.0 || gate.startsolid {
        return solid;
    }
    let mut slot = VisSlot {
        state: 1,
        ..VisSlot::default()
    };
    for (i, &id) in ids.iter().enumerate().take(15) {
        let l = &lights[id];
        let seen = if l.directional {
            let t = world.point_trace(
                point + l.origin * 0.1,
                point + l.origin * 32768.0,
                VIS_MASK,
                false,
            );
            !t.startsolid && (t.fraction >= 1.0 || t.surface_flags & SURF_SKY != 0)
        } else {
            let dir = (l.origin - point).normalize_or_zero();
            let t = world.point_trace(point + dir * 0.1, l.origin, VIS_MASK, false);
            !t.startsolid && t.fraction >= 1.0
        };
        if seen {
            slot.mask |= 1 << i;
        }
    }
    if sees_sky {
        let spacing = if SUN_STEPS > 3 { 16384.0 } else { 32768.0 };
        let half = spacing * 0.5 * (SUN_STEPS - 1) as f32;
        for a in 0..SUN_STEPS {
            let oy = -half + a as f32 * spacing;
            for b in 0..SUN_STEPS {
                let ox = -half + b as f32 * spacing;
                let start = point + Vec3::new(ox * 6.1035155e-7, oy * 6.1035155e-7, 0.1);
                let end = Vec3::new(point.x + ox, point.y + oy, point.z + 32768.0);
                let t = world.point_trace(start, end, VIS_MASK, false);
                if !t.startsolid && (t.fraction >= 1.0 || t.surface_flags & SURF_SKY != 0) {
                    slot.mask |= SKY_BIT;
                    slot.sun = slot.sun.saturating_add(2);
                }
            }
        }
    }
    slot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_axis_cells_and_fractions() {
        // 32-unit cells from -131072: x = 0 is cell 4096 exactly
        assert_eq!(grid_axis(0.0, 5), (4096, 0.0));
        assert_eq!(grid_axis(16.0, 5), (4096, 0.5));
        assert_eq!(grid_axis(-8.0, 5), (4095, 0.75));
        // z cells are 64 tall from the same corner
        assert_eq!(grid_axis(96.0, 6), (2049, 0.5));
        // fistp(v - 0.5) rounds an exact odd integer down a unit; the cell
        // only moves when that crosses a boundary
        assert_eq!(grid_axis(-131071.0, 0), (0, 1.0));
        assert_eq!(grid_axis(-131070.0, 0), (2, 0.0));
    }

    #[test]
    fn grid_point_inverts_the_cell() {
        assert_eq!(grid_point(4096, 4097, 2049), Vec3::new(0.0, 32.0, 64.0));
    }

    #[test]
    fn key_and_bucket_match_a_carentan_slot() {
        // mp_carentan's lump 32 slot 0 (bucket 0) holds key 0x07421197:
        // x low bits 33, y 29, cluster 407, from grid point (4129, 4125, 2049)
        assert_eq!(VisCache::key(4096 + 33, 4096 + 29, 407), 0x0742_1197);
        assert_eq!(VisCache::bucket(4096 + 33, 4096 + 29, 2049), 0);
        assert_eq!(reverse_bytes(0x0000_0801), 0x0000_1080);
    }

    #[test]
    fn full_bucket_evicts_its_last_slot() {
        let mut c = VisCache::from_lump(&[]);
        let filled = |sun| {
            move || VisSlot {
                state: 1,
                sun,
                ..VisSlot::default()
            }
        };
        // 33 keys into one bucket: same (x, z), clusters 0..33
        for cl in 0..33 {
            c.lookup(4096, 4096, 2048, cl, filled(cl as u8));
        }
        let b = VisCache::bucket(4096, 4096, 2048) * SLOTS;
        assert_eq!(c.slots[b].sun, 32, "the newest slot goes first");
        assert_eq!(c.slots[b + 1].sun, 0);
        assert_eq!(c.slots[b + SLOTS - 1].sun, 30, "cluster 31 fell off");
        // a hit returns the stored slot without refilling
        let hit = c.lookup(4096, 4096, 2048, 5, || unreachable!());
        assert_eq!(hit.sun, 5);
    }

    fn point_light(origin: Vec3, color: f32) -> Light {
        let mut w = [0u32; 18];
        w[0] = 4;
        w[1..4].fill(color.to_bits());
        for (i, v) in origin.to_array().iter().enumerate() {
            w[4 + i] = v.to_bits();
        }
        w[10] = 1.0f32.to_bits(); // quadratic; constant stays 0
        Light::from_words(&w, Vec3::ZERO)
    }

    #[test]
    fn point_light_decodes_and_shades() {
        let l = point_light(Vec3::new(0.0, 0.0, 10.0), 202.0);
        // colours load halved; ambient a tenth, diffuse four fifths
        assert!((l.ambient - Vec3::splat(10.1)).length() < 1e-4);
        assert!((l.diffuse - Vec3::splat(80.8)).length() < 1e-4);
        let m = ModelLights {
            lights: vec![(l, 1.0)],
            sky: 0.0,
        };
        let world = WorldLight::default();
        // 10 units straight below, facing it: (80.8 + 10.1) / 100
        let up = shade_vertex(&world, &m, Vec3::ZERO, Vec3::Z);
        assert_eq!(up, [232, 232, 232, 255]);
        // facing away keeps only the ambient tenth
        let down = shade_vertex(&world, &m, Vec3::ZERO, -Vec3::Z);
        assert_eq!(down, [26, 26, 26, 255]);
    }

    #[test]
    fn sky_becomes_a_hemisphere_pair() {
        let lighting_world = WorldLight {
            sky: Vec3::splat(0.4),
            sky_luma: 0.4,
            ..WorldLight::default()
        };
        let up = Light::sky(0.75, 0.25, 1.0, lighting_world.sky, 0.4);
        let under = Light::sky(0.0, -0.25, -1.0, lighting_world.sky, -0.1);
        let m = ModelLights {
            lights: vec![(under, 1.0), (up, 1.0)],
            sky: 0.0,
        };
        let px = |n: Vec3| shade_vertex(&lighting_world, &m, Vec3::ZERO, n)[0];
        // up: 0.4 full, side: 0.75 of it, down: half
        assert_eq!(px(Vec3::Z), 102);
        assert_eq!(px(Vec3::X), 77);
        assert_eq!(px(-Vec3::Z), 51);
    }

    #[test]
    fn spot_cone_uses_the_sine_table() {
        assert!((table_cos(0.0) - 1.0).abs() < 1e-6);
        assert!((table_cos(60.0) - 0.5).abs() < 0.01);
        let mut l = point_light(Vec3::new(0.0, 0.0, 10.0), 202.0);
        // the cone tests the vertex-to-light ray against the stored axis
        l.spot_dir = Vec3::Z;
        l.spot_cutoff = 30.0;
        l.spot_exponent = 2.0;
        let m = ModelLights {
            lights: vec![(l, 1.0)],
            sky: 0.0,
        };
        let w = WorldLight::default();
        assert_eq!(shade_vertex(&w, &m, Vec3::ZERO, Vec3::Z)[0], 232);
        // 45 degrees off the axis is outside a 30 degree cone
        let off = shade_vertex(&w, &m, Vec3::new(10.0, 0.0, 0.0), Vec3::Z);
        assert_eq!(off[0], 0);
    }

    #[test]
    fn select_keeps_the_eight_strongest() {
        let bsp = crate::vis::two_cell_world();
        let mut s = StaticLighting::new(&bsp);
        s.lights = (0..10)
            .map(|i| point_light(Vec3::new(i as f32 * 10.0, 0.0, 0.0), 100.0))
            .collect();
        let ids: Vec<usize> = (0..10).collect();
        let weights = vec![1.0; 10];
        let m = s.select(Vec3::ZERO, &ids, &weights, &[], 0.0);
        assert_eq!(m.lights.len(), MAX_ENT_LIGHTS);
        let xs: Vec<f32> = m.lights.iter().map(|(l, _)| l.origin.x).collect();
        assert_eq!(xs, [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0]);
    }

    #[test]
    fn a_scene_light_competes_for_the_eight_slots() {
        let bsp = crate::vis::two_cell_world();
        let mut s = StaticLighting::new(&bsp);
        s.lights = (0..8)
            .map(|i| point_light(Vec3::new(i as f32 * 10.0 + 10.0, 0.0, 0.0), 1000.0))
            .collect();
        let ids: Vec<usize> = (0..8).collect();
        let weights = vec![1.0; 8];
        let scene = [
            // a grenade's 800 at 20 units outranks every map light
            SceneLight {
                origin: Vec3::new(0.0, 20.0, 0.0),
                color: Vec3::ONE,
                intensity: 800.0,
            },
            // 10 units short of twice its intensity away
            SceneLight {
                origin: Vec3::new(0.0, -210.0, 0.0),
                color: Vec3::ONE,
                intensity: 100.0,
            },
        ];
        let near = scene_candidates(Vec3::ZERO, &scene);
        assert_eq!(near.len(), 1);
        let m = s.select(Vec3::ZERO, &ids, &weights, &near, 0.0);
        assert_eq!(m.lights.len(), MAX_ENT_LIGHTS);
        assert_eq!(m.lights[0].0.kind, 2);
        assert_eq!(m.lights[0].0.atten, [0.001, 0.0, 1.0]);
        // the farthest map light lost its slot
        let xs: Vec<f32> = m.lights[1..].iter().map(|(l, _)| l.origin.x).collect();
        assert_eq!(xs, [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0]);
    }

    #[test]
    fn entities_on_an_unlit_map_take_identity_light() {
        let bsp = crate::vis::two_cell_world();
        let mut s = StaticLighting::new(&bsp);
        assert!(!s.lit());
        let l = s.entity_lights(Vec3::ZERO, &[]);
        assert_eq!(l.ambient, Vec3::splat(0.5));
        assert!(l.lights.is_empty());
    }

    /// Release-build cost of the per-frame pick, and that a player on
    /// mp_carentan's open ground gets the sky pair and the sun.
    #[test]
    fn carentan_entity_pick() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(path) = fs.resolve_map("mp_carentan") else {
            return;
        };
        let bsp = bsp::parse(&fs.read(&path).unwrap()).unwrap();
        let Some((origin, _)) = bsp::find_spawn(&bsp.entities) else {
            return;
        };
        let mut s = StaticLighting::new(&bsp);
        let at = Vec3::from_array(origin) + Vec3::Z * 32.0;
        let l = s.entity_lights(at, &[]);
        assert!(!l.lights.is_empty(), "no lights at {at}");
        assert!(l.lights.len() <= MAX_ENT_LIGHTS);
        let t = std::time::Instant::now();
        let n = 2000;
        for i in 0..n {
            let p = at + Vec3::new((i % 40) as f32 * 4.0, (i / 40) as f32 * 4.0, 0.0);
            std::hint::black_box(s.entity_lights(p, &[]));
        }
        eprintln!(
            "entity pick: {:.2} us each, {} traced samples",
            t.elapsed().as_secs_f64() * 1e6 / n as f64,
            s.misses
        );
    }

    #[test]
    fn worldspawn_keys_split_sun_and_sky() {
        let w = WorldLight::parse(
            "{\n\"classname\" \"worldspawn\"\n\"suncolor\" \"1 1 1\"\n\"sunlight\" \"1.2\"\n\"ambient\" \"0.2\"\n\"_color\" \"0 0 2\"\n\"diffuseFraction\" \"0.25\"\n}\n",
        );
        let t = (1.2f32 - 0.2) * 0.5;
        let n = Vec3::ONE.normalize();
        assert_eq!(w.ambient, Vec3::new(0.0, 0.0, 0.1));
        assert!((w.sun_diffuse - n * (0.75 * t)).length() < 1e-6);
        assert!((w.sky - n * (0.25 * t)).length() < 1e-6);
    }
}
