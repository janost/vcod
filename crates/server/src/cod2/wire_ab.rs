//! The whole wire against retail: `crates/server/tests/fixtures/cod2-wire/`
//! holds two clients of one retail `cod2_lnxded` 1.3 session per capture
//! (`sv_pure 1`, `developer 1`, `sv_maxclients 8`, inside `unshare -rn` on
//! loopback): `<map>[-<tag>]-spec.bin`, `examples/spectate.rs --secs 12
//! --save .. --save-messages 120` started 12 s after the server, and
//! `-join.bin`, the same with `--join --name joiner` 3 s later, which
//! answers the team menu `autoassign` and the weapon menu with the rifle
//! ([`GAMETYPES`] lists the gametype and kill runs). The same two clients
//! run against `Cod2Server` in process on retail's clock, the joiner's
//! spawn spots and death anims pinned to retail's, and the gamestate
//! (configstrings, baselines), the server commands, the spectator's
//! entities and playerstate and the joined player's playerstate are
//! diffed field by field. What still differs is [`GAPS`], each with the
//! cod2-protocol.md section that explains it. Needs `COD2_DIR`.

use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use vcod_cod2::net::capture::{Capture, replay};
use vcod_cod2::net::delta::{EntityState2, PlayerState2};
use vcod_cod2::net::fields::{ENTITY_STATE_FIELDS, NetField, PLAYER_STATE_FIELDS};
use vcod_cod2::net::oob::{self, Reply};
use vcod_cod2::net::{Client, Gamestate2, Snapshot2, latin1};
use vcod_common::net::info_value_for_key;

use super::*;

/// A capture's shape: each client's `--secs`, the sequenced messages each
/// saved (`--save-messages`), and the joiner's `--kill-at`, in ms after it
/// started.
#[derive(Clone, Copy)]
struct Run {
    secs: i32,
    msgs: usize,
    kill_ms: Option<i32>,
}

/// c10-wirenum's captures: 120 messages, no kill.
const PLAIN: Run = Run {
    secs: 12,
    msgs: 120,
    kill_ms: None,
};

/// What one client read.
struct Side {
    gs: Gamestate2,
    commands: Vec<String>,
    snaps: Vec<Snapshot2>,
}

fn retail(bytes: &[u8]) -> Side {
    let r = replay(&Capture::decode(bytes).unwrap()).unwrap();
    Side {
        gs: r.parser.gamestate.clone().expect("a gamestate"),
        commands: r.commands,
        snaps: r.snapshots,
    }
}

/// One in-process client: its address, its wire state and what it read.
struct Probe {
    addr: SocketAddr,
    cl: Client,
    join: bool,
    asked_team: bool,
    /// Sequenced messages read; past [`Run::msgs`] nothing is kept, as
    /// `--save-messages` cut retail's capture.
    seen: usize,
    /// The level time its `--secs` ran out and it sent `disconnect`.
    end: i32,
    gone: bool,
    commands: Vec<String>,
    snaps: Vec<Snapshot2>,
}

/// `examples/spectate.rs --join`'s weapon menu answer: the nation's rifle.
fn weapon_reply(cl: &Client, text: &str) -> Option<String> {
    let n: u16 = text.strip_prefix("t ")?.trim().parse().ok()?;
    let menu = cl.parser.gamestate.as_ref()?.configstring(1246 + n);
    let weapon = match menu.strip_prefix("weapon_")? {
        "german" => "kar98k_mp",
        "british" => "enfield_mp",
        "american" => "m1garand_mp",
        "russian" => "mosin_nagant_mp",
        _ => return None,
    };
    Some(format!("mr {} {n} {weapon}", cl.server_id()))
}

/// Each spawn of the retail joiner as a spot and its yaw: a snapshot whose
/// `pm_type` turns 0 (normal) from anything else. The script's spawn pick
/// is a `randomInt` off a clock-seeded stream, so the gate replays retail's
/// picks as CoD1's `Server::pin_spawns` does.
fn spawn_pins(join: &Side) -> Vec<crate::game::host::SpawnPin> {
    let mut last = -1;
    let mut pins = Vec::new();
    for s in &join.snaps {
        let t = s.ps.i32("pm_type");
        if t == 0 && last != 0 {
            pins.push(Some((s.ps.origin(), [0.0, s.ps.viewangles()[1], 0.0])));
        }
        last = t;
    }
    pins
}

/// How long before its first snapshot's `serverTime` a retail client's
/// process started, which its `--secs` and `--kill-at` count from: the
/// kill runs' `--kill-at 5` lands 4350 after the joiner's first snapshot,
/// a frame before the first dead one. Each client here reads its first
/// snapshot when its retail twin did and leaves at that start
/// plus its `--secs`, so the level's clock (`d 13`, a round's end) and
/// the joins and drops run as retail's did.
const START_LEAD_MS: i32 = 650;

/// How long a client here takes from its `getchallenge` to its first
/// snapshot: the gamestate, the `cp`, the first cmd.
const CONNECT_MS: i32 = 350;

/// The `serverTime` of the retail joiner's first snapshot dead.
fn death_time(join: &Side) -> Option<i32> {
    join.snaps
        .windows(2)
        .find(|w| w[0].ps.i32("pm_type") == 0 && w[1].ps.i32("pm_type") >= 5)
        .map(|w| w[1].server_time)
}

/// Each death of the retail joiner, its death anim without the toggle:
/// `player_die` draws it with the game's `rand()`, seeded off the clock.
fn death_pins(join: &Side) -> Vec<i32> {
    let mut last = -1;
    let mut pins = Vec::new();
    for s in &join.snaps {
        let t = s.ps.i32("pm_type");
        if t >= 5 && last == 0 {
            pins.push(s.ps.i32("legsAnim") & !vcod_cod2::pmove::anim::ANIM_TOGGLEBIT);
        }
        last = t;
    }
    pins
}

/// Each weapon the retail joiner's deaths dropped, in the order the
/// spectator first saw them in flight, as the `crandom()` values its drop
/// drew off the clock-seeded `rand()`: the climb (`trDelta[2]` is `10 + 5
/// c`), then the spin's roll, yaw and pitch (`apos.trDelta` is `(50 c3,
/// 40 c2, 60 c1)`), cod2-combat.md 6.3.
fn drop_pins(spec: &Side) -> Vec<f64> {
    let mut seen = std::collections::HashSet::new();
    let mut pins = Vec::new();
    for s in &spec.snaps {
        for e in &s.entities {
            if e.i32("eType") != 3 || e.i32("apos.trType") != 2 || !seen.insert(e.number) {
                continue;
            }
            let c = |name: &str, k: f64| f64::from(e.f32(name)) / k;
            pins.push((f64::from(e.f32("pos.trDelta[2]")) - 10.0) / 5.0);
            pins.push(c("apos.trDelta[2]", 60.0));
            pins.push(c("apos.trDelta[1]", 40.0));
            pins.push(c("apos.trDelta[0]", 50.0));
        }
    }
    pins
}

/// The two clients against `Cod2Server` on `map`, each reading its first
/// snapshot when its twin in `retail` (spectator, joiner) did, the
/// joiner's spawn spots, death anims and kill replayed off its twin's.
/// `seed` is the script's RNG, picked so `menuAutoAssign`'s `randomInt(2)`
/// draws retail's team.
fn ours(
    fs: &Rc<Pk3Fs>,
    map: &str,
    gametype: &str,
    seed: u64,
    run: Run,
    retail: [&Side; 2],
) -> (Side, Side) {
    let pins = spawn_pins(retail[1]);
    let deaths = death_pins(retail[1]);
    let starts = retail.map(|s| s.snaps[0].server_time);
    let kill_at = death_time(retail[1]);
    let mut now = Instant::now();
    let cfg = Cod2Config {
        map: map.into(),
        gametype: gametype.into(),
        hostname: "CoD2Host".into(),
        max_clients: 8,
        gametype_script: None,
    };
    let mut sv = Cod2Server::new(cfg, 7, now);
    sv.set_cvar("developer", "1");
    sv.load(Some(fs.clone())).unwrap();
    let host = &mut sv.game.level.as_mut().unwrap().rt.host;
    host.rng = seed | 1;
    host.spawn_pins = Some(Rc::new(std::cell::RefCell::new(vec![
        Default::default(),
        pins.into(),
    ])));
    host.cod2_mut().death_pins = vec![Default::default(), deaths.into()];
    host.cod2_mut().drop_pins = drop_pins(retail[0]).into();
    let mut probes: Vec<Probe> = Vec::new();
    let mut kill_at = kill_at.filter(|_| run.kill_ms.is_some());
    // `--secs` runs from the client's start, [`START_LEAD_MS`] before its
    // first snapshot.
    let end_at = |first: i32| first - START_LEAD_MS + run.secs * 1000;
    // Until both clients have run out, or retail's message cap.
    let last = (starts[1] + 2_000 + 50 * run.msgs as i32).max(end_at(starts[1]) + 500);
    while sv.time_ms() < last {
        // The kill's cmd runs on the next frame, which retail's first dead
        // snapshot carries.
        if kill_at.is_some_and(|t| sv.time_ms() + FRAME_MS >= t) {
            kill_at = None;
            probes[1].cl.add_reliable("kill");
        }
        if probes.len() < 2 && sv.time_ms() + CONNECT_MS >= starts[probes.len()] {
            let i = probes.len();
            let addr = SocketAddr::from(([127, 0, 0, 1], 28961 + i as u16));
            sv.handle_packet(addr, &oob::getchallenge(None), now);
            let out = sv.take_outgoing();
            let Reply::Challenge(c) = oob::parse_reply(oob::payload(&out[0].1).unwrap()) else {
                panic!("no challenge");
            };
            let name = if i == 0 { "vcodspec" } else { "joiner" };
            let ui = format!("\\name\\{name}\\rate\\25000\\snaps\\20\\cl_punkbuster\\0");
            sv.handle_packet(addr, &oob::connect(&ui, c, 4660 + i as u16), now);
            sv.take_outgoing();
            probes.push(Probe {
                addr,
                cl: Client::new(4660 + i as u16, c),
                join: i == 1,
                asked_team: false,
                seen: 0,
                end: end_at(starts[i]),
                gone: false,
                commands: Vec::new(),
                snaps: Vec::new(),
            });
        }
        for p in probes.iter_mut().filter(|p| !p.gone) {
            if sv.time_ms() >= p.end {
                p.cl.add_reliable("disconnect");
                for _ in 0..3 {
                    let pkt = p.cl.build_packet(&[]).unwrap();
                    sv.handle_packet(p.addr, &pkt, now);
                }
                p.gone = true;
                continue;
            }
            // A primed client gets no snapshot until it moves
            // (cod2-protocol.md 17.2): cmds go out once the gamestate is in.
            let base = match p.cl.parser.snapshot() {
                Some(s) => Some(s.server_time),
                None => p.cl.parser.gamestate.is_some().then(|| sv.time_ms()),
            };
            let pkt = match base {
                Some(t) => {
                    let cmd = p.cl.idle_cmd(t + FRAME_MS);
                    p.cl.build_packet(&[cmd]).unwrap()
                }
                None => p.cl.build_packet(&[]).unwrap(),
            };
            sv.handle_packet(p.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        // The console lines the script queued (`map_restart`), ahead of the
        // frame as `main.rs`' loop runs them.
        sv.drain_console(now);
        sv.tick_frames(now, 1);
        for (to, pkt) in sv.take_outgoing() {
            let Some(p) = probes.iter_mut().find(|p| p.addr == to) else {
                continue;
            };
            let Ok(Some(parsed)) = p.cl.receive(&pkt) else {
                continue;
            };
            p.seen += 1;
            let keep = p.seen <= run.msgs;
            if parsed.gamestate {
                let gs = p.cl.parser.gamestate.as_ref().unwrap();
                let si = gs.configstring(1).to_string();
                fs.clear_references();
                fs.touch(&format!("maps/mp/{map}.d3dbsp"));
                let cp = vcod_cod2::net::pure::pure_command(fs, gs.checksum_feed, &si);
                p.cl.add_reliable(&cp);
            }
            for (_, text) in &parsed.commands {
                let text = latin1(text);
                if p.join
                    && let Some(reply) = weapon_reply(&p.cl, &text)
                {
                    p.cl.add_reliable(&reply);
                }
                if keep {
                    p.commands.push(text);
                }
            }
            if let Some(n) = parsed.snapshot {
                if keep {
                    p.snaps.push(p.cl.parser.frame(n).unwrap().clone());
                }
                if p.join && !p.asked_team {
                    p.asked_team = true;
                    let mr = format!("mr {} 1 autoassign", p.cl.server_id());
                    p.cl.add_reliable(&mr);
                }
            }
        }
    }
    let mut sides = probes.into_iter().map(|p| Side {
        gs: p.cl.parser.gamestate.clone().unwrap(),
        commands: p.commands,
        snaps: p.snaps,
    });
    (sides.next().unwrap(), sides.next().unwrap())
}

/// `name` of each differing field, prefixed `tag`; `skip` names fields
/// that move with time or place.
fn field_diffs(
    tag: &str,
    table: &[NetField],
    a: &[u32],
    b: &[u32],
    skip: &dyn Fn(&str) -> bool,
    out: &mut Vec<String>,
) {
    for ((f, x), y) in table.iter().zip(a).zip(b) {
        if x != y && !skip(f.name) {
            out.push(format!("{tag} {}", f.name));
        }
    }
}

fn entity_diffs(
    tag: &str,
    a: &[EntityState2],
    b: &[EntityState2],
    skip: &dyn Fn(u32, &str) -> bool,
    out: &mut Vec<String>,
) {
    let numbers: std::collections::BTreeSet<u32> = a.iter().chain(b).map(|e| e.number).collect();
    for n in numbers {
        match (
            a.iter().find(|e| e.number == n),
            b.iter().find(|e| e.number == n),
        ) {
            (Some(x), Some(y)) => {
                field_diffs(
                    &format!("{tag} {n}"),
                    &ENTITY_STATE_FIELDS,
                    &x.f,
                    &y.f,
                    &|f| skip(n, f),
                    out,
                );
            }
            (Some(_), None) => out.push(format!("{tag} {n} retail only")),
            (None, Some(_)) => out.push(format!("{tag} {n} ours only")),
            (None, None) => {}
        }
    }
}

/// Playerstate fields a spawn spot, a clock or a pmove step moves.
fn placed(f: &str) -> bool {
    [
        "commandTime",
        "origin",
        "velocity",
        "viewangles",
        "delta_angles",
        "bobCycle",
        "movementDir",
        "groundEntityNum",
        "eventSequence",
        "events",
        "eventParms",
        "oldEventSequence",
        "jumpTime",
        "jumpOriginZ",
        "legsTimer",
        "torsoTimer",
        "legsAnim",
        "torsoAnim",
        "aimSpreadScale",
        "weapAnim",
        "weaponTime",
        "weaponstate",
        "pm_time",
        "pm_flags",
    ]
    .iter()
    .any(|p| f == *p || f.starts_with(&format!("{p}[")))
}

fn ps_diffs(tag: &str, a: &PlayerState2, b: &PlayerState2, out: &mut Vec<String>) {
    field_diffs(tag, &PLAYER_STATE_FIELDS, &a.f, &b.f, &placed, out);
    for (name, x, y) in [
        ("stats", &a.stats[..], &b.stats[..]),
        ("ammo", &a.ammo[..], &b.ammo[..]),
        ("ammoclip", &a.ammoclip[..], &b.ammoclip[..]),
    ] {
        for (i, (p, q)) in x.iter().zip(y).enumerate() {
            if p != q {
                out.push(format!("{tag} {name}[{i}]"));
            }
        }
    }
}

/// Every difference between retail's two clients and ours.
fn diffs(r_spec: &Side, r_join: &Side, o_spec: &Side, o_join: &Side) -> Vec<String> {
    let mut out = Vec::new();
    // The gamestate: configstrings, 0 and 1 key by key past the serverid.
    let (ra, oa) = (&r_spec.gs, &o_spec.gs);
    for i in 0..vcod_cod2::net::ops::MAX_CONFIGSTRINGS as u16 {
        let (x, y) = (ra.configstring(i), oa.configstring(i));
        if i < 2 {
            let keys = |s: &str| -> Vec<String> {
                s.split('\\')
                    .skip(1)
                    .step_by(2)
                    .map(str::to_string)
                    .collect()
            };
            let mut all = keys(x);
            all.extend(keys(y));
            all.sort();
            all.dedup();
            for k in all {
                if k != "sv_serverid" && info_value_for_key(x, &k) != info_value_for_key(y, &k) {
                    out.push(format!("cs {i} {k}"));
                }
            }
        } else if x != y {
            out.push(format!("cs {i}"));
        }
    }
    let bases = |gs: &Gamestate2| -> Vec<EntityState2> { gs.baselines.values().copied().collect() };
    entity_diffs("baseline", &bases(ra), &bases(oa), &|_, _| false, &mut out);

    // The server commands each client read, up to retail's last.
    for (tag, r, o) in [("spec", r_spec, o_spec), ("join", r_join, o_join)] {
        for (i, c) in r.commands.iter().enumerate() {
            if o.commands.get(i) != Some(c) {
                out.push(format!("{tag} cmd {i}"));
                break;
            }
        }
    }

    // The spectator's last snapshot: every entity (a live body's clock and
    // anim aside; its spawn is pinned to retail's) and its playerstate.
    let (rs, os) = (r_spec.snaps.last().unwrap(), o_spec.snaps.last().unwrap());
    let moving = |n: u32, f: &str| n < 64 && (placed(f) || f == "pos.trTime");
    entity_diffs("spec ent", &rs.entities, &os.entities, &moving, &mut out);
    ps_diffs("spec ps", &rs.ps, &os.ps, &mut out);
    // The joined player's last playerstate.
    let (rj, oj) = (r_join.snaps.last().unwrap(), o_join.snaps.last().unwrap());
    ps_diffs("join ps", &rj.ps, &oj.ps, &mut out);
    out
}

/// What still differs, on which capture (`<map>` or `<map> <gametype>`,
/// "" for every one), and why (cod2-protocol.md section).
const GAPS: &[(&str, &str, &str)] = &[
    // The two weapons its death dropped start at the posed body's tag
    // (`G_DObjGetWorldTagMatrix`), whose pose is not retail's to the bit:
    // 0.01 off on tdm's turned body, two ulps of x on sd's (15.7). Their
    // flight, spin and rest match once the start is retail's.
    ("mp_toujane tdm-kill", "spec ent 73 pos.trBase[1]", "15.7"),
    ("mp_toujane tdm-kill", "spec ent 73 pos.trBase[0]", "15.7"),
    ("mp_toujane tdm-kill", "spec ent 73 pos.trBase[2]", "15.7"),
    ("mp_toujane tdm-kill", "spec ent 74 pos.trBase[1]", "15.7"),
    ("mp_toujane tdm-kill", "spec ent 74 pos.trBase[0]", "15.7"),
    ("mp_toujane tdm-kill", "spec ent 74 pos.trBase[2]", "15.7"),
    ("mp_toujane sd-kill", "spec ent 73 pos.trBase[0]", "15.7"),
    ("mp_toujane sd-kill", "spec ent 74 pos.trBase[0]", "15.7"),
];

/// `WIRE_REPORT=1`: each difference's two values, and both clients'
/// server commands where they part.
fn report(r_spec: &Side, r_join: &Side, o_spec: &Side, o_join: &Side, found: &[String]) {
    for d in found {
        let w: Vec<&str> = d.split(' ').collect();
        match w.as_slice() {
            ["cs", n, rest @ ..] => {
                let n: u16 = n.parse().unwrap();
                let (x, y) = (r_spec.gs.configstring(n), o_spec.gs.configstring(n));
                if rest.is_empty() {
                    eprintln!("{d}: retail {x:?}\n    ours {y:?}");
                } else {
                    eprintln!(
                        "{d}: retail {:?} ours {:?}",
                        info_value_for_key(x, rest[0]),
                        info_value_for_key(y, rest[0])
                    );
                }
            }
            [tag, "cmd", i] => {
                let i: usize = i.parse().unwrap();
                let (r, o) = if *tag == "spec" {
                    (r_spec, o_spec)
                } else {
                    (r_join, o_join)
                };
                let lo = i.saturating_sub(14);
                eprintln!(
                    "{d}: retail {:#?}\n    ours {:#?}",
                    &r.commands[lo..(i + 60).min(r.commands.len())],
                    &o.commands[lo..(i + 60).min(o.commands.len())]
                );
            }
            ["baseline", n, ..] | ["spec", "ent", n, ..] => {
                let n: u32 = n.parse().unwrap();
                let show = |e: Option<&EntityState2>| {
                    e.map(|e| {
                        ENTITY_STATE_FIELDS
                            .iter()
                            .zip(&e.f)
                            .filter(|(_, w)| **w != 0)
                            .map(|(f, w)| format!("{}={w:x}", f.name))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                };
                let (r, o) = if w[0] == "baseline" {
                    (r_spec.gs.baselines.get(&n), o_spec.gs.baselines.get(&n))
                } else {
                    let (a, b) = (r_spec.snaps.last().unwrap(), o_spec.snaps.last().unwrap());
                    (
                        a.entities.iter().find(|e| e.number == n),
                        b.entities.iter().find(|e| e.number == n),
                    )
                };
                eprintln!("{d}:\n  retail {:?}\n  ours   {:?}", show(r), show(o));
            }
            [tag @ ("join" | "spec"), "ps", f] => {
                let (r, o) = if *tag == "spec" {
                    (r_spec, o_spec)
                } else {
                    (r_join, o_join)
                };
                let (r, o) = (&r.snaps.last().unwrap().ps, &o.snaps.last().unwrap().ps);
                match PLAYER_STATE_FIELDS.iter().position(|x| x.name == *f) {
                    Some(i) => eprintln!("{d}: retail {:#x} ours {:#x}", r.f[i], o.f[i]),
                    None => eprintln!("{d}"),
                }
            }
            _ => eprintln!("{d}"),
        }
    }
}

/// The two sides of `map` differ in exactly its [`GAPS`].
fn check(map: &str, gametype: &str, tag: &str, spec: &[u8], join: &[u8], seed: u64, run: Run) {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let (r_spec, r_join) = (retail(spec), retail(join));
    let (o_spec, o_join) = ours(&fs, map, gametype, seed, run, [&r_spec, &r_join]);
    let found = diffs(&r_spec, &r_join, &o_spec, &o_join);
    let want: Vec<String> = GAPS
        .iter()
        .filter(|(m, ..)| m.is_empty() || *m == tag)
        .map(|(_, d, _)| d.to_string())
        .collect();
    if std::env::var_os("WIRE_REPORT").is_some() {
        report(&r_spec, &r_join, &o_spec, &o_join, &found);
    }
    assert_eq!(found, want, "{tag}: the wire against retail's");
}

#[test]
fn mp_toujane_wire_matches_retail() {
    check(
        "mp_toujane",
        "dm",
        "mp_toujane",
        include_bytes!("../../tests/fixtures/cod2-wire/mp_toujane-spec.bin"),
        include_bytes!("../../tests/fixtures/cod2-wire/mp_toujane-join.bin"),
        2,
        PLAIN,
    );
}

#[test]
fn mp_carentan_wire_matches_retail() {
    check(
        "mp_carentan",
        "dm",
        "mp_carentan",
        include_bytes!("../../tests/fixtures/cod2-wire/mp_carentan-spec.bin"),
        include_bytes!("../../tests/fixtures/cod2-wire/mp_carentan-join.bin"),
        1,
        PLAIN,
    );
}

/// The gametype captures, `<map>-<tag>-{spec,join}.bin`: each with the seed
/// whose `randomInt(2)` draws retail's team, its gametype and its run.
const KILL: Run = Run {
    secs: 15,
    msgs: 240,
    kill_ms: Some(5000),
};
const GAMETYPES: &[(&str, &str, &str, u64, Run)] = &[
    ("mp_toujane", "tdm", "tdm", 1, PLAIN),
    ("mp_toujane", "sd", "sd", 2, PLAIN),
    ("mp_toujane", "ctf", "ctf", 2, PLAIN),
    ("mp_toujane", "hq", "hq", 1, PLAIN),
    ("mp_toujane", "tdm-kill", "tdm", 1, KILL),
    ("mp_toujane", "sd-kill", "sd", 2, KILL),
    ("mp_carentan", "tdm", "tdm", 2, PLAIN),
    ("mp_carentan", "sd", "sd", 1, PLAIN),
    ("mp_carentan", "ctf", "ctf", 1, PLAIN),
    ("mp_carentan", "hq", "hq", 1, PLAIN),
    ("mp_burgundy", "dm", "dm", 2, PLAIN),
    ("mp_burgundy", "sd", "sd", 2, PLAIN),
];

#[test]
fn gametypes_wire_matches_retail() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cod2-wire");
    for &(map, tag, gametype, seed, run) in GAMETYPES {
        let read = |side: &str| std::fs::read(format!("{dir}/{map}-{tag}-{side}.bin")).unwrap();
        check(
            map,
            gametype,
            &format!("{map} {tag}"),
            &read("spec"),
            &read("join"),
            seed,
            run,
        );
    }
}

/// A capture outside the fixtures: `WIRE_GT_DIR=<dir> WIRE_GT="<tag>.."`
/// reads `<dir>/<WIRE_MAP>-<tag>-{spec,join}.bin` (`WIRE_SEED`,
/// `WIRE_KILL` for a `--kill-at 5` run), and prints whether each matches.
#[test]
fn wire_capture_from_env() {
    let dir = std::env::var("WIRE_GT_DIR").unwrap_or_default();
    if dir.is_empty() {
        return;
    }
    let map = std::env::var("WIRE_MAP").unwrap_or("mp_toujane".into());
    let seed = std::env::var("WIRE_SEED").map_or(2, |s| s.parse().unwrap());
    let run = if std::env::var_os("WIRE_KILL").is_some() {
        KILL
    } else {
        PLAIN
    };
    for gt in std::env::var("WIRE_GT").unwrap_or("tdm".into()).split(' ') {
        let spec = std::fs::read(format!("{dir}/{map}-{gt}-spec.bin")).unwrap();
        let join = std::fs::read(format!("{dir}/{map}-{gt}-join.bin")).unwrap();
        let tag = format!("{map} {gt}");
        let gametype = gt.split('-').next().unwrap();
        let r = std::panic::catch_unwind(|| check(&map, gametype, &tag, &spec, &join, seed, run));
        eprintln!("== {gt}: {}", if r.is_ok() { "ok" } else { "differs" });
    }
}
