//! Two clients fighting on mp_toujane through `Cod2Level`: a rifle round
//! and an smg burst into the other's body, the stock `dm` callbacks
//! running on them, the kill, the score and the respawn, and a grenade's
//! blast. Needs `COD2_DIR`; without it every test passes.

use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::net::UserCmd2;

use crate::game::cod2::Cod2Level;
use crate::game::script::MenuRef;

/// The two clients' cmds for one frame and the frame itself.
pub(super) struct Fight {
    pub(super) level: Cod2Level,
    pub(super) now: i32,
    /// Each slot's view (pitch, yaw) and held buttons.
    pub(super) aim: [[f32; 2]; 2],
    pub(super) buttons: [i32; 2],
}

const ADS: i32 = vcod_cod2::pmove::button::ADS;

fn short(deg: f32) -> i32 {
    ((deg * 65536.0 / 360.0) as i32) & 0xffff
}

impl Fight {
    pub(super) fn frame(&mut self) {
        self.now += 50;
        for slot in 0..2 {
            let Some(p) = self.level.players[slot].as_ref() else {
                continue;
            };
            let mut angles = [0; 3];
            for (k, a) in angles.iter_mut().take(2).enumerate() {
                *a = (short(self.aim[slot][k]) - p.ps.delta_angles[k]) & 0xffff;
            }
            let weapon = self.level.rt.host.client_weapons[slot].current;
            let cmd = UserCmd2 {
                server_time: self.now,
                buttons: self.buttons[slot],
                weapon,
                off_hand_index: self.level.rt.host.cod2().clients[slot].offhand,
                angles,
                ..Default::default()
            };
            self.level.client_think(slot, &cmd);
        }
        self.level.run_frame(self.now);
    }

    pub(super) fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    pub(super) fn field(&mut self, slot: usize, name: &str) -> String {
        self.level.rt.client_field(slot, name).unwrap_or_default()
    }

    /// Both clients set down at two spawn spots that see each other once
    /// they stand on the floor, the shooter (0) looking at the victim's
    /// chest.
    pub(super) fn face_off(&mut self) {
        let spots = self.level.world.spawn_points.clone();
        for a in &spots {
            for b in &spots {
                let (a, b) = (Vec3::from(*a), Vec3::from(*b));
                if !(300.0..900.0).contains(&a.distance(b)) {
                    continue;
                }
                let floor = |at: Vec3| self.level.combat.floor_under(at + Vec3::Z * 8.0);
                let (Some(a), Some(b)) = (floor(a), floor(b)) else {
                    continue;
                };
                for (slot, at) in [(0, a), (1, b)] {
                    let p = self.level.players[slot].as_mut().unwrap();
                    p.ps.origin = at + Vec3::Z * 0.25;
                    p.ps.velocity = Vec3::ZERO;
                }
                self.frames(10);
                let (pa, pb) = (
                    self.level.players[0].as_ref().unwrap().ps.clone(),
                    self.level.players[1].as_ref().unwrap().ps.clone(),
                );
                let eye = Cod2Level::view_origin(&pa);
                let standing = pa.on_ground()
                    && pb.on_ground()
                    && pa.origin.distance(a) < 2.0
                    && pb.origin.distance(b) < 2.0
                    && self.health(0) == 100
                    && self.health(1) == 100;
                let seen = [20.0, 44.0, 60.0]
                    .iter()
                    .all(|&z| self.level.combat.clear_shot(eye, pb.origin + Vec3::Z * z));
                if standing && seen && (300.0..900.0).contains(&pa.origin.distance(pb.origin)) {
                    self.aim_at(0, 1, 44.0);
                    self.frames(2);
                    return;
                }
            }
        }
        panic!("no two mp_toujane spawns in sight of each other");
    }

    /// `from` looks at `to`'s origin raised `up`.
    pub(super) fn aim_at(&mut self, from: usize, to: usize, up: f32) {
        let a = &self.level.players[from].as_ref().unwrap().ps;
        let eye = Cod2Level::view_origin(a);
        let target = self.level.players[to].as_ref().unwrap().ps.origin + Vec3::Z * up;
        let ang = crate::game::builtins::math::vectoangles((target - eye).to_array());
        self.aim[from] = [ang[0], ang[1]];
    }

    pub(super) fn health(&self, slot: usize) -> i32 {
        self.level.rt.client_vitals(slot).health
    }
}

/// `dm` on mp_toujane with two clients spawned and armed, killcam off and
/// a forced respawn so a death comes back on its own.
pub(super) fn fight() -> Option<Fight> {
    fight_with(&[])
}

/// [`fight`] with `cvars` set over its own before the load.
pub(super) fn fight_with(cvars_over: &[(&str, &str)]) -> Option<Fight> {
    let fs = vcod_common::testing::cod2_fs()?;
    vcod_cod2::model::register();
    let mut cvars = crate::cvars::Cvars::new();
    cvars.set("developer", "1");
    cvars.set("scr_killcam", "0");
    cvars.set("scr_forcerespawn", "1");
    for (k, v) in cvars_over {
        cvars.set(k, v);
    }
    let level = Cod2Level::load(Rc::new(fs), "mp_toujane", "dm", cvars, 0).expect("load dm");
    let mut f = Fight {
        level,
        now: 0,
        aim: [[0.0; 2]; 2],
        buttons: [0; 2],
    };
    f.level.connect(0, "shooter");
    f.level.connect(1, "victim");
    f.frame();
    f.level.begin(0);
    f.level.begin(1);
    f.frame();
    for slot in 0..2 {
        f.level.menu_response(
            slot,
            &MenuRef::Name("team_britishgerman".into()),
            "autoassign",
        );
    }
    f.frames(4);
    for slot in 0..2 {
        let (menu, weapon) =
            if f.field(slot, "sessionteam") == "axis" || f.field(slot, "pers") == "axis" {
                ("weapon_german", "kar98k_mp")
            } else {
                ("weapon_british", "enfield_mp")
            };
        f.level
            .menu_response(slot, &MenuRef::Name(menu.into()), weapon);
    }
    f.frames(4);
    for slot in 0..2 {
        assert_eq!(f.field(slot, "sessionstate"), "playing", "slot {slot}");
    }
    // A raise runs before the first shot.
    f.frames(20);
    Some(f)
}

pub(super) fn assert_no_script_errors(f: &mut Fight) {
    let aborts = f.level.rt.aborts();
    assert!(aborts.is_empty(), "{}", aborts.join("\n"));
}

#[test]
fn a_rifle_round_kills_scores_and_the_victim_respawns() {
    let Some(mut f) = fight() else {
        return;
    };
    f.face_off();
    let weapon = f.level.rt.host.client_weapons[0].current;
    let name = f.level.rt.host.weapon_name(usize::from(weapon)).unwrap();
    assert!(
        name == "kar98k_mp" || name == "enfield_mp",
        "the shooter holds {name}"
    );
    // Down the sight, where the spread is `adsSpread`'s.
    f.buttons[0] = ADS;
    f.frames(20);
    assert_eq!(f.level.players[0].as_ref().unwrap().ws.pos_frac, 1.0);
    let clip = f.level.players[0].as_ref().unwrap().ws.clip;
    f.buttons[0] = ADS | 1;
    f.frame();
    f.buttons[0] = 0;
    let after = f.level.players[0].as_ref().unwrap().ws.clip;
    assert_eq!(
        clip.iter().sum::<i32>() - after.iter().sum::<i32>(),
        1,
        "one round left the clip"
    );
    // The flesh impacts: the large hit for all but the victim, the client
    // hit for the victim alone; and the killfeed's obituary.
    use crate::game::temp_entity::Scope;
    use vcod_common::net::event_ids as v1;
    let temps: Vec<(i32, Scope)> = f
        .level
        .rt
        .temp_entities()
        .map(|(_, te)| (te.event, te.scope))
        .collect();
    for want in [
        (v1::EV_BULLET_HIT_LARGE, Scope::AllBut(1)),
        (v1::EV_BULLET_HIT_CLIENT_LARGE, Scope::Only(1)),
        (v1::EV_OBITUARY, Scope::Broadcast),
    ] {
        assert!(temps.contains(&want), "{want:?} in {temps:?}");
    }
    let hit = f
        .level
        .combat
        .hits
        .back()
        .cloned()
        .expect("the round struck");
    assert_eq!((hit.victim, hit.attacker), (1, 0));
    assert_eq!(hit.mod_, "MOD_RIFLE_BULLET");
    assert_eq!(hit.weapon, name);
    assert!(hit.hitloc.starts_with("torso"), "{}", hit.hitloc);
    assert!(hit.damage >= 100, "{}", hit.damage);
    f.frames(2);
    assert_eq!(f.field(1, "sessionstate"), "dead");
    assert_eq!(f.field(0, "score"), "1");
    assert_eq!(f.field(1, "deaths"), "1");
    assert_eq!(
        f.level.players[1].as_ref().unwrap().ps.pm_type,
        vcod_cod2::pmove::pm_type::DEAD
    );
    // The corpse went into the body queue with the death anim.
    assert_eq!(f.level.rt.host.bodies.entities().count(), 1);
    assert_ne!(f.level.rt.host.cod2().clients[1].death_anim, 0);
    // `wait 2` in the killed callback, then the forced respawn.
    let mut back = false;
    for _ in 0..120 {
        f.frame();
        if f.field(1, "sessionstate") == "playing" {
            back = true;
            break;
        }
    }
    assert!(back, "the victim never respawned");
    assert_eq!(
        f.level.players[1].as_ref().unwrap().ps.pm_type,
        vcod_cod2::pmove::pm_type::NORMAL
    );
    assert_eq!(f.health(1), 100);
    assert_no_script_errors(&mut f);
}

#[test]
fn an_smg_burst_lands_several_rounds() {
    let Some(mut f) = fight() else {
        return;
    };
    // An mp40 in hand, a full clip.
    let (index, clip_index, ammo_index, clip_size) = f.level.rt.with_host(|h, _| {
        let i = h.weapon_index("mp40_mp").expect("mp40_mp");
        let d = h.weapons.get(i).unwrap();
        (i, d.clip_index, d.ammo_index, d.clip_size)
    });
    let h = &mut f.level.rt.host;
    h.client_weapons[0].give(index, 1);
    h.client_weapons[0].current = index as u8;
    h.client_ammo[0].clip[clip_index] = clip_size as i16;
    h.client_ammo[0].ammo[ammo_index] = 64;
    f.face_off();
    f.aim_at(0, 1, 20.0);
    f.buttons[0] = ADS;
    f.frames(20);
    // A held burst climbs off the body with the gun spring (cod2-combat.md
    // 9.6), so it holds long enough for a second round to come back on.
    f.buttons[0] = ADS | 1;
    f.frames(12);
    f.buttons[0] = 0;
    let hits: Vec<_> = f.level.combat.hits.iter().cloned().collect();
    assert!(hits.len() >= 2, "{} hits", hits.len());
    for h in &hits {
        assert_eq!(h.mod_, "MOD_PISTOL_BULLET");
        assert_eq!(h.weapon, "mp40_mp");
        assert_eq!(h.victim, 1);
    }
    let dealt: i32 = hits.iter().map(|h| h.damage).sum();
    let health = f.health(1);
    assert!(
        health == 100 - dealt || f.field(1, "sessionstate") == "dead",
        "{health} after {dealt}"
    );
    assert_no_script_errors(&mut f);
}

#[test]
fn a_grenade_at_the_feet_kills_by_its_blast() {
    let Some(mut f) = fight() else {
        return;
    };
    f.face_off();
    let offhand = f.level.rt.host.cod2().clients[0].offhand;
    assert_ne!(offhand, 0);
    // The frag key: the quick drop, the pin's `holdFireTime`, the hold;
    // released, the grenade flies.
    f.buttons[0] = vcod_cod2::pmove::weapon::key::FRAG;
    f.frames(20);
    assert_eq!(
        f.level.players[0].as_ref().unwrap().ws.state,
        vcod_cod2::pmove::weapon::state::OFFHAND_HOLD
    );
    f.buttons[0] = 0;
    f.frames(2);
    assert_eq!(f.level.combat.missiles.len(), 1, "the throw put one up");
    let thrown = f.level.combat.missiles[0].clone();
    assert_eq!(thrown.weapon, offhand);
    assert_eq!(thrown.owner, Some(0));
    // One at the victim's feet on a short fuse.
    let at = f.level.players[1].as_ref().unwrap().ps.origin + Vec3::Z * 4.0;
    f.level.fire_grenade(Some(0), offhand, at, Vec3::ZERO, 100);
    f.frames(4);
    let blast = f
        .level
        .combat
        .hits
        .iter()
        .find(|h| h.victim == 1)
        .cloned()
        .expect("the blast reached the victim");
    assert_eq!(blast.mod_, "MOD_GRENADE_SPLASH");
    assert!(blast.inflictor.is_some());
    assert_eq!(blast.dflags & 1, 1);
    assert_eq!(f.field(1, "sessionstate"), "dead");
    // As the wire sends it.
    let explode = f.level.missile_entities().iter().any(|(_, e)| {
        let c = crate::game::cod2_wire::to_cod2_entity(e).unwrap();
        (0..4).any(|k| c.i32(&format!("events[{k}]")) == vcod_cod2::net::events::EV_GRENADE_EXPLODE)
    });
    assert!(explode, "an explode event rides a missile");
    assert_no_script_errors(&mut f);
}

/// The level's turrets load their v20 models: `G_SpawnTurret`'s rest
/// pitch sweep reads the gun's bones (`turret_bones`).
#[test]
fn the_turrets_load_their_v20_bones() {
    let Some(f) = fight() else {
        return;
    };
    assert_eq!(f.level.rt.host.turrets.len(), 3);
    let fs = f.level.rt.host.fs.clone().unwrap();
    let bones = vcod_common::model::load_bones(&*fs, "weapon_mg42").expect("weapon_mg42");
    assert!(bones.iter().any(|b| b.name == "tag_aim"));
}

/// A kill as retail's walker capture shows it (`cod2-player-anims.md` 10):
/// the death anim on the victim's legs, no anim on its torso, both
/// toggled; the corpse (`eType` 2) with those anims and `eFlags` 0xa0000
/// on the wire, the fresh bit out once the death anim has played; and
/// under `scr_forcerespawn 0` the victim back only on the use key.
#[test]
fn a_kill_plays_the_death_anim_on_the_corpse_and_use_respawns() {
    let Some(mut f) = fight() else {
        return;
    };
    f.level.rt.host.cvars.set("scr_forcerespawn", "0");
    f.face_off();
    let before = f.level.players[1].as_ref().unwrap().ps.clone();
    assert_ne!(before.legs_anim, 0, "a standing body plays an idle");
    f.buttons[0] = ADS;
    f.frames(20);
    f.buttons[0] = ADS | 1;
    f.frame();
    f.buttons[0] = 0;
    f.frames(2);
    assert_eq!(f.field(1, "sessionstate"), "dead");
    let toggle = vcod_cod2::pmove::anim::ANIM_TOGGLEBIT;
    let death = f.level.rt.host.cod2().clients[1].death_anim;
    let ps = f.level.players[1].as_ref().unwrap().ps.clone();
    assert_ne!(death, 0);
    assert_eq!(ps.legs_anim & !toggle, death);
    assert_eq!(ps.torso_anim & !toggle, 0);
    assert!(ps.legs_timer > 0 && ps.legs_timer == ps.torso_timer);
    let corpse = |f: &Fight| {
        let (_, e) = f.level.rt.host.bodies.entities().next().expect("a corpse");
        crate::game::cod2_wire::to_cod2_entity(&e).unwrap()
    };
    let c = corpse(&f);
    assert_eq!(c.i32("eType"), 2);
    assert_eq!(c.i32("eFlags") & 0xa_0000, 0xa_0000);
    assert_eq!(c.i32("legsAnim"), ps.legs_anim);
    assert_eq!(c.i32("torsoAnim"), ps.torso_anim);
    // The fresh bit goes once the death anim's duration has run.
    let frames = (ps.legs_timer / 50 + 2) as usize;
    f.frames(frames.min(60));
    assert_eq!(corpse(&f).i32("eFlags") & 0xa_0000, 0x2_0000);
    // Nothing pressed: still dead well past the killed callback's wait.
    f.frames(60);
    assert_eq!(f.field(1, "sessionstate"), "dead");
    f.buttons[1] = 0x20;
    f.frames(4);
    f.buttons[1] = 0;
    assert_eq!(f.field(1, "sessionstate"), "playing");
    assert_no_script_errors(&mut f);
}

/// The stock killcam (`_killcam.gsc`'s `waitSkipKillcamButton`) ends on a
/// use press after a release, the follower's cmds read by
/// `useButtonPressed` as retail's `SpectatorThink` arm stores them
/// (cod2-killcam.md 1.1).
#[test]
fn a_use_press_skips_the_killcam() {
    let Some(mut f) = fight_with(&[("scr_killcam", "1"), ("scr_forcerespawn", "0")]) else {
        return;
    };
    // An archive old enough for the killcam not to give up.
    f.frames(200);
    f.face_off();
    f.buttons[0] = ADS;
    f.frames(20);
    f.buttons[0] = ADS | 1;
    f.frame();
    f.buttons[0] = 0;
    let mut frames = 0;
    while f.field(1, "sessionstate") != "spectator" {
        f.frame();
        frames += 1;
        assert!(frames < 100, "no killcam: {}", f.field(1, "sessionstate"));
    }
    f.frames(10);
    assert!(f.level.players[1].as_ref().unwrap().follow.is_some());
    assert_eq!(f.field(1, "sessionstate"), "spectator");
    // `killcam(.., true)`: the press ends the replay, and `dm`'s
    // `waitRespawnButton` reads the same held key as the respawn.
    f.buttons[1] = 0x8;
    f.frames(4);
    f.buttons[1] = 0;
    assert_eq!(f.field(1, "sessionstate"), "playing", "the press ended it");
    assert_no_script_errors(&mut f);
}

/// `kill`: the asker dies by `MOD_SUICIDE` (obituary parm `0x80 | 12`,
/// attacker himself), and `score` then ranks the two by score, the
/// suicide's -1 last (`SortRanks`, cod2-protocol.md 3.4.1).
#[test]
fn kill_obituary_and_score_rank_a_suicide_last() {
    let Some(mut f) = fight() else {
        return;
    };
    f.level.kill(0);
    f.frames(2);
    assert_eq!(f.field(0, "sessionstate"), "dead");
    assert_eq!(f.field(0, "deaths"), "1");
    let obit = f
        .level
        .rt
        .temp_entities()
        .map(|(_, te)| te)
        .find(|te| te.event == vcod_common::net::event_ids::EV_OBITUARY)
        .map(|te| (te.other, te.attacker, te.parm))
        .expect("an obituary");
    assert_eq!(obit, (0, 0, 0x80 | 12));
    // A playing client asked again: nothing, he is dead.
    f.level.kill(0);
    let board = f.level.scoreboard();
    let words: Vec<i32> = board
        .split_whitespace()
        .skip(1)
        .map(|w| w.parse().unwrap())
        .collect();
    assert_eq!(&words[..3], &[2, 0, 0], "{board}");
    let (a, b) = (&words[3..8], &words[8..13]);
    assert_eq!((a[0], a[1], a[3]), (1, 0, 0), "{board}");
    assert_eq!((b[0], b[1], b[3]), (0, -1, 1), "{board}");
    assert_no_script_errors(&mut f);
}

/// `client_think` scales the walk by the held weapon's `moveSpeedScale`
/// (0x80e1c58): an mp40 (1.05) runs faster than the rifle beside it.
#[test]
fn the_held_weapon_scales_the_run() {
    let Some(mut f) = fight() else {
        return;
    };
    let index = f
        .level
        .rt
        .with_host(|h, _| h.weapon_index("mp40_mp").expect("mp40_mp"));
    let h = &mut f.level.rt.host;
    h.client_weapons[0].give(index, 1);
    h.client_weapons[0].current = index as u8;
    f.face_off();
    f.aim_at(1, 0, 44.0);
    for _ in 0..10 {
        f.now += 50;
        for slot in 0..2 {
            let p = f.level.players[slot].as_ref().unwrap();
            let mut angles = [0; 3];
            for (k, a) in angles.iter_mut().take(2).enumerate() {
                *a = (short(f.aim[slot][k]) - p.ps.delta_angles[k]) & 0xffff;
            }
            let cmd = UserCmd2 {
                server_time: f.now,
                weapon: f.level.rt.host.client_weapons[slot].current,
                angles,
                forward_move: 127,
                ..Default::default()
            };
            f.level.client_think(slot, &cmd);
        }
        f.level.run_frame(f.now);
    }
    let speed = |slot: usize| {
        let p = f.level.players[slot].as_ref().unwrap();
        (p.ws.weapon, p.ps.velocity.truncate().length())
    };
    let ((w0, v0), (w1, v1)) = (speed(0), speed(1));
    let defs = &f.level.combat.weapons;
    let scale = |w: u8| vcod_cod2::pmove::weapon::move_scale(defs, w);
    assert_eq!(w0 as usize, index);
    assert_eq!(scale(w0), 1.05);
    assert!(v1 > 150.0, "the rifle runs at {v1}");
    let want = scale(w0) / scale(w1);
    assert!((v0 / v1 - want).abs() < 0.01, "{v0} / {v1}, want {want}");
}

/// Two playing clients set down on one spot `apart` units along x from
/// the first spawn's floor, both still.
fn stacked(f: &mut Fight, apart: f32) -> Vec3 {
    let spot = Vec3::from(f.level.world.spawn_points[0]);
    let at = f.level.combat.floor_under(spot + Vec3::Z * 8.0).unwrap() + Vec3::Z * 0.25;
    for (slot, x) in [(0, 0.0), (1, apart)] {
        let p = f.level.players[slot].as_mut().unwrap();
        p.ps.origin = at + Vec3::X * x;
        p.ps.velocity = Vec3::ZERO;
    }
    at
}

/// A run into another player stops at the two capsules (15 + 15), and
/// two players set down overlapping are pushed apart by `StuckInClient`
/// (lnxded 0x80f6e9e), each whose turn found the pair going to contents
/// 0x4000000 for the frame (cod2-turrets.md 14.2).
#[test]
fn players_block_each_other_and_overlapping_ones_part() {
    let Some(mut f) = fight() else {
        return;
    };
    let at = stacked(&mut f, 120.0);
    f.aim = [[0.0, 0.0], [0.0, 180.0]];
    for _ in 0..30 {
        f.now += 50;
        let weapon = f.level.rt.host.client_weapons[0].current;
        let delta = f.level.players[0].as_ref().unwrap().ps.delta_angles;
        let cmd = UserCmd2 {
            server_time: f.now,
            weapon,
            forward_move: 127,
            angles: [(-delta[0]) & 0xffff, (-delta[1]) & 0xffff, 0],
            ..Default::default()
        };
        f.level.client_think(0, &cmd);
        f.level.run_frame(f.now);
    }
    let (a, b) = (
        f.level.players[0].as_ref().unwrap().ps.origin,
        f.level.players[1].as_ref().unwrap().ps.origin,
    );
    assert!(a.x > at.x + 60.0, "the runner left the spot: {a} from {at}");
    assert!(b.x - a.x >= 29.9, "the runner went into the other: {a} {b}");
    assert!(b.x - a.x < 40.0, "the runner stopped short: {a} {b}");
    assert_no_script_errors(&mut f);

    stacked(&mut f, 10.0);
    f.aim = [[0.0, 0.0]; 2];
    f.frame();
    let contents = |f: &Fight, slot: usize| f.level.players[slot].as_ref().unwrap().contents;
    // Slot 1's own turn finds the pair again (a 0x4000000 partner still
    // counts) and pushes the same way.
    assert_eq!(contents(&f, 0), vcod_common::movetrace::CONTENTS_CORPSE);
    assert_eq!(contents(&f, 1), vcod_common::movetrace::CONTENTS_CORPSE);
    let (pa, pb) = (
        f.level.players[0].as_ref().unwrap().ps.clone(),
        f.level.players[1].as_ref().unwrap().ps.clone(),
    );
    assert!(
        pa.velocity.x < 0.0 && pb.velocity.x > 0.0,
        "{:?} {:?}",
        pa.velocity,
        pb.velocity
    );
    assert_eq!((pa.pm_time, pb.pm_time), (300, 300));
    f.frames(20);
    let (a, b) = (
        f.level.players[0].as_ref().unwrap().ps.origin,
        f.level.players[1].as_ref().unwrap().ps.origin,
    );
    assert!(
        (b - a).truncate().length() > 30.0,
        "still overlapping: {a} {b}"
    );
}
