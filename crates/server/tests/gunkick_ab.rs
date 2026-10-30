//! The server's gun-kick spring against retail's: three captures of
//! `--save-slope --probe-autofire --probe-weapon thompson_mp` from
//! `client-probes/probe_prone`'s street placement on mp_carentan, the
//! thompson held from the hip, held down the sight, then tapped, at a wall
//! about 1090 units out (`mp_carentan-dm-slope-8ms-gunkick-{1.1,1.5,uo}.txt`).
//! Each server ran a thompson file with [`EDITS`] applied: no spread, so a
//! round leaves along the aim block's direction, and the gun-kick ranges
//! pinned, so the spring's draw is the same on every server whatever its
//! `rand()` returns.
//!
//! On 1.5b and UO each sighted round climbs the wall: `G_PlayerEvent` kicks
//! the spring off every fire event and the next cmd's aim block adds it
//! down the sight. 1.1d's never kicks it, and its rounds stay on one spot
//! (docs/research/cod15-movement.md 6, cod11-combat.md 15.2). Ours runs the
//! same probe gametype, replays the capture's cmds at their own times, and
//! compares every impact in order.
//!
//! `GUNKICK_REPORT=1` prints both sides. Needs `COD_DIR`: the 1.1 capture
//! runs at a 1.1 install, the 1.5 one at a 1.5 or UO install (UO's `main/`
//! is 1.5's), the UO one at a UO install.

use crate::common;

use glam::Vec3;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::event_ids::{EV_BULLET_HIT_LARGE, EV_BULLET_HIT_SMALL, EV_UO_BULLET_HIT};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};
use vcod_common::pk3::Pk3Fs;

const MAP: &str = "mp_carentan";
const PROBE: &str = "probe_prone";
const WEAPON: &str = "thompson_mp";
/// `probe_prone`'s street spot.
const SPOT: [f32; 2] = [900.0, 1930.0];
/// What the captures' servers changed in the stock `weapons/mp/thompson_mp`
/// (keys a file lacks are left out; UO's adds the two stance sight spreads).
const EDITS: &[(&str, &str)] = &[
    ("adsSpread", "0"),
    ("adsSpreadProne", "0"),
    ("adsSpreadDucked", "0"),
    ("hipSpreadStandMin", "0"),
    ("hipSpreadDuckedMin", "0"),
    ("hipSpreadProneMin", "0"),
    ("hipSpreadMax", "0"),
    ("hipGunKickPitchMin", "-60"),
    ("hipGunKickPitchMax", "-60"),
    ("hipGunKickYawMin", "40"),
    ("hipGunKickYawMax", "40"),
    ("adsGunKickPitchMin", "-60"),
    ("adsGunKickPitchMax", "-60"),
    ("adsGunKickYawMin", "40"),
    ("adsGunKickYawMax", "40"),
];
/// How far an impact of ours may sit from retail's: both are snapped to
/// whole units, and the aim agrees to a few hundredths of a degree, under
/// half a unit at this range.
const TOL: f32 = 1.5;

/// The stock file with [`EDITS`] applied: `WEAPONFILE\key\value...`.
fn edited(stock: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(stock);
    let mut parts: Vec<String> = text.split('\\').map(str::to_string).collect();
    let mut i = 1;
    while i + 1 < parts.len() {
        if let Some((_, v)) = EDITS.iter().find(|(k, _)| *k == parts[i]) {
            parts[i + 1] = v.to_string();
        }
        i += 2;
    }
    parts.join("\\").into_bytes()
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn ints<const N: usize>(s: &str) -> [i32; N] {
    let v: Vec<i32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    std::array::from_fn(|i| v[i])
}

/// One `!cmd`: its serverTime, the view in absolute shorts (wire angles
/// plus the `delta_angles` of the snapshot before it) and the rest.
struct Cmd {
    st: i32,
    view: [i32; 3],
    cmd: UserCmd,
}

/// Impacts in the order they first showed, `(entity, at)` new against the
/// snapshot before.
#[derive(Default)]
struct Impacts {
    prev: Vec<(u32, [i32; 3])>,
    all: Vec<Vec3>,
}

impl Impacts {
    fn snapshot(&mut self, cur: Vec<(u32, [i32; 3])>) {
        for &(n, at) in &cur {
            if !self.prev.contains(&(n, at)) {
                self.all.push(Vec3::from(at.map(|c| c as f32)));
            }
        }
        self.prev = cur;
    }
}

/// What the capture holds: every cmd, each snapshot's `(serverTime,
/// commandTime)`, and the impacts.
struct Capture {
    cmds: Vec<Cmd>,
    snaps: Vec<(i32, i32)>,
    impacts: Vec<Vec3>,
}

fn retail(text: &str) -> Capture {
    let mut cmds = Vec::new();
    let mut snaps = Vec::new();
    let mut impacts = Impacts::default();
    let mut da = [0; 3];
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("!snap ") {
            da = ints(field(rest, "da").unwrap());
            let num = |k| field(rest, k).unwrap().parse::<i32>().unwrap();
            snaps.push((num("t"), num("ct")));
            let cur = field(rest, "imp")
                .map(|s| {
                    s.split(';')
                        .map(|e| {
                            let (n, at) = e.split_once(':').unwrap();
                            let at: Vec<f32> = at.split(',').map(|c| c.parse().unwrap()).collect();
                            (n.parse().unwrap(), std::array::from_fn(|i| at[i] as i32))
                        })
                        .collect()
                })
                .unwrap_or_default();
            impacts.snapshot(cur);
        } else if let Some(rest) = line.strip_prefix("!cmd ") {
            let angles: [i32; 3] = ints(field(rest, "angles").unwrap());
            let num = |k| field(rest, k).unwrap().parse::<i32>().unwrap();
            cmds.push(Cmd {
                st: num("st"),
                view: std::array::from_fn(|i| (angles[i] + da[i]) & 0xffff),
                cmd: UserCmd {
                    buttons: num("buttons") as u8,
                    wbuttons: num("wbuttons") as u8,
                    forward: num("forward") as i8,
                    right: num("right") as i8,
                    up: num("up") as i8,
                    ..NULL_USERCMD
                },
            });
        }
    }
    Capture {
        cmds,
        snaps,
        impacts: impacts.all,
    }
}

/// The bullet impacts on one of our snapshots.
fn our_impacts(p: &Protocol, s: &vcod_common::net::snapshot::Snapshot) -> Vec<(u32, [i32; 3])> {
    s.entities
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
        .collect()
}

/// The probe on ours: joined allies with the edited thompson, placed by the
/// gametype, held until ours' clock reaches the capture's first snapshot,
/// then each frame the cmds retail's snapshot of that frame had
/// acknowledged, at their own times: the sight's idle sway runs on the
/// level time, so both servers aim on the same one.
fn ours(p: &'static Protocol, mut fs: Pk3Fs, cap: &Capture) -> Vec<Vec3> {
    let path = format!("weapons/mp/{WEAPON}");
    let stock = fs.read(&path).expect("the stock thompson");
    fs.overlay(&path, edited(&stock));
    let src = format!("../gsc/tests/fixtures/semantics/client-probes/{PROBE}.gsc");
    let probe = std::fs::read_to_string(src).expect("read the probe");
    let bsp_path = fs.resolve_map(MAP).expect("the map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, PROBE), now);
    sv.set_protocol(p);
    sv.overlay_script(&format!("maps/mp/gametypes/{PROBE}"), &probe);
    sv.set_cvar("probe_teleport", "1");
    sv.set_cvar("probe_spot", "street");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = common::connect(&mut sv, &q, &mut now);
    let mut join = common::Join::new("allies", WEAPON);
    let weapon = vcod_server::defs::Defs::of(p).weapon_index(WEAPON).unwrap() as u8;
    let mut impacts = Impacts::default();
    let frame = |sv: &mut vcod_server::Server,
                 cl: &mut vcod_common::net::NetClient<common::ClientEnd>,
                 join: &mut common::Join,
                 now: Instant| {
        for e in common::step(sv, &q, cl, now) {
            if let vcod_common::net::NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, cl, now);
            }
        }
    };
    // Until the gametype has put the body on the spot, and a second more,
    // as retail's capture opens.
    let mut placed_at = None;
    for i in 0..400 {
        now += Duration::from_millis(common::FRAME_MS as u64);
        cl.send_frame(&UserCmd {
            weapon,
            ..NULL_USERCMD
        });
        frame(&mut sv, &mut cl, &mut join, now);
        let on_spot = cl.snapshots().newest().is_some_and(|s| {
            let o = s.ps.origin(p);
            s.ps.field_i32(p, "pm_type") == 0
                && (o[0] - SPOT[0]).abs() < 1.0
                && (o[1] - SPOT[1]).abs() < 1.0
                && s.ps.field_i32(p, "weapon") == weapon as i32
        });
        if on_spot && placed_at.is_none() {
            placed_at = Some(i);
        }
        if placed_at.is_some_and(|at| i >= at + 20) {
            break;
        }
    }
    assert!(placed_at.is_some(), "the probe never placed the client");

    let delta = |cl: &vcod_common::net::NetClient<common::ClientEnd>| {
        cl.snapshots().newest().map_or([0; 3], |s| {
            [0, 1, 2].map(|i| s.ps.field_i32(p, &format!("delta_angles[{i}]")))
        })
    };
    let first = cap.snaps.first().expect("a snapshot").0;
    let last = cap.snaps.last().unwrap().0;
    let mut next_cmd = 0;
    loop {
        let t = cl
            .snapshots()
            .newest()
            .map_or(0, |s| s.server_time + common::FRAME_MS as i32);
        if t > last {
            break;
        }
        if t < first {
            cl.send_frame(&UserCmd {
                weapon,
                ..NULL_USERCMD
            });
        } else {
            let ct = cap
                .snaps
                .iter()
                .take_while(|(st, _)| *st <= t)
                .last()
                .map_or(i32::MIN, |s| s.1);
            let da = delta(&cl);
            let mut batch = Vec::new();
            while let Some(c) = cap.cmds.get(next_cmd)
                && c.st <= ct
            {
                next_cmd += 1;
                batch.push(UserCmd {
                    server_time: c.st,
                    angles: std::array::from_fn(|i| (c.view[i] - da[i]) & 0xffff),
                    weapon,
                    ..c.cmd
                });
            }
            cl.send_cmds(&batch);
        }
        now += Duration::from_millis(common::FRAME_MS as u64);
        frame(&mut sv, &mut cl, &mut join, now);
        if let Some(s) = cl.snapshots().newest() {
            impacts.snapshot(our_impacts(p, s));
        }
    }
    impacts.all
}

/// The capture's impacts against ours, one for one. Returns how far the
/// sighted rounds climbed on retail, the highest less the lowest.
fn check(p: &'static Protocol, fs: Pk3Fs, tag: &str) -> f32 {
    let path = format!("tests/fixtures/playerstate/{MAP}-dm-slope-8ms-gunkick-{tag}.txt");
    let text = std::fs::read_to_string(&path).expect("read the fixture");
    let cap = retail(&text);
    let ours = ours(p, fs, &cap);
    let retail = cap.impacts;
    let report = std::env::var_os("GUNKICK_REPORT").is_some();
    let mut bad = Vec::new();
    for i in 0..retail.len().max(ours.len()) {
        let (r, o) = (retail.get(i), ours.get(i));
        let line = format!("{tag} round {i}: retail {r:?} ours {o:?}");
        if report {
            println!("{line}");
        }
        if !matches!((r, o), (Some(r), Some(o)) if r.distance(*o) <= TOL) {
            bad.push(line);
        }
    }
    assert!(bad.is_empty(), "rounds off retail's:\n{}", bad.join("\n"));
    let z = retail.iter().map(|v| v.z);
    z.clone().fold(f32::MIN, f32::max) - z.fold(f32::MAX, f32::min)
}

/// 1.1d's server never kicks: every round, sighted or not, on one spot.
#[test]
fn a_1_1_sighted_burst_stays_on_one_spot() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let climb = check(&PROTOCOL_V1, fs, "1.1");
    assert!(climb < 3.0, "{climb}");
}

/// 1.5b's sighted rounds climb with the spring.
#[test]
fn a_1_5_sighted_burst_climbs_with_the_gun_kick() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    let climb = check(&PROTOCOL_V6, fs, "1.5");
    assert!(climb > 50.0, "{climb}");
}

/// And United Offensive's.
#[test]
fn a_uo_sighted_burst_climbs_with_the_gun_kick() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let climb = check(&PROTOCOL_V22, fs, "uo");
    assert!(climb > 50.0, "{climb}");
}
