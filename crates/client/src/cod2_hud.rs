//! A CoD2 session's 2D layer while it plays or watches: the player's own
//! HUD (`cod2_player_hud`: `hud.menu`'s owner draws, the crosshair, the
//! damage icons), the playerstate's hudelems (archived, then current;
//! `hud::cod2elem` draws them), and the scoreboard while `+scores` is
//! held. The board is `hud::scoreboard` in CoD2's style
//! (`scoreboard::COD2`, cod2-hud.md 13) over CoD2's `b`, whose words are
//! 1.1's (cod2-protocol.md 5); its team names, colours and banners are the
//! server dvar mirror's. The chat ring is `hud::chat`'s (CoD2's
//! `CG_AddToTeamChat` keeps 1.1's ring and wrap, cod2-protocol.md 3.4.1),
//! drawn and typed where CoD2 puts them (cod2-hud.md 14). An obituary
//! (event 198) becomes a kill line in the game message window, placed as
//! CoD2's, and, for the drawn player's kill or death, a centre print
//! (`CG_Obituary`, cod2-combat.md 6.1; cod2-hud.md 15).

use vcod_cod2::hudelem::HudElem;
use vcod_cod2::net::Frame;
use vcod_cod2::net::delta::ClientState2;
use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::CLIENT_STATE_FIELDS;
use vcod_cod2::net::protocol::CS_LAYOUT_118;
use vcod_common::asset_source::AssetSource;
use vcod_common::localize::Localized;
use vcod_common::net::protocol::CsRange;
use vcod_common::pk3::Pk3Fs;

use crate::cod2_player_hud::{HudInput, PlayerHud};
use crate::hud::centerprint::{CenterPrint, OBITUARY_Y};
use crate::hud::chat::{self, Chat};
use crate::hud::cod2elem::{self, ScreenPlacement};
use crate::hud::font::UiFonts;
use crate::hud::hudelem::Virtual;
use crate::hud::messages::{KillLine, Kind, Window};
use crate::hud::scoreboard::{self, BoardView, Scoreboard};
use crate::hud::{ChatField, HudQuad};
use crate::play::cod2_events::Event2;

/// `pm_type` 5: the board stays up (cod2-movement.md).
const PM_INTERMISSION: i32 = 5;
const PM_DEAD: i32 = 6;
/// `b`'s status icon `n` is configstring `0x16 + n` (cod2-protocol.md 5).
const CS_STATUS_ICONS: usize = 0x16;
/// The vote's four configstrings, 1.1's 15..18.
const CS_VOTE: usize = 15;
/// `ps.eFlags` 0x100000: this client's vote is in.
const EF_VOTED: u32 = 0x10_0000;
/// The game message window's newest baseline: `CG_Draw2D` hands
/// `Con_DrawNotify` x 6 and the `Compass` menu's y (-140 from the bottom)
/// plus 12, left- and bottom-aligned (cod2-hud.md 14).
const GAME_MESSAGES: (f32, f32) = (6.0, 480.0 - 128.0);

/// What `G_InitGame` registers the scoreboard's server dvars with, for a
/// server whose mirror lacks one (Mac 0x1addca).
const SERVER_DVAR_DEFAULTS: [(&str, &str); 8] = [
    ("g_ScoresBanner_Allies", "mpflag_american"),
    ("g_ScoresBanner_Axis", "mpflag_german"),
    ("g_ScoresBanner_None", "mpflag_none"),
    ("g_ScoresBanner_Spectators", "mpflag_spectator"),
    ("g_TeamName_Allies", "GAME_ALLIES"),
    ("g_TeamName_Axis", "GAME_AXIS"),
    ("g_TeamColor_Allies", "0.5 0.5 1 1"),
    ("g_TeamColor_Axis", "1 0.5 0.5 1"),
];

/// A dvar off the server mirror (configstrings 142..237 name them, 238..
/// hold their values; cod2-protocol.md 15.4), else its `G_InitGame`
/// default.
pub fn server_dvar<'a>(configstrings: &'a [String], name: &str) -> Option<&'a str> {
    let (first, last) = CS_LAYOUT_118.cvar_names;
    let offset = CS_LAYOUT_118.cvar_values - first;
    (first..=last)
        .find(|&i| {
            configstrings
                .get(i)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|i| configstrings.get(i + offset))
        .map(String::as_str)
        .or_else(|| {
            SERVER_DVAR_DEFAULTS
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| *v)
        })
}

/// `CG_DrawScoreboard_GetTeamColor` (Mac 0x1c514c): `g_TeamColor_Axis` or
/// `_Allies` unpacked, white for any other team.
pub fn team_rgb(configstrings: &[String], team: i32) -> [f32; 3] {
    let name = match team {
        1 => "g_TeamColor_Axis",
        2 => "g_TeamColor_Allies",
        _ => return [1.0; 3],
    };
    let mut c = [1.0; 3];
    let floats = server_dvar(configstrings, name)
        .unwrap_or("")
        .split_whitespace()
        .map_while(|f| f.parse::<f32>().ok());
    for (slot, f) in c.iter_mut().zip(floats) {
        *slot = f.clamp(0.0, 1.0);
    }
    c
}

/// The scoreboard's map name: `UI_GetMapDisplayName` (Mac 0x14f4a4), the
/// `.arena` `longname` of the map, else the name without a leading
/// `maps/` and `mp/`.
fn map_display_name(arenas: &[crate::frontend::create::Arena], map: &str) -> String {
    if let Some(a) = arenas
        .iter()
        .find(|a| a.map.eq_ignore_ascii_case(map) && !a.long_name.is_empty())
    {
        return a.long_name.clone();
    }
    let strip = |s: &str, p: &str| -> Option<String> {
        let head = s.get(..p.len())?;
        let sep = s.as_bytes().get(p.len())?;
        (head.eq_ignore_ascii_case(p) && matches!(sep, b'/' | b'\\'))
            .then(|| s[p.len() + 1..].to_string())
    };
    let map = strip(map, "maps").unwrap_or_else(|| map.to_string());
    strip(&map, "mp").unwrap_or(map)
}

pub struct Cod2Hud {
    /// The UI fonts, loaded on first use; `None` inside once that failed.
    fonts: Option<Option<UiFonts>>,
    /// The player's own HUD: `hud.menu`'s owner draws, the crosshair.
    player: PlayerHud,
    /// `cg_weaponSelect`, set by the session each frame.
    pub selected: u8,
    /// `+activate`'s key, for the hints.
    pub use_key: String,
    /// `cg_drawMantleHint`, set by the session each frame.
    pub mantle_hint: bool,
    /// `cg_drawGun`, set by the session each frame.
    pub draw_gun: bool,
    /// The session's [`crate::play::cod2::Cod2Session::interrupted`], set
    /// each frame.
    pub interrupted: bool,
    /// The session's lagometer under `cg_drawLagometer`, set each frame.
    pub lagometer: Option<crate::play::cod2_net::Lagometer>,
    /// `+scores`' board and the last `b` reply.
    pub scoreboard: Scoreboard,
    /// The `h` / `i` lines, localized, and the line being typed.
    pub chat: Chat,
    pub chat_field: Option<ChatField>,
    /// The kill lines (`CL_DeathMessagePrint`) and the obituary's centre
    /// print.
    pub kills: Window,
    /// Sound alias subtitles (`CL_SubtitlePrint`, cod2-sound.md 7.11).
    pub subtitles: Window,
    pub center: CenterPrint,
    /// `killIcon`, `wideKillIcon` and `flipKillIcon` by weapon index.
    kill_icons: std::collections::HashMap<i32, Option<(String, bool, bool)>>,
    /// Configstrings 15..18 as `CG_ConfigStringModified` keeps them.
    pub vote: crate::hud::vote::Vote,
    /// `cg_objectiveText`, which `CG_SetClientDvarFromServer` keeps for the
    /// board (Mac 0x1e2676).
    pub objective: String,
    /// The keys bound to `+attack`, `toggleads` (else `+speed`) and
    /// `+melee` (else `+melee_breath`), for a spectator's follow lines.
    pub follow_keys: [String; 3],
    /// The entity the session's crosshair trace met this frame
    /// (`cod2_names::scan`), set by the session each frame.
    pub crosshair_hit: Option<u32>,
    /// `cg_drawCrosshairNames`, `cg_drawCrosshairNamesPosX` and `PosY`.
    pub crosshair_names: (bool, i32, i32),
    names: crate::cod2_names::CrosshairNames,
    /// The game types' and the maps' display names, loaded on first use.
    game_types: Option<Vec<vcod_common::ui_menu::GameType>>,
    arenas: Option<Vec<crate::frontend::create::Arena>>,
    /// The cgame dvars the app reads each frame ([`HudDvars::read`]).
    pub dvars: HudDvars,
}

/// The CoD2 cgame's archived HUD dvars vcod honours, read each frame as
/// `CG_UpdateDvars` does (`CG_Init`, Mac 0x145214, registers them).
#[derive(Clone, Debug, PartialEq)]
pub struct HudDvars {
    /// `cg_chatHeight`, `cg_chatTime`.
    pub chat: chat::Rules,
    /// `cg_hudChatPosition` (5, 150) and `cg_hudSayPosition` (5, 126).
    pub chat_pos: [f32; 2],
    pub say_pos: [f32; 2],
    /// `cg_hudStanceHintPrints` (0): the bound keys of each stance hint
    /// command, empty while it is off.
    pub stance_keys: Option<Vec<(&'static str, String)>>,
    /// `cg_hintFadeTime` (100).
    pub hint_fade_ms: i32,
}

impl Default for HudDvars {
    fn default() -> Self {
        HudDvars {
            chat: chat::Rules::default(),
            chat_pos: [5.0, 150.0],
            say_pos: [5.0, 126.0],
            stance_keys: None,
            hint_fade_ms: 100,
        }
    }
}

impl HudDvars {
    /// The dvars off `cvar`; `keys` gives a command's bound keys as the
    /// hints print them. A missing dvar keeps its default.
    pub fn read<'a>(
        cvar: impl Fn(&str) -> Option<&'a str>,
        keys: impl Fn(&str) -> Option<String>,
    ) -> HudDvars {
        let d = HudDvars::default();
        let vec2 = |name: &str, def: [f32; 2]| {
            let Some(v) = cvar(name) else { return def };
            let mut it = v
                .split_whitespace()
                .map(|t| t.parse::<f32>().unwrap_or(0.0));
            [it.next().unwrap_or(0.0), it.next().unwrap_or(0.0)]
        };
        let on = |name: &str| {
            cvar(name).is_some_and(|v| v.trim().parse::<f32>().is_ok_and(|f| f != 0.0))
        };
        HudDvars {
            chat: chat::Rules::read(&cvar, false),
            chat_pos: vec2("cg_hudChatPosition", d.chat_pos),
            say_pos: vec2("cg_hudSayPosition", d.say_pos),
            stance_keys: on("cg_hudStanceHintPrints").then(|| {
                crate::cod2_player_hud::stance_hint_commands()
                    .filter_map(|c| Some((c, keys(c)?)))
                    .collect()
            }),
            hint_fade_ms: cvar("cg_hintFadeTime").map_or(d.hint_fade_ms, |v| {
                v.trim().parse::<f32>().map_or(0, |f| f as i32)
            }),
        }
    }
}

impl Default for Cod2Hud {
    fn default() -> Self {
        let mut scoreboard = Scoreboard::default();
        scoreboard.style = &scoreboard::COD2;
        let mut kills = Window::new(Kind::Game);
        kills.origin = Some(GAME_MESSAGES);
        let mut center = CenterPrint::default();
        center.cod2 = true;
        Cod2Hud {
            fonts: None,
            player: PlayerHud::default(),
            selected: 0,
            use_key: String::new(),
            mantle_hint: true,
            draw_gun: true,
            interrupted: false,
            lagometer: None,
            scoreboard,
            chat: Chat::default(),
            chat_field: None,
            kills,
            subtitles: Window::new(Kind::Subtitle),
            center,
            kill_icons: Default::default(),
            vote: Default::default(),
            objective: String::new(),
            follow_keys: Default::default(),
            crosshair_hit: None,
            crosshair_names: (true, 300, 180),
            names: Default::default(),
            game_types: None,
            arenas: None,
            dvars: HudDvars::default(),
        }
    }
}

/// `CG_DrawDisconnect` (Mac 0x1cd848), drawn by `CG_Draw2D` after the
/// board: `CGAME_CONNECTIONINTERUPTED` centred at y 100 in the default font
/// at scale 0.5, and the 48-unit `net_disconnect` icon at (296, 416) on
/// the half-second when `cg.time & 0x200` is clear.
fn connection_interrupted(
    fonts: &UiFonts,
    loc: &Localized,
    now: i32,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    use crate::hud::font::Slot;
    use crate::hud::player::{menu_text_rgba, text_width};
    let text = loc.message("CGAME_CONNECTIONINTERUPTED");
    let font = fonts.pick(Slot::Default, 0.5, 480.0 * v.scale);
    let w = text_width(font, &text, 0.5) as i32;
    let x = ((640 - w) / 2) as f32;
    menu_text_rgba(font, &text, (x, 100.0), 0.5, [1.0; 4], v, out);
    if now & 0x200 == 0 {
        out.push(v.quad(296.0, 416.0, 48.0, 48.0, [1.0; 4], "net_disconnect"));
    }
}

/// `CG_Draw2D`'s lagometer (Mac 0x1d114a, inline before
/// `CG_DrawDisconnect`), every placement right- and bottom-aligned (3, 3):
/// the `lagometer` material at (-55, -140), 48 square, then 48 one-unit
/// columns of `white` leftwards from x -7, newest first. A frame's offset
/// from the newest snapshot times 16/300 (0.0533) rises yellow from y -124
/// when ahead and hangs blue below it when behind, each at most 16; a
/// snapshot's ping times 24/900 (0.0267) rises from y -92, at most 24,
/// green or yellow when `snapFlags & 1`, and a refused one is a full red
/// column (cod2-protocol.md 18.6).
fn lagometer(l: &crate::play::cod2_net::Lagometer, sp: &ScreenPlacement, out: &mut Vec<HudQuad>) {
    const YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    let mut quad = |x: f32, y: f32, w: f32, h: f32, rgba: [f32; 4], texture: &str| {
        let (x0, w) = sp.axis(0, x, w, 3);
        let (y0, h) = sp.axis(1, y, h, 3);
        out.push(HudQuad {
            verts: [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba,
            texture: texture.to_string(),
        });
    };
    quad(-55.0, -140.0, 48.0, 48.0, [1.0; 4], "lagometer");
    for (i, offset) in l.frames(48).enumerate() {
        let x = -7.0 - i as f32;
        let v = offset as f32 * 0.053_333_335;
        if v > 0.0 {
            let (y, h) = if v <= 16.0 {
                (-124.0 - v, v)
            } else {
                (-140.0, 16.0)
            };
            quad(x, y, 1.0, h, YELLOW, "white");
        } else if v < 0.0 {
            quad(x, -124.0, 1.0, (-v).min(16.0), BLUE, "white");
        }
    }
    for (i, (ping, flags)) in l.snaps(48).enumerate() {
        let x = -7.0 - i as f32;
        if ping > 0 {
            let v = ping as f32 * 0.026_666_667;
            let (y, h) = if v <= 24.0 {
                (-92.0 - v, v)
            } else {
                (-116.0, 24.0)
            };
            let rgba = if flags & 1 != 0 { YELLOW } else { GREEN };
            quad(x, y, 1.0, h, rgba, "white");
        } else if ping < 0 {
            quad(x, -116.0, 1.0, 24.0, RED, "white");
        }
    }
}

/// What `CG_Obituary` makes of one obituary: the kill line, and the
/// unlocalized centre print when the drawn player killed or was killed.
#[derive(Debug, PartialEq)]
pub struct Obit {
    pub line: KillLine,
    pub center: Option<String>,
}

/// `CG_Obituary` (inline in `CG_EntityEvent`, Mac 0x1e4650, event 198) over
/// a temp entity's `otherEntityNum` (the victim), `attackerEntityNum` (64
/// and up: none) and `parm` (`0x80 | mod`, or the weapon's index), with
/// the names off the snapshot's client states. `None` when the victim or
/// the attacker has none, as retail returns. `kill_icon` gives a weapon
/// index's `killIcon`, `wideKillIcon` and `flipKillIcon`; `team_rgb` a
/// team's colour (`CG_DrawScoreboard_GetTeamColor`).
pub fn obituary(
    victim: i32,
    attacker: i32,
    parm: i32,
    own: i32,
    clients: &[ClientState2],
    kill_icon: impl FnOnce(i32) -> Option<(String, bool, bool)>,
    team_rgb: impl Fn(i32) -> [f32; 3],
) -> Option<Obit> {
    let (icon, icon_wide, icon_flip) = if parm & 0x80 != 0 {
        let icon = match parm & 0x7f {
            7 => "killiconmelee",
            8 => "killiconheadshot",
            9 => "killiconcrush",
            11 => "killiconfalling",
            12 => "killiconsuicide",
            _ => "killicondied",
        };
        (icon.to_string(), false, false)
    } else {
        kill_icon(parm)
            .filter(|(icon, ..)| !icon.is_empty())
            .unwrap_or_else(|| ("killicondied".into(), false, false))
    };
    let named = |n: i32| {
        let (name, team) = board_client(clients, u32::try_from(n).ok()?)?;
        Some((name, team, team_rgb(team)))
    };
    let victim_info = named(victim)?;
    let attacker_info = if (0..64).contains(&attacker) {
        Some(named(attacker)?)
    } else {
        None
    };
    // A name as `CG_Obituary` copies it: 31 bytes, then `^7`.
    let cut = |name: &str| -> String { name.chars().take(31).collect() };
    let teammates = |a: &(String, i32, [f32; 3])| a.1 != 0 && a.1 == victim_info.1;
    let center = match &attacker_info {
        _ if attacker == victim => None,
        Some(a) if attacker == own => Some(if teammates(a) {
            format!(
                "CGAME_YOUKILLED\u{15}^1&&2^7 {}^7\u{14}CGAME_TEAMMATE",
                cut(&victim_info.0)
            )
        } else {
            format!("CGAME_YOUKILLED\u{15}{}^7", cut(&victim_info.0))
        }),
        Some(a) if victim == own => Some(if teammates(a) {
            format!(
                "CGAME_YOUWEREKILLED\u{15}^1&&2^7 {}^7\u{14}CGAME_TEAMMATE",
                cut(&a.0)
            )
        } else {
            format!("CGAME_YOUWEREKILLED\u{15}{}^7", cut(&a.0))
        }),
        _ => None,
    };
    let line_name = crate::hud::killfeed::line_name;
    Some(Obit {
        line: KillLine {
            attacker: attacker_info
                .filter(|_| attacker != victim)
                .map(|(name, _, rgb)| (line_name(&name), rgb)),
            icon,
            icon_wide,
            icon_flip,
            victim: (line_name(&victim_info.0), victim_info.2),
        },
        center,
    })
}

/// A 1-based `G_LocalizedStringIndex` index's reference, translated.
fn localized(frame: &Frame, loc: &Localized, index: i32) -> Option<String> {
    let s = frame
        .configstrings
        .get(CS_LAYOUT_118.slot(CsRange::Localized, index)?);
    (!s.is_empty()).then(|| loc.get(s).unwrap_or(s).to_string())
}

/// A `b` row's name and team: the snapshot's client state for `n`
/// (`cgs.clientinfo` is filled from them), `None` for a client the
/// snapshot does not carry, whose row the board leaves out.
pub fn board_client(clients: &[ClientState2], n: u32) -> Option<(String, i32)> {
    let c = clients.iter().find(|c| c.number == n)?;
    let team = c.f[field_index(&CLIENT_STATE_FIELDS, "team")] as i32;
    Some((c.name(), team))
}

/// `m:ss` of a timer element at `now`: counting down to `time` (7, 9) or
/// up from it (8, 10), whole seconds.
fn timer(e: &HudElem, now: i32) -> String {
    let ms = match e.kind {
        8 | 10 => now - e.time,
        _ => e.time - now,
    }
    .max(0);
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

impl Cod2Hud {
    /// The frame's obituaries into the kill lines and the centre print.
    pub fn on_events(
        &mut self,
        events: &[Event2],
        frame: &Frame,
        fs: &dyn AssetSource,
        loc: &Localized,
        now_ms: i32,
    ) {
        self.player.on_events(events, frame, now_ms);
        for ev in events
            .iter()
            .filter(|e| e.event == vcod_cod2::net::events::EV_OBITUARY)
        {
            let Some(obit) = self.obit(ev, frame, fs) else {
                continue;
            };
            if let Some(text) = &obit.center {
                self.center.print(&loc.message(text), OBITUARY_Y, now_ms);
            }
            self.kills.push_kill(obit.line, now_ms);
        }
    }

    /// The last `CG_DamageFeedback`'s view kick (`play::cod2_view`).
    pub fn damage_kick(&self) -> crate::play::cod2_view::DamageKick {
        self.player.damage_kick()
    }

    /// [`obituary`] for one event off the snapshot that carried it.
    pub fn obit(&mut self, ev: &Event2, frame: &Frame, fs: &dyn AssetSource) -> Option<Obit> {
        let attacker = frame
            .snap
            .entities
            .iter()
            .find(|e| e.number == ev.entity)
            .map_or(1022, |e| e.raw.i32("attackerEntityNum"));
        let own = frame.snap.ps.raw.i32("clientNum");
        let icons = &mut self.kill_icons;
        let cs = frame.configstrings.all();
        obituary(
            ev.other_entity,
            attacker,
            ev.parm,
            own,
            &frame.snap.clients,
            |weapon| {
                icons
                    .entry(weapon)
                    .or_insert_with(|| {
                        let name = frame.configstrings.weapon_name(weapon)?;
                        let def = vcod_cod2::weapon::load(fs, name).ok()?.def;
                        Some((
                            def.str("killIcon").to_string(),
                            def.int("wideKillIcon") != 0,
                            def.int("flipKillIcon") != 0,
                        ))
                    })
                    .clone()
            },
            |team| team_rgb(cs, team),
        )
    }

    /// The quads for a `w` x `h` window at server time `now`; `clock` is
    /// the HUD clock in seconds the scoreboard fades on.
    pub fn build(
        &mut self,
        frame: &Frame,
        fs: &Pk3Fs,
        loc: &Localized,
        (w, h): (f32, f32),
        now: i32,
        clock: f32,
    ) -> Vec<HudQuad> {
        let ps = &frame.snap.ps;
        let mut out = Vec::new();
        let fonts = self
            .fonts
            .get_or_insert_with(|| match UiFonts::load(fs) {
                Ok(f) => Some(f),
                Err(e) => {
                    log::warn!("cod2 hud: no UI fonts: {e}");
                    None
                }
            })
            .as_ref();
        let Some(fonts) = fonts else {
            return out;
        };
        let stance_keys = self.dvars.stance_keys.clone();
        let lookup = move |c: &str| {
            stance_keys
                .as_ref()?
                .iter()
                .find(|(k, _)| *k == c)
                .map(|(_, v)| v.clone())
        };
        let input = HudInput {
            selected: self.selected,
            use_key: &self.use_key,
            mantle_hint: self.mantle_hint,
            draw_gun: self.draw_gun,
            stance_keys: self.dvars.stance_keys.as_ref().map(|_| &lookup as _),
            hint_fade_ms: self.dvars.hint_fade_ms,
        };
        self.chat.set_rules(self.dvars.chat);
        self.player
            .build(frame, fs, loc, fonts, (w, h), now, &input, &mut out);
        self.player.spectator(frame, fonts, (w, h), loc, &mut out);
        // `CG_Draw2D` (Mac 0x1d114a): the names after the crosshair, or
        // after a free spectator's line; none for the dead.
        let (names_on, x, y) = self.crosshair_names;
        if names_on && ps.pm_type < PM_DEAD && ps.pm_type != PM_INTERMISSION {
            self.names.update(self.crosshair_hit, now);
            if let Some((name, rgba)) = self.names.shown(frame, now) {
                self.player
                    .crosshair_name(&name, rgba, (x, y), fonts, (w, h), loc, &mut out);
            }
        }
        let elems: Vec<HudElem> = ps
            .raw
            .hud_archival
            .iter()
            .chain(ps.raw.hud_current.iter())
            .map(HudElem::from_wire)
            .collect();
        let sp = ScreenPlacement::new(w, h);
        let text_of = |e: &HudElem| {
            let label = localized(frame, loc, e.label).unwrap_or_default();
            let body = match e.kind {
                1 => localized(frame, loc, e.text).unwrap_or_default(),
                2 => format!("{}", e.value),
                7..=10 => timer(e, now),
                _ => String::new(),
            };
            match label.find("&&1") {
                Some(i) => format!("{}{body}{}", &label[..i], &label[i + 3..]),
                None => format!("{label}{body}"),
            }
        };
        let material_of = |i: i32| frame.configstrings.material(i).map(str::to_string);
        for foreground in [false, true] {
            out.extend(cod2elem::build(
                &elems,
                foreground,
                fonts,
                &sp,
                now,
                &text_of,
                &material_of,
            ));
        }
        self.player
            .build_late(frame, fonts, (w, h), now, loc, &mut out);
        self.scoreboard.intermission = ps.pm_type == PM_INTERMISSION;
        let info = |key: &str| {
            vcod_common::net::info_value_for_key(frame.configstrings.get(0), key).unwrap_or("")
        };
        let cs = frame.configstrings.all();
        // `UI_GetGameTypeDisplayName` (Mac 0x14f58c): the first token of
        // the game type's `.txt`, translated by the board.
        let game_types = self
            .game_types
            .get_or_insert_with(|| vcod_common::ui_menu::game_types(fs));
        let gametype = info("g_gametype");
        let gametype = game_types
            .iter()
            .find(|g| g.script.eq_ignore_ascii_case(gametype))
            .map_or(gametype, |g| g.name.as_str());
        let arenas = self
            .arenas
            .get_or_insert_with(|| crate::frontend::create::load_arenas(fs));
        let map = map_display_name(arenas, info("mapname"));
        let objective = &self.objective;
        let cvar = |name: &str| {
            if name.eq_ignore_ascii_case("cg_objectiveText") {
                return Some(objective.clone());
            }
            server_dvar(cs, name).map(str::to_string)
        };
        let client = |n: u32| board_client(&frame.snap.clients, n);
        // `CG_ServerCommand`'s `b` (Mac 0x1e2ea4) takes icons 1 to 8; 0 is
        // none, and slot 0x16 the `winner` string.
        let status_icon = |n: i32| {
            let n = usize::try_from(n).ok().filter(|n| (1..=8).contains(n))?;
            let s = frame.configstrings.get(CS_STATUS_ICONS + n);
            (!s.is_empty()).then(|| s.to_string())
        };
        let view = BoardView {
            now: clock,
            own_client: u32::try_from(frame.snap.ps.raw.i32("clientNum")).ok(),
            client: &client,
            cvar: &cvar,
            loc,
            bound_key: &|_| None,
            gametype,
            map: &map,
            hostname: info("sv_hostname"),
            address: None,
            status_icon: &status_icon,
        };
        let v = Virtual::new((w, h));
        self.chat
            .build_cod2(fonts, now, &sp, self.dvars.chat_pos, &mut out);
        // `CG_Draw2D` (Mac 0x1d114a): the board, then the centre print,
        // then the game messages, which a held board hides.
        let board = self.scoreboard.build(&view, fonts, &v, &mut out);
        self.center.build(fonts, now, &v, &mut out);
        if !board {
            self.kills.build(fonts, now, &v.left(), &mut out);
            // `cg + 0x2cd14`, the killcam, is `ps.deltaTime` set
            // (`CG_SetNextSnap`, Mac 0x1dee08).
            let killcam = ps.raw.i32("deltaTime") != 0;
            self.player.follow(
                frame,
                fonts,
                (w, h),
                loc,
                &self.follow_keys,
                killcam,
                &mut out,
            );
        }
        // `CG_Draw2D` draws the subtitles under `cg_subtitles` (default
        // on) whether or not the board is up (Mac 0x1d1884).
        self.subtitles.build(fonts, now, &v.left(), &mut out);
        // `CG_Draw2D`'s vote lines (Mac 0x1d114a): 1.1's two lines at x 8,
        // y 200 and 210, the end in server time, `eFlags` 0x100000 the
        // voted bit (cod2-protocol.md 3.4.2). No sound marks a change.
        let cs: Vec<String> = (0..4)
            .map(|i| frame.configstrings.get(CS_VOTE + i).to_string())
            .collect();
        self.vote.update(&cs, 0, false, now);
        let voted = ps.eflags as u32 & EF_VOTED != 0;
        self.vote
            .build(fonts, loc, now, voted, &|_| None, &v.left(), &mut out);
        if let Some(l) = &self.lagometer {
            lagometer(l, &sp, &mut out);
        }
        if self.interrupted {
            connection_interrupted(fonts, loc, now, &v, &mut out);
        }
        if let Some(field) = &self.chat_field {
            chat::field_cod2(field, fonts, loc, &sp, self.dvars.say_pos, &mut out);
        }
        out
    }
}

/// `quads` as text, one per line (texture, rgba, then each corner's x y
/// u v), for `renderer/hud_shot.rs` to draw: what the headless probe's
/// `VCOD_COD2_HUD_DUMP` writes.
pub fn dump_quads(quads: &[HudQuad]) -> String {
    let mut s = String::new();
    for q in quads {
        s.push_str(&q.texture);
        for v in q.rgba {
            s.push_str(&format!(" {v}"));
        }
        for (p, uv) in q.verts.iter().zip(&q.uvs) {
            s.push_str(&format!(" {} {} {} {}", p[0], p[1], uv[0], uv[1]));
        }
        s.push('\n');
    }
    s
}

/// [`dump_quads`]' text back; a malformed line is skipped.
#[cfg(test)]
pub fn load_quads(text: &str) -> Vec<HudQuad> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split(' ');
            let texture = it.next()?.to_string();
            let n: Vec<f32> = it.map(|v| v.parse().ok()).collect::<Option<_>>()?;
            if n.len() != 20 {
                return None;
            }
            let corner = |i: usize| ([n[4 + i * 4], n[5 + i * 4]], [n[6 + i * 4], n[7 + i * 4]]);
            let c = [0, 1, 2, 3].map(corner);
            Some(HudQuad {
                verts: c.map(|c| c.0),
                uvs: c.map(|c| c.1),
                rgba: [n[0], n[1], n[2], n[3]],
                texture,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client state named `name` on `team`, as a snapshot carries it.
    fn client(number: u32, name: &str, team: u32) -> ClientState2 {
        let mut c = ClientState2::null(number);
        c.f[field_index(&CLIENT_STATE_FIELDS, "team")] = team;
        let mut bytes = name.as_bytes().to_vec();
        bytes.resize(32, 0);
        for k in 0..8 {
            let i = field_index(&CLIENT_STATE_FIELDS, &format!("name[{}]", k * 4));
            c.f[i] = u32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().unwrap());
        }
        c
    }

    /// The HUD dvars: a vec2 dvar's two numbers, the stance prints' keys
    /// only while they are on, defaults for what is missing.
    #[test]
    fn hud_dvars_read_the_positions_and_the_hint_keys() {
        let off = HudDvars::read(|_| None, |_| Some("X".into()));
        assert_eq!(off, HudDvars::default());
        let cvar = |n: &str| match n {
            "cg_hudChatPosition" => Some("20 300"),
            "cg_hudStanceHintPrints" => Some("1"),
            "cg_hintFadeTime" => Some("250"),
            "cg_chatHeight" => Some("4"),
            _ => None,
        };
        let keys = |c: &str| (c == "+gostand").then(|| "SPACE".to_string());
        let d = HudDvars::read(cvar, keys);
        assert_eq!(d.chat_pos, [20.0, 300.0]);
        assert_eq!(d.say_pos, [5.0, 126.0]);
        assert_eq!(d.hint_fade_ms, 250);
        assert_eq!(d.chat.height, 4);
        assert_eq!(d.stance_keys, Some(vec![("+gostand", "SPACE".to_string())]));
        assert_eq!(crate::cod2_player_hud::stance_hint_commands().count(), 10);
    }

    /// On a 640x480 window the lagometer sits 55 in from the bottom right:
    /// a frame 75 ms behind the newest snapshot hangs a 4-unit blue bar from
    /// y 356, a ping of 450 a 12-unit green one up from 388, a refused
    /// snapshot a full red column.
    #[test]
    fn the_lagometer_draws_from_the_bottom_right() {
        let mut l = crate::play::cod2_net::Lagometer::default();
        // `latest` 0: the frame's time is its offset.
        l.frame(-75);
        l.parsed(5, 450, 0);
        l.read(&[(4, false), (5, true)]);
        let mut out = Vec::new();
        lagometer(&l, &ScreenPlacement::new(640.0, 480.0), &mut out);
        let rect = |q: &HudQuad| {
            let [a, _, c, _] = q.verts;
            [a[0], a[1], c[0] - a[0], c[1] - a[1]].map(|v| (v * 100.0).round() / 100.0)
        };
        assert_eq!(
            (rect(&out[0]), out[0].texture.as_str()),
            ([585.0, 340.0, 48.0, 48.0], "lagometer")
        );
        assert_eq!(
            (rect(&out[1]), out[1].rgba[2]),
            ([633.0, 356.0, 1.0, 4.0], 1.0)
        );
        assert_eq!(
            (rect(&out[2]), out[2].rgba[1]),
            ([633.0, 376.0, 1.0, 12.0], 1.0)
        );
        assert_eq!(
            (rect(&out[3]), out[3].rgba[0]),
            ([632.0, 364.0, 1.0, 24.0], 1.0)
        );
        assert_eq!(out.len(), 4);
    }

    /// The mirror as retail's mp_toujane tdm sent it (cod2-hud.md 13):
    /// names from 142, values 96 on; a name it lacks reads the default.
    #[test]
    fn server_dvars_come_off_the_mirror() {
        let mut cs = vec![String::new(); 400];
        cs[149] = "g_TeamColor_Allies".into();
        cs[245] = "0.25098 0.25098 0.74902 0".into();
        cs[150] = "g_teamname_allies".into();
        cs[246] = "MPUI_BRITISH".into();
        assert_eq!(server_dvar(&cs, "g_TeamName_Allies"), Some("MPUI_BRITISH"));
        assert_eq!(server_dvar(&cs, "g_TeamName_Axis"), Some("GAME_AXIS"));
        assert_eq!(team_rgb(&cs, 2), [0.25098, 0.25098, 0.74902]);
        assert_eq!(team_rgb(&cs, 1), [1.0, 0.5, 0.5]);
        assert_eq!(team_rgb(&cs, 3), [1.0; 3]);
    }

    /// `UI_GetMapDisplayName`: the arena's `longname`, else the bare name.
    #[test]
    fn the_board_names_the_map_by_its_arena() {
        let arenas = [crate::frontend::create::Arena {
            map: "mp_toujane".into(),
            long_name: "Toujane, Tunisia".into(),
            gametypes: None,
        }];
        assert_eq!(map_display_name(&arenas, "MP_Toujane"), "Toujane, Tunisia");
        assert_eq!(map_display_name(&arenas, "maps/mp/mp_custom"), "mp_custom");
        assert_eq!(map_display_name(&arenas, "mp_custom"), "mp_custom");
    }

    /// A weapon's `flipKillIcon` mirrors its icon; a means of death's icon
    /// never flips. Names take the mirror's team colours.
    #[test]
    fn an_obituary_flips_the_weapon_icon() {
        let clients = [client(0, "a", 1), client(1, "b", 2)];
        let rgb = |t: i32| [t as f32, 0.0, 0.0];
        let icon = |_| Some(("gfx/icons/hud@kar98.tga".to_string(), true, true));
        let obit = obituary(1, 0, 3, 5, &clients, icon, rgb).unwrap();
        assert!(obit.line.icon_flip && obit.line.icon_wide);
        assert_eq!(obit.line.attacker, Some(("a".into(), [1.0, 0.0, 0.0])));
        assert_eq!(obit.line.victim, ("b".into(), [2.0, 0.0, 0.0]));
        let obit = obituary(1, 0, 0x80 | 8, 5, &clients, icon, rgb).unwrap();
        assert_eq!(obit.line.icon, "killiconheadshot");
        assert!(!obit.line.icon_flip);
    }

    /// Retail's reply to `score` with two players on a `dm` level
    /// (cod2-protocol.md 3.4.1, captured live): both rows drawn under the
    /// no-team block, their names off the snapshot's client states.
    #[test]
    fn a_two_player_dm_reply_draws_two_rows() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let fonts = UiFonts::load(&fs).unwrap();
        let loc = Localized::load(&fs);
        let clients = [client(0, "vcodplay", 0), client(1, "other", 0)];
        assert_eq!(board_client(&clients, 1), Some(("other".into(), 0)));
        assert_eq!(board_client(&clients, 2), None);
        let mut board = Scoreboard::new();
        board.press(0.0);
        let reply = "b 2 0 0 0 0 0 0 0 1 0 0 0 0";
        board.on_server_command(&reply.split(' ').map(str::to_string).collect::<Vec<_>>());
        let lookup = |n: u32| board_client(&clients, n);
        let view = BoardView {
            now: 0.0,
            own_client: Some(0),
            client: &lookup,
            cvar: &|_| None,
            loc: &loc,
            bound_key: &|_| None,
            gametype: "dm",
            map: "mp_toujane",
            hostname: "retail",
            address: None,
            status_icon: &|_| None,
        };
        let mut out = Vec::new();
        assert!(board.build(&view, &fonts, &Virtual::new((640.0, 480.0)), &mut out));
        assert_eq!(board.rows_drawn(), 2);
    }
}
