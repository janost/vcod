//! CoD1 static props under dynamic lights. A placement a scene light
//! reaches leaves the cached colours (0x505260): its `lightingDiffuse`
//! vertices are lit again from the pick with the scene lights in the eight
//! slots, and take the cache back once no light reaches it
//! (cod11-light-grid-and-leaf-lights.md 11).

use super::*;
use vcod_common::static_light::{self, SceneLight, StaticLighting};

/// The placements that can relight, with what each was last drawn with.
pub(super) struct PropRelight {
    /// The first prop vertex's index in the map's vertex buffer.
    base: u32,
    placements: Vec<Relit>,
}

struct Relit {
    /// Its lighting origin: LOD 0's box centre, as the bake's.
    centre: glam::Vec3,
    /// Its first vertex among the props'.
    first: u32,
    /// Its vertices as baked.
    verts: Vec<DrawVert>,
    /// Per vertex, the normal it is lit with; `None` for a skin the map
    /// does not light.
    normals: Vec<Option<glam::Vec3>>,
    /// The scene lights it was last lit with; empty while it draws baked.
    near: Vec<SceneLight>,
}

impl PropRelight {
    /// Every placement of `props` with a `lightingDiffuse` vertex; `base`
    /// is where the props start in the map's vertex buffer.
    pub(super) fn new(props: &props::Props, base: u32) -> Self {
        let mut placements = Vec::new();
        let mut start = 0;
        while start < props.verts.len() {
            let pi = props.vert_placement[start];
            let end = start
                + props.vert_placement[start..]
                    .iter()
                    .take_while(|&&p| p == pi)
                    .count();
            if props.vert_lit[start..end].iter().any(|&l| l) {
                let (lo, hi) = props.bounds[pi as usize];
                placements.push(Relit {
                    centre: (lo + hi) * 0.5,
                    first: start as u32,
                    verts: props.verts[start..end].to_vec(),
                    normals: (start..end)
                        .map(|v| props.vert_lit[v].then(|| props.vert_lit_normal[v].into()))
                        .collect(),
                    near: Vec::new(),
                });
            }
            start = end;
        }
        PropRelight { base, placements }
    }

    /// This frame's vertex writes: (first vertex in the map's buffer, the
    /// run). A placement a light of `scene` reaches is lit again whenever
    /// the lights reaching it change, and one no light reaches any more
    /// takes its baked colours back.
    pub(super) fn frame(
        &mut self,
        lighting: &mut StaticLighting,
        scene: &[SceneLight],
    ) -> Vec<(u32, Vec<DrawVert>)> {
        let mut out = Vec::new();
        for p in &mut self.placements {
            let near: Vec<SceneLight> = scene
                .iter()
                .filter(|l| static_light::reaches(p.centre, l))
                .copied()
                .collect();
            if near == p.near {
                continue;
            }
            let verts = if near.is_empty() {
                p.verts.clone()
            } else {
                let m = lighting.model_lights_near(p.centre, &near);
                p.verts
                    .iter()
                    .zip(&p.normals)
                    .map(|(v, n)| match n {
                        Some(n) => DrawVert {
                            color: lighting.vertex_color(&m, glam::Vec3::from(v.pos), *n),
                            ..*v
                        },
                        None => *v,
                    })
                    .collect()
            };
            p.near = near;
            out.push((self.base + p.first, verts));
        }
        out
    }
}

impl Renderer {
    /// Relights the props this frame's scene lights reach (CoD1 only;
    /// `r_dynamiclight` decides which lights models see).
    pub(super) fn relight_props(&mut self) {
        let Some(w) = self.world.as_mut() else {
            return;
        };
        let (Some(relight), Some(lighting)) =
            (w.prop_relight.as_mut(), self.dynamic.lighting.as_mut())
        else {
            return;
        };
        let scene = model_scene_lights(
            &self.dynamic.scene_lights,
            &self.dynamic.entity_scene_lights,
            self.dynamic_light,
        );
        let writes = relight.frame(lighting, &scene);
        for (first, verts) in &writes {
            self.queue.write_buffer(
                &w.vertex_buf,
                u64::from(*first) * std::mem::size_of::<DrawVert>() as u64,
                bytemuck::cast_slice(verts),
            );
        }
        #[cfg_attr(not(test), expect(unused_variables))]
        let bytes: usize = writes
            .iter()
            .map(|(_, v)| std::mem::size_of_val(v.as_slice()))
            .sum();
        frame_write!(self, "relight props", bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::static_light::SceneLight;

    /// On mp_carentan a light at a prop's centre relights it from the pick
    /// with the light in its eight slots, an unchanged light writes nothing,
    /// and once the light is gone the baked colours go back.
    #[test]
    fn a_reached_prop_relights_and_then_takes_its_cache_back() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let data = fs.read("maps/mp/mp_carentan.bsp").expect("mp_carentan");
        let bsp = vcod_common::mapworld::load(&data).unwrap();
        let mut lighting = StaticLighting::new(&bsp);
        let props = props::build(&fs, &bsp, &mut lighting, &|_| props::SkinDraw::default());
        let mut relight = PropRelight::new(&props, 1000);
        assert!(!relight.placements.is_empty());
        let (centre, first, baked) = {
            let p = &relight.placements[0];
            (p.centre, p.first, p.verts.clone())
        };
        let flash = SceneLight {
            origin: centre + glam::Vec3::Z * 16.0,
            color: glam::Vec3::new(1.0, 0.8, 0.5),
            intensity: 200.0,
        };
        // a light far from every prop reaches none
        let far = SceneLight {
            origin: glam::Vec3::splat(1e6),
            ..flash
        };
        assert!(relight.frame(&mut lighting, &[far]).is_empty());
        let writes = relight.frame(&mut lighting, &[flash]);
        let (at, verts) = writes
            .iter()
            .find(|(at, _)| *at == 1000 + first)
            .expect("the reached prop writes");
        assert_eq!(*at, 1000 + first);
        let m = lighting.model_lights_near(centre, &[flash]);
        assert!(
            m.lights.iter().any(|(l, _)| l.kind == 2),
            "the flash took a slot"
        );
        let p = &relight.placements[0];
        for ((v, b), n) in verts.iter().zip(&baked).zip(&p.normals) {
            match n {
                Some(n) => assert_eq!(
                    v.color,
                    lighting.vertex_color(&m, glam::Vec3::from(b.pos), *n)
                ),
                None => assert_eq!(v.color, b.color),
            }
        }
        assert!(verts.iter().zip(&baked).any(|(v, b)| v.color != b.color));
        assert!(relight.frame(&mut lighting, &[flash]).is_empty());
        let back = relight.frame(&mut lighting, &[]);
        let (_, verts) = back.iter().find(|(at, _)| *at == 1000 + first).unwrap();
        assert!(verts.iter().zip(&baked).all(|(v, b)| v.color == b.color));
    }
}
