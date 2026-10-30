//! CoD4 1.7's shellshock records: `shock/<name>.shock`, a rawfile of
//! `bg_shock_*` dvar lines, read into the `shellshock_parms_t` the server's
//! pmove and the client's view, screen and sound read
//! (docs/research/cod4-combat.md 10.8). `BG_LoadShellShockDvars` resets every
//! dvar to its default before the file sets its own lines, so a line the
//! file leaves out reads the registered default.

use std::collections::HashMap;

/// `bg_ShockScreenTypeNames`: the screen effect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenType {
    /// The `shellshock` material: the last frame over this one.
    #[default]
    Blurred,
    /// The `shellshock_flashed` material: a still of the first frame and
    /// white, added.
    Flashed,
    None,
}

/// One shellshock's `shellshock_parms_t`, the times in ms
/// (round to nearest of `seconds * 1000`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShockParms {
    pub screen: ScreenType,
    pub blur_effect_ms: i32,
    pub blur_fade_ms: i32,
    pub flash_white_fade_ms: i32,
    pub flash_shot_fade_ms: i32,
    /// The camera's kick: `0.001 / max(period, 0.001)` cycles a ms, its
    /// radius, fading over the last 3000 ms.
    pub kick_rate: f32,
    pub kick_radius: f32,
    pub sound: bool,
    pub sound_loop: String,
    pub sound_loop_silent: String,
    pub sound_end: String,
    pub sound_end_abort: String,
    pub fade_in_ms: i32,
    pub fade_out_ms: i32,
    pub loop_fade_ms: i32,
    pub loop_end_delay_ms: i32,
    pub room: String,
    pub dry: f32,
    pub wet: f32,
    pub mod_end_delay_ms: i32,
    /// `bg_shock_volume_<channel>` by `channels.def` name, clamped to 0..1.
    pub volumes: HashMap<String, f32>,
    pub look: bool,
    pub look_fade_ms: i32,
    pub max_pitch_speed: f32,
    pub max_yaw_speed: f32,
    pub sensitivity: f32,
    /// `bg_shock_movement`: `PM_CmdScaleForWeapon` takes 0.4 of the speed.
    pub movement: bool,
}

/// round to nearest of `seconds * 1000`.
fn ms(seconds: f32) -> i32 {
    (seconds * 1000.0).round_ties_even() as i32
}

/// A channel volume's default, `Dvar_RegisterFloat(.., 0.5, ..)`.
pub const DEFAULT_VOLUME: f32 = 0.5;

impl Default for ShockParms {
    /// Every dvar at its registered default: the floats and bools as
    /// `cod4_lnxded` registers them (VERIFIED, `tools/re/cod4_dvars.py`),
    /// the screen type, strings and room INFERRED.
    fn default() -> Self {
        ShockParms::from_text("")
    }
}

impl ShockParms {
    /// `BG_LoadShellShockDvars` then `BG_SetShellShockParmsFromDvars` over a
    /// `.shock` file's text: one `name "value"` a line, `//` comments.
    pub fn from_text(text: &str) -> ShockParms {
        let mut v: HashMap<String, String> = HashMap::new();
        for line in text.lines() {
            let line = line.split("//").next().unwrap_or("").trim();
            let mut it = line.splitn(2, char::is_whitespace);
            let (Some(k), Some(val)) = (it.next(), it.next()) else {
                continue;
            };
            v.insert(
                k.to_ascii_lowercase(),
                val.trim().trim_matches('"').to_string(),
            );
        }
        let s = |k: &str, d: &str| v.get(k).cloned().unwrap_or_else(|| d.to_string());
        let f = |k: &str, d: f32, lo: f32, hi: f32| {
            v.get(k)
                .and_then(|x| x.parse::<f32>().ok())
                .unwrap_or(d)
                .clamp(lo, hi)
        };
        let b = |k: &str, d: bool| {
            v.get(k)
                .map_or(d, |x| x.parse::<f32>().is_ok_and(|n| n != 0.0))
        };
        let screen = match s("bg_shock_screentype", "blurred")
            .to_ascii_lowercase()
            .as_str()
        {
            "flashed" | "1" => ScreenType::Flashed,
            "none" | "2" => ScreenType::None,
            _ => ScreenType::Blurred,
        };
        let period = f("bg_shock_viewkickperiod", 0.75, 0.001, 1000.0);
        let volumes = v
            .iter()
            .filter_map(|(k, x)| {
                let ch = k.strip_prefix("bg_shock_volume_")?;
                Some((ch.to_string(), x.parse::<f32>().ok()?.clamp(0.0, 1.0)))
            })
            .collect();
        ShockParms {
            screen,
            blur_effect_ms: ms(f("bg_shock_screenblurblendtime", 0.4, 0.001, 10.0)),
            blur_fade_ms: ms(f("bg_shock_screenblurblendfadetime", 1.0, 0.001, 1000.0)),
            flash_white_fade_ms: ms(f("bg_shock_screenflashwhitefadetime", 1.0, 0.0, 1000.0)),
            flash_shot_fade_ms: ms(f("bg_shock_screenflashshotfadetime", 1.0, 0.0, 1000.0)),
            kick_rate: 0.001 / period.max(0.001),
            kick_radius: f("bg_shock_viewkickradius", 0.05, 0.0, 1.0),
            sound: b("bg_shock_sound", true),
            sound_loop: s("bg_shock_soundloop", "shellshock_loop"),
            sound_loop_silent: s("bg_shock_soundloopsilent", "shellshock_loop_silent"),
            sound_end: s("bg_shock_soundend", "shellshock_end"),
            sound_end_abort: s("bg_shock_soundendabort", "shellshock_end_abort"),
            fade_in_ms: ms(f("bg_shock_soundfadeintime", 0.25, 0.001, 1000.0)),
            fade_out_ms: ms(f("bg_shock_soundfadeouttime", 2.5, 0.001, 1000.0)),
            loop_fade_ms: ms(f("bg_shock_soundloopfadetime", 1.5, 0.001, 1000.0)),
            loop_end_delay_ms: ms(f("bg_shock_soundloopenddelay", -3.0, -10.0, 1000.0)),
            room: s("bg_shock_soundroomtype", "generic"),
            dry: f("bg_shock_sounddrylevel", 1.0, 0.0, 1.0),
            wet: f("bg_shock_soundwetlevel", 0.5, 0.0, 1.0),
            mod_end_delay_ms: ms(f("bg_shock_soundmodenddelay", 2.0, -1000.0, 1000.0)),
            volumes,
            look: b("bg_shock_lookcontrol", true),
            look_fade_ms: ms(f("bg_shock_lookcontrol_fadetime", 2.0, 0.001, 1000.0)),
            max_pitch_speed: f("bg_shock_lookcontrol_maxpitchspeed", 90.0, 0.0, f32::MAX),
            max_yaw_speed: f("bg_shock_lookcontrol_maxyawspeed", 90.0, 0.0, f32::MAX),
            sensitivity: f("bg_shock_lookcontrol_mousesensitivityscale", 0.5, 0.0, 2.0),
            movement: b("bg_shock_movement", true),
        }
    }

    /// A channel's volume under the shock (`channels.def`'s name).
    pub fn volume(&self, channel: &str) -> f32 {
        self.volumes
            .get(&channel.to_ascii_lowercase())
            .copied()
            .unwrap_or(DEFAULT_VOLUME)
    }
}

/// `BG_LoadShellShockDvars`' file for `name`.
pub fn path(name: &str) -> String {
    format!("shock/{name}.shock")
}

/// The configstring slots' shellshocks, `ps.shellshockIndex` 1..=15 to
/// slot `index - 1`, whose record slows the walk (`bg_shock_movement`):
/// bit `index` set. `read` hands back a file's text by path; a name with
/// no file reads `shock/default.shock`, as `BG_LoadShellShockDvars` does.
pub fn movement_bits(names: &[String], read: &dyn Fn(&str) -> Option<String>) -> u16 {
    let mut bits = 0;
    for (i, name) in names.iter().enumerate().take(15) {
        if name.is_empty() {
            continue;
        }
        let text = read(&path(name)).or_else(|| read(&path("default")));
        if text.is_some_and(|t| ShockParms::from_text(&t).movement) {
            bits |= 1 << (i + 1);
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 1.7 `flashbang.shock` lines that matter here (its rawfile in
    /// `common_mp.ff`).
    const FLASH: &str = "bg_shock_screenType \"flashed\"\n\
        bg_shock_screenBlurBlendTime \"0.4\"\n\
        bg_shock_screenFlashWhiteFadeTime \"3.5\"\n\
        bg_shock_screenFlashShotFadeTime \"1.0\"\n\
        bg_shock_viewKickPeriod \".75\"\n\
        bg_shock_soundLoop \"flashbang_tinnitus_loop\"\n\
        bg_shock_volume_weapon \"0.1\"\n\
        bg_shock_lookControl \"0\"\n\
        bg_shock_movement \"0\"";

    #[test]
    fn a_shock_file_reads_its_lines_over_the_defaults() {
        let p = ShockParms::from_text(FLASH);
        assert_eq!(p.screen, ScreenType::Flashed);
        assert_eq!(p.flash_white_fade_ms, 3500);
        assert_eq!(p.flash_shot_fade_ms, 1000);
        assert_eq!(p.sound_loop, "flashbang_tinnitus_loop");
        assert_eq!(p.sound_end, "shellshock_end");
        assert_eq!(p.volume("weapon"), 0.1);
        assert_eq!(p.volume("music"), DEFAULT_VOLUME);
        assert!(!p.look && !p.movement && p.sound);
        assert!((p.kick_rate - 0.001 / 0.75).abs() < 1e-9);
        let d = ShockParms::default();
        assert_eq!(d.screen, ScreenType::Blurred);
        assert_eq!((d.loop_end_delay_ms, d.fade_out_ms), (-3000, 2500));
        assert!(d.movement);
    }

    #[test]
    fn the_movement_bits_follow_the_slots() {
        let names = [
            "frag_grenade_mp".to_string(),
            String::new(),
            "flashbang".into(),
        ];
        let read = |p: &str| match p {
            "shock/flashbang.shock" => Some(FLASH.to_string()),
            "shock/default.shock" => Some("bg_shock_movement \"1\"".to_string()),
            _ => None,
        };
        assert_eq!(movement_bits(&names, &read), 1 << 1);
    }
}
