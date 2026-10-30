//! CoD4 1.7's damage feedback on the 2D layer, off the snapshots'
//! playerstates: the `low health` menu's pulsing `overlay_low_health`
//! (owner draw 112, `iw3mp.exe` 0x442e70) and the `hit_direction` icons
//! turned toward each hit (0x42c6e0). Addresses and defaults:
//! docs/research/cod4-hud.md 19.

use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use vcod_cod4::net::parse::Snapshot;

/// `hud_healthOverlay_pulseStart`: the health fraction under which the
/// overlay throbs, and `regenPauseTime` in ms.
const PULSE_START: f32 = 0.55;
const REGEN_PAUSE_MS: i32 = 8000;
/// A beat's three legs: the beat's magnitude times this, eased to over
/// these ms (`phaseOne_pulseDuration`, `phaseTwo_*`, `phaseThree_*`).
const LEGS: [(f32, i32); 3] = [(1.0, 150), (0.7, 320), (0.6, 400)];
/// The four beats' magnitudes (0x442c23..0x442e5e).
const BEATS: [f32; 4] = [1.0, 0.8, 0.6, 0.3];
/// After the last beat: `phaseEnd_toAlpha` over `phaseEnd_pulseDuration`.
const REST: (f32, i32) = (0.0, 700);

/// `cg_hudDamageIconWidth`, `-Height`, `-Offset` and `-Time` (0x43b3ab..
/// 0x43b4b1); `cg_hudDamageIconInScope` is 0.
const ICON_W: f32 = 128.0;
const ICON_H: f32 = 64.0;
const ICON_OFFSET: f32 = 128.0;
const ICON_MS: i32 = 2000;
/// The icons' material.
pub(crate) const HIT_DIRECTION: &str = "hit_direction";
/// `damageYaw` and `damagePitch` both 255: a hit from no direction.
const NO_DIRECTION: (u32, u32) = (255, 255);

/// `pm_type` 7, dead.
const PM_DEAD: u32 = 7;

/// A straight ease of the overlay's alpha from `from` to `to` over `ms`
/// from `start`.
#[derive(Clone, Copy, Debug, Default)]
struct Ease {
    from: f32,
    to: f32,
    start: i32,
    ms: i32,
}

impl Ease {
    fn at(&self, now: i32) -> f32 {
        let t = (now - self.start).max(0);
        if self.ms <= 0 || t >= self.ms {
            return self.to;
        }
        self.from + (self.to - self.from) * (t as f32 / self.ms as f32)
    }

    fn running(&self, now: i32) -> bool {
        self.start + self.ms > now
    }

    /// The next ease, on from where this one aimed.
    fn then(&mut self, to: f32, ms: i32, now: i32) {
        *self = Ease {
            from: self.to,
            to,
            start: now,
            ms,
        };
    }
}

/// The low-health overlay's throb (0x442c20, state at `cg` +
/// 0x50334..0x50354): under [`PULSE_START`] it beats four times, each in
/// three legs, then rests.
#[derive(Clone, Copy, Debug, Default)]
struct Throb {
    ease: Ease,
    /// Which of [`BEATS`] and which of its [`LEGS`] come next.
    beat: usize,
    leg: usize,
    beating: bool,
    /// When the health last fell under the start.
    hurt_at: i32,
    last_health: f32,
}

impl Throb {
    /// The respawn's reset (0x44610a): not beating, resting at 0 with the
    /// health read as full; the ease's start and source survive it.
    fn reset(&mut self) {
        *self = Throb {
            ease: Ease {
                to: REST.0,
                ms: 0,
                ..self.ease
            },
            hurt_at: self.hurt_at,
            last_health: 1.0,
            ..Throb::default()
        };
    }

    /// One frame at `health` (0..1): a fall under the start begins the
    /// beats again; once the running leg ends the next one starts.
    fn step(&mut self, health: f32, now: i32) {
        let low = health < PULSE_START;
        if low && health < self.last_health {
            self.hurt_at = now;
            self.beat = 0;
        }
        self.last_health = health;
        if self.ease.running(now) {
            return;
        }
        self.beating |= low;
        if !self.beating {
            return;
        }
        let Some(&magnitude) = BEATS.get(self.beat) else {
            self.beating = false;
            self.leg = 0;
            self.ease.then(REST.0, REST.1, now);
            return;
        };
        let (scale, ms) = LEGS[self.leg];
        self.ease.then((magnitude * scale).clamp(0.0, 1.0), ms, now);
        self.leg = (self.leg + 1) % LEGS.len();
        // A beat's last leg moves on once the regen pause, less three
        // beats, has run since the hurt.
        let beat_ms: i32 = LEGS.iter().map(|l| l.1).sum();
        if self.leg == 0 && now >= self.hurt_at - 3 * beat_ms + REGEN_PAUSE_MS {
            self.beat += 1;
        }
    }
}

/// One of the eight hit direction slots (`iw3mp.exe` 0x79e6b8): when, for how
/// long, and the hit's yaw in the world.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Icon {
    pub start: i32,
    pub ms: i32,
    pub yaw: f32,
}

/// What the last taken snapshot's playerstate held.
#[derive(Clone, Copy, Debug)]
struct Seen {
    server_time: i32,
    /// `clientNum` and `stats[4]`, the spawn count: either changing is a
    /// respawn (0x44de5f).
    spawn: (u32, i32),
    damage_event: u32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Cod4Damage {
    throb: Throb,
    icons: [Icon; 8],
    seen: Option<Seen>,
    /// The newest snapshot's health fraction.
    health: f32,
    /// The state of the CRT `rand()` the icons' jitter draws on.
    rng: u32,
    /// The view's and gun's kick of the last hit (`play::cod4_kick`).
    kick: crate::play::cod4_kick::DamageKick,
    /// The view the next hit is measured against (pitch, yaw).
    view: [f32; 2],
}

impl Cod4Damage {
    /// Takes in the snapshots newer than the last taken, in order, and
    /// steps the overlay's throb at cg time `now`.
    pub(crate) fn update(&mut self, snaps: &[Snapshot], now: i32, view: [f32; 2]) {
        self.view = view;
        let after = self.seen.map_or(i32::MIN, |s| s.server_time);
        for snap in snaps.iter().filter(|s| s.server_time > after) {
            self.take(snap);
        }
        if let Some(newest) = snaps.last() {
            self.health = health_frac(newest);
        }
        // The owner draw neither steps nor draws at 0 health.
        if self.health != 0.0 {
            self.throb.step(self.health, now);
        }
    }

    /// One snapshot: a respawn on the first or on a new spawn, else a hit
    /// on a new `damageEvent` with a `damageCount` (`CG_DamageFeedback`,
    /// 0x445d00).
    fn take(&mut self, snap: &Snapshot) {
        let ps = &snap.ps;
        let word = |n: &str| ps.get(n).unwrap_or(0);
        let seen = Seen {
            server_time: snap.server_time,
            spawn: (word("clientNum"), ps.stats[4]),
            damage_event: word("damageEvent"),
        };
        let Some(last) = self.seen.replace(seen) else {
            return self.respawn();
        };
        if last.spawn != seen.spawn {
            return self.respawn();
        }
        let count = word("damageCount");
        if last.damage_event == seen.damage_event || count == 0 {
            return;
        }
        let (yaw, pitch) = (word("damageYaw"), word("damagePitch"));
        self.kick
            .feedback(count, yaw, pitch, self.view, snap.server_time);
        if (yaw, pitch) != NO_DIRECTION {
            self.mark(snap.server_time, yaw);
        }
    }

    /// `CG_Respawn` (0x445fa0): the slots cleared, the throb reset.
    fn respawn(&mut self) {
        self.icons = [Icon::default(); 8];
        self.throb.reset();
        self.kick = Default::default();
    }

    /// The oldest slot takes a hit from the byte angle `yaw`, jittered up
    /// to 10 degrees either way (0x445eb0..0x445f2f).
    fn mark(&mut self, server_time: i32, yaw: u32) {
        let jitter = (self.rand() as f32 * (1.0 / 32768.0) - 0.5) * 20.0;
        let degrees = yaw as f32 / 255.0 * 360.0;
        let oldest = self
            .icons
            .iter_mut()
            .reduce(|a, b| if b.start < a.start { b } else { a })
            .expect("eight slots");
        *oldest = Icon {
            start: server_time,
            ms: ICON_MS,
            yaw: (degrees + jitter).rem_euclid(360.0),
        };
    }

    /// The MSVC CRT's `rand()` (0x670622) from its seed of 1.
    fn rand(&mut self) -> u32 {
        let state = self.rng.max(1);
        self.rng = state.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.rng >> 16) & 0x7fff
    }

    /// The last hit's kick (cod4-client-view.md 14.2).
    pub(crate) fn kick(&self) -> crate::play::cod4_kick::DamageKick {
        self.kick
    }

    /// The eight slots, what the live gate reads.
    #[cfg(test)]
    pub(crate) fn icons(&self) -> &[Icon; 8] {
        &self.icons
    }

    /// The low-health overlay's alpha at `now`, 0 while the health reads 0.
    pub(crate) fn overlay_alpha(&self, now: i32) -> f32 {
        if self.health == 0.0 {
            0.0
        } else {
            self.throb.ease.at(now)
        }
    }

    /// The hit direction icons at `now`: each live slot a quad
    /// [`ICON_OFFSET`] below the screen's centre, swung about the centre
    /// by the view's yaw less the hit's, whole for the first half of its
    /// time, then fading out. None while `hidden` (a sight's overlay up,
    /// or a shellshock's screen blend).
    pub(crate) fn icon_quads(
        &self,
        sp: &ScreenPlacement,
        now: i32,
        view_yaw: f32,
        hidden: bool,
        texture: &str,
    ) -> Vec<HudQuad> {
        if hidden {
            return Vec::new();
        }
        let centre = [sp.axis(0, 0.0, 0.0, 2).0, sp.axis(1, 0.0, 0.0, 2).0];
        let [sx, sy] = sp.virtual_to_real;
        let half_w = sx * ICON_W * 0.5;
        let near = sy * ICON_OFFSET;
        let far = sy * ICON_H + near;
        // The far edge first, as the texture's top.
        let corners = [
            [half_w, far],
            [-half_w, far],
            [-half_w, near],
            [half_w, near],
        ];
        let live = self.icons.iter().filter_map(|icon| {
            let age = now - icon.start;
            (age > 0 && age < icon.ms).then_some((icon, age))
        });
        live.map(|(icon, age)| {
            let fade = 2.0 - (2 * age) as f32 / icon.ms as f32;
            let (sin, cos) = (view_yaw - icon.yaw).to_radians().sin_cos();
            HudQuad {
                verts: corners.map(|c| turn_on_grid(centre, c, [sin, cos], [sx, sy])),
                uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                rgba: [1.0, 1.0, 1.0, fade.min(1.0)],
                texture: texture.to_string(),
            }
        })
        .collect()
    }
}

/// A real-pixel offset `[x, y]` from `centre` turned by `[sin, cos]` as
/// the 640x480 grid would turn it, `[sx, sy]` the grid's scales.
fn turn_on_grid(
    centre: [f32; 2],
    [x, y]: [f32; 2],
    [sin, cos]: [f32; 2],
    [sx, sy]: [f32; 2],
) -> [f32; 2] {
    [
        centre[0] + cos * x - sin * y * sx / sy,
        centre[1] + cos * y + sin * x * sy / sx,
    ]
}

/// The health fraction (0x442640): `stats[0] / stats[2]` in 0..1, 0 for
/// a dead body or an unset stat.
fn health_frac(snap: &Snapshot) -> f32 {
    let ps = &snap.ps;
    let (hp, max) = (ps.stats[0], ps.stats[2]);
    if hp == 0 || max == 0 || ps.get("pm_type") == Some(PM_DEAD) {
        return 0.0;
    }
    (hp as f32 / max as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Under 0.55 the overlay beats in at full, eases to 0.7 and 0.6 of
    /// it, and with no new hurt takes the next, weaker beat once the regen
    /// pause allows; over it the last leg runs out to 0.
    #[test]
    fn the_throb_runs_its_legs_and_ends_at_rest() {
        let mut p = Throb::default();
        p.reset();
        p.step(0.5, 1000);
        assert_eq!((p.ease.to, p.ease.ms), (1.0, 150));
        assert_eq!(p.ease.at(1075), 0.5);
        p.step(0.5, 1150);
        assert!((p.ease.to - 0.7).abs() < 1e-6 && p.ease.ms == 320);
        p.step(0.5, 1470);
        assert!((p.ease.to - 0.6).abs() < 1e-6 && p.ease.ms == 400);
        // 1000 - 3 * 870 + 8000 = 6390: not yet, the first beat again.
        assert_eq!(p.beat, 0);
        p.step(0.9, 1870);
        assert_eq!(p.ease.to, 1.0, "a beat running keeps going over 0.55");
        let mut q = p;
        q.beat = 4;
        q.step(0.9, 2020);
        assert_eq!((q.ease.to, q.ease.ms, q.beating), (0.0, 700, false));
        q.step(0.9, 3000);
        assert_eq!(q.ease.at(3000), 0.0);
    }

    fn snap(time: i32, words: &[(&str, u32)], hp: i32, spawn: i32) -> Snapshot {
        let mut ps = vcod_cod4::net::delta::PlayerState::default();
        for (name, v) in words {
            let f = vcod_cod4::net::fields::PLAYER_STATE
                .iter()
                .find(|f| f.name == *name)
                .expect("a playerstate field");
            ps.words[f.word()] = *v;
        }
        ps.stats[0] = hp;
        ps.stats[2] = 100;
        ps.stats[4] = spawn;
        Snapshot {
            message_num: 0,
            server_time: time,
            delta_num: 0,
            base: None,
            snap_flags: 0,
            ps,
            entities: Vec::new(),
            clients: Vec::new(),
            wire: Default::default(),
        }
    }

    /// A digest of a scripted life (hits with and without a direction, the
    /// health falling under the pulse's start and climbing back, a death
    /// and a respawn): the overlay's alpha every 50 ms, the eight slots and
    /// the icons' quads, fixed before the module's rewrite.
    fn scripted_life() -> String {
        use std::fmt::Write;
        // (time, damageEvent, damageCount, damageYaw, damagePitch, health,
        // spawn count, pm_type)
        type Step = (i32, u32, u32, u32, u32, i32, i32, u32);
        let script: &[Step] = &[
            (1000, 0, 0, 0, 0, 100, 1, 0),
            (1500, 1, 30, 64, 0, 70, 1, 0),
            (1550, 1, 30, 64, 0, 70, 1, 0),
            (2200, 2, 25, 255, 255, 45, 1, 0),
            (2600, 3, 10, 200, 10, 35, 1, 0),
            (3100, 4, 0, 30, 0, 35, 1, 0),
            (3300, 5, 5, 10, 0, 30, 1, 0),
            (3400, 6, 5, 20, 0, 25, 1, 0),
            (3500, 7, 5, 30, 0, 20, 1, 0),
            (3600, 8, 5, 40, 0, 15, 1, 0),
            (3700, 9, 5, 50, 0, 12, 1, 0),
            (3800, 10, 5, 60, 0, 10, 1, 0),
            (3900, 11, 5, 70, 0, 8, 1, 0),
            (6000, 11, 5, 70, 0, 60, 1, 0),
            (9000, 11, 5, 70, 0, 80, 1, 0),
            (14000, 11, 5, 70, 0, 40, 1, 0),
            (20000, 12, 40, 128, 0, 0, 1, 7),
            (21000, 12, 40, 128, 0, 0, 1, 7),
            (23000, 12, 40, 128, 0, 100, 2, 0),
            (23500, 13, 20, 100, 0, 50, 2, 0),
            (30000, 13, 20, 100, 0, 50, 2, 0),
        ];
        let mut d = Cod4Damage::default();
        let mut s = String::new();
        let sp = ScreenPlacement::scr_place(1280.0, 720.0, 1.0);
        let mut snaps = Vec::new();
        let mut next = 0;
        for now in (1000..31000).step_by(50) {
            let mut fresh = false;
            while next < script.len() && script[next].0 <= now {
                let (t, ev, count, yaw, pitch, hp, spawn, pm) = script[next];
                let words = [
                    ("damageEvent", ev),
                    ("damageCount", count),
                    ("damageYaw", yaw),
                    ("damagePitch", pitch),
                    ("pm_type", pm),
                ];
                snaps.push(snap(t, &words, hp, spawn));
                next += 1;
                fresh = true;
            }
            d.update(&snaps, now, [5.0, 30.0]);
            write!(s, "{now} {:?}", d.overlay_alpha(now)).unwrap();
            if fresh {
                write!(s, " {:?}", d.icons()).unwrap();
            }
            for (yaw, hidden) in [(0.0, false), (137.5, false), (0.0, true)] {
                for q in d.icon_quads(&sp, now, yaw, hidden, "hit") {
                    write!(s, " | {} {:?} {:?} {:?}", q.texture, q.rgba, q.verts, q.uvs).unwrap();
                }
            }
            writeln!(s).unwrap();
        }
        s
    }

    #[test]
    fn a_scripted_life_draws_its_pinned_overlay_and_icons() {
        let got = scripted_life();
        if let Ok(path) = std::env::var("DAMAGE_PIN_DUMP") {
            std::fs::write(path, &got).unwrap();
        }
        let want = include_str!("../tests/fixtures/cod4_damage_pins.txt");
        assert!(
            got == want,
            "the overlay or the icons moved off the pinned ones"
        );
    }
}
