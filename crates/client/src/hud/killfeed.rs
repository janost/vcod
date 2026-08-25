//! Top-left killfeed: the last [`FEED_LINES`] obituaries, newest at the bottom.
//! Field mapping and icon rule: docs/research/cod11-hud-protocol.md, sections 1-2.

use std::collections::VecDeque;

use super::font::{self, Font};
use super::HudQuad;
use vcod_common::net::events::GameEvent;

pub const FEED_LINES: usize = 5;
pub const FEED_LIFE: f32 = 6.0;

/// `attackerEntityNum` for a world kill (doc section 1).
const ENTITYNUM_WORLD: i32 = 1022;

/// `Obituary::mod_` on a weapon death; real ids are 0..=24.
const NO_MOD: i32 = -1;

// Means-of-death ids (doc section 2); only the ones used here are named.
#[cfg_attr(not(test), allow(dead_code))] // only exercised by this module's own tests
const MOD_RIFLE_BULLET: i32 = 2;
const MOD_MELEE: i32 = 7;
const MOD_HEAD_SHOT: i32 = 8;
const MOD_WATER: i32 = 16;
const MOD_SLIME: i32 = 17;
const MOD_CRUSH: i32 = 19;
const MOD_FALLING: i32 = 21;
const MOD_SUICIDE: i32 = 22;

/// A decoded `EV_OBITUARY`. `attacker` is `None` for a world kill or suicide.
/// Exactly one of `weapon`/`mod_` is meaningful, discriminated by `mod_ == NO_MOD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obituary {
    pub attacker: Option<u32>,
    pub victim: u32,
    /// 1-based index into CS 7; 0 on a MOD death.
    pub weapon: i32,
    /// Means-of-death id (doc section 2); `NO_MOD` on a weapon death.
    pub mod_: i32,
}

/// Doc section 1 for the victim/attacker fields, section 2 for `eventParm`
/// bit 7 selecting MOD versus weapon index.
pub fn decode_obituary(ev: &GameEvent) -> Obituary {
    let victim = ev.other_entity_num;
    let attacker_raw = ev.attacker_entity_num;
    // A negative attacker is `GameEvent`'s no-entity sentinel; guard it
    // before the `as u32` cast.
    let attacker =
        (attacker_raw >= 0 && attacker_raw != ENTITYNUM_WORLD && attacker_raw as u32 != victim)
            .then_some(attacker_raw as u32);
    let (weapon, mod_) = if ev.parm & 0x80 != 0 {
        (0, ev.parm & 0x7f)
    } else {
        (ev.parm, NO_MOD)
    };
    Obituary {
        attacker,
        victim,
        weapon,
        mod_,
    }
}

/// The seven MODs the server tags with bit 7; anything else falls to
/// `killIconDied` like the client's switch default. Drown and slime ship no
/// art (doc section 2).
fn mod_icon(mod_: i32) -> &'static str {
    match mod_ {
        MOD_MELEE => "gfx/hud/death_melee",
        MOD_HEAD_SHOT => "gfx/hud/death_headshot",
        MOD_WATER => "gfx/hud/death_drown",
        MOD_SLIME => "gfx/hud/death_slime",
        MOD_CRUSH => "gfx/hud/death_crush",
        MOD_FALLING => "gfx/hud/death_falling",
        MOD_SUICIDE => "gfx/hud/death_suicide",
        _ => "gfx/hud/death_died",
    }
}

/// The weapon's `killIcon` when non-empty, else the MOD icon. `kill_icon` is
/// resolved by the caller, which has filesystem access.
pub fn icon_for(ob: &Obituary, kill_icon: Option<&str>) -> String {
    if let Some(icon) = kill_icon.filter(|s| !s.is_empty()) {
        return icon.to_string();
    }
    mod_icon(ob.mod_).to_string()
}

/// Stock `g_TeamColor_Axis`/`g_TeamColor_Allies` from
/// `pak0.pk3:safemode_mp.cfg`; the cvars are server-settable.
const AXIS_COLOR: [f32; 4] = [1.0, 0.5, 0.5, 1.0];
const ALLIES_COLOR: [f32; 4] = [0.5, 0.5, 1.0, 1.0];

/// Team 1 is Axis, 2 is Allies; 0 and 3 render white (doc section 1).
pub fn team_color(team: i32) -> [f32; 4] {
    match team {
        1 => AXIS_COLOR,
        2 => ALLIES_COLOR,
        _ => [1.0, 1.0, 1.0, 1.0],
    }
}

/// One row with names, colours and icon resolved. `icon_wide` is the weapon
/// file's `wideKillIcon`; MOD icons are never wide (doc section 2).
pub struct Entry {
    pub attacker: Option<(String, [f32; 4])>,
    pub victim: (String, [f32; 4]),
    pub icon: String,
    pub icon_wide: bool,
    pub spawn: f32,
}

/// The last [`FEED_LINES`] obituaries, oldest first.
pub struct Killfeed {
    entries: VecDeque<Entry>,
}

impl Killfeed {
    pub fn new() -> Killfeed {
        Killfeed {
            entries: VecDeque::new(),
        }
    }

    pub fn push(&mut self, e: Entry) {
        self.entries.push_back(e);
        while self.entries.len() > FEED_LINES {
            self.entries.pop_front();
        }
    }

    #[allow(dead_code)] // called by the loading task
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Row `i` at `y = 16 + line_h * i`: attacker, icon (`2 * line_h` wide
    /// when `icon_wide`), victim. Alpha fades over the last second of
    /// [`FEED_LIFE`].
    pub fn build(&self, font: &Font, scale: f32, now: f32, out: &mut Vec<HudQuad>) {
        let line_h = font.line_height(scale);
        let alive = self.entries.iter().filter(|e| now - e.spawn < FEED_LIFE);
        for (i, e) in alive.enumerate() {
            let age = now - e.spawn;
            let alpha = 1.0 - (age - (FEED_LIFE - 1.0)).clamp(0.0, 1.0);
            let y = 16.0 + line_h * i as f32;
            let mut x = 16.0;
            let base = out.len();
            if let Some((name, color)) = &e.attacker {
                x += font::layout(font, name, x, y, scale, *color, out);
                x += 4.0;
            }
            let icon_w = if e.icon_wide { 2.0 * line_h } else { line_h };
            out.push(HudQuad {
                verts: [
                    [x, y],
                    [x + icon_w, y],
                    [x + icon_w, y + line_h],
                    [x, y + line_h],
                ],
                uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                rgba: [1.0, 1.0, 1.0, 1.0],
                texture: e.icon.clone(),
            });
            x += icon_w + 4.0;
            let (name, color) = &e.victim;
            font::layout(font, name, x, y, scale, *color, out);
            // Multiply rather than overwrite: shadow quads carry their own 0.8 alpha.
            for q in &mut out[base..] {
                q.rgba[3] *= alpha;
            }
        }
    }
}

impl Default for Killfeed {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(parm: i32, victim: u32, attacker: i32) -> GameEvent {
        GameEvent {
            event: crate::fx::registry::EV_OBITUARY,
            parm,
            entity_num: 99,
            client_num: -1,
            weapon: 0,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            other_entity_num: victim,
            attacker_entity_num: attacker,
        }
    }

    #[test]
    fn feed_keeps_five_newest_and_expires() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = crate::hud::font::load_font(&fs, 16).unwrap();
        let mut k = Killfeed::new();
        for i in 0..7 {
            k.push(Entry {
                attacker: Some((format!("a{i}"), [1.0; 4])),
                victim: (format!("v{i}"), [1.0; 4]),
                icon: "gfx/hud/death_melee".into(),
                icon_wide: false,
                spawn: i as f32,
            });
        }
        let mut out = Vec::new();
        k.build(&f, 1.0, 6.5, &mut out);
        let icons = out
            .iter()
            .filter(|q| q.texture == "gfx/hud/death_melee")
            .count();
        assert_eq!(icons, 5); // entries 2..6 alive (spawn+6 > 6.5), one icon quad each
        out.clear();
        k.build(&f, 1.0, 13.5, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn wide_icon_draws_twice_the_normal_width() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = crate::hud::font::load_font(&fs, 16).unwrap();
        let entry = |icon_wide| Entry {
            attacker: None,
            victim: ("v".into(), [1.0; 4]),
            icon: "gfx/hud/hud@death_thompson".into(),
            icon_wide,
            spawn: 0.0,
        };
        let icon_quad_width = |wide| {
            let mut k = Killfeed::new();
            k.push(entry(wide));
            let mut out = Vec::new();
            k.build(&f, 1.0, 0.0, &mut out);
            let q = out
                .iter()
                .find(|q| q.texture == "gfx/hud/hud@death_thompson")
                .unwrap();
            q.verts[1][0] - q.verts[0][0] // TR.x - TL.x
        };
        let normal = icon_quad_width(false);
        let wide = icon_quad_width(true);
        assert!((wide - 2.0 * normal).abs() < 0.01, "{normal} vs {wide}");
        assert!((normal - f.line_height(1.0)).abs() < 0.01, "{normal}");
    }

    #[test]
    fn icon_prefers_weapon_kill_icon_for_weapon_deaths() {
        let ob = Obituary {
            attacker: Some(1),
            victim: 2,
            weapon: 3,
            mod_: MOD_RIFLE_BULLET,
        };
        assert_eq!(
            icon_for(&ob, Some("gfx/hud/hud@death_kar98.tga")),
            "gfx/hud/hud@death_kar98.tga"
        );
        let ob = Obituary {
            attacker: None,
            victim: 2,
            weapon: 0,
            mod_: MOD_FALLING,
        };
        assert_eq!(icon_for(&ob, None), "gfx/hud/death_falling");
    }

    #[test]
    fn real_weapon_files_carry_kill_icons() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let w = vcod_common::weapon::load(&fs, "kar98k_mp").unwrap();
        assert_eq!(w.kill_icon.as_deref(), Some("gfx/hud/hud@death_kar98.tga"));
    }

    #[test]
    fn decode_reads_victim_attacker_and_weapon_index() {
        let ob = decode_obituary(&ev(5, 3, 7));
        assert_eq!(
            ob,
            Obituary {
                attacker: Some(7),
                victim: 3,
                weapon: 5,
                mod_: NO_MOD,
            }
        );
    }

    #[test]
    fn decode_reads_mod_when_bit_seven_is_set() {
        let parm = 0x80 | MOD_FALLING;
        let ob = decode_obituary(&ev(parm, 3, 7));
        assert_eq!(
            ob,
            Obituary {
                attacker: Some(7),
                victim: 3,
                weapon: 0,
                mod_: MOD_FALLING,
            }
        );
    }

    #[test]
    fn decode_blanks_attacker_for_world_kills_and_suicides() {
        // world (ENTITYNUM_WORLD)
        assert_eq!(decode_obituary(&ev(5, 3, 1022)).attacker, None);
        // suicide
        assert_eq!(decode_obituary(&ev(5, 3, 3)).attacker, None);
        assert_eq!(decode_obituary(&ev(5, 3, 7)).attacker, Some(7));
    }

    #[test]
    fn decode_guards_a_negative_attacker_entity_num_before_the_cast() {
        // GameEvent's no-entity sentinel must not wrap through `as u32`.
        assert_eq!(decode_obituary(&ev(5, 3, -1)).attacker, None);
    }

    #[test]
    fn team_color_survives_a_color_code_run_after_caret_seven() {
        // ^7 restores the row's own colour, not white (doc section 7).
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = font::load_font(&fs, 16).unwrap();
        let team = team_color(2); // not white
        let mut k = Killfeed::new();
        k.push(Entry {
            attacker: None,
            victim: ("^1Foo^7bar".to_string(), team),
            icon: "gfx/hud/death_died".into(),
            icon_wide: false,
            spawn: 0.0,
        });
        let mut out = Vec::new();
        k.build(&f, 1.0, 0.0, &mut out);
        // Glyph quads only: skip the icon quad and the shadow quads.
        let glyph_quads: Vec<&HudQuad> = out
            .iter()
            .filter(|q| q.texture == f.page && q.rgba != [0.0, 0.0, 0.0, 0.8])
            .collect();
        assert!(
            glyph_quads.iter().any(|q| q.rgba == team),
            "expected a glyph carrying the team color after ^7"
        );
        assert!(
            !glyph_quads.iter().any(|q| q.rgba == font::COLORS[7]),
            "^7 must not fall back to white when the row's own color isn't white"
        );
    }

    #[test]
    fn push_evicts_oldest_past_feed_lines() {
        let mut k = Killfeed::new();
        for i in 0..(FEED_LINES + 2) {
            k.push(Entry {
                attacker: None,
                victim: (format!("v{i}"), [1.0; 4]),
                icon: "gfx/hud/death_died".into(),
                icon_wide: false,
                spawn: 0.0,
            });
        }
        assert_eq!(k.entries.len(), FEED_LINES);
        assert_eq!(k.entries.front().unwrap().victim.0, "v2");
    }
}
