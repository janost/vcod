//! On-screen HUD: chat, killfeed, Tab scoreboard and status header composed
//! into the quads [`crate::renderer::Renderer::set_hud_quads`] draws.

pub mod chat;
pub mod font;
pub mod friends;
pub mod hudelem;
pub mod hudmenu;
pub mod killfeed;
pub mod menu;
pub mod messages;
pub mod player;
pub mod scope;
pub mod scoreboard;
pub mod status;

use std::collections::{BTreeMap, HashMap};

use crate::fx::registry::EV_OBITUARY;
use crate::play::input::{EF_CROUCH, EF_PRONE};
use vcod_common::localize::Localized;
use vcod_common::net::NetEvent;
use vcod_common::net::events::GameEvent;
use vcod_common::net::flags::PMF_PRONE_BLOCKED;
use vcod_common::net::msg::{ClientState, EntityState, HudElem, PlayerState};
use vcod_common::net::protocol::Protocol;
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::Stance;
use vcod_common::pmove::predict::Predicted;
use vcod_common::pmove::weapon::SpreadStance;
use vcod_common::weapon::WeaponDef;

use chat::Chat;
use font::UiFonts;
use friends::Sighting;
use killfeed::Killfeed;
use player::{DamageFeedback, PlayerHud, PlayerView};
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

/// `unknown` counts inputs a build could not resolve (a killfeed victim with
/// no `HudFrame.clients` entry); shown on the F3 overlay.
pub struct Hud {
    /// `normal` is the body text (chat), `big` the status header and the
    /// scoreboard's headings.
    fonts: UiFonts,
    pub chat: Chat,
    /// `e`/`f` and `c`/`g` lines.
    game_messages: messages::Window,
    bold_messages: messages::Window,
    /// The `messagemode` line being typed: `main.rs` edits it, the HUD
    /// draws it.
    pub chat_field: Option<ChatField>,
    pub killfeed: Killfeed,
    /// `main.rs`'s Tab handler owns `visible` and the `score` request.
    pub scoreboard: Scoreboard,
    /// Per-CS7-index `(killIcon, wideKillIcon)`. `None` caches a failed load
    /// so it is tried once.
    kill_icons: HashMap<i32, Option<(String, bool)>>,
    player: PlayerHud,
    /// A mod's own `hud.menu` items.
    hud_menu: hudmenu::HudMenu,
    pub unknown: u64,
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
    /// The server's open script menu, if any; drawn on top of everything else.
    pub menu: Option<&'a menu::MenuView>,
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
    /// A client cvar: a server `v`, else the 140/204 mirror. Mod `hud.menu`
    /// items read it.
    pub cvar: &'a dyn Fn(&str) -> Option<String>,
    /// The key text a command is bound to, `None` while unbound.
    pub bound_key: &'a dyn Fn(&str) -> Option<String>,
    pub draw: DrawToggles,
    /// Our pending weapon switch, the cgame's `cg.weaponSelect`
    /// (0x301cbb0c) while it differs from `ps.weapon`; the weapon name shows
    /// it while it is held.
    pub weapon_select: Option<u8>,
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
            chat_field: None,
            killfeed: Killfeed::new(),
            scoreboard: Scoreboard::new(),
            kill_icons: HashMap::new(),
            player: PlayerHud::default(),
            hud_menu: hudmenu::HudMenu::load(fs),
            unknown: 0,
        })
    }

    /// A download reopened the search path: reload the fonts and `hud.menu`,
    /// and drop the kill icons read out of the old weapon files. A font the new paks do
    /// not parse keeps the old one.
    pub fn reopen(&mut self, fs: &Pk3Fs) -> Result<(), String> {
        self.fonts = UiFonts::load(fs)?;
        self.kill_icons.clear();
        self.hud_menu = hudmenu::HudMenu::load(fs);
        Ok(())
    }

    /// Net does not filter `ServerCommand`; `Scoreboard::on_server_command`
    /// ignores anything but `b`. `CG_ServerCommand` (0x3002e0d0) localizes
    /// a chat line or a message through `loc` before it is kept.
    pub fn on_net_event(&mut self, ev: &NetEvent, now: f32, loc: &Localized) {
        let now_ms = (now * 1000.0) as i32;
        match ev {
            NetEvent::Chat { text, .. } => self.chat.push(&loc.message(text), now_ms),
            NetEvent::ServerCommand(tokens) => {
                self.scoreboard.on_server_command(tokens);
                let text = || bind_keys(&loc.message(tokens.get(1).map_or("", String::as_str)));
                match tokens.first().map(String::as_str) {
                    Some("a") => self.weapon_selected(),
                    Some("e" | "f") => self.game_messages.push(&text(), now_ms),
                    Some("c" | "g") => self.bold_messages.push(&text(), now_ms),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// A weapon-select bind ran: the weapon name shows again.
    pub fn weapon_selected(&mut self) {
        self.player.weapon_name.select();
    }

    /// A new gamestate: CS 7 indices and the scoreboard change, chat does not.
    pub fn on_gamestate(&mut self) {
        self.kill_icons.clear();
        self.killfeed.clear();
        self.scoreboard.clear();
    }

    /// `EV_OBITUARY` to the killfeed. Entity number == clientNum for players
    /// (docs/research/cod11-hud-protocol.md, section 1). An unresolved victim
    /// drops the row and counts toward `unknown`; an unresolved attacker
    /// draws a victim-only row, as the stock client does for a world attacker.
    pub fn on_game_event(&mut self, ev: &GameEvent, f: &HudFrame) {
        if ev.event != EV_OBITUARY {
            return;
        }
        let ob = killfeed::decode_obituary(ev);

        let resolve = |num: u32| -> Option<(String, [f32; 4])> {
            let client = f.clients.get(&num)?;
            let team = client.field_i32(f.protocol, "team");
            Some((client.name(f.protocol), killfeed::team_color(team)))
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
        let icon = killfeed::icon_for(&ob, kill_icon.as_ref().map(|(path, _)| path.as_str()));

        self.killfeed.push(killfeed::Entry {
            attacker,
            victim,
            icon,
            icon_wide,
            spawn: f.now,
        });
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
        let mut out = Vec::new();
        let screen = (f.screen_w, f.screen_h);
        let pm_type = |ps: &PlayerState| ps.field_i32(f.protocol, "pm_type");
        match f.ps.filter(|ps| draws_native_hud(pm_type(ps))) {
            Some(ps) => {
                let friends = compass_friends(ps, f);
                let mut view = player_view(ps, &friends, f);
                // The replay at its own clock, or a followed player's
                // snapshot at the render clock: retail runs the sway on
                // whichever playerstate it draws.
                let (gun_ps, gun_ms) = match f.predicted {
                    Some(pred) => (pred.ps, pred.command_time),
                    None => (
                        vcod_common::pmove::predict::from_wire(f.protocol, ps, None).ps,
                        f.server_time,
                    ),
                };
                let gun = &mut self.player.gun;
                gun.feed(view.client_num, view.damage, &gun_ps, gun_ms);
                view.gun_angles = gun.step(view.weapon, &gun_ps, gun_ms);
                let cx = player::Context {
                    weapons: f.weapons,
                    configstrings: f.configstrings,
                    loc: f.localized,
                    font: &self.fonts.normal,
                    entity_origin: f.entity_origin,
                    bound_key: f.bound_key,
                    draw: f.draw,
                };
                self.player
                    .build(&view, &cx, f.server_time, screen, &mut out);
                // `Menu_PaintAll` paints the rest of hud.menu over the native items.
                if f.draw.status {
                    self.hud_menu
                        .build(&self.fonts, screen, f.localized, f.cvar, &mut out);
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
                f.localized,
                &self.fonts,
                f.server_time,
                screen,
                &mut out,
            );
        }
        let v = hudelem::Virtual::new(screen);
        let now_ms = (f.now * 1000.0) as i32;
        // The viewer's own team colours the chat strip and names.
        let team = f.ps.map_or(0, |ps| {
            let own = ps.field_i32(f.protocol, "clientNum") as u32;
            f.clients
                .get(&own)
                .map_or(0, |c| c.field_i32(f.protocol, "team"))
        });
        let [r, g, b, _] = killfeed::team_color(team);
        self.chat
            .build(&self.fonts, [r, g, b], now_ms, &v, &mut out);
        self.game_messages.build(&self.fonts, now_ms, &v, &mut out);
        self.bold_messages.build(&self.fonts, now_ms, &v, &mut out);
        if let Some(field) = &self.chat_field {
            chat::field(field, &self.fonts, f.localized, &v, &mut out);
        }
        self.killfeed
            .build(&self.fonts.normal, HUD_SCALE, f.now, &mut out);
        // The scoreboard reuses the parsed gametype; its last `b` reply
        // overrides CS 5/6 (status.rs).
        let status = status::read_status(f.configstrings, f.server_time, self.scoreboard.totals());
        // vcod's own header, which retail does not draw: spectators only.
        if !f.local_player {
            status::build(&status, &self.fonts.big, f.screen_w, &mut out);
        }
        if self.scoreboard.visible {
            let names = |client: u32| -> Option<(String, i32)> {
                let cs = f.clients.get(&client)?;
                let team = cs.field_i32(f.protocol, "team");
                Some((cs.name(f.protocol), team))
            };
            self.scoreboard.build(
                &self.fonts.big,
                &self.fonts.normal,
                f.screen_w,
                &names,
                &status.gametype,
                &mut out,
            );
        }
        // Last, so the open menu sits on top of everything else.
        if let Some(view) = f.menu {
            menu::build(
                view,
                &self.fonts.normal,
                HUD_SCALE,
                f.screen_w,
                f.screen_h,
                &mut out,
            );
        }
        out
    }
}

/// A line being typed after `messagemode` (`say`) or `messagemode2`
/// (`say_team`).
#[derive(Default)]
pub struct ChatField {
    pub team: bool,
    pub text: String,
}

/// The cgame's `[{command}]` pass over a localized message (0x300229b0):
/// the key bound to the command, in its brackets. Only the use key is
/// filled; any other command is left as written, which is what retail shows
/// for an unbound one.
pub fn bind_keys(text: &str) -> String {
    text.replace("[{+activate}]", "[F]")
}

/// `pm_type` 4 is a free-flying spectator and 5 the intermission; any other,
/// a follower's copy of its target included, draws the menu HUD.
fn draws_native_hud(pm_type: i32) -> bool {
    !matches!(pm_type, 4 | 5)
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
    let name_weapon = f
        .weapon_select
        .map(usize::from)
        .filter(|&w| w < 64 && held & 1 << w != 0)
        .unwrap_or(weapon);
    PlayerView {
        client_num: int("clientNum"),
        health: ps.health(),
        max_health: ps.max_health(),
        eflags,
        weapon: f.weapons.get(weapon).and_then(Option::as_ref),
        name_weapon: f.weapons.get(name_weapon).and_then(Option::as_ref),
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
        damage: DamageFeedback {
            event: int("damageEvent"),
            yaw: int("damageYaw"),
            pitch: int("damagePitch"),
            count: int("damageCount"),
        },
        spawn_count: ps.arrays.stats[5],
        // The cgame reads it off the playerstate it predicts.
        prone_blocked: f
            .predicted
            .map_or(int("pm_flags") & PMF_PRONE_BLOCKED != 0, |pred| {
                pred.ps.prone_blocked
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::msg::hud_field as msg_field;
    use vcod_common::net::protocol::PROTOCOL_V1;

    static NO_ENTITIES: BTreeMap<u32, EntityState> = BTreeMap::new();

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
            menu: None,
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
            cvar: &|_| None,
            bound_key: &|_| None,
            draw: DrawToggles::default(),
            weapon_select: None,
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
        set("weapons[0]", 0b110);
        let def = |name: &str| {
            Some(WeaponDef {
                display_name: name.into(),
                ..Default::default()
            })
        };
        let weapons = [None, def("rifle"), def("pistol"), def("grenade")];
        let (fs, loc, clients) = (Pk3Fs::empty(), Localized::default(), BTreeMap::new());
        let named = |select: Option<u8>| {
            let f = HudFrame {
                weapons: &weapons,
                weapon_select: select,
                ..frame(&ps, None, &fs, &loc, &clients)
            };
            let v = player_view(&ps, &[], &f);
            assert_eq!(v.weapon.map(|d| d.display_name.as_str()), Some("rifle"));
            v.name_weapon.map(|d| d.display_name.clone())
        };
        assert_eq!(named(None).as_deref(), Some("rifle"));
        assert_eq!(named(Some(2)).as_deref(), Some("pistol"));
        assert_eq!(named(Some(3)).as_deref(), Some("rifle"), "not held");
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

    #[test]
    fn on_gamestate_forgets_the_map_but_keeps_chat() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let mut hud = Hud::new(&fs).expect("hud");
        hud.kill_icons.insert(3, None);
        hud.killfeed.push(killfeed::Entry {
            attacker: None,
            victim: ("victim".into(), [1.0, 1.0, 1.0, 1.0]),
            icon: "gfx/hud/icon".into(),
            icon_wide: false,
            spawn: 0.0,
        });
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
        assert!(hud.killfeed.is_empty());
        assert!(hud.scoreboard.rows_for_team(1).is_empty());
        assert!(!hud.chat.is_empty());
    }
}
