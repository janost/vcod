//! A watched or played CoD4 session on screen, `App`'s `Session::Cod4`:
//! the session's clock, its view on the camera, its entities as dynamic
//! models and brush models, its events and obituaries on the console, the
//! binds into its cmds and the server's script menus as a list
//! (`play::cod4` holds the session; this draws it and feeds it keys).
//!
//! A player or corpse is posed (`play::cod4_players`); any other entity
//! whose xmodel a zone holds draws it in its bind pose, one with none its
//! type's box.

use glam::{Mat4, Quat, Vec3};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;
use vcod_common::model::{Bone, Lod, Model, Surface, Vertex};
use vcod_common::pmove::angle_normalize180;
use vcod_common::turretpose::angles_quat;

use crate::camera::FlyCamera;
use crate::cod2_client::Cod2Frame;
use crate::cod2_viewmodel::ViewFrame;
use crate::cod4_audio::{self, Cod4Sounds};
use crate::cod4_fly::WeaponModels;
use crate::cod4_fx::{self, Cod4Effects};
use crate::cod4_hud::Cod4Hud;
use crate::cod4_vehicles::Cod4Vehicles;
use crate::cod4_viewmodel::{Cod4View, tag_to_world};
use crate::console;
use crate::play::cod4::{
    BodyView, CS_AMBIENT, Cod4Session, Drawn, ET_ITEM, ET_MISSILE, ET_PLAYER, ET_PLAYER_CORPSE,
    Fired, Scene, Visual, weapon_list,
};
use crate::play::cod4_players::{Cod4Players, WeaponTags};
use crate::play::quake::Quakes;
use crate::renderer::{DynamicModelInstance, ModelHandle, Renderer};
use vcod_cod4::anim::BodySource;
use vcod_common::asset_source::AssetSource;

/// A window size, fov, aspect and HUD placement, as logged.
type LoggedScreen = (f32, f32, f32, f32, crate::hud::cod2elem::ScreenPlacement);

pub(crate) struct Cod4Client {
    pub(crate) session: Cod4Session,
    /// The helicopters' poses and effects, and the drawn models' tags
    /// (`cod4_vehicles`).
    pub(crate) vehicles: Cod4Vehicles,
    /// A live session's clock starts on the first frame.
    start: Option<Instant>,
    /// Test only: the last frame's entity instances as (instance, model,
    /// entity number, origin); the bodies' instances come first, unnamed.
    #[cfg(test)]
    pub(crate) drawn_names: std::cell::RefCell<Vec<(u32, String, u32, Vec3)>>,
    /// A capture's clock: the frames' `dt` summed (capped at 100 ms each,
    /// as the frame caps it), so a slow frame slows the replay rather than
    /// skipping it.
    played_ms: f64,
    models: HashMap<String, Model>,
    /// `models`' surfaces' `partBits` (`Cod4Map::parts`).
    parts: crate::cod4_fly::Parts,
    weapons: HashMap<String, WeaponModels>,
    uploaded: HashMap<String, Option<ModelHandle>>,
    /// The camera's (pitch, yaw) as the last pump left it: live, the
    /// mouse's turn since then is what moves the cmd's view.
    cam_left: Option<(f32, f32)>,
    /// What the bodies are posed from; `None` leaves them in the bind pose.
    bodies: Option<BodySource>,
    players: Cod4Players,
    /// The last frame's drawn counts (models, brush models, bodies posed)
    /// and server time, for F3.
    drawn: (usize, usize, usize),
    /// The last frame's drawn angles by entity, the axes its voices turn
    /// with ([`Self::entity_angles`]).
    drawn_angles: HashMap<u32, Vec3>,
    time: i32,
    /// The capture's end was announced.
    ended: bool,
    /// The fly keys held this frame ([`Self::set_keys`]).
    keys: FlyKeys,
    /// A capture's camera left the recorded view for the keys; a followed
    /// view takes it back.
    free: bool,
    /// The 2D layer, drawn on [`Self::render_time`].
    pub(crate) hud: Cod4Hud,
    render_time: i32,
    /// The drawn eye and yaw, for the compass.
    view: Option<(Vec3, f32)>,
    /// The drawn eye and angles, which the own shots' client-side bullets
    /// leave from.
    aim: Option<(Vec3, Vec3)>,
    /// The events' sounds and effects (`cod4_audio`, `cod4_fx`), and the
    /// earthquakes shaking the view.
    sounds: Option<Cod4Sounds>,
    effects: Option<Cod4Effects>,
    quakes: Quakes,
    /// The view weapon (`cod4_viewmodel`), this frame's draw of it for
    /// [`Self::take_view`], the fov the frame draws at, and the drawn
    /// view's client with its gun's tags in the world.
    view_weapon: Cod4View,
    vm: Option<ViewFrame>,
    fov: Option<f32>,
    /// The last drawn view's horizontal fov (degrees) and aspect, the
    /// frustum the next frame's effects spawn against.
    drawn_fov: std::cell::Cell<Option<(f32, f32)>>,
    /// The window, fov, aspect and HUD placement last logged
    /// (`RUST_LOG=vcod::cod4_client=debug`).
    logged_screen: std::cell::Cell<Option<LoggedScreen>>,
    /// The drawn playerstate the crosshair reads.
    view_ps: Option<crate::cod4_viewmodel::ViewPs>,
    view_tags: Option<(u32, WeaponTags)>,
    /// Each drawn mounted gun's `tag_flash` in the world this frame.
    turret_tags: HashMap<u32, WeaponTags>,
    /// msvcrt `rand()`'s state for the gunner's view jitter.
    jitter_rand: u32,
    /// The zones' script menus went to the session.
    menus_loaded: bool,
    /// The fog and vision set its configstrings name.
    look: crate::cod4_look::Cod4Look,
    /// The drawn snapshot's shellshock (`play::cod4_shock`): its latches,
    /// this frame's camera kick and the mouse's sensitivity and turn caps.
    shock: crate::play::cod4_shock::Cod4Shock,
    shock_kick: [f32; 2],
    /// The damage kick's roll this frame, degrees (its pitch goes on the
    /// camera; cod4-client-view.md 14).
    damage_roll: f32,
    /// The frame draws in third person (`play::cod4_third_person`).
    third_person: bool,
    shock_look: crate::play::cod4_shock::Look,
    turn: crate::play::cod2_shock::TurnCap,
    /// The voices `playLocalSound` started, by alias, for `stopLocalSound`.
    local_voices: HashMap<String, crate::audio::voices::VoiceId>,
    /// The missiles whose `projTrailEffect` is playing (the trail flag).
    trails_made: HashSet<u32>,
    /// The depth of field the sight eases from frame to frame
    /// (`cod4_dof`), and this frame's gun's `adsDofStart` and `adsDofEnd`.
    dof: crate::cod4_dof::ViewDof,
    ads_dof: Option<[f32; 2]>,
    /// Voice chat's codec, microphone and talker streams, made on the
    /// first live frame (`crate::voice`).
    voice: Option<crate::voice::VoiceChat>,
    /// Each fire event's client-side rounds as the effects took them, for
    /// the tests.
    #[cfg(test)]
    rounds_seen: Vec<RoundsSeen>,
}

/// A fire event's rounds ([`cod4_fx::client_bullets`]): its entity, its
/// aim, the muzzle's forward axis a gun's rounds left along, the rounds
/// and the tracers they drew.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct RoundsSeen {
    pub entity: Option<u32>,
    pub aim: Option<cod4_fx::Aim>,
    pub axis: Option<Vec3>,
    pub hits: Vec<cod4_fx::BulletHit>,
    pub tracers: usize,
}

/// The fly keys: forward, back, left, right, up, down, boost; a capture's
/// camera flies by them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FlyKeys(pub [bool; 7]);

impl FlyKeys {
    pub(crate) fn of(i: &crate::camera::InputState) -> FlyKeys {
        FlyKeys([i.forward, i.back, i.left, i.right, i.up, i.down, i.boost])
    }

    fn input(self) -> crate::camera::InputState {
        let [forward, back, left, right, up, down, boost] = self.0;
        crate::camera::InputState {
            forward,
            back,
            left,
            right,
            up,
            down,
            boost,
        }
    }

    fn moving(self) -> bool {
        self.0[..6].iter().any(|&k| k)
    }
}

/// The fired events' sounds, each player shot's fire sound's length then
/// holding the shooter's compass ping (`CG_FireWeapon`,
/// cod4-hud.md 15.9), placed by `at` (`cod4_audio::place`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn play_event_cues(
    audio: &mut crate::audio::AudioSystem,
    fs: &vcod_common::pk3::Pk3Fs,
    sounds: &mut cod4_audio::Cod4Sounds,
    hud: &mut Cod4Hud,
    fired: &[Fired],
    weapons: &[String],
    cs: &dyn Fn(u16) -> String,
    at: &cod4_audio::Placement,
) {
    for f in fired {
        let shot = shot_client(f);
        let ops = cod4_audio::place(f, sounds.cues(f, weapons, cs), at);
        for (k, op) in ops.into_iter().enumerate() {
            let id = cod4_audio::play_op(audio, fs, op);
            if let (Some(client), 0) = (shot, k) {
                let ms = id.and_then(|id| audio.voice_ms(id)).unwrap_or(0);
                hud.hold_shot(client, ms);
            }
        }
    }
}

/// The client of a player's `EV_FIRE_WEAPON` or `_LASTSHOT`, the shots
/// whose fire sound `CG_CompassAddWeaponPingInfo` takes the length of.
/// `VCOD_PERF_TURN`'s yaw rate, degrees a second; read once.
fn perf_turn() -> Option<f32> {
    static RATE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *RATE.get_or_init(|| std::env::var("VCOD_PERF_TURN").ok()?.parse().ok())
}

fn shot_client(f: &Fired) -> Option<usize> {
    let e = f.state.as_ref()?;
    (matches!(f.name, "EV_FIRE_WEAPON" | "EV_FIRE_WEAPON_LASTSHOT")
        && e.etype() == crate::play::cod4::ET_PLAYER)
        .then(|| crate::play::cod4::ent_u32(e, "clientNum") as usize)
}

impl Cod4Client {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        session: Cod4Session,
        models: HashMap<String, Model>,
        parts: crate::cod4_fly::Parts,
        weapons: HashMap<String, WeaponModels>,
        bodies: Option<BodySource>,
        av: Option<crate::cod4_fly::Av>,
        world: Option<std::sync::Arc<crate::play::cod4_predict::Cod4World>>,
        look: crate::cod4_look::Cod4Look,
    ) -> Self {
        let (sounds, effects) = av.unzip();
        let mut session = session;
        if let Some(w) = world {
            session.set_world(w);
        }
        let mut players = Cod4Players::default();
        session.legs_movetypes = bodies.as_ref().and_then(|b| players.legs_movetypes(b));
        Self {
            session,
            vehicles: vehicles(bodies.as_ref()),
            start: None,
            #[cfg(test)]
            drawn_names: Default::default(),
            played_ms: 0.0,
            models,
            parts,
            weapons,
            uploaded: HashMap::new(),
            cam_left: None,
            bodies,
            players,
            drawn: (0, 0, 0),
            drawn_angles: HashMap::new(),
            time: 0,
            ended: false,
            keys: FlyKeys::default(),
            free: false,
            hud: Cod4Hud::default(),
            render_time: 0,
            view: None,
            aim: None,
            sounds,
            effects,
            quakes: Quakes::default(),
            view_weapon: Cod4View::default(),
            vm: None,
            fov: None,
            drawn_fov: std::cell::Cell::new(None),
            logged_screen: std::cell::Cell::new(None),
            view_ps: None,
            view_tags: None,
            turret_tags: HashMap::new(),
            jitter_rand: 1,
            menus_loaded: false,
            look,
            shock: Default::default(),
            shock_kick: [0.0, 0.0],
            damage_roll: 0.0,
            third_person: false,
            shock_look: crate::play::cod4_shock::Look::NONE,
            turn: Default::default(),
            local_voices: HashMap::new(),
            trails_made: HashSet::new(),
            dof: Default::default(),
            ads_dof: None,
            voice: None,
            #[cfg(test)]
            rounds_seen: Vec::new(),
        }
    }

    /// A map change (`play::cod4::Cod4Session::take_map_change`): the new
    /// map's entity models, weapons, bodies, sounds and effects,
    /// prediction world and look in place of the last map's; every
    /// renderer handle and the zones' menus start over.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_map(
        &mut self,
        models: HashMap<String, Model>,
        parts: crate::cod4_fly::Parts,
        weapons: HashMap<String, WeaponModels>,
        bodies: Option<BodySource>,
        av: Option<crate::cod4_fly::Av>,
        world: Option<std::sync::Arc<crate::play::cod4_predict::Cod4World>>,
        look: crate::cod4_look::Cod4Look,
    ) {
        let (sounds, effects) = av.unzip();
        self.vehicles = vehicles(bodies.as_ref());
        (self.models, self.weapons, self.bodies) = (models, weapons, bodies);
        self.parts = parts;
        (self.sounds, self.effects, self.look) = (sounds, effects, look);
        if let Some(w) = world {
            self.session.set_world(w);
        }
        self.players = Cod4Players::default();
        self.session.legs_movetypes = self
            .bodies
            .as_ref()
            .and_then(|b| self.players.legs_movetypes(b));
        self.menus_loaded = false;
        self.cam_left = None;
        self.view_tags = None;
        self.reset_render();
    }

    /// The fly keys held this frame, which a capture's camera flies by.
    pub(crate) fn set_keys(&mut self, keys: FlyKeys) {
        self.keys = keys;
    }

    /// A console effect: live, the cmd's keys and weapon picks
    /// (`play::cod4_input`), `+scores` and the lines that go to the
    /// server (`say`, `cmd`, and an unknown command as
    /// `CL_ForwardCommandToServer` sends it); a capture takes only the
    /// board.
    pub(crate) fn apply(&mut self, effect: &crate::console::shell::Effect) -> bool {
        use crate::console::shell::Effect;
        if let Effect::Scores(down) = effect {
            self.set_scores(*down);
            return true;
        }
        if !self.session.is_live() {
            return false;
        }
        match effect {
            Effect::Forward(line) => {
                self.session.command(line);
                true
            }
            // A key-up command goes nowhere; a `+` one is unknown.
            Effect::Unknown { word, line } if !word.starts_with(['+', '-']) => {
                self.session.command(line);
                true
            }
            Effect::MessageMode { team } => {
                self.session.input.release_all();
                self.hud.chat_field = Some(crate::hud::ChatField {
                    team: *team,
                    text: String::new(),
                });
                true
            }
            Effect::Impulse(crate::play::input::Action::UseSlot(n)) => {
                self.session.action_slot(*n);
                true
            }
            _ => {
                let primaries = self.session.held_primaries();
                self.session.input.effect(effect, &primaries)
            }
        }
    }

    /// A key while a script menu is up: the arrows and Tab move the
    /// focus, Enter runs it, Esc the menu's `onEsc`, a character its
    /// `execKey` (on the list, 1-9 pick); the rest go on to the binds,
    /// which the cmds leave out while the menu is up (`talking`). With
    /// none up, Esc opens the team menu (`g_scriptMainMenu`) and goes on
    /// to free the mouse. `None` when the session has no menus to show (a
    /// capture).
    /// A key with the chat field open: Enter sends `say <line>` or
    /// `say_team <line>` as `CL_KeyEvent`'s chat arm does,
    /// Escape drops it; with none open the script menus take it
    /// ([`Self::menu_key`]).
    pub(crate) fn chat_key(
        &mut self,
        code: winit::keyboard::KeyCode,
        text: Option<&str>,
    ) -> Option<bool> {
        let Some(field) = &mut self.hud.chat_field else {
            return self.menu_key(code);
        };
        if let Some(line) = crate::chat_field_key(field, code, text) {
            if let Some(line) = line {
                let word = if field.team { "say_team" } else { "say" };
                self.session.command(&format!("{word} {line}"));
            }
            self.hud.chat_field = None;
        }
        Some(true)
    }

    pub(crate) fn menu_key(&mut self, code: winit::keyboard::KeyCode) -> Option<bool> {
        use crate::play::cod4_menus::MenuKey;
        use winit::keyboard::KeyCode;
        if !self.session.is_live() {
            return None;
        }
        if self.session.menus.open().is_none() {
            // `CL_KeyEvent`: Esc cancels the location selector.
            if code == KeyCode::Escape && self.session.selecting_location() != 0 {
                self.session.selector.cancel = true;
                return Some(true);
            }
            if code == KeyCode::Escape {
                self.session.open_main_menu();
            }
            return Some(false);
        }
        // A bind row waiting for its key takes any key (`Menu_HandleKey`).
        if self.session.menus.waiting_for_key() {
            if let Some(name) = crate::console::keys::key_name(code) {
                self.session.menus.bind_key(name);
            }
            return Some(true);
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
                    _ => return Some(false),
                }
            }
        };
        Some(self.session.menu_key(key) || key == MenuKey::Escape)
    }

    /// A mouse button or wheel notch (`MOUSE1`, `MWHEELUP`, ..) over a
    /// drawn script menu: a bind row waiting for its key takes it, else
    /// MOUSE1 clicks and the wheel scrolls a focused list. False when no
    /// drawn menu holds the mouse.
    pub(crate) fn menu_button(&mut self, key: &str, size: (f32, f32)) -> bool {
        if !self.menu_has_mouse() {
            return false;
        }
        if self.session.menus.waiting_for_key() {
            self.session.menus.bind_key(key);
        } else if key == "MOUSE1" {
            self.session.menu_mouse(None, None, size);
        } else if let Some(up) = match key {
            "MWHEELUP" => Some(true),
            "MWHEELDOWN" => Some(false),
            _ => None,
        } {
            self.session.menus.wheel(up);
        }
        true
    }

    /// Whether a drawn script menu holds the mouse.
    pub(crate) fn menu_has_mouse(&self) -> bool {
        self.session.is_live()
            && self
                .session
                .menus
                .open()
                .is_some_and(|o| o.layout.is_some())
    }

    /// The mouse over a drawn script menu: raw counts `d` or a window
    /// position `at`, or with neither MOUSE1. False when no drawn menu
    /// holds the mouse.
    pub(crate) fn menu_mouse(
        &mut self,
        d: Option<[f32; 2]>,
        at: Option<[f32; 2]>,
        size: (f32, f32),
    ) -> bool {
        if !self.menu_has_mouse() {
            return false;
        }
        self.session.menu_mouse(d, at, size);
        true
    }

    /// A demo played out: `Some` with its `timedemo` line, if any.
    pub(crate) fn demo_over(&self) -> Option<Option<String>> {
        self.session
            .demo_over()
            .then(|| self.session.timedemo_report(self.played_ms as i32))
    }

    fn now_ms(&mut self, dt: f32) -> f64 {
        if !self.session.is_live() {
            self.played_ms += f64::from(dt.min(0.1)) * 1000.0;
            return self.played_ms;
        }
        // the session's clock: the wall's, or a live test's sim clock
        let now = crate::play::cod4::clock_now();
        let start = *self.start.get_or_insert(now);
        now.saturating_duration_since(start).as_secs_f64() * 1000.0
    }

    /// Reads what is due, logs the events, prints the obituaries, puts the
    /// camera on the snapshot's view and draws the entities. False with no
    /// snapshot yet (the camera flies).
    pub(crate) fn pump(&mut self, cx: &mut Cod2Frame, cam: &mut FlyCamera) -> bool {
        use crate::frame_prof::{Stage, span};
        let mut prof = span(Stage::Net);
        let now = self.now_ms(cx.dt);
        if let Some(fx) = &mut self.effects {
            // `cg_blood` (cod4-effects.md 6), on unless set to 0
            fx.set_blood(
                cx.shell
                    .cvar("cg_blood")
                    .is_none_or(|v| vcod_cod4::menuexp::atoi(v) != 0),
            );
        }
        let menu_was = self.session.menus.open().is_some();
        if self.session.is_live() {
            // The mouse turns `cl.viewangles`, the cmd's angles; the camera
            // draws the predicted view, which the pitch clamp holds
            // (cod4-client-view.md 2).
            match self.cam_left {
                // The location selector takes the mouse; the view holds.
                Some((p, y)) if self.session.selecting_location() != 0 => {
                    let turn = |a: f32, b: f32| angle_normalize180((a - b).to_degrees());
                    self.session
                        .move_selector([-turn(cam.pitch, p), turn(cam.yaw, y)]);
                    (cam.pitch, cam.yaw) = (p, y);
                }
                Some((p, y)) => {
                    let turn = |a: f32, b: f32| angle_normalize180((a - b).to_degrees());
                    self.session.turn([-turn(cam.pitch, p), turn(cam.yaw, y)]);
                }
                None => self.session.view = [-cam.pitch.to_degrees(), cam.yaw.to_degrees()],
            }
            // `VCOD_PERF_TURN=<deg/s>`: the view turns on its own, a moving
            // view for `tools/cod4_perf.sh` (cod4-client-perf.md 36)
            if let Some(rate) = perf_turn() {
                self.session.turn([0.0, rate * cx.dt]);
            }
            self.session.talking = cx.console_open || menu_was || self.hud.chat_field.is_some();
        }
        self.session
            .set_freeze_demo(cx.shell.cvar_f32("cl_freezeDemo") != 0.0);
        if let Err(e) = self.session.step(now) {
            log::warn!("cod4: {e:#}");
        }
        if self.session.is_live() {
            self.voice_frame(cx);
        }
        // A spawn or the server's turn: the camera takes the new view.
        if self.session.take_view_jump() {
            cam.pitch = -self.session.view[0].to_radians();
            cam.yaw = self.session.view[1].to_radians();
        }
        if !menu_was && self.session.menus.open().is_some() {
            self.session.input.release_all();
        }
        if self.session.menus.open().is_some() {
            self.session.menus.mirror = self.hud.mirrored_dvars(&self.session);
            self.session.menus.host = crate::play::cod4_menus::Host {
                cvars: cx
                    .shell
                    .cvar_values()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                enums: cx
                    .shell
                    .enum_domains()
                    .map(|(k, v)| (k.to_string(), v.to_vec()))
                    .collect(),
                binds: cx
                    .shell
                    .binds()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                players: self.session.players(),
            };
            self.session.menus.game = self.hud.menu_game(&self.session, self.render_time);
            let (gametype, name, objective) = self.hud.gametype_strings(&self.session);
            let m = &mut self.session.menus;
            (m.gametype, m.gametype_name, m.objective) = (gametype, name, objective);
        }
        let (team, map) = (self.session.own_team(), self.session.map_name());
        self.session.menus.team = team;
        self.session.menus.mapname = map.unwrap_or_default();
        self.load_hud(cx);
        // The scene first: its prediction fires our own events this frame
        // (cod4-effects.md 12), which play off this frame's eye and view
        // weapon tags.
        prof.next(Stage::Scene);
        self.session.third_person =
            crate::play::cod4_third_person::Dvars::read(|n| cx.shell.cvar(n).map(str::to_string));
        let scene = self.session.scene();
        let Some(scene) = scene else {
            prof.next(Stage::Events);
            self.events(cx, now);
            self.look_frame(cx, false);
            self.cam_left = Some((cam.pitch, cam.yaw));
            return false;
        };
        self.time = scene.server_time;
        self.render_time = scene.render_time;
        self.look_frame(cx, crate::cod4_look::night_vision(scene.view.weap_flags));
        // `UpdateScores`: a `score` every 2 s while the board draws.
        if let Some(snap) = self.session.client().snapshots.last()
            && self.hud.scoreboard_fade(snap, self.render_time).is_some()
            && self.hud.score_request(self.render_time)
        {
            self.session.command("score");
        }
        // `BG_CalculateView_DamageKick` turns the drawn view (pitch, roll)
        // and the gun its own way, never the cmd.
        let mut scene = scene;
        let dk = self.hud.damage_kick();
        let since = self.render_time - dk.time;
        scene.view.damage = (dk.time != 0).then_some((dk, since));
        let overlay = weapon_list_of(&self.session)
            .and_then(|list| {
                let w = crate::play::cod4::weapon_name(&list, scene.view.view_weapon())?;
                Some(self.weapons.get(w)?.view.as_ref()?.numbers.overlay_reticle)
            })
            .unwrap_or(false);
        // On a gun (`eFlags` 0x300) `CG_OffsetFirstPersonView` skips the
        // kick (0x450063); in third person it does not run.
        let gunner = scene.view.eflags & 0x300 != 0;
        let third = scene.third_person;
        self.third_person = third.is_some();
        let [dmg_pitch, _, dmg_roll] = match gunner || third.is_some() {
            false => dk.view(since, scene.view.pos_frac, overlay),
            true => [0.0; 3],
        };
        self.damage_roll = dmg_roll;
        self.view = Some((third.map_or(scene.eye, |t| t.origin), scene.angles.y));
        self.view_ps = Some(scene.view);
        self.aim = Some((scene.eye, scene.angles));
        // Live, the server flies a free spectator by its cmd's moves (the
        // binds, `play::cod4_input`); a capture's camera leaves the
        // recorded view once a key flies it. A followed view is the
        // snapshot's either way.
        let live = self.session.is_live();
        self.free = !live && !scene.following && (self.free || self.keys.moving());
        if self.free {
            cam.update(&self.keys.input(), cx.dt);
        } else {
            // The drawn view is the scene's: a capture's recorded one, live
            // the predicted one, or the third-person camera. The earthquakes
            // shake it (`CG_ShakeCamera`).
            let (pos, angles) = third.map_or((scene.eye, scene.angles), |t| (t.origin, t.angles));
            cam.pos = pos;
            let [p, y, _] = self.quakes.shake(now as i32, scene.eye);
            // A gunner's clamp leaves the pitch in 0..360 (345 is 15 up,
            // cod4-client-view.md 13); the camera's own pitch clamp reads
            // -180..180.
            cam.pitch = -angle_normalize180(angles.x + p + dmg_pitch).to_radians();
            cam.yaw = (angles.y + y).to_radians();
        }
        self.shellshock(cx, scene.render_time);
        prof.next(Stage::Pose);
        self.draw(cx, &scene);
        // `CG_CalcTurretViewValues`: on a gun (`eFlags` 0x300) the eye is
        // the gun's `tag_player` as the view turns it, and a held trigger
        // (`viewlocked` 2) jitters the view by the gun's
        // `vertViewJitter` / `horizViewJitter` (cod4-client-view.md 13).
        // In third person the camera backs off the gun's `tag_player`
        // raised by the view height, and nothing jitters (0x4508ba).
        if !self.free
            && let Some(eye) = self.turret_eye(&scene)
        {
            self.aim = Some((eye, scene.angles));
            if let Some(t) = third {
                let (pos, angles) = self.session.third_person_camera(
                    eye + Vec3::Z * t.view_height,
                    scene.angles,
                    t.dead_yaw,
                    scene.view.client_num,
                );
                cam.pos = pos;
                cam.pitch = -angle_normalize180(angles.x).to_radians();
                cam.yaw = angles.y.to_radians();
                self.view = Some((pos, scene.angles.y));
            } else {
                cam.pos = eye;
                self.view = Some((eye, scene.angles.y));
                let [p, y] = self.turret_jitter();
                cam.pitch -= p.to_radians();
                cam.yaw += y.to_radians();
            }
        }
        // The mouse's next turn is from the view drawn.
        self.cam_left = Some((cam.pitch, cam.yaw));
        prof.next(Stage::Vm);
        self.view_weapon(cx, &scene, cam);
        // A gunner's shots flash on the gun's own `tag_flash`, as the view
        // weapon's would (the flash step with `playerUsingTurret`).
        if let Some(gun) = self.own_gun()
            && let Some(t) = self.turret_tags.get(&gun)
        {
            self.view_tags = Some((gun, *t));
        }
        self.depth_of_field(cx, &scene, cam);
        // `FX_CullElemForSpawn` tests each element's `spawnRange` against
        // the drawn camera (cod4-effects.md 5): without it every element
        // of an effect spawned, its near and far versions both.
        cx.fx.set_view(cam.pos);
        // With flag 0x4 it also tests the frustum, the drawn view's
        // (cod4-effects.md 18).
        if let Some((fov_x, aspect)) = self.drawn_fov.get() {
            let (fwd, right, up) = crate::camera::basis(cam.yaw, cam.pitch);
            let tan_x = (fov_x.to_radians() / 2.0).tan();
            cx.fx.set_frustum(crate::fx::sim::Frustum::new(
                cam.pos,
                (fwd, -right, up),
                tan_x,
                tan_x / aspect,
            ));
        }
        prof.next(Stage::Events);
        self.events(cx, now);
        prof.next(Stage::Fx);
        self.effects_frame(cx, &scene);
        true
    }

    /// The level's first-use builds, run once its map has loaded and
    /// before its first frame, behind the loading screen as retail's
    /// `CG_Init` registers them (cod4-client-perf.md 17): the HUD's and the
    /// script menus' zones, the effects' sprite images and mark materials,
    /// and every entity model the level can draw uploaded. Each is a no-op
    /// the second time; [`Self::pump`] runs the first two again, which a
    /// session that never prewarms still needs.
    pub(crate) fn prewarm(&mut self, cx: &mut Cod2Frame) {
        let mut laps = vcod_common::laps::Laps::new("cod4 level prewarm");
        self.load_hud(cx);
        laps.lap("hud");
        if let Some(e) = &mut self.effects {
            take_level_effects(e, cx);
        }
        laps.lap("fx");
        let mut names: Vec<&String> = self
            .models
            .keys()
            .filter(|n| !self.uploaded.contains_key(*n))
            .collect();
        names.sort();
        let models: Vec<&Model> = names.iter().map(|n| &self.models[*n]).collect();
        let handles = cx.r.upload_dynamic_models(cx.fs, &models);
        for (m, h) in models.iter().zip(&handles) {
            if h.is_none() {
                log::warn!("cod4: {} did not upload", m.name);
            }
        }
        let names: Vec<String> = names.into_iter().cloned().collect();
        self.uploaded.extend(names.into_iter().zip(handles));
        laps.lap("models");
        laps.finish();
    }

    /// The level's HUD zones as [`crate::cod4_fly::load_with_hud`] read
    /// them beside the map: the prewarm's `load_hud` then reads none, and
    /// hands their menus to the session.
    pub(crate) fn install_hud(&mut self, z: crate::cod4_hud::LevelZones) {
        if self.hud.install_level(z) {
            self.menus_loaded = false;
        }
    }

    /// The level's HUD and script menu zones (`HudAssets::load`) under its
    /// `fs_game`, loaded again when either changes, and the menus' defs,
    /// strings and lists handed to the session.
    fn load_hud(&mut self, cx: &Cod2Frame) {
        let (Some(dir), Some(map)) = (cx.fs.dirs().first(), self.session.map_name()) else {
            return;
        };
        let reloaded = self
            .hud
            .load_under(dir, &map, self.session.fs_game().as_deref());
        if (reloaded || !self.menus_loaded)
            && let Some((defs, tables)) = self.hud.script_menus()
        {
            self.menus_loaded = true;
            self.session.menus.set_defs(defs.clone(), tables.clone());
            self.session.menus.strings = self.hud.strings();
            if let Some(d) = self.hud.startup_dvars() {
                self.session.menus.startup = d.clone();
            }
            if let Some(c) = self.hud.menu_cfgs() {
                self.session.menus.cfgs = c.clone();
            }
            if let Some(l) = self.hud.ui_lists() {
                self.session.menus.set_lists(l.clone());
            }
        }
    }

    /// Voice chat's frame: `+talk`'s frames encoded onto the session's
    /// batch while [`crate::voice::may_send`] holds, the heard ones
    /// played. `cl_voice` defaults to 1 and `rate` to the userinfo's 25000.
    fn voice_frame(&mut self, cx: &mut Cod2Frame) {
        use crate::voice::{VoiceChat, VoiceFrame, may_send};
        let atoi = vcod_cod4::menuexp::atoi;
        let (sv_voice, quality) = self.session.voice_settings();
        let cl_voice = cx.shell.cvar("cl_voice").is_none_or(|v| atoi(v) != 0);
        let rate = cx.shell.cvar("rate").map_or(25000, atoi);
        let active = self.session.client().snapshots.last().is_some();
        let held = self.session.input.held(crate::play::input::Action::Talk);
        self.session.voice.enabled = cl_voice;
        let f = VoiceFrame {
            sending: may_send(held, active, sv_voice, cl_voice, rate),
            quality,
        };
        self.voice
            .get_or_insert_with(|| VoiceChat::new(vcod_cod4::net::voice::SAMPLE_RATE))
            .frame(&mut self.session.voice, &f, Some(&mut *cx.audio));
    }

    /// The drawn snapshot's shellshock at the render time `now`
    /// (`play::cod4_shock`): the screen to the renderer, the mix to the
    /// audio, the kick and mouse caps kept for the draw and the mouse.
    fn shellshock(&mut self, cx: &mut Cod2Frame, now: i32) {
        let (parms, start, duration) = crate::play::cod4_shock::of_session(&self.session);
        let f = self.shock.frame(parms.as_ref(), start, duration, now);
        cx.r.set_cod4_shock(Some(f.screen));
        cx.audio.set_cod4_shellshock(cx.fs, &f.sound);
        self.shock_kick = f.kick;
        self.shock_look = f.look;
        self.turn
            .set(cx.dt * 1000.0, [f.look.max_pitch, f.look.max_yaw]);
    }

    /// The mouse's turn in degrees under the shellshock's sensitivity and
    /// caps (`CL_MouseMove`, `CL_CapTurnRate`).
    pub(crate) fn turn(&mut self, degrees: [f32; 2]) -> [f32; 2] {
        let s = self.shock_look.sensitivity;
        self.turn.turn(degrees.map(|d| d * s))
    }

    /// The frame's fired events and lines: the HUD's, the console's, their
    /// sounds and effects.
    fn events(&mut self, cx: &mut Cod2Frame, now: f64) {
        let fired = self.session.take_fired();
        let at = self
            .session
            .client()
            .snapshots
            .last()
            .map_or(0, |s| s.server_time);
        self.hud
            .update(&mut self.session, &fired, at.max(self.render_time));
        // `playLocalSound`'s `s`: the leader dialog, the announcer.
        for (alias, start) in self.hud.take_local_sounds() {
            if start {
                if let Some(id) = cx.audio.play_local(cx.fs, &alias) {
                    self.local_voices.insert(alias, id);
                }
            } else if let Some(id) = self.local_voices.remove(&alias) {
                cx.audio.stop(id);
            }
        }
        // The script menus' `play` (cod4-hud.md 20).
        for alias in self.session.menus.take_sounds() {
            cx.audio.play_local(cx.fs, &alias);
        }
        // Retail prints an event only under `cg_debugEvents`; a held sprint
        // raises two a cmd (cod4-pmove.md 8).
        for f in &fired {
            match f.entity {
                Some(n) => log::debug!("cod4 event {} (entity {n})", f.name),
                None => log::debug!("cod4 event {}", f.name),
            }
        }
        self.sound_and_fx(cx, &fired, now as i32);
        for line in self.session.take_lines() {
            console::log::print(&line);
        }
        if self.session.finished() && !self.ended {
            self.ended = true;
            console::log::print("cod4: the capture played out");
        }
    }

    /// The fog and vision set on the cgame's clock: configstring 9's fog
    /// faded to when it changes, 824's set blended, 825's while `night`
    /// (cod4-world.md 11).
    fn look_frame(&mut self, cx: &mut Cod2Frame, night: bool) {
        // the HUD keeps the `d` updates since the gamestate
        if self.session.gamestate().is_some() {
            let (hud, session) = (&self.hud, &self.session);
            let cs = |n: u16| hud.configstring(session, n);
            if let Some(fog) = self.look.configstrings(cs, self.render_time) {
                cx.r.set_fog_now(fog);
            }
        }
        cx.r.set_level_look(Some(self.look.at(self.render_time, night)));
    }

    /// The effects that run on from frame to frame (cod4-effects.md 11):
    /// this frame's tag poses for the bolted ones, the map's createfx
    /// ambience on the first frame, the `ET_FX` / `ET_LOOP_FX` entities
    /// and the looping elements' spawns.
    fn effects_frame(&mut self, cx: &mut Cod2Frame, scene: &Scene) {
        let tags = self.weapon_tags();
        let (Some(e), Some(gs)) = (&mut self.effects, self.session.gamestate()) else {
            return;
        };
        let now = f64::from(cx.time);
        e.set_tags(cx.fx, &tags, self.view_tags.as_ref());
        // The helicopters' smoke and wash, then every model tag an effect
        // rides.
        let clip = self.session.world().map(|w| w.clip.clone());
        let heli = self.vehicles.heli_fx(now * 1000.0, clip.as_deref());
        e.add_tags(cx.fx, &self.vehicles.tags());
        for (name, at, bolt) in heli {
            if let Some(p) = e.play_named(name, at, bolt) {
                let sounds = e.start(cx.fx, cx.fs, &p, now, None);
                cx.audio.play_fx(cx.fs, sounds);
            }
        }
        // `CG_Missile`: a drawn missile's `projTrailEffect` on its
        // `tag_fx`, once an entity (the trail flag; cod4-effects.md 16).
        self.trails_made
            .retain(|n| scene.entities.iter().any(|d| d.number == *n));
        for d in &scene.entities {
            let Visual::Model(name) = &d.visual else {
                continue;
            };
            let Some(("projectile", w)) = name.split_once(':') else {
                continue;
            };
            let Some(trail) = e
                .weapons
                .get(w)
                .and_then(|a| a.effects.get("projTrailEffect"))
            else {
                continue;
            };
            let Some((b, pose)) = self.vehicles.bolt(d.number, "tag_fx") else {
                continue;
            };
            if !self.trails_made.insert(d.number) {
                continue;
            }
            let trail = trail.clone();
            e.add_tags(cx.fx, &HashMap::from([(b, pose)]));
            let at = crate::fx::sim::SpawnAt::Oriented {
                pos: pose.origin,
                axis: pose.axis,
            };
            if let Some(p) = e.play_named(&trail, at, Some(b)) {
                let sounds = e.start(cx.fx, cx.fs, &p, now, None);
                cx.audio.play_fx(cx.fs, sounds);
            }
        }
        let mut sounds = e.start_ambience(cx.fx, cx.fs, (scene.render_time, now));
        if let Some(snap) = self.session.client().snapshots.last() {
            let cs = |n: u16| String::from_utf8_lossy(gs.configstring(n)).into_owned();
            sounds.extend(e.entity_fx(
                cx.fx,
                cx.fs,
                &snap.entities,
                &cs,
                (scene.render_time, now),
                scene.eye,
            ));
        }
        sounds.extend(e.run_loops(cx.fx, cx.fs, now));
        cx.audio.play_fx(cx.fs, sounds);
        // The entities' and the map's looping sounds.
        if self.sounds.is_some() {
            let cs = |n: u16| String::from_utf8_lossy(gs.configstring(n)).into_owned();
            let ents = self
                .session
                .client()
                .snapshots
                .last()
                .map_or(&[][..], |s| &s.entities[..]);
            // On the drawn vehicles' origins, as the audio's step moves
            // them ([`Self::entity_positions`]).
            let drawn: HashMap<u32, Vec3> = self.vehicles.origins().collect();
            let clip = self.session.world().map(|w| w.clip.clone());
            let bounds = |i: usize| {
                let m = clip.as_ref()?.submodels.get(i)?;
                Some((m.mins, m.maxs))
            };
            let pos_of = |s: &vcod_cod4::net::delta::EntityState| {
                cod4_audio::loop_origin(s, drawn.get(&s.number()).copied(), &bounds)
            };
            let loops = cod4_audio::loop_sounds(ents, &cs, &e.client_sounds, &pos_of);
            cx.audio.set_loop_aliases(cx.fs, &loops);
        }
        // What nothing refreshed this frame, as `SND_UpdateLoopingSounds`.
        cx.audio.reap_loops();
    }

    /// The view weapon of the drawn playerstate, posed for this frame; a
    /// camera flown off the recorded view draws none, and keeps the fly
    /// fov.
    fn view_weapon(&mut self, cx: &mut Cod2Frame, scene: &Scene, cam: &FlyCamera) {
        self.vm = None;
        self.view_tags = None;
        self.ads_dof = None;
        if self.free {
            self.view_weapon.clear();
            self.fov = None;
            return;
        }
        let list = self
            .session
            .gamestate()
            .map(weapon_list)
            .unwrap_or_default();
        let out = self.view_weapon.frame(
            &scene.view,
            &list,
            scene.hands.as_deref(),
            &self.weapons,
            &self.models,
            scene.render_time,
        );
        self.fov = Some(out.fov);
        if let Some(v) = &mut self.view_ps {
            v.gun_dir = out.gun_dir;
        }
        // `CG_AddViewWeapon` draws nothing in third person (0x45704e): the
        // own shots flash and leave from the body's gun.
        if scene.third_person.is_some() {
            return;
        }
        // `CG_PlayClientSoundAliasByName`: the clip's mapped notetracks.
        for alias in &out.sounds {
            cx.audio.play_local(cx.fs, alias);
        }
        // A scope's overlay hides the gun but not its tags
        // (cod4-client-view.md 4.6).
        let posed = out.frame.is_some() || out.tags.flash.is_some();
        if posed {
            let basis = crate::camera::basis(cam.yaw, cam.pitch);
            let world = |t: Option<(Vec3, Quat)>| t.map(|t| tag_to_world(t, cam.pos, basis));
            self.view_tags = Some((
                scene.view.client_num,
                WeaponTags {
                    flash: world(out.tags.flash),
                    brass: world(out.tags.brass),
                },
            ));
        }
        let Some(mut frame) = out.frame else {
            return;
        };
        frame.draw.light_origin = scene.light_eye;
        self.ads_dof = crate::play::cod4::weapon_name(&list, scene.view.view_weapon())
            .and_then(|w| self.weapons.get(w)?.view.as_ref())
            .map(|d| d.numbers.ads_dof);
        self.vm = Some(frame);
    }

    /// `CG_UpdateSceneDepthOfField` for the drawn view: the scripted
    /// `dof*` fields, else the sight's, with the gun's own range; the
    /// sight's targets reach as far as the view's forward trace
    /// (cod4-client-view.md 12).
    fn depth_of_field(&mut self, cx: &mut Cod2Frame, scene: &Scene, cam: &FlyCamera) {
        // `CG_UpdateAdsDof`'s trace mask
        const MASK: u32 = 0x0080_6c31;
        let (fwd, _, _) = crate::camera::basis(cam.yaw, cam.pitch);
        let end = cam.pos + fwd * 8192.0;
        let trace = self.session.world().map_or(8192.0, |w| {
            8192.0
                * w.clip
                    .trace(cam.pos, end, Vec3::ZERO, Vec3::ZERO, MASK)
                    .fraction
        });
        let dof = self
            .dof
            .frame(&scene.view, &scene.dof, self.ads_dof, trace, cx.dt);
        cx.r.set_cod4_dof(dof);
    }

    /// This frame's view weapon, once.
    /// Whether a snapshot's view has been drawn, the game frame
    /// `--exec-in-game` waits for.
    pub(crate) fn in_game(&self) -> bool {
        self.view.is_some()
    }

    /// The drawn playerstate's view fields, last pump's.
    #[cfg(test)]
    pub(crate) fn view_ps(&self) -> Option<&crate::cod4_viewmodel::ViewPs> {
        self.view_ps.as_ref()
    }

    pub(crate) fn take_view(&mut self) -> Option<ViewFrame> {
        self.vm.take()
    }

    /// The draw's angles, fov and view weapon: the camera's angles, 1.7's
    /// fov for a recorded view (`CG_GetViewFov`), `fly_fov` for a flown
    /// camera; the view weapon draws at the world's fov
    /// (cod4-client-view.md 4).
    pub(crate) fn draw_view(
        &self,
        fs: &vcod_common::pk3::Pk3Fs,
        r: &mut Renderer,
        cam: &FlyCamera,
        vm: Option<ViewFrame>,
        (fly_fov, aspect, _): (f32, f32, bool),
    ) -> crate::cod2_client::DrawView {
        // `CG_UpdateFov` (iw3mp.exe 0x450510) is Hor+ whatever
        // `vc_widescreen` says: the fov is a 4:3 view's width.
        let fov = self
            .fov
            .map_or(fly_fov, |f| crate::camera::view_fov_x(f, aspect, true));
        {
            let (w, h) = r.screen_size();
            let sp = crate::hud::cod2elem::ScreenPlacement::cod4(w, h);
            let seen: LoggedScreen = (w, h, fov, aspect, sp);
            if self.logged_screen.get() != Some(seen) {
                self.logged_screen.set(Some(seen));
                log::debug!(
                    "cod4 view: window {w}x{h} aspect {aspect:.4} fov_x {fov:.3} fov_y {:.3}; \
                     hud 4:3 column x {:.1}..{:.1}, left {:.1} right {:.1}",
                    crate::camera::fov_y(fov, aspect),
                    sp.axis(0, 0.0, 0.0, 0).0,
                    sp.axis(0, 640.0, 0.0, 0).0,
                    sp.axis(0, 0.0, 0.0, 1).0,
                    sp.axis(0, 0.0, 0.0, 3).0,
                );
            }
        }
        self.drawn_fov.set(Some((fov, aspect)));
        let vm = vm.map(|v| {
            if let Some(m) = &v.models {
                r.set_viewmodel(fs, m);
            }
            crate::renderer::VmDraw {
                fov_x: fov,
                ..v.draw
            }
        });
        // The shellshock's kick turns the drawn view only
        // (`CG_PerturbCamera`).
        let roll = if self.free { 0.0 } else { self.damage_roll };
        let (pitch, yaw, roll) = if self.shock_kick == [0.0, 0.0] {
            (cam.pitch, cam.yaw, roll.to_radians())
        } else {
            let [p, y, r] = crate::play::shock::kicked_angles(
                [-cam.pitch.to_degrees(), cam.yaw.to_degrees(), roll],
                self.shock_kick,
            );
            (-p.to_radians(), y.to_radians(), r.to_radians())
        };
        crate::cod2_client::DrawView {
            yaw,
            pitch,
            roll,
            fov,
            vm,
        }
    }

    /// The fired events' sounds and effects, the map's ambient, and the
    /// zones' tables handed to the audio and fx systems on the first call.
    fn sound_and_fx(&mut self, cx: &mut Cod2Frame, fired: &[Fired], now_ms: i32) {
        let tags = self.weapon_tags();
        let Some(gs) = self.session.gamestate() else {
            return;
        };
        let weapons = weapon_list(gs);
        let cs = |n: u16| String::from_utf8_lossy(gs.configstring(n)).into_owned();
        if let Some(s) = &mut self.sounds {
            if let Some(t) = s.take_table() {
                cx.audio.set_cod4_aliases(t);
            }
            let ambient = cod4_audio::ambient_alias(gs.configstring(CS_AMBIENT));
            cx.audio.set_ambient(cx.fs, ambient.as_deref());
            let view_flash = self.view_tags.as_ref().and_then(|(_, t)| t.flash);
            let flash = |e: Option<u32>| match e {
                None => view_flash.map(|f| f.0),
                Some(n) => tags.get(&n).and_then(|t| t.flash).map(|f| f.0),
            };
            let at = cod4_audio::Placement {
                own: self.session.own_client(),
                flash: &flash,
            };
            play_event_cues(cx.audio, cx.fs, s, &mut self.hud, fired, &weapons, &cs, &at);
        }
        // The blasts' pushes on the running and sleeping ragdolls.
        if let Some(s) = &self.sounds {
            for f in fired {
                if let Some(e) = crate::play::cod4_players::explosion_of(f, &weapons, &s.weapons) {
                    self.players.explode(&e);
                }
            }
        }
        // The client-side bullets of the other players' shots.
        let own = self.aim.map(|(eye, angles)| cod4_fx::OwnView {
            client: self.view_tags.as_ref().map(|(c, _)| *c).or_else(|| {
                self.view_ps
                    .filter(|_| self.third_person)
                    .map(|v| v.client_num)
            }),
            eye,
            angles,
            flash: self
                .view_tags
                .as_ref()
                .and_then(|(_, t)| t.flash)
                .map(|f| f.0),
            third_person: self.third_person,
        });
        let hits: Vec<_> = match (self.session.world(), &self.sounds) {
            (Some(w), Some(s)) => fired
                .iter()
                .map(|f| {
                    cod4_fx::client_bullets(
                        f,
                        &weapons,
                        &s.weapons,
                        &tags,
                        own.as_ref(),
                        cod4_fx::ShotWorld {
                            clip: &w.clip,
                            solids: &self.session.shot_solids,
                        },
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        if let Some(s) = &mut self.sounds {
            let eye = self.view.map(|(eye, _)| eye);
            for (f, hit) in fired.iter().zip(&hits) {
                for c in hit.iter().flat_map(|h| s.bullet_cues(f, &weapons, h, eye)) {
                    cx.audio.play(cx.fs, c);
                }
            }
        }
        let own_gun = self.own_gun();
        // In third person our own shots flash and eject on our body's gun,
        // as anyone's do (cod4-client-view.md 16.4).
        let own_body = self
            .view_ps
            .filter(|_| self.third_person)
            .map(|v| v.client_num);
        if let Some(e) = &mut self.effects {
            take_level_effects(e, cx);
            for f in fired {
                let body_shot;
                let f = match own_body {
                    Some(c) if f.entity.is_none() && f.name.starts_with("EV_FIRE_WEAPON") => {
                        body_shot = Fired {
                            entity: Some(c),
                            ..f.clone()
                        };
                        &body_shot
                    }
                    _ => f,
                };
                for p in e.plays(f, &weapons, &cs, &tags, self.view_tags.as_ref()) {
                    let sounds = e.start(cx.fx, cx.fs, &p, f64::from(cx.time), None);
                    cx.audio.play_fx(cx.fs, sounds);
                }
                // `CG_PlayFxOnTag`: the effect bolted to the entity's tag;
                // nothing when its model has no such tag.
                if f.name != "EV_PLAY_FX_ON_TAG" {
                    continue;
                }
                let tag_cs = cs(cod4_fx::CS_EFFECT_TAGS + f.parm as u16);
                let Some((id, tag)) = cod4_fx::effect_on_tag(&tag_cs) else {
                    continue;
                };
                let name = cs(cod4_fx::CS_EFFECT_NAMES + id);
                let Some((b, pose)) = f.entity.and_then(|n| self.vehicles.bolt(n, tag)) else {
                    log::debug!("cod4: {name} on {tag}: no such tag drawn");
                    continue;
                };
                e.add_tags(cx.fx, &HashMap::from([(b, pose)]));
                let at = crate::fx::sim::SpawnAt::Oriented {
                    pos: pose.origin,
                    axis: pose.axis,
                };
                if let Some(p) = e.play_named(&name, at, Some(b)) {
                    let sounds = e.start(cx.fx, cx.fs, &p, f64::from(cx.time), None);
                    cx.audio.play_fx(cx.fs, sounds);
                }
            }
            for (f, hit) in fired.iter().zip(&hits) {
                // The tracer test: never for the gun we man
                // (cod4-effects.md 12.2).
                #[cfg(test)]
                let before = e.tracers;
                if f.entity.is_none() || f.entity != own_gun {
                    e.tracers(cx.fx, hit, cx.time, (cx.dt * 1000.0) as i32);
                }
                #[cfg(test)]
                if !hit.is_empty() {
                    let axis = f
                        .entity
                        .and_then(|n| tags.get(&n)?.flash)
                        .map(|(_, q)| q * Vec3::X);
                    self.rounds_seen.push(RoundsSeen {
                        entity: f.entity,
                        aim: f.aim,
                        axis,
                        hits: hit.clone(),
                        tracers: e.tracers - before,
                    });
                }
                let plays: Vec<_> = hit
                    .iter()
                    .flat_map(|h| e.bullet_plays(f, &weapons, h))
                    .collect();
                for p in plays {
                    let sounds = e.start(cx.fx, cx.fs, &p, f64::from(cx.time), None);
                    cx.audio.play_fx(cx.fs, sounds);
                }
            }
        }
        for f in fired {
            if let Some(q) = cod4_fx::earthquake(f) {
                self.quakes.start(q, now_ms);
            }
            // `EV_FIRE_WEAPON_MG42` shakes the view near the gun
            // (`CG_StartShakeCamera(0.05, 100, origin, 100)`,
            // cod4-client-view.md 13).
            if f.name == "EV_FIRE_WEAPON_MG42" {
                let q = crate::play::quake::Quake {
                    start_ms: 0,
                    scale: 0.05,
                    duration_ms: 100.0,
                    radius: 100.0,
                    origin: f.origin,
                };
                self.quakes.start(q, now_ms);
            }
        }
    }

    /// The drawn bodies' weapon tags and the mounted guns' flashes, by
    /// entity: where the others' shots flash and leave from.
    fn weapon_tags(&self) -> HashMap<u32, WeaponTags> {
        let mut out = self.players.tags.clone();
        out.extend(self.turret_tags.iter().map(|(&n, &t)| (n, t)));
        out
    }

    /// The gun our drawn view is locked to (`eFlags` 0x300 and
    /// `viewlocked_entNum`, as `CG_mg42_PreControllers` tests them):
    /// `None` off a gun.
    fn own_gun(&self) -> Option<u32> {
        if self.free || self.view_ps?.eflags & 0x300 == 0 {
            return None;
        }
        let ps = &self.session.client().snapshots.last()?.ps;
        ps.get("viewlocked_entNum").filter(|&n| n != 1023)
    }

    /// The gun our view is locked to, its `tag_player` this frame: `None`
    /// off a gun or for a gun not drawn.
    fn turret_eye(&self, _scene: &Scene) -> Option<Vec3> {
        let gun = self.own_gun()?;
        self.vehicles.tag(gun, "tag_player").map(|(_, p)| p.origin)
    }

    /// `CG_CalcTurretViewValues`' jitter (pitch, yaw), degrees: under
    /// `viewlocked` 2 each angle `crandom()` times the gun's
    /// `vertViewJitter` and `horizViewJitter`, pitch drawn first; else 0.
    fn turret_jitter(&mut self) -> [f32; 2] {
        let Some(snap) = self.session.client().snapshots.last() else {
            return [0.0; 2];
        };
        if snap.ps.get("viewlocked") != Some(2) {
            return [0.0; 2];
        }
        let Some(gun) = self.own_gun() else {
            return [0.0; 2];
        };
        let weapon = snap
            .entities
            .iter()
            .find(|e| e.number() == gun)
            .map(|e| crate::play::cod4::ent_u32(e, "weapon"));
        let list = self
            .session
            .gamestate()
            .map(weapon_list)
            .unwrap_or_default();
        let Some(j) = weapon
            .and_then(|w| crate::play::cod4::weapon_name(&list, w))
            .and_then(|w| self.weapons.get(w))
            .map(|w| w.view_jitter)
        else {
            return [0.0; 2];
        };
        let mut crandom = || {
            self.jitter_rand = self.jitter_rand.wrapping_mul(214013).wrapping_add(2531011);
            let r = ((self.jitter_rand >> 16) & 0x7fff) as f32 / 32768.0;
            2.0 * (r - 0.5)
        };
        let p = j[0] * crandom();
        let y = j[1] * crandom();
        [p, y]
    }

    fn draw(&mut self, cx: &mut Cod2Frame, scene: &Scene) {
        let mut instances = Vec::new();
        let mut submodels = Vec::new();
        #[cfg(test)]
        self.drawn_names.borrow_mut().clear();
        let bodies = crate::frame_prof::span(crate::frame_prof::Stage::Bodies);
        let posed = self.pose_bodies(cx, scene, &mut instances);
        drop(bodies);
        self.vehicles.begin();
        self.turret_tags.clear();
        self.drawn_angles = scene
            .entities
            .iter()
            .map(|d| (d.number, d.angles))
            .collect();
        let own_gun = self.own_gun();
        // A gun's weapon, its `weapon` index into the weapon list.
        let list = self
            .session
            .gamestate()
            .map(weapon_list)
            .unwrap_or_default();
        let gun_weapons: HashMap<u32, String> = self
            .session
            .client()
            .snapshots
            .last()
            .map(|s| {
                s.entities
                    .iter()
                    .filter(|e| e.etype() == crate::play::cod4::ET_MG42)
                    .filter_map(|e| {
                        let w = crate::play::cod4::ent_u32(e, "weapon");
                        let name = crate::play::cod4::weapon_name(&list, w)?;
                        Some((e.number(), name.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for d in &scene.entities {
            if posed.contains(&d.number) {
                continue;
            }
            let pose = Mat4::from_rotation_translation(angles_quat(d.angles.to_array()), d.origin);
            // A model's tags for the effects on them, a helicopter's
            // rotors and gun.
            let bones = match self.model_of(d).and_then(|m| self.models.get(&m)) {
                Some(m) => match d.turret {
                    Some(gun) => {
                        // `CG_mg42_DoControllers`: the gun we man turns
                        // with our drawn view off its own angles, the
                        // others by their lerped `gunAngles`
                        // (cod4-client-view.md 13).
                        let gun = match own_gun == Some(d.number) {
                            true => [
                                angle_normalize180(scene.angles.x - d.angles.x),
                                angle_normalize180(scene.angles.y - d.angles.y),
                                gun[2],
                            ],
                            false => gun,
                        };
                        // Our own gun stays on its idle; another plays its
                        // fire while its firing bit is on.
                        let own = own_gun == Some(d.number);
                        let anims = gun_weapons
                            .get(&d.number)
                            .and_then(|w| self.weapons.get(w)?.gun_anims.as_ref())
                            .map(|clips| crate::cod4_vehicles::GunAnims {
                                clips,
                                goal: usize::from(!own && d.firing),
                                render_ms: scene.render_time,
                            });
                        let bones = self.vehicles.place_turret(d.number, m, pose, gun, anims);
                        let tag = |t: &str| {
                            self.vehicles.tag(d.number, t).map(|(_, p)| {
                                let q = Quat::from_mat3(&glam::Mat3::from_cols(
                                    p.axis[0], p.axis[1], p.axis[2],
                                ));
                                (p.origin, q)
                            })
                        };
                        // `CG_FireWeapon` flashes an `ET_MG42` on its
                        // `tag_flash` and ejects nothing.
                        let tags = WeaponTags {
                            flash: tag("tag_flash"),
                            brass: None,
                        };
                        self.turret_tags.insert(d.number, tags);
                        bones
                    }
                    None => {
                        let bones = self.vehicles.place(
                            d.number,
                            m,
                            pose,
                            d.vehicle.as_ref(),
                            scene.render_time,
                        );
                        // `CG_FireWeapon`'s helicopter arm flashes on the
                        // helicopter's own `tag_flash` and ejects on its
                        // `tag_brass` (cod4-client-view.md 11.1).
                        if d.vehicle.is_some() {
                            let tag = |t: &str| {
                                self.vehicles.tag(d.number, t).map(|(_, p)| {
                                    let q = Quat::from_mat3(&glam::Mat3::from_cols(
                                        p.axis[0], p.axis[1], p.axis[2],
                                    ));
                                    (p.origin, q)
                                })
                            };
                            let tags = WeaponTags {
                                flash: tag("tag_flash"),
                                brass: tag("tag_brass"),
                            };
                            self.turret_tags.insert(d.number, tags);
                        }
                        bones
                    }
                },
                None => None,
            };
            if let Visual::Submodel(n) = d.visual {
                let still =
                    d.origin.abs().max_element() < 0.01 && d.angles.abs().max_element() < 0.01;
                submodels.push((n, if still { Mat4::IDENTITY } else { pose }));
                continue;
            }
            let (handle, transform, bones) = match self.handle(cx, d) {
                Some(h) => (h, pose * lie_down(d), bones),
                None => {
                    let (lo, hi) = box_of(d.etype);
                    let Some(h) = self.upload(cx, BOX, || Some(cube())) else {
                        continue;
                    };
                    let scale = Mat4::from_scale_rotation_translation(
                        hi - lo,
                        Quat::IDENTITY,
                        (lo + hi) * 0.5,
                    );
                    (h, pose * scale, None)
                }
            };
            #[cfg(test)]
            self.drawn_names.borrow_mut().push((
                instances.len() as u32,
                self.model_of(d).unwrap_or_default(),
                d.number,
                d.origin,
            ));
            instances.push(DynamicModelInstance {
                model: handle,
                transform,
                bones,
                light_origin: light_origin(d),
                shader_time: None,
            });
        }
        self.vehicles.end();
        self.drawn = (instances.len(), submodels.len(), posed.len());
        crate::frame_prof::count(crate::frame_prof::Count::Models, instances.len());
        cx.r.set_dynamic_models(&instances);
        cx.r.set_submodels(&submodels);
    }

    /// Poses the scene's bodies and adds each rig's models, skinned, to
    /// `out`; returns the entity numbers drawn so.
    fn pose_bodies(
        &mut self,
        cx: &mut Cod2Frame,
        scene: &Scene,
        out: &mut Vec<DynamicModelInstance>,
    ) -> Vec<u32> {
        let Some(src) = &self.bodies else {
            return Vec::new();
        };
        let views: Vec<(u32, &BodyView)> = scene
            .entities
            .iter()
            .filter_map(|d| Some((d.number, d.body.as_ref()?)))
            .collect();
        if let Some(gs) = self.session.gamestate() {
            self.players.lean =
                crate::play::cod4_predict::lean_from_mirror(|i| gs.configstring(i).to_vec());
        }
        // Each gunner's gun as drawn: its model's two tags at the bind and
        // its weapon's column step.
        let guns: Vec<Option<crate::play::cod4_players::GunInfo>> = views
            .iter()
            .map(|(_, v)| {
                let (number, weapon) = v.gun.as_ref()?;
                let d = scene
                    .entities
                    .iter()
                    .find(|d| d.number == *number && d.turret.is_some())?;
                let model = self.model_of(d)?;
                let bones = &self.models.get(&model)?.bones;
                let tag = |n: &str| bones.iter().find(|b| b.name == n).map(|b| b.pos);
                let weapon = weapon.clone()?;
                Some(crate::play::cod4_players::GunInfo {
                    frame: vcod_cod2::player::GunFrame {
                        model,
                        weapon: weapon.clone(),
                        origin: d.origin,
                        angles: d.angles.to_array(),
                        barrel: d.turret?,
                    },
                    tags: (tag("tag_aim")?, tag("tag_weapon")?),
                    rotate_inc: self.weapons.get(&weapon)?.rotate_inc,
                })
            })
            .collect();
        // `ragdoll_enable` (on unless set to 0) and the level's ragdoll
        // def and clip map (cod4-player-anims.md 10).
        let enable = cx
            .shell
            .cvar("ragdoll_enable")
            .is_none_or(|v| vcod_cod4::menuexp::atoi(v) != 0);
        self.players.ragdoll = self.session.world().and_then(|w| {
            Some(crate::play::cod4_players::RagdollWorld {
                def: w.ragdoll.clone()?,
                clip: w.clip.clone(),
                dvars: vcod_cod4::ragdoll::Dvars {
                    enable,
                    ..Default::default()
                },
            })
        });
        let posed = self.players.posed(
            src,
            &self.weapons,
            &views,
            &guns,
            scene.render_time,
            scene.server_time,
        );
        let mut drawn = Vec::new();
        for (i, rig, p) in posed {
            let light_origin = views[i].1.frame.light_origin();
            for ((name, model), (_, skins)) in rig.models.iter().zip(p.models) {
                let Some(model) = self.upload(cx, name, || Some((**model).clone())) else {
                    continue;
                };
                out.push(DynamicModelInstance {
                    model,
                    transform: p.transform,
                    bones: Some(skins),
                    light_origin,
                    shader_time: None,
                });
            }
            drawn.push(views[i].0);
        }
        drawn
    }

    /// The xmodel `d` draws, by name.
    fn model_of(&self, d: &Drawn) -> Option<String> {
        let Visual::Model(name) = &d.visual else {
            return None;
        };
        crate::cod4_fly::visual_model(&self.weapons, name)
    }

    /// The xmodel `d` draws. A set of hidden parts draws the whole model's
    /// upload less the surfaces whose `partBits` meet the entity's (the
    /// renderer skips them, `R_SkinXModel`, cod4-client-view.md 15), so
    /// of a destructible car's intact and cracked glass only the state's
    /// pair draws, and a newly broken part builds no GPU data in play
    /// (cod4-client-perf.md 18).
    fn handle(&mut self, cx: &mut Cod2Frame, d: &Drawn) -> Option<ModelHandle> {
        let name = self.model_of(d)?;
        let hides = |p: &[u32; 4]| p.iter().zip(d.part_bits).any(|(p, h)| p & h != 0);
        let keep: Option<Vec<bool>> = self
            .parts
            .get(&name)
            .filter(|p| p.iter().any(hides))
            .map(|p| p.iter().map(|p| !hides(p)).collect());
        let full = match self.uploaded.get(&name) {
            Some(h) => (*h)?,
            None => {
                let model = self.models.get(&name).cloned();
                self.upload(cx, &name, || model)?
            }
        };
        let Some(keep) = keep else {
            return Some(full);
        };
        let [a, b, c, e] = d.part_bits;
        let key = format!("{name}#{a:08x}{b:08x}{c:08x}{e:08x}");
        if let Some(h) = self.uploaded.get(&key) {
            return *h;
        }
        let h =
            cx.r.dynamic_model_hiding(full, self.models.get(&name)?, &keep);
        self.uploaded.insert(key, h);
        h
    }

    fn upload(
        &mut self,
        cx: &mut Cod2Frame,
        name: &str,
        model: impl FnOnce() -> Option<Model>,
    ) -> Option<ModelHandle> {
        if let Some(h) = self.uploaded.get(name) {
            return *h;
        }
        let h = model().and_then(|m| upload(cx.r, cx.fs, &m));
        self.uploaded.insert(name.to_string(), h);
        h
    }

    /// A new map or renderer: every handle is gone.
    pub(crate) fn reset_render(&mut self) {
        self.uploaded.clear();
        // The view weapon's models go to the new renderer on its next frame.
        self.view_weapon.clear();
    }

    /// `+scores` / `-scores`: the board shows, and a live session asks
    /// for the scores (`CG_ScoresDown`).
    pub(crate) fn set_scores(&mut self, down: bool) {
        if self.hud.set_scores(down, self.render_time) {
            self.session.command("score");
        }
    }

    /// The waypoints in the world, put under `quads` (the 2D HUD).
    pub(crate) fn world_hud(
        &self,
        quads: &mut Vec<crate::hud::HudQuad>,
        view_proj: glam::Mat4,
        (yaw, pitch, roll): (f32, f32, f32),
        size: (f32, f32),
    ) {
        let up = crate::camera::up_hint(yaw, pitch, roll);
        let fwd = crate::camera::basis(yaw, pitch).0;
        let right = fwd.cross(up).normalize();
        let view = crate::cod2_world_hud::WorldView {
            view_proj,
            right,
            up: right.cross(fwd),
            size,
        };
        let mut q = self.hud.world_quads(&self.session, &view, self.render_time);
        // The offscreen-capable waypoints, 2D, under the rest of the HUD:
        // the eye's axes, and its fov read back off the projection.
        if let Some((origin, _)) = self.view.filter(|_| !self.free) {
            let ndc = |p: Vec3| {
                let c = view_proj * (origin + fwd + p).extend(1.0);
                c.truncate() / c.w
            };
            let eye = crate::cod4_waypoints::Eye {
                origin,
                forward: fwd,
                left: -view.right,
                up: view.up,
                tan: [1.0 / ndc(view.right).x, 1.0 / ndc(view.up).y],
                size: [size.0, size.1],
            };
            q.extend(
                self.hud
                    .offscreen_waypoints(&self.session, &eye, self.render_time),
            );
        }
        q.append(quads);
        *quads = q;
    }

    /// The 2D layer for a `size` window at the drawn time.
    pub(crate) fn hud_quads(
        &self,
        size: (f32, f32),
        shell: &crate::console::shell::Shell,
    ) -> Vec<crate::hud::HudQuad> {
        let mut out = self
            .hud
            .build(&self.session, size, self.render_time, self.view, shell);
        // The crosshair at the fov and aspect the world draws at
        // (`draw_view`, `Renderer::set_view_aspect`).
        if let (Some(ps), Some(fov), false) = (&self.view_ps, self.fov, self.free) {
            let aspect = self.drawn_fov.get().map_or_else(
                || {
                    let setting = shell.cvar("r_aspectRatio").unwrap_or_default();
                    crate::camera::cod4_aspect(setting, size.0, size.1)
                },
                |(_, a)| a,
            );
            let fov_x = crate::camera::view_fov_x(fov, aspect, true);
            let tan_x = (fov_x * 0.5).to_radians().tan();
            let tan = [tan_x, tan_x / aspect];
            out.extend(self.hud.crosshair(&self.session, ps, tan, size, shell));
        }
        if let (Some(ps), Some((_, yaw)), false) = (&self.view_ps, self.view, self.free) {
            out.extend(
                self.hud
                    .damage_icons(&self.session, ps, size, self.render_time, yaw),
            );
            // The ceiling probe (cod4-hud.md 21): the player box 1024 up,
            // traced only when a frag asks
            let ceiling = || {
                self.session.world().map_or(f32::MAX, |w| {
                    use vcod_cod4::pmove::{PLAYER_MAXS, PLAYER_MINS};
                    let up = ps.origin + Vec3::Z * 1024.0;
                    let t = w.clip.trace(ps.origin, up, PLAYER_MINS, PLAYER_MAXS, 1);
                    if t.fraction < 1.0 {
                        1024.0 * t.fraction
                    } else {
                        f32::MAX
                    }
                })
            };
            out.extend(self.hud.grenade_indicators(
                &self.session,
                ps,
                &ceiling,
                size,
                self.render_time,
            ));
        }
        if let Some(a) = self.hud.menu_assets() {
            let now = self.start.map_or(0, |t| {
                crate::play::cod4::clock_now()
                    .saturating_duration_since(t)
                    .as_millis() as i32
            });
            out.extend(crate::cod4_menu::quads(
                &self.session.menus,
                &a,
                size,
                now,
                self.menu_has_mouse(),
            ));
            // The team and class menus' minimap (cod4-hud.md 15.8).
            out.extend(self.hud.menu_owner_draws(&self.session, size, self.view));
        }
        out
    }

    /// Where the newest snapshot's entities stand, a drawn model where it
    /// drew: the sounds riding them (a helicopter's loop, a jet's linked
    /// `script_origin`) follow them.
    pub(crate) fn entity_positions(&self) -> HashMap<u32, Vec3> {
        let mut out: HashMap<u32, Vec3> = self
            .session
            .client()
            .snapshots
            .last()
            .map(|s| {
                s.entities
                    .iter()
                    .map(|e| (e.number(), crate::play::cod4::origin_of(e)))
                    .collect()
            })
            .unwrap_or_default();
        out.extend(self.vehicles.origins());
        out
    }

    /// The angles each entity's voices turn with: its drawn angles where it
    /// drew, else its angle trajectory at the drawn time.
    /// `SND_GetCurrent3DPosition` (iw3mp.exe 0x5c28d0) places a voice at
    /// its start offset in the axes of `CG_GetSoundEntityOrientation`
    /// (0x43dd30), the centity's drawn angles (cod4-sound.md 4).
    pub(crate) fn entity_angles(&self) -> HashMap<u32, Vec3> {
        let mut out: HashMap<u32, Vec3> = self
            .session
            .client()
            .snapshots
            .last()
            .map(|s| {
                s.entities
                    .iter()
                    .map(|e| {
                        let a = crate::play::cod4::angles_at(e, self.render_time);
                        (e.number(), a)
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.extend(&self.drawn_angles);
        out
    }

    /// The level's effects went to the fx system, their materials built.
    #[cfg(test)]
    pub(crate) fn effects_taken(&self) -> bool {
        self.effects.as_ref().is_some_and(|e| e.taken())
    }

    /// The effects the cgame's own paths started, by name.
    #[cfg(test)]
    pub(crate) fn effects_named(&self) -> Option<std::collections::HashSet<String>> {
        Some(self.effects.as_ref()?.named.clone())
    }

    /// The fire events' rounds so far ([`RoundsSeen`]).
    /// The model keys uploaded so far, and the bodies the last pump posed.
    #[cfg(test)]
    pub(crate) fn uploads(&self) -> (BTreeSet<String>, usize) {
        (self.uploaded.keys().cloned().collect(), self.drawn.2)
    }

    /// Whether the level's script menus reached the session.
    #[cfg(test)]
    pub(crate) fn menus_loaded(&self) -> bool {
        self.menus_loaded
    }

    #[cfg(test)]
    pub(crate) fn take_rounds_seen(&mut self) -> Vec<RoundsSeen> {
        std::mem::take(&mut self.rounds_seen)
    }

    pub(crate) fn debug_lines(&self) -> Vec<String> {
        let c = self.session.client();
        let mut lines = vec![format!(
            "cod4: snapshots {}  time {}  models {}  brush models {}  bodies {}",
            c.snapshots.len(),
            self.time,
            self.drawn.0,
            self.drawn.1,
            self.drawn.2
        )];
        if let (Some(s), Some(e)) = (&self.sounds, &self.effects) {
            lines.push(format!(
                "cod4 av: sounds {} missing {}  effects {} missing {}  elements left out {}  loops {}  heli smoke {} wash {}",
                s.started,
                s.missing.len(),
                e.spawned,
                e.missing.len(),
                e.gaps.values().sum::<usize>(),
                e.loopers(),
                self.vehicles.smoke,
                self.vehicles.wash
            ));
        }
        let (parts, flying) = self.session.physics.counts();
        if parts > 0 {
            lines.push(format!("cod4 physics: launched {parts}  flying {flying}"));
        }
        lines
    }
}

/// The level's effects, once they are read: their sprite images and mark
/// materials build with the level, not in an effect's or a surface's first
/// frame (cod4-map-sweep.md, "The soak"), and the defs go to the fx system.
fn take_level_effects(e: &mut Cod4Effects, cx: &mut Cod2Frame) {
    let Some(all) = e.take_effects() else {
        return;
    };
    let (mut marks, mut sprites) = (BTreeSet::new(), BTreeSet::new());
    for m in all.values().flat_map(|e| &e.emitters) {
        let set = if m.kind == "Decal" {
            &mut marks
        } else {
            &mut sprites
        };
        set.extend(m.shaders.iter().map(|s| &**s));
    }
    sprites.insert(crate::fx::sim::TRACER_SHADER);
    cx.r.prewarm_marks(cx.fs, marks);
    cx.r.prewarm_fx(cx.fs, sprites);
    for (k, v) in all {
        cx.fx.insert(&k, v);
    }
}

fn upload(r: &mut Renderer, fs: &vcod_common::pk3::Pk3Fs, m: &Model) -> Option<ModelHandle> {
    let h = r.upload_dynamic_model(fs, m);
    if h.is_none() {
        log::warn!("cod4: {} did not upload", m.name);
    }
    h
}

const BOX: &str = "<box>";

/// The vehicles' state for a map, its rotor clip out of `bodies`.
fn vehicles(bodies: Option<&BodySource>) -> Cod4Vehicles {
    let rotors = bodies.and_then(|b| b.decoded_anim(vcod_cod4::anim::ENTITY_CLIPS[0])?.ok());
    Cod4Vehicles::new(rotors)
}

/// A body lying where it fell when it could not be posed: its bind pose
/// laid on its back along its yaw.
fn lie_down(d: &Drawn) -> Mat4 {
    match d.etype {
        ET_PLAYER_CORPSE => {
            Mat4::from_translation(Vec3::Z * 8.0) * Mat4::from_rotation_y(-90f32.to_radians())
        }
        _ => Mat4::IDENTITY,
    }
}

/// `eType` 11 (a turret) and 14 (a vehicle): the slots of 1.7's per-eType
/// table list (`cod4_lnxded` 0x8228be0) whose rows are the general table
/// and the vehicle one (0x8225f80), cod4-netfields.md 1. The names are
/// INFERRED.
const ET_MG42: u32 = 11;
const ET_VEHICLE: u32 = 14;

/// Where a non-body entity samples the light grid (cod4-world.md 3.3):
/// its drawn origin raised 32 for a turret or a vehicle, 4 for the rest
/// (`CG_General`, `CG_Item`, `CG_Missile`, `CG_ScriptMover`: iw3mp.exe
/// 0x432950, 0x432f77, 0x43280a; `CG_mg42` 0x432e63, the vehicle's
/// 0x44f857). A player or a corpse is lit from `BodyFrame::light_origin`.
fn light_origin(d: &Drawn) -> Vec3 {
    let up = match d.etype {
        ET_MG42 | ET_VEHICLE => 32.0,
        _ => 4.0,
    };
    d.origin + Vec3::Z * up
}

/// A type's box when it has no model: a standing player's bounds, a body
/// on the ground, a weapon, a grenade.
fn box_of(etype: u32) -> (Vec3, Vec3) {
    let (lo, hi) = match etype {
        ET_PLAYER => ([-15.0, -15.0, 0.0], [15.0, 15.0, 70.0]),
        ET_PLAYER_CORPSE => ([-36.0, -15.0, 0.0], [36.0, 15.0, 12.0]),
        ET_ITEM => ([-16.0, -3.0, 0.0], [16.0, 3.0, 6.0]),
        ET_MISSILE => ([-3.0, -3.0, -3.0], [3.0, 3.0, 3.0]),
        _ => ([-8.0, -8.0, -8.0], [8.0, 8.0, 8.0]),
    };
    (Vec3::from(lo), Vec3::from(hi))
}

fn one_bone(name: &str) -> Bone {
    Bone {
        name: name.to_string(),
        parent: -1,
        pos: Vec3::ZERO,
        rot: Quat::IDENTITY,
        local_pos: Vec3::ZERO,
        local_rot: Quat::IDENTITY,
        hit_mins: Vec3::ZERO,
        hit_maxs: Vec3::ZERO,
        hit_location: 0,
    }
}

fn vertex(pos: [f32; 3], normal: [f32; 3], uv: [f32; 2], color: [u8; 4]) -> Vertex {
    Vertex {
        pos,
        normal,
        uv,
        bone_indices: [0; 4],
        bone_weights: [1.0, 0.0, 0.0, 0.0],
        color,
        tangent: [0.0; 3],
        binormal: [0.0; 3],
    }
}

/// The unit cube about the origin, one material with no colour map (drawn
/// with the default image).
fn cube() -> Model {
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut n = [0.0; 3];
            n[axis] = sign;
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let base = verts.len() as u16;
            for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                let mut p = [0.0; 3];
                p[axis] = 0.5 * sign;
                p[u] = a;
                p[v] = b;
                verts.push(vertex(p, n, [a + 0.5, b + 0.5], [255; 4]));
            }
            let quad = if sign > 0.0 {
                [0, 1, 2, 0, 2, 3]
            } else {
                [0, 2, 1, 0, 3, 2]
            };
            indices.extend(quad.map(|k| base + k));
        }
    }
    Model {
        name: BOX.to_string(),
        bones: vec![one_bone("tag_origin")],
        lods: vec![Lod {
            name: BOX.to_string(),
            dist: 0.0,
            surfaces: vec![Surface {
                verts,
                indices,
                material: 0,
            }],
            materials: vec!["$box".to_string()],
        }],
        collision: Vec::new(),
    }
}

/// The session's weapon list, once a gamestate came.
fn weapon_list_of(s: &Cod4Session) -> Option<std::sync::Arc<[String]>> {
    s.gamestate().map(weapon_list)
}

#[cfg(test)]
mod saw_tests {
    use super::*;
    use crate::play::cod4::tests::{clk_now, clk_sleep, run_held, since as clk_since};
    use crate::play::input::Action;
    use std::time::Duration;

    /// One client frame of the test, the mouse's turn first.
    type Frame1<'a> = dyn FnMut(
            &mut Cod4Client,
            &mut FlyCamera,
            &mut Renderer,
            &mut crate::fx::sim::FxSystem,
            [f32; 2],
        ) + 'a;

    /// What the gunner's frames showed.
    #[derive(Debug, Default)]
    struct Seen {
        /// Mounted, the gun's pixels: how many, and their mean brightness.
        gun_pixels: usize,
        gun_light: f32,
        /// The effects the gun's rounds started.
        effects: std::collections::HashSet<String>,
        /// Each frame's drawn pitch (degrees, up negative) while the mouse
        /// pushes up.
        pitches: Vec<f32>,
        /// The farthest the eye got from the gun's `tag_player` as posed
        /// from the drawn view, while turning.
        eye_off: f32,
        /// The barrel's drawn yaw off the view's, at the end of a turn.
        yaw_off: f32,
        /// The flash's distance from the eye.
        flash_from_eye: Option<f32>,
        /// The gun's fire events' client-side rounds while firing.
        rounds: Vec<super::RoundsSeen>,
    }

    /// vcod-server's war on mp_backlot with us joined on allies, played
    /// through `Cod4Client::pump` as `main` runs a frame: behind the crouch
    /// SAW 393 (cod4-gsc.md 9.2) a use press mans it; the gun then draws
    /// lit, its rounds flash on its own `tag_flash` in front of the eye,
    /// the eye sits on the gun's `tag_player`, the barrel follows the view,
    /// and a mouse pushed up past the arc holds the view on the arc without
    /// a jump. `VCOD_C4_SHOTS=<dir>` writes the frames. Needs `COD4_DIR`
    /// and an adapter with BC textures (lavapipe will do).
    #[test]
    fn cod4_live_saw_gunner_view() {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let root = install.root().to_path_buf();
        let _slot = crate::renderer::pipeline_check::live_session_slot();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_backlot".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("sv_cheats", "1");
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.load(Some(&install)).unwrap();
        // The watcher steps in the gunner's frames, on one held clock.
        let (seen, watched) = run_held(
            &mut sv,
            move |addr| {
                let watch = GunWatch::connect(addr);
                play(&root, addr, watch)
            },
            |_, _, _| true,
        );
        let Some(seen) = seen else {
            return;
        };
        let shots = sv.game.level.as_ref().unwrap().combat_shots().to_vec();
        println!("{seen:#?}");
        println!("server rounds {shots:?}");
        println!("{watched:?}");
        // A spectator sees the gun play its fire anim while it fires and
        // its idle again after (`CG_mg42_PreControllers`).
        assert!(watched.firing_frames > 10, "{watched:?}");
        assert!(watched.fire_goal, "the fire anim never became the goal");
        assert!(
            watched.moved > 0.01,
            "the fire anim moved no bone: {watched:?}"
        );
        assert!(watched.idle_after, "the gun never went back to its idle");
        assert!(seen.gun_pixels > 2000, "{} gun pixels", seen.gun_pixels);
        assert!(
            seen.gun_light > 40.0,
            "the gun drew dark: {}",
            seen.gun_light
        );
        assert!(
            seen.effects.iter().any(|e| e.contains("muzzleflash")),
            "{:?}",
            seen.effects
        );
        let flash = seen.flash_from_eye.expect("no flash tag");
        assert!(
            flash > 10.0 && flash < 80.0,
            "the flash {flash} from the eye"
        );
        assert!(
            seen.eye_off < 0.01,
            "the eye {} off tag_player",
            seen.eye_off
        );
        assert!(
            seen.yaw_off.abs() < 0.01,
            "the barrel {} off the view",
            seen.yaw_off
        );
        // The arc's top is 15 up: the view climbs to it and stays.
        let top = seen.pitches.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            (top + 15.0).abs() < 0.1,
            "the top {top}: {:?}",
            seen.pitches
        );
        for w in seen.pitches.windows(2) {
            assert!(w[1] <= w[0] + 0.01, "the view jumped: {:?}", seen.pitches);
        }
        gun_rounds_follow_the_server(&seen.rounds, &shots);
    }

    /// The gun's client-side rounds against the server's (cod4-effects.md
    /// 12.2), the view held still: one fire event per server round, each
    /// seeded by our `commandTime` on the frame it fired (two rounds of one
    /// snapshot share it, as 1.7's do), leaving `tag_flash` off its forward
    /// axis by the disc point of that seed at the gun's `playerSpread`; the
    /// axis within 0.05 degrees of the view the server's round left along;
    /// no tracer off the gun we man.
    fn gun_rounds_follow_the_server(
        seen: &[super::RoundsSeen],
        shots: &[(usize, i32, Vec3, Vec3, Vec3)],
    ) {
        const GUN: u32 = 393;
        let ours: Vec<_> = seen.iter().filter(|r| r.entity == Some(GUN)).collect();
        assert!(shots.len() >= 10, "{} server rounds", shots.len());
        // The last frame's rounds may still be on the wire when the
        // trigger lets go.
        assert!(
            ours.len() <= shots.len() && ours.len() + 2 >= shots.len(),
            "{} client rounds, {} server rounds",
            ours.len(),
            shots.len()
        );
        let tan = |deg: f32| (deg * 0.017_453_292).tan();
        for r in &ours {
            let aim = r.aim.expect("a gun's round without its spread");
            assert!(aim.spread > 0.0, "no playerSpread: {aim:?}");
            assert_eq!(r.tracers, 0, "the gun we man drew a tracer");
            let axis = r.axis.expect("no tag_flash");
            for (k, h) in r.hits.iter().enumerate() {
                let (x, y) = vcod_cod4::bullet::random_dir(aim.seed + k as i32);
                let want = (x.hypot(y) * tan(aim.spread)).atan().to_degrees();
                // `atan2` of the cross and dot: `acos` reads 0 under a
                // hundredth of a degree in f32.
                let d = (h.end - h.start).normalize();
                let off = d.cross(axis).length().atan2(d.dot(axis)).to_degrees();
                assert!(
                    (off - want).abs() < 0.01,
                    "round {} off its axis {off}, its disc {want}",
                    aim.seed
                );
                let centre = shots
                    .iter()
                    .map(|s| axis.angle_between((s.3 - s.2).normalize()).to_degrees())
                    .fold(f32::MAX, f32::min);
                assert!(centre < 0.05, "the axis {centre} off the server's view");
            }
        }
    }

    /// What [`GunWatch`] saw of the gun.
    #[derive(Debug, Default)]
    struct Watched {
        /// Frames the gun's firing bit was on.
        firing_frames: usize,
        /// Whether its fire anim became the pose's goal.
        fire_goal: bool,
        /// The most a skin matrix moved against the controllers-only pose.
        moved: f32,
        /// Whether the goal went back to the idle once the firing stopped.
        idle_after: bool,
    }

    /// A spectator beside the gunner of [`cod4_live_saw_gunner_view`],
    /// posing the gun as the draw does ([`Cod4Vehicles::place_turret`]
    /// with its weapon's clips, the fire anim the goal while the firing
    /// bit is on), a step in each of the gunner's frames.
    struct GunWatch {
        s: Cod4Session,
        t0: Instant,
        loaded: Option<crate::cod4_fly::Bodies>,
        animated: Cod4Vehicles,
        plain: Cod4Vehicles,
        w: Watched,
    }

    impl GunWatch {
        fn connect(addr: std::net::SocketAddr) -> GunWatch {
            let join = crate::play::cod4_menus::Join {
                team: Some("spectator".into()),
                class: None,
            };
            let s = Cod4Session::connect(addr, "watcher", vcod_cod4::net::conn::Cp::Bare, join)
                .unwrap();
            GunWatch {
                s,
                t0: clk_now(),
                loaded: None,
                animated: Cod4Vehicles::default(),
                plain: Cod4Vehicles::default(),
                w: Watched::default(),
            }
        }

        fn step(&mut self, root: &std::path::Path) {
            const GUN: u32 = 393;
            let s = &mut self.s;
            s.step(clk_since(self.t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            let Some(scene) = s.scene() else { return };
            let Some(d) = scene.entities.iter().find(|d| d.number == GUN) else {
                return;
            };
            let (Some(gun), Visual::Model(name)) = (d.turret, &d.visual) else {
                return;
            };
            let (models, weapons, _) = self.loaded.get_or_insert_with(|| {
                crate::cod4_fly::load_bodies(root, "mp_backlot", &s.entity_assets()).unwrap()
            });
            let list = s.gamestate().map(weapon_list).unwrap_or_default();
            let weapon = s
                .client()
                .snapshots
                .last()
                .and_then(|f| f.entities.iter().find(|e| e.number() == GUN))
                .and_then(|e| {
                    crate::play::cod4::weapon_name(&list, crate::play::cod4::ent_u32(e, "weapon"))
                })
                .and_then(|n| weapons.get(n));
            let Some(clips) = weapon.and_then(|w| w.gun_anims.as_ref()) else {
                return;
            };
            let model_name = match name.split_once(':') {
                Some(("weapon", n)) => weapons.get(n).and_then(|w| w.world.clone()),
                _ => Some(name.clone()),
            };
            let Some(model) = model_name.and_then(|n| models.get(&n)) else {
                return;
            };
            let at = Mat4::from_rotation_translation(angles_quat(d.angles.to_array()), d.origin);
            let anims = crate::cod4_vehicles::GunAnims {
                clips,
                goal: usize::from(d.firing),
                render_ms: scene.render_time,
            };
            let a = self.animated.place_turret(GUN, model, at, gun, Some(anims));
            let b = self.plain.place_turret(GUN, model, at, gun, None);
            let goal = self.animated.guns.get(&GUN).map(|c| c.goal);
            let w = &mut self.w;
            w.firing_frames += usize::from(d.firing);
            w.fire_goal |= goal == Some(1);
            w.idle_after |= w.fire_goal && goal == Some(0);
            if let (Some(a), Some(b)) = (a, b) {
                for (x, y) in a.iter().zip(&b) {
                    w.moved = w.moved.max((x.w_axis - y.w_axis).length());
                }
            }
        }

        fn finish(mut self) -> Watched {
            self.s.disconnect();
            self.w
        }
    }

    /// The client's half of [`cod4_live_saw_gunner_view`]; `None` without
    /// an adapter.
    fn play(
        root: &std::path::Path,
        addr: std::net::SocketAddr,
        mut watch: GunWatch,
    ) -> (Option<Seen>, Watched) {
        use crate::audio::{AudioOpts, AudioSystem, bank};
        use crate::renderer::{CullMode, Frame};
        const GUN: u32 = 393;
        const W: u32 = 640;
        const H: u32 = 384;
        let join = crate::play::cod4_menus::Join {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s =
            Cod4Session::connect(addr, "gunner", vcod_cod4::net::conn::Cp::Bare, join).unwrap();
        let t0 = clk_now();
        while s.spawn().is_none() && clk_since(t0) < Duration::from_secs(60) {
            s.step(clk_since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            watch.step(root);
            clk_sleep(Duration::from_millis(16));
        }
        assert!(s.spawn().is_some(), "never spawned");
        let map = crate::cod4_fly::load(root, None, "mp_backlot", &s.entity_assets()).unwrap();
        let mut fs = crate::cod4_fly::open_fs(root, None, None).unwrap();
        for (p, w) in &map.sound_files {
            fs.overlay(p, w.clone());
        }
        let Some((mut r, errors)) = Renderer::headless_sized(&fs, false, 1, (W, H)) else {
            s.disconnect();
            return (None, watch.finish());
        };
        r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
        r.set_material_lib(map.materials.clone());
        r.load_world(&map.bsp, &fs).unwrap();
        let shell = crate::console::shell::Shell::new();
        let localized = vcod_common::localize::Localized::load(&fs);
        let ui = crate::frontend::Ui::without_menus(&fs);
        let mut audio = AudioSystem::with_aliases(
            &fs,
            AudioOpts {
                enabled: false,
                volume: shell.cvar_f32("mss_volume"),
                rate: bank::mixer_rate(shell.cvar_f32("mss_khz") as i32),
            },
            Default::default(),
        );
        audio.on_gamestate("mp_backlot");
        let mut fx = crate::fx::sim::FxSystem::new();
        let mut c = Cod4Client::new(
            s,
            map.models,
            map.parts,
            map.weapons,
            map.bodies,
            map.av,
            map.world,
            map.look,
        );
        let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
        let mut seen = Seen::default();
        let start = clk_now();
        let mut last = start;
        // One frame as `main` runs it, the mouse's turn first.
        let mut frame = |c: &mut Cod4Client,
                         cam: &mut FlyCamera,
                         r: &mut Renderer,
                         fx: &mut crate::fx::sim::FxSystem,
                         look: [f32; 2]| {
            cam.look(look);
            let now = clk_now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            let mut cx = Cod2Frame {
                fs: &fs,
                shell: &shell,
                localized: &localized,
                audio: &mut audio,
                fx,
                ui: &ui,
                r,
                console_open: false,
                time: clk_since(start).as_secs_f32(),
                dt,
            };
            c.pump(&mut cx, cam);
            cx.fx.step(dt, cx.time, None);
            c.session.take_commands();
            watch.step(root);
            clk_sleep(Duration::from_millis(16));
        };
        let run = |c: &mut Cod4Client,
                   cam: &mut FlyCamera,
                   r: &mut Renderer,
                   fx: &mut crate::fx::sim::FxSystem,
                   frame: &mut Frame1,
                   ms: u64| {
            let end = clk_now() + Duration::from_millis(ms);
            while clk_now() < end {
                frame(c, cam, r, fx, [0.0, 0.0]);
            }
        };
        let ps = |c: &Cod4Client, n: &str| {
            c.session
                .client()
                .snapshots
                .last()
                .and_then(|f| f.ps.get(n))
                .unwrap_or(0)
        };
        run(&mut c, &mut cam, &mut r, &mut fx, &mut frame, 2500);
        c.session.command("setviewpos -770 68 320 0");
        run(&mut c, &mut cam, &mut r, &mut fx, &mut frame, 1500);
        c.session.input.key(Action::Use, true);
        run(&mut c, &mut cam, &mut r, &mut fx, &mut frame, 200);
        c.session.input.key(Action::Use, false);
        run(&mut c, &mut cam, &mut r, &mut fx, &mut frame, 800);
        assert_eq!(ps(&c, "viewlocked_entNum"), GUN, "not on the gun");
        let shot = |r: &mut Renderer, cam: &FlyCamera| {
            let (fwd, _, up) = crate::camera::basis(cam.yaw, cam.pitch);
            const FOV: f32 = 55.0;
            let f = |r: &Renderer| Frame {
                view_proj: crate::camera::view_proj_from(
                    cam.pos,
                    cam.yaw,
                    cam.pitch,
                    0.0,
                    FOV,
                    r.aspect(),
                ),
                eye: cam.pos,
                fwd,
                up,
                time: clk_since(start).as_secs_f32(),
                fov_x: FOV,
                cull: CullMode::On,
                hud_lines: Vec::new(),
            };
            r.render(f(r), None);
            r.read_offscreen().unwrap().2
        };
        let save = |px: &[u8], name: &str| {
            if let Some(dir) = std::env::var_os("VCOD_C4_SHOTS") {
                let rgba: Vec<u8> = px.chunks(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
                let path = std::path::Path::new(&dir).join(format!("saw_{name}.tga"));
                image::save_buffer(path, &rgba, W, H, image::ExtendedColorType::Rgba8).unwrap();
            }
        };
        let with = shot(&mut r, &cam);
        save(&with, "mounted");
        r.set_dynamic_models(&[]);
        let without = shot(&mut r, &cam);
        let diff: Vec<f32> = with
            .chunks(4)
            .zip(without.chunks(4))
            .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 8))
            .map(|(p, _)| f32::from(p[0].max(p[1]).max(p[2])))
            .collect();
        seen.gun_pixels = diff.len();
        seen.gun_light = diff.iter().sum::<f32>() / diff.len().max(1) as f32;
        // Fire a second.
        c.session.input.key(Action::Attack, true);
        let end = clk_now() + Duration::from_millis(1000);
        let mut saved = false;
        while clk_now() < end {
            frame(&mut c, &mut cam, &mut r, &mut fx, [0.0, 0.0]);
            if let Some((_, p)) = c.vehicles.tag(GUN, "tag_flash") {
                seen.flash_from_eye = Some(p.origin.distance(cam.pos));
            }
            seen.rounds.extend(c.take_rounds_seen());
            if !saved && clk_now() + Duration::from_millis(500) > end {
                saved = true;
                save(&shot(&mut r, &cam), "firing");
            }
        }
        c.session.input.key(Action::Attack, false);
        seen.effects = c.effects_named().unwrap_or_default();
        // Turn right and up past the arcs, a few degrees a frame.
        for _ in 0..40 {
            frame(&mut c, &mut cam, &mut r, &mut fx, [-1.5, -1.0]);
            seen.pitches.push(-cam.pitch.to_degrees());
            if let Some((_, p)) = c.vehicles.tag(GUN, "tag_player") {
                seen.eye_off = seen.eye_off.max(p.origin.distance(cam.pos));
            }
        }
        run(&mut c, &mut cam, &mut r, &mut fx, &mut frame, 300);
        save(&shot(&mut r, &cam), "up");
        if let Some((_, p)) = c.vehicles.tag(GUN, "tag_flash") {
            let f = p.axis[0];
            let yaw = f.y.atan2(f.x).to_degrees();
            seen.yaw_off = angle_normalize180(yaw - cam.yaw.to_degrees());
        }
        c.session.disconnect();
        for e in errors.lock().unwrap().iter() {
            log::warn!("wgpu: {e}");
        }
        (Some(seen), watch.finish())
    }
}
