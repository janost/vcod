//! CoD2's xmodel v20 as a [`vcod_common::model::ModelFormat`]: the raw
//! records of [`crate::xmodel`] baked into the neutral [`Model`] through the
//! engine's base pose. docs/research/cod2-xmodel-v20-format.md.
//!
//! A viewmodel's or viewhands' zeroed bind (types `2` and `4` store every
//! child translation as zero, doc "Viewmodels and hands") is rebuilt from
//! its per-weight offsets ([`recover_bind`]). The per-vertex colour goes
//! in [`Vertex::color`], the tangent and binormal through the base pose as
//! the normal does.
use crate::xmodel::{self, AnimMat, Descriptor, Parts, Surfs};
use anyhow::{Result, anyhow, ensure};
use glam::{Quat, Vec3};
use std::collections::BTreeMap;
use vcod_common::asset_source::AssetSource;
use vcod_common::model::{self, Bone, CollSurf, Lod, Model, ModelFormat, Surface, Vertex};

/// The loader's bounds growth per collision surface (Linux 0x081507a0).
const COLL_BOUNDS_GROWTH: f32 = 0.001;
/// The contents bits the loader keeps (Linux 0x080c69d0).
const COLL_CONTENTS_MASK: u32 = 0xdfff_fffb;

/// xmodel version 20.
pub struct V20;

/// Adds [`V20`] to [`vcod_common::model::load`]'s dispatch; safe to call
/// more than once.
pub fn register() {
    model::register_format(&V20);
}

impl ModelFormat for V20 {
    fn version(&self) -> u16 {
        xmodel::VERSION
    }

    fn load(&self, fs: &dyn AssetSource, descriptor: &[u8]) -> Result<Model> {
        let (desc, parts) = read_rig(fs, descriptor)?;
        let mut pose = xmodel::base_pose(&parts);
        let mut lods = Vec::new();
        let mut bones_out = None;
        for slot in desc.present_lods() {
            let path = format!("xmodelsurfs/{}", slot.name);
            let surfs = read(fs, &path).and_then(|d| xmodel::parse_surfs(&d));
            let surfs = surfs.map_err(|e| e.context(path))?;
            // LOD 0 comes first; its mesh fixes the bind every LOD bakes on.
            if bones_out.is_none() {
                if is_view_rig(&slot.name) {
                    recover_bind(&mut pose, &surfs);
                }
                bones_out = Some(bones(&desc, &parts, &pose));
            }
            ensure!(
                surfs.surfaces.len() == slot.materials.len(),
                "LOD {}: {} surfaces for {} materials",
                slot.name,
                surfs.surfaces.len(),
                slot.materials.len()
            );
            let surfaces = surfs
                .surfaces
                .iter()
                .enumerate()
                .map(|(i, s)| bake_surface(s, i, &pose))
                .collect::<Result<_>>()
                .map_err(|e| e.context(format!("LOD {}", slot.name)))?;
            lods.push(Lod {
                name: slot.name.clone(),
                dist: slot.dist,
                surfaces,
                materials: slot.materials.clone(),
            });
        }
        Ok(Model {
            name: desc.lods[0].name.clone(),
            collision: collision(&desc, &pose)?,
            bones: bones_out.unwrap_or_else(|| bones(&desc, &parts, &pose)),
            lods,
        })
    }

    /// The stored bind, not [`recover_bind`]'s: the traces that read bones
    /// never pose a viewmodel.
    fn load_bones(&self, fs: &dyn AssetSource, descriptor: &[u8]) -> Result<Vec<Bone>> {
        let (desc, parts) = read_rig(fs, descriptor)?;
        Ok(bones(&desc, &parts, &xmodel::base_pose(&parts)))
    }

    /// The `mins`/`maxs` after the version and the flags byte.
    fn bounds(&self, data: &[u8]) -> Option<(Vec3, Vec3)> {
        if data.get(..2)? != xmodel::VERSION.to_le_bytes() {
            return None;
        }
        let f = |i: usize| {
            let at = 3 + 4 * i;
            Some(f32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
        };
        Some((
            Vec3::new(f(0)?, f(1)?, f(2)?),
            Vec3::new(f(3)?, f(4)?, f(5)?),
        ))
    }
}

fn read(fs: &dyn AssetSource, path: &str) -> Result<Vec<u8>> {
    fs.read(path).ok_or_else(|| anyhow!("{path} not found"))
}

/// The descriptor and LOD 0's parts, which size its trailing hit boxes.
fn read_rig(fs: &dyn AssetSource, descriptor: &[u8]) -> Result<(Descriptor, Parts)> {
    let mut parts = None;
    let desc = xmodel::parse_descriptor(descriptor, |lod0| {
        let path = format!("xmodelparts/{lod0}");
        let p = read(fs, &path)
            .and_then(|d| xmodel::parse_parts(&d))
            .map_err(|e| e.context(path))?;
        let n = p.bones.len();
        parts = Some(p);
        Ok(n)
    })?;
    let parts = parts.expect("parse_descriptor asks for the bone count");
    Ok((desc, parts))
}

/// The unit rotation an engine quaternion stands for (its matrix divides by
/// `|q|^2`); identity for a zero one, as `trans_weight` 2 makes it.
fn unit(q: Quat) -> Quat {
    if q.length_squared() == 0.0 {
        Quat::IDENTITY
    } else {
        q.normalize()
    }
}

fn bones(desc: &Descriptor, parts: &Parts, pose: &[AnimMat]) -> Vec<Bone> {
    parts
        .bones
        .iter()
        .zip(pose)
        .zip(&desc.bone_infos)
        .map(|((b, m), info)| Bone {
            name: b.name.clone(),
            parent: b.parent.map_or(-1, |p| p as i32),
            pos: m.trans,
            rot: unit(m.quat),
            local_pos: Vec3::from(b.trans),
            local_rot: unit(Quat::from_array(b.quat)),
            hit_mins: Vec3::from(info.mins),
            hit_maxs: Vec3::from(info.maxs),
            hit_location: b.classification,
        })
        .collect()
}

/// A viewmodel (`2`) or viewhands (`4`) by its LOD 0 name's last
/// character, the exporter's type rule.
pub fn is_view_rig(lod0: &str) -> bool {
    matches!(lod0.as_bytes().last(), Some(b'2' | b'4'))
}

/// Rewrites the base pose translation of every bone that shares a vertex
/// with another to the one the mesh's per-weight offsets imply, as v14's
/// `recover_viewhands_bind` does: `pos_b - pos_a` is
/// `R_a * offset_a - R_b * offset_b` over each vertex weighting both. A
/// connected group keeps its first bone's translation; rotations stand.
pub fn recover_bind(pose: &mut [AnimMat], surfs: &Surfs) {
    // (a, b) -> summed `pos_b - pos_a` and its sample count.
    let mut offsets: BTreeMap<(usize, usize), (Vec3, u32)> = BTreeMap::new();
    for v in surfs.surfaces.iter().flat_map(|s| &s.vertices) {
        let a = v.bone as usize;
        let Some(pa) = pose.get(a) else { continue };
        let at_primary = pa.rotate(Vec3::from(v.offset));
        for e in &v.extras {
            let b = e.bone as usize;
            let Some(pb) = pose.get(b) else { continue };
            let d = at_primary - pb.rotate(Vec3::from(e.offset));
            for (key, d) in [((a, b), d), ((b, a), -d)] {
                let s = offsets.entry(key).or_insert((Vec3::ZERO, 0));
                s.0 += d;
                s.1 += 1;
            }
        }
    }
    let mut placed = vec![false; pose.len()];
    for start in 0..pose.len() {
        if placed[start] || offsets.range((start, 0)..(start + 1, 0)).next().is_none() {
            continue;
        }
        placed[start] = true;
        let mut queue = vec![start];
        while let Some(a) = queue.pop() {
            let next: Vec<_> = offsets
                .range((a, 0)..(a + 1, 0))
                .filter(|((_, to), _)| !placed[*to])
                .map(|(&(_, to), &(sum, n))| (to, sum / n as f32))
                .collect();
            for (to, d) in next {
                if !placed[to] {
                    pose[to].trans = pose[a].trans + d;
                    placed[to] = true;
                    queue.push(to);
                }
            }
        }
    }
}

/// The worst disagreement between where a vertex's primary and one of its
/// extra weights put it under `pose`, units: 0 on a bind every weight
/// agrees with.
pub fn bind_spread(pose: &[AnimMat], surfs: &Surfs) -> f32 {
    let mut worst = 0f32;
    for v in surfs.surfaces.iter().flat_map(|s| &s.vertices) {
        let Some(pa) = pose.get(v.bone as usize) else {
            continue;
        };
        let at = pa.transform_point(Vec3::from(v.offset));
        for e in &v.extras {
            if let Some(pb) = pose.get(e.bone as usize) {
                worst = worst.max(at.distance(pb.transform_point(Vec3::from(e.offset))));
            }
        }
    }
    worst
}

fn bone_mat(pose: &[AnimMat], bone: u16) -> Result<&AnimMat> {
    pose.get(bone as usize)
        .ok_or_else(|| anyhow!("bone {bone} past {} bones", pose.len()))
}

/// Bakes each vertex through its primary bone's base pose; the extras only
/// weight it, as v14's bake does.
fn bake_surface(s: &xmodel::Surface, material: usize, pose: &[AnimMat]) -> Result<Surface> {
    let mut verts = Vec::with_capacity(s.vertices.len());
    for v in &s.vertices {
        let m = bone_mat(pose, v.bone)?;
        let mut weights: Vec<(u16, f32)> = std::iter::once((v.bone, v.primary_fraction()))
            .chain(v.extras.iter().map(|e| (e.bone, e.fraction())))
            .collect();
        for &(b, _) in &weights {
            bone_mat(pose, b)?;
        }
        weights.sort_by(|a, b| b.1.total_cmp(&a.1));
        weights.truncate(4);
        let mut bone_indices = [0u8; 4];
        let mut bone_weights = [0f32; 4];
        for (i, (b, w)) in weights.into_iter().enumerate() {
            // `MAX_BONES` is 128, so every bone fits a byte.
            bone_indices[i] = b as u8;
            bone_weights[i] = w;
        }
        verts.push(Vertex {
            pos: m.transform_point(Vec3::from(v.offset)).to_array(),
            normal: m.rotate(Vec3::from(v.normal)).to_array(),
            uv: v.uv,
            bone_indices,
            bone_weights,
            color: v.color,
            tangent: m.rotate(Vec3::from(v.tangent)).to_array(),
            binormal: m.rotate(Vec3::from(v.binormal)).to_array(),
        });
    }
    Ok(Surface {
        verts,
        indices: s.triangles.iter().flatten().copied().collect(),
        material,
    })
}

/// The descriptor's collision as the loader keeps it: bounds grown, contents
/// masked, triangles solved to vertices and baked at their bone's base pose.
fn collision(desc: &Descriptor, pose: &[AnimMat]) -> Result<Vec<CollSurf>> {
    let mut out = Vec::with_capacity(desc.coll_surfs.len());
    let mut degenerate = 0usize;
    for s in &desc.coll_surfs {
        let bone = usize::try_from(s.bone)
            .ok()
            .filter(|&b| b < pose.len())
            .ok_or_else(|| anyhow!("collision surface on bone {} out of range", s.bone))?;
        let m = &pose[bone];
        let mut tris = Vec::with_capacity(s.tris.len());
        for t in &s.tris {
            let mut rec = [0f32; 12];
            rec[..4].copy_from_slice(&t.plane);
            rec[4..8].copy_from_slice(&t.svec);
            rec[8..].copy_from_slice(&t.tvec);
            match vcod_common::xmodel::coll_tri_verts(&rec) {
                Some(vs) => tris.push(vs.map(|v| m.transform_point(v))),
                None => degenerate += 1,
            }
        }
        let grow = Vec3::splat(COLL_BOUNDS_GROWTH);
        out.push(CollSurf {
            contents: s.contents as u32 & COLL_CONTENTS_MASK,
            flags: s.surf_flags as u32,
            tris,
            bounds: (Vec3::from(s.mins) - grow, Vec3::from(s.maxs) + grow),
            bone,
        });
    }
    if degenerate > 0 {
        log::debug!("xmodel v20: dropped {degenerate} degenerate collision triangles");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::pk3::Pk3Fs;

    #[test]
    fn bounds_skip_the_flags_byte() {
        let mut d = vec![20, 0, 1];
        for v in [-1.0f32, -2.0, -3.0, 4.0, 5.0, 6.0] {
            d.extend(v.to_le_bytes());
        }
        assert_eq!(
            V20.bounds(&d),
            Some((Vec3::new(-1.0, -2.0, -3.0), Vec3::new(4.0, 5.0, 6.0)))
        );
        assert_eq!(V20.bounds(&d[..d.len() - 1]), None);
        d[0] = 14;
        assert_eq!(V20.bounds(&d), None);
    }

    fn f32s(out: &mut Vec<u8>, vs: &[f32]) {
        for v in vs {
            out.extend(v.to_le_bytes());
        }
    }

    /// `m`: a root and `arm`, turned 90 degrees about z at (1, 2, 3), with
    /// one rigid triangle and one collision triangle on `arm`.
    fn synthetic_fs() -> Pk3Fs {
        let mut parts = vec![20, 0, 1, 0, 1, 0, 0];
        f32s(&mut parts, &[1.0, 2.0, 3.0]);
        for s in [0i16, 0, 23170] {
            parts.extend(s.to_le_bytes());
        }
        parts.extend(b"\0arm\0");
        parts.extend([0, 18]);

        let mut surfs = vec![20, 0, 1, 0, 2, 3, 0, 1, 0, 1, 0];
        for x in [0.0, 1.0, 2.0] {
            f32s(&mut surfs, &[0.0, 0.0, 1.0]);
            surfs.extend([255; 4]);
            f32s(&mut surfs, &[x, 0.5, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]);
            f32s(&mut surfs, &[x, 0.0, 0.0]);
        }
        for i in [0u16, 1, 2] {
            surfs.extend(i.to_le_bytes());
        }

        let mut desc = vec![20, 0, 0];
        f32s(&mut desc, &[-1.0, -1.0, -1.0, 1.0, 1.0, 1.0]);
        f32s(&mut desc, &[300.0]);
        desc.extend(b"m_lod0\0");
        for _ in 0..3 {
            f32s(&mut desc, &[0.0]);
            desc.push(0);
        }
        desc.extend(0i32.to_le_bytes());
        desc.extend(1i32.to_le_bytes());
        desc.extend(1i32.to_le_bytes());
        // The floor triangle (0,0,0) (1,0,0) (0,1,0) in plane form.
        f32s(&mut desc, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        f32s(&mut desc, &[1.0, 0.0, 0.0, 0.0]);
        f32s(&mut desc, &[0.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
        for v in [1i32, 0x2000_0015, 0x0010_0000] {
            desc.extend(v.to_le_bytes());
        }
        desc.extend(1u16.to_le_bytes());
        desc.extend(b"mtl_arm\0");
        f32s(&mut desc, &[0.0; 6]);
        f32s(&mut desc, &[-1.0, -2.0, -3.0, 4.0, 5.0, 6.0]);

        let mut fs = Pk3Fs::empty();
        fs.overlay("xmodel/m", desc);
        fs.overlay("xmodelparts/m_lod0", parts);
        fs.overlay("xmodelsurfs/m_lod0", surfs);
        fs
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        Vec3::from(a).abs_diff_eq(Vec3::from(b), 1e-4)
    }

    #[test]
    fn a_synthetic_model_loads_through_the_dispatch() {
        register();
        register();
        let fs = synthetic_fs();
        let m = model::load(&fs, "m").unwrap();
        assert_eq!(m.name, "m_lod0");
        assert_eq!(m.lod_dists(), [300.0]);
        assert_eq!(m.materials(), ["mtl_arm"]);

        let [root, arm] = &m.bones[..] else { panic!() };
        assert_eq!((root.parent, arm.parent), (-1, 0));
        assert_eq!(arm.hit_location, 18);
        assert_eq!(arm.hit_maxs, Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(root.hit_maxs, Vec3::ZERO);
        assert!((arm.rot.length() - 1.0).abs() < 1e-6);
        assert!(close((arm.rot * Vec3::X).to_array(), [0.0, 1.0, 0.0]));
        assert_eq!(arm.pos, Vec3::new(1.0, 2.0, 3.0));

        // Vertex x along the arm's x lands along the model's y.
        let s = &m.surfaces()[0];
        assert_eq!(s.indices, [0, 2, 1]);
        assert!(
            close(s.verts[2].pos, [1.0, 4.0, 3.0]),
            "{:?}",
            s.verts[2].pos
        );
        assert!(close(s.verts[2].normal, [0.0, 0.0, 1.0]));
        assert_eq!(s.verts[0].bone_indices, [1, 0, 0, 0]);
        assert_eq!(s.verts[0].bone_weights, [1.0, 0.0, 0.0, 0.0]);

        let c = &m.collision[0];
        assert_eq!((c.bone, c.contents, c.flags), (1, 0x11, 0x0010_0000));
        assert_eq!(c.bounds.0, Vec3::splat(-0.001));
        assert_eq!(c.bounds.1, Vec3::new(1.001, 1.001, 0.001));
        let [v0, v1, v2] = c.tris[0];
        assert!(close(v0.to_array(), [1.0, 2.0, 3.0]));
        assert!(close(v1.to_array(), [1.0, 3.0, 3.0]), "{v1}");
        assert!(close(v2.to_array(), [0.0, 2.0, 3.0]), "{v2}");

        let bones = model::load_bones(&fs, "m").unwrap();
        assert_eq!(bones.len(), 2);
        assert_eq!(bones[1].hit_mins, Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(model::bounds(&fs, "m"), Some((Vec3::NEG_ONE, Vec3::ONE)));
    }
}
