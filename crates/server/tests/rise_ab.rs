//! The stance refusals, the ladder's stance strip and a dead body's fire
//! bit, ours against retail 1.1d's, 1.5b's and UO 1.51b's (one fixture
//! pair per server). `client-probes/probe_rise` drives one
//! `--probe-uo-weapon direct` client through `PROBE_IN` lines: on
//! mp_carentan it lies him down, moves him prone under a ceiling a crouch
//! does not fit and then one a stand does not, tries each rise with and
//! without `wbuttons` 0x2, and kills him twice with the trigger held, once
//! through dm's callback (which drops his weapon) and once through one that
//! keeps it; on mp_ship it hangs him on the hold ladder holding each stance
//! key (docs/research/cod11-mantle.md, "The forced-stance events"). The
//! fixtures are the client's `--save-slope --slope-every-state` capture at
//! one cmd a frame (`--probe-cmd-ms 50`) with the server's `PROBE` lines.
//!
//! Ours runs the same gsc with one in-process client that sends one cmd a
//! frame, stamped with the newest snapshot's time, as the net probe did. Each
//! phase is one row, held exactly: the state the phase settles in (its
//! last snapshot's `pm_type`, `pm_flags`, `eFlags` and `viewHeightTarget`),
//! every event id its own cmds raised, and for a death the fire bit (eFlags
//! 0x400) of the death's snapshot and the two after it.
//!
//! `RISE_REPORT=1` prints both sides' rows. Needs `COD_DIR`; without the
//! paks it returns early.

use crate::common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{self, PlayerState, UserCmd};
use vcod_common::net::protocol::Protocol;
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_rise.gsc");
const CARENTAN: &str = include_str!("fixtures/playerstate/mp_carentan-dm-slope-50ms-rise-1.1.txt");
const SHIP: &str = include_str!("fixtures/playerstate/mp_ship-dm-slope-50ms-rise-1.1.txt");
const CARENTAN_15: &str =
    include_str!("fixtures/playerstate/mp_carentan-dm-slope-50ms-rise-1.5.txt");
const SHIP_15: &str = include_str!("fixtures/playerstate/mp_ship-dm-slope-50ms-rise-1.5.txt");
const CARENTAN_UO: &str =
    include_str!("fixtures/playerstate/mp_carentan-dm-slope-50ms-rise-uo.txt");
const SHIP_UO: &str = include_str!("fixtures/playerstate/mp_ship-dm-slope-50ms-rise-uo.txt");

/// Phases whose death snapshots the row reads.
const DEATHS: &[&str] = &["dead", "dead_keep"];

/// One phase as both sides are held to it.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    pm_type: i32,
    pm_flags: i32,
    eflags: i32,
    view_target: i32,
    events: BTreeSet<i32>,
    /// A death's fire bit on its own snapshot and the two after it.
    fire: Option<[Option<bool>; 3]>,
}

/// `PROBE phase <time> <name>` and `PROBE done <time>`, in order.
fn phases(lines: &[String]) -> Vec<(i32, String)> {
    let mut out = Vec::new();
    for l in lines {
        let t: Vec<&str> = l.split_whitespace().collect();
        match t.as_slice() {
            ["PROBE", "phase", time, name] => out.push((time.parse().unwrap(), name.to_string())),
            ["PROBE", "done", time] => out.push((time.parse().unwrap(), "done".into())),
            _ => {}
        }
    }
    out
}

/// The capture's `!snap` lines as `(serverTime, playerstate)`.
fn retail_snaps(text: &str, p: &Protocol) -> Vec<(i32, PlayerState)> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| l.starts_with("!snap ")) {
        let kv: BTreeMap<&str, &str> = line
            .split_whitespace()
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let mut ps = PlayerState::null(p);
        for pair in kv["wire"].split(',').filter(|s| !s.is_empty()) {
            let (name, v) = pair.rsplit_once(':').expect("name:value");
            let i = PlayerState::field_index(p, name).expect(name);
            ps.fields[i] = v.parse().expect("an int");
        }
        out.push((kv["t"].parse().unwrap(), ps));
    }
    out
}

/// Each phase's row off one side's snapshots. A phase's own cmds reach the
/// snapshots past its first two (the `PROBE_IN` rides the first, the cmd
/// that reads it runs before the next frame), up to the next phase's first.
fn rows(
    snaps: &[(i32, PlayerState)],
    phases: &[(i32, String)],
    p: &Protocol,
) -> BTreeMap<String, Row> {
    let at = |t: i32| snaps.iter().find(|s| s.0 == t).map(|s| &s.1);
    let mut out = BTreeMap::new();
    for w in phases.windows(2) {
        let ((start, name), (end, _)) = (&w[0], &w[1]);
        let Some(settled) = snaps.iter().rev().find(|s| s.0 <= *end).map(|s| &s.1) else {
            continue;
        };
        if snaps.first().is_none_or(|s| s.0 > *start) {
            continue;
        }
        let mut events = BTreeSet::new();
        for pair in snaps.windows(2) {
            let (t, ps) = (pair[1].0, &pair[1].1);
            if t < start + 100 || t > *end {
                continue;
            }
            let (prev, seq) = (
                pair[0].1.field_i32(p, "eventSequence"),
                ps.field_i32(p, "eventSequence"),
            );
            let gained = ((seq - prev) & 0xff).min(4);
            for k in (seq - gained)..seq {
                events.insert(ps.field_i32(p, &format!("events[{}]", k & 3)));
            }
        }
        let fire = DEATHS.contains(&name.as_str()).then(|| {
            [0, 50, 100].map(|d| at(start + d).map(|ps| ps.field_i32(p, "eFlags") & 0x400 != 0))
        });
        out.insert(
            name.clone(),
            Row {
                pm_type: settled.field_i32(p, "pm_type"),
                pm_flags: settled.field_i32(p, "pm_flags"),
                eflags: settled.field_i32(p, "eFlags"),
                view_target: settled.field_i32(p, "viewHeightTarget"),
                events,
                fire,
            },
        );
    }
    out
}

/// The buttons, `wbuttons` and `up` of a `f "PROBE_IN ..."` command.
fn direct_input(tokens: &[String]) -> Option<(u8, u8, i8)> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let mut t = tokens.get(1)?.strip_prefix("PROBE_IN ")?.split_whitespace();
    Some((
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    ))
}

/// Ours under the probe: its snapshots and its script log.
fn ours(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    weapon: &str,
) -> (Vec<(i32, PlayerState)>, Vec<String>) {
    let p = vcod_server::server::install_protocol(&fs);
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_rise"), now);
    sv.set_protocol(p);
    sv.overlay_script("maps/mp/gametypes/probe_rise", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", weapon);
    let mut input = (0u8, 0u8, 0i8);
    let mut spawned = false;
    let mut last_cmd = i32::MIN;
    let mut snaps: Vec<(i32, PlayerState)> = Vec::new();
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for _ in 0..2000 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        let newest = cl
            .snapshots()
            .newest()
            .map(|s| (s.server_time, s.ps.clone()));
        spawned |= newest
            .as_ref()
            .is_some_and(|(_, ps)| ps.field_i32(p, "pm_type") == 0);
        match newest {
            Some((t, ps)) if spawned && t > last_cmd => {
                // The view as the snapshot has it, rebased on `delta_angles`.
                let view = ps.viewangles(p);
                let angles = std::array::from_fn(|i| {
                    let da = ps.field_i32(p, &format!("delta_angles[{i}]"));
                    ((view[i] * 65536.0 / 360.0).round() as i32 - da) & 0xffff
                });
                cl.send_cmds(&[UserCmd {
                    server_time: t,
                    angles,
                    buttons: input.0,
                    wbuttons: input.1,
                    up: input.2,
                    weapon: ps.field_i32(p, "weapon") as u8,
                    ..msg::NULL_USERCMD
                }]);
                last_cmd = t;
            }
            _ if !spawned => {
                cl.send_frame(&UserCmd::default());
            }
            _ => {}
        }
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for pk in pending {
            sv.handle_packet(common::ADDR, &pk, now);
        }
        sv.tick(now);
        for (_, pk) in sv.take_outgoing() {
            q.borrow_mut().to_client.push_back(pk);
        }
        for e in cl.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if let Some(i) = direct_input(&tokens) {
                    input = i;
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest()
            && snaps.last().is_none_or(|l| l.0 != s.server_time)
        {
            snaps.push((s.server_time, s.ps.clone()));
        }
    }
    (snaps, sv.script_log().to_vec())
}

fn check(fs: Option<vcod_common::pk3::Pk3Fs>, fixture: &str, map: &str, weapon: &str) {
    let Some(fs) = fs else {
        return;
    };
    let p = vcod_server::server::install_protocol(&fs);
    let retail_log: Vec<String> = fixture
        .lines()
        .filter(|l| l.starts_with("PROBE "))
        .map(str::to_string)
        .collect();
    let theirs = rows(&retail_snaps(fixture, p), &phases(&retail_log), p);
    let (snaps, log) = ours(fs, map, weapon);
    let log: Vec<String> = log
        .iter()
        .filter_map(|l| l.find("PROBE ").map(|i| l[i..].trim_end().to_string()))
        .collect();
    let mine = rows(&snaps, &phases(&log), p);
    let report = std::env::var_os("RISE_REPORT").is_some();
    let mut bad = Vec::new();
    for (name, want) in &theirs {
        let got = mine.get(name);
        if report {
            println!("{name}\n  retail {want:?}\n  ours   {got:?}");
        }
        if got != Some(want) {
            bad.push(name.as_str());
        }
    }
    assert!(theirs.len() >= 5, "{map}: the capture holds its phases");
    assert!(
        bad.is_empty(),
        "{map}: phases off retail's: {bad:?} (RISE_REPORT=1)"
    );
}

/// The 1.5b install's paks, `None` elsewhere (a UO install's `main/` is
/// 1.5's, so this runs there too).
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    let v6 = vcod_common::net::protocol::PROTOCOL_V6.transport.version;
    (vcod_server::server::install_protocol(&fs).transport.version == v6).then_some(fs)
}

#[test]
fn a_rise_under_a_low_ceiling_is_refused_as_on_retail() {
    let fs = vcod_common::testing::game_fs_11();
    check(fs, CARENTAN, "mp_carentan", "m1carbine_mp");
}

#[test]
fn a_climber_s_stance_keys_are_stripped_as_on_retail() {
    let fs = vcod_common::testing::game_fs_11();
    check(fs, SHIP, "mp_ship", "enfield_mp");
}

#[test]
fn a_rise_under_a_low_ceiling_is_refused_as_on_15() {
    check(fs_15(), CARENTAN_15, "mp_carentan", "m1carbine_mp");
}

#[test]
fn a_climber_s_stance_keys_are_stripped_as_on_15() {
    check(fs_15(), SHIP_15, "mp_ship", "enfield_mp");
}

#[test]
fn a_rise_under_a_low_ceiling_is_refused_as_on_uo() {
    let fs = vcod_common::testing::uo_fs();
    check(fs, CARENTAN_UO, "mp_carentan", "m1carbine_mp");
}

#[test]
fn a_climber_s_stance_keys_are_stripped_as_on_uo() {
    let fs = vcod_common::testing::uo_fs();
    check(fs, SHIP_UO, "mp_ship", "enfield_mp");
}
