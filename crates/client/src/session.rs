//! The game `App` plays: one set of calls for each step of the frame and
//! for the console, input, renderer builds and quit, whichever game it is.
//! CoD1's state still lives on `App` (`mode`, `world`, `hud`), so its arm
//! answers each call as "nothing here" and the frame's mode arms run it;
//! CoD2's arm is a [`Cod2Client`], CoD4's a [`Cod4Client`] (a watched
//! capture or a live join, its pump drawing too). The game is fixed at
//! startup. With no CoD4 game up a CoD4 install runs its front end
//! ([`Cod4Front`], `Session::Cod4Menu`), which a join hands over to
//! `Session::Cod4` and leaving the game takes back.

use anyhow::Result;
use glam::Vec3;
use std::collections::HashMap;
use vcod_common::localize::Localized;
use vcod_common::pk3::Pk3Fs;

use crate::camera::FlyCamera;
use crate::cod2_client::{Cod2Client, Cod2Frame, DrawView};
use crate::cod4_client::Cod4Client;
use crate::cod4_front::Cod4Front;
use crate::console::shell::{Effect, Shell};
use crate::frontend::{Ui, UiEffect};
use crate::play::cod2_events::Event2;
use crate::renderer::Renderer;
use crate::{cod2_viewmodel, console, hud, play};

pub(crate) enum Session {
    Cod1,
    Cod2(Box<Cod2Client>),
    Cod4(Box<Cod4Client>),
    /// CoD4's main menu, with no game up.
    Cod4Menu(Box<Cod4Front>),
}

impl Session {
    pub(crate) fn cod2(&self) -> Option<&Cod2Client> {
        match self {
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => None,
            Session::Cod2(c) => Some(c),
        }
    }

    pub(crate) fn cod4_mut(&mut self) -> Option<&mut Cod4Client> {
        match self {
            Session::Cod4(c) => Some(c),
            Session::Cod1 | Session::Cod2(_) | Session::Cod4Menu(_) => None,
        }
    }

    /// The CoD4 front end, while it is up.
    pub(crate) fn cod4_front_mut(&mut self) -> Option<&mut Cod4Front> {
        match self {
            Session::Cod4Menu(f) => Some(f),
            Session::Cod1 | Session::Cod2(_) | Session::Cod4(_) => None,
        }
    }

    /// The CoD4 front end has a menu up: it takes the keys and the mouse.
    pub(crate) fn cod4_front_active(&self) -> bool {
        matches!(self, Session::Cod4Menu(f) if f.active())
    }

    /// The CoD4 front end's quads on a `size` window.
    pub(crate) fn cod4_front_quads(&self, size: (f32, f32)) -> Vec<hud::HudQuad> {
        match self {
            Session::Cod4Menu(f) if f.active() => f.quads(size),
            Session::Cod1 | Session::Cod2(_) | Session::Cod4(_) | Session::Cod4Menu(_) => {
                Vec::new()
            }
        }
    }

    pub(crate) fn cod2_mut(&mut self) -> Option<&mut Cod2Client> {
        match self {
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => None,
            Session::Cod2(c) => Some(c),
        }
    }

    // The frame, in its order (`App::redraw`).

    /// Before the frame: the script menus off the session. True when a
    /// session read them (the caller syncs the menu mouse).
    pub(crate) fn script_menus(
        &mut self,
        ui: &mut Ui,
        fs: &Pk3Fs,
        shell: &mut Shell,
        pending: &mut Vec<UiEffect>,
    ) -> bool {
        match self {
            Session::Cod2(c) => c.script_menus(ui, fs, shell, pending),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => false,
        }
    }

    /// A drop's reason, or the map a new gamestate names.
    pub(crate) fn take_level_change(&mut self) -> Result<Option<String>> {
        match self {
            Session::Cod2(c) => c.take_level_change(),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => Ok(None),
        }
    }

    /// A demo played out, with its `timedemo` line.
    pub(crate) fn demo_over(&self) -> Option<Option<String>> {
        match self {
            Session::Cod4(c) => c.demo_over(),
            Session::Cod2(c) => c.demo_over(),
            Session::Cod1 | Session::Cod4Menu(_) => None,
        }
    }

    pub(crate) fn finish_pump(&mut self) {
        match self {
            Session::Cod2(c) => {
                c.finish_pump();
            }
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => {}
        }
    }

    /// The session read off the wire; false with none (the camera flies).
    pub(crate) fn pump(&mut self, cx: &mut Cod2Frame, cam: &mut FlyCamera) -> bool {
        match self {
            Session::Cod4(c) => c.pump(cx, cam),
            Session::Cod2(c) => c.pump(cx, cam),
            Session::Cod1 | Session::Cod4Menu(_) => false,
        }
    }

    pub(crate) fn scene(
        &mut self,
        cx: &mut Cod2Frame,
        cam: &mut FlyCamera,
    ) -> (Option<cod2_viewmodel::ViewFrame>, Vec<hud::HudQuad>) {
        match self {
            Session::Cod4(c) => {
                // A map change loading draws its loading screen alone.
                if let Some(q) = c.map_load_quads(cx.r.screen_size()) {
                    return (None, q);
                }
                (c.take_view(), c.hud_quads(cx.r.screen_size(), cx.shell))
            }
            Session::Cod2(c) => c.scene(cx, cam),
            Session::Cod1 | Session::Cod4Menu(_) => (None, Vec::new()),
        }
    }

    pub(crate) fn events(&mut self, cx: &mut Cod2Frame) -> Vec<Event2> {
        match self {
            Session::Cod2(c) => c.events(cx),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => Vec::new(),
        }
    }

    pub(crate) fn audio(&mut self, cx: &mut Cod2Frame, events: &[Event2], cam_pos: Vec3) {
        match self {
            Session::Cod2(c) => {
                c.audio(cx, events, cam_pos);
            }
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn effects(&mut self, cx: &mut Cod2Frame, events: &[Event2], cam_pos: Vec3) {
        match self {
            Session::Cod2(c) => {
                c.effects(cx, events, cam_pos);
            }
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn view(
        &mut self,
        cx: &mut Cod2Frame,
        events: &[Event2],
        cam: &FlyCamera,
    ) -> Option<play::cod2_view::ViewOut> {
        match self {
            Session::Cod2(c) => c.view(cx, events, cam),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => None,
        }
    }

    pub(crate) fn idle(&mut self) {
        match self {
            Session::Cod2(c) => {
                c.idle();
            }
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => {}
        }
    }

    /// The draw's angles, fov and view weapon; the camera's own without a
    /// session.
    pub(crate) fn draw_view(
        &self,
        fs: &Pk3Fs,
        r: &mut Renderer,
        cam: &FlyCamera,
        fx: Option<&play::cod2_view::ViewOut>,
        vm: Option<cod2_viewmodel::ViewFrame>,
        fovs: (f32, f32, bool),
    ) -> DrawView {
        match self {
            Session::Cod4(c) => c.draw_view(fs, r, cam, vm, fovs),
            Session::Cod2(c) => c.draw_view(fs, r, cam, fx, vm, fovs),
            Session::Cod1 | Session::Cod4Menu(_) => DrawView {
                yaw: cam.yaw,
                pitch: cam.pitch,
                roll: 0.0,
                fov: fovs.0,
                vm: None,
            },
        }
    }

    pub(crate) fn world_hud(
        &self,
        quads: &mut Vec<hud::HudQuad>,
        view_proj: glam::Mat4,
        angles: (f32, f32, f32),
        size: (f32, f32),
    ) {
        match self {
            Session::Cod4(c) => c.world_hud(quads, view_proj, angles, size),
            Session::Cod2(c) => c.world_hud(quads, view_proj, angles, size),
            Session::Cod1 | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn map_name(&self) -> Option<&str> {
        match self {
            Session::Cod2(c) => c.map_name(),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => None,
        }
    }

    /// A CoD4 session's view drawn (`Cod4Client::in_game`).
    pub(crate) fn cod4_in_game(&self) -> bool {
        matches!(self, Session::Cod4(c) if c.in_game())
    }

    /// The drawn CoD2 frame's `serverTime`; None with no frame.
    pub(crate) fn frame_time(&self) -> Option<i32> {
        match self {
            Session::Cod2(c) => c.session.as_deref()?.frame().map(|f| f.time_ms),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => None,
        }
    }

    /// Where a CoD4 session's entities stand this frame, which the sounds
    /// riding them follow; empty for any other session.
    pub(crate) fn entity_positions(&self) -> HashMap<u32, Vec3> {
        match self {
            Session::Cod4(c) => c.entity_positions(),
            Session::Cod1 | Session::Cod2(_) | Session::Cod4Menu(_) => HashMap::new(),
        }
    }

    /// The angles a CoD4 session's entities' voices turn with; empty for
    /// any other session.
    pub(crate) fn entity_angles(&self) -> HashMap<u32, Vec3> {
        match self {
            Session::Cod4(c) => c.entity_angles(),
            Session::Cod1 | Session::Cod2(_) | Session::Cod4Menu(_) => HashMap::new(),
        }
    }

    pub(crate) fn recording_status(&self) -> Option<(&str, u64)> {
        match self {
            Session::Cod4(c) => c.session.recording_status(),
            Session::Cod2(c) => c.recording_status(),
            Session::Cod1 | Session::Cod4Menu(_) => None,
        }
    }

    pub(crate) fn connect_lines(&self, localized: &Localized) -> Vec<String> {
        match self {
            Session::Cod2(c) => c.connect_lines(localized),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => Vec::new(),
        }
    }

    pub(crate) fn debug_lines(&mut self) -> Option<Vec<String>> {
        match self {
            Session::Cod4(c) => Some(c.debug_lines()),
            Session::Cod2(c) => c.debug_lines(),
            Session::Cod1 | Session::Cod4Menu(_) => None,
        }
    }

    /// The fly keys held this frame, which a watched CoD4 session flies
    /// its camera (a capture) or its spectator (live) by.
    pub(crate) fn set_fly_keys(&mut self, input: &crate::camera::InputState) {
        match self {
            Session::Cod4(c) => c.set_keys(crate::cod4_client::FlyKeys::of(input)),
            Session::Cod1 | Session::Cod2(_) | Session::Cod4Menu(_) => {}
        }
    }

    /// The F3 camera line's tag while the session drives `Mode::Cod2`'s
    /// camera.
    pub(crate) fn cam_tag(&self) -> &'static str {
        match self {
            Session::Cod4(_) => "cod4",
            Session::Cod1 | Session::Cod2(_) | Session::Cod4Menu(_) => "cod2",
        }
    }

    // Outside the frame.

    /// A CoD2 or CoD4 session is up.
    pub(crate) fn live(&self) -> bool {
        match self {
            Session::Cod4(_) => true,
            Session::Cod2(c) => c.live(),
            Session::Cod1 | Session::Cod4Menu(_) => false,
        }
    }

    /// A CoD2 front-end connect is running.
    pub(crate) fn busy(&self) -> bool {
        match self {
            Session::Cod2(c) => c.busy(),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => false,
        }
    }

    pub(crate) fn playing_demo(&self) -> bool {
        match self {
            Session::Cod4(c) => c.session.is_demo(),
            Session::Cod2(c) => c.playing_demo(),
            Session::Cod1 | Session::Cod4Menu(_) => false,
        }
    }

    /// The quit, after CoD1's `disconnect`.
    pub(crate) fn shutdown(&mut self) {
        match self {
            Session::Cod4(c) => c.session.disconnect(),
            Session::Cod2(c) => c.shutdown(),
            Session::Cod1 | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn park(&mut self) {
        match self {
            Session::Cod2(c) => {
                c.park();
            }
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn reset_render(&mut self) {
        match self {
            Session::Cod4(c) => c.reset_render(),
            Session::Cod2(c) => c.reset_render(),
            Session::Cod1 | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn release_keys(&mut self) {
        match self {
            Session::Cod4(c) => c.session.input.release_all(),
            Session::Cod2(c) => c.release_keys(),
            Session::Cod1 | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn hide_scoreboard(&mut self) {
        match self {
            Session::Cod4(c) => c.set_scores(false),
            Session::Cod2(c) => c.hide_scoreboard(),
            Session::Cod1 | Session::Cod4Menu(_) => {}
        }
    }

    /// A key press over a CoD4 session's held board
    /// (`Cod4Hud::scoreboard_key`); true when the board took it.
    pub(crate) fn cod4_scoreboard_key(&mut self, key: &str, step: i32) -> bool {
        match self {
            Session::Cod4(c) => c.hud.scoreboard_key(key, step),
            Session::Cod1 | Session::Cod2(_) | Session::Cod4Menu(_) => false,
        }
    }

    pub(crate) fn scoreboard_mut(&mut self) -> Option<&mut hud::scoreboard::Scoreboard> {
        match self {
            Session::Cod2(c) => c.scoreboard_mut(),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => None,
        }
    }

    /// The mouse's turn under a CoD2 or CoD4 shellshock's caps.
    pub(crate) fn turn(&mut self, degrees: [f32; 2]) -> [f32; 2] {
        match self {
            Session::Cod4(c) => c.turn(degrees),
            Session::Cod2(c) => c.turn(degrees),
            Session::Cod1 | Session::Cod4Menu(_) => degrees,
        }
    }

    pub(crate) fn reset_view_fx(&mut self) {
        match self {
            Session::Cod2(c) => {
                c.reset_view_fx();
            }
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn chat_key(
        &mut self,
        code: winit::keyboard::KeyCode,
        text: Option<&str>,
    ) -> Option<bool> {
        match self {
            Session::Cod4(c) => c.chat_key(code, text),
            Session::Cod4Menu(f) if f.active() => Some(f.key(code, text)),
            Session::Cod2(c) => c.chat_key(code, text),
            Session::Cod1 | Session::Cod4Menu(_) => None,
        }
    }

    /// The mouse over a CoD4 session's drawn script menu: raw counts `d`,
    /// a window position `at`, or with neither a MOUSE1 press. False when
    /// no such menu holds the mouse (nothing happened).
    pub(crate) fn cod4_menu_mouse(
        &mut self,
        d: Option<[f32; 2]>,
        at: Option<[f32; 2]>,
        size: (f32, f32),
    ) -> bool {
        match self {
            Session::Cod4(c) => c.menu_mouse(d, at, size),
            Session::Cod4Menu(f) if f.active() => {
                match (d, at) {
                    (None, None) => f.button("MOUSE1", size),
                    _ => f.mouse(d, at, size),
                }
                true
            }
            Session::Cod1 | Session::Cod2(_) | Session::Cod4Menu(_) => false,
        }
    }

    /// A mouse button or wheel notch by its key name over a CoD4
    /// session's drawn script menu (`Cod4Client::menu_button`); false when
    /// no such menu holds the mouse.
    pub(crate) fn cod4_menu_button(&mut self, key: &str, size: (f32, f32)) -> bool {
        match self {
            Session::Cod4(c) => c.menu_button(key, size),
            Session::Cod4Menu(f) if f.active() => {
                f.button(key, size);
                true
            }
            Session::Cod1 | Session::Cod2(_) | Session::Cod4Menu(_) => false,
        }
    }

    /// MOUSE1 up over a CoD4 session's drawn script menu: a slider or
    /// scroll bar drag ends.
    pub(crate) fn cod4_menu_release(&mut self) {
        match self {
            Session::Cod4(c) => c.session.menus.release(),
            Session::Cod4Menu(f) => f.release(),
            Session::Cod1 | Session::Cod2(_) => {}
        }
    }

    /// `rcon` on CoD2 1.3 and CoD4 1.7, whose `CL_Rcon_f` logs in
    /// (`console::rcon::Cod4Rcon`): to the server joined, else to `rcon
    /// host`'s address over `sock`; the console lines it prints. `None` on
    /// CoD1, whose `rcon` reads `rconPassword` and `rconAddress`
    /// (`App::rcon`).
    pub(crate) fn login_rcon(
        &mut self,
        rcon: &mut console::rcon::Cod4Rcon,
        sock: &mut console::rcon::RconSocket,
        args: &[String],
    ) -> Option<Vec<String>> {
        use console::rcon::Cod4RconOut;
        // `cls.state` at or past `CA_CONNECTED` (CoD2MP_s.exe 0x4b4398,
        // iw3mp.exe 0x471cb7): a CoD2 session exists only past its
        // `connectResponse`; a demo or a capture joins nothing.
        let joined = match self {
            Session::Cod1 => return None,
            Session::Cod2(c) => c.session.as_ref().is_some_and(|s| !s.is_demo()),
            Session::Cod4(c) => c.session.is_live(),
            Session::Cod4Menu(_) => false,
        };
        match rcon.command(args, joined) {
            Cod4RconOut::Lines(lines) => return Some(lines),
            Cod4RconOut::Server(text) => match self {
                Session::Cod2(c) => {
                    if let Some(s) = c.session.as_deref_mut() {
                        s.send_oob(&text);
                    }
                }
                Session::Cod4(c) => c.session.send_oob(&text),
                Session::Cod1 | Session::Cod4Menu(_) => {}
            },
            Cod4RconOut::Host(to, text) => {
                let mut p = vec![0xff; 4];
                p.extend_from_slice(&text);
                sock.send(to, &p);
            }
        }
        Some(Vec::new())
    }

    /// A CoD2 session's OOB `print`s (rcon's answers) since the last call;
    /// CoD4's reach the console through its frame's lines.
    pub(crate) fn take_prints(&mut self) -> Vec<String> {
        match self {
            Session::Cod2(c) => c
                .session
                .as_deref_mut()
                .map_or(Vec::new(), |s| s.take_prints()),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => Vec::new(),
        }
    }

    /// The console lines a CoD4 session's script menus ran (`popup_leavegame`'s
    /// `disconnect`), for the client's console.
    pub(crate) fn take_menu_console(&mut self) -> Vec<String> {
        match self {
            Session::Cod4(c) => c.session.menus.take_console(),
            Session::Cod4Menu(f) => f.take_console(),
            Session::Cod1 | Session::Cod2(_) => Vec::new(),
        }
    }

    /// Whether a CoD4 session's drawn script menu holds the mouse.
    pub(crate) fn cod4_menu_has_mouse(&self) -> bool {
        match self {
            Session::Cod4(c) => c.menu_has_mouse(),
            Session::Cod4Menu(f) => f.active(),
            Session::Cod1 | Session::Cod2(_) => false,
        }
    }

    pub(crate) fn menu_response(&mut self, menu: &str, response: &str) -> bool {
        match self {
            Session::Cod2(c) => c.menu_response(menu, response),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => false,
        }
    }

    pub(crate) fn apply(
        &mut self,
        effect: &Effect,
        shell: &Shell,
        start: std::time::Instant,
    ) -> bool {
        match self {
            Session::Cod4(c) => c.apply(effect),
            Session::Cod2(c) => c.apply(effect, shell, start),
            Session::Cod1 | Session::Cod4Menu(_) => false,
        }
    }

    pub(crate) fn record(&mut self, dir: &std::path::Path, name: Option<&str>) -> Option<String> {
        match self {
            Session::Cod4(c) => Some(c.session.start_recording(dir, name)),
            Session::Cod2(c) => c.record(dir, name),
            Session::Cod1 | Session::Cod4Menu(_) => None,
        }
    }

    pub(crate) fn stop_record(&mut self) -> Option<String> {
        match self {
            Session::Cod4(c) => Some(c.session.stop_recording()),
            Session::Cod2(c) => c.stop_record(),
            Session::Cod1 | Session::Cod4Menu(_) => None,
        }
    }

    pub(crate) fn shellshock(&mut self, args: Vec<String>) -> Result<(), Vec<String>> {
        match self {
            Session::Cod2(c) => c.shellshock(args),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => Err(args),
        }
    }

    pub(crate) fn set_userinfo(&mut self, info: &vcod_common::net::Userinfo) {
        match self {
            Session::Cod2(c) => c.set_userinfo(info),
            // CoD4's userinfo carries only `name` of these (`Conn::set_name`).
            Session::Cod4(c) => c.session.set_name(&info.name),
            Session::Cod1 | Session::Cod4Menu(_) => {}
        }
    }

    pub(crate) fn escape(&mut self, ui: &mut Ui, shell: &Shell, out: &mut Vec<UiEffect>) -> bool {
        match self {
            Session::Cod2(c) => c.escape(ui, shell, out),
            Session::Cod1 | Session::Cod4(_) | Session::Cod4Menu(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::rcon::{Cod4Rcon, RconSocket};
    use crate::play::cod2::Cod2Session;
    use std::time::{Duration, Instant};

    /// `rcon` from a client joined to `vcod-server --game cod2 --set
    /// rconPassword=x` (cod2-protocol.md 1.6): each console line through
    /// the shell and `Session::login_rcon`, the answers back through
    /// `Session::take_prints` while joined, through the rcon socket for
    /// `rcon host`. Needs `COD2_DIR`.
    #[test]
    fn cod2_live_rcon_status_from_a_joined_client_and_from_the_host() {
        use crate::play::cod4::tests::run_held;
        use vcod_server::cod2::{Cod2Config, Cod2Server};
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let fs = std::rc::Rc::new(fs);
        let mut sv = Cod2Server::new(
            Cod2Config {
                map: "mp_toujane".into(),
                gametype: "dm".into(),
                hostname: "rcon".into(),
                max_clients: 4,
                gametype_script: None,
            },
            7,
            Instant::now(),
        );
        sv.exec_default_cfg(&*fs);
        sv.set_cvar("sv_pure", "0");
        // As `vcod-server`'s `--set rconPassword=x` sets it.
        sv.set_cvar(vcod_server::rcon::login_cvar_name("rconPassword"), "x");
        sv.load(Some(fs)).unwrap();
        run_held(&mut sv, rcon_client, |_, _, _| true);
    }

    /// The rcon test's client, on the test clock.
    fn rcon_client(addr: std::net::SocketAddr) {
        use crate::play::cod4::tests::{clk_now, clk_sleep};
        let fs = vcod_common::testing::cod2_fs().unwrap();
        let mut shell = Shell::new();
        shell.use_cod2_cvars();
        shell.apply_latched();
        shell.execute("set name admin");
        let opts = play::cod2::ConnectOpts {
            home: None,
            ..crate::cod2_front::connect_opts(&shell, &fs)
        };
        let s = Cod2Session::connect(&addr.to_string(), &opts, Duration::from_secs(60)).unwrap();
        let mut client = Cod2Client::new(None);
        client.session = Some(Box::new(s));
        let mut session = Session::Cod2(Box::new(client));
        let mut rcon = Cod4Rcon::default();
        let mut sock = RconSocket::default();
        let mut run = |session: &mut Session, line: &str| {
            let effects = shell.execute(line);
            let [Effect::Rcon(args)] = effects.as_slice() else {
                panic!("{line}: {effects:?}");
            };
            let mut got = session.login_rcon(&mut rcon, &mut sock, args).unwrap();
            // Past the server's 500 ms between two rcons.
            let end = clk_now() + Duration::from_millis(700);
            while clk_now() < end {
                if let Session::Cod2(c) = session
                    && let Some(s) = c.session.as_deref_mut()
                {
                    s.poll(None);
                }
                got.extend(session.take_prints());
                got.extend(sock.poll());
                clk_sleep(Duration::from_millis(16));
            }
            got
        };
        let a: Vec<Vec<String>> = [
            "rcon status",
            "rcon login wrong",
            "rcon status",
            "rcon login x",
            "rcon status",
        ]
        .iter()
        .map(|l| run(&mut session, l))
        .collect();
        assert_eq!(
            a[0],
            ["You need to log in with 'rcon login <password>' before using rcon."]
        );
        assert!(a[1].is_empty(), "{:?}", a[1]);
        assert_eq!(a[2], ["Invalid password."]);
        assert!(a[3].is_empty(), "{:?}", a[3]);
        let status = a[4].join("\n");
        assert!(
            status.starts_with("map: mp_toujane\nnum score ping"),
            "{status}"
        );
        assert!(status.contains(" admin^7"), "{status}");
        // Left, the target is `rcon host`'s.
        if let Session::Cod2(c) = &mut session {
            c.session.take().unwrap().disconnect();
        }
        assert_eq!(
            run(&mut session, "rcon status"),
            [
                "Can't determine rcon target.  You can fix this by either:",
                "1) Joining the server as a player.",
                "2) Setting the host server with 'rcon host <address>'.",
            ]
        );
        assert!(run(&mut session, &format!("rcon host {addr}")).is_empty());
        let status = run(&mut session, "rcon status").join("\n");
        assert!(
            status.starts_with("map: mp_toujane\nnum score ping"),
            "{status}"
        );
    }
}
