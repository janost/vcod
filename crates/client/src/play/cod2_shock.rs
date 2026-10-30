//! CoD2's shellshock records: `shock/<name>.shock` read into the 29
//! `cg_shock_*` dvars and built into the record `play::shock` steps
//! (docs/research/cod2-hud.md 16). The per-frame half (blend, kick, mouse,
//! sound, `wbuttons` slowdown) is 1.1's: `CG_UpdateShellShock` (Mac
//! 0x1d7528), `CG_DrawShellShockSavedScreenBlend` (0x1d6e64) and
//! `CG_PerturbCamera` (0x1d6c10) run 1.1's formulas over the same 128-point
//! kick table.

use vcod_common::pk3::Pk3Fs;
use vcod_common::text::atoi;

use crate::play::shock::{ShellShock, ShockParams, atof, parse_named};

/// One dvar of `_cg_shock_dvar_names` (Mac `.data` 0x34d680, 29 names):
/// its `CG_RegisterDvars` default and range.
#[derive(Clone, Copy)]
enum Dvar {
    Float(f32, f32, f32),
    Bool(bool),
    /// `cg_shock_soundRoomType` over `_cg_soundRoomTypes`, default `generic`.
    Room,
}

const NAMES: [(&str, Dvar); 29] = [
    ("cg_shock_screenBlendTime", Dvar::Float(0.4, 0.001, 10.0)),
    (
        "cg_shock_screenBlendFadeTime",
        Dvar::Float(1.0, 0.001, 10.0),
    ),
    ("cg_shock_viewKickPeriod", Dvar::Float(0.75, 0.001, 10.0)),
    ("cg_shock_viewKickRadius", Dvar::Float(0.05, 0.0, 1.0)),
    ("cg_shock_sound", Dvar::Bool(true)),
    ("cg_shock_soundFadeInTime", Dvar::Float(0.25, 0.001, 10.0)),
    ("cg_shock_soundFadeOutTime", Dvar::Float(2.5, 0.001, 10.0)),
    ("cg_shock_soundLoopFadeTime", Dvar::Float(1.5, 0.001, 10.0)),
    ("cg_shock_soundLoopEndDelay", Dvar::Float(-3.0, -10.0, 10.0)),
    ("cg_shock_soundRoomType", Dvar::Room),
    ("cg_shock_soundDryLevel", Dvar::Float(1.0, 0.0, 1.0)),
    ("cg_shock_soundWetLevel", Dvar::Float(0.5, 0.0, 1.0)),
    ("cg_shock_soundModEndDelay", Dvar::Float(2.0, -10.0, 10.0)),
    ("cg_shock_volume_auto", Dvar::Float(0.1, 0.0, 1.0)),
    ("cg_shock_volume_auto2d", Dvar::Float(0.1, 0.0, 1.0)),
    ("cg_shock_volume_menu", Dvar::Float(1.0, 0.0, 1.0)),
    ("cg_shock_volume_weapon", Dvar::Float(0.5, 0.0, 1.0)),
    ("cg_shock_volume_voice", Dvar::Float(0.2, 0.0, 1.0)),
    ("cg_shock_volume_item", Dvar::Float(0.1, 0.0, 1.0)),
    ("cg_shock_volume_body", Dvar::Float(0.1, 0.0, 1.0)),
    ("cg_shock_volume_local", Dvar::Float(1.0, 0.0, 1.0)),
    ("cg_shock_volume_music", Dvar::Float(1.0, 0.0, 1.0)),
    ("cg_shock_volume_announcer", Dvar::Float(1.0, 0.0, 1.0)),
    ("cg_shock_volume_shellshock", Dvar::Float(1.0, 0.0, 1.0)),
    ("cg_shock_mouse", Dvar::Bool(true)),
    (
        "cg_shock_mouse_maxpitchspeed",
        Dvar::Float(90.0, 0.0, f32::MAX),
    ),
    (
        "cg_shock_mouse_maxyawspeed",
        Dvar::Float(90.0, 0.0, f32::MAX),
    ),
    (
        "cg_shock_mouse_sensitivityscale",
        Dvar::Float(0.5, 0.0, 2.0),
    ),
    ("cg_shock_mouse_fadeTime", Dvar::Float(2.0, 0.001, 10.0)),
];

/// `_cg_soundRoomTypes` (Mac `.data` 0x34ad00).
const ROOMS: [&str; 26] = [
    "generic",
    "paddedcell",
    "room",
    "bathroom",
    "livingroom",
    "stoneroom",
    "auditorium",
    "concerthall",
    "cave",
    "arena",
    "hangar",
    "carpetedhallway",
    "hallway",
    "stonecorridor",
    "alley",
    "forest",
    "city",
    "mountains",
    "quarry",
    "plain",
    "parkinglot",
    "sewerpipe",
    "underwater",
    "drugged",
    "dizzy",
    "psychotic",
];

/// The `cg_shock_volume_*` dvar feeding each `audio::alias::Channel` index
/// (`auto, menu, weapon, voice, item, body, local, music, announcer,
/// shellshock, auto2d`). `CG_SetShellShockParmsFromDvars` stores body,
/// item, weapon, voice at +0x44..+0x50, so as in 1.1 the four 3D channels
/// after `menu` are crossed.
const VOLUMES: [usize; 11] = [13, 15, 19, 18, 16, 17, 20, 21, 22, 23, 14];

/// A dvar's value as `Dvar_SetFromStringFromSource` leaves it: a value
/// outside the range is refused and the reset default stays.
fn float(text: &str, d: Dvar) -> f32 {
    match d {
        Dvar::Float(def, min, max) => {
            let v = atof(text);
            if (min..=max).contains(&v) { v } else { def }
        }
        Dvar::Bool(def) => {
            if text.trim().is_empty() {
                f32::from(u8::from(def))
            } else {
                f32::from(u8::from(atoi(text) != 0))
            }
        }
        Dvar::Room => 0.0,
    }
}

/// `floorf(x * 1000 + 0.5)`.
fn ms(x: f32) -> i32 {
    (x * 1000.0 + 0.5).floor() as i32
}

/// `CG_SetShellShockParmsFromDvars` (Mac 0x1d6f2a) over the 29 values in
/// [`NAMES`] order.
pub fn from_values(v: &[String]) -> ShockParams {
    let f = |i: usize| float(&v[i], NAMES[i].1);
    let period = f(2);
    let room = ROOMS
        .iter()
        .find(|r| r.eq_ignore_ascii_case(v[9].trim()))
        .copied()
        .unwrap_or(ROOMS[0]);
    ShockParams {
        blend_ms: ms(f(0)),
        blend_fade_ms: ms(f(1)),
        kick_rate: if 0.001 - period < 0.0 {
            0.001 / period
        } else {
            1.0
        },
        kick_radius: f(3),
        sound: f(4) != 0.0,
        fade_in_ms: ms(f(5)),
        fade_out_ms: ms(f(6)),
        loop_fade_ms: ms(f(7)),
        loop_end_delay_ms: ms(f(8)),
        room: room.to_string(),
        wet: f(11),
        mod_end_delay_ms: ms(f(12)),
        volumes: VOLUMES.map(|i| f(i).clamp(0.0, 1.0)),
        mouse: f(24) != 0.0,
        max_pitch: f(25),
        max_yaw: f(26),
        sensitivity: f(27),
        mouse_fade_ms: ms(f(28)),
    }
}

/// `CG_LoadShellShockDvars` (Mac 0x1d6d88): `shock/<name>.shock`, or
/// `shock/default.shock` when that cannot be opened; `None` when neither
/// opens or the file leaves a dvar out.
pub fn load(fs: &Pk3Fs, name: &str) -> Option<ShockParams> {
    let path = format!("shock/{name}.shock");
    let (path, bytes) = match fs.read(&path) {
        Some(b) => (path, b),
        None => {
            log::warn!("cod2 shellshock: couldn't open '{path}'");
            let d = "shock/default.shock".to_string();
            let b = fs.read(&d)?;
            (d, b)
        }
    };
    let names = NAMES.map(|(n, _)| n);
    match parse_named(&String::from_utf8_lossy(&bytes), &path, &names) {
        Ok(v) => Some(from_values(&v)),
        Err(missing) => {
            log::warn!(
                "cod2 shellshock: the following dvars were not specified in file '{path}': {}",
                missing.join(" ")
            );
            None
        }
    }
}

/// The record of the 29 dvars at their `CG_RegisterDvars` defaults, what
/// `cg_shellshock <duration>` plays before any file loaded.
pub fn defaults() -> ShockParams {
    let v: Vec<String> = NAMES
        .iter()
        .map(|(_, d)| match *d {
            Dvar::Float(def, _, _) => def.to_string(),
            Dvar::Bool(b) => u8::from(b).to_string(),
            Dvar::Room => ROOMS[0].to_string(),
        })
        .collect();
    from_values(&v)
}

/// A CoD2 session's shellshock state: records by configstring name,
/// loaded from `shock/`.
pub fn state() -> ShellShock {
    ShellShock::with_loader(load, defaults)
}

/// The shellshock's turn caps (`CL_CapTurnRate`, Mac 0x17b078, into
/// `cl + 0x24788` pitch and `0x2478c` yaw, degrees a second, 0 for none):
/// `CL_MouseMove` (Mac 0x18908e) holds a frame's mouse pitch and yaw, after
/// the sensitivity and `m_pitch` / `m_yaw`, to `frame_msec * cap * 0.001`
/// either way. `CG_UpdateShellShock` (Mac 0x1d7528) sets them each frame
/// to the record's `max*speed` over the mouse fade's fraction, and 0
/// without a shock (`play::shock::ShockMouse`). vcod's mouse arrives in
/// several moves a frame; their sum is held to the cap.
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnCap {
    caps: [Option<f32>; 2],
    sum: [f32; 2],
    passed: [f32; 2],
}

impl TurnCap {
    /// A new frame of `frame_ms` under `max` (pitch, yaw).
    pub fn set(&mut self, frame_ms: f32, max: [f32; 2]) {
        *self = TurnCap {
            caps: max.map(|m| (m != 0.0).then_some(frame_ms * m * 0.001)),
            ..TurnCap::default()
        };
    }

    /// One mouse move's `[pitch, yaw]` degrees, what of it the frame's
    /// caps let through.
    pub fn turn(&mut self, d: [f32; 2]) -> [f32; 2] {
        let mut out = d;
        for (axis, o) in out.iter_mut().enumerate() {
            if let Some(cap) = self.caps[axis] {
                self.sum[axis] += *o;
                let total = self.sum[axis].clamp(-cap, cap);
                *o = total - self.passed[axis];
                self.passed[axis] = total;
            }
        }
        out
    }
}

/// `player_breath_hold_time`, `player_breath_snd_lerp` and
/// `player_breath_snd_delay` at their registered defaults (`BG_RegisterDvars`, Mac
/// 0x694d4: 4.5, 2 and 1).
const BREATH_HOLD_S: f32 = 4.5;
const BREATH_SND_LERP: f32 = 2.0;
const BREATH_SND_DELAY_S: f32 = 1.0;
/// `pm_flags` 0x4000: the breath is held (cod2-movement.md 19).
pub const PMF_BREATH: i32 = 0x4000;

/// The hold-breath sounds and mix `CG_AddPlayerWeapon` (Mac 0x1d9c8e)
/// plays for the view weapon each frame: `cg + 0x2cd00` (ms into the hold,
/// -1 off it), `0x2cd04` (the breath in's length), `0x2cd08` (ms until a
/// breath may play again) and `0x2cd0c` (the mix's fraction), set back by
/// `CG_HoldBreathInit` (Mac 0x1d7ddc). The mix is `hold_breath.shock`'s
/// eleven volumes (`cgs + 0x23284`, built at the gamestate), each `(v - 1)
/// * f + 1`, at channel priority 1.
pub struct HoldBreath {
    held_ms: i32,
    in_ms: i32,
    delay_ms: i32,
    lerp: f32,
    /// `hold_breath.shock`, loaded on first use.
    record: Option<Option<ShockParams>>,
}

impl Default for HoldBreath {
    fn default() -> Self {
        HoldBreath {
            held_ms: -1,
            in_ms: 0,
            delay_ms: 0,
            lerp: 0.0,
            record: None,
        }
    }
}

/// One frame of [`HoldBreath`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BreathFrame {
    /// Aliases to play once on the client (`CG_PlayClientSoundAlias`).
    pub cues: Vec<&'static str>,
    /// `weap_sniper_heartbeat` (a looping alias) asked for this frame.
    pub heartbeat: bool,
    /// The channel volumes at priority 1, `None` deactivated.
    pub volumes: Option<[f32; 11]>,
}

impl HoldBreath {
    /// A new cgame: the timers back to rest, the record kept.
    pub fn reset(&mut self) {
        *self = HoldBreath {
            record: self.record.take(),
            ..HoldBreath::default()
        };
    }

    /// The breath in's length off the sound system
    /// (`CG_PlayClientSoundAlias`' return), once it started.
    pub fn set_in_ms(&mut self, ms: i32) {
        self.in_ms = ms;
    }

    /// One frame of `ms` (`cg.frametime`) with `pm_flags` 0x4000 `held`.
    /// `volumes` is the record's eleven in channel order.
    pub fn step(&mut self, held: bool, ms: i32, volumes: Option<[f32; 11]>) -> BreathFrame {
        let mut out = BreathFrame::default();
        if self.delay_ms > 0 {
            self.delay_ms -= ms;
        }
        let delay = (BREATH_SND_DELAY_S * 1000.0) as i32;
        if !held {
            if self.held_ms >= 0 {
                self.held_ms += ms;
                if self.held_ms > (BREATH_HOLD_S * 1000.0) as i32 {
                    out.cues.push("weap_sniper_breathgasp");
                } else if self.delay_ms < 1 {
                    out.cues.push("weap_sniper_breathout");
                    self.delay_ms = delay;
                }
            }
            self.held_ms = -1;
            self.in_ms = 0;
            self.lerp = 0.0;
        } else {
            self.lerp = crate::play::cod2_gun::diff_track(
                1.0,
                self.lerp,
                BREATH_SND_LERP,
                ms as f32 * 0.001,
            );
            if self.held_ms < 0 {
                self.held_ms = 0;
                if self.delay_ms < 1 {
                    out.cues.push("weap_sniper_breathin");
                    self.delay_ms = delay;
                } else {
                    self.in_ms = 0;
                }
            } else if self.in_ms < self.held_ms {
                out.heartbeat = true;
            }
            self.held_ms += ms;
        }
        if self.lerp != 0.0 {
            let f = self.lerp;
            out.volumes = volumes.map(|v| v.map(|v| (v - 1.0) * f + 1.0));
        }
        out
    }

    /// `hold_breath.shock`'s volumes; `CG_RegisterGraphics` makes a missing
    /// file a `Com_Error`, vcod a silent mix.
    pub fn volumes(&mut self, fs: &Pk3Fs) -> Option<[f32; 11]> {
        self.record
            .get_or_insert_with(|| load(fs, "hold_breath"))
            .as_ref()
            .map(|p| p.volumes)
    }
}

/// Configstring `CS_SHELLSHOCKS + i` names shock `i` (1..15); slot 0 is
/// the console's (`CG_RegisterGraphics`, Mac 0x143c4a: `CL_GetConfigString(i
/// + 0x48e)`).
pub const CS_SHELLSHOCKS: usize = 0x48e;

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_SHOCK: &str = "cg_shock_screenBlendTime \"0.4\"\n\
        cg_shock_screenBlendFadeTime \"1\"\n\
        cg_shock_viewKickPeriod \".75\"\n\
        cg_shock_viewKickRadius \".05\"\n\
        cg_shock_viewKickFadeTime \"3\"\n\
        cg_shock_sound \"1\"\n\
        cg_shock_soundFadeInTime \".25\"\n\
        cg_shock_soundFadeOutTime \"2.5\"\n\
        cg_shock_soundLoopFadeTime \"2\"\n\
        cg_shock_soundLoopEndDelay \"-1.5\"\n\
        cg_shock_soundRoomType \"underwater\"\n\
        cg_shock_soundDryLevel \"1.0\"\n\
        cg_shock_soundWetLevel \"0.50\"\n\
        cg_shock_soundModEndDelay \"-.75\"\n\
        cg_shock_volume_auto \"0.1\"\n\
        cg_shock_volume_auto2d \"0.1\"\n\
        cg_shock_volume_menu \"1.0\"\n\
        cg_shock_volume_weapon \"0.5\"\n\
        cg_shock_volume_voice \"0.2\"\n\
        cg_shock_volume_item \"0.1\"\n\
        cg_shock_volume_body \"0.1\"\n\
        cg_shock_volume_local \"0.1\"\n\
        cg_shock_volume_music \"0.1\"\n\
        cg_shock_volume_announcer \"1\"\n\
        cg_shock_volume_shellshock \"1.0\"\n\
        cg_shock_mouse \"1\"\n\
        cg_shock_mouse_maxpitchspeed \"90\"\n\
        cg_shock_mouse_maxyawspeed \"90\"\n\
        cg_shock_mouse_sensitivityscale \"0.5\"\n\
        cg_shock_mouse_fadeTime \"2\"\n";

    /// `iw_07.iwd`'s `shock/default.shock`, the one stock MP's
    /// `_shellshock.gsc` plays, through the record builder.
    #[test]
    fn default_shock_builds_retails_record() {
        let names = NAMES.map(|(n, _)| n);
        let p = from_values(&parse_named(DEFAULT_SHOCK, "default", &names).unwrap());
        assert_eq!((p.blend_ms, p.blend_fade_ms), (400, 1000));
        assert!((p.kick_rate - 0.001 / 0.75).abs() < 1e-9);
        assert_eq!(p.kick_radius, 0.05);
        assert_eq!(
            (p.fade_in_ms, p.fade_out_ms, p.loop_fade_ms),
            (250, 2500, 2000)
        );
        assert_eq!((p.loop_end_delay_ms, p.mod_end_delay_ms), (-1500, -750));
        assert_eq!((p.room.as_str(), p.wet), ("underwater", 0.5));
        // auto, menu, weapon <- body, voice <- item, item <- weapon,
        // body <- voice, local, music, announcer, shellshock, auto2d.
        assert_eq!(
            p.volumes,
            [0.1, 1.0, 0.1, 0.1, 0.5, 0.2, 0.1, 0.1, 1.0, 1.0, 0.1]
        );
        assert!(p.sound && p.mouse);
        assert_eq!((p.max_pitch, p.max_yaw, p.sensitivity), (90.0, 90.0, 0.5));
        assert_eq!(p.mouse_fade_ms, 2000);
    }

    /// A value outside its dvar's range keeps the default; a 0.001 s
    /// period kicks once a millisecond.
    #[test]
    fn out_of_range_values_keep_the_default() {
        let mut v: Vec<String> = NAMES.iter().map(|_| "0".to_string()).collect();
        v[0] = "20".into();
        v[2] = "0.001".into();
        v[9] = "nowhere".into();
        let p = from_values(&v);
        assert_eq!(p.blend_ms, 400);
        assert_eq!(p.kick_rate, 1.0);
        assert_eq!(p.room, "generic");
        // 0 is below the fade dvars' 0.001: their defaults.
        assert_eq!(p.fade_in_ms, 250);
    }

    /// `cg_shellshock <duration>` before any file plays the registered
    /// dvars: CoD2's 29, not 1.1's 27.
    #[test]
    fn the_console_shock_takes_cod2s_defaults() {
        let d = defaults();
        assert_eq!((d.blend_ms, d.blend_fade_ms), (400, 1000));
        assert_eq!((d.loop_end_delay_ms, d.mod_end_delay_ms), (-3000, 2000));
        assert_eq!((d.max_pitch, d.max_yaw, d.sensitivity), (90.0, 90.0, 0.5));
        assert_eq!(d.room, "generic");
        let mut s = state();
        let fs = Pk3Fs::empty();
        s.command(&fs, &["2.5"], 1000).unwrap();
        let f = s.frame(&fs, |_| None, (0, 0, 0), 1500);
        assert!(f.slowed && f.left_ms == 2000, "{}", f.left_ms);
        // A file that does not load starts nothing and says nothing more.
        assert_eq!(s.command(&fs, &["1", "nothing"], 1000), Err(String::new()));
        assert!(s.command(&fs, &[], 0).is_err());
    }

    /// A frame's turn is held to `frame_ms * cap / 1000` either way over
    /// all its moves; a cap of 0 passes everything.
    #[test]
    fn the_turn_caps_hold_a_frames_mouse() {
        let mut t = TurnCap::default();
        assert_eq!(t.turn([5.0, -7.0]), [5.0, -7.0]);
        t.set(20.0, [90.0, 0.0]);
        // 1.8 degrees of pitch a 20 ms frame.
        assert_eq!(t.turn([1.0, 3.0]), [1.0, 3.0]);
        let [p, y] = t.turn([1.5, 3.0]);
        assert!((p - 0.8).abs() < 1e-6 && y == 3.0, "{p} {y}");
        assert_eq!(t.turn([1.0, 0.0])[0], 0.0);
        // Back the other way: the frame's sum, 0.5, is under the cap.
        let back = t.turn([-3.0, 0.0])[0];
        assert!((back + 1.3).abs() < 1e-5, "{back}");
        t.set(20.0, [90.0, 0.0]);
        assert!((t.turn([-4.0, 0.0])[0] + 1.8).abs() < 1e-6);
    }

    /// A hold breathes in, the heartbeat comes once the breath in has run,
    /// the mix eases in at 2 a second; a let-go inside the hold time
    /// breathes out, one past it gasps, and the second breath waits out the
    /// 1 s delay.
    #[test]
    fn the_breath_hold_breathes_and_mixes() {
        let vols = Some([0.5; 11]);
        let mut b = HoldBreath::default();
        let f = b.step(false, 16, vols);
        assert_eq!(f, BreathFrame::default());
        let f = b.step(true, 100, vols);
        assert_eq!(f.cues, ["weap_sniper_breathin"]);
        assert!(!f.heartbeat);
        // DiffTrack: a fifth of the gap at 2 a second over 100 ms.
        assert!(
            (f.volumes.unwrap()[0] - 0.9).abs() < 1e-6,
            "{:?}",
            f.volumes
        );
        b.set_in_ms(300);
        assert!(!b.step(true, 100, vols).heartbeat, "held 100 of 300");
        assert!(!b.step(true, 100, vols).heartbeat);
        assert!(!b.step(true, 100, vols).heartbeat, "held 300 of 300");
        assert!(b.step(true, 100, vols).heartbeat);
        // The breath in's delay runs out 1 s after it.
        for _ in 0..6 {
            b.step(true, 100, vols);
        }
        let f = b.step(false, 100, vols);
        assert_eq!(f.cues, ["weap_sniper_breathout"]);
        assert_eq!(f.volumes, None);
        // Again at once: the breath out's delay runs, so no breath in, and
        // the heartbeat starts on the next frame.
        let f = b.step(true, 100, vols);
        assert!(f.cues.is_empty());
        assert!(b.step(true, 100, vols).heartbeat);
        for _ in 0..50 {
            b.step(true, 100, vols);
        }
        assert_eq!(b.step(false, 100, vols).cues, ["weap_sniper_breathgasp"]);
        // No record, no mix.
        assert_eq!(b.step(true, 100, None).volumes, None);
    }

    /// Every stock `.shock` in the install parses.
    #[test]
    fn stock_shock_files_load() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        for name in ["default", "pain", "hold_breath", "victoryscreen"] {
            assert!(load(&fs, name).is_some(), "{name}");
        }
        // A missing name falls back to the default file.
        assert_eq!(load(&fs, "no_such_shock"), load(&fs, "default"));
    }
}
