//! [`PmWorld`] on a trace tape (`vcod_common::tracetape`): every trace
//! pmove asks, recorded off a live world or answered from the tape.
//!
//! `t sx sy sz ex ey ez mins maxs pass mask = fraction normal surfaceflags
//! contents flags entity`, `flags` from `s` (startsolid), `a` (allsolid)
//! and `w` (walkable), `-` for none; a trace that met nothing answers `1`.
//! Masks, flags and contents are hex, entities decimal.

use glam::Vec3;
use std::fmt::Write as _;
use vcod_common::tracetape::{Tape, push_f32, push_vec3};

use super::{PmWorld, Tr};

/// The live world when recording, none when replaying.
pub struct Taped<'a> {
    pub tape: &'a Tape,
    pub live: Option<&'a dyn PmWorld>,
}

impl PmWorld for Taped<'_> {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, pass: u32, mask: u32) -> Tr {
        let mut q = String::from("t");
        for v in [start, end, mins, maxs] {
            push_vec3(&mut q, v);
        }
        let _ = write!(q, " {pass} {mask:x}");
        self.tape.answer(
            q,
            || {
                self.live
                    .expect("a recording tape needs the live world")
                    .trace(start, end, mins, maxs, pass, mask)
            },
            encode,
            decode,
        )
    }
}

fn encode(t: &Tr) -> String {
    let bits = |t: &Tr| (t.fraction.to_bits(), t.normal.to_array().map(f32::to_bits));
    if *t == Tr::default() && bits(t) == bits(&Tr::default()) {
        return "1".into();
    }
    let mut a = String::new();
    push_f32(&mut a, t.fraction);
    push_vec3(&mut a, t.normal);
    let _ = write!(a, " {:x} {:x} ", t.surface_flags, t.contents);
    for (on, c) in [(t.startsolid, 's'), (t.allsolid, 'a'), (t.walkable, 'w')] {
        if on {
            a.push(c);
        }
    }
    if !(t.startsolid || t.allsolid || t.walkable) {
        a.push('-');
    }
    let _ = write!(a, " {}", t.entity_num);
    a.trim_start().to_owned()
}

fn decode(a: &str) -> Option<Tr> {
    if a == "1" {
        return Some(Tr::default());
    }
    let tok: Vec<&str> = a.split(' ').collect();
    let [fr, nx, ny, nz, sf, c, flags, e] = tok[..] else {
        return None;
    };
    let p = |s: &str| s.parse::<f32>().ok();
    Some(Tr {
        fraction: p(fr)?,
        normal: Vec3::new(p(nx)?, p(ny)?, p(nz)?),
        surface_flags: u32::from_str_radix(sf, 16).ok()?,
        contents: u32::from_str_radix(c, 16).ok()?,
        entity_num: e.parse().ok()?,
        startsolid: flags.contains('s'),
        allsolid: flags.contains('a'),
        walkable: flags.contains('w'),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmove::testing::FLOOR;

    /// A floor's answers come back to the bit with no world behind them.
    #[test]
    fn a_taped_floor_answers_as_the_floor() {
        let asks = [
            (Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.3, 0.1, -5.0)),
            (Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 0.0, -2.0)),
            (Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, 20.0)),
        ];
        let rec = Tape::record();
        let live = Taped {
            tape: &rec,
            live: Some(&FLOOR),
        };
        let want: Vec<Tr> = asks
            .iter()
            .map(|&(s, e)| live.trace(s, e, Vec3::splat(-15.0), Vec3::splat(15.0), 3, 0x2810011))
            .collect();
        let play = Tape::replay(&rec.text(&[]));
        let taped = Taped {
            tape: &play,
            live: None,
        };
        for (&(s, e), w) in asks.iter().zip(&want) {
            let t = taped.trace(s, e, Vec3::splat(-15.0), Vec3::splat(15.0), 3, 0x2810011);
            assert_eq!(t.fraction.to_bits(), w.fraction.to_bits());
            assert_eq!(t, *w);
        }
    }
}
