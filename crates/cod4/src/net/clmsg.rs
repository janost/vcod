//! A CoD4 1.7 client message: its ops (client commands, a move with its
//! usercmds) and their writer, the mirror image the probe builds its
//! packets with and the capture gate re-encodes against
//! (docs/research/cod4-netfields.md, "The client's answer").
//!
//! The usercmd's fields, widths and keys follow `cod4_lnxded` 1.7's reader
//! (0x813928e) and the choice of branch `iw3mp.exe`'s writer (0x505c60);
//! the 1.7 server playing a probe that moves, crouches, fires and throws
//! through it is the check (section 8).

use super::huffman::Huffman;
use super::parse::{UNUSED_HUFF, read_raw_string};
use anyhow::{Result, bail};
use vcod_common::net::codec::{MsgReader, MsgWriter};

pub const CLC_MOVE: u32 = 0;
pub const CLC_MOVE_NO_DELTA: u32 = 1;
pub const CLC_CLIENT_COMMAND: u32 = 2;
pub const CLC_EOF: u32 = 3;

/// `usercmd_s.buttons` bits. Prone, crouch, jump, the frag and the
/// secondary offhand did what they say on the 1.7 server under the playing
/// probe (cod4-netfields.md 8, cod4-combat.md 10.1). VERIFIED live.
pub mod buttons {
    /// Fire: the one button both branches carry, on its own bit ahead of
    /// the fields (`cod4_lnxded` 0x813928e).
    pub const ATTACK: u32 = 0x1;
    /// The melee key; a long cmd holding it carries the charge's yaw and
    /// distance (the `testb $0x4` in 0x813928e). VERIFIED.
    pub const MELEE: u32 = 0x4;
    pub const PRONE: u32 = 0x100;
    pub const CROUCH: u32 = 0x200;
    pub const JUMP: u32 = 0x400;
    /// The held sight key: the shot aim capture's `fWeaponPosFrac` rises on
    /// it (VERIFIED live, docs/research/cod4-combat.md 3.2).
    pub const ADS: u32 = 0x800;
    pub const FRAG: u32 = 0x4000;
    /// The secondary offhand (smoke, flash or concussion).
    pub const SMOKE: u32 = 0x8000;
    /// Held while a location is picked; a long cmd holding it carries the
    /// two location bytes (the `testb $0x1` on the buttons' third byte in
    /// 0x813928e). VERIFIED.
    pub const SELECTING_LOCATION: u32 = 0x10000;
    /// The fire key of a weapon whose definition sets the detonator word
    /// (+0x5a8 on a weapon of type 1): `cod4_lnxded` 0x805f7fa answers it
    /// for one, `ATTACK` for the rest. INFERRED (the branch).
    pub const DETONATE: u32 = 0x80000;
}

/// `usercmd_s` as the wire carries it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UserCmd {
    pub server_time: i32,
    pub buttons: u32,
    /// Short angles, the view minus `delta_angles`.
    pub angles: [u16; 3],
    pub weapon: u8,
    pub offhand: u8,
    /// 127, -127 or 0: the wire carries only the sign.
    pub forwardmove: i8,
    pub rightmove: i8,
    pub selected_location: [u8; 2],
    /// `meleeChargeYaw` as its wire short.
    pub melee_charge_yaw: u16,
    pub melee_charge_dist: u8,
}

/// How much of a usercmd went past its time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Branch {
    /// Nothing: the cmd is its base with the new time.
    #[default]
    Same,
    /// Attack, pitch, yaw and the moves.
    Short,
    /// Every field.
    Long,
}

/// The choices one usercmd's encoding made that its value does not show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CmdWire {
    /// The time went as a byte past the base's, not as a long.
    pub stepped: bool,
    pub branch: Branch,
    /// The sent flag of each field the branch carries, in [`FIELDS`] order.
    pub flags: Vec<bool>,
}

impl CmdWire {
    /// The long branch with every field flagged: the cmd reads the same
    /// whatever base the server builds for a message's first cmd (it takes
    /// that base from its own playerstate, `cod4_lnxded` 0x8130ab0 ahead of
    /// the reader in `SV_UserMove`).
    pub fn full(from: &UserCmd, to: &UserCmd) -> CmdWire {
        let flags = FIELDS
            .iter()
            .filter(|f| f.carried(Branch::Long, to.buttons))
            .map(|_| true)
            .collect();
        CmdWire {
            stepped: steps_from(from, to),
            branch: Branch::Long,
            flags,
        }
    }

    /// What `iw3mp.exe` 1.7's writer (0x505c60) picks. The long branch
    /// when anything but attack, pitch, yaw, the moves and the location
    /// differs; else nothing when those four match too, else the short one.
    /// A location change alone is never sent. Each flag is set when its
    /// field differs from the base. INFERRED (the writer's compares).
    pub fn natural(from: &UserCmd, to: &UserCmd) -> CmdWire {
        let differs = |f: &Field| f.get(from) != f.get(to);
        let attack = (from.buttons ^ to.buttons) & buttons::ATTACK != 0;
        let branch = if [Field::Roll, Field::Buttons, Field::Weapon, Field::Offhand]
            .iter()
            .chain(&[Field::ChargeYaw, Field::ChargeDist])
            .any(differs)
        {
            Branch::Long
        } else if attack || [Field::Pitch, Field::Yaw, Field::Moves].iter().any(differs) {
            Branch::Short
        } else {
            Branch::Same
        };
        CmdWire {
            stepped: steps_from(from, to),
            branch,
            flags: FIELDS
                .iter()
                .filter(|f| f.carried(branch, to.buttons))
                .map(differs)
                .collect(),
        }
    }
}

/// The writer's time rule: a byte when the new time is 0 to 255 ms past
/// the base's, else the whole long (`cmp $0x100` at `iw3mp.exe` 0x505c75).
fn steps_from(from: &UserCmd, to: &UserCmd) -> bool {
    (to.server_time.wrapping_sub(from.server_time) as u32) < 256
}

/// One flagged field of a usercmd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Pitch,
    Yaw,
    Moves,
    Roll,
    Buttons,
    Weapon,
    Offhand,
    LocationX,
    LocationY,
    ChargeYaw,
    ChargeDist,
}

/// Wire order, `cod4_lnxded` 0x813928e. Each is a flag bit and, when set,
/// the value XOR the key cut to its [`Width`]. VERIFIED (the reader's
/// widths: 0x826e250, 4 bits; 0x826e290, 20; 0x826e25c, 7; 0x826e260, 8).
const FIELDS: [Field; 11] = [
    Field::Pitch,
    Field::Yaw,
    Field::Moves,
    Field::Roll,
    Field::Buttons,
    Field::Weapon,
    Field::Offhand,
    Field::LocationX,
    Field::LocationY,
    Field::ChargeYaw,
    Field::ChargeDist,
];

/// Where a field's value goes: the bit stream, or a whole byte or short.
#[derive(Clone, Copy)]
enum Width {
    Bits(u32),
    Byte,
    Short,
}

impl Width {
    fn mask(self) -> u32 {
        match self {
            Width::Bits(n) => (1 << n) - 1,
            Width::Byte => 0xff,
            Width::Short => 0xffff,
        }
    }

    fn read(self, r: &mut MsgReader) -> u32 {
        match self {
            Width::Bits(n) => r.read_bits(n as i32) as u32,
            Width::Byte => r.read_byte() as u32,
            Width::Short => r.read_short() as u16 as u32,
        }
    }

    fn write(self, w: &mut MsgWriter, v: u32) {
        let v = v & self.mask();
        match self {
            Width::Bits(n) => w.write_bits(v as i32, n as i32),
            Width::Byte => w.write_byte(v as u8),
            Width::Short => w.write_short(v as u16 as i16),
        }
    }
}

impl Field {
    fn width(self) -> Width {
        match self {
            Field::Pitch | Field::Yaw | Field::Roll | Field::ChargeYaw => Width::Short,
            Field::Moves => Width::Bits(4),
            Field::Buttons => Width::Bits(20),
            Field::Weapon | Field::Offhand => Width::Bits(7),
            Field::LocationX | Field::LocationY => Width::Byte,
            Field::ChargeDist => Width::Bits(8),
        }
    }

    /// Whether a cmd on `branch` holding `held` sends the field's flag.
    fn carried(self, branch: Branch, held: u32) -> bool {
        match (branch, self) {
            (Branch::Same, _) => false,
            (_, Field::Pitch | Field::Yaw | Field::Moves) => true,
            (Branch::Short, _) => false,
            (_, Field::LocationX | Field::LocationY) => held & buttons::SELECTING_LOCATION != 0,
            (_, Field::ChargeYaw | Field::ChargeDist) => held & buttons::MELEE != 0,
            _ => true,
        }
    }

    /// The long branch keys the fields the short one shares with the bare
    /// key and the rest with the time folded in; the short keys all of
    /// them with the time.
    fn key(self, branch: Branch, key: i32, time: i32) -> i32 {
        let shared = matches!(self, Field::Pitch | Field::Yaw | Field::Moves);
        if branch == Branch::Long && shared {
            key
        } else {
            key ^ time
        }
    }

    fn get(self, c: &UserCmd) -> u32 {
        match self {
            Field::Pitch => c.angles[0] as u32,
            Field::Yaw => c.angles[1] as u32,
            Field::Roll => c.angles[2] as u32,
            Field::Moves => move_mask(c),
            Field::Buttons => c.buttons >> 1,
            Field::Weapon => c.weapon as u32,
            Field::Offhand => c.offhand as u32,
            Field::LocationX => c.selected_location[0] as u32,
            Field::LocationY => c.selected_location[1] as u32,
            Field::ChargeYaw => c.melee_charge_yaw as u32,
            Field::ChargeDist => c.melee_charge_dist as u32,
        }
    }

    fn put(self, c: &mut UserCmd, v: u32) {
        match self {
            Field::Pitch => c.angles[0] = v as u16,
            Field::Yaw => c.angles[1] = v as u16,
            Field::Roll => c.angles[2] = v as u16,
            Field::Moves => set_moves(c, v),
            Field::Buttons => c.buttons = c.buttons & buttons::ATTACK | v << 1,
            Field::Weapon => c.weapon = v as u8,
            Field::Offhand => c.offhand = v as u8,
            Field::LocationX => c.selected_location[0] = v as u8,
            Field::LocationY => c.selected_location[1] = v as u8,
            Field::ChargeYaw => c.melee_charge_yaw = v as u16,
            Field::ChargeDist => c.melee_charge_dist = v as u8,
        }
    }
}

/// The moves' 4-bit mask, forward in bits 0-1 and right in 2-3: the low
/// bit of a pair past +10, the high one past -10 (`cmp $0xa` and
/// `$0xfffffff5` at `cod4_lnxded` 0x8139601, `$0xfffffff6` at `iw3mp.exe`
/// 0x505d18). VERIFIED (the constants).
fn move_mask(c: &UserCmd) -> u32 {
    let pair = |v: i8| match v {
        11.. => 1,
        ..=-11 => 2,
        _ => 0,
    };
    pair(c.forwardmove) | pair(c.rightmove) << 2
}

/// A mask back to moves: a pair's low bit reads 127, else its high bit
/// -127 (`and $0x81` on the reader's borrow, 0x813a67c).
fn set_moves(c: &mut UserCmd, mask: u32) {
    let axis = |pair: u32| match pair & 3 {
        1 | 3 => 127,
        2 => -127,
        _ => 0,
    };
    c.forwardmove = axis(mask);
    c.rightmove = axis(mask >> 2);
}

/// One keyed bit: the flag XOR the key's low bit.
fn read_keyed_bit(r: &mut MsgReader, key: i32) -> bool {
    (r.read_bits(1) ^ key) & 1 != 0
}

fn write_keyed_bit(w: &mut MsgWriter, on: bool, key: i32) {
    w.write_bits((on as i32 ^ key) & 1, 1);
}

/// `MSG_ReadDeltaUsercmdKey` (`cod4_lnxded` 0x813928e) off `from`.
pub fn read_usercmd(r: &mut MsgReader, key: i32, from: &UserCmd) -> (UserCmd, CmdWire) {
    let mut cmd = *from;
    let stepped = r.read_bits(1) != 0;
    cmd.server_time = match stepped {
        true => from.server_time.wrapping_add(r.read_byte() as i32),
        false => r.read_long(),
    };
    // An unchanged cmd stops after its first keyed bit.
    let branch = if !read_keyed_bit(r, key) {
        Branch::Same
    } else if read_keyed_bit(r, key) {
        Branch::Long
    } else {
        Branch::Short
    };
    let mut wire = CmdWire {
        stepped,
        branch,
        flags: Vec::new(),
    };
    if branch == Branch::Same {
        return (cmd, wire);
    }
    let attack = read_keyed_bit(r, Field::Pitch.key(branch, key, cmd.server_time));
    cmd.buttons = cmd.buttons & !buttons::ATTACK | attack as u32;
    for f in FIELDS {
        if !f.carried(branch, cmd.buttons) {
            continue;
        }
        let sent = r.read_bits(1) != 0;
        wire.flags.push(sent);
        let v = if sent {
            let k = f.key(branch, key, cmd.server_time) as u32;
            (f.width().read(r) ^ k) & f.width().mask()
        } else {
            f.get(&cmd)
        };
        f.put(&mut cmd, v);
    }
    (cmd, wire)
}

/// `to` off `from` the way `wire` says: [`read_usercmd`]'s inverse.
pub fn write_usercmd(w: &mut MsgWriter, key: i32, from: &UserCmd, to: &UserCmd, wire: &CmdWire) {
    w.write_bits(wire.stepped as i32, 1);
    if wire.stepped {
        w.write_byte(to.server_time.wrapping_sub(from.server_time) as u8);
    } else {
        w.write_long(to.server_time);
    }
    write_keyed_bit(w, wire.branch != Branch::Same, key);
    if wire.branch == Branch::Same {
        return;
    }
    write_keyed_bit(w, wire.branch == Branch::Long, key);
    let attack = to.buttons & buttons::ATTACK != 0;
    write_keyed_bit(
        w,
        attack,
        Field::Pitch.key(wire.branch, key, to.server_time),
    );
    let carried = FIELDS.iter().filter(|f| f.carried(wire.branch, to.buttons));
    for (f, i) in carried.zip(0..) {
        let sent = wire.flags.get(i).copied().unwrap_or(false);
        w.write_bits(sent as i32, 1);
        if sent {
            let k = f.key(wire.branch, key, to.server_time) as u32;
            f.width().write(w, f.get(to) ^ k);
        }
    }
}

/// One op of a client message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClOp {
    Command {
        sequence: i32,
        text: Vec<u8>,
    },
    Move {
        /// `clc_move` (deltas the snapshot it acknowledges) or
        /// `clc_moveNoDelta`.
        delta: bool,
        cmds: Vec<(UserCmd, CmdWire)>,
        /// The predicted origin, three float words, and its server time.
        origin: [u32; 3],
        origin_time: i32,
    },
    Eof,
}

/// A client netchan message: the 9-byte plain header and its ops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientMessage {
    pub sequence: u32,
    pub qport: u16,
    pub server_id: u8,
    pub message_ack: i32,
    pub reliable_ack: i32,
    pub ops: Vec<ClOp>,
}

/// `checksumFeed ^ messageAcknowledge ^ Com_HashKey(serverCommand, 32)`:
/// the usercmd key, the command the one `reliableAcknowledge` names
/// (`SV_UserMove`, `cod4_lnxded` 0x81725eb; cod4-netfields.md 5).
pub fn usercmd_key(checksum_feed: i32, message_ack: i32, server_command: &[u8]) -> i32 {
    checksum_feed ^ message_ack ^ vcod_common::net::com_hash_key_bytes(server_command, 32)
}

/// The ops of a decompressed client block (`SV_ExecuteClientMessage`): 3-bit
/// op codes, commands until the move. `base` is the first cmd's base;
/// fields a cmd leaves unsent read off it.
/// The commands that open a message, up to its first other op: what
/// `SV_ProcessClientCommands` reads of a message from before a
/// map change, whose usercmds are keyed to the last level's feed.
pub fn read_commands(plain: Vec<u8>) -> Vec<ClOp> {
    let mut r = MsgReader::from_plain(plain);
    let mut ops = Vec::new();
    while r.read_bits(3) as u32 == CLC_CLIENT_COMMAND && !r.is_overflowed() {
        let sequence = r.read_long();
        let text = read_raw_string(&mut r);
        if r.is_overflowed() {
            break;
        }
        ops.push(ClOp::Command { sequence, text });
    }
    ops
}

pub fn read_ops(plain: Vec<u8>, key: i32, base: &UserCmd) -> Result<Vec<ClOp>> {
    let mut r = MsgReader::from_plain(plain);
    let mut ops = Vec::new();
    loop {
        let op = r.read_bits(3) as u32;
        if r.is_overflowed() {
            break;
        }
        match op {
            CLC_EOF => {
                ops.push(ClOp::Eof);
                break;
            }
            CLC_CLIENT_COMMAND => {
                let sequence = r.read_long();
                let text = read_raw_string(&mut r);
                ops.push(ClOp::Command { sequence, text });
            }
            CLC_MOVE | CLC_MOVE_NO_DELTA => {
                let n = r.read_byte();
                let mut cmds = Vec::with_capacity(n as usize);
                let mut from = *base;
                for _ in 0..n {
                    let (c, w) = read_usercmd(&mut r, key, &from);
                    cmds.push((c, w));
                    from = c;
                }
                let origin = [0; 3].map(|_| r.read_long() as u32);
                let origin_time = r.read_long();
                if r.is_overflowed() {
                    bail!("move ran past the message");
                }
                ops.push(ClOp::Move {
                    delta: op == CLC_MOVE,
                    cmds,
                    origin,
                    origin_time,
                });
            }
            o => bail!("client op {o}"),
        }
    }
    Ok(ops)
}

/// The mirror of [`read_ops`]: the plain op bits, to be compressed and
/// scrambled into the block.
pub fn write_ops(ops: &[ClOp], key: i32, base: &UserCmd) -> Vec<u8> {
    let mut w = MsgWriter::new(&UNUSED_HUFF);
    for op in ops {
        match op {
            ClOp::Eof => w.write_bits(CLC_EOF as i32, 3),
            ClOp::Command { sequence, text } => {
                w.write_bits(CLC_CLIENT_COMMAND as i32, 3);
                w.write_long(*sequence);
                w.write_string_bytes(text);
            }
            ClOp::Move {
                delta,
                cmds,
                origin,
                origin_time,
            } => {
                let code = if *delta { CLC_MOVE } else { CLC_MOVE_NO_DELTA };
                w.write_bits(code as i32, 3);
                w.write_byte(cmds.len() as u8);
                let mut from = *base;
                for (c, wire) in cmds {
                    write_usercmd(&mut w, key, &from, c, wire);
                    from = *c;
                }
                for &o in origin {
                    w.write_long(o as i32);
                }
                w.write_long(*origin_time);
            }
        }
    }
    w.into_ops()
}

/// The compressed block of `ops`, before the scramble.
pub fn compress_ops(huff: &Huffman, ops: &[ClOp], key: i32, base: &UserCmd) -> Vec<u8> {
    huff.compress(&write_ops(ops, key, base))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(t: i32) -> UserCmd {
        UserCmd {
            server_time: t,
            buttons: buttons::CROUCH | buttons::ATTACK,
            angles: [100, 0xf000, 3],
            weapon: 5,
            offhand: 2,
            forwardmove: 127,
            rightmove: -127,
            ..UserCmd::default()
        }
    }

    #[test]
    fn usercmd_branches_round_trip_under_a_key() {
        let a = cmd(1000);
        let mut b = a;
        b.server_time = 1016;
        let mut c = b;
        c.server_time = 1033;
        c.angles[1] = 0xf010;
        c.buttons &= !buttons::ATTACK;
        let mut d = c;
        d.server_time = 1050;
        d.buttons |= buttons::JUMP;
        d.weapon = 6;
        let base = UserCmd::default();
        let key = 0x5a5a_1234u32 as i32;
        let wires = [
            CmdWire::full(&base, &a),
            CmdWire::natural(&a, &b),
            CmdWire::natural(&b, &c),
            CmdWire::natural(&c, &d),
        ];
        assert_eq!(wires[1].branch, Branch::Same);
        assert_eq!(wires[2].branch, Branch::Short);
        assert_eq!(wires[3].branch, Branch::Long);
        let ops = vec![
            ClOp::Command {
                sequence: 3,
                text: b"say hi".to_vec(),
            },
            ClOp::Move {
                delta: true,
                cmds: [a, b, c, d].into_iter().zip(wires).collect(),
                origin: [1, 2, 3],
                origin_time: 1050,
            },
            ClOp::Eof,
        ];
        let plain = write_ops(&ops, key, &base);
        assert_eq!(read_ops(plain, key, &base).unwrap(), ops);
    }

    /// Pins the wire bytes of every branch: the long one with the location
    /// and melee fields, a short one, an unchanged one and a long time.
    #[test]
    fn usercmd_wire_bytes_are_pinned() {
        let base = UserCmd::default();
        let a = UserCmd {
            server_time: 5000,
            buttons: buttons::MELEE | buttons::SELECTING_LOCATION | buttons::ATTACK,
            angles: [0x1234, 0xfedc, 0x0102],
            weapon: 9,
            offhand: 3,
            forwardmove: -127,
            rightmove: 127,
            selected_location: [17, 200],
            melee_charge_yaw: 0x4000,
            melee_charge_dist: 77,
        };
        let mut b = a;
        b.server_time = 5016;
        b.weapon = 10;
        b.selected_location = [18, 200];
        b.melee_charge_dist = 80;
        let mut c = b;
        c.server_time = 5033;
        c.angles[0] = 0x1300;
        c.forwardmove = 0;
        c.buttons &= !buttons::ATTACK;
        let mut d = c;
        d.server_time = 5050;
        let mut e = d;
        e.server_time = 9000;
        e.buttons = buttons::JUMP;
        let key = 0x0bad_f00du32 as i32;
        let wires = [
            CmdWire::full(&base, &a),
            CmdWire::natural(&a, &b),
            CmdWire::natural(&b, &c),
            CmdWire::natural(&c, &d),
            CmdWire::natural(&d, &e),
        ];
        let ops = vec![
            ClOp::Move {
                delta: false,
                cmds: [a, b, c, d, e].into_iter().zip(wires).collect(),
                origin: [7, 8, 9],
                origin_time: 9000,
            },
            ClOp::Eof,
        ];
        let plain = write_ops(&ops, key, &base);
        let hex: String = plain.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "81058813000039e2efd10e87e287639dd1f0944d85a3c801107e6487711115a4f04611282300005812dd0c07000000080000000900000028230000"
        );
        assert_eq!(read_ops(plain, key, &base).unwrap(), ops);
    }
}
