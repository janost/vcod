//! The decode gate: the committed 1.7 captures parse end to end through the
//! delta codec, every op up to `svc_EOF`, and `svc_EOF` lands on the
//! message's last bits (docs/research/cod4-netfields.md).

use vcod_cod4::net::capture::{self, Item, Session};
use vcod_cod4::net::clmsg::{self, ClientMessage, UserCmd};
use vcod_cod4::net::parse::{Client, Op, Parsed};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/net");

/// A capture walked both ways. Each server message must end exactly at its
/// block's last byte and re-encode to its block byte for byte, each client
/// message too.
pub struct Walk {
    pub client: Client,
    pub parsed: Vec<Parsed>,
    pub sent: Vec<ClientMessage>,
}

pub fn walk(name: &str) -> Walk {
    // A bare name is a fixture's; a path reads as it is.
    let path = if name.contains('/') {
        name.to_string()
    } else {
        format!("{FIXTURES}/{name}")
    };
    let text = std::fs::read_to_string(path).unwrap();
    let lines = capture::parse(&text).unwrap();
    let mut session = Session::new();
    let mut parsed = Vec::new();
    let mut sent = Vec::new();
    for l in &lines {
        match session
            .feed(l)
            .unwrap_or_else(|e| panic!("{name} at {} ms: {e:#}", l.ms))
        {
            Some(Item::Server { msg: m, parsed: p }) => {
                let client = &session.client;
                assert!(
                    p.exact,
                    "{name} message {}: svc_EOF at byte {} is not the block's end",
                    m.sequence, p.eof_at
                );
                let plain = client.write(&p).unwrap();
                let orig = client.huff.decompress(&m.block);
                if let Some(at) = (0..p.eof_at).find(|&i| plain.get(i) != orig.get(i)) {
                    panic!(
                        "{name} message {}: re-encode differs at byte {at} of {} ({:?})",
                        m.sequence, p.eof_at, p.ops
                    );
                }
                assert_eq!(
                    client.huff.compress(&plain),
                    m.block,
                    "{name} message {}",
                    m.sequence
                );
                parsed.push(p);
            }
            Some(Item::Client { msg, block, key }) => {
                let again =
                    clmsg::compress_ops(&session.client.huff, &msg.ops, key, &UserCmd::default());
                assert_eq!(again, block, "{name} client message {}", msg.sequence);
                sent.push(msg);
            }
            None => {}
        }
    }
    Walk {
        client: session.client,
        parsed,
        sent,
    }
}

#[test]
fn gamestate_decodes_every_baseline_to_checksum_feed() {
    let Walk { client, parsed, .. } = walk("handshake-1.7.txt");
    let gs = client.gamestate.as_ref().expect("a gamestate");
    assert_eq!(
        parsed
            .iter()
            .flat_map(|p| &p.ops)
            .filter(|o| matches!(o, Op::Gamestate))
            .count(),
        1
    );
    assert_eq!(gs.configstrings.len(), 479);
    assert_eq!(gs.map_center, [429.0, 126.5, 137.0]);
    assert_eq!(gs.systeminfo("sv_serverid").as_deref(), Some("16"));
    assert_eq!(gs.baselines.len(), 62);
    assert_eq!(gs.client_num, 0);
    assert_eq!(gs.checksum_feed, 995670164);
    for (n, e) in &gs.baselines {
        assert_eq!(e.number(), *n);
        // mp_crash's script movers (6) and general entities (0).
        assert!(
            matches!(e.etype(), 0 | 6),
            "baseline {n} eType {}",
            e.etype()
        );
        assert!(e.origin().iter().all(|v| v.abs() < 8192.0), "baseline {n}");
    }
}

#[test]
fn server_commands_ahead_of_the_gamestate_decode() {
    let Walk { client, parsed, .. } = walk("handshake-1.7-cmds.txt");
    let cmds: Vec<String> = parsed
        .iter()
        .flat_map(|p| &p.ops)
        .filter_map(|o| match o {
            Op::ServerCommand { text, .. } => Some(String::from_utf8_lossy(text).into_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        cmds,
        ["v ui_3dwaypointtext \"1\"", "v ui_deathicontext \"1\""]
    );
    assert_eq!(client.server_command_sequence, 2);
}

#[test]
fn spectator_snapshots_decode_to_svc_eof() {
    let Walk { client, .. } = walk("snapshots-1.7.txt");
    let gs = client.gamestate.as_ref().expect("a gamestate");
    let snaps = &client.snapshots;
    assert_eq!(snaps.len(), 301);
    assert_eq!(snaps[0].delta_num, 0);
    assert!(snaps[1..].iter().all(|s| s.delta_num > 0), "the rest delta");
    for s in snaps {
        let ct = s.ps.get("commandTime").unwrap() as i32;
        assert!(
            ct <= s.server_time && ct > s.server_time - 1000,
            "msg {}",
            s.message_num
        );
        assert_eq!(s.ps.get("clientNum"), Some(gs.client_num as u32));
        assert!(s.ps.origin_sent);
        let nums: Vec<u32> = s.entities.iter().map(|e| e.number()).collect();
        assert!(
            nums.windows(2).all(|w| w[0] < w[1]),
            "msg {}",
            s.message_num
        );
        assert!(s.entities.iter().all(|e| e.etype() < 18));
        assert_eq!(s.clients.len(), 1);
        assert_eq!(s.clients[0].0[0], 0);
    }
    // Time moves 50 ms a frame.
    assert!(
        snaps
            .windows(2)
            .all(|w| w[1].server_time > w[0].server_time)
    );
}

/// The gamestate leaves out the slots that hold their
/// `constantConfigStrings` default, and sends an empty string for a
/// defaulted slot that is empty on this server (cod4-handshake.md,
/// "Configstring defaults").
#[test]
fn configstring_defaults_fill_the_slots_left_out() {
    use vcod_cod4::net::constant_cs::{CONSTANT_CONFIGSTRINGS, default_of};
    let Walk { client, .. } = walk("handshake-1.7.txt");
    let gs = client.gamestate.as_ref().unwrap();
    assert_eq!(CONSTANT_CONFIGSTRINGS.len(), 832);
    for (&i, s) in &gs.configstrings {
        if let Some(d) = default_of(i) {
            // Slots from 821 compare case-blind on the server.
            let same = if i >= 821 {
                d.as_bytes().eq_ignore_ascii_case(s)
            } else {
                d.as_bytes() == s.as_slice()
            };
            assert!(!same, "slot {i} sent its own default");
        } else {
            assert!(!s.is_empty(), "slot {i}: an empty string with no default");
        }
    }
    let all = gs.all_configstrings();
    assert_eq!(all.len(), 2442);
    assert_eq!(all[933], b"weapon_beretta");
    assert!(!gs.configstrings.contains_key(&20));
    assert_eq!(all[20], b"bg_aimSpreadMoveSpeedThreshold");
    assert_eq!(all[390], b"PERKS_EAVESDROP");
    assert_eq!(all[2020], b"white");
    // 1.7 keeps its weapon list at 2258; 2271 stays empty.
    assert!(all[2258].starts_with(b"defaultweapon_mp "));
    assert_eq!(all[2271], b"");
    let filled = all.iter().filter(|s| !s.is_empty()).count();
    let defaulted = CONSTANT_CONFIGSTRINGS
        .iter()
        .filter(|(i, _)| !gs.configstrings.contains_key(i))
        .count();
    assert_eq!(
        filled,
        gs.configstrings.values().filter(|s| !s.is_empty()).count() + defaulted
    );
}

/// The free spectator's capture (`play-1.7-fly.txt`, its recipe in its
/// `#` header; cod4-pmove.md 13): both ways re-encode, and it flies at
/// spectator speed 400.
#[test]
fn fly_capture_decodes_and_reencodes() {
    let w = walk("play-1.7-fly.txt");
    let ps = |s: &vcod_cod4::net::parse::Snapshot, n: &str| s.ps.get(n).unwrap();
    let snaps = &w.client.snapshots;
    assert!(
        snaps.iter().all(|s| ps(s, "pm_type") == 4),
        "a spectator throughout"
    );
    assert!(snaps.iter().any(|s| ps(s, "speed") == 400));
    let x: Vec<f32> = snaps
        .iter()
        .map(|s| f32::from_bits(ps(s, "origin[0]")))
        .collect();
    let (lo, hi) = x
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    assert!(hi - lo > 500.0, "it flew: x {lo}..{hi}");
}

/// The playing capture (cod4-netfields.md 8): a walker on allies and a
/// spectator, both decoded both ways and re-encoded byte for byte.
#[test]
fn play_captures_decode_and_reencode() {
    use std::collections::BTreeMap;
    use vcod_cod4::net::clmsg::ClOp;
    for t in ["walk", "spec"] {
        let w = walk(&format!("play-1.7-{t}.txt"));
        let mut types: BTreeMap<u32, usize> = BTreeMap::new();
        let mut events: BTreeMap<u32, usize> = BTreeMap::new();
        for s in &w.client.snapshots {
            for e in &s.entities {
                *types.entry(e.etype()).or_default() += 1;
            }
        }
        for s in &w.client.snapshots {
            for (n, _) in &s.wire.entities {
                if let Some(e) = s.entities.iter().find(|e| e.number() == *n)
                    && e.etype() >= 17
                {
                    *events.entry(e.etype() - 17).or_default() += 1;
                }
            }
        }
        let mut ring: BTreeMap<u32, usize> = BTreeMap::new();
        let mut missiles = std::collections::BTreeSet::new();
        let pf = |n: &str| {
            vcod_cod4::net::fields::ENTITY_PLAYER
                .iter()
                .find(|f| f.name == n)
                .unwrap()
                .word()
        };
        for s in &w.client.snapshots {
            for i in 0..4 {
                let v = s.ps.get(&format!("events[{i}]")).unwrap();
                if v != 0 {
                    *ring.entry(v).or_default() += 1;
                }
            }
            for e in &s.entities {
                if e.etype() == 1 {
                    for i in 0..4 {
                        let v = e.0[pf(&format!("events[{i}]"))];
                        if v != 0 {
                            *ring.entry(1000 + v).or_default() += 1;
                        }
                    }
                }
                if e.etype() == 4 {
                    missiles.insert((e.number(), e.0[pf("weapon")]));
                }
            }
        }
        let (mut full, mut short, mut same, mut cmds) = (0, 0, 0, Vec::new());
        for m in &w.sent {
            for op in &m.ops {
                match op {
                    ClOp::Move { cmds: c, .. } => {
                        for (_, wire) in c {
                            match wire.branch {
                                clmsg::Branch::Same => same += 1,
                                clmsg::Branch::Long => full += 1,
                                clmsg::Branch::Short => short += 1,
                            }
                        }
                    }
                    ClOp::Command { text, .. } => {
                        cmds.push(String::from_utf8_lossy(text).into_owned())
                    }
                    ClOp::Eof => {}
                }
            }
        }
        // Players (1), the frag (4, weapon 40), script movers (6) and
        // temp entities (17 + 61); the walker's events on its own
        // playerstate, its body's on the spectator's.
        assert!(
            types.contains_key(&1) && types.contains_key(&6),
            "{t}: {types:?}"
        );
        assert_eq!(missiles.len(), 1, "{t}: {missiles:?}");
        assert_eq!(events.get(&61), Some(&3), "{t}: {events:?}");
        assert!(
            full > 0 && short > 0 && same > 0,
            "{t}: {full} {short} {same}"
        );
        if t == "walk" {
            for id in [26, 32, 33, 73, 76] {
                assert!(ring.contains_key(&id), "walk ring {ring:?}");
            }
            assert_eq!(
                cmds.last().map(String::as_str),
                Some("say \"vcod probe here\"")
            );
        } else {
            assert!(ring.contains_key(&(1000 + 73)), "spec ring {ring:?}");
        }
    }
}

/// Event ids of a decoded snapshot run: the playerstate ring's entries
/// and each event entity's (`eType` - 17), by name.
fn event_names(w: &Walk) -> std::collections::BTreeSet<&'static str> {
    use vcod_cod4::net::events::event_name;
    let mut out = std::collections::BTreeSet::new();
    for s in &w.client.snapshots {
        for i in 0..4 {
            if let Some(n) = event_name(s.ps.get(&format!("events[{i}]")).unwrap()) {
                out.insert(n);
            }
        }
        for e in &s.entities {
            if e.etype() >= 17 {
                out.insert(event_name(e.etype() - 17).unwrap());
            }
        }
    }
    out
}

/// The round-3 walker's events read as 1.7's names (cod4-netfields.md,
/// "Event names").
#[test]
fn walk_capture_events_have_1_7_names() {
    use vcod_cod4::net::events::{EVENT_NAMES, event_name};
    assert_eq!(EVENT_NAMES.len(), 135);
    let names: Vec<_> = [6, 14, 26, 32, 33, 34, 73, 76, 99]
        .map(|id| event_name(id).unwrap())
        .into();
    assert_eq!(
        names,
        [
            "EV_STANCE_FORCE_STAND",
            "EV_RESET_ADS",
            "EV_FIRE_WEAPON",
            "EV_PREP_OFFHAND",
            "EV_USE_OFFHAND",
            "EV_SWITCH_OFFHAND",
            "EV_FOOTSTEP_RUN",
            "EV_JUMP",
            "EV_LANDING_ASPHALT",
        ]
    );
    let seen = event_names(&walk("play-1.7-walk.txt"));
    for n in &names {
        assert!(seen.contains(n), "{n} not in {seen:?}");
    }
    // The frag's explode shakes the screen: the temp entity 17 + 61.
    assert!(seen.contains("EV_EARTHQUAKE"), "{seen:?}");
}

/// The kill capture (cod4-netfields.md 10): on a pure `devmap` server the
/// shooter (axis, `cp` with every iwd's pure checksum) teleports beside the
/// target (allies, the bare `cp` key) and kills it. Both decode and
/// re-encode byte for byte; the target sees its death, its corpse, its
/// dropped rifle, the obituary, the killcam and its respawn.
#[test]
fn kill_captures_decode_and_reencode() {
    use vcod_cod4::net::clmsg::ClOp;
    use vcod_cod4::net::pure::verify_cp;
    let target = walk("play-1.7-kill-target.txt");
    let shooter = walk("play-1.7-kill-shooter.txt");
    for (w, sums) in [(&target, 0), (&shooter, 14)] {
        let gs = w.client.gamestate.as_ref().unwrap();
        assert_eq!(gs.systeminfo("sv_pure").as_deref(), Some("1"));
        assert_eq!(gs.systeminfo("sv_cheats").as_deref(), Some("1"));
        let first = w
            .sent
            .iter()
            .flat_map(|m| &m.ops)
            .find_map(|o| match o {
                ClOp::Command { sequence, text } => Some((*sequence, text.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(first.0, 1);
        let cp = String::from_utf8(first.1).unwrap();
        let args: Vec<&str> = cp.split_whitespace().collect();
        assert_eq!(args.len(), 3 + sums, "{cp}");
        let given: Vec<i32> = args[2..2 + sums]
            .iter()
            .map(|a| a.parse().unwrap())
            .collect();
        assert!(verify_cp(&args, &given, gs.checksum_feed), "{cp}");
        // Neither was dropped: snapshots run to the capture's end.
        assert!(w.client.snapshots.len() > 300);
    }

    let snaps = &target.client.snapshots;
    let pm = |s: &vcod_cod4::net::parse::Snapshot| s.ps.get("pm_type").unwrap();
    let died = snaps.iter().position(|s| pm(s) == 7).expect("a death");
    let back = died
        + snaps[died..]
            .iter()
            .position(|s| pm(s) == 0)
            .expect("a respawn");
    let at = |i: usize| snaps[i].ps.get_f32("origin[0]").unwrap();
    assert!((at(back) - at(died)).abs() > 500.0, "respawned elsewhere");
    let types: std::collections::BTreeSet<u32> = snaps
        .iter()
        .flat_map(|s| s.entities.iter().map(|e| e.etype()))
        .collect();
    // Player, corpse, item, script mover; the bullet's hit on a client and
    // the obituary as temp entities.
    for t in [1, 2, 3, 6, 17 + 43, 17 + 66] {
        assert!(types.contains(&t), "eType {t} not in {types:?}");
    }
    let body = snaps
        .iter()
        .flat_map(|s| &s.entities)
        .find(|e| e.etype() == 2)
        .unwrap();
    assert!(
        (64..72).contains(&body.number()),
        "body queue {}",
        body.number()
    );
    let ob = snaps
        .iter()
        .flat_map(|s| &s.entities)
        .find(|e| e.etype() == 17 + 66)
        .unwrap();
    let f = |n: &str| {
        let w = vcod_cod4::net::delta::EntityState::table(17 + 66)
            .iter()
            .find(|f| f.name == n)
            .unwrap()
            .word();
        ob.0[w]
    };
    // Victim 0, attacker 1, the M16's weapon index.
    assert_eq!(
        (f("otherEntityNum"), f("attackerEntityNum"), f("eventParm")),
        (0, 1, 27)
    );
    // The shooter's own client sees its rounds as plain bullet hits
    // (`sv_clientSideBullets`), the target as hits on a client.
    let seen = event_names(&shooter);
    assert!(seen.contains("EV_BULLET_HIT") && !seen.contains("EV_BULLET_HIT_CLIENT_LARGE"));
    assert!(event_names(&target).contains("EV_BULLET_HIT_CLIENT_LARGE"));
}

/// `sv_iwds` lists each iwd's unkeyed checksum: the CRC-32 of its entries'
/// CRC-32s (cod4-handshake.md, "The pure check"). With `$COD4_DIR` the
/// install's iwds give the capture's list, and the shooter's `cp` carries
/// exactly their checksums keyed with the gamestate's feed.
#[test]
fn iwd_checksums_match_the_server() {
    use vcod_cod4::net::pure::{loaded_iwds, pure_command};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let main = install.root().join("main");
    for name in ["handshake-1.7.txt", "play-1.7-kill-shooter.txt"] {
        let w = walk(name);
        let gs = w.client.gamestate.as_ref().unwrap();
        let si = String::from_utf8_lossy(gs.configstring(1)).into_owned();
        assert_eq!(loaded_iwds(&si).len(), 14);
        let cp = pure_command(&main, &si, gs.checksum_feed).unwrap();
        assert_eq!(cp.split_whitespace().count(), 2 + 14 + 1, "{cp}");
        if name.contains("shooter") {
            let sent = w
                .sent
                .iter()
                .flat_map(|m| &m.ops)
                .find_map(|o| match o {
                    vcod_cod4::net::clmsg::ClOp::Command { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .unwrap();
            assert_eq!(String::from_utf8(sent).unwrap(), cp);
        }
    }
}

/// `COD4_REENCODE=<capture>`: any capture, a run against vcod-server's CoD4
/// server included, walked through the same gate.
#[test]
fn an_env_named_capture_reencodes() {
    let Some(path) = std::env::var_os("COD4_REENCODE") else {
        return;
    };
    let w = walk(&path.to_string_lossy());
    eprintln!(
        "{} server messages, {} client",
        w.parsed.len(),
        w.sent.len()
    );
    assert!(!w.parsed.is_empty());
}

/// `record`'s opening gamestate (cod4-demos.md 3) off the handshake's
/// parsed one parses back to the same table, baselines, clientNum and
/// checksumFeed, every slot explicit, and ends on its block's last byte.
#[test]
fn a_demo_gamestate_parses_back_to_the_live_one() {
    use vcod_cod4::net::demo;
    let Walk { client, .. } = walk("handshake-1.7.txt");
    let gs = client.gamestate.as_ref().unwrap();
    let msg = demo::gamestate_message(&client.huff, 3, gs.server_command_sequence, gs);
    assert_eq!(&msg[..4], &3i32.to_le_bytes());
    let mut back = Client::new();
    let p = back.parse(9, &msg[4..]).unwrap();
    assert!(
        p.exact,
        "svc_EOF at byte {} is not the block's end",
        p.eof_at
    );
    // The plain bytes `CL_Record_f` stores ahead of the bit stream
    // (0x468b2a..0x468bd0) and after the baselines (0x468d03..0x468d32):
    // svc_gamestate, serverCommandSequence, svc_configstring, the count;
    // svc_EOF, clientNum, checksumFeed, svc_EOF.
    let plain = client.huff.decompress(&msg[4..]);
    let held = back.gamestate.as_ref().unwrap().configstrings.len() as i16;
    let mut head = vec![1u8];
    head.extend(gs.server_command_sequence.to_le_bytes());
    head.push(2);
    head.extend(held.to_le_bytes());
    assert_eq!(&plain[..8], head.as_slice());
    let mut tail = vec![7u8];
    tail.extend(gs.client_num.to_le_bytes());
    tail.extend(gs.checksum_feed.to_le_bytes());
    tail.push(7);
    assert_eq!(&plain[p.eof_at - 10..p.eof_at], tail.as_slice());
    assert!(matches!(p.ops.as_slice(), [Op::Gamestate]));
    let b = back.gamestate.as_ref().unwrap();
    assert_eq!(b.all_configstrings(), gs.all_configstrings());
    // Every default the client filled in is carried as a string.
    assert!(b.configstrings.len() > gs.configstrings.len());
    assert_eq!(b.baselines, gs.baselines);
    assert_eq!(b.client_num, gs.client_num);
    assert_eq!(b.checksum_feed, gs.checksum_feed);
    assert_eq!(b.server_command_sequence, gs.server_command_sequence);
    assert_eq!(b.map_center, gs.map_center);
}
