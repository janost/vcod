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

/// Global fog parameters, configstring 12. Written by the game dll's
/// `setCullFog` / `setExpFog` builtins; field order per RTCW-MP
/// cg_servercmds.c `CG_ParseFog` and two live captures
/// (docs/research/cod11-server-handshake.md, "Map-dependent, index < 140").
pub const CS_FOG_V1: usize = 12;

/// `<near> <far> <density> <r> <g> <b> <fadeTime ms>`. `density > 1`
/// selects linear farclip fog across near..far; otherwise GL_EXP at that
/// density (RTCW-MP tr_main.c R_SetFog). Linear fog leaves the sky alone,
/// exp fog fogs it (`drawsky` in the same function).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogParams {
    pub near: f32,
    pub far: f32,
    pub density: f32,
    pub color: [f32; 3],
    pub fade_ms: u32,
}

impl FogParams {
    /// RTCW-MP's client adds .1 to this slot before R_SetFog so its >1 test
    /// fires; CoD's wire carries the un-offset value ("1" in both live
    /// captures), and setExpFog densities are script-enforced below 1.
    pub fn is_linear(&self) -> bool {
        self.density >= 1.0
    }

    /// Empty or malformed configstrings mean "no fog state", not zero fog.
    pub fn parse(s: &str) -> Option<FogParams> {
        let mut it = s.split_whitespace();
        let mut num = || it.next().and_then(|t| t.parse::<f32>().ok());
        let near = num()?;
        let far = num()?;
        let density = num()?;
        let r = num()?;
        let g = num()?;
        let b = num()?;
        let fade_ms = num()? as u32;
        Some(FogParams {
            near,
            far,
            density,
            color: [r, g, b],
            fade_ms,
        })
    }
}

pub const GENTITYNUM_BITS: u32 = 10; // codextended/src/shared.h:395
pub const MAX_GENTITIES: usize = 1 << GENTITYNUM_BITS;
pub const ENTITYNUM_NONE: u32 = (MAX_GENTITIES - 1) as u32;
/// Reserved slot for the world entity (`worldspawn`); never a dynamic entity number.
pub const ENTITYNUM_WORLD: u32 = (MAX_GENTITIES - 2) as u32;

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

    #[test]
    fn fog_params_parse_live_captures() {
        // mp_pavlov.gsc setCullFog(0, 6000, 0.8, 0.8, 0.8, 0), captured live
        let f = FogParams::parse("0 6000 1 0.8 0.8 0.8 0").unwrap();
        assert_eq!(f.near, 0.0);
        assert_eq!(f.far, 6000.0);
        assert!(f.is_linear());
        assert_eq!(f.color, [0.8, 0.8, 0.8]);
        assert_eq!(f.fade_ms, 0);
        // mp_carentan.gsc setCullFog(0, 16500, 0.7, 0.85, 1.0, 0)
        let f = FogParams::parse("0 16500 1 0.7 0.85 1 0").unwrap();
        assert_eq!(f.far, 16500.0);
        assert_eq!(f.color, [0.7, 0.85, 1.0]);
    }

    #[test]
    fn fog_params_reject_garbage() {
        assert_eq!(FogParams::parse(""), None);
        assert_eq!(FogParams::parse("1 2 3"), None);
        assert_eq!(FogParams::parse("a b c d e f g"), None);
    }
}
