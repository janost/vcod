//! Weapon state machine: picks the viewmodel clip (idle, fire, rechamber,
//! reload, ADS) from LMB/RMB/R input and weapon-file times. Pure logic.
//!
//! Checked against the six WALK_LOADOUT files. Not modelled: sway, kick,
//! segmented reloads, altWeapon. `adsBobFactor` semantics are INFERRED from
//! its values (1 on rifles, 0 on thompson/springfield); no decompilation
//! evidence yet.

use crate::pk3::Pk3Fs;
use anyhow::{anyhow, Result};
use std::collections::HashMap;

/// Degrees.
pub const DEFAULT_FOV: f32 = 75.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum WeaponAnim {
    Idle,
    Fire,
    LastShot,
    Rechamber,
    Reload,
    AdsUp,
    AdsDown,
    AdsFire,
    AdsLastShot,
    AdsRechamber,
    Raise,
}

impl WeaponAnim {
    pub const ALL: [WeaponAnim; 11] = [
        WeaponAnim::Idle,
        WeaponAnim::Fire,
        WeaponAnim::LastShot,
        WeaponAnim::Rechamber,
        WeaponAnim::Reload,
        WeaponAnim::AdsUp,
        WeaponAnim::AdsDown,
        WeaponAnim::AdsFire,
        WeaponAnim::AdsLastShot,
        WeaponAnim::AdsRechamber,
        WeaponAnim::Raise,
    ];

    pub fn key(self) -> &'static str {
        match self {
            WeaponAnim::Idle => "idleAnim",
            WeaponAnim::Fire => "fireAnim",
            WeaponAnim::LastShot => "lastShotAnim",
            WeaponAnim::Rechamber => "rechamberAnim",
            WeaponAnim::Reload => "reloadAnim",
            WeaponAnim::AdsUp => "adsUpAnim",
            WeaponAnim::AdsDown => "adsDownAnim",
            WeaponAnim::AdsFire => "adsFireAnim",
            WeaponAnim::AdsLastShot => "adsLastShotAnim",
            WeaponAnim::AdsRechamber => "adsRechamberAnim",
            WeaponAnim::Raise => "raiseAnim",
        }
    }
}

/// Alias names from the weapon file's `*Sound` keys
/// (docs/research/cod11-sound-system.md, section 8). Blank is the norm and
/// means no sound, so it parses as `None` without a warning.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct WeaponSounds {
    pub fire: Option<String>,
    pub last_shot: Option<String>,
    pub rechamber: Option<String>,
    pub reload: Option<String>,
    pub reload_empty: Option<String>,
    pub reload_start: Option<String>,
    pub reload_end: Option<String>,
    pub alt_switch: Option<String>,
    pub raise: Option<String>,
    pub putaway: Option<String>,
    pub pullback: Option<String>,
    pub pickup: Option<String>,
    /// In-flight loop. Unused: vcod tracks no missile entities.
    pub projectile: Option<String>,
    pub proj_explosion: Option<String>,
    /// `EV_MELEE_SWIPE` plays `melee_swing_large` instead of `_small`.
    pub rifle_bullet: bool,
    /// Suppresses `player_out_of_ammo` on `EV_NOAMMO`.
    pub clip_only: bool,
}

fn opt_str(map: &HashMap<String, String>, key: &str) -> Option<String> {
    map.get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl WeaponSounds {
    pub fn from_map(map: &HashMap<String, String>) -> WeaponSounds {
        WeaponSounds {
            fire: opt_str(map, "fireSound"),
            last_shot: opt_str(map, "lastShotSound"),
            rechamber: opt_str(map, "rechamberSound"),
            reload: opt_str(map, "reloadSound"),
            reload_empty: opt_str(map, "reloadEmptySound"),
            reload_start: opt_str(map, "reloadStartSound"),
            reload_end: opt_str(map, "reloadEndSound"),
            alt_switch: opt_str(map, "altSwitchSound"),
            raise: opt_str(map, "raiseSound"),
            putaway: opt_str(map, "putawaySound"),
            pullback: opt_str(map, "pullbackSound"),
            pickup: opt_str(map, "pickupSound"),
            projectile: opt_str(map, "projectileSound"),
            proj_explosion: opt_str(map, "projExplosionSound"),
            rifle_bullet: parse_bool(map, "rifleBullet", false),
            clip_only: parse_bool(map, "clipOnly", false),
        }
    }
}

/// Times in seconds.
#[derive(Clone)]
pub struct WeaponDef {
    pub clip_size: u32, // >= 1
    pub fire_time: f32,
    pub rechamber_time: f32,
    pub reload_time: f32,
    pub raise_time: f32,
    pub ads_trans_in: f32,
    pub ads_trans_out: f32,
    pub ads_zoom_fov: f32, // DEFAULT_FOV means no zoom
    pub ads_view_bob_mult: f32,
    /// Second ADS bob multiplier the files carry separately; 1.0 changes
    /// nothing, thompson/springfield ship 0 (bob frozen when fully aimed).
    /// Semantics INFERRED from the values; no decompilation evidence yet.
    pub ads_bob_factor: f32,
    /// `semiAuto 0` fires while the button is held at `fireTime` cadence.
    pub semi_auto: bool,
    /// Reserve rounds behind the clip (`startAmmo`).
    pub start_ammo: u32,
    pub bolt_action: bool,
    /// Dropped-weapon xmodel, `xmodel/` prefix stripped.
    pub world_model: Option<String>,
    /// Third-person muzzle flash, a full `fx/*.efx` path as written
    /// (docs/research/cod11-events-and-fx.md, section 5b). `None` lets
    /// `fx::registry::resolve` pick a class-based flash.
    pub world_flash_effect: Option<String>,
    /// Killfeed material, kept verbatim: `Pk3Fs` resolves the `@` itself.
    /// `None` when absent or empty; the killfeed then uses the means-of-death
    /// icon (docs/research/cod11-hud-protocol.md, section 2).
    pub kill_icon: Option<String>,
    /// Double-width `kill_icon` (hud-protocol doc, section 2).
    pub wide_kill_icon: bool,
    pub sounds: WeaponSounds,
}

fn parse_f32(map: &HashMap<String, String>, key: &str, default: f32) -> f32 {
    match map.get(key) {
        Some(v) => v.trim().parse().unwrap_or_else(|_| {
            log::warn!("weapon: invalid {key} value {v:?}, using default {default}");
            default
        }),
        None => {
            log::warn!("weapon: missing {key}, using default {default}");
            default
        }
    }
}

fn parse_u32(map: &HashMap<String, String>, key: &str, default: u32) -> u32 {
    match map.get(key) {
        Some(v) => v.trim().parse().unwrap_or_else(|_| {
            log::warn!("weapon: invalid {key} value {v:?}, using default {default}");
            default
        }),
        None => {
            log::warn!("weapon: missing {key}, using default {default}");
            default
        }
    }
}

/// Absence is normal (no `boltAction` on a semi-auto) and stays silent;
/// garbage warns.
fn parse_bool(map: &HashMap<String, String>, key: &str, default: bool) -> bool {
    match map.get(key).map(|v| v.trim()) {
        Some("0") => false,
        Some("1") => true,
        Some(v) => {
            log::warn!("weapon: invalid {key} value {v:?}, using default {default}");
            default
        }
        None => default,
    }
}

/// `name` is the CS 7 name (`kar98k_mp`), without the `weapons/mp/` prefix.
pub fn load(fs: &Pk3Fs, name: &str) -> Result<WeaponDef> {
    let path = format!("weapons/mp/{name}");
    let text = fs
        .read(&path)
        .ok_or_else(|| anyhow!("{path} not found in pk3s"))?;
    let map = crate::xmodel::parse_weapon(&String::from_utf8_lossy(&text));
    Ok(WeaponDef::from_map(&map))
}

impl WeaponDef {
    /// Times default to 0, so a missing time completes its state instantly.
    pub fn from_map(map: &HashMap<String, String>) -> WeaponDef {
        WeaponDef {
            clip_size: parse_u32(map, "clipSize", 1).max(1),
            fire_time: parse_f32(map, "fireTime", 0.0),
            rechamber_time: parse_f32(map, "rechamberTime", 0.0),
            reload_time: parse_f32(map, "reloadTime", 0.0),
            raise_time: parse_f32(map, "raiseTime", 0.0),
            ads_trans_in: parse_f32(map, "adsTransInTime", 0.0),
            ads_trans_out: parse_f32(map, "adsTransOutTime", 0.0),
            ads_zoom_fov: parse_f32(map, "adsZoomFov", DEFAULT_FOV),
            ads_view_bob_mult: parse_f32(map, "adsViewBobMult", 1.0),
            ads_bob_factor: parse_f32(map, "adsBobFactor", 1.0),
            semi_auto: parse_bool(map, "semiAuto", true),
            start_ammo: parse_u32(map, "startAmmo", 0),
            bolt_action: parse_bool(map, "boltAction", false),
            world_model: map
                .get("worldModel")
                .map(|v| v.strip_prefix("xmodel/").unwrap_or(v).to_string()),
            world_flash_effect: map.get("worldFlashEffect").map(|v| v.trim().to_string()),
            kill_icon: opt_str(map, "killIcon"),
            wide_kill_icon: parse_bool(map, "wideKillIcon", false),
            sounds: WeaponSounds::from_map(map),
        }
    }
}

/// Debounced by the caller.
#[derive(Default, Copy, Clone)]
pub struct WeaponInput {
    pub fire: bool,      // LMB edge this frame
    pub fire_held: bool, // LMB down; only full-auto (`semiAuto 0`) acts on it
    pub ads: bool,       // RMB held
    pub reload: bool,    // R edge this frame
}

pub struct WeaponOut {
    pub anim: WeaponAnim,
    pub anim_time: f32, // seconds into the clip
    pub looping: bool,  // only Idle
    pub ads_frac: f32,  // 0..1
    pub fired: bool,
    /// Sound cue the client plays this frame; consumed once.
    pub cue: Option<WeaponCue>,
}

/// The state transitions that carry a weapon-file sound
/// (docs/research/cod11-sound-system.md, section 8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WeaponCue {
    Fire,
    LastShot,
    Rechamber,
    Reload,
    ReloadFromEmpty,
    Raise,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum State {
    Raising,
    Idle,
    AdsUp,
    AdsIdle,
    AdsDown,
    Firing,
    Rechambering,
    Reloading,
}

pub struct WeaponState {
    state: State,
    t: f32,
    ammo: u32,
    ads_frac: f32,
    def: WeaponDef,
    /// Hip or Ads* clips for the Firing/Rechambering pair, fixed at the fire edge.
    ads_variant: bool,
    fire_anim: WeaponAnim,
    /// Set by the transitions that make a sound; `output` hands it out once.
    pending_cue: Option<WeaponCue>,
}

impl WeaponState {
    pub fn new(def: WeaponDef) -> WeaponState {
        WeaponState {
            state: State::Raising,
            t: 0.0,
            ammo: def.clip_size,
            ads_frac: 0.0,
            def,
            ads_variant: false,
            fire_anim: WeaponAnim::Fire,
            pending_cue: Some(WeaponCue::Raise),
        }
    }

    /// Rounds left in the clip, for HUD readouts.
    pub fn ammo(&self) -> u32 {
        self.ammo
    }

    pub fn update(&mut self, dt: f32, input: WeaponInput) -> WeaponOut {
        let mut fired = false;

        match self.state {
            // No input buffering; RMB still integrates below.
            State::Raising | State::Firing | State::Rechambering | State::Reloading => {}
            State::Idle | State::AdsIdle | State::AdsUp | State::AdsDown => {
                let trigger = input.fire || (!self.def.semi_auto && input.fire_held);
                if trigger && self.ammo > 0 {
                    self.enter_firing();
                    fired = true;
                } else if input.reload && self.ammo < self.def.clip_size {
                    self.pending_cue = Some(if self.ammo == 0 {
                        WeaponCue::ReloadFromEmpty
                    } else {
                        WeaponCue::Reload
                    });
                    self.enter(State::Reloading, 0.0);
                } else {
                    self.handle_ads_toggle(input.ads);
                }
            }
        }

        self.t += dt;
        self.integrate_ads_frac(dt, input.ads);

        if self.t >= self.duration() {
            self.complete(input.ads, input.fire_held, &mut fired);
        }

        self.output(fired)
    }

    fn enter(&mut self, state: State, t: f32) {
        self.state = state;
        self.t = t;
    }

    fn enter_firing(&mut self) {
        self.ammo -= 1;
        let last_shot = self.ammo == 0;
        self.pending_cue = Some(if last_shot {
            WeaponCue::LastShot
        } else {
            WeaponCue::Fire
        });
        self.ads_variant = self.ads_frac >= 0.5;
        self.fire_anim = match (self.ads_variant, last_shot) {
            (false, false) => WeaponAnim::Fire,
            (false, true) => WeaponAnim::LastShot,
            (true, false) => WeaponAnim::AdsFire,
            (true, true) => WeaponAnim::AdsLastShot,
        };
        self.enter(State::Firing, 0.0);
    }

    fn handle_ads_toggle(&mut self, ads: bool) {
        let switching = matches!(
            (self.state, ads),
            (State::Idle, true)
                | (State::AdsIdle, false)
                | (State::AdsUp, false)
                | (State::AdsDown, true)
        );
        if switching {
            self.goto_ads(ads);
        }
    }

    /// Mirrors `t` from `ads_frac` so a half-raised ADS never snaps. At the
    /// extremes goes straight to the settled state, so a transition never
    /// starts past its own duration.
    fn goto_ads(&mut self, ads: bool) {
        if ads {
            if self.ads_frac >= 1.0 {
                self.enter(State::AdsIdle, 0.0);
            } else {
                let t = self.ads_frac * self.def.ads_trans_in;
                self.enter(State::AdsUp, t);
            }
        } else if self.ads_frac <= 0.0 {
            self.enter(State::Idle, 0.0);
        } else {
            let t = (1.0 - self.ads_frac) * self.def.ads_trans_out;
            self.enter(State::AdsDown, t);
        }
    }

    fn integrate_ads_frac(&mut self, dt: f32, ads: bool) {
        let forced_down = matches!(self.state, State::Raising | State::Reloading);
        let target_up = ads && !forced_down;
        let dur = if target_up {
            self.def.ads_trans_in
        } else {
            self.def.ads_trans_out
        };
        let next = if dur <= 0.0 {
            if target_up {
                1.0
            } else {
                0.0
            }
        } else if target_up {
            self.ads_frac + dt / dur
        } else {
            self.ads_frac - dt / dur
        };
        self.ads_frac = next.clamp(0.0, 1.0);
    }

    fn duration(&self) -> f32 {
        match self.state {
            State::Raising => self.def.raise_time,
            State::AdsUp => self.def.ads_trans_in,
            State::AdsDown => self.def.ads_trans_out,
            State::Firing => self.def.fire_time,
            State::Rechambering => self.def.rechamber_time,
            State::Reloading => self.def.reload_time,
            State::Idle | State::AdsIdle => f32::INFINITY,
        }
    }

    fn complete(&mut self, ads: bool, fire_held: bool, fired: &mut bool) {
        match self.state {
            State::Raising => {
                if !self.def.semi_auto && fire_held && self.ammo > 0 {
                    self.enter_firing();
                    *fired = true;
                } else {
                    self.enter(State::Idle, 0.0);
                }
            }
            State::AdsUp => self.enter(State::AdsIdle, 0.0),
            State::AdsDown => self.enter(State::Idle, 0.0),
            State::Firing => {
                if self.def.bolt_action && self.ammo > 0 {
                    self.pending_cue = Some(WeaponCue::Rechamber);
                    self.enter(State::Rechambering, 0.0);
                } else if !self.def.semi_auto && fire_held && self.ammo > 0 {
                    self.enter_firing();
                    *fired = true;
                } else {
                    self.goto_ads(ads);
                }
            }
            State::Rechambering => self.goto_ads(ads),
            State::Reloading => {
                self.ammo = self.def.clip_size;
                self.goto_ads(ads);
            }
            State::Idle | State::AdsIdle => {}
        }
    }

    fn output(&mut self, fired: bool) -> WeaponOut {
        let rechamber_anim = if self.ads_variant {
            WeaponAnim::AdsRechamber
        } else {
            WeaponAnim::Rechamber
        };
        let (anim, anim_time, looping) = match self.state {
            State::Raising => (WeaponAnim::Raise, self.t, false),
            State::Idle => (WeaponAnim::Idle, self.t, true),
            State::AdsUp => (WeaponAnim::AdsUp, self.t, false),
            // Holds the last frame of the up-transition.
            State::AdsIdle => (WeaponAnim::AdsUp, self.def.ads_trans_in, false),
            State::AdsDown => (WeaponAnim::AdsDown, self.t, false),
            State::Firing => (self.fire_anim, self.t, false),
            State::Rechambering => (rechamber_anim, self.t, false),
            State::Reloading => (WeaponAnim::Reload, self.t, false),
        };
        WeaponOut {
            anim,
            anim_time,
            looping,
            ads_frac: self.ads_frac,
            fired,
            cue: self.pending_cue.take(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xmodel;

    #[test]
    fn sound_keys_blank_means_absent() {
        let mut map = HashMap::new();
        map.insert("fireSound".to_string(), "weap_kar98k_fire".to_string());
        map.insert("lastShotSound".to_string(), "".to_string());
        map.insert(
            "reloadSound".to_string(),
            " weap_kar98k_reload ".to_string(),
        );
        map.insert("rifleBullet".to_string(), "1".to_string());
        let s = WeaponSounds::from_map(&map);
        assert_eq!(s.fire.as_deref(), Some("weap_kar98k_fire"));
        assert_eq!(s.last_shot, None);
        assert_eq!(s.reload.as_deref(), Some("weap_kar98k_reload"));
        assert_eq!(s.proj_explosion, None);
        assert!(s.rifle_bullet && !s.clip_only);
        assert_eq!(WeaponDef::from_map(&map).sounds, s);
    }

    #[test]
    fn retail_thompson_sounds() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let def = load(&fs, "thompson_mp").unwrap();
        assert_eq!(def.sounds.fire.as_deref(), Some("weap_thompson_fire"));
        assert_eq!(
            def.sounds.reload_empty.as_deref(),
            Some("weap_thompson_reload")
        );
        assert_eq!(
            def.sounds.alt_switch.as_deref(),
            Some("weap_thompson_altswitch")
        );
    }

    #[test]
    fn retail_files_parse_semi_auto_start_ammo_and_ads_bob() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let kar = load(&fs, "kar98k_mp").unwrap();
        assert!(kar.semi_auto);
        assert_eq!(kar.start_ammo, 60);
        assert!((kar.ads_bob_factor - 1.0).abs() < 1e-6);
        let thompson = load(&fs, "thompson_mp").unwrap();
        assert!(!thompson.semi_auto, "thompson is full-auto");
        assert_eq!(thompson.start_ammo, 270);
        assert_eq!(thompson.ads_bob_factor, 0.0);
    }

    fn def() -> WeaponDef {
        WeaponDef {
            clip_size: 5,
            fire_time: 0.33,
            rechamber_time: 1.0,
            reload_time: 2.5,
            raise_time: 0.5,
            ads_trans_in: 0.3,
            ads_trans_out: 0.4,
            ads_zoom_fov: 50.0,
            ads_view_bob_mult: 0.2,
            ads_bob_factor: 1.0,
            semi_auto: true,
            start_ammo: 60,
            bolt_action: true,
            world_model: None,
            world_flash_effect: None,
            kill_icon: None,
            wide_kill_icon: false,
            sounds: WeaponSounds::default(),
        }
    }

    fn step(w: &mut WeaponState, secs: f32, input: WeaponInput) -> WeaponOut {
        let n = ((secs / (1.0 / 60.0)).ceil() as usize).max(1);
        let mut out = None;
        for _ in 0..n {
            out = Some(w.update(1.0 / 60.0, input));
        }
        out.unwrap()
    }

    /// The bolt-action drain: Raise, then Fire/Rechamber per shot, the
    /// emptying shot as LastShot with no rechamber after it.
    #[test]
    fn cues_follow_the_transitions() {
        let mut w = WeaponState::new(def());
        let fire = WeaponInput {
            fire: true,
            ..Default::default()
        };
        assert_eq!(
            w.update(0.0, WeaponInput::default()).cue,
            Some(WeaponCue::Raise)
        );
        // consumed once
        assert_eq!(w.update(1.0 / 60.0, WeaponInput::default()).cue, None);
        step(&mut w, 0.6, WeaponInput::default());

        for _ in 0..4 {
            let out = w.update(1.0 / 60.0, fire);
            assert_eq!(out.cue, Some(WeaponCue::Fire));
            // fire_time 0.33 + rechamber 1.0: the rechamber cue fires on the
            // completion frame
            let cues: Vec<_> = (0..84)
                .filter_map(|_| w.update(1.0 / 60.0, WeaponInput::default()).cue)
                .collect();
            assert_eq!(cues, vec![WeaponCue::Rechamber]);
            step(&mut w, 0.05, WeaponInput::default());
        }

        let out = w.update(1.0 / 60.0, fire);
        assert_eq!(out.cue, Some(WeaponCue::LastShot));
        let cues: Vec<_> = (0..84)
            .filter_map(|_| w.update(1.0 / 60.0, WeaponInput::default()).cue)
            .collect();
        assert!(cues.is_empty(), "empty clip must not rechamber: {cues:?}");

        let reload = WeaponInput {
            reload: true,
            ..Default::default()
        };
        let out = w.update(1.0 / 60.0, reload);
        assert_eq!(out.cue, Some(WeaponCue::ReloadFromEmpty));
    }

    fn auto_def() -> WeaponDef {
        WeaponDef {
            semi_auto: false,
            bolt_action: false,
            fire_time: 0.12,
            ..def()
        }
    }

    #[test]
    fn auto_weapon_refires_while_held_at_fire_cadence() {
        let mut w = WeaponState::new(auto_def());
        step(&mut w, 0.6, WeaponInput::default());
        let press = WeaponInput {
            fire: true,
            fire_held: true,
            ..Default::default()
        };
        assert!(w.update(1.0 / 60.0, press).fired);
        let held = WeaponInput {
            fire_held: true,
            ..Default::default()
        };
        // 0.3 s at fire_time 0.12: exactly two more shots, no new edge needed
        let mut shots = 0;
        for _ in 0..18 {
            shots += w.update(1.0 / 60.0, held).fired as usize;
        }
        assert_eq!(shots, 2, "one shot per fireTime while held");
    }

    #[test]
    fn semi_weapon_needs_a_new_edge_per_shot() {
        let mut d = def();
        d.bolt_action = false;
        let mut w = WeaponState::new(d);
        step(&mut w, 0.6, WeaponInput::default());
        let press = WeaponInput {
            fire: true,
            fire_held: true,
            ..Default::default()
        };
        assert!(w.update(1.0 / 60.0, press).fired);
        let held = WeaponInput {
            fire_held: true,
            ..Default::default()
        };
        let mut shots = 0;
        for _ in 0..60 {
            shots += w.update(1.0 / 60.0, held).fired as usize;
        }
        assert_eq!(shots, 0, "holding a semi-auto trigger must not refire");
    }

    #[test]
    fn auto_starts_firing_when_the_button_is_already_down_after_raise() {
        let mut w = WeaponState::new(auto_def());
        let held = WeaponInput {
            fire_held: true,
            ..Default::default()
        };
        assert!(!w.update(1.0 / 60.0, held).fired, "still raising");
        let mut shots = 0;
        for _ in 0..40 {
            shots += w.update(1.0 / 60.0, held).fired as usize;
        }
        assert_eq!(shots, 2, "fires on raise settle, then once more by 0.66 s");
    }

    #[test]
    fn ammo_getter_reads_the_clip() {
        let mut w = WeaponState::new(auto_def());
        assert_eq!(w.ammo(), 5);
        step(&mut w, 0.6, WeaponInput::default());
        assert!(
            w.update(
                1.0 / 60.0,
                WeaponInput {
                    fire: true,
                    ..Default::default()
                }
            )
            .fired
        );
        assert_eq!(w.ammo(), 4);
    }

    #[test]
    fn raises_then_idles() {
        let mut w = WeaponState::new(def());
        assert_eq!(
            w.update(0.0, WeaponInput::default()).anim,
            WeaponAnim::Raise
        );
        let out = step(
            &mut w,
            0.1,
            WeaponInput {
                fire: true,
                ..Default::default()
            },
        );
        assert_eq!(out.anim, WeaponAnim::Raise);
        assert!(!out.fired);
        let out = step(&mut w, 0.5, WeaponInput::default());
        assert_eq!(out.anim, WeaponAnim::Idle);
        assert!(out.looping);
    }

    #[test]
    fn fire_rechamber_cycle() {
        let mut w = WeaponState::new(def());
        step(&mut w, 0.6, WeaponInput::default());
        let out = w.update(
            1.0 / 60.0,
            WeaponInput {
                fire: true,
                ..Default::default()
            },
        );
        assert_eq!(out.anim, WeaponAnim::Fire);
        assert!(out.fired);
        assert!(!w.update(1.0 / 60.0, WeaponInput::default()).fired);
        let out = step(
            &mut w,
            0.4,
            WeaponInput {
                fire: true,
                ..Default::default()
            },
        );
        assert_eq!(out.anim, WeaponAnim::Rechamber);
        assert!(!out.fired);
        let out = step(&mut w, 1.1, WeaponInput::default());
        assert_eq!(out.anim, WeaponAnim::Idle);
    }

    #[test]
    fn fifth_shot_is_lastshot_and_trigger_goes_dead() {
        let mut w = WeaponState::new(def());
        step(&mut w, 0.6, WeaponInput::default());
        for _ in 0..4 {
            assert!(
                w.update(
                    1.0 / 60.0,
                    WeaponInput {
                        fire: true,
                        ..Default::default()
                    }
                )
                .fired
            );
            step(&mut w, 1.5, WeaponInput::default()); // fire + rechamber
        }
        let out = w.update(
            1.0 / 60.0,
            WeaponInput {
                fire: true,
                ..Default::default()
            },
        );
        assert_eq!(out.anim, WeaponAnim::LastShot);
        // empty clip: no rechamber
        let out = step(&mut w, 0.4, WeaponInput::default());
        assert_eq!(out.anim, WeaponAnim::Idle);
        let out = w.update(
            1.0 / 60.0,
            WeaponInput {
                fire: true,
                ..Default::default()
            },
        );
        assert!(!out.fired);
        assert_eq!(out.anim, WeaponAnim::Idle);
    }

    #[test]
    fn reload_refills_and_restores_ads() {
        let mut w = WeaponState::new(def());
        step(&mut w, 0.6, WeaponInput::default());
        w.update(
            1.0 / 60.0,
            WeaponInput {
                fire: true,
                ..Default::default()
            },
        );
        step(&mut w, 1.5, WeaponInput::default());
        let ads_held = WeaponInput {
            ads: true,
            ..Default::default()
        };
        step(&mut w, 0.5, ads_held); // fully aimed
        let out = w.update(
            1.0 / 60.0,
            WeaponInput {
                reload: true,
                ads: true,
                ..Default::default()
            },
        );
        assert_eq!(out.anim, WeaponAnim::Reload);
        let out = step(&mut w, 1.0, ads_held);
        assert!(out.ads_frac < 0.05, "{}", out.ads_frac);
        assert!(
            !w.update(
                1.0 / 60.0,
                WeaponInput {
                    fire: true,
                    ads: true,
                    ..Default::default()
                }
            )
            .fired
        );
        let out = step(&mut w, 1.8, ads_held);
        assert_eq!(out.anim, WeaponAnim::AdsUp);
        let out = step(&mut w, 0.4, ads_held);
        assert!(out.ads_frac > 0.95);
        assert!(
            w.update(
                1.0 / 60.0,
                WeaponInput {
                    fire: true,
                    ads: true,
                    ..Default::default()
                }
            )
            .fired
        );
    }

    #[test]
    fn ads_tap_mirrors_fraction() {
        let mut w = WeaponState::new(def());
        step(&mut w, 0.6, WeaponInput::default());
        let ads = WeaponInput {
            ads: true,
            ..Default::default()
        };
        // half of ads_trans_in
        let half = step(&mut w, 0.15, ads);
        assert_eq!(half.anim, WeaponAnim::AdsUp);
        assert!(
            half.ads_frac > 0.3 && half.ads_frac < 0.7,
            "{}",
            half.ads_frac
        );
        let down = w.update(1.0 / 60.0, WeaponInput::default());
        assert_eq!(down.anim, WeaponAnim::AdsDown);
        assert!(down.anim_time > 0.1, "{}", down.anim_time);
        let out = step(&mut w, 0.5, WeaponInput::default());
        assert_eq!(out.anim, WeaponAnim::Idle);
        assert!(out.ads_frac < 0.05);
    }

    #[test]
    fn ads_fire_uses_ads_clips_and_holds_zoom() {
        let mut w = WeaponState::new(def());
        step(&mut w, 0.6, WeaponInput::default());
        let ads = WeaponInput {
            ads: true,
            ..Default::default()
        };
        step(&mut w, 0.5, ads);
        let out = w.update(
            1.0 / 60.0,
            WeaponInput {
                fire: true,
                ads: true,
                ..Default::default()
            },
        );
        assert_eq!(out.anim, WeaponAnim::AdsFire);
        assert!(out.fired);
        let out = step(&mut w, 0.7, ads);
        assert_eq!(out.anim, WeaponAnim::AdsRechamber);
        assert!(out.ads_frac > 0.95, "{}", out.ads_frac);
    }

    #[test]
    fn def_from_map_parses_and_defaults() {
        let map = xmodel::parse_weapon(
            r"\clipSize\5\fireTime\0.33\rechamberTime\1\reloadTime\2.5\raiseTime\0.5\adsTransInTime\0.3\adsTransOutTime\0.4\adsZoomFov\50\adsViewBobMult\0.2\boltAction\1",
        );
        let d = WeaponDef::from_map(&map);
        assert_eq!(d.clip_size, 5);
        assert!((d.fire_time - 0.33).abs() < 1e-6);
        assert!(d.bolt_action);
        let d = WeaponDef::from_map(&HashMap::new());
        assert_eq!(d.clip_size, 1);
        assert_eq!(d.fire_time, 0.0);
        assert!(!d.bolt_action);
        assert_eq!(d.world_model, None);
        assert_eq!(d.world_flash_effect, None);
        assert_eq!(d.kill_icon, None);
        assert!(!d.wide_kill_icon);
    }

    #[test]
    fn world_model_strips_xmodel_prefix() {
        let map = xmodel::parse_weapon(r"\worldModel\xmodel/weapon_kar98");
        let d = WeaponDef::from_map(&map);
        assert_eq!(d.world_model.as_deref(), Some("weapon_kar98"));
    }

    #[test]
    fn kill_icon_parses_as_is_and_empty_is_none() {
        let map = xmodel::parse_weapon(r"\killIcon\gfx/hud/hud@death_kar98.tga");
        let d = WeaponDef::from_map(&map);
        assert_eq!(d.kill_icon.as_deref(), Some("gfx/hud/hud@death_kar98.tga"));
        let map = xmodel::parse_weapon(r"\killIcon\");
        let d = WeaponDef::from_map(&map);
        assert_eq!(d.kill_icon, None);
    }

    #[test]
    fn wide_kill_icon_parses_as_a_weapon_file_bool() {
        let map = xmodel::parse_weapon(r"\killIcon\gfx/hud/hud@death_thompson.tga\wideKillIcon\1");
        let d = WeaponDef::from_map(&map);
        assert!(d.wide_kill_icon);
        let map = xmodel::parse_weapon(r"\killIcon\gfx/hud/hud@death_colt45.tga\wideKillIcon\0");
        let d = WeaponDef::from_map(&map);
        assert!(!d.wide_kill_icon);
        assert!(!WeaponDef::from_map(&HashMap::new()).wide_kill_icon);
    }

    #[test]
    fn real_weapon_files_carry_wide_kill_icon_flags() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let thompson = load(&fs, "thompson_mp").unwrap();
        assert!(thompson.wide_kill_icon);
        let colt = load(&fs, "colt_mp");
        if let Ok(colt) = colt {
            assert!(!colt.wide_kill_icon);
        }
    }

    #[test]
    fn world_flash_effect_parses_as_is_no_prefix_to_strip() {
        let map =
            xmodel::parse_weapon(r"\worldFlashEffect\fx/muzzleflashes/standardflashworld.efx");
        let d = WeaponDef::from_map(&map);
        assert_eq!(
            d.world_flash_effect.as_deref(),
            Some("fx/muzzleflashes/standardflashworld.efx")
        );
    }

    #[test]
    fn load_reads_weapon_file_by_cs7_name() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let d = load(&fs, "kar98k_mp").unwrap();
        let name = d.world_model.expect("kar98k_mp ships a worldModel");
        assert!(name.to_lowercase().contains("kar98"), "{name}");
        assert!(load(&fs, "not_a_real_weapon").is_err());
    }

    #[test]
    fn load_real_weapon_flash_effects_differ_by_weapon() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let kar98 = load(&fs, "kar98k_mp").unwrap();
        assert_eq!(
            kar98.world_flash_effect.as_deref(),
            Some("fx/muzzleflashes/standardflashworld.efx")
        );
        let thompson = load(&fs, "thompson_mp").unwrap();
        assert_eq!(
            thompson.world_flash_effect.as_deref(),
            Some("fx/muzzleflashes/thompson.efx")
        );
    }

    #[test]
    fn loads_real_kar98k_world_model() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let text = fs.read("weapons/mp/kar98k_mp").unwrap();
        let map = xmodel::parse_weapon(&String::from_utf8_lossy(&text));
        let d = WeaponDef::from_map(&map);
        let name = d.world_model.expect("kar98k_mp ships a worldModel");
        assert!(name.to_lowercase().contains("kar98"), "{name}");
    }
}
