//! The whole playerstate, diffed against a retail capture field by field,
//! after a client has joined a team and spawned. The captures are from
//! `tools/run_server.sh` with the join `--save-playerstate` performs; each
//! fixture header names the map, gametype, team and weapon it asked for, and
//! the join here answers the stock menus with the same two.
//!
//! Only `[playerstate]` is diffed. Retail sends a client no entity for
//! itself -- the playerstate carries it -- so both fixtures' `[entity]`
//! section is a comment saying so, and there is nothing to compare there;
//! the check below fails if a retaken fixture ever grows one. The
//! client-tagged entity fields are asserted against script state elsewhere,
//! and what a second client sees of the first is stage 5's gate.
//! `[servercommands]` is task 7's evidence, not this gate's.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{connect, step, ClientEnd, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{PlayerState, NULL_USERCMD};
use vcod_common::net::protocol::{Protocol, PROTOCOL_V1};
use vcod_common::net::{NetClient, NetEvent};

/// Fields stage 4 knowingly does not reproduce, each with the reason. Empty
/// is the goal. A gap that starts matching fails the guard below, so this
/// list cannot rot into a lie.
const GAPS: &[(&str, &str)] = &[];

/// Fields no capture can pin: they advance with the frame the capture was
/// taken on, so retail's value and ours are both correct and different.
/// Unlike `GAPS` these carry no guard -- one matching by luck is not a
/// defect, and asserting that it must differ would be wrong.
const VOLATILE: &[(&str, &str)] = &[(
    "commandTime",
    "the serverTime of the last usercmd run, so it counts up with the frame",
)];

/// Fields `check_spawn_shape` covers instead of the literal diff, because
/// `_spawnlogic::getSpawnpoint_DM` picks weighted-random from the top third
/// of its candidates: every capture lands somewhere else.
const SPAWN_SHAPE: &[&str] = &[
    "origin[0]",
    "origin[1]",
    "origin[2]",
    "delta_angles[0]",
    "delta_angles[1]",
    "delta_angles[2]",
    "viewangles[0]",
    "viewangles[1]",
    "viewangles[2]",
];

/// The fixture header's "3 s after the weapon menu was answered".
const SPAWN_SETTLE: Duration = Duration::from_secs(3);

/// `ANGLE2SHORT` (q_shared.h), as the 16-bit wire word `delta_angles` is.
fn angle2short(deg: f32) -> i32 {
    (deg * 65536.0 / 360.0) as i32 & 0xffff
}

fn cfg(map: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        // The capture was taken at 8, and the fixture header is checked
        // against this below.
        max_clients: 8,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
    }
}

// ---------------------------------------------------------------- the fixture

/// One retail capture: the join it was taken with, and the playerstate it
/// ended in.
struct Capture {
    team: String,
    weapon: String,
    /// `Protocol::player_fields` order, raw wire words.
    ps: Vec<i32>,
}

/// Every `# key value` clause in the header block with this key. The caller
/// insists on exactly one: the header is prose as well as data, so a second
/// clause opening with the same word would otherwise decide the join
/// silently, by being first.
fn header_values<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    text.lines()
        .take_while(|l| l.starts_with('#'))
        .flat_map(|l| l.trim_start_matches('#').split(','))
        .map(str::trim)
        .filter_map(|clause| clause.strip_prefix(key)?.strip_prefix(' '))
        .map(|v| v.trim_end_matches('.'))
        .collect()
}

/// The committed retail capture. The fixture name carries the gametype, so
/// it comes from `cfg` rather than from a literal: a changed `cfg().gametype`
/// has to move the fixture too, not silently keep reading the `dm` capture.
fn retail(map: &str) -> Capture {
    let path = format!("tests/fixtures/playerstate/{map}-{}.txt", cfg(map).gametype);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let head = |k: &str| {
        let found = header_values(&text, k);
        assert_eq!(
            found.len(),
            1,
            "{path}: the header has {} clauses opening with {k:?}, expected 1: {found:?}",
            found.len()
        );
        found[0]
    };
    assert_eq!(head("map"), map, "{path}: header names another map");
    assert_eq!(
        head("g_gametype"),
        cfg(map).gametype,
        "{path}: header names another gametype"
    );
    assert_eq!(
        head("sv_maxclients"),
        cfg(map).max_clients.to_string(),
        "{path}: header names another sv_maxclients, which moves script state"
    );

    let mut section = "";
    let mut ps: Vec<(&str, i32)> = Vec::new();
    let mut entity_lines = 0;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = line;
            continue;
        }
        match section {
            "[playerstate]" => {
                let (name, v) = line
                    .split_once(' ')
                    .unwrap_or_else(|| panic!("{path}: bad playerstate line: {line}"));
                let v = v
                    .parse()
                    .unwrap_or_else(|e| panic!("{path}: {name} is not an i32: {e}"));
                ps.push((name, v));
            }
            "[entity]" => entity_lines += 1,
            _ => {}
        }
    }

    // The gate diffs no entity because retail sends a client none for itself.
    // A fixture that grows one has invalidated that, so say so rather than
    // ignoring it.
    assert_eq!(
        entity_lines, 0,
        "{path}: the capture now carries a self-entity; this gate compares none"
    );

    let p = &PROTOCOL_V1;
    let names: Vec<&str> = ps.iter().map(|(n, _)| *n).collect();
    let expected: Vec<&str> = p.player_fields.iter().map(|f| f.name).collect();
    assert_eq!(
        names, expected,
        "{path}: the capture is not in Protocol::player_fields order"
    );

    let capture = Capture {
        team: head("joined").to_string(),
        weapon: head("weapon").to_string(),
        ps: ps.into_iter().map(|(_, v)| v).collect(),
    };

    // The two clauses that steer the join are the two nothing else pins, so
    // they are cross-checked against the capture itself: `_teams::restrict`
    // spawns nobody for any other team, and `weapon` is a 1-based index into
    // configstring 7, resolved through retail's own list because it is
    // retail's index.
    assert!(
        capture.team == "allies" || capture.team == "axis",
        "{path}: header joined {:?}, which never spawns a player",
        capture.team
    );
    let cs7 = retail_weapon_list(map);
    let want = capture.ps[field_index(p, "weapon")];
    assert!(
        want >= 1,
        "{path}: the capture carries weapon {want}, so it holds none"
    );
    let named = cs7
        .split_whitespace()
        .nth(want as usize - 1)
        .unwrap_or_else(|| panic!("{path}: weapon {want} is past retail's configstring 7"));
    assert_eq!(
        named, capture.weapon,
        "{path}: header says weapon {:?} but the capture spawned with {named:?}",
        capture.weapon
    );

    capture
}

/// Configstring 7 out of the retail capture the stage 3 gate diffs, which is
/// the list `playerState.weapon` indexes.
fn retail_weapon_list(map: &str) -> String {
    let path = format!(
        "tests/fixtures/configstrings/{map}-{}.txt",
        cfg(map).gametype
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    text.lines()
        .find_map(|l| l.strip_prefix("7 "))
        .unwrap_or_else(|| panic!("{path}: no configstring 7"))
        .to_string()
}

/// Answers the stock team and weapon menus with what the capture was taken
/// with. `v g_scriptMainMenu <menu>` names the menu, `t <index>` opens it and
/// `mr <serverId> <index> <response>` answers it
/// (docs/research/cod11-hud-protocol.md, section 0.1).
struct Join<'a> {
    capture: &'a Capture,
    main_menu: String,
    answered: Vec<i32>,
    answered_team: bool,
    answered_weapon_at: Option<Instant>,
    log: Vec<String>,
}

impl<'a> Join<'a> {
    fn new(capture: &'a Capture) -> Self {
        Join {
            capture,
            main_menu: String::new(),
            answered: Vec::new(),
            answered_team: false,
            answered_weapon_at: None,
            log: Vec::new(),
        }
    }

    fn on_server_command(
        &mut self,
        tokens: &[String],
        cl: &mut NetClient<ClientEnd>,
        now: Instant,
    ) {
        match tokens.first().map(String::as_str) {
            Some("v") if tokens.get(1).map(String::as_str) == Some("g_scriptMainMenu") => {
                self.main_menu = tokens.get(2).cloned().unwrap_or_default();
            }
            Some("t") => {
                let Some(idx) = tokens.get(1).and_then(|t| t.parse::<i32>().ok()) else {
                    return;
                };
                if self.answered.contains(&idx) {
                    return;
                }
                let menu = self.main_menu.clone();
                let reply = if menu.starts_with("team_") {
                    self.capture.team.clone()
                } else if menu.starts_with("weapon_") {
                    self.capture.weapon.clone()
                } else {
                    self.log.push(format!("menu {idx} ({menu:?}) has no reply"));
                    return;
                };
                cl.send_reliable(&format!("mr {} {idx} {reply}", cl.server_id()));
                self.answered.push(idx);
                self.log
                    .push(format!("answered menu {idx} ({menu}) with {reply}"));
                if menu.starts_with("weapon_") {
                    self.answered_weapon_at = Some(now);
                } else {
                    self.answered_team = true;
                }
            }
            _ => {}
        }
    }

    fn settled(&self, now: Instant) -> bool {
        self.answered_weapon_at
            .is_some_and(|t| now.duration_since(t) >= SPAWN_SETTLE)
    }

    /// What the join did, for the failure message.
    fn summary(&self) -> String {
        let log = match self.log.is_empty() {
            true => "the server opened no script menu at all".to_string(),
            false => self.log.join("; "),
        };
        format!("join: begin sent, {log}")
    }

    /// The path, not the state it ended in: a player that reached the
    /// playerstate without being asked which team and which weapon did not
    /// join through the stock menus, whatever its 103 fields say. Nothing
    /// else in the gate constrains how the spawn happened.
    fn findings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.answered_team {
            out.push("no team menu opened, so the client never answered one".to_string());
        }
        if self.answered_weapon_at.is_none() {
            out.push("no weapon menu opened, so the client never spawned".to_string());
        }
        out
    }
}

/// Our playerstate at the same moment retail's was read: `begin`, both menu
/// answers, then `SPAWN_SETTLE` of simulated time.
fn ours<'a>(
    map: &str,
    capture: &'a Capture,
    fs: vcod_common::pk3::Pk3Fs,
    bsp: &vcod_common::bsp::Bsp,
) -> (PlayerState, Join<'a>) {
    let fs = std::rc::Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(map), now);
    sv.load_world(vcod_server::world::World::from_bsp(bsp));
    sv.load_scripts(fs).expect("load the scripts");

    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = connect(&mut sv, &q, &mut now);
    // `begin` is what releases `Callback_PlayerConnect`'s `waittill`; the join
    // menus follow from it.
    cl.send_reliable("begin");

    let mut join = Join::new(capture);
    // 20 s of simulated time at sv_fps 20, well past the settle a completed
    // join needs; a join that never happens burns the lot.
    for _ in 0..400 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        for e in step(&mut sv, &q, &mut cl, now) {
            match e {
                NetEvent::ServerCommand(tokens) => join.on_server_command(&tokens, &mut cl, now),
                NetEvent::Dropped(r) => panic!("dropped mid-join: {r}"),
                _ => {}
            }
        }
        if join.settled(now) {
            break;
        }
    }

    let ps = cl
        .snapshots()
        .newest()
        .map(|s| s.ps.clone())
        .expect("the server sent no snapshot at all");
    (ps, join)
}

// ------------------------------------------------------------------- the diff

/// The spawn-dependent fields, checked for shape rather than for equality.
/// `origin` must be one of the map's DM spawn points, and `delta_angles` the
/// yaw the server hands a player spawning there: the server sets
/// `ANGLE2SHORT(spawn yaw) - cmd.angles`, and our client sends
/// `-delta_angles` of the frame it last saw, which while spectating is
/// `ANGLE2SHORT` of the intermission it was parked at. Those two sum to the
/// word on the wire, and the same subtraction is why a settled `viewangles`
/// reads zero on every axis.
fn check_spawn_shape(entities: &str, ps: &PlayerState, who: &str, out: &mut Vec<String>) {
    let p = &PROTOCOL_V1;
    let blocks = vcod_common::bsp::entity_blocks(entities);
    // A missing or unreadable key is a fact about the map, not about the
    // playerstate, and a default would quietly make every expectation below
    // wrong. Both classes carry `origin` and an `angles` triple on the stock
    // maps; anything else stops the run.
    let of_class = |class: &str| -> Vec<([f32; 3], f32)> {
        blocks
            .iter()
            .filter(|b| b.get("classname").map(String::as_str) == Some(class))
            .map(|b| {
                let origin = b
                    .get("origin")
                    .and_then(|o| vcod_common::bsp::parse_vec3(o))
                    .unwrap_or_else(|| panic!("{class} with no readable origin: {b:?}"));
                let yaw = b
                    .get("angles")
                    .and_then(|a| a.split_whitespace().nth(1))
                    .and_then(|t| t.parse::<f32>().ok())
                    .unwrap_or_else(|| panic!("{class} with no readable angles yaw: {b:?}"));
                (origin, yaw)
            })
            .collect()
    };

    // Spawn-independent, so they are checked whether or not the origin lands
    // on a spawn point: nothing else covers them, `SPAWN_SHAPE` having taken
    // them out of the literal diff.
    for f in ["delta_angles[0]", "delta_angles[2]"] {
        let v = ps.field_i32(p, f);
        if v != 0 {
            out.push(format!(
                "{f} {who} {v}, expected 0: only the yaw is set at spawn"
            ));
        }
    }
    for (i, v) in ps.viewangles(p).iter().enumerate() {
        if *v != 0.0 {
            out.push(format!(
                "viewangles[{i}] {who} {v}, expected 0: the client subtracts delta_angles"
            ));
        }
    }

    let origin = ps.origin(p);
    let spawns = of_class("mp_deathmatch_spawn");
    let Some((spawn_origin, spawn_yaw)) = spawns
        .iter()
        .find(|(o, _)| o[0] == origin[0] && o[1] == origin[1])
        .copied()
    else {
        out.push(format!(
            "origin {who} {origin:?} is at none of the {} mp_deathmatch_spawn points",
            spawns.len()
        ));
        return;
    };

    // The player settles onto the floor under the spawn point during the
    // three seconds before the capture and never rises above it. The band is
    // picked, not measured: the two captures drop 8.875 and 7.875 units, and
    // 64 is room for a spawn point set further above its floor.
    const MAX_DROP: f32 = 64.0;
    const MAX_RISE: f32 = 1.0;
    if origin[2] > spawn_origin[2] + MAX_RISE || origin[2] < spawn_origin[2] - MAX_DROP {
        out.push(format!(
            "origin[2] {who} {} is not within {MAX_DROP} below the spawn point's {}",
            origin[2], spawn_origin[2]
        ));
    }

    // Both maps park a spectator at a `mp_deathmatch_intermission`; pavlov
    // has two, with different yaws, so any of them may be the one.
    let wanted: Vec<i32> = of_class("mp_deathmatch_intermission")
        .iter()
        .map(|(_, iyaw)| (angle2short(spawn_yaw) + angle2short(*iyaw)) & 0xffff)
        .collect();
    let got = ps.field_i32(p, "delta_angles[1]");
    if !wanted.contains(&got) {
        out.push(format!(
            "delta_angles[1] {who} {got} is not the yaw of the spawn point at {spawn_origin:?} \
             ({spawn_yaw} deg) from any intermission; expected one of {wanted:?}"
        ));
    }
}

fn field_index(p: &Protocol, name: &str) -> usize {
    PlayerState::field_index(p, name).unwrap_or_else(|| panic!("no playerstate field {name:?}"))
}

fn check(map: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");

    let capture = retail(map);
    let (ours, join) = ours(map, &capture, fs, &bsp);

    let p = &PROTOCOL_V1;
    let retail_ps = PlayerState {
        fields: capture.ps.clone(),
        arrays: Vec::new(),
    };
    let path = join.findings();
    let mut shape = Vec::new();
    check_spawn_shape(&bsp.entities, &retail_ps, "retail", &mut shape);
    check_spawn_shape(&bsp.entities, &ours, "ours", &mut shape);

    // A misspelt name in any of the three lists would silently cover nothing.
    for name in SPAWN_SHAPE
        .iter()
        .chain(VOLATILE.iter().map(|(n, _)| n))
        .chain(GAPS.iter().map(|(n, _)| n))
    {
        field_index(p, name);
    }
    let skip = |name: &str| {
        SPAWN_SHAPE.contains(&name)
            || VOLATILE.iter().any(|(v, _)| *v == name)
            || GAPS.iter().any(|(g, _)| *g == name)
    };
    let mut diffs = Vec::new();
    // `legsAnim` carries `ANIM_TOGGLEBIT` (512) above the index, so retail's
    // 634 is index 122 with the bit set; a mismatch here can be the parity
    // rather than the wrong animation
    // (docs/research/player-model-anim-system.md, "Animation indices").
    for (i, f) in p.player_fields.iter().enumerate() {
        if skip(f.name) {
            continue;
        }
        let (r, o) = (capture.ps[i], ours.fields[i]);
        if r != o {
            diffs.push(format!("{} retail {r} ours {o}", f.name));
        }
    }

    // The field count is the number every later task moves, so it counts
    // fields only; the join-path and spawn-shape findings are listed
    // separately, because neither is a field.
    assert!(
        diffs.is_empty() && shape.is_empty() && path.is_empty(),
        "{map}: {} playerstate field(s) differ from retail, {} join-path and {} spawn-shape \
         finding(s)\n{}\n{}",
        diffs.len(),
        path.len(),
        shape.len(),
        join.summary(),
        path.iter()
            .map(|s| format!("[join] {s}"))
            .chain(shape.iter().map(|s| format!("[shape] {s}")))
            .chain(diffs.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    );

    // A gap that starts matching is a gap that should have been deleted.
    for (name, why) in GAPS {
        let i = field_index(p, name);
        assert_ne!(
            capture.ps[i], ours.fields[i],
            "{name} now matches retail; drop it from GAPS ({why})"
        );
    }
}

#[test]
fn the_playerstate_matches_retail_on_mp_pavlov() {
    check("mp_pavlov");
}

/// The other nationality branch: `mp_pavlov` joins allies as russians with a
/// `mosin_nagant_mp`, `mp_carentan` as americans with an `m1carbine_mp`, so
/// the pair catches a spawn that hardcodes either weapon set.
#[test]
fn the_playerstate_matches_retail_on_mp_carentan() {
    check("mp_carentan");
}
