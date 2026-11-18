//! `G_FindTeams` and the team pick against retail: mp_carentan with
//! `tests/fixtures/teams/mp_carentan-teams.ent` appended to its entity lump,
//! on 1.1d, 1.5b and UO (`tests/fixtures/teams/mp_carentan-dm-teams*.txt`,
//! recipes in their headers). The banner's team line must read retail's,
//! and `client-probes/probe_teams.gsc`'s `useBy`s, on retail's draws, must
//! respawn what retail's picks did, in order, up to the master's `G_Error`
//! (cod11-items.md 14.8).
//!
//! `FinishSpawningItem` against retail: `client-probes/probe_startsolid.gsc`
//! logs a classname's items a frame at a time from the load on the teams
//! lump (all three), on 1.1's mp_railyard (suspended panzerfausts) and on
//! UO's mp_kursk (a panzerschreck freed) and mp_rhinevalley (satchels
//! dropped off the second trace) (`tests/fixtures/items/*-finish*.txt`,
//! cod11-items.md 14.9).
//!
//! Needs `COD_DIR` at the capture's install; without it the test returns
//! early.

use crate::server::{Server, ServerConfig};
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::event_ids::EV_ITEM_RESPAWN;
use vcod_common::pk3::Pk3Fs;

const MAP: &str = "mp_carentan";
const BLOCKS: &str = include_str!("../../tests/fixtures/teams/mp_carentan-teams.ent");
const PROBE: &str =
    include_str!("../../../gsc/tests/fixtures/semantics/client-probes/probe_teams.gsc");
const FINISH: &str =
    include_str!("../../../gsc/tests/fixtures/semantics/client-probes/probe_startsolid.gsc");
const FRAME_MS: u64 = 50;

fn cfg(map: &str, gametype: &str) -> ServerConfig {
    ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: gametype.into(),
        test_entities: 0,
        trace: false,
        bots: 0,
        bots_shoot: false,
    }
}

/// The fixture's lines that start with `tag `, the tag stripped.
fn lines<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    text.lines()
        .filter_map(|l| l.strip_prefix(tag)?.strip_prefix(' '))
        .collect()
}

/// A server on `fs` with the teams lump in place of mp_carentan's.
fn server(fs: Pk3Fs, gametype: &str) -> (Server, Instant) {
    let source = if gametype == "probe_teams" {
        Some(PROBE)
    } else {
        None
    };
    server_on(fs, MAP, Some(BLOCKS), gametype, source, &[])
}

/// A server on `fs` loading `map`, `blocks` appended to its entity lump,
/// with `source` as the `gametype` script when given.
fn server_on(
    mut fs: Pk3Fs,
    map: &str,
    blocks: Option<&str>,
    gametype: &str,
    source: Option<&str>,
    cvars: &[(&str, &str)],
) -> (Server, Instant) {
    let p = crate::server::install_protocol(&fs);
    let path = fs.resolve_map(map).expect("map in the mounted paks");
    let mut bytes = fs.read(&path).unwrap();
    if let Some(b) = blocks {
        bytes = vcod_common::testing::bsp_with_entities(&bytes, b);
        fs.overlay(&path, bytes.clone());
    }
    let bsp = vcod_common::bsp::parse(&bytes).unwrap();
    let now = Instant::now();
    let mut sv = Server::new(cfg(map, gametype), now);
    sv.set_protocol(p);
    sv.set_cvar("developer", "1");
    sv.set_cvar("rconpassword", "pw");
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    if let Some(text) = source {
        sv.overlay_script(&format!("maps/mp/gametypes/{gametype}"), text);
    }
    sv.load_world(crate::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    (sv, now)
}

/// Ours' `PROBE` lines, the tag stripped.
fn probe_log(sv: &Server) -> Vec<String> {
    sv.script_log()
        .iter()
        .filter_map(|l| l.find("PROBE ").map(|i| l[i + 6..].trim_end().to_string()))
        .collect()
}

/// `probe_startsolid` on `map` (the teams lump on mp_carentan) under
/// `probe_class` `class`: every `PROBE` line must be retail's.
fn finish(fs: fn() -> Option<Pk3Fs>, map: &str, class: &str, fixture: &str) {
    let Some(fs) = fs() else {
        eprintln!("COD_DIR unset or not the capture's install: skipping");
        return;
    };
    let text = std::fs::read_to_string(fixture).unwrap();
    let blocks = (map == MAP).then_some(BLOCKS);
    let (mut sv, mut now) = server_on(
        fs,
        map,
        blocks,
        "probe_startsolid",
        Some(FINISH),
        &[("probe_class", class)],
    );
    for _ in 0..(3000 / FRAME_MS) {
        now += Duration::from_millis(FRAME_MS);
        sv.tick(now);
        sv.take_outgoing();
    }
    let ours = probe_log(&sv);
    assert_eq!(ours, lines(&text, "PROBE"), "{fixture}");
}

#[test]
fn map_items_finish_spawning_like_retail() {
    use vcod_common::testing::{game_fs_11, uo_fs};
    const D: &str = "tests/fixtures/items";
    finish(
        game_fs_11,
        MAP,
        "item_health",
        &format!("{D}/mp_carentan-dm-finish.txt"),
    );
    finish(
        vcod_common::testing::game_fs_15,
        MAP,
        "item_health",
        &format!("{D}/mp_carentan-dm-finish-1.5.txt"),
    );
    finish(
        uo_fs,
        MAP,
        "item_health",
        &format!("{D}/mp_carentan-dm-finish-uo.txt"),
    );
    finish(
        game_fs_11,
        "mp_railyard",
        "mpweapon_panzerfaust",
        &format!("{D}/mp_railyard-dm-finish.txt"),
    );
    finish(
        uo_fs,
        "mp_kursk",
        "mpweapon_panzerschreck",
        &format!("{D}/mp_kursk-dm-finish-uo.txt"),
    );
    finish(
        uo_fs,
        "mp_rhinevalley",
        "mpweapon_satchelcharge",
        &format!("{D}/mp_rhinevalley-dm-finish-uo.txt"),
    );
}

/// A restart's banner over rcon carries `G_FindTeams`' line as retail's
/// console printed it.
fn banner(fs: fn() -> Option<Pk3Fs>, fixture: &str) {
    let Some(fs) = fs() else {
        eprintln!("COD_DIR unset or not the capture's install: skipping");
        return;
    };
    let text = std::fs::read_to_string(fixture).unwrap();
    let retail = lines(&text, "CONSOLE")[0];
    let (mut sv, now) = server(fs, "dm");
    let from = SocketAddr::from(([127, 0, 0, 1], 9));
    sv.handle_packet(
        from,
        b"\xff\xff\xff\xffrcon pw map_restart",
        now + Duration::from_millis(500),
    );
    let reply: String = sv
        .take_outgoing()
        .into_iter()
        .filter(|(to, _)| *to == from)
        .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
        .collect();
    let tail = format!("\n{retail}\n-----------------------------------\n");
    assert!(
        reply.starts_with("print\n==== RestartGame ====\n"),
        "{reply}"
    );
    assert!(reply.ends_with(&tail), "{fixture}: {reply}");
}

#[test]
fn the_banner_counts_the_teams_like_retail() {
    banner(
        vcod_common::testing::game_fs_11,
        "tests/fixtures/teams/mp_carentan-dm-teams.txt",
    );
    banner(
        vcod_common::testing::game_fs_15,
        "tests/fixtures/teams/mp_carentan-dm-teams-1.5.txt",
    );
    banner(
        vcod_common::testing::uo_fs,
        "tests/fixtures/teams/mp_carentan-dm-teams-uo.txt",
    );
}

/// The probe on ours with retail's draws pinned: its `PROBE` lines, and per
/// `useBy` the entity that took `EV_ITEM_RESPAWN`, which a frame holds one
/// of, must be retail's; the master's use drops the level.
fn picks(fs: fn() -> Option<Pk3Fs>, fixture: &str) {
    let Some(fs) = fs() else {
        eprintln!("COD_DIR unset or not the capture's install: skipping");
        return;
    };
    let text = std::fs::read_to_string(fixture).unwrap();
    let draws = lines(&text, "GDB draw")
        .iter()
        .map(|d| d.parse().unwrap())
        .collect();
    // A pick of a slot freed since the load reads `s.number` 0 and sends
    // nothing on ours (cod11-items.md 14.8).
    let retail_picks: Vec<u32> = lines(&text, "GDB pick")
        .iter()
        .map(|d| d.parse().unwrap())
        .filter(|&n| n != 0)
        .collect();
    let (mut sv, mut now) = server(fs, "probe_teams");
    sv.pin_item_draws(draws);
    let rings = |sv: &Server| -> Vec<(u32, i32, [i32; 4])> {
        let Some(host) = sv.game_host() else {
            return Vec::new();
        };
        host.ents
            .iter_inuse()
            .filter(|(id, _)| id.0 >= 64)
            .map(|(id, e)| (id.0, e.events.seq, e.events.events))
            .collect()
    };
    let mut ours_picks = Vec::new();
    let mut before = rings(&sv);
    for _ in 0..(8000 / FRAME_MS) {
        now += Duration::from_millis(FRAME_MS);
        sv.tick(now);
        sv.take_outgoing();
        let after = rings(&sv);
        for (n, seq, events) in &after {
            let old = before.iter().find(|(m, ..)| m == n).map_or(0, |b| b.1);
            if *seq != old && events[((seq - 1) & 3) as usize] == EV_ITEM_RESPAWN {
                ours_picks.push(*n);
            }
        }
        before = after;
        if sv.is_killed() {
            break;
        }
    }
    let retail: Vec<&str> = lines(&text, "PROBE");
    let ours = probe_log(&sv);
    assert_eq!(ours, retail, "{fixture}: PROBE lines");
    assert_eq!(ours_picks, retail_picks, "{fixture}: picks");
    assert!(
        sv.is_killed(),
        "{fixture}: the master's use drops the level"
    );
}

#[test]
fn use_by_on_a_teamed_item_respawns_retail_s_pick() {
    picks(
        vcod_common::testing::game_fs_11,
        "tests/fixtures/teams/mp_carentan-dm-teams.txt",
    );
    picks(
        vcod_common::testing::game_fs_15,
        "tests/fixtures/teams/mp_carentan-dm-teams-1.5.txt",
    );
    picks(
        vcod_common::testing::uo_fs,
        "tests/fixtures/teams/mp_carentan-dm-teams-uo.txt",
    );
}
