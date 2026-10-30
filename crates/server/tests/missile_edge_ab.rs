//! Missiles and swings at the edges, ours against retail's
//! (`fixtures/missiles/`). `client-probes/probe_skyshot` holds a shooter
//! under the sky and fires a rocket 60 degrees up: 1.1d frees it where its
//! move meets the sky, UO pushes it through and it goes off 15 s on, out
//! past the map (combat doc 12.3). Its air part swings at nothing, which
//! raises no event on either, and at the floor, which raises the miss
//! (2.5). `client-probes/probe_takedamage` stands a `setTakeDamage` barrel
//! in front of a UO shooter: a rocket's direct hit and a swing both run
//! `G_Damage` on it (13.1, 2.5). `probe_skyshot`'s spot part stands the shooter on
//! mp_harbor's sea wall: a rocket, a frag and UO's smoke grenade into the
//! water raise retail's splash (12.1). A rocket fired with the shooter's eye
//! inside mp_carentan's 0x2080 kerb brush starts solid and goes off at
//! the end of its first move (11.1, 12.3).
//!
//! `ME_REPORT=1` prints both logs. The 1.1d tests need `COD_DIR` at a 1.1
//! install, the 1.5b rows a 1.5 or UO one, the UO ones a UO one; each
//! returns early without.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, BUTTON_MELEE, UserCmd};
use vcod_common::net::{NetClient, NetEvent};

const SKYSHOT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_skyshot.gsc");
const TAKEDAMAGE: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_takedamage.gsc");

/// What the shooter does from its first live snapshot, as
/// `vehicle_probe.rs`'s on-foot scripts: `Aim` settles 1 s and holds the
/// sight and the trigger, `Melee` settles 3 s and taps melee for 100 ms
/// three times 1.5 s apart.
#[derive(Clone, Copy, PartialEq)]
enum Script {
    Aim,
    Melee,
    /// `--probe-uo-weapon throw`: settles 3 s, pulls back for 1 s, waits
    /// 6 s and pulls back for 1 s again.
    Throw,
}

impl Script {
    fn buttons(self, ms: i64) -> u8 {
        match self {
            Script::Aim if ms >= 1000 => BUTTON_ADS | BUTTON_ATTACK,
            Script::Melee if (0..3).any(|k| (3000 + 1500 * k..3100 + 1500 * k).contains(&ms)) => {
                BUTTON_MELEE
            }
            Script::Throw if (3000..4000).contains(&ms) || (10_000..11_000).contains(&ms) => {
                BUTTON_ATTACK
            }
            _ => 0,
        }
    }
}

/// Our run of `name` (`script`) on `map` with `cvars` set and one
/// allies shooter running `run` for `ms` from its spawn. Returns the
/// script's log, then a line per snapshot per missile and temp entity in
/// the shooter's snapshots, in the fixtures' `VEH` shape.
fn ours(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    name: &str,
    script: &str,
    cvars: &[(&str, &str)],
    run: Script,
    ms: i64,
) -> Vec<String> {
    ours_pinned(
        fs,
        map,
        name,
        script,
        cvars,
        run,
        ms,
        Vec::new(),
        Vec::new(),
    )
}

/// [`ours`] with the missile pool's first draws pinned
/// (`Server::pin_missile_draws`), and the first rockets' `trDelta`
/// (`Server::pin_rocket_deltas`).
#[allow(clippy::too_many_arguments)]
fn ours_pinned(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    name: &str,
    script: &str,
    cvars: &[(&str, &str)],
    run: Script,
    ms: i64,
    draws: Vec<f64>,
    rocket_deltas: Vec<[f32; 3]>,
) -> Vec<String> {
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, name), now);
    sv.set_protocol(vcod_server::server::install_protocol(&fs));
    sv.overlay_script(&format!("maps/mp/gametypes/{name}"), script);
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    sv.pin_missile_draws(draws);
    sv.pin_rocket_deltas(rocket_deltas);
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    // The net probe's allies pick (`--probe-team allies`): mp_harbor's
    // allies are Russian.
    let rifle = if map == "mp_harbor" {
        "mosin_nagant_mp"
    } else {
        "m1carbine_mp"
    };
    let mut join = Join::new("allies", rifle);
    let mut hold = common::ProbeHold {
        aim: true,
        ..Default::default()
    };
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let mut selected: Option<u8> = None;
    let mut spawned: Option<i64> = None;
    let mut last_snap = None;
    let mut missiles = std::collections::BTreeSet::new();
    let mut lines = Vec::new();
    for i in 0..4000i64 {
        if spawned.is_some_and(|at| (i - at) * common::FRAME_MS > ms) {
            break;
        }
        now += frame;
        let live = cl.state() == vcod_common::net::NetState::Active
            && cl
                .snapshots()
                .newest()
                .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
        if live {
            let at = *spawned.get_or_insert(i);
            let mut cmds = hold.cmds(&cl, run.buttons((i - at) * common::FRAME_MS));
            if let Some(w) = selected {
                cmds.iter_mut().for_each(|c| c.weapon = w);
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if tokens.first().map(String::as_str) == Some("a") {
                    selected = tokens.get(1).and_then(|w| w.parse().ok());
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        let p = cl.protocol();
        let Some(snap) = cl.snapshots().newest() else {
            continue;
        };
        if last_snap == Some(snap.message_num) {
            continue;
        }
        last_snap = Some(snap.message_num);
        let v = snap.ps.viewangles(p);
        lines.push(format!(
            "VEH {} ps view {:.2} {:.2} frac {:.3} wstate {} weapon {}",
            snap.server_time,
            v[0],
            v[1],
            snap.ps.field_f32(p, "fWeaponPosFrac"),
            snap.ps.field_i32(p, "weaponstate"),
            snap.ps.field_i32(p, "weapon"),
        ));
        missiles.retain(|n| snap.entities.contains_key(n));
        for (num, e) in &snap.entities {
            let i = |n: &str| e.field_i32(p, n);
            let f = |n: &str| e.field_f32(p, n);
            if let Some(ev) = p.event_of(i("eType")) {
                let o = e.origin(p);
                lines.push(format!(
                    "VEH {} temp {num} ev {ev} parm {} other {} pos {:.1} {:.1} {:.1} \
                     surf {} scale {} weapon {}",
                    snap.server_time,
                    i("eventParm"),
                    i("otherEntityNum"),
                    o[0],
                    o[1],
                    o[2],
                    i("surfType"),
                    i("_union.scale"),
                    i("weapon"),
                ));
            } else if i("eType") == 4
                || missiles.contains(num)
                // Exploded on its first move: on the wire as `eType` 0.
                || (i("eType") == 0 && i("weapon") != 0 && i("eventSequence") != 0)
            {
                missiles.insert(*num);
                let slot = (i("eventSequence") - 1) & 3;
                lines.push(format!(
                    "VEH {} missile {num} type {} base {:.3} {:.3} {:.3} bits {:#x} {:#x} {:#x} \
                     apos {} {} {} atraj {} {} {} {} {} etype {} ev {} parm {}",
                    snap.server_time,
                    i("pos.trType"),
                    f("pos.trBase[0]"),
                    f("pos.trBase[1]"),
                    f("pos.trBase[2]"),
                    f("pos.trBase[0]").to_bits(),
                    f("pos.trBase[1]").to_bits(),
                    f("pos.trBase[2]").to_bits(),
                    f("apos.trBase[0]"),
                    f("apos.trBase[1]"),
                    f("apos.trBase[2]"),
                    i("apos.trType"),
                    i("apos.trTime"),
                    f("apos.trDelta[0]"),
                    f("apos.trDelta[1]"),
                    f("apos.trDelta[2]"),
                    i("eType"),
                    i(&format!("events[{slot}]")),
                    i(&format!("eventParms[{slot}]")),
                ));
            }
        }
    }
    let mut out: Vec<String> = sv
        .script_log()
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect();
    out.extend(lines);
    out
}

fn retail(fixture: &str) -> Vec<String> {
    fixture
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn report(retail: &[String], ours: &[String]) {
    if std::env::var_os("ME_REPORT").is_some() {
        for l in ours {
            eprintln!("ours   {l}");
        }
        for l in retail {
            eprintln!("retail {l}");
        }
    }
}

/// The trailing `(x, y, z)` of a PROBE line.
fn vec_at(l: &str, from: usize) -> [f32; 3] {
    let s = &l[l[from..].find('(').unwrap() + from + 1..];
    let s = &s[..s.find(')').unwrap()];
    let v: Vec<f32> = s.split(", ").map(|c| c.parse().unwrap()).collect();
    [v[0], v[1], v[2]]
}

/// The first missile's `PROBE miss` lines, rocket or grenade: (time,
/// origin).
fn first_rocket(lines: &[String]) -> Vec<(i32, [f32; 3])> {
    let miss: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("PROBE miss "))
        .collect();
    let Some(first) = miss.first() else {
        return Vec::new();
    };
    let num = first.split(' ').nth(3).unwrap().to_string();
    miss.iter()
        .filter(|l| l.split(' ').nth(3) == Some(num.as_str()))
        .map(|l| (l.split(' ').nth(2).unwrap().parse().unwrap(), vec_at(l, 0)))
        .collect()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

/// Two runs of one rocket over their first `frames` frames: each frame's
/// step and height. The launch takes the weapon's spread off each server's
/// own RNG (sighted, a fraction of a degree on both), so the rounds part
/// ways sideways; the climb does not.
fn same_climb(r: &[(i32, [f32; 3])], o: &[(i32, [f32; 3])], frames: usize, skip: &[usize]) {
    assert!(
        r.len() >= frames && o.len() >= frames,
        "retail {r:?}\nours {o:?}"
    );
    let (r, o) = (&r[..frames], &o[..frames]);
    for k in (1..frames).filter(|k| !skip.contains(k)) {
        let step = |v: &[(i32, [f32; 3])]| dist(v[k].1, v[k - 1].1);
        assert!(
            (step(r) - step(o)).abs() < 0.5,
            "frame {k}: retail {r:?}\nours {o:?}"
        );
        assert!(
            (r[k].1[2] - o[k].1[2]).abs() < 5.0,
            "frame {k}: retail {r:?}\nours {o:?}"
        );
    }
}

/// 1.1d: the rocket flies its six frames and is gone the frame its move
/// meets the sky, with no explode on the wire.
#[test]
fn a_rocket_at_the_sky_is_freed_silently() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fixture = retail(include_str!(
        "fixtures/missiles/mp_carentan-skyshot-rocket.txt"
    ));
    let log = ours(
        fs,
        "mp_carentan",
        "probe_skyshot",
        SKYSHOT,
        &[("probe_part", "sky"), ("probe_weapon", "panzerfaust_mp")],
        Script::Aim,
        15_000,
    );
    report(&fixture, &log);
    let (r, o) = (first_rocket(&fixture), first_rocket(&log));
    assert_eq!(o.len(), r.len(), "retail {r:?}\nours {o:?}");
    same_climb(&r, &o, r.len(), &[]);
    assert!(
        !log.iter()
            .any(|l| l.contains(" missile ") && l.contains("etype 0")),
        "ours exploded: {log:?}"
    );
}

/// UO: the rocket is pushed 2 into the sky, flies on above it on its line,
/// and goes off on its 15 s think out past the map with 185 and parm 0.
#[test]
fn a_uo_rocket_flies_on_through_the_sky() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = retail(include_str!(
        "fixtures/missiles/mp_carentan-skyshot-rocket-uo.txt"
    ));
    let log = ours(
        fs,
        "mp_carentan",
        "probe_skyshot",
        SKYSHOT,
        &[("probe_part", "sky"), ("probe_weapon", "bazooka_mp")],
        Script::Aim,
        25_000,
    );
    report(&fixture, &log);
    let (r, o) = (first_rocket(&fixture), first_rocket(&log));
    let through = |v: &[(i32, [f32; 3])]| v.iter().position(|p| p.1[2] > 2304.0);
    assert_eq!(
        through(&o),
        through(&r),
        "retail {:?}\nours {:?}",
        &r[..24],
        &o[..24]
    );
    let at = through(&r).expect("retail's rocket went through");
    assert!(
        (o[at].1[2] - r[at].1[2]).abs() < 0.01,
        "pushed 2 in: retail {:?}, ours {:?}",
        r[at],
        o[at]
    );
    // The contact frame's step is cut where the spread met the sky.
    same_climb(&r, &o, 24, &[at, at + 1]);
    let explode = |lines: &[String], launched: i32| {
        lines
            .iter()
            .find(|l| l.contains(" missile ") && l.contains("etype 0"))
            .map(|l| {
                let t: Vec<&str> = l.split(' ').collect();
                let v = |k: usize| t[k].parse::<f32>().unwrap();
                (
                    t[1].parse::<i32>().unwrap() - launched,
                    [v(7), v(8), v(9)],
                    t[t.len() - 3].to_string(),
                    t[t.len() - 1].to_string(),
                )
            })
            .unwrap_or_else(|| panic!("no explode in {lines:?}"))
    };
    let (rx, ox) = (explode(&fixture, r[0].0), explode(&log, o[0].0));
    assert_eq!((&ox.2, &ox.3), (&rx.2, &rx.3), "retail {rx:?}, ours {ox:?}");
    assert!((ox.0 - rx.0).abs() <= 50, "retail {rx:?}, ours {ox:?}");
    // Out past the map on the line: the spread swings where, not how far.
    let (rd, od) = (dist(rx.1, r[0].1), dist(ox.1, o[0].1));
    assert!(
        (rd - od).abs() < rd * 0.005,
        "retail {rx:?} {rd}, ours {ox:?} {od}"
    );
}

/// `PROBE mdamage` lines: (amount, mod, point).
fn mdamage(lines: &[String]) -> Vec<(i32, String, [f32; 3])> {
    lines
        .iter()
        .filter_map(|l| l.strip_prefix("PROBE mdamage "))
        .map(|l| {
            let t: Vec<&str> = l.split(' ').collect();
            (t[1].parse().unwrap(), t[2].to_string(), vec_at(l, 0))
        })
        .collect()
}

/// UO: each rocket at the `setTakeDamage` barrel takes it 1000 as
/// `MOD_PROJECTILE` at the contact, and the splash skips it.
#[test]
fn a_rocket_hits_a_takedamage_model_directly_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = retail(include_str!(
        "fixtures/missiles/mp_carentan-takedamage-rocket-uo.txt"
    ));
    let log = ours(
        fs,
        "mp_carentan",
        "probe_takedamage",
        TAKEDAMAGE,
        &[("probe_weapon", "bazooka_mp")],
        Script::Aim,
        20_000,
    );
    report(&fixture, &log);
    let (r, o) = (mdamage(&fixture), mdamage(&log));
    assert_eq!(o.len(), r.len(), "retail {r:?}\nours {o:?}");
    // The contact is on the barrel's side wherever the spread put it: its
    // distance off the barrel's axis is the collision's radius.
    let axis = |lines: &[String]| {
        let l = lines.iter().find(|l| l.starts_with("PROBE site ")).unwrap();
        vec_at(l, l.rfind('(').unwrap())
    };
    let off = |p: [f32; 3], a: [f32; 3]| ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    let (ra, oa) = (axis(&fixture), axis(&log));
    for (rd, od) in r.iter().zip(&o) {
        assert_eq!(
            (od.0, od.1.as_str()),
            (rd.0, rd.1.as_str()),
            "retail {r:?}\nours {o:?}"
        );
        assert!(
            (off(od.2, oa) - off(rd.2, ra)).abs() < 1.0,
            "retail {r:?}\nours {o:?}"
        );
    }
}

/// UO: a swing at the `setTakeDamage` barrel raises the miss naming it and
/// takes it `meleeDamage + rand()%5` as `MOD_MELEE` at the trace's end.
#[test]
fn a_swing_damages_a_takedamage_model_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = retail(include_str!(
        "fixtures/missiles/mp_carentan-takedamage-melee-uo.txt"
    ));
    let log = ours(
        fs,
        "mp_carentan",
        "probe_takedamage",
        TAKEDAMAGE,
        &[("probe_dist", "40")],
        Script::Melee,
        8000,
    );
    report(&fixture, &log);
    let (r, o) = (mdamage(&fixture), mdamage(&log));
    assert_eq!(o.len(), r.len(), "retail {r:?}\nours {o:?}");
    let low = r.iter().map(|d| d.0).min().unwrap();
    for d in &o {
        assert_eq!(d.1, "MOD_MELEE");
        assert!(
            (low - 4..low + 5).contains(&d.0),
            "retail {r:?}\nours {o:?}"
        );
        assert!(dist(d.2, r[0].2) < 0.5, "retail {r:?}\nours {o:?}");
    }
    let barrel = |lines: &[String]| {
        lines
            .iter()
            .find_map(|l| l.strip_prefix("PROBE site "))
            .and_then(|l| l.split(' ').nth(5).map(str::to_string))
            .expect("the barrel was spawned")
    };
    let misses = |lines: &[String], b: &str| {
        lines
            .iter()
            .filter(|l| l.contains(" temp ") && l.contains(" ev 167 "))
            .all(|l| l.contains(&format!(" other {b} ")))
            && lines.iter().any(|l| l.contains(" ev 167 "))
    };
    assert!(misses(&fixture, &barrel(&fixture)));
    assert!(misses(&log, &barrel(&log)), "{log:?}");
}

/// A swing at nothing raises no temp entity; at the floor it raises 167 on
/// the world with parm 5.
#[test]
fn a_swing_at_nothing_raises_no_event() {
    for (fs, fixture) in [
        (
            vcod_common::testing::game_fs_11(),
            include_str!("fixtures/missiles/mp_carentan-skyshot-air.txt"),
        ),
        (
            vcod_common::testing::uo_fs(),
            include_str!("fixtures/missiles/mp_carentan-skyshot-air-uo.txt"),
        ),
    ] {
        let Some(fs) = fs else {
            continue;
        };
        let fixture = retail(fixture);
        let log = ours(
            fs,
            "mp_carentan",
            "probe_skyshot",
            SKYSHOT,
            &[("probe_part", "air")],
            Script::Melee,
            8000,
        );
        report(&fixture, &log);
        assert!(!fixture.iter().any(|l| l.contains(" temp ")));
        assert!(!log.iter().any(|l| l.contains(" temp ")), "{log:?}");
    }
}

/// The floor swing's truncated event origin: ours reads x 1375 where
/// retail reads 1374 with y 2615 on both, though the shooter faces yaw 45
/// from (1368, 2608.1). At pitch 80 a 64-unit swing from the eye 60 up meets
/// the floor 7.5 out along each axis; retail's x lands under 7, a start or
/// aim a fraction of a unit off ours that this capture cannot pin (combat
/// doc 2.5).
const FLOOR_POS_GAP: f32 = 1.0;

#[test]
fn a_swing_at_the_floor_raises_the_miss() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fixture = retail(include_str!(
        "fixtures/missiles/mp_carentan-skyshot-floor.txt"
    ));
    let log = ours(
        fs,
        "mp_carentan",
        "probe_skyshot",
        SKYSHOT,
        &[("probe_part", "air"), ("probe_pitch", "80")],
        Script::Melee,
        8000,
    );
    report(&fixture, &log);
    // One line per temp entity: its first snapshot.
    let firsts = |lines: &[String]| -> Vec<String> {
        let mut seen = std::collections::BTreeSet::new();
        lines
            .iter()
            .filter(|l| l.contains(" temp "))
            .filter(|l| seen.insert(l.split(' ').nth(3).unwrap().to_string()))
            .map(|l| l.splitn(5, ' ').nth(4).unwrap().to_string())
            .collect()
    };
    let (r, o) = (firsts(&fixture), firsts(&log));
    assert_eq!(r.len(), 3, "{fixture:?}");
    assert_eq!(o.len(), r.len(), "retail {r:?}\nours {o:?}");
    for (a, b) in r.iter().zip(&o) {
        let (ta, tb): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
        assert_eq!(ta[..6], tb[..6], "retail {r:?}\nours {o:?}");
        let pos = |t: &[&str]| -> [f32; 3] { std::array::from_fn(|k| t[7 + k].parse().unwrap()) };
        assert!(
            dist(pos(&ta), pos(&tb)) <= FLOOR_POS_GAP,
            "retail {r:?}\nours {o:?}"
        );
    }
}

/// The install's `main/` when it is 1.5's (a UO install's is): the 1.5b
/// fixtures' paks.
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    vcod_common::testing::game_fs().filter(|fs| {
        vcod_server::server::install_protocol(fs).transport.version
            == vcod_common::net::protocol::PROTOCOL_V6.transport.version
    })
}

/// The shooter on mp_harbor's south sea wall over open water, looking
/// along yaw 90 and `pitch` degrees down (`probe_skyshot`'s spot part),
/// his rockets leaving with `rocket_deltas`.
fn harbor(
    fs: vcod_common::pk3::Pk3Fs,
    weapon: &str,
    pitch: &str,
    run: Script,
    rocket_deltas: Vec<[f32; 3]>,
) -> Vec<String> {
    ours_pinned(
        fs,
        "mp_harbor",
        "probe_skyshot",
        SKYSHOT,
        &[
            ("probe_part", "spot"),
            ("probe_weapon", weapon),
            ("probe_x", "-12640"),
            ("probe_y", "-7036"),
            ("probe_z", "152"),
            ("probe_yaw", "90"),
            ("probe_pitch", pitch),
        ],
        run,
        20_000,
        Vec::new(),
        rocket_deltas,
    )
}

/// The `trDelta` retail's first rocket left with, off its script's first
/// two positions: the launch is a frame behind, so the second is 100 ms
/// of flight past the first, and the delta is whole units (`fire_rocket`
/// truncates it).
fn retail_rocket_delta(fixture: &[String]) -> [f32; 3] {
    let r = first_rocket(fixture);
    [0, 1, 2].map(|k| ((r[1].1[k] - r[0].1[k]) * 10.0).round())
}

/// The first temp entity's first snapshot: (time, number, the line from
/// `ev` on). The shooter's own spawn and swings raise none here.
fn splash(lines: &[String]) -> (i32, String, Vec<String>) {
    let l = lines
        .iter()
        .find(|l| l.contains(" temp "))
        .unwrap_or_else(|| panic!("no splash in {lines:?}"));
    let t: Vec<&str> = l.split(' ').collect();
    (
        t[1].parse().unwrap(),
        t[3].to_string(),
        t[4..].iter().map(|s| s.to_string()).collect(),
    )
}

/// The `VEH` missile lines' (time, base, etype) once it has exploded.
fn explode(lines: &[String]) -> Option<(i32, [f32; 3])> {
    lines
        .iter()
        .find(|l| l.contains(" missile ") && l.contains(" etype 0"))
        .map(|l| {
            let t: Vec<&str> = l.split(' ').collect();
            let v = |k: usize| t[k].parse::<f32>().unwrap();
            (t[1].parse().unwrap(), [v(7), v(8), v(9)])
        })
}

/// A frag thrown flat off the wall: its `trDelta` z (the throw's 120)
/// passes 30, so the move that enters the water raises the splash at the
/// missile's origin before it, and the whole flight matches retail's to
/// the hundredth (combat doc 12.1).
#[test]
fn a_frag_into_water_splashes() {
    for (fs, fixture) in [
        (
            vcod_common::testing::game_fs_11(),
            include_str!("fixtures/missiles/mp_harbor-water-frag.txt"),
        ),
        (
            fs_15(),
            include_str!("fixtures/missiles/mp_harbor-water-frag-15.txt"),
        ),
        (
            vcod_common::testing::uo_fs(),
            include_str!("fixtures/missiles/mp_harbor-water-frag-uo.txt"),
        ),
    ] {
        let Some(fs) = fs else {
            continue;
        };
        let fixture = retail(fixture);
        let log = harbor(fs, "fraggrenade_mp", "0", Script::Throw, Vec::new());
        report(&fixture, &log);
        let (r, o) = (first_rocket(&fixture), first_rocket(&log));
        assert!(r.len() > 30, "{fixture:?}");
        assert!(o.len() >= 30, "ours threw nothing: {log:?}");
        for k in 0..30 {
            assert!(
                dist(r[k].1, o[k].1) < 0.01,
                "frame {k}: retail {:?}\nours {:?}",
                &r[..30],
                &o[..30]
            );
        }
        let (rs, os) = (splash(&fixture), splash(&log));
        assert_eq!(os.1, rs.1, "retail {rs:?}, ours {os:?}");
        assert_eq!(os.2, rs.2, "retail {rs:?}, ours {os:?}");
        assert_eq!(os.0 - o[0].0, rs.0 - r[0].0, "retail {rs:?}, ours {os:?}");
        // The fuse runs out on the harbor floor, 70 under the water: 1.1d
        // and 1.5b go off there, UO's `G_ExplodeMissile` (0x5e2b5) moves
        // the explode up to the surface (coduo-diff.md 21, "Explode").
        let (rx, ox) = (
            explode(&fixture).expect("retail's went off"),
            explode(&log).unwrap_or_else(|| panic!("ours did not go off: {log:?}")),
        );
        assert_eq!(ox.0 - o[0].0, rx.0 - r[0].0, "retail {rx:?}, ours {ox:?}");
        assert_eq!(ox.1, rx.1, "retail {rx:?}, ours {ox:?}");
    }
}

/// A rocket 45 degrees down into the water: the splash on the move that
/// enters it, then the explode on the harbor floor, its origin snapped
/// toward the launch by `floor` and `ceil` (combat doc 12.1, 13.1). Each
/// server draws its own launch spread, so ours flies retail's `trDelta`
/// (`retail_rocket_delta`) and the splash and the explode are held
/// exactly: on 1.5b's flight the move that enters the water reaches the
/// floor too, and both go out in one snapshot.
#[test]
fn a_rocket_into_water_splashes() {
    for (fs, fixture, weapon) in [
        (
            vcod_common::testing::game_fs_11(),
            include_str!("fixtures/missiles/mp_harbor-water-rocket.txt"),
            "panzerfaust_mp",
        ),
        (
            fs_15(),
            include_str!("fixtures/missiles/mp_harbor-water-rocket-15.txt"),
            "panzerfaust_mp",
        ),
        (
            vcod_common::testing::uo_fs(),
            include_str!("fixtures/missiles/mp_harbor-water-rocket-uo.txt"),
            "bazooka_mp",
        ),
    ] {
        let Some(fs) = fs else {
            continue;
        };
        let fixture = retail(fixture);
        let log = harbor(
            fs,
            weapon,
            "45",
            Script::Aim,
            vec![retail_rocket_delta(&fixture)],
        );
        report(&fixture, &log);
        let (r, o) = (first_rocket(&fixture), first_rocket(&log));
        assert!(!o.is_empty(), "ours fired nothing: {log:?}");
        for k in 0..r.len().min(o.len()).min(4) {
            assert!(
                dist(r[k].1, o[k].1) < 0.01,
                "frame {k}: retail {r:?}\nours {o:?}"
            );
        }
        let (rs, os) = (splash(&fixture), splash(&log));
        assert_eq!(os.1, rs.1, "retail {rs:?}, ours {os:?}");
        assert_eq!(os.2, rs.2, "retail {rs:?}, ours {os:?}");
        assert_eq!(os.0 - o[0].0, rs.0 - r[0].0, "retail {rs:?}, ours {os:?}");
        let (rx, ox) = (
            explode(&fixture).expect("retail's exploded"),
            explode(&log).unwrap_or_else(|| panic!("ours did not explode: {log:?}")),
        );
        assert_eq!(ox.0 - o[0].0, rx.0 - r[0].0, "retail {rx:?}, ours {ox:?}");
        assert_eq!(ox.1, rx.1, "retail {rx:?}, ours {ox:?}");
    }
}

/// The integer `trDelta` whose `vectoangles` is the `apos` retail's
/// missile line shows (`fire_rocket` truncates the delta and takes its
/// angles off it), near `speed` along those angles; `None` unless exactly
/// one fits.
fn delta_for_apos(lines: &[String], speed: f32) -> Option<[f32; 3]> {
    let l = lines.iter().find(|l| l.contains(" missile "))?;
    let t: Vec<&str> = l.split(' ').collect();
    let at = t.iter().position(|w| *w == "apos")? + 1;
    let apos = [t[at].parse::<f32>().ok()?, t[at + 1].parse::<f32>().ok()?];
    let (p, y) = (apos[0].to_radians(), apos[1].to_radians());
    let near = glam::Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin()) * speed;
    let mut fits = Vec::new();
    for dx in -2..=2 {
        for dy in -2..=2 {
            for dz in -2..=2 {
                let d = [
                    near.x.trunc() + dx as f32,
                    near.y.trunc() + dy as f32,
                    near.z.trunc() + dz as f32,
                ];
                let a = vcod_server::game::builtins::math::vectoangles(d);
                if a[0] == apos[0] && a[1] == apos[1] {
                    fits.push(d);
                }
            }
        }
    }
    (fits.len() == 1).then(|| fits[0])
}

/// A rocket fired with the shooter's eye inside mp_carentan's 0x2080 caulk
/// brush over a window: `fire_rocket`'s clipmask 0x2802091 meets the
/// brush, so the first move starts solid, and every protocol reads that as
/// a contact at the move's end, where the rocket goes off in mid-air on
/// the launch snapshot. 1.1d keeps the trace's zero normal (parm 0), 1.5b
/// packs the reversed flight (combat doc 11.1, 12.3). Ours flies retail's
/// `trDelta`, recovered off its launch angles.
#[test]
fn a_rocket_fired_inside_a_kerb_brush_goes_off_at_once() {
    for (fs, fixture) in [
        (
            vcod_common::testing::game_fs_11(),
            include_str!("fixtures/missiles/mp_carentan-kerb-rocket.txt"),
        ),
        (
            fs_15(),
            include_str!("fixtures/missiles/mp_carentan-kerb-rocket-15.txt"),
        ),
    ] {
        let Some(fs) = fs else {
            continue;
        };
        let fixture = retail(fixture);
        let delta = delta_for_apos(&fixture, 2800.0).expect("one delta fits retail's apos");
        let log = ours_pinned(
            fs,
            "mp_carentan",
            "probe_skyshot",
            SKYSHOT,
            &[
                ("probe_part", "spot"),
                ("probe_weapon", "panzerfaust_mp"),
                ("probe_x", "1192"),
                ("probe_y", "-536"),
                ("probe_z", "340"),
                ("probe_yaw", "90"),
                ("probe_pitch", "0"),
            ],
            Script::Aim,
            20_000,
            Vec::new(),
            vec![delta],
        );
        report(&fixture, &log);
        let (r, o) = (first_rocket(&fixture), first_rocket(&log));
        assert!(!o.is_empty(), "ours fired nothing: {log:?}");
        let at = |m: &[(i32, [f32; 3])]| {
            m[..2]
                .iter()
                .map(|(t, p)| (t - m[0].0, *p))
                .collect::<Vec<_>>()
        };
        assert_eq!(at(&o), at(&r), "retail {r:?}\nours {o:?}");
        // From `ev` on: event, parm, apos and the rest.
        let line = |lines: &[String]| {
            let l = lines
                .iter()
                .find(|l| l.contains(" missile "))
                .unwrap_or_else(|| panic!("no missile on the wire: {lines:?}"));
            let t: Vec<String> = l.split(' ').map(str::to_string).collect();
            let ev = t.iter().position(|w| w == "ev").unwrap();
            let base = t.iter().position(|w| w == "base").unwrap();
            let time: i32 = t[1].parse().unwrap();
            (time, t[base..base + 4].to_vec(), t[ev..ev + 4].to_vec())
        };
        let (rl, ol) = (line(&fixture), line(&log));
        assert_eq!(ol.0 - o[0].0, rl.0 - r[0].0, "retail {rl:?}, ours {ol:?}");
        assert_eq!((&ol.1, &ol.2), (&rl.1, &rl.2), "retail {rl:?}, ours {ol:?}");
    }
}

/// UO frees a smoke grenade on its splash: the temp entity goes out and
/// the grenade is gone from that snapshot on, never popped (combat doc
/// 12.1).
#[test]
fn a_uo_smoke_grenade_into_water_is_freed() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = retail(include_str!(
        "fixtures/missiles/mp_harbor-water-smoke-uo.txt"
    ));
    let log = harbor(fs, "smokegrenade_mp", "0", Script::Throw, Vec::new());
    report(&fixture, &log);
    let gone = |lines: &[String]| {
        let (at, num, tail) = splash(lines);
        let missile = lines
            .iter()
            .filter(|l| l.contains(" missile "))
            .map(|l| l.split(' ').nth(1).unwrap().parse::<i32>().unwrap())
            .max()
            .expect("a grenade on the wire");
        (at - missile, num, tail)
    };
    let (rg, og) = (gone(&fixture), gone(&log));
    assert_eq!(og, rg, "retail {rg:?}, ours {og:?}");
    assert_eq!(rg.0, 50);
}

/// The first missile's `apos.trBase` in a committed net capture
/// (`crates/common/tests/fixtures/net/`).
fn retail_launch_angles(tag: &str) -> [f32; 3] {
    let p = &vcod_common::net::protocol::PROTOCOL_V1;
    let frames = vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-{tag}.bin"),
        &format!("net/snapshots-{tag}.bin"),
    );
    frames
        .iter()
        .flat_map(|f| f.snapshot.entities.values())
        .find(|e| e.field_i32(p, "eType") == 4)
        .map(|e| [0, 1, 2].map(|k| e.field_f32(p, &format!("apos.trBase[{k}]"))))
        .expect("a missile in the capture")
}

/// Our first missile line's `apos`.
fn our_launch_angles(log: &[String]) -> [f32; 3] {
    let l = log
        .iter()
        .find(|l| l.contains(" missile "))
        .expect("a missile on our wire");
    let v: Vec<f32> = l[l.find(" apos ").unwrap() + 6..]
        .split(' ')
        .take(3)
        .map(|c| c.parse().unwrap())
        .collect();
    [v[0], v[1], v[2]]
}

/// 1.1d's launch angles go through retail's `vectoangles` (0x3de94), down
/// positive and both in [0, 360): the sky part's rocket 80 degrees up reads
/// pitch 280.1, where Q3's sign sent -79.9 (combat doc 13.6). A frag's
/// pitch is `AngleNormalize360` of that less 120, in 65536ths truncated,
/// off the throw's velocity before `fire_grenade` truncates it (11.2):
/// bit for bit retail's. The rocket leaves along the bullet spread, drawn
/// off each server's own RNG inside the panzerfaust's `adsSpread` of 3.5
/// degrees, so its angles are held to that. Retail:
/// `net/snapshots-skyrocket.bin` and `net/snapshots-skyfrag.bin`,
/// `probe_skyshot`'s sky part by the recipe in its header (`throw` for the
/// frag), with `--save-snapshots --capture-tag`.
#[test]
fn launch_angles_match_retail_on_1_1() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    for (tag, weapon, run) in [
        ("skyrocket", "panzerfaust_mp", Script::Aim),
        ("skyfrag", "fraggrenade_mp", Script::Throw),
    ] {
        let log = ours(
            vcod_common::testing::game_fs_11().unwrap(),
            "mp_carentan",
            "probe_skyshot",
            SKYSHOT,
            &[("probe_part", "sky"), ("probe_weapon", weapon)],
            run,
            15_000,
        );
        report(&[], &log);
        let (r, o) = (retail_launch_angles(tag), our_launch_angles(&log));
        if tag == "skyfrag" {
            assert_eq!(o, r, "{tag}");
            continue;
        }
        assert!((r[0] - 280.1).abs() < 0.05, "retail {r:?}");
        let forward = |a: [f32; 3]| {
            let (p, y) = (a[0].to_radians(), a[1].to_radians());
            glam::Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin())
        };
        let off = forward(r).angle_between(forward(o)).to_degrees();
        assert!(
            (0.0..360.0).contains(&o[0]) && off < 3.5,
            "{tag}: retail {r:?}, ours {o:?}, {off} degrees apart"
        );
    }
}

/// One `apos` as the wire carried it: type, `trTime` from the launch's,
/// base, delta.
type Apos = (i32, i32, [f32; 3], [f32; 3]);

/// The first missile's distinct `apos` values in a committed net capture,
/// in wire order.
fn retail_apos(tag: &str) -> Vec<Apos> {
    let p = &vcod_common::net::protocol::PROTOCOL_V1;
    let frames = vcod_common::testing::fixture_frames(
        p,
        &format!("net/gamestate-{tag}.bin"),
        &format!("net/snapshots-{tag}.bin"),
    );
    let mut num = None;
    let mut out: Vec<Apos> = Vec::new();
    for f in &frames {
        for (n, e) in &f.snapshot.entities {
            if num.is_none() && e.field_i32(p, "eType") == 4 {
                num = Some(*n);
            }
            if num != Some(*n) {
                continue;
            }
            let v = |k: &str| [0, 1, 2].map(|i| e.field_f32(p, &format!("apos.{k}[{i}]")));
            let a = (
                e.field_i32(p, "apos.trType"),
                e.field_i32(p, "apos.trTime"),
                v("trBase"),
                v("trDelta"),
            );
            if out.last() != Some(&a) {
                out.push(a);
            }
        }
    }
    rebase(out)
}

/// The same off our first missile's lines.
fn our_apos(log: &[String]) -> Vec<Apos> {
    let mut num = None;
    let mut out: Vec<Apos> = Vec::new();
    for l in log.iter().filter(|l| l.contains(" missile ")) {
        let t: Vec<&str> = l.split(' ').collect();
        let n = t[3];
        if *num.get_or_insert(n) != n {
            continue;
        }
        let at = |k: &str| t.iter().position(|w| *w == k).unwrap() + 1;
        let f = |i: usize| t[i].parse::<f32>().unwrap();
        let (b, d) = (at("apos"), at("atraj"));
        let a = (
            t[d].parse().unwrap(),
            t[d + 1].parse().unwrap(),
            [f(b), f(b + 1), f(b + 2)],
            [f(d + 2), f(d + 3), f(d + 4)],
        );
        if out.last() != Some(&a) {
            out.push(a);
        }
    }
    rebase(out)
}

/// `trTime` counted from the launch's; a resting one's 0 stays 0.
fn rebase(mut v: Vec<Apos>) -> Vec<Apos> {
    let launch = v.first().map_or(0, |a| a.1);
    for a in v.iter_mut().filter(|a| a.0 != 0) {
        a.1 -= launch;
    }
    v
}

/// What retail drew for one flight: the tumble's pitch and roll rates,
/// then each contact's factor on the pitch rate (a floor's, whose size is
/// the ratio of the rates) or whole-degree step (a wall's).
fn retail_draws(apos: &[Apos]) -> Vec<f64> {
    let mut out = vec![f64::from(apos[0].3[0]), f64::from(apos[0].3[2])];
    for w in apos.windows(2).filter(|w| w[1].0 == w[0].0) {
        let (old, new) = (f64::from(w[0].3[0]), f64::from(w[1].3[0]));
        let ratio = (new / old).abs();
        if (0.55..=0.85).contains(&ratio) {
            out.push(ratio);
        } else {
            out.push(((new - old).rem_euclid(360.0).round() + 63.0).rem_euclid(360.0) - 63.0);
        }
    }
    out
}

/// `G_MissileLandAngles` (combat doc 12.7) against the sky frag's five
/// bounces in `net/snapshots-skyfrag.bin`: with retail's draws pinned (the
/// tumble and each floor contact's factor, which only the wire shows),
/// every `apos` our wire sends matches retail's bit for bit. That holds
/// the base laid onto the floor in whole 65536ths (0, and the 36.77124 a
/// pitch 49 off the floor eases to), the yaw and roll left unwrapped (a
/// roll of 1250), and the pitch rate's sign flip.
#[test]
fn bounce_angles_match_retail_on_1_1() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let retail = retail_apos("skyfrag");
    let draws = retail_draws(&retail);
    let log = ours_pinned(
        fs,
        "mp_carentan",
        "probe_skyshot",
        SKYSHOT,
        &[("probe_part", "sky"), ("probe_weapon", "fraggrenade_mp")],
        Script::Throw,
        15_000,
        draws.clone(),
        Vec::new(),
    );
    report(&[], &log);
    let ours = our_apos(&log);
    assert!(retail.len() >= 6, "five bounces on retail: {retail:?}");
    assert_eq!(
        ours, retail,
        "pinned draws {draws:?}\nours   {ours:?}\nretail {retail:?}"
    );
}
