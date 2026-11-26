//! A trace tape: every collision query a move asks and the answer it got,
//! recorded against a real map and replayed with no map at all, so an A/B
//! gate that needs a retail install can also run in CI
//! (docs/dev/testing.md, "Trace tapes").
//!
//! The tape is text, one query per line, `<query> = <answer>`, floats in
//! their shortest round-trip form so the bits come back exact. It holds
//! numbers only: no map geometry beyond the answers. A replay answers each
//! query from the next line and panics on the first query that is not the
//! one recorded there, so a change to what pmove asks fails at the query
//! that moved.
//!
//! Two shorthands keep it small. `^k` is the query and answer of the
//! entry `k` back, asked again. A query's first token is its kind, and a
//! `.` in its place `i` is token `i` of the last query of that kind, as
//! is each token past the end of a shortened line: a kind's queries all
//! carry the same number of tokens.
//!
//! The core is game-agnostic: each engine's seam formats its query and
//! answer ([`cod1`] for [`crate::movetrace::MoveWorld`], which CoD2's pmove
//! traces through too; `vcod_cod4::pmove::tape` for CoD4's `PmWorld`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;

/// One recorded query and its answer, both as written on the tape.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Entry {
    query: String,
    answer: String,
}

enum Mode {
    Record(RefCell<Vec<Entry>>),
    Replay {
        entries: Vec<Entry>,
        /// The tape line of each entry, for the mismatch message.
        lines: Vec<usize>,
        next: Cell<usize>,
    },
}

pub struct Tape {
    mode: Mode,
}

impl Tape {
    /// An empty tape that records what the live world answers.
    pub fn record() -> Tape {
        Tape {
            mode: Mode::Record(RefCell::new(Vec::new())),
        }
    }

    /// A tape that answers from `text`; `#` lines and blank lines are
    /// comments.
    pub fn replay(text: &str) -> Tape {
        let mut entries: Vec<Entry> = Vec::new();
        let mut lines = Vec::new();
        let mut last: HashMap<String, Vec<String>> = HashMap::new();
        for (n, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let bad = |why: &str| -> ! { panic!("trace tape line {}: {why}: {line:?}", n + 1) };
            let e = if let Some(k) = line.strip_prefix('^') {
                let k: usize = k.parse().unwrap_or_else(|_| bad("not `^k`"));
                let i = entries
                    .len()
                    .checked_sub(k)
                    .unwrap_or_else(|| bad("before the tape"));
                entries[i].clone()
            } else {
                let (short, answer) = line.split_once(" = ").unwrap_or_else(|| bad("no ` = `"));
                let mut toks = short.split(' ');
                let kind = toks.next().unwrap_or_default();
                let prev = last.get(kind).map_or(&[][..], Vec::as_slice);
                let mut query = kind.to_owned();
                let mut count = 1;
                for (i, t) in toks.enumerate() {
                    let t = match t {
                        "." => prev
                            .get(i)
                            .unwrap_or_else(|| bad("`.` with no query before")),
                        t => t,
                    };
                    query.push(' ');
                    query.push_str(t);
                    count += 1;
                }
                for t in prev.iter().skip(count - 1) {
                    query.push(' ');
                    query.push_str(t);
                }
                Entry {
                    query,
                    answer: answer.to_owned(),
                }
            };
            let mut toks = e.query.split(' ');
            let kind = toks.next().unwrap_or_default().to_owned();
            last.insert(kind, toks.map(str::to_owned).collect());
            entries.push(e);
            lines.push(n + 1);
        }
        Tape {
            mode: Mode::Replay {
                entries,
                lines,
                next: Cell::new(0),
            },
        }
    }

    /// Queries recorded, or queries on the tape.
    pub fn len(&self) -> usize {
        match &self.mode {
            Mode::Record(e) => e.borrow().len(),
            Mode::Replay { entries, .. } => entries.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Replay: the queries not asked yet.
    pub fn remaining(&self) -> usize {
        match &self.mode {
            Mode::Record(_) => 0,
            Mode::Replay { entries, next, .. } => entries.len() - next.get(),
        }
    }

    /// The recording, `header` lines first, each prefixed `# `.
    pub fn text(&self, header: &[String]) -> String {
        let Mode::Record(entries) = &self.mode else {
            panic!("trace tape: only a recording is written out");
        };
        let mut out = String::new();
        for h in header {
            let _ = writeln!(out, "# {h}");
        }
        let entries = entries.borrow();
        let mut seen: HashMap<&Entry, usize> = HashMap::new();
        let mut last: HashMap<&str, Vec<&str>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            let mut toks = e.query.split(' ');
            let kind = toks.next().unwrap_or_default();
            let toks: Vec<&str> = toks.collect();
            let prev = last.insert(kind, toks.clone()).unwrap_or_default();
            if let Some(j) = seen.insert(e, i) {
                let _ = writeln!(out, "^{}", i - j);
                continue;
            }
            let mut short: Vec<&str> = toks
                .iter()
                .enumerate()
                .map(|(i, t)| if prev.get(i) == Some(t) { "." } else { t })
                .collect();
            assert!(
                prev.is_empty() || prev.len() == toks.len(),
                "trace tape: `{kind}` queries of {} and {} tokens",
                prev.len(),
                toks.len()
            );
            while short.last() == Some(&".") {
                short.pop();
            }
            out.push_str(kind);
            for t in short {
                out.push(' ');
                out.push_str(t);
            }
            let _ = writeln!(out, " = {}", e.answer);
        }
        out
    }

    /// The answer to `query`: `live()`'s, written down by `encode`, when
    /// recording; the tape's, read back by `decode`, when replaying.
    pub fn answer<T>(
        &self,
        query: String,
        live: impl FnOnce() -> T,
        encode: impl FnOnce(&T) -> String,
        decode: impl FnOnce(&str) -> Option<T>,
    ) -> T {
        match &self.mode {
            Mode::Record(entries) => {
                let a = live();
                let answer = encode(&a);
                entries.borrow_mut().push(Entry { query, answer });
                a
            }
            Mode::Replay {
                entries,
                lines,
                next,
            } => {
                let i = next.get();
                let Some(e) = entries.get(i) else {
                    panic!(
                        "trace tape: query {} past the tape's {} asked `{query}`",
                        i + 1,
                        entries.len()
                    );
                };
                if e.query != query {
                    panic!(
                        "trace tape: query {} (line {}) asked `{query}`, the tape holds `{}`",
                        i + 1,
                        lines[i],
                        e.query
                    );
                }
                next.set(i + 1);
                decode(&e.answer).unwrap_or_else(|| {
                    panic!(
                        "trace tape line {}: unreadable answer `{}`",
                        lines[i], e.answer
                    )
                })
            }
        }
    }
}

/// Writes ` v` so that `str::parse` gives back the same bits.
pub fn push_f32(out: &mut String, v: f32) {
    let _ = write!(out, " {v}");
}

pub fn push_vec3(out: &mut String, v: glam::Vec3) {
    push_f32(out, v.x);
    push_f32(out, v.y);
    push_f32(out, v.z);
}

/// CoD1's seam: [`crate::movetrace::MoveWorld`]'s box traces and point
/// contents.
///
/// - `t sx sy sz ex ey ez mins maxs mask pass = fraction endpos normal
///   surfaceflags flags entity`, where `flags` is `s` (startsolid), `a`
///   (allsolid), `b` (stopped on a body) and `w` (on water, a mask with
///   `CONTENTS_WATER`), `-` for none, and a trace
///   that reached `end` untouched answers `1`.
/// - `c x y z = contents`.
///
/// Masks, surface flags and contents are hex; `pass` and the entity are
/// decimal, `w` for the world and `n` for none.
pub mod cod1 {
    use super::{Tape, push_f32 as f, push_vec3 as v3};
    use crate::collision::{CONTENTS_WATER, Prim, Trace};
    use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
    use glam::Vec3;
    use std::fmt::Write as _;

    fn ent(out: &mut String, e: u32) {
        match e {
            ENTITYNUM_WORLD => out.push_str(" w"),
            ENTITYNUM_NONE => out.push_str(" n"),
            u32::MAX => out.push_str(" -"),
            e => {
                let _ = write!(out, " {e}");
            }
        }
    }

    fn read_ent(s: &str) -> Option<u32> {
        match s {
            "w" => Some(ENTITYNUM_WORLD),
            "n" => Some(ENTITYNUM_NONE),
            "-" => Some(u32::MAX),
            s => s.parse().ok(),
        }
    }

    /// A box trace's answer, taped. `live` traces the real world, and
    /// `entity_num` names what a live trace stopped on.
    #[expect(clippy::too_many_arguments)]
    pub fn trace(
        tape: &Tape,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        pass: u32,
        live: impl FnOnce() -> Trace,
        entity_num: impl FnOnce(&Trace) -> u32,
    ) -> Trace {
        let mut q = String::from("t");
        v3(&mut q, start);
        v3(&mut q, end);
        v3(&mut q, mins);
        v3(&mut q, maxs);
        let _ = write!(q, " {mask:x}");
        ent(&mut q, pass);
        let live = || {
            let t = live();
            let e = entity_num(&t);
            (t, e)
        };
        tape.answer(
            q,
            live,
            |(t, e)| encode(t, start, end, *e),
            |a| decode(a, start, end).map(|t| (t, 0)),
        )
        .0
    }

    fn clear(t: &Trace, end: Vec3, entity: u32) -> bool {
        t.fraction == 1.0
            && t.endpos.to_array().map(f32::to_bits) == end.to_array().map(f32::to_bits)
            && t.normal.to_array().map(f32::to_bits) == [0; 3]
            && t.surface_flags == 0
            && !t.startsolid
            && !t.allsolid
            && entity == ENTITYNUM_NONE
    }

    /// `SV_Trace`'s end point off the fraction (`MoveWorld::box_trace`).
    fn lerp(start: Vec3, end: Vec3, fraction: f32) -> Vec3 {
        let f = f64::from(fraction);
        let l = |s: f32, e: f32| (f64::from(s) + (f64::from(e) - f64::from(s)) * f) as f32;
        Vec3::new(l(start.x, end.x), l(start.y, end.y), l(start.z, end.z))
    }

    fn same(a: Vec3, b: Vec3) -> bool {
        a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits)
    }

    /// The end point is `e` when it is `end` and `~` when it is the lerp's.
    fn encode(t: &Trace, start: Vec3, end: Vec3, entity: u32) -> String {
        if clear(t, end, entity) {
            return "1".into();
        }
        let mut a = String::new();
        f(&mut a, t.fraction);
        if same(t.endpos, end) {
            a.push_str(" e");
        } else if same(t.endpos, lerp(start, end, t.fraction)) {
            a.push_str(" ~");
        } else {
            v3(&mut a, t.endpos);
        }
        v3(&mut a, t.normal);
        let _ = write!(a, " {:x} ", t.surface_flags);
        let body = matches!(t.hit, Some(Prim::Body(_)));
        let water = t.hit == Some(Prim::Clip(CONTENTS_WATER));
        for (on, c) in [
            (t.startsolid, 's'),
            (t.allsolid, 'a'),
            (body, 'b'),
            (water, 'w'),
        ] {
            if on {
                a.push(c);
            }
        }
        if !(t.startsolid || t.allsolid || body || water) {
            a.push('-');
        }
        ent(&mut a, entity);
        a.trim_start().to_owned()
    }

    /// The trace back off the tape. Its `hit` is what makes
    /// `CollisionWorld::entity_num` name the recorded entity on any world,
    /// and keeps a body hit a body hit for `MoveWorld::pm_trace`.
    fn decode(a: &str, start: Vec3, end: Vec3) -> Option<Trace> {
        if a == "1" {
            return Some(Trace::clear(end));
        }
        let tok: Vec<&str> = a.split(' ').collect();
        let p = |s: &str| s.parse::<f32>().ok();
        let mut t = Trace::clear(end);
        t.fraction = p(tok.first()?)?;
        let rest = match tok[1..] {
            ["e", ref rest @ ..] => rest,
            ["~", ref rest @ ..] => {
                t.endpos = lerp(start, end, t.fraction);
                rest
            }
            [x, y, z, ref rest @ ..] => {
                t.endpos = Vec3::new(p(x)?, p(y)?, p(z)?);
                rest
            }
            _ => return None,
        };
        let [nx, ny, nz, sf, flags, e] = *rest else {
            return None;
        };
        t.normal = Vec3::new(p(nx)?, p(ny)?, p(nz)?);
        t.surface_flags = u32::from_str_radix(sf, 16).ok()?;
        t.startsolid = flags.contains('s');
        t.allsolid = flags.contains('a');
        let entity = read_ent(e)?;
        let none = match entity {
            ENTITYNUM_WORLD => t.fraction < 1.0,
            ENTITYNUM_NONE => t.fraction >= 1.0,
            _ => false,
        };
        t.hit = if flags.contains('w') {
            Some(Prim::Clip(CONTENTS_WATER))
        } else {
            (flags.contains('b') || !none).then_some(Prim::Body(entity))
        };
        Some(t)
    }

    /// A point contents query, taped.
    pub fn contents(tape: &Tape, p: Vec3, live: impl FnOnce() -> u32) -> u32 {
        let mut q = String::from("c");
        v3(&mut q, p);
        tape.answer(
            q,
            live,
            |c| format!("{c:x}"),
            |a| u32::from_str_radix(a, 16).ok(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{Prim, Trace};
    use glam::Vec3;

    fn hit() -> Trace {
        let mut t = Trace::clear(Vec3::new(1.0, 2.0, 3.0));
        t.fraction = 0.1 + 0.2;
        t.endpos = Vec3::new(-0.0, 1e-30, 123456.79);
        t.normal = Vec3::new(0.0, 0.0, 1.0);
        t.surface_flags = 0x4000;
        t.startsolid = true;
        t.hit = Some(Prim::Body(5));
        t
    }

    fn ask(tape: &Tape, start: Vec3, live: Trace) -> Trace {
        let end = Vec3::new(1.0, 2.0, 3.0);
        let e = if matches!(live.hit, Some(Prim::Body(_))) {
            5
        } else {
            crate::net::protocol::ENTITYNUM_NONE
        };
        cod1::trace(
            tape,
            start,
            end,
            Vec3::splat(-15.0),
            Vec3::splat(15.0),
            0x2810011,
            3,
            || live,
            |_| e,
        )
    }

    /// The answers come back to the bit, and a replay never runs the live
    /// world.
    #[test]
    fn a_replay_gives_back_what_was_recorded() {
        let end = Vec3::new(1.0, 2.0, 3.0);
        let mut lerped = hit();
        lerped.endpos = Vec3::X.lerp(end, lerped.fraction);
        let mut at_end = hit();
        at_end.endpos = end;
        let mut water = hit();
        water.startsolid = false;
        water.hit = Some(Prim::Clip(crate::collision::CONTENTS_WATER));
        let rec = Tape::record();
        let asked = [
            (Vec3::ZERO, hit()),
            (Vec3::X, Trace::clear(end)),
            (Vec3::X, lerped),
            (Vec3::ZERO, hit()),
            (Vec3::Y, at_end),
            (Vec3::Z, water),
        ];
        let want: Vec<Trace> = asked.iter().map(|&(s, t)| ask(&rec, s, t)).collect();
        let c = cod1::contents(&rec, Vec3::Y, || 0x10001);
        let text = rec.text(&["test".into()]);
        assert!(text.contains("\n^3\n") && text.contains(" ~ ") && text.contains(" e "));
        let play = Tape::replay(&text);
        let poison = Trace::clear(Vec3::splat(9.0));
        let got: Vec<Trace> = asked.iter().map(|&(s, _)| ask(&play, s, poison)).collect();
        assert_eq!(cod1::contents(&play, Vec3::Y, || 0), c);
        assert_eq!(play.remaining(), 0);
        for (x, y) in want.into_iter().zip(got) {
            assert_eq!(x.fraction.to_bits(), y.fraction.to_bits());
            assert_eq!(
                x.endpos.to_array().map(f32::to_bits),
                y.endpos.to_array().map(f32::to_bits)
            );
            assert_eq!(x.normal, y.normal);
            assert_eq!(x.surface_flags, y.surface_flags);
            assert_eq!((x.startsolid, x.allsolid), (y.startsolid, y.allsolid));
            assert_eq!(x.hit, y.hit);
        }
    }

    /// A query that is not the next one recorded stops the replay there.
    #[test]
    #[should_panic(expected = "query 2 (line 3) asked")]
    fn a_replay_refuses_a_query_it_did_not_record() {
        let rec = Tape::record();
        ask(&rec, Vec3::ZERO, hit());
        ask(&rec, Vec3::ZERO, hit());
        let play = Tape::replay(&rec.text(&["test".into()]));
        ask(&play, Vec3::ZERO, hit());
        ask(&play, Vec3::new(0.0, 0.0, 1e-6), hit());
    }
}
