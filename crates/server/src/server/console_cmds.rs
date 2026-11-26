//! The server console: its commands, kick and ban, status and cvars.

use super::*;

impl Server {
    /// What [`Self::drain_console`] does with a level load's outcome.
    /// Returns whether the console may keep running.
    fn absorb_load(&mut self, what: &str, r: Result<(), LoadFailure>) -> bool {
        match r {
            Ok(()) => true,
            Err(LoadFailure::KeptLevel(e)) => {
                log::error!("{what}: {e:#}");
                true
            }
            Err(LoadFailure::Fatal(e)) => {
                self.game.fatal = Some(e.context(what.to_string()));
                false
            }
        }
    }

    /// A command-line `+set` placed after `+map`: `Com_AddStartupCommands`
    /// queues it behind the map, so it runs as a console `set` once the
    /// level has loaded, over the script's load-time writes (map-cycle doc
    /// 4.5).
    pub fn push_set(&mut self, name: &str, value: &str) {
        self.push_console(&format!("set {name} \"{value}\""));
    }

    /// `Cbuf_Execute`: every queued line through [`Self::exec_console_line`].
    /// A command pushed by a builtin during the script pass runs here, at the
    /// top of the next tick, which is `EXEC_APPEND`. The frame loop runs it
    /// ahead of [`crate::engine::tick`], as `Com_Frame` runs it ahead of
    /// `SV_Frame`, so the time a command took is owed by the frames after it.
    pub fn drain_console(&mut self, now: Instant) {
        while let Some(text) = self.core.console.pop_front() {
            let mut lines = console::split_commands(&text).into_iter();
            let Some(line) = lines.next() else {
                continue;
            };
            for rest in lines.rev() {
                self.core.console.push_front(rest);
            }
            if !self.exec_console_line(&line, now) || self.core.quit {
                return;
            }
        }
    }

    /// `Cmd_ExecuteString` for the commands the map cycle and rcon use, then
    /// `Cvar_Command`; anything else logs and drops, and prints nothing, as
    /// retail's does for a command no one registered. `false` when a load left no level,
    /// which stops the drain.
    ///
    /// A load that failed before the teardown (no paks, a map the paks do
    /// not have, a bsp that would not parse) leaves the level that is
    /// serving alone and is logged. One that failed after it (the map or
    /// gametype scripts) has no level left to fall back to, so it ends the
    /// process through [`Self::take_fatal`].
    pub(super) fn exec_console_line(&mut self, line: &str, now: Instant) -> bool {
        self.game.argv1 = crate::game::say::tokenize(line)
            .into_iter()
            .nth(1)
            .unwrap_or_default();
        match console::Command::parse(line) {
            console::Command::Map { map, bsp, cheats } => {
                if self
                    .game
                    .fs
                    .as_ref()
                    .is_some_and(|fs| fs.resolve_map(&map).is_none())
                {
                    self.print(&format!("Can't find map {bsp}\n"));
                    return true;
                }
                // `map <the map already serving>` is a restart, not a
                // spawn (doc section 4.2).
                let r = if map.eq_ignore_ascii_case(&self.game.cfg.map)
                    && self.game.module.script.is_some()
                {
                    self.map_restart()
                } else {
                    self.spawn_server(&map)
                };
                if !self.absorb_load(&format!("map {map}"), r) {
                    return false;
                }
                // After the load, so the gamestate it sent still carries the
                // old value and the change follows as a configstring update.
                self.set_cheats(cheats);
                if self.game.module.script.is_some() {
                    self.game.killed = false;
                }
            }
            console::Command::MapRestart => {
                let _p = crate::tick_profile::phase("console/map_restart");
                let r = self.map_restart();
                return self.absorb_load("map_restart", r);
            }
            console::Command::MapRotate => {
                // Both rotation cvars are the one table a script's
                // `setCvar` and `getCvar` reach on retail (doc section 5.2).
                let mut full = self.game.sv_map_rotation.clone();
                if let Some(rt) = self.game.module.script.as_ref() {
                    let script = rt.host.cvars.get("sv_mapRotation");
                    if !script.is_empty() {
                        full = script.to_string();
                    }
                    self.game.rotation.current =
                        rt.host.cvars.get("sv_mapRotationCurrent").to_string();
                }
                let before = self.game.cfg.gametype.clone();
                let mut gametype = before.clone();
                let next = self.game.rotation.rotate(&full, &mut gametype);
                if let Some(rt) = self.game.module.script.as_mut() {
                    rt.host
                        .cvars
                        .set("sv_mapRotationCurrent", &self.game.rotation.current);
                }
                for w in self.game.rotation.warnings.drain(..) {
                    log::warn!("{w}");
                }
                if gametype != before {
                    // Retail's plain `Cvar_Set` (doc section 5.2), so
                    // through `set_cvar`: writing `cfg.gametype` alone
                    // picks the new gametype's script while `cvars`
                    // replays an earlier `--set g_gametype` over the
                    // table that script then reads.
                    self.set_cvar("g_gametype", &gametype);
                    // A gametype change discards what exitLevel(true)
                    // asked to keep (doc section 5.2).
                    if let Some(rt) = self.game.module.script.as_mut() {
                        rt.host.save_persist = false;
                    }
                }
                match next {
                    console::Rotate::Map(m) => self.core.console.push_front(format!("map {m}")),
                    console::Rotate::Restart => self.core.console.push_front("map_restart".into()),
                }
            }
            console::Command::Status => {
                let text = self.status_text(now);
                self.print(&text);
            }
            console::Command::ClientKick(arg) => self.client_kick(arg.as_deref(), now),
            console::Command::Kick(arg) => self.kick(arg.as_deref(), now),
            console::Command::AddBot(args) => bot_admin::add_bot_command(self, &args, &mut ()),
            console::Command::RemoveBot(arg) => {
                bot_admin::remove_bot_command(self, arg.as_deref(), &mut ())
            }
            console::Command::ListBots => bot_admin::list_bots(self, &mut ()),
            console::Command::DumpUser(arg) => self.dump_user(arg.as_deref()),
            console::Command::ServerInfo => {
                let info = self.core.configstrings.first().cloned().unwrap_or_default();
                self.print(&format!(
                    "Server info settings:\n{}",
                    console::info_print(&info)
                ));
            }
            console::Command::SystemInfo => {
                let info = self.core.configstrings.get(1).cloned().unwrap_or_default();
                self.print(&format!(
                    "System info settings:\n{}",
                    console::info_print(&info)
                ));
            }
            console::Command::Say(text) => {
                if let Some(text) = text {
                    self.console_say(&text);
                }
            }
            console::Command::Set { cmd, args: None } => {
                self.print(&format!("usage: {cmd} <variable> <value>\n"));
            }
            console::Command::Set {
                cmd,
                args: Some((name, value)),
            } => {
                self.console_set(&name, &value);
                // `Cvar_SetA_f`: the archive flag whatever the write did.
                if cmd == "seta"
                    && let Some(rt) = self.game.module.script.as_mut()
                {
                    rt.host.cvars.add_flags(&name, crate::cvars::flag::ARCHIVE);
                }
            }
            console::Command::Heartbeat => self.core.masters.force(),
            console::Command::GameCompleteStatus(arg) => self.game_complete_status(&arg),
            console::Command::ScriptUsage => {
                if let Some(text) = self.game.module.script.as_mut().map(|rt| rt.script_usage()) {
                    self.print(&text);
                }
            }
            console::Command::StringUsage => {
                if let Some(text) = self.game.module.script.as_ref().map(|rt| rt.string_usage()) {
                    self.print(&text);
                }
            }
            console::Command::Quit => {
                self.final_message("EXE_SERVERQUIT");
                self.shutdown_game_log();
                let mut resolve = self.core.resolver;
                let beats = self.core.masters.shutdown(
                    self.game.dedicated,
                    self.svs_time_ms(),
                    &mut resolve,
                );
                self.core.outbox.extend(beats);
                self.core.quit = true;
            }
            console::Command::KillServer => self.kill_server(),
            console::Command::BanUser(arg) => {
                let Some(arg) = arg else {
                    self.print("Usage: banUser <player name>\n");
                    return true;
                };
                if let Some(slot) = self.player_by_name(&arg) {
                    self.ban_slot(slot, now);
                }
            }
            console::Command::BanClient(arg) => {
                let Some(arg) = arg else {
                    self.print("Usage: banClient <client number>\n");
                    return true;
                };
                if let Some(slot) = self.player_by_num(&arg) {
                    self.ban_slot(slot, now);
                }
            }
            console::Command::Unban(arg) if self.v15() => {
                let Some(arg) = arg else {
                    self.print("Usage: unban <client name>\n");
                    return true;
                };
                let name = crate::game::say::clean_name(&arg);
                match self.core.bans.unban(&name) {
                    Some(0) => self.print(&format!("no banned user has name {name}\n")),
                    Some(n) => self.print(&format!("unbanned {n} user(s) named {name}\n")),
                    None => {}
                }
            }
            console::Command::CvarList(filter) => {
                let text = match self.game.module.script.as_ref() {
                    Some(rt) => rt.cvars().list(filter.as_deref()),
                    None => self
                        .game
                        .carried_cvars
                        .clone()
                        .unwrap_or_default()
                        .list(filter.as_deref()),
                };
                self.print(&text);
            }
            console::Command::TempBan { by_num, arg } if self.v15() => {
                let Some(arg) = arg else {
                    self.print(if by_num {
                        "Usage: tempBanClient <client number>\n"
                    } else {
                        "Usage: tempBanUser <player name>\n"
                    });
                    return true;
                };
                let slot = if by_num {
                    self.player_by_num(&arg)
                } else {
                    self.player_by_name(&arg)
                };
                if let Some(slot) = slot {
                    self.temp_ban_slot(slot, now);
                }
            }
            console::Command::SetScr { word, name, value } if self.v15() => {
                let Some(value) = value else {
                    self.print(&format!("Usage: {word} <integer>\n"));
                    return true;
                };
                for cvar in [format!("scr_{name}"), format!("ui_{name}")] {
                    self.set_cvar(&cvar, &value);
                    if let Some(rt) = self.game.module.script.as_mut() {
                        rt.host.cvars.set(&cvar, &value);
                    }
                }
            }
            console::Command::TempBan { .. }
            | console::Command::SetScr { .. }
            | console::Command::Unban(_) => {
                log::warn!("console: unknown command {line:?}");
            }
            console::Command::Unknown(argv) => {
                if !self.cvar_command(&argv) {
                    log::warn!("console: unknown command {line:?}");
                }
            }
        }
        true
    }

    /// `SV_KickNum_f` (0x8084be4) and `SV_GetPlayerByNum` (0x8083b9c). A
    /// zombie counts as a client here: kicking one only restarts its clock.
    fn client_kick(&mut self, arg: Option<&str>, now: Instant) {
        let Some(arg) = arg else {
            self.print("Usage: kicknum <client number>\n");
            return;
        };
        if let Some(n) = self.player_by_num(arg) {
            self.kick_slot(n, now);
        }
    }

    /// `SV_GetPlayerByNum` (0x8083b9c): the slot in use that `arg` names,
    /// or `None` after printing why not.
    pub(super) fn player_by_num(&mut self, arg: &str) -> Option<usize> {
        if !arg.bytes().all(|b| b.is_ascii_digit()) {
            self.print(&format!("Bad slot number: {arg}\n"));
            return None;
        }
        let n = arg.parse::<usize>().unwrap_or(usize::MAX);
        if n >= self.core.clients.len() {
            self.print(&format!(
                "Bad client slot: {}\n",
                arg.parse::<i64>().unwrap_or(-1)
            ));
            return None;
        }
        if self.core.clients[n].is_none() && self.game.zombies[n].is_none() {
            self.print(&format!("Client {n} is not active\n"));
            return None;
        }
        Some(n)
    }

    /// The tail both bans share (0x8084394, 0x8084524). Retail 1.1 sends
    /// `banUser <ip>` to the authorize server and leaves the client
    /// connected; vcod keeps the client's guid in `ban.txt` instead, where
    /// 1.5 keeps its own (docs/research/cod11-server-handshake.md, "Bans").
    fn ban_slot(&mut self, slot: usize, now: Instant) {
        let Some(c) = self.core.clients[slot]
            .as_ref()
            .or(self.game.zombies[slot].as_ref())
        else {
            return;
        };
        let name = if self.core.clients[slot].is_some() {
            c.name.clone()
        } else {
            String::new()
        };
        let guid = guid_of(crate::engine::Lan::Cod1, c.link.addr);
        if self.v15() {
            self.ban_slot_v15(slot, guid, &name, now);
            return;
        }
        // A LAN client's guid is 0, which no check refuses, as the
        // authorize server is never asked about one.
        if guid != 0 && !self.core.bans.is_banned(guid) {
            self.core
                .bans
                .append(guid, &crate::game::say::clean_name(&name));
        }
        self.print(&format!("{name} was banned from coming back\n"));
    }

    /// 1.5's ban tail (cod_lnxded 1.5b 0x8089121): nothing for a client
    /// without a guid or one already in `ban.txt`, else the guid and the
    /// cleaned name appended and the client kicked, with no print
    /// (docs/research/cod11-server-handshake.md, "Permanent bans on 1.5").
    fn ban_slot_v15(&mut self, slot: usize, guid: i32, name: &str, now: Instant) {
        if guid == 0 || self.core.bans.is_banned(guid) {
            return;
        }
        self.core
            .bans
            .append(guid, &crate::game::say::clean_name(name));
        self.kick_slot(slot, now);
    }

    /// 1.5's `tempBanUser`/`tempBanClient` past the lookup (cod_lnxded 1.5b
    /// 0x8087f2a, 0x8088087): the kick, then, for a client with a guid,
    /// `%s (guid %i) was kicked for cheating` and the guid into the table
    /// (docs/research/cod11-server-handshake.md, "Bans").
    fn temp_ban_slot(&mut self, slot: usize, now: Instant) {
        let banned = self.core.clients[slot].as_ref().map(|c| {
            (
                guid_of(crate::engine::Lan::Cod1, c.link.addr),
                crate::game::say::clean_name(&c.name),
            )
        });
        self.kick_slot(slot, now);
        if let Some((guid, name)) = banned
            && guid != 0
        {
            self.print(&format!("{name} (guid {guid}) was kicked for cheating\n"));
            // 0x80890f5 stamps the game clock (0x83ccd84).
            self.core.temp_bans.add(guid, self.core.sv_time_ms);
        }
    }

    /// `sv_kickBanTime`, read at each check (0x8088f75), so a change
    /// reaches bans already made.
    pub(super) fn kick_ban_time(&self) -> f32 {
        crate::game::builtins::cvar::atof(&self.live_cvar("sv_kickBanTime", "300"))
    }

    /// Replaces `ban.txt`; the binary opens it from `--ban-file`.
    pub fn set_bans(&mut self, bans: BanFile) {
        self.core.bans = bans;
    }

    /// The console's commands for this version, then every cvar, for the
    /// tty console's tab ([`crate::tty_console`]).
    pub(crate) fn console_names(&self) -> Vec<String> {
        let extra: &[&str] = if self.v15() {
            console::COMMANDS_15
        } else {
            &[]
        };
        let mut names: Vec<String> = console::COMMANDS
            .iter()
            .chain(extra)
            .map(|c| c.to_string())
            .collect();
        let cvars = match self.game.module.script.as_ref() {
            Some(rt) => Some(rt.cvars()),
            None => self.game.carried_cvars.as_ref(),
        };
        names.extend(
            cvars
                .into_iter()
                .flat_map(|c| c.names())
                .map(str::to_string),
        );
        names
    }

    /// `SV_KillServer_f` (0x8084d3c) into `SV_Shutdown("EXE_SERVERKILLED")`:
    /// the final message to every client, the flatline to the masters,
    /// every slot freed and the level gone. The process stays; it reads
    /// no packet and runs no frame until a console `map` loads a level
    /// (docs/research/cod11-server-handshake.md, "Shutdown").
    pub(super) fn kill_server(&mut self) {
        self.final_message("EXE_SERVERKILLED");
        self.shutdown();
    }

    /// `Com_Error(ERR_DROP)` (cod_lnxded 0x806b93c) on a dedicated server:
    /// the `ERROR:` banner, then `SV_Shutdown` with `com_errorEntered` set,
    /// which skips `SV_FinalMessage`, so a client hears nothing and times
    /// out. The process stays, idle, like after `killserver`
    /// (docs/research/cod11-movers.md 18).
    pub(super) fn drop_level(&mut self, error: &str) {
        log::error!("ERROR: {error}");
        self.shutdown();
    }

    /// `SV_Shutdown` past its final message.
    pub(super) fn shutdown(&mut self) {
        if let Some(rt) = self.game.module.script.as_ref() {
            self.game.shutdown_log = rt.script_log().to_vec();
        }
        self.shutdown_game_log();
        let mut resolve = self.core.resolver;
        let beats =
            self.core
                .masters
                .shutdown(self.game.dedicated, self.svs_time_ms(), &mut resolve);
        self.core.outbox.extend(beats);
        // The cvar table is process-global and outlives the shutdown.
        let _ = self.lift_persistence();
        self.game.module.script = None;
        self.core.clients.iter_mut().for_each(|c| *c = None);
        self.game.zombies.iter_mut().for_each(|z| *z = None);
        self.core.challenges.clear();
        self.game.module.driver.bots.clear();
        self.game.module.pending_explosions.clear();
        self.game.module.weapon_changes.clear();
        self.game.killed = true;
    }

    /// The level's `G_LogPrintf` lines since the last call into the file.
    pub(crate) fn drain_game_log(&mut self) {
        if let Some(rt) = self.game.module.script.as_mut() {
            let lines = std::mem::take(&mut rt.host.game_log);
            self.game.game_log.write(lines);
        }
    }

    /// `G_ShutdownGame`: the level's last lines, `ShutdownGame:` and the
    /// separator at its last `level.time`, the file closed.
    pub(super) fn shutdown_game_log(&mut self) {
        self.close_game_log(crate::engine::gamelog::SHUTDOWN);
    }

    /// [`Self::shutdown_game_log`] with `word`: 1.1's `G_ShutdownGame(1)`
    /// writes `RestartGame:` (VERIFIED live, cod11-map-cycle.md 9).
    pub(super) fn close_game_log(&mut self, word: &str) {
        if let Some(rt) = self.game.module.script.as_mut() {
            let lines = std::mem::take(&mut rt.host.game_log);
            let at = rt.host.level_time_ms;
            self.game.game_log.shutdown(lines, at, word);
        }
    }

    /// `sv_cheats`, which only `SV_Map_f` writes: into systeminfo on the
    /// next frame's flush, and into the cvar table a query reads.
    fn set_cheats(&mut self, on: bool) {
        self.game.cheats = on;
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.cvars.set("sv_cheats", if on { "1" } else { "0" });
        }
        self.refresh_fall_heights();
    }

    /// The drop both kicks end in; a zombie's clock restarts, so a kicked
    /// zombie only lingers longer.
    pub(super) fn kick_slot(&mut self, slot: usize, now: Instant) {
        self.drop_client(slot, "EXE_PLAYERKICKED");
        let svs_ms = self.svs_time_ms();
        if let Some(z) = self.game.zombies[slot].as_mut() {
            z.last_packet = now;
            z.link.last_packet_ms = svs_ms;
        }
    }

    /// `SV_GetPlayerByName` (0x8083aa0): the first slot in use whose name
    /// matches `name` case-insensitively, as is or with its colour codes
    /// stripped. A zombie's name is gone with its userinfo, so it matches
    /// only an empty one.
    pub(super) fn player_by_name(&mut self, name: &str) -> Option<usize> {
        let found = (0..self.core.clients.len()).find(|&slot| {
            let n = match (&self.core.clients[slot], &self.game.zombies[slot]) {
                (Some(c), _) => c.name.as_str(),
                (None, Some(_)) => "",
                (None, None) => return false,
            };
            n.eq_ignore_ascii_case(name)
                || crate::game::say::clean_name(n).eq_ignore_ascii_case(name)
        });
        if found.is_none() {
            self.print(&format!("Player {name} is not on the server\n"));
        }
        found
    }

    /// `SV_Kick_f` (0x8084288). The name is looked up first, so `kick all`
    /// prints the miss before it kicks every slot in use.
    pub(super) fn kick(&mut self, arg: Option<&str>, now: Instant) {
        let Some(arg) = arg else {
            self.print("Usage: kick <player name>\nkick all = kick everyone\n");
            return;
        };
        if let Some(slot) = self.player_by_name(arg) {
            self.kick_slot(slot, now);
        } else if arg.eq_ignore_ascii_case("all") {
            for slot in 0..self.core.clients.len() {
                if self.core.clients[slot].is_some() || self.game.zombies[slot].is_some() {
                    self.kick_slot(slot, now);
                }
            }
        }
    }

    /// `SV_DumpUser_f` (0x8084cc0).
    fn dump_user(&mut self, arg: Option<&str>) {
        let Some(arg) = arg else {
            self.print("Usage: info <userid>\n");
            return;
        };
        let Some(slot) = self.player_by_name(arg) else {
            return;
        };
        let info = self.core.clients[slot]
            .as_ref()
            .map_or_else(String::new, |c| c.link.userinfo.clone());
        self.print(&format!(
            "userinfo\n--------\n{}",
            console::info_print(&info)
        ));
    }

    /// `SV_ConSay_f` (0x8084974): `h "\x15console: <text>"` to every client
    /// past `CS_CONNECTED`, as a droppable command.
    fn console_say(&mut self, text: &str) {
        let cmd = format!("h \"\u{15}console: {text}\"");
        log::info!("say: console: {text}");
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state != ClientState::Connected)
            {
                self.send_server_command(slot, &cmd);
            }
        }
    }

    /// The two `CVAR_LATCH` cvars `Server` keeps in its own fields, by
    /// their folded name: the registered spelling, the value the running
    /// level loaded with, and the value the next load takes.
    fn latched_cvar(&self, name: &str) -> Option<(&'static str, String, String)> {
        match name.to_ascii_lowercase().as_str() {
            "g_gametype" => Some((
                "g_gametype",
                self.live_gametype(),
                self.game.cfg.gametype.clone(),
            )),
            "sv_maxclients" => Some((
                "sv_maxclients",
                self.level_cvar("sv_maxclients")
                    .unwrap_or_else(|| self.game.cfg.max_clients.to_string()),
                self.pending_cvar("sv_maxclients", &self.game.cfg.max_clients.to_string()),
            )),
            _ => None,
        }
    }

    /// A cvar in the running level's table; `None` with no level or no
    /// value. The level stamps `g_gametype` and `sv_maxclients` at load and
    /// a console write never reaches them there, so this is the live half
    /// of the latch.
    pub(super) fn level_cvar(&self, name: &str) -> Option<String> {
        let v = self.game.module.script.as_ref()?.cvars().get(name);
        (!v.is_empty()).then(|| v.to_string())
    }

    /// `g_gametype` as the running level loaded it; serverinfo, `getinfo`
    /// and `getstatus` carry this one until a load takes a latched value.
    pub(super) fn live_gametype(&self) -> String {
        self.level_cvar("g_gametype")
            .unwrap_or_else(|| self.game.cfg.gametype.clone())
    }

    /// `cfg` with the running level's `g_gametype`.
    pub(super) fn live_cfg(&self) -> ServerConfig {
        let mut cfg = self.game.cfg.clone();
        cfg.gametype = self.live_gametype();
        cfg
    }

    /// `Cvar_Set_f`: a read-only, init or (without `sv_cheats`) cheat cvar
    /// refuses, a latched one keeps its value until the next load and says
    /// so, anything else is written to the running level's table at once.
    pub(super) fn console_set(&mut self, name: &str, value: &str) {
        if let Some((canon, live, pending)) = self.latched_cvar(name) {
            let mut waiting = (pending != live).then_some(pending);
            if let Some(msg) = crate::cvars::latch(&mut waiting, &live, name, value) {
                self.print(&msg);
            }
            self.set_cvar(canon, waiting.as_deref().unwrap_or(&live));
            return;
        }
        let cheats = self.game.cheats;
        let outcome = match self.game.module.script.as_mut() {
            Some(rt) => rt.host.cvars.console_set(name, value, cheats),
            None => {
                self.set_cvar(name, value);
                return;
            }
        };
        match outcome {
            crate::cvars::ConsoleSet::Refused(msg) => self.print(&msg),
            crate::cvars::ConsoleSet::Latched(msg) => {
                if let Some(msg) = msg {
                    self.print(&msg);
                }
            }
            crate::cvars::ConsoleSet::Applied => self.apply_engine_cvar(name, value),
        }
    }

    /// `Cvar_Command`: a line whose first word names a cvar prints it, or
    /// sets it to the second word. False when no such cvar exists, which
    /// leaves the line an unknown command.
    fn cvar_command(&mut self, argv: &[String]) -> bool {
        let Some(name) = argv.first() else {
            return false;
        };
        let Some((canon, value, default, waiting)) =
            self.game.module.script.as_ref().and_then(|rt| {
                let cv = rt.cvars();
                let (n, v, d) = cv.lookup(name)?;
                Some((
                    n.to_string(),
                    v.to_string(),
                    d.to_string(),
                    cv.latched(name).map(str::to_string),
                ))
            })
        else {
            return false;
        };
        if let Some(v) = argv.get(1) {
            self.console_set(&canon, v);
            return true;
        }
        let waiting = match self.latched_cvar(&canon) {
            Some((_, live, pending)) => (pending != live).then_some(pending),
            None => waiting,
        };
        let mut text = format!("\"{canon}\" is:\"{value}^7\" default:\"{default}^7\"\n");
        if let Some(pending) = waiting {
            text.push_str(&format!("latched: \"{pending}\"\n"));
        }
        self.print(&text);
        true
    }

    /// A client's `score` script field, 0 without one.
    pub(super) fn client_score(&mut self, slot: usize) -> i32 {
        self.game
            .module
            .script
            .as_mut()
            .and_then(|rt| rt.client_field(slot, "score"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// `SV_Status_f` (0x80846b4).
    fn status_text(&mut self, now: Instant) -> String {
        let mut out = format!(
            "map: {}\nnum score ping name            lastmsg address               qport rate\n\
             --- ----- ---- --------------- ------- --------------------- ----- -----\n",
            self.game.cfg.map
        );
        for slot in 0..self.core.clients.len() {
            let zombie = self.core.clients[slot].is_none();
            let score = if zombie { 0 } else { self.client_score(slot) };
            let Some(c) = self.core.clients[slot]
                .as_ref()
                .or(self.game.zombies[slot].as_ref())
            else {
                continue;
            };
            let row = console::StatusRow {
                num: slot,
                score,
                ping: if zombie {
                    console::Ping::Zombie
                } else if c.state == ClientState::Connected {
                    console::Ping::Connecting
                } else {
                    console::Ping::Ms(c.link.ping)
                },
                // The drop nukes the userinfo, and the name with it.
                name: if zombie { "" } else { &c.name },
                last_msg_ms: now.saturating_duration_since(c.last_packet).as_millis() as i64,
                addr: c
                    .bot_port
                    .map_or(console::Adr::Ip(c.link.addr), console::Adr::Bot),
                qport: c.link.netchan.qport,
                rate: c.rate(self.game.dedicated),
            };
            out.push_str(&row.to_string());
        }
        out.push('\n');
        out
    }

    /// `Com_Printf`: the log, and the rcon reply when one is being collected.
    pub(super) fn print(&mut self, msg: &str) {
        if let Some(r) = self.game.redirect.as_mut() {
            r.print(msg);
        }
        log::info!("{}", msg.trim_end());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// A restart starts the level on the compile the outgoing level loaded;
    /// a map load and a new overlay compile again (map-cycle doc, 4.7).
    /// `games_mp.log` across a `map_restart`, a map load and `quit`, as
    /// the retail 1.1d run wrote it (cod11-map-cycle.md 9): the separator
    /// and `InitGame:` at each `G_InitGame`, `RestartGame:` closing the
    /// restarted level, `ShutdownGame:` the others, the script's
    /// `logPrint` between.
    #[test]
    fn games_mp_log_follows_the_levels() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("fs_homepath", &home.display().to_string());
        sv.overlay_script(
            "maps/mp/gametypes/dm",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); logPrint(\"hi\\n\"); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        sv.tick(now);
        sv.map_restart().expect("the restart failed");
        sv.tick(now);
        sv.spawn_server("mp_carentan").expect("the map load failed");
        sv.tick(now);
        sv.push_console("quit");
        sv.tick(now);
        let text = std::fs::read_to_string(home.join("main/games_mp.log")).unwrap();
        let sep = "-".repeat(60);
        let lines: Vec<String> = text
            .lines()
            .map(|l| match l[7..].strip_prefix("InitGame: \\") {
                Some(info) => {
                    assert!(info.contains("\\mapname\\mp_carentan\\"), "{l}");
                    "InitGame".to_string()
                }
                None => l[7..].to_string(),
            })
            .collect();
        let level = [sep.as_str(), "InitGame", "hi"];
        let mut want: Vec<&str> = level.to_vec();
        want.extend(["RestartGame:", &sep]);
        want.extend(level);
        want.extend(["ShutdownGame:", &sep]);
        want.extend(level);
        want.extend(["ShutdownGame:", &sep]);
        assert_eq!(lines, want, "{text}");
        assert!(text.lines().all(|l| l.as_bytes()[3] == b':'), "{text}");
    }

    /// `scriptUsage` on retail 1.1d, mp_brecourt `tdm` (handshake doc,
    /// "Console commands over rcon"): the gametype's two waits, then the
    /// 32 minefield threads, in code address order, not by file name.
    #[test]
    fn script_usage_orders_files_as_retail() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                map: "mp_brecourt".into(),
                gametype: "tdm".into(),
                ..cfg()
            },
            now,
        );
        sv.set_cvar("developer", "1");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for _ in 0..10 {
            sv.tick(now);
        }
        let text = sv.game.module.script.as_mut().unwrap().script_usage();
        // The var usage numbers are vcod's own VM's; the counts and the
        // order are retail's.
        let heads: Vec<&str> = text
            .lines()
            .filter_map(|l| match l.starts_with("count") {
                true => l.split(',').next(),
                false => l.starts_with("(file").then_some(l),
            })
            .collect();
        assert_eq!(
            heads,
            [
                "count: 1",
                "(file 'maps\\mp\\gametypes\\tdm.gsc', line 992)",
                "count: 1",
                "(file 'maps\\mp\\gametypes\\tdm.gsc', line 1184)",
                "count: 32",
                "(file 'maps\\mp\\_minefields.gsc', line 19)",
            ],
            "{text}"
        );
    }

    /// `scriptUsage` 40 frames into `probe` (a `client-probes` gametype,
    /// its text `src`) on mp_brecourt at `developer`, with `probe_mode`
    /// and its two keys set as the retail runs set them.
    fn usage_under(probe: &str, src: &str, developer: &str, mode: [&str; 3]) -> Option<String> {
        let fs = vcod_common::testing::game_fs_11()?;
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                map: "mp_brecourt".into(),
                gametype: probe.into(),
                ..cfg()
            },
            now,
        );
        sv.overlay_script(&format!("maps/mp/gametypes/{probe}"), src);
        sv.set_cvar("developer", developer);
        for (k, v) in ["probe_mode", "probe_value", "probe_key"].iter().zip(mode) {
            sv.set_cvar(k, v);
        }
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for _ in 0..40 {
            sv.tick(now);
        }
        assert!(sv.script_log().iter().any(|l| l.trim() == "PROBE ready"));
        Some(sv.game.module.script.as_mut().unwrap().script_usage())
    }

    /// A capture's reply, its `#` header dropped.
    fn usage_reply(capture: &str) -> String {
        // `split_inclusive` keeps the `\r` a CRLF source line prints.
        capture
            .split_inclusive('\n')
            .filter(|l| !l.starts_with('#'))
            .collect()
    }

    const PROBE_USAGE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../gsc/tests/fixtures/semantics/client-probes/probe_usage.gsc"
    ));

    /// `scriptUsage` under `probe_usage` on retail 1.1d, mp_brecourt, at
    /// `developer 1` (`tests/fixtures/usage/`, handshake doc, "Console
    /// commands over rcon"), byte for byte: every group's counts, each
    /// position's file, line, source line and caret, the entity and HUD
    /// element lines.
    #[test]
    fn script_usage_counts_as_retail() {
        let Some(ours) = usage_under("probe_usage", PROBE_USAGE, "1", ["", "", ""]) else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let retail = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/usage/mp_brecourt-probe_usage.txt"
        ));
        assert_eq!(ours, usage_reply(retail));
    }

    /// The same at `developer 0`: each position is `@ <offset>`. vcod's
    /// offsets count its own instructions, so only their order is
    /// retail's.
    #[test]
    fn script_usage_prints_offsets_at_developer_0() {
        let Some(ours) = usage_under("probe_usage", PROBE_USAGE, "0", ["", "", ""]) else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let retail = usage_reply(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/usage/mp_brecourt-probe_usage-dev0.txt"
        )));
        let offsets = |t: &str| -> Vec<u32> {
            t.lines()
                .filter_map(|l| l.strip_prefix("@ ")?.parse().ok())
                .collect()
        };
        let mask = |t: &str| -> Vec<String> {
            t.lines()
                .map(|l| match l.starts_with("@ ") {
                    true => "@".to_string(),
                    false => l.to_string(),
                })
                .collect()
        };
        assert_eq!(mask(&ours), mask(&retail), "{ours}");
        let (a, b) = (offsets(&ours), offsets(&retail));
        for i in 0..a.len() {
            for j in 0..a.len() {
                assert_eq!(a[i] < a[j], b[i] < b[j], "{a:?} against {b:?}");
            }
        }
    }

    /// `probe_usage_ents` on retail 1.1d, mp_brecourt (handshake doc,
    /// "Console commands over rcon"): the caret under a path call and a
    /// method call through a pointer, and the entity line under each
    /// `probe_mode`: only the entities script reached, a named radiant key
    /// or the spawn gave an object count.
    #[test]
    fn script_usage_entities_as_retail() {
        const SRC: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../gsc/tests/fixtures/semantics/client-probes/probe_usage_ents.gsc"
        ));
        let Some(base) = usage_under("probe_usage_ents", SRC, "1", ["base", "x", "classname"])
        else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let retail = usage_reply(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/usage/mp_brecourt-probe_usage_ents.txt"
        )));
        let threads = |t: &str| t.split("ent type").next().unwrap().to_string();
        assert_eq!(threads(&base), threads(&retail));
        // The entity line's count and var usage.
        let held = |t: &str| -> (i64, i64) {
            let l = t
                .lines()
                .find(|l| l.starts_with("ent type 'entity'"))
                .unwrap();
            let n: Vec<i64> = l
                .split(|c: char| !c.is_ascii_digit())
                .filter_map(|w| w.parse().ok())
                .collect();
            (n[0], n[1])
        };
        // Retail's entity line per mode (count, var usage), captured.
        let modes: [([&str; 3], i64, i64); 19] = [
            (["base", "x", "classname"], 62, 283),
            (["ref", "x", "classname"], 94, 315),
            (["field", "x", "classname"], 94, 318),
            (["w1", "x", "classname"], 94, 320),
            (["w2", "x", "classname"], 94, 321),
            (["w2n", "x", "classname"], 94, 323),
            (["we", "x", "classname"], 94, 321),
            (["e1", "x", "classname"], 94, 320),
            (["arr", "x", "classname"], 94, 320),
            (["read", "x", "classname"], 94, 316),
            (["isdef", "x", "classname"], 94, 316),
            (["unset", "x", "classname"], 94, 315),
            (["read2", "x", "classname"], 94, 317),
            (["del", "script_model", "classname"], 38, 235),
            (["del", "minefield", "targetname"], 30, 59),
            (["del", "trigger_use", "classname"], 61, 281),
            (["del", "trigger_hurt", "classname"], 61, 281),
            (["del", "misc_mg42", "classname"], 61, 282),
            (["del", "trigger_multiple", "classname"], 27, 53),
        ];
        for (mode, count, usage) in modes {
            let ours = usage_under("probe_usage_ents", SRC, "1", mode).unwrap();
            assert_eq!(held(&ours), (count, usage), "{mode:?}: {ours}");
        }
        // Naming three radiant keys stores them: `export` once,
        // `script_noteworthy` twice, `script_gameobjectname` 27 times
        // (retail 63, 314, one more object for the trigger_lookat).
        const KEYS: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../gsc/tests/fixtures/semantics/client-probes/probe_usage_keys.gsc"
        ));
        let keys = usage_under("probe_usage_keys", KEYS, "1", ["", "", ""]).unwrap();
        assert_eq!(held(&keys), (63, 314));
    }

    /// An rcon line is tokenized whole, comments dropped, rebuilt and
    /// tokenized again; the cvar reads are what retail 1.1d, 1.5b and UO
    /// 1.51b printed (cod11-server-handshake.md, "rcon").
    #[test]
    fn an_rcon_line_loses_its_comments_like_retail() {
        let mut t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.set_cvar("rconPassword", "pw");
        let mut ask = |sv: &mut Server, line: &str| {
            t += Duration::from_millis(500);
            rcon(sv, &format!("pw {line}"), t).concat()
        };
        for (set, value) in [
            ("\"a//b\"", "a"),
            ("\"a b//c\"", "a b//c"),
            ("a/*/b", "a b"),
        ] {
            ask(&mut sv, &format!("set vc_t {set}"));
            assert_eq!(
                ask(&mut sv, "vc_t"),
                format!("print\n\"vc_t\" is:\"{value}^7\" default:\"a^7\"\n"),
                "{set}"
            );
        }
        ask(&mut sv, "sv_hostname a;b\\c");
        sv.handle_packet(addr(10), &oob("getstatus"), t);
        let (_, _, rest) = reply(&mut sv);
        let text = String::from_utf8_lossy(&rest);
        assert!(text.contains("\\sv_hostname\\abc\\"), "{text}");
    }

    /// The replies a retail server gave (docs/research/cod11-server-handshake.md,
    /// "rcon"): the password is checked after a server-wide 500 ms window
    /// that drops anything inside it unanswered.
    #[test]
    fn rcon_answers_like_retail() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        assert_eq!(
            rcon(&mut sv, "x status", t),
            ["print\nNo rconpassword set on the server.\n"]
        );
        sv.set_cvar("rconPassword", "secret");
        let t = t + Duration::from_millis(500);
        assert_eq!(
            rcon(&mut sv, "wrong status", t),
            ["print\nBad rconpassword.\n"]
        );
        assert!(rcon(&mut sv, "secret status", t + Duration::from_millis(499)).is_empty());
        let t = t + Duration::from_millis(500);
        assert_eq!(
            rcon(&mut sv, "secret status", t),
            [
                "print\nmap: mp_carentan\nnum score ping name            lastmsg address               qport rate\n\
              --- ----- ---- --------------- ------- --------------------- ----- -----\n\n"
            ]
        );
        // A command nobody registered prints nothing, and so does an empty one.
        let t = t + Duration::from_millis(500);
        assert_eq!(rcon(&mut sv, "secret bogus_cmd", t), ["print\n"]);
        let t = t + Duration::from_millis(500);
        assert_eq!(rcon(&mut sv, "secret", t), ["print\n"]);
    }

    /// Two requests 510 ms apart read in one tick: retail handles each as it
    /// arrives, so both are answered; the window reads the handling moment,
    /// not the tick's (cod11-server-handshake.md, "rcon").
    #[test]
    fn rcon_s_window_reads_each_request_s_own_moment() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconPassword", "pw");
        let tick = t + Duration::from_millis(560);
        let mut answered = 0;
        for at in [t, t + Duration::from_millis(510)] {
            let pkt = oob("rcon pw bogus_cmd");
            let read = crate::engine::Arrival {
                now: tick,
                arrived: at,
                read: at,
            };
            sv.packet_read(addr(9), &pkt, read);
            answered += sv.take_outgoing().len();
        }
        assert_eq!(answered, 2);
    }

    /// `dedicated 2` heartbeats on the first frame, again when the first
    /// client connects, and flatlines on `quit`; the port is always 20510.
    /// `kick`, `dumpuser`, `serverinfo` and `say` against the replies a
    /// retail server gave (handshake doc, "rcon commands").
    #[test]
    fn rcon_admin_commands_answer_like_retail() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        let mut nc = begun(&mut sv, t);
        sv.tick(t);
        let huff = Huffman::new();
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff);
        }
        let step = Duration::from_millis(500);
        let mut at = t;
        let mut ask = |sv: &mut Server, line: &str| {
            at += step;
            rcon(sv, &format!("pw {line}"), at).concat()
        };
        assert_eq!(
            ask(&mut sv, "kick"),
            "print\nUsage: kick <player name>\nkick all = kick everyone\n"
        );
        assert_eq!(
            ask(&mut sv, "dumpuser nobody"),
            "print\nPlayer nobody is not on the server\n"
        );
        assert_eq!(ask(&mut sv, "dumpuser"), "print\nUsage: info <userid>\n");
        let dump = ask(&mut sv, "dumpuser VCOD");
        assert!(dump.starts_with("print\nuserinfo\n--------\nname                vcod\n"));
        assert!(
            dump.ends_with("ip                  127.0.0.1:5\n"),
            "{dump}"
        );
        let info = ask(&mut sv, "serverinfo");
        assert!(info.starts_with("print\nServer info settings:\ng_gametype          dm\n"));
        assert!(info.contains("sv_hostname         vcod test\n"), "{info}");
        assert_eq!(
            ask(&mut sv, "set foo"),
            "print\nusage: set <variable> <value>\n"
        );

        assert_eq!(ask(&mut sv, "say hello there"), "print\n");
        sv.tick(t);
        let out = sv.take_outgoing();
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(cmds, ["h \"\u{15}console: hello there\""]);

        assert_eq!(
            ask(&mut sv, "kick vcod"),
            "print\n0:vcod EXE_PLAYERKICKED\n"
        );
        assert_eq!(sv.client_count(), 0);
        assert!(sv.game.zombies[0].is_some());
    }

    /// `set` writes the running level's table and a bare name reads it back
    /// in `Cvar_Command`'s format; `g_gametype` is latched until a load.
    #[test]
    fn rcon_set_and_query_cvars_like_retail() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        if let Some(rt) = sv.game.module.script.as_mut() {
            rt.host.cvars.set("g_gametype", "dm");
            rt.host.cvars.set("scr_dm_timelimit", "30");
        }
        sv.set_cvar("rconpassword", "pw");
        let step = Duration::from_millis(500);
        let mut at = t;
        let mut ask = |sv: &mut Server, line: &str| {
            at += step;
            rcon(sv, &format!("pw {line}"), at).concat()
        };
        assert_eq!(ask(&mut sv, "set foo a b c"), "print\n");
        assert_eq!(
            ask(&mut sv, "foo"),
            "print\n\"foo\" is:\"a b c^7\" default:\"a b c^7\"\n"
        );
        assert_eq!(ask(&mut sv, "foo baz"), "print\n");
        assert_eq!(
            ask(&mut sv, "FOO"),
            "print\n\"foo\" is:\"baz^7\" default:\"a b c^7\"\n"
        );
        assert_eq!(ask(&mut sv, "set scr_dm_timelimit 5"), "print\n");
        assert_eq!(sv.script_cvar("scr_dm_timelimit").as_deref(), Some("5"));
        assert_eq!(
            ask(&mut sv, "set g_gametype tdm"),
            "print\ng_gametype will be changed upon restarting.\n"
        );
        assert_eq!(
            ask(&mut sv, "g_gametype"),
            "print\n\"g_gametype\" is:\"dm^7\" default:\"dm^7\"\nlatched: \"tdm\"\n"
        );
        // Latched means serverinfo keeps the running level's value.
        assert!(sv.configstring(0).contains("\\g_gametype\\dm\\"));
        assert_eq!(ask(&mut sv, "set g_gametype tdm"), "print\n");
        assert_eq!(
            ask(&mut sv, "set g_gametype sd"),
            "print\ng_gametype will be changed upon restarting.\n"
        );
        // The live value clears the latch silently.
        assert_eq!(ask(&mut sv, "set g_gametype dm"), "print\n");
        assert_eq!(
            ask(&mut sv, "g_gametype"),
            "print\n\"g_gametype\" is:\"dm^7\" default:\"dm^7\"\n"
        );
        assert_eq!(
            ask(&mut sv, "set g_useGear 0"),
            "print\ng_useGear will be changed upon restarting.\n"
        );
        assert_eq!(
            ask(&mut sv, "g_useGear"),
            "print\n\"g_useGear\" is:\"1^7\" default:\"1^7\"\nlatched: \"0\"\n"
        );
        assert_eq!(ask(&mut sv, "set sv_hostname newname"), "print\n");
        assert!(sv.configstring(0).contains("\\sv_hostname\\newname"));
    }

    /// `quit` runs `SV_FinalMessage("EXE_SERVERQUIT")` and exits without
    /// answering the rcon that asked for it.
    #[test]
    fn quit_tells_the_clients_and_answers_nothing() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        let mut nc = begun(&mut sv, t);
        sv.tick(t);
        let huff = Huffman::new();
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff);
        }
        sv.handle_packet(addr(9), &oob("rcon pw quit"), t);
        assert!(sv.quit_requested());
        let out = sv.take_outgoing();
        assert!(
            out.iter().all(|(to, _)| *to == addr(5)),
            "an rcon reply went out"
        );
        assert_eq!(out.len(), 2);
        assert_eq!(
            server_commands(&mut nc, &out[0].1, &huff),
            ["e \"EXE_SERVERQUIT\"", "w"]
        );
        sv.tick(t);
        assert!(sv.take_outgoing().is_empty());
    }

    /// `SV_FinalMessage`'s snapshots are built fresh off a clock that has
    /// not moved: a cmd parsed after the last frame shows in their
    /// `commandTime`, as on 1.1d (handshake doc, Shutdown).
    #[test]
    fn the_final_snapshots_carry_the_cmds_parsed_since_the_frame() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
            prop_areas: None,
        });
        sv.set_cvar("rconpassword", "pw");
        let huff = Huffman::new();
        let p = &PROTOCOL_V1;
        let mut nc = begun(&mut sv, t);
        let mut ring = SnapshotRing::new();
        let send_cmd = |sv: &mut Server, nc: &mut Netchan, server_time: i32, at: Instant| {
            let ack = nc.incoming_sequence as i32;
            let cmd = UserCmd {
                server_time,
                ..Default::default()
            };
            let ops = move_ops(sv.core.checksum_feed, ack, cmd);
            sv.handle_packet(
                addr(5),
                &nc.build_out(0x10, ack, 0, &ops, &huff).unwrap(),
                at,
            );
        };
        let at = t + Duration::from_millis(50);
        send_cmd(&mut sv, &mut nc, 40, at);
        let last = latest_snapshot(&mut sv, &mut nc, &mut ring, at);
        assert_eq!(last.ps.field_i32(p, "commandTime"), 40);
        send_cmd(&mut sv, &mut nc, 45, at);
        sv.handle_packet(addr(9), &oob("rcon pw quit"), at);
        let out = sv.take_outgoing();
        assert_eq!(out.len(), 2);
        for (_, pkt) in out {
            let msgbytes = nc.process_in(&pkt, &huff).unwrap().unwrap();
            let mut r = MsgReader::new(&msgbytes[4..], &huff);
            loop {
                match r.read_byte() {
                    msg::SVC_SERVER_COMMAND => {
                        r.read_long();
                        r.read_big_string();
                    }
                    SVC_SNAPSHOT => break,
                    op => panic!("op {op} before the snapshot"),
                }
            }
            let s = ring.parse_into(&mut r, p, nc.incoming_sequence).unwrap();
            assert_eq!(s.server_time, last.server_time);
            assert_eq!(s.ps.field_i32(p, "commandTime"), 45);
        }
    }

    /// A stdin line reaches the game's cvars and `quit` ends the server,
    /// run by the console as an rcon line is (cod11-server-handshake.md,
    /// "The stdin console").
    #[test]
    fn a_stdin_line_runs_on_the_console() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        crate::stdin_console::StdinConsole::read_now(b"set scr_dm_timelimit 5\nquit\n")
            .queue(&mut sv);
        sv.drain_console(t);
        assert_eq!(sv.script_cvar("scr_dm_timelimit").as_deref(), Some("5"));
        assert!(sv.quit_requested());
    }

    /// The tty console's tab asks the frame loop for its names: the
    /// version's commands, then the cvars.
    #[test]
    fn a_tab_gets_the_commands_and_cvars() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.set_cvar("scr_dm_timelimit", "5");
        let names = crate::stdin_console::StdinConsole::names_now(&mut sv);
        assert_eq!(names[0], "map");
        assert!(names.iter().any(|n| n == "killserver"));
        assert!(
            !names.iter().any(|n| n == "tempBanUser"),
            "1.1 has no tempBanUser"
        );
        assert!(names.iter().any(|n| n == "scr_dm_timelimit"));
    }

    /// `banUser` and `banClient` against the replies a retail server gave
    /// (handshake doc, "Bans"). The client stays connected, as on retail; a
    /// banned address off the LAN is refused its next challenge, and a LAN
    /// one is not, since retail never asks the authorize server about it.
    #[test]
    fn rcon_bans_answer_like_retail() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        let _nc = begun(&mut sv, t);
        sv.take_outgoing();
        let mut at = t;
        let mut ask = |sv: &mut Server, line: &str| {
            at += Duration::from_millis(500);
            rcon(sv, &format!("pw {line}"), at).concat()
        };
        for (line, want) in [
            ("banUser", "Usage: banUser <player name>\n"),
            ("banUser a b", "Usage: banUser <player name>\n"),
            ("banUser nobody", "Player nobody is not on the server\n"),
            ("banClient", "Usage: banClient <client number>\n"),
            ("banClient 3", "Client 3 is not active\n"),
            ("banClient x", "Bad slot number: x\n"),
            ("banUser VCOD", "vcod was banned from coming back\n"),
            ("banClient 0", "vcod was banned from coming back\n"),
        ] {
            assert_eq!(ask(&mut sv, line), format!("print\n{want}"), "{line}");
        }
        assert_eq!(sv.client_count(), 1);
        // A LAN client has no guid: nothing is kept and nothing refused.
        assert_eq!(sv.core.bans.unban("vcod"), None);
        sv.handle_packet(addr(6), &oob("getchallenge"), at);
        assert_eq!(reply(&mut sv).1, "challengeResponse");

        let wan = SocketAddr::from(([203, 0, 113, 7], 28960));
        sv.core
            .bans
            .append(guid_of(crate::engine::Lan::Cod1, wan), "");
        sv.handle_packet(wan, &oob("getchallenge"), at);
        let (to, cmd, rest) = reply(&mut sv);
        assert_eq!(
            (to, cmd.as_str(), rest.as_slice()),
            (wan, "error", &b"EXE_ERR_BAD_CDKEY"[..])
        );
    }

    /// 1.5's temporary ban (handshake doc, "Bans"): refused with retail's
    /// own error while the entry is no older than `sv_kickBanTime`, read at
    /// the check, so a longer time revives an expired ban.
    #[test]
    fn a_temporary_ban_lasts_sv_kickbantime_read_at_the_check() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        let ip = std::net::Ipv4Addr::new(203, 0, 113, 7);
        let wan = SocketAddr::from((ip, 28960));
        let now = sv.core.sv_time_ms;
        let guid = guid_of(crate::engine::Lan::Cod1, wan);
        sv.core.temp_bans.add(guid, now.wrapping_sub(300_000));
        sv.handle_packet(wan, &oob("getchallenge"), t);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(
            (cmd.as_str(), rest.as_slice()),
            (
                "error",
                &b"\x15You are temporarily banned from this server"[..]
            )
        );
        sv.core.temp_bans = crate::engine::bans::TempBans::default();
        sv.core.temp_bans.add(guid, now.wrapping_sub(300_001));
        sv.handle_packet(wan, &oob("getchallenge"), t);
        assert_eq!(reply(&mut sv).1, "challengeResponse");
        sv.set_cvar("sv_kickBanTime", "400");
        sv.handle_packet(wan, &oob("getchallenge"), t);
        assert_eq!(reply(&mut sv).1, "error");
    }

    /// 1.5's `ban.txt` (handshake doc, "Permanent bans on 1.5"): a LAN client
    /// has no guid, so `banUser` answers an empty print and leaves it on,
    /// as the retail 1.5b server did; an off-LAN one is kicked and refused
    /// with retail's string, and `unbanUser` drops it by its cleaned name
    /// with retail's replies.
    #[test]
    fn a_15_ban_kicks_off_lan_refuses_and_unbans_by_name() {
        let t = Instant::now();
        let mut sv = v6_server(t);
        sv.set_cvar("rconpassword", "pw");
        let _lan = active(&mut sv, t);
        let ip = std::net::Ipv4Addr::new(203, 0, 113, 7);
        let wan = SocketAddr::from((ip, 28960));
        let _nc = active_at(&mut sv, wan, t);
        sv.take_outgoing();
        let at = std::cell::Cell::new(t);
        let ask = |sv: &mut Server, line: &str| {
            at.set(at.get() + Duration::from_millis(500));
            rcon(sv, &format!("pw {line}"), at.get()).concat()
        };
        assert_eq!(ask(&mut sv, "unbanUser vcod"), "print\n");
        assert_eq!(ask(&mut sv, "banClient 0"), "print\n");
        assert_eq!(sv.client_count(), 2);
        let guid = guid_of(crate::engine::Lan::Cod1, wan);
        assert!(!sv.core.bans.is_banned(guid));
        assert_eq!(
            ask(&mut sv, "banClient 1"),
            "print\n1:vcod EXE_PLAYERKICKED\n"
        );
        assert_eq!(sv.client_count(), 1);
        assert!(sv.core.bans.is_banned(guid));

        sv.handle_packet(wan, &oob("getchallenge"), at.get());
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(
            (cmd.as_str(), rest.as_slice()),
            (
                "error",
                &b"\x15You are permanently banned from this server"[..]
            )
        );
        for (line, want) in [
            ("unbanUser", "Usage: unban <client name>\n"),
            ("unbanUser a b", "Usage: unban <client name>\n"),
            ("unbanUser nobody", "no banned user has name nobody\n"),
            ("unbanUser ^1VCOD", "unbanned 1 user(s) named VCOD\n"),
        ] {
            assert_eq!(ask(&mut sv, line), format!("print\n{want}"), "{line}");
        }
        sv.handle_packet(wan, &oob("getchallenge"), at.get());
        assert_eq!(reply(&mut sv).1, "challengeResponse");
    }

    /// `killserver` tells every client `EXE_SERVERKILLED`, answers nothing,
    /// and leaves a process that reads no packet; retail's rcon, `getinfo`
    /// and `status` all went unanswered after it. A console `map` brings
    /// the server back.
    #[test]
    fn killserver_shuts_down_and_keeps_the_process() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        let mut nc = begun(&mut sv, t);
        sv.tick(t);
        let huff = Huffman::new();
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff);
        }
        sv.handle_packet(addr(9), &oob("rcon pw killserver"), t);
        assert!(!sv.quit_requested());
        let out = sv.take_outgoing();
        assert!(
            out.iter().all(|(to, _)| *to == addr(5)),
            "an rcon reply went out"
        );
        assert_eq!(
            server_commands(&mut nc, &out[0].1, &huff),
            ["e \"EXE_SERVERKILLED\"", "w"]
        );
        assert_eq!(sv.client_count(), 0);
        let later = t + Duration::from_secs(1);
        sv.handle_packet(addr(9), &oob("rcon pw status"), later);
        sv.handle_packet(addr(6), &oob("getinfo x"), later);
        sv.tick(later);
        assert!(sv.take_outgoing().is_empty());

        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping the restart");
            return;
        };
        sv.game.fs = Some(Rc::new(fs));
        sv.push_console("map mp_carentan");
        sv.tick(later);
        assert!(sv.take_fatal().is_none());
        sv.take_outgoing();
        sv.handle_packet(addr(6), &oob("getinfo x"), later);
        assert_eq!(reply(&mut sv).1, "infoResponse");
    }

    /// `devmap` is `map` plus `sv_cheats 1`, written after the load so the
    /// gamestate still carries the old value; `map` writes it back to 0.
    /// The replies and the systeminfo values are retail's (handshake doc,
    /// "Console commands over rcon").
    #[test]
    fn devmap_turns_cheats_on_and_map_turns_them_off() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut at = t;
        let mut ask = |sv: &mut Server, line: &str| {
            at += Duration::from_millis(500);
            let r = rcon(sv, &format!("pw {line}"), at).concat();
            sv.tick(at);
            sv.take_outgoing();
            r
        };
        assert_eq!(
            ask(&mut sv, "devmap"),
            "print\nCan't find map maps/mp/.bsp\n"
        );
        assert_eq!(
            ask(&mut sv, "devmap nosuchmap"),
            "print\nCan't find map maps/mp/nosuchmap.bsp\n"
        );
        assert_eq!(
            ask(&mut sv, "set timescale 2"),
            "print\ntimescale is cheat protected.\n"
        );
        ask(&mut sv, "devmap mp/mp_carentan");
        assert_eq!(sv.game.cfg.map, "mp_carentan");
        assert_eq!(
            ask(&mut sv, "sv_cheats"),
            "print\n\"sv_cheats\" is:\"1^7\" default:\"0^7\"\n"
        );
        assert!(sv.configstring(1).contains("\\sv_cheats\\1\\"));
        assert_eq!(ask(&mut sv, "set timescale 2"), "print\n");
        assert_eq!(
            ask(&mut sv, "set sv_cheats 0"),
            "print\nsv_cheats is read only.\n"
        );
        ask(&mut sv, "map mp_carentan");
        assert!(sv.configstring(1).contains("\\sv_cheats\\0\\"));
        assert_eq!(
            ask(&mut sv, "set timescale 1"),
            "print\ntimescale is cheat protected.\n"
        );
    }

    /// A restart over rcon, by `map_restart` or by `map` on the map already
    /// serving, answers with the game module's banner block; retail's reply
    /// was these bytes, client or not and whatever the argument (handshake
    /// doc, "Console commands over rcon").
    #[test]
    fn a_restart_over_rcon_prints_the_game_banner() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let banner = "print\n==== RestartGame ====\n------- Game Initialization -------\n\
                      gamename: main\ngamedate: Nov 13 2003\n0 teams with 0 entities\n\
                      -----------------------------------\n";
        let mut at = t;
        for line in ["map_restart", "map_restart 1", "map mp_carentan"] {
            at += Duration::from_millis(500);
            assert_eq!(rcon(&mut sv, &format!("pw {line}"), at), [banner], "{line}");
            sv.tick(at);
            sv.take_outgoing();
        }
    }

    /// `map` of another map answers with the outgoing game's shutdown, the
    /// spawn's two lines, the search path with each pak's file count and
    /// their total, then the incoming game's banner, as retail 1.1d's did
    /// over rcon (handshake doc, "Console commands over rcon").
    #[test]
    fn a_map_change_over_rcon_prints_the_spawn_and_the_search_path() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "pw");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let reply = rcon(&mut sv, "pw map mp_harbor", t + Duration::from_millis(500)).concat();
        let head = "print\n==== ShutdownGame ====\n------ Server Initialization ------\n\
                    Server: mp_harbor\n----- FS_Startup -----\nCurrent search path:\n";
        let rest = reply.strip_prefix(head).expect(&reply);
        let (path, tail) = rest.split_once("\n\nFile Handles:\n").expect(&reply);
        let counts: Vec<usize> = path
            .lines()
            .filter_map(|l| {
                l.strip_suffix(" files)")?
                    .rsplit_once(".pk3 (")?
                    .1
                    .parse()
                    .ok()
            })
            .collect();
        assert!(counts.len() >= 2, "{path}");
        let total: usize = counts.iter().sum();
        let game = "------- Game Initialization -------\ngamename: main\n\
                    gamedate: Nov 13 2003\n0 teams with 0 entities\n\
                    -----------------------------------\n";
        assert_eq!(
            tail,
            format!("----------------------\n{total} files in pk3 files\n{game}")
        );
    }
}
