//! `spawn_server`, `map_restart`, the settle frames and the baselines.

use super::*;

/// What `G_ShutdownGame(1)` and `G_InitGame` print on a restart ahead of
/// the spawn (handshake doc, "Console commands over rcon"); the rest is
/// [`game_init_tail`].
fn restart_game_banner(p: &Cod1Wire) -> String {
    format!("==== RestartGame ====\n{}", game_init_head(p))
}

/// `G_InitGame`'s lines ahead of the spawn, the game module's name and
/// date being 1.1's, 1.5b's or UO's (their startup logs).
fn game_init_head(p: &Cod1Wire) -> String {
    let (name, date) = match p.transport.version {
        6 => ("Call of Duty", "Nov 15 2004"),
        22 => ("CoD:United Offensive", "Feb 10 2005"),
        _ => ("main", "Nov 13 2003"),
    };
    format!(
        "------- Game Initialization -------\n\
         gamename: {name}\n\
         gamedate: {date}\n"
    )
}

/// `G_FindTeams`' line after the spawn (0x4faff), then `G_InitGame`'s
/// closing dashes ahead of the scripts (0x4fe40).
fn game_init_tail((teams, members): (u32, u32)) -> String {
    format!(
        "{teams} teams with {members} entities\n\
         -----------------------------------\n"
    )
}

/// What a `map` load prints after the outgoing game's `==== ShutdownGame
/// ====`: `SV_SpawnServer`'s two lines and `FS_Startup`'s search path, each
/// directory after its paks, highest first, the `localized_*` paks last
/// (handshake doc, "Console commands over rcon"). vcod has no hunk and
/// loads no module, so retail's `Hunk_Clear` and `Sys_LoadDll` lines have
/// no counterpart.
fn spawn_server_banner(map: &str, fs: &vcod_common::pk3::Pk3Fs) -> String {
    let paks: Vec<_> = fs.paks().collect();
    let mut total = 0;
    let mut line = |pak: &vcod_common::pk3::PakInfo| {
        let n = vcod_common::pak_checksum::entry_count(&pak.path).unwrap_or(0);
        total += n;
        format!("{} ({n} files)\n", pak.path.display())
    };
    let mut out = format!(
        "------ Server Initialization ------\nServer: {map}\n\
         ----- FS_Startup -----\nCurrent search path:\n"
    );
    let in_dir = |pak: &&&vcod_common::pk3::PakInfo, dir: &std::path::Path, loc: bool| {
        pak.localized == loc && pak.path.parent() == Some(dir)
    };
    for dir in fs.dirs().iter().rev() {
        for pak in paks.iter().filter(|p| in_dir(p, dir, false)) {
            out.push_str(&line(pak));
        }
        out.push_str(&format!("{}\n", dir.display()));
    }
    for dir in fs.dirs().iter().rev() {
        for pak in paks.iter().filter(|p| in_dir(p, dir, true)) {
            out.push_str(&line(pak));
        }
    }
    out.push_str(&format!(
        "\nFile Handles:\n----------------------\n{total} files in pk3 files\n"
    ));
    out
}

impl Server {
    /// `G_FindTeams`' counts for the level loaded last, `(0, 0)` with none.
    pub(super) fn team_counts(&self) -> (u32, u32) {
        self.game
            .module
            .script
            .as_ref()
            .map_or((0, 0), |rt| rt.host.team_counts)
    }

    /// `SV_SpawnServer` (docs/research/cod11-map-cycle.md, section 3), in the
    /// order that list numbers. No gamestate is pushed: every client is
    /// demoted to `CS_CONNECTED` and pulls one off the high-nibble branch of
    /// `handle_client_packet` on its next message (section 3.1).
    ///
    /// The bsp is parsed before anything is torn down, so a `map` naming a
    /// map the paks do not have leaves the level that is serving alone
    /// ([`LoadFailure::KeptLevel`]); the script load past the teardown has no
    /// such way back and is [`LoadFailure::Fatal`].
    /// Retail's 250 ms sleep of step 4 has no counterpart here: this runs
    /// inside a tick that owns the whole server, and sleeping would only
    /// delay the same work.
    pub fn spawn_server(&mut self, map: &str) -> Result<(), LoadFailure> {
        let kept = |e: anyhow::Error| LoadFailure::KeptLevel(e);
        let fs = self
            .game
            .fs
            .clone()
            .ok_or_else(|| kept(anyhow::anyhow!("no paks are mounted")))?;
        // Step 15's `CM_LoadMap`, pulled ahead of the teardown, and with it
        // step 9's `FS_ClearPakReferences`.
        fs.clear_references();
        let bsp = vcod_common::mapworld::find(&*fs, map).map_err(kept)?;

        // Step 2: the flag is read off the outgoing level and handed to the
        // incoming one's `G_InitGame`, along with the two tables it decides
        // the fate of.
        let (save_persist, carry) = self.lift_persistence();
        // Step 3: every client whose state is `CS_PRIMED` or above, which is
        // every one that has a gamestate to be told is stale.
        let text = format!("loadingnewmap\n{map}\n{}", self.game.cfg.gametype);
        let told: Vec<SocketAddr> = self
            .core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state != ClientState::Connected)
            .map(|c| c.link.addr)
            .collect();
        for addr in told {
            self.send_oob(addr, &text);
        }
        // Step 5: the game module is unloaded, its object table with it, and
        // step 8's `memset(&sv, 0, ...)` takes everything the level queued.
        self.shutdown_game_log();
        if self.game.module.script.is_some() {
            self.print("==== ShutdownGame ====\n");
        }
        self.game.module.script = None;
        self.game.module.pending_explosions.clear();
        self.game.module.weapon_changes.clear();
        // Step 9.
        self.core.checksum_feed = (self.rand() << 16) ^ self.rand() ^ self.core.sv_time_ms;
        // Step 13.
        self.game.snap_flag_server_bit ^= console::SNAPFLAG_SERVERCOUNT;
        // Step 15's `Cvar_Set("mapname", ...)` and the collision with it.
        self.game.cfg.map = map.to_string();
        let mut world = World::from_bsp(&bsp, Some(&*fs));
        world
            .collision
            .set_source(fs.map_checksum(map).unwrap_or(0));
        self.load_world(world);
        // Step 16, then steps 7 and 10 with step 24 folded in: the table is
        // rebuilt empty around the serverinfo and systeminfo the new id
        // belongs in, which is where the client reads the id back from.
        self.game.server_id = console::next_map_id(self.game.server_id);
        self.core.configstrings = configstrings::static_configstrings(
            &self.game.cfg,
            self.game.server_id,
            self.game.module.fall_heights,
            self.game.cheats,
            &self.game.paks,
            self.systeminfo_v6().as_deref(),
            self.game.proto,
        );
        self.core.configstrings[CS_SERVERINFO] = self.serverinfo().to_string();
        self.print(&spawn_server_banner(map, &fs));
        self.print(&game_init_head(self.game.proto));
        // Step 19. Past the teardown: a failure here leaves no level.
        self.load_scripts_with(fs, false, carry, save_persist)
            .map_err(LoadFailure::Fatal)?;
        self.print(&game_init_tail(self.team_counts()));
        // Step 20. Every client here is `CS_CONNECTED` and pulls the whole
        // table in the gamestate it asks for, so the diff has nothing to say
        // about a level boundary (the restart path re-syncs the same way).
        self.settle_frames();
        // Step 21.
        self.rebuild_baselines();
        // Step 22, after the settle frames and the baselines: `ClientConnect`
        // again for everyone still on a netchan, then back to `CS_CONNECTED`.
        // A denial drops the client (0x808a76f).
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            if self.password_denied(&c.link.userinfo, c.is_bot) {
                self.drop_client(slot, "GAME_INVALIDPASSWORD");
                continue;
            }
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            let name = c.name.clone();
            c.reset_for_level();
            if let Some(rt) = self.game.module.script.as_mut() {
                rt.reconnect_client(slot, name);
            }
        }
        // Step 24 proper: configstring 0 again from the table the scripts
        // and settle frames left, so a `setCvar` of a serverinfo cvar in the
        // level's init is in the gamestate.
        self.flush_serverinfo();
        self.game.last_spawn_tick = Some(self.svs_time_ms());
        log::info!(
            "map {map} ({}), serverId {:#04x}, {} clients kept",
            self.game.cfg.gametype,
            self.game.server_id,
            self.client_count()
        );
        // The masters hear of the new map on the next frame (0x808a915).
        self.core.masters.force();
        Ok(())
    }

    /// What the outgoing level asked to keep and the two tables it names
    /// (map-cycle doc, section 1). Also stashes its cvar table, which is
    /// process-global on retail and outlives both boundaries whatever
    /// `savePersist` says. `false` and an empty carry when no level is
    /// running.
    pub(super) fn lift_persistence(&mut self) -> (bool, crate::game::script::Carry) {
        let Some(rt) = self.game.module.script.as_ref() else {
            return (false, crate::game::script::Carry::default());
        };
        self.game.carried_cvars = Some(rt.cvars().clone());
        (rt.host.save_persist, rt.take_carry())
    }

    /// `SV_MapRestart_f` (map-cycle doc, section 4), in the order that
    /// numbered list. The level is re-inited in place: no gamestate, the
    /// netchan and both reliable rings kept, and only a client that was
    /// already `CS_ACTIVE` re-entered (step 11); a `CS_PRIMED` one is
    /// promoted later by its own next message (4.4).
    pub fn map_restart(&mut self) -> Result<(), LoadFailure> {
        // Step 1: a second restart in one frame, or one straight after a
        // spawn, is a no-op.
        if self.game.last_spawn_tick == Some(self.svs_time_ms()) {
            return Ok(());
        }
        // Step 2.
        if self.game.module.script.is_none() {
            log::info!("Server is not running.");
            return Ok(());
        }
        let Some(fs) = self.game.fs.clone() else {
            return Err(LoadFailure::KeptLevel(anyhow::anyhow!(
                "no paks are mounted"
            )));
        };
        // Step 3: the escalation. A level that asked to keep its
        // persistence gets the in-place restart even across one of these.
        let save_persist = self
            .game
            .module
            .script
            .as_ref()
            .is_some_and(|rt| rt.host.save_persist);
        if !save_persist {
            let changed = self
                .game
                .level_cvars
                .as_ref()
                .and_then(|(gametype, max_clients)| {
                    if *gametype != self.pending_cvar("g_gametype", &self.game.cfg.gametype) {
                        Some("g_gametype")
                    } else if *max_clients
                        != self
                            .pending_cvar("sv_maxclients", &self.game.cfg.max_clients.to_string())
                    {
                        Some("sv_maxclients")
                    } else {
                        None
                    }
                });
            if let Some(name) = changed {
                log::info!("{name} variable change -- restarting.");
                let map = self.game.cfg.map.clone();
                return self.spawn_server(&map);
            }
        }
        let (_, carry) = self.lift_persistence();
        // What every client holds of the table, which the incoming level's
        // writes are measured against below.
        let held = self.core.sent_configstrings.clone();
        // Step 4's six counters: what this level queued and nothing else,
        // since a restart keeps the map and its collision.
        self.game.module.pending_explosions.clear();
        self.game.module.weapon_changes.clear();
        self.game.snap_flag_server_bit ^= console::SNAPFLAG_SERVERCOUNT;
        // Step 5: only the low nibble moves.
        self.game.server_id = console::next_restart_id(self.game.server_id);
        // `sv.core.configstrings` survives a restart: only a spawn clears it, so
        // every slot the outgoing level allocated keeps its index and its
        // text, and the incoming level's `G_ModelIndex` finds the same slot
        // (map-cycle doc, 4.6). The static slots are rebuilt around the new
        // id on top; 4.3 is what carries slot 1 to a client that already
        // has a gamestate.
        for (i, s) in configstrings::static_configstrings(
            &self.game.cfg,
            self.game.server_id,
            self.game.module.fall_heights,
            self.game.cheats,
            &self.game.paks,
            self.systeminfo_v6().as_deref(),
            self.game.proto,
        )
        .into_iter()
        .enumerate()
        {
            if !s.is_empty() {
                self.core.configstrings[i] = s;
            }
        }
        self.core.configstrings[CS_SERVERINFO] = self.serverinfo().to_string();
        // Step 7: `SV_RestartGameProgs(savePersist)`. Past the teardown: the
        // outgoing level's script is gone and a failure here leaves none.
        self.print(&restart_game_banner(self.game.proto));
        let p = crate::tick_profile::phase("console/map_restart/load");
        self.load_scripts_with(fs, true, carry, save_persist)
            .map_err(LoadFailure::Fatal)?;
        drop(p);
        // The bots' read of the old level's vehicles goes with it.
        self.game.module.bot_parked = Default::default();
        self.game.module.bot_drives.restart();
        self.print(&game_init_tail(self.team_counts()));
        // Step 8. The incoming level's whole table goes into the diff's copy
        // in one go: what a client learns of it is the burst below, measured
        // against `held` (doc 4.5).
        let p = crate::tick_profile::phase("console/map_restart/settle");
        self.settle_frames();
        drop(p);
        let reenter = crate::tick_profile::phase("console/map_restart/reenter");
        // The incoming level's writes, ahead of the `n`: `sv.restarting`
        // lets `SV_SetConfigstring` broadcast every write that changes its
        // slot, and retail's settle frames flush them before step 9 does
        // (`tests/fixtures/netchan/mp_carentan-dm-mapchange.txt`, seq 40-42;
        // a team score the outgoing level's `ExitLevel` zeroed, coduo-diff.md
        // 22). The game's writes go in their order (`ConfigStrings::set`,
        // from the load through the settle frames), then what the engine
        // itself left different: the cvar mirror, in slot order. The
        // queue's squash keeps each slot's last `d` at its last position, so
        // the fog `G_setfog("0")` cleared and the map set again still goes
        // out. Slots 0 and 1 are the frame's cvar flush (4.3).
        let game_writes = self
            .game
            .module
            .script
            .as_mut()
            .map(|rt| rt.host.configstrings.take_writes())
            .unwrap_or_default();
        let mut table = held;
        let mut writes: Vec<(usize, String)> = Vec::new();
        for (i, v) in game_writes {
            if i >= 2 && table.get(i) != Some(&v) {
                table[i].clone_from(&v);
                writes.push((i, v));
            }
        }
        for i in 2..self.core.configstrings.len() {
            if table.get(i) != self.core.configstrings.get(i) {
                writes.push((i, self.core.configstrings[i].clone()));
            }
        }
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot].is_some() {
                for (i, v) in &writes {
                    for cmd in vcod_common::net::configstring_commands(*i, v) {
                        self.send_server_command(slot, &cmd);
                    }
                }
            }
        }
        // Steps 9 to 11: `n`, `ClientConnect` again, and back into the world
        // for a client that was already in it.
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            let was_active = c.state == ClientState::Active;
            let name = c.name.clone();
            let entering = c.last_cmd;
            c.reset_for_restart();
            // Through the guarded path, like every other reliable: a client
            // whose acks have fallen a ring behind is dropped rather than
            // handed an overwritten slot.
            // 1.5's and UO's `SV_MapRestart_f` send `n` for a restart that
            // keeps `pers[]` and `B` for one that does not (coduo_lnxded
            // 0x8088325, the letters at 0x80e35b1/0x80e35b3; CoDMP15.exe's
            // at 0x59df40/0x59df44); 1.1 sends `n` either way.
            let restart = if save_persist {
                "n"
            } else {
                self.game.proto.map_restart_command
            };
            self.send_server_command(slot, restart);
            // That guard can drop the slot; a client that is gone has no
            // `ClientConnect` to run and no world to enter.
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            // A denial drops the client (0x8083f99).
            if self.password_denied(&c.link.userinfo, c.is_bot) {
                self.drop_client(slot, "GAME_INVALIDPASSWORD");
                self.print(&format!(
                    "SV_MapRestart_f: dropped client {slot} - denied!\n"
                ));
                continue;
            }
            if let Some(rt) = self.game.module.script.as_mut() {
                rt.reconnect_client(slot, name);
            }
            if was_active {
                // The sim `enter_world` builds carries a clear
                // `EF_TELEPORT_BIT`, which is what retail's re-run
                // `ClientConnect` leaves: the incoming level's first spawn is
                // the flip that puts the client at 24 (map-cycle doc, 8.2).
                self.enter_world(slot, Some(&entering));
            }
        }
        // 4.3: `SV_Frame`'s cvar flush carrying the bumped `sv_serverid`,
        // which is what puts the client on the new id.
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot].is_some() {
                self.send_configstring_update(slot, 1);
            }
        }
        drop(reenter);
        self.game.last_spawn_tick = Some(self.svs_time_ms());
        log::info!(
            "map_restart {} ({}), serverId {:#04x}, {} clients kept",
            self.game.cfg.map,
            self.game.cfg.gametype,
            self.game.server_id,
            self.client_count()
        );
        Ok(())
    }

    /// The three settle frames a load or a restart runs, 100 ms of
    /// `svs.time` each (map-cycle doc, section 3 step 20, section 4 step 8),
    /// then what they allocated read back into the configstring table and
    /// the diff's copy of it.
    pub(super) fn settle_frames(&mut self) {
        for _ in 0..3 {
            self.core.sv_time_ms = self.core.sv_time_ms.wrapping_add(100);
            if let Some(rt) = self.game.module.script.as_mut() {
                rt.run_frame(self.core.sv_time_ms);
                // The vehicles' first think (`Scr_Vehicle_Init`, spawn + 50)
                // settles them before the baselines.
                vehicle_frame(&mut self.core.clients, rt);
            }
        }
        if let Some(rt) = self.game.module.script.as_ref() {
            self.core.configstrings = rt.configstrings().to_vec();
            if let Err(e) = rt.cvars().write_mirror(&mut self.core.configstrings) {
                log::warn!("rebuilding the cvar mirror: {e:?}");
            }
        }
        self.refresh_fall_heights();
        self.sync_sent_configstrings();
    }

    /// `SV_CreateBaseline` (map-cycle doc, section 3 step 21): every entity
    /// linked after the load, the `SVF_NOCLIENT` ones included (3.3).
    pub(super) fn rebuild_baselines(&mut self) {
        let proto = self.game.proto;
        let mut baselines: HashMap<u32, msg::EntityState> = self
            .game
            .module
            .script
            .as_mut()
            .map_or_else(HashMap::new, |rt| {
                rt.baseline_entities(proto).into_iter().collect()
            });
        if let Some(te) = self.game.module.test_entities.as_ref() {
            baselines.extend(te.baselines(proto));
        }
        for es in baselines.values_mut() {
            crate::game::wire::entity_to_protocol(proto, es);
        }
        self.game.baselines = baselines;
    }

    /// The load failure that left the server with no level, taken. The binary
    /// polls it after every tick and ends the process on it: nothing can call
    /// `exitLevel` any more and every client would pull an empty gamestate.
    /// vcod's choice, unverified against retail: 1.1's `Com_Error(ERR_DROP)`
    /// drops the level and leaves the process idle (cod11-movers.md 18, "The
    /// drop"), and which error a failed script load raises is unread.
    pub fn take_fatal(&mut self) -> Option<anyhow::Error> {
        self.game.fatal.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    #[test]
    fn a_restart_reuses_the_compiled_scripts_and_a_map_load_does_not() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let compiled = |sv: &Server| {
            sv.game
                .module
                .script
                .as_ref()
                .and_then(|rt| rt.compiled().cloned())
                .expect("a loaded level keeps its compile")
        };
        let first = compiled(&sv);
        sv.tick(now);
        sv.map_restart().expect("the restart failed");
        assert!(
            Rc::ptr_eq(&first, &compiled(&sv)),
            "the restart compiled again"
        );
        sv.tick(now);
        sv.spawn_server("mp_carentan").expect("the map load failed");
        let spawned = compiled(&sv);
        assert!(
            !Rc::ptr_eq(&first, &spawned),
            "the map load kept the old compile"
        );
        sv.tick(now);
        sv.overlay_script(
            "maps/mp/gametypes/dm",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); }",
        );
        sv.map_restart().expect("the restart failed");
        assert!(
            !Rc::ptr_eq(&spawned, &compiled(&sv)),
            "the restart ran the old text after a new overlay"
        );
    }

    /// The first map's baselines against the retail gamestate fixture
    /// (`mp_carentan` dm): every entity a snapshot can carry is baselined
    /// after the settle frames, as it stood then, so the first snapshot
    /// writes each as a delta against itself. The turrets' barrels are two
    /// slew steps down from 0. The trigger brush and the script origins are
    /// linked `SVF_NOCLIENT` and baselined all the same (map-cycle doc 3.3).
    #[test]
    fn the_first_maps_baselines_match_retail() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let p = &PROTOCOL_V1;
        let huff = Huffman::new();
        let gs_bytes = vcod_common::testing::fixture("net/gamestate.bin");
        let mut r = MsgReader::new(&gs_bytes[4..], &huff);
        let retail = vcod_common::net::gamestate::parse(&mut r, p)
            .unwrap()
            .baselines;

        let fs = Rc::new(fs);
        let mut sv = Server::new(cfg(), Instant::now());
        let path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp =
            vcod_common::bsp::parse(&fs.read(&path).expect("read the bsp")).expect("parse the bsp");
        sv.load_world(World::from_bsp(&bsp, Some(&*fs)));
        sv.load_scripts(fs).expect("load the scripts");

        let mut ours: Vec<u32> = sv.game.baselines.keys().copied().collect();
        ours.sort_unstable();
        let mut want: Vec<u32> = retail.keys().copied().collect();
        want.sort_unstable();
        assert_eq!(ours, want);
        for n in ours {
            let (a, b) = (&sv.game.baselines[&n], &retail[&n]);
            for (f, (x, y)) in p.entity_fields.iter().zip(a.fields.iter().zip(&b.fields)) {
                assert_eq!(x, y, "entity {n} {}", f.name);
            }
        }
    }

    /// mp_cassino's `script_origin` "auto9" (entity 414) lists `origin`
    /// before `angles` "0 2.50448e-006 0": applied in lump order the yaw
    /// stays unsnapped and goes out as a sent 0, as the retail UO
    /// gamestate's baseline carries it; `angles` first, the `origin` link
    /// snapped it and the baseline left it out (object-model doc 7).
    #[test]
    fn uo_cassinos_auto9_baseline_matches_retail() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            eprintln!("COD_DIR is not a UO install: skipping");
            return;
        };
        let p = &PROTOCOL_V22;
        let huff = Huffman::new();
        let gs_bytes = vcod_common::testing::fixture("net/gamestate-uo.bin");
        let mut r = MsgReader::new(&gs_bytes[4..], &huff);
        let retail = vcod_common::net::gamestate::parse(&mut r, p)
            .unwrap()
            .baselines;
        let fs = Rc::new(fs);
        let mut sv = Server::new(
            ServerConfig {
                map: "mp_cassino".into(),
                max_clients: 8,
                ..cfg()
            },
            Instant::now(),
        );
        sv.set_protocol(p);
        let path = fs.resolve_map("mp_cassino").expect("map in the paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&path).unwrap()).unwrap();
        sv.load_world(World::from_bsp(&bsp, Some(&*fs)));
        sv.load_scripts(fs).expect("load the scripts");
        let null = msg::EntityState::null(p);
        let wire = |e: &msg::EntityState| {
            let mut w = MsgWriter::new(&huff);
            msg::write_delta_entity(&mut w, p, &null, Some(e));
            w.finish()
        };
        assert_eq!(wire(&sv.game.baselines[&414]), wire(&retail[&414]));
    }

    /// Map-cycle doc 4.8: `G_InitGame` reseeds the script's generator at
    /// every load, a restart too, so a restarted level does not replay the
    /// last one's draws (UO hq's radio pick, coduo-diff.md 22).
    #[test]
    fn a_restart_reseeds_the_script_draws() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR not at 1.1 install: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_cvar("g_gametype", "dm");
        let path = fs.resolve_map("mp_carentan").expect("map in paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&path).expect("read bsp")).expect("parse bsp");
        sv.load_world(World::from_bsp(&bsp, Some(&*fs)));
        sv.load_scripts(fs).expect("load scripts");
        let draws = |sv: &mut Server| -> Vec<i32> {
            let host = &mut sv.game.module.script.as_mut().unwrap().host;
            (0..4).map(|_| host.rand_int()).collect()
        };
        let first = draws(&mut sv);
        let id = sv.game.server_id;
        sv.push_console("map_restart");
        sv.tick(now);
        assert_ne!(sv.game.server_id, id, "the restart did not run");
        assert_ne!(draws(&mut sv), first, "the restart replayed the draws");
    }

    /// Doc section 4 step 3: a `g_gametype` that moved since the level
    /// loaded turns a restart into a full spawn of the same map, so the
    /// serverId's high nibble climbs where a restart would have moved the
    /// low one. A level that asked to keep its persistence is exempt.
    #[test]
    fn a_gametype_change_escalates_a_restart_to_a_spawn() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let fs = Rc::new(fs);
        let mut now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(fs).expect("load the scripts");
        let before = sv.game.server_id;

        // No change yet: the plain restart path.
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(sv.game.server_id, console::next_restart_id(before));

        // The rotation's `gametype` token, then a restart: the level is
        // still running dm, so this escalates.
        let id = sv.game.server_id;
        sv.set_cvar("g_gametype", "tdm");
        now += Duration::from_millis(50);
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.game.server_id,
            console::next_map_id(id),
            "a gametype change did not escalate to a spawn"
        );
        assert_eq!(
            sv.game.cfg.map, "mp_carentan",
            "the escalation moved off the map"
        );
        assert_eq!(sv.script_cvar("g_gametype").as_deref(), Some("tdm"));
    }

    /// A load that fails after the teardown leaves no level to serve and no
    /// console line that could bring one back, which is what retail ends the
    /// process for: the server records it and `main` exits on it. A load that
    /// fails before the teardown (a map the paks do not have) keeps the
    /// level that is serving and records nothing.
    #[test]
    fn a_script_load_that_fails_after_the_teardown_is_fatal() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let mut now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");

        // `resolve_map` fails ahead of everything the spawn tears down.
        sv.push_console("map mp_nosuchmap");
        sv.tick(now);
        assert!(
            sv.take_fatal().is_none(),
            "a map the paks do not have took the server down"
        );
        assert_eq!(
            sv.game.cfg.map, "mp_carentan",
            "the failed map load moved off"
        );

        // The rotation's `gametype` token names a gametype with no script,
        // and the `map` token behind it escalates to a spawn whose
        // `load_scripts_with` fails with the level already gone.
        now += Duration::from_millis(50);
        sv.set_cvar("sv_mapRotation", "gametype nosuch map mp_carentan");
        sv.push_console("map_rotate");
        sv.tick(now);
        let e = sv
            .take_fatal()
            .expect("a gametype with no script was survivable");
        assert!(format!("{e:#}").contains("nosuch"), "{e:#}");
        assert!(sv.take_fatal().is_none(), "the failure was reported twice");
    }

    /// A `g_password` set while a client is on is checked again by the
    /// `ClientConnect` both level boundaries re-run; retail drops the client
    /// with `w "GAME_INVALIDPASSWORD"` (handshake doc, "g_password").
    #[test]
    fn a_new_g_password_drops_clients_at_the_next_restart_or_map() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let fs = Rc::new(fs);
        for spawn in [false, true] {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.load_scripts(fs.clone()).expect("load the scripts");
            active(&mut sv, now);
            sv.console_set("g_password", "secret");
            sv.take_outgoing();
            sv.handle_packet(addr(7), &oob("getinfo x"), now);
            let (_, _, rest) = reply(&mut sv);
            assert_eq!(
                info_value_for_key(&String::from_utf8_lossy(&rest), "pswrd"),
                Some("1"),
                "g_password is live, not latched"
            );
            if spawn {
                sv.spawn_server("mp_carentan").expect("the map load failed");
            } else {
                sv.map_restart().expect("the restart failed");
            }
            assert!(
                sv.core.clients[0].is_none(),
                "spawn {spawn}: the client stayed"
            );
            let z = sv.game.zombies[0].as_ref().expect("a zombie");
            let at = z.link.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1);
            assert_eq!(z.link.netchan.reliable[at], "w \"GAME_INVALIDPASSWORD\"");
        }
    }

    /// Doc section 4 step 11: `SV_ClientEnterWorld` runs only for a client
    /// that reads `CS_ACTIVE`. A `CS_PRIMED` one keeps its state through
    /// the restart and is promoted by its own next message instead (4.4),
    /// which is the branch the second half exercises.
    #[test]
    fn a_primed_client_is_not_re_entered_by_a_restart() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut nc = active(&mut sv, now);
        assert_eq!(
            sv.core.clients[0].as_ref().unwrap().state,
            ClientState::Primed
        );

        sv.map_restart().expect("the restart failed");
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!(
            c.state,
            ClientState::Primed,
            "the restart entered a primed client"
        );
        assert!(
            c.sim.is_none(),
            "a primed client came out of the restart with a sim"
        );

        // Its next message still carries the old serverId: same high nibble,
        // differing low one, which is the promotion branch.
        let huff = Huffman::new();
        let ack = nc.incoming_sequence as i32;
        let pkt = nc.build_out(0x10, ack, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!(c.state, ClientState::Active);
        assert!(c.sim.is_some());
    }

    /// The burst ahead of the restart's `n` carries every slot the incoming
    /// level wrote to a new value: the outgoing level's team score, which
    /// `ExitLevel` zeroes in the game only, goes out as `d 6 0` when the next
    /// level sets it (coduo-diff.md 22.2, "Round and map ends").
    #[test]
    fn a_restart_sends_the_slots_the_incoming_level_changed() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.overlay_script(
            "maps/mp/gametypes/dm",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             if (getcvar(\"probe_restarted\") == \"\") { setcvar(\"probe_restarted\", \"1\"); \
             setTeamScore(\"allies\", 1); } else setTeamScore(\"allies\", 0); }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut nc = begun(&mut sv, now);
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..4 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                server_commands(&mut nc, &pkt, &huff);
            }
        }
        assert_eq!(sv.configstring(6), "1");

        sv.map_restart().expect("the restart failed");
        for _ in 0..4 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                seen.extend(server_commands(&mut nc, &pkt, &huff));
            }
        }
        let d = seen.iter().position(|c| c == "d 6 0");
        let n = seen.iter().position(|c| c == "n");
        assert!(
            matches!((d, n), (Some(d), Some(n)) if d < n),
            "no `d 6 0` ahead of the `n` in {seen:?}"
        );
    }

    /// The burst goes in the incoming level's write order, not slot order:
    /// `SV_SetConfigstring` queues each `d` as the game writes it (map-cycle
    /// doc 4.5). The allies' 6 is written ahead of the axis' 5 here.
    #[test]
    fn a_restart_s_burst_keeps_the_write_order() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.overlay_script(
            "maps/mp/gametypes/dm",
            "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
             if (getcvar(\"probe_restarted\") == \"\") setcvar(\"probe_restarted\", \"1\"); \
             else { setTeamScore(\"allies\", 3); setTeamScore(\"axis\", 2); } }",
        );
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let mut nc = begun(&mut sv, now);
        for _ in 0..4 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                server_commands(&mut nc, &pkt, &huff);
            }
        }
        sv.map_restart().expect("the restart failed");
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..4 {
            sv.tick(now);
            for (_, pkt) in sv.take_outgoing() {
                seen.extend(server_commands(&mut nc, &pkt, &huff));
            }
        }
        let at = |s: &str| seen.iter().position(|c| c == s);
        assert!(
            matches!((at("d 6 3"), at("d 5 2")), (Some(a), Some(b)) if a < b),
            "the allies' `d 6` not ahead of the axis' `d 5` in {seen:?}"
        );
    }

    /// The other half of step 3: a `+set` that names one of the two cvars is
    /// not a change. `cvars` replays the override list after stamping the
    /// config's own value, so `--max-clients 4 --set sv_maxclients=8` loads
    /// the level with 8; comparing that against the config's 4 would escalate
    /// every restart for the whole run.
    #[test]
    fn a_set_override_of_sv_maxclients_is_not_a_gametype_change() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        assert_eq!(sv.game.cfg.max_clients, 4);
        sv.set_cvar("sv_maxclients", "8");
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        assert_eq!(sv.script_cvar("sv_maxclients").as_deref(), Some("8"));

        let before = sv.game.server_id;
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.game.server_id,
            console::next_restart_id(before),
            "the override escalated a restart to a spawn"
        );
    }

    /// Doc section 4 step 1: a second restart inside one frame is a no-op,
    /// and so is one straight after a spawn.
    #[test]
    fn a_second_restart_in_one_frame_is_a_no_op() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        let before = sv.game.server_id;
        sv.push_console("map_restart");
        sv.push_console("map_restart");
        sv.tick(now);
        assert_eq!(
            sv.game.server_id,
            console::next_restart_id(before),
            "the second restart in the frame was not a no-op"
        );
    }
}
