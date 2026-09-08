//! The mover integrator against the retail capture, frame for frame.
//!
//! The evidence is `tests/fixtures/movers/mp_pavlov-dm-movers.txt`, the
//! server half of the paired run described in
//! `crates/gsc/tests/fixtures/semantics/client-probes/README.md`: retail's
//! own `getorigin()` and `.angles`, once per server frame, through each of
//! the ten scriptent verbs. `docs/research/cod11-movers.md` is what it
//! measured.
//!
//! The walk is not replayed and no map is loaded. Each phase is set up here
//! the way `probe_mover.gsc` set it up -- a `script_origin` at
//! `(100, 0, 100)` with zero angles -- the verb is called at the level time
//! retail called it at, and the level clock is stepped in 50 ms frames while
//! every sampled frame's origin and angles are compared. Retail's own frame
//! times are the ones stepped to, so a phase that starts at 4150 is compared
//! at 4150, not at a rebased zero.
//!
//! The wire half, `mp_pavlov-dm-movers-wire.txt`, is read by one test here:
//! the four records a ramped move puts on the wire. Its other cycles are not
//! replayed, because their entity numbers belong to a live map load and a
//! client's PVS and the shape they carry is the same one.
//!
//! What nothing here covers is the moving clip: a `script_brushmodel`'s
//! brushes do not follow its trajectory, and the capture has none to compare
//! against (`crate::game::mover`, and the movers doc's section 10).
//!
//! Needs no game data.

use std::collections::BTreeMap;

const FIXTURE: &str = "tests/fixtures/movers/mp_pavlov-dm-movers.txt";
const FRAME_MS: i32 = 50;

/// One `PROBE p <phase> <level time> (x, y, z) (p, y, r)` line.
#[derive(Debug, Clone, PartialEq)]
struct Sample {
    ms: i32,
    origin: [f32; 3],
    angles: [f32; 3],
}

/// Retail renders a vector as `(x, y, z)`, and the log line splits it across
/// three whitespace-separated tokens.
fn parse_vector(tokens: &[&str]) -> Option<[f32; 3]> {
    let mut out = [0.0; 3];
    for (i, t) in tokens.iter().enumerate().take(3) {
        let t = t.trim_start_matches('(').trim_end_matches(')');
        out[i] = t.trim_end_matches(',').parse().ok()?;
    }
    (tokens.len() >= 3).then_some(out)
}

/// The capture as `phase -> samples`, in file order.
fn retail() -> BTreeMap<String, Vec<Sample>> {
    let text = std::fs::read_to_string(FIXTURE).unwrap_or_else(|e| panic!("read {FIXTURE}: {e}"));
    let mut out: BTreeMap<String, Vec<Sample>> = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim_end().strip_prefix("PROBE p ") else {
            continue;
        };
        let t: Vec<&str> = rest.split_whitespace().collect();
        if t.len() < 8 {
            continue;
        }
        let Ok(ms) = t[1].parse::<i32>() else {
            continue;
        };
        let (Some(origin), Some(angles)) = (parse_vector(&t[2..5]), parse_vector(&t[5..8])) else {
            continue;
        };
        out.entry(t[0].to_string())
            .or_default()
            .push(Sample { ms, origin, angles });
    }
    out
}

/// What `probe_mover.gsc` called in each phase, transcribed from it. The
/// entity is fresh for each, so nothing carries over.
#[derive(Clone, Copy)]
enum Verb {
    MoveTo([f32; 3], f32, f32, f32),
    MoveX(f32, f32),
    MoveZ(f32, f32, f32, f32),
    RotateYaw(f32, f32),
    RotateTo([f32; 3], f32, f32, f32),
    RotateVelocity([f32; 3], f32, f32, f32),
    MoveGravity([f32; 3], f32),
}

fn phases() -> Vec<(&'static str, Verb)> {
    vec![
        (
            "moveto_t1",
            Verb::MoveTo([600.0, 0.0, 100.0], 1.0, 0.0, 0.0),
        ),
        (
            "moveto_t2",
            Verb::MoveTo([600.0, 0.0, 100.0], 2.0, 0.0, 0.0),
        ),
        (
            "moveto_ramp",
            Verb::MoveTo([600.0, 0.0, 100.0], 2.0, 0.5, 0.5),
        ),
        ("movex", Verb::MoveX(500.0, 2.0)),
        (
            "moveto_accel",
            Verb::MoveTo([600.0, 0.0, 100.0], 2.0, 0.5, 0.0),
        ),
        ("rotateyaw", Verb::RotateYaw(90.0, 2.0)),
        ("rotateto", Verb::RotateTo([0.0, 90.0, 0.0], 2.0, 0.5, 0.5)),
        (
            "rotatevelocity",
            Verb::RotateVelocity([0.0, 180.0, 0.0], 2.0, 0.0, 0.0),
        ),
        ("movegravity", Verb::MoveGravity([0.0, 0.0, 300.0], 3.0)),
    ]
}

/// `rotateyaw_twice` is left out of the table above and driven by its own
/// test: it is the only phase that calls a verb a second time mid-phase.
#[test]
fn every_verb_matches_the_retail_trace_frame_for_frame() {
    let retail = retail();
    let mut diffs: Vec<String> = Vec::new();

    for (phase, verb) in phases() {
        let samples = retail
            .get(phase)
            .unwrap_or_else(|| panic!("{FIXTURE} has no {phase} phase"));
        assert!(
            samples.len() > 10,
            "{phase}: only {} samples",
            samples.len()
        );

        // The probe's sampler logs one frame before the verb is called: the
        // threaded call runs to its first `wait` before the caller continues.
        let call_ms = samples[0].ms;
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = spawn_mover(&mut host, cx);
            host.level_time_ms = call_ms;
            call(&mut host, cx, e, verb);

            for s in &samples[1..] {
                host.level_time_ms = s.ms;
                vcod_server::game::mover::run(&mut host, cx);
                let (o, a) = read(&mut host, cx, e);
                if let Some(d) = differs(phase, s, o, a) {
                    diffs.push(d);
                }
            }
        });
    }

    assert!(
        diffs.is_empty(),
        "{} frames differ from retail\n{}",
        diffs.len(),
        diffs
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The one phase that calls its verb twice. Retail ran `rotateyaw(90, 1)`,
/// waited 1.5 s, and ran it again; the yaw went to 90 and then to 180, which
/// is what says the axis rotate verbs take deltas.
#[test]
fn a_second_rotateyaw_continues_from_where_the_first_stopped() {
    let retail = retail();
    let samples = retail
        .get("rotateyaw_twice")
        .expect("the rotateyaw_twice phase");
    let call_ms = samples[0].ms;

    let (mut vm, mut host) = fixture();
    let mut diffs: Vec<String> = Vec::new();
    vm.with_cx(|cx| {
        let e = spawn_mover(&mut host, cx);
        host.level_time_ms = call_ms;
        call(&mut host, cx, e, Verb::RotateYaw(90.0, 1.0));
        let second = call_ms + 1500;

        for s in &samples[1..] {
            host.level_time_ms = s.ms;
            if s.ms == second {
                call(&mut host, cx, e, Verb::RotateYaw(90.0, 1.0));
            }
            vcod_server::game::mover::run(&mut host, cx);
            let (o, a) = read(&mut host, cx, e);
            if let Some(d) = differs("rotateyaw_twice", s, o, a) {
                diffs.push(d);
            }
        }
    });

    assert!(
        diffs.is_empty(),
        "{} frames differ from retail\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// The completion notifies land on the frames retail logged them on. Retail's
/// `PROBE done` lines carry the level time, which is one frame past the end
/// of the trajectory (movers doc, section 8).
#[test]
fn the_completion_notifies_land_on_retails_frames() {
    let text = std::fs::read_to_string(FIXTURE).unwrap_or_else(|e| panic!("read {FIXTURE}: {e}"));
    let retail = retail();
    let mut checked = 0;

    for (phase, verb) in phases() {
        let want: Vec<(String, i32)> = text
            .lines()
            .filter_map(|l| l.trim_end().strip_prefix("PROBE done "))
            .map(|r| r.split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .filter(|t| t.len() >= 3 && t[0] == phase)
            .map(|t| (t[1].clone(), t[2].parse().unwrap_or(0)))
            .collect();
        assert_eq!(want.len(), 1, "{phase}: {} done lines", want.len());
        let (event, at) = &want[0];

        let samples = &retail[phase];
        let call_ms = samples[0].ms;
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = spawn_mover(&mut host, cx);
            host.level_time_ms = call_ms;
            call(&mut host, cx, e, verb);

            let mut seen = None;
            for f in 1..=120 {
                host.level_time_ms = call_ms + f * FRAME_MS;
                for d in vcod_server::game::mover::run(&mut host, cx) {
                    seen.get_or_insert((d.event, host.level_time_ms));
                }
            }
            assert_eq!(
                seen,
                Some((event.as_str(), *at)),
                "{phase}: retail raised {event} at {at}"
            );
        });
        checked += 1;
    }
    assert_eq!(checked, 9);
}

/// The wire half: the four records a ramped `movez(96, 2, 0.5, 0.5)` from
/// `z = 592` put on the wire, read out of the client capture and compared
/// against what our own table would send. This is the fact that decides the
/// design -- a mover travels as one trajectory per segment and the client
/// extrapolates between them -- so it is checked against the capture rather
/// than against a transcription of it.
#[test]
fn a_ramped_move_puts_retails_four_records_on_the_wire() {
    const WIRE: &str = "tests/fixtures/movers/mp_pavlov-dm-movers-wire.txt";
    let text = std::fs::read_to_string(WIRE).unwrap_or_else(|e| panic!("read {WIRE}: {e}"));

    // `entity <n> serverTime <t> eType <e> pos trType <n> trTime <t>
    // trDuration <d> base [x,y,z] delta [x,y,z] apos ...`
    let records: Vec<(i32, i32, i32, f32, f32)> = text
        .lines()
        .filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            let pos = t.iter().position(|w| *w == "pos")?;
            let num: i32 = t.get(1)?.parse().ok()?;
            let f = |s: &str| -> Option<f32> {
                s.trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .nth(2)?
                    .parse()
                    .ok()
            };
            Some((
                num,
                t.get(pos + 2)?.parse().ok()?,
                t.get(pos + 6)?.parse().ok()?,
                f(t.get(pos + 8)?)?,
                f(t.get(pos + 10)?)?,
            ))
        })
        .collect();
    assert!(!records.is_empty(), "{WIRE} carries no pos records");

    // The ramp cycle: the first accelerating record and the three after it
    // for the same entity.
    let i = records
        .iter()
        .position(|r| r.1 == 9)
        .expect("a TR_ACCELERATE record");
    let ent = records[i].0;
    let retail: Vec<(i32, i32, f32, f32)> = records[i..]
        .iter()
        .filter(|r| r.0 == ent)
        .take(4)
        .map(|r| (r.1, r.2, r.3, r.4))
        .collect();

    let (mut vm, mut host) = fixture();
    vm.with_cx(|cx| {
        use vcod_gsc::{Host, Value};
        let e = spawn_mover(&mut host, cx);
        let o = cx.intern_folded("origin");
        let start = retail[0].2;
        host.set_field(cx, e, o, Value::Vector([0.0, 0.0, start]))
            .unwrap();
        host.level_time_ms = 0;
        call(&mut host, cx, e, Verb::MoveZ(96.0, 2.0, 0.5, 0.5));

        let mut ours = Vec::new();
        let mut last = None;
        for f in 0..=45 {
            host.level_time_ms = f * FRAME_MS;
            vcod_server::game::mover::run(&mut host, cx);
            let (pos, _) = host.movers.wire(e).expect("a mover row");
            let rec = (pos.tr_type, pos.tr_duration, pos.base.z, pos.delta.z);
            if last.as_ref() != Some(&rec) {
                ours.push(rec);
                last = Some(rec);
            }
        }
        // The trailing stationary record keeps the delta of the segment it
        // ended, exactly as retail's does.
        assert_eq!(ours.len(), 4, "{ours:?}");
        for (i, (r, o)) in retail.iter().zip(&ours).enumerate() {
            assert_eq!((r.0, r.1), (o.0, o.1), "record {i}: type and duration");
            assert!(
                (r.2 - o.2).abs() < 0.01 && (r.3 - o.3).abs() < 0.01,
                "record {i}: retail base {} delta {}, ours base {} delta {}",
                r.2,
                r.3,
                o.2,
                o.3
            );
        }
    });
}

/// The game module's own test fixture, which is `#[cfg(test)]` and so out of
/// reach from here: a VM and a host with an empty object table.
fn fixture() -> (vcod_gsc::Vm, vcod_server::game::host::GameHost) {
    (
        vcod_gsc::Vm::new(),
        vcod_server::game::host::GameHost::new(vec![String::new(); 2048]),
    )
}

fn spawn_mover(
    host: &mut vcod_server::game::host::GameHost,
    cx: &mut vcod_gsc::Cx,
) -> vcod_gsc::EntId {
    use vcod_gsc::{Host, Value};
    let e = host.ents.spawn(cx).unwrap();
    let class = cx.intern_folded("classname");
    let v = Value::String(cx.intern_exact("script_origin"));
    host.set_field(cx, e, class, v).unwrap();
    let o = cx.intern_folded("origin");
    host.set_field(cx, e, o, Value::Vector([100.0, 0.0, 100.0]))
        .unwrap();
    let a = cx.intern_folded("angles");
    host.set_field(cx, e, a, Value::Vector([0.0; 3])).unwrap();
    e
}

fn call(
    host: &mut vcod_server::game::host::GameHost,
    cx: &mut vcod_gsc::Cx,
    e: vcod_gsc::EntId,
    verb: Verb,
) {
    use vcod_gsc::{Target, Value};
    let t = Some(Target::Entity(e));
    let f = Value::Float;
    let r = match verb {
        Verb::MoveTo(dest, s, a, d) => vcod_server::game::builtins::mover::move_to(
            host,
            cx,
            t,
            &[Value::Vector(dest), f(s), f(a), f(d)],
        ),
        Verb::MoveX(x, s) => vcod_server::game::builtins::mover::move_x(host, cx, t, &[f(x), f(s)]),
        Verb::MoveZ(z, s, a, d) => {
            vcod_server::game::builtins::mover::move_z(host, cx, t, &[f(z), f(s), f(a), f(d)])
        }
        Verb::RotateYaw(y, s) => {
            vcod_server::game::builtins::mover::rotate_yaw(host, cx, t, &[f(y), f(s)])
        }
        Verb::RotateTo(dest, s, a, d) => vcod_server::game::builtins::mover::rotate_to(
            host,
            cx,
            t,
            &[Value::Vector(dest), f(s), f(a), f(d)],
        ),
        Verb::RotateVelocity(v, s, a, d) => vcod_server::game::builtins::mover::rotate_velocity(
            host,
            cx,
            t,
            &[Value::Vector(v), f(s), f(a), f(d)],
        ),
        Verb::MoveGravity(v, s) => {
            vcod_server::game::builtins::mover::move_gravity(host, cx, t, &[Value::Vector(v), f(s)])
        }
    };
    r.expect("the verb");
}

fn read(
    host: &mut vcod_server::game::host::GameHost,
    cx: &mut vcod_gsc::Cx,
    e: vcod_gsc::EntId,
) -> ([f32; 3], [f32; 3]) {
    use vcod_gsc::{Host, Value};
    let get = |host: &mut vcod_server::game::host::GameHost, cx: &mut vcod_gsc::Cx, n: &str| {
        let atom = cx.intern_folded(n);
        match host.get_field(cx, e, atom) {
            Value::Vector(v) => v,
            _ => [0.0; 3],
        }
    };
    let o = get(host, cx, "origin");
    let a = get(host, cx, "angles");
    (o, a)
}

/// Retail logs two decimals, so the tolerance is the rendering's, not a
/// slack: half of the last printed digit.
const EPS: f32 = 0.005;

fn differs(phase: &str, s: &Sample, origin: [f32; 3], angles: [f32; 3]) -> Option<String> {
    let off = |a: [f32; 3], b: [f32; 3]| (0..3).any(|i| (a[i] - b[i]).abs() > EPS);
    (off(origin, s.origin) || off(angles, s.angles)).then(|| {
        format!(
            "{phase} at {}: retail origin {:?} angles {:?}, ours origin {:?} angles {:?}",
            s.ms, s.origin, s.angles, origin, angles
        )
    })
}
