//! Worlds 0 and 1's bodies as iw3mp.exe 1.7 makes them (cod4-gsc.md
//! 3.6.3): `Phys_ObjCreate` (0x5a4fb0) into 0x5a4d90, the part build
//! 0x5a4ab0 with its mass centre move 0x5a47c0, `Phys_ObjSetCollisionFromXModel`
//! (0x5a5710) and its box and cylinder helpers, the dynamic entities'
//! collision (0x490240), and the impact sound row a preset carries
//! (0x43dfb0).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::collide::Obj;
use super::geom::{Geom, Transform};
use super::mass::Mass;
use super::math::{M3, V3};
use super::phys::{PhysObj, PhysWorld, WorldId, axes_of};
use super::world::{BodyId, PhysDvars};
use crate::assets::model::{PhysBrush, PhysGeomList, XModel};
use crate::ragdoll::iw;

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// What `Phys_ObjCreate` and the dynamic entities' collision read off a
/// `PhysPreset`: the impact sound row (+4, written at registration,
/// [`impact_sound_rows`]), mass (+8), bounce (+0xc), friction (+0x10) and
/// the dynamic entity's cylinder byte (+0x28, 0x490292).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjPreset {
    pub sound: i32,
    pub mass: f32,
    pub bounce: f32,
    pub friction: f32,
    pub cylinder: bool,
}

impl ObjPreset {
    /// From the asset's 0x2C bytes, with the row registration gave it.
    pub fn from_raw(raw: &[u8], sound: i32) -> Option<ObjPreset> {
        let f = |o: usize| Some(f32::from_le_bytes(raw.get(o..o + 4)?.try_into().ok()?));
        Some(ObjPreset {
            sound,
            mass: f(8)?,
            bounce: f(12)?,
            friction: f(16)?,
            cylinder: *raw.get(0x28)? != 0,
        })
    }
}

/// The block 0x5a4ab0 builds a body's mass and one geom from: the part
/// (+0, the type its table at 0x5a4d70 switches on, and its fields from
/// +0x2c), and the axes a geom transform turns it by when +4 is set (+8).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub kind: PartKind,
    pub transform: Option<M3>,
}

/// 0x5a4ab0's types as worlds 0 and 1 reach them. Type 2 (a brush model,
/// 0x5a8d50, class 11) comes only from a brush dynamic entity (0x5a53a0)
/// and type 5 (the capsule) only from the ragdoll (0x5a56a0); neither is
/// here.
#[derive(Clone, Debug, PartialEq)]
pub enum PartKind {
    /// 0: a sphere's mass of radius 1 (0x5b5c70), no geom.
    Sphere,
    /// 1: `dMassSetBox` (0x5b5e50) and `dCreateBox` (0x5aa500), the full
    /// side lengths.
    Box(V3),
    /// 3: `PhysMass` (0x5a49d0) of the model's moments and products, and a
    /// class 12 geom (0x5a8d90) on its brush.
    Brush {
        brush: Arc<PhysBrush>,
        moments: V3,
        products: V3,
    },
    /// 4: 0x5b5dc0 and a class 13 geom (0x5a8dd0): axis `direction`
    /// (1..=3), the radius and the length the mass takes, which the geom
    /// keeps as its +0x54.
    Cylinder {
        direction: usize,
        radius: f32,
        length: f32,
    },
}

fn transpose(m: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}

impl PhysWorld {
    /// `Phys_ObjCreate` (0x5a4fb0) into 0x5a4d90: the world's sleep
    /// thresholds off the dvars (0x5a4d93..0x5a4dfa, as each frame stores
    /// them), `dBodyCreate`, the position, `vel` and no spin, the rotation
    /// through `dBodySetRotation` from `axes` (by rows: 0x596b80 makes them
    /// `R`'s columns), the phys object (its offset 0, the preset's
    /// friction, bounce and sound row, state 0), and a type 0 part of the
    /// preset's mass about `-0` (0x5a4f05..0x5a4f57). The object's pose is
    /// then `pos` and `axes` as given (0x5a4f5f..0x5a4f7c).
    pub fn obj_create(
        &mut self,
        pos: V3,
        axes: &M3,
        vel: V3,
        preset: &ObjPreset,
        dv: &PhysDvars,
    ) -> BodyId {
        let sq = |x: f32| (w(x) * w(x)) as f32;
        let adis = &mut self.world.params.adis;
        adis.linear_threshold = sq(dv.auto_disable_linear);
        adis.angular_threshold = sq(dv.auto_disable_angular);
        adis.idle_time = dv.auto_disable_time;
        let id = self.create_body();
        let b = self.world.body_mut(id);
        if self.id == WorldId::Ragdoll {
            b.flags &= !3;
            b.set_finite_rotation(true);
        }
        b.pos = pos;
        b.lvel = vel;
        b.avel = [0.0; 3];
        b.set_rotation(&transpose(axes));
        self.add_obj(
            id,
            PhysObj {
                offset: [0.0; 3],
                obj: Obj {
                    friction: preset.friction,
                    bounce: preset.bounce,
                    state: 0,
                },
                prev: (pos, *axes),
                sound: preset.sound,
                geoms: Vec::new(),
            },
        );
        let sphere = Part {
            kind: PartKind::Sphere,
            transform: None,
        };
        self.add_part(id, preset.mass, &sphere, [-0.0; 3]);
        if let Some(o) = self.obj_mut(id) {
            o.prev.0 = pos;
        }
        id
    }

    /// 0x5a4ab0: the body moved to sit at `centre` in its model's frame
    /// ([`Self::move_to`]), then the part's mass of `total` and its geom,
    /// wrapped in a transform turned by the part's axes when it has them
    /// (0x5aeb50: the inner geom out of the space and off the body,
    /// 0x5a9850; the frame's position 0); the geom goes first on the body,
    /// the mass replaces the body's (0x5a4d5a).
    pub fn add_part(&mut self, id: BodyId, total: f32, part: &Part, centre: V3) {
        self.move_to(id, centre);
        let (mass, geom) = match &part.kind {
            PartKind::Sphere => (Mass::sphere_total(total, 1.0), None),
            &PartKind::Box(s) => (
                Mass::box_total(total, s[0], s[1], s[2]),
                Some(Geom::Box { sides: s }),
            ),
            PartKind::Brush {
                brush,
                moments,
                products,
            } => (
                Mass::phys_mass(total, *moments, *products),
                Some(Geom::Brush {
                    brush: brush.clone(),
                    offset: centre,
                }),
            ),
            &PartKind::Cylinder {
                direction,
                radius,
                length,
            } => (
                Mass::cylinder_total(total, direction, radius, length),
                Some(Geom::Cylinder {
                    direction,
                    radius,
                    half_height: length,
                }),
            ),
        };
        if let Some(g) = geom {
            let g = match &part.transform {
                Some(axes) => Geom::Transform(Box::new(Transform {
                    inner: g,
                    r: transpose(axes),
                    pos: [0.0; 3],
                })),
                None => g,
            };
            if let Some(o) = self.obj_mut(id) {
                o.geoms.insert(0, g);
            }
        }
        self.world.body_mut(id).set_mass(mass);
    }

    /// 0x5a47c0: the body moved so its model's origin stays where its
    /// offset puts it (`pos + R offset`, 0x5a4740) and the body sits at
    /// `centre` in the model's frame: `origin + R centre`, the offset
    /// becoming `-centre`; each transform's frame moves by `-offset -
    /// centre`, each step stored (0x5a4911..0x5a4994); the object's
    /// position (+0x10) follows.
    pub fn move_to(&mut self, id: BodyId, centre: V3) {
        let b = self.world.body(id);
        let axes = axes_of(&b.r);
        let Some(o) = self.obj(id) else { return };
        let back = o.offset.map(|x| -x);
        let origin = iw::offset(b.pos, &axes, o.offset);
        let pos = iw::offset(origin, &axes, centre);
        self.world.body_mut(id).pos = pos;
        let Some(o) = self.obj_mut(id) else { return };
        o.offset = centre.map(|x| -x);
        for g in &mut o.geoms {
            if let Geom::Transform(t) = g {
                t.pos = std::array::from_fn(|k| {
                    ((w(t.pos[k]) + w(back[k])) as f32 as f64 - w(centre[k])) as f32
                });
            }
        }
        o.prev.0 = pos;
    }

    /// `Phys_ObjSetCollisionFromXModel` (0x5a5710): with `physGeoms`, each
    /// geom in its order at the body's mass, a brush (0x5a5420) about the
    /// list's mass centre, a type 1 a box of twice `halfLengths` (0x5a52e0)
    /// and any other type a cylinder along x of radius `halfLengths` y and
    /// length x (0x5a55f0), those two turned by `orientation` about their
    /// `offset`; then the list's `PhysMass` and the body moved to its centre
    /// (0x5a57bd..0x5a57fc). With none, [`Self::bounds_box`] on the model's
    /// bounds, a flat axis making them -50..50 (0x5a5847..0x5a589a).
    pub fn set_collision_from_xmodel(&mut self, id: BodyId, model: &XModel) {
        match &model.phys_geoms {
            Some(list) => self.collision_from_geoms(id, list),
            None => {
                let (mut mins, mut maxs) = (model.mins, model.maxs);
                if (0..3).any(|k| mins[k] == maxs[k]) {
                    (mins, maxs) = ([-50.0; 3], [50.0; 3]);
                }
                self.bounds_box(id, mins, maxs);
            }
        }
    }

    /// A dynamic entity's collision (0x490240): [`Self::set_collision_from_xmodel`]
    /// with `physGeoms`; else its model's bounds as they are (0x48fd00), a
    /// cylinder on them when the preset's +0x28 is set (0x5a54a0), a box
    /// otherwise. Its brush model arm (0x5a53a0) is not here.
    pub fn dynent_collision(&mut self, id: BodyId, model: &XModel, cylinder: bool) {
        if model.phys_geoms.is_some() {
            self.set_collision_from_xmodel(id, model);
        } else if cylinder {
            self.bounds_cylinder(id, model.mins, model.maxs);
        } else {
            self.bounds_box(id, model.mins, model.maxs);
        }
    }

    fn collision_from_geoms(&mut self, id: BodyId, list: &PhysGeomList) {
        for g in &list.geoms {
            let total = self.world.body(id).mass.mass;
            let (part, centre) = match &g.brush {
                Some(b) => (
                    Part {
                        kind: PartKind::Brush {
                            brush: Arc::new(b.clone()),
                            moments: list.moments,
                            products: list.products,
                        },
                        transform: None,
                    },
                    list.center_of_mass,
                ),
                None if g.kind == 1 => (
                    Part {
                        kind: PartKind::Box(g.half_lengths.map(|h| (w(h) * 2.0) as f32)),
                        transform: Some(g.orientation),
                    },
                    g.offset,
                ),
                None => (
                    Part {
                        kind: PartKind::Cylinder {
                            direction: 1,
                            radius: g.half_lengths[1],
                            length: g.half_lengths[0],
                        },
                        transform: Some(g.orientation),
                    },
                    g.offset,
                ),
            };
            self.add_part(id, total, &part, centre);
        }
        let total = self.world.body(id).mass.mass;
        let m = Mass::phys_mass(total, list.moments, list.products);
        self.world.body_mut(id).set_mass(m);
        self.move_to(id, list.center_of_mass);
    }

    /// 0x5a5240: a box of the body's mass on `mins..maxs`, its sides
    /// `maxs - mins` and the body at their midpoint, each stored.
    pub fn bounds_box(&mut self, id: BodyId, mins: V3, maxs: V3) {
        let (c, d) = mid_and_size(mins, maxs);
        let part = Part {
            kind: PartKind::Box(d),
            transform: None,
        };
        let total = self.world.body(id).mass.mass;
        self.add_part(id, total, &part, c);
    }

    /// 0x5a54a0: a cylinder of the body's mass along z on `mins..maxs` at
    /// their midpoint: radius half the larger of the x and y sizes (y when
    /// `x - y` stored is under 0), length half the z size.
    pub fn bounds_cylinder(&mut self, id: BodyId, mins: V3, maxs: V3) {
        let (c, d) = mid_and_size(mins, maxs);
        let m = if ((w(d[0]) - w(d[1])) as f32) < 0.0 {
            d[1]
        } else {
            d[0]
        };
        let part = Part {
            kind: PartKind::Cylinder {
                direction: 3,
                radius: (w(m) * 0.5) as f32,
                length: (0.5 * w(d[2])) as f32,
            },
            transform: None,
        };
        let total = self.world.body(id).mass.mass;
        self.add_part(id, total, &part, c);
    }
}

/// The bounds' midpoint and size, each part stored (0x5a5246..0x5a52b0).
fn mid_and_size(mins: V3, maxs: V3) -> (V3, V3) {
    (
        std::array::from_fn(|k| ((w(mins[k]) + w(maxs[k])) * 0.5) as f32),
        std::array::from_fn(|k| (w(maxs[k]) - w(mins[k])) as f32),
    )
}

/// `DB_HashForName` (0x488fd0): from the asset type, `h = 31 h + c` per
/// character lowered (a backslash as `/`), the low 15 bits.
pub fn db_hash(kind: u32, name: &str) -> u32 {
    let mut h = kind;
    for c in name.bytes() {
        let c = match c.to_ascii_lowercase() {
            b'\\' => b'/',
            c => c,
        };
        h = h.wrapping_mul(31).wrapping_add(c as i8 as i32 as u32);
    }
    h & 0x7fff
}

/// The impact sound row each preset's +4 holds once the cgame registers
/// them (0x43e030): the loaded `PhysPreset`s (asset type 1) in the asset
/// hash's order (0x489120: buckets up, each chain newest first, 0x489442),
/// the first 50 (0x43e053); each takes its `sndAliasPrefix`'s index among
/// those met before it, compared without case, a new one appended
/// (0x43dfb0). `presets` are `(name, prefix)` in load order; a name met
/// again is the asset already in the table.
pub fn impact_sound_rows<'a>(
    presets: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> HashMap<String, i32> {
    let mut seen = HashSet::new();
    let mut table: Vec<(u32, usize, &str, &str)> = Vec::new();
    for (name, prefix) in presets {
        if seen.insert(name.to_ascii_lowercase()) {
            table.push((db_hash(1, name), table.len(), name, prefix.unwrap_or("")));
        }
    }
    table.sort_by_key(|&(h, i, _, _)| (h, std::cmp::Reverse(i)));
    let mut prefixes: Vec<&str> = Vec::new();
    let mut out = HashMap::new();
    for &(_, _, name, prefix) in table.iter().take(50) {
        let row = match prefixes.iter().position(|p| p.eq_ignore_ascii_case(prefix)) {
            Some(i) => i,
            None => {
                prefixes.push(prefix);
                prefixes.len() - 1
            }
        };
        out.insert(name.to_ascii_lowercase(), row as i32);
    }
    out
}
