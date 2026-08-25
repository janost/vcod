//! CoD1 xanim v14 parsing and sampling. Layout: `docs/research/xanim-v14-format.md`;
//! header flags: `docs/research/player-model-anim-system.md`, "xanim v14 header flags".

use crate::pk3::Pk3Fs;
use crate::xmodel::Reader;
use anyhow::{anyhow, ensure, Result};
use glam::{Quat, Vec3};

#[derive(Debug)]
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
    /// Notetracks, no runtime consumer yet.
    pub notes: Vec<(String, u16)>,
}

#[derive(Debug)]
pub struct Track {
    pub bone: String,
    pub rot_keys: Vec<(u16, Quat)>, // (frame, local rot), ascending; empty = hold current
    pub trans_keys: Vec<(u16, Vec3)>, // (frame, offset from the bind local pos)
}

fn dequant(x: i16, y: i16, z: i16) -> Quat {
    let (qx, qy, qz) = (x as f32 / 32768.0, y as f32 / 32768.0, z as f32 / 32768.0);
    let qw = (1.0 - qx * qx - qy * qy - qz * qz).max(0.0).sqrt();
    Quat::from_xyzw(qx, qy, qz, qw)
}

pub fn parse(name: &str, data: &[u8]) -> Result<XAnim> {
    let mut r = Reader::new(data);
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
    let frame_count = header_frames + looping as u32;
    let framerate = r.u16()? as f32;
    ensure!(framerate > 0.0, "{name}: zero framerate");
    // Sparse index list only when 1 < count < frame_count; u8 unless the
    // highest index overflows a byte.
    let index_u16 = frame_count > 256;
    let frames_of = |r: &mut Reader, count: u32| -> Result<Vec<u16>> {
        if 1 < count && count < frame_count {
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
    };
    // Flag 0x2: nameless root-motion track with yaw-only rotations. The
    // server owns entity movement, so it is read and dropped.
    if flags & 2 != 0 {
        let rc = r.u16()? as u32;
        ensure!(rc <= frame_count, "{name}: delta rot {rc} > {frame_count}");
        let n = frames_of(&mut r, rc)?.len();
        r.skip(2 * n)?;
        let tc = r.u16()? as u32;
        ensure!(
            tc <= frame_count,
            "{name}: delta trans {tc} > {frame_count}"
        );
        let n = frames_of(&mut r, tc)?.len();
        r.skip(12 * n)?;
    }
    let nb = bone_count.div_ceil(8);
    r.skip(nb)?; // bitset A, role unknown
    let simple = r.take(nb)?.to_vec(); // bitset B: z-only rotation keys
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
        let rframes = frames_of(&mut r, rc)?;
        let mut rot_keys = Vec::with_capacity(rc as usize);
        for f in rframes {
            let q = if is_simple(i) {
                dequant(0, 0, r.i16()?)
            } else {
                let (x, y, z) = (r.i16()?, r.i16()?, r.i16()?);
                dequant(x, y, z)
            };
            rot_keys.push((f, q));
        }
        let tc = r.u16()? as u32;
        ensure!(
            tc <= frame_count,
            "{name}/{bone}: {tc} trans keys > {frame_count} frames"
        );
        let tframes = frames_of(&mut r, tc)?;
        let mut trans_keys = Vec::with_capacity(tc as usize);
        for f in tframes {
            trans_keys.push((f, Vec3::new(r.f32()?, r.f32()?, r.f32()?)));
        }
        tracks.push(Track {
            bone,
            rot_keys,
            trans_keys,
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
        notes,
    })
}

impl XAnim {
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

pub fn load(fs: &Pk3Fs, name: &str) -> Result<XAnim> {
    let path = format!("xanim/{name}");
    let data = fs
        .read(&path)
        .ok_or_else(|| anyhow!("{path} not found in pk3s"))?;
    parse(name, &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3 frames at 30 fps; bone 0 full-quat with sparse rot keys, bone 1 simple.
    fn fixture() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(14u16.to_le_bytes()); // version
        d.extend(3u16.to_le_bytes()); // frame_count
        d.extend(2u16.to_le_bytes()); // bone_count
        d.push(0); // flags
        d.extend(30u16.to_le_bytes()); // framerate
        d.push(0b00); // bitset A (ignored)
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
        assert!(parse("v", &20u16.to_le_bytes())
            .unwrap_err()
            .to_string()
            .contains("14"));
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
        assert!(q2
            .unwrap()
            .abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-3));
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
        // 2957 in the stock paks; downloaded map paks only add
        assert!(ok >= 2957, "corpus shrank: {ok}");
        let run = load(&fs, "pb_combatrun_forward_loop").unwrap();
        assert!(run.looping, "movement loop anim must set the loop flag");
    }

    #[test]
    fn loads_real_kar98mp_anims() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
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
