//! Live pins of the mount, the gunner's frame, the release and the cursor
//! hint's pick on mp_backlot's crouch SAW 393 (cod4-gsc.md 9.1, 9.2).
//! Each test loads the level; all need `COD4_DIR`.

use std::time::{Duration, Instant};

use glam::Vec3;
use vcod_cod4::net::clmsg::UserCmd;
use vcod_gsc::{EntId, Host};

use super::level::{Cod4Level, PsExt};
use crate::cod4::{Cod4Config, Cod4Game, Cod4Server};
use crate::engine::GameModule;

/// mp_backlot's upstairs crouch SAW.
const GUN: u32 = 393;
/// The retail capture's `setviewpos` behind it (cod4-gsc.md 9.2).
const BEHIND: [f32; 3] = [-770.0, 68.0, 320.0];

struct Run {
    sv: Cod4Server,
    now: Instant,
    slots: Vec<usize>,
}

impl Run {
    /// mp_backlot `war`.
    fn backlot(bots: usize) -> Option<Run> {
        Run::start("mp_backlot", "war", bots)
    }

    /// `map` and `gametype` under `sv_cheats` with `bots` bots in play,
    /// taken off bot control.
    fn start(map: &str, gametype: &str, bots: usize) -> Option<Run> {
        let install = vcod_cod4::testing::cod4_install()?;
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: map.into(),
                gametype: gametype.into(),
                hostname: "pins".into(),
                max_clients: 8,
            },
            3,
        );
        sv.set_cvar("sv_pure", "0");
        sv.set_cvar("sv_cheats", "1");
        sv.load(Some(&install)).unwrap();
        let slots = sv.add_bots(bots, false);
        sv.bots_mut().unwrap().wait_for_graph();
        let mut run = Run {
            sv,
            now: Instant::now(),
            slots,
        };
        for _ in 0..1500 {
            run.tick();
            let all = run.slots.clone().into_iter().all(|s| {
                let level = run.level();
                level.rt.client_field(s, "sessionstate").as_deref() == Some("playing")
                    && level
                        .pm_state(s)
                        .is_some_and(|ps| ps.pm_flags & vcod_cod4::pmove::pmf::FROZEN == 0)
            });
            if all {
                break;
            }
        }
        for s in run.slots.clone() {
            run.sv.bots_mut().unwrap().remove(s);
        }
        for _ in 0..4 {
            run.tick();
        }
        Some(run)
    }

    fn tick(&mut self) {
        self.now += Duration::from_millis(50);
        Cod4Game::drain_console(&mut self.sv, self.now);
        self.sv.tick_frames(self.now, 1);
        self.sv.take_outgoing();
    }

    fn level(&mut self) -> &mut Cod4Level {
        self.sv.game.level.as_mut().unwrap()
    }

    fn gun(&mut self) -> EntId {
        self.level().rt.host.ents.handle(GUN).expect("SAW 393")
    }

    /// One cmd from `slot` looking at `view` (pitch, yaw) with `buttons`.
    fn think(&mut self, slot: usize, view: [f32; 2], buttons: u32) {
        let level = self.level();
        let ps = level.pm_state(slot).unwrap();
        let short = |a: f32, d: f32| ((a - d) * 65536.0 / 360.0) as i32 as u16;
        let cmd = UserCmd {
            server_time: level.level_time() + 50,
            buttons,
            angles: [
                short(view[0], ps.delta_angles[0]),
                short(view[1], ps.delta_angles[1]),
                0,
            ],
            weapon: ps.weapon as u8,
            offhand: ps.off_hand_index as u8,
            ..Default::default()
        };
        level.client_think(slot, &cmd);
    }

    /// `cursorHint`, `cursorHintString`, `cursorHintEntIndex`.
    fn hint(&mut self, slot: usize) -> (u32, u32, u32) {
        let p = self.level().players[slot].as_ref().unwrap();
        (
            p.ps.word("cursorHint"),
            p.ps.word("cursorHintString"),
            p.ps.word("cursorHintEntIndex"),
        )
    }

    /// Drops `slot`'s gun where it stands (`dropWeaponForDeath`) and
    /// waits for it to come to rest; its entity number.
    fn drop_gun(&mut self, slot: usize, at: [f32; 3]) -> u32 {
        self.level().set_view_pos(slot, at, [0.0; 3]);
        self.tick();
        self.level()
            .start_player_script(
                slot,
                "maps/mp/gametypes/_weapons",
                "dropweaponfordeath",
                &["x"],
            )
            .unwrap();
        for _ in 0..60 {
            self.tick();
        }
        let items = &self.level().rt.host.cod4().items;
        let item = items.iter().find(|i| i.resting()).expect("a resting drop");
        item.id.0
    }

    /// The gunner's and the gun's state the turret code writes.
    fn mount_state(&mut self, slot: usize) -> String {
        let gun = self.gun();
        let level = self.level();
        let rec = &level.rt.host.turrets[&gun];
        let rec = format!(
            "owner {:?} busy {} barrel {:?} teleport {} firing {} stance {:?} spot {:?}",
            rec.owner,
            rec.busy,
            rec.angles2,
            rec.teleport_bit,
            rec.firing,
            rec.mount_stance,
            rec.mount_origin,
        );
        let p = level.players[slot].as_ref().unwrap();
        let w = |k: &str| p.ps.word(k);
        let f = |k: &str| f32::from_bits(p.ps.word(k));
        format!(
            "{rec}; active {} eFlags {:#x} pm_flags {:#x} viewlocked {} {} \
             clamp {:?} {:?} {:?} {:?} origin {:?} {:?} {:?} view {:?} hint {:?}",
            p.turret_active,
            w("eFlags") & !0x2,
            w("pm_flags"),
            w("viewlocked"),
            w("viewlocked_entNum"),
            f("viewAngleClampBase[0]"),
            f("viewAngleClampRange[0]"),
            f("viewAngleClampBase[1]"),
            f("viewAngleClampRange[1]"),
            f("origin[0]"),
            f("origin[1]"),
            f("origin[2]"),
            f("viewangles[1]"),
            (
                w("cursorHint"),
                w("cursorHintString"),
                w("cursorHintEntIndex")
            ),
        )
    }
}

/// Behind the gun the hint is the gun; a use press with the view 70 past
/// the yaw arc and 40 above the pitch arc mans it with the barrel on the
/// arcs' edges and the drop hint up; a death lets the gun go on the death
/// frame (`turret_think_client`, 0x80b7a40) and the respawn stays at its
/// spawn point.
#[test]
fn the_saw_mounts_at_its_arc_limits_and_lets_go_on_death() {
    let Some(mut run) = Run::backlot(1) else {
        return;
    };
    let slot = run.slots[0];
    run.level().set_view_pos(slot, BEHIND, [0.0; 3]);
    run.tick();
    run.tick();
    assert_eq!(run.hint(slot), (12, 0, GUN));
    let view = [-40.0, 70.0];
    for _ in 0..10 {
        run.think(slot, view, 0x8);
        run.tick();
        if run.level().manned_gun(slot).is_some() {
            break;
        }
    }
    let gun = run.gun();
    assert_eq!(run.level().manned_gun(slot), Some(gun));
    let mounted = run.mount_state(slot);
    run.think(slot, view, 0);
    run.tick();
    let tracked = run.mount_state(slot);
    // The trigger held three frames, then let go.
    let mut fire = Vec::new();
    for buttons in [1, 1, 1, 0] {
        run.think(slot, view, buttons);
        run.tick();
        let gun = run.gun();
        let level = run.level();
        let rec = &level.rt.host.turrets[&gun];
        let (firing, cooldown, lp) = (rec.firing, rec.cooldown_ms, rec.loop_left_ms);
        let p = level.players[slot].as_ref().unwrap();
        let locked = p.ps.word("viewlocked");
        let down = p.trigger_down;
        let shots = level.combat_shots().len();
        fire.push((firing, cooldown, lp, locked, down, shots));
    }
    run.level().kill_command(slot);
    run.tick();
    let dead = run.mount_state(slot);
    // The respawn places him at a spawn point, never back on the spot.
    let playing = |run: &mut Run| {
        run.level().rt.client_session(slot).map(|s| s.state)
            == Some(crate::follow::SessionState::Playing)
    };
    let mut ticks = 1;
    while !playing(&mut run) && ticks < 400 {
        run.tick();
        ticks += 1;
    }
    let respawned = run.mount_state(slot);
    assert_eq!(
        mounted,
        "owner Some(0) busy 1 barrel [-15.0, 45.0, 0.0] teleport true firing false stance Some(Stand) spot [-770.0, 68.0, 258.125]; active 1 eFlags 0x200 pm_flags 0x0 viewlocked 1 393 clamp 7.4999995 22.5 360.0 45.0 origin -770.0 68.0 258.125 view 69.99938 hint (12, 1, 1023)"
    );
    assert_eq!(
        tracked,
        "owner Some(0) busy 1 barrel [-14.999986, 45.0, 0.0] teleport true firing false stance Some(Stand) spot [-770.0, 68.0, 258.125]; active 1 eFlags 0x204 pm_flags 0x2 viewlocked 1 393 clamp 7.4999995 22.5 360.0 45.0 origin -751.03937 48.727333 258.125 view 45.0 hint (12, 1, 1023)"
    );
    assert_eq!(
        dead,
        "owner None busy 0 barrel [-14.999986, 45.0, 0.0] teleport true firing false stance None spot [-770.0, 68.0, 258.125]; active 0 eFlags 0x4 pm_flags 0x0 viewlocked 0 1023 clamp 7.4999995 22.5 360.0 45.0 origin -770.0 68.0 259.125 view 45.0 hint (0, 1, 1023)"
    );
    assert_eq!(
        fire,
        [
            (true, 50, 150, 2, true, 1),
            (true, 50, 150, 2, true, 2),
            (true, 50, 150, 2, true, 3),
            (false, 0, 150, 1, false, 3),
        ]
    );
    assert_eq!(ticks, 41);
    assert_eq!(
        respawned,
        "owner None busy 0 barrel [-51.0, 0.0, 0.0] teleport true firing false stance None spot [-770.0, 68.0, 258.125]; active 0 eFlags 0x0 pm_flags 0x0 viewlocked 0 1023 clamp 0.0 0.0 0.0 0.0 origin -208.0 2328.0 58.235626 view -89.99999 hint (0, 0, 0)"
    );
}

/// A live gunner respawned on the gun (the stock `spawnPlayer`) lets go
/// before `ClientSpawn` places him (0x80a7b8c..0x80a7ecf): the gun is free
/// and he stands at his spawn point, not over the mount spot.
#[test]
fn a_gunner_respawned_on_the_gun_stays_at_his_spawn() {
    let Some(mut run) = Run::backlot(1) else {
        return;
    };
    let slot = run.slots[0];
    run.level().set_view_pos(slot, BEHIND, [0.0; 3]);
    run.tick();
    run.tick();
    for _ in 0..10 {
        run.think(slot, [0.0, 0.0], 0x8);
        run.tick();
        if run.level().manned_gun(slot).is_some() {
            break;
        }
    }
    assert!(run.level().manned_gun(slot).is_some());
    run.level()
        .start_player_script(slot, "maps/mp/gametypes/_globallogic", "spawnplayer", &[])
        .unwrap();
    run.tick();
    let level = run.level();
    assert_eq!(level.manned_gun(slot), None);
    let ps = level.pm_state(slot).unwrap();
    assert_eq!(ps.e_flags & vcod_cod4::pmove::ef::TURRET, 0);
    let spot = Vec3::new(-770.0, 68.0, 259.125);
    assert!(ps.origin.distance(spot) > 100.0, "{:?}", ps.origin);
}

/// The hint's pick between a gun and a dropped weapon by the use list's
/// order, and a gunner's drop hint over a weapon at his feet.
#[test]
fn the_hint_picks_by_the_use_list_and_a_gunner_keeps_the_drop_hint() {
    let Some(mut run) = Run::backlot(2) else {
        return;
    };
    let (slot, dropper) = (run.slots[0], run.slots[1]);
    // A drop just behind the gunner's spot, of a weapon he does not hold.
    let behind = run.drop_gun(dropper, [BEHIND[0] - 30.0, BEHIND[1], BEHIND[2]]);
    let level = run.level();
    let w = level
        .rt
        .host
        .cod4()
        .items
        .iter()
        .find(|i| i.id.0 == behind)
        .unwrap()
        .weapon;
    let c4 = level.rt.host.cod4_mut();
    let table = std::mem::take(&mut c4.table);
    let l = &mut c4.clients[slot].loadout;
    l.take(&table, w as usize);
    c4.table = table;
    let l = &mut c4.clients[slot].loadout;
    let held = l.held().find(|&h| h != w as usize).unwrap_or(0);
    l.weapon = held;
    run.level().set_view_pos(slot, BEHIND, [0.0; 3]);
    let look = |run: &mut Run, view: [f32; 2]| {
        for _ in 0..2 {
            run.think(slot, view, 0);
            run.tick();
        }
        run.hint(slot)
    };
    let facing_gun = look(&mut run, [0.0, 0.0]);
    let facing_drop = look(&mut run, [0.0, 180.0]);
    let looking_down = look(&mut run, [60.0, 0.0]);
    let down_behind = look(&mut run, [60.0, 180.0]);
    run.level().set_view_pos(slot, BEHIND, [0.0; 3]);
    run.tick();
    for _ in 0..10 {
        run.think(slot, [0.0, 0.0], 0x8);
        run.tick();
        if run.level().manned_gun(slot).is_some() {
            break;
        }
    }
    run.think(slot, [60.0, 180.0], 0);
    run.tick();
    let mounted = run.hint(slot);
    assert_eq!(w, 14);
    assert_eq!(facing_gun, (12, 0, GUN));
    assert_eq!(facing_drop, (18, u32::MAX, behind));
    assert_eq!(looking_down, (12, 0, GUN));
    assert_eq!(down_behind, (18, u32::MAX, behind));
    assert_eq!(mounted, (12, 1, 1023));
}

/// A `trigger_use_touch` (an `sd` bomb zone on mp_crash) against a dropped
/// weapon at the feet: each team's player stands in the zone over the drop.
#[test]
fn a_bomb_zone_and_a_drop_under_one_player() {
    let Some(mut run) = Run::start("mp_crash", "sd", 3) else {
        return;
    };
    let (a, b, dropper) = (run.slots[0], run.slots[1], run.slots[2]);
    let level = run.level();
    let zone = level.rt.with_host(|h, cx| {
        let classname = cx.intern_folded("classname");
        let ids: Vec<EntId> = h.triggers.iter().map(|(id, _)| id).collect();
        let mut zones = Vec::new();
        for id in ids {
            let touch = matches!(h.get_field(cx, id, classname),
                vcod_gsc::Value::String(s) if cx.resolve(s) == "trigger_use_touch");
            if let Some(l) = h.area.last_link(id.0).filter(|_| touch) {
                zones.push((id.0, l));
            }
        }
        zones.sort_by_key(|z| z.0);
        zones.first().map(|(n, l)| {
            let mid = |k: usize| (l.absmin[k] + l.absmax[k]) * 0.5;
            (*n, [mid(0), mid(1), l.absmin[2]])
        })
    });
    let (zone, at) = zone.expect("a bomb zone");
    let eye = [at[0], at[1], at[2] + 60.0];
    let drop = run.drop_gun(dropper, eye);
    run.level().set_view_pos(
        dropper,
        [eye[0] + 2000.0, eye[1], eye[2] + 2000.0],
        [0.0; 3],
    );
    let w = run
        .level()
        .rt
        .host
        .cod4()
        .items
        .iter()
        .find(|i| i.id.0 == drop)
        .unwrap()
        .weapon;
    let mut seen = Vec::new();
    for slot in [a, b] {
        let level = run.level();
        let team = level.rt.client_team(slot);
        let c4 = level.rt.host.cod4_mut();
        let table = std::mem::take(&mut c4.table);
        let l = &mut c4.clients[slot].loadout;
        l.take(&table, w as usize);
        c4.table = table;
        let l = &mut c4.clients[slot].loadout;
        let held = l.held().find(|&h| h != w as usize).unwrap_or(0);
        l.weapon = held;
        run.level().set_view_pos(slot, eye, [60.0, 0.0, 0.0]);
        for _ in 0..2 {
            run.think(slot, [60.0, 0.0], 0);
            run.tick();
        }
        seen.push((team, run.hint(slot)));
        run.level()
            .set_view_pos(slot, [eye[0] + 2000.0, eye[1], eye[2] + 2000.0], [0.0; 3]);
        run.tick();
    }
    assert_eq!(zone, 118);
    assert_eq!(seen, [(2, (2, 0, zone)), (1, (w + 4, u32::MAX, drop))]);
}
