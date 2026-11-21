//! CoD4 1.7's connectionless packets (docs/research/cod4-handshake.md 2,
//! 3): `getinfo`, `getstatus`, `getchallenge`, the plain-text `connect`,
//! the seven `stats` packets and `rcon`, and the console the last runs.

use std::net::SocketAddr;
use std::time::Instant;

use vcod_cod4::net::stats::{STAT_CHUNK, STAT_PACKETS, STATS_BYTES};

use super::{Cod4Server, State};
use crate::rcon::Redirect;

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
    // vcod heartbeats only at 2; retail beats at 1 too
    // (cod4-server-frame.md 5). An empty `sv_master1` beats master.cod.pm.
    ("dedicated", "1"),
    ("sv_master1", ""),
    // The timeout and ban defaults here are unverified on 1.7. `sv_timeout`,
    // `sv_zombietime`, `sv_reconnectlimit` and `sv_kickBanTime` are CoD2
    // 1.3's `SV_Init` values (cod2-protocol.md 17.1, 17.3, 17.7);
    // `sv_connectTimeout`'s 45 has no source in the docs.
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
        let cmd = cmd.to_ascii_lowercase();
        // vcod's own limiter (`engine::ratelimit`); retail answers every query.
        if self.core.limiter.limited(&cmd, from.ip(), now) {
            log::debug!("cod4: {from}: {cmd} rate limited");
            return;
        }
        match cmd.as_str() {
            "getstatus" => {
                crate::engine::answer_status(self, from, crate::engine::StatusQuery::Status, arg(1))
            }
            "getinfo" => {
                crate::engine::answer_status(self, from, crate::engine::StatusQuery::Info, arg(1))
            }
            "getchallenge" => self.get_challenge(from, arg(2)),
            "connect" => self.direct_connect(from, arg(1)),
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
    /// goes into the slot as the GUID the connect takes. With no authorize
    /// tying the hex to a key, an off-LAN address is banned beside it
    /// ([`address_guid`]).
    fn get_challenge(&mut self, from: SocketAddr, guid: &str) {
        if self.refuse_per_ip(from, true) {
            return;
        }
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
        let addr = address_guid(from);
        let kind = if self.core.bans.lists_guid(guid)
            || addr
                .as_deref()
                .is_some_and(|a| self.core.bans.lists_guid(a))
        {
            "permanently"
        } else if self.temp_banned(guid, addr.as_deref()) {
            "temporarily"
        } else {
            self.core.challenges.keep_guid(from, guid);
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
    /// engine's checks and slot ([`crate::engine::conn`]; the refusal
    /// `EXE_SERVER_IS_DIFFERENT_VER\x151.7`, VERIFIED live), the GUID the
    /// challenge kept (the connect reads no ban), `ClientConnect`, then
    /// `connectResponse ` with `fs_game` after the space (VERIFIED live).
    fn direct_connect(&mut self, from: SocketAddr, userinfo: &str) {
        let Some(a) = self.connect_slot(from, userinfo) else {
            return;
        };
        let guid = self.core.challenges.guid(a.ci).to_string();
        self.admit(a.slot, from, a.qport, a.challenge, guid.clone(), userinfo);
        self.core.challenges.taken(a.ci);
        let name = vcod_common::net::info_value_for_key(userinfo, "name").unwrap_or("");
        log::info!(
            "cod4: {from} connected as client {} ({name}, guid {guid:?})",
            a.slot
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
        // vcod takes the slot's own address only, and logs a miss at debug:
        // the line is retail's, but anyone can send the packet.
        let Some(slot) = crate::engine::conn::slot_of(&self.core, from, qport)
            .filter(|&s| self.core.clients[s].as_ref().unwrap().link.addr == from)
        else {
            log::debug!("Received stats packet from unknown remote client {from}");
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
        // Only a slot waiting for its gamestate (after `connect`, or after
        // a map change's `requeststats`) takes stats; any other answers
        // with its mask and keeps its block (vcod's, cod4-handshake.md 3).
        if c.state == State::Connected {
            log::debug!("Received packet {idx} of stats data");
            c.extra.stats.0[start..start + len].copy_from_slice(&data[..len]);
            c.extra.stat_packets |= 1 << idx;
            c.link.last_packet_ms = svs;
            if c.extra.stat_packets == 0x7f
                && let Some(level) = self.game.level.as_mut()
            {
                level.set_stats(slot, &c.extra.stats);
            }
        }
        let left = !c.extra.stat_packets & 0x7f;
        self.send_oob(from, &format!("statResponse {left}"));
    }

    /// `mod`: 1 off pure or with `fs_game`.
    pub(super) fn modded(&self) -> bool {
        !self.pure() || !self.game.cvars().get("fs_game").is_empty()
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
                        c.extra.guid,
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
            "serverinfo" | "systeminfo" | "dumpuser" | "tell" | "banuser" | "banclient"
            | "unbanuser" => {
                self.console_common(&args, out);
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
                let r = self.map_command(arg(1), devmap, out);
                self.load_failed(&format!("{cmd} {}", arg(1)), r);
            }
            "map_restart" | "fast_restart" => {
                let r = self.map_restart(cmd.eq_ignore_ascii_case("fast_restart"));
                self.load_failed(cmd, r);
            }
            "map_rotate" => {
                let r = self.map_rotate(out);
                self.load_failed("map_rotate", r);
            }
            "tempbanclient" | "tempbanuser" | "kick" | "onlykick" => {
                self.kick_command(cmd, &args, out);
            }
            // `SV_Heartbeat_f` (0x816c180): the next heartbeat is due at
            // once (`svs.nextHeartbeatTime` to INT_MIN).
            "heartbeat" => self.core.masters.force(),
            // `SV_KillServer_f` (0x816c2f2): `SV_Shutdown("EXE_SERVERKILLED")`.
            "killserver" => self.kill_server("EXE_SERVERKILLED"),
            // `SV_ConSay_f` (0x816d328) and `SV_ConTell_f` (0x816cca6): the
            // arguments joined, one pair of quotes around them cut, as the
            // chat line `h "\x15console: <text>"` (`%c "\x15%s"`,
            // 0x82300c7) to everyone, or to one client in the world.
            "say" if args.len() > 1 => {
                let line = format!(
                    "h \"\u{15}console: {}\"",
                    crate::engine::conn::console_args(&args[1..])
                );
                self.send_to_all(&line);
            }
            "say" => {}
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
                    if !self.console_set(arg(1), &args[2..].join(" "), out) {
                        return;
                    }
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
                    self.console_set(&n, &d, out);
                }
                None => {}
            },
            "setfromdvar" => match self.game.cvars().lookup(arg(2)) {
                _ if args.len() != 3 => out.print("USAGE: setfromdvar <dest_dvar> <source_dvar>\n"),
                Some((_, v, _)) => {
                    let v = v.to_string();
                    self.console_set(arg(1), &v, out);
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
                    self.console_set(arg(1), &now, out);
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
                        _ => {
                            self.console_set(name, &value, out);
                        }
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
        let (name, guid, addr) = (
            crate::game::say::clean_name(&super::client_name(&c.link.userinfo)),
            c.extra.guid.clone(),
            address_guid(c.link.addr),
        );
        self.kick_slot(slot, out);
        if !cmd.eq_ignore_ascii_case("onlykick") {
            out.print(&format!(
                "{name} (guid \"{guid}\") was kicked for cheating\n"
            ));
            let now = self.core.sv_time_ms;
            self.game
                .temp_bans
                .retain(|(g, a, _)| *g != guid || *a != addr);
            if self.game.temp_bans.len() == 16 {
                self.game.temp_bans.remove(0);
            }
            self.game.temp_bans.push((guid, addr, now));
        }
    }

    /// `G_GetClientScore` (0x80b384e): the script's `score` (`sess.score`),
    /// 0 for a slot with no entity.
    pub(super) fn client_score(&mut self, slot: usize) -> i32 {
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
    pub(super) fn find_client(
        &self,
        by_name: bool,
        who: &str,
        out: &mut Redirect,
    ) -> Option<usize> {
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
    /// cleaned name) is appended and the client kicked. vcod also appends
    /// an off-LAN client's [`address_guid`], so a client with no GUID can
    /// be banned and one with a new hex stays out.
    pub(super) fn ban_client(&mut self, slot: usize, out: &mut Redirect) {
        let Some(c) = self.core.clients[slot].as_ref() else {
            return;
        };
        let (guid, name, addr) = (
            c.extra.guid.clone(),
            crate::game::say::clean_name(&super::client_name(&c.link.userinfo)),
            address_guid(c.link.addr),
        );
        let keys: Vec<String> = [Some(guid).filter(|g| !g.is_empty()), addr]
            .into_iter()
            .flatten()
            .collect();
        if keys.is_empty() {
            out.print("Can't ban user, GUID is unknown\n");
        } else if keys.iter().all(|k| self.core.bans.lists_guid(k)) {
            out.print(&format!("This GUID ({}) is already banned\n", keys[0]));
        } else {
            for k in keys {
                if !self.core.bans.lists_guid(&k) {
                    self.core.bans.append_line(&k, &name);
                }
            }
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
    /// `sv_kickBanTime` seconds; vcod's address guid the same.
    pub(super) fn temp_banned(&self, guid: &str, addr: Option<&str>) -> bool {
        let secs = crate::game::builtins::cvar::atof(self.game.cvars().get("sv_kickBanTime"));
        let now = self.core.sv_time_ms;
        self.game.temp_bans.iter().any(|(g, a, t)| {
            let named = (!guid.is_empty() && g == guid) || (addr.is_some() && a.as_deref() == addr);
            named && f64::from(secs) * 1000.0 >= f64::from(now - t)
        })
    }
}

/// vcod's stand-in for the authorize server's tie of a GUID to a key: an
/// off-LAN client's IPv4 address as a signed integer, as CoD1 and CoD2 ban
/// ([`crate::engine::bans::guid_of`]); `None` on the LAN.
fn address_guid(addr: SocketAddr) -> Option<String> {
    match crate::engine::bans::guid_of(crate::engine::Lan::Cod2, addr) {
        0 => None,
        g => Some(g.to_string()),
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
        if !self.console_set(&name, &next, out) {
            return;
        }
        if print {
            out.print(&format!("{name} toggled to {next}\n"));
        }
    }

    /// A console write of a dvar: `set_cvar`, but a file root
    /// (`crate::cvars::FILE_ROOTS`) prints the dvar set's refusal
    /// (0x81a0653) and keeps its value. `false` when refused.
    fn console_set(&mut self, name: &str, value: &str, out: &mut Redirect) -> bool {
        if crate::cvars::is_file_root(name) {
            out.print(&format!("{name} is write protected.\n"));
            return false;
        }
        self.set_cvar(name, value);
        true
    }

    /// `Com_WriteConfig_f` (`writeconfig`, file 0x1db56b): `<name>`
    /// (`.cfg` added when it has no extension) under the home path's
    /// `players/`, the header, a `seta` line per archived dvar (its latched
    /// value when one waits) and the console channel line (VERIFIED live:
    /// `players/vtest.cfg`, `players/sub/vw3.cfg`). A name that would
    /// leave `players/` writes nothing (`confined_rel_path`).
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
        let Some(rel) = vcod_common::game_dir::confined_rel_path(&name) else {
            out.print(&format!("Couldn't write {name}.\n"));
            return;
        };
        let path = std::path::Path::new(&home).join("players").join(rel);
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

/// `PROTOCOL_VERSION`, 6 for 1.6 and 1.7 alike (cod4-handshake.md 3).
pub(super) const PROTOCOL: u32 = 6;

#[cfg(test)]
mod query_tests {
    use super::*;
    use crate::cod4::Cod4Config;
    use crate::engine::ratelimit::{ADDR_BURST, ADDR_PERIOD, GLOBAL_BURST, GLOBAL_PERIOD};

    fn bare() -> Cod4Server {
        let cfg = Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        };
        let mut sv = Cod4Server::new(cfg, 7);
        sv.load(None).unwrap();
        sv
    }

    /// The out-of-band replies `text` from `from` gets.
    fn ask(sv: &mut Cod4Server, from: SocketAddr, text: &str, now: Instant) -> Vec<String> {
        let mut pkt = vec![0xff; 4];
        pkt.extend_from_slice(text.as_bytes());
        sv.handle_packet(from, &pkt, now);
        sv.take_outgoing()
            .into_iter()
            .filter(|(to, _)| *to == from)
            .filter_map(|(_, p)| p.strip_prefix(&[0xff; 4][..]).map(<[u8]>::to_vec))
            .map(|t| String::from_utf8_lossy(&t).into_owned())
            .collect()
    }

    /// `engine::ratelimit`: one address's burst of status queries gets the
    /// address's burst of replies, then one a second; many addresses share
    /// the global bucket, which does not hold a challenge off.
    #[test]
    fn cod4_query_bursts_are_rate_limited() {
        let mut sv = bare();
        let now = Instant::now();
        let a = SocketAddr::from(([198, 51, 100, 2], 5000));
        let answered: usize = (0..ADDR_BURST + 5)
            .map(|i| {
                let q = if i % 2 == 0 {
                    "getstatus x"
                } else {
                    "GETINFO x"
                };
                ask(&mut sv, a, q, now).len()
            })
            .sum();
        assert_eq!(answered, ADDR_BURST as usize);
        assert_eq!(ask(&mut sv, a, "getinfo x", now + ADDR_PERIOD).len(), 1);
        assert_eq!(ask(&mut sv, a, "getinfo x", now + ADDR_PERIOD).len(), 0);

        let t = now + ADDR_PERIOD * 10;
        let answered: usize = (0..GLOBAL_BURST + 5)
            .map(|i| {
                ask(
                    &mut sv,
                    SocketAddr::from(([10, 1, 0, i as u8], 5)),
                    "getstatus",
                    t,
                )
                .len()
            })
            .sum();
        assert_eq!(answered, GLOBAL_BURST as usize);
        let joiner = SocketAddr::from(([10, 2, 0, 1], 5));
        let reply = ask(&mut sv, joiner, "getchallenge 0 \"ab\"", t);
        assert!(reply[0].starts_with("challengeResponse "), "{reply:?}");
        let late = SocketAddr::from(([10, 2, 0, 2], 5));
        assert_eq!(ask(&mut sv, late, "getinfo x", t + GLOBAL_PERIOD).len(), 1);
    }

    /// The `getchallenge` hex lives in the challenge slot, cut to 32 chars
    /// as retail's `Q_strncpyz(.., 0x21)` cuts it, and the connect takes it
    /// from there (cod4-handshake.md 15).
    #[test]
    fn cod4_the_connect_takes_the_slots_hex() {
        let mut sv = bare();
        let now = Instant::now();
        let from = SocketAddr::from(([127, 0, 0, 1], 4660));
        let hex = "0123456789abcdef".repeat(3);
        let reply = ask(&mut sv, from, &format!("getchallenge 0 \"{hex}\""), now);
        let challenge = reply[0].strip_prefix("challengeResponse ").unwrap();
        let ui = format!("\\protocol\\6\\challenge\\{challenge}\\qport\\1\\name\\a");
        let reply = ask(&mut sv, from, &format!("connect \"{ui}\""), now);
        assert!(reply[0].starts_with("connectResponse"), "{reply:?}");
        let client = sv.core.clients.iter().flatten().next().unwrap();
        assert_eq!(client.extra.guid, hex[..32]);
    }

    /// vcod's `vc_maxClientsPerIP` (4): one public address's fifth
    /// `getchallenge` and fifth `connect` are refused, its held sockets
    /// still get a challenge, another address still gets in.
    #[test]
    fn cod4_one_public_address_takes_at_most_four_slots() {
        let mut sv = bare();
        let now = Instant::now();
        let refused = ["error\n\u{15}Too many clients from your address"];
        let wan = |port: u16| SocketAddr::from(([198, 51, 100, 2], port));
        let challenge = |sv: &mut Cod4Server, from| {
            let reply = ask(sv, from, "getchallenge 0 \"ab\"", now);
            reply[0]
                .strip_prefix("challengeResponse ")
                .unwrap()
                .to_string()
        };
        let connect = |sv: &mut Cod4Server, from: SocketAddr, c: &str| {
            let ui = format!("\\protocol\\6\\challenge\\{c}\\qport\\{}", from.port());
            ask(sv, from, &format!("connect \"{ui}\""), now)
        };
        let challenges: Vec<String> = (0..5).map(|p| challenge(&mut sv, wan(p))).collect();
        for (p, c) in challenges.iter().enumerate().take(4) {
            let reply = connect(&mut sv, wan(p as u16), c);
            assert!(reply[0].starts_with("connectResponse"), "{reply:?}");
        }
        assert_eq!(connect(&mut sv, wan(4), &challenges[4]), refused);
        assert_eq!(ask(&mut sv, wan(9), "getchallenge 0 \"ab\"", now), refused);
        challenge(&mut sv, wan(0));
        let other = SocketAddr::from(([198, 51, 100, 3], 1));
        let c = challenge(&mut sv, other);
        assert!(connect(&mut sv, other, &c)[0].starts_with("connectResponse"));
    }
}
