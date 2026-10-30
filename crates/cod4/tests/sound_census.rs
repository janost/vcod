//! A 1.7 install's MP sound aliases through `vcod_cod4::sound`
//! (docs/research/cod4-sound.md): every alias of the zones a client loads
//! for mp_crash names a file it can play, loaded from a zone (as PCM) or
//! streamed from the iwds' `sound/`, on a channel `channels.def` lists.
//! Vacuous without `COD4_DIR`.

use std::collections::BTreeMap;
use vcod_cod4::assets::Assets;
use vcod_cod4::sound::{AliasFile, SoundSet, wav};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;
use vcod_common::asset_source::AssetSource;
use vcod_common::pk3::Pk3Fs;

fn zone(name: &str) -> Option<Assets> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", name)).unwrap();
    Some(walk(z, WalkOptions { keep_bulk: true }).unwrap().assets)
}

/// The zones a 1.7 client loads for an MP map, in load order
/// (cod4-handshake.md 1).
const MP_ZONES: [&str; 4] = [
    "code_post_gfx_mp",
    "localized_code_post_gfx_mp",
    "localized_common_mp",
    "mp_crash",
];

#[test]
fn mp_crash_aliases_all_name_a_playable_file() {
    let Some(install) = cod4_install() else {
        return;
    };
    let mut set = SoundSet::default();
    for z in MP_ZONES {
        set.add(&zone(z).unwrap());
    }
    let fs = Pk3Fs::open_iwd(&install.root().join("main"), None).unwrap();
    assert_eq!(set.channels.len(), 33);
    assert_eq!(set.channels[11].name, "body");
    assert_eq!(set.channels[24].name, "ambient");
    let (mut loaded, mut streamed) = (0, 0);
    let mut missing: BTreeMap<String, String> = BTreeMap::new();
    for (name, rows) in &set.aliases {
        for r in rows {
            assert!(r.channel() < set.channels.len(), "{name}: {:#x}", r.flags);
            match &r.file {
                AliasFile::Loaded(f) => {
                    assert!(!r.streamed(), "{name}");
                    match set.wav(f) {
                        Some(Ok(w)) => assert!(w.len() > 44, "{name}: {f}"),
                        Some(Err(e)) => panic!("{name}: {e:#}"),
                        None => {
                            missing.insert(name.clone(), f.clone());
                        }
                    }
                    loaded += 1;
                }
                AliasFile::Streamed(f) => {
                    assert!(r.streamed(), "{name}");
                    if !fs.exists(&format!("sound/{f}")) {
                        missing.insert(name.clone(), f.clone());
                    }
                    streamed += 1;
                }
            }
        }
    }
    println!(
        "{} lists, {loaded} loaded rows, {streamed} streamed, {} loaded sounds, missing {missing:?}",
        set.aliases.len(),
        set.loaded.len()
    );
    assert!(set.aliases.len() > 2900, "{}", set.aliases.len());
    // The footsteps, the shots and the map's ambient a spectator hears.
    for a in [
        "step_run_asphalt",
        "weap_m16_fire_npc",
        "ambient_crash",
        "land_rock",
    ] {
        assert!(set.alias(a).is_some(), "{a}");
    }
    let amb = &set.alias("ambient_crash").unwrap()[0];
    assert!(amb.looping() && amb.streamed());
    assert_eq!(set.channels[amb.channel()].name, "ambient");
    assert!(missing.is_empty(), "{missing:?}");
    assert_eq!(set.skipped, 0);
    // Every loaded sound with data is 16-bit PCM.
    for l in set.loaded.values() {
        wav(l).unwrap();
    }
}

/// 1.7's master volume: `snd_volume` registers at 0.8 (the float at
/// iw3mp.exe 0x6cdd4c, `SND_Init`'s `Dvar_RegisterFloat` at 0x5c6be0..0x5c6c3e,
/// range 0..1, archived) and the master is it times the double at
/// 0x70b0c8, 0.75 (0x5c4fb6, 0x5c6ff3). The bytes of the install's
/// `iw3mp.exe` (cod4-sound.md 5).
#[test]
fn master_volume_is_snd_volume_times_three_quarters() {
    let Some(install) = cod4_install() else {
        return;
    };
    let Ok(exe) = std::fs::read(install.root().join("iw3mp.exe")) else {
        return;
    };
    // `.rdata` at VA 0x691000 sits at file offset 0x291000.
    let at = |va: usize| va - 0x691000 + 0x291000;
    let o = at(0x6fa0f4);
    assert_eq!(&exe[o..o + 11], b"snd_volume\0", "not the 1.7 exe");
    let o = at(0x6cdd4c);
    assert_eq!(f32::from_le_bytes(exe[o..o + 4].try_into().unwrap()), 0.8);
    let o = at(0x70b0c8);
    assert_eq!(f64::from_le_bytes(exe[o..o + 8].try_into().unwrap()), 0.75);
}

/// What the falloff and the speaker fields of the mp_crash zones' rows
/// hold (cod4-sound.md 2, 5): the curves, the speaker maps and the LFE and
/// centre sends, printed; the asserts pin what vcod plays off them.
#[test]
fn mp_crash_falloff_and_speaker_fields() {
    if cod4_install().is_none() {
        return;
    }
    let mut set = SoundSet::default();
    for z in MP_ZONES {
        set.add(&zone(z).unwrap());
    }
    let mut curves: BTreeMap<String, usize> = BTreeMap::new();
    let mut maps: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let (mut lfe, mut center, mut rows, mut no_curve) = (0, 0, 0, 0);
    for rs in set.aliases.values() {
        for r in rs {
            rows += 1;
            match &r.curve {
                Some(k) => *curves.entry(format!("{k:?}")).or_default() += 1,
                None => no_curve += 1,
            }
            // On a stereo output (index 0): every map sends a mono source
            // half to each front speaker, and a stereo one whole, but
            // `amb_rear`, which halves it and rides only 2D rows.
            if let Some(m) = &r.speaker_map {
                let lv = |s: usize, k: usize| {
                    m.maps[s][0].iter().map(|l| l.levels[k]).collect::<Vec<_>>()
                };
                assert_eq!(
                    (lv(0, 0), lv(0, 1)),
                    (vec![0.5, 0.5], vec![0.0, 0.0]),
                    "{}",
                    m.name
                );
                let full = if m.name == "amb_rear" { 0.5 } else { 1.0 };
                assert_eq!(
                    (lv(1, 0), lv(1, 1)),
                    (vec![full, 0.0], vec![0.0, full]),
                    "{}",
                    m.name
                );
                if m.name == "amb_rear" {
                    assert!(!set.channels[r.channel()].is3d, "{}", r.name);
                }
            }
            lfe += usize::from(r.lfe != 0.0);
            center += usize::from(r.center != 0.0);
            let key = r
                .speaker_map
                .as_ref()
                .map_or("-".to_string(), |m| m.name.clone());
            let e = maps.entry(key).or_insert_with(|| {
                (
                    0,
                    r.speaker_map.as_ref().map_or(String::new(), |m| {
                        format!("{:?} {:?}", m.is_default, m.maps)
                    }),
                )
            });
            e.0 += 1;
        }
    }
    println!("{rows} rows, {no_curve} without a curve, lfe {lfe}, centre {center}");
    for (k, n) in &curves {
        println!("curve {n}: {k}");
    }
    for (k, (n, m)) in &maps {
        println!("speaker map {k} ({n}): {m}");
    }
    // Rows whose curve handle names one no walked zone carries play
    // linear, the stock default's two knots.
    assert!(no_curve * 50 < rows, "{no_curve} of {rows}");
    assert_eq!(maps.get("amb_rear").map(|m| m.0), Some(17));
}
