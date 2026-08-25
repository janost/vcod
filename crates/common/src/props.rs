//! `misc_model` props from the BSP entity lump: baked to world space in the
//! map vertex format so they draw unlit through the map pipeline (`build`),
//! and their solid collision triangles for the collision world
//! (`collision_tris`).

use crate::bsp::{self, DrawVert};
use crate::collision::CONTENTS_SOLID;
use crate::pk3::Pk3Fs;
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
    /// `lightingPrecalc` as an RGBA vertex colour; white when absent.
    pub color: [u8; 4],
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
}

/// Q3 `AnglesToAxis` (code/game/q_math.c): `Rz(yaw) * Ry(pitch) * Rx(roll)`,
/// with PITCH/YAW/ROLL at indices 0/1/2. Pinned by `rotation_matches_q3_angles_to_axis`.
fn rotation(angles: Vec3) -> Mat3 {
    Mat3::from_rotation_z(angles.y.to_radians())
        * Mat3::from_rotation_y(angles.x.to_radians())
        * Mat3::from_rotation_x(angles.z.to_radians())
}

/// q3map2 reads `modelscale` with `FloatForKey` and applies it only when
/// non-zero. Also guards `bake`'s normal division.
fn scale_or_one(v: f32) -> f32 {
    if v.is_finite() && v != 0.0 {
        v
    } else {
        1.0
    }
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
        // "lightingPrecalc" stands in for the lightmap props never get
        let color = e
            .get("lightingPrecalc")
            .and_then(|s| bsp::parse_vec3(s))
            .map_or([255; 4], |c| {
                let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                [b(c[0]), b(c[1]), b(c[2]), 255]
            });

        out.push(Placement {
            model: model.to_string(),
            origin: Vec3::from_array(origin),
            angles: Vec3::from_array(angles),
            scale: Vec3::from_array(scale),
            color,
        });
    }
    out
}

/// Scale, rotate, translate. Normals use the inverse-transpose (`R * S⁻¹`) so
/// a non-uniform `modelscale_vec` doesn't skew them.
fn bake(p: &Placement, rot: Mat3, v: &xmodel::VmVert) -> DrawVert {
    let pos = rot * (p.scale * Vec3::from_array(v.pos)) + p.origin;
    let normal = rot * (Vec3::from_array(v.normal) / p.scale);
    DrawVert {
        pos: pos.to_array(),
        uv: v.uv,
        // props have no lightmap; the renderer binds the white 1x1 page
        lm_uv: [0.0, 0.0],
        normal: normal.normalize_or_zero().to_array(),
        color: p.color,
    }
}

/// A model that fails to load is warned once and its placements dropped.
fn load_model(fs: &Pk3Fs, name: &str) -> Option<xmodel::XModel> {
    xmodel::load(fs, name)
        .map_err(|e| log::warn!("prop {name}: {e:#}, skipping its placements"))
        .ok()
}

/// Bakes every placed prop into world geometry, one batch per skin.
pub fn build(fs: &Pk3Fs, entities: &str) -> Props {
    let placements = placements(entities);
    let mut cache: HashMap<String, Option<xmodel::XModel>> = HashMap::new();
    let mut verts: Vec<DrawVert> = Vec::new();
    let mut groups: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut drawn = 0usize;

    for p in &placements {
        let model = cache
            .entry(p.model.clone())
            .or_insert_with(|| load_model(fs, &p.model));
        let Some(model) = model else { continue };
        drawn += 1;
        let rot = rotation(p.angles);
        for surf in &model.surfaces {
            let Some(skin) = model.materials.get(surf.material) else {
                continue;
            };
            let base = verts.len() as u32;
            verts.extend(surf.verts.iter().map(|v| bake(p, rot, v)));
            groups
                .entry(skin.clone())
                .or_default()
                .extend(surf.indices.iter().map(|&i| base + i as u32));
        }
    }

    let mut indices = Vec::new();
    let mut batches = Vec::new();
    for (skin, idx) in groups {
        batches.push(Batch {
            skin,
            first_index: indices.len() as u32,
            index_count: idx.len() as u32,
        });
        indices.extend(idx);
    }
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
    }
}

/// World-space triangles of one placement's solid collision surfaces, placed
/// like `bake` places render vertices. Canopies and glass (`contents` without
/// the solid bit) are left out, as retail leaves them passable.
pub fn placed_collision_tris(p: &Placement, model: &xmodel::XModel, out: &mut Vec<[Vec3; 3]>) {
    let rot = rotation(p.angles);
    let place = |v: Vec3| rot * (p.scale * v) + p.origin;
    for surf in &model.collision {
        if surf.contents & CONTENTS_SOLID == 0 {
            continue;
        }
        out.extend(surf.tris.iter().map(|t| t.map(place)));
    }
}

/// Every solid collision triangle of every placed prop, for
/// `CollisionWorld::build`. Models load once each.
pub fn collision_tris(fs: &Pk3Fs, entities: &str) -> Vec<[Vec3; 3]> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_collision_keeps_solid_surfaces_and_transforms_them() {
        let tri = [Vec3::ZERO, Vec3::X, Vec3::Y];
        let model = xmodel::XModel {
            lod: "t0".into(),
            surfaces: vec![],
            materials: vec![],
            bones: vec![],
            collision: vec![
                xmodel::CollSurf {
                    contents: CONTENTS_SOLID,
                    flags: 0,
                    tris: vec![tri],
                },
                xmodel::CollSurf {
                    contents: 0,
                    flags: 0,
                    tris: vec![tri],
                },
            ],
        };
        let p = Placement {
            model: "t".into(),
            origin: Vec3::new(10.0, 0.0, 0.0),
            angles: Vec3::new(0.0, 90.0, 0.0),
            scale: Vec3::splat(2.0),
            color: [255; 4],
        };
        let mut out = Vec::new();
        placed_collision_tris(&p, &model, &mut out);
        assert_eq!(out.len(), 1, "the contents-0 surface is skipped");
        // scale first, then yaw 90 turns +X into +Y and +Y into -X
        assert!(
            out[0][0].abs_diff_eq(Vec3::new(10.0, 0.0, 0.0), 1e-4),
            "{out:?}"
        );
        assert!(
            out[0][1].abs_diff_eq(Vec3::new(10.0, 2.0, 0.0), 1e-4),
            "{out:?}"
        );
        assert!(
            out[0][2].abs_diff_eq(Vec3::new(8.0, 0.0, 0.0), 1e-4),
            "{out:?}"
        );
    }

    /// End to end on retail data: a ray dropped onto a placed prop stops
    /// earlier with the prop soup than without it, for at least one prop.
    #[test]
    fn mp_pavlov_props_stop_traces() {
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
            let with = world.box_trace(top, bottom, Vec3::ZERO, Vec3::ZERO);
            let without = bare.box_trace(top, bottom, Vec3::ZERO, Vec3::ZERO);
            with.fraction < without.fraction - 0.01
        });
        assert!(blocked);
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
                color: [128, 64, 0, 255],
            }
        );
        assert_eq!(
            p[1],
            Placement {
                model: "crate_misc1".into(),
                origin: Vec3::new(1.0, 2.0, 3.0),
                angles: Vec3::ZERO,
                scale: Vec3::ONE,
                color: [255; 4],
            }
        );
    }

    #[test]
    fn angle_key_is_a_yaw_only_fallback() {
        let ents = "{\n\"model\" \"xmodel/a\"\n\"angle\" \"45\"\n\"classname\" \"misc_model\"\n}";
        assert_eq!(placements(ents)[0].angles, Vec3::new(0.0, 45.0, 0.0));
        // a full "angles" triple wins over the shorthand
        let both =
            "{\n\"model\" \"xmodel/a\"\n\"angle\" \"45\"\n\"angles\" \"1 2 3\"\n\"classname\" \"misc_model\"\n}";
        assert_eq!(placements(both)[0].angles, Vec3::new(1.0, 2.0, 3.0));
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
            color: [10, 20, 30, 255],
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
        assert_eq!(out.color, [10, 20, 30, 255]);
    }

    /// Counts measured from the shipped BSP.
    #[test]
    fn parses_real_mp_neuville_props() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let data = fs.read("maps/mp/mp_neuville.bsp").unwrap();
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

    /// Size alone doesn't say: some real skins (`metal@civcar2tread.dds`) are
    /// 64x64 too, so the pixels must match.
    fn is_checkerboard(img: &crate::assets::Image) -> bool {
        use crate::assets::{checkerboard, ImageData};
        let (ImageData::Rgba8(px), ImageData::Rgba8(cb)) = (&img.data, checkerboard().data) else {
            return false;
        };
        (img.width, img.height) == (64, 64) && *px == cb
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
                !is_checkerboard(&load_skin_image(&fs, skin)),
                "{skin} fell back to the checkerboard"
            );
        }
    }

    #[test]
    fn builds_mp_neuville_props() {
        use crate::assets::load_skin_image;
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let data = fs.read("maps/mp/mp_neuville.bsp").unwrap();
        let bsp = bsp::parse(&data).unwrap();
        let props = build(&fs, &bsp.entities);
        assert!(props.verts.len() > 10_000, "{}", props.verts.len());
        assert!(!props.batches.is_empty());
        let total: u32 = props.batches.iter().map(|b| b.index_count).sum();
        assert_eq!(total as usize, props.indices.len());
        assert!(props
            .indices
            .iter()
            .all(|&i| (i as usize) < props.verts.len()));
        for b in &props.batches {
            assert!(
                !is_checkerboard(&load_skin_image(&fs, &b.skin)),
                "{} fell back to the checkerboard",
                b.skin
            );
        }
    }
}
