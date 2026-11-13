//! The console's command layer: retail's tokenizer, a cvar table, key binds
//! and the client commands vcod implements (docs/research/cod11-console.md).
//! Commands that act on the window or the connection come back as
//! [`Effect`]s for `main.rs` to carry out; the rest run here.

use std::collections::BTreeMap;

use vcod_common::game::Game;
use vcod_common::localize::Localized;
use vcod_common::net::{self, Userinfo};
use vcod_common::text::atoi_msvc;

use crate::play::input::Action;

use super::keys;

/// Something a command asks of the app, in the order the commands ran.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Print(String),
    Clear,
    Connect(String),
    Disconnect,
    Reconnect,
    Quit,
    /// `record [name]`; no name picks the next free `demoNNNN`.
    Record(Option<String>),
    StopRecord,
    /// `demo <name>`, or CoD2's `timedemo <name>`, which plays it under
    /// `timedemo` (Mac 0x14c288).
    Demo {
        name: String,
        timedemo: bool,
    },
    /// `rcon <args>`: the arguments after the word, as tokenized
    /// ([`super::rcon::packet`] rebuilds the line).
    Rcon(Vec<String>),
    /// A client command for the server: `say`, `say_team`, `cmd <text>`.
    Forward(String),
    /// A command the client does not know. Retail forwards it to the server
    /// while connected (`CL_ForwardCommandToServer`), except `+`/`-` ones.
    Unknown {
        word: String,
        line: String,
    },
    /// A `+button` bind's press or release.
    Button(Action, bool),
    /// A one-shot bind (`gocrouch`, `weaponslot`, `weapnext`, `weapalt`).
    Impulse(Action),
    /// `weapon <name|number>`, the argument as typed ("" for none).
    Weapon(String),
    Scores(bool),
    MessageMode {
        team: bool,
    },
    ToggleConsole,
    /// A userinfo cvar (`name`) changed.
    Userinfo,
    /// A bind or an archived cvar changed.
    SaveConfig,
    /// `exec <file>`: run a config file from the game's search path.
    Exec(String),
    /// `vid_restart`: apply `r_mode` and `r_fullscreen` to the window and
    /// restart the UI.
    VidRestart,
    /// `snd_restart`: reopen the sound output under the latched `mss_*`
    /// cvars, then `vid_restart` (CoDMP.exe 0x40fdf0).
    SndRestart,
    /// CoD4's `r_applyPicmip` (`R_ApplyPicmip`, iw3mp.exe 0x5f2c80): the
    /// picmip dvars read again and the images reloaded at them.
    ApplyPicmip,
    /// `ui_load`: reload the menus off `ui_menuFiles`.
    UiLoad,
    /// `screenshot` (`jpeg` false) or `screenshotJPEG` and the arguments
    /// after the word (crate::screenshot).
    Screenshot {
        jpeg: bool,
        args: Vec<String>,
    },
    /// `mp_QuickMessage`, a cgame command (cgame_mp_x86.dll 0x300140e0):
    /// open the quick message menu.
    QuickMessage,
    /// `cg_shellshock <duration> [file]`, a cgame command (0x30013e70):
    /// the arguments after the word.
    ShellShock(Vec<String>),
    /// CoD2's `openscriptmenu <menu> <response>` (`CL_OpenScriptMenu_f`,
    /// Mac 0x1484fe): a script menu response sent as if `menu` had
    /// answered it (docs/research/cod2-protocol.md 9).
    OpenScriptMenu {
        menu: String,
        response: String,
    },
}

/// One cvar. `archive` is retail's `CVAR_ARCHIVE`: written to the config.
/// `cheat` is `CVAR_CHEAT`: only `sv_cheats 1` lets it move off its default.
/// `latch` is `CVAR_LATCH`: a set waits in `latched` until the next restart.
/// `rom` is `CVAR_ROM`: only the client itself sets it (`Shell::set_rom`).
#[derive(Debug, Clone)]
pub struct Cvar {
    pub name: String,
    pub value: String,
    pub default: String,
    pub archive: bool,
    pub userinfo: bool,
    pub cheat: bool,
    pub latch: bool,
    pub rom: bool,
    pub init: bool,
    pub latched: Option<String>,
}

/// Retail's cvar flag bits (docs/research/cod11-console.md, section 5).
const ARCHIVE: u32 = 0x1;
const USERINFO: u32 = 0x2;
/// `CVAR_INIT`: only the command line or a forced set moves it.
const INIT: u32 = 0x10;
const LATCH: u32 = 0x20;
const ROM: u32 = 0x40;
const CHEAT: u32 = 0x200;

/// The client commands, for `cmdlist` and completion.
const COMMANDS: &[&str] = &[
    "+actionslot",
    "+activate",
    "+attack",
    "+back",
    "+binoculars",
    "+breath_sprint",
    "+forward",
    "+frag",
    "+gostand",
    "+holdbreath",
    "+leanleft",
    "+leanright",
    "+left",
    "+lookdown",
    "+lookup",
    "+melee",
    "+melee_breath",
    "+mlook",
    "+movedown",
    "+moveleft",
    "+moveright",
    "+moveup",
    "+prone",
    "+reload",
    "+right",
    "+scores",
    "+smoke",
    "+speed",
    "+speed_throw",
    "+sprint",
    "+strafe",
    "+talk",
    "+toggleads_throw",
    "+usereload",
    "-actionslot",
    "-activate",
    "-attack",
    "-back",
    "-binoculars",
    "-breath_sprint",
    "-forward",
    "-frag",
    "-gostand",
    "-holdbreath",
    "-leanleft",
    "-leanright",
    "-left",
    "-lookdown",
    "-lookup",
    "-melee",
    "-melee_breath",
    "-mlook",
    "-movedown",
    "-moveleft",
    "-moveright",
    "-moveup",
    "-prone",
    "-reload",
    "-right",
    "-scores",
    "-smoke",
    "-speed",
    "-speed_throw",
    "-sprint",
    "-strafe",
    "-talk",
    "-toggleads_throw",
    "-usereload",
    "bind",
    "bindlist",
    "cg_shellshock",
    "chatmodepublic",
    "chatmodeteam",
    "clear",
    "cmd",
    "cmdlist",
    "connect",
    "cvarlist",
    "demo",
    "disconnect",
    "echo",
    "exec",
    "gocrouch",
    "goprone",
    "leaveads",
    "lowerstance",
    "messagemode",
    "messagemode2",
    "mp_QuickMessage",
    "openscriptmenu",
    "quit",
    "r_applypicmip",
    "raisestance",
    "rcon",
    "reconnect",
    "record",
    "say",
    "say_team",
    "screenshot",
    "screenshotJPEG",
    "set",
    "seta",
    "setfromcvar",
    "setfromdvar",
    "snd_restart",
    "stoprecord",
    "toggle",
    "toggleads",
    "toggleconsole",
    "togglecrouch",
    "toggleprone",
    "ui_load",
    "unbind",
    "unbindall",
    "version",
    "vid_restart",
    "weapalt",
    "weapnext",
    "weapon",
    "weaponslot",
    "weapprev",
];

/// The `+button` commands and the input each holds.
fn button(word: &str) -> Option<Action> {
    Some(match word {
        "forward" => Action::Forward,
        "back" => Action::Back,
        "moveleft" => Action::Left,
        "moveright" => Action::Right,
        "gostand" => Action::Jump,
        // CoDMP.exe 0x40a670, 0x40a700, 0x40aa60 (cod11-console.md 9).
        "moveup" => Action::MoveUp,
        "movedown" => Action::MoveDown,
        "prone" => Action::HoldProne,
        "speed" => Action::Ads,
        "attack" => Action::Attack,
        "melee" => Action::Melee,
        "activate" => Action::Use,
        "reload" => Action::Reload,
        "leanleft" => Action::LeanLeft,
        "leanright" => Action::LeanRight,
        "sprint" => Action::Sprint,
        "mlook" => Action::MLook,
        "strafe" => Action::Strafe,
        // CoDMP.exe 0x40a7a0..0x40a840 (cod11-console.md 5).
        "left" => Action::TurnLeft,
        "right" => Action::TurnRight,
        "lookup" => Action::LookUp,
        "lookdown" => Action::LookDown,
        "frag" => Action::Frag,
        "smoke" => Action::Smoke,
        "binoculars" => Action::Binoculars,
        "holdbreath" => Action::HoldBreath,
        "melee_breath" => Action::MeleeBreath,
        "usereload" => Action::UseReload,
        // CoD4's (`CL_InitInput`): `+speed_throw` is the held
        // sight with the throw key, as vcod's `+speed` already sends.
        "speed_throw" => Action::Ads,
        "breath_sprint" => Action::BreathSprint,
        "toggleads_throw" => Action::ToggleAdsThrow,
        "talk" => Action::Talk,
        _ => return None,
    })
}

/// `weaponslot`'s argument and the slot it asks for (cgame 0x3000f220, then
/// `atol` at 0x3003836a): a slot name, case folded, else a number. Every
/// protocol's name table is a prefix of UO's, and `PlayInput` refuses a slot
/// past the protocol's last, so `satchel` does nothing on 1.1 as there.
fn weapon_slot(arg: &str) -> usize {
    vcod_common::weapon::SLOT_NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(arg))
        .unwrap_or_else(|| {
            let s = arg.trim_start();
            let digits = s.strip_prefix('+').unwrap_or(s);
            if s.starts_with('-') {
                return 0;
            }
            digits
                .chars()
                .take_while(char::is_ascii_digit)
                .fold(0usize, |n, c| {
                    n.saturating_mul(10)
                        .saturating_add(usize::from(c as u8 - b'0'))
                })
        })
}

/// The binds `vcod` starts with when it has no config: the stock keys
/// `main.rs` hard-coded before the console existed, and the vote keys
/// `default_mp.cfg` (localized_english_pak0) binds.
pub const DEFAULT_BINDS: &[(&str, &str)] = &[
    ("TAB", "+scores"),
    ("SPACE", "+gostand"),
    ("1", "weaponslot primary"),
    ("2", "weaponslot primaryb"),
    ("3", "weaponslot pistol"),
    ("4", "weaponslot grenade"),
    ("`", "toggleconsole"),
    ("A", "+moveleft"),
    ("C", "gocrouch"),
    ("D", "+moveright"),
    ("E", "+leanright"),
    ("F", "+activate"),
    ("Q", "+leanleft"),
    ("R", "+reload"),
    ("S", "+back"),
    ("T", "messagemode"),
    ("V", "mp_QuickMessage"),
    ("W", "+forward"),
    ("Y", "messagemode2"),
    ("F1", "vote yes"),
    ("F2", "vote no"),
    ("CTRL", "goprone"),
    // United Offensive's `default_mp.cfg`; 1.1 leaves ALT unbound and
    // its cmd has no sprint bit.
    ("ALT", "+sprint"),
    ("SHIFT", "+melee"),
    ("MOUSE1", "+attack"),
    ("MOUSE2", "+speed"),
    ("MWHEELDOWN", "weapnext"),
    ("MWHEELUP", "weapprev"),
];

/// CoD2 1.3's `default_mp.cfg` binds (`iw_00.iwd`) where they differ
/// from [`DEFAULT_BINDS`]: [`Shell::use_cod2_binds`] puts each on a key
/// still holding vcod's CoD1 default or none.
pub const COD2_BINDS: &[(&str, &str)] = &[
    ("SHIFT", "+melee_breath"),
    ("MOUSE2", "toggleads"),
    ("1", "weapnext"),
    ("2", "weapnext"),
    ("MOUSE3", "+frag"),
    ("G", "+frag"),
    ("4", "+smoke"),
    ("B", "+binoculars"),
    ("X", "openscriptmenu ingame changeweapon"),
    ("Z", "+talk"),
];

/// CoD4 1.7's `default_mp_controls.cfg` binds (a rawfile of
/// `localized_code_post_gfx_mp.ff`) where they differ from CoD2's:
/// [`Shell::use_cod4_binds`] puts each on a key still holding vcod's CoD1
/// or CoD2 default, or none.
pub const COD4_BINDS: &[(&str, &str)] = &[
    ("SHIFT", "+breath_sprint"),
    ("MOUSE2", "+toggleads_throw"),
    ("V", "+melee"),
    ("B", "mp_QuickMessage"),
    ("N", "+actionslot 1"),
    ("5", "+actionslot 3"),
    ("6", "+actionslot 4"),
    ("7", "+actionslot 2"),
    ("T", "chatmodepublic"),
    ("Y", "chatmodeteam"),
];

/// The archived cvars the stock options screens set beyond the ones above,
/// at the defaults CoDMP.exe and cgame register them with
/// (docs/research/cod11-front-end.md, section 14), so a choice made there
/// survives a restart. What vcod drives with each is in that section;
/// vcod's window starts at its own size and windowed, so `r_mode` -1 and
/// `r_fullscreen` 0 stand in for retail's 3 and 1, and `r_picmip`,
/// `r_picmip2` and `r_swapInterval` start at 0, 0 and 1, what retail's
/// first-run `configure_mp.csv` picks for any machine past its top row,
/// instead of the registered 1, 2 and 0.
const MENU_CVARS: &[(&str, &str)] = &[
    ("mss_volume", "0.8"),
    ("mss_khz", "44"),
    ("mss_3d_provider", "Miles Fast 2D Positional Audio"),
    ("r_mode", "-1"),
    ("r_fullscreen", "0"),
    ("r_picmip", "0"),
    ("r_picmip2", "0"),
    ("r_textureMode", "GL_LINEAR_MIPMAP_NEAREST"),
    ("r_texturebits", "0"),
    ("r_gamma", "1.0"),
    ("r_ignorehwgamma", "0"),
    ("r_lodscale", "1"),
    ("r_lodbias", "0"),
    ("r_dynamiclight", "1"),
    ("r_dlightQuality", "1"),
    ("r_swapInterval", "1"),
    ("r_nv_fog_dist", "1"),
    ("cl_freelook", "1"),
    ("m_filter", "0"),
    ("cg_drawCrosshair", "1"),
    ("cg_drawStatus", "1"),
    // Registered by CoDMP.exe and read by nothing in 1.1, 1.5 or UO, so the
    // compass and the (absent) team overlay ignore them in retail too.
    ("cg_drawCompass", "1"),
    ("cg_drawTeamOverlay", "2"),
    ("cg_marks", "1"),
    // The cgame's, archived (cgame 0x30075110).
    ("cg_scoreboardScrollStep", "3"),
    ("cg_brass", "1"),
    ("cg_blood", "1"),
    // United Offensive's cgame, archived (uo cgame 0x30085d60): the
    // wheel and tread dust.
    ("cg_vehicletrails", "1"),
];

/// The voice options' dvars (cod2-voice.md 4, cod4-voice.md 4):
/// `winvoice_mic_mute` (1) mutes the microphone's line in the Windows
/// mixer's playback, `winvoice_mic_reclevel` (65535, 0..65535) sets its
/// recording volume, `winvoice_save_voice` (0) appends heard voice to
/// `voice.wav`, `winvoice_mic_scaler` (1, 0.25..2) scales each captured
/// sample. vcod reads the level and the scaler (`voice::mic_gain`).
const WINVOICE_DVARS: [(&str, &str, u32); 4] = [
    ("winvoice_mic_mute", "1", ARCHIVE),
    ("winvoice_mic_reclevel", "65535", ARCHIVE),
    ("winvoice_save_voice", "0", ARCHIVE),
    ("winvoice_mic_scaler", "1", ARCHIVE),
];

/// [`Shell::register_cod4_options`]'s dvars: name, default, flags
/// (`Dvar_RegisterNew` 0x56c350 calls, cod4-front-end.md 9).
const COD4_OPTION_DVARS: [(&str, &str, u32); 56] = [
    // the crosshair names' wait and fade, ints, cheat-protected
    // (`iw3mp.exe` 0x43d801..0x43d916, flags 0x80; cod4-hud.md 17)
    ("cg_friendlyNameFadeIn", "0", CHEAT),
    ("cg_enemyNameFadeIn", "250", CHEAT),
    ("cg_friendlyNameFadeOut", "1500", CHEAT),
    ("cg_enemyNameFadeOut", "250", CHEAT),
    // the names over heads (`iw3mp.exe` 0x43b1d8: flags 0x81; 0x43d4e5..
    // 0x43d959: flags 0x80; cod4-hud.md 17.1)
    ("cg_drawCrosshairNames", "1", ARCHIVE | CHEAT),
    ("cg_drawFriendlyNames", "1", CHEAT),
    ("cg_drawThroughWalls", "0", CHEAT),
    ("cg_overheadNamesMaxDist", "10000", CHEAT),
    ("cg_overheadNamesNearDist", "256", CHEAT),
    ("cg_overheadNamesFarDist", "1024", CHEAT),
    ("cg_overheadNamesFarScale", "0.6", CHEAT),
    ("cg_overheadNamesSize", "0.5", CHEAT),
    ("cg_overheadNamesFont", "2", CHEAT),
    ("cg_overheadIconSize", "0.7", CHEAT),
    ("cg_overheadRankSize", "0.5", CHEAT),
    ("cg_overheadNamesGlow", "0 0 0 1", CHEAT),
    // `Voice_Init`'s (`iw3mp.exe` 0x57a92e..0x57aa37, flags 1; CoD2's at
    // `CoD2MP_s.exe` 0x4b8dd0, flags 0x1001): see [`WINVOICE_DVARS`]
    WINVOICE_DVARS[0],
    WINVOICE_DVARS[1],
    WINVOICE_DVARS[2],
    WINVOICE_DVARS[3],
    // the custom classes' names, archived (`LiveStorage_StatsInit`,
    // 0x4fc5d7: flags 1; cod4-front-end.md 4.1)
    ("customclass1", "", ARCHIVE),
    ("customclass2", "", ARCHIVE),
    ("customclass3", "", ARCHIVE),
    ("customclass4", "", ARCHIVE),
    ("customclass5", "", ARCHIVE),
    // the texture page's (cod4-front-end.md 9.3): 1.7's picmip dvars are
    // archived, not latched; `r_applyPicmip` reloads the images at them
    ("r_picmip_manual", "0", ARCHIVE),
    ("r_picmip", "0", ARCHIVE),
    ("r_picmip_bump", "0", ARCHIVE),
    ("r_picmip_spec", "0", ARCHIVE),
    ("r_texFilterAnisoMin", "1", ARCHIVE),
    ("r_texFilterAnisoMax", "16", ARCHIVE),
    // latched to the `vid_restart` (0x62d358: flags 0x21)
    ("r_aspectRatio", "auto", ARCHIVE | LATCH),
    ("r_glow_allowed", "1", ARCHIVE),
    // 0x62c669, flags 0x80 (`DVAR_CHEAT`; cod4-world.md 11.4)
    ("r_glow", "1", CHEAT),
    ("r_zFeather", "1", ARCHIVE),
    // 0x62c59f, registered as r_zFeather is (cod4-effects.md 17)
    ("r_distortion", "1", ARCHIVE),
    ("r_dof_enable", "1", ARCHIVE),
    ("r_specular", "1", 0),
    // 0x62a50c, flags 0x80 (`DVAR_CHEAT`): 0 draws no fog
    // (cod4-client-perf.md 25)
    ("r_fog", "1", CHEAT),
    ("r_dlightLimit", "4", 0),
    ("r_lodScaleRigid", "1", ARCHIVE),
    ("r_lodBiasRigid", "0", ARCHIVE),
    ("r_drawWater", "1", ARCHIVE),
    ("r_texFilterMipMode", "Unchanged", ARCHIVE),
    ("monkeytoy", "0", ARCHIVE),
    // iw3mp.exe 0x5ba36b, archived (cod4-player-anims.md 10)
    ("ragdoll_enable", "1", ARCHIVE),
    // the third-person view, 0x43ccb1..0x43cdbf: flags 0x80, the range
    // 120 in 0..1024, the angle 0 in -180..360 (cod4-client-view.md 16)
    ("cg_thirdPerson", "0", CHEAT),
    ("cg_thirdPersonRange", "120", CHEAT),
    ("cg_thirdPersonAngle", "0", CHEAT),
    // the scoreboard's ping graph, 0x449b84..0x44a0a3 (cod4-hud.md 7.1)
    ("cg_scoreboardPingGraph", "0", 0),
    ("cg_ScoresPing_MaxBars", "4", ARCHIVE),
    ("cg_ScoresPing_Interval", "100", ARCHIVE),
    ("cg_ScoresPing_HighColor", "0.8 0 0 1", ARCHIVE),
    ("cg_ScoresPing_MedColor", "0.8 0.8 0 1", ARCHIVE),
    ("cg_ScoresPing_LowColor", "0 0.75 0 1", ARCHIVE),
    ("cg_ScoresPing_BgColor", "0.25 0.25 0.25 0.5", ARCHIVE),
];

/// `r_aspectRatio`'s names (iw3mp.exe 0x71c5f0), its enum's order.
pub const COD4_ASPECT_RATIOS: [&str; 4] = ["auto", "standard", "wide 16:10", "wide 16:9"];

/// The stamp [`Shell::config_text`] writes as `vc_cfgVersion`; one past
/// the newest [`STALE_DEFAULTS`] row's version (cod11-console.md 8).
pub const CONFIG_VERSION: u32 = 1;

/// The first line of every config vcod writes, since the first one.
const CONFIG_HEADER: &str = "// generated by vcod, do not modify";

/// vcod's own old defaults of archived cvars: (the stamp that retired it,
/// name, the old default, the games that had it). A vcod config stamped
/// below the row's version holding the old value gets the cvar's current
/// default ([`Shell::upgrade_config`]). An old default that was retail's
/// own (`com_maxfps` 85, `rate` 5000, `r_picmip` 1) has no row.
const STALE_DEFAULTS: &[(u32, &str, &str, &[Game])] = &[
    // 4x MSAA on CoD1 and UO until 68d97193, on CoD4 until 36372025;
    // retail's 1 since (cod11-client-perf.md 4, cod4-client-perf.md 19)
    (1, "r_aaSamples", "4", &[Game::Cod1, Game::Cod4]),
    // vcod's name until f5635f70; CL_Init's "Unknown Soldier" since
    (1, "name", "vcod", &[Game::Cod1, Game::Cod2, Game::Cod4]),
];

/// The menu cvars vcod applies only at a restart, registered `0x21`
/// like retail: the image settings (`R_Register`, CoDMP.exe 0x4b3300) wait
/// for `vid_restart`, `mss_khz` (0x44fe60) for `snd_restart`.
fn menu_cvar_latch(name: &str) -> u32 {
    match name {
        "r_picmip" | "r_picmip2" | "r_texturebits" | "mss_khz" => LATCH,
        _ => 0,
    }
}

pub struct Shell {
    /// Keyed by lowercase name; `Cvar::name` keeps the registered spelling.
    cvars: BTreeMap<String, Cvar>,
    /// Keyed by canonical key name ([`keys::canonical`]).
    binds: BTreeMap<String, String>,
    /// Cvars set for this run only ([`Shell::set_for_run`]), keyed like
    /// `cvars`, with the value the config keeps for them.
    run_only: BTreeMap<String, String>,
    /// CoD2's enum dvars' domains ([`Shell::set_enum_domain`]), keyed like
    /// `cvars`.
    enums: BTreeMap<String, Vec<String>>,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

/// CoD2's `sc_*` dvars and their defaults.
pub const COOKIE_DVARS: [(&str, &str); 13] = [
    ("sc_enable", "1"),
    ("sc_blur", "2"),
    ("sc_count", "24"),
    ("sc_debugCasterCount", "24"),
    ("sc_debugReceiverCount", "24"),
    ("sc_wantCount", "12"),
    ("sc_wantCountMargin", "1"),
    ("sc_fadeRange", "0.25"),
    ("sc_shadowInRate", "2"),
    ("sc_shadowOutRate", "5"),
    ("sc_length", "400"),
    ("sc_offscreenCasterLodBias", "0"),
    ("sc_offscreenCasterLodScale", "20"),
];

impl Shell {
    /// The client's own cvars at their retail defaults, and [`DEFAULT_BINDS`].
    pub fn new() -> Shell {
        let mut s = Shell {
            cvars: BTreeMap::new(),
            binds: BTreeMap::new(),
            run_only: BTreeMap::new(),
            enums: BTreeMap::new(),
        };
        // CL_Init (CoDMP.exe 0x41236b; 1.5 and UO alike): "Unknown Soldier",
        // archived and userinfo (docs/research/cod11-console.md 5).
        s.register("name", "Unknown Soldier", ARCHIVE | USERINFO);
        // Com_Init (CoDMP.exe 0x437636): "0", CVAR_SYSTEMINFO | CVAR_ROM; only
        // a systeminfo moves it, and `Cvar_Set2`'s cheat check reads it.
        s.register("sv_cheats", "0", ROM);
        // CoDMP.exe registers both with these defaults
        // (docs/research/cod11-console.md, section 4).
        s.register("cl_run", "1", ARCHIVE);
        s.register("scr_conspeed", "3", 0);
        // CL_Init and cgame's cvar table (section 5).
        s.register("sensitivity", "5", ARCHIVE);
        s.register("m_yaw", "0.022", ARCHIVE);
        s.register("m_pitch", "0.022", ARCHIVE);
        // CL_Init: "0.25", archived; `cl_freelook` 0 and `+strafe` move
        // through them (section 5).
        s.register("m_forward", "0.25", ARCHIVE);
        s.register("m_side", "0.25", ARCHIVE);
        // CL_Init (0x41201f..0x412128): the turn keys' rates, archived but
        // `cl_anglespeedkey`, and `cl_mouseAccel` (section 5).
        s.register("cl_yawspeed", "140", ARCHIVE);
        s.register("cl_pitchspeed", "140", ARCHIVE);
        s.register("cl_anglespeedkey", "1.5", 0);
        s.register("cl_mouseAccel", "0", ARCHIVE);
        // Retail's first-run rate is 5000, which starves snapshots on a busy server.
        s.register("rate", "25000", ARCHIVE | USERINFO);
        s.register("snaps", "20", ARCHIVE | USERINFO);
        // CL_Init (CoDMP.exe 1.5 0x413380): default "1", flags 3. Only a 1.5
        // or UO connect sends it (cod15-protocol-diff.md 14).
        s.register("cl_wwwDownload", "1", ARCHIVE | USERINFO);
        // CoD2's and CoD4's voice chat: CoD2 `CL_InitOnceForAllClients`
        // (Mac 0x1485e2) registers it "1", flags 0x1003 (cod2-voice.md 4).
        s.register("cl_voice", "1", ARCHIVE | USERINFO);
        s.register("cg_fov", "80", ARCHIVE | CHEAT);
        // cgame's cvar table row at 0x30075420: default "0.2", CVAR_CHEAT,
        // read by the body's yaw swings (docs/research/cod11-combat.md 16.4).
        s.register("bg_swingSpeed", "0.2", CHEAT);
        // cgame's row at 0x300749b0: default "1", archived and CVAR_CHEAT.
        // 0 hides the view weapon, 2 keeps it under a scope overlay
        // (docs/research/cod11-combat.md 17.4).
        s.register("cg_drawGun", "1", ARCHIVE | CHEAT);
        // cgame's row at 0x30074c10: default "0", archived; gates the
        // weapon-select binds (docs/research/cod11-hud-protocol.md).
        s.register("cg_weaponCycleDelay", "0", ARCHIVE);
        // cgame's rows at 0x300750c0 and 0x300750d0 (1.5 0x300770b0, UO
        // 0x30085950): "70" and "60", archived. The game and bold message
        // windows wrap at them (docs/research/cod11-chat.md 3.3).
        s.register("cg_gameMessageWidth", "70", ARCHIVE);
        s.register("cg_gameBoldMessageWidth", "60", ARCHIVE);
        // CL_Init (CoDMP.exe 0x412318) registers `cg_teamChatsOnly` "0";
        // cgame's rows at 0x30074ed4 and 0x30074ec4 (1.5 0x30076e94 and
        // 0x30076e84, UO 0x30085734 and 0x30085724) `cg_chatHeight` "8" and
        // `cg_chatTime` "12000"; all archived (docs/research/cod11-chat.md 3.2).
        s.register("cg_teamChatsOnly", "0", ARCHIVE);
        s.register("cg_chatHeight", "8", ARCHIVE);
        s.register("cg_chatTime", "12000", ARCHIVE);
        // cgame's row at 0x30074a20: default "0", archived and CVAR_CHEAT.
        // 1 drops a blob under each player (crate::blob_shadow).
        s.register("cg_shadows", "0", ARCHIVE | CHEAT);
        // CoDMP.exe 0x4120e6 and cgame's row at file 0x74f60: default "0",
        // CVAR_TEMP. Non-zero keeps the forced-stance events off `cl_stance`
        // (docs/research/cod11-mantle.md, "The forced-stance events").
        s.register("cl_stanceTemp", "0", 0);
        // CL_Init (CoDMP.exe 0x411e60): the send rate and the clock's
        // nudge (docs/protocol-1.1.md, "The client's clock").
        s.register("cl_maxpackets", "30", ARCHIVE);
        s.register("cl_packetdup", "1", ARCHIVE);
        s.register("cl_timeNudge", "0", 0);
        // 1.5's and UO's cgame records on the server's `C` and stops on its
        // `D` while this is 1; their cvar table rows are "0", archived.
        // `cg_showdemoname` 1, archived, names the demo on their
        // `RECORDING` line (cod11-demos.md 6 and 8).
        s.register("cg_autodemo", "0", ARCHIVE);
        s.register("cg_showdemoname", "1", ARCHIVE);
        // 1.5's and UO's cgame shoots a JPEG on the server's `E` while this
        // is 1 (rows at 0x30077024 and 0x300858c4: "0", archived;
        // cod11-demos.md 10.5).
        s.register("cg_autoscreenshot", "0", ARCHIVE);
        // CL_Init (CoDMP.exe 0x411fd8, 0x411f8e): `timedemo` "0" with no
        // flags, `cl_freezeDemo` "0" CVAR_TEMP (cod11-demos.md, "Playback").
        s.register("timedemo", "0", 0);
        s.register("cl_freezeDemo", "0", 0);
        // CL_Init (CoDMP.exe 0x411fe3, 0x411ff8): "0", no flags; CoD2 and
        // CoD4 register them int and bool, 0 (cod11-demos.md 10.6).
        s.register("cl_avidemo", "0", 0);
        s.register("cl_forceavidemo", "0", 0);
        // Com_Init (CoDMP.exe 0x437807) registers "85", archived; vcod
        // defaults to 0 (1 ms floor) so high-refresh vsync isn't held near
        // 90 fps. `Com_Frame` paces every frame by it (cod11-demos.md 5).
        s.register("com_maxfps", "0", ARCHIVE);
        // CoDMP.exe registers `password` CVAR_USERINFO, not archived
        // (0x4123ea); the browser's password popup edits it.
        s.register("password", "", USERINFO);
        // CL_Init (CoDMP.exe 0x411e60; 1.5 and UO alike): "", the password
        // flags 0x100 (CVAR_TEMP), the address none; `rcon` reads both
        // (docs/research/cod11-console.md 7).
        s.register("rconPassword", "", 0);
        s.register("rconAddress", "", 0);
        // CoDMP.exe registers it "0", archived (0x412152); vcod downloads
        // unless told not to. A server's systeminfo value wins either way.
        s.register("cl_allowDownload", "1", ARCHIVE);
        // vcod only, no retail counterpart: the glow on bright pixels
        // (crate::bloom). 0 is the retail look; read every frame.
        s.register("vc_bloom", "0", ARCHIVE);
        s.register("vc_bloomThreshold", "0.8", ARCHIVE);
        // The browser's cvars, archived, with ui_mp_x86.dll's defaults
        // (cvar table at 0x40036c8c..0x40036dfc).
        s.register("ui_netSource", "0", ARCHIVE);
        // The browser's game type filter (vmCvar row at 0x40036cbc).
        s.register("ui_joinGametype", "0", ARCHIVE);
        // vcod's CoD4 browser filter: 1 lists CoD4x servers, which never
        // let 1.7 play (docs/research/cod4-handshake.md 13).
        s.register("ui_browserShowCoD4x", "0", ARCHIVE);
        // The menu list `ui_load` reads, not archived (vmCvar row at
        // 0x40036c9c).
        s.register("ui_menuFiles", "ui_mp/menus.txt", 0);
        // CoDMP.exe 0x4a9a60: "0", archived and latched, so a set waits for
        // `vid_restart`. The UI reads it when it loads a menu file
        // (docs/research/cod11-front-end.md, 1).
        s.register("cl_language", "0", ARCHIVE | LATCH);
        for name in [
            "ui_browserShowFull",
            "ui_browserShowEmpty",
            "ui_browserShowPassword",
            "ui_browserShowNoPassword",
        ] {
            s.register(name, "1", ARCHIVE);
        }
        // Start New Server's: ui_mp_x86.dll's archived `ui_dedicated`,
        // `ui_netGametype` and `ui_netGametypeName` (rows at 0x40036c28,
        // 0x40036cc8, 0x40036cd8), and the server cvars CoDMP.exe registers
        // in a client too (0x459130), at its defaults and flags.
        s.register("ui_dedicated", "0", ARCHIVE);
        s.register("ui_netGametype", "0", ARCHIVE);
        s.register("ui_netGametypeName", "", ARCHIVE);
        s.register("g_gametype", "dm", 0);
        s.register("sv_hostname", "CoDHost", ARCHIVE);
        s.register("sv_maxclients", "20", 0);
        s.register("sv_maxRate", "0", ARCHIVE);
        s.register("sv_minPing", "0", ARCHIVE);
        s.register("sv_maxPing", "0", ARCHIVE);
        s.register("sv_pure", "1", 0);
        // vcod's own, not in retail: 1 filters lightmaps B-spline bicubic
        // instead of bilinear (docs/research/cod11-gamma.md, section 7).
        s.register("vc_bicubic", "0", ARCHIVE);
        // vcod-only, beyond retail (README, client): screen-space ambient
        // occlusion, its strength; 0 is retail's look (crate::ssao).
        s.register("vc_ssao", "0", ARCHIVE);
        // vcod-only, beyond retail: effect sprites fade into the opaque
        // scene behind them, the value the fade's scale (crate::soft_particles)
        s.register("vc_softParticles", "0", ARCHIVE);
        // `r_vc_depthprepass` registers after the config
        // (`Shell::register_depth_prepass`), its default per game
        // vcod-only debug switches for the profile's sweep, the normal path
        // at 1 (cod4-client-perf.md 25): 0 drops the CoD4 sets' normal map
        // reads, or the level's film grade
        s.register("r_vc_normalMaps", "1", 0);
        s.register("r_vc_film", "1", 0);
        for (name, value) in MENU_CVARS {
            s.register(name, value, ARCHIVE | menu_cvar_latch(name));
        }
        // R_Register (CoDMP.exe 0x4b3300): the filter the mip chain and the
        // picmip halving use, latched (texture_settings::MipFilter)
        s.register("r_simpleMipMaps", "1", ARCHIVE | LATCH);
        s.register("r_weightMipMaps", "0", LATCH);
        // vcod-only, beyond retail; 0 is the retail look
        s.register("vc_shadows", "0", ARCHIVE);
        // CoD4 1.7's sun shadow map (crate::cod4_sunshadow): on by default
        // (0x62c08c), and its near texel size, 0.25 in 0.0625..32 (0x62c390;
        // docs/research/cod4-world.md 3.6)
        s.register("sm_enable", "1", ARCHIVE);
        s.register("sm_sunSampleSizeNear", "0.25", ARCHIVE | CHEAT);
        // the far partition's share of its square, 1 in 0.25..1, and a
        // fixed centre for it, 0 0 0 for none (0x62c3a3-0x62c438)
        s.register("sm_sunShadowScale", "1", ARCHIVE | CHEAT);
        s.register("sm_sunShadowCenter", "0 0 0", CHEAT);
        // the spot light's map (crate::cod4_spotshadow, cod4-world.md 3.5):
        // r_spotLightShadows and kin (0x62b4bb-0x62b547, default 1; every
        // one read),
        // sm_spotEnable (0x62c11d), sm_spotShadowFadeTime 1 in 0.01..5,
        // the light score's 64 and 0.125, the offset's 0.5 and 2
        // (0x62c15a-0x62c32c)
        s.register("r_spotLightShadows", "1", CHEAT);
        s.register("r_spotLightSModelShadows", "1", CHEAT);
        s.register("r_spotLightEntityShadows", "1", CHEAT);
        s.register("sm_spotEnable", "1", 0);
        s.register("sm_spotShadowFadeTime", "1", 0);
        // shadowed primary lights at once, 4 in 0..4 (0x62c15a)
        s.register("sm_maxLights", "4", 0);
        s.register("sm_lightScore_eyeProjectDist", "64", 0);
        s.register("sm_lightScore_spotProjectFrac", "0.125", 0);
        s.register("sm_polygonOffsetBias", "0.5", 0);
        s.register("sm_polygonOffsetScale", "2", 0);
        // vcod-only, beyond retail (crate::post): a float world target and
        // tonemap, latched to `vid_restart` like retail's r_ cvars; then a
        // film grade, vignette and grain, strengths read every frame.
        s.register("vc_hdr", "0", ARCHIVE | LATCH);
        s.register("vc_grade", "0", ARCHIVE);
        s.register("vc_vignette", "0", ARCHIVE);
        s.register("vc_grain", "0", ARCHIVE);
        // vcod-only, beyond retail: light shafts from the sun's screen
        // position, their strength; 0 is the retail look (crate::sun_shafts)
        s.register("vc_sunShafts", "0", ARCHIVE);
        // CoDMP.exe 0x4b386b: default "1", CVAR_CHEAT. 0 drops the flare
        // list, the sun's flare and washes with it (0x4d9770,
        // docs/research/cod11-shader-scripts.md section 8, "r_flares").
        s.register("r_flares", "1", CHEAT);
        // iw3mp.exe 0x49943a (`FX_RegisterDvars`): CoD4's "Toggles drawing
        // of effects after processing", cheat-protected; 0 still runs the
        // effects but draws none of their sprites (cod4-client-perf.md 15).
        s.register("fx_draw", "1", CHEAT);
        // CoDMP.exe 0x4b3965: default "1", CVAR_ARCHIVE; the sky stage
        // (0x4e84d0) draws the disc and adds the flare only while it is set.
        s.register("r_drawSun", "1", ARCHIVE);
        // vcod-only: 1 widens the view on a window wider than 4:3 (Hor+,
        // every fov a 4:3 view's width) and keeps the HUD's side pieces at
        // the window's edges; 0 is retail's fixed horizontal fov with the
        // whole HUD in the 4:3 box (camera::view_fov_x, hudelem::Virtual).
        s.register("vc_widescreen", "1", ARCHIVE);
        // vcod-only, CoD4: 1 projects the view at the window's own aspect
        // (Hor+ off `cg_fov`'s 4:3 view, as `CG_UpdateFov` widens one) and
        // keeps the HUD square in a centred box of `r_aspectRatio`'s
        // aspect; 0 is 1.7's picked aspect stretched over the window
        // (camera::Cod4Screen, cod4-front-end.md 10).
        s.register("vc_windowAspect", "0", ARCHIVE);
        // R_Register (CoDMP.exe 0x4b374b, 0x4b3760): the size `r_mode -1`
        // asks for, and its pixel aspect, which vcod ignores. Retail's are
        // 1600 and 1024; vcod's 0 keeps its own window size (crate::video).
        s.register("r_customwidth", "0", ARCHIVE);
        s.register("r_customheight", "0", ARCHIVE);
        s.register("r_customaspect", "1", ARCHIVE);
        // vcod-only: 1 under `--vanilla`, the stock paks alone on the
        // search path (Pk3Fs::vanilla).
        s.register("vc_vanilla", "0", ROM);
        // CoDMP.exe 0x42cdb0: "", CVAR_SYSTEMINFO | CVAR_INIT. The player's
        // mod and a server's systeminfo both force it; a disconnect leaves
        // it (docs/research/cod11-front-end.md, section 16).
        s.register("fs_game", "", INIT);
        // vcod-only: the build a saved config was written by
        // (`Shell::upgrade_config`); a config without it reads 0.
        s.register("vc_cfgVersion", &CONFIG_VERSION.to_string(), ARCHIVE);
        for (key, cmd) in DEFAULT_BINDS {
            s.binds.insert(key.to_string(), cmd.to_string());
        }
        s
    }

    fn register(&mut self, name: &str, value: &str, flags: u32) {
        self.cvars.insert(
            name.to_ascii_lowercase(),
            Cvar {
                name: name.to_string(),
                value: value.to_string(),
                default: value.to_string(),
                archive: flags & ARCHIVE != 0,
                userinfo: flags & USERINFO != 0,
                cheat: flags & CHEAT != 0,
                latch: flags & LATCH != 0,
                rom: flags & ROM != 0,
                init: flags & INIT != 0,
                latched: None,
            },
        );
    }

    /// `CL_SystemInfoChanged` (CoDMP.exe 0x415eb0; 1.5 0x4176f0, UO
    /// 0x418b80): a systeminfo `sv_cheats` of 0 (or none) puts every cheat
    /// cvar back to its default (`Cvar_SetCheatState`, 0x439bd0), then every
    /// key/value pair is set as a cvar with `force`, whatever its flags. A
    /// demo sets nothing; the caller skips it.
    pub fn apply_systeminfo(&mut self, info: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        if atoi_msvc(net::info_value_for_key(info, "sv_cheats").unwrap_or("")) == 0 {
            for c in self.cvars.values_mut().filter(|c| c.cheat) {
                if c.value != c.default {
                    c.value.clone_from(&c.default);
                    if c.archive {
                        out.push(Effect::SaveConfig);
                    }
                }
            }
        }
        for (key, value) in net::info_pairs(info) {
            self.set_forced(key, value, &mut out);
        }
        let mut once = Vec::new();
        for e in out {
            if !once.contains(&e) {
                once.push(e);
            }
        }
        once
    }

    /// `Cvar_Set2` with `force` (CoDMP.exe 0x439650, `eax` 1): no read-only,
    /// write-protect, cheat or latch check; a pending latched value is
    /// dropped. A cvar the client never registered is made with no flags.
    fn set_forced(&mut self, name: &str, value: &str, out: &mut Vec<Effect>) {
        let key = name.to_ascii_lowercase();
        self.run_only.remove(&key);
        let c = self.cvars.entry(key).or_insert_with(|| Cvar {
            name: name.to_string(),
            value: value.to_string(),
            default: value.to_string(),
            archive: false,
            userinfo: false,
            cheat: false,
            latch: false,
            rom: false,
            init: false,
            latched: None,
        });
        c.latched = None;
        if c.value == value {
            return;
        }
        c.value = value.to_string();
        if c.userinfo {
            out.push(Effect::Userinfo);
        }
        if c.archive {
            out.push(Effect::SaveConfig);
        }
    }

    /// `Cvar_Set` (`Cvar_Set2` with `force`), as the UI's `RunMod` sets
    /// `fs_game`. Returns what the set asks of the caller.
    pub fn force(&mut self, name: &str, value: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        self.set_forced(name, value, &mut out);
        out
    }

    /// `sv_cheats` as `Cvar_Set2` reads it: its integer value.
    fn cheats(&self) -> bool {
        atoi_msvc(self.cvar("sv_cheats").unwrap_or("")) != 0
    }

    /// The userinfo the cvars make.
    pub fn userinfo(&self) -> Userinfo {
        let get = |n: &str| self.cvar(n).unwrap_or_default().to_string();
        Userinfo {
            name: get("name"),
            rate: get("rate"),
            snaps: get("snaps"),
            password: get("password"),
            www_download: get("cl_wwwDownload"),
        }
    }

    pub fn cvar(&self, name: &str) -> Option<&str> {
        self.cvars
            .get(&name.to_ascii_lowercase())
            .map(|c| c.value.as_str())
    }

    /// Sets a cvar for this run without touching what the config file keeps
    /// for it, until something sets it again (a command-line override).
    pub fn set_for_run(&mut self, name: &str, value: &str) {
        let key = name.to_ascii_lowercase();
        let kept = self.cvars.get(&key).map(|c| c.value.clone());
        self.set(name, value, false, &mut Vec::new());
        // A command-line value is the start-up value: a latched cvar takes
        // it now rather than at the next restart.
        if let Some(c) = self.cvars.get_mut(&key)
            && let Some(v) = c.latched.take()
        {
            c.value = v;
        }
        if let Some(kept) = kept {
            self.run_only.insert(key, kept);
        }
    }

    /// Sets a read-only cvar's value, which no console or config set moves.
    pub fn set_rom(&mut self, name: &str, value: &str) {
        if let Some(c) = self.cvars.get_mut(&name.to_ascii_lowercase()) {
            c.value = value.to_string();
        }
    }

    /// Q3's `atof`: a non-number reads 0.
    pub fn cvar_f32(&self, name: &str) -> f32 {
        self.cvar(name)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0.0)
    }

    #[cfg(test)]
    pub fn bind(&self, key: &str) -> Option<&str> {
        self.binds.get(key).map(String::as_str)
    }

    /// Binds `key` to `cmd` when no key holds `cmd` yet and `key` holds
    /// nothing. Returns whether it bound.
    pub fn bind_if_free(&mut self, key: &str, cmd: &str) -> bool {
        if !self.keys_bound_to(cmd).is_empty() || self.binds.contains_key(key) {
            return false;
        }
        self.binds.insert(key.to_string(), cmd.to_string());
        true
    }

    /// CoD2's archived dvars the stock options menus set where vcod's CoD1
    /// set has none: `snd_volume` (`SND_Init`, Mac 0x4b496: 1.0, archived),
    /// `cg_drawMantleHint` (`CG_Init`, Mac 0x145214: 1, flags 0x1001),
    /// `cg_drawCrosshairNames` (1, 0x1001), its unarchived position
    /// `cg_drawCrosshairNamesPosX` 300 and `PosY` 180 (`CG_RegisterDvars`),
    /// and the HUD's below. `cg_drawGun` drops its CoD1 archive flag.
    /// Registered after the config ran, keeping a value it set. CoD1's
    /// `timedemo` cvar goes: CoD2's `timedemo` is a command.
    pub fn use_cod2_cvars(&mut self) {
        // `timedemo` is a command there (`CL_Init`, Mac 0x14b032).
        self.cvars.remove("timedemo");
        // `CG_Init` (Mac 0x145214): a bool, 1, flags 0x1080, cheat and not
        // archived (cod2-efx.md 6.8).
        self.register("cg_drawGun", "1", CHEAT);
        // `CG_Init` (Mac 0x145214), all archived (flags 0x1001): the chat's
        // and say field's places, the stance hint prints and the hints'
        // fade (cod2-hud.md 9, 14).
        for (name, default, _) in WINVOICE_DVARS {
            let kept = self.cvar(name).map(str::to_string);
            self.register(name, default, ARCHIVE);
            if let (Some(v), Some(c)) = (kept, self.cvars.get_mut(name)) {
                c.value = v;
            }
        }
        for (name, default) in [
            ("snd_volume", "1"),
            ("cg_drawMantleHint", "1"),
            ("cg_drawCrosshairNames", "1"),
            ("cg_hudChatPosition", "5 150"),
            ("cg_hudSayPosition", "5 126"),
            ("cg_hudStanceHintPrints", "0"),
            ("cg_hintFadeTime", "100"),
            ("cg_drawLagometer", "0"),
        ] {
            let kept = self.cvar(name).map(str::to_string);
            self.register(name, default, ARCHIVE);
            if let (Some(v), Some(c)) = (kept, self.cvars.get_mut(&name.to_ascii_lowercase())) {
                c.value = v;
            }
        }
        // The player profile dvars (`Com_InitPlayerProfiles`, Mac 0x111aa6;
        // `UI_RegisterDvars`, Mac 0x1505b0), none archived; the main menu's
        // `openMenuOnDvar` asks for the first two.
        for (name, default) in [
            ("com_playerProfile", ""),
            ("ui_playerProfileCount", "0"),
            ("ui_playerProfileSelected", ""),
            ("ui_playerProfileNameNew", ""),
            ("ui_playerProfileAlreadyChosen", "0"),
        ] {
            if self.cvar(name).is_none() {
                self.register(name, default, 0);
            }
        }
        // The shadow cookies' dvars at the renderer's defaults, none
        // archived (`R_RegisterDvars`, Mac 0x000cb8b6; cod2-materials.md
        // 9.8); `sc_count` is registered and read by nothing.
        for (name, default) in [
            ("cg_drawCrosshairNamesPosX", "300"),
            ("cg_drawCrosshairNamesPosY", "180"),
        ] {
            if self.cvar(name).is_none() {
                self.register(name, default, 0);
            }
        }
        for (name, default) in COOKIE_DVARS {
            if self.cvar(name).is_none() {
                self.register(name, default, 0);
            }
        }
    }

    /// Makes `name` an enum dvar over `entries` (CoD2's `Dvar_RegisterEnum`):
    /// a set of an index in range or of an entry's name in any case stores
    /// the entry ([`Self::set`]). A value already held that names an entry
    /// is spelled as the entry; any other is left, as vcod's `r_mode` -1
    /// (its own window size) is.
    pub fn set_enum_domain(&mut self, name: &str, entries: Vec<String>) {
        let key = name.to_ascii_lowercase();
        if let Some(c) = self.cvars.get_mut(&key)
            && let Some(e) = enum_entry(&entries, &c.value)
        {
            c.value = e.to_string();
        }
        self.enums.insert(key, entries);
    }

    /// Every enum dvar's entries, by lowercased name.
    pub fn enum_domains(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.enums.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// An enum dvar's entries, `None` for any other cvar.
    pub fn enum_domain(&self, name: &str) -> Option<&[String]> {
        self.enums
            .get(&name.to_ascii_lowercase())
            .map(Vec::as_slice)
    }

    /// `Dvar_GetInt` (Mac 0x51dca): an enum dvar's index, any other's
    /// integer (`atoi`); a value no entry names (vcod's `r_mode` -1) reads
    /// as its integer.
    pub fn int(&self, name: &str) -> i32 {
        let v = self.cvar(name).unwrap_or("");
        self.enum_domain(name)
            .and_then(|es| {
                let e = enum_entry(es, v)?;
                es.iter().position(|x| x == e)
            })
            .map_or_else(|| atoi_msvc(v), |i| i as i32)
    }

    /// `r_aaSamples`, the scene's MSAA (`Renderer::new`: 1 or 4), latched
    /// and registered after the config ran, keeping a value it set. The
    /// default is retail's 1 in every game: CoD1's pixel format asks for no
    /// sample buffers and its one multisample cvar, `r_ati_fsaa_samples`
    /// (CoDMP.exe 0x4b33e4, "1"), is never read (cod11-client-perf.md 4);
    /// CoD2's `R_RegisterDvars` (Mac 0xc8f98: 1 in 1..4, flags 0x2021) and
    /// CoD4's (iw3mp.exe 0x62b1cb: 1 in 1..16, cod4-front-end.md 9.1).
    pub fn register_aa_samples(&mut self) {
        self.register_kept("r_aaSamples", "1", ARCHIVE | LATCH);
    }

    /// `r_vc_depthprepass`, vcod's depth-only prepass under the opaque
    /// surfaces (the same pixels; cod4-client-perf.md 12), registered
    /// after the config ran at the game's default, not archived: a config's
    /// line for it is dropped, as the opt-in builds wrote a 0 that pinned
    /// it off (cod2-client-perf.md 12). On for CoD2 and CoD4, whose retail
    /// renderers lay the world's depth first; off for CoD1 and UO, whose
    /// retail renderer runs no prepass and whose prepass takes only the
    /// entities' first stages, which cost more than they save
    /// (cod11-client-perf.md 4).
    pub fn register_depth_prepass(&mut self, on: bool) {
        self.register("r_vc_depthprepass", if on { "1" } else { "0" }, 0);
    }

    /// Registers `name` at `value`, keeping the value a config already set.
    fn register_kept(&mut self, name: &str, value: &str, flags: u32) {
        let kept = self.cvar(name).map(str::to_string);
        self.register(name, value, flags);
        if let (Some(v), Some(c)) = (kept, self.cvars.get_mut(&name.to_ascii_lowercase())) {
            c.value = v;
        }
    }

    /// 1.7's Options dvars vcod reads on a CoD4 install
    /// (cod4-front-end.md 9), registered after the config ran, keeping a
    /// value it set: `r_glow_allowed`, `r_glow`, `r_zFeather`, `r_distortion`,
    /// `r_dof_enable`,
    /// `r_specular`, `r_fog`, `r_dlightLimit`,
    /// `r_lodScaleRigid`, `r_lodBiasRigid`, `r_drawWater`,
    /// `r_texFilterMipMode`, `monkeytoy`, the picmip and anisotropy dvars,
    /// `r_aspectRatio`, the third-person dvars, the scoreboard's ping graph
    /// dvars and the custom class names at iw3mp.exe's defaults; then
    /// [`Shell::cod4_set_picmip`].
    pub fn register_cod4_options(&mut self) {
        // `timedemo` is a command beside `demo` on 1.7 too (`CL_Init`,
        // `iw3mp.exe` 0x46f541).
        self.cvars.remove("timedemo");
        for (name, value, flags) in COD4_OPTION_DVARS {
            let kept = self.cvar(name).map(str::to_string);
            self.register(name, value, flags);
            if let (Some(v), Some(c)) = (kept, self.cvars.get_mut(&name.to_ascii_lowercase())) {
                c.value = v;
            }
        }
        self.set_enum_domain(
            "r_aspectRatio",
            COD4_ASPECT_RATIOS.iter().map(|s| s.to_string()).collect(),
        );
        self.cod4_set_picmip();
    }

    /// `R_SetPicmip` (iw3mp.exe 0x617520) on a CoD4 install: with
    /// `r_picmip_manual` 0 the renderer picks the levels off the card's
    /// and the system's memory and writes them back to the three dvars.
    /// vcod reads every machine as 1.7's top tier (450 MB of texture
    /// memory, over 640 MB of RAM), whose levels are all 0.
    pub fn cod4_set_picmip(&mut self) {
        if self
            .cvar("r_picmip_manual")
            .is_none_or(|m| atoi_msvc(m) != 0)
        {
            return;
        }
        for name in ["r_picmip", "r_picmip_bump", "r_picmip_spec"] {
            if let Some(c) = self.cvars.get_mut(name) {
                c.value = "0".into();
            }
        }
    }

    /// [`COD2_BINDS`] on every key unbound or still at its
    /// [`DEFAULT_BINDS`] command, for a CoD2 install. Returns how many.
    pub fn use_cod2_binds(&mut self) -> usize {
        let mut n = 0;
        for (key, cmd) in COD2_BINDS {
            let cod1 = DEFAULT_BINDS.iter().find(|(k, _)| k == key).map(|b| b.1);
            if self
                .binds
                .get(*key)
                .is_none_or(|b| Some(b.as_str()) == cod1)
            {
                self.binds.insert(key.to_string(), cmd.to_string());
                n += 1;
            }
        }
        n
    }

    /// [`COD4_BINDS`] on every key unbound or still at its [`DEFAULT_BINDS`]
    /// or [`COD2_BINDS`] command, for a CoD4 install, after
    /// [`Self::use_cod2_binds`]. Returns how many.
    pub fn use_cod4_binds(&mut self) -> usize {
        let mut n = 0;
        for (key, cmd) in COD4_BINDS {
            fn stock(t: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
                t.iter().find(|(k, _)| *k == key).map(|b| b.1)
            }
            let (cod1, cod2) = (stock(DEFAULT_BINDS, key), stock(COD2_BINDS, key));
            if self
                .binds
                .get(*key)
                .is_none_or(|b| Some(b.as_str()) == cod1 || Some(b.as_str()) == cod2)
            {
                self.binds.insert(key.to_string(), cmd.to_string());
                n += 1;
            }
        }
        n
    }

    /// A CoD4 start's stock keys ([`Self::use_cod2_binds`], then
    /// [`Self::use_cod4_binds`]), only when no config was read: a saved
    /// config holds every bind after its `unbindall`, so a key it leaves
    /// free was cleared in the bind menu and stays free. Returns how many.
    pub fn use_cod4_defaults(&mut self, config_read: bool) -> usize {
        if config_read {
            return 0;
        }
        self.use_cod2_binds() + self.use_cod4_binds()
    }

    /// Every cvar's value by its lowercased name.
    pub fn cvar_values(&self) -> impl Iterator<Item = (&str, &str)> {
        self.cvars
            .iter()
            .map(|(k, c)| (k.as_str(), c.value.as_str()))
    }

    /// Every bind, key and command.
    pub fn binds(&self) -> impl Iterator<Item = (&str, &str)> {
        self.binds.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Every key bound to exactly `cmd` (case folded), in retail's key-number
    /// order, which is the order the options screens list them in.
    pub fn keys_bound_to(&self, cmd: &str) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .binds
            .iter()
            .filter(|(_, v)| v.eq_ignore_ascii_case(cmd))
            .map(|(k, _)| k.as_str())
            .collect();
        keys.sort_by_key(|k| keys::number(k));
        keys
    }

    /// The cgame's key text for `cmd` (0x30046940): its first key's name,
    /// or the first two joined by `KEY_OR`, names through `KEY_*`. `None`
    /// while nothing is bound to it.
    pub fn key_text(&self, cmd: &str, loc: &Localized) -> Option<String> {
        let name = |k: &str| loc.get(&format!("KEY_{k}")).unwrap_or(k).to_string();
        match self.keys_bound_to(cmd).as_slice() {
            [] => None,
            [a] => Some(name(a)),
            [a, b, ..] => Some(format!(
                "{} {} {}",
                name(a),
                loc.translate("@KEY_OR"),
                name(b)
            )),
        }
    }

    /// Runs `text`: commands split on `;` and newlines outside quotes, as
    /// `Cbuf_Execute` does.
    pub fn execute(&mut self, text: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        for line in split_commands(text) {
            self.execute_one(&line, &mut out);
        }
        out
    }

    /// A bound key's edge. A press runs the bind; a release runs `-cmd` for a
    /// `+cmd` bind and nothing otherwise (CoDMP.exe 0x40dc30).
    pub fn key_event(&mut self, key: &str, down: bool) -> Vec<Effect> {
        let Some(cmd) = self.binds.get(key).cloned() else {
            return Vec::new();
        };
        if down {
            self.execute(&cmd)
        } else if let Some(rest) = cmd.strip_prefix('+') {
            self.execute(&format!("-{rest}"))
        } else {
            Vec::new()
        }
    }

    /// The bind of `key` if it holds a [`Action`] button or impulse, which is
    /// what the mouse grab gates (`main.rs`).
    pub fn bind_is_gameplay(&self, key: &str) -> bool {
        let Some(cmd) = self.binds.get(key) else {
            return false;
        };
        let tokens = tokenize(cmd);
        match tokens.first().map(String::as_str) {
            Some(w) if w.eq_ignore_ascii_case("+actionslot") => true,
            Some(w) if w.starts_with('+') => button(&w[1..]).is_some(),
            Some(
                "gocrouch" | "goprone" | "togglecrouch" | "toggleprone" | "lowerstance"
                | "raisestance" | "weaponslot" | "weapnext" | "weapprev" | "weapalt" | "weapon"
                | "toggleads" | "leaveads",
            ) => true,
            _ => false,
        }
    }

    fn execute_one(&mut self, line: &str, out: &mut Vec<Effect>) {
        let tokens = tokenize(line);
        let Some(word) = tokens.first() else { return };
        let word = word.to_ascii_lowercase();
        let arg = |i: usize| tokens.get(i).map(String::as_str);
        let print = |out: &mut Vec<Effect>, s: String| out.push(Effect::Print(s));

        if let Some(rest) = word.strip_prefix('+')
            && let Some(action) = button(rest)
        {
            out.push(Effect::Button(action, true));
            return;
        }
        if let Some(rest) = word.strip_prefix('-')
            && let Some(action) = button(rest)
        {
            out.push(Effect::Button(action, false));
            return;
        }

        match word.as_str() {
            // `+actionslot` (`iw3mp.exe` 0x457960) acts on the press, `-actionslot` does
            // nothing a cmd carries; a slot outside 1..4 does nothing.
            "+actionslot" => match arg(1).map(vcod_common::text::atoi) {
                Some(n @ 1..=4) => out.push(Effect::Impulse(Action::UseSlot(n as usize))),
                Some(n) => print(
                    out,
                    format!(
                        "+/-actionslot; number given is out of range.  Was {n}, expected 1 thru 4."
                    ),
                ),
                None => print(out, "USAGE: +/-actionslot <number>".into()),
            },
            "-actionslot" => {}
            "+scores" => out.push(Effect::Scores(true)),
            "-scores" => out.push(Effect::Scores(false)),
            "gocrouch" => out.push(Effect::Impulse(Action::Crouch)),
            "goprone" => out.push(Effect::Impulse(Action::Prone)),
            "togglecrouch" => out.push(Effect::Impulse(Action::ToggleCrouch)),
            "toggleprone" => out.push(Effect::Impulse(Action::ToggleProne)),
            "lowerstance" => out.push(Effect::Impulse(Action::LowerStance)),
            "raisestance" => out.push(Effect::Impulse(Action::RaiseStance)),
            "weapnext" => out.push(Effect::Impulse(Action::NextWeapon)),
            "weapprev" => out.push(Effect::Impulse(Action::PrevWeapon)),
            "weapalt" => out.push(Effect::Impulse(Action::AltWeapon)),
            "toggleads" => out.push(Effect::Impulse(Action::ToggleAds)),
            "leaveads" => out.push(Effect::Impulse(Action::LeaveAds)),
            "weapon" => out.push(Effect::Weapon(arg(1).unwrap_or("").to_string())),
            // Retail stamps the weapon name before it looks the slot up and
            // prints nothing for a bad one (0x30038342).
            "weaponslot" => out.push(Effect::Impulse(Action::Slot(weapon_slot(
                arg(1).unwrap_or(""),
            )))),
            "openscriptmenu" => match (arg(1), arg(2), tokens.len()) {
                (Some(menu), Some(response), 3) => out.push(Effect::OpenScriptMenu {
                    menu: menu.to_string(),
                    response: response.to_string(),
                }),
                _ => print(
                    out,
                    "USAGE: openscriptmenu <parent menu name> <script menu response>".into(),
                ),
            },
            "messagemode" => out.push(Effect::MessageMode { team: false }),
            "messagemode2" => out.push(Effect::MessageMode { team: true }),
            // CoD4's chat field (`Con_ChatModePublic_f`,
            // `Con_ChatModeTeam_f`): `messagemode`'s and `messagemode2`'s.
            "chatmodepublic" => out.push(Effect::MessageMode { team: false }),
            "chatmodeteam" => out.push(Effect::MessageMode { team: true }),
            "toggleconsole" => out.push(Effect::ToggleConsole),
            "clear" => out.push(Effect::Clear),
            "echo" => print(out, args_from(line, 1)),
            "version" => print(out, concat!("vcod ", env!("VCOD_VERSION")).into()),
            "quit" => out.push(Effect::Quit),
            "disconnect" => out.push(Effect::Disconnect),
            "reconnect" => out.push(Effect::Reconnect),
            "connect" => match arg(1) {
                Some(addr) => out.push(Effect::Connect(addr.to_string())),
                None => print(out, "usage: connect [server]".into()),
            },
            // CoDMP.exe 0x40e540, 0x40e430 and 0x40eb40, their usage lines.
            "record" if tokens.len() > 2 => print(out, "record <demoname>".into()),
            "record" => out.push(Effect::Record(arg(1).map(str::to_string))),
            "stoprecord" => out.push(Effect::StopRecord),
            "demo" => match (arg(1), tokens.len()) {
                (Some(name), 2) => out.push(Effect::Demo {
                    name: name.to_string(),
                    timedemo: false,
                }),
                _ => print(out, "playdemo <demoname>".into()),
            },
            // CoD2's and CoD4's `timedemo` is a command beside `demo` (Mac
            // 0x14c288, `iw3mp.exe` 0x46f541);
            // its install has no `timedemo` cvar ([`Self::use_cod2_cvars`]).
            "timedemo" if !self.cvars.contains_key("timedemo") => match (arg(1), tokens.len()) {
                (Some(name), 2) => out.push(Effect::Demo {
                    name: name.to_string(),
                    timedemo: true,
                }),
                _ => print(out, "timedemo <demoname>".into()),
            },
            "rcon" => out.push(Effect::Rcon(tokens[1..].to_vec())),
            "say" | "say_team" => out.push(Effect::Forward(line.trim().to_string())),
            "cmd" => out.push(Effect::Forward(args_from(line, 1))),
            "bind" => self.cmd_bind(&tokens, line, out),
            "unbind" => match arg(1) {
                None => print(out, "unbind <key> : remove commands from a key".into()),
                Some(k) => match keys::canonical(k) {
                    None => print(out, format!("\"{k}\" isn't a valid key")),
                    Some(k) => {
                        self.binds.remove(&k);
                        out.push(Effect::SaveConfig);
                    }
                },
            },
            "unbindall" => {
                self.binds.clear();
                out.push(Effect::SaveConfig);
            }
            "bindlist" => {
                for (k, v) in &self.binds {
                    print(out, format!("{k} \"{v}\""));
                }
            }
            "set" | "seta" => {
                let (Some(name), Some(_)) = (arg(1), arg(2)) else {
                    print(out, format!("usage: {word} <variable> <value>"));
                    return;
                };
                let value = args_from(line, 2);
                self.set(name, &value, word == "seta", out);
            }
            "toggle" => self.cmd_toggle(&tokens, out),
            // `setfromcvar <variable> <variablein>` (CoDMP.exe 0x43a070): a
            // missing source reads "".
            // CoD2 renames it `setfromdvar`; its options menus `exec` that.
            "setfromcvar" | "setfromdvar" => match (arg(1), arg(2)) {
                (Some(dest), Some(src)) => {
                    let value = self.cvar(src).unwrap_or("").to_string();
                    self.set(dest, &value, false, out);
                }
                _ => print(out, "usage: setfromcvar <variable> <variablein>".into()),
            },
            "exec" => match arg(1) {
                Some(file) => out.push(Effect::Exec(file.to_string())),
                None => print(out, "exec <filename> : execute a script file".into()),
            },
            "vid_restart" => out.push(Effect::VidRestart),
            "snd_restart" => out.push(Effect::SndRestart),
            "r_applypicmip" => out.push(Effect::ApplyPicmip),
            "ui_load" => out.push(Effect::UiLoad),
            "mp_quickmessage" => out.push(Effect::QuickMessage),
            "cg_shellshock" => out.push(Effect::ShellShock(tokens[1..].to_vec())),
            "screenshot" | "screenshotjpeg" => out.push(Effect::Screenshot {
                jpeg: word == "screenshotjpeg",
                args: tokens[1..].to_vec(),
            }),
            "cvarlist" => {
                let prefix = arg(1).map(str::to_ascii_lowercase);
                let mut n = 0;
                for (key, c) in &self.cvars {
                    if prefix
                        .as_ref()
                        .is_some_and(|p| !key.starts_with(p.as_str()))
                    {
                        continue;
                    }
                    let flags = if c.archive { 'A' } else { ' ' };
                    print(out, format!("{flags} {} \"{}\"", c.name, c.value));
                    n += 1;
                }
                print(out, format!("{n} total cvars"));
            }
            "cmdlist" => {
                let prefix = arg(1).map(str::to_ascii_lowercase);
                let mut n = 0;
                for c in COMMANDS {
                    if prefix
                        .as_ref()
                        .is_some_and(|p| !c.to_ascii_lowercase().starts_with(p.as_str()))
                    {
                        continue;
                    }
                    print(out, c.to_string());
                    n += 1;
                }
                print(out, format!("{n} commands"));
            }
            _ => {
                // `Cvar_Command`: a bare cvar name prints it, a value sets it.
                if let Some(c) = self.cvars.get(&word) {
                    match arg(1) {
                        None => print(
                            out,
                            format!(
                                "\"{}\" is:\"{}^7\" default:\"{}^7\"",
                                c.name, c.value, c.default
                            ),
                        ),
                        Some(_) => {
                            let name = c.name.clone();
                            self.set(&name, &args_from(line, 1), false, out);
                        }
                    }
                    return;
                }
                out.push(Effect::Unknown {
                    word: tokens[0].clone(),
                    line: line.trim().to_string(),
                });
            }
        }
    }

    fn cmd_bind(&mut self, tokens: &[String], line: &str, out: &mut Vec<Effect>) {
        let Some(k) = tokens.get(1) else {
            out.push(Effect::Print(
                "bind <key> [command] : attach a command to a key".into(),
            ));
            return;
        };
        let Some(key) = keys::canonical(k) else {
            out.push(Effect::Print(format!("\"{k}\" isn't a valid key")));
            return;
        };
        if tokens.len() == 2 {
            out.push(Effect::Print(match self.binds.get(&key) {
                Some(cmd) => format!("\"{k}\" = \"{cmd}\""),
                None => format!("\"{k}\" is not bound"),
            }));
            return;
        }
        // `bind k "a; b"` keeps the quoted text whole; `bind k +attack` too.
        let cmd = if tokens.len() == 3 {
            tokens[2].clone()
        } else {
            args_from(line, 2)
        };
        self.binds.insert(key, cmd);
        out.push(Effect::SaveConfig);
    }

    /// `toggle <var>` flips 0/1; `toggle <var> a b c` steps through the list.
    fn cmd_toggle(&mut self, tokens: &[String], out: &mut Vec<Effect>) {
        let Some(name) = tokens.get(1) else {
            out.push(Effect::Print(
                "usage: toggle <variable> <optional value sequence>".into(),
            ));
            return;
        };
        let current = self.cvar(name).unwrap_or("").to_string();
        let next = if tokens.len() <= 3 {
            if current.trim().parse::<f32>().unwrap_or(0.0) != 0.0 {
                "0".to_string()
            } else {
                "1".to_string()
            }
        } else {
            let seq = &tokens[2..];
            match seq.iter().position(|v| *v == current) {
                Some(i) => seq[(i + 1) % seq.len()].clone(),
                None => seq[0].clone(),
            }
        };
        self.set(name, &next, false, out);
    }

    /// `set` creates a cvar the client does not know, as retail does;
    /// `seta` marks it archived as well.
    fn set(&mut self, name: &str, value: &str, archive: bool, out: &mut Vec<Effect>) {
        let cheats = self.cheats();
        let key = name.to_ascii_lowercase();
        let value = match self.enums.get(&key).and_then(|e| enum_entry(e, value)) {
            Some(e) => e.to_string(),
            None => value.to_string(),
        };
        let value = value.as_str();
        // An explicit set makes a run-only value the user's own.
        self.run_only.remove(&key);
        let c = self.cvars.entry(key).or_insert_with(|| Cvar {
            name: name.to_string(),
            value: String::new(),
            default: value.to_string(),
            archive: false,
            userinfo: false,
            cheat: false,
            latch: false,
            rom: false,
            init: false,
            latched: None,
        });
        if c.rom {
            out.push(Effect::Print(format!("{} is read only.", c.name)));
            return;
        }
        if c.init {
            out.push(Effect::Print(format!("{} is write protected.", c.name)));
            return;
        }
        if c.cheat && !cheats && c.value != value {
            out.push(Effect::Print(format!("{} is cheat protected.", c.name)));
            return;
        }
        // `Cvar_Set2` (CoDMP.exe 0x4397db): a latched cvar compares against
        // the pending value when there is one, else the live one.
        if c.latch {
            if c.latched.as_deref().unwrap_or(&c.value) == value {
                return;
            }
            out.push(Effect::Print(format!(
                "{} will be changed upon restarting.",
                c.name
            )));
            c.latched = Some(value.to_string());
            if c.archive {
                out.push(Effect::SaveConfig);
            }
            return;
        }
        let changed = c.value != value;
        let newly_archived = archive && !c.archive;
        c.value = value.to_string();
        c.archive |= archive;
        if changed && c.userinfo {
            out.push(Effect::Userinfo);
        }
        if (changed || newly_archived) && c.archive {
            out.push(Effect::SaveConfig);
        }
    }

    /// Command and cvar names starting with `prefix`, case-insensitively,
    /// sorted.
    /// Re-registration takes a latched value (`Cvar_Get`, CoDMP.exe
    /// 0x439489): `vid_restart` re-registers `cl_language` (0x40fd14), and
    /// startup registers it after the config ran. True when a value moved.
    pub fn apply_latched(&mut self) -> bool {
        self.apply_latched_where(|_| true)
    }

    /// `vid_restart`'s latched cvars: all but the sound system's, which
    /// only `snd_restart` re-registers (CoDMP.exe 0x44fe60).
    pub fn apply_latched_video(&mut self) -> bool {
        self.apply_latched_where(|n| !n.starts_with("mss_"))
    }

    /// `snd_restart`'s Miles start-up re-registers the `mss_*` cvars; its
    /// `vid_restart` takes the rest.
    pub fn apply_latched_sound(&mut self) -> bool {
        self.apply_latched_where(|n| n.starts_with("mss_"))
    }

    /// [`Shell::apply_latched`] for the cvars whose lowercase name `pick`
    /// accepts.
    fn apply_latched_where(&mut self, pick: impl Fn(&str) -> bool) -> bool {
        let mut moved = false;
        for (_, c) in self.cvars.iter_mut().filter(|(k, _)| pick(k)) {
            if let Some(v) = c.latched.take() {
                moved |= c.value != v;
                c.value = v;
            }
        }
        moved
    }

    pub fn complete(&self, prefix: &str) -> Vec<String> {
        let p = prefix.to_ascii_lowercase();
        let mut names: Vec<String> = COMMANDS
            .iter()
            .filter(|c| c.to_ascii_lowercase().starts_with(&p))
            .map(|c| c.to_string())
            .chain(
                self.cvars
                    .iter()
                    .filter(|(k, _)| k.starts_with(&p))
                    .map(|(_, c)| c.name.clone()),
            )
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Runs a saved config ([`Self::execute`]). A vcod config without a
    /// `vc_cfgVersion` line reads as stamp 0; any other file (1.7's
    /// `config_mp.cfg`) as this build's, so [`Self::upgrade_config`]
    /// leaves what it set alone.
    pub fn exec_config(&mut self, text: &str) -> Vec<Effect> {
        let vcod = text.starts_with(CONFIG_HEADER);
        self.set_rom("vc_cfgVersion", "0");
        let out = self.execute(text);
        if !vcod {
            self.set_rom("vc_cfgVersion", &CONFIG_VERSION.to_string());
        }
        out
    }

    /// After a config ran and every late cvar registered: each
    /// [`STALE_DEFAULTS`] row past the config's stamp whose cvar still holds
    /// the old default (or has it latched) takes the current default; any
    /// other value stays. Then the stamp is this build's. Returns the
    /// cvars reset, and whether anything moved, the stamp included, so the
    /// caller writes the config.
    pub fn upgrade_config(&mut self, game: Game) -> (Vec<String>, bool) {
        let stamp = self.cvar("vc_cfgVersion").map_or(0, atoi_msvc);
        if stamp >= CONFIG_VERSION as i32 {
            return (Vec::new(), false);
        }
        let mut reset = Vec::new();
        for (version, name, old, games) in STALE_DEFAULTS {
            let Some(c) = self.cvars.get_mut(&name.to_ascii_lowercase()) else {
                continue;
            };
            let held = c.latched.as_ref().unwrap_or(&c.value);
            if *version as i32 > stamp && games.contains(&game) && held == old {
                c.value.clone_from(&c.default);
                c.latched = None;
                reset.push(c.name.clone());
            }
        }
        self.set_rom("vc_cfgVersion", &CONFIG_VERSION.to_string());
        (reset, true)
    }

    /// The config file text, in the shape CoDMP.exe writes `config_mp.cfg`:
    /// `unbindall`, every bind, then every archived cvar as `seta`.
    pub fn config_text(&self) -> String {
        let mut s = format!("{CONFIG_HEADER}\nunbindall\n");
        for (k, v) in &self.binds {
            s.push_str(&format!("bind {k} \"{v}\"\n"));
        }
        // `Cvar_WriteVariables` (CoDMP.exe 0x43a148) writes a pending latch.
        for (k, c) in self.cvars.iter().filter(|(_, c)| c.archive) {
            let value = self
                .run_only
                .get(k)
                .or(c.latched.as_ref())
                .unwrap_or(&c.value);
            s.push_str(&format!("seta {} \"{value}\"\n", c.name));
        }
        s
    }
}

/// The entry an enum dvar set to `value` holds: the index's when `value`
/// is an integer in range, else the one `value` names in any case
/// (`Dvar_StringToEnum`, the lookup `Item_Paint`'s `ITEM_TYPE_DVARENUM`
/// arm repeats, Mac 0x1704d6).
pub fn enum_entry<'a>(entries: &'a [String], value: &str) -> Option<&'a str> {
    if let Ok(i) = value.trim().parse::<usize>() {
        return entries.get(i).map(String::as_str);
    }
    entries
        .iter()
        .find(|e| e.eq_ignore_ascii_case(value))
        .map(String::as_str)
}

/// `Cmd_TokenizeString`: whitespace splits, a quoted run is one token
/// (quotes dropped), `//` ends the line.
pub fn tokenize(line: &str) -> Vec<String> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while i < b.len() && b[i] <= b' ' {
            i += 1;
        }
        if i >= b.len() || b[i..].starts_with(b"//") {
            return out;
        }
        let start;
        if b[i] == b'"' {
            start = i + 1;
            i = start;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            out.push(line[start..i].to_string());
            i += 1;
        } else {
            start = i;
            while i < b.len() && b[i] > b' ' && b[i] != b'"' && !b[i..].starts_with(b"//") {
                i += 1;
            }
            out.push(line[start..i].to_string());
        }
    }
}

/// The raw text from token `n` on, quotes kept, as `Cmd_ArgsFrom` gives it.
/// Leading and trailing whitespace go.
fn args_from(line: &str, n: usize) -> String {
    let b = line.as_bytes();
    let mut i = 0;
    for _ in 0..n {
        while i < b.len() && b[i] <= b' ' {
            i += 1;
        }
        if i < b.len() && b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            i += 1;
        } else {
            while i < b.len() && b[i] > b' ' && b[i] != b'"' {
                i += 1;
            }
        }
    }
    let rest = line.get(i.min(line.len())..).unwrap_or("").trim();
    // A lone quoted argument comes back without its quotes.
    if n > 0 {
        let toks = tokenize(rest);
        if toks.len() == 1 && rest.starts_with('"') {
            return toks[0].clone();
        }
    }
    rest.to_string()
}

/// Splits on `;` and newlines outside quotes, as `Cbuf_Execute` does.
pub fn split_commands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => out.push(std::mem::take(&mut cur)),
            '\n' | '\r' => {
                quoted = false;
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.retain(|l| !l.trim().is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nextdemo_holds_a_whole_line_and_clears() {
        let mut s = Shell::new();
        s.execute("set nextdemo \"demo b; set nextdemo demo a\"");
        assert_eq!(s.cvar("nextdemo"), Some("demo b; set nextdemo demo a"));
        s.execute("set nextdemo \"\"");
        assert_eq!(s.cvar("nextdemo"), Some(""));
    }

    #[test]
    fn a_run_only_value_stays_out_of_the_config_until_set_again() {
        let mut s = Shell::new();
        s.set_for_run("vc_bloom", "1");
        assert_eq!(s.cvar_f32("vc_bloom"), 1.0);
        assert!(s.config_text().contains("seta vc_bloom \"0\"\n"));
        s.execute("set vc_bloom 2");
        assert!(s.config_text().contains("seta vc_bloom \"2\"\n"));
    }

    #[test]
    fn a_run_only_latched_cvar_takes_its_value_at_once() {
        let mut s = Shell::new();
        s.set_for_run("vc_hdr", "1");
        assert_eq!(s.cvar_f32("vc_hdr"), 1.0);
        assert!(!s.apply_latched());
        assert!(s.config_text().contains("seta vc_hdr \"0\"\n"));
    }

    /// The config a run reads at start-up, through the late registration.
    fn load(text: &str, game: Game) -> (Shell, Vec<String>, bool) {
        let mut s = Shell::new();
        s.exec_config(text);
        s.register_aa_samples();
        let (reset, moved) = s.upgrade_config(game);
        (s, reset, moved)
    }

    /// An unstamped vcod config's `r_aaSamples 4`, vcod's old default, loads
    /// as retail's 1 and the stamp moves; a 2 the user picked stays.
    #[test]
    fn an_old_config_s_stale_default_resets_and_a_chosen_value_stays() {
        let old = "// generated by vcod, do not modify\nunbindall\nseta r_aaSamples \"4\"\n";
        let (s, reset, moved) = load(old, Game::Cod4);
        assert_eq!(s.cvar("r_aaSamples"), Some("1"));
        assert_eq!(reset, ["r_aaSamples"]);
        assert!(moved);
        assert!(
            s.config_text()
                .contains(&format!("seta vc_cfgVersion \"{CONFIG_VERSION}\"\n"))
        );
        let picked = old.replace("\"4\"", "\"2\"");
        let (s, reset, moved) = load(&picked, Game::Cod4);
        assert_eq!(s.cvar("r_aaSamples"), Some("2"));
        assert!(reset.is_empty());
        assert!(moved);
        // CoD2's default was never 4: there it is the user's
        let (s, _, _) = load(old, Game::Cod2);
        assert_eq!(s.cvar("r_aaSamples"), Some("4"));
    }

    /// A stamped config keeps even an old default's value, set again after
    /// the upgrade; so does a config vcod did not write (1.7's).
    #[test]
    fn a_stamped_or_retail_config_keeps_its_values() {
        let mut s = Shell::new();
        s.register_aa_samples();
        s.execute("set r_aaSamples 4");
        let text = s.config_text();
        let (s, reset, moved) = load(&text, Game::Cod1);
        assert_eq!(
            (s.cvar("r_aaSamples"), reset.len(), moved),
            (Some("4"), 0, false)
        );
        let (s, reset, moved) = load("seta r_aaSamples \"4\"\n", Game::Cod4);
        assert_eq!(
            (s.cvar("r_aaSamples"), reset.len(), moved),
            (Some("4"), 0, false)
        );
    }

    /// No config: the shell already holds this build's stamp, which the
    /// first config it writes carries.
    #[test]
    fn a_fresh_config_gets_the_stamp() {
        let mut s = Shell::new();
        s.register_aa_samples();
        assert_eq!(s.upgrade_config(Game::Cod1), (Vec::new(), false));
        assert!(
            s.config_text()
                .contains(&format!("seta vc_cfgVersion \"{CONFIG_VERSION}\"\n"))
        );
    }

    /// A config from the opt-in builds (`seta r_vc_depthprepass "0"`) no
    /// longer pins the prepass off, nor is the line written back.
    #[test]
    fn a_config_s_depth_prepass_gives_way_to_the_game_s_default() {
        let mut s = Shell::new();
        s.execute("seta r_vc_depthprepass \"0\"");
        s.register_depth_prepass(true);
        assert_eq!(s.cvar("r_vc_depthprepass"), Some("1"));
        assert!(!s.config_text().contains("r_vc_depthprepass"));
    }

    /// An old UO config (`unbindall` and no `+sprint`) gets ALT back; one
    /// that moved the sprint or used ALT keeps its choice.
    #[test]
    fn a_config_without_the_sprint_gets_alt_only_where_free() {
        let mut s = Shell::new();
        s.execute("unbindall\nbind W +forward");
        assert!(s.bind_if_free("ALT", "+sprint"));
        assert_eq!(s.bind("ALT"), Some("+sprint"));
        let mut s = Shell::new();
        s.execute("unbindall\nbind F +sprint");
        assert!(!s.bind_if_free("ALT", "+sprint"));
        let mut s = Shell::new();
        s.execute("unbindall\nbind ALT +melee");
        assert!(!s.bind_if_free("ALT", "+sprint"));
        assert_eq!(s.bind("ALT"), Some("+melee"));
    }

    #[test]
    fn tokenizer_groups_quotes_and_drops_comments() {
        assert_eq!(
            tokenize("bind  K \"say hi there\" // note"),
            ["bind", "K", "say hi there"]
        );
        assert_eq!(tokenize("a\"b c\"d"), ["a", "b c", "d"]);
        assert!(tokenize("   // all comment").is_empty());
    }

    #[test]
    fn semicolons_split_outside_quotes() {
        assert_eq!(
            split_commands("bind k \"a; b\"; echo x\nquit"),
            ["bind k \"a; b\"", " echo x", "quit"]
        );
    }

    #[test]
    fn bind_and_key_events() {
        let mut s = Shell::new();
        assert_eq!(s.execute("bind k \"+attack\""), [Effect::SaveConfig]);
        assert_eq!(s.bind("K"), Some("+attack"));
        assert_eq!(
            s.key_event("K", true),
            [Effect::Button(Action::Attack, true)]
        );
        assert_eq!(
            s.key_event("K", false),
            [Effect::Button(Action::Attack, false)]
        );
        // A one-shot bind fires on the press only.
        s.execute("bind g weaponslot pistol");
        assert_eq!(s.key_event("G", true), [Effect::Impulse(Action::Slot(3))]);
        assert!(s.key_event("G", false).is_empty());
        assert!(s.bind_is_gameplay("G"));
        assert!(!s.bind_is_gameplay("T"));
    }

    /// The Options menu's stance binds (issue #8): the held ones press and
    /// release, the rest are one-shots, and none is unknown.
    #[test]
    fn stance_binds_dispatch() {
        let mut s = Shell::new();
        for (cmd, action) in [
            ("+movedown", Action::MoveDown),
            ("+moveup", Action::MoveUp),
            ("+prone", Action::HoldProne),
        ] {
            s.execute(&format!("bind CTRL {cmd}"));
            assert_eq!(s.key_event("CTRL", true), [Effect::Button(action, true)]);
            assert_eq!(s.key_event("CTRL", false), [Effect::Button(action, false)]);
            assert!(s.bind_is_gameplay("CTRL"));
        }
        for (cmd, action) in [
            ("togglecrouch", Action::ToggleCrouch),
            ("toggleprone", Action::ToggleProne),
            ("lowerstance", Action::LowerStance),
            ("raisestance", Action::RaiseStance),
        ] {
            s.execute(&format!("bind CTRL {cmd}"));
            assert_eq!(s.key_event("CTRL", true), [Effect::Impulse(action)]);
            assert!(s.key_event("CTRL", false).is_empty());
            assert!(s.bind_is_gameplay("CTRL"));
        }
    }

    /// `weaponslot` takes UO's eight names, case folded, or a number as
    /// `atol` reads it, and says nothing for anything else (0x300382f0).
    #[test]
    fn weaponslot_takes_names_and_numbers() {
        let mut s = Shell::new();
        let slot = |s: &mut Shell, line: &str| match s.execute(line).as_slice() {
            [Effect::Impulse(Action::Slot(n))] => *n,
            other => panic!("{line}: {other:?}"),
        };
        for (n, name) in ["primary", "primaryb", "pistol", "grenade"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(slot(&mut s, &format!("weaponslot {name}")), n + 1);
        }
        assert_eq!(slot(&mut s, "weaponslot SmokeGrenade"), 5);
        assert_eq!(slot(&mut s, "weaponslot satchel"), 6);
        assert_eq!(slot(&mut s, "weaponslot binocular"), 7);
        assert_eq!(slot(&mut s, "weaponslot 4"), 4);
        assert_eq!(slot(&mut s, "weaponslot 2x"), 2);
        for refused in ["none", "binoculars", "-1", ""] {
            assert_eq!(slot(&mut s, &format!("weaponslot {refused}")), 0);
        }
    }

    #[test]
    fn bind_queries_and_bad_keys() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("bind w"),
            [Effect::Print("\"w\" = \"+forward\"".into())]
        );
        assert_eq!(
            s.execute("bind nosuchkey quit"),
            [Effect::Print("\"nosuchkey\" isn't a valid key".into())]
        );
        s.execute("unbind w");
        assert_eq!(
            s.execute("bind w"),
            [Effect::Print("\"w\" is not bound".into())]
        );
    }

    #[test]
    fn multi_command_binds_run_every_command() {
        let mut s = Shell::new();
        s.execute("bind h \"say hi; weapnext\"");
        assert_eq!(
            s.key_event("H", true),
            [
                Effect::Forward("say hi".into()),
                Effect::Impulse(Action::NextWeapon)
            ]
        );
    }

    #[test]
    fn cvars_set_print_and_toggle() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("cl_run"),
            [Effect::Print(
                "\"cl_run\" is:\"1^7\" default:\"1^7\"".into()
            )]
        );
        assert_eq!(s.execute("toggle cl_run"), [Effect::SaveConfig]);
        assert_eq!(s.cvar("CL_RUN"), Some("0"));
        assert_eq!(
            s.execute("name \"Big Bob\""),
            [Effect::Userinfo, Effect::SaveConfig]
        );
        assert_eq!(s.cvar("name"), Some("Big Bob"));
        // A user cvar: `set` keeps it out of the config, `seta` writes it.
        assert!(s.execute("set foo bar").is_empty());
        assert!(!s.config_text().contains("foo"));
        s.execute("seta foo baz");
        assert!(s.config_text().contains("seta foo \"baz\"\n"));
        s.execute("toggle foo a b c");
        assert_eq!(s.cvar("foo"), Some("a"));
        s.execute("toggle foo a b c");
        assert_eq!(s.cvar("foo"), Some("b"));
    }

    #[test]
    fn unknown_and_forwarded_commands() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("say hello world"),
            [Effect::Forward("say hello world".into())]
        );
        assert_eq!(
            s.execute("callvote map mp_harbor"),
            [Effect::Unknown {
                word: "callvote".into(),
                line: "callvote map mp_harbor".into()
            }]
        );
        assert_eq!(s.execute("cmd score"), [Effect::Forward("score".into())]);
    }

    #[test]
    fn config_round_trips() {
        let mut s = Shell::new();
        s.execute("unbindall; bind mouse3 +attack; bind f1 \"vote yes\"; name Rob");
        let text = s.config_text();
        let mut t = Shell::new();
        t.execute(&text);
        assert_eq!(t.config_text(), text);
        assert_eq!(t.bind("F1"), Some("vote yes"));
        assert_eq!(t.bind("W"), None);
        assert_eq!(t.cvar("name"), Some("Rob"));
    }

    #[test]
    fn completion_covers_commands_and_cvars() {
        let s = Shell::new();
        assert_eq!(s.complete("unb"), ["unbind", "unbindall"]);
        assert_eq!(
            s.complete("CL_"),
            [
                "cl_allowDownload",
                "cl_anglespeedkey",
                "cl_avidemo",
                "cl_forceavidemo",
                "cl_freelook",
                "cl_freezeDemo",
                "cl_language",
                "cl_maxpackets",
                "cl_mouseAccel",
                "cl_packetdup",
                "cl_pitchspeed",
                "cl_run",
                "cl_stanceTemp",
                "cl_timeNudge",
                "cl_voice",
                "cl_wwwDownload",
                "cl_yawspeed",
            ]
        );
        assert!(s.complete("zzz").is_empty());
    }

    /// The voice options' four dvars register for CoD2 and CoD4 at
    /// `Voice_Init`'s defaults, archived, keeping a config's value.
    #[test]
    fn winvoice_dvars_register_as_voice_init_does() {
        let defaults = [
            ("winvoice_mic_mute", "1"),
            ("winvoice_mic_reclevel", "65535"),
            ("winvoice_save_voice", "0"),
            ("winvoice_mic_scaler", "1"),
        ];
        for cod4 in [false, true] {
            let mut s = Shell::new();
            assert_eq!(s.cvar("winvoice_mic_scaler"), None);
            s.execute("seta winvoice_mic_scaler 1.25");
            match cod4 {
                true => s.register_cod4_options(),
                false => s.use_cod2_cvars(),
            }
            for (name, default) in defaults {
                let want = if name == "winvoice_mic_scaler" {
                    "1.25"
                } else {
                    default
                };
                assert_eq!(s.cvar(name), Some(want), "{name}");
                assert!(s.cvars[name].archive, "{name}");
            }
        }
    }

    /// CoD2's `snd_volume` registers at 1 and archived, keeping what the
    /// config set; its options menus run `setfromdvar`.
    #[test]
    fn cod2_dvars_and_setfromdvar() {
        let mut s = Shell::new();
        s.use_cod2_cvars();
        assert_eq!(s.cvar("snd_volume"), Some("1"));
        let mut s = Shell::new();
        s.execute("seta snd_volume 0.3");
        s.use_cod2_cvars();
        assert_eq!(s.cvar("snd_volume"), Some("0.3"));
        assert!(s.config_text().contains("snd_volume"));
        s.execute("setfromdvar ui_snd_khz snd_volume");
        assert_eq!(s.cvar("ui_snd_khz"), Some("0.3"));
    }

    #[test]
    fn setfromcvar_exec_and_keys_in_key_order() {
        let mut s = Shell::new();
        s.execute("setfromcvar ui_name name; setfromcvar ui_x nosuchcvar");
        assert_eq!(s.cvar("ui_name"), Some("Unknown Soldier"));
        assert_eq!(s.cvar("ui_x"), Some(""));
        assert_eq!(
            s.execute("exec default_mp.cfg; vid_restart"),
            [Effect::Exec("default_mp.cfg".into()), Effect::VidRestart]
        );
        // Space (32) before letters, letters before named keys, mouse last.
        s.execute("bind MOUSE2 +forward; bind UPARROW +forward; bind SPACE +forward");
        assert_eq!(
            s.keys_bound_to("+FORWARD"),
            ["SPACE", "W", "UPARROW", "MOUSE2"]
        );
    }

    #[test]
    fn key_text_names_the_first_two_keys() {
        let mut loc = Localized::default();
        loc.parse_into(
            "key",
            "REFERENCE SPACE\nLANG_ENGLISH \"Space\"\nREFERENCE OR\nLANG_ENGLISH \"or\"\n",
        );
        let mut s = Shell::new();
        assert_eq!(s.key_text("+gostand", &loc).as_deref(), Some("Space"));
        assert_eq!(s.key_text("toggleprone", &loc), None);
        s.execute("bind Z +gostand; bind X +gostand");
        assert_eq!(s.key_text("+gostand", &loc).as_deref(), Some("Space or X"));
    }

    #[test]
    fn client_cvars_start_at_defaults() {
        let s = Shell::new();
        for (name, v) in [
            ("sensitivity", "5"),
            ("m_yaw", "0.022"),
            ("m_pitch", "0.022"),
            ("rate", "25000"),
            ("snaps", "20"),
            ("cg_fov", "80"),
        ] {
            assert_eq!(s.cvar(name), Some(v), "{name}");
        }
        assert_eq!(
            s.userinfo(),
            Userinfo {
                name: "Unknown Soldier".into(),
                rate: "25000".into(),
                snaps: "20".into(),
                password: String::new(),
                www_download: "1".into(),
            }
        );
    }

    #[test]
    fn rate_and_snaps_are_userinfo() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("rate 15000"),
            [Effect::Userinfo, Effect::SaveConfig]
        );
        assert_eq!(
            s.execute("seta snaps 30"),
            [Effect::Userinfo, Effect::SaveConfig]
        );
        assert_eq!(s.userinfo().rate, "15000");
        assert_eq!(s.userinfo().snaps, "30");
        assert_eq!(s.execute("sensitivity 3"), [Effect::SaveConfig]);
    }

    /// 1.7's `r_glow` is a cheat dvar, on by default.
    #[test]
    fn cod4_r_glow_is_a_cheat_dvar() {
        let mut s = Shell::new();
        s.register_cod4_options();
        assert_eq!(s.cvar("r_glow"), Some("1"));
        assert_eq!(
            s.execute("r_glow 0"),
            [Effect::Print("r_glow is cheat protected.".into())]
        );
        s.apply_systeminfo(r"\sv_cheats\1");
        s.execute("r_glow 0");
        assert_eq!(s.cvar("r_glow"), Some("0"));
    }

    /// 1.7's three third-person dvars are cheat dvars at their defaults.
    #[test]
    fn cod4_third_person_dvars_are_cheat_protected() {
        let mut s = Shell::new();
        s.register_cod4_options();
        for (name, value) in [
            ("cg_thirdPerson", "0"),
            ("cg_thirdPersonRange", "120"),
            ("cg_thirdPersonAngle", "0"),
        ] {
            assert_eq!(s.cvar(name), Some(value));
            assert_eq!(
                s.execute(&format!("{name} 1")),
                [Effect::Print(format!("{name} is cheat protected."))]
            );
        }
        s.apply_systeminfo(r"\sv_cheats\1");
        s.execute("cg_thirdPerson 1");
        assert_eq!(s.cvar("cg_thirdperson"), Some("1"));
    }

    #[test]
    fn cg_fov_is_cheat_protected() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("cg_fov 95"),
            [Effect::Print("cg_fov is cheat protected.".into())]
        );
        assert_eq!(s.cvar("cg_fov"), Some("80"));
        assert_eq!(
            s.execute("sv_cheats 1"),
            [Effect::Print("sv_cheats is read only.".into())]
        );
        s.apply_systeminfo(r"\sv_cheats\1");
        assert_eq!(s.execute("cg_fov 95"), [Effect::SaveConfig]);
        assert_eq!(s.cvar("cg_fov"), Some("95"));
        assert_eq!(s.apply_systeminfo(r"\sv_cheats\0"), [Effect::SaveConfig]);
        assert_eq!(s.cvar("cg_fov"), Some("80"), "sv_cheats 0 resets it");
        s.apply_systeminfo(r"\sv_cheats\1");
        s.execute("cg_fov 95");
        s.apply_systeminfo(r"\sv_pure\1");
        assert_eq!(s.cvar("cg_fov"), Some("80"), "no sv_cheats reads 0");
    }

    /// `CL_SystemInfoChanged` forces every key: read-only, cheat and
    /// latched cvars too, and keys the client never registered.
    /// `fs_game` is `CVAR_INIT`: the console cannot set it, the Mods
    /// menu's forced set and a systeminfo can, and a systeminfo without the
    /// key leaves it.
    #[test]
    fn fs_game_moves_only_by_force() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("set fs_game x"),
            [Effect::Print("fs_game is write protected.".into())]
        );
        assert!(s.force("fs_game", "uo").is_empty());
        assert_eq!(s.cvar("fs_game"), Some("uo"));
        s.apply_systeminfo(r"\fs_game\mymod");
        assert_eq!(s.cvar("fs_game"), Some("mymod"));
        s.apply_systeminfo(r"\sv_serverid\3");
        assert_eq!(s.cvar("fs_game"), Some("mymod"));
        assert!(!s.config_text().contains("fs_game"));
    }

    #[test]
    fn the_systeminfo_forces_every_key() {
        let mut s = Shell::new();
        let out = s.apply_systeminfo(
            r"\cl_allowDownload\0\rate\5000\vc_vanilla\1\cg_fov\90\r_picmip\2\fs_game\mymod\sv_serverid\16",
        );
        assert_eq!(out, [Effect::SaveConfig, Effect::Userinfo]);
        assert_eq!(s.cvar("cl_allowDownload"), Some("0"));
        assert_eq!(s.cvar("rate"), Some("5000"));
        assert_eq!(s.cvar("vc_vanilla"), Some("1"));
        assert_eq!(s.cvar("cg_fov"), Some("90"), "forced past the cheat check");
        assert_eq!(s.cvar("r_picmip"), Some("2"), "no latch");
        assert_eq!(s.cvar("fs_game"), Some("mymod"));
        assert_eq!(s.cvar("sv_serverid"), Some("16"));
        // A cvar the systeminfo made is not archived.
        assert!(!s.config_text().contains("sv_serverid"));
        // Keys already at their values change nothing; without `sv_cheats`
        // the forced `cg_fov` goes back to its default.
        assert!(s.apply_systeminfo(r"\sv_cheats\1\rate\5000").is_empty());
        assert_eq!(s.apply_systeminfo(r"\rate\5000"), [Effect::SaveConfig]);
        assert_eq!(s.cvar("cg_fov"), Some("80"));
    }

    #[test]
    fn vc_vanilla_is_read_only() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("seta vc_vanilla 1"),
            [Effect::Print("vc_vanilla is read only.".into())]
        );
        assert_eq!(s.cvar("vc_vanilla"), Some("0"));
        s.set_rom("vc_vanilla", "1");
        assert_eq!(s.cvar("vc_vanilla"), Some("1"));
    }

    /// A CoD4 config whose melee moved from V to B keeps V free on the
    /// next start; a first start takes the stock keys.
    #[test]
    fn a_cod4_config_keeps_a_cleared_key_free() {
        let mut s = Shell::new();
        assert!(s.use_cod4_defaults(false) > 0);
        assert_eq!(s.bind("V"), Some("+melee"));
        s.execute("unbind V; bind B +melee");
        let saved = s.config_text();
        let mut s = Shell::new();
        s.execute(&saved);
        assert_eq!(s.use_cod4_defaults(true), 0);
        assert_eq!(s.bind("V"), None);
        assert_eq!(s.bind("B"), Some("+melee"));
    }

    /// `Dvar_GetInt` reads an enum dvar as its entry's index; vcod's own
    /// `r_mode` -1, which no entry names, as -1.
    #[test]
    fn an_enum_dvar_reads_as_its_index() {
        let mut s = Shell::new();
        s.set_enum_domain("r_mode", vec!["640x480".into(), "1920x1080".into()]);
        assert_eq!(s.int("r_mode"), -1);
        s.execute("set r_mode 1920x1080");
        assert_eq!(s.int("r_mode"), 1);
        assert_eq!(s.int("com_maxfps"), 0);
    }

    #[test]
    fn cl_language_waits_for_a_restart() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("seta cl_language 2"),
            [
                Effect::Print("cl_language will be changed upon restarting.".into()),
                Effect::SaveConfig
            ]
        );
        assert_eq!(s.cvar("cl_language"), Some("0"));
        assert!(s.config_text().contains("seta cl_language \"2\"\n"));
        assert!(s.execute("cl_language 2").is_empty());
        assert!(s.apply_latched());
        assert_eq!(s.cvar("cl_language"), Some("2"));
        assert!(!s.apply_latched());
        // Back to the live value with nothing pending is no change at all.
        assert!(s.execute("set cl_language 2").is_empty());
    }

    #[test]
    fn image_and_sound_quality_wait_for_their_restarts() {
        let mut s = Shell::new();
        s.execute("r_picmip 3; r_texturebits 16; mss_khz 11; r_textureMode GL_LINEAR");
        assert_eq!(s.cvar("r_picmip"), Some("0"));
        assert_eq!(s.cvar("r_texturebits"), Some("0"));
        assert_eq!(s.cvar("mss_khz"), Some("44"));
        // read every frame, never latched
        assert_eq!(s.cvar("r_textureMode"), Some("GL_LINEAR"));
        assert!(s.apply_latched_video());
        assert_eq!(s.cvar("r_picmip"), Some("3"));
        assert_eq!(s.cvar("r_texturebits"), Some("16"));
        assert_eq!(
            s.cvar("mss_khz"),
            Some("44"),
            "vid_restart leaves the sound alone"
        );
        assert!(s.apply_latched_sound());
        assert_eq!(s.cvar("mss_khz"), Some("11"));
    }

    #[test]
    fn v_opens_the_quick_message_menu() {
        let mut s = Shell::new();
        assert_eq!(s.key_event("V", true), [Effect::QuickMessage]);
        assert_eq!(s.complete("mp_q"), ["mp_QuickMessage"]);
    }
}
