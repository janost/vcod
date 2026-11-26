//! Models and animations.

use super::material::{Material, material};
use super::misc::{PhysPreset, phys_preset};
use super::world::SharedBulk;
use super::{Id, asset, push};
use crate::walk::{Follow, Handle, S, W, u16s};
use crate::zone::{Block, Pointer};
use anyhow::Result;
use std::io::BufRead;

/// `XModel`, 0xDC.
#[derive(Clone, Debug, Default)]
pub struct XModel {
    pub name: String,
    pub num_bones: u8,
    pub num_root_bones: u8,
    /// Script string indices, `numBones` of them.
    pub bone_names: Vec<u16>,
    /// `numBones - numRootBones` parent offsets.
    pub parent_list: Vec<u8>,
    /// `XModelQuat` (4 x i16) per non-root bone.
    pub quats: Vec<u8>,
    /// Four floats per non-root bone.
    pub trans: Vec<u8>,
    pub part_classification: Vec<u8>,
    /// `DObjAnimMat`, 0x20 per bone.
    pub base_mat: Vec<u8>,
    /// `XBoneInfo`, 0x28 per bone: `bounds[2][3]`, `offset[3]`,
    /// `radiusSquared` (the hit boxes, cod4-player-anims.md 7).
    pub bone_info: Vec<u8>,
    pub surfaces: Vec<XSurface>,
    /// One per surface.
    pub materials: Vec<Option<Id<Material>>>,
    /// `lodInfo[4]`, the first `numLods` used.
    pub lods: Vec<XModelLod>,
    pub num_lods: u8,
    pub coll_lod: i8,
    pub coll_surfs: usize,
    /// The collision surfaces, kept by every walk: a map's static models
    /// take theirs from the common zones' models too.
    pub coll: Vec<XModelCollSurf>,
    pub contents: i32,
    pub radius: f32,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub flags: u8,
    pub phys_preset: Option<Id<PhysPreset>>,
    /// `physGeoms`, when the model has its own physics shapes.
    pub phys_geoms: Option<PhysGeomList>,
}

/// `PhysGeomList`, 0x2C: `count`, the geoms, then `PhysMass`.
#[derive(Clone, Debug, Default)]
pub struct PhysGeomList {
    pub geoms: Vec<PhysGeom>,
    /// `centerOfMass`, in the model's frame.
    pub center_of_mass: [f32; 3],
    /// `momentsOfInertia` and `productsOfInertia`, per unit of mass.
    pub moments: [f32; 3],
    pub products: [f32; 3],
}

/// `PhysGeomInfo`, 0x44: a brush (`BrushWrapper`) or a shape of `type`
/// (1 a box) at `offset` along `orientation`'s rows, `halfLengths` in size.
#[derive(Clone, Debug)]
pub struct PhysGeom {
    pub brush: Option<PhysBrush>,
    pub kind: i32,
    pub orientation: [[f32; 3]; 3],
    pub offset: [f32; 3],
    pub half_lengths: [f32; 3],
}

/// A `physGeoms` brush in the model's frame: `BrushWrapper`'s bounds and
/// its plane array at +0x4c (normal, dist), solid where every
/// `n . p <= dist`, empty where the walker dropped it; each side's own
/// plane, which the brush colliders read; and each face's neighbours as
/// they wind it, faces `0..6` the axial ones in 0x596ae0's order, then
/// the sides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhysBrush {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub planes: Vec<[f32; 4]>,
    pub side_planes: Vec<[f32; 4]>,
    pub adjacent: Vec<Vec<u8>>,
}

/// `XModelCollSurf_s`, 0x2C: its triangles (0x30 each: plane, `svec`,
/// `tvec`), bounds, bone, contents and surface flags.
#[derive(Clone, Debug)]
pub struct XModelCollSurf {
    pub tris: Vec<[f32; 12]>,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub bone: i32,
    pub contents: i32,
    pub surface_flags: i32,
}

/// `XModelLodInfo`, 0x1C.
#[derive(Clone, Copy, Debug)]
pub struct XModelLod {
    pub dist: f32,
    pub num_surfs: u16,
    pub surf_index: u16,
    pub part_bits: [u32; 4],
}

/// `XSurface`, 0x38.
#[derive(Clone, Debug)]
pub struct XSurface {
    pub tile_mode: u8,
    pub deformed: bool,
    pub vert_count: u16,
    pub tri_count: u16,
    pub base_tri_index: u16,
    pub base_vert_index: u16,
    /// Vertices with 1, 2, 3 and 4 bone weights.
    pub blend_counts: [u16; 4],
    /// `vertsBlend`: u16 bone data, `c0 + 3 c1 + 5 c2 + 7 c3` of them.
    pub verts_blend: SharedBulk,
    /// `GfxPackedVertex`, 0x20 each, in the vertex block.
    pub verts: SharedBulk,
    /// Three u16 per triangle, in the index block.
    pub tris: SharedBulk,
    /// `XRigidVertList`, 0xC each: bone offset, vertex and triangle runs.
    pub rigid: Vec<[u16; 4]>,
    pub part_bits: [u32; 4],
}

pub fn xmodel<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<XModel>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0xDC)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let nb = s.u8(4) as usize;
        let nonroot = nb.saturating_sub(s.u8(5) as usize);
        let bone_names = w.shared(s.ptr(8), 2, 2 * nb, "bone names", false)?;
        let parent_list = w.shared(s.ptr(12), 1, nonroot, "parent list", false)?;
        let quats = w.shared(s.ptr(16), 2, 8 * nonroot, "bone quats", false)?;
        let trans = w.shared(s.ptr(20), 4, 16 * nonroot, "bone trans", false)?;
        let part_classification = w.shared(s.ptr(24), 1, nb, "part classes", false)?;
        let base_mat = w.shared(s.ptr(28), 4, 32 * nb, "base mats", false)?;
        let nsurf = s.u8(6) as usize;
        let mut surfaces = Vec::new();
        if w.inline_only(s.ptr(32), 4)? {
            let a = w.load(0x38 * nsurf)?;
            for i in 0..nsurf {
                surfaces.push(xsurface(w, &a.elem(i, 0x38))?);
            }
        }
        let mut materials = Vec::new();
        if w.inline_only(s.ptr(36), 4)? {
            let a = w.load(4 * nsurf)?;
            for i in 0..nsurf {
                materials.push(material(w, a.ptr(4 * i), a.slot(4 * i))?);
            }
        }
        let coll_surfs = s.i32(156).max(0) as usize;
        let mut coll = Vec::new();
        if w.inline_only(s.ptr(152), 4)? {
            let a = w.load(0x2C * coll_surfs)?;
            for i in 0..coll_surfs {
                let c = a.elem(i, 0x2C);
                let mut tris = Vec::new();
                if w.inline_only(c.ptr(0), 4)? {
                    let b = w.bulk_kept(4, 0x30 * c.i32(4).max(0) as usize)?;
                    tris = b
                        .as_chunks::<0x30>()
                        .0
                        .iter()
                        .map(|t| std::array::from_fn(|k| crate::walk::f32_at(t, 4 * k)))
                        .collect();
                }
                coll.push(XModelCollSurf {
                    tris,
                    mins: c.vec3(8),
                    maxs: c.vec3(20),
                    bone: c.i32(32),
                    contents: c.i32(36),
                    surface_flags: c.i32(40),
                });
            }
        }
        let bone_info = w.shared(s.ptr(164), 4, 0x28 * nb, "bone info", false)?;
        let phys = phys_preset(w, s.ptr(212), s.slot(212))?;
        let phys_geoms = match w.follow(s.ptr(216), 4)? {
            Follow::Inline => Some(phys_geom_list(w)?),
            _ => None,
        };
        w.pop()?;
        let lods = (0..4)
            .map(|i| {
                let l = s.sub(40 + 0x1C * i, 0x1C);
                XModelLod {
                    dist: l.f32(0),
                    num_surfs: l.u16(4),
                    surf_index: l.u16(6),
                    part_bits: [l.u32(8), l.u32(12), l.u32(16), l.u32(20)],
                }
            })
            .collect();
        let bone_names = u16s(&bone_names.unwrap_or_default());
        Ok(push(
            &mut w.out.xmodels,
            XModel {
                name,
                num_bones: s.u8(4),
                num_root_bones: s.u8(5),
                bone_names,
                parent_list: parent_list.as_deref().unwrap_or_default().to_vec(),
                quats: quats.as_deref().unwrap_or_default().to_vec(),
                trans: trans.as_deref().unwrap_or_default().to_vec(),
                part_classification: part_classification.as_deref().unwrap_or_default().to_vec(),
                base_mat: base_mat.as_deref().unwrap_or_default().to_vec(),
                bone_info: bone_info.as_deref().unwrap_or_default().to_vec(),
                surfaces,
                materials,
                lods,
                num_lods: s.u8(196),
                coll_lod: s.i8(198),
                coll_surfs,
                coll,
                contents: s.i32(160),
                radius: s.f32(168),
                mins: s.vec3(172),
                maxs: s.vec3(184),
                flags: s.u8(208),
                phys_preset: phys,
                phys_geoms,
            },
        ))
    })
}

fn xsurface<R: BufRead>(w: &mut W<R>, s: &S) -> Result<XSurface> {
    let blend_counts = [s.u16(16), s.u16(18), s.u16(20), s.u16(22)];
    let [c0, c1, c2, c3] = blend_counts.map(usize::from);
    let blend = c0 + 3 * c1 + 5 * c2 + 7 * c3;
    let verts_blend = w.shared(s.ptr(24), 2, 2 * blend, "verts blend", true)?;
    let vert_count = s.u16(2);
    let tri_count = s.u16(4);
    w.push(Block::Vertex);
    let verts = w.shared(s.ptr(28), 16, 0x20 * vert_count as usize, "verts0", true)?;
    w.pop()?;
    let rigid = match w.follow(s.ptr(36), 4)? {
        Follow::Inline => {
            let at = w.here()?;
            let r = rigid_lists(w, s.u32(32) as usize)?;
            w.remember("rigid vert lists", at, r.clone());
            r
        }
        Follow::At(a) => w.recall("rigid vert lists", a).unwrap_or_default(),
        Follow::Null => Vec::new(),
    };
    w.push(Block::Index);
    let tris = w.shared(s.ptr(12), 16, 6 * tri_count as usize, "tris", true)?;
    w.pop()?;
    Ok(XSurface {
        tile_mode: s.u8(0),
        deformed: s.u8(1) != 0,
        vert_count,
        tri_count,
        base_tri_index: s.u16(8),
        base_vert_index: s.u16(10),
        blend_counts,
        verts_blend,
        verts,
        tris,
        rigid,
        part_bits: [s.u32(40), s.u32(44), s.u32(48), s.u32(52)],
    })
}

/// `XRigidVertList`, 0xC each, `n` of them, with their collision trees
/// (0x28: 0x10-byte nodes, u16 leafs).
fn rigid_lists<R: BufRead>(w: &mut W<R>, n: usize) -> Result<Vec<[u16; 4]>> {
    let a = w.load(0xC * n)?;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let e = a.elem(i, 0xC);
        out.push([e.u16(0), e.u16(2), e.u16(4), e.u16(6)]);
        match w.follow(e.ptr(8), 4)? {
            Follow::Inline => {
                let t = w.load(0x28)?;
                if w.inline_only(t.ptr(28), 16)? {
                    w.bulk(16, 0x10 * t.i32(24).max(0) as usize)?;
                }
                if w.inline_only(t.ptr(36), 2)? {
                    w.bulk(2, 2 * t.i32(32).max(0) as usize)?;
                }
            }
            Follow::At(_) => w.unresolved("collision tree"),
            Follow::Null => {}
        }
    }
    Ok(out)
}

/// `PhysGeomList`, 0x2C: `count` 0x44-byte `PhysGeomInfo`, each maybe
/// with a `BrushWrapper`, then the `PhysMass`.
fn phys_geom_list<R: BufRead>(w: &mut W<R>) -> Result<PhysGeomList> {
    let s = w.load(0x2C)?;
    let n = s.u32(0) as usize;
    let mut geoms = Vec::new();
    match w.follow(s.ptr(4), 4)? {
        Follow::Inline => {
            let a = w.load(0x44 * n)?;
            for i in 0..n {
                let g = a.elem(i, 0x44);
                let brush = if w.inline_only(g.ptr(0), 4)? {
                    Some(brush_wrapper(w)?)
                } else {
                    None
                };
                geoms.push(PhysGeom {
                    brush,
                    kind: g.i32(4),
                    orientation: [g.vec3(8), g.vec3(20), g.vec3(32)],
                    offset: g.vec3(44),
                    half_lengths: g.vec3(56),
                });
            }
        }
        Follow::At(_) => w.unresolved("phys geoms"),
        Follow::Null => {}
    }
    Ok(PhysGeomList {
        geoms,
        center_of_mass: s.vec3(8),
        moments: s.vec3(20),
        products: s.vec3(32),
    })
}

/// `BrushWrapper`, 0x50: bounds (+0, +16), sides, adjacency bytes, then
/// one `cplane_s` (0x14) per side at +76.
pub(crate) fn brush_wrapper<R: BufRead>(w: &mut W<R>) -> Result<PhysBrush> {
    let b = w.load(0x50)?;
    let sides = b.u32(28) as usize;
    let (side_bytes, side_planes) = if w.inline_only(b.ptr(32), 4)? {
        brush_sides_planes(w, sides)?
    } else {
        Default::default()
    };
    let edges = if w.inline_only(b.ptr(48), 1)? {
        w.bulk_kept(1, b.u32(72) as usize)?
    } else {
        Vec::new()
    };
    // Each face's run of the edges: the axial ones' first (+0x34) and
    // count (+0x40) at `[i & 1][i >> 1]`, a side's at +8 and +0xa.
    let mut runs: Vec<(i16, u8)> = (0..6)
        .map(|i| {
            let j = 3 * (i & 1) + (i >> 1);
            (b.i16(0x34 + 2 * j), b.u8(0x40 + j))
        })
        .collect();
    runs.extend(
        side_bytes
            .as_chunks::<0xC>()
            .0
            .iter()
            .map(|s| (i16::from_le_bytes([s[8], s[9]]), s[10])),
    );
    let adjacent = runs
        .iter()
        .map(|&(first, k)| {
            usize::try_from(first)
                .ok()
                .and_then(|f| edges.get(f..f + usize::from(k)))
                .map_or_else(Vec::new, <[u8]>::to_vec)
        })
        .collect();
    // A shared plane array may start inside another brush's.
    let planes: Vec<[f32; 4]> = w
        .shared(b.ptr(76), 4, 0x14 * sides, "brush planes", false)?
        .map(|p| {
            let f = |o: usize| crate::walk::f32_at(&p, o);
            (0..sides)
                .map(|i| {
                    [
                        f(i * 0x14),
                        f(i * 0x14 + 4),
                        f(i * 0x14 + 8),
                        f(i * 0x14 + 12),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    let side_planes = (0..sides)
        .filter_map(|i| {
            side_planes
                .get(i)
                .copied()
                .flatten()
                .or(planes.get(i).copied())
        })
        .collect();
    Ok(PhysBrush {
        mins: b.vec3(0),
        maxs: b.vec3(16),
        planes,
        side_planes,
        adjacent,
    })
}

/// `cbrushside_t`, 0xC, `n` of them, each plane inline or shared.
/// Returns the sides as stored.
pub(crate) fn brush_sides<R: BufRead>(w: &mut W<R>, n: usize) -> Result<Vec<u8>> {
    Ok(brush_sides_planes(w, n)?.0)
}

/// The sides as stored and each side's plane (normal, dist) when inline.
type SidesPlanes = (Vec<u8>, Vec<Option<[f32; 4]>>);

/// [`brush_sides`] and each side's inline plane.
fn brush_sides_planes<R: BufRead>(w: &mut W<R>, n: usize) -> Result<SidesPlanes> {
    let a = w.load(0xC * n)?;
    let mut planes = Vec::with_capacity(n);
    for i in 0..n {
        planes.push(match w.follow(a.elem(i, 0xC).ptr(0), 4)? {
            Follow::Inline => {
                let p = w.load(0x14)?;
                Some([p.f32(0), p.f32(4), p.f32(8), p.f32(12)])
            }
            _ => None,
        });
    }
    Ok((a.b, planes))
}

/// `XAnimParts`, 0x58.
#[derive(Clone, Debug)]
pub struct XAnim {
    pub name: String,
    pub num_frames: u16,
    pub looped: bool,
    pub delta: bool,
    /// Bones per part type; `bone_count[9]` is the total.
    pub bone_count: [u8; 10],
    pub framerate: f32,
    pub frequency: f32,
    pub asset_type: u8,
    /// Script string indices, `bone_count[9]` of them.
    pub names: Vec<u16>,
    /// `XAnimNotifyInfo`: script string name and time.
    pub notify: Vec<(u16, f32)>,
    pub data_byte: Vec<u8>,
    pub data_short: Vec<u8>,
    pub data_int: Vec<u8>,
    pub random_data_short: Vec<u8>,
    pub random_data_byte: Vec<u8>,
    pub random_data_int: Vec<u8>,
    /// u8 frame indices under 256 frames, u16 at or above.
    pub indices: Vec<u8>,
    /// The root's motion, `None` for a clip without one.
    pub root: Option<DeltaPart>,
}

pub fn xanim<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<XAnim>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x58)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let frames = s.u16(14);
        let wide = frames >= 256;
        let mut bone_count = [0u8; 10];
        bone_count.copy_from_slice(&s.b[18..28]);
        let mut names = Vec::new();
        if w.inline_only(s.ptr(48), 2)? {
            let b = w.bytes(2, 2 * bone_count[9] as usize)?;
            names = u16s(&b);
        }
        let mut notify = Vec::new();
        if w.inline_only(s.ptr(80), 4)? {
            let n = s.u8(28) as usize;
            let a = w.load(8 * n)?;
            notify = (0..n).map(|i| (a.u16(8 * i), a.f32(8 * i + 4))).collect();
        }
        let root = if w.inline_only(s.ptr(84), 4)? {
            Some(delta_part(w, wide)?)
        } else {
            None
        };
        let arr = |w: &mut W<R>, o: usize, align: u32, n: usize| -> Result<Vec<u8>> {
            Ok(if w.inline_only(s.ptr(o), align)? {
                w.bytes(align, n)?
            } else {
                Vec::new()
            })
        };
        let data_byte = arr(w, 52, 1, s.u16(4) as usize)?;
        let data_short = arr(w, 56, 2, 2 * s.u16(6) as usize)?;
        let data_int = arr(w, 60, 4, 4 * s.u16(8) as usize)?;
        let random_data_short = arr(w, 64, 2, 2 * s.u32(32) as usize)?;
        let random_data_byte = arr(w, 68, 1, s.u16(10) as usize)?;
        let random_data_int = arr(w, 72, 4, 4 * s.u16(12) as usize)?;
        let n = s.u32(36) as usize;
        let indices = if wide {
            arr(w, 76, 2, 2 * n)?
        } else {
            arr(w, 76, 1, n)?
        };
        w.pop()?;
        Ok(push(
            &mut w.out.xanims,
            XAnim {
                name,
                num_frames: frames,
                looped: s.u8(16) != 0,
                delta: s.u8(17) != 0,
                bone_count,
                framerate: s.f32(40),
                frequency: s.f32(44),
                asset_type: s.u8(29),
                names,
                notify,
                data_byte,
                data_short,
                data_int,
                random_data_short,
                random_data_byte,
                random_data_int,
                indices,
                root,
            },
        ))
    })
}

/// `XAnimDeltaPart` decoded: the root's motion over the clip, keyed on
/// frames (cod4-player-anims.md 2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeltaPart {
    /// Frame and the root's offset there: `mins + size * frame`.
    pub trans: Vec<(u16, [f32; 3])>,
    /// Frame and the root's yaw as a half quaternion's (z, w) shorts.
    pub rot: Vec<(u16, [i16; 2])>,
    /// The translation track as the zone stores it, what
    /// `XAnim_CalcDeltaForTime` lerps before it scales: `mins`, `size` and
    /// each key's quantized offset (empty for a one-key track).
    pub trans_mins: [f32; 3],
    pub trans_size: [f32; 3],
    pub trans_quant: Vec<[u16; 3]>,
}

/// `XAnimDeltaPart`, 8: a translation track and a rotation track, each a
/// variable-length struct whose frames follow it.
fn delta_part<R: BufRead>(w: &mut W<R>, wide: bool) -> Result<DeltaPart> {
    let d = w.load(8)?;
    let idx = if wide { 2 } else { 1 };
    let frames = |b: &[u8], n: usize| -> Vec<u16> {
        (0..n)
            .map(|k| {
                if wide {
                    u16::from_le_bytes([b[2 * k], b[2 * k + 1]])
                } else {
                    u16::from(b[k])
                }
            })
            .collect()
    };
    let mut out = DeltaPart::default();
    if w.inline_only(d.ptr(0), 4)? {
        // XAnimPartTrans: size u16, smallTrans u8, then frame0 (a vec3) or
        // mins, size, the frames slot and size + 1 indices.
        let h = w.load(4)?;
        let n = h.u16(0) as usize;
        if n > 0 {
            let f = w.load(28)?;
            let keys = frames(&w.bytes(1, idx * (n + 1))?, n + 1);
            let (mins, size) = (f.vec3(0), f.vec3(12));
            out.trans_mins = mins;
            out.trans_size = size;
            let small = h.u8(2) != 0;
            let data = if small {
                w.inline_only(f.ptr(24), 1)?
                    .then(|| w.bytes(1, 3 * (n + 1)))
                    .transpose()?
            } else {
                w.inline_only(f.ptr(24), 4)?
                    .then(|| w.bytes(4, 6 * (n + 1)))
                    .transpose()?
            };
            if let Some(b) = data {
                for (k, &frame) in keys.iter().enumerate() {
                    let v = |c: usize| -> f32 {
                        if small {
                            f32::from(b[3 * k + c])
                        } else {
                            f32::from(u16::from_le_bytes([b[6 * k + 2 * c], b[6 * k + 2 * c + 1]]))
                        }
                    };
                    let p = [0, 1, 2].map(|c| mins[c] + size[c] * v(c));
                    out.trans.push((frame, p));
                    out.trans_quant.push([0, 1, 2].map(|c| v(c) as u16));
                }
            }
        } else {
            let f = w.load(12)?;
            out.trans.push((0, f.vec3(0)));
        }
    }
    if w.inline_only(d.ptr(4), 4)? {
        // XAnimDeltaPartQuat: size u16, then frame0 (an XQuat2) or the
        // frames slot and size + 1 indices.
        let h = w.load(4)?;
        let n = h.u16(0) as usize;
        if n > 0 {
            let f = w.load(4)?;
            let keys = frames(&w.bytes(1, idx * (n + 1))?, n + 1);
            if w.inline_only(f.ptr(0), 4)? {
                let b = w.bytes(4, 4 * (n + 1))?;
                for (k, &frame) in keys.iter().enumerate() {
                    let i = |o: usize| i16::from_le_bytes([b[4 * k + o], b[4 * k + o + 1]]);
                    out.rot.push((frame, [i(0), i(2)]));
                }
            }
        } else {
            let f = w.load(4)?;
            out.rot.push((0, [f.i16(0), f.i16(2)]));
        }
    }
    Ok(out)
}
