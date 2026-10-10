//! The earthquake shake on the drawn view (docs/research/cod11-combat.md,
//! "The earthquake shake"): four slots of `(start, scale, duration, radius,
//! origin)` the cgame fills off `EV_EARTHQUAKE` and every
//! `EV_FIRE_WEAPON_MG42`, and the strongest one shaking the refdef's
//! angles after `CG_CalcViewValues`.

use glam::Vec3;
use vcod_common::net::event_ids::{EV_EARTHQUAKE, EV_FIRE_WEAPON_MG42};
use vcod_common::net::events::GameEvent;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::Protocol;

/// One quake: what `0x30017dd0` copies into a slot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quake {
    pub start_ms: i32,
    pub scale: f32,
    pub duration_ms: f32,
    pub radius: f32,
    pub origin: Vec3,
}

impl Quake {
    /// `0x30017d40`: `(strength, fade)` at `now` for an eye at `eye`, `None`
    /// before it starts or once it is over. `fade` is the scale run down
    /// over the duration, `strength` that by `1 - dist / radius` (divided
    /// instead outside the radius, which leaves it negative).
    fn strength(&self, now_ms: i32, eye: Vec3) -> Option<(f32, f32)> {
        let dt = now_ms.checked_sub(self.start_ms).filter(|d| *d >= 0)? as f32;
        if dt >= self.duration_ms {
            return None;
        }
        let near = 1.0 - eye.distance(self.origin) / self.radius;
        let fade = (1.0 - dt / self.duration_ms) * self.scale;
        let strength = if near < 0.0 { near / fade } else { near * fade };
        Some((strength, fade))
    }
}

/// The quake an event starts, if any: `CG_EntityPreEvent`'s two calls to
/// `0x30017dd0`. `es` is the event's entity, `None` for the playerstate's
/// ring; `origin` is where the event's body is drawn.
pub fn from_event(
    p: &Protocol,
    ev: &GameEvent,
    es: Option<&EntityState>,
    origin: Vec3,
) -> Option<Quake> {
    let (scale, duration_ms, radius) = match ev.event {
        // 0x3001e936: a fixed 0.05 for 100 ms within 100 units.
        EV_FIRE_WEAPON_MG42 => (0.05, 100, 100.0),
        // `es.angles2[0]`, `es.time` and `es.angles2[1]`, as the
        // `earthquake` builtin (game 0x5f3d8) writes them.
        EV_EARTHQUAKE => {
            let es = es?;
            (
                es.field_f32(p, "angles2[0]"),
                es.field_i32(p, "time"),
                es.field_f32(p, "angles2[1]"),
            )
        }
        _ => return None,
    };
    Some(Quake {
        start_ms: 0,
        scale,
        duration_ms: duration_ms as f32,
        radius,
        origin,
    })
}

/// `0x3020cff4`, the four slots, and `0x3020d084`, the phase.
#[derive(Default)]
pub struct Quakes {
    slots: [Quake; 4],
    phase: f32,
    /// msvcrt `rand()`'s state, for the phase.
    seed: u32,
}

impl Quakes {
    /// `0x30017dd0` at `now`: a non-positive scale is dropped; a free slot
    /// (not started yet, or over) takes it, else the weakest slot weaker
    /// than it does, else it is dropped.
    pub fn start(&mut self, mut q: Quake, now_ms: i32, eye: Vec3) {
        // `!(scale > 0)`, so a NaN is dropped too.
        if q.scale.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return;
        }
        q.start_ms = now_ms;
        let strength = |s: &Quake| s.strength(now_ms, eye).map_or(0.0, |(s, _)| s);
        let free = self.slots.iter().position(|s| {
            s.start_ms > now_ms || (s.start_ms as f32 + s.duration_ms) <= now_ms as f32
        });
        let slot = free.or_else(|| {
            let mut best = None;
            let mut min = strength(&q);
            for (i, s) in self.slots.iter().enumerate() {
                if strength(s) < min {
                    min = strength(s);
                    best = Some(i);
                }
            }
            best
        });
        if let Some(i) = slot {
            self.slots[i] = q;
        }
    }

    /// A map load: no slot carries over (cg.time starts again).
    pub fn reset(&mut self) {
        self.slots = Default::default();
    }

    /// `0x30017f00`: the shake to add to the view's pitch, yaw and roll,
    /// wire degrees, for an eye at `eye`. The strongest slot wins, capped
    /// at 1 and scaled once more by its fade; with none the phase is
    /// re-drawn every frame.
    pub fn shake(&mut self, now_ms: i32, eye: Vec3) -> [f32; 3] {
        let (mut strength, mut fade) = (0.0f32, 0.0f32);
        for s in &self.slots {
            if let Some((st, f)) = s.strength(now_ms, eye)
                && st > strength
            {
                (strength, fade) = (st, f);
            }
        }
        if strength <= 0.0 {
            self.phase = (self.rand() as f32 * (1.0 / 32768.0) * 2.0 - 1.0) * std::f32::consts::PI;
            return [0.0; 3];
        }
        let amp = strength.min(1.0) * fade;
        // Stored as a float, then multiplied out in x87 precision.
        let t = f64::from((f64::from(now_ms) * f64::from(0.001_666_666_7f32)) as f32);
        let wave = |freq: f32, gain: f32| {
            ((t * f64::from(freq) + f64::from(self.phase)).sin() as f32) * amp * gain
        };
        // 8, 15 and 12 PI over 600 ms: 150, 80 and 100 ms periods.
        [
            wave(25.132_742, 18.0),
            wave(47.123_89, 16.0),
            wave(37.699_112, 10.0),
        ]
    }

    /// msvcrt's `rand()`: the LCG `0x3004b189` calls, 15 bits out.
    fn rand(&mut self) -> u32 {
        self.seed = self.seed.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.seed >> 16) & 0x7fff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quake(scale: f32, duration_ms: f32, radius: f32) -> Quake {
        Quake {
            start_ms: 0,
            scale,
            duration_ms,
            radius,
            origin: Vec3::ZERO,
        }
    }

    /// Strength is scale by time left by distance left; the fade is the
    /// first two alone.
    #[test]
    fn strength_falls_with_time_and_distance() {
        let mut q = quake(0.5, 1000.0, 400.0);
        q.start_ms = 1000;
        assert_eq!(q.strength(999, Vec3::ZERO), None);
        assert_eq!(q.strength(1000, Vec3::ZERO), Some((0.5, 0.5)));
        assert_eq!(
            q.strength(1500, Vec3::new(200.0, 0.0, 0.0)),
            Some((0.125, 0.25))
        );
        assert_eq!(q.strength(2000, Vec3::ZERO), None);
        let (outside, _) = q.strength(1500, Vec3::new(800.0, 0.0, 0.0)).unwrap();
        assert!(outside < 0.0);
    }

    /// The strongest live slot shakes the view, at most its fade times
    /// 18, 16 and 10 degrees; nothing live shakes nothing.
    #[test]
    fn the_strongest_quake_shakes_within_its_amplitude() {
        let mut qs = Quakes::default();
        assert_eq!(qs.shake(500, Vec3::ZERO), [0.0; 3]);
        qs.start(quake(0.3, 3000.0, 850.0), 1000, Vec3::ZERO);
        qs.start(quake(0.05, 100.0, 100.0), 1000, Vec3::ZERO);
        let mut peak = [0.0f32; 3];
        for t in (1000..1150).step_by(5) {
            let s = qs.shake(t, Vec3::ZERO);
            for (p, v) in peak.iter_mut().zip(s) {
                *p = p.max(v.abs());
            }
        }
        // At the start: strength 0.3, fade 0.3, so 0.09 a unit of gain.
        for (p, gain) in peak.iter().zip([18.0f32, 16.0, 10.0]) {
            assert!(*p <= 0.09 * gain + 1e-4 && *p > 0.06 * gain, "{peak:?}");
        }
        assert_eq!(qs.shake(4000, Vec3::ZERO), [0.0; 3]);
        assert_eq!(qs.shake(1100, Vec3::new(900.0, 0.0, 0.0)), [0.0; 3]);
    }

    /// With every slot live, a new quake replaces the weakest one weaker
    /// than itself, and a weaker one is dropped.
    #[test]
    fn a_full_table_keeps_the_strongest() {
        let mut qs = Quakes::default();
        for scale in [0.4, 0.2, 0.3, 0.5] {
            qs.start(quake(scale, 5000.0, 1000.0), 100, Vec3::ZERO);
        }
        qs.start(quake(0.1, 5000.0, 1000.0), 100, Vec3::ZERO);
        let scales = |qs: &Quakes| qs.slots.map(|s| s.scale);
        assert_eq!(scales(&qs), [0.4, 0.2, 0.3, 0.5]);
        qs.start(quake(0.25, 5000.0, 1000.0), 100, Vec3::ZERO);
        assert_eq!(scales(&qs), [0.4, 0.25, 0.3, 0.5]);
        qs.start(quake(0.0, 5000.0, 1000.0), 100, Vec3::ZERO);
        assert_eq!(scales(&qs), [0.4, 0.25, 0.3, 0.5]);
    }
}
