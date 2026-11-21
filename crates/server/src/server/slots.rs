//! Client slots: gamestate, server commands, drops, zombies, pings and timeouts.

use super::*;
use crate::game::commands::Recipient;
use vcod_common::net::Msec;

/// `SV_UpdateServerCommandsToClient`. The caller bounds `from_ack` to
/// `0..=reliable_sequence`, so the range is empty or inside the ring.
pub(super) fn write_pending_commands(w: &mut MsgWriter, nc: &ServerNetchan, from_ack: i32) {
    for seq in (from_ack + 1)..=(nc.reliable_sequence as i32) {
        msg::write_server_command(
            w,
            seq,
            &nc.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)],
        );
    }
}

/// `SV_WriteDownloadToClient`'s blocks for `c`'s transfer, if it has one:
/// as many as the rate carries in a snapshot interval, or the www redirect.
/// A refusal ends it.
pub(super) fn write_download(
    c: &mut Client,
    w: &mut MsgWriter,
    now: i32,
    rate: i32,
    open: &impl Fn(&str) -> Result<crate::download::DownloadFile, crate::download::Refusal>,
    www: Option<&crate::download::WwwConfig>,
) {
    let budget = crate::download::blocks_per_message(rate, c.snapshot_msec());
    let wants_www = c.wants_www();
    if let Some(dl) = c.download.as_mut()
        && !dl.write(w, now, budget, open, www, wants_www, &mut c.www)
    {
        c.download = None;
    }
}

/// A zombie's message (`SV_SendClientSnapshot` 0x808f844): its unacked
/// server commands, then the ring slot the new sequence lands on.
/// `SV_BuildClientSnapshot` (0x808f130) zeroes that slot's entity and client
/// counts for a zombie and copies no playerstate, and
/// `SV_WriteSnapshotToClient` (0x808dee0) deltas only for an active client,
/// so the snapshot is the playerstate of 32 messages back, written from
/// null, with no entities and no clients (handshake doc, "Zombie slots").
fn zombie_message(
    c: &mut Client,
    sv_time_ms: i32,
    snap_flags: u32,
    proto: &'static Cod1Wire,
    huff: &Huffman,
) -> Vec<Vec<u8>> {
    let mut w = MsgWriter::new(huff);
    write_pending_commands(&mut w, &c.link.netchan, c.link.reliable_ack);
    c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
    let message_num = c.link.netchan.outgoing_sequence;
    let ps = c.link.frames[message_num as usize % crate::client::SV_PACKET_BACKUP]
        .as_ref()
        .map_or_else(|| msg::PlayerState::null(proto), |f| f.ps.clone());
    let frame = snapshot::SentSnapshot {
        server_time: Msec::from_raw(sv_time_ms),
        message_num,
        delta_num: -1,
        snap_flags,
        ps,
        entities: Default::default(),
        clients: Default::default(),
        valid: true,
    };
    proto.write_snapshot(&mut w, None, &frame, frame.head(None), &HashMap::new());
    c.link.unsent.clear();
    c.link
        .netchan
        .transmit(c.link.last_client_command, &w.into_ops(), huff)
}

impl Server {
    /// `SV_SendClientGameState`.
    pub(super) fn send_gamestate(&mut self, slot: usize) {
        let is_bot = self.core.clients[slot].as_ref().is_some_and(|c| c.is_bot);
        let (dedicated, max_rate) = (self.game.dedicated, self.live_int_cvar("sv_maxRate"));
        let svs_ms = self.svs_time_ms();
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        c.state = ClientState::Primed;
        // The client restarts its usercmd chain on every gamestate it
        // receives, so the base it deltas against restarts here too.
        c.last_cmd = NULL_USERCMD;
        c.delta_message = -1;
        c.gamestate_message_num = i64::from(c.link.netchan.outgoing_sequence);
        let mut w = MsgWriter::new(&self.core.huff);
        write_pending_commands(&mut w, &c.link.netchan, c.link.reliable_ack);
        c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
        let gs = Gamestate {
            configstrings: self.core.configstrings.clone(),
            baselines: self.game.baselines.clone(),
            client_num: slot as i32,
            checksum_feed: self.core.checksum_feed,
            server_command_sequence: c.link.netchan.reliable_sequence as i32,
        };
        self.game.proto.write_gamestate(&mut w, &gs);
        let ops = w.into_ops();
        log::info!("client {slot}: gamestate, {} bytes", ops.len());
        let max = if self.game.proto.is_uo() {
            MAX_GAMESTATE_BYTES_UO
        } else {
            MAX_GAMESTATE_BYTES
        };
        if ops.len() > max {
            // Retail's message buffer would cut it here, unchecked.
            log::warn!(
                "client {slot}: gamestate is {} bytes, past retail's {max}",
                ops.len()
            );
        }
        // A bot pulls its own gamestate in step_bots and reads nothing back.
        if is_bot {
            return;
        }
        // The fragments left of an earlier message all go first, in this
        // turn (0x8085f34).
        self.core
            .outbox
            .extend(c.link.unsent.drain(..).map(|pkt| (c.link.addr, pkt)));
        c.stamp_sent(c.link.netchan.outgoing_sequence, svs_ms);
        let rate = c.send_rate(dedicated, max_rate);
        let sent = c
            .link
            .netchan
            .transmit(c.link.last_client_command, &ops, &self.core.huff);
        let msec = c.snapshot_msec();
        c.pace(svs_ms, sent.iter().map(Vec::len).sum(), rate, msec);
        // The first fragment now, the rest one per turn as any message's.
        if let Some(pkt) = c.first_packet(sent) {
            self.core.outbox.push((c.link.addr, pkt));
        }
    }

    /// `SV_GameSendServerCommand`: `cmd` to one client or to every one.
    pub(super) fn send_to(&mut self, to: Recipient, cmd: &str) {
        match to {
            Recipient::All => self.send_to_all(cmd),
            Recipient::One(slot) => self.send_server_command(slot.index(), cmd),
        }
    }

    /// `SV_AddServerCommand` (cod_lnxded 0x808b680): queued, squashed or
    /// dropped by [`Client::queue_server_command`], and sent with the
    /// client's next snapshot or gamestate. A client whose acks fall a whole
    /// ring behind has stopped consuming reliables; overwriting an unacked
    /// slot would desync both ends' scramble keys, so that is fatal, with
    /// retail's `EXE_SERVERCOMMANDOVERFLOW` (docs/protocol-1.1.md, "The
    /// server command queue").
    pub(super) fn send_server_command(&mut self, slot: usize, cmd: &str) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        match c.queue_server_command(cmd) {
            Queued::Overflow => self.drop_client(slot, "EXE_SERVERCOMMANDOVERFLOW"),
            Queued::Dropped => log::debug!("client {slot}: dropped {cmd:?}"),
            Queued::Added | Queued::Replaced => {}
        }
    }

    /// `SV_DropClient` (cod_lnxded 0x8085cf4). The game's `ClientDisconnect`
    /// runs inside it: on 1.1 after the drop notice, on 1.5 and UO before it
    /// (cod11-server-handshake.md, "When the game hears a drop"). The slot
    /// goes `CS_ZOMBIE` with the drop notice queued, and its slot's turn
    /// keeps sending it until `check_timeouts` frees it, [`ZOMBIE_TIME`]
    /// past the client's last packet.
    /// A bot has nobody to tell and is freed now.
    pub(super) fn drop_client(&mut self, slot: usize, reason: &str) {
        let Some(mut c) = self.core.clients[slot].take() else {
            return;
        };
        let v15 = self.v15();
        let mut own = Vec::new();
        if v15 {
            own = self.client_disconnect(slot);
        }
        // The first slot with the client's address (0x8085cf4).
        if !c.is_bot {
            self.core.challenges.release(c.link.addr);
        }
        log::debug!("Going to CS_ZOMBIE for {}", c.name);
        // Everyone else is told unless the client left on its own.
        if reason != "EXE_DISCONNECTED" {
            let notice = format!("e \"\u{15}{}^7 \u{14}{reason}\"", c.name);
            for other in 0..self.core.clients.len() {
                if self.core.clients[other]
                    .as_ref()
                    .is_some_and(|o| o.state != ClientState::Connected)
                {
                    self.send_server_command(other, &notice);
                }
            }
        }
        self.print(&format!("{slot}:{} {reason}\n", c.name));
        if !v15 {
            own = self.client_disconnect(slot);
        }
        // What the callback sent the leaver alone goes into its ring, and
        // the zombie's `w` squashes every unsent ignorable command, the
        // callback's `iPrintLn` with them: a `setClientCvar` reaches the
        // zombie on all three, a print on none (`client-probes/probe_reconnect`).
        for cmd in own {
            c.queue_server_command(&cmd);
        }
        c.squash_ignorable();
        // Appended with no guard: the notice must not be dropped, and a full
        // ring cannot drop a client twice.
        c.link.netchan.reliable_sequence += 1;
        let at = c.link.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1);
        c.link.netchan.reliable[at] =
            vcod_common::net::codec::client_read_string(&format!("w \"{reason}\"")).into_owned();
        c.reliable_kept[at] = true;
        if c.is_bot {
            // The slot is free at once, so its brain goes now: a client that
            // takes the slot before the bot step would be driven by it.
            self.game.module.driver.forget(slot);
        } else {
            c.zombie = true;
            self.game.zombies[slot] = Some(c);
        }
        // The last client gone (0x8085ebc).
        if self.client_count() == 0 {
            self.core.masters.force();
        }
    }

    /// The game's `ClientDisconnect` for a slot already out of `clients`,
    /// whose server commands and configstrings go out at once, as the
    /// game's `trap_SendServerCommand` sends them: a drop after the frame
    /// still reaches that frame's message. Neither the zombie nor, on 1.5
    /// and UO, the client still in its slot hears the callback's lines
    /// (`client-probes/probe_dropclient`). Returns the commands addressed
    /// to the leaver's own slot, for its zombie.
    pub(super) fn client_disconnect(&mut self, slot: usize) -> Vec<String> {
        let collision = self.game.world.as_ref().map(|w| &w.collision);
        // 1.5's and UO's `ClientDisconnect` stops a follower outright
        // (1.5b 0x4cf5f, UO 0x55433); 1.1's cycles first.
        let cycle = !self.v15();
        pass_followers_on(
            &mut self.core.clients,
            slot,
            self.game.module.script.as_mut(),
            collision,
            cycle,
        );
        let Some(rt) = self.game.module.script.as_mut() else {
            return Vec::new();
        };
        mirror_roster(&self.core.clients, rt);
        rt.client_disconnect(slot);
        let written: Vec<usize> = rt
            .host
            .configstrings
            .writes()
            .iter()
            .map(|(i, _)| *i)
            .collect();
        let commands = rt.take_client_commands();
        let console = rt.take_console();
        self.core.console.extend(console);
        self.copy_game_configstrings(written);
        let mut own = Vec::new();
        for (to, cmd) in commands {
            match to {
                Recipient::One(s) if s.index() == slot => own.push(cmd),
                to => self.send_to(to, &cmd),
            }
        }
        own
    }

    /// A zombie's packet still runs the netchan and records the acks, so a
    /// drop notice the client has seen stops being resent; nothing in it is
    /// executed and it does not hold the slot open (0x808ca44).
    pub(super) fn zombie_packet(&mut self, slot: usize, pkt: &[u8]) {
        let Some(z) = self.game.zombies[slot].as_mut() else {
            return;
        };
        let Some(m) = z.link.netchan.process_in(pkt) else {
            return;
        };
        if (0..z.link.netchan.outgoing_sequence as i32).contains(&m.message_ack) {
            z.message_ack = m.message_ack;
        }
        if z.link.reliable_ack_in_window(m.reliable_ack) {
            z.link.reliable_ack = m.reliable_ack;
        }
    }

    /// What `SV_WriteDownloadToClient` (0x8086290) opens a download with:
    /// the pak's open file, or the refusal. Only paks this server mounts and lists in
    /// `sv_referencedPakNames` go out. Retail's `FS_SV_FOpenFileRead`
    /// opens any path under its game directories, `..` included, so a
    /// `download main/server.cfg` hands out the rcon password there
    /// (docs/research/cod11-server-handshake.md, "Serving a download").
    pub(super) fn download_opener(
        &self,
    ) -> impl Fn(&str) -> Result<crate::download::DownloadFile, crate::download::Refusal> + use<>
    {
        let allow = self
            .level_cvar("sv_allowDownload")
            .is_none_or(|v| v.trim().parse::<i32>().unwrap_or(0) != 0);
        let pure = self.game.paks.pure;
        let fs = self.game.fs.clone();
        // No stock-pak refusal: retail's `FS_idPak` test gets the name with
        // its `.pk3` and never matches, so both retail servers send a stock
        // pak a client asks for (handshake doc, "Serving a download").
        move |name: &str| {
            use crate::download::Refusal;
            let stem = name.strip_suffix(".pk3").unwrap_or(name);
            if !allow {
                return Err(Refusal::Disabled { pure });
            }
            fs.as_deref()
                .and_then(|fs| {
                    fs.paks()
                        .find(|p| p.qualified_name().eq_ignore_ascii_case(stem))
                })
                .and_then(|p| crate::download::DownloadFile::open(&p.path))
                .filter(|f| !f.is_empty())
                .ok_or(Refusal::NotFound)
        }
    }

    /// `sv_wwwDownload`, `sv_wwwBaseURL` and `sv_wwwDlDisconnected`, read
    /// per message as retail reads them (UO 0x808c559); `None` with the
    /// first off or on protocol 1, which has no redirect.
    pub(super) fn www_config(&self) -> Option<crate::download::WwwConfig> {
        (self.v15() && self.live_int_cvar("sv_wwwDownload") != 0).then(|| {
            crate::download::WwwConfig {
                base_url: self.live_cvar("sv_wwwBaseURL", ""),
                disconnected: self.live_int_cvar("sv_wwwDlDisconnected") != 0,
                udp_fallback: !self.game.proto.is_uo(),
            }
        })
    }

    /// `SV_SendClientSnapshot` (0x808f844) for a client not in the world:
    /// retail writes the server commands and the snapshot only for an
    /// active client (or a zombie), so the message is the header and the
    /// download blocks, or the bare header (9 bytes on the wire) for a
    /// client that is not downloading. It goes out on the slot's own turn
    /// once its `nextSnapshotTime` comes round, as every message does
    /// ([`Client::pace`]: every frame on a LAN, else at least a second
    /// apart while not downloading); a client in the world gets its blocks
    /// after its snapshot in [`Self::send_client_snapshot`].
    pub(super) fn send_unentered(&mut self, slot: usize, snaps: &Cod1Snapshots) {
        let (dedicated, now) = (self.game.dedicated, self.svs_time_ms());
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        let rate = c.send_rate(dedicated, snaps.max_rate);
        let mut w = MsgWriter::new(&self.core.huff);
        write_download(c, &mut w, now, rate, &snaps.open, snaps.www.as_ref());
        c.stamp_sent(c.link.netchan.outgoing_sequence, now);
        let sent =
            c.link
                .netchan
                .transmit(c.link.last_client_command, &w.into_ops(), &self.core.huff);
        let msec = c.snapshot_msec();
        c.pace(now, sent.iter().map(Vec::len).sum(), rate, msec);
        if let Some(pkt) = c.first_packet(sent) {
            self.core.outbox.push((c.link.addr, pkt));
        }
    }

    /// `SV_SendClientSnapshot` for a zombie (0x808f844), on its slot's
    /// turn: its unacked server commands, the drop notice among them, and
    /// the stale ring slot ([`zombie_message`]). Paced like any message:
    /// off the LAN a zombie, not being `CS_ACTIVE`, waits a second between
    /// them (`docs/research/cod11-server-handshake.md`, "Zombie slots").
    pub(super) fn send_zombie(&mut self, slot: usize, max_rate: i32) {
        let (dedicated, now, game_ms) = (
            self.game.dedicated,
            self.svs_time_ms(),
            self.core.sv_time_ms,
        );
        let snap_flags = self.game.snap_flag_server_bit;
        let Some(z) = self.game.zombies[slot].as_mut() else {
            return;
        };
        let not_active = if z.link.was_active {
            0
        } else {
            SNAPFLAG_NOT_ACTIVE
        };
        let flags = snap_flags | u32::from(z.link.rate_delayed) | not_active;
        let sent = zombie_message(z, game_ms, flags, self.game.proto, &self.core.huff);
        let rate = z.send_rate(dedicated, max_rate);
        let msec = z.snapshot_msec();
        z.pace(now, sent.iter().map(Vec::len).sum(), rate, msec);
        self.core
            .outbox
            .extend(sent.into_iter().map(|p| (z.link.addr, p)));
    }

    /// `SV_FinalMessage` (0x808ad8c), which `SV_Shutdown` runs before the
    /// process exits: twice over, every client past `CS_ZOMBIE` is queued
    /// `e "<message>"` and a bare `w` and sent a snapshot carrying them,
    /// built fresh as `SV_SendClientSnapshot` builds every one: the clock
    /// has not moved, the playerstate has (handshake doc, Shutdown).
    /// Retail thought every cmd it parsed before the shutdown on arrival,
    /// so the ones vcod still holds for the next tick run first.
    /// The `e` is dropped for a client not yet active.
    pub(super) fn final_message(&mut self, message: &str) {
        let notice = format!("e \"{message}\"");
        let moved = self.replay_moves();
        let snaps = self.snapshots_begin(moved, (None, 0));
        for _ in 0..2 {
            for slot in 0..self.core.clients.len() {
                if self.core.clients[slot].as_ref().is_none_or(|c| c.is_bot) {
                    continue;
                }
                self.send_server_command(slot, &notice);
                self.send_server_command(slot, "w");
                self.send_client_snapshot(slot, &snaps);
            }
        }
    }

    /// `SV_CalcPings` (0x808cab8), once a frame after the packets and before
    /// the clock moves. Only an active client in the world is measured; a
    /// test client acks nothing and reads 999 (retail's `status`), vcod's
    /// own bots, which retail does not have, 0 as Q3's do.
    pub(super) fn calc_pings(&mut self) {
        for (slot, c) in self.core.clients.iter_mut().enumerate() {
            let Some(c) = c else { continue };
            c.link.ping = if c.is_bot && self.game.module.driver.bots.contains_key(&slot) {
                0
            } else if c.is_bot || c.state != ClientState::Active || c.sim.is_none() {
                999
            } else {
                c.calc_ping()
            };
        }
    }

    /// `SV_CheckTimeouts` (0x808cbc0) for the clients in a slot, after the
    /// frame's clock advance and game frame. A last-packet time ahead of the
    /// clock is pulled back to it; one more than `sv_timeout` seconds old
    /// counts a frame, a fresh one resets the count, and the sixth stale
    /// frame in a row drops the client and frees the slot at once, its
    /// notice never sent. A zombie whose last packet is more than
    /// `sv_zombietime` old goes `CS_FREE` first, before any message.
    pub(super) fn check_timeouts(&mut self) {
        let now = self.svs_time_ms();
        let zombie_ms = ZOMBIE_TIME.as_millis() as i32;
        for z in &mut self.game.zombies {
            if let Some(c) = z.take_if(|c| now.wrapping_sub(c.link.last_packet_ms) > zombie_ms) {
                log::debug!("Going from CS_ZOMBIE to CS_FREE for {}", c.name);
            }
        }
        let timeout = self.live_cvar("sv_timeout", TIMEOUT_DEFAULT);
        let timeout_ms = crate::game::builtins::cvar::atoi(&timeout).wrapping_mul(1000);
        let mut stale = Vec::new();
        for (slot, c) in self.core.clients.iter_mut().enumerate() {
            let Some(c) = c.as_mut().filter(|c| !c.is_bot) else {
                continue;
            };
            if now.wrapping_sub(c.link.last_packet_ms) < 0 {
                c.link.last_packet_ms = now;
            }
            if now.wrapping_sub(c.link.last_packet_ms) > timeout_ms {
                c.link.timeout_count += 1;
                if c.link.timeout_count > 5 {
                    stale.push(slot);
                }
            } else {
                c.link.timeout_count = 0;
            }
        }
        for slot in stale {
            self.drop_client(slot, "EXE_TIMEDOUT");
            self.game.zombies[slot] = None;
        }
    }

    /// The `Client` behind an `addTestClient` slot, whose `ClientConnect` and
    /// `ClientBegin` the builtin already ran: `CS_ACTIVE` at once, a
    /// spectator sim at the origin `ClientConnect`'s zeroing leaves, the
    /// userinfo `SV_AddTestClient` connects with (0x808759b) and no
    /// netchan. It sends no cmds and no brain drives it.
    pub(super) fn install_test_client(
        clients: &mut [Option<Client>],
        slot: usize,
        port: u16,
        name: String,
        proto: &'static Cod1Wire,
        sv_time_ms: i32,
    ) {
        let Some(entry) = clients.get_mut(slot) else {
            return;
        };
        let userinfo = format!(
            "\\cg_predictItems\\1\\cl_anonymous\\0\\handicap\\100\\color\\4\\head\\default\\model\\multi\\snaps\\20\\rate\\5000\\name\\{name}\\protocol\\{}\\ip\\bot",
            proto.transport.version
        );
        let addr = SocketAddr::from(([127, 0, 0, 1], 65000 - slot as u16));
        let mut client = Client::new(addr, 0, 0, userinfo, Instant::now());
        client.is_bot = true;
        client.bot_port = Some(port);
        client.state = ClientState::Active;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, [0; 3]);
        sim.ps.rules = vcod_common::pmove::MoveRules::for_protocol(proto);
        client.sim = Some(sim);
        client.last_processed_st = sv_time_ms;
        client.last_usercmd_st = sv_time_ms;
        log::info!("client {slot} {name:?} connected (test client)");
        *entry = Some(client);
    }

    /// One slot's `StuckInClient` view: `None` for a free slot, `own_view`
    /// false for a connected client with no sim yet, a spectator or an
    /// intermission client.
    pub(super) fn stuck_view(c: &Option<Client>) -> Option<StuckView> {
        let client = c.as_ref()?;
        let Some(sim) = client.sim.as_ref() else {
            return Some(StuckView {
                own_view: false,
                playing: false,
                health: 0,
                contents: 0,
                origin: glam::Vec3::ZERO,
                mins: glam::Vec3::ZERO,
                maxs: glam::Vec3::ZERO,
                vel_xy: glam::Vec2::ZERO,
                speed: 0.0,
                seated: false,
            });
        };
        Some(StuckView {
            own_view: sim.pm_type == PmType::Normal,
            playing: sim.pm_type == PmType::Normal && !sim.dead,
            health: sim.health,
            contents: sim.contents,
            origin: sim.ps.origin,
            mins: sim.ps.mins(),
            maxs: sim.ps.maxs(),
            vel_xy: sim.ps.velocity.truncate(),
            speed: sim.ps.speed,
            seated: sim.seat.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// The disconnect callback's commands to the leaver alone go into its
    /// ring, and the zombie's `w` squashes the unsent prints: retail's
    /// zombie gets the `setClientCvar` and not the `iPrintLn` on all three
    /// (`client-probes/probe_reconnect`).
    #[test]
    fn a_leavers_zombie_keeps_the_callbacks_reliable_commands_only() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        for proto in [&PROTOCOL_V1, &PROTOCOL_V6] {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(proto);
            install_script(
                &mut sv,
                ScriptRuntime::for_test_at(
                    CALLBACK_SETUP,
                    "main() {}\n\
                     CodeCallback_PlayerDisconnect() { self iprintln(\"self\"); \
                       self setclientcvar(\"probe_self\", 1); }\n",
                ),
            );
            begun(&mut sv, now);
            sv.tick(now);
            sv.take_outgoing();
            sv.send_server_command(0, "e \"unsent print\"");
            sv.drop_client(0, "EXE_DISCONNECTED");
            let z = sv.game.zombies[0].as_ref().expect("a zombie");
            let ring: Vec<&str> = (z.link.reliable_sent + 1
                ..=z.link.netchan.reliable_sequence as i32)
                .map(|i| z.link.netchan.reliable[i as usize & (MAX_RELIABLE_COMMANDS - 1)].as_str())
                .collect();
            assert_eq!(
                ring,
                ["v probe_self \"1\"", "w \"EXE_DISCONNECTED\""],
                "{}",
                proto.transport.version
            );
        }
    }

    /// `SV_SendClientGameState` (0x8085eec): fragments left of an earlier
    /// message go first, all at once; the gamestate's first fragment follows
    /// and the rest go one per frame, as retail sent a loopback probe
    /// (cod11-server-handshake.md, "The gamestate goes out fragmented").
    #[test]
    fn the_gamestate_goes_out_one_fragment_per_frame() {
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // Enough configstrings to pass one fragment.
        for i in 0..40 {
            sv.core.configstrings[vcod_common::net::protocol::CS_SOUNDS_V1 + 1 + i] =
                format!("sound/misc/{i:03}_{}", "x".repeat(40));
        }
        let mut nc = connected(&mut sv, addr(5), now);
        let stale = vec![vec![1u8; 8], vec![2u8; 8]];
        sv.core.clients[0].as_mut().unwrap().link.unsent = stale.clone().into();
        let pkt = nc.build_out(0, 0, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        let out: Vec<Vec<u8>> = sv.take_outgoing().into_iter().map(|(_, p)| p).collect();
        assert_eq!(
            out.len(),
            3,
            "the stale fragments, then one of the gamestate"
        );
        assert_eq!(out[..2], stale[..]);
        let fragment = |p: &[u8]| u32::from_le_bytes(p[..4].try_into().unwrap()) & (1 << 31) != 0;
        assert!(fragment(&out[2]));
        let mut whole = nc.process_in(&out[2], &huff).unwrap();
        let mut frames = 0;
        while whole.is_none() {
            sv.tick(now + Duration::from_millis(50 * (frames + 1)));
            let out = sv.take_outgoing();
            assert_eq!(out.len(), 1, "one fragment a frame");
            whole = nc.process_in(&out[0].1, &huff).unwrap();
            frames += 1;
        }
        assert!(frames >= 1);
        assert!(sv.core.clients[0].as_ref().unwrap().link.unsent.is_empty());
    }

    #[test]
    fn a_client_before_its_gamestate_gets_bare_messages() {
        // `SV_SendClientMessages` (0x809045c): a `CS_CONNECTED` client gets
        // the header and `svc_EOF`, 9 bytes, every frame on a LAN and a
        // second apart off it (0x808f817).
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let wan = SocketAddr::from(([203, 0, 113, 9], 27960));
        let mut lan_nc = connected(&mut sv, addr(5), now);
        let mut wan_nc = connected(&mut sv, wan, now);
        let huff = Huffman::new();
        let (mut lan_n, mut wan_n) = (0, 0);
        for i in 0..22 {
            sv.tick(now + Duration::from_millis(50 * i));
            for (to, pkt) in sv.take_outgoing() {
                assert_eq!(pkt.len(), 9, "a bare message");
                let nc = if to == wan { &mut wan_nc } else { &mut lan_nc };
                let msg = nc.process_in(&pkt, &huff).unwrap().unwrap();
                let mut r = MsgReader::new(&msg[4..], &huff);
                assert_eq!(r.read_byte(), msg::SVC_EOF, "no commands, no snapshot");
                *if to == wan { &mut wan_n } else { &mut lan_n } += 1;
            }
        }
        assert_eq!(lan_n, 22);
        assert_eq!(wan_n, 2, "a keepalive at 0 and at 1000 ms");
    }

    /// `SV_CheckTimeouts` (0x808cbc0): after the clock advance, a last
    /// packet more than `sv_timeout` seconds old counts a frame, a fresh one
    /// resets the count, and the sixth stale frame in a row drops the client
    /// with no zombie. A last-packet time ahead of the clock is pulled back.
    #[test]
    fn a_silent_client_is_dropped_on_its_sixth_stale_frame() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("sv_timeout", "1");
        let _nc = active(&mut sv, t);
        sv.take_outgoing();
        let last = sv.core.sv_time_ms;
        sv.core.clients[0].as_mut().unwrap().link.last_packet_ms = last + 400;
        sv.tick(t);
        assert_eq!(
            sv.core.clients[0].as_ref().unwrap().link.last_packet_ms,
            last + 50
        );
        let stale_at = |sv: &Server| {
            sv.core.sv_time_ms - sv.core.clients[0].as_ref().unwrap().link.last_packet_ms
        };
        while stale_at(&sv) <= 1000 {
            sv.tick(t);
        }
        assert_eq!(sv.core.clients[0].as_ref().unwrap().link.timeout_count, 1);
        // From a fresh count, the sixth stale frame drops.
        sv.core.clients[0].as_mut().unwrap().link.last_packet_ms = sv.core.sv_time_ms - 1001;
        sv.core.clients[0].as_mut().unwrap().link.timeout_count = 0;
        for _ in 0..5 {
            sv.tick(t);
            assert_eq!(sv.client_count(), 1);
        }
        sv.tick(t);
        assert_eq!(sv.client_count(), 0);
        assert!(sv.game.zombies[0].is_none());
    }

    /// `SV_DropClient` runs the game's `ClientDisconnect` inside the drop, so
    /// a timeout after the frame still puts the callback's line in that
    /// frame's message: after the drop notice on 1.1, ahead of it on 1.5 and
    /// UO (`client-probes/probe_dropclient`, handshake doc "When the game
    /// hears a drop").
    #[test]
    fn a_timed_out_clients_callback_lines_go_out_with_its_drop_notice() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        for (proto, gone_first) in [(&PROTOCOL_V1, false), (&PROTOCOL_V6, true)] {
            let t = Instant::now();
            let mut sv = Server::new(cfg(), t);
            sv.set_protocol(proto);
            install_script(
                &mut sv,
                ScriptRuntime::for_test_at(
                    CALLBACK_SETUP,
                    "main() {}\n\
                     CodeCallback_PlayerDisconnect() { iprintln(\"gone\"); }\n",
                ),
            );
            sv.set_cvar("sv_timeout", "1");
            let mut watcher = begun(&mut sv, t);
            sv.core.clients[1] = Some(Client::new(addr(6), 0x3001, 0, "\\name\\quiet".into(), t));
            sv.game
                .module
                .script
                .as_mut()
                .unwrap()
                .push_client_event(ClientEvent::Connect {
                    slot: 1,
                    name: "quiet".into(),
                });
            sv.tick(t);
            sv.take_outgoing();
            let huff = Huffman::new();
            sv.core.clients[1].as_mut().unwrap().link.last_packet_ms = sv.core.sv_time_ms - 1001;
            sv.core.clients[1].as_mut().unwrap().link.timeout_count = 5;
            sv.core.clients[0].as_mut().unwrap().link.last_packet_ms = sv.core.sv_time_ms;
            sv.tick(t);
            assert_eq!(sv.client_count(), 1);
            let out = sv.take_outgoing();
            let (_, pkt) = out.iter().find(|(to, _)| *to == addr(5)).unwrap();
            let cmds = server_commands(&mut watcher, pkt, &huff);
            let notice = "e \"\u{15}quiet^7 \u{14}EXE_TIMEDOUT\"".to_string();
            let gone = "f \"gone\"".to_string();
            let want = if gone_first {
                [gone, notice]
            } else {
                [notice, gone]
            };
            assert!(
                cmds.ends_with(&want),
                "{}: {cmds:?}",
                proto.transport.version
            );
        }
    }

    /// `clientkick` drops through `SV_DropClient`: the slot is a zombie that
    /// resends the notice every frame until it is acked, shows as `ZMBI`,
    /// and is freed `sv_zombietime` after the kick, not at it.
    #[test]
    fn a_kicked_client_lingers_as_a_zombie() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "secret");
        let mut nc = active(&mut sv, t);
        sv.take_outgoing();
        let huff = Huffman::new();
        assert_eq!(
            rcon(&mut sv, "secret clientkick 0", t),
            ["print\n0:vcod EXE_PLAYERKICKED\n"]
        );
        assert_eq!(sv.client_count(), 0);
        sv.tick(t);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1);
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(
            cmds.last().map(String::as_str),
            Some("w \"EXE_PLAYERKICKED\"")
        );

        let t1 = t + Duration::from_millis(500);
        let status = rcon(&mut sv, "secret status", t1).concat();
        assert!(
            status.contains("  0     0 ZMBI                     500 127.0.0.1:5 "),
            "{status}"
        );

        // A different address finds no free slot while the zombie holds 0.
        sv.tick(t1);
        sv.take_outgoing();
        let seq = sv.game.zombies[0]
            .as_ref()
            .unwrap()
            .link
            .netchan
            .reliable_sequence as i32;
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, seq, &ack_ops(), &huff).unwrap(),
            t1,
        );
        sv.tick(t1);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "the zombie still gets frames");
        assert!(
            server_commands(&mut nc, &out[0].1, &huff).is_empty(),
            "the ack stopped the resend"
        );
        // Its packets do not keep it alive. The kick stamped the clock
        // between frames; the slot goes on the first frame more than
        // `sv_zombietime` past it, before that frame's message. Three
        // frames have run since the kick.
        for frame in 4..=40 {
            sv.tick(t1);
            assert!(sv.game.zombies[0].is_some(), "frame {frame}");
            sv.take_outgoing();
        }
        sv.tick(t1);
        assert!(sv.game.zombies[0].is_none());
        assert!(sv.take_outgoing().is_empty());
    }

    /// Off the LAN a zombie is paced as a client out of the world: a
    /// second between messages. Live, retail sent a disconnected probe's
    /// zombie a message 1000 ms apart where vcod had sent one a frame
    /// (cod11-server-handshake.md, "Zombie slots").
    #[test]
    fn an_off_lan_zombie_gets_a_message_a_second() {
        let huff = Huffman::new();
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "secret");
        let wan = SocketAddr::from(([203, 0, 113, 9], 27960));
        let mut nc = active_at(&mut sv, wan, t);
        let ack = nc.incoming_sequence as i32;
        let ops = move_ops(sv.core.checksum_feed, ack, NULL_USERCMD);
        let pkt = nc
            .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
            .unwrap();
        sv.handle_packet(wan, &pkt, t);
        assert_eq!(
            sv.core.clients[0].as_ref().unwrap().state,
            ClientState::Active
        );
        sv.tick(t);
        sv.take_outgoing();
        let kick = t + Duration::from_millis(1000);
        rcon(&mut sv, "secret clientkick 0", kick);
        let mut sent = Vec::new();
        for i in 0..36 {
            sv.tick(kick + Duration::from_millis(50 * i));
            sent.extend(sv.take_outgoing().into_iter().map(|_| 50 * i));
        }
        assert_eq!(sent, [0, 1000], "a zombie's messages off the LAN");
    }

    /// A zombie's snapshot is the ring slot its sequence lands on, written
    /// whole: the playerstate of 32 messages back, no entities and no
    /// clients (handshake doc, "Zombie slots").
    #[test]
    fn a_zombie_gets_the_stale_ring_slot_uncompressed() {
        let p = &PROTOCOL_V1;
        let huff = Huffman::new();
        let mut c = Client::new(addr(5), 1, 7, String::new(), Instant::now());
        let commandtime = |n: i32| {
            let mut ps = msg::PlayerState::null(p);
            ps.fields[msg::PlayerState::field_index(p, "commandTime").unwrap()] = n;
            ps
        };
        for n in 1..=40u32 {
            let mut e = msg::EntityState::null(p);
            e.number = 100;
            c.record_frame(snapshot::SentSnapshot {
                server_time: Msec::from_raw(50 * n as i32),
                message_num: n,
                delta_num: -1,
                snap_flags: 0,
                ps: commandtime(n as i32),
                entities: [(100, Rc::new(e))].into(),
                clients: Rc::new([(0, msg::ClientState::null(p))].into()),
                valid: true,
            });
        }
        c.link.netchan.outgoing_sequence = 41;
        let pkts = zombie_message(&mut c, 2050, 4, p, &huff);
        let mut nc = Netchan::new(1, 7);
        nc.incoming_sequence = 40;
        let msgbytes = nc.process_in(&pkts[0], &huff).unwrap().unwrap();
        let mut r = MsgReader::new(&msgbytes[4..], &huff);
        assert_eq!(r.read_byte(), SVC_SNAPSHOT);
        let mut ring = SnapshotRing::new();
        let z = ring.parse_into(&mut r, p, 41).unwrap();
        assert_eq!(z.delta_num, -1, "written whole");
        assert!(z.entities.is_empty() && z.clients.is_empty());
        assert_eq!(z.ps, commandtime(41 - 32));
    }

    /// `SNAPFLAG_NOT_ACTIVE` rides a zombie's snapshot only when the client
    /// was never sent one as an active client (0x808dfc3..0x808dfe3).
    #[test]
    fn a_zombie_never_active_carries_snapflag_not_active() {
        let p = &PROTOCOL_V1;
        let huff = Huffman::new();
        let zombie_flags = |entered: bool| {
            let t = Instant::now();
            let mut sv = Server::new(cfg(), t);
            let mut nc = if entered {
                begun(&mut sv, t)
            } else {
                active(&mut sv, t)
            };
            sv.tick(t);
            for (_, pkt) in sv.take_outgoing() {
                let _ = nc.process_in(&pkt, &huff);
            }
            sv.drop_client(0, "EXE_DISCONNECTED");
            sv.tick(t);
            let out = sv.take_outgoing();
            let msg = nc.process_in(&out[0].1, &huff).unwrap().unwrap();
            let mut r = MsgReader::new(&msg[4..], &huff);
            loop {
                match r.read_byte() {
                    msg::SVC_SERVER_COMMAND => {
                        r.read_long();
                        r.read_big_string();
                    }
                    SVC_SNAPSHOT => break,
                    b => panic!("unexpected {b}"),
                }
            }
            let mut ring = SnapshotRing::new();
            ring.parse_into(&mut r, p, nc.incoming_sequence)
                .unwrap()
                .snap_flags
        };
        assert_eq!(
            zombie_flags(false) & SNAPFLAG_NOT_ACTIVE,
            SNAPFLAG_NOT_ACTIVE
        );
        assert_eq!(zombie_flags(true) & SNAPFLAG_NOT_ACTIVE, 0);
    }

    /// A zombie of the same peer is a reconnect into its own slot, and a
    /// stranger gets the next free one.
    #[test]
    fn a_zombie_slot_is_held_from_strangers_but_not_its_own_client() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        let _nc = active(&mut sv, t);
        sv.drop_client(0, "EXE_DISCONNECTED");
        let _ = connected(&mut sv, SocketAddr::from(([10, 0, 0, 2], 5)), t);
        assert!(sv.core.clients[0].is_none() && sv.core.clients[1].is_some());
        let t2 = t + RECONNECT_LIMIT + Duration::from_millis(100);
        let _ = connected(&mut sv, addr(5), t2);
        assert!(sv.core.clients[0].is_some() && sv.game.zombies[0].is_none());
    }

    /// `SV_CalcPings`: the round trip each acked message took, on the
    /// server's frame clock, averaged into `status`, `getstatus` and the
    /// scoreboard.
    #[test]
    fn ping_is_the_mean_ack_time_over_the_ring() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        let mut nc = begun(&mut sv, t);
        let huff = Huffman::new();
        let mut last = NULL_USERCMD;
        // Each frame's snapshot is acked one frame later: 50 ms apiece.
        for i in 0..40 {
            sv.tick(t);
            for (_, pkt) in sv.take_outgoing() {
                let _ = nc.process_in(&pkt, &huff);
            }
            last.server_time = 100 + i * 50;
            let ack = nc.incoming_sequence as i32;
            sv.tick(t);
            sv.take_outgoing();
            let ops = move_ops(sv.core.checksum_feed, ack, last);
            let pkt = nc
                .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
                .unwrap();
            sv.handle_packet(addr(5), &pkt, t);
        }
        sv.tick(t);
        assert_eq!(sv.core.clients[0].as_ref().unwrap().link.ping, 50);
        sv.take_outgoing();
        sv.handle_packet(addr(7), &oob("getstatus x"), t);
        let (_, body) = reply_text(&mut sv);
        assert!(body.ends_with("\n0 50 \"vcod\""), "{body}");
    }

    #[test]
    fn an_ack_is_stamped_with_the_frame_sent_before_it_arrived() {
        // Each ack of frame i reaches the socket after frame i went out and
        // is read after frame i + 1. Every other one arrives before frame
        // i + 1 went out (0 ms), the rest after it (50 ms).
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut sv = Server::new(cfg(), t);
        let mut nc = begun(&mut sv, t);
        let huff = Huffman::new();
        let mut last = NULL_USERCMD;
        let mut clock = 0;
        for i in 0..40 {
            sv.tick(t);
            clock += 50;
            sv.frame_sent(ms(clock));
            for (_, pkt) in sv.take_outgoing() {
                let _ = nc.process_in(&pkt, &huff);
            }
            last.server_time = 100 + i * 50;
            let ack = nc.incoming_sequence as i32;
            sv.tick(t);
            clock += 50;
            sv.frame_sent(ms(clock));
            sv.take_outgoing();
            let ops = move_ops(sv.core.checksum_feed, ack, last);
            let pkt = nc
                .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
                .unwrap();
            let arrived = if i % 2 == 0 {
                ms(clock - 1)
            } else {
                ms(clock + 1)
            };
            let at = crate::engine::Arrival {
                now: t,
                arrived,
                read: arrived,
            };
            sv.packet_read(addr(5), &pkt, at);
        }
        sv.tick(t);
        assert_eq!(sv.core.clients[0].as_ref().unwrap().link.ping, 25);
    }
}
