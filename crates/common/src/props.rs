//! `misc_model` props from the BSP entity lump: baked to world space in the
//! map vertex format with their lighting in the vertex colour, so they draw
//! through the map pipeline (`build`), and their collision triangles for the
//! collision world (`collision_tris`).

use crate::bsp::{self, Bsp, DrawVert};
use crate::collision::ModelTri;
use crate::mesh::IndexRange;
use crate::pk3::Pk3Fs;
use crate::static_light::{self, ModelLights, StaticLighting};
use crate::xmodel;
use glam::{Mat3, Vec3};
use std::collections::{BTreeMap, HashMap};

/// One `misc_model` placement.
#[derive(Debug, PartialEq)]
pub struct Placement {
    /// xmodel entry name, `xmodel/` prefix stripped.
    pub model: String,
    pub origin: Vec3,
    /// Quake pitch, yaw, roll in degrees (see `rotation`).
    pub angles: Vec3,
    /// Per-axis scale from `modelscale`/`modelscale_vec`.
    pub scale: Vec3,
    /// `lightingPrecalc`, clamped to 0..1; white when absent.
    pub precalc: Vec3,
}

/// Retail's static-model loader skips every `xmodel/shadow_*` placement
/// (`strnicmp`, 0x4dbae0): those models only cast the compiler's lightmap
/// shadows and are never drawn (cod11-light-grid-and-leaf-lights.md 1).
pub fn is_unregistered(model: &str) -> bool {
    model
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("shadow_"))
}

/// One prop draw batch: every triangle in the map that uses this skin.
pub struct Batch {
    /// Skin filename with extension under `skins/`, not a `textures/` material
    /// path; resolves through `assets::load_skin_image`.
    pub skin: String,
    pub first_index: u32,
    pub index_count: u32,
}

/// All props of a map, ready to append to the map's vertex/index buffers.
pub struct Props {
    pub verts: Vec<DrawVert>,
    /// Relative to `verts`; the renderer rebases them onto the combined buffer.
    pub indices: Vec<u32>,
    pub batches: Vec<Batch>,
    /// (placement index, range) for every placement/skin pair; `batch`
    /// indexes `batches`.
    pub ranges: Vec<(u32, IndexRange)>,
    /// World AABB per placement, index-aligned with `placements(entities)`.
    pub bounds: Vec<(Vec3, Vec3)>,
}

/// Q3 `AnglesToAxis` (code/game/q_math.c): `Rz(yaw) * Ry(pitch) * Rx(roll)`,
/// with PITCH/YAW/ROLL at indices 0/1/2. Pinned by `rotation_matches_q3_angles_to_axis`.
pub fn rotation(angles: Vec3) -> Mat3 {
    Mat3::from_rotation_z(angles.y.to_radians())
        * Mat3::from_rotation_y(angles.x.to_radians())
        * Mat3::from_rotation_x(angles.z.to_radians())
}

/// q3map2 reads `modelscale` with `FloatForKey` and applies it only when
/// non-zero. Also guards `bake`'s normal division.
fn scale_or_one(v: f32) -> f32 {
    if v.is_finite() && v != 0.0 { v } else { 1.0 }
}

/// Every `misc_model`. Other classnames with a `model` key (spawn points name
/// `xmodel/airborne`) are not world geometry.
pub fn placements(entities: &str) -> Vec<Placement> {
    let mut out = Vec::new();
    for e in bsp::entity_blocks(entities) {
        if e.get("classname").map(String::as_str) != Some("misc_model") {
            continue;
        }
        // some values spell paths with '\'
        let Some(model) = e.get("model").map(|m| m.replace('\\', "/")) else {
            continue;
        };
        let Some(model) = model.strip_prefix("xmodel/") else {
            log::warn!("misc_model references non-xmodel '{model}', skipping it");
            continue;
        };

        let origin = e
            .get("origin")
            .and_then(|s| bsp::parse_vec3(s))
            .unwrap_or([0.0; 3]);
        // "angles" is the full triple; a bare "angle" is a yaw-only shorthand
        let angles = e
            .get("angles")
            .and_then(|s| bsp::parse_vec3(s))
            .or_else(|| {
                let yaw: f32 = e.get("angle")?.trim().parse().ok()?;
                Some([0.0, yaw, 0.0])
            })
            .unwrap_or([0.0; 3]);
        // q3map2 precedence (model.c): "modelscale" seeds all axes, then
        // "modelscale_vec" overwrites them. Retail maps carry both keys.
        let uniform = e
            .get("modelscale")
            .and_then(|s| s.trim().parse::<f32>().ok())
            .map_or(1.0, scale_or_one);
        let scale = e
            .get("modelscale_vec")
            .and_then(|s| bsp::parse_vec3(s))
            .map_or([uniform; 3], |v| v.map(scale_or_one));
        let precalc = e
            .get("lightingPrecalc")
            .and_then(|s| bsp::parse_vec3(s))
            .map_or(Vec3::ONE, |c| {
                Vec3::from_array(c).clamp(Vec3::ZERO, Vec3::ONE)
            });

        out.push(Placement {
            model: model.to_string(),
            origin: Vec3::from_array(origin),
            angles: Vec3::from_array(angles),
            scale: Vec3::from_array(scale),
            precalc,
        });
    }
    out
}

/// Scale, rotate, translate. Normals use the inverse-transpose (`R * S⁻¹`) so
/// a non-uniform `modelscale_vec` doesn't skew them. The colour is filled in
/// by `build` once the lighting is known.
fn bake(p: &Placement, rot: Mat3, v: &xmodel::VmVert) -> DrawVert {
    let pos = rot * (p.scale * Vec3::from_array(v.pos)) + p.origin;
    let normal = rot * (Vec3::from_array(v.normal) / p.scale);
    DrawVert {
        pos: pos.to_array(),
        uv: v.uv,
        // props have no lightmap; the renderer binds the white 1x1 page
        lm_uv: [0.0, 0.0],
        normal: normal.normalize_or_zero().to_array(),
        color: [255; 4],
    }
}

/// How a prop skin's stage colours its vertices: the `rgbGen` of its
/// `shadertypes/model/<type>.stype`, `<type>` being the skin name before
/// `@`. A skin without one is an implicit model skin, `lightingDiffuse`
/// (0x4fc440).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SkinGen {
    Diffuse,
    Precalc,
    IdentityLighting,
    ConstLighting(Vec3),
    /// Anything else (`wave`): drawn with the precalc colour.
    Other,
}

/// The first stage `rgbGen` in a shader body.
pub fn parse_skin_gen(text: &str) -> SkinGen {
    let mut toks = text
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .flat_map(str::split_whitespace);
    while let Some(t) = toks.next() {
        if !t.eq_ignore_ascii_case("rgbgen") {
            continue;
        }
        let Some(kind) = toks.next() else { break };
        return match kind.to_ascii_lowercase().as_str() {
            "lightingdiffuse" => SkinGen::Diffuse,
            "lightingprecalc" => SkinGen::Precalc,
            "identitylighting" => SkinGen::IdentityLighting,
            "constlighting" => {
                let v: Vec<f32> = toks
                    .by_ref()
                    .skip_while(|t| *t == "(")
                    .take(3)
                    .filter_map(|t| t.parse().ok())
                    .collect();
                match v[..] {
                    [r, g, b] => SkinGen::ConstLighting(Vec3::new(r, g, b)),
                    _ => SkinGen::Other,
                }
            }
            _ => SkinGen::Other,
        };
    }
    SkinGen::Other
}

fn skin_gen(fs: &Pk3Fs, skin: &str) -> SkinGen {
    let Some(at) = skin.rfind('@') else {
        return SkinGen::Diffuse;
    };
    let start = skin[..at].rfind(['/', '\\']).map_or(0, |i| i + 1);
    let kind = skin[start..at].to_ascii_lowercase();
    match fs.read(&format!("shadertypes/model/{kind}.stype")) {
        Some(text) => parse_skin_gen(&String::from_utf8_lossy(&text)),
        None => SkinGen::Diffuse,
    }
}

/// A model that fails to load is warned once and its placements dropped.
fn load_model(fs: &Pk3Fs, name: &str) -> Option<xmodel::XModel> {
    xmodel::load(fs, name)
        .map_err(|e| log::warn!("prop {name}: {e:#}, skipping its placements"))
        .ok()
}

/// Bakes every placed prop into world geometry, one batch per skin,
/// lit from the map's lights. `lighting` keeps the grid samples it filled,
/// which entity models go on to share.
pub fn build(fs: &Pk3Fs, bsp: &Bsp, lighting: &mut StaticLighting) -> Props {
    build_with(fs, &bsp.entities, Some(lighting))
}

fn build_with(fs: &Pk3Fs, entities: &str, mut lighting: Option<&mut StaticLighting>) -> Props {
    let placements = placements(entities);
    let mut gens: HashMap<String, SkinGen> = HashMap::new();
    // (placement, vertex range, gen) per surface, and the normal each
    // vertex is lit with: retail's, the scaled axis renormalised
    let mut surf_runs: Vec<(usize, usize, usize, SkinGen)> = Vec::new();
    let mut lit_normals: Vec<Vec3> = Vec::new();
    let mut cache: HashMap<String, Option<xmodel::XModel>> = HashMap::new();
    let mut verts: Vec<DrawVert> = Vec::new();
    let mut groups: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut drawn = 0usize;

    // per placement: skin -> (offset in the group, count); plus bounds
    let mut runs: Vec<(u32, String, u32, u32)> = Vec::new();
    let mut bounds: Vec<(Vec3, Vec3)> = Vec::new();
    for (pi, p) in placements.iter().enumerate() {
        if is_unregistered(&p.model) {
            bounds.push((Vec3::ZERO, Vec3::ZERO));
            continue;
        }
        let model = cache
            .entry(p.model.clone())
            .or_insert_with(|| load_model(fs, &p.model));
        let Some(model) = model else {
            bounds.push((Vec3::ZERO, Vec3::ZERO));
            continue;
        };
        drawn += 1;
        let rot = rotation(p.angles);
        let mut lo = Vec3::INFINITY;
        let mut hi = Vec3::NEG_INFINITY;
        // a placement's surfaces sharing a skin land consecutively in that
        // skin's group, so one (offset, count) per skin stays contiguous
        let mut this: HashMap<&str, (u32, u32)> = HashMap::new();
        for surf in &model.surfaces {
            let Some(skin) = model.materials.get(surf.material) else {
                continue;
            };
            let base = verts.len() as u32;
            for v in &surf.verts {
                let dv = bake(p, rot, v);
                lo = lo.min(Vec3::from(dv.pos));
                hi = hi.max(Vec3::from(dv.pos));
                verts.push(dv);
                lit_normals
                    .push((rot * (p.scale * Vec3::from_array(v.normal))).normalize_or_zero());
            }
            let sg = *gens
                .entry(skin.clone())
                .or_insert_with(|| skin_gen(fs, skin));
            surf_runs.push((pi, base as usize, verts.len(), sg));
            let group = groups.entry(skin.clone()).or_default();
            let run = this.entry(skin.as_str()).or_insert((group.len() as u32, 0));
            group.extend(surf.indices.iter().map(|&i| base + i as u32));
            run.1 += surf.indices.len() as u32;
        }
        for (skin, (offset, count)) in this {
            runs.push((pi as u32, skin.to_string(), offset, count));
        }
        bounds.push(if lo.x <= hi.x {
            (lo, hi)
        } else {
            (Vec3::ZERO, Vec3::ZERO)
        });
    }

    // Retail registers static models onto a list it then lights head first,
    // so in reverse entity order; that order fills the grid cache.
    let mut model_lights: Vec<Option<ModelLights>> = vec![None; placements.len()];
    if let Some(l) = lighting.as_mut() {
        let t = std::time::Instant::now();
        for pi in (0..placements.len()).rev() {
            let (lo, hi) = bounds[pi];
            if lo == hi {
                continue;
            }
            model_lights[pi] = Some(l.model_lights((lo + hi) * 0.5));
        }
        log::info!(
            "props: lit in {:.0} ms, {} light grid samples traced",
            t.elapsed().as_secs_f64() * 1000.0,
            l.misses
        );
    }
    // Vertex alpha 255 marks a `lightingDiffuse` skin, which dynamic lights
    // reach (`vs_prop` in the client's shader.wgsl); the other gens read 0.
    let identity = static_light::identity_byte();
    for (pi, first, end, sg) in surf_runs {
        let p = &placements[pi];
        let precalc = static_light::precalc_byte;
        for (v, n) in verts[first..end].iter_mut().zip(&lit_normals[first..end]) {
            v.color = match sg {
                SkinGen::Diffuse => match (&lighting, &model_lights[pi]) {
                    (Some(l), Some(m)) => l.vertex_color(m, Vec3::from(v.pos), *n),
                    _ => [identity, identity, identity, 255],
                },
                SkinGen::IdentityLighting => [identity, identity, identity, 0],
                SkinGen::ConstLighting(c) => [precalc(c.x), precalc(c.y), precalc(c.z), 0],
                SkinGen::Precalc | SkinGen::Other => [
                    precalc(p.precalc.x),
                    precalc(p.precalc.y),
                    precalc(p.precalc.z),
                    0,
                ],
            };
        }
    }

    let mut indices = Vec::new();
    let mut batches = Vec::new();
    let mut batch_of: HashMap<String, (u32, u32)> = HashMap::new();
    for (skin, idx) in groups {
        batch_of.insert(skin.clone(), (batches.len() as u32, indices.len() as u32));
        batches.push(Batch {
            skin,
            first_index: indices.len() as u32,
            index_count: idx.len() as u32,
        });
        indices.extend(idx);
    }
    let ranges = runs
        .into_iter()
        .map(|(pi, skin, offset, count)| {
            let (batch, first) = batch_of[&skin];
            (
                pi,
                IndexRange {
                    batch,
                    first: first + offset,
                    count,
                },
            )
        })
        .collect();
    log::info!(
        "props: {drawn}/{} placements over {} models, {} vertices in {} skin batches",
        placements.len(),
        cache.values().filter(|m| m.is_some()).count(),
        verts.len(),
        batches.len()
    );
    Props {
        verts,
        indices,
        batches,
        ranges,
        bounds,
    }
}

/// World-space triangles of one placement's collision surfaces, placed like
/// `bake` places render vertices, each with its surface's `contents` and
/// flags for the trace mask and the sound material. A surface with contents
/// 0 (a tree canopy, a hanging sign) can match no mask and is left out.
pub fn placed_collision_tris(p: &Placement, model: &xmodel::XModel, out: &mut Vec<ModelTri>) {
    let rot = rotation(p.angles);
    let place = |v: Vec3| rot * (p.scale * v) + p.origin;
    for surf in &model.collision {
        if surf.contents == 0 {
            continue;
        }
        out.extend(surf.tris.iter().map(|t| ModelTri {
            tri: t.map(place),
            contents: surf.contents,
            surface_flags: surf.flags,
        }));
    }
}

/// Every collision triangle of every placed prop, for
/// `CollisionWorld::build`. Models load once each.
pub fn collision_tris(fs: &Pk3Fs, entities: &str) -> Vec<ModelTri> {
    let placements = placements(entities);
    let mut cache: HashMap<String, Option<xmodel::XModel>> = HashMap::new();
    let mut out = Vec::new();
    let mut collidable = 0usize;
    for p in &placements {
        let model = cache
            .entry(p.model.clone())
            .or_insert_with(|| load_model(fs, &p.model));
        let Some(model) = model else { continue };
        let before = out.len();
        placed_collision_tris(p, model, &mut out);
        collidable += (out.len() > before) as usize;
    }
    log::info!(
        "props: {collidable}/{} placements collide, {} triangles",
        placements.len(),
        out.len()
    );
    out
}

/// The world box the engine files each collidable `misc_model` under in its
/// entity area tree, in entity-string order. VERIFIED, `cod_lnxded`: the
/// loader (0x8051420) takes the eight corners of every collision surface's
/// stored bounds through the scaled axis (0x80c241c), adds the origin, and
/// links the model (0x80594c4) only when the OR of its surfaces' contents,
/// masked `& 0xdfff7ffb`, is non-zero. INFERRED: the stored bounds are read
/// unbaked, in the bone's space, which is model space for a rigid prop.
pub fn area_bounds(fs: &Pk3Fs, entities: &str) -> Vec<(Vec3, Vec3)> {
    let mut cache: HashMap<String, Option<xmodel::XModel>> = HashMap::new();
    let mut out = Vec::new();
    for p in placements(entities) {
        let model = cache
            .entry(p.model.clone())
            .or_insert_with(|| load_model(fs, &p.model));
        let Some(model) = model else { continue };
        let contents = model.collision.iter().fold(0, |c, s| c | s.contents) & 0xdfff_7ffb;
        if model.collision.is_empty() || contents == 0 {
            continue;
        }
        let rot = rotation(p.angles);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for surf in &model.collision {
            let (a, b) = surf.bounds;
            for corner in 0..8 {
                let c = Vec3::new(
                    if corner & 1 == 0 { b.x } else { a.x },
                    if corner & 2 == 0 { b.y } else { a.y },
                    if corner & 4 == 0 { b.z } else { a.z },
                );
                let w = rot * (p.scale * c);
                lo = lo.min(w);
                hi = hi.max(w);
            }
        }
        out.push((lo + p.origin, hi + p.origin));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_collision_keeps_contents_and_transforms_them() {
        let tri = [Vec3::ZERO, Vec3::X, Vec3::Y];
        let model = xmodel::XModel {
            lod: "t0".into(),
            surfaces: vec![],
            materials: vec![],
            bones: vec![],
            collision: vec![
                xmodel::CollSurf {
                    contents: crate::collision::CONTENTS_SOLID,
                    flags: 21 << 20,
                    tris: vec![tri],
                    bounds: (Vec3::ZERO, Vec3::ONE),
                },
                xmodel::CollSurf {
                    contents: 0,
                    flags: 0,
                    tris: vec![tri],
                    bounds: (Vec3::ZERO, Vec3::ONE),
                },
            ],
        };
        let p = Placement {
            model: "t".into(),
            origin: Vec3::new(10.0, 0.0, 0.0),
            angles: Vec3::new(0.0, 90.0, 0.0),
            scale: Vec3::splat(2.0),
            precalc: Vec3::ONE,
        };
        let mut out = Vec::new();
        placed_collision_tris(&p, &model, &mut out);
        assert_eq!(out.len(), 1, "the contents-0 surface is skipped");
        assert_eq!(out[0].contents, crate::collision::CONTENTS_SOLID);
        assert_eq!(crate::collision::sound_material(out[0].surface_flags), 21);
        // scale first, then yaw 90 turns +X into +Y and +Y into -X
        let t = out[0].tri;
        assert!(t[0].abs_diff_eq(Vec3::new(10.0, 0.0, 0.0), 1e-4), "{t:?}");
        assert!(t[1].abs_diff_eq(Vec3::new(10.0, 2.0, 0.0), 1e-4), "{t:?}");
        assert!(t[2].abs_diff_eq(Vec3::new(8.0, 0.0, 0.0), 1e-4), "{t:?}");
    }

    /// End to end on retail data: a shot dropped onto a placed prop stops
    /// earlier with the prop mesh than without it, for at least one prop,
    /// and a movement trace never does.
    #[test]
    fn mp_pavlov_props_stop_shots_and_not_moves() {
        use crate::collision::CollisionWorld;
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let bsp = bsp::parse(&crate::testing::real_bsp().unwrap()).unwrap();
        let tris = collision_tris(&fs, &bsp.entities);
        assert!(tris.len() > 1000, "{}", tris.len());
        let bare = CollisionWorld::build(&bsp, &[]);
        let world = CollisionWorld::build(&bsp, &tris);
        let blocked = placements(&bsp.entities).iter().any(|p| {
            let (top, bottom) = (p.origin + Vec3::Z * 200.0, p.origin - Vec3::Z * 10.0);
            let with = world.shot_trace(top, bottom);
            let without = bare.shot_trace(top, bottom);
            with.fraction < without.fraction - 0.01
        });
        assert!(blocked);
        let moved = placements(&bsp.entities).iter().all(|p| {
            let (top, bottom) = (p.origin + Vec3::Z * 200.0, p.origin - Vec3::Z * 10.0);
            let with = world.box_trace(top, bottom, Vec3::ZERO, Vec3::ZERO);
            let without = bare.box_trace(top, bottom, Vec3::ZERO, Vec3::ZERO);
            (with.fraction - without.fraction).abs() < 1e-6
        });
        assert!(moved, "a movement trace met a prop");
    }

    const ENTS: &str = r#"{
"classname" "worldspawn"
}
{
"model" "xmodel/fullspikeyshrub"
"origin" "10 20 30"
"angles" "5 90 15"
"modelscale" "2"
"lightingPrecalc" "0.5 0.25 0"
"classname" "misc_model"
}
{
"model" "xmodel/crate_misc1"
"origin" "1 2 3"
"classname" "misc_model"
}
{
"model" "xmodel/airborne"
"origin" "4 5 6"
"classname" "mp_deathmatch_spawn"
}
"#;

    #[test]
    fn parses_misc_models_with_defaults() {
        let p = placements(ENTS);
        assert_eq!(p.len(), 2, "only misc_model entities count");
        assert_eq!(
            p[0],
            Placement {
                model: "fullspikeyshrub".into(),
                origin: Vec3::new(10.0, 20.0, 30.0),
                angles: Vec3::new(5.0, 90.0, 15.0),
                scale: Vec3::splat(2.0),
                precalc: Vec3::new(0.5, 0.25, 0.0),
            }
        );
        assert_eq!(
            p[1],
            Placement {
                model: "crate_misc1".into(),
                origin: Vec3::new(1.0, 2.0, 3.0),
                angles: Vec3::ZERO,
                scale: Vec3::ONE,
                precalc: Vec3::ONE,
            }
        );
    }

    #[test]
    fn angle_key_is_a_yaw_only_fallback() {
        let ents = "{\n\"model\" \"xmodel/a\"\n\"angle\" \"45\"\n\"classname\" \"misc_model\"\n}";
        assert_eq!(placements(ents)[0].angles, Vec3::new(0.0, 45.0, 0.0));
        // a full "angles" triple wins over the shorthand
        let both = "{\n\"model\" \"xmodel/a\"\n\"angle\" \"45\"\n\"angles\" \"1 2 3\"\n\"classname\" \"misc_model\"\n}";
        assert_eq!(placements(both)[0].angles, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn shadow_models_are_unregistered() {
        assert!(is_unregistered("shadow_crate"));
        assert!(is_unregistered("Shadow_tree_pine_mid"));
        // only the prefix counts, as in retail's strnicmp
        assert!(!is_unregistered("FullSpikeyShrub_shadow"));
        assert!(!is_unregistered("fullspikeyshrub_nocol_noshadow"));
        assert!(!is_unregistered("crate_misc1"));
    }

    #[test]
    fn modelscale_vec_overrides_modelscale() {
        let ents = "{\n\"model\" \"xmodel/a\"\n\"modelscale\" \"3.5\"\n\"modelscale_vec\" \"0.8 0.8 0.7\"\n\"classname\" \"misc_model\"\n}";
        assert_eq!(placements(ents)[0].scale, Vec3::new(0.8, 0.8, 0.7));
        // zero means unset, like q3map2's FloatForKey check
        let zero =
            "{\n\"model\" \"xmodel/a\"\n\"modelscale\" \"0\"\n\"classname\" \"misc_model\"\n}";
        assert_eq!(placements(zero)[0].scale, Vec3::ONE);
    }

    #[test]
    fn skips_non_xmodel_references() {
        let ents = "{\n\"model\" \"*3\"\n\"classname\" \"misc_model\"\n}";
        assert!(placements(ents).is_empty());
    }

    /// Q3 `AngleVectors` transcribed; forward/-right/up are the matrix columns.
    #[test]
    fn rotation_matches_q3_angles_to_axis() {
        for angles in [
            Vec3::new(0.0, 90.0, 0.0),
            Vec3::new(278.246, 163.945, 165.916),
            Vec3::new(-30.0, 200.0, 45.0),
        ] {
            let (p, y, r) = (
                angles.x.to_radians(),
                angles.y.to_radians(),
                angles.z.to_radians(),
            );
            let (sp, cp) = (p.sin(), p.cos());
            let (sy, cy) = (y.sin(), y.cos());
            let (sr, cr) = (r.sin(), r.cos());
            let forward = Vec3::new(cp * cy, cp * sy, -sp);
            let right = Vec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
            let up = Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);

            let m = rotation(angles);
            assert!((m.x_axis - forward).length() < 1e-5, "{angles} forward");
            assert!((m.y_axis + right).length() < 1e-5, "{angles} left");
            assert!((m.z_axis - up).length() < 1e-5, "{angles} up");
        }
    }

    #[test]
    fn bakes_vertices_into_world_space() {
        let p = Placement {
            model: "a".into(),
            origin: Vec3::new(100.0, 0.0, 8.0),
            angles: Vec3::new(0.0, 90.0, 0.0), // yaw +90: +x turns into +y
            scale: Vec3::new(2.0, 2.0, 4.0),
            precalc: Vec3::ONE,
        };
        let v = xmodel::VmVert {
            pos: [1.0, 0.0, 1.0],
            normal: [1.0, 0.0, 0.0],
            uv: [0.25, 0.75],
            bone_indices: [0; 4],
            bone_weights: [1.0, 0.0, 0.0, 0.0],
        };
        let out = bake(&p, rotation(p.angles), &v);
        // scale (2,0,4), yaw 90 -> (0,2,4), translate -> (100,2,12)
        assert!((Vec3::from_array(out.pos) - Vec3::new(100.0, 2.0, 12.0)).length() < 1e-4);
        // the normal follows the yaw and stays unit length under non-uniform scale
        assert!((Vec3::from_array(out.normal) - Vec3::Y).length() < 1e-5);
        assert_eq!(out.uv, [0.25, 0.75]);
        assert_eq!(out.lm_uv, [0.0, 0.0]);
    }

    /// Counts measured from the shipped BSP.
    #[test]
    fn parses_real_mp_neuville_props() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        // mp_neuville is a UO map; a stock 1.1 install does not carry it.
        let Some(data) = fs.read("maps/mp/mp_neuville.bsp") else {
            return;
        };
        let bsp = bsp::parse(&data).unwrap();
        let p = placements(&bsp.entities);
        assert_eq!(p.len(), 236);

        let mut counts: HashMap<&str, usize> = HashMap::new();
        for pl in &p {
            *counts.entry(pl.model.as_str()).or_default() += 1;
        }
        assert_eq!(counts.len(), 52);
        assert_eq!(counts["brush_hedgrowwall1"], 98);
        assert_eq!(counts["fullspikeyshrub"], 28);
        assert_eq!(counts["hedgehog_lp"], 14);
        assert_eq!(counts["crate_ger_rola"], 8);
    }

    /// Size alone doesn't say: a real skin could be 16x16 too, so the pixels
    /// must match.
    fn is_default_image(img: &crate::assets::Image) -> bool {
        use crate::assets::{ImageData, default_image};
        let (ImageData::Rgba8(px), ImageData::Rgba8(d)) = (&img.data, default_image().data) else {
            return false;
        };
        (img.width, img.height) == (16, 16) && *px == d
    }

    #[test]
    fn loads_a_real_prop_model_and_its_skins() {
        use crate::assets::load_skin_image;
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let shrub = xmodel::load(&fs, "fullspikeyshrub").unwrap();
        assert!(!shrub.surfaces.is_empty());
        assert!(shrub.surfaces.iter().all(|s| !s.verts.is_empty()));
        assert_eq!(shrub.materials.len(), shrub.surfaces.len());

        // prop materials are skin filenames under skins/, not textures/ paths
        for skin in &shrub.materials {
            assert!(skin.contains('.'), "expected a filename, got {skin}");
            assert!(
                !is_default_image(&load_skin_image(&fs, skin)),
                "{skin} fell back to the default image"
            );
        }
    }

    #[test]
    fn build_reports_placement_ranges_and_bounds() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let ents = "{\n\"model\" \"xmodel/crate_misc1a\"\n\"origin\" \"100 200 300\"\n\"classname\" \"misc_model\"\n}";
        let props = build_with(&fs, ents, None);
        assert_eq!(props.bounds.len(), 1);
        let (lo, hi) = props.bounds[0];
        assert!(
            lo.x > 50.0 && hi.x < 150.0 && lo.z >= 300.0 && hi.z < 340.0,
            "{lo} {hi}"
        );
        let total: u32 = props.ranges.iter().map(|(_, r)| r.count).sum();
        assert_eq!(total as usize, props.indices.len());
        assert!(
            props
                .ranges
                .iter()
                .all(|(p, r)| *p == 0 && (r.batch as usize) < props.batches.len())
        );
        for (_, r) in &props.ranges {
            let b = &props.batches[r.batch as usize];
            assert!(r.first >= b.first_index && r.first + r.count <= b.first_index + b.index_count);
        }
    }

    #[test]
    fn builds_mp_neuville_props() {
        use crate::assets::load_skin_image;
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        // mp_neuville is a UO map; a stock 1.1 install does not carry it.
        let Some(data) = fs.read("maps/mp/mp_neuville.bsp") else {
            return;
        };
        let bsp = bsp::parse(&data).unwrap();
        let props = build(&fs, &bsp, &mut StaticLighting::new(&bsp));
        assert!(props.verts.len() > 10_000, "{}", props.verts.len());
        assert!(!props.batches.is_empty());
        let total: u32 = props.batches.iter().map(|b| b.index_count).sum();
        assert_eq!(total as usize, props.indices.len());
        assert!(
            props
                .indices
                .iter()
                .all(|&i| (i as usize) < props.verts.len())
        );
        for b in &props.batches {
            assert!(
                !is_default_image(&load_skin_image(&fs, &b.skin)),
                "{} fell back to the default image",
                b.skin
            );
        }
    }

    #[test]
    fn skin_gen_reads_the_first_stage() {
        let wood =
            "{\n\tsurfaceparm wood\n\t{\n\t\tmap $texturename\n\t\trgbGen lightingDiffuse\n\t}\n}";
        assert_eq!(parse_skin_gen(wood), SkinGen::Diffuse);
        let detail =
            "{\n\tradialNormals\n\t{\n\t\t//rgbGen identity\n\t\trgbGen lightingPrecalc\n\t}\n}";
        assert_eq!(parse_skin_gen(detail), SkinGen::Precalc);
        let objective = "{\n\t{\n\t\trgbgen constLighting ( 0.40 0.316 0.124 )\n\t}\n}";
        assert_eq!(
            parse_skin_gen(objective),
            SkinGen::ConstLighting(Vec3::new(0.4, 0.316, 0.124))
        );
        assert_eq!(
            parse_skin_gen("{ { rgbGen wave sin 0 1 0 1 } }"),
            SkinGen::Other
        );
    }

    #[test]
    fn lights_mp_carentan_props_per_vertex() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let Some(path) = fs.resolve_map("mp_carentan") else {
            return;
        };
        let bsp = bsp::parse(&fs.read(&path).unwrap()).unwrap();
        let props = build(&fs, &bsp, &mut StaticLighting::new(&bsp));
        let colors: std::collections::HashSet<[u8; 4]> =
            props.verts.iter().map(|v| v.color).collect();
        // one tint per placement would give a few hundred at most
        assert!(colors.len() > 2000, "{} distinct colours", colors.len());
        let mean = props
            .verts
            .iter()
            .map(|v| f32::from(v.color[1]))
            .sum::<f32>()
            / props.verts.len() as f32;
        assert!((40.0..200.0).contains(&mean), "mean green {mean}");
    }
}
