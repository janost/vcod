//! The configstrings a server sets before any entity exists, from
//! docs/research/cod11-server-handshake.md ("What a minimal server has to set").
//! Entity-driven slots (3, 8, 11, 12, 269.., 524.., 781..) stay empty.

use crate::server::ServerConfig;
use vcod_common::net::connectionless::Info;
use vcod_common::net::protocol::{
    CS_LEVEL_START_TIME, CS_SERVERINFO, CS_SYSTEMINFO, CsLayout, PROTOCOL_V1, Protocol,
};
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

/// [`STATIC`] at United Offensive's slots (coduo-diff.md 7): its own weapon
/// list, the fog, the start time and the winner one slot up, and five hint
/// strings from 1365. A vehicle's own hint string follows when it spawns
/// (coduo-vehicles.md 10.2).
pub const STATIC_V22: &[(usize, &str)] = &[
    (2, "cod"),
    (7, crate::defs::UO.weapons),
    // `G_InitGame` clears the fog (`G_setfog("0")`, game.mp.uo 0x586cf).
    (13, "0"),
    (14, "0"),
    (21, "\\winner\\0"),
    (1365, "CGAME_USEMG42"),
    (1366, "CGAME_USEflak88"),
    (1367, "GMI_CGAME_USE50CAL"),
    (1368, "GMI_CGAME_USESG43"),
    (1369, "CGAME_USEPTRS41"),
];

/// The static table a server speaking `p` starts from.
pub fn static_table(p: &Protocol) -> &'static [(usize, &'static str)] {
    if p.is_uo() { STATIC_V22 } else { STATIC }
}

/// `Cvar_InfoString(CVAR_SERVERINFO)`, capture cs 0. Alphabetical, as the
/// cvar table iterates.
pub fn serverinfo(cfg: &ServerConfig) -> Info {
    let mut i = Info::new();
    i.set("g_gametype", &cfg.gametype)
        .set("gamename", "main")
        .set("mapname", &cfg.map)
        .set("protocol", PROTOCOL_V1.transport.version)
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
    /// `fs_game`, the directory layered over the base; empty for none.
    pub fs_game: String,
}

impl PakLists {
    /// The lists for the search path `paks` (highest priority first), as a
    /// 1.1 server builds them: every pak is referenced.
    pub fn new<'a>(pure: bool, paks: impl IntoIterator<Item = &'a PakInfo>) -> Self {
        Self::of(pure, paks.into_iter().map(|p| (p, true)), &PROTOCOL_V1, "")
    }

    /// The lists a server speaking `p` builds off the search path `paks`
    /// (highest priority first, each with whether the level read a file
    /// from it); `basegame` is `fs_basegame`.
    pub fn of<'a>(
        pure: bool,
        paks: impl IntoIterator<Item = (&'a PakInfo, bool)>,
        p: &Protocol,
        basegame: &str,
    ) -> Self {
        let mut lists = PakLists {
            pure,
            ..PakLists::default()
        };
        for (pak, read) in paks {
            if pure && !pak.localized {
                lists.paks.push((pak.checksum, pak.name.clone()));
            }
            if lists_referenced(p, pak, read, basegame) {
                lists.referenced.push((pak.checksum, pak.qualified_name()));
            }
        }
        lists
    }
}

/// Whether `FS_ReferencedPakChecksums` / `FS_ReferencedPakNames` list
/// `pak` (docs/research/cod11-server-handshake.md, "The four cvars"): 1.1
/// lists every pak, 1.5 a pak a file was read from or one outside `main`,
/// UO one read from or outside both `main` and `fs_basegame`. Each
/// directory test is `Q_stricmpn` over the other name's length.
fn lists_referenced(p: &Protocol, pak: &PakInfo, read: bool, basegame: &str) -> bool {
    let under = |dir: &str| {
        pak.game
            .get(..dir.len())
            .is_some_and(|g| g.eq_ignore_ascii_case(dir))
    };
    if p.is_v1() {
        true
    } else if p.transport.version == vcod_common::net::protocol::PROTOCOL_V6.transport.version {
        read || !under("main")
    } else {
        read || !(under("main") || under(basegame))
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

/// `MAX_INFO_STRING` less the terminator: where 1.1's `SV_SpawnServer` cuts
/// its systeminfo.
const MAX_INFO_CHARS: usize = 1023;
/// `BIG_INFO_STRING` less the terminator: 1.5's and UO's bound.
const BIG_INFO_CHARS: usize = 8191;

/// `Cvar_InfoString_Big(CVAR_SYSTEMINFO)`, capture cs 1. The fall bounds are
/// the cvars' current values, which a client predicts its landings with;
/// `cheats` is `sv_cheats`, which `devmap` sets. An empty pak list leaves its
/// key out, as an empty cvar does.
///
/// 1.1 copies it into a `MAX_INFO_STRING` buffer, so with enough paks it
/// loses `sv_serverid` off the end; 1.5 and UO keep `BIG_INFO_STRING`
/// (docs/research/cod11-server-handshake.md, "Configstring 1, systeminfo").
/// Past the bound vcod drops paks off the ends of the lists instead, the
/// referenced pair first.
pub fn systeminfo(
    server_id: u8,
    fall: FallHeights,
    cheats: bool,
    paks: &PakLists,
    v6: Option<&[(String, String)]>,
) -> Info {
    let mut paks = paks.clone();
    loop {
        let (sums, names) = sums_and_names(&paks.paks);
        let (ref_sums, ref_names) = sums_and_names(&paks.referenced);
        let mut pairs: Vec<(String, String)> = vec![
            ("bg_fallDamageMaxHeight".into(), fall.max.to_string()),
            ("bg_fallDamageMinHeight".into(), fall.min.to_string()),
            ("g_synchronousClients".into(), "0".into()),
            ("pmove_fixed".into(), "0".into()),
            ("pmove_msec".into(), "8".into()),
            ("sv_cheats".into(), u8::from(cheats).to_string()),
            ("sv_pure".into(), u8::from(paks.pure).to_string()),
            ("sv_serverid".into(), server_id.to_string()),
        ];
        if !paks.fs_game.is_empty() {
            pairs.push(("fs_game".into(), paks.fs_game.clone()));
        }
        if !names.is_empty() {
            pairs.push(("sv_pakNames".into(), names));
            pairs.push(("sv_paks".into(), sums));
        }
        if !ref_names.is_empty() {
            pairs.push(("sv_referencedPakNames".into(), ref_names));
            pairs.push(("sv_referencedPaks".into(), ref_sums));
        }
        // 1.5 mirrors `timescale` instead and adds its own rows.
        match v6 {
            None => pairs.push(("timescale".into(), "1".into())),
            Some(extra) => pairs.extend(extra.iter().cloned()),
        }
        // `Cvar_InfoString_Big` walks the cvars in folded-name order.
        pairs.sort_by_key(|(k, _)| k.to_ascii_lowercase());
        let mut i = Info::new();
        for (k, v) in &pairs {
            i.set(k, v);
        }
        let bound = if v6.is_some() {
            BIG_INFO_CHARS
        } else {
            MAX_INFO_CHARS
        };
        if i.to_string().len() <= bound || (paks.referenced.is_empty() && paks.paks.is_empty()) {
            return i;
        }
        // Rebuilt every frame the fall bounds are read: say it once.
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log::warn!("systeminfo over {bound} chars: trimming the pak lists"));
        // A stock pak is never downloaded, so its referenced entry is the
        // cheapest to lose; `sv_paks` is what a pure client keeps loaded.
        // 1.5's rows ride a UO table too; its stock paks are either's.
        let stock: &[&Protocol] = if v6.is_some() {
            &[
                &vcod_common::net::protocol::PROTOCOL_V6,
                &vcod_common::net::protocol::PROTOCOL_V22,
            ]
        } else {
            &[&PROTOCOL_V1]
        };
        if let Some(i) = paks.referenced.iter().rposition(|(_, n)| {
            stock
                .iter()
                .any(|p| vcod_common::net::download::is_stock_pak(n, &p.transport))
        }) {
            paks.referenced.remove(i);
        } else if paks.referenced.len() >= paks.paks.len() {
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
    v6: Option<&[(String, String)]>,
    p: &Protocol,
) -> Vec<String> {
    let mut cs = vec![String::new(); p.transport.max_configstrings];
    let table = static_table(p);
    // Names an out-of-range literal instead of a bare index panic.
    debug_assert!(table.iter().all(|&(i, _)| i < cs.len()));
    cs[CS_SERVERINFO] = serverinfo(cfg)
        .set("sv_pure", u8::from(paks.pure))
        .to_string();
    cs[CS_SYSTEMINFO] = systeminfo(server_id, fall, cheats, paks, v6).to_string();
    for &(i, s) in table {
        cs[i] = s.to_string();
    }
    cs
}

pub use vcod_common::net::protocol::CsRange;

/// The game's configstring table. Every write goes through [`Self::set`],
/// `SV_SetConfigstring`'s shape: a write that leaves its slot as it was is
/// dropped, and one that changes it is logged in order, which is the order
/// retail queues its `d`s in (a restart's burst, map-cycle doc 4.5).
#[derive(Clone, Debug, Default)]
pub struct ConfigStrings {
    table: Vec<String>,
    writes: Vec<(usize, String)>,
    /// Each write's [`next_write_seq`], beside `writes`.
    seqs: Vec<u64>,
}

thread_local! {
    static WRITE_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One step of the order a game's configstring writes and server commands
/// share (`ConfigStrings::set`, `game::commands::ClientCommands::push`).
pub fn next_write_seq() -> u64 {
    WRITE_SEQ.with(|s| {
        let n = s.get() + 1;
        s.set(n);
        n
    })
}

impl ConfigStrings {
    pub fn new(table: Vec<String>) -> Self {
        ConfigStrings {
            table,
            writes: Vec::new(),
            seqs: Vec::new(),
        }
    }

    pub fn set(&mut self, index: usize, value: impl Into<String>) {
        let value = value.into();
        if self.table[index] != value {
            self.table[index].clone_from(&value);
            self.writes.push((index, value));
            self.seqs.push(next_write_seq());
        }
    }

    /// The writes since the last [`Self::take_writes`], oldest first.
    pub fn writes(&self) -> &[(usize, String)] {
        &self.writes
    }

    pub fn take_writes(&mut self) -> Vec<(usize, String)> {
        self.seqs.clear();
        std::mem::take(&mut self.writes)
    }

    /// [`Self::take_writes`] with each write's [`next_write_seq`].
    pub fn take_writes_stamped(&mut self) -> Vec<(u64, usize, String)> {
        let seqs = std::mem::take(&mut self.seqs);
        seqs.into_iter()
            .zip(std::mem::take(&mut self.writes))
            .map(|(s, (i, v))| (s, i, v))
            .collect()
    }
}

impl std::ops::Deref for ConfigStrings {
    type Target = [String];
    fn deref(&self) -> &[String] {
        &self.table
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
    layout: &'static CsLayout,
    status_icon: usize,
    head_icon: usize,
    tag: usize,
    model: usize,
    sound_alias: usize,
    effect: usize,
    fx_tag: usize,
    shell_shock: usize,
    menu: usize,
    localized: usize,
    shader: usize,
    /// CoD4's `constantConfigStrings`, `(slot, default)`
    /// ([`Self::with_constants`]); `None` on CoD1 and CoD2.
    constants: Option<&'static [(u16, &'static str)]>,
}

/// CoD4's first case-blind slot (`CS_CASE_INSENSITIVE_BEGIN`):
/// the constants rule stores names from here on lowercased.
const CASE_INSENSITIVE_FROM: usize = 821;

impl Default for Allocators {
    fn default() -> Self {
        Allocators::new(&PROTOCOL_V1.cs)
    }
}

/// A model's 1-based index inside [`CsRange::Model`], the way the range's own
/// indexer numbers it, or 0 for a name nothing has precached. What an entity
/// state's `index` field carries for a model.
pub fn model_index(cs: &[String], layout: &CsLayout, name: &str) -> i32 {
    if name.is_empty() {
        return 0;
    }
    let (first, last) = layout.bounds(CsRange::Model);
    cs.get(first..=last)
        .and_then(|range| range.iter().position(|s| s.eq_ignore_ascii_case(name)))
        .map_or(0, |slot| slot as i32 + 1)
}

/// The index of a model a weapon file names. `GameHost::register_item`
/// precaches the file's value verbatim, `xmodel/` prefix and all, where
/// `WeaponDef` strips the prefix, so a lookup by the parsed name has to put
/// it back.
pub fn weapon_model_index(cs: &[String], layout: &CsLayout, name: &str) -> i32 {
    if name.is_empty() {
        return 0;
    }
    model_index(cs, layout, &format!("xmodel/{name}"))
}

impl Allocators {
    pub fn new(layout: &'static CsLayout) -> Self {
        let first = |r: CsRange| layout.bounds(r).0;
        Allocators {
            layout,
            status_icon: first(CsRange::StatusIcon),
            head_icon: first(CsRange::HeadIcon),
            tag: first(CsRange::Tag),
            model: first(CsRange::Model),
            sound_alias: first(CsRange::SoundAlias),
            effect: first(CsRange::Effect),
            fx_tag: first(CsRange::FxTag),
            shell_shock: first(CsRange::ShellShock),
            menu: first(CsRange::Menu),
            localized: first(CsRange::Localized),
            shader: first(CsRange::Shader),
            constants: None,
        }
    }

    /// Where this table's blocks sit.
    pub fn layout(&self) -> &'static CsLayout {
        self.layout
    }

    /// Allocators that continue a table an earlier level filled: each
    /// range's next free slot is the one past its last non-empty entry, so a
    /// name the last level indexed is found where it was and a new one lands
    /// after everything. This is what a `map_restart` needs, since the
    /// engine keeps `sv.configstrings` across one (map-cycle doc, 4.6).
    pub fn seeded(cs: &[String], layout: &'static CsLayout) -> Self {
        let mut a = Allocators::new(layout);
        for range in CsRange::ALL {
            let (lo, hi) = layout.bounds(range);
            let used = (lo..=hi)
                .rev()
                .find(|&i| cs.get(i).is_some_and(|s| !s.is_empty()));
            *a.next_mut(range) = used.map_or(lo, |i| i + 1);
        }
        a
    }

    /// CoD4's `G_FindConfigstringIndex` over a table that starts empty
    /// (cod4-gsc.md 5): a name is found anywhere in its range; a new one
    /// whose `constantConfigStrings` slot lies in the range takes that slot,
    /// any other the first empty slot no constant names (the first empty
    /// one once those run out). The menus and icons keep their own scan,
    /// the first empty slot from the block's base. The 1.7 capture's
    /// shellshocks (`default` and `concussion_grenade_mp` on their
    /// constants 1968 and 1969, `flashbang` on 1955), its localized strings
    /// (350 and 351 left empty) and its menus (`scoreboard` on 1977, not
    /// its constant 1986) are this rule.
    pub fn with_constants(
        layout: &'static CsLayout,
        constants: &'static [(u16, &'static str)],
    ) -> Self {
        let mut a = Allocators::new(layout);
        a.constants = Some(constants);
        a
    }

    /// The constants rule's slot for a new `name` in `range`.
    fn constant_slot(
        cs: &[String],
        constants: &[(u16, &str)],
        lo: usize,
        hi: usize,
        own_scan: bool,
        name: &str,
    ) -> Option<usize> {
        let empty = |i: usize| cs[i].is_empty();
        if own_scan {
            return (lo..=hi).find(|&i| empty(i));
        }
        let is_constant = |i: usize| constants.iter().any(|&(s, _)| s as usize == i);
        constants
            .iter()
            .find(|(s, d)| (lo..=hi).contains(&(*s as usize)) && d.eq_ignore_ascii_case(name))
            .map(|&(s, _)| s as usize)
            .or_else(|| (lo..=hi).find(|&i| empty(i) && !is_constant(i)))
            .or_else(|| (lo..=hi).find(|&i| empty(i)))
    }

    fn next_mut(&mut self, range: CsRange) -> &mut usize {
        match range {
            CsRange::StatusIcon => &mut self.status_icon,
            CsRange::HeadIcon => &mut self.head_icon,
            CsRange::Tag => &mut self.tag,
            CsRange::Model => &mut self.model,
            CsRange::SoundAlias => &mut self.sound_alias,
            CsRange::Effect => &mut self.effect,
            CsRange::FxTag => &mut self.fx_tag,
            CsRange::ShellShock => &mut self.shell_shock,
            CsRange::Menu => &mut self.menu,
            CsRange::Localized => &mut self.localized,
            CsRange::Shader => &mut self.shader,
        }
    }

    /// Intern-or-append: an existing name returns its slot, a new one takes
    /// the next free slot, an exhausted range is an error. Every indexer
    /// behind these ranges matches with `strcasecmp` or `Q_stricmp`
    /// (`G_ModelIndex` 0x66ed8, `G_FindConfigstringIndex` 0x66e30, the menu
    /// and icon builtins at 0x5a92c..0x5aae4), so a name in another case is
    /// the same name.
    pub fn index(
        &mut self,
        cs: &mut ConfigStrings,
        range: CsRange,
        name: &str,
    ) -> Result<usize, ErrorKind> {
        let hi = self.layout.bounds(range).1;
        if let Some(slot) = self.find_folded(cs, range, name) {
            return Ok(slot);
        }
        if let Some(constants) = self.constants {
            let lo = self.layout.bounds(range).0;
            let own_scan = matches!(
                range,
                CsRange::Menu | CsRange::StatusIcon | CsRange::HeadIcon
            );
            let slot = Self::constant_slot(cs, constants, lo, hi, own_scan, name)
                .ok_or(ErrorKind::BadType("configstring range exhausted"))?;
            // From the ambient slot (821) on, names are kept lowercased,
            // but for the sound aliases: the walk capture's `d 1344
            // US_1mc_team_deathmtch` keeps the alias's case (VERIFIED live,
            // cod4-gsc.md 7).
            if slot >= CASE_INSENSITIVE_FROM && range != CsRange::SoundAlias {
                cs.set(slot, name.to_ascii_lowercase());
            } else {
                cs.set(slot, name);
            }
            return Ok(slot);
        }
        let next = *self.next_mut(range);
        if next > hi {
            return Err(ErrorKind::BadType("configstring range exhausted"));
        }
        cs.set(next, name);
        *self.next_mut(range) = next + 1;
        Ok(next)
    }

    /// `G_ModelIndex` (0x66ed8), `G_ShaderIndex` (0x65ee8) and
    /// `G_LocalizedStringIndex` (0x65e30), with `G_SetModel` and
    /// `G_EntAttach` inlining the first: a name already in `range`
    /// (`strcasecmp`) is found at any time, and a new one takes a slot only
    /// while `level.initializing` (`level+0x1c`). Past it the new name is
    /// the script error [`not_precached`] spells and nothing is allocated
    /// (docs/research/cod11-gsc-language.md 9, "Indexing past the first
    /// wait").
    pub fn index_precached(
        &mut self,
        cs: &mut ConfigStrings,
        range: CsRange,
        name: &str,
        initializing: bool,
    ) -> Result<usize, ErrorKind> {
        if let Some(slot) = self.find_folded(cs, range, name) {
            return Ok(slot);
        }
        if !initializing {
            return Err(ErrorKind::Custom(not_precached(range, name)));
        }
        self.index(cs, range, name)
    }

    /// What `G_LocalizedStringIndex` (0x65e30) answers: the 1-based index
    /// within the localized range rather than the configstring slot, since
    /// its scan starts at `i = 1` and it writes `0x4dc + i`. That index is
    /// what a `hudelem_t`'s `text` and `label` carry, and a client resolves
    /// it back through configstring `1244 + n`. "" is 0.
    pub fn localized_index(
        &mut self,
        cs: &mut ConfigStrings,
        name: &str,
        initializing: bool,
    ) -> Result<i32, ErrorKind> {
        if name.is_empty() {
            return Ok(0);
        }
        let slot = self.index_precached(cs, CsRange::Localized, name, initializing)?;
        Ok((slot - self.layout.bounds(CsRange::Localized).0) as i32 + 1)
    }

    /// The same for `G_ShaderIndex` (0x65ee8), whose scan starts at `i = 1`
    /// too: `setShader`'s material index, resolved through configstring
    /// `1500 + n`.
    pub fn shader_index(
        &mut self,
        cs: &mut ConfigStrings,
        name: &str,
        initializing: bool,
    ) -> Result<i32, ErrorKind> {
        if name.is_empty() {
            return Ok(0);
        }
        let slot = self.index_precached(cs, CsRange::Shader, name, initializing)?;
        Ok((slot - self.layout.bounds(CsRange::Shader).0) as i32 + 1)
    }

    /// The same for `G_EffectIndex` (0x65fa4), scanning from `i = 1` and
    /// writing `0x30c + i`: the effect id `loadFX` returns and `EV_PLAY_FX`
    /// carries, resolved through configstring `780 + n`.
    pub fn effect_index(
        &mut self,
        cs: &mut ConfigStrings,
        name: &str,
        initializing: bool,
    ) -> Result<i32, ErrorKind> {
        if name.is_empty() {
            return Ok(0);
        }
        let slot = self.index_precached(cs, CsRange::Effect, name, initializing)?;
        Ok((slot - self.layout.bounds(CsRange::Effect).0) as i32 + 1)
    }

    /// A filled slot of `range` holding `name` under `strcasecmp`.
    pub(crate) fn find_folded(
        &mut self,
        cs: &[String],
        range: CsRange,
        name: &str,
    ) -> Option<usize> {
        let (lo, hi) = self.layout.bounds(range);
        let end = if self.constants.is_some() {
            hi + 1
        } else {
            *self.next_mut(range)
        };
        (lo..end).find(|&s| cs[s].eq_ignore_ascii_case(name))
    }

    /// `playFxOnTag`'s index for `"%02d%s"` of the effect id and the tag,
    /// 1-based from configstring 844 (game.mp 0x5b386): what
    /// `EV_PLAY_FX_ON_TAG` carries and cgame reads back at `844 + parm`
    /// (0x3001db80).
    pub fn fx_tag_index(
        &mut self,
        cs: &mut ConfigStrings,
        id: i32,
        tag: &str,
    ) -> Result<i32, ErrorKind> {
        let text = format!("{id:02}{tag}");
        let slot = self.index(cs, CsRange::FxTag, &text)?;
        Ok((slot - self.layout.bounds(CsRange::FxTag).0) as i32 + 1)
    }
}

/// The script error an indexer raises for a name past `level.initializing`:
/// `model '%s' not precached` (0x79b3a) for a model, `%s "%s" not precached`
/// (0x79ae0) for the rest.
pub fn not_precached(range: CsRange, name: &str) -> String {
    let what = match range {
        CsRange::Model => return format!("model '{name}' not precached"),
        CsRange::Shader => "shader",
        CsRange::Localized => "localized string",
        CsRange::Effect => "effect",
        // No other indexer has the gate.
        other => return format!("{other:?} \"{name}\" not precached"),
    };
    format!("{what} \"{name}\" not precached")
}

/// `GScr_GetScriptMenuIndex` (0x5c73c): the offset within `CsRange::Menu` of
/// the slot holding `name`, scanning the whole range for an exact match.
/// That offset, not the configstring number, is what `openMenu` puts on the
/// wire. `None` is retail's `Menu '%s' was not precached` error.
pub fn script_menu_index(cs: &[String], layout: &CsLayout, name: &str) -> Option<usize> {
    let (lo, hi) = layout.bounds(CsRange::Menu);
    (lo..=hi).find(|s| cs[*s] == name).map(|s| s - lo)
}

/// The index an already registered sound alias travels as, in
/// `EV_SOUND_ALIAS`'s parm and in `loopSound`; `None` if nothing indexed it.
pub fn sound_alias_index(cs: &[String], layout: &CsLayout, name: &str) -> Option<i32> {
    let (lo, hi) = layout.bounds(CsRange::SoundAlias);
    (lo..=hi)
        .find(|&i| cs.get(i).is_some_and(|s| s == name))
        .map(|i| (i - layout.sounds()) as i32)
}

/// How many hint-string configstrings `G_GetHintStringIndex` (0x5a238)
/// fills from `CsLayout::hint_strings`
/// (`docs/research/cod11-gsc-object-model.md`, the hint-string paragraph).
const MAX_HINT_STRINGS: usize = 32;

/// The slot within the hint-string range holding `name`, which is what
/// `serverCursorHintString` carries: the stock turret's `CGAME_USEMG42` is 0.
pub fn hint_string_index(cs: &[String], layout: &CsLayout, name: &str) -> Option<i32> {
    (0..MAX_HINT_STRINGS)
        .find(|i| cs.get(layout.hint_strings + i).is_some_and(|s| s == name))
        .map(|i| i as i32)
}

/// `G_GetHintStringIndex` (0x5a238) for `setHintString`: the slot already
/// holding `name`, else the first empty one, claimed. `None` when all 32 are
/// taken by other strings, retail's "Too many different hintstring values".
/// The scan order is VERIFIED by the mp_chateau `re` capture, whose two
/// objectives' strings land in 1214 and 1215 behind the engine's 1212/1213
/// (docs/research/cod11-gametypes-re-bel.md 4).
pub fn hint_string_alloc(cs: &mut ConfigStrings, layout: &CsLayout, name: &str) -> Option<i32> {
    for i in 0..MAX_HINT_STRINGS {
        let slot = &cs[layout.hint_strings + i];
        if slot == name {
            return Some(i as i32);
        }
        if slot.is_empty() {
            cs.set(layout.hint_strings + i, name);
            return Some(i as i32);
        }
    }
    None
}

/// The inverse: what `Cmd_MenuResponse_f` (0x486d8) reads back out of
/// configstring `CsRange::Menu.start + index` to name the menu in its
/// `menuresponse` notify. Empty when nothing precached that slot, which is
/// the empty string retail passes on too.
pub fn script_menu_name<'a>(cs: &'a [String], layout: &CsLayout, index: usize) -> &'a str {
    let (lo, hi) = layout.bounds(CsRange::Menu);
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
    /// Past initializing a name in the range is found in any case and a new
    /// one is the indexer's error with nothing taken; while initializing
    /// it is allocated.
    #[test]
    fn index_precached_allocates_only_while_initializing() {
        let mut cs = ConfigStrings::new(vec![String::new(); 2048]);
        let mut a = Allocators::default();
        let slot = a
            .index_precached(&mut cs, CsRange::Model, "xmodel/crate", true)
            .unwrap();
        assert_eq!(
            a.index_precached(&mut cs, CsRange::Model, "XMODEL/Crate", false),
            Ok(slot)
        );
        assert_eq!(
            a.index_precached(&mut cs, CsRange::Model, "xmodel/new", false),
            Err(ErrorKind::Custom("model 'xmodel/new' not precached".into()))
        );
        assert_eq!(cs[slot + 1], "");
        assert_eq!(
            a.shader_index(&mut cs, "white", false),
            Err(ErrorKind::Custom("shader \"white\" not precached".into()))
        );
        assert_eq!(a.localized_index(&mut cs, "", false), Ok(0));
    }

    #[test]
    fn systeminfo_carries_the_pak_lists_as_retail_does() {
        let paks = [
            pak("pak6", "main", 1_252_304_247),
            pak("pak5", "main", 77_111_478),
            pak("localized_english_pak1", "main", -961_133_319),
        ];
        let fall = FallHeights::default();
        let pure = systeminfo(0x10, fall, false, &PakLists::new(true, &paks), None).to_string();
        assert!(pure.contains(
            "\\sv_cheats\\0\\sv_pakNames\\pak6 pak5\\sv_paks\\1252304247 77111478 \\sv_pure\\1\
             \\sv_referencedPakNames\\main/pak6 main/pak5 main/localized_english_pak1\
             \\sv_referencedPaks\\1252304247 77111478 -961133319 \\sv_serverid\\16\\timescale\\1"
        ));
        // `sv_pure 0` empties the loaded pair, which leaves the info string.
        let impure = systeminfo(0x10, fall, false, &PakLists::new(false, &paks), None).to_string();
        assert!(!impure.contains("sv_paks") && !impure.contains("sv_pakNames"));
        assert!(impure.contains("\\sv_pure\\0\\sv_referencedPakNames\\main/pak6"));
    }

    /// A mod server's systeminfo names its `fs_game`, which a client
    /// layers over its base directory.
    #[test]
    fn systeminfo_names_the_mod_in_key_order() {
        let paks = PakLists {
            fs_game: "mymod".into(),
            ..PakLists::default()
        };
        let info = systeminfo(1, FallHeights::default(), false, &paks, None).to_string();
        assert!(
            info.contains("\\bg_fallDamageMinHeight\\256\\fs_game\\mymod\\g_synchronousClients"),
            "{info}"
        );
    }

    /// Who is in the referenced pair: 1.1 every pak, 1.5 the paks read
    /// from and any outside `main`, UO the paks read from and any outside
    /// both `main` and `fs_basegame` (coduo_lnxded 0x807647e, 1.5b
    /// 0x8075cf2).
    #[test]
    fn the_referenced_pair_is_each_versions() {
        use vcod_common::net::protocol::{PROTOCOL_V6, PROTOCOL_V22};
        let paks = [
            (pak("zzz_mod", "mymod", 1), false),
            (pak("pakuo01", "uo", 2), false),
            (pak("pakuo00", "uo", 3), true),
            (pak("pak0", "main", 4), false),
            (pak("localized_english_pak0", "main", 5), true),
        ];
        let names = |p: &Protocol, basegame: &str| -> Vec<String> {
            let lists = PakLists::of(false, paks.iter().map(|(p, r)| (p, *r)), p, basegame);
            lists.referenced.into_iter().map(|(_, n)| n).collect()
        };
        assert_eq!(names(&PROTOCOL_V1, "").len(), 5);
        assert_eq!(
            names(&PROTOCOL_V6, ""),
            [
                "mymod/zzz_mod",
                "uo/pakuo01",
                "uo/pakuo00",
                "main/localized_english_pak0"
            ]
        );
        assert_eq!(
            names(&PROTOCOL_V22, "uo"),
            ["mymod/zzz_mod", "uo/pakuo00", "main/localized_english_pak0"]
        );
        // An empty `fs_basegame` matches every directory.
        assert_eq!(
            names(&PROTOCOL_V22, ""),
            ["uo/pakuo00", "main/localized_english_pak0"]
        );
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
            None,
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
        // 1.5 and UO keep `BIG_INFO_STRING`: all 60 fit.
        let big = systeminfo(
            0x10,
            FallHeights::default(),
            false,
            &PakLists::new(true, &paks),
            Some(&[]),
        )
        .to_string();
        assert!(
            big.len() > 4000 && big.len() <= BIG_INFO_CHARS,
            "{}",
            big.len()
        );
        let names = vcod_common::net::info_value_for_key(&big, "sv_referencedPakNames").unwrap();
        assert_eq!(names.split_whitespace().count(), 60);
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
    /// name twice is one slot, a new name takes the next.
    #[test]
    fn an_allocator_interns_and_appends() {
        let mut cs = ConfigStrings::new(vec![String::new(); 2048]);
        let mut a = Allocators::default();
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
        assert_eq!(CsRange::FxTag.bounds(), (845, 1099));
        assert_eq!(CsRange::ShellShock.bounds(), (1101, 1115));
        assert_eq!(CsRange::Menu.bounds(), (1180, 1211));
        assert_eq!(CsRange::Localized.bounds(), (1245, 1499));
        assert_eq!(CsRange::Shader.bounds(), (1501, 1627));
        assert_eq!(CsRange::ALL.len(), 11);
    }

    /// Exhausting a range is a hard error, not a wrap or a silent overwrite:
    /// a map that precaches 256 models is broken and should say so.
    #[test]
    fn an_exhausted_range_is_an_error() {
        let mut cs = ConfigStrings::new(vec![String::new(); 2048]);
        let mut a = Allocators::default();
        let (lo, hi) = CsRange::Effect.bounds();
        for i in 0..=(hi - lo) {
            a.index(&mut cs, CsRange::Effect, &format!("fx{i}"))
                .unwrap();
        }
        assert!(a.index(&mut cs, CsRange::Effect, "one too many").is_err());
    }
}
