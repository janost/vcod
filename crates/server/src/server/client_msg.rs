//! A client's packets: the message parse, its commands and its moves.

use super::*;

/// The last script menu `Cmd_MenuResponse_f` (0x486d8) will look up, and the
/// last `GScr_GetScriptMenuIndex` (0x5c73c) will hand out: both walk
/// `CsRange::Menu`'s 32 slots.
const MAX_MENUS: i64 = 31;

/// UO's `Cmd_FlameDamageClient_f` (game.mp.uo 0x4ee2c) on `fdc`'s
/// arguments: exactly one, read into a 4-byte buffer (so its first three
/// characters) and `atoi`'d, then kept when it lies in `0..=maxclients`
/// (`level+0x1e4`, `g_maxclients`). A bad one leaves the last report alone
/// (coduo-diff.md 21).
fn fdc_inflictor(args: &str, max_clients: usize) -> Option<u32> {
    let mut words = args.split_whitespace();
    let (Some(word), None) = (words.next(), words.next()) else {
        return None;
    };
    let head: String = word.chars().take(3).collect();
    u32::try_from(crate::game::builtins::cvar::atoi(&head))
        .ok()
        .filter(|&n| n as usize <= max_clients)
}

/// `Cmd_MenuResponse_f` (0x486d8) on `mr <serverId> <menuIndex> <response>`,
/// tokenized as `Cmd_TokenizeString` does (quotes group). The menu and the
/// response the `menuresponse` notify carries, or `None` for a stale
/// serverId, the one shape retail drops (gsc language doc 6):
///
/// - any argument count but four is the menu `""` and the response `"bad"`,
///   which is what a client's `mr <index> noop` and `mr <index> bad` reach;
/// - an index `strtol` reads past 31 (unsigned, so a negative one too) names
///   the menu by its own text;
/// - otherwise the menu is the slot's configstring, empty if unset.
pub(crate) fn parse_menu_response(cmd: &str, server_id: i32) -> Option<(MenuRef, String)> {
    let args = crate::game::say::tokenize(cmd);
    let [_, sid, index, response] = args.as_slice() else {
        return Some((MenuRef::Name(String::new()), "bad".to_string()));
    };
    if strtol(sid) != i64::from(server_id) {
        return None;
    }
    let menu = match strtol(index) {
        i @ 0..=MAX_MENUS => MenuRef::Index(i as usize),
        _ => MenuRef::Name(index.clone()),
    };
    Some((menu, response.clone()))
}

/// glibc `strtol(s, NULL, 10)` on the i386 `long`: leading whitespace, a
/// sign, the digits that follow, saturated; 0 when there are none.
fn strtol(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let v = if neg { -v } else { v };
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX))
}

impl Server {
    /// The netchan half of `SV_PacketEvent`, then `SV_ExecuteClientMessage`.
    /// The slot changes only once the message has passed every check the
    /// plain header and the op stream allow (`Client::accept`), so a packet
    /// forged with the client's ip and qport cannot stall it behind a huge
    /// sequence, redirect its replies or keep a dead slot alive.
    pub(super) fn handle_client_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        let sv_time_ms = self.svs_time_ms();
        if pkt.len() < 6 {
            log::debug!("{from}: netchan packet too short ({} bytes)", pkt.len());
            return;
        }
        let qport = u16::from_le_bytes([pkt[4], pkt[5]]);
        let peer = |c: &Option<Client>| {
            c.as_ref().is_some_and(|c| {
                !c.is_bot && c.link.addr.ip() == from.ip() && c.link.netchan.qport == qport
            })
        };
        let Some(slot) = self.core.clients.iter().position(peer) else {
            match self.game.zombies.iter().position(peer) {
                Some(slot) => self.zombie_packet(slot, pkt),
                None => log::debug!("{from}: netchan packet from no client (qport {qport})"),
            }
            return;
        };
        let Some(c) = self.core.clients[slot].as_ref() else {
            return;
        };
        let Some(m) = c.link.netchan.process_in(pkt) else {
            return;
        };
        // messageAcknowledge names a message we sent, so it is below
        // outgoing_sequence; retail only rejects a negative one.
        if m.message_ack < 0 || m.message_ack >= c.link.netchan.outgoing_sequence as i32 {
            log::debug!(
                "client {slot}: messageAcknowledge {} out of range",
                m.message_ack
            );
            return;
        }
        // A ring behind takes every command as acknowledged and drops the
        // message, its sequence kept (CoDMP.exe 0x45ac4a..0x45ac65,
        // docs/protocol-1.1.md, "Message headers"); one past what we sent
        // passes.
        if c.link.reliable_ack_behind(m.reliable_ack) {
            log::debug!(
                "client {slot}: reliableAcknowledge {} out of range",
                m.reliable_ack
            );
            self.core.clients[slot]
                .as_mut()
                .unwrap()
                .link
                .take_ack_clamp(&m);
            return;
        }

        if m.server_id != self.game.server_id {
            let c = self.core.clients[slot].as_mut().unwrap();
            if m.server_id & 0xf0 != self.game.server_id & 0xf0 {
                // A map change from the client's view (a fresh client, serverId
                // 0, looks the same). Resend the gamestate once its ack is past
                // the last one. Until snapshots exist the gamestate is our only
                // message, so a lost one never advances the ack and the client
                // falls to its own timeout.
                if i64::from(m.message_ack) > c.gamestate_message_num {
                    c.accept(&m, now, sv_time_ms);
                    self.send_gamestate(slot);
                }
            } else if c.state == ClientState::Primed {
                // Restart path; retail promotes to CS_ACTIVE here. No
                // usercmd came with it, so entry has no entering cmd.
                c.accept(&m, now, sv_time_ms);
                self.enter_world(slot, None);
            }
            return;
        }

        let base = c.last_cmd;
        let Some((ops, last_cmd)) = self.parse_client_ops(c, &m, base) else {
            log::debug!("client {slot}: message from {from} does not decode");
            return;
        };
        // The new delta base commits before any op runs; a message that fails
        // to decode left it alone above.
        self.core.clients[slot].as_mut().unwrap().last_cmd = last_cmd;
        let c = self.core.clients[slot].as_mut().unwrap();
        c.accept(&m, now, sv_time_ms);
        c.link.addr = from; // NAT may move the port; the qport is the identity
        self.game.packet_seq += 1;
        let mut commands_done = false;
        for op in ops {
            if !commands_done && matches!(op, ClientOp::Move { .. }) {
                commands_done = true;
                if !self.pure_after_commands(slot) {
                    return;
                }
            }
            match op {
                ClientOp::Command { seq, text } => {
                    if !self.client_command(slot, seq, text) {
                        return;
                    }
                }
                ClientOp::Move { cmds, delta } => {
                    // `SV_UserMove` sets `deltaMessage` before anything else.
                    let c = self.core.clients[slot].as_mut().unwrap();
                    c.delta_message = if delta { c.message_ack } else { -1 };
                    self.user_move(slot, cmds);
                }
            }
        }
        if !commands_done {
            self.pure_after_commands(slot);
        }
    }

    /// `SV_ExecuteClientMessage`'s op walk, read to the end before any op is
    /// applied. `None` when the stream does not decode: an overflow anywhere,
    /// a bad usercmd count or more commands than `ServerNetchan::max_commands`,
    /// which is what a forged block looks like once the scramble key is
    /// wrong. On success the new delta base comes back
    /// with the ops: the last cmd decoded, for the next move message to
    /// chain from.
    fn parse_client_ops(
        &self,
        c: &Client,
        m: &ClientMessage,
        base: UserCmd,
    ) -> Option<(Vec<ClientOp>, UserCmd)> {
        let mut r = MsgReader::new(&m.ops, &self.core.huff);
        let mut ops = Vec::new();
        let mut prev = base;
        loop {
            let op = r.read_bits(CLC_BITS);
            if r.is_overflowed() {
                return None;
            }
            match op {
                CLC_CLIENT_COMMAND => {
                    let seq = r.read_long();
                    let text = r.read_server_string();
                    if r.is_overflowed() || ops.len() == c.link.netchan.max_commands() {
                        return None;
                    }
                    ops.push(ClientOp::Command { seq, text });
                }
                CLC_EOF => return Some((ops, prev)),
                CLC_MOVE | CLC_MOVE_NO_DELTA => {
                    let cmds = self.parse_move(c, &mut r, m, &mut prev)?;
                    ops.push(ClientOp::Move {
                        cmds,
                        delta: op == CLC_MOVE,
                    });
                    return Some((ops, prev));
                }
                // `read_bits(CLC_BITS)` yields 0..=3; unreachable.
                _ => return None,
            }
        }
    }

    /// `SV_UserMove`'s parse: the whole block, in order, so the sim can
    /// replay every cmd like retail's pmove does. `prev` enters as the
    /// client's stored delta base and leaves as the last cmd of the block.
    fn parse_move(
        &self,
        c: &Client,
        r: &mut MsgReader,
        m: &ClientMessage,
        prev: &mut UserCmd,
    ) -> Option<Vec<UserCmd>> {
        let count = r.read_byte();
        if !(1..=MAX_PACKET_USERCMDS).contains(&count) {
            return None;
        }
        let cmd = &c.link.netchan.reliable[m.reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)];
        let key = self.core.checksum_feed ^ m.message_ack ^ com_hash_key(cmd, 32);
        let mut out = Vec::with_capacity(count as usize);
        for _ in 0..count {
            match read_delta_usercmd_in(r, self.game.proto, key, prev) {
                Ok(next) => {
                    out.push(next);
                    *prev = next;
                }
                Err(e) => {
                    log::debug!("usercmd not parsed: {e}");
                    return None;
                }
            }
        }
        Some(out)
    }

    /// `SV_ClientCommand`. Returns false when the client is gone.
    pub(super) fn client_command(&mut self, slot: usize, seq: i32, s: String) -> bool {
        let svs_ms = self.svs_time_ms();
        let Some(c) = self.core.clients[slot].as_mut() else {
            return false;
        };
        if seq <= c.link.last_client_command {
            return true;
        }
        if seq > c.link.last_client_command.saturating_add(1) {
            self.drop_client(slot, "EXE_LOSTRELIABLECOMMANDS");
            return false;
        }
        // Split rather than slice; the command may arrive with leading whitespace.
        let trimmed = s.trim_start();
        let (word, args) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        // `sv_floodProtect`, which the systeminfo advertises as 1. The three
        // exemptions are prefix compares with the space, so a bare `score`
        // is not one of them and opens the window like any other command;
        // that is what dropped every other `kill` the round-restart probe
        // sent within 800 ms of its own `score`. Engine commands run either
        // way, only the game's dispatch is skipped.
        let exempt = ["team ", "score ", "mr "]
            .iter()
            .any(|p| trimmed.starts_with(p));
        let mut client_ok = true;
        if !exempt {
            if c.state == ClientState::Active && svs_ms < c.link.next_reliable_ms {
                client_ok = false;
                log::debug!("client text ignored for {}: {trimmed}", c.name);
            }
            c.link.next_reliable_ms = svs_ms.wrapping_add(FLOOD_WINDOW_MS);
        }
        let engine_command = matches!(
            word,
            "disconnect"
                | "userinfo"
                | "cp"
                | "vdr"
                | "download"
                | "nextdl"
                | "stopdl"
                | "donedl"
                | "retransdl"
                | "wwwdl"
        );
        if !client_ok && !engine_command {
            c.link.last_client_command = seq;
            c.link.netchan.last_client_command_string = s;
            return true;
        }
        match word {
            "disconnect" => {
                self.drop_client(slot, "EXE_DISCONNECTED");
                return false;
            }
            // `SV_VerifyPaks_f` and `SV_ResetPureClient_f`, run whatever
            // `sv_pure` says; only the drops read it.
            "cp" => {
                let ok = self
                    .game
                    .pure_check
                    .as_ref()
                    .is_some_and(|p| p.verify(trimmed));
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.pure = if ok {
                        crate::client::Pure::Authentic
                    } else {
                        crate::client::Pure::Unpure
                    };
                }
            }
            "vdr" => {
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.pure = crate::client::Pure::Unchecked;
                }
            }
            // The download ucmds (cod_lnxded 0x8087a64, 0x8086168, 0x8087960,
            // 0x80879fc, 0x8087a2c).
            "download" => {
                let name = args.split_whitespace().next().unwrap_or("");
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.download = Some(crate::download::ServerDownload::new(name));
                }
            }
            "nextdl" => {
                let block = args.trim().parse::<i32>().unwrap_or(0);
                let now = self.svs_time_ms();
                let Some(c) = self.core.clients[slot].as_mut() else {
                    return false;
                };
                use crate::download::NextDl;
                match c.download.as_mut().map(|d| d.next_dl(block, now)) {
                    Some(NextDl::Completed) => c.download = None,
                    Some(NextDl::Acked) | None => {}
                    Some(NextDl::Broken) => {
                        self.drop_client(slot, "broken download");
                        return false;
                    }
                }
            }
            "stopdl" => {
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.download = None;
                }
            }
            "donedl" => {
                if !self.core.clients[slot]
                    .as_ref()
                    .is_some_and(Client::gamestate_last)
                {
                    self.send_gamestate(slot);
                }
            }
            // `SV_WWWDownload` (UO 0x808c298), the redirect's replies.
            "wwwdl" => {
                let sub = args.split_whitespace().next().unwrap_or("");
                let uo = self.game.proto.is_uo();
                let Some(c) = self.core.clients[slot].as_mut() else {
                    return false;
                };
                // 1.5b's redirect clears the name it sent (0x808b81e), so its
                // `chkfail` warning prints ''; UO's keeps it
                // (cod15-protocol-diff.md 14).
                let name = match &c.download {
                    Some(d) if uo => d.name.as_str(),
                    _ => "",
                };
                use crate::download::WwwReply;
                match crate::download::wwwdl(&mut c.www, sub, &c.name, name) {
                    WwwReply::None => {}
                    WwwReply::Close => c.download = None,
                    WwwReply::CloseAndGamestate => {
                        c.download = None;
                        if !c.gamestate_last() {
                            self.send_gamestate(slot);
                        }
                    }
                    WwwReply::Drop(reason) => {
                        self.drop_client(slot, reason);
                        return false;
                    }
                }
            }
            "retransdl" => {
                let block = args.trim().parse::<i32>().unwrap_or(0);
                if let Some(d) = self.core.clients[slot]
                    .as_mut()
                    .and_then(|c| c.download.as_mut())
                {
                    d.retransmit(block);
                }
            }
            // The entity's `.name` is not updated with it: script sees the
            // connect-time name and a rename goes stale there, which stage
            // 6's obituaries and scoreboard are the first to notice.
            "userinfo" => {
                let ui = args.trim().trim_matches('"').to_string();
                if let Some(c) = self.core.clients[slot].as_mut() {
                    if let Some(name) = info_value_for_key(&ui, "name") {
                        c.name = sanitize_name(name);
                    }
                    c.link.userinfo = ui;
                }
            }
            // `Cmd_Say_f` (0x47050): nothing without an argument. 1.5 adds
            // `say_squad` (0x434b7 with mode 3).
            "say" | "say_team" | "say_squad" if word != "say_squad" || self.v15() => {
                let mode = match word {
                    "say" => SayMode::All,
                    "say_team" => SayMode::Team,
                    _ => SayMode::Squad,
                };
                if let Some(text) = crate::game::say::concat_args(args) {
                    self.say(slot, None, mode, &text);
                }
            }
            // `Cmd_Tell_f` (0x47210): to a client in use, then the speaker's
            // own copy.
            "tell" => {
                let (to, text) = args
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .unwrap_or((args.trim(), ""));
                if let Ok(to) = to.parse::<usize>()
                    && self.core.clients.get(to).is_some_and(Option::is_some)
                {
                    let text = crate::game::say::concat_args(text).unwrap_or_default();
                    self.log_tell(slot, to, &text);
                    self.say(slot, Some(to), SayMode::Tell, &text);
                    self.say(slot, Some(slot), SayMode::Tell, &text);
                }
            }
            // DeathmatchScoreboardMessage (.so 0x459c0); grammar in
            // docs/research/cod11-hud-protocol.md section 3.
            "score" => {
                let text = self.scoreboard();
                self.send_server_command(slot, &text);
            }
            // `Cmd_Kill_f`: the same death `self suicide()` gives, asked for
            // by the client. The retail hit capture's death half is this
            // command (combat doc, section 8). `SV_ExecuteClientMessage`
            // (0x80872ec) runs a packet's client commands ahead of its
            // usercmds, so the move pass runs it ahead of the cmds that came
            // with it (combat doc, 9.2).
            "kill" => {
                let packet = self.game.packet_seq;
                if let Some(c) = self.core.clients[slot].as_mut()
                    && !c.commands_at.iter().any(|(_, k)| *k == PacketCommand::Kill)
                {
                    c.queue_command(packet, PacketCommand::Kill);
                }
            }
            // 1.5's `ClientCommand` (0x46341) runs both past its
            // intermission test (cod15-protocol-diff.md, section 12).
            "matchtimeout" | "matchtimein" if self.v15() => {
                self.match_timeout(slot, word == "matchtimein");
            }
            // `Cmd_CallVote_f` and `Cmd_Vote_f`, past `ClientCommand`'s
            // intermission return (docs/research/cod11-votes.md).
            "callvote" | "vote" => self.vote_command(slot, trimmed),
            // `Cmd_NextVehSlot_f` / `Cmd_PrevVehSlot_f` (game.mp.uo 0x4ed9b,
            // 0x4ede1): a living rider changes seats, in the move pass
            // after the cmds of every earlier packet, so the seat's
            // `SetClientViewAngle` bases `delta_angles` on the last of them.
            "nextvehslot" | "prevvehslot" if self.game.proto.is_uo() => {
                let packet = self.game.packet_seq;
                let back = word == "prevvehslot";
                if let Some(c) = self.core.clients[slot].as_mut() {
                    c.queue_command(packet, PacketCommand::CycleSeat { back });
                }
            }
            "fdc" if self.game.proto.is_uo() => {
                let now = self.core.sv_time_ms;
                match fdc_inflictor(args, self.core.clients.len()) {
                    Some(n) => {
                        if let Some(c) = self.core.clients[slot].as_mut() {
                            c.flame = Some((n, now));
                        }
                    }
                    None => log::debug!("client {slot}: bad fdc {args:?}"),
                }
            }
            // `Cmd_MenuResponse_f`: the client answering a menu `openMenu`
            // opened, or giving one up (`noop`, `bad`). Unlike the entry
            // notify this fires straight through: nothing is armed by it,
            // and a notify no thread is parked on is simply lost, which is
            // what retail does too. It runs in the move pass, in packet
            // order with `kill` (`client-probes/probe_threadorder`).
            "mr" => {
                if let Some((menu, response)) =
                    parse_menu_response(trimmed, i32::from(self.game.server_id))
                {
                    let packet = self.game.packet_seq;
                    if let Some(c) = self.core.clients[slot].as_mut() {
                        c.queue_command(packet, PacketCommand::MenuResponse(menu, response));
                    }
                }
            }
            other => log::debug!("client {slot}: command {other:?} ignored"),
        }
        let Some(c) = self.core.clients[slot].as_mut() else {
            return false;
        };
        c.link.last_client_command = seq;
        c.link.netchan.last_client_command_string = s;
        true
    }

    /// `SV_UserMove` past the parse: queue the block for the next tick's
    /// replay. A flood past the cap drops the oldest cmds, not the newest.
    pub(super) fn user_move(&mut self, slot: usize, cmds: Vec<UserCmd>) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        // The first usercmd after a gamestate is what puts a client in the
        // world; `begin` is not a CoD client command. docs/protocol-1.1.md,
        // "Entering the world".
        if c.state == ClientState::Primed {
            let first = cmds[0];
            self.enter_world(slot, Some(&first));
        }
        // `SV_UserMove` (0x8086fa4) drops a client a pure server has had no
        // `cp` from, after the entry and before any cmd runs.
        if self.game.paks.pure
            && self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| !c.is_bot && c.pure == crate::client::Pure::Unchecked)
        {
            self.drop_client(slot, "EXE_CANNOTVALIDATEPURECLIENT");
            return;
        }
        let packet = self.game.packet_seq;
        let acked = self.game.ack_time_ms.unwrap_or(self.svs_time_ms());
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        c.stamp_acked(acked);
        c.pending
            .extend(cmds.into_iter().map(|cmd| QueuedCmd { packet, cmd }));
        let excess = c.pending.len().saturating_sub(MAX_PENDING_CMDS);
        c.pending.drain(..excess);
    }

    /// `SV_ExecuteClientMessage` (0x80872ec) once a message's client
    /// commands ran: a pure server drops a client whose `cp` failed. False
    /// when it did.
    fn pure_after_commands(&mut self, slot: usize) -> bool {
        let unpure = self.game.paks.pure
            && self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.pure == crate::client::Pure::Unpure);
        if unpure {
            self.drop_client(slot, "EXE_UNPURECLIENTDETECTED");
        }
        !unpure
    }

    /// [`scoreboard`] for this server's clients.
    pub(super) fn scoreboard(&mut self) -> String {
        scoreboard(&self.core.clients, self.game.module.script.as_mut())
    }

    /// `Cmd_MatchTimeout_f` or `Cmd_MatchTimein_f`, inside the packet as
    /// retail runs it: the configstrings go out as `trap_SetConfigstring`
    /// sends them, ahead of the `e` lines, and the `timescale` write
    /// reaches the cvar mirror at the next frame's flush.
    pub(super) fn match_timeout(&mut self, slot: usize, timein: bool) {
        let intermission = self.core.clients[slot]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .is_some_and(|s| s.pm_type == crate::spectate::PmType::Intermission);
        let now = self.svs_time_ms();
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        if intermission {
            return;
        }
        mirror_roster(&self.core.clients, rt);
        let out = rt.match_timeout(slot, timein, now);
        self.copy_timeout_configstrings();
        for (to, cmd) in out {
            match to {
                Some(to) => self.send_server_command(to, &cmd),
                None => self.send_to_all(&cmd),
            }
        }
    }

    /// `callvote` or `vote` inside the packet: the lines go out first, then
    /// the vote configstrings the command wrote, as retail sends them.
    pub(super) fn vote_command(&mut self, slot: usize, line: &str) {
        let intermission = self.core.clients[slot]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .is_some_and(|s| s.pm_type == crate::spectate::PmType::Intermission);
        let svs_ms = self.svs_time_ms();
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        if intermission {
            return;
        }
        mirror_roster(&self.core.clients, rt);
        let argv = crate::game::say::tokenize(line);
        let out = rt.vote_command(slot, &argv, svs_ms);
        // `trap_DropClient` comes before the voter's `m -1`.
        if let Some(target) = rt.host.complaint_drop.take() {
            self.drop_client(target, "GAME_KICKEDFROMCOMPLAINTS");
        }
        self.send_vote_lines(out);
    }

    /// A vote command's or `CheckVote`'s lines, then configstrings 15..18
    /// off the game's table.
    pub(super) fn send_vote_lines(&mut self, out: crate::game::vote::Sends) {
        for (to, cmd) in out {
            match to {
                Some(to) => self.send_server_command(to, &cmd),
                None => self.send_to_all(&cmd),
            }
        }
        let at = self.game.proto.cs.vote;
        self.copy_game_configstrings(at..=at + 3);
    }

    /// The two match timeout configstrings off the game's table, sent at
    /// once: the game writes them with `trap_SetConfigstring`, which does
    /// not wait for the frame's diff.
    pub(super) fn copy_timeout_configstrings(&mut self) {
        let at = self
            .game
            .proto
            .cs
            .match_timeout
            .unwrap_or(crate::game::timeout::CS_TIMEOUT_MS);
        self.copy_game_configstrings([at, at + 1]);
    }

    /// Configstrings off the game's table, sent at once, as a
    /// `trap_SetConfigstring` outside the frame's diff sends them.
    pub(super) fn copy_game_configstrings(&mut self, slots: impl IntoIterator<Item = usize>) {
        let Some(rt) = self.game.module.script.as_ref() else {
            return;
        };
        for i in slots {
            if let (Some(to), Some(from)) = (
                self.core.configstrings.get_mut(i),
                rt.configstrings().get(i),
            ) {
                to.clone_from(from);
            }
        }
        self.broadcast_configstring_changes();
    }

    /// `SV_SendServerCommand(NULL, ..)`: every client that has its gamestate.
    pub(super) fn send_to_all(&mut self, cmd: &str) {
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state != ClientState::Connected)
            {
                self.send_server_command(slot, cmd);
            }
        }
    }

    /// `Cmd_Tell_f`'s log line ahead of its two `G_Say`s: 1.1's `tell: %s
    /// to %s: %s` (0x47210), 1.5's and UO's `tell;%d;%d;%s;%d;%d;%s;%s`
    /// (each side's GUID, 0 with no authorize, slot and name), the names
    /// as sent (cod11-map-cycle.md 9).
    fn log_tell(&mut self, slot: usize, to: usize, text: &str) {
        let name = |s: usize| -> String {
            self.core
                .clients
                .get(s)
                .and_then(Option::as_ref)
                .map_or(String::new(), |c| c.name.clone())
        };
        let (from_name, to_name) = (name(slot), name(to));
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        let line = crate::game::say::tell_line(
            rt.host.logs_1_1(),
            (slot, &from_name),
            (to, &to_name),
            text,
        );
        rt.host.log_printf(line);
    }

    /// `G_Say`: retail's `trap_SendServerCommand` queues the line from
    /// inside `ClientCommand`, and so does this. Nothing without a script, which is
    /// where the teams and session states live.
    pub(super) fn say(&mut self, slot: usize, target: Option<usize>, mode: SayMode, text: &str) {
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        mirror_roster(&self.core.clients, rt);
        for (to, cmd) in rt.say(slot, target, mode, text) {
            self.send_to(to, &cmd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// `Cmd_FlameDamageClient_f`'s reading (game.mp.uo 0x4ee2c): `atoi` of
    /// the first three characters, kept in `0..=maxclients`, one argument
    /// only.
    #[test]
    fn an_fdc_argument_reads_as_retail_reads_it() {
        assert_eq!(fdc_inflictor("3", 20), Some(3));
        assert_eq!(fdc_inflictor("20", 20), Some(20));
        assert_eq!(fdc_inflictor("21", 20), None);
        assert_eq!(fdc_inflictor("100", 20), None);
        assert_eq!(fdc_inflictor("1000", 64), None);
        assert_eq!(fdc_inflictor("5000", 64), None);
        assert_eq!(fdc_inflictor("12345", 64), None);
        assert_eq!(fdc_inflictor("1234", 200), Some(123));
        assert_eq!(fdc_inflictor("x", 20), Some(0));
        assert_eq!(fdc_inflictor("4x", 20), Some(4));
        assert_eq!(fdc_inflictor("-1", 20), None);
        assert_eq!(fdc_inflictor("", 20), None);
        assert_eq!(fdc_inflictor("1 2", 20), None);
    }

    /// Every shape `client-probes/probe_menuresponse` sent retail 1.1d, and
    /// what its `menuresponse` carried (gsc language doc 6). The response
    /// comes back verbatim: the gametype compares it against `"allies"` and
    /// weapon names.
    #[test]
    fn mr_answers_each_shape_as_retail() {
        let idx = |i| Some((MenuRef::Index(i), "hello".to_string()));
        let bad = Some((MenuRef::Name(String::new()), "bad".to_string()));
        assert_eq!(parse_menu_response("mr 7 3 hello", 7), idx(3));
        assert_eq!(parse_menu_response("mr 7 0x1 hello", 7), idx(0));
        assert_eq!(parse_menu_response("mr 7 1abc hello", 7), idx(1));
        assert_eq!(parse_menu_response("mr 7 31 hello", 7), idx(31));
        assert_eq!(parse_menu_response("mr 6 3 hello", 7), None, "stale");
        for cmd in ["mr 0 noop", "mr 9 bad", "mr", "mr 7 0", "mr 7 0 a b"] {
            assert_eq!(parse_menu_response(cmd, 7), bad, "{cmd}");
        }
        assert_eq!(
            parse_menu_response("mr 7 40 past", 7),
            Some((MenuRef::Name("40".into()), "past".into()))
        );
        assert_eq!(
            parse_menu_response("mr 7 -1 neg", 7),
            Some((MenuRef::Name("-1".into()), "neg".into()))
        );
        assert_eq!(
            parse_menu_response("mr 7 0 \"quoted word\"", 7),
            Some((MenuRef::Index(0), "quoted word".into()))
        );
    }

    /// A reliableAcknowledge a ring behind (here `i32::MIN`) takes every
    /// command as acknowledged and drops the message, its sequence kept
    /// within the window; one past what was sent is let through, as
    /// retail's signed test lets it.
    #[test]
    fn a_reliable_acknowledge_a_ring_behind_is_clamped() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        let ops = w.into_ops();
        for i in 0..3 {
            sv.send_server_command(0, &format!("v c{i} 1"));
        }
        let link = |sv: &Server| {
            let c = sv.core.clients[0].as_ref().unwrap();
            (c.link.reliable_ack, c.link.netchan.reliable_sequence as i32)
        };
        let (ack, seq) = link(&sv);
        assert!(ack < seq);

        let incoming = |sv: &Server| {
            sv.core.clients[0]
                .as_ref()
                .unwrap()
                .link
                .netchan
                .incoming_sequence
        };
        // serverId 0 asks for a gamestate; only the ack makes this inadmissible.
        let pkt = nc.build_out(0, 0, i32::MIN, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(sv.take_outgoing().is_empty(), "bogus ack got a reply");
        assert_eq!(link(&sv), (seq, seq));
        assert_eq!(incoming(&sv), nc.outgoing_sequence - 1, "sequence kept");

        // One past the window is dropped with its sequence.
        let before = incoming(&sv);
        nc.outgoing_sequence += crate::engine::slot::CLAMP_SEQUENCE_WINDOW;
        let pkt = nc.build_out(0, 0, i32::MIN, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert_eq!(incoming(&sv), before);

        // messageAcknowledge names a message we sent; we have sent none.
        let pkt = nc.build_out(0, 1, 0, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(
            sv.take_outgoing().is_empty(),
            "messageAcknowledge ahead of us got a reply"
        );

        let pkt = nc.build_out(0, 0, seq + 5, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(
            !sv.take_outgoing().is_empty(),
            "no gamestate for an ack ahead of us"
        );
        assert_eq!(link(&sv).0, seq + 5);
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_gap_in_the_client_command_sequence_drops_the_client() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();

        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(2); // skips sequence 1, which we never sent
        w.write_string("say hi");
        w.write_bits(CLC_EOF, 2);
        let pkt = nc
            .build_out(i32::from(sv.game.server_id), 0, 0, &w.into_ops(), &huff)
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);

        assert_eq!(sv.client_count(), 0);
        assert!(
            sv.take_outgoing().is_empty(),
            "the zombie's frame carries it"
        );
        sv.tick(now);
        let mut out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected the drop notice");
        let (to, pkt) = out.remove(0);
        assert_eq!(to, addr(5));
        assert_eq!(
            server_commands(&mut nc, &pkt, &huff),
            vec!["w \"EXE_LOSTRELIABLECOMMANDS\"".to_string()]
        );
    }

    /// A pure server with made-up pak checksums, and an entered client's
    /// first move after the commands `cmds` gives for the level's
    /// `checksumFeed`.
    fn pure_server_after(cmds: impl Fn(i32) -> Vec<String>) -> Server {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.game.paks.pure = true;
        sv.game.pure_check = Some(PureCheck {
            cgame: Some(11),
            ui: Some(22),
            loaded: vec![5, 6],
            feed: sv.core.checksum_feed,
        });
        let mut nc = active(&mut sv, now);
        let cmds = cmds(sv.core.checksum_feed);
        if !cmds.is_empty() {
            let cmds: Vec<&str> = cmds.iter().map(String::as_str).collect();
            pipelined(&mut sv, &mut nc, &cmds, now);
        }
        if sv.client_count() == 1 {
            let huff = Huffman::new();
            let ack = nc.incoming_sequence as i32;
            let ops = move_ops(sv.core.checksum_feed, ack, NULL_USERCMD);
            let pkt = nc
                .build_out(i32::from(sv.game.server_id), ack, 0, &ops, &huff)
                .unwrap();
            sv.handle_packet(addr(5), &pkt, now);
        }
        sv
    }

    #[test]
    fn a_pure_server_keeps_only_clients_whose_cp_verifies() {
        use vcod_common::pak_checksum::pure_command;
        let good = |feed| vec![pure_command(Some(11), Some(22), &[6], feed)];
        assert_eq!(pure_server_after(good).client_count(), 1);
        // No `cp` before the first move: `EXE_CANNOTVALIDATEPURECLIENT`.
        assert_eq!(pure_server_after(|_| Vec::new()).client_count(), 0);
        // A pak the server lacks: `EXE_UNPURECLIENTDETECTED`.
        let bad = |feed| vec![pure_command(Some(11), Some(22), &[7], feed)];
        assert_eq!(pure_server_after(bad).client_count(), 0);
        // `vdr` forgets a good one.
        let reset = |feed| vec![pure_command(Some(11), Some(22), &[6], feed), "vdr".into()];
        assert_eq!(pure_server_after(reset).client_count(), 0);
    }

    /// One client command per sequence, all in one message.
    fn pipelined(sv: &mut Server, nc: &mut Netchan, cmds: &[&str], now: Instant) {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        for (i, cmd) in cmds.iter().enumerate() {
            w.write_bits(CLC_CLIENT_COMMAND, 2);
            w.write_long(i as i32 + 1);
            w.write_string(cmd);
            // The client's own ring, which keys the server's reply.
            nc.reliable[(i + 1) & 63] = cmd.to_string();
        }
        w.write_bits(CLC_EOF, 2);
        let pkt = nc
            .build_out(
                i32::from(sv.game.server_id),
                nc.incoming_sequence as i32,
                0,
                &w.into_ops(),
                &huff,
            )
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);
    }

    /// Fifty `donedl` in one message build one gamestate: a second one
    /// while the gamestate is still the last message sent is a flood.
    #[test]
    fn a_donedl_flood_sends_one_gamestate() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        let seq = |sv: &Server| {
            let c = sv.core.clients[0].as_ref().unwrap();
            (c.link.netchan.outgoing_sequence, c.gamestate_message_num)
        };
        let mut t = now;
        while seq(&sv).0 as i64 == seq(&sv).1 + 1 {
            t += Duration::from_millis(50);
            sv.tick(t);
        }
        sv.take_outgoing();
        let (before, _) = seq(&sv);
        pipelined(&mut sv, &mut nc, &["donedl"; 50], t);
        assert_eq!(seq(&sv), (before + 1, i64::from(before)));
    }

    /// A message with more commands than twice the client's ring is a
    /// forgery: none of it runs.
    #[test]
    fn a_message_past_the_command_cap_runs_nothing() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        let last = |sv: &Server| {
            sv.core.clients[0]
                .as_ref()
                .unwrap()
                .link
                .last_client_command
        };
        pipelined(&mut sv, &mut nc, &["x"; 129], now);
        assert_eq!(last(&sv), 0);
        pipelined(&mut sv, &mut nc, &["x"; 128], now);
        assert_eq!(last(&sv), 128);
    }

    /// Score requests pipelined faster than a snapshot goes out leave one
    /// scoreboard queued: each `b` replaces the unsent one before it.
    #[test]
    fn pipelined_score_requests_leave_one_scoreboard() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();
        let before = sv.core.clients[0]
            .as_ref()
            .unwrap()
            .link
            .netchan
            .reliable_sequence;
        pipelined(&mut sv, &mut nc, &["score"; 65], now);
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!(c.link.netchan.reliable_sequence, before + 1);
        assert!(
            sv.take_outgoing().is_empty(),
            "nothing goes out before a snapshot"
        );
    }

    /// A client that stops acking is dropped once one more command would
    /// overwrite the oldest unacked slot, with retail's reason, the notice
    /// last; the slot then takes a fresh connect.
    #[test]
    fn a_full_ring_of_unacked_commands_drops_the_client() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();
        let room = {
            let c = sv.core.clients[0].as_ref().unwrap();
            MAX_RELIABLE_COMMANDS as i32
                - (c.link.netchan.reliable_sequence as i32 - c.link.reliable_ack)
        };
        for i in 0..room {
            sv.send_server_command(0, &format!("v c{i} 1"));
        }
        assert_eq!(sv.client_count(), 1, "a full ring is not yet fatal");
        sv.send_server_command(0, "v one_more 1");
        assert_eq!(sv.client_count(), 0);
        let huff = Huffman::new();
        sv.tick(now);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "the zombie's message alone");
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(
            cmds.last().map(String::as_str),
            Some("w \"EXE_SERVERCOMMANDOVERFLOW\"")
        );
        let t2 = now + RECONNECT_LIMIT + Duration::from_millis(100);
        connected(&mut sv, addr(5), t2);
        assert_eq!(sv.client_count(), 1);
    }

    /// The reply rides the next snapshot, not a message of its own.
    #[test]
    fn a_score_request_gets_a_deathmatch_scoreboard() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = begun(&mut sv, now);
        sv.tick(now);
        let huff = Huffman::new();
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff);
        }
        pipelined(&mut sv, &mut nc, &["score"], now);
        assert!(sv.take_outgoing().is_empty());
        sv.tick(now);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected one snapshot");
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(cmds.len(), 1);
        // No ack has landed after time 0 yet, so `SV_CalcPings` reads 999.
        assert!(cmds[0].starts_with("b 1 0 0 0 0 999 0 0"), "{:?}", cmds[0]);
    }

    #[test]
    fn the_scoreboard_carries_the_team_scores_axis_first() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { setTeamScore(\"axis\", 3); setTeamScore(\"allies\", -9999); }",
            ),
        );
        assert_eq!(sv.scoreboard(), "b 0 3 -9999");
        let cs = sv.game.module.script.as_ref().unwrap().configstrings();
        assert_eq!((cs[5].as_str(), cs[6].as_str()), ("3", "-9999"));
    }

    /// clc_move carrying one cmd whose pitch and yaw ride change bit 0
    /// (omitted, unchanged from the server's stored cmd), what a retail client
    /// sends when the mouse did not move this packet. Forward/right stay
    /// announced; each serverTime is absolute so the test isolates the angle
    /// base from the serverTime base. `n` cmds, 20 ms apart from `st`: one is
    /// enough to prove the field decodes, but a probe of *which* base it
    /// decoded against needs the resulting velocity to actually get there:
    /// `pmove::spectator_move`'s `accelerate` blends toward the wishdir
    /// rather than snapping to it, so a single cmd's movement is still mostly
    /// the previous leg's residual velocity. A burst gives the (correct or
    /// wrongly-reset) wishdir enough simulated time to dominate.
    fn move_ops_omit_angles(checksum_feed: i32, message_ack: i32, st: i32, n: i32) -> Vec<u8> {
        let huff = Huffman::new();
        let key = checksum_feed ^ message_ack ^ com_hash_key("", 32);
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_MOVE, 2);
        w.write_byte(n as u8);
        for i in 0..n {
            let cmd_st = st + i * 20;
            w.write_bits(0, 1); // serverTime: 32-bit absolute
            w.write_long(cmd_st);
            // Not the whole-cmd shortcut, and the branch bit picks the compact one.
            w.write_bits((key & 1) ^ 1, 1);
            w.write_bits(key & 1, 1);
            let ckey = key ^ cmd_st;
            w.write_bits(ckey & 1, 1); // buttons bit 0, announced as 0
            w.write_bits(0, 1); // pitch omitted
            w.write_bits(0, 1); // yaw omitted
            w.write_bits(1, 1); // forward/right announced
            w.write_bits(1 ^ (ckey & 0xf), 4); // forward 127: bucket 1
        }
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// clc_move whose count byte is past the usercmd cap: the header decodes,
    /// the move block cannot.
    fn garbled_move_ops() -> Vec<u8> {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_MOVE, 2);
        w.write_byte(u8::MAX);
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// Entry is the first usercmd after the gamestate, so every later usercmd
    /// runs past a client that is already in the world; re-entering one would
    /// park its sim back at the spawn mid-flight. The `Primed` guard is all
    /// that stops it.
    #[test]
    fn a_later_move_leaves_a_flying_client_where_it_is() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let p = &PROTOCOL_V1;
        latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        // Two messages, so the base has to chain across them the way the
        // client's does; `begun` entered on a null cmd, which is where a
        // fresh chain starts.
        let mut chain = MoveChain::new(sv.core.checksum_feed);
        let later = now + std::time::Duration::from_millis(50);
        let huff = Huffman::new();
        let ack = nc.incoming_sequence as i32;
        let ops = chain.ops(
            ack,
            &[UserCmd {
                server_time: 50,
                forward: 127,
                ..Default::default()
            }],
        );
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &ops, &huff).unwrap(),
            now,
        );
        let flying = latest_snapshot(&mut sv, &mut nc, &mut ring, later);
        assert!(
            flying.ps.origin(p)[0] > 1.0,
            "the client never left the spawn"
        );

        let ack = nc.incoming_sequence as i32;
        let ops = chain.ops(
            ack,
            &[UserCmd {
                server_time: 100,
                forward: 127,
                ..Default::default()
            }],
        );
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &ops, &huff).unwrap(),
            later,
        );
        let after = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            later + std::time::Duration::from_millis(50),
        );
        assert!(
            after.ps.origin(p)[0] >= flying.ps.origin(p)[0],
            "a later move teleported the client back: {} -> {}",
            flying.ps.origin(p)[0],
            after.ps.origin(p)[0]
        );
    }

    /// A block whose mouse turns mid-flight: per-cmd replay must cover both
    /// headings. The old last-cmd-only step integrated once at yaw 90 and
    /// never moved along +X.
    #[test]
    fn turning_cmds_integrate_per_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let t0 = now;
        let t1 = now + std::time::Duration::from_millis(50);
        let s = latest_snapshot(&mut sv, &mut nc, &mut ring, t0);

        // Forward at yaw 0 (faces +X), then the mouse swings to yaw 90 (+Y).
        let ack = nc.incoming_sequence as i32;
        let cmds = [
            UserCmd {
                server_time: 50,
                forward: 127,
                angles: [0, 0, 0],
                ..Default::default()
            },
            UserCmd {
                server_time: 100,
                forward: 127,
                angles: [0, 16384, 0],
                ..Default::default()
            },
        ];
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_cmds(sv.core.checksum_feed, ack, &cmds),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let o1 = s.ps.origin(&PROTOCOL_V1);
        let o2 = s2.ps.origin(&PROTOCOL_V1);
        assert!(
            o2[0] - o1[0] > 2.0,
            "cmd A should have flown +X, dx {}",
            o2[0] - o1[0]
        );
        assert!(
            o2[1] - o1[1] > 2.0,
            "cmd B should have flown +Y, dy {}",
            o2[1] - o1[1]
        );
    }

    /// A cmd from before the processed clock is stale: skipped whole, no
    /// motion, no panic.
    #[test]
    fn stale_cmds_are_skipped() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.core.checksum_feed);
        let mut tick_at = now + std::time::Duration::from_millis(50);

        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 500,
                        forward: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, tick_at);
        assert!(
            s1.ps.origin(&PROTOCOL_V1)[0] > 1.0,
            "the fresh cmd must move the sim first, at {}",
            s1.ps.origin(&PROTOCOL_V1)[0]
        );

        // A reordered/duplicated cmd from before serverTime 500.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 0,
                        forward: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        tick_at += std::time::Duration::from_millis(50);
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, tick_at);
        assert_eq!(
            s2.ps.origin(&PROTOCOL_V1),
            s1.ps.origin(&PROTOCOL_V1),
            "a stale cmd must not move the sim"
        );
    }

    /// More cmds than a tick may replay arrive back to back; the queue stays
    /// bounded and snapshotting carries on.
    #[test]
    fn flood_is_bounded() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.core.checksum_feed);
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        let cmds: Vec<UserCmd> = (0..100)
            .map(|i| UserCmd {
                server_time: i * 20,
                forward: 127,
                ..Default::default()
            })
            .collect();
        for chunk in cmds.chunks(MAX_PACKET_USERCMDS as usize) {
            let ack = nc.incoming_sequence as i32;
            sv.handle_packet(
                addr(5),
                &nc.build_out(0x10, ack, 0, &chain.ops(ack, chunk), &Huffman::new())
                    .unwrap(),
                now,
            );
        }
        let s = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            now + std::time::Duration::from_millis(50),
        );
        assert!(s.valid);
        assert!(
            sv.core.clients[0].as_ref().unwrap().pending.len() <= 2,
            "flood left {} cmds queued",
            sv.core.clients[0].as_ref().unwrap().pending.len()
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            now + std::time::Duration::from_millis(100),
        );
        assert!(s2.valid, "snapshots must continue");
    }

    /// `mr` is flood-exempt; a client cannot queue more of them for the
    /// move pass than its reliable ring holds unacknowledged.
    #[test]
    fn menu_response_flood_is_bounded() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let _nc = begun(&mut sv, now);
        let last = sv.core.clients[0]
            .as_ref()
            .unwrap()
            .link
            .last_client_command;
        for i in 1..=1000 {
            assert!(sv.client_command(0, last + i, "mr x".to_string()));
        }
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!(c.link.last_client_command, last + 1000);
        assert_eq!(c.commands_at.len(), MAX_RELIABLE_COMMANDS);
    }

    /// A retail client omits unchanged angle fields (change bit 0) instead of
    /// announcing them, so the server must decode them against the cmd it
    /// last stored for this client, not `NULL_USERCMD`: decoding against
    /// the null base resets the move basis to yaw 0 for a frame (the
    /// "spectator flash" bug; protocol doc, "The base cmd is built from the
    /// playerstate"). `ps.yaw` steers
    /// `pmove::spectator_move`'s wishdir directly, so a burst of `forward`
    /// cmds sent right after the omission is a direct probe of which base
    /// the server used: yaw -45 accelerates the spectator diagonally
    /// (velocity settles near `vx == -vy`), yaw 0 (the bug) only along +x
    /// (`vy` stays small). A single cmd is not enough to tell them apart:
    /// `accelerate` blends toward the wishdir rather than snapping to it, so
    /// one frame is still mostly the previous leg's momentum; a 20-cmd burst
    /// gives the (correct or wrongly-reset) wishdir time to dominate, which
    /// I confirmed empirically (`vy` -283 with the real base, -20 with the
    /// bug forced, at this burst length). `viewangles` no longer carries
    /// this (it stays unwritten, docs/protocol-1.1.md "Spectator", "View
    /// angles"), so this checks the property directly instead of through
    /// that field.
    #[test]
    fn omitted_angles_decode_against_the_stored_last_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        // Turn to yaw -45, announced like vcod's own client always does.
        let yaw = ((-45.0f32) * 65536.0 / 360.0) as i32 & 0xffff;
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.core.checksum_feed,
                    ack,
                    UserCmd {
                        server_time: 500,
                        forward: 127,
                        angles: [0, yaw, 0],
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let o1 = s1.ps.origin(&PROTOCOL_V1);
        assert!(
            o1[0] > 0.1 && o1[1] < -0.1,
            "cmd 1's announced yaw -45 must steer the spectator diagonally from spawn, origin {o1:?}"
        );

        // Next packet: the mouse did not move for 20 frames, so pitch and yaw
        // ride change bit 0 off the cmd the server just stored, every frame.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_omit_angles(sv.core.checksum_feed, ack, 520, 20),
                &Huffman::new(),
            )
            .unwrap(),
            t1,
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        // Anti-vacuous: the burst must actually have been applied.
        let dx = s2.ps.origin(&PROTOCOL_V1)[0] - o1[0];
        assert!(dx > 1.0, "the omitted-angle burst was not applied, dx {dx}");
        // The discriminator: only a base of yaw -45 leaves a large negative
        // vy once the burst settles; a base reset to yaw 0 does not.
        let vy = s2.ps.field_f32(&PROTOCOL_V1, "velocity[1]");
        assert!(
            vy < -50.0,
            "omitted angles must keep the stored yaw -45, not reset to 0: vy {vy}"
        );
    }

    /// A protocol 6 server moves its client by 1.5's rules and says so on
    /// the wire: a jump arms the penalty, `pm_flags` 0x2000 with its
    /// `pm_time` (cod15-movement.md, "The jump penalty"). A 1.1 server's
    /// jump carries neither.
    #[test]
    fn protocol_22_sprints_and_tires_on_the_wire() {
        use vcod_common::net::flags::PMF_SPRINT_V22;
        use vcod_common::net::msg::BUTTON_SPRINT_V22;
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_protocol(&PROTOCOL_V22);
        sv.load_world(World {
            collision: test_world(&[(
                glam::Vec3::new(-4000.0, -4000.0, -64.0),
                glam::Vec3::new(4000.0, 4000.0, 0.0),
            )]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 0.125], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let sim = sv.core.clients[0].as_mut().unwrap().sim.as_mut().unwrap();
        sim.become_player([0.0, 0.0, 0.125], 0.0, [0; 3]);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.core.checksum_feed);
        let mut t = now;
        let mut seen = Vec::new();
        for i in 0..40 {
            let st = 100 + i * 50;
            let ack = nc.incoming_sequence as i32;
            let cmd = UserCmd {
                server_time: st,
                forward: 127,
                buttons: BUTTON_SPRINT_V22,
                ..Default::default()
            };
            let ops = chain.ops(ack, &[cmd]);
            let pkt = nc.build_out(0x10, ack, 0, &ops, &Huffman::new()).unwrap();
            sv.handle_packet(addr(5), &pkt, t);
            t += std::time::Duration::from_millis(50);
            sv.core.sv_time_ms = st;
            let s = latest_snapshot_as(&mut sv, &mut nc, &mut ring, t, &PROTOCOL_V22);
            let ps = &s.ps;
            seen.push((
                ps.field_i32(&PROTOCOL_V22, "pm_flags") & PMF_SPRINT_V22 != 0,
                ps.field_f32(&PROTOCOL_V22, "fatigueScale"),
                ps.field_i32(&PROTOCOL_V22, "lastSprintTime"),
                ps.field_f32(&PROTOCOL_V22, "sprintSpeedScale"),
            ));
        }
        let last = seen.last().unwrap();
        assert!(seen.iter().any(|s| s.0), "never sprinting: {seen:?}");
        assert!(last.1 < 1.0 && last.2 > 0, "{last:?}");
        assert_eq!(last.3, 1.6);
    }

    #[test]
    fn protocol_6_jumps_carry_the_15_penalty() {
        for (p, decode) in [(&PROTOCOL_V6, &RAW_V6), (&PROTOCOL_V1, &PROTOCOL_V1)] {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(p);
            sv.load_world(World {
                collision: test_world(&[(
                    glam::Vec3::new(-2000.0, -2000.0, -64.0),
                    glam::Vec3::new(2000.0, 2000.0, 0.0),
                )]),
                vis: vcod_common::bsp::Visibility::none(),
                spawn: ([0.0, 0.0, 0.125], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
                rubble: Vec::new(),
                nav_cover: Vec::new(),
            });
            let mut nc = begun(&mut sv, now);
            let sim = sv.core.clients[0].as_mut().unwrap().sim.as_mut().unwrap();
            sim.become_player([0.0, 0.0, 0.125], 0.0, [0; 3]);
            let mut ring = SnapshotRing::new();
            let mut chain = MoveChain::new(sv.core.checksum_feed);
            let mut t = now;
            let mut penalty = Vec::new();
            for i in 0..30 {
                let st = 100 + i * 50;
                let up = if i == 10 { 127 } else { 0 };
                let ack = nc.incoming_sequence as i32;
                let cmd = UserCmd {
                    server_time: st,
                    up,
                    ..Default::default()
                };
                let ops = chain.ops(ack, &[cmd]);
                let pkt = nc.build_out(0x10, ack, 0, &ops, &Huffman::new()).unwrap();
                sv.handle_packet(addr(5), &pkt, t);
                t += std::time::Duration::from_millis(50);
                sv.core.sv_time_ms = st;
                let s = latest_snapshot_as(&mut sv, &mut nc, &mut ring, t, decode);
                let flags = s.ps.field_i32(decode, "pm_flags");
                penalty.push((flags & 0x2000 != 0, s.ps.field_i32(decode, "pm_time")));
            }
            let armed = penalty.iter().filter(|(on, time)| *on && *time > 0).count();
            if p.transport.version == PROTOCOL_V6.transport.version {
                assert!(armed > 0, "{penalty:?}");
                assert!(!penalty[5].0, "no penalty before the jump");
            } else {
                assert_eq!(armed, 0, "{penalty:?}");
            }
        }
    }

    /// Press then release, end to end: the release must decode up=0 against
    /// the server's stored base, or the sim replays the jump forever.
    #[test]
    fn released_upmove_stops_the_climb() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.core.checksum_feed);
        let p = &PROTOCOL_V1;

        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 500,
                        up: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let vz = s1.ps.field_f32(p, "velocity[2]");
        assert!(vz > 100.0, "holding up must climb, vz {vz}");

        // A burst of release frames: friction alone has to bring the climb
        // back to rest. Spectator friction drops speed * 5 * frametime, a
        // geometric decay with no stop-speed floor, over 32 frames (the wire
        // per-message cap).
        let releases: Vec<UserCmd> = (0..32)
            .map(|i| UserCmd {
                server_time: 550 + i * 50,
                ..Default::default()
            })
            .collect();
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &chain.ops(ack, &releases), &Huffman::new())
                .unwrap(),
            t1,
        );
        // A clock the burst's last cmd is within 200 ms of: the earliest
        // cmds clamp up to 1000 ms behind it, and their frames run as one.
        sv.core.sv_time_ms = 1900;
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        let vz = s2.ps.field_f32(p, "velocity[2]");
        // `PM_Friction` stops at speed 1 and zeroes only xy, so the climb
        // rests just under 1 u/s, as retail's idle spectator read 0.9565565
        // (docs/protocol-1.1.md, "Spectator").
        assert!(
            vz > 0.0 && vz < 1.0,
            "released upmove must decay to rest, vz {vz}"
        );
    }

    /// A move message that fails to decode is discarded whole: the next good
    /// one still chains from the last successfully decoded cmd, not
    /// `NULL_USERCMD`: same base-decode property and the same burst probe
    /// as `omitted_angles_decode_against_the_stored_last_cmd`, with a
    /// garbled packet spliced in first.
    #[test]
    fn a_garbled_message_does_not_poison_the_base() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = begun(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        let yaw = ((-45.0f32) * 65536.0 / 360.0) as i32 & 0xffff;
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.core.checksum_feed,
                    ack,
                    UserCmd {
                        server_time: 500,
                        forward: 127,
                        angles: [0, yaw, 0],
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let o1 = s1.ps.origin(&PROTOCOL_V1);
        assert!(
            o1[0] > 0.1 && o1[1] < -0.1,
            "cmd 1's announced yaw -45 must steer the spectator diagonally from spawn, origin {o1:?}"
        );

        // Garbage that cannot parse, then a good packet whose angles are
        // omitted: they must come off the pre-failure base.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &garbled_move_ops(), &Huffman::new())
                .unwrap(),
            t1,
        );
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_omit_angles(sv.core.checksum_feed, ack, 520, 20),
                &Huffman::new(),
            )
            .unwrap(),
            t1,
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        assert!(s2.valid, "snapshots must continue");
        // Anti-vacuous: the burst must actually have been applied.
        let dx = s2.ps.origin(&PROTOCOL_V1)[0] - o1[0];
        assert!(dx > 1.0, "the omitted-angle burst was not applied, dx {dx}");
        // The discriminator: only a base of yaw -45 leaves a large negative
        // vy once the burst settles; a base reset to yaw 0 does not.
        let vy = s2.ps.field_f32(&PROTOCOL_V1, "velocity[1]");
        assert!(
            vy < -50.0,
            "the garbled packet must not reset the base to yaw 0: vy {vy}"
        );
    }

    /// A panic in one client's packet drops that client, not the server
    /// (`engine::contain`): the next frame runs, the zombie gets its drop
    /// and the other client its snapshot.
    #[test]
    fn a_panicking_packet_drops_only_its_client() {
        use crate::engine::contain::{self, inject};
        let t = Instant::now();
        let low = addr(5);
        let high = SocketAddr::from(([127, 0, 0, 2], 5));
        let mut sv = Server::new(cfg(), t);
        let mut nc = active_at(&mut sv, low, t);
        active_at(&mut sv, high, t);
        sv.take_outgoing();
        contain::enable();
        inject::arm(inject::Site::ClientPacket);
        let huff = Huffman::new();
        let ack = nc.incoming_sequence as i32;
        let pkt = nc.build_out(0x10, ack, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(low, &pkt, t);
        assert_eq!(sv.client_count(), 1);
        assert!(sv.game.zombies[0].is_some(), "slot 0 dropped");
        assert!(sv.core.clients[1].is_some());
        sv.tick(t);
        let sent: Vec<_> = sv.take_outgoing().into_iter().map(|(to, _)| to).collect();
        assert_eq!(sent, [low, high]);
        assert!(sv.fatal().is_none());
    }
}
