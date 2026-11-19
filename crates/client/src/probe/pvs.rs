//! `--save-entities` and `--probe-pvs`: the entity list along a route and its fixture.

use super::*;

/// One leg of the `--probe-pvs` route: a heading relative to where the spawn
/// faced, walked for `walk_ms`, then stood still until the station is taken.
struct PvsLeg {
    label: &'static str,
    yaw_deg: i32,
    walk_ms: u64,
}

/// How long a leg stands still before its station is recorded, so a moving
/// entity settles and its motion is not read as the list changing.
const PVS_STAND: Duration = Duration::from_millis(1500);

/// Ground covered below this inside [`PVS_STUCK_WINDOW`] means the walk is
/// against geometry; the leg turns 45 degrees rather than spending itself on a
/// wall. Retail's run speed is ~190 u/s, so a clear leg covers ~280u per window.
pub(super) const PVS_STUCK_UNITS: f32 = 60.0;

pub(super) const PVS_STUCK_WINDOW: Duration = Duration::from_millis(1500);

/// A wander: twelve stations, each leg a heading the walk keeps unless geometry
/// turns it. Legs repeat and reverse headings on purpose, so the route crosses
/// its own ground and a set that changes with position is told apart from one
/// that only grows with time.
fn pvs_route() -> Vec<PvsLeg> {
    let leg = |label, yaw_deg| PvsLeg {
        label,
        yaw_deg,
        walk_ms: 5000,
    };
    vec![
        PvsLeg {
            label: "spawn",
            yaw_deg: 0,
            walk_ms: 0,
        },
        leg("out", 0),
        leg("across", 90),
        leg("on", 0),
        leg("back", 180),
        leg("over", 270),
        leg("far", 180),
        leg("side", 90),
        leg("return", 0),
        leg("wide", 90),
        leg("long", 180),
        leg("last", 270),
    ]
}

/// A brush entity's `origin` key, zero when it carries none. Lump 27's bounds
/// are relative to it: mp_carentan's bombzones spell one and mp_pavlov's
/// triggers do not, so an absolute reading walks a whole map off.
pub(super) fn entity_origin(key: Option<&str>) -> [f32; 3] {
    let mut o = [0.0; 3];
    if let Some(v) = key {
        for (i, n) in v.split_whitespace().take(3).enumerate() {
            o[i] = n.parse().unwrap_or(0.0);
        }
    }
    o
}

pub(super) fn offset_by(v: [f32; 3], by: [f32; 3]) -> [f32; 3] {
    [v[0] + by[0], v[1] + by[1], v[2] + by[2]]
}

/// What identifies an entity across snapshots. The slot number alone cannot: a
/// freed slot is reused, so a reappearing number with a different `index` is a
/// different entity, not a visibility change.
#[derive(Clone, Copy, PartialEq)]
struct EntInfo {
    etype: i32,
    index: i32,
    solid: i32,
    client_num: i32,
    origin: [f32; 3],
}

impl EntInfo {
    fn of(p: &net::protocol::Cod1Wire, e: &net::msg::EntityState) -> Self {
        EntInfo {
            etype: e.field_i32(p, "eType"),
            index: e.field_i32(p, "index"),
            solid: e.field_i32(p, "solid"),
            client_num: e.field_i32(p, "clientNum"),
            origin: e.origin(p),
        }
    }
}

/// One recorded stop on the route: where the probe stood, and every entity
/// the server had sent it as of that frame.
pub(super) struct PvsStation {
    pub(super) label: &'static str,
    pub(super) origin: [f32; 3],
    pub(super) server_time: i32,
    pub(super) ents: BTreeMap<u32, net::msg::EntityState>,
}

/// Directory `crates/server/tests/entities_ab.rs` reads its fixtures from.
const ENTITIES_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/entities"
);

/// Writes the walked route as `<map>-<gametype>.txt`: one `[sample]` per
/// station, carrying where the probe stood and every entity the server had
/// sent it there.
///
/// The origin is the point of the file. A capture cannot be told to stand
/// anywhere (`setviewpos` is refused on a dedicated server), so the gate
/// does not replay the route, it replays each recorded origin
/// (docs/protocol-1.1.md, "Which entities a client is sent").
pub(super) fn write_entities_fixture(
    probe: &PvsProbe,
    configstrings: &[String],
    join: &JoinProbe,
    tag: Option<&str>,
) -> anyhow::Result<()> {
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
    let (map, gametype) = (key("mapname"), key("g_gametype"));

    let mut head = String::new();
    head.push_str(&format!(
        "# Retail CoD {} dedicated server entity trace along walked routes.\n",
        retail_build()
    ));
    head.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
    ));
    head.push_str(&format!(
        "# sv_maxclients {}, sv_pure {}, stock scr_* defaults, one client on the server.\n",
        key("sv_maxclients"),
        key("sv_pure"),
    ));
    head.push_str("# Captured with tools/run_server.sh and --net-probe --save-entities, one run\n");
    head.push_str("# per random spawn: a walk cannot be steered across the map, so coverage is\n");
    head.push_str("# several runs' stations sharing a file. Each run appends and rewrites,\n");
    head.push_str(&format!(
        "# keeping at most {MAX_SAMPLES_PER_SET} samples of any one entity set, since a fourth \
sample of a set\n"
    ));
    head.push_str("# already seen adds a position and no new visibility case.\n");
    head.push_str("# Every run is taken from a server one client has already joined and left:\n");
    head.push_str("# its ClientDisconnect frees entity 0, and G_FreeEntity's sweep turns every\n");
    head.push_str("# groundEntityNum still at G_Spawn's 0 (the script models, the turrets) to\n");
    head.push_str("# 1023, so a first run on a fresh server disagrees with every later one.\n");
    head.push_str(
        "# One [sample] per station, carrying the probe's origin there and every entity\n",
    );
    head.push_str(
        "# the server had sent it as of that frame. The origin is what the gate replays:\n",
    );
    head.push_str("# the route itself is not reproducible and does not need to be.\n");
    head.push_str(
        "# Values are the raw i32 wire words, floats as their bit patterns. Names come\n",
    );
    head.push_str("# from Protocol::entity_fields; a field a block does not list is zero.\n");

    let suffix = tag.map(|t| format!("-{t}")).unwrap_or_default();
    let path = fixture_path(
        ENTITIES_FIXTURE_DIR,
        &format!("{map}-{gametype}{suffix}.txt"),
    )?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let (body, taken, kept, sets) = merge_entity_samples(&existing, &probe.stations);
    let mut out = head;
    out.push_str(&body);

    std::fs::create_dir_all(ENTITIES_FIXTURE_DIR)?;
    // Each run's samples join the file's: it is this mode's append target,
    // so an existing file is no reason to refuse.
    std::fs::write(&path, &out)?;
    println!(
        "entities: {} stations this run, {} samples in the file ({} read, {} dropped as \
         repeats), {} distinct entity sets -> {path}",
        probe.stations.len(),
        kept,
        taken,
        taken + probe.stations.len() - kept,
        sets,
    );
    Ok(())
}

/// The samples of an entities fixture's `existing` text, then a sample per
/// station, at most [`MAX_SAMPLES_PER_SET`] of any one entity set, numbered
/// afresh; with how many were read, how many kept and the distinct sets.
pub(super) fn merge_entity_samples(
    existing: &str,
    stations: &[PvsStation],
) -> (String, usize, usize, usize) {
    let p = wire();
    let mut samples = parse_fixture_samples(existing);
    let taken = samples.len();
    for st in stations {
        let mut body = format!("serverTime {}\n", st.server_time);
        for (axis, v) in st.origin.iter().enumerate() {
            body.push_str(&format!("origin[{axis}] {}\n", v.to_bits() as i32));
        }
        for (num, e) in &st.ents {
            body.push_str(&format!("[ent {num}]\n"));
            for (f, v) in p.entity_fields.iter().zip(&e.fields) {
                if *v != 0 {
                    body.push_str(&format!("{} {v}\n", f.name));
                }
            }
        }
        samples.push(FixtureSample {
            label: st.label.to_string(),
            ents: st.ents.keys().copied().collect(),
            body,
        });
    }

    let mut out = String::new();
    let mut per_set: std::collections::HashMap<Vec<u32>, usize> = std::collections::HashMap::new();
    let mut kept = 0;
    for s in &samples {
        let n = per_set.entry(s.ents.clone()).or_insert(0);
        if *n >= MAX_SAMPLES_PER_SET {
            continue;
        }
        *n += 1;
        out.push_str(&format!("[sample {kept}] {}\n", s.label));
        out.push_str(&s.body);
        kept += 1;
    }
    (out, taken, kept, per_set.len())
}

/// At most this many samples of one entity set survive in a fixture. A fourth
/// sample of a set already seen adds a position and no new visibility case,
/// and the cap keeps a many-station map's fixture small.
const MAX_SAMPLES_PER_SET: usize = 3;

/// A sample as it sits in the fixture. The body is kept verbatim so a reread
/// and rewrite never has to understand the fields it carries.
pub(super) struct FixtureSample {
    pub(super) label: String,
    pub(super) ents: Vec<u32>,
    pub(super) body: String,
}

/// Splits a fixture into its samples, dropping the header. Tolerant by
/// design: an unreadable file reads as no samples and the run starts the map
/// over rather than failing after the capture is already spent.
pub(super) fn parse_fixture_samples(text: &str) -> Vec<FixtureSample> {
    let mut out: Vec<FixtureSample> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("[sample ") {
            let label = rest
                .split_once("] ")
                .map(|(_, l)| l)
                .unwrap_or("")
                .to_string();
            out.push(FixtureSample {
                label,
                ents: Vec::new(),
                body: String::new(),
            });
            continue;
        }
        let Some(s) = out.last_mut() else {
            continue;
        };
        if let Some(num) = line
            .strip_prefix("[ent ")
            .and_then(|r| r.strip_suffix(']'))
            .and_then(|n| n.parse::<u32>().ok())
        {
            s.ents.push(num);
        }
        s.body.push_str(line);
        s.body.push('\n');
    }
    out
}

/// Walks [`pvs_route`] and keeps the snapshot entity list at each station,
/// printing every appearance and removal in between with the position it
/// happened at. The question it answers: does the server decide the entity
/// list from where the client stands?
pub(super) struct PvsProbe {
    legs: Vec<PvsLeg>,
    idx: usize,
    started: Option<Instant>,
    /// Added to every remaining leg's heading once a walk stalls against
    /// geometry. It carries across legs on purpose: a leg that reset it would
    /// walk straight back into the wall the last one escaped.
    detour_deg: i32,
    /// Time and position the current stuck window opened at.
    last_progress: Option<(Instant, [f32; 3])>,
    pub(super) spawn_yaw: ViewHold,
    /// The previous snapshot's entities, which the next one is diffed against.
    live: BTreeMap<u32, EntInfo>,
    /// Newest snapshot already diffed, so the diff runs per snapshot rather
    /// than per loop iteration.
    traced: Option<u32>,
    stations: Vec<PvsStation>,
    done: bool,
}

impl Default for PvsProbe {
    fn default() -> Self {
        Self {
            legs: pvs_route(),
            idx: 0,
            started: None,
            detour_deg: 0,
            last_progress: None,
            spawn_yaw: ViewHold::default(),
            live: BTreeMap::new(),
            traced: None,
            stations: Vec::new(),
            done: false,
        }
    }
}

impl PvsProbe {
    pub(super) fn running(&self) -> bool {
        !self.done
    }

    pub(super) fn cmd(&self) -> net::msg::UserCmd {
        let Some(leg) = self.legs.get(self.idx) else {
            return net::msg::NULL_USERCMD;
        };
        let walking = self
            .started
            .is_some_and(|t| t.elapsed() < Duration::from_millis(leg.walk_ms));
        net::msg::UserCmd {
            angles: [
                0,
                (leg.yaw_deg + self.detour_deg).rem_euclid(360) * 65536 / 360,
                0,
            ],
            forward: if walking { 127 } else { 0 },
            ..net::msg::NULL_USERCMD
        }
    }

    /// Feeds the newest snapshot in. Returns true once the last station is
    /// recorded.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        if self.done {
            return true;
        }
        let Some(leg) = self.legs.get(self.idx) else {
            self.done = true;
            return true;
        };
        let (label, walk) = (leg.label, Duration::from_millis(leg.walk_ms));
        let started = *self.started.get_or_insert(now);
        let elapsed = now.duration_since(started);
        let me = snap.ps.origin(wire());
        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            self.diff(snap, label, elapsed, me);
        }
        if elapsed < walk {
            let (t0, o0) = *self.last_progress.get_or_insert((now, me));
            if now.duration_since(t0) >= PVS_STUCK_WINDOW {
                if dist(o0, me) < PVS_STUCK_UNITS {
                    self.detour_deg += 45;
                    let heading = self.legs[self.idx].yaw_deg + self.detour_deg;
                    println!(
                        "PVS {label}: only {:.0}u in {}ms, turning to {heading} deg",
                        dist(o0, me),
                        PVS_STUCK_WINDOW.as_millis(),
                    );
                }
                self.last_progress = Some((now, me));
            }
            return false;
        }
        if elapsed < walk + PVS_STAND {
            return false;
        }
        println!(
            "PVS station {label}: origin=[{:.0},{:.0},{:.0}], {} entities",
            me[0],
            me[1],
            me[2],
            self.live.len(),
        );
        for (num, e) in &self.live {
            println!(
                "  ent {num:>3} eType={:<3} index={:<4} solid={:<8} clientNum={:<3} @[{:>7.0},{:>7.0},{:>7.0}] d={:.0}",
                e.etype,
                e.index,
                e.solid,
                e.client_num,
                e.origin[0],
                e.origin[1],
                e.origin[2],
                dist(e.origin, me),
            );
        }
        self.stations.push(PvsStation {
            label,
            origin: me,
            server_time: snap.server_time,
            // The frame's own list, not the diff state: what the server sent
            // is what the fixture has to carry.
            ents: snap.entities.clone(),
        });
        self.idx += 1;
        self.started = None;
        self.last_progress = None;
        if self.idx >= self.legs.len() {
            self.done = true;
            return true;
        }
        false
    }

    /// Prints what appeared and what vanished since the previous snapshot. An
    /// entity that only moved is not a visibility change and is not printed.
    fn diff(
        &mut self,
        snap: &net::snapshot::Snapshot,
        label: &str,
        elapsed: Duration,
        me: [f32; 3],
    ) {
        let p = wire();
        let now: BTreeMap<u32, EntInfo> = snap
            .entities
            .iter()
            .map(|(n, e)| (*n, EntInfo::of(p, e)))
            .collect();
        let at = |e: &EntInfo| {
            format!(
                "eType={} index={} clientNum={} @[{:.0},{:.0},{:.0}] d={:.0} (me [{:.0},{:.0},{:.0}], {label} +{}ms)",
                e.etype,
                e.index,
                e.client_num,
                e.origin[0],
                e.origin[1],
                e.origin[2],
                dist(e.origin, me),
                me[0],
                me[1],
                me[2],
                elapsed.as_millis(),
            )
        };
        for (num, e) in &now {
            let fresh = match self.live.get(num) {
                None => true,
                // A reused slot: same number, different entity.
                Some(old) => old.index != e.index || old.etype != e.etype,
            };
            if fresh {
                println!("PVS +ent {num}: {}", at(e));
            }
        }
        for (num, e) in &self.live {
            if !now.contains_key(num) {
                println!("PVS -ent {num}: {}", at(e));
            }
        }
        self.live = now;
    }

    /// The table the run exists for: one row per entity slot seen anywhere,
    /// one column per station.
    pub(super) fn report(&self) {
        let all: std::collections::BTreeSet<u32> = self
            .stations
            .iter()
            .flat_map(|st| st.ents.keys().copied())
            .collect();
        println!("\nPVS report: {} stations", self.stations.len());
        for st in &self.stations {
            println!(
                "  {:<8} origin=[{:>7.0},{:>7.0},{:>7.0}] {} entities",
                st.label,
                st.origin[0],
                st.origin[1],
                st.origin[2],
                st.ents.len(),
            );
        }
        let head: Vec<&str> = self.stations.iter().map(|st| st.label).collect();
        println!("  ent  {}", head.join(" "));
        for num in &all {
            let row: Vec<String> = self
                .stations
                .iter()
                .zip(&head)
                .map(|(st, h)| {
                    let mark = if st.ents.contains_key(num) { "x" } else { "." };
                    format!("{mark:^width$}", width = h.len())
                })
                .collect();
            println!("  {num:>3}  {}", row.join(" "));
        }
        // The verdict, on the sets alone: same slots everywhere means the
        // route saw no culling, which is not the same as proving there is none.
        let first = self
            .stations
            .first()
            .map(|st| st.ents.keys().copied().collect::<Vec<_>>());
        let same = self
            .stations
            .iter()
            .all(|st| Some(st.ents.keys().copied().collect::<Vec<_>>()) == first);
        if same {
            println!(
                "PVS verdict: every station saw the same entity slots; this route found no position culling"
            );
        } else {
            println!(
                "PVS verdict: the entity list differs between stations; it depends on where the client stands"
            );
        }
    }
}
