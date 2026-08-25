//! On-screen HUD: chat, killfeed, Tab scoreboard and status header composed
//! into the quads [`crate::renderer::Renderer::set_hud_quads`] draws.

pub mod chat;
pub mod font;
pub mod killfeed;
pub mod scoreboard;
pub mod status;

use std::collections::{BTreeMap, HashMap};

use crate::fx::registry::EV_OBITUARY;
use vcod_common::net::events::GameEvent;
use vcod_common::net::msg::ClientState;
use vcod_common::net::protocol::Protocol;
use vcod_common::net::NetEvent;
use vcod_common::pk3::Pk3Fs;

use chat::Chat;
use font::Font;
use killfeed::Killfeed;
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
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
