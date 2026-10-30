//! A CoD4 level's fog and vision sets on the renderer (`vcod_cod4::look`,
//! docs/research/cod4-world.md 11): a session follows configstrings 9,
//! 824 and 825 on the cgame's clock, the night set drawn while the view
//! looks through night vision; fly mode takes the map's art script's
//! `setExpFog` and the vision set the game type names after the prematch.

use std::collections::HashMap;
use vcod_cod4::look::{
    self, CS_FOG, CS_VISION_NAKED, CS_VISION_NIGHT, ExpFog, FogCs, Vision, VisionBlend,
};
use vcod_common::net::FogParams;

use crate::renderer::LevelLook;

/// The vision sets a level can name, and the naked one as it blends.
#[derive(Clone, Debug, Default)]
pub struct Cod4Look {
    visions: HashMap<String, Vision>,
    blend: VisionBlend,
    /// The night channel, `None` until configstring 825 names a set.
    night: Option<VisionBlend>,
    /// The configstrings last applied.
    fog_cs: Option<String>,
    vision_cs: Option<String>,
    night_cs: Option<String>,
    /// Fly mode's fog, from the map's script.
    pub fly_fog: Option<ExpFog>,
}

impl Cod4Look {
    /// `visions` from `common_mp` and the map's zone, by name. The client
    /// starts on `default` (the default vision) until a configstring
    /// names another.
    pub fn new(visions: HashMap<String, Vision>) -> Cod4Look {
        let mut l = Cod4Look {
            visions,
            ..Cod4Look::default()
        };
        l.blend.set(l.vision(look::DEFAULT_VISION), 0, 0);
        l
    }

    /// Fly mode on `map`: the script's fog, and the map's own vision set
    /// as the game type sets it once the prematch ends.
    pub fn fly(visions: HashMap<String, Vision>, map: &str, fog: Option<ExpFog>) -> Cod4Look {
        let mut l = Cod4Look::new(visions);
        l.blend.set(l.vision(map), 0, 0);
        l.fly_fog = fog;
        l
    }

    /// A set by name; a name with no file draws `default`, as
    /// The raw file read falls back.
    fn vision(&self, name: &str) -> Vision {
        let name = name.to_ascii_lowercase();
        self.visions
            .get(&name)
            .or_else(|| self.visions.get(look::DEFAULT_VISION))
            .copied()
            .unwrap_or_default()
    }

    /// A session's configstrings at cgame time `now_ms`: the fog to
    /// fade to when configstring 9 changed (`Some(None)` clears it), and
    /// a new naked vision set's blend started.
    pub fn configstrings(
        &mut self,
        cs: impl Fn(u16) -> String,
        now_ms: i32,
    ) -> Option<Option<FogParams>> {
        let vision = cs(CS_VISION_NAKED);
        if self.vision_cs.as_deref() != Some(vision.as_str()) {
            let (name, ms) = look::parse_vision_cs(&vision);
            // an empty name keeps what draws (the vision set lookup refuses it)
            if !name.is_empty() {
                let v = self.vision(&name);
                self.blend.set(v, ms, now_ms);
            }
            self.vision_cs = Some(vision);
        }
        let night = cs(CS_VISION_NIGHT);
        if self.night_cs.as_deref() != Some(night.as_str()) {
            let (name, ms) = look::parse_vision_cs(&night);
            if !name.is_empty() {
                let v = self.vision(&name);
                self.night.get_or_insert_default().set(v, ms, now_ms);
            }
            self.night_cs = Some(night);
        }
        let fog = cs(CS_FOG);
        if self.fog_cs.as_deref() == Some(fog.as_str()) {
            return None;
        }
        self.fog_cs = Some(fog.clone());
        Some(match look::parse_fog(&fog) {
            FogCs::On(f) => Some(f.params()),
            // the fog fades to none: density 0 keeps every colour
            FogCs::Off { transition_ms } if transition_ms > 0 => Some(FogParams {
                near: 0.0,
                far: 1.0,
                density: 0.0,
                color: [0.0; 3],
                fade_ms: transition_ms as u32,
                start: 0.0,
            }),
            FogCs::Off { .. } => None,
        })
    }

    /// What the renderer draws at `now_ms`: the film tweak's constants
    /// and the glow (cod4-world.md 11.3, 11.4) of the naked set, or with
    /// `night` (the view looks through night vision, [`night_vision`]) of
    /// the night set, neither while no night set was named
    /// (`CG_VisionSetApplyToRefdef`).
    pub fn at(&self, now_ms: i32, night: bool) -> LevelLook {
        let v = if night {
            match &self.night {
                Some(b) => b.at(now_ms),
                None => return LevelLook::default(),
            }
        } else {
            self.blend.at(now_ms)
        };
        let g = v.glow;
        LevelLook {
            film: v.film.consts().map(|c| crate::post::Film {
                bias: c.bias,
                base: c.base,
                delta: c.delta,
            }),
            glow: g.enable.then_some(crate::cod2_glow::Cod4Glow {
                cutoff: g.bloom_cutoff,
                desaturation: g.bloom_desaturation,
                intensity: g.bloom_intensity[0],
                radius: g.radius[0],
            }),
        }
    }
}

/// `CG_LookingThroughNightVision` on the drawn playerstate: its
/// `weapFlags` night vision bit (0x40). The goggles' wear and remove
/// states (25, 26), on past their weapon's power-up time, are not read.
pub fn night_vision(weap_flags: u32) -> bool {
    weap_flags & 0x40 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_cod4::look::Film;

    fn sets() -> HashMap<String, Vision> {
        let film = |contrast| Vision {
            film: Film {
                enable: true,
                contrast,
                light_tint: [1.0; 3],
                dark_tint: [1.0; 3],
                ..Film::default()
            },
            ..Vision::default()
        };
        HashMap::from([
            ("default".to_string(), Vision::default()),
            ("mpintro".to_string(), film(1.8)),
            ("mp_crash".to_string(), film(1.4)),
        ])
    }

    /// The prematch capture's order: `"mpintro" 0` at the join, then
    /// `"mp_crash" 2000` at the prematch's end, the fog once.
    #[test]
    fn the_prematch_greys_then_eases_to_the_map() {
        let mut l = Cod4Look::new(sets());
        assert_eq!(l.at(0, false).film, None);
        let mut cs = HashMap::from([
            (
                CS_FOG,
                "500 0.000198042 0.501961 0.501961 0.45098 0".to_string(),
            ),
            (CS_VISION_NAKED, "\"mpintro\" 0".to_string()),
        ]);
        let get = |cs: &HashMap<u16, String>| {
            let cs = cs.clone();
            move |k: u16| cs.get(&k).cloned().unwrap_or_default()
        };
        let fog = l.configstrings(get(&cs), 6600).unwrap().unwrap();
        assert_eq!(
            (fog.start, fog.density, fog.fade_ms),
            (500.0, 0.000198042, 0)
        );
        assert_eq!(fog.color, [128.0 / 255.0, 128.0 / 255.0, 115.0 / 255.0]);
        let grey = l.at(6600, false).film.unwrap();
        // contrast 1.8 at desaturation 0: bias 0.5 - 0.9
        assert!((grey.bias[0] + 0.4).abs() < 1e-6);
        cs.insert(CS_VISION_NAKED, "\"mp_crash\" 2000".to_string());
        assert_eq!(l.configstrings(get(&cs), 19850), None);
        let mid = l.at(20850, false).film.unwrap().bias[0];
        let end = l.at(21850, false).film.unwrap().bias[0];
        assert!(mid < -0.2 && mid > -0.4, "{mid}");
        assert!((end + 0.2).abs() < 1e-6, "{end}");
        // an unknown name draws `default`: no film
        cs.insert(CS_VISION_NAKED, "\"nothing\" 0".to_string());
        l.configstrings(get(&cs), 30000);
        assert_eq!(l.at(30000, false).film, None);
        // fog off with a fade fades to density 0
        cs.insert(CS_FOG, "1000".to_string());
        let off = l.configstrings(get(&cs), 30000).unwrap().unwrap();
        assert_eq!((off.density, off.fade_ms), (0.0, 1000));
    }

    /// A set that turns the glow on hands its level-0 intensity and
    /// radius to the renderer; the night set draws only with night vision
    /// on and after configstring 825 named one, as retail's
    /// `"default_night" 1000` does.
    #[test]
    fn the_glow_and_the_night_set() {
        let mut sets = sets();
        let bog = Vision {
            glow: vcod_cod4::look::Glow {
                enable: true,
                bloom_cutoff: 0.925522,
                bloom_desaturation: 0.75,
                bloom_intensity: [1.42171, 1.4883],
                radius: [25.2691, 25.7724],
                sky_bleed_intensity: [0.0; 2],
            },
            ..Vision::default()
        };
        sets.insert("mp_bog".into(), bog);
        let mut night = sets["mpintro"];
        night.film.contrast = 0.5;
        sets.insert("default_night".into(), night);
        let mut l = Cod4Look::new(sets);
        let mut cs = HashMap::from([(CS_VISION_NAKED, "\"mp_bog\" 0".to_string())]);
        let get = |cs: &HashMap<u16, String>| {
            let cs = cs.clone();
            move |k: u16| cs.get(&k).cloned().unwrap_or_default()
        };
        l.configstrings(get(&cs), 0);
        let g = l.at(0, false).glow.unwrap();
        assert_eq!((g.intensity, g.radius), (1.42171, 25.2691));
        assert_eq!((g.cutoff, g.desaturation), (0.925522, 0.75));
        // night vision before 825 names a set: neither film nor glow
        assert_eq!(l.at(0, true), LevelLook::default());
        cs.insert(CS_VISION_NIGHT, "\"default_night\" 1000".to_string());
        l.configstrings(get(&cs), 100);
        let n = l.at(100, true);
        assert!(n.glow.is_none());
        // the first set named on the channel snaps
        assert!((n.film.unwrap().bias[0] - 0.25).abs() < 1e-6);
        assert!(l.at(100, false).glow.is_some());
        assert!(night_vision(0x40) && !night_vision(0x4));
    }

    /// The retail prematch capture (cod4-hud.md 13) through the session:
    /// the fog of configstring 9 from the gamestate, `mpintro` at the join
    /// and the 2 s ease to `mp_crash` when the prematch ends.
    #[test]
    fn the_prematch_capture_names_the_fog_and_both_sets() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/net/play-1.7-prematch.txt"
        );
        let text = std::fs::read_to_string(path).unwrap();
        let mut s = crate::play::cod4::Cod4Session::open_capture(&text).unwrap();
        let mut l = Cod4Look::new(sets());
        let mut hud = crate::cod4_hud::Cod4Hud::default();
        let (mut fogs, mut films) = (Vec::new(), Vec::new());
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            hud.update(&mut s, &fired, scene.server_time);
            let now = scene.render_time;
            let cs = |n: u16| hud.configstring(&s, n);
            if let Some(f) = l.configstrings(cs, now) {
                fogs.push((now, f));
            }
            let contrast = l.blend.at(now).film.contrast;
            if films.last().is_none_or(|&(_, c)| c != contrast) {
                films.push((now, contrast));
            }
        }
        assert_eq!(fogs.len(), 1, "{fogs:?}");
        let f = fogs[0].1.unwrap();
        assert_eq!((f.start, f.density, f.fade_ms), (500.0, 0.000198042, 0));
        assert_eq!(films[0].1, 1.8, "mpintro first");
        let last = films.last().unwrap();
        assert_eq!(last.1, 1.4, "mp_crash last");
        // `d 824` rides the snapshot of level time 19850, which the
        // drawn time, a snapshot behind, reaches first
        let start = films[1].0;
        assert!((19_700..=19_900).contains(&start), "{start}");
        assert!(last.0 - start <= 2000 && last.0 - start > 1900, "{films:?}");
    }
}
