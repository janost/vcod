//! Retail's client shellshock: `scripts/<name>.shock` parsed into the 27
//! `cg_shock_*` cvars, and the per-frame effect a playerstate's
//! `shellshockIndex` / `shellshockTime` / `shellshockDuration` drives (the
//! screen blend, the view kick, the mouse damping, the sound mix and loop,
//! and the `wbuttons` 0x4 slowdown). Addresses and labels are in
//! docs/research/cod11-shellshock.md.

use vcod_common::pk3::Pk3Fs;
use vcod_common::text::atoi_msvc;

/// The cvar names in the parser's table order (cgame `0x300748a0`), with the
/// defaults `CG_RegisterCvars` gives them (`0x30075260`).
pub const CVARS: [(&str, &str); 27] = [
    ("cg_shock_screenBlendTime", "0.4"),
    ("cg_shock_screenBlendFadeTime", "1"),
    ("cg_shock_viewKickPeriod", ".75"),
    ("cg_shock_viewKickRadius", ".05"),
    ("cg_shock_sound", "1"),
    ("cg_shock_soundFadeInTime", "0.25"),
    ("cg_shock_soundFadeOutTime", "2.5"),
    ("cg_shock_soundLoopFadeTime", "1.5"),
    ("cg_shock_soundLoopEndDelay", "-3.0"),
    ("cg_shock_soundRoomType", "underwater"),
    ("cg_shock_soundWetLevel", "0.5"),
    ("cg_shock_soundModEndDelay", "2"),
    ("cg_shock_volume_auto", "0.1"),
    ("cg_shock_volume_menu", "1.0"),
    ("cg_shock_volume_weapon", "0.5"),
    ("cg_shock_volume_voice", "0.2"),
    ("cg_shock_volume_item", "0.1"),
    ("cg_shock_volume_body", "0.1"),
    ("cg_shock_volume_local", "1.0"),
    ("cg_shock_volume_music", "1.0"),
    ("cg_shock_volume_announcer", "1.0"),
    ("cg_shock_volume_shellshock", "1.0"),
    ("cg_shock_mouse", "1"),
    ("cg_shock_mouse_maxpitchspeed", "90"),
    ("cg_shock_mouse_maxyawspeed", "90"),
    ("cg_shock_mouse_sensitivityscale", "0.5"),
    ("cg_shock_mouse_fadeTime", "2"),
];

/// `cg_shock_volume_*` by engine channel (`auto, menu, weapon, voice, item,
/// body, local, music, announcer, shellshock`): the index into [`CVARS`]
/// `CG_ShellShockParms` (`0x3002eb30`) stores at that channel's slot. The
/// four 3D channels after `menu` are crossed: `body` scales the weapon
/// channel, `item` voices, `weapon` items and `voice` bodies.
const VOLUME_CVARS: [usize; 10] = [12, 13, 17, 16, 14, 15, 18, 19, 20, 21];

/// The view kick's fade, a constant in 1.1 (`0x3002ec13`): the `.shock`
/// files' `cg_shock_viewKickFadeTime` is no cvar the parser knows.
const KICK_FADE_MS: i32 = 3000;

/// One shellshock's parameters, cgame's 0x7c-byte record.
#[derive(Clone, Debug, PartialEq)]
pub struct ShockParams {
    pub blend_ms: i32,
    pub blend_fade_ms: i32,
    /// Kicks per ms, `0.001 / max(period, 0.001)`.
    pub kick_rate: f32,
    pub kick_radius: f32,
    pub sound: bool,
    pub fade_in_ms: i32,
    pub fade_out_ms: i32,
    pub loop_fade_ms: i32,
    pub loop_end_delay_ms: i32,
    /// Up to 15 characters, as `strncpy` leaves it.
    pub room: String,
    pub wet: f32,
    pub mod_end_delay_ms: i32,
    /// By `audio::alias::Channel` index ([`VOLUME_CVARS`]); 1.1 has no
    /// `auto2d`, so its slot (10) stays 1.
    pub volumes: [f32; 11],
    pub mouse: bool,
    pub max_pitch: f32,
    pub max_yaw: f32,
    pub sensitivity: f32,
    pub mouse_fade_ms: i32,
}

/// `(int)(x * 1000 + 2^-30)` under the x87's round-to-nearest.
fn ms(x: f32) -> i32 {
    (f64::from(x) * 1000.0 + 9.313226e-10).round_ties_even() as i32
}

/// The fades are at least a millisecond.
fn ms1(x: f32) -> i32 {
    ms(x).max(1)
}

/// A NaN lands on 0, as the compares fall through.
fn unit(x: f32) -> f32 {
    if x >= 0.0 { x.min(1.0) } else { 0.0 }
}

/// `atof`: the longest leading float, 0 for none.
pub(crate) fn atof(s: &str) -> f32 {
    let s = s.trim_start();
    let b = s.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    while end < b.len() {
        let c = b[end];
        match c {
            b'+' | b'-' if end == 0 || matches!(b[end - 1], b'e' | b'E') => {}
            b'0'..=b'9' => seen_digit = true,
            b'.' if !seen_dot && !seen_exp => seen_dot = true,
            b'e' | b'E' if seen_digit && !seen_exp => seen_exp = true,
            _ => break,
        }
        end += 1;
    }
    // Back off a dangling exponent or sign.
    while end > 0 {
        if let Ok(v) = s[..end].parse::<f32>() {
            return v;
        }
        end -= 1;
    }
    0.0
}

/// `COM_Parse`'s tokens with the line each starts on: whitespace, `//` and
/// `/* */` comments skipped, a quoted string one token.
fn tokens(text: &str) -> Vec<(usize, String)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 0);
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() || c < b' ' {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            i += 2;
            while i < b.len() && !b[i..].starts_with(b"*/") {
                line += usize::from(b[i] == b'\n');
                i += 1;
            }
            i += 2;
        } else if c == b'"' {
            let start = i + 1;
            i = start;
            while i < b.len() && b[i] != b'"' && b[i] != b'\n' {
                i += 1;
            }
            out.push((line, text[start..i.min(b.len())].to_string()));
            i += usize::from(i < b.len() && b[i] == b'"');
        } else {
            let start = i;
            while i < b.len() && b[i] > b' ' {
                i += 1;
            }
            out.push((line, text[start..i].to_string()));
        }
    }
    out
}

/// The parser's view of a `.shock` file (CoDMP.exe `0x438eb0`): the first
/// token of a line names a cvar, the next token on the same line is its
/// value, the rest of the line is skipped. An unknown name is a warning;
/// any of the 27 left unset fails the file. Returns the 27 values in
/// [`CVARS`] order.
pub fn parse_cvars(text: &str, file: &str) -> Result<[String; 27], Vec<&'static str>> {
    let names = CVARS.map(|(n, _)| n);
    let v = parse_named(text, file, &names)?;
    Ok(std::array::from_fn(|i| v[i].clone()))
}

/// [`parse_cvars`] over any name table: CoD2's `Com_LoadDvarsFromBuffer`
/// (Mac 0x54806) reads its 29 the same way.
pub fn parse_named(
    text: &str,
    file: &str,
    names: &[&'static str],
) -> Result<Vec<String>, Vec<&'static str>> {
    let mut values: Vec<Option<String>> = vec![None; names.len()];
    let toks = tokens(text);
    let mut i = 0;
    while i < toks.len() {
        let (line, name) = &toks[i];
        let value = toks.get(i + 1).filter(|(l, _)| l == line);
        match names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            Some(k) => values[k] = Some(value.map_or(String::new(), |(_, v)| v.clone())),
            None => log::warn!("shellshock: unknown cvar '{name}' in file '{file}'"),
        }
        i += 1;
        while i < toks.len() && toks[i].0 == *line {
            i += 1;
        }
    }
    let missing: Vec<&str> = names
        .iter()
        .zip(&values)
        .filter(|(_, v)| v.is_none())
        .map(|(n, _)| *n)
        .collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    Ok(values.into_iter().map(Option::unwrap_or_default).collect())
}

impl ShockParams {
    /// `CG_ShellShockParms` (`0x3002eb30`) over the 27 cvar strings.
    pub fn from_cvars(v: &[String; 27]) -> ShockParams {
        let f = |i: usize| atof(&v[i]);
        ShockParams {
            blend_ms: ms1(f(0)),
            blend_fade_ms: ms1(f(1)),
            kick_rate: 0.001 / f(2).max(0.001),
            kick_radius: f(3),
            sound: atoi_msvc(&v[4]) != 0,
            fade_in_ms: ms1(f(5)),
            fade_out_ms: ms1(f(6)),
            loop_fade_ms: ms1(f(7)),
            loop_end_delay_ms: ms(f(8)),
            room: v[9].chars().take(15).collect(),
            wet: unit(f(10)),
            mod_end_delay_ms: ms(f(11)),
            volumes: {
                let mut v = [1.0; 11];
                for (o, i) in v.iter_mut().zip(VOLUME_CVARS) {
                    *o = unit(f(i));
                }
                v
            },
            mouse: atoi_msvc(&v[22]) != 0,
            max_pitch: f(23),
            max_yaw: f(24),
            sensitivity: f(25),
            mouse_fade_ms: ms1(f(26)),
        }
    }

    /// The registered defaults, what `cg_shellshock <duration>` uses before
    /// any file has been read.
    pub fn defaults() -> ShockParams {
        ShockParams::from_cvars(&CVARS.map(|(_, d)| d.to_string()))
    }

    /// `scripts/<name>.shock`, `None` (with the console's message) when the
    /// file is missing or leaves a cvar out.
    pub fn load(fs: &Pk3Fs, name: &str) -> Option<ShockParams> {
        let path = format!("scripts/{name}.shock");
        let Some(bytes) = fs.read(&path) else {
            log::warn!("shellshock: couldn't open '{path}'");
            return None;
        };
        match parse_cvars(&String::from_utf8_lossy(&bytes), &path) {
            Ok(v) => Some(ShockParams::from_cvars(&v)),
            Err(missing) => {
                log::warn!(
                    "shellshock: the following cvars were not specified in file '{path}': {}",
                    missing.join(" ")
                );
                None
            }
        }
    }
}

/// The view kick's noise: 128 points in the unit disc (cgame `0x300608f8`),
/// the table's first three repeated after it so a cubic can read past 127.
#[rustfmt::skip]
const KICK_POINTS: [[f32; 2]; 131] = include!("shock_kick.in");

/// Bourke's cubic through `p1..p2` at `t`, as cgame `0x3002f6b0` spells it.
fn cubic(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = p3 - p2 + p1 - p0;
    ((p2 - p0 + (a * t + (p0 - p1 - a)) * t) * t) + p1
}

/// The view kick in the view's own frame (`0x3002f6b0`): the forward axis
/// moves by `[left, up]` before it is normalized. Zero once the shock is
/// over; full radius until its last 3 s, then a smoothstep down.
pub fn view_kick(p: &ShockParams, elapsed: i32, duration: i32) -> [f32; 2] {
    let remaining = duration - elapsed;
    if remaining < 1 {
        return [0.0, 0.0];
    }
    let amp = kick_amplitude(p, remaining);
    let t = elapsed as f32 * p.kick_rate;
    let i = t.floor();
    let frac = t - i;
    let base = (duration.wrapping_mul(0x3d).wrapping_add(i as i32) & 0x7f) as usize;
    let k = |axis: usize| {
        let q = |n: usize| KICK_POINTS[base + n][axis];
        cubic(q(0), q(1), q(2), q(3), frac) * amp
    };
    [k(0), k(1)]
}

/// `0x3002e7a0`: the view axes `angles` (wire degrees, pitch down) turned
/// by `kick` in their own frame. The kicked forward is `forward + kick[0] *
/// left + kick[1] * up` normalized, the left the world up crossed with it,
/// the up their cross, multiplied onto the axes; back to angles the way
/// `AxisToAngles` reads them.
pub fn kicked_angles(angles: [f32; 3], kick: [f32; 2]) -> [f32; 3] {
    use glam::Vec3;
    if kick == [0.0, 0.0] {
        return angles;
    }
    let f = Vec3::new(1.0, kick[0], kick[1]).normalize();
    let l = Vec3::Z.cross(f).normalize();
    let u = f.cross(l);
    let [af, al, au] = vcod_common::pmove::aim::angles_to_axis_msvc(angles).map(Vec3::from);
    let turn = |k: Vec3| af * k.x + al * k.y + au * k.z;
    let (nf, nl, nu) = (turn(f), turn(l), turn(u));
    let yaw = nf.y.atan2(nf.x).to_degrees();
    let pitch = (-nf.z).atan2(nf.x.hypot(nf.y)).to_degrees();
    let roll = nl.z.atan2(nu.z).to_degrees();
    [pitch, yaw, roll]
}

/// The kick's radius `remaining` ms before the end.
fn kick_amplitude(p: &ShockParams, remaining: i32) -> f32 {
    let f = if remaining < KICK_FADE_MS {
        remaining as f32 / KICK_FADE_MS as f32
    } else {
        1.0
    };
    (3.0 - (f + f)) * f * f * p.kick_radius
}

/// The turn sway's factor for a weapon whose `swayShellShockScale` is
/// `weapon_scale`, `left_ms` before the shock ends (`0x300361f0`): the kick's
/// smoothstep over the last 3 s, 1 once it is over.
pub fn sway_scale(weapon_scale: f32, left_ms: i32) -> f32 {
    if left_ms < 1 {
        return 1.0;
    }
    let f = if left_ms < KICK_FADE_MS {
        left_ms as f32 / KICK_FADE_MS as f32
    } else {
        1.0
    };
    (weapon_scale - 1.0) * (3.0 - (f + f)) * f * f + 1.0
}

/// The mouse half (`0x3002f5b0`): the sensitivity scale and the turn caps,
/// degrees a second, 0 for none (CoDMP.exe `CL_MouseMove` 0x40b240).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShockMouse {
    pub sensitivity: f32,
    pub max_pitch: f32,
    pub max_yaw: f32,
}

impl ShockMouse {
    pub const NONE: ShockMouse = ShockMouse {
        sensitivity: 1.0,
        max_pitch: 0.0,
        max_yaw: 0.0,
    };
}

fn mouse(p: &ShockParams, elapsed: i32, duration: i32) -> ShockMouse {
    if !p.mouse {
        return ShockMouse::NONE;
    }
    let remaining = duration - elapsed;
    if remaining < p.mouse_fade_ms {
        if remaining < 1 {
            return ShockMouse::NONE;
        }
        let f = remaining as f32 / p.mouse_fade_ms as f32;
        if f != 1.0 {
            return ShockMouse {
                sensitivity: (p.sensitivity - 1.0) * f + 1.0,
                max_pitch: 1.0 / f * p.max_pitch,
                max_yaw: 1.0 / f * p.max_yaw,
            };
        }
    }
    ShockMouse {
        sensitivity: p.sensitivity,
        max_pitch: p.max_pitch,
        max_yaw: p.max_yaw,
    }
}

/// The sound half's per-frame state (`0x3002f2d0`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShockSound {
    /// By `audio::alias::Channel` index, multiplied into every voice
    /// (syscall 0xd8).
    pub volumes: [f32; 11],
    /// The room type and its wet level (syscall 0xd9).
    pub room: String,
    pub wet: f32,
    /// `shellshock_loop` blended toward `shellshock_loop_silent` by this
    /// (syscall 0xc3), refreshed while `Some`.
    pub loop_blend: Option<f32>,
    /// A one-shot this frame: the alias and how far into it, ms.
    pub cue: Option<(&'static str, i32)>,
}

impl ShockSound {
    fn idle(cue: Option<(&'static str, i32)>) -> ShockSound {
        ShockSound {
            volumes: [1.0; 11],
            room: "generic".into(),
            wet: 0.0,
            loop_blend: None,
            cue,
        }
    }
}

/// What a frame of shellshock asks of the rest of the client.
#[derive(Clone, Debug, PartialEq)]
pub struct ShockFrame {
    /// The screen blend: `Some(ms)` captures the finished 3D view after
    /// blending the last capture over it with `ms` as the decay
    /// (render command 10, 0 skips the blend); `None` captures nothing and
    /// forgets the last capture.
    pub blend_ms: Option<i32>,
    /// [`view_kick`].
    pub kick: [f32; 2],
    pub mouse: ShockMouse,
    /// `wbuttons` 0x4 on the next cmds (syscall 0x57).
    pub slowed: bool,
    pub sound: ShockSound,
    /// The shock's end less this frame's clock, ms; 0 with none. The view
    /// weapon's sway reads it ([`sway_scale`]).
    pub left_ms: i32,
}

impl ShockFrame {
    /// No shock: what a frame with no snapshot to read asks for.
    pub fn rest() -> ShockFrame {
        ShockFrame {
            blend_ms: None,
            kick: [0.0, 0.0],
            mouse: ShockMouse::NONE,
            slowed: false,
            sound: ShockSound::idle(None),
            left_ms: 0,
        }
    }
}

/// The per-client shellshock state: the parsed records by configstring
/// slot, the console's own shock in slot 0, and the latches the cgame keeps
/// between frames.
#[derive(Default)]
pub struct ShellShock {
    /// Slot `i` is configstring `CsRange::ShellShock` + `i - 1`; parsed by
    /// name, `None` for a file that failed.
    loaded: std::collections::HashMap<String, Option<ShockParams>>,
    /// `cg_shellshock`'s record and `(start, duration)` (`0x3020d5d0`).
    local: Option<(ShockParams, i32, i32)>,
    /// The last file parsed, which `cg_shellshock <duration>` reuses, as the
    /// cvars hold it.
    last: Option<ShockParams>,
    /// The screen blend captured last frame (`0x3020d5cc`).
    captured: bool,
    /// The end cue's latch (`0x3020d5bc`): the end time it played for.
    end_latch: i32,
    /// How a name becomes a record: 1.1's `scripts/<name>.shock` when
    /// `None`, CoD2's `shock/<name>.shock` through
    /// `play::cod2_shock::load`.
    loader: Option<fn(&Pk3Fs, &str) -> Option<ShockParams>>,
    /// The registered defaults `cg_shellshock <duration>` takes before any
    /// file loaded: 1.1's when `None`.
    defaults: Option<fn() -> ShockParams>,
}

impl ShellShock {
    /// A fresh cgame's state for a map load or an in-game `vid_restart`:
    /// only the last file parsed, which the cvars hold, survives.
    pub fn restart_cgame(&mut self) -> ShellShock {
        ShellShock {
            last: self.last.take(),
            loader: self.loader,
            defaults: self.defaults,
            ..ShellShock::default()
        }
    }

    /// A state whose records load through `loader` (CoD2's files), with
    /// `defaults` its registered dvars.
    pub fn with_loader(
        loader: fn(&Pk3Fs, &str) -> Option<ShockParams>,
        defaults: fn() -> ShockParams,
    ) -> ShellShock {
        ShellShock {
            loader: Some(loader),
            defaults: Some(defaults),
            ..ShellShock::default()
        }
    }

    fn params(&mut self, fs: &Pk3Fs, name: &str) -> Option<&ShockParams> {
        if !self.loaded.contains_key(name) {
            let p = match self.loader {
                Some(load) => load(fs, name),
                None => ShockParams::load(fs, name),
            };
            if let Some(p) = &p {
                self.last = Some(p.clone());
            }
            self.loaded.insert(name.to_string(), p);
        }
        self.loaded.get(name).and_then(Option::as_ref)
    }

    /// `cg_shellshock <duration> [file]` (`0x30013e70`) at `now`, the
    /// shock slot 0 plays while the playerstate names none. `Err` is the
    /// console's reply.
    pub fn command(&mut self, fs: &Pk3Fs, args: &[&str], now: i32) -> Result<(), String> {
        let params = match args {
            [_] => self
                .last
                .clone()
                .unwrap_or_else(self.defaults.unwrap_or(ShockParams::defaults)),
            // CoD2's loader warns on its own (`CG_LoadShellShockDvars`) and
            // the command then starts nothing (`CG_ShellShock_f`, Mac
            // 0x1d2e3c): an empty reply.
            [_, file] => {
                let p = match self.loader {
                    Some(load) => load(fs, file).ok_or_else(String::new)?,
                    None => ShockParams::load(fs, file)
                        .ok_or_else(|| format!("couldn't load 'scripts/{file}.shock'"))?,
                };
                self.last = Some(p.clone());
                p
            }
            _ => return Err("USAGE: cg_shellshock <duration> <filename?>".into()),
        };
        let duration = ms(atof(args[0]));
        self.local = Some((params, now, duration));
        Ok(())
    }

    /// One frame at `now` (cg.time) for the playerstate's
    /// `(shellshockIndex, shellshockTime, shellshockDuration)`; `names`
    /// resolves an index to its configstring.
    pub fn frame<'a>(
        &mut self,
        fs: &Pk3Fs,
        names: impl Fn(i32) -> Option<&'a str>,
        ps: (i32, i32, i32),
        now: i32,
    ) -> ShockFrame {
        let (index, start, duration) = ps;
        let picked = if index == 0 {
            self.local.as_ref().map(|(p, s, d)| (p.clone(), *s, *d))
        } else {
            names(index)
                .filter(|n| !n.is_empty())
                .and_then(|n| self.params(fs, n).cloned())
                .map(|p| (p, start, duration))
        };
        let Some((p, start, duration)) = picked else {
            self.captured = false;
            return self.reset();
        };
        let blend_ms = self.screen_blend(&p, start, duration, now);
        let left_ms = start.wrapping_add(duration).wrapping_sub(now);
        let elapsed = now.wrapping_sub(start);
        if start == 0 || elapsed < 0 {
            return ShockFrame {
                blend_ms,
                left_ms,
                ..self.reset()
            };
        }
        ShockFrame {
            blend_ms,
            kick: view_kick(&p, elapsed, duration),
            mouse: mouse(&p, elapsed, duration),
            slowed: elapsed < duration,
            sound: self.sound(&p, start, duration, now),
            left_ms,
        }
    }

    /// `0x3002f270`: everything back to rest, the end cue cut off if one
    /// played.
    fn reset(&mut self) -> ShockFrame {
        ShockFrame {
            blend_ms: None,
            kick: [0.0, 0.0],
            mouse: ShockMouse::NONE,
            slowed: false,
            sound: ShockSound::idle(self.abort_end()),
            left_ms: 0,
        }
    }

    fn abort_end(&mut self) -> Option<(&'static str, i32)> {
        (std::mem::take(&mut self.end_latch) != 0).then_some(("shellshock_end_abort", 0))
    }

    /// `0x3002e900`: the blend's decay, shortened over the last
    /// `blend_fade_ms` of the shock.
    fn screen_blend(
        &mut self,
        p: &ShockParams,
        start: i32,
        duration: i32,
        now: i32,
    ) -> Option<i32> {
        let remaining = start.wrapping_sub(now).wrapping_add(duration);
        if start == 0 || duration <= 0 || remaining <= 0 {
            self.captured = false;
            return None;
        }
        let ms = if remaining < p.blend_fade_ms {
            (remaining as f32 / p.blend_fade_ms as f32 * p.blend_ms as f32 + 9.313226e-10)
                .round_ties_even() as i32
        } else {
            p.blend_ms
        };
        // The first frame only captures.
        Some(if std::mem::replace(&mut self.captured, true) {
            ms
        } else {
            0
        })
    }

    fn sound(&mut self, p: &ShockParams, start: i32, duration: i32, now: i32) -> ShockSound {
        if !p.sound {
            return ShockSound::idle(self.abort_end());
        }
        let elapsed = now - start;
        let until_off = p.mod_end_delay_ms - elapsed + p.fade_out_ms + duration;
        let f = if until_off < p.fade_out_ms {
            (until_off as f32 / p.fade_out_ms as f32).max(0.0)
        } else if elapsed < p.fade_in_ms {
            (elapsed as f32 / p.fade_in_ms as f32).max(0.0)
        } else {
            1.0
        };
        let volumes = p.volumes.map(|v| (v - 1.0) * f + 1.0);
        let (room, wet) = if f == 0.0 {
            ("generic".to_string(), 0.0)
        } else {
            (p.room.clone(), f * p.wet)
        };
        let until_loop_off = p.loop_end_delay_ms - elapsed + p.loop_fade_ms + duration;
        let loop_blend = (until_loop_off > 0)
            .then(|| (1.0 - until_loop_off as f32 / p.loop_fade_ms as f32).max(0.0));
        let end = start
            .wrapping_add(duration)
            .wrapping_add(p.loop_end_delay_ms);
        let cue = if now < end {
            self.abort_end()
        } else if end != self.end_latch {
            self.end_latch = end;
            Some(("shellshock_end", now - end))
        } else {
            None
        };
        ShockSound {
            volumes,
            room,
            wet,
            loop_blend,
            cue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_SHOCK: &str = "cg_shock_screenBlendTime \"0.4\"\r\n\
        cg_shock_screenBlendFadeTime \"1\"\r\n\
        cg_shock_viewKickPeriod \".75\"\r\n\
        cg_shock_viewKickRadius \".05\"\r\n\
        cg_shock_viewKickFadeTime \"3\"\r\n\
        cg_shock_sound \"1\"\r\n\
        cg_shock_soundFadeInTime \".25\"\r\n\
        cg_shock_soundFadeOutTime \"2.5\"\r\n\
        cg_shock_soundLoopFadeTime \"2\"\r\n\
        cg_shock_soundLoopEndDelay \"-1.5\"\r\n\
        cg_shock_soundRoomType \"underwater\"\r\n\
        cg_shock_soundWetLevel \"0.5\"\r\n\
        cg_shock_soundModEndDelay \"-.75\"\r\n\
        cg_shock_volume_auto \"0.1\"\r\n\
        cg_shock_volume_menu \"1.0\"\r\n\
        cg_shock_volume_weapon \"0.5\"\r\n\
        cg_shock_volume_voice \"0.2\"\r\n\
        cg_shock_volume_item \"0.1\"\r\n\
        cg_shock_volume_body \"0.1\"\r\n\
        cg_shock_volume_local \".1\"\r\n\
        cg_shock_volume_music \".1\"\r\n\
        cg_shock_volume_announcer \"1\"\r\n\
        cg_shock_volume_shellshock \"1.0\"\r\n\
        cg_shock_mouse \"1\"\r\n\
        cg_shock_mouse_maxpitchspeed \"90\"\r\n\
        cg_shock_mouse_maxyawspeed \"90\"\r\n\
        cg_shock_mouse_sensitivityscale \"0.5\"\r\n\
        cg_shock_mouse_fadeTime \"2\"\r\n";

    fn default_shock() -> ShockParams {
        ShockParams::from_cvars(&parse_cvars(DEFAULT_SHOCK, "default").unwrap())
    }

    /// `pak4.pk3`'s `scripts/default.shock` through the record builder.
    #[test]
    fn default_shock_builds_retails_record() {
        let p = default_shock();
        assert_eq!((p.blend_ms, p.blend_fade_ms), (400, 1000));
        assert!((p.kick_rate - 0.001 / 0.75).abs() < 1e-9);
        assert_eq!(p.kick_radius, 0.05);
        assert!(p.sound);
        assert_eq!(
            (p.fade_in_ms, p.fade_out_ms, p.loop_fade_ms),
            (250, 2500, 2000)
        );
        assert_eq!((p.loop_end_delay_ms, p.mod_end_delay_ms), (-1500, -750));
        assert_eq!((p.room.as_str(), p.wet), ("underwater", 0.5));
        // auto, menu, then body/item/weapon/voice crossed onto weapon/voice/
        // item/body, then local, music, announcer, shellshock.
        assert_eq!(
            p.volumes,
            [0.1, 1.0, 0.1, 0.1, 0.5, 0.2, 0.1, 0.1, 1.0, 1.0, 1.0]
        );
        assert!(p.mouse);
        assert_eq!((p.max_pitch, p.max_yaw, p.sensitivity), (90.0, 90.0, 0.5));
        assert_eq!(p.mouse_fade_ms, 2000);
    }

    /// The parser skips comment lines and refuses a file that leaves a cvar
    /// out, naming it.
    #[test]
    fn a_missing_cvar_fails_the_file() {
        let commented =
            format!("// cg_shock_sound \"0\"\n{DEFAULT_SHOCK}/* cg_shock_mouse \"0\" */\n");
        assert_eq!(
            ShockParams::from_cvars(&parse_cvars(&commented, "x").unwrap()),
            default_shock()
        );
        let short = DEFAULT_SHOCK.replace("cg_shock_mouse_fadeTime \"2\"\r\n", "");
        assert_eq!(
            parse_cvars(&short, "x").unwrap_err(),
            vec!["cg_shock_mouse_fadeTime"]
        );
    }

    #[test]
    fn number_parsing_is_atof_and_atoi() {
        assert_eq!(atof("-.75"), -0.75);
        assert_eq!(atof("00000"), 0.0);
        assert_eq!(atof("1.5x"), 1.5);
        assert_eq!(atof("1e"), 1.0);
        assert_eq!(atof(""), 0.0);
        assert_eq!(atoi_msvc("0.5"), 0);
        assert_eq!(atoi_msvc("-3"), -3);
        assert_eq!(ms(-0.75), -750);
        assert_eq!(ms1(0.0), 1);
    }

    /// The kick runs at full radius until 3 s before the end, smoothsteps
    /// to zero, and is a pure function of the shock's duration and time.
    #[test]
    fn view_kick_fades_over_the_last_three_seconds() {
        let p = default_shock();
        assert_eq!(view_kick(&p, 2500, 2500), [0.0, 0.0]);
        let full = view_kick(&p, 100, 10_000);
        assert!(full[0].hypot(full[1]) <= 0.05 * 1.5, "{full:?}");
        assert_eq!(view_kick(&p, 100, 10_000), full);
        // Half way through the fade the smoothstep is a half.
        assert_eq!(kick_amplitude(&p, 3000), 0.05);
        assert_eq!(kick_amplitude(&p, 1500), 0.025);
        assert_eq!(kick_amplitude(&p, 750), 0.05 * 0.15625);
        // At a knot the cubic passes through the table point.
        let mut q = p.clone();
        q.kick_rate = 0.001;
        let at_knot = view_kick(&q, 5000, 100_000);
        let base = ((100_000i32 * 0x3d + 5) & 0x7f) as usize;
        assert!((at_knot[0] - KICK_POINTS[base + 1][0] * 0.05).abs() < 1e-6);
        assert!((at_knot[1] - KICK_POINTS[base + 1][1] * 0.05).abs() < 1e-6);
    }

    /// Level, a sideways kick is a yaw and an upward one a pitch up; on a
    /// rolled view the kick stays in the view's frame.
    #[test]
    fn kick_turns_the_view_in_its_own_frame() {
        assert_eq!(
            kicked_angles([10.0, 20.0, 5.0], [0.0, 0.0]),
            [10.0, 20.0, 5.0]
        );
        let [p, y, r] = kicked_angles([0.0, 90.0, 0.0], [0.05, 0.0]);
        assert!(p.abs() < 1e-4 && r.abs() < 1e-4);
        assert!(
            (y - (90.0 + 0.05f32.atan().to_degrees())).abs() < 1e-3,
            "{y}"
        );
        let [p, y, _] = kicked_angles([0.0, 0.0, 0.0], [0.0, 0.05]);
        assert!((p + 0.05f32.atan().to_degrees()).abs() < 1e-3, "{p}");
        assert!(y.abs() < 1e-4);
        // Rolled 90 degrees the view's up is the world's right: a kick up
        // it turns the yaw right.
        let [_, y, r] = kicked_angles([0.0, 0.0, 90.0], [0.0, 0.05]);
        assert!(y < -2.0 && (r - 90.0).abs() < 1e-2, "{y} {r}");
    }

    #[test]
    fn sway_scale_eases_out_over_the_last_three_seconds() {
        assert_eq!(sway_scale(5.0, 10_000), 5.0);
        assert_eq!(sway_scale(5.0, 1500), 3.0);
        assert_eq!(sway_scale(5.0, 0), 1.0);
        assert_eq!(sway_scale(0.0, 0), 1.0);
    }

    #[test]
    fn mouse_damps_and_fades_back() {
        let p = default_shock();
        let full = mouse(&p, 0, 5000);
        assert_eq!(
            full,
            ShockMouse {
                sensitivity: 0.5,
                max_pitch: 90.0,
                max_yaw: 90.0
            }
        );
        // A second left of a 2 s fade: half way back.
        let m = mouse(&p, 4000, 5000);
        assert_eq!(m.sensitivity, 0.75);
        assert_eq!((m.max_pitch, m.max_yaw), (180.0, 180.0));
        assert_eq!(mouse(&p, 5000, 5000), ShockMouse::NONE);
    }

    /// The sound mix over a 5 s shock: in over 0.25 s, out over 2.5 s from
    /// 0.75 s before the end; the loop to 1.5 s before the end and faded
    /// over the 2 s after; the end cue once at 1.5 s before the end.
    #[test]
    fn sound_mix_follows_the_records_times() {
        let p = default_shock();
        let mut s = ShellShock::default();
        let (start, dur) = (10_000, 5000);
        let at = |s: &mut ShellShock, t: i32| s.sound(&p, start, dur, start + t);
        let first = at(&mut s, 125);
        assert!(
            (first.volumes[0] - 0.55).abs() < 1e-6,
            "{:?}",
            first.volumes
        );
        assert_eq!(first.room, "underwater");
        assert_eq!(first.wet, 0.25);
        assert_eq!(first.loop_blend, Some(0.0));
        assert_eq!(first.cue, None);
        let full = at(&mut s, 2000);
        for (a, b) in full.volumes.iter().zip(p.volumes) {
            assert!((a - b).abs() < 1e-6, "{:?}", full.volumes);
        }
        // The end cue at 3500, a frame late plays 16 ms in.
        let end = at(&mut s, 3516);
        assert_eq!(end.cue, Some(("shellshock_end", 16)));
        assert_eq!(at(&mut s, 3532).cue, None);
        // Mod fade: off 2.5 s after 4250.
        let fading = at(&mut s, 4250 + 1250);
        assert!((fading.volumes[0] - 0.55).abs() < 1e-6);
        let off = at(&mut s, 4250 + 2500);
        assert_eq!(
            (off.room.as_str(), off.wet, off.volumes),
            ("generic", 0.0, [1.0; 11])
        );
        // Loop: half faded a second past 3500, gone at 5500.
        assert_eq!(at(&mut s, 4500).loop_blend, Some(0.5));
        assert_eq!(at(&mut s, 5500).loop_blend, None);
        // A new shock before its end time aborts the end cue once.
        let again = s.sound(&p, 20_000, dur, 20_100);
        assert_eq!(again.cue, Some(("shellshock_end_abort", 0)));
        assert_eq!(s.sound(&p, 20_000, dur, 20_116).cue, None);
    }

    /// The first frame only captures; the decay shortens over the last
    /// second; the shock's end drops the capture.
    #[test]
    fn screen_blend_captures_then_blends() {
        let p = default_shock();
        let mut s = ShellShock::default();
        assert_eq!(s.screen_blend(&p, 1000, 3000, 1000), Some(0));
        assert_eq!(s.screen_blend(&p, 1000, 3000, 1016), Some(400));
        assert_eq!(s.screen_blend(&p, 1000, 3000, 3500), Some(200));
        assert_eq!(s.screen_blend(&p, 1000, 3000, 4000), None);
        assert_eq!(s.screen_blend(&p, 1000, 3000, 3990), Some(0));
    }
}
