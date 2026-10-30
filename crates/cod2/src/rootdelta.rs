//! A clip's root motion between two node times, as `XAnimCalcDelta`
//! (lnxded 0x80c1504) hands it to `G_RunCorpseMove`: the leaf's
//! `XAnimCalcRelDeltaParts` (0x80bde02) over the delta part's keys, and the
//! node time a server frame steps (0x80beb06). cod2-combat.md 6.2.
//!
//! Each value is stored as a float where the binary stores it; a sum of
//! products that stays on the x87 stack is an `f64` here.

use glam::Vec3;
use vcod_common::xanim::{Track, XAnim};

/// The parts' `+0x8`: frames a second over the clip's frame span, stored
/// (`XAnimLoadFile` 0x80c3f8a); 0 for a single frame.
pub fn frequency(anim: &XAnim) -> f32 {
    match anim.frame_count - 1 {
        0 => 0.0,
        n => anim.framerate / n as f32,
    }
}

/// One server frame of a leaf's node time (0x80beb06): `dtime * rate *
/// frequency` stored, added to the time and stored; a looping clip wraps
/// by 1, any other stops at 1, and a step that leaves it at or under 0
/// is dropped. `dtime` is `0.001 * level.frametime`, stored.
pub fn step_time(t: f32, dtime: f32, rate: f32, frequency: f32, looping: bool) -> f32 {
    let inc = (f64::from(dtime) * f64::from(rate) * f64::from(frequency)) as f32;
    if inc == 0.0 {
        return t;
    }
    let mut next = t + inc;
    if next >= 1.0 {
        if looping {
            while next >= 1.0 {
                next -= 1.0;
            }
        } else {
            next = 1.0;
        }
    }
    if t - next > 0.0 && !looping {
        return t;
    }
    next
}

/// The key a frame falls on or after and how far toward the next
/// (0x80bd0a6, 0x80bd876/0x80bda1c): with every frame keyed the frame
/// itself, else a search of the frame list.
fn key_at(frames: &[u16], numframes: u32, t: f32) -> (usize, f32) {
    let pos = (f64::from(t) * f64::from(numframes as f32)) as f32;
    let frame = pos as i32;
    if frames.len() as u32 > numframes {
        return (frame as usize, pos - frame as f32);
    }
    let i = frames
        .partition_point(|&f| i32::from(f) <= frame)
        .saturating_sub(1)
        .min(frames.len() - 2);
    let (a, b) = (frames[i], frames[i + 1]);
    let frac = ((f64::from(pos) - f64::from(a)) / f64::from(i32::from(b) - i32::from(a))) as f32;
    (i, frac)
}

/// The delta part's yaw (`z`, `w` in short units) and translation at node
/// time `t` (0x80bdc5a): the last key at 1 or on a single-frame clip, the
/// two keys around the frame lerped otherwise; no keys read (0, 32767)
/// and zero.
fn sample(root: &Track, numframes: u32, t: f32) -> ([f32; 2], Vec3) {
    let rot = |i: usize| {
        let k = root.rot_raw[i];
        [f32::from(k[2]), f32::from(k[3])]
    };
    let last = t == 1.0 || numframes == 0;
    let q = match root.rot_raw.len() {
        0 => [0.0, 32767.0],
        1 => rot(0),
        n if last => rot(n - 1),
        _ => {
            let frames: Vec<u16> = root.rot_keys.iter().map(|k| k.0).collect();
            let (i, f) = key_at(&frames, numframes, t);
            let (a, b) = (rot(i), rot(i + 1));
            // 0x80c3cb0: `(b - a) * frac + a` off the shorts.
            let lerp = |a: f32, b: f32| (f64::from(b - a) * f64::from(f) + f64::from(a)) as f32;
            [lerp(a[0], b[0]), lerp(a[1], b[1])]
        }
    };
    let trans = match root.trans_keys.len() {
        0 => Vec3::ZERO,
        1 => root.trans_keys[0].1,
        n if last => root.trans_keys[n - 1].1,
        _ => {
            let frames: Vec<u16> = root.trans_keys.iter().map(|k| k.0).collect();
            let (i, f) = key_at(&frames, numframes, t);
            let (a, b) = (root.trans_keys[i].1, root.trans_keys[i + 1].1);
            // 0x80c36fe: `a + (b - a) * frac`, one store.
            (a.as_dvec3() + (b.as_dvec3() - a.as_dvec3()) * f64::from(f)).as_vec3()
        }
    };
    (q, trans)
}

/// How far the clip's root moved from node time `old` to `new`, in the
/// clip's frame as it stands at `new`: `XAnimCalcDelta` over a tree whose
/// one weighted leaf is `anim` (its weight 1 adds nothing). A looping clip
/// that wrapped adds its whole travel (0x80bde02); the move is turned back
/// by the root's yaw at `new` (0x80bdd54). Zero without a root track.
pub fn rel_delta(anim: &XAnim, old: f32, new: f32) -> Vec3 {
    let Some(root) = &anim.root else {
        return Vec3::ZERO;
    };
    let numframes = anim.frame_count - 1;
    let (_, t0) = sample(root, numframes, old);
    let (q1, mut t1) = sample(root, numframes, new);
    if anim.looping
        && new < old
        && let (Some(first), Some(last)) = (root.trans_keys.first(), root.trans_keys.last())
    {
        t1 = (t1 + last.1) - first.1;
    }
    // 0x80bdd54: turned back by the yaw at `new`.
    vcod_common::xanim::unturn_by_yaw(t1 - t0, q1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    fn clip(trans: Vec<(u16, Vec3)>, yaw_z: Vec<(u16, i16)>, frames: u32) -> XAnim {
        XAnim {
            name: "t".into(),
            frame_count: frames,
            framerate: 30.0,
            looping: false,
            tracks: Vec::new(),
            root: Some(Track {
                bone: String::new(),
                rot_keys: yaw_z.iter().map(|&(f, _)| (f, Quat::IDENTITY)).collect(),
                rot_raw: yaw_z
                    .iter()
                    .map(|&(_, z)| vcod_common::xanim::raw_key(0, 0, z))
                    .collect(),
                trans_keys: trans,
            }),
            notes: Vec::new(),
        }
    }

    #[test]
    fn the_delta_is_the_travel_between_the_two_times() {
        let a = clip(
            vec![(0, Vec3::ZERO), (10, Vec3::new(10.0, 0.0, 0.0))],
            vec![],
            11,
        );
        let d = rel_delta(&a, 0.0, 0.5);
        assert!((d - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5, "{d}");
    }

    #[test]
    fn the_delta_turns_back_by_the_roots_yaw() {
        // A root turned 90 degrees (z = sin 45) reads a move along +y as +x.
        let a = clip(
            vec![(0, Vec3::ZERO), (10, Vec3::new(0.0, 10.0, 0.0))],
            vec![(0, 23170)],
            11,
        );
        let d = rel_delta(&a, 0.0, 1.0);
        assert!((d - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-3, "{d}");
    }

    #[test]
    fn a_step_stops_a_clip_at_its_end() {
        assert_eq!(step_time(0.95, 0.05, 1.0, 3.0, false), 1.0);
        assert_eq!(step_time(1.0, 0.05, 1.0, 3.0, false), 1.0);
    }

    /// The move over a grid of time pairs under a turning track, bit for
    /// bit.
    #[test]
    fn pins_the_rel_delta() {
        let mut a = clip(
            vec![
                (0, Vec3::new(1.0, -2.0, 0.5)),
                (4, Vec3::new(7.25, 3.5, -1.0)),
                (10, Vec3::new(-4.0, 12.0, 2.0)),
            ],
            vec![(0, 0), (3, 5000), (10, 23170)],
            11,
        );
        let times = [0.0, 0.05, 0.3, 0.41, 0.77, 0.999, 1.0];
        let mut out = Vec::new();
        for looping in [false, true] {
            a.looping = looping;
            for &t0 in &times {
                for &t1 in &times {
                    out.extend(rel_delta(&a, t0, t1).to_array().map(f32::to_bits));
                }
            }
        }
        let h = out.into_iter().fold(0xcbf2_9ce4_8422_2325u64, |h, w| {
            (h ^ u64::from(w)).wrapping_mul(0x100_0000_01b3)
        });
        assert_eq!(h, 2323033017023889879);
    }
}
