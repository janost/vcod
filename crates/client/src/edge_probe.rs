//! `--net-probe ADDR --probe-edge MODE`: the server's edge cases a joined
//! client never sees, measured from a spectator that keeps decoding where a
//! real client would stop. Every snapshot prints an `EDGE snap` line with its
//! arrival time, `serverTime`, `snapFlags`, `deltaNum`, the x of each
//! HUD element its playerstate carries and its mount fields, every server
//! command an `EDGE cmd` line, every OOB packet an `EDGE oob` line. Writes no
//! fixture; docs/research/cod11-server-handshake.md ("Zombie slots") and
//! docs/protocol-1.1.md ("Spectator") hold what it measured.

use std::time::{Duration, Instant};
use vcod_common::net::msg::{UserCmd, WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT};
use vcod_common::net::{NetClient, NetEvent, NetState};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum EdgeMode {
    /// Fly 5 s, send one last packet straight after a snapshot, then send
    /// nothing and print what arrives: `sv_timeout`'s drop, measured as the
    /// last snapshot's `serverTime` minus the one the last packet answered.
    Silence,
    /// Send `disconnect` in the first message after the gamestate, before
    /// any usercmd enters the world: a zombie that was never active.
    Zombie,
    /// Fly 5 s, then `disconnect` and keep decoding: a zombie that was.
    ZombieActive,
    /// Send a `cp` that cannot verify in the first message after the
    /// gamestate, no usercmd, and keep decoding: a pure server's
    /// `EXE_UNPURECLIENTDETECTED` drop of a client still loading.
    Unpure,
    /// Fly 5 s, then connect again from the same socket and qport with no
    /// `disconnect`: a crashed client coming back into the slot it still
    /// holds (`client-probes/probe_reconnect`).
    Reconnect,
    /// Hold each lean bit in turn on an otherwise idle spectator and print
    /// the velocity per snapshot, beside the cmds sent: `PM_FlyMove`'s lean
    /// terms (0x2ef19..0x2ef4a).
    LeanFly,
    /// Hold forward the whole run and print every snapshot's origin
    /// (`EDGE origin`): beside `client-probes/probe_specspawn`, whether a
    /// spectator spawn's think runs the client's last cmd
    /// (cod15-movement.md 4.2, "The spawn's lie-down").
    SpecFly,
    /// Stand idle and print an `EDGE ents` line whenever the snapshot's
    /// entity list changes, each entity as `num:eType`: what rides past the
    /// PVS cull (`client-probes/probe_playersound`).
    Watch,
}

/// The lean script: (start ms after the first snapshot, wbuttons) steps,
/// each held until the next.
const LEAN_STEPS: [(u64, u8); 7] = [
    (0, 0),
    (3000, WBUTTON_LEAN_RIGHT),
    (3500, 0),
    (6000, WBUTTON_LEAN_LEFT),
    (6500, 0),
    (9000, WBUTTON_LEAN_LEFT | WBUTTON_LEAN_RIGHT),
    (9500, 0),
];

/// Before the silence and the active zombie's disconnect.
const ACTIVE_FOR: Duration = Duration::from_secs(5);

pub fn run(addr: &str, mode: EdgeMode, secs: u64, cmd_ms: u64) -> anyhow::Result<()> {
    let mut client = NetClient::connect(addr)?;
    let mut reconnected = false;
    client.listen_after_drop();
    let start = Instant::now();
    let ms = |at: Instant| at.duration_since(start).as_millis();
    let mut first_snap: Option<Instant> = None;
    let mut printed: Option<u32> = None;
    // Silence: the newest snapshot the last packet went out after.
    let mut silent_after: Option<i32> = None;
    let mut disconnected = false;
    let mut last_seq = 0;
    let mut last_ents = String::new();
    let mut last_cmd = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if crate::quit::requested() {
            break;
        }
        let now = Instant::now();
        let mut fresh_snapshot = false;
        for e in client.pump_at(now) {
            match e {
                NetEvent::GamestateReady => {
                    println!(
                        "EDGE gamestate t={} serverId={}",
                        ms(now),
                        client.server_id()
                    );
                    if mode == EdgeMode::Zombie && !disconnected {
                        disconnect_and_listen(&mut client);
                        disconnected = true;
                        println!("EDGE disconnect t={} entered=no", ms(now));
                    }
                    if mode == EdgeMode::Unpure && !disconnected {
                        client.send_reliable("cp 1 2 @ 3 4");
                        client.send_cmds(&[]);
                        disconnected = true;
                        println!("EDGE cp t={}", ms(now));
                    }
                }
                NetEvent::OutOfBand(text) => println!("EDGE oob t={} {text:?}", ms(now)),
                NetEvent::Dropped(why) => println!("EDGE dropped t={} {why}", ms(now)),
                _ => {}
            }
        }
        if client.incoming_sequence() != last_seq {
            last_seq = client.incoming_sequence();
            println!("EDGE msg t={} seq={last_seq}", ms(now));
        }
        for c in client.take_server_commands() {
            println!("EDGE cmd t={} {c:?}", ms(now));
        }
        if let Some(newest) = client.snapshots().newest().map(|s| s.message_num) {
            let p = client.protocol();
            let from = printed.map_or(newest, |n| n + 1);
            for num in from..=newest {
                let Some(s) = client.snapshots().get(num) else {
                    continue;
                };
                fresh_snapshot = true;
                let f = |n: &str| f32::from_bits(s.ps.field_i32(p, n) as u32);
                println!(
                    "EDGE snap t={} msg={} serverTime={} flags={} delta={} ct={} \
                     origin_z={:.3} vel_z={} vel_z_bits={:08x} ents={} hud_x={} \
                     eflags={:#x} viewlocked={} viewlocked_ent={}",
                    ms(now),
                    s.message_num,
                    s.server_time,
                    s.snap_flags,
                    s.delta_num,
                    s.ps.field_i32(p, "commandTime"),
                    f("origin[2]"),
                    f("velocity[2]"),
                    s.ps.field_i32(p, "velocity[2]") as u32,
                    s.entities.len(),
                    hud_xs(&s.ps.arrays),
                    s.ps.field_i32(p, "eFlags"),
                    s.ps.field_i32(p, "viewlocked"),
                    s.ps.field_i32(p, "viewlocked_entNum"),
                );
                if mode == EdgeMode::SpecFly {
                    println!(
                        "EDGE origin serverTime={} ct={} origin={:.3},{:.3},{:.3}",
                        s.server_time,
                        s.ps.field_i32(p, "commandTime"),
                        f("origin[0]"),
                        f("origin[1]"),
                        f("origin[2]"),
                    );
                }
                if mode == EdgeMode::Watch {
                    let ents = s
                        .entities
                        .iter()
                        .map(|(n, e)| format!("{n}:{}", e.field_i32(p, "eType")))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if ents != last_ents {
                        println!("EDGE ents serverTime={} [{ents}]", s.server_time);
                        last_ents = ents;
                    }
                }
            }
            printed = Some(newest);
            first_snap.get_or_insert(now);
        }
        let active_for = first_snap.map_or(Duration::ZERO, |t| now.duration_since(t));
        let sending = client.state() == NetState::Active && !disconnected && silent_after.is_none();
        if sending && now.duration_since(last_cmd) >= Duration::from_millis(cmd_ms) {
            let mut cmd = UserCmd::default();
            if mode == EdgeMode::SpecFly {
                cmd.forward = 127;
            }
            if mode == EdgeMode::LeanFly {
                let t = active_for.as_millis() as u64;
                cmd.wbuttons = LEAN_STEPS
                    .iter()
                    .rev()
                    .find(|(at, _)| t >= *at)
                    .map_or(0, |(_, b)| *b);
            }
            if let Some(sent) = client.send_frame(&cmd)
                && mode == EdgeMode::LeanFly
            {
                println!(
                    "EDGE usercmd t={} serverTime={} wbuttons=0x{:02x}",
                    ms(now),
                    sent.server_time,
                    sent.wbuttons
                );
            }
            last_cmd = now;
        }
        if first_snap.is_some() && active_for >= ACTIVE_FOR && sending {
            match mode {
                // Straight after a snapshot, so the server stamps the
                // packet with that snapshot's frame time.
                EdgeMode::Silence if fresh_snapshot => {
                    client.send_frame(&UserCmd::default());
                    let t = client
                        .snapshots()
                        .newest()
                        .map_or(0, |s| s.server_time.raw());
                    silent_after = Some(t);
                    println!("EDGE silent t={} after_serverTime={t}", ms(now));
                }
                EdgeMode::ZombieActive => {
                    disconnect_and_listen(&mut client);
                    disconnected = true;
                    println!("EDGE disconnect t={} entered=yes", ms(now));
                }
                EdgeMode::Reconnect if !reconnected => {
                    client = client.reconnect(now);
                    reconnected = true;
                    printed = None;
                    last_seq = 0;
                    println!("EDGE reconnect t={}", ms(now));
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if let (Some(after), Some(last)) = (
        silent_after,
        client.snapshots().newest().map(|s| s.server_time),
    ) {
        println!(
            "EDGE silence: last snapshot serverTime {last}, {} ms past the last packet's frame",
            last - after
        );
    }
    if !disconnected {
        client.disconnect();
    }
    Ok(())
}

/// `disconnect` in three messages with no usercmd, as `CL_Disconnect` sends
/// it, but the client stays up and keeps decoding what the slot is sent.
fn disconnect_and_listen(client: &mut NetClient<vcod_common::net::UdpTransport>) {
    client.send_reliable("disconnect");
    for _ in 0..3 {
        client.send_cmds(&[]);
    }
}

/// The `x` of every element in both HUD arrays, archived first, comma
/// separated (`-` for none): which elements a reconnect kept.
fn hud_xs(a: &vcod_common::net::msg::PsArrays) -> String {
    let xs: Vec<String> = a
        .hud_archived
        .iter()
        .chain(&a.hud_current)
        .map(|e| e.get(vcod_common::net::msg::hud_field::X).to_string())
        .collect();
    if xs.is_empty() {
        "-".into()
    } else {
        xs.join(",")
    }
}
