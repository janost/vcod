//! The killcam's rule against the retail tdm hit capture, which caught one:
//! the shooter killed the target and the target's next nine seconds are the
//! shooter's, replayed (docs/research/cod11-spectator-follow.md, section 12).
//! The rule read off it is the same on both sides: the replay starts 2000 ms
//! after the kill, its age is `delay + 7` seconds trimmed to the first frame
//! the killer was archived with a view of its own, it ends that age after it
//! started, and the end is a dead spawn where the replay left off. Retail's
//! numbers come out of the two committed fixtures; ours out of the same
//! schedule on our server.
//!
//! Needs `COD_DIR` for our half; the retail half reads only the fixtures.

mod common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{UserCmd, BUTTON_ADS, BUTTON_ATTACK, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
const TARGET: &str = "tests/fixtures/playerstate/mp_carentan-tdm-hit-target.txt";
const SHOOTER: &str = "tests/fixtures/playerstate/mp_carentan-tdm-hit-shooter.txt";
const PM_DEAD: i32 = 6;
const EF_TELEPORT_BIT: i32 = 0x8;
/// `Callback_PlayerKilled`'s `delay`, and the seven seconds `killcam` adds.
const DELAY_MS: i32 = 2000;
const ASKED_MS: i32 = DELAY_MS + 7000;
/// The kill in the retail capture came this long after the shooter's first
/// archived frame, which is what trimmed its killcam's age.
const RETAIL_SPAWN_TO_KILL_MS: i32 = 6650;

/// One `!trace` line's fields, by name.
struct Trace(BTreeMap<String, String>);

impl Trace {
    fn parse(line: &str) -> Option<Self> {
        let rest = line.strip_prefix("!trace ")?;
        Some(Trace(
            rest.split(' ')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ))
    }

    fn int(&self, k: &str) -> i32 {
        self.0[k].parse().unwrap()
    }

    fn origin(&self) -> [f32; 3] {
        let v: Vec<f32> = self.0["origin"]
            .split(',')
            .map(|c| c.parse().unwrap())
            .collect();
        [v[0], v[1], v[2]]
    }
}

fn read(path: &str) -> String {
    let path = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// What one killcam reads as, on either server: serverTimes of the kill, the
/// replay's first frame and the dead spawn after it, and the replay's age.
#[derive(Debug, PartialEq)]
struct Killcam {
    kill: i32,
    start: i32,
    end: i32,
    age: i32,
}

/// The retail capture's killcam. The target probe traces each snapshot whose
/// watched fields moved, and the replay is the stretch where the dead target
/// reads a live `pm_type` 0 before it is dead again. Its age is the offset at
/// which every replayed origin lies on the shooter's own trail.
fn retail() -> (Killcam, Trace, Trace) {
    let target = read(TARGET);
    let traces: Vec<Trace> = target.lines().filter_map(Trace::parse).collect();
    let kill_ms = target
        .lines()
        .filter_map(|l| l.strip_prefix("!obituary "))
        .map(|l| Trace::parse(&format!("!trace {l}")).unwrap())
        .find(|o| o.int("victim") != o.int("attacker"))
        .expect("the capture has a kill by the shooter")
        .int("ms");
    let at = traces.iter().position(|t| t.int("ms") == kill_ms).unwrap();
    assert_eq!(traces[at].int("pm_type"), PM_DEAD);
    let start = at
        + traces[at..]
            .iter()
            .position(|t| t.int("pm_type") == 0)
            .unwrap();
    let end = start
        + traces[start..]
            .iter()
            .position(|t| t.int("pm_type") == PM_DEAD)
            .unwrap();

    let trail: BTreeMap<i32, [f32; 3]> = read(SHOOTER)
        .lines()
        .filter_map(Trace::parse)
        .map(|t| (t.int("serverTime"), t.origin()))
        .collect();
    let replay = &traces[start..end];
    let age = (0..=ASKED_MS)
        .step_by(50)
        .find(|d| {
            let on: Vec<bool> = replay
                .iter()
                .filter_map(|t| {
                    let then = trail.get(&(t.int("serverTime") - d))?;
                    Some(*then == t.origin())
                })
                .collect();
            on.len() >= 5 && on.iter().all(|m| *m)
        })
        .expect("no age puts the replay on the shooter's trail");
    let k = Killcam {
        kill: traces[at].int("serverTime"),
        start: traces[start].int("serverTime"),
        end: traces[end].int("serverTime"),
        age,
    };
    let last = Trace(traces[end - 1].0.clone());
    (k, last, Trace(traces[end].0.clone()))
}

/// Retail's numbers, which the rule has to reproduce: the replay 2000 ms
/// after the kill, an age of 8650 where 9000 was asked, the end that age
/// after the start, and the dead spawn a bare playerstate where the replay
/// left the shooter, its teleport bit the replay's flipped.
#[test]
fn the_retail_capture_reads_as_the_rule() {
    let (k, last, spawn) = retail();
    assert_eq!(k.start - k.kill, DELAY_MS);
    assert_eq!(k.age, 8650);
    assert_eq!(k.age, k.start - (k.kill - RETAIL_SPAWN_TO_KILL_MS));
    assert_eq!(k.end - k.start, k.age);
    assert_eq!(spawn.int("health"), 0);
    assert_eq!(spawn.int("eventSequence"), 0);
    assert_eq!(spawn.0["clip"], "-");
    assert_eq!(spawn.0["ammo"], "-");
    assert_eq!(spawn.origin(), last.origin());
    assert_ne!(
        spawn.int("eFlags") & EF_TELEPORT_BIT,
        last.int("eFlags") & EF_TELEPORT_BIT
    );
    // The kill's own obituary comes round again inside the replay, one age
    // after it went out: the archived frame carries the temp entity.
    let target = read(TARGET);
    let obituaries: Vec<i32> = target
        .lines()
        .filter_map(|l| l.strip_prefix("!obituary "))
        .map(|l| Trace::parse(&format!("!trace {l}")).unwrap())
        .filter(|o| o.int("victim") != o.int("attacker"))
        .map(|o| o.int("ms"))
        .collect();
    assert_eq!(obituaries.len(), 2);
    let served = |ms: i32| {
        target
            .lines()
            .filter_map(Trace::parse)
            .find(|t| t.int("ms") == ms)
            .unwrap()
            .int("serverTime")
    };
    assert_eq!(served(obituaries[1]) - served(obituaries[0]), k.age);
}

/// Our server on retail's schedule: the target joins, the shooter joins
/// later, and the shooter's killing round lands `RETAIL_SPAWN_TO_KILL_MS`
/// after its first archived frame.
#[test]
fn ours_trims_and_ends_the_killcam_as_retail_does() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let p = &PROTOCOL_V1;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "tdm"), now);
    sv.set_cvar("scr_friendlyfire", "1");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    let mut ca = NetClient::start_with_qport(common::ClientEnd(qa.clone()), now, 0x2001);
    let mut cb = NetClient::start_with_qport(common::ClientEnd(qb.clone()), now, 0x2002);
    let (mut ja, mut jb) = (
        Join::new("allies", "m1carbine_mp"),
        Join::new("allies", "m1carbine_mp"),
    );

    // Both connect; the target answers its menus at once and the shooter
    // holds its answers ten seconds, so the level's archive is older than
    // the shooter's first view by more than the killcam asks for.
    let mut held: Vec<NetEvent> = Vec::new();
    let mut shooter_view = None;
    for frame in 0..800 {
        now += Duration::from_millis(50);
        ca.send_frame(&NULL_USERCMD);
        cb.send_frame(&NULL_USERCMD);
        let (ea, eb) = common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        held.extend(ea);
        let ea = if frame >= 200 {
            std::mem::take(&mut held)
        } else {
            Vec::new()
        };
        for (events, join, cl) in [(ea, &mut ja, &mut ca), (eb, &mut jb, &mut cb)] {
            for e in events {
                if let NetEvent::ServerCommand(tokens) = e {
                    join.on_server_command(&tokens, cl, now);
                }
            }
        }
        if let Some(s) = ca.snapshots().newest()
            && shooter_view.is_none() && s.ps.field_i32(p, "pm_flags") & 0x40000 != 0 {
                shooter_view = Some(s.server_time);
            }
        if ja.settled(now) && jb.settled(now) {
            break;
        }
    }
    assert!(ja.settled(now) && jb.settled(now), "{}", ja.summary());
    let shooter_view = shooter_view.expect("the shooter never had a view of its own");
    let na = ca
        .snapshots()
        .newest()
        .unwrap()
        .ps
        .field_i32(p, "clientNum") as usize;
    let nb = cb
        .snapshots()
        .newest()
        .unwrap()
        .ps
        .field_i32(p, "clientNum") as usize;
    let spot = ca.snapshots().newest().unwrap().ps.origin(p);
    assert!(sv.test_clear_line(spot, 0.0, 40.0));
    sv.place_client(na, spot, 0.0);
    sv.place_client(nb, [spot[0] + 40.0, spot[1], spot[2]], 180.0);
    let facing_a = UserCmd {
        angles: [0, 32768, 0],
        ..NULL_USERCMD
    };
    let fire = UserCmd {
        buttons: BUTTON_ADS | BUTTON_ATTACK,
        ..NULL_USERCMD
    };
    let ads = UserCmd {
        buttons: BUTTON_ADS,
        ..NULL_USERCMD
    };

    // Two head hits kill; the second goes out on the frame that puts the
    // kill where retail's was.
    let kill_due = shooter_view + RETAIL_SPAWN_TO_KILL_MS;
    let first_due = kill_due - 1500;
    let mut kill = None;
    let mut replay: Vec<vcod_common::net::snapshot::Snapshot> = Vec::new();
    let mut spawn = None;
    let mut trail: BTreeMap<i32, [f32; 3]> = BTreeMap::new();
    for _ in 0..600 {
        let next = sv_time(&ca) + 50;
        let a = if next == first_due || next == kill_due {
            &fire
        } else {
            &ads
        };
        now += Duration::from_millis(50);
        ca.send_frame(a);
        cb.send_frame(&facing_a);
        common::step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        let sa = ca.snapshots().newest().unwrap();
        trail.insert(sa.server_time, sa.ps.origin(p));
        let sb = cb.snapshots().newest().unwrap().clone();
        if kill.is_none() && sb.ps.field_i32(p, "pm_type") == PM_DEAD {
            kill = Some(sb.server_time);
        }
        if sb.ps.field_i32(p, "pm_flags") & 0x10000 != 0 {
            replay.push(sb);
        } else if !replay.is_empty() {
            spawn = Some(sb);
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let kill = kill.expect("the target was not killed");
    assert_eq!(kill, kill_due, "the kill landed off retail's schedule");
    let spawn = spawn.expect("the killcam never ended");
    let first = &replay[0];
    let last = replay.last().unwrap();
    let ours = Killcam {
        kill,
        start: first.server_time,
        end: spawn.server_time,
        age: first.ps.field_i32(p, "deltaTime"),
    };
    let (theirs, _, _) = retail();
    assert_eq!(
        ours,
        Killcam {
            kill,
            start: kill + (theirs.start - theirs.kill),
            end: kill + (theirs.end - theirs.kill),
            age: theirs.age,
        }
    );
    for s in &replay {
        assert_eq!(s.ps.field_i32(p, "clientNum"), na as i32);
        assert_eq!(s.ps.field_i32(p, "deltaTime"), ours.age);
        if let Some(then) = trail.get(&(s.server_time - ours.age)) {
            assert_eq!(s.ps.origin(p), *then);
        }
    }
    assert_eq!(spawn.ps.field_i32(p, "clientNum"), nb as i32);
    assert_eq!(spawn.ps.field_i32(p, "pm_type"), PM_DEAD);
    // Retail's live runs read 0x40800: the own view and `PMF_RESPAWNED`.
    assert_eq!(spawn.ps.field_i32(p, "pm_flags"), 0x40800);
    // The spawn's own think ran its 100 ms of dead pmove up to the frame's
    // clock: the eye has dropped 18 and `commandTime` is the frame's.
    assert_eq!(spawn.ps.field_i32(p, "commandTime"), spawn.server_time);
    assert_eq!(spawn.ps.field_f32(p, "viewHeightCurrent"), 42.0);
    assert_eq!(spawn.ps.health(), 0);
    assert_eq!(spawn.ps.field_i32(p, "eventSequence"), 0);
    assert!(spawn.ps.arrays.ammoclip.iter().all(|c| *c == 0));
    assert!(spawn.ps.arrays.ammo.iter().all(|c| *c == 0));
    assert_eq!(spawn.ps.origin(p), last.ps.origin(p));
    assert_ne!(
        spawn.ps.field_i32(p, "eFlags") & EF_TELEPORT_BIT,
        last.ps.field_i32(p, "eFlags") & EF_TELEPORT_BIT
    );
}

fn sv_time(c: &NetClient<common::ClientEnd>) -> i32 {
    c.snapshots().newest().map_or(0, |s| s.server_time)
}
