//! A CoD2 session's client state, `App`'s `Session::Cod2`: the session
//! (or its load-time pump), the connect in flight, and what is drawn or
//! read off it, with the frame's steps over them. Field order is drop
//! order and matches the fields' old order in `App`.

use anyhow::{Result, bail};
use glam::Vec3;
use vcod_common::localize::Localized;
use vcod_common::pk3::Pk3Fs;

use crate::audio::AudioSystem;
use crate::camera::{self, FlyCamera};
use crate::console::{
    self,
    shell::{Effect, Shell},
};
use crate::frontend::Ui;
use crate::fx::sim::FxSystem;
use crate::play::cod2_events::Event2;
use crate::renderer::{self, Renderer};
use crate::{cod2_hud, cod2_scene, cod2_viewmodel, cod2_world_hud, hud, play};

pub(crate) struct Cod2Client {
    /// A CoD2 server this fly-mode camera spectates or plays on (`play::cod2`, whose
    /// module doc is the interface for drawing its entities).
    pub(crate) session: Option<Box<play::cod2::Cod2Session>>,
    /// The session while it is read on its own thread through the startup
    /// load; the first redraw (or a renderer build) takes it back.
    pub(crate) pump: Option<play::cod2::Pump>,
    /// The CoD2 session's entities and bodies.
    pub(crate) draw: cod2_scene::Cod2Draw,
    /// The CoD2 view weapon of the followed or own body.
    pub(crate) view: cod2_viewmodel::Cod2View,
    /// A CoD2 session's view effects: damage kick, scope sway, shake,
    /// shellshock, the sight's zoom (`play::cod2_view`).
    pub(crate) view_fx: play::cod2_view::ViewFx,
    /// The shellshock's turn caps on a CoD2 session's mouse.
    pub(crate) turn: play::cod2_shock::TurnCap,
    /// A CoD2 session's hudelems and reticle.
    pub(crate) hud: cod2_hud::Cod2Hud,
    /// A front-end `connect` to a CoD2 server still running (`cod2_front`).
    pub(crate) connecting: Option<play::cod2::Connecting>,
    /// The CoD2 session's script menus and server cvars, the player's
    /// side (the session answers `--team`'s itself), and the gamestate
    /// count their menus were loaded under.
    pub(crate) menus: play::join::Join,
    pub(crate) menus_for: u32,
    /// The gamestate count the level's models were loaded under.
    pub(crate) models_for: u32,
    /// Voice chat's codec, microphone and talker streams, made on the
    /// first live frame (`crate::voice`).
    voice: Option<crate::voice::VoiceChat>,
}

impl Cod2Client {
    /// No session yet; `pump` is the startup `--connect`'s, if any.
    pub(crate) fn new(pump: Option<play::cod2::Pump>) -> Self {
        Self {
            session: None,
            pump,
            draw: cod2_scene::Cod2Draw::new(),
            view: cod2_viewmodel::Cod2View::default(),
            view_fx: play::cod2_view::ViewFx::default(),
            turn: Default::default(),
            hud: cod2_hud::Cod2Hud::default(),
            connecting: None,
            menus: play::join::Join::new(None, None),
            menus_for: 0,
            models_for: 0,
            voice: None,
        }
    }
}

/// What a [`Cod2Client`] frame step borrows from `App`, built at the call
/// site so the client and the rest of `App` borrow apart.
pub(crate) struct Cod2Frame<'a> {
    pub(crate) fs: &'a Pk3Fs,
    pub(crate) shell: &'a Shell,
    pub(crate) localized: &'a Localized,
    pub(crate) audio: &'a mut AudioSystem,
    pub(crate) fx: &'a mut FxSystem,
    pub(crate) ui: &'a Ui,
    pub(crate) r: &'a mut Renderer,
    pub(crate) console_open: bool,
    /// Seconds since the client started, the effects' clock.
    pub(crate) time: f32,
    pub(crate) dt: f32,
}

/// The fly camera's draw for a frame: the view effects' (or predicted)
/// angles, the world's fov and the view weapon.
pub(crate) struct DrawView {
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) roll: f32,
    pub(crate) fov: f32,
    pub(crate) vm: Option<renderer::VmDraw>,
}

impl Cod2Client {
    /// The startup or renderer-build pump hands the session back; first
    /// in every frame that reads the session.
    pub(crate) fn finish_pump(&mut self) {
        if let Some(p) = self.pump.take() {
            self.session = Some(p.finish());
        }
    }

    /// The session's frame read off the wire: the camera flies or follows,
    /// a restart clears the effects, scores and chat land in the HUD and
    /// the console. False with no session (the caller flies freely).
    pub(crate) fn pump(&mut self, cx: &mut Cod2Frame, cam: &mut FlyCamera) -> bool {
        let Some(s) = self.session.as_deref_mut() else {
            return false;
        };
        let _prof = crate::frame_prof::span(crate::frame_prof::Stage::Net);
        s.talking = cx.console_open || cx.ui.active() || self.hud.chat_field.is_some();
        s.set_freeze_demo(cx.shell.cvar_f32("cl_freezeDemo") != 0.0);
        s.fly(cam, cx.fs);
        self.voice_frame(cx);
        let Some(s) = self.session.as_deref_mut() else {
            return false;
        };
        if s.take_restart() {
            cx.fx.clear();
        }
        if let Some(b) = s.take_scores() {
            self.hud.scoreboard.on_server_command(&b);
        }
        let chat_ms = s.frame().map_or(0, |f| f.time_ms);
        for (_, line) in s.take_chat() {
            let line = cx.localized.message(&line);
            console::log::print(&line);
            self.hud.chat.push(&line, chat_ms);
        }
        // `score` again every 2 s while Tab is held.
        if self.hud.scoreboard.due(cx.time) {
            s.send_command("score");
            self.hud.scoreboard.mark_requested(cx.time);
        }
        // `CL_GetMenuBlurRadius`: none on a demo
        cx.r.set_world_blur(if s.is_demo() { 0.0 } else { cx.ui.blur_world() });
        true
    }

    /// Voice chat's frame (cod2-voice.md 4): `+talk`'s frames encoded onto
    /// the session's batch while [`crate::voice::may_send`] holds, the
    /// batch sent on `CL_VoiceTransmit`'s clock, the heard ones played.
    /// `cl_voice` goes to the server in the userinfo when it changes.
    fn voice_frame(&mut self, cx: &mut Cod2Frame) {
        use crate::voice::{VoiceChat, VoiceFrame, may_send};
        let Some(s) = self.session.as_deref_mut() else {
            return;
        };
        if s.is_demo() {
            return;
        }
        let (sv_voice, quality) = s.voice_settings();
        let cl_voice = cx.shell.cvar_f32("cl_voice") != 0.0;
        // `Voice_SendVoiceData` reads `rate` with `Dvar_GetInt`.
        let rate = vcod_common::text::atoi(cx.shell.cvar("rate").unwrap_or("0"));
        let active = s.frame().is_some();
        let held = s.input.held(play::input::Action::Talk);
        s.set_cl_voice(cl_voice);
        let f = VoiceFrame {
            sending: may_send(held, active, sv_voice, cl_voice, rate),
            quality,
        };
        self.voice
            .get_or_insert_with(|| VoiceChat::new(vcod_cod2::net::voice::SAMPLE_RATE))
            .frame(&mut s.voice, &f, Some(&mut *cx.audio));
        s.transmit_voice();
        self.hud.voice = cod2_hud::VoiceShown {
            own: crate::voice::cod2_own_talking(held, sv_voice, cl_voice),
            talkers: s.talkers(),
            muted: s
                .muted
                .iter()
                .enumerate()
                .fold(0, |m, (n, &on)| m | u64::from(on) << n),
        };
    }

    /// The mute list's `uiScript mutePlayer` (`CL_MutePlayer`): flips
    /// `client`'s mute.
    pub(crate) fn toggle_mute(&mut self, client: u32) {
        if let Some(s) = self.session.as_deref_mut() {
            s.toggle_mute(client);
        }
    }

    /// The mute list's rows (`UI_BuildPlayerList`, Mac 0x14f5e2): every
    /// client the newest snapshot names, by number, and whether each is
    /// muted.
    pub(crate) fn mute_rows(&self) -> Vec<(u32, String, bool)> {
        let Some(s) = self.session.as_deref() else {
            return Vec::new();
        };
        let Some(f) = s.frame() else {
            return Vec::new();
        };
        let mut rows: Vec<(u32, String, bool)> = f
            .snap
            .clients
            .iter()
            .map(|c| {
                let muted = s.muted.get(c.number as usize).copied().unwrap_or(false);
                (c.number, c.name(), muted)
            })
            .collect();
        rows.sort_by_key(|r| r.0);
        rows
    }

    /// The session's frame drawn: its entities and bodies into the
    /// renderer, the view weapon's frame, the HUD's fields off the session
    /// and the console and its quads, then a follower's camera onto the
    /// followed view (a demo draws its recorder as a follower is drawn).
    /// One `frame()` for all of it, as its time can move between reads.
    pub(crate) fn scene(
        &mut self,
        cx: &mut Cod2Frame,
        cam: &mut FlyCamera,
    ) -> (Option<cod2_viewmodel::ViewFrame>, Vec<hud::HudQuad>) {
        let Some(s) = self.session.as_deref() else {
            return (None, Vec::new());
        };
        let Some(f) = s.frame() else {
            return (None, Vec::new());
        };
        if self.models_for != s.gamestates() {
            self.models_for = s.gamestates();
            let mut laps = vcod_common::laps::Laps::new("cod2 level models");
            let n = self.draw.preload(f.configstrings, cx.fs, cx.r);
            laps.lap("models");
            let views = cod2_viewmodel::level_view_models(f.configstrings, cx.fs);
            cx.r.preload_viewmodel(cx.fs, &views);
            laps.lap("view models");
            laps.finish();
            log::debug!("cod2: {n} level models, {} view models loaded", views.len());
        }
        self.draw.draw(&f, cx.fs, cx.r);
        let vm_prof = crate::frame_prof::span(crate::frame_prof::Stage::Vm);
        let vm = self.view.frame(&f, cx.fs, s.predicting());
        drop(vm_prof);
        let shell = cx.shell;
        self.hud.selected = s.input.weapons().0;
        self.hud.interrupted = s.interrupted();
        self.hud.lagometer = shell
            .cvar("cg_drawLagometer")
            .is_some_and(|v| v.trim().parse::<f32>().is_ok_and(|v| v != 0.0))
            .then(|| s.lagometer.clone());
        self.hud.mantle_hint = shell
            .cvar("cg_drawMantleHint")
            .is_none_or(|v| v.trim().parse::<f32>() != Ok(0.0));
        self.hud.use_key = shell
            .key_text("+activate", cx.localized)
            .unwrap_or_default();
        let key = |cmd: &str| shell.key_text(cmd, cx.localized).unwrap_or_default();
        let or = |a: String, b: &str| if a.is_empty() { key(b) } else { a };
        self.hud.follow_keys = [
            key("+attack"),
            or(key("toggleads"), "+speed"),
            or(key("+melee"), "+melee_breath"),
        ];
        self.hud.crosshair_hit = s.crosshair_entity();
        let int = |name: &str, default: i32| {
            shell
                .cvar(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .map_or(default, |v| v as i32)
        };
        self.hud.crosshair_names = (
            int("cg_drawCrosshairNames", 1) != 0,
            int("cg_drawCrosshairNamesPosX", 300),
            int("cg_drawCrosshairNamesPosY", 180),
        );
        self.hud.dvars =
            cod2_hud::HudDvars::read(|c| shell.cvar(c), |c| shell.key_text(c, cx.localized));
        self.hud.objective = shell
            .cvar("cg_objectiveText")
            .unwrap_or_default()
            .to_string();
        let quads = self.hud.build(
            &f,
            cx.fs,
            cx.localized,
            cx.r.screen_size(),
            f.time_ms,
            cx.time,
        );
        if f.snap.ps.followed().is_some() || s.is_demo() {
            let [pitch, yaw, _] = cod2_viewmodel::view_angles(&f);
            cam.pitch = -pitch.to_radians();
            cam.yaw = yaw.to_radians();
        }
        (vm, quads)
    }

    /// The frame's one list of events for the sounds and the effects,
    /// after the recoil and the HUD read it.
    pub(crate) fn events(&mut self, cx: &mut Cod2Frame) -> Vec<Event2> {
        let Some(s) = self.session.as_deref_mut() else {
            return Vec::new();
        };
        let _prof = crate::frame_prof::span(crate::frame_prof::Stage::Events);
        let events = s.take_events();
        s.recoil_frame(&events, cx.fs);
        if let Some(f) = s.frame() {
            let now = f.time_ms;
            self.hud.on_events(&events, &f, cx.fs, cx.localized, now);
        }
        events
    }

    /// The events' sound cues, subtitles, the ambient, blends, the
    /// server's sound commands and the loops.
    pub(crate) fn audio(&mut self, cx: &mut Cod2Frame, events: &[Event2], cam_pos: Vec3) {
        let Some(s) = self.session.as_deref_mut() else {
            return;
        };
        let _prof = crate::frame_prof::span(crate::frame_prof::Stage::Events);
        let audio = &mut *cx.audio;
        let (cues, (cs3, ambient, fade)) =
            s.sound_cues(events, cx.fs, cam_pos, self.draw.muzzles());
        for c in cues {
            audio.play(cx.fs, c);
        }
        let hud_now = s.frame().map_or(0, |f| f.time_ms);
        for (key, ms) in audio.take_subtitles() {
            // `CL_SubtitlePrint`: the key itself when
            // `subtitle.str` lacks it.
            let text = cx.localized.get(&key).unwrap_or(&key).to_string();
            self.hud.subtitles.push_for(&text, hud_now, ms);
        }
        audio.set_cod2_ambient(cx.fs, &cs3, ambient.as_deref(), fade);
        let blends = s.sound_blends();
        audio.set_sound_blends(cx.fs, &blends);
        for c in s.take_sound_commands() {
            use play::cod2_sound::SoundCommand as Sc;
            match c {
                Sc::Local(a) => {
                    audio.play_local(cx.fs, &a);
                }
                Sc::Music(a) => audio.music_play(cx.fs, &a),
                Sc::MusicStop(ms) => audio.music_stop(ms),
                Sc::Fade(v, ms) => audio.sound_fade(v, ms),
                Sc::Reverb {
                    prio,
                    room,
                    dry,
                    wet,
                    fade_ms,
                } => audio.set_environment(prio, &room, dry, wet, fade_ms),
                Sc::ReverbOff { prio, fade_ms } => audio.deactivate_environment(prio, fade_ms),
            }
        }
        let (loops, missiles) = s.loop_sounds(cx.fs);
        audio.set_loop_aliases(cx.fs, &loops);
        audio.set_projectile_aliases(cx.fs, &missiles);
    }

    /// The events' effects spawned at the view.
    pub(crate) fn effects(&mut self, cx: &mut Cod2Frame, events: &[Event2], cam_pos: Vec3) {
        let Some(s) = self.session.as_deref_mut() else {
            return;
        };
        let _prof = crate::frame_prof::span(crate::frame_prof::Stage::Fx);
        cx.fx.set_view(cam_pos);
        // The level's effect files, sprite images and mark materials build
        // with the gamestate, not in an effect's or a surface's first
        // frame (cod2-client-perf.md, "The soak").
        if let Some(paths) = s.take_level_effects(cx.fs) {
            let t0 = std::time::Instant::now();
            let (marks, sprites) = cx.fx.preload(cx.fs, paths.iter().map(String::as_str));
            log::debug!(
                "cod2: {} level effects, {} mark materials, {} sprite materials built",
                paths.len(),
                marks.len(),
                sprites.len()
            );
            cx.r.prewarm_marks(cx.fs, marks.iter().map(|m| &**m));
            let tracer = crate::fx::cod2_tracer::MATERIAL;
            cx.r.prewarm_fx(cx.fs, sprites.iter().map(|m| &**m).chain([tracer]));
            vcod_common::laps::span(t0, || "cod2 level effects".into());
        }
        for play in s.effects(events, cx.fs, cam_pos, self.draw.muzzles()) {
            play.spawn(cx.fx, cx.fs, cx.time);
        }
    }

    /// `CG_CalcViewValues`' angles, shake, shellshock and fov over the
    /// predicted (or followed) view, with the shock's sound, turn caps,
    /// breath cues and screen blend. None without a frame.
    pub(crate) fn view(
        &mut self,
        cx: &mut Cod2Frame,
        events: &[Event2],
        cam: &FlyCamera,
    ) -> Option<play::cod2_view::ViewOut> {
        let s = self.session.as_deref_mut()?;
        let _prof = crate::frame_prof::span(crate::frame_prof::Stage::Scene);
        let f = s.frame()?;
        let dt = cx.dt;
        let audio = &mut *cx.audio;
        let kick = self.hud.damage_kick();
        let (view, first) = match s.view() {
            Some(v) => (v, true),
            None => (
                (
                    cam.pos,
                    [-cam.pitch.to_degrees(), cam.yaw.to_degrees(), 0.0],
                ),
                f.snap.ps.followed().is_some(),
            ),
        };
        let out = self.view_fx.frame(
            &f,
            cx.fs,
            events,
            &kick,
            view,
            first,
            camera::DEFAULT_FOV_DEG,
            (f.time_ms, (dt * 1000.0) as i32),
            s.gun_kick(),
            s.predicting(),
            s.land_gun(),
        );
        s.input.shellshock = out.shock.slowed;
        audio.set_shellshock(cx.fs, &out.shock.sound);
        self.turn.set(
            dt * 1000.0,
            [out.shock.mouse.max_pitch, out.shock.mouse.max_yaw],
        );
        for alias in &out.breath.cues {
            let v = audio.play_local(cx.fs, alias);
            if *alias == "weap_sniper_breathin" {
                let ms = v.and_then(|v| audio.voice_ms(v));
                self.view_fx.breath_in_ms(ms.unwrap_or(0));
            }
        }
        if out.breath.heartbeat {
            let pos = cam.pos;
            audio.hold(
                cx.fs,
                crate::audio::BREATH_ENTITY,
                "weap_sniper_heartbeat",
                pos,
            );
        } else {
            audio.release(crate::audio::BREATH_ENTITY);
        }
        cx.r.set_screen_blend(out.shock.blend_ms);
        Some(out)
    }

    /// The mouse's turn in degrees under the shellshock's caps.
    pub(crate) fn turn(&mut self, degrees: [f32; 2]) -> [f32; 2] {
        self.turn.turn(degrees)
    }

    /// A level change drops the view effects.
    pub(crate) fn reset_view_fx(&mut self) {
        self.view_fx.reset();
    }

    /// No session: the shellshock's turn caps go.
    pub(crate) fn idle(&mut self) {
        self.turn = Default::default();
    }

    /// The draw's angles and fov: the view effects' when they ran, else
    /// the predicted view's (the pitch cap included; `cam` keeps the
    /// input's), else the camera's at `fly_fov`. The view weapon draws at
    /// the zoomed fov, as the scene's one view parms carry it
    /// (`R_SetViewParmsForScene`, Mac 0xc5a9a, cod2-hud.md 18.1), and hides
    /// under a scope overlay (`CG_AddViewWeapon`, Mac 0x1da2f8).
    pub(crate) fn draw_view(
        &self,
        fs: &Pk3Fs,
        r: &mut Renderer,
        cam: &FlyCamera,
        fx: Option<&play::cod2_view::ViewOut>,
        mut vm: Option<cod2_viewmodel::ViewFrame>,
        (fly_fov, aspect, widescreen): (f32, f32, bool),
    ) -> DrawView {
        let (draw_yaw, draw_pitch) = self
            .session
            .as_deref()
            .and_then(|s| s.view())
            .map_or((cam.yaw, cam.pitch), |(_, [p, y, _])| {
                (y.to_radians(), -p.to_radians())
            });
        let (yaw, pitch, roll, fov) = match fx {
            Some(o) => (
                o.angles[1].to_radians(),
                -o.angles[0].to_radians(),
                o.angles[2].to_radians(),
                camera::view_fov_x(o.fov, aspect, widescreen),
            ),
            None => (draw_yaw, draw_pitch, 0.0, fly_fov),
        };
        if fx.is_some_and(|o| o.overlay.is_some()) {
            vm = None;
        }
        let vm = vm.map(|v| {
            if let Some(m) = &v.models {
                r.set_viewmodel(fs, m);
            }
            // The gun's sway, bob and lag off the view
            // (`play::cod2_gun`).
            let transform = match fx.and_then(|o| o.gun) {
                Some(g) => play::view::gun_transform(
                    play::view::view_from_ful(Vec3::from(g.offset)),
                    g.angles,
                ),
                None => v.draw.transform,
            };
            renderer::VmDraw {
                fov_x: fov,
                transform,
                ..v.draw
            }
        });
        DrawView {
            yaw,
            pitch,
            roll,
            fov,
            vm,
        }
    }

    /// The session's waypoints and head icons, put under `quads` (the 2D
    /// HUD).
    pub(crate) fn world_hud(
        &self,
        quads: &mut Vec<hud::HudQuad>,
        view_proj: glam::Mat4,
        (yaw, pitch, roll): (f32, f32, f32),
        size: (f32, f32),
    ) {
        let Some(f) = self.session.as_deref().and_then(|s| s.frame()) else {
            return;
        };
        let up = camera::up_hint(yaw, pitch, roll);
        let fwd = camera::basis(yaw, pitch).0;
        let right = fwd.cross(up).normalize();
        let view = cod2_world_hud::WorldView {
            view_proj,
            right,
            up: right.cross(fwd),
            size,
        };
        let mut q = cod2_world_hud::build(&f, self.draw.heads(), &view, f.time_ms);
        q.append(quads);
        *quads = q;
    }

    /// The session's map, which CoD2's snow tests the roofs of.
    pub(crate) fn map_name(&self) -> Option<&str> {
        self.session.as_deref().and_then(|s| s.map_name())
    }
}

/// Outside the frame: input, console commands, menus, the renderer's
/// builds and the quit.
impl Cod2Client {
    /// Whether a session is up (a connection or a demo).
    pub(crate) fn live(&self) -> bool {
        self.session.is_some()
    }

    /// A front-end connect is still running.
    pub(crate) fn busy(&self) -> bool {
        self.connecting.is_some()
    }

    pub(crate) fn playing_demo(&self) -> bool {
        self.session.as_deref().is_some_and(|s| s.is_demo())
    }

    /// The quit: the session back off its pump, then `disconnect`.
    pub(crate) fn shutdown(&mut self) {
        self.finish_pump();
        if let Some(s) = &mut self.session {
            s.disconnect();
        }
    }

    /// The session onto its pump while the renderer builds; the build's
    /// end hands it back through [`Self::finish_pump`].
    pub(crate) fn park(&mut self) {
        self.pump = self
            .pump
            .take()
            .or_else(|| self.session.take().map(|s| s.pump()));
    }

    /// The draw's model handles died with the old renderer.
    pub(crate) fn reset_render(&mut self) {
        self.draw = cod2_scene::Cod2Draw::new();
        self.view = cod2_viewmodel::Cod2View::default();
        self.models_for = 0;
    }

    /// No key stays held in the session's input.
    pub(crate) fn release_keys(&mut self) {
        if let Some(s) = self.session.as_deref_mut() {
            s.input.release_all();
        }
    }

    pub(crate) fn hide_scoreboard(&mut self) {
        self.hud.scoreboard.hide();
    }

    /// The session's scoreboard, which takes the wheel and page keys
    /// while held.
    pub(crate) fn scoreboard_mut(&mut self) -> Option<&mut crate::hud::scoreboard::Scoreboard> {
        self.session.as_ref()?;
        Some(&mut self.hud.scoreboard)
    }

    /// The chat field's keys while a session is up: Enter sends
    /// `say`/`say_team`, Escape drops the line. None with no session;
    /// false when the key is not the field's.
    pub(crate) fn chat_key(
        &mut self,
        code: winit::keyboard::KeyCode,
        text: Option<&str>,
    ) -> Option<bool> {
        let s = self.session.as_deref_mut()?;
        let Some(field) = &mut self.hud.chat_field else {
            return Some(false);
        };
        if let Some(line) = crate::chat_field_key(field, code, text) {
            if let Some(cmd) = line.and_then(|l| play::cod2::chat_command(field.team, &l)) {
                s.send_command(&cmd);
            }
            self.hud.chat_field = None;
        }
        Some(true)
    }

    /// A script menu's response to the server. False with no session.
    pub(crate) fn menu_response(&mut self, menu: &str, response: &str) -> bool {
        let Some(s) = self.session.as_deref_mut() else {
            return false;
        };
        s.menu_response(menu, response);
        true
    }

    /// A console effect the session takes before `App::apply` sees it: the
    /// gameplay binds (`play::cod2_input`), the board's cycle, `score`,
    /// forwarded lines, the chat field, script menus. True when taken.
    pub(crate) fn apply(
        &mut self,
        effect: &Effect,
        shell: &Shell,
        start: std::time::Instant,
    ) -> bool {
        let Some(s) = self.session.as_deref_mut() else {
            return false;
        };
        // A held board takes `weapnext` (up) and `weapprev` (down)
        // first (`CG_NextWeapon_f`, `CG_PrevWeapon_f`, Mac 0x1dc352,
        // 0x1dc2d8).
        if let Effect::Impulse(
            a @ (play::input::Action::NextWeapon | play::input::Action::PrevWeapon),
        ) = effect
        {
            let step = shell.cvar_f32("cg_scoreboardScrollStep") as i32;
            let next = *a == play::input::Action::NextWeapon;
            if self.hud.scoreboard.take_cycle(next, step) {
                return true;
            }
        }
        let ps = s.frame().map(|f| f.snap.ps.raw.clone());
        if s.input.effect(effect, ps.as_ref()) {
            return true;
        }
        match effect {
            // `score` on the down edge, again every 2 s while held
            // (the Cod2 arm), as CoD1's board does.
            Effect::Scores(down) => {
                let now = (std::time::Instant::now() - start).as_secs_f32();
                let board = &mut self.hud.scoreboard;
                if !*down {
                    board.release(now);
                } else if board.press(now) {
                    s.send_command("score");
                }
                true
            }
            Effect::Forward(line) => {
                s.send_command(line);
                true
            }
            Effect::MessageMode { team } => {
                s.input.release_all();
                self.hud.chat_field = Some(hud::ChatField {
                    team: *team,
                    text: String::new(),
                });
                true
            }
            Effect::OpenScriptMenu { menu, response } => {
                s.menu_response(menu, response);
                true
            }
            // `CL_ForwardCommandToServer`, as `App::apply`'s.
            Effect::Unknown { word, line } => {
                if !word.starts_with('-') && !word.starts_with('+') {
                    s.send_command(line);
                } else if !word.starts_with('-') {
                    console::log::print(&format!("Unknown command \"{word}\""));
                }
                true
            }
            _ => false,
        }
    }

    /// `record`'s reply, None with no session.
    pub(crate) fn record(&mut self, dir: &std::path::Path, name: Option<&str>) -> Option<String> {
        Some(self.session.as_deref_mut()?.start_recording(dir, name))
    }

    /// `stoprecord`'s reply, None with no session.
    pub(crate) fn stop_record(&mut self) -> Option<String> {
        Some(self.session.as_deref_mut()?.stop_recording())
    }

    /// `cg_shellshock`'s arguments for the view effects; handed back with
    /// no session.
    pub(crate) fn shellshock(&mut self, args: Vec<String>) -> Result<(), Vec<String>> {
        if self.session.is_none() {
            return Err(args);
        }
        self.view_fx.shock_command = Some(args);
        Ok(())
    }

    pub(crate) fn set_userinfo(&mut self, info: &vcod_common::net::Userinfo) {
        if let Some(s) = self.session.as_deref_mut() {
            s.set_userinfo(info);
        }
    }

    /// The session's drop or level change, before the frame: the drop's
    /// bare reason (which the error popup localizes, `EXE_*`), or the map
    /// a new gamestate names.
    pub(crate) fn take_level_change(&mut self) -> Result<Option<String>> {
        let Some(s) = self.session.as_deref_mut() else {
            return Ok(None);
        };
        if let Some(why) = s.dropped.take() {
            log::error!("cod2: the connection ended: {why}");
            bail!("{why}");
        }
        Ok(s.take_map_change())
    }

    /// A demo played out (`CL_DemoCompleted`, Mac 0x14c096): its
    /// `timedemo` line, if any.
    pub(crate) fn demo_over(&self) -> Option<Option<String>> {
        let s = self.session.as_deref()?;
        s.demo_over().then(|| s.timedemo_report())
    }

    /// The recording's name and size while a frame is up
    /// (`SCR_UpdateFrame`, Mac 0x172d88).
    pub(crate) fn recording_status(&self) -> Option<(&str, u64)> {
        let s = self.session.as_deref()?;
        s.frame()?;
        s.recording_status()
    }

    /// The connect screen's lines (`cod2_front::connect_lines`).
    pub(crate) fn connect_lines(&self, localized: &Localized) -> Vec<String> {
        crate::cod2_front::connect_lines(&self.connecting, self.session.as_deref(), localized)
    }

    /// The session's F3 lines.
    pub(crate) fn debug_lines(&mut self) -> Option<Vec<String>> {
        Some(self.session.as_deref_mut()?.debug_lines())
    }

    /// Esc with no menu up (`UI_SetActiveMenu` case 2, Mac 0x15118e):
    /// the menu `g_scriptMainMenu` names, else the main menu over the
    /// game; a demo's opens the main menu (`CL_KeyEvent`, Mac 0x1412e2,
    /// `UI_SetActiveMenu` 1). False with no session.
    pub(crate) fn escape(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        out: &mut Vec<crate::frontend::UiEffect>,
    ) -> bool {
        let Some(s) = self.session.as_deref() else {
            return false;
        };
        if s.is_demo() {
            ui.open_main(shell, out);
        } else {
            ui.set_server_cvars(self.menus.cvars.all(s.configstrings(), shell));
            let main = self.menus.main_menu(s.configstrings(), shell);
            crate::frontend::cod2::escape(ui, main.as_deref(), shell, out);
        }
        true
    }

    /// The script menus, once a frame: each gamestate loads the
    /// precached ones into the UI (closing what was up), the `v` the
    /// session left to the player set cvars, its `t`, `u` and `K` go
    /// through `frontend::cod2::menu_command`, and a refused `t` is
    /// offered again. False with no session.
    pub(crate) fn script_menus(
        &mut self,
        ui: &mut Ui,
        fs: &Pk3Fs,
        shell: &mut Shell,
        pending: &mut Vec<crate::frontend::UiEffect>,
    ) -> bool {
        let Some(s) = self.session.as_deref_mut() else {
            return false;
        };
        let gamestates = s.gamestates();
        if gamestates > 0 && self.menus_for != gamestates {
            self.menus_for = gamestates;
            self.menus
                .set_layout(vcod_cod2::net::protocol::CS_LAYOUT_118);
            self.menus.on_gamestate();
            ui.close_all();
            let language = shell.cvar("cl_language").unwrap_or("").to_string();
            let names = crate::frontend::cod2::script_menu_names(s.configstrings());
            for name in ui.load_script_menus(fs, &names, &language) {
                log::warn!("cod2: could not load script menu file '{name}'");
            }
        }
        for tokens in s.take_menu_commands() {
            // A `v` lands in the console's cvars; a userinfo dvar set so
            // goes back as `CL_Frame`'s userinfo push. No config save.
            if let Some(effects) = self.menus.cvars.on_server_command(&tokens, shell) {
                if effects.iter().any(|e| matches!(e, Effect::Userinfo)) {
                    s.set_userinfo(&shell.userinfo());
                }
                continue;
            }
            let mut out = Vec::new();
            if let Some(cmd) = crate::frontend::cod2::menu_command(
                ui,
                &mut self.menus,
                &tokens,
                s.configstrings(),
                s.server_id(),
                shell,
                &mut out,
            ) {
                s.send_command(&cmd);
            }
            pending.extend(out);
        }
        let mut out = Vec::new();
        crate::frontend::cod2::offer_waiting(
            ui,
            &mut self.menus,
            s.configstrings(),
            shell,
            &mut out,
        );
        pending.extend(out);
        if ui.active() {
            ui.set_server_cvars(self.menus.cvars.all(s.configstrings(), shell));
        }
        true
    }
}
