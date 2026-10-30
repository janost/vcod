//! `fs_game mods/ModWarfare`, the install's stock example mod, against
//! retail: `handshake-1.7-mod.txt` is `tools/cod4_handshake_probe.py` on a
//! retail `cod4_lnxded` 1.7 started with `+set fs_game mods/ModWarfare`
//! (recipe in cod4-fastfile.md 13). The level boots its gametype scripts
//! out of the mod's iwd over `common_mp`'s, reads the weapon files off
//! the iwds, and configstrings 0 and 1 name the mod as retail's do, the
//! referenced iwds included. Passes vacuously without `COD4_DIR` or without
//! the mod in it.

use super::*;
use vcod_cod4::net::capture;
use vcod_common::net::connectionless::info_pairs;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cod4/tests/fixtures/net/handshake-1.7-mod.txt"
);

fn pairs(s: &str) -> Vec<(String, String)> {
    info_pairs(s)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn modwarfare_boots_its_own_scripts_and_names_the_mod_as_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    if install
        .clone()
        .with_fs_game(Some("mods/ModWarfare"))
        .mod_zone()
        .is_none()
    {
        return;
    }
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    sv.set_cvar("fs_game", "mods/ModWarfare");
    sv.load(Some(&install)).unwrap();
    let level = sv.game.level.as_mut().unwrap();
    let aborts = level.rt.aborts();
    assert!(
        aborts.is_empty(),
        "bootstrap errors:\n{}",
        aborts.join("\n")
    );
    // `_modwarfare::init`, which only the mod's `_globallogic` threads,
    // set its class limits.
    assert_eq!(level.rt.host.cvars.get("class_assault_limit"), "64");

    let lines = capture::parse(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let mut s = capture::Session::new();
    for l in lines.iter().filter(|l| l.dir != "auth") {
        let _ = s.feed(l);
    }
    let gs = s.client.gamestate.expect("a gamestate");
    let cs = |i: u16| String::from_utf8_lossy(gs.configstring(i)).into_owned();
    let retail0 = pairs(&cs(0));
    let ours0 = pairs(&sv.serverinfo());
    assert_eq!(
        ours0.first(),
        retail0.first(),
        "configstring 0 leads with fs_game"
    );
    let retail1 = pairs(&cs(1));
    let ours1 = pairs(&sv.systeminfo());
    let keys = |p: &[(String, String)]| p.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&ours1), keys(&retail1), "configstring 1's keys");
    for ((k, ours), (_, theirs)) in ours1.iter().zip(&retail1) {
        if k == "sv_serverid" {
            continue;
        }
        assert_eq!(ours, theirs, "configstring 1's {k}");
    }
    // The weapon files came off the iwds, `main/iw_11`'s, as retail's.
    assert!(
        sv.game.zones4.as_ref().unwrap().weapon_files.len() > 100,
        "the weapon files"
    );
}
