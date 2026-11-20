//! The global fog the frame draws, faded toward each new target, and its
//! setters.

use super::*;

/// The fog values as currently displayed.
#[derive(Copy, Clone, Default, PartialEq)]
pub(super) struct FogLive {
    pub(super) linear: bool,
    pub(super) color: [f32; 3],
    pub(super) density: f32,
    pub(super) near: f32,
    pub(super) far: f32,
    pub(super) start: f32,
}

impl FogLive {
    pub(super) fn of(p: &FogParams) -> Self {
        FogLive {
            linear: p.is_linear(),
            color: p.color,
            density: p.density,
            near: p.near,
            far: p.far,
            start: p.start,
        }
    }

    /// 1 = GL_EXP, 2 = GL_LINEAR: the WGSL Camera mode encoding; the off
    /// state is carried by [`FogState::set`], not here.
    pub(super) fn mode(&self) -> f32 {
        if self.linear { 2.0 } else { 1.0 }
    }
}

/// Global fog from configstring 12, faded toward each new target. Same-mode
/// transitions lerp over the script's fade time; cross-mode changes snap and
/// linear fog clears to the fog colour (docs/protocol-1.1.md, configstring
/// 12: the clear is VERIFIED there; the fade and snap follow RTCW-MP's
/// R_SetFog and are unverified against retail).
#[derive(Default)]
pub(super) struct FogState {
    pub(super) set: bool,
    /// CoD2's world: fog runs on the radial eye distance (fogcalc.hlsl's
    /// `useRadialFog`), not CoD1's eye-space depth.
    pub(super) radial: bool,
    /// The last frame's time, where [`Renderer::set_fog_now`] starts a fade.
    pub(super) clock: f32,
    pub(super) live: FogLive,
    pub(super) from: FogLive,
    /// target params, transition start (frame seconds), duration (s)
    pub(super) target: Option<(FogParams, f32, f32)>,
    /// The last fog asked for: CoD1's frame sets it every frame, and only a
    /// new one starts a fade.
    pub(super) asked: Option<FogParams>,
}

impl FogState {
    pub(super) fn set_target(&mut self, p: FogParams, now: f32) {
        if self.set && self.asked == Some(p) {
            return;
        }
        self.asked = Some(p);
        // No fade, first fog, or a mode change snaps like retail.
        let snap = !self.set || p.fade_ms == 0 || self.live.linear != p.is_linear();
        self.from = self.live;
        self.target = Some((p, now, p.fade_ms as f32 / 1000.0));
        self.set = true;
        if snap {
            self.finish();
        }
    }

    pub(super) fn clear(&mut self) {
        self.set = false;
        self.target = None;
        self.asked = None;
    }

    pub(super) fn finish(&mut self) {
        if let Some((p, _, _)) = self.target.take() {
            self.live = FogLive::of(&p);
        }
    }

    pub(super) fn advance(&mut self, now: f32) {
        self.clock = now;
        if self.target.is_none() {
            return;
        }
        let (_, t0, dur) = self.target.unwrap();
        let k = ((now - t0) / dur.max(f32::EPSILON)).clamp(0.0, 1.0);
        let to = match self.target {
            Some((p, _, _)) => FogLive::of(&p),
            None => return,
        };
        let f = &self.from;
        self.live = FogLive {
            linear: to.linear,
            color: [
                f.color[0] + (to.color[0] - f.color[0]) * k,
                f.color[1] + (to.color[1] - f.color[1]) * k,
                f.color[2] + (to.color[2] - f.color[2]) * k,
            ],
            density: f.density + (to.density - f.density) * k,
            near: f.near + (to.near - f.near) * k,
            far: f.far + (to.far - f.far) * k,
            start: f.start + (to.start - f.start) * k,
        };
        if k >= 1.0 {
            self.target = None;
        }
    }
}

impl Renderer {
    /// New global fog target from configstring 12; `None` clears it.
    pub fn set_fog(&mut self, fog: Option<FogParams>, now: f32) {
        match fog {
            Some(p) => self.fog.set_target(p, now),
            None => self.fog.clear(),
        }
    }

    /// [`Self::set_fog`] with a fade starting at the last frame's time,
    /// for a caller that does not hold the frame clock (CoD2's session).
    pub fn set_fog_now(&mut self, fog: Option<FogParams>) {
        let now = self.fog.clock;
        self.set_fog(fog, now);
    }

    /// The displayed fog, for the F3 `fog:` line.
    pub fn fog_debug(&self) -> String {
        if !self.fog.set {
            return "off".to_string();
        }
        let f = &self.fog.live;
        let tail = if self.fog.target.is_some() { " ->" } else { "" };
        if f.linear {
            format!("linear {}..{} {:?}{tail}", f.near, f.far, f.color)
        } else {
            format!("exp den {:.6} {:?}{tail}", f.density, f.color)
        }
    }
}
