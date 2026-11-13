//! The server's cvar table. `GameHost` owns it while the script runs and
//! `Server` reads it back every frame, the same shape as the configstring
//! table: one owner at a time, no half-synced copy.
//!
//! Configstrings 140..203 hold the names of the mirrored cvars and
//! 204..267 their values, paired by offset 64, and the client runs
//! `Cvar_Set(cs[140 + i], cs[204 + i])` down the block until a value is
//! empty. The block is sorted by name, not written in call order. The
//! engine's writer is in `docs/research/cod11-map-cycle.md` 3.2.

use std::collections::BTreeMap;
use vcod_gsc::ErrorKind;

mod registry;

pub use vcod_common::net::protocol::{
    CS_CVAR_NAMES as MIRROR_NAMES, CS_CVAR_VALUES as MIRROR_VALUES,
};

/// The 21 cvars the game module registers into the mirror at init, before
/// any script runs: its cvar table's 0x800-flagged rows. Values are the
/// retail capture's, recorded in docs/research/cod11-server-handshake.md.
const ENGINE_MIRRORED: &[(&str, &str)] = &[
    ("bg_duck2prone_time", "400"),
    ("bg_foliagesnd_fastinterval", "500"),
    ("bg_foliagesnd_maxspeed", "180"),
    ("bg_foliagesnd_minspeed", "40"),
    ("bg_foliagesnd_resetinterval", "500"),
    ("bg_foliagesnd_slowinterval", "1500"),
    ("bg_ladder_yawcap", "100"),
    ("bg_prone2duck_time", "400"),
    ("bg_prone_softyawedge", "1"),
    ("bg_prone_yawcap", "85"),
    ("bg_viewheight_crouched", "40"),
    ("bg_viewheight_prone", "11"),
    ("bg_viewheight_standing", "60"),
    ("g_ScoresBanner_Allies", "gfx/hud/hud@mpflag_american.tga"),
    ("g_ScoresBanner_Axis", "gfx/hud/hud@mpflag_german.tga"),
    ("g_ScoresBanner_None", "gfx/hud/hud@mpflag_none.tga"),
    (
        "g_ScoresBanner_Spectators",
        "gfx/hud/hud@mpflag_spectator.tga",
    ),
    ("g_TeamColor_Allies", "0.5 0.5 1"),
    ("g_TeamColor_Axis", "1 0.5 0.5"),
    ("g_TeamName_Allies", "GAME_ALLIES"),
    ("g_TeamName_Axis", "GAME_AXIS"),
];

/// Rows of the same table that are not mirrored but that something here
/// reads. `character\_utility::useOptionalModels` gates every character
/// script's gear model precaches on `g_useGear`, so without it the model
/// configstring block loses every gear model
/// (docs/research/cod11-gsc-object-model.md section 18). The two fall damage
/// bounds are the server's own `PM_CrashLand` input and ride systeminfo
/// (docs/research/cod11-player-clip.md 8.3). `g_weaponrespawn` is the
/// seconds a `spawnflags & 8` weapon takes to come back
/// (docs/research/cod11-items.md section 7).
///
/// The middle field is the row's `cvarlist` flag letters. Every other
/// registered cvar is in [`registry::REGISTRY`].
const ENGINE_DEFAULTS: &[(&str, &str, &str)] = &[
    ("g_useGear", "AL", "1"),
    ("bg_fallDamageMinHeight", "C", "256"),
    ("bg_fallDamageMaxHeight", "C", "480"),
    ("g_weaponrespawn", "", "5"),
];

/// `cvar_t` flag bits, in `cvarlist`'s column order: the letters
/// `S U R I A L C` (`Cvar_List_f`, cod_lnxded 0x806f530).
pub mod flag {
    pub const ARCHIVE: u16 = 0x1;
    pub const USERINFO: u16 = 0x2;
    pub const SERVERINFO: u16 = 0x4;
    /// `cvarlist` has no column for it; only the 1.5 rows that ride
    /// configstring 1 carry it here ([`super::registry::SYSTEMINFO_V6`]).
    pub const SYSTEMINFO: u16 = 0x8;
    pub const INIT: u16 = 0x10;
    pub const LATCH: u16 = 0x20;
    pub const ROM: u16 = 0x40;
    pub const CHEAT: u16 = 0x200;
    /// Bit and letter, in the order `cvarlist` prints them.
    pub const COLUMNS: [(u16, char); 7] = [
        (SERVERINFO, 'S'),
        (USERINFO, 'U'),
        (ROM, 'R'),
        (INIT, 'I'),
        (ARCHIVE, 'A'),
        (LATCH, 'L'),
        (CHEAT, 'C'),
    ];

    pub fn parse(letters: &str) -> u16 {
        COLUMNS
            .iter()
            .filter(|(_, c)| letters.contains(*c))
            .fold(0, |f, (b, _)| f | b)
    }
}

#[derive(Clone)]
struct Cvar {
    /// The registration spelling. Lookup folds, the mirror does not: the
    /// capture holds `g_TeamName_Allies`, not `g_teamname_allies`.
    name: String,
    value: String,
    /// `resetString`: the value the cvar was created with, by a `set` or a
    /// registration, which a console query prints beside the value. A
    /// registration of a cvar a `set` created takes over its default, which
    /// is why retail's `scr_allow_fg42` reads `"0"` with default `"1"`.
    default: String,
    /// Created by a `set` and not registered since.
    user_created: bool,
    flags: u16,
    /// `latchedString`: a console write to a `LATCH` cvar, taken at the
    /// next level load by [`Cvars::apply_latched`].
    latched: Option<String>,
    /// In the 140/204 mirror. Set by `makeCvarServerInfo`, which despite
    /// its name does not put the cvar in configstring 0: retail's cs 0
    /// holds only the `sv_*` and `g_gametype` set.
    mirrored: bool,
}

/// What a console write did; the message, if any, is `Cvar_Set2`'s.
#[derive(Debug, PartialEq, Eq)]
pub enum ConsoleSet {
    /// The value is live.
    Applied,
    /// Held for the next level load, or a latch cleared; the live value
    /// is unchanged.
    Latched(Option<String>),
    /// Refused: read only, write protected or cheat protected.
    Refused(String),
}

#[derive(Clone)]
pub struct Cvars {
    /// Keyed by the folded name, so lookup is case-insensitive the way
    /// `Cvar_FindVar` is, while the mirror keeps the registration
    /// spelling.
    vars: BTreeMap<String, Cvar>,
    /// Where the protocol's mirror block sits (`CsLayout::cvar_names`).
    mirror: &'static vcod_common::net::protocol::CsLayout,
}

impl Default for Cvars {
    fn default() -> Self {
        Cvars::new()
    }
}

impl Cvars {
    pub fn new() -> Cvars {
        Cvars::for_protocol(1)
    }

    /// The table a server speaking `protocol` starts from: 1.1's, or for 6
    /// 1.5b's, which mirrors and registers a different set
    /// ([`registry::REGISTRY_V6`]), or for 22 United Offensive's, 1.5's with
    /// its own rows on top ([`registry::REGISTRY_V22`]).
    pub fn for_protocol(protocol: u32) -> Cvars {
        let uo = protocol == 22;
        let v6 = protocol == 6 || uo;
        let mut cv = Cvars {
            vars: BTreeMap::new(),
            mirror: vcod_common::net::protocol::Protocol::by_version(
                vcod_common::game::Game::Cod1,
                protocol,
            )
            .map_or(&vcod_common::net::protocol::PROTOCOL_V1.cs, |p| &p.cs),
        };
        let mirrored = ENGINE_MIRRORED
            .iter()
            .filter(|(n, _)| !(v6 && registry::UNMIRRORED_V6.contains(n)))
            .chain(if v6 { registry::MIRRORED_V6 } else { &[] })
            .chain(if uo { registry::MIRRORED_V22 } else { &[] });
        for &(name, value) in mirrored {
            cv.make_server_info(name, value);
        }
        let v22: &[_] = if uo { registry::REGISTRY_V22 } else { &[] };
        let v6_rows = if v6 { registry::REGISTRY_V6 } else { &[] }
            .iter()
            .filter(|(n, ..)| !v22.iter().any(|(o, ..)| o == n));
        let overlay: Vec<&(&str, &str, &str)> = v6_rows.chain(v22).collect();
        let base = ENGINE_DEFAULTS
            .iter()
            .chain(registry::REGISTRY)
            .chain(registry::VCOD_ONLY)
            .filter(|(n, ..)| !overlay.iter().any(|(o, ..)| o == n));
        for &(name, flags, value) in base.chain(overlay.iter().copied()) {
            cv.register(name, value);
            cv.add_flags(name, flag::parse(flags));
        }
        let systeminfo: &[&str] = if uo { registry::SYSTEMINFO_V22 } else { &[] };
        if v6 {
            for name in registry::SYSTEMINFO_V6.iter().chain(systeminfo) {
                cv.add_flags(name, flag::SYSTEMINFO);
            }
        }
        cv
    }

    /// `G_SetUICvars` (1.5 game.mp 0x4ec08), which 1.5's `G_InitGame` runs
    /// at every level load: the vote, friendly fire, killcam and timeout
    /// settings copied into the `ui_*` rows configstring 1 carries.
    pub fn set_ui_cvars(&mut self) {
        const PAIRS: &[(&str, &str)] = &[
            ("g_allowvote", "ui_allowvote"),
            ("g_allowVoteMapRestart", "ui_allowVoteMapRestart"),
            ("g_allowVoteMapRotate", "ui_allowVoteMapRotate"),
            ("g_allowVoteTypeMap", "ui_allowVoteTypeMap"),
            ("g_allowVoteMap", "ui_allowVoteMap"),
            ("g_allowVoteGameType", "ui_allowVoteGameType"),
            ("g_allowVoteKick", "ui_allowVoteKick"),
            ("g_allowVoteClientKick", "ui_allowVoteClientKick"),
            ("g_allowVoteTempBanUser", "ui_allowVoteTempBanUser"),
            ("g_allowVoteTempBanClient", "ui_allowVoteTempBanClient"),
            ("g_allowVoteDrawFriend", "ui_allowVoteDrawFriend"),
            ("g_allowVoteKillCam", "ui_allowVoteKillCam"),
            ("g_allowVoteFriendlyFire", "ui_allowVoteFriendlyFire"),
            ("scr_drawfriend", "ui_drawfriend"),
            ("scr_friendlyfire", "ui_friendlyfire"),
            ("scr_killcam", "ui_killcam"),
            ("g_timeoutsAllowed", "ui_timeoutsAllowed"),
            ("g_timeoutBank", "ui_timeoutBank"),
            ("g_timeoutLength", "ui_timeoutLength"),
            ("g_timeoutRecovery", "ui_timeoutRecovery"),
        ];
        for (from, to) in PAIRS {
            let v = self.get(from).to_string();
            self.set(to, &v);
        }
    }

    pub fn get(&self, name: &str) -> &str {
        self.vars
            .get(&name.to_ascii_lowercase())
            .map_or("", |c| c.value.as_str())
    }

    /// `Cvar_Command`'s view of one cvar: the registration spelling, the
    /// value and the default. `None` when no such cvar exists.
    pub fn lookup(&self, name: &str) -> Option<(&str, &str, &str)> {
        self.vars
            .get(&name.to_ascii_lowercase())
            .map(|c| (c.name.as_str(), c.value.as_str(), c.default.as_str()))
    }

    /// Whether a `set` made the cvar rather than a registration.
    pub fn user_created(&self, name: &str) -> bool {
        self.vars
            .get(&name.to_ascii_lowercase())
            .is_some_and(|c| c.user_created)
    }

    /// `Cvar_Set`: registers the cvar if it does not exist, and never
    /// changes an existing one's flags.
    pub fn set(&mut self, name: &str, value: &str) {
        let entry = self
            .vars
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| Cvar {
                name: name.to_string(),
                value: String::new(),
                default: value.to_string(),
                user_created: true,
                flags: 0,
                latched: None,
                mirrored: false,
            });
        entry.value = value.to_string();
    }

    /// `Cvar_Set2` without `force`, as `set` and a bare `<name> <value>`
    /// reach it: read-only, init and cheat cvars refuse, a latched one
    /// holds the value for the next load. `cheats` is `sv_cheats`.
    /// The messages name the cvar as the caller spelled it.
    pub fn console_set(&mut self, name: &str, value: &str, cheats: bool) -> ConsoleSet {
        let Some(c) = self.vars.get_mut(&name.to_ascii_lowercase()) else {
            self.set(name, value);
            return ConsoleSet::Applied;
        };
        if c.flags & flag::ROM != 0 {
            return ConsoleSet::Refused(format!("{name} is read only.\n"));
        }
        if c.flags & flag::INIT != 0 {
            return ConsoleSet::Refused(format!("{name} is write protected.\n"));
        }
        if c.flags & flag::LATCH != 0 {
            return ConsoleSet::Latched(latch(&mut c.latched, &c.value, name, value));
        }
        if c.flags & flag::CHEAT != 0 && !cheats {
            return ConsoleSet::Refused(format!("{name} is cheat protected.\n"));
        }
        c.value = value.to_string();
        ConsoleSet::Applied
    }

    /// The value a latched cvar waits with, if any.
    pub fn latched(&self, name: &str) -> Option<&str> {
        self.vars
            .get(&name.to_ascii_lowercase())
            .and_then(|c| c.latched.as_deref())
    }

    /// A level load's `Cvar_Get` of every latched cvar: the waiting value
    /// goes live.
    pub fn apply_latched(&mut self) {
        for c in self.vars.values_mut() {
            if let Some(v) = c.latched.take() {
                c.value = v;
            }
        }
    }

    /// ORs flag bits into an existing cvar, as a registration does.
    pub fn add_flags(&mut self, name: &str, bits: u16) {
        if let Some(c) = self.vars.get_mut(&name.to_ascii_lowercase()) {
            c.flags |= bits;
        }
    }

    /// Every cvar's name as registered, case-folded order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.vars.values().map(|c| c.name.as_str())
    }

    /// `Cvar_InfoString(bit)`: every cvar carrying `bit` as `(name,
    /// value)`, case-folded order, empty values left out as
    /// `Info_SetValueForKey` leaves them.
    pub fn info_pairs(&self, bit: u16) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .values()
            .filter(move |c| c.flags & bit != 0 && !c.value.is_empty())
            .map(|c| (c.name.as_str(), c.value.as_str()))
    }

    /// Gives an existing cvar `name`'s spelling, as another game registers
    /// it (CoD4's `sv_floodprotect` over CoD1's `sv_floodProtect`).
    pub fn respell(&mut self, name: &str) {
        if let Some(c) = self.vars.get_mut(&name.to_ascii_lowercase()) {
            c.name = name.to_string();
        }
    }

    /// Clears `bits` on every cvar.
    pub fn clear_flags(&mut self, bits: u16) {
        for c in self.vars.values_mut() {
            c.flags &= !bits;
        }
    }

    /// Every cvar carrying `bit` as `(name, value)`, case-folded order,
    /// a latched value in place of the live one, empty values kept: what
    /// CoD4's `writeconfig` archives (cod4-handshake.md 14.1).
    pub fn flagged(&self, bit: u16) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .values()
            .filter(move |c| c.flags & bit != 0)
            .map(|c| (c.name.as_str(), c.latched.as_deref().unwrap_or(&c.value)))
    }

    pub fn flags(&self, name: &str) -> Option<u16> {
        self.vars.get(&name.to_ascii_lowercase()).map(|c| c.flags)
    }

    /// `Cvar_List_f` (cod_lnxded 0x806f530): every cvar whose name matches
    /// `filter` (`com_filter`), sorted case-folded, each
    /// as seven flag columns, the name and the quoted value; then the
    /// count of every cvar, matched or not, twice.
    pub fn list(&self, filter: Option<&str>) -> String {
        let mut out = String::new();
        for c in self.vars.values() {
            if filter.is_some_and(|f| !com_filter(f, &c.name)) {
                continue;
            }
            for (bit, letter) in flag::COLUMNS {
                out.push(if c.flags & bit != 0 { letter } else { ' ' });
            }
            out.push_str(&format!(" {} \"{}\"\n", c.name, c.value));
        }
        let n = self.vars.len();
        out.push_str(&format!("\n{n} total cvars\n{n} cvar indexes\n"));
        out
    }

    /// `Cvar_Get(name, default)`: takes the default only when the cvar does
    /// not exist, so a cvar already set on the command line keeps its
    /// value. This is how a cvar table's row is seeded, and the opposite of
    /// `set` above -- seeding with `set` would silently overwrite whatever
    /// the operator passed.
    pub fn register(&mut self, name: &str, default: &str) {
        let c = self
            .vars
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| Cvar {
                name: name.to_string(),
                value: default.to_string(),
                default: default.to_string(),
                user_created: false,
                flags: 0,
                latched: None,
                mirrored: false,
            });
        if c.user_created && !default.is_empty() {
            c.user_created = false;
            c.default = default.to_string();
        }
    }

    /// Applies a config file's `set` lines, as `Cvar_Set` each, and returns
    /// how many it applied. `bind`, `unbindall` and everything else in the
    /// file is dropped: nothing else in `default_mp.cfg` reaches a
    /// dedicated server's cvar table.
    ///
    /// `set` and not `register` is the right primitive here. Nothing the
    /// stock file names is in the game module's own cvar table, so there is
    /// no registration to lose, and `set` never flags a cvar into the
    /// 140/204 mirror -- a `scr_*` value from the file reaches the mirror
    /// only if a script later calls `makeCvarServerInfo` on that name,
    /// which keeps the existing value.
    pub fn exec_cfg(&mut self, text: &str) -> usize {
        let mut applied = 0;
        for line in text.lines() {
            let tokens = cfg_tokens(line);
            if tokens.len() >= 2 && tokens[0].eq_ignore_ascii_case("set") {
                self.set(&tokens[1], tokens.get(2).map_or("", |s| s.as_str()));
                applied += 1;
            }
        }
        applied
    }

    /// [`Self::exec_cfg`] under the command line: a row the operator
    /// already set (`+set` runs after the startup configs) keeps its value.
    pub fn exec_cfg_under_sets(&mut self, text: &str) -> usize {
        let mut applied = 0;
        for line in text.lines() {
            let tokens = cfg_tokens(line);
            if tokens.len() >= 2 && tokens[0].eq_ignore_ascii_case("set") {
                let set_by_hand = self
                    .vars
                    .get(&tokens[1].to_ascii_lowercase())
                    .is_some_and(|c| c.user_created || c.value != c.default);
                if !set_by_hand {
                    self.set(&tokens[1], tokens.get(2).map_or("", |s| s.as_str()));
                    applied += 1;
                }
            }
        }
        applied
    }

    /// `makeCvarServerInfo(name, default)`: `register` plus the mirror
    /// flag, which it sets whether or not the cvar already existed.
    pub fn make_server_info(&mut self, name: &str, default: &str) {
        self.register(name, default);
        if let Some(c) = self.vars.get_mut(&name.to_ascii_lowercase()) {
            c.mirrored = true;
        }
    }

    /// Another game's mirror: the block at `layout`'s `cvar_names`, and
    /// `rows` its engine's mirrored dvars at their defaults in place of the
    /// 1.1 table's (CoD2's, cod2-protocol.md 15.4).
    pub fn remirror(
        &mut self,
        layout: &'static vcod_common::net::protocol::CsLayout,
        rows: &[(&str, &str)],
    ) {
        for c in self.vars.values_mut() {
            c.mirrored = false;
        }
        self.mirror = layout;
        for &(name, value) in rows {
            self.register(name, value);
            if let Some(c) = self.vars.get_mut(&name.to_ascii_lowercase()) {
                c.value = value.to_string();
                c.default = value.to_string();
                c.mirrored = true;
            }
        }
    }

    /// Rebuilds the 140/204 block from the flagged set, sorted by folded
    /// name: the 1.5b capture puts `ui_allow_thompson` before
    /// `ui_allowVote`, which byte order would not
    /// (docs/research/cod15-protocol-diff.md, "Server side").
    pub fn write_mirror(&self, cs: &mut [String]) -> Result<(), ErrorKind> {
        // `vars` iterates in folded-name order.
        let flagged: Vec<&Cvar> = self.vars.values().filter(|c| c.mirrored).collect();
        let (first, last) = self.mirror.cvar_names;
        let values = self.mirror.cvar_values;
        if flagged.len() > last + 1 - first {
            // Checked before the clear loop below: `engine::tick` passes its
            // live configstring table in, and an overflow here must leave
            // the previous frame's mirror stale rather than wipe it.
            return Err(ErrorKind::BadType("cvar mirror range exhausted"));
        }
        for i in (first..=last).chain(values..=values + last - first) {
            cs[i] = String::new();
        }
        for (n, c) in flagged.iter().enumerate() {
            cs[first + n] = c.name.clone();
            cs[values + n] = c.value.clone();
        }
        Ok(())
    }
}

/// The `LATCH` arm of `Cvar_Set2`, measured on retail: the live value
/// clears a waiting one silently, the waiting value again is silent, and
/// anything else waits and says so.
pub fn latch(latched: &mut Option<String>, live: &str, name: &str, value: &str) -> Option<String> {
    if value == live {
        *latched = None;
        None
    } else if latched.as_deref() == Some(value) {
        None
    } else {
        *latched = Some(value.to_string());
        Some(format!("{name} will be changed upon restarting.\n"))
    }
}

/// `Com_Filter` (cod_lnxded 0x8067774), case-insensitive: a prefix match
/// (the filter's end matches whatever is left of the name), `?` steps one
/// character, `*` finds the first occurrence of the run up to the next `*`
/// or `?` (a `[` in that run is literal), `[...]` is a set with `a-z`
/// ranges, `]]` a literal `]` in one, and `[[` drops the first `[`
/// (handshake doc, "Console commands over rcon"). A `?` past the name's
/// end leaves retail reading past its terminator; that position matches
/// nothing more here.
fn com_filter(filter: &str, name: &str) -> bool {
    let (f, n) = (filter.as_bytes(), name.as_bytes());
    // The name's byte at `i`: its terminator at the end, `None` past it.
    let at = |i: usize| match i.cmp(&n.len()) {
        std::cmp::Ordering::Less => Some(n[i].to_ascii_uppercase()),
        std::cmp::Ordering::Equal => Some(0),
        std::cmp::Ordering::Greater => None,
    };
    let up = |c: u8| c.to_ascii_uppercase();
    let set_end = |fi: usize| f[fi] == b']' && f.get(fi + 1) != Some(&b']');
    let (mut fi, mut ni) = (0, 0);
    while fi < f.len() {
        match f[fi] {
            b'*' => {
                fi += 1;
                let start = fi;
                while fi < f.len() && f[fi] != b'*' && f[fi] != b'?' {
                    fi += 1;
                }
                let run = &f[start..fi];
                if run.is_empty() {
                    continue;
                }
                let Some(rest) = n.get(ni..) else {
                    return false;
                };
                match rest
                    .windows(run.len())
                    .position(|w| w.eq_ignore_ascii_case(run))
                {
                    Some(p) => ni += p + run.len(),
                    None => return false,
                }
            }
            b'?' => {
                fi += 1;
                ni += 1;
            }
            b'[' if f.get(fi + 1) == Some(&b'[') => fi += 1,
            b'[' => {
                fi += 1;
                let Some(c) = at(ni) else {
                    return false;
                };
                let mut found = false;
                while fi < f.len() && !found && !set_end(fi) {
                    let hi = f.get(fi + 2).copied();
                    if f.get(fi + 1) == Some(&b'-')
                        && hi.is_some_and(|h| h != b']' || f.get(fi + 3) == Some(&b']'))
                    {
                        found = (up(f[fi])..=up(f[fi + 2])).contains(&c);
                        fi += 3;
                    } else {
                        found = up(f[fi]) == c;
                        fi += 1;
                    }
                }
                if !found {
                    return false;
                }
                while fi < f.len() && !set_end(fi) {
                    fi += 1;
                }
                fi += 1;
                ni += 1;
            }
            c => {
                if at(ni) != Some(up(c)) {
                    return false;
                }
                fi += 1;
                ni += 1;
            }
        }
    }
    true
}

/// `Cmd_TokenizeString` cut down to what a cfg line needs: `//` starts a
/// comment, a double-quoted run is one token, everything else splits on
/// whitespace.
fn cfg_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                tokens.push(std::mem::take(&mut cur));
                quoted = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '/' if chars.peek() == Some(&'/') => break,
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1.5 table mirrors what the 1.5b gamestate's 140/204 block carries
    /// before the scripts add theirs (`gamestate-1.5.bin`, less
    /// `scr_layoutimage` and the `ui_*` rows the stock scripts make), sorted
    /// by folded name, which puts `ui_allow_bar` before `ui_allowVote`.
    #[test]
    fn a_15_table_mirrors_15bs_set() {
        let mut cv = Cvars::for_protocol(6);
        let mut cs = vec![String::new(); 2048];
        cv.write_mirror(&mut cs).unwrap();
        let names: Vec<&str> = MIRROR_NAMES
            .map(|i| cs[i].as_str())
            .take_while(|n| !n.is_empty())
            .collect();
        assert_eq!(
            names,
            [
                "bg_foliagesnd_fastinterval",
                "bg_foliagesnd_maxspeed",
                "bg_foliagesnd_minspeed",
                "bg_foliagesnd_resetinterval",
                "bg_foliagesnd_slowinterval",
                "bg_ladder_yawcap",
                "bg_prone_yawcap",
                "bg_viewheight_crouched",
                "bg_viewheight_prone",
                "bg_viewheight_standing",
                "cg_bobAmplitudeDucked",
                "cg_bobAmplitudeProne",
                "cg_bobAmplitudeStanding",
                "cg_bobMax",
                "g_ScoresBanner_Allies",
                "g_ScoresBanner_Axis",
                "g_ScoresBanner_None",
                "g_ScoresBanner_Spectators",
                "g_TeamColor_Allies",
                "g_TeamColor_Axis",
                "g_TeamName_Allies",
                "g_TeamName_Axis",
                "timescale",
            ]
        );
        cv.make_server_info("ui_allowVote", "1");
        cv.make_server_info("ui_allow_bar", "1");
        cv.write_mirror(&mut cs).unwrap();
        let at = |n: &str| MIRROR_NAMES.clone().find(|&i| cs[i] == n).unwrap();
        assert!(at("ui_allow_bar") < at("ui_allowVote"));
        assert_eq!(cv.get("protocol"), "6");
        assert_eq!(cv.get("shortversion"), "1.5");
        assert_eq!(cv.flags("scr_killcam"), Some(flag::parse("SRA")));
    }

    /// `g_useGear` is in the game module's own cvar table with default
    /// `"1"` and flags 0x21, not the 0x800 the mirrored set carries, so it
    /// never reaches the 140/204 block -- but every character script gates
    /// its gear model precaches on it through
    /// `character\_utility::useOptionalModels`, so a missing default costs
    /// the model configstring block every gear model on the map.
    #[test]
    fn g_use_gear_defaults_to_one_and_stays_out_of_the_mirror() {
        let cv = Cvars::new();
        assert_eq!(cv.get("g_useGear"), "1");
        let mut cs = vec![String::new(); 2048];
        cv.write_mirror(&mut cs).unwrap();
        assert!(!cs[MIRROR_NAMES].iter().any(|n| n == "g_useGear"));
    }

    /// Seeding a cvar table row keeps a value that is already there, so an
    /// operator's command-line `+set g_useGear 0` survives the seed. Both
    /// `ENGINE_MIRRORED` and `ENGINE_DEFAULTS` go through this primitive;
    /// `set` is the other direction and would overwrite.
    #[test]
    fn registering_a_default_never_overwrites_an_existing_value() {
        let mut cv = Cvars::new();
        cv.set("g_useGear", "0");
        cv.register("g_useGear", "1");
        assert_eq!(cv.get("g_useGear"), "0");
    }

    /// The 20 stock script cvars beyond the engine's 21: the 18
    /// `scr_allow_*` names and `scr_motd` that `_teams::initGlobalCvars()`
    /// passes to `makeCvarServerInfo`, plus `scr_layoutimage`, flagged
    /// earlier in `Callback_StartGameType`. Names and values are the
    /// retail capture's 161..180 / 225..244 block
    /// (crates/server/tests/fixtures/configstrings/mp_pavlov-dm.txt).
    ///
    /// `scr_allow_fg42` is `"0"` here (slot 164/228 in that capture) while
    /// every other `scr_allow_*` is `"1"`. Activision's own
    /// `_teams::initGlobalCvars()` passes `"1"` as fg42's default too, and
    /// `makeCvarServerInfo` never overwrites a cvar that already has a
    /// value, so that call is not what produced the `"0"`. What did is
    /// `default_mp.cfg`, which the engine execs at startup and whose line
    /// 95 is `set scr_allow_fg42 0` -- the file's only `scr_allow_*` row
    /// set to `0` (docs/research/cod11-gsc-object-model.md section 18).
    /// `Server::cvars` runs the file, so this table is a transcription of
    /// the capture, not a source of values.
    const STOCK_SCRIPT_CVARS: &[(&str, &str)] = &[
        ("scr_allow_bar", "1"),
        ("scr_allow_bren", "1"),
        ("scr_allow_enfield", "1"),
        ("scr_allow_fg42", "0"),
        ("scr_allow_kar98k", "1"),
        ("scr_allow_kar98ksniper", "1"),
        ("scr_allow_m1carbine", "1"),
        ("scr_allow_m1garand", "1"),
        ("scr_allow_mp40", "1"),
        ("scr_allow_mp44", "1"),
        ("scr_allow_nagant", "1"),
        ("scr_allow_nagantsniper", "1"),
        ("scr_allow_panzerfaust", "1"),
        ("scr_allow_ppsh", "1"),
        ("scr_allow_springfield", "1"),
        ("scr_allow_sten", "1"),
        ("scr_allow_thompson", "1"),
        ("scr_allow_vote", "1"),
        ("scr_layoutimage", "levelshots/layouts/hud@layout_mp_pavlov"),
        ("scr_motd", ""),
    ];

    /// The mirror is a sorted rebuild of the flagged set, not a call-order
    /// log. `scr_layoutimage` is flagged in `Callback_StartGameType`,
    /// before `_teams::initGlobalCvars()` runs its 18 calls, and still
    /// lands after every `scr_allow_*`; `scr_motd`, one of those 18, lands
    /// last. Registering the full stock flagged set (the 21 engine cvars
    /// plus these 20) is what makes the absolute slots match the retail
    /// capture; a handful of cvars sorts into different slots than the
    /// stock set does.
    #[test]
    fn the_mirror_is_sorted_not_call_ordered() {
        let mut cv = Cvars::new();
        let mut cs = vec![String::new(); 2048];
        for &(name, value) in STOCK_SCRIPT_CVARS {
            cv.make_server_info(name, value);
        }
        cv.write_mirror(&mut cs).unwrap();
        assert_eq!(cs[178], "scr_allow_vote");
        assert_eq!(cs[179], "scr_layoutimage");
        assert_eq!(cs[180], "scr_motd");
        assert_eq!(cs[243], "levelshots/layouts/hud@layout_mp_pavlov");
        // scr_motd's value is empty, which is also the client's loop stop
        // condition, so 244 stays unset.
        assert_eq!(cs[244], "");
    }

    /// The 21 cvars the engine registers into the mirror before any script
    /// runs, from the retail capture's 140..160 block.
    #[test]
    fn the_engine_defaults_fill_140_to_160() {
        let cv = Cvars::new();
        let mut cs = vec![String::new(); 2048];
        cv.write_mirror(&mut cs).unwrap();
        assert_eq!(cs[140], "bg_duck2prone_time");
        assert_eq!(cs[204], "400");
        assert_eq!(cs[152], "bg_viewheight_standing");
        assert_eq!(cs[216], "60");
        assert_eq!(cs[159], "g_TeamName_Allies");
        assert_eq!(cs[223], "GAME_ALLIES");
        assert_eq!(cs[160], "g_TeamName_Axis");
        assert_eq!(cs[224], "GAME_AXIS");
    }

    /// `exec_cfg` takes the `set` lines and nothing else, dequotes a
    /// quoted value, stops at a `//` comment, and overwrites a value that
    /// is already there the way `Cvar_Set` does. The shapes here are all
    /// from `default_mp.cfg` itself: tab-separated binds, a `set` whose
    /// value is quoted, and a commented-out line.
    #[test]
    fn exec_cfg_applies_set_lines_only() {
        let mut cv = Cvars::new();
        cv.set("scr_allow_fg42", "1");
        let applied = cv.exec_cfg(concat!(
            "// Hello Dave, Would you like to play a game?\r\n",
            "unbindall\r\n",
            "bind\tPAUSE\t\t\"toggle cl_paused\"\r\n",
            "//set scr_allow_mp40 0\r\n",
            "set scr_allow_fg42 0\r\n",
            "set m_pitch \"0.022\"\r\n",
        ));
        assert_eq!(applied, 2);
        assert_eq!(cv.get("scr_allow_fg42"), "0");
        assert_eq!(cv.get("m_pitch"), "0.022");
        assert_eq!(cv.get("scr_allow_mp40"), "");
        assert_eq!(cv.get("toggle cl_paused"), "");
    }

    /// A cfg value never reaches the 140/204 mirror by itself: `set` does
    /// not flag, so a `scr_*` row only appears there once a script's
    /// `makeCvarServerInfo` flags that name, and then with the cfg's value
    /// rather than the script's default. That pair is exactly how retail's
    /// slot 228 reads `0` while `_teams::initGlobalCvars()` passes `"1"`.
    #[test]
    fn a_cfg_value_enters_the_mirror_only_when_a_script_flags_it() {
        let mut cv = Cvars::new();
        cv.exec_cfg("set scr_allow_fg42 0\nset scr_dm_scorelimit 50\n");
        let mut cs = vec![String::new(); 2048];
        cv.write_mirror(&mut cs).unwrap();
        assert!(!cs[MIRROR_NAMES].iter().any(|n| n == "scr_allow_fg42"));
        cv.make_server_info("scr_allow_fg42", "1");
        cv.write_mirror(&mut cs).unwrap();
        let at = cs[MIRROR_NAMES]
            .iter()
            .position(|n| n == "scr_allow_fg42")
            .expect("flagged now");
        assert_eq!(cs[MIRROR_VALUES.start() + at], "0");
        assert!(!cs[MIRROR_NAMES].iter().any(|n| n == "scr_dm_scorelimit"));
    }

    /// An unset cvar reads empty, which is what `Cvar_VariableString`
    /// returns for an unregistered name.
    #[test]
    fn an_unset_cvar_reads_empty() {
        let cv = Cvars::new();
        assert_eq!(cv.get("no_such_cvar"), "");
    }

    /// `setCvar` on a mirrored cvar updates the mirrored value; on an
    /// unflagged one it does not enter the mirror at all.
    #[test]
    fn set_updates_a_mirrored_value_and_does_not_flag_a_new_one() {
        let mut cv = Cvars::new();
        let mut cs = vec![String::new(); 2048];
        cv.set("g_TeamName_Allies", "MPSCRIPT_RUSSIAN");
        cv.set("scr_dm_scorelimit", "50");
        cv.write_mirror(&mut cs).unwrap();
        assert_eq!(cs[223], "MPSCRIPT_RUSSIAN");
        assert!(!cs[140..=203].contains(&"scr_dm_scorelimit".to_string()));
        assert_eq!(cv.get("scr_dm_scorelimit"), "50");
    }

    /// The mirror never reaches a slot the static table owns; 140..267 is
    /// clear of all six.
    #[test]
    fn the_mirror_is_clear_of_the_static_table() {
        for &(i, _) in crate::configstrings::STATIC {
            assert!(
                !MIRROR_NAMES.contains(&i) && !MIRROR_VALUES.contains(&i),
                "the static table sets {i}, which the mirror writes"
            );
        }
    }
    /// `cvarlist g_*` off a retail 1.1d server before any client joined
    /// (handshake doc, "Console commands over rcon"): flag columns, case-folded order, every
    /// engine and game module row. That server's archived config had
    /// `g_logSync 1`; the default is 0.
    #[test]
    fn cvarlist_matches_retail() {
        let mut cv = Cvars::new();
        cv.set("g_logSync", "1");
        let want = concat!(
            "        g_allowVote \"1\"\n",
            "    A   g_banIPs \"\"\n",
            "      C g_bounds_height_standing \"70\"\n",
            "      C g_bounds_width \"30\"\n",
            "    A   g_complaintlimit \"3\"\n",
            "        g_debugAlloc \"0\"\n",
            "      C g_debuganim \"0\"\n",
            "      C g_debugBullets \"0\"\n",
            "      C g_debugDamage \"0\"\n",
            "      C g_debugLocDamage \"0\"\n",
            "        g_debugMove \"0\"\n",
            "        g_debugProneCheck \"0\"\n",
            "        g_debugProneCheckDepthCheck \"1\"\n",
            "      C g_debugShowHit \"0\"\n",
            "      C g_dumpAnims \"-1\"\n",
            "S    L  g_gametype \"dm\"\n",
            "        g_gravity \"800\"\n",
            "        g_inactivity \"0\"\n",
            "        g_intermissionDelay \"1000\"\n",
            "        g_knockback \"1000\"\n",
            "        g_listEntity \"0\"\n",
            "    A   g_log \"games_mp.log\"\n",
            "    A   g_logSync \"1\"\n",
            "        g_motd \"\"\n",
            "        g_no_script_spam \"0\"\n",
            "        g_password \"\"\n",
            "        g_ScoresBanner_Allies \"gfx/hud/hud@mpflag_american.tga\"\n",
            "        g_ScoresBanner_Axis \"gfx/hud/hud@mpflag_german.tga\"\n",
            "        g_ScoresBanner_None \"gfx/hud/hud@mpflag_none.tga\"\n",
            "        g_ScoresBanner_Spectators \"gfx/hud/hud@mpflag_spectator.tga\"\n",
            "        g_scriptMainMenu \"\"\n",
            "        g_smoothClients \"1\"\n",
            "        g_speed \"190\"\n",
            "        g_synchronousClients \"0\"\n",
            "        g_TeamColor_Allies \"0.5 0.5 1\"\n",
            "        g_TeamColor_Axis \"1 0.5 0.5\"\n",
            "        g_TeamName_Allies \"GAME_ALLIES\"\n",
            "        g_TeamName_Axis \"GAME_AXIS\"\n",
            "    AL  g_useGear \"1\"\n",
            "    A   g_voiceChatsAllowed \"4\"\n",
            "        g_weaponAmmoPools \"0\"\n",
            "        g_weaponrespawn \"5\"\n",
        );
        let n = cv.vars.len();
        assert_eq!(
            cv.list(Some("g_*")),
            format!("{want}\n{n} total cvars\n{n} cvar indexes\n")
        );
        assert_eq!(
            cv.list(Some("G_SPEED")),
            format!("        g_speed \"190\"\n\n{n} total cvars\n{n} cvar indexes\n")
        );
        assert!(
            cv.list(Some("*_debugMove"))
                .starts_with("        g_debugMove \"0\"\n\n")
        );
    }

    /// `cvarlist <filter>` on a retail 1.1d server's 210 cvars (handshake
    /// doc, "Console commands over rcon"): the names each filter listed.
    #[test]
    fn com_filter_matches_retail() {
        let cv = Cvars::new();
        let listed = |f: &str| -> Vec<&str> {
            cv.vars
                .values()
                .map(|c| c.name.as_str())
                .filter(|n| com_filter(f, n))
                .collect()
        };
        let fs = [
            "fs_basegame",
            "fs_basepath",
            "fs_cdpath",
            "fs_copyfiles",
            "fs_debug",
            "fs_game",
            "fs_homepath",
            "fs_ignoreLozalized",
            "fs_restrict",
        ];
        assert_eq!(listed("fs_"), fs);
        for f in [
            "g_spe",
            "g_spe?",
            "g_speed??",
            "[[g]_speed",
            "g_s[pq-r]",
            "G_[A-z]PEED",
        ] {
            assert_eq!(listed(f), ["g_speed"], "{f}");
        }
        assert_eq!(listed("sv_*rate"), ["sv_maxRate"]);
        assert_eq!(listed("*speeds"), ["com_speeds"]);
        assert_eq!(
            listed("g_[s]"),
            [
                "g_ScoresBanner_Allies",
                "g_ScoresBanner_Axis",
                "g_ScoresBanner_None",
                "g_ScoresBanner_Spectators",
                "g_scriptMainMenu",
                "g_smoothClients",
                "g_speed",
                "g_synchronousClients",
            ]
        );
        // A `[` inside a `*` run is literal; `[]` and `[` past the name's
        // end never match.
        for f in ["*_[s]", "*[ab]", "g_s[]]", "g_speed[]", "g_speed[a]"] {
            assert!(listed(f).is_empty(), "{f}");
        }
    }

    /// `Cvar_Set2`'s refusals and its latch, each string as retail printed
    /// it over rcon.
    #[test]
    fn console_writes_follow_the_flags() {
        let mut cv = Cvars::new();
        let refused = |m: &str| ConsoleSet::Refused(m.to_string());
        assert_eq!(
            cv.console_set("sv_cheats", "1", false),
            refused("sv_cheats is read only.\n")
        );
        assert_eq!(
            cv.console_set("fs_game", "x", false),
            refused("fs_game is write protected.\n")
        );
        assert_eq!(
            cv.console_set("timescale", "2", false),
            refused("timescale is cheat protected.\n")
        );
        assert_eq!(cv.console_set("timescale", "2", true), ConsoleSet::Applied);
        assert_eq!(cv.get("timescale"), "2");

        assert_eq!(
            cv.console_set("g_useGear", "0", false),
            ConsoleSet::Latched(Some("g_useGear will be changed upon restarting.\n".into()))
        );
        assert_eq!(
            cv.console_set("g_useGear", "0", false),
            ConsoleSet::Latched(None)
        );
        assert_eq!(
            (cv.get("g_useGear"), cv.latched("g_usegear")),
            ("1", Some("0"))
        );
        // The live value again clears the latch without a word.
        assert_eq!(
            cv.console_set("g_useGear", "1", false),
            ConsoleSet::Latched(None)
        );
        assert_eq!(cv.latched("g_useGear"), None);
        cv.console_set("g_useGear", "0", false);
        cv.apply_latched();
        assert_eq!((cv.get("g_useGear"), cv.latched("g_useGear")), ("0", None));
    }

    /// A registration takes over the default of a cvar a `set` created:
    /// retail's `scr_allow_fg42` reads `"0"` from `default_mp.cfg` and
    /// `default:"1"` from `makeCvarServerInfo`.
    #[test]
    fn a_registration_takes_over_a_set_cvar_s_default() {
        let mut cv = Cvars::new();
        cv.exec_cfg("set scr_allow_fg42 0\n");
        cv.make_server_info("scr_allow_fg42", "1");
        assert_eq!(
            cv.lookup("scr_allow_fg42"),
            Some(("scr_allow_fg42", "0", "1"))
        );
        cv.register("scr_allow_fg42", "2");
        assert_eq!(
            cv.lookup("scr_allow_fg42"),
            Some(("scr_allow_fg42", "0", "1"))
        );
    }
}
