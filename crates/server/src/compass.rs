//! `G_GetNonPVSFriendlyInfo` (game.mp.i386.so 0x42c30): the one teammate a
//! client's snapshot lacks, packed into `ps.iCompassFriendInfo` for the
//! compass (docs/research/cod11-hud-protocol.md, section 9, "Compass
//! friendlies").

/// `client + 0x2264` after an answer of 0: the next scan starts at slot 0.
pub const NO_FRIEND: u32 = 0x3ff;

/// What the scan reads of one slot's entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// `r.currentOrigin` x and y.
    pub at: [f32; 2],
    /// `r.currentAngles[YAW]`, degrees.
    pub yaw: f32,
}

/// The scan: from the slot after `last` (slot 0 after [`NO_FRIEND`]),
/// wrapping, the first of 64 slots `friend` answers for, which is a live
/// teammate the viewer's snapshot does not carry. Returns the packed field
/// and the slot, or `None` for 0.
pub fn next_friend(
    last: u32,
    eye: [f32; 2],
    mut friend: impl FnMut(usize) -> Option<Candidate>,
) -> Option<(i32, u32)> {
    let start = if last == NO_FRIEND { 0 } else { last + 1 };
    (0..64)
        .map(|i| ((start + i) % 64) as usize)
        .find_map(|n| friend(n).map(|c| (pack(n as u32, eye, c), n as u32)))
}

/// The packing: client number in bits 0..5, the x and y offsets from the
/// eye in 9 bits each at 6 and 15 (`offset / 4 + 255`), the yaw in the top
/// byte at 256/360. An offset past 1024 or -1022 scales the other axis by
/// the same factor and is clamped itself. Every conversion truncates, as
/// the x87 code does with its rounding mode set to chop.
pub fn pack(client: u32, eye: [f32; 2], c: Candidate) -> i32 {
    let mut d = [0i32; 2];
    for i in 0..2 {
        d[i] = (f64::from(c.at[i]) - f64::from(eye[i]) + 0.5) as i32;
    }
    let scale = |v: i32| -> f32 {
        if v > 1024 {
            1024.0 / v as f32
        } else if v < -1022 {
            -1022.0 / v as f32
        } else {
            1.0
        }
    };
    let (sx, sy) = (scale(d[0]), scale(d[1]));
    if sx < 1.0 || sy < 1.0 {
        if sx < sy {
            d[1] = (f64::from(d[1]) * f64::from(sx)) as i32;
        } else if sy < sx {
            d[0] = (f64::from(d[0]) * f64::from(sy)) as i32;
        }
    }
    // Rust's `/` truncates like the `sar` after the sign fix-up.
    let field = |v: i32| (((v.clamp(-1022, 1024) + 2) / 4 + 255) & 0x1ff) as u32;
    let yaw = (f64::from(c.yaw) * f64::from(256.0f32 / 360.0)) as i32 as u32;
    (client & 0x3f | field(d[0]) << 6 | field(d[1]) << 15 | yaw << 24) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot 0 at (-7352, -7976) on mp_harbor and slot 1 set down at
    /// spawns, from `client-probes/probe_compass` on retail, 2026-10-07:
    /// what each viewer was sent.
    #[test]
    fn packs_as_the_retail_probe_read() {
        let zero = [-7352.0, -7976.0];
        let one = |x: f32, y: f32, yaw: f32| Candidate { at: [x, y], yaw };
        let cases = [
            // In range on both axes; the negative one chops toward zero.
            (zero, 1, one(-8136.0, -7712.0, 0.0), 0x00a08f01_u32),
            (zero, 1, one(-8120.0, -8224.0, 0.0), 0x00611001),
            (zero, 1, one(-7216.0, -8784.0, 0.0), 0x001b4841),
            // x past -1022: y scaled by it, x clamped.
            (zero, 1, one(-9104.0, -8712.0, 0.0), 0x004a8001),
            // The yaw byte on the frame `setPlayerAngles` turned it.
            (zero, 1, one(-9512.0, -8972.0, 45.0), 0x20450001),
            (zero, 1, one(-9680.0, -7953.0, 270.0), 0xc0810001),
            (zero, 1, one(-9240.0, -8148.0, 180.0), 0x80748001),
            // The other way round: y past 1024 scales x.
            (
                [-7680.0, -9056.0],
                0,
                Candidate { at: zero, yaw: 0.0 },
                0x00ffd340,
            ),
            (
                [-9104.0, -8712.0],
                0,
                Candidate { at: zero, yaw: 0.0 },
                0x00b5ffc0,
            ),
        ];
        for (eye, client, c, want) in cases {
            assert_eq!(pack(client, eye, c) as u32, want, "{eye:?} {c:?}");
        }
    }

    #[test]
    fn the_scan_starts_after_the_last_answer_and_wraps() {
        let c = Candidate {
            at: [0.0, 0.0],
            yaw: 0.0,
        };
        let friends = |n: usize| [3, 9].contains(&n).then_some(c);
        let slot = |last| next_friend(last, [0.0, 0.0], friends).map(|(_, n)| n);
        assert_eq!(slot(NO_FRIEND), Some(3));
        assert_eq!(slot(3), Some(9));
        assert_eq!(slot(9), Some(3));
        assert_eq!(next_friend(NO_FRIEND, [0.0, 0.0], |_| None), None);
    }
}
