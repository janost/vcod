//! The configstring table a protocol 6 server builds on a 1.5 install,
//! diffed slot by slot against the 1.5b server's gamestate in
//! `crates/common/tests/fixtures/net/gamestate-1.5.bin` (mp_carentan dm,
//! `sv_hostname CoDHost`, `sv_maxclients 8`, `sv_pure 0`, stock `scr_*`;
//! docs/research/cod15-protocol-diff.md 7 and "Server side").
//!
//! Needs `COD_DIR` at a 1.5 install; on a 1.1 one (the suite's usual) or
//! without paks it returns early.

use std::collections::BTreeMap;
use vcod_common::net::protocol::PROTOCOL_V6;

fn retail() -> BTreeMap<usize, String> {
    use vcod_common::net::{gamestate, huffman::Huffman, msg::MsgReader};
    let data = vcod_common::testing::fixture("net/gamestate-1.5.bin");
    let huff = Huffman::new();
    let mut r = MsgReader::new(&data[4..], &huff);
    let gs = gamestate::parse(&mut r, &PROTOCOL_V6).expect("the 1.5b gamestate");
    gs.configstrings
        .into_iter()
        .enumerate()
        .filter(|(_, s)| !s.is_empty())
        .collect()
}

fn ours(fs: vcod_common::pk3::Pk3Fs) -> BTreeMap<usize, String> {
    ours_with(fs, &[], None, false)
}

/// The `+set`s and worldspawn key `mp_carentan-dm-motd-15.txt` was
/// captured under (its header has the recipe; configstrings_ab.rs has the
/// 1.1 pair).
const MOTD_SETS: &[(&str, &str)] = &[
    ("g_motd", "Welcome to vcod"),
    ("scr_motd", "scr motd text"),
    ("sv_hostname", "vcod motd host"),
    ("scr_allow_vote", "0"),
    ("sv_privateClients", "2"),
];
const MOTD_MESSAGE: &str = "vcod worldspawn message";

/// Ours with `sets` ahead of the load, the worldspawn carrying `message`
/// when given, and with `replay` the sets run again as console lines after
/// the load and one frame on, as retail's command line runs a `+set` placed
/// after `+map`.
fn ours_with(
    mut fs: vcod_common::pk3::Pk3Fs,
    sets: &[(&str, &str)],
    message: Option<&str>,
    replay: bool,
) -> BTreeMap<usize, String> {
    let bsp_path = fs
        .resolve_map("mp_carentan")
        .expect("map in the mounted paks");
    let mut bsp_bytes = fs.read(&bsp_path).unwrap();
    if let Some(m) = message {
        bsp_bytes = vcod_common::testing::bsp_with_worldspawn_key(&bsp_bytes, "message", m);
        fs.overlay(&bsp_path, bsp_bytes.clone());
    }
    let fs = std::rc::Rc::new(fs);
    let start = std::time::Instant::now();
    let cfg = vcod_server::ServerConfig {
        map: "mp_carentan".into(),
        hostname: "CoDHost".into(),
        max_clients: 8,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
        bots: 0,
        bots_shoot: false,
    };
    let mut sv = vcod_server::server::Server::new(cfg, start);
    sv.set_protocol(&PROTOCOL_V6);
    for (k, v) in sets {
        sv.set_cvar(k, v);
    }
    let bsp = vcod_common::bsp::parse(&bsp_bytes).unwrap();
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    if replay {
        for (k, v) in sets {
            sv.push_set(k, v);
        }
        sv.drain_console(start);
        sv.tick(start + std::time::Duration::from_millis(50));
    }
    (0..2048)
        .filter(|i| !sv.configstring(*i).is_empty())
        .map(|i| (i, sv.configstring(i).to_string()))
        .collect()
}

/// The slots of `retail` and `ours` that differ, `skip` left out.
fn diff(retail: &BTreeMap<usize, String>, ours: &BTreeMap<usize, String>, skip: &[usize]) {
    let mut slots: Vec<usize> = retail.keys().chain(ours.keys()).copied().collect();
    slots.sort_unstable();
    slots.dedup();
    let diffs: Vec<String> = slots
        .into_iter()
        .filter(|i| !skip.contains(i) && retail.get(i) != ours.get(i))
        .map(|i| {
            format!(
                "cs[{i}] retail {:?} ours {:?}",
                retail.get(&i),
                ours.get(&i)
            )
        })
        .collect();
    assert!(
        diffs.is_empty(),
        "{} configstring(s) differ from 1.5b\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// The 1.5 install, `None` at any other.
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    (vcod_server::server::install_protocol(&fs).transport.version == PROTOCOL_V6.transport.version)
        .then_some(fs)
}

#[test]
fn the_configstring_table_matches_15b_on_mp_carentan() {
    let Some(fs) = fs_15() else {
        return;
    };
    diff(&retail(), &ours(fs), &[]);
}

/// Every cvar-backed slot a `+set` reaches, non-empty, and 4 from a
/// worldspawn `message`: 0's hostname and private slots, 14 `g_motd`
/// (cod11-map-cycle.md 4.5) and the mirror's `ui_motd`, which carries
/// `scr_motd` (186/250). 1, which lists the capture's extra pak, stays out.
#[test]
fn the_configstring_table_matches_15b_with_a_motd_and_a_message() {
    let Some(fs) = fs_15() else {
        return;
    };
    let path = "tests/fixtures/configstrings/mp_carentan-dm-motd-15.txt";
    let retail: BTreeMap<usize, String> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let (i, v) = l.split_once(' ').unwrap_or((l, ""));
            (i.parse().unwrap(), v.to_string())
        })
        .collect();
    for (i, v) in [(4, MOTD_MESSAGE), (14, "Welcome to vcod")] {
        assert_eq!(
            retail.get(&i).map(String::as_str),
            Some(v),
            "fixture cs[{i}]"
        );
    }
    diff(
        &retail,
        &ours_with(fs, MOTD_SETS, Some(MOTD_MESSAGE), true),
        &[1],
    );
}
