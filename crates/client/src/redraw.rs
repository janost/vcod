//! `App::window_event`'s `RedrawRequested`: one frame.

use super::*;

impl App {
    /// One frame, then the next frame's request: every early return of
    /// `redraw_frame` (a drop, a demo played out) keeps the redraw chain going.
    pub(crate) fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        self.redraw_frame(event_loop);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// One frame of whichever mode is up, then the 2D layer, the render and
    /// the frame's end.
    fn redraw_frame(&mut self, event_loop: &ActiveEventLoop) {
        {
            let _pace = frame_prof::span(frame_prof::Stage::Pace);
            self.pace_frame();
        }
        let frame_start = Instant::now();
        self.mouse_frame();
        self.cod2_script_menus();
        let now = frame_clock::tick(Instant::now(), self.avidemo());
        let dt = now
            .saturating_duration_since(self.last_frame)
            .as_secs_f32()
            .min(0.1);
        self.last_frame = now;
        let elapsed = now.saturating_duration_since(self.start);
        let time = elapsed.as_secs_f64();
        let local_ms = elapsed.as_secs_f64() * 1000.0;
        let cull = self.cull_mode;
        let blood = self.shell.cvar_f32("cg_blood") as i32 != 0;
        let eject_brass = self.shell.cvar_f32("cg_brass") as i32 != 0;
        self.texture_mode_changed();
        self.audio
            .set_master_volume(self.shell.cvar_f32(audio::volume_cvar(&self.shell)));
        let gamma = gamma_cvar(&mut self.shell);
        let fullscreen = self
            .window
            .as_ref()
            .is_some_and(|w| w.fullscreen().is_some());
        let overbright = gamma::overbright_bits(self.fs.game(), fullscreen, !self.ignore_hw_gamma);
        // A drop or a failed map load ends the game for the main
        // menu with its reason, as a CoD1 server's does.
        if let Err(e) = self.cod2_level() {
            self.disconnect(Some(format!("{e:#}")));
        }
        // The join in flight: its session and map once loaded; a
        // refused or failed join or load comes back to the main
        // menu with its reason.
        if let Err(e) = self.cod4_join_frame() {
            self.unload_world();
            self.cod4_join_failed(&format!("{e:#}"));
        }
        // A CoD4 map that fails to load ends the game for the main
        // menu with its reason.
        if let Err(e) = self.cod4_level() {
            self.disconnect(Some(format!("{e:#}")));
        }
        // A CoD4 drop (`w`, an out-of-band `disconnect`, `cl_timeout`
        // of silence) ends the game for the main menu, its reason
        // the server's notice (`Com_SetErrorMessage`).
        if let Some(why) = self
            .session
            .cod4_mut()
            .and_then(|c| c.session.take_dropped())
        {
            let why = play::cod4::localize_drop(&why, &self.game_dir);
            log::error!("{why}");
            self.leave_cod4_game(Some((&why, true)));
            return;
        }
        // The CoD4 systeminfo's dvars, `sv_cheats` among them
        // (`CL_SystemInfoChanged`, iw3mp.exe 0x473ab0).
        if let Some(si) = self
            .session
            .cod4_mut()
            .and_then(|c| c.session.take_systeminfo())
        {
            let out = self.shell.apply_systeminfo(&si);
            self.apply(event_loop, out);
        }
        // A CoD2 demo played out (`CL_DemoCompleted`, Mac 0x14c096).
        if let Some(line) = self.session.demo_over() {
            self.demo_finished(event_loop, line);
            return;
        }
        self.apply_render_cvars(gamma, overbright);
        let Some(r) = &mut self.renderer else { return };
        let aspect = r.aspect();
        // vcod's Hor+: every fov below is a 4:3 view's width until
        // the frame widens it (camera::view_fov_x).
        let widescreen = self.shell.cvar_f32("vc_widescreen") != 0.0;
        // Set inside the online arm where `self` is borrowed out
        // field-by-field; acted on once the borrows end.
        let mut fatal: Option<anyhow::Error> = None;
        // The demo played out: back to the menu, no error.
        let mut demo_over = false;
        // What the mode draws on the 2D layer; the console goes on top.
        let mut hud_quads: Vec<hud::HudQuad> = Vec::new();
        if let Some(l) = &mut self.cod4_loading {
            hud_quads.extend(l.quads(r.screen_size(), elapsed.as_millis() as i32));
        }
        let tick = Tick {
            now,
            dt,
            elapsed,
            time,
            local_ms,
            cull,
            blood,
            eject_brass,
            aspect,
            widescreen,
        };
        let (mut frame, vm) = match &mut self.mode {
            Mode::Idle => (
                renderer::Frame {
                    // Nothing is drawn, so any projection works.
                    view_proj: camera::view_proj_from(
                        Vec3::ZERO,
                        0.0,
                        0.0,
                        0.0,
                        camera::DEFAULT_FOV_DEG,
                        aspect,
                    ),
                    eye: Vec3::ZERO,
                    fwd: camera::basis(0.0, 0.0).0,
                    up: Vec3::Z,
                    time,
                    fov_x: camera::DEFAULT_FOV_DEG,
                    cull,
                    hud_lines: Vec::new(),
                },
                None,
            ),
            Mode::Fly(_) => self.fly_frame(&tick, &mut hud_quads),
            Mode::Cod2(_) => self.cod2_frame(&tick, &mut hud_quads),
            Mode::Online { .. } => {
                self.online_frame(&tick, &mut hud_quads, &mut fatal, &mut demo_over)
            }
            Mode::Walk { .. } => self.walk_frame(&tick, &mut hud_quads),
        };
        // A drop or a failed load ends the session, not the program:
        // retail falls back to its console and menus.
        if demo_over && fatal.is_none() {
            self.demo_completed(event_loop, local_ms as i32);
            return;
        }
        if let Some(err) = fatal {
            self.disconnect(Some(format!("{err:#}")));
            return;
        }
        self.frame_ui(&tick, hud_quads);
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        // The scene lives in the online phase, so pull it back
        // out for the overlay after the mode's mutable borrows end.
        let scene = match &self.mode {
            Mode::Online {
                phase: Phase::Live(live),
                ..
            } => Some(&live.scene),
            _ => None,
        };
        self.hud_stats.frame(
            dt,
            scene.map_or(0, |s| s.stats.anim_restarts),
            self.interp_misses,
        );
        if self.debug_overlay {
            frame.hud_lines = hud_lines(
                &self.mode,
                self.session.cam_tag(),
                scene,
                &self.hud_stats,
                (self.build_ms, self.render_ms, self.fx_ms),
                (self.ev_seen, self.ev_unknown),
                self.fx.counts(),
                (
                    r.hud_quad_count(),
                    self.hud_ms,
                    self.hud.as_ref().map_or(0, |h| h.unknown),
                ),
                self.audio.stats(),
                r,
            );
            if let Some(lines) = self.session.debug_lines() {
                frame.hud_lines.extend(lines);
            }
        }
        if frame_digest::on() {
            frame_digest::view(&frame, vm.as_ref(), !self.session.live());
        }
        if self.shot.is_some() {
            r.request_capture(screenshot::pre_gamma(self.game, overbright));
        }
        let t0 = Instant::now();
        r.render(frame, vm);
        self.render_ms = t0.elapsed().as_secs_f32() * 1000.0;
        if let Some(cap) = r.take_capture()
            && let Some(req) = self.shot.take()
        {
            let lines = match cap {
                Some(cap) => {
                    let dir = self.gamedir();
                    let map = self.world.as_ref().map(|w| w.name.as_str());
                    self.shooter.run(self.game, &dir, map, &req, &cap)
                }
                None => vec!["^1ERROR: cannot take screenshot".into()],
            };
            for line in lines {
                console::log::print(&line);
            }
        }
        if vcod_common::laps::loading()
            && (self.digest_time().is_some() || self.session.cod4_in_game())
        {
            vcod_common::laps::span(frame_start, || "the first frame in game".into());
            vcod_common::laps::end_load();
            vcod_common::laps::trim_heap();
        }
        if frame_digest::on() {
            frame_digest::end_frame(self.digest_time());
        }
        if let Some(soak) = &mut self.soak {
            soak.end_frame(self.audio.stats().misses);
            for ask in soak.take_asks() {
                match ask {
                    play::cod1_soak::Ask::Scores(on) => {
                        let effects = self.shell.execute(if on { "+scores" } else { "-scores" });
                        self.apply(event_loop, effects);
                    }
                    play::cod1_soak::Ask::Esc if self.ui.active() => {
                        let (_, effects) = self.ui.key(KeyCode::Escape, &self.shell);
                        self.ui_effects(event_loop, effects);
                        self.after_menu();
                    }
                    play::cod1_soak::Ask::Esc => self.escape(),
                }
            }
        }
        frame_prof::end_frame();
        if (!self.exec_in_game.is_empty()
            || !self.exec_after.is_empty() && self.exec_after_last.is_none())
            && (self.digest_time().is_some() || self.session.cod4_in_game())
        {
            for line in std::mem::take(&mut self.exec_in_game) {
                let effects = self.shell.execute(&line);
                self.apply(event_loop, effects);
            }
            self.exec_after_last = Some(std::time::Instant::now());
        }
        if let (Some(last), Some(&(secs, _))) = (self.exec_after_last, self.exec_after.front())
            && last.elapsed().as_secs_f32() >= secs
        {
            let (_, line) = self.exec_after.pop_front().expect("front");
            frame_prof::exec(&line);
            let effects = self.shell.execute(&line);
            self.apply(event_loop, effects);
            // after the line's own work (a `vid_restart`'s rebuild)
            self.exec_after_last = Some(std::time::Instant::now());
        }
    }

    /// The renderer's settings off their cvars, once a frame.
    pub(crate) fn apply_render_cvars(&mut self, gamma: f32, overbright: u32) {
        let Some(r) = &mut self.renderer else { return };
        r.set_gamma(if self.ignore_hw_gamma { 1.0 } else { gamma }, overbright);
        r.set_bicubic(self.shell.cvar_f32("vc_bicubic") != 0.0);
        r.set_bloom(
            self.shell.cvar_f32("vc_bloom"),
            self.shell.cvar_f32("vc_bloomThreshold"),
        );
        r.set_ssao(self.shell.cvar_f32("vc_ssao"));
        r.set_soft_particles(self.shell.cvar_f32("vc_softParticles"));
        r.set_shadows(self.shell.cvar_f32("vc_shadows") != 0.0);
        r.set_sun_shadow(
            self.shell.cvar_f32("sm_enable") != 0.0,
            vcod_cod4::sunshadow::Settings {
                sample_size_near: self.shell.cvar_f32("sm_sunSampleSizeNear"),
                scale: self.shell.cvar_f32("sm_sunShadowScale"),
                center: self
                    .shell
                    .cvar("sm_sunShadowCenter")
                    .and_then(vcod_common::bsp::parse_vec3)
                    .map_or(glam::Vec3::ZERO, glam::Vec3::from),
            },
        );
        let on = |n: &str| self.shell.cvar_f32(n) != 0.0;
        r.set_spot_shadow(crate::cod4_spotshadow::Settings {
            on: on("r_spotLightShadows") && on("sm_spotEnable"),
            entities: on("r_spotLightEntityShadows"),
            smodels: on("r_spotLightSModelShadows"),
            max_lights: (self.shell.cvar_f32("sm_maxLights").max(0.0) as usize)
                .min(crate::cod4_spotshadow::MAX_LIGHTS),
            fade_time: self
                .shell
                .cvar_f32("sm_spotShadowFadeTime")
                .clamp(0.01, 5.0),
            eye_project_dist: self.shell.cvar_f32("sm_lightScore_eyeProjectDist"),
            spot_project_frac: self.shell.cvar_f32("sm_lightScore_spotProjectFrac"),
            offset_bias: self.shell.cvar_f32("sm_polygonOffsetBias"),
            offset_scale: self.shell.cvar_f32("sm_polygonOffsetScale"),
        });
        r.set_cookie_settings(cookie_settings(&self.shell));
        r.set_post(post::PostSettings {
            grade: self.shell.cvar_f32("vc_grade"),
            vignette: self.shell.cvar_f32("vc_vignette"),
            grain: self.shell.cvar_f32("vc_grain"),
        });
        r.set_flares(self.shell.cvar_f32("r_flares") as i32 != 0);
        r.set_draw_sun(self.shell.cvar_f32("r_drawSun") as i32 != 0);
        r.set_swap_interval(self.shell.cvar_f32("r_swapInterval") as i32 != 0);
        // CoD4's Model Detail sets the rigid pair (iw3mp.exe
        // 0x62a29c; cod4-front-end.md 9); vcod has one LOD scale.
        let (lod_scale, lod_bias) = if self.game == vcod_common::game::Game::Cod4 {
            ("r_lodScaleRigid", "r_lodBiasRigid")
        } else {
            ("r_lodscale", "r_lodbias")
        };
        r.set_lod(
            self.shell.cvar_f32(lod_scale),
            self.shell.cvar_f32(lod_bias),
        );
        if let Some(c) = self.session.cod4_mut() {
            c.session.menus.listen_server = self
                .server_process
                .as_mut()
                .is_some_and(|s| s.joined && s.exited().is_none());
        }
        // CoD4's views project at `r_aspectRatio`'s aspect, the
        // value its last `vid_restart` latched, and its 2D layer
        // places by the same `vidConfig` (camera::Cod4Screen)
        r.set_view_aspect((self.game == vcod_common::game::Game::Cod4).then(|| {
            let (w, h) = r.screen_size();
            let setting = self.shell.cvar("r_aspectRatio").unwrap_or_default();
            let screen = camera::Cod4Screen::of(
                setting,
                w,
                h,
                self.shell.cvar_f32("r_fullscreen") as i32 != 0,
                cvar_on(&self.shell, "vc_windowAspect"),
            );
            hud::cod2elem::set_cod4_screen(&screen);
            screen.aspect
        }));
        r.set_cod4_options(cod4_options(&self.shell));
        r.set_depth_prepass(cvar_on(&self.shell, "r_vc_depthprepass"));
        r.set_dynamic_lights(
            self.shell.cvar_f32("r_dynamiclight") as i32,
            self.shell.cvar_f32("r_dlightQuality") as i32 != 0,
        );
        r.set_sun_shafts(self.shell.cvar_f32("vc_sunShafts"));
    }

    pub(crate) fn fly_frame(
        &mut self,
        tick: &Tick,
        hud_quads: &mut Vec<hud::HudQuad>,
    ) -> (renderer::Frame, Option<renderer::VmDraw>) {
        let Tick {
            dt,
            time,
            cull,
            aspect,
            widescreen,
            ..
        } = *tick;
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Fly(cam) = &mut self.mode else {
            unreachable!()
        };
        self.look_zoom = (1.0, false);
        cam.update(&offline_binds::fly(&self.input), dt);
        if let Some(av) = &mut self.cod4_fly {
            av.frame(&self.fs, &mut self.audio, &mut self.fx, time);
            r.set_level_look(Some(av.look.at((time * 1000.0) as i32, false)));
        }
        let fly_fov = camera::view_fov_x(camera::DEFAULT_FOV_DEG, aspect, widescreen);
        let view = session::DrawView {
            yaw: cam.yaw,
            pitch: cam.pitch,
            roll: 0.0,
            fov: fly_fov,
            vm: None,
        };
        ViewTail {
            fs: &self.fs,
            shell: &self.shell,
            audio: &mut self.audio,
            fx: &mut self.fx,
            session: &self.session,
            r,
            time,
            dt,
            aspect,
            cull,
        }
        .run(cam, view, fly_fov, hud_quads, &mut self.fx_ms)
    }

    pub(crate) fn cod2_frame(
        &mut self,
        tick: &Tick,
        hud_quads: &mut Vec<hud::HudQuad>,
    ) -> (renderer::Frame, Option<renderer::VmDraw>) {
        let Tick {
            dt,
            time,
            cull,
            aspect,
            widescreen,
            ..
        } = *tick;
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Cod2(cam) = &mut self.mode else {
            unreachable!()
        };
        self.session.finish_pump();
        self.session.set_fly_keys(&offline_binds::fly(&self.input));
        let mut cod2_vm = None;
        let mut cod2_fx = None;
        let mut cx = session::ClientFrame {
            fs: &self.fs,
            shell: &self.shell,
            localized: &self.localized,
            audio: &mut self.audio,
            fx: &mut self.fx,
            ui: &self.ui,
            r: &mut *r,
            console_open: self.console.open,
            time,
            dt,
        };
        if self.session.pump(&mut cx, cam) {
            let hud_span = frame_prof::span(frame_prof::Stage::Hud);
            (cod2_vm, *hud_quads) = self.session.scene(&mut cx, cam);
            drop(hud_span);
            let events = self.session.events(&mut cx);
            self.session.audio(&mut cx, &events, cam.pos);
            self.session.effects(&mut cx, &events, cam.pos);
            if let Some(out) = self.session.view(&mut cx, &events, cam) {
                self.look_zoom = (
                    out.fov / camera::DEFAULT_FOV_DEG * out.shock.mouse.sensitivity,
                    false,
                );
                cod2_fx = Some(out);
            }
        } else {
            self.look_zoom = (1.0, false);
            self.session.idle();
            cam.update(&offline_binds::fly(&self.input), dt);
        }
        let fly_fov = camera::view_fov_x(camera::DEFAULT_FOV_DEG, aspect, widescreen);
        let view = self.session.draw_view(
            &self.fs,
            r,
            cam,
            cod2_fx.as_ref(),
            cod2_vm,
            (fly_fov, aspect, widescreen),
        );
        frame_prof::count(frame_prof::Count::Particles, self.fx.counts().0);
        ViewTail {
            fs: &self.fs,
            shell: &self.shell,
            audio: &mut self.audio,
            fx: &mut self.fx,
            session: &self.session,
            r,
            time,
            dt,
            aspect,
            cull,
        }
        .run(cam, view, fly_fov, hud_quads, &mut self.fx_ms)
    }

    pub(crate) fn walk_frame(
        &mut self,
        tick: &Tick,
        hud_quads: &mut Vec<hud::HudQuad>,
    ) -> (renderer::Frame, Option<renderer::VmDraw>) {
        let Tick {
            dt,
            time,
            cull,
            aspect,
            ..
        } = *tick;
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Walk {
            world,
            ps,
            input,
            keys,
            view_weapon,
            configstrings,
            weapon_slot,
            reserve,
            switch_to,
            rigs,
            fire_held,
            ads_held,
            ..
        } = &mut self.mode
        else {
            unreachable!()
        };
        // Before this frame's fire event spawns, or the
        // [now-dt, now] integration would move the new tracer
        // ahead of its muzzle.
        let fx_t0 = Instant::now();
        self.fx.step(dt, time, Some(&*world));
        let chained = self.fx.spawn_chained(&self.fs, time);
        frame_digest::clocked(|| self.audio.play_fx(&self.fs, chained));

        if let Some(slot) = switch_to.take()
            && slot != *weapon_slot
            && slot < WALK_LOADOUT.len()
        {
            let name = WALK_LOADOUT[slot];
            let t0 = Instant::now();
            match rigs.load(&self.fs, name, None) {
                Some((models, vw)) => {
                    let t1 = Instant::now();
                    r.set_viewmodel(&self.fs, &models);
                    log::debug!(
                        "switch to {name}: rig {:.2} ms, upload {:.2} ms",
                        (t1 - t0).as_secs_f64() * 1e3,
                        t1.elapsed().as_secs_f64() * 1e3
                    );
                    *reserve = vw.as_ref().map_or(0, |w| w.def.start_ammo);
                    self.viewmodel = models;
                    *view_weapon = vw;
                    *weapon_slot = slot;
                }
                None => log::warn!(
                    "weapon {name} failed to load; keeping {}",
                    WALK_LOADOUT[*weapon_slot]
                ),
            }
        }

        *ads_held = offline_binds::walk(keys, input);
        *fire_held = keys.held(play::input::Action::Attack);
        let mw = MoveWorld::bare(world);
        for ev in pmove::pmove(ps, input, &mw, dt, &[]) {
            let ev = net::events::GameEvent {
                event: ev.event,
                parm: 0,
                // playerState-ring form: no entity, rides the listener
                entity_num: u32::MAX,
                client_num: -1,
                weapon: 0,
                surf_type: 0,
                pos: ps.origin.to_array(),
                dir: [0.0; 3],
                scale: 0,
                other_entity_num: u32::MAX,
                attacker_entity_num: -1,
                e_type: 0,
                eflags: 0,
            };
            // A refused stance moves the kept one, as retail's `cl_stance`.
            if let Some(s) = play::events::forced_stance(&ev, -1, keys.held_stance().is_some()) {
                keys.stance = s;
            }
            self.audio.on_game_event(
                &self.fs,
                &ev,
                configstrings,
                &HashMap::new(),
                &HashMap::new(),
            );
        }
        let v = ps.view();
        let (eye_forward, eye_right, eye_up) = camera::basis(v.yaw, v.pitch);
        self.audio
            .set_listener(v.eye, eye_forward, eye_right, eye_up, u32::MAX);
        let (bone_sets, fov, gun_pose, scope_quads, scoped) =
            self.walk_weapon(tick, &v, eye_forward);
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Walk {
            ps,
            view_weapon,
            configstrings,
            weapon_slot,
            reserve,
            fire_edge,
            reload_edge,
            ..
        } = &mut self.mode
        else {
            unreachable!()
        };
        *fire_edge = false;
        *reload_edge = false;

        *hud_quads = scope_quads;
        self.audio.step(&HashMap::new(), &HashMap::new(), None);
        r.set_fx_quads(
            &self.fs,
            self.fx.build_quads(v.eye, eye_right, eye_up, time),
            time,
        );
        let clouds = self.fx.clouds(v.eye, eye_right, eye_up, time);
        let map = self.session.map_name();
        r.set_fx_clouds(&self.fs, map, &clouds, time);
        r.set_marks(
            &self.fs,
            &self.fx.take_marks(time),
            &[],
            time,
            self.shell.cvar_f32("cg_marks") != 0.0,
        );
        r.set_fx_lights(&self.fx.lights(v.eye, time));
        r.set_fog(
            net::FogParams::parse(
                configstrings
                    .get(net::protocol::CS_FOG_V1)
                    .map(String::as_str)
                    .unwrap_or(""),
            ),
            time as f32,
        );
        self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;
        (
            renderer::Frame {
                view_proj: camera::view_proj_from(v.eye, v.yaw, v.pitch, 0.0, fov, aspect),
                eye: v.eye,
                fwd: camera::basis(v.yaw, v.pitch).0,
                up: camera::up_hint(v.yaw, v.pitch, 0.0),
                time,
                fov_x: fov,
                cull,
                hud_lines: vec![format!(
                    "[{}] {} {} / {}",
                    *weapon_slot + 1,
                    WALK_LOADOUT[*weapon_slot],
                    view_weapon.as_ref().map_or(0, |w| w.state.ammo()),
                    reserve
                )],
            },
            (!scoped).then(|| renderer::VmDraw {
                transform: play::view::pose_transform(gun_pose),
                light_origin: ps.origin + Vec3::Z * ps.view_height(),
                fov_x: fov,
                bone_sets,
            }),
        )
    }

    /// The walk mode's view weapon: its state, pose, sway, scope and cues.
    pub(crate) fn walk_weapon(
        &mut self,
        tick: &Tick,
        v: &pmove::ViewParams,
        eye_forward: Vec3,
    ) -> (
        Vec<Vec<glam::Mat4>>,
        f32,
        hud::scope::GunPose,
        Vec<hud::HudQuad>,
        bool,
    ) {
        let Tick {
            dt,
            time,
            aspect,
            widescreen,
            ..
        } = *tick;
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Walk {
            world,
            ps,
            view_weapon,
            configstrings,
            weapon_slot,
            reserve,
            fire_edge,
            fire_held,
            reload_edge,
            ads_held,
            gun_aim,
            ..
        } = &mut self.mode
        else {
            unreachable!()
        };
        let mut bone_sets = Vec::new();
        let mut fov = camera::view_fov_x(camera::DEFAULT_FOV_DEG, aspect, widescreen);
        let mut gun_pose = hud::scope::GunPose::default();
        let mut scope_quads = Vec::new();
        let mut scoped = false;
        if let Some(w) = view_weapon {
            let out = w.state.update(
                dt,
                weapon::WeaponInput {
                    fire: *fire_edge,
                    fire_held: *fire_held,
                    ads: *ads_held,
                    reload: *reload_edge,
                },
            );
            let frame = w
                .anims
                .get(&out.anim)
                .map_or(0.0, |(anim, _)| anim.frame_pos(out.anim_time, out.looping));
            w.pose(out.ads_frac, *ads_held, out.anim, frame);
            // two models, hands then gun, in set_viewmodel order
            bone_sets = (0..2)
                .map(|m| w.pose.skin_matrices(&w.skeleton, m))
                .collect();
            fov = weapon::view_fov_x(
                cg_fov(&self.shell),
                Some(&w.def),
                out.ads_frac,
                *ads_held,
                false,
                false,
            );
            fov = camera::view_fov_x(fov, aspect, widescreen);
            // The walk sight runs outside pmove, so the gun's
            // sway reads its fraction off a copy.
            let mut sighted = **ps;
            sighted.weapon_pos_frac = out.ads_frac;
            gun_pose = gun_aim
                .step(
                    Some(&w.def),
                    &sighted,
                    (time * 1000.0) as i32,
                    Default::default(),
                )
                .unwrap_or_default();
            if hud::scope::overlay_frac(&w.def, out.ads_frac, *ads_held).is_some() {
                let screen = r.screen_size();
                let fovs = (fov, camera::fov_y(fov, aspect));
                let at = hud::scope::gun_point(gun_pose.angles, fovs, screen);
                let layout = hud::scope::Layout::of(net::protocol::install_protocol(&self.fs));
                hud::scope::build(&w.def, layout, at, screen, &mut scope_quads);
                // The gun hides under it, as online.
                scoped = true;
            }
            if let Some(cue) = out.cue {
                if matches!(
                    cue,
                    weapon::WeaponCue::Reload | weapon::WeaponCue::ReloadFromEmpty
                ) {
                    let need = w.def.clip_size.saturating_sub(w.state.ammo());
                    *reserve -= need.min(*reserve);
                }
                self.audio.on_game_event(
                    &self.fs,
                    &net::events::GameEvent {
                        event: match cue {
                            weapon::WeaponCue::Fire => fx::registry::EV_FIRE_WEAPON,
                            weapon::WeaponCue::LastShot => fx::registry::EV_FIRE_WEAPON_LASTSHOT,
                            weapon::WeaponCue::Rechamber => fx::registry::EV_RECHAMBER_WEAPON,
                            weapon::WeaponCue::Reload => fx::registry::EV_RELOAD,
                            weapon::WeaponCue::ReloadFromEmpty => {
                                fx::registry::EV_RELOAD_FROM_EMPTY
                            }
                            weapon::WeaponCue::Raise => fx::registry::EV_RAISE_WEAPON,
                        },
                        parm: 0,
                        entity_num: u32::MAX,
                        client_num: -1,
                        // 1-based CS7 index of the active loadout slot
                        weapon: *weapon_slot as i32 + 1,
                        surf_type: 0,
                        pos: ps.origin.to_array(),
                        dir: [0.0; 3],
                        scale: 0,
                        other_entity_num: u32::MAX,
                        attacker_entity_num: -1,
                        e_type: 0,
                        eflags: 0,
                    },
                    configstrings,
                    &HashMap::new(),
                    &HashMap::new(),
                );
            }
            if out.fired {
                /// Q3/CoD bullet trace length.
                const FIRE_RANGE: f32 = 8192.0;
                // a point trace against solids only: playerclip-only
                // geometry stops movement but not bullets
                let tr = world.shot_trace(v.eye, v.eye + eye_forward * FIRE_RANGE);
                // A muzzle inside solid returns no normal, which
                // would build a NaN quad that never leaves the ring.
                if tr.fraction < 1.0 && !tr.startsolid {
                    // Same csv resolution as a server-driven hit:
                    // surfType rides the surface flags' bits 20-24.
                    let mut muzzles = HashMap::new();
                    muzzles.insert(
                        u32::MAX,
                        (v.eye + eye_forward * 16.0 - Vec3::Z * 2.0, eye_forward),
                    );
                    let ctx = fx::registry::ResolveCtx {
                        brass: &HashMap::new(),
                        weapons: &[],
                        eject_brass: false,
                        configstrings: &[],
                        cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
                        blood: true,
                        muzzles: &muzzles,
                        shot_fallbacks: &HashMap::new(),
                        weapon_flash: &HashMap::new(),
                        view_flash: None,
                        view_body: None,
                    };
                    let ev = net::events::GameEvent {
                        event: fx::registry::EV_BULLET_HIT_SMALL,
                        parm: net::events::dir_to_byte(tr.normal.to_array()),
                        entity_num: u32::MAX,
                        client_num: -1,
                        weapon: *weapon_slot as i32 + 1,
                        surf_type: collision::sound_material(tr.surface_flags),
                        pos: tr.endpos.to_array(),
                        dir: [0.0; 3],
                        scale: 0,
                        other_entity_num: u32::MAX,
                        attacker_entity_num: -1,
                        e_type: 0,
                        eflags: 0,
                    };
                    for r in fx::registry::resolve(&ev, &ctx) {
                        match r {
                            fx::registry::Resolved::Spawn { path, at, .. } => {
                                let sounds = self.fx.spawn(&self.fs, &path, at, time);
                                self.audio.play_fx(&self.fs, sounds);
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
                            _ => {}
                        }
                    }
                }
            }
        }
        (bone_sets, fov, gun_pose, scope_quads, scoped)
    }

    /// The 2D layer over the mode's: the demo line, the menus, the console.
    pub(crate) fn frame_ui(&mut self, tick: &Tick, mut hud_quads: Vec<hud::HudQuad>) {
        let Tick {
            now, dt, elapsed, ..
        } = *tick;
        // Under the frame digest the menus' pulses and edit cursor run on the
        // drawn serverTime, which a replay repeats.
        let digest_clock = frame_digest::on().then(|| self.digest_time().unwrap_or(0));
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let ui_span = frame_prof::span(frame_prof::Stage::Ui);
        // Over the game, under the menus (CoDMP.exe 0x416b10, drawn
        // in `CA_ACTIVE` with no fullscreen menu up, cod11-demos.md 8).
        if let Mode::Online {
            net,
            phase: Phase::Live(_),
            ..
        } = &self.mode
            && !self.ui.fullscreen()
            && let Some((name, bytes)) = net.recording_status()
        {
            let v1 = net.protocol().is_v1();
            let show_name = v1 || self.shell.cvar_f32("cg_showdemoname") as i32 == 1;
            let text = console::recording_text(name, bytes, show_name);
            let (w, h) = r.screen_size();
            hud_quads.extend(self.console.recording_line(&text, v1, w, h));
        }
        // CoD2's `SCR_UpdateFrame` (Mac 0x172d88): the name always,
        // white at (5, 479) on the 640x480 grid as 1.5's. 1.7
        // draws it in `consoleFont`, placed `LEFT` (cod4-demos.md 9).
        if !self.ui.fullscreen()
            && let Some((name, bytes)) = self.session.recording_status()
        {
            let text = console::recording_text(name, bytes, true);
            let (w, h) = r.screen_size();
            hud_quads.extend(match self.session {
                session::Session::Cod4(_) => self.console.recording_line_cod4(&text, w, h),
                _ => self.console.recording_line(&text, false, w, h),
            });
        }
        // The CoD2 connect screen: the address and the download.
        for (i, line) in self
            .session
            .connect_lines(&self.localized)
            .iter()
            .enumerate()
        {
            let (w, h) = r.screen_size();
            let y = 240.0 + 20.0 * i as f32;
            hud_quads.extend(self.console.screen_text(line, 24.0, y, w, h));
        }
        if self.ui.active() {
            let lists = self.ui.player_list_shown(&self.shell);
            // CoD2's mute list (`muteplayer`): the rows on the
            // player list's clock, the mute bits live.
            if lists && let Some(c) = self.session.cod2() {
                let rows = c.mute_rows();
                let muted = rows
                    .iter()
                    .filter(|r| r.2 && r.0 < 64)
                    .fold(0u64, |m, r| m | 1 << r.0);
                self.ui.set_muted(muted);
                self.ui.offer_clients(now, || {
                    rows.into_iter().map(|(n, name, _)| (n, name)).collect()
                });
            }
            if lists && let Mode::Online { net, .. } = &self.mode {
                self.ui.offer_players(now, || {
                    let protocol = net.protocol();
                    net.snapshots()
                        .newest()
                        .map(|s| s.clients.values().map(|c| c.name(protocol)).collect())
                        .unwrap_or_default()
                });
            }
            self.ui.frame(now, &self.shell);
            self.ui.set_digest_clock(digest_clock);
            let dated = self.ui.date_refreshes(&self.localized);
            self.ui_pending.extend(dated);
            let (w, h) = r.screen_size();
            self.ui.set_screen(w, h);
            hud_quads.extend(self.ui.build(w, h, &self.localized, &self.shell));
        }
        hud_quads.extend(self.session.cod4_front_quads(r.screen_size()));
        self.console.drain_log();
        self.console.update(
            dt * 1000.0,
            self.shell.cvar_f32("scr_conspeed"),
            matches!(self.mode, Mode::Idle)
                && !self.ui.active()
                && !self.session.cod4_front_active()
                && self.cod4_loading.is_none(),
        );
        if self.console.visible() {
            let (w, h) = r.screen_size();
            hud_quads.extend(self.console.build(w, h, elapsed.as_millis() as u64));
        }
        frame_digest::hud(&hud_quads);
        r.set_hud_quads(&self.fs, hud_quads);
        drop(ui_span);
    }
}

/// One redraw's clock and the cvars each mode's frame reads.
#[derive(Clone, Copy)]
pub(crate) struct Tick {
    pub now: Instant,
    pub dt: f32,
    pub elapsed: std::time::Duration,
    pub time: f64,
    pub local_ms: f64,
    pub cull: renderer::CullMode,
    pub blood: bool,
    pub eject_brass: bool,
    pub aspect: f32,
    pub widescreen: bool,
}
