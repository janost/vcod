//! `Mode::Online`'s frame: the net, the cmds and the phase's draw.

use super::*;
use crate::redraw::Tick;

impl App {
    pub(crate) fn online_frame(
        &mut self,
        tick: &Tick,
        demo_dirs: &[std::path::PathBuf],
        hud_quads: &mut Vec<hud::HudQuad>,
        fatal: &mut Option<anyhow::Error>,
        demo_over: &mut bool,
    ) -> (renderer::Frame, Option<renderer::VmDraw>) {
        let Tick {
            now,
            dt,
            time,
            local_ms,
            cull,
            blood,
            eject_brass,
            aspect,
            widescreen,
            ..
        } = *tick;
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Online {
            net,
            cam,
            input,
            clock: cmd_clock,
            ring,
            predictor,
            view,
            phase,
            join,
        } = &mut self.mode
        else {
            unreachable!()
        };
        // `VCOD_FRAME_PROFILE`'s stages through the frame; the
        // player bodies inside `build_instances` are `bodies`.
        let mut prof = frame_prof::span(frame_prof::Stage::Net);
        let mut vm = None;
        // A demo is read up to the clock (`CL_SetCGameTime`,
        // CoDMP.exe 0x404d60), nothing while a map loads.
        let mut demo_clock = play::clock::DemoClock::default();
        let events = if net.is_demo() {
            let knobs = play::demos::DemoKnobs {
                freeze: self.shell.cvar_f32("cl_freezeDemo") != 0.0,
                timedemo: self.shell.cvar_f32("timedemo") != 0.0,
                nudge: self.shell.cvar_f32("cl_timeNudge") as i32,
            };
            match &mut *phase {
                Phase::Live(live) => {
                    let (events, how) = play::demos::read_frame(
                        net,
                        &mut live.clock,
                        &mut self.timedemo,
                        now,
                        local_ms as i32,
                        knobs,
                    );
                    demo_clock = how;
                    events
                }
                _ => net.pump_demo(now, None),
            }
        } else {
            net.pump()
        };
        let mut gamestate_ready = false;
        for ev in &events {
            if let net::NetEvent::Chat { team, .. } = ev
                && !hud::chat::takes(*team, self.shell.cvar("cg_teamChatsOnly"))
            {
                continue;
            }
            if let Some(hud) = &mut self.hud {
                let p = net.protocol();
                let bound = |cmd: &str| bound_key(&self.shell, &self.localized, p, cmd);
                hud.on_net_event(ev, time, &self.localized, &bound);
            }
            match ev {
                // `player_talk` with every chat line, as
                // `CG_ServerCommand`'s `h`/`i` play it.
                net::NetEvent::Chat { text, .. } => {
                    console::log::print(&self.localized.message(text));
                    self.audio.play_local(&self.fs, "player_talk");
                }
                net::NetEvent::Print(s) => console::log::print(s),
                net::NetEvent::Dropped(why) => *fatal = Some(anyhow!("{why}")),
                net::NetEvent::DemoEnded(_) => *demo_over = true,
                // Map ambient; each round restart re-sends it
                // with a new fade deadline, which is ignored.
                net::NetEvent::ConfigstringChanged(3) => {
                    self.audio
                        .set_ambient(&self.fs, net::info_value_for_key(net.configstring(3), "n"));
                }
                // `CG_ConfigStringModified` re-runs
                // `CG_RegisterItems` on CS 8 (cgame 0x3002c70a).
                net::NetEvent::ConfigstringChanged(8) if matches!(phase, Phase::Live(_)) => {
                    prewarm_viewmodels(view, r, &self.fs, net.configstrings());
                }
                // A mirror slot writes every pair into the
                // console again (cgame 0x3002c72a).
                net::NetEvent::ConfigstringChanged(i)
                    if matches!(phase, Phase::Live(_)) && join.cvars.in_mirror(*i) =>
                {
                    let out = join
                        .cvars
                        .apply_mirror(net.configstrings(), &mut self.shell);
                    cvar_effects(&self.shell, net, &self.config_path, out);
                }
                // `j/k/l` is quick chat; `s <idx>` is the announcer.
                net::NetEvent::ServerCommand(tokens) => {
                    // `CG_MapRestart` (cgame 0x3002ca60) frees
                    // the local entities, the marks and every
                    // effect, stops all sounds, restarts the
                    // ambient, stands the player up running,
                    // closes the menus and re-pegs the clock
                    // (events doc 9.8).
                    if play::join::is_map_restart(tokens) {
                        join.on_restart();
                        let out = join.cvars.on_map_restart(&mut self.shell);
                        cvar_effects(&self.shell, net, &self.config_path, out);
                        // `cg.mapRestart`: the next snapshot runs
                        // the respawn reset (0x30028a70) whatever
                        // its spawn count.
                        view.map_restart();
                        input.map_restart();
                        self.fx.clear();
                        r.clear_marks();
                        self.audio.stop_all();
                        self.audio.set_ambient(
                            &self.fs,
                            net::info_value_for_key(net.configstring(3), "n"),
                        );
                        input.force_stance(play::input::Stance::Stand);
                        let closes = play::join::RestartCloses::for_command(
                            tokens,
                            net.protocol().is_v1(),
                            net.protocol().is_uo(),
                        );
                        let out = self.ui.map_restart(closes, &self.shell);
                        self.ui_pending.extend(out);
                        // the last syscall re-pegs the clock,
                        // unless the snapshot is `SNAPFLAG_NOT_ACTIVE`
                        if let Phase::Live(live) = &mut *phase
                            && net
                                .snapshots()
                                .newest()
                                .is_some_and(|s| s.snap_flags & 2 == 0)
                        {
                            live.clock.map_restart();
                        }
                    }
                    // UO's `F <ms>`, `rewindFx` (uo cgame
                    // 0x3003ac90, trap 0xf0).
                    if net.protocol().is_uo()
                        && tokens.first().is_some_and(|t| t == "F")
                        && let Some(ms) = tokens.get(1).and_then(|t| t.parse::<i32>().ok())
                    {
                        self.fx.rewind(ms as f32);
                    }
                    // `v` sets a client cvar in the console's store,
                    // a demo's too. A UO seat's `v cl_stance 0`
                    // (game 0x8998d) moves vcod's stance, which
                    // lives in the input, not a cvar.
                    if let Some(out) = join.cvars.on_server_command(tokens, &mut self.shell) {
                        cvar_effects(&self.shell, net, &self.config_path, out);
                        if tokens[1].eq_ignore_ascii_case("cl_stance") {
                            let value = tokens.get(2).map_or("", String::as_str);
                            input.force_stance(match value.trim() {
                                "1" => play::input::Stance::Crouch,
                                "2" => play::input::Stance::Prone,
                                _ => play::input::Stance::Stand,
                            });
                        }
                    }
                    // `a <weapon>` selects (cgame 0x3002e139).
                    if tokens.first().is_some_and(|t| t == "a") {
                        input.server_select(tokens.get(1).map_or("", String::as_str));
                    }
                    // `g`, the bold game message, beeps.
                    if tokens.first().is_some_and(|t| t == "g") {
                        self.audio.play_local(&self.fs, "game_message");
                    }
                    if let Some(cmd) = screenshot::auto_command(
                        tokens,
                        net.protocol().is_v1(),
                        self.shell.cvar("cg_autoscreenshot").unwrap_or("0"),
                        self.shell.cvar("name").unwrap_or(""),
                        &play::demos::stamp_now(),
                    ) {
                        // `screenshotJPEG <name>`, one word
                        let args: Vec<String> =
                            cmd.split(' ').skip(1).map(str::to_string).collect();
                        self.shot = Some(screenshot::parse(true, &args));
                    }
                    if let Some(cmd) = play::demos::auto_command(
                        tokens,
                        net.protocol(),
                        self.shell.cvar("cg_autodemo").unwrap_or("0"),
                        self.shell.cvar("name").unwrap_or(""),
                        &play::demos::stamp_now(),
                    ) {
                        match cmd.strip_prefix("record ") {
                            Some(name) => start_record(net, &demo_dirs[0], Some(name)),
                            None => {
                                if net.stop_recording().unwrap_or(false) {
                                    console::log::print("Stopped demo.");
                                }
                            }
                        }
                    }
                    use play::join::MenuCommand;
                    // A demo answers no menu.
                    let menu = if net.is_demo() {
                        None
                    } else {
                        join.on_server_command(tokens, net.configstrings(), net.server_id())
                    };
                    match menu {
                        Some(MenuCommand::Send(cmd)) => net.send_reliable(&cmd),
                        Some(MenuCommand::Popup {
                            name,
                            index,
                            no_mouse,
                        }) => {
                            let mut out = Vec::new();
                            self.ui
                                .set_server_cvars(join.cvars.all(net.configstrings(), &self.shell));
                            if !self
                                .ui
                                .open_script_popup(&name, no_mouse, &self.shell, &mut out)
                                && let Some(noop) = join.wait(play::join::Waiting {
                                    name,
                                    index,
                                    no_mouse,
                                })
                            {
                                net.send_reliable(&noop);
                            }
                            self.ui_pending.extend(out);
                        }
                        Some(MenuCommand::Close) => {
                            let out = self.ui.close_script_popup(&self.shell);
                            self.ui_pending.extend(out);
                        }
                        None => {}
                    }
                    let newest = net.snapshots().newest();
                    let protocol = net.protocol();
                    let quick_chat = self.quick_chat.on_server_command(&self.fs, tokens, |num| {
                        newest
                            .and_then(|s| s.clients.get(&num))
                            .map(|c| c.field_i32(protocol, "team"))
                    });
                    if !quick_chat {
                        self.audio
                            .on_server_command(&self.fs, tokens, net.configstrings());
                    }
                }
                net::NetEvent::GamestateReady => {
                    join.set_layout(net.protocol().cs);
                    join.on_gamestate();
                    // The UI catcher goes down with it, so an
                    // in-game menu closes (CoDMP.exe 0x40ffb0).
                    self.ui.close_all();
                    gamestate_ready = true;
                    predictor.fall_heights =
                        pmove::FallHeights::from_systeminfo(net.configstring(1));
                    system_info_changed(&mut self.shell, net, &self.config_path);
                }
                net::NetEvent::ConfigstringChanged(1) => {
                    predictor.fall_heights =
                        pmove::FallHeights::from_systeminfo(net.configstring(1));
                    system_info_changed(&mut self.shell, net, &self.config_path);
                }
                _ => {}
            }
        }

        // The cgame offers a refused `t` again each frame
        // (cgame_mp_x86.dll 0x3002dd10).
        if let Some(w) = join.waiting() {
            let mut out = Vec::new();
            self.ui
                .set_server_cvars(join.cvars.all(net.configstrings(), &self.shell));
            if self
                .ui
                .open_script_popup(&w.name, w.no_mouse, &self.shell, &mut out)
            {
                join.opened_waiting();
            }
            self.ui_pending.extend(out);
        }
        if self.ui.active() {
            self.ui
                .set_server_cvars(join.cvars.all(net.configstrings(), &self.shell));
        }

        // One quick-chat line per second, played and shown
        // like a chat line (retail queues text + alias).
        if let Some(line) = self.quick_chat.drain(time) {
            let name = net
                .snapshots()
                .newest()
                .and_then(|s| s.clients.get(&line.client_num))
                .map(|c| c.name(net.protocol()))
                .unwrap_or_else(|| format!("player {}", line.client_num));
            console::log::print(&format!("{name}: {}", line.text));
            if let Some(hud) = &mut self.hud {
                let now_ms = (time * 1000.0) as i32;
                hud.chat.push(&format!("{name}: {}", line.text), now_ms);
            }
            self.audio.play(&self.fs, line.cue);
        }

        // Re-send `score` every 2 s while Tab is held, as the
        // stock client does (cod11-hud-protocol.md, section 4).
        if let Some(hud) = &mut self.hud
            && hud.scoreboard.due(time)
        {
            net.send_reliable("score");
            hud.scoreboard.mark_requested(time);
        }

        if gamestate_ready {
            cmd_clock.reset();
            ring.clear();
            predictor.reset();
            // `CL_SystemInfoChanged` forced the `fs_game` cvar
            // (`system_info_changed` above) and sets the pure
            // list, and the gamestate parse restarts the
            // search path when either changed
            // (docs/research/cod11-front-end.md, section 16).
            // A server with no `fs_game` sends no key, which
            // leaves the cvar as it was. A demo's systeminfo
            // sets neither (CoDMP.exe 0x415eb0).
            let (game, pure) = if net.is_demo() {
                (self.fs_game.clone(), self.fs_pure.clone())
            } else {
                (
                    net::download::mod_dir(self.shell.cvar("fs_game").unwrap_or(""), &self.mod_dir),
                    net::download::pure_paks(net.configstring(1)),
                )
            };
            if game != self.fs_game || pure != self.fs_pure {
                log::info!(
                    "fs_game {}{}: restarting the search path",
                    game.as_deref().unwrap_or("(none)"),
                    if pure.is_some() { ", pure server" } else { "" },
                );
                self.fs_game = game;
                self.fs_pure = pure;
                let (base, game) = search_dirs(&self.game_dir, &self.mod_dir, &self.fs_game);
                match reopen_fs(
                    &base,
                    game.as_deref(),
                    self.fs_pure.as_deref(),
                    &mut self.fs,
                    &mut self.localized,
                    &mut self.hud,
                    &mut self.audio,
                    &mut self.fx,
                    &mut self.quick_chat,
                ) {
                    Ok(()) => {
                        r.reopen(&self.fs);
                        view.reopen();
                        self.shell.execute("set ui_menuFiles ui_mp/menus.txt");
                        self.ui = new_ui(
                            &self.fs,
                            &self.game_dir,
                            &self.mod_dir,
                            self.shell.cvar("cl_language").unwrap_or(""),
                        );
                    }
                    Err(e) => *fatal = Some(e),
                }
            }
        }
        // Only with the map up: the first cmd after a gamestate
        // is what enters the client into the world
        // (docs/protocol-1.1.md, "Entering the world").
        // A demo's clock never slews toward its snapshots
        // (`CL_AdjustTimeDelta`, CoDMP.exe 0x404bf0), and it
        // sends no cmds.
        if let Some(soak) = &mut self.soak
            && !net.is_demo()
        {
            let ps = match &*phase {
                Phase::Live(_) => net.snapshots().newest().map(|s| &s.ps),
                _ => None,
            };
            soak.act(
                input,
                ps,
                net.protocol(),
                net.configstring(0),
                cam.pos,
                time,
                self.ui.active(),
            );
        }
        if net.is_demo()
            && !gamestate_ready
            && let (Phase::Live(live), Some(s)) = (&mut *phase, net.snapshots().newest())
        {
            let nudge = self.shell.cvar_f32("cl_timeNudge") as i32;
            live.clock
                .update_demo(local_ms as i32, s.server_time.raw(), nudge, demo_clock);
        }
        if matches!(phase, Phase::Live(_))
            && !gamestate_ready
            && !net.is_demo()
            && net.state() == net::NetState::Active
        {
            let realtime = local_ms as i32;
            // Before the first snapshot there is no clock yet;
            // the entering cmd goes out on the gamestate's.
            let server_now = match (&mut *phase, net.snapshots().newest()) {
                (Phase::Live(live), Some(s)) => {
                    let nudge = self.shell.cvar_f32("cl_timeNudge") as i32;
                    live.clock
                        .update(realtime, s.server_time.raw(), s.message_num, nudge);
                    live.clock.cmd_time(s.server_time.raw())
                }
                _ => net.server_clock_ms(),
            };
            let times = cmd_clock.due(server_now);
            if !times.is_empty() {
                let weapons = match &*phase {
                    Phase::Live(live) => live.weapons.as_slice(),
                    _ => &[],
                };
                let held = net
                    .snapshots()
                    .newest()
                    .map_or_else(play::input::Held::default, |s| {
                        play::input::Held::from_ps(&s.ps, net.protocol(), weapons)
                    });
                input.sync(&held);
                // A select or a respawn puts `cl_run` back to
                // 1, the cgame's write of the cvar (0x30037f20,
                // 0x30028a70).
                if input.take_run_reset() {
                    self.shell.execute("set cl_run 1");
                }
                input.cl_run = play::input::ClRun(self.shell.cvar_f32("cl_run") as i32);
                input.talking = play::input::talking(
                    self.console.open,
                    self.ui.active(),
                    self.hud.as_ref().is_some_and(|h| h.chat_field.is_some()),
                    self.shell.cvar_f32("cl_bypassMouseInput") as i32,
                );
                for &t in &times {
                    ring.push(input.build(t, &held));
                }
            }
            let lan = net.transport().is_lan();
            let max_packets = self.shell.cvar_f32("cl_maxpackets") as i32;
            if ring.packet_due(realtime, lan, max_packets) {
                let dup = self.shell.cvar_f32("cl_packetdup") as i32;
                net.send_cmds(&ring.packet(realtime, dup));
            }
        }

        let progress = net.download_progress();
        let frame = match phase {
            Phase::Connecting => {
                // The net client's own resend and gamestate
                // timers decide when a slow server is given up
                // on; a mod's connect notice can hold the
                // `connectResponse` back for seconds.
                if gamestate_ready {
                    vcod_common::laps::mark(|| "gamestate".into());
                    match start_loading(
                        net,
                        &mut self.hud,
                        &mut self.audio,
                        &mut self.fx,
                        &mut self.world,
                        r,
                        self.window.as_deref(),
                        &mut self.title,
                        &self.game_dir,
                        &download_dirs(&self.mod_dir, &self.fs_game),
                        allow_download(&self.shell),
                        self.fs.vanilla(),
                    ) {
                        Ok(next) => *phase = next,
                        Err(e) => *fatal = Some(e),
                    }
                }
                loading_frame(
                    r,
                    &self.fs,
                    &self.localized,
                    &mut self.hud,
                    time,
                    aspect,
                    cull,
                    net.configstrings(),
                    format!(
                        "Connecting to {}...",
                        self.connect_addr.as_deref().unwrap_or("?")
                    ),
                    hud_quads,
                )
            }
            Phase::Loading { loader } => {
                let map = loader.map().to_string();
                let map_resolves = self.fs.resolve_map(&map).is_some();
                let mut waited = None;
                // A Ready or Failed loader is never stepped again:
                // Ready swaps the phase, Failed exits.
                match loader.step(&events, progress, map_resolves, now) {
                    loading::Action::BeginDownload { remote, dest } => {
                        if let Err(e) = net.begin_download(&remote, &dest) {
                            *fatal = Some(e.context("cannot start the download"));
                        }
                    }
                    loading::Action::Reopen => {
                        let (base, game) =
                            search_dirs(&self.game_dir, &self.mod_dir, &self.fs_game);
                        match reopen_fs(
                            &base,
                            game.as_deref(),
                            self.fs_pure.as_deref(),
                            &mut self.fs,
                            &mut self.localized,
                            &mut self.hud,
                            &mut self.audio,
                            &mut self.fx,
                            &mut self.quick_chat,
                        ) {
                            Ok(()) => {
                                r.reopen(&self.fs);
                                view.reopen();
                            }
                            Err(e) => *fatal = Some(e),
                        }
                    }
                    loading::Action::FinishDownloads => net.finish_downloads(),
                    loading::Action::Ready => {
                        r.set_image_gamma(image_gamma(&mut self.shell, self.ignore_hw_gamma));
                        let t_load = Instant::now();
                        match load_map(
                            &map,
                            net,
                            &mut self.fs,
                            &mut self.audio,
                            &mut self.world,
                            r,
                            self.window.as_deref(),
                            &mut self.title,
                        ) {
                            Ok(next) => {
                                *phase = next;
                                vcod_common::laps::span(t_load, || format!("load_map {map}"));
                                // `CL_DownloadsComplete` sends it
                                // after the cgame loads; a pure
                                // server drops a client without it.
                                let feed = net.gamestate().map_or(0, |g| g.checksum_feed);
                                net.send_reliable(
                                    &self.fs.pure_command(feed, net.protocol().pure_dlls),
                                );
                                view.set_protocol(net.protocol());
                                prewarm_viewmodels(view, r, &self.fs, net.configstrings());
                                if let Phase::Live(live) = &mut *phase {
                                    prewarm_level(&mut self.fx, r, &self.fs, net, live);
                                }
                                // `CL_DownloadsComplete` frees the
                                // old cgame and loads a fresh one.
                                fresh_cgame(
                                    view,
                                    input,
                                    predictor,
                                    self.hud.as_mut(),
                                    &mut self.fx,
                                );
                                if let Err(e) = cgame_init(
                                    view,
                                    join,
                                    &mut self.shell,
                                    &self.fs,
                                    net,
                                    &self.config_path,
                                ) {
                                    *fatal = Some(anyhow!(e));
                                }
                                let t_menus = Instant::now();
                                if let Err(e) = load_script_menus(
                                    &mut self.ui,
                                    &self.fs,
                                    &self.shell,
                                    net.configstrings(),
                                    &net.protocol().cs,
                                ) {
                                    *fatal = Some(anyhow!(e));
                                }
                                vcod_common::laps::span(t_menus, || "script menus".into());
                            }
                            Err(e) => *fatal = Some(e),
                        }
                    }
                    loading::Action::Failed(msg) => *fatal = Some(anyhow!(msg)),
                    loading::Action::Wait(p) => waited = p,
                }
                loading_frame(
                    r,
                    &self.fs,
                    &self.localized,
                    &mut self.hud,
                    time,
                    aspect,
                    cull,
                    net.configstrings(),
                    loading_text(&map, waited),
                    hud_quads,
                )
            }
            Phase::Live(live) => {
                if gamestate_ready {
                    vcod_common::laps::begin_load("cod1 map change");
                    match start_loading(
                        net,
                        &mut self.hud,
                        &mut self.audio,
                        &mut self.fx,
                        &mut self.world,
                        r,
                        self.window.as_deref(),
                        &mut self.title,
                        &self.game_dir,
                        &download_dirs(&self.mod_dir, &self.fs_game),
                        allow_download(&self.shell),
                        self.fs.vanilla(),
                    ) {
                        Ok(next) => *phase = next,
                        Err(e) => *fatal = Some(e),
                    }
                    let map = match phase {
                        Phase::Loading { loader } => loader.map().to_string(),
                        _ => String::new(),
                    };
                    loading_frame(
                        r,
                        &self.fs,
                        &self.localized,
                        &mut self.hud,
                        time,
                        aspect,
                        cull,
                        net.configstrings(),
                        format!("Loading {map}"),
                        hud_quads,
                    )
                } else {
                    let LivePhase {
                        world,
                        weapons,
                        scene,
                        events,
                        predicted_events,
                        clock,
                        last_loop_snap,
                        looped_fx,
                        vehicle_trails,
                        drawn_pos,
                        drawn_vehicle_angles,
                        seat_turn,
                        gunner_sway,
                        turret_sway,
                        seat_frac,
                    } = &mut **live;
                    let p = net.protocol();
                    let client_num = net.gamestate().map_or(-1, |g| g.client_num);
                    // While following, ps.clientNum is the followed
                    // client's (docs/research/cod11-events-and-fx.md,
                    // section 7); the camera sits inside that body, so
                    // skip it instead of ours.
                    let ps_client = net
                        .snapshots()
                        .newest()
                        .map_or(-1, |s| s.ps.field_i32(p, "clientNum"));
                    // A demo draws its recorder as a followed
                    // player, off the snapshots
                    // (`CG_InterpolatePlayerState` under
                    // `cg.demoPlayback`).
                    let demo = net.is_demo();
                    let following = ps_client >= 0 && (ps_client != client_num || demo);
                    // Intermission (5) and dead (6, 7) take no view
                    // from the cmd (Q3 `PM_UpdateViewAngles`), so
                    // those draw the snapshot's view, as following does.
                    let pm_type = net
                        .snapshots()
                        .newest()
                        .map_or(0, |s| s.ps.field_i32(p, "pm_type"));
                    let snapshot_view = following || (5..=7).contains(&pm_type);
                    let skip_num = if following { ps_client } else { client_num };
                    // Rebuilt every frame so absent entities (PVS churn) drop out.
                    let mut instances: Vec<DynamicModelInstance> = Vec::new();
                    // Per-shooter muzzle transforms for flashes and tracers.
                    let mut muzzles: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
                    // Per-shooter `tag_brass`, for spent cases.
                    let mut brass: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
                    // Bodies with no muzzle tag, where their tracers start.
                    let mut shot_fallbacks: HashMap<u32, Vec3> = HashMap::new();
                    let mut weapon_flash: HashMap<i32, String> = HashMap::new();
                    let mut entity_pos: HashMap<u32, Vec3> = HashMap::new();
                    let mut entity_angles: HashMap<u32, Vec3> = HashMap::new();
                    let mut vehicle_angles: HashMap<u32, [f32; 3]> = HashMap::new();
                    let mut vehicle_wheels = Vec::new();
                    let mut heads: HashMap<u32, Vec3> = HashMap::new();
                    let mut turret_eye = None;
                    let mut shadow_marks = Vec::new();
                    // The drawn `leanf`; the intermission view
                    // takes none (cgame 0x30033413).
                    let mut view_lean = 0.0;
                    // The drawn origin's height, under which
                    // the eye never goes (cgame 0x30032de5).
                    let mut feet_z = None;

                    let render_time = net
                        .snapshots()
                        .newest()
                        .and(clock.delta())
                        .map(|_| clock.server_time());
                    if let Some(render_time) = render_time
                        && let Some((a, b)) = net.snapshots().two_for_time(render_time)
                    {
                        // No straddling pair: one frame, held.
                        if std::ptr::eq(a, b) {
                            self.interp_misses += 1;
                        }
                        let oa = Vec3::from(a.ps.origin(p));
                        let ob = Vec3::from(b.ps.origin(p));
                        let f = ((Msec::from_raw(render_time) - a.server_time) as f32
                            / (b.server_time - a.server_time).max(1) as f32)
                            .clamp(0.0, 1.0);
                        let t0 = Instant::now();
                        prof.next(frame_prof::Stage::Pose);
                        // Last frame's drawn bodies, for the gunner's
                        // trace down.
                        let bodies = play::predict::solid_bodies(
                            p,
                            &b.entities,
                            client_num as u32,
                            drawn_pos,
                        );
                        scene.swing_speed = self.shell.cvar_f32("bg_swingSpeed");
                        // The view a ridden UO gun turns with: ours
                        // off the cmd, a followed one's off the
                        // snapshots.
                        scene.view = Some(if snapshot_view {
                            let (va, vb) = (a.ps.viewangles(p), b.ps.viewangles(p));
                            std::array::from_fn(|i| camera::lerp_angle(va[i], vb[i], f))
                        } else {
                            let delta = net::DELTA_ANGLE_FIELDS.map(|name| b.ps.field_i32(p, name));
                            let (y, pt, r) = own_view(input.cmd_angles(), delta);
                            [-pt.to_degrees(), y.to_degrees(), r.to_degrees()]
                        });
                        // The ridden body draws in a UO seat the view
                        // leaves (cgame 0x3003392b), and is posed
                        // undrawn otherwise. It is not in the snapshot,
                        // so it is built from the playerstate, as
                        // `cg.predictedPlayerEntity` is.
                        let seat_body = b.ps.field_i32(p, "pm_type") != 5
                            && play::seat::of_snapshot(p, b).is_some_and(|(seat, ..)| {
                                play::seat::own_body_drawn(seat, *seat_frac)
                            });
                        let own_pair = play::seat::adds_view_body(
                            b.ps.field_i32(p, "pm_flags"),
                            seat_body,
                            following,
                        )
                        .then(|| {
                            (
                                play::seat::own_body(p, &a.ps),
                                play::seat::own_body(p, &b.ps),
                            )
                        });
                        let view = |s, own| entities::SnapView { snap: s, own };
                        let built = entities::build_instances(
                            scene,
                            (
                                view(a, own_pair.as_ref().map(|o| &o.0)),
                                view(b, own_pair.as_ref().map(|o| &o.1)),
                                f,
                            ),
                            render_time,
                            net.snapshots()
                                .newest()
                                .map_or(b.server_time, |s| s.server_time)
                                .raw(),
                            if seat_body { -1 } else { skip_num },
                            net.configstrings(),
                            &self.fs,
                            Some(MoveWorld::new(world, &bodies, u32::MAX)),
                            r,
                            p,
                        );
                        self.build_ms = t0.elapsed().as_secs_f32() * 1000.0;
                        prof.next(frame_prof::Stage::Net);
                        instances = built.instances;
                        muzzles = built.muzzles;
                        shot_fallbacks = built.shot_fallbacks;
                        brass = built.brass;
                        weapon_flash = built.weapon_flash;
                        entity_pos = built.entity_pos;
                        entity_angles = built.entity_angles;
                        vehicle_angles = built.vehicle_angles;
                        vehicle_wheels = built.vehicle_wheels;
                        heads = built.heads;
                        turret_eye = built.turret_eye;
                        frame_digest::submodels(&built.submodels);
                        r.set_submodels(&built.submodels);
                        r.set_entity_lights(built.scene_lights);
                        // `cg_shadows` 2 is the renderer's
                        // stencil path; only 1 places blobs.
                        if self.shell.cvar_f32("cg_shadows") as i32 == 1 {
                            shadow_marks = built
                                .shadow_casters
                                .iter()
                                .filter_map(|&o| blob_shadow::player_shadow(&*world, oa, o))
                                .collect();
                        }
                        // Over 512 u is a teleport, not motion.
                        let pos = if oa.distance(ob) > 512.0 {
                            ob
                        } else {
                            oa.lerp(ob, f)
                        };
                        // viewHeightCurrent: eye offset above the feet origin.
                        let vh = a.ps.field_f32(p, "viewHeightCurrent") * (1.0 - f)
                            + b.ps.field_f32(p, "viewHeightCurrent") * f;
                        cam.pos = pos + Vec3::Z * play::view::eye_height(pm_type, vh);
                        feet_z = Some(pos.z);
                        // `CG_InterpolatePlayerState` lerps `leanf`
                        // too (cgame 0x3002936a).
                        if pm_type != 5 {
                            view_lean = a.ps.field_f32(p, "leanf") * (1.0 - f)
                                + b.ps.field_f32(p, "leanf") * f;
                        }
                        if snapshot_view {
                            let va_a = a.ps.viewangles(p);
                            let va_b = b.ps.viewangles(p);
                            cam.yaw = camera::lerp_angle(va_a[1], va_b[1], f).to_radians();
                            cam.pitch = -camera::lerp_angle(va_a[0], va_b[0], f).to_radians();
                        }
                    }
                    // A UO vehicle seat (docs/research/coduo-vehicles.md).
                    let seat_now = net
                        .snapshots()
                        .newest()
                        .and_then(|s| play::seat::of_snapshot(p, s));
                    predictor.seat_angles = seat_now.map_or([0.0; 3], |(_, num, vang)| {
                        drawn_vehicle_angles.get(&num).copied().unwrap_or(vang)
                    });
                    // The cgame's `bg_*` cvars, off the 140/204 mirror.
                    // UO's cgame never registers `bg_nofatigue`.
                    predictor.bg = pmove::BgCvars {
                        nofatigue: false,
                        ..pmove::BgCvars::from_cvars(|c| {
                            join.cvars.get(c, net.configstrings(), &self.shell)
                        })
                    };
                    if let Some(t) = render_time {
                        predictor.set_time(t);
                    }
                    let predicted = if ps_client == client_num && !demo {
                        net.snapshots().newest().and_then(|s| {
                            let bodies = play::predict::solid_bodies(
                                p,
                                &s.entities,
                                client_num as u32,
                                drawn_pos,
                            );
                            let movers = (
                                pmove::movers::SnapshotMovers::from_entities(p, &s.entities),
                                s.server_time.raw(),
                            );
                            predictor
                                .predict(p, &s.ps, ring, world, &bodies, &movers, weapons, local_ms)
                        })
                    } else {
                        predictor.reset();
                        None
                    };
                    drawn_pos.clone_from(&entity_pos);
                    drawn_vehicle_angles.clone_from(&vehicle_angles);
                    if let Some(v) = &predicted {
                        cam.pos =
                            v.origin + Vec3::Z * play::view::eye_height(pm_type, v.view_height);
                        feet_z = Some(v.origin.z);
                        view_lean = v.pred.ps.lean;
                    }
                    if let Some(s) = net.snapshots().newest() {
                        let spawn = (s.ps.field_i32(p, "clientNum"), s.ps.arrays.stats[5]);
                        view.track_spawn(spawn);
                        if let Some(hud) = &mut self.hud {
                            hud.track_spawn(spawn);
                        }
                        let mut held = play::input::Held::from_ps(&s.ps, p, weapons);
                        // `cg.snap`, the older of the drawn pair.
                        if let Some((snap, _)) =
                            render_time.and_then(|t| net.snapshots().two_for_time(t))
                        {
                            held.cg_snap(&s.ps, &snap.ps, p);
                        }
                        input.sync(&held);
                    }
                    // A followed player's view weapon, zoom and scope
                    // come off the snapshot, as retail draws them.
                    let view_ps = net.snapshots().newest().and_then(|s| {
                        pmove::predict::on_foot(pm_type).then(|| {
                            let mut v = match &predicted {
                                Some(v) => play::view::ViewPs::from_predicted(
                                    &v.pred,
                                    s.ps.field_i32(p, "viewmodelIndex"),
                                ),
                                None => play::view::ViewPs::from_snapshot(p, &s.ps),
                            };
                            v.seat = seat_now.map(|s| s.0);
                            v
                        })
                    });
                    if let Some(ps) = &view_ps
                        && let Some(models) = view.sync_rig(&self.fs, net.configstrings(), ps)
                    {
                        r.set_viewmodel(&self.fs, &models);
                        self.viewmodel = models;
                    }
                    // `CG_TransitionPlayerState` feeds a hit ahead of
                    // the snapshot's events and the view, mounted
                    // or not (hud doc "Damage direction"): off the
                    // prediction's newest playerstate, or a followed
                    // player's at `cg.snap`'s transition, stamped
                    // with `cg.snap`'s time either way.
                    if let (Some(hud), Some(newest)) = (&mut self.hud, net.snapshots().newest()) {
                        let own = pmove::predict::on_foot(pm_type) && predicted.is_some();
                        let cg_snap = render_time
                            .and_then(|t| net.snapshots().two_for_time(t))
                            .map_or(newest, |(a, _)| a);
                        let fed = if own { newest } else { cg_snap };
                        hud.feed_damage(
                            p,
                            &fed.ps,
                            render_time.unwrap_or(0),
                            cg_snap.server_time.raw(),
                        );
                    }
                    // Retail runs a snapshot's events in
                    // `CG_ProcessSnapshots`, ahead of the view
                    // (cgame 0x30033c14, 0x30033d9b): a quake or a
                    // landing moves the frame it arrives on.
                    prof.next(frame_prof::Stage::Events);
                    let mut evs = Vec::new();
                    if let Some(newest) = net.snapshots().newest() {
                        // Our own ring plays off the prediction, and
                        // the snapshot's copy of it is skipped.
                        let own = ps_client == client_num && !demo;
                        evs = play::events::frame_events(
                            predicted_events,
                            events,
                            newest,
                            p,
                            own,
                            own && pmove::predict::predictable(pm_type),
                            predicted.as_ref().map(|v| &v.pred),
                            ps_client,
                        );
                        if let Some(ps) = &view_ps {
                            for _ in 0..play::events::view_kicks(&evs, ps_client) {
                                view.fire(weapons, ps);
                            }
                        }
                        let fall = pmove::FallHeights::from_systeminfo(net.configstring(1));
                        for ev in &evs {
                            let Some(now) = render_time else {
                                break;
                            };
                            // At the event body's drawn origin; ours
                            // is never in `entity_pos`.
                            let body = entity_pos
                                .get(&ev.entity_num)
                                .copied()
                                .unwrap_or(Vec3::from(ev.pos));
                            if let Some(q) = play::quake::from_event(
                                p,
                                ev,
                                newest.entities.get(&ev.entity_num),
                                body,
                            ) {
                                view.start_quake(q, now);
                            }
                            if ev.client_num == ps_client
                                && let Some(c) = play::view::land_change(ev.event, ev.parm, fall)
                            {
                                view.land(c, now);
                            }
                        }
                    }
                    prof.next(frame_prof::Stage::Vm);
                    view.step_recoil(weapons, view_ps.as_ref(), local_ms);
                    // Next frame's cmds carry this kick, as
                    // `CL_FinishMove` reads last frame's syscall 0x56.
                    input.kick = view.view_kick();
                    // `CG_CalcViewValues` draws the predicted
                    // `viewangles[2]`, the kick's roll included.
                    let mut view_roll = 0.0;
                    if !snapshot_view {
                        let delta = match &predicted {
                            Some(v) => v.delta_angles,
                            None => net.snapshots().newest().map_or([0; 3], |s| {
                                net::DELTA_ANGLE_FIELDS.map(|name| s.ps.field_i32(p, name))
                            }),
                        };
                        (cam.yaw, cam.pitch, view_roll) = own_view(input.cmd_angles(), delta);
                    }
                    // `CG_OffsetFirstPersonView` (cgame 0x30032b58)
                    // first adds the damage kick and the sight's
                    // bob; the intermission arm skips it.
                    let mut damage_kick = [0.0; 3];
                    if pm_type != 5
                        && let (Some(hud), Some(s)) = (&mut self.hud, net.snapshots().newest())
                    {
                        let own = pmove::predict::on_foot(pm_type);
                        let [kp, ky, kr] = hud.view_kick(
                            p,
                            &s.ps,
                            predicted.as_ref().filter(|_| own).map(|v| &v.pred),
                            render_time.unwrap_or(0),
                            weapons,
                        );
                        damage_kick = [kp, ky, kr];
                        cam.pitch -= kp.to_radians();
                        cam.yaw += ky.to_radians();
                        view_roll += kr.to_radians();
                        // Then the walk bob, whose amplitudes a 1.5
                        // server sets through the 140/204 mirror
                        // (cod15-movement.md 6).
                        hud.bob = if pmove::MoveRules::for_protocol(p).v15() {
                            pmove::BobCvars::from_cvars(|c| {
                                join.cvars.get(c, net.configstrings(), &self.shell)
                            })
                        } else {
                            pmove::BobCvars::default()
                        };
                        let kicked = [
                            -cam.pitch.to_degrees(),
                            cam.yaw.to_degrees(),
                            view_roll.to_degrees(),
                        ];
                        cam.pos += hud.eye_bob(
                            p,
                            &s.ps,
                            predicted.as_ref().filter(|_| own).map(|v| &v.pred),
                            render_time.unwrap_or(0),
                            kicked,
                        );
                    }
                    // Then the landing's dip (0x30032ca1), and it
                    // ends on `AddLeanToPosition` (cgame
                    // 0x30032dda): a lean moves the eye and rolls
                    // nothing.
                    if pm_type != 5
                        && let Some(now) = render_time
                    {
                        cam.pos.z += view.land_dip(now);
                    }
                    cam.pos = pmove::aim::add_lean(
                        cam.pos,
                        cam.yaw,
                        view_lean,
                        vcod_common::patch::Arith::Msvc,
                    );
                    if pm_type != 5
                        && let Some(z) = feet_z
                    {
                        cam.pos.z = cam.pos.z.max(z + 8.0);
                    }
                    // In a vehicle seat `CG_VehicleView` (uo cgame
                    // 0x30040810) replaces the first-person offsets.
                    if let Some((seat, num, vang)) = seat_now {
                        let delta = match &predicted {
                            Some(v) => v.delta_angles,
                            None => net.snapshots().newest().map_or([0; 3], |s| {
                                net::DELTA_ANGLE_FIELDS.map(|name| s.ps.field_i32(p, name))
                            }),
                        };
                        let newest_ps = &net.snapshots().newest().expect("seated").ps;
                        let frac = predicted.as_ref().map_or_else(
                            || newest_ps.field_f32(p, "fWeaponPosFrac"),
                            |v| v.pred.ps.weapon_pos_frac,
                        );
                        *seat_frac = frac;
                        // A followed rider's view is the snapshot's.
                        let wire_view = |input: &play::input::PlayInput| {
                            let (y, pt) = if snapshot_view {
                                (cam.yaw, cam.pitch)
                            } else {
                                let (y, pt, _) =
                                    own_view_at(input.cmd_angles(), delta, SEAT_PITCH_CLAMP);
                                (y, pt)
                            };
                            [-pt.to_degrees(), y.to_degrees(), 0.0]
                        };
                        let mut tag = |name: &str| scene.tag_pose(&self.fs, num, name);
                        // 0x30040580: a four-wheel turns its riders by
                        // the vehicle's `lerpAngles` (0x3004060f).
                        let drawn_angles = vehicle_angles.get(&num).copied().unwrap_or(vang);
                        if !snapshot_view
                            && seat.class == play::seat::CLASS_FOUR_WHEEL
                            && seat.pos != 2
                            && !seat.own_weapon()
                            && newest_ps.field_i32(p, "weapon") != 0
                        {
                            if let Some(v) = seat_turn.step(
                                wire_view(input),
                                drawn_angles,
                                frac != 0.0,
                                newest_ps.field_i32(p, "eFlags") & net::flags::EF_HULL_VIEW_V22
                                    != 0,
                            ) {
                                input.set_view(v, delta);
                            }
                        } else {
                            seat_turn.reset();
                        }
                        let origin = entity_pos.get(&num).copied().unwrap_or(cam.pos);
                        // The gunner's sway off a `thompson_MP` copy of
                        // the predicted playerstate, and its shake off
                        // `cg.snap`'s (coduo-vehicles.md 5).
                        let mut gunner = play::seat::GunnerEye::default();
                        if seat.pos == 2
                            && let Some(now) = render_time
                        {
                            gunner.time_ms = now;
                            let thompson = net
                                .configstring(7)
                                .split(' ')
                                .position(|w| w.eq_ignore_ascii_case("thompson_mp"))
                                .and_then(|i| weapons.get(i + 1)?.as_ref());
                            if let (Some(v), Some(def)) = (&predicted, thompson) {
                                gunner.sway =
                                    gunner_sway.step(&v.pred.ps, def, wire_view(input), now);
                            }
                            let snaps = net.snapshots();
                            if let Some(s) =
                                snaps.two_for_time(now).map(|(a, _)| a).or(snaps.newest())
                            {
                                let movetype = s
                                    .entities
                                    .get(&num)
                                    .map_or(0, |e| e.field_i32(p, "animMovetype"));
                                if play::seat::gunner_shakes(s.ps.field_i32(p, "eFlags"), movetype)
                                {
                                    gunner.shake = Some(play::seat::shake_phase(now));
                                }
                            }
                        }
                        if let Some(sc) = play::seat::camera(
                            seat,
                            wire_view(input),
                            frac,
                            origin,
                            gunner,
                            &mut tag,
                        ) {
                            if !snapshot_view {
                                if let Some(v) = sc.snap {
                                    input.set_view(v, delta);
                                }
                                (cam.yaw, cam.pitch, _) =
                                    own_view_at(input.cmd_angles(), delta, SEAT_PITCH_CLAMP);
                            }
                            view_roll = 0.0;
                            if let Some([pitch, roll]) = sc.gun_pitch_roll {
                                cam.pitch = -pitch.to_radians();
                                view_roll = roll.to_radians();
                            }
                            cam.yaw += sc.yaw_shake.to_radians();
                            let half = Vec3::splat(8.0);
                            cam.pos = play::seat::clip(&sc, &mut |a, b, mask| {
                                let t = world.move_trace(a, b, -half, half, mask);
                                (t.fraction, t.endpos, t.startsolid)
                            });
                        }
                    } else {
                        seat_turn.reset();
                    }
                    // On a mounted gun the view rides the gun's
                    // `tag_player` and barrel, not the cmd's angles
                    // (docs/research/cod11-turrets.md section 14).
                    // 1.5's and UO's (1.5 cgame 0x300348c0, uo
                    // 0x30041550) hold the view inside the gun's
                    // arcs; UO's sways the eye.
                    if let Some(eye) = turret_eye.map(|e| e.kicked(damage_kick)) {
                        cam.pos = eye.pos;
                        (cam.yaw, cam.pitch) = eye.view();
                        view_roll = eye.angles[2].to_radians();
                        if !snapshot_view && let Some([pt, y]) = eye.snap {
                            let delta = match &predicted {
                                Some(v) => v.delta_angles,
                                None => net.snapshots().newest().map_or([0; 3], |s| {
                                    net::DELTA_ANGLE_FIELDS.map(|name| s.ps.field_i32(p, name))
                                }),
                            };
                            // The cmd adds the kick back on.
                            input.set_view([pt - input.kick[0], y - input.kick[1]], delta);
                        }
                        if let Some(axis) = eye.axis {
                            let thompson = net
                                .configstring(7)
                                .split(' ')
                                .position(|w| w.eq_ignore_ascii_case("thompson_mp"))
                                .and_then(|i| weapons.get(i + 1)?.as_ref());
                            if let (Some(v), Some(def), Some(now)) =
                                (&predicted, thompson, render_time)
                            {
                                let o = turret_sway.step(
                                    &v.pred.ps,
                                    def,
                                    scene.view.unwrap_or(eye.angles),
                                    now,
                                );
                                cam.pos += turret::sway_offset(axis, o);
                            }
                        }
                    }
                    // The quake shakes whatever view that left
                    // (cgame 0x30033da9, after `CG_CalcViewValues`).
                    if let Some(now) = render_time {
                        let [sp, sy, sr] = view.shake(now, cam.pos);
                        cam.pitch -= sp.to_radians();
                        cam.yaw += sy.to_radians();
                        view_roll += sr.to_radians();
                    }
                    // The shellshock off `cg.snap`'s playerstate
                    // (cgame 0x3002f7d0, 0x3002e900); its kick
                    // turns the view after the quake (0x30033dbd).
                    let shock = match render_time {
                        Some(now) => {
                            if let Some(args) = view.shock_command.take() {
                                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                                if let Err(e) = view.shock.command(&self.fs, &args, now) {
                                    console::log::print(&e);
                                }
                            }
                            let snaps = net.snapshots();
                            let fields = snaps
                                .two_for_time(now)
                                .map(|(a, _)| a)
                                .or(snaps.newest())
                                .map_or((0, 0, 0), |s| {
                                    let f = |n: &str| s.ps.field_i32(p, n);
                                    (
                                        f("shellshockIndex"),
                                        f("shellshockTime"),
                                        f("shellshockDuration"),
                                    )
                                });
                            let cs = net.configstrings();
                            view.shock.frame(
                                &self.fs,
                                |i| {
                                    p.cs.slot(net::protocol::CsRange::ShellShock, i)
                                        .and_then(|k| cs.get(k))
                                        .map(String::as_str)
                                },
                                fields,
                                now,
                            )
                        }
                        None => play::shock::ShockFrame::rest(),
                    };
                    if shock.kick != [0.0; 2] {
                        let pitch;
                        [pitch, cam.yaw, view_roll] = play::shock::kicked_angles(
                            [
                                -cam.pitch.to_degrees(),
                                cam.yaw.to_degrees(),
                                view_roll.to_degrees(),
                            ],
                            shock.kick,
                        )
                        .map(f32::to_radians);
                        cam.pitch = -pitch;
                    }
                    input.shellshock = shock.slowed;
                    if let Some(hud) = &mut self.hud {
                        hud.drawn_view = Some([
                            -cam.pitch.to_degrees(),
                            cam.yaw.to_degrees(),
                            view_roll.to_degrees(),
                        ]);
                    }
                    input.set_mouse_caps(dt * 1000.0, [shock.mouse.max_pitch, shock.mouse.max_yaw]);
                    self.audio.set_shellshock(&self.fs, &shock.sound);
                    r.set_screen_blend(shock.blend_ms);

                    // `0x300371f0` poses the view weapon off the
                    // finished view, the gun's aim stepped once for
                    // it and the scope (combat doc 17.4).
                    let local_player = ps_client == client_num && pmove::predict::on_foot(pm_type);
                    if let Some(hud) = &mut self.hud {
                        hud.set_shock_left(shock.left_ms);
                    }
                    let gun = match (&mut self.hud, net.snapshots().newest()) {
                        (Some(hud), Some(s)) => hud.step_gun(
                            p,
                            &s.ps,
                            predicted.as_ref().filter(|_| local_player).map(|v| &v.pred),
                            render_time.unwrap_or(0),
                            weapons,
                            view.gun_kick(),
                        ),
                        _ => None,
                    };
                    let world_up = {
                        let [f, l, u] = pmove::aim::angles_to_axis_msvc([
                            -cam.pitch.to_degrees(),
                            cam.yaw.to_degrees(),
                            view_roll.to_degrees(),
                        ]);
                        Vec3::new(f[2], l[2], u[2])
                    };
                    let cg_fov = cg_fov(&self.shell);
                    view.draw_gun =
                        play::view::DrawGun::from_cvar(self.shell.cvar_f32("cg_drawGun") as i32);
                    let (mut vm_draw, fov) =
                        view.frame(cg_fov, weapons, view_ps.as_ref(), local_ms, gun, world_up);
                    // `0x30033e97`: the shellshock's sensitivity
                    // scale rides the zoom's, on the 4:3 fovs.
                    self.look_zoom = (
                        fov / cg_fov * shock.mouse.sensitivity,
                        view_ps.as_ref().is_some_and(|v| v.mounted),
                    );
                    let fov = camera::view_fov_x(fov, aspect, widescreen);
                    if let Some(d) = &mut vm_draw {
                        d.fov_x = fov;
                    }
                    vm = vm_draw;

                    let (cam_forward, cam_right, cam_up) = camera::basis(cam.yaw, cam.pitch);
                    // The ridden body is `skip_num`, never in `entity_pos`,
                    // so the listener is told which entity rides the camera.
                    self.audio.set_listener(
                        cam.pos,
                        cam_forward,
                        cam_right,
                        cam_up,
                        audio::cues::ps_entity(ps_client),
                    );

                    let (muzzle_pos, muzzle_dir) = view
                        .muzzle(cam.pos, (cam_forward, cam_right, cam_up))
                        .unwrap_or_else(|| view_muzzle(cam.pos, cam_forward, cam_right, cam_up));
                    // Under a scope our own shots draw no flash: with
                    // no muzzle the fire event resolves to nothing.
                    if !view.scoped() {
                        muzzles.insert(u32::MAX, (muzzle_pos, muzzle_dir));
                    }
                    // Bullet hits carry the shooter's number in
                    // `other_entity_num`, and the body the camera rides
                    // (ours, or the followed player's) is excluded from
                    // the muzzle map, so key the view muzzle under it too
                    // for the whizby; its tracer is `view_body`'s call.
                    if let Ok(num) = u32::try_from(skip_num) {
                        muzzles.insert(num, (muzzle_pos, muzzle_dir));
                    }
                    // The ridden body's cases leave the viewmodel's
                    // `tag_brass` (`CG_EjectBrass`, cgame 0x30038a30).
                    if let Some(tag) = view.brass(cam.pos, (cam_forward, cam_right, cam_up)) {
                        self.fx.set_view_brass(tag.0);
                        brass.insert(u32::MAX, tag);
                        if let Ok(num) = u32::try_from(skip_num) {
                            brass.insert(num, tag);
                        }
                    }

                    // Step before this frame's events spawn, or the
                    // [now-dt, now] integration would move particles born
                    // at `now` (a tracer would start ahead of its muzzle).
                    let fx_t0 = Instant::now();
                    prof.next(frame_prof::Stage::FxStep);
                    self.fx.set_view(cam.pos);
                    self.fx.set_bolts(&muzzles);
                    // The `playFxOnTag` tags a live effect rides,
                    // posed as drawn (CoDMP.exe 0x49e8d0, a bolt
                    // posed once a frame).
                    let tag_poses = self
                        .fx
                        .tag_bolts()
                        .into_iter()
                        .filter_map(|b| {
                            let tag = fx::registry::bolt_tag(net.configstrings(), &p.cs, b.slot)?;
                            Some((b, scene.tag_pose(&self.fs, b.entity, tag)?))
                        })
                        .collect();
                    self.fx.set_tag_bolts(&tag_poses);
                    self.fx.step(dt, time, Some(&*world));
                    prof.next(frame_prof::Stage::Fx);
                    let chained = self.fx.spawn_chained(&self.fs, time);
                    // Placed by the wall-clock step (the brass).
                    frame_digest::clocked(|| self.audio.play_fx(&self.fs, chained));
                    // UO's wheel dust (uo cgame 0x30020540): the
                    // world under each wheel, mask 0x211, then
                    // every other vehicle's collmap where drawn.
                    fx::vehicle_trails::place_collmaps(world, &vehicle_wheels);
                    let trails = vehicle_trails.frame(
                        self.shell.cvar_f32("cg_vehicletrails") as i32 != 0,
                        &vehicle_wheels,
                        |n, a, b| {
                            let t = fx::vehicle_trails::wheel_trace(world, n, a, b);
                            (t.fraction < 1.0).then_some(t.surface_flags)
                        },
                    );
                    for play in trails {
                        let at = fx::sim::SpawnAt::Directed {
                            pos: play.pos,
                            dir: play.dir,
                        };
                        let sounds = self.fx.spawn(&self.fs, play.path, at, time);
                        self.audio.play_fx(&self.fs, sounds);
                    }
                    // `ET_FX` off the drawn snapshot (`CG_FX`, cgame
                    // 0x3001cf20), on the drawn server time.
                    if let Some(t) = render_time
                        && let Some((a, _)) = net.snapshots().two_for_time(t)
                    {
                        for play in looped_fx.frame(a, p, t, cam.pos, net.configstrings()) {
                            let sounds = self
                                .fx
                                .spawn_marked(&self.fs, &play.path, play.at, None, time, true);
                            self.audio.play_fx(&self.fs, sounds);
                        }
                    }

                    prof.next(frame_prof::Stage::Events);
                    let (screen_w, screen_h) = r.screen_size();

                    let no_clients = BTreeMap::new();
                    let no_entities = BTreeMap::new();
                    let newest = net.snapshots().newest();
                    let entity_origin = |num: i32| {
                        let num = u32::try_from(num).ok()?;
                        entity_pos.get(&num).map(|v| v.to_array())
                    };
                    // Retail reads `cg_entities[n].currentState`;
                    // the newest snapshot stands in.
                    let turret_weapon = newest.and_then(|s| {
                        let int = |n: &str| s.ps.field_i32(p, n);
                        let num = turret::ridden(
                            int("eFlags"),
                            int("viewlocked"),
                            int("viewlocked_entNum"),
                            p.is_uo(),
                        )?;
                        let w = s.entities.get(&num)?.field_i32(p, "weapon");
                        usize::try_from(w).ok()
                    });
                    // UO's barrel icon turns with the ridden
                    // vehicle's `tag_turret` (uo cgame 0x30031e20).
                    let vehicle_turret_yaw = newest
                        .filter(|s| {
                            s.ps.field_i32(p, "eFlags") & net::flags::EF_IN_VEHICLE_V22 != 0
                        })
                        .and_then(|s| {
                            let num = u32::try_from(s.ps.field_i32(p, "viewlocked_entNum")).ok()?;
                            let f = scene.tag_pose(&self.fs, num, "tag_turret")?.axis[0];
                            Some(f.y.atan2(f.x).to_degrees())
                        });
                    let client_cvar = |c: &str| join.cvars.get(c, net.configstrings(), &self.shell);
                    let server_address = net.transport().peer().map(|a| a.to_string());
                    let hud_frame = hud::HudFrame {
                        now: time,
                        screen_w,
                        screen_h,
                        configstrings: net.configstrings(),
                        clients: newest.map_or(&no_clients, |s| &s.clients),
                        protocol: p,
                        // The render clock, so hudelem tweens and
                        // timers move between snapshots.
                        server_time: render_time.unwrap_or(0),
                        snap_time: render_time
                            .and_then(|t| net.snapshots().two_for_time(t))
                            .map_or(0, |(a, _)| a.server_time.raw()),
                        fs: &self.fs,
                        ps: newest.map(|s| &s.ps),
                        entities: newest.map_or(&no_entities, |s| &s.entities),
                        predicted: predicted.as_ref().filter(|_| local_player).map(|v| &v.pred),
                        local_player,
                        weapons,
                        localized: &self.localized,
                        view_yaw: cam.yaw.to_degrees(),
                        eye: cam.pos.to_array(),
                        fov,
                        entity_origin: &entity_origin,
                        turret_weapon,
                        vehicle_turret_yaw,
                        cvar: &client_cvar,
                        bound_key: &|cmd| bound_key(&self.shell, &self.localized, p, cmd),
                        draw: hud::DrawToggles {
                            crosshair: self.shell.cvar_f32("cg_drawCrosshair") as i32 != 0,
                            status: self.shell.cvar_f32("cg_drawStatus") as i32 != 0,
                        },
                        weapon_select: input.selected(),
                        own_client: net
                            .gamestate()
                            .and_then(|g| u32::try_from(g.client_num).ok()),
                        server_address: server_address.as_deref(),
                    };

                    // UO's tracers: the length gate, and `sv_night`
                    // off the systeminfo (fx::sim::TracerRules).
                    let uo = p.is_uo();
                    self.fx.set_tracer_rules(fx::sim::TracerRules {
                        min_length: uo,
                        night: uo
                            && net::info_value_for_key(net.configstring(1), "sv_night")
                                .and_then(|v| v.trim().parse::<f32>().ok())
                                .is_some_and(|v| v as i32 != 0),
                    });
                    // Events use the newest snapshot, not the interpolation
                    // pair; they must not wait out the interp delay.
                    if let Some(newest) = newest {
                        let ctx = fx::registry::ResolveCtx {
                            brass: &brass,
                            weapons: weapons.as_slice(),
                            eject_brass,
                            configstrings: net.configstrings(),
                            cs: &p.cs,
                            blood,
                            muzzles: &muzzles,
                            shot_fallbacks: &shot_fallbacks,
                            weapon_flash: &weapon_flash,
                            view_flash: vm.is_some().then_some(weapons.as_slice()),
                            view_body: fx::registry::view_body(
                                newest.ps.field_i32(p, "pm_flags"),
                                ps_client,
                            ),
                        };
                        // A client-traced round's hit joins the
                        // queue after its fire event, with the
                        // tracer's tag as its shooter's muzzle.
                        let mut evs: std::collections::VecDeque<_> =
                            evs.into_iter().map(|e| (e, None)).collect();
                        let bodies = play::predict::solid_bodies(
                            p,
                            &newest.entities,
                            client_num as u32,
                            &entity_pos,
                        );
                        // Brush models and vehicles' collmaps where
                        // the newest snapshot has them, as the
                        // cgame's entity clip meets them.
                        let movers =
                            pmove::movers::SnapshotMovers::from_entities(p, &newest.entities);
                        movers.place(world, newest.server_time.raw());
                        let round_trace = |s: Vec3, e: Vec3, skip: u32| {
                            let w = MoveWorld::new(world, &bodies, skip);
                            let t = w.box_trace(
                                s,
                                e,
                                Vec3::ZERO,
                                Vec3::ZERO,
                                fx::client_bullet::MASK_CLIENT_SHOT,
                            );
                            fx::client_bullet::RoundTrace {
                                fraction: t.fraction,
                                endpos: t.endpos,
                                normal: t.normal,
                                surface_flags: t.surface_flags,
                                entity: w.entity_num(&t),
                            }
                        };
                        // The second trace stops only on a
                        // vehicle; the coaxial's passes its own.
                        let vehicle_trace = |s: Vec3, e: Vec3, skip: Option<u32>| {
                            let own = skip.map(collision::Pusher::Copy);
                            let t = world.point_trace_except(
                                s,
                                e,
                                fx::client_bullet::MASK_VEHICLE_SHOT,
                                false,
                                own,
                            );
                            let entity = world.entity_num(&t);
                            newest
                                .entities
                                .get(&entity)
                                .is_some_and(|v| {
                                    v.field_i32(p, "eType") == net::flags::ET_VEHICLE_V22
                                })
                                .then_some(fx::client_bullet::RoundTrace {
                                    fraction: t.fraction,
                                    endpos: t.endpos,
                                    normal: t.normal,
                                    surface_flags: t.surface_flags,
                                    entity,
                                })
                        };
                        let view = fx::client_bullet::View {
                            origin: cam.pos,
                            angles: [0, 1, 2]
                                .map(|i| newest.ps.field_f32(p, &format!("viewangles[{i}]"))),
                        };
                        while let Some((ev, round_muzzle)) = evs.pop_front() {
                            let ctx = match &round_muzzle {
                                Some(m) => fx::registry::ResolveCtx { muzzles: m, ..ctx },
                                None => fx::registry::ResolveCtx { ..ctx },
                            };
                            let muzzles = ctx.muzzles;
                            if let Some(round) = fx::registry::client_round(&ev, &ctx)
                                && let Some(pose) =
                                    scene.tag_pose(&self.fs, round.entity, round.tag)
                            {
                                let def = usize::try_from(round.weapon)
                                    .ok()
                                    .and_then(|w| weapons.get(w)?.as_ref());
                                match self.client_bullets.fire(
                                    time.to_bits(),
                                    &round,
                                    def,
                                    &pose,
                                    Some(view),
                                    &round_trace,
                                    &vehicle_trace,
                                ) {
                                    Some(fx::client_bullet::Outcome::Hit {
                                        pos,
                                        normal,
                                        reflect,
                                        surf_type,
                                    }) => {
                                        // No tag, no tracer (0x30048b60).
                                        let tracer: HashMap<u32, (Vec3, Vec3)> = scene
                                            .tag_pose(&self.fs, round.entity, round.tracer_tag)
                                            .map(|t| (round.entity, (t.origin, t.axis[0])))
                                            .into_iter()
                                            .collect();
                                        evs.push_front((
                                            fx::client_bullet::hit_event(
                                                &round, pos, normal, reflect, surf_type,
                                            ),
                                            Some(tracer),
                                        ));
                                    }
                                    Some(fx::client_bullet::Outcome::Miss { start, end }) => {
                                        let heavy = def.is_some_and(fx::registry::heavy_round);
                                        self.fx.spawn_tracer(
                                            (start, end),
                                            false,
                                            (heavy, true),
                                            time,
                                            dt,
                                        );
                                    }
                                    None => {}
                                }
                            }
                            self.ev_seen += 1;
                            if let Some(hud) = &mut self.hud {
                                hud.on_game_event(&ev, &hud_frame);
                            }
                            if let Some(s) = play::events::forced_stance(
                                &ev,
                                client_num,
                                // Retail's `+prone` / `+movedown`
                                // set it 1 while held (0x40aa60).
                                self.shell.cvar_f32("cl_stanceTemp") as i32 != 0
                                    || input.stance_held(),
                            ) {
                                input.force_stance(s);
                            }
                            if let Some(w) =
                                play::events::picked_up_weapon(&ev, ctx.view_body, net.protocol())
                                && input.selected() == 0
                            {
                                input.picked_up(w);
                                if let Some(hud) = &mut self.hud {
                                    hud.weapon_selected();
                                }
                            }
                            if play::events::runs_out_of_ammo(&ev, ctx.view_body) {
                                // Off the predicted playerstate, as
                                // 0x300381b0 reads it.
                                if let Some(v) = predicted.as_ref().filter(|_| local_player) {
                                    input.predict_held(&v.pred.ps, weapons);
                                }
                                input.out_of_ammo();
                            }
                            self.audio.on_game_event(
                                &self.fs,
                                &ev,
                                net.configstrings(),
                                muzzles,
                                ctx.shot_fallbacks,
                            );
                            for r in fx::registry::resolve(&ev, &ctx) {
                                match r {
                                    fx::registry::Resolved::Spawn {
                                        path,
                                        at,
                                        nomarks,
                                        bolt,
                                        brass,
                                    } => {
                                        // `cullrange` from the origin the
                                        // cgame hands syscall 0xe5: the
                                        // view weapon's last `tag_brass`
                                        // for a case, the shooter's
                                        // `lerpOrigin` for a flash.
                                        let cull_from = if brass {
                                            Some(self.fx.view_brass())
                                        } else {
                                            bolt.filter(|&n| n != u32::MAX)
                                                .and_then(|n| scene.drawn_origin(n))
                                        };
                                        let sounds = self.fx.spawn_marked_from(
                                            &self.fs, &path, at, bolt, time, !nomarks, cull_from,
                                        );
                                        self.audio.play_fx(&self.fs, sounds);
                                    }
                                    fx::registry::Resolved::OnTag {
                                        path,
                                        entity,
                                        tag,
                                        slot,
                                    } => {
                                        // The playerState ring's
                                        // events are the view
                                        // body's, posed undrawn.
                                        let entity = match entity {
                                            u32::MAX => ps_client as u32,
                                            n => n,
                                        };
                                        // `cullrange` from its `lerpOrigin`
                                        // (cgame 0x3001db80).
                                        let origin = scene.drawn_origin(entity);
                                        if let Some(pose) = scene.tag_pose(&self.fs, entity, &tag) {
                                            let bolt = fx::sim::TagBolt { entity, slot };
                                            let sounds = self.fx.spawn_on_tag_from(
                                                &self.fs, &path, bolt, pose, time, origin,
                                            );
                                            self.audio.play_fx(&self.fs, sounds);
                                        }
                                    }
                                    fx::registry::Resolved::Tracer {
                                        muzzle,
                                        impact,
                                        flesh,
                                        heavy,
                                        roll,
                                    } => {
                                        self.fx.spawn_tracer(
                                            (muzzle, impact),
                                            flesh,
                                            (heavy, roll),
                                            time,
                                            dt,
                                        );
                                    }
                                    fx::registry::Resolved::Known => {}
                                    fx::registry::Resolved::Unknown => {
                                        self.ev_unknown += 1;
                                        let foreign = ev.event & net::event_ids::EV_FOREIGN;
                                        if foreign != 0
                                            && self.foreign_events.insert(ev.event & !foreign)
                                        {
                                            log::info!(
                                                "protocol {} event {} not played",
                                                p.transport.version,
                                                ev.event & !foreign
                                            );
                                        }
                                        log::debug!("unknown event {} parm {}", ev.event, ev.parm);
                                    }
                                }
                            }
                        }
                    }

                    // A UO effect's `CameraShake` feeds the same
                    // shake core as an earthquake (uo cgame
                    // vmMain 16, 0x3002b0e0).
                    for sh in self.fx.take_shakes() {
                        if let Some(now) = render_time {
                            view.start_quake(
                                play::quake::Quake {
                                    start_ms: 0,
                                    scale: sh.scale,
                                    duration_ms: sh.duration_ms,
                                    radius: sh.radius,
                                    origin: sh.origin,
                                },
                                now,
                            );
                        }
                    }

                    prof.next(frame_prof::Stage::Sound);
                    // `es.loopSound` (docs/research/cod11-sound-system.md,
                    // section 9) and each missile's `projectileSound`
                    // (section 8), reconciled once per snapshot. An entity
                    // that left the snapshot is absent from the map, which
                    // is what stops its loop.
                    if let Some(newest) = newest
                        && *last_loop_snap != Some(newest.message_num)
                    {
                        *last_loop_snap = Some(newest.message_num);
                        let loops: HashMap<u32, (i32, Vec3)> = newest
                            .entities
                            .iter()
                            .filter_map(|(&num, e)| {
                                let idx = e.field_i32(p, "loopSound");
                                (idx != 0).then(|| (num, (idx, Vec3::from(e.origin(p)))))
                            })
                            .collect();
                        self.audio
                            .set_loop_sounds(&self.fs, net.configstrings(), &loops);
                        self.audio.set_projectile_sounds(
                            &self.fs,
                            net.configstrings(),
                            &audio::missile_sounds(p, newest),
                        );
                    }

                    // UO's sound blend entities, every frame
                    // off the drawn pair (uo cgame 0x30021860).
                    if let Some(t) = render_time
                        && let Some((a, b)) = net.snapshots().two_for_time(t)
                    {
                        let f = ((Msec::from_raw(t) - a.server_time) as f32
                            / (b.server_time - a.server_time).max(1) as f32)
                            .clamp(0.0, 1.0);
                        // A blend restarts when its voice ran out,
                        // on the wall clock.
                        frame_digest::clocked(|| {
                            self.audio.step_sound_blends(
                                &self.fs,
                                p,
                                (a, b, f),
                                (net.configstrings(), weapons),
                                &entity_pos,
                            )
                        });
                    }

                    // After the drain, so new voices get this frame's positions.
                    self.audio
                        .step_fire_loops(&self.fs, dt * 1000.0, &entity_pos, &muzzles);
                    self.audio.step(&entity_pos, &entity_angles, Some(&*world));

                    prof.next(frame_prof::Stage::Pose);
                    // The brass below steps on wall-clock time.
                    frame_digest::models(&instances);
                    // After the events, so a case ejected this frame
                    // draws this frame.
                    for m in self.fx.models(time) {
                        if let Some(model) = scene.fx_model(r, &self.fs, &m.model) {
                            let [f, l, u] = m.axis;
                            instances.push(DynamicModelInstance {
                                model,
                                transform: glam::Mat4::from_cols(
                                    f.extend(0.0),
                                    l.extend(0.0),
                                    u.extend(0.0),
                                    m.origin.extend(1.0),
                                ),
                                bones: None,
                                light_origin: m.origin,
                                shader_time: None,
                            });
                        }
                    }
                    frame_prof::count(frame_prof::Count::Models, instances.len());
                    frame_prof::count(frame_prof::Count::Particles, self.fx.counts().0);
                    r.set_dynamic_models(&instances);
                    prof.next(frame_prof::Stage::FxDraw);

                    // `cg.snap`'s own player and solid bodies, the
                    // older of the drawn pair (coduo-diff.md 19):
                    // the chunks clip them and the report tests
                    // the player.
                    let (victim, bodies) = if p.is_uo() {
                        let snap = render_time
                            .and_then(|t| net.snapshots().two_for_time(t))
                            .map(|(a, _)| a)
                            .or(newest);
                        let victim = snap.and_then(|s| {
                            let v = |n: &str| {
                                Vec3::from_array(
                                    [0, 1, 2].map(|i| s.ps.field_f32(p, &format!("{n}[{i}]"))),
                                )
                            };
                            Some(fx::flame::Victim {
                                client: u32::try_from(s.ps.field_i32(p, "clientNum")).ok()?,
                                origin: v("origin"),
                                mins: v("mins"),
                                maxs: v("maxs"),
                            })
                        });
                        let bodies = snap
                            .zip(victim)
                            .map(|(s, v)| {
                                play::predict::solid_bodies(p, &s.entities, v.client, &entity_pos)
                            })
                            .unwrap_or_default();
                        (victim, bodies)
                    } else {
                        (None, Vec::new())
                    };
                    // UO's flamethrower streams off each firing
                    // body's `tag_flash` (fx::flame).
                    let nozzles = newest
                        .map(|s| fx::flame::nozzles(p, s, ps_client, weapons, &muzzles))
                        .unwrap_or_default();
                    if !nozzles.is_empty() || !self.flame.is_empty() {
                        let sounds = self.flame.step(
                            time * 1000.0,
                            dt * 1000.0,
                            &nozzles,
                            cam.pos,
                            Some(&fx::flame::ChunkWorld {
                                world,
                                bodies: &bodies,
                                local: victim,
                                max_clients: fx::flame::max_clients(net.configstring(0)),
                            }),
                        );
                        for s in sounds {
                            use fx::flame::FlameSound;
                            let cue = |alias: &str, owner: u32, pos: Vec3| audio::cues::Cue {
                                alias: alias.to_string(),
                                source: audio::cues::Source::Entity { num: owner, pos },
                                delay_s: 0.0,
                            };
                            match s {
                                FlameSound::Start { owner, pos } => {
                                    self.audio
                                        .play(&self.fs, cue("flamethrower_start", owner, pos));
                                }
                                FlameSound::Fire { owner, pos } => {
                                    self.audio.hold(&self.fs, owner, "flamethrower_fire", pos)
                                }
                                FlameSound::Stop { owner, pos } => {
                                    self.audio.release(owner);
                                    self.audio
                                        .play(&self.fs, cue("flamethrower_cooldown", owner, pos));
                                }
                            }
                        }
                    }
                    // UO's flame report on `cg.snap`'s player.
                    let mut fdc = None;
                    if p.is_uo() {
                        let flame_trace = |s: Vec3, e: Vec3| {
                            let w = MoveWorld::new(world, &bodies, u32::MAX);
                            let t = w.box_trace(
                                s,
                                e,
                                Vec3::ZERO,
                                Vec3::ZERO,
                                fx::client_bullet::MASK_CLIENT_SHOT,
                            );
                            (t.fraction, w.entity_num(&t))
                        };
                        let report = self.flame.report(
                            time * 1000.0,
                            victim,
                            net.flood_window_open(),
                            Some(&flame_trace),
                        );
                        input.in_flame = report.in_flame;
                        fdc = report.fdc;
                    }
                    let mut fx_quads = self.fx.build_quads(cam.pos, cam_right, cam_up, time);
                    fx_quads.extend(self.flame.quads(time * 1000.0, cam_right, cam_up));
                    if let Some(newest) = newest {
                        fx_quads.extend(head_icon::quads(
                            &head_icon::Scene {
                                protocol: p,
                                entities: &newest.entities,
                                clients: &newest.clients,
                                configstrings: net.configstrings(),
                                viewer: ps_client,
                                own: client_num,
                                eye: cam.pos,
                                heads: &heads,
                                entity_pos: &entity_pos,
                            },
                            cam_right,
                            cam_up,
                        ));
                        fx::sim::sort_back_to_front(&mut fx_quads, cam.pos);
                    }
                    r.set_fx_quads(&self.fs, fx_quads, time);
                    // CoD2's snow (fx::cod2_cloud), tested against the map's roofs
                    let clouds = self.fx.clouds(cam.pos, cam_right, cam_up, time);
                    let map = self.session.map_name();
                    r.set_fx_clouds(&self.fs, map, &clouds, time);
                    r.set_marks(
                        &self.fs,
                        &self.fx.take_marks(time),
                        &shadow_marks,
                        (time * 1000.0) as i32,
                        time,
                        self.shell.cvar_f32("cg_marks") != 0.0,
                    );
                    r.set_fx_lights(&self.fx.lights(cam.pos, time));
                    r.set_fog(
                        net::FogParams::parse(net.configstring(net.protocol().cs.fog)),
                        time,
                    );
                    self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;

                    prof.next(frame_prof::Stage::Hud);
                    if let Some(hud) = &mut self.hud {
                        let hud_t0 = Instant::now();
                        hud.set_message_widths(
                            self.shell.cvar_f32("cg_gameMessageWidth") as i32,
                            self.shell.cvar_f32("cg_gameBoldMessageWidth") as i32,
                            screen_w as u32,
                        );
                        hud.set_chat_rules(hud::chat::Rules::read(
                            |c| self.shell.cvar(c),
                            p.is_uo(),
                        ));
                        *hud_quads = hud.build(&hud_frame);
                        self.hud_ms = hud_t0.elapsed().as_secs_f32() * 1000.0;
                        // The vote draw's `player_talk` on a
                        // changed time or count.
                        if std::mem::take(&mut hud.vote_sound) {
                            self.audio.play_local(&self.fs, "player_talk");
                        }
                    }
                    // After the HUD, which borrows the snapshot ring.
                    if let Some(n) = fdc {
                        net.send_reliable(&format!("fdc {n}"));
                    }

                    renderer::Frame {
                        view_proj: camera::view_proj_from(
                            cam.pos, cam.yaw, cam.pitch, view_roll, fov, aspect,
                        ),
                        eye: cam.pos,
                        fwd: cam_forward,
                        up: Vec3::Z,
                        time,
                        fov_x: fov,
                        cull,
                        hud_lines: Vec::new(),
                    }
                }
            }
        };
        (frame, vm)
    }
}
