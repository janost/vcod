//! Self-pins for the pmove tests: a run's FNV-1a digest is the cheap check,
//! and the step lines kept beside it in `tests/fixtures/pins/<name>.txt`
//! name the first step and field that moved when it misses. The values are
//! the model's own, not retail's; `cod4_moves_ab` is the retail gate.
//! `PIN_DUMP=1` rewrites the fixture of every pin the run checks. A fixture
//! is a `#` line of field names, then a line of values a step.

use std::fmt::{Display, Write as _};
use std::path::PathBuf;

use glam::Vec3;

const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// A digest with its step lines. [`Pin::words`] folds each 32-bit word at
/// once, [`Pin::bytes`] each little-endian byte: the two forms the pins
/// were first written in.
#[derive(Default)]
pub(crate) struct Pin {
    pub h: u64,
    bytes: bool,
    /// The field names, the same every step.
    names: Vec<String>,
    lines: Vec<String>,
    field: usize,
}

impl Pin {
    /// FNV-1a over whole words, the basis taken on the first word.
    pub fn words() -> Pin {
        Pin::default()
    }

    /// FNV-1a over bytes.
    pub fn bytes() -> Pin {
        Pin {
            h: FNV_BASIS,
            bytes: true,
            ..Pin::default()
        }
    }

    /// Starts the next step's line.
    pub fn step(&mut self) {
        self.lines.push(String::new());
        self.field = 0;
    }

    fn note(&mut self, name: &str, v: impl Display) {
        if self.lines.is_empty() {
            self.step();
        }
        if self.lines.len() == 1 {
            self.names.push(name.to_owned());
        } else {
            assert_eq!(self.names.get(self.field).map(String::as_str), Some(name));
        }
        self.field += 1;
        let line = self.lines.last_mut().unwrap();
        if !line.is_empty() {
            line.push(' ');
        }
        let _ = write!(line, "{v}");
    }

    fn fold(&mut self, w: u32) {
        if self.bytes {
            for b in w.to_le_bytes() {
                self.h = (self.h ^ u64::from(b)).wrapping_mul(FNV_PRIME);
            }
        } else {
            let h = if self.h == 0 { FNV_BASIS } else { self.h };
            self.h = (h ^ u64::from(w)).wrapping_mul(FNV_PRIME);
        }
    }

    /// A single byte, for the byte form only.
    pub fn byte(&mut self, name: &str, b: u8) {
        assert!(self.bytes, "a byte in a word digest");
        self.h = (self.h ^ u64::from(b)).wrapping_mul(FNV_PRIME);
        self.note(name, b);
    }

    pub fn u32(&mut self, name: &str, w: u32) {
        self.fold(w);
        self.note(name, format_args!("{w:x}"));
    }

    pub fn i32(&mut self, name: &str, w: i32) {
        self.fold(w as u32);
        self.note(name, w);
    }

    pub fn f32(&mut self, name: &str, v: f32) {
        self.fold(v.to_bits());
        self.note(name, v);
    }

    pub fn vec(&mut self, name: &str, v: Vec3) {
        for c in v.to_array() {
            self.fold(c.to_bits());
        }
        self.note(name, format_args!("{},{},{}", v.x, v.y, v.z));
    }

    /// Folds `other` in as `h ^= other.h.rotate_left(rot)`, its lines after
    /// these: the composite pins of a run played in parts.
    pub fn xor(&mut self, other: Pin, rot: u32) {
        self.h ^= other.h.rotate_left(rot);
        if self.names.is_empty() {
            self.names = other.names;
        } else {
            assert_eq!(self.names, other.names);
        }
        self.lines.extend(other.lines);
    }

    /// Asserts the digest is `want`. A miss names the first steps whose
    /// fields differ from the fixture; `PIN_DUMP` writes the fixture.
    pub fn check(&self, name: &str, want: u64) {
        if let Some(m) = self.miss(name, want) {
            panic!("{m}");
        }
    }

    /// [`Pin::check`]'s message on a miss, for a test that gathers them.
    pub fn miss(&self, name: &str, want: u64) -> Option<String> {
        let path = fixture(name);
        if std::env::var_os("PIN_DUMP").is_some() {
            let mut text = format!("# {}\n", self.names.join(" "));
            for l in &self.lines {
                text.push_str(l);
                text.push('\n');
            }
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        if self.h == want {
            return None;
        }
        let head = format!("pin {name}: digest {:#018x}, pinned {want:#018x}", self.h);
        Some(match std::fs::read_to_string(&path) {
            Ok(old) => format!("{head}\n{}", diff(&old, &self.names, &self.lines)),
            Err(_) => format!("{head}; no {} (PIN_DUMP=1 writes it)", path.display()),
        })
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pins")
        .join(format!("{name}.txt"))
}

/// The first steps of `new` that differ from the fixture text, by field.
fn diff(old: &str, names: &[String], new: &[String]) -> String {
    let old: Vec<&str> = old.lines().filter(|l| !l.starts_with('#')).collect();
    let mut out = Vec::new();
    for (i, (a, b)) in old.iter().zip(new).enumerate() {
        if *a == b.as_str() {
            continue;
        }
        let moved: Vec<String> = a
            .split(' ')
            .zip(b.split(' '))
            .zip(names)
            .filter(|((x, y), _)| x != y)
            .map(|((x, y), n)| format!("{n} {x} -> {y}"))
            .collect();
        out.push(format!("step {i}: {}", moved.join(", ")));
        if out.len() == 4 {
            break;
        }
    }
    if out.is_empty() || old.len() != new.len() {
        out.push(format!(
            "{} steps, the fixture has {}",
            new.len(),
            old.len()
        ));
    }
    out.join("\n")
}
