//! `App::window_event`'s keyboard and mouse arms.

use super::*;
use winit::dpi::PhysicalPosition;
use winit::event::KeyEvent;

impl App {
    /// `WindowEvent::KeyboardInput`.
    pub(crate) fn on_key(&mut self, event_loop: &ActiveEventLoop, event: KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        // Alt+Enter flips `r_fullscreen` and restarts the video
        // ahead of the console and binds (video::toggled). Its
        // release goes on, as retail's `WM_SYSKEYUP` does
        // (cod11-front-end.md 17, "Alt+Enter's release").
        if pressed
            && !event.repeat
            && self.modifiers.alt_key()
            && matches!(code, KeyCode::Enter | KeyCode::NumpadEnter)
        {
            let on = video::toggled(self.shell.cvar_f32("r_fullscreen") as i32);
            let effects = self
                .shell
                .execute(&format!("r_fullscreen {on}; vid_restart"));
            self.apply(event_loop, effects);
            return;
        }
        // ` and ~ toggle the console whatever they are bound to
        // (CoDMP.exe 0x40dc30).
        // CoD4's `monkeytoy` 1 (Enable Console No) keeps a closed
        // console closed (iw3mp.exe 0x467fb9; cod4-front-end.md 9).
        if code == KeyCode::Backquote {
            let restricted = self.game == vcod_common::game::Game::Cod4
                && !self.console.open
                && self.shell.cvar_f32("monkeytoy") != 0.0;
            if pressed && !event.repeat && !restricted {
                self.toggle_console();
            }
            return;
        }
        if self.console_active() {
            if !pressed {
                return;
            }
            match code {
                // In a game Esc reaches the menus under the console
                // and leaves it down (CoDMP.exe 0x40ddca, 0x4180a0).
                KeyCode::Escape if self.console.open && self.in_game() => {
                    // A CoD4 session's script menus take it first
                    // (`Cod4Client::menu_key`), then it frees the
                    // mouse.
                    if matches!(self.session, session::Session::Cod4(_)) {
                        if self.session.chat_key(code, None) != Some(true) {
                            self.escape();
                        }
                    } else if self.ui.active() {
                        let (_, effects) = self.ui.key(code, &self.shell);
                        self.ui_effects(event_loop, effects);
                    } else {
                        self.escape();
                    }
                }
                // With no game up a CoD1 Esc reaches the menus
                // under the console too, and the console stays
                // down; with none up it opens the main menu
                // (CoDMP.exe 0x40ddca, `UI_SetActiveMenu` 1).
                KeyCode::Escape if self.console.open && self.cod1_front_end() => {
                    if self.ui.active() {
                        let (_, effects) = self.ui.key(code, &self.shell);
                        self.ui_effects(event_loop, effects);
                    } else {
                        self.reopen_main();
                    }
                }
                KeyCode::Escape if self.console.open => self.toggle_console(),
                _ => {
                    let shell = &self.shell;
                    if let Some(line) = self
                        .console
                        .key(code, event.text.as_deref(), |p| shell.complete(p))
                    {
                        self.console_line(event_loop, &line);
                    }
                }
            }
            return;
        }
        // F3 and F4 fall through the menu; in a game nothing else
        // does, so no bind fires under it.
        if self.menu_takes_key(code) {
            if event.repeat {
                if pressed {
                    let effects = self.ui.key_repeat(code, event.text.as_deref(), &self.shell);
                    self.ui_effects(event_loop, effects);
                }
                return;
            }
            if !pressed {
                let effects = self.ui.key_up(&self.shell);
                self.ui_effects(event_loop, effects);
                self.after_menu();
                return;
            }
            self.ui.release();
            // A bind waiting for its key or an edit field takes
            // every key.
            if self.ui.waiting_for_key() {
                if let Some(key) = console::keys::key_name(code) {
                    let effects = self.ui.bind_key(key, &self.shell);
                    self.ui_effects(event_loop, effects);
                }
                return;
            }
            if self.ui.editing() {
                let effects = self.ui.edit_key(code, event.text.as_deref(), &self.shell);
                self.ui_effects(event_loop, effects);
                return;
            }
            let (used, effects) = self.ui.key(code, &self.shell);
            self.ui_effects(event_loop, effects);
            self.after_menu();
            if used || !matches!(code, KeyCode::F3 | KeyCode::F4) {
                return;
            }
        }
        if pressed && self.chat_key(code, event.text.as_deref()) {
            return;
        }
        if pressed
            && self.playing_demo()
            && console::keys::key_name(code).is_some_and(|k| console::keys::number(k) < 0x80)
        {
            self.demo_key();
            return;
        }
        if pressed && !event.repeat && self.eat_stray_key() {
            return;
        }
        // auto-repeat would retrigger a bind's press
        if event.repeat {
            return;
        }
        if pressed && code == KeyCode::Escape {
            self.escape();
            return;
        }
        // vcod's own: M opens the script menu Esc does.
        if pressed
            && code == KeyCode::KeyM
            && !self.ui.active()
            && matches!(
                self.mode,
                Mode::Online {
                    phase: Phase::Live(_),
                    ..
                }
            )
        {
            self.escape_in_game();
            return;
        }
        if code == KeyCode::F3 && pressed {
            self.debug_overlay = !self.debug_overlay;
            return;
        }
        if code == KeyCode::F4 && pressed {
            self.cull_mode = self.cull_mode.next();
            return;
        }
        if self.plays_binds()
            && let Some(key) = console::keys::key_name(code)
        {
            self.bound_key(event_loop, key, pressed);
        }
    }

    /// `WindowEvent::CursorMoved`.
    pub(crate) fn on_cursor_moved(
        &mut self,
        event_loop: &ActiveEventLoop,
        position: PhysicalPosition<f64>,
    ) {
        // Only where the pointer cannot be captured: a captured one
        // moves the cursor by its deltas (`device_event`).
        if !self.captured
            && let Some(r) = &self.renderer
        {
            let at = [position.x as f32, position.y as f32];
            if self
                .session
                .cod4_menu_mouse(None, Some(at), r.screen_size())
            {
                return;
            }
        }
        if !self.captured
            && self.menu_takes_mouse()
            && let Some(r) = &self.renderer
        {
            let (w, h) = r.screen_size();
            let effects =
                self.ui
                    .mouse_move(position.x as f32, position.y as f32, w, h, &self.shell);
            self.ui_effects(event_loop, effects);
        }
    }

    /// `WindowEvent::MouseInput`.
    pub(crate) fn on_mouse_input(
        &mut self,
        event_loop: &ActiveEventLoop,
        state: ElementState,
        button: winit::event::MouseButton,
    ) {
        use winit::event::MouseButton;
        let pressed = state == ElementState::Pressed;
        if self.menu_takes_mouse() {
            if pressed
                && self.ui.waiting_for_key()
                && let Some(key) = console::keys::mouse_name(button)
            {
                let effects = self.ui.bind_key(key, &self.shell);
                self.ui_effects(event_loop, effects);
            } else if pressed && let Some(key) = console::keys::mouse_name(button) {
                let key = console::keys::number(key);
                let effects = self.ui.mouse_button(key, Instant::now(), &self.shell);
                self.ui_effects(event_loop, effects);
                self.after_menu();
            } else if !pressed {
                let effects = self.ui.key_up(&self.shell);
                self.ui_effects(event_loop, effects);
                self.after_menu();
            }
            return;
        }
        if self.console_active() {
            return;
        }
        // A CoD4 script menu takes the buttons; MOUSE1 clicks it, a
        // waiting bind row takes any (`Cod4Client::menu_button`).
        if self.session.cod4_menu_has_mouse() {
            if !pressed && button == MouseButton::Left {
                self.session.cod4_menu_release();
            }
            if pressed
                && let Some(key) = console::keys::mouse_name(button)
                && let Some(r) = &self.renderer
            {
                let size = r.screen_size();
                self.session.cod4_menu_button(key, size);
            }
            return;
        }
        if button == MouseButton::Left && pressed && self.playing_demo() {
            self.demo_key();
            return;
        }
        // the click that captures the mouse must not also fire
        if button == MouseButton::Left && pressed && !self.grabbed {
            self.set_grab(true);
            return;
        }
        if pressed && self.eat_stray_key() {
            return;
        }
        if self.plays_binds()
            && let Some(key) = console::keys::mouse_name(button)
        {
            self.bound_key(event_loop, key, pressed);
        }
    }

    /// `WindowEvent::MouseWheel`.
    pub(crate) fn on_wheel(&mut self, event_loop: &ActiveEventLoop, delta: MouseScrollDelta) {
        // One press a whole notch (console::keys::WheelNotches).
        let scroll = match delta {
            MouseScrollDelta::LineDelta(_, y) => self.wheel.step(Some(y), None),
            MouseScrollDelta::PixelDelta(p) => self.wheel.step(None, Some(p.y as f32)),
        } as f32;
        if self.menu_takes_mouse() {
            if scroll != 0.0 && self.ui.waiting_for_key() {
                let key = if scroll > 0.0 {
                    "MWHEELUP"
                } else {
                    "MWHEELDOWN"
                };
                let effects = self.ui.bind_key(key, &self.shell);
                self.ui_effects(event_loop, effects);
            } else if scroll != 0.0 {
                let effects = self.ui.wheel(scroll < 0.0, &self.shell);
                self.ui_effects(event_loop, effects);
            }
            return;
        }
        if self.console_active() {
            if scroll != 0.0 {
                self.console.scroll(scroll > 0.0);
            }
            return;
        }
        if self.session.cod4_menu_has_mouse() {
            if scroll != 0.0
                && let Some(r) = &self.renderer
            {
                let key = if scroll > 0.0 {
                    "MWHEELUP"
                } else {
                    "MWHEELDOWN"
                };
                let size = r.screen_size();
                self.session.cod4_menu_button(key, size);
            }
            return;
        }
        // The fly camera's wheel sets its speed, a vcod control no bind holds.
        let flying = match self.mode {
            Mode::Fly(_) => true,
            Mode::Cod2(_) => !self.session.live(),
            _ => false,
        };
        if flying && let Mode::Fly(cam) | Mode::Cod2(cam) = &mut self.mode {
            cam.adjust_speed(scroll);
        } else if self.plays_binds() && self.grabbed && scroll != 0.0 {
            let key = if scroll < 0.0 {
                "MWHEELDOWN"
            } else {
                "MWHEELUP"
            };
            if self.eat_stray_key() {
                return;
            }
            for _ in 0..scroll.abs() as i32 {
                self.bound_key(event_loop, key, true);
                self.bound_key(event_loop, key, false);
            }
        }
    }
}
