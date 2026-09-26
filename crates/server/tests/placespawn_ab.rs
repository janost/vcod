//! `placeSpawnpoint` against retail, spawnpoint for spawnpoint, on every
//! stock map.
//!
//! `probe_placespawn` runs as the gametype on both sides and logs each
//! `mp_deathmatch_spawn`'s origin before and after the drop. Retail's lines
//! are `tests/fixtures/spawnpoints/placespawn-retail.txt`, whose header
//! carries the recipe. A drop that differs is a player spawned somewhere
//! retail never puts one: the point drop this gate replaced left mp_harbor's
//! entity 241 at z -262144, below the world, and a player spawned there fell
//! for the rest of that life.
//!
//! The drop height is compared to [`TOLERANCE`], not exactly: with the drop
//! a capsule, 65 of the 383 spawnpoints still land up to 2.63 units off
//! retail's floor, and what in our clip that residual comes from is not
//! pinned. The point drop missed by more than that on 17, by 8 units on
//! two, and by the whole world on one.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

const PROBE_PATH: &str = "maps/mp/gametypes/probe_placespawn";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/probe_placespawn.gsc";
const FIXTURE: &str = "tests/fixtures/spawnpoints/placespawn-retail.txt";
/// How far a placed spawnpoint may sit from retail's height, in units.
const TOLERANCE: f32 = 3.0;

/// `PROBE place <num> (x, y, z) (x, y, z)`: the entity and both origins.
fn parse(line: &str) -> Option<(u32, [f32; 3], [f32; 3])> {
    let rest = line.trim_end().strip_prefix("PROBE place ")?;
    let (num, vecs) = rest.split_once(' ')?;
    let v: Vec<f32> = vecs
        .split(['(', ')', ','])
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    let [bx, by, bz, ax, ay, az] = v[..] else {
        return None;
    };
    Some((num.parse().ok()?, [bx, by, bz], [ax, ay, az]))
}

/// The fixture's `PROBE` lines, keyed by the `## <map>` heading above them.
fn retail() -> BTreeMap<String, Vec<String>> {
    let text = std::fs::read_to_string(FIXTURE).unwrap_or_else(|e| panic!("read {FIXTURE}: {e}"));
    let mut maps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut map = None;
    for line in text.lines().map(str::trim_end) {
        if let Some(m) = line.strip_prefix("## ") {
            map = Some(m.to_string());
        } else if line.starts_with("PROBE ") {
            let m = map.clone().expect("a PROBE line before any map heading");
            maps.entry(m).or_default().push(line.to_string());
        }
    }
    maps
}

#[test]
fn placespawnpoint_matches_retail_on_every_stock_map() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let probe =
        std::fs::read_to_string(PROBE_SRC).unwrap_or_else(|e| panic!("read the probe: {e}"));
    let retail = retail();
    assert_eq!(retail.len(), 12, "{FIXTURE} should carry the 12 stock maps");

    let mut diffs = Vec::new();
    for (map, retail_lines) in &retail {
        let bsp_path = fs.resolve_map(map).expect("the map in the mounted paks");
        let bsp =
            vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("the bsp");
        let mut sv = vcod_server::Server::new(common::cfg(map, "probe_placespawn"), Instant::now());
        sv.overlay_script(PROBE_PATH, &probe);
        sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
        sv.load_scripts(fs.clone()).expect("load the scripts");
        let ours: Vec<&String> = sv
            .script_log()
            .iter()
            .filter(|l| l.starts_with("PROBE "))
            .collect();
        assert_eq!(
            ours.len(),
            retail_lines.len(),
            "{map}: ours placed {} spawnpoints, retail {}",
            ours.len(),
            retail_lines.len()
        );
        for (o, r) in ours.iter().zip(retail_lines) {
            let (on, ob, oa) = parse(o).unwrap_or_else(|| panic!("ours: {o:?}"));
            let (rn, rb, ra) = parse(r).unwrap_or_else(|| panic!("retail: {r:?}"));
            // The rendered vector has two decimals, so x and y compare
            // exactly: the drop is vertical and never moves them.
            let same =
                on == rn && ob == rb && oa[..2] == ra[..2] && (oa[2] - ra[2]).abs() <= TOLERANCE;
            if !same {
                diffs.push(format!("{map}: ours {} / retail {r}", o.trim_end()));
            }
        }
    }
    assert!(
        diffs.is_empty(),
        "{} drops differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
