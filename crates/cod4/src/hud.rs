//! CoD4 1.7's 2D layer inputs, renderer-free: the cgame's server commands
//! (`CG_DeployServerCommand`), the score list, the playerstate's hudelems
//! and the zone assets the HUD draws with: the fonts, each material's
//! image, the localized strings, the weapons' kill icons and the game
//! message windows' placement (docs/research/cod4-hud.md).

use crate::assets::Assets;
use crate::assets::material::Material;
use crate::assets::ui::Font;
use crate::install::Install;
use crate::map::stub_name;
use crate::net::delta;
use crate::net::fields::HUDELEM;
use crate::walk::{WalkOptions, walk};
use crate::zone::Zone;
use anyhow::{Context, Result};
use std::collections::HashMap;

/// The server dvar mirror: names at `CS_CODINFO..CS_CODINFO_VALUE`, each
/// value `CS_CODINFO_VALUE - CS_CODINFO` slots on.
pub const CS_CODINFO: u16 = 20;
pub const CS_CODINFO_VALUE: u16 = 148;
/// Localized string `n` is configstring `CS_LOCALIZED_STRINGS + n`.
pub const CS_LOCALIZED_STRINGS: u16 = 309;
/// Material `n` (a hudelem's `materialIndex`) is `CS_MATERIALS + n`.
pub const CS_MATERIALS: u16 = 2002;
/// A score row's status icon `n` (1..=8) is `CS_STATUS_ICONS + n`
/// (`[ebp*4 + 0xc64c34]` in `CG_ParseScores`, cod4-hud.md 7): slot 0 is
/// the weapon list, 2259 `hud_status_dead`, 2260 `hud_status_connecting`.
pub const CS_STATUS_ICONS: u16 = 2258;
/// The server's `sv_hostname` lives in the serverinfo; the server dvars the
/// scoreboard reads (`g_ScoresColor_*`, `g_TeamIcon_*`, `g_TeamName_*`)
/// come through the [`CS_CODINFO`] mirror.
pub const PM_INTERMISSION: u32 = 5;
/// `"compass_map_<map>" x0 y0 x1 y1`: the minimap material and the world
/// corners it spans.
pub const CS_MINIMAP: u16 = 823;
/// The compass's north yaw.
pub const CS_NORTH_YAW: u16 = 822;

/// The zones the HUD's assets come from, in load order (the server's
/// `com_startup` order, cod4-handshake.md 1; iw3mp.exe 0x46cd60 names the
/// same four for the client's game load); the map's goes last.
pub const HUD_ZONES: [&str; 4] = [
    "code_post_gfx_mp",
    "localized_code_post_gfx_mp",
    "common_mp",
    "localized_common_mp",
];

/// The zones the front end's assets come from, in the order iw3mp.exe
/// 0x5f3c00 loads the names 0x46ccb0 sets: the code zones, `ui_mp` (the
/// main menu and the screens behind it), then the common zones
/// (cod4-front-end.md 1). vcod walks a mod's `mod.ff` after them.
pub const FRONT_ZONES: [&str; 5] = [
    "code_post_gfx_mp",
    "localized_code_post_gfx_mp",
    "ui_mp",
    "common_mp",
    "localized_common_mp",
];

/// `Cmd_TokenizeString`: tokens split on whitespace, a `"` opening one that
/// runs to the next `"`.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            chars.next();
        }
        let Some(&c) = chars.peek() else { break };
        let mut tok = String::new();
        if c == '"' {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                tok.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_ascii_whitespace() {
                    break;
                }
                tok.push(c);
                chars.next();
            }
        }
        out.push(tok);
    }
    out
}

/// The dvars the client's startup `exec default_mp.cfg` sets, in order
/// (`iw3mp.exe` 0x4ff130 queues it ahead of `language.cfg` and the
/// player's config, cod4-hud.md 12.2): each `set`, `seta` or `sets` line
/// as name and the rest of its tokens, an `exec` read through `read` in
/// its place. `default_mp_gamesettings.cfg` is where the team menu's
/// `scr_game_spectatetype` 1 comes from.
pub fn startup_dvars(read: impl Fn(&str) -> Option<Vec<u8>>) -> Vec<(String, String)> {
    fn exec(
        name: &str,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
        depth: u32,
        out: &mut Vec<(String, String)>,
    ) {
        // `Cbuf_Execute`'s alias loop guard is 1024 deep; a cfg chain
        // past a handful is a loop.
        if depth > 8 {
            return;
        }
        let Some(data) = read(name) else {
            return;
        };
        let text = String::from_utf8_lossy(&data);
        for line in text.lines().flat_map(|l| l.split(';')) {
            let mut t = tokenize(line);
            if let Some(i) = t.iter().position(|w| w.starts_with("//")) {
                t.truncate(i);
            }
            match t.first().map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("set" | "seta" | "sets") if t.len() >= 2 => {
                    out.push((t[1].clone(), t[2..].join(" ")));
                }
                Some("exec") if t.len() >= 2 => exec(&t[1], read, depth + 1, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    exec("default_mp.cfg", &read, 0, &mut out);
    out
}

/// One row of `b`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScoreRow {
    pub client: i32,
    pub score: i32,
    pub ping: i32,
    pub deaths: i32,
    /// 1..=8 names configstring `CS_STATUS_ICONS + n`; anything else none.
    pub status_icon: i32,
    pub kills: i32,
    pub assists: i32,
}

/// `b`, as `CG_ParseScores` (`iw3mp.exe` 0x44a350) reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scores {
    /// Team 1's (axis, opfor) and team 2's (allies, marines) score.
    pub team_scores: [i32; 2],
    pub score_limit: i32,
    pub rows: Vec<ScoreRow>,
}

impl Scores {
    /// `b <count> <team 1> <team 2> <limit>` and seven numbers a row; the
    /// count is capped at 64 and a missing argument reads 0.
    pub fn parse(args: &[String]) -> Scores {
        let num = |i: usize| args.get(i).map_or(0, |a| atoi(a));
        let count = num(1).clamp(0, 64) as usize;
        let rows = (0..count)
            .map(|i| {
                let at = |k: usize| num(7 * i + 5 + k);
                ScoreRow {
                    client: Some(at(0)).filter(|c| (0..64).contains(c)).unwrap_or(0),
                    score: at(1),
                    ping: at(2),
                    deaths: at(3),
                    status_icon: at(4),
                    kills: at(5),
                    assists: at(6),
                }
            })
            .collect();
        Scores {
            team_scores: [num(2), num(3)],
            score_limit: num(4),
            rows,
        }
    }
}

/// C's `atoi`: an optional sign and the leading digits, else 0.
pub fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let v = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |v, d| {
            (v * 10 + i64::from(d - b'0')).min(i64::from(i32::MAX))
        });
    (if neg { -v } else { v }) as i32
}

/// A server command the 2D layer reads, by `CG_DeployServerCommand`'s
/// first character (`iw3mp.exe` 0x44be40, its jump table at 0x44c530).
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `h`: a chat line, `i`: a team chat line, each localized.
    Chat(String),
    /// `e`, `f`: a game message (`iPrintLn`).
    GameMessage(String),
    /// `c`, `g`: a bold game message (`iPrintLnBold`, announcements).
    BoldMessage(String),
    /// `b`: the score list.
    Scores(Scores),
    /// `d <slot> <text>`: a configstring changed.
    ConfigString(u16, String),
    /// `v <name> <value> ...`: server dvars mirrored on the client.
    Dvars(Vec<(String, String)>),
    /// `G`, `H`: team 1's or 2's score.
    TeamScore { team: usize, score: i32 },
    /// `s <index>`: `playLocalSound`, the alias in configstring
    /// `CS_SOUND_ALIASES + index` (`CG_LocalSound`, `iw3mp.exe` 0x44bc70:
    /// two arguments, an index in 1..=256).
    LocalSound(u16),
    /// `k <index>`: `stopLocalSound` of the same alias (0x44bd10).
    StopLocalSound(u16),
}

impl Command {
    /// The command `text` is, `None` for one the HUD ignores. The first
    /// token's first character picks it, as the cgame's switch does: a
    /// leading space (1.7's `MP_CONNECTED` lines, cod4-hud.md 2) makes the
    /// quoted message the first token, whose `M` is no command.
    pub fn parse(text: &[u8]) -> Option<Command> {
        let text = String::from_utf8_lossy(text);
        let args = tokenize(&text);
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        Some(match args.first()?.chars().next()? {
            'h' | 'i' => Command::Chat(arg(1)),
            'e' | 'f' => Command::GameMessage(arg(1)),
            'c' | 'g' => Command::BoldMessage(arg(1)),
            'b' => Command::Scores(Scores::parse(&args)),
            'd' => {
                let slot = u16::try_from(atoi(&arg(1))).ok()?;
                // `CL_ConfigstringModified` takes the rest of the line.
                let rest = text
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .map(|(_, r)| r.trim_start())
                    .and_then(|r| r.split_once(char::is_whitespace))
                    .map_or("", |(_, r)| r);
                Command::ConfigString(slot, rest.to_string())
            }
            'v' => Command::Dvars(
                args[1..]
                    .chunks(2)
                    .map(|p| (p[0].clone(), p.get(1).cloned().unwrap_or_default()))
                    .collect(),
            ),
            's' | 'k' => {
                let index = (args.len() == 2)
                    .then(|| atoi(&arg(1)))
                    .and_then(|i| u16::try_from(i).ok())
                    .filter(|i| (1..=256).contains(i))?;
                match args[0].starts_with('s') {
                    true => Command::LocalSound(index),
                    false => Command::StopLocalSound(index),
                }
            }
            'G' => Command::TeamScore {
                team: 1,
                score: atoi(&arg(1)),
            },
            'H' => Command::TeamScore {
                team: 2,
                score: atoi(&arg(1)),
            },
            _ => return None,
        })
    }
}

/// `ReplaceDirective` (`iw3mp.exe` 0x4380d0), run until it finds none:
/// each `[{directive}]` in a hudelem's text becomes `binding(directive)`,
/// the brackets and braces dropped, the search going on past what it put
/// in. An empty directive or a `[{` with no `}]` after it ends the
/// search and leaves the rest as it is (1.7 returns there).
/// cod4-hud.md 8.
pub fn replace_directives(text: &str, binding: &dyn Fn(&str) -> String) -> String {
    let mut s = text.to_string();
    let mut from = 0;
    while let Some(open) = s[from..].find("[{").map(|i| from + i) {
        let Some(close) = s[open..].find("}]").map(|i| open + i) else {
            break;
        };
        let directive = &s[open + 2..close];
        if directive.is_empty() {
            break;
        }
        let b = binding(directive);
        s = format!("{}{b}{}", &s[..open], &s[close + 2..]);
        from = open + b.len();
    }
    s
}

/// The label and text merge (`iw3mp.exe` 0x438a00, called from
/// `GetHudElemInfo` at 0x438d6f only when both are non-empty): the
/// label's first `&&1` gives way to the text; a label without one is
/// followed by it. cod4-hud.md 5.
pub fn consolidate(label: &str, text: &str) -> String {
    if label.is_empty() || text.is_empty() {
        return format!("{label}{text}");
    }
    match label.find("&&1") {
        Some(i) => format!("{}{text}{}", &label[..i], &label[i + 3..]),
        None => format!("{label}{text}"),
    }
}

/// `he_type_t`.
pub mod kind {
    pub const FREE: i32 = 0;
    pub const TEXT: i32 = 1;
    pub const VALUE: i32 = 2;
    pub const PLAYERNAME: i32 = 3;
    pub const MAPNAME: i32 = 4;
    pub const GAMETYPE: i32 = 5;
    pub const MATERIAL: i32 = 6;
    pub const TIMER_DOWN: i32 = 7;
    pub const TIMER_UP: i32 = 8;
    pub const TENTHS_TIMER_DOWN: i32 = 9;
    pub const TENTHS_TIMER_UP: i32 = 10;
    pub const CLOCK_DOWN: i32 = 11;
    pub const CLOCK_UP: i32 = 12;
    pub const WAYPOINT: i32 = 13;
}

/// `hudelem_s.flags`: drawn in the foreground pass.
pub const HE_FLAG_FOREGROUND: i32 = 1;
/// Hidden while the player is dead.
pub const HE_FLAG_HIDE_WHEN_DEAD: i32 = 2;
/// Hidden while a menu is up (`hideWhenInMenu`; `CG_DrawHudElems`'s
/// `UI_AnyMenuVisible` test, cod4-hud.md 13).
pub const HE_FLAG_HIDE_WHEN_IN_MENU: i32 = 4;

/// CoD4's `hudelem_s`, read off its 40 netfield words (cod4-netfields.md
/// 1): positions and scales are floats, colours `r, g, b, a` bytes, times
/// server ms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudElem {
    pub kind: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub target_ent: i32,
    pub font_scale: f32,
    /// 0 default, 1 bigfixed, 2 smallfixed, 3 objective, 4 big, 5 small.
    pub font: i32,
    /// `alignx << 2 | aligny`, as CoD2's.
    pub align_org: i32,
    /// `horzalign << 3 | vertalign`, as CoD2's.
    pub align_screen: i32,
    pub color: [u8; 4],
    pub from_color: [u8; 4],
    pub fade_start_time: i32,
    pub fade_time: i32,
    pub label: i32,
    pub width: i32,
    pub height: i32,
    pub material: i32,
    pub offscreen_material: i32,
    pub from_width: i32,
    pub from_height: i32,
    pub scale_start_time: i32,
    pub scale_time: i32,
    pub from_x: f32,
    pub from_y: f32,
    pub from_align_org: i32,
    pub from_align_screen: i32,
    pub move_start_time: i32,
    pub move_time: i32,
    pub time: i32,
    pub duration: i32,
    pub value: f32,
    pub text: i32,
    pub sort: f32,
    pub glow_color: [u8; 4],
    pub fx_birth_time: i32,
    pub fx_letter_time: i32,
    pub fx_decay_start_time: i32,
    pub fx_decay_duration: i32,
    pub sound_id: i32,
    pub flags: i32,
}

impl HudElem {
    pub fn from_wire(w: &delta::HudElem) -> HudElem {
        let word = |name: &str| {
            let f = HUDELEM
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("no hudelem field {name}"));
            w.0[f.word()]
        };
        let int = |name: &str| word(name) as i32;
        let float = |name: &str| f32::from_bits(word(name));
        let rgba = |name: &str| word(name).to_le_bytes();
        HudElem {
            kind: int("type"),
            x: float("x"),
            y: float("y"),
            z: float("z"),
            target_ent: int("targetEntNum"),
            font_scale: float("fontScale"),
            font: int("font"),
            align_org: int("alignOrg"),
            align_screen: int("alignScreen"),
            color: rgba("color.rgba"),
            from_color: rgba("fromColor.rgba"),
            fade_start_time: int("fadeStartTime"),
            fade_time: int("fadeTime"),
            label: int("label"),
            width: int("width"),
            height: int("height"),
            material: int("materialIndex"),
            offscreen_material: int("offscreenMaterialIdx"),
            from_width: int("fromWidth"),
            from_height: int("fromHeight"),
            scale_start_time: int("scaleStartTime"),
            scale_time: int("scaleTime"),
            from_x: float("fromX"),
            from_y: float("fromY"),
            from_align_org: int("fromAlignOrg"),
            from_align_screen: int("fromAlignScreen"),
            move_start_time: int("moveStartTime"),
            move_time: int("moveTime"),
            time: int("time"),
            duration: int("duration"),
            value: float("value"),
            text: int("text"),
            sort: float("sort"),
            glow_color: rgba("glowColor.rgba"),
            fx_birth_time: int("fxBirthTime"),
            fx_letter_time: int("fxLetterTime"),
            fx_decay_start_time: int("fxDecayStartTime"),
            fx_decay_duration: int("fxDecayDuration"),
            sound_id: int("soundID"),
            flags: int("flags"),
        }
    }
}

/// A playerstate's elements in draw order's input: archived first, then
/// current, free slots dropped.
pub fn hud_elems(ps: &delta::PlayerState) -> Vec<HudElem> {
    ps.hud_archival
        .iter()
        .chain(ps.hud_current.iter())
        .map(HudElem::from_wire)
        .filter(|e| e.kind != kind::FREE)
        .collect()
}

/// The scoreboard's dvars at their 1.7 registrations (`iw3mp.exe`
/// 0x449d0c..0x44a100, cod4-hud.md 7).
pub mod scoreboard {
    pub const BANNER_HEIGHT: f32 = 35.0;
    pub const ITEM_HEIGHT: f32 = 18.0;
    pub const PING_WIDTH: f32 = 0.036;
    pub const PING_HEIGHT: f32 = 0.7;
    pub const WIDTH: f32 = 500.0;
    pub const HEIGHT: f32 = 435.0;
    /// `cg_scoreboardMyColor`: the drawn player's row text.
    pub const MY_COLOR: [f32; 4] = [1.0, 0.8, 0.4, 1.0];
    pub const TEXT_OFFSET: f32 = 0.5;
    pub const FONT: i32 = 0;
    pub const HEADER_FONT_SCALE: f32 = 0.35;
    /// `cg_scoreboardPingText` 1: the ping column is drawn.
    pub const PING_TEXT: bool = true;
    pub const RANK_FONT_SCALE: f32 = 0.25;
    /// The list's text scale (`CG_DrawClientScore`'s 0.35).
    pub const LIST_SCALE: f32 = 0.35;

    /// What a column draws.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Column {
        RankIcon,
        StatusIcon,
        Name,
        TalkingIcon,
        Score,
        Kills,
        Assists,
        Deaths,
        Ping,
    }

    /// `columnInfoWithPing`: kind, share of the list's width, header
    /// string and alignment (0 left, 1 centre, 2 right).
    pub const COLUMNS: [(Column, f32, &str, i32); 9] = [
        (Column::RankIcon, 0.05, "", 0),
        (Column::StatusIcon, 0.05, "", 2),
        (Column::Name, 0.35, "", 0),
        (Column::TalkingIcon, 0.05, "", 0),
        (Column::Score, 0.1, "CGAME_SB_SCORE", 2),
        (Column::Kills, 0.1, "CGAME_SB_KILLS", 2),
        (Column::Assists, 0.1, "CGAME_SB_ASSISTS", 2),
        (Column::Deaths, 0.1, "CGAME_SB_DEATHS", 2),
        (Column::Ping, 0.1, "CGAME_SB_PING", 2),
    ];
}

/// A weapon's kill icon (`killIcon`, `killIconRatio`, `flipKillIcon` at
/// `WeaponDef` +1304, +1308, +1312).
#[derive(Clone, Debug, PartialEq)]
pub struct KillIcon {
    pub material: String,
    /// `weaponIconRatioType_t`: 0 1:1, 1 2:1, 2 4:1.
    pub ratio: i32,
    pub flip: bool,
}

/// A game message window's `itemDef_s` (`ITEM_TYPE_GAME_MESSAGE_WINDOW`,
/// 19): its rect in the 640x480 grid and its text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MessageWindow {
    pub x: f32,
    pub y: f32,
    pub horz_align: i32,
    pub vert_align: i32,
    /// `textAlignMode`: 0 left, 1 centre, 2 right.
    pub alignment: i32,
    pub font: i32,
    pub text_scale: f32,
    pub text_style: i32,
    /// `gameMsgWindowIndex`: 0 game messages, 1 bold, 2 subtitles.
    pub index: i32,
    /// `gameMsgWindowMode`: 0 lines run down from the rect, 1 up from it.
    pub mode: i32,
}

/// `ITEM_TYPE_GAME_MESSAGE_WINDOW`.
const ITEM_GAME_MESSAGE_WINDOW: i32 = 19;

impl MessageWindow {
    fn of(raw: &[u8]) -> MessageWindow {
        let f = |o: usize| f32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        let i = |o: usize| i32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        MessageWindow {
            x: f(4),
            y: f(8),
            horz_align: i(20),
            vert_align: i(24),
            alignment: i(196),
            font: i(192),
            text_scale: f(208),
            text_style: i(212),
            index: i(216),
            mode: i(220),
        }
    }
}

/// What the HUD draws with, out of [`HUD_ZONES`] and the map's zone.
#[derive(Clone, Debug, Default)]
pub struct HudAssets {
    /// `fonts/smallFont`, `fonts/normalFont`, ... by name, with their
    /// glyph page's image name.
    pub fonts: Vec<(Font, Option<String>)>,
    /// A material's name, lowercased, to its colour map's image name.
    pub images: HashMap<String, String>,
    /// The localized strings by key (`MP_CONNECTED`).
    pub localize: HashMap<String, String>,
    pub kill_icons: HashMap<String, KillIcon>,
    /// The `gamemessages`, `boldgamemessages` and `subtitles` menus'
    /// windows, by index.
    pub windows: HashMap<i32, MessageWindow>,
    /// The `compass_old` menu's items by name (`minimapBG`, `mini_map`,
    /// `compassplayer`).
    pub compass: HashMap<String, MenuRect>,
    /// `mp/rankIconTable.csv`: the icon of rank `r` at prestige `p` is
    /// row `r`'s column `p + 1` (`CL_GetRankIcon`).
    pub rank_icons: HashMap<(i32, i32), String>,
    /// `mp/rankTable.csv`'s display level (column 14) by rank.
    pub rank_levels: HashMap<i32, String>,
    /// Weapon HUD fields by weapon name (`weaponinfo`'s owner draws).
    pub weapon_hud: HashMap<String, WeaponHud>,
    /// The `weaponinfo` menu's items by name (`ammoStock`, `weapname`, ..).
    pub weapon_info: HashMap<String, MenuItem>,
    /// The `stance` menu's one item.
    pub stance: Option<MenuItem>,
    /// The `killcam` menu's items in draw order: the letterbox's two bars
    /// and the `KILLCAM` title (cod4-hud.md 11).
    pub killcam: Vec<MenuItem>,
    /// Every menu's script responses by its name, lowercased.
    pub script_menus: HashMap<String, crate::scriptmenu::ScriptMenu>,
    /// The string tables by name, lowercased (`mp/classTable.csv`), for
    /// the menus' `tableLookup`.
    pub tables: HashMap<String, Table>,
    /// Each game type's display name key by its lowercased name: the first
    /// token of `maps/mp/gametypes/<name>.txt` (`war` reads `MPUI_WAR`),
    /// rawfiles of `code_post_gfx_mp` (`UI_GetGameTypesList_FastFile`, cod4-hud.md 12.2).
    pub gametypes: HashMap<String, String>,
    /// The game types in `maps/mp/gametypes/_gametypes.txt`'s order, the
    /// order the call vote's game type steps through.
    pub gametype_order: Vec<String>,
    /// `mp/cod2maps.arena`'s maps, in its order.
    pub arenas: Vec<crate::scriptmenu::Arena>,
}

/// A string table's cells, row by row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub columns: usize,
    pub cells: Vec<String>,
}

impl Table {
    /// `StringTable_Lookup`: `ret` of the last row whose `column` reads
    /// `value` (any case), else "".
    pub fn lookup(&self, column: i32, value: &str, ret: i32) -> &str {
        let (Ok(c), Ok(r)) = (usize::try_from(column), usize::try_from(ret)) else {
            return "";
        };
        if self.columns == 0 || c >= self.columns {
            return "";
        }
        self.cells
            .chunks(self.columns)
            .rev()
            .find(|row| row[c].eq_ignore_ascii_case(value))
            .and_then(|row| row.get(r))
            .map_or("", String::as_str)
    }
}

/// A weapon's HUD fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponHud {
    /// `displayName`, a localized string key.
    pub display_name: String,
    /// `hudIcon`'s material.
    pub hud_icon: Option<String>,
    /// `ammoCounterIcon`'s material.
    pub ammo_counter_icon: Option<String>,
    /// `modeName`, a localized key or empty.
    pub mode_name: String,
    /// `offhandClass` (+324): 0 none, 1 frag, 2 smoke, 3 flash.
    pub offhand_class: i32,
    /// `ammoName` and `clipName`: weapons sharing one share its slot.
    pub ammo_name: String,
    pub clip_name: String,
    /// `ammoCounterClip` (+796): how the clip graphic draws the clip, 0
    /// none, 1 magazine, 2 short magazine, 3 shotgun, 4 rocket, 5 belt
    /// fed, 6 the alt weapon's.
    pub ammo_counter_clip: i32,
    /// `clipSize` (+824).
    pub clip_size: i32,
    /// `maxAmmo` (+820), `sharedAmmoCap` (+840) and `clipOnly` (+1280):
    /// what `BG_GetAmmoPlayerMax` sums (cod4-hud.md 15).
    pub max_ammo: i32,
    pub shared_ammo_cap: i32,
    pub clip_only: bool,
    /// `dpadIcon` (+1316) and `dpadIconRatio` (+1320): the action slot's
    /// picture, 0 square, 1 2:1, 2 4:1.
    pub dpad_icon: Option<String>,
    pub dpad_icon_ratio: i32,
    /// `lowAmmoWarningThreshold` (+1468): the clip's fraction at or under
    /// which it reads low.
    pub low_ammo_threshold: f32,
    /// `weapType` (+300), 1 a grenade, and `silenced` (+1360): a shot of
    /// either pings no compass (cod4-hud.md 15.7).
    pub weap_type: i32,
    pub silenced: bool,
    /// `hudIconRatio` (+784): 0 square, 1 2:1, 2 4:1; `weapClass` (+304),
    /// 7 a turret, 9 an item (no hold breath hint); `inventoryType` (+316), 0 a primary (the cursor hint,
    /// cod4-hud.md 20).
    pub hud_icon_ratio: i32,
    pub weap_class: i32,
    pub inventory_type: i32,
    /// `explosionRadius` (+1364), `projExplosionType` (+1416: 0 a
    /// grenade, 2 a flashbang, 6 heavy) and `timedDetonation` (+1452):
    /// what the grenade indicator tests (cod4-hud.md 21).
    pub explosion_radius: i32,
    pub proj_explosion: i32,
    pub timed_detonation: bool,
    /// What the crosshair and the sight's overlay draw with.
    pub reticle: Reticle,
}

/// A weapon's crosshair and sight overlay fields at their 1.7 offsets
/// (`iw3mp.exe`'s weapon field table, cod4-hud.md 17).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reticle {
    /// `reticleCenter` (+540) and `reticleSide` (+544).
    pub center: Option<String>,
    pub side: Option<String>,
    /// `reticleCenterSize` (+548), `reticleSideSize` (+552) and
    /// `reticleMinOfs` (+556), in 640 grid units.
    pub center_size: i32,
    pub side_size: i32,
    pub min_ofs: i32,
    /// `hipReticleSidePos` (+1152): how far in the sides sit, a fraction
    /// of their size.
    pub side_pos: f32,
    /// `crosshairColorChange` (+1044).
    pub color_change: bool,
    /// `enemyCrosshairRange` (+1040): how near an enemy under the
    /// crosshair turns it (`CG_ScanForCrosshairEntity`).
    pub enemy_range: f32,
    /// `hipSpreadStandMin`, `DuckedMin`, `ProneMin`, `hipSpreadMax`,
    /// `DuckedMax`, `ProneMax` (+1104..+1124), degrees.
    pub hip_spread: [f32; 6],
    /// `cookOffHold` (+1276).
    pub cook_off_hold: bool,
    /// `adsZoomInFrac` (+1064), `adsZoomOutFrac` (+1068).
    pub ads_zoom_frac: [f32; 2],
    /// `adsCrosshairInFrac` (+1744), `adsCrosshairOutFrac` (+1748) and
    /// `adsAimPitch` (+1740).
    pub ads_crosshair_frac: [f32; 2],
    pub ads_aim_pitch: f32,
    /// `adsOverlayShader` (+1072), `adsOverlayShaderLowRes` (+1076),
    /// `adsOverlayReticle` (+1080) and `adsOverlayWidth` / `Height`
    /// (+1088, +1092).
    pub overlay: Option<String>,
    pub overlay_low_res: Option<String>,
    pub overlay_reticle: i32,
    pub overlay_size: [f32; 2],
}

/// `perk_weapSpreadMultiplier`'s default, 0.65 (the float at `iw3mp.exe`
/// 0x70b390 its registration at 0x40dd48 loads).
pub const PERK_SPREAD_MULTIPLIER: f32 = 0.65;

impl Reticle {
    /// `BG_GetSpreadForWeapon` (`iw3mp.exe` 0x416c70): the hip spread's
    /// two ends, by the eye's height between prone (11), crouched (40)
    /// and standing (60); a `spreadOverride` of state 2 pins both, of
    /// state 1 the max; Steady Aim (`perks` bit 1) scales both by
    /// [`PERK_SPREAD_MULTIPLIER`].
    pub fn spread(
        &self,
        view_height: f32,
        perks: u32,
        override_state: i32,
        override_value: i32,
    ) -> (f32, f32) {
        let [
            stand_min,
            ducked_min,
            prone_min,
            stand_max,
            ducked_max,
            prone_max,
        ] = self.hip_spread;
        let (mut min, mut max) = if override_state == 2 {
            (override_value as f32, override_value as f32)
        } else if view_height <= 40.0 {
            let f = (view_height - 11.0) / 29.0;
            (
                (ducked_min - prone_min) * f + prone_min,
                (ducked_max - prone_max) * f + prone_max,
            )
        } else {
            let f = (view_height - 40.0) / 20.0;
            (
                (stand_min - ducked_min) * f + ducked_min,
                (stand_max - ducked_max) * f + ducked_max,
            )
        };
        if override_state == 1 {
            max = override_value as f32;
        }
        if perks & 2 != 0 {
            min *= PERK_SPREAD_MULTIPLIER;
            max *= PERK_SPREAD_MULTIPLIER;
        }
        (min, max)
    }
}

/// Each weapon's slots in the playerstate's `ammo` and `ammoclip`
/// (`BG_SetupAmmoIndexes`, `BG_SetupClipIndexes`): in the weapon
/// list's order, a weapon takes the slot of the first one before it with
/// its ammo (clip) name, else the next free one; slot 0 is the default
/// weapon's. `list[0]` is weapon 1. cod4-hud.md 9.
pub fn ammo_slots(list: &[String], defs: &HashMap<String, WeaponHud>) -> Vec<(usize, usize)> {
    let none = WeaponHud::default();
    let mut ammo = vec![none.ammo_name.clone()];
    let mut clip = vec![none.clip_name.clone()];
    let slot = |names: &mut Vec<String>, n: &str| match names.iter().position(|x| x == n) {
        Some(i) => i,
        None => {
            names.push(n.to_string());
            names.len() - 1
        }
    };
    list.iter()
        .map(|w| {
            let d = defs.get(w).unwrap_or(&none);
            (slot(&mut ammo, &d.ammo_name), slot(&mut clip, &d.clip_name))
        })
        .collect()
}

/// A HUD menu item: its rect, owner draw and text.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub rect: MenuRect,
    pub owner_draw: i32,
    pub font: i32,
    pub text_scale: f32,
    pub text_style: i32,
    pub fore_color: [f32; 4],
    /// `itemDef_s.textAlignMode` (+196): 0 left, 1 centre, 2 right.
    pub text_align: i32,
    /// The item's text, else its `textExp`'s first string.
    pub text: Option<String>,
}

/// A menu item's rect in the 640x480 grid and its background material.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRect {
    pub rect: [f32; 4],
    pub horz_align: i32,
    pub vert_align: i32,
    pub background: Option<String>,
}

/// The compass's menu.
pub const COMPASS_MENU: &str = "compass_old";
/// The ammo counter, weapon name and offhand menu, and the stance menu.
pub const WEAPON_INFO_MENU: &str = "weaponinfo";
const STANCE_MENU: &str = "stance";
/// The killcam's letterbox and title.
const KILLCAM_MENU: &str = "killcam";

impl HudAssets {
    /// Walks the HUD zones and `map`'s (later zones override), then under
    /// an `fs_game` its `mod.ff`, whose assets win over both
    /// (cod4-fastfile.md 13).
    pub fn load(install: &Install, map: &str) -> Result<HudAssets> {
        let mod_zone = install.mod_zone().map(|_| "mod");
        Self::load_zones(
            install,
            HUD_ZONES.iter().copied().chain([map]).chain(mod_zone),
        )
    }

    /// The front end's zones ([`FRONT_ZONES`]), with no level's, then
    /// under an `fs_game` its `mod.ff`.
    pub fn load_front(install: &Install) -> Result<HudAssets> {
        let mod_zone = install.mod_zone().map(|_| "mod");
        Self::load_zones(install, FRONT_ZONES.iter().copied().chain(mod_zone))
    }

    /// Walks `zones` in order (later zones override): the console's font
    /// needs only `localized_code_post_gfx_mp`, which no map waits on.
    pub fn load_zones<'a>(
        install: &Install,
        zones: impl IntoIterator<Item = &'a str>,
    ) -> Result<HudAssets> {
        let mut out = HudAssets::default();
        for name in zones {
            let path = install.zone_path("english", name);
            let walked = Zone::open_path(&path)
                .and_then(|z| walk(z, WalkOptions { keep_bulk: false }))
                .with_context(|| path.display().to_string())?;
            out.add(&walked.assets);
        }
        Ok(out)
    }

    pub fn add(&mut self, a: &Assets) {
        for m in &a.materials {
            if let Some(img) = colour_image(a, m) {
                self.images.insert(m.name.to_ascii_lowercase(), img);
            }
        }
        for f in &a.fonts {
            let img = f
                .material
                .and_then(|m| colour_image(a, &a.materials[m.index()]));
            self.fonts
                .retain(|(g, _)| !g.name.eq_ignore_ascii_case(&f.name));
            self.fonts.push((f.clone(), img));
        }
        for l in &a.localize {
            if let Some(v) = &l.value {
                self.localize.insert(l.name.to_ascii_uppercase(), v.clone());
            }
        }
        for w in &a.weapons {
            let Some(&m) = w.materials.get("killIcon") else {
                continue;
            };
            let int = |o: usize| {
                w.raw
                    .get(o..o + 4)
                    .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
            };
            self.kill_icons.insert(
                w.name.clone(),
                KillIcon {
                    material: stub_name(&a.materials[m.index()].name).to_string(),
                    ratio: int(1308),
                    flip: int(1312) != 0,
                },
            );
        }
        for r in &a.raw_files {
            let text = || String::from_utf8_lossy(&r.data);
            if r.name
                .eq_ignore_ascii_case("maps/mp/gametypes/_gametypes.txt")
            {
                self.gametype_order = tokenize(&text());
            } else if r.name.eq_ignore_ascii_case("mp/cod2maps.arena") {
                self.arenas = crate::scriptmenu::arenas(&text());
            }
            let Some(name) = r
                .name
                .strip_prefix("maps/mp/gametypes/")
                .and_then(|n| n.strip_suffix(".txt"))
                .filter(|n| !n.starts_with('_'))
            else {
                continue;
            };
            let text = String::from_utf8_lossy(&r.data);
            if let Some(key) = tokenize(&text).into_iter().next() {
                self.gametypes.insert(name.to_ascii_lowercase(), key);
            }
        }
        for t in &a.string_tables {
            self.tables.insert(
                t.name.to_ascii_lowercase(),
                Table {
                    columns: t.columns as usize,
                    cells: t
                        .values
                        .iter()
                        .map(|v| v.clone().unwrap_or_default())
                        .collect(),
                },
            );
            let cell = |r: u32, c: u32| {
                t.values
                    .get((r * t.columns + c) as usize)
                    .cloned()
                    .flatten()
                    .unwrap_or_default()
            };
            if t.name.eq_ignore_ascii_case("mp/rankIconTable.csv") {
                for r in 0..t.rows {
                    let Ok(rank) = cell(r, 0).parse::<i32>() else {
                        continue;
                    };
                    for c in 1..t.columns {
                        self.rank_icons
                            .insert((rank, c as i32 - 1), cell(r, c).to_string());
                    }
                }
            }
            if t.name.eq_ignore_ascii_case("mp/rankTable.csv") && t.columns > 14 {
                for r in 0..t.rows {
                    if let Ok(rank) = cell(r, 0).parse::<i32>() {
                        self.rank_levels.insert(rank, cell(r, 14).to_string());
                    }
                }
            }
        }
        for w in &a.weapons {
            let int = |o: usize| {
                w.raw
                    .get(o..o + 4)
                    .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
            };
            let mat = |k: &str| {
                w.materials
                    .get(k)
                    .map(|m| stub_name(&a.materials[m.index()].name).to_string())
            };
            let flt = |o: usize| f32::from_bits(int(o) as u32);
            self.weapon_hud.insert(
                w.name.clone(),
                WeaponHud {
                    display_name: w.strings.get("displayName").cloned().unwrap_or_default(),
                    hud_icon: mat("hudIcon"),
                    ammo_counter_icon: mat("ammoCounterIcon"),
                    mode_name: w.strings.get("modeName").cloned().unwrap_or_default(),
                    offhand_class: int(324),
                    ammo_name: w.strings.get("ammoName").cloned().unwrap_or_default(),
                    clip_name: w.strings.get("clipName").cloned().unwrap_or_default(),
                    ammo_counter_clip: int(796),
                    clip_size: int(824),
                    max_ammo: int(820),
                    shared_ammo_cap: int(840),
                    clip_only: int(1280) != 0,
                    dpad_icon: mat("dpadIcon"),
                    dpad_icon_ratio: int(1320),
                    low_ammo_threshold: f32::from_bits(int(1468) as u32),
                    weap_type: int(300),
                    silenced: int(1360) != 0,
                    hud_icon_ratio: int(784),
                    weap_class: int(304),
                    inventory_type: int(316),
                    explosion_radius: int(1364),
                    proj_explosion: int(1416),
                    timed_detonation: int(1452) != 0,
                    reticle: Reticle {
                        center: mat("reticleCenter"),
                        side: mat("reticleSide"),
                        center_size: int(548),
                        side_size: int(552),
                        min_ofs: int(556),
                        side_pos: flt(1152),
                        color_change: int(1044) != 0,
                        enemy_range: flt(1040),
                        hip_spread: std::array::from_fn(|k| flt(1104 + 4 * k)),
                        cook_off_hold: int(1276) != 0,
                        ads_zoom_frac: [flt(1064), flt(1068)],
                        ads_crosshair_frac: [flt(1744), flt(1748)],
                        ads_aim_pitch: flt(1740),
                        overlay: mat("adsOverlayShader"),
                        overlay_low_res: mat("adsOverlayShaderLowRes"),
                        overlay_reticle: int(1080),
                        overlay_size: [flt(1088), flt(1092)],
                    },
                },
            );
        }
        for menu in &a.menus {
            let sm = crate::scriptmenu::ScriptMenu::of(menu, a);
            self.script_menus.insert(sm.name.to_ascii_lowercase(), sm);
            let menu_name = menu.window.name.as_deref();
            let compass = menu_name == Some(COMPASS_MENU);
            // A later zone's menu replaces an earlier's.
            if menu_name == Some(KILLCAM_MENU) {
                self.killcam.clear();
            }
            for item in menu.items.iter().flatten() {
                let r = &item.raw;
                let f = |o: usize| f32::from_le_bytes(r[o..o + 4].try_into().unwrap());
                let i = |o: usize| i32::from_le_bytes(r[o..o + 4].try_into().unwrap());
                let rect = MenuRect {
                    rect: [f(4), f(8), f(12), f(16)],
                    horz_align: i(20),
                    vert_align: i(24),
                    background: item
                        .window
                        .background
                        .map(|m| stub_name(&a.materials[m.index()].name).to_string()),
                };
                if compass && let Some(name) = &item.window.name {
                    self.compass.insert(name.clone(), rect.clone());
                }
                let hud_item = || MenuItem {
                    rect: rect.clone(),
                    owner_draw: i(64),
                    font: i(192),
                    text_scale: f(208),
                    text_style: i(212),
                    fore_color: [f(88), f(92), f(96), f(100)],
                    text_align: i(196),
                    text: item.text.clone().or_else(|| item.text_exp.first().cloned()),
                };
                if menu_name == Some(WEAPON_INFO_MENU)
                    && let Some(name) = &item.window.name
                {
                    self.weapon_info.insert(name.clone(), hud_item());
                }
                if menu_name == Some(STANCE_MENU) {
                    self.stance = Some(hud_item());
                }
                if menu_name == Some(KILLCAM_MENU) {
                    self.killcam.push(hud_item());
                }
                if item.kind == ITEM_GAME_MESSAGE_WINDOW && item.raw.len() >= 224 {
                    let w = MessageWindow::of(&item.raw);
                    self.windows.insert(w.index, w);
                }
            }
        }
    }

    /// Font `name` (`fonts/normalFont`), any case.
    pub fn font(&self, name: &str) -> Option<&(Font, Option<String>)> {
        self.fonts
            .iter()
            .find(|(f, _)| f.name.eq_ignore_ascii_case(name))
    }
}

/// A material's colour map: its 2D (semantic 0) or colour (2) texture's
/// image, else its first; a reference to another zone's image (`,name`)
/// by its name.
pub fn colour_image(a: &Assets, m: &Material) -> Option<String> {
    let t = m
        .textures
        .iter()
        .find(|t| matches!(t.semantic, 0 | 2) && t.image.is_some())
        .or_else(|| m.textures.iter().find(|t| t.image.is_some()))?;
    Some(stub_name(&a.images[t.image?.index()].name).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock killstreak and promotion splashes' labels.
    #[test]
    fn a_label_takes_its_text_at_its_first_placeholder() {
        assert_eq!(consolidate("&&1 Kill Streak!", "3"), "3 Kill Streak!");
        assert_eq!(consolidate("Private &&1 &&1", "I"), "Private I &&1");
        assert_eq!(consolidate("Score: ", "10"), "Score: 10");
        assert_eq!(consolidate("&&1 Kill Streak!", ""), "&&1 Kill Streak!");
    }

    #[test]
    fn tokenize_keeps_a_quoted_token_whole() {
        assert_eq!(
            tokenize("h \"\u{15}\u{15}^9walker^7: ^7vcod probe here\""),
            ["h", "\u{15}\u{15}^9walker^7: ^7vcod probe here"]
        );
        assert_eq!(tokenize("  N 2311  4 "), ["N", "2311", "4"]);
    }

    #[test]
    fn startup_dvars_follow_the_execs() {
        let read = |n: &str| {
            Some(
                match n {
                    "default_mp.cfg" => {
                        "bind TAB \"+scores\"\nexec gs.cfg // the rules\nset ui_buildLocation -60 460\n"
                    }
                    "gs.cfg" => "set scr_game_spectatetype\t\t1 // free\nseta a 2; sets b \"x y\"\n",
                    _ => return None,
                }
                .as_bytes()
                .to_vec(),
            )
        };
        assert_eq!(
            startup_dvars(read),
            [
                ("scr_game_spectatetype".to_string(), "1".to_string()),
                ("a".into(), "2".into()),
                ("b".into(), "x y".into()),
                ("ui_buildLocation".into(), "-60 460".into()),
            ]
        );
    }

    #[test]
    fn commands_by_their_first_character() {
        assert_eq!(
            Command::parse(b"h \"hi\""),
            Some(Command::Chat("hi".into()))
        );
        assert_eq!(
            Command::parse(b"d 1343 mp_spawn_usa"),
            Some(Command::ConfigString(1343, "mp_spawn_usa".into()))
        );
        assert_eq!(
            Command::parse(b"G 10"),
            Some(Command::TeamScore { team: 1, score: 10 })
        );
        assert_eq!(Command::parse(b"s 1"), Some(Command::LocalSound(1)));
        assert_eq!(Command::parse(b"k 256"), Some(Command::StopLocalSound(256)));
        // `CG_LocalSound` refuses an index past 256 and a third argument.
        assert_eq!(Command::parse(b"s 257"), None);
        assert_eq!(Command::parse(b"s 1 2"), None);
        // 1.7's connected line opens on a space: its first token is the
        // message, which no case takes.
        assert_eq!(Command::parse(b" \"MP_CONNECTED\x15spec^7\""), None);
        assert_eq!(
            Command::parse(b"v a \"1\" b \"x y\""),
            Some(Command::Dvars(vec![
                ("a".into(), "1".into()),
                ("b".into(), "x y".into())
            ]))
        );
    }

    #[test]
    fn directives_become_their_bindings() {
        let bind = |d: &str| match d {
            "+activate" => "F".to_string(),
            d => format!("<{d}>"),
        };
        assert_eq!(
            replace_directives("Press [{+activate}] to respawn", &bind),
            "Press F to respawn"
        );
        assert_eq!(
            replace_directives("[{+attack}][{+activate}] x", &bind),
            "<+attack>F x"
        );
        // A binding that spells a directive is not read again.
        let echo = |d: &str| format!("[{{{d}}}]");
        assert_eq!(replace_directives("[{a}]", &echo), "[{a}]");
        // No end token, or an empty directive: the rest stays.
        assert_eq!(replace_directives("a [{b c", &bind), "a [{b c");
        assert_eq!(replace_directives("[{}] [{x}]", &bind), "[{}] [{x}]");
    }

    #[test]
    fn scores_read_four_numbers_then_seven_a_row() {
        let args = tokenize("b 2 10 20 750 0 5 48 1 0 5 2 1 3 999 4 1 3 0");
        let s = Scores::parse(&args);
        assert_eq!(s.team_scores, [10, 20]);
        assert_eq!(s.score_limit, 750);
        assert_eq!(
            s.rows,
            [
                ScoreRow {
                    client: 0,
                    score: 5,
                    ping: 48,
                    deaths: 1,
                    status_icon: 0,
                    kills: 5,
                    assists: 2
                },
                ScoreRow {
                    client: 1,
                    score: 3,
                    ping: 999,
                    deaths: 4,
                    status_icon: 1,
                    kills: 3,
                    assists: 0
                },
            ]
        );
        // A count past the arguments reads zeros, as `Cmd_Argv` gives "".
        assert_eq!(Scores::parse(&tokenize("b 1")).rows, [ScoreRow::default()]);
    }
}
