//! CoD4 1.7 effects in the client (docs/research/cod4-effects.md): each
//! `FxEffectDef` of the zones turned into CoD2 primitives
//! (`vcod_cod2::efx::Primitive`), whose channel curves carry CoD4's
//! sampled graphs, so vcod's fx system plays them as it plays CoD2's; and
//! `CG_EntityEvent`'s effect arms over the events a watched session fires.

use glam::Vec3;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use vcod_cod2::efx::{Channel, ChannelDef, Curve, Kind, Primitive, attr, spawn};
use vcod_cod4::assets::Assets;
use vcod_cod4::assets::fx::{Fx, FxVisual};
use vcod_cod4::fx::{ElemDef, ElemType, Range, Role, flags, roles};
use vcod_cod4::map::stub_name;
use vcod_common::net::events::byte_to_dir;

use crate::cod4_audio::WeaponAv;
use crate::fx::efx::{self, Effect};
use crate::fx::sim::{FxSound, FxSystem, SpawnAt, TagBolt, TagPose};
use crate::play::cod4::{ET_FX, ET_LOOP_FX, Fired, ent_f32, ent_u32, origin_of};
use crate::play::cod4_players::WeaponTags;
use vcod_common::asset_source::AssetSource;

/// Configstring `CS_EFFECT_NAMES` (cod4-handshake.md 5): effect `n` of
/// `EV_PLAY_FX` is slot `1598 + n`.
pub const CS_EFFECT_NAMES: u16 = 1598;
/// `CS_EFFECT_TAGS`: `playFxOnTag`'s `"%02d%s"`, the effect's number and
/// the tag, which `EV_PLAY_FX_ON_TAG`'s parm indexes (cod4-gsc.md 3.2).
pub const CS_EFFECT_TAGS: u16 = 1698;

/// `CG_PlayFxOnTag`'s read of a [`CS_EFFECT_TAGS`] string: its
/// first two digits the effect's number,
/// the rest the tag. `None` for a string too short or not led by digits.
pub fn effect_on_tag(cs: &str) -> Option<(u16, &str)> {
    let digits = cs.get(..2)?;
    let tag = &cs[2..];
    let id: u16 = digits.parse().ok()?;
    (!tag.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some((id, tag))
}
/// Most spawns a looping element is given when another effect's runner
/// plays it through the fx system's chains (cod4-effects.md 5).
const LOOP_SPAWNS: i32 = 8;
/// Most looping elements running at once; a new one past it ends the
/// oldest.
const MAX_LOOPERS: usize = 1024;
/// The longest update `FX_SpawnLoopingElems` runs whole; a longer one
/// starts its spawns late, seconds.
const CATCH_UP_S: f64 = 0.128;

/// The key the fx system holds effect `name` under, as its chains name
/// it (`efx::chained_path`); asset names are case-blind (`FX_Register`).
pub fn key(name: &str) -> String {
    efx::chained_path(&stub_name(name).to_ascii_lowercase())
}

/// The key of effect `key`'s one-shot elements alone.
fn one_shot_key(key: &str) -> String {
    format!("{key}#oneshot")
}

/// The key of effect `key`'s looping element `i`, one element a spawn.
fn loop_key(key: &str, i: usize) -> String {
    format!("{key}#loop{i}")
}

fn ends(r: Range) -> (f32, f32) {
    r.ends()
}

fn degrees(r: Range) -> (f32, f32) {
    let (a, b) = r.ends();
    (a.to_degrees(), b.to_degrees())
}

/// A channel and its rand curve, keys `(i / n, v)`.
fn put(p: &mut Primitive, c: Channel, base: &[f32], rand: &[f32]) {
    let curve = |vals: &[f32]| {
        let n = (vals.len().max(2) - 1) as f32;
        let keys: Vec<f32> = if vals.len() == 1 {
            vec![0.0, vals[0], 1.0, vals[0]]
        } else {
            vals.iter()
                .enumerate()
                .flat_map(|(i, v)| [i as f32 / n, *v])
                .collect()
        };
        Curve::new(1, &keys)
    };
    p.channels[c as usize] = ChannelDef {
        curve: curve(base),
        scale: (1.0, 1.0),
    };
    p.channels[c as usize + 1] = ChannelDef {
        curve: curve(rand),
        scale: (1.0, 1.0),
    };
}

/// The colour channel pair from the visual samples' bytes.
fn put_rgb(p: &mut Primitive, base: &[[f32; 3]], rand: &[[f32; 3]]) {
    let curve = |vals: &[[f32; 3]]| {
        let n = (vals.len().max(2) - 1) as f32;
        let keys: Vec<f32> = vals
            .iter()
            .enumerate()
            .flat_map(|(i, v)| [i as f32 / n, v[0], v[1], v[2]])
            .collect();
        Curve::new(3, &keys)
    };
    p.channels[Channel::Rgb as usize].curve = curve(base);
    p.channels[Channel::RgbRand as usize].curve = curve(rand);
}

/// What an element converts as, or why it does not.
pub enum Converted {
    Primitive(Box<Primitive>, (u32, u32)),
    /// An element vcod's fx system has no counterpart for (its type).
    Gap(&'static str),
}

/// How a looping element converts (cod4-effects.md 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Looping {
    /// One play spawns its first [`LOOP_SPAWNS`] intervals: what another
    /// effect's runner starts through the fx system's chains.
    Spread,
    /// One element a spawn; [`Cod4Effects`] spawns it every interval.
    Each,
}

/// Element `index` of effect `fx` as a CoD2 primitive and its atlas grid
/// `(rows, columns)` (cod4-effects.md 4).
pub fn primitive(
    a: &Assets,
    fx: &Fx,
    role: Role,
    d: &ElemDef,
    e_index: usize,
    looping: Looping,
) -> Converted {
    let e = &fx.elems[e_index];
    let Some(t) = d.elem_type else {
        return Converted::Gap("unknown type");
    };
    let kind = match t {
        ElemType::BillboardSprite => Kind::Particle,
        ElemType::OrientedSprite => Kind::OrientedParticle,
        ElemType::Tail => Kind::Tail,
        ElemType::Cloud => Kind::Cloud,
        ElemType::Model => Kind::Emitter,
        ElemType::OmniLight | ElemType::SpotLight => Kind::Light,
        ElemType::Decal => Kind::Decal,
        ElemType::Runner => Kind::FxRunner,
        // `fx::cod4_trail` draws it off the element's channels.
        ElemType::Trail if e.trail.is_some() => Kind::Particle,
        ElemType::Trail => return Converted::Gap("trail"),
        ElemType::Sound => return Converted::Gap("sound"),
    };
    let mut p = Primitive::new(kind, e_index);
    p.name = fx.name.clone();
    let range_i = |(base, amp): (i32, i32)| (base as f32, (base + amp) as f32);
    p.life = range_i(d.life_span_msec);
    p.delay = range_i(d.spawn_delay_msec);
    match (role, looping) {
        (Role::OneShot, _) => p.count = range_i(d.spawn),
        (_, Looping::Each) => p.count = (1.0, 1.0),
        (_, Looping::Spread) => {
            // `(intervalMsec, count)`: one element an interval, `count` in
            // all (0x7fffffff for ever); a chained play spawns the first
            // few, spread evenly over their intervals.
            let (interval, count) = (d.spawn.0.max(1), d.spawn.1.max(1));
            let n = count.min(LOOP_SPAWNS);
            p.count = (n as f32, n as f32);
            p.delay = (p.delay.0, p.delay.0 + (n * interval) as f32);
            p.spawn_flags |= spawn::EVEN_DISTRIBUTION;
        }
    }
    // A trail is one play that drops its own cross-sections, each after
    // the element's delay (cod4-effects.md 16).
    if t == ElemType::Trail {
        p.count = (1.0, 1.0);
        p.delay = (0.0, 0.0);
    }
    // FX_CullElemForSpawn: outside `[base, base + amplitude]` of the
    // camera, when the amplitude is set.
    if d.spawn_range.amplitude != 0.0 {
        p.spawn_range = ends(d.spawn_range);
    } else {
        p.spawn_range = (0.0, 0.0);
    }
    p.origin = d.spawn_origin.map(ends);
    // Without 0x2 the offset adds to the effect's origin in the world's
    // axes, which CoD2's `cheapOrgCalc` does (cod4-effects.md 2).
    if d.flags & flags::SPAWN_RELATIVE_TO_EFFECT == 0 {
        p.spawn_flags |= spawn::CHEAP_ORG_CALC;
    }
    match d.flags & flags::SPAWN_OFFSET_MASK {
        flags::SPAWN_OFFSET_SPHERE => {
            p.spawn_flags |= spawn::ORG_ON_SPHERE;
            p.radius = ends(d.spawn_offset_radius);
        }
        flags::SPAWN_OFFSET_CYLINDER => {
            p.spawn_flags |= spawn::ORG_ON_CYLINDER;
            p.radius = ends(d.spawn_offset_radius);
            p.height = ends(d.spawn_offset_height);
        }
        _ => {}
    }
    p.angle = d.spawn_angles.map(degrees);
    // Radians per ms to degrees per second.
    p.angle_delta = d.angular_velocity.map(|r| {
        let (a, b) = degrees(r);
        (a * 1000.0, b * 1000.0)
    });
    p.rotation = degrees(d.initial_rotation);
    // `0.5 * gravity * 800 * t^2` down (FX_NextElementPosition).
    let (g0, g1) = ends(d.gravity);
    p.gravity = (-400.0 * g0, -400.0 * g1);
    if d.flags & flags::USE_COLLISION != 0 {
        p.bounce = Some(ends(d.reflection_factor));
    }
    if d.flags & flags::RUN_MASK >= flags::RUN_RELATIVE_TO_EFFECT {
        p.flags |= attr::RELATIVE;
    }
    if d.flags & flags::SPAWN_FRUSTUM_CULL != 0 {
        p.spawn_flags |= spawn::FRUSTUM_CULL;
        p.spawn_frustum_cull_radius = d.spawn_frustum_cull_radius;
    }
    p.fade_ranges = [d.fade_in_range, d.fade_out_range].map(|r| (r.base, r.amplitude));
    p.non_uniform_scale = d.flags & flags::NONUNIFORM_SCALE != 0;
    p.flags |= attr::USE_ALPHA
        | attr::USE_RANDOM_COLORS
        | attr::USE_RANDOM_ALPHA
        | attr::USE_RANDOM_SIZE
        | attr::USE_RANDOM_SIZE2
        | attr::USE_RANDOM_LENGTH
        | attr::USE_RANDOM_ROTATION_DELTA
        | attr::USE_RANDOM_VELOCITY
        | attr::USE_RANDOM_VELOCITY2
        | attr::ABSOLUTE_VEL2;
    // The visual graph.
    if !d.vis.is_empty() {
        let n = d.vis_intervals().max(1) as f32;
        // The colour bytes are a `D3DCOLOR` (B, G, R, A), and the
        // amplitude's colour is the far end of the random lerp, not an
        // offset (cod4-effects.md 4).
        let col = |s: &vcod_cod4::fx::VisSample, k: usize, rand: bool| {
            let c = if rand {
                s.amplitude.color
            } else {
                s.base.color
            };
            f32::from(c[k]) / 255.0
        };
        let rgb = |rand: bool| -> Vec<[f32; 3]> {
            d.vis
                .iter()
                .map(|s| [2, 1, 0].map(|k| col(s, k, rand)))
                .collect()
        };
        put_rgb(&mut p, &rgb(false), &rgb(true));
        let ch = |f: &dyn Fn(&vcod_cod4::fx::VisState) -> f32| {
            let base: Vec<f32> = d.vis.iter().map(|s| f(&s.base)).collect();
            let rand: Vec<f32> = d.vis.iter().map(|s| f(&s.base) + f(&s.amplitude)).collect();
            (base, rand)
        };
        let (a, ar) = (
            d.vis.iter().map(|s| col(s, 3, false)).collect::<Vec<_>>(),
            d.vis.iter().map(|s| col(s, 3, true)).collect::<Vec<_>>(),
        );
        put(&mut p, Channel::Alpha, &a, &ar);
        let (s0, s0r) = ch(&|v| v.size[0]);
        let (s1, s1r) = ch(&|v| v.size[1]);
        if kind == Kind::Cloud {
            // `FX_DrawElem_Cloud`: the cloud's cube is the visual state's
            // scale, each flake `size[0]` wide and `size[1]` long along
            // the element's velocity (cod4-effects.md 5).
            let (sc, scr) = ch(&|v| v.scale);
            put(&mut p, Channel::Size, &sc, &scr);
            put(&mut p, Channel::Size2, &s0, &s0r);
            put(&mut p, Channel::Length, &s1, &s1r);
            p.non_uniform_scale = true;
            p.use_length = true;
        } else {
            put(&mut p, Channel::Size, &s0, &s0r);
            put(&mut p, Channel::Size2, &s1, &s1r);
            put(&mut p, Channel::Length, &s1, &s1r);
        }
        // Radians per ms of life over the interval count, to degrees per
        // 100 ms.
        let (r, rr) = ch(&|v| v.rotation_delta * n * 100.0 * (180.0 / std::f32::consts::PI));
        put(&mut p, Channel::RotationDelta, &r, &rr);
    }
    // The velocity graphs: units per ms over the interval count, to units
    // per second of life (cod4-effects.md 3).
    if !d.vel.is_empty() {
        let n = d.vel_intervals().max(1) as f32 * 1000.0;
        for (first, world) in [(Channel::VelocityX, false), (Channel::Velocity2X, true)] {
            for k in 0..3 {
                let r = |s: &vcod_cod4::fx::VelSample| {
                    let v = if world { s.world } else { s.local };
                    v.velocity[k]
                };
                let base: Vec<f32> = d.vel.iter().map(|s| r(s).base * n).collect();
                let rand: Vec<f32> = d
                    .vel
                    .iter()
                    .map(|s| (r(s).base + r(s).amplitude) * n)
                    .collect();
                let c = first as usize + k;
                p.channels[c] = ChannelDef {
                    curve: Curve::new(1, &interleave(&base)),
                    scale: (1.0, 1.0),
                };
                p.channels[c + 3] = ChannelDef {
                    curve: Curve::new(1, &interleave(&rand)),
                    scale: (1.0, 1.0),
                };
            }
        }
    }
    // Sequences: `FxElemAtlas` (cod4-effects.md 4).
    let at = d.atlas;
    p.sequence_start_frame_mode = u32::from(at.behavior & 3);
    p.sequence_fixed_frame = i32::from(at.index) + 1;
    if at.behavior & 4 != 0 {
        p.sequence_play_rate_mode = 1;
    } else {
        p.sequence_play_rate_mode = 0;
        p.sequence_fixed_fps = f32::from(at.fps);
    }
    if at.behavior & 8 != 0 {
        p.sequence_loop_mode = 1;
        p.sequence_loop_times = i32::from(at.loop_count);
    }
    let grid = (1u32 << at.row_bits.min(8), 1u32 << at.col_bits.min(8));
    // Visuals.
    for v in &e.visuals {
        match v {
            FxVisual::Material(Some(id)) | FxVisual::Mark([Some(id), _]) => p
                .materials
                .push(stub_name(&a.materials[id.index()].name).to_string()),
            FxVisual::Model(Some(id)) => {
                p.models
                    .push(stub_name(&a.xmodels[id.index()].name).to_string());
                p.flags |= attr::USE_MODEL;
            }
            FxVisual::Effect(Some(n)) => p.play_fx.push(n.clone()),
            _ => {}
        }
    }
    let named = |n: &Option<String>| n.iter().filter(|s| !s.is_empty()).cloned().collect();
    p.impact_fx = named(&e.effect_on_impact);
    if !p.impact_fx.is_empty() {
        p.flags |= attr::IMPACT_FX;
    }
    p.death_fx = named(&e.effect_on_death);
    if !p.death_fx.is_empty() {
        p.flags |= attr::DEATH_FX;
    }
    p.emit_fx = named(&e.effect_emitted);
    if !p.emit_fx.is_empty() {
        p.flags |= attr::EMIT_FX;
    }
    Converted::Primitive(Box::new(p), grid)
}

/// Values at `i / (len - 1)` as curve keys.
fn interleave(vals: &[f32]) -> Vec<f32> {
    if vals.len() < 2 {
        let v = vals.first().copied().unwrap_or(0.0);
        return vec![0.0, v, 1.0, v];
    }
    let n = (vals.len() - 1) as f32;
    vals.iter()
        .enumerate()
        .flat_map(|(i, v)| [i as f32 / n, *v])
        .collect()
}

/// A looping element's schedule (cod4-effects.md 5): one spawn every
/// `interval_ms` from the effect's start, `count` in all (`i32::MAX` for
/// ever); `reach_ms` is its longest delay plus life, past which a spawn
/// shows nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopSpec {
    pub interval_ms: i32,
    pub count: i32,
    pub reach_ms: i32,
}

/// An effect converted: whole (looping elements [`Looping::Spread`]), its
/// one-shot elements alone, and each looping element alone
/// ([`Looping::Each`]) with its schedule.
pub struct Parts {
    pub full: Effect,
    pub one_shot: Effect,
    pub loops: Vec<(Effect, LoopSpec)>,
    /// The element types left out.
    pub gaps: Vec<&'static str>,
}

/// The angle falloff of a trail's material, when its set is a falloff one
/// (`effect_falloff`, `effect_*falloff_add*`): the material's
/// `falloffParms`, `falloffBeginColor` and `falloffEndColor` constants
/// (cod4-effects.md 16).
fn trail_falloff(
    a: &Assets,
    e: &vcod_cod4::assets::fx::FxElem,
) -> Option<crate::fx::cod4_trail::Falloff> {
    let m = e.visuals.iter().find_map(|v| match v {
        FxVisual::Material(Some(id)) => Some(&a.materials[id.index()]),
        _ => None,
    })?;
    let set = &a.techsets[m.techset?.index()].name;
    if !set.contains("falloff") || set.contains("distfalloff") {
        return None;
    }
    let constant = |name: &[u8]| {
        m.constants
            .iter()
            .find(|c| c.name.starts_with(name))
            .map(|c| c.literal)
    };
    let parms = constant(b"falloffParms")?;
    Some(crate::fx::cod4_trail::Falloff {
        parms: [parms[2], parms[3]],
        begin: constant(b"falloffBegin").unwrap_or([1.0; 4]),
        end: constant(b"falloffEndCo").unwrap_or([1.0; 4]),
    })
}

/// A converted primitive of element `d` (`e` its zone element) as the fx
/// system's emitter.
fn emitter(
    a: &Assets,
    p: Primitive,
    grid: (u32, u32),
    d: &ElemDef,
    e: &vcod_cod4::assets::fx::FxElem,
) -> crate::fx::efx::Emitter {
    let fs = vcod_common::pk3::Pk3Fs::empty();
    let materials = p.materials.clone();
    let range_i = |(base, amp): (i32, i32)| crate::fx::efx::R1 {
        a: base as f32,
        b: (base + amp) as f32,
    };
    let mut em = crate::fx::cod2::to_emitter(p, &fs);
    if d.elem_type == Some(ElemType::Trail)
        && let Some(t) = &e.trail
    {
        em.cod4_trail = Some(crate::fx::cod4_trail::TrailDef {
            def: Arc::new(t.clone()),
            life_ms: range_i(d.life_span_msec),
            delay_ms: range_i(d.spawn_delay_msec),
            falloff: trail_falloff(a, e),
        });
    }
    if !em.emit_fx.is_empty() {
        let (a, b) = ends(d.emit_dist);
        em.cod4_emit = Some(crate::fx::efx::Cod4Emit {
            dist: crate::fx::efx::R1 { a, b },
            variance_base: d.emit_dist_variance.base,
            variance_amp: d.emit_dist_variance.amplitude,
        });
    }
    // The runner names effects as the chains look them up.
    em.play_fx = em.play_fx.iter().map(|n| key(n)).collect();
    em.impact_fx = em.impact_fx.iter().map(|n| key(n)).collect();
    em.death_fx = em.death_fx.iter().map(|n| key(n)).collect();
    em.emit_fx = em.emit_fx.iter().map(|n| key(n)).collect();
    if let Some(el) = em.cod2.take() {
        let el = Arc::try_unwrap(el).unwrap_or_else(|a| crate::fx::cod2::Element {
            prim: a.prim.clone(),
            atlas: a.atlas.clone(),
        });
        em.cod2 = Some(Arc::new(crate::fx::cod2::Element {
            prim: el.prim,
            atlas: materials.into_iter().map(|m| (m, grid)).collect(),
        }));
    }
    em
}

/// Effect `fx` as the fx system's, and the element types left out.
#[cfg(test)]
pub fn convert(a: &Assets, fx: &Fx) -> (Effect, Vec<&'static str>) {
    let p = parts(a, fx);
    (p.full, p.gaps)
}

/// Effect `fx` in [`Parts`].
pub fn parts(a: &Assets, fx: &Fx) -> Parts {
    let mut out = Parts {
        full: Effect {
            emitters: Vec::new(),
        },
        one_shot: Effect {
            emitters: Vec::new(),
        },
        loops: Vec::new(),
        gaps: Vec::new(),
    };
    for (i, (role, e)) in roles(fx).enumerate() {
        // The emission elements are copies of the one-shot elements of an
        // element's `effectEmitted`, which plays as its own effect; no
        // play of this one spawns them (`FX_TriggerOneShot` covers the
        // looping and one-shot ranges only, cod4-effects.md 15).
        if role == Role::Emitted {
            continue;
        }
        let d = ElemDef::of(e);
        match primitive(a, fx, role, &d, i, Looping::Spread) {
            Converted::Primitive(p, grid) => {
                let em = emitter(a, *p, grid, &d, e);
                if role == Role::OneShot {
                    out.one_shot.emitters.push(em.clone());
                }
                out.full.emitters.push(em);
            }
            Converted::Gap(why) => {
                out.gaps.push(why);
                continue;
            }
        }
        // A looping trail drops sections by distance, not by interval: its
        // one play starts with the effect's one-shot elements.
        if role == Role::Looping && d.elem_type == Some(ElemType::Trail) {
            if let Some(em) = out.full.emitters.last() {
                out.one_shot.emitters.push(em.clone());
            }
            continue;
        }
        if role == Role::Looping
            && let Converted::Primitive(p, grid) = primitive(a, fx, role, &d, i, Looping::Each)
        {
            let spec = LoopSpec {
                interval_ms: d.spawn.0.max(1),
                count: d.spawn.1.max(0),
                reach_ms: d.spawn_delay_msec.0.max(0)
                    + d.spawn_delay_msec.1.max(0)
                    + d.life_span_msec.0.max(0)
                    + d.life_span_msec.1.max(0),
            };
            out.loops.push((
                Effect {
                    emitters: vec![emitter(a, *p, grid, &d, e)],
                },
                spec,
            ));
        }
    }
    out
}

/// The impact table's names: `[fxType][surface]` for what is not flesh,
/// `[fxType][fleshType]` for flesh.
#[derive(Clone, Debug, Default)]
pub struct ImpactTable {
    pub nonflesh: Vec<Vec<Option<String>>>,
    pub flesh: Vec<Vec<Option<String>>>,
    /// `cg_blood` 0: a flesh hit plays [`FLESH_NOBLOOD`].
    pub no_blood: bool,
}

/// The flesh hit `cg_blood` 0 plays in place of the table's
/// (iw3mp.exe 0x43efd1 registers it, 0x458ad9 swaps it in;
/// cod4-effects.md 6).
pub const FLESH_NOBLOOD: &str = "impacts/flesh_hit_noblood";

/// One effect an event, an entity or the map plays: its key in the fx
/// system, where, and the tag it rides (cod4-effects.md 10).
#[derive(Clone, Debug)]
pub struct Play {
    pub key: String,
    pub at: SpawnAt,
    pub bolt: Option<TagBolt>,
}

/// The tags a CoD4 effect is bolted to, keyed in the fx system's
/// [`TagBolt`] by entity number and one of these slots.
pub mod slot {
    /// A drawn body's held weapon's `tag_flash` and `tag_brass`.
    pub const FLASH: i32 = 0;
    pub const BRASS: i32 = 1;
    /// The view weapon's, keyed by the drawn view's client.
    pub const VIEW_FLASH: i32 = 2;
    pub const VIEW_BRASS: i32 = 3;
}

/// A looping element running (`FX_SpawnLoopingElems`): spawn `next` falls
/// at `begin + next * interval`.
#[derive(Clone, Debug)]
struct Looper {
    key: String,
    spec: LoopSpec,
    at: SpawnAt,
    bolt: Option<TagBolt>,
    /// The effect's start, on the fx clock (seconds).
    begin: f64,
    next: i32,
    /// The `ET_FX` / `ET_LOOP_FX` entity that started it.
    owner: Option<u32>,
}

/// An `ET_FX` or `ET_LOOP_FX` entity's state (`cent->pose.fx`): the
/// `time2` its effect started at, or the next retrigger.
#[derive(Clone, Copy, Debug)]
struct EntFx {
    trigger: i32,
}

/// What a watched session plays effects from: the zones' effects
/// converted, the impact table and the weapons, the looping elements
/// running and the map's createfx ambience.
pub(crate) struct Cod4Effects {
    /// Waiting for the fx system, which takes them on the first frame.
    effects: Option<HashMap<String, Effect>>,
    names: HashSet<String>,
    /// The looping elements' schedules by effect key; the `i`th plays as
    /// [`loop_key`].
    loops: HashMap<String, Vec<LoopSpec>>,
    impacts: ImpactTable,
    pub(crate) weapons: Arc<HashMap<String, WeaponAv>>,
    /// Element types no effect played, counted by kind.
    pub(crate) gaps: HashMap<&'static str, usize>,
    /// Effects the events played that a zone holds, and the names of
    /// those none does.
    pub(crate) spawned: usize,
    pub(crate) missing: HashSet<String>,
    /// The effects started, by name: the events' and
    /// [`Self::play_named`]'s.
    pub(crate) named: HashSet<String>,
    loopers: Vec<Looper>,
    /// Looping spawns made, for the gates.
    pub(crate) loop_spawns: usize,
    /// This frame's bolted tags.
    tags: HashMap<TagBolt, TagPose>,
    ents: HashMap<u32, EntFx>,
    /// The map's createfx effects (`CG_StartClientSideEffects`), started
    /// on the first frame, and its client sounds.
    ambience: Option<Vec<(Play, f32)>>,
    pub(crate) client_sounds: Vec<(Vec3, String)>,
    /// The tracer rolls' `rand()` state, and tracers spawned.
    rand: u32,
    pub(crate) tracers: usize,
}

impl Cod4Effects {
    /// Every effect of `zones` (the first holding one whole wins), the
    /// impact table of the first zone carrying one, and the createfx
    /// ambience of `map`.
    pub(crate) fn new(
        zones: &[&Assets],
        weapons: Arc<HashMap<String, WeaponAv>>,
        map: &str,
    ) -> Cod4Effects {
        let mut effects = HashMap::new();
        let mut loops = HashMap::new();
        let mut gaps: HashMap<&'static str, usize> = HashMap::new();
        for a in zones {
            for fx in a.fx.iter().filter(|f| !f.name.starts_with(',')) {
                let k = key(&fx.name);
                if effects.contains_key(&k) {
                    continue;
                }
                let p = parts(a, fx);
                for why in p.gaps {
                    *gaps.entry(why).or_default() += 1;
                }
                if !p.loops.is_empty() {
                    let mut specs = Vec::new();
                    for (i, (e, spec)) in p.loops.into_iter().enumerate() {
                        effects.insert(loop_key(&k, i), e);
                        specs.push(spec);
                    }
                    effects.insert(one_shot_key(&k), p.one_shot);
                    loops.insert(k.clone(), specs);
                }
                effects.insert(k, p.full);
            }
        }
        let impacts = zones
            .iter()
            .find_map(|a| {
                let t = a.impact_fx.iter().find(|t| !t.table.is_empty())?;
                let name = |id: &Option<vcod_cod4::assets::Id<Fx>>| {
                    id.map(|id| stub_name(&a.fx[id.index()].name).to_string())
                };
                Some(ImpactTable {
                    nonflesh: t
                        .table
                        .iter()
                        .map(|e| e.nonflesh.iter().map(name).collect())
                        .collect(),
                    flesh: t
                        .table
                        .iter()
                        .map(|e| e.flesh.iter().map(name).collect())
                        .collect(),
                    no_blood: false,
                })
            })
            .unwrap_or_default();
        let (ambience, client_sounds) = createfx(zones, map);
        log::info!(
            "cod4: {} effects, {} createfx, element types left out {gaps:?}",
            effects.len(),
            ambience.len()
        );
        Cod4Effects {
            names: effects.keys().cloned().collect(),
            effects: Some(effects),
            loops,
            impacts,
            weapons,
            gaps,
            spawned: 0,
            missing: HashSet::new(),
            named: HashSet::new(),
            loopers: Vec::new(),
            loop_spawns: 0,
            tags: HashMap::new(),
            ents: HashMap::new(),
            ambience: Some(ambience),
            client_sounds,
            rand: 1,
            tracers: 0,
        }
    }

    /// `cg_blood`, read on every hit.
    pub(crate) fn set_blood(&mut self, on: bool) {
        self.impacts.no_blood = !on;
    }

    /// The effects went to the fx system ([`Self::take_effects`]).
    #[cfg(test)]
    pub(crate) fn taken(&self) -> bool {
        self.effects.is_none()
    }

    pub(crate) fn take_effects(&mut self) -> Option<HashMap<String, Effect>> {
        self.effects.take()
    }

    /// `f`'s effects (keys the fx system holds), where they play and the
    /// tag each rides, counted into [`Self::spawned`] and
    /// [`Self::missing`].
    pub(crate) fn plays(
        &mut self,
        f: &Fired,
        weapons: &[String],
        configstring: &dyn Fn(u16) -> String,
        tags: &HashMap<u32, WeaponTags>,
        view: Option<&(u32, WeaponTags)>,
    ) -> Vec<Play> {
        let mut out = Vec::new();
        let fx = event_effects(
            f,
            weapons,
            &self.weapons,
            &self.impacts,
            configstring,
            tags,
            view,
        );
        for (name, at, bolt) in fx {
            match self.resolve(&name) {
                Some(key) => {
                    self.spawned += 1;
                    self.named.insert(name);
                    out.push(Play { key, at, bolt });
                }
                None => {
                    self.missing.insert(name);
                }
            }
        }
        out
    }

    /// `name` as a play at `at` on `bolt`, counted into [`Self::spawned`]
    /// or [`Self::missing`]: the cgame's own effects (a helicopter's smoke
    /// and wash) and `EV_PLAY_FX_ON_TAG`'s.
    pub(crate) fn play_named(
        &mut self,
        name: &str,
        at: SpawnAt,
        bolt: Option<TagBolt>,
    ) -> Option<Play> {
        match self.resolve(name) {
            Some(key) => {
                self.spawned += 1;
                self.named.insert(name.to_string());
                Some(Play { key, at, bolt })
            }
            None => {
                self.missing.insert(name.to_string());
                None
            }
        }
    }

    /// Adds `extra` to this frame's bolted tags (a drawn model's, which
    /// `cod4_vehicles` poses) and hands the lot to the fx system.
    pub(crate) fn add_tags(&mut self, fx: &mut FxSystem, extra: &HashMap<TagBolt, TagPose>) {
        if extra.is_empty() {
            return;
        }
        self.tags.extend(extra.iter().map(|(&b, &p)| (b, p)));
        fx.set_tag_bolts(&self.tags);
    }

    /// The fx system's key of effect `name`, when a zone holds it.
    fn resolve(&self, name: &str) -> Option<String> {
        let k = key(name);
        self.names.contains(&k).then_some(k)
    }

    /// This frame's poses of the bolted tags, handed on to the fx system:
    /// an effect's elements that move with it follow the tag, and a
    /// looping one on a tag gone ends (cod4-effects.md 10).
    pub(crate) fn set_tags(
        &mut self,
        fx: &mut FxSystem,
        bodies: &HashMap<u32, WeaponTags>,
        view: Option<&(u32, WeaponTags)>,
    ) {
        self.tags = tag_poses(bodies, view);
        fx.set_tag_bolts(&self.tags);
    }

    /// Starts `play` at `begin` on the fx clock (seconds; `now` is the
    /// clock): its one-shot elements, and a [`Looper`] for each looping
    /// one, which [`Self::run_loops`] spawns. A start in the past spawns
    /// only the loop intervals whose elements could still show
    /// (`FX_LimitStabilizeTimeForElemDef`'s catch-up).
    pub(crate) fn start(
        &mut self,
        fx: &mut FxSystem,
        fs: &dyn AssetSource,
        play: &Play,
        begin: f64,
        owner: Option<u32>,
    ) -> Vec<FxSound> {
        let Some(specs) = self.loops.get(&play.key) else {
            return self.spawn_one(fx, fs, &play.key, play.at, play.bolt, begin as f32);
        };
        let specs = specs.clone();
        let sounds = self.spawn_one(
            fx,
            fs,
            &one_shot_key(&play.key),
            play.at,
            play.bolt,
            begin as f32,
        );
        for (i, spec) in specs.into_iter().enumerate() {
            if spec.count <= 0 {
                continue;
            }
            if self.loopers.len() >= MAX_LOOPERS {
                self.loopers.remove(0);
            }
            self.loopers.push(Looper {
                key: loop_key(&play.key, i),
                spec,
                at: play.at,
                bolt: play.bolt,
                begin,
                next: 0,
                owner,
            });
        }
        sounds
    }

    /// One play of `key` at `at` on the fx clock `t`, on `bolt` when its
    /// tag poses this frame.
    fn spawn_one(
        &mut self,
        fx: &mut FxSystem,
        fs: &dyn AssetSource,
        key: &str,
        at: SpawnAt,
        bolt: Option<TagBolt>,
        t: f32,
    ) -> Vec<FxSound> {
        match bolt.and_then(|b| Some((b, *self.tags.get(&b)?))) {
            Some((b, pose)) => fx.spawn_on_tag(fs, key, b, pose, t),
            None => fx.spawn(fs, key, at, t),
        }
    }

    /// The looping elements' spawns due by `now` (fx clock, seconds). A
    /// looper ends past its count, or once its tag stops posing.
    pub(crate) fn run_loops(
        &mut self,
        fx: &mut FxSystem,
        fs: &dyn AssetSource,
        now: f64,
    ) -> Vec<FxSound> {
        let mut sounds = Vec::new();
        let mut loopers = std::mem::take(&mut self.loopers);
        loopers.retain_mut(|l| {
            if l.bolt.is_some_and(|b| !self.tags.contains_key(&b)) {
                return false;
            }
            let interval = f64::from(l.spec.interval_ms) / 1000.0;
            // A catch-up longer than `FX_SpawnLoopingElems`' 128 ms skips
            // the spawns whose elements are already gone.
            let settled = now - l.begin - f64::from(l.spec.reach_ms) / 1000.0 - CATCH_UP_S;
            if settled > 0.0 {
                let skip = (settled / interval).floor().min(f64::from(i32::MAX)) as i32;
                l.next = l.next.max(skip.min(l.spec.count));
            }
            while l.next < l.spec.count {
                let t = l.begin + f64::from(l.next) * interval;
                if t > now {
                    break;
                }
                sounds.extend(self.spawn_one(fx, fs, &l.key, l.at, l.bolt, t as f32));
                self.loop_spawns += 1;
                l.next += 1;
            }
            l.next < l.spec.count
        });
        // Loopers started while these ran go after them.
        loopers.append(&mut self.loopers);
        self.loopers = loopers;
        sounds
    }

    /// Looping elements running, for F3 and the gates.
    pub(crate) fn loopers(&self) -> usize {
        self.loopers.len()
    }

    /// The `ET_FX` and `ET_LOOP_FX` entities of the drawn snapshot
    /// (`CG_Fx`, `CG_LoopFx`, cod4-effects.md 11): an `ET_FX` starts its
    /// effect at `time2`, again whenever `time2` changes; an `ET_LOOP_FX`
    /// starts it on first sight within `cullDist` of `eye` (0 never culls)
    /// and retriggers it every `period` ms. `render_ms` is the drawn
    /// server time, `now` the fx clock at it. An entity gone from the
    /// snapshot ends its looping elements; those spawned live out.
    pub(crate) fn entity_fx(
        &mut self,
        fx: &mut FxSystem,
        fs: &dyn AssetSource,
        ents: &[vcod_cod4::net::delta::EntityState],
        configstring: &dyn Fn(u16) -> String,
        (render_ms, now): (i32, f64),
        eye: Vec3,
    ) -> Vec<FxSound> {
        let mut sounds = Vec::new();
        let seen: HashSet<u32> = ents
            .iter()
            .filter(|e| matches!(e.etype(), ET_FX | ET_LOOP_FX))
            .map(|e| e.number())
            .collect();
        self.ents.retain(|n, _| seen.contains(n));
        self.loopers
            .retain(|l| l.owner.is_none_or(|o| seen.contains(&o)));
        let at_ms = |t: i32| now + f64::from(t - render_ms) / 1000.0;
        for e in ents {
            let (etype, num) = (e.etype(), e.number());
            if !matches!(etype, ET_FX | ET_LOOP_FX) {
                continue;
            }
            let Some(play) = self.entity_play(e, configstring) else {
                continue;
            };
            if etype == ET_FX {
                let time2 = ent_u32(e, "time2") as i32;
                if self.ents.get(&num).is_some_and(|s| s.trigger == time2) {
                    continue;
                }
                // `FX_ThroughWithEffect` on the last one.
                self.loopers.retain(|l| l.owner != Some(num));
                self.ents.insert(num, EntFx { trigger: time2 });
                sounds.extend(self.start(fx, fs, &play, at_ms(time2), Some(num)));
                continue;
            }
            let cull = ent_f32(e, "lerp.u.loopFx.cullDist");
            if cull != 0.0 && (origin_of(e) - eye).length_squared() >= cull * cull {
                continue;
            }
            let period = (ent_u32(e, "lerp.u.loopFx.period") as i32).max(1);
            let due = match self.ents.get(&num) {
                None => Some(render_ms),
                Some(s) if render_ms >= s.trigger => {
                    // Only the last of several due retriggers outlives the
                    // frame.
                    Some(s.trigger + (render_ms - s.trigger) / period * period)
                }
                Some(_) => None,
            };
            if let Some(t) = due {
                self.loopers.retain(|l| l.owner != Some(num));
                self.ents.insert(
                    num,
                    EntFx {
                        trigger: t + period,
                    },
                );
                sounds.extend(self.start(fx, fs, &play, at_ms(t), Some(num)));
            }
        }
        sounds
    }

    /// `CG_StartFx`'s effect for an entity: configstring
    /// [`CS_EFFECT_NAMES`] + `un1`, on the entity's origin and
    /// `AnglesToAxis` of its angles.
    fn entity_play(
        &mut self,
        e: &vcod_cod4::net::delta::EntityState,
        configstring: &dyn Fn(u16) -> String,
    ) -> Option<Play> {
        let id = ent_u32(e, "un1");
        let name = configstring(CS_EFFECT_NAMES + id as u16);
        if id == 0 || name.is_empty() {
            return None;
        }
        let Some(key) = self.resolve(&name) else {
            self.missing.insert(name);
            return None;
        };
        let angles = [0, 1, 2].map(|k| ent_f32(e, &format!("lerp.apos.trBase[{k}]")));
        Some(Play {
            key,
            at: on_axes(origin_of(e), angles),
            bolt: None,
        })
    }

    /// The map's createfx effects, once: each at its `delay` on the
    /// cgame's clock, `render_ms` the drawn server time and `now` the fx
    /// clock at it (`CG_ParseEffect`'s `FX_SpawnOrientedEffect`).
    pub(crate) fn start_ambience(
        &mut self,
        fx: &mut FxSystem,
        fs: &dyn AssetSource,
        (render_ms, now): (i32, f64),
    ) -> Vec<FxSound> {
        let mut sounds = Vec::new();
        for (play, delay) in self.ambience.take().unwrap_or_default() {
            if !self.names.contains(&play.key) {
                self.missing.insert(play.key);
                continue;
            }
            let begin = now + (f64::from(delay) * 1000.0 - f64::from(render_ms)) / 1000.0;
            sounds.extend(self.start(fx, fs, &play, begin, None));
        }
        sounds
    }
}

/// An effect on `AnglesToAxis(angles)` at `pos`.
fn on_axes(pos: Vec3, angles: [f32; 3]) -> SpawnAt {
    let q = vcod_common::turretpose::angles_quat(angles);
    SpawnAt::Oriented {
        pos,
        axis: [q * Vec3::X, q * Vec3::Y, q * Vec3::Z],
    }
}

/// The bolted tags' poses this frame: each drawn body's weapon tags and
/// the view weapon's.
fn tag_poses(
    bodies: &HashMap<u32, WeaponTags>,
    view: Option<&(u32, WeaponTags)>,
) -> HashMap<TagBolt, TagPose> {
    let pose = |(origin, q): (Vec3, glam::Quat)| TagPose {
        origin,
        axis: [q * Vec3::X, q * Vec3::Y, q * Vec3::Z],
    };
    let mut out = HashMap::new();
    let mut put = |entity: u32, t: &WeaponTags, (flash, brass): (i32, i32)| {
        if let Some(f) = t.flash {
            out.insert(
                TagBolt {
                    entity,
                    slot: flash,
                },
                pose(f),
            );
        }
        if let Some(b) = t.brass {
            out.insert(
                TagBolt {
                    entity,
                    slot: brass,
                },
                pose(b),
            );
        }
    };
    for (&n, t) in bodies {
        put(n, t, (slot::FLASH, slot::BRASS));
    }
    if let Some((c, t)) = view {
        put(*c, t, (slot::VIEW_FLASH, slot::VIEW_BRASS));
    }
    out
}

/// The map's createfx effects with their delays (seconds), and its client
/// sounds.
type Ambience = (Vec<(Play, f32)>, Vec<(Vec3, String)>);

/// The createfx effects of `map` (`maps/mp/<map>_fx.gsc` names them,
/// `maps/createfx/<map>_fx.gsc` places them) with their delays, and its
/// client sounds; the first zone holding the rawfiles wins.
fn createfx(zones: &[&Assets], map: &str) -> Ambience {
    use vcod_cod4::fx::createfx;
    let raw = |name: String| {
        zones.iter().find_map(|a| {
            let r = a
                .raw_files
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(&name))?;
            Some(createfx::text(&r.data))
        })
    };
    let (Some(names), Some(placed)) = (
        raw(format!("maps/mp/{map}_fx.gsc")),
        raw(format!("maps/createfx/{map}_fx.gsc")),
    ) else {
        return (Vec::new(), Vec::new());
    };
    let names = createfx::effect_map(&names);
    let placed = createfx::placed(&placed);
    if let Some(e) = &placed.error {
        log::warn!("cod4: maps/createfx/{map}_fx.gsc: {e}");
    }
    let plays = placed
        .effects
        .iter()
        .filter_map(|e| {
            let Some((_, file)) = names.iter().find(|(n, _)| *n == e.fxid) else {
                log::warn!("cod4: createfx: no _fx.gsc mapping for {:?}", e.fxid);
                return None;
            };
            let play = Play {
                key: key(file),
                at: on_axes(Vec3::from(e.origin), e.angles),
                bolt: None,
            };
            Some((play, e.delay))
        })
        .collect();
    let sounds = placed
        .sounds
        .into_iter()
        .map(|(o, a)| (Vec3::from(o), a))
        .collect();
    (plays, sounds)
}

/// `CG_ImpactEffectForWeapon`'s effect: the table
/// row by the weapon's impact type, the flesh column
/// for surface 7.
fn impact_effect(t: &ImpactTable, impact_type: u32, surf: u32, flags: u32) -> Option<String> {
    let exit = u32::from(flags & 4 != 0) as usize;
    let row = match impact_type {
        1 => exit,
        2 => 2 + exit,
        3 => 6 + exit,
        4 => 4 + exit,
        5 => 8,
        6 => 9,
        7 => 10,
        8 => 11,
        _ => return None,
    };
    if surf == 7 {
        let col = match (flags & 2 != 0, flags & 1 != 0) {
            (true, true) => 3,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 0,
        };
        let name = t.flesh.get(row)?.get(col)?.clone()?;
        Some(if t.no_blood {
            FLESH_NOBLOOD.to_string()
        } else {
            name
        })
    } else {
        t.nonflesh.get(row)?.get(surf as usize)?.clone()
    }
}

/// An effect axis from a forward direction.
fn directed(pos: Vec3, dir: Vec3) -> SpawnAt {
    if dir.length_squared() > 0.0 {
        SpawnAt::Directed { pos, dir }
    } else {
        SpawnAt::Point { pos }
    }
}

/// Height over a body's origin its weapon flash plays at when no posed
/// body gives a `tag_flash` (a standing player's eye).
const FLASH_HEIGHT: f32 = 56.0;

/// An effect on a tag's own axes.
fn on_tag((pos, q): (Vec3, glam::Quat)) -> SpawnAt {
    SpawnAt::Oriented {
        pos,
        axis: [q * Vec3::X, q * Vec3::Y, q * Vec3::Z],
    }
}

/// `CG_EntityEvent`'s effect arms for
/// one fired event: names as the zones and configstrings spell them.
/// `view` is the drawn view weapon's client and its tags in the world: a
/// shot of that client (its playerstate's ring or its entity) flashes and
/// ejects the weapon's view effects on them (the flash step,
/// `CG_EjectWeaponBrass`, cod4-effects.md 10).
pub fn event_effects(
    f: &Fired,
    weapons: &[String],
    defs: &HashMap<String, WeaponAv>,
    impacts: &ImpactTable,
    configstring: &dyn Fn(u16) -> String,
    tags: &HashMap<u32, WeaponTags>,
    view: Option<&(u32, WeaponTags)>,
) -> Vec<(String, SpawnAt, Option<TagBolt>)> {
    let weapon = crate::play::cod4::weapon_name(weapons, f.weapon).and_then(|w| defs.get(w));
    let state_u32 = |name: &str| f.state.as_ref().map_or(0, |e| ent_u32(e, name));
    let normal = Vec3::from(byte_to_dir(f.parm as i32));
    let mut out = Vec::new();
    match f.name {
        "EV_FIRE_WEAPON" | "EV_FIRE_WEAPON_LASTSHOT" | "EV_FIRE_WEAPON_MG42" => {
            let yaw = f
                .state
                .as_ref()
                .map_or(0.0, |e| ent_f32(e, "lerp.apos.trBase[1]"))
                .to_radians();
            let dir = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
            // The view weapon's tags for the drawn view's own shot, else
            // the drawn body's (last frame's pose); a body not posed
            // flashes over its origin and ejects nothing.
            let own = view.filter(|(c, _)| f.entity.is_none_or(|n| n == *c));
            let t = match own {
                Some((_, t)) => Some(t),
                None => f.entity.and_then(|n| tags.get(&n)),
            };
            // The tags the effects ride while they play.
            let bolt = |s: i32| {
                let (entity, s) = match own {
                    Some((c, _)) => (*c, s + slot::VIEW_FLASH),
                    None => (f.entity?, s),
                };
                Some(TagBolt { entity, slot: s })
            };
            let (flash, shell, last_shell) = match own {
                Some(_) => (
                    "viewFlashEffect",
                    "viewShellEjectEffect",
                    "viewLastShotEjectEffect",
                ),
                None => (
                    "worldFlashEffect",
                    "worldShellEjectEffect",
                    "worldLastShotEjectEffect",
                ),
            };
            if let Some(n) = weapon.and_then(|w| w.effects.get(flash)) {
                match t.and_then(|t| t.flash) {
                    Some(tag) => out.push((n.clone(), on_tag(tag), bolt(slot::FLASH))),
                    None => out.push((
                        n.clone(),
                        directed(f.origin + Vec3::Z * FLASH_HEIGHT, dir),
                        None,
                    )),
                }
            }
            // `CG_EjectWeaponBrass`: the last shot's own eject when the
            // weapon has one.
            let last = f.name == "EV_FIRE_WEAPON_LASTSHOT"
                && weapon.is_some_and(|w| w.effects.contains_key(last_shell));
            let eject = if last { last_shell } else { shell };
            if let (Some(n), Some(tag)) = (
                weapon.and_then(|w| w.effects.get(eject)),
                t.and_then(|t| t.brass),
            ) {
                out.push((n.clone(), on_tag(tag), bolt(slot::BRASS)));
            }
        }
        "EV_BULLET_HIT" => {
            if let Some(n) = weapon.and_then(|w| {
                impact_effect(
                    impacts,
                    w.impact_type,
                    state_u32("surfType"),
                    state_u32("un1"),
                )
            }) {
                out.push((n, directed(f.origin, normal), None));
            }
        }
        "EV_GRENADE_BOUNCE" => {
            let surf = state_u32("surfType") as usize;
            if let Some(Some(n)) = impacts.nonflesh.get(8).and_then(|r| r.get(surf)) {
                out.push((n.clone(), directed(f.origin, normal), None));
            }
        }
        "EV_GRENADE_EXPLODE"
        | "EV_ROCKET_EXPLODE"
        | "EV_ROCKET_EXPLODE_NOMARKS"
        | "EV_CUSTOM_EXPLODE"
        | "EV_CUSTOM_EXPLODE_NOMARKS"
        | "EV_FLASHBANG_EXPLODE" => {
            if f.name != "EV_FLASHBANG_EXPLODE"
                && let Some(n) = weapon
                    .and_then(|w| impact_effect(impacts, w.impact_type, state_u32("surfType"), 0))
            {
                out.push((n, directed(f.origin, normal), None));
            }
            if let Some(n) = weapon.and_then(|w| w.effects.get("projExplosionEffect")) {
                out.push((n.clone(), directed(f.origin, normal), None));
            }
        }
        "EV_PLAY_FX" => {
            let name = configstring(CS_EFFECT_NAMES + f.parm as u16);
            if !name.is_empty() {
                let a = f.state.as_ref().map_or(Vec3::ZERO, |e| {
                    Vec3::new(
                        ent_f32(e, "lerp.apos.trBase[0]"),
                        ent_f32(e, "lerp.apos.trBase[1]"),
                        ent_f32(e, "lerp.apos.trBase[2]"),
                    )
                });
                // `CG_PlayFx`: the entity's angles give the effect's
                // forward.
                let (p, y) = (a.x.to_radians(), a.y.to_radians());
                let dir = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
                out.push((name, directed(f.origin, dir), None));
            }
        }
        _ => {}
    }
    out
}

/// A client-side bullet (`sv_clientSideBullets`, cod4-effects.md 12):
/// where a player's shot meets the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BulletHit {
    /// The trace's start: the shooter's `tag_flash`, or the drawn view's
    /// eye for its own shot.
    pub start: Vec3,
    /// Where a tracer and the whizby line start: the `tag_flash` (the
    /// view weapon's for the own shot).
    pub muzzle: Vec3,
    /// The world point the trace stopped at, or the range's end.
    pub end: Vec3,
    pub normal: Vec3,
    /// The struck surface type (`surfaceFlags` bits 20-24).
    pub surf: u32,
    /// The trace met a surface before its range.
    pub hit: bool,
    /// The drawn view's own shot.
    pub own: bool,
    /// The player whose box the first contact is: no client-side impact,
    /// sound, tracer or whizby for it; the server's `EV_BULLET_HIT` plays
    /// the flesh hit (cod4-effects.md 12.2).
    pub body: Option<u32>,
    /// The contacts past the first a penetrating round made, in order
    /// ([`Pierce`]).
    pub pierced: [Option<Pierce>; PIERCE_MAX],
}

/// A contact of a round past its first surface (the penetration walk,
/// `iw3mp.exe` 0x455a20..0x4561a6): the far side of what it passed (an
/// exit, `CG_BulletHitEvent`'s impact flag 4, along the round), or the
/// next surface it entered (along that surface's normal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pierce {
    pub at: Vec3,
    pub normal: Vec3,
    pub surf: u32,
    pub exit: bool,
}

/// Five penetrations, an exit and an entry each.
pub const PIERCE_MAX: usize = 10;

/// `perks` bit 5, `specialty_bulletpenetration` (Deep Impact), and
/// `perk_bulletPenetrationMultiplier`'s default (`iw3mp.exe` 0x40de65,
/// read at 0x455c93).
const PERK_BULLET_PENETRATION: u32 = 0x20;
const PERK_PENETRATION_MULTIPLIER: f32 = 2.0;
/// `bullet_penetrationMinFxDist`'s default (`iw3mp.exe` 0x40b993, read at
/// 0x455e8b): a further contact this close to the last plays nothing.
const PENETRATION_MIN_FX_DIST: f32 = 30.0;
/// `SURF_NOPENETRATE` (its surface's depth reads 0) and `SURF_SKY` (no
/// exit effect into it).
const SURF_NOPENETRATE: u32 = 0x100;
const SURF_SKY: u32 = 0x4;

/// What a client-side round meets (`CG_LocationalTrace` under 0x2806831):
/// the clip map and its static models, then the snapshot's solid
/// entities. A player is its packed `solid` box, never his hit boxes:
/// 1.7's `CG_LocationalTraceDObj` (iw3mp.exe 0x459250) hands a DObj only
/// to an entity with no `solid` of type 6, 12 or 14 (cod4-effects.md
/// 12.2).
#[derive(Clone, Copy)]
pub struct ShotWorld<'a> {
    pub clip: &'a vcod_cod4::collision::ClipMap,
    pub solids: &'a [crate::play::cod4_predict::Solid],
}

/// Where one leg of a round stopped and what it met.
#[derive(Clone, Copy, Debug, Default)]
struct Leg {
    hit: bool,
    pos: Vec3,
    normal: Vec3,
    flags: u32,
    startsolid: bool,
    allsolid: bool,
    /// The surface type its depth is read for ([`depth_surface`]).
    depth: usize,
    /// The entity it met, `None` the world.
    entity: Option<u32>,
    /// That entity is a player (the player test): no client-side impact.
    player: bool,
}

/// One leg of a round: the entity it passes and where it runs.
#[derive(Clone, Copy, Debug)]
struct Params {
    ignore: Option<u32>,
    start: Vec3,
    end: Vec3,
    dir: Vec3,
}

/// `BulletTrace` (iw3mp.exe 0x4558e0): a body's contact with no surface
/// flags reads flesh (0x700000, 0x4559c4).
fn leg(w: ShotWorld, bp: &Params, last: usize) -> Leg {
    use crate::play::cod4_predict::SolidKind;
    use vcod_cod4::collision::MASK_SHOT;
    let (start, end) = (bp.start, bp.end);
    let mut tr = w.clip.trace(start, end, Vec3::ZERO, Vec3::ZERO, MASK_SHOT);
    w.clip.trace_static_models(start, end, MASK_SHOT, &mut tr);
    let (mut fraction, mut normal, mut flags) = (tr.fraction, tr.normal, tr.surface_flags);
    let (mut startsolid, mut allsolid) = (tr.startsolid, tr.allsolid);
    let (mut entity, mut player) = (None, false);
    for s in w.solids.iter().filter(|s| Some(s.number) != bp.ignore) {
        match &s.kind {
            // `CG_LocationalTraceDObj`'s script mover: its model's
            // triangles in its frame, a hidden part's skipped
            // (cod4-combat.md 13.1).
            SolidKind::Model { surfs, hidden } => {
                let axis = vcod_common::x87::angles_to_axis(s.angles);
                let local = |p: Vec3| {
                    let d = p - s.origin;
                    Vec3::new(d.dot(axis[0]), d.dot(axis[1]), d.dot(axis[2]))
                };
                let (a, b) = (local(start), local(end));
                let mut c = vcod_common::clipmap::ClipTrace::clear();
                c.fraction = fraction;
                let mut met = false;
                for surf in surfs.iter() {
                    let bone = surf.bone.max(0) as usize;
                    if bone < 128 && hidden[bone / 32] & (0x8000_0000 >> (bone % 32)) != 0 {
                        continue;
                    }
                    met |= vcod_cod4::collision::trace_line(
                        std::slice::from_ref(surf),
                        a,
                        b,
                        MASK_SHOT,
                        &mut c,
                    );
                }
                if met && c.fraction < fraction {
                    let n = c.normal;
                    fraction = c.fraction;
                    normal = axis[0] * n.x + axis[1] * n.y + axis[2] * n.z;
                    flags = c.surface_flags;
                    (startsolid, allsolid) = (false, false);
                    (entity, player) = (Some(s.number), false);
                }
            }
            &SolidKind::Box {
                mins,
                maxs,
                contents,
            } => {
                if contents & MASK_SHOT == 0 {
                    continue;
                }
                let mut c = vcod_common::collision::Trace::clear(end);
                c.fraction = fraction;
                vcod_common::collision::clip_entity_box(
                    &mut c,
                    start,
                    end,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    s.origin + mins,
                    s.origin + maxs,
                    contents,
                    s.number,
                );
                if c.fraction < fraction {
                    let body = contents & vcod_cod4::pmove::CONTENTS_BODY != 0;
                    (fraction, normal) = (c.fraction, c.normal);
                    flags = if body { SURF_FLESH } else { 0 };
                    (startsolid, allsolid) = (c.startsolid, c.allsolid);
                    (entity, player) = (Some(s.number), body);
                }
            }
            &SolidKind::Brush(model) => {
                if model >= w.clip.submodels.len() {
                    continue;
                }
                let q = vcod_common::turretpose::angles_quat(s.angles.to_array());
                let local = |p: Vec3| q.inverse() * (p - s.origin);
                let c = w.clip.trace_model(
                    model,
                    local(start),
                    local(end),
                    Vec3::ZERO,
                    Vec3::ZERO,
                    MASK_SHOT,
                );
                if c.fraction < fraction {
                    (fraction, normal, flags) = (c.fraction, q * c.normal, c.surface_flags);
                    (startsolid, allsolid) = (c.startsolid, c.allsolid);
                    (entity, player) = (Some(s.number), false);
                }
            }
        }
    }
    let hit = fraction < 1.0;
    Leg {
        hit,
        pos: start.lerp(end, fraction),
        normal,
        flags,
        startsolid,
        allsolid,
        depth: depth_surface(hit, flags, last),
        entity: entity.filter(|_| hit),
        player: player && hit,
    }
}

/// The flesh surface a body's contact reads (`BulletTrace`).
const SURF_FLESH: u32 = 0x70_0000;

/// The surface type in `surfaceFlags` bits 20-24.
fn surface_type(flags: u32) -> u32 {
    (flags >> 20) & 0x1f
}

/// The surface a leg's penetration depth is read for (cod4-combat.md
/// 3.1): none past a miss or on a `SURF_NOPENETRATE` face, the last
/// leg's on an untyped face.
fn depth_surface(hit: bool, flags: u32, last: usize) -> usize {
    if !hit || flags & SURF_NOPENETRATE != 0 {
        return 0;
    }
    match surface_type(flags) as usize {
        0 => last,
        typed => typed,
    }
}

/// `BG_AdvanceTrace`'s step past a contact (cod4-effects.md 12.1):
/// where the next leg along `dir` starts and whether the round grazed
/// the face. Off an entity it starts at the contact (the entity is
/// passed); off the world `gap` on over the cosine to the face, a cosine
/// under 0.125 counting as 0.125 and a graze.
fn step_past(at: &Leg, face: Vec3, dir: Vec3, gap: f32) -> (Vec3, bool) {
    if at.entity.is_some() {
        return (at.pos, false);
    }
    let cos = -face.dot(dir);
    let grazed = cos < 0.125;
    let cos = if grazed { 0.125 } else { cos };
    (at.pos + dir * (gap / cos), grazed)
}

/// What a trace back toward the entry found of a surface's far side:
/// its thickness, the far leg, and whether the trace stayed in solid
/// (no far face to play).
struct FarSide {
    thick: f32,
    far: Leg,
    buried: bool,
}

/// The trace back from `next` (or the round's end when it met nothing)
/// to 0.01 short of `entry`, the way `round` runs; `None` when it meets
/// no far face and is not buried in solid.
fn far_side(w: ShotWorld, round: &Params, entry: &Leg, next: &Leg) -> Option<FarSide> {
    let back = -round.dir;
    let to = entry.pos - round.dir * 0.01;
    let (from, ignore) = if next.hit {
        (step_past(next, -next.normal, back, 0.01).0, next.entity)
    } else {
        (round.end, entry.entity)
    };
    let tr = Params {
        ignore,
        start: from,
        end: to,
        dir: back,
    };
    let far = leg(w, &tr, next.depth);
    let buried = far.hit && far.allsolid || next.startsolid && far.startsolid;
    if !far.hit && !buried {
        return None;
    }
    let span = if buried {
        to.distance(from)
    } else {
        entry.pos.distance(far.pos)
    };
    Some(FarSide {
        thick: span.max(1.0),
        far,
        buried,
    })
}

/// The contacts a walk plays, a player's left out.
#[derive(Default)]
struct Played {
    list: [Option<Pierce>; PIERCE_MAX],
    len: usize,
}

impl Played {
    fn add(&mut self, l: &Leg, at: Vec3, normal: Vec3, exit: bool) {
        if l.player || self.len == PIERCE_MAX {
            return;
        }
        let surf = surface_type(l.flags);
        self.list[self.len] = Some(Pierce {
            at,
            normal,
            surf,
            exit,
        });
        self.len += 1;
    }
}

/// The penetration walk past a round's first contact `first`
/// (iw3mp.exe 0x455a20..0x4561a6, cod4-effects.md 12.1): up to five
/// surfaces, each passed only while its depth (and its far side's, both
/// doubled by Deep Impact) outlasts the share of the round left, each
/// thickness over the lesser depth spent off that share. A contact
/// within 30 units of the last plays nothing; an exit into the sky
/// neither. INFERRED for the shape; the 1.7 reads cited on the
/// constants.
fn penetrate(
    w: ShotWorld,
    wav: &WeaponAv,
    perks: u32,
    first: Leg,
    mut round: Params,
) -> [Option<Pierce>; PIERCE_MAX] {
    let mut played = Played::default();
    if first.startsolid || !first.hit {
        return played.list;
    }
    let perk = match perks & PERK_BULLET_PENETRATION {
        0 => 1.0,
        _ => PERK_PENETRATION_MULTIPLIER,
    };
    let depth = |l: &Leg| wav.penetration.get(l.depth).map_or(0.0, |d| d * perk);
    let near = PENETRATION_MIN_FX_DIST * PENETRATION_MIN_FX_DIST;
    let mut share = 1.0f32;
    let mut entry = first;
    for _ in 0..5 {
        let mut room = depth(&entry);
        if room <= 0.0 {
            break;
        }
        let (start, grazed) = step_past(&entry, entry.normal, round.dir, 0.135);
        round.start = start;
        round.ignore = entry.entity;
        if grazed {
            break;
        }
        let next = leg(w, &round, entry.depth);
        match far_side(w, &round, &entry, &next) {
            Some(side) => {
                if side.far.hit {
                    room = room.min(depth(&side.far));
                    if room <= 0.0 {
                        break;
                    }
                }
                share -= side.thick / room;
                if share <= 0.0 {
                    break;
                }
                let spaced = side.far.pos.distance_squared(next.pos) > near;
                if !side.buried && spaced {
                    let into_sky = next.hit && next.flags & SURF_SKY != 0;
                    if !into_sky {
                        played.add(&side.far, side.far.pos, round.dir, true);
                    }
                    if next.hit {
                        played.add(&next, next.pos, next.normal, false);
                    }
                }
            }
            None => {
                let spaced = entry.pos.distance_squared(next.pos) > near;
                if next.hit && !next.allsolid && spaced {
                    played.add(&next, next.pos, next.normal, false);
                }
            }
        }
        if !next.hit {
            break;
        }
        entry = next;
    }
    played.list
}

/// The drawn view for [`client_bullets`]: its client, its eye and angles
/// (degrees, pitch down), its view weapon's `tag_flash`, and whether it
/// draws in third person (`cg.renderingThirdPerson`).
#[derive(Clone, Copy, Debug)]
pub struct OwnView {
    pub client: Option<u32>,
    pub eye: Vec3,
    pub angles: Vec3,
    pub flash: Option<Vec3>,
    pub third_person: bool,
}

/// How a shot's rounds leave the gun (`DrawBulletImpacts`,
/// cod4-effects.md 12.2): the seed the first round's spread disc
/// draws from (the next round's one more), the spread in degrees, the
/// rounds and their range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    pub seed: i32,
    pub spread: f32,
    pub shots: i32,
    pub range: f32,
    /// An own shot's eye and gun pitch and yaw, the server's for the cmd
    /// that fired (`play::cod4_predict::ShotAim`); `None` leaves the
    /// view's.
    pub from: Option<crate::play::cod4_predict::ShotAim>,
}

impl Aim {
    /// No spread: one round straight along the view.
    pub const STRAIGHT: Aim = Aim {
        seed: 0,
        spread: 0.0,
        shots: 1,
        range: BULLET_RANGE,
        from: None,
    };

    /// The drawn view's own shot (iw3mp.exe 0x456659..0x4566cc): the
    /// spread between `BG_GetSpreadForWeapon`'s ends by last frame's
    /// `aimSpreadScale` over 255, `adsSpread` in the sight
    /// (`fWeaponPosFrac` 1), seeded by the predicted playerstate's
    /// `commandTime` (0x456ca2): `seed`, the time of the cmd that fired.
    pub fn own(
        ps: &vcod_cod4::pmove::PlayerState,
        def: &vcod_cod4::pmove::weapon::WeaponDef,
        last_scale: f32,
        seed: i32,
    ) -> Aim {
        use vcod_cod4::bullet::{shots_and_range, spread_amount, spread_for_weapon};
        let spread = spread_amount(
            def,
            spread_for_weapon(ps, def),
            last_scale / 255.0,
            ps.weapon_pos_frac == 1.0,
        );
        let (shots, range) = shots_and_range(def);
        Aim {
            seed,
            spread,
            shots,
            range,
            from: None,
        }
    }

    /// Another player's shot (0x456854..0x456949): the hip ends of the
    /// stance his legs anim reads (`CG_GuessSpreadForWeapon`, 0x456250),
    /// scaled by his speed between the last two snapshots over 255,
    /// capped at 1, `adsSpread` at the near end when his legs anim is a
    /// walk (0x456200, 0x45681b); seeded by our own playerstate's
    /// `commandTime`.
    pub fn other(
        def: &vcod_cod4::pmove::weapon::WeaponDef,
        legs: vcod_cod4::pmove::anim::LegsPose,
        speed: f32,
        seed: i32,
    ) -> Aim {
        let ends = if legs.prone {
            (def.hip_spread_prone_min, def.hip_spread_prone_max)
        } else if legs.crouched {
            (def.hip_spread_ducked_min, def.hip_spread_ducked_max)
        } else {
            (def.hip_spread_stand_min, def.hip_spread_stand_max)
        };
        let scale = speed.min(255.0) / 255.0;
        let (shots, range) = vcod_cod4::bullet::shots_and_range(def);
        Aim {
            seed,
            spread: vcod_cod4::bullet::spread_amount(def, ends, scale, legs.sighted),
            shots,
            range,
            from: None,
        }
    }

    /// A turret's shot (`eType` 11, iw3mp.exe 0x456a12..0x456a4e): its
    /// weapon's `playerSpread` at both ends, so the scale reads nothing;
    /// seeded by our own `commandTime`, not the server's `level.time`.
    pub fn turret(def: &vcod_cod4::pmove::weapon::WeaponDef, seed: i32) -> Aim {
        let (shots, range) = vcod_cod4::bullet::shots_and_range(def);
        Aim {
            seed,
            spread: def.player_spread,
            shots,
            range,
            from: None,
        }
    }
}

/// `DrawBulletImpacts`' rounds for a fire event (iw3mp.exe 0x456620, the
/// rounds at 0x456c2b..0x456cee), each through the penetration walk: the drawn
/// view's own shot (its playerstate's ring, or its client's entity) from
/// the eye along the view, its muzzle the view weapon's `tag_flash`;
/// another player's from his posed `tag_flash` along his angles. Each
/// round's end is [`vcod_cod4::bullet::bullet_end`] off the event's
/// [`Aim`] (straight without one). Empty for a weapon firing no bullets
/// (`impactType` 1 to 4) or another body not posed (retail draws nothing
/// without the tag).
pub fn client_bullets(
    f: &Fired,
    weapons: &[String],
    defs: &HashMap<String, WeaponAv>,
    tags: &HashMap<u32, WeaponTags>,
    own: Option<&OwnView>,
    world: ShotWorld,
) -> Vec<BulletHit> {
    if !matches!(
        f.name,
        "EV_FIRE_WEAPON" | "EV_FIRE_WEAPON_LASTSHOT" | "EV_FIRE_WEAPON_MG42"
    ) {
        return Vec::new();
    }
    let Some(w) = crate::play::cod4::weapon_name(weapons, f.weapon).and_then(|w| defs.get(w))
    else {
        return Vec::new();
    };
    if !(1..=4).contains(&w.impact_type) {
        return Vec::new();
    }
    // A mounted gun's round leaves its `tag_flash` along the tag
    // (`DrawBulletImpacts`' `ET_MG42` arm), manned by us or not.
    let gun = f
        .state
        .as_ref()
        .is_some_and(|e| e.etype() == crate::play::cod4::ET_MG42);
    let mine = own.filter(|v| !gun && (f.entity.is_none() || f.entity == v.client));
    let shooter = match mine {
        Some(v) => v.client,
        None => f.entity,
    };
    let (start, axis, muzzle) = match mine {
        Some(v) => {
            let (start, axis) = match f.aim.and_then(|a| a.from) {
                Some(a) => (
                    a.eye,
                    vcod_common::x87::angle_vectors(Vec3::new(a.gun[0], a.gun[1], v.angles.z)),
                ),
                None => (v.eye, vcod_common::x87::angle_vectors(v.angles)),
            };
            (start, axis, own_muzzle(v, start, axis[0]))
        }
        None if gun => {
            let Some((pos, q)) = f.entity.and_then(|n| tags.get(&n)?.flash) else {
                return Vec::new();
            };
            (pos, [q * Vec3::X, q * Vec3::Y, q * Vec3::Z], pos)
        }
        None => {
            let Some(e) = f.state.as_ref() else {
                return Vec::new();
            };
            if e.etype() != crate::play::cod4::ET_PLAYER {
                return Vec::new();
            }
            let Some(flash) = f.entity.and_then(|n| tags.get(&n)?.flash) else {
                return Vec::new();
            };
            let angles = Vec3::new(
                ent_f32(e, "lerp.apos.trBase[0]"),
                ent_f32(e, "lerp.apos.trBase[1]"),
                0.0,
            );
            (flash.0, vcod_common::x87::angle_vectors(angles), flash.0)
        }
    };
    let aim = f.aim.unwrap_or(Aim::STRAIGHT);
    (0..aim.shots.max(1))
        .map(|shot| {
            let end =
                vcod_cod4::bullet::bullet_end(aim.seed + shot, aim.spread, start, &axis, aim.range);
            let bp = Params {
                ignore: shooter,
                start,
                end,
                dir: (end - start).normalize_or_zero(),
            };
            let first = leg(world, &bp, 0);
            BulletHit {
                start,
                muzzle,
                end: first.pos,
                normal: first.normal,
                surf: surface_type(first.flags),
                hit: first.hit,
                own: mine.is_some(),
                body: first.entity.filter(|_| first.player),
                pierced: penetrate(world, w, f.perks, first, bp),
            }
        })
        .collect()
}

/// Where the own shot's tracer starts (`DrawBulletImpacts`,
/// 0x456743..0x4567eb): the view weapon's `tag_flash`, or in third person
/// 30 (the double at 0x70b438) along the rounds' way from the eye
/// (cod4-effects.md 12.3).
fn own_muzzle(v: &OwnView, start: Vec3, forward: Vec3) -> Vec3 {
    if v.third_person {
        let k = |s: f32, d: f32| (f64::from(d) * 30.0 + f64::from(s)) as f32;
        return Vec3::new(
            k(start.x, forward.x),
            k(start.y, forward.y),
            k(start.z, forward.z),
        );
    }
    v.flash.unwrap_or(start)
}

/// `cg_firstPersonTracerChance` and `cg_tracerchance` (iw3mp.exe
/// 0x43c4f1, 0x43c99d): the share of the own and the others' rounds that
/// draw a tracer.
const FIRST_PERSON_TRACER_CHANCE: f32 = 0.5;
const TRACER_CHANCE: f32 = 0.2;

/// A single bullet's range (`DrawBulletImpacts`).
const BULLET_RANGE: f32 = 8192.0;

impl Cod4Effects {
    /// The tracers of one fire event's rounds ([`client_bullets`]), from
    /// the muzzle to the hit, each back-dated half a draw of the frame's ms
    /// (`CG_SpawnTracer`, iw3mp.exe 0x458110). The own shot rolls
    /// `cg_firstPersonTracerChance` once for all its rounds, a shotgun's
    /// pellets too (`DrawBulletImpacts`, 0x4567f2..0x456816), and each
    /// round draws one when it passed, contact or not. Another's round
    /// rolls `cg_tracerchance` on each contact it plays, the penetration's
    /// too, and draws to it (`CG_BulletHitEvent`'s
    /// The trajectory effects, 0x458bcd; the tracer test, 0x458720):
    /// none for a miss, none for a round into a player (the server's
    /// `EV_BULLET_HIT` plays it). The gun we man draws none: the caller
    /// skips it (0x458749..0x458758). The number spawned.
    pub(crate) fn tracers(
        &mut self,
        fx: &mut FxSystem,
        hits: &[BulletHit],
        now: f32,
        frame_ms: i32,
    ) -> usize {
        let own = hits.first().is_some_and(|h| h.own);
        if own && self.roll() >= FIRST_PERSON_TRACER_CHANCE {
            return 0;
        }
        let mut spawned = 0;
        for hit in hits {
            let ends: Vec<Vec3> = if own {
                vec![hit.end]
            } else {
                let first = (hit.hit && hit.body.is_none()).then_some(hit.end);
                first
                    .into_iter()
                    .chain(hit.pierced.iter().flatten().map(|p| p.at))
                    .collect()
            };
            for end in ends {
                if !own && self.roll() >= TRACER_CHANCE {
                    continue;
                }
                let back = if frame_ms > 0 {
                    (self.next_rand() % frame_ms as u32) as i32 / 2
                } else {
                    0
                };
                fx.spawn_cod4_tracer(hit.muzzle, end, now, back);
                spawned += 1;
            }
        }
        self.tracers += spawned;
        spawned
    }

    fn next_rand(&mut self) -> u32 {
        self.rand = self.rand.wrapping_mul(214013).wrapping_add(2531011);
        (self.rand >> 16) & 0x7fff
    }

    /// `rand() / 32768`.
    fn roll(&mut self) -> f32 {
        self.next_rand() as f32 / 32768.0
    }

    /// `CG_BulletHitEvent`'s effect for a client-side bullet that met the
    /// world: the impact table's row by the weapon's impact type and the
    /// surface's column, along the surface normal.
    pub(crate) fn bullet_plays(
        &mut self,
        f: &Fired,
        weapons: &[String],
        hit: &BulletHit,
    ) -> Vec<Play> {
        let w = crate::play::cod4::weapon_name(weapons, f.weapon).and_then(|w| self.weapons.get(w));
        let Some(w) = w.filter(|_| hit.hit).cloned() else {
            return Vec::new();
        };
        // The first contact, then the penetration's exits (impact flag 4)
        // and entries.
        let first = hit
            .body
            .is_none()
            .then_some((hit.end, hit.normal, hit.surf, 0));
        let contacts = first.into_iter().chain(
            hit.pierced
                .iter()
                .flatten()
                .map(|p| (p.at, p.normal, p.surf, if p.exit { 4 } else { 0 })),
        );
        let mut out = Vec::new();
        for (at, normal, surf, flags) in contacts {
            let Some(name) = impact_effect(&self.impacts, w.impact_type, surf, flags) else {
                continue;
            };
            match self.resolve(&name) {
                Some(key) => {
                    self.spawned += 1;
                    out.push(Play {
                        key,
                        at: directed(at, normal),
                        bolt: None,
                    });
                }
                None => {
                    self.missing.insert(name);
                }
            }
        }
        out
    }
}

/// `EV_EARTHQUAKE`'s shake (`CG_StartShakeCamera`): the event entity's
/// `lerp.u.earthquake` words, float scale, float radius, int duration ms
/// (cod4-effects.md 7).
pub fn earthquake(f: &Fired) -> Option<crate::play::quake::Quake> {
    if f.name != "EV_EARTHQUAKE" {
        return None;
    }
    let e = f.state.as_ref()?;
    Some(crate::play::quake::Quake {
        start_ms: 0,
        scale: ent_f32(e, "lerp.u.anonymous.data[0]"),
        radius: ent_f32(e, "lerp.u.anonymous.data[1]"),
        duration_ms: ent_u32(e, "lerp.u.anonymous.data[2]") as i32 as f32,
        origin: f.origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The penetration walk's contacts for a fan of rounds on mp_crash,
    /// through its walls and two boxes (a player's and a solid's), with
    /// and without Deep Impact. Pinned bit for bit off the build that
    /// first walked them (cod4-effects.md 12.1).
    #[test]
    fn the_penetration_walk_holds_on_mp_crash() {
        use crate::play::cod4_predict::{Solid, SolidKind};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let world = crate::cod4_fly::load_world(install.root(), "mp_crash").unwrap();
        let solids = [
            Solid {
                number: 3,
                origin: Vec3::new(207.0, 2380.0, 296.0),
                angles: Vec3::ZERO,
                kind: SolidKind::Box {
                    mins: Vec3::new(-15.0, -15.0, 0.0),
                    maxs: Vec3::new(15.0, 15.0, 70.0),
                    contents: vcod_cod4::pmove::CONTENTS_BODY,
                },
            },
            Solid {
                number: 40,
                origin: Vec3::new(260.0, 2200.0, 300.0),
                angles: Vec3::ZERO,
                kind: SolidKind::Box {
                    mins: Vec3::splat(-20.0),
                    maxs: Vec3::splat(20.0),
                    contents: 1,
                },
            },
        ];
        let w = ShotWorld {
            clip: &world.clip,
            solids: &solids,
        };
        let mut wav = WeaponAv::default();
        for (s, d) in wav.penetration.iter_mut().enumerate().skip(1) {
            *d = ((s * 37) % 60) as f32;
        }
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |x: u32| {
            for b in x.to_le_bytes() {
                hash = (hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
            }
        };
        let (mut dump, mut contacts) = (String::new(), 0);
        let eyes = [
            Vec3::new(207.0, 2250.0, 356.0),
            Vec3::new(207.0, 2380.0, 356.0),
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(-300.0, 1200.0, 200.0),
        ];
        for eye in eyes {
            for yaw in (0..360).step_by(10) {
                for pitch in [-20.0f32, -5.0, 0.0, 7.5, 30.0] {
                    for perks in [0, PERK_BULLET_PENETRATION] {
                        let axis =
                            vcod_common::x87::angle_vectors(Vec3::new(pitch, yaw as f32, 0.0));
                        let end = eye + axis[0] * 8192.0;
                        let bp = Params {
                            ignore: Some(0),
                            start: eye,
                            end,
                            dir: (end - eye).normalize_or_zero(),
                        };
                        let first = leg(w, &bp, 0);
                        let walk = penetrate(w, &wav, perks, first, bp);
                        for p in walk.iter().flatten() {
                            contacts += 1;
                            dump += &format!("{eye} {yaw} {pitch} {perks} {p:?}\n");
                            p.at.to_array()
                                .into_iter()
                                .chain(p.normal.to_array())
                                .for_each(|x| eat(x.to_bits()));
                            eat(p.surf);
                            eat(u32::from(p.exit));
                        }
                    }
                }
            }
        }
        if let Some(path) = std::env::var_os("PIERCE_PIN_DUMP") {
            std::fs::write(path, dump).unwrap();
        }
        assert_eq!(
            (contacts, hash),
            (301, 0x5414_2c66_4388_8382),
            "{contacts} {hash:#x}"
        );
    }
    use vcod_cod4::walk::{WalkOptions, walk};
    use vcod_cod4::zone::Zone;

    /// The tracer rolls (cod4-effects.md 12.2): the own shot rolls once,
    /// all its pellets drawing one or none; another's round rolls on each
    /// contact, never on a miss or a body.
    #[test]
    fn tracers_roll_once_a_shot_and_on_each_contact_of_anothers() {
        let mut e = Cod4Effects::new(&[], Arc::new(HashMap::new()), "");
        let mut fx = FxSystem::new();
        let round = BulletHit {
            start: Vec3::ZERO,
            muzzle: Vec3::ZERO,
            end: Vec3::X * 100.0,
            normal: -Vec3::X,
            surf: 1,
            hit: true,
            own: true,
            body: None,
            pierced: [None; PIERCE_MAX],
        };
        let pellets = [round; 8];
        let drawn: Vec<usize> = (0..20)
            .map(|_| e.tracers(&mut fx, &pellets, 0.0, 16))
            .collect();
        assert!(drawn.iter().all(|&n| n == 0 || n == 8), "{drawn:?}");
        assert!(drawn.contains(&0) && drawn.contains(&8), "{drawn:?}");
        let theirs = BulletHit {
            own: false,
            ..round
        };
        let miss = BulletHit {
            hit: false,
            ..theirs
        };
        let body = BulletHit {
            body: Some(3),
            ..theirs
        };
        for _ in 0..50 {
            assert_eq!(e.tracers(&mut fx, &[miss, body], 0.0, 16), 0);
        }
        let mut pierced = theirs;
        pierced.pierced[0] = Some(Pierce {
            at: Vec3::X * 108.0,
            normal: Vec3::X,
            surf: 1,
            exit: true,
        });
        let most = (0..200)
            .map(|_| e.tracers(&mut fx, &[pierced], 0.0, 16))
            .max();
        assert_eq!(most, Some(2), "one roll a contact");
    }

    /// `cg_blood` 0 swaps a flesh hit for `impacts/flesh_hit_noblood`
    /// (iw3mp.exe 0x458ad9); a wall's hit stays the table's; the zone
    /// holds the effect. The zone half is vacuous without `COD4_DIR`.
    #[test]
    fn cg_blood_off_plays_the_no_blood_flesh_hit() {
        let mut t = ImpactTable {
            nonflesh: vec![vec![Some("impacts/small_concrete".into()); 8]],
            flesh: vec![vec![Some("impacts/flesh_hit".into()); 4]],
            no_blood: false,
        };
        assert_eq!(
            impact_effect(&t, 1, 7, 0).as_deref(),
            Some("impacts/flesh_hit")
        );
        t.no_blood = true;
        assert_eq!(impact_effect(&t, 1, 7, 0).as_deref(), Some(FLESH_NOBLOOD));
        assert_eq!(
            impact_effect(&t, 1, 3, 0).as_deref(),
            Some("impacts/small_concrete")
        );
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let z = Zone::open_path(&install.zone("english", "common_mp")).unwrap();
        let a = walk(z, WalkOptions { keep_bulk: false }).unwrap().assets;
        assert!(a.fx.iter().any(|f| f.name == FLESH_NOBLOOD));
    }

    /// `common_mp`'s effects as the fx system holds them, and the walked
    /// zone.
    fn common_fx() -> Option<(FxSystem, Assets)> {
        let install = vcod_cod4::testing::cod4_install()?;
        let z = Zone::open_path(&install.zone("english", "common_mp")).unwrap();
        let a = walk(z, WalkOptions { keep_bulk: true }).unwrap().assets;
        let mut e = Cod4Effects::new(&[&a], Arc::new(HashMap::new()), "");
        let mut fx = FxSystem::new();
        for (k, v) in e.take_effects().unwrap() {
            fx.insert(&k, v);
        }
        Some((fx, a))
    }

    /// A large round's brick hit (`impacts/large_brick`): its flying
    /// chunks (a one-shot model, `effectEmitted`
    /// `impacts/brickimpact_em2`, `emitDist` 4, variance 1 + 0..11) play
    /// the dust effect along their flight, which the zone holds as an
    /// effect of its own; the emission copies are not spawned by the hit
    /// (cod4-effects.md 15). Vacuous without `COD4_DIR`.
    #[test]
    fn brick_hit_chunks_emit_their_dust_along_their_flight() {
        let Some((mut fx, a)) = common_fx() else {
            return;
        };
        let hit =
            a.fx.iter()
                .find(|f| f.name == "impacts/large_brick")
                .unwrap();
        let p = parts(&a, hit);
        assert!(p.gaps.is_empty(), "{:?}", p.gaps);
        let em = p
            .full
            .emitters
            .iter()
            .find(|e| e.cod4_emit.is_some())
            .expect("an emitting element");
        let c = em.cod4_emit.unwrap();
        assert_eq!(
            (c.dist.a, c.dist.b, c.variance_base, c.variance_amp),
            (4.0, 4.0, 1.0, 11.0)
        );
        assert_eq!(em.emit_fx, vec![key("impacts/brickimpact_em2")]);
        // One emitter an element: the copies stay out.
        assert_eq!(
            p.full.emitters.len(),
            hit.looping as usize + hit.one_shot as usize
        );
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let at = SpawnAt::Directed {
            pos: Vec3::ZERO,
            dir: Vec3::X,
        };
        fx.spawn(&fs, &key("impacts/large_brick"), at, 0.0);
        let mut emits = 0;
        for i in 1..=60 {
            let now = i as f32 / 60.0;
            fx.step(1.0 / 60.0, now, None);
            emits += fx
                .pending_chains()
                .iter()
                .filter(|p| **p == key("impacts/brickimpact_em2"))
                .count();
            fx.spawn_chained(&fs, now);
        }
        println!("{emits} dust plays");
        assert!(emits > 0, "the chunks emitted nothing");
    }

    /// The RPG's `projTrailEffect` (`smoke/smoke_geotrail_rpg`, a trail of
    /// `splitDist` 100 and `repeatDist` 128 over two crossed cuts and two
    /// diagonal ones) on a tag flying 1003 units a second: a cross-section
    /// every 100 units, eight quads between each pair, the texture's `u`
    /// 100 / 128 a section on; once the tag stops posing the trail lives
    /// its sections out (8 s) and goes (cod4-effects.md 16). Vacuous
    /// without `COD4_DIR`.
    #[test]
    fn rpg_trail_drops_a_section_every_split_distance_on_its_tag() {
        let Some((mut fx, a)) = common_fx() else {
            return;
        };
        let rpg =
            a.fx.iter()
                .find(|f| f.name == "smoke/smoke_geotrail_rpg")
                .unwrap();
        assert!(parts(&a, rpg).gaps.is_empty());
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let bolt = TagBolt {
            entity: 300,
            slot: 0,
        };
        let pose = |x: f32| TagPose {
            origin: Vec3::new(x, 0.0, 100.0),
            axis: [Vec3::X, Vec3::Y, Vec3::Z],
        };
        fx.set_tag_bolts(&HashMap::from([(bolt, pose(0.0))]));
        fx.spawn_on_tag(&fs, &key("smoke/smoke_geotrail_rpg"), bolt, pose(0.0), 0.0);
        let trail = |fx: &FxSystem, now: f32| -> Vec<crate::fx::sim::FxQuad> {
            fx.build_quads(Vec3::new(500.0, -500.0, 100.0), Vec3::X, Vec3::Z, now)
                .into_iter()
                .filter(|q| q.shader.contains("smoke_trail02"))
                .collect()
        };
        for i in 1..=60 {
            let now = i as f32 / 60.0;
            fx.set_tag_bolts(&HashMap::from([(bolt, pose(1003.0 * now))]));
            fx.step(1.0 / 60.0, now, None);
        }
        let q = trail(&fx, 1.0);
        // Two cuts at the start, then one at 100, .. 1000: eleven segments.
        assert_eq!(q.len(), 8 * 11, "{}", q.len());
        // The quads come sorted back to front: the cuts' `u`s, in order.
        let mut us: Vec<f32> = q.iter().flat_map(|q| q.uvs.map(|uv| uv[0])).collect();
        us.sort_by(f32::total_cmp);
        us.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        assert_eq!(us.len(), 11, "{us:?}");
        for w in us.windows(2) {
            assert!((w[1] - w[0] - 100.0 / 128.0).abs() < 1e-3, "{us:?}");
        }
        // Every point within the trail's size of the line it flew.
        let reach = q
            .iter()
            .flat_map(|q| q.verts)
            .map(|v| Vec3::from(v).truncate().y.abs().max((v[2] - 100.0).abs()))
            .fold(0.0f32, f32::max);
        println!("cuts reach {reach} off the line");
        assert!(reach > 0.0);
        // `gfx_smoke_trail02_godray` is an `effect_falloff` set: seen from
        // the side at the trail's height, the flat cut is edge-on and
        // gone, the upright one face-on and whole.
        let alphas: Vec<f32> = q.iter().map(|q| q.rgba[3]).collect();
        assert!(alphas.contains(&0.0), "{alphas:?}");
        assert!(alphas.iter().any(|&a| a > 0.1), "{alphas:?}");
        fx.set_tag_bolts(&HashMap::new());
        fx.step(1.0 / 60.0, 1.02, None);
        assert!(!trail(&fx, 1.02).is_empty(), "the sections live on");
        fx.step(1.0 / 60.0, 9.5, None);
        assert!(trail(&fx, 9.5).is_empty(), "the sections never went");
    }

    /// The frag's concrete explode out of `common_mp` (cod4-effects.md 4,
    /// 6, 17): its heat haze draws as a distortion element, and its
    /// colour keys are the zone's `D3DCOLOR` bytes, the amplitude's colour
    /// the random end. Vacuous without `COD4_DIR`.
    #[test]
    fn the_grenade_explode_draws_the_zones_colours_and_its_haze() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let z = Zone::open_path(&install.zone("english", "common_mp")).unwrap();
        let a = walk(z, WalkOptions { keep_bulk: true }).unwrap().assets;
        let fx =
            a.fx.iter()
                .find(|f| f.name == "explosions/grenadeexp_concrete_1")
                .unwrap();
        let (effect, gaps) = convert(&a, fx);
        assert!(!gaps.contains(&"distortion"), "{gaps:?}");
        assert!(
            effect.emitters.iter().any(|em| em
                .cod2
                .as_ref()
                .is_some_and(|el| el.atlas.keys().any(|m| m == "gfx_distortion_heat_far"))),
            "no haze element"
        );
        // Every drawn colour key against the zone's bytes, B G R A.
        let mut checked = 0;
        for (i, (role, e)) in roles(fx).enumerate() {
            let d = ElemDef::of(e);
            let Converted::Primitive(p, _) = primitive(&a, fx, role, &d, i, Looping::Spread) else {
                continue;
            };
            if d.vis.is_empty() {
                continue;
            }
            let rgb = &p.channels[Channel::Rgb as usize].curve;
            let rand = &p.channels[Channel::RgbRand as usize].curve;
            for (k, s) in d.vis.iter().enumerate() {
                let want = |c: [u8; 4]| [c[2], c[1], c[0]].map(|b| f32::from(b) / 255.0);
                assert_eq!(
                    rgb.keys[4 * k + 1..4 * k + 4],
                    want(s.base.color),
                    "{i} {k}"
                );
                assert_eq!(
                    rand.keys[4 * k + 1..4 * k + 4],
                    want(s.amplitude.color),
                    "{i} {k}"
                );
                checked += 1;
            }
        }
        assert!(checked > 20, "{checked}");
        // The m16's flash light is warm: bytes (158, 182, 255) are B G R.
        let flash =
            a.fx.iter()
                .find(|f| f.name == "muzzleflashes/m16_flash_wv_a")
                .unwrap();
        let (role, e) = roles(flash).next().unwrap();
        let d = ElemDef::of(e);
        assert_eq!(d.elem_type, Some(ElemType::OmniLight));
        let Converted::Primitive(p, _) = primitive(&a, flash, role, &d, 0, Looping::Spread) else {
            panic!("light not converted");
        };
        let k = &p.channels[Channel::Rgb as usize].curve.keys;
        assert!(k[1] > k[3], "{:?}", &k[..4]);
    }

    /// mp_cargoship's rain (`weather/rain_mp_cargoship` element 0, a
    /// cloud of `cloud_snowflake01`): the cube is the visual scale's 1000,
    /// each flake 1.5 wide and 150 long, streaked along its fall. It drew
    /// as a 1.5-unit cube of 150-unit square flakes, giant white discs.
    #[test]
    fn cargoship_rain_clouds_are_thin_streaks_in_a_wide_cube() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let z = Zone::open_path(&install.zone("english", "mp_cargoship")).unwrap();
        let a = walk(z, WalkOptions { keep_bulk: true }).unwrap().assets;
        let fx =
            a.fx.iter()
                .find(|f| f.name == "weather/rain_mp_cargoship")
                .unwrap();
        let (role, e) = roles(fx).next().unwrap();
        let d = ElemDef::of(e);
        assert_eq!(d.elem_type, Some(ElemType::Cloud));
        let Converted::Primitive(p, _) = primitive(&a, fx, role, &d, 0, Looping::Spread) else {
            panic!("cloud not converted");
        };
        let key = |c: Channel| p.channels[c as usize].curve.keys[1];
        assert_eq!(key(Channel::Size), 1000.0);
        assert_eq!(key(Channel::Size2), 1.5);
        assert_eq!(key(Channel::Length), 150.0);
        assert!(p.non_uniform_scale && p.use_length);
        let c = crate::fx::cod2_cloud::FxCloud::new(
            Vec3::ZERO,
            Vec3::new(0.0, -0.4, -1.5),
            (
                key(Channel::Size),
                key(Channel::Size2),
                key(Channel::Length),
            ),
            0.0,
            Vec3::Z,
            [1.0; 4],
            "cloud_snowflake01",
            (Vec3::X, Vec3::Z),
        );
        assert!((c.axis[0].length() - 1000.0).abs() < 0.01, "{:?}", c.axis);
        let [m0, m1, m2, m3] = c.matrix;
        let (w, l) = (m0.hypot(m1), m2.hypot(m3));
        assert!(
            (w - 1.5).abs() < 0.01 && (l - 150.0).abs() < 0.5,
            "{:?}",
            c.matrix
        );
    }

    /// An entity state of `etype` numbered `num`, the named fields set.
    fn ent(etype: u32, num: u32, fields: &[(&str, u32)]) -> vcod_cod4::net::delta::EntityState {
        use vcod_cod4::net::delta::EntityState;
        let mut e = EntityState::default();
        e.0[0] = num;
        e.0[1] = etype;
        for (name, v) in fields {
            let f = EntityState::table(etype)
                .iter()
                .find(|f| f.name == *name)
                .unwrap_or_else(|| panic!("{name}"));
            e.0[f.word()] = *v;
        }
        e
    }

    /// No zones, one effect `t` with the given looping schedules.
    fn looping(specs: &[LoopSpec]) -> (Cod4Effects, FxSystem) {
        let mut e = Cod4Effects::new(&[], Arc::new(HashMap::new()), "");
        let k = key("t");
        e.names.insert(k.clone());
        e.loops.insert(k, specs.to_vec());
        (e, FxSystem::new())
    }

    fn play_t() -> Play {
        Play {
            key: key("t"),
            at: SpawnAt::Point { pos: Vec3::ZERO },
            bolt: None,
        }
    }

    /// `FX_SpawnLoopingElems`: one element an interval from the start,
    /// `count` in all; a start long past spawns only the intervals whose
    /// elements still show (cod4-effects.md 5).
    #[test]
    fn a_looping_element_spawns_once_an_interval() {
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let forever = LoopSpec {
            interval_ms: 100,
            count: i32::MAX,
            reach_ms: 500,
        };
        let three = LoopSpec {
            interval_ms: 50,
            count: 3,
            reach_ms: 100,
        };
        let (mut e, mut fx) = looping(&[forever, three]);
        e.start(&mut fx, &fs, &play_t(), 10.0, None);
        assert_eq!(e.loopers(), 2);
        e.run_loops(&mut fx, &fs, 10.0);
        assert_eq!(e.loop_spawns, 2);
        e.run_loops(&mut fx, &fs, 10.25);
        // 10.1 and 10.2, then 10.05 and 10.1, the last of three.
        assert_eq!(e.loop_spawns, 6);
        assert_eq!(e.loopers(), 1);
        e.run_loops(&mut fx, &fs, 10.95);
        assert_eq!(e.loop_spawns, 6 + 7);
        // Settled 15 s ago: the spawns of the last 628 ms only.
        let (mut e, mut fx) = looping(&[forever]);
        e.start(&mut fx, &fs, &play_t(), 85.0, None);
        e.run_loops(&mut fx, &fs, 100.0);
        assert_eq!(e.loop_spawns, 8);
    }

    /// `CG_LoopFx` and `CG_Fx` (cod4-effects.md 11): a looped effect
    /// starts on first sight inside its `cullDist` and restarts every
    /// `period`; a `spawnFx` one starts at `time2` and again when it
    /// changes; an entity gone from the snapshot ends its loops.
    #[test]
    fn effect_entities_start_and_retrigger() {
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let spec = LoopSpec {
            interval_ms: 125,
            count: i32::MAX,
            reach_ms: 10_000,
        };
        let cs = |n: u16| {
            if n == CS_EFFECT_NAMES + 3 {
                "t".to_string()
            } else {
                String::new()
            }
        };
        let (mut e, mut fx) = looping(&[spec]);
        let near = ent(
            ET_LOOP_FX,
            40,
            &[
                ("un1", 3),
                ("lerp.u.loopFx.cullDist", 500f32.to_bits()),
                ("lerp.u.loopFx.period", 2000),
                ("lerp.pos.trBase[0]", 100f32.to_bits()),
            ],
        );
        let far = ent(
            ET_LOOP_FX,
            41,
            &[
                ("un1", 3),
                ("lerp.u.loopFx.cullDist", 500f32.to_bits()),
                ("lerp.u.loopFx.period", 2000),
                ("lerp.pos.trBase[0]", 900f32.to_bits()),
            ],
        );
        let spot = ent(ET_FX, 42, &[("un1", 3), ("time2", 1500)]);
        let mut frame = |e: &mut Cod4Effects, ents: &[_], ms: i32| {
            let now = f64::from(ms) / 1000.0;
            e.entity_fx(&mut fx, &fs, ents, &cs, (ms, now), Vec3::ZERO);
            e.run_loops(&mut fx, &fs, now);
        };
        frame(&mut e, &[near, far, spot], 1000);
        // The near loop and the spot effect, the far one culled.
        assert_eq!(e.loopers(), 2);
        assert_eq!(e.loop_spawns, 1);
        frame(&mut e, &[near, far, spot], 2900);
        // 1.125 s to 2.875 s, and the spot one's 1.5 s to 2.875 s.
        assert_eq!(e.loop_spawns, 1 + 15 + 12);
        // The near one restarts at 3 s: its spawn 0 again.
        frame(&mut e, &[near, far, spot], 3000);
        assert_eq!(e.loop_spawns, 28 + 1 + 1);
        assert_eq!(e.loopers(), 2);
        let moved = ent(ET_FX, 42, &[("un1", 3), ("time2", 3050)]);
        frame(&mut e, &[near, moved], 3100);
        // The spot one restarted at 3.05 s.
        assert_eq!(e.loop_spawns, 31);
        assert_eq!(e.loopers(), 2);
        frame(&mut e, &[], 3200);
        assert_eq!(e.loopers(), 0);
        assert_eq!(e.loop_spawns, 31);
    }

    /// mp_crash's createfx ambience through the fx system
    /// (cod4-effects.md 11): its 36 effects start settled, every looping
    /// element keeps spawning for as long as the map runs, and the
    /// particles stay around the placed origins. Vacuous without
    /// `COD4_DIR`.
    #[test]
    fn mp_crash_ambience_runs_for_ever() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let ((_, mut e), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let mut fx = FxSystem::new();
        for (k, v) in e.take_effects().unwrap() {
            fx.insert(&k, v);
        }
        let placed: Vec<Vec3> = e
            .ambience
            .as_ref()
            .unwrap()
            .iter()
            .map(|(p, _)| match p.at {
                SpawnAt::Oriented { pos, .. } => pos,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(placed.len(), 36);
        let render_ms = 60_000;
        let mut now = 100.0f64;
        e.start_ambience(&mut fx, &fs, (render_ms, now));
        assert!(e.missing.is_empty(), "{:?}", e.missing);
        let loopers = e.loopers();
        let mut samples = Vec::new();
        for i in 0..600 {
            e.run_loops(&mut fx, &fs, now);
            fx.step(1.0 / 60.0, now as f32, None);
            if i % 120 == 0 {
                let (parts, _, lights) = fx.counts();
                samples.push((e.loop_spawns, parts, lights));
            }
            now += 1.0 / 60.0;
        }
        println!("{loopers} loopers, (spawns, particles, lights) {samples:?}");
        // Every looping element loops for ever: none ends.
        assert_eq!(e.loopers(), loopers);
        assert!(loopers > 50, "{loopers}");
        // Already settled on the first frame, and as many a second after.
        let parts: Vec<usize> = samples.iter().map(|s| s.1).collect();
        assert!(parts[0] > 300, "{parts:?}");
        assert!(
            parts
                .iter()
                .all(|&p| p * 2 > parts[0] && p < crate::fx::sim::MAX_PARTICLES),
            "{parts:?}"
        );
        let rate: Vec<usize> = samples.windows(2).map(|w| w[1].0 - w[0].0).collect();
        assert!(
            rate.iter()
                .all(|&r| r == rate[0] || r.abs_diff(rate[0]) * 50 < rate[0]),
            "{rate:?}"
        );
        // The drawn sprites stay near the placed effects.
        let quads = fx.build_quads(Vec3::ZERO, Vec3::Y, Vec3::Z, now as f32);
        let near = quads
            .iter()
            .filter(|q| {
                let c = q.verts.iter().map(|v| Vec3::from(*v)).sum::<Vec3>() / 4.0;
                placed.iter().any(|o| (c - *o).length() < 400.0)
            })
            .count();
        println!("{near} of {} sprites within 400 units", quads.len());
        assert!(near * 10 > quads.len() * 9, "{near} of {}", quads.len());
    }

    /// mp_crash's ambience seen from the playtest's spot by the red sedan
    /// (`setviewpos -279 1396 233 -15 28`, cod4-effects.md 18), run for
    /// 10 s with the drawn view set each frame: `(eye, quads)` with the
    /// view turned `turn` degrees of yaw. Vacuous without `COD4_DIR`.
    fn crash_sedan_view(turn: f32) -> Option<(Vec3, Vec<crate::fx::sim::FxQuad>, Vec<Vec3>)> {
        let install = vcod_cod4::testing::cod4_install()?;
        let ((_, mut e), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let mut fx = FxSystem::new();
        for (k, v) in e.take_effects().unwrap() {
            fx.insert(&k, v);
        }
        // firelp_small_pm's three placements (maps/createfx/mp_crash_fx.gsc)
        let fires: Vec<Vec3> = e
            .ambience
            .as_ref()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.key.contains("firelp_small_pm."))
            .map(|(p, _)| match p.at {
                SpawnAt::Oriented { pos, .. } => pos,
                _ => unreachable!(),
            })
            .collect();
        let eye = Vec3::new(-279.0, 1396.0, 233.0 + 60.0);
        let (yaw, pitch) = ((-15.0f32 + turn).to_radians(), (-28.0f32).to_radians());
        let (fwd, right, up) = crate::camera::basis(yaw, pitch);
        let aspect = 16.0 / 9.0;
        let tan_x = (crate::camera::view_fov_x(65.0, aspect, true).to_radians() / 2.0).tan();
        let view = |fx: &mut FxSystem| {
            fx.set_view(eye);
            fx.set_frustum(crate::fx::sim::Frustum::new(
                eye,
                (fwd, -right, up),
                tan_x,
                tan_x / aspect,
            ));
        };
        let mut now = 100.0f64;
        view(&mut fx);
        e.start_ambience(&mut fx, &fs, (60_000, now));
        for _ in 0..600 {
            view(&mut fx);
            e.run_loops(&mut fx, &fs, now);
            fx.step(1.0 / 60.0, now as f32, None);
            now += 1.0 / 60.0;
        }
        Some((eye, fx.build_quads(eye, right, up, now as f32), fires))
    }

    /// The fires' smoke (`fire/firelp_small_pm` element 2,
    /// `gfx_smk_white_atlas`, `fadeInRange` 1000 + 200) by the playtest's
    /// sedan: 1.7's `FX_EvaluateDistanceFade` (iw3mp.exe 0x498010) scales
    /// its alpha to under a tenth from 1182 units, and it spawns only
    /// from a camera within its 1250 range, so of the three fires only
    /// the nearest smokes. vcod drew it at full alpha, a smoke column
    /// over the town that retail's frame lacks. Turned away, the element's
    /// frustum cull (flag 0x4, radius 80) spawns none of it. Vacuous
    /// without `COD4_DIR`.
    #[test]
    fn crash_fire_smoke_fades_with_distance_and_spawns_in_view() {
        let smoke = |eye: Vec3, quads: &[crate::fx::sim::FxQuad], fires: &[Vec3]| {
            quads
                .iter()
                .filter(|q| q.shader.ends_with("gfx_smk_white_atlas"))
                .filter_map(|q| {
                    let c = q.verts.iter().map(|v| Vec3::from(*v)).sum::<Vec3>() / 4.0;
                    let near = fires.iter().any(|f| (c - *f).truncate().length() < 250.0);
                    near.then(|| ((c - eye).length(), q.rgba[3]))
                })
                .collect::<Vec<_>>()
        };
        let Some((eye, quads, fires)) = crash_sedan_view(0.0) else {
            return;
        };
        assert_eq!(fires.len(), 3, "{fires:?}");
        let seen = smoke(eye, &quads, &fires);
        assert!(!seen.is_empty(), "no fire smoke drawn");
        for (dist, alpha) in &seen {
            // the fade's ramp at the sprite's own distance, a step over
            let fade = (1.0 - (dist - 1000.0) / 200.0).clamp(0.0, 1.0);
            assert!(*alpha <= fade + 1.0 / 255.0, "{dist} {alpha}");
        }
        let max = seen.iter().map(|s| s.1).fold(0.0, f32::max);
        println!("{} smoke sprites, alpha at most {max}", seen.len());
        assert!(max < 0.2, "{seen:?}");
        let (eye, quads, fires) = crash_sedan_view(180.0).unwrap();
        assert_eq!(smoke(eye, &quads, &fires), vec![]);
    }

    /// mp_backlot's `weather/hawk` (createfx at (496.7, 63.3, 275.5),
    /// angles (270, 0, 0)): its one oriented sprite spawns 1000 to 1500
    /// units over the effect in the world's axes, its element flags
    /// lacking 0x2 (`FX_GetSpawnOrigin`, cod4-effects.md 2), and circles
    /// there. In the effect's axes the offset turned onto world -x and the
    /// hawks wheeled at the effect's own height, through a building.
    /// Vacuous without `COD4_DIR`.
    #[test]
    fn backlot_hawks_circle_over_the_town() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let ((_, mut e), _) = crate::cod4_fly::load_av(install.root(), "mp_backlot").unwrap();
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let mut fx = FxSystem::new();
        for (k, v) in e.take_effects().unwrap() {
            fx.insert(&k, v);
        }
        let mut now = 100.0f64;
        e.start_ambience(&mut fx, &fs, (60_000, now));
        let mut heights = Vec::new();
        for i in 0..600 {
            e.run_loops(&mut fx, &fs, now);
            fx.step(1.0 / 60.0, now as f32, None);
            if i % 60 == 0 {
                for q in fx.build_quads(Vec3::ZERO, Vec3::Y, Vec3::Z, now as f32) {
                    if q.shader.contains("hawk") {
                        let c = q.verts.iter().map(|v| Vec3::from(*v)).sum::<Vec3>() / 4.0;
                        heights.push(c.z);
                    }
                }
            }
            now += 1.0 / 60.0;
        }
        println!("hawk heights {heights:?}");
        assert!(!heights.is_empty(), "no hawk drawn");
        assert!(
            heights.iter().all(|&z| (1100.0..1900.0).contains(&z)),
            "{heights:?}"
        );
    }
}
