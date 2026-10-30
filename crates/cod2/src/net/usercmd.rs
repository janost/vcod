//! CoD2's 28-byte `usercmd_s` and its keyed delta
//! (docs/research/cod2-protocol.md 4.2, 4.3): `MSG_ReadDeltaUsercmdKey`
//! 0x8068f9e, `MSG_WriteDeltaUsercmdKey` 0x8068bc0, `MSG_SetDefaultUserCmd`
//! 0x80689a6.

use super::delta::PlayerState2;
use vcod_common::net::codec::{MsgReader, MsgWriter};

/// `usercmd_s`. Angles are 16-bit (`ANGLE2SHORT`) values held in the low half.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UserCmd2 {
    pub server_time: i32,
    pub buttons: i32,
    pub weapon: u8,
    pub off_hand_index: u8,
    pub angles: [i32; 3],
    pub forward_move: i8,
    pub right_move: i8,
}

pub const MAX_PACKET_USERCMDS: usize = 32;

fn mask(n: u32) -> u32 {
    if n >= 32 { u32::MAX } else { (1 << n) - 1 }
}

/// The forward/right code (0x8068b20): bit 0 forward, 1 back, 2 right, 3 left.
fn move_code(fwd: i8, right: i8) -> u32 {
    let mut c = 0;
    if fwd > 10 {
        c |= 1;
    } else if fwd < -10 {
        c |= 2;
    }
    if right > 10 {
        c |= 4;
    } else if right < -10 {
        c |= 8;
    }
    c
}

/// Its decode (0x8068b66): 127, -127 or 0 each.
fn move_from_code(c: u32) -> (i8, i8) {
    let one = |pos: u32, neg: u32| {
        if c & pos != 0 {
            127
        } else if c & neg != 0 {
            -127
        } else {
            0
        }
    };
    (one(1, 2), one(4, 8))
}

fn read_keyed_bits(r: &mut MsgReader, key: u32, n: u32) -> u32 {
    r.read_bits(n as i32) as u32 ^ (key & mask(n))
}

/// A change bit, then `n` keyed bits; clear keeps `base`.
fn read_keyed_field(r: &mut MsgReader, key: u32, base: u32, n: u32) -> u32 {
    if r.read_bits(1) == 0 {
        base
    } else {
        read_keyed_bits(r, key, n)
    }
}

/// A change bit, then a keyed short; masked to 16 bits.
fn read_keyed_angle(r: &mut MsgReader, key: u32, base: i32) -> i32 {
    let v = if r.read_bits(1) == 0 {
        base
    } else {
        i32::from((key as u16 ^ r.read_short() as u16) as i16)
    };
    v & 0xffff
}

/// `MSG_ReadDeltaUsercmdKey`.
pub fn read_delta_usercmd(r: &mut MsgReader, mut key: u32, from: &UserCmd2) -> UserCmd2 {
    let mut to = *from;
    to.server_time = if r.read_bits(1) == 0 {
        r.read_long()
    } else {
        from.server_time.wrapping_add(i32::from(r.read_byte()))
    };
    if read_keyed_bits(r, key, 1) == 0 {
        return to;
    }
    to.buttons &= !1;
    let full = read_keyed_bits(r, key, 1) != 0;
    if !full {
        key ^= to.server_time as u32;
    }
    to.buttons |= read_keyed_bits(r, key, 1) as i32;
    to.angles[0] = read_keyed_angle(r, key, from.angles[0]);
    to.angles[1] = read_keyed_angle(r, key, from.angles[1]);
    let code = read_keyed_field(r, key, move_code(from.forward_move, from.right_move), 4);
    (to.forward_move, to.right_move) = move_from_code(code);
    if full {
        key ^= to.server_time as u32;
        to.angles[2] = read_keyed_angle(r, key, from.angles[2]);
        let hi = read_keyed_field(r, key, (from.buttons >> 1) as u32, 18);
        to.buttons = (to.buttons & 1) | (hi << 1) as i32;
        to.weapon = read_keyed_field(r, key, u32::from(from.weapon), 7) as u8;
        to.off_hand_index = read_keyed_field(r, key, u32::from(from.off_hand_index), 7) as u8;
    }
    to
}

fn write_keyed_bits(w: &mut MsgWriter, key: u32, v: u32, n: u32) {
    w.write_bits((key ^ v) as i32, n as i32);
}

fn write_keyed_field(w: &mut MsgWriter, key: u32, from: u32, to: u32, n: u32) {
    if from == to {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        write_keyed_bits(w, key, to, n);
    }
}

fn write_keyed_angle(w: &mut MsgWriter, key: u32, from: i32, to: i32) {
    if from as i16 == to as i16 {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        w.write_short((key ^ to as u32) as i16);
    }
}

/// `MSG_WriteDeltaUsercmdKey`.
pub fn write_delta_usercmd(w: &mut MsgWriter, mut key: u32, from: &UserCmd2, to: &UserCmd2) {
    let dt = to.server_time.wrapping_sub(from.server_time) as u32;
    if dt < 0x100 {
        w.write_bits(1, 1);
        w.write_byte(dt as u8);
    } else {
        w.write_bits(0, 1);
        w.write_long(to.server_time);
    }
    let to_code = move_code(to.forward_move, to.right_move);
    let from_code = move_code(from.forward_move, from.right_move);
    let compact = from.buttons >> 1 == to.buttons >> 1
        && from.weapon == to.weapon
        && from.off_hand_index == to.off_hand_index
        && from.angles[2] == to.angles[2];
    if compact {
        if from.angles[0] == to.angles[0]
            && from.angles[1] == to.angles[1]
            && from.buttons & 1 == to.buttons & 1
            && from_code == to_code
        {
            write_keyed_bits(w, key, 0, 1);
            return;
        }
        write_keyed_bits(w, key, 1, 1);
        write_keyed_bits(w, key, 0, 1);
        key ^= to.server_time as u32;
    } else {
        write_keyed_bits(w, key, 1, 1);
        write_keyed_bits(w, key, 1, 1);
    }
    write_keyed_bits(w, key, to.buttons as u32, 1);
    write_keyed_angle(w, key, from.angles[0], to.angles[0]);
    write_keyed_angle(w, key, from.angles[1], to.angles[1]);
    write_keyed_field(w, key, from_code, to_code, 4);
    if !compact {
        key ^= to.server_time as u32;
        write_keyed_angle(w, key, from.angles[2], to.angles[2]);
        write_keyed_field(
            w,
            key,
            (from.buttons >> 1) as u32,
            (to.buttons >> 1) as u32,
            18,
        );
        write_keyed_field(w, key, from.weapon.into(), to.weapon.into(), 7);
        write_keyed_field(
            w,
            key,
            from.off_hand_index.into(),
            to.off_hand_index.into(),
            7,
        );
    }
}

/// `MSG_SetDefaultUserCmd`: the base the first cmd of a message deltas from.
pub fn default_usercmd(ps: &PlayerState2) -> UserCmd2 {
    let mut c = UserCmd2 {
        weapon: ps.i32("weapon") as u8,
        off_hand_index: ps.i32("offHandIndex") as u8,
        ..UserCmd2::default()
    };
    let view = ps.viewangles();
    for (i, v) in view.iter().take(2).enumerate() {
        let delta = ps.i32(&format!("delta_angles[{i}]")) as i16;
        let a = ((v * (65536.0f32 / 360.0)) as i32) as i16;
        c.angles[i] = i32::from(a.wrapping_sub(delta) as u16);
    }
    if ps.i32("pm_flags") & 0x80_0000 == 0 {
        return c;
    }
    let eflags = ps.i32("eFlags");
    if eflags & 8 != 0 {
        c.buttons |= 0x100;
    } else if eflags & 4 != 0 {
        c.buttons |= 0x200;
    }
    let lean = ps.f32("leanf");
    if lean < 0.0 {
        c.buttons |= 0x40;
    } else if lean > 0.0 {
        c.buttons |= 0x80;
    }
    if ps.f32("fWeaponPosFrac") != 0.0 {
        c.buttons |= 0x1000;
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::huffman::Huffman;

    fn roundtrip(key: u32, from: UserCmd2, to: UserCmd2) -> UserCmd2 {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        write_delta_usercmd(&mut w, key, &from, &to);
        w.write_bits(3, 3);
        let mut r = MsgReader::from_plain(w.into_ops());
        let got = read_delta_usercmd(&mut r, key, &from);
        assert_eq!(r.read_bits(3), 3, "trailing op misaligned");
        got
    }

    #[test]
    fn keyed_delta_round_trips_each_branch() {
        let from = UserCmd2 {
            server_time: 1000,
            buttons: 0x4001,
            weapon: 3,
            off_hand_index: 1,
            angles: [100, 0xff00, 0],
            forward_move: 127,
            right_move: 0,
        };
        let key = 0xdead_beef;
        // unchanged but for time
        let t = UserCmd2 {
            server_time: 1050,
            ..from
        };
        assert_eq!(roundtrip(key, from, t), t);
        // compact: angles, the fire bit, the move code
        let c = UserCmd2 {
            server_time: 1100,
            buttons: 0x4000,
            angles: [0x8001, 5, 0],
            forward_move: -127,
            right_move: 127,
            ..from
        };
        assert_eq!(roundtrip(key, from, c), c);
        // full: buttons above bit 0, weapon, offhand, roll, a long time step
        let f = UserCmd2 {
            server_time: 90_000,
            buttons: 0x3_0405,
            weapon: 77,
            off_hand_index: 9,
            angles: [1, 2, 0xfffe],
            forward_move: 0,
            right_move: -127,
        };
        assert_eq!(roundtrip(key, from, f), f);
    }
}
