//! On-screen HUD: chat, game messages and kill lines, Tab scoreboard and status header composed
//! into the quads [`crate::renderer::Renderer::set_hud_quads`] draws.

pub mod centerprint;
pub mod chat;
// Nothing feeds it yet: CoD2 runs fly mode only, with no server to send hudelems.
#[allow(dead_code)]
pub mod cod2elem;
pub mod font;
pub mod friends;
pub mod hudelem;
pub mod hudmenu;
pub mod killfeed;
pub mod messages;
pub mod player;
pub mod scope;
pub mod scoreboard;
pub mod status;
pub mod timeout;
pub mod uo;
pub mod vote;
pub mod weapstrip;

use std::collections::{BTreeMap, HashMap};

use crate::fx::registry::EV_OBITUARY;
use crate::play::input::{EF_CROUCH, EF_PRONE};
use vcod_common::localize::Localized;
use vcod_common::net::NetEvent;
use vcod_common::net::events::GameEvent;
use vcod_common::net::flags::{PM_DEAD, PMF_FOLLOW, PMF_PRONE_BLOCKED};
use vcod_common::net::msg::{ClientState, EntityState, HudElem, PlayerState};
use vcod_common::net::protocol::{CsRange, Protocol};
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::Stance;
use vcod_common::pmove::predict::Predicted;
use vcod_common::pmove::weapon::SpreadStance;
use vcod_common::weapon::WeaponDef;

use chat::Chat;
use font::UiFonts;
use friends::Sighting;
use hudelem::Virtual;
use player::{DamageFeedback, PlayerHud, PlayerView};
use scope::GunPose;
use scoreboard::Scoreboard;

/// Resolution knob on top of `Font::unit_scale`; a HUD-scale setting would
/// change only this.
pub const HUD_SCALE: f32 = 1.0;

/// One screen-space textured quad, pixel coordinates with the origin at
/// the top-left of the window. The renderer converts to clip space.
pub struct HudQuad {
    pub verts: [[f32; 2]; 4], // ring order [0,1,2, 0,2,3]: TL, TR, BR, BL
    pub uvs: [[f32; 2]; 4],
    pub rgba: [f32; 4],
    pub texture: String, // pk3 image name, extensionless ok
}

/// A texture name's prefix that draws its quad blended `SrcAlpha One`
/// (CoD4's font glow materials, `fonts/*_glow`), the image the rest names.
pub const ADDITIVE: &str = "$add:";

/// Retail's default shader (`<default>`, CoDMP.exe 0x4fd780) as a HUD
/// texture: the `*default` image drawn opaque and uncoloured.
pub const DEFAULT_SHADER: &str = "$default";

/// `name`, or [`DEFAULT_SHADER`] when it resolves to no image: a 2D
/// registration of a missing image returns handle 0 (`RE_RegisterShaderNoMip`
/// 0x4fcae0), which `RE_StretchPic` draws as the default shader.
pub fn default_if_missing(name: &str) -> String {
    format!("{DEFAULT_SHADER}:{name}")
}

/// `unknown` counts inputs a build could not resolve (an obituary victim with
/// no `HudFrame.clients` entry); shown on the F3 overlay.
pub struct Hud {
    /// `normal` is the body text (chat), `big` the status header and the
    /// scoreboard's headings.
    fonts: UiFonts,
    pub chat: Chat,
    /// `e`/`f` and kill lines, and `c`/`g` lines.
    game_messages: messages::Window,
    bold_messages: messages::Window,
    /// `CG_Obituary`'s "You killed" line.
    center_print: centerprint::CenterPrint,
    /// The `messagemode` line being typed: `main.rs` edits it, the HUD
    /// draws it.
    pub chat_field: Option<ChatField>,
    /// `main.rs`'s Tab handler holds it and sends the `score` requests.
    pub scoreboard: Scoreboard,
    /// `UI_LoadGameTypes`' list, which names the board's game type.
    game_types: Vec<vcod_common::ui_menu::GameType>,
    /// Per-CS7-index `(killIcon, wideKillIcon)`. `None` caches a failed load
    /// so it is tried once.
    kill_icons: HashMap<i32, Option<(String, bool)>>,
    player: PlayerHud,
    /// A mod's own `hud.menu` items.
    hud_menu: hudmenu::HudMenu,
    /// 1.5's match timeout banner.
    timeout: timeout::MatchTimeout,
    /// The vote lines.
    vote: vote::Vote,
    complaint: vote::Complaint,
    /// The newest snapshot's `(clientNum, stats[5])` ([`Self::track_spawn`]);
    /// `None` from a gamestate or a restart until the next snapshot.
    spawn: Option<(i32, i32)>,
    /// A vote draw owes `player_talk`; `main.rs` takes it.
    pub vote_sound: bool,
    pub unknown: u64,
    /// The bob cvars the gun, sight and eye bobs read: the defaults, or a
    /// 1.5 server's through the 140/204 mirror. `main.rs` keeps them.
    pub bob: vcod_common::pmove::BobCvars,
    /// The view last drawn, (pitch, yaw, roll) wire degrees: the refdef
    /// the next hit's kick is split along. `main.rs` sets it each frame.
    pub drawn_view: Option<[f32; 3]>,
}

/// Per-frame inputs the composer's elements read from.
pub struct HudFrame<'a> {
    pub now: f32,
    pub screen_w: f32,
    pub screen_h: f32,
    pub configstrings: &'a [String],
    pub clients: &'a BTreeMap<u32, ClientState>,
    pub protocol: &'a Protocol,
    /// Server-clock ms.
    pub server_time: i32,
    /// The serverTime of the snapshot the render clock interpolates from,
    /// retail's `cg.snap->serverTime`, which the crosshair's stance blend
    /// reads the eye's leg against.
    pub snap_time: i32,
    /// Lazy weapon-file loads for killfeed icons.
    pub fs: &'a Pk3Fs,
    /// The newest snapshot's playerstate: ours, or the followed player's.
    /// The native HUD and the hudelems are drawn whoever it belongs to.
    pub ps: Option<&'a PlayerState>,
    /// The newest snapshot's entities, for teammates on the compass.
    pub entities: &'a BTreeMap<u32, EntityState>,
    /// Our replay while predicting; its weapon, ammo, spread and stance stand
    /// in for the snapshot's.
    pub predicted: Option<&'a Predicted>,
    /// `ps` is ours and alive (`pm_type` 0 or 1). vcod's status header is
    /// drawn otherwise.
    pub local_player: bool,
    /// Configstring 7's defs, index = weapon number.
    pub weapons: &'a [Option<WeaponDef>],
    pub localized: &'a Localized,
    /// The camera's yaw in degrees, and its eye.
    pub view_yaw: f32,
    pub eye: [f32; 3],
    /// The drawn horizontal fov, degrees.
    pub fov: f32,
    /// An entity's current origin, for objectives placed on one and
    /// teammates on the compass.
    pub entity_origin: &'a dyn Fn(i32) -> Option<[f32; 3]>,
    /// The `weapon` of the gun `ps` rides, the entity `viewlocked_entNum`
    /// names, which picks the mounted reticle.
    pub turret_weapon: Option<usize>,
    /// United Offensive: the world yaw of the ridden vehicle's `tag_turret`.
    pub vehicle_turret_yaw: Option<f32>,
    /// A client cvar: a server `v`, else the 140/204 mirror. Mod `hud.menu`
    /// items read it.
    pub cvar: &'a dyn Fn(&str) -> Option<String>,
    /// The key text a command is bound to, `None` while unbound.
    pub bound_key: &'a dyn Fn(&str) -> Option<String>,
    pub draw: DrawToggles,
    /// The cgame's `cg_weaponSelect` (0x301cbb0c), `PlayInput::selected`:
    /// the weapon name, the mode icon and the ammo count read it.
    pub weapon_select: u8,
    /// The gamestate's client number, the local client's.
    pub own_client: Option<u32>,
    /// The server's `ip:port`, which the scoreboard's footer shows.
    pub server_address: Option<&'a str>,
}

/// `cg_drawCrosshair` and `cg_drawStatus`, read as the cgame's vmCvar
/// integers (docs/research/cod11-hud-protocol.md, "Which views draw it").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawToggles {
    /// The weapon crosshair and the mounted gun's reticle.
    pub crosshair: bool,
    /// The `hud.menu` HUD (owner draws included) and the hudelems.
    pub status: bool,
}

impl Default for DrawToggles {
    fn default() -> Self {
        DrawToggles {
            crosshair: true,
            status: true,
        }
    }
}

impl Hud {
    pub fn new(fs: &Pk3Fs) -> Result<Hud, String> {
        Ok(Hud {
            fonts: UiFonts::load(fs)?,
            chat: Chat::new(),
            game_messages: messages::Window::new(messages::Kind::Game),
            bold_messages: messages::Window::new(messages::Kind::Bold),
            center_print: Default::default(),
            chat_field: None,
            scoreboard: Scoreboard::new(),
            game_types: vcod_common::ui_menu::game_types(fs),
            kill_icons: HashMap::new(),
            player: PlayerHud::default(),
            hud_menu: hudmenu::HudMenu::load(fs),
            timeout: timeout::MatchTimeout::default(),
            vote: vote::Vote::default(),
            complaint: vote::Complaint::default(),
            spawn: None,
            vote_sound: false,
            unknown: 0,
            bob: Default::default(),
            drawn_view: None,
        })
    }

    /// A download reopened the search path: reload the fonts and `hud.menu`,
    /// and drop the kill icons read out of the old weapon files. A font the new paks do
    /// not parse keeps the old one.
    pub fn reopen(&mut self, fs: &Pk3Fs) -> Result<(), String> {
        self.fonts = UiFonts::load(fs)?;
        self.kill_icons.clear();
        self.hud_menu = hudmenu::HudMenu::load(fs);
        self.game_types = vcod_common::ui_menu::game_types(fs);
        Ok(())
    }

    /// Net does not filter `ServerCommand`; `Scoreboard::on_server_command`
    /// ignores anything but `b`. `CG_ServerCommand` (0x3002e0d0) localizes
    /// a chat line or a message through `loc` before it is kept.
    pub fn on_net_event(
        &mut self,
        ev: &NetEvent,
        now: f32,
        loc: &Localized,
        bound: &dyn Fn(&str) -> Option<String>,
    ) {
        let now_ms = (now * 1000.0) as i32;
        match ev {
            NetEvent::Chat { text, .. } => self.chat.push(&loc.message(text), now_ms),
            NetEvent::ServerCommand(tokens) => {
                self.scoreboard.on_server_command(tokens);
                let text = || {
                    bind_keys(
                        &loc.message(tokens.get(1).map_or("", String::as_str)),
                        bound,
                    )
                };
                match tokens.first().map(String::as_str) {
                    Some("a") => self.weapon_selected(),
                    Some("m") => self
                        .complaint
                        .on_command(tokens.get(1).map_or("", String::as_str), now_ms),
                    // `CG_MapRestart` (events doc 9.8); 1.5 and UO send
                    // `B` for the restart that drops `pers` (votes doc 5).
                    Some("n" | "B") => self.on_map_restart(now),
                    Some("e" | "f") => self.game_messages.push(&text(), now_ms),
                    Some("c" | "g") => self.bold_messages.push(&text(), now_ms),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// A line for the game message window, as the cgame's own prints there
    /// (0x300207b0, which the `e` and `f` commands also reach).
    pub fn game_message(&mut self, text: &str, now_ms: i32) {
        self.game_messages.push(text, now_ms);
    }

    /// `cg_gameMessageWidth` and `cg_gameBoldMessageWidth`, read as the
    /// cgame's vmCvar integers each frame, for a window `vid_width` px wide
    /// ([`messages::line_width`]).
    pub fn set_message_widths(&mut self, game: i32, bold: i32, vid_width: u32) {
        self.game_messages.width = messages::line_width(game, vid_width);
        self.bold_messages.width = messages::line_width(bold, vid_width);
    }

    /// `cg_chatHeight` and `cg_chatTime` as this frame reads them, and
    /// whether the chat wraps as UO's does ([`chat::Rules`]).
    pub fn set_chat_rules(&mut self, rules: chat::Rules) {
        self.chat.set_rules(rules);
    }

    /// A weapon-select bind ran: the weapon name shows again.
    pub fn weapon_selected(&mut self) {
        self.player.weapon_name.select();
    }

    /// Whether a weapon-select bind is past `cg_weaponCycleDelay` ms from
    /// the weapon name's stamp, on the HUD's clock (snapshot server time).
    pub fn weapon_cycle_allowed(&self, delay_ms: i32) -> bool {
        self.player.weapon_name.cycle_allowed(delay_ms)
    }

    /// `CG_MapRestart`'s HUD words (0x3002ca60, events doc 9.8): the
    /// centre print, the complaint and the vote lines, the weapon strip's
    /// slot scales and clock, the hit icons and flash, a held scoreboard;
    /// and `cg.mapRestart`, so the next snapshot runs the respawn reset.
    /// The engine's own `n`/`B` case empties the message windows first
    /// (cod11-hud-protocol.md 1.1).
    fn on_map_restart(&mut self, now: f32) {
        self.game_messages.clear();
        self.bold_messages.clear();
        self.center_print.clear();
        self.complaint.clear();
        self.vote.on_map_restart();
        self.scoreboard.release(now);
        self.player.strip.reset();
        self.player.damage.clear(true);
        self.player.weapon_name.map_restart();
        self.spawn = None;
    }

    /// The newest snapshot's `(clientNum, stats[5])`. A new pair, or any
    /// pair after a restart, is the respawn reset (`0x30028a70`, combat doc
    /// 17.1): the hit icons and the gun's sway, tilt, movement and damage
    /// kick go back to zero.
    pub fn track_spawn(&mut self, spawn: (i32, i32)) {
        if self.spawn.replace(spawn) != Some(spawn) {
            self.player.damage.clear(false);
            self.player.gun.respawn();
        }
    }

    /// A new gamestate, or leaving a server: CS 7 indices and the
    /// scoreboard change. The chat ring goes once the cgame loads
    /// ([`Self::restart_cgame`]).
    pub fn on_gamestate(&mut self) {
        self.kill_icons.clear();
        // CL_InitCGame empties the message windows (cod11-hud-protocol.md 1.1).
        self.game_messages.clear();
        self.bold_messages.clear();
        self.center_print.clear();
        self.scoreboard.clear();
        self.player.strip.reset();
        self.timeout.on_gamestate();
        self.vote.on_gamestate();
        self.spawn = None;
    }

    /// A fresh cgame, on a map load or an in-game `vid_restart`: a
    /// gamestate's resets, and the chat ring, which lives in the cgame
    /// (cod11-chat.md 3.2) and goes with it (cod11-front-end.md section
    /// 17, "The cgame restart").
    pub fn restart_cgame(&mut self) {
        self.on_gamestate();
        self.chat = Chat::new();
    }

    /// `EV_OBITUARY` to a kill line in the game message window. Entity
    /// number == clientNum for players (docs/research/cod11-hud-protocol.md,
    /// sections 1 and 1.1). An unresolved victim drops the row and counts toward `unknown`; an unresolved attacker
    /// draws a victim-only row, as the stock client does for a world attacker.
    /// A kill by the drawn player also centre prints.
    pub fn on_game_event(&mut self, ev: &GameEvent, f: &HudFrame) {
        if ev.event != EV_OBITUARY {
            return;
        }
        let ob = killfeed::decode_obituary(ev);
        if let Some((text, y)) = obituary_center_print(&ob, f) {
            self.center_print.print(&text, y, (f.now * 1000.0) as i32);
        }

        let resolve = |num: u32| -> Option<(String, [f32; 3])> {
            let client = f.clients.get(&num)?;
            let team = client.field_i32(f.protocol, "team");
            let [r, g, b, _] = killfeed::team_color(team);
            Some((killfeed::line_name(&client.name(f.protocol)), [r, g, b]))
        };

        let Some(victim) = resolve(ob.victim) else {
            self.unknown += 1;
            return;
        };
        let attacker = ob.attacker.and_then(resolve);

        let kill_icon = (ob.weapon > 0)
            .then(|| self.weapon_kill_icon(f.fs, f.configstrings, ob.weapon))
            .flatten();
        let icon_wide = kill_icon.as_ref().is_some_and(|(_, wide)| *wide);
        let uo = f.protocol.is_uo();
        let icon = killfeed::icon_for(&ob, kill_icon.as_ref().map(|(path, _)| path.as_str()), uo);

        let line = messages::KillLine {
            attacker,
            icon,
            icon_wide,
            icon_flip: false,
            victim,
        };
        self.game_messages.push_kill(line, (f.now * 1000.0) as i32);
    }

    /// Steps the gun's aim once for this frame, as `0x300371f0` does before
    /// the 2D pass: the view weapon draws off the result and the scope
    /// overlay centres on it (combat doc 17.4). `gun_kick` is this frame's
    /// spring. Nothing steps, and no pose comes back, on a pm_type 4 or 5,
    /// dead (`cg.renderingThirdPerson`, `[0x30207158]`, set at `0x30033d39`
    /// for a snapshot pm_type of 6 or more) or on a mounted gun
    /// (`0x30037210`-`0x30037274`).
    /// The shellshock's time left at this frame's clock, which scales the
    /// view weapon's turn sway ([`crate::play::shock::sway_scale`]).
    pub fn set_shock_left(&mut self, ms: i32) {
        self.player.gun.shock_left_ms = ms;
    }

    pub fn step_gun(
        &mut self,
        p: &Protocol,
        ps: &PlayerState,
        predicted: Option<&Predicted>,
        server_time: i32,
        weapons: &[Option<WeaponDef>],
        gun_kick: [f32; 2],
    ) -> Option<GunPose> {
        self.player.gun_pose = None;
        if !steps_gun(ps.field_i32(p, "pm_type"), ps.field_i32(p, "eFlags")) {
            return None;
        }
        let (gun_ps, gun_ms) = gun_clock(p, ps, predicted, server_time, self.bob);
        let kick = self.player.damage.kick();
        let gun = &mut self.player.gun;
        gun.gun_kick = gun_kick;
        let def = weapons
            .get(usize::from(gun_ps.weapon))
            .and_then(Option::as_ref);
        self.player.gun_pose = gun.step(def, &gun_ps, gun_ms, kick);
        self.player.gun_pose
    }

    /// CS7 index to the weapon file's `(killIcon, wideKillIcon)`, cached hit
    /// or miss. Same 1-based convention as `entities::resolve_held_weapon`.
    fn weapon_kill_icon(
        &mut self,
        fs: &Pk3Fs,
        configstrings: &[String],
        weapon_index: i32,
    ) -> Option<(String, bool)> {
        if let Some(cached) = self.kill_icons.get(&weapon_index) {
            return cached.clone();
        }
        let list = configstrings.get(7).map(String::as_str).unwrap_or("");
        let weapons = crate::entities::split_weapon_list(list);
        let resolved = crate::entities::weapon_name_for_index(&weapons, weapon_index)
            .and_then(|name| vcod_common::weapon::load(fs, name).ok())
            .and_then(|def| {
                let wide = def.wide_kill_icon;
                def.kill_icon.map(|icon| (icon, wide))
            });
        self.kill_icons.insert(weapon_index, resolved.clone());
        resolved
    }

    /// This frame's quads from every element, at [`HUD_SCALE`].
    pub fn build(&mut self, f: &HudFrame) -> Vec<HudQuad> {
        font::set_team_colours(f.protocol.team_colour_codes, f.cvar);
        let mut out = Vec::new();
        let screen = (f.screen_w, f.screen_h);
        // vcod's: `vc_widescreen 0` keeps the whole HUD in the 4:3 box.
        let edges = (f.cvar)("vc_widescreen").is_none_or(|c| vcod_common::text::atoi_msvc(&c) != 0);
        let v = hudelem::Virtual::with_edges(screen, edges);
        let pm_type = |ps: &PlayerState| ps.field_i32(f.protocol, "pm_type");
        match f.ps.filter(|ps| draws_native_hud(pm_type(ps))) {
            Some(ps) => {
                let friends = compass_friends(ps, f);
                let mut view = player_view(ps, &friends, f);
                view.gun_angles = self.player.gun_pose.map(|g| g.angles);
                let cx = player::Context {
                    weapons: f.weapons,
                    configstrings: f.configstrings,
                    cs: &f.protocol.cs,
                    loc: f.localized,
                    font: &self.fonts.normal,
                    console_font: &self.fonts.console,
                    entity_origin: f.entity_origin,
                    bound_key: f.bound_key,
                    draw: f.draw,
                    scope: scope::Layout::of(f.protocol),
                };
                self.player.build(&view, &cx, f.server_time, &v, &mut out);
                // `Menu_PaintAll` paints the rest of hud.menu over the native items.
                if f.draw.status {
                    self.hud_menu
                        .build(&self.fonts, &v, f.localized, f.cvar, &mut out);
                }
            }
            None => self.player.hidden(),
        }
        if let Some(ps) = f.ps.filter(|_| f.draw.status) {
            let elems: Vec<HudElem> = ps
                .arrays
                .hud_archived
                .iter()
                .chain(&ps.arrays.hud_current)
                .copied()
                .collect();
            hudelem::build(
                &elems,
                f.configstrings,
                &f.protocol.cs,
                f.localized,
                f.bound_key,
                &self.fonts,
                f.server_time,
                &v,
                &mut out,
            );
        }
        let now_ms = (f.now * 1000.0) as i32;
        let clocked = out.len();
        // `CG_Draw2D` returns for intermission (pm_type 5) before both.
        // 1.1 sends the vote's end as server time, 1.5 and UO the ms left.
        let relative = !f.protocol.is_v1();
        self.vote
            .update(f.configstrings, f.protocol.cs.vote, relative, now_ms);
        self.timeout
            .update(f.configstrings, f.protocol.cs.match_timeout, now_ms);
        if f.ps.is_none_or(|ps| pm_type(ps) != 5) {
            let voted =
                f.ps.is_some_and(|ps| ps.field_i32(f.protocol, "eFlags") & vote::EF_VOTED != 0);
            let clock = if relative { now_ms } else { f.server_time };
            let name = |n: i32| {
                let c = f.clients.get(&u32::try_from(n).ok()?)?;
                Some(c.name(f.protocol))
            };
            let complaint = self.complaint.build(
                &self.fonts,
                f.localized,
                now_ms,
                &name,
                f.bound_key,
                &v.left(),
                &mut out,
            );
            self.vote_sound |= !complaint
                && self.vote.build(
                    &self.fonts,
                    f.localized,
                    clock,
                    voted,
                    f.bound_key,
                    &v.left(),
                    &mut out,
                );
            self.timeout
                .build(&self.fonts, f.localized, now_ms, &v, &mut out);
        }
        // The viewer's own team colours the chat strip and names.
        let team = f.ps.map_or(0, |ps| {
            let own = ps.field_i32(f.protocol, "clientNum") as u32;
            f.clients
                .get(&own)
                .map_or(0, |c| c.field_i32(f.protocol, "team"))
        });
        let [r, g, b, _] = killfeed::team_color(team);
        self.chat
            .build(&self.fonts, [r, g, b], now_ms, &v.left(), &mut out);
        // The board goes over everything; while it is up the cgame skips
        // the game and bold message windows (0x30018810) and the centre
        // print (0x300188e4).
        let mut board = Vec::new();
        let board_up = self.build_scoreboard(f, &v, &mut board);
        if !board_up {
            if f.ps.is_none_or(|ps| pm_type(ps) != 5) {
                self.center_print.build(&self.fonts, now_ms, &v, &mut out);
            }
            self.game_messages
                .build(&self.fonts, now_ms, &v.left(), &mut out);
            self.bold_messages.build(&self.fonts, now_ms, &v, &mut out);
        }
        crate::frame_digest::hud_clocked(clocked..out.len());
        if let Some(field) = &self.chat_field {
            chat::field(field, &self.fonts, f.localized, &v.left(), &mut out);
        }
        // The scoreboard reuses the parsed gametype; its last `b` reply
        // overrides CS 5/6 (status.rs).
        let status = status::read_status(
            f.configstrings,
            &f.protocol.cs,
            f.server_time,
            self.scoreboard.totals(),
        );
        // vcod's own header, which retail does not draw: spectators only.
        if !f.local_player {
            status::build(&status, &self.fonts.big, f.screen_w, &mut out);
        }
        out.extend(board);
        out
    }
}

impl Hud {
    /// The scoreboard while `+scores` is held, fading or in intermission.
    /// 1.5 and UO draw none while `timescale` is 0, a match timeout
    /// (1.5 cgame 0x3002cae0, UO 0x30037d90). True when drawn.
    fn build_scoreboard(&mut self, f: &HudFrame, v: &Virtual, out: &mut Vec<HudQuad>) -> bool {
        self.scoreboard.intermission =
            f.ps.is_some_and(|ps| ps.field_i32(f.protocol, "pm_type") == 5);
        let paused = !f.protocol.is_v1()
            && (f.cvar)("timescale").is_some_and(|t| vcod_common::text::atoi_msvc(&t) == 0);
        if paused {
            return false;
        }
        let info = |key: &str| {
            f.configstrings
                .get(status::CS_SERVERINFO)
                .and_then(|si| vcod_common::net::info_value_for_key(si, key))
                .unwrap_or("")
        };
        // The game type's own name (its `.txt`), else what `g_gametype` says.
        let gametype = info("g_gametype");
        let gametype = self
            .game_types
            .iter()
            .find(|g| g.script.eq_ignore_ascii_case(gametype))
            .map_or(gametype, |g| g.name.as_str());
        let client = |n: u32| -> Option<(String, i32)> {
            let cs = f.clients.get(&n)?;
            Some((cs.name(f.protocol), cs.field_i32(f.protocol, "team")))
        };
        let status_icon = |n: i32| {
            let slot = f.protocol.cs.slot(CsRange::StatusIcon, n)?;
            f.configstrings.get(slot).filter(|s| !s.is_empty()).cloned()
        };
        let view = scoreboard::BoardView {
            now: f.now,
            own_client: f.own_client,
            client: &client,
            cvar: f.cvar,
            bound_key: f.bound_key,
            loc: f.localized,
            gametype,
            map: info("mapname"),
            hostname: info("sv_hostname"),
            address: f.server_address,
            status_icon: &status_icon,
        };
        self.scoreboard.build(&view, &self.fonts, v, out)
    }
}

/// `CG_Obituary`'s centre print (0x3001d97f): the localized line and its y
/// when the drawn playerstate's client killed someone else. UO skips it
/// while following (docs/research/cod11-hud-protocol.md, "The centre print").
fn obituary_center_print(ob: &killfeed::Obituary, f: &HudFrame) -> Option<(String, i32)> {
    let ps = f.ps?;
    let attacker = ob.attacker?;
    if attacker == ob.victim || ps.field_i32(f.protocol, "clientNum") as u32 != attacker {
        return None;
    }
    let uo = f.protocol.is_uo();
    if uo && ps.field_i32(f.protocol, "pm_flags") & PMF_FOLLOW != 0 {
        return None;
    }
    let victim = f.clients.get(&ob.victim)?;
    let team = |c: &ClientState| c.field_i32(f.protocol, "team");
    let killer_team = team(f.clients.get(&attacker)?);
    let teammate = killer_team != 0 && killer_team == team(victim);
    let msg = centerprint::obituary_message(&victim.name(f.protocol), teammate);
    let y = if uo {
        centerprint::OBITUARY_Y_UO
    } else {
        centerprint::OBITUARY_Y
    };
    Some((bind_keys(&f.localized.message(&msg), f.bound_key), y))
}

/// A line being typed after `messagemode` (`say`) or `messagemode2`
/// (`say_team`).
#[derive(Default)]
pub struct ChatField {
    pub team: bool,
    pub text: String,
}

/// The cgame's `[{command}]` pass over a localized message (1.1
/// 0x300229b0, 1.5 and UO the same shape): the first `[{` with a `}]`
/// after it has the command between them replaced by its key text
/// (`bound`, 0x30046940), the brackets kept. An unbound command, a later
/// `[{`, or a result past 1023 bytes leaves the text as written.
pub fn bind_keys(text: &str, bound: &dyn Fn(&str) -> Option<String>) -> String {
    let b = text.as_bytes();
    // The scan stops with fewer than five bytes left from the `[`.
    let Some(open) = (0..b.len().saturating_sub(4)).find(|&i| b[i] == b'[' && b[i + 1] == b'{')
    else {
        return text.to_string();
    };
    let Some(close) = (open..b.len() - 1).find(|&j| b[j] == b'}' && b[j + 1] == b']') else {
        return text.to_string();
    };
    let Some(keys) = text.get(open + 2..close).and_then(bound) else {
        return text.to_string();
    };
    let out = format!("{}{keys}{}", &text[..=open], &text[close + 1..]);
    if out.len() > 1023 {
        log::info!("String too long to add key binding: {text}");
        return text.to_string();
    }
    out
}

/// The cgame's bind table (1.1 0x3006f150, 1.5 0x30071150, UO
/// 0x3008adb8, 24-byte rows): the only commands its key lookup
/// (0x30046860) names. Anything else reads unbound.
pub fn bind_commands(p: &Protocol) -> &'static [&'static str] {
    match p.transport.version {
        6 => BIND_COMMANDS_15,
        22 => BIND_COMMANDS_UO,
        _ => BIND_COMMANDS_11,
    }
}

#[rustfmt::skip]
const BIND_COMMANDS_11: &[&str] = &[
    "+scores", "+speed", "+forward", "+back", "+moveleft", "+moveright", "+moveup",
    "+movedown", "+left", "+right", "+strafe", "+lookup", "+lookdown", "+mlook",
    "centerview", "+attack", "weapprev", "weapnext", "weapalt", "scoresUp", "scoresDown",
    "messagemode", "messagemode2", "messagemode3", "messagemode4", "+activate", "+reload",
    "help", "+leanleft", "+leanright", "vote yes", "vote no", "mp_QuickMessage",
    "weaponslot primary", "weaponslot primaryb", "weaponslot pistol", "weaponslot grenade",
    "weaponslot smokegrenade", "+melee", "+prone", "lowerstance", "raisestance",
    "togglecrouch", "toggleprone", "goprone", "gocrouch", "+gostand", "toggle cl_run",
    "screenshot", "screenshotJPEG",
];

/// 1.5's: `messagesquad` where 1.1 has `messagemode4`.
#[rustfmt::skip]
const BIND_COMMANDS_15: &[&str] = &[
    "+scores", "+speed", "+forward", "+back", "+moveleft", "+moveright", "+moveup",
    "+movedown", "+left", "+right", "+strafe", "+lookup", "+lookdown", "+mlook",
    "centerview", "+attack", "weapprev", "weapnext", "weapalt", "scoresUp", "scoresDown",
    "messagemode", "messagemode2", "messagemode3", "messagesquad", "+activate", "+reload",
    "help", "+leanleft", "+leanright", "vote yes", "vote no", "mp_QuickMessage",
    "weaponslot primary", "weaponslot primaryb", "weaponslot pistol", "weaponslot grenade",
    "weaponslot smokegrenade", "+melee", "+prone", "lowerstance", "raisestance",
    "togglecrouch", "toggleprone", "goprone", "gocrouch", "+gostand", "toggle cl_run",
    "screenshot", "screenshotJPEG",
];

#[rustfmt::skip]
const BIND_COMMANDS_UO: &[&str] = &[
    "+scores", "+speed", "+forward", "+back", "+moveleft", "+moveright", "+moveup",
    "+movedown", "+left", "+right", "+strafe", "+lookup", "+lookdown", "+mlook",
    "centerview", "+attack", "weapprev", "weapnext", "weapalt", "scoresUp", "scoresDown",
    "messagemode", "messagemode2", "messagemode3", "messagemode4", "+activate", "+reload",
    "help", "+leanleft", "+leanright", "vote yes", "vote no", "mp_QuickMessage",
    "mp_quickmap", "mp_purchase", "weaponslot primary", "weaponslot primaryb",
    "weaponslot pistol", "weaponslot grenade", "weaponslot smokegrenade", "weaponslot satchel",
    "weaponslot binocular", "+melee", "+prone", "lowerstance", "raisestance", "togglecrouch",
    "toggleprone", "goprone", "gocrouch", "+gostand", "toggle cl_run", "+sprint", "screenshot",
    "screenshotJPEG",
];

/// `pm_type` 4 is a free-flying spectator and 5 the intermission; any other,
/// a follower's copy of its target included, draws the menu HUD.
fn draws_native_hud(pm_type: i32) -> bool {
    !matches!(pm_type, 4 | 5)
}

/// `0x300371f0`'s skip gates on the drawn playerstate: spectator or
/// intermission, dead (`cg_thirdPerson`, which vcod lacks, takes the same
/// gate), or a mounted gun. `[0x3020cad4]`, the other gate, is never
/// written (combat doc 17.4).
fn steps_gun(pm_type: i32, e_flags: i32) -> bool {
    draws_native_hud(pm_type) && pm_type < PM_DEAD && e_flags & 0xc000 == 0
}

/// The playerstate the gun's aim and the damage kick run on, and its clock:
/// the replay at its own clock, or a followed player's snapshot at the
/// render clock. Retail runs both on whichever playerstate it draws.
fn gun_clock(
    p: &Protocol,
    ps: &PlayerState,
    predicted: Option<&Predicted>,
    server_time: i32,
    bob: vcod_common::pmove::BobCvars,
) -> (vcod_common::pmove::PlayerState, i32) {
    let (mut gun_ps, ms) = match predicted {
        Some(pred) => (pred.ps, pred.command_time),
        None => (
            vcod_common::pmove::predict::from_wire(p, ps, None).ps,
            server_time,
        ),
    };
    gun_ps.bob = bob;
    (gun_ps, ms)
}

impl Hud {
    /// `CG_TransitionPlayerState`'s damage half (`0x30028d2c` after the
    /// prediction, `0x3002fc5e` at a followed snapshot's transition): a
    /// new `damageEvent` on `ps`, the newest snapshot's, kicks, flashes and
    /// adds a hit icon. Runs every frame ahead of the view, mounted, dead
    /// or HUD hidden alike; the kick splits along last frame's
    /// [`Self::drawn_view`] (the refdef's axis), the playerstate's view
    /// before the first. `predicted` is the replay when it is ours.
    pub fn feed_damage(
        &mut self,
        p: &Protocol,
        ps: &PlayerState,
        predicted: Option<&Predicted>,
        server_time: i32,
    ) {
        let view = self.drawn_view.unwrap_or_else(|| ps.viewangles(p));
        let kick_ms = predicted.map_or(server_time, |pred| pred.command_time);
        let fb = DamageFeedback::from_ps(p, ps);
        let _ = self.player.damage.feed(fb, view, server_time, kick_ms);
    }

    /// The damage kick and the sight's walk bob on the drawn view, wire
    /// degrees: `CG_OffsetFirstPersonView`'s first adds (cgame
    /// `0x30032b58`-`0x30032b9c`, combat doc 15.5). `ps` is the newest
    /// snapshot's, `predicted` the replay when it is ours; the kick is
    /// [`Self::feed_damage`]'s.
    pub fn view_kick(
        &mut self,
        p: &Protocol,
        ps: &PlayerState,
        predicted: Option<&Predicted>,
        server_time: i32,
        weapons: &[Option<WeaponDef>],
    ) -> [f32; 3] {
        if !draws_native_hud(ps.field_i32(p, "pm_type")) {
            return [0.0; 3];
        }
        let (gun_ps, gun_ms) = gun_clock(p, ps, predicted, server_time, self.bob);
        let kick = self.player.damage.kick();
        let def = weapons
            .get(usize::from(gun_ps.weapon))
            .and_then(Option::as_ref);
        let aim = vcod_common::pmove::aim::view_angles;
        let mut out = aim(&gun_ps, def, &kick, gun_ms);
        // 1.5's sight bob (0x30013cd0) skips a mounted gun; 1.1's does not.
        if p.transport.version == 6 && ps.field_i32(p, "eFlags") & 0xc000 != 0 {
            let bob = aim(&gun_ps, def, &Default::default(), gun_ms);
            for (o, b) in out.iter_mut().zip(bob) {
                *o -= b;
            }
        }
        out
    }

    /// The walk bob `CG_OffsetFirstPersonView` adds to the eye after the
    /// kick (`aim::eye_bob`), on the same playerstate the kick reads; none on
    /// a mounted gun (cgame 0x30032bb4). The caller skips intermission.
    /// `view` is the kicked view, wire degrees.
    pub fn eye_bob(
        &self,
        p: &Protocol,
        ps: &PlayerState,
        predicted: Option<&Predicted>,
        server_time: i32,
        view: [f32; 3],
    ) -> glam::Vec3 {
        if ps.field_i32(p, "eFlags") & 0xc000 != 0 {
            return glam::Vec3::ZERO;
        }
        let (gun_ps, _) = gun_clock(p, ps, predicted, server_time, self.bob);
        vcod_common::pmove::aim::eye_bob(&gun_ps, view)
    }
}

/// The teammates of `ps`'s client the snapshot shows, live `ET_PLAYER`
/// entities on its team, and the one `iCompassFriendInfo` names. None while
/// that client is a spectator or on no team.
fn compass_friends(ps: &PlayerState, f: &HudFrame) -> Vec<Sighting> {
    let p = f.protocol;
    let team = |num: u32| f.clients.get(&num).map(|c| c.field_i32(p, "team"));
    let own = ps.field_i32(p, "clientNum") as u32;
    let Some(our_team) = team(own).filter(|t| !matches!(t, 0 | 3)) else {
        return Vec::new();
    };
    let mut out: Vec<Sighting> = f
        .entities
        .iter()
        .filter(|&(&num, ent)| {
            num < 64
                && ent.field_i32(p, "eType") == crate::entities::ET_PLAYER
                && ent.field_i32(p, "eFlags") & 1 == 0
                && team(num) == Some(our_team)
        })
        .map(|(&num, ent)| {
            let origin = (f.entity_origin)(num as i32).unwrap_or_else(|| ent.origin(p));
            Sighting {
                client: num,
                at: friends::Mark::At([origin[0], origin[1]]),
                yaw: ent.angles(p)[1],
                pinged: ent.field_i32(p, "eFlags") & friends::EF_PING != 0,
            }
        })
        .collect();
    let pinged = ps.field_i32(p, "eFlags") & friends::PS_EF_FRIEND_PING != 0;
    let info = ps.field_i32(p, "iCompassFriendInfo");
    out.extend(friends::decode_friend_info(info, ps.origin(p), pinged));
    out
}

/// The native HUD's inputs off `ps`, with the replay's fields in place of
/// the snapshot's where `f.predicted` has them.
fn player_view<'a>(
    ps: &'a PlayerState,
    friends: &'a [Sighting],
    f: &HudFrame<'a>,
) -> PlayerView<'a> {
    let int = |name: &str| ps.field_i32(f.protocol, name);
    let mut eflags = int("eFlags");
    let (weapon, ammo, ammoclip, aim_spread_scale, spread_stance, ads_frac) = match f.predicted {
        Some(pred) => {
            let s = &pred.ps;
            eflags &= !(EF_CROUCH | EF_PRONE);
            eflags |= match s.stance {
                Stance::Stand => 0,
                Stance::Crouch => EF_CROUCH,
                Stance::Prone => EF_PRONE,
            };
            (
                usize::from(s.weapon),
                &s.ammo,
                &s.ammoclip,
                s.aim_spread_scale,
                SpreadStance::of(s, pred.command_time, f.snap_time),
                s.weapon_pos_frac,
            )
        }
        None => {
            let pm_flags = int("pm_flags");
            let lerp_time = int("viewHeightLerpTime");
            let stance = SpreadStance {
                prone: pm_flags & 0x1 != 0,
                ducked: pm_flags & 0x2 != 0,
                view_height: ps.field_f32(f.protocol, "viewHeightCurrent"),
                // A signed byte on the wire, which reads back unsigned.
                lerp_target: f32::from(int("viewHeightLerpTarget") as u8 as i8),
                lerp_down: int("viewHeightLerpDown") != 0,
                dive: pm_flags & 0x4 != 0,
                into_leg_ms: (lerp_time != 0).then(|| f.snap_time - lerp_time),
            };
            (
                int("weapon") as usize,
                &ps.arrays.ammo,
                &ps.arrays.ammoclip,
                ps.field_f32(f.protocol, "aimSpreadScale"),
                stance,
                ps.field_f32(f.protocol, "fWeaponPosFrac"),
            )
        }
    };
    let held = match f.predicted {
        Some(pred) => pred.ps.weapons_held,
        None => u64::from(int("weapons[0]") as u32) | u64::from(int("weapons[1]") as u32) << 32,
    };
    // The name and the mode icon read the selection while it is carried
    // and below the weapon count (0x30023c30, 0x30023e90); the count reads
    // it from our own view whenever it is below the weapon count, carried
    // or not, and `cent->currentState.weapon` otherwise (0x30025ab0). The
    // count is the highest weapon number, so the last one never qualifies.
    let select = usize::from(f.weapon_select);
    let count = f.weapons.len().saturating_sub(1);
    let name_weapon = if select < count.min(64) && held & 1 << select != 0 {
        select
    } else {
        weapon
    };
    let own_view = int("pm_flags") & vcod_common::net::flags::PMF_OWN_VIEW != 0;
    let ammo_weapon = if own_view && select < count {
        select
    } else {
        weapon
    };
    PlayerView {
        client_num: int("clientNum"),
        health: ps.health(),
        max_health: ps.max_health(),
        eflags,
        weapon: f.weapons.get(weapon).and_then(Option::as_ref),
        weapon_index: weapon,
        name_weapon: f.weapons.get(name_weapon).and_then(Option::as_ref),
        ammo_weapon: f.weapons.get(ammo_weapon).and_then(Option::as_ref),
        turret: f.turret_weapon.and_then(|w| f.weapons.get(w)?.as_ref()),
        ammo,
        ammoclip,
        aim_spread_scale,
        spread_stance,
        ads_frac,
        view_yaw: f.view_yaw,
        eye: f.eye,
        fov: (
            f.fov,
            crate::camera::fov_y(f.fov, f.screen_w / f.screen_h.max(1.0)),
        ),
        gun_angles: None,
        objectives: &ps.arrays.objectives,
        friends,
        alive: int("pm_type") < 4,
        cursor_hint: int("serverCursorHint"),
        // Playerstate fields arrive unsigned; retail's -1 is 255.
        cursor_hint_string: i32::from(int("serverCursorHintString") as u8 as i8),
        spawn_count: ps.arrays.stats[5],
        // The cgame reads it off the playerstate it predicts.
        prone_blocked: f
            .predicted
            .map_or(int("pm_flags") & PMF_PRONE_BLOCKED != 0, |pred| {
                pred.ps.prone_blocked
            }),
        selected: f.weapon_select,
        slots: match f.predicted {
            Some(pred) => pred.ps.weapon_slots,
            None => {
                let mut slots = [0u8; 8];
                slots[..4].copy_from_slice(&int("weaponslots[0]").to_le_bytes());
                slots[4..].copy_from_slice(&int("weaponslots[4]").to_le_bytes());
                slots
            }
        },
        held,
        uo: f.protocol.is_uo().then(|| uo_view(ps, eflags, f)),
    }
}

/// United Offensive's playerstate and vehicle reads for the native HUD
/// (docs/research/coduo-diff.md section 15).
fn uo_view(ps: &PlayerState, eflags: i32, f: &HudFrame) -> uo::UoView {
    use vcod_common::net::flags::{
        EF_IN_VEHICLE_V22, EF_POPPED_OUT_V22, ET_VEHICLE_V22, PMF_SPRINT_V22,
    };
    let p = f.protocol;
    let int = |name: &str| ps.field_i32(p, name);
    let seat = (eflags & EF_IN_VEHICLE_V22 != 0 && eflags & EF_POPPED_OUT_V22 == 0)
        .then(|| {
            let num = u32::try_from(int("viewlocked_entNum")).ok()?;
            let ent = f
                .entities
                .get(&num)
                .filter(|e| e.field_i32(p, "eType") == ET_VEHICLE_V22)?;
            Some(uo::Seat {
                pos: int("vehPos"),
                kind: int("vehType"),
                vehicle_kind: ent.field_i32(p, "surfType"),
                body_yaw: ent.angles(p)[1],
                turret_yaw: f.vehicle_turret_yaw,
                seats_taken: ent.field_i32(p, "animMovetype"),
            })
        })
        .flatten();
    uo::UoView {
        sprinting: int("pm_flags") & PMF_SPRINT_V22 != 0,
        fatigue: ps.field_f32(p, "fatigueScale"),
        seat,
        vehicles: compass_vehicles(ps, f),
    }
}

/// `CG_PLAYER_COMPASS_TANKS`' feed (uo cgame 0x30016c00): every vehicle a
/// teammate drives, by its slot in the game's vehicle pool
/// (`attackerEntityNum`), and the one `iCompassTankInfo` packs. Nothing
/// for a spectator or a client on no team.
fn compass_vehicles(ps: &PlayerState, f: &HudFrame) -> Vec<uo::VehicleSighting> {
    use vcod_common::net::flags::ET_VEHICLE_V22;
    let p = f.protocol;
    let team = |num: i32| {
        f.clients
            .get(&u32::try_from(num).ok()?)
            .map(|c| c.field_i32(p, "team"))
    };
    let own = ps.field_i32(p, "clientNum");
    let Some(our_team) = team(own).filter(|t| !matches!(t, 0 | 3)) else {
        return Vec::new();
    };
    let mut out: Vec<_> = f
        .entities
        .iter()
        .filter_map(|(&num, ent)| {
            let int = |name: &str| ent.field_i32(p, name);
            let driver = int("otherEntityNum");
            let slot = u32::try_from(int("attackerEntityNum")).ok()?;
            (int("eType") == ET_VEHICLE_V22
                && driver != 0x3ff
                && driver != own
                && team(driver) == Some(our_team))
            .then_some(())?;
            let origin = (f.entity_origin)(num as i32).unwrap_or_else(|| ent.origin(p));
            let kind = int("surfType");
            // A flak gun (type 5) points its turret.
            let yaw = match kind {
                5 => ent.field_f32(p, "angles2[1]"),
                _ => ent.angles(p)[1],
            };
            Some(uo::VehicleSighting {
                slot,
                at: friends::Mark::At([origin[0], origin[1]]),
                yaw,
                kind: Some(kind),
            })
        })
        .collect();
    out.extend(uo::decode_tank_info(
        ps.field_i32(p, "iCompassTankInfo"),
        ps.origin(p),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::PROTOCOL_V22;

    /// 0x300229b0: the first `[{command}]` takes the command's key text;
    /// an unbound command, an unclosed one or a later one stays as written.
    #[test]
    fn bind_keys_fills_the_first_bound_command() {
        let bound = |c: &str| match c {
            "+activate" => Some("F".to_string()),
            "vote yes" => Some("F1 or Y".to_string()),
            _ => None,
        };
        assert_eq!(
            bind_keys("Press [{+activate}] to plant", &bound),
            "Press [F] to plant"
        );
        assert_eq!(bind_keys("[{vote yes}] Yes", &bound), "[F1 or Y] Yes");
        assert_eq!(
            bind_keys("[{+activate}] and [{+activate}]", &bound),
            "[F] and [{+activate}]",
            "only the first"
        );
        assert_eq!(
            bind_keys("[{+melee}] then [{+activate}]", &bound),
            "[{+melee}] then [{+activate}]",
            "an unbound first one ends the pass"
        );
        assert_eq!(bind_keys("[{+activate", &bound), "[{+activate");
        assert_eq!(bind_keys("x[{}]", &bound), "x[{}]");
    }

    /// Each cgame's bind table, as many rows as its loop bound.
    #[test]
    fn the_bind_tables_hold_retails_rows() {
        use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6};
        assert_eq!(bind_commands(&PROTOCOL_V1).len(), 50);
        assert_eq!(bind_commands(&PROTOCOL_V6).len(), 50);
        assert_eq!(bind_commands(&PROTOCOL_V22).len(), 55);
        assert!(bind_commands(&PROTOCOL_V22).contains(&"+sprint"));
        assert!(!bind_commands(&PROTOCOL_V1).contains(&"+sprint"));
    }
    use vcod_common::net::msg::hud_field as msg_field;
    use vcod_common::net::protocol::PROTOCOL_V1;

    static NO_ENTITIES: BTreeMap<u32, EntityState> = BTreeMap::new();

    #[test]
    fn the_gun_holds_still_dead_spectating_or_mounted() {
        assert!(steps_gun(0, 0) && steps_gun(1, 0));
        for pm_type in [4, 5, 6, 7] {
            assert!(!steps_gun(pm_type, 0), "pm_type {pm_type}");
        }
        assert!(!steps_gun(0, 0x4000) && !steps_gun(0, 0x8000));
    }

    fn frame<'a>(
        ps: &'a PlayerState,
        predicted: Option<&'a Predicted>,
        fs: &'a Pk3Fs,
        loc: &'a Localized,
        clients: &'a BTreeMap<u32, ClientState>,
    ) -> HudFrame<'a> {
        HudFrame {
            now: 0.0,
            screen_w: 640.0,
            screen_h: 480.0,
            configstrings: &[],
            clients,
            protocol: &PROTOCOL_V1,
            server_time: 0,
            snap_time: 0,
            fs,
            ps: Some(ps),
            entities: &NO_ENTITIES,
            predicted,
            local_player: true,
            weapons: &[],
            localized: loc,
            view_yaw: 0.0,
            eye: [0.0; 3],
            fov: 80.0,
            entity_origin: &|_| None,
            turret_weapon: None,
            vehicle_turret_yaw: None,
            cvar: &|_| None,
            bound_key: &|_| None,
            draw: DrawToggles::default(),
            weapon_select: 0,
            own_client: None,
            server_address: None,
        }
    }

    /// The weapon name follows `cg.weaponSelect` while the playerstate
    /// holds it, and the playerstate's weapon otherwise (0x30023c30).
    #[test]
    fn the_weapon_name_shows_the_held_selection() {
        let p = &PROTOCOL_V1;
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            ps.fields[PlayerState::field_index(p, name).expect(name)] = v;
        };
        set("weapon", 1);
        set("weapons[0]", 0b10110);
        let def = |name: &str| {
            Some(WeaponDef {
                display_name: name.into(),
                ..Default::default()
            })
        };
        let weapons = [None, def("rifle"), def("pistol"), def("grenade"), def("mg")];
        let (fs, loc, clients) = (Pk3Fs::empty(), Localized::default(), BTreeMap::new());
        let named = |select: u8| {
            let f = HudFrame {
                weapons: &weapons,
                weapon_select: select,
                ..frame(&ps, None, &fs, &loc, &clients)
            };
            let v = player_view(&ps, &[], &f);
            assert_eq!(v.weapon.map(|d| d.display_name.as_str()), Some("rifle"));
            v.name_weapon.map(|d| d.display_name.clone())
        };
        assert_eq!(named(0).as_deref(), Some("rifle"));
        assert_eq!(named(2).as_deref(), Some("pistol"));
        assert_eq!(named(3).as_deref(), Some("rifle"), "not held");
        assert_eq!(named(4).as_deref(), Some("rifle"), "the last weapon");
    }

    /// The ammo count reads the selection from our own view, carried or
    /// not, while it is below the weapon count, and the playerstate's
    /// weapon otherwise (0x30025ab0).
    #[test]
    fn the_ammo_count_reads_the_selection_from_our_own_view() {
        let p = &PROTOCOL_V1;
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            ps.fields[PlayerState::field_index(p, name).expect(name)] = v;
        };
        set("weapon", 1);
        set("weapons[0]", 0b10);
        let def = |name: &str| {
            Some(WeaponDef {
                display_name: name.into(),
                ..Default::default()
            })
        };
        let weapons = [None, def("rifle"), def("pistol"), def("grenade")];
        let (fs, loc, clients) = (Pk3Fs::empty(), Localized::default(), BTreeMap::new());
        let counted = |ps: &PlayerState, select: u8| {
            let f = HudFrame {
                weapons: &weapons,
                weapon_select: select,
                ..frame(ps, None, &fs, &loc, &clients)
            };
            let v = player_view(ps, &[], &f);
            v.ammo_weapon.map(|d| d.display_name.clone())
        };
        assert_eq!(counted(&ps, 2).as_deref(), Some("rifle"), "a follower");
        let mut own = ps.clone();
        own.fields[PlayerState::field_index(p, "pm_flags").unwrap()] =
            vcod_common::net::flags::PMF_OWN_VIEW;
        assert_eq!(counted(&own, 2).as_deref(), Some("pistol"), "not carried");
        assert_eq!(
            counted(&own, 3).as_deref(),
            Some("rifle"),
            "the last weapon"
        );
        assert_eq!(counted(&own, 0), None);
    }

    #[test]
    fn the_replay_stands_in_for_the_snapshot_weapon_fields() {
        let p = &PROTOCOL_V1;
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            ps.fields[PlayerState::field_index(p, name).expect(name)] = v;
        };
        set("eFlags", 0x10);
        set("aimSpreadScale", 200f32.to_bits() as i32);
        set("serverCursorHintString", 255);
        set("pm_flags", 0x5);
        set("viewHeightCurrent", 30f32.to_bits() as i32);
        set("viewHeightLerpTarget", 11);
        set("viewHeightLerpDown", 1);
        set("viewHeightLerpTime", 1000);
        ps.arrays.ammoclip[10] = 5;
        let mut pred = Predicted {
            ps: vcod_common::pmove::PlayerState::spawn(glam::Vec3::ZERO, 0.0),
            pm_type: 0,
            delta_angles: [0; 3],
            command_time: 0,
            view_lerp_start: 0,
            ring: Default::default(),
            respawned: false,
        };
        pred.ps.ammoclip[10] = 4;
        pred.ps.aim_spread_scale = 50.0;
        pred.ps.stance = Stance::Crouch;
        let (fs, loc, clients) = (Pk3Fs::empty(), Localized::default(), BTreeMap::new());

        let snap = player_view(
            &ps,
            &[],
            &HudFrame {
                snap_time: 1100,
                ..frame(&ps, None, &fs, &loc, &clients)
            },
        );
        assert_eq!(
            snap.spread_stance,
            SpreadStance {
                prone: true,
                ducked: false,
                view_height: 30.0,
                lerp_target: 11.0,
                lerp_down: true,
                dive: true,
                into_leg_ms: Some(100),
            }
        );
        assert_eq!(
            (snap.ammoclip[10], snap.aim_spread_scale, snap.eflags),
            (5, 200.0, 0x10)
        );
        assert_eq!(snap.cursor_hint_string, -1, "retail's -1 arrives as 255");
        assert_eq!(snap.fov, (80.0, crate::camera::fov_y(80.0, 640.0 / 480.0)));

        let own = player_view(&ps, &[], &frame(&ps, Some(&pred), &fs, &loc, &clients));
        assert_eq!(
            (own.ammoclip[10], own.aim_spread_scale, own.eflags),
            (4, 50.0, 0x10 | EF_CROUCH)
        );
    }

    #[test]
    fn the_native_hud_draws_for_any_player_view_and_hudelems_always() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = Hud::new(&fs).expect("hud");
        let mut ps = PlayerState::null(&PROTOCOL_V1);
        let mut bar = HudElem::default();
        bar.set(msg_field::TYPE, 3);
        bar.set(msg_field::SHADER, 1);
        bar.set(msg_field::WIDTH, 10);
        bar.set(msg_field::HEIGHT, 10);
        bar.set(msg_field::COLOR, -1);
        ps.arrays.hud_current.push(bar);
        let mut cs = vec![String::new(); hudelem::CS_SHADERS + 2];
        cs[hudelem::CS_SHADERS + 1] = "white".into();
        let (loc, clients) = (Localized::default(), BTreeMap::new());
        let pm_type = PlayerState::field_index(&PROTOCOL_V1, "pm_type").expect("pm_type");
        let mut drawn = |pm: i32, local_player: bool| -> Vec<String> {
            ps.fields[pm_type] = pm;
            let f = HudFrame {
                configstrings: &cs,
                local_player,
                ..frame(&ps, None, &fs, &loc, &clients)
            };
            hud.build(&f).into_iter().map(|q| q.texture).collect()
        };
        let native = |t: &[String]| t.iter().any(|t| t.contains("health_back"));
        let header = hud_header_page(&fs);

        // A free-flying spectator, and the intermission: hudelems only.
        for pm in [4, 5] {
            let t = drawn(pm, false);
            assert!(
                t.iter().any(|t| t == "white") && !native(&t),
                "pm_type {pm}"
            );
        }
        // Following: the target's HUD, and vcod's header.
        let following = drawn(0, false);
        assert!(native(&following) && following.contains(&header));
        // Our own, alive and dead.
        let own = drawn(0, true);
        assert!(native(&own) && !own.contains(&header));
        assert!(native(&drawn(6, false)), "dead");
    }

    #[test]
    fn draw_status_off_hides_the_menu_hud_and_hudelems() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = Hud::new(&fs).expect("hud");
        let mut ps = PlayerState::null(&PROTOCOL_V1);
        let mut bar = HudElem::default();
        bar.set(msg_field::TYPE, 3);
        bar.set(msg_field::SHADER, 1);
        bar.set(msg_field::WIDTH, 10);
        bar.set(msg_field::HEIGHT, 10);
        bar.set(msg_field::COLOR, -1);
        ps.arrays.hud_current.push(bar);
        let mut cs = vec![String::new(); hudelem::CS_SHADERS + 2];
        cs[hudelem::CS_SHADERS + 1] = "white".into();
        let (loc, clients) = (Localized::default(), BTreeMap::new());
        let mut drawn = |status: bool| -> Vec<String> {
            let f = HudFrame {
                configstrings: &cs,
                local_player: true,
                draw: DrawToggles {
                    crosshair: true,
                    status,
                },
                ..frame(&ps, None, &fs, &loc, &clients)
            };
            hud.build(&f).into_iter().map(|q| q.texture).collect()
        };
        let on = drawn(true);
        assert!(on.iter().any(|t| t.contains("health_back")) && on.contains(&"white".into()));
        let off = drawn(false);
        assert!(
            !off.iter()
                .any(|t| t.contains("health_back") || t == "white")
        );
    }

    fn hud_header_page(fs: &Pk3Fs) -> String {
        Hud::new(fs).expect("hud").fonts.big.page.clone()
    }

    #[test]
    fn compass_friends_are_live_teammates_and_the_packed_one() {
        let p = &PROTOCOL_V1;
        let mut ps = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            ps.fields[PlayerState::field_index(p, name).expect(name)] = v;
        };
        set("clientNum", 0);
        // Slot 9, 64 units east of the origin, pinged.
        set("iCompassFriendInfo", 9 | (16 + 255) << 6 | 255 << 15);
        set("eFlags", friends::PS_EF_FRIEND_PING);
        let client = |team: i32| {
            let mut c = ClientState::null(p);
            c.fields[ClientState::field_index(p, "team").expect("team")] = team;
            c
        };
        let mut clients = BTreeMap::new();
        for (num, team) in [(0, 2), (1, 2), (2, 1), (3, 2), (4, 2)] {
            clients.insert(num, client(team));
        }
        let player = |etype: i32, eflags: i32| {
            let mut e = EntityState::null(p);
            e.fields[EntityState::field_index(p, "eType").expect("eType")] = etype;
            e.fields[EntityState::field_index(p, "eFlags").expect("eFlags")] = eflags;
            e
        };
        let mut entities = BTreeMap::new();
        entities.insert(1, player(1, friends::EF_PING)); // teammate, pinging
        entities.insert(2, player(1, 0)); // enemy
        entities.insert(3, player(1, 1)); // dead teammate
        entities.insert(4, player(2, 0)); // a teammate's corpse
        let (fs, loc) = (Pk3Fs::empty(), Localized::default());
        let f = HudFrame {
            entities: &entities,
            ..frame(&ps, None, &fs, &loc, &clients)
        };
        let seen = compass_friends(&ps, &f);
        let who: Vec<(u32, bool)> = seen.iter().map(|s| (s.client, s.pinged)).collect();
        assert_eq!(who, [(1, true), (9, true)]);
        assert_eq!(seen[1].at, friends::Mark::At([64.0, 0.0]));

        // A spectator, or a player on no team, sees nobody.
        for team in [0, 3] {
            clients.insert(0, client(team));
            let f = HudFrame {
                entities: &entities,
                ..frame(&ps, None, &fs, &loc, &clients)
            };
            assert!(compass_friends(&ps, &f).is_empty(), "team {team}");
        }
    }

    /// `CG_MapRestart`'s HUD words go on `n`, and the next snapshot runs
    /// the respawn reset with the same spawn count; a respawn alone clears
    /// the hit icons but not the centre print.
    #[test]
    fn a_restart_clears_the_hud_words_and_rearms_the_respawn_reset() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = Hud::new(&fs).expect("hud");
        let hit = |event| DamageFeedback {
            event,
            yaw: 0,
            pitch: 0,
            count: 50,
        };
        let spawn = (0, 3);
        hud.track_spawn(spawn);
        hud.player.damage.hit(hit(1), [0.0; 3], 1_000);
        hud.player.damage.hit(hit(2), [0.0; 3], 1_000);
        hud.center_print.print("killed", 200, 1_000);
        let mut cs = vec![String::new(); 20];
        cs[15] = "90000".into();
        hud.vote.update(&cs, 15, false, 1_000);
        let icons = |hud: &Hud| hud.player.damage.live_count(1_100);
        assert_eq!(icons(&hud), 1);

        hud.track_spawn(spawn);
        assert_eq!(icons(&hud), 1, "same spawn, nothing reset");
        hud.track_spawn((0, 4));
        assert_eq!(icons(&hud), 0, "a respawn clears the icons");
        assert!(hud.center_print.alpha(1_100).is_some());

        hud.player.damage.hit(hit(3), [0.0; 3], 1_000);
        let restart = NetEvent::ServerCommand(vec!["n".into()]);
        hud.on_net_event(&restart, 1.1, &Localized::default(), &|_| None);
        assert_eq!(icons(&hud), 0);
        assert_eq!(hud.player.damage.flash_alpha(1_100), None);
        assert_eq!(hud.center_print.alpha(1_100), None);
        let none = |_: &str| None;
        let lines = hud.vote.lines(&Localized::default(), 1_100, false, &none);
        assert!(lines.is_none(), "the vote lines go");

        hud.player.damage.hit(hit(4), [0.0; 3], 1_000);
        hud.track_spawn((0, 4));
        assert_eq!(icons(&hud), 0, "cg.mapRestart takes the same spawn through");
    }

    /// `CG_TransitionPlayerState` feeds a hit whatever the frame draws:
    /// on a mounted gun, with the HUD hidden, the kick and the flash come
    /// from `feed_damage` alone. The kick splits along the drawn view, not
    /// the playerstate's.
    #[test]
    fn a_hit_feeds_the_kick_and_the_flash_on_a_mounted_gun() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let p = &PROTOCOL_V1;
        let mut hud = Hud::new(&fs).expect("hud");
        let ps = |event: i32| {
            let mut ps = PlayerState::null(p);
            let mut set = |name: &str, v: i32| {
                ps.fields[PlayerState::field_index(p, name).expect(name)] = v;
            };
            set("eFlags", crate::turret::EF_MOUNTED);
            set("damageEvent", event);
            // Travelling along +x, square into a view down +x.
            set("damageCount", 100);
            ps
        };
        hud.feed_damage(p, &ps(1), None, 1_000);
        assert_eq!(hud.player.damage.kick(), Default::default(), "baseline");
        hud.player.hidden();
        hud.feed_damage(p, &ps(2), None, 1_050);
        assert_eq!(hud.player.damage.kick().time_ms, 1_050);
        assert!((hud.player.damage.kick().pitch - 20.0).abs() < 1e-4);
        assert!(hud.player.damage.flash_alpha(1_100).is_some());

        // The drawn view looks down +y: the same hit comes from the side.
        hud.drawn_view = Some([0.0, 90.0, 0.0]);
        hud.feed_damage(p, &ps(3), None, 2_000);
        let k = hud.player.damage.kick();
        assert!(
            k.pitch.abs() < 1e-4 && (k.side.abs() - 20.0).abs() < 1e-4,
            "{k:?}"
        );
    }

    #[test]
    fn on_gamestate_forgets_the_map_but_keeps_chat() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = Hud::new(&fs).expect("hud");
        hud.kill_icons.insert(3, None);
        hud.game_messages.push_kill(
            messages::KillLine {
                attacker: None,
                icon: "gfx/hud/icon".into(),
                icon_wide: false,
                icon_flip: false,
                victim: ("victim".into(), [1.0; 3]),
            },
            0,
        );
        hud.scoreboard.on_server_command(&[
            "b".into(),
            "1".into(),
            "0".into(),
            "0".into(),
            "2".into(),
            "10".into(),
            "50".into(),
            "3".into(),
            "0".into(),
        ]);
        hud.chat.push("hello", 0);

        hud.on_gamestate();

        assert!(hud.kill_icons.is_empty());
        assert!(hud.game_messages.is_empty());
        assert!(hud.scoreboard.rows_for_team(0).is_empty());
        assert!(!hud.chat.is_empty());
    }

    /// `n` and `B` empty both message windows, kill lines included.
    #[test]
    fn a_restart_empties_the_message_windows() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = Hud::new(&fs).expect("hud");
        let loc = Localized::default();
        for restart in ["n", "B"] {
            let cmd =
                |t: &[&str]| NetEvent::ServerCommand(t.iter().map(|s| s.to_string()).collect());
            hud.on_net_event(&cmd(&["e", "hello"]), 1.0, &loc, &|_| None);
            hud.on_net_event(&cmd(&["c", "round"]), 1.0, &loc, &|_| None);
            assert!(!hud.game_messages.is_empty() && !hud.bold_messages.is_empty());
            hud.on_net_event(&cmd(&[restart]), 1.1, &loc, &|_| None);
            assert!(hud.game_messages.is_empty(), "{restart}");
            assert!(hud.bold_messages.is_empty(), "{restart}");
        }
    }

    /// An obituary becomes a kill line in the game message window, names
    /// stripped of colour codes and coloured by team.
    #[test]
    fn an_obituary_goes_to_the_game_message_window() {
        use vcod_common::net::events::GameEvent;
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = Hud::new(&fs).expect("hud");
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let ps = PlayerState::null(p);
        let clients: BTreeMap<u32, ClientState> = [(2, 1, "^1Me"), (3, 2, "Bob")]
            .into_iter()
            .map(|(n, team, name)| (n, ClientState::named(p, n, team, name)))
            .collect();
        let loc = Localized::default();
        let f = frame(&ps, None, &fs, &loc, &clients);
        let ev = GameEvent {
            event: EV_OBITUARY,
            parm: 0x80 | 7, // MOD_MELEE
            entity_num: 99,
            client_num: -1,
            weapon: 0,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            scale: 0,
            other_entity_num: 3,
            attacker_entity_num: 2,
            e_type: 0,
            eflags: 0,
        };
        hud.on_game_event(&ev, &f);
        let lines = hud.game_messages.kill_lines();
        assert_eq!(
            lines,
            [&messages::KillLine {
                attacker: Some(("Me".into(), [1.0, 0.5, 0.5])),
                icon: "killIconMelee".into(),
                icon_wide: false,
                icon_flip: false,
                victim: ("Bob".into(), [0.5, 0.5, 1.0]),
            }]
        );
    }

    /// `CG_Obituary` centre prints a kill by the drawn player only, with
    /// the teammate form for a kill on its own team; UO skips a follower's.
    #[test]
    fn a_kill_by_the_drawn_player_centre_prints() {
        use vcod_common::net::events::GameEvent;
        let mut loc = Localized::default();
        loc.parse_into(
            "cgame",
            "REFERENCE YOUKILLED\nLANG_ENGLISH \"You killed %s\"\n\
             REFERENCE TEAMMATE\nLANG_ENGLISH \"TEAMMATE\"\n",
        );
        let ev = |victim: u32, attacker: i32| GameEvent {
            event: EV_OBITUARY,
            parm: 1,
            entity_num: 99,
            client_num: -1,
            weapon: 0,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            scale: 0,
            other_entity_num: victim,
            attacker_entity_num: attacker,
            e_type: 0,
            eflags: 0,
        };
        let printed = |p: &Protocol, ev: &GameEvent, pm_flags: i32| {
            let mut ps = PlayerState::null(p);
            ps.fields[PlayerState::field_index(p, "clientNum").unwrap()] = 2;
            ps.fields[PlayerState::field_index(p, "pm_flags").unwrap()] = pm_flags;
            let clients: BTreeMap<u32, ClientState> = [(2, 1, "Me"), (3, 2, "Bob"), (4, 1, "Pal")]
                .into_iter()
                .map(|(n, team, name)| (n, ClientState::named(p, n, team, name)))
                .collect();
            let fs = Pk3Fs::empty();
            let f = HudFrame {
                protocol: p,
                ..frame(&ps, None, &fs, &loc, &clients)
            };
            obituary_center_print(&killfeed::decode_obituary(ev), &f)
        };
        let v1 = &vcod_common::net::protocol::PROTOCOL_V1;
        assert_eq!(
            printed(v1, &ev(3, 2), 0),
            Some(("You killed Bob^7".to_string(), 400))
        );
        assert_eq!(
            printed(v1, &ev(4, 2), 0),
            Some(("You killed ^1TEAMMATE^7 Pal^7".to_string(), 400))
        );
        assert_eq!(printed(v1, &ev(2, 3), 0), None, "killed, not the killer");
        assert_eq!(printed(v1, &ev(2, 2), 0), None, "suicide");
        assert_eq!(printed(v1, &ev(3, 1022), 0), None, "the world");
        assert!(printed(v1, &ev(3, 2), PMF_FOLLOW).is_some(), "1.1 follower");
        assert_eq!(
            printed(&PROTOCOL_V22, &ev(3, 2), 0),
            Some(("You killed Bob^7".to_string(), 390))
        );
        assert_eq!(printed(&PROTOCOL_V22, &ev(3, 2), PMF_FOLLOW), None);
    }
}
