//! A 1.7 zone's xmodels and xanims in vcod-common's neutral types: an
//! xmodel's skeleton and skinned mesh ([`model`]) and an xanim's tracks
//! ([`clip`]), so the shared pose code (`animtree`, `animclock`,
//! `playerpose`, `skeleton`) poses CoD4 bodies as it poses CoD1's and
//! CoD2's. docs/research/cod4-player-anims.md.

use crate::assets::Assets;
use crate::assets::model::{XAnim, XModel};
use anyhow::{Context, Result, bail, ensure};
use glam::{Quat, Vec3};
use vcod_common::asset_source::AssetSource;
use vcod_common::model::{Bone, Lod, Model, Surface, Vertex};
use vcod_common::xanim::{self as cxanim, Track};

/// Script string `i` of the zone, `""` where it is null or out of range.
pub fn script_string(a: &Assets, i: u16) -> &str {
    a.script_strings
        .get(usize::from(i))
        .and_then(|s| s.as_deref())
        .unwrap_or("")
}

fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// An `XModelQuat`'s four shorts over 32767 (`DObjCalcBaseAnim`).
fn quat_of(b: &[u8], o: usize) -> Quat {
    let c = |k: usize| f32::from(i16_at(b, o + 2 * k)) / 32767.0;
    Quat::from_xyzw(c(0), c(1), c(2), c(3)).normalize()
}

/// `x`'s bones: the root bones at their `baseMat`, every other one off its
/// parent by `parentList`'s offset with the local quat and translation
/// the zone stores, the world bind composed from them.
pub fn bones(a: &Assets, x: &XModel) -> Result<Vec<Bone>> {
    let n = usize::from(x.num_bones);
    let roots = usize::from(x.num_root_bones);
    ensure!(roots <= n, "{}: {roots} root bones of {n}", x.name);
    let nonroot = n - roots;
    ensure!(
        x.bone_names.len() == n
            && x.parent_list.len() == nonroot
            && x.quats.len() == 8 * nonroot
            && x.trans.len() == 16 * nonroot,
        "{}: bone arrays not kept",
        x.name
    );
    let mut out: Vec<Bone> = Vec::with_capacity(n);
    for i in 0..n {
        let name = script_string(a, x.bone_names[i]).to_string();
        let (parent, local_pos, local_rot) = if i < roots {
            // A root stands where its `baseMat` puts it: the origin on a
            // body, off it on a viewmodel's `tag_torso`.
            match x.base_mat.get(32 * i..32 * i + 28) {
                Some(m) => (
                    -1,
                    Vec3::new(f32_at(m, 16), f32_at(m, 20), f32_at(m, 24)),
                    Quat::from_xyzw(f32_at(m, 0), f32_at(m, 4), f32_at(m, 8), f32_at(m, 12))
                        .normalize(),
                ),
                None => (-1, Vec3::ZERO, Quat::IDENTITY),
            }
        } else {
            let k = i - roots;
            let off = usize::from(x.parent_list[k]);
            ensure!(
                off >= 1 && off <= i,
                "{}: bone {i} parent offset {off}",
                x.name
            );
            let t = Vec3::new(
                f32_at(&x.trans, 12 * k),
                f32_at(&x.trans, 12 * k + 4),
                f32_at(&x.trans, 12 * k + 8),
            );
            ((i - off) as i32, t, quat_of(&x.quats, 8 * k))
        };
        // `XBoneInfo.bounds`: the hit box in the bone's frame (zero for a
        // bone with none), `partClassification` its hit location.
        let info = |o: usize| match x.bone_info.get(0x28 * i + o..0x28 * i + o + 12) {
            Some(b) => Vec3::new(f32_at(b, 0), f32_at(b, 4), f32_at(b, 8)),
            None => Vec3::ZERO,
        };
        let (pos, rot) = match usize::try_from(parent) {
            Ok(p) => (
                out[p].pos + out[p].rot * local_pos,
                (out[p].rot * local_rot).normalize(),
            ),
            Err(_) => (local_pos, local_rot),
        };
        out.push(Bone {
            name,
            parent,
            pos,
            rot,
            local_pos,
            local_rot,
            hit_mins: info(0),
            hit_maxs: info(12),
            hit_location: x.part_classification.get(i).copied().unwrap_or(0),
        });
    }
    Ok(out)
}

/// One `GfxPackedVertex` (0x20): position 0, colour B G R A at 0x10,
/// packed texture coordinates 0x14, packed normal 0x18 (map.rs reads the
/// same fields).
fn packed_vertex(b: &[u8]) -> Vertex {
    let pos = [f32_at(b, 0), f32_at(b, 4), f32_at(b, 8)];
    let uv = crate::map::unpack_tex_coords(u32::from_le_bytes([b[20], b[21], b[22], b[23]]));
    let normal = crate::map::unpack_unit_vec([b[24], b[25], b[26], b[27]]);
    let tangent = crate::map::unpack_unit_vec([b[28], b[29], b[30], b[31]]);
    // `binormalSign` rides the position's w: the binormal is
    // `sign * (normal x tangent)`, as the reflex sight's vertex shader
    // rebuilds it (cod4-client-view.md 4.5).
    let binormal = (Vec3::from(normal).cross(Vec3::from(tangent)) * f32_at(b, 12)).to_array();
    Vertex {
        pos,
        normal,
        uv,
        bone_indices: [0; 4],
        bone_weights: [1.0, 0.0, 0.0, 0.0],
        color: [b[18], b[17], b[16], b[19]],
        tangent,
        binormal,
    }
}

/// A blend or rigid list's bone: a byte offset into the skeleton's
/// 0x40-byte `DObjSkelMat`s (`R_SkinXSurfaceWeight0`, `R_SkinXSurfaceRigid`).
fn bone_of(offset: u16, bones: usize) -> Result<u8> {
    ensure!(
        offset.is_multiple_of(64),
        "bone offset {offset} not a matrix"
    );
    let b = usize::from(offset / 64);
    ensure!(b < bones, "bone {b} of {bones}");
    Ok(b as u8)
}

/// Each vertex's bones and weights: a deformed surface's blend data (the
/// vertices with one bone, then two, three and four, each extra bone with
/// a weight over 65536 and the first taking the rest), a rigid one's runs.
fn skin(s: &crate::assets::model::XSurface, verts: &mut [Vertex], bones: usize) -> Result<()> {
    if s.deformed {
        let blend = s.verts_blend.as_deref().context("blend data not kept")?;
        let blend: Vec<u16> = crate::walk::u16s(blend);
        let mut at = 0;
        let mut v = 0;
        for (extra, &count) in s.blend_counts.iter().enumerate() {
            for _ in 0..count {
                let vert = verts.get_mut(v).context("blend data past the vertices")?;
                let w = blend
                    .get(at..at + 1 + 2 * extra)
                    .context("blend data short")?;
                let mut first = 1.0;
                vert.bone_indices[0] = bone_of(w[0], bones)?;
                for e in 0..extra {
                    let weight = f32::from(w[2 + 2 * e]) / 65536.0;
                    vert.bone_indices[1 + e] = bone_of(w[1 + 2 * e], bones)?;
                    vert.bone_weights[1 + e] = weight;
                    first -= weight;
                }
                vert.bone_weights[0] = first;
                at += 1 + 2 * extra;
                v += 1;
            }
        }
        ensure!(
            v == verts.len(),
            "blend data covers {v} of {} vertices",
            verts.len()
        );
    } else {
        let mut v = 0;
        for r in &s.rigid {
            let bone = bone_of(r[0], bones)?;
            for _ in 0..r[1] {
                verts
                    .get_mut(v)
                    .context("rigid runs past the vertices")?
                    .bone_indices[0] = bone;
                v += 1;
            }
        }
        if s.rigid.is_empty() {
            // A surface with no runs rides the root.
            return Ok(());
        }
        ensure!(
            v == verts.len(),
            "rigid runs cover {v} of {} vertices",
            verts.len()
        );
    }
    Ok(())
}

/// [`bones`] with each one's bind its `baseMat`: the vertices sit at the
/// `baseMat` pose, so it is the skin's bind. A view hands model ships its
/// local bind zeroed (its anims key every bone's whole translation), which
/// the composition cannot rebuild.
fn bound_bones(a: &Assets, x: &XModel) -> Result<Vec<Bone>> {
    let mut bones = bones(a, x)?;
    for (i, b) in bones.iter_mut().enumerate() {
        if let Some(m) = x.base_mat.get(32 * i..32 * i + 28) {
            b.pos = Vec3::new(f32_at(m, 16), f32_at(m, 20), f32_at(m, 24));
            b.rot = Quat::from_xyzw(f32_at(m, 0), f32_at(m, 4), f32_at(m, 8), f32_at(m, 12))
                .normalize();
        }
    }
    Ok(bones)
}

/// `x`'s skeleton and hit boxes with an empty mesh: what a server poses
/// and traces, off a zone walked without its bulk.
pub fn skeleton_model(a: &Assets, x: &XModel) -> Result<Model> {
    Ok(Model {
        name: x.name.clone(),
        bones: bound_bones(a, x)?,
        lods: vec![Lod {
            name: x.name.clone(),
            dist: 0.0,
            surfaces: Vec::new(),
            materials: Vec::new(),
        }],
        collision: Vec::new(),
    })
}

/// `x`'s first LOD skinned to its bones (each one's bind its `baseMat`),
/// the vertices in model space at the bind pose as the zone stores them; each surface's material named
/// by `material` (a surface whose material or geometry the walk did not
/// keep is left out). `None` for a model with no surface kept.
pub fn model(
    a: &Assets,
    x: &XModel,
    material: impl FnMut(crate::assets::Id<crate::assets::material::Material>) -> String,
) -> Result<Option<Model>> {
    model_hiding(a, x, [0; 4], material)
}

/// The `hidePartBits` of `x`'s bones named in `tags` (bone `b` is word
/// `b >> 5`, bit `0x80000000 >> (b & 31)`), as `CG_RegisterWeapon` sets a
/// view weapon's off its `hideTags` (cod4-client-view.md 4.2). A name
/// the model lacks hides nothing.
pub fn hide_part_bits(a: &Assets, x: &XModel, tags: &[String]) -> [u32; 4] {
    let mut bits = [0u32; 4];
    for (b, &n) in x.bone_names.iter().enumerate().take(128) {
        let name = a.script_strings.get(usize::from(n)).cloned().flatten();
        if name.is_some_and(|n| tags.iter().any(|t| t.eq_ignore_ascii_case(&n))) {
            bits[b >> 5] |= 0x8000_0000 >> (b & 31);
        }
    }
    bits
}

/// [`model`] without the surfaces whose `partBits` meet `hide`: the
/// renderer skips a surface any of whose bones is hidden
/// (`R_SkinXModel`, cod4-client-view.md 4.2).
pub fn model_hiding(
    a: &Assets,
    x: &XModel,
    hide: [u32; 4],
    material: impl FnMut(crate::assets::Id<crate::assets::material::Material>) -> String,
) -> Result<Option<Model>> {
    Ok(model_parts(a, x, hide, material)?.map(|(m, _)| m))
}

/// [`model_hiding`] and each kept surface's `XSurface.partBits`, by
/// index into the LOD's surfaces: what an entity's `s.partBits` (its
/// `hidePart`s, cod4-client-view.md 15) hides from the model drawn.
pub fn model_parts(
    a: &Assets,
    x: &XModel,
    hide: [u32; 4],
    mut material: impl FnMut(crate::assets::Id<crate::assets::material::Material>) -> String,
) -> Result<Option<(Model, Vec<[u32; 4]>)>> {
    let bones = bound_bones(a, x)?;
    let mut parts = Vec::new();
    let Some(lod) = x.lods.first().filter(|_| x.num_lods > 0) else {
        return Ok(None);
    };
    let mut materials: Vec<String> = Vec::new();
    let mut surfaces = Vec::new();
    let first = usize::from(lod.surf_index);
    for si in first..first + usize::from(lod.num_surfs) {
        let (Some(s), Some(Some(m))) = (x.surfaces.get(si), x.materials.get(si)) else {
            continue;
        };
        let (Some(vb), Some(tb)) = (&s.verts, &s.tris) else {
            continue;
        };
        if s.part_bits.iter().zip(hide).any(|(p, h)| p & h != 0) {
            continue;
        }
        let mut verts: Vec<Vertex> = vb
            .as_chunks::<0x20>()
            .0
            .iter()
            .map(|b| packed_vertex(b))
            .collect();
        skin(s, &mut verts, bones.len()).with_context(|| format!("{} surface {si}", x.name))?;
        let indices = crate::walk::u16s(tb);
        if indices.iter().any(|&i| usize::from(i) >= verts.len()) {
            bail!("{} surface {si}: an index past its vertices", x.name);
        }
        let name = material(*m);
        let mi = materials
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| {
                materials.push(name);
                materials.len() - 1
            });
        surfaces.push(Surface {
            verts,
            indices,
            material: mi,
        });
        parts.push(s.part_bits);
    }
    if surfaces.is_empty() {
        return Ok(None);
    }
    let model = Model {
        name: x.name.clone(),
        bones,
        lods: vec![Lod {
            name: x.name.clone(),
            dist: 0.0,
            surfaces,
            materials,
        }],
        collision: Vec::new(),
    };
    Ok(Some((model, parts)))
}

/// A read cursor over one of an xanim's data streams.
struct Cur<'a> {
    b: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8], what: &'static str) -> Self {
        Cur { b, at: 0, what }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.b.get(self.at..self.at + n).with_context(|| {
            format!("{} ends at {} of {}", self.what, self.b.len(), self.at + n)
        })?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }
    fn f32(&mut self) -> Result<f32> {
        let s = self.take(4)?;
        Ok(f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn vec3(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
}

/// The streams `XAnimCalcParts` reads in step, and the frame index type.
struct Streams<'a> {
    byte: Cur<'a>,
    short: Cur<'a>,
    int: Cur<'a>,
    rshort: Cur<'a>,
    rbyte: Cur<'a>,
    indices: Cur<'a>,
    /// u16 frame indices (256 frames or more).
    wide: bool,
}

impl Streams<'_> {
    /// One track's key frames: its table size off `dataShort`, then
    /// `size + 1` frame indices: bytes off `dataByte` under 256 frames; at
    /// or past it shorts off `dataShort`, or, from 64 keys up, a coarse
    /// table there (one entry per 256 keys, plus one) and the keys in
    /// `indices`.
    fn keys(&mut self) -> Result<Vec<u16>> {
        let size = usize::from(self.short.u16()?);
        if !self.wide {
            return Ok(self
                .byte
                .take(size + 1)?
                .iter()
                .map(|&b| u16::from(b))
                .collect());
        }
        let src = if size >= 64 {
            self.short.take(2 * (((size - 1) >> 8) + 2))?;
            &mut self.indices
        } else {
            &mut self.short
        };
        (0..=size).map(|_| src.u16()).collect()
    }
}

/// Frame `f`'s key list checked: ascending, starting at 0 and ending at
/// the clip's last frame, as `XAnim_GetTimeIndex` reads it.
fn check_keys(keys: &[u16], last: u16) -> Result<()> {
    ensure!(
        keys.first() == Some(&0) && keys.last() == Some(&last),
        "keys run {:?}..{:?}, the clip 0..{last}",
        keys.first(),
        keys.last()
    );
    ensure!(keys.windows(2).all(|w| w[0] < w[1]), "keys not ascending");
    Ok(())
}

/// `x`'s tracks, one per bone it names: rotation keys as the local
/// quaternion (`noQuat` bones the identity, half quats about z), and
/// translation keys as the offset from the bind's local position
/// (`noTrans` bones none), decoded in `XAnimCalcParts`' stream order;
/// the root motion as the nameless root track.
pub fn clip(a: &Assets, x: &XAnim) -> Result<cxanim::XAnim> {
    let names: Vec<String> = x
        .names
        .iter()
        .map(|&i| script_string(a, i).to_string())
        .collect();
    let bc = x.bone_count.map(usize::from);
    let quat_total: usize = bc[..5].iter().sum();
    let trans_total: usize = bc[5..9].iter().sum();
    ensure!(
        names.len() == bc[9] && quat_total == bc[9] && trans_total == bc[9],
        "{}: bone counts {:?} for {} names",
        x.name,
        x.bone_count,
        names.len()
    );
    let last = x.num_frames;
    let mut s = Streams {
        byte: Cur::new(&x.data_byte, "dataByte"),
        short: Cur::new(&x.data_short, "dataShort"),
        int: Cur::new(&x.data_int, "dataInt"),
        rshort: Cur::new(&x.random_data_short, "randomDataShort"),
        rbyte: Cur::new(&x.random_data_byte, "randomDataByte"),
        indices: Cur::new(&x.indices, "indices"),
        wide: last >= 256,
    };
    let mut tracks: Vec<Track> = names
        .iter()
        .map(|n| Track {
            bone: n.clone(),
            rot_keys: Vec::new(),
            trans_keys: Vec::new(),
            rot_raw: Vec::new(),
        })
        .collect();
    let q = |v: [i16; 4]| {
        let [x, y, z, w] = v.map(|c| f32::from(c) / 32767.0);
        Quat::from_xyzw(x, y, z, w).normalize()
    };
    let mut bone = 0;
    let mut decode = || -> Result<()> {
        for t in tracks.iter_mut().take(bc[0]) {
            t.rot_keys.push((0, Quat::IDENTITY));
        }
        bone += bc[0];
        for (part, &count) in bc.iter().enumerate().take(5).skip(1) {
            for _ in 0..count {
                let t = &mut tracks[bone];
                bone += 1;
                let half = part % 2 == 1;
                if part <= 2 {
                    let keys = s.keys()?;
                    check_keys(&keys, last).with_context(|| format!("bone {}", t.bone))?;
                    for f in keys {
                        let v = if half {
                            [0, 0, s.rshort.i16()?, s.rshort.i16()?]
                        } else {
                            [
                                s.rshort.i16()?,
                                s.rshort.i16()?,
                                s.rshort.i16()?,
                                s.rshort.i16()?,
                            ]
                        };
                        t.rot_keys.push((f, q(v)));
                    }
                } else {
                    let v = if half {
                        [0, 0, s.short.i16()?, s.short.i16()?]
                    } else {
                        [
                            s.short.i16()?,
                            s.short.i16()?,
                            s.short.i16()?,
                            s.short.i16()?,
                        ]
                    };
                    t.rot_keys.push((0, q(v)));
                }
            }
        }
        for (part, &count) in bc.iter().enumerate().take(9).skip(5) {
            for _ in 0..count {
                let b = usize::from(s.byte.u8()?);
                let t = tracks
                    .get_mut(b)
                    .with_context(|| format!("trans bone {b}"))?;
                match part {
                    5 | 6 => {
                        let keys = s.keys()?;
                        check_keys(&keys, last).with_context(|| format!("bone {}", t.bone))?;
                        let (mins, size) = (s.int.vec3()?, s.int.vec3()?);
                        for f in keys {
                            let v = if part == 5 {
                                let b = s.rbyte.take(3)?;
                                Vec3::new(b[0].into(), b[1].into(), b[2].into())
                            } else {
                                Vec3::new(
                                    s.rshort.u16()?.into(),
                                    s.rshort.u16()?.into(),
                                    s.rshort.u16()?.into(),
                                )
                            };
                            t.trans_keys.push((f, mins + size * v));
                        }
                    }
                    7 => t.trans_keys.push((0, s.int.vec3()?)),
                    _ => t.trans_keys.push((0, Vec3::ZERO)),
                }
            }
        }
        Ok(())
    };
    decode().with_context(|| x.name.clone())?;
    // A clip's streams end where its last bone's data does.
    for c in [&s.byte, &s.short, &s.int, &s.rshort, &s.rbyte] {
        ensure!(
            c.at == c.b.len(),
            "{}: {} read {} of {}",
            x.name,
            c.what,
            c.at,
            c.b.len()
        );
    }
    let root = x.root.as_ref().map(|d| Track {
        bone: String::new(),
        rot_keys: d
            .rot
            .iter()
            .map(|&(f, [z, w])| (f, q([0, 0, z, w])))
            .collect(),
        trans_keys: d.trans.iter().map(|&(f, p)| (f, Vec3::from(p))).collect(),
        rot_raw: Vec::new(),
    });
    Ok(cxanim::XAnim {
        name: x.name.clone(),
        frame_count: u32::from(last) + 1,
        framerate: x.framerate,
        looping: x.looped,
        tracks,
        root,
        notes: x
            .notify
            .iter()
            .map(|&(n, t)| {
                (
                    script_string(a, n).to_string(),
                    (t * f32::from(last)).round() as u16,
                )
            })
            .collect(),
    })
}

/// The player animtree and script, as rawfiles (`common_mp` carries both).
pub const ANIMTREE: &str = "animtrees/multiplayer.atr";
pub const ANIMSCRIPT: &str = "mp/playeranim.script";
/// The clips the cgame itself plays on an entity, outside the player
/// tree: a helicopter's rotors (`CG_GetHelicopterAnims`, iw3mp.exe
/// 0x432ae0; the name at 0x6c8308).
pub const ENTITY_CLIPS: [&str; 1] = ["bh_rotors"];

/// What the shared player code reads through an [`AssetSource`]: the
/// animtree and the anim script as files, every clip the tree names
/// decoded with the cgame's [`ENTITY_CLIPS`], and the models a session's
/// bodies are built of (skinned by
/// [`model`]). A 1.7 zone keeps clips and models in pools by name, not as
/// `xanim/` and `xmodel/` files, so these come back through
/// [`AssetSource::decoded_anim`] and [`AssetSource::decoded_model`].
pub struct BodySource {
    raw: std::collections::HashMap<String, Vec<u8>>,
    clips: std::collections::HashMap<String, cxanim::XAnim>,
    models: std::collections::HashMap<String, Model>,
    cache: vcod_common::asset_source::AssetCache,
}

impl BodySource {
    /// The tree and script out of the first of `zones` holding each, and
    /// every clip the tree names, and [`ENTITY_CLIPS`], out of the first
    /// zone holding it whole;
    /// `models` as the session built them.
    pub fn from_zones(
        zones: &[&Assets],
        models: std::collections::HashMap<String, Model>,
    ) -> Result<BodySource> {
        let raw_file = |name: &str| {
            zones.iter().find_map(|a| {
                a.raw_files
                    .iter()
                    .find(|r| r.name.eq_ignore_ascii_case(name))
                    .map(|r| r.data.clone())
            })
        };
        let mut raw = std::collections::HashMap::new();
        for name in [ANIMTREE, ANIMSCRIPT] {
            raw.insert(
                name.to_string(),
                raw_file(name).with_context(|| format!("no zone carries {name}"))?,
            );
        }
        let tree =
            vcod_common::animtree::AnimTree::parse(&String::from_utf8_lossy(&raw[ANIMTREE]))?;
        let mut clips = std::collections::HashMap::new();
        let names = tree
            .nodes
            .iter()
            .filter(|n| n.children.is_empty())
            .map(|n| n.name.as_str())
            .chain(ENTITY_CLIPS);
        for name in names {
            if clips.contains_key(name) {
                continue;
            }
            let found = zones.iter().find_map(|a| {
                let x = a
                    .xanims
                    .iter()
                    .find(|x| x.name.eq_ignore_ascii_case(name))?;
                Some((*a, x))
            });
            let Some((a, x)) = found else {
                continue;
            };
            match clip(a, x) {
                Ok(c) => {
                    clips.insert(name.to_ascii_lowercase(), c);
                }
                Err(e) => log::warn!("cod4 anim {name}: {e:#}"),
            }
        }
        Ok(BodySource {
            raw,
            clips,
            models: models
                .into_iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v))
                .collect(),
            cache: Default::default(),
        })
    }

    /// How many clips the tree named that a zone held.
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }
}

impl AssetSource for BodySource {
    fn game(&self) -> vcod_common::game::Game {
        vcod_common::game::Game::Cod4
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.raw
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(path))
            .map(|(_, v)| v.clone())
    }

    fn exists(&self, path: &str) -> bool {
        self.raw.keys().any(|k| k.eq_ignore_ascii_case(path))
    }

    fn list(&self, dir: &str, ext: &str) -> Vec<String> {
        let prefix = match dir.trim_end_matches('/') {
            "" => String::new(),
            d => format!("{}/", d.to_ascii_lowercase()),
        };
        let mut out: Vec<String> = self
            .raw
            .keys()
            .map(|k| k.to_ascii_lowercase())
            .filter(|k| k.starts_with(&prefix) && k.ends_with(&ext.to_ascii_lowercase()))
            .collect();
        out.sort();
        out
    }

    fn cache(&self) -> &vcod_common::asset_source::AssetCache {
        &self.cache
    }

    fn decoded_anim(&self, name: &str) -> Option<Result<cxanim::XAnim>> {
        Some(
            self.clips
                .get(&name.to_ascii_lowercase())
                .cloned()
                .with_context(|| format!("no zone holds xanim {name}")),
        )
    }

    fn decoded_model(&self, name: &str) -> Option<Result<Model>> {
        Some(
            self.models
                .get(&name.to_ascii_lowercase())
                .cloned()
                .with_context(|| format!("no zone holds xmodel {name}")),
        )
    }
}
