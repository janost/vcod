mod audio;
mod camera;
mod entities;
mod fx;
mod hud;
mod hud_text;
mod loading;
mod probe;
mod quick_chat;
mod renderer;
mod sky;
mod viewmodel;

use anyhow::{anyhow, bail, Context, Result};
use clap::{CommandFactory, Parser};
use glam::Vec3;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use vcod_common::pk3::Pk3Fs;
use vcod_common::{bsp, collision, mesh, net, pmove, props, skeleton, weapon, xanim, xmodel};

use camera::{FlyCamera, InputState};
use renderer::{DynamicModelInstance, Renderer};

#[derive(Parser)]
#[command(about = "Call of Duty (2003) map viewer and spectator client")]
struct Args {
    /// Map name, e.g. mp_pavlov
    map: Option<String>,
    /// List all maps found in the pk3 search path
    #[arg(long)]
    list: bool,
    /// Game install (holds main/). Defaults to $COD_DIR, else the executable's directory.
    #[arg(long, default_value_os_t = vcod_common::game_dir::default_game_dir())]
    game_dir: std::path::PathBuf,
    /// Pk3 subdirectory: main for CoD1, uo for United Offensive (untested)
    #[arg(long, default_value = "main")]
    mod_dir: String,
    /// Spawn as a soldier and walk (first person) instead of flying
    #[arg(long)]
    walk: bool,
    /// Start with the debug overlay visible (F3 toggles it at runtime)
    #[arg(long)]
    debug_overlay: bool,
    /// Headless: connect to a CoD server, dump the gamestate, then exit
    #[arg(long)]
    net_probe: Option<String>,
    /// Connect to a CoD server (ip:port) and spectate
    #[arg(long)]
    connect: Option<String>,
    /// Overwrite the committed gamestate.bin fixture with the --net-probe capture.
    /// Off by default: the parser tests pin that file, and a capture from another
    /// map or a mid-round server is not a drop-in. Otherwise the probe writes to tmp/.
    #[arg(long)]
    save_fixture: bool,
    /// Overwrite the committed snapshots.bin fixture only; gamestate.bin stays pinned.
    #[arg(long)]
    save_snapshots: bool,
    /// Write the retail server's non-empty configstring table to
    /// crates/server/tests/fixtures/configstrings/<map>-<gametype>.txt, the
    /// fixture crates/server/tests/configstrings_ab.rs diffs against. Separate
    /// from --save-fixture because that pair pins the parser's byte-exact
    /// captures and this one pins the game module's table.
    #[arg(long)]
    save_configstrings: bool,
    /// Join a team through the stock menu and write the retail playerstate to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>.txt, the
    /// fixture crates/server/tests/playerstate_ab.rs diffs against.
    #[arg(long)]
    save_playerstate: bool,
    /// Join a team, then hold each movement input in turn and write the
    /// retail playerstate settled under each to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-motion.txt.
    /// Separate from --save-playerstate because that one pins a standing
    /// player, which leaves every field the client predicts at zero.
    #[arg(long)]
    save_motion: bool,
    /// Join a team, then run a scripted weapon sequence -- single shot,
    /// sustained fire, reload, fire crouched and prone -- and write the retail
    /// playerstate to
    /// crates/server/tests/fixtures/playerstate/<map>-<gametype>-combat.txt.
    /// Separate from --save-motion because that one samples a settled state
    /// and a shot is a transient: this fixture carries every snapshot, since
    /// the playerstate event ring holds four slots and overwrites as it fires.
    /// Each firing step waits for weaponstate to read ready before it taps, and
    /// the reload holds the weapon file's reloadTime, so a weapon still busy
    /// from the step before cannot be recorded under a firing label. The
    /// sequence takes about 40 s, inside the default --probe-secs.
    #[arg(long)]
    save_combat: bool,
    /// Join a team, walk the --probe-pvs route and write the entity trace to
    /// crates/server/tests/fixtures/entities/<map>-<gametype>.txt, the fixture
    /// crates/server/tests/entities_ab.rs diffs against. Separate from
    /// --probe-pvs because that one only prints: this one overwrites committed
    /// evidence. Each run appends its stations to the map's fixture, since one
    /// walk gets one random spawn; delete the file to start that map over.
    #[arg(long)]
    save_entities: bool,
    /// Join a team, then walk a route and print the snapshot entity list at
    /// each station: which entities the server sent, and every add and removal
    /// along the way with the position it happened at. Answers whether the
    /// server culls the entity list by where the client stands. Writes no
    /// fixture; the route is relative to the spawn, so its stations are
    /// positions in one run, not reproducible places.
    #[arg(long)]
    probe_pvs: bool,
    /// Suffix for the --save-entities fixture name, so a capture taken under
    /// different conditions lands beside the plain one rather than on top of
    /// it: --capture-tag players writes <map>-<gametype>-players.txt.
    #[arg(long)]
    capture_tag: Option<String>,
    /// Which team --net-probe answers the stock team menu with: allies, axis,
    /// autoassign or spectator. On its own it makes the probe join and then
    /// report the roster once a second, which is how two probes on opposite
    /// teams measured what clientState.team carries.
    #[arg(long)]
    probe_team: Option<String>,
    /// Seconds --net-probe stays connected; an SD round boundary needs a few minutes
    #[arg(long, default_value_t = 65)]
    probe_secs: u64,
    /// Run without sound (also what happens when no output device opens).
    #[arg(long)]
    no_audio: bool,
    /// Master volume, 0.0 to 1.0.
    #[arg(long, default_value_t = 1.0)]
    volume: f32,
}

/// The map every mode reads: the renderer builds from it, entities resolve
/// submodels in it. `None` while the spectator is between maps.
struct World {
    bsp: bsp::Bsp,
}

/// Where the spectator is between connecting and drawing a map.
enum Phase {
    Connecting {
        since: Instant,
    },
    Loading {
        loader: loading::MapLoader,
    },
    /// Boxed to keep the variants a similar size.
    Live(Box<LivePhase>),
}

/// Everything the spectator needs to draw a map.
struct LivePhase {
    world: collision::CollisionWorld,
    scene: entities::EntityScene,
    events: net::events::EventTracker,
    clock: ServerClock,
    seeded: bool,
    last_loop_snap: Option<u32>,
}

fn live_phase(fs: &Pk3Fs, bsp: &bsp::Bsp) -> Phase {
    Phase::Live(Box::new(LivePhase {
        world: collision::CollisionWorld::build(bsp, &props::collision_tris(fs, &bsp.entities)),
        scene: entities::EntityScene::new(),
        events: net::events::EventTracker::new(),
        clock: ServerClock::new(),
        seeded: false,
        last_loop_snap: None,
    }))
}

enum Mode {
    Fly(FlyCamera),
    /// Position follows the interpolated server playerState; look angles are
    /// local and echoed to the server each frame, like a Q3 free spectator.
    Spectate {
        /// Boxed to keep the variants a similar size.
        net: Box<net::NetClient<net::UdpTransport>>,
        cam: FlyCamera,
        input: InputState,
        phase: Phase,
    },
    Walk {
        /// Boxed to keep the variants a similar size.
        world: Box<collision::CollisionWorld>,
        ps: pmove::PlayerState,
        input: pmove::PmInput,
        keys: WalkKeys,
        motion: viewmodel::ViewmodelMotion,
        /// `None` when the anims failed to load; the viewmodel then draws
        /// statically. Boxed to keep the variants a similar size.
        view_weapon: Option<Box<ViewWeapon>>,
        /// Minimal configstring table so weapon cues resolve through the same
        /// path as spectate: CS 7 carries [`WALK_LOADOUT`].
        configstrings: Vec<String>,
        /// Active index into [`WALK_LOADOUT`].
        weapon_slot: usize,
        /// Rounds behind the clip; refilled per weapon from its file.
        reserve: u32,
        /// Digit press latched until the next redraw loads that slot.
        switch_to: Option<usize>,
        /// Raw counts since the last frame, drained into the sway once per redraw.
        mouse_delta: (f32, f32),
        /// Press edges latched until the next redraw; `ads_held`/`fire_held` are level.
        fire_edge: bool,
        fire_held: bool,
        reload_edge: bool,
        ads_held: bool,
    },
}

struct ViewWeapon {
    skeleton: skeleton::Skeleton,
    pose: skeleton::PoseBuffer,
    state: weapon::WeaponState,
    def: weapon::WeaponDef,
    /// Missing entries fall back to Idle's clip.
    anims: HashMap<weapon::WeaponAnim, (xanim::XAnim, skeleton::AnimBinding)>,
}

/// Held movement keys, folded into `PmInput`'s float axes once per frame.
#[derive(Default)]
struct WalkKeys {
    w: bool,
    s: bool,
    a: bool,
    d: bool,
}

/// The walk-mode arsenal on number keys 1..=N: every retail archetype (semi
/// pistol, full-auto SMGs, auto rifle, big-clip bolt rifle) plus kar98k as
/// the baseline. Files carry `semiAuto`, `startAmmo`, `adsBobFactor`.
const WALK_LOADOUT: [&str; 6] = [
    "colt_mp",
    "thompson_mp",
    "mp40_mp",
    "mp44_mp",
    "enfield_mp",
    "kar98k_mp",
];

fn digit_slot(code: KeyCode) -> Option<usize> {
    Some(match code {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        KeyCode::Digit4 => 3,
        KeyCode::Digit5 => 4,
        KeyCode::Digit6 => 5,
        _ => return None,
    })
}

impl WalkKeys {
    /// (forward, right) in -1..1, opposite keys cancel.
    fn axes(&self) -> (f32, f32) {
        let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as f32;
        (axis(self.w, self.s), axis(self.d, self.a))
    }
}

/// Render this far behind the newest snapshot so a straddling pair is always
/// buffered; tolerates one dropped snapshot at 20 Hz.
const INTERP_DELAY_MS: i32 = 100;

/// Server now = newest snapshot time plus wall time since it was first seen,
/// so interpolation sweeps between 20 Hz snapshots instead of stepping. A new
/// snapshot re-anchors: continuous when on schedule, a snap after a long gap.
struct ServerClock {
    /// (newest snapshot server time, local ms it was first seen).
    anchor: Option<(i32, f64)>,
}

impl ServerClock {
    fn new() -> Self {
        Self { anchor: None }
    }

    /// Server time to interpolate at; `local_ms` is a monotonic wall clock.
    fn render_time(&mut self, local_ms: f64, newest: i32) -> i32 {
        match self.anchor {
            Some((t, _)) if t == newest => {}
            _ => self.anchor = Some((newest, local_ms)),
        }
        let (anchor_time, anchor_local) = self.anchor.unwrap();
        let server_now = anchor_time as f64 + (local_ms - anchor_local);
        (server_now - INTERP_DELAY_MS as f64) as i32
    }
}

/// F3 overlay text, top line first. A free function because the caller holds
/// the renderer borrowed out of `App`.
#[allow(clippy::too_many_arguments)]
fn hud_lines(
    mode: &Mode,
    // The spectate entity scene; fly/walk have none.
    scene: Option<&entities::EntityScene>,
    stats: &hud_text::HudStats,
    // (build_instances, render, fx step+build_quads) ms
    cpu_ms: (f32, f32, f32),
    // (events drained, of those unrecognized)
    ev_counts: (u64, u64),
    // (particles, decals, lights)
    fx_counts: (usize, usize, usize),
    // (hud quads, hud build+upload ms, Hud::unknown); zeros outside Spectate
    hud_counts: (usize, f32, u64),
    audio: audio::AudioStats,
    r: &Renderer,
) -> Vec<String> {
    let fps = if stats.dt_smooth > 0.0 {
        1.0 / stats.dt_smooth
    } else {
        0.0
    };
    let (build_ms, render_ms, fx_ms) = cpu_ms;
    let mut lines = vec![
        format!(
            "fps {fps:5.1}  ({:5.2} ms, worst {:5.1})",
            stats.dt_smooth * 1000.0,
            stats.worst_ms
        ),
        format!("cpu ms: build {build_ms:5.2}  render {render_ms:5.2}"),
    ];
    let (fx_particles, fx_decals, fx_lights) = fx_counts;
    lines.push(format!(
        "fx p{fx_particles} d{fx_decals} l{fx_lights} {fx_ms:.2}ms"
    ));
    let (draws, insts, bones) = r.debug_counts();
    lines.push(format!("draw: world {draws}  inst {insts}  bones {bones}"));
    let vc = r.vis_counts();
    lines.push(format!(
        "vis: {} cells {}/{} soups {}/{} tris {}/{} props {}/{} occ {} {}  {:.2}ms",
        vc.mode.map_or("-".to_string(), |m| m.to_string()),
        vc.cells.0,
        vc.cells.1,
        vc.soups.0,
        vc.soups.1,
        vc.tris.0,
        vc.tris.1,
        vc.props.0,
        vc.props.1,
        vc.occluders,
        vc.portals_occluded,
        vc.gather_ms
    ));
    let (hud_quads, hud_ms, hud_unknown) = hud_counts;
    lines.push(format!("hud q{hud_quads} {hud_ms:.2}ms unk {hud_unknown}"));
    let (stage_draws, dropped, warns) = r.shader_stats();
    lines.push(format!(
        "shader: sky {}  stages {stage_draws}  dropped {dropped}  warns {warns}",
        r.sky_name().unwrap_or("none")
    ));
    lines.push(format!("fog: {}", r.fog_debug()));
    lines.push(format!(
        "audio v{} plays {} miss {} cull {} drop {} steal {} {:.2}ms",
        audio.voices,
        audio.plays,
        audio.misses,
        audio.culled,
        audio.drops,
        audio.steals,
        audio.step_ms
    ));
    let cam_line = |tag: &str, pos: Vec3, yaw: f32, pitch: f32| {
        format!(
            "{tag}: {:6.0} {:6.0} {:5.0}  yaw {:4.0} pitch {:3.0}",
            pos.x,
            pos.y,
            pos.z,
            yaw.to_degrees(),
            pitch.to_degrees()
        )
    };
    match mode {
        Mode::Fly(cam) => lines.push(cam_line("fly", cam.pos, cam.yaw, cam.pitch)),
        Mode::Walk { ps, .. } => {
            lines.push(cam_line("walk", ps.origin, ps.yaw, ps.pitch));
            lines.push(format!(
                "vel {:4.0}  ground {}",
                ps.velocity.length(),
                ps.on_ground as u8
            ));
        }
        Mode::Spectate { net, cam, .. } => {
            lines.push(cam_line("spec", cam.pos, cam.yaw, cam.pitch));
            lines.push(format!(
                "net: {:?}  drops {}",
                net.state(),
                net.packet_drops()
            ));
            if let Some(snap) = net.snapshots().newest() {
                lines.push(format!(
                    "snap: ents {}  players {}  age {:3} ms",
                    snap.entities.len(),
                    snap.clients.len(),
                    net.snapshot_age().as_millis()
                ));
            }
            lines.push(format!(
                "interp miss/s {:4.1}  anim restarts/s {:4.1}",
                stats.misses_per_s, stats.restarts_per_s
            ));
            if let Some(scene) = scene {
                if scene.stats.pending_assemblies > 0 {
                    lines.push(format!(
                        "loading: {} assemblies pending",
                        scene.stats.pending_assemblies
                    ));
                }
            }
            if let Some((got, size)) = net.download_progress() {
                lines.push(format!("download: {got}/{size} bytes"));
            }
            let (ev_seen, ev_unknown) = ev_counts;
            lines.push(format!("ev {ev_seen} unk {ev_unknown}"));
        }
    }
    lines
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let dir = args.game_dir.join(&args.mod_dir);

    if let Some(addr) = &args.net_probe {
        // Sound cue resolution needs the game data, the wire-level prints do
        // not, so a failed open only costs the audio line.
        let fs = match Pk3Fs::open(&dir) {
            Ok(fs) => Some(fs),
            Err(e) => {
                log::warn!(
                    "cannot open game data in {} ({e:#}); the probe will not resolve sound aliases",
                    dir.display()
                );
                None
            }
        };
        return probe::probe(
            addr,
            probe::Save {
                fixture: args.save_fixture,
                snapshots: args.save_snapshots,
                configstrings: args.save_configstrings,
                playerstate: args.save_playerstate,
                motion: args.save_motion,
                combat: args.save_combat,
                entities: args.save_entities,
            },
            args.capture_tag.clone(),
            args.probe_pvs,
            args.probe_team.as_deref(),
            args.probe_secs,
            fs.as_ref(),
        );
    }

    let fs =
        Pk3Fs::open(&dir).with_context(|| format!("cannot open game data in {}", dir.display()))?;

    if args.list {
        for name in fs.find_maps() {
            println!("{name}");
        }
        return Ok(());
    }

    let net_client = match &args.connect {
        Some(addr) => Some(
            net::NetClient::connect(addr)
                .with_context(|| format!("cannot open a socket to {addr}"))?,
        ),
        None => None,
    };

    // Fly and walk need their map up front; spectate learns it from the
    // server inside the loop and loads through the same path as a map change.
    let local = if net_client.is_none() {
        let Some(map) = args.map.as_deref() else {
            Args::command()
                .error(
                    clap::error::ErrorKind::MissingRequiredArgument,
                    "a map name is required (or pass --list to see the available maps)",
                )
                .exit();
        };
        let Some(path) = fs.resolve_map(map) else {
            let all = fs.find_maps();
            let needle = map.to_lowercase();
            let similar: Vec<String> = all
                .iter()
                .filter(|m| m.to_lowercase().contains(&needle))
                .cloned()
                .collect();
            let suggestions = if similar.is_empty() { all } else { similar };
            bail!("map '{map}' not found; similar: {}", suggestions.join(", "));
        };
        let data = fs
            .read(&path)
            .with_context(|| format!("cannot read {path}"))?;
        let bsp = bsp::parse(&data).with_context(|| format!("cannot parse {path}"))?;
        Some((map.to_string(), bsp))
    } else {
        None
    };

    let (viewmodel, view_weapon) = if args.walk && net_client.is_none() {
        load_view_weapon(&fs, "kar98k_mp").unwrap_or_else(|| {
            log::warn!("no viewmodel; walking without one");
            (Vec::new(), None)
        })
    } else {
        (Vec::new(), None)
    };

    let hud = if net_client.is_some() {
        match hud::Hud::new(&fs) {
            Ok(hud) => Some(hud),
            Err(e) => {
                log::warn!("hud: {e}, disabling the on-screen HUD");
                None
            }
        }
    } else {
        None
    };

    // Fly and walk have no aliases, but `.efx` spawns carry cues and need a listener.
    let mut audio = audio::AudioSystem::new(
        &fs,
        audio::AudioOpts {
            enabled: !args.no_audio,
            volume: args.volume,
        },
    );
    if !audio.enabled() {
        println!("audio: silent (no output device, or --no-audio)");
    }

    let (mode, world, title) = if let Some(net) = net_client {
        (
            Mode::Spectate {
                net: Box::new(net),
                cam: FlyCamera::new(Vec3::ZERO, 0.0),
                input: InputState::default(),
                phase: Phase::Connecting {
                    since: Instant::now(),
                },
            },
            None,
            "vcod — connecting".to_string(),
        )
    } else {
        let (map, bsp) = local.expect("fly or walk without a local map");
        audio.on_gamestate(&map);
        // No server to send configstring 3; every stock MP map ships an
        // `ambient_<map>` alias (iw_sound.csv), unknown names just stay silent.
        let ambient = format!("ambient_{map}");
        audio.set_ambient(&fs, Some(&ambient));
        let mode = if args.walk {
            walk_mode(&map, &bsp, &fs, view_weapon)?
        } else {
            Mode::Fly(match bsp::find_spawn(&bsp.entities) {
                Some((origin, yaw)) => FlyCamera::new(Vec3::from(origin) + Vec3::Z * 60.0, yaw),
                None => {
                    let (min, max) = mesh::map_bounds(&bsp);
                    let center = (Vec3::from(min) + Vec3::from(max)) * 0.5;
                    log::warn!("no player spawn found, starting at the center of the map");
                    FlyCamera::new(center, 0.0)
                }
            })
        };
        (mode, Some(World { bsp }), format!("vcod — {map}"))
    };

    println!("click to capture mouse, Esc to release");
    if args.walk {
        println!("WASD move, Space jump, Ctrl crouch, Z prone, Q/E lean, Shift walk");
        println!("LMB fire, RMB aim, R reload, 1-6 weapons");
    } else if args.connect.is_some() {
        println!("WASD move (server-authoritative), mouse look; look up/down to ascend/descend");
    } else {
        println!("WASD + Space/Ctrl fly, Shift boost, scroll changes speed");
    }

    fx::registry::init(&fs);

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        title,
        world,
        fs,
        game_dir: args.game_dir.clone(),
        mod_dir: args.mod_dir.clone(),
        connect_addr: args.connect.clone(),
        mode,
        viewmodel,
        input: InputState::default(),
        window: None,
        renderer: None,
        grabbed: false,
        fx: fx::sim::FxSystem::new(),
        start: Instant::now(),
        last_frame: Instant::now(),
        debug_overlay: args.debug_overlay,
        cull_mode: renderer::CullMode::On,
        hud_stats: hud_text::HudStats::new(),
        interp_misses: 0,
        ev_seen: 0,
        ev_unknown: 0,
        build_ms: 0.0,
        render_ms: 0.0,
        fx_ms: 0.0,
        hud,
        hud_ms: 0.0,
        audio,
        quick_chat: quick_chat::QuickChat::new(0x51ee),
        error: None,
    };
    event_loop.run_app(&mut app)?;
    match app.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `ANGLE2SHORT` (q_shared.h).
fn angle2short(deg: f32) -> i32 {
    (deg * 65536.0 / 360.0) as i32 & 0xffff
}

/// Position is server-authoritative, so only movement axes and look angles;
/// `server_time` is filled by `send_frame`.
fn usercmd_from_input(input: &InputState, cam: &FlyCamera) -> net::msg::UserCmd {
    let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as i8 * 127;
    // Camera pitch is up-positive, usercmd pitch is down-positive.
    let pitch_deg = -cam.pitch.to_degrees();
    let yaw_deg = cam.yaw.to_degrees();
    net::msg::UserCmd {
        angles: [angle2short(pitch_deg), angle2short(yaw_deg), 0],
        forward: axis(input.forward, input.back),
        right: axis(input.right, input.left),
        // A nonzero `up` selects the full usercmd branch, which carries it;
        // the compact branch is the one that cannot (write_delta_usercmd).
        up: axis(input.up, input.down),
        ..Default::default()
    }
}

/// Synthetic muzzle for playerState-ring fire events (`entity_num ==
/// u32::MAX` in `net::events`): the ridden body is `skip_num`, never drawn,
/// so it has no real muzzle. 20 forward, 4 right, 3 down of the camera,
/// aimed down the view forward. Same shape as `BuiltScene::muzzles` entries.
fn view_muzzle(cam_pos: Vec3, forward: Vec3, right: Vec3, up: Vec3) -> (Vec3, Vec3) {
    (cam_pos + forward * 20.0 + right * 4.0 - up * 3.0, forward)
}

/// Leave the live map and enter Loading for the server's new map: drop the
/// GPU world and per-map state, reset what a gamestate invalidates, and list
/// the paks the client is missing.
#[allow(clippy::too_many_arguments)]
fn start_loading(
    net: &net::NetClient<net::UdpTransport>,
    hud: &mut Option<hud::Hud>,
    audio: &mut audio::AudioSystem,
    fx: &mut fx::sim::FxSystem,
    world: &mut Option<World>,
    r: &mut Renderer,
    window: Option<&Window>,
    title: &mut String,
    game_dir: &std::path::Path,
    mod_dir: &str,
) -> Result<Phase> {
    let map = net::info_value_for_key(net.configstring(0), "mapname")
        .map(str::to_string)
        .context("the server sent no mapname in its serverinfo")?;
    r.unload_world();
    *world = None;
    if let Some(hud) = hud {
        hud.on_gamestate();
    }
    audio.on_gamestate(&map);
    fx.clear();
    *title = format!("vcod — loading {map}");
    if let Some(window) = window {
        window.set_title(title);
    }
    // Same candidate order as the old blocking downloader; `MapLoader` caps it.
    let systeminfo = net.configstring(1).to_string();
    let candidates = net::download::candidates_for_map(&systeminfo, &map, mod_dir, |rel| {
        game_dir.join(rel).exists()
    })
    .into_iter()
    .filter_map(|name| {
        let rel = net::download::safe_rel_path(&name, mod_dir)?;
        Some((format!("{name}.pk3"), game_dir.join(rel)))
    })
    .collect();
    Ok(Phase::Loading {
        loader: loading::MapLoader::new(map, candidates),
    })
}

/// Parse the map, upload it to the GPU and hand back the live phase.
#[allow(clippy::too_many_arguments)]
fn load_map(
    map: &str,
    net: &net::NetClient<net::UdpTransport>,
    fs: &mut Pk3Fs,
    audio: &mut audio::AudioSystem,
    world: &mut Option<World>,
    r: &mut Renderer,
    window: Option<&Window>,
    title: &mut String,
) -> Result<Phase> {
    let Some(path) = fs.resolve_map(map) else {
        bail!("map '{map}' did not resolve in the pk3 search path");
    };
    let data = fs
        .read(&path)
        .with_context(|| format!("cannot read {path}"))?;
    let bsp = bsp::parse(&data).with_context(|| format!("cannot parse {path}"))?;
    // The ambient rides configstring 3 (docs/research/cod11-sound-system.md,
    // section 9); a fresh gamestate may have changed it.
    audio.set_ambient(fs, net::info_value_for_key(net.configstring(3), "n"));
    *title = format!("vcod — {map}");
    if let Some(window) = window {
        window.set_title(title);
    }
    let phase = live_phase(fs, &bsp);
    r.load_world(&bsp, fs)?;
    *world = Some(World { bsp });
    Ok(phase)
}

/// The between-maps frame: the status text over an empty view, with chat
/// still flowing through the normal HUD build (no snapshot clients).
#[allow(clippy::too_many_arguments)]
fn loading_frame(
    r: &mut Renderer,
    fs: &Pk3Fs,
    hud: &mut Option<hud::Hud>,
    now: f32,
    aspect: f32,
    cull: renderer::CullMode,
    configstrings: &[String],
    text: String,
) -> renderer::Frame {
    if let Some(hud) = hud {
        let (screen_w, screen_h) = r.screen_size();
        let no_clients = BTreeMap::new();
        let f = hud::HudFrame {
            now,
            screen_w,
            screen_h,
            configstrings,
            clients: &no_clients,
            protocol: &net::protocol::PROTOCOL_V1,
            server_time: 0,
            fs,
        };
        let quads = hud.build(&f);
        r.set_hud_quads(fs, quads);
    }
    renderer::Frame {
        // Nothing is drawn, so any projection works.
        view_proj: camera::view_proj_from(
            Vec3::ZERO,
            0.0,
            0.0,
            0.0,
            camera::DEFAULT_FOV_DEG,
            aspect,
        ),
        eye: Vec3::ZERO,
        fwd: camera::basis(0.0, 0.0).0,
        time: now,
        cull,
        hud_lines: vec![text],
    }
}

/// `size == 0` means no block has named the pak's size yet.
fn loading_text(map: &str, progress: Option<loading::Progress>) -> String {
    match progress {
        None => format!("Loading {map}"),
        Some(p) => {
            let size_kb = if p.size == 0 {
                "?".to_string()
            } else {
                format!("{}", p.size as u64 / 1024)
            };
            format!(
                "Downloading {map}: pak {}/{}  {} / {} KB",
                p.pak,
                p.paks,
                p.received / 1024,
                size_kb
            )
        }
    }
}

fn walk_mode(
    map: &str,
    bsp: &bsp::Bsp,
    fs: &Pk3Fs,
    view_weapon: Option<Box<ViewWeapon>>,
) -> Result<Mode> {
    let Some((origin, yaw)) = bsp::find_spawn(&bsp.entities) else {
        bail!("map {map} has no player spawn; run without --walk to fly");
    };
    let world = collision::CollisionWorld::build(bsp, &props::collision_tris(fs, &bsp.entities));
    if world.brushes.is_empty() && world.tris.is_empty() {
        bail!("no collidable geometry in {map}; run without --walk to fly");
    }

    let mut ps = pmove::PlayerState::spawn(Vec3::from(origin) + Vec3::Z * 2.0, yaw);
    let drop = world.box_trace(
        ps.origin,
        ps.origin - Vec3::Z * 4096.0,
        ps.mins(),
        ps.maxs(),
    );
    if drop.startsolid {
        // CoD spawns anyway; pmove usually pushes out of solid in the first frames.
        log::warn!("spawn point is inside solid geometry, spawning there anyway");
    } else if drop.fraction < 1.0 {
        ps.origin = drop.endpos;
    } else {
        log::warn!("no ground within 4096 units below the spawn point");
    }

    let mut configstrings = vec![String::new(); 8];
    // `entityState.weapon` is 1-based into CS 7; walk mode carries the loadout.
    configstrings[7] = WALK_LOADOUT.join(" ");
    let start_slot = WALK_LOADOUT
        .iter()
        .position(|&w| w == "kar98k_mp")
        .unwrap_or(0);
    let reserve = view_weapon.as_ref().map_or(0, |w| w.def.start_ammo);

    Ok(Mode::Walk {
        world: Box::new(world),
        ps,
        input: pmove::PmInput::default(),
        keys: WalkKeys::default(),
        motion: viewmodel::ViewmodelMotion::new(),
        view_weapon,
        configstrings,
        weapon_slot: start_slot,
        reserve,
        switch_to: None,
        mouse_delta: (0.0, 0.0),
        fire_edge: false,
        fire_held: false,
        reload_edge: false,
        ads_held: false,
    })
}

/// Hands first so the gun draws over them and the shared skeleton takes the
/// hands' bones as its base. `None` if a model is missing (walk mode then has
/// no viewmodel); the inner `None` means the models loaded but the anims did not.
fn load_view_weapon(
    fs: &Pk3Fs,
    name: &str,
) -> Option<(Vec<xmodel::XModel>, Option<Box<ViewWeapon>>)> {
    let text = fs.read(&format!("weapons/mp/{name}"))?;
    let weapon = xmodel::parse_weapon(&String::from_utf8_lossy(&text));
    let mut models = Vec::new();
    for key in ["handModel", "gunModel"] {
        let name = weapon.get(key)?;
        match xmodel::load(fs, name) {
            Ok(mut m) => {
                if key == "handModel" {
                    xmodel::apply_viewhands_placeholder_override(&mut m);
                }
                models.push(m);
            }
            Err(e) => {
                log::warn!("viewmodel {name}: {e:#}");
                return None;
            }
        }
    }
    let animated = load_anims(fs, &weapon, &models).map(Box::new);
    Some((models, animated))
}

/// A clip that is unnamed or fails to load is skipped and its state plays
/// idle. Without idle there is no fallback, so the rig is dropped and the
/// viewmodel draws in bind pose.
fn load_anims(
    fs: &Pk3Fs,
    weapon: &HashMap<String, String>,
    models: &[xmodel::XModel],
) -> Option<ViewWeapon> {
    let [hands, gun] = models else {
        return None;
    };
    // same order as set_viewmodel, so bone_sets[i] matches model i
    let skeleton = skeleton::Skeleton::build(&[hands, gun]);

    let mut anims = HashMap::new();
    for which in weapon::WeaponAnim::ALL {
        let key = which.key();
        let Some(name) = weapon.get(key).map(|n| n.trim()).filter(|n| !n.is_empty()) else {
            log::warn!("weapon: no {key}, that state will play idle");
            continue;
        };
        match xanim::load(fs, name) {
            Ok(anim) => {
                let binding = skeleton.bind(&anim);
                anims.insert(which, (anim, binding));
            }
            Err(e) => log::warn!("xanim {name} ({key}): {e:#}"),
        }
    }
    if !anims.contains_key(&weapon::WeaponAnim::Idle) {
        log::warn!("no idle anim loaded; drawing the viewmodel statically");
        return None;
    }

    let def = weapon::WeaponDef::from_map(weapon);
    Some(ViewWeapon {
        pose: skeleton::PoseBuffer::new(&skeleton),
        skeleton,
        state: weapon::WeaponState::new(def.clone()),
        def,
        anims,
    })
}

struct App {
    title: String,
    world: Option<World>,
    fs: Pk3Fs,
    /// Where downloads land and the pk3 path reopens from.
    game_dir: std::path::PathBuf,
    mod_dir: String,
    /// `--connect` target, for the connecting screen text.
    connect_addr: Option<String>,
    mode: Mode,
    viewmodel: Vec<xmodel::XModel>,
    /// Fly-mode keys; walk keeps its own in `Mode::Walk`.
    input: InputState,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    grabbed: bool,
    fx: fx::sim::FxSystem,
    /// Origin of the clock effects are timed against.
    start: Instant,
    last_frame: Instant,
    debug_overlay: bool,
    cull_mode: renderer::CullMode,
    hud_stats: hud_text::HudStats,
    /// Spectate frames without a straddling snapshot pair. Cumulative; the
    /// overlay shows the rate.
    interp_misses: u64,
    /// Cumulative events drained, and those with an unrecognized code.
    ev_seen: u64,
    ev_unknown: u64,
    build_ms: f32,
    render_ms: f32,
    fx_ms: f32,
    hud: Option<hud::Hud>,
    hud_ms: f32,
    audio: audio::AudioSystem,
    quick_chat: quick_chat::QuickChat,
    error: Option<anyhow::Error>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: anyhow::Error) {
        self.error = Some(err);
        event_loop.exit();
    }

    fn set_grab(&mut self, grabbed: bool) {
        let Some(window) = &self.window else { return };
        if grabbed {
            if window.set_cursor_grab(CursorGrabMode::Locked).is_err()
                && window.set_cursor_grab(CursorGrabMode::Confined).is_err()
            {
                log::warn!("this platform does not support grabbing the cursor");
                return;
            }
        } else if window.set_cursor_grab(CursorGrabMode::None).is_err() {
            return;
        }
        window.set_cursor_visible(!grabbed);
        self.grabbed = grabbed;
    }

    /// On grab release or focus loss, so no held key stays latched. Prone
    /// stays, it is a stance toggle. The scoreboard drops, Tab is held too.
    fn clear_held_keys(&mut self) {
        match &mut self.mode {
            Mode::Fly(_) => self.input = InputState::default(),
            Mode::Spectate { input, .. } => *input = InputState::default(),
            Mode::Walk {
                input,
                keys,
                fire_edge,
                fire_held,
                reload_edge,
                ads_held,
                ..
            } => {
                *keys = WalkKeys::default();
                input.jump = false;
                input.crouch = false;
                input.walk_slow = false;
                input.lean_left = false;
                input.lean_right = false;
                *fire_edge = false;
                *fire_held = false;
                *reload_edge = false;
                *ads_held = false;
            }
        }
        if let Some(hud) = &mut self.hud {
            hud.scoreboard.visible = false;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 900.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                return self.fail(
                    event_loop,
                    anyhow::Error::new(e).context("cannot create a window"),
                )
            }
        };
        match Renderer::new(window.clone(), &self.fs) {
            Ok(mut r) => {
                if let Some(w) = &self.world {
                    if let Err(e) = r.load_world(&w.bsp, &self.fs) {
                        return self.fail(event_loop, e);
                    }
                }
                if !self.viewmodel.is_empty() {
                    r.set_viewmodel(&self.fs, &self.viewmodel);
                }
                self.renderer = Some(r);
            }
            Err(e) => return self.fail(event_loop, e),
        }
        window.request_redraw();
        self.window = Some(window);
        self.last_frame = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // auto-repeat would retrigger the jump and the prone toggle
                if event.repeat {
                    return;
                }
                if code == KeyCode::Escape && pressed {
                    self.set_grab(false);
                    self.clear_held_keys();
                    return;
                }
                if code == KeyCode::F3 && pressed {
                    self.debug_overlay = !self.debug_overlay;
                    return;
                }
                if code == KeyCode::F4 && pressed {
                    self.cull_mode = self.cull_mode.next();
                    return;
                }
                let grabbed = self.grabbed;
                match &mut self.mode {
                    Mode::Fly(_) => match code {
                        KeyCode::KeyW => self.input.forward = pressed,
                        KeyCode::KeyS => self.input.back = pressed,
                        KeyCode::KeyA => self.input.left = pressed,
                        KeyCode::KeyD => self.input.right = pressed,
                        KeyCode::Space => self.input.up = pressed,
                        KeyCode::ControlLeft => self.input.down = pressed,
                        KeyCode::ShiftLeft => self.input.boost = pressed,
                        _ => {}
                    },
                    Mode::Spectate { net, input, .. } => match code {
                        KeyCode::KeyW => input.forward = pressed,
                        KeyCode::KeyS => input.back = pressed,
                        KeyCode::KeyA => input.left = pressed,
                        KeyCode::KeyD => input.right = pressed,
                        KeyCode::Space => input.up = pressed,
                        KeyCode::ControlLeft => input.down = pressed,
                        // The server never pushes scores: send `score` on the
                        // down edge, and every 2 s while held (see the redraw
                        // tick; docs/research/cod11-hud-protocol.md, section 4).
                        KeyCode::Tab => {
                            if let Some(hud) = &mut self.hud {
                                hud.scoreboard.visible = pressed;
                                if pressed {
                                    let now = (Instant::now() - self.start).as_secs_f32();
                                    net.send_reliable("score");
                                    hud.scoreboard.mark_requested(now);
                                }
                            }
                        }
                        _ => {}
                    },
                    Mode::Walk {
                        input,
                        keys,
                        reload_edge,
                        switch_to,
                        ..
                    } => match code {
                        // weapon actions count only while the mouse is captured
                        KeyCode::KeyR if pressed && grabbed => *reload_edge = true,
                        KeyCode::KeyW => keys.w = pressed,
                        KeyCode::KeyS => keys.s = pressed,
                        KeyCode::KeyA => keys.a = pressed,
                        KeyCode::KeyD => keys.d = pressed,
                        KeyCode::Space => input.jump = pressed,
                        KeyCode::ControlLeft => input.crouch = pressed,
                        KeyCode::KeyZ if pressed => input.prone = !input.prone,
                        KeyCode::KeyQ => input.lean_left = pressed,
                        KeyCode::KeyE => input.lean_right = pressed,
                        KeyCode::ShiftLeft => input.walk_slow = pressed,
                        _ => {
                            if pressed && grabbed {
                                if let Some(slot) = digit_slot(code) {
                                    *switch_to = Some(slot);
                                }
                            }
                        }
                    },
                }
            }
            WindowEvent::Focused(false) => self.clear_held_keys(),
            WindowEvent::MouseInput { state, button, .. } => {
                use winit::event::MouseButton;
                let pressed = state == ElementState::Pressed;
                // the click that captures the mouse must not also fire
                if button == MouseButton::Left && pressed && !self.grabbed {
                    self.set_grab(true);
                    return;
                }
                if !self.grabbed {
                    return;
                }
                let Mode::Walk {
                    fire_edge,
                    fire_held,
                    ads_held,
                    ..
                } = &mut self.mode
                else {
                    return;
                };
                match button {
                    MouseButton::Left if pressed => {
                        *fire_edge = true;
                        *fire_held = true;
                    }
                    MouseButton::Left => *fire_held = false,
                    MouseButton::Right => *ads_held = pressed,
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Mode::Fly(cam) = &mut self.mode else {
                    return;
                };
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                };
                cam.adjust_speed(scroll);
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;
                let elapsed = now - self.start;
                let time = elapsed.as_secs_f32();
                let local_ms = elapsed.as_secs_f64() * 1000.0;
                let cull = self.cull_mode;
                let Some(r) = &mut self.renderer else { return };
                let aspect = r.aspect();
                // Set inside the spectate arm where `self` is borrowed out
                // field-by-field; acted on once the borrows end.
                let mut fatal: Option<anyhow::Error> = None;
                let (mut frame, vm) = match &mut self.mode {
                    Mode::Fly(cam) => {
                        cam.update(&self.input, dt);
                        let (cam_forward, cam_right, cam_up) = camera::basis(cam.yaw, cam.pitch);
                        // u32::MAX: no entity pinned to the camera.
                        self.audio
                            .set_listener(cam.pos, cam_forward, cam_right, cam_up, u32::MAX);
                        // no CollisionWorld in fly mode, so usePhysics fx don't collide
                        let fx_t0 = Instant::now();
                        self.fx.step(dt, time, None);
                        self.audio.step(&HashMap::new(), None);
                        r.set_fx_quads(
                            &self.fs,
                            self.fx.build_quads(cam.pos, cam_right, cam_up, time),
                        );
                        r.set_fx_lights(&self.fx.lights(cam.pos, time));
                        self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;
                        (
                            renderer::Frame {
                                view_proj: cam.view_proj(aspect),
                                eye: cam.pos,
                                fwd: cam_forward,
                                time,
                                cull,
                                hud_lines: Vec::new(),
                            },
                            None,
                        )
                    }
                    Mode::Spectate {
                        net,
                        cam,
                        input,
                        phase,
                    } => {
                        let events = net.pump();
                        let mut gamestate_ready = false;
                        for ev in &events {
                            if let Some(hud) = &mut self.hud {
                                hud.on_net_event(ev, time);
                            }
                            match ev {
                                net::NetEvent::Chat { text, .. } => {
                                    println!("{}", net::strip_colors(text))
                                }
                                net::NetEvent::Print(s) => println!("{s}"),
                                net::NetEvent::Dropped(why) => {
                                    fatal = Some(anyhow!("disconnected: {why}"))
                                }
                                // Map ambient; each round restart re-sends it
                                // with a new fade deadline, which is ignored.
                                net::NetEvent::ConfigstringChanged(3) => {
                                    self.audio.set_ambient(
                                        &self.fs,
                                        net::info_value_for_key(net.configstring(3), "n"),
                                    );
                                }
                                // `j/k/l` is quick chat; `s <idx>` is the announcer.
                                net::NetEvent::ServerCommand(ref tokens) => {
                                    let newest = net.snapshots().newest();
                                    let protocol = &net::protocol::PROTOCOL_V1;
                                    let quick_chat = self.quick_chat.on_server_command(
                                        &self.fs,
                                        tokens,
                                        |num| {
                                            newest
                                                .and_then(|s| s.clients.get(&num))
                                                .map(|c| c.field_i32(protocol, "team"))
                                        },
                                    );
                                    if !quick_chat {
                                        self.audio.on_server_command(
                                            &self.fs,
                                            tokens,
                                            net.configstrings(),
                                        );
                                    }
                                }
                                net::NetEvent::GamestateReady => gamestate_ready = true,
                                _ => {}
                            }
                        }

                        // One quick-chat line per second, played and shown
                        // like a chat line (retail queues text + alias).
                        if let Some(line) = self.quick_chat.drain(time) {
                            let name = net
                                .snapshots()
                                .newest()
                                .and_then(|s| s.clients.get(&line.client_num))
                                .map(|c| c.name(&net::protocol::PROTOCOL_V1))
                                .unwrap_or_else(|| format!("player {}", line.client_num));
                            println!("{}", net::strip_colors(&format!("{name}: {}", line.text)));
                            if let Some(hud) = &mut self.hud {
                                hud.chat
                                    .push(&format!("{name}: {}", line.text), false, time);
                            }
                            self.audio.play(&self.fs, line.cue);
                        }

                        // Re-send `score` every 2 s while Tab is held, as the
                        // stock client does (cod11-hud-protocol.md, section 4).
                        if let Some(hud) = &mut self.hud {
                            if hud.scoreboard.due(time) {
                                net.send_reliable("score");
                                hud.scoreboard.mark_requested(time);
                            }
                        }

                        let progress = net.download_progress();
                        let frame = match phase {
                            Phase::Connecting { since } => {
                                if gamestate_ready {
                                    match start_loading(
                                        net,
                                        &mut self.hud,
                                        &mut self.audio,
                                        &mut self.fx,
                                        &mut self.world,
                                        r,
                                        self.window.as_deref(),
                                        &mut self.title,
                                        &self.game_dir,
                                        &self.mod_dir,
                                    ) {
                                        Ok(next) => *phase = next,
                                        Err(e) => fatal = Some(e),
                                    }
                                } else if since.elapsed() > Duration::from_secs(10) {
                                    fatal = Some(anyhow!("no gamestate from the server in 10 s"));
                                }
                                loading_frame(
                                    r,
                                    &self.fs,
                                    &mut self.hud,
                                    time,
                                    aspect,
                                    cull,
                                    net.configstrings(),
                                    format!(
                                        "Connecting to {}...",
                                        self.connect_addr.as_deref().unwrap_or("?")
                                    ),
                                )
                            }
                            Phase::Loading { loader } => {
                                let map = loader.map().to_string();
                                let map_resolves = self.fs.resolve_map(&map).is_some();
                                let mut waited = None;
                                // A Ready or Failed loader is never stepped again:
                                // Ready swaps the phase, Failed exits.
                                match loader.step(&events, progress, map_resolves, now) {
                                    loading::Action::BeginDownload { remote, dest } => {
                                        if let Err(e) = net.begin_download(&remote, &dest) {
                                            fatal = Some(e.context("cannot start the download"));
                                        }
                                    }
                                    loading::Action::Reopen => {
                                        match Pk3Fs::open(&self.game_dir.join(&self.mod_dir)) {
                                            Ok(reopened) => self.fs = reopened,
                                            Err(e) => fatal = Some(e),
                                        }
                                    }
                                    loading::Action::FinishDownloads => net.finish_downloads(),
                                    loading::Action::Ready => {
                                        match load_map(
                                            &map,
                                            net,
                                            &mut self.fs,
                                            &mut self.audio,
                                            &mut self.world,
                                            r,
                                            self.window.as_deref(),
                                            &mut self.title,
                                        ) {
                                            Ok(next) => *phase = next,
                                            Err(e) => fatal = Some(e),
                                        }
                                    }
                                    loading::Action::Failed(msg) => fatal = Some(anyhow!(msg)),
                                    loading::Action::Wait(p) => waited = p,
                                }
                                loading_frame(
                                    r,
                                    &self.fs,
                                    &mut self.hud,
                                    time,
                                    aspect,
                                    cull,
                                    net.configstrings(),
                                    loading_text(&map, waited),
                                )
                            }
                            Phase::Live(live) => {
                                if gamestate_ready {
                                    match start_loading(
                                        net,
                                        &mut self.hud,
                                        &mut self.audio,
                                        &mut self.fx,
                                        &mut self.world,
                                        r,
                                        self.window.as_deref(),
                                        &mut self.title,
                                        &self.game_dir,
                                        &self.mod_dir,
                                    ) {
                                        Ok(next) => *phase = next,
                                        Err(e) => fatal = Some(e),
                                    }
                                    let map = match phase {
                                        Phase::Loading { loader } => loader.map().to_string(),
                                        _ => String::new(),
                                    };
                                    loading_frame(
                                        r,
                                        &self.fs,
                                        &mut self.hud,
                                        time,
                                        aspect,
                                        cull,
                                        net.configstrings(),
                                        format!("Loading {map}"),
                                    )
                                } else {
                                    let LivePhase {
                                        world,
                                        scene,
                                        events,
                                        clock,
                                        seeded,
                                        last_loop_snap,
                                    } = &mut **live;
                                    let bsp =
                                        &self.world.as_ref().expect("live phase has a map").bsp;
                                    let p = &net::protocol::PROTOCOL_V1;
                                    let client_num = net.gamestate().map_or(-1, |g| g.client_num);
                                    // While following, ps.clientNum is the followed
                                    // client's (docs/research/cod11-events-and-fx.md,
                                    // section 7); the camera sits inside that body, so
                                    // skip it instead of ours.
                                    let ps_client = net
                                        .snapshots()
                                        .newest()
                                        .map_or(-1, |s| s.ps.field_i32(p, "clientNum"));
                                    let following = ps_client >= 0 && ps_client != client_num;
                                    let skip_num = if following { ps_client } else { client_num };
                                    // Rebuilt every frame so absent entities (PVS churn) drop out.
                                    let mut instances: Vec<DynamicModelInstance> = Vec::new();
                                    // Per-shooter muzzle transforms for flashes and tracers.
                                    let mut muzzles: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
                                    let mut weapon_flash: HashMap<i32, String> = HashMap::new();
                                    let mut entity_pos: HashMap<u32, Vec3> = HashMap::new();

                                    if let Some(newest_time) =
                                        net.snapshots().newest().map(|s| s.server_time)
                                    {
                                        let render_time = clock.render_time(local_ms, newest_time);
                                        if let Some((a, b)) =
                                            net.snapshots().two_for_time(render_time)
                                        {
                                            let oa = Vec3::from(a.ps.origin(p));
                                            let ob = Vec3::from(b.ps.origin(p));
                                            let f = ((render_time - a.server_time) as f32
                                                / (b.server_time - a.server_time).max(1) as f32)
                                                .clamp(0.0, 1.0);
                                            let t0 = Instant::now();
                                            let built = entities::build_instances(
                                                scene,
                                                (a, b, f),
                                                render_time,
                                                skip_num,
                                                net.configstrings(),
                                                &self.fs,
                                                bsp,
                                                r,
                                                p,
                                            );
                                            self.build_ms = t0.elapsed().as_secs_f32() * 1000.0;
                                            instances = built.instances;
                                            muzzles = built.muzzles;
                                            weapon_flash = built.weapon_flash;
                                            entity_pos = built.entity_pos;
                                            // Over 512 u is a teleport, not motion.
                                            let pos = if oa.distance(ob) > 512.0 {
                                                ob
                                            } else {
                                                oa.lerp(ob, f)
                                            };
                                            // viewHeightCurrent: eye offset above the feet origin.
                                            let vh = a.ps.field_f32(p, "viewHeightCurrent")
                                                * (1.0 - f)
                                                + b.ps.field_f32(p, "viewHeightCurrent") * f;
                                            cam.pos = pos + Vec3::Z * vh;
                                            if !*seeded {
                                                let va_a = a.ps.viewangles(p);
                                                let va_b = b.ps.viewangles(p);
                                                cam.yaw = camera::lerp_angle(va_a[1], va_b[1], f)
                                                    .to_radians();
                                                cam.pitch =
                                                    -camera::lerp_angle(va_a[0], va_b[0], f)
                                                        .to_radians();
                                                *seeded = true;
                                            }
                                        } else if let Some(newest) = net.snapshots().newest() {
                                            self.interp_misses += 1;
                                            // too few snapshots to straddle; sit on newest
                                            cam.pos = Vec3::from(newest.ps.origin(p))
                                                + Vec3::Z
                                                    * newest.ps.field_f32(p, "viewHeightCurrent");
                                        }
                                    }

                                    let (cam_forward, cam_right, cam_up) =
                                        camera::basis(cam.yaw, cam.pitch);
                                    // The ridden body is `skip_num`, never in `entity_pos`,
                                    // so the listener is told which entity rides the camera.
                                    self.audio.set_listener(
                                        cam.pos,
                                        cam_forward,
                                        cam_right,
                                        cam_up,
                                        audio::cues::ps_entity(ps_client),
                                    );

                                    let (muzzle_pos, muzzle_dir) =
                                        view_muzzle(cam.pos, cam_forward, cam_right, cam_up);
                                    muzzles.insert(u32::MAX, (muzzle_pos, muzzle_dir));
                                    // While following, the followed player's bullet hits
                                    // carry `other_entity_num == ps_client`, and that body
                                    // is excluded from the muzzle map, so key the view
                                    // muzzle under `ps_client` too or they get no tracer.
                                    if following {
                                        muzzles.insert(ps_client as u32, (muzzle_pos, muzzle_dir));
                                    }

                                    r.set_dynamic_models(&instances);

                                    // Every frame; also the keepalive.
                                    net.send_frame(&usercmd_from_input(input, cam));

                                    // Step before this frame's events spawn, or the
                                    // [now-dt, now] integration would move particles born
                                    // at `now` (a tracer would start ahead of its muzzle).
                                    let fx_t0 = Instant::now();
                                    self.fx.step(dt, time, Some(&*world));

                                    let (screen_w, screen_h) = r.screen_size();

                                    let no_clients = BTreeMap::new();
                                    let newest = net.snapshots().newest();
                                    let hud_frame = hud::HudFrame {
                                        now: time,
                                        screen_w,
                                        screen_h,
                                        configstrings: net.configstrings(),
                                        clients: newest.map_or(&no_clients, |s| &s.clients),
                                        protocol: p,
                                        server_time: newest.map_or(0, |s| s.server_time),
                                        fs: &self.fs,
                                    };

                                    // Events use the newest snapshot, not the interpolation
                                    // pair; they must not wait out the interp delay.
                                    if let Some(newest) = newest {
                                        let ctx = fx::registry::ResolveCtx {
                                            muzzles: &muzzles,
                                            weapon_flash: &weapon_flash,
                                        };
                                        for ev in events.drain(newest, p) {
                                            self.ev_seen += 1;
                                            if let Some(hud) = &mut self.hud {
                                                hud.on_game_event(&ev, &hud_frame);
                                            }
                                            self.audio.on_game_event(
                                                &self.fs,
                                                &ev,
                                                net.configstrings(),
                                                &muzzles,
                                            );
                                            for r in fx::registry::resolve(&ev, &ctx) {
                                                match r {
                                                    fx::registry::Resolved::Spawn { path, at } => {
                                                        let sounds = self
                                                            .fx
                                                            .spawn(&self.fs, &path, at, time);
                                                        self.audio.play_fx(&self.fs, sounds);
                                                    }
                                                    fx::registry::Resolved::Tracer {
                                                        muzzle,
                                                        impact,
                                                    } => {
                                                        self.fx.spawn_tracer(muzzle, impact, time);
                                                    }
                                                    fx::registry::Resolved::Known => {}
                                                    fx::registry::Resolved::Unknown => {
                                                        self.ev_unknown += 1;
                                                        log::debug!(
                                                            "unknown event {} parm {}",
                                                            ev.event,
                                                            ev.parm
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // `es.loopSound` (docs/research/cod11-sound-system.md,
                                    // section 9), reconciled once per snapshot. An entity
                                    // that left the snapshot is absent from the map, which
                                    // is what stops its loop.
                                    if let Some(newest) = newest {
                                        if *last_loop_snap != Some(newest.message_num) {
                                            *last_loop_snap = Some(newest.message_num);
                                            let loops: HashMap<u32, (i32, Vec3)> = newest
                                                .entities
                                                .iter()
                                                .filter_map(|(&num, e)| {
                                                    let idx = e.field_i32(p, "loopSound");
                                                    (idx != 0).then(|| {
                                                        (num, (idx, Vec3::from(e.origin(p))))
                                                    })
                                                })
                                                .collect();
                                            self.audio.set_loop_sounds(
                                                &self.fs,
                                                net.configstrings(),
                                                &loops,
                                            );
                                        }
                                    }

                                    // After the drain, so new voices get this frame's positions.
                                    self.audio.step(&entity_pos, Some(&*world));

                                    r.set_fx_quads(
                                        &self.fs,
                                        self.fx.build_quads(cam.pos, cam_right, cam_up, time),
                                    );
                                    r.set_fx_lights(&self.fx.lights(cam.pos, time));
                                    r.set_fog(
                                        net::FogParams::parse(
                                            net.configstring(net::protocol::CS_FOG_V1),
                                        ),
                                        time,
                                    );
                                    self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;

                                    if let Some(hud) = &mut self.hud {
                                        let hud_t0 = Instant::now();
                                        let quads = hud.build(&hud_frame);
                                        r.set_hud_quads(&self.fs, quads);
                                        self.hud_ms = hud_t0.elapsed().as_secs_f32() * 1000.0;
                                    }

                                    renderer::Frame {
                                        view_proj: cam.view_proj(aspect),
                                        eye: cam.pos,
                                        fwd: cam_forward,
                                        time,
                                        cull,
                                        hud_lines: Vec::new(),
                                    }
                                }
                            }
                        };
                        (frame, None)
                    }
                    Mode::Walk {
                        world,
                        ps,
                        input,
                        keys,
                        motion,
                        view_weapon,
                        configstrings,
                        weapon_slot,
                        reserve,
                        switch_to,
                        mouse_delta,
                        fire_edge,
                        fire_held,
                        reload_edge,
                        ads_held,
                    } => {
                        // Before this frame's fire event spawns, or the
                        // [now-dt, now] integration would move the new tracer
                        // ahead of its muzzle.
                        let fx_t0 = Instant::now();
                        self.fx.step(dt, time, Some(&*world));

                        if let Some(slot) = switch_to.take() {
                            if slot != *weapon_slot && slot < WALK_LOADOUT.len() {
                                let name = WALK_LOADOUT[slot];
                                match load_view_weapon(&self.fs, name) {
                                    Some((models, vw)) => {
                                        r.set_viewmodel(&self.fs, &models);
                                        *reserve = vw.as_ref().map_or(0, |w| w.def.start_ammo);
                                        self.viewmodel = models;
                                        *view_weapon = vw;
                                        *weapon_slot = slot;
                                    }
                                    None => log::warn!(
                                        "weapon {name} failed to load; keeping {}",
                                        WALK_LOADOUT[*weapon_slot]
                                    ),
                                }
                            }
                        }

                        (input.forward, input.right) = keys.axes();
                        for ev in pmove::pmove(ps, input, world, dt) {
                            self.audio.on_game_event(
                                &self.fs,
                                &net::events::GameEvent {
                                    event: ev.event,
                                    parm: 0,
                                    // playerState-ring form: no entity, rides the listener
                                    entity_num: u32::MAX,
                                    client_num: -1,
                                    weapon: 0,
                                    surf_type: 0,
                                    pos: ps.origin.to_array(),
                                    dir: [0.0; 3],
                                    other_entity_num: u32::MAX,
                                    attacker_entity_num: -1,
                                },
                                configstrings,
                                &HashMap::new(),
                            );
                        }
                        input.jump = false; // Q3: a held jump doesn't autohop
                        let ground_speed = if ps.on_ground {
                            ps.velocity.truncate().length()
                        } else {
                            0.0
                        };

                        let v = ps.view();
                        let (eye_forward, eye_right, eye_up) = camera::basis(v.yaw, v.pitch);
                        self.audio
                            .set_listener(v.eye, eye_forward, eye_right, eye_up, u32::MAX);
                        let mut bone_sets = Vec::new();
                        let mut fov = camera::DEFAULT_FOV_DEG;
                        let mut damp = 1.0;
                        if let Some(w) = view_weapon {
                            let out = w.state.update(
                                dt,
                                weapon::WeaponInput {
                                    fire: *fire_edge,
                                    fire_held: *fire_held,
                                    ads: *ads_held,
                                    reload: *reload_edge,
                                },
                            );
                            let clip = w
                                .anims
                                .get(&out.anim)
                                .or_else(|| w.anims.get(&weapon::WeaponAnim::Idle));
                            if let Some((anim, binding)) = clip {
                                let frame = anim.frame_pos(out.anim_time, out.looping);
                                w.pose.apply(anim, binding, frame);
                                // two models, hands then gun, in set_viewmodel order
                                bone_sets = (0..2)
                                    .map(|m| w.pose.skin_matrices(&w.skeleton, m))
                                    .collect();
                            }
                            fov = camera::DEFAULT_FOV_DEG
                                + (w.def.ads_zoom_fov - camera::DEFAULT_FOV_DEG) * out.ads_frac;
                            damp = 1.0 + (w.def.ads_view_bob_mult - 1.0) * out.ads_frac;
                            damp *= 1.0 + (w.def.ads_bob_factor - 1.0) * out.ads_frac;
                            if let Some(cue) = out.cue {
                                if matches!(
                                    cue,
                                    weapon::WeaponCue::Reload | weapon::WeaponCue::ReloadFromEmpty
                                ) {
                                    let need = w.def.clip_size.saturating_sub(w.state.ammo());
                                    *reserve -= need.min(*reserve);
                                }
                                self.audio.on_game_event(
                                    &self.fs,
                                    &net::events::GameEvent {
                                        event: match cue {
                                            weapon::WeaponCue::Fire => fx::registry::EV_FIRE_WEAPON,
                                            weapon::WeaponCue::LastShot => {
                                                fx::registry::EV_FIRE_WEAPON_LASTSHOT
                                            }
                                            weapon::WeaponCue::Rechamber => {
                                                fx::registry::EV_RECHAMBER_WEAPON
                                            }
                                            weapon::WeaponCue::Reload => fx::registry::EV_RELOAD,
                                            weapon::WeaponCue::ReloadFromEmpty => {
                                                fx::registry::EV_RELOAD_FROM_EMPTY
                                            }
                                            weapon::WeaponCue::Raise => {
                                                fx::registry::EV_RAISE_WEAPON
                                            }
                                        },
                                        parm: 0,
                                        entity_num: u32::MAX,
                                        client_num: -1,
                                        // 1-based CS7 index of the active loadout slot
                                        weapon: *weapon_slot as i32 + 1,
                                        surf_type: 0,
                                        pos: ps.origin.to_array(),
                                        dir: [0.0; 3],
                                        other_entity_num: u32::MAX,
                                        attacker_entity_num: -1,
                                    },
                                    configstrings,
                                    &HashMap::new(),
                                );
                            }
                            if out.fired {
                                /// Q3/CoD bullet trace length.
                                const FIRE_RANGE: f32 = 8192.0;
                                // a point trace against solids only: playerclip-only
                                // geometry stops movement but not bullets
                                let tr = world.shot_trace(v.eye, v.eye + eye_forward * FIRE_RANGE);
                                // A muzzle inside solid returns no normal, which
                                // would build a NaN quad that never leaves the ring.
                                if tr.fraction < 1.0 && !tr.startsolid {
                                    // Same csv resolution as a server-driven hit:
                                    // surfType rides the surface flags' bits 20-24.
                                    let mut muzzles = HashMap::new();
                                    muzzles.insert(
                                        u32::MAX,
                                        (v.eye + eye_forward * 16.0 - Vec3::Z * 2.0, eye_forward),
                                    );
                                    let ctx = fx::registry::ResolveCtx {
                                        muzzles: &muzzles,
                                        weapon_flash: &HashMap::new(),
                                    };
                                    let ev = net::events::GameEvent {
                                        event: fx::registry::EV_BULLET_HIT_SMALL,
                                        parm: net::events::dir_to_byte(tr.normal.to_array()),
                                        entity_num: u32::MAX,
                                        client_num: -1,
                                        weapon: *weapon_slot as i32 + 1,
                                        surf_type: collision::sound_material(tr.surface_flags),
                                        pos: tr.endpos.to_array(),
                                        dir: [0.0; 3],
                                        other_entity_num: u32::MAX,
                                        attacker_entity_num: -1,
                                    };
                                    for r in fx::registry::resolve(&ev, &ctx) {
                                        match r {
                                            fx::registry::Resolved::Spawn { path, at } => {
                                                let sounds =
                                                    self.fx.spawn(&self.fs, &path, at, time);
                                                self.audio.play_fx(&self.fs, sounds);
                                            }
                                            fx::registry::Resolved::Tracer { muzzle, impact } => {
                                                self.fx.spawn_tracer(muzzle, impact, time);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        *fire_edge = false;
                        *reload_edge = false;

                        motion.update(
                            dt,
                            ground_speed,
                            ps.on_ground,
                            mouse_delta.0,
                            mouse_delta.1,
                            damp,
                        );
                        *mouse_delta = (0.0, 0.0);
                        self.audio.step(&HashMap::new(), None);
                        r.set_fx_quads(
                            &self.fs,
                            self.fx.build_quads(v.eye, eye_right, eye_up, time),
                        );
                        r.set_fx_lights(&self.fx.lights(v.eye, time));
                        r.set_fog(
                            net::FogParams::parse(
                                configstrings
                                    .get(net::protocol::CS_FOG_V1)
                                    .map(String::as_str)
                                    .unwrap_or(""),
                            ),
                            time,
                        );
                        self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;
                        (
                            renderer::Frame {
                                view_proj: camera::view_proj_from(
                                    v.eye, v.yaw, v.pitch, v.roll, fov, aspect,
                                ),
                                eye: v.eye,
                                fwd: camera::basis(v.yaw, v.pitch).0,
                                time,
                                cull,
                                hud_lines: vec![format!(
                                    "[{}] {} {} / {}",
                                    *weapon_slot + 1,
                                    WALK_LOADOUT[*weapon_slot],
                                    view_weapon.as_ref().map_or(0, |w| w.state.ammo()),
                                    reserve
                                )],
                            },
                            Some(renderer::VmDraw {
                                transform: motion.transform(),
                                bone_sets,
                            }),
                        )
                    }
                };
                if let Some(err) = fatal {
                    self.fail(event_loop, err);
                    return;
                }
                // The scene lives in the spectate phase now, so pull it back
                // out for the overlay after the mode's mutable borrows end.
                let scene = match &self.mode {
                    Mode::Spectate {
                        phase: Phase::Live(live),
                        ..
                    } => Some(&live.scene),
                    _ => None,
                };
                self.hud_stats.frame(
                    dt,
                    scene.map_or(0, |s| s.stats.anim_restarts),
                    self.interp_misses,
                );
                if self.debug_overlay {
                    frame.hud_lines = hud_lines(
                        &self.mode,
                        scene,
                        &self.hud_stats,
                        (self.build_ms, self.render_ms, self.fx_ms),
                        (self.ev_seen, self.ev_unknown),
                        self.fx.counts(),
                        (
                            r.hud_quad_count(),
                            self.hud_ms,
                            self.hud.as_ref().map_or(0, |h| h.unknown),
                        ),
                        self.audio.stats(),
                        r,
                    );
                }
                let t0 = Instant::now();
                r.render(frame, vm);
                self.render_ms = t0.elapsed().as_secs_f32() * 1000.0;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if !self.grabbed {
                return;
            }
            let (dx, dy) = (dx as f32, dy as f32);
            match &mut self.mode {
                Mode::Fly(cam) => cam.mouse_delta(dx, dy),
                Mode::Spectate { cam, .. } => cam.mouse_delta(dx, dy),
                Mode::Walk {
                    ps, mouse_delta, ..
                } => {
                    // same sensitivity and pitch clamp as FlyCamera::mouse_delta
                    const SENS: f32 = 0.003;
                    ps.yaw -= dx * SENS;
                    ps.pitch =
                        (ps.pitch - dy * SENS).clamp(-89.0f32.to_radians(), 89.0f32.to_radians());
                    // raw counts; the sway spring scales them
                    mouse_delta.0 += dx;
                    mouse_delta.1 += dy;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Space and Ctrl reach the server. `up` rides the full usercmd branch,
    /// not the compact one, so it needs the axis set like forward and right.
    #[test]
    fn usercmd_carries_the_up_axis() {
        let cam = FlyCamera::new(Vec3::ZERO, 0.0);

        let rising = usercmd_from_input(
            &InputState {
                up: true,
                ..Default::default()
            },
            &cam,
        );
        assert_eq!(rising.up, 127, "Space must climb");

        let falling = usercmd_from_input(
            &InputState {
                down: true,
                ..Default::default()
            },
            &cam,
        );
        assert_eq!(falling.up, -127, "Ctrl must descend");

        let held = usercmd_from_input(
            &InputState {
                up: true,
                down: true,
                ..Default::default()
            },
            &cam,
        );
        assert_eq!(held.up, 0, "both held cancels");
    }

    /// Drives the real kar98k through the redraw loop's calls without a window.
    /// Reaches idle, fire, rechamber, ADS up and ADS fire; not LastShot,
    /// AdsDown or Reloading.
    #[test]
    fn real_kar98k_animates_through_a_fire_and_ads_cycle() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let (models, view_weapon) = load_view_weapon(&fs, "kar98k_mp").expect("kar98k viewmodel");
        assert_eq!(models.len(), 2);
        let mut w = view_weapon.expect("kar98k anim rig");
        assert!(
            w.anims.contains_key(&weapon::WeaponAnim::Idle),
            "the rig only exists when idle loaded"
        );

        // fire every 40th frame so the bolt cycle completes; ADS for the second half
        let mut poses = Vec::new();
        for step in 0..240 {
            let out = w.state.update(
                1.0 / 60.0,
                weapon::WeaponInput {
                    fire: step > 60 && step % 40 == 0,
                    fire_held: false,
                    ads: step > 120,
                    reload: false,
                },
            );
            let (anim, binding) = w
                .anims
                .get(&out.anim)
                .or_else(|| w.anims.get(&weapon::WeaponAnim::Idle))
                .unwrap_or_else(|| panic!("{:?} has no clip and no idle fallback", out.anim));
            let frame = anim.frame_pos(out.anim_time, out.looping);
            assert!(
                frame.is_finite() && frame >= 0.0 && frame <= (anim.frame_count - 1) as f32,
                "{:?} frame {frame} out of range",
                out.anim
            );
            w.pose.apply(anim, binding, frame);
            for (i, model) in models.iter().enumerate() {
                let mats = w.pose.skin_matrices(&w.skeleton, i);
                assert_eq!(mats.len(), model.bones.len(), "model {i} bone count");
                assert!(mats.iter().all(|m| m.is_finite()), "model {i} step {step}");
            }
            poses.push(w.pose.skin_matrices(&w.skeleton, 1));
        }
        // a static rig would be a silent failure
        assert!(
            poses.iter().any(|p| p != &poses[0]),
            "the gun never moved across 240 frames"
        );
    }

    /// Snapshots arrive at 20 Hz and the window redraws at 60 Hz, so render
    /// time must advance every frame, not per snapshot.
    #[test]
    fn interp_clock_advances_between_snapshots() {
        let mut clock = ServerClock::new();
        let t0 = clock.render_time(0.0, 10_000);
        let t1 = clock.render_time(16.0, 10_000);
        let t2 = clock.render_time(32.0, 10_000);
        let t3 = clock.render_time(48.0, 10_000);
        assert!(
            t0 < t1 && t1 < t2 && t2 < t3,
            "render time froze between snapshots: {t0} {t1} {t2} {t3}"
        );
    }

    #[test]
    fn view_muzzle_offsets_20_forward_4_right_3_down() {
        let cam_pos = Vec3::new(100.0, 200.0, 300.0);
        let forward = Vec3::X;
        let right = Vec3::Y;
        let up = Vec3::Z;
        let (pos, dir) = view_muzzle(cam_pos, forward, right, up);
        assert_eq!(pos, Vec3::new(120.0, 204.0, 297.0));
        assert_eq!(dir, forward);
    }

    /// A snapshot arriving on schedule must not lurch the render time by an
    /// interval.
    #[test]
    fn interp_clock_is_continuous_across_arrival() {
        let mut clock = ServerClock::new();
        clock.render_time(0.0, 10_000);
        let before = clock.render_time(48.0, 10_000);
        let after = clock.render_time(50.0, 10_050);
        assert!(
            (after - before).abs() <= 8,
            "render time jumped across the snapshot seam: {before} -> {after}"
        );
    }

    /// A long gap (tab-out, map change) re-pegs instead of interpolating across it.
    #[test]
    fn interp_clock_repegs_on_large_jump() {
        let mut clock = ServerClock::new();
        clock.render_time(0.0, 10_000);
        clock.render_time(16.0, 10_000);
        let rt = clock.render_time(30_016.0, 40_000);
        assert!(
            (rt - (40_000 - INTERP_DELAY_MS)).abs() <= 4,
            "clock did not re-peg after a large jump: {rt}"
        );
    }
}
