//! The configstrings a server sets before any entity exists, from
//! docs/research/cod11-server-handshake.md ("What a minimal server has to set").
//! Entity-driven slots (3, 8, 11, 12, 269.., 524.., 781..) stay empty.

use crate::server::ServerConfig;
use vcod_common::net::connectionless::Info;
use vcod_common::net::protocol::PROTOCOL_V1;

/// `BG_SetupWeaponInfo`'s list, configstring 7, 1-based on the wire.
pub const WEAPON_LIST: &str = "bar_mp bar_slow_mp bren_mp colt_mp enfield_mp fg42_mp fg42_semi_mp fraggrenade_mp kar98k_mp kar98k_sniper_mp luger_mp m1carbine_mp m1garand_mp mg42_bipod_duck_mp mg42_bipod_prone_mp mg42_bipod_stand_mp mk1britishfrag_mp mosin_nagant_mp mosin_nagant_sniper_mp mp40_mp mp44_mp mp44_semi_mp panzerfaust_mp ppsh_mp ppsh_semi_mp ptrs41_antitank_rifle_mp rgd-33russianfrag_mp springfield_mp sten_mp stielhandgranate_mp thompson_mp thompson_semi_mp";

/// Map-independent game-module configstrings. 140..178 are cvar names and
/// 204..242 their values (`Cvar_Set(cs[140+i], cs[204+i])` on the client);
/// each pair sits adjacent so the halves cannot drift apart.
pub const STATIC: &[(usize, &str)] = &[
    (2, "cod"),
    (7, WEAPON_LIST),
    (13, "0"), // level.startTime
    (20, "\\winner\\0"),
    (21, "gfx/hud/hud@status_dead.tga"),
    (22, "gfx/hud/hud@status_connecting.tga"),
    (29, "gfx/hud/headicon@quickmessage"),
    (140, "bg_duck2prone_time"),
    (204, "400"),
    (141, "bg_foliagesnd_fastinterval"),
    (205, "500"),
    (142, "bg_foliagesnd_maxspeed"),
    (206, "180"),
    (143, "bg_foliagesnd_minspeed"),
    (207, "40"),
    (144, "bg_foliagesnd_resetinterval"),
    (208, "500"),
    (145, "bg_foliagesnd_slowinterval"),
    (209, "1500"),
    (146, "bg_ladder_yawcap"),
    (210, "100"),
    (147, "bg_prone2duck_time"),
    (211, "400"),
    (148, "bg_prone_softyawedge"),
    (212, "1"),
    (149, "bg_prone_yawcap"),
    (213, "85"),
    (150, "bg_viewheight_crouched"),
    (214, "40"),
    (151, "bg_viewheight_prone"),
    (215, "11"),
    (152, "bg_viewheight_standing"),
    (216, "60"),
    (153, "g_ScoresBanner_Allies"),
    (217, "gfx/hud/hud@mpflag_american.tga"),
    (154, "g_ScoresBanner_Axis"),
    (218, "gfx/hud/hud@mpflag_german.tga"),
    (155, "g_ScoresBanner_None"),
    (219, "gfx/hud/hud@mpflag_none.tga"),
    (156, "g_ScoresBanner_Spectators"),
    (220, "gfx/hud/hud@mpflag_spectator.tga"),
    (157, "g_TeamColor_Allies"),
    (221, "0.5 0.5 1"),
    (158, "g_TeamColor_Axis"),
    (222, "1 0.5 0.5"),
    (159, "g_TeamName_Allies"),
    (223, "GAME_ALLIES"),
    (160, "g_TeamName_Axis"),
    (224, "GAME_AXIS"),
    (161, "scr_allow_bar"),
    (225, "1"),
    (162, "scr_allow_bren"),
    (226, "1"),
    (163, "scr_allow_enfield"),
    (227, "1"),
    (164, "scr_allow_fg42"),
    (228, "0"),
    (165, "scr_allow_kar98k"),
    (229, "1"),
    (166, "scr_allow_kar98ksniper"),
    (230, "1"),
    (167, "scr_allow_m1carbine"),
    (231, "1"),
    (168, "scr_allow_m1garand"),
    (232, "1"),
    (169, "scr_allow_mp40"),
    (233, "1"),
    (170, "scr_allow_mp44"),
    (234, "1"),
    (171, "scr_allow_nagant"),
    (235, "1"),
    (172, "scr_allow_nagantsniper"),
    (236, "1"),
    (173, "scr_allow_panzerfaust"),
    (237, "1"),
    (174, "scr_allow_ppsh"),
    (238, "1"),
    (175, "scr_allow_springfield"),
    (239, "1"),
    (176, "scr_allow_sten"),
    (240, "1"),
    (177, "scr_allow_thompson"),
    (241, "1"),
    (178, "scr_allow_vote"),
    (242, "1"),
    // 180 `scr_motd` stays unset. Retail sets it empty, and an empty value
    // ends the client's 140/204 loop.
    // 1180/1181 pick the American/German team menus. The map's nationality is
    // script data the server does not have, so a Russian map gets the wrong
    // menu until game logic exists.
    (1180, "team_americangerman"),
    (1181, "weapon_american"),
    (1182, "weapon_german"),
    (1183, "viewmap"),
    (1184, "callvote"),
    (1185, "quickcommands"),
    (1186, "quickstatements"),
    (1187, "quickresponses"),
    (1212, "CGAME_USEMG42"),
    (1213, "CGAME_USEPTRS41"),
    (1245, "MPSCRIPT_PRESS_ACTIVATE_TO_RESPAWN"),
    (1246, "MPSCRIPT_KILLCAM"),
    (1502, "black"),
    (1503, "hudScoreboard_mp"),
    (1504, "gfx/hud/hud@mpflag_none.tga"),
    (1505, "gfx/hud/hud@mpflag_spectator.tga"),
];

/// `Cvar_InfoString(CVAR_SERVERINFO)`, capture cs 0. Alphabetical, as the
/// cvar table iterates.
pub fn serverinfo(cfg: &ServerConfig) -> Info {
    let mut i = Info::new();
    i.set("g_gametype", &cfg.gametype)
        .set("gamename", "main")
        .set("mapname", &cfg.map)
        .set("protocol", PROTOCOL_V1.version)
        .set("shortversion", "1.1")
        .set("sv_allowAnonymous", 0)
        .set("sv_floodProtect", 1)
        .set("sv_hostname", &cfg.hostname)
        .set("sv_maxclients", cfg.max_clients)
        .set("sv_maxPing", 0)
        .set("sv_maxRate", 0)
        .set("sv_minPing", 0)
        .set("sv_privateClients", 0)
        .set("sv_pure", 0);
    i
}

/// `Cvar_InfoString_Big(CVAR_SYSTEMINFO)`, capture cs 1, minus the pak lists.
/// Must stay under `MAX_INFO_STRING` with `sv_serverid` intact; the overflow
/// is in docs/research/cod11-server-handshake.md, "Configstring 1, systeminfo".
pub fn systeminfo(server_id: u8) -> Info {
    let mut i = Info::new();
    i.set("bg_fallDamageMaxHeight", 480)
        .set("bg_fallDamageMinHeight", 256)
        .set("g_synchronousClients", 0)
        .set("pmove_fixed", 0)
        .set("pmove_msec", 8)
        .set("sv_cheats", 0)
        .set("sv_pure", 0)
        .set("sv_serverid", server_id)
        .set("timescale", 1);
    i
}

/// The full 2048-slot table for a fresh map.
pub fn static_configstrings(cfg: &ServerConfig, server_id: u8) -> Vec<String> {
    let mut cs = vec![String::new(); PROTOCOL_V1.max_configstrings];
    // Names an out-of-range literal instead of a bare index panic.
    debug_assert!(STATIC.iter().all(|&(i, _)| i < cs.len()) && 1501 < cs.len());
    cs[0] = serverinfo(cfg).to_string();
    cs[1] = systeminfo(server_id).to_string();
    for &(i, s) in STATIC {
        cs[i] = s.to_string();
    }
    // 179/243 is the layout-image cvar pair, 1501 the same path in the
    // material block.
    let layout = format!("levelshots/layouts/hud@layout_{}", cfg.map);
    cs[179] = "scr_layoutimage".to_string();
    cs[243] = layout.clone();
    cs[1501] = layout;
    cs
}
