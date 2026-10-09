//! The configstrings a server sets before any entity exists, from
//! docs/research/cod11-server-handshake.md ("What a minimal server has to set").
//! Entity-driven slots (3, 8, 11, 12, 269.., 524.., 781..) stay empty.

use crate::server::ServerConfig;
use vcod_common::net::connectionless::Info;
use vcod_common::net::protocol::{CS_LEVEL_START_TIME, CS_SERVERINFO, CS_SYSTEMINFO, PROTOCOL_V1};
use vcod_common::pk3::PakInfo;
use vcod_common::pmove::FallHeights;
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
    (CS_LEVEL_START_TIME, "0"),
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

/// The four pak cvars `SV_SpawnServer` sets (docs/research/cod11-map-cycle.md,
/// section 3 step 23), as lists so the systeminfo can trim them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PakLists {
    /// `sv_pure`; without it `sv_paks` and `sv_pakNames` are empty.
    pub pure: bool,
    /// `FS_LoadedPakChecksums` / `FS_LoadedPakNames`: every non-localized
    /// pak, checksum and bare name.
    pub paks: Vec<(i32, String)>,
    /// `FS_ReferencedPakChecksums` / `FS_ReferencedPakNames`: every pak,
    /// localized ones included, checksum and `<game>/<name>`.
    pub referenced: Vec<(i32, String)>,
}

impl PakLists {
    /// The lists for the search path `paks` (highest priority first).
    pub fn new<'a>(pure: bool, paks: impl IntoIterator<Item = &'a PakInfo>) -> Self {
        let mut lists = PakLists {
            pure,
            ..PakLists::default()
        };
        for p in paks {
            if pure && !p.localized {
                lists.paks.push((p.checksum, p.name.clone()));
            }
            lists.referenced.push((p.checksum, p.qualified_name()));
        }
        lists
    }
}

/// `"%i "` per checksum, names joined by single spaces: the lists' shape on
/// the wire, trailing space included.
fn sums_and_names(list: &[(i32, String)]) -> (String, String) {
    let sums = list.iter().map(|(c, _)| format!("{c} ")).collect();
    let names = list
        .iter()
        .map(|(_, n)| n.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    (sums, names)
}

/// A serverinfo string with its `sv_pure` value replaced in place.
pub fn with_sv_pure(info: &str, pure: bool) -> String {
    let key = "\\sv_pure\\";
    let Some(at) = info.find(key) else {
        return info.to_string();
    };
    let start = at + key.len();
    let end = info[start..].find('\\').map_or(info.len(), |e| start + e);
    format!("{}{}{}", &info[..start], u8::from(pure), &info[end..])
}

/// `MAX_INFO_STRING` less the terminator.
const MAX_INFO_CHARS: usize = 1023;

/// `Cvar_InfoString_Big(CVAR_SYSTEMINFO)`, capture cs 1. The fall bounds are
/// the cvars' current values, which a client predicts its landings with;
/// `cheats` is `sv_cheats`, which `devmap` sets. An empty pak list leaves its
/// key out, as an empty cvar does.
///
/// Retail overflows `MAX_INFO_STRING` with enough paks and loses
/// `sv_serverid` off the end (docs/research/cod11-server-handshake.md,
/// "Configstring 1, systeminfo"); vcod drops paks off the ends of the lists
/// instead, the referenced pair first.
pub fn systeminfo(server_id: u8, fall: FallHeights, cheats: bool, paks: &PakLists) -> Info {
    let mut paks = paks.clone();
    let mut warned = false;
    loop {
        let (sums, names) = sums_and_names(&paks.paks);
        let (ref_sums, ref_names) = sums_and_names(&paks.referenced);
        let mut i = Info::new();
        i.set("bg_fallDamageMaxHeight", fall.max)
            .set("bg_fallDamageMinHeight", fall.min)
            .set("g_synchronousClients", 0)
            .set("pmove_fixed", 0)
            .set("pmove_msec", 8)
            .set("sv_cheats", u8::from(cheats));
        if !names.is_empty() {
            i.set("sv_pakNames", names).set("sv_paks", sums);
        }
        i.set("sv_pure", u8::from(paks.pure));
        if !ref_names.is_empty() {
            i.set("sv_referencedPakNames", ref_names)
                .set("sv_referencedPaks", ref_sums);
        }
        i.set("sv_serverid", server_id).set("timescale", 1);
        if i.to_string().len() <= MAX_INFO_CHARS
            || (paks.referenced.is_empty() && paks.paks.is_empty())
        {
            return i;
        }
        if !warned {
            log::warn!("systeminfo over {MAX_INFO_CHARS} chars: trimming the pak lists");
            warned = true;
        }
        if paks.referenced.len() >= paks.paks.len() {
            paks.referenced.pop();
        } else {
            paks.paks.pop();
        }
    }
}

/// The full 2048-slot table for a fresh map.
pub fn static_configstrings(
    cfg: &ServerConfig,
    server_id: u8,
    fall: FallHeights,
    cheats: bool,
    paks: &PakLists,
) -> Vec<String> {
    let mut cs = vec![String::new(); PROTOCOL_V1.max_configstrings];
    // Names an out-of-range literal instead of a bare index panic.
    debug_assert!(STATIC.iter().all(|&(i, _)| i < cs.len()));
    cs[CS_SERVERINFO] = serverinfo(cfg)
        .set("sv_pure", u8::from(paks.pure))
        .to_string();
    cs[CS_SYSTEMINFO] = systeminfo(server_id, fall, cheats, paks).to_string();
    for &(i, s) in STATIC {
        cs[i] = s.to_string();
    }
    cs
}

pub use vcod_common::net::protocol::CsRange;

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

/// A model's 1-based index inside [`CsRange::Model`], the way the range's own
/// indexer numbers it, or 0 for a name nothing has precached. What an entity
/// state's `index` field carries for a model.
pub fn model_index(cs: &[String], name: &str) -> i32 {
    if name.is_empty() {
        return 0;
    }
    let (first, last) = CsRange::Model.bounds();
    cs.get(first..=last)
        .and_then(|range| range.iter().position(|s| s == name))
        .map_or(0, |slot| slot as i32 + 1)
}

/// The index of a model a weapon file names. `GameHost::register_item`
/// precaches the file's value verbatim, `xmodel/` prefix and all, where
/// `WeaponDef` strips the prefix, so a lookup by the parsed name has to put
/// it back.
pub fn weapon_model_index(cs: &[String], name: &str) -> i32 {
    if name.is_empty() {
        return 0;
    }
    model_index(cs, &format!("xmodel/{name}"))
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

    /// Allocators that continue a table an earlier level filled: each
    /// range's next free slot is the one past its last non-empty entry, so a
    /// name the last level indexed is found where it was and a new one lands
    /// after everything. This is what a `map_restart` needs, since the
    /// engine keeps `sv.configstrings` across one (map-cycle doc, 4.6).
    pub fn seeded(cs: &[String]) -> Self {
        let mut a = Allocators::new();
        for range in CsRange::ALL {
            let (lo, hi) = range.bounds();
            let used = (lo..=hi)
                .rev()
                .find(|&i| cs.get(i).is_some_and(|s| !s.is_empty()));
            *a.next_mut(range) = used.map_or(lo, |i| i + 1);
        }
        a
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

    /// The same for `G_EffectIndex` (0x65fa4), scanning from `i = 1` and
    /// writing `0x30c + i`: the effect id `loadFX` returns and `EV_PLAY_FX`
    /// carries, resolved through configstring `780 + n`.
    pub fn effect_index(&mut self, cs: &mut [String], name: &str) -> Result<i32, ErrorKind> {
        let slot = self.index(cs, CsRange::Effect, name)?;
        Ok((slot - CsRange::Effect.bounds().0) as i32 + 1)
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

use vcod_common::net::protocol::CS_SOUNDS;

/// The index an already registered sound alias travels as, in
/// `EV_SOUND_ALIAS`'s parm and in `loopSound`; `None` if nothing indexed it.
pub fn sound_alias_index(cs: &[String], name: &str) -> Option<i32> {
    let (lo, hi) = CsRange::SoundAlias.bounds();
    (lo..=hi)
        .find(|&i| cs.get(i).is_some_and(|s| s == name))
        .map(|i| (i - CS_SOUNDS) as i32)
}

/// The first of the 32 hint-string configstrings `G_GetHintStringIndex`
/// (0x5a238) fills (`docs/research/cod11-gsc-object-model.md`, the
/// hint-string paragraph).
use vcod_common::net::protocol::CS_HINT_STRINGS;
const MAX_HINT_STRINGS: usize = 32;

/// The slot within the hint-string range holding `name`, which is what
/// `serverCursorHintString` carries: the stock turret's `CGAME_USEMG42` is 0.
pub fn hint_string_index(cs: &[String], name: &str) -> Option<i32> {
    (0..MAX_HINT_STRINGS)
        .find(|i| cs.get(CS_HINT_STRINGS + i).is_some_and(|s| s == name))
        .map(|i| i as i32)
}

/// `G_GetHintStringIndex` (0x5a238) for `setHintString`: the slot already
/// holding `name`, else the first empty one, claimed. `None` when all 32 are
/// taken by other strings, retail's "Too many different hintstring values".
/// The scan order is VERIFIED by the mp_chateau `re` capture, whose two
/// objectives' strings land in 1214 and 1215 behind the engine's 1212/1213
/// (docs/research/cod11-gametypes-re-bel.md 4).
pub fn hint_string_alloc(cs: &mut [String], name: &str) -> Option<i32> {
    for i in 0..MAX_HINT_STRINGS {
        let slot = &mut cs[CS_HINT_STRINGS + i];
        if slot == name {
            return Some(i as i32);
        }
        if slot.is_empty() {
            *slot = name.to_string();
            return Some(i as i32);
        }
    }
    None
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

    fn pak(name: &str, game: &str, checksum: i32) -> PakInfo {
        PakInfo {
            path: format!("{game}/{name}.pk3").into(),
            game: game.into(),
            name: name.into(),
            localized: name.starts_with("localized_"),
            checksum,
            crcs: Vec::new(),
        }
    }

    /// The pak half of the retail capture under `sv_pure 1` (2026-10-09,
    /// 1.1d on a stock 1.1 install, docs/research/cod11-server-handshake.md,
    /// "Pak checksums"), trimmed to three paks.
    #[test]
    fn systeminfo_carries_the_pak_lists_as_retail_does() {
        let paks = [
            pak("pak6", "main", 1_252_304_247),
            pak("pak5", "main", 77_111_478),
            pak("localized_english_pak1", "main", -961_133_319),
        ];
        let fall = FallHeights::default();
        let pure = systeminfo(0x10, fall, false, &PakLists::new(true, &paks)).to_string();
        assert!(pure.contains(
            "\\sv_cheats\\0\\sv_pakNames\\pak6 pak5\\sv_paks\\1252304247 77111478 \\sv_pure\\1\
             \\sv_referencedPakNames\\main/pak6 main/pak5 main/localized_english_pak1\
             \\sv_referencedPaks\\1252304247 77111478 -961133319 \\sv_serverid\\16\\timescale\\1"
        ));
        // `sv_pure 0` empties the loaded pair, which leaves the info string.
        let impure = systeminfo(0x10, fall, false, &PakLists::new(false, &paks)).to_string();
        assert!(!impure.contains("sv_paks") && !impure.contains("sv_pakNames"));
        assert!(impure.contains("\\sv_pure\\0\\sv_referencedPakNames\\main/pak6"));
    }

    /// Too many paks for `MAX_INFO_STRING` cost paks, never `sv_serverid`.
    #[test]
    fn systeminfo_trims_paks_to_keep_the_server_id() {
        let paks: Vec<PakInfo> = (0..60)
            .map(|i| {
                pak(
                    &format!("zzz_some_long_map_pak_{i}"),
                    "main",
                    -1_000_000_000 - i,
                )
            })
            .collect();
        let info = systeminfo(
            0x10,
            FallHeights::default(),
            false,
            &PakLists::new(true, &paks),
        )
        .to_string();
        assert!(info.len() <= MAX_INFO_CHARS, "{}", info.len());
        assert!(info.ends_with("\\sv_serverid\\16\\timescale\\1"));
        let names = vcod_common::net::info_value_for_key(&info, "sv_referencedPakNames").unwrap();
        let sums = vcod_common::net::info_value_for_key(&info, "sv_referencedPaks").unwrap();
        assert_eq!(
            names.split_whitespace().count(),
            sums.split_whitespace().count()
        );
        assert!(names.starts_with("main/zzz_some_long_map_pak_0 "));
    }

    #[test]
    fn sv_pure_is_replaced_in_place() {
        assert_eq!(
            with_sv_pure("\\sv_privateClients\\0\\sv_pure\\0", true),
            "\\sv_privateClients\\0\\sv_pure\\1"
        );
        assert_eq!(with_sv_pure("\\a\\b", true), "\\a\\b");
    }

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
