//! Protocol descriptors: the version-specific wire facts. A connection holds
//! one and everything that decodes reads it from there. Each game's
//! descriptor type and tables live in its own module: CoD1's [`Cod1Wire`]
//! (protocol 1 is patch 1.1, protocol 6 patch 1.5,
//! docs/research/cod15-protocol-diff.md, protocol 22 United Offensive 1.51,
//! docs/research/coduo-diff.md) in [`crate::net::cod1`], re-exported here.
//! What every game's descriptor shares is here: the configstring layout and
//! the HUD block.

pub use super::cod1::{
    CS_ALLIES_SCORE, CS_AXIS_SCORE, CS_CVAR_NAMES, CS_CVAR_VALUES, CS_EFFECTS, CS_FOG_V1,
    CS_FX_TAGS, CS_HINT_STRINGS, CS_LAYOUT_V1, CS_LAYOUT_V22, CS_LEVEL_START_TIME, CS_LOCALIZED,
    CS_MATCH_TIMEOUT_TEXT_V6, CS_MATCH_TIMEOUT_V6, CS_MODELS_V1, CS_NORTHYAW, CS_SHADERS,
    CS_SOUNDS, CS_TAGS_V1, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, install_protocol,
    install_protocol_of,
};
pub use super::cod1::{Cod1Wire, CursorHints};
pub use super::codec::{ANGLE_BITS, FieldCodec, NetField};
pub use super::netchan::NetchanParams;
pub use super::wire::{PakRules, SnapshotHead, Transport, WireGame, WireId};

/// The playerState's HUD element block, which United Offensive widened
/// (coduo-diff.md, section 5).
pub struct HudWire {
    /// Width of each array's element count.
    pub count_bits: i32,
    pub max_elems: usize,
    /// Fields per element on the wire; [`crate::net::msg::HudElem`] holds
    /// the widest protocol's.
    pub fields: usize,
}

/// CoD1's descriptor under its old name, for the call sites that predate
/// [`Cod1Wire`].
pub type Protocol = Cod1Wire;

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
    /// Where exp fog starts: CoD4's `setExpFog` start distance, which
    /// keeps everything nearer clear (cod4-world.md 11). 0 on CoD1 and
    /// CoD2.
    pub start: f32,
}

impl FogParams {
    /// RTCW-MP's client adds .1 to this slot before R_SetFog so its >1 test
    /// fires; CoD's wire carries the un-offset value ("1" in both live
    /// captures), and `setExpFog` refuses a density outside (0, 1).
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
            start: 0.0,
        })
    }
}

pub const GENTITYNUM_BITS: u32 = 10; // codextended/src/shared.h:395
pub const MAX_GENTITIES: usize = 1 << GENTITYNUM_BITS;
pub const ENTITYNUM_NONE: u32 = (MAX_GENTITIES - 1) as u32;
/// Reserved slot for the world entity (`worldspawn`); never a dynamic entity number.
pub const ENTITYNUM_WORLD: u32 = (MAX_GENTITIES - 2) as u32;
/// The client index in the snapshot's clientState stream (cod_lnxded 0x808e1fd).
pub const CLIENTNUM_BITS: u32 = 6;
/// Retail's `MAX_CLIENTS`: client slots index the 6-bit clientState field
/// (`ps.clientNum` gets 8), so more than 64 would collide silently.
pub const MAX_CLIENTS: usize = 1 << CLIENTNUM_BITS;

/// Serverinfo, `Cvar_InfoString(CVAR_SERVERINFO)`; `g_gametype` is a key.
pub const CS_SERVERINFO: usize = 0;
/// Systeminfo, `Cvar_InfoString_Big(CVAR_SYSTEMINFO)`, `sv_serverid` included.
pub const CS_SYSTEMINFO: usize = 1;
/// A configstring block the game module allocates into at runtime, each
/// mirroring one engine indexer in `game.mp.i386.so`. The bounds are the
/// ones those indexers walk; the addresses are in
/// docs/research/clientstate-wire-format.md.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CsRange {
    StatusIcon,
    HeadIcon,
    Tag,
    Model,
    SoundAlias,
    Effect,
    /// `playFxOnTag`'s `"%02d%s"` effect id and tag pairs
    /// (`G_FindConfigstringIndex(.., 0x34c, 0x100, ..)`, game.mp 0x5b386).
    FxTag,
    /// `G_ShellShockIndex` (game.mp 0x67540): `0x44c + i`, `i` in 1..16.
    ShellShock,
    Menu,
    Localized,
    Shader,
}

impl CsRange {
    pub const ALL: [CsRange; 11] = [
        CsRange::StatusIcon,
        CsRange::HeadIcon,
        CsRange::Tag,
        CsRange::Model,
        CsRange::SoundAlias,
        CsRange::Effect,
        CsRange::FxTag,
        CsRange::ShellShock,
        CsRange::Menu,
        CsRange::Localized,
        CsRange::Shader,
    ];

    /// Inclusive, in 1.1's layout (the one vcod's server writes).
    pub const fn bounds(self) -> (usize, usize) {
        CS_LAYOUT_V1.bounds(self)
    }
}

/// One protocol's configstring layout: the runtime blocks by [`CsRange`]
/// and the fixed slots that moved with them. Slots 0..21 (serverinfo up to
/// the level start time) are the same in every protocol.
#[derive(Clone, Copy, Debug)]
pub struct CsLayout {
    /// Inclusive bounds, indexed by `CsRange as usize`.
    pub ranges: [(usize, usize); 11],
    /// The mirrored cvars' names, inclusive; the values follow at
    /// `cvar_values`, the same offset into each block.
    pub cvar_names: (usize, usize),
    pub cvar_values: usize,
    /// The first of the 32 hint strings.
    pub hint_strings: usize,
    /// The match timeout's ms left; its text is the slot after.
    pub match_timeout: Option<usize>,
    /// The vote's time; its string, yes and no counts are the three slots
    /// after (docs/research/cod11-votes.md).
    pub vote: usize,
    /// `setCullFog`'s and `setExpFog`'s slot.
    pub fog: usize,
    /// `level.startTime`.
    pub level_start: usize,
    /// `g_motd`, which `SP_worldspawn` writes after the worldspawn's
    /// `message` (4).
    pub motd: usize,
    /// The `\winner\` info string `G_InitGame` resets.
    pub winner: usize,
}

impl CsLayout {
    pub const fn bounds(&self, r: CsRange) -> (usize, usize) {
        self.ranges[r as usize]
    }

    /// The slot one below a 1-based block: `base(r) + index` for index >= 1.
    pub const fn base(&self, r: CsRange) -> usize {
        self.bounds(r).0 - 1
    }

    pub const fn models(&self) -> usize {
        self.base(CsRange::Model)
    }

    pub const fn tags(&self) -> usize {
        self.base(CsRange::Tag)
    }

    pub const fn sounds(&self) -> usize {
        self.base(CsRange::SoundAlias)
    }

    pub const fn effects(&self) -> usize {
        self.base(CsRange::Effect)
    }

    pub const fn fx_tags(&self) -> usize {
        self.base(CsRange::FxTag)
    }

    pub const fn localized(&self) -> usize {
        self.base(CsRange::Localized)
    }

    pub const fn shaders(&self) -> usize {
        self.base(CsRange::Shader)
    }

    /// The configstring of a 1-based block index, `None` past the block.
    pub fn slot(&self, r: CsRange, index: i32) -> Option<usize> {
        let (first, last) = self.bounds(r);
        let i = self.base(r).checked_add(usize::try_from(index).ok()?)?;
        (first..=last).contains(&i).then_some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
