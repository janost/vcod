//! `client-probes/probe_snapcull` on ours against a retail run of it
//! (`tests/fixtures/entities/snapcull-carentan-*.txt`): the subject's
//! entity lists and the viewer's compass word from the `quiet` step on.
//! Shared by `uocull_ab` (1.1, UO) and `cull15_ab` (1.5).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::Msec;
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::Cod1Wire;

/// An entity list as `number:eType`, the wire's type.
type Ents = BTreeSet<String>;

fn step_line(l: &str) -> Option<(String, i32)> {
    let rest = l.trim().strip_prefix("PROBE step ")?;
    let (name, t) = rest.split_once(' ')?;
    Some((name.to_string(), t.trim().parse().ok()?))
}

/// A side's `probe_snapcull` run: the step lines, the viewer's compass
/// word and the subject's entity list, each at every change.
struct SnapRun {
    steps: Vec<(String, i32)>,
    compass: Vec<(i32, i32)>,
    subject: Vec<(i32, Ents)>,
}

impl SnapRun {
    /// Every change from the `quiet` step to `done`, as (ms past `quiet`,
    /// value), the value in force at `quiet` first.
    fn past_quiet<T: Clone + PartialEq>(&self, changes: &[(i32, T)]) -> Vec<(i32, T)> {
        let at = |name: &str| self.steps.iter().find(|(n, _)| n == name).unwrap().1;
        let (quiet, done) = (at("quiet"), at("done"));
        let mut out: Vec<(i32, T)> = Vec::new();
        let first = changes.iter().rev().find(|(t, _)| *t <= quiet);
        for (t, v) in first
            .into_iter()
            .chain(changes.iter().filter(|(t, _)| *t > quiet && *t < done))
        {
            if out.last().is_none_or(|(_, l)| l != v) {
                out.push(((*t - quiet).max(0), v.clone()));
            }
        }
        out
    }
}

fn retail(path: &str, p: &Cod1Wire) -> SnapRun {
    let text = std::fs::read_to_string(path).expect("read the fixture");
    let field = |l: &str, key: &str| -> Option<String> {
        l.split_whitespace()
            .find_map(|w| w.strip_prefix(key))
            .map(str::to_string)
    };
    let compass = text
        .lines()
        .filter(|l| l.starts_with("COMPASS "))
        .filter_map(|l| {
            let t = field(l, "t=")?.parse().ok()?;
            let info = field(l, "info=0x")?;
            Some((t, u32::from_str_radix(&info, 16).ok()? as i32))
        })
        .collect();
    let subject = text
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("EDGE ents serverTime=")?;
            let (t, list) = rest.split_once(" [")?;
            let ents = list
                .trim_end_matches(']')
                .split_whitespace()
                .filter(|e| {
                    let t: i32 = e.split_once(':').unwrap().1.parse().unwrap();
                    p.event_of(t).is_none()
                })
                .map(str::to_string)
                .collect();
            Some((t.parse().ok()?, ents))
        })
        .collect();
    SnapRun {
        steps: text.lines().filter_map(step_line).collect(),
        compass,
        subject,
    }
}

fn ours(fs: vcod_common::pk3::Pk3Fs, p: &'static Cod1Wire, cvars: &[(&str, &str)]) -> SnapRun {
    use super::{ClientEnd, Queues};
    use vcod_common::net::NetClient;
    const SNAP_MAP: &str = "mp_carentan";
    let bsp_path = fs.resolve_map(SNAP_MAP).expect("the map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(super::cfg(SNAP_MAP, "probe_snapcull"), now);
    sv.set_protocol(p);
    sv.overlay_script(
        "maps/mp/gametypes/probe_snapcull",
        include_str!("../../../gsc/tests/fixtures/semantics/client-probes/probe_snapcull.gsc"),
    );
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    let mut viewer = NetClient::start_with_qport(ClientEnd(qa.clone()), now, 0x2001);
    let mut subject = NetClient::start_with_qport(ClientEnd(qb.clone()), now, 0x2002);
    let mut compass: Vec<(i32, i32)> = Vec::new();
    let mut ents: Vec<(i32, Ents)> = Vec::new();
    let (mut seen_v, mut seen_s) = (i32::MIN, i32::MIN);
    for _ in 0..(60_000 / super::FRAME_MS) {
        now += Duration::from_millis(super::FRAME_MS as u64);
        viewer.send_frame(&NULL_USERCMD);
        subject.send_frame(&NULL_USERCMD);
        super::step_pair(&mut sv, (&qa, &mut viewer), (&qb, &mut subject), now);
        if let Some(s) = viewer.snapshots().newest()
            && s.server_time != Msec::from_raw(seen_v)
        {
            seen_v = s.server_time.raw();
            let info = s.ps.field_i32(p, "iCompassFriendInfo");
            if compass.last().is_none_or(|(_, i)| *i != info) {
                compass.push((s.server_time.raw(), info));
            }
        }
        if let Some(s) = subject.snapshots().newest()
            && s.server_time != Msec::from_raw(seen_s)
        {
            seen_s = s.server_time.raw();
            let list: Ents = s
                .entities
                .iter()
                .map(|(n, e)| (n, e.field_i32(p, "eType")))
                .filter(|&(_, t)| p.event_of(t).is_none())
                .map(|(n, t)| format!("{n}:{t}"))
                .collect();
            if ents.last().is_none_or(|(_, e)| *e != list) {
                ents.push((s.server_time.raw(), list));
            }
        }
        if sv
            .script_log()
            .iter()
            .any(|l| l.contains("PROBE step done"))
        {
            break;
        }
    }
    SnapRun {
        steps: sv
            .script_log()
            .iter()
            .filter_map(|l| step_line(l))
            .collect(),
        compass,
        subject: ents,
    }
}

/// `probe_snapcull`'s rows, retail's `fixture` against ours.
pub fn matches(fixture: &str, fs: vcod_common::pk3::Pk3Fs, p: &'static Cod1Wire) {
    matches_with(fixture, fs, p, &[]);
}

/// [`matches`] with the probe's `cvars` set on ours as retail's run set them.
pub fn matches_with(
    fixture: &str,
    fs: vcod_common::pk3::Pk3Fs,
    p: &'static Cod1Wire,
    cvars: &[(&str, &str)],
) {
    let r = retail(fixture, p);
    let o = ours(fs, p, cvars);
    let names = |s: &SnapRun| s.steps.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&r), names(&o), "steps");
    let fmt = |c: Vec<(i32, i32)>| {
        c.into_iter()
            .map(|(t, i)| (t, format!("{i:#010x}")))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        fmt(o.past_quiet(&o.compass)),
        fmt(r.past_quiet(&r.compass)),
        "the viewer's iCompassFriendInfo past quiet, (ms, word)"
    );
    let (rs, os) = (r.past_quiet(&r.subject), o.past_quiet(&o.subject));
    assert_eq!(
        os.iter().map(|(t, e)| (*t, e.len())).collect::<Vec<_>>(),
        rs.iter().map(|(t, e)| (*t, e.len())).collect::<Vec<_>>(),
        "the subject's lists past quiet, (ms, entities)"
    );
    for ((t, a), (_, b)) in rs.iter().zip(&os) {
        assert!(
            a == b,
            "+{t}: retail only {:?}, ours only {:?}",
            a.difference(b).collect::<Vec<_>>(),
            b.difference(a).collect::<Vec<_>>()
        );
    }
}
