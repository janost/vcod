//! Committed captures through the frame path: their events, sounds,
//! effects and muzzle flashes.

use std::collections::BTreeMap;

use super::*;
use crate::play::cod4::cmd::out_of_ammo_pick;

/// The M16's grenade launcher (2) out of grenades goes back to the
/// rifle (1); an empty rifle cycles to the next primary with rounds,
/// wrapping past the highest index; a launcher whose rifle is gone
/// selects nothing; the dead change nothing.
#[test]
fn cod4_out_of_ammo_picks_as_cg_out_of_ammo_change() {
    let def = |clip_index, ammo_index, inventory_type, alt_weapon| WeaponDef {
        clip_index,
        ammo_index,
        inventory_type,
        alt_weapon,
        ..WeaponDef::default()
    };
    // 1 the rifle, 2 its launcher, 3 a pistol.
    let defs = vec![
        WeaponDef::default(),
        def(1, 1, 0, 2),
        def(2, 2, INVENTORY_ALT_MODE, 1),
        def(3, 3, 0, 0),
    ];
    let mut d = Dry {
        pm_type: 0,
        weapon: 2,
        held: [0b1110, 0, 0, 0],
        ammo: [0; 64],
        ammoclip: [0; 128],
    };
    d.ammoclip[1] = 30;
    d.ammoclip[3] = 12;
    assert_eq!(out_of_ammo_pick(&defs, &d, 2, &[1, 3]), Some(1));
    d.weapon = 3;
    d.ammoclip[3] = 0;
    assert_eq!(out_of_ammo_pick(&defs, &d, 3, &[1, 3]), Some(1), "wraps");
    d.ammoclip[1] = 0;
    assert_eq!(
        out_of_ammo_pick(&defs, &d, 3, &[1, 3]),
        None,
        "nothing has rounds"
    );
    d.weapon = 2;
    d.held = [0b0100, 0, 0, 0];
    assert_eq!(out_of_ammo_pick(&defs, &d, 2, &[]), Some(0));
    d.pm_type = pm::pm_type::DEAD;
    assert_eq!(out_of_ammo_pick(&defs, &d, 2, &[]), None);
}

#[derive(Default, Debug)]
struct Counts {
    pub(super) snapshots: usize,
    pub(super) frames: usize,
    /// Entities drawn, summed over frames, by `eType`.
    pub(super) drawn: BTreeMap<u32, usize>,
    /// The most drawn in one frame.
    pub(super) most: usize,
    submodels: HashSet<usize>,
    pub(super) models: HashSet<String>,
    pub(super) fired: BTreeMap<&'static str, usize>,
    pub(super) lines: Vec<String>,
}

fn replay(name: &str) -> (Cod4Session, Counts) {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    assert_eq!(s.map_name().as_deref(), Some("mp_crash"));
    let mut n = Counts::default();
    let mut ms = 0.0;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        for f in s.take_fired() {
            *n.fired.entry(f.name).or_default() += 1;
        }
        n.lines.extend(s.take_lines());
        let Some(scene) = s.scene() else { continue };
        n.frames += 1;
        n.most = n.most.max(scene.entities.len());
        for d in &scene.entities {
            *n.drawn.entry(d.etype).or_default() += 1;
            match &d.visual {
                Visual::Submodel(m) => {
                    n.submodels.insert(*m);
                }
                Visual::Model(m) => {
                    n.models.insert(m.clone());
                }
                Visual::Box => {}
            }
            assert!(d.origin.is_finite() && d.angles.is_finite());
        }
        assert!(scene.eye.is_finite() && scene.angles.is_finite());
    }
    n.snapshots = s.client().snapshots.len();
    (s, n)
}

/// Events counted straight off the snapshots, without the session:
/// each event entity once per entry, each ring entry once.
fn capture_temp_events(c: &Client) -> usize {
    let mut last: HashSet<u32> = HashSet::new();
    let mut n = 0;
    for s in &c.snapshots {
        let now: HashSet<u32> = s
            .entities
            .iter()
            .filter(|e| e.etype() >= ET_EVENTS)
            .map(|e| e.number())
            .collect();
        n += now.difference(&last).count();
        last = now;
    }
    n
}

/// A capture line that does not decode is skipped: the replay plays to
/// its end with every snapshot (BUG-144).
#[test]
fn a_capture_skips_a_line_that_does_not_decode() {
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = lines.iter().position(|l| l.starts_with("6803 ")).unwrap();
    lines.insert(at, "6803 c2s 127.0.0.1:28960 00".into());
    let mut s = Cod4Session::open_capture(&lines.join("\n")).unwrap();
    let mut ms = 0.0;
    for _ in 0..10_000 {
        if s.finished() {
            break;
        }
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
    }
    assert!(s.finished(), "stalled at {ms} ms");
    assert_eq!(s.client().snapshots.len(), 900);
}

#[test]
fn spectator_capture_replays_through_the_frame_path() {
    let (s, n) = replay("play-1.7-spec.txt");
    let c = s.client();
    assert_eq!(n.snapshots, c.snapshots.len());
    assert_eq!(n.snapshots, 900);
    // The temp entities the capture holds, each fired once.
    let temps: usize = n
        .fired
        .iter()
        .filter(|(k, _)| **k == "EV_EARTHQUAKE")
        .map(|(_, v)| v)
        .sum();
    assert_eq!(temps, capture_temp_events(c));
    // The walker's body: its footsteps, shots and jump as the
    // spectator sees them (cod4-netfields.md 8).
    let fired: Vec<(&str, usize)> = n.fired.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(
        fired,
        [
            ("EV_EARTHQUAKE", 1),
            ("EV_FIRE_WEAPON", 18),
            ("EV_FOOTSTEP_RUN", 10),
            ("EV_GRENADE_BOUNCE", 4),
            ("EV_GRENADE_EXPLODE", 1),
            ("EV_JUMP", 1),
            ("EV_LANDING_ASPHALT", 1),
        ]
    );
    assert_eq!(n.most, 14);
    // Two players, the movers, the frag in flight.
    assert!(n.drawn.contains_key(&ET_PLAYER), "{:?}", n.drawn);
    assert!(n.drawn.contains_key(&ET_SCRIPTMOVER), "{:?}", n.drawn);
    assert!(n.drawn.contains_key(&ET_MISSILE), "{:?}", n.drawn);
    assert!(
        n.models.iter().any(|m| m.starts_with("body_mp_")),
        "{:?}",
        n.models
    );
    assert!(
        n.models.contains("projectile:frag_grenade_mp"),
        "{:?}",
        n.models
    );
    // mp_crash's movers are its destructible cars, xmodels (`solid` 0),
    // not brush models.
    assert!(n.submodels.is_empty());
    assert!(
        n.models
            .iter()
            .any(|m| m.starts_with("vehicle_80s_sedan1_")),
        "{:?}",
        n.models
    );
}

/// Retail's destructible sedans reach the drawn scene with the
/// `partBits` their setup's `hidePart`s left (the ten later-state
/// tags, cod4-client-view.md 15), which the draw hides surfaces by.
#[test]
fn retail_sedans_draw_with_their_hidden_parts() {
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let mut ms = 0.0;
    let mut cars = Vec::new();
    while cars.is_empty() && !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        if let Some(scene) = s.scene() {
            cars = scene
                .entities
                .iter()
                .filter(|d| {
                    matches!(&d.visual, Visual::Model(m) if m.starts_with("vehicle_80s_sedan1_"))
                })
                .map(|d| d.part_bits)
                .collect();
        }
    }
    assert!(!cars.is_empty(), "no sedan drawn");
    for bits in cars {
        assert_eq!(bits, [0x242a, 0x9248_0000, 0, 0], "{bits:08x?}");
    }
}

#[test]
fn kill_capture_draws_the_corpse_and_prints_the_obituary() {
    let (s, n) = replay("play-1.7-kill-target.txt");
    assert_eq!(n.snapshots, s.client().snapshots.len());
    assert_eq!(n.snapshots, 336);
    assert!(n.drawn.contains_key(&ET_PLAYER_CORPSE), "{:?}", n.drawn);
    assert!(n.drawn.contains_key(&ET_ITEM), "{:?}", n.drawn);
    assert!(n.models.contains("weapon:m16_gl_mp"), "{:?}", n.models);
    // Twice: the killcam replays the shooter's frames, the obituary's
    // temp entity among them (cod4-netfields.md 10).
    assert_eq!(n.fired.get("EV_OBITUARY"), Some(&2), "{:?}", n.fired);
    assert_eq!(
        n.fired.get("EV_BULLET_HIT_CLIENT_LARGE"),
        Some(&2),
        "{:?}",
        n.fired
    );
    assert_eq!(n.lines.len(), 2);
    assert_eq!(n.lines[0], n.lines[1]);
    assert_eq!(n.fired.get("EV_FIRE_WEAPON"), Some(&32), "{:?}", n.fired);
    assert_eq!(n.lines[0], "shooter killed target (m16_gl_mp)");
}

/// The spectator capture's walker heard and seen (cod4-sound.md 4,
/// cod4-effects.md 6): its rock and asphalt steps with the gear rattle,
/// the M16's shots and flashes, the frag's bounces and explode, the
/// landing, and the frag's earthquake shaking the view. Every alias and
/// effect resolves.
#[test]
fn spectator_capture_plays_its_sounds_and_effects() {
    let Some(n) = replay_av("play-1.7-spec.txt") else {
        return;
    };
    let count = |k: &str| n.sounds.get(k).copied().unwrap_or(0);
    assert_eq!(count("step_run_rock"), 3, "{:?}", n.sounds);
    assert_eq!(count("step_run_asphalt"), 8, "{:?}", n.sounds);
    assert_eq!(count("gear_rattle_run"), 11, "{:?}", n.sounds);
    assert_eq!(count("land_asphalt"), 1, "{:?}", n.sounds);
    assert_eq!(count("weap_m16_fire_npc"), 18, "{:?}", n.sounds);
    assert!(n.sounds_missing.is_empty(), "{:?}", n.sounds_missing);
    assert!(n.effects_missing.is_empty(), "{:?}", n.effects_missing);
    // The frag's explode on the entity that enters with it.
    assert_eq!(count("grenade_explode_concrete"), 1, "{:?}", n.sounds);
    assert_eq!(
        n.effects.get("explosions/grenadeexp_concrete_1.efx"),
        Some(&1),
        "{:?}",
        n.effects
    );
    assert_eq!(n.effects.values().sum::<usize>(), 23, "{:?}", n.effects);
    assert_eq!(n.quakes.len(), 1);
    let q = n.quakes[0];
    assert_eq!(
        (q.scale, q.radius, q.duration_ms),
        (0.3, 400.0, 500.0),
        "{q:?}"
    );
}

/// The walker's shots flash on its rifle's `tag_flash` and eject from
/// its `tag_brass`, off the posed body (cod4-effects.md 10).
#[test]
fn spectator_capture_flashes_on_the_weapon_tags() {
    use crate::fx::sim::SpawnAt;
    use crate::play::cod4_players::Cod4Players;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (_, models, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let ((_, mut effects), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
    let mut players = Cod4Players::default();
    let (mut flashes, mut ejects) = (Vec::new(), 0);
    let mut ms = 0.0;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let fired = s.take_fired();
        let Some(scene) = s.scene() else { continue };
        let gs = s.gamestate().unwrap();
        let weapons = weapon_list(gs);
        let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
        for f in fired.iter().filter(|f| f.name == "EV_FIRE_WEAPON") {
            let body = scene.entities.iter().find(|d| Some(d.number) == f.entity);
            for p in effects.plays(f, &weapons, &cs, &players.tags, None) {
                let k = p.key;
                match p.at {
                    SpawnAt::Oriented { pos, axis } if k.starts_with("muzzleflashes/") => {
                        flashes.push((pos, axis[0], body.map(|b| (b.origin, b.angles.y))))
                    }
                    SpawnAt::Oriented { .. } if k.starts_with("shellejects/") => ejects += 1,
                    _ => {}
                }
            }
        }
        let views: Vec<(u32, &BodyView)> = scene
            .entities
            .iter()
            .filter_map(|d| Some((d.number, d.body.as_ref()?)))
            .collect();
        players.posed(
            &src,
            &models,
            &views,
            &[],
            scene.render_time,
            scene.server_time,
        );
    }
    println!(
        "{} flashes on the tag, {ejects} ejects: {flashes:?}",
        flashes.len()
    );
    // All 18 of the walker's shots, its rifle posed in each.
    assert_eq!(flashes.len(), 18);
    assert_eq!(ejects, 18);
    for (pos, forward, body) in flashes {
        let (origin, yaw) = body.unwrap();
        let off = pos - origin;
        // Out at the muzzle, ahead of the body along its yaw, below
        // the old 56-unit guess at the eye.
        let ahead = glam::Vec2::new(yaw.to_radians().cos(), yaw.to_radians().sin());
        assert!(off.truncate().dot(ahead) > 15.0, "{off:?} yaw {yaw}");
        assert!((10.0..60.0).contains(&off.z), "{off:?}");
        assert!(
            forward.truncate().normalize().dot(ahead) > 0.7,
            "{forward:?} {yaw}"
        );
    }
}

/// The walker's flashes and ejects ride its rifle's tags while they
/// play (cod4-effects.md 10): each play names the walker's
/// `tag_flash` / `tag_brass` bolt, and through the fx system the
/// flash's elements that move with the effect stay on the tag as the
/// body moves on, frame after frame.
#[test]
fn spectator_capture_flashes_follow_the_tag() {
    use crate::cod4_fx::slot;
    use crate::fx::sim::{FxSystem, TagBolt};
    use crate::play::cod4_players::Cod4Players;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (_, models, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let ((_, mut effects), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
    let fs = vcod_common::pk3::Pk3Fs::empty();
    let mut fx = FxSystem::new();
    for (k, v) in effects.take_effects().unwrap() {
        fx.insert(&k, v);
    }
    let mut players = Cod4Players::default();
    let (mut bolted, mut walker) = (Vec::new(), None);
    // Over the frames a flash rides the tag, the farthest the nearest
    // sprite sat from the tag's pose this frame, and the frames.
    let (mut worst, mut riding) = (0f32, 0);
    let mut ms = 0.0;
    let mut now = 0.0f64;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        now += 1.0 / 60.0;
        s.step(ms).unwrap();
        let fired = s.take_fired();
        let Some(scene) = s.scene() else { continue };
        let gs = s.gamestate().unwrap();
        let weapons = weapon_list(gs);
        let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
        for f in fired.iter().filter(|f| f.name == "EV_FIRE_WEAPON") {
            walker = f.entity;
            for p in effects.plays(f, &weapons, &cs, &players.tags, None) {
                bolted.push(p.bolt);
                effects.start(&mut fx, &fs, &p, now, None);
            }
        }
        let views: Vec<(u32, &BodyView)> = scene
            .entities
            .iter()
            .filter_map(|d| Some((d.number, d.body.as_ref()?)))
            .collect();
        players.posed(
            &src,
            &models,
            &views,
            &[],
            scene.render_time,
            scene.server_time,
        );
        effects.set_tags(&mut fx, &players.tags, None);
        effects.run_loops(&mut fx, &fs, now);
        fx.step(1.0 / 60.0, now, None);
        fx.spawn_chained(&fs, now);
        let ridden = fx.tag_bolts();
        if let Some(w) = walker
            && let Some(tag) = players.tags.get(&w).and_then(|t| t.flash)
            && ridden.contains(&TagBolt {
                entity: w,
                slot: slot::FLASH,
            })
        {
            // The nearest drawn sprite: the flash's own, on the tag.
            let near = fx
                .build_quads(scene.eye, glam::Vec3::Y, glam::Vec3::Z, now)
                .iter()
                .map(|q| {
                    let c = q
                        .verts
                        .iter()
                        .map(|v| glam::Vec3::from(*v))
                        .sum::<glam::Vec3>();
                    (c / 4.0 - tag.0).length()
                })
                .fold(f32::MAX, f32::min);
            worst = worst.max(near);
            riding += 1;
        }
    }
    println!("bolts {bolted:?}, {riding} frames riding, worst {worst}");
    let flash = Some(TagBolt {
        entity: walker.unwrap(),
        slot: slot::FLASH,
    });
    let brass = Some(TagBolt {
        entity: walker.unwrap(),
        slot: slot::BRASS,
    });
    assert_eq!(bolted.iter().filter(|b| **b == flash).count(), 18);
    assert_eq!(bolted.iter().filter(|b| **b == brass).count(), 18);
    assert!(riding >= 18, "{riding}");
    assert!(worst < 12.0, "{worst}");
}
