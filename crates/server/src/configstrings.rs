//! The configstrings a server sets before any entity exists, from
//! docs/research/cod11-server-handshake.md ("What a minimal server has to set").
//! Entity-driven slots (3, 8, 11, 12, 269.., 524.., 781..) stay empty.

use crate::server::ServerConfig;
use vcod_common::net::connectionless::Info;
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_gsc::ErrorKind;

/// `BG_SetupWeaponInfo`'s list, configstring 7, 1-based on the wire.
pub const WEAPON_LIST: &str = "bar_mp bar_slow_mp bren_mp colt_mp enfield_mp fg42_mp fg42_semi_mp fraggrenade_mp kar98k_mp kar98k_sniper_mp luger_mp m1carbine_mp m1garand_mp mg42_bipod_duck_mp mg42_bipod_prone_mp mg42_bipod_stand_mp mk1britishfrag_mp mosin_nagant_mp mosin_nagant_sniper_mp mp40_mp mp44_mp mp44_semi_mp panzerfaust_mp ppsh_mp ppsh_semi_mp ptrs41_antitank_rifle_mp rgd-33russianfrag_mp springfield_mp sten_mp stielhandgranate_mp thompson_mp thompson_semi_mp";

/// A weapon's 1-based position in [`WEAPON_LIST`], which is what every
/// weapon-shaped index on the wire and in the item table means. The lookup is
/// case-sensitive, matching retail's `strcmp` (`BG_FindItem` 0x2e214).
pub fn weapon_index(name: &str) -> Option<usize> {
    WEAPON_LIST
        .split(' ')
        .position(|w| w == name)
        .map(|i| i + 1)
}

/// Map-independent configstrings the engine itself sets. Everything else
/// this table used to carry is script output and is earned back at load:
/// 21/22 from `precacheStatusIcon`, 29 from `precacheHeadIcon`,
/// 140..180 / 204..244 from the cvar mirror, 1180..1187 from
/// `precacheMenu`, 1245/1246 from `precacheString` and 1501..1505 from
/// `precacheShader`.
///
/// 1212/1213 stay: no stock script precaches them, so they come from the
/// engine's own `G_GetHintStringIndex` at weapon load.
pub const STATIC: &[(usize, &str)] = &[
    (2, "cod"),
    (7, WEAPON_LIST),
    (13, "0"), // level.startTime
    (20, "\\winner\\0"),
    (1212, "CGAME_USEMG42"),
    (1213, "CGAME_USEPTRS41"),
];

/// `Cvar_InfoString(CVAR_SERVERINFO)`, capture cs 0. Alphabetical, as the
/// cvar table iterates.
pub fn serverinfo(cfg: &ServerConfig) -> Info {
    let mut i = Info::new();
    i.set("g_gametype", &cfg.gametype)
        .set("gamename", "main")
        .set("mapname", &cfg.map)
        .set("protocol", PROTOCOL_V1.version)
        .set("shortversion", "1.1")
        .set("sv_allowAnonymous", 0)
        .set("sv_floodProtect", 1)
        .set("sv_hostname", &cfg.hostname)
        .set("sv_maxclients", cfg.max_clients)
        .set("sv_maxPing", 0)
        .set("sv_maxRate", 0)
        .set("sv_minPing", 0)
        .set("sv_privateClients", 0)
        .set("sv_pure", 0);
    i
}

/// `Cvar_InfoString_Big(CVAR_SYSTEMINFO)`, capture cs 1, minus the pak lists.
/// Must stay under `MAX_INFO_STRING` with `sv_serverid` intact; the overflow
/// is in docs/research/cod11-server-handshake.md, "Configstring 1, systeminfo".
pub fn systeminfo(server_id: u8) -> Info {
    let mut i = Info::new();
    i.set("bg_fallDamageMaxHeight", 480)
        .set("bg_fallDamageMinHeight", 256)
        .set("g_synchronousClients", 0)
        .set("pmove_fixed", 0)
        .set("pmove_msec", 8)
        .set("sv_cheats", 0)
        .set("sv_pure", 0)
        .set("sv_serverid", server_id)
        .set("timescale", 1);
    i
}

/// The full 2048-slot table for a fresh map.
pub fn static_configstrings(cfg: &ServerConfig, server_id: u8) -> Vec<String> {
    let mut cs = vec![String::new(); PROTOCOL_V1.max_configstrings];
    // Names an out-of-range literal instead of a bare index panic.
    debug_assert!(STATIC.iter().all(|&(i, _)| i < cs.len()));
    cs[0] = serverinfo(cfg).to_string();
    cs[1] = systeminfo(server_id).to_string();
    for &(i, s) in STATIC {
        cs[i] = s.to_string();
    }
    cs
}

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
    Menu,
    Localized,
    Shader,
}

impl CsRange {
    pub const ALL: [CsRange; 9] = [
        CsRange::StatusIcon,
        CsRange::HeadIcon,
        CsRange::Tag,
        CsRange::Model,
        CsRange::SoundAlias,
        CsRange::Effect,
        CsRange::Menu,
        CsRange::Localized,
        CsRange::Shader,
    ];

    /// Inclusive.
    pub fn bounds(self) -> (usize, usize) {
        match self {
            CsRange::StatusIcon => (21, 28),
            CsRange::HeadIcon => (29, 43),
            CsRange::Tag => (109, 139),
            CsRange::Model => (269, 523),
            CsRange::SoundAlias => (525, 779),
            CsRange::Effect => (781, 843),
            CsRange::Menu => (1180, 1211),
            CsRange::Localized => (1245, 1499),
            CsRange::Shader => (1501, 1627),
        }
    }
}

/// One next-free cursor per range, named rather than kept in a `CsRange::ALL`-
/// order array: `next_mut`'s match is exhaustive, so a range added to the enum
/// without a matching field here is a compile error instead of a silent
/// misindex into the wrong allocator.
///
/// `Default` is hand-written rather than derived: derived, it would give
/// all-zero cursors and the first allocation would land in configstring 0,
/// the serverinfo slot.
pub struct Allocators {
    status_icon: usize,
    head_icon: usize,
    tag: usize,
    model: usize,
    sound_alias: usize,
    effect: usize,
    menu: usize,
    localized: usize,
    shader: usize,
}

impl Default for Allocators {
    fn default() -> Self {
        Allocators::new()
    }
}

impl Allocators {
    pub fn new() -> Self {
        Allocators {
            status_icon: CsRange::StatusIcon.bounds().0,
            head_icon: CsRange::HeadIcon.bounds().0,
            tag: CsRange::Tag.bounds().0,
            model: CsRange::Model.bounds().0,
            sound_alias: CsRange::SoundAlias.bounds().0,
            effect: CsRange::Effect.bounds().0,
            menu: CsRange::Menu.bounds().0,
            localized: CsRange::Localized.bounds().0,
            shader: CsRange::Shader.bounds().0,
        }
    }

    fn next_mut(&mut self, range: CsRange) -> &mut usize {
        match range {
            CsRange::StatusIcon => &mut self.status_icon,
            CsRange::HeadIcon => &mut self.head_icon,
            CsRange::Tag => &mut self.tag,
            CsRange::Model => &mut self.model,
            CsRange::SoundAlias => &mut self.sound_alias,
            CsRange::Effect => &mut self.effect,
            CsRange::Menu => &mut self.menu,
            CsRange::Localized => &mut self.localized,
            CsRange::Shader => &mut self.shader,
        }
    }

    /// Intern-or-append: an existing name returns its slot, a new one takes
    /// the next free slot, an exhausted range is an error.
    pub fn index(
        &mut self,
        cs: &mut [String],
        range: CsRange,
        name: &str,
    ) -> Result<usize, ErrorKind> {
        let (lo, hi) = range.bounds();
        let next = *self.next_mut(range);
        if let Some(slot) = (lo..next).find(|s| cs[*s] == name) {
            return Ok(slot);
        }
        if next > hi {
            return Err(ErrorKind::BadType("configstring range exhausted"));
        }
        cs[next] = name.to_string();
        *self.next_mut(range) = next + 1;
        Ok(next)
    }

    /// What `G_LocalizedStringIndex` (0x65e30) answers: the 1-based index
    /// within the localized range rather than the configstring slot, since
    /// its scan starts at `i = 1` and it writes `0x4dc + i`. That index is
    /// what a `hudelem_t`'s `text` and `label` carry, and a client resolves
    /// it back through configstring `1244 + n`.
    pub fn localized_index(&mut self, cs: &mut [String], name: &str) -> Result<i32, ErrorKind> {
        let slot = self.index(cs, CsRange::Localized, name)?;
        Ok((slot - CsRange::Localized.bounds().0) as i32 + 1)
    }

    /// The same for `G_ShaderIndex` (0x65ee8), whose scan starts at `i = 1`
    /// too: `setShader`'s material index, resolved through configstring
    /// `1500 + n`.
    pub fn shader_index(&mut self, cs: &mut [String], name: &str) -> Result<i32, ErrorKind> {
        let slot = self.index(cs, CsRange::Shader, name)?;
        Ok((slot - CsRange::Shader.bounds().0) as i32 + 1)
    }
}

/// `GScr_GetScriptMenuIndex` (0x5c73c): the offset within `CsRange::Menu` of
/// the slot holding `name`, scanning the whole range for an exact match.
/// That offset, not the configstring number, is what `openMenu` puts on the
/// wire. `None` is retail's `Menu '%s' was not precached` error.
pub fn script_menu_index(cs: &[String], name: &str) -> Option<usize> {
    let (lo, hi) = CsRange::Menu.bounds();
    (lo..=hi).find(|s| cs[*s] == name).map(|s| s - lo)
}

/// The inverse: what `Cmd_MenuResponse_f` (0x486d8) reads back out of
/// configstring `CsRange::Menu.start + index` to name the menu in its
/// `menuresponse` notify. Empty when nothing precached that slot, which is
/// the empty string retail passes on too.
pub fn script_menu_name(cs: &[String], index: usize) -> &str {
    let (lo, hi) = CsRange::Menu.bounds();
    cs.get(lo + index)
        .filter(|_| lo + index <= hi)
        .map_or("", |s| s.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Intern-or-append, mirroring `G_ModelIndex` and its siblings: the same
    /// name twice is one slot, a new name takes the next
    /// (docs/design/2026-08-28-gsc-gameplay-design.md, "Configstrings stop
    /// being a static table").
    #[test]
    fn an_allocator_interns_and_appends() {
        let mut cs = vec![String::new(); 2048];
        let mut a = Allocators::new();
        let first = a.index(&mut cs, CsRange::Model, "xmodel/fx").unwrap();
        assert_eq!(first, 269);
        assert_eq!(cs[269], "xmodel/fx");
        assert_eq!(a.index(&mut cs, CsRange::Model, "xmodel/fx").unwrap(), 269);
        assert_eq!(
            a.index(&mut cs, CsRange::Model, "xmodel/other").unwrap(),
            270
        );
    }

    /// Ranges do not overlap, each starts where the research doc says, and
    /// none of them covers a slot the static table already fills: the first
    /// allocation into such a range would overwrite a configstring the
    /// client needs, silently.
    #[test]
    fn the_ranges_are_the_documented_ones_and_clear_of_the_static_table() {
        assert_eq!(CsRange::Model.bounds(), (269, 523));
        assert_eq!(CsRange::SoundAlias.bounds(), (525, 779));
        assert_eq!(CsRange::Effect.bounds(), (781, 843));
        assert_eq!(CsRange::Tag.bounds(), (109, 139));
        let mut seen: Vec<(usize, usize)> = Vec::new();
        for r in CsRange::ALL {
            let (lo, hi) = r.bounds();
            assert!(lo <= hi);
            assert!(
                seen.iter().all(|(a, b)| hi < *a || lo > *b),
                "{r:?} overlaps a range already declared"
            );
            seen.push((lo, hi));
            // 0 and 1 are set by `static_configstrings` outside the
            // `STATIC` list (serverinfo, systeminfo). 179/243/1501 used to
            // be as well, but the layout-image write is gone now that
            // `Shader` genuinely starts at 1501.
            for i in STATIC.iter().map(|&(i, _)| i).chain([0, 1]) {
                assert!(
                    i < lo || i > hi,
                    "{r:?} covers configstring {i}, which the static table sets"
                );
            }
        }
    }

    /// Ranges are the ones the indexers in `game.mp.i386.so` walk, not the
    /// ones the design table guessed: the icon and menu indexers start their
    /// scan at `i = 0`, the localized-string and shader ones at `i = 1`, and
    /// applying one convention to all five is how three of them ended up a
    /// slot too high.
    #[test]
    fn the_ranges_are_the_ones_the_indexers_walk() {
        assert_eq!(CsRange::Tag.bounds(), (109, 139));
        assert_eq!(CsRange::StatusIcon.bounds(), (21, 28));
        assert_eq!(CsRange::HeadIcon.bounds(), (29, 43));
        assert_eq!(CsRange::Model.bounds(), (269, 523));
        assert_eq!(CsRange::SoundAlias.bounds(), (525, 779));
        assert_eq!(CsRange::Effect.bounds(), (781, 843));
        assert_eq!(CsRange::Menu.bounds(), (1180, 1211));
        assert_eq!(CsRange::Localized.bounds(), (1245, 1499));
        assert_eq!(CsRange::Shader.bounds(), (1501, 1627));
        assert_eq!(CsRange::ALL.len(), 9);
    }

    /// Exhausting a range is a hard error, not a wrap or a silent overwrite:
    /// a map that precaches 256 models is broken and should say so.
    #[test]
    fn an_exhausted_range_is_an_error() {
        let mut cs = vec![String::new(); 2048];
        let mut a = Allocators::new();
        let (lo, hi) = CsRange::Effect.bounds();
        for i in 0..=(hi - lo) {
            a.index(&mut cs, CsRange::Effect, &format!("fx{i}"))
                .unwrap();
        }
        assert!(a.index(&mut cs, CsRange::Effect, "one too many").is_err());
    }
}
