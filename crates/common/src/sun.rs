//! The map's sky and sun as the renderer picks them at load, and whether the
//! sky (and so the sun) draws in a frame
//! (docs/research/cod11-shader-scripts.md section 8, "The sun").

use crate::asset_source::AssetSource;
use glam::Vec3;

use crate::bsp;
use crate::mapworld::MapWorld;
use crate::material::{Material, MaterialLib};
use crate::mesh::{self, MaterialKind};

/// `scripts/<name>.sun`, the cvar dump a sky block's `sunfile <name>` token
/// names (loaded by CoDMP.exe via `"scripts/%s.sun"`, 0x4e7f30). Defaults
/// are the cvars' registration strings (the `r_sun*` block of 0x4b3300 from
/// 0x4b41b1 on), which a key the file leaves out keeps.
#[derive(Debug, Clone, PartialEq)]
pub struct SunFile {
    /// `r_sunsprite_shader`; `None` draws no disc ("0" is how mp_harbor.sun
    /// disables it).
    pub sprite: Option<String>,
    /// `r_sunsprite_size`: the disc's half-width is this times 0.0013110929
    /// at unit distance (0x4e7980), and in 640-wide pixels the radius of
    /// the flare's visibility window (0x4d9250).
    pub sprite_size: f32,
    /// `r_sunflare_shader`; "0" or empty draws no flare.
    pub flare: Option<String>,
    pub flare_min_size: f32,
    pub flare_max_size: f32,
    pub flare_min_angle: f32,
    pub flare_max_angle: f32,
    pub flare_max_alpha: f32,
    /// Seconds.
    pub flare_fade_in: f32,
    pub flare_fade_out: f32,
    /// `r_sunblind_*`: darkens the screen, `max` is `max_darken`.
    pub blind: SunFade,
    /// `r_sunglare_*`: lightens the screen, `max` is `max_lighten`.
    pub glare: SunFade,
}

/// The blind and glare screen washes: full strength within `max_angle`
/// degrees of the sun, none past `min_angle`, eased in and out over the
/// fade times in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunFade {
    pub min_angle: f32,
    pub max_angle: f32,
    pub max: f32,
    pub fade_in: f32,
    pub fade_out: f32,
}

impl Default for SunFile {
    fn default() -> Self {
        let fade = SunFade {
            min_angle: 30.0,
            max_angle: 5.0,
            max: 0.75,
            fade_in: 0.5,
            fade_out: 3.0,
        };
        SunFile {
            sprite: None,
            sprite_size: Self::DEFAULT_SPRITE_SIZE,
            flare: Some("sunFlareShader".into()),
            flare_min_size: 0.0,
            flare_max_size: 2500.0,
            flare_min_angle: 45.0,
            flare_max_angle: 2.0,
            flare_max_alpha: 1.0,
            flare_fade_in: 1.0,
            flare_fade_out: 1.0,
            blind: fade,
            glare: fade,
        }
    }
}

impl SunFile {
    /// Engine default for `r_sunsprite_size`; every stock `.sun` that wants a
    /// disc sets both fields explicitly, so this only covers partial files.
    pub const DEFAULT_SPRITE_SIZE: f32 = 16.0;

    pub fn parse(text: &str) -> SunFile {
        let mut sf = SunFile::default();
        let shader = |v: &str| (!v.is_empty() && v != "0").then(|| v.to_string());
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            let n = value.parse::<f32>().ok();
            let key = key.trim().to_ascii_lowercase();
            let num = match key.as_str() {
                "r_sunsprite_shader" => {
                    sf.sprite = shader(value);
                    continue;
                }
                "r_sunflare_shader" => {
                    sf.flare = shader(value);
                    continue;
                }
                "r_sunsprite_size" => &mut sf.sprite_size,
                "r_sunflare_min_size" => &mut sf.flare_min_size,
                "r_sunflare_max_size" => &mut sf.flare_max_size,
                "r_sunflare_min_angle" => &mut sf.flare_min_angle,
                "r_sunflare_max_angle" => &mut sf.flare_max_angle,
                "r_sunflare_max_alpha" => &mut sf.flare_max_alpha,
                "r_sunflare_fadein" => &mut sf.flare_fade_in,
                "r_sunflare_fadeout" => &mut sf.flare_fade_out,
                "r_sunblind_min_angle" => &mut sf.blind.min_angle,
                "r_sunblind_max_angle" => &mut sf.blind.max_angle,
                "r_sunblind_max_darken" => &mut sf.blind.max,
                "r_sunblind_fadein" => &mut sf.blind.fade_in,
                "r_sunblind_fadeout" => &mut sf.blind.fade_out,
                "r_sunglare_min_angle" => &mut sf.glare.min_angle,
                "r_sunglare_max_angle" => &mut sf.glare.max_angle,
                "r_sunglare_max_lighten" => &mut sf.glare.max,
                "r_sunglare_fadein" => &mut sf.glare.fade_in,
                "r_sunglare_fadeout" => &mut sf.glare.fade_out,
                _ => continue,
            };
            if let Some(n) = n {
                *num = n;
            }
        }
        sf
    }
}

/// Each material's draw decision over the lib's parsed surface bits.
pub fn material_kinds(lib: &MaterialLib, bsp: &MapWorld) -> Vec<MaterialKind> {
    bsp.materials
        .iter()
        .map(|m| match lib.get(&m.name) {
            Some(sh) => mesh::material_kind(
                &m.name,
                true,
                sh.surface.nodraw,
                sh.surface.sky || sh.sky.is_some(),
            ),
            None => mesh::material_kind(&m.name, false, false, false),
        })
        .collect()
}

/// Picks the active sky block: highest soup reference count wins, ties go to
/// the alphabetically first name, an empty list means no sky pass.
pub fn pick_sky(candidates: &[(String, u32)]) -> Option<String> {
    candidates
        .iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(name, _)| name.clone())
}

/// The map's active sky: the authored `skyparms` blocks its materials name,
/// ranked by soup reference count. The name and its material index.
pub fn active_sky(
    lib: &MaterialLib,
    bsp: &MapWorld,
    kinds: &[MaterialKind],
) -> Option<(String, u16)> {
    let norm = |s: &str| s.to_lowercase().replace('\\', "/");
    let mut material_idx = std::collections::HashMap::new();
    for (i, m) in bsp.materials.iter().enumerate() {
        material_idx.entry(norm(&m.name)).or_insert(i as u16);
    }
    let mut soup_refs = std::collections::HashMap::<u16, u32>::new();
    for s in &bsp.soups {
        if kinds[s.material as usize] == MaterialKind::Sky {
            *soup_refs.entry(s.material).or_default() += 1;
        }
    }
    let candidates: Vec<(String, u16)> = lib
        .sky_blocks()
        .filter_map(|sh| material_idx.get(&sh.name).map(|&mi| (sh.name.clone(), mi)))
        .collect();
    let with_refs: Vec<(String, u32)> = candidates
        .iter()
        .map(|(n, mi)| (n.clone(), soup_refs.get(mi).copied().unwrap_or(0)))
        .collect();
    let picked = pick_sky(&with_refs)?;
    candidates.into_iter().find(|(n, _)| *n == picked)
}

/// The direction of the map's lump-30 sun light (`world+0x114`), the last
/// kind-1 light. Retail draws no sun without one (0x4e83f0).
pub fn sun_light(bsp: &MapWorld) -> Option<Vec3> {
    bsp.lights
        .iter()
        .rev()
        .find(|w| w[0] == 1)
        .map(|w| Vec3::from_array([7, 8, 9].map(|i| f32::from_bits(w[i]))))
}

/// The sun a map draws: its sky block's `.sun` file, the lump-30 light the
/// flare sits on and the worldspawn `sundirection` the disc and the flare's
/// angle test follow.
#[derive(Debug, Clone)]
pub struct MapSun {
    pub file: SunFile,
    pub light_dir: Vec3,
    pub dir: Vec3,
}

/// `None` where retail draws no sun: no `sunfile` on the sky block, a
/// `.sun` file that does not load (the world load zeroes the sun state and
/// only a loaded file refills it), or no lump-30 sun light.
pub fn load(fs: &dyn AssetSource, sky: &Material, bsp: &MapWorld) -> Option<MapSun> {
    let name = sky.sunfile.as_ref()?;
    let Some(bytes) = fs.read(&format!("scripts/{name}.sun")) else {
        log::debug!("sunfile: scripts/{name}.sun not found");
        return None;
    };
    let light_dir = sun_light(bsp)?;
    Some(MapSun {
        file: SunFile::parse(&String::from_utf8_lossy(&bytes)),
        light_dir,
        dir: Vec3::from(bsp::sun_direction(&bsp.entities)),
    })
}

/// Whether retail's sky stage iterator (0x4e84d0, where the sun's disc and
/// flare are added) runs this frame: a sky surface came through the cell
/// walk, or, under a far plane (`r_zfar` or linear fog, 0x4e1670), a sky
/// surface's box reaches nearer than `far` along the view (0x4e4b70 adds
/// those whatever the walk saw).
pub fn sky_drawn(
    walked: bool,
    far: Option<f32>,
    eye: Vec3,
    fwd: Vec3,
    sky_bounds: &[(Vec3, Vec3)],
) -> bool {
    walked
        || far.is_some_and(|far| {
            sky_bounds.iter().any(|&(lo, hi)| {
                // the corner nearest along the view
                let near = Vec3::select(fwd.cmpgt(Vec3::ZERO), lo, hi);
                fwd.dot(near - eye) <= far
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_sky_prefers_refs_then_alphabetical() {
        let cands = |pairs: &[(&str, u32)]| {
            pick_sky(
                &pairs
                    .iter()
                    .map(|(n, r)| ((*n).to_string(), *r))
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(cands(&[]), None, "no candidates, no sky pass");
        assert_eq!(
            cands(&[("textures/sky/b", 3), ("textures/sky/a", 7)]).as_deref(),
            Some("textures/sky/a")
        );
        assert_eq!(
            cands(&[("textures/sky/b", 3), ("textures/sky/a", 3)]).as_deref(),
            Some("textures/sky/a"),
            "tie breaks alphabetically"
        );
        // zero-reference candidates stay eligible (presence ranks, not wins)
        assert_eq!(
            cands(&[("textures/sky/b", 0), ("textures/sky/a", 0)]).as_deref(),
            Some("textures/sky/a")
        );
    }

    #[test]
    fn sky_draws_through_the_walk_or_inside_the_far_plane() {
        let eye = Vec3::ZERO;
        let fwd = Vec3::X;
        let ahead = [(Vec3::new(3000.0, -10.0, 0.0), Vec3::new(3100.0, 10.0, 10.0))];
        let behind = [(
            Vec3::new(-3100.0, -10.0, 0.0),
            Vec3::new(-3000.0, 10.0, 10.0),
        )];
        assert!(sky_drawn(true, None, eye, fwd, &[]));
        assert!(
            !sky_drawn(false, None, eye, fwd, &ahead),
            "no far plane: the walk only"
        );
        assert!(
            !sky_drawn(false, Some(2000.0), eye, fwd, &ahead),
            "all past the far plane"
        );
        assert!(
            sky_drawn(false, Some(3050.0), eye, fwd, &ahead),
            "its near side inside"
        );
        assert!(
            sky_drawn(false, Some(10.0), eye, fwd, &behind),
            "behind the eye counts"
        );
        assert!(
            !sky_drawn(false, Some(10.0), eye, fwd, &[]),
            "a map without sky surfaces"
        );
    }
}
