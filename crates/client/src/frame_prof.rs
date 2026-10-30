//! `VCOD_FRAME_PROFILE=<path>`: CPU milliseconds per frame by stage and GPU
//! milliseconds per pass group, for `tools/cod4_perf.sh`. Off, every call is
//! one atomic load.
//!
//! The file opens with `# stages <names>`, `# counts <names>` and `# gpu
//! <names>`, then a line per drawn frame, `f <frame> <wall ms> <stage
//! ms...> <count...>` (a `# size` line before the first and at a resize), and a
//! line per frame whose timestamps came back, `g <frame> <pass ms...>`
//! (read a few frames late), and where the adapter counts them a `p
//! <frame> <fragments...>` line of the main pass's fragment shader
//! invocations by draw kind ([`FragStats`], `# frags <names>`), and a `q
//! <frame> <ms...>` line of the finer GPU groups inside and between the
//! passes ([`GpuGroups`], `# groups <names>`). Stages are exclusive spans: a frame's time
//! outside every span is `wall - sum`. `tools/cod4_perf.py` summarizes.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Where a frame's CPU time goes, in file order.
#[derive(Clone, Copy)]
pub enum Stage {
    /// `pace_frame`'s sleep (`com_maxfps`).
    Pace,
    /// The session's step: packets, snapshots, prediction, menus.
    Net,
    /// The snapshot scene: entities interpolated, events gathered.
    Scene,
    /// Player bodies posed and skinned.
    Bodies,
    /// The other models and brush models placed.
    Pose,
    /// The view weapon.
    Vm,
    /// Events: sounds and effects started.
    Events,
    /// The session's effects started: tags, ambience, entity and loop fx.
    Fx,
    /// The effect system stepped.
    FxStep,
    /// The sound system stepped.
    Sound,
    /// Effect quads, clouds, marks and lights built and handed over.
    FxDraw,
    /// The game HUD's 2D draw list.
    Hud,
    /// Menus, console and the rest of the 2D.
    Ui,
    /// The renderer's visibility walk.
    Cull,
    /// The visible soups' indices gathered and ordered, dlights, uploads.
    Gather,
    /// Buffers, instances and uniforms written before encoding.
    Build,
    /// The surface texture acquired (blocks on the swap chain).
    Acquire,
    /// Command encoding.
    Encode,
    /// `CommandEncoder::finish`: wgpu validates, tracks and records into
    /// the driver every command encoding queued (wgpu 30 defers it).
    Finish,
    /// Queue submit and present.
    Submit,
}

const STAGES: usize = 20;
const NAMES: [&str; STAGES] = [
    "pace", "net", "scene", "bodies", "pose", "vm", "events", "fx", "fxstep", "sound", "fxdraw",
    "hud", "ui", "cull", "gather", "build", "acquire", "encode", "finish", "submit",
];

/// Per-frame counts, printed after the stages.
#[derive(Clone, Copy)]
pub enum Count {
    /// Draw calls in the world pass list.
    Draws,
    /// Dynamic model instances handed to the renderer.
    Models,
    /// Live particles.
    Particles,
    /// World triangles gathered by the cull.
    Tris,
    /// Visible soups.
    Soups,
    /// Entity surface draws issued, a run of one instance's surfaces on
    /// one skin counted once.
    EntDraws,
    /// Depth prepass draws (`r_vc_depthprepass`), world and entity.
    Prepass,
    /// CoD2 shadow cookie draws: casters into the atlas and receivers
    /// into the dynamic shadow.
    CookieDraws,
    /// 1 on a frame whose entities stayed out of the depth prepass
    /// because a feathered effect draws (cod4-client-perf.md 19). Since
    /// section 20 a CoD4 feather keeps them in, so only CoD2 counts it.
    Feathered,
    /// Pipelines set in the depth prepass and the main scene pass.
    Pipelines,
    /// Bind groups set in the depth prepass and the main scene pass.
    BindGroups,
    /// World draws replayed from a render bundle (cod4-client-perf.md 31).
    Bundled,
    /// World run bundles made anew because a run's commands changed.
    Rebundled,
    /// Draw calls in the depth prepass and the main scene pass, a bundle's
    /// counted (cod4-client-perf.md 38).
    Calls,
    /// Draw calls in CoD4's sun and spot shadow passes.
    ShadowDraws,
    /// The HUD's draw calls (its texture runs).
    HudDraws,
    /// Entity draw calls in the depth prepass and the main scene pass,
    /// the viewmodel's left out (cod4-client-perf.md 40).
    EntCalls,
    /// Of `EntCalls`, those replayed from a render bundle.
    EntBundled,
    /// Depth prepass world draws replayed from a render bundle.
    PreBundled,
}

const COUNTS: usize = 19;
const COUNT_NAMES: [&str; COUNTS] = [
    "draws",
    "models",
    "particles",
    "tris",
    "soups",
    "entdraws",
    "prepass",
    "cookiedraws",
    "feathered",
    "pipelines",
    "bindgroups",
    "bundled",
    "rebundled",
    "calls",
    "shadowdraws",
    "huddraws",
    "entcalls",
    "entbundled",
    "prebundled",
];

/// The GPU pass groups the renderer stamps, in file order.
/// `opaque` through `post` were one `scene` group: the main pass up to the
/// split, the split's work (shadow mask, SSAO, depth and scene copies), the
/// main pass after it (blends, effects), then the post passes and the HUD.
pub const GPU_NAMES: [&str; 8] = [
    "vcshadow",
    "cookies",
    "sunshadow",
    "opaque",
    "split",
    "blend",
    "post",
    "gamma",
];

static ON: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<State>> = Mutex::new(None);

struct State {
    out: std::io::BufWriter<std::fs::File>,
    frame: u64,
    begun: Instant,
    ms: [f64; STAGES],
    counts: [u64; COUNTS],
    /// The open spans, innermost last, and the one being charged.
    open: Vec<usize>,
    running: Option<(usize, Instant)>,
}

/// Opens `VCOD_FRAME_PROFILE`'s file when it is set.
pub fn init() {
    let Some(path) = std::env::var_os("VCOD_FRAME_PROFILE") else {
        return;
    };
    match std::fs::File::create(&path) {
        Ok(f) => {
            let mut out = std::io::BufWriter::new(f);
            let _ = writeln!(out, "# stages {}", NAMES.join(" "));
            let _ = writeln!(out, "# counts {}", COUNT_NAMES.join(" "));
            let _ = writeln!(out, "# gpu {}", GPU_NAMES.join(" "));
            let _ = writeln!(out, "# frags {}", FRAG_NAMES.join(" "));
            let _ = writeln!(out, "# groups {}", GROUP_NAMES.join(" "));
            *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(State {
                out,
                frame: 0,
                begun: Instant::now(),
                ms: [0.0; STAGES],
                counts: [0; COUNTS],
                open: Vec::new(),
                running: None,
            });
            ON.store(true, Ordering::Relaxed);
        }
        Err(e) => log::warn!("VCOD_FRAME_PROFILE {}: {e}", path.to_string_lossy()),
    }
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Times its scope into `stage`. Spans nest: an inner one pauses the
/// outer, so every stage's time is its own.
pub struct Span(bool);

pub fn span(stage: Stage) -> Span {
    if !on() {
        return Span(false);
    }
    with(|s| {
        s.switch(Some(stage as usize));
        s.open.push(stage as usize);
    });
    Span(true)
}

impl Span {
    /// Ends this span and starts `next` at the same instant.
    pub fn next(&mut self, next: Stage) {
        if self.0 {
            with(|s| {
                s.open.pop();
                s.switch(Some(next as usize));
                s.open.push(next as usize);
            });
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.0 {
            with(|s| {
                s.open.pop();
                let outer = s.open.last().copied();
                s.switch(outer);
            });
        }
    }
}

impl State {
    /// Charges the running stage up to now and runs `stage` from here.
    fn switch(&mut self, stage: Option<usize>) {
        let now = Instant::now();
        if let Some((i, t)) = self.running {
            self.ms[i] += (now - t).as_secs_f64() * 1000.0;
        }
        self.running = stage.map(|i| (i, now));
    }
}

fn with(f: impl FnOnce(&mut State)) {
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        f(s);
    }
}

pub fn count(c: Count, n: usize) {
    if !on() {
        return;
    }
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.counts[c as usize] += n as u64;
    }
}

/// The frame number the renderer stamps its timestamps with.
pub fn frame() -> u64 {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map_or(0, |s| s.frame)
}

/// Writes the frame's line and starts the next.
pub fn end_frame() {
    if !on() {
        return;
    }
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let now = Instant::now();
        let wall = (now - s.begun).as_secs_f64() * 1000.0;
        let mut line = format!("f {} {wall:.3}", s.frame);
        for ms in s.ms {
            line += &format!(" {ms:.3}");
        }
        for n in s.counts {
            line += &format!(" {n}");
        }
        let _ = writeln!(s.out, "{line}");
        s.frame += 1;
        s.begun = now;
        s.ms = [0.0; STAGES];
        s.counts = [0; COUNTS];
        if s.frame.is_multiple_of(120) {
            let _ = s.out.flush();
        }
    }
}

/// A frame's GPU pass groups, [`GPU_NAMES`] order.
pub fn gpu(frame: u64, ms: &[f32]) {
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let mut line = format!("g {frame}");
        for m in ms {
            line += &format!(" {m:.3}");
        }
        let _ = writeln!(s.out, "{line}");
    }
}

/// Notes a console line run at this frame (`--exec-after`) as `# exec
/// <frame> <line>`, so `cod4_perf.py --sweep` can cut the frames into
/// passes.
pub fn exec(line: &str) {
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(s.out, "# exec {} {line}", s.frame);
    }
}

/// Flushes the file (at exit).
pub fn flush() {
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = s.out.flush();
    }
}

/// The renderer's per-frame GPU stamps: a ring of query sets so a frame's
/// readback never waits on the GPU. Stamp `i` sits after pass group `i - 1`.
pub struct GpuStamps {
    ring: Vec<Slot>,
    next: usize,
    ns_per_tick: f32,
}

struct Slot {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// 0 free, 1 copy submitted, 2 mapped, 3 waiting on the map.
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    frame: u64,
}

const STAMPS: u32 = GPU_NAMES.len() as u32 + 1;

impl GpuStamps {
    /// Needs `TIMESTAMP_QUERY_INSIDE_ENCODERS`; `None` without it or with
    /// the profile off.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuStamps> {
        let need =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        if !on() || !device.features().contains(need) {
            return None;
        }
        let bytes = u64::from(STAMPS) * 8;
        let ring = (0..4)
            .map(|_| Slot {
                set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("profile timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: STAMPS,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("profile resolve"),
                    size: bytes,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                read: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("profile read"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Default::default(),
                frame: 0,
            })
            .collect();
        Some(GpuStamps {
            ring,
            next: 0,
            ns_per_tick: queue.get_timestamp_period(),
        })
    }

    /// The slot this frame stamps into, when one is free.
    pub fn begin(&mut self) -> Option<usize> {
        let i = self.next;
        (self.ring[i].state.load(Ordering::Acquire) == 0).then(|| {
            self.ring[i].frame = frame();
            i
        })
    }

    pub fn stamp(&self, slot: Option<usize>, encoder: &mut wgpu::CommandEncoder, index: u32) {
        if let Some(i) = slot {
            encoder.write_timestamp(&self.ring[i].set, index);
        }
    }

    /// After the last stamp, before submit.
    pub fn resolve(&mut self, slot: Option<usize>, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = slot else { return };
        let s = &self.ring[i];
        encoder.resolve_query_set(&s.set, 0..STAMPS, &s.resolve, 0);
        encoder.copy_buffer_to_buffer(&s.resolve, 0, &s.read, 0, None);
        s.state.store(1, Ordering::Release);
        self.next = (i + 1) % self.ring.len();
    }

    /// After submit: maps the submitted copies and writes the mapped ones.
    pub fn collect(&mut self) {
        for s in &self.ring {
            match s.state.load(Ordering::Acquire) {
                1 => {
                    s.state.store(3, Ordering::Release);
                    let state = s.state.clone();
                    s.read.map_async(wgpu::MapMode::Read, .., move |r| {
                        state.store(if r.is_ok() { 2 } else { 0 }, Ordering::Release);
                    });
                }
                2 => {
                    let ticks: Vec<u64> = match s.read.get_mapped_range(..) {
                        Ok(view) => bytemuck::pod_collect_to_vec(&view[..]),
                        Err(_) => vec![0; STAMPS as usize],
                    };
                    s.read.unmap();
                    s.state.store(0, Ordering::Release);
                    let ms: Vec<f32> = ticks
                        .windows(2)
                        .map(|w| w[1].saturating_sub(w[0]) as f32 * self.ns_per_tick / 1.0e6)
                        .collect();
                    gpu(s.frame, &ms);
                }
                _ => {}
            }
        }
    }
}

/// The device features the GPU stamps need beyond the renderer's own:
/// none with the profile off, so a normal run asks for nothing new.
pub fn extra_features() -> wgpu::Features {
    if on() {
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
    } else {
        wgpu::Features::empty()
    }
}

/// Notes the frame's size and MSAA samples as a `# size WxH samples N`
/// line when either changes (a resize, a `vid_restart`).
pub fn size(w: u32, h: u32) {
    if !on() {
        return;
    }
    static LAST: Mutex<(u32, u32, u32)> = Mutex::new((0, 0, 0));
    let samples = crate::renderer::msaa_samples();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last != (w, h, samples) {
        *last = (w, h, samples);
        if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = writeln!(s.out, "# size {w}x{h} samples {samples}");
        }
    }
}

/// What a main pass draw is, for the fragment counts ([`FragStats`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frag {
    World,
    /// Entity model surfaces.
    Ents,
    /// The view weapon.
    Gun,
    Marks,
    /// Effect runs in the plain pipelines (clouds and distortion too).
    Fx,
    /// Effect runs reading the depth copy (`zfeather`, `vc_softParticles`).
    FxSoft,
    Dlight,
    /// CoD1 static models (`misc_model` props); `World` elsewhere.
    Props,
}

pub const FRAG_NAMES: [&str; 8] = [
    "world", "ents", "gun", "marks", "fx", "fxsoft", "dlight", "props",
];

/// `VCOD_FRAG_STATS` set, or the profile on: the renderer counts the
/// fragment shader invocations of the main pass by [`Frag`] (a pipeline
/// statistics query per run of one kind), a pixel's worth of overdraw per
/// fragment whatever the MSAA. The profile gets a `p <frame> <count...>`
/// line per frame read back ([`FRAG_NAMES`] order).
pub fn frag_stats_on() -> bool {
    static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    on() || *ENV.get_or_init(|| std::env::var_os("VCOD_FRAG_STATS").is_some())
}

/// The device features [`FragStats`] needs, none while it is off.
pub fn frag_features() -> wgpu::Features {
    if frag_stats_on() {
        wgpu::Features::PIPELINE_STATISTICS_QUERY
    } else {
        wgpu::Features::empty()
    }
}

const FRAG_QUERIES: u32 = 4096;

struct FragSlot {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// As [`Slot::state`].
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    frame: u64,
    kinds: Vec<Frag>,
}

/// The main pass's fragment counts by [`Frag`], read back a few frames
/// late through a ring of query sets as [`GpuStamps`] are.
pub struct FragStats {
    ring: Vec<FragSlot>,
    next: usize,
    slot: Option<usize>,
    open: Option<Frag>,
    /// The last frame read back: its number and counts.
    pub last: Option<(u64, [u64; FRAG_NAMES.len()])>,
}

impl FragStats {
    /// `None` while [`frag_stats_on`] is off or without the feature.
    pub fn new(device: &wgpu::Device) -> Option<FragStats> {
        if !frag_stats_on()
            || !device
                .features()
                .contains(wgpu::Features::PIPELINE_STATISTICS_QUERY)
        {
            return None;
        }
        let bytes = u64::from(FRAG_QUERIES) * 8;
        let ring = (0..4)
            .map(|_| FragSlot {
                set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("fragment counts"),
                    ty: wgpu::QueryType::PipelineStatistics(
                        wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS,
                    ),
                    count: FRAG_QUERIES,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("fragment counts resolve"),
                    size: bytes,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                read: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("fragment counts read"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Default::default(),
                frame: 0,
                kinds: Vec::new(),
            })
            .collect();
        Some(FragStats {
            ring,
            next: 0,
            slot: None,
            open: None,
            last: None,
        })
    }

    /// Takes a free slot for this frame's counts, if there is one.
    pub fn begin_frame(&mut self, frame: u64) {
        let i = self.next;
        let s = &mut self.ring[i];
        self.slot = (s.state.load(Ordering::Acquire) == 0).then(|| {
            s.frame = frame;
            s.kinds.clear();
            i
        });
        self.open = None;
    }

    /// The draws from here on are `kind`'s (`None`: uncounted). Call with
    /// `None` before the pass ends.
    pub fn switch(&mut self, pass: &mut wgpu::RenderPass, kind: Option<Frag>) {
        if self.open == kind {
            return;
        }
        let Some(i) = self.slot else { return };
        if self.open.take().is_some() {
            pass.end_pipeline_statistics_query();
        }
        let s = &mut self.ring[i];
        if let Some(k) = kind
            && (s.kinds.len() as u32) < FRAG_QUERIES
        {
            pass.begin_pipeline_statistics_query(&s.set, s.kinds.len() as u32);
            s.kinds.push(k);
            self.open = Some(k);
        }
    }

    /// After the main pass, before submit.
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.slot.take() else { return };
        let s = &self.ring[i];
        let n = s.kinds.len() as u32;
        if n > 0 {
            encoder.resolve_query_set(&s.set, 0..n, &s.resolve, 0);
            encoder.copy_buffer_to_buffer(&s.resolve, 0, &s.read, 0, u64::from(n) * 8);
        }
        s.state.store(1, Ordering::Release);
        self.next = (i + 1) % self.ring.len();
    }

    /// After submit: maps the submitted copies and sums the mapped ones.
    pub fn collect(&mut self) {
        for s in &mut self.ring {
            match s.state.load(Ordering::Acquire) {
                1 if s.kinds.is_empty() => s.state.store(0, Ordering::Release),
                1 => {
                    s.state.store(3, Ordering::Release);
                    let state = s.state.clone();
                    s.read.map_async(wgpu::MapMode::Read, .., move |r| {
                        state.store(if r.is_ok() { 2 } else { 0 }, Ordering::Release);
                    });
                }
                2 => {
                    let counts: Vec<u64> = match s.read.get_mapped_range(..) {
                        Ok(view) => bytemuck::pod_collect_to_vec(&view[..s.kinds.len() * 8]),
                        Err(_) => Vec::new(),
                    };
                    s.read.unmap();
                    s.state.store(0, Ordering::Release);
                    let mut sum = [0u64; FRAG_NAMES.len()];
                    for (k, n) in s.kinds.iter().zip(counts) {
                        sum[*k as usize] += n;
                    }
                    self.last = Some((s.frame, sum));
                    if let Some(st) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        let mut line = format!("p {}", s.frame);
                        for n in sum {
                            line += &format!(" {n}");
                        }
                        let _ = writeln!(st.out, "{line}");
                    }
                }
                _ => {}
            }
        }
    }
}

/// What a stretch of GPU work between two [`GpuGroups`] stamps was: the
/// main pass's draws by kind (world surfaces, the static models' lit
/// sets, entity surfaces, the view weapon, ...), and the passes between
/// and after it, each on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// The depth prepass (`r_vc_depthprepass`).
    Prepass,
    /// CoD2's shadow cookie receivers drawn over the world's prepass
    /// (`Renderer::encode_cookie_receivers`).
    Receivers,
    /// The main pass's clear and the sky.
    Sky,
    /// World surfaces, the static models' unlit and fx stages included.
    World,
    /// The static models' lit sets (`fs_cod4_model`, CoD2's `fs_smodel`).
    Smodel,
    /// Entity model surfaces.
    Ents,
    /// The view weapon.
    Gun,
    Marks,
    /// Effect runs in the plain pipelines (clouds and distortion too).
    Fx,
    /// Effect runs reading the depth copy.
    FxSoft,
    Dlight,
    /// The sun's flare and washes.
    Flare,
    /// `vc_shadows`' mask.
    ShadowMask,
    Ssao,
    /// The depth copy the feathered effects read.
    SoftCopy,
    /// The colour copy distortion reads.
    SceneCopy,
    /// CoD4's depth of field.
    Dof,
    Shafts,
    /// The glow and a CoD2 menu's world blur.
    Glow,
    Bloom,
    /// The post pass: film, grade, the glow's add.
    PostFx,
    /// The shellshock's and flashbang's screen blends.
    Shock,
    /// The game HUD and the debug text.
    Hud,
}

pub const GROUP_NAMES: [&str; 23] = [
    "prepass",
    "receivers",
    "sky",
    "world",
    "smodel",
    "ents",
    "gun",
    "marks",
    "fx",
    "fxsoft",
    "dlight",
    "flare",
    "shadowmask",
    "ssao",
    "softcopy",
    "scenecopy",
    "dof",
    "shafts",
    "glow",
    "bloom",
    "postfx",
    "shock",
    "hud",
];

const GROUP_STAMPS: u32 = 4096;

struct GroupSlot {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// As [`Slot::state`].
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    frame: u64,
    /// The group each stamp opens, `None` a gap no group owns.
    kinds: Vec<Option<Group>>,
}

/// GPU ms per [`Group`]: a timestamp wherever the work changes group,
/// in the encoder between passes and inside the main pass (needs
/// `TIMESTAMP_QUERY_INSIDE_PASSES`), the span to the next stamp charged
/// to the group it opened. Vulkan writes them at the bottom of the pipe,
/// so a group's time is when its last work finished past the last
/// group's: overlapping draws land in the later one. On with the profile
/// where the adapter has the features, unless `VCOD_GPU_GROUPS=0`; read
/// back a few frames late through a ring as [`GpuStamps`] are.
pub struct GpuGroups {
    ring: Vec<GroupSlot>,
    next: usize,
    slot: Option<usize>,
    open: Option<Group>,
    ns_per_tick: f32,
}

/// The device features [`GpuGroups`] needs, none with the profile off.
pub fn group_features() -> wgpu::Features {
    if on() && std::env::var_os("VCOD_GPU_GROUPS").is_none_or(|v| v != "0") {
        wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES
    } else {
        wgpu::Features::empty()
    }
}

impl GpuGroups {
    /// `None` with the profile off, `VCOD_GPU_GROUPS=0` or without the
    /// features.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuGroups> {
        let need = group_features();
        if need.is_empty() || !device.features().contains(need) {
            return None;
        }
        let bytes = u64::from(GROUP_STAMPS) * 8;
        let ring = (0..4)
            .map(|_| GroupSlot {
                set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("profile group timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: GROUP_STAMPS,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("profile group resolve"),
                    size: bytes,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                read: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("profile group read"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Default::default(),
                frame: 0,
                kinds: Vec::new(),
            })
            .collect();
        Some(GpuGroups {
            ring,
            next: 0,
            slot: None,
            open: None,
            ns_per_tick: queue.get_timestamp_period(),
        })
    }

    /// Takes a free slot for this frame's stamps, if there is one.
    pub fn begin_frame(&mut self) {
        let i = self.next;
        let s = &mut self.ring[i];
        self.slot = (s.state.load(Ordering::Acquire) == 0).then(|| {
            s.frame = frame();
            s.kinds.clear();
            i
        });
        self.open = None;
    }

    /// The next stamp's index when `group` changes what is open; the
    /// last stamp is kept for the closing `None`.
    fn take(&mut self, group: Option<Group>) -> Option<(usize, u32)> {
        if self.open == group {
            return None;
        }
        let i = self.slot?;
        let s = &mut self.ring[i];
        let n = s.kinds.len() as u32;
        if n + 1 >= GROUP_STAMPS && group.is_some() {
            return None;
        }
        s.kinds.push(group);
        self.open = group;
        Some((i, n))
    }

    /// The encoder's work from here on is `group`'s (between passes).
    pub fn enc(&mut self, encoder: &mut wgpu::CommandEncoder, group: Option<Group>) {
        if let Some((i, n)) = self.take(group) {
            encoder.write_timestamp(&self.ring[i].set, n);
        }
    }

    /// The pass's draws from here on are `group`'s.
    pub fn pass(&mut self, pass: &mut wgpu::RenderPass, group: Option<Group>) {
        if let Some((i, n)) = self.take(group) {
            pass.write_timestamp(&self.ring[i].set, n);
        }
    }

    /// After the last stamp (an `enc(None)`), before submit.
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.slot.take() else { return };
        let s = &self.ring[i];
        let n = s.kinds.len() as u32;
        if n > 0 {
            encoder.resolve_query_set(&s.set, 0..n, &s.resolve, 0);
            encoder.copy_buffer_to_buffer(&s.resolve, 0, &s.read, 0, u64::from(n) * 8);
        }
        s.state.store(1, Ordering::Release);
        self.next = (i + 1) % self.ring.len();
    }

    /// After submit: maps the submitted copies and writes the mapped ones.
    pub fn collect(&mut self) {
        for s in &mut self.ring {
            match s.state.load(Ordering::Acquire) {
                1 if s.kinds.is_empty() => s.state.store(0, Ordering::Release),
                1 => {
                    s.state.store(3, Ordering::Release);
                    let state = s.state.clone();
                    s.read.map_async(wgpu::MapMode::Read, .., move |r| {
                        state.store(if r.is_ok() { 2 } else { 0 }, Ordering::Release);
                    });
                }
                2 => {
                    let ticks: Vec<u64> = match s.read.get_mapped_range(..) {
                        Ok(view) => bytemuck::pod_collect_to_vec(&view[..s.kinds.len() * 8]),
                        Err(_) => Vec::new(),
                    };
                    s.read.unmap();
                    s.state.store(0, Ordering::Release);
                    let ms = group_ms(&s.kinds, &ticks, self.ns_per_tick);
                    if let Some(st) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        let mut line = format!("q {}", s.frame);
                        for m in ms {
                            line += &format!(" {m:.3}");
                        }
                        let _ = writeln!(st.out, "{line}");
                    }
                }
                _ => {}
            }
        }
    }
}

/// Each stamp's span to the next, in ms, summed by the group it opened.
fn group_ms(kinds: &[Option<Group>], ticks: &[u64], ns_per_tick: f32) -> [f32; GROUP_NAMES.len()] {
    let mut ms = [0.0f32; GROUP_NAMES.len()];
    for (k, w) in kinds.iter().zip(ticks.windows(2)) {
        if let Some(g) = k {
            ms[*g as usize] += w[1].saturating_sub(w[0]) as f32 * ns_per_tick / 1.0e6;
        }
    }
    ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_spans_charge_the_group_they_open() {
        let kinds = [
            Some(Group::Sky),
            Some(Group::World),
            Some(Group::Ents),
            Some(Group::World),
            None,
            Some(Group::Hud),
            None,
        ];
        let ticks = [
            0, 1_000_000, 3_000_000, 4_000_000, 8_000_000, 9_000_000, 9_500_000,
        ];
        let ms = group_ms(&kinds, &ticks, 1.0);
        assert_eq!(ms[Group::Sky as usize], 1.0);
        assert_eq!(ms[Group::World as usize], 6.0);
        assert_eq!(ms[Group::Ents as usize], 1.0);
        assert_eq!(ms[Group::Hud as usize], 0.5);
        assert_eq!(ms.iter().sum::<f32>(), 8.5);
        assert_eq!(GROUP_NAMES[Group::Hud as usize], "hud");
    }
}
