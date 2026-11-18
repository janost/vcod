//! The configstring table a protocol 22 server builds on a United Offensive
//! install, diffed slot by slot against the retail UO 1.51b server's
//! gamestate in `crates/common/tests/fixtures/net/gamestate-uo.bin`
//! (mp_cassino dm, `sv_hostname CoDHost`, `sv_maxclients 8`, `sv_pure 0`;
//! docs/research/coduo-diff.md 7 and "Server"), and against the
//! `--save-configstrings --capture-tag uo` tables in
//! `tests/fixtures/configstrings/<map>-<gametype>-uo.txt` for UO's own
//! gametypes (coduo-diff.md 22).
//!
//! Needs `COD_DIR` at a UO install (`uo/` beside `main/`); without one it
//! returns early. The `-uo-pure` tables are a `sv_pure 1` server on that
//! install with the third-party `nsf_mp_j5outpost.pk3` added to `main/`
//! (coduo-diff.md 24); without that pak their gate returns early.

use std::collections::BTreeMap;
use vcod_common::net::protocol::PROTOCOL_V22;

fn retail() -> BTreeMap<usize, String> {
    use vcod_common::net::{gamestate, huffman::Huffman, msg::MsgReader};
    let data = vcod_common::testing::fixture("net/gamestate-uo.bin");
    let huff = Huffman::new();
    let mut r = MsgReader::new(&data[4..], &huff);
    let gs = gamestate::parse(&mut r, &PROTOCOL_V22).expect("the UO gamestate");
    gs.configstrings
        .into_iter()
        .enumerate()
        .filter(|(_, s)| !s.is_empty())
        .collect()
}

/// A `--save-configstrings` table, by slot.
fn retail_txt(map: &str, gametype: &str) -> BTreeMap<usize, String> {
    table(&format!("{map}-{gametype}-uo"))
}

/// `tests/fixtures/configstrings/<name>.txt`, by slot.
fn table(name: &str) -> BTreeMap<usize, String> {
    let path = format!("tests/fixtures/configstrings/{name}.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let (i, v) = l.split_once(' ').unwrap_or((l, ""));
            (i.parse().unwrap(), v.to_string())
        })
        .collect()
}

/// Ours after the load at `clock_ms` and `frames` frames: retail's table
/// was read at a client's gamestate, seconds in.
fn ours(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    gametype: &str,
    frames: u32,
    clock_ms: i32,
) -> BTreeMap<usize, String> {
    let sv = server_with(fs, map, gametype, frames, &[], clock_ms);
    if let Ok(p) = std::env::var("CS_UO_DUMP") {
        let t: String = (0..2048)
            .filter(|i| !sv.configstring(*i).is_empty())
            .map(|i| format!("{i} {}\n", sv.configstring(i)))
            .collect();
        std::fs::write(format!("{p}-{map}-{gametype}.txt"), t).unwrap();
    }
    table_of(&sv)
}

/// Every non-empty slot of `sv`'s table.
fn table_of(sv: &vcod_server::server::Server) -> BTreeMap<usize, String> {
    (0..2048)
        .filter(|i| !sv.configstring(*i).is_empty())
        .map(|i| (i, sv.configstring(i).to_string()))
        .collect()
}

fn server_with(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    gametype: &str,
    frames: u32,
    sets: &[(&str, &str)],
    clock_ms: i32,
) -> vcod_server::server::Server {
    server_at(fs, map, gametype, frames, sets, clock_ms, None, false)
}

/// [`server_with`] with the worldspawn carrying `message` when given and,
/// with `replay`, the sets run again as console lines after the load, as
/// retail's command line runs a `+set` placed after `+map`.
#[allow(clippy::too_many_arguments)]
fn server_at(
    mut fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    gametype: &str,
    frames: u32,
    sets: &[(&str, &str)],
    clock_ms: i32,
    message: Option<&str>,
    replay: bool,
) -> vcod_server::server::Server {
    if let Some(m) = message {
        let path = fs.resolve_map(map).expect("map in the mounted paks");
        let bsp = fs.read(&path).unwrap();
        let bsp = vcod_common::testing::bsp_with_worldspawn_key(&bsp, "message", m);
        fs.overlay(&path, bsp);
    }
    let fs = std::rc::Rc::new(fs);
    let start = std::time::Instant::now();
    let cfg = vcod_server::ServerConfig {
        map: map.into(),
        hostname: "CoDHost".into(),
        max_clients: 8,
        gametype: gametype.into(),
        test_entities: 0,
        trace: false,
        bots: 0,
        bots_shoot: false,
    };
    let mut sv = vcod_server::server::Server::new(cfg, start);
    sv.set_protocol(&PROTOCOL_V22);
    sv.start_clock_at(clock_ms);
    for (k, v) in sets {
        sv.set_cvar(k, v);
    }
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    if replay {
        for (k, v) in sets {
            sv.push_set(k, v);
        }
        sv.drain_console(start);
    }
    for i in 1..=frames {
        sv.tick(start + std::time::Duration::from_millis(50 * u64::from(i)));
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new(), "{map} {gametype}");
    sv
}

/// The keys two info strings disagree on.
fn info_diff(a: &str, b: &str) -> String {
    let pairs = |s: &str| -> BTreeMap<String, String> {
        let parts: Vec<&str> = s.split('\\').skip(1).collect();
        parts
            .chunks(2)
            .map(|kv| (kv[0].to_string(), kv.get(1).unwrap_or(&"").to_string()))
            .collect()
    };
    let (a, b) = (pairs(a), pairs(b));
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .map(|k| format!("{k}: retail {:?} ours {:?}", a.get(k), b.get(k)))
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn the_configstring_table_matches_uo_on_mp_cassino() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    // That server had been up half an hour: UO's level clock starts at the
    // uptime, and its start time (14, and 3's `t`) reads 1810050
    // (coduo-diff.md 20). Its `sv_serverid` 17 is one spawn and one
    // `map_restart` (a fresh server reads 16, cod11-map-cycle.md 3 step 16
    // and 4 step 5), so ours restarts once too; the restart starts its
    // level after the spawn's three 100 ms settle frames.
    let mut sv = server_with(fs, "mp_cassino", "dm", 0, &[], 1_810_050 - 300);
    sv.push_console("map_restart");
    sv.drain_console(std::time::Instant::now());
    check(retail(), table_of(&sv), "mp_cassino dm");
}

/// UO's own gametypes, each on the first stock map whose arena lists it.
/// `bas` runs on mp_foy, tanks and all.
#[test]
fn the_configstring_tables_match_uo_s_gametypes() {
    for (map, gametype) in [
        ("mp_cassino", "ctf"),
        ("mp_cassino", "dom"),
        ("mp_cassino", "hq"),
        ("mp_foy", "bas"),
    ] {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        check(
            retail_txt(map, gametype),
            ours(fs, map, gametype, 20, 0),
            &format!("{map} {gametype}"),
        );
    }
}

/// The `+set`s and worldspawn key `mp_cassino-dm-motd-uo.txt` was captured
/// under (its header has the recipe; configstrings_ab.rs has the 1.1 pair).
const MOTD_SETS: &[(&str, &str)] = &[
    ("g_motd", "Welcome to vcod"),
    ("scr_motd", "scr motd text"),
    ("sv_hostname", "vcod motd host"),
    ("scr_allow_vote", "0"),
    ("sv_privateClients", "2"),
];
const MOTD_MESSAGE: &str = "vcod worldspawn message";

/// Every cvar-backed slot a `+set` reaches, non-empty, and 4 from a
/// worldspawn `message`: 0's hostname and private slots, 15 `g_motd`
/// (UO's motd slot, cod11-map-cycle.md 4.5) and the mirror's `ui_motd`,
/// which carries `scr_motd` (225/353). 1, which lists the capture's extra
/// pak, stays out.
#[test]
fn the_configstring_table_matches_uo_with_a_motd_and_a_message() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let mut retail = retail_txt("mp_cassino", "dm-motd");
    for (i, v) in [(4, MOTD_MESSAGE), (15, "Welcome to vcod")] {
        assert_eq!(
            retail.get(&i).map(String::as_str),
            Some(v),
            "fixture cs[{i}]"
        );
    }
    let sv = server_at(
        fs,
        "mp_cassino",
        "dm",
        1,
        MOTD_SETS,
        0,
        Some(MOTD_MESSAGE),
        true,
    );
    let mut ours: BTreeMap<usize, String> = (0..2048)
        .filter(|i| !sv.configstring(*i).is_empty())
        .map(|i| (i, sv.configstring(i).to_string()))
        .collect();
    retail.remove(&1);
    ours.remove(&1);
    check(retail, ours, "mp_cassino dm motd");
}

/// A pure server's systeminfo, all four pak lists in it and 1318 and 1351
/// bytes long: UO keeps `BIG_INFO_STRING` where 1.1 cuts at 1023, and
/// references only the paks the level read a file from, plus any outside
/// `main` and `uo` (cod11-server-handshake.md, "The four cvars"). Each was
/// the recipe's second map load, so its `sv_serverid` reads 32 and ours
/// loads the recipe's first map before it.
#[test]
fn a_pure_server_s_systeminfo_matches_uo() {
    for (first, map) in [("mp_foy", "mp_cassino"), ("mp_cassino", "mp_j5outpost")] {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        if fs.resolve_map("mp_j5outpost").is_none() {
            eprintln!("no nsf_mp_j5outpost.pk3 in this install: skipped");
            return;
        }
        let mut sv = server_with(fs, first, "dm", 0, &[("sv_pure", "1")], 0);
        sv.push_console(&format!("map {map}"));
        // The load runs in the tick, the cvar refresh with it.
        let t = std::time::Instant::now();
        for i in 0..3 {
            sv.tick(t + std::time::Duration::from_millis(50 * i));
        }
        assert_eq!(sv.script_aborts(), Vec::<String>::new(), "{map}");
        let retail = table(&format!("{map}-dm-uo-pure"));
        for i in [0, 1] {
            let (a, b) = (&retail[&i], sv.configstring(i));
            assert_eq!(a, b, "{map} cs[{i}]: {}", info_diff(a, b));
        }
    }
}

/// A pak in `uo/` is listed once the level reads a file from it, and three
/// reads only the load makes: the map's sound load spec, each
/// `#using_animtree`'s tree (bas's `_flak_gmi.gsc` names `flak88`) and the
/// player's `multiplayer` tree. Retail UO with `fs_debug 1` reads all three
/// on mp_foy bas
/// (cod11-server-handshake.md, "The four cvars").
#[test]
fn a_pak_holding_the_load_spec_or_an_animtree_is_referenced() {
    let Some(stock) = vcod_common::testing::uo_fs() else {
        return;
    };
    let dir = vcod_common::testing::game_dir();
    let tmp = tempfile::tempdir().unwrap();
    let scratch = tmp.path().join("uo");
    std::fs::create_dir_all(&scratch).unwrap();
    let files = [
        ("zzz_spec", "soundloadspecs/mp/mp_foy.csv"),
        ("zzz_tree", "animtrees/flak88.atr"),
        ("zzz_mptree", "animtrees/multiplayer.atr"),
    ];
    for (pak, file) in files {
        let bytes = stock.read(file).expect(file);
        let pk3 = vcod_common::testing::pk3_with(&[(file, &bytes)]);
        std::fs::write(scratch.join(format!("{pak}.pk3")), pk3).unwrap();
    }
    let dirs = vec![dir.join("main"), dir.join("uo"), scratch.clone()];
    let fs = vcod_common::pk3::Pk3Fs::open_dirs(dirs, None, false).unwrap();
    let sv = server_with(fs, "mp_foy", "bas", 0, &[], 0);
    let names = vcod_common::net::connectionless::info_value_for_key(
        sv.configstring(1),
        "sv_referencedPakNames",
    )
    .unwrap_or_default();
    for (pak, _) in files {
        assert!(
            names.split(' ').any(|n| n == format!("uo/{pak}")),
            "uo/{pak} not in sv_referencedPakNames: {names}"
        );
    }
}

fn check(retail: BTreeMap<usize, String>, ours: BTreeMap<usize, String>, what: &str) {
    let mut slots: Vec<usize> = retail.keys().chain(ours.keys()).copied().collect();
    slots.sort_unstable();
    slots.dedup();
    let diffs: Vec<String> = slots
        .into_iter()
        .filter(|i| retail.get(i) != ours.get(i))
        .map(|i| match (retail.get(&i), ours.get(&i)) {
            (Some(a), Some(b)) if a.starts_with('\\') => format!("cs[{i}] {}", info_diff(a, b)),
            (a, b) => format!("cs[{i}] retail {a:?} ours {b:?}"),
        })
        .collect();
    assert!(
        diffs.is_empty(),
        "{what}: {} configstring(s) differ from UO 1.51b\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// The two tanks dm keeps on mp_cassino, as the retail gamestate's
/// baselines have them: the entity numbers the spawn order gives, the pool
/// slots, the collmaps' brush models, the settled height and every other
/// field (docs/research/coduo-vehicles.md 10).
#[test]
fn the_vehicle_baselines_match_uo_on_mp_cassino() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    use vcod_common::net::{gamestate, huffman::Huffman, msg::MsgReader};
    let data = vcod_common::testing::fixture("net/gamestate-uo.bin");
    let huff = Huffman::new();
    let mut r = MsgReader::new(&data[4..], &huff);
    let p = &PROTOCOL_V22;
    let gs = gamestate::parse(&mut r, p).expect("the UO gamestate");
    let mut sv = server_with(fs, "mp_cassino", "dm", 0, &[], 0);
    let mut ours = sv.all_entities();
    for e in ours.values_mut() {
        vcod_server::game::wire::entity_to_protocol(p, e);
    }
    let mut tanks: Vec<u32> = gs
        .baselines
        .iter()
        .filter(|(_, e)| e.field_i32(p, "eType") == vcod_common::net::flags::ET_VEHICLE_V22)
        .map(|(n, _)| *n)
        .collect();
    tanks.sort_unstable();
    assert_eq!(tanks, [176, 181]);
    for n in tanks {
        let (r, o) = (&gs.baselines[&n], ours.get(&n).expect("our tank"));
        for (f, (a, b)) in p.entity_fields.iter().zip(r.fields.iter().zip(&o.fields)) {
            // The vehicle table sends angles as 16 bits, which read back
            // within -180..180: ours holds the BSP's 270 where the wire
            // gives -90.
            if f.name.starts_with("apos.trBase") {
                let d = (f32::from_bits(*a as u32) - f32::from_bits(*b as u32)).rem_euclid(360.0);
                assert!(d.min(360.0 - d) < 1e-3, "entity {n} {}", f.name);
                continue;
            }
            // An int field travels in the vehicle table's width (`time2`
            // 6 bits).
            let bits = p
                .vehicle_fields
                .iter()
                .find(|v| v.name == f.name)
                .map_or(f.bits, |v| v.bits);
            let mask = match bits {
                1..=31 => ((1i64 << bits) - 1) as i32,
                _ => !0,
            };
            assert_eq!(a & mask, b & mask, "entity {n} {}", f.name);
        }
    }
}
