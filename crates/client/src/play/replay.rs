//! Retail captures replayed through the client's per-frame event path,
//! headless: the prediction (`Predictor::predict`), the event collection and
//! its dedup (`events::frame_events`), the kick (`events::view_kicks`), the
//! effects (`fx::registry::resolve`) and the sounds (`audio::cues::resolve`),
//! each called as the frame in `main.rs` calls it. Every shot has to play
//! once: one fire event, one flash, one fire sound, one kick.
//!
//! The own-body replays read a `--save-slope` capture's cmds and snapshots
//! in arrival order, one frame per cmd, and compare what played with the
//! events each snapshot brought (`ev=`). They need the paks: the UO autofire
//! capture the UO install's (`COD_DIR` at it), else 1.1's mp_carentan and
//! mp44, whose fire and kick keys are UO's (coduo-movement.md 15); the 1.1
//! moves capture a 1.1 install. The follow replays decode the committed
//! snapshot captures and need nothing; the 1.1 one resolves its flashes
//! and fire sounds only with a 1.1 install's weapon files.

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::events::{EventTracker, GameEvent};
use vcod_common::net::msg::{self, NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};
use vcod_common::net::snapshot::Snapshot;
use vcod_common::patch::Arith;
use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::movers::SnapshotMovers;
use vcod_common::pmove::predict;
use vcod_common::weapon::{WeaponDef, WeaponSounds};

use super::cmds::CmdRing;
use super::events::{self, PredictedEvents};
use super::predict::{Predictor, solid_bodies};
use crate::audio::cues::{self, CueCtx};
use crate::fx::registry::{self, ResolveCtx, Resolved};
use crate::hud::player::{DamageFeedback, DamageIndicators};

const SLOPE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/playerstate"
);

/// What a replay played.
#[derive(Default, Debug)]
struct Tally {
    /// Events played, by 1.1 id.
    played: BTreeMap<i32, usize>,
    /// Fire events played, and their flashes, fire sounds and kicks.
    fires: usize,
    flashes: usize,
    fire_sounds: usize,
    kicks: usize,
    /// Fire events that flashed more than once or sounded more than once a
    /// barrel.
    doubled: usize,
    /// Events no effect handler knows, by id.
    unknown: BTreeMap<i32, usize>,
    /// Sound cues started, by the event id that started them.
    cues: BTreeMap<i32, usize>,
    /// Hits fed to the kick, the flash and the icons, and those taken on
    /// a mounted gun.
    hits: usize,
    mounted_hits: usize,
}

/// One frame's state, as `LivePhase` holds it.
struct Client<'a> {
    p: &'static Cod1Wire,
    client_num: i32,
    weapons: &'a [Option<WeaponDef>],
    sounds: HashMap<i32, Option<WeaponSounds>>,
    configstrings: &'a [String],
    tracked: PredictedEvents,
    tracker: EventTracker,
    predictor: Predictor,
    damage: DamageIndicators,
    tally: Tally,
}

impl<'a> Client<'a> {
    fn new(
        p: &'static Cod1Wire,
        client_num: i32,
        weapons: &'a [Option<WeaponDef>],
        configstrings: &'a [String],
    ) -> Self {
        let sounds = weapons
            .iter()
            .enumerate()
            .map(|(i, w)| (i as i32, w.as_ref().map(|d| d.sounds.clone())))
            .collect();
        Client {
            p,
            client_num,
            weapons,
            sounds,
            configstrings,
            tracked: PredictedEvents::default(),
            tracker: EventTracker::new(),
            predictor: Predictor::default(),
            damage: DamageIndicators::default(),
            tally: Tally::default(),
        }
    }

    /// One frame on `newest`, predicted through `ring` when a world is given.
    fn frame(
        &mut self,
        newest: &Snapshot,
        prediction: Option<(&CmdRing, &CollisionWorld)>,
        now_ms: f64,
    ) {
        let p = self.p;
        let ps_client = newest.ps.field_i32(p, "clientNum");
        let pm_type = newest.ps.field_i32(p, "pm_type");
        let own = ps_client == self.client_num;
        let predicted = match prediction.filter(|_| own) {
            Some((ring, world)) => {
                let bodies =
                    solid_bodies(p, &newest.entities, self.client_num as u32, &HashMap::new());
                let movers = (
                    SnapshotMovers::from_entities(p, &newest.entities),
                    newest.server_time,
                );
                self.predictor.predict(
                    p,
                    &newest.ps,
                    ring,
                    world,
                    &bodies,
                    &movers,
                    self.weapons,
                    now_ms,
                )
            }
            None => None,
        };
        // `main.rs`'s `Hud::feed_damage`, the playerstate's view standing in
        // for the drawn one.
        let fb = DamageFeedback::from_ps(p, &newest.ps);
        let kick_ms = predicted
            .as_ref()
            .map_or(newest.server_time, |v| v.pred.command_time);
        if self
            .damage
            .feed(fb, newest.ps.viewangles(p), newest.server_time, kick_ms)
        {
            self.tally.hits += 1;
            self.tally.mounted_hits +=
                usize::from(newest.ps.field_i32(p, "eFlags") & crate::turret::EF_MOUNTED != 0);
        }
        let evs = events::frame_events(
            &mut self.tracked,
            &mut self.tracker,
            newest,
            p,
            own,
            own && predict::predictable(pm_type),
            predicted.as_ref().map(|v| &v.pred),
            ps_client,
        );
        // `view_ps` is drawn, and so kicked, only while predictable.
        if predict::on_foot(pm_type) {
            self.tally.kicks += events::view_kicks(&evs, ps_client);
        }
        // Every body has a muzzle: the model pass poses them all.
        let mut muzzles: HashMap<u32, (Vec3, Vec3)> = newest
            .entities
            .keys()
            .map(|&n| (n, (Vec3::ZERO, Vec3::X)))
            .collect();
        muzzles.insert(u32::MAX, (Vec3::ZERO, Vec3::X));
        let no_flash = HashMap::new();
        let ctx = ResolveCtx {
            muzzles: &muzzles,
            shot_fallbacks: &HashMap::new(),
            weapon_flash: &no_flash,
            view_flash: Some(self.weapons),
            view_body: registry::view_body(newest.ps.field_i32(p, "pm_flags"), ps_client),
            blood: true,
            brass: &HashMap::new(),
            weapons: self.weapons,
            eject_brass: true,
            configstrings: self.configstrings,
            cs: &p.cs,
        };
        let cue_ctx = CueCtx {
            configstrings: self.configstrings,
            sounds: p.cs.sounds(),
            weapon_sounds: &self.sounds,
            muzzles: &muzzles,
            shot_fallbacks: &HashMap::new(),
            listener_pos: Vec3::ZERO,
            ps_entity: cues::ps_entity(ps_client),
            uo: p.is_uo(),
        };
        for ev in &evs {
            dispatch(&mut self.tally, ev, &ctx, &cue_ctx);
        }
    }
}

/// One event through the effects and the sounds, as the frame's event loop
/// runs it.
fn dispatch(t: &mut Tally, ev: &GameEvent, ctx: &ResolveCtx, cue_ctx: &CueCtx) {
    *t.played.entry(ev.event).or_default() += 1;
    let resolved = registry::resolve(ev, ctx);
    if resolved.iter().any(|r| matches!(r, Resolved::Unknown)) {
        *t.unknown.entry(ev.event).or_default() += 1;
    }
    let sounds = cues::resolve(ev, cue_ctx).len();
    if sounds > 0 {
        *t.cues.entry(ev.event).or_default() += sounds;
    }
    if !cues::fires_weapon(ev.event) {
        return;
    }
    // A fire event resolves to its flash, its spent case and its sound alone.
    let flashes = resolved
        .iter()
        .filter(|r| match r {
            Resolved::Spawn { bolt, .. } => bolt.is_some(),
            Resolved::OnTag { .. } => true,
            _ => false,
        })
        .count();
    let calls = super::recoil::fire_calls(ev.event).max(1);
    t.fires += 1;
    t.flashes += flashes;
    t.fire_sounds += sounds;
    t.doubled += usize::from(flashes > 1 || sounds > calls);
}

/// A `--save-slope` capture line.
enum Line {
    Cmd(UserCmd),
    /// The snapshot, and the `(event, parm)` pairs it brought in 1.1's ids.
    Snap(Box<Snapshot>, Vec<(i32, i32)>),
}

fn parse(p: &Cod1Wire, text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((kind, rest)) = line.split_once(' ') else {
            continue;
        };
        let kv: BTreeMap<&str, &str> = rest
            .split_whitespace()
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let int = |k: &str| -> i32 { kv[k].parse().unwrap_or_else(|_| panic!("{k} in {line}")) };
        let pairs = |k: &str| -> Vec<(&str, i32)> {
            kv.get(k)
                .into_iter()
                .flat_map(|v| v.split(','))
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let (a, b) = s.rsplit_once(':').expect("name:value");
                    (a, b.parse().expect("an int"))
                })
                .collect()
        };
        match kind {
            "!cmd" => {
                let a: Vec<i32> = kv["angles"]
                    .split(',')
                    .map(|x| x.parse().unwrap())
                    .collect();
                out.push(Line::Cmd(UserCmd {
                    server_time: int("st"),
                    buttons: int("buttons") as u8,
                    wbuttons: int("wbuttons") as u8,
                    weapon: int("weapon") as u8,
                    angles: [a[0], a[1], a[2]],
                    forward: int("forward") as i8,
                    right: int("right") as i8,
                    up: int("up") as i8,
                    ..NULL_USERCMD
                }));
            }
            "!snap" => {
                let mut ps = msg::PlayerState::null(p);
                for (name, v) in pairs("wire") {
                    ps.fields[msg::PlayerState::field_index(p, name).expect(name)] = v;
                }
                for (k, v) in pairs("ammo") {
                    ps.arrays.ammo[k.parse::<usize>().unwrap()] = v as i16;
                }
                for (k, v) in pairs("clip") {
                    ps.arrays.ammoclip[k.parse::<usize>().unwrap()] = v as i16;
                }
                let ev = pairs("ev")
                    .into_iter()
                    .map(|(e, parm)| (e.parse().expect("an event id"), parm))
                    .collect();
                let snap = Snapshot {
                    server_time: int("t"),
                    message_num: int("msg") as u32,
                    delta_num: -1,
                    snap_flags: 0,
                    ps,
                    entities: BTreeMap::new(),
                    clients: BTreeMap::new(),
                    valid: true,
                };
                out.push(Line::Snap(Box::new(snap), ev));
            }
            _ => {}
        }
    }
    out
}

/// The capture's map world as the client loads it.
fn load_world(fs: &Pk3Fs, map: &str) -> CollisionWorld {
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("parse");
    let world = CollisionWorld::build(&bsp, &vcod_common::props::collision_tris(fs, &bsp.entities));
    world.unlink_script_brushes(&bsp.entities, "dm");
    world
}

/// A `--save-slope` capture replayed as our own body, one frame per cmd:
/// what played, and the events the snapshots brought by id.
fn replay_own(
    p: &'static Cod1Wire,
    file: &str,
    fs: &Pk3Fs,
    map: &str,
) -> (Tally, BTreeMap<i32, usize>) {
    let (t, brought, _) = replay_own_as(p, file, fs, map, Arith::for_client());
    (t, brought)
}

/// `replay_own` with the predictor's world on `arith`, and the
/// predictor's corrections (`Predictor::corrections`).
fn replay_own_as(
    p: &'static Cod1Wire,
    file: &str,
    fs: &Pk3Fs,
    map: &str,
    arith: Arith,
) -> (Tally, BTreeMap<i32, usize>, (usize, usize)) {
    let text = std::fs::read_to_string(format!("{SLOPE_DIR}/{file}")).unwrap();
    // The 1.1 captures name none: retail's mp_carentan dm list.
    let cs7 = text
        .lines()
        .find_map(|l| l.strip_prefix("# cs7 "))
        .or_else(|| {
            include_str!("../../../server/tests/fixtures/configstrings/mp_carentan-dm.txt")
                .lines()
                .find_map(|l| l.strip_prefix("7 "))
        })
        .expect("a weapon list");
    let weapons = vcod_common::weapon_table::from_configstring(fs, cs7);
    let world = load_world(fs, map);
    world.set_arith(arith);
    let mut client = Client::new(p, 0, &weapons, &[]);
    let mut ring = CmdRing::default();
    let mut newest: Option<Snapshot> = None;
    let mut brought = BTreeMap::new();
    for line in parse(p, &text) {
        match line {
            Line::Snap(s, ev) => {
                // The first drain only marks the ring.
                if newest.is_some() {
                    for (e, _) in ev {
                        *brought.entry(e).or_default() += 1;
                    }
                }
                newest = Some(*s);
            }
            Line::Cmd(c) => {
                ring.push(c);
                if let Some(s) = &newest {
                    client.frame(s, Some((&ring, &world)), f64::from(c.server_time));
                }
            }
        }
    }
    (client.tally, brought, client.predictor.corrections)
}

/// Every shot of an own-body replay played once, with its flash, its
/// sound and its kick.
fn assert_each_shot_once(name: &str, t: &Tally, brought: &BTreeMap<i32, usize>, shots: usize) {
    println!("{name}: {t:?}");
    let fires: usize = brought
        .iter()
        .filter(|(e, _)| cues::fires_weapon(**e))
        .map(|(_, n)| n)
        .sum();
    assert_eq!(fires, shots, "{name}: the capture's shots");
    assert_eq!(&t.played, brought, "{name}: events played against brought");
    assert_eq!(t.fires, shots, "{name}: fire events");
    assert_eq!(t.flashes, shots, "{name}: flashes");
    assert_eq!(t.fire_sounds, shots, "{name}: fire sounds");
    assert_eq!(t.kicks, shots, "{name}: kicks");
    assert!(
        t.unknown.is_empty(),
        "{name}: unknown events {:?}",
        t.unknown
    );
}

/// United Offensive's mp44 held from the hip, held down the sight, then
/// tapped (`--probe-autofire`, moves_ab's `AUTOFIRE_UO`): 29 rounds, each
/// once. Reading the predicted ring in UO's ids played every round twice
/// (coduo-movement.md 15).
#[test]
fn a_uo_autofire_capture_plays_each_shot_once() {
    let Some(fs) = vcod_common::testing::uo_fs().or_else(vcod_common::testing::stock_fs) else {
        return;
    };
    let (t, brought) = replay_own(
        &PROTOCOL_V22,
        "mp_carentan-dm-slope-8ms-autofire-uo.txt",
        &fs,
        "mp_carentan",
    );
    assert_each_shot_once("autofire-uo", &t, &brought, 29);
}

/// 1.1's movement script (moves_ab's `moves-1.1`): jumps, landings, stance
/// changes and eight carbine shots, every event once.
#[test]
fn a_1_1_moves_capture_plays_each_event_once() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let (t, brought) = replay_own(
        &PROTOCOL_V1,
        "mp_carentan-dm-slope-8ms-moves-1.1.txt",
        &fs,
        "mp_carentan",
    );
    assert_each_shot_once("moves-1.1", &t, &brought, 8);
}

/// The own-body captures predicted as the Windows client computes
/// (`Arith::Msvc`) against the same replay on the Linux server's
/// arithmetic. Every capture was taken against a Linux server, so the
/// client's build corrects more often in the last bits (1.5 moves 13 to 46,
/// UO sprint 114 to 125, 1.1 moves 5 to 8 on 2026-10-17): ties the retail
/// client loses too (cod15-movement.md 8). No correction it draws, one
/// past `MISS_EPSILON`, may be added.
#[test]
fn the_windows_build_draws_no_new_correction() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let v1 = vcod_common::testing::game_fs_11();
    let mut runs: Vec<(&'static Cod1Wire, &str, &Pk3Fs)> = vec![
        (&PROTOCOL_V6, "mp_carentan-dm-slope-8ms-moves-1.5.txt", &fs),
        (&PROTOCOL_V6, "mp_carentan-dm-slope-8ms-turns-1.5.txt", &fs),
        (
            &PROTOCOL_V22,
            "mp_carentan-dm-slope-8ms-autofire-uo.txt",
            &fs,
        ),
        (&PROTOCOL_V22, "mp_carentan-dm-slope-8ms-sprint-uo.txt", &fs),
    ];
    if let Some(v1) = &v1 {
        for file in [
            "mp_carentan-dm-slope-8ms-moves-1.1.txt",
            "mp_carentan-dm-slope-8ms-turns-1.1.txt",
            "mp_carentan-dm-slope-8ms-springfield-1.1.txt",
        ] {
            runs.push((&PROTOCOL_V1, file, v1));
        }
    }
    let mut worse = Vec::new();
    for (p, file, fs) in runs {
        let (_, _, linux) = replay_own_as(p, file, fs, "mp_carentan", Arith::for_protocol(p));
        let (_, _, msvc) = replay_own_as(p, file, fs, "mp_carentan", Arith::for_client());
        println!("{file}: corrections, any and past 0.1: linux {linux:?}, msvc {msvc:?}");
        if msvc.1 > linux.1 {
            worse.push((file, linux, msvc));
        }
    }
    assert!(worse.is_empty(), "{worse:?}");
}

/// The 1.1 ids of a ring's new slots, `prev..cur` (`eventSequence` counts
/// after the write).
fn new_slots(p: &Cod1Wire, prev: i32, cur: i32, slot: impl Fn(String) -> i32) -> Vec<i32> {
    (prev..prev + ((cur - prev) & 0xff).min(4))
        .map(|i| (p.event_to_v1)(slot(format!("events[{}]", i & 3))))
        .collect()
}

/// A committed snapshot capture decoded and played frame by frame with no
/// prediction: a follower's view as the client draws it, and a seated
/// body's own ring as a prediction miss plays it, off the snapshot. Checks
/// the fire events played against the rings' own count.
fn replay_capture(p: &'static Cod1Wire, tag: &str, weapons: &[Option<WeaponDef>]) -> Tally {
    let gs = vcod_common::testing::fixture_gamestate(p, &format!("net/gamestate{tag}.bin"));
    let mut client = Client::new(p, gs.client_num, weapons, &gs.configstrings);
    let (mut fires, mut kicks) = (0, 0);
    let mut ps_seq = None;
    let (mut hits, mut last_event) = (0, None);
    let mut seqs: HashMap<u32, i32> = HashMap::new();
    for f in vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate{tag}.bin"),
        &format!("net/snapshots{tag}.bin"),
    ) {
        let s = &f.snapshot;
        let event = s.ps.field_i32(p, "damageEvent");
        if last_event.replace(event).is_some_and(|e| e != event) {
            hits += usize::from(s.ps.field_i32(p, "damageCount") != 0);
        }
        let cur = s.ps.field_i32(p, "eventSequence");
        if let Some(prev) = ps_seq.replace(cur) {
            for e in new_slots(p, prev, cur, |n| s.ps.field_i32(p, &n)) {
                kicks += super::recoil::fire_calls(e);
                fires += usize::from(cues::fires_weapon(e));
            }
        }
        for (&num, e) in &s.entities {
            if p.event_of(e.field_i32(p, "eType")).is_some() {
                continue;
            }
            let cur = e.field_i32(p, "eventSequence");
            if let Some(prev) = seqs.insert(num, cur) {
                fires += new_slots(p, prev, cur, |n| e.field_i32(p, &n))
                    .into_iter()
                    .filter(|&ev| cues::fires_weapon(ev))
                    .count();
            }
        }
        seqs.retain(|n, _| s.entities.contains_key(n));
        client.frame(s, None, f64::from(s.server_time));
    }
    let t = client.tally;
    println!("{tag}: {fires} fires, {kicks} kicks and {hits} hits in the rings, {t:?}");
    assert_eq!(t.fires, fires, "{tag}: fire events");
    assert_eq!(t.hits, hits, "{tag}: hits");
    assert_eq!(t.kicks, kicks, "{tag}: kicks");
    t
}

/// The UO captures' weapons: the UO install's files, else the keys
/// `registry::uo_captures` copied out of them.
fn uo_weapons(tag: &str) -> Vec<Option<WeaponDef>> {
    match vcod_common::testing::uo_fs() {
        Some(fs) => {
            let gs = vcod_common::testing::fixture_gamestate(
                &PROTOCOL_V22,
                &format!("net/gamestate{tag}.bin"),
            );
            vcod_common::weapon_table::from_configstring(&fs, &gs.configstrings[7])
        }
        None => registry::uo_captures::weapons(tag.trim_start_matches('-')),
    }
}

/// Each fire event of the UO captures `tags` played and kicked once,
/// flashing and sounding at most once, and no event the client cannot place.
fn check_uo_captures(tags: &[&str]) {
    for tag in tags {
        let weapons = uo_weapons(tag);
        let t = replay_capture(&PROTOCOL_V22, tag, &weapons);
        assert!(
            t.unknown.is_empty(),
            "{tag}: unknown events {:?}",
            t.unknown
        );
        assert_eq!(t.doubled, 0, "{tag}: fire events flashed or sounded twice");
    }
}

// The committed UO captures, split so the harness decodes them in parallel.

#[test]
fn the_uo_tank_captures_play_each_shot_once() {
    check_uo_captures(&["-uo-tank", "-uo-overheat"]);
}

#[test]
fn the_uo_jeep_and_turret_captures_play_each_shot_once() {
    check_uo_captures(&["-uo-jeep", "-uo-turret"]);
}

/// A mounted MG's rounds ride the turret's ring, and `CG_FireWeapon` kicks
/// only for the body the view rides (uo cgame 0x30047d20, 1.1's
/// 0x30038bca): the followed gunner's 148 rounds kick nothing.
#[test]
fn a_followed_mounted_gunner_is_not_kicked() {
    let t = replay_capture(&PROTOCOL_V22, "-uo-turret", &uo_weapons("-uo-turret"));
    assert_eq!((t.fires, t.kicks), (148, 0));
}

#[test]
fn the_uo_mg_arty_and_quake_captures_play_each_shot_once() {
    check_uo_captures(&["-uo-mg", "-uo-arty", "-uo-quake"]);
}

#[test]
fn the_uo_foot_captures_play_each_shot_once() {
    check_uo_captures(&["-uo", "-uo-flame", "-uo-satchel", "-uo-smoke"]);
}

/// The followed gunner of `*-uo-turret.bin` through UO's turret view (uo
/// cgame 0x30041550). Its playerstate rides the gun with `viewlocked` 0, so
/// only UO's gate finds it; the view held inside the gun's arcs is the
/// barrel the server sent on every mounted snapshot, the playerstate's own
/// pitch running 20 degrees past the top arc at 28250 included; and the eye
/// shakes on the snapshots that fire.
#[test]
fn a_followed_uo_gunner_sees_down_the_gun() {
    use crate::turret::{self, MsvcRand, UoArcs};
    let p = &PROTOCOL_V22;
    let (mut mounted, mut held, mut firing) = (0, 0, 0);
    let mut rng = MsvcRand::default();
    let tag = (Vec3::ZERO, [Vec3::X, Vec3::Y, Vec3::Z]);
    for f in vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-uo-turret.bin",
        "net/snapshots-uo-turret.bin",
    ) {
        let s = &f.snapshot;
        let int = |n: &str| s.ps.field_i32(p, n);
        let (ef, vl, ent) = (int("eFlags"), int("viewlocked"), int("viewlocked_entNum"));
        assert_eq!(turret::ridden(ef, vl, ent, false), None);
        let Some(num) = turret::ridden(ef, vl, ent, true) else {
            continue;
        };
        mounted += 1;
        let gun = &s.entities[&num];
        let g = |n: &str| gun.field_f32(p, n);
        let arcs_at = UoArcs::FIELDS.map(g);
        let arcs = UoArcs::lerp(arcs_at, arcs_at, 0.0);
        let base = [g("apos.trBase[0]"), g("apos.trBase[1]")];
        let view = s.ps.viewangles(p);
        let shakes = super::seat::gunner_shakes(ef, gun.field_i32(p, "animMovetype"));
        let eye = turret::uo_eye(
            tag,
            view,
            base,
            &arcs,
            shakes.then(|| super::seat::shake_phase(s.server_time)),
            [0.0; 2],
            &mut rng,
        );
        held += usize::from(eye.snap.is_some());
        firing += usize::from(shakes);
        for (i, name) in ["angles2[0]", "angles2[1]"].into_iter().enumerate() {
            let d = vcod_common::pmove::angle_normalize180(eye.angles[i] - base[i] - g(name));
            assert!(
                d.abs() < 0.02,
                "t {}: view {view:?} held at {:?}, gun {base:?} + {}",
                s.server_time,
                eye.angles,
                g(name)
            );
        }
    }
    println!("uo-turret: {mounted} mounted, {held} held, {firing} firing");
    assert!(mounted > 50, "{mounted}");
    assert!(held > 0, "the view never ran past an arc");
    assert!(firing > 20, "{firing}");
}

/// The jeep driver of `*-uo-jeep.bin` through the weapon binds' gates
/// (cod11-hud-protocol.md, "Weapon slots"): the own-view bit is `cg.snap`'s,
/// so on the frames between the first spawned snapshot (14100) and the one
/// before it the binds still refuse; in the driver's seat (from 16150)
/// `weaponslot` refuses on either snapshot's seat fields.
#[test]
fn the_jeep_drivers_binds_wait_for_cg_snap() {
    use super::input::{Action, Held, PlayInput};
    let p = &PROTOCOL_V22;
    let frames: Vec<_> = vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-uo-jeep.bin",
        "net/snapshots-uo-jeep.bin",
    )
    .into_iter()
    .map(|f| f.snapshot)
    .collect();
    let mut late = Vec::new();
    let mut seated = 0;
    for w in frames.windows(2) {
        let (snap, newest) = (&w[0], &w[1]);
        let mut held = Held::from_ps(&newest.ps, p, &[]);
        let ahead = held.own_view;
        held.cg_snap(&newest.ps, &snap.ps, p);
        let mut input = PlayInput::default();
        input.sync(&held);
        if ahead && !input.select_allowed(Action::NextWeapon) {
            late.push(newest.server_time);
        }
        if newest.ps.field_i32(p, "vehPos") == 1 {
            seated += 1;
            assert!(!input.select_allowed(Action::Slot(1)));
            assert!(input.select_allowed(Action::NextWeapon));
        }
    }
    assert_eq!(late, [14100]);
    assert!(seated > 20, "{seated} seated pairs");
}

/// The Panzer IV's three shells strike the seated driver of
/// `*-uo-shellveh.bin`: each new `damageEvent` kicks and flashes once, off
/// the playerstate the frame draws, in a seat where no HUD gun steps.
#[test]
fn a_seated_drivers_hits_each_feed_the_damage_once() {
    let t = replay_capture(&PROTOCOL_V22, "-uo-shellveh", &[]);
    assert_eq!(t.hits, 3);
}

/// The own playerstate of `*-turret-hit.bin` (1.1d, mp_carentan's MG42
/// under `client-probes/probe_turret`, an allied `--save-hit` probe with
/// `m1carbine_mp` shooting the mounted gunner from behind; recipe in
/// cod11-turrets.md 14.3): the first round lands on the mounted gunner
/// (`EV_PAIN` at 23150) and kicks and flashes once on the gun; the second
/// kills him off it at 23350 and feeds no damage.
#[test]
fn a_mounted_gunners_hit_feeds_the_damage_once() {
    let t = replay_capture(&PROTOCOL_V1, "-turret-hit", &[]);
    assert_eq!((t.hits, t.mounted_hits), (1, 1));
}

/// `*-follow-1.1.bin` (1.1d, mp_carentan dm under
/// `client-probes/probe_passthru` with `probe_teleport 1`): a spectator
/// following an allied `--save-hit` shooter whose `m1carbine_mp` puts three
/// rounds into an axis `--probe-target` 200 units ahead and kills him, then
/// following the target through his respawn (recipe in
/// cod11-spectator-follow.md 11.1). Each followed round
/// plays, flashes, sounds and kicks once; with the paks the weapon file's
/// flash and fire sound resolve for every one.
#[test]
fn a_1_1_follow_capture_plays_each_shot_once() {
    let tag = "-follow-1.1";
    let weapons = vcod_common::testing::game_fs_11()
        .map(|fs| {
            let gs = vcod_common::testing::fixture_gamestate(
                &PROTOCOL_V1,
                &format!("net/gamestate{tag}.bin"),
            );
            vcod_common::weapon_table::from_configstring(&fs, &gs.configstrings[7])
        })
        .unwrap_or_default();
    let t = replay_capture(&PROTOCOL_V1, tag, &weapons);
    assert!(t.unknown.is_empty(), "unknown events {:?}", t.unknown);
    assert_eq!(t.doubled, 0, "fire events flashed or sounded twice");
    // 17 rounds and the clip's last (161), a reload (152), 21 impacts (three
    // on the target's torso, 18 on the grass past him), two pains with no
    // sound (cod11-sound-system.md's `EV_PAIN` row) and the kill's
    // obituary. No `EV_NONE`: the switch onto the respawned target
    // (sequence 19 to 0) plays none of his empty slots, as retail's compare
    // (cod11-spectator-follow.md 11.1).
    let expected = BTreeMap::from([(152, 1), (159, 17), (161, 1), (174, 21), (187, 2), (201, 1)]);
    assert_eq!(t.played, expected);
    assert_eq!((t.fires, t.kicks), (18, 18));
    if weapons.is_empty() {
        assert_eq!(t.cues, BTreeMap::from([(174, 21)]));
    } else {
        assert_eq!((t.flashes, t.fire_sounds), (18, 18));
        let cues = BTreeMap::from([(152, 1), (159, 17), (161, 1), (174, 21)]);
        assert_eq!(t.cues, cues);
    }
}

/// Every grenade or rocket explode a capture's entity rings carry, once per
/// missile, against what the client's frame played and how many effects
/// those plays spawned.
fn explodes(p: &'static Cod1Wire, tag: &str, fs: Option<&Pk3Fs>) -> (usize, usize, usize) {
    use vcod_common::net::event_ids::{EV_GRENADE_EXPLODE, EV_ROCKET_EXPLODE};
    let explode = |e: i32| matches!(e, EV_GRENADE_EXPLODE | EV_ROCKET_EXPLODE);
    let gs = vcod_common::testing::fixture_gamestate(p, &format!("net/gamestate{tag}.bin"));
    let mut tracker = EventTracker::new();
    let ctx = ResolveCtx {
        muzzles: &HashMap::new(),
        shot_fallbacks: &HashMap::new(),
        weapon_flash: &HashMap::new(),
        view_flash: None,
        view_body: None,
        blood: true,
        brass: &HashMap::new(),
        weapons: &[],
        eject_brass: false,
        configstrings: &gs.configstrings,
        cs: &p.cs,
    };
    let mut exploded = std::collections::BTreeSet::new();
    let (mut played, mut spawns) = (0, 0);
    for f in vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate{tag}.bin"),
        &format!("net/snapshots{tag}.bin"),
    ) {
        for (&num, e) in &f.snapshot.entities {
            if (0..4).any(|i| explode((p.event_to_v1)(e.field_i32(p, &format!("events[{i}]"))))) {
                exploded.insert(num);
            }
        }
        for ev in tracker.drain(&f.snapshot, p) {
            if explode(ev.event) {
                played += 1;
                if let Some(fs) = fs {
                    spawns += registry::resolve_in(fs, &ev, &ctx)
                        .iter()
                        .filter(|r| matches!(r, Resolved::Spawn { .. }))
                        .count();
                }
            }
        }
    }
    (exploded.len(), played, spawns)
}

/// Retail's own frags and panzerfaust shells, watched: each missile's
/// explode plays once and spawns one effect, the impact table's row for its
/// surface (cod11-events-and-fx.md, "One effect per explode").
#[test]
fn each_retail_explode_plays_one_effect() {
    let fs = vcod_common::testing::game_fs_11();
    for tag in ["-skyfrag", "-corpseblast", "-panzer", "-panzer-wall"] {
        let (missiles, played, spawns) = explodes(&PROTOCOL_V1, tag, fs.as_ref());
        println!("{tag}: {missiles} missiles exploded, {played} played, {spawns} effects");
        assert!(missiles > 0, "{tag}: no explode in the capture");
        assert_eq!(played, missiles, "{tag}: explodes played");
        if fs.is_some() {
            assert_eq!(spawns, missiles, "{tag}: effects spawned");
        }
    }
}

/// The followed gunner of `*-15-turret.bin` (1.5b, mp_carentan's MG42,
/// `client-probes/probe_turret`) through 1.5's turret view (1.5 cgame
/// 0x300348c0). The gun sends its arcs from its first manned frame, and
/// the view held inside them is the barrel the server sent on every
/// mounted snapshot, the playerstate's pitch 20 degrees past the top arc
/// on the mount included; `viewlocked` 2 marks the frames that fired.
#[test]
fn a_followed_1_5_gunner_sees_down_the_gun() {
    use crate::turret::{self, MsvcRand, UoArcs};
    use vcod_common::net::protocol::PROTOCOL_V6;
    let p = &PROTOCOL_V6;
    let (mut mounted, mut held, mut firing) = (0, 0, 0);
    let mut rng = MsvcRand::default();
    for f in vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-15-turret.bin",
        "net/snapshots-15-turret.bin",
    ) {
        let s = &f.snapshot;
        let int = |n: &str| s.ps.field_i32(p, n);
        let (ef, vl, ent) = (int("eFlags"), int("viewlocked"), int("viewlocked_entNum"));
        let Some(num) = turret::ridden(ef, vl, ent, false) else {
            continue;
        };
        mounted += 1;
        let gun = &s.entities[&num];
        let g = |n: &str| gun.field_f32(p, n);
        let arcs_at = UoArcs::FIELDS.map(g);
        assert_eq!(arcs_at, [45.0, -40.0, -45.0, 40.0], "t {}", s.server_time);
        let arcs = UoArcs::lerp(arcs_at, arcs_at, 0.0);
        let base = [g("apos.trBase[0]"), g("apos.trBase[1]")];
        let view = s.ps.viewangles(p);
        let eye = turret::v15_eye(Vec3::ZERO, view, base, &arcs, None, &mut rng);
        held += usize::from(eye.snap.is_some());
        firing += usize::from(vl == 2);
        for (i, name) in ["angles2[0]", "angles2[1]"].into_iter().enumerate() {
            let d = vcod_common::pmove::angle_normalize180(eye.angles[i] - base[i] - g(name));
            assert!(
                d.abs() < 0.02,
                "t {}: view {view:?} held at {:?}, gun {base:?} + {}",
                s.server_time,
                eye.angles,
                g(name)
            );
        }
    }
    println!("15-turret: {mounted} mounted, {held} held, {firing} firing");
    assert!(mounted > 100, "{mounted}");
    assert!(held > 0, "the view never ran past an arc");
    assert!(firing > 50, "{firing}");
}

/// The followed gunner's rounds in `*-15-turret.bin` (1.5b): every world
/// impact names the gun (298), `Bullet_Fire`'s inflictor, never the gunner
/// (cod11-turrets.md 6.3), so a client watching from elsewhere draws the
/// tracer off the gun's own `tag_flash` (1.5 cgame 0x3003ab90).
#[test]
fn a_1_5_turret_rounds_impacts_name_the_gun() {
    use vcod_common::net::flags::ET_TURRET;
    use vcod_common::net::protocol::PROTOCOL_V6;
    let p = &PROTOCOL_V6;
    let mut tracker = EventTracker::new();
    let mut hits = 0;
    for f in vcod_common::testing::fixture_frames(
        p,
        "net/gamestate-15-turret.bin",
        "net/snapshots-15-turret.bin",
    ) {
        for ev in tracker.drain(&f.snapshot, p) {
            if matches!(
                ev.event,
                registry::EV_BULLET_HIT_SMALL | registry::EV_BULLET_HIT_LARGE
            ) {
                let gun = &f.snapshot.entities[&ev.other_entity_num];
                assert_eq!(
                    gun.field_i32(p, "eType"),
                    ET_TURRET,
                    "t {}",
                    f.snapshot.server_time
                );
                hits += 1;
            }
        }
    }
    assert!(hits > 100, "{hits}");
}
