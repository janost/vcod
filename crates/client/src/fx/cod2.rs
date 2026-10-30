//! CoD2 effects on vcod's fx system. A parsed [`vcod_cod2::efx::Effect`]
//! becomes [`efx::Emitter`]s that carry their primitive ([`Element`]); the
//! sim spawns them as it spawns CoD1's and hands each particle a [`Live`],
//! which evaluates the channels over the particle's life as the CoD2 engine
//! does (docs/research/cod2-efx.md 2, 4). What the sim reads off the plain
//! emitter fields (count, delay, origin, flags, lists) is filled from the
//! primitive; the CoD1 start/end curves hold the channels' ends for the
//! paths that never ask a [`Live`] (decals).

use crate::fx::efx::{self, Curve, CurveV3, Emitter, R1, R3};
use crate::fx::sim::Rng;
use glam::Vec3;
use std::collections::HashMap;
use std::sync::Arc;
use vcod_cod2::efx::{Channel, Kind, Primitive, attr, spawn};
use vcod_common::asset_source::{Asset, AssetSource};

/// One primitive, shared by every particle it spawns.
#[derive(Debug)]
pub struct Element {
    pub prim: Primitive,
    /// Each material's atlas, `(rows, columns)` from its file header
    /// (cod2-materials.md, `textureAtlasRowCount`); 1 x 1 when unread.
    pub atlas: HashMap<String, (u32, u32)>,
}

/// The `.efx` at `path` (any of `fx/a/b`, `/fx/a/b`, `fx/a/b.efx`) as fx
/// emitters; an error carries the engine's reason for refusing it.
pub fn load(fs: &dyn AssetSource, path: &str) -> Result<efx::Effect, String> {
    let e = vcod_cod2::efx::Effect::find(fs, path).map_err(|e| e.to_string())?;
    Ok(efx::Effect {
        emitters: e
            .primitives
            .into_iter()
            .map(|p| to_emitter(p, fs))
            .collect(),
    })
}

/// An effect name as the sim chains it: `fx/...` without a leading slash.
fn fx_name(n: &str) -> String {
    vcod_cod2::efx::effect_key(n).unwrap_or_else(|| n.to_string())
}

fn r1((a, b): (f32, f32)) -> R1 {
    R1 { a, b }
}

fn r3(v: [(f32, f32); 3]) -> R3 {
    R3 {
        a: v.map(|r| r.0),
        b: v.map(|r| r.1),
    }
}

fn mid((a, b): (f32, f32)) -> f32 {
    (a + b) * 0.5
}

/// A channel's two ends at its mid scale, as a CoD1 `linear` curve.
fn ends(p: &Primitive, c: Channel) -> Curve {
    let ch = p.channel(c);
    let s = mid(ch.scale);
    let v = |t: f32| R1 {
        a: ch.curve.sample1(t) * s,
        b: ch.curve.sample1(t) * s,
    };
    Curve {
        start: v(0.0),
        end: v(1.0),
        flags: vec!["linear".into()],
    }
}

fn atlas_of(fs: &dyn AssetSource, name: &str) -> (u32, u32) {
    fs.read(&format!("materials/{name}"))
        .and_then(|b| vcod_cod2::material::MaterialFile::parse(&b).ok())
        .map_or((1, 1), |m| {
            (
                u32::from(m.atlas_rows.max(1)),
                u32::from(m.atlas_columns.max(1)),
            )
        })
}

/// What the sim reads, from the primitive; the primitive itself rides
/// along in [`Emitter::cod2`].
pub fn to_emitter(p: Primitive, fs: &dyn AssetSource) -> Emitter {
    let flags = attr::NAMES
        .iter()
        .filter(|(_, b)| p.flags & b != 0)
        .map(|(n, _)| n.to_string())
        .collect();
    let spawn_flags = spawn::NAMES
        .iter()
        .filter(|(_, a, s)| (*s != 0 && p.spawn_flags & s != 0) || (*a != 0 && p.flags & a != 0))
        .map(|(n, _, _)| n.to_string())
        .collect();
    let rgb = {
        let ch = p.channel(Channel::Rgb);
        let s = mid(ch.scale);
        let v = |t: f32| {
            let c = ch.curve.sample(t).map(|x| x * s);
            R3 { a: c, b: c }
        };
        CurveV3 {
            start: v(0.0),
            end: v(1.0),
            flags: vec!["linear".into()],
        }
    };
    let atlas = p
        .materials
        .iter()
        .map(|m| (m.clone(), atlas_of(fs, m)))
        .collect();
    let names = |v: &[String]| v.iter().map(|n| fx_name(n)).collect::<Vec<_>>();
    Emitter {
        kind: p.kind.name().to_string(),
        name: Some(p.name.clone()),
        flags,
        spawn_flags,
        count: r1(p.count),
        life: r1(p.life),
        delay: r1(p.delay),
        radius: r1(p.radius),
        height: r1(p.height),
        origin: r3(p.origin),
        cullrange: p.spawn_range.1,
        rotation: r1(p.rotation),
        angle: r3(p.angle),
        angle_delta: r3(p.angle_delta),
        bounce: p.bounce.map(r1),
        models: p.models.clone(),
        impact_fx: names(&p.impact_fx),
        emit_fx: names(&p.emit_fx),
        death_fx: names(&p.death_fx),
        play_fx: names(&p.play_fx),
        density: r1(p.density),
        variance: r1(p.variance),
        size: ends(&p, Channel::Size),
        size2: ends(&p, Channel::Size2),
        non_uniform_scale: p.non_uniform_scale,
        length: ends(&p, Channel::Length),
        alpha: ends(&p, Channel::Alpha),
        rgb,
        shaders: p.materials.iter().map(|m| m.as_str().into()).collect(),
        cod2: Some(Arc::new(Element { prim: p, atlas })),
        ..Emitter::default()
    }
}

/// The per-particle draws of `FX_InitParticle` and the channel instances
/// (Mac 0x59d78, 0x9ffba), and where the particle started.
#[derive(Debug)]
pub struct Live {
    el: Arc<Element>,
    /// `flrand(0, 1)` per random channel, 0 when its flag is off: rgb,
    /// alpha, size, size2, length, rotation delta.
    weight: [f32; 6],
    vel_w: [f32; 3],
    vel2_w: [f32; 3],
    /// Each channel's scale, drawn once.
    scale: [f32; vcod_cod2::efx::CHANNELS],
    /// Spawn point and effect axis, in the space the particle's `pos` is in.
    pub origin: Vec3,
    pub axis: [Vec3; 3],
    /// Up, in the same space; gravity pulls along `-up * -gravity`.
    pub up: Vec3,
    pub life_ms: f32,
    /// Units per second squared, along `up`.
    pub gravity: f32,
    /// Off once a hit has bounced it: the sim's own integration takes over.
    pub moving: bool,
    /// The material drawn and its first atlas frame and rate (frames per
    /// ms), `FX_SetMaterialAndSequenceParams` (Mac 0x5a4fc).
    frame0: u32,
    rate: f32,
    frames: u32,
    grid: (u32, u32),
    /// A `Cloud`'s spin axis (`fx::cod2_cloud::spin_axis`); `None` for
    /// every other kind.
    pub cloud_axis: Option<Vec3>,
}

impl Live {
    /// A new particle of `el` at `origin` on `axis`, drawing `material`;
    /// `index` is its place in the spawn's count.
    pub fn spawn(
        el: Arc<Element>,
        rng: &mut Rng,
        (origin, axis): (Vec3, [Vec3; 3]),
        up: Vec3,
        life_ms: f32,
        material: &str,
        index: u32,
    ) -> Live {
        let p = &el.prim;
        let mut flag = |bits: u32| if p.has(bits) { rng.next_f32() } else { 0.0 };
        let weight = [
            flag(attr::USE_RANDOM_COLORS),
            flag(attr::USE_RANDOM_ALPHA),
            flag(attr::USE_RANDOM_SIZE),
            flag(attr::USE_RANDOM_SIZE2),
            flag(attr::USE_RANDOM_LENGTH),
            flag(attr::USE_RANDOM_ROTATION_DELTA),
        ];
        let mut three = |bits: u32| {
            if p.has(bits) {
                // Drawn z, y, x (`SetRandomVelocityWeights`' arguments).
                let (z, y, x) = (rng.next_f32(), rng.next_f32(), rng.next_f32());
                [x, y, z]
            } else {
                [0.0; 3]
            }
        };
        let vel_w = three(attr::USE_RANDOM_VELOCITY);
        let vel2_w = three(attr::USE_RANDOM_VELOCITY2);
        let gravity = rng.range(p.gravity.0, p.gravity.1);
        let scale = std::array::from_fn(|i| {
            let (a, b) = p.channels[i].scale;
            rng.range(a, b)
        });
        let grid = el.atlas.get(material).copied().unwrap_or((1, 1));
        let frames = grid.0 * grid.1;
        let (frame0, rate) = if frames <= 1 {
            (0, 0.0)
        } else {
            let start = match p.sequence_start_frame_mode {
                0 => (p.sequence_fixed_frame - 1).max(0) as u32,
                1 => ((rng.next_f32() * frames as f32) as u32).min(frames - 1),
                _ => index,
            };
            let rate = match p.sequence_play_rate_mode {
                0 => p.sequence_fixed_fps / 1000.0,
                _ => frames as f32 / life_ms.max(1.0),
            };
            (start, rate)
        };
        let cloud_axis = (p.kind == Kind::Cloud)
            .then(|| crate::fx::cod2_cloud::spin_axis(|| rng.range(-1.0, 1.0)));
        Live {
            weight,
            vel_w,
            vel2_w,
            scale,
            origin,
            axis,
            up,
            life_ms,
            gravity,
            moving: true,
            frame0,
            rate,
            frames,
            grid,
            cloud_axis,
            el,
        }
    }

    /// `lerp(curve, rand curve, weight) * scale` at life fraction `t`.
    fn chan(&self, c: Channel, w: f32, t: f32) -> f32 {
        let p = &self.el.prim;
        let a = p.channel(c).curve.sample1(t);
        let b = p.channels[c as usize + 1].curve.sample1(t);
        (a + (b - a) * w) * self.scale[c as usize]
    }

    /// Colour 0..1 (`Particle::UpdateRGB`, Mac 0xa1c6a): clamped as the
    /// byte store clamps.
    pub fn rgb(&self, t: f32) -> Vec3 {
        let p = &self.el.prim;
        let a = Vec3::from(p.channel(Channel::Rgb).curve.sample(t));
        let b = Vec3::from(p.channel(Channel::RgbRand).curve.sample(t));
        (a.lerp(b, self.weight[0]) * self.scale[Channel::Rgb as usize]).clamp(Vec3::ZERO, Vec3::ONE)
    }

    /// `Particle::UpdateAlpha` (Mac 0xa5178), clamped to 0..1.
    pub fn alpha(&self, t: f32) -> f32 {
        self.chan(Channel::Alpha, self.weight[1], t).clamp(0.0, 1.0)
    }

    pub fn size(&self, t: f32) -> f32 {
        self.chan(Channel::Size, self.weight[2], t)
    }

    pub fn size2(&self, t: f32) -> f32 {
        self.chan(Channel::Size2, self.weight[3], t)
    }

    pub fn length(&self, t: f32) -> f32 {
        self.chan(Channel::Length, self.weight[4], t)
    }

    /// Degrees per 100 ms: `Particle::Update` adds it times the frame's ms
    /// times 0.01 (Mac 0xa7970).
    pub fn rotation_delta(&self, t: f32) -> f32 {
        self.chan(Channel::RotationDelta, self.weight[5], t)
    }

    /// One velocity's displacement integral over life fraction `t`, in
    /// the effect frame, life-seconds (`Particle::IntegrateVelocity`).
    fn integral(&self, first: Channel, w: [f32; 3], t: f32) -> Vec3 {
        let p = &self.el.prim;
        Vec3::from_array(std::array::from_fn(|i| {
            let c = first as usize + i;
            let a = p.channels[c].curve.integrate(t);
            let b = p.channels[c + 3].curve.integrate(t);
            (a + (b - a) * w[i]) * self.scale[c]
        }))
    }

    fn to_frame(&self, v: Vec3, absolute: bool) -> Vec3 {
        if absolute {
            v
        } else {
            self.axis[0] * v.x + self.axis[1] * v.y + self.axis[2] * v.z
        }
    }

    /// Where the particle is `age_s` after its spawn
    /// (`Particle::IntegrateTotalVelocity`, Mac 0xa25fc): both velocities'
    /// integrals times the life in seconds, plus `gravity * s^2` up.
    pub fn position(&self, age_s: f32) -> Vec3 {
        let p = &self.el.prim;
        let life_s = self.life_ms * 0.001;
        let t = if life_s > 0.0 {
            (age_s / life_s).min(1.0)
        } else {
            1.0
        };
        let v1 = self.integral(Channel::VelocityX, self.vel_w, t);
        let v2 = self.integral(Channel::Velocity2X, self.vel2_w, t);
        let d = self.to_frame(v1, p.has(attr::ABSOLUTE_VEL))
            + self.to_frame(v2, p.has(attr::ABSOLUTE_VEL2));
        self.origin + d * life_s + self.up * (self.gravity * age_s * age_s)
    }

    /// The atlas cell drawn `age_ms` in, as the quad's four uvs in
    /// `make_quad`'s corner order; `None` for a one-frame material
    /// (`Particle::Update`'s sequence step, Mac 0xa7970).
    pub fn uvs(&self, age_ms: f32) -> Option<[[f32; 2]; 4]> {
        if self.frames <= 1 {
            return None;
        }
        let p = &self.el.prim;
        let mut f = (age_ms * self.rate) as u32 + self.frame0;
        if f >= self.frames {
            f = if p.sequence_loop_mode == 1
                && (p.sequence_loop_times < 1
                    || (p.sequence_loop_times as u32 + 1) * self.frames <= f)
            {
                self.frames - 1
            } else {
                f % self.frames
            };
        }
        let (rows, cols) = self.grid;
        let (cw, ch) = (1.0 / cols as f32, 1.0 / rows as f32);
        let (u0, v0) = ((f % cols) as f32 * cw, (f / cols) as f32 * ch);
        Some([[u0, v0], [u0 + cw, v0], [u0 + cw, v0 + ch], [u0, v0 + ch]])
    }

    /// Whether a sprite of this kind stretches along its travel.
    pub fn stretches(&self) -> bool {
        matches!(self.el.prim.kind, Kind::Tail | Kind::Line)
    }

    /// The frame `f` maps points and directions through, applied to the
    /// spawn point and axes (a delayed element posed on its tag).
    pub fn carry(&mut self, point: impl Fn(Vec3) -> Vec3, dir: impl Fn(Vec3) -> Vec3) {
        self.origin = point(self.origin);
        self.axis = self.axis.map(&dir);
        self.up = dir(self.up);
    }

    pub fn prim(&self) -> &Primitive {
        &self.el.prim
    }

    /// A CoD4 element's distance fade at `dist` from the camera, 0..=1:
    /// `FX_EvaluateDistanceFade` (iw3mp.exe 0x498010) takes the smaller of
    /// the fade in (1 inside its base, 0 past base plus amplitude) and one
    /// less the fade out, rounded to 255ths, and `FX_EvaluateVisualState`
    /// scales the alpha by it (cod4-effects.md 18). 1 without either range.
    pub fn distance_fade(&self, dist: f32) -> f32 {
        let [fade_in, fade_out] = self.el.prim.fade_ranges;
        if fade_in.1 == 0.0 && fade_out.1 == 0.0 {
            return 1.0;
        }
        // `FX_ClampRangeLerp` (0x497fb0).
        let lerp = |(base, amplitude): (f32, f32)| {
            let x = dist - base;
            if x < 0.0 {
                1.0
            } else if amplitude > x {
                1.0 - x / amplitude
            } else {
                0.0
            }
        };
        let i = if fade_in.1 != 0.0 { lerp(fade_in) } else { 1.0 };
        let o = if fade_out.1 != 0.0 {
            1.0 - lerp(fade_out)
        } else {
            1.0
        };
        (i.min(o) * 255.0 + 0.5).trunc() / 255.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(text: &[u8]) -> Arc<Element> {
        let p = vcod_cod2::efx::parse(text).unwrap().primitives.remove(0);
        Arc::new(Element {
            prim: p,
            atlas: HashMap::new(),
        })
    }

    const UP: [Vec3; 3] = [Vec3::Z, Vec3::X, Vec3::Y];

    /// A Particle whose x velocity runs 0 -> 100 over its life, size 4 at
    /// scale 2, alpha 1 -> 0 over a 2 s life.
    fn rising() -> Live {
        let el = element(
            b"Particle\n{\n\tlife 2000\n\tshaders\n\t[\n\t\tm\n\t]\n\
              \tvelocityX\n\t{\n\t\tcurve\n\t\t[\n\t\t\t0 0 0 0\n\t\t\t1 1 0 0\n\t\t]\n\t\tscale 100\n\t}\n\
              \tsize\n\t{\n\t\tcurve\n\t\t[\n\t\t\t0 4 0 0\n\t\t\t1 4 0 0\n\t\t]\n\t\tscale 2\n\t}\n}\n",
        );
        Live::spawn(
            el,
            &mut Rng::new(7),
            (Vec3::ZERO, UP),
            Vec3::Z,
            2000.0,
            "m",
            0,
        )
    }

    #[test]
    fn position_is_the_velocity_integral_over_the_life() {
        let l = rising();
        // v(t) = 100 t along axis[0] (world z); the integral to t = 0.5 is
        // 100 * 0.5^2 / 2 = 12.5, times the 2 s life = 25.
        let at = l.position(1.0);
        assert!((at - Vec3::new(0.0, 0.0, 25.0)).length() < 1e-3, "{at}");
        // At the end: 100 / 2 * 2 = 100.
        assert!((l.position(2.0).z - 100.0).abs() < 1e-3);
    }

    #[test]
    fn channels_scale_their_curve() {
        let l = rising();
        assert!((l.size(0.3) - 8.0).abs() < 1e-5);
        // Init's default alpha, 1 down to 0.
        assert!((l.alpha(0.25) - 0.75).abs() < 1e-5);
    }

    #[test]
    fn gravity_is_g_s_squared_without_a_half() {
        let mut l = rising();
        l.gravity = -100.0;
        // 25 up from the velocity, -100 * 1^2 from gravity.
        assert!((l.position(1.0).z - (25.0 - 100.0)).abs() < 1e-3);
    }

    #[test]
    fn a_random_weight_blends_towards_the_rand_curve() {
        let el = element(
            b"Particle\n{\n\tflags useRandomAlpha\n\tshaders\n\t[\n\t\tm\n\t]\n\
              \talpha\n\t{\n\t\tcurve\n\t\t[\n\t\t\t0 0 0 0\n\t\t\t1 0 0 0\n\t\t]\n\t}\n\
              \talphaRand\n\t{\n\t\tcurve\n\t\t[\n\t\t\t0 1 0 0\n\t\t\t1 1 0 0\n\t\t]\n\t}\n}\n",
        );
        let mut l = Live::spawn(
            el,
            &mut Rng::new(3),
            (Vec3::ZERO, UP),
            Vec3::Z,
            100.0,
            "m",
            0,
        );
        assert!((0.0..1.0).contains(&l.weight[1]));
        l.weight[1] = 0.25;
        assert!((l.alpha(0.5) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn atlas_frames_step_and_wrap() {
        let mut el =
            element(b"Particle\n{\n\tsequenceFixedFpsValue 10\n\tshaders\n\t[\n\t\tm\n\t]\n}\n");
        Arc::get_mut(&mut el)
            .unwrap()
            .atlas
            .insert("m".into(), (2, 2));
        let l = Live::spawn(
            el,
            &mut Rng::new(1),
            (Vec3::ZERO, UP),
            Vec3::Z,
            1000.0,
            "m",
            0,
        );
        // 10 fps: frame 1 (top right) at 150 ms, frame 2 (bottom left) at
        // 250, back to 0 at 450 (loop mode 0 wraps).
        assert_eq!(l.uvs(150.0).unwrap()[0], [0.5, 0.0]);
        assert_eq!(l.uvs(250.0).unwrap()[0], [0.0, 0.5]);
        assert_eq!(l.uvs(450.0).unwrap()[0], [0.0, 0.0]);
    }

    /// Every stock effect through the fx system: spawned at a point and
    /// on a surface, stepped through four seconds, drawn each step. Only
    /// the file the engine refuses fails to load.
    #[test]
    fn stock_effects_run_through_the_sim() {
        use crate::fx::sim::{FxSystem, SpawnAt};
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let names: Vec<String> = fs
            .names_with_suffix(".efx")
            .into_iter()
            .filter(|n| n.starts_with("fx/"))
            .collect();
        assert_eq!(names.len(), 375);
        let mut refused = Vec::new();
        let (mut drew, mut lit) = (0, 0);
        for n in &names {
            if let Err(e) = load(&fs, n) {
                refused.push(format!("{n}: {e}"));
                continue;
            }
            let mut fx = FxSystem::new();
            let mut now = 1.0;
            fx.spawn(&fs, n, SpawnAt::Point { pos: Vec3::ZERO }, now);
            let at = SpawnAt::Surface {
                pos: Vec3::new(64.0, 0.0, 0.0),
                normal: Vec3::Z,
            };
            fx.spawn(&fs, n, at, now);
            let (mut quads, mut lights) = (0, 0);
            for _ in 0..80 {
                now += 0.05;
                fx.step(0.05, now, None);
                fx.spawn_chained(&fs, now);
                let q = fx.build_quads(Vec3::new(-200.0, 0.0, 50.0), Vec3::Y, Vec3::Z, now);
                for quad in &q {
                    assert!(quad.verts.iter().flatten().all(|v| v.is_finite()), "{n}");
                    assert!(quad.rgba.iter().all(|v| (0.0..=1.0).contains(v)), "{n}");
                }
                quads += q.len();
                lights += fx.lights(Vec3::ZERO, now).len();
            }
            drew += usize::from(quads > 0);
            lit += usize::from(lights > 0);
        }
        assert_eq!(refused.len(), 1, "{refused:#?}");
        println!("{drew} effects drew quads, {lit} lit");
        assert!(drew > 300, "{drew}");
        assert!(lit > 20, "{lit}");
    }

    /// mp_downtown's snow: each `Cloud` particle draws as a cloud of
    /// flakes, never as a sprite: a 300-unit cube (`size`), 1.5-unit square
    /// flakes (`size2` under `nonUniformScale`, no `useLength`), white,
    /// within the effect's 3600-unit square.
    #[test]
    fn snow_draws_as_clouds_of_flakes() {
        use crate::fx::sim::{FxSystem, SpawnAt};
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let mut fx = FxSystem::new();
        let mut now = 1.0;
        let path = "fx/misc/snow_light_mp_downtown.efx";
        fx.spawn(&fs, path, SpawnAt::Point { pos: Vec3::ZERO }, now);
        for _ in 0..20 {
            now += 0.05;
            fx.step(0.05, now, None);
        }
        let eye = Vec3::new(0.0, 0.0, 100.0);
        assert!(fx.build_quads(eye, Vec3::NEG_Y, Vec3::Z, now).is_empty());
        let clouds = fx.clouds(eye, Vec3::NEG_Y, Vec3::Z, now);
        assert_eq!(clouds.list.len(), 12);
        for c in &clouds.list {
            assert!(c.shader.starts_with("cloud_snowflake0"), "{}", c.shader);
            assert!((c.axis[0].length() - 300.0).abs() < 0.01, "{:?}", c.axis);
            assert_eq!(c.matrix, [1.5, 0.0, 0.0, 1.5]);
            assert_eq!(&c.rgba[..3], &[1.0, 1.0, 1.0]);
            assert!(
                c.origin.x.abs() < 1900.0 && c.origin.y.abs() < 1900.0,
                "{}",
                c.origin
            );
        }
        // far to near
        let d: Vec<f32> = clouds.list.iter().map(|c| c.origin.distance(eye)).collect();
        assert!(d.windows(2).all(|w| w[0] >= w[1]), "{d:?}");
    }
}
