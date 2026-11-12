//! CoD4's front end with no game up: 1.7's main menu and the screens behind
//! it, the `ui_mp` zone's menus (docs/research/cod4-front-end.md) run by
//! the script menu engine (`play::cod4_menus`, stacked) and drawn by
//! `cod4_menu`. `App` feeds it the keys and the mouse while it is up, runs
//! the console lines its scripts leave (`quit`, the binds and cvars the
//! option pages set) and carries out its requests (Start New Server, a
//! mod, the server browser's join) and plays its sounds and music; a game
//! it starts or joins goes to a `Session::Cod4`, and leaving
//! that game comes back here, over `error_popmenu` when the server or the
//! load said why (`UI_SetActiveMenu(UIMENU_MAIN)`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use vcod_cod4::net::parse::Gamestate;
use vcod_cod4::net::stats::Stats;
use winit::keyboard::KeyCode;

use anyhow::Context;

use crate::cod4_hud::Cod4Hud;
use crate::cod4_loading::{JoinArgs, Loader};
use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::play::cod4_menus::browser::{Browser, JoinRequest};
use crate::play::cod4_menus::front::{Request, active_profile};
use crate::play::cod4_menus::{Host, MenuKey, ScriptMenus};
use crate::play::cod4_profile::ProfileStats;
use crate::session::Session;
use crate::{App, Mode, cod4_fly, console, version_tag};

/// The menu `UI_SetActiveMenu(UIMENU_MAIN)` opens, and the one it opens
/// over it when `com_errorMessage` is set.
const MAIN: &str = "main";
const ERROR_POPUP: &str = "error_popmenu";

pub(crate) struct Cod4Front {
    /// The install root (`main/`, `zone/`, `mods/`, `players/`).
    root: PathBuf,
    /// The zones' fonts, images, strings and menus.
    hud: Cod4Hud,
    pub(crate) menus: ScriptMenus,
    /// The server the menus never have: an empty gamestate.
    gs: Gamestate,
    start: Instant,
    /// The `fs_game` the zones loaded under, once loaded.
    loaded: Option<Option<String>>,
    /// The zones' sound aliases and loaded sounds, and whether the audio
    /// system and the search path hold them (a game replaces both).
    sounds: Option<Arc<vcod_cod4::sound::SoundSet>>,
    sounds_installed: bool,
    /// The active profile's stats file, which the menus' `stat()` read
    /// and their `statset`s write; `None` with no profile.
    profile: Option<ProfileStats>,
    /// The profile whose custom classes a fresh block named last.
    named: Option<PathBuf>,
}

impl Cod4Front {
    pub(crate) fn new(root: PathBuf) -> Self {
        let mut menus = ScriptMenus::front_end();
        menus.front.root = Some(root.clone());
        menus.front.players_root = Some(root.clone());
        // `LAN_LoadCachedServers` at the UI's start (cod4-handshake.md 12.5).
        menus.browser = Browser::with_cache(root.join(vcod_cod4::net::browser::CACHE_FILE));
        Cod4Front {
            root,
            hud: Cod4Hud::default(),
            menus,
            gs: Gamestate::default(),
            start: Instant::now(),
            loaded: None,
            sounds: None,
            sounds_installed: false,
            profile: None,
            named: None,
        }
    }

    /// The active profile's stats into the menus, a fresh profile's without
    /// a file or a profile: what Create a Class and Rank & Challenges read.
    /// A game's `N`s reach the file before the menu comes back
    /// (cod4-front-end.md 4).
    pub(crate) fn reload_stats(&mut self) {
        self.save_stats(true);
        let fs_game = self.loaded.clone().flatten().unwrap_or_default();
        let loaded = self
            .menus
            .front
            .players_root
            .as_deref()
            .and_then(|root| ProfileStats::load_from(root, &fs_game));
        let (profile, stats) = match loaded {
            Some((p, s)) => (Some(p), s),
            None => (None, Stats::fresh()),
        };
        // `LiveStorage_StatsInit` names the five custom classes for a
        // profile with no stats yet, once as it loads (0x4fc5b3 on).
        let fresh = profile.as_ref().filter(|p| !p.found()).map(|p| p.dir());
        if let Some(dir) = fresh
            && self.named.as_deref() != Some(dir)
        {
            self.named = Some(dir.to_path_buf());
            for n in 1..=5 {
                let name = self.translate(&format!("CLASS_SLOT{n}"));
                self.menus.set_class_name(n, &name);
            }
        }
        self.profile = profile;
        self.menus.set_stats(stats);
    }

    /// The menus' `statset`s to the profile's file once settled, or at
    /// once with `now`.
    fn save_stats(&mut self, now: bool) {
        if let (Some(p), Some(stats)) = (self.profile.as_mut(), self.menus.stats()) {
            p.save(stats, now);
        }
    }

    /// The front end's zones under `fs_game` (`mods/<dir>`), loaded on
    /// first use and again when the mod changes; false when they did not
    /// load (warned).
    pub(crate) fn load(&mut self, fs_game: Option<&str>) -> bool {
        let want = fs_game.map(str::to_string);
        if self.loaded.as_ref() != Some(&want) {
            self.hud.load_front(&self.root.join("main"), fs_game);
            self.loaded = Some(want);
            self.sounds = cod4_fly::front_sounds(&self.root, fs_game)
                .inspect_err(|e| log::warn!("cod4 front end: no sounds: {e:#}"))
                .ok()
                .map(Arc::new);
            self.sounds_installed = false;
            if let Some((defs, tables)) = self.hud.script_menus() {
                self.menus.set_defs(defs.clone(), tables.clone());
                self.menus.strings = self.hud.strings();
                if let Some(d) = self.hud.startup_dvars() {
                    self.menus.startup = d.clone();
                }
                if let Some(c) = self.hud.menu_cfgs() {
                    self.menus.cfgs = c.clone();
                }
                if let Some(l) = self.hud.ui_lists() {
                    self.menus.set_lists(l.clone());
                }
            }
        }
        self.hud.menu_assets().is_some()
    }

    /// `UI_SetActiveMenu(UIMENU_MAIN)`: every menu closes, `main` opens
    /// (its `onOpen` opens `main_text` and the profile pick), and over it
    /// `error_popmenu` when `error` says why the last game ended. A
    /// server's disconnect titles it `MENU_NOTICE`, anything else
    /// `MENU_ERROR` (`Com_SetErrorMessage`).
    pub(crate) fn open_main(&mut self, shell: &Shell, error: Option<(&str, bool)>) {
        self.sync(shell);
        let mut out = self.menus.close_all(&self.gs);
        self.set_profile();
        self.reload_stats();
        if let Some((message, notice)) = error {
            let key = if notice { "MENU_NOTICE" } else { "MENU_ERROR" };
            let title = self.translate(key);
            let ui = &mut self.menus.ui.dvars;
            ui.insert("com_errortitle".into(), title);
            ui.insert("com_errormessage".into(), message.to_string());
        }
        out.extend(self.menus.open_by_name(MAIN, &self.gs));
        let message = self.menus.ui.dvars.get("com_errormessage");
        if message.is_some_and(|m| !m.is_empty() && m != ";") {
            out.extend(self.menus.open_by_name(ERROR_POPUP, &self.gs));
        }
        log_unsent(out);
    }

    /// `Com_InitPlayerProfiles`: `com_playerProfile` is `active.txt`'s
    /// profile, else empty, and `main`'s `onOpen` then opens the profile
    /// pick, which with none on disk asks for a new one
    /// (cod4-front-end.md 3).
    fn set_profile(&mut self) {
        let dvars = &mut self.menus.ui.dvars;
        if dvars.contains_key("com_playerprofile") {
            return;
        }
        let players = self.menus.front.players_root.as_deref();
        let profile = players.and_then(active_profile).unwrap_or_default();
        dvars.insert("com_playerprofile".into(), profile);
    }

    fn translate(&self, key: &str) -> String {
        self.hud
            .strings()
            .and_then(|l| l.get(key).map(str::to_string))
            .unwrap_or_else(|| key.to_string())
    }

    /// Every menu closes (`Menus_CloseAll`): a join's connect screen
    /// takes the window.
    pub(crate) fn close(&mut self) {
        let out = self.menus.close_all(&self.gs);
        log_unsent(out);
        // The join reads the file next.
        self.save_stats(true);
    }

    /// A menu is up.
    pub(crate) fn active(&self) -> bool {
        self.menus.open().is_some()
    }

    /// The console's cvars and binds, as the option pages read them.
    pub(crate) fn sync(&mut self, shell: &Shell) {
        self.menus.host = Host {
            cvars: shell
                .cvar_values()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            enums: shell
                .enum_domains()
                .map(|(k, v)| (k.to_string(), v.to_vec()))
                .collect(),
            binds: shell
                .binds()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            players: Vec::new(),
        };
    }

    /// A key while the front end is up (`Menu_HandleKey`): a bind row
    /// waiting takes any; the arrows and Tab move the focus, Enter runs
    /// it, Esc the menu's `onEsc`, a character its `execKey`. An edit field
    /// being edited takes the typed `text` and Backspace. False when the
    /// menu does nothing with it.
    pub(crate) fn key(&mut self, code: KeyCode, text: Option<&str>) -> bool {
        if !self.active() {
            return false;
        }
        if self.menus.editing() {
            let mut out = Vec::new();
            match code {
                KeyCode::Backspace => {
                    self.menus.key(MenuKey::Char(127), &self.gs, &mut out);
                    log_unsent(out);
                    return true;
                }
                KeyCode::Escape
                | KeyCode::Enter
                | KeyCode::NumpadEnter
                | KeyCode::Tab
                | KeyCode::ArrowUp
                | KeyCode::ArrowDown => {}
                _ => {
                    if let Some(t) = text {
                        self.menus.type_text(t);
                    }
                    return true;
                }
            }
        }
        if self.menus.waiting_for_key() {
            if let Some(name) = crate::console::keys::key_name(code) {
                self.menus.bind_key(name);
            }
            return true;
        }
        let key = match code {
            KeyCode::Escape => MenuKey::Escape,
            KeyCode::Tab | KeyCode::ArrowDown => MenuKey::Next,
            KeyCode::ArrowUp => MenuKey::Prev,
            KeyCode::ArrowLeft => MenuKey::Left,
            KeyCode::ArrowRight => MenuKey::Right,
            KeyCode::Enter | KeyCode::NumpadEnter => MenuKey::Enter,
            c => {
                use crate::console::keys;
                match keys::key_name(c).map(keys::number) {
                    Some(n @ 33..=126) => MenuKey::Char(n as u8),
                    _ => return false,
                }
            }
        };
        let mut out = Vec::new();
        let taken = self.menus.key(key, &self.gs, &mut out);
        log_unsent(out);
        taken || key == MenuKey::Escape
    }

    /// The mouse: raw counts `d` (a captured pointer) or a window position
    /// `at` on a `size` window.
    pub(crate) fn mouse(&mut self, d: Option<[f32; 2]>, at: Option<[f32; 2]>, size: (f32, f32)) {
        let out = self.menus.mouse_move(d, at, size, &self.gs);
        log_unsent(out);
    }

    /// A mouse button or wheel notch by its key name: a bind row waiting
    /// takes it, MOUSE1 clicks, the wheel scrolls a focused list.
    pub(crate) fn button(&mut self, key: &str, size: (f32, f32)) {
        if self.menus.waiting_for_key() {
            self.menus.bind_key(key);
        } else if key == "MOUSE1" {
            let out = self.menus.click(size, &self.gs);
            log_unsent(out);
        } else if let Some(up) = match key {
            "MWHEELUP" => Some(true),
            "MWHEELDOWN" => Some(false),
            _ => None,
        } {
            self.menus.wheel(up);
        }
    }

    /// MOUSE1 up: a drag ends.
    pub(crate) fn release(&mut self) {
        self.menus.release();
    }

    /// The open menus' quads on a `size` window, the cursor over them,
    /// on black: with no game up nothing else draws, and a popup that
    /// covers part of the screen (`error_popmenu`) shows black around it.
    pub(crate) fn quads(&self, size: (f32, f32)) -> Vec<HudQuad> {
        let Some(a) = self.hud.menu_assets() else {
            return Vec::new();
        };
        let now = self.start.elapsed().as_millis() as i32;
        let (w, h) = size;
        let mut out = vec![HudQuad {
            verts: [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba: [0.0, 0.0, 0.0, 1.0],
            texture: "white".into(),
        }];
        out.extend(crate::cod4_menu::quads(&self.menus, &a, size, now, true));
        out
    }

    /// The texts the open menus draw and each one's baseline, for tests.
    #[cfg(test)]
    pub(crate) fn texts(&self, size: (f32, f32)) -> Vec<(String, [f32; 2])> {
        let Some(a) = self.hud.menu_assets() else {
            return Vec::new();
        };
        crate::cod4_menu::paint(&self.menus, &a, size, 0, true).1
    }

    /// The console lines the menus ran.
    pub(crate) fn take_console(&mut self) -> Vec<String> {
        self.menus.take_console()
    }

    /// What the menus asked the client for.
    pub(crate) fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.menus.front.requests)
    }

    /// The aliases the menus played.
    pub(crate) fn take_sounds(&mut self) -> Vec<String> {
        std::mem::take(&mut self.menus.front.sounds)
    }

    /// The browser's frame (replies read, pings sent), and the join its
    /// Join Server asked for.
    pub(crate) fn frame(&mut self, now: Instant) -> Option<JoinRequest> {
        let upload = self.menus.take_upload_stats();
        self.save_stats(upload);
        self.menus.pump_browser(now);
        self.menus.take_join()
    }

    /// The zones' sounds, once each time the front end comes up over a
    /// fresh audio table and search path: `None` once handed over.
    fn sounds_to_install(&mut self) -> Option<Arc<vcod_cod4::sound::SoundSet>> {
        if self.sounds_installed {
            return None;
        }
        self.sounds_installed = true;
        self.sounds.clone()
    }

    /// The material a HUD texture name draws, for tests.
    #[cfg(test)]
    pub(crate) fn texture_of(&self, material: &str) -> Option<String> {
        self.hud.texture_of(material)
    }
}

impl App {
    /// The front end is up, or starts: on a CoD4 install with no game up
    /// the zones load (under `fs_game`) and `main` opens, with
    /// `error_popmenu` over it when `error` says why the last game ended
    /// (`notice`: the server ended it).
    pub(crate) fn enter_cod4_menu(&mut self, error: Option<(&str, bool)>) -> bool {
        let fs_game = self.cod4_fs_game();
        let Some(f) = self.session.cod4_front_mut() else {
            return false;
        };
        if !f.load(fs_game.as_deref()) {
            log::error!("cod4 front end: the zones' menus did not load");
        }
        f.open_main(&self.shell, error);
        true
    }

    /// The `fs_game` the front end runs under: the Mods menu's pick
    /// (`mods/<dir>`), else none.
    fn cod4_fs_game(&self) -> Option<String> {
        self.shell
            .cvar("fs_game")
            .filter(|g| !g.is_empty())
            .map(str::to_string)
    }

    /// `connect <addr>` on a CoD4 install, or the browser's join with its
    /// `password`: the front end's menus close, and the join and the map's
    /// load run behind the connect screen (`cod4_loading`) until
    /// `cod4_join_frame` hands the game over or [`Self::cod4_join_failed`]
    /// brings the menu back.
    pub(crate) fn connect_cod4(&mut self, addr: String, password: String) {
        if matches!(self.session, Session::Cod4(_)) {
            self.disconnect(None);
        }
        // A join already in flight is called off for this one.
        if let Some(l) = self.cod4_loading.take() {
            l.cancel();
        }
        let resolved = std::net::ToSocketAddrs::to_socket_addrs(addr.as_str())
            .ok()
            .and_then(|mut a| a.next());
        let Some(at) = resolved else {
            self.cod4_join_failed(&format!("cannot resolve {addr}"));
            return;
        };
        if let Some(f) = self.session.cod4_front_mut() {
            f.close();
        }
        log::info!("connecting to {addr}");
        vcod_common::laps::begin_load(format!("cod4 connect {addr}"));
        self.connect_addr = Some(addr.clone());
        self.cod4_loading = Some(Loader::start(JoinArgs {
            addr: at,
            server: addr,
            name: self.shell.cvar("name").unwrap_or("vcod").to_string(),
            password,
            join: crate::play::cod4_menus::Join {
                team: self.team.clone(),
                class: None,
            },
            game_dir: self.game_dir.clone(),
            // `cl_allowDownload` and `cl_wwwDownload`, on by default as
            // 1.7's (cod4-handshake.md 11).
            allow_download: self.shell.cvar("cl_allowDownload").is_none_or(|v| v != "0"),
            www_download: self.shell.cvar("cl_wwwDownload").is_none_or(|v| v != "0"),
        }));
    }

    /// Where CoD4 demos go and are looked for: `FS_FOpenFileWrite`'s game
    /// directory, the mod's (`fs_game`) before `main/`, each's `demos/`
    /// (cod4-demos.md 1).
    pub(crate) fn cod4_demo_dirs(&self) -> Vec<PathBuf> {
        self.cod4_fs_game()
            .iter()
            .map(|g| self.game_dir.join(g))
            .chain([self.game_dir.join("main")])
            .map(|d| d.join("demos"))
            .collect()
    }

    /// `demo <name>` / `timedemo <name>` on a CoD4 install (`CL_PlayDemo_f`,
    /// `iw3mp.exe` 0x469120): any game left first, `demos/<name>.dm_1` (a
    /// name already ending so kept) read to its gamestate and its map
    /// loaded behind the connect screen, then played as a watched session.
    /// A missing file is `EXE_ERR_NOT_FOUND`.
    pub(crate) fn play_cod4_demo(&mut self, name: &str, timedemo: bool) {
        if matches!(self.session, Session::Cod4(_)) {
            self.disconnect(None);
        }
        if let Some(l) = self.cod4_loading.take() {
            l.cancel();
        }
        let file = vcod_cod4::net::demo::file_name(name);
        let Some(data) = self
            .cod4_demo_dirs()
            .into_iter()
            .map(|d| d.join(&file))
            .find_map(|p| std::fs::read(p).ok())
        else {
            // `Com_Error`'s text goes through `SEH_LocalizeTextMessage`.
            let why = format!("EXE_ERR_NOT_FOUND\u{15}demos/{file}");
            self.cod4_join_failed(&crate::play::cod4::localize_drop(&why, &self.game_dir));
            return;
        };
        if let Some(f) = self.session.cod4_front_mut() {
            f.close();
        }
        log::info!("playing demos/{file}");
        self.connect_addr = None;
        self.cod4_loading = Some(Loader::start_demo(
            name.to_string(),
            data,
            timedemo,
            self.game_dir.clone(),
        ));
    }

    /// A CoD4 join, or its map's load, that failed: the main menu comes
    /// back saying why, a front end made for a client that started on
    /// `--connect`. A server Start New Server launched for the join stops.
    pub(crate) fn cod4_join_failed(&mut self, why: &str) {
        log::error!("{why}");
        self.cod4_back_to_menu(Some(why));
    }

    /// Esc on the connect screen (`CL_Disconnect`): the join
    /// in flight is called off, a connection it holds
    /// says `disconnect`, and the main menu comes back with no error.
    /// False with no join in flight.
    pub(crate) fn cancel_cod4_join(&mut self) -> bool {
        let Some(l) = self.cod4_loading.take() else {
            return false;
        };
        l.cancel();
        log::info!("cod4: connect cancelled");
        self.cod4_back_to_menu(None);
        true
    }

    /// The main menu after a join that did not take, over `error_popmenu`
    /// with `why` when there is one. A server Start New Server launched
    /// for the join stops.
    fn cod4_back_to_menu(&mut self, why: Option<&str>) {
        self.server_process = self.server_process.take().filter(|s| !s.joined);
        let error = why.map(|w| (w, false));
        if matches!(self.session, Session::Cod4(_)) {
            self.leave_cod4_game(error);
            return;
        }
        if self.session.cod4_front_mut().is_none() {
            let front = self
                .cod4_parked
                .take()
                .unwrap_or_else(|| Box::new(Cod4Front::new(self.game_dir.clone())));
            self.session = Session::Cod4Menu(front);
            let fs_game = self.cod4_fs_game();
            match cod4_fly::open_fs(&self.game_dir, fs_game.as_deref(), None) {
                Ok(fs) => self.fs = fs,
                Err(e) => log::error!("cod4: {e:#}"),
            }
        }
        self.mode = Mode::Idle;
        self.set_grab(false);
        self.set_title(version_tag());
        self.enter_cod4_menu(error);
    }

    /// The CoD4 game ends (`CL_Disconnect`): the server is left, the map
    /// goes, and the front end comes back on the stock search path.
    pub(crate) fn leave_cod4(&mut self) {
        if !matches!(self.session, Session::Cod4(_)) {
            return;
        }
        self.session.shutdown();
        let root = self.game_dir.clone();
        let front = self
            .cod4_parked
            .take()
            .unwrap_or_else(|| Box::new(Cod4Front::new(root)));
        let mut front = front;
        front.sounds_installed = false;
        self.session = Session::Cod4Menu(front);
        let fs_game = self.cod4_fs_game();
        match cod4_fly::open_fs(&self.game_dir, fs_game.as_deref(), None) {
            Ok(fs) => self.fs = fs,
            Err(e) => log::error!("cod4: {e:#}"),
        }
    }

    /// A CoD4 game the server ended: [`Self::disconnect`]'s steps, with the
    /// reason titled as the server's notice when `error` says so.
    pub(crate) fn leave_cod4_game(&mut self, error: Option<(&str, bool)>) {
        if self.server_process.as_ref().is_some_and(|s| s.joined) {
            self.server_process = None;
        }
        self.unload_world();
        self.leave_cod4();
        self.mode = Mode::Idle;
        self.set_grab(false);
        self.set_title(version_tag());
        self.enter_cod4_menu(error);
    }

    /// The front end's frame: its sounds and music, the browser's work and
    /// its join, and the requests (a server to start, a mod to run).
    pub(crate) fn cod4_front_requests(&mut self) {
        let Some(f) = self.session.cod4_front_mut() else {
            return;
        };
        if let Some(set) = f.sounds_to_install() {
            self.audio.set_cod4_aliases(crate::cod4_audio::table(&set));
            for (path, wav) in crate::cod4_audio::overlay_files(&set) {
                self.fs.overlay(&path, wav);
            }
        }
        for alias in f.take_sounds() {
            self.audio.play_local(&self.fs, &alias);
        }
        // `music_mainmenu_mp` starts again whenever it is not playing while
        // a menu that names it draws.
        match f.active().then(|| f.menus.music()).flatten() {
            Some(m) => self.audio.music_play(&self.fs, m),
            None => self.audio.music_stop(0),
        }
        let join = f.frame(Instant::now());
        let requests = f.take_requests();
        if let Some(j) = join {
            self.connect_cod4(j.addr.to_string(), j.password);
        }
        for r in requests {
            match r {
                Request::StartServer {
                    map,
                    gametype,
                    dedicated,
                    cvars,
                } => self.start_cod4_server(map, gametype, dedicated, cvars),
                Request::RunMod(game) => self.run_cod4_mod(game),
                Request::LoadProfile(name) => self.load_cod4_profile(&name),
                Request::ProfileDeleted(name) => self.cod4_profile_deleted(&name),
            }
        }
    }

    /// Start New Server: `vcod-server --game cod4` beside this executable
    /// with the menu's map, game type and settings; Dedicated No joins it
    /// once it answers, as CoD1's front end does (no listen server).
    fn start_cod4_server(
        &mut self,
        map: String,
        gametype: String,
        dedicated: i32,
        cvars: Vec<(String, String)>,
    ) {
        use crate::frontend::create;
        self.server_process = None;
        let req = create::StartServer {
            map,
            gametype,
            dedicated,
            cvars: cvars.iter().map(|(k, _)| k.clone()).collect(),
            cod2: false,
        };
        let fs_game = self.cod4_fs_game();
        let launched = (|| {
            let exe = create::server_exe()?;
            let port = create::free_port(28960).context("no free UDP port from 28960")?;
            let value = |n: &str| cvars.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
            let dirs = (self.game_dir.as_path(), "main", fs_game.as_deref(), false);
            let mut args = create::server_args(&req, port, dirs, &value);
            args.extend(["--game".into(), "cod4".into()]);
            let log = self.config_path.with_file_name("vcod-server.log");
            log::info!("starting {} {}", exe.display(), args.join(" "));
            create::ServerProcess::spawn(&exe, &args, port, log, dedicated == 0)
        })();
        match launched {
            Ok(server) => {
                console::log::print(&format!(
                    "vcod-server on port {}, log in {}",
                    server.port,
                    server.log.display()
                ));
                self.server_process = Some(server);
            }
            Err(e) => {
                let why = format!("{e:#}");
                log::error!("{why}");
                self.enter_cod4_menu(Some((&why, false)));
            }
        }
    }

    /// `Com_ChangePlayerProfile`:
    /// `players/profiles/active.txt` names the profile and its
    /// config takes over, vcod's `vcod_mp.cfg` there, else 1.7's
    /// `config_mp.cfg`; with neither, `name` takes the profile's name as
    /// `Com_SetPlayerProfile` gives an empty one. The config is written at
    /// once, as every later change writes it. Retail resets every dvar
    /// first; vcod keeps what the file does not set (cod4-front-end.md 3).
    fn load_cod4_profile(&mut self, name: &str) {
        let Some(config) = change_profile(&self.game_dir, name, &mut self.shell) else {
            return;
        };
        self.config_path = config;
        if let Some(f) = self.session.cod4_front_mut() {
            f.sync(&self.shell);
            f.reload_stats();
        }
        log::info!("player profile {name}: {}", self.config_path.display());
    }

    /// A deleted profile that held the config: the config goes back to
    /// `main/`'s until another profile is loaded.
    fn cod4_profile_deleted(&mut self, name: &str) {
        if let Some(f) = self.session.cod4_front_mut() {
            f.reload_stats();
        }
        let gone = self
            .config_path
            .parent()
            .and_then(|d| d.file_name())
            .is_some_and(|d| d.to_string_lossy().eq_ignore_ascii_case(name))
            && !self.config_path.exists();
        if gone {
            self.config_path = self.game_dir.join("main").join(crate::CONFIG_FILE);
            log::info!(
                "player profile {name} deleted; config {}",
                self.config_path.display()
            );
        }
    }

    /// The Mods menu's `RunMod` / `ClearMods`: `fs_game` and the front
    /// end's zones again under it (`vid_restart`), which brings the main
    /// menu back up off the mod's `mod.ff`.
    fn run_cod4_mod(&mut self, game: Option<String>) {
        let value = game.clone().unwrap_or_default();
        let _ = self.shell.force("fs_game", &value);
        crate::play::cod4_profile::set_context(&self.game_dir, &value);
        match cod4_fly::open_fs(&self.game_dir, game.as_deref(), None) {
            Ok(fs) => self.fs = fs,
            Err(e) => log::error!("cod4: {e:#}"),
        }
        if let Some(r) = &mut self.renderer {
            r.reopen(&self.fs);
        }
        log::info!("fs_game {value:?}");
        self.enter_cod4_menu(None);
    }
}

/// [`App::load_cod4_profile`]'s files: `active.txt` written, the
/// profile's config run (vcod's, else 1.7's), `name` given the profile's
/// when it has none, and vcod's config written there. Returns that
/// config's path; `None` when the profile's directory is gone.
fn change_profile(root: &std::path::Path, name: &str, shell: &mut Shell) -> Option<PathBuf> {
    use crate::play::cod4_menus::front::{profile_dir, profiles_root};
    let Some(dir) = profile_dir(root, name) else {
        log::warn!("cod4: no profile {name:?}");
        return None;
    };
    let active = profiles_root(root).join("active.txt");
    if let Err(e) = std::fs::write(&active, name) {
        log::warn!("cannot write {}: {e}", active.display());
    }
    let config = dir.join(crate::CONFIG_FILE);
    let text = std::fs::read_to_string(&config)
        .or_else(|_| std::fs::read_to_string(dir.join("config_mp.cfg")))
        .ok();
    if let Some(t) = &text {
        let _ = shell.exec_config(t);
    }
    // the config is written below either way
    let _ = shell.upgrade_config(vcod_common::game::Game::Cod4);
    if text.is_none() || shell.cvar("name").is_none_or(str::is_empty) {
        let _ = shell.force("name", name);
    }
    if let Err(e) = std::fs::write(&config, shell.config_text()) {
        log::warn!("cannot write {}: {e}", config.display());
    }
    Some(config)
}

/// With no server the menus answer nobody: a `scriptMenuResponse` in a
/// front-end menu goes nowhere, as retail's does with no connection.
fn log_unsent(out: Vec<String>) {
    for line in out {
        log::debug!("cod4 front end: {line} not sent, no server");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use vcod_cod4::scriptmenu::Cmd;

    /// The front end over the `COD4_DIR` install, its main menu up with
    /// vcod's stock CoD4 console.
    fn front() -> Option<(Cod4Front, Shell)> {
        let install = vcod_cod4::testing::cod4_install()?;
        let mut f = Cod4Front::new(install.root().to_path_buf());
        // A profile of the test's own, active, so `main` opens over no
        // profile pick and nothing is written into the install.
        let players = tempfile::tempdir().unwrap().keep();
        std::fs::create_dir_all(players.join("players/profiles/tester")).unwrap();
        std::fs::write(players.join("players/profiles/active.txt"), "tester").unwrap();
        f.menus.front.players_root = Some(players);
        // No `servercache.dat` of the install's: its servers are not
        // pinged, and nothing is written into it.
        f.menus.browser = Browser::default();
        f.menus.browser.lan_targets.clear();
        assert!(f.load(None), "the front end's zones load");
        let mut shell = Shell::new();
        shell.use_cod4_defaults(false);
        f.open_main(&shell, None);
        // The console runs what the menus set: the fresh profile's class
        // names.
        for line in f.take_console() {
            shell.execute(&line);
        }
        Some((f, shell))
    }

    fn names(f: &Cod4Front) -> Vec<String> {
        f.texts(SIZE).into_iter().map(|(s, _)| s).collect()
    }

    const SIZE: (f32, f32) = (640.0, 480.0);

    /// Moves the mouse onto `label` and clicks it.
    fn click(f: &mut Cod4Front, label: &str) {
        let texts = f.texts(SIZE);
        let Some(p) = texts.iter().find(|(s, _)| s == label).map(|(_, p)| *p) else {
            panic!("no {label:?} in {:?}", names(f));
        };
        f.mouse(None, Some([p[0] + 2.0, p[1] - 4.0]), SIZE);
        f.button("MOUSE1", SIZE);
    }

    /// Clicks the middle of the open menu's first item `pick` names.
    fn click_item(f: &mut Cod4Front, pick: impl Fn(&vcod_cod4::scriptmenu::LayoutItem) -> bool) {
        let open = f.menus.open().unwrap();
        let layout = open.layout.as_ref().unwrap();
        let Some(it) = layout.items.iter().find(|it| pick(it)) else {
            panic!(
                "no such item in {}: {:?}",
                open.name,
                layout.items.iter().map(|i| &i.action).collect::<Vec<_>>()
            );
        };
        let sp = crate::hud::cod2elem::ScreenPlacement::new(SIZE.0, SIZE.1);
        let [x, y, w, h] = crate::play::cod4_menus::placed(&sp, f.menus.rect(it, layout));
        f.mouse(None, Some([x + 0.5 * w, y + 0.5 * h]), SIZE);
        f.button("MOUSE1", SIZE);
    }

    fn stack(f: &Cod4Front) -> Vec<String> {
        f.menus.drawn().iter().map(|o| o.name.clone()).collect()
    }

    /// Bare `vcod` on a CoD4 install: `main` opens `main_text`, drawn from
    /// `ui_mp` with its ten rows; the background's animated layers draw
    /// their materials.
    #[test]
    fn cod4_front_main_menu_draws_from_ui_mp() {
        let Some((f, _)) = front() else {
            return;
        };
        assert_eq!(stack(&f), ["main_text"]);
        let n = names(&f);
        for t in [
            "Join Game",
            "Start New Server",
            "Select Profile",
            "Create a Class",
            "Rank & Challenges",
            "Controls",
            "Options",
            "Mods",
            "Single Player",
            "Quit",
        ] {
            assert!(n.iter().any(|s| s == t), "{t} in {n:?}");
        }
        let quads = f.quads(SIZE);
        for m in ["animbg_back", "animbg_fogscroll", "ui_cursor"] {
            let tex = f.texture_of(m).unwrap_or_else(|| panic!("{m}"));
            assert!(quads.iter().any(|q| q.texture == tex), "{m} drawn");
        }
    }

    /// The main menu reads the active profile's `vcod_mpdata`: Create a
    /// Class is locked off a fresh profile and opens `pc_cac_popup` once
    /// the file holds stat 260 (`feature_cac`, 2 when the rank scripts
    /// unlock it); its `statset 260 1` reaches the file.
    #[test]
    fn cod4_front_create_a_class_unlocks_off_the_profiles_stats() {
        let Some((mut f, shell)) = front() else {
            return;
        };
        click(&mut f, "Create a Class");
        assert_eq!(stack(&f), ["main_text"], "locked on a fresh profile");
        let file = f.menus.front.players_root.clone().unwrap();
        let file = file.join("players/profiles/tester/vcod_mpdata");
        let mut earned = Stats::fresh();
        earned.set(260, 2);
        earned.seal();
        std::fs::write(&file, &earned.0[..]).unwrap();
        f.open_main(&shell, None);
        assert_eq!(f.menus.stat(260), 2);
        click(&mut f, "Create a Class");
        assert_eq!(stack(&f), ["main_text", "pc_cac_popup"]);
        f.close();
        let saved = std::fs::read(&file).unwrap();
        assert_eq!(saved[4 + 260], 1);
    }

    /// Clicks the open menu's first visible item whose action holds a
    /// command `pick` takes.
    fn pick(f: &mut Cod4Front, pick: impl Fn(&Cmd) -> bool) {
        let open = f.menus.open().unwrap();
        let layout = open.layout.as_ref().unwrap();
        let at = layout
            .items
            .iter()
            .position(|it| f.menus.item_visible(it) && it.action.iter().any(&pick));
        let Some(at) = at else {
            panic!("no such visible item in {}", open.name);
        };
        let sp = crate::hud::cod2elem::ScreenPlacement::new(SIZE.0, SIZE.1);
        let r = f.menus.rect(&layout.items[at], layout);
        let [x, y, w, h] = crate::play::cod4_menus::placed(&sp, r);
        f.mouse(None, Some([x + 0.5 * w, y + 0.5 * h]), SIZE);
        f.button("MOUSE1", SIZE);
    }

    fn opens(menu: &'static str) -> impl Fn(&Cmd) -> bool {
        move |c| matches!(c, Cmd::Open(m) if m.eq_ignore_ascii_case(menu))
    }

    /// A `statsetusingtable` of stat `stat` off the row named `name`.
    fn sets(stat: &'static str, name: &str) -> impl Fn(&Cmd) -> bool {
        let name = name.to_string();
        move |c| {
            matches!(c, Cmd::StatSetUsingTable { index, value, .. }
                if index == stat && value.eq_ignore_ascii_case(&name))
        }
    }

    /// A `mp/statstable.csv` row's column `ret`, by its name (column 4)
    /// or, for a perk, its stat (column 1).
    fn stats_row(f: &Cod4Front, column: i32, key: &str, ret: i32) -> String {
        use vcod_cod4::menuexp::Env;
        f.menus
            .env()
            .table_lookup("mp/statstable.csv", column, key, ret)
    }

    /// What [`edit_custom_class`] picked: the primary's and the special
    /// grenade's numbers, the perk 2's stat and name.
    struct Picks {
        primary: i32,
        perk: i32,
        perk_name: String,
        grenade: i32,
    }

    /// Opens Create a Class on a rank 3 profile whose AK-47 the rank
    /// scripts unlocked, and in the first custom class picks the AK-47
    /// with no attachment and no camo, a perk 2 of a fresh profile other
    /// than the class's, and the flash grenade; Esc leaves the class.
    fn edit_custom_class(f: &mut Cod4Front, shell: &Shell, file: &std::path::Path) -> Picks {
        let atoi = vcod_cod4::menuexp::atoi;
        let primary = atoi(&stats_row(f, 4, "ak47", 0));
        let mut earned = Stats::fresh();
        earned.set(260, 2);
        earned.set(2301, 300);
        earned.set(3000 + primary as u16, 0x10001);
        earned.seal();
        std::fs::write(file, &earned.0[..]).unwrap();
        f.open_main(shell, None);
        click(f, "Create a Class");
        pick(f, opens("menu_cac_assault"));
        pick(f, opens("assault_popup_cac_primary"));
        pick(f, opens("assault_popup_cac_assault"));
        pick(f, sets("201", "ak47"));
        assert_eq!(stack(f).last().unwrap(), "assault_attachment_popup_assault");
        pick(f, sets("202", "none"));
        if stack(f).last().unwrap() == "assault_popup_cac_camo" {
            pick(f, sets("209", "camo_none"));
        }
        assert_eq!(stack(f).last().unwrap(), "menu_cac_assault");
        // Fresh profiles hold perk 2's 156, 160 and 167; the class has 160.
        let perk = [156, 167]
            .into_iter()
            .find(|&p| p != f.menus.stat(206))
            .unwrap();
        let perk_name = stats_row(f, 1, &perk.to_string(), 4);
        pick(f, opens("assault_popup_cac_perk2"));
        pick(f, sets("206", &perk_name));
        pick(f, opens("assault_popup_cac_extra"));
        pick(f, sets("208", "flash_grenade"));
        let grenade = atoi(&stats_row(f, 4, "flash_grenade", 0));
        assert!(f.key(KeyCode::Escape, None));
        Picks {
            primary,
            perk,
            perk_name,
            grenade,
        }
    }

    /// Create a Class's picks write the class's stats (cod4-front-end.md
    /// 4.1): the AK-47's `new` bit clears as it is picked, the Esc's
    /// `uploadstats` writes the file at the next frame, and the class
    /// page shows the picks when it opens again.
    #[test]
    fn cod4_front_create_a_class_picks_reach_the_profile() {
        let Some((mut f, shell)) = front() else {
            return;
        };
        let players = f.menus.front.players_root.clone().unwrap();
        let file = players.join("players/profiles/tester/vcod_mpdata");
        let p = edit_custom_class(&mut f, &shell, &file);
        let read = |f: &Cod4Front| [201, 202, 206, 208, 209].map(|i| f.menus.stat(i));
        let want = [p.primary, 0, p.perk, p.grenade, 0];
        assert_eq!(read(&f), want);
        assert_eq!(
            f.menus.stat(3000 + p.primary),
            1,
            "the AK-47 is no longer new"
        );
        assert_eq!(f.menus.stat(p.perk), 1, "an earned perk stays earned");
        assert_eq!(stack(&f), ["main_text"]);
        f.frame(Instant::now());
        let saved = Stats(Box::new(std::fs::read(&file).unwrap().try_into().unwrap()));
        assert_eq!(
            [201, 202, 206, 208, 209].map(|i| saved.get(i).unwrap() as i32),
            want
        );
        click(&mut f, "Create a Class");
        // A fresh profile's classes took `CLASS_SLOT<n>`'s names.
        let slot = f.translate("CLASS_SLOT1");
        assert!(names(&f).contains(&slot), "{slot} in {:?}", names(&f));
        pick(&mut f, opens("menu_cac_assault"));
        // The perk's row names its string in one of its columns.
        let perk_key = (0..10)
            .map(|c| stats_row(&f, 1, &p.perk.to_string(), c))
            .find_map(|v| {
                v.trim_start_matches('@')
                    .strip_prefix("PERKS_")
                    .map(str::to_string)
            })
            .expect("the perk's string");
        let n = names(&f);
        for key in ["WEAPON_AK47".to_string(), format!("PERKS_{perk_key}")] {
            let shown = f.translate(&key);
            assert!(n.contains(&shown), "{shown} in {n:?}");
        }
    }

    /// The editor's class, joined: vcod-server's `custom1` spawns with the
    /// AK-47, the perk and the flash grenade the editor picked, read off
    /// the stats the join sent from the profile's file.
    #[test]
    fn cod4_front_create_a_class_spawns_with_its_picks() {
        let Some((mut f, shell)) = front() else {
            return;
        };
        let players = f.menus.front.players_root.clone().unwrap();
        let file = players.join("players/profiles/tester/vcod_mpdata");
        let p = edit_custom_class(&mut f, &shell, &file);
        f.close();
        let (spawned, perks, weapons) =
            crate::play::cod4::tests::live_join_profile(players, "custom1,0", |s, spawned| {
                let gs = s.gamestate().unwrap();
                let names = crate::play::cod4::weapon_list(gs).to_vec();
                let ps = &s.client().snapshots.last().unwrap().ps;
                let held: Vec<String> = (0..names.len())
                    .filter(|&w| {
                        ps.get(&format!("weapons[{}]", w / 32)).unwrap_or(0) & (1 << (w % 32)) != 0
                    })
                    .map(|w| names[w].clone())
                    .collect();
                (spawned, ps.get("perks").unwrap_or(0), held)
            })
            .expect("the install front() found");
        let spawned = spawned.expect("spawned");
        assert!(spawned.ends_with("ak47_mp"), "{spawned}");
        assert!(
            weapons.iter().any(|w| w == "flash_grenade_mp"),
            "{weapons:?}"
        );
        let bit = PERK_NAMES
            .iter()
            .position(|n| *n == p.perk_name)
            .unwrap_or_else(|| panic!("{} is a perk", p.perk_name));
        assert_ne!(perks & (1 << bit), 0, "{} in {perks:#x}", p.perk_name);
    }

    /// `bg_perkNames`, the `perks` bit order (cod4-gsc.md, "The perks").
    const PERK_NAMES: [&str; 20] = [
        "specialty_gpsjammer",
        "specialty_bulletaccuracy",
        "specialty_fastreload",
        "specialty_rof",
        "specialty_holdbreath",
        "specialty_bulletpenetration",
        "specialty_grenadepulldeath",
        "specialty_pistoldeath",
        "specialty_quieter",
        "specialty_parabolic",
        "specialty_longersprint",
        "specialty_detectexplosive",
        "specialty_explosivedamage",
        "specialty_exposeenemy",
        "specialty_bulletdamage",
        "specialty_extraammo",
        "specialty_twoprimaries",
        "specialty_armorvest",
        "specialty_fraggrenade",
        "specialty_specialgrenade",
    ];

    /// Quit opens `quit_popmenu` over `main_text`, both drawn; No closes
    /// it back to the main menu, Yes runs `uiScript quit`, the client's
    /// `quit`. Esc backs out of a popup as its `onEsc` does.
    #[test]
    fn cod4_front_quit_asks_first() {
        let Some((mut f, _)) = front() else {
            return;
        };
        click(&mut f, "Quit");
        assert_eq!(stack(&f), ["main_text", "quit_popmenu"]);
        assert!(
            names(&f).iter().any(|s| s == "Join Game"),
            "the menu under it draws"
        );
        click(&mut f, "No");
        assert_eq!(stack(&f), ["main_text"]);
        assert!(f.take_console().is_empty());
        click(&mut f, "Quit");
        assert!(f.key(KeyCode::Escape, None));
        assert_eq!(stack(&f), ["main_text"]);
        click(&mut f, "Quit");
        click(&mut f, "Yes");
        assert_eq!(f.take_console(), ["quit"]);
    }

    /// Start New Server closes `main_text` for `createserver`: the game
    /// type and map lists from the zones, `ui_dedicated` stepping No, LAN,
    /// Internet; Start asks for the picked map and game type, at the
    /// startup configs' `sv_maxclients`. Back (and
    /// Esc) closes it, whose `onClose` opens `main_text` again.
    #[test]
    fn cod4_front_start_new_server_asks_for_its_server() {
        let Some((mut f, _)) = front() else {
            return;
        };
        click(&mut f, "Start New Server");
        assert_eq!(stack(&f), ["createserver"]);
        let n = names(&f);
        for t in [
            "Free-for-all",
            "Ambush",
            "Backlot",
            "Crash",
            "Start",
            "Back",
        ] {
            assert!(n.iter().any(|s| s == t), "{t} in {n:?}");
        }
        // The Dedicated row steps on a click.
        click(&mut f, "Dedicated:  ");
        assert!(names(&f).iter().any(|s| s == "LAN"), "{:?}", names(&f));
        // The game type steps through `_gametypes.txt`'s list.
        click_item(&mut f, |it| it.owner_draw == 245);
        let gametype = f.hud.ui_lists().unwrap().gametypes[1].0.clone();
        click(&mut f, "Backlot");
        click(&mut f, "Start");
        assert_eq!(
            f.take_requests(),
            [Request::StartServer {
                map: "mp_backlot".into(),
                gametype,
                dedicated: 1,
                // The startup configs' `sv_maxclients 24`, which the
                // form's Max Players field shows.
                cvars: vec![("sv_maxclients".into(), "24".into())],
            }]
        );
        assert!(f.key(KeyCode::Escape, None));
        assert_eq!(stack(&f), ["main_text"]);
    }

    /// Every control of the open menu with a value draws it on its own
    /// item's row: the text's middle inside the item's rect, its start in
    /// the rect's width (`Item_Paint`'s value paints). Returns how many it
    /// checked.
    fn values_in_their_rects(f: &Cod4Front) -> usize {
        let open = f.menus.open().unwrap();
        let layout = open.layout.as_ref().unwrap();
        let sp = crate::hud::cod2elem::ScreenPlacement::new(SIZE.0, SIZE.1);
        let a = f.hud.menu_assets().unwrap();
        let texts = f.texts(SIZE);
        let mut checked = 0;
        for it in layout.items.iter().filter(|it| f.menus.item_visible(it)) {
            if !matches!(it.kind, 4 | 8 | 9 | 11 | 12 | 14) {
                continue;
            }
            let Some(raw) = f.menus.control_text(usize::MAX, it) else {
                continue;
            };
            let value = crate::cod4_menu::localize(&a, &raw);
            if value.is_empty() {
                continue;
            }
            let [x, y, w, h] = crate::play::cod4_menus::placed(&sp, f.menus.rect(it, layout));
            let half = 0.5 * (48.0 * it.text_scale) as i32 as f32 * sp.virtual_to_real[1];
            assert!(
                texts.iter().any(|(s, p)| *s == value
                    && (x..=x + w).contains(&p[0])
                    && (y..=y + h).contains(&(p[1] - half))),
                "{}: {value:?} for {:?} in {:?}: {texts:?}",
                open.name,
                it.dvar,
                [x, y, w, h]
            );
            checked += 1;
        }
        checked
    }

    /// The baseline of the drawn `label`.
    fn baseline(f: &Cod4Front, label: &str) -> [f32; 2] {
        let texts = f.texts(SIZE);
        texts
            .iter()
            .find(|(s, _)| s == label)
            .unwrap_or_else(|| panic!("no {label:?} in {texts:?}"))
            .1
    }

    /// Start New Server's values sit beside their labels: the game type
    /// owner draw at its rect's corner plus `textalignx` / `y`
    /// (`UI_OwnerDraw`), on the Game Mode row, stepping there on a click;
    /// the numeric fields (Maximum Players) take digits only and draw them
    /// 8 past their own text (`Item_TextField_Paint`).
    #[test]
    fn cod4_front_start_new_server_values_sit_beside_their_labels() {
        let Some((mut f, _)) = front() else {
            return;
        };
        click(&mut f, "Start New Server");
        assert!(values_in_their_rects(&f) >= 6);
        let gametypes = f.hud.ui_lists().unwrap().gametypes.clone();
        let a = f.hud.menu_assets().unwrap();
        let (first, second) = (
            a.translate(&gametypes[0].1).to_string(),
            a.translate(&gametypes[1].1).to_string(),
        );
        let label = baseline(&f, "Game Mode:  ");
        let at = baseline(&f, &first);
        assert!(
            (at[1] - label[1]).abs() <= 1.0,
            "{first} at {at:?}, label {label:?}"
        );
        assert!(at[0] > label[0]);
        click_item(&mut f, |it| it.owner_draw == 245);
        assert!((baseline(&f, &second)[1] - label[1]).abs() <= 1.0);
        assert!(!names(&f).contains(&first));

        click_item(&mut f, |it| it.dvar.as_deref() == Some("sv_maxclients"));
        assert!(f.menus.editing(), "a numeric field edits");
        // The field opens on the startup configs' 24.
        while f.menus.dvar("sv_maxclients").is_some_and(|v| !v.is_empty()) {
            assert!(f.key(KeyCode::Backspace, None));
        }
        assert!(f.key(KeyCode::Digit1, Some("1a8")));
        assert!(f.key(KeyCode::Enter, None));
        assert_eq!(f.menus.dvar("sv_maxclients").as_deref(), Some("18"));
        let label = baseline(&f, "Maximum Players:  ");
        let at = baseline(&f, "18");
        assert!(
            (at[1] - label[1]).abs() <= 1.0,
            "18 at {at:?}, label {label:?}"
        );
        assert!(values_in_their_rects(&f) >= 7);
    }

    /// The Options and Controls pages and Join Game draw every control's
    /// value on its own row; Join Game's game type owner draw draws
    /// its text, then its value past it. Game Options shows 1.7 MP's
    /// rows: `ui_multiplayer` 1 hides single player's subtitles and
    /// crosshair rows.
    #[test]
    fn cod4_front_option_pages_draw_their_values() {
        let Some((mut f, shell)) = front() else {
            return;
        };
        for (path, at_least) in [
            (&["Options", "Graphics..."][..], 10),
            (&["Options", "Texture Settings..."], 1),
            (&["Options", "Sound..."], 1),
            (&["Options", "Game Options..."], 2),
            (&["Controls", "Look..."], 5),
            (&["Controls", "Move..."], 5),
            (&["Controls", "Combat..."], 5),
            (&["Join Game"], 2),
        ] {
            f.open_main(&shell, None);
            for row in path {
                click(&mut f, row);
            }
            let n = values_in_their_rects(&f);
            assert!(n >= at_least, "{path:?}: {n}");
        }
        let label = baseline(&f, "Game Mode:  ");
        let all = baseline(&f, "All");
        assert!(all[0] > label[0] + 8.0 && (all[1] - label[1]).abs() <= 1.0);

        f.open_main(&shell, None);
        click(&mut f, "Options");
        click(&mut f, "Game Options...");
        let n = names(&f);
        assert!(n.iter().any(|s| s == "Show Blood"), "{n:?}");
        assert!(!n.iter().any(|s| s == "Enable Crosshair"), "{n:?}");
    }

    /// The graphics page's rows vcod has a cvar for take keys, the rest
    /// stay greyed; the video mode steps through `r_mode`'s entries in
    /// its `ui_r_mode` copy (`options_graphics.cfg`), and the video restart
    /// popup's Yes copies it back (`options_graphics_set.cfg`) before its
    /// `vid_restart`, which the client's console runs.
    #[test]
    fn cod4_graphics_page_applies_the_video_mode() {
        let Some((mut f, mut shell)) = front() else {
            return;
        };
        shell.register_cod4_options();
        shell.set_enum_domain(
            "r_mode",
            vec!["640x480".into(), "800x600".into(), "1920x1080".into()],
        );
        shell.set_enum_domain("r_displayRefresh", vec!["60 Hz".into(), "144 Hz".into()]);
        shell.execute("set r_mode 800x600");
        f.open_main(&shell, None);
        click(&mut f, "Options");
        click(&mut f, "Graphics...");
        assert_eq!(f.menus.dvar("ui_r_mode").as_deref(), Some("800x600"));
        let items = f.menus.open().unwrap().layout.clone().unwrap().items;
        let greyed = |f: &Cod4Front, d: &str| {
            let it = items
                .iter()
                .find(|it| it.dvar.as_deref() == Some(d))
                .unwrap();
            f.menus.greyed(it)
        };
        for d in [
            "ui_r_mode",
            "ui_r_displayRefresh",
            "ui_r_aasamples",
            "ui_r_vsync",
            "r_specular",
            "r_glow_allowed",
            "r_dlightlimit",
            "r_lodScaleRigid",
            "r_drawWater",
            "r_dof_enable",
            "ui_zfeather",
            "ragdoll_enable",
        ] {
            assert!(!greyed(&f, d), "{d} takes keys");
        }
        assert!(greyed(&f, "r_multiGpu"), "r_multiGpu stays greyed");
        // Soften Smoke Edges copies r_zFeather in on open and back on close
        assert_eq!(f.menus.dvar("ui_zfeather").as_deref(), Some("1"));
        click_item(&mut f, |it| it.dvar.as_deref() == Some("ui_zfeather"));
        assert_eq!(f.menus.dvar("ui_zfeather").as_deref(), Some("0"));
        click_item(&mut f, |it| it.dvar.as_deref() == Some("ui_r_mode"));
        assert_eq!(f.menus.dvar("ui_r_mode").as_deref(), Some("2"));
        assert!(
            names(&f).iter().any(|s| s == "1920x1080"),
            "{:?}",
            names(&f)
        );
        let opens_popup = |it: &vcod_cod4::scriptmenu::LayoutItem| {
            it.action.iter().any(
                |c| matches!(c, vcod_cod4::scriptmenu::Cmd::Open(m) if m == "vid_restart_popmenu"),
            )
        };
        click_item(&mut f, opens_popup);
        assert_eq!(f.menus.open().unwrap().name, "vid_restart_popmenu");
        click_item(&mut f, |it| {
            it.action
                .iter()
                .any(|c| matches!(c, vcod_cod4::scriptmenu::Cmd::Exec(l) if l == "vid_restart"))
        });
        let lines = f.take_console();
        assert!(lines.contains(&"set r_mode \"2\"".to_string()), "{lines:?}");
        assert_eq!(lines.last().map(String::as_str), Some("vid_restart"));
        for l in &lines {
            shell.execute(l);
        }
        assert_eq!(shell.cvar("r_mode"), Some("1920x1080"));
        // leaving the page sets Soften Smoke Edges' No (its `onClose`)
        while stack(&f).iter().any(|m| m == "options_graphics") {
            assert!(f.key(KeyCode::Escape, None));
        }
        let lines = f.take_console();
        assert!(
            lines.contains(&"set r_zfeather \"0\"".to_string()),
            "{lines:?}"
        );
        for l in &lines {
            shell.execute(l);
        }
        assert_eq!(shell.cvar("r_zFeather"), Some("0"));
    }

    /// The aspect ratio row steps its `ui_r_aspectratio` copy and the
    /// video restart popup latches `r_aspectRatio` for the `vid_restart`;
    /// Number of Corpses is single player's (`!ui_multiplayer`) and Dual
    /// Video Cards stays greyed. The texture page's picmip rows open on
    /// copies of the dvars and take keys whatever Texture Quality says,
    /// and its apply popup sets the dvars and runs `r_applyPicmip`; the
    /// anisotropy slider sets `r_texFilterAnisoMin` itself.
    #[test]
    fn cod4_graphics_pages_apply_the_aspect_and_the_picmip() {
        use crate::console::shell::Effect;
        let Some((mut f, mut shell)) = front() else {
            return;
        };
        shell.register_cod4_options();
        f.open_main(&shell, None);
        click(&mut f, "Options");
        click(&mut f, "Graphics...");
        let item = |f: &Cod4Front, d: &str| {
            let layout = f.menus.open().unwrap().layout.clone().unwrap();
            layout
                .items
                .into_iter()
                .find(|it| {
                    it.dvar
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(d))
                })
                .unwrap_or_else(|| panic!("no {d} row"))
        };
        assert_eq!(f.menus.dvar("ui_r_aspectratio").as_deref(), Some("auto"));
        assert!(!f.menus.greyed(&item(&f, "ui_r_aspectratio")));
        assert!(f.menus.greyed(&item(&f, "r_multiGpu")));
        assert!(!f.menus.item_visible(&item(&f, "ai_corpsecount")));
        click_item(&mut f, |it| it.dvar.as_deref() == Some("ui_r_aspectratio"));
        assert_eq!(
            f.menus.dvar("ui_r_aspectratio").as_deref(),
            Some("standard")
        );
        click_item(&mut f, |it| {
            it.action.iter().any(
                |c| matches!(c, vcod_cod4::scriptmenu::Cmd::Open(m) if m == "vid_restart_popmenu"),
            )
        });
        click_item(&mut f, |it| {
            it.action
                .iter()
                .any(|c| matches!(c, vcod_cod4::scriptmenu::Cmd::Exec(l) if l == "vid_restart"))
        });
        let lines = f.take_console();
        assert!(
            lines.contains(&"set r_aspectratio \"standard\"".to_string()),
            "{lines:?}"
        );
        for l in &lines {
            shell.execute(l);
        }
        // latched until the restart (flags 0x21)
        assert_eq!(shell.cvar("r_aspectRatio"), Some("auto"));
        shell.apply_latched_video();
        assert_eq!(shell.cvar("r_aspectRatio"), Some("standard"));
        let aspect =
            crate::camera::cod4_aspect(shell.cvar("r_aspectRatio").unwrap(), 1920.0, 1080.0);
        assert_eq!(aspect, 4.0 / 3.0);

        // the texture page, its rows on copies of the dvars
        shell.execute("set r_picmip_manual 0; set r_texFilterAnisoMin 1");
        f.open_main(&shell, None);
        click(&mut f, "Options");
        click(&mut f, "Graphics...");
        click_item(&mut f, |it| {
            it.action.iter().any(
                |c| matches!(c, vcod_cod4::scriptmenu::Cmd::Open(m) if m == "options_graphics_texture"),
            )
        });
        assert_eq!(f.menus.open().unwrap().name, "options_graphics_texture");
        assert_eq!(f.menus.dvar("ui_r_picmip_manual").as_deref(), Some("0"));
        assert!(!f.menus.greyed(&item(&f, "ui_r_picmip_manual")));
        assert!(!f.menus.greyed(&item(&f, "r_texFilterAnisoMin")));
        let rows = ["ui_r_picmip", "ui_r_picmip_bump", "ui_r_picmip_spec"];
        // 1.7's MP rows carry no `dvarTest`: they take keys under
        // Automatic too, which `R_SetPicmip` then overrides
        for d in rows {
            assert!(!f.menus.greyed(&item(&f, d)), "{d} takes keys");
        }
        click_item(&mut f, |it| {
            it.dvar.as_deref() == Some("ui_r_picmip_manual")
        });
        assert_eq!(f.menus.dvar("ui_r_picmip_manual").as_deref(), Some("1"));
        // Extra (0) steps round to Low (3)
        click_item(&mut f, |it| it.dvar.as_deref() == Some("ui_r_picmip"));
        assert_eq!(f.menus.dvar("ui_r_picmip").as_deref(), Some("3"));
        assert_eq!(shell.cvar("r_picmip"), Some("0"), "a copy until applied");
        click_item(&mut f, |it| {
            it.action.iter().any(
                |c| matches!(c, vcod_cod4::scriptmenu::Cmd::Open(m) if m == "apply_picmip_popmenu"),
            )
        });
        assert_eq!(f.menus.open().unwrap().name, "options_graphics");
        let lines = f.take_console();
        assert!(
            lines.contains(&"set r_picmip \"3\"".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"set r_picmip_manual \"1\"".to_string()),
            "{lines:?}"
        );
        assert_eq!(lines.last().map(String::as_str), Some("r_applypicmip"));
        let mut effects = Vec::new();
        for l in &lines {
            effects.extend(shell.execute(l));
        }
        assert!(effects.contains(&Effect::ApplyPicmip), "{effects:?}");
        shell.cod4_set_picmip();
        let t = crate::texture_cvars(
            &shell,
            crate::texture_settings::TextureSettings::default().filter,
        );
        assert_eq!((t.picmip, t.picmip_bump, t.picmip_spec), (3, 0, 0));
        // Automatic writes the levels back, 0 on any machine vcod runs on
        shell.execute("set r_picmip_manual 0");
        shell.cod4_set_picmip();
        assert_eq!(shell.cvar("r_picmip"), Some("0"));
        // the anisotropy slider sets its dvar, which the samplers read live
        shell.execute("set r_texFilterAnisoMin 8; set r_texFilterMipMode \"Force Trilinear\"");
        let t = crate::texture_cvars(&shell, t.filter);
        assert_eq!(t.filter.aniso, 8);
        let d = t.filter.sampler("t", wgpu::AddressMode::Repeat);
        assert_eq!(d.anisotropy_clamp, 8);
    }

    /// Start New Server drawn by the real renderer: the game type's value
    /// lights its own row right of the label, not the Server Name row
    /// under it (no Vulkan or Metal adapter, no frame).
    /// `VCOD_FRONT_PPM=<path>` saves the frame.
    #[test]
    fn cod4_front_start_new_server_renders_offscreen() {
        let Some((mut f, _)) = front() else {
            return;
        };
        click(&mut f, "Start New Server");
        // The map preview draws the picked map's level shot.
        let shot = f.menus.map_preview().and_then(|m| f.texture_of(&m));
        assert!(shot.is_some(), "the level shot loads");
        assert!(
            f.quads(SIZE)
                .iter()
                .any(|q| Some(&q.texture) == shot.as_ref())
        );
        let (w, h) = (640u32, 480u32);
        let Some(bgra) = render(&f, (w, h)) else {
            return;
        };
        let lit = |x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    bgra[((y * w + x) * 4) as usize..][..3]
                        .iter()
                        .any(|&c| c > 150)
                })
                .count()
        };
        // On the 640x480 grid the Game Mode row is y 34..56 and Server
        // Name's 58..80; the game type draws from x 240 there, the empty
        // host name field leaves Server Name's right of 240 dark.
        let (game, name) = (lit(240, 330, 38, 56), lit(240, 330, 62, 80));
        assert!(game > 40 && name < game / 4, "{game} {name}");
    }

    /// Mods lists `mods/`' directories; Launch asks for `fs_game
    /// mods/<dir>`.
    #[test]
    fn cod4_front_mods_lists_the_mods_directory() {
        let Some((mut f, _)) = front() else {
            return;
        };
        let mods = crate::play::cod4_menus::front::mod_list(&f.root);
        click(&mut f, "Mods");
        assert_eq!(stack(&f).last().map(String::as_str), Some("mods_menu"));
        let n = names(&f);
        for (_, desc) in &mods {
            assert!(n.contains(desc), "{desc} in {n:?}");
        }
        if let Some((dir, desc)) = mods.first() {
            click(&mut f, desc);
            click(&mut f, "Launch");
            assert_eq!(
                f.take_requests(),
                [Request::RunMod(Some(format!("mods/{dir}")))]
            );
        }
        assert!(f.key(KeyCode::Escape, None));
        assert_eq!(stack(&f), ["main_text"]);
    }

    /// A game that ended with a reason comes back to the main menu with
    /// `error_popmenu` over it, the server's notice titled so; its OK
    /// clears the message and opens `main` again.
    #[test]
    fn cod4_front_error_popup_says_why() {
        let Some((mut f, shell)) = front() else {
            return;
        };
        f.open_main(&shell, Some(("Server disconnected", true)));
        assert_eq!(stack(&f).last().map(String::as_str), Some("error_popmenu"));
        let n = names(&f);
        assert!(n.iter().any(|s| s == "Server disconnected"), "{n:?}");
        assert!(n.iter().any(|s| s == "NOTICE" || s == "Notice"), "{n:?}");
        assert!(f.key(KeyCode::Escape, None));
        assert_eq!(stack(&f), ["main_text"]);
        assert_eq!(
            f.menus.ui.dvars.get("com_errormessage").map(String::as_str),
            Some("")
        );
    }

    /// The front end's quads drawn by the real renderer, headless, off the
    /// install's search path on a `w` x `h` target: the frame's BGRA
    /// bytes, after asserting no validation error. `None` with no adapter.
    fn render(f: &Cod4Front, (w, h): (u32, u32)) -> Option<Vec<u8>> {
        use crate::renderer::{CullMode, Frame, Renderer};
        let fs = cod4_fly::open_fs(&f.root, None, None).unwrap();
        let (mut r, errors) = Renderer::headless_sized(&fs, false, 1, (w, h))?;
        r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
        r.set_hud_quads(&fs, f.quads((w as f32, h as f32)));
        r.render(
            Frame {
                view_proj: glam::Mat4::IDENTITY,
                eye: Vec3::ZERO,
                fwd: Vec3::X,
                up: Vec3::Z,
                time: 0.0,
                fov_x: 80.0,
                cull: CullMode::On,
                hud_lines: Vec::new(),
            },
            None,
        );
        let (_, _, bgra) = r.read_offscreen().unwrap();
        assert!(
            errors.lock().unwrap().is_empty(),
            "{:?}",
            errors.lock().unwrap()
        );
        if let Ok(path) = std::env::var("VCOD_FRONT_PPM") {
            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
            ppm.extend(bgra.chunks(4).flat_map(|p| [p[2], p[1], p[0]]));
            std::fs::write(path, ppm).unwrap();
        }
        Some(bgra)
    }

    /// The main menu drawn by the real renderer: the animated background
    /// covering the screen, the rows' text brighter than it.
    /// `VCOD_FRONT_PPM=<path>` saves the frame for a look.
    #[test]
    fn cod4_front_main_menu_renders_offscreen() {
        let Some((f, _)) = front() else {
            return;
        };
        let Some(bgra) = render(&f, (640, 384)) else {
            return;
        };
        let lum: Vec<u32> = bgra
            .chunks(4)
            .map(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]))
            .collect();
        let black = lum.iter().filter(|&&l| l < 6).count();
        let bright = lum.iter().filter(|&&l| l > 450).count();
        assert!(
            black < lum.len() / 10,
            "{black} of {} pixels black",
            lum.len()
        );
        assert!(bright > 200, "{bright} bright pixels: the rows' text");
    }

    /// Each row of the main menu opens its screen from `ui_mp`, drawn from
    /// its layout (no expression vcod cannot evaluate); Esc comes back.
    #[test]
    fn cod4_front_screens_open_and_draw() {
        let Some((mut f, shell)) = front() else {
            return;
        };
        for (row, menu) in [
            ("Join Game", "pc_join_unranked"),
            ("Select Profile", "player_profile"),
            ("Rank & Challenges", "menu_challenges"),
            ("Controls", "main_controls"),
            ("Options", "main_options"),
            ("Single Player", "single_popmenu"),
        ] {
            f.open_main(&shell, None);
            click(&mut f, row);
            let open = f.menus.open().unwrap();
            assert_eq!(open.name, menu, "{row}");
            assert!(open.layout.is_some(), "{menu} evaluates");
            assert!(!names(&f).is_empty(), "{menu} draws");
        }
        // The option pages open over `main_options`.
        f.open_main(&shell, None);
        click(&mut f, "Options");
        click(&mut f, "Graphics...");
        assert_eq!(f.menus.open().unwrap().name, "options_graphics");
        assert!(f.menus.open().unwrap().layout.is_some());
    }

    /// Clicks the window point `p`.
    fn click_at(f: &mut Cod4Front, p: [f32; 2]) {
        f.mouse(None, Some(p), SIZE);
        f.button("MOUSE1", SIZE);
    }

    /// Runs the front end's frames until `done` or 5 s pass, the frame's
    /// sleep first: a Local reply read in the broadcast's own millisecond
    /// pings 0, which the display list hides (browser.rs `pump_until`).
    fn frames_until(f: &mut Cod4Front, done: impl Fn(&Cod4Front) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(16));
            assert!(f.frame(Instant::now()).is_none());
            if done(f) {
                return true;
            }
        }
        false
    }

    /// Join Game up over the main menu with its source stepped to Local,
    /// whose broadcast goes to `server`, until the server is listed.
    fn join_game_lists(f: &mut Cod4Front, server: std::net::SocketAddrV4) {
        f.menus.browser.lan_targets = vec![server];
        click(f, "Join Game");
        assert_eq!(f.menus.open().unwrap().name, "pc_join_unranked");
        // Internet is 1.7's default source; nothing asks the master until
        // its Refresh List.
        assert_eq!(f.menus.dvar("ui_netSource").as_deref(), Some("1"));
        assert!(!f.menus.browser.refreshing && !f.menus.browser.waiting_master());
        // Internet -> Favorites -> Local.
        for _ in 0..2 {
            let t = f.texts(SIZE);
            let p = t.iter().find(|(s, _)| s.starts_with("Source")).unwrap().1;
            click_at(f, [p[0] + 2.0, p[1] - 4.0]);
        }
        assert_eq!(f.menus.dvar("ui_netSource").as_deref(), Some("0"));
        // A loaded machine can starve the server past the broadcast's
        // wait: Refresh List broadcasts again, as a player would.
        let found =
            |f: &Cod4Front| !f.menus.shown_servers().is_empty() && !f.menus.browser.refreshing;
        let mut tries = 1;
        while !frames_until(f, found) {
            assert!(tries < 4, "Local found no server");
            click(f, "Refresh List");
            tries += 1;
        }
        assert_eq!(f.menus.shown_servers()[0].rec.addr, server);
    }

    /// Bare `vcod`'s Join Game against vcod-server on loopback: Local lists
    /// it, the password popup's field takes the typed keys (Backspace
    /// too), Join Server hands the front end the join, which runs through
    /// the join behind the connect screen (`cod4_loading::Loader`, the map loaded) with the
    /// password in the userinfo; the server's `w` drops the session, and
    /// the main menu comes back with `error_popmenu` saying why.
    #[test]
    fn cod4_front_join_game_joins_a_lan_server_and_drops_back_to_the_menu() {
        let Some((mut f, shell)) = front() else {
            return;
        };
        let Some(server) = crate::play::cod4_menus::browser::TestServer::start("secret") else {
            return;
        };
        join_game_lists(&mut f, server.addr);
        click_at(&mut f, [100.0, 89.0]);
        click(&mut f, "Password");
        assert_eq!(f.menus.open().unwrap().name, "password_popmenu");
        assert!(f.key(KeyCode::KeyS, Some("secrex")));
        assert!(f.key(KeyCode::Backspace, None));
        assert!(f.key(KeyCode::KeyT, Some("t")));
        assert!(f.key(KeyCode::Enter, None));
        assert_eq!(f.menus.open().unwrap().name, "pc_join_unranked");
        click(&mut f, "Join Server");
        let join = f
            .frame(Instant::now())
            .expect("Join Server asks for a join");
        assert_eq!((join.addr, join.password.as_str()), (server.addr, "secret"));

        let mut loader = Loader::start(JoinArgs {
            addr: std::net::SocketAddr::V4(join.addr),
            server: join.addr.to_string(),
            name: "frontjoin".into(),
            password: join.password,
            join: crate::play::cod4_menus::Join::default(),
            game_dir: f.root.clone(),
            allow_download: false,
            www_download: false,
        });
        let t0 = Instant::now();
        let joined = loop {
            assert!(t0.elapsed() < std::time::Duration::from_secs(60), "no join");
            if let Some(j) = loader.poll() {
                break j;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let mut s = joined
            .expect("the join with the password lands in the game")
            .session;
        assert_eq!(s.map_name().as_deref(), Some("mp_crash"));
        server.command("w \"EXE_SERVERQUIT\"");
        // The `w` rides the next snapshot: five seconds of the server's
        // snapshots (20 a second), with a wall-clock backstop.
        let (sent, snaps) = (Instant::now(), s.client().snapshots.len());
        let why = loop {
            assert!(
                s.client().snapshots.len() < snaps + 100
                    && sent.elapsed() < std::time::Duration::from_secs(60),
                "no drop"
            );
            s.step(t0.elapsed().as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            if let Some(why) = s.take_dropped() {
                break why;
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        };
        let why = crate::play::cod4::localize_drop(&why, &f.root);
        f.open_main(&shell, Some((&why, true)));
        assert_eq!(stack(&f).last().map(String::as_str), Some("error_popmenu"));
        assert!(names(&f).contains(&why), "{why:?} in {:?}", names(&f));
    }

    /// The join menu with the server's row, drawn through the front end
    /// by the real renderer (no Vulkan or Metal adapter, no frame): the
    /// row's band holds lit text, the hardware column its icon.
    /// `VCOD_FRONT_PPM=<path>` saves the frame.
    #[test]
    fn cod4_front_join_game_renders_its_row_offscreen() {
        let Some((mut f, _)) = front() else {
            return;
        };
        let Some(server) = crate::play::cod4_menus::browser::TestServer::start("") else {
            return;
        };
        join_game_lists(&mut f, server.addr);
        drop(server);
        // What the HUD pass gets: the row's host name and its icon.
        assert!(names(&f).iter().any(|s| s == "^1vcod ^7browser"));
        let icon = f.texture_of("server_hardware_linux_dedicated").unwrap();
        assert!(f.quads(SIZE).iter().any(|q| q.texture == icon));
        let (w, h) = (640u32, 480u32);
        let Some(bgra) = render(&f, (w, h)) else {
            return;
        };
        let lit = |x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    bgra[((y * w + x) * 4) as usize..][..3]
                        .iter()
                        .any(|&c| c > 96)
                })
                .count()
        };
        // The first row: y 81..97 on the 640x480 grid; the host name from
        // x 52, the hardware icon in x 27..43.
        let (text, icon, below) = (
            lit(52, 250, 81, 97),
            lit(27, 43, 81, 97),
            lit(52, 250, 120, 136),
        );
        assert!(
            text > 40 && icon > 10 && below < text / 4,
            "{text} {icon} {below}"
        );
    }

    /// Start New Server's Server Name and Password fields take typed text,
    /// and Start hands both to the server it asks for.
    #[test]
    fn cod4_front_start_new_server_edit_fields_reach_the_server() {
        let Some((mut f, _)) = front() else {
            return;
        };
        click(&mut f, "Start New Server");
        for (dvar, typed) in [("sv_hostname", "my server"), ("g_password", "pw")] {
            click_item(&mut f, |it| it.dvar.as_deref() == Some(dvar));
            assert!(f.menus.editing(), "{dvar} edits");
            assert!(f.key(KeyCode::KeyA, Some(typed)));
            assert!(f.key(KeyCode::Enter, None));
            assert!(!f.menus.editing());
            assert!(names(&f).iter().any(|s| s == typed), "{typed} drawn");
        }
        click(&mut f, "Start");
        let requests = f.take_requests();
        let [Request::StartServer { cvars, .. }] = requests.as_slice() else {
            panic!("Start asks for one server");
        };
        assert!(
            cvars.contains(&("sv_hostname".into(), "my server".into())),
            "{cvars:?}"
        );
        assert!(
            cvars.contains(&("g_password".into(), "pw".into())),
            "{cvars:?}"
        );
    }

    /// The front end's sounds: the code zones' `mouse_over`, `mouse_click`
    /// and `music_mainmenu_mp` load, the main menu names its music, a click
    /// plays `mouse_click`, and the audio system plays each off the
    /// zones' table and the search path.
    #[test]
    fn cod4_front_plays_its_menu_sounds_and_music() {
        let Some((mut f, _)) = front() else {
            return;
        };
        let set = f.sounds_to_install().expect("the front end's sounds load");
        assert!(f.sounds_to_install().is_none(), "installed once");
        assert_eq!(f.menus.music(), Some("music_mainmenu_mp"));
        f.take_sounds();
        click(&mut f, "Quit");
        assert!(f.take_sounds().iter().any(|s| s == "mouse_click"));
        let mut fs = cod4_fly::open_fs(&f.root, None, None).unwrap();
        for (path, wav) in crate::cod4_audio::overlay_files(&set) {
            fs.overlay(&path, wav);
        }
        let opts = crate::audio::AudioOpts {
            enabled: false,
            volume: 1.0,
            rate: 0,
        };
        let table = crate::cod4_audio::table(&set);
        // Each alias's file is on the search path or its overlay.
        for alias in ["mouse_over", "mouse_click", "music_mainmenu_mp"] {
            let rows = table.get(alias).unwrap_or_else(|| panic!("{alias}"));
            for r in rows {
                let path = crate::audio::bank::sound_path(&r.file);
                assert!(fs.read(&path).is_some(), "{alias}: {path}");
            }
        }
        let mut a = crate::audio::AudioSystem::new(&fs, opts);
        a.set_cod4_aliases(table);
        for alias in ["mouse_over", "mouse_click"] {
            assert!(a.play_local(&fs, alias).is_some(), "{alias}");
        }
        a.music_play(&fs, "music_mainmenu_mp");
        assert_eq!(a.stats().misses, 0);
    }

    /// Clicks the open menu's item whose action runs `uiScript <script>`.
    fn click_script(f: &mut Cod4Front, script: &str) {
        use vcod_cod4::scriptmenu::Cmd;
        click_item(f, |it| {
            it.action
                .iter()
                .any(|c| matches!(c, Cmd::UiScript(n, _) if n.eq_ignore_ascii_case(script)))
        });
    }

    /// 1.7's profile menus over a fresh `players/`: with no profile
    /// `main` opens the pick and its create popup; a typed name becomes
    /// `players/profiles/<name>/`, listed and picked, and OK loads it
    /// (`Com_ChangePlayerProfile`, the client's request). Delete's Yes
    /// removes the directory, and with none left the popup that asks for
    /// one comes back (cod4-front-end.md 3).
    #[test]
    fn cod4_front_profiles_are_created_loaded_and_deleted() {
        let Some((mut f, shell)) = front() else {
            return;
        };
        let players = tempfile::tempdir().unwrap();
        let profiles = players.path().join("players/profiles");
        f.menus.front.players_root = Some(players.path().to_path_buf());
        f.menus.ui.dvars.remove("com_playerprofile");
        f.open_main(&shell, None);
        assert_eq!(
            stack(&f),
            ["player_profile", "profile_create_popmenu"],
            "no profile: the pick (which closes `main_text`) and its create popup"
        );
        assert!(f.menus.editing(), "the name field takes the keys");
        assert!(f.key(KeyCode::KeyA, Some("Alpha")));
        // The field's `onAccept`: `createPlayerProfile`, `close self`.
        assert!(f.key(KeyCode::Enter, None));
        assert!(profiles.join("Alpha").is_dir(), "the profile's directory");
        assert_eq!(stack(&f), ["player_profile"]);
        assert_eq!(f.menus.dvar("ui_playerProfileCount").as_deref(), Some("1"));
        assert_eq!(
            f.menus.dvar("ui_playerProfileSelected").as_deref(),
            Some("Alpha")
        );
        assert!(names(&f).iter().any(|s| s == "Alpha"), "listed");
        // A second profile of the same name, in any case, is refused.
        click(&mut f, "New");
        assert!(f.key(KeyCode::KeyA, Some("alpha")));
        // The field's `onAccept`: `createPlayerProfile`, `close self`.
        assert!(f.key(KeyCode::Enter, None));
        assert_eq!(
            stack(&f).last().map(String::as_str),
            Some("profile_exists_popmenu")
        );
        assert!(f.key(KeyCode::Escape, None));
        assert_eq!(stack(&f), ["player_profile"]);
        click_script(&mut f, "loadPlayerProfile");
        assert_eq!(f.take_requests(), [Request::LoadProfile("Alpha".into())]);
        assert_eq!(f.menus.dvar("com_playerProfile").as_deref(), Some("Alpha"));
        assert_eq!(stack(&f), ["main_text"]);
        // Delete it from Select Profile.
        click(&mut f, "Select Profile");
        assert_eq!(stack(&f), ["player_profile"]);
        click(&mut f, "Delete");
        assert_eq!(
            stack(&f).last().map(String::as_str),
            Some("profile_del_sure_popmenu")
        );
        click_script(&mut f, "deletePlayerProfile");
        assert!(!profiles.join("Alpha").exists(), "the directory goes");
        assert_eq!(f.take_requests(), [Request::ProfileDeleted("Alpha".into())]);
        assert_eq!(f.menus.dvar("com_playerProfile").as_deref(), Some(""));
        assert_eq!(f.menus.dvar("ui_playerProfileCount").as_deref(), Some("0"));
        assert_eq!(
            stack(&f).last().map(String::as_str),
            Some("profile_must_create_popmenu"),
            "{:?}",
            stack(&f)
        );
    }

    /// Loading a profile: `active.txt` names it, a fresh one takes its
    /// name as the player's and gets vcod's config written, one with
    /// 1.7's `config_mp.cfg` runs it.
    #[test]
    fn cod4_profile_change_writes_active_and_the_config() {
        let root = tempfile::tempdir().unwrap();
        let profiles = root.path().join("players/profiles");
        std::fs::create_dir_all(profiles.join("Fresh")).unwrap();
        std::fs::create_dir_all(profiles.join("Retail")).unwrap();
        std::fs::write(
            profiles.join("Retail/config_mp.cfg"),
            "seta name \"Old Hand\"\nseta sensitivity \"7\"\n",
        )
        .unwrap();
        let mut shell = Shell::new();
        let config = change_profile(root.path(), "Fresh", &mut shell).unwrap();
        assert_eq!(config, profiles.join("Fresh").join(crate::CONFIG_FILE));
        assert_eq!(
            std::fs::read_to_string(profiles.join("active.txt")).unwrap(),
            "Fresh"
        );
        assert_eq!(shell.cvar("name"), Some("Fresh"));
        assert!(std::fs::read_to_string(&config).unwrap().contains("Fresh"));
        let config = change_profile(root.path(), "retail", &mut shell).unwrap();
        assert_eq!(config, profiles.join("Retail").join(crate::CONFIG_FILE));
        assert_eq!(shell.cvar("name"), Some("Old Hand"));
        assert_eq!(shell.cvar("sensitivity"), Some("7"));
        assert!(change_profile(root.path(), "gone", &mut shell).is_none());
    }
}
