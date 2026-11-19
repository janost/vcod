//! Configstring changes, diffed and sent.

use super::*;

impl Server {
    /// `SV_SetConfigstring`'s per-client half, the `d <index> <text>` server
    /// command. Two callers: the restart's slot 1, sent to a client that
    /// keeps its gamestate after the burst of the incoming level's writes
    /// (map-cycle doc, 4.3 and 4.5), and
    /// [`Self::broadcast_configstring_changes`] once a frame. A map change
    /// reaches neither: it re-syncs the table after its settle frames, so the
    /// diff has nothing to say about the boundary, and every client it leaves
    /// behind is `CS_CONNECTED`, which the diff skips. That is what keeps a
    /// map change off the reliable stream entirely, the gamestate each client
    /// pulls carrying the whole table instead (3.1).
    ///
    /// A value of 1000 bytes or more goes as `x`, `y`..., `z` chunks of 999
    /// (cod11-server-handshake.md, "Configstring 1, systeminfo").
    pub(super) fn send_configstring_update(&mut self, slot: usize, index: usize) {
        for cmd in vcod_common::net::configstring_commands(index, self.configstring(index)) {
            self.send_server_command(slot, &cmd);
        }
    }

    /// `SV_SetConfigstring`'s per-frame half: every slot the running level
    /// changed goes out as `d <index> <text>` to every client, which is what
    /// retail's broadcast gate lets through while `sv.state` reads 2
    /// (map-cycle doc, 3.1). The script allocates all through the level (a
    /// `playSound` or `precacheString` from a thread past a `wait`), and the
    /// index a later `s` or event carries is worthless to a client whose slot
    /// is still empty.
    ///
    /// Called after the script frame and before the frame's own client
    /// commands, so the `d` naming an alias precedes the `s` that plays it.
    /// A `CS_CONNECTED` client is skipped: it has no table to patch, and the
    /// gamestate it pulls carries every slot allocated by then. UNVERIFIED:
    /// what retail's own broadcast does with such a client; its gate is on
    /// `sv.state`, not on the client's.
    ///
    /// The game's slots go in the order of their last write
    /// (`ConfigStrings::writes`), as retail queues each `d` at its write;
    /// the rest (the cvar mirror) after them, in slot order.
    pub(super) fn broadcast_configstring_changes(&mut self) {
        let order: Vec<usize> = self
            .game
            .module
            .script
            .as_ref()
            .map(|rt| {
                rt.host
                    .configstrings
                    .writes()
                    .iter()
                    .map(|(i, _)| *i)
                    .collect()
            })
            .unwrap_or_default();
        self.broadcast_changes_in(&order);
    }

    /// [`Self::broadcast_configstring_changes`] with the game's write order
    /// given: each changed slot at its last place in `order`, the rest after.
    fn broadcast_changes_in(&mut self, order: &[usize]) {
        let mut changed: Vec<usize> = (0..self.core.configstrings.len())
            .filter(|&i| self.core.sent_configstrings.get(i) != self.core.configstrings.get(i))
            .collect();
        changed.sort_by_key(|&i| order.iter().rposition(|w| *w == i).unwrap_or(usize::MAX));
        if changed.is_empty() {
            return;
        }
        self.sync_sent_configstrings();
        for slot in 0..self.core.clients.len() {
            let has_table = self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state != ClientState::Connected);
            if !has_table {
                continue;
            }
            for &i in &changed {
                self.send_configstring_update(slot, i);
            }
        }
    }

    /// A frame's game writes and server commands in their write order
    /// (both stamped by `configstrings::next_write_seq`). The writes ahead
    /// of the first command go with the engine's own changes through
    /// [`Self::broadcast_configstring_changes`] as before; each later write
    /// goes out with its own value between the commands it fell between.
    /// `setClientCvar` and `openMenu` queue rather than send, and this is
    /// where the queue reaches the netchan.
    pub(super) fn send_frame_writes(
        &mut self,
        writes: Vec<(u64, usize, String)>,
        commands: Vec<(u64, usize, String)>,
    ) {
        let first = commands.first().map_or(u64::MAX, |c| c.0);
        let split = writes.partition_point(|w| w.0 < first);
        let (early, late) = writes.split_at(split);
        // Each late slot held at what the early writes (or the last frame)
        // left it, for the diff; its final value goes back after.
        let mut finals = Vec::new();
        for &(_, i, _) in late {
            if finals.iter().any(|(j, _)| *j == i) || i >= self.core.configstrings.len() {
                continue;
            }
            let before = early
                .iter()
                .rev()
                .find(|w| w.1 == i)
                .map(|w| w.2.clone())
                .or_else(|| self.core.sent_configstrings.get(i).cloned())
                .unwrap_or_default();
            finals.push((
                i,
                std::mem::replace(&mut self.core.configstrings[i], before),
            ));
        }
        let order: Vec<usize> = early.iter().map(|w| w.1).collect();
        self.broadcast_changes_in(&order);
        for (i, v) in finals {
            self.core.configstrings[i] = v;
        }
        let mut late = late.iter().peekable();
        for (seq, slot, cmd) in commands {
            while let Some((_, i, v)) = late.next_if(|w| w.0 < seq) {
                self.send_game_write(*i, v);
            }
            crate::tick_trace::step!("SV_GameSendServerCommand slot {slot}");
            self.send_server_command(slot, &cmd);
        }
        for (_, i, v) in late {
            self.send_game_write(*i, v);
        }
    }

    /// One game write's `d` to every client with a table, as
    /// `SV_SetConfigstring` sends it at the write.
    fn send_game_write(&mut self, i: usize, v: &str) {
        if i < 2 || i >= self.core.configstrings.len() {
            return;
        }
        if self.core.sent_configstrings.get(i).is_some_and(|s| s == v) {
            return;
        }
        if let Some(s) = self.core.sent_configstrings.get_mut(i) {
            v.clone_into(s);
        }
        let cmds = vcod_common::net::configstring_commands(i, v);
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state != ClientState::Connected)
            {
                for cmd in &cmds {
                    self.send_server_command(slot, cmd);
                }
            }
        }
    }

    /// Marks the whole table as already known to every client. The two level
    /// boundaries hand it over wholesale (a map change in the gamestate
    /// each client pulls, a restart in the burst 4.3 describes), so neither
    /// leaves anything for the diff above to send.
    pub(super) fn sync_sent_configstrings(&mut self) {
        self.core.sent_configstrings = self.core.configstrings.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// The script keeps allocating configstrings after map load, from any
    /// thread that has passed a `wait` (`G_SoundAliasIndex` has no
    /// `level.initializing` gate). The server re-reads the script's table
    /// each tick, so such an allocation reaches it.
    #[test]
    fn a_configstring_allocated_after_a_wait_reaches_the_server() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { e = spawn(\"script_origin\", (0, 0, 0)); wait 0.5; e playsound(\"late_alias\"); }",
            ),
        );
        let slot = vcod_common::net::protocol::CS_SOUNDS_V1 + 1;
        // sv_time starts at 0 and each tick advances it one 50 ms frame, so
        // the thread is still suspended after the first.
        sv.tick(now);
        assert_eq!(sv.configstring(slot), "");
        for _ in 0..12 {
            sv.tick(now);
        }
        assert_eq!(sv.configstring(slot), "late_alias");
    }

    /// `--set g_gametype` reaches the serverinfo configstring, not only the
    /// script's cvar table: retail's cvar flush follows every serverinfo
    /// write with `SV_SetConfigstring(0, Cvar_InfoString(CVAR_SERVERINFO))`
    /// (map-cycle doc 4.3), and `main.rs` replays the `--set` list after
    /// `Server::new` has already stamped the table.
    #[test]
    fn a_set_of_a_serverinfo_cvar_reaches_configstring_0() {
        let mut sv = Server::new(cfg(), Instant::now());
        assert!(sv.configstring(0).contains("g_gametype\\dm"));
        sv.set_cvar("g_gametype", "sd");
        assert!(
            sv.configstring(0).contains("g_gametype\\sd"),
            "serverinfo: {:?}",
            sv.configstring(0)
        );
    }

    /// A script `setCvar` of a serverinfo cvar reaches configstring 0 and
    /// `getinfo` by the next frame, as retail's `SV_Frame` flush makes it
    /// (`sv_privateClients 12` on 8 slots read `sv_maxclients -4` live; 4
    /// slots here).
    #[test]
    fn a_script_write_of_a_serverinfo_cvar_reaches_configstring_0() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { wait 0.1; setCvar(\"sv_privateClients\", \"12\"); setCvar(\"sv_hostname\", \"scripted\"); }",
            ),
        );
        sv.tick(now);
        assert!(sv.configstring(0).contains("\\sv_privateClients\\0"));
        for _ in 0..4 {
            sv.tick(now);
        }
        let cs0 = sv.configstring(0).to_string();
        assert_eq!(info_value_for_key(&cs0, "sv_privateClients"), Some("12"));
        assert_eq!(info_value_for_key(&cs0, "sv_hostname"), Some("scripted"));
        sv.take_outgoing();
        sv.handle_packet(addr(20), &oob("getinfo x"), now);
        let (_, _, rest) = reply(&mut sv);
        let info = String::from_utf8_lossy(&rest).to_string();
        assert_eq!(info_value_for_key(&info, "sv_maxclients"), Some("-8"));
        assert_eq!(info_value_for_key(&info, "hostname"), Some("scripted"));
    }

    /// `SV_SetConfigstring` broadcasts a slot the running level changed to
    /// every client that already has its gamestate (map-cycle doc, 3.1 and
    /// 4.3). The announcer is what needs it: `playLocalSound` allocates its
    /// alias slot and then sends `s <idx>`, so a client that never heard the
    /// `d` reads an empty slot and plays nothing.
    #[test]
    fn a_configstring_allocated_mid_level_reaches_a_connected_client() {
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { wait 0.5; p = getentarray(\"noclass\", \"classname\"); \
             p[0] playlocalsound(\"MP_announcer_allies_win\"); }",
            ),
        );
        let mut nc = begun(&mut sv, now);
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..16 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                seen.extend(server_commands(&mut nc, &pkt, &huff));
            }
        }
        let d = seen
            .iter()
            .position(|c| c == "d 525 MP_announcer_allies_win")
            .unwrap_or_else(|| panic!("no `d 525` in {seen:?}"));
        let s = seen
            .iter()
            .position(|c| c == "s 1")
            .unwrap_or_else(|| panic!("no `s 1` in {seen:?}"));
        assert!(
            d < s,
            "the announcer's `s 1` came before its `d 525`: {seen:?}"
        );
    }

    /// A `CS_CONNECTED` client has no table to patch: its gamestate has not
    /// gone out, and the one it pulls carries every slot the level has
    /// allocated by then. So the per-frame diff skips it and sends to the
    /// clients that do. UNVERIFIED: whether retail gates its own broadcast
    /// per client this way; `SV_SetConfigstring`'s gate is on `sv.state`
    /// (map-cycle doc, 3.1) and nothing measured says what it does with a
    /// client below `CS_PRIMED`.
    #[test]
    fn the_configstring_broadcast_skips_a_client_with_no_gamestate() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let _nc = connected(&mut sv, addr(5), now);
        sv.sync_sent_configstrings();
        assert_eq!(
            sv.core.clients[0].as_ref().unwrap().state,
            ClientState::Connected
        );

        let seq = sv.core.clients[0]
            .as_ref()
            .unwrap()
            .link
            .netchan
            .reliable_sequence;
        sv.core.configstrings[524] = "MP_announcer_allies_win".to_string();
        sv.broadcast_configstring_changes();
        assert_eq!(
            sv.core.clients[0]
                .as_ref()
                .unwrap()
                .link
                .netchan
                .reliable_sequence,
            seq,
            "a client with no gamestate was sent a `d`"
        );

        // The same slot moving again once the gamestate has gone out does
        // reach it, so the skip above is the state and not the diff.
        sv.core.clients[0].as_mut().unwrap().state = ClientState::Primed;
        sv.core.configstrings[524] = "MP_announcer_axis_win".to_string();
        sv.broadcast_configstring_changes();
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!(c.link.netchan.reliable_sequence, seq + 1);
        assert_eq!(
            c.link.netchan.reliable
                [c.link.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1)],
            "d 524 MP_announcer_axis_win"
        );
    }

    /// Doc 3 step 20: the settle frames allocate into the table, and every
    /// client on the far side of a spawn pulls the whole of it in its own
    /// gamestate. So the copy the per-frame diff works against is re-synced
    /// after those frames, the way the restart path re-syncs after its own;
    /// without it the first tick after a map change queues a `d` per slot the
    /// three frames touched.
    #[test]
    fn a_spawn_re_syncs_the_table_the_broadcast_diffs_against() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                gametype: "settle".into(),
                ..cfg()
            },
            now,
        );
        // A gametype whose settle frames allocate: the `wait` is due in the
        // first of the three, and a status icon is one precache retail
        // takes past the first wait.
        sv.overlay_script(
            "maps/mp/gametypes/settle",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             wait 0.05; precacheStatusIcon(\"gfx/hud/settle\"); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        sv.spawn_server("mp_brecourt").expect("the map load failed");
        // What the next tick reads back out of the script, which is what it
        // diffs the sent copy against.
        let rt = sv
            .game
            .module
            .script
            .as_ref()
            .expect("the spawn left no script");
        let mut live = rt.configstrings().to_vec();
        rt.cvars().write_mirror(&mut live).expect("the mirror fits");
        assert!(
            live.iter().any(|s| s == "gfx/hud/settle"),
            "the settle frames allocated nothing, so this proves nothing"
        );
        let stale = |table: &[String]| -> Vec<usize> {
            (0..live.len())
                .filter(|&i| table.get(i) != live.get(i))
                .collect()
        };
        assert!(
            stale(&sv.core.sent_configstrings).is_empty(),
            "the spawn left slots {:?} for the diff to send",
            stale(&sv.core.sent_configstrings)
        );
        assert!(
            stale(&sv.core.configstrings).is_empty(),
            "the server's own copy is a frame behind the script at slots {:?}",
            stale(&sv.core.configstrings)
        );
    }
}
