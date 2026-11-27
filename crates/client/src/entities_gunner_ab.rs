//! The client's own gunner placement (0x300279b0, cod11-turrets.md 14.7)
//! against the server's on the ownline sweep (`probe_ownline.gsc`,
//! `crates/server/tests/ownline_ab.rs`): vcod-server in process runs the
//! capture's gunner on its clock and cmd times, a spectator beside the gun
//! watches him, and each snapshot the spectator gets is drawn at its own
//! server time. Needs `COD_DIR` at a 1.1 install; otherwise it returns early.

use super::*;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetEvent};

#[allow(clippy::all, unused)]
#[path = "../../server/tests/common/mod.rs"]
mod common;

/// The capture under the server's tests.
fn capture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../server/tests/fixtures/ownline/{name}",
        vcod_common::manifest_dir!()
    ))
    .expect("the capture")
}

/// The `buttons` of a `f "PROBE_IN <buttons> <wbuttons> <up>"` command.
fn direct_buttons(tokens: &[String]) -> Option<u8> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let mut t = tokens.get(1)?.strip_prefix("PROBE_IN ")?.split_whitespace();
    t.next()?.parse().ok()
}

/// One drawn frame of the mounted gunner: the server's origin (the
/// snapshot's), the client's placement off the legs clock and the same at
/// frame 0.
struct Drawn {
    t: i32,
    server: Vec3,
    client: Vec3,
    at_frame_0: Vec3,
}

/// The ownline gate's run of `name` (`ownline_ab::ours`: the gunner joined
/// `PLACED_MS` before the capture's `PROBE player` time, his cmds on its
/// `gunner CMDS` times), with a spectator connected once he is placed and
/// held over the gun; every mounted gunner the spectator's snapshots carry,
/// drawn as `build_instances` places him.
fn drawn(fs: Pk3Fs, name: &str, cvars: &[(&str, &str)]) -> Vec<Drawn> {
    const PLACED_MS: i64 = 800;
    let text = capture(name);
    let proto = vcod_server::server::install_protocol(&fs);
    let p: &Cod1Wire = proto;
    let (map, probe) = ("mp_carentan", "probe_ownline");
    let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map(map).unwrap()).unwrap()).unwrap();
    let world = CollisionWorld::build(&bsp, &[]);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, probe), now);
    sv.set_protocol(proto);
    let src = std::fs::read_to_string(format!(
        "{}/../gsc/tests/fixtures/semantics/client-probes/{probe}.gsc",
        vcod_common::manifest_dir!()
    ))
    .expect("the probe");
    sv.overlay_script(&format!("maps/mp/gametypes/{probe}"), &src);
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    let fs = Rc::new(fs);
    sv.load_scripts(fs.clone()).expect("load the scripts");
    let placed: i64 = text
        .lines()
        .find_map(|l| l.strip_prefix("PROBE player "))
        .and_then(|r| r.split_whitespace().nth(1)?.parse().ok())
        .expect("the capture's PROBE player time");
    common::run_alone(&mut sv, &mut now, placed - PLACED_MS);
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = common::Join::new("allies", "m1carbine_mp");
    let mut hold = common::ProbeHold {
        aim: true,
        framed: true,
        times: common::cmds_times(&text, "gunner"),
        ran: common::cmds_ran(&text, "gunner"),
        ..Default::default()
    };
    let mut spawn_vel: Option<[f32; 3]> = text
        .lines()
        .find_map(|l| l.strip_prefix("gunner SPAWNED "))
        .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("vel=")))
        .map(|v| {
            let v: Vec<f32> = v.split(',').map(|c| c.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        });
    let (mut buttons, mut selected) = (0u8, None::<u8>);
    let mut watcher: Option<(Rc<RefCell<common::Queues>>, NetClient<common::ClientEnd>)> = None;

    let anims = PlayerAnims::load(&*fs).expect("the player anims");
    let mut clips = HashMap::new();
    let mut weapon_cache = HashMap::new();
    let mut skeletons: HashMap<String, Option<(Vec3, Vec3)>> = HashMap::new();
    let mut clocks: HashMap<u32, BodyClocks> = HashMap::new();
    let mut last: Option<Snapshot> = None;
    // The gunner's own playerstate origin by server time: the server's
    // placement unsnapped, where his entity's `pos.trBase` is truncated.
    let mut own: HashMap<i32, Vec3> = HashMap::new();
    let mut out = Vec::new();
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for _ in 0..3000 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
        if live {
            let mut cmds = hold.cmds(&cl, buttons);
            for c in &mut cmds {
                if let Some(w) = selected {
                    c.weapon = w;
                }
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for d in pending {
            sv.handle_packet(common::ADDR, &d, now);
        }
        if let Some((wq, w)) = watcher.as_mut() {
            w.send_frame(&UserCmd::default());
            let pending: Vec<Vec<u8>> = wq.borrow_mut().to_server.drain(..).collect();
            for d in pending {
                sv.handle_packet(common::ADDR_B, &d, now);
            }
            // Over the gun, where the gunner stands in its PVS.
            sv.test_set_client_origin(1, [1700.0, 1800.0, 60.0]);
        }
        sv.tick(now);
        if spawn_vel.is_some()
            && sv
                .script_log()
                .iter()
                .any(|l| l.starts_with("PROBE player "))
        {
            sv.test_set_client_velocity(0, spawn_vel.take().unwrap());
            let wq = Rc::new(RefCell::new(common::Queues::default()));
            let w = NetClient::start_with_qport(common::ClientEnd(wq.clone()), now, 0x2002);
            watcher = Some((wq, w));
        }
        for (addr, d) in sv.take_outgoing() {
            match (&watcher, addr == common::ADDR_B) {
                (Some((wq, _)), true) => wq.borrow_mut().to_client.push_back(d),
                _ => q.borrow_mut().to_client.push_back(d),
            }
        }
        for e in cl.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if tokens.first().map(String::as_str) == Some("a") {
                    selected = tokens.get(1).and_then(|w| w.parse().ok());
                }
                if let Some(b) = direct_buttons(&tokens) {
                    buttons = b;
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if let Some(s) = cl.snapshots().newest() {
            own.insert(s.server_time.raw(), Vec3::from(s.ps.origin(cl.protocol())));
        }
        let Some((_, w)) = watcher.as_mut() else {
            continue;
        };
        w.pump_at(now);
        let Some(b) = w.snapshots().newest().cloned() else {
            continue;
        };
        if last
            .as_ref()
            .is_some_and(|l| l.server_time >= b.server_time)
        {
            continue;
        }
        let a = last.replace(b.clone()).unwrap_or_else(|| b.clone());
        let t = b.server_time.raw();
        let configstrings = w.configstrings();
        let weapon_names =
            split_weapon_list(configstrings.get(7).map(String::as_str).unwrap_or(""));
        let from = LerpFrom {
            snap: SnapView {
                snap: &a,
                own: None,
            },
            movers: SnapshotMovers::from_entities(p, &a.entities),
        };
        // `build_instances`' gun pass at f = 1.
        let mut guns = HashMap::new();
        for (&num, ent) in &b.entities {
            let EntityVisual::Turret { model, weapon } =
                resolve_visual(ent, &b.clients, configstrings, p)
            else {
                continue;
            };
            let tags = *skeletons.entry(model.clone()).or_insert_with(|| {
                let m = model::load(&*fs, &model).ok()?;
                aim_tags(&Skeleton::build(&[&m]))
            });
            let (pos, angles) = lerp_pos_angles(num, ent, &from, 1.0, t, p);
            let barrel = turret::barrel(
                None,
                ["angles2[0]", "angles2[1]", "angles2[2]"].map(|n| ent.field_f32(p, n)),
                1.0,
            );
            let rotate_inc = weapon_name_for_index(&weapon_names, weapon as i32)
                .and_then(|name| resolve_weapon_def(&mut weapon_cache, &*fs, name))
                .map_or(0.0, |d| d.anim_hor_rotate_inc);
            guns.insert(
                num,
                GunFrame {
                    pos,
                    rot: angles_quat(angles.to_array()),
                    barrel,
                    uo: None,
                    tag_weapon: tags.map(|(aim, weapon)| tag_weapon_local(aim, weapon, barrel)),
                    rotate_inc,
                },
            );
        }
        let rules = MoveRules::for_protocol(p);
        let at = Times {
            clock_ms: t,
            snapshot_ms: t,
        };
        let frametime_ms = t - a.server_time.raw();
        for (&num, ent) in &b.entities {
            if ent.field_i32(p, "eType") != ET_PLAYER {
                continue;
            }
            // The server's own origin in: the drawn one, off the snapped
            // `pos.trBase`, stands the body up to a unit higher and moves
            // the row blend with it.
            let pos = own.get(&t).copied().unwrap_or(Vec3::NAN);
            let before = clocks.get(&num).copied();
            let seconds = || gunner_legs_seconds(before, &anims, rules, ent, p, at, frametime_ms);
            let placed = place_body(&anims, ent, p, &guns, pos, seconds, &mut clips, &*fs);
            let at_frame_0 = place_body(&anims, ent, p, &guns, pos, || 0.0, &mut clips, &*fs);
            // The pose's own step, as the draw takes it after the placement.
            let mut c = before.unwrap_or((ChannelClock::default(), ChannelClock::default(), 0, t));
            let wires = (ent.field_i32(p, "legsAnim"), ent.field_i32(p, "torsoAnim"));
            let refs = (&mut c.0, &mut c.1, &mut c.2, &mut c.3);
            step_clocks(refs, &anims, rules, wires, &seen(ent, p), at, frametime_ms);
            clocks.insert(num, c);
            let (Some((g, gun_z)), Some((g0, _)), true) = (placed, at_frame_0, pos.is_finite())
            else {
                continue;
            };
            let mw = MoveWorld::bare(&world);
            out.push(Drawn {
                t,
                server: pos,
                client: trace_down(mw, num, g.origin, gun_z),
                at_frame_0: trace_down(mw, num, g0.origin, gun_z),
            });
        }
    }
    out
}

/// The firing sweep: the drawn gunner stands where vcod-server places him
/// (which `ownline_ab` holds to retail's on every frame) to a float step at
/// 2048, his leaves read at the legs clock's time; read at frame 0, the
/// `fire_30left` leaves' second root key puts frames off.
#[test]
fn a_drawn_firing_gunner_stands_where_the_server_places_him() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let frames = drawn(fs, "mp_carentan-1.1.txt", &[]);
    assert!(frames.len() > 300, "{} mounted frames drawn", frames.len());
    let off = |v: Vec3, w: Vec3| (v - w).abs().max_element();
    let rows: Vec<String> = frames
        .iter()
        .filter(|d| off(d.client, d.server) > 2.5e-4)
        .map(|d| format!("t={} client {} server {}", d.t, d.client, d.server))
        .collect();
    let frame_0 = frames
        .iter()
        .filter(|d| off(d.at_frame_0, d.server) > 2.5e-4)
        .count();
    println!(
        "{} mounted frames drawn, {frame_0} off at frame 0",
        frames.len()
    );
    assert!(
        rows.is_empty(),
        "{} of {} frames off\n{}",
        rows.len(),
        frames.len(),
        rows.join("\n")
    );
    assert!(frame_0 > 0, "no frame tells the clip time from frame 0");
}
