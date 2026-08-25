//! v14 `xmodel`/`xmodelparts`/`xmodelsurfs` parsing and bind-pose bake, plus
//! the weapon file parser. Layouts: `docs/research/xmodel-v14-format.md`.

use crate::pk3::Pk3Fs;
use anyhow::{anyhow, ensure, Result};
use glam::{DMat3, DVec3, Quat, Vec3};
use std::collections::HashMap;

pub struct XModel {
    /// LOD0 name, used only in diagnostics.
    pub lod: String,
    pub surfaces: Vec<Surface>,
    pub materials: Vec<String>, // skin filenames, index-aligned with surfaces
    pub bones: Vec<Bone>,
    /// The descriptor's collision mesh, baked to model space. Empty for the
    /// models that have none (`collision_lod == -1`).
    pub collision: Vec<CollSurf>,
}

/// One collision surface. Only `contents & collision::CONTENTS_SOLID` stops
/// the player; tree canopies and signs carry 0, lamp glass 0x10.
/// docs/research/xmodel-v14-format.md, "Collision block".
pub struct CollSurf {
    pub contents: u32,
    pub flags: u32,
    pub tris: Vec<[Vec3; 3]>,
}

pub struct Surface {
    pub verts: Vec<VmVert>,
    pub indices: Vec<u16>,
    pub material: usize,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VmVert {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Model-local bone ids, padded with 0.
    pub bone_indices: [u8; 4],
    /// Sums to 1.0, padded with 0.0.
    pub bone_weights: [f32; 4],
}

/// `pos`/`rot` are the composed world bind; `local_*` the unbaked local,
/// after the viewhands zeroing.
pub struct Bone {
    pub name: String,
    pub parent: i32,
    pub pos: Vec3,
    pub rot: Quat,
    pub local_pos: Vec3,
    pub local_rot: Quat,
}

/// Little-endian cursor; bounds-checked, never panics on a truncated file.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(
            self.pos + n <= self.data.len(),
            "unexpected EOF at offset {} reading {n} bytes (len {})",
            self.pos,
            self.data.len()
        );
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n)?;
        Ok(())
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn i8(&mut self) -> Result<i8> {
        Ok(self.take(1)?[0] as i8)
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub(crate) fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(crate) fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(crate) fn cstr(&mut self) -> Result<String> {
        let rest = &self.data[self.pos..];
        let nul = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| anyhow!("unterminated string at offset {}", self.pos))?;
        let s = String::from_utf8_lossy(&rest[..nul]).into_owned();
        self.pos += nul + 1;
        Ok(s)
    }
}

#[derive(Debug)]
struct Descriptor {
    /// First present LOD; its last byte is the model type.
    lod0: String,
    /// Index-aligned with surfaces.
    materials: Vec<String>,
    collision: Vec<RawCollSurf>,
}

/// A collision surface before the bone bake; triangles in bone space.
#[derive(Debug)]
struct RawCollSurf {
    bone: i32,
    contents: u32,
    flags: u32,
    tris: Vec<[Vec3; 3]>,
}

/// A collision triangle is stored as its plane and two barycentric edge
/// planes, `u = svec·p - svec.w` and `v = tvec·p - tvec.w`; the vertices are
/// the points at (u,v) = (0,0), (1,0), (0,1). Emitted as (v0, v2, v1) so the
/// winding's cross product matches the stored normal. `None` when the three
/// planes are not independent.
fn coll_tri_verts(rec: &[f32; 12]) -> Option<[Vec3; 3]> {
    let row = |i: usize| DVec3::new(rec[i] as f64, rec[i + 1] as f64, rec[i + 2] as f64);
    let a = DMat3::from_cols(row(0), row(4), row(8)).transpose();
    let det = a.determinant();
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let inv = a.inverse();
    let (d, sw, tw) = (rec[3] as f64, rec[7] as f64, rec[11] as f64);
    let at = |u: f64, v: f64| (inv * DVec3::new(d, u + sw, v + tw)).as_vec3();
    Some([at(0.0, 0.0), at(0.0, 1.0), at(1.0, 0.0)])
}

fn parse_descriptor(data: &[u8]) -> Result<Descriptor> {
    let mut r = Reader::new(data);
    let version = r.u16()?;
    ensure!(
        version == 14,
        "unsupported xmodel version {version}, expected 14"
    );
    for _ in 0..6 {
        r.f32()?; // mins[3], maxs[3]
    }
    let mut lod_names = Vec::with_capacity(3);
    for _ in 0..3 {
        r.f32()?; // dist
        lod_names.push(r.cstr()?);
    }
    r.skip(4)?; // collision LOD (i32), -1 when there is no collision
    let surf_count = r.u32()?;
    let mut collision = Vec::with_capacity(surf_count.min(64) as usize);
    let mut degenerate = 0usize;
    for _ in 0..surf_count {
        let tri_count = r.u32()?;
        let mut tris = Vec::with_capacity(tri_count.min(1024) as usize);
        for _ in 0..tri_count {
            let mut rec = [0f32; 12];
            for v in &mut rec {
                *v = r.f32()?;
            }
            match coll_tri_verts(&rec) {
                Some(t) => tris.push(t),
                None => degenerate += 1,
            }
        }
        r.skip(24)?; // mins[3], maxs[3], bone space
        let bone = r.i32()?;
        let contents = r.u32()?;
        let flags = r.u32()?;
        collision.push(RawCollSurf {
            bone,
            contents,
            flags,
            tris,
        });
    }
    if degenerate > 0 {
        log::debug!("xmodel: dropped {degenerate} degenerate collision triangles");
    }
    let mut lod0 = None;
    let mut materials = Vec::new();
    for name in &lod_names {
        if name.is_empty() {
            continue;
        }
        let tex_count = r.u16()?;
        let mut mats = Vec::with_capacity(tex_count as usize);
        for _ in 0..tex_count {
            mats.push(r.cstr()?);
        }
        if lod0.is_none() {
            lod0 = Some(name.clone());
            materials = mats;
        }
    }
    let lod0 = lod0.ok_or_else(|| anyhow!("xmodel descriptor has no LOD present"))?;
    Ok(Descriptor {
        lod0,
        materials,
        collision,
    })
}

/// Hands bones whose stored bind position is a placeholder; zeroed before
/// composing worlds. docs/research/xmodel-v14-format.md, "xmodelparts/<lod>".
fn is_viewhands_posed(name: &str) -> bool {
    matches!(name, "tag_view" | "tag_torso" | "tag_weapon")
        || name.starts_with("bip01 ")
        || name.contains("webbing")
        || name.starts_with("r cuff")
        || name.starts_with("r wrist")
}

/// `model_type` is the LOD name's last byte; `'4'` (viewhands) zeroes the
/// placeholder bind positions.
fn parse_parts(data: &[u8], model_type: u8) -> Result<Vec<Bone>> {
    let mut r = Reader::new(data);
    let version = r.u16()?;
    ensure!(
        version == 14,
        "unsupported xmodelparts version {version}, expected 14"
    );
    let num_bones = r.u16()? as usize; // non-root
    let num_root = r.u16()? as usize;
    let total = num_root + num_bones;

    let mut locals: Vec<(i32, Vec3, Quat)> = Vec::with_capacity(total);
    locals.resize(num_root, (-1, Vec3::ZERO, Quat::IDENTITY));
    for _ in 0..num_bones {
        let parent = r.i8()? as i32;
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let (rx, ry, rz) = (r.i16()?, r.i16()?, r.i16()?);
        let (qx, qy, qz) = (
            rx as f32 / 32768.0,
            ry as f32 / 32768.0,
            rz as f32 / 32768.0,
        );
        let qw = (1.0 - qx * qx - qy * qy - qz * qz).max(0.0).sqrt();
        locals.push((parent, pos, Quat::from_xyzw(qx, qy, qz, qw)));
    }

    // The viewhands zeroing needs the name, so it happens here, before this
    // bone's world is composed. Parents precede children in file order.
    let is_viewhands = model_type == b'4';
    let mut bones: Vec<Bone> = Vec::with_capacity(total);
    for &(parent, mut local_pos, local_rot) in &locals {
        let name = r.cstr()?;
        r.skip(24)?;
        if is_viewhands && is_viewhands_posed(&name) {
            local_pos = Vec3::ZERO;
        }
        let (pos, rot) = if parent >= 0 {
            let p = bones
                .get(parent as usize)
                .ok_or_else(|| anyhow!("bone '{name}' references parent {parent} out of range"))?;
            (p.pos + p.rot * local_pos, p.rot * local_rot)
        } else {
            (local_pos, local_rot)
        };
        bones.push(Bone {
            name,
            parent,
            pos,
            rot,
            local_pos,
            local_rot,
        });
    }
    r.skip(total)?; // per-bone u8 table (part group)
    Ok(bones)
}

/// Ported from mauserzjeh/cod-asset-importer (GPL-3.0).
fn decode_tri_strip(r: &mut Reader, triangle_count: usize) -> Result<Vec<u16>> {
    let mut tris: Vec<u16> = Vec::new();
    while tris.len() / 3 < triangle_count {
        let count = r.u8()? as usize;
        let i1 = r.u16()?;
        let mut i2 = r.u16()?;
        let mut i3 = r.u16()?;
        if i1 != i2 && i1 != i3 && i2 != i3 {
            tris.extend([i3, i2, i1]);
        }
        let mut i = 3;
        while i < count {
            let i4 = i3;
            let i5 = r.u16()?;
            if i4 != i2 && i4 != i5 && i2 != i5 {
                tris.extend([i5, i2, i4]);
            }
            let v = i + 1;
            if v >= count {
                break;
            }
            i2 = i5;
            i3 = r.u16()?;
            if i4 != i2 && i4 != i3 && i2 != i3 {
                tris.extend([i3, i2, i4]);
            }
            i = v + 1;
        }
    }
    Ok(tris)
}

/// Bakes every vertex into model space through its primary bone; `bones`
/// must hold composed world binds.
fn parse_surfs(data: &[u8], bones: &[Bone]) -> Result<Vec<Surface>> {
    let mut r = Reader::new(data);
    let version = r.u16()?;
    ensure!(
        version == 14,
        "unsupported xmodelsurfs version {version}, expected 14"
    );
    let surface_count = r.u16()? as usize;
    let mut surfaces = Vec::with_capacity(surface_count);
    for si in 0..surface_count {
        r.skip(1)?;
        let vertex_count = r.u16()? as usize;
        let triangle_count = r.u16()? as usize;
        r.skip(2)?;
        let default_bone = r.u16()?;
        let rigged = default_bone == u16::MAX;
        if rigged {
            r.skip(4)?; // rigged-only header word
        }

        let indices = decode_tri_strip(&mut r, triangle_count)?;
        ensure!(
            indices.iter().all(|&i| (i as usize) < vertex_count),
            "surface {si} has a strip index past its {vertex_count} vertices"
        );

        struct RawVert {
            normal: [f32; 3],
            uv: [f32; 2],
            bone: u16,
            weight_count: u16,
            local_pos: Vec3,
            primary_influence: f32,
        }
        let mut raws = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            let normal = [r.f32()?, r.f32()?, r.f32()?];
            let uv = [r.f32()?, r.f32()?];
            let (weight_count, bone) = if rigged {
                (r.u16()?, r.u16()?)
            } else {
                (0u16, default_bone)
            };
            let local_pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
            let primary_influence = if weight_count != 0 { r.f32()? } else { 1.0 };
            raws.push(RawVert {
                normal,
                uv,
                bone,
                weight_count,
                local_pos,
                primary_influence,
            });
        }
        // Extra weights follow as a second pass over all vertices.
        let mut extras: Vec<Vec<(u16, f32)>> = Vec::with_capacity(vertex_count);
        for rv in &raws {
            let mut vert_extras = Vec::with_capacity(rv.weight_count as usize);
            for _ in 0..rv.weight_count {
                let bone = r.u16()?;
                r.skip(12)?; // per-weight bone-local position, unused
                let influence = r.f32()?;
                vert_extras.push((bone, influence));
            }
            extras.push(vert_extras);
        }

        let mut verts = Vec::with_capacity(vertex_count);
        for (rv, vert_extras) in raws.iter().zip(&extras) {
            let bone = bones.get(rv.bone as usize).ok_or_else(|| {
                anyhow!(
                    "surface {si} vertex references bone {} out of range",
                    rv.bone
                )
            })?;
            let pos = bone.rot * rv.local_pos + bone.pos;
            let normal = bone.rot * Vec3::from_array(rv.normal);

            let mut weights: Vec<(u16, f32)> = Vec::with_capacity(1 + vert_extras.len());
            weights.push((rv.bone, rv.primary_influence));
            weights.extend(vert_extras.iter().copied());
            weights.sort_by(|a, b| b.1.total_cmp(&a.1));
            weights.truncate(4);
            let total: f32 = weights.iter().map(|(_, w)| w).sum();
            let norm = if total > 0.0 { 1.0 / total } else { 0.0 };

            let mut bone_indices = [0u8; 4];
            let mut bone_weights = [0f32; 4];
            for (i, (b, w)) in weights.iter().enumerate() {
                ensure!(
                    *b <= u8::MAX as u16,
                    "surface {si} bone id {b} does not fit u8"
                );
                bone_indices[i] = *b as u8;
                bone_weights[i] = w * norm;
            }

            verts.push(VmVert {
                pos: pos.to_array(),
                normal: normal.to_array(),
                uv: rv.uv,
                bone_indices,
                bone_weights,
            });
        }
        surfaces.push(Surface {
            verts,
            indices,
            material: si,
        });
    }
    Ok(surfaces)
}

/// `name` is the LOD0 name; its last byte is the model type.
fn parse_model(name: &str, desc: &[u8], parts: &[u8], surfs: &[u8]) -> Result<XModel> {
    let descriptor = parse_descriptor(desc)?;
    let model_type = *name
        .as_bytes()
        .last()
        .ok_or_else(|| anyhow!("empty model name"))?;
    let bones = parse_parts(parts, model_type)?;
    let surfaces = parse_surfs(surfs, &bones)?;
    ensure!(
        surfaces.len() == descriptor.materials.len(),
        "surface count {} does not match material count {}",
        surfaces.len(),
        descriptor.materials.len()
    );
    let mut collision = Vec::with_capacity(descriptor.collision.len());
    for s in descriptor.collision {
        let bone = usize::try_from(s.bone)
            .ok()
            .and_then(|b| bones.get(b))
            .ok_or_else(|| anyhow!("collision surface references bone {} out of range", s.bone))?;
        let tris = s
            .tris
            .iter()
            .map(|t| t.map(|v| bone.rot * v + bone.pos))
            .collect();
        collision.push(CollSurf {
            contents: s.contents,
            flags: s.flags,
            tris,
        });
    }
    Ok(XModel {
        lod: name.to_string(),
        surfaces,
        materials: descriptor.materials,
        bones,
        collision,
    })
}

/// `name` is the `xmodel/` entry, e.g. `viewmodel_kar98k`.
pub fn load(fs: &Pk3Fs, name: &str) -> Result<XModel> {
    let read = |path: String| -> Result<Vec<u8>> {
        fs.read(&path)
            .ok_or_else(|| anyhow!("{path} not found in pk3s"))
    };
    let desc = read(format!("xmodel/{name}"))?;
    let lod0 = parse_descriptor(&desc)?.lod0;
    let parts = read(format!("xmodelparts/{lod0}"))?;
    let surfs = read(format!("xmodelsurfs/{lod0}"))?;
    parse_model(&lod0, &desc, &parts, &surfs)
}

/// The hands ship with all four surfaces on `viewhands@default.jpg`, a white
/// placeholder the engine re-skins at runtime. Substitutes the hand atlas and
/// the Wehrmacht sleeve, only when the materials are exactly the four
/// placeholders. docs/research/xmodel-v14-format.md, "`xmodel/<name>` (descriptor)".
pub fn apply_viewhands_placeholder_override(model: &mut XModel) {
    const PLACEHOLDER: &str = "viewhands@default.jpg";
    if model.materials.len() != 4 || model.materials.iter().any(|m| m != PLACEHOLDER) {
        log::debug!(
            "{}: materials aren't the 4x viewhands placeholder, leaving as-is",
            model.lod
        );
        return;
    }
    model.materials[0] = "viewhands@hand.dds".to_string();
    model.materials[1] = "viewhands@hand.dds".to_string();
    model.materials[2] = "viewhands@vsleeve_whermact.tga".to_string();
    model.materials[3] = "viewhands@vsleeve_whermact.tga".to_string();
}

/// `\key\value...` weapon file. The shipped files open with a bare
/// `WEAPONFILE` token that would shift every pair by one; it is dropped.
pub fn parse_weapon(text: &str) -> HashMap<String, String> {
    let mut toks = text.trim_start_matches('\\').split('\\').peekable();
    if toks
        .peek()
        .is_some_and(|t| t.eq_ignore_ascii_case("WEAPONFILE"))
    {
        toks.next();
    }
    let mut map = HashMap::new();
    while let (Some(k), Some(v)) = (toks.next(), toks.next()) {
        map.insert(k.to_string(), v.to_string());
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a triangle the way the descriptor does and decodes it back.
    #[test]
    fn collision_triangle_round_trips_through_plane_form() {
        use glam::Mat3;
        let (a, b, c) = (
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(5.0, 2.5, 3.5),
            Vec3::new(1.5, 7.0, 2.0),
        );
        let n = (c - a).cross(b - a).normalize();
        // rows n, b-a, c-a: s has u(a)=0, u(b)=1, u(c)=0 and lies in the plane
        let inv = Mat3::from_cols(n, b - a, c - a).transpose().inverse();
        let s = inv * Vec3::Y;
        let t = inv * Vec3::Z;
        let rec = [
            n.x,
            n.y,
            n.z,
            n.dot(a),
            s.x,
            s.y,
            s.z,
            s.dot(a),
            t.x,
            t.y,
            t.z,
            t.dot(a),
        ];
        let v = coll_tri_verts(&rec).unwrap();
        assert!(v[0].abs_diff_eq(a, 1e-3), "{v:?}");
        assert!(v[1].abs_diff_eq(c, 1e-3), "{v:?}");
        assert!(v[2].abs_diff_eq(b, 1e-3), "{v:?}");
        let wind = (v[1] - v[0]).cross(v[2] - v[0]).normalize();
        assert!(wind.abs_diff_eq(n, 1e-3));
        let flat = [n.x, n.y, n.z, 0.0, n.x, n.y, n.z, 0.0, n.x, n.y, n.z, 0.0];
        assert!(coll_tri_verts(&flat).is_none());
    }

    #[test]
    fn crate_misc1a_collision_block() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let m = load(&fs, "crate_misc1a").unwrap();
        let counts: Vec<usize> = m.collision.iter().map(|s| s.tris.len()).collect();
        assert_eq!(counts, [16, 12]);
        assert!(m
            .collision
            .iter()
            .all(|s| s.contents == crate::collision::CONTENTS_SOLID));
        // first floor triangle: half of the 24x48 base at z = 1.33
        let t = m.collision[0].tris[0];
        let close = |v: Vec3, x: f32, y: f32| v.abs_diff_eq(Vec3::new(x, y, 1.331), 0.01);
        assert!(close(t[0], -12.028, 24.055), "{t:?}");
        assert!(close(t[1], 12.028, -24.055), "{t:?}");
        assert!(close(t[2], -12.028, -24.055), "{t:?}");
    }

    #[test]
    fn tree_trunk_is_solid_and_canopy_is_not() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let m = load(&fs, "tree_shortspruce").unwrap();
        assert_eq!(m.collision.len(), 5);
        assert_eq!(m.collision[0].tris.len(), 163);
        assert_eq!(m.collision[0].contents, crate::collision::CONTENTS_SOLID);
        assert!(m.collision[1..].iter().all(|s| s.contents == 0));
        let trunk = m.collision[0]
            .tris
            .iter()
            .flatten()
            .fold(Vec3::ZERO, |m, v| m.max(v.abs()));
        assert!(
            trunk.x < 13.0 && trunk.y < 13.0 && trunk.z > 200.0,
            "{trunk}"
        );
    }

    /// Surfaces on non-root bones are stored in bone space; the bake puts the
    /// tank's tracks on the ground instead of 23 units under it.
    #[test]
    fn multi_bone_collision_bakes_through_bone_binds() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let m = load(&fs, "static_vehicle_tank_tiger_d").unwrap();
        assert!(m.collision.iter().any(|s| !s.tris.is_empty()));
        let min_z = m
            .collision
            .iter()
            .flat_map(|s| s.tris.iter().flatten())
            .map(|v| v.z)
            .fold(f32::MAX, f32::min);
        assert!((-3.0..1.0).contains(&min_z), "{min_z}");
    }

    fn err_of<T>(r: Result<T>) -> String {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn parses_weapon_key_values() {
        let map =
            parse_weapon(r"\gunModel\viewmodel_kar98k\handModel\viewmodel_hands_new\twoHanded\1");
        assert_eq!(map.get("gunModel").unwrap(), "viewmodel_kar98k");
        assert_eq!(map.get("handModel").unwrap(), "viewmodel_hands_new");
        let map = parse_weapon("gunModel\\a\\handModel\\b");
        assert_eq!(map.get("gunModel").unwrap(), "a");
        let map = parse_weapon(r"WEAPONFILE\weaponType\bullet\gunModel\viewmodel_kar98k");
        assert_eq!(map.get("gunModel").unwrap(), "viewmodel_kar98k");
        assert_eq!(map.get("weaponType").unwrap(), "bullet");
    }

    #[test]
    fn parses_the_shipped_kar98k_weapon_file() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let text = fs.read("weapons/mp/kar98k_mp").unwrap();
        let map = parse_weapon(&String::from_utf8_lossy(&text));
        assert_eq!(map.get("gunModel").unwrap(), "viewmodel_kar98k");
        assert_eq!(map.get("handModel").unwrap(), "viewmodel_hands_new");
    }

    #[test]
    fn rejects_wrong_version() {
        let err = parse_descriptor(&20u16.to_le_bytes())
            .unwrap_err()
            .to_string();
        assert!(err.contains("14"), "got: {err}");
    }

    /// Descriptor, parts and surfs of a one-surface model: two bones
    /// (root, child at +Z 2), 3 verts on the child, one strip of `strip`.
    fn synthetic_model(strip: [u16; 3]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        // descriptor: one LOD "m2", one texture
        let mut desc = Vec::new();
        desc.extend(14u16.to_le_bytes());
        for v in [-10f32, -10.0, -10.0, 10.0, 10.0, 10.0] {
            desc.extend(v.to_le_bytes());
        }
        desc.extend(0f32.to_le_bytes());
        desc.extend(b"m2\0"); // LOD0
        desc.extend(0f32.to_le_bytes());
        desc.push(0); // empty LOD
        desc.extend(0f32.to_le_bytes());
        desc.push(0); // empty LOD
        desc.extend((-1i32).to_le_bytes()); // collision lod
        desc.extend(0u32.to_le_bytes()); // pad_count 0
        desc.extend(1u16.to_le_bytes());
        desc.extend(b"tex@a.dds\0"); // materials

        // parts: one root, one child at +Z 2, identity rotations
        let mut parts = Vec::new();
        parts.extend(14u16.to_le_bytes());
        parts.extend(1u16.to_le_bytes()); // num_bones
        parts.extend(1u16.to_le_bytes()); // num_root
        parts.extend((0i8).to_le_bytes()); // parent = root
        for v in [0f32, 0.0, 2.0] {
            parts.extend(v.to_le_bytes());
        }
        for _ in 0..3 {
            parts.extend(0i16.to_le_bytes()); // identity quat
        }
        parts.extend(b"root\0");
        parts.extend([0u8; 24]);
        parts.extend(b"child\0");
        parts.extend([0u8; 24]);
        parts.extend([0u8; 2]); // per-bone u8 table

        // surfs: 1 surface, 3 verts, 1 tri, unrigged on bone 1 (child)
        let mut surfs = Vec::new();
        surfs.extend(14u16.to_le_bytes());
        surfs.extend(1u16.to_le_bytes()); // surface_count
        surfs.push(0); // skip 1
        surfs.extend(3u16.to_le_bytes()); // verts
        surfs.extend(1u16.to_le_bytes()); // tris
        surfs.extend([0u8; 2]); // skip 2
        surfs.extend(1u16.to_le_bytes()); // default_bone = 1
        surfs.push(3); // strip: count 3
        for i in strip {
            surfs.extend(i.to_le_bytes());
        }
        for i in 0..3u32 {
            for v in [0f32, 0.0, 1.0] {
                surfs.extend(v.to_le_bytes()); // normal +Z
            }
            for v in [0.25f32, 0.75] {
                surfs.extend(v.to_le_bytes()); // uv
            }
            for v in [i as f32, 0.0, 0.0] {
                surfs.extend(v.to_le_bytes()); // pos (bone space)
            }
        }

        (desc, parts, surfs)
    }

    #[test]
    fn parses_synthetic_model() {
        let (desc, parts, surfs) = synthetic_model([0, 1, 2]);
        let m = parse_model("m2", &desc, &parts, &surfs).unwrap();
        assert_eq!(m.materials, vec!["tex@a.dds".to_string()]);
        assert_eq!(m.bones.len(), 2);
        assert_eq!(m.bones[1].name, "child");
        let s = &m.surfaces[0];
        assert_eq!(s.indices, vec![2, 1, 0]); // strip emits reversed
        assert_eq!(s.verts[1].pos, [1.0, 0.0, 2.0]); // baked through the child at (0,0,2)
        assert_eq!(s.verts[0].uv, [0.25, 0.75]); // no v flip
        assert_eq!(s.verts[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(s.verts[0].bone_indices, [1, 0, 0, 0]);
        assert_eq!(s.verts[0].bone_weights, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(m.bones[1].local_pos, Vec3::new(0.0, 0.0, 2.0));
        assert_eq!(m.bones[1].local_rot, Quat::IDENTITY);
    }

    #[test]
    fn rejects_strip_index_past_vertex_count() {
        let (desc, parts, surfs) = synthetic_model([0, 1, 5]);
        let err = err_of(parse_model("m2", &desc, &parts, &surfs));
        assert!(err.contains("index"), "got: {err}");
    }

    #[test]
    fn loads_real_kar98k_and_hands() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let gun = load(&fs, "viewmodel_kar98k").unwrap();
        assert_eq!(gun.surfaces.len(), 5);
        assert_eq!(gun.materials.len(), 5);
        assert_eq!(gun.bones.len(), 6);
        assert_eq!(gun.bones[0].name, "tag_weapon");
        assert_eq!(
            gun.surfaces.iter().map(|s| s.verts.len()).sum::<usize>(),
            1125
        );
        assert_eq!(
            gun.surfaces.iter().map(|s| s.indices.len()).sum::<usize>(),
            1001 * 3
        );

        let hands = load(&fs, "viewmodel_hands_new").unwrap();
        assert_eq!(hands.surfaces.len(), 4);
        assert_eq!(hands.bones.len(), 55);
        assert_eq!(
            hands.surfaces.iter().map(|s| s.verts.len()).sum::<usize>(),
            1914
        );
        assert_eq!(
            hands
                .surfaces
                .iter()
                .map(|s| s.indices.len())
                .sum::<usize>(),
            2924 * 3
        );

        // descriptor bounds (±36.2 gun, ±23.7 hands) plus 0.5 slack
        for (m, bound) in [(&gun, 36.7), (&hands, 24.2)] {
            for s in &m.surfaces {
                for v in &s.verts {
                    assert!(
                        v.pos.iter().all(|c| c.abs() <= bound),
                        "vert {:?} outside",
                        v.pos
                    );
                }
            }
            for s in &m.surfaces {
                assert!(s.indices.iter().all(|&i| (i as usize) < s.verts.len()));
            }
        }

        for m in [&gun, &hands] {
            for s in &m.surfaces {
                for v in &s.verts {
                    let sum: f32 = v.bone_weights.iter().sum();
                    assert!((sum - 1.0).abs() < 1e-3, "weights {:?}", v.bone_weights);
                    assert!(v.bone_indices.iter().all(|&b| (b as usize) < m.bones.len()));
                }
            }
        }
        assert!(hands
            .surfaces
            .iter()
            .flat_map(|s| &s.verts)
            .any(|v| v.bone_weights[1] > 0.0));
    }

    #[test]
    fn viewhands_placeholder_override_substitutes_real_textures() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let mut hands = load(&fs, "viewmodel_hands_new").unwrap();
        assert_eq!(
            hands.materials,
            vec!["viewhands@default.jpg".to_string(); 4],
            "expected the shipped hands model to still be the blank placeholder"
        );

        apply_viewhands_placeholder_override(&mut hands);
        assert_eq!(
            hands.materials,
            vec![
                "viewhands@hand.dds".to_string(),
                "viewhands@hand.dds".to_string(),
                "viewhands@vsleeve_whermact.tga".to_string(),
                "viewhands@vsleeve_whermact.tga".to_string(),
            ]
        );

        use crate::assets::{load_skin_image, ImageData};
        let hand = load_skin_image(&fs, "viewhands@hand.dds");
        assert!(
            matches!(hand.data, ImageData::Bc { .. }),
            "expected hand.dds to decode as a BC texture"
        );
        assert_ne!(
            (hand.width, hand.height),
            (64, 64),
            "must not be the checkerboard fallback"
        );

        let sleeve = load_skin_image(&fs, "viewhands@vsleeve_whermact.tga");
        assert!(
            matches!(sleeve.data, ImageData::Rgba8(_)),
            "expected the sleeve tga to decode as Rgba8"
        );
        assert_ne!(
            (sleeve.width, sleeve.height),
            (64, 64),
            "must not be the checkerboard fallback"
        );
    }

    #[test]
    fn viewhands_override_leaves_non_placeholder_models_untouched() {
        let mut m = XModel {
            lod: "test".into(),
            surfaces: Vec::new(),
            materials: vec!["some@other.dds".to_string(); 4],
            bones: Vec::new(),
            collision: Vec::new(),
        };
        let before = m.materials.clone();
        apply_viewhands_placeholder_override(&mut m);
        assert_eq!(m.materials, before);

        let mut m3 = XModel {
            lod: "test".into(),
            surfaces: Vec::new(),
            materials: vec!["viewhands@default.jpg".to_string(); 3],
            bones: Vec::new(),
            collision: Vec::new(),
        };
        let before3 = m3.materials.clone();
        apply_viewhands_placeholder_override(&mut m3);
        assert_eq!(m3.materials, before3);
    }
}
