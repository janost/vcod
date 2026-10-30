//! CoD4 1.7's shellshock on the client (docs/research/cod4-client-view.md
//! 8): the drawn snapshot's `shellshockIndex`, `shellshockTime` and
//! `shellshockDuration` name a configstring (1954 + the index) whose
//! `shock/<name>.shock` record (`vcod_cod4::shock`) drives the frame as
//! `CG_UpdateShellShock` and the shellshock blend do:
//! the screen blend (a
//! flashbang's white-out over a still of its first frame, a blur's last
//! frame over this one), the camera's kick, the mouse's caps and
//! sensitivity, and the sound mix with its tinnitus loop and end.

use vcod_cod4::shock::{ScreenType, ShockParms};

/// The screen half of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Screen {
    /// No blend; the saved still goes.
    Off,
    /// `RB_BlendSavedScreenBlurredCmd`: the last capture over this frame
    /// at `0.01 ^ (age / decay)`, then this frame captured.
    Blur { decay_ms: i32 },
    /// The first frame of a flash: captured, nothing drawn.
    Save,
    /// `RB_BlendSavedScreenFlashedCmd`: the still times `grab` plus
    /// `white`, added to the frame (`shell_shock_flashed`, blend One One);
    /// both the bytes the vertex colour carries.
    Flash { white: f32, grab: f32 },
}

/// The mouse half: the sensitivity's scale and the turn caps, degrees a
/// second (0 for none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub sensitivity: f32,
    pub max_pitch: f32,
    pub max_yaw: f32,
}

impl Look {
    pub const NONE: Look = Look {
        sensitivity: 1.0,
        max_pitch: 0.0,
        max_yaw: 0.0,
    };
}

/// The sound half: how far into the shock's mix (`k`, 0 none to 1 full:
/// the engine's fade toward `SND_SetChannelVolumes` and
/// `SND_SetEnvironmentEffects`), the record it mixes, the blended loop and
/// a one-shot.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    /// By `audio::alias::Channel` index, each `1 + (v - 1) * k`.
    pub volumes: [f32; 11],
    pub room: String,
    pub dry: f32,
    pub wet: f32,
    /// `SND_PlayBlendedSoundAliases(loop, loopSilent, fade)`.
    pub loop_blend: Option<(String, String, f32)>,
    /// A one-shot this frame and how far into it, ms.
    pub cue: Option<(String, i32)>,
}

impl Sound {
    fn idle(cue: Option<(String, i32)>) -> Sound {
        Sound {
            volumes: [1.0; 11],
            room: "generic".into(),
            dry: 1.0,
            wet: 0.0,
            loop_blend: None,
            cue,
        }
    }
}

/// One frame's shellshock.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub screen: Screen,
    /// `cg.shellshock.viewDelta`: the forward axis's turn toward the left
    /// and up axes (`CG_PerturbCamera`).
    pub kick: [f32; 2],
    pub look: Look,
    pub sound: Sound,
}

impl Frame {
    pub fn rest() -> Frame {
        Frame {
            screen: Screen::Off,
            kick: [0.0, 0.0],
            look: Look::NONE,
            sound: Sound::idle(None),
        }
    }
}

/// vcod's channels (`audio::alias::Channel`'s order) by the CoD4
/// `channels.def` name whose `bg_shock_volume_*` each takes.
const CHANNELS: [&str; 11] = [
    "auto",
    "menu",
    "weapon",
    "voice",
    "item",
    "body",
    "local",
    "music",
    "announcer",
    "shellshock",
    "auto2d",
];

/// The newest snapshot's shock: the record its `shellshockIndex` names
/// (configstring 1954 + the index) out of the session's world, its
/// `shellshockTime` and `shellshockDuration`.
pub(crate) fn of_session(s: &super::cod4::Cod4Session) -> (Option<ShockParms>, i32, i32) {
    let snap = s.client().snapshots.last();
    let u = |n: &str| snap.and_then(|s| s.ps.get(n)).unwrap_or(0);
    let (index, start, duration) = (
        u("shellshockIndex"),
        u("shellshockTime") as i32,
        u("shellshockDuration") as i32,
    );
    let name = match (index, s.gamestate()) {
        (1..16, Some(gs)) => {
            String::from_utf8_lossy(gs.configstring(1954 + index as u16)).into_owned()
        }
        _ => String::new(),
    };
    let parms = (!name.is_empty())
        .then(|| s.world().and_then(|w| w.shock(&name).cloned()))
        .flatten();
    (parms, start, duration)
}

/// `CG_Flashbanged` (`iw3mp.exe` 0x42c6f3, inlined in
/// `CG_DrawDamageDirectionIndicators`): a shock still running at `now`
/// whose record blends the screen (cod4-hud.md 19).
pub(crate) fn flashbanged(parms: Option<&ShockParms>, start: i32, duration: i32, now: i32) -> bool {
    start - now + duration > 0 && parms.is_some_and(|p| p.screen != ScreenType::None)
}

/// What `cg.shellshock` carries from frame to frame.
#[derive(Default)]
pub struct Cod4Shock {
    /// A flash still has been captured for the running shock.
    still: bool,
    /// The end time whose end sound has already played.
    end_played_at: Option<i32>,
}

/// Where a frame sits in a shock: client time `now` against a shock that
/// started at `start` and runs `duration` ms.
#[derive(Clone, Copy)]
struct Span {
    start: i32,
    duration: i32,
    now: i32,
}

impl Span {
    fn elapsed(self) -> i32 {
        self.now - self.start
    }

    /// Ms to the shock's end (negative past it).
    fn left(self) -> i32 {
        self.start + self.duration - self.now
    }
}

/// A linear ramp over `span` ms: 0 at `ms <= 0`, 1 at `ms >= span`. Every
/// fade in the shock (screen, mix, loop, mouse) is one of these, read off
/// the time left or, for the mix's fade-in, the time spent.
fn ramp(ms: i32, span: i32) -> f32 {
    if ms <= 0 {
        0.0
    } else if ms >= span {
        1.0
    } else {
        ms as f32 / span as f32
    }
}

/// `value` reached `share` of the way from 1: how the mix scales a channel
/// and the mouse eases its sensitivity back.
fn toward(value: f32, share: f32) -> f32 {
    value * share + (1.0 - share)
}

/// `BlendSmooth` (`iw3mp.exe` 0x44c9a0): a sine ease of `p` in 0..1.
fn ease(p: f32) -> f32 {
    0.5 + 0.5 * (std::f32::consts::PI * (p - 0.5)).sin()
}

/// A 0..1 factor through the vertex colour's byte (`f * 255` rounded).
fn colour_byte(f: f32) -> f32 {
    (255.0 * f).round_ties_even().clamp(0.0, 255.0) / 255.0
}

/// The generic abort `iw3mp.exe` 0x44cb30 plays when a shock's sound is
/// dropped after its end sound played.
const GENERIC_ABORT: &str = "shellshock_end_abort";

impl Cod4Shock {
    /// The frame at client time `now` for a shock of `parms` that started
    /// at `start` and lasts `duration` ms (`start` 0, or no record: none).
    /// A shock runs from a nonzero start once `now` reaches it (the gate
    /// at `iw3mp.exe` 0x44d020); otherwise the shock ends, its still and
    /// any end sound dropped.
    pub fn frame(
        &mut self,
        parms: Option<&ShockParms>,
        start: i32,
        duration: i32,
        now: i32,
    ) -> Frame {
        let span = Span {
            start,
            duration,
            now,
        };
        match parms {
            Some(p) if start != 0 && span.elapsed() >= 0 => Frame {
                screen: self.blend(p, span),
                kick: kick(p, span),
                look: look(p, span),
                sound: self.mix(p, span),
            },
            _ => {
                self.still = false;
                Frame {
                    sound: Sound::idle(self.drop_end()),
                    ..Frame::rest()
                }
            }
        }
    }

    /// The screen blend (cod4-client-view.md 8; `iw3mp.exe` 0x44ca00
    /// flashed, 0x44c8d0 blurred). Nothing draws once the shock is over or
    /// for a record with no blend, and the still goes with it.
    fn blend(&mut self, p: &ShockParms, span: Span) -> Screen {
        let left = span.left();
        if span.duration <= 0 || left <= 0 || p.screen == ScreenType::None {
            self.still = false;
            return Screen::Off;
        }
        if p.screen == ScreenType::Blurred {
            // Every frame is captured, the first included, so the blur
            // has a capture from the next frame on.
            self.still = true;
            let share = ramp(left, p.blur_fade_ms);
            return Screen::Blur {
                decay_ms: (share * p.blur_effect_ms as f32).round_ties_even() as i32,
            };
        }
        if !std::mem::replace(&mut self.still, true) {
            return Screen::Save;
        }
        let fade = |span_ms| colour_byte(ease(ramp(left, span_ms)));
        Screen::Flash {
            white: fade(p.flash_white_fade_ms),
            grab: fade(p.flash_shot_fade_ms),
        }
    }

    /// The sound mix, the loop and its end (cod4-client-view.md 8;
    /// `iw3mp.exe` 0x44cbc0).
    fn mix(&mut self, p: &ShockParms, span: Span) -> Sound {
        if !p.sound {
            return Sound::idle(self.drop_end());
        }
        let share = if span.elapsed() < p.fade_in_ms {
            ramp(span.elapsed(), p.fade_in_ms)
        } else {
            // Fades out over `soundFadeOutTime`, ending `soundModEndDelay`
            // past the shock.
            ramp(
                span.left() + p.mod_end_delay_ms + p.fade_out_ms,
                p.fade_out_ms,
            )
        };
        // The loop runs to `soundLoopEndDelay` past the shock plus its
        // fade, blending toward the silent twin over that fade.
        let loop_left = span.left() + p.loop_end_delay_ms + p.loop_fade_ms;
        let loop_blend = (loop_left > 0).then(|| {
            (
                p.sound_loop.clone(),
                p.sound_loop_silent.clone(),
                1.0 - ramp(loop_left, p.loop_fade_ms),
            )
        });
        let end_at = span.start + span.duration + p.loop_end_delay_ms;
        let cue = if span.now < end_at {
            // A shock restarted or stretched before its end: the end
            // already played is cut off.
            self.end_played_at
                .take()
                .map(|_| (p.sound_end_abort.clone(), 0))
        } else if self.end_played_at.replace(end_at) != Some(end_at) {
            Some((p.sound_end.clone(), span.now - end_at))
        } else {
            None
        };
        Sound {
            volumes: CHANNELS.map(|c| toward(p.volume(c), share)),
            room: p.room.clone(),
            dry: toward(p.dry, share),
            wet: p.wet * share,
            loop_blend,
            cue,
        }
    }

    /// Forgets a played end sound, cutting it off with the generic abort.
    fn drop_end(&mut self) -> Option<(String, i32)> {
        self.end_played_at
            .take()
            .map(|_| (GENERIC_ABORT.to_string(), 0))
    }
}

/// The camera (`iw3mp.exe` 0x44cf10): 1.1's kick over the same table, its
/// radius eased out over the last 3 s.
fn kick(p: &ShockParms, span: Span) -> [f32; 2] {
    let k = crate::play::shock::ShockParams {
        kick_rate: p.kick_rate,
        kick_radius: p.kick_radius,
        ..crate::play::shock::ShockParams::defaults()
    };
    crate::play::shock::view_kick(&k, span.elapsed(), span.duration)
}

/// The mouse (`iw3mp.exe` 0x44ce30): the record's sensitivity scale and
/// turn caps while `lookControl_fadeTime` or more is left, each eased
/// back to none over that time.
fn look(p: &ShockParms, span: Span) -> Look {
    let left = span.left();
    if !p.look || left <= 0 {
        return Look::NONE;
    }
    let share = ramp(left, p.look_fade_ms);
    Look {
        sensitivity: toward(p.sensitivity, share),
        max_pitch: p.max_pitch_speed / share,
        max_yaw: p.max_yaw_speed / share,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flash() -> ShockParms {
        ShockParms::from_text(
            "bg_shock_screenType \"flashed\"\n\
             bg_shock_screenFlashWhiteFadeTime \"3.5\"\n\
             bg_shock_screenFlashShotFadeTime \"1.0\"\n\
             bg_shock_soundLoop \"flashbang_tinnitus_loop\"\n\
             bg_shock_soundLoopSilent \"flashbang_tinnitus_loop_silent\"\n\
             bg_shock_soundEnd \"flashbang_tinnitus_end\"\n\
             bg_shock_soundEndAbort \"flashbang_tinnitus_abort\"\n\
             bg_shock_soundFadeInTime \".25\"\n\
             bg_shock_soundFadeOutTime \"2.0\"\n\
             bg_shock_soundModEndDelay \"1.5\"\n\
             bg_shock_soundLoopFadeTime \"2\"\n\
             bg_shock_soundLoopEndDelay \"2.0\"\n\
             bg_shock_volume_weapon \"0.1\"\n\
             bg_shock_lookControl \"0\"",
        )
    }

    /// A 6 s flashbang: the first frame saves the still, then the screen
    /// is white with the still at full until 3.5 s before the end, the
    /// white eases out over those 3.5 s and the still over the last one;
    /// past the end nothing draws.
    #[test]
    fn a_flashbang_whites_out_then_fades_to_the_still() {
        let p = flash();
        let mut s = Cod4Shock::default();
        assert_eq!(s.frame(Some(&p), 1000, 6000, 1000).screen, Screen::Save);
        assert_eq!(
            s.frame(Some(&p), 1000, 6000, 1050).screen,
            Screen::Flash {
                white: 1.0,
                grab: 1.0
            }
        );
        let Screen::Flash { white, grab } = s.frame(Some(&p), 1000, 6000, 5250).screen else {
            panic!()
        };
        // 1.75 s of the 3.5 left: half way down the white's ease.
        assert_eq!(white, colour_byte(0.5));
        assert_eq!(grab, 1.0);
        let Screen::Flash { grab, .. } = s.frame(Some(&p), 1000, 6000, 6500).screen else {
            panic!()
        };
        assert_eq!(grab, colour_byte(ease(0.5)));
        assert_eq!(s.frame(Some(&p), 1000, 6000, 7000).screen, Screen::Off);
        // A new shock saves a new still.
        assert_eq!(s.frame(Some(&p), 8000, 2000, 8000).screen, Screen::Save);
    }

    /// The flashbang's tinnitus loops from the start; 2 s past the shock
    /// (`soundLoopEndDelay`) its end plays once while the loop blends
    /// toward its silent twin over 2 s more. The weapon channel sits at a
    /// tenth while the mix is in, fading in over 250 ms.
    #[test]
    fn the_flashbangs_tinnitus_loops_then_ends() {
        let p = flash();
        let mut s = Cod4Shock::default();
        let f = s.frame(Some(&p), 1000, 6000, 1000);
        assert_eq!(f.sound.volumes[2], 1.0);
        let l = f.sound.loop_blend.clone().unwrap();
        assert_eq!((l.0.as_str(), l.2), ("flashbang_tinnitus_loop", 0.0));
        let f = s.frame(Some(&p), 1000, 6000, 2000);
        assert!((f.sound.volumes[2] - 0.1).abs() < 1e-6);
        assert_eq!(s.frame(Some(&p), 1000, 6000, 8000).sound.cue, None);
        let f = s.frame(Some(&p), 1000, 6000, 9050);
        assert!(f.sound.loop_blend.is_some());
        assert_eq!(f.sound.cue, Some(("flashbang_tinnitus_end".into(), 50)));
        // 1 s into the loop's 2 s fade.
        let f = s.frame(Some(&p), 1000, 6000, 10_000);
        assert_eq!(f.sound.loop_blend.unwrap().2, 0.5);
        assert_eq!(f.sound.cue, None);
        assert_eq!(s.frame(Some(&p), 1000, 6000, 11_050).sound.loop_blend, None);
        // The look is the record's to leave alone; the kick runs.
        assert_eq!(f.look, Look::NONE);
        assert_ne!(s.frame(Some(&p), 1000, 6000, 2000).kick, [0.0, 0.0]);
    }

    /// The concussion's record caps the turn at 35 a second and scales the
    /// mouse by a tenth, both easing back over its last 2 s.
    #[test]
    fn the_concussion_caps_the_turn() {
        let p = ShockParms::from_text(
            "bg_shock_lookControl \"1\"\n\
             bg_shock_lookControl_maxpitchspeed \"35\"\n\
             bg_shock_lookControl_maxyawspeed \"35\"\n\
             bg_shock_lookControl_mousesensitivityscale \"0.1\"\n\
             bg_shock_lookControl_fadeTime \"2\"",
        );
        let mut s = Cod4Shock::default();
        let f = s.frame(Some(&p), 1000, 4000, 2000);
        assert_eq!(
            f.look,
            Look {
                sensitivity: 0.1,
                max_pitch: 35.0,
                max_yaw: 35.0
            }
        );
        let f = s.frame(Some(&p), 1000, 4000, 4000);
        assert!((f.look.sensitivity - 0.55).abs() < 1e-6);
        assert_eq!(f.look.max_yaw, 70.0);
        assert_eq!(s.frame(Some(&p), 1000, 4000, 5000).look, Look::NONE);
    }
}

/// The stock records and the aliases they name, in the zones (`COD4_DIR`;
/// vacuous without it): `flashbang` whites out and leaves the walk alone,
/// `concussion_grenade_mp` slows it and caps the turn.
#[cfg(test)]
#[test]
fn the_zones_hold_the_flashbang_and_concussion_records() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let w = crate::cod4_fly::load_world(install.root(), "mp_crash").unwrap();
    let flash = w.shock("flashbang").unwrap();
    assert_eq!(flash.screen, ScreenType::Flashed);
    assert!(!flash.movement && !flash.look);
    let conc = w.shock("concussion_grenade_mp").unwrap();
    assert!(conc.movement && conc.look);
    let ((sounds, _), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
    for a in [&flash.sound_loop, &flash.sound_end, &conc.sound_loop] {
        assert!(sounds.has(a), "{a}");
    }
}
