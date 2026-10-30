//! CoD1 xanim v14 parsing and sampling. Layout: `docs/research/xanim-v14-format.md`;
//! header flags: `docs/research/player-model-anim-system.md`, "xanim v14 header flags".

use crate::asset_source::{Asset, AssetKind, AssetSource};
use crate::xmodel::Reader;
use anyhow::{Result, anyhow, ensure};
use glam::{Quat, Vec3};
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct XAnim {
    #[allow(dead_code)] // diagnostics only
    pub name: String,
    /// Key positions, so key frames are `0..frame_count`. A looping clip's
    /// loop-closing repeat of frame 0 is counted, so `duration()` is the loop period.
    pub frame_count: u32,
    pub framerate: f32,
    /// Header flag 0x1.
    pub looping: bool,
    pub tracks: Vec<Track>,
    /// Header flag 0x2's nameless root-motion track, yaw-only rotations.
    pub root: Option<Track>,
    /// Notetracks, no runtime consumer yet.
    pub notes: Vec<(String, u16)>,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub bone: String,
    pub rot_keys: Vec<(u16, Quat)>, // (frame, local rot), ascending; empty = hold current
    pub trans_keys: Vec<(u16, Vec3)>, // (frame, offset from the bind local pos)
    /// `rot_keys` as the engine keeps them: the four shorts, `w` rebuilt and
    /// the sign the loader gave each key. Empty for a hand-built track.
    pub rot_raw: Vec<[i16; 4]>,
}

/// A key as `cod_lnxded`'s loader rebuilds it (0x080c2598, 0x080c264c): `w`
/// is `floor(sqrt(32767^2 - x^2 - y^2 - z^2) + 0.5)` in the same 1/32767
/// units, or 0 when that is not positive. A near-half-turn key, `w` a few
/// units, turns up to a degree off a float rebuild (xanim-v14-format.md).
fn dequant(x: i16, y: i16, z: i16) -> Quat {
    let [x, y, z, w] = raw_key(x, y, z).map(f32::from);
    Quat::from_xyzw(x, y, z, w).normalize()
}

/// A key's four shorts with `w` rebuilt as [`dequant`] rebuilds it.
pub fn raw_key(x: i16, y: i16, z: i16) -> [i16; 4] {
    let (xi, yi, zi) = (x as i64, y as i64, z as i64);
    let r = 32767 * 32767 - (xi * xi + yi * yi + zi * zi);
    let w = if r < 1 {
        0
    } else {
        ((r as f64).sqrt() + 0.5).floor() as i16
    };
    [x, y, z, w]
}

/// What the fixed-size header carries. `frame_count` already counts a looping
/// clip's loop-closing repeat of frame 0.
struct Header {
    frame_count: u32,
    bone_count: usize,
    looping: bool,
    /// Flag 0x2: the clip carries a nameless root-motion track.
    root_motion: bool,
    framerate: f32,
}

fn read_header(name: &str, r: &mut Reader) -> Result<Header> {
    let version = r.u16()?;
    ensure!(
        version == 14,
        "{name}: unsupported xanim version {version}, expected 14"
    );
    let header_frames = r.u16()? as u32;
    ensure!(header_frames >= 1, "{name}: zero frames");
    let bone_count = r.u16()? as usize;
    let flags = r.u8()?;
    ensure!(flags <= 3, "{name}: unsupported xanim flags {flags:#x}");
    let looping = flags & 1 != 0;
    let framerate = r.u16()? as f32;
    ensure!(framerate > 0.0, "{name}: zero framerate");
    Ok(Header {
        frame_count: header_frames + looping as u32,
        bone_count,
        looping,
        root_motion: flags & 2 != 0,
        framerate,
    })
}

/// A root move put into the clip's own frame: turned back by the root's
/// yaw, given as a half quaternion's unnormalized `(z, w)`; a zero
/// quaternion leaves it. The sums group as CoD2's do (lnxded 0x80bdd54,
/// cod2-combat.md 6.2), which `cod2::wire_ab`'s kill-run corpses match bit
/// for bit; the CoD4 corpse takes the same.
pub fn unturn_by_yaw(v: Vec3, [z, w]: [f32; 2]) -> Vec3 {
    let len2 = w * w + z * z;
    if len2 == 0.0 {
        return v;
    }
    let scale = 2.0 / len2;
    // The turn's sine and 1 - cosine.
    let sin = scale * (w * z);
    let versin = scale * (z * z);
    Vec3::new(
        v.x * (1.0 - versin) + v.y * sin,
        v.y - (v.x * sin + v.y * versin),
        v.z,
    )
}

/// The clip's own length in ms, read from the header alone: a caller that only
/// needs the timing does not pay for the tracks. The engine's
/// `XAnimGetLength` syscall (`cod_lnxded` 0x08088b3d..0x08088b5b): frames
/// over the float framerate times 1000.0 on the x87 stack, stored under a
/// 0xc00 control word, so truncated (player-model-anim-system.md, "The
/// weapon channel").
pub fn duration_ms(name: &str, data: &[u8]) -> Result<u32> {
    let h = read_header(name, &mut Reader::new(data))?;
    Ok(x87_length_ms(h.frame_count - 1, h.framerate as u32))
}

/// `(int)(frames / fps * 1000.0)` in 80-bit extended precision, rounding
/// each step to a 64-bit mantissa (nearest even) and truncating the store.
fn x87_length_ms(frames: u32, fps: u32) -> u32 {
    if frames == 0 || fps == 0 {
        return 0;
    }
    // Rounds `m * 2^-e` (m > 0) to a 64-bit mantissa.
    fn round64(m: u128, e: i32) -> (u128, i32) {
        let bits = 128 - m.leading_zeros() as i32;
        if bits <= 64 {
            return (m, e);
        }
        let sh = bits - 64;
        let (mut q, rem, half) = (m >> sh, m & ((1 << sh) - 1), 1u128 << (sh - 1));
        if rem > half || (rem == half && q & 1 == 1) {
            q += 1;
        }
        (q, e - sh)
    }
    // The quotient, with enough bits below the mantissa to round it.
    let shift = 100;
    let num = u128::from(frames) << shift;
    let (mut q, rem) = (num / u128::from(fps), num % u128::from(fps));
    // A sticky bit, so an inexact tie rounds up.
    if rem != 0 {
        q = (q << 1) | 1;
    } else {
        q <<= 1;
    }
    let (q, e) = round64(q, shift + 1);
    let (p, e) = round64(q * 1000, e);
    (if e >= 0 { p >> e } else { p << -e }) as u32
}

/// A track's key frame list: an explicit index list only when
/// `1 < count < frame_count`, u8 unless a clip's frames overflow a byte.
fn frames_of(name: &str, r: &mut Reader, count: u32, frame_count: u32) -> Result<Vec<u16>> {
    if 1 < count && count < frame_count {
        let index_u16 = frame_count > 256;
        let frames: Vec<u16> = (0..count)
            .map(|_| {
                if index_u16 {
                    r.u16()
                } else {
                    Ok(r.u8()? as u16)
                }
            })
            .collect::<Result<_>>()?;
        // interp subtracts neighbouring frames; every retail list ascends
        ensure!(
            frames.windows(2).all(|w| w[0] < w[1]),
            "{name}: frame index list is not ascending"
        );
        Ok(frames)
    } else {
        Ok((0..count.min(frame_count) as u16).collect())
    }
}

/// Header flag 0x2's root-motion track, which sits right after the header.
fn read_root(name: &str, r: &mut Reader, frame_count: u32) -> Result<Track> {
    let rc = r.u16()? as u32;
    ensure!(rc <= frame_count, "{name}: delta rot {rc} > {frame_count}");
    let mut rot_keys = Vec::with_capacity(rc as usize);
    let mut rot_raw: Vec<[i16; 4]> = Vec::with_capacity(rc as usize);
    for f in frames_of(name, r, rc, frame_count)? {
        let raw = raw_key(0, 0, r.i16()?);
        rot_keys.push((f, dequant(0, 0, raw[2])));
        rot_raw.push(raw);
    }
    // CoD2's loader (lnxded `XAnimLoadFile` 0x80c3f8a) negates a key whose
    // (z, w) turns from its predecessor's; only the shorts take it.
    for k in 1..rot_raw.len() {
        let (a, b) = (rot_raw[k - 1], rot_raw[k]);
        if i32::from(a[2]) * i32::from(b[2]) + i32::from(a[3]) * i32::from(b[3]) < 0 {
            rot_raw[k] = b.map(i16::wrapping_neg);
        }
    }
    let tc = r.u16()? as u32;
    ensure!(
        tc <= frame_count,
        "{name}: delta trans {tc} > {frame_count}"
    );
    let trans_keys = frames_of(name, r, tc, frame_count)?
        .into_iter()
        .map(|f| Ok((f, Vec3::new(r.f32()?, r.f32()?, r.f32()?))))
        .collect::<Result<Vec<_>>>()?;
    Ok(Track {
        bone: String::new(),
        rot_keys,
        trans_keys,
        rot_raw,
    })
}

/// What a clip's header and root track say about it as a movement anim:
/// `BG_FinalizePlayerAnims` reads the same three things off the engine.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RootMotion {
    /// `XAnimGetLength`: the loop period in whole ms, truncated.
    pub length_ms: i32,
    /// Seconds from first to last key, [`XAnim::duration`].
    pub duration: f32,
    pub looping: bool,
    /// The parts' `+0x8`: the clip's time fraction a second, the frame rate
    /// over `numframes` (`frame_count - 1`), 0 for a single frame (UO
    /// `coduo_lnxded` loader 0x80b816e, the division at 0x80b83e9).
    pub frequency: f32,
    /// How far the root moves over the whole clip, `XAnimGetRelDelta` from
    /// time 0 to 1; 0 without a root track.
    pub distance: f32,
}

/// [`RootMotion`] off the header and root track alone, without the bones.
pub fn root_motion(name: &str, data: &[u8]) -> Result<RootMotion> {
    let mut r = Reader::new(data);
    let h = read_header(name, &mut r)?;
    let duration = (h.frame_count - 1) as f32 / h.framerate;
    let distance = if h.root_motion {
        let root = read_root(name, &mut r, h.frame_count)?;
        let last = (h.frame_count - 1) as f32;
        match (root.sample(0.0).0, root.sample(last).0) {
            (Some(a), Some(b)) => (b - a).length(),
            _ => 0.0,
        }
    } else {
        0.0
    };
    Ok(RootMotion {
        // `cod_lnxded` 0x80bed5c: frames over framerate, times 1000.0 under a
        // truncating control word (syscall 0x5d's case, 0x8088b32; 1.5b's
        // and UO's syscall 0x61, 0x808f584 and 0x8090396).
        length_ms: x87_length_ms(h.frame_count - 1, h.framerate as u32) as i32,
        duration,
        looping: h.looping,
        frequency: match h.frame_count - 1 {
            0 => 0.0,
            n => h.framerate / n as f32,
        },
        distance,
    })
}

pub fn parse(name: &str, data: &[u8]) -> Result<XAnim> {
    let mut r = Reader::new(data);
    let Header {
        frame_count,
        bone_count,
        looping,
        root_motion,
        framerate,
    } = read_header(name, &mut r)?;
    let root = if root_motion {
        Some(read_root(name, &mut r, frame_count)?)
    } else {
        None
    };
    let nb = bone_count.div_ceil(8);
    // Bitset A: negate the track's first rotation key; bitset B: z-only keys
    // (cod_lnxded 0x080b7160, xanim-v14-format.md, "Rotation keys as the
    // engine rebuilds them").
    let flip = r.take(nb)?.to_vec();
    let simple = r.take(nb)?.to_vec();
    let is_flipped = |i: usize| flip[i / 8] >> (i % 8) & 1 == 1;
    let is_simple = |i: usize| simple[i / 8] >> (i % 8) & 1 == 1;
    let mut names = Vec::with_capacity(bone_count);
    for _ in 0..bone_count {
        names.push(r.cstr()?);
    }
    let mut tracks = Vec::with_capacity(bone_count);
    for (i, bone) in names.into_iter().enumerate() {
        let rc = r.u16()? as u32;
        ensure!(
            rc <= frame_count,
            "{name}/{bone}: {rc} rot keys > {frame_count} frames"
        );
        let rframes = frames_of(name, &mut r, rc, frame_count)?;
        let mut rot_keys = Vec::with_capacity(rc as usize);
        let mut rot_raw = Vec::with_capacity(rc as usize);
        for f in rframes {
            let raw = if is_simple(i) {
                raw_key(0, 0, r.i16()?)
            } else {
                let (x, y, z) = (r.i16()?, r.i16()?, r.i16()?);
                raw_key(x, y, z)
            };
            rot_keys.push((f, dequant(raw[0], raw[1], raw[2])));
            rot_raw.push(raw);
        }
        // The loader's sign: the flagged first key negated, then each key
        // put on its predecessor's hemisphere, so the track's keys run
        // continuously from a sign the file chose. The sampler sums them
        // raw, so the sign shows wherever two tracks blend.
        let negate = |k: [i16; 4]| k.map(|c| c.wrapping_neg());
        if is_flipped(i)
            && let Some((_, q)) = rot_keys.first_mut()
        {
            *q = -*q;
            rot_raw[0] = negate(rot_raw[0]);
        }
        for k in 1..rot_keys.len() {
            if rot_keys[k].1.dot(rot_keys[k - 1].1) < 0.0 {
                rot_keys[k].1 = -rot_keys[k].1;
                rot_raw[k] = negate(rot_raw[k]);
            }
        }
        let tc = r.u16()? as u32;
        ensure!(
            tc <= frame_count,
            "{name}/{bone}: {tc} trans keys > {frame_count} frames"
        );
        let tframes = frames_of(name, &mut r, tc, frame_count)?;
        let mut trans_keys = Vec::with_capacity(tc as usize);
        for f in tframes {
            trans_keys.push((f, Vec3::new(r.f32()?, r.f32()?, r.f32()?)));
        }
        tracks.push(Track {
            bone,
            rot_keys,
            trans_keys,
            rot_raw,
        });
    }
    let note_count = r.u8()?;
    let mut notes = Vec::with_capacity(note_count as usize);
    for _ in 0..note_count {
        let n = r.cstr()?;
        notes.push((n, r.u16()?));
    }
    // No length prefixes: landing exactly on EOF is the proof of a correct decode.
    let trailing = r.remaining();
    ensure!(
        trailing == 0,
        "{name}: {trailing} trailing bytes after decode"
    );
    Ok(XAnim {
        name: name.to_string(),
        frame_count,
        framerate,
        looping,
        tracks,
        root,
        notes,
    })
}

/// [`RootMotion`] of clip `name` in `fs`: its file's header and root track,
/// or a decoded clip's ([`AssetSource::decoded_anim`]). `None` where the
/// source has no such clip.
pub fn clip_motion(fs: &dyn AssetSource, name: &str) -> Option<RootMotion> {
    if let Some(data) = fs.read(&format!("xanim/{name}")) {
        return root_motion(name, &data).ok();
    }
    Some(fs.decoded_anim(name)?.ok()?.motion())
}

impl XAnim {
    /// [`root_motion`] off a clip already parsed.
    pub fn motion(&self) -> RootMotion {
        let last = self.frame_count - 1;
        let distance = match &self.root {
            Some(root) => match (root.sample(0.0).0, root.sample(last as f32).0) {
                (Some(a), Some(b)) => (b - a).length(),
                _ => 0.0,
            },
            None => 0.0,
        };
        RootMotion {
            length_ms: x87_length_ms(last, self.framerate as u32) as i32,
            duration: self.duration(),
            looping: self.looping,
            frequency: match last {
                0 => 0.0,
                n => self.framerate / n as f32,
            },
            distance,
        }
    }

    /// Seconds from first to last frame; 0.0 for single-frame anims.
    pub fn duration(&self) -> f32 {
        (self.frame_count - 1) as f32 / self.framerate
    }
    /// Seconds to fractional frame. Looping wraps over `frame_count - 1`,
    /// otherwise clamps to the last frame.
    pub fn frame_pos(&self, t: f32, looping: bool) -> f32 {
        let last = (self.frame_count - 1) as f32;
        let f = t.max(0.0) * self.framerate;
        if looping && last > 0.0 {
            f % last
        } else {
            f.min(last)
        }
    }
}

fn interp<T: Copy>(keys: &[(u16, T)], frame_pos: f32, lerp: impl Fn(T, T, f32) -> T) -> Option<T> {
    let (first, last) = (keys.first()?, keys.last()?);
    if frame_pos <= first.0 as f32 {
        return Some(first.1);
    }
    if frame_pos >= last.0 as f32 {
        return Some(last.1);
    }
    let i = keys.partition_point(|(f, _)| (*f as f32) <= frame_pos);
    let (f0, v0) = keys[i - 1];
    let (f1, v1) = keys[i];
    let s = (frame_pos - f0 as f32) / (f1 - f0).max(1) as f32;
    Some(lerp(v0, v1, s))
}

impl Track {
    /// `None` for a channel with no keys; clamps outside the keyed range.
    pub fn sample(&self, frame_pos: f32) -> (Option<Vec3>, Option<Quat>) {
        (
            interp(&self.trans_keys, frame_pos, |a, b, s| a.lerp(b, s)),
            interp(&self.rot_keys, frame_pos, |a, b, s| a.slerp(b, s)),
        )
    }
}

/// [`load`] through a cache by name that keeps hits and failures; a
/// failure is warned once, as `<what> '<name>': <error>`.
pub fn load_cached(
    cache: &mut HashMap<String, Option<Rc<XAnim>>>,
    fs: &dyn AssetSource,
    name: &str,
    what: &str,
) -> Option<Rc<XAnim>> {
    if let Some(c) = cache.get(name) {
        return c.clone();
    }
    let clip = match load(fs, name) {
        Ok(a) => Some(Rc::new(a)),
        Err(e) => {
            log::warn!("{what} '{name}': {e:#}, skipping that clip");
            None
        }
    };
    cache.insert(name.to_string(), clip.clone());
    clip
}

/// A clip by its `xanim/` entry's name. The read goes through
/// [`AssetSource::read`], so a [`crate::pk3::Pk3Fs`] marks the pak it came from.
/// [`load`] builds through [`Asset::find`], not the cached
/// [`crate::asset_source::load`]: every caller keeps its own clip cache
/// ([`load_cached`], the hit rigs, the view weapons).
impl Asset for XAnim {
    const KIND: AssetKind = AssetKind::Anim;

    fn find(src: &dyn AssetSource, name: &str) -> Result<Self> {
        if let Some(clip) = src.decoded_anim(name) {
            return clip;
        }
        let path = format!("xanim/{name}");
        let data = src
            .read(&path)
            .ok_or_else(|| anyhow!("{path} not found in pk3s"))?;
        parse(name, &data)
    }
}

/// [`XAnim::find`]: `name` is the `xanim/` entry, e.g. `pb_stand_alert`.
pub fn load(fs: &dyn AssetSource, name: &str) -> Result<XAnim> {
    XAnim::find(fs, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_length_truncates_as_the_engine_stores_it() {
        // 1.1d's death clip: 3066.67 reads 3066 (retail's `legsTimer` 3116).
        assert_eq!(x87_length_ms(92, 30), 3066);
        assert_eq!(x87_length_ms(58, 30), 1933);
        assert_eq!(x87_length_ms(60, 30), 2000);
        // A tenth rounds up in extended precision, so it stays whole.
        assert_eq!(x87_length_ms(3, 30), 100);
        assert_eq!(x87_length_ms(1, 3), 333);
    }

    /// 3 frames at 30 fps; bone 0 full-quat with sparse rot keys, bone 1 simple.
    fn fixture() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(14u16.to_le_bytes()); // version
        d.extend(3u16.to_le_bytes()); // frame_count
        d.extend(2u16.to_le_bytes()); // bone_count
        d.push(0); // flags
        d.extend(30u16.to_le_bytes()); // framerate
        d.push(0b00); // bitset A: no first key negated
        d.push(0b10); // bitset B: bone 1 simple
        d.extend(b"full\0simp\0");
        // bone 0: rot_count 2 with index list [0, 2]
        d.extend(2u16.to_le_bytes());
        d.extend([0u8, 2]);
        d.extend([0i16, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
        // 90 deg about Z, z = sin 45 * 32768
        d.extend([0i16, 0, 23170].iter().flat_map(|v| v.to_le_bytes()));
        // bone 0: trans_count 1, no index list, key (1, 2, 3)
        d.extend(1u16.to_le_bytes());
        for v in [1f32, 2.0, 3.0] {
            d.extend(v.to_le_bytes());
        }
        // bone 1: rot_count 3 == frame_count, no index list, 3 simple keys
        d.extend(3u16.to_le_bytes());
        for v in [0i16, 11585, 23170] {
            d.extend(v.to_le_bytes());
        }
        d.extend(0u16.to_le_bytes()); // trans_count 0
        d.push(1); // note_count
        d.extend(b"fire\0");
        d.extend(2u16.to_le_bytes());
        d
    }

    #[test]
    fn parses_synthetic_anim() {
        let a = parse("fix", &fixture()).unwrap();
        assert_eq!(a.frame_count, 3);
        assert_eq!(a.framerate, 30.0);
        assert_eq!(a.tracks.len(), 2);
        assert_eq!(a.tracks[0].bone, "full");
        assert_eq!(a.tracks[0].rot_keys.len(), 2);
        assert_eq!(a.tracks[0].rot_keys[1].0, 2);
        let q = a.tracks[0].rot_keys[1].1;
        assert!(
            q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-3),
            "{q:?}"
        );
        assert_eq!(a.tracks[0].trans_keys, vec![(0, Vec3::new(1.0, 2.0, 3.0))]);
        assert_eq!(a.tracks[1].rot_keys.len(), 3);
        assert_eq!(a.tracks[1].rot_keys[2].0, 2);
        assert!(a.tracks[1].trans_keys.is_empty());
        assert_eq!(a.notes, vec![("fire".to_string(), 2)]);
    }

    /// Bitset A negates a track's first key, and the keys after it follow
    /// it onto that hemisphere: the sampler sums them signed.
    #[test]
    fn bitset_a_negates_the_first_key_and_the_track_follows() {
        let mut d = fixture();
        assert_eq!(d[9], 0b00);
        d[9] = 0b01;
        let a = parse("flip", &d).unwrap();
        let k = &a.tracks[0].rot_keys;
        assert_eq!(k[0].1, -Quat::IDENTITY);
        let q = -Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        assert!(k[1].1.abs_diff_eq(q, 1e-3), "{:?}", k[1].1);
        assert!(a.tracks[1].rot_keys[0].1.w > 0.0);
    }

    #[test]
    fn rejects_frame_lists_that_are_not_ascending() {
        // bone 0's rot index list [0, 2] sits at bytes 23..25
        let mut d = fixture();
        assert_eq!(&d[23..25], &[0, 2]);
        d[23..25].copy_from_slice(&[2, 0]);
        let err = parse("desc", &d).unwrap_err().to_string();
        assert!(err.contains("ascending"), "got: {err}");
        d[23..25].copy_from_slice(&[1, 1]);
        assert!(parse("dup", &d).is_err());
    }

    #[test]
    fn rejects_wrong_version_and_unknown_flags() {
        assert!(
            parse("v", &20u16.to_le_bytes())
                .unwrap_err()
                .to_string()
                .contains("14")
        );
        let mut d = fixture();
        d[6] = 4; // no flag above 0x3 occurs in the shipped corpus
        assert!(parse("f", &d).unwrap_err().to_string().contains("flags"));
    }

    #[test]
    fn loop_flag_adds_the_loop_closing_key() {
        let mut d = Vec::new();
        d.extend(14u16.to_le_bytes());
        d.extend(2u16.to_le_bytes()); // header frame count
        d.extend(1u16.to_le_bytes()); // bone_count
        d.push(1); // flags: looping
        d.extend(30u16.to_le_bytes());
        d.push(0b0); // bitset A
        d.push(0b1); // bitset B: bone 0 simple
        d.extend(b"b\0");
        d.extend(3u16.to_le_bytes()); // rot_count == span, so frames are implicit
        for v in [0i16, 11585, 0] {
            d.extend(v.to_le_bytes());
        }
        d.extend(0u16.to_le_bytes()); // trans_count
        d.push(0); // note_count

        let a = parse("loop", &d).unwrap();
        assert!(a.looping);
        assert_eq!(a.frame_count, 3);
        assert_eq!(a.duration(), 2.0 / 30.0); // loop period, not span-1 frames
        let frames: Vec<u16> = a.tracks[0].rot_keys.iter().map(|(f, _)| *f).collect();
        assert_eq!(frames, vec![0, 1, 2]);
    }

    #[test]
    fn delta_flag_consumes_the_root_motion_track() {
        let mut d = Vec::new();
        d.extend(14u16.to_le_bytes());
        d.extend(3u16.to_le_bytes()); // frame_count
        d.extend(1u16.to_le_bytes()); // bone_count
        d.push(2); // flags: delta
        d.extend(30u16.to_le_bytes());
        // delta rot: 1 key, constant, so no index list; single i16
        d.extend(1u16.to_le_bytes());
        d.extend(4096i16.to_le_bytes());
        // delta trans: 2 sparse keys on frames 0 and 2
        d.extend(2u16.to_le_bytes());
        d.extend([0u8, 2]);
        for v in [0f32, 0.0, 0.0, 7.0, 0.0, 0.0] {
            d.extend(v.to_le_bytes());
        }
        d.push(0b0); // bitset A
        d.push(0b1); // bitset B: bone 0 simple
        d.extend(b"b\0");
        d.extend(1u16.to_le_bytes()); // rot_count 1
        d.extend(0i16.to_le_bytes());
        d.extend(0u16.to_le_bytes()); // trans_count 0
        d.push(1); // note_count
        d.extend(b"land\0");
        d.extend(2u16.to_le_bytes());

        let a = parse("delta", &d).unwrap();
        assert!(!a.looping);
        assert_eq!(a.frame_count, 3);
        assert_eq!(a.tracks.len(), 1);
        assert_eq!(a.tracks[0].rot_keys.len(), 1);
        assert!(a.tracks[0].trans_keys.is_empty());
        assert_eq!(a.notes, vec![("land".to_string(), 2)]);
        let root = a.root.expect("flag 0x2 keeps the root track");
        assert_eq!(root.rot_keys.len(), 1);
        assert_eq!(root.rot_keys[0].1, dequant(0, 0, 4096));
        assert_eq!(
            root.trans_keys,
            vec![(0, Vec3::ZERO), (2, Vec3::new(7.0, 0.0, 0.0))]
        );
        assert!(parse("plain", &fixture()).unwrap().root.is_none());
    }

    #[test]
    fn the_mg42_gunner_leaves_carry_a_root_track() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let a = load(&fs, "pb_standMG42gunner_aim_forward_level").unwrap();
        let root = a.root.expect("root track");
        let (trans, _) = root.sample(0.0);
        assert!(
            trans
                .unwrap()
                .abs_diff_eq(Vec3::new(-11.92, 6.01, -47.46), 0.01),
            "{trans:?}"
        );
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut d = fixture();
        d.push(0);
        assert!(parse("x", &d).unwrap_err().to_string().contains("trailing"));
    }

    #[test]
    fn rejects_truncated_file() {
        let d = fixture();
        assert!(parse("t", &d[..d.len() - 4]).is_err());
    }

    #[test]
    fn duration_and_frame_pos() {
        let a = parse("fix", &fixture()).unwrap(); // 3 frames @30
        assert!((a.duration() - 2.0 / 30.0).abs() < 1e-6);
        assert_eq!(a.frame_pos(0.0, false), 0.0);
        assert!((a.frame_pos(1.0 / 30.0, false) - 1.0).abs() < 1e-4);
        assert_eq!(a.frame_pos(10.0, false), 2.0); // clamps
        let w = a.frame_pos(2.5 / 30.0, true); // wraps over 2.0
        assert!((w - 0.5).abs() < 1e-4, "{w}");
    }

    #[test]
    fn sampling_interpolates_between_sparse_keys() {
        let a = parse("fix", &fixture()).unwrap();
        let t = &a.tracks[0]; // rot keys on frames 0 (identity) and 2 (90 deg Z)
        let (p, q) = t.sample(1.0);
        assert_eq!(p, Some(Vec3::new(1.0, 2.0, 3.0))); // constant key holds everywhere
        let q = q.unwrap();
        assert!(
            q.abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4), 1e-3),
            "{q:?}"
        );
        let (_, q0) = t.sample(0.0);
        assert!(q0.unwrap().abs_diff_eq(Quat::IDENTITY, 1e-3));
        let (_, q2) = t.sample(2.0);
        assert!(
            q2.unwrap()
                .abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-3)
        );
    }

    #[test]
    fn empty_channels_sample_none() {
        let a = parse("fix", &fixture()).unwrap();
        let (p, q) = a.tracks[1].sample(1.0); // simple bone: rot only
        assert!(p.is_none());
        assert!(q.is_some());
    }

    #[test]
    fn parses_full_corpus_including_flagged() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let (mut ok, mut failed) = (0usize, Vec::new());
        for name in fs.list_prefix("xanim/") {
            let Some(d) = fs.read(&name) else { continue };
            if d.is_empty() {
                continue; // directory entry
            }
            match parse(&name, &d) {
                Ok(a) => {
                    ok += 1;
                    for t in &a.tracks {
                        for (f, q) in &t.rot_keys {
                            assert!((*f as u32) < a.frame_count && q.is_finite(), "{name}");
                        }
                        for (f, p) in &t.trans_keys {
                            assert!((*f as u32) < a.frame_count && p.is_finite(), "{name}");
                        }
                    }
                }
                Err(e) => failed.push(format!("{name}: {e}")),
            }
        }
        assert!(failed.is_empty(), "unparsed xanims: {failed:?}");
        // 2957 in the Deluxe paks, 2947 in a stock 1.1 install (the 1.5
        // paks add ten, e.g. the viewmodel_kar98mp set); downloaded map
        // paks only add
        assert!(ok >= 2900, "corpus shrank: {ok}");
        let run = load(&fs, "pb_combatrun_forward_loop").unwrap();
        assert!(run.looping, "movement loop anim must set the loop flag");
    }

    #[test]
    fn loads_real_kar98mp_anims() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        // The 1.5 paks repackaged the view anims; a stock 1.1 install has none.
        if fs.read("xanim/viewmodel_kar98mp_idle").is_none() {
            return;
        }
        // (name, frame_count, track_count)
        for (name, frames, bones) in [
            ("viewmodel_kar98mp_idle", 1, 48),
            ("viewmodel_kar98mp_fire", 11, 48),
            ("viewmodel_kar98mp_lastshot", 11, 48),
            ("viewmodel_kar98mp_rechamber", 32, 48),
            ("viewmodel_kar98mp_reload", 78, 48),
            ("viewmodel_kar98mp_ADS_up", 10, 1),
            ("viewmodel_kar98mp_ADS_down", 17, 1),
            ("viewmodel_kar98mp_pullout", 10, 48),
        ] {
            let a = load(&fs, name).unwrap();
            assert_eq!(a.frame_count, frames, "{name}");
            assert_eq!(a.tracks.len(), bones, "{name}");
            assert_eq!(a.framerate, 30.0, "{name}");
            for t in &a.tracks {
                for (f, q) in &t.rot_keys {
                    assert!(
                        (*f as u32) < a.frame_count && q.is_finite(),
                        "{name}/{}",
                        t.bone
                    );
                }
                for (f, p) in &t.trans_keys {
                    assert!(
                        (*f as u32) < a.frame_count && p.is_finite(),
                        "{name}/{}",
                        t.bone
                    );
                }
            }
        }
        let ads = load(&fs, "viewmodel_kar98mp_ADS_up").unwrap();
        assert_eq!(ads.tracks[0].bone, "tag_torso");
    }
}
