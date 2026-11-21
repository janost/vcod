//! Obituaries: `EV_OBITUARY` decoded into a kill line for the game message
//! window (`messages::KillLine`), as `CG_Obituary` hands one to the engine.
//! Field mapping, icon rule and placement: docs/research/cod11-hud-protocol.md,
//! sections 1-2 and 1.1.

use vcod_common::net::events::GameEvent;

/// `attackerEntityNum` for a world kill (doc section 1).
const ENTITYNUM_WORLD: i32 = vcod_common::net::protocol::ENTITYNUM_WORLD as i32;

/// `Obituary::mod_` on a weapon death; real ids are 0..=26.
const NO_MOD: i32 = -1;

/// A decoded `EV_OBITUARY`. `attacker` is `None` for a world kill or suicide.
/// Exactly one of `weapon`/`mod_` is meaningful, discriminated by `mod_ == NO_MOD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obituary {
    pub attacker: Option<u32>,
    pub victim: u32,
    /// 1-based index into CS 7; 0 on a MOD death.
    pub weapon: i32,
    /// The wire's means-of-death id, in the server protocol's numbering
    /// (doc section 2, coduo-diff.md 12); `NO_MOD` on a weapon death.
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

/// `CG_Obituary`'s icon switch on a bit-7 parm: the material each case
/// names, `killIconDied` for any other (doc section 2). The `killIcon*`
/// materials are pak4's `scripts/hud.shader` (drown and slime draw
/// `death_died`) and UO's `scripts/gmi_hud.shader`. 1.5's switch is 1.1's
/// (`cgame_mp_x86.dll` 0x3001e9f5); UO renumbered the ids and added
/// artillery, the tank and jeep crush and binoculars (`uo_cgame_mp_x86.dll`
/// 0x3002231e, coduo-diff.md 12).
fn mod_icon(mod_: i32, uo: bool) -> &'static str {
    match (uo, mod_) {
        (_, 7) => "killIconMelee",
        (_, 8) => "killIconHeadShot",
        (false, 16) | (true, 15) => "killIconDrown",
        (false, 17) => "killIconSlime",
        (false, 19) | (true, 16) => "killIconCrush",
        (false, 21) | (true, 20) => "killIconFalling",
        (false, 22) | (true, 21) => "killIconSuicide",
        (true, 13) => "gfx/icons/hud@artillery",
        (true, 17) => "killIconCrushTank",
        (true, 18) => "killIconCrushJeep",
        (true, 26) => "gfx/icons/hud@bino_owned",
        _ => "killIconDied",
    }
}

/// The weapon's `killIcon` when non-empty, else the MOD icon. `kill_icon` is
/// resolved by the caller, which has filesystem access; `uo` picks United
/// Offensive's switch.
pub fn icon_for(ob: &Obituary, kill_icon: Option<&str>, uo: bool) -> String {
    if let Some(icon) = kill_icon.filter(|s| !s.is_empty()) {
        return icon.to_string();
    }
    mod_icon(ob.mod_, uo).to_string()
}

/// CoD2's and CoD4's means-of-death icon (`eventParm & 0x7f` under bit
/// 7); CoD4 adds `killiconimpact` for 15.
pub fn iw_mod_icon(mod_: u32, cod4: bool) -> &'static str {
    match mod_ {
        7 => "killiconmelee",
        8 => "killiconheadshot",
        9 => "killiconcrush",
        11 => "killiconfalling",
        12 => "killiconsuicide",
        15 if cod4 => "killiconimpact",
        _ => "killicondied",
    }
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

/// A name as the kill line draws it: `CG_Obituary` cuts it to 31 bytes, and
/// the engine's line writer (CoDMP.exe 0x409590; 1.5 0x409e60 and UO
/// 0x40a4a0 also `^8`, `^9`) drops every colour code, so the whole name
/// takes its team colour.
pub fn line_name(name: &str) -> String {
    let b = &name.as_bytes()[..name.len().min(31)];
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'^'
            && b.get(i + 1)
                .is_some_and(|&c| super::font::is_colour_code(c as char))
        {
            i += 2;
            continue;
        }
        if b[i] != b'\n' && b[i] != b'\r' {
            out.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
            scale: 0,
            other_entity_num: victim,
            attacker_entity_num: attacker,
            e_type: 0,
            eflags: 0,
        }
    }

    #[test]
    fn icon_prefers_weapon_kill_icon_for_weapon_deaths() {
        let ob = Obituary {
            attacker: Some(1),
            victim: 2,
            weapon: 3,
            mod_: NO_MOD,
        };
        assert_eq!(
            icon_for(&ob, Some("gfx/hud/hud@death_kar98.tga"), false),
            "gfx/hud/hud@death_kar98.tga"
        );
        let ob = Obituary {
            attacker: None,
            victim: 2,
            weapon: 0,
            mod_: 21,
        };
        assert_eq!(icon_for(&ob, None, false), "killIconFalling");
        // An empty `killIcon` is the switch's default.
        let ob = Obituary { mod_: NO_MOD, ..ob };
        assert_eq!(icon_for(&ob, Some(""), false), "killIconDied");
    }

    /// 1.1's ids against UO's: UO's 15 is the drown, 1.1's the airstrike.
    #[test]
    fn each_protocol_reads_its_own_means_of_death() {
        let icon = |m, uo| mod_icon(m, uo);
        assert_eq!(icon(16, false), "killIconDrown");
        assert_eq!(icon(17, false), "killIconSlime");
        assert_eq!(icon(19, false), "killIconCrush");
        assert_eq!(icon(23, false), "killIconDied");
        assert_eq!(icon(13, false), "killIconDied");
        assert_eq!(icon(15, true), "killIconDrown");
        assert_eq!(icon(16, true), "killIconCrush");
        assert_eq!(icon(17, true), "killIconCrushTank");
        assert_eq!(icon(18, true), "killIconCrushJeep");
        assert_eq!(icon(13, true), "gfx/icons/hud@artillery");
        assert_eq!(icon(14, true), "killIconDied");
        assert_eq!(icon(26, true), "gfx/icons/hud@bino_owned");
        assert_eq!(icon(25, true), "killIconDied");
    }

    /// Every icon the switch names has art: a shader script's image or a
    /// file of its own.
    #[test]
    fn every_mod_icon_resolves_to_an_image() {
        let check = |fs: &vcod_common::pk3::Pk3Fs, uo: bool| {
            let shaders = vcod_common::assets::load_shaders(fs);
            for m in 0..32 {
                let name = mod_icon(m, uo);
                let map = shaders.image_map();
                let path = map
                    .get(name)
                    .or_else(|| map.get(&name.to_ascii_lowercase()))
                    .map_or(name, String::as_str);
                assert!(
                    vcod_common::assets::resolve_bundle_image(fs, path).is_some(),
                    "MOD {m} (uo {uo}): {name} -> {path}"
                );
            }
        };
        if let Some(fs) = vcod_common::testing::game_fs_11() {
            check(&fs, false);
        }
        if let Some(fs) = vcod_common::testing::uo_fs() {
            check(&fs, true);
        }
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
        let parm = 0x80 | 21;
        let ob = decode_obituary(&ev(parm, 3, 7));
        assert_eq!(
            ob,
            Obituary {
                attacker: Some(7),
                victim: 3,
                weapon: 0,
                mod_: 21,
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
    fn a_line_name_is_cut_and_loses_its_colour_codes() {
        assert_eq!(line_name("^1Foo^7bar^^9"), "Foobar^^9");
        assert_eq!(line_name(&"x".repeat(40)).len(), 31);
    }

    #[test]
    fn decode_guards_a_negative_attacker_entity_num_before_the_cast() {
        // GameEvent's no-entity sentinel must not wrap through `as u32`.
        assert_eq!(decode_obituary(&ev(5, 3, -1)).attacker, None);
    }
}
