//! `placeSpawnpoint` against retail, to the float's last bit, on every
//! stock map of all three servers.
//!
//! `client-probes/probe_placebits` runs as the gametype on both sides and
//! logs where every spawnpoint a stock gametype drops lands, its height as
//! sign, whole part and 2^-24ths. Retail's lines are
//! `tests/fixtures/spawnpoints/placebits-retail.txt`, whose header carries
//! the recipe. A drop that differs is a player spawned somewhere retail
//! never puts one: the point drop this gate's first version replaced left
//! mp_harbor's entity 241 at z -262144, below the world, and a player
//! spawned there fell for the rest of that life
//! (cod11-gsc-object-model.md 16.1). The low bits matter too: a spawn
//! think's fall from rest ties `PmoveSingle`'s velocity clamp, so they pick
//! the velocity a sloped spawn leaves (cod15-movement.md 4).
//!
//! The rows ours still misses are [`BITS_GAPS`], each pinned to what ours
//! prints.
//!
//! Needs `COD_DIR`; without the paks it returns early. `PLACEBITS_MAP=<map>`
//! runs one map.

use crate::common;

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

const BITS_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_placebits.gsc";
const BITS_FIXTURE: &str = "tests/fixtures/spawnpoints/placebits-retail.txt";

/// The spawnpoints ours places off retail's height, as (server, map,
/// entity, ours' sign, whole part and 2^-24ths). A row fails once ours
/// moves at all, closed or not. cod11-gsc-object-model.md 16.1 has what
/// each group rests on.
const BITS_GAPS: &[(&str, &str, u32, &str)] = &[];

/// The bit-exact capture's `PROBE` lines, keyed by the map of each
/// `## <server> <map>` heading under `server`.
fn bits_retail(server: &str) -> BTreeMap<String, Vec<String>> {
    let text = std::fs::read_to_string(BITS_FIXTURE)
        .unwrap_or_else(|e| panic!("read {BITS_FIXTURE}: {e}"));
    let mut maps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut map: Option<String> = None;
    for line in text.lines().map(str::trim_end) {
        if let Some(head) = line.strip_prefix("## ") {
            let (srv, m) = head.split_once(' ').expect("## <server> <map>");
            map = (srv == server).then(|| m.to_string());
        } else if let (Some(m), true) = (&map, line.starts_with("PROBE ")) {
            maps.entry(m.clone()).or_default().push(line.to_string());
        }
    }
    maps
}

/// `map`'s spawnpoints placed as `probe_placebits` prints them, on `fs`'s
/// install.
fn bits_ours(fs: &Rc<vcod_common::pk3::Pk3Fs>, probe: &str, map: &str) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(fs);
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map(map).unwrap()).unwrap()).unwrap();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_placebits"), Instant::now());
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_placebits", probe);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    sv.script_log()
        .iter()
        .filter(|l| l.starts_with("PROBE class ") || l.starts_with("PROBE placebits "))
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// Every spawnpoint of every stock map on `server`, to the last bit but
/// for [`BITS_GAPS`]. mp_carentan's entity 92 rests on a terrain edge,
/// which 1.5b and UO sweep on frames their own build made
/// (`bsp-ibsp59-format.md`, "Patch collision on 1.5 and UO"); its 128 and
/// 221 on a near tie between two terrain partitions that only retail's
/// leaf order settles ("The leaf walk").
fn check_bits(fs: vcod_common::pk3::Pk3Fs, server: &str, maps: usize) {
    let fs = Rc::new(fs);
    let probe = std::fs::read_to_string(BITS_SRC).unwrap_or_else(|e| panic!("read the probe: {e}"));
    let mut retail = bits_retail(server);
    assert_eq!(retail.len(), maps, "{BITS_FIXTURE}: {server}'s stock maps");
    // `PLACEBITS_MAP=<map>` runs one map, for a diagnosis.
    if let Ok(only) = std::env::var("PLACEBITS_MAP") {
        retail.retain(|m, _| *m == only);
    }
    let mut diffs = Vec::new();
    for (map, retail) in &retail {
        let ours = bits_ours(&fs, &probe, map);
        assert_eq!(ours.len(), retail.len(), "{server} {map}: line count");
        for (o, r) in ours.iter().zip(retail) {
            let tokens = |l: &str| l.split(' ').map(str::to_string).collect::<Vec<_>>();
            let (ot, rt) = (tokens(o), tokens(r));
            let gap = BITS_GAPS
                .iter()
                .find(|g| g.0 == server && g.1 == map && rt[2] == g.2.to_string());
            // A gap keeps the entity, x and y and differs in the height
            // exactly as pinned.
            let fine = match gap {
                None => o == r,
                Some(g) => o != r && ot[..5] == rt[..5] && ot[6..].join(" ") == g.3,
            };
            if !fine {
                diffs.push(format!("{server} {map}: ours {o} / retail {r}"));
            }
        }
    }
    assert!(
        diffs.is_empty(),
        "{} spawnpoints differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

#[test]
fn placespawnpoint_matches_retail_to_the_bit() {
    if let Some(fs) = vcod_common::testing::game_fs_11() {
        check_bits(fs, "1.1d", 12);
    }
}

#[test]
fn placespawnpoint_on_15_matches_retail_to_the_bit() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let v6 = vcod_common::net::protocol::PROTOCOL_V6.transport.version;
    if vcod_server::server::install_protocol(&fs).transport.version == v6 {
        check_bits(fs, "1.5b", 16);
    }
}

#[test]
fn placespawnpoint_on_uo_matches_retail_to_the_bit() {
    if let Some(fs) = vcod_common::testing::uo_fs() {
        check_bits(fs, "uo", 20);
    }
}
