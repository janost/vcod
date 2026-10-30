//! CoD2 1.3 `xmodel` / `xmodelparts` / `xmodelsurfs`, version 20, as raw
//! file records, plus the engine's base-pose composition. Layouts and the
//! evidence for each rule: `docs/research/cod2-xmodel-v20-format.md`.
//!
//! A descriptor ends with one hit box per bone of its LOD 0 `xmodelparts`,
//! so [`parse_descriptor`] asks the caller for that bone count.
use anyhow::{Result, bail, ensure};
use glam::{Quat, Vec3};

/// The version all three files carry.
pub const VERSION: u16 = 20;
/// LOD slots in a descriptor.
pub const LOD_SLOTS: usize = 4;
/// The loader refuses a skeleton of this many bones or more.
pub const MAX_BONES: usize = 128;
/// The 19 hit locations a classification byte indexes, as CoD1's.
pub const HIT_LOCATIONS: [&str; 19] = [
    "none",
    "helmet",
    "head",
    "neck",
    "torso_upper",
    "torso_lower",
    "right_arm_upper",
    "left_arm_upper",
    "right_arm_lower",
    "left_arm_lower",
    "right_hand",
    "left_hand",
    "right_leg_upper",
    "left_leg_upper",
    "right_leg_lower",
    "left_leg_lower",
    "right_foot",
    "left_foot",
    "gun",
];

/// `xmodel/<name>`.
#[derive(Clone, Debug)]
pub struct Descriptor {
    /// Bit 0: a `misc_model` with `gndLt` lights from the ground colour.
    pub flags: u8,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// All four slots as stored; an empty `name` is an unused slot.
    pub lods: [LodSlot; LOD_SLOTS],
    /// LOD whose collision this is; -1 for none.
    pub coll_lod: i32,
    /// As stored: the loader's 0.001 bounds growth and contents mask are
    /// the consumer's (doc, "Collision block").
    pub coll_surfs: Vec<CollSurf>,
    /// One per LOD 0 bone, in bone space; an all-zero box turns the bone off.
    pub bone_infos: Vec<BoneInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct LodSlot {
    /// Draws while the view distance is under this; 0 draws at any distance.
    pub dist: f32,
    /// The `xmodelsurfs` name (and for LOD 0 the `xmodelparts` one).
    pub name: String,
    /// One per surface of the LOD's `xmodelsurfs`, index-aligned. Empty on
    /// an unused slot.
    pub materials: Vec<String>,
}

impl Descriptor {
    /// The used slots up to the first empty one, as the surfs loader walks
    /// them (no stock descriptor has a gap).
    pub fn present_lods(&self) -> impl Iterator<Item = &LodSlot> {
        self.lods.iter().take_while(|l| !l.name.is_empty())
    }
}

#[derive(Clone, Debug)]
pub struct CollSurf {
    pub tris: Vec<CollTri>,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub bone: i32,
    pub contents: i32,
    pub surf_flags: i32,
}

/// v14's three-plane triangle (`XModelCollTri_s`).
#[derive(Clone, Copy, Debug)]
pub struct CollTri {
    pub plane: [f32; 4],
    pub svec: [f32; 4],
    pub tvec: [f32; 4],
}

/// `XBoneInfo_s` as read: the box's two corners in bone space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneInfo {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

impl BoneInfo {
    /// The trace's sphere centre, `(mins + maxs) / 2`.
    pub fn offset(&self) -> Vec3 {
        (Vec3::from(self.mins) + Vec3::from(self.maxs)) * 0.5
    }

    /// `|maxs - offset|^2`; 0 means the trace skips the bone.
    pub fn radius_squared(&self) -> f32 {
        (Vec3::from(self.maxs) - self.offset()).length_squared()
    }
}

/// `xmodelparts/<lod0 name>`: roots first, then children in file order.
#[derive(Clone, Debug)]
pub struct Parts {
    pub num_root: usize,
    pub bones: Vec<PartBone>,
}

#[derive(Clone, Debug)]
pub struct PartBone {
    pub name: String,
    /// Absolute index of a bone before this one; `None` on a root.
    pub parent: Option<usize>,
    /// Local translation in the parent's frame; zero on a root.
    pub trans: [f32; 3],
    /// Local rotation (x, y, z, w) as the engine scales it: each short,
    /// `w` derived, times the float 1/32767. Not unit length. (0,0,0,1)
    /// on a root.
    pub quat: [f32; 4],
    /// Index into [`HIT_LOCATIONS`].
    pub classification: u8,
}

/// The quaternion's `w` short the engine derives from x, y, z (Linux
/// 0x080c613c): 0 once `x^2 + y^2 + z^2` reaches 32767^2.
pub fn quat_w(x: i16, y: i16, z: i16) -> i16 {
    let rest = 0x3fff_0001i64 - [x, y, z].iter().map(|&c| c as i64 * c as i64).sum::<i64>();
    if rest < 1 {
        0
    } else {
        ((rest as f64).sqrt() + 0.5).floor() as i16
    }
}

/// The four shorts scaled by the engine's float 1/32767.
pub fn quat_from_shorts(x: i16, y: i16, z: i16) -> [f32; 4] {
    const SCALE: f32 = 1.0 / 32767.0;
    let w = quat_w(x, y, z);
    [x, y, z, w].map(|c| c as f32 * SCALE)
}

/// `xmodelsurfs/<lod name>`.
#[derive(Clone, Debug)]
pub struct Surfs {
    pub surfaces: Vec<Surface>,
}

impl Surfs {
    /// The highest bone index any surface or vertex names, for checking
    /// against LOD 0's bone count.
    pub fn max_bone(&self) -> Option<u16> {
        self.surfaces
            .iter()
            .flat_map(|s| {
                s.vertices
                    .iter()
                    .flat_map(|v| std::iter::once(v.bone).chain(v.extras.iter().map(|e| e.bone)))
            })
            .max()
    }
}

#[derive(Clone, Debug)]
pub struct Surface {
    /// `XSurface_s.tileMode` (0, 1 or 2 in stock).
    pub tile_mode: u8,
    /// The rigid bone every vertex rides; `None` on a skinned surface.
    pub bone: Option<u16>,
    pub vertices: Vec<Vertex>,
    /// Already in the emitted winding `(i0, i2, i1)` (doc, "Winding"),
    /// every index inside `vertices`.
    pub triangles: Vec<[u16; 3]>,
}

#[derive(Clone, Debug)]
pub struct Vertex {
    pub normal: [f32; 3],
    pub color: [u8; 4],
    pub uv: [f32; 2],
    pub binormal: [f32; 3],
    pub tangent: [f32; 3],
    /// The primary bone; the surface's bone on a rigid surface.
    pub bone: u16,
    /// Position in the primary bone's space.
    pub offset: [f32; 3],
    /// The primary's weight in 256ths; read only with extras, else 0.
    pub primary_weight: u8,
    /// Extra influences (0..3 in stock), each with its own bone-local
    /// position.
    pub extras: Vec<BlendWeight>,
}

impl Vertex {
    /// The primary bone's share: 1 with no extras, else `primary_weight / 256`.
    pub fn primary_fraction(&self) -> f32 {
        if self.extras.is_empty() {
            1.0
        } else {
            self.primary_weight as f32 / 256.0
        }
    }

    /// `primary_weight * 256 + sum(extras)`; 65536 on every stock vertex
    /// with extras.
    pub fn weight_sum_65536(&self) -> u32 {
        if self.extras.is_empty() {
            return 65536;
        }
        self.primary_weight as u32 * 256 + self.extras.iter().map(|e| e.weight as u32).sum::<u32>()
    }
}

/// `XBlendInfo_s`: one extra influence.
#[derive(Clone, Copy, Debug)]
pub struct BlendWeight {
    pub bone: u16,
    pub offset: [f32; 3],
    /// In 65536ths.
    pub weight: u16,
}

impl BlendWeight {
    pub fn fraction(&self) -> f32 {
        self.weight as f32 / 65536.0
    }
}

/// A descriptor's flags byte alone, `None` for another version or a short
/// file.
pub fn descriptor_flags(data: &[u8]) -> Option<u8> {
    let v = u16::from_le_bytes(data.get(..2)?.try_into().ok()?);
    (v == VERSION).then(|| data.get(2).copied())?
}

/// Parses a descriptor. `bone_count` gets LOD 0's name and returns the bone
/// count of `xmodelparts/<that name>`, which sizes the trailing hit boxes.
pub fn parse_descriptor(
    data: &[u8],
    bone_count: impl FnOnce(&str) -> Result<usize>,
) -> Result<Descriptor> {
    let mut r = Reader::new(data);
    r.version()?;
    let flags = r.u8()?;
    let mins = r.vec3()?;
    let maxs = r.vec3()?;
    let mut lods: [LodSlot; LOD_SLOTS] = Default::default();
    for lod in &mut lods {
        lod.dist = r.f32()?;
        lod.name = r.cstr()?;
    }
    let coll_lod = r.i32()?;
    let num_coll = r.count_i32("collision surface")?;
    let mut coll_surfs = Vec::with_capacity(num_coll.min(r.remaining() / 40));
    for _ in 0..num_coll {
        let num_tris = r.count_i32("collision triangle")?;
        ensure!(
            num_tris <= r.remaining() / 48,
            "collision surface claims {num_tris} triangles past the end"
        );
        let mut tris = Vec::with_capacity(num_tris);
        for _ in 0..num_tris {
            tris.push(CollTri {
                plane: r.f32s()?,
                svec: r.f32s()?,
                tvec: r.f32s()?,
            });
        }
        coll_surfs.push(CollSurf {
            tris,
            mins: r.vec3()?,
            maxs: r.vec3()?,
            bone: r.i32()?,
            contents: r.i32()?,
            surf_flags: r.i32()?,
        });
    }
    for lod in lods.iter_mut().filter(|l| !l.name.is_empty()) {
        let count = r.u16()?;
        lod.materials = (0..count).map(|_| r.cstr()).collect::<Result<_>>()?;
    }
    ensure!(!lods[0].name.is_empty(), "LOD 0 has no name");
    let num_bones = bone_count(&lods[0].name)?;
    ensure!(
        num_bones <= r.remaining() / 24,
        "{num_bones} bone infos run past the end"
    );
    let bone_infos = (0..num_bones)
        .map(|_| {
            Ok(BoneInfo {
                mins: r.vec3()?,
                maxs: r.vec3()?,
            })
        })
        .collect::<Result<_>>()?;
    r.end("descriptor")?;
    Ok(Descriptor {
        flags,
        mins,
        maxs,
        lods,
        coll_lod,
        coll_surfs,
        bone_infos,
    })
}

/// Parses an `xmodelparts` file.
pub fn parse_parts(data: &[u8]) -> Result<Parts> {
    let mut r = Reader::new(data);
    r.version()?;
    let num_child = r.i16()?;
    let num_root = r.i16()?;
    ensure!(
        num_child >= 0 && num_root >= 0,
        "negative bone counts {num_child}/{num_root}"
    );
    let (num_child, num_root) = (num_child as usize, num_root as usize);
    let total = num_child + num_root;
    ensure!(
        total < MAX_BONES,
        "{total} bones, the engine takes at most 127"
    );
    let mut bones: Vec<PartBone> = (0..num_root)
        .map(|_| PartBone {
            name: String::new(),
            parent: None,
            trans: [0.0; 3],
            quat: [0.0, 0.0, 0.0, 1.0],
            classification: 0,
        })
        .collect();
    for i in num_root..total {
        let parent = r.u8()? as usize;
        ensure!(parent < i, "bone {i}'s parent {parent} does not precede it");
        let trans = r.vec3()?;
        let (x, y, z) = (r.i16()?, r.i16()?, r.i16()?);
        bones.push(PartBone {
            name: String::new(),
            parent: Some(parent),
            trans,
            quat: quat_from_shorts(x, y, z),
            classification: 0,
        });
    }
    for bone in &mut bones {
        bone.name = r.cstr()?;
    }
    for (i, bone) in bones.iter_mut().enumerate() {
        bone.classification = r.u8()?;
        ensure!(
            (bone.classification as usize) < HIT_LOCATIONS.len(),
            "bone {i} classification {} out of range",
            bone.classification
        );
    }
    r.end("xmodelparts")?;
    Ok(Parts { num_root, bones })
}

/// Parses an `xmodelsurfs` file.
pub fn parse_surfs(data: &[u8]) -> Result<Surfs> {
    let mut r = Reader::new(data);
    r.version()?;
    let num_surfs = r.u16()?;
    let mut surfaces = Vec::with_capacity(num_surfs as usize);
    for si in 0..num_surfs {
        surfaces.push(read_surface(&mut r).map_err(|e| e.context(format!("surface {si}")))?);
    }
    r.end("xmodelsurfs")?;
    Ok(Surfs { surfaces })
}

fn read_surface(r: &mut Reader) -> Result<Surface> {
    let tile_mode = r.u8()?;
    let vert_count = r.u16()? as usize;
    let tri_count = r.u16()? as usize;
    let bone = match r.i16()? {
        -1 => None,
        b if b >= 0 => Some(b as u16),
        b => bail!("surface bone {b}"),
    };
    let blend_count = if bone.is_none() { r.u16()? as usize } else { 0 };
    let mut vertices = Vec::with_capacity(vert_count.min(r.remaining() / 56));
    let mut blends = 0;
    for _ in 0..vert_count {
        let normal = r.vec3()?;
        let color = [r.u8()?, r.u8()?, r.u8()?, r.u8()?];
        let uv = [r.f32()?, r.f32()?];
        let binormal = r.vec3()?;
        let tangent = r.vec3()?;
        let mut v = Vertex {
            normal,
            color,
            uv,
            binormal,
            tangent,
            bone: 0,
            offset: [0.0; 3],
            primary_weight: 0,
            extras: Vec::new(),
        };
        match bone {
            Some(b) => {
                v.bone = b;
                v.offset = r.vec3()?;
            }
            None => {
                let num_weights = r.u8()? as usize;
                v.bone = r.u16()?;
                v.offset = r.vec3()?;
                if num_weights > 0 {
                    v.primary_weight = r.u8()?;
                    v.extras = (0..num_weights)
                        .map(|_| {
                            Ok(BlendWeight {
                                bone: r.u16()?,
                                offset: r.vec3()?,
                                weight: r.u16()?,
                            })
                        })
                        .collect::<Result<_>>()?;
                }
                blends += num_weights;
            }
        }
        vertices.push(v);
    }
    ensure!(
        blends == blend_count,
        "blend count {blend_count}, vertices carry {blends}"
    );
    let mut triangles = Vec::with_capacity(tri_count.min(r.remaining() / 6));
    for _ in 0..tri_count {
        let (i0, i1, i2) = (r.u16()?, r.u16()?, r.u16()?);
        ensure!(
            [i0, i1, i2].iter().all(|&i| (i as usize) < vert_count),
            "triangle ({i0}, {i1}, {i2}) past {vert_count} vertices"
        );
        triangles.push([i0, i2, i1]);
    }
    Ok(Surface {
        tile_mode,
        bone,
        vertices,
        triangles,
    })
}

/// One bone's `DObjAnimMat`: the world rotation left unnormalized, with
/// `trans_weight = 2 / |quat|^2` standing in for the 2 of the unit
/// quaternion matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimMat {
    pub quat: Quat,
    pub trans: Vec3,
    pub trans_weight: f32,
}

impl AnimMat {
    pub const IDENTITY: Self = Self {
        quat: Quat::from_xyzw(0.0, 0.0, 0.0, 1.0),
        trans: Vec3::ZERO,
        trans_weight: 2.0,
    };

    /// `v` turned by the quaternion's matrix with `s = trans_weight`, which
    /// is a pure rotation however long the quaternion is.
    pub fn rotate(&self, v: Vec3) -> Vec3 {
        let (x, y, z, w) = (self.quat.x, self.quat.y, self.quat.z, self.quat.w);
        let s = self.trans_weight;
        Vec3::new(
            (1.0 - s * (y * y + z * z)) * v.x
                + s * (x * y - w * z) * v.y
                + s * (x * z + w * y) * v.z,
            s * (x * y + w * z) * v.x
                + (1.0 - s * (x * x + z * z)) * v.y
                + s * (y * z - w * x) * v.z,
            s * (x * z - w * y) * v.x
                + s * (y * z + w * x) * v.y
                + (1.0 - s * (x * x + y * y)) * v.z,
        )
    }

    /// A bone-local point in model space.
    pub fn transform_point(&self, v: Vec3) -> Vec3 {
        self.rotate(v) + self.trans
    }
}

/// `2 / |q|^2`, or 2 for a zero quaternion.
fn trans_weight(q: Quat) -> f32 {
    let len2 = q.length_squared();
    if len2 == 0.0 { 2.0 } else { 2.0 / len2 }
}

/// The base pose the loader builds (Linux 0x080c621e): roots at identity,
/// each child in file order `QuatMultiply(local, parent)`, the Hamilton
/// product `parent * local`, never normalized, translated by its `trans`
/// turned through the parent's matrix. f32 throughout; not checked to the
/// last bit against retail.
pub fn base_pose(parts: &Parts) -> Vec<AnimMat> {
    let mut out: Vec<AnimMat> = Vec::with_capacity(parts.bones.len());
    for bone in &parts.bones {
        let Some(p) = bone.parent else {
            out.push(AnimMat::IDENTITY);
            continue;
        };
        // `parse_parts` guarantees the parent precedes the child.
        let parent = out[p];
        let local = Quat::from_array(bone.quat);
        let quat = hamilton(parent.quat, local);
        out.push(AnimMat {
            quat,
            trans: parent.transform_point(Vec3::from(bone.trans)),
            trans_weight: trans_weight(quat),
        });
    }
    out
}

/// `a * b`, written out so no SIMD path or normalization intervenes.
fn hamilton(a: Quat, b: Quat) -> Quat {
    Quat::from_xyzw(
        a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    )
}

/// Little-endian cursor whose every read fails, never panics, past the end.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let Some(bytes) = self.data.get(self.pos..self.pos + N) else {
            bail!("short read of {N} at {}/{}", self.pos, self.data.len());
        };
        self.pos += N;
        Ok(bytes.try_into().expect("slice of N"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn f32s<const N: usize>(&mut self) -> Result<[f32; N]> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.f32()?;
        }
        Ok(out)
    }
    fn vec3(&mut self) -> Result<[f32; 3]> {
        self.f32s()
    }

    fn count_i32(&mut self, what: &str) -> Result<usize> {
        let n = self.i32()?;
        ensure!(n >= 0, "negative {what} count {n}");
        Ok(n as usize)
    }

    fn cstr(&mut self) -> Result<String> {
        let rest = &self.data[self.pos..];
        let Some(len) = rest.iter().position(|&b| b == 0) else {
            bail!("unterminated string at {}", self.pos);
        };
        // Latin-1, as the engine treats names as bytes.
        let s = vcod_common::text::latin1(&rest[..len]);
        self.pos += len + 1;
        Ok(s)
    }

    fn version(&mut self) -> Result<()> {
        let v = self.u16()?;
        ensure!(v == VERSION, "version {v}, expected {VERSION}");
        Ok(())
    }

    fn end(&self, what: &str) -> Result<()> {
        ensure!(
            self.pos == self.data.len(),
            "{what}: {} trailing bytes",
            self.data.len() - self.pos
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Little-endian byte builder for synthetic files.
    #[derive(Default)]
    struct W(Vec<u8>);

    impl W {
        fn u8(mut self, v: u8) -> Self {
            self.0.push(v);
            self
        }
        fn u16(mut self, v: u16) -> Self {
            self.0.extend(v.to_le_bytes());
            self
        }
        fn i16(mut self, v: i16) -> Self {
            self.0.extend(v.to_le_bytes());
            self
        }
        fn i32(mut self, v: i32) -> Self {
            self.0.extend(v.to_le_bytes());
            self
        }
        fn f(mut self, vs: &[f32]) -> Self {
            for v in vs {
                self.0.extend(v.to_le_bytes());
            }
            self
        }
        fn s(mut self, v: &str) -> Self {
            self.0.extend(v.as_bytes());
            self.0.push(0);
            self
        }
    }

    /// Two roots (`tag_origin`, `extra`), then `a` under 0 and `b` under 2.
    fn parts_file() -> Vec<u8> {
        W::default()
            .u16(20)
            .i16(2)
            .i16(2)
            .u8(0)
            .f(&[1.0, 2.0, 3.0])
            .i16(0)
            .i16(0)
            .i16(23170)
            .u8(2)
            .f(&[1.0, 0.0, 0.0])
            .i16(0)
            .i16(0)
            .i16(0)
            .s("tag_origin")
            .s("extra")
            .s("a")
            .s("b")
            .u8(0)
            .u8(0)
            .u8(5)
            .u8(18)
            .0
    }

    #[test]
    fn quat_w_matches_the_engine_rule() {
        assert_eq!(quat_w(0, 0, 0), 32767);
        // sqrt(32767^2 - 23170^2) = 23169.53, +0.5 floors to 23170.
        assert_eq!(quat_w(0, 0, 23170), 23170);
        // Past 32767^2 the engine takes 0.
        assert_eq!(quat_w(20000, 20000, 20000), 0);
        assert_eq!(quat_w(32767, 0, 0), 0);
        assert_eq!(quat_w(-32768, 0, 0), 0);
    }

    #[test]
    fn parts_parse() {
        let p = parse_parts(&parts_file()).unwrap();
        assert_eq!(p.num_root, 2);
        let names: Vec<_> = p.bones.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["tag_origin", "extra", "a", "b"]);
        assert_eq!(p.bones[1].parent, None);
        assert_eq!(p.bones[2].parent, Some(0));
        assert_eq!(p.bones[3].parent, Some(2));
        assert_eq!(p.bones[2].trans, [1.0, 2.0, 3.0]);
        let a = 23170.0 * (1.0f32 / 32767.0);
        assert_eq!(p.bones[2].quat, [0.0, 0.0, a, a]);
        assert_eq!(p.bones[3].quat[3], 32767.0 * (1.0f32 / 32767.0));
        assert_eq!(p.bones[2].classification, 5);
        assert_eq!(p.bones[3].classification, 18);
    }

    #[test]
    fn parts_reject_bad_files() {
        let good = parts_file();
        let mut tail = good.clone();
        tail.push(0);
        assert!(parse_parts(&tail).is_err(), "trailing byte");
        for n in 0..good.len() {
            assert!(parse_parts(&good[..n]).is_err(), "truncated at {n}");
        }
        let mut forward = good.clone();
        forward[6] = 2; // first child's parent is itself
        assert!(parse_parts(&forward).is_err());
        let mut ver = good;
        ver[0] = 14;
        assert!(parse_parts(&ver).is_err());
    }

    fn pose_of(children: &[(usize, [f32; 3], [i16; 3])]) -> Vec<AnimMat> {
        let mut bones = vec![PartBone {
            name: "root".into(),
            parent: None,
            trans: [0.0; 3],
            quat: [0.0, 0.0, 0.0, 1.0],
            classification: 0,
        }];
        for &(parent, trans, [x, y, z]) in children {
            bones.push(PartBone {
                name: String::new(),
                parent: Some(parent),
                trans,
                quat: quat_from_shorts(x, y, z),
                classification: 0,
            });
        }
        base_pose(&Parts { num_root: 1, bones })
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 1e-5
    }

    #[test]
    fn base_pose_by_hand() {
        let pose = pose_of(&[
            // 1: 90 degrees about z, stored a hair long (a = 23170/32767).
            (0, [1.0, 2.0, 3.0], [0, 0, 23170]),
            // 2: under 1, identity, one unit along the parent's x.
            (1, [1.0, 0.0, 0.0], [0, 0, 0]),
            // 3: w = 0, q = (b, b, b, 0): 180 degrees about (1,1,1).
            (0, [0.0, 0.0, 0.0], [20000, 20000, 20000]),
            // 4: under 3, one unit along y.
            (3, [0.0, 1.0, 0.0], [0, 0, 0]),
        ]);
        assert_eq!(pose[0], AnimMat::IDENTITY);

        let a = 23170.0 * (1.0f32 / 32767.0);
        // A root parent turns nothing: the local quaternion, unnormalized.
        assert_eq!(pose[1].quat, Quat::from_xyzw(0.0, 0.0, a, a));
        assert_eq!(pose[1].trans, Vec3::new(1.0, 2.0, 3.0));
        // 2 / (2 a^2) = 32767^2 / 23170^2 = 1.999963.
        assert!((pose[1].trans_weight - 1.999963).abs() < 1e-5);

        // Column x of the matrix with s = 1/a^2: (1 - s a^2, s a^2, 0) =
        // (0, 1, 0), so (1,0,0) lands at (1,2,3) + (0,1,0).
        assert!(close(pose[2].trans, Vec3::new(1.0, 3.0, 3.0)));
        // Times the identity short (32767 * 1/32767) the rotation stays q1.
        assert!((pose[2].quat - pose[1].quat).length() < 1e-6);

        let b = 20000.0 * (1.0f32 / 32767.0);
        assert_eq!(pose[3].quat, Quat::from_xyzw(b, b, b, 0.0));
        // 2 / (3 b^2) = 1.789...
        assert!((pose[3].trans_weight - 2.0 / (3.0 * b * b)).abs() < 1e-6);
        // Half-turn about (1,1,1)/sqrt3: y -> 2(n.y)n - y = (2/3, -1/3, 2/3).
        assert!(close(
            pose[4].trans,
            Vec3::new(2.0 / 3.0, -1.0 / 3.0, 2.0 / 3.0)
        ));
    }

    #[test]
    fn base_pose_multiplies_parent_then_local() {
        // 90 degrees about z, then a local 90 about x: parent * local takes
        // a child's y onto the parent-turned z, which stays z.
        let pose = pose_of(&[
            (0, [0.0; 3], [0, 0, 23170]),
            (1, [0.0; 3], [23170, 0, 0]),
            (2, [0.0, 1.0, 0.0], [0, 0, 0]),
        ]);
        assert!(close(pose[3].trans, Vec3::Z));
        // local * parent would have taken it to -x.
        assert!(!close(pose[3].trans, Vec3::NEG_X));
    }

    fn vertex(w: W) -> W {
        w.f(&[0.0, 0.0, 1.0])
            .u8(255)
            .u8(128)
            .u8(64)
            .u8(32)
            .f(&[0.25, 0.75])
            .f(&[0.0, 1.0, 0.0])
            .f(&[1.0, 0.0, 0.0])
    }

    fn surfs_file() -> Vec<u8> {
        let mut w = W::default().u16(20).u16(2);
        // Rigid surface on bone 3: one triangle.
        w = w.u8(2).u16(3).u16(1).i16(3);
        for i in 0..3 {
            w = vertex(w).f(&[i as f32, 0.0, 0.0]);
        }
        w = w.u16(0).u16(1).u16(2);
        // Skinned surface, three extra weights over two vertices.
        w = w.u8(0).u16(3).u16(1).i16(-1).u16(3);
        w = vertex(w).u8(0).u16(1).f(&[1.0, 1.0, 1.0]);
        w = vertex(w)
            .u8(1)
            .u16(2)
            .f(&[2.0, 0.0, 0.0])
            .u8(64)
            .u16(4)
            .f(&[0.0, 2.0, 0.0])
            .u16(49152);
        w = vertex(w)
            .u8(2)
            .u16(0)
            .f(&[0.0, 0.0, 0.0])
            .u8(128)
            .u16(1)
            .f(&[1.0, 0.0, 0.0])
            .u16(16384)
            .u16(2)
            .f(&[0.0, 1.0, 0.0])
            .u16(16384);
        w.u16(2).u16(1).u16(0).0
    }

    #[test]
    fn surfs_parse() {
        let s = parse_surfs(&surfs_file()).unwrap();
        assert_eq!(s.surfaces.len(), 2);
        let rigid = &s.surfaces[0];
        assert_eq!((rigid.tile_mode, rigid.bone), (2, Some(3)));
        assert_eq!(rigid.triangles, [[0, 2, 1]]);
        let v = &rigid.vertices[2];
        assert_eq!(v.bone, 3);
        assert_eq!(v.offset, [2.0, 0.0, 0.0]);
        assert_eq!(v.color, [255, 128, 64, 32]);
        assert_eq!(v.uv, [0.25, 0.75]);
        assert_eq!(v.binormal, [0.0, 1.0, 0.0]);
        assert_eq!(v.tangent, [1.0, 0.0, 0.0]);
        assert_eq!(v.primary_fraction(), 1.0);

        let skinned = &s.surfaces[1];
        assert_eq!(skinned.bone, None);
        assert_eq!(skinned.triangles, [[2, 0, 1]]);
        let [v0, v1, v2] = &skinned.vertices[..] else {
            panic!()
        };
        assert_eq!((v0.bone, v0.extras.len()), (1, 0));
        assert_eq!(v1.primary_fraction(), 0.25);
        assert_eq!(v1.extras[0].bone, 4);
        assert_eq!(v1.extras[0].offset, [0.0, 2.0, 0.0]);
        assert_eq!(v1.extras[0].fraction(), 0.75);
        assert_eq!(v2.extras.len(), 2);
        for v in [v0, v1, v2] {
            assert_eq!(v.weight_sum_65536(), 65536);
        }
        assert_eq!(s.max_bone(), Some(4));
    }

    #[test]
    fn surfs_reject_bad_files() {
        let good = surfs_file();
        for n in 0..good.len() {
            assert!(parse_surfs(&good[..n]).is_err(), "truncated at {n}");
        }
        let mut tail = good.clone();
        tail.push(0);
        assert!(parse_surfs(&tail).is_err());
        // The rigid triangle's first index past its three vertices.
        let mut idx = good.clone();
        let tri = 4 + 7 + 3 * 60;
        idx[tri] = 3;
        assert!(parse_surfs(&idx).is_err());
        // Blend count one short of the vertices' sum.
        let mut blend = good;
        let skinned = tri + 6 + 7;
        assert_eq!(blend[skinned], 3);
        blend[skinned] = 2;
        assert!(parse_surfs(&blend).is_err());
    }

    fn descriptor_file() -> Vec<u8> {
        let mut w = W::default()
            .u16(20)
            .u8(1)
            .f(&[-1.0, -2.0, -3.0, 4.0, 5.0, 6.0])
            .f(&[160.0])
            .s("body_lod0")
            .f(&[0.0])
            .s("body_lod1")
            .f(&[0.0])
            .s("")
            .f(&[0.0])
            .s("")
            .i32(0)
            .i32(1)
            .i32(1);
        w = w.f(&[0.0, 0.0, 1.0, 5.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        w = w
            .f(&[0.0, 0.0, 4.999, 1.0, 1.0, 5.001])
            .i32(2)
            .i32(0x2000_0010)
            .i32(0x0010_0000);
        w = w.u16(2).s("mtl_a").s("mtl_b").u16(1).s("mtl_c");
        w.f(&[0.0; 6]).f(&[-1.0, -1.0, -1.0, 3.0, 1.0, 1.0]).0
    }

    #[test]
    fn descriptor_parse() {
        let mut asked = String::new();
        let d = parse_descriptor(&descriptor_file(), |name| {
            asked = name.to_owned();
            Ok(2)
        })
        .unwrap();
        assert_eq!(asked, "body_lod0");
        assert_eq!(d.flags, 1);
        assert_eq!((d.mins, d.maxs), ([-1.0, -2.0, -3.0], [4.0, 5.0, 6.0]));
        let present: Vec<_> = d.present_lods().map(|l| l.name.as_str()).collect();
        assert_eq!(present, ["body_lod0", "body_lod1"]);
        assert_eq!(d.lods[0].dist, 160.0);
        assert_eq!(d.lods[0].materials, ["mtl_a", "mtl_b"]);
        assert_eq!(d.lods[1].materials, ["mtl_c"]);
        assert!(d.lods[2].materials.is_empty());
        assert_eq!(d.coll_lod, 0);
        let c = &d.coll_surfs[0];
        assert_eq!(c.tris.len(), 1);
        assert_eq!(c.tris[0].plane, [0.0, 0.0, 1.0, 5.0]);
        assert_eq!(c.tris[0].tvec, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(
            (c.bone, c.contents, c.surf_flags),
            (2, 0x2000_0010, 0x0010_0000)
        );
        assert_eq!(d.bone_infos.len(), 2);
        assert_eq!(d.bone_infos[0].radius_squared(), 0.0);
        let box1 = d.bone_infos[1];
        assert_eq!(box1.offset(), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(box1.radius_squared(), 4.0 + 1.0 + 1.0);
    }

    #[test]
    fn descriptor_rejects_bad_files() {
        let good = descriptor_file();
        assert!(parse_descriptor(&good, |_| Ok(1)).is_err(), "too few bones");
        assert!(
            parse_descriptor(&good, |_| Ok(3)).is_err(),
            "too many bones"
        );
        assert!(parse_descriptor(&good, |_| bail!("no parts")).is_err());
        for n in 0..good.len() {
            assert!(
                parse_descriptor(&good[..n], |_| Ok(2)).is_err(),
                "truncated at {n}"
            );
        }
        // A collision surface whose triangle count runs past the end.
        let mut huge = good;
        let num_tris = 2 + 1 + 24 + 4 + 10 + 4 + 10 + 4 + 1 + 4 + 1 + 4 + 4;
        assert_eq!(huge[num_tris], 1);
        huge[num_tris + 3] = 0x7f;
        assert!(parse_descriptor(&huge, |_| Ok(2)).is_err());
    }
}
