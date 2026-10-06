//! The whole configstring table, diffed against a retail capture slot by
//! slot on four maps. Retail assigns the precache blocks in call order, so a
//! match proves the script load order, the bootstrap sequence and the
//! execution order inside each `main()`, none of which a semantic test sees.
//!
//! The captures are from `tools/run_server.sh` with `g_gametype dm` (one each
//! under `sd`, `re` and `bel`),
//! `sv_maxclients 8`, `sv_pure 0` and stock `scr_*` defaults; the fixture
//! headers name them. Change any of those and the fixtures need retaking,
//! because cvar defaults move slots.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use std::collections::BTreeMap;

/// Slots the diff never covers. Neither is script output: 0 carries the
/// hostname and `sv_maxclients` this run was configured with, 1 the pak
/// checksum lists. `crates/server/tests/connect.rs` covers both.
const STRUCTURAL_SKIP: &[usize] = &[0, 1];

/// Slots stage 3 knowingly does not reproduce, each with the reason. Empty
/// is the goal. A gap that starts matching fails the guard below, so this
/// list cannot rot into a lie.
const GAPS: &[(usize, &str)] = &[];

fn cfg(map: &str, gametype: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        // The capture was taken at 8. `sv_maxclients` reaches the script
        // through the cvar table, so it has to match or the diff moves for
        // a reason that is not a bug.
        max_clients: 8,
        gametype: gametype.into(),
        test_entities: 0,
        trace: false,
        bots: 0,
        bots_shoot: false,
    }
}

/// The committed retail capture, indexed by slot. The fixture name carries
/// the gametype, so it comes from `cfg` rather than from a literal: a
/// changed `cfg().gametype` has to move the fixture too, not silently keep
/// reading the `dm` capture.
fn retail(map: &str, gametype: &str) -> BTreeMap<usize, String> {
    let path = format!("tests/fixtures/configstrings/{map}-{gametype}.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let (i, v) = l.split_once(' ').unwrap_or((l, ""));
            let i: usize = i.parse().unwrap_or_else(|_| panic!("bad index line: {l}"));
            (i, v.to_string())
        })
        .collect()
}

/// Our table right after the scripts have loaded and every thread has
/// reached its first wait, then `frames` server frames on. Retail's was read
/// at a client's gamestate, seconds in, so a script that writes past its
/// first wait (`re`'s `setHintString` sits behind a `wait 0`) needs the
/// frames to have run.
fn ours(
    map: &str,
    gametype: &str,
    fs: vcod_common::pk3::Pk3Fs,
    frames: u32,
) -> BTreeMap<usize, String> {
    let fs = std::rc::Rc::new(fs);
    let start = std::time::Instant::now();
    let mut sv = vcod_server::server::Server::new(cfg(map, gametype), start);
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    for i in 1..=frames {
        sv.tick(start + std::time::Duration::from_millis(50 * u64::from(i)));
    }
    (0..2048)
        .filter(|i| !sv.configstring(*i).is_empty())
        .map(|i| (i, sv.configstring(i).to_string()))
        .collect()
}

fn check(map: &str, gametype: &str) {
    check_after(map, gametype, 0);
}

fn check_after(map: &str, gametype: &str, frames: u32) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let retail = retail(map, gametype);
    let ours = ours(map, gametype, fs, frames);
    let skip = |i: usize| STRUCTURAL_SKIP.contains(&i) || GAPS.iter().any(|(g, _)| *g == i);

    let mut diffs = Vec::new();
    let mut slots: Vec<usize> = retail.keys().chain(ours.keys()).copied().collect();
    slots.sort_unstable();
    slots.dedup();
    for i in slots {
        if skip(i) {
            continue;
        }
        match (retail.get(&i), ours.get(&i)) {
            (Some(r), Some(o)) if r == o => {}
            (Some(r), Some(o)) => diffs.push(format!("cs[{i}] retail {r:?} ours {o:?}")),
            (Some(r), None) => diffs.push(format!("cs[{i}] retail {r:?} ours unset")),
            (None, Some(o)) => diffs.push(format!("cs[{i}] retail unset ours {o:?}")),
            (None, None) => unreachable!("slot came from one of the two maps"),
        }
    }
    assert!(
        diffs.is_empty(),
        "{map} {gametype}: {} configstring(s) differ from retail\n{}",
        diffs.len(),
        diffs.join("\n")
    );

    // A gap that starts matching is a gap that should have been deleted.
    for (i, why) in GAPS {
        assert_ne!(
            retail.get(i),
            ours.get(i),
            "cs[{i}] now matches retail; drop it from GAPS ({why})"
        );
    }
}

#[test]
fn the_configstring_table_matches_retail_on_mp_pavlov() {
    check("mp_pavlov", "dm");
}

/// A second map with a much larger precache set (131 models against 93) and
/// the other nationality branch: `mp_pavlov` is `team_russiangerman`,
/// `mp_carentan` is `team_americangerman`, so the pair catches a bootstrap
/// that hardcodes either one.
#[test]
fn the_configstring_table_matches_retail_on_mp_carentan() {
    check("mp_carentan", "dm");
}

/// The other stock gametype the map-cycle gates run under, whose bootstrap
/// precaches a different string set and only when `game["gamestarted"]` is
/// unset (`tests/restart_carry.rs` is what checks the restart after it).
#[test]
fn the_configstring_table_matches_retail_on_mp_carentan_sd() {
    check("mp_carentan", "sd");
}

/// `re` on the one map whose script fogs with `setExpFog`: the bootstrap's
/// string set, configstring 12's exponent-form density and the two
/// objectives' `setHintString` slots behind the engine's own 1212/1213.
#[test]
fn the_configstring_table_matches_retail_on_mp_chateau_re() {
    check_after("mp_chateau", "re", 2);
}

/// `bel`, whose bootstrap runs `_teams` against `team_germanonly` and whose
/// `_gameobjects` pass keeps only the `bel` objects mp_brecourt places.
#[test]
fn the_configstring_table_matches_retail_on_mp_brecourt_bel() {
    check_after("mp_brecourt", "bel", 2);
}
