//! Protocol 6 (patch 1.5) netfield tables, read out of cod_lnxded 1.5b
//! (md5 d3ac406b33acf2c9278a813d5f011b46) with `tools/re/dump_field_table.py`
//! and diffed against 1.1d (docs/research/cod15-protocol-diff.md, "Netfield
//! tables").
//!
//!   entityStateFields 0x080db860 (59), clientState 0x080dbfe0 (22): every
//!     entry equal to 1.1d's, so protocol 6 shares `fields_v1`'s tables.
//!   playerStateFields 0x080dc560 (103): 1.1d's in the same order, but
//!     `pm_flags` [12] widened from 19 to 21 bits. Entry [76] is named
//!     `fJumpOriginZ` where 1.1d says `fJumpPeak`, at the same offset and
//!     width; it keeps the 1.1 name so a lookup by name reads either table.

use super::fields_v1;
use crate::net::codec::NetField;

pub static PLAYER_FIELDS: &[NetField] = &PLAYER_TABLE;

const PLAYER_TABLE: [NetField; 103] = {
    let mut t = fields_v1::PLAYER_TABLE;
    assert!(t[12].offset == 12 && t[12].bits == 19);
    t[12].bits = 21;
    t
};
