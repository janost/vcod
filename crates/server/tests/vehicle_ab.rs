//! A United Offensive vehicle on ours against retail's, cmd for cmd. The
//! fixtures are `--probe-vehicle seats|drive --save-slope` captures of
//! retail UO 1.51b (`client-probes/probe_vehicle`, the header names the
//! recipe): every usercmd the probe sent from its first seated snapshot on,
//! and every snapshot's playerstate and, on the drive captures, the
//! vehicle's whole entity state (`veh=`). The replay boards the same
//! vehicle on ours through the same probe gametype and the use key, then
//! sends the capture's cmds verbatim, rebased onto our clock and, once a
//! seat, onto our `delta_angles`, and diffs each snapshot (docs/research/coduo-vehicles.md, "The server").
//!
//! `VEHICLE_REPORT=1` prints every snapshot's differences. Needs `COD_DIR`
//! at a UO install; without one it returns early.

use crate::common;

use common::{ClientEnd, Join, Queues, step};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetClient;
use vcod_common::net::NetEvent;
use vcod_common::net::event_ids::{EV_BULLET_HIT_LARGE, EV_BULLET_HIT_SMALL, EV_UO_BULLET_HIT};
use vcod_common::net::msg::{BUTTON_USE, EntityState, PlayerState, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V22;

/// From the snapshot a seat first shows on to the capture's first: the
/// retail logs' `activated` times against the captures' first snapshots
/// (tank 15150 and 16050, jeep 15550 and 16450).
const CAPTURE_LAG_MS: i32 = 900;

/// UO's `ET_SOUND_BLEND`.
const ET_SOUND_BLEND: i32 = 9;

/// One `!cmd` or `!snap` line.
enum Line {
    Cmd(UserCmd),
    Snap(Snap),
}

impl Snap {
    fn delta_angles(&self) -> [i32; 3] {
        ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"]
            .map(|n| self.wire.get(n).copied().unwrap_or(0))
    }
}

#[derive(Clone, Debug)]
struct Snap {
    t: i32,
    /// `commandTime`: the last cmd the frame had run.
    ct: i32,
    /// Every non-zero playerstate netfield as decoded.
    wire: BTreeMap<String, i32>,
    /// The seat's vehicle's every non-zero entity field (drive captures).
    veh: Option<BTreeMap<String, i32>>,
    /// Every bullet-impact temp entity on the snapshot, `(number, at)`.
    imp: Vec<(u32, [i32; 3])>,
}

fn kv(s: &str) -> BTreeMap<String, i32> {
    s.split(',')
        .filter_map(|p| p.split_once(':'))
        .map(|(k, v)| (k.to_string(), v.parse().unwrap()))
        .collect()
}

fn parse(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((kind, rest)) = line.split_once(' ') else {
            continue;
        };
        let field = |name: &str| {
            rest.split(' ')
                .find_map(|t| t.strip_prefix(&format!("{name}=")))
                .unwrap_or("")
        };
        match kind {
            "!cmd" => {
                let n = |name: &str| field(name).parse::<i32>().unwrap();
                let a: Vec<i32> = field("angles")
                    .split(',')
                    .map(|x| x.parse().unwrap())
                    .collect();
                out.push(Line::Cmd(UserCmd {
                    server_time: n("st"),
                    buttons: n("buttons") as u8,
                    wbuttons: n("wbuttons") as u8,
                    weapon: n("weapon") as u8,
                    up: n("up") as i8,
                    forward: n("forward") as i8,
                    right: n("right") as i8,
                    angles: [a[0], a[1], a[2]],
                    ..UserCmd::default()
                }));
            }
            "!snap" => {
                let veh = field("veh");
                out.push(Line::Snap(Snap {
                    t: field("t").parse().unwrap(),
                    ct: field("ct").parse().unwrap(),
                    wire: kv(field("wire")),
                    veh: (!veh.is_empty()).then(|| kv(veh)),
                    imp: field("imp")
                        .split(';')
                        .filter_map(|e| e.split_once(':'))
                        .map(|(n, at)| {
                            let at: Vec<f32> = at.split(',').map(|c| c.parse().unwrap()).collect();
                            (n.parse().unwrap(), std::array::from_fn(|i| at[i] as i32))
                        })
                        .collect(),
                }));
            }
            _ => {}
        }
    }
    out
}

/// Ours, one sample per retail snapshot.
struct Sample {
    ps: PlayerState,
    veh: Option<EntityState>,
    /// The sound-blend entities on the wire: number, both aliases.
    blends: Vec<(u32, i32, i32)>,
    /// The bullet impacts on the wire, `(number, at)`.
    impacts: Vec<(u32, [i32; 3])>,
}

/// A cmd that holds the view where the newest snapshot has it, stamped at
/// that snapshot's time so the capture's first cmd is never stale behind it.
fn holding(cl: &NetClient<ClientEnd>, buttons: u8) -> UserCmd {
    let p = &PROTOCOL_V22;
    let Some(s) = cl.snapshots().newest() else {
        return UserCmd::default();
    };
    let v = s.ps.viewangles(p);
    let da =
        ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"].map(|n| s.ps.field_i32(p, n));
    UserCmd {
        server_time: s.server_time,
        angles: std::array::from_fn(|i| ((v[i] * 65536.0 / 360.0).round() as i32 - da[i]) & 0xffff),
        buttons,
        weapon: s.ps.field_i32(p, "weapon") as u8,
        ..UserCmd::default()
    }
}

fn seated(cl: &NetClient<ClientEnd>) -> bool {
    cl.snapshots().newest().is_some_and(|s| {
        s.ps.field_i32(&PROTOCOL_V22, "eFlags") & vcod_common::net::flags::EF_IN_VEHICLE_V22 != 0
    })
}

/// Our replay of `lines` on `map`, boarding a `vehicletype` beside it.
fn ours(
    map: &str,
    vehicletype: &str,
    cvars: &[(&str, &str)],
    lines: &[Line],
) -> Option<Vec<Sample>> {
    Some(replay(map, vehicletype, cvars, lines)?.samples)
}

/// What a replay left: a sample per capture snapshot, the gsc's `PROBE`
/// lines, and how far our clock runs ahead of the capture's.
struct Replay {
    samples: Vec<Sample>,
    log: Vec<String>,
    offset: i32,
}

/// A client in a seat on ours: the server, its queues, the client, the
/// clock, the join and the last cmd's `serverTime`.
struct Boarded {
    sv: vcod_server::Server,
    q: Rc<RefCell<Queues>>,
    cl: NetClient<ClientEnd>,
    now: Instant,
    join: Join,
    last_sent: i32,
}

impl Boarded {
    /// One frame on `cmd`, its clock kept past the last one's (a cmd's
    /// clock rides as a delta on the last one's, so it must not go back).
    fn frame(&mut self, mut cmd: UserCmd) {
        self.now += Duration::from_millis(common::FRAME_MS as u64);
        cmd.server_time = cmd.server_time.max(self.last_sent + 1);
        self.last_sent = cmd.server_time;
        self.cl.send_cmds(&[cmd]);
        for e in step(&mut self.sv, &self.q, &mut self.cl, self.now) {
            if let NetEvent::ServerCommand(tokens) = e {
                self.join.on_server_command(&tokens, &mut self.cl, self.now);
            }
        }
    }
}

/// `client-probes/probe_vehicle` on `map` with `cvars`, one axis client
/// joined, placed beside the `vehicletype` and in its seat by the use key.
fn board(map: &str, vehicletype: &str, cvars: &[(&str, &str)]) -> Option<Boarded> {
    board_with(map, vehicletype, cvars, &Setup::default(), |_| {})
}

/// Each map's first spawn when a [`Setup`] pins none: a flat
/// `mp_teamdeathmatch_spawn`, whose think starts the idle on the spawn
/// frame, where the seeded draw's sloped point slides and sets a walk
/// first. Every mp_cassino capture's `legsAnim` toggle reads the flat
/// spawn's (coduo-vehicles.md 13, "The driver's idle"); on mp_peaks the
/// seeded draw already reads retail's, `JEEP_GUN`'s aside ([`JEEP_GUN_PIN`]).
const FLAT_SPAWNS: &[(&str, Spot)] = &[("mp_cassino", ([64.0, 152.0, 0.125], [0.0, 270.0, 0.0]))];

/// A spawn pin's origin and angles.
type Spot = ([f32; 3], [f32; 3]);

/// What a replay changes on the server before the load: the seed, for a
/// death clip's random draw, and the spawn picks, for a respawn's (none:
/// the map's [`FLAT_SPAWNS`] entry).
#[derive(Default)]
struct Setup {
    seed: Option<u64>,
    pins: Vec<Vec<vcod_server::game::host::SpawnPin>>,
}

/// [`board`] with the paks changed first and `setup`'s seed and pins
/// (`VEHICLE_SEED=<n>` overrides the seed).
fn board_with(
    map: &str,
    vehicletype: &str,
    cvars: &[(&str, &str)],
    setup: &Setup,
    edit: impl FnOnce(&mut vcod_common::pk3::Pk3Fs),
) -> Option<Boarded> {
    let mut fs = vcod_common::testing::uo_fs()?;
    edit(&mut fs);
    let p = &PROTOCOL_V22;
    let bsp_path = fs.resolve_map(map).expect("map in the UO paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let seed = std::env::var("VEHICLE_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .or(setup.seed);
    let cfg = common::cfg(map, "probe_vehicle");
    let mut sv = match seed {
        Some(seed) => vcod_server::Server::with_seed(cfg, now, seed),
        None => vcod_server::Server::new(cfg, now),
    };
    sv.set_cvar("sv_pure", "0");
    let pins = match (
        setup.pins.is_empty(),
        FLAT_SPAWNS.iter().find(|(m, _)| *m == map),
    ) {
        (true, Some((_, pin))) => vec![vec![Some(*pin)]],
        _ => setup.pins.clone(),
    };
    sv.pin_spawns(pins);
    sv.set_protocol(p);
    sv.overlay_script(
        "maps/mp/gametypes/probe_vehicle",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_vehicle.gsc"),
    );
    sv.set_cvar("probe_vehicletype", vehicletype);
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut join = Join::new("axis", "kar98k_mp");
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let mut placed_at = None;
    // A cmd's clock rides as a delta on the last one's, so it must not go
    // back.
    let mut last_sent = 0;
    // Join, wait for the gsc's placement, tap use for 150 ms from 2050 ms
    // after it, and wait for the seat. The probe settles 2 s from the
    // placed snapshot and its use lands a frame later: every retail
    // capture's `PROBE activated` is 2050 past its `PROBE place`
    // (coduo-vehicles.md 13, "The driver's idle").
    for i in 0..2000i64 {
        now += frame;
        let noboard = std::env::var("VEHICLE_NOBOARD").is_ok();
        if noboard && placed_at.is_some_and(|at| (i - at) * common::FRAME_MS >= 3000) {
            break;
        }
        let tap = !noboard
            && placed_at.is_some_and(|at| {
                (i - at) * common::FRAME_MS >= 2050 && (i - at) * common::FRAME_MS < 2200
            });
        if cl.snapshots().newest().is_some() {
            let mut cmd = holding(&cl, if tap { BUTTON_USE } else { 0 });
            cmd.server_time = cmd.server_time.max(last_sent + 1);
            last_sent = cmd.server_time;
            cl.send_cmds(&[cmd]);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            last_sent = c.server_time;
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if placed_at.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE place")) {
            placed_at = Some(i);
        }
        if std::env::var("VEHICLE_CMDS").is_ok()
            && let Some(n) = cl.snapshots().newest()
        {
            eprintln!(
                "board t {} ct {} sent {last_sent}",
                n.server_time,
                n.ps.field_i32(&PROTOCOL_V22, "commandTime")
            );
        }
        if seated(&cl) {
            break;
        }
    }
    if std::env::var("VEHICLE_REPORT").is_ok() {
        for l in sv.script_log().iter().filter(|l| l.contains("PROBE")) {
            eprintln!("{l}");
        }
    }
    assert!(seated(&cl), "never got into the {vehicletype}");
    Some(Boarded {
        sv,
        q,
        cl,
        now,
        join,
        last_sent,
    })
}

fn replay(map: &str, vehicletype: &str, cvars: &[(&str, &str)], lines: &[Line]) -> Option<Replay> {
    replay_with(map, vehicletype, cvars, lines, &Setup::default(), |_| {})
}

/// [`replay`] with the paks changed first.
fn replay_with(
    map: &str,
    vehicletype: &str,
    cvars: &[(&str, &str)],
    lines: &[Line],
    setup: &Setup,
    edit: impl FnOnce(&mut vcod_common::pk3::Pk3Fs),
) -> Option<Replay> {
    let Boarded {
        mut sv,
        q,
        mut cl,
        mut now,
        mut join,
        mut last_sent,
    } = board_with(map, vehicletype, cvars, setup, edit)?;
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let first = lines.iter().find_map(|l| match l {
        Line::Snap(s) => Some(s.t),
        _ => None,
    })?;
    // The probe's capture opens 900 ms after the seat (every capture so
    // far: its first message is the 64th), the probe holding the view it
    // boarded with, which its first cmd still carries.
    let first_cmd = lines.iter().find_map(|l| match l {
        Line::Cmd(c) => Some(*c),
        _ => None,
    })?;
    let first_da = lines.iter().find_map(|l| match l {
        Line::Snap(s) => Some(s.delta_angles()),
        _ => None,
    })?;
    let seated_at = cl.snapshots().newest()?.server_time;
    let mut ours_first = seated_at;
    while ours_first < seated_at + CAPTURE_LAG_MS {
        now += frame;
        let mut cmd = rebased(first_cmd, first_da, &cl);
        cmd.server_time = ours_first.max(last_sent + 1);
        last_sent = cmd.server_time;
        cl.send_cmds(&[cmd]);
        step(&mut sv, &q, &mut cl, now);
        ours_first = cl.snapshots().newest()?.server_time;
    }
    let offset = ours_first - first;
    // One rebase a seat: retail's `delta_angles` onto ours at the capture's
    // first snapshot, and again at the first snapshot both show a changed
    // seat on. In between each cmd goes out as the probe sent it, so a
    // clamp push moves ours as it moved retail's, cmd for cmd.
    let mut base = rebase_from(first_da, &cl);
    let mut reanchor = false;
    let mut out = Vec::new();
    // Each cmd with the `delta_angles` of the last retail snapshot ahead of
    // it, which the probe built it against.
    let mut pending: Vec<(UserCmd, [i32; 3])> = Vec::new();
    let mut da = first_da;
    let mut last_seat = 0;
    let mut last_eflags = 0;
    let mut started = false;
    for line in lines {
        match line {
            Line::Cmd(c) => {
                if started {
                    let mut c = *c;
                    c.server_time += offset;
                    pending.push((c, da));
                }
            }
            Line::Snap(s) => {
                da = s.delta_angles();
                let seat = s.wire.get("vehPos").copied().unwrap_or(0);
                if !started {
                    started = true;
                    last_seat = seat;
                    last_eflags = s.wire.get("eFlags").copied().unwrap_or(0);
                    out.push(sample(&cl));
                    continue;
                }
                // The probe's `nextvehslot` rides no cmd: a seat change in
                // the capture is where it went.
                let mut change = seat != 0 && last_seat != 0 && seat != last_seat;
                reanchor |= change;
                last_seat = seat;
                let want = s.t + offset;
                // The cmds retail's frame had run by this snapshot. A spawn
                // (`eFlags` 0x8 flips) thinks at the frame's own time, so a
                // `commandTime` there is no cmd's: the frame ran those before.
                let eflags = s.wire.get("eFlags").copied().unwrap_or(0);
                let spawned = std::mem::replace(&mut last_eflags, eflags) & 8 != eflags & 8;
                let ran = if spawned && s.ct == s.t {
                    s.t - 1
                } else {
                    s.ct
                } + offset;
                for _ in 0..40 {
                    if cl
                        .snapshots()
                        .newest()
                        .is_some_and(|n| n.server_time >= want)
                    {
                        break;
                    }
                    now += frame;
                    let due: Vec<UserCmd> = pending
                        .iter()
                        .filter(|(c, _)| c.server_time <= ran)
                        .map(|(c, _)| shifted(*c, base))
                        .collect();
                    pending.retain(|(c, _)| c.server_time > ran);
                    if std::mem::take(&mut change) {
                        // The probe says `nextvehslot` as a step starts, in
                        // the packet with the step's first cmd, and a `say`
                        // step holds the view the last one ended on: the
                        // change runs after the frame's cmds up to that held
                        // run, and `SetClientViewAngle` bases the seat's
                        // `delta_angles` on the last of them.
                        let held = due
                            .iter()
                            .rev()
                            .take_while(|c| due.last().is_some_and(|l| l.angles == c.angles))
                            .count();
                        let (before, after) = due.split_at(due.len() - held);
                        if !before.is_empty() {
                            cl.send_cmds(before);
                        }
                        cl.send_reliable("nextvehslot");
                        cl.send_cmds(after);
                    } else {
                        cl.send_cmds(&due);
                    }
                    for e in step(&mut sv, &q, &mut cl, now) {
                        if let NetEvent::ServerCommand(tokens) = e {
                            join.on_server_command(&tokens, &mut cl, now);
                        }
                    }
                }
                let ours_seat = cl
                    .snapshots()
                    .newest()
                    .map_or(0, |n| n.ps.field_i32(&PROTOCOL_V22, "vehPos"));
                if reanchor && ours_seat == seat {
                    base = rebase_from(da, &cl);
                    reanchor = false;
                }
                out.push(sample(&cl));
            }
        }
    }
    let log = sv
        .script_log()
        .iter()
        .filter(|l| l.contains("PROBE"))
        .cloned()
        .collect();
    Some(Replay {
        samples: out,
        log,
        offset,
    })
}

/// What turns a capture cmd's angles into ours: our `delta_angles` less
/// retail's, both read at the capture's first snapshot.
fn rebase_from(retail_da: [i32; 3], cl: &NetClient<ClientEnd>) -> [i32; 3] {
    let p = &PROTOCOL_V22;
    let ours = cl.snapshots().newest().map_or([0; 3], |s| {
        ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"].map(|n| s.ps.field_i32(p, n))
    });
    std::array::from_fn(|i| retail_da[i] - ours[i])
}

fn shifted(mut c: UserCmd, base: [i32; 3]) -> UserCmd {
    for (a, b) in c.angles.iter_mut().zip(base) {
        *a = (*a + b) & 0xffff;
    }
    c
}

/// A capture cmd as the probe would have sent it to ours: it holds an
/// absolute view, rebased on the newest `delta_angles` it saw, which on
/// ours differ from retail's by whatever the two clients' cmds were when
/// a seat's `SetClientViewAngle` ran.
fn rebased(mut c: UserCmd, retail_da: [i32; 3], cl: &NetClient<ClientEnd>) -> UserCmd {
    let p = &PROTOCOL_V22;
    let ours = cl.snapshots().newest().map_or([0; 3], |s| {
        ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"].map(|n| s.ps.field_i32(p, n))
    });
    for i in 0..3 {
        c.angles[i] = (c.angles[i] + retail_da[i] - ours[i]) & 0xffff;
    }
    c
}

fn sample(cl: &NetClient<ClientEnd>) -> Sample {
    let p = &PROTOCOL_V22;
    let s = cl.snapshots().newest().unwrap();
    let veh_num = s.ps.field_i32(p, "viewlocked_entNum") as u32;
    let blends = s
        .entities
        .iter()
        .filter(|(_, e)| e.field_i32(p, "eType") == ET_SOUND_BLEND)
        .map(|(n, e)| {
            (
                *n,
                e.field_i32(p, "eventParm"),
                e.field_i32(p, "_union.scale"),
            )
        })
        .collect();
    let impacts = s
        .entities
        .iter()
        .filter(|(_, e)| {
            p.event_of(e.field_i32(p, "eType")).is_some_and(|ev| {
                [EV_BULLET_HIT_SMALL, EV_BULLET_HIT_LARGE, EV_UO_BULLET_HIT].contains(&ev)
            })
        })
        .map(|(&n, e)| {
            let at = vcod_common::net::trajectory::Trajectory::read(e, p, "pos").base;
            (n, at.to_array().map(|c| c as i32))
        })
        .collect();
    Sample {
        ps: s.ps.clone(),
        veh: s.entities.get(&veh_num).cloned(),
        blends,
        impacts,
    }
}

fn f(v: i32) -> f32 {
    f32::from_bits(v as u32)
}

/// Per field, the snapshots that differ: a float to `tol`, an angle
/// modulo 360, an int exactly.
#[derive(Default)]
struct Tally {
    seen: BTreeMap<String, (usize, usize)>,
    report: Vec<String>,
    /// Every miss, by snapshot and field.
    missed: Vec<(usize, String)>,
}

impl Tally {
    fn check(&mut self, k: usize, name: &str, retail: Option<i32>, ours: i32, kind: Kind) {
        let r = retail.unwrap_or(0);
        let same = match kind {
            Kind::Int => r == ours,
            Kind::Mask(m) => r & m == ours & m,
            Kind::Float(tol) => (f(r) - f(ours)).abs() <= tol,
            Kind::Angle(tol) => {
                let d = (f(r) - f(ours)).rem_euclid(360.0);
                d.min(360.0 - d) <= tol
            }
        };
        let e = self.seen.entry(name.to_string()).or_default();
        e.0 += 1;
        if !same {
            e.1 += 1;
            self.missed.push((k, name.to_string()));
            let show = |v: i32| match kind {
                Kind::Float(_) | Kind::Angle(_) => format!("{}", f(v)),
                _ => format!("{v}"),
            };
            self.report.push(format!(
                "snap {k} {name}: retail {} ours {}",
                show(r),
                show(ours)
            ));
        }
    }

    /// Every miss is one a gap names, and every gap still misses.
    fn assert_within(&self, gaps: &[Gap]) {
        let stray: Vec<String> = self
            .missed
            .iter()
            .filter(|(k, n)| !gaps.iter().any(|g| g.covers(*k, n)))
            .map(|(k, n)| format!("snap {k} {n}"))
            .collect();
        assert!(
            stray.is_empty(),
            "{} miss(es) outside the gaps (VEHICLE_REPORT=1 shows the values)\n{}",
            stray.len(),
            stray.join("\n")
        );
        for g in gaps {
            assert!(
                self.missed.iter().any(|(k, n)| g.covers(*k, n)),
                "{} {:?} now matches retail; drop the gap ({})",
                g.field,
                g.snaps,
                g.why
            );
        }
    }
}

/// Snapshots where a field knowingly differs, with the reason.
struct Gap {
    field: &'static str,
    snaps: std::ops::RangeInclusive<usize>,
    why: &'static str,
}

impl Gap {
    fn covers(&self, k: usize, name: &str) -> bool {
        self.snaps.contains(&k) && name.starts_with(self.field)
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Int,
    Mask(i32),
    Float(f32),
    Angle(f32),
}

const PS_FIELDS: &[(&str, Kind)] = &[
    ("pm_type", Kind::Int),
    ("eFlags", Kind::Mask(!0x10)),
    ("pm_flags", Kind::Int),
    ("vehPos", Kind::Int),
    ("vehType", Kind::Int),
    ("vehMotion", Kind::Int),
    ("viewlocked_entNum", Kind::Int),
    ("viewangles[0]", Kind::Angle(0.01)),
    ("viewangles[1]", Kind::Angle(0.01)),
    ("origin[0]", Kind::Float(0.01)),
    ("origin[1]", Kind::Float(0.01)),
    ("origin[2]", Kind::Float(0.01)),
    ("fWeaponPosFrac", Kind::Float(0.001)),
    ("legsAnim", Kind::Int),
    ("torsoAnim", Kind::Int),
    ("weapon", Kind::Int),
    ("aimSpreadScale", Kind::Float(0.001)),
    ("weaponstate", Kind::Int),
    ("weaponTime", Kind::Int),
    ("weaponDelay", Kind::Int),
    ("eventSequence", Kind::Int),
    ("events[0]", Kind::Int),
    ("events[1]", Kind::Int),
    ("events[2]", Kind::Int),
    ("events[3]", Kind::Int),
    ("eventParms[0]", Kind::Int),
    ("eventParms[1]", Kind::Int),
    ("eventParms[2]", Kind::Int),
    ("eventParms[3]", Kind::Int),
    ("damageEvent", Kind::Int),
    ("damageCount", Kind::Int),
    ("velocity[0]", Kind::Float(0.0)),
    ("velocity[1]", Kind::Float(0.0)),
    ("velocity[2]", Kind::Float(0.0)),
];

const VEH_FIELDS: &[(&str, Kind)] = &[
    ("pos.trBase[0]", Kind::Float(0.01)),
    ("pos.trBase[1]", Kind::Float(0.01)),
    ("pos.trBase[2]", Kind::Float(0.01)),
    ("apos.trBase[0]", Kind::Angle(0.01)),
    ("apos.trBase[1]", Kind::Angle(0.01)),
    ("apos.trBase[2]", Kind::Angle(0.01)),
    ("angles2[0]", Kind::Angle(0.01)),
    ("angles2[1]", Kind::Angle(0.01)),
    ("angles2[2]", Kind::Angle(0.01)),
    ("origin2[0]", Kind::Int),
    ("origin2[1]", Kind::Angle(0.01)),
    ("origin2[2]", Kind::Int),
    ("leanf", Kind::Float(0.01)),
    ("time", Kind::Int),
    ("time2", Kind::Int),
    ("animMovetype", Kind::Int),
    ("otherEntityNum", Kind::Int),
    ("eFlags", Kind::Int),
    ("weapon", Kind::Int),
    ("index", Kind::Int),
    ("solid", Kind::Int),
    ("xmodel", Kind::Int),
    ("surfType", Kind::Int),
    ("attackerEntityNum", Kind::Int),
    ("fTorsoPitch", Kind::Float(0.0)),
    ("fWaistPitch", Kind::Float(0.0)),
    ("loopSound", Kind::Int),
    ("eventSequence", Kind::Int),
    ("events[0]", Kind::Int),
    ("events[1]", Kind::Int),
    ("events[2]", Kind::Int),
    ("events[3]", Kind::Int),
    ("eventParms[0]", Kind::Int),
    ("eventParms[1]", Kind::Int),
    ("eventParms[2]", Kind::Int),
    ("eventParms[3]", Kind::Int),
];

fn tally(lines: &[Line], ours: &[Sample]) -> Tally {
    let p = &PROTOCOL_V22;
    let snaps: Vec<&Snap> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Snap(s) => Some(s),
            _ => None,
        })
        .collect();
    let mut t = Tally::default();
    for (k, (r, o)) in snaps.iter().zip(ours).enumerate() {
        for (name, kind) in PS_FIELDS {
            let i = PlayerState::field_index(p, name).unwrap();
            t.check(k, name, r.wire.get(*name).copied(), o.ps.fields[i], *kind);
        }
        if let (Some(rv), Some(ov)) = (&r.veh, &o.veh) {
            for (name, kind) in VEH_FIELDS {
                let i = EntityState::field_index(p, name).unwrap();
                let mut ours = ov.fields[i];
                // The wire keeps `time` to 16 bits and `time2` to 6.
                if *name == "time" {
                    ours &= 0xffff;
                }
                t.check(
                    k,
                    &format!("veh {name}"),
                    rv.get(*name).copied(),
                    ours,
                    *kind,
                );
            }
        }
    }
    if std::env::var("VEHICLE_REPORT").is_ok() {
        for l in &t.report {
            eprintln!("{l}");
        }
    }
    for (name, (n, miss)) in &t.seen {
        eprintln!("{name}: {miss} of {n} differ");
    }
    t
}

const TANK_SEATS: &str = "tests/fixtures/playerstate/mp_cassino-tdm-slope-8ms-tank-seats-uo.txt";

#[test]
fn the_tank_seats_replay_on_ours() {
    let text = std::fs::read_to_string(TANK_SEATS).unwrap();
    let lines = parse(&text);
    let Some(ours) = ours("mp_cassino", "panzeriv_mp", &[], &lines) else {
        return;
    };
    tally(&lines, &ours).assert_within(&[]);
}

const JEEP_SEATS: &str = "tests/fixtures/playerstate/mp_peaks-tdm-slope-8ms-jeep-seats-uo.txt";

#[test]
fn the_jeep_seats_replay_on_ours() {
    let text = std::fs::read_to_string(JEEP_SEATS).unwrap();
    let lines = parse(&text);
    let cvars = [("probe_side", "75"), ("probe_pitch", "15")];
    let Some(ours) = ours("mp_peaks", "willyjeep_mp", &cvars, &lines) else {
        return;
    };
    tally(&lines, &ours).assert_within(&[]);
}

const TANK_DRIVE: &str = "tests/fixtures/playerstate/mp_cassino-tdm-slope-8ms-tank-drive-uo.txt";
const JEEP_DRIVE: &str = "tests/fixtures/playerstate/mp_peaks-tdm-slope-8ms-jeep-drive-uo.txt";

#[test]
fn the_tank_drive_replays_on_ours() {
    let text = std::fs::read_to_string(TANK_DRIVE).unwrap();
    let lines = parse(&text);
    let Some(ours) = ours("mp_cassino", "panzeriv_mp", &[], &lines) else {
        return;
    };
    tally(&lines, &ours).assert_within(&[]);
    // The engine pairs ride the first two blends while the driver runs it:
    // retail's crew capture has 187 on aliases 17/18 (idle) and 188 on
    // 19/20 (engine), the Panzer IV's four in the vehicle files' order.
    let driven = &ours[100];
    assert!(
        driven.blends.contains(&(187, 17, 18)),
        "{:?}",
        driven.blends
    );
    assert!(
        driven.blends.contains(&(188, 19, 20)),
        "{:?}",
        driven.blends
    );
}

#[test]
fn the_jeep_drive_replays_on_ours() {
    let text = std::fs::read_to_string(JEEP_DRIVE).unwrap();
    let lines = parse(&text);
    let cvars = [("probe_side", "75"), ("probe_pitch", "15")];
    let Some(ours) = ours("mp_peaks", "willyjeep_mp", &cvars, &lines) else {
        return;
    };
    tally(&lines, &ours).assert_within(&[]);
}

const JEEP_DIE: &str = "tests/fixtures/playerstate/mp_peaks-tdm-slope-8ms-jeep-die-uo.txt";

/// The jeep's driver killed in his seat 3.5 s into retail's drive
/// (`probe_suicide`): out of the seat on the death's frame, `pm_type` 6
/// with the view's pitch dropped, thrown off the coasting hull a cmd at a
/// time, then respawned with the seat's `vehType` cleared and placed
/// beside the hull, where the body slides down the slope
/// (docs/research/coduo-vehicles.md 11.7).
#[test]
fn the_jeep_driver_killed_in_his_seat_replays_on_ours() {
    let text = std::fs::read_to_string(JEEP_DIE).unwrap();
    let lines = parse(&text);
    let cvars = [
        ("probe_side", "75"),
        ("probe_pitch", "15"),
        ("probe_suicide", "3.5"),
    ];
    let setup = Setup {
        // Seed 2 draws retail's death clip, `legsAnim` 20.
        seed: Some(2),
        // The first spawn the gsc moves at once; the respawn is retail's.
        pins: vec![vec![
            None,
            Some(([-832.0, -504.0, 339.125], [0.0, -90.0, 0.0])),
        ]],
    };
    let Some(ours) = replay_with("mp_peaks", "willyjeep_mp", &cvars, &lines, &setup, |_| {}) else {
        return;
    };
    tally(&lines, &ours.samples).assert_within(&[]);
}

const JEEP_BONES: &str = "tests/fixtures/playerstate/mp_peaks-tdm-slope-8ms-jeep-bones-uo.txt";
const JEEP_BONES_LOG: &str = "tests/fixtures/vehicles/mp_peaks-jeep-bones-uo.txt";

/// Each `PROBE bone <time> <client> <seat> <tag> <origin>` line by time
/// and tag.
fn bones(log: &[String], offset: i32) -> BTreeMap<(i32, String), [f32; 3]> {
    let mut out = BTreeMap::new();
    for l in log {
        let Some(rest) = l.trim().strip_prefix("PROBE bone ") else {
            continue;
        };
        let (head, at) = rest.split_once('(').unwrap();
        let t: Vec<&str> = head.split_whitespace().collect();
        let v: Vec<f32> = at
            .trim_end_matches(')')
            .split(", ")
            .map(|x| x.parse().unwrap())
            .collect();
        out.insert(
            (t[0].parse::<i32>().unwrap() - offset, t[3..].join(" ")),
            [v[0], v[1], v[2]],
        );
    }
    out
}

/// The jeep driver's hands, head, chest and foot through retail's drive,
/// the gsc's `getTagOrigin` every frame against ours: the steer's seat
/// blend and its 100 ms ramp, the controls' easing and the rider's frame
/// (docs/research/coduo-vehicles.md 14, "The seat blend's ramp").
#[test]
fn the_jeep_driver_s_bones_follow_retail_s() {
    let text = std::fs::read_to_string(JEEP_BONES).unwrap();
    let lines = parse(&text);
    let cvars = [
        ("probe_side", "75"),
        ("probe_pitch", "15"),
        ("probe_bones", "1"),
    ];
    let Some(ours) = replay("mp_peaks", "willyjeep_mp", &cvars, &lines) else {
        return;
    };
    let retail: Vec<String> = std::fs::read_to_string(JEEP_BONES_LOG)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let (r, o) = (bones(&retail, 0), bones(&ours.log, ours.offset));
    // The links' first frame: retail's marks read the pose of the frame
    // after, ours the frame's own (one sample a bone).
    let first = r.keys().map(|(t, _)| *t).min().unwrap_or(0) + 50;
    let report = std::env::var("VEHICLE_REPORT").is_ok();
    let (mut n, mut sum, mut worst) = (0, 0.0, (0.0f32, 0, String::new()));
    for ((t, tag), a) in r.iter().filter(|((t, _), _)| *t > first) {
        let Some(b) = o.get(&(*t, tag.clone())) else {
            continue;
        };
        let d = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max);
        if report {
            eprintln!("{t} {tag}: retail {a:?} ours {b:?} off {d:.2}");
        }
        n += 1;
        sum += d;
        if d > worst.0 {
            worst = (d, *t, tag.clone());
        }
    }
    let mean = sum / n.max(1) as f32;
    eprintln!("{n} bone samples, mean {mean:.3}, worst {worst:?}");
    assert!(n > 2000, "only {n} samples in common");
    assert!(
        mean <= 0.002,
        "the bones sit {mean} off retail's on average"
    );
    assert!(worst.0 <= BONE_TOLERANCE, "{worst:?} off retail's");
}

/// How far a bone may sit off retail's in one sample: all 2170 read
/// retail's marks to the print's 0.01 but one 0.02 (17550, the hand on the
/// first steer), through the driving and idle nodes' toggles
/// (coduo-vehicles.md 14, "The seat blend's ramp").
const BONE_TOLERANCE: f32 = 0.05;

const JEEP_GUN: &str = "tests/fixtures/playerstate/mp_peaks-tdm-slope-8ms-jeep-gun-uo.txt";

/// The capture opens in the seat, so its spawn is not in evidence. Its
/// `legsAnim` toggle reads one set more than the seeded draw gives (606 to
/// 94): a spawn whose think lands sliding sets a walk before the idle.
/// Retail's [`JEEP_GUN_SPAWN`] run spawned on tdm spawnpoint 277 and reads
/// 606 too. Of the map's 32 tdm spawns pinned, 277 and (-1328, -160) match
/// every field here; the other 30 miss the toggle (coduo-vehicles.md 13,
/// "The driver's idle").
const JEEP_GUN_PIN: Spot = ([-3872.0, 3064.0, -25.528], [0.0, 255.0, 0.0]);

#[test]
fn the_jeep_gun_replays_on_ours() {
    let text = std::fs::read_to_string(JEEP_GUN).unwrap();
    let lines = parse(&text);
    let cvars = [("probe_side", "75"), ("probe_pitch", "15")];
    let setup = Setup {
        pins: vec![vec![Some(JEEP_GUN_PIN)]],
        ..Setup::default()
    };
    let Some(ours) = replay_with("mp_peaks", "willyjeep_mp", &cvars, &lines, &setup, |_| {}) else {
        return;
    };
    tally(&lines, &ours.samples).assert_within(&[]);
}

const JEEP_GUN_SPAWN: &str =
    "tests/fixtures/playerstate/mp_peaks-tdm-slope-8ms-jeep-gun-spawn-uo.txt";

/// `GUN` on the jeep again, logged from the spawn
/// (`--slope-from-spawn`): retail's spawn on tdm spawnpoint 277
/// (`fixtures/vehicles/mp_peaks-jeep-gun-spawn-uo.txt`) lands its think
/// sliding at (-12, 0, -3) with the walk 128 set, so the seat reads 606.
/// Ours on that spawn, every field from where [`replay`] opens a capture
/// (900 ms into the seat) to the end.
#[test]
fn the_jeep_gun_from_a_logged_spawn_replays_on_ours() {
    let text = std::fs::read_to_string(JEEP_GUN_SPAWN).unwrap();
    let lines = from_the_seat(parse(&text));
    let cvars = [("probe_side", "75"), ("probe_pitch", "15")];
    let setup = Setup {
        pins: vec![vec![Some(JEEP_GUN_PIN)]],
        ..Setup::default()
    };
    let Some(ours) = replay_with("mp_peaks", "willyjeep_mp", &cvars, &lines, &setup, |_| {}) else {
        return;
    };
    tally(&lines, &ours.samples).assert_within(&[]);
}

/// A capture logged from the spawn cut where a seated one opens: at the
/// first snapshot [`CAPTURE_LAG_MS`] past the seat, with the cmds sent
/// since the snapshot before it.
fn from_the_seat(lines: Vec<Line>) -> Vec<Line> {
    let seated = lines
        .iter()
        .find_map(|l| match l {
            Line::Snap(s) if s.wire.get("pm_type") == Some(&1) => Some(s.t),
            _ => None,
        })
        .expect("a seated snapshot");
    let first = lines
        .iter()
        .position(|l| matches!(l, Line::Snap(s) if s.t >= seated + CAPTURE_LAG_MS))
        .expect("a snapshot past the lag");
    let from = lines[..first]
        .iter()
        .rposition(|l| matches!(l, Line::Snap(_)))
        .map_or(0, |i| i + 1);
    lines.into_iter().skip(from).collect()
}

const TANK_GUN: &str = "tests/fixtures/playerstate/mp_cassino-tdm-slope-8ms-tank-gun-uo.txt";
const TANK_GUN_BONES: &str = "tests/fixtures/vehicles/mp_cassino-tank-gun-bones-uo.txt";

/// `GUN` on a Panzer IV: the driver's seat by use, `nextvehslot` to the
/// gunner's, the gun swept past its spans on `tag_secondary_player`'s
/// base, then out (coduo-vehicles.md 8). Every field of every snapshot.
#[test]
fn the_tank_gun_replays_on_ours() {
    let text = std::fs::read_to_string(TANK_GUN).unwrap();
    let lines = parse(&text);
    let Some(ours) = ours("mp_cassino", "panzeriv_mp", &[("probe_bones", "1")], &lines) else {
        return;
    };
    tally(&lines, &ours).assert_within(&[]);
}

/// The same run's marks linked to the rider's bones, every frame of both
/// stints. Through the first, a driver boarded by use is linked with no
/// seat offset, so every mark reads 20 above the seat until the
/// `nextvehslot` (coduo-vehicles.md 13, "The tank driver's link offset").
#[test]
fn the_tank_rider_s_bones_follow_retail_s_in_both_seats() {
    let text = std::fs::read_to_string(TANK_GUN).unwrap();
    let lines = parse(&text);
    // Retail's spawn did not slide: its seat's `legsAnim` 697 is the idle
    // with the toggle on, one set since the spawn, as ours on mp_cassino's
    // `FLAT_SPAWNS` point.
    let Some(ours) = replay("mp_cassino", "panzeriv_mp", &[("probe_bones", "1")], &lines) else {
        return;
    };
    let retail: Vec<String> = std::fs::read_to_string(TANK_GUN_BONES)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let seat_of = |log: &[String], offset: i32| -> BTreeMap<i32, i32> {
        log.iter()
            .filter_map(|l| l.trim().strip_prefix("PROBE bone "))
            .map(|r| {
                let t: Vec<&str> = r.split_whitespace().collect();
                (t[0].parse::<i32>().unwrap() - offset, t[2].parse().unwrap())
            })
            .collect()
    };
    let (r, o) = (bones(&retail, 0), bones(&ours.log, ours.offset));
    let seats = seat_of(&retail, 0);
    let first = r.keys().map(|(t, _)| *t).min().unwrap_or(0) + 50;
    let report = std::env::var("VEHICLE_REPORT").is_ok();
    // Per seat: samples, their sum, and the worst (miss, time, tag).
    type Worst = (f32, i32, String);
    let mut per_seat: BTreeMap<i32, (usize, f32, Worst)> = BTreeMap::new();
    for ((t, tag), a) in r.iter().filter(|((t, _), _)| *t > first) {
        let Some(b) = o.get(&(*t, tag.clone())) else {
            continue;
        };
        let d = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max);
        if report {
            eprintln!("{t} {tag}: retail {a:?} ours {b:?} off {d:.2}");
        }
        let e = per_seat
            .entry(seats[t])
            .or_insert((0, 0.0, (0.0, 0, String::new())));
        e.0 += 1;
        e.1 += d;
        if d > e.2.0 {
            e.2 = (d, *t, tag.clone());
        }
    }
    for (seat, (n, sum, worst)) in &per_seat {
        eprintln!(
            "seat {seat}: {n} samples, mean {:.3}, worst {worst:?}",
            sum / *n as f32
        );
    }
    for (seat, mean_tol, worst_tol) in TANK_BONE_TOLERANCE {
        let (n, sum, worst) = per_seat.get(&seat).cloned().unwrap_or_default();
        assert!(n > 200, "seat {seat}: only {n} samples in common");
        let mean = sum / n as f32;
        assert!(mean <= mean_tol, "seat {seat}: mean {mean} off");
        assert!(worst.0 <= worst_tol, "seat {seat}: {worst:?} off retail's");
    }
}

/// Per seat, the mean and worst miss of one bone sample: both seats read
/// retail's marks to the print's 0.01 (driver 0, gunner 0.01 at 18500).
const TANK_BONE_TOLERANCE: [(i32, f32, f32); 2] = [(1, 0.001, 0.02), (2, 0.001, 0.02)];

/// The client's own gunner draws his gun off his view, turned into the
/// hull and clamped to `fTorsoPitch`/`fWaistPitch` (cgame 0x300207c0): once
/// retail's server gun has settled on the same view, the two agree.
#[test]
fn the_own_gunner_s_gun_settles_where_retail_s_server_puts_it() {
    use vcod_common::vehiclepose::{self, KIND_FOUR_WHEEL, VehicleFields};
    let text = std::fs::read_to_string(JEEP_GUN).unwrap();
    let snaps: Vec<Snap> = parse(&text)
        .into_iter()
        .filter_map(|l| match l {
            Line::Snap(s) => Some(s),
            _ => None,
        })
        .collect();
    let f = |m: &BTreeMap<String, i32>, k: &str| f32::from_bits(*m.get(k).unwrap_or(&0) as u32);
    let diff = |a: f32, b: f32| ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs();
    let (mut n, mut worst, mut worst_on, mut pinned) = (0, 0.0f32, 0.0f32, 0);
    for w in snaps.windows(2) {
        let (prev, s) = (&w[0], &w[1]);
        let (Some(pv), Some(v)) = (&prev.veh, &s.veh) else {
            continue;
        };
        let gun = |m: &BTreeMap<String, i32>| [f(m, "angles2[2]"), f(m, "origin2[1]")];
        let view = |m: &BTreeMap<String, i32>| [0, 1].map(|i| f(m, &format!("viewangles[{i}]")));
        if s.wire.get("vehPos") != Some(&2)
            || gun(pv) != gun(v)
            || view(&prev.wire) != view(&s.wire)
        {
            continue; // the server's gun still turning
        }
        let i = |k: &str| *v.get(k).unwrap_or(&0);
        let fields = VehicleFields {
            body_packed: [i("origin2[0]"), i("origin2[2]")],
            turret: [f(v, "angles2[0]"), f(v, "angles2[1]")],
            gunner: gun(v),
            steer: f(v, "leanf"),
            tread_ms: i("time"),
            kind: i("surfType"),
            gunner_spans: [f(v, "fTorsoPitch"), f(v, "fWaistPitch")],
        };
        assert_eq!(fields.kind, KIND_FOUR_WHEEL);
        let mut pose = vehiclepose::lerp(&fields, &fields, 0.0);
        let [vp, vy] = view(&s.wire);
        let hull = [0, 1, 2].map(|k| f(v, &format!("apos.trBase[{k}]")));
        vehiclepose::own_gunner(&mut pose, [vp, vy, 0.0], hull, fields.gunner_spans);
        // A 16-bit angle reads the span's 30 as 29.998.
        let [up, down] = fields.gunner_spans;
        let on_span = diff(fields.gunner[0], -up) < 0.01 || diff(fields.gunner[0], down) < 0.01;
        let (dp, dy) = (
            diff(pose.gunner_gun[0], fields.gunner[0]),
            diff(pose.gunner_base[1], fields.gunner[1]),
        );
        assert!(
            dp < 0.02,
            "t={} ours {pose:?} wire {:?}",
            s.t,
            fields.gunner
        );
        // On a span the client turns the clamped view back into the hull, so
        // its yaw comes off the tilted hull's frame where the server clamps
        // the pitch alone and keeps the unclamped yaw.
        let tol = if on_span { 1.0 } else { 0.02 };
        assert!(dy < tol, "t={} ours {pose:?} wire {:?}", s.t, fields.gunner);
        if on_span {
            worst_on = worst_on.max(dy);
        } else {
            worst = worst.max(dy.max(dp));
        }
        n += 1;
        pinned += usize::from(on_span);
    }
    eprintln!("{n} settled, {pinned} on a pitch span (yaw within {worst_on}), worst off {worst}");
    assert!(n > 100 && pinned > 20, "{n} settled, {pinned} pinned");
}

/// `ClientSpawn` takes a rider out of his seat (0x44df4) and thinks on his
/// last cmd: a driver holding forward whom the script spawns 3 s after the
/// seat reads the origin that cmd walked him to from the spot, inside the
/// spawning thread, as retail's (`fixtures/seatspawn/mp_cassino-tank-uo.txt`,
/// coduo-vehicles.md 11.5).
#[test]
fn a_driver_spawned_out_of_the_seat_reads_retail_s_origin() {
    let Some(mut b) = board("mp_cassino", "panzeriv_mp", &[("probe_respawn", "3")]) else {
        return;
    };
    let fixture = include_str!("fixtures/seatspawn/mp_cassino-tank-uo.txt");
    // The capture's `ram` sits 2 s, then holds forward.
    for i in 0..120 {
        let mut c = holding(&b.cl, 0);
        if i >= 40 {
            c.forward = 127;
        }
        b.frame(c);
        if b.sv
            .script_log()
            .iter()
            .any(|l| l.starts_with("PROBE seat "))
        {
            break;
        }
    }
    let read = |l: &str| l.split(' ').skip(3).collect::<Vec<_>>().join(" ");
    let theirs = fixture
        .lines()
        .find(|l| l.starts_with("PROBE seat "))
        .map(read)
        .unwrap();
    let mine =
        b.sv.script_log()
            .iter()
            .find(|l| l.starts_with("PROBE seat "))
            .map(|l| read(l))
            .expect("our PROBE seat");
    assert_eq!(
        mine, theirs,
        "origin and angles the script reads after the spawn"
    );
}

/// What the entity pass finds of a tank driver boarded by use: his turn
/// links him with no seat offset, 20 above the seat his end frame puts
/// `ps.origin` on (coduo-vehicles.md 13, "The tank driver's link offset"),
/// so the area tree a later entity's blast or missile queries, and the body
/// such a round traces, sit at the link through the first stint, and on
/// the seat once a `nextvehslot` has asked for the slot it takes.
#[test]
fn a_tank_driver_boarded_by_use_sits_in_the_area_tree_at_his_link() {
    let Some(mut b) = board("mp_cassino", "panzeriv_mp", &[]) else {
        return;
    };
    let p = &PROTOCOL_V22;
    // Held still long enough for the turret to settle, whose turn moves
    // the seat between the link and the end frame.
    let lift = |b: &mut Boarded| {
        for _ in 0..100 {
            let c = holding(&b.cl, 0);
            b.frame(c);
        }
        let ps = &b.cl.snapshots().newest().unwrap().ps;
        let (mins, _) = b.sv.test_area_link(0).expect("the driver is linked");
        let mins_z = f(ps.field_i32(p, "mins[2]"));
        let origin_z = ps.origin(p)[2];
        mins[2] - mins_z - origin_z
    };
    let first = lift(&mut b);
    // A link box is grown a unit each way (`SV_LinkEntity`).
    assert!(
        (first - 19.0).abs() < 1e-3,
        "first stint: {first} above the seat"
    );
    for seat in [2, 1] {
        b.cl.send_reliable("nextvehslot");
        for _ in 0..60 {
            let c = holding(&b.cl, 0);
            b.frame(c);
            if b.cl.snapshots().newest().unwrap().ps.field_i32(p, "vehPos") == seat {
                break;
            }
        }
    }
    let back = lift(&mut b);
    assert!(
        (back + 1.0).abs() < 1e-3,
        "back in the driver's seat: {back}"
    );
}

const TANK_RESPAWN: &str =
    "tests/fixtures/playerstate/mp_cassino-tdm-slope-8ms-tank-respawn-uo.txt";

/// The same spawn on the capture's own cmds: the frame after it the
/// spawned driver has fallen and walked as far as those cmds' clock took
/// him, which `PROBE seatnext` reads (retail 31.96 here, 33.15 in
/// `mp_cassino-tank-uo.txt`'s run); then every snapshot on foot.
#[test]
fn a_driver_spawned_out_of_the_seat_moves_on_retail_s_cmds() {
    let text = std::fs::read_to_string(TANK_RESPAWN).unwrap();
    let lines = parse(&text);
    let Some(ours) = replay(
        "mp_cassino",
        "panzeriv_mp",
        &[("probe_respawn", "3")],
        &lines,
    ) else {
        return;
    };
    let fixture = include_str!("fixtures/seatspawn/mp_cassino-tank-cmds-uo.txt");
    let read = |log: &mut dyn Iterator<Item = &str>, offset: i32| -> Vec<String> {
        log.filter(|l| l.starts_with("PROBE seat"))
            .map(|l| {
                let t: Vec<&str> = l.splitn(4, ' ').collect();
                let at = t[2].parse::<i32>().unwrap() - offset;
                format!("{} {at} {}", t[1], t[3])
            })
            .collect()
    };
    let theirs = read(&mut fixture.lines(), 0);
    let mine = read(&mut ours.log.iter().map(String::as_str), ours.offset);
    assert_eq!(theirs.len(), 2, "retail's seat and seatnext");
    assert_eq!(
        mine, theirs,
        "what the script reads after the spawn and a frame later"
    );
    tally(&lines, &ours.samples).assert_within(&[]);
}

/// Every impact in the order it first showed on a snapshot.
fn new_impacts(snaps: impl Iterator<Item = Vec<(u32, [i32; 3])>>) -> Vec<[i32; 3]> {
    let mut prev = Vec::new();
    let mut out = Vec::new();
    for cur in snaps {
        out.extend(cur.iter().filter(|i| !prev.contains(*i)).map(|i| i.1));
        prev = cur;
    }
    out
}

const JEEP_RIDER: &str = "tests/fixtures/playerstate/mp_peaks-tdm-slope-8ms-jeep-rider-uo.txt";

/// The jeep passenger's own round, `SEATS`' `seat_fire` with the view 75
/// degrees down past the hull, from a kar98k with no spread
/// (`common::STEADY_WEAPON` on both servers): UO's `CalcMuzzlePoint` puts
/// a rider's muzzle 28 over his origin, not at his eye, so the round
/// strikes the ground where retail's did (docs/research/cod11-combat.md
/// 2.1).
#[test]
fn the_jeep_passenger_s_round_lands_as_on_retail() {
    let text = std::fs::read_to_string(JEEP_RIDER).unwrap();
    let lines = parse(&text);
    let cvars = [("probe_side", "75"), ("probe_pitch", "15")];
    let Some(ours) = replay_with(
        "mp_peaks",
        "willyjeep_mp",
        &cvars,
        &lines,
        &Setup::default(),
        |fs| common::steady_weapon(fs, "kar98k_mp"),
    ) else {
        return;
    };
    let retail = new_impacts(lines.iter().filter_map(|l| match l {
        Line::Snap(s) => Some(s.imp.clone()),
        Line::Cmd(_) => None,
    }));
    let ours = new_impacts(ours.samples.iter().map(|s| s.impacts.clone()));
    if std::env::var("VEHICLE_REPORT").is_ok() {
        eprintln!("rider rounds: retail {retail:?} ours {ours:?}");
    }
    assert_eq!(retail.len(), 1, "{retail:?}");
    assert_eq!(ours.len(), retail.len(), "retail {retail:?} ours {ours:?}");
    for (r, o) in retail.iter().zip(&ours) {
        // Both snapped to whole units: one muzzle lands on one unit, or the
        // next across an edge.
        assert!(
            r.iter().zip(o).all(|(a, b)| (a - b).abs() <= 1),
            "retail {r:?} ours {o:?}"
        );
    }
}
