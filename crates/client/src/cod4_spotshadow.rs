//! CoD4's spot light shadow maps (techniques 11 and 13,
//! docs/research/cod4-world.md 3.5): each frame up to `sm_maxLights` spot
//! primary lights that some visible surface stores, the best scoring
//! (`R_ShadowedSpotLightScore`), hold a place in a short
//! history whose entries fade in and out over `sm_spotShadowFadeTime`
//! (`R_ChooseShadowedLights`); each entry draws the sun's casters
//! from its light into its 512 tile of a 512 x 2048 map (the column right
//! of the sun's partitions in their atlas), and the lit world, static
//! model and entity shaders on that light read it (`cod4_spot_shadow` in
//! cod4_sunshadow_recv.wgsl). The passes and their pipelines are the
//! sun's (`cod4_sunshadow::SunShadowPass::encode_spot`); this module holds
//! the choice and the lights' matrices.

use glam::{Mat4, Vec3, Vec4};
use vcod_common::mapworld::ModelLightSource;

/// The map's size: four 512 tiles stacked (`R_RENDERTARGET_SHADOWMAP_SPOT`),
/// one a shadowed light.
pub(crate) const TILE: u32 = 512;
pub(crate) const TILES: u32 = 4;
/// `sm_maxLights`' maximum and default (0x62c15a).
pub const MAX_LIGHTS: usize = 4;

/// `spotShadowmapPixelAdjust` of a 512 tile (`R_AddSpotShadowsForLight`): the
/// four taps' offsets, in the map's coordinates.
pub(crate) const PIXEL_ADJUST: [f32; 4] = [1.0 / 2048.0, 1.0 / 4096.0, 1.0 / 1024.0, -1.0 / 8192.0];

/// The spot shadow dvars (iw3mp.exe 1.7, cod4-world.md 3.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// `r_spotLightShadows` and `sm_spotEnable`, both on.
    pub on: bool,
    /// `r_spotLightEntityShadows`, `r_spotLightSModelShadows`.
    pub entities: bool,
    pub smodels: bool,
    /// `sm_maxLights`, 0..=4.
    pub max_lights: usize,
    /// `sm_spotShadowFadeTime`, seconds.
    pub fade_time: f32,
    /// `sm_lightScore_eyeProjectDist`.
    pub eye_project_dist: f32,
    /// `sm_lightScore_spotProjectFrac`.
    pub spot_project_frac: f32,
    /// `sm_polygonOffsetBias`, `sm_polygonOffsetScale`.
    pub offset_bias: f32,
    pub offset_scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            on: true,
            entities: true,
            smodels: true,
            max_lights: MAX_LIGHTS,
            fade_time: 1.0,
            eye_project_dist: 64.0,
            spot_project_frac: 0.125,
            offset_bias: 0.5,
            offset_scale: 2.0,
        }
    }
}

/// A spot light's radius and outer cone cosine off its constants.
fn cone(l: &ModelLightSource) -> (f32, f32) {
    let [scale, bias, _] = l.spot_factors;
    // factors are 1 / (inner - outer) and -outer times that
    let cos_outer = if scale != 0.0 { -bias / scale } else { 0.5 };
    (1.0 / l.inv_radius, cos_outer.clamp(0.01, 0.999))
}

/// `R_ShadowedSpotLightScore` (cod4-world.md 3.5): how much a spot is
/// worth a map, its radius times its colour's luminance, falling off
/// with the distance from a point the eye looks at to a point down the
/// beam.
pub fn score(l: &ModelLightSource, eye: Vec3, fwd: Vec3, s: &Settings) -> f32 {
    const LUMA: Vec3 = Vec3::new(0.2989, 0.587, 0.114);
    let radius = 1.0 / l.inv_radius;
    let looked_at = eye + fwd * s.eye_project_dist;
    // `dir` points back at the light, so down the beam is against it
    let lit = Vec3::from(l.origin) - Vec3::from(l.dir) * radius * s.spot_project_frac;
    let brightness = Vec3::from(l.colour).dot(LUMA);
    radius * brightness / ((lit - looked_at).length() + 1.0)
}

/// The spots that may take a map this frame, best first, at most
/// `sm_maxLights` (cod4-world.md 3.5): every light past the sun's slot 0
/// that a visible surface stores (`used`), is a spot and may cast
/// (`canUseShadowMap`). Lights are taken from the highest index down,
/// and of two equal scores the one taken first stays ahead.
pub fn candidates(
    lights: &[Option<ModelLightSource>],
    used: &[bool],
    eye: Vec3,
    fwd: Vec3,
    s: &Settings,
) -> Vec<usize> {
    let count = lights.len().min(used.len());
    let eligible = (1..count).rev().filter_map(|n| {
        let l = lights[n].as_ref()?;
        (used[n] && l.spot && l.can_shadow).then(|| (n, score(l, eye, fwd, s)))
    });
    let mut ranked: Vec<(usize, f32)> = Vec::with_capacity(s.max_lights + 1);
    for (n, worth) in eligible {
        // past every entry not scored below it (a NaN never is)
        let below = |w: &f32| w.partial_cmp(&worth) == Some(std::cmp::Ordering::Less);
        let ahead = ranked.iter().take_while(|(_, w)| !below(w)).count();
        ranked.insert(ahead, (n, worth));
        ranked.truncate(s.max_lights);
    }
    ranked.into_iter().map(|(n, _)| n).collect()
}

/// One shadowed light: its index, its fade (`lightSpotFactors.w`) and
/// whether it is on its way out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub light: usize,
    pub fade: f32,
    fading_out: bool,
}

/// `GfxShadowedLightHistory`: the shadowed lights, each a tile
/// in order, and last frame's used lights.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub entries: Vec<Entry>,
    last: Option<f32>,
    was_used: Vec<bool>,
}

impl History {
    /// One frame at `time` seconds (`R_ChooseShadowedLights`,
    /// cod4-world.md 3.5). A frame at last frame's time changes nothing.
    /// Otherwise each entry fades by `dt / sm_spotShadowFadeTime`:
    /// - an entry whose light no visible surface stores now is dropped;
    /// - one still stored stops being renewed: on its first such frame it
    ///   only marks itself fading, after that it fades and goes under 0.01;
    /// - a candidate already held is renewed and fades back in, to 1;
    /// - a candidate not held joins while fewer than `sm_maxLights` are,
    ///   from the frame's step if its light was stored last frame, else
    ///   at full strength.
    pub fn update(&mut self, candidates: &[usize], used: &[bool], time: f32, s: &Settings) {
        let dt = time - self.last.unwrap_or(0.0);
        if dt == 0.0 {
            return;
        }
        self.last = Some(time);
        let step = dt / s.fade_time.max(0.01);
        let stored = |light: usize| used.get(light).copied().unwrap_or(false);
        drop_where(&mut self.entries, |e| {
            if !stored(e.light) {
                return true;
            }
            if !e.fading_out {
                e.fading_out = true;
                return false;
            }
            e.fade -= step;
            e.fade < 0.01
        });
        for &light in candidates {
            if let Some(held) = self.entries.iter_mut().find(|e| e.light == light) {
                held.fading_out = false;
                held.fade = (held.fade + step).min(1.0);
                continue;
            }
            if self.entries.len() < s.max_lights {
                let seen = self.was_used.get(light).copied().unwrap_or(false);
                self.entries.push(Entry {
                    light,
                    fade: if seen { step.min(1.0) } else { 1.0 },
                    fading_out: false,
                });
            }
        }
        self.was_used = used.to_vec();
    }
}

/// Removes the entries `gone` picks, each visited once; a removed
/// entry's slot takes the last entry, which is visited next, so the
/// survivors' order (and so their tiles) follows that.
fn drop_where(entries: &mut Vec<Entry>, mut gone: impl FnMut(&mut Entry) -> bool) {
    let mut k = 0;
    while let Some(e) = entries.get_mut(k) {
        if gone(e) {
            entries.swap_remove(k);
        } else {
            k += 1;
        }
    }
}

/// A shadowed spot's matrices: the light's view and projection, the
/// lookup into the map, the depth scale its casters store by, its origin.
#[derive(Clone, Copy, Debug)]
pub struct SpotView {
    pub view_proj: Mat4,
    pub lookup: Mat4,
    /// One over the far plane: the map holds clip z (not over w) times it.
    pub inv_far: f32,
    pub origin: Vec3,
    /// The light's frustum, to cull casters.
    pub planes: [Vec4; 6],
}

/// `R_SetViewParmsForLight` for a primary light (near plane 1,
/// far its radius, the outer cone's tangent both ways, looking down the
/// beam) and `R_GetSpotShadowLookupMatrix` for `tile` of four.
pub fn light_view(l: &ModelLightSource, tile: u32) -> SpotView {
    let (radius, cos_outer) = cone(l);
    let origin = Vec3::from(l.origin);
    let beam = -Vec3::from(l.dir).normalize_or(Vec3::NEG_Z);
    let up = if beam.z.abs() > 0.9 { Vec3::X } else { Vec3::Z };
    let tan = (1.0 - cos_outer * cos_outer).sqrt() / cos_outer;
    let far = radius.max(2.0);
    let proj = glam::camera::lh::proj::directx::perspective(2.0 * tan.atan(), 1.0, 1.0, far);
    let view_proj = proj * glam::camera::lh::view::look_to_mat4(origin, beam, up);
    // clip x onto 0..1, y onto the tile's quarter (top down)
    let y1 = tile as f32 / TILES as f32;
    let y0 = y1 + 1.0 / TILES as f32;
    let t = Mat4::from_cols(
        Vec4::new(0.5, 0.0, 0.0, 0.0),
        Vec4::new(0.0, (y1 - y0) * 0.5, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.5, (y1 + y0) * 0.5, 0.0, 1.0),
    );
    SpotView {
        view_proj,
        lookup: t * view_proj,
        inv_far: 1.0 / far,
        origin,
        planes: frustum(view_proj),
    }
}

/// The six planes (inside positive) of a 0..1 depth `view_proj`.
fn frustum(m: Mat4) -> [Vec4; 6] {
    let r = |i| m.row(i);
    let n = |p: Vec4| p / p.truncate().length().max(1e-12);
    [
        n(r(3) + r(0)),
        n(r(3) - r(0)),
        n(r(3) + r(1)),
        n(r(3) - r(1)),
        n(r(2)),
        n(r(3) - r(2)),
    ]
}

/// Whether the box `lo..hi` lies wholly outside one of `planes`.
pub fn culls(planes: &[Vec4; 6], lo: Vec3, hi: Vec3) -> bool {
    planes.iter().any(|p| {
        let far = Vec3::select(p.truncate().cmpge(Vec3::ZERO), hi, lo);
        p.truncate().dot(far) + p.w < 0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(origin: [f32; 3], beam: [f32; 3], radius: f32) -> ModelLightSource {
        // a 60 degree half angle, inner 0.9
        let (inner, outer) = (0.9f32, 0.5f32);
        let scale = 1.0 / (inner - outer);
        ModelLightSource {
            spot: true,
            can_shadow: true,
            origin,
            inv_radius: 1.0 / radius,
            colour: [1.0; 3],
            dir: beam.map(|c| -c),
            spot_factors: [scale, -scale * outer, 1.0],
            attenuation: Vec::new(),
        }
    }

    /// Fails with the whole new text when `actual` moved off the pin.
    fn pin(tag: &str, actual: &str, expected: &str) {
        assert!(actual == expected, "{tag} moved:\n{actual}");
    }

    /// Spots scattered round the eye, a few equal, one without a cone.
    fn scattered() -> Vec<Option<ModelLightSource>> {
        let mut v = vec![None];
        for k in 0..12 {
            let f = k as f32;
            let mut l = spot(
                [
                    100.0 * (f * 1.3).cos() * f,
                    80.0 * (f * 0.7).sin() * f,
                    120.0,
                ],
                [(f * 0.4).sin() * 0.3, (f * 0.9).cos() * 0.3, -1.0],
                150.0 + 40.0 * (k % 4) as f32,
            );
            l.colour = [1.0 - 0.05 * f, 0.6, 0.2 + 0.05 * f];
            if k == 5 {
                l.spot_factors = [0.0, 0.0, 1.0];
            }
            if k == 7 {
                l.spot = false;
            }
            v.push(Some(l));
        }
        // equal to light 2
        v.push(v[2].clone());
        v.push(None);
        v
    }

    #[test]
    fn spot_scores_and_candidates_pin() {
        let lights = scattered();
        let mut s = String::new();
        let views = [
            (Vec3::ZERO, Vec3::X),
            (Vec3::new(300.0, -200.0, 60.0), Vec3::new(0.0, 0.8, -0.6)),
        ];
        let settings = [
            Settings::default(),
            Settings {
                eye_project_dist: 512.0,
                spot_project_frac: 0.9,
                max_lights: 2,
                ..Settings::default()
            },
            Settings {
                max_lights: 0,
                ..Settings::default()
            },
        ];
        let used_all = vec![true; lights.len()];
        let used_some: Vec<bool> = (0..lights.len()).map(|i| i % 3 != 1).collect();
        for (eye, fwd) in views {
            for set in &settings {
                let scores: Vec<String> = lights
                    .iter()
                    .flatten()
                    .map(|l| format!("{:?}", score(l, eye, fwd, set)))
                    .collect();
                s += &format!("{}\n", scores.join(" "));
                for used in [&used_all, &used_some, &used_all[..6].to_vec()] {
                    s += &format!("  {:?}\n", candidates(&lights, used, eye, fwd, set));
                }
            }
        }
        pin(
            "candidates",
            &s,
            r##"0.8360122 1.0742666 0.51497597 0.5004835 0.49189857 0.25668082 0.33497 0.18107513 0.13639237 0.21349409 0.13517518 0.17130952 1.0742666
  [13, 2, 1, 3]
  [2, 3, 5, 6]
  [2, 1, 3, 4]
0.19635084 0.26506042 0.22787665 0.25327703 0.3247359 0.7355181 0.24254993 0.13126099 0.086840235 1.5778534 0.22600667 0.1386211 0.26506042
  [10, 6]
  [6, 5]
  [5, 2]
0.8360122 1.0742666 0.51497597 0.5004835 0.49189857 0.25668082 0.33497 0.18107513 0.13639237 0.21349409 0.13517518 0.17130952 1.0742666
  []
  []
  []
0.29073647 0.36251143 0.2685436 0.28149983 0.33499813 0.56869537 0.37497863 0.15820065 0.11315319 0.33871192 0.14840789 0.13745065 0.36251143
  [6, 7, 13, 2]
  [6, 2, 5, 3]
  [2, 5, 1, 4]
0.24279132 0.36569458 0.32652584 0.3691262 0.3400778 0.25155103 0.21836986 0.14045976 0.09151365 0.2887863 0.20928149 0.18856925 0.36569458
  [4, 13]
  [2, 5]
  [4, 2]
0.29073647 0.36251143 0.2685436 0.28149983 0.33499813 0.56869537 0.37497863 0.15820065 0.11315319 0.33871192 0.14840789 0.13745065 0.36251143
  []
  []
  []
"##,
        );
    }

    #[test]
    fn spot_history_pin() {
        let s3 = Settings {
            max_lights: 3,
            fade_time: 0.4,
            ..Settings::default()
        };
        let zero_fade = Settings {
            fade_time: 0.0,
            ..s3
        };
        let used = [false, true, true, true, true, true];
        let some = [false, true, false, true, true, false];
        // (candidates, used, time, settings)
        let frames: Vec<(Vec<usize>, &[bool], f32, &Settings)> = vec![
            (vec![1], &used, 0.0, &s3),
            (vec![1, 2], &used, 0.1, &s3),
            (vec![1, 2], &used, 0.1, &s3),
            (vec![3, 4, 5], &used, 0.2, &s3),
            (vec![3, 4, 5], &used, 0.3, &s3),
            (vec![5, 1], &some, 0.35, &s3),
            (vec![5, 4], &used, 0.5, &s3),
            (vec![4], &used, 0.7, &s3),
            (vec![4], &used, 0.75, &zero_fade),
            (vec![2, 3], &used, 1.5, &s3),
            (vec![], &used, 1.6, &s3),
            (vec![], &used, 1.9, &s3),
            (vec![9], &used, 2.0, &s3),
            (vec![1], &[], 2.1, &s3),
        ];
        let mut h = History::default();
        let mut s = String::new();
        for (c, u, t, set) in frames {
            h.update(&c, u, t, set);
            s += &format!("{t} {:?}\n", h.entries);
        }
        pin(
            "history",
            &s,
            r##"0 []
0.1 [Entry { light: 1, fade: 1.0, fading_out: false }, Entry { light: 2, fade: 1.0, fading_out: false }]
0.1 [Entry { light: 1, fade: 1.0, fading_out: false }, Entry { light: 2, fade: 1.0, fading_out: false }]
0.2 [Entry { light: 1, fade: 1.0, fading_out: true }, Entry { light: 2, fade: 1.0, fading_out: true }, Entry { light: 3, fade: 0.25, fading_out: false }]
0.3 [Entry { light: 1, fade: 0.75, fading_out: true }, Entry { light: 2, fade: 0.75, fading_out: true }, Entry { light: 3, fade: 0.5, fading_out: false }]
0.35 [Entry { light: 1, fade: 0.75, fading_out: false }, Entry { light: 3, fade: 0.5, fading_out: true }, Entry { light: 5, fade: 0.124999955, fading_out: false }]
0.5 [Entry { light: 1, fade: 0.75, fading_out: true }, Entry { light: 3, fade: 0.125, fading_out: true }, Entry { light: 5, fade: 0.49999994, fading_out: false }]
0.7 [Entry { light: 1, fade: 0.25000003, fading_out: true }, Entry { light: 5, fade: 0.49999994, fading_out: true }, Entry { light: 4, fade: 0.49999997, fading_out: false }]
0.75 [Entry { light: 4, fade: 1.0, fading_out: false }]
1.5 [Entry { light: 4, fade: 1.0, fading_out: true }, Entry { light: 2, fade: 1.0, fading_out: false }, Entry { light: 3, fade: 1.0, fading_out: false }]
1.6 [Entry { light: 4, fade: 0.74999994, fading_out: true }, Entry { light: 2, fade: 1.0, fading_out: true }, Entry { light: 3, fade: 1.0, fading_out: true }]
1.9 [Entry { light: 3, fade: 0.25000012, fading_out: true }, Entry { light: 2, fade: 0.25000012, fading_out: true }]
2 [Entry { light: 9, fade: 1.0, fading_out: false }]
2.1 [Entry { light: 1, fade: 0.24999976, fading_out: false }]
"##,
        );
    }

    #[test]
    fn the_lookup_puts_the_beam_in_tile_zero_and_depth_by_the_far_plane() {
        let l = spot([0.0, 0.0, 200.0], [0.0, 0.0, -1.0], 240.0);
        let v = light_view(&l, 0);
        // straight down the beam, 100 units: the tile's middle
        let s = v.lookup * Vec4::new(0.0, 0.0, 100.0, 1.0);
        let uv = s.truncate().truncate() / s.w;
        assert!(
            (uv.x - 0.5).abs() < 1e-4 && (uv.y - 0.125).abs() < 1e-4,
            "{uv}"
        );
        // its clip z over the far plane: the map's stored scale
        let z = s.z * v.inv_far;
        assert!(z > 0.0 && z < 1.0, "{z}");
        // a point past the cone's edge falls outside the tile
        let s = v.lookup * Vec4::new(150.0, 0.0, 120.0, 1.0);
        let (u, t) = (s.x / s.w, s.y / s.w);
        assert!(
            !(0.0..=1.0).contains(&u) || !(0.0..=0.25).contains(&t),
            "{u} {t}"
        );
        // the light's frustum keeps the lit floor and drops what is behind
        assert!(!culls(
            &v.planes,
            Vec3::new(-10.0, -10.0, 90.0),
            Vec3::new(10.0, 10.0, 110.0)
        ));
        assert!(culls(
            &v.planes,
            Vec3::new(-10.0, -10.0, 210.0),
            Vec3::new(10.0, 10.0, 230.0)
        ));
    }

    #[test]
    fn a_later_tile_takes_its_quarter() {
        let l = spot([0.0, 0.0, 200.0], [0.0, 0.0, -1.0], 240.0);
        let s = light_view(&l, 2).lookup * Vec4::new(0.0, 0.0, 100.0, 1.0);
        assert!((s.y / s.w - 0.625).abs() < 1e-4, "{}", s.y / s.w);
    }

    #[test]
    fn the_used_spots_ahead_score_first() {
        let s = Settings::default();
        let near = spot([200.0, 0.0, 100.0], [0.0, 0.0, -1.0], 240.0);
        let far = spot([2000.0, 0.0, 100.0], [0.0, 0.0, -1.0], 240.0);
        let (eye, fwd) = (Vec3::ZERO, Vec3::X);
        assert!(score(&near, eye, fwd, &s) > score(&far, eye, fwd, &s));
        let mut unshadowed = near.clone();
        unshadowed.can_shadow = false;
        let lights = vec![
            None,
            Some(far.clone()),
            Some(near.clone()),
            Some(unshadowed),
        ];
        let all = [true; 4];
        assert_eq!(candidates(&lights, &all, eye, fwd, &s), vec![2, 1]);
        // a light no visible surface stores is no candidate
        assert_eq!(
            candidates(&lights, &[true, true, false, true], eye, fwd, &s),
            vec![1]
        );
        let one = Settings { max_lights: 1, ..s };
        assert_eq!(candidates(&lights, &all, eye, fwd, &one), vec![2]);
    }

    #[test]
    fn a_dropped_candidate_fades_out_and_a_new_one_in() {
        let s = Settings {
            max_lights: 2,
            fade_time: 1.0,
            ..Settings::default()
        };
        let used = [false, true, true, true];
        let mut h = History::default();
        // first seen: in at once (not used last frame)
        h.update(&[1], &used, 1.0, &s);
        assert_eq!(
            h.entries,
            vec![Entry {
                light: 1,
                fade: 1.0,
                fading_out: false
            }]
        );
        // light 2 replaces it as the candidate: 2 fades in from the frame's
        // step, 1 keeps its fade a frame and then loses it
        h.update(&[2], &used, 1.25, &s);
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[1].light, 2);
        assert!((h.entries[1].fade - 0.25).abs() < 1e-6);
        assert_eq!(h.entries[0].fade, 1.0);
        h.update(&[2], &used, 1.5, &s);
        assert!((h.entries[0].fade - 0.75).abs() < 1e-6, "{:?}", h.entries);
        assert!((h.entries[1].fade - 0.5).abs() < 1e-6);
        for t in [2.0, 2.5, 2.9] {
            h.update(&[2], &used, t, &s);
        }
        assert_eq!(
            h.entries.iter().map(|e| e.light).collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(h.entries[0].fade, 1.0);
        // its light no longer stored by a visible surface: gone at once
        h.update(&[], &[false, true, false, true], 3.0, &s);
        assert!(h.entries.is_empty());
    }
}
