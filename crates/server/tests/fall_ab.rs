//! Fall damage against two retail runs of `client-probes/probe_fall`: one at
//! the stock bounds, one under `+set bg_fallDamageMinHeight 200 +set
//! bg_fallDamageMaxHeight 1000`. The probe drops a standing allied player
//! six times and wraps the damage and killed callbacks to log what the
//! engine hands them; each fixture holds those `PROBE` lines and the client's
//! `FALL` lines (docs/research/cod11-player-clip.md 8.10).
//!
//! Ours runs the same probe on the real server with a client sending 16 and
//! 17 ms cmds, and is held to:
//!
//! - the probe's lines, timestamps aside: the damage callback's arguments
//!   (undefined entities and vectors, `MOD_FALLING`, weapon and hit location
//!   `none`), the killed callback's (the world twice, a zero direction), and
//!   the session state each hit leaves. The damage figure and the health
//!   after are left out of the text compare and checked below;
//! - each landing's damage is `ClientEvents`' share of `maxhealth` for the
//!   landing pain's parm, on both sides, so the parm-to-health mapping is
//!   retail's even where ours lands a parm off (8.9: the impact speed moves
//!   with the frame length);
//! - no landing raises `EV_PAIN`, on either side: the fall's
//!   `pain_debounce_time` holds it off.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::PROTOCOL_V1;

const PROBE_PATH: &str = "maps/mp/gametypes/probe_fall";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_fall.gsc";
const MAP: &str = "mp_carentan";
const STOCK: &str = "tests/fixtures/playerstate/mp_carentan-dm-fall-damage.txt";
const CVARS: &str = "tests/fixtures/playerstate/mp_carentan-dm-fall-damage-cvars.txt";

const EV_LANDING_PAIN: std::ops::RangeInclusive<i32> = 116..=138;
const EV_PAIN: i32 = 187;

/// `ClientEvents`' damage for a landing pain's parm, transcribed from the
/// research doc (8.8) and held to retail's numbers by the gate itself.
fn expected_damage(percent: i32, max_health: i32) -> i32 {
    let share = if percent > 99 {
        f64::from(1.1f32)
    } else {
        f64::from(percent) * f64::from(0.01f32)
    };
    (f64::from(max_health) * share) as i32
}

/// The landing pains and `EV_PAIN`s a run raised, in order.
#[derive(Default, Debug)]
struct Events {
    pains: Vec<i32>,
    ev_pain: usize,
}

impl Events {
    fn take(&mut self, event: i32, parm: i32) {
        if EV_LANDING_PAIN.contains(&event) {
            self.pains.push(parm);
        } else if event == EV_PAIN {
            self.ev_pain += 1;
        }
    }
}

/// The events each new `seq` of a retail `FALL` line brought, off its ring.
fn retail_events(text: &str) -> Events {
    let mut out = Events::default();
    let mut last_seq = 0;
    for l in text.lines().filter(|l| l.starts_with("FALL ")) {
        let field = |k: &str| {
            l.split_whitespace()
                .find_map(|t| t.strip_prefix(k))
                .unwrap_or_else(|| panic!("no {k} in {l}"))
        };
        let list = |k: &str| -> Vec<i32> {
            field(k)
                .trim_matches(['[', ']'])
                .split(',')
                .map(|n| n.parse().unwrap())
                .collect()
        };
        let seq: i32 = field("seq=").parse().unwrap();
        let (events, parms) = (list("events="), list("parms="));
        for i in last_seq.max(seq - 4)..seq {
            out.take(events[(i & 3) as usize], parms[(i & 3) as usize]);
        }
        last_seq = seq;
    }
    out
}

/// The `PROBE` lines a fixture carries as comments.
fn retail_probe(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("# PROBE "))
        .map(|l| format!("PROBE {l}"))
        .collect()
}

/// A probe line with what the two sides are allowed to differ in masked:
/// every timestamp, the damage and the health after a hit, and where the
/// player came to rest.
fn shape(line: &str) -> String {
    let t: Vec<&str> = line.split_whitespace().collect();
    let mut out: Vec<String> = t.iter().map(|s| s.to_string()).collect();
    if out.len() > 2 {
        out[2] = "<t>".into();
    }
    match t.get(1) {
        Some(&"damage") => {
            // `damage` is also the line's own kind, at index 1.
            if let Some(i) = (2..t.len()).find(|&i| t[i] == "damage") {
                out[i + 1] = "<n>".into();
            }
        }
        Some(&"damaged") => out[4] = "<n>".into(),
        Some(&"after") => out.truncate(4),
        _ => {}
    }
    out.join(" ")
}

/// The number after `key` in a probe line.
fn num_after(line: &str, key: &str) -> i32 {
    let t: Vec<&str> = line.split_whitespace().collect();
    // `damage` is also the line's own kind, at index 1.
    let i = (2..t.len())
        .find(|&i| t[i] == key)
        .unwrap_or_else(|| panic!("no {key} in {line}"));
    t[i + 1].parse().unwrap()
}

/// Each damage line's figure against the parm of the landing that caused
/// it, and the health the hit left against it.
fn check_damage(side: &str, probe: &[String], pains: &[i32]) -> Vec<String> {
    let mut diffs = Vec::new();
    let hits: Vec<(&String, &String)> = probe
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("PROBE damage "))
        .map(|(i, l)| {
            let after = probe[i + 1..]
                .iter()
                .find(|n| n.starts_with("PROBE damaged "))
                .expect("a damaged line after every damage line");
            (l, after)
        })
        .collect();
    if hits.len() != pains.len() {
        diffs.push(format!(
            "{side}: {} damage callbacks for {} landing pains {pains:?}",
            hits.len(),
            pains.len()
        ));
        return diffs;
    }
    for ((hit, after), &parm) in hits.iter().zip(pains) {
        let (damage, health, max) = (
            num_after(hit, "damage"),
            num_after(hit, "health"),
            num_after(hit, "maxhealth"),
        );
        let want = expected_damage(parm, max);
        if damage != want {
            diffs.push(format!(
                "{side}: parm {parm} at maxhealth {max} did {damage}, the share is {want}"
            ));
        }
        let left = num_after(after, "health");
        if left != (health - damage).max(0) {
            diffs.push(format!("{side}: {health} less {damage} left {left}"));
        }
    }
    diffs
}

struct Ours {
    probe: Vec<String>,
    events: Events,
    /// Configstring 1 as the client holds it at the end.
    systeminfo: String,
}

/// The probe on our server, `sets` as `+set`s, until it logs `done`.
fn run_ours(fs: vcod_common::pk3::Pk3Fs, sets: &[(&str, &str)]) -> Ours {
    let probe = std::fs::read_to_string(PROBE_SRC).expect("read the probe");
    let bsp_path = fs.resolve_map(MAP).expect("the map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("bsp");
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_fall"), now);
    sv.overlay_script(PROBE_PATH, &probe);
    sv.set_cvar("probe_teleport", "1");
    for (k, v) in sets {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut cl, _join) = common::join(&mut sv, &q, &mut now, "allies", "m1carbine_mp");

    let p = &PROTOCOL_V1;
    let mut events = Events::default();
    let mut last_seq = cl
        .snapshots()
        .newest()
        .map_or(0, |s| s.ps.field_i32(p, "eventSequence"));
    // 75 s of server time at most; the probe is done in about 60.
    for _ in 0..1500 {
        // The retail capture's cadence: 16 and 17 ms cmds.
        for ms in [16, 17, 17] {
            now += Duration::from_millis(ms);
            cl.pump_at(now);
            cl.send_frame(&NULL_USERCMD);
        }
        common::step(&mut sv, &q, &mut cl, now);
        if let Some(s) = cl.snapshots().newest() {
            let seq = s.ps.field_i32(p, "eventSequence");
            for i in last_seq.max(seq - 4)..seq {
                let slot = i & 3;
                events.take(
                    s.ps.field_i32(p, &format!("events[{slot}]")),
                    s.ps.field_i32(p, &format!("eventParms[{slot}]")),
                );
            }
            last_seq = seq;
        }
        if sv.script_log().iter().any(|l| l.starts_with("PROBE done")) {
            break;
        }
    }
    let probe = sv
        .script_log()
        .iter()
        .filter(|l| l.starts_with("PROBE "))
        .map(|l| l.trim_end().to_string())
        .collect();
    let systeminfo = cl.configstring(1).to_string();
    Ours {
        probe,
        events,
        systeminfo,
    }
}

/// `sets` are the two bounds as retail's systeminfo carried them.
fn gate(fixture: &str, sets: &[(&str, &str)]) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let text = std::fs::read_to_string(fixture).unwrap_or_else(|e| panic!("read {fixture}: {e}"));
    let retail = retail_probe(&text);
    let retail_events = retail_events(&text);
    assert!(
        retail.iter().any(|l| l.starts_with("PROBE done")),
        "{fixture} has no finished run"
    );
    let ours = run_ours(fs, sets);
    let mut diffs = Vec::new();

    for (key, stock) in [
        ("bg_fallDamageMaxHeight", "480"),
        ("bg_fallDamageMinHeight", "256"),
    ] {
        let want = sets
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(stock, |(_, v)| v);
        let got = vcod_common::net::info_value_for_key(&ours.systeminfo, key);
        if got != Some(want) {
            diffs.push(format!("systeminfo {key} is {got:?}, retail's {want}"));
        }
    }

    let (rs, os): (Vec<String>, Vec<String>) = (
        retail.iter().map(|l| shape(l)).collect(),
        ours.probe.iter().map(|l| shape(l)).collect(),
    );
    if rs != os {
        diffs.push(format!(
            "the probe lines differ\nretail:\n  {}\nours:\n  {}",
            rs.join("\n  "),
            os.join("\n  ")
        ));
    }
    diffs.extend(check_damage("retail", &retail, &retail_events.pains));
    diffs.extend(check_damage("ours", &ours.probe, &ours.events.pains));
    for (side, ev) in [("retail", &retail_events), ("ours", &ours.events)] {
        if ev.ev_pain != 0 {
            diffs.push(format!("{side}: {} EV_PAIN on a fall", ev.ev_pain));
        }
    }
    if std::env::var_os("FALL_REPORT").is_some() {
        eprintln!(
            "{fixture}: retail parms {:?}, ours {:?}",
            retail_events.pains, ours.events.pains
        );
    }
    assert!(diffs.is_empty(), "{fixture}:\n{}", diffs.join("\n"));
}

#[test]
fn fall_damage_matches_retail_at_the_stock_bounds() {
    gate(STOCK, &[]);
}

#[test]
fn fall_damage_follows_the_bound_cvars_as_retail_does() {
    gate(
        CVARS,
        &[
            ("bg_fallDamageMinHeight", "200"),
            ("bg_fallDamageMaxHeight", "1000"),
        ],
    );
}
