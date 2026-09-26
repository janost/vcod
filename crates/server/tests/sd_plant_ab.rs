//! The S&D plant, defuse and lookat against the retail captures.
//!
//! Three fixtures from one retail run (their headers carry the recipe):
//! `mp_carentan-sd-plant-attacker.txt` and `-sd-defuse-defender.txt` are the
//! two clients' sides, one `!cmd` per usercmd sent and one `!trace` per
//! snapshot; `triggers/mp_carentan-sd-lookat.txt` is the server's own
//! `games_mp.log` under `client-probes/probe_lookat.gsc`, which ours runs as
//! its gametype too. The measurements are written up in
//! `docs/research/cod11-gsc-object-model.md`, 23.1 to 23.4.
//!
//! The replay hands ours the same absolute view retail had. A capture cmd's
//! `angles` are wire words relative to retail's `delta_angles`, which the
//! fixture does not carry, so the offset is calibrated off the first cmd and
//! the trace that followed it and then tracked through retail's pitch clamp:
//! `PM_UpdateViewAngles` caps the view at 16000 short units (87.9 degrees)
//! by pushing `delta_angles[0]`, and the sweep's `pitch +30` and `+45`
//! stations both saturate there (object-model doc, 23.1). Our own
//! `NetClient::send_frame` subtracts our snapshot's `delta_angles` again.
//!
//! Needs `COD_DIR`; without the paks both tests return early.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{holding, step_pair, ClientEnd, Queues, SdFixture, SdTrace, CMD_MS, FRAME_MS};
use vcod_common::net::msg::{hud_field as h, HudElem, PlayerState, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::NetClient;
use vcod_server::Server;

const MAP: &str = "mp_carentan";
const PROBE_PATH: &str = "maps/mp/gametypes/probe_lookat";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_lookat.gsc";
const ATTACKER: &str = "tests/fixtures/playerstate/mp_carentan-sd-plant-attacker.txt";
const DEFENDER: &str = "tests/fixtures/playerstate/mp_carentan-sd-defuse-defender.txt";
const LOOKAT: &str = "tests/fixtures/triggers/mp_carentan-sd-lookat.txt";
/// Retail's configstring table under `sd`, for resolving its shader indices
/// to names. The probe run registered the same shaders in the same order.
const RETAIL_CS: &str = "tests/fixtures/configstrings/mp_carentan-sd.txt";

/// Where the gsc probe put the planter once the charge was down: the
/// attackers' courtyard spawn, off the defender's sightline. Ours moves it
/// there too, since a body on the ray is what stopped retail's lookat.
const PLANTER_PARK: [f32; 3] = [-512.0, 2688.0, -16.0];

const PITCH_CLAMP_SHORT: i32 = 16000;

/// Rows of the plant diff that are known divergences, each a substring of
/// the row it excuses.
const PLANT_GAPS: &[&str] = &[];

/// Rows of the defuse diff that are known divergences, same shape.
const DEFUSE_GAPS: &[&str] = &[];

/// Sweep stations `(yaw, pitch)` whose fired-or-not the hull edge decides
/// differently on ours, recorded with the number rather than widened over.
const LOOKAT_GAPS: &[(f32, f32)] = &[];

fn report() -> bool {
    std::env::var("SD_REPORT").is_ok_and(|v| v == "1")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn retail_cs() -> BTreeMap<usize, String> {
    read(RETAIL_CS)
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .filter_map(|l| {
            let (i, v) = l.split_once(' ')?;
            Some((i.parse().ok()?, v.to_string()))
        })
        .collect()
}

fn short_of(deg: f32) -> i32 {
    (deg * 65536.0 / 360.0).round() as i32
}

fn yaw_diff(a: f32, b: f32) -> f32 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// ------------------------------------------------------------------ the rig

struct Rig {
    sv: Server,
    qa: Rc<RefCell<Queues>>,
    qb: Rc<RefCell<Queues>>,
    ca: NetClient<ClientEnd>,
    cb: NetClient<ClientEnd>,
    now: Instant,
    /// How much of `sv.script_log()` the replay has already read.
    drained: usize,
}

impl Rig {
    /// One frame with both clients holding their weapon and no input.
    fn idle(&mut self) {
        self.now += Duration::from_millis(FRAME_MS as u64);
        self.ca.send_frame(&holding(&self.ca));
        self.cb.send_frame(&holding(&self.cb));
        step_pair(
            &mut self.sv,
            (&self.qa, &mut self.ca),
            (&self.qb, &mut self.cb),
            self.now,
        );
    }

    /// `place_client` plus the frames the client needs to see the placed
    /// spawn's `delta_angles`, sent the absolute view it is to hold.
    fn place(&mut self, slot: usize, origin: [f32; 3], view: [f32; 3]) {
        self.sv.place_client(slot, origin, view[1]);
        let angles = [short_of(view[0]), short_of(view[1]), 0];
        for _ in 0..6 {
            self.now += Duration::from_millis(FRAME_MS as u64);
            let (mut a, mut b) = (holding(&self.ca), holding(&self.cb));
            if slot == 0 {
                a.angles = angles;
            } else {
                b.angles = angles;
            }
            self.ca.send_frame(&a);
            self.cb.send_frame(&b);
            step_pair(
                &mut self.sv,
                (&self.qa, &mut self.ca),
                (&self.qb, &mut self.cb),
                self.now,
            );
        }
        self.drained = self.sv.script_log().len();
    }

    fn new_fires(&mut self) -> usize {
        let log = self.sv.script_log();
        let n = log[self.drained.min(log.len())..]
            .iter()
            .filter(|l| l.starts_with("PROBE fire "))
            .count();
        self.drained = log.len();
        n
    }

    /// `None` for an index our table holds no shader at.
    fn shader_name(&self, index: i32) -> Option<&str> {
        Some(self.sv.configstring(1500 + index as usize)).filter(|s| !s.is_empty())
    }
}

/// Two clients through the stock menus with the probe as the gametype, then
/// past the match-start restart, where `_gameobjects` makes the zones
/// plantable.
fn rig() -> Option<Rig> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = Server::new(common::cfg(MAP, "probe_lookat"), now);
    sv.overlay_script(PROBE_PATH, &read(PROBE_SRC));
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    let qa = Rc::new(RefCell::new(Queues::default()));
    let qb = Rc::new(RefCell::new(Queues::default()));
    let (ca, cb) = common::join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("axis", "kar98k_mp"),
    );
    let mut rig = Rig {
        sv,
        qa,
        qb,
        ca,
        cb,
        now,
        drained: 0,
    };
    let id_before = rig.sv.server_id();
    for _ in 0..200 {
        rig.idle();
    }
    assert_eq!(rig.sv.script_aborts(), Vec::<String>::new());
    assert_ne!(rig.sv.server_id(), id_before, "the match never restarted");
    Some(rig)
}

// --------------------------------------------------------------- the replay

/// One of our snapshots during a replay, at `ms` from the window's first cmd,
/// with the lookat fires the frame logged.
struct Ours {
    ms: i64,
    ps: PlayerState,
    fires: usize,
}

/// Every capture cmd with `st` in `[from, to]`, across phases, in order.
fn cmds_in(fx: &SdFixture, from: i32, to: i32) -> Vec<(i32, UserCmd)> {
    let mut out: Vec<(i32, UserCmd)> = fx
        .phases
        .iter()
        .flat_map(|p| p.cmds.iter().copied())
        .filter(|(st, _)| (from..=to).contains(st))
        .collect();
    out.sort_by_key(|(st, _)| *st);
    out
}

/// The `delta_angles` retail held when the window opened: the last cmd
/// before the first trace after it, against that trace's view.
fn calibrate(cmds: &[(i32, UserCmd)], traces: &[&SdTrace]) -> [i32; 3] {
    let first = traces
        .iter()
        .find(|t| t.server_time > cmds[0].0)
        .expect("a trace after the window's first cmd");
    let (_, cmd) = cmds
        .iter()
        .rev()
        .find(|(st, _)| *st <= first.server_time)
        .unwrap();
    [
        short_of(first.viewangles[0]) - cmd.angles[0],
        short_of(first.viewangles[1]) - cmd.angles[1],
        0,
    ]
}

/// Where a window's clock starts: the retail frame at or before its first
/// cmd, so our frame `i` pairs with retail's frame `(i + 1) * 50` ms on.
fn window_base(cmds: &[(i32, UserCmd)]) -> i32 {
    let st = cmds[0].0;
    st - st.rem_euclid(FRAME_MS as i32)
}

/// Replays `cmds` into `slot`'s client from `window_base` to `end`, two cmds
/// per frame on the 25 ms grid (a slot with no capture cmd holding the
/// last), sampling the client's newest snapshot after every frame. A cmd
/// goes into the frame retail ran it in, the first one stamped after its
/// `st`: the defender's view cmd at `st` 94850 is on retail's 94900
/// snapshot, not its 94850 one, and the planter's use at 86766 on 86800.
fn replay(
    rig: &mut Rig,
    slot: usize,
    cmds: &[(i32, UserCmd)],
    end: i32,
    mut delta: [i32; 3],
) -> Vec<Ours> {
    let t0 = window_base(cmds);
    let mut by_slot: BTreeMap<i64, UserCmd> = BTreeMap::new();
    for (st, cmd) in cmds {
        let grid = i64::from(st - t0) / CMD_MS * CMD_MS;
        let mut abs = *cmd;
        // Retail's clamp pushes `delta_angles[0]`; every later word is
        // relative to the pushed value.
        for i in 0..2 {
            let mut a = (cmd.angles[i] + delta[i]) & 0xffff;
            if a > 32767 {
                a -= 65536;
            }
            if i == 0 && a.abs() > PITCH_CLAMP_SHORT {
                let capped = PITCH_CLAMP_SHORT * a.signum();
                delta[0] -= a - capped;
                a = capped;
            }
            abs.angles[i] = a;
        }
        abs.angles[2] = 0;
        by_slot.insert(grid, abs);
    }
    let slots = (i64::from(end - t0) + CMD_MS - 1) / CMD_MS;
    let frames = slots / 2 + 1;
    let mut current = *by_slot.values().next().expect("a cmd in the window");
    let mut out = Vec::new();
    for i in 0..frames {
        for half in 0..2 {
            let t = i * FRAME_MS + half * CMD_MS;
            if let Some(c) = by_slot.get(&t) {
                current = *c;
            }
            rig.now += Duration::from_millis(CMD_MS as u64);
            rig.ca.pump_at(rig.now);
            rig.cb.pump_at(rig.now);
            // The capture's weapon byte is retail's CS 7 index; ours is
            // whatever our table gave the same weapon.
            let (mut a, mut b) = (holding(&rig.ca), holding(&rig.cb));
            let mine = if slot == 0 { &mut a } else { &mut b };
            mine.buttons = current.buttons;
            mine.wbuttons = current.wbuttons;
            mine.forward = current.forward;
            mine.right = current.right;
            mine.up = current.up;
            mine.angles = current.angles;
            rig.ca.send_frame(&a);
            rig.cb.send_frame(&b);
        }
        step_pair(
            &mut rig.sv,
            (&rig.qa, &mut rig.ca),
            (&rig.qb, &mut rig.cb),
            rig.now,
        );
        let fires = rig.new_fires();
        let cl = if slot == 0 { &rig.ca } else { &rig.cb };
        if let Some(s) = cl.snapshots().newest() {
            out.push(Ours {
                ms: (i + 1) * FRAME_MS,
                ps: s.ps.clone(),
                fires,
            });
        }
    }
    out
}

/// Our sample nearest a retail trace `rel` ms after the window's first cmd.
fn paired(ours: &[Ours], rel: i64) -> &Ours {
    let i = ((rel + FRAME_MS / 2) / FRAME_MS - 1).max(0) as usize;
    assert!(
        i < ours.len(),
        "retail trace at {rel} ms is past our last sample ({})",
        ours.len()
    );
    &ours[i]
}

// ----------------------------------------------------------------- the diff

/// The probe's two flags off a HUD array: a 64x64 shader element is the
/// plant/defuse icon, a shader element with a running `scaleTime` the bar.
fn icon_and_bar(elems: &[HudElem]) -> (bool, bool) {
    let icon = elems
        .iter()
        .any(|e| e.get(h::SHADER) != 0 && e.get(h::WIDTH) == 64 && e.get(h::HEIGHT) == 64);
    let bar = elems
        .iter()
        .any(|e| e.get(h::SHADER) != 0 && e.get(h::SCALE_TIME) != 0);
    (icon, bar)
}

/// The shader elements of the archived array followed by the current one, as
/// `name:w:h:fromW:fromH:scaleTime:x:y`, sorted. `scaleStartTime` is a clock
/// and left out.
fn shader_elems_ours(rig: &Rig, archived: &[HudElem], current: &[HudElem]) -> Vec<String> {
    let mut out: Vec<String> = archived
        .iter()
        .chain(current)
        .filter(|e| e.get(h::TYPE) == 3)
        .map(|e| {
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}",
                rig.shader_name(e.get(h::SHADER)).unwrap_or("?"),
                e.get(h::WIDTH),
                e.get(h::HEIGHT),
                e.get(h::FROM_WIDTH),
                e.get(h::FROM_HEIGHT),
                e.get(h::SCALE_TIME),
                e.get(h::X),
                e.get(h::Y)
            )
        })
        .collect();
    out.sort();
    out
}

/// The same off a trace's `hud=` column
/// (`type:shader:w:h:fromW:fromH:scaleStart:scaleTime:x:y` per element).
/// The column is the archived array followed by the current one: the
/// committed fixtures concatenate the two with no separator (their headers'
/// "unarchived" is the probe's old label for it), and a probe from 17e43b0 on
/// writes `archived|current` with `-` for an empty half. Both parse here.
fn shader_elems_retail(cs: &BTreeMap<usize, String>, hud: &str) -> Vec<String> {
    let mut out: Vec<String> = hud
        .split(|c: char| c.is_whitespace() || c == '|')
        .filter_map(|e| {
            let f: Vec<&str> = e.split(':').collect();
            (f.len() == 10 && f[0] == "3").then(|| {
                let shader: usize = f[1].parse().unwrap();
                let name = cs.get(&(1500 + shader)).map_or("?", String::as_str);
                format!(
                    "{name}:{}:{}:{}:{}:{}:{}:{}",
                    f[2], f[3], f[4], f[5], f[7], f[8], f[9]
                )
            })
        })
        .collect();
    out.sort();
    out
}

struct Diff {
    rows: Vec<String>,
    cs: BTreeMap<usize, String>,
}

impl Diff {
    fn row(
        &mut self,
        phase: &str,
        ms: i64,
        field: &str,
        retail: impl std::fmt::Display,
        ours: impl std::fmt::Display,
    ) {
        self.rows.push(format!(
            "{phase} ms={ms} {field}: retail {retail} ours {ours}"
        ));
    }

    /// One retail trace against our paired snapshot. `motion` compares
    /// `pm_type`, the ground entity and the origin; the plant's last frame
    /// turns it off, being the frame the gsc probe unlinked and teleported
    /// the planter rather than anything retail did (object-model doc, 23.2).
    #[allow(clippy::too_many_arguments)]
    fn trace(
        &mut self,
        rig: &Rig,
        phase: &str,
        t: &SdTrace,
        ms: i64,
        ours: &Ours,
        anchor: [f32; 3],
        motion: bool,
    ) {
        let p = &PROTOCOL_V1;
        let ps = &ours.ps;
        let view = ps.viewangles(p);
        if (view[0] - t.viewangles[0]).abs() > 1.0 || yaw_diff(view[1], t.viewangles[1]) > 1.0 {
            self.row(
                phase,
                ms,
                "viewangles",
                format!("{:.1},{:.1}", t.viewangles[0], t.viewangles[1]),
                format!("{:.1},{:.1}", view[0], view[1]),
            );
        }
        if motion {
            let pm_type = ps.field_i32(p, "pm_type");
            if pm_type != t.pm_type {
                self.row(phase, ms, "pm_type", t.pm_type, pm_type);
            }
            // The release window's first frame is the one retail's cmds
            // still ran linked; ours replays it from a placement instead.
            let placed = phase == "release" && ms == 0;
            let ground = ps.field_i32(p, "groundEntityNum");
            if ground != t.ground && !placed {
                self.row(phase, ms, "groundEntityNum", t.ground, ground);
            }
            // Absolute origins are not comparable (retail slid into its
            // link); the link's hold is. Retail moves nothing while linked.
            if t.pm_type == 1 {
                let moved = dist(ps.origin(p), anchor);
                let tol = if t.forward != 0 { 2.0 } else { 1.0 };
                if moved > tol {
                    self.row(
                        phase,
                        ms,
                        "origin moved while linked",
                        "0.0",
                        format!("{moved:.1}"),
                    );
                }
            }
        }
        let (icon, bar) = icon_and_bar(&ps.arrays.hud_archived);
        let (icon_c, bar_c) = icon_and_bar(&ps.arrays.hud_current);
        if (icon || icon_c) != t.icon {
            self.row(phase, ms, "icon", t.icon, icon || icon_c);
        }
        if (bar || bar_c) != t.bar {
            self.row(phase, ms, "bar", t.bar, bar || bar_c);
        }
        let ours_elems = shader_elems_ours(rig, &ps.arrays.hud_archived, &ps.arrays.hud_current);
        let retail_elems = shader_elems_retail(&self.cs, &t.hud);
        if ours_elems != retail_elems {
            self.row(
                phase,
                ms,
                "hud shader elements",
                format!("{retail_elems:?}"),
                format!("{ours_elems:?}"),
            );
        }
        for (slot, (state, icon, ent, team, origin)) in &t.objectives {
            let o = ps.arrays.objectives[*slot];
            if o.state != *state {
                self.row(
                    phase,
                    ms,
                    &format!("objective {slot} state"),
                    state,
                    o.state,
                );
            }
            let retail_icon = self
                .cs
                .get(&(1500 + *icon as usize))
                .filter(|s| !s.is_empty())
                .cloned();
            let ours_icon = rig.shader_name(o.icon);
            if retail_icon.is_none() {
                let row = format!("{phase} ms={ms} objective {slot}: unresolved icon {icon}");
                self.rows.push(row);
            } else if retail_icon.as_deref() != ours_icon {
                self.row(
                    phase,
                    ms,
                    &format!("objective {slot} icon"),
                    retail_icon.as_deref().unwrap_or("-"),
                    ours_icon.unwrap_or("-"),
                );
            }
            if o.ent_num != *ent || o.team_num != *team {
                self.row(
                    phase,
                    ms,
                    &format!("objective {slot} ent/team"),
                    format!("{ent}/{team}"),
                    format!("{}/{}", o.ent_num, o.team_num),
                );
            }
            let d = dist(o.origin_f32(), *origin);
            if d > 1.0 {
                self.row(
                    phase,
                    ms,
                    &format!("objective {slot} origin"),
                    format!("{origin:?}"),
                    format!("{:?}", o.origin_f32()),
                );
            }
        }
        for slot in 0..2 {
            let o = ps.arrays.objectives[slot];
            if !t.objectives.contains_key(&slot) && o.state != 0 {
                self.row(phase, ms, &format!("objective {slot} state"), 0, o.state);
            }
        }
    }

    fn finish(self, what: &str, gaps: &[&str]) {
        if report() {
            println!("{what}: {} rows", self.rows.len());
            for r in &self.rows {
                println!("  {r}");
            }
        }
        let open: Vec<&String> = self
            .rows
            .iter()
            .filter(|r| !gaps.iter().any(|g| r.contains(g)))
            .collect();
        assert!(
            open.is_empty(),
            "{what}: {} of {} rows differ from retail\n{}",
            open.len(),
            self.rows.len(),
            open.iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// Retail trace `t` in `phase`, against our sample at the trace's offset
/// from the window's first cmd.
fn diff_phase(
    diff: &mut Diff,
    rig: &Rig,
    fx: &SdFixture,
    phase: &str,
    ours: &[Ours],
    t0: i32,
    skip_last_motion: bool,
) {
    let ph = fx.phase(phase);
    let p = &PROTOCOL_V1;
    // Where ours stood when retail's link began: the origin a linked client
    // is held at, since absolute origins differ (retail slid into its link).
    let mut anchor: Option<[f32; 3]> = None;
    let n = ph.traces.len();
    for (i, t) in ph.traces.iter().enumerate() {
        let rel = i64::from(t.server_time - t0);
        let o = paired(ours, rel);
        let a = if t.pm_type == 1 {
            *anchor.get_or_insert_with(|| o.ps.origin(p))
        } else {
            anchor = None;
            [0.0; 3]
        };
        let ms = rel - i64::from(ph.traces[0].server_time - t0);
        diff.trace(rig, phase, t, ms, o, a, !(skip_last_motion && i + 1 == n));
    }
}

/// The plant: the attacker's `hold1` from the station, then, from where
/// retail's own client had slid to by the time it pressed again, `release`
/// and `hold2`. Returns the rig with the charge down. `with_abort` false
/// skips `hold1`, so a gate that only needs a charge down is not hostage to
/// whatever the abort's aftermath does on ours.
fn plant(rig: &mut Rig, attacker: &SdFixture, with_abort: bool) -> Diff {
    let cs = retail_cs();
    let mut diff = Diff {
        rows: Vec::new(),
        cs,
    };
    let (hold1, release, hold2) = (
        attacker.phase("hold1"),
        attacker.phase("release"),
        attacker.phase("hold2"),
    );

    // hold1: the use cmds sit at the end of `approach`, one frame before its
    // first trace; the window runs to the release's first snapshot.
    if with_abort {
        let cmds = cmds_in(
            attacker,
            hold1.traces[0].server_time - FRAME_MS as i32,
            release.traces[0].server_time - 1,
        );
        let traces: Vec<&SdTrace> = hold1.traces.iter().collect();
        let delta = calibrate(&cmds, &traces);
        rig.place(0, attacker.station.0, attacker.station.1);
        let ours = replay(rig, 0, &cmds, release.traces[0].server_time - 1, delta);
        diff_phase(
            &mut diff,
            rig,
            attacker,
            "hold1",
            &ours,
            window_base(&cmds),
            false,
        );
    }

    // release and hold2 from retail's own hold2 origin and view.
    let cmds = cmds_in(
        attacker,
        release.traces[0].server_time,
        hold2.traces.last().unwrap().server_time,
    );
    let traces: Vec<&SdTrace> = release.traces.iter().chain(hold2.traces.iter()).collect();
    let delta = calibrate(&cmds, &traces);
    let start = &hold2.traces[0];
    // `place_client` is a respawn retail never did: `become_player` resets
    // `pm_type`, flips the teleport bit, drops the link and clears the event
    // ring. It stays because retail's client slid about 30 units into its
    // link, so absolute origins differ; the anchor-drift check, which starts
    // from retail's hold2 spot, is what makes the positions comparable.
    rig.place(0, start.origin, start.viewangles);
    let ours = replay(
        rig,
        0,
        &cmds,
        hold2.traces.last().unwrap().server_time,
        delta,
    );
    let t0 = window_base(&cmds);
    diff_phase(&mut diff, rig, attacker, "release", &ours, t0, false);
    diff_phase(&mut diff, rig, attacker, "hold2", &ours, t0, true);
    assert_eq!(
        rig.sv.script_aborts(),
        Vec::<String>::new(),
        "script aborts during the plant"
    );
    diff
}

#[test]
fn the_plant_matches_retail_on_mp_carentan() {
    let Some(mut rig) = rig() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let attacker = common::parse_sd_fixture(&read(ATTACKER));
    let diff = plant(&mut rig, &attacker, true);
    let planted = rig
        .sv
        .script_log()
        .iter()
        .any(|l| l.starts_with("A;") && l.contains("bomb_plant"));
    diff.finish("plant", PLANT_GAPS);
    assert!(planted, "no `A;…bomb_plant` line in the script log");
}

/// The sweep's stations in order: `(yaw, pitch)` and the `serverTime` span
/// of the traces taken there.
fn stations(fx: &SdFixture) -> Vec<((f32, f32), i32, i32)> {
    let mut out: Vec<((f32, f32), i32, i32)> = Vec::new();
    for t in &fx.phase("sweep").traces {
        let key = (t.yaw_off, t.pitch_off);
        match out.last_mut() {
            Some((k, _, end)) if *k == key => *end = t.server_time,
            _ => out.push((key, t.server_time, t.server_time)),
        }
    }
    out
}

/// Retail's `PROBE fire` clock stamps.
fn retail_fires() -> Vec<i32> {
    read(LOOKAT)
        .lines()
        .filter_map(|l| {
            l.strip_prefix("PROBE fire ")?
                .split_whitespace()
                .nth(1)?
                .parse()
                .ok()
        })
        .collect()
}

/// A station fired when more than two frames did: the first two of a station
/// carry the previous one's fires, still arriving while the new view walks
/// to the server (object-model doc, 23.1).
const CARRYOVER: usize = 2;

#[test]
fn the_defuse_and_the_lookat_match_retail_on_mp_carentan() {
    let Some(mut rig) = rig() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let attacker = common::parse_sd_fixture(&read(ATTACKER));
    let defender = common::parse_sd_fixture(&read(DEFENDER));
    plant(&mut rig, &attacker, false);
    assert!(
        rig.sv
            .script_log()
            .iter()
            .any(|l| l.starts_with("A;") && l.contains("bomb_plant")),
        "the plant never landed, so there is nothing to defuse"
    );
    // The planter out of the way, as the gsc probe's teleport did.
    rig.place(0, PLANTER_PARK, [0.0, 0.0, 0.0]);

    let (sweep, defuse) = (defender.phase("sweep"), defender.phase("defuse"));
    let end = defuse.traces.last().unwrap().server_time;
    let cmds = cmds_in(
        &defender,
        sweep.traces[0].server_time - FRAME_MS as i32,
        end,
    );
    let traces: Vec<&SdTrace> = sweep.traces.iter().collect();
    let delta = calibrate(&cmds, &traces);
    rig.place(1, defender.station.0, defender.station.1);
    let ours = replay(&mut rig, 1, &cmds, end, delta);
    let t0 = window_base(&cmds);
    assert_eq!(
        rig.sv.script_aborts(),
        Vec::<String>::new(),
        "script aborts during the defuse"
    );

    let mut diff = Diff {
        rows: Vec::new(),
        cs: retail_cs(),
    };
    diff_phase(&mut diff, &rig, &defender, "sweep", &ours, t0, false);
    diff_phase(&mut diff, &rig, &defender, "defuse", &ours, t0, false);

    // The lookat: fires per station, retail's off the server log by clock,
    // ours off the frames replayed inside the same station's span.
    let retail = retail_fires();
    let mut rows = Vec::new();
    for ((yaw, pitch), from, to) in stations(&defender) {
        let r = retail.iter().filter(|t| (from..=to).contains(t)).count();
        let o: usize = ours
            .iter()
            .filter(|s| (from..=to).contains(&(t0 + s.ms as i32)))
            .map(|s| s.fires)
            .sum();
        let (rf, of) = (r > CARRYOVER, o > CARRYOVER);
        let line = format!(
            "station yaw={yaw:+.0} pitch={pitch:+.0}: retail fired on {r} frames, ours {o}"
        );
        if report() {
            println!("  {line}");
        }
        if rf != of && !LOOKAT_GAPS.contains(&(yaw, pitch)) {
            rows.push(line);
        }
    }
    if report() {
        println!("lookat: {} stations differ", rows.len());
    }

    let defused = rig
        .sv
        .script_log()
        .iter()
        .any(|l| l.starts_with("A;") && l.contains("bomb_defuse"));
    let last = ours.last().unwrap();
    diff.finish("defuse", DEFUSE_GAPS);
    assert!(
        rows.is_empty(),
        "lookat: {} stations differ from retail\n{}",
        rows.len(),
        rows.join("\n")
    );
    assert!(defused, "no `A;…bomb_defuse` line in the script log");
    assert_eq!(
        last.ps.arrays.objectives[0].state, 0,
        "slot 0 still shown after the defuse"
    );
}

/// Where `getPlant` put the retail capture's charge, on the flak88 beside
/// bombzone_A (object-model doc, 23.6).
const CHARGE: [f32; 3] = [-176.8, 2473.1, -22.956871];
/// Two of `client-probes/probe_bomb.gsc`'s stations round that charge: in the
/// open 26 units off, and across the flak. Retail's fuse blast damaged
/// neither (combat doc, 14.4).
const OPEN_STATION: [f32; 3] = [-196.0, 2455.0, -22.0];
const FLAK_STATION: [f32; 3] = [-112.0, 2446.0, -22.0];
const EV_PLAY_FX: i32 = 191;
const EV_OBITUARY: i32 = 201;
const ET_EVENTS: i32 = 12;

/// The charge left alone: when the fuse runs out, `bomb_countdown`'s
/// `playfx` reaches the wire as the bomb effect's event, and its
/// `radiusDamage` hurts neither player, since the flak88 script models the
/// charge sits on stop every `CanDamage` trace.
#[test]
fn the_charge_blows_on_the_wire_and_its_flak_shields_the_blast() {
    let Some(mut rig) = rig() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let attacker = common::parse_sd_fixture(&read(ATTACKER));
    plant(&mut rig, &attacker, false);
    rig.place(0, OPEN_STATION, [0.0; 3]);
    rig.place(1, FLAK_STATION, [0.0; 3]);
    let p = &PROTOCOL_V1;
    let bomb_fx = (1..64)
        .find(|i| rig.sv.configstring(780 + i) == "fx/explosions/mp_bomb.efx")
        .expect("sd.gsc loads the bomb effect") as i32;
    let charge = rig
        .ca
        .snapshots()
        .newest()
        .unwrap()
        .entities
        .values()
        .any(|e| dist(e.origin(p), CHARGE) < 0.01);
    assert!(charge, "the charge is not where the retail plant put it");

    let (mut fx_seen, mut obituaries, mut damage) = ([false; 2], 0, Vec::new());
    for _ in 0..1300 {
        rig.idle();
        for (n, cl) in [&rig.ca, &rig.cb].into_iter().enumerate() {
            for e in cl.snapshots().newest().unwrap().entities.values() {
                let ev = e.field_i32(p, "eType") - ET_EVENTS;
                if ev == EV_PLAY_FX && e.field_i32(p, "eventParm") == bomb_fx {
                    // `G_TempEntity` truncates the origin.
                    assert_eq!(e.origin(p), CHARGE.map(f32::trunc));
                    fx_seen[n] = true;
                }
                obituaries += usize::from(ev == EV_OBITUARY);
            }
        }
        let log = rig.sv.script_log();
        damage.extend(
            log[rig.drained.min(log.len())..]
                .iter()
                .filter(|l| l.contains("MOD_EXPLOSIVE"))
                .cloned(),
        );
        rig.drained = log.len();
        if fx_seen.iter().any(|s| *s) {
            break;
        }
    }
    assert_eq!(
        fx_seen,
        [true, true],
        "the bomb effect's event on each client"
    );
    assert_eq!(obituaries, 0, "the blast killed someone");
    assert_eq!(damage, Vec::<String>::new(), "the blast damaged someone");
}
