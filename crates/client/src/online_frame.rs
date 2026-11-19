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
            time,
            local_ms,
            cull,
            aspect,
            ..
        } = *tick;
        let Mode::Online { net, phase, .. } = &mut self.mode else {
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
        let gamestate_ready = self.online_events(tick, &events, demo_dirs, fatal, demo_over);

        self.online_cmds(tick, gamestate_ready, demo_clock, fatal);
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Online {
            net,
            input,
            predictor,
            view,
            phase,
            join,
            ..
        } = &mut self.mode
        else {
            unreachable!()
        };

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
            Phase::Live(_) => {
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
                    self.live_frame(tick, &mut prof, &mut vm, hud_quads)
                }
            }
        };
        (frame, vm)
    }

    /// The frame's net events, then the menus' and quick chat's share of
    /// them; true when a gamestate came.
    pub(crate) fn online_events(
        &mut self,
        tick: &Tick,
        events: &[net::NetEvent],
        demo_dirs: &[std::path::PathBuf],
        fatal: &mut Option<anyhow::Error>,
        demo_over: &mut bool,
    ) -> bool {
        let Tick { time, .. } = *tick;
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Online {
            net,
            input,
            predictor,
            view,
            phase,
            join,
            ..
        } = &mut self.mode
        else {
            unreachable!()
        };
        let mut gamestate_ready = false;
        for ev in events {
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
        gamestate_ready
    }

    /// After the net events: the gamestate's search path, the soak, the demo
    /// clock and the cmds.
    pub(crate) fn online_cmds(
        &mut self,
        tick: &Tick,
        gamestate_ready: bool,
        demo_clock: play::clock::DemoClock,
        fatal: &mut Option<anyhow::Error>,
    ) {
        let Tick { time, local_ms, .. } = *tick;
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
            ..
        } = &mut self.mode
        else {
            unreachable!()
        };
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
                        restart_ui(
                            &mut self.ui,
                            &mut self.shell,
                            &self.fs,
                            &self.game_dir,
                            &self.mod_dir,
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
    }
}
