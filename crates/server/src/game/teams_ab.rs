//! `G_FindTeams` and the team pick against retail: mp_carentan with
//! `tests/fixtures/teams/mp_carentan-teams.ent` appended to its entity lump,
//! on 1.1d, 1.5b and UO (`tests/fixtures/teams/mp_carentan-dm-teams*.txt`,
//! recipes in their headers). The banner's team line must read retail's,
//! and `client-probes/probe_teams.gsc`'s `useBy`s, on retail's draws, must
//! respawn what retail's picks did, in order, up to the master's `G_Error`
//! (cod11-items.md 14.8).
//!
//! Needs `COD_DIR` at the capture's install; without it the test returns
//! early.

use crate::server::{Server, ServerConfig};
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::event_ids::EV_ITEM_RESPAWN;
use vcod_common::net::protocol::PROTOCOL_V6;
use vcod_common::pk3::Pk3Fs;

const MAP: &str = "mp_carentan";
const BLOCKS: &str = include_str!("../../tests/fixtures/teams/mp_carentan-teams.ent");
const PROBE: &str =
    include_str!("../../../gsc/tests/fixtures/semantics/client-probes/probe_teams.gsc");
const FRAME_MS: u64 = 50;

fn cfg(gametype: &str) -> ServerConfig {
    ServerConfig {
        map: MAP.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: gametype.into(),
        test_entities: 0,
        trace: false,
        bots: 0,
        bots_shoot: false,
    }
}

fn fs_15() -> Option<Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    (crate::server::install_protocol(&fs).transport.version == PROTOCOL_V6.transport.version)
        .then_some(fs)
}

/// The fixture's lines that start with `tag `, the tag stripped.
fn lines<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    text.lines()
        .filter_map(|l| l.strip_prefix(tag)?.strip_prefix(' '))
        .collect()
}

/// A server on `fs` with the teams lump in place of mp_carentan's.
fn server(mut fs: Pk3Fs, gametype: &str) -> (Server, Instant) {
    let p = crate::server::install_protocol(&fs);
    let path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bytes = vcod_common::testing::bsp_with_entities(&fs.read(&path).unwrap(), BLOCKS);
    fs.overlay(&path, bytes.clone());
    let bsp = vcod_common::bsp::parse(&bytes).unwrap();
    let now = Instant::now();
    let mut sv = Server::new(cfg(gametype), now);
    sv.set_protocol(p);
    sv.set_cvar("developer", "1");
    sv.set_cvar("rconpassword", "pw");
    if gametype != "dm" {
        sv.overlay_script(&format!("maps/mp/gametypes/{gametype}"), PROBE);
    }
    sv.load_world(crate::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    (sv, now)
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
    banner(fs_15, "tests/fixtures/teams/mp_carentan-dm-teams-1.5.txt");
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
    let retail_picks: Vec<u32> = lines(&text, "GDB pick")
        .iter()
        .map(|d| d.parse().unwrap())
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
    let ours: Vec<String> = sv
        .script_log()
        .iter()
        .filter_map(|l| l.find("PROBE ").map(|i| l[i + 6..].trim_end().to_string()))
        .collect();
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
    picks(fs_15, "tests/fixtures/teams/mp_carentan-dm-teams-1.5.txt");
    picks(
        vcod_common::testing::uo_fs,
        "tests/fixtures/teams/mp_carentan-dm-teams-uo.txt",
    );
}
