//! The flashbang's and the concussion grenade's shellshocks on ours, over
//! the kill pair's join (`combat_ab::ours_hooked`): a grenade bursts in
//! front of the target, the stock scripts (`_flashgrenades.gsc`'s
//! `monitorFlash`, `_weapons.gsc`'s `onWeaponDamage`) answer with
//! `shellShock`, and the target's playerstate carries it to the client
//! (docs/research/cod4-combat.md 10.8). Needs `COD4_DIR`.

use glam::Vec3;

use super::combat_ab::{kill, ours_hooked};
use super::wire_ab::{Probe, side};
use super::*;

/// A snapshot's `(time, shellshockIndex, shellshockDuration, pm_flags,
/// torsoAnim, torsoTimer)`.
type Row = (i32, u32, u32, u32, u32, u32);

/// When the grenade bursts, past both spawns and the prematch.
const BURST_MS: i32 = 47_450;

/// `weapon` bursts `ahead` units in front of the target's eye at
/// [`BURST_MS`], thrown by the shooter (who stands still from its spawn
/// on); the target's snapshots from then on, as `(time, shellshockIndex,
/// shellshockDuration, pm_flags, torsoAnim, torsoTimer)`, the name of the
/// shellshock slot the first shocked one names and the player animtree's
/// names by wire index.
fn shocked(weapon: &str, ahead: f32) -> Option<(Vec<Row>, String, Vec<String>)> {
    let install = vcod_cod4::testing::cod4_install()?;
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let mut hook = |sv: &mut Cod4Server, probes: &mut [Probe], svs: i32| {
        let level = sv.game.level.as_mut().unwrap();
        if svs >= 46_900
            && let Some(ps) = level.pm_state(1)
        {
            // The shooter holds still: its own view, no keys.
            let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
            let c = UserCmd {
                server_time: svs + 10,
                weapon: 27,
                offhand: 40,
                angles: std::array::from_fn(|k| {
                    short(ps.view_angles[k]).wrapping_sub(short(ps.delta_angles[k]))
                }),
                ..Default::default()
            };
            probes[1].replay.clear();
            probes[1].replay.push_back((svs, vec![c]));
        }
        if svs == BURST_MS
            && let Some(ps) = level.pm_state(0)
        {
            let [forward, _, _] = vcod_common::x87::angle_vectors(Vec3::from(ps.view_angles));
            let eye = ps.origin + Vec3::Z * ps.view_height_current;
            assert!(level.burst_for_test(1, weapon, eye + forward * ahead));
        }
    };
    let (probes, aborts, sv) = ours_hooked(&install, &cs0, None, &[], &mut hook);
    let names = sv
        .game
        .level
        .as_ref()
        .unwrap()
        .rt
        .host
        .cod4()
        .anim_names
        .to_vec();
    assert!(aborts.is_empty(), "{aborts:?}");
    let ot = side(&probes[0].lines);
    let rows: Vec<_> = ot
        .snaps
        .iter()
        .filter(|f| f.server_time >= BURST_MS)
        .map(|f| {
            let u = |n: &str| f.ps.get(n).unwrap_or(0);
            (
                f.server_time,
                u("shellshockIndex"),
                u("shellshockDuration"),
                u("pm_flags"),
                u("torsoAnim"),
                u("torsoTimer"),
            )
        })
        .collect();
    let index = rows.iter().map(|r| r.1).find(|&i| i != 0).unwrap_or(0);
    let name = if index == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(ot.gs.configstring(1954 + index as u16)).into_owned()
    };
    Some((rows, name, names))
}

/// A flashbang bursting in the target's face, inside `explosionRadiusMin`
/// (200): `flashbang` reads distance 1 and angle 1, so `monitorFlash`'s
/// duration is 1 * 1 * 6 s, and a frame on (`applyFlash`'s `wait .05`) the
/// target carries the `flashbang` shellshock for 6000 ms with
/// `PMF_SHELLSHOCKED`.
#[test]
fn a_flashbang_in_the_face_shocks_for_six_seconds() {
    let Some((rows, name, _)) = shocked("flash_grenade_mp", 40.0) else {
        return;
    };
    assert_eq!(name, "flashbang", "{rows:?}");
    let first = rows.iter().find(|r| r.1 != 0).unwrap();
    assert_eq!(first.2, 6000, "{rows:?}");
    assert!(first.3 & 0x10000 != 0, "{rows:?}");
    // The flag goes once the shock has run out.
    let end = first.0 + 6000;
    assert!(
        rows.iter()
            .filter(|r| r.0 > end + 100)
            .all(|r| r.3 & 0x10000 == 0),
        "{rows:?}"
    );
}

/// A concussion grenade's one point of splash runs `onWeaponDamage`: its
/// `concussion_grenade_mp` shellshock lasts `2 + 4 * (1 - distance / 512)`
/// seconds from the target's feet to the grenade.
#[test]
fn a_concussion_grenade_shocks_by_its_distance() {
    let Some((rows, name, _)) = shocked("concussion_grenade_mp", 40.0) else {
        return;
    };
    assert_eq!(name, "concussion_grenade_mp", "{rows:?}");
    let first = rows.iter().find(|r| r.1 != 0).unwrap();
    // The eye is 60 up and the burst 40 out: about 72 units from the feet.
    let want = (2.0 + 4.0 * (1.0 - 72.1 / 512.0)) * 1000.0;
    assert!((first.2 as f32 - want).abs() < 30.0, "{rows:?}");
    assert!(first.3 & 0x10000 != 0, "{rows:?}");
}

/// `shellShock` on a live player runs the `shellshock` anim event
/// (`ANIM_ET_SHELLSHOCK`, 0x80c4bab): the standing target's torso plays
/// `pb_stand_shellshock` for the clause's 5000 ms (cod4-combat.md 10.8).
#[test]
fn a_shellshock_plays_the_torso_shock_anim() {
    let Some((rows, _, names)) = shocked("flash_grenade_mp", 40.0) else {
        return;
    };
    let first = rows.iter().find(|r| r.1 != 0).unwrap();
    let anim = names
        .get((first.4 & 0x1ff) as usize)
        .cloned()
        .unwrap_or_default();
    assert_eq!(anim, "pb_stand_shellshock", "{rows:?}");
    assert!((4900..=5050).contains(&first.5), "{rows:?}");
}
