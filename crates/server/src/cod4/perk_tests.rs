//! Every CoD4 1.7 perk on ours (docs/research/cod4-gsc.md, "The perks"):
//! the kill pair's two clients (`combat_ab::ours_set_up`), a perk set on
//! one side or a custom class built off its stats, an act, and the effect
//! asserted. Needs `COD4_DIR`.

use glam::Vec3;

use super::combat_ab::{kill, ours_set_up};
use super::wire_ab::{Probe, side};
use super::*;

fn cs0() -> String {
    let rt = side(&kill("target"));
    String::from_utf8_lossy(rt.gs.configstring(0)).into_owned()
}

/// The view from `ps`'s eye toward `at` as a cmd's angles.
fn aim(ps: &vcod_cod4::pmove::PlayerState, at: Vec3) -> [u16; 3] {
    let eye = crate::game::cod4::combat::view_origin(ps, ps.command_time);
    let d = at - eye;
    let flat = (d.x * d.x + d.y * d.y).sqrt();
    let view = [-d.z.atan2(flat).to_degrees(), d.y.atan2(d.x).to_degrees()];
    let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
    [
        short(view[0]).wrapping_sub(short(ps.delta_angles[0])),
        short(view[1]).wrapping_sub(short(ps.delta_angles[1])),
        0u16.wrapping_sub(short(ps.delta_angles[2])),
    ]
}

/// `slot`'s cmd this tick: viewing `at` (or holding its view), `buttons`,
/// its own weapon and offhand.
fn hold(
    probes: &mut [Probe],
    slot: usize,
    ps: &vcod_cod4::pmove::PlayerState,
    at: Option<Vec3>,
    buttons: u32,
    svs: i32,
) {
    let angles = match at {
        Some(at) => aim(ps, at),
        None => aim(
            ps,
            crate::game::cod4::combat::view_origin(ps, ps.command_time) + Vec3::X,
        ),
    };
    let c = UserCmd {
        server_time: svs + 10,
        buttons,
        weapon: ps.weapon as u8,
        offhand: ps.off_hand_index as u8,
        angles,
        ..Default::default()
    };
    probes[slot].replay.clear();
    probes[slot].replay.push_back((svs, vec![c]));
}

/// A perk's bit (`builtins::cod4_client::PERKS`).
fn bit(name: &str) -> u32 {
    let i = crate::game::builtins::cod4_client::PERKS
        .iter()
        .position(|p| *p == name)
        .unwrap();
    1 << i
}

/// The kill pair under `cvars`, both on the assault class, each side's
/// perks replaced by `perks` (target, shooter) from 47000 on; the shooter
/// holds still, `act` runs each tick from then on with the level, the
/// probes and the time. The server comes back.
fn with_perks(
    cvars: &[(&str, &str)],
    perks: [u32; 2],
    extra: &[(i32, &'static str)],
    act: &mut dyn FnMut(&mut crate::game::cod4::level::Cod4Level, &mut [Probe], i32),
) -> Option<(Vec<Probe>, Vec<String>, Cod4Server)> {
    let install = vcod_cod4::testing::cod4_install()?;
    let mut hook = |sv: &mut Cod4Server, probes: &mut [Probe], svs: i32| {
        if svs < 47_000 {
            return;
        }
        let level = sv.game.level.as_mut().unwrap();
        for (s, p) in perks.iter().enumerate() {
            level.rt.host.cod4_mut().clients[s].perks = *p;
        }
        if let Some(ps) = level.pm_state(1) {
            hold(probes, 1, &ps, None, 0, svs);
        }
        act(level, probes, svs);
    };
    let classes = ["mr {sid} 15 assault_mp,0"; 2];
    Some(ours_set_up(
        &install,
        &cs0(),
        cvars,
        classes,
        None,
        extra,
        &mut hook,
    ))
}

/// The first drop of a health trace: what the first round (the M16 fires
/// a three-round burst, one round a frame) took.
fn first_drop(trace: &[i32]) -> i32 {
    let start = trace[0];
    trace.iter().find(|&&h| h != start).map_or(0, |h| start - h)
}

/// The target's health lost to the first M16 round in its upper torso
/// from the shooter (fired at 47800), each side's perks given; full
/// health 100.
fn one_round(perks: [u32; 2]) -> Option<i32> {
    let mut trace = Vec::new();
    let mut act =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            let (Some(at), Some(ps)) = (level.hit_box_center(0, 4), level.pm_state(1)) else {
                return;
            };
            if (47_450..48_300).contains(&svs) {
                let fire = u32::from((47_800..47_850).contains(&svs));
                hold(probes, 1, &ps, Some(at), 0x800 | fire, svs);
            }
            if (47_700..48_300).contains(&svs) {
                trace.push(level.rt.client_vitals(0).health);
            }
        };
    let (_, aborts, sv) = with_perks(&[("scr_player_maxhealth", "100")], perks, &[], &mut act)?;
    assert!(aborts.is_empty(), "{aborts:?}");
    let hits = sv.game.level.as_ref().unwrap().combat_hits();
    assert!(hits.iter().any(|h| h.0 == 1 && h.1 == 0), "{hits:?}");
    Some(first_drop(&trace))
}

/// Stopping Power (`specialty_bulletdamage`) adds `perk_bulletDamage`'s 40
/// per cent to a bullet, Juggernaut (`specialty_armorvest`) takes
/// `perk_armorVest`'s 75 per cent of it, and the two together leave it as
/// it was (`_class.gsc`'s `cac_modified_damage`).
#[test]
fn stopping_power_and_juggernaut_scale_a_round() {
    let sp = bit("specialty_bulletdamage");
    let jugg = bit("specialty_armorvest");
    let Some(base) = one_round([0, 0]) else {
        return;
    };
    assert!(base > 0 && base < 100, "{base}");
    assert_eq!(one_round([0, sp]), Some(base * 140 / 100));
    assert_eq!(one_round([jugg, 0]), Some((base as f32 * 0.75) as i32));
    assert_eq!(one_round([jugg, sp]), Some(base));
}

/// The target's health lost to an M16 launcher grenade (`gl_m16_mp`, 155
/// inner and 25 outer damage over 300) the shooter bursts about 215 units
/// off its feet, inside the room, at 47450, each side's perks given.
fn one_frag(perks: [u32; 2]) -> Option<i32> {
    let mut before = None;
    let mut after = None;
    let mut act = |level: &mut crate::game::cod4::level::Cod4Level, _: &mut [Probe], svs: i32| {
        let Some(ps) = level.pm_state(0) else {
            return;
        };
        if svs == 47_400 {
            before = Some(level.rt.client_vitals(0).health);
        }
        if svs == 47_450 {
            assert!(level.burst_for_test(
                1,
                "gl_m16_mp",
                ps.origin + Vec3::new(-200.0, -80.0, 10.0)
            ));
        }
        if svs == 48_000 {
            after = Some(level.rt.client_vitals(0).health);
        }
    };
    let (_, aborts, _) = with_perks(&[], perks, &[], &mut act)?;
    assert!(aborts.is_empty(), "{aborts:?}");
    Some(before.unwrap() - after.unwrap())
}

/// Sonic Boom (`specialty_explosivedamage`) adds `perk_explosiveDamage`'s
/// 25 per cent to a blast; Juggernaut cancels it.
#[test]
fn sonic_boom_scales_a_blast() {
    let boom = bit("specialty_explosivedamage");
    let jugg = bit("specialty_armorvest");
    let Some(base) = one_frag([0, 0]) else {
        return;
    };
    assert!(base > 0 && base * 125 / 100 < 100, "{base}");
    assert_eq!(one_frag([0, boom]), Some(base * 125 / 100));
    assert_eq!(one_frag([jugg, boom]), Some(base));
}

/// The kill pair's one round at 47800 into the target's chest (health 30,
/// one round kills), the target's perks given; each tick's level after
/// it goes to `watch`.
fn shot_dead(
    target_perks: u32,
    watch: &mut dyn FnMut(&mut crate::game::cod4::level::Cod4Level, &mut [Probe], i32),
) -> Option<(Vec<Probe>, Vec<String>)> {
    let mut act =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            if (47_450..48_300).contains(&svs)
                && let (Some(at), Some(ps)) = (level.hit_box_center(0, 4), level.pm_state(1))
            {
                let fire = u32::from((47_800..47_850).contains(&svs));
                hold(probes, 1, &ps, Some(at), 0x800 | fire, svs);
            }
            if svs >= 47_800 {
                watch(level, probes, svs);
            }
        };
    let (probes, aborts, _) = with_perks(&[], [target_perks, 0x4400], &[], &mut act)?;
    Some((probes, aborts))
}

/// Martyrdom (`specialty_grenadepulldeath`): `player_die` drops a live
/// `perk_grenadeDeath` (`frag_grenade_short_mp`) at the body; without it
/// no grenade comes.
#[test]
fn martyrdom_drops_a_live_grenade() {
    let martyr = bit("specialty_grenadepulldeath");
    let seen = |perks: u32| {
        let mut found = Vec::new();
        let mut watch =
            |level: &mut crate::game::cod4::level::Cod4Level, _: &mut [Probe], _: i32| {
                let c4 = level.rt.host.cod4();
                for m in &c4.missiles {
                    let name = c4.table.defs.get(m.weapon as usize).map(|d| d.name.clone());
                    if m.owner == Some(0) && !found.contains(&name) {
                        found.push(name);
                    }
                }
            };
        let (_, aborts) = shot_dead(perks, &mut watch)?;
        assert!(aborts.is_empty(), "{aborts:?}");
        Some(found)
    };
    let Some(with) = seen(martyr) else {
        return;
    };
    assert_eq!(with, vec![Some("frag_grenade_short_mp".to_string())]);
    assert_eq!(seen(0), Some(vec![]));
}

/// Last Stand (`specialty_pistoldeath`): the killing round puts the target
/// down (`PM_LASTSTAND`, 6) with a pistol instead of killing it, and
/// `lastStandTimer`'s 10 s later it bleeds out (`PM_DEAD`, 7).
#[test]
fn last_stand_goes_down_then_bleeds_out() {
    let mut rows = Vec::new();
    let mut watch =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            if let Some(ps) = level.pm_state(0) {
                // Down, the client follows the script's `switchToWeapon`
                // (`a <weapon>`) to the pistol.
                if ps.pm_type == 6 {
                    let pistol = level.rt.host.cod4().table.index("beretta_mp").unwrap();
                    let mut cmd_ps = ps.clone();
                    cmd_ps.weapon = pistol as u32;
                    hold(probes, 0, &cmd_ps, None, 0, svs);
                }
                let name = level
                    .rt
                    .host
                    .cod4()
                    .table
                    .defs
                    .get(ps.weapon as usize)
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
                rows.push((svs, ps.pm_type, level.rt.client_vitals(0).health, name));
            }
        };
    let Some((_, aborts)) = shot_dead(bit("specialty_pistoldeath"), &mut watch) else {
        return;
    };
    assert!(aborts.is_empty(), "{aborts:?}");
    let down = rows.iter().find(|r| r.1 == 6).cloned();
    let (t, _, health, _) = down.clone().unwrap_or_else(|| panic!("{rows:?}"));
    assert!(health > 0, "{down:?}");
    // The pistol is in hand once its raise is over.
    let armed = rows.iter().find(|r| r.0 > t + 1000 && r.1 == 6).unwrap();
    assert_eq!(armed.3, "beretta_mp", "{armed:?}");
    let dead = rows
        .iter()
        .find(|r| r.1 == 7)
        .unwrap_or_else(|| panic!("{rows:?}"));
    assert!(
        (10_000..11_500).contains(&(dead.0 - t)),
        "{} after {t}",
        dead.0
    );
}

/// A weapon's name, ammo slot and clip slot.
type WeaponSlots = (String, usize, usize);

/// The shooter's M16 reload in frames (its `weaponstate` reloading) from
/// a reload key at 48500 after a burst at 47800, its perks given.
fn reload_frames(perks: u32) -> Option<usize> {
    let mut states = Vec::new();
    let mut act =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            let Some(ps) = level.pm_state(1) else {
                return;
            };
            // A burst first, so the magazine is not full.
            let buttons = if (47_800..47_850).contains(&svs) {
                1
            } else if (48_500..48_550).contains(&svs) {
                vcod_cod4::pmove::weapon::RELOAD_BUTTON
            } else {
                0
            };
            hold(probes, 1, &ps, None, buttons, svs);
            if svs >= 48_500 {
                states.push(ps.weapon_state);
            }
        };
    let (_, aborts, _) = with_perks(&[], [0, perks], &[], &mut act)?;
    assert!(aborts.is_empty(), "{aborts:?}");
    let reloading = vcod_cod4::pmove::weapon::state::RELOAD;
    Some(states.iter().filter(|&&s| s == reloading).count())
}

/// Sleight of Hand (`specialty_fastreload`) runs the reload's clock at
/// twice the rate (`perk_weapReloadMultiplier` 0.5, `PM_Weapon`'s
/// timers).
#[test]
fn sleight_of_hand_halves_the_reload() {
    let Some(plain) = reload_frames(0) else {
        return;
    };
    let fast = reload_frames(bit("specialty_fastreload")).unwrap();
    assert!(plain > 10, "{plain}");
    assert!(fast.abs_diff(plain / 2) <= 1, "{fast} of {plain}");
}

/// `mp/statsTable.csv`'s number (column 0) of the row whose column `col`
/// reads `value`.
pub(super) fn stat_row(
    level: &crate::game::cod4::level::Cod4Level,
    col: usize,
    value: &str,
) -> u32 {
    let t = &level.rt.host.cod4().string_tables["mp/statstable.csv"];
    let cols = t.columns as usize;
    (0..t.rows as usize)
        .find(|r| t.values[r * cols + col].as_deref() == Some(value))
        .and_then(|r| t.values[r * cols].as_deref()?.parse().ok())
        .unwrap_or_else(|| panic!("no statsTable row {value}"))
}

/// What a custom class (`custom1`, stats 201..209, `_class.gsc`'s
/// `cac_getdata`) gave the target once spawned: its playerstate and the
/// level's weapon names by number. `secondary` names the second weapon,
/// `perks` the three specialties, `special` the special grenade.
fn custom_class(
    secondary: &str,
    perks: [&str; 3],
    special: &str,
) -> Option<(vcod_cod4::pmove::PlayerState, Vec<WeaponSlots>)> {
    let install = vcod_cod4::testing::cod4_install()?;
    let mut out = None;
    let (secondary, special) = (secondary.to_string(), special.to_string());
    let mut hook = |sv: &mut Cod4Server, _: &mut [Probe], svs: i32| {
        let level = sv.game.level.as_mut().unwrap();
        if (44_000..45_850).contains(&svs) {
            let vals = [
                (201, stat_row(level, 4, "m16")),
                (202, 0),
                (203, stat_row(level, 4, &secondary)),
                (204, 0),
                (205, stat_row(level, 6, perks[0])),
                (206, stat_row(level, 6, perks[1])),
                (207, stat_row(level, 6, perks[2])),
                (208, stat_row(level, 4, &special)),
                (209, 0),
            ];
            let st = &mut level.rt.host.cod4_mut().stats[0];
            for (k, v) in vals {
                st.set(k, v);
            }
        }
        if svs == 47_500 {
            let names = level
                .rt
                .host
                .cod4()
                .table
                .defs
                .iter()
                .map(|d| (d.name.clone(), d.ammo_slot, d.clip_slot))
                .collect();
            out = level.pm_state(0).map(|ps| (ps, names));
        }
    };
    let classes = ["mr {sid} 15 custom1,0", "mr {sid} 15 assault_mp,0"];
    let (_, aborts, _) = ours_set_up(&install, &cs0(), &[], classes, None, &[], &mut hook);
    assert!(aborts.is_empty(), "{aborts:?}");
    out
}

/// A weapon's number, its clip and its reserve on `ps`, `None` when it
/// does not hold it.
fn holds(
    ps: &vcod_cod4::pmove::PlayerState,
    defs: &[WeaponSlots],
    weapon: &str,
) -> Option<(i32, i32)> {
    let w = defs.iter().position(|d| d.0 == weapon)?;
    (ps.weapons[w / 32] & (1 << (w % 32)) != 0)
        .then(|| (ps.ammoclip[defs[w].2], ps.ammo[defs[w].1]))
}

/// Bandolier (`specialty_extraammo`) fills the primary's and the
/// pistol's reserves (`giveMaxAmmo`); without it the M16 starts on its
/// start ammo.
#[test]
fn bandolier_fills_the_reserves() {
    let Some((ps, defs)) = custom_class(
        "beretta",
        [
            "specialty_extraammo",
            "specialty_bulletdamage",
            "specialty_longersprint",
        ],
        "flash_grenade",
    ) else {
        return;
    };
    let full = holds(&ps, &defs, "m16_mp").unwrap();
    let (plain, defs2) = custom_class(
        "beretta",
        [
            "specialty_fraggrenade",
            "specialty_bulletdamage",
            "specialty_longersprint",
        ],
        "flash_grenade",
    )
    .unwrap();
    let start = holds(&plain, &defs2, "m16_mp").unwrap();
    assert!(full.1 > start.1, "{full:?} {start:?}");
    // The frag perk's three frags, the special grenade's one.
    assert_eq!(
        holds(&plain, &defs2, "frag_grenade_mp").map(|h| h.0),
        Some(3)
    );
    assert_eq!(
        holds(&plain, &defs2, "flash_grenade_mp").map(|h| h.0),
        Some(1)
    );
    assert_eq!(holds(&ps, &defs, "frag_grenade_mp").map(|h| h.0), Some(1));
}

/// Special Grenades x3 (`specialty_specialgrenade`) give three of the
/// class's special grenade.
#[test]
fn special_grenades_come_three() {
    let Some((ps, defs)) = custom_class(
        "beretta",
        [
            "specialty_specialgrenade",
            "specialty_bulletdamage",
            "specialty_longersprint",
        ],
        "concussion_grenade",
    ) else {
        return;
    };
    assert_eq!(
        holds(&ps, &defs, "concussion_grenade_mp").map(|h| h.0),
        Some(3)
    );
}

/// Overkill (`specialty_twoprimaries`) keeps a primary as the second
/// weapon; without it that slot falls back to the pistol.
#[test]
fn overkill_carries_two_primaries() {
    let Some((ps, defs)) = custom_class(
        "ak47",
        [
            "specialty_fraggrenade",
            "specialty_twoprimaries",
            "specialty_longersprint",
        ],
        "flash_grenade",
    ) else {
        return;
    };
    assert!(holds(&ps, &defs, "ak47_mp").is_some());
    assert!(holds(&ps, &defs, "m16_mp").is_some());
    let (ps, defs) = custom_class(
        "ak47",
        [
            "specialty_fraggrenade",
            "specialty_bulletdamage",
            "specialty_longersprint",
        ],
        "flash_grenade",
    )
    .unwrap();
    assert!(holds(&ps, &defs, "ak47_mp").is_none());
}

/// C4, Claymore and RPG x2 (`specialty_weapon_*`) each put their weapon
/// in the inventory slot with two rounds, and the class's perks reach
/// the playerstate's bits (`setPerk`).
#[test]
fn the_inventory_perks_give_two_of_their_weapon() {
    for (perk, weapon) in [
        ("specialty_weapon_c4", "c4_mp"),
        ("specialty_weapon_claymore", "claymore_mp"),
        ("specialty_weapon_rpg", "rpg_mp"),
    ] {
        let Some((ps, defs)) = custom_class(
            "beretta",
            [perk, "specialty_armorvest", "specialty_bulletpenetration"],
            "flash_grenade",
        ) else {
            return;
        };
        let (clip, reserve) = holds(&ps, &defs, weapon).unwrap_or_else(|| panic!("{weapon}"));
        assert_eq!(clip + reserve, 2, "{weapon}: {clip} {reserve}");
        let want = bit("specialty_armorvest") | bit("specialty_bulletpenetration");
        assert_eq!(ps.perks & want, want, "{:#x}", ps.perks);
    }
}
/// The target's health lost to the first M16 round fired through the
/// room's north wall on mp_crash (there a metal plate, surface 13, 4.25
/// thick at y 2331.875 to 2336.125): the shooter inside at (207, 2250),
/// the target outside at (207, 2380), both on the floor at 236 (a
/// `setviewpos` names the eye); full health 100.
fn through_the_wall(cvars: &[(&str, &str)], shooter_perks: u32) -> Option<i32> {
    let mut trace = Vec::new();
    let mut act =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            if svs == 47_100 || svs == 47_600 {
                level.set_view_pos(0, [207.0, 2380.0, 296.0], [0.0, -90.0, 0.0]);
                level.set_view_pos(1, [207.0, 2250.0, 296.0], [0.0, 90.0, 0.0]);
            }
            let (Some(at), Some(ps)) = (level.hit_box_center(0, 4), level.pm_state(1)) else {
                return;
            };
            if (47_650..48_300).contains(&svs) {
                let fire = u32::from((47_800..47_850).contains(&svs));
                hold(probes, 1, &ps, Some(at), 0x800 | fire, svs);
            }
            if (47_700..48_300).contains(&svs) {
                trace.push(level.rt.client_vitals(0).health);
            }
        };
    let mut cv = vec![("scr_player_maxhealth", "100")];
    cv.extend_from_slice(cvars);
    let (_, aborts, _) = with_perks(&cv, [0, shooter_perks], &[], &mut act)?;
    assert!(aborts.is_empty(), "{aborts:?}");
    Some(first_drop(&trace))
}

/// An M16 round (`penetrateType` medium, 20 into metal) through the plate
/// (4.27 along the round) keeps `1 - 4.27 / 20` of its 40, 31; Deep
/// Impact (`specialty_bulletpenetration`) doubles the depth
/// (`perk_bulletPenetrationMultiplier` 2), `1 - 4.27 / 40`, 35; with
/// `bullet_penetrationEnabled` 0 the wall stops it.
#[test]
fn a_round_goes_through_a_wall_and_deep_impact_goes_further() {
    let Some(plain) = through_the_wall(&[], 0) else {
        return;
    };
    assert_eq!(plain, 31);
    let deep = through_the_wall(&[], bit("specialty_bulletpenetration")).unwrap();
    assert_eq!(deep, 35);
    let off = through_the_wall(&[("bullet_penetrationEnabled", "0")], 0).unwrap();
    assert_eq!(off, 0);
    // The assault class's Stopping Power on top, `int(31 * 140 / 100)`:
    // what retail's first round took through the same plate
    // (`play-1.7-wall-target.txt`, cod4-combat.md 3.1).
    let retail = side(&super::wire_ab::retail("wall-target"));
    let health: Vec<i32> = retail
        .snaps
        .iter()
        .filter(|f| f.ps.get("pm_type") == Some(0) && f.ps.stats[2] == 100)
        .map(|f| f.ps.stats[0])
        .collect();
    let retail_first = first_drop(&health[health.iter().position(|&h| h == 100).unwrap()..]);
    assert_eq!(retail_first, 43, "{health:?}");
    let assault = bit("specialty_bulletdamage") | bit("specialty_longersprint");
    assert_eq!(through_the_wall(&[], assault), Some(retail_first));
}

/// How far the shooter's first M16 round fired from the hip at 47800
/// strays off its aim (the spread's disc at the round's range), its perks
/// given.
fn hip_stray(perks: u32) -> Option<f32> {
    let mut act =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            let (Some(at), Some(ps)) = (level.hit_box_center(0, 4), level.pm_state(1)) else {
                return;
            };
            if (47_450..48_300).contains(&svs) {
                let fire = u32::from((47_800..47_850).contains(&svs));
                hold(probes, 1, &ps, Some(at), fire, svs);
            }
        };
    let (_, aborts, sv) = with_perks(&[], [0, perks], &[], &mut act)?;
    assert!(aborts.is_empty(), "{aborts:?}");
    let level = sv.game.level.as_ref().unwrap();
    let &(_, _, _, aimed, end) = level
        .combat_shots()
        .iter()
        .find(|s| s.0 == 1)
        .expect("the shooter fired");
    Some(aimed.distance(end))
}

/// Steady Aim (`specialty_bulletaccuracy`) narrows the hip spread to
/// `perk_weapSpreadMultiplier`'s 0.65 (`BG_GetSpreadForWeapon`): the same
/// round, drawn off the same disc point, strays 0.65 as far.
#[test]
fn steady_aim_narrows_the_hip_spread() {
    let Some(plain) = hip_stray(0) else {
        return;
    };
    let steady = hip_stray(bit("specialty_bulletaccuracy")).unwrap();
    assert!(plain > 1.0, "{plain}");
    assert!((steady / plain - 0.65).abs() < 1e-3, "{steady} of {plain}");
}

/// A custom class (`custom1`, as [`custom_class`]) with `primary` and
/// `perks` on the target, `act` each tick from 47000 with the level, the
/// probes and the time.
fn custom_run(
    primary: &str,
    perks: [&str; 3],
    act: &mut dyn FnMut(&mut crate::game::cod4::level::Cod4Level, &mut [Probe], i32),
) -> Option<Vec<String>> {
    let install = vcod_cod4::testing::cod4_install()?;
    let primary = primary.to_string();
    let mut hook = |sv: &mut Cod4Server, probes: &mut [Probe], svs: i32| {
        let level = sv.game.level.as_mut().unwrap();
        if (44_000..45_850).contains(&svs) {
            let vals = [
                (201, stat_row(level, 4, &primary)),
                (202, 0),
                (203, stat_row(level, 4, "beretta")),
                (204, 0),
                (205, stat_row(level, 6, perks[0])),
                (206, stat_row(level, 6, perks[1])),
                (207, stat_row(level, 6, perks[2])),
                (208, stat_row(level, 4, "flash_grenade")),
                (209, 0),
            ];
            let st = &mut level.rt.host.cod4_mut().stats[0];
            for (k, v) in vals {
                st.set(k, v);
            }
        }
        if svs >= 47_000 {
            if let Some(ps) = level.pm_state(1) {
                hold(probes, 1, &ps, None, 0, svs);
            }
            act(level, probes, svs);
        }
    };
    let classes = ["mr {sid} 15 custom1,0", "mr {sid} 15 assault_mp,0"];
    let (_, aborts, _) = ours_set_up(&install, &cs0(), &[], classes, None, &[], &mut hook);
    Some(aborts)
}

/// The target's AK-47 rounds fired holding the trigger from 47500 to
/// 49000 (short of the 30-round magazine either way), its second perk given (the others Frag x3 and Extreme
/// Conditioning).
fn ak_rounds(perk: &str) -> Option<usize> {
    let mut clips = Vec::new();
    let mut act =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            let Some(ps) = level.pm_state(0) else {
                return;
            };
            let fire = u32::from((47_500..49_000).contains(&svs));
            hold(probes, 0, &ps, None, fire, svs);
            // Three cmds a tick, as a client at 60 frames a second sends:
            // one a tick steps the fire clock 50 ms at a time and fires on
            // every other cmd either way.
            if let Some((_, cmds)) = probes[0].replay.back_mut() {
                let c = cmds[0];
                *cmds = [-34, -17, 0]
                    .map(|dt| UserCmd {
                        server_time: c.server_time + dt,
                        ..c
                    })
                    .to_vec();
            }
            if (47_400..49_800).contains(&svs) {
                let defs = &level.rt.host.cod4().table.defs;
                let ak = defs.iter().position(|d| d.name == "ak47_mp").unwrap();
                clips.push(ps.ammoclip[defs[ak].clip_slot]);
            }
        };
    let aborts = custom_run(
        "ak47",
        ["specialty_fraggrenade", perk, "specialty_longersprint"],
        &mut act,
    )?;
    assert!(aborts.is_empty(), "{aborts:?}");
    Some((clips[0] - clips.iter().min().unwrap()) as usize)
}

/// Double Tap (`specialty_rof`) runs the fire clock at `1 / 0.75`
/// (`perk_weapRateMultiplier`, `PM_Weapon`'s timers). The clock drops what
/// a cmd overshoots, so the AK-47's 85 ms round takes six of these 16 and
/// 17 ms cmds plain (100 ms) and four under the perk (each step scaled to
/// 21 or 23): over the 1.5 s on the trigger, 15 rounds against 23, the
/// rate retail's `cod4_fire_rate_ab` captures fire at (cod4-combat.md 2.1).
#[test]
fn double_tap_fires_faster() {
    let Some(plain) = ak_rounds("specialty_armorvest") else {
        return;
    };
    let fast = ak_rounds("specialty_rof").unwrap();
    assert_eq!((plain, fast), (15, 23));
}

/// The ticks of the target's first breath held on a scoped M40A3,
/// sighted from 47000, the breath key held from 48500, its third perk
/// given.
fn breath_ticks(perk: &str) -> Option<usize> {
    let mut held = 0;
    let mut done = false;
    let mut act =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            let Some(ps) = level.pm_state(0) else {
                return;
            };
            let breath = if svs >= 48_500 { 0x2000 } else { 0 };
            hold(probes, 0, &ps, None, 0x800 | breath, svs);
            // `WEAPFLAG_HOLDING_BREATH`.
            if ps.weap_flags & 4 != 0 && !done {
                held += 1;
            } else if held > 0 {
                done = true;
            }
        };
    let aborts = custom_run(
        "m40a3",
        ["specialty_fraggrenade", "specialty_armorvest", perk],
        &mut act,
    )?;
    assert!(aborts.is_empty(), "{aborts:?}");
    Some(held)
}

/// Iron Lungs (`specialty_holdbreath`) holds the breath
/// `perk_extraBreath`'s 5 s past `player_breath_hold_time`'s 4.5
/// (`PM_UpdateHoldBreath`).
#[test]
fn iron_lungs_holds_the_breath_longer() {
    let Some(plain) = breath_ticks("specialty_longersprint") else {
        return;
    };
    let long = breath_ticks("specialty_holdbreath").unwrap();
    assert!(plain.abs_diff(90) <= 1, "{plain}");
    assert!(long.abs_diff(190) <= 1, "{long}");
}

/// The shooter's ticks sprinting, the sprint and forward keys held from
/// 47500, its perks given.
fn sprint_ticks(perks: u32) -> Option<usize> {
    let mut ticks = 0;
    let mut act =
        |level: &mut crate::game::cod4::level::Cod4Level, probes: &mut [Probe], svs: i32| {
            let Some(ps) = level.pm_state(1) else {
                return;
            };
            if svs < 47_500 {
                return;
            }
            hold(probes, 1, &ps, None, vcod_cod4::pmove::button::SPRINT, svs);
            if let Some((_, cmds)) = probes[1].replay.back_mut() {
                cmds[0].forwardmove = 127;
            }
            if ps.pm_flags & vcod_cod4::pmove::pmf::SPRINTING != 0 {
                ticks += 1;
            }
        };
    let (_, aborts, _) = with_perks(&[], [0, perks], &[(14_000, "score")], &mut act)?;
    assert!(aborts.is_empty(), "{aborts:?}");
    Some(ticks)
}

/// Extreme Conditioning (`specialty_longersprint`) doubles the sprint
/// (`perk_sprintMultiplier` 2 over `player_sprintTime`'s 4 s, times the
/// M16's `sprintDurationScale`, `BG_GetMaxSprintTime`).
#[test]
fn extreme_conditioning_doubles_the_sprint() {
    let Some(plain) = sprint_ticks(0) else {
        return;
    };
    let long = sprint_ticks(bit("specialty_longersprint")).unwrap();
    assert!(plain > 20, "{plain}");
    assert!(long.abs_diff(plain * 2) <= 1, "{long} of {plain}");
}
