//! `screenshot`, `screenshotJPEG` and `cl_avidemo`'s shot a frame, as each
//! game's renderer writes them: the file's name and folder, the TGA header
//! and row order, the JPEG quality, the gamma and the console line
//! (cod11-demos.md 9, cod2-demos.md 8, cod4-demos.md 10). The frame comes
//! from `Renderer::request_capture` and `take_capture`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use vcod_common::game::Game;

use crate::renderer::Capture;

/// A file's format: `screenshot` writes TGA, `screenshotJPEG` JPEG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Tga,
    Jpeg,
}

impl Format {
    fn ext(self) -> &'static str {
        match self {
            Format::Tga => "tga",
            Format::Jpeg => "jpg",
        }
    }
}

/// A screenshot command, its arguments read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub format: Format,
    /// The file's name without folder or extension; `None` takes the next
    /// free `shotNNNN`.
    pub name: Option<String>,
    /// No `Wrote` line (`screenshot silent`, `cl_avidemo`).
    pub silent: bool,
}

/// What a game's writer does.
#[derive(Clone, Copy, Debug)]
struct Rules {
    /// The TGA's rows top-down (header byte 17 `0x20`), else bottom-up (0).
    tga_top_down: bool,
    jpeg_quality: u8,
    /// The highest `shotNNNN` a format writes: `[tga, jpg]`.
    last_shot: [u32; 2],
    /// One counter for both formats, else one each.
    one_counter: bool,
}

fn rules(game: Game) -> Rules {
    match game {
        // CoDMP.exe 0x4b25a0 / 0x4b2770 (cod11-demos.md 9.2, 9.3)
        Game::Cod1 => Rules {
            tga_top_down: false,
            jpeg_quality: 95,
            last_shot: [9998, 9999],
            one_counter: false,
        },
        // gfx_d3d_mp_x86_s.dll 0x10026790, iw3mp.exe 0x60ebf0
        // (cod2-demos.md 8, cod4-demos.md 10)
        Game::Cod2 | Game::Cod4 => Rules {
            tga_top_down: true,
            jpeg_quality: 90,
            last_shot: [9998, 9998],
            one_counter: true,
        },
    }
}

/// Reads `screenshot`'s or `screenshotJPEG`'s arguments (`args` past the
/// word), alike in all three engines: `silent` as the first, else a lone
/// argument names the file. `None` for `levelshot` and `savegame`, which
/// vcod does not write.
pub fn parse(jpeg: bool, args: &[String]) -> Option<Request> {
    let first = args.first().map(String::as_str);
    if matches!(first, Some("levelshot" | "savegame")) {
        return None;
    }
    let silent = first == Some("silent");
    Some(Request {
        format: if jpeg { Format::Jpeg } else { Format::Tga },
        name: first
            .filter(|_| args.len() == 1 && !silent)
            .map(str::to_string),
        silent,
    })
}

/// Whether a shot reads the framebuffer before the gamma ramp. CoD1 reads
/// the back buffer and maps it through the gamma table only with
/// overbright bits (full screen, device gamma); CoD2 and CoD4 read D3D's
/// front buffer, which the device ramp never touches.
pub fn pre_gamma(game: Game, overbright: u32) -> bool {
    game != Game::Cod1 || overbright == 0
}

/// The writer's counters across shots: each game counts on from its last
/// number and never looks below it again.
#[derive(Debug, Default)]
pub struct Shooter {
    next: [u32; 2],
}

impl Shooter {
    /// Writes `cap` as `req` asks into `dir` (the game folder's
    /// `screenshots/`). Returns the console lines.
    pub fn save(&mut self, game: Game, dir: &Path, req: &Request, cap: &Capture) -> Vec<String> {
        let rules = rules(game);
        let path = match &req.name {
            Some(name) => dir.join(format!("{name}.{}", req.format.ext())),
            None => match self.next_free(dir, req.format, rules) {
                Some(p) => p,
                None => return vec!["ScreenShot: Couldn't create a file".into()],
            },
        };
        let bytes = match req.format {
            Format::Tga => tga(cap, rules.tga_top_down),
            Format::Jpeg => jpeg(cap, rules.jpeg_quality),
        };
        let mut lines = Vec::new();
        if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, bytes)) {
            log::warn!("{}: {e}", path.display());
            lines.push(format!("Failed to open {}", shown(&path)));
        }
        // printed whether the write took or not, as retail's
        if !req.silent {
            lines.push(format!("Wrote {}", shown(&path)));
        }
        lines
    }

    /// The first `shotNNNN` from the counter on that no file holds.
    fn next_free(&mut self, dir: &Path, format: Format, rules: Rules) -> Option<PathBuf> {
        let slot = if rules.one_counter {
            0
        } else {
            format as usize
        };
        let last = rules.last_shot[format as usize];
        let next = &mut self.next[slot];
        while *next <= last {
            let path = dir.join(format!("shot{:04}.{}", *next, format.ext()));
            *next += 1;
            if !path.exists() {
                return Some(path);
            }
        }
        None
    }
}

/// `screenshots/<file>`, as the game names it in its line.
fn shown(path: &Path) -> String {
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy())
        .unwrap_or_default();
    format!("screenshots/{file}")
}

/// An uncompressed 24-bit TGA: the 18-byte header (type 2, the size, 24
/// bits, the origin bit), then B, G, R a pixel.
pub fn tga(cap: &Capture, top_down: bool) -> Vec<u8> {
    let (w, h) = (cap.width as usize, cap.height as usize);
    let mut out = vec![0u8; 18];
    out[2] = 2;
    out[12..14].copy_from_slice(&(cap.width as u16).to_le_bytes());
    out[14..16].copy_from_slice(&(cap.height as u16).to_le_bytes());
    out[16] = 24;
    out[17] = if top_down { 0x20 } else { 0 };
    out.reserve(w * h * 3);
    for y in 0..h {
        let row = if top_down { y } else { h - 1 - y };
        for px in cap.rgb[row * w * 3..][..w * 3].as_chunks::<3>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    out
}

/// A baseline JPEG at `quality`, rows top-down.
pub fn jpeg(cap: &Capture, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    if let Err(e) = enc.encode(
        &cap.rgb,
        cap.width,
        cap.height,
        image::ExtendedColorType::Rgb8,
    ) {
        log::warn!("screenshot: {e}");
    }
    out
}

/// One frame of `cl_avidemo`: the client clock's step, and the silent shot
/// when the client is in a game or `cl_forceavidemo` is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AviFrame {
    pub step: Duration,
    pub shot: Option<Request>,
}

/// `cl_avidemo` `frames` a second at `timescale`: `CL_Frame`'s override
/// (CoDMP.exe 0x4112f1, `CL_RunOncePerClientFrame` 0x40f770 in
/// CoD2MP_s.exe, iw3mp.exe 0x46c8d0), `trunc(1000 / frames * timescale)`
/// ms and at least 1. UO takes the count's magnitude and writes JPEGs for a
/// negative one; the others write TGAs. `None` at 0.
pub fn avidemo(
    game: Game,
    uo: bool,
    frames: i32,
    timescale: f32,
    active: bool,
    force: bool,
) -> Option<AviFrame> {
    if frames == 0 {
        return None;
    }
    let ms = match game {
        // a double divide on 1.7
        Game::Cod4 => (1000.0 / f64::from(frames) * f64::from(timescale)) as i64,
        _ if uo => (1000.0 / (frames as f32).abs() * timescale) as i64,
        _ => (1000.0 / frames as f32 * timescale) as i64,
    };
    // 1.1's and 1.5's negative step runs their clock back; vcod holds 1 ms
    let step = Duration::from_millis(ms.max(1) as u64);
    let jpeg = uo && frames < 0;
    let shot = (active || force).then_some(Request {
        format: if jpeg { Format::Jpeg } else { Format::Tga },
        name: None,
        silent: true,
    });
    Some(AviFrame { step, shot })
}

/// What 1.5's and UO's `CG_ServerCommand` runs for the server's `E`
/// (`cgame_mp_x86.dll` 1.5 0x3002fa61) while `cg_autoscreenshot` is 1:
/// `screenshotJPEG <yyyyMMdd-HHmmss>-<name>`, the name kept to letters and
/// digits. 1.1's cgame has no `E`, CoD2 and CoD4 no `cg_autoscreenshot`.
pub fn auto_command(
    tokens: &[String],
    v1: bool,
    auto: &str,
    name: &str,
    stamp: &str,
) -> Option<String> {
    if v1 || vcod_common::text::atoi_msvc(auto) != 1 || tokens.first()? != "E" {
        return None;
    }
    let clean: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
    Some(format!("screenshotJPEG {stamp}-{clean}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn cap(w: u32, h: u32) -> Capture {
        // red on the top row, blue below
        let rgb = (0..h)
            .flat_map(|y| (0..w).flat_map(move |_| if y == 0 { [255, 0, 0] } else { [0, 0, 255] }))
            .collect();
        Capture {
            width: w,
            height: h,
            rgb,
        }
    }

    #[test]
    fn arguments_read_alike_in_every_engine() {
        let p = |jpeg, s| parse(jpeg, &args(s));
        let req = |format, name: Option<&str>, silent| Request {
            format,
            name: name.map(str::to_string),
            silent,
        };
        assert_eq!(p(false, ""), Some(req(Format::Tga, None, false)));
        assert_eq!(p(true, "silent"), Some(req(Format::Jpeg, None, true)));
        assert_eq!(p(false, "foo"), Some(req(Format::Tga, Some("foo"), false)));
        // two arguments: numbered; `silent` only first, case and all
        assert_eq!(p(false, "foo bar"), Some(req(Format::Tga, None, false)));
        assert_eq!(p(false, "silent x"), Some(req(Format::Tga, None, true)));
        assert_eq!(
            p(false, "Silent"),
            Some(req(Format::Tga, Some("Silent"), false))
        );
        assert_eq!(p(false, "levelshot"), None);
    }

    #[test]
    fn a_tga_is_bottom_up_on_cod1_and_top_down_after() {
        let c = cap(2, 2);
        let old = tga(&c, false);
        assert_eq!(
            &old[..18],
            &[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 2, 0, 24, 0]
        );
        // bottom row first, B G R
        assert_eq!(&old[18..21], &[255, 0, 0]);
        assert_eq!(&old[24..27], &[0, 0, 255]);
        let new = tga(&c, true);
        assert_eq!(new[17], 0x20);
        assert_eq!(&new[18..21], &[0, 0, 255]);
        assert_eq!(new.len(), 18 + 2 * 2 * 3);
    }

    #[test]
    fn numbering_skips_taken_files_and_stops_at_the_last() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shot0000.tga"), b"x").unwrap();
        let c = cap(4, 4);
        let tga = Request {
            format: Format::Tga,
            name: None,
            silent: false,
        };
        let jpg = Request {
            format: Format::Jpeg,
            ..tga.clone()
        };
        let mut s = Shooter::default();
        assert_eq!(
            s.save(Game::Cod1, dir.path(), &tga, &c),
            ["Wrote screenshots/shot0001.tga"]
        );
        // CoD1 counts each format on its own
        assert_eq!(
            s.save(Game::Cod1, dir.path(), &jpg, &c),
            ["Wrote screenshots/shot0000.jpg"]
        );
        let mut s = Shooter::default();
        s.save(Game::Cod2, dir.path(), &tga, &c);
        assert_eq!(
            s.save(Game::Cod2, dir.path(), &jpg, &c),
            ["Wrote screenshots/shot0003.jpg"]
        );
        let mut s = Shooter { next: [9998, 9999] };
        assert_eq!(
            s.save(Game::Cod1, dir.path(), &tga, &c),
            ["Wrote screenshots/shot9998.tga"]
        );
        assert_eq!(
            s.save(Game::Cod1, dir.path(), &tga, &c),
            ["ScreenShot: Couldn't create a file"]
        );
        assert_eq!(
            s.save(Game::Cod1, dir.path(), &jpg, &c),
            ["Wrote screenshots/shot9999.jpg"]
        );
        let named = Request {
            name: Some("mine".into()),
            silent: false,
            ..jpg
        };
        assert_eq!(
            s.save(Game::Cod4, dir.path(), &named, &c),
            ["Wrote screenshots/mine.jpg"]
        );
        let bytes = std::fs::read(dir.path().join("mine.jpg")).unwrap();
        assert_eq!(&bytes[..3], &[0xff, 0xd8, 0xff]);
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (4, 4));
    }

    #[test]
    fn avidemo_steps_by_the_truncated_rate() {
        let ms =
            |game, uo, n, ts| avidemo(game, uo, n, ts, true, false).map(|f| f.step.as_millis());
        assert_eq!(ms(Game::Cod1, false, 30, 1.0), Some(33));
        assert_eq!(ms(Game::Cod1, false, 0, 1.0), None);
        assert_eq!(ms(Game::Cod2, false, 3000, 1.0), Some(1));
        assert_eq!(ms(Game::Cod4, false, 25, 0.5), Some(20));
        assert_eq!(ms(Game::Cod1, true, -30, 1.0), Some(33));
        let shot =
            |uo, n, active, force| avidemo(Game::Cod1, uo, n, 1.0, active, force).unwrap().shot;
        assert_eq!(shot(true, -30, true, false).unwrap().format, Format::Jpeg);
        assert_eq!(shot(false, -30, true, false).unwrap().format, Format::Tga);
        assert!(shot(false, 30, true, false).unwrap().silent);
        assert_eq!(shot(false, 30, false, false), None);
        assert!(shot(false, 30, false, true).is_some());
    }

    #[test]
    fn the_server_e_shoots_under_cg_autoscreenshot_1() {
        let e = |v1, auto| auto_command(&args("E"), v1, auto, "^1Big Joe!", "20261112-213045");
        assert_eq!(
            e(false, "1").as_deref(),
            Some("screenshotJPEG 20261112-213045-1BigJoe")
        );
        assert_eq!(e(false, "0"), None);
        assert_eq!(e(false, "2"), None);
        assert_eq!(e(true, "1"), None);
    }
}
