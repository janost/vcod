//! Connectionless packets: getinfo, getstatus, rcon, getchallenge and connect.

use super::*;

/// The line of a connectionless packet as `SV_ConnectionlessPacket`
/// (cod_lnxded 0x808c63c) reads it: `MSG_ReadStringLine` stops at a NUL, a
/// newline or 1023 bytes and turns `%` and every byte past 127 into `.`
/// (docs/research/cod11-server-handshake.md, "The challenge key").
fn oob_line(raw: &[u8]) -> String {
    raw.get(4..)
        .unwrap_or_default()
        .iter()
        .take_while(|&&b| b != 0 && b != b'\n')
        .take(1023)
        .map(|&b| {
            if b == b'%' || b > 127 {
                '.'
            } else {
                char::from(b)
            }
        })
        .collect()
}

/// `Cmd_Argv(n)` of a connectionless packet: its line through
/// `Cmd_TokenizeString`.
fn oob_argv(raw: &[u8], n: usize) -> String {
    crate::game::say::tokenize(&oob_line(raw))
        .into_iter()
        .nth(n)
        .unwrap_or_default()
}

/// `Info_RemoveKey`: the info string without `key`'s pair.
fn remove_info_key(info: &str, key: &str) -> String {
    let mut out = String::new();
    let mut parts = info.strip_prefix('\\').unwrap_or(info).split('\\');
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if k != key {
            out.push_str(&format!("\\{k}\\{v}"));
        }
    }
    out
}

impl Server {
    /// `connect` re-parses `raw` because its body is compressed; the browser
    /// queries tokenize its line (`oob_argv`). The queries are rate limited
    /// before anything is looked at; a limited request is dropped silently.
    pub(super) fn handle_oob(
        &mut self,
        from: SocketAddr,
        cmd: &str,
        raw: &[u8],
        now: Instant,
        read: Instant,
    ) {
        // A `connect` line is compressed; its tokenized userinfo is not kept.
        if cmd != "connect" {
            self.game.argv1 = oob_argv(raw, 1);
        }
        match cmd {
            "getinfo" | "getstatus" | "getchallenge" => {
                if self.core.limiter.limited(cmd, from.ip(), now) {
                    log::debug!("{from}: {cmd} rate limited");
                    return;
                }
                match cmd {
                    "getinfo" => crate::engine::answer_status(
                        self,
                        from,
                        crate::engine::StatusQuery::Info,
                        &oob_argv(raw, 1),
                    ),
                    "getstatus" => crate::engine::answer_status(
                        self,
                        from,
                        crate::engine::StatusQuery::Status,
                        &oob_argv(raw, 1),
                    ),
                    _ => self.svc_get_challenge(from, now),
                }
            }
            "connect" => self.svc_direct_connect(from, raw, now),
            // Rate limited by its own server-wide window, not the buckets,
            // on the clock at the handling.
            "rcon" => self.svc_remote_command(from, raw, read, now),
            // `SV_ConnectionlessPacket` (0x808c827) matches it and does
            // nothing; a client leaves through the netchan `disconnect`.
            "disconnect" => {}
            other => log::debug!("{from}: unhandled oob {other:?}"),
        }
    }

    /// `g_password` as the game module reads it: the running level's table,
    /// or the `--set` that the first load will stamp. Not latched.
    fn game_password(&self) -> String {
        self.live_cvar("g_password", "")
    }

    /// A cvar as the engine or the game reads it at this moment: the running
    /// level's table, else the `--set` the first load will stamp, else
    /// `default`.
    pub(super) fn live_cvar(&self, name: &str, default: &str) -> String {
        match self.game.module.script.as_ref() {
            Some(rt) => rt.cvars().get(name).to_string(),
            None => self.pending_cvar(name, default),
        }
    }

    /// `sv_privateClients` read as `->integer`: the slots below it are
    /// reserved for a client whose userinfo `password` is
    /// `sv_privatePassword` (docs/research/cod11-server-handshake.md,
    /// "Private slots").
    pub(super) fn private_clients(&self) -> i32 {
        crate::game::builtins::cvar::atoi(&self.live_cvar("sv_privateClients", "0"))
    }

    /// A cvar read as `->integer`.
    pub(super) fn live_int_cvar(&self, name: &str) -> i32 {
        crate::game::builtins::cvar::atoi(&self.live_cvar(name, "0"))
    }

    /// `getinfo`/`getstatus`'s `pswrd`: 1 for any non-empty `g_password`,
    /// `none` included (0x808c3ae, 0x808bf23).
    fn pswrd(&self) -> u8 {
        u8::from(!self.game_password().is_empty())
    }

    /// `ClientConnect`'s password test (game.mp.i386.so 0x425da): an empty
    /// or `none` `g_password` lets anyone in, otherwise the userinfo's
    /// `password` must match it case-sensitively. Bots are exempt, as a
    /// retail test client's `ip` reads `localhost`
    /// (docs/research/cod11-server-handshake.md, "g_password").
    pub(super) fn password_denied(&self, userinfo: &str, is_bot: bool) -> bool {
        let pw = self.game_password();
        !is_bot
            && !pw.is_empty()
            && !pw.eq_ignore_ascii_case("none")
            && info_value_for_key(userinfo, "password").unwrap_or("") != pw
    }

    /// `SVC_Info` (cod_lnxded 0x808c1ac). Key order is retail's;
    /// `minPing`/`maxPing`/`game` only appear when the matching cvar is set.
    /// The private slots are left out of both counts: `clients` counts the
    /// occupied slots from `sv_privateClients` up, `sv_maxclients` is the
    /// rest. A challenge too long to fit gets no reply: retail's
    /// `Info_SetValueForKey` drops the level or crashes there.
    pub(super) fn svc_info(&mut self, challenge: &str) -> Option<String> {
        if self.v15() {
            return self.svc_info_v6(challenge);
        }
        let private = self.private_clients();
        let public_clients = self
            .core
            .clients
            .iter()
            .skip(private.max(0) as usize)
            .flatten()
            .count();
        let mut pairs = vec![
            ("challenge", challenge.to_string()),
            ("protocol", self.game.proto.transport.version.to_string()),
            (
                "hostname",
                self.live_cvar("sv_hostname", &self.game.cfg.hostname),
            ),
            ("mapname", self.game.cfg.map.clone()),
            ("clients", public_clients.to_string()),
            (
                "sv_maxclients",
                (self.game.cfg.max_clients as i32 - private).to_string(),
            ),
            ("gametype", self.live_gametype()),
            ("pure", u8::from(self.game.paks.pure).to_string()),
        ];
        // `->integer`, each key only when non-zero (0x808c318, 0x808c344).
        for (key, cvar) in [("minPing", "sv_minPing"), ("maxPing", "sv_maxPing")] {
            let v = self.live_int_cvar(cvar);
            if v != 0 {
                pairs.push((key, v.to_string()));
            }
        }
        // An empty `fs_game` reads as no key, as `Info_SetValueForKey` has it.
        pairs.push(("game", self.live_cvar("fs_game", "")));
        pairs.push((
            "sv_allowAnonymous",
            self.live_int_cvar("sv_allowAnonymous").to_string(),
        ));
        pairs.push(("pswrd", self.pswrd().to_string()));
        let mut i = Info::new();
        self.info_pairs(&mut i, pairs)
            .then(|| format!("infoResponse\n{i}"))
    }

    /// 1.5b's `SVC_Info` (cod_lnxded 1.5b 0x8092a74,
    /// docs/research/cod15-protocol-diff.md, "Server side"): `clients`
    /// counts every slot and `sv_maxclients` gives back the private slots
    /// in use, the zero-valued keys are left out, and five keys follow
    /// `pswrd`. A pair that does not fit is left out and the rest still go.
    fn svc_info_v6(&mut self, challenge: &str) -> Option<String> {
        let private = self.private_clients().max(0) as usize;
        let in_private = self.core.clients.iter().take(private).flatten().count() as i32;
        let clients = self.client_count() as i32;
        let max = self.game.cfg.max_clients as i32 - (private as i32 - in_private);
        let mut i = Info::new();
        i.set_value_for_key("challenge", challenge);
        i.set_value_for_key("protocol", self.game.proto.transport.version);
        i.set_value_for_key(
            "hostname",
            self.live_cvar("sv_hostname", &self.game.cfg.hostname),
        );
        i.set_value_for_key("mapname", &self.game.cfg.map);
        if clients != 0 {
            i.set_value_for_key("clients", clients);
        }
        if max > 0 {
            i.set_value_for_key("sv_maxclients", max);
        }
        i.set_value_for_key("gametype", self.live_gametype());
        if self.game.paks.pure {
            i.set_value_for_key("pure", 1);
        }
        let int = |sv: &Self, cvar: &str| sv.live_int_cvar(cvar);
        for (key, cvar) in [("minPing", "sv_minPing"), ("maxPing", "sv_maxPing")] {
            let v = int(self, cvar);
            if v != 0 {
                i.set_value_for_key(key, v);
            }
        }
        let game = self.live_cvar("fs_game", "");
        if !game.is_empty() {
            i.set_value_for_key("game", &game);
        }
        let anon = int(self, "sv_allowAnonymous");
        if anon != 0 {
            i.set_value_for_key("sv_allowAnonymous", anon);
        }
        if self.pswrd() != 0 {
            i.set_value_for_key("pswrd", 1);
        }
        // The string, when its `atoi` is non-zero; UO adds its vehicle
        // switches (coduo_lnxded 0x809392e).
        let uo: &[(&str, &str)] = if self.game.proto.is_uo() {
            &[("jps", "scr_allow_jeeps"), ("tnk", "scr_allow_tanks")]
        } else {
            &[]
        };
        for &(key, cvar) in [
            ("ff", "scr_friendlyfire"),
            ("kc", "scr_killcam"),
            ("timeoutsAllowed", "g_timeoutsallowed"),
        ]
        .iter()
        .chain(uo)
        {
            let v = self.live_cvar(cvar, "0");
            if crate::game::builtins::cvar::atoi(&v) != 0 {
                i.set_value_for_key(key, v);
            }
        }
        i.set_value_for_key("hw", 1);
        i.set_value_for_key("pb", int(self, "sv_punkbuster"));
        i.set_value_for_key("mod", u8::from(self.modded(&game)));
        Some(format!("infoResponse\n{i}"))
    }

    /// 1.5's `mod` key: 1 for a server that is not pure, runs an `fs_game`,
    /// or references a pak `FS_idPak` (cod_lnxded 1.5b 0x80754ea, the same
    /// rule as the client's) does not count as stock.
    pub(super) fn modded(&self, fs_game: &str) -> bool {
        !self.game.paks.pure
            || !fs_game.is_empty()
            || self.game.paks.referenced.iter().any(|(_, name)| {
                !vcod_common::net::download::is_stock_pak(name, &self.game.proto.transport)
            })
    }

    /// `SV_MasterGameCompleteStatus` (0x808bbc4) and the packet it sends,
    /// `SVC_GameCompleteStatus` (0x808bfa8): the serverinfo with the
    /// argument as `challenge`, and a `statusResponse` player line per
    /// connected client.
    pub(super) fn game_complete_status(&mut self, arg: &str) {
        let mut resolve = self.core.resolver;
        let (text, to) = self.core.masters.game_complete_status(
            self.game.dedicated,
            self.svs_time_ms(),
            &mut resolve,
        );
        self.print(&text);
        if to.is_empty() {
            return;
        }
        let mut i = self.serverinfo();
        let mut pairs = vec![("challenge", arg.to_string())];
        pairs.extend(self.demo_keywords(&i));
        if !self.info_pairs(&mut i, pairs) {
            return;
        }
        let msg = format!("gameCompleteStatus\n{i}\n{}", self.status_lines());
        for a in to {
            self.send_oob(a, &msg);
        }
    }

    /// The player lines `SVC_Status` and `SVC_GameCompleteStatus` share:
    /// score, ping and name per client.
    fn status_lines(&mut self) -> String {
        let mut lines = String::new();
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            let (ping, name) = (c.link.ping, c.name.clone());
            let score = self.client_score(slot);
            lines.push_str(&format!("{score} {ping} \"{name}\"\n"));
        }
        lines
    }

    /// Appends `pairs` through `Info_SetValueForKey`. A pair past
    /// `MAX_INFO_STRING` is skipped on 1.5 and UO, which warn and go on;
    /// 1.1's `Com_Error` drops the level (or crashes), so there the reply
    /// is abandoned and false comes back (cod11-server-handshake.md, "The
    /// challenge key").
    pub(super) fn info_pairs(&self, i: &mut Info, pairs: Vec<(&str, String)>) -> bool {
        for (key, value) in pairs {
            if !i.set_value_for_key(key, value) && !self.v15() {
                return false;
            }
        }
        true
    }

    /// The `demo` keyword `SVC_Status` and `SVC_GameCompleteStatus` add
    /// after `challenge` when `fs_restrict` is non-zero (0x808bda3,
    /// 0x808bff3; 1.5b 0x80924c5, UO 0x809336d): `sv_keywords` becomes
    /// `demo <sv_keywords>`, moved to the end.
    fn demo_keywords(&self, info: &Info) -> Option<(&'static str, String)> {
        let restrict = crate::game::builtins::cvar::atof(&self.live_cvar("fs_restrict", ""));
        (restrict != 0.0).then(|| {
            let kw = info.get("sv_keywords").unwrap_or("");
            ("sv_keywords", format!("demo {kw}"))
        })
    }

    /// `SVC_Status` (0x808bd50; 1.5b 0x8092538 adds `con_disabled` and
    /// `mod`).
    pub(super) fn svc_status(&mut self, challenge: &str) -> Option<String> {
        let mut i = self.serverinfo();
        let mut pairs = vec![("challenge", challenge.to_string())];
        pairs.extend(self.demo_keywords(&i));
        if self.v15() {
            let disabled = self.live_int_cvar("sv_disableClientConsole");
            if disabled != 0 {
                pairs.push(("con_disabled", disabled.to_string()));
            }
        }
        pairs.push(("pswrd", self.pswrd().to_string()));
        if self.v15() {
            let game = self.live_cvar("fs_game", "");
            pairs.push(("mod", u8::from(self.modded(&game)).to_string()));
        }
        if !self.info_pairs(&mut i, pairs) {
            return None;
        }
        let lines = self.status_lines();
        Some(format!("statusResponse\n{i}\n{lines}"))
    }

    /// `SVC_RemoteCommand` (0x808c404). The reply is whatever the command
    /// printed, in `print\n` packets of up to `rcon::OUTPUT_BUF` bytes; a
    /// rate-limited request gets nothing.
    fn svc_remote_command(&mut self, from: SocketAddr, raw: &[u8], read: Instant, now: Instant) {
        use crate::rcon::Verdict;
        let args = crate::game::say::tokenize(&oob_line(raw));
        let verdict = self
            .core
            .rcon
            .check(&self.game.rcon_password, &args, from.ip(), read);
        match crate::rcon::log_line(&verdict, from, &args) {
            Some(line) => log::info!("{line}"),
            None => return,
        }
        self.game.redirect = Some(crate::rcon::Redirect::default());
        match verdict {
            Verdict::NoPassword => self.print(crate::rcon::NO_PASSWORD),
            Verdict::BadPassword => self.print(crate::rcon::BAD_PASSWORD),
            Verdict::Run(Some(line)) => {
                self.exec_console_line(&line, now);
            }
            Verdict::Run(None) | Verdict::Limited => {}
        }
        // A `quit` exits before the redirect is flushed, and a `killserver`
        // shuts the server down under it: no reply either way.
        if self.core.quit || self.game.killed {
            self.game.redirect = None;
            return;
        }
        for reply in self.game.redirect.take().unwrap_or_default().finish() {
            self.send_oob(from, &reply);
        }
    }

    /// `SV_GetChallenge` without the authorize-server detour, on the table
    /// all four retail servers share (`engine::challenge`). A banned guid
    /// gets the error retail relays from an authorize denial (1.1) or its
    /// own refusal at the `accept` (1.5); the guid stands in for an
    /// off-LAN address, and a LAN client has none, as on retail.
    fn svc_get_challenge(&mut self, from: SocketAddr, _now: Instant) {
        let guid = guid_of(crate::engine::Lan::Cod1, from);
        if self.core.bans.is_banned(guid) {
            self.core.challenges.forget(from);
            if self.v15() {
                // 1.5's `ban.txt` check at the authorize `accept` (0x80896bc).
                self.print(&format!(
                    "rejected connection from permanently banned GUID {guid}\n"
                ));
                self.send_oob(
                    from,
                    "error\n\u{15}You are permanently banned from this server",
                );
            } else {
                self.send_oob(from, "error\nEXE_ERR_BAD_CDKEY");
            }
            return;
        }
        if self
            .core
            .temp_bans
            .banned(guid, self.core.sv_time_ms, self.kick_ban_time())
        {
            self.core.challenges.forget(from);
            self.print(&format!(
                "rejected connection from temporarily banned GUID {guid}\n"
            ));
            self.send_oob(
                from,
                "error\n\u{15}You are temporarily banned from this server",
            );
            return;
        }
        if self.per_ip_full(from, true) {
            log::info!("{from}: getchallenge refused, its address holds too many slots");
            self.send_oob(from, crate::engine::link::PER_IP_REFUSAL);
            return;
        }
        // Slots on the unscaled clock (1.5b 0x83ccd88), the draw's xor on
        // the game's (0x83ccd84); one clock on 1.1d (0x83b67a4).
        let (svs, game) = (self.svs_time_ms(), self.core.sv_time_ms);
        let rng = &mut self.core.rng;
        let challenge = self
            .core
            .challenges
            .issue(from, svs, || (rng.rand() << 16) ^ rng.rand() ^ game);
        self.send_oob(from, &format!("challengeResponse {challenge}"));
    }

    /// `SV_DirectConnect` (sv_client.c:252) with CoD's rejection strings.
    fn svc_direct_connect(&mut self, from: SocketAddr, raw: &[u8], now: Instant) {
        let userinfo = match parse_connect(raw) {
            Ok(u) => u,
            Err(e) => {
                log::debug!("{from}: bad connect: {e}");
                return;
            }
        };
        let val = |k: &str| info_value_for_key(&userinfo, k).and_then(|v| v.parse::<i32>().ok());
        if val("protocol") != Some(self.game.proto.transport.version as i32) {
            let v = self.game.proto.transport.short_version;
            self.send_oob(
                from,
                &format!("error\nEXE_SERVER_IS_DIFFERENT_VER\x15{v}\n"),
            );
            return;
        }
        let (Some(challenge), Some(qport)) = (val("challenge"), val("qport")) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        let qport = qport as u16;
        // A bot's loopback port and qport are made up; no socket owns them.
        let same_peer = |c: &Client| {
            !c.is_bot
                && c.link.addr.ip() == from.ip()
                && (c.link.netchan.qport == qport || c.link.addr.port() == from.port())
        };
        if self
            .core
            .clients
            .iter()
            .chain(&self.game.zombies)
            .flatten()
            .any(|c| same_peer(c) && now.duration_since(c.last_connect) < RECONNECT_LIMIT)
        {
            log::debug!("{from}: reconnect rejected, too soon");
            return;
        }
        // The whole address, port included (`NET_CompareAdr` 0x8080dec).
        let svs = self.svs_time_ms();
        let Some((ci, ping)) = self.core.challenges.connect(from, challenge, svs) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        log::debug!("Client {ci} connecting with {ping} challenge ping");
        // Off the LAN only, against `->value`; 0 turns each test off
        // (0x80857ee..0x80858a4).
        if !crate::engine::Lan::Cod1.is_lan(from.ip()) {
            let min = crate::game::builtins::cvar::atof(&self.live_cvar("sv_minPing", "0"));
            let max = crate::game::builtins::cvar::atof(&self.live_cvar("sv_maxPing", "0"));
            if min != 0.0 && (ping as f32) < min {
                log::debug!("Client {ci} rejected on a too low ping");
                self.send_oob(from, "error\nEXE_ERR_HIGH_PING_ONLY");
                return;
            }
            if max != 0.0 && (ping as f32) > max {
                log::debug!("Client {ci} rejected on a too high ping: {ping}");
                self.send_oob(from, "error\nEXE_ERR_LOW_PING_ONLY");
                return;
            }
        }
        let reconnect = self
            .core
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(same_peer));
        if let Some(i) = reconnect {
            // Retail hands the slot to any challenge issued to this ip, so
            // a neighbour behind the same NAT who knows the qport can take
            // over a live player. Only the slot's own challenge (the
            // client's connect retry) may replace a client still heard
            // from; a silent slot (crash, lost disconnect) is reclaimable.
            let c = self.core.clients[i].as_ref().unwrap();
            if c.link.netchan.challenge != challenge
                && now.duration_since(c.last_packet) < RECONNECT_LIMIT
            {
                log::info!("{from}: connect with a foreign challenge refused, client {i} is live");
                return;
            }
        }
        // A zombie of this peer is a reconnect into its own slot; the game
        // saw it leave at the drop, so there is nothing to tear down.
        let zombie = self
            .game
            .zombies
            .iter()
            .position(|c| c.as_ref().is_some_and(same_peer));
        if reconnect.is_none() && zombie.is_none() && self.per_ip_full(from, false) {
            log::info!("{from}: connect refused, its address holds too many slots");
            self.send_oob(from, crate::engine::link::PER_IP_REFUSAL);
            return;
        }
        let Some(slot) = reconnect.or(zombie).or_else(|| {
            // A new client: a matching `sv_privatePassword` (an empty one
            // matches an absent `password`) searches from slot 0, anyone
            // else from `sv_privateClients` (0x8085a03).
            let private_pw = self.live_cvar("sv_privatePassword", "");
            let start = if info_value_for_key(&userinfo, "password").unwrap_or("") == private_pw {
                0
            } else {
                self.private_clients().max(0) as usize
            };
            self.free_slot_from(start)
        }) else {
            log::debug!("Rejected a connection.");
            self.send_oob(from, "error\nEXE_SERVERISFULL");
            return;
        };
        // `ClientConnect`'s verdict, after the engine's own checks
        // (0x8085baa); a denied connect leaves the slot as it was.
        if self.password_denied(&userinfo, false) {
            log::debug!("Game rejected a connection: GAME_INVALIDPASSWORD.");
            self.send_oob(from, "error\nGAME_INVALIDPASSWORD");
            return;
        }
        if reconnect.is_some() {
            log::info!("{from}: reconnect");
            // 1.5's and UO's `SV_FreeClient` (1.5b 0x808a6e3, UO 0x808b4e7)
            // runs the game's `ClientDisconnect` here, inside the connect.
            // 1.1d frees only the client's script id (0x80859c8) and the
            // game's `ClientConnect` starts the held player over: no
            // callback, and its threads live on (cod11-server-handshake.md,
            // "A reconnect into a held slot").
            if self.v15() {
                self.core.clients[slot] = None;
                self.client_disconnect(slot);
            }
        } else {
            self.game.zombies[slot] = None;
        }
        // `Info_SetValueForKey(userinfo, "ip", NET_AdrToString(from))`
        // (0x8085498, key at 0x80d43da): any `ip` the client sent goes, the real one is last.
        let userinfo = format!(
            "{}\\ip\\{}:{}",
            remove_info_key(&userinfo, "ip"),
            from.ip(),
            from.port() as i16
        );
        let mut client = Client::new(from, qport, challenge, userinfo, now);
        client.link.last_packet_ms = self.svs_time_ms();
        log::info!("client {slot} {:?} connected from {from}", client.name);
        let name = client.name.clone();
        self.core.clients[slot] = Some(client);
        self.game.udp_slots |= 1 << slot;
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.push_client_event(ClientEvent::Connect { slot, name });
        }
        // A taken connect clears the stored ping (0x8085c16).
        self.core.challenges.taken(ci);
        self.send_oob(from, "connectResponse");
        // The first client, or the last the server holds (0x8085cd3).
        let count = self.client_count();
        if count == 1 || count == self.core.clients.len() {
            self.core.masters.force();
        }
    }

    /// Whether `from`'s address holds its share of slots
    /// (`engine::link::per_ip_full`); bots and zombies hold none.
    fn per_ip_full(&self, from: SocketAddr, exact: bool) -> bool {
        use crate::engine::link;
        let cap = self.live_cvar(link::PER_IP_CVAR, link::PER_IP_DEFAULT);
        let held = self.core.clients.iter().flatten().filter(|c| !c.is_bot);
        link::per_ip_full(
            crate::game::builtins::cvar::atoi(&cap),
            from,
            exact,
            held.map(|c| c.link.addr),
        )
    }

    /// The first slot at `start` or above neither a client nor a zombie
    /// holds.
    pub(super) fn free_slot_from(&self, start: usize) -> Option<usize> {
        (start..self.core.clients.len())
            .find(|&i| self.core.clients[i].is_none() && self.game.zombies[i].is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// The checksum feed rides the seed, not how long the caller took to
    /// build the server: two servers off one seed key their pure checks
    /// and cmds alike.
    #[test]
    fn a_seeded_server_draws_the_same_checksum_feed() {
        let now = Instant::now();
        let a = Server::new(cfg(), now);
        let b = Server::new(cfg(), now - Duration::from_secs(5));
        assert_eq!(a.core.checksum_feed, b.core.checksum_feed);
        let c = Server::with_seed(cfg(), now, 7);
        assert_ne!(a.core.checksum_feed, c.core.checksum_feed);
    }

    #[test]
    fn getinfo_echoes_the_challenge_and_names_the_map() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo 4242"), now);
        let (to, cmd, rest) = reply(&mut sv);
        assert_eq!(to, addr(5));
        assert_eq!(cmd, "infoResponse");
        let info = String::from_utf8_lossy(&rest);
        assert_eq!(info_value_for_key(&info, "challenge"), Some("4242"));
        assert_eq!(info_value_for_key(&info, "mapname"), Some("mp_carentan"));
        assert_eq!(info_value_for_key(&info, "protocol"), Some("1"));
        assert_eq!(info_value_for_key(&info, "clients"), Some("0"));
        assert_eq!(info_value_for_key(&info, "sv_maxclients"), Some("4"));
        assert_eq!(info_value_for_key(&info, "gametype"), Some("dm"));
        assert_eq!(info_value_for_key(&info, "pswrd"), Some("0"));
        // Key order is retail's; pin the whole string.
        assert_eq!(
            info,
            "\\challenge\\4242\\protocol\\1\\hostname\\vcod test\\mapname\\mp_carentan\\clients\\0\\sv_maxclients\\4\\gametype\\dm\\pure\\0\\sv_allowAnonymous\\0\\pswrd\\0"
        );
    }

    /// `SVC_Info` adds `game` (`fs_game`) after `pure` when it is set.
    #[test]
    fn getinfo_names_the_mod() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("fs_game", "mymod");
        sv.handle_packet(addr(5), &oob("getinfo 1"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest);
        assert!(info.ends_with("\\pure\\0\\game\\mymod\\sv_allowAnonymous\\0\\pswrd\\0"));
    }

    #[test]
    fn getinfo_sanitizes_the_challenge_argument() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo a\\pure\\1 extra"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "infoResponse");
        let info = String::from_utf8_lossy(&rest);
        assert_eq!(info_value_for_key(&info, "challenge"), Some("apure1"));
        assert_eq!(info_value_for_key(&info, "pure"), Some("0"));
    }

    #[test]
    fn getstatus_has_serverinfo_and_no_player_lines() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "statusResponse");
        let text = String::from_utf8_lossy(&rest);
        let (info, players) = text.split_once('\n').unwrap();
        assert_eq!(info_value_for_key(info, "sv_hostname"), Some("vcod test"));
        assert_eq!(info_value_for_key(info, "challenge"), Some("7"));
        assert_eq!(players, "");
    }

    /// `Cmd_Argv(1)` of a query, against what 1.1d, 1.5b and UO 1.51b all
    /// echoed as `challenge` (cod11-server-handshake.md, "The challenge
    /// key"); the info string's own stripping (`;`, `\`) comes after.
    #[test]
    fn oob_argv_reads_the_line_as_retail_does() {
        let cases: [(&[u8], &str); 12] = [
            (b"getinfo a\xc3\xa9b", "a..b"),
            (b"getinfo a%b", "a.b"),
            (b"getinfo a\x7fb", "a\x7fb"),
            (b"getinfo a\x01b", "a"),
            (b"getinfo a\nb", "a"),
            (b"getinfo a\"b", "a"),
            (b"getinfo \"a b\" c", "a b"),
            (b"getinfo a//b", "a"),
            (b"getinfo a/*x*/b", "a"),
            (b"getinfo /*x*/b", "b"),
            (b"getinfo \"ab", "ab"),
            (b"getinfo", ""),
        ];
        for (line, arg) in cases {
            let pkt = [&[0xff; 4][..], line].concat();
            assert_eq!(oob_argv(&pkt, 1), arg, "{line:?}");
        }
    }

    /// No argument, no `challenge` key: retail's `Info_SetValueForKey`
    /// leaves an empty value out, on every protocol.
    #[test]
    fn a_bare_query_sends_no_challenge_key() {
        let now = Instant::now();
        for proto in [&PROTOCOL_V1, &PROTOCOL_V6, &PROTOCOL_V22] {
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(proto);
            for (n, (query, answer)) in
                [("getinfo", "infoResponse"), ("getstatus", "statusResponse")]
                    .into_iter()
                    .enumerate()
            {
                sv.handle_packet(addr(10 + n as u16), &oob(query), now);
                let (_, cmd, rest) = reply(&mut sv);
                assert_eq!(cmd, answer);
                let text = String::from_utf8_lossy(&rest);
                assert!(
                    !text.contains("challenge"),
                    "{} {text}",
                    proto.transport.version
                );
            }
        }
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo"), now);
        let (_, _, rest) = reply(&mut sv);
        assert_eq!(
            String::from_utf8_lossy(&rest),
            "\\protocol\\1\\hostname\\vcod test\\mapname\\mp_carentan\\clients\\0\\sv_maxclients\\4\\gametype\\dm\\pure\\0\\sv_allowAnonymous\\0\\pswrd\\0"
        );
    }

    /// A challenge that takes the info string past 1024 bytes: 1.1d drops
    /// its level or crashes, so vcod sends nothing; 1.5b and UO skip each
    /// pair that no longer fits and answer with the rest.
    #[test]
    fn an_overlong_challenge_overflows_as_retail_does() {
        use vcod_common::net::connectionless::MAX_INFO_STRING;
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo x"), now);
        let base = reply(&mut sv).2.len() - "\\challenge\\x".len();
        let fits = MAX_INFO_STRING - base - "\\challenge\\".len();
        sv.handle_packet(addr(6), &oob(&format!("getinfo {}", "a".repeat(fits))), now);
        assert_eq!(reply(&mut sv).2.len(), MAX_INFO_STRING);
        sv.handle_packet(
            addr(7),
            &oob(&format!("getinfo {}", "a".repeat(fits + 1))),
            now,
        );
        assert!(sv.take_outgoing().is_empty());

        let mut sv = v6_server(now);
        let long = "a".repeat(1000);
        sv.handle_packet(addr(5), &oob(&format!("getinfo {long}")), now);
        let (_, _, rest) = reply(&mut sv);
        assert_eq!(
            String::from_utf8_lossy(&rest),
            format!("\\challenge\\{long}\\protocol\\6")
        );
        sv.handle_packet(addr(6), &oob(&format!("getstatus {long}")), now);
        let (_, _, rest) = reply(&mut sv);
        let text = String::from_utf8_lossy(&rest);
        assert!(!text.contains("challenge"), "{text}");
        assert!(text.ends_with("\\pswrd\\0\\mod\\1\n"), "{text}");
    }

    /// `fs_restrict` puts `demo` in front of `sv_keywords` (empty here) and
    /// the key after `challenge`; `getinfo` carries no keywords.
    #[test]
    fn fs_restrict_adds_the_demo_keyword() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("fs_restrict", "1");
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, _, rest) = reply(&mut sv);
        let text = String::from_utf8_lossy(&rest);
        assert!(
            text.ends_with("\\challenge\\7\\sv_keywords\\demo \\pswrd\\0\n"),
            "{text}"
        );
        sv.handle_packet(addr(7), &oob("getinfo 7"), now);
        let (_, _, rest) = reply(&mut sv);
        assert!(!String::from_utf8_lossy(&rest).contains("demo"));
    }

    /// 1.5b's `infoResponse` against the capture in
    /// docs/research/cod15-protocol-diff.md 1: the zero-valued keys left out
    /// and five more after them. `kc` is `scr_killcam`'s string.
    #[test]
    fn protocol_6_answers_getinfo_as_15b_does() {
        let now = Instant::now();
        let mut sv = v6_server(now);
        sv.set_cvar("scr_killcam", "1");
        sv.handle_packet(addr(5), &oob("getinfo xyz"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "infoResponse");
        assert_eq!(
            String::from_utf8_lossy(&rest),
            "\\challenge\\xyz\\protocol\\6\\hostname\\vcod test\\mapname\\mp_carentan\\sv_maxclients\\4\\gametype\\dm\\kc\\1\\hw\\1\\pb\\0\\mod\\1"
        );
        // A password and a client: `clients` counts the private slots too,
        // and `sv_maxclients` gives back the ones in use.
        sv.set_cvar("g_password", "secret");
        sv.set_cvar("sv_privateClients", "2");
        sv.core.clients[0] = Some(Client::new(addr(9), 1, 0, "\\name\\x".into(), now));
        sv.handle_packet(addr(5), &oob("getinfo 1"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest);
        assert!(
            info.contains("\\clients\\1\\sv_maxclients\\3\\gametype\\dm\\pswrd\\1\\kc\\1"),
            "{info}"
        );
    }

    /// 1.5b refuses `+set sv_punkbuster 1` as read-only and answers
    /// `pb 0`; vcod runs no PunkBuster either.
    #[test]
    fn protocol_6_refuses_punkbuster_on_the_command_line() {
        let now = Instant::now();
        let mut sv = v6_server(now);
        sv.set_cvar("sv_punkbuster", "1");
        sv.handle_packet(addr(5), &oob("getinfo x"), now);
        let (_, _, rest) = reply(&mut sv);
        assert!(String::from_utf8_lossy(&rest).ends_with("\\pb\\0\\mod\\1"));
    }

    /// 1.5b's `statusResponse` ends `challenge`, `pswrd`, `mod`, and its
    /// serverinfo names protocol 6.
    #[test]
    fn protocol_6_answers_getstatus_as_15b_does() {
        let now = Instant::now();
        let mut sv = v6_server(now);
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "statusResponse");
        let text = String::from_utf8_lossy(&rest);
        let (info, _) = text.split_once('\n').unwrap();
        assert_eq!(info_value_for_key(info, "protocol"), Some("6"));
        assert_eq!(info_value_for_key(info, "shortversion"), Some("1.5"));
        assert!(info.ends_with("\\challenge\\7\\pswrd\\0\\mod\\1"), "{info}");
    }

    /// A 1.1 client is turned away with 1.5's version, which a 1.1 client
    /// shows and a vcod client retries in (`net::NetClient`).
    #[test]
    fn protocol_6_refuses_a_11_connect_with_its_version() {
        let now = Instant::now();
        let mut sv = v6_server(now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(addr(5), &connect_pkt(challenge, QPORT, 1), now);
        assert_eq!(
            reply_text(&mut sv),
            (
                "error".to_string(),
                "EXE_SERVER_IS_DIFFERENT_VER\x151.5".to_string()
            )
        );
        sv.handle_packet(addr(5), &connect_pkt(challenge, QPORT, 6), now);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
    }

    fn uo_server(now: Instant) -> Server {
        let mut sv = Server::new(cfg(), now);
        sv.set_protocol(&PROTOCOL_V22);
        sv
    }

    /// UO 1.51b's `infoResponse` (coduo-diff.md 2, live on mp_cassino):
    /// 1.5's keys with the vehicle switches `jps` and `tnk` after
    /// `timeoutsAllowed`, and its refusal of another version's connect.
    #[test]
    fn protocol_22_answers_getinfo_and_refuses_as_uo_does() {
        let now = Instant::now();
        let mut sv = uo_server(now);
        // What the level's table holds on retail's stock cfg; no level runs
        // here.
        for name in [
            "scr_friendlyfire",
            "scr_killcam",
            "scr_allow_jeeps",
            "scr_allow_tanks",
        ] {
            sv.set_cvar(name, "1");
        }
        sv.handle_packet(addr(5), &oob("getinfo xyz"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "infoResponse");
        assert_eq!(
            String::from_utf8_lossy(&rest),
            "\\challenge\\xyz\\protocol\\22\\hostname\\vcod test\\mapname\\mp_carentan\\sv_maxclients\\4\\gametype\\dm\\ff\\1\\kc\\1\\jps\\1\\tnk\\1\\hw\\1\\pb\\0\\mod\\1"
        );
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "statusResponse");
        let status = String::from_utf8_lossy(&rest);
        // The serverinfo half is the level's cvar table, measured byte for
        // byte against retail in coduo-diff.md 20; the trailer is 1.5's.
        assert!(
            status.contains("\\protocol\\22\\")
                && status.contains("\\shortversion\\1.51\\")
                && status.ends_with("\\challenge\\7\\pswrd\\0\\mod\\1\n"),
            "{status}"
        );
        let challenge = challenge_for(&mut sv, addr(6), now);
        for refused in [1, 6] {
            sv.handle_packet(addr(6), &connect_pkt(challenge, QPORT, refused), now);
            assert_eq!(
                reply_text(&mut sv),
                (
                    "error".to_string(),
                    "EXE_SERVER_IS_DIFFERENT_VER\x151.51".to_string()
                )
            );
        }
        sv.handle_packet(addr(6), &connect_pkt(challenge, QPORT, 22), now);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
    }

    /// A UO server's entities and playerstate go out in UO's numbering: the
    /// `eFlags` bits from 0x100 one place down, the events renumbered, an
    /// event entity's `eType` from `ET_EVENTS` 16 (coduo-diff.md 8, 11).
    #[test]
    fn protocol_22_writes_uos_event_and_flag_numbers() {
        use vcod_common::net::event_ids::{EV_FIRE_WEAPON, EV_OBITUARY};
        use vcod_common::net::flags::EF_FIRING;
        let p = &PROTOCOL_V22;
        let mut es = msg::EntityState::null(p);
        let set = |es: &mut msg::EntityState, name: &str, v: i32| {
            let i = msg::EntityState::field_index(p, name).unwrap();
            es.fields[i] = v;
        };
        set(&mut es, "eFlags", EF_FIRING | 0x20);
        set(&mut es, "events[1]", EV_FIRE_WEAPON);
        set(
            &mut es,
            "eType",
            vcod_common::net::events::ET_EVENTS + EV_OBITUARY,
        );
        crate::game::wire::entity_to_protocol(p, &mut es);
        assert_eq!(es.field_i32(p, "eFlags"), 0x200 | 0x20);
        assert_eq!(es.field_i32(p, "events[1]"), 163);
        assert_eq!(es.field_i32(p, "eType"), 16 + 209);
        // 1.1 is left alone.
        let mut es = msg::EntityState::null(&PROTOCOL_V1);
        let i = msg::EntityState::field_index(&PROTOCOL_V1, "eFlags").unwrap();
        es.fields[i] = EF_FIRING;
        crate::game::wire::entity_to_protocol(&PROTOCOL_V1, &mut es);
        assert_eq!(es.fields[i], EF_FIRING);
    }

    #[test]
    fn getchallenge_is_stable_per_address() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getchallenge"), now);
        let (_, cmd, a) = reply(&mut sv);
        assert_eq!(cmd, "challengeResponse");
        sv.handle_packet(addr(5), &oob("getchallenge"), now);
        let (_, _, b) = reply(&mut sv);
        assert_eq!(a, b);
        sv.handle_packet(addr(6), &oob("getchallenge"), now);
        let (_, _, c) = reply(&mut sv);
        assert_ne!(a, c);
        assert!(String::from_utf8_lossy(&a).trim().parse::<i32>().is_ok());
    }

    #[test]
    fn a_valid_connect_takes_a_slot() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(
            addr(5),
            &connect_pkt(challenge, QPORT, PROTOCOL_V1.transport.version),
            now,
        );
        let (to, cmd, _) = reply(&mut sv);
        assert_eq!(to, addr(5));
        assert_eq!(cmd, "connectResponse");
        assert_eq!(sv.client_count(), 1);
    }

    /// Bots sit at loopback ports and qports a real client can share; a
    /// connect from one never takes a bot's slot (BUG-109).
    #[test]
    fn a_loopback_connect_at_a_bots_qport_takes_its_own_slot() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut bot = Client::new(addr(65535), 0x7000, 1, "\\name\\bot".into(), now);
        bot.is_bot = true;
        sv.core.clients[0] = Some(bot);
        let later = now + Duration::from_secs(10);
        let challenge = challenge_for(&mut sv, addr(65535), later);
        sv.handle_packet(
            addr(65535),
            &connect_pkt(challenge, 0x7000, PROTOCOL_V1.transport.version),
            later,
        );
        assert_eq!(sv.client_count(), 2);
        assert!(sv.core.clients[0].as_ref().is_some_and(|c| c.is_bot));
    }

    /// A reconnect overwrites a slot the server may still believe is live
    /// (`sv_timeout` is 240 s, `RECONNECT_LIMIT` 3). 1.5's and UO's
    /// `SV_FreeClient` runs the disconnect callback inside the connect; 1.1d
    /// runs none and starts the held player over, its threads and script
    /// fields kept and `pers` fresh (`client-probes/probe_reconnect`).
    /// 1.1's reconnect zeroes the held player's entity without an unlink:
    /// the area tree keeps the entry where it was filed, and a walk reads
    /// it with no contents.
    #[test]
    fn a_reconnect_on_1_1_leaves_the_area_entry() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        let nc = connected(&mut sv, addr(5), now);
        sv.tick(now);
        sv.take_outgoing();
        fn area(sv: &mut Server) -> &mut crate::area::AreaTree {
            &mut sv.game.module.script.as_mut().unwrap().host.area
        }
        let link = crate::area::Link::boxed([0.0; 3], [-15.0; 3], [15.0; 3], 0x2000000);
        area(&mut sv).link(0, &link);
        let filed = area(&mut sv).filed(0);
        assert!(filed.is_some());
        let t = now + RECONNECT_LIMIT + Duration::from_millis(100);
        sv.handle_packet(addr(5), &connect_pkt(nc.challenge, QPORT, 1), t);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        sv.tick(t);
        assert_eq!(area(&mut sv).filed(0), filed, "still filed where it was");
        assert_eq!(
            area(&mut sv).contents(0),
            0,
            "the walk reads zeroed contents"
        );
    }

    #[test]
    fn a_reconnect_into_a_held_slot_follows_the_protocol() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        for (proto, callbacks) in [(&PROTOCOL_V1, 0), (&PROTOCOL_V6, 1)] {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(proto);
            install_script(
                &mut sv,
                ScriptRuntime::for_test_at(
                    CALLBACK_SETUP,
                    "main() { level.gone = 0; level.gens = 0; level.ticks = 0; }\n\
                     CodeCallback_PlayerConnect() { level.field = isdefined(self.mark); \
                       level.pers = isdefined(self.pers[\"mark\"]); self.mark = 1; \
                       self.pers[\"mark\"] = 1; level.gens++; self thread tick(level.gens); }\n\
                     tick(gen) { for(;;) { wait 0.05; if (gen == 1) level.ticks++; } }\n\
                     CodeCallback_PlayerDisconnect() { level.gone++; }\n",
                ),
            );
            let nc = connected(&mut sv, addr(5), now);
            sv.tick(now);
            sv.take_outgoing();
            let field =
                |sv: &mut Server, f: &str| sv.game.module.script.as_mut().unwrap().level_field(f);

            // Past sv_reconnectlimit, the same peer's connect takes the slot back.
            let t = now + RECONNECT_LIMIT + Duration::from_millis(100);
            sv.handle_packet(
                addr(5),
                &connect_pkt(nc.challenge, QPORT, proto.transport.version),
                t,
            );
            assert_eq!(reply_text(&mut sv).0, "connectResponse");
            sv.tick(t);
            let ticks = field(&mut sv, "ticks");
            sv.tick(t + Duration::from_millis(100));
            let v = proto.transport.version;
            assert_eq!(
                field(&mut sv, "gone"),
                vcod_gsc::Value::Int(callbacks),
                "{v}"
            );
            assert_eq!(field(&mut sv, "gens"), vcod_gsc::Value::Int(2), "{v}");
            assert_eq!(field(&mut sv, "pers"), vcod_gsc::Value::Int(0), "{v}");
            let held = callbacks == 0;
            assert_eq!(
                field(&mut sv, "field"),
                vcod_gsc::Value::Int(held as i32),
                "{v}"
            );
            let ticking = field(&mut sv, "ticks") != ticks;
            assert_eq!(ticking, held, "{v}: the old player's thread");
            assert_eq!(sv.client_count(), 1);
        }
    }

    /// `addTestClient`'s first bot (port 0) inside the first 3 s is refused
    /// while a slot no client ever took is left, and taken once a UDP client
    /// has held every slot: an IP address never matches a bot's in
    /// `SV_DirectConnect`'s reconnect check (cod_lnxded 0x8080e60, object
    /// model doc 25).
    #[test]
    fn the_first_test_client_is_refused_only_beside_a_never_used_slot() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        for every_slot_held in [false, true] {
            let now = Instant::now();
            let mut c = cfg();
            c.max_clients = 2;
            let mut sv = Server::new(c, now);
            install_script(
                &mut sv,
                ScriptRuntime::for_test_at(
                    CALLBACK_SETUP,
                    "main() { level.bot = -1; thread add(); }\n\
                     add() { wait 2.5; level.bot = isdefined(addtestclient()); }\n\
                     CodeCallback_PlayerConnect() {}\n\
                     CodeCallback_PlayerDisconnect() {}\n",
                ),
            );
            connected(&mut sv, addr(5), now);
            if every_slot_held {
                let challenge = challenge_for(&mut sv, addr(6), now);
                let v = sv.game.proto.transport.version;
                sv.handle_packet(addr(6), &connect_pkt(challenge, QPORT + 1, v), now);
                assert_eq!(reply_text(&mut sv).0, "connectResponse");
                sv.drop_client(1, "EXE_DISCONNECTED");
            }
            for ms in (0..=2800).step_by(50) {
                sv.tick(now + Duration::from_millis(ms));
            }
            assert!(sv.svs_time_ms() < 3000);
            let bot = sv.game.module.script.as_mut().unwrap().level_field("bot");
            assert_eq!(
                bot,
                vcod_gsc::Value::Int(every_slot_held as i32),
                "every slot held: {every_slot_held}"
            );
        }
    }

    #[test]
    fn a_connect_on_the_wrong_protocol_is_rejected() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(addr(5), &connect_pkt(challenge, QPORT, 6), now);
        assert_eq!(
            reply_text(&mut sv),
            (
                "error".to_string(),
                "EXE_SERVER_IS_DIFFERENT_VER\x151.1".to_string()
            )
        );
        assert_eq!(sv.client_count(), 0);
    }

    #[test]
    fn a_connect_with_a_challenge_we_never_issued_is_rejected() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(
            addr(5),
            &connect_pkt(
                challenge.wrapping_add(1),
                QPORT,
                PROTOCOL_V1.transport.version,
            ),
            now,
        );
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_BAD_CHALLENGE".to_string())
        );
        assert_eq!(sv.client_count(), 0);
    }

    #[test]
    fn a_full_server_rejects_the_next_connect() {
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                max_clients: 1,
                ..cfg()
            },
            now,
        );
        connected(&mut sv, addr(5), now);
        assert_eq!(sv.client_count(), 1);

        // Neither qport nor port matches, so not a reconnect of the first client.
        let challenge = challenge_for(&mut sv, addr(6), now);
        sv.handle_packet(
            addr(6),
            &connect_pkt(challenge, QPORT + 1, PROTOCOL_V1.transport.version),
            now,
        );
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_SERVERISFULL".to_string())
        );
        assert_eq!(sv.client_count(), 1);
    }

    /// One public address gets `vc_maxClientsPerIP` slots, from any ports
    /// and qports: past them its connect and its next `getchallenge` are
    /// refused, a held socket still gets a challenge, and another address
    /// still gets in.
    #[test]
    fn one_public_address_takes_at_most_four_slots() {
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                max_clients: 8,
                ..cfg()
            },
            now,
        );
        let version = PROTOCOL_V1.transport.version;
        let wan = |port: u16| SocketAddr::from(([203, 0, 113, 5], port));
        let refused = (
            "error".to_string(),
            "\u{15}Too many clients from your address".to_string(),
        );
        // Each challenge on its own clock: a tie replaces the first slot.
        let challenges: Vec<i32> = (0..5)
            .map(|i| {
                sv.core.sv_time_ms += 50;
                challenge_for(&mut sv, wan(i), now)
            })
            .collect();
        for (i, &c) in challenges.iter().enumerate() {
            sv.handle_packet(
                wan(i as u16),
                &connect_pkt(c, QPORT + i as u16, version),
                now,
            );
            let want = if i < 4 { "connectResponse" } else { "error" };
            assert_eq!(reply_text(&mut sv).0, want, "connect {i}");
        }
        assert_eq!(sv.client_count(), 4);
        sv.handle_packet(wan(9), &oob("getchallenge"), now);
        assert_eq!(reply_text(&mut sv), refused);
        challenge_for(&mut sv, wan(0), now);
        let other = SocketAddr::from(([203, 0, 113, 6], 28960));
        let c = challenge_for(&mut sv, other, now);
        sv.handle_packet(other, &connect_pkt(c, QPORT, version), now);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        // 0 turns the cap off.
        sv.set_cvar("vc_maxClientsPerIP", "0");
        let c = challenge_for(&mut sv, wan(9), now);
        sv.handle_packet(wan(9), &connect_pkt(c, QPORT + 9, version), now);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
    }

    fn connect_with(
        sv: &mut Server,
        from: SocketAddr,
        extra: &str,
        now: Instant,
    ) -> (String, String) {
        let challenge = challenge_for(sv, from, now);
        let ui = format!(
            "\\name\\vcod{extra}\\protocol\\{}\\qport\\{QPORT}\\challenge\\{challenge}",
            PROTOCOL_V1.transport.version
        );
        sv.handle_packet(from, &build_connect(&ui), now);
        reply_text(sv)
    }

    /// `sv_minPing` / `sv_maxPing` against the challenge ping, off the LAN
    /// only, and the two `getinfo` keys that appear while either is set
    /// (handshake doc, "Ping limits").
    #[test]
    fn ping_limits_refuse_a_connect_off_the_lan() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("sv_minPing", "100");
        sv.set_cvar("sv_maxPing", "250");
        let wan = SocketAddr::from(([203, 0, 113, 7], 27960));
        // The ping is `svs.time` at the connect less at the answer: the
        // server's clock, not the packets' arrival.
        let connect_after = |sv: &mut Server, from, ms| {
            let challenge = challenge_for(sv, from, now);
            sv.core.sv_time_ms += ms as i32;
            let ui = format!(
                "\\name\\vcod\\protocol\\{}\\qport\\{QPORT}\\challenge\\{challenge}",
                PROTOCOL_V1.transport.version
            );
            sv.handle_packet(from, &build_connect(&ui), now + Duration::from_millis(ms));
            reply_text(sv)
        };
        assert_eq!(
            connect_after(&mut sv, wan, 50),
            ("error".into(), "EXE_ERR_HIGH_PING_ONLY".into())
        );
        assert_eq!(
            connect_after(&mut sv, addr(9), 50).0,
            "connectResponse",
            "a LAN client skips both tests"
        );
        sv.handle_packet(addr(20), &oob("getinfo x"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest).to_string();
        assert_eq!(info_value_for_key(&info, "minPing"), Some("100"));
        assert_eq!(info_value_for_key(&info, "maxPing"), Some("250"));
        let wan2 = SocketAddr::from(([203, 0, 113, 8], 27960));
        assert_eq!(
            connect_after(&mut sv, wan2, 400),
            ("error".into(), "EXE_ERR_LOW_PING_ONLY".into())
        );
        let wan3 = SocketAddr::from(([203, 0, 113, 9], 27960));
        assert_eq!(connect_after(&mut sv, wan3, 150).0, "connectResponse");
    }

    /// Retail 1.1d with `sv_privateClients 2` (handshake doc, "Private
    /// slots"): a client without `sv_privatePassword` searches from slot 2,
    /// one with it from 0, a reconnect keeps its slot either way, and an
    /// empty private password matches a client that sends none.
    #[test]
    fn private_slots_take_the_private_password() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("sv_privateClients", "2");
        sv.set_cvar("sv_privatePassword", "pp");
        let slot_of = |sv: &Server, from: SocketAddr| {
            sv.core
                .clients
                .iter()
                .position(|c| c.as_ref().is_some_and(|c| c.link.addr == from))
        };
        // One ip each: the reply limiter would refuse a sixth challenge to one.
        let addr = |n: u8| SocketAddr::from(([10, 0, 1, n], 28960));
        let full = ("error".to_string(), "EXE_SERVERISFULL".to_string());
        for (n, extra, slot) in [(5, "", 2), (6, "\\password\\wrong", 3)] {
            assert_eq!(
                connect_with(&mut sv, addr(n), extra, now).0,
                "connectResponse"
            );
            assert_eq!(slot_of(&sv, addr(n)), Some(slot));
        }
        assert_eq!(connect_with(&mut sv, addr(7), "", now), full);
        assert_eq!(
            connect_with(&mut sv, addr(8), "\\password\\pp", now).0,
            "connectResponse"
        );
        assert_eq!(slot_of(&sv, addr(8)), Some(0));

        let browser = addr(20);
        sv.handle_packet(browser, &oob("getinfo x"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest).to_string();
        assert_eq!(info_value_for_key(&info, "clients"), Some("2"));
        assert_eq!(info_value_for_key(&info, "sv_maxclients"), Some("2"));
        sv.handle_packet(browser, &oob("getstatus x"), now);
        let (_, _, rest) = reply(&mut sv);
        let status = String::from_utf8_lossy(&rest).to_string();
        let serverinfo = status.lines().next().unwrap_or("");
        assert_eq!(
            info_value_for_key(serverinfo, "sv_privateClients"),
            Some("2")
        );

        let later = now + RECONNECT_LIMIT + Duration::from_secs(1);
        assert_eq!(
            connect_with(&mut sv, addr(8), "", later).0,
            "connectResponse"
        );
        assert_eq!(slot_of(&sv, addr(8)), Some(0), "a reconnect keeps its slot");

        sv.set_cvar("sv_privatePassword", "");
        assert_eq!(
            connect_with(&mut sv, addr(9), "", later).0,
            "connectResponse"
        );
        assert_eq!(slot_of(&sv, addr(9)), Some(1));
    }

    /// Retail 1.1d with `g_password secret` (handshake doc, "g_password"):
    /// no password, a wrong one and a wrong case are refused before a slot
    /// is taken; the right one connects.
    #[test]
    fn g_password_gates_the_connect() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("g_password", "secret");
        let denied = ("error".to_string(), "GAME_INVALIDPASSWORD".to_string());
        for (i, extra) in ["", "\\password\\wrong", "\\password\\Secret"]
            .iter()
            .enumerate()
        {
            assert_eq!(
                connect_with(&mut sv, addr(5 + i as u16), extra, now),
                denied,
                "{extra:?}"
            );
        }
        assert_eq!(sv.client_count(), 0);
        let ok = connect_with(&mut sv, addr(9), "\\password\\secret", now);
        assert_eq!(ok.0, "connectResponse");
        assert_eq!(sv.client_count(), 1);
    }

    /// `none` still reads `pswrd 1` but lets anyone in; an empty value is 0.
    #[test]
    fn pswrd_reports_any_g_password_and_none_admits_everyone() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let pswrd = |sv: &mut Server, query: &str| {
            sv.handle_packet(addr(5), &oob(&format!("{query} x")), now);
            let (_, _, rest) = reply(sv);
            let text = String::from_utf8_lossy(&rest).to_string();
            info_value_for_key(text.lines().next().unwrap_or(""), "pswrd").map(str::to_string)
        };
        for (value, flag) in [("", "0"), ("secret", "1"), ("none", "1")] {
            sv.set_cvar("g_password", value);
            assert_eq!(
                pswrd(&mut sv, "getinfo").as_deref(),
                Some(flag),
                "{value:?}"
            );
            assert_eq!(
                pswrd(&mut sv, "getstatus").as_deref(),
                Some(flag),
                "{value:?}"
            );
        }
        assert_eq!(connect_with(&mut sv, addr(6), "", now).0, "connectResponse");
    }

    use crate::engine::ratelimit::{ADDR_BURST, ADDR_PERIOD, GLOBAL_BURST, GLOBAL_PERIOD};

    fn count_replies(sv: &mut Server, to: SocketAddr) -> usize {
        sv.take_outgoing().iter().filter(|(a, _)| *a == to).count()
    }

    #[test]
    fn getstatus_is_rate_limited_per_address() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let a = SocketAddr::from(([10, 0, 0, 1], 5));
        let b = SocketAddr::from(([10, 0, 0, 2], 5));
        for _ in 0..ADDR_BURST + 1 {
            sv.handle_packet(a, &oob("getstatus x"), now);
        }
        assert_eq!(count_replies(&mut sv, a), ADDR_BURST as usize);
        // One global period on, so the burst above is not what limits b.
        let t1 = now + GLOBAL_PERIOD;
        sv.handle_packet(b, &oob("getstatus x"), t1);
        assert_eq!(count_replies(&mut sv, b), 1);
        // The same bucket covers getinfo; one token comes back per period.
        sv.handle_packet(a, &oob("getinfo x"), t1);
        assert_eq!(count_replies(&mut sv, a), 0);
        let t2 = now + ADDR_PERIOD;
        sv.handle_packet(a, &oob("getinfo x"), t2);
        assert_eq!(count_replies(&mut sv, a), 1);
        sv.handle_packet(a, &oob("getinfo x"), t2);
        assert_eq!(count_replies(&mut sv, a), 0);
    }

    #[test]
    fn status_replies_share_a_global_bucket() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut answered = 0;
        for i in 0..GLOBAL_BURST + 5 {
            let from = SocketAddr::from(([10, 1, (i >> 8) as u8, i as u8], 5));
            sv.handle_packet(from, &oob("getinfo x"), now);
            answered += count_replies(&mut sv, from);
        }
        assert_eq!(answered, GLOBAL_BURST as usize);
        // A challenge is not held off by the spent global bucket.
        let joiner = SocketAddr::from(([10, 2, 0, 2], 5));
        sv.handle_packet(joiner, &oob("getchallenge"), now);
        assert_eq!(count_replies(&mut sv, joiner), 1);
        let late = SocketAddr::from(([10, 2, 0, 1], 5));
        sv.handle_packet(late, &oob("getinfo x"), now + GLOBAL_PERIOD);
        assert_eq!(count_replies(&mut sv, late), 1);
    }

    /// Retail matches an OOB `disconnect` and drops nothing (0x808c827).
    #[test]
    fn an_oob_disconnect_is_ignored() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        connected(&mut sv, addr(5), now);
        sv.take_outgoing();
        sv.handle_packet(addr(5), &oob("disconnect"), now);
        assert_eq!(sv.client_count(), 1);
        assert!(sv.take_outgoing().is_empty());
    }

    /// A forged packet carrying the victim's ip and qport must not advance the
    /// netchan, move the address or refresh the timeout; the real client's
    /// next packet still goes through.
    #[test]
    fn a_spoofed_packet_leaves_the_client_untouched() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut eof = MsgWriter::new(&huff);
        eof.write_bits(CLC_EOF, 2);
        let eof = eof.into_ops();
        let later = now + Duration::from_secs(5);
        let spoofer = addr(6);
        let mut forged = Netchan::new(QPORT, nc.challenge);
        let snapshot = |sv: &Server| {
            let c = sv.core.clients[0].as_ref().unwrap();
            (c.link.netchan.incoming_sequence, c.link.addr, c.last_packet)
        };
        let before = snapshot(&sv);

        // Header checks: a messageAcknowledge of a message we never sent.
        forged.outgoing_sequence = 0x7fff_fffe;
        let pkt = forged.build_out(0, 1, 0, &eof, &huff).unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(snapshot(&sv), before, "a bad ack committed state");

        // A stale serverId with nothing to resend.
        forged.outgoing_sequence = 0x7fff_fffe;
        let pkt = forged
            .build_out(i32::from(sv.game.server_id) ^ 0x01, 0, 0, &eof, &huff)
            .unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(snapshot(&sv), before, "a stale serverId committed state");

        // Right header, ops that end inside a command.
        forged.outgoing_sequence = 0x7fff_fffe;
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(1);
        let pkt = forged
            .build_out(i32::from(sv.game.server_id), 0, 0, &w.into_ops(), &huff)
            .unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(
            snapshot(&sv),
            before,
            "a truncated op stream committed state"
        );
        assert!(sv.take_outgoing().is_empty());

        // The legitimate client's first message, sequence 1, asks for the gamestate.
        let pkt = nc.build_out(0, 0, 0, &eof, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, later);
        assert_eq!(snapshot(&sv), (1, addr(5), later));
        assert!(
            !sv.take_outgoing().is_empty(),
            "no gamestate after the spoofs"
        );
    }

    #[test]
    fn a_foreign_challenge_cannot_replace_a_live_client() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // A loaded level's clock: at 0 the client's slot, written at 0,
        // ties the unwritten ones and goes first to the next challenge.
        sv.start_clock_at(300);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        let eof = w.into_ops();
        let own = nc.challenge;

        // The client is heard from just before the attempt.
        let t1 = now + RECONNECT_LIMIT + Duration::from_millis(100);
        let pkt = nc.build_out(0, 0, 0, &eof, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, t1);
        sv.take_outgoing();

        // A neighbour behind the same NAT holds a valid challenge for this ip
        // and knows the qport. Past sv_reconnectlimit, retail would hand it the slot.
        let foreign = challenge_for(&mut sv, addr(7), t1);
        let t2 = t1 + Duration::from_millis(100);
        sv.handle_packet(
            addr(7),
            &connect_pkt(foreign, QPORT, PROTOCOL_V1.transport.version),
            t2,
        );
        assert!(sv.take_outgoing().is_empty(), "the takeover got a reply");
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!((c.link.addr, c.link.netchan.challenge), (addr(5), own));

        // The client's own connect retry, same challenge, still resets its slot.
        sv.handle_packet(
            addr(5),
            &connect_pkt(own, QPORT, PROTOCOL_V1.transport.version),
            t2,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        assert_eq!(sv.client_count(), 1);

        // Once the slot has been silent for sv_reconnectlimit, a crashed client
        // coming back with a new challenge may reclaim it.
        let t3 = t2 + RECONNECT_LIMIT + Duration::from_millis(100);
        sv.handle_packet(
            addr(7),
            &connect_pkt(foreign, QPORT, PROTOCOL_V1.transport.version),
            t3,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        assert_eq!(sv.client_count(), 1);
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!((c.link.addr, c.link.netchan.challenge), (addr(7), foreign));
    }

    #[test]
    fn a_challenge_never_expires_and_binds_the_port() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let old = challenge_for(&mut sv, addr(5), now);
        // An hour on, asking again gives the same slot (retail has no TTL).
        let later = now + Duration::from_secs(3600);
        assert_eq!(challenge_for(&mut sv, addr(5), later), old);
        // Another port of the same host is another address (0x8080dec).
        sv.handle_packet(
            addr(6),
            &connect_pkt(old, QPORT, PROTOCOL_V1.transport.version),
            later,
        );
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_BAD_CHALLENGE".to_string())
        );
        sv.handle_packet(
            addr(5),
            &connect_pkt(old, QPORT, PROTOCOL_V1.transport.version),
            later,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
    }

    /// `gameCompleteStatus` over rcon (handshake doc, "Console commands
    /// over rcon"): a master the heartbeat resolved first is named and sent
    /// nothing; one the command resolves itself gets the serverinfo with
    /// the argument, if any, as `challenge` and a line per player.
    #[test]
    fn game_complete_status_reaches_only_the_masters_it_resolved() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_master_resolver(local_master);
        sv.set_cvar("rconPassword", "pw");
        sv.set_cvar("dedicated", "2");
        let _ = connected(&mut sv, addr(5), t);
        sv.tick(t);
        sv.take_outgoing();
        let master = addr(crate::master::DEFAULT_MASTER.port);
        let ask = |sv: &mut Server, line: &str, at: Instant| {
            sv.handle_packet(addr(9), &oob(&format!("rcon pw {line}")), at);
            let out = sv.take_outgoing();
            let text = |to: SocketAddr| -> Vec<String> {
                out.iter()
                    .filter(|(a, _)| *a == to)
                    .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
                    .collect()
            };
            (text(addr(9)), text(master))
        };
        let (reply, sent) = ask(&mut sv, "gameCompleteStatus abc", t);
        assert_eq!(
            reply,
            ["print\nSending gameCompleteStatus to master.cod.pm\n"]
        );
        assert!(sent.is_empty(), "{sent:?}");
        sv.set_cvar("sv_master2", "127.0.0.1");
        // A second past the heartbeat, which went to the same host.
        let at = t + Duration::from_millis(1500);
        sv.tick_frames(at, 30);
        let (reply, sent) = ask(&mut sv, "gameCompleteStatus abc", at);
        assert_eq!(
            reply,
            ["print\nSending gameCompleteStatus to master.cod.pm\n\
              Resolving 127.0.0.1\n127.0.0.1 resolved to 127.0.0.1:20510\n\
              Sending gameCompleteStatus to 127.0.0.1\n"]
        );
        assert_eq!(sent.len(), 1);
        let (head, players) = sent[0]
            .split_once('\n')
            .unwrap()
            .1
            .split_once('\n')
            .unwrap();
        assert!(sent[0].starts_with("gameCompleteStatus\n\\"), "{sent:?}");
        assert!(head.ends_with("\\challenge\\abc"), "{head}");
        assert!(!head.contains("pswrd"), "{head}");
        assert_eq!(players.lines().count(), 1, "{players}");
        let at = t + Duration::from_millis(3000);
        sv.tick_frames(at, 30);
        let (_, sent) = ask(&mut sv, "gameCompleteStatus", at);
        assert!(!sent[0].contains("challenge"), "{sent:?}");
    }
}
