//! `enter_world`, the tick, the game clock and the game frame.

use super::*;

impl Server {
    pub(super) const FALLBACK_SPAWN: ([f32; 3], f32) = ([0.0, 0.0, 64.0], 0.0);

    /// `SV_ClientEnterWorld` for a spectator: park the sim, start
    /// snapping. `entering` is the usercmd that brought the client in, which
    /// is `None` on the paths that have none, as ioq3's is null there; its
    /// angles are what the spawn's `delta_angles` subtract.
    pub(super) fn enter_world(&mut self, slot: usize, entering: Option<&UserCmd>) {
        let svs_ms = self.svs_time_ms();
        // Under a game module the client stays where `ClientConnect`'s
        // zeroing left it, origin and `delta_angles` 0, until a script
        // spawns it (cod11-gametypes-re-bel.md 8.3). Without one nothing
        // ever would, so it stands at the map's spawn.
        let (spawn, cmd_angles) = if self.game.module.script.is_some() {
            (([0.0; 3], 0.0), [0; 3])
        } else {
            (
                self.game
                    .world
                    .as_ref()
                    .map_or(Self::FALLBACK_SPAWN, |w| w.spawn),
                entering.map_or(NULL_USERCMD.angles, |c| c.angles),
            )
        };
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        c.state = ClientState::Active;
        // The first snapshot goes this frame, not after the primed client's
        // 1 s keepalive interval (0x80877d8).
        c.link.next_message_ms = svs_ms;
        let mut sim = ClientSim::spectator(spawn.0, spawn.1, cmd_angles);
        // A 1.5 server moves every client, bots too, under 1.5's rules, as
        // its game module's one pmove does.
        sim.ps.rules = vcod_common::pmove::MoveRules::for_protocol(self.game.proto);
        c.sim = Some(sim);
        // The entering cmd is not simulated: retail's execute loop skips
        // every cmd at or before `lastUsercmd`, which entry has just set to
        // it. The clock is the client's own, held to the window
        // `replay_moves` holds every cmd to, so a handshake cmd stamped far
        // ahead cannot leave `commandTime` past every cmd that follows. With
        // no entering cmd, one frame back, so the first cmd's dt is a sane
        // 50 ms rather than the whole age of the client's clock.
        c.last_processed_st = entering.map_or(self.core.sv_time_ms.wrapping_sub(FRAME_MS), |c| {
            crate::game::hold_cmd_time(c.server_time, self.core.sv_time_ms)
        });
        c.last_usercmd_st = entering.map_or(c.last_processed_st, |c| c.server_time);
        log::info!("client {slot} {:?} begin (spectator)", c.name);
        // `ClientBegin`: the notify that releases the connect callback's
        // `waittill("begin")`. The event queues rather than fires here, so it
        // is drained after the `Connect` that armed the wait.
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.push_client_event(ClientEvent::Begin(slot));
        }
    }

    /// One `SV_Frame` that runs a single game frame.
    pub fn tick(&mut self, now: Instant) {
        self.tick_frames(now, 1);
    }

    /// `SV_Frame`'s head: `Cbuf_Execute`, then whether the frame runs at
    /// all. Keeps the wall gap for `--trace` and starts the tick's moves.
    pub(super) fn frame_head(&mut self, now: Instant, frames: u32) -> bool {
        crate::tick_trace::step!("Cbuf_Execute");
        self.drain_console(now);
        // `Com_Quit_f` exits inside the command; nothing after it runs. A
        // killed server runs no frame (`SV_Frame` returns while
        // `sv_running` is 0).
        if self.core.quit || self.game.killed {
            return false;
        }
        // Wall gap between ticks: sv_time advances FRAME_MS a frame, so a
        // gap far off it means the frames the client interpolates between
        // are not arriving at the rate their timestamps claim.
        let wall_ms = self
            .game
            .last_tick
            .map(|t| now.saturating_duration_since(t).as_secs_f32() * 1000.0);
        self.game.last_tick = Some(now);
        self.game.tick_wall = (wall_ms, frames);
        self.game.tick_moved = None;
        true
    }

    /// The bots' cmds, once a `SV_Frame`.
    pub(super) fn bot_frame(&mut self) {
        // The bots' walks move by this server's rules, as their bodies do.
        crate::tick_trace::step!("SV_BotFrame");
        crate::nav::with_rules(self.move_rules(), || self.step_bots());
    }

    /// `SV_ArchiveSnapshot` between the frames (0x808d1bb), while script
    /// has the archive on.
    pub(super) fn archive_between(&mut self) {
        if self.game.archive.is_on() {
            crate::tick_trace::step!("SV_ArchiveSnapshot");
            let world = self.build_world_frame();
            self.archive_frame(world);
        }
    }

    /// How far this frame moves the game clock. 1.1's `SV_Frame` steps
    /// it a whole frame whatever `timescale` says; 1.5b's (0x80942a1)
    /// steps `svs.time` a frame and the game clock by `Com_ModifyMsec`
    /// (0x80716b2) of it, which truncates the scaled frame, bumps a
    /// non-zero result under 1 to 1 and lets 0 stand. A negative
    /// `timescale` is set to 0 first (cod15-protocol-diff.md, section 12).
    pub(super) fn game_step_ms(&mut self) -> i32 {
        if !self.v15() {
            return FRAME_MS;
        }
        let Some(rt) = self.game.module.script.as_mut() else {
            return FRAME_MS;
        };
        let mut scale = crate::game::builtins::cvar::atof(rt.cvars().get("timescale"));
        if scale < 0.0 {
            rt.host.cvars.set("timescale", "0");
            scale = 0.0;
        }
        let step = (FRAME_MS as f32 * scale) as i32;
        if step <= 0 && scale != 0.0 {
            1
        } else {
            step.clamp(0, 5000)
        }
    }

    /// The frame's clock: 1.5's game clock steps by `timescale`, `svs.time`
    /// a whole frame. Returns the game clock's step, which the engine adds.
    pub(super) fn frame_clock(&mut self) -> i32 {
        let step = self.game_step_ms();
        self.game.paused_ms = self.game.paused_ms.wrapping_add(FRAME_MS - step);
        crate::tick_trace::step!(
            "G_RunFrame time {}",
            self.core.sv_time_ms.wrapping_add(step)
        );
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.frozen = step == 0;
        }
        step
    }

    /// One game frame past its clock and its moves (`moved`, by slot, which
    /// left the world at this frame before script or anyone's snapshot reads
    /// it): the script frame and the entity pass, and what they queued for
    /// the wire. False when a `G_Error` dropped the level mid-frame.
    pub(super) fn game_frame(&mut self, moved: &[MoveSummary]) -> bool {
        let svs_ms = self.svs_time_ms();
        // `G_RunFrame`, the script threads through every `ClientEndFrame`.
        let (game, eng) = self.split(svs_ms);
        let FrameOut {
            vote_lines,
            timeout_written,
            console_lines,
            client_commands,
            ranks_dirty,
        } = match game.run_frame_cod1(eng, moved) {
            Ok(out) => out,
            // A `G_Error` in the pass: `Com_Error`'s drop runs instead of
            // the rest of the frame (docs/research/cod11-movers.md 18).
            Err(e) => {
                self.drop_level(&e);
                return false;
            }
        };
        crate::tick_trace::step!("SV_GameFrameOut");
        self.send_vote_lines(vote_lines.into_iter().map(|l| (None, l)).collect());
        if timeout_written {
            self.copy_timeout_configstrings();
        }
        let mut _p = crate::tick_profile::phase("frame/configstrings");
        if let Some(rt) = self.game.module.script.as_mut() {
            // The script owns the table while it runs and allocates into it
            // from any thread, so the server re-reads it rather than trusting
            // the copy `load_scripts` took. A whole-table copy per frame is
            // cheap next to a snapshot, and there is no single write choke
            // point on the host's table to hang a dirty flag off. The cvar
            // mirror gets the same treatment: a thread past a `wait` can
            // still call `setCvar`.
            self.core.configstrings = rt.configstrings().to_vec();
            if let Err(e) = rt.cvars().write_mirror(&mut self.core.configstrings) {
                log::warn!("rebuilding the cvar mirror: {e:?}");
            }
        }
        self.refresh_fall_heights();
        self.flush_serverinfo();
        // `trap_SendConsoleCommand`'s `EXEC_APPEND`: what a builtin queued
        // this frame runs at the top of the next tick, never mid-frame.
        self.core.console.extend(console_lines);
        // Retail sends each `d` at its `SV_SetConfigstring` and each command
        // at its `SV_GameSendServerCommand`, so a slot the script allocated
        // is named before anything points at it, and one allocated after a
        // command goes out after it (gametypes doc 7.6).
        crate::tick_trace::step!("SV_SetConfigstring broadcast");
        _p = crate::tick_profile::phase("frame/commands");
        let writes = self
            .game
            .module
            .script
            .as_mut()
            .map(|rt| rt.host.configstrings.take_writes_stamped())
            .unwrap_or_default();
        self.send_frame_writes(writes, client_commands);
        // `G_RunFrame`'s inlined drain (map-cycle doc, 6.3): a frame that
        // moved a score pushes the scoreboard to every client in
        // intermission, and no frame pushes one otherwise. The map-change
        // capture carries no `b` its probe did not ask for, so a timer here
        // would be a command retail never sends.
        if ranks_dirty {
            let watching: Vec<usize> = self
                .core
                .clients
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.as_ref()
                        .and_then(|c| c.sim.as_ref())
                        .is_some_and(|s| s.pm_type == crate::spectate::PmType::Intermission)
                })
                .map(|(slot, _)| slot)
                .collect();
            for slot in watching {
                crate::tick_trace::step!("scoreboard slot {slot}");
                let text = self.scoreboard();
                self.send_server_command(slot, &text);
            }
        }

        true
    }

    /// The game and what the engine lends it for one call, borrowed apart.
    pub(super) fn split(&mut self, svs_time_ms: i32) -> (&mut Cod1Module, Lend<'_>) {
        (
            &mut self.game.module,
            Lend {
                clients: &mut self.core.clients,
                world: &self.game.world,
                fs: &self.game.fs,
                proto: self.game.proto,
                sv_time_ms: self.core.sv_time_ms,
                svs_time_ms,
                rng: &mut self.core.rng,
                stuck_rand: &mut self.game.stuck_rand,
                configstrings: &self.core.configstrings,
                archive: &mut self.game.archive,
                test_client_counter: &mut self.game.test_client_counter,
                udp_slots: self.game.udp_slots,
            },
        )
    }

    /// `SV_Frame`'s last step (0x808d258).
    pub(super) fn master_heartbeat(&mut self) {
        let mut resolve = self.core.resolver;
        let beats = self.core.masters.heartbeat(
            self.game.dedicated,
            self.svs_time_ms(),
            crate::master::HEARTBEAT_GAME,
            &mut resolve,
        );
        self.core.outbox.extend(beats);
        // The weapon changes are already drained when a script is loaded,
        // and a server without one has nothing to write them to.
        self.game.module.weapon_changes.clear();
    }

    /// `SV_ExecuteClientMessage` for every packet since the last tick, in
    /// arrival order, each cmd a `ClientThink_real`
    /// ([`Cod1Module::client_think_cod1`]). Returns what each slot
    /// replayed.
    pub(super) fn replay_moves(&mut self) -> Vec<MoveSummary> {
        let svs = self.svs_time_ms();
        let (game, mut eng) = self.split(svs);
        game.client_think_cod1(&mut eng)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// `probe_startclock` on retail: the script clock starts on the load's
    /// `level.time`, the settle frames walk it 100 ms at a time, and a thread
    /// due at a frame's `level.time` runs the frame after
    /// (docs/research/cod11-gsc-language.md, "The script clock").
    #[test]
    fn the_script_clock_matches_probe_startclock() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                gametype: "probe_startclock".into(),
                ..cfg()
            },
            now,
        );
        sv.overlay_script(
            "maps/mp/gametypes/probe_startclock",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../gsc/tests/fixtures/semantics/client-probes/probe_startclock.gsc"
            )),
        );
        // The first load runs the settle frames a map change does.
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for _ in 0..36 {
            sv.tick(now);
        }
        let mut log: Vec<String> = sv
            .script_log()
            .iter()
            .map(|l| l.trim().to_string())
            .collect();
        // The `wait 0.05` loop's 6..10 rows add nothing past its 5th.
        log.retain(|l| {
            !matches!(
                l.as_str(),
                "PROBE tick 7 450" | "PROBE tick 8 500" | "PROBE tick 9 550" | "PROBE tick 10 600"
            )
        });
        assert_eq!(
            log,
            [
                "PROBE main 0",
                "PROBE start 0",
                "PROBE wait0 100",
                "PROBE tick 0 100",
                "PROBE tick 1 200",
                "PROBE wait01 200",
                "PROBE tick 2 200",
                "PROBE tick 3 300",
                "PROBE tick 4 300",
                "PROBE tick 5 350",
                "PROBE wait02 350",
                "PROBE tick 6 400",
                "PROBE tick 11 650",
                "PROBE wait1 1050",
                "PROBE wait1b 2050",
            ]
        );
    }

    /// Off the LAN a primed client gets a bare message a second apart, and
    /// entering the world makes it due at once (0x80877d8 sets
    /// `nextSnapshotTime` to `svs.time`): live, retail answered the entering
    /// cmd 15 ms later (cod11-server-handshake.md, "Clients not in the
    /// world").
    #[test]
    fn entering_the_world_cuts_the_primed_keepalive_short() {
        let huff = Huffman::new();
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let wan = SocketAddr::from(([203, 0, 113, 9], 27960));
        let mut nc = connected(&mut sv, wan, now);
        let pkt = nc.build_out(0, 0, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(wan, &pkt, now);
        sv.take_outgoing();
        sv.core.clients[0].as_mut().unwrap().link.unsent.clear();
        let mut bare = Vec::new();
        for i in 0..21 {
            sv.tick(now + Duration::from_millis(50 * i));
            bare.extend(sv.take_outgoing().into_iter().map(|_| i));
        }
        assert_eq!(bare, [19], "one keepalive, a second after the gamestate");
        let t = now + Duration::from_millis(1100);
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
        assert_eq!(
            sv.take_outgoing().len(),
            1,
            "the first snapshot goes at once"
        );
    }

    /// `ps.commandTime` must name a usercmd we actually simulated, never a
    /// later moment. A client replays every cmd past commandTime, so claiming
    /// to have simulated further than we have silently drops that slice of
    /// its input on every frame and its prediction judders (retail trace,
    /// 2026-08-28: commandTime ran 11-24 ms ahead on 100% of frames).
    #[test]
    fn command_time_never_runs_ahead_of_the_last_simulated_cmd() {
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

        // A client whose cmd clock trails the server's, as a real one's does:
        // it runs its own serverTime behind so it has frames to interpolate.
        let mut at = now;
        let mut cmd_time = 0i32;
        for _ in 0..8 {
            at += std::time::Duration::from_millis(50);
            cmd_time += 40; // 10 ms per frame behind the server's 50
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
                            server_time: cmd_time,
                            forward: 127,
                            ..Default::default()
                        },
                    ),
                    &Huffman::new(),
                )
                .unwrap(),
                at,
            );
            let s = latest_snapshot(&mut sv, &mut nc, &mut ring, at);
            let ct = s.ps.field_i32(p, "commandTime");
            assert!(
                ct <= cmd_time,
                "commandTime {ct} is ahead of the newest cmd we simulated ({cmd_time})"
            );
        }
    }

    /// A client holding its last frag and nothing else, in a server running
    /// `script`, with the `serverTime` its next cmd builds on.
    fn last_frag_in_hand(script: &str) -> Option<(Server, i32, usize)> {
        last_frag_in_hand_under(crate::game::script::ScriptRuntime::for_test(script))
    }

    /// `last_frag_in_hand` under a runtime the caller built.
    fn last_frag_in_hand_under(
        rt: crate::game::script::ScriptRuntime,
    ) -> Option<(Server, i32, usize)> {
        let fs = vcod_common::testing::game_fs()?;
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
        install_script(&mut sv, rt);
        sv.game.module.weapon_table =
            Rc::new(crate::weapons::WeaponTable::load(&fs, &crate::defs::V1));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 64.0], 0.0);
        let frag = crate::defs::V1.weapon_index("fraggrenade_mp").unwrap();
        let def = sv.game.module.weapon_table.get(frag).unwrap().clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        held.give(frag, sv.game.module.weapon_table.slot(frag));
        held.current = frag as u8;
        let rt = sv.game.module.script.as_mut().unwrap();
        rt.host.weapons = sv.game.module.weapon_table.clone();
        rt.host.precache_items_for_test(&["fraggrenade_mp"]);
        rt.host.clients[0].weapons = held;
        let c = sv.core.clients[0].as_mut().unwrap();
        let sim = c.sim.as_mut().unwrap();
        sim.ps.weapons_held = held.held;
        sim.ps.weapon_slots = held.slots;
        sim.ps.weapon = frag as u8;
        sim.ps.ammoclip[def.clip_index] = 1;
        sim.ps.ammo[def.ammo_index] = 0;
        let st = c.last_processed_st;
        Some((sv, st, frag))
    }

    /// The cmds that hold the trigger through the pullback and then let go:
    /// the release is the throw, and the throw is the last round.
    fn frag_throw_cmd(i: i32, st: i32, frag: usize) -> UserCmd {
        use vcod_common::net::msg::BUTTON_ATTACK;
        UserCmd {
            server_time: st + 50 * (i + 1),
            weapon: frag as u8,
            buttons: if i < 20 { BUTTON_ATTACK } else { 0 },
            ..NULL_USERCMD
        }
    }

    /// A `clipOnly` weapon's last round takes it before that cmd's touch
    /// pass, as retail's `PM_Weapon` does ahead of `G_TouchTriggers`
    /// (combat doc 1.5 step 9): a grab on the same cmd must not see the frag
    /// as held and top it up. Read straight after `replay_moves`, which is
    /// where the touch pass ends and before the script frame.
    #[test]
    fn the_last_frag_is_taken_before_that_cmd_s_touch() {
        let Some((mut sv, st, frag)) = last_frag_in_hand("main() {}") else {
            return;
        };
        for i in 0..60 {
            let cmd = frag_throw_cmd(i, st, frag);
            sv.core.clients[0]
                .as_mut()
                .unwrap()
                .pending
                .push(cmd.into());
            // The frame's clock keeps up with the cmds, which `replay_moves`
            // clamps to 200 ms past `level.time`.
            sv.core.sv_time_ms = cmd.server_time;
            sv.replay_moves();
            let rt = sv.game.module.script.as_ref().unwrap();
            let thrown = rt.missiles().entities(sv.game.proto).next().is_some();
            if thrown {
                assert!(
                    !rt.host.clients[0].weapons.holds(frag),
                    "the spent frag was still held when its cmd's touch ran"
                );
                return;
            }
        }
        panic!("the frag was never thrown");
    }

    /// `FireWeapon` spawns the grenade inside the release cmd (combat doc,
    /// 11.4): a run queued behind the release in the same tick leaves the
    /// missile where the release stood, stamped with the frame before.
    #[test]
    fn a_throw_leaves_from_its_own_cmd_not_the_tick_s_last() {
        let Some((mut sv, st, frag)) = last_frag_in_hand("main() {}") else {
            return;
        };
        let now = Instant::now();
        for i in 0..20 {
            let cmd = frag_throw_cmd(i, st, frag);
            sv.core.clients[0]
                .as_mut()
                .unwrap()
                .pending
                .push(cmd.into());
            sv.tick(now);
        }
        let stood = sv.core.clients[0]
            .as_ref()
            .unwrap()
            .sim
            .as_ref()
            .unwrap()
            .origin();
        let release = frag_throw_cmd(20, st, frag);
        let run = UserCmd {
            server_time: release.server_time + 300,
            forward: 127,
            ..release
        };
        let c = sv.core.clients[0].as_mut().unwrap();
        c.pending.push(release.into());
        c.pending.push(run.into());
        sv.tick(now);
        let ran = sv.core.clients[0]
            .as_ref()
            .unwrap()
            .sim
            .as_ref()
            .unwrap()
            .origin();
        assert!(
            (glam::Vec3::from(ran) - glam::Vec3::from(stood)).length() > 30.0,
            "the run behind the release moved {stood:?} to {ran:?}"
        );
        let rt = sv.game.module.script.as_ref().unwrap();
        let (_, e) = rt
            .missiles()
            .entities(sv.game.proto)
            .next()
            .expect("the release threw nothing");
        let traj = vcod_common::net::trajectory::Trajectory::read(&e, sv.game.proto, "pos");
        assert_eq!(traj.tr_time, sv.core.sv_time_ms - FRAME_MS);
        assert_eq!(
            [traj.base.x, traj.base.y],
            [stood[0].trunc(), stood[1].trunc()],
            "the grenade left from {:?}, not the release's {stood:?}",
            traj.base
        );
    }

    /// A `trigger_hurt` death runs `player_die` inside the victim's own
    /// `G_TouchTriggers`, where `r.currentOrigin` is the snapped
    /// `s.pos.trBase` (combat doc 5.5): the live grenade and the weapon the
    /// killed callback drops both leave from whole units, truncated toward
    /// zero.
    #[test]
    fn a_trigger_hurt_death_drops_and_clones_at_the_snapped_origin() {
        use vcod_common::net::msg::BUTTON_ATTACK;
        let rt = crate::game::script::ScriptRuntime::for_test_at(
            crate::game::script::CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerDamage(inflictor, attacker, damage, flags, mod, weapon, point, \
             dir, hitloc) { self finishPlayerDamage(inflictor, attacker, damage, flags, mod, \
             weapon, point, dir, hitloc); }\n\
             CodeCallback_PlayerKilled(inflictor, attacker, damage, mod, weapon, dir, hitloc) \
             { self dropItem(self getcurrentweapon()); body = self cloneplayer(); }\n",
        );
        let Some((mut sv, st, frag)) = last_frag_in_hand_under(rt) else {
            return;
        };
        sv.test_set_client_origin(0, [10.6, -20.3, 1.0]);
        let cook = |i: i32| UserCmd {
            server_time: st + 50 * (i + 1),
            weapon: frag as u8,
            buttons: BUTTON_ATTACK,
            ..NULL_USERCMD
        };
        fn sim(sv: &Server) -> &ClientSim {
            sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap()
        }
        let mut i = 0;
        while sim(&sv).ps.grenade_time_left_ms == 0 {
            assert!(i < 60, "the pin never came out");
            sv.core.clients[0]
                .as_mut()
                .unwrap()
                .pending
                .push(cook(i).into());
            sv.replay_moves();
            i += 1;
        }
        let stood = sim(&sv).origin();
        assert!(
            stood[0].fract() != 0.0 && stood[1].fract() != 0.0,
            "the cook settled on the unit grid at {stood:?}"
        );
        let rt = sv.game.module.script.as_mut().unwrap();
        let hurt = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.register_hurt_for_test(
            hurt,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, -8.0], [64.0, 64.0, 64.0]),
            1000,
        );
        sv.core.clients[0]
            .as_mut()
            .unwrap()
            .pending
            .push(cook(i).into());
        sv.replay_moves();

        let snapped = glam::Vec3::from(stood).trunc();
        assert_eq!(snapped.truncate(), glam::Vec2::new(10.0, -20.0));
        let rt = sv.game.module.script.as_mut().unwrap();
        let (_, e) = rt
            .missiles()
            .entities(sv.game.proto)
            .next()
            .expect("the death dropped no grenade");
        let traj = vcod_common::net::trajectory::Trajectory::read(&e, sv.game.proto, "pos");
        assert_eq!(
            traj.base,
            snapped + glam::Vec3::Z * 40.0,
            "the grenade left from {:?}, standing at {stood:?}",
            traj.base
        );
        let item = rt
            .host
            .ents
            .iter_inuse()
            .find(|(_, e)| e.item.is_some_and(|i| i.dropped))
            .map(|(id, _)| id)
            .expect("the killed callback dropped no weapon");
        let at = rt.entity_origin_of(item).unwrap();
        assert_eq!([at[0], at[1]], [snapped.x, snapped.y]);
        // `G_SetOrigin(body, self->r.currentOrigin)` (5.2): the corpse too.
        let (_, body) = rt.host.bodies.entities().next().expect("no corpse");
        let at = body.origin(sv.game.proto);
        assert_eq!([at[0], at[1]], [snapped.x, snapped.y]);
    }

    /// A spawn and its loadout whose weapon ops something applies ahead of
    /// the frame's spawn pass (here the move pass, after the damage
    /// callback; on UO a vehicle round's callbacks) keep the loadout: the
    /// spawn's emptying of the ammo is queued with them, in call order, and
    /// the spawn pass leaves the arrays alone (cod11-gsc-object-model.md 20,
    /// "The spawn's memset and the weapon op queue").
    #[test]
    fn a_loadout_applied_before_the_spawn_pass_survives_it() {
        let rt = crate::game::script::ScriptRuntime::for_test_at(
            crate::game::script::CALLBACK_SETUP,
            "main() {}\n\
             CodeCallback_PlayerDamage(inflictor, attacker, damage, flags, mod, weapon, point, \
             dir, hitloc) { self spawn((400, 0, 64), (0, 0, 0)); \
             self giveWeapon(\"fraggrenade_mp\"); }\n",
        );
        let Some((mut sv, st, frag)) = last_frag_in_hand_under(rt) else {
            return;
        };
        let def = sv.game.module.weapon_table.get(frag).unwrap().clone();
        let rt = sv.game.module.script.as_mut().unwrap();
        let hurt = rt.spawn_map_entity_for_test([0.0, 0.0, 0.0]);
        rt.register_hurt_for_test(
            hurt,
            crate::game::trigger::TriggerShape::boxed([-64.0, -64.0, -8.0], [64.0, 64.0, 64.0]),
            10,
        );
        let cmd = UserCmd {
            server_time: st + 50,
            weapon: frag as u8,
            ..NULL_USERCMD
        };
        sv.core.clients[0]
            .as_mut()
            .unwrap()
            .pending
            .push(cmd.into());
        sv.replay_moves();
        fn sim(sv: &Server) -> &ClientSim {
            sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap()
        }
        let full = def.clip_size as i16;
        assert_eq!(sim(&sv).ps.ammoclip[def.clip_index], full, "the give");
        assert_eq!(
            sv.game
                .module
                .script
                .as_ref()
                .unwrap()
                .host
                .client_spawns
                .len(),
            1
        );
        sv.tick(Instant::now() + Duration::from_millis(50));
        assert!(
            sv.game
                .module
                .script
                .as_ref()
                .unwrap()
                .host
                .client_spawns
                .is_empty()
        );
        let at = sim(&sv).origin();
        assert!(at[0] > 300.0, "no spawn ran: {at:?}");
        assert_eq!(sim(&sv).ps.ammoclip[def.clip_index], full, "emptied");
    }

    /// The take happens once, at the cmd's touch: a frag the script gives
    /// back in the frame it was thrown stays held.
    #[test]
    fn a_frag_given_back_in_the_frame_it_was_thrown_stays_held() {
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        let script = "main() { \
               precacheitem(\"fraggrenade_mp\"); \
               while (1) { \
                 wait 0.05; \
                 g = getentarray(\"grenade\", \"classname\"); \
                 if (g.size > 0) { \
                   p = getentarray(\"noclass\", \"classname\"); \
                   p[0] giveweapon(\"fraggrenade_mp\"); \
                   logprint(\"regive\"); \
                   return; \
                 } \
               } \
             }";
        let Some((mut sv, st, frag)) = last_frag_in_hand(script) else {
            return;
        };
        let now = Instant::now();
        for i in 0..60 {
            let cmd = frag_throw_cmd(i, st, frag);
            sv.core.clients[0]
                .as_mut()
                .unwrap()
                .pending
                .push(cmd.into());
            sv.tick(now);
            if sv.script_log().iter().any(|l| l.trim() == "regive") {
                assert!(
                    sv.game.module.script.as_ref().unwrap().host.clients[0]
                        .weapons
                        .holds(frag),
                    "the frag the script gave back was taken again"
                );
                return;
            }
        }
        panic!("the script never saw the grenade");
    }

    /// A use key on the cmd whose move finished a switch grabs with the new
    /// `ps.weapon` in hand: both primaries are full, so the swap drops the
    /// weapon being held, and with the colt the tick started on it would
    /// have dropped the carbine from the primary slot instead (section 5).
    #[test]
    fn a_grab_on_the_switch_cmd_swaps_out_the_new_weapon() {
        use vcod_common::net::msg::BUTTON_USE;
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 1.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.game.module.weapon_table =
            Rc::new(crate::weapons::WeaponTable::load(&fs, &crate::defs::V1));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
        let index = |n| crate::defs::V1.weapon_index(n).unwrap();
        let (carbine, thompson, colt) = (
            index("m1carbine_mp"),
            index("thompson_mp"),
            index("colt_mp"),
        );
        let table = sv.game.module.weapon_table.clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        for (w, slot) in [(carbine, 1), (thompson, 2), (colt, 3)] {
            held.give(w, slot);
        }
        held.current = colt as u8;
        let rt = sv.game.module.script.as_mut().unwrap();
        rt.host.weapons = table.clone();
        rt.host
            .precache_items_for_test(&["m1carbine_mp", "thompson_mp", "colt_mp"]);
        rt.host.clients[0].weapons = held;
        rt.host.clients[0].vitals = crate::game::host::Vitals {
            health: 100,
            max_health: 100,
            dead: false,
            takedamage: true,
        };
        let pf = rt.place_item("mpweapon_panzerfaust", [40.0, 0.0, 50.0], 0);
        let c = sv.core.clients[0].as_mut().unwrap();
        let sim = c.sim.as_mut().unwrap();
        sim.ps.weapons_held = held.held;
        sim.ps.weapon_slots = held.slots;
        sim.ps.weapon = colt as u8;
        for w in [carbine, thompson, colt] {
            let d = table.get(w).unwrap();
            sim.ps.ammoclip[d.clip_index] = d.clip_size as _;
            sim.ps.ammo[d.ammo_index] = d.max_ammo as _;
        }
        // One cmd long enough for the colt's putaway to end inside it.
        let cmd = UserCmd {
            server_time: c.last_processed_st + 1000,
            weapon: thompson as u8,
            buttons: BUTTON_USE,
            ..NULL_USERCMD
        };
        c.pending.push(cmd.into());
        sv.core.sv_time_ms = cmd.server_time;
        sv.replay_moves();
        let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
        assert_eq!(sim.ps.weapon, thompson as u8, "the switch did not land");
        let rt = sv.game.module.script.as_mut().unwrap();
        assert!(rt.host.ents.get(pf).unwrap().item.unwrap().taken);
        let dropped: Vec<usize> = rt
            .host
            .ents
            .iter_inuse()
            .filter_map(|(_, e)| e.item.filter(|i| i.dropped).map(|i| i.index as usize))
            .collect();
        assert_eq!(dropped, vec![thompson]);
    }

    /// `Touch_Item` writes the playerstate inside the toucher's cmd (items
    /// doc, 13.2): a walk-over ammo grab is in the sim, and its 148 on the ring,
    /// before the cmd behind it fires, with no hit needed to land it there.
    /// Read straight after `replay_moves`, ahead of the script frame.
    /// `ClientEndFrame` stores `g_speed` in a 1.1 player's `ps.speed`
    /// (game.mp 0x41100): the body runs at it and the wire carries it.
    #[test]
    fn a_player_runs_at_g_speed() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 1.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
        let rt = sv.game.module.script.as_mut().unwrap();
        rt.set_client_state_for_test(0, "playing");
        rt.host.cvars.set("g_speed", "250.9");
        sv.tick(now + Duration::from_millis(50));
        let c = sv.core.clients[0].as_mut().unwrap();
        assert_eq!(c.sim.as_ref().unwrap().ps.speed, 250.0);
        let st = c.last_processed_st;
        for i in 1..=20 {
            let cmd = UserCmd {
                server_time: st + 50 * i,
                forward: 127,
                ..NULL_USERCMD
            };
            c.pending.push(cmd.into());
        }
        sv.replay_moves();
        let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
        let h = sim.ps.velocity.truncate().length();
        assert!((h - 250.0).abs() < 1.0, "{h}");
        let w = sim.to_wire(&PROTOCOL_V1, 0, 0);
        assert_eq!(w.field_i32(&PROTOCOL_V1, "speed"), 250);
    }

    /// Past `MAX_CMDS_PER_TICK` a flood keeps its newest cmd, which covers
    /// the dropped span once: 50 one-ms cmds a tick for 2 s run no farther
    /// than 25 two-ms ones, and `commandTime` ends each tick on the newest.
    #[test]
    fn a_cmd_flood_runs_no_faster_than_real_time() {
        let run = |per_tick: i32| {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.load_world(World {
                collision: test_world(&[]),
                vis: vcod_common::bsp::Visibility::none(),
                spawn: ([0.0, 0.0, 1.0], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
                rubble: Vec::new(),
                nav_cover: Vec::new(),
            });
            install_script(
                &mut sv,
                crate::game::script::ScriptRuntime::for_test("main() {}"),
            );
            let _nc = begun(&mut sv, now);
            sv.tick(now);
            sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
            let rt = sv.game.module.script.as_mut().unwrap();
            rt.set_client_state_for_test(0, "playing");
            sv.tick(now + Duration::from_millis(50));
            let st = sv.core.clients[0].as_ref().unwrap().last_processed_st;
            let start = sv.core.clients[0]
                .as_ref()
                .unwrap()
                .sim
                .as_ref()
                .unwrap()
                .ps
                .origin;
            let step = FRAME_MS / per_tick;
            for tick in 1..=40 {
                let c = sv.core.clients[0].as_mut().unwrap();
                let newest = st + FRAME_MS * tick;
                for j in (0..per_tick).rev() {
                    let cmd = UserCmd {
                        server_time: newest - step * j,
                        forward: 127,
                        ..NULL_USERCMD
                    };
                    c.pending.push(cmd.into());
                }
                sv.tick(now + Duration::from_millis(50 + 50 * tick as u64));
                let c = sv.core.clients[0].as_ref().unwrap();
                assert_eq!(
                    c.last_processed_st, newest,
                    "{per_tick} a tick, tick {tick}"
                );
            }
            let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
            (sim.ps.origin - start).truncate().length()
        };
        let (honest, flood) = (run(25), run(50));
        assert!(honest > 300.0, "{honest}");
        assert!(flood <= honest + 1.0, "flood {flood} against {honest}");
    }

    /// `ClientEndFrame` stores `(int)g_gravity` in a player's `ps.gravity`
    /// (game.mp 0x410aa): a jump climbs as high and stays up `sqrt 2` times
    /// as long at 400, and the wire carries it (cod15-movement.md,
    /// "ps.gravity").
    #[test]
    fn a_player_jumps_at_g_gravity() {
        let airtime = |gravity: &str| {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.load_world(World {
                collision: test_world(&[]),
                vis: vcod_common::bsp::Visibility::none(),
                spawn: ([0.0, 0.0, 1.0], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
                rubble: Vec::new(),
                nav_cover: Vec::new(),
            });
            install_script(
                &mut sv,
                crate::game::script::ScriptRuntime::for_test("main() {}"),
            );
            let _nc = begun(&mut sv, now);
            sv.tick(now);
            sv.place_client(0, [0.0, 0.0, 0.125], 0.0);
            let rt = sv.game.module.script.as_mut().unwrap();
            rt.set_client_state_for_test(0, "playing");
            rt.host.cvars.set("g_gravity", gravity);
            sv.tick(now + Duration::from_millis(50));
            let st = sv.core.clients[0].as_ref().unwrap().last_processed_st;
            let mut top = 0.0f32;
            for i in 1..=60 {
                sv.core.clients[0].as_mut().unwrap().pending.push(
                    UserCmd {
                        server_time: st + 50 * i,
                        up: if i == 1 { 127 } else { 0 },
                        ..NULL_USERCMD
                    }
                    .into(),
                );
                sv.tick(now + Duration::from_millis(50 + 50 * i as u64));
                let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
                top = top.max(sim.ps.origin.z);
                if i > 1 && sim.ps.on_ground {
                    let w = sim.to_wire(&PROTOCOL_V1, 0, 0);
                    return (w.field_i32(&PROTOCOL_V1, "gravity"), top, 50 * i);
                }
            }
            panic!("no landing at g_gravity {gravity}");
        };
        let (stock, light) = (airtime("800"), airtime("400.7"));
        assert_eq!((stock.0, light.0), (800, 400));
        assert!((light.1 - stock.1).abs() < 1.5, "{light:?} vs {stock:?}");
        let ratio = light.2 as f32 / stock.2 as f32;
        assert!((ratio - 2f32.sqrt()).abs() < 0.1, "{light:?} vs {stock:?}");
    }

    #[test]
    fn a_walk_over_pickup_lands_before_the_next_cmd() {
        use vcod_common::net::msg::BUTTON_ATTACK;
        use vcod_common::pmove::weapon::EV_FIRE_WEAPON;
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 1.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        sv.game.module.weapon_table =
            Rc::new(crate::weapons::WeaponTable::load(&fs, &crate::defs::V1));
        let _nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 1.0], 0.0);
        let carbine = crate::defs::V1.weapon_index("m1carbine_mp").unwrap();
        let def = sv.game.module.weapon_table.get(carbine).unwrap().clone();
        let mut held = crate::weapons::PlayerWeapons::default();
        held.give(carbine, sv.game.module.weapon_table.slot(carbine));
        held.current = carbine as u8;
        let rt = sv.game.module.script.as_mut().unwrap();
        rt.host.weapons = sv.game.module.weapon_table.clone();
        rt.host.precache_items_for_test(&["fraggrenade_mp"]);
        rt.host.clients[0].weapons = held;
        rt.host.clients[0].vitals = crate::game::host::Vitals {
            health: 100,
            max_health: 100,
            dead: false,
            takedamage: true,
        };
        rt.place_item("mpweapon_m1carbine", [0.0, 0.0, 1.0], 20);
        let c = sv.core.clients[0].as_mut().unwrap();
        let sim = c.sim.as_mut().unwrap();
        sim.ps.weapons_held = held.held;
        sim.ps.weapon_slots = held.slots;
        sim.ps.weapon = carbine as u8;
        sim.ps.ammoclip[def.clip_index] = def.clip_size as _;
        sim.ps.ammo[def.ammo_index] = 10;
        let st = c.last_processed_st;
        for (i, buttons) in [0, BUTTON_ATTACK].into_iter().enumerate() {
            let cmd = UserCmd {
                server_time: st + 50 * (i as i32 + 1),
                weapon: carbine as u8,
                buttons,
                ..NULL_USERCMD
            };
            c.pending.push(cmd.into());
        }
        sv.replay_moves();
        let sim = sv.core.clients[0].as_ref().unwrap().sim.as_ref().unwrap();
        assert_eq!(sim.ps.ammo[def.ammo_index], 30, "the grab's ammo");
        let ring = sim.ring;
        let drained: Vec<i32> = (0..ring.seq)
            .map(|i| ring.events[(i & 3) as usize])
            .collect();
        assert_eq!(
            drained,
            vec![crate::game::pickup::EV_AMMO_PICKUP, EV_FIRE_WEAPON],
            "the grab's event ahead of the shot behind it"
        );
    }

    /// `getPlant` reads a planter's `self.angles` for the direction of its
    /// first trace, and `ClientThink_real` writes a player's after every cmd:
    /// the view's yaw with pitch and roll 0
    /// (docs/research/cod11-gsc-object-model.md 23.6). The spawn's yaw is not
    /// what it reads once the client has turned.
    #[test]
    fn a_player_s_angles_field_carries_its_view_yaw() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test("main() {}"),
        );
        let mut nc = begun(&mut sv, now);
        sv.tick(now);
        sv.place_client(0, [0.0, 0.0, 64.0], 90.0);
        // Pitch 22.5 down, yaw 45 on the wire; the placed spawn's delta
        // turns the yaw into 135.
        let ack = nc.incoming_sequence as i32;
        let cmd = UserCmd {
            server_time: 100,
            angles: [4096, 8192, 0],
            ..Default::default()
        };
        let pkt = nc
            .build_out(
                i32::from(sv.game.server_id),
                ack,
                0,
                &move_ops(sv.core.checksum_feed, ack, cmd),
                &Huffman::new(),
            )
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        sv.tick(now + Duration::from_millis(50));
        let rt = sv.game.module.script.as_mut().unwrap();
        let ent = rt.client_entity(0).expect("client 0 has an entity");
        assert_eq!(rt.field_str(ent, "angles"), "(0.00, 135.00, 0.00)");
    }

    /// The gap `_minefields.gsc` puts between the warning click and the
    /// blast, measured on our clock. The stock script (pak5.pk3,
    /// `maps/MP/_minefields.gsc`) is `playsound("minefield_click");
    /// wait(.5); wait(randomFloat(.5));` and then an `istouching` re-check,
    /// so retail's gap is 500 to 1000 ms by construction and every draw has
    /// to land in it. A `wait` wakes on the first frame past its deadline,
    /// which is what buys the extra frame at the top.
    ///
    /// The draws also have to differ: a `randomFloat` stuck at one value
    /// would sit inside that window and pass everything else here.
    ///
    /// Six draws on our clock read 550, 700, 850, 750, 800, 850 ms, which is
    /// what settles the open question of whether ours detonates sooner than
    /// retail: it does not, and neither `wait` nor `randomFloat` is short.
    #[test]
    fn the_minefield_kill_delay_is_half_to_one_second() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { \
                   while (1) { \
                     wait 0.05; \
                     players = getentarray(\"noclass\", \"classname\"); \
                     if (players.size > 0) { \
                       for (i = 0; i < 6; i++) { players[0] mine(); } \
                       return; \
                     } \
                   } \
                 } \
                 mine() { \
                   self playsound(\"minefield_click\"); \
                   logprint(\"click\"); \
                   wait(.5); \
                   wait(randomFloat(.5)); \
                   logprint(\"boom\"); \
                 }",
            ),
        );
        let _nc = begun(&mut sv, now);

        let mut drained = sv.script_log().len();
        let mut click: Option<i32> = None;
        let mut gaps: Vec<i32> = Vec::new();
        for frame in 0..200 {
            sv.tick(now);
            for line in &sv.script_log()[drained..] {
                match line.trim() {
                    "click" => click = Some(frame),
                    "boom" => gaps.push((frame - click.take().expect("a click first")) * FRAME_MS),
                    _ => {}
                }
            }
            drained = sv.script_log().len();
        }

        assert_eq!(gaps.len(), 6, "six kills ran to the blast");
        for gap in &gaps {
            assert!(
                (500..=1000 + FRAME_MS).contains(gap),
                "a kill waited {gap} ms; the stock script waits 500 to 1000, \
                 plus at most one frame of wake quantization. All: {gaps:?}"
            );
        }
        assert!(
            gaps.iter().any(|g| *g != gaps[0]),
            "every draw was {} ms, so randomFloat is not drawing",
            gaps[0]
        );
    }

    /// Entry takes its `delta_angles` from the cmd that entered the world,
    /// not from zero. `SV_ClientEnterWorld` stores `cmds[0]` in
    /// `lastUsercmd`, and `ClientSpawn` reads it back through
    /// `trap_GetUsercmd` into `pers.cmd` on the line before
    /// `SetClientViewAngle` subtracts it (RTCW-MP `g_client.c`, `sv_game.c`).
    /// A client that was already looking somewhere when it entered keeps that
    /// view; zeroing the pair snaps it, and the move basis rotates with it.
    /// Every other entry in the suite enters with zero angles, so this is the
    /// only thing holding the argument in place.
    #[test]
    fn entry_takes_delta_angles_from_the_entering_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 90.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
        });
        let mut nc = active(&mut sv, now);

        // Pitch 22.5 degrees, yaw 45: a client that looked around on the
        // loading screen before its first cmd went out.
        let entering = UserCmd {
            server_time: 50,
            angles: [4096, 8192, 0],
            ..Default::default()
        };
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(sv.core.checksum_feed, ack, entering),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );

        let mut ring = SnapshotRing::new();
        let s = latest_snapshot(&mut sv, &mut nc, &mut ring, now);
        let p = &PROTOCOL_V1;
        // `ANGLE2SHORT(90) - cmd.angles[1]` on the yaw, `-cmd.angles[i]` on
        // the other two, all as the 16-bit wire words the field carries.
        assert_eq!(s.ps.field_i32(p, "delta_angles[0]"), -4096 & 0xffff);
        assert_eq!(s.ps.field_i32(p, "delta_angles[1]"), 16_384 - 8192);
        assert_eq!(s.ps.field_i32(p, "delta_angles[2]"), 0);
    }

    /// A UO server heartbeats UO's master unless `sv_master1` says otherwise.
    #[test]
    fn sv_master1_defaults_to_the_protocols_master() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_protocol(&PROTOCOL_V22);
        assert_eq!(sv.core.masters.name(0), "master.cod.pm");
        sv.set_cvar("sv_master1", "127.0.0.1");
        sv.set_protocol(&PROTOCOL_V22);
        assert_eq!(sv.core.masters.name(0), "127.0.0.1");
    }

    #[test]
    fn dedicated_2_heartbeats_and_flatlines() {
        let t = Instant::now();
        let mut sv = Server::new(cfg(), t);
        sv.set_master_resolver(local_master);
        let beats = |sv: &mut Server| -> Vec<String> {
            sv.take_outgoing()
                .into_iter()
                .filter(|(to, _)| *to == addr(crate::master::DEFAULT_MASTER.port))
                .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
                .collect()
        };
        sv.tick(t);
        assert!(beats(&mut sv).is_empty(), "dedicated 1 stays quiet");
        sv.set_cvar("dedicated", "2");
        sv.tick(t);
        assert_eq!(beats(&mut sv), ["heartbeat COD-1\n"]);
        sv.tick(t);
        assert!(beats(&mut sv).is_empty());
        // One packet a second to a master: the beat a new client forces
        // waits that out.
        let _ = connected(&mut sv, addr(5), t);
        sv.tick(t);
        assert!(beats(&mut sv).is_empty());
        sv.push_console("heartbeat");
        sv.tick_frames(t, 20);
        assert_eq!(beats(&mut sv), ["heartbeat COD-1\n"]);
        sv.tick_frames(t, 20);
        sv.push_console("quit");
        sv.tick(t);
        assert_eq!(beats(&mut sv), ["heartbeat flatline\n"]);
        assert!(sv.quit_requested());
    }
}
