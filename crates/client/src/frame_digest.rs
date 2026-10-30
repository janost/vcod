//! `VCOD_FRAME_DIGEST=<path>`: a line per drawn game frame, for the
//! App-level behaviour gate (a `timedemo` playback, digested twice, must
//! match). Each line is `<frame> <serverTime> <hash>` and then the hash of
//! each part, all 64-bit FNV-1a. The parts are hashed in their own buffers
//! and folded in [`Part`] order, so two independent steps that swap places
//! in the frame do not change the line. Off, every call is one atomic load.
//!
//! Only what the frame already decides goes in. The effects' particles,
//! marks and lights, the brass and every `time` argument step on real time
//! and stay out. What the frame decides off the wall clock but still hands
//! on (the cues of chained effects, the CoD1 view weapon, the CoD1 HUD's
//! timed windows) goes in the `clock` part, which the line prints but the
//! frame's hash leaves out.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a frame decided, in the order the parts fold.
#[derive(Clone, Copy)]
pub enum Part {
    /// The view: eye, basis, projection, fov.
    View,
    /// The view weapon: pose, light origin, fov, bones.
    Vm,
    /// Brush model poses and model instances as handed to the renderer.
    Ents,
    /// The 2D draw list.
    Hud,
    /// Sound cues started: alias, source, delay.
    Snd,
    /// Effects and tracers spawned: path and place.
    Fx,
    /// `console::log::print` lines.
    Con,
    /// What [`clocked`] fed, and the CoD1 HUD's timed windows: printed, not
    /// in the frame's hash.
    Clock,
}

const PARTS: usize = 8;
const NAMES: [&str; PARTS] = ["view", "vm", "ents", "hud", "snd", "fx", "con", "clock"];
const BASIS: u64 = 0xcbf2_9ce4_8422_2325;

static ON: AtomicBool = AtomicBool::new(false);
static CLOCKED: AtomicBool = AtomicBool::new(false);
/// The CoD1 HUD's quads that ride the wall clock, from `Hud::build`.
static HUD_CLOCKED: Mutex<Option<std::ops::Range<usize>>> = Mutex::new(None);
static STATE: Mutex<Option<State>> = Mutex::new(None);

struct State {
    out: std::io::BufWriter<std::fs::File>,
    frame: u64,
    parts: [u64; PARTS],
}

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Opens `VCOD_FRAME_DIGEST`'s file when it is set.
pub fn init() {
    let Some(path) = std::env::var_os("VCOD_FRAME_DIGEST") else {
        return;
    };
    match std::fs::File::create(&path) {
        Ok(f) => {
            *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(State {
                out: std::io::BufWriter::new(f),
                frame: 0,
                parts: [BASIS; PARTS],
            });
            ON.store(true, Ordering::Relaxed);
        }
        Err(e) => log::warn!("VCOD_FRAME_DIGEST {}: {e}", path.to_string_lossy()),
    }
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn bytes(part: Part, b: &[u8]) {
    if !on() {
        return;
    }
    let i = match CLOCKED.load(Ordering::Relaxed) {
        true => Part::Clock,
        false => part,
    } as usize;
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.parts[i] = fnv(s.parts[i], b);
    }
}

/// Runs `f` with everything it feeds going to [`Part::Clock`].
pub fn clocked<R>(f: impl FnOnce() -> R) -> R {
    if !on() {
        return f();
    }
    CLOCKED.store(true, Ordering::Relaxed);
    let r = f();
    CLOCKED.store(false, Ordering::Relaxed);
    r
}

/// `Hud::build`'s quads in `range` are timed off the wall clock.
pub fn hud_clocked(range: std::ops::Range<usize>) {
    if on() {
        *HUD_CLOCKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(range);
    }
}

pub fn str(part: Part, s: &str) {
    // The length keeps "ab","c" apart from "a","bc".
    bytes(part, &(s.len() as u32).to_le_bytes());
    bytes(part, s.as_bytes());
}

pub fn u32(part: Part, v: u32) {
    bytes(part, &v.to_le_bytes());
}

pub fn f32s(part: Part, v: &[f32]) {
    if !on() {
        return;
    }
    let b: Vec<u8> = v.iter().flat_map(|f| f.to_bits().to_le_bytes()).collect();
    bytes(part, &b);
}

pub fn mat(part: Part, m: &glam::Mat4) {
    f32s(part, &m.to_cols_array());
}

/// The view the frame draws from; `frame.time` is wall-clock and stays out.
/// `vm_clocked`: the view weapon steps on the wall clock (CoD1's).
pub fn view(
    frame: &crate::renderer::Frame,
    vm: Option<&crate::renderer::VmDraw>,
    vm_clocked: bool,
) {
    if !on() {
        return;
    }
    mat(Part::View, &frame.view_proj);
    f32s(Part::View, &frame.eye.to_array());
    f32s(Part::View, &frame.fwd.to_array());
    f32s(Part::View, &frame.up.to_array());
    f32s(Part::View, &[frame.fov_x]);
    let Some(vm) = vm else {
        return u32(Part::Vm, 0);
    };
    u32(Part::Vm, 1);
    if vm_clocked {
        CLOCKED.store(true, Ordering::Relaxed);
    }
    mat(Part::Vm, &vm.transform);
    f32s(Part::Vm, &vm.light_origin.to_array());
    f32s(Part::Vm, &[vm.fov_x]);
    for set in &vm.bone_sets {
        u32(Part::Vm, set.len() as u32);
        for b in set {
            mat(Part::Vm, b);
        }
    }
    CLOCKED.store(false, Ordering::Relaxed);
}

/// Brush model poses, `(model, pose)` as `Renderer::set_submodels` takes.
pub fn submodels(models: &[(usize, glam::Mat4)]) {
    if !on() {
        return;
    }
    u32(Part::Ents, models.len() as u32);
    for (m, pose) in models {
        u32(Part::Ents, *m as u32);
        mat(Part::Ents, pose);
    }
}

/// Model instances as `Renderer::set_dynamic_models` takes them.
pub fn models(instances: &[crate::renderer::DynamicModelInstance]) {
    if !on() {
        return;
    }
    u32(Part::Ents, instances.len() as u32);
    for i in instances {
        u32(Part::Ents, i.model.0 as u32);
        mat(Part::Ents, &i.transform);
        f32s(Part::Ents, &i.light_origin.to_array());
        f32s(Part::Ents, &[i.shader_time.unwrap_or(f32::NAN)]);
        let bones = i.bones.as_deref().unwrap_or(&[]);
        u32(Part::Ents, bones.len() as u32);
        for b in bones {
            mat(Part::Ents, b);
        }
    }
}

/// The 2D list as `Renderer::set_hud_quads` takes it.
pub fn hud(quads: &[crate::hud::HudQuad]) {
    if !on() {
        return;
    }
    let clocked = HUD_CLOCKED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_default();
    for (i, q) in quads.iter().enumerate() {
        let part = match clocked.contains(&i) {
            true => Part::Clock,
            false => Part::Hud,
        };
        f32s(part, q.verts.as_flattened());
        f32s(part, q.uvs.as_flattened());
        f32s(part, &q.rgba);
        str(part, &q.texture);
    }
}

/// A sound cue as the frame asked for it, before the alias's pick.
pub fn cue(c: &crate::audio::cues::Cue) {
    if !on() {
        return;
    }
    use crate::audio::cues::Source;
    str(Part::Snd, &c.alias);
    let (kind, num, pos) = match c.source {
        Source::Point(p) => (0, u32::MAX, p),
        Source::Entity { num, pos } => (1, num, pos),
        Source::Tag { num, pos } => (2, num, pos),
    };
    u32(Part::Snd, kind);
    u32(Part::Snd, num);
    f32s(Part::Snd, &pos.to_array());
    f32s(Part::Snd, &[c.delay_s]);
}

/// The loop sounds a snapshot wants, `(key, alias, origin)` per entity,
/// as `AudioSystem::reconcile_loops` takes them.
pub fn loops(what: &str, wanted: &HashMap<u32, (i32, Option<String>, glam::Vec3)>) {
    if !on() {
        return;
    }
    str(Part::Snd, what);
    let mut ents: Vec<_> = wanted.iter().collect();
    ents.sort_by_key(|(e, _)| **e);
    for (e, (key, alias, pos)) in ents {
        u32(Part::Snd, *e);
        u32(Part::Snd, *key as u32);
        str(Part::Snd, alias.as_deref().unwrap_or(""));
        f32s(Part::Snd, &pos.to_array());
    }
}

/// An effect spawned at `pos`; `path` empty for a tracer.
pub fn effect(path: &str, pos: glam::Vec3) {
    if !on() {
        return;
    }
    str(Part::Fx, path);
    f32s(Part::Fx, &pos.to_array());
}

/// Ends a frame: writes its line when `server_time` is known (a game is
/// drawn), else drops what the frame fed (the menu, a map load).
pub fn end_frame(server_time: Option<i32>) {
    if !on() {
        return;
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = guard.as_mut() else { return };
    let parts = std::mem::replace(&mut s.parts, [BASIS; PARTS]);
    HUD_CLOCKED.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(t) = server_time else { return };
    let total = parts[..Part::Clock as usize]
        .iter()
        .fold(BASIS, |h, p| fnv(h, &p.to_le_bytes()));
    let mut line = format!("{} {t} {total:016x}", s.frame);
    for (name, p) in NAMES.iter().zip(parts) {
        line += &format!(" {name}={p:016x}");
    }
    s.frame += 1;
    let _ = writeln!(s.out, "{line}");
    let _ = s.out.flush();
}
