//! The touch pass against a retail capture, trigger for trigger.
//!
//! The evidence is the retail *server's* `games_mp.log`, not a client
//! capture: `client-probes/probe_trigger.gsc` runs as the gametype, threads a
//! `waittill("trigger", other)` watcher onto every trigger entity the map
//! spawned, and logs one line per notify with the trigger's entity number and
//! the toucher's origin. `--net-probe --probe-triggers` walks a client across
//! the map's trigger belts to produce them; the fixture's own header carries
//! both commands.
//!
//! Ours runs the same probe script as its gametype through the same
//! `logPrint` channel, so the two sides are the same lines from the same
//! source, and the diff is a diff of the engine underneath it. Two things are
//! compared and neither is a wall clock: the census of trigger entities the
//! map spawned and survived the gametype's `_gameobjects` pass, and, at every
//! origin retail recorded a fire from, the set of trigger entity numbers that
//! fire there.
//!
//! What it does not cover: mp_pavlov's one `trigger_hurt` never fired. Its
//! brush is the kill volume under the floor and a walking player never reaches
//! it, so every fire line is a `trigger_multiple` and the hurt half of the
//! touch pass -- the cadence, the damage, the slow spawnflag -- has no retail
//! evidence behind it here. Entity 72 is pinned as existing and as surviving
//! the gametype's deletion pass, and no further. Nor does any other trigger
//! kind fire: `dm` on this map deletes its `trigger_use` and `trigger_lookat`
//! with the three `script_gameobjectname` entities, and the map ships no
//! `trigger_once` or `trigger_damage` at all.
//!
//! A capture cannot be replayed as a walk -- the route is not reproducible and
//! a retail spawn is random -- so the gate replays each recorded origin, the
//! way `entities_ab.rs` does (docs/protocol-1.1.md, "Which entities a client
//! is sent").
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use vcod_common::net::msg::NULL_USERCMD;

/// The probe script both sides run, as the gametype. Overlaid onto the pak
/// path a gametype is loaded from, since it ships in no pak.
const PROBE_PATH: &str = "maps/mp/gametypes/probe_trigger";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_trigger.gsc";

const MAP: &str = "mp_pavlov";
/// mp_pavlov's allies are russian, so the stock team menu offers the mosin.
const JOIN: (&str, &str) = ("allies", "mosin_nagant_mp");

const FIXTURE: &str = "tests/fixtures/triggers/mp_pavlov-dm-triggers.txt";

/// One `PROBE fire` record: which trigger fired, its classname, and where the
/// toucher stood. The toucher's own entity number is dropped -- it is the
/// client slot on both sides and says nothing about the touch.
#[derive(Debug, Clone, PartialEq)]
struct Fire {
    trigger: u32,
    class: String,
    origin: [f32; 3],
}

/// A retail `(x, y, z)` as the gsc vector renderer wrote it.
fn parse_vector(s: &str) -> Option<[f32; 3]> {
    let inner = s.trim().strip_prefix('(')?.strip_suffix(')')?;
    let mut it = inner.split(',').map(|p| p.trim().parse::<f32>());
    let v = [it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?];
    it.next().is_none().then_some(v)
}

/// `PROBE watch <num> <classname>` out of a log, as an ordered set.
fn watched(lines: &[String]) -> BTreeSet<(u32, String)> {
    lines
        .iter()
        .filter_map(|l| {
            let rest = l.trim_end().strip_prefix("PROBE watch ")?;
            let (num, class) = rest.split_once(' ')?;
            Some((num.parse().ok()?, class.to_string()))
        })
        .collect()
}

/// `PROBE fire <num> <classname> <toucher> (x, y, z)` out of a log, in order.
fn fires(lines: &[String]) -> Vec<Fire> {
    lines
        .iter()
        .filter_map(|l| {
            let rest = l.trim_end().strip_prefix("PROBE fire ")?;
            let mut it = rest.splitn(4, ' ');
            let trigger = it.next()?.parse().ok()?;
            let class = it.next()?.to_string();
            let _toucher = it.next()?;
            Some(Fire {
                trigger,
                class,
                origin: parse_vector(it.next()?)?,
            })
        })
        .collect()
}

/// One station of the replay: an origin retail fired from, and every trigger
/// it fired there. Consecutive lines sharing an origin are one pass of
/// retail's touch loop, so they belong to one station rather than to several.
struct Station {
    origin: [f32; 3],
    triggers: BTreeSet<u32>,
}

fn stations(fires: &[Fire]) -> Vec<Station> {
    let mut out: Vec<Station> = Vec::new();
    for f in fires {
        match out.last_mut() {
            Some(st) if st.origin == f.origin => {
                st.triggers.insert(f.trigger);
            }
            _ => out.push(Station {
                origin: f.origin,
                triggers: BTreeSet::from([f.trigger]),
            }),
        }
    }
    out
}

/// The committed capture, header stripped.
fn retail() -> Vec<String> {
    let text = std::fs::read_to_string(FIXTURE).unwrap_or_else(|e| panic!("read {FIXTURE}: {e}"));
    text.lines()
        .map(str::trim_end)
        .filter(|l| l.starts_with("PROBE "))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_touch_pass_matches_retail_on_mp_pavlov() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let retail_lines = retail();
    let retail_watch = watched(&retail_lines);
    let retail_fires = fires(&retail_lines);
    assert!(
        !retail_watch.is_empty() && !retail_fires.is_empty(),
        "{FIXTURE} carries no watch or no fire lines"
    );

    let probe =
        std::fs::read_to_string(PROBE_SRC).unwrap_or_else(|e| panic!("read the probe: {e}"));
    let bsp_path = fs.resolve_map(MAP).expect("the map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("the bsp");
    let fs = Rc::new(fs);

    let mut now = Instant::now();
    // `g_gametype` is the probe, exactly as retail ran it: the probe's own
    // `main()` is what calls the real gametype's, and `_gameobjects` deletes
    // by the list that gametype registered either way.
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_trigger"), now);
    sv.overlay_script(PROBE_PATH, &probe);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");

    let q = Rc::new(RefCell::new(common::Queues::default()));
    // The join is what puts a player in the world; it also spends the four
    // seconds the probe's own `wait 1` and the bootstrap need.
    let (mut cl, _join) = common::join(&mut sv, &q, &mut now, JOIN.0, JOIN.1);

    let ours_watch = watched(sv.script_log());
    assert_eq!(
        ours_watch,
        retail_watch,
        "the trigger census differs from retail: ours has {} entities, retail {}",
        ours_watch.len(),
        retail_watch.len()
    );

    // Each station is replayed from where retail recorded it. `place_client`
    // is the only way to stand anywhere: a spawn is weighted-random and
    // `setviewpos` is refused on a dedicated server.
    let mut drained = sv.script_log().len();
    let mut diffs: Vec<String> = Vec::new();
    let stations = stations(&retail_fires);
    for (i, st) in stations.iter().enumerate() {
        sv.place_client(0, st.origin, 0.0);
        now += Duration::from_millis(common::FRAME_MS as u64);
        cl.send_frame(&NULL_USERCMD);
        common::step(&mut sv, &q, &mut cl, now);

        let fresh: Vec<String> = sv.script_log()[drained..].to_vec();
        drained = sv.script_log().len();
        let ours: BTreeSet<u32> = fires(&fresh).iter().map(|f| f.trigger).collect();
        if ours != st.triggers {
            let at = sv.script_players();
            let stood = at.first().map(|(_, o)| *o).unwrap_or(st.origin);
            diffs.push(format!(
                "station {i} at [{:.0},{:.0},{:.0}] (ours stood at [{:.0},{:.0},{:.0}]): retail \
                 fired {:?}, ours {:?}",
                st.origin[0],
                st.origin[1],
                st.origin[2],
                stood[0],
                stood[1],
                stood[2],
                st.triggers,
                ours,
            ));
        }
    }

    assert!(
        diffs.is_empty(),
        "{} of {} stations differ from retail\n{}",
        diffs.len(),
        stations.len(),
        diffs.join("\n")
    );
}
