//! On-screen HUD: chat, killfeed, Tab scoreboard and status header composed
//! into the quads [`crate::renderer::Renderer::set_hud_quads`] draws.

pub mod chat;
pub mod font;
pub mod hudelem;
pub mod killfeed;
pub mod menu;
pub mod player;
pub mod scoreboard;
pub mod status;

use std::collections::{BTreeMap, HashMap};

use crate::fx::registry::EV_OBITUARY;
use crate::play::input::{EF_CROUCH, EF_PRONE};
use vcod_common::localize::Localized;
use vcod_common::net::events::GameEvent;
use vcod_common::net::msg::{ClientState, HudElem, PlayerState};
use vcod_common::net::protocol::Protocol;
use vcod_common::net::NetEvent;
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::predict::Predicted;
use vcod_common::pmove::weapon::PMF_ADS_WALK;
use vcod_common::pmove::Stance;
use vcod_common::weapon::WeaponDef;

use chat::Chat;
use font::Font;
use killfeed::Killfeed;
use player::{DamageFeedback, PlayerHud, PlayerView};
use scoreboard::Scoreboard;

/// Body text, e.g. chat.
pub const SIZE_TEXT: u32 = 16;
/// Status header and scoreboard section headings.
pub const SIZE_HEADER: u32 = 24;

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
    font_text: Font,
    font_header: Font,
    pub chat: Chat,
    pub killfeed: Killfeed,
    /// `main.rs`'s Tab handler owns `visible` and the `score` request.
    pub scoreboard: Scoreboard,
    /// Per-CS7-index `(killIcon, wideKillIcon)`. `None` caches a failed load
    /// so it is tried once.
    kill_icons: HashMap<i32, Option<(String, bool)>>,
    player: PlayerHud,
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
    /// Lazy weapon-file loads for killfeed icons.
    pub fs: &'a Pk3Fs,
    /// The server's open script menu, if any; drawn on top of everything else.
    pub menu: Option<&'a menu::MenuView>,
    /// The newest snapshot's playerstate: ours, or the followed player's.
    /// Its hudelems are drawn whoever it belongs to.
    pub ps: Option<&'a PlayerState>,
    /// Our replay while predicting; its weapon, ammo, spread and stance stand
    /// in for the snapshot's.
    pub predicted: Option<&'a Predicted>,
    /// `ps` is ours and alive (`pm_type` 0 or 1), which is when the native
    /// player HUD is drawn.
    pub local_player: bool,
    /// Configstring 7's defs, index = weapon number.
    pub weapons: &'a [Option<WeaponDef>],
    pub localized: &'a Localized,
    /// The camera's yaw in degrees, and its eye.
    pub view_yaw: f32,
    pub eye: [f32; 3],
    /// The drawn vertical fov, degrees.
    pub fov: f32,
    /// An entity's current origin, for objectives placed on one.
    pub entity_origin: &'a dyn Fn(i32) -> Option<[f32; 3]>,
}

impl Hud {
    pub fn new(fs: &Pk3Fs) -> Result<Hud, String> {
        Ok(Hud {
            font_text: font::load_font(fs, SIZE_TEXT)?,
            font_header: font::load_font(fs, SIZE_HEADER)?,
            chat: Chat::new(),
            killfeed: Killfeed::new(),
            scoreboard: Scoreboard::new(),
            kill_icons: HashMap::new(),
            player: PlayerHud::default(),
            unknown: 0,
        })
    }

    /// Net does not filter `ServerCommand`; `Scoreboard::on_server_command`
    /// ignores anything but `b`.
    pub fn on_net_event(&mut self, ev: &NetEvent, now: f32) {
        match ev {
            NetEvent::Chat { text, team } => self.chat.push(text, *team, now),
            NetEvent::ServerCommand(tokens) => self.scoreboard.on_server_command(tokens),
            _ => {}
        }
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
        match f.ps.filter(|_| f.local_player) {
            Some(ps) => {
                let view = player_view(ps, f);
                let cx = player::Context {
                    weapons: f.weapons,
                    configstrings: f.configstrings,
                    loc: f.localized,
                    font: &self.font_text,
                    entity_origin: f.entity_origin,
                };
                self.player
                    .build(&view, &cx, f.server_time, screen, &mut out);
            }
            // The next life starts from a fresh baseline, so a hit taken
            // meanwhile does not flash on return.
            None => self.player = PlayerHud::default(),
        }
        if let Some(ps) = f.ps {
            let elems: Vec<HudElem> = ps
                .arrays
                .hud_archived
                .iter()
                .chain(&ps.arrays.hud_current)
                .copied()
                .collect();
            // No fixed-width atlas ships: bigfixed takes the header font.
            let fonts = (&self.font_text, &self.font_header, &self.font_text);
            hudelem::build(
                &elems,
                f.configstrings,
                f.localized,
                fonts,
                f.server_time,
                screen,
                &mut out,
            );
        }
        self.chat
            .build(&self.font_text, HUD_SCALE, f.screen_h, f.now, &mut out);
        self.killfeed
            .build(&self.font_text, HUD_SCALE, f.now, &mut out);
        // The scoreboard reuses the parsed gametype; its last `b` reply
        // overrides CS 5/6 (status.rs).
        let status = status::read_status(f.configstrings, f.server_time, self.scoreboard.totals());
        status::build(&status, &self.font_header, f.screen_w, &mut out);
        if self.scoreboard.visible {
            let names = |client: u32| -> Option<(String, i32)> {
                let cs = f.clients.get(&client)?;
                let team = cs.field_i32(f.protocol, "team");
                Some((cs.name(f.protocol), team))
            };
            self.scoreboard.build(
                &self.font_header,
                &self.font_text,
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
                &self.font_text,
                HUD_SCALE,
                f.screen_w,
                f.screen_h,
                &mut out,
            );
        }
        out
    }
}

/// The native HUD's inputs off `ps`, with the replay's fields in place of
/// the snapshot's where `f.predicted` has them.
fn player_view<'a>(ps: &'a PlayerState, f: &HudFrame<'a>) -> PlayerView<'a> {
    let int = |name: &str| ps.field_i32(f.protocol, name);
    let mut eflags = int("eFlags");
    let (weapon, ammo, ammoclip, aim_spread_scale, ads_frac, ads_held) = match f.predicted {
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
                s.weapon_pos_frac,
                s.walking,
            )
        }
        None => (
            int("weapon") as usize,
            &ps.arrays.ammo,
            &ps.arrays.ammoclip,
            ps.field_f32(f.protocol, "aimSpreadScale"),
            ps.field_f32(f.protocol, "fWeaponPosFrac"),
            int("pm_flags") & PMF_ADS_WALK != 0,
        ),
    };
    PlayerView {
        client_num: int("clientNum"),
        health: ps.health(),
        max_health: ps.max_health(),
        eflags,
        weapon: f.weapons.get(weapon).and_then(Option::as_ref),
        ammo,
        ammoclip,
        aim_spread_scale,
        ads_frac,
        ads_held,
        view_yaw: f.view_yaw,
        eye: f.eye,
        fov: (fov_x_4_3(f.fov), f.fov),
        objectives: &ps.arrays.objectives,
        cursor_hint: int("serverCursorHint"),
        // Playerstate fields arrive unsigned; retail's -1 is 255.
        cursor_hint_string: i32::from(int("serverCursorHintString") as u8 as i8),
        damage: DamageFeedback {
            event: int("damageEvent"),
            yaw: int("damageYaw"),
            pitch: int("damagePitch"),
            count: int("damageCount"),
        },
    }
}

/// The horizontal fov across the 640x480 virtual screen for a vertical
/// `fov_y`, both in degrees.
fn fov_x_4_3(fov_y: f32) -> f32 {
    2.0 * ((fov_y / 2.0).to_radians().tan() * 4.0 / 3.0)
        .atan()
        .to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::msg::hud_field as msg_field;
    use vcod_common::net::protocol::PROTOCOL_V1;

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
            fs,
            menu: None,
            ps: Some(ps),
            predicted,
            local_player: true,
            weapons: &[],
            localized: loc,
            view_yaw: 0.0,
            eye: [0.0; 3],
            fov: 75.0,
            entity_origin: &|_| None,
        }
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
        ps.arrays.ammoclip[10] = 5;
        let mut pred = Predicted {
            ps: vcod_common::pmove::PlayerState::spawn(glam::Vec3::ZERO, 0.0),
            pm_type: 0,
            delta_angles: [0; 3],
            command_time: 0,
            view_lerp_start: 0,
            event_sequence: 0,
            events: [0; 4],
            event_parms: [0; 4],
        };
        pred.ps.ammoclip[10] = 4;
        pred.ps.aim_spread_scale = 50.0;
        pred.ps.stance = Stance::Crouch;
        let (fs, loc, clients) = (Pk3Fs::empty(), Localized::default(), BTreeMap::new());

        let snap = player_view(&ps, &frame(&ps, None, &fs, &loc, &clients));
        assert_eq!(
            (snap.ammoclip[10], snap.aim_spread_scale, snap.eflags),
            (5, 200.0, 0x10)
        );
        assert_eq!(snap.cursor_hint_string, -1, "retail's -1 arrives as 255");

        let own = player_view(&ps, &frame(&ps, Some(&pred), &fs, &loc, &clients));
        assert_eq!(
            (own.ammoclip[10], own.aim_spread_scale, own.eflags),
            (4, 50.0, 0x10 | EF_CROUCH)
        );
    }

    #[test]
    fn the_native_hud_waits_for_our_own_live_playerstate_and_hudelems_do_not() {
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
        let drawn = |hud: &mut Hud, local_player: bool| -> Vec<String> {
            let f = HudFrame {
                configstrings: &cs,
                local_player,
                ..frame(&ps, None, &fs, &loc, &clients)
            };
            hud.build(&f).into_iter().map(|q| q.texture).collect()
        };

        let following = drawn(&mut hud, false);
        assert!(following.iter().any(|t| t == "white"));
        assert!(!following.iter().any(|t| t.contains("health_back")));

        let own = drawn(&mut hud, true);
        assert!(own.iter().any(|t| t == "white"));
        assert!(own.iter().any(|t| t.contains("health_back")));
    }

    #[test]
    fn the_virtual_screen_fov_is_4_3_wide() {
        // A 90-degree 4:3 frustum is 73.74 degrees tall.
        assert!((fov_x_4_3(73.739_8) - 90.0).abs() < 1e-3);
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
        hud.chat.push("hello", false, 0.0);

        hud.on_gamestate();

        assert!(hud.kill_icons.is_empty());
        assert!(hud.killfeed.is_empty());
        assert!(hud.scoreboard.rows_for_team(1).is_empty());
        assert!(!hud.chat.is_empty());
    }
}
