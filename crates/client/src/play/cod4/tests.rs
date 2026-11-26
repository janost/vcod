//! The replay gate: the committed captures through the session's frame
//! path (step, events, scene) at 60 frames a second of the capture's
//! clock, as `cod4_client` runs them.

use super::*;
use std::collections::BTreeMap;

use super::scene::crosshair_scan;

use super::live::*;
use crate::harness::*;

mod hud_menus;

mod capture;
mod capture_bodies;
mod capture_bullets;
mod capture_view;
mod cod4x;
mod demo;
mod join;
mod live_hud;
mod live_play;
mod live_rounds;
mod live_session;
mod live_spectate;
mod live_streaks;
mod live_view;
mod ragdoll;
mod voice;
mod www;

/// A server command whose sequence leaps past the reliable ring
/// drops the session as `EXE_ERR_RELIABLE_CYCLED_OUT`, the gap never
/// walked.
#[test]
fn a_command_past_the_reliable_ring_drops_the_session() {
    use vcod_cod4::net::parse::{Gamestate, SVC_EOF, SVC_SERVERCOMMAND, UNUSED_HUFF};
    use vcod_common::net::codec::MsgWriter;
    let huff = vcod_cod4::net::huffman::Huffman::new();
    let gs = Gamestate {
        server_command_sequence: 3,
        ..Default::default()
    };
    let opening = vcod_cod4::net::demo::gamestate_message(&huff, 0, 3, &gs);
    let mut demo = vec![0];
    demo.extend(1i32.to_le_bytes());
    demo.extend((opening.len() as i32).to_le_bytes());
    demo.extend(&opening);
    let mut s = Cod4Session::play_demo(demo, false).unwrap();
    let command = |seq: i32| {
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        w.write_byte(SVC_SERVERCOMMAND);
        w.write_long(seq);
        w.write_string_bytes(b"x");
        w.write_byte(SVC_EOF);
        huff.compress(&w.into_ops())
    };
    let feed = |s: &mut Cod4Session, seq: i32| {
        let Source::Demo(d) = &mut s.source else {
            unreachable!()
        };
        d.client.parse(2, &command(seq)).unwrap();
        s.queue_commands()
    };
    assert_eq!(feed(&mut s, 4), [b"x".to_vec()]);
    assert!(s.take_dropped().is_none());
    assert!(feed(&mut s, i32::MAX).is_empty());
    assert_eq!(
        s.take_dropped().as_deref(),
        Some("EXE_ERR_RELIABLE_CYCLED_OUT")
    );
}

/// `CG_ScanForCrosshairEntity`'s trace: a client under 64 the point
/// met, a teammate by team, none while we spectate, nothing on the
/// world.
#[test]
fn the_crosshair_scan_names_the_client_it_meets() {
    struct Hits(u32);
    impl vcod_cod4::pmove::PmWorld for Hits {
        fn trace(&self, start: Vec3, end: Vec3, _: Vec3, _: Vec3, pass: u32, mask: u32) -> pm::Tr {
            assert_eq!((pass, mask), (2, 0x280_3001));
            // straight ahead along +x, 8192 out
            assert!((end - start - Vec3::X * 8192.0).length() < 0.01);
            pm::Tr {
                entity_num: self.0,
                ..Default::default()
            }
        }
    }
    let team = |n: u32| match n {
        2 | 7 => 1,
        _ => 2,
    };
    let at = |_| Some(Vec3::new(300.0, 0.0, 60.0));
    let eye = Vec3::new(0.0, 0.0, 60.0);
    let scan = |hit, own| crosshair_scan(&Hits(hit), eye, Vec3::ZERO, own, team, at);
    let enemy = scan(9, 2).unwrap();
    assert_eq!((enemy.client, enemy.friendly), (9, Some(false)));
    assert_eq!(enemy.dist_sq, 90_000.0);
    assert_eq!(scan(7, 2).unwrap().friendly, Some(true));
    assert_eq!(scan(1022, 2), None);
    assert_eq!(scan(64, 2), None);
    let spectating = |n: u32| if n == 2 { 3 } else { 1 };
    let s = crosshair_scan(&Hits(9), eye, Vec3::ZERO, 2, spectating, at).unwrap();
    assert_eq!(s.friendly, None);
}

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod4/tests/fixtures/net");

/// What the events of a capture play (`cod4_audio`, `cod4_fx`).
#[derive(Default, Debug)]
struct AvCounts {
    /// Cues by alias, and the aliases no zone holds.
    sounds: BTreeMap<String, usize>,
    sounds_missing: std::collections::BTreeSet<String>,
    /// Effects by name, and the names no zone holds.
    effects: BTreeMap<String, usize>,
    effects_missing: std::collections::BTreeSet<String>,
    quakes: Vec<crate::play::quake::Quake>,
}

/// `name` through the session's frame path with the zones' sounds and
/// effects (`COD4_DIR`; `None` without it): each fired event's cues and
/// effects, counted as `cod4_client` plays them.
fn replay_av(name: &str) -> Option<AvCounts> {
    let install = vcod_cod4::testing::cod4_install()?;
    let ((mut sounds, mut effects), _) =
        crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let mut n = AvCounts::default();
    let mut ms = 0.0;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let fired = s.take_fired();
        let Some(gs) = s.gamestate() else { continue };
        let weapons = weapon_list(gs);
        let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
        for f in &fired {
            for c in sounds.cues(f, &weapons, &cs) {
                *n.sounds.entry(c.alias).or_default() += 1;
            }
            for p in effects.plays(f, &weapons, &cs, &HashMap::new(), None) {
                *n.effects.entry(p.key).or_default() += 1;
            }
            n.quakes.extend(crate::cod4_fx::earthquake(f));
        }
    }
    n.sounds_missing = sounds.missing.iter().cloned().collect();
    n.effects_missing = effects.missing.iter().cloned().collect();
    assert_eq!(sounds.started, n.sounds.values().sum::<usize>());
    assert_eq!(effects.spawned, n.effects.values().sum::<usize>());
    println!("{name}: {n:#?}");
    println!("element types left out: {:?}", effects.gaps);
    Some(n)
}

/// Per shot the hit's distance and surface, the impacts by name, the
/// whizbys.
type Bullets = (Vec<(f32, u32)>, BTreeMap<String, usize>, usize);

/// The first round of [`crate::cod4_fx::client_bullets`].
fn first_bullet(
    f: &Fired,
    weapons: &[String],
    defs: &HashMap<String, crate::cod4_audio::WeaponAv>,
    tags: &HashMap<u32, crate::play::cod4_players::WeaponTags>,
    own: Option<&crate::cod4_fx::OwnView>,
    clip: &vcod_cod4::collision::ClipMap,
    solids: &[Solid],
) -> Option<crate::cod4_fx::BulletHit> {
    let world = crate::cod4_fx::ShotWorld { clip, solids };
    crate::cod4_fx::client_bullets(f, weapons, defs, tags, own, world)
        .into_iter()
        .next()
}
