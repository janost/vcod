//! Every stock CoD2 MP map end to end, one at a time: vcod-server loads it
//! on a thread of the test, the client joins as a spectator through
//! [`Cod2Session`] (the `--net-probe` path: snapshots, events, sounds and
//! the map's `_fx` effects), loads the world into a headless renderer and
//! draws a frame from the first spawn, and a body walks from every spawn
//! over the server's collision world. Prints a table and writes each
//! frame, as a PPM, and each map's warnings under `VCOD_CENSUS_OUT`
//! (default `tmp/cod2-map-census` at the workspace root).
//!
//! `COD2_DIR=... cargo test -p vcod --bins -- --ignored cod2_map_census`.
//! The frames need an adapter with BC textures; under lavapipe set
//! `VK_DRIVER_FILES`, `WGPU_BACKEND=vulkan` and `GALLIVM_PERF=nopt`, and
//! run it `nice`d. `VCOD_CENSUS_MAPS=mp_a,mp_b` limits the maps,
//! `VCOD_CENSUS_SECS` sets the spectated seconds (6), `VCOD_CENSUS_FRAMES=0`
//! skips the renderer.

use std::collections::{BTreeMap, HashMap};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::Context;
use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::mapworld::MapWorld;
use vcod_common::pk3::Pk3Fs;

use super::cod2::{Cod2Session, ConnectOpts};
use crate::camera::FlyCamera;
use crate::renderer::{CullMode, Frame, Renderer};

const SERVER_THREAD: &str = "census-server";
const FRAME_SIZE: (u32, u32) = (640, 384);

/// Every warning and error logged, with whether the server's thread
/// logged it.
static LOG: Mutex<Vec<(bool, String)>> = Mutex::new(Vec::new());

struct Collect;

impl log::Log for Collect {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            let server = std::thread::current().name() == Some(SERVER_THREAD);
            let line = format!("{} {}: {}", r.level(), r.target(), r.args());
            LOG.lock().unwrap().push((server, line));
        }
    }
    fn flush(&self) {}
}

/// Whether a warning names something the map asked for and did not get.
fn is_missing(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    [
        "not found",
        "missing",
        "no texture",
        "cannot",
        "can't",
        "failed",
        "no such",
    ]
    .iter()
    .any(|w| l.contains(w))
}

/// One map's row.
#[derive(Default)]
struct Row {
    map: String,
    /// Panics and hard failures, by stage.
    errors: Vec<String>,
    warn_client: usize,
    warn_server: usize,
    missing: usize,
    // world
    materials: usize,
    sky: bool,
    light_grid: bool,
    fog: bool,
    // session
    snapshots: usize,
    most_ents: usize,
    models: usize,
    events: usize,
    effects: usize,
    particles: usize,
    // frame
    frame: Option<FrameStats>,
    validation: usize,
    // vis
    cell: Option<usize>,
    cells: (usize, usize),
    soups: (usize, usize),
    props: (usize, usize),
    // walk
    spawns: usize,
    fell: usize,
    stuck: usize,
    moved: usize,
}

/// A cheap read of the frame: is anything there.
struct FrameStats {
    mean: f32,
    stddev: f32,
    /// Share of near-black pixels.
    black: f32,
    /// Share of the most common colour.
    flat: f32,
    /// Standard deviation of the lower half's luma, where the ground is.
    low_stddev: f32,
}

impl FrameStats {
    fn of(w: u32, h: u32, bgra: &[u8]) -> FrameStats {
        let luma = |p: &[u8; 4]| {
            0.114 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.299 * f32::from(p[2])
        };
        let px: Vec<f32> = bgra.as_chunks::<4>().0.iter().map(luma).collect();
        let stats = |v: &[f32]| {
            let n = v.len().max(1) as f32;
            let mean = v.iter().sum::<f32>() / n;
            let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
            (mean, var.sqrt())
        };
        let (mean, stddev) = stats(&px);
        let (_, low_stddev) = stats(&px[(w * h / 2) as usize..]);
        let mut colours: HashMap<[u8; 3], usize> = HashMap::new();
        for p in bgra.as_chunks::<4>().0 {
            *colours.entry([p[0], p[1], p[2]]).or_default() += 1;
        }
        let n = px.len().max(1) as f32;
        FrameStats {
            mean,
            stddev,
            black: px.iter().filter(|&&l| l < 8.0).count() as f32 / n,
            flat: colours.values().copied().max().unwrap_or(0) as f32 / n,
            low_stddev,
        }
    }

    /// What looks broken: all black, one colour (all sky or the clear),
    /// or nothing on the lower half.
    fn verdict(&self) -> Option<&'static str> {
        if self.black > 0.95 {
            Some("black")
        } else if self.flat > 0.9 || self.stddev < 2.0 {
            Some("flat")
        } else if self.low_stddev < 2.0 {
            Some("empty ground")
        } else {
            None
        }
    }
}

/// vcod-server on `map`, on a thread of its own, until `stop`: the address
/// it listens on, or why it did not load.
fn serve(
    map: &str,
    stop: Arc<AtomicBool>,
) -> (std::thread::JoinHandle<()>, anyhow::Result<SocketAddr>) {
    let (tx, rx) = mpsc::channel();
    let map = map.to_string();
    let handle = std::thread::Builder::new()
        .name(SERVER_THREAD.into())
        .spawn(move || {
            let up = (|| -> anyhow::Result<_> {
                let fs = std::rc::Rc::new(vcod_common::testing::cod2_fs().context("COD2_DIR")?);
                let mut srv = vcod_server::cod2::Cod2Server::new(
                    vcod_server::cod2::Cod2Config {
                        map: map.clone(),
                        gametype: "dm".into(),
                        hostname: "census".into(),
                        max_clients: 4,
                        gametype_script: None,
                    },
                    1,
                    Instant::now(),
                );
                srv.exec_default_cfg(&*fs);
                srv.load(Some(fs))?;
                let sock = UdpSocket::bind("127.0.0.1:0")?;
                sock.set_read_timeout(Some(Duration::from_millis(2)))?;
                Ok((srv, sock))
            })();
            let (mut srv, sock) = match up {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let _ = tx.send(sock.local_addr().map_err(Into::into));
            let start = Instant::now();
            let mut frames = 0u64;
            let mut buf = vec![0u8; 0x10000];
            while !stop.load(Ordering::Relaxed) {
                while let Ok((n, from)) = sock.recv_from(&mut buf) {
                    srv.handle_packet(from, &buf[..n], Instant::now());
                }
                let owed = start.elapsed().as_millis() as u64 / vcod_server::cod2::FRAME_MS as u64
                    - frames;
                if owed > 0 {
                    srv.tick_frames(Instant::now(), owed as u32);
                    frames += owed;
                }
                for (to, pkt) in srv.take_outgoing() {
                    let _ = sock.send_to(&pkt, to);
                }
            }
        })
        .expect("the server thread starts");
    let addr = rx
        .recv_timeout(Duration::from_secs(120))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the server never came up")));
    (handle, addr)
}

/// Spawn origins and yaws of the entity text (`mp_*_spawn`).
fn spawns(entities: &str) -> Vec<(Vec3, f32)> {
    vcod_common::bsp::entity_blocks(entities)
        .into_iter()
        .filter(|b| {
            b.get("classname")
                .is_some_and(|c| c.starts_with("mp_") && c.ends_with("_spawn"))
        })
        .filter_map(|b| {
            let o = vcod_common::bsp::parse_vec3(b.get("origin")?)?;
            let yaw = b
                .get("angles")
                .and_then(|a| vcod_common::bsp::parse_vec3(a))
                .map_or(0.0, |a| a[1]);
            Some((Vec3::from_array(o), yaw))
        })
        .collect()
}

/// A run, a jump, a crouch, prone, a turn and a back-up from every spawn,
/// placed, over `cw` (pmove_walk.rs's script): `(fell, stuck, moved)`.
fn walk(cw: &CollisionWorld, floor: f32, spots: &[(Vec3, f32)]) -> (usize, usize, usize) {
    use vcod_cod2::net::UserCmd2;
    use vcod_cod2::pmove::{self, PlayerState, Rules, button};
    use vcod_common::movetrace::MoveWorld;
    let script: &[(usize, i32, i8, f32)] = &[
        (10, 0, 0, 0.0),
        (20, 0, 127, 0.0),
        (1, button::JUMP, 127, 0.0),
        (15, 0, 127, 0.0),
        (15, button::CROUCH, 127, 0.0),
        (20, button::PRONE, 127, 0.0),
        (20, 0, 127, 90.0),
        (20, 0, -127, 0.0),
        (10, 0, 0, 0.0),
    ];
    let (mut fell, mut stuck, mut moved) = (0, 0, 0);
    for &(spot, yaw) in spots {
        // Where the gametype's `placeSpawnPoint` puts it.
        let (placed, _) = vcod_server::game::builtins::entity::placed_origin(cw, spot.to_array());
        let spot = Vec3::from(placed);
        let mut ps = PlayerState::spawn(spot + Vec3::Z * 0.125, yaw, 1000);
        ps.delta_angles[1] = (yaw * 65536.0 / 360.0) as i32 & 0xffff;
        let mut old = UserCmd2 {
            server_time: 1000,
            ..Default::default()
        };
        let t = MoveWorld::bare(cw).box_trace(
            ps.origin,
            ps.origin,
            ps.mins,
            ps.maxs,
            pmove::MASK_PLAYERSOLID,
        );
        if t.startsolid {
            log::warn!("census walk: the spawn at {spot} starts in solid");
        }
        let mut turn = 0.0f32;
        let mut fallen = false;
        'walk: for &(n, buttons, forward, dyaw) in script {
            turn += dyaw;
            for _ in 0..n {
                let c = UserCmd2 {
                    server_time: ps.command_time + 50,
                    buttons,
                    forward_move: forward,
                    angles: [0, (turn * 65536.0 / 360.0) as i32 & 0xffff, 0],
                    ..Default::default()
                };
                pmove::player_step(
                    &mut ps,
                    &c,
                    &mut old,
                    MoveWorld::bare(cw),
                    Rules::default(),
                    1.0,
                );
                if !ps.origin.is_finite() || ps.origin.z <= floor {
                    log::warn!(
                        "census walk: from {spot} the body fell out of the world at {}",
                        ps.origin
                    );
                    fallen = true;
                    break 'walk;
                }
            }
        }
        if fallen {
            fell += 1;
            continue;
        }
        let (mins, maxs) = ps.stance_box();
        let t = MoveWorld::bare(cw).box_trace(
            ps.origin,
            ps.origin,
            mins,
            maxs,
            pmove::MASK_PLAYERSOLID,
        );
        if t.startsolid {
            log::warn!(
                "census walk: from {spot} the body ended in solid at {}",
                ps.origin
            );
            stuck += 1;
        }
        if (ps.origin - spot).truncate().length() > 50.0 {
            moved += 1;
        }
    }
    (fell, stuck, moved)
}

/// Runs `f`, a panic recorded in `row` as `stage`'s error.
fn stage<T>(row: &mut Row, name: &str, f: impl FnOnce(&mut Row) -> anyhow::Result<T>) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(row))) {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            row.errors.push(format!("{name}: {e:#}"));
            None
        }
        Err(p) => {
            let why = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            row.errors.push(format!("{name} panicked: {why}"));
            None
        }
    }
}

/// The world's own facts and the walk.
fn world_and_walk(row: &mut Row, fs: &Pk3Fs, map: &str) -> anyhow::Result<MapWorld> {
    let data = fs
        .read(&format!("maps/mp/{map}.d3dbsp"))
        .context("no .d3dbsp")?;
    let b = vcod_cod2::d3dbsp::D3dBsp::parse(&data)?;
    let w = vcod_cod2::world::from_bsp(&b);
    row.materials = w.materials.len();
    row.sky = w
        .materials
        .iter()
        .any(|m| m.surface_flags & vcod_cod2::collision::SURF_SKY != 0);
    row.light_grid = w.light_grid.is_some();
    row.fog = fs
        .read(&format!("maps/mp/{map}.gsc"))
        .and_then(|g| vcod_cod2::world::script_fog(&String::from_utf8_lossy(&g)))
        .is_some();
    let cw = vcod_cod2::map::collision_world(&b, &w, &[])?;
    cw.set_arith(vcod_common::patch::Arith::Stored);
    let spots = spawns(&w.entities);
    row.spawns = spots.len();
    (row.fell, row.stuck, row.moved) = walk(&cw, w.models[0].mins[2] - 1.0, &spots);
    Ok(w)
}

/// Joins `addr` as a spectator, loads the world into `r` under the pump,
/// spectates for `secs` and draws a frame from the first spawn.
fn session(
    row: &mut Row,
    fs: &Pk3Fs,
    addr: SocketAddr,
    bsp: &MapWorld,
    r: Option<&mut Renderer>,
    secs: f32,
    out: &Path,
) -> anyhow::Result<()> {
    let s = Cod2Session::connect(
        &addr.to_string(),
        &ConnectOpts::new("census", fs),
        Duration::from_secs(30),
    )?;
    let map = s.map_name().unwrap_or_default().to_string();
    anyhow::ensure!(map == row.map, "the gamestate is for {map:?}");
    let pump = Box::new(s).pump();
    let mut r = r;
    let loaded = match r.as_deref_mut() {
        Some(r) => r.load_world(bsp, fs),
        None => Ok(()),
    };
    let mut s = pump.finish();
    loaded?;
    let (origin, yaw) = vcod_common::bsp::find_spawn(&bsp.entities).context("no spawn")?;
    let eye = Vec3::from(origin) + Vec3::Z * 60.0;
    let mut cam = FlyCamera::new(eye, yaw);
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    let mut fx = crate::fx::sim::FxSystem::new();
    let start = Instant::now();
    let mut last = start;
    while start.elapsed().as_secs_f32() < secs {
        s.fly(&mut cam, fs);
        if let Some(why) = s.dropped.take() {
            anyhow::bail!("dropped: {why}");
        }
        let _ = s.take_map_change();
        let events = s.take_events();
        let _ = s.sound_cues(&events, fs, eye, draw.muzzles());
        let plays = s.effects(&events, fs, eye, draw.fx_scene(true));
        let now = start.elapsed().as_secs_f32();
        fx.set_view(eye);
        for p in &plays {
            p.spawn(&mut fx, fs, now);
        }
        fx.step(last.elapsed().as_secs_f32(), now, None);
        fx.spawn_chained(fs, now);
        last = Instant::now();
        row.events += events.len();
        row.effects += plays.len();
        row.particles = row.particles.max(fx.counts().0);
        if let Some(f) = s.frame() {
            row.most_ents = row.most_ents.max(f.snap.entities.len());
            row.models = row.models.max(draw.count(&f, fs).models);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    row.snapshots = s.stats.snapshots;
    anyhow::ensure!(
        s.stats.parse_errors == 0,
        "{} parse errors",
        s.stats.parse_errors
    );
    anyhow::ensure!(s.stats.snapshots > 0, "no snapshot");

    let vis = vcod_common::vis::WorldVis::build(bsp);
    row.cell = vis.cell_for_point(eye);
    if let Some(r) = r {
        if let Some(f) = s.frame() {
            draw.draw(&f, fs, r);
        }
        let (fwd, right, up) = crate::camera::basis(yaw.to_radians(), 0.0);
        let fov = crate::camera::DEFAULT_FOV_DEG;
        for time in [secs, secs + 0.05] {
            r.set_fx_quads(fs, fx.build_quads(eye, right, up, time), time);
            r.set_fx_lights(&fx.lights(eye, time));
            r.render(
                Frame {
                    view_proj: crate::camera::view_proj_from(
                        eye,
                        yaw.to_radians(),
                        0.0,
                        0.0,
                        fov,
                        r.aspect(),
                    ),
                    eye,
                    fwd,
                    up,
                    time,
                    fov_x: fov,
                    cull: CullMode::On,
                    hud_lines: Vec::new(),
                },
                None,
            );
        }
        let vc = r.vis_counts();
        (row.cells, row.soups, row.props) = (vc.cells, vc.soups, vc.props);
        let (w, h, bgra) = r.read_offscreen().context("no frame")?;
        row.frame = Some(FrameStats::of(w, h, &bgra));
        let rgb: Vec<u8> = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        ppm.extend(rgb);
        std::fs::write(out.join(format!("{}.ppm", row.map)), ppm)?;
        r.unload_world();
    }
    s.disconnect();
    Ok(())
}

fn census_map(
    fs: &Pk3Fs,
    map: &str,
    renderer: &mut Option<(Renderer, crate::renderer::pipeline_check::Errors)>,
    secs: f32,
    out: &Path,
) -> Row {
    LOG.lock().unwrap().clear();
    let mut row = Row {
        map: map.to_string(),
        ..Row::default()
    };
    let t0 = Instant::now();
    let world = stage(&mut row, "world", |row| world_and_walk(row, fs, map));
    if let Some(bsp) = world {
        let stop = Arc::new(AtomicBool::new(false));
        let (server, addr) = serve(map, stop.clone());
        match addr {
            Err(e) => row.errors.push(format!("server: {e:#}")),
            Ok(addr) => {
                let ok = stage(&mut row, "session", |row| {
                    session(
                        row,
                        fs,
                        addr,
                        &bsp,
                        renderer.as_mut().map(|(r, _)| r),
                        secs,
                        out,
                    )
                });
                if ok.is_none()
                    && let Some((r, _)) = renderer.as_mut()
                {
                    r.unload_world();
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        if server.join().is_err() {
            row.errors.push("the server panicked".into());
        }
    }
    if let Some((_, errors)) = renderer.as_ref() {
        let mut e = errors.lock().unwrap();
        row.validation = e.len();
        for v in e.drain(..) {
            LOG.lock().unwrap().push((false, format!("VALIDATION {v}")));
        }
    }
    let log = std::mem::take(&mut *LOG.lock().unwrap());
    row.warn_server = log.iter().filter(|(s, _)| *s).count();
    row.warn_client = log.len() - row.warn_server;
    row.missing = log.iter().filter(|(_, l)| is_missing(l)).count();
    // Each distinct line once, with its count.
    let mut lines: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, l) in &log {
        *lines.entry(l).or_default() += 1;
    }
    let text: String = row
        .errors
        .iter()
        .map(|e| format!("ERROR {e}\n"))
        .chain(lines.iter().map(|(l, n)| format!("{n:>4} {l}\n")))
        .collect();
    let _ = std::fs::write(out.join(format!("{map}.log")), text);
    eprintln!("census: {map} in {:.1} s", t0.elapsed().as_secs_f32());
    row
}

fn table(rows: &[Row]) -> String {
    let mut t = String::from(
        "map            | warn c/s miss | mats sky grid fog | snaps ents models events fx parts | \
         frame (mean sd black flat) | valid | cell cells soups props | spawns fell stuck moved | errors\n",
    );
    let pair = |(a, b): (usize, usize)| format!("{a}/{b}");
    let yn = |b: bool| if b { "y" } else { "n" };
    for r in rows {
        let frame = match &r.frame {
            Some(f) => format!(
                "{:.0} {:.0} {:.2} {:.2}{}",
                f.mean,
                f.stddev,
                f.black,
                f.flat,
                f.verdict().map(|v| format!(" {v}")).unwrap_or_default()
            ),
            None => "-".into(),
        };
        t += &format!(
            "{:<14} | {}/{} {} | {} {} {} {} | {} {} {} {} {} {} | {} | {} | {} {} {} {} | {} {} {} {} | {}\n",
            r.map,
            r.warn_client,
            r.warn_server,
            r.missing,
            r.materials,
            yn(r.sky),
            yn(r.light_grid),
            yn(r.fog),
            r.snapshots,
            r.most_ents,
            r.models,
            r.events,
            r.effects,
            r.particles,
            frame,
            r.validation,
            r.cell.map_or("-".into(), |c| c.to_string()),
            pair(r.cells),
            pair(r.soups),
            pair(r.props),
            r.spawns,
            r.fell,
            r.stuck,
            r.moved,
            r.errors.join("; "),
        );
    }
    t
}

#[test]
#[ignore = "every stock map through server, client, renderer and pmove; minutes"]
fn cod2_map_census() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        eprintln!("no COD2_DIR; skipped");
        return;
    };
    if log::set_boxed_logger(Box::new(Collect)).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
    vcod_cod2::model::register();
    let out = std::env::var_os("VCOD_CENSUS_OUT").map_or_else(
        || {
            PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tmp/cod2-map-census"
            ))
        },
        PathBuf::from,
    );
    std::fs::create_dir_all(&out).unwrap();
    let only: Option<Vec<String>> = std::env::var("VCOD_CENSUS_MAPS")
        .ok()
        .map(|v| v.split(',').map(|m| m.trim().to_string()).collect());
    let secs = std::env::var("VCOD_CENSUS_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6.0);
    let maps: Vec<String> = fs
        .list_prefix("maps/mp/")
        .into_iter()
        .filter_map(|p| {
            p.strip_prefix("maps/mp/")?
                .strip_suffix(".d3dbsp")
                .map(str::to_string)
        })
        .filter(|m| !m.contains('/'))
        .filter(|m| only.as_ref().is_none_or(|o| o.contains(m)))
        .collect();
    assert!(!maps.is_empty(), "no maps/mp/*.d3dbsp");
    let mut renderer = if std::env::var("VCOD_CENSUS_FRAMES").is_ok_and(|v| v == "0") {
        None
    } else {
        Renderer::headless_sized(&fs, false, 1, FRAME_SIZE)
    };
    // The window's ramp: gamma 1, windowed overbright (`main.rs`'s frame).
    if let Some((r, _)) = renderer.as_mut() {
        let bits = crate::gamma::overbright_bits(fs.game(), false, true);
        r.set_gamma(1.0, bits);
    }
    if renderer.is_none() {
        eprintln!("census: no renderer (no adapter with BC textures, or VCOD_CENSUS_FRAMES=0)");
    }
    let rows: Vec<Row> = maps
        .iter()
        .map(|m| census_map(&fs, m, &mut renderer, secs, &out))
        .collect();
    let t = table(&rows);
    println!("{t}");
    std::fs::write(out.join("census.txt"), &t).unwrap();
    let bad: Vec<&str> = rows
        .iter()
        .filter(|r| {
            !r.errors.is_empty()
                || r.validation > 0
                || r.fell > 0
                || r.frame.as_ref().is_some_and(|f| f.verdict().is_some())
        })
        .map(|r| r.map.as_str())
        .collect();
    assert!(bad.is_empty(), "maps that failed: {bad:?}\n{t}");
}
