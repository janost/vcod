//! `screenshot`, `screenshotJPEG` and `cl_avidemo` through the real
//! renderer, a game each: the console line's effect read as `main` reads
//! it, the frame drawn headless with the capture asked for, and the file
//! written as `main` writes it, its header and size checked. CoD1 draws
//! mp_carentan as `main` loads it, CoD2 plays the retail mp_toujane demo
//! under `cl_avidemo` on the frame clock, CoD4 draws mp_crash as a joined
//! session hands it over. Each needs its install (`COD_DIR`, `COD2_DIR`,
//! `COD4_DIR`) and an adapter with BC textures (lavapipe will do), and
//! passes vacuously without them.

use std::path::Path;
use std::time::{Duration, Instant};

use glam::Vec3;
use vcod_common::game::Game;

use super::*;
use crate::console::shell::{Effect, Shell};
use crate::screenshot::{self, Format, Request, Shooter};

const SIZE: (u32, u32) = (320, 192);

/// `line` through the console as `main` applies it: the request it queues.
fn command(line: &str) -> Request {
    let effects = Shell::new().execute(line);
    let Some(Effect::Screenshot { jpeg, args }) = effects.into_iter().next() else {
        panic!("{line}: no screenshot effect");
    };
    match screenshot::parse(jpeg, &args) {
        screenshot::Command::Screen(req) => req,
        other => panic!("{line}: {other:?}"),
    }
}

/// One frame from `eye` along `yaw` and `pitch` (radians, pitch up).
fn draw(r: &mut Renderer, eye: Vec3, yaw: f32, pitch: f32, time: f32) {
    let fov = 80.0;
    let (fwd, _, up) = crate::camera::basis(yaw, pitch);
    r.render(
        Frame {
            view_proj: crate::camera::view_proj_from(eye, yaw, pitch, 0.0, fov, r.aspect()),
            eye,
            fwd,
            up,
            time,
            fov_x: fov,
            cull: CullMode::On,
            hud_lines: vec!["screenshot".into()],
        },
        None,
    );
}

/// The file's header and size: an uncompressed 24-bit TGA of the frame
/// with `origin` as its descriptor, or a JPEG of the frame's size.
fn check_file(path: &Path, format: Format, origin: u8) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (w, h) = SIZE;
    match format {
        Format::Tga => {
            let mut head = [0u8; 18];
            head[2] = 2;
            head[12..14].copy_from_slice(&(w as u16).to_le_bytes());
            head[14..16].copy_from_slice(&(h as u16).to_le_bytes());
            head[16] = 24;
            head[17] = origin;
            assert_eq!(bytes[..18], head, "{}", path.display());
            assert_eq!(bytes.len(), 18 + (w * h * 3) as usize);
        }
        Format::Jpeg => {
            assert_eq!(bytes[..3], [0xff, 0xd8, 0xff]);
            let img = image::load_from_memory(&bytes).unwrap();
            assert_eq!((img.width(), img.height()), SIZE);
        }
    }
    bytes
}

/// The mean of a capture's bytes.
fn mean(b: &[u8]) -> f32 {
    b.iter().map(|&x| f32::from(x)).sum::<f32>() / b.len() as f32
}

/// The mean of the offscreen target's colour bytes (BGRA, alpha left out).
fn screen_mean(r: &Renderer) -> f32 {
    let (_, _, bgra) = r.read_offscreen().unwrap();
    let rgb: Vec<u8> = bgra.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    mean(&rgb)
}

#[test]
fn cod1_screenshot_writes_a_bottom_up_tga_before_the_windowed_ramp() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless_sized(&fs, false, 1, SIZE) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    let data = fs.read("maps/mp/mp_carentan.bsp").expect("mp_carentan");
    let bsp = vcod_common::mapworld::load(&data).unwrap();
    r.load_world(&bsp, &fs).unwrap();
    let (o, yaw) = vcod_common::bsp::find_spawn(&bsp.entities).expect("a spawn");
    let eye = Vec3::from(o) + Vec3::Z * 60.0;
    // windowed with device gamma: no overbright bits, `r_gamma` 1.5
    let overbright = crate::gamma::overbright_bits(Game::Cod1, false, true);
    r.set_gamma(1.5, overbright);
    let dir = tempfile::tempdir().unwrap();
    let mut shooter = Shooter::default();
    for line in ["screenshot", "screenshotJPEG"] {
        let req = command(line);
        r.request_capture(screenshot::pre_gamma(Game::Cod1, overbright));
        draw(&mut r, eye, yaw.to_radians(), 0.0, 1.0);
        let cap = r.take_capture().expect("drawn").expect("read back");
        let lines = shooter.save(Game::Cod1, dir.path(), &req, &cap);
        let name = format!(
            "shot0000.{}",
            if req.format == Format::Tga {
                "tga"
            } else {
                "jpg"
            }
        );
        assert_eq!(lines, [format!("Wrote screenshots/{name}")]);
        let bytes = check_file(&dir.path().join(&name), req.format, 0);
        if req.format == Format::Tga {
            // bottom row first: the capture's last row, B G R
            let (w, h) = (cap.width as usize, cap.height as usize);
            let last = &cap.rgb[(h - 1) * w * 3..][..3];
            assert_eq!(bytes[18..21], [last[2], last[1], last[0]]);
            // retail's pass skips the table windowed: darker than the screen
            let screen = screen_mean(&r);
            assert!(
                mean(&cap.rgb) + 4.0 < screen,
                "{} against {screen}",
                mean(&cap.rgb)
            );
        }
    }
    assert!(errors.lock().unwrap().is_empty());
}

#[test]
fn cod2_avidemo_writes_a_silent_tga_a_frame_on_the_demo_clock() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((mut r, errors)) = Renderer::headless_sized(&fs, false, 1, SIZE) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    vcod_cod2::model::register();
    let world = fs.read("maps/mp/mp_toujane.d3dbsp").expect("mp_toujane");
    r.load_world(&vcod_cod2::world::from_bytes(&world).unwrap(), &fs)
        .unwrap();
    r.set_gamma(1.0, 0);
    let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../cod2/tests/fixtures/demos/mp_toujane-retail.dm_1");
    let mut s =
        crate::play::cod2::Cod2Session::play_demo(std::fs::read(demo).unwrap(), false).unwrap();
    let mut cam = crate::camera::FlyCamera::new(Vec3::ZERO, 0.0);
    let mut scene = crate::cod2_scene::Cod2Draw::new();
    let dir = tempfile::tempdir().unwrap();
    let mut shooter = Shooter::default();
    let mut times = Vec::new();
    let mut shots = 0;
    for _ in 0..12 {
        // `cl_avidemo 20`: 50 ms a frame however long the frame takes
        let active = s.frame().is_some();
        let avi = screenshot::avidemo(Game::Cod2, false, 20, 1.0, active, false).unwrap();
        crate::frame_clock::tick(Instant::now(), Some(avi.step));
        std::thread::sleep(Duration::from_millis(5));
        s.fly(&mut cam, &fs);
        if let Some(f) = s.frame() {
            times.push(f.time_ms);
            scene.draw(&f, &fs, &mut r);
        }
        if avi.shot.is_some() {
            r.request_capture(screenshot::pre_gamma(Game::Cod2, 0));
        }
        draw(&mut r, cam.pos, cam.yaw, cam.pitch, 1.0);
        if let (Some(req), Some(cap)) = (avi.shot, r.take_capture()) {
            let lines = shooter.save(Game::Cod2, dir.path(), &req, &cap.expect("read back"));
            assert!(lines.is_empty(), "silent: {lines:?}");
            let path = dir.path().join(format!("shot{shots:04}.tga"));
            let bytes = check_file(&path, Format::Tga, 0x20);
            assert!(bytes[18..].iter().any(|&b| b > 8), "a lit frame");
            shots += 1;
        }
    }
    assert!(errors.lock().unwrap().is_empty());
    assert!(shots >= 8, "{shots} shots");
    // the demo's time moves the step a frame, the wall's 5+ ms aside
    let steps: Vec<i32> = times.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        steps.len() >= 8 && steps.iter().all(|&d| d == 50),
        "{steps:?}"
    );
}

#[test]
fn cod4_screenshot_writes_a_top_down_tga_and_a_named_jpeg() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let _slot = pipeline_check::live_session_slot();
    let root = install.root().to_path_buf();
    let fs = crate::cod4_fly::open_fs(&root, None, None).unwrap();
    let Some((mut r, errors)) = Renderer::headless_sized(&fs, false, 1, SIZE) else {
        eprintln!("no adapter with BC textures; skipped");
        return;
    };
    let ents = crate::cod4_fly::EntityAssets {
        models: Vec::new(),
        weapons: Vec::new(),
    };
    let map = crate::cod4_fly::load(&root, None, "mp_crash", &ents).unwrap();
    r.set_material_lib(map.materials.clone());
    r.load_world(&map.bsp, &fs).unwrap();
    r.set_level_look(Some(map.look.at(1000, false)));
    // `r_gamma` 1.4: the front buffer 1.7 reads holds no ramp
    r.set_gamma(1.4, 0);
    // mp_crash's sunny street (cod4_live.rs)
    let eye = Vec3::new(246.0, -224.0, 192.0);
    let dir = tempfile::tempdir().unwrap();
    let mut shooter = Shooter::default();
    for (line, file) in [
        ("screenshot", "shot0000.tga"),
        ("screenshotJpeg crash", "crash.jpg"),
    ] {
        let req = command(line);
        r.request_capture(screenshot::pre_gamma(Game::Cod4, 0));
        draw(&mut r, eye, 57f32.to_radians(), 0.0, 1.0);
        let cap = r.take_capture().expect("drawn").expect("read back");
        let lines = shooter.save(Game::Cod4, dir.path(), &req, &cap);
        assert_eq!(lines, [format!("Wrote screenshots/{file}")]);
        let bytes = check_file(&dir.path().join(file), req.format, 0x20);
        if req.format == Format::Tga {
            // top row first, B G R
            assert_eq!(bytes[18..21], [cap.rgb[2], cap.rgb[1], cap.rgb[0]]);
            let screen = screen_mean(&r);
            assert!(
                mean(&cap.rgb) + 4.0 < screen,
                "{} against {screen}",
                mean(&cap.rgb)
            );
        }
    }
    assert!(errors.lock().unwrap().is_empty());
}
