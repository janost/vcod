//! `screenshot`, `screenshotJPEG` and `cl_avidemo`'s shot a frame, as each
//! game's renderer writes them: the file's name and folder, the TGA header
//! and row order, the JPEG quality, the gamma and the console line, and the
//! `levelshot` and `savegame` pictures' filters (cod11-demos.md 10,
//! cod2-demos.md 8, cod4-demos.md 10). The frame comes
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
        // CoDMP.exe 0x4b25a0 / 0x4b2770 (cod11-demos.md 10.2, 10.3)
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

/// What a screenshot command writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A shot into `screenshots/`.
    Screen(Request),
    /// `levelshot`: a 128 x 128 `levelshots/<map>.tga`, TGA from either
    /// command.
    Level,
    /// `savegame <name>`: a 512 x 512 `<name>.jpg` at quality 90.
    Savegame(String),
}

/// Reads `screenshot`'s or `screenshotJPEG`'s arguments (`args` past the
/// word), alike in all three engines (cod11-demos.md 10.1): `levelshot`
/// first, `savegame` with exactly one non-empty name after it, then
/// `silent` as the first, else a lone argument names the file.
pub fn parse(jpeg: bool, args: &[String]) -> Command {
    let first = args.first().map(String::as_str);
    if first == Some("levelshot") {
        return Command::Level;
    }
    if first == Some("savegame") && args.len() == 2 && !args[1].is_empty() {
        return Command::Savegame(args[1].clone());
    }
    let silent = first == Some("silent");
    Command::Screen(Request {
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
    /// Runs `cmd` on `cap`: a shot into `gamedir`'s `screenshots/`, a
    /// levelshot of `map` (the world's base name; nothing with no world,
    /// where retail reads a null one) or a save game's picture, all under
    /// `gamedir` (`fs_homepath/<fs_gamedir>`). Returns the console lines.
    pub fn run(
        &mut self,
        game: Game,
        gamedir: &Path,
        map: Option<&str>,
        cmd: &Command,
        cap: &Capture,
    ) -> Vec<String> {
        let (path, bytes) = match cmd {
            Command::Screen(req) => return self.save(game, &gamedir.join("screenshots"), req, cap),
            Command::Level => {
                let Some(map) = map else {
                    return Vec::new();
                };
                (format!("levelshots/{map}.tga"), levelshot(game, cap))
            }
            Command::Savegame(name) => (format!("{name}.jpg"), savegame(game, cap)),
        };
        let full = gamedir.join(&path);
        let mut lines = Vec::new();
        let written = full
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&full, bytes));
        if let Err(e) = written {
            log::warn!("{}: {e}", full.display());
            lines.push(format!("Failed to open {path}"));
        }
        // the levelshot's line follows whatever the write did; the save
        // game's picture prints none
        if *cmd == Command::Level {
            lines.push(format!("Wrote {path}"));
        }
        lines
    }

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
    tga_rows(cap, top_down, if top_down { 0x20 } else { 0 })
}

/// [`tga`] with the rows in `top_down` order and `origin` as header byte
/// 17, which a levelshot leaves 0 whatever order its rows run.
fn tga_rows(cap: &Capture, top_down: bool, origin: u8) -> Vec<u8> {
    let (w, h) = (cap.width as usize, cap.height as usize);
    let mut out = vec![0u8; 18];
    out[2] = 2;
    out[12..14].copy_from_slice(&(cap.width as u16).to_le_bytes());
    out[14..16].copy_from_slice(&(cap.height as u16).to_le_bytes());
    out[16] = 24;
    out[17] = origin;
    out.reserve(w * h * 3);
    for y in 0..h {
        let row = if top_down { y } else { h - 1 - y };
        for px in cap.rgb[row * w * 3..][..w * 3].as_chunks::<3>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    out
}

/// `levelshot`'s file (cod11-demos.md 10.1, cod2-demos.md 8.1,
/// cod4-demos.md 10.1): CoD1 box-filters the frame to 128 x 128 and writes
/// it bottom-up as its other TGAs; CoD2 and CoD4 resample the front buffer
/// and write its rows top-down under a header whose origin byte stays 0.
pub fn levelshot(game: Game, cap: &Capture) -> Vec<u8> {
    match game {
        Game::Cod1 => tga_rows(&box_filter(cap, 128, 512.0, 384.0), false, 0),
        Game::Cod2 | Game::Cod4 => tga_rows(&resample(cap, 128, 128), true, 0),
    }
}

/// `savegame`'s 512 x 512 JPEG at quality 90, rows top-down.
pub fn savegame(game: Game, cap: &Capture) -> Vec<u8> {
    match game {
        Game::Cod1 => jpeg(&box_filter(cap, 512, 2048.0, 1536.0), 90),
        Game::Cod2 | Game::Cod4 => jpeg(&resample(cap, 512, 512), 90),
    }
}

/// CoD1's `R_LevelShot` filter (`CoDMP.exe` 0x4b11c0, the save game's
/// 0x4b1520): output pixel (x, y), counted from the bottom row as
/// `glReadPixels` reads, averages 12 samples at `(x * 4 + i) * w / gw`,
/// `(y * 3 + j) * h / gh` (i < 4, j < 3, truncated), the sum divided by 12
/// truncating. `gw` and `gh` are 512 x 384 for the levelshot and 2048 x 1536
/// for the save game; the scales are `w` times the f32 reciprocal. The
/// result is top-down, as [`Capture`]s are.
fn box_filter(cap: &Capture, size: u32, gw: f32, gh: f32) -> Capture {
    let (w, h) = (cap.width as usize, cap.height as usize);
    let xs = cap.width as f64 * f64::from(1.0 / gw);
    let ys = cap.height as f64 * f64::from(1.0 / gh);
    let n = size as usize;
    let mut rgb = vec![0u8; n * n * 3];
    for y in 0..n {
        for x in 0..n {
            let mut sum = [0u32; 3];
            for j in 0..3 {
                // the GL row, from the bottom
                let gl = (((y * 3 + j) as f64 * ys) as usize).min(h - 1);
                let row = h - 1 - gl;
                for i in 0..4 {
                    let col = (((x * 4 + i) as f64 * xs) as usize).min(w - 1);
                    let px = &cap.rgb[(row * w + col) * 3..][..3];
                    for c in 0..3 {
                        sum[c] += u32::from(px[c]);
                    }
                }
            }
            let out = &mut rgb[((n - 1 - y) * n + x) * 3..][..3];
            for c in 0..3 {
                out[c] = (sum[c] / 12) as u8;
            }
        }
    }
    Capture {
        width: size,
        height: size,
        rgb,
    }
}

/// CoD2's and CoD4's `R_TakeResampledScreenshot` (`gfx_d3d_mp_x86_s.dll`
/// 0x10026500 and 0x10026100, `iw3mp.exe` 0x60e990): the rows to `w`, then
/// the columns to `h`, each pass an area average shrinking and a centred
/// linear blend growing, rounded to bytes between the passes.
fn resample(cap: &Capture, w: u32, h: u32) -> Capture {
    let (sw, sh) = (cap.width as usize, cap.height as usize);
    let (w, h) = (w as usize, h as usize);
    let mut rows = vec![0u8; w * sh * 3];
    for y in 0..sh {
        let src: Vec<&[u8]> = (0..sw).map(|x| &cap.rgb[(y * sw + x) * 3..][..3]).collect();
        for (x, px) in resample_line(&src, w).into_iter().enumerate() {
            rows[(y * w + x) * 3..][..3].copy_from_slice(&px);
        }
    }
    let mut rgb = vec![0u8; w * h * 3];
    for x in 0..w {
        let src: Vec<&[u8]> = (0..sh).map(|y| &rows[(y * w + x) * 3..][..3]).collect();
        for (y, px) in resample_line(&src, h).into_iter().enumerate() {
            rgb[(y * w + x) * 3..][..3].copy_from_slice(&px);
        }
    }
    Capture {
        width: w as u32,
        height: h as u32,
        rgb,
    }
}

/// One line of `n` pixels to `m`. Shrinking (0x10025d90) weighs each source
/// pixel by `m` and each output covers `n`, scale `1 / n`; growing
/// (0x10025f00) blends the two nearest at the output's centre in units of
/// `1 / 2m`, scale `0.5 / m`, the edges held. Each sum times its f32 scale
/// rounds to nearest after a `2^-30` nudge, as the `fistp` there does.
fn resample_line(src: &[&[u8]], m: usize) -> Vec<[u8; 3]> {
    let n = src.len();
    let round = |sum: u32, scale: f32| -> u8 {
        let v = f64::from(sum as f32 * scale) + f64::from(2f32.powi(-30));
        v.round_ties_even().clamp(0.0, 255.0) as u8
    };
    let px = |i: usize, c: usize| u32::from(src[i.min(n - 1)][c]);
    let mut out = Vec::with_capacity(m);
    if n > m {
        let scale = 1.0 / n as f32;
        // the current source pixel and the weight it has left
        let (mut i, mut left) = (0usize, m);
        for _ in 0..m {
            let mut sum = [0u32; 3];
            let mut need = n;
            while need > 0 {
                let take = left.min(need);
                for (c, s) in sum.iter_mut().enumerate() {
                    *s += px(i, c) * take as u32;
                }
                need -= take;
                left -= take;
                if left == 0 {
                    i += 1;
                    left = m;
                }
            }
            out.push(sum.map(|s| round(s, scale)));
        }
    } else if n < m {
        let scale = 0.5 / m as f32;
        for j in 0..m {
            // the centre in source pixels times 2m, from pixel 0's centre
            let pos = (2 * j + 1) as i64 * n as i64 - m as i64;
            let i = pos.div_euclid(2 * m as i64);
            let frac = pos.rem_euclid(2 * m as i64) as u32;
            let a = i.max(0) as usize;
            let b = (i + 1).max(0) as usize;
            let sum = |c| px(a, c) * (2 * m as u32 - frac) + px(b, c) * frac;
            out.push([0, 1, 2].map(|c| round(sum(c), scale)));
        }
    } else {
        out.extend(src.iter().map(|p| [p[0], p[1], p[2]]));
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
        let shot = |format, name, silent| Command::Screen(req(format, name, silent));
        assert_eq!(p(false, ""), shot(Format::Tga, None, false));
        assert_eq!(p(true, "silent"), shot(Format::Jpeg, None, true));
        assert_eq!(p(false, "foo"), shot(Format::Tga, Some("foo"), false));
        // two arguments: numbered; `silent` only first, case and all
        assert_eq!(p(false, "foo bar"), shot(Format::Tga, None, false));
        assert_eq!(p(false, "silent x"), shot(Format::Tga, None, true));
        assert_eq!(p(false, "Silent"), shot(Format::Tga, Some("Silent"), false));
        assert_eq!(p(true, "levelshot x y"), Command::Level);
        assert_eq!(p(false, "savegame s1"), Command::Savegame("s1".into()));
        // `savegame` needs exactly one name after it, else it is a name
        assert_eq!(
            p(false, "savegame"),
            shot(Format::Tga, Some("savegame"), false)
        );
        assert_eq!(p(false, "savegame a b"), shot(Format::Tga, None, false));
        let empty = vec!["savegame".to_string(), String::new()];
        assert_eq!(parse(false, &empty), shot(Format::Tga, None, false));
    }

    #[test]
    fn levelshots_and_save_games_land_where_each_game_writes_them() {
        let dir = tempfile::tempdir().unwrap();
        // red over the top half, blue below
        let mut c = cap(640, 480);
        c.rgb[..640 * 240 * 3]
            .chunks_mut(3)
            .for_each(|p| p.copy_from_slice(&[255, 0, 0]));
        let mut s = Shooter::default();
        for game in [Game::Cod1, Game::Cod2, Game::Cod4] {
            let lines = s.run(game, dir.path(), Some("mp_x"), &Command::Level, &c);
            assert_eq!(lines, ["Wrote levelshots/mp_x.tga"]);
            let tga = std::fs::read(dir.path().join("levelshots/mp_x.tga")).unwrap();
            // 128 x 128, 24 bits, origin byte 0 in every game
            assert_eq!(
                &tga[..18],
                &[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 0, 128, 0, 24, 0]
            );
            assert_eq!(tga.len(), 18 + 128 * 128 * 3);
            // B G R: the red top last in CoD1's file, first in the others'
            let (red, blue) = (&[0u8, 0, 255][..], &[255u8, 0, 0][..]);
            let (first, last) = (&tga[18..21], &tga[tga.len() - 3..]);
            match game {
                Game::Cod1 => assert_eq!((first, last), (blue, red)),
                _ => assert_eq!((first, last), (red, blue)),
            }
            let lines = s.run(
                game,
                dir.path(),
                None,
                &Command::Savegame("save/s1".into()),
                &c,
            );
            assert!(lines.is_empty());
            let jpg = std::fs::read(dir.path().join("save/s1.jpg")).unwrap();
            let img = image::load_from_memory(&jpg).unwrap();
            assert_eq!((img.width(), img.height()), (512, 512));
            std::fs::remove_file(dir.path().join("save/s1.jpg")).unwrap();
        }
        // no world: no file, no line
        assert!(
            s.run(Game::Cod2, dir.path(), None, &Command::Level, &c)
                .is_empty()
        );
    }

    #[test]
    fn cod1s_box_filter_reads_twelve_points_a_pixel() {
        // a 512 x 384 frame: output (x, y) averages columns 4x..4x+3 of GL
        // rows 3y..3y+2, the frame's bottom row GL row 0
        let (w, h) = (512usize, 384usize);
        let rgb = (0..h)
            .flat_map(|r| (0..w).flat_map(move |c| [(c % 4 * 10) as u8, (h - 1 - r) as u8, 0]))
            .collect();
        let out = box_filter(
            &Capture {
                width: 512,
                height: 384,
                rgb,
            },
            128,
            512.0,
            384.0,
        );
        // red: (0 + 10 + 20 + 30) * 3 / 12 = 15; green: GL rows 3y..3y+2
        // average 3y + 1
        let at = |x: usize, y_from_top: usize| &out.rgb[(y_from_top * 128 + x) * 3..][..3];
        assert_eq!(at(0, 127), &[15, 1, 0]);
        assert_eq!(at(5, 127 - 10), &[15, 31, 0]);
    }

    #[test]
    fn the_d3d_resample_averages_areas_and_blends_up() {
        let line = |v: &[u8], m| {
            let px: Vec<[u8; 3]> = v.iter().map(|&b| [b, b, b]).collect();
            let src: Vec<&[u8]> = px.iter().map(|p| &p[..]).collect();
            resample_line(&src, m)
                .iter()
                .map(|p| p[0])
                .collect::<Vec<_>>()
        };
        // 3 -> 2: (0 * 2 + 30 * 1) / 3 = 10, (30 * 1 + 61 * 2) / 3 = 50.67
        assert_eq!(line(&[0, 30, 61], 2), [10, 51]);
        // 4 -> 2: plain pairs, a half rounding up
        assert_eq!(line(&[0, 1, 10, 20], 2), [1, 15]);
        // 2 -> 4: centres at -0.25, 0.25, 0.75, 1.25; edges held
        assert_eq!(line(&[0, 100], 4), [0, 25, 75, 100]);
        assert_eq!(line(&[7, 9], 2), [7, 9]);
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
