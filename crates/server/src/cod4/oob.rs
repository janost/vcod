//! CoD4 1.7's connectionless packets (docs/research/cod4-handshake.md 2,
//! 3): `getinfo`, `getstatus`, `getchallenge`, the plain-text `connect`,
//! the seven `stats` packets and `rcon`, and the console the last runs.

use std::net::SocketAddr;
use std::time::Instant;

use vcod_cod4::net::stats::{STAT_CHUNK, STAT_PACKETS, STATS_BYTES};

use super::{Cod4Server, State};
use crate::engine::link;
use crate::rcon::{Redirect, Verdict};

/// The engine dvars the info strings read, at the values the 1.7 captures
/// carry (VERIFIED live, `handshake-1.7.txt`'s configstrings 0 and 1 and
/// its `getinfo`). `scr_game_allowkillcam` and `scr_team_fftype` are the
/// stock gametype's (its `kc\1`, no `ff`), registered here until the gsc
/// boot sets them.
pub(super) const ENGINE_CVARS: &[(&str, &str)] = &[
    ("gamename", "Call of Duty 4"),
    ("protocol", "6"),
    ("shortversion", "1.7"),
    ("g_compassShowEnemies", "0"),
    ("sv_allowAnonymous", "0"),
    ("sv_disableClientConsole", "0"),
    ("sv_floodprotect", "4"),
    ("sv_maxPing", "0"),
    ("sv_maxRate", "5000"),
    ("sv_minPing", "0"),
    ("sv_privateClients", "0"),
    ("sv_punkbuster", "0"),
    ("sv_pure", "1"),
    ("sv_voice", "0"),
    ("ui_maxclients", "32"),
    ("bg_fallDamageMaxHeight", "300"),
    ("bg_fallDamageMinHeight", "128"),
    ("com_timescale", "1"),
    ("con_gameMsgWindow0Filter", "gamenotify obituary"),
    ("con_gameMsgWindow1Filter", "boldgame"),
    ("con_gameMsgWindow2Filter", "subtitle"),
    ("g_synchronousClients", "0"),
    ("sv_cheats", "0"),
    ("sv_clientSideBullets", "1"),
    ("sv_voiceQuality", "3"),
    ("timescale", "1"),
    ("scr_game_allowkillcam", "1"),
    ("scr_team_fftype", "0"),
    ("fs_game", ""),
    ("g_password", ""),
    ("rcon_password", ""),
    // vcod heartbeats only when asked (D6); retail beats at 1 too
    // (cod4-server-frame.md 5).
    ("dedicated", "1"),
    ("sv_master1", ""),
    // Registered defaults; `sv_timeout`'s
    // 240 is Q3's.
    ("sv_timeout", "240"),
    // `SV_Init`'s int dvar (0x81735c8): 20, 10..1000, read every frame
    // (cod4-server-frame.md 8).
    ("sv_fps", "20"),
    ("sv_connectTimeout", "45"),
    ("sv_zombietime", "2"),
    ("sv_reconnectlimit", "3"),
    ("sv_kickBanTime", "300"),
    // The map cycle's (the names VERIFIED in `cod4_lnxded`).
    ("sv_mapRotation", ""),
    ("sv_mapRotationCurrent", ""),
    // `SV_Init`'s download dvars (0x8173319, 0x81738c9..0x817393a):
    // `sv_allowDownload` on, the www ones off.
    ("sv_allowDownload", "1"),
    ("sv_wwwDownload", "0"),
    ("sv_wwwBaseURL", ""),
    ("sv_wwwDlDisconnected", "0"),
    // The game's vote dvars (`G_RegisterDvars`: `g_allowVote` 1,
    // `g_voteAbstainWeight` 0.5).
    ("g_allowVote", "1"),
    ("g_voteAbstainWeight", "0.5"),
    // Voice chat's game dvars (`G_RegisterDvars`, 0x80b4c86..0x80b4d93;
    // `BG_RegisterDvars` 0x8055e2c, 0x8055e5e; cod4-voice.md 3).
    ("voice_global", "0"),
    ("voice_localEcho", "0"),
    ("voice_deadChat", "0"),
    ("g_voiceChatTalkingDuration", "500"),
    ("perk_parabolicRadius", "400"),
    ("perk_parabolicAngle", "180"),
    // `G_RegisterDvars`: `g_log` (0x80b4830, archived), `g_logSync`
    // (0x80b4868) and `g_inactivity` (0x80b4a3c, 0..INT_MAX). The binary
    // fills the paths in; without a home path nothing is logged.
    ("g_log", "games_mp.log"),
    ("g_logSync", "0"),
    ("g_inactivity", "0"),
    ("fs_basepath", ""),
    ("fs_homepath", ""),
    // The archived dvars nothing here reads, at the values a retail
    // `writeconfig` wrote (VERIFIED live, cod4-handshake.md 14.1).
    ("com_introPlayed", "0"),
    ("com_maxfps", "85"),
    ("con_gameMsgWindow3Filter", ""),
    ("g_banIPs", ""),
    ("g_clonePlayerMaxVelocity", "80"),
    ("g_deadChat", "1"),
    ("g_dropForwardSpeed", "10"),
    ("g_dropHorzSpeedRand", "100"),
    ("g_dropUpSpeedBase", "10"),
    ("g_dropUpSpeedRand", "5"),
    ("g_oldVoting", "1"),
    ("g_playerCollisionEjectSpeed", "25"),
    ("g_redCrosshairs", "1"),
    ("g_useholdspawndelay", "500"),
    ("loc_forceEnglish", "0"),
    ("loc_language", "0"),
];

/// The dvars a dedicated 1.7 server registers archived: the `seta` lines
/// of its `writeconfig` (VERIFIED live, cod4-handshake.md 14.1). A `seta`
/// adds its dvar.
pub(super) const ARCHIVED: &[&str] = &[
    "com_introPlayed",
    "com_maxfps",
    "con_gameMsgWindow0Filter",
    "con_gameMsgWindow1Filter",
    "con_gameMsgWindow2Filter",
    "con_gameMsgWindow3Filter",
    "g_banIPs",
    "g_clonePlayerMaxVelocity",
    "g_deadChat",
    "g_dropForwardSpeed",
    "g_dropHorzSpeedRand",
    "g_dropUpSpeedBase",
    "g_dropUpSpeedRand",
    "g_log",
    "g_logSync",
    "g_oldVoting",
    "g_playerCollisionEjectSpeed",
    "g_redCrosshairs",
    "g_useholdspawndelay",
    "g_voiceChatTalkingDuration",
    "g_voteAbstainWeight",
    "loc_forceEnglish",
    "loc_language",
    "sv_allowDownload",
    "sv_floodprotect",
    "sv_hostname",
    "sv_maxclients",
    "sv_maxPing",
    "sv_maxRate",
    "sv_minPing",
    "sv_punkbuster",
    "sv_reconnectlimit",
    "sv_voice",
    "sv_wwwBaseURL",
    "sv_wwwDlDisconnected",
    "sv_wwwDownload",
    "ui_maxclients",
    "voice_deadChat",
    "voice_global",
    "voice_localEcho",
];

/// The header `Com_WriteConfigToFile` writes (file 0x1dba00).
const CONFIG_HEADER: &str = "// generated by Call of Duty, do not modify\n";

/// The console channel line a dedicated server's `writeconfig` ends with
/// (VERIFIED live).
const CONFIG_CHANNELS: &str = "con_hidechannel *; con_showchannel dontfilter error gamenotify boldgame subtitle obituary logfile_only console_only gfx sound files devgui profile ui client server system playerweap ai anim physics fx leaderboards parserscript script\n";

/// The domain `dvar_bool`, `dvar_int` or `dvar_float` gives the dvar it
/// makes, which a query prints (`Dvar_PrintDomain`'s strings, file
/// 0x1dc3c0..; VERIFIED live).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Domain {
    Bool,
    Int(i32, i32),
    Float(f32, f32),
}

impl Domain {
    fn describe(self) -> String {
        match self {
            Domain::Bool => "Domain is 0 or 1".into(),
            Domain::Int(lo, hi) => format!("Domain is any integer from {lo} to {hi}"),
            Domain::Float(lo, hi) => format!(
                "Domain is any number from {} to {}",
                vcod_gsc::value::format_g(lo),
                vcod_gsc::value::format_g(hi)
            ),
        }
    }
}

/// Configstring 0's dvars besides `g_gametype`, `mapname`, `sv_hostname`
/// and `sv_maxclients` (VERIFIED live: the capture's key set; `fs_game`
/// only when set, cod4-fastfile.md 13).
pub(super) const SERVERINFO: &[&str] = &[
    "fs_game",
    "g_compassShowEnemies",
    "g_gametype",
    "gamename",
    "mapname",
    "protocol",
    "shortversion",
    "sv_allowAnonymous",
    "sv_disableClientConsole",
    "sv_floodprotect",
    "sv_hostname",
    "sv_maxclients",
    "sv_maxPing",
    "sv_maxRate",
    "sv_minPing",
    "sv_privateClients",
    "sv_punkbuster",
    "sv_pure",
    "sv_voice",
    "ui_maxclients",
];

/// Configstring 1's dvars besides the pure lists, the fastfiles and
/// `sv_serverid` (VERIFIED live).
pub(super) const SYSTEMINFO: &[&str] = &[
    "bg_fallDamageMaxHeight",
    "bg_fallDamageMinHeight",
    "com_timescale",
    "con_gameMsgWindow0Filter",
    "con_gameMsgWindow1Filter",
    "con_gameMsgWindow2Filter",
    "g_synchronousClients",
    "sv_cheats",
    "sv_clientSideBullets",
    "sv_voice",
    "sv_voiceQuality",
    "timescale",
];

/// The dvar mirror (names 20..147, values 148..275) as `handshake-1.7.txt`
/// carries it on `war` (VERIFIED live). The engine's and the stock scripts'
/// values together: the gsc boot takes over the script's (the team names,
/// icons and colours, `ui_*`, `scr_*`).
pub(super) const MIRRORED: &[(&str, &str)] = &[
    ("bg_aimSpreadMoveSpeedThreshold", "11"),
    ("bg_bobAmplitudeDucked", "0.0075 0.0075"),
    ("bg_bobAmplitudeProne", "0.02 0.005"),
    ("bg_bobAmplitudeSprinting", "0.02 0.014"),
    ("bg_bobAmplitudeStanding", "0.007 0.007"),
    ("bg_bobMax", "8"),
    ("bg_fallDamageMaxHeight", "300"),
    ("bg_fallDamageMinHeight", "128"),
    ("bg_foliagesnd_fastinterval", "500"),
    ("bg_foliagesnd_maxspeed", "180"),
    ("bg_foliagesnd_minspeed", "40"),
    ("bg_foliagesnd_resetinterval", "500"),
    ("bg_foliagesnd_slowinterval", "1500"),
    ("bg_ladder_yawcap", "100"),
    ("bg_maxGrenadeIndicatorSpeed", "20"),
    ("bg_prone_yawcap", "85"),
    ("friction", "5.5"),
    ("g_ScoresColor_Allies", "0.6 0.639216 0.690196 0"),
    ("g_ScoresColor_Axis", "0.65098 0.568627 0.411765 0"),
    ("g_ScoresColor_EnemyTeam", "0.690196 0.0705882 0.0509804 1"),
    ("g_ScoresColor_Free", "0.760784 0.780392 0.101961 0"),
    ("g_ScoresColor_MyTeam", "0.25098 0.721569 0.25098 1"),
    ("g_ScoresColor_Spectator", "0.25098 0.25098 0.25098 0"),
    ("g_TeamColor_Allies", "0.6 0.639216 0.690196 0"),
    ("g_TeamColor_Axis", "0.65098 0.568627 0.411765 0"),
    ("g_TeamColor_EnemyTeam", "1 0.45098 0.501961 0"),
    ("g_TeamColor_Free", "0.74902 0.25098 0.25098 1"),
    ("g_TeamColor_MyTeam", "0.6 0.8 0.6 0"),
    ("g_TeamColor_Spectator", "0.25098 0.25098 0.25098 1"),
    ("g_TeamIcon_Allies", "faction_128_usmc"),
    ("g_TeamIcon_Axis", "faction_128_arab"),
    ("g_TeamIcon_Free", ""),
    ("g_TeamIcon_Spectator", ""),
    ("g_TeamName_Allies", "MPUI_MARINES_SHORT"),
    ("g_TeamName_Axis", "MPUI_OPFOR_SHORT"),
    ("inertiaAngle", "0"),
    ("inertiaDebug", "0"),
    ("inertiaMax", "50"),
    ("jump_height", "39"),
    ("jump_ladderPushVel", "128"),
    ("jump_slowdownEnable", "1"),
    ("jump_spreadAdd", "64"),
    ("jump_stepSize", "18"),
    ("mantle_check_angle", "60"),
    ("mantle_check_radius", "0.1"),
    ("mantle_check_range", "20"),
    ("mantle_debug", "0"),
    ("mantle_enable", "1"),
    ("mantle_view_yawcap", "60"),
    ("player_adsExitDelay", "0"),
    ("player_backSpeedScale", "0.7"),
    ("player_breath_fire_delay", "0"),
    ("player_breath_gasp_lerp", "6"),
    ("player_breath_gasp_scale", "4.5"),
    ("player_breath_gasp_time", "1"),
    ("player_breath_hold_lerp", "1"),
    ("player_breath_hold_time", "4.5"),
    ("player_breath_snd_delay", "1"),
    ("player_breath_snd_lerp", "2"),
    ("player_debugHealth", "0"),
    ("player_dmgtimer_flinchTime", "500"),
    ("player_dmgtimer_maxTime", "750"),
    ("player_dmgtimer_minScale", "0"),
    ("player_dmgtimer_stumbleTime", "500"),
    ("player_dmgtimer_timePerPoint", "100"),
    ("player_footstepsThreshhold", "0"),
    ("player_lean_rotate_crouch_left", "0.8"),
    ("player_lean_rotate_crouch_right", "0.3"),
    ("player_lean_rotate_left", "0.8"),
    ("player_lean_rotate_right", "0.4"),
    ("player_lean_shift_crouch_left", "4"),
    ("player_lean_shift_crouch_right", "15"),
    ("player_lean_shift_left", "5"),
    ("player_lean_shift_right", "9"),
    ("player_moveThreshhold", "10"),
    ("player_scopeExitOnDamage", "0"),
    ("player_spectateSpeedScale", "1"),
    ("player_sprintCameraBob", "0.5"),
    ("player_sprintForwardMinimum", "105"),
    ("player_sprintMinTime", "1"),
    ("player_sprintRechargePause", "0"),
    ("player_sprintSpeedScale", "1.5"),
    ("player_sprintStrafeSpeedScale", "0.667"),
    ("player_sprintTime", "4"),
    ("player_strafeAnimCosAngle", "0.5"),
    ("player_strafeSpeedScale", "0.8"),
    ("player_turnAnims", "0"),
    ("player_view_pitch_down", "85"),
    ("player_view_pitch_up", "85"),
    ("scr_allies", "usmc"),
    ("scr_axis", "arab"),
    ("scr_oldschool", "0"),
    ("stopspeed", "100"),
    ("ui_allow_classchange", ""),
    ("ui_allow_teamchange", ""),
    ("ui_allowvote", "1"),
    ("ui_bomb_timer", "0"),
    ("ui_friendlyfire", "0"),
    ("ui_hostname", "CoD4Host"),
    ("ui_hud_hardcore", "0"),
    ("ui_motd", ""),
    ("ui_scorelimit", "750"),
    ("ui_timelimit", "10"),
    ("ui_uav_allies", "0"),
    ("ui_uav_axis", "0"),
];

/// The line `SV_ConnectionlessPacket` tokenizes: up to a NUL or newline.
fn line(payload: &[u8]) -> String {
    payload
        .iter()
        .take_while(|&&b| b != 0 && b != b'\n')
        .take(1023)
        .map(|&b| char::from(b))
        .collect()
}

impl Cod4Server {
    pub(super) fn connectionless(&mut self, from: SocketAddr, payload: &[u8], now: Instant) {
        let text = line(payload);
        let args = crate::game::say::tokenize(&text);
        let Some(cmd) = args.first() else {
            return;
        };
        let arg = |i: usize| args.get(i).map_or("", String::as_str);
        match cmd.to_ascii_lowercase().as_str() {
            "getstatus" => {
                crate::engine::answer_status(self, from, crate::engine::StatusQuery::Status, arg(1))
            }
            "getinfo" => {
                crate::engine::answer_status(self, from, crate::engine::StatusQuery::Info, arg(1))
            }
            "getchallenge" => self.get_challenge(from, arg(2)),
            "connect" => self.direct_connect(from, arg(1), now),
            "stats" => self.receive_stats(from, &payload[text.len().min(payload.len())..]),
            "rcon" => self.remote_command(from, &args, now),
            // `v\0` and the binary rest (0x81776e9).
            "v" => self.voice_packet(from, payload.get(2..).unwrap_or(&[])),
            other => log::debug!("cod4: {from}: unhandled oob {other:?}"),
        }
    }

    /// `SV_GetChallenge` (0x816f05a): the address's challenge slot, the
    /// hex's bans (`ban.txt`, then the temporary ones, each refusal clearing
    /// the slot), then `challengeResponse`. Off the LAN retail asks the
    /// authorize server first and answers only on its `accept` or after it
    /// has been silent 20 min (cod4-handshake.md 15); vcod skips that round
    /// trip and answers every address as retail answers the LAN. The hex
    /// is kept as the GUID the connect takes.
    fn get_challenge(&mut self, from: SocketAddr, guid: &str) {
        let svs = self.core.sv_time_ms;
        // Two draws off the process's `rand()` (0x816f394), the level's
        // stream once one is up (cod4-combat.md 10.9).
        let level = self.game.level.as_mut().map(|l| l.rt.host.cod4_mut());
        let mut draw: Box<dyn FnMut() -> i32> = match level {
            Some(c4) => Box::new(move || c4.rand()),
            None => {
                let rng = &mut self.core.rng;
                Box::new(move || crate::engine::core::rand_from(rng))
            }
        };
        let challenge = self
            .core
            .challenges
            .issue(from, svs, || (draw() << 16) ^ draw() ^ svs);
        drop(draw);
        let kind = if self.core.bans.lists_guid(guid) {
            "permanently"
        } else if self.temp_banned(guid) {
            "temporarily"
        } else {
            self.game.guids.insert(from, guid.to_string());
            self.send_oob(from, &format!("challengeResponse {challenge}"));
            return;
        };
        // 0x816f3f8 and 0x816f4db: the line, the OOB, the slot zeroed.
        log::info!("rejected connection from {kind} banned GUID \"{guid}\"");
        self.send_oob(
            from,
            &format!("error\n\u{15}You are {kind} banned from this server"),
        );
        self.core.challenges.forget(from);
    }

    /// `SV_DirectConnect` on a plain-text `connect "<userinfo>"`: the
    /// protocol (`EXE_SERVER_IS_DIFFERENT_VER\x151.7`, VERIFIED live), the
    /// reconnect limit, the challenge, a slot, `g_password`, then
    /// `connectResponse ` with `fs_game` after the space (VERIFIED live).
    fn direct_connect(&mut self, from: SocketAddr, userinfo: &str, _now: Instant) {
        let svs = self.core.sv_time_ms;
        let key = |k: &str| vcod_common::net::info_value_for_key(userinfo, k).unwrap_or("");
        let atoi = crate::game::builtins::cvar::atoi;
        if atoi(key("protocol")) != PROTOCOL as i32 {
            self.send_oob(from, "error\nEXE_SERVER_IS_DIFFERENT_VER\u{15}1.7\n");
            return;
        }
        let challenge = atoi(key("challenge"));
        let qport = atoi(key("qport")) as u16;
        let same = |c: &super::Client| {
            c.link.addr.ip() == from.ip()
                && (c.link.netchan.qport == qport || c.link.addr.port() == from.port())
        };
        let limit = self.int("sv_reconnectlimit") * 1000;
        if self
            .core
            .clients
            .iter()
            .flatten()
            .find(|c| same(c))
            .is_some_and(|c| svs - c.last_connect_ms < limit)
        {
            log::debug!("cod4: {from}: reconnect rejected: too soon");
            return;
        }
        let Some((ci, ping)) = self.core.challenges.connect(from, challenge, svs) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        log::info!("Client {ci} connecting with {ping} challenge ping from {from}");
        // `sv_minPing` and `sv_maxPing` hold off-LAN clients only
        // (0x8171847..0x8171a1e), past the slot's `connected` mark.
        if !link::Lan::Cod2.is_lan(from.ip()) {
            let (min, max) = (self.int("sv_minPing"), self.int("sv_maxPing"));
            if min != 0 && ping < min {
                self.send_oob(from, "error\nEXE_ERR_HIGH_PING_ONLY");
                return;
            }
            if max != 0 && ping > max {
                self.send_oob(from, "error\nEXE_ERR_LOW_PING_ONLY");
                return;
            }
        }
        let slot = match self
            .core
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(same))
        {
            // A reconnect into a live slot frees the old client first
            // (`SV_FreeClient`, so `ClientDisconnect`); a zombie's ran at
            // its drop (cod4-handshake.md, "Reconnects").
            Some(slot) => {
                if self.core.clients[slot]
                    .as_ref()
                    .is_some_and(|c| c.state > State::Zombie)
                    && let Some(level) = self.game.level.as_mut()
                {
                    level.disconnect(slot);
                }
                slot
            }
            None => match self.core.clients.iter().position(Option::is_none) {
                Some(slot) => slot,
                None => {
                    self.send_oob(from, "error\nEXE_SERVERISFULL");
                    return;
                }
            },
        };
        let gp = self.game.cvars().get("g_password");
        if !gp.is_empty() && gp != key("password") {
            self.send_oob(from, "error\nGAME_INVALIDPASSWORD");
            return;
        }
        // The bans were read at `getchallenge`; the connect reads none.
        let guid = self.game.guids.get(&from).cloned().unwrap_or_default();
        self.admit(slot, from, qport, challenge, guid.clone(), userinfo);
        self.core.challenges.taken(ci);
        log::info!(
            "cod4: {from} connected as client {slot} ({}, guid {guid:?})",
            key("name")
        );
        let fs_game = self.game.cvars().get("fs_game").to_string();
        self.send_oob(from, &format!("connectResponse {fs_game}"));
    }

    /// `SV_ReceiveStats`: `[i16 qport][u8 index][data]`, 1240 bytes
    /// a packet into the 0x2000-byte block, each answered `statResponse` with
    /// the mask of the packets still missing (126 down to 0, VERIFIED live).
    /// `rest` starts at the NUL after `stats`.
    fn receive_stats(&mut self, from: SocketAddr, rest: &[u8]) {
        let Some(body) = rest.get(1..) else {
            return;
        };
        if body.len() < 3 {
            return;
        }
        let qport = u16::from_le_bytes([body[0], body[1]]);
        let idx = usize::from(body[2]);
        let data = &body[3..];
        let Some(slot) = self.slot_of(from, qport) else {
            log::warn!("Received stats packet from unknown remote client {from}");
            return;
        };
        if idx >= STAT_PACKETS {
            log::warn!("Invalid stat packet {idx} of stats data");
            return;
        }
        let start = idx * STAT_CHUNK;
        let len = STAT_CHUNK.min(STATS_BYTES - start);
        if data.len() < len {
            log::warn!("Truncated stat packet {idx} from client");
            return;
        }
        let svs = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        log::debug!("Received packet {idx} of stats data");
        c.stats.0[start..start + len].copy_from_slice(&data[..len]);
        c.stat_packets |= 1 << idx;
        c.link.last_packet_ms = svs;
        let left = !c.stat_packets & 0x7f;
        if left == 0
            && let Some(level) = self.game.level.as_mut()
        {
            level.set_stats(slot, &c.stats);
        }
        self.send_oob(from, &format!("statResponse {left}"));
    }

    /// `SVC_Info`, its keys in their order (VERIFIED live; 1.7's `hw` is
    /// 1).
    pub(super) fn svc_info(&mut self, challenge: &str) -> String {
        let int = |s: &Self, n: &str| s.int(n);
        let mut i = vcod_common::net::connectionless::Info::new();
        i.set("challenge", challenge);
        i.set("protocol", PROTOCOL);
        i.set("hostname", &self.game.cfg.hostname);
        i.set("mapname", &self.game.cfg.map);
        let clients = self.client_count();
        if clients > 0 {
            i.set("clients", clients);
        }
        i.set("sv_maxclients", self.core.clients.len());
        i.set("gametype", &self.game.cfg.gametype);
        if self.pure() {
            i.set("pure", 1);
        }
        for (key, cvar) in [("minPing", "sv_minPing"), ("maxPing", "sv_maxPing")] {
            let v = int(self, cvar);
            if v != 0 {
                i.set(key, v);
            }
        }
        i.set("game", self.game.cvars().get("fs_game"));
        for (key, cvar) in [
            ("sv_allowAnonymous", "sv_allowAnonymous"),
            ("con_disabled", "sv_disableClientConsole"),
        ] {
            let v = int(self, cvar);
            if v != 0 {
                i.set(key, v);
            }
        }
        if !self.game.cvars().get("g_password").is_empty() {
            i.set("pswrd", 1);
        }
        for (key, cvar) in [("ff", "scr_team_fftype"), ("kc", "scr_game_allowkillcam")] {
            let v = int(self, cvar);
            if v != 0 {
                i.set(key, v);
            }
        }
        i.set("hw", 1);
        i.set("mod", u8::from(self.modded()));
        i.set("voice", int(self, "sv_voice"));
        i.set("pb", int(self, "sv_punkbuster"));
        format!("infoResponse\n{i}")
    }

    /// `mod`: 1 off pure or with `fs_game`.
    fn modded(&self) -> bool {
        !self.pure() || !self.game.cvars().get("fs_game").is_empty()
    }

    /// `SVC_Status`: the serverinfo with `challenge`, `pswrd` and `mod`
    /// appended, then a `score ping "name"` line for each client past
    /// `CS_ZOMBIE`, the score `G_GetClientScore`'s (0x81764a2;
    /// cod4-handshake.md 14).
    pub(super) fn svc_status(&mut self, challenge: &str) -> String {
        let mut info = self.core.configstrings[0].clone();
        let mut add = |k: &str, v: String| {
            if !v.is_empty() {
                info.push_str(&format!("\\{k}\\{v}"));
            }
        };
        add("challenge", challenge.to_string());
        add(
            "pswrd",
            u8::from(!self.game.cvars().get("g_password").is_empty()).to_string(),
        );
        add("mod", u8::from(self.modded()).to_string());
        let mut text = format!("statusResponse\n{info}\n");
        for slot in 0..self.core.clients.len() {
            let score = self.client_score(slot);
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            if c.state >= State::Connected {
                let name = super::client_name(&c.link.userinfo);
                text.push_str(&format!("{score} {} \"{name}\"\n", c.link.ping));
            }
        }
        text
    }

    /// `SVC_RemoteCommand` (`cod4_lnxded` 0x817818a): `rcon_password`, the
    /// output as `print` packets, 1.7's refusals (cod4-handshake.md 14.2).
    fn remote_command(&mut self, from: SocketAddr, args: &[String], now: Instant) {
        let password = self.game.cvars().get("rcon_password").to_string();
        let mut out = Redirect::default();
        let verdict = self.core.rcon.check(&password, args, now);
        match verdict {
            Verdict::Limited => return,
            Verdict::NoPassword | Verdict::BadPassword => {
                out.print(verdict.login_refusal(args).unwrap_or_default())
            }
            Verdict::Run(None) => {}
            Verdict::Run(Some(line)) => self.exec(&line, &mut out),
        }
        // `killserver` ends the redirect with the server: nothing answers.
        if self.game.killed {
            return;
        }
        for text in out.finish() {
            self.send_oob(from, &text);
        }
    }

    /// The console: `status`, `serverinfo`, `systeminfo`, `clientkick`,
    /// the kicks and bans, `say`, `tell`, the map cycle (`cycle`), `quit`,
    /// `set` and a
    /// bare dvar name to read or write.
    pub(super) fn exec(&mut self, line: &str, out: &mut Redirect) {
        let args = crate::game::say::tokenize(line);
        let Some(cmd) = args.first() else {
            return;
        };
        let arg = |i: usize| args.get(i).map_or("", String::as_str);
        match cmd.to_ascii_lowercase().as_str() {
            // `SV_Status_f` (0x816c658): the separator line, the score
            // (`G_GetClientScore`, 0x80b384e), the ping capped at 9999,
            // `lastmsg` as `svs.time` less `lastPacketTime` (0x816c824),
            // the GUID right-aligned, the name closed by `^7` and padded
            // to 16 by its raw length, the address padded to 22
            // (cod4-handshake.md 14).
            "status" => {
                out.print(&format!("map: {}\n", self.game.cfg.map));
                out.print("num score ping guid                             name            lastmsg address               qport rate\n");
                out.print("--- ----- ---- -------------------------------- --------------- ------- --------------------- ----- -----\n");
                for slot in 0..self.core.clients.len() {
                    let score = self.client_score(slot);
                    let Some(c) = self.core.clients[slot].as_ref() else {
                        continue;
                    };
                    let ping = match c.state {
                        State::Connected => "CNCT ".to_string(),
                        State::Zombie => "ZMBI ".to_string(),
                        _ => format!("{:4} ", c.link.ping.min(9999)),
                    };
                    let name = super::client_name(&c.link.userinfo);
                    let line = format!(
                        "{slot:3} {:5} {ping}{:>32} {name}^7{:pad$}{:7} {:<22}{:5} {:5}\n",
                        score,
                        c.guid,
                        "",
                        self.core.sv_time_ms - c.link.last_packet_ms,
                        c.link.addr.to_string(),
                        c.link.netchan.qport,
                        c.rates.rate,
                        pad = 16usize.saturating_sub(name.len()),
                    );
                    out.print(&line);
                }
            }
            "serverinfo" | "systeminfo" => {
                let (info, head) = if cmd.eq_ignore_ascii_case("serverinfo") {
                    (&self.core.configstrings[0], "Server")
                } else {
                    (&self.core.configstrings[1], "System")
                };
                out.print(&format!("{head} info settings:\n"));
                for (k, v) in vcod_common::net::connectionless::info_pairs(info) {
                    out.print(&format!("{k:<20}{v}\n"));
                }
            }
            "clientkick" => {
                let n = crate::game::builtins::cvar::atoi(arg(1));
                match usize::try_from(n)
                    .ok()
                    .filter(|&s| self.core.clients.get(s).is_some_and(Option::is_some))
                {
                    Some(slot) => self.drop_client(slot, "EXE_PLAYERKICKED"),
                    None => out.print(&format!("Client {n} is not active\n")),
                }
            }
            "map" | "devmap" => {
                let devmap = cmd.eq_ignore_ascii_case("devmap");
                if let Err(e) = self.map_command(arg(1), devmap, out) {
                    log::error!("cod4: {cmd} {}: {e:#}", arg(1));
                }
            }
            "map_restart" | "fast_restart" => {
                if let Err(e) = self.map_restart(cmd.eq_ignore_ascii_case("fast_restart")) {
                    log::error!("cod4: {cmd}: {e:#}");
                }
            }
            "map_rotate" => {
                if let Err(e) = self.map_rotate(out) {
                    log::error!("cod4: map_rotate: {e:#}");
                }
            }
            "tempbanclient" | "tempbanuser" | "kick" | "onlykick" => {
                self.kick_command(cmd, &args, out);
            }
            // `SV_DumpUser_f` (0x816cc0a): `userinfo`, a rule, then the
            // pairs as `Info_Print` lays them (VERIFIED live).
            "dumpuser" => {
                if args.len() != 2 {
                    out.print("Usage: info <userid>\n");
                } else if let Some(slot) = self.find_client(true, arg(1), out) {
                    let info = self.core.clients[slot]
                        .as_ref()
                        .unwrap()
                        .link
                        .userinfo
                        .clone();
                    out.print("userinfo\n--------\n");
                    for (k, v) in vcod_common::net::connectionless::info_pairs(&info) {
                        out.print(&format!("{k:<20}{v}\n"));
                    }
                }
            }
            // `SV_Heartbeat_f` (0x816c180): the next heartbeat is due at
            // once (`svs.nextHeartbeatTime` to INT_MIN).
            "heartbeat" => self.game.last_beat_ms = None,
            // `SV_KillServer_f` (0x816c2f2): `SV_Shutdown("EXE_SERVERKILLED")`.
            "killserver" => self.kill_server("EXE_SERVERKILLED"),
            // `SV_ConSay_f` (0x816d328) and `SV_ConTell_f` (0x816cca6): the
            // arguments joined, one pair of quotes around them cut, as the
            // chat line `h "\x15console: <text>"` (`%c "\x15%s"`,
            // 0x82300c7) to everyone, or to one client in the world.
            "say" if args.len() > 1 => {
                let text = args[1..].join(" ");
                let line = format!("h \"\u{15}console: {}\"", unquote(&text));
                self.send_to_all(&line);
            }
            "tell" if args.len() > 2 => {
                let n = crate::game::builtins::cvar::atoi(arg(1));
                if let Some(slot) = usize::try_from(n).ok().filter(|&s| {
                    self.core
                        .clients
                        .get(s)
                        .is_some_and(|c| c.as_ref().is_some_and(|c| c.state == State::Active))
                }) {
                    let text = args[2..].join(" ");
                    let line = format!("h \"\u{15}console: {}\"", unquote(&text));
                    self.send_server_command(slot, &line);
                }
            }
            "say" | "tell" => {}
            // `banUser` (0x816cbaa) and `banClient` (0x816d012) into
            // `SV_BanClient` (0x8171360).
            "banuser" | "banclient" => {
                let by_name = cmd.eq_ignore_ascii_case("banuser");
                if args.len() != 2 {
                    out.print(if by_name {
                        "Usage: banUser <player name>\n"
                    } else {
                        "Usage: banClient <client number>\n"
                    });
                } else if let Some(slot) = self.find_client(by_name, arg(1), out) {
                    self.ban_client(slot, out);
                }
            }
            // `unbanUser` into `SV_UnbanClient` (0x816ea9c).
            "unbanuser" => {
                if args.len() != 2 {
                    out.print("Usage: unban <client name>\n");
                } else {
                    let name = crate::game::say::clean_name(arg(1));
                    match self.core.bans.unban(&name) {
                        None => {}
                        Some(0) => out.print(&format!("no banned user has name {name}\n")),
                        Some(n) => out.print(&format!("unbanned {n} user(s) named {name}\n")),
                    }
                }
            }
            // vcod's bot management (`server/bot_admin.rs`, docs/bots.md).
            "addbot" => crate::server::bot_admin::add_bot_command(self, &args[1..], out),
            "removebot" | "kickbot" => {
                let arg = (args.len() > 1).then(|| args[1..].join(" "));
                crate::server::bot_admin::remove_bot_command(self, arg.as_deref(), out);
            }
            "listbots" => crate::server::bot_admin::list_bots(self, out),
            "quit" => {
                for slot in 0..self.core.clients.len() {
                    self.drop_client(slot, "EXE_SERVERQUIT");
                }
                self.shutdown_game_log();
                self.core.quit = true;
            }
            "set" | "seta" | "sets" | "setu" => {
                if args.len() >= 3 {
                    self.set_cvar(arg(1), &args[2..].join(" "));
                    // `seta` archives the dvar, a registered one too
                    // (VERIFIED live: `seta g_inactivity 5` reached the
                    // next `writeconfig`).
                    if cmd.eq_ignore_ascii_case("seta") {
                        self.game
                            .cvars_mut()
                            .add_flags(arg(1), crate::cvars::flag::ARCHIVE);
                    }
                } else {
                    out.print(&format!(
                        "USAGE: {} <variable> <value>\n",
                        cmd.to_ascii_lowercase()
                    ));
                }
            }
            "toggle" | "togglep" => self.toggle(cmd.eq_ignore_ascii_case("togglep"), &args, out),
            "reset" => match self.game.cvars().lookup(arg(1)) {
                _ if args.len() != 2 => out.print("USAGE: reset <variable>\n"),
                Some((n, _, d)) => {
                    let (n, d) = (n.to_string(), d.to_string());
                    self.set_cvar(&n, &d);
                }
                None => {}
            },
            "setfromdvar" => match self.game.cvars().lookup(arg(2)) {
                _ if args.len() != 3 => out.print("USAGE: setfromdvar <dest_dvar> <source_dvar>\n"),
                Some((_, v, _)) => {
                    let v = v.to_string();
                    self.set_cvar(arg(1), &v);
                }
                None => out.print(&format!("dvar '{}' doesn't exist\n", arg(2))),
            },
            // `Cmd_Vstr_f`: the dvar's value run as console lines.
            "vstr" => {
                if args.len() != 2 {
                    out.print("vstr <variablename> : execute a variable command\n");
                } else {
                    let text = self.game.cvars().get(arg(1)).to_string();
                    for line in crate::console::split_commands(&text) {
                        self.exec(&line, out);
                    }
                }
            }
            // A console's `wait` holds the buffer a frame; one rcon line
            // has nothing after it to hold.
            "wait" => {}
            "exec" => self.exec_file(&args, out),
            "writeconfig" => self.write_config(&args, out),
            "dvar_bool" | "dvar_int" | "dvar_float" => self.dvar_typed(&args, out),
            // `setdvartotime` (file 0x1dbe71): the dvar set to
            // `Sys_Milliseconds`, the process's clock, which the server's
            // stands in for here.
            "setdvartotime" => {
                if args.len() == 2 {
                    let now = self.core.sv_time_ms.to_string();
                    self.set_cvar(arg(1), &now);
                } else {
                    out.print("USAGE: set <variable>\n");
                }
            }
            // `touchFile` marks a file referenced and prints nothing
            // (VERIFIED live: no file written).
            "touchfile" => {
                if args.len() != 2 {
                    out.print("Usage: touchFile <file>\n");
                }
            }
            "cmdlist" => {
                for name in COMMANDS {
                    out.print(&format!("{name}\n"));
                }
                out.print(&format!("{} commands\n", COMMANDS.len()));
            }
            name => match self.game.cvars().lookup(name) {
                // An int dvar refuses a value off its domain and keeps its
                // own (`sv_fps 5`, VERIFIED live, cod4-server-frame.md 8).
                Some((n, _, _)) if args.len() >= 2 => {
                    let value = args[1..].join(" ");
                    match self.game.domains.get(&n.to_ascii_lowercase()) {
                        Some(&dom @ Domain::Int(lo, hi))
                            if !(lo..=hi).contains(&crate::game::builtins::cvar::atoi(&value)) =>
                        {
                            out.print(&format!(
                                "'{value}' is not a valid value for dvar '{n}'\n  {}\n",
                                dom.describe()
                            ));
                        }
                        _ => self.set_cvar(name, &value),
                    }
                }
                // `Dvar_Command`'s print; a dvar a `set` made is a string
                // (`  Domain is any text`). VERIFIED live.
                Some((n, v, d)) => {
                    out.print(&format!("\"{n}\" is: \"{v}^7\" default: \"{d}^7\"\n"));
                    if let Some(dom) = self.game.domains.get(&n.to_ascii_lowercase()) {
                        out.print(&format!("  {}\n", dom.describe()));
                    } else if self.game.cvars().user_created(n) {
                        out.print("  Domain is any text\n");
                    }
                }
                None => log::info!("cod4: unknown command {line:?}"),
            },
        }
    }
}

impl Cod4Server {
    /// `kick`, `tempBanUser` (one handler, 0x816cb4a) and `onlykick`
    /// (0x816cb36), a player by name, and `tempBanClient` (by number):
    /// `SV_KickClient`'s drop with `EXE_PLAYERKICKED`. All but `onlykick`
    /// print `was kicked for cheating` and hold the GUID out for
    /// `sv_kickBanTime` seconds (0x816e808; VERIFIED live for `kick`);
    /// `kick all` drops everyone and bans nobody.
    fn kick_command(&mut self, cmd: &str, args: &[String], out: &mut Redirect) {
        let by_number = cmd.eq_ignore_ascii_case("tempbanclient");
        // `SV_KickUser_f`'s helper (0x816ca14): one argument, else the
        // usage; a name no player has but `all` drops every client.
        if !by_number && args.len() != 2 {
            out.print(&format!(
                "Usage: {cmd} <player name>\n{cmd} all = kick everyone\n"
            ));
            return;
        }
        let who = args.get(1).map_or("", String::as_str);
        let Some(slot) = self.find_client(!by_number, who, out) else {
            if !by_number && who.eq_ignore_ascii_case("all") {
                for slot in 0..self.core.clients.len() {
                    self.kick_slot(slot, out);
                }
            }
            return;
        };
        let c = self.core.clients[slot].as_ref().unwrap();
        let (name, guid) = (
            crate::game::say::clean_name(&super::client_name(&c.link.userinfo)),
            c.guid.clone(),
        );
        self.kick_slot(slot, out);
        if !cmd.eq_ignore_ascii_case("onlykick") {
            out.print(&format!(
                "{name} (guid \"{guid}\") was kicked for cheating\n"
            ));
            let now = self.core.sv_time_ms;
            self.game.temp_bans.retain(|(g, _)| *g != guid);
            if self.game.temp_bans.len() == 16 {
                self.game.temp_bans.remove(0);
            }
            self.game.temp_bans.push((guid, now));
        }
    }

    /// `G_GetClientScore` (0x80b384e): the script's `score` (`sess.score`),
    /// 0 for a slot with no entity.
    fn client_score(&mut self, slot: usize) -> i32 {
        self.game
            .level
            .as_mut()
            .and_then(|l| l.rt.client_field(slot, "score"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// `SV_GetPlayerByName` (0x816c1b6), a name as sent or cleaned of
    /// colours, case folded; or by number, a client past `CS_ZOMBIE`. A
    /// miss prints why.
    fn find_client(&self, by_name: bool, who: &str, out: &mut Redirect) -> Option<usize> {
        let found = if by_name {
            (0..self.core.clients.len()).find(|&s| {
                self.core.clients[s].as_ref().is_some_and(|c| {
                    let name = super::client_name(&c.link.userinfo);
                    c.state > State::Zombie
                        && (name.eq_ignore_ascii_case(who)
                            || crate::game::say::clean_name(&name).eq_ignore_ascii_case(who))
                })
            })
        } else {
            let n = crate::game::builtins::cvar::atoi(who);
            usize::try_from(n).ok().filter(|&s| {
                self.core
                    .clients
                    .get(s)
                    .is_some_and(|c| c.as_ref().is_some_and(|c| c.state > State::Zombie))
            })
        };
        if found.is_none() {
            match by_name {
                false => out.print(&format!("Bad client slot: {who}\n")),
                true => out.print(&format!("Player {who} is not on the server\n")),
            }
        }
        found
    }

    /// `SV_BanClient` (0x8171360): a client with no GUID, or one already
    /// in `ban.txt`, is refused with a print; otherwise `%s %s\r\n` (GUID,
    /// cleaned name) is appended and the client kicked.
    fn ban_client(&mut self, slot: usize, out: &mut Redirect) {
        let Some(c) = self.core.clients[slot].as_ref() else {
            return;
        };
        let (guid, name) = (
            c.guid.clone(),
            crate::game::say::clean_name(&super::client_name(&c.link.userinfo)),
        );
        if guid.is_empty() {
            out.print("Can't ban user, GUID is unknown\n");
        } else if self.core.bans.lists_guid(&guid) {
            out.print(&format!("This GUID ({guid}) is already banned\n"));
        } else {
            self.core.bans.append_line(&guid, &name);
            self.kick_slot(slot, out);
        }
    }

    /// The kick helper's drop (0x816c96e): `SV_DropClient` with
    /// `EXE_PLAYERKICKED`, whose `<slot>:<name> <reason>` line reaches the
    /// redirect; a zombie prints and drops nothing (cod4-handshake.md
    /// 14.1).
    fn kick_slot(&mut self, slot: usize, out: &mut Redirect) {
        let Some(c) = self.core.clients[slot].as_ref() else {
            return;
        };
        if c.state == State::Zombie {
            return;
        }
        out.print(&format!(
            "{slot}:{} EXE_PLAYERKICKED\n",
            super::client_name(&c.link.userinfo)
        ));
        self.drop_client(slot, "EXE_PLAYERKICKED");
    }

    /// `SV_IsTempBannedGuid`: a GUID a ban named within
    /// `sv_kickBanTime` seconds.
    pub(super) fn temp_banned(&self, guid: &str) -> bool {
        let secs = crate::game::builtins::cvar::atof(self.game.cvars().get("sv_kickBanTime"));
        let now = self.core.sv_time_ms;
        !guid.is_empty()
            && self
                .game
                .temp_bans
                .iter()
                .any(|(g, t)| g == guid && f64::from(secs) * 1000.0 >= f64::from(now - t))
    }
}

impl Cod4Server {
    /// `Dvar_Toggle_f` (`toggle`, `togglep`): a 0/1 dvar flips, one with a
    /// value list steps to the value after its own (the first when its own
    /// is not listed); `togglep` prints the new value (VERIFIED live).
    fn toggle(&mut self, print: bool, args: &[String], out: &mut Redirect) {
        if args.len() < 2 {
            out.print(&format!(
                "USAGE: {} <variable> <optional value sequence>\n",
                args[0].to_ascii_lowercase()
            ));
            return;
        }
        let Some((name, value, default)) = self.game.cvars().lookup(&args[1]) else {
            out.print(&format!("toggle failed: dvar '{}' not found.\n", args[1]));
            return;
        };
        let (name, value) = (name.to_string(), value.to_string());
        let boolean = |v: &str| v == "0" || v == "1";
        let next = if args.len() > 2 {
            let list = &args[2..];
            let at = list.iter().position(|v| *v == value);
            list[at.map_or(0, |i| (i + 1) % list.len())].clone()
        } else if boolean(&value) && boolean(default) {
            if value == "0" { "1" } else { "0" }.to_string()
        } else {
            out.print(&format!(
                "'toggle' with no arguments makes no sense for dvar '{name}'\n"
            ));
            return;
        };
        self.set_cvar(&name, &next);
        if print {
            out.print(&format!("{name} toggled to {next}\n"));
        }
    }

    /// `Com_WriteConfig_f` (`writeconfig`, file 0x1db56b): `<name>`
    /// (`.cfg` added when it has no extension) under the home path's
    /// `players/`, the header, a `seta` line per archived dvar (its latched
    /// value when one waits) and the console channel line (VERIFIED live:
    /// `players/vtest.cfg`, `players/sub/vw3.cfg`).
    fn write_config(&mut self, args: &[String], out: &mut Redirect) {
        if args.len() != 2 {
            out.print("Usage: writeconfig <filename>\n");
            return;
        }
        let name = default_cfg(&args[1]);
        out.print(&format!("Writing {name}.\n"));
        let home = self.game.cvars().get("fs_homepath").to_string();
        if home.is_empty() {
            return;
        }
        let path = std::path::Path::new(&home).join("players").join(&name);
        let text = self.config_text();
        let wrote = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&path, text));
        if wrote.is_err() {
            out.print(&format!("Couldn't write {name}.\n"));
        }
    }

    /// The file `writeconfig` writes.
    pub(super) fn config_text(&self) -> String {
        let mut text = CONFIG_HEADER.to_string();
        for (name, value) in self.game.cvars().flagged(crate::cvars::flag::ARCHIVE) {
            text.push_str(&format!("seta {name} \"{value}\"\n"));
        }
        text.push_str(CONFIG_CHANNELS);
        text
    }

    /// `dvar_bool <name> <default>`, `dvar_int` and `dvar_float <name>
    /// <default> <min> <max>`: a dvar of that domain, made at the default
    /// when it does not exist (VERIFIED live: each query read the default
    /// and the domain).
    fn dvar_typed(&mut self, args: &[String], out: &mut Redirect) {
        let cmd = args[0].to_ascii_lowercase();
        let num = |i: usize| crate::game::builtins::cvar::atof(&args[i]);
        let domain = match (cmd.as_str(), args.len()) {
            ("dvar_bool", 3) => Domain::Bool,
            ("dvar_int", 5) => Domain::Int(num(3) as i32, num(4) as i32),
            ("dvar_float", 5) => Domain::Float(num(3), num(4)),
            ("dvar_bool", _) => {
                out.print(&format!("USAGE: {cmd} <name> <default>\n"));
                return;
            }
            _ => {
                out.print(&format!("USAGE: {cmd} <name> <default> <min> <max>\n"));
                return;
            }
        };
        if self.game.cvars().lookup(&args[1]).is_none() {
            self.game.cvars_mut().register(&args[1], &args[2]);
            self.write_engine_configstrings();
        }
        self.game
            .domains
            .insert(args[1].to_ascii_lowercase(), domain);
    }

    /// `Cmd_Exec_f` (0x81121a2): `<name>` (`.cfg` added when it has no
    /// extension). `config_mp.cfg` comes off the disk only and a miss says
    /// nothing; any other name is first a rawfile of the loaded zones
    /// (`useFastFile`, `execing %s from fastfile`), then the search path
    /// ([`Self::read_disk`], `execing %s from disk`); a miss prints the
    /// argument as typed (VERIFIED live, cod4-handshake.md 14.1).
    fn exec_file(&mut self, args: &[String], out: &mut Redirect) {
        if args.len() != 2 {
            out.print("exec <filename> : execute a script file\n");
            return;
        }
        let name = default_cfg(&args[1]);
        let disk_only = name.eq_ignore_ascii_case("config_mp.cfg");
        let text = match self.rawfile(&name).filter(|_| !disk_only) {
            Some(raw) => {
                out.print(&format!("execing {name} from fastfile\n"));
                raw
            }
            None => match self.read_disk(&name) {
                Some(raw) => {
                    out.print(&format!("execing {name} from disk\n"));
                    raw
                }
                None if disk_only => return,
                None => {
                    out.print(&format!("^1Error: couldn't exec {}\n", args[1]));
                    return;
                }
            },
        };
        let text = String::from_utf8_lossy(&text).into_owned();
        for line in text.lines().flat_map(crate::console::split_commands) {
            self.exec(&line, out);
        }
    }

    /// A rawfile of the loaded zones by name, the zone loaded last first.
    fn rawfile(&self, name: &str) -> Option<Vec<u8>> {
        self.game.zones.iter().rev().find_map(|z| {
            z.raw_files
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(name))
                .map(|r| r.data.clone())
        })
    }

    /// `FS_ReadFile` off 1.7's search path (`path`, VERIFIED live): the
    /// `fs_game` directory then `main/`, each under the home path then the
    /// base path, each directory's iwds ahead of its loose files, the
    /// `localized_*` iwds last.
    fn read_disk(&self, name: &str) -> Option<Vec<u8>> {
        let cv = self.game.cvars();
        let mut roots: Vec<&str> = ["fs_homepath", "fs_basepath"]
            .iter()
            .map(|d| cv.get(d))
            .filter(|d| !d.is_empty())
            .collect();
        roots.dedup();
        let games: Vec<&str> = match cv.get("fs_game") {
            "" => vec!["main"],
            g => vec![g, "main"],
        };
        let dirs: Vec<std::path::PathBuf> = games
            .iter()
            .flat_map(|g| roots.iter().map(move |r| std::path::Path::new(r).join(g)))
            .collect();
        vcod_common::pk3::read_search(&dirs, vcod_common::pk3::ArchiveKind::Iwd, name)
    }

    /// `SV_Shutdown` (0x81743a8) for `killserver`: every client dropped
    /// with `reason` and sent it at once (`SV_FinalMessage`), the level shut
    /// down (`ShutdownGame`), and nothing answered after (VERIFIED live:
    /// neither rcon nor `getinfo`, the process still up).
    pub(super) fn kill_server(&mut self, reason: &str) {
        for slot in 0..self.core.clients.len() {
            self.drop_client(slot, reason);
        }
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot].as_ref().is_some_and(|c| !c.bot) {
                self.send_message(slot, &[], &Default::default());
            }
            self.core.clients[slot] = None;
        }
        self.shutdown_game_log();
        if let Some(mut old) = self.game.level.take() {
            self.game.cvars = std::mem::take(&mut old.rt.host.cvars);
        }
        self.game.killed = true;
    }
}

#[cfg(test)]
pub(super) const COMMANDS_LEN: usize = COMMANDS.len();

/// The commands `cmdlist` names: 1.7's (VERIFIED live, a dedicated
/// server's `cmdlist`, in its order) that the console here runs.
const COMMANDS: &[&str] = &[
    "cmdlist",
    "say",
    "vstr",
    "set",
    "reset",
    "toggle",
    "setu",
    "heartbeat",
    "dumpuser",
    "touchFile",
    "map",
    "tell",
    "killserver",
    "devmap",
    "map_rotate",
    "fast_restart",
    "map_restart",
    "systeminfo",
    "serverinfo",
    "status",
    "clientkick",
    "unbanUser",
    "tempBanClient",
    "tempBanUser",
    "kick",
    "banClient",
    "banUser",
    "onlykick",
    "writeconfig",
    "quit",
    "exec",
    "wait",
    "dvar_float",
    "dvar_int",
    "dvar_bool",
    "setdvartotime",
    "setfromdvar",
    "seta",
    "sets",
    "togglep",
];

/// `COM_DefaultExtension(name, ".cfg")`.
fn default_cfg(name: &str) -> String {
    if std::path::Path::new(name).extension().is_none() {
        format!("{name}.cfg")
    } else {
        name.to_string()
    }
}

/// The console chat's text: a leading quote cut with the last character.
fn unquote(text: &str) -> &str {
    match text.strip_prefix('"') {
        Some(rest) => {
            let mut end = rest.len();
            if let Some(c) = rest.chars().last() {
                end -= c.len_utf8();
            }
            &rest[..end]
        }
        None => text,
    }
}

/// `PROTOCOL_VERSION`, 6 for 1.6 and 1.7 alike (cod4-handshake.md 3).
const PROTOCOL: u32 = 6;
