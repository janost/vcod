//! Protocol descriptor: the version-specific wire facts, so another protocol
//! version is a second const.

/// One netfield. `bits == 0` is a float; a negative `bits` is a plain
/// unsigned width on CoD 1.1 (docs/protocol-1.1.md, "Delta field encoding").
/// `offset` is the engine struct offset, used only as a stable identity.
pub struct NetField {
    pub name: &'static str,
    pub offset: u16,
    pub bits: i32,
}

pub struct Protocol {
    pub version: u32,
    pub max_configstrings: usize,
    pub entity_fields: &'static [NetField],
    pub player_fields: &'static [NetField],
    pub client_fields: &'static [NetField],
}

pub const PROTOCOL_V1: Protocol = Protocol {
    version: 1,              // reported by cod_lnxded 1.1d getstatus, 2026-08-23
    max_configstrings: 2048, // codextended/src/shared.h:141
    entity_fields: crate::net::fields_v1::ENTITY_FIELDS,
    player_fields: crate::net::fields_v1::PLAYER_FIELDS,
    client_fields: crate::net::fields_v1::CLIENT_FIELDS,
};

// Configstring block layout: docs/research/clientstate-wire-format.md,
// "Configstring map".

/// Model paths sit at `CS_MODELS_V1 + index`, index 1..=255 (`G_ModelIndex`,
/// game.mp.i386.so 0x66ed8).
pub const CS_MODELS_V1: usize = 268;

/// Tag names sit at `CS_TAGS_V1 + index`, index 1..=31 (`G_TagIndex`,
/// game.mp.i386.so 0x66fda).
pub const CS_TAGS_V1: usize = 108;

pub const GENTITYNUM_BITS: u32 = 10; // codextended/src/shared.h:395
pub const MAX_GENTITIES: usize = 1 << GENTITYNUM_BITS;
pub const ENTITYNUM_NONE: u32 = (MAX_GENTITIES - 1) as u32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_tables_look_sane() {
        let p = &PROTOCOL_V1;
        assert_eq!(p.entity_fields.len(), 59);
        assert_eq!(p.player_fields.len(), 103);
        // Array order is the wire order; pin both ends against a reordering
        // regeneration.
        assert_eq!(p.entity_fields[0].name, "pos.trTime");
        assert_eq!(p.entity_fields[58].name, "dmgFlags");
        assert_eq!(p.player_fields[0].name, "commandTime");
        assert_eq!(p.player_fields[102].name, "gunfx");
        assert_eq!(p.client_fields.len(), 22);
        assert_eq!(p.client_fields[0].name, "team");
        assert_eq!(p.client_fields[21].name, "name[28]");
        for f in p
            .entity_fields
            .iter()
            .chain(p.player_fields)
            .chain(p.client_fields)
        {
            assert!((-32..=32).contains(&f.bits), "{} bits {}", f.name, f.bits);
        }
        // Offsets identify fields, so a duplicate means a mis-parsed table.
        for table in [p.entity_fields, p.player_fields, p.client_fields] {
            let mut offsets: Vec<u16> = table.iter().map(|f| f.offset).collect();
            offsets.sort_unstable();
            let len = offsets.len();
            offsets.dedup();
            assert_eq!(offsets.len(), len, "duplicate offset in field table");
        }
    }
}
