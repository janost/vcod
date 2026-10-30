//! What a CoD2 session's drawn view adds over the predicted eye and angles
//! (docs/research/cod2-hud.md 16-18): `CG_CalcViewValues` (Mac 0x1d3b28)
//! runs `BG_CalculateViewAngles` (0x8033a, the damage kick and the sight's
//! idle sway and bob), then `CG_ShakeCamera` (0x1cea88), then
//! `CG_PerturbCamera` (0x1d6c10, the shellshock kick), then `CG_CalcFov`
//! (0x1d351e, the sight's zoom). The shake is 1.1's (`play::quake`), the
//! kick 1.1's (`play::shock`).

use std::collections::HashMap;

use glam::Vec3;
use vcod_cod2::net::Frame;
use vcod_cod2::net::events::{EV_EARTHQUAKE, EV_FIRE_WEAPON_MG42};
use vcod_cod2::weapon::{self, WeaponDef};
use vcod_common::pk3::Pk3Fs;

use crate::play::cod2_events::Event2;
use crate::play::cod2_gun::{GunInput, GunKeys, GunPose, GunPs, GunState};
use crate::play::cod2_shock;
use crate::play::quake::{Quake, Quakes};
use crate::play::shock::{ShellShock, ShockFrame, kicked_angles};

/// `pm_flags` 0x10: the offhand is up, its weapon the view's.
const PMF_OFFHAND: i32 = 0x10;

/// `pm_type` 5, intermission: a 90 degree view.
const PM_INTERMISSION: i32 = 5;
/// `eFlags` 0x300: on a turret, a 55 degree view.
const EF_TURRET: i32 = 0x300;
/// `eFlags` 4 and 8: crouched and prone, for the scope's idle factor.
const EF_CROUCH: i32 = 0x4;
const EF_PRONE: i32 = 0x8;
/// `viewHeightTarget` of a prone and a crouched body.
const VIEW_PRONE: i32 = 11;
const VIEW_CROUCH: i32 = 40;
/// `bg_bobAmplitudeStanding`, `Ducked`, `Prone` (cod2-movement.md 13).
const BOB_AMPLITUDE: [f32; 3] = [0.007, 0.0075, 0.03];

/// What the view reads of the weapon `BG_GetViewmodelWeaponIndex` names.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SightDef {
    /// `aimDownSight` (0x32c), `BG_IsAimDownSightWeapon`.
    pub ads: bool,
    /// `adsOverlayShader` set or `adsOverlayReticle` not `none`: the
    /// reticle draws and the gun hides (`CG_GetWeapReticleZoom`).
    pub overlay: bool,
    /// `adsOverlayReticle` (0x278) not `none`: a scope's view terms.
    pub reticle: bool,
    pub zoom_fov: f32,
    pub zoom_in_frac: f32,
    pub zoom_out_frac: f32,
    pub ads_idle_amount: f32,
    pub hip_idle_amount: f32,
    pub ads_idle_speed: f32,
    pub hip_idle_speed: f32,
    pub idle_crouch: f32,
    pub idle_prone: f32,
    pub ads_bob_factor: f32,
    pub ads_view_bob_mult: f32,
}

impl SightDef {
    pub fn of(d: &WeaponDef) -> SightDef {
        let reticle = d.int("adsOverlayReticle") != 0;
        SightDef {
            ads: d.flag("aimDownSight"),
            overlay: !d.str("adsOverlayShader").is_empty() || reticle,
            reticle,
            zoom_fov: d.float("adsZoomFov"),
            zoom_in_frac: d.float("adsZoomInFrac"),
            zoom_out_frac: d.float("adsZoomOutFrac"),
            ads_idle_amount: d.float("adsIdleAmount"),
            hip_idle_amount: d.float("hipIdleAmount"),
            ads_idle_speed: d.float("adsIdleSpeed"),
            hip_idle_speed: d.float("hipIdleSpeed"),
            idle_crouch: d.float("idleCrouchFactor"),
            idle_prone: d.float("idleProneFactor"),
            ads_bob_factor: d.float("adsBobFactor"),
            ads_view_bob_mult: d.float("adsViewBobMult"),
        }
    }

    /// The zoom's fraction (`CG_GetViewFov`, `CG_GetWeapReticleZoom`):
    /// `fWeaponPosFrac` past `1 - frac` over `frac`, the in fraction while
    /// the sight comes up (`cg + 0x28494` set), the out one going down.
    /// Unscaled when not past.
    pub fn zoom(&self, pos: f32, zooming_in: bool) -> f32 {
        let frac = if zooming_in {
            self.zoom_in_frac
        } else {
            self.zoom_out_frac
        };
        let f = pos - (1.0 - frac);
        if f > 0.0 { f / frac } else { f }
    }

    /// `CG_GetWeapReticleZoom` (Mac 0x1d0f8a): the overlay's alpha, `None`
    /// at 0.01 or under (no overlay, gun drawn).
    pub fn overlay_alpha(&self, pos: f32, zooming_in: bool) -> Option<f32> {
        if !self.overlay || pos == 0.0 {
            return None;
        }
        let f = self.zoom(pos, zooming_in);
        (f > 0.01).then(|| f.min(1.0))
    }
}

/// `cg + 0x28494`, set by `CG_AddViewWeapon` (Mac 0x1da2f8) on an ADS
/// weapon: cleared once the sight is all the way up, set once all the way
/// down. Picks the zoom's in or out fraction.
#[derive(Clone, Copy, Debug, Default)]
pub struct SightLatch {
    pub zooming_in: bool,
}

impl SightLatch {
    pub fn step(&mut self, ads: bool, pos: f32) {
        if !ads {
            return;
        }
        if pos == 1.0 {
            self.zooming_in = false;
        } else if pos == 0.0 {
            self.zooming_in = true;
        }
    }
}

/// `CG_GetViewFov` (Mac 0x1d33ca): `cg_fov` closing on `adsZoomFov` past
/// the zoom fraction, 90 at intermission, 55 on a turret, times
/// `cg_fovScale` (1) and at least `cg_fovMin` (10).
pub fn view_fov(
    cg_fov: f32,
    pm_type: i32,
    eflags: i32,
    def: Option<&SightDef>,
    pos: f32,
    latch: SightLatch,
) -> f32 {
    let mut fov = 90.0;
    if pm_type != PM_INTERMISSION {
        fov = cg_fov;
        if let Some(d) = def.filter(|d| d.ads) {
            if pos == 1.0 {
                fov = d.zoom_fov;
            } else if pos != 0.0 {
                let f = d.zoom(pos, latch.zooming_in);
                if f > 0.0 {
                    fov = cg_fov - (cg_fov - d.zoom_fov) * f;
                }
            }
        }
    }
    if eflags & EF_TURRET != 0 {
        fov = 55.0;
    }
    fov.max(10.0)
}

/// `CG_DamageFeedback`'s view kick (Mac 0x1e3e10): `cg + 0x2bed4` (the
/// snapshot's time), `0x2bf10` and `0x2bf14` (pitch and roll, degrees) and
/// `0x2bf0c` (the red flash's end).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageKick {
    pub time: i32,
    pub pitch: f32,
    pub roll: f32,
    pub flash_until: i32,
}

impl DamageKick {
    /// A new `damageEvent` with `damageYaw`, `damagePitch` and
    /// `damageCount`, on the view axes `[forward, left, up]` as last drawn:
    /// `count * 0.2` clamped to 5..90, along the hit's forward and left
    /// parts; both angles 255 kick the pitch alone.
    pub fn feedback(
        yaw: i32,
        pitch: i32,
        count: i32,
        axis: [Vec3; 3],
        snap_time: i32,
        now: i32,
    ) -> DamageKick {
        let k = count as f32 * 0.2;
        let (front, side) = if k < 5.0 {
            (5.0, -5.0)
        } else if k > 90.0 {
            (90.0, -90.0)
        } else {
            (k, -k)
        };
        let (p, r) = if (yaw, pitch) == (255, 255) {
            (side, 0.0)
        } else {
            let angles = Vec3::new(
                pitch as f32 / 255.0 * 360.0,
                yaw as f32 / 255.0 * 360.0,
                0.0,
            );
            let [dir, _, _] = vcod_common::x87::angle_vectors(angles);
            (dir.dot(axis[0]) * front, dir.dot(axis[1]) * side)
        };
        DamageKick {
            time: snap_time,
            pitch: p,
            roll: r,
            flash_until: now + 500,
        }
    }

    /// `CG_Draw2D`'s red flash (Mac 0x1d114a): `(0.2, 0, 0)` over the whole
    /// screen at `0.7 * min(|left * pitch| / 500 / 5, 1)`, `left` the ms to
    /// its end.
    pub fn flash(&self, now: i32) -> Option<[f32; 4]> {
        if now >= self.flash_until {
            return None;
        }
        let f = ((self.flash_until - now) as f32 * self.pitch / 500.0).abs();
        let a = if f <= 5.0 { f / 5.0 } else { 1.0 };
        Some([0.2, 0.0, 0.0, a * 0.7])
    }
}

/// The change a landing of the own body sets (`CG_EntityEvent`, Mac
/// 0x1e4650, cg+0x284d8): `EV_LANDING_*` sinks by its parm,
/// `EV_LANDING_PAIN_*` by the fall height its damage percent stands for
/// between `bg_fallDamageMinHeight` and `MaxHeight` (`fall`), as `(h - 12)
/// / 26 * 4 + 4` truncated and capped at 24, nothing at or under 12 units
/// or under 1.
pub fn land_change(event: i32, parm: i32, fall: (f32, f32)) -> Option<f32> {
    use vcod_cod2::net::events::{EV_LANDING, EV_LANDING_PAIN, SURFACE_EVENTS};
    let group = |base: i32| (base..base + SURFACE_EVENTS).contains(&event);
    if group(EV_LANDING) {
        return Some(0.0 - parm as f32);
    }
    if !group(EV_LANDING_PAIN) {
        return None;
    }
    let (min, max) = fall;
    let h = parm as f32 * 0.01 * (max - min) + min;
    if h <= 12.0 {
        return None;
    }
    let n = ((h - 12.0) / 26.0 * 4.0 + 4.0) as i32;
    match n {
        ..1 => None,
        1..25 => Some(0.0 - n as f32),
        _ => Some(-24.0),
    }
}

/// The own body's last landing: `cg + 0x284dc` (its cg.time) and
/// `0x284d8` (the change, negative down).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Land {
    pub time_ms: i32,
    pub change: f32,
}

impl Land {
    pub fn land(&mut self, change: f32, now: i32) {
        *self = Land {
            time_ms: now,
            change,
        };
    }

    /// `CG_CalcViewValues`' dip of the eye (Mac 0x1d3b28): down over 150
    /// ms, back over 300. A landing ahead of the clock is moved 450 ms back,
    /// that frame still dipping by its negative time.
    pub fn eye(&mut self, now: i32) -> f32 {
        let dt = now.wrapping_sub(self.time_ms) as f32;
        if dt < 0.0 {
            self.time_ms = now - 450;
        }
        if dt < 150.0 {
            dt / 150.0 * self.change
        } else if dt < 450.0 {
            ((dt - 150.0) / -300.0 + 1.0) * self.change
        } else {
            0.0
        }
    }

    /// `CG_AddViewWeapon`'s further drop of the gun (Mac 0x1da2f8), on the
    /// world z of its origin: a quarter of the change, over the same 150
    /// and 300 ms in whole milliseconds.
    pub fn gun(&self, now: i32) -> f32 {
        let dt = now.wrapping_sub(self.time_ms);
        if dt < 150 {
            0.25 * self.change * dt as f32 / 150.0
        } else if dt < 450 {
            0.25 * self.change * (450 - dt) as f32 / 300.0
        } else {
            0.0
        }
    }
}

/// `GetLeanFraction` (Mac 0x28ec4).
fn lean_fraction(f: f32) -> f32 {
    (2.0 - f.abs()) * f
}

/// The playerstate fields `BG_CalculateViewAngles` reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewPs {
    pub pos_frac: f32,
    pub eflags: i32,
    pub view_height_target: i32,
    /// `bobCycle`'s low byte.
    pub bob_cycle: u8,
    pub hold_breath_scale: f32,
    /// `BG_GetSpeed`: the flat speed.
    pub xyspeed: f32,
}

/// The idle sway's carried state: `cg + 0x284a4` (the stance factor, which
/// only the gun's pose eases for keeps) and `cg + 0x2beec` (the idle clock,
/// which the view and the gun both run).
#[derive(Clone, Copy, Debug)]
pub struct IdleState {
    pub factor: f32,
    pub time: i32,
}

impl Default for IdleState {
    fn default() -> Self {
        IdleState {
            factor: 1.0,
            time: 0,
        }
    }
}

fn bob_amplitude(ps: &ViewPs) -> f32 {
    match ps.view_height_target {
        VIEW_PRONE => BOB_AMPLITUDE[2],
        VIEW_CROUCH => BOB_AMPLITUDE[1],
        _ => BOB_AMPLITUDE[0],
    }
}

/// `BG_CalculateViewAngles` (Mac 0x8033a): what the view's angles take
/// (pitch, yaw, roll degrees) at `now` (cg.time), `msec` the frame. The
/// damage kick, 100 ms up and 400 down, halved down the sight but less so
/// on a scope; on a scope (`adsOverlayReticle`) the idle sway and the bob
/// by the sight fraction; the sight's own bob by `adsViewBobMult` off a
/// turret.
pub fn view_angles(
    ps: &ViewPs,
    def: Option<&SightDef>,
    kick: &DamageKick,
    idle: &mut IdleState,
    now: i32,
    msec: i32,
) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    let d = def.copied().unwrap_or_default();
    let pos = ps.pos_frac;
    if kick.time != 0 {
        let mut f = pos * -0.5 + 1.0;
        if pos != 0.0 && d.reticle {
            f *= pos * 0.5 + 1.0;
        }
        let dt = (now - kick.time) as f32;
        let g = if dt < 100.0 {
            Some(lean_fraction(dt / 100.0))
        } else {
            let r = (dt - 100.0) / -400.0 + 1.0;
            (r > 0.0).then(|| 1.0 - lean_fraction(1.0 - r))
        };
        if let Some(g) = g {
            out[0] += g * f * kick.pitch;
            out[2] += g * f * kick.roll;
        }
    }
    let frametime = msec as f32 * 0.001;
    if d.reticle {
        let (speed, amount) = if !d.ads {
            if d.hip_idle_amount != 0.0 {
                (d.hip_idle_speed, d.hip_idle_amount)
            } else {
                (1.0, 80.0)
            }
        } else {
            (
                (d.ads_idle_speed - d.hip_idle_speed) * pos + d.hip_idle_speed,
                (d.ads_idle_amount - d.hip_idle_amount) * pos + d.hip_idle_amount,
            )
        };
        let target = if ps.eflags & EF_PRONE != 0 {
            d.idle_prone
        } else if ps.eflags & EF_CROUCH != 0 {
            d.idle_crouch
        } else {
            1.0
        };
        // Eased on a copy: the gun's pose keeps the factor (`cg + 0x284a4`).
        let mut factor = idle.factor;
        if pos != 0.0 && target != factor {
            factor = if target <= factor {
                target.max(factor - 0.5 * frametime)
            } else {
                target.min(factor + 0.5 * frametime)
            };
        }
        let k = amount * factor * pos * ps.hold_breath_scale;
        idle.time = idle
            .time
            .wrapping_add((speed * ps.hold_breath_scale * frametime * 1000.0) as i32);
        let t = idle.time as f32;
        out[1] += (t * 0.0007).sin() * k * 0.01;
        out[0] += k * (t * 0.001).sin() * 0.01;

        // The scope's bob: a quarter turn ahead of the eye's.
        let turn = f64::from(f32::from(ps.bob_cycle) / 255.0) * std::f64::consts::PI;
        let c = ((turn + turn + std::f64::consts::TAU) as f32) as f64;
        let c = (c + std::f64::consts::FRAC_PI_4 + std::f64::consts::TAU) as f32;
        let base = 0.16 * ps.xyspeed;
        let amp = bob_amplitude(ps);
        let pitch_amp = (base * amp).min(10.0);
        let mut bp = -(((c + c).sin()
            + ((f64::from(c * 4.0) + std::f64::consts::FRAC_PI_2).sin() as f32) * 0.2)
            * 0.75
            * pitch_amp);
        let mut by = -(pitch_amp * c.sin());
        let roll_amp = (base * 1.5 * amp).min(10.0);
        let r = ((f64::from(c) - 0.471_238_916_763_820_4) as f32).sin() * roll_amp;
        let mut br = if r <= 0.0 { r } else { 0.0 };
        if pos != 0.0 {
            let m = 1.0 - (1.0 - d.ads_bob_factor) * pos;
            bp *= m;
            by *= m;
            br *= m;
        }
        out[0] += bp * pos;
        out[1] += by * pos;
        out[2] += pos * br;
    }
    if ps.eflags & EF_TURRET == 0 && pos != 0.0 && d.ads_view_bob_mult != 0.0 {
        let turn = f64::from(f32::from(ps.bob_cycle) / 255.0) * std::f64::consts::PI;
        let c = (turn + turn + std::f64::consts::TAU) as f32;
        let a = (ps.xyspeed * bob_amplitude(ps)).min(45.0);
        let k = pos * d.ads_view_bob_mult;
        out[0] -= a
            * ((c + c).sin()
                + ((f64::from(c * 4.0) + std::f64::consts::FRAC_PI_2).sin() as f32) * 0.2)
            * 0.75
            * k;
        out[1] -= a * c.sin() * k;
    }
    out
}

/// The shake an event starts (`CG_EntityEvent`, Mac 0x1e4650): every
/// `EV_FIRE_WEAPON_MG42` a fixed 0.05 for 100 ms within 100 units;
/// `EV_EARTHQUAKE` its `angles2[0]` scale, `time` ms and `angles2[1]`
/// radius, as `GScr_Earthquake` (lnxded 0x81161ba) writes them.
pub fn quake_of(ev: &Event2) -> Option<Quake> {
    let (scale, duration_ms, radius) = match ev.event {
        EV_FIRE_WEAPON_MG42 => (0.05, 100, 100.0),
        EV_EARTHQUAKE => (ev.angles2[0], ev.time, ev.angles2[1]),
        _ => return None,
    };
    Some(Quake {
        start_ms: 0,
        scale,
        duration_ms: duration_ms as f32,
        radius,
        origin: ev.pos,
    })
}

/// What one frame of [`ViewFx`] hands the camera, the HUD and the sound.
pub struct ViewOut {
    /// The drawn angles (pitch, yaw, roll degrees).
    pub angles: [f32; 3],
    /// `CG_GetViewFov`'s horizontal fov at 4:3.
    pub fov: f32,
    /// The scope overlay's alpha (`CG_GetWeapReticleZoom`); the view weapon
    /// is not drawn while it is `Some`.
    pub overlay: Option<f32>,
    pub shock: ShockFrame,
    /// The view weapon's place off the view (`play::cod2_gun`), `None`
    /// when no first-person gun is drawn.
    pub gun: Option<GunPose>,
    /// The hold-breath cues; its mix is already in `shock.sound`.
    pub breath: cod2_shock::BreathFrame,
}

/// A CoD2 session's view state across frames: the four quake slots, the
/// shellshock records and latches, the sight's latch and idle clock.
pub struct ViewFx {
    quakes: Quakes,
    shock: ShellShock,
    latch: SightLatch,
    idle: IdleState,
    gun: GunState,
    /// [`SightDef`] and [`GunKeys`] by weapon name.
    defs: HashMap<String, Option<(SightDef, GunKeys)>>,
    breath: cod2_shock::HoldBreath,
    /// A console `cg_shellshock`'s arguments, run on the next frame's
    /// clock.
    pub shock_command: Option<Vec<String>>,
}

impl Default for ViewFx {
    fn default() -> Self {
        ViewFx {
            quakes: Quakes::default(),
            shock: cod2_shock::state(),
            latch: SightLatch::default(),
            idle: IdleState::default(),
            gun: GunState::default(),
            defs: HashMap::new(),
            breath: cod2_shock::HoldBreath::default(),
            shock_command: None,
        }
    }
}

impl ViewFx {
    /// A map load: a fresh cgame (no quake carries over, the shock
    /// records reload off the new configstrings).
    pub fn reset(&mut self) {
        self.quakes.reset();
        self.shock = self.shock.restart_cgame();
        self.latch = SightLatch::default();
        self.idle = IdleState::default();
        self.gun = GunState::default();
        self.defs.clear();
        self.breath.reset();
    }

    /// The breath in's length once the sound system started it
    /// ([`cod2_shock::HoldBreath::set_in_ms`]).
    pub fn breath_in_ms(&mut self, ms: i32) {
        self.breath.set_in_ms(ms);
    }

    /// The view weapon's sight keys (`BG_GetViewmodelWeaponIndex`: the
    /// offhand while it is up).
    pub fn sight_def(&mut self, f: &Frame, fs: &Pk3Fs) -> Option<(SightDef, GunKeys)> {
        let ps = &f.snap.ps;
        let w = if ps.pm_flags & PMF_OFFHAND != 0 {
            ps.raw.i32("offHandIndex")
        } else {
            ps.weapon
        };
        let name = f.configstrings.weapon_name(w)?;
        *self.defs.entry(name.to_string()).or_insert_with(|| {
            weapon::load(fs, name)
                .ok()
                .map(|p| (SightDef::of(&p.def), GunKeys::of(&p.def)))
        })
    }

    /// One frame at cg.time `now` (`msec` since the last) for a view drawn
    /// from `eye` at `angles`: the events' quakes start, then
    /// `CG_CalcViewValues`' order over the angles, then the fov. `kick` is
    /// the HUD's `CG_DamageFeedback` record; `first_person` false (a
    /// turret's or a free spectator's view) skips `BG_CalculateViewAngles`.
    /// `gun_kick` is the recoil spring's pitch and yaw
    /// (`play::cod2_recoil`); the view kick is already in `angles`, which
    /// the cmds carry it into. `predicted` when `f`'s playerstate is the
    /// frame's prediction ([`crate::cod2_viewmodel::pos_frac`]). `land_z`
    /// is [`Land::gun`]'s drop on the gun's world z.
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        f: &Frame,
        fs: &Pk3Fs,
        events: &[Event2],
        kick: &DamageKick,
        (eye, angles): (Vec3, [f32; 3]),
        first_person: bool,
        cg_fov: f32,
        (now, msec): (i32, i32),
        gun_kick: [f32; 2],
        predicted: bool,
        land_z: f32,
    ) -> ViewOut {
        for ev in events {
            if let Some(q) = quake_of(ev) {
                self.quakes.start(q, now);
            }
        }
        let defs = self.sight_def(f, fs);
        let def = defs.map(|(d, _)| d);
        let ps = &f.snap.ps;
        let pos = crate::cod2_viewmodel::pos_frac(f, predicted);
        let mut out = angles;
        if first_person && ps.eflags & EF_TURRET == 0 && ps.pm_type < PM_INTERMISSION {
            let vps = ViewPs {
                pos_frac: pos,
                eflags: ps.eflags,
                view_height_target: ps.raw.i32("viewHeightTarget"),
                bob_cycle: ps.raw.i32("bobCycle") as u8,
                hold_breath_scale: ps.raw.f32("holdBreathScale"),
                xyspeed: Vec3::from(ps.velocity).truncate().length(),
            };
            let a = view_angles(&vps, def.as_ref(), kick, &mut self.idle, now, msec);
            for (o, d) in out.iter_mut().zip(a) {
                *o += d;
            }
        }
        let shake = self.quakes.shake(now, eye);
        for (o, d) in out.iter_mut().zip(shake) {
            *o += d;
        }
        // `CG_ShellShock_f` (Mac 0x1d2e3c): record 0 at cg.time.
        if let Some(args) = self.shock_command.take() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match self.shock.command(fs, &args, now) {
                Err(e) if !e.is_empty() => crate::console::log::print(&e),
                _ => {}
            }
        }
        // `CG_UpdateShellShock` reads `cg.snap`'s three fields; index 0 is
        // the console's record.
        let raw = &ps.raw;
        let fields = (
            raw.i32("shellshockIndex"),
            raw.i32("shellshockTime"),
            raw.i32("shellshockDuration"),
        );
        let cs = f.configstrings;
        let mut shock = self.shock.frame(
            fs,
            |i| {
                (1..16)
                    .contains(&i)
                    .then(|| cs.get(cod2_shock::CS_SHELLSHOCKS + i as usize))
            },
            fields,
            now,
        );
        if shock.kick != [0.0; 2] {
            out = kicked_angles(out, shock.kick);
        }
        // `CG_AddViewWeapon` after the view: the gun off the view it drew.
        let mut breath = cod2_shock::BreathFrame::default();
        let gun = match defs {
            Some((_, keys)) if first_person && ps.eflags & EF_TURRET == 0 && ps.pm_type < 4 => {
                let gps = GunPs {
                    view: angles,
                    pos_frac: pos,
                    leanf: raw.f32("leanf"),
                    eflags: ps.eflags,
                    weaponstate: raw.i32("weaponstate"),
                    view_height_target: raw.i32("viewHeightTarget"),
                    view_height_current: raw.f32("viewHeightCurrent"),
                    speed: raw.i32("speed") as f32,
                    xyspeed: Vec3::from(ps.velocity).truncate().length(),
                    bob_cycle: raw.i32("bobCycle") as u8,
                };
                let input = GunInput {
                    kick,
                    now,
                    msec,
                    shock_left_ms: shock.left_ms,
                    recoil: [gun_kick[0], gun_kick[1], 0.0],
                };
                let mut g = self.gun.step(&gps, &keys, &mut self.idle, &input);
                // `CG_AddPlayerWeapon` (Mac 0x1d9c8e) runs the breath off the
                // view weapon whether or not it draws.
                let vols = self.breath.volumes(fs);
                breath = self
                    .breath
                    .step(ps.pm_flags & cod2_shock::PMF_BREATH != 0, msec, vols);
                if land_z != 0.0 {
                    // Along (forward, left, up) of the drawn view.
                    let [f, r, u] = vcod_common::x87::angle_vectors(Vec3::from(out));
                    g.offset[0] += land_z * f.z;
                    g.offset[1] -= land_z * r.z;
                    g.offset[2] += land_z * u.z;
                }
                Some(g)
            }
            _ => None,
        };
        let fov = view_fov(cg_fov, ps.pm_type, ps.eflags, def.as_ref(), pos, self.latch);
        let overlay = def
            .as_ref()
            .and_then(|d| d.overlay_alpha(pos, self.latch.zooming_in));
        // `CG_AddViewWeapon` steps the latch after the view is set.
        self.latch.step(def.is_some_and(|d| d.ads), pos);
        // `SND_SetChannelVolumes` (Mac 0x4a2de) keeps the highest active
        // priority in force: the shellshock's 3 over the breath's 1.
        if let Some(v) = breath.volumes
            && shock.sound.volumes == [1.0; 11]
        {
            shock.sound.volumes = v;
        }
        ViewOut {
            angles: out,
            fov,
            overlay,
            shock,
            gun,
            breath,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A landing's change: the plain landing's parm, the pain landing's
    /// height between 256 and 480 as `(h - 12) / 26 * 4 + 4`, capped at 24.
    #[test]
    fn a_landing_sinks_by_its_parm_or_height() {
        use vcod_cod2::net::events::{EV_FOOTSTEP_RUN, EV_LANDING, EV_LANDING_PAIN};
        let fall = (256.0, 480.0);
        assert_eq!(land_change(EV_LANDING + 5, 9, fall), Some(-9.0));
        assert_eq!(land_change(EV_LANDING_PAIN, 0, fall), Some(-24.0));
        assert_eq!(
            land_change(EV_LANDING_PAIN + 22, 50, (12.0, 50.0)),
            Some(-6.0)
        );
        assert_eq!(land_change(EV_LANDING_PAIN, 0, (10.0, 20.0)), None);
        assert_eq!(land_change(EV_FOOTSTEP_RUN, 9, fall), None);
    }

    /// The eye dips over 150 ms and comes back over 300; the gun drops a
    /// further quarter on whole milliseconds; a landing stamped ahead of the
    /// clock dips by its negative time once, then is put 450 ms back.
    #[test]
    fn the_landing_dips_the_eye_and_the_gun() {
        let mut l = Land::default();
        l.land(-12.0, 1000);
        assert_eq!(l.eye(1000), 0.0);
        assert_eq!(l.eye(1075), -6.0);
        assert_eq!(l.gun(1075), -1.5);
        assert_eq!(l.eye(1150), -12.0);
        assert_eq!(l.eye(1300), -6.0);
        assert_eq!(l.gun(1300), -1.5);
        assert_eq!(l.eye(1450), 0.0);
        assert_eq!(l.gun(1450), 0.0);
        l.land(-12.0, 2000);
        assert_eq!(l.eye(1925), 6.0);
        assert_eq!(l.time_ms, 1925 - 450);
        assert_eq!((l.eye(1925), l.gun(1925)), (0.0, 0.0));
    }

    fn scope() -> SightDef {
        SightDef {
            ads: true,
            overlay: true,
            reticle: true,
            zoom_fov: 20.0,
            zoom_in_frac: 0.05,
            zoom_out_frac: 0.05,
            ads_idle_amount: 20.0,
            hip_idle_amount: 40.0,
            ads_idle_speed: 1.0,
            hip_idle_speed: 1.0,
            idle_crouch: 0.6,
            idle_prone: 0.3,
            ads_bob_factor: 1.0,
            ads_view_bob_mult: 0.0,
        }
    }

    /// The view closes on `adsZoomFov` only past the zoom fraction; a
    /// turret and intermission take their fixed fovs.
    #[test]
    fn the_fov_zooms_past_the_fraction() {
        let d = scope();
        let l = SightLatch { zooming_in: true };
        assert_eq!(view_fov(80.0, 0, 0, Some(&d), 0.0, l), 80.0);
        assert_eq!(view_fov(80.0, 0, 0, Some(&d), 0.9, l), 80.0);
        let half = view_fov(80.0, 0, 0, Some(&d), 0.975, l);
        assert!((half - 50.0).abs() < 1e-3, "{half}");
        assert_eq!(view_fov(80.0, 0, 0, Some(&d), 1.0, l), 20.0);
        assert_eq!(view_fov(80.0, 0, 0x100, Some(&d), 1.0, l), 55.0);
        assert_eq!(view_fov(80.0, 5, 0, Some(&d), 1.0, l), 90.0);
        // A hip-only weapon never zooms.
        let hip = SightDef { ads: false, ..d };
        assert_eq!(view_fov(80.0, 0, 0, Some(&hip), 1.0, l), 80.0);
    }

    /// The latch flips only at the ends; the overlay shows past 0.01 of
    /// the zoom.
    #[test]
    fn the_overlay_follows_the_zoom() {
        let d = scope();
        let mut l = SightLatch::default();
        l.step(true, 0.0);
        assert!(l.zooming_in);
        l.step(true, 0.5);
        assert!(l.zooming_in);
        l.step(true, 1.0);
        assert!(!l.zooming_in);
        assert_eq!(d.overlay_alpha(0.95, true), None);
        assert_eq!(d.overlay_alpha(1.0, true), Some(1.0));
        let a = d.overlay_alpha(0.975, true).unwrap();
        assert!((a - 0.5).abs() < 1e-4, "{a}");
        let plain = SightDef {
            overlay: false,
            ..d
        };
        assert_eq!(plain.overlay_alpha(1.0, true), None);
    }

    /// `damageCount * 0.2`, at least 5 and at most 90, splits along the
    /// view; a hit with no direction kicks the pitch up by it.
    #[test]
    fn the_damage_kick_splits_along_the_view() {
        let axis = [Vec3::X, Vec3::Y, Vec3::Z];
        let k = DamageKick::feedback(255, 255, 100, axis, 1000, 2000);
        assert_eq!(
            (k.pitch, k.roll, k.time, k.flash_until),
            (-20.0, 0.0, 1000, 2500)
        );
        // Travelling along +x (yaw 0): all pitch, `count` 10 floored at 5.
        let k = DamageKick::feedback(0, 0, 10, axis, 0, 0);
        assert!((k.pitch - 5.0).abs() < 1e-5 && k.roll.abs() < 1e-5, "{k:?}");
        // Along +y (yaw 90, byte 63.75 is not a byte: 64 is 90.35).
        let k = DamageKick::feedback(64, 0, 1000, axis, 0, 0);
        assert!(k.pitch.abs() < 1.0 && (k.roll + 90.0).abs() < 0.1, "{k:?}");
    }

    /// The flash fades from 0.7 over its 500 ms by the pitch kick.
    #[test]
    fn the_red_flash_fades_with_the_kick() {
        let k = DamageKick {
            time: 0,
            pitch: -20.0,
            roll: 0.0,
            flash_until: 1500,
        };
        assert_eq!(k.flash(1000), Some([0.2, 0.0, 0.0, 0.7]));
        // 125 ms left: 125 * 20 / 500 = 5, still full.
        assert_eq!(k.flash(1375), Some([0.2, 0.0, 0.0, 0.7]));
        let a = k.flash(1450).unwrap()[3];
        assert!((a - 0.28).abs() < 1e-5, "{a}");
        assert_eq!(k.flash(1500), None);
    }

    /// The kick rises over 100 ms and falls over 400, halved at full
    /// sight on a plain weapon.
    #[test]
    fn the_view_kick_rises_and_falls() {
        let kick = DamageKick {
            time: 1000,
            pitch: -10.0,
            roll: 4.0,
            flash_until: 0,
        };
        let ps = ViewPs::default();
        let mut idle = IdleState::default();
        let at =
            |t: i32, ps: &ViewPs, idle: &mut IdleState| view_angles(ps, None, &kick, idle, t, 16);
        assert_eq!(at(1000, &ps, &mut idle), [0.0; 3]);
        let peak = at(1100, &ps, &mut idle);
        assert!((peak[0] + 10.0).abs() < 1e-5 && (peak[2] - 4.0).abs() < 1e-5);
        let mid = at(1300, &ps, &mut idle);
        // r = 0.5: 1 - lean(0.5) = 0.25.
        assert!((mid[0] + 2.5).abs() < 1e-5, "{mid:?}");
        assert_eq!(at(1500, &ps, &mut idle), [0.0; 3]);
        let sighted = ViewPs {
            pos_frac: 1.0,
            ..ps
        };
        let half = at(1100, &sighted, &mut idle);
        assert!((half[0] + 5.0).abs() < 1e-5, "{half:?}");
    }

    /// A scope sways while sighted, by `holdBreathScale`, and not at the
    /// hip.
    #[test]
    fn a_scope_sways_while_sighted() {
        let d = scope();
        let none = DamageKick::default();
        let mut idle = IdleState::default();
        let hip = ViewPs {
            hold_breath_scale: 1.0,
            ..ViewPs::default()
        };
        let mut moved = 0.0f32;
        for t in 0..100 {
            let a = view_angles(&hip, Some(&d), &none, &mut idle, t * 16, 16);
            moved = moved.max(a[0].abs() + a[1].abs());
        }
        assert_eq!(moved, 0.0);
        let sighted = ViewPs {
            pos_frac: 1.0,
            ..hip
        };
        let mut peak = 0.0f32;
        for t in 0..200 {
            let a = view_angles(&sighted, Some(&d), &none, &mut idle, t * 16, 16);
            peak = peak.max(a[0].abs());
        }
        // `adsIdleAmount` 20 * 0.01: at most 0.2 degrees.
        assert!(peak > 0.1 && peak <= 0.2 + 1e-5, "{peak}");
        let held = ViewPs {
            hold_breath_scale: 0.0,
            ..sighted
        };
        let a = view_angles(&held, Some(&d), &none, &mut idle, 5000, 16);
        assert_eq!(a, [0.0; 3]);
    }

    /// The earthquake builtin's temp entity carries scale, ms and radius.
    #[test]
    fn an_earthquake_event_is_a_quake() {
        let ev = Event2 {
            entity: 70,
            event: EV_EARTHQUAKE,
            parm: 0,
            etype: 0,
            eflags: 0,
            weapon: 0,
            surf: 0,
            other_entity: 0,
            time: 2000,
            scale: 0,
            angles2: [0.3, 850.0],
            pos: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::ZERO,
            from_ps: false,
        };
        let q = quake_of(&ev).unwrap();
        assert_eq!(
            (q.scale, q.duration_ms, q.radius, q.origin),
            (0.3, 2000.0, 850.0, ev.pos)
        );
        let mg = Event2 {
            event: EV_FIRE_WEAPON_MG42,
            ..ev
        };
        let q = quake_of(&mg).unwrap();
        assert_eq!((q.scale, q.duration_ms, q.radius), (0.05, 100.0, 100.0));
    }

    /// The stock scoped Kar98k: a 15 degree zoom past 0.42 of the way in,
    /// `scope_overlay_german` over the view.
    #[test]
    fn the_stock_sniper_rifle_zooms_under_its_overlay() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let def = weapon::load(&fs, "kar98k_sniper_mp").unwrap().def;
        let d = SightDef::of(&def);
        assert!(d.ads && d.overlay && d.reticle);
        assert_eq!(
            (d.zoom_fov, d.zoom_in_frac, d.zoom_out_frac),
            (15.0, 0.42, 0.08)
        );
        let l = SightLatch { zooming_in: true };
        assert_eq!(view_fov(80.0, 0, 0, Some(&d), 1.0, l), 15.0);
        // `(1 - 0.58) / 0.42` in floats: a hair under 1, as retail's.
        assert!(d.overlay_alpha(1.0, true).is_some_and(|a| a > 0.9999));
        assert_eq!(d.overlay_alpha(0.5, true), None);
        // A plain rifle sights without an overlay.
        let plain = SightDef::of(&weapon::load(&fs, "kar98k_mp").unwrap().def);
        assert!(plain.ads && !plain.overlay);
    }
}
