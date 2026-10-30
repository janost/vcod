//! CoD2's delta codec (docs/research/cod2-protocol.md 5.2, 5.3) and the
//! states it builds. A state holds one 32-bit word per netfield, in table
//! order: a float field keeps its `f32` bits, an int field its value
//! (sign-extended for a negative width). That is the C struct's word at the
//! field's offset, so `to_bits`/`from_bits` round-trip what retail stores.

use super::fields::{
    CLIENT_STATE_FIELDS, ENTITY_STATE_FIELDS, HUDELEM_FIELDS, NetField, OBJECTIVE_FIELDS,
    PLAYER_STATE_FIELDS,
};
use vcod_common::net::codec::{FieldCodec, MsgReader};

pub const ENTITY_FIELDS: usize = ENTITY_STATE_FIELDS.len();
pub const CLIENT_FIELDS: usize = CLIENT_STATE_FIELDS.len();
pub const PS_FIELDS: usize = PLAYER_STATE_FIELDS.len();
pub const OBJ_FIELDS: usize = OBJECTIVE_FIELDS.len();
pub const HUD_FIELDS: usize = HUDELEM_FIELDS.len();
pub const MAX_HUDELEMS: usize = 31;
pub const MAX_OBJECTIVES: usize = 16;

/// The index of `name` in `table`. Panics on a name the table lacks.
pub fn field_index(table: &[NetField], name: &str) -> usize {
    table
        .iter()
        .position(|f| f.name == name)
        .unwrap_or_else(|| panic!("no netfield {name}"))
}

/// `entityState_s`, 240 bytes: its number and the 59 netfield words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityState2 {
    pub number: u32,
    pub f: [u32; ENTITY_FIELDS],
}

impl EntityState2 {
    pub fn null(number: u32) -> Self {
        EntityState2 {
            number,
            f: [0; ENTITY_FIELDS],
        }
    }

    pub fn i32(&self, name: &str) -> i32 {
        self.f[field_index(&ENTITY_STATE_FIELDS, name)] as i32
    }

    pub fn f32(&self, name: &str) -> f32 {
        f32::from_bits(self.f[field_index(&ENTITY_STATE_FIELDS, name)])
    }

    /// `pos.trBase`.
    pub fn origin(&self) -> [f32; 3] {
        [
            self.f32("pos.trBase[0]"),
            self.f32("pos.trBase[1]"),
            self.f32("pos.trBase[2]"),
        ]
    }
}

/// `clientState_s`, 92 bytes: its number and the 22 netfield words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientState2 {
    pub number: u32,
    pub f: [u32; CLIENT_FIELDS],
}

impl ClientState2 {
    pub fn null(number: u32) -> Self {
        ClientState2 {
            number,
            f: [0; CLIENT_FIELDS],
        }
    }

    /// `name`, eight words of a 32-byte NUL-padded string.
    pub fn name(&self) -> String {
        let mut b = Vec::new();
        for k in 0..8 {
            let i = field_index(&CLIENT_STATE_FIELDS, &format!("name[{}]", k * 4));
            b.extend_from_slice(&self.f[i].to_le_bytes());
        }
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        super::latin1(&b[..end])
    }
}

/// `objective_t`, 28 bytes: the 3-bit state and the six netfield words.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Objective2 {
    pub state: u32,
    pub f: [u32; OBJ_FIELDS],
}

/// `hudelem_s`, 128 bytes, as its 32 netfield words.
pub type HudElem2 = [u32; HUD_FIELDS];

/// `playerState_s` (0x26a8 bytes): the 105 netfield words and the five array
/// blocks the delta carries. Everything else in the C struct is client-side.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerState2 {
    pub f: [u32; PS_FIELDS],
    pub stats: [i32; 6],
    pub ammo: [i32; 64],
    pub ammoclip: [i32; 64],
    pub objectives: [Objective2; MAX_OBJECTIVES],
    pub hud_archival: [HudElem2; MAX_HUDELEMS],
    pub hud_current: [HudElem2; MAX_HUDELEMS],
}

impl Default for PlayerState2 {
    fn default() -> Self {
        PlayerState2 {
            f: [0; PS_FIELDS],
            stats: [0; 6],
            ammo: [0; 64],
            ammoclip: [0; 64],
            objectives: [Objective2::default(); MAX_OBJECTIVES],
            hud_archival: [[0; HUD_FIELDS]; MAX_HUDELEMS],
            hud_current: [[0; HUD_FIELDS]; MAX_HUDELEMS],
        }
    }
}

impl PlayerState2 {
    pub fn i32(&self, name: &str) -> i32 {
        self.f[field_index(&PLAYER_STATE_FIELDS, name)] as i32
    }

    pub fn f32(&self, name: &str) -> f32 {
        f32::from_bits(self.f[field_index(&PLAYER_STATE_FIELDS, name)])
    }

    pub fn origin(&self) -> [f32; 3] {
        [
            self.f32("origin[0]"),
            self.f32("origin[1]"),
            self.f32("origin[2]"),
        ]
    }

    pub fn viewangles(&self) -> [f32; 3] {
        [
            self.f32("viewangles[0]"),
            self.f32("viewangles[1]"),
            self.f32("viewangles[2]"),
        ]
    }
}

/// CoD2's codec: a negative width sign-extends (5.2).
const CODEC: FieldCodec = FieldCodec::COD2;

/// `MSG_ReadDeltaField` (0x8069960), change bit included.
pub fn read_delta_field(r: &mut MsgReader, field: &NetField, from: u32) -> u32 {
    CODEC.read_delta_field(r, from as i32, field.bits) as u32
}

/// The playerstate's per-field read (0x806aaca): no zero flag on floats or
/// ints, the `-100` arm keeps its own.
fn read_ps_field(r: &mut MsgReader, field: &NetField, from: u32) -> u32 {
    if r.read_bits(1) == 0 {
        return from;
    }
    CODEC.read_ps_value(r, from as i32, field.bits) as u32
}

/// What `MSG_ReadDeltaStruct` (0x8069d4a) found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delta {
    Removed,
    Unchanged,
    Changed { lc: u8 },
}

/// `MSG_ReadDeltaStruct`: remove bit, no-change bit, `lc` byte, `lc` fields,
/// the rest copied from `from`. `to` is written unless the entry is removed.
fn read_delta_struct(
    r: &mut MsgReader,
    table: &[NetField],
    from: &[u32],
    to: &mut [u32],
) -> anyhow::Result<Delta> {
    if r.read_bits(1) == 1 {
        return Ok(Delta::Removed);
    }
    if r.read_bits(1) == 0 {
        to.copy_from_slice(from);
        return Ok(Delta::Unchanged);
    }
    let lc = r.read_byte();
    anyhow::ensure!(
        usize::from(lc) <= table.len(),
        "delta lc {lc} past {}",
        table.len()
    );
    for (i, field) in table.iter().enumerate() {
        to[i] = if i < usize::from(lc) {
            read_delta_field(r, field, from[i])
        } else {
            from[i]
        };
    }
    Ok(Delta::Changed { lc })
}

/// `MSG_ReadDeltaEntity`: entity `number` against `from`.
pub fn read_delta_entity(
    r: &mut MsgReader,
    from: &EntityState2,
    number: u32,
) -> anyhow::Result<(Delta, EntityState2)> {
    let mut to = EntityState2::null(number);
    let d = read_delta_struct(r, &ENTITY_STATE_FIELDS, &from.f, &mut to.f)?;
    Ok((d, to))
}

/// `MSG_ReadDeltaClient`: client state `number` against `from`.
pub fn read_delta_client(
    r: &mut MsgReader,
    from: &ClientState2,
    number: u32,
) -> anyhow::Result<(Delta, ClientState2)> {
    let mut to = ClientState2::null(number);
    let d = read_delta_struct(r, &CLIENT_STATE_FIELDS, &from.f, &mut to.f)?;
    Ok((d, to))
}

/// `MSG_ReadDeltaHudElems` (0x806a0d0): a 5-bit count, then per element a
/// 5-bit last-changed index; elements past the count are zeroed.
fn read_hudelems(r: &mut MsgReader, from: &[HudElem2], to: &mut [HudElem2]) {
    let count = (r.read_bits(5) as usize).min(to.len());
    for e in 0..count {
        let last = r.read_bits(5) as usize;
        for (i, field) in HUDELEM_FIELDS.iter().enumerate() {
            to[e][i] = if i <= last {
                read_delta_field(r, field, from[e][i])
            } else {
                from[e][i]
            };
        }
    }
    for el in &mut to[count..] {
        *el = [0; HUD_FIELDS];
    }
}

/// `MSG_ReadDeltaPlayerstate` (0x806aaca). `from` `None` is the zero state.
pub fn read_delta_playerstate(
    r: &mut MsgReader,
    from: Option<&PlayerState2>,
) -> anyhow::Result<(u8, PlayerState2)> {
    let zero;
    let from = match from {
        Some(p) => p,
        None => {
            zero = PlayerState2::default();
            &zero
        }
    };
    let mut to = from.clone();
    let lc = r.read_byte();
    anyhow::ensure!(usize::from(lc) <= PS_FIELDS, "playerstate lc {lc}");
    for (i, field) in PLAYER_STATE_FIELDS.iter().enumerate().take(lc.into()) {
        to.f[i] = read_ps_field(r, field, from.f[i]);
    }

    // stats[6]: shorts, but stats[3] 6 bits and stats[5] a byte.
    if r.read_bits(1) != 0 {
        let mask = r.read_bits(6);
        for i in 0..6 {
            if mask >> i & 1 != 0 {
                to.stats[i] = match i {
                    3 => r.read_bits(6),
                    5 => i32::from(r.read_byte()),
                    _ => i32::from(r.read_short()),
                };
            }
        }
    }
    // ammo behind a group gate, ammoclip without one.
    for (gated, arr) in [(true, &mut to.ammo), (false, &mut to.ammoclip)] {
        if gated && r.read_bits(1) == 0 {
            continue;
        }
        for block in 0..4 {
            if r.read_bits(1) != 0 {
                let mask = r.read_short() as u16;
                for i in 0..16 {
                    if mask >> i & 1 != 0 {
                        arr[block * 16 + i] = i32::from(r.read_short());
                    }
                }
            }
        }
    }
    // objectives: 3 raw bits of state, then the fields behind a gate (0x8069c98).
    if r.read_bits(1) != 0 {
        for (o, base) in to.objectives.iter_mut().zip(&from.objectives) {
            o.state = r.read_bits(3) as u32;
            if r.read_bits(1) != 0 {
                for (i, field) in OBJECTIVE_FIELDS.iter().enumerate() {
                    o.f[i] = read_delta_field(r, field, base.f[i]);
                }
            } else {
                o.f = base.f;
            }
        }
    }
    if r.read_bits(1) != 0 {
        read_hudelems(r, &from.hud_archival, &mut to.hud_archival);
        read_hudelems(r, &from.hud_current, &mut to.hud_current);
    }
    Ok((lc, to))
}
