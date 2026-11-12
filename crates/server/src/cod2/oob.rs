//! CoD2's connectionless packets (docs/research/cod2-protocol.md 1):
//! `getchallenge`, the plain-text `connect`, `getinfo`, `getstatus` and
//! `rcon`, and the console the last runs.

use std::net::SocketAddr;
use std::time::Instant;

use vcod_cod2::net::ops::PROTOCOL;

use super::{Cod2Server, State};
use crate::rcon::{Redirect, Verdict};
use crate::server::bot_admin;

/// The engine dvars the two info strings and `getinfo` read, at the
/// values a stock `cod2_lnxded` registers them with (9).
pub(super) const ENGINE_CVARS: &[(&str, &str)] = &[
    ("g_antilag", "1"),
    ("gamename", "Call of Duty 2"),
    ("protocol", "118"),
    ("shortversion", "1.3"),
    ("scr_friendlyfire", "0"),
    ("scr_killcam", "1"),
    ("sv_allowAnonymous", "0"),
    ("sv_floodProtect", "1"),
    ("sv_maxPing", "0"),
    ("sv_maxRate", "0"),
    ("sv_minPing", "0"),
    ("sv_privateClients", "0"),
    ("sv_punkbuster", "0"),
    // `SV_Init` registers it on (11).
    ("sv_pure", "1"),
    ("sv_voice", "0"),
    ("sv_voiceQuality", "1"),
    ("sv_disableClientConsole", "0"),
    ("sv_cheats", "0"),
    ("g_synchronousClients", "0"),
    ("bg_fallDamageMaxHeight", "480"),
    ("bg_fallDamageMinHeight", "256"),
    ("timescale", "1"),
    ("rcon_password", ""),
    ("g_password", ""),
    ("fs_game", ""),
    // Retail registers 2 (cod2-protocol.md 1.6); vcod heartbeats only when
    // asked, as on CoD1.
    ("dedicated", "1"),
    // Housekeeping (`SV_Init` 0x8093adc, cod2-protocol.md 17).
    ("sv_privatePassword", ""),
    ("sv_timeout", super::house::TIMEOUT_S),
    ("sv_zombietime", super::house::ZOMBIETIME_S),
    ("sv_reconnectlimit", super::house::RECONNECT_LIMIT_S),
    ("sv_allowDownload", "1"),
    ("sv_wwwDownload", "0"),
    ("sv_wwwBaseURL", ""),
    ("sv_wwwDlDisconnected", "0"),
    // `SV_Init`: 300 s, 0..3600 (`Dvar_RegisterFloat`, 17.7).
    ("sv_kickBanTime", "300"),
    // `G_InitGame` (Mac 0x1addca) registers the relay's rules and `isTalking`'s
    // window (cod2-voice.md 3).
    ("voice_global", "0"),
    ("voice_localEcho", "0"),
    ("voice_deadChat", "0"),
    ("g_voiceChatTalkingDuration", "500"),
    // Where `download` finds its files: the install, set by `main.rs`.
    ("fs_basepath", ""),
];

/// Configstring 0's dvars besides the map, gametype, hostname and slot
/// count the config holds: retail's capture (9).
pub(super) const SERVERINFO: &[&str] = &[
    "g_antilag",
    "gamename",
    "protocol",
    "scr_friendlyfire",
    "scr_killcam",
    "shortversion",
    "sv_allowAnonymous",
    "sv_floodProtect",
    "sv_maxPing",
    "sv_maxRate",
    "sv_minPing",
    "sv_privateClients",
    "sv_punkbuster",
    "sv_pure",
    "sv_voice",
];

/// CoD2's engine dvars in the configstring mirror (142..237 names,
/// 238..333 values; cod2-protocol.md 15.4) at their registered defaults:
/// the names and the values of the ones no stock script sets are the
/// mp_toujane capture's, VERIFIED live. Each is registered with flag 0x100
/// set (0x1180 in `BG_RegisterDvars` 0x80de830, 0x1100 in 0x81084a4),
/// VERIFIED. The `g_ScoresBanner_*`, `g_TeamName_*` and `g_TeamColor_*`
/// defaults are those registrations', which the gametypes' team setup
/// overwrites; the colours' printed form is INFERRED.
pub(super) const MIRRORED: &[(&str, &str)] = &[
    ("bg_aimSpreadMoveSpeedThreshold", "11"),
    ("bg_bobAmplitudeDucked", "0.0075"),
    ("bg_bobAmplitudeProne", "0.03"),
    ("bg_bobAmplitudeStanding", "0.007"),
    ("bg_bobMax", "8"),
    ("bg_fallDamageMaxHeight", "480"),
    ("bg_fallDamageMinHeight", "256"),
    ("bg_foliagesnd_fastinterval", "500"),
    ("bg_foliagesnd_maxspeed", "180"),
    ("bg_foliagesnd_minspeed", "40"),
    ("bg_foliagesnd_resetinterval", "500"),
    ("bg_foliagesnd_slowinterval", "1500"),
    ("bg_ladder_yawcap", "100"),
    ("bg_prone_yawcap", "85"),
    ("friction", "5.5"),
    ("g_ScoresBanner_Allies", "mpflag_american"),
    ("g_ScoresBanner_Axis", "mpflag_german"),
    ("g_ScoresBanner_None", "mpflag_none"),
    ("g_ScoresBanner_Spectators", "mpflag_spectator"),
    ("g_TeamColor_Allies", "0.5 0.5 1 1"),
    ("g_TeamColor_Axis", "1 0.5 0.5 1"),
    ("g_TeamName_Allies", "GAME_ALLIES"),
    ("g_TeamName_Axis", "GAME_AXIS"),
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
    ("player_breath_hold_lerp", "4"),
    ("player_breath_hold_time", "4.5"),
    ("player_breath_snd_delay", "1"),
    ("player_breath_snd_lerp", "2"),
    ("player_dmgtimer_flinchTime", "500"),
    ("player_dmgtimer_maxTime", "750"),
    ("player_dmgtimer_minScale", "0"),
    ("player_dmgtimer_stumbleTime", "500"),
    ("player_dmgtimer_timePerPoint", "100"),
    ("player_footstepsThreshhold", "0"),
    ("player_moveThreshhold", "10"),
    ("player_scopeExitOnDamage", "0"),
    ("player_spectateSpeedScale", "2"),
    ("player_strafeSpeedScale", "0.8"),
    ("player_toggleBinoculars", "1"),
    ("player_turnAnims", "0"),
    ("player_view_pitch_down", "85"),
    ("player_view_pitch_up", "85"),
    ("stopspeed", "100"),
];

/// Configstring 1's dvars besides the iwd lists and `sv_serverid` (9, 11.3).
pub(super) const SYSTEMINFO: &[&str] = &[
    "bg_fallDamageMaxHeight",
    "bg_fallDamageMinHeight",
    "g_synchronousClients",
    "sv_cheats",
    "sv_disableClientConsole",
    "sv_voice",
    "sv_voiceQuality",
    "timescale",
];

/// The line `SV_ConnectionlessPacket` tokenizes: up to a NUL or newline,
/// one char per byte.
fn line(payload: &[u8]) -> String {
    payload
        .iter()
        .take_while(|&&b| b != 0 && b != b'\n')
        .take(1023)
        .map(|&b| char::from(b))
        .collect()
}

impl Cod2Server {
    pub(super) fn connectionless(&mut self, from: SocketAddr, payload: &[u8], now: Instant) {
        // `v` and the binary rest past its line (`SV_VoicePacket`, 1.7).
        if let Some(body) = vcod_cod2::net::voice::oob_body(payload) {
            self.voice_packet(from, body);
            return;
        }
        let text = line(payload);
        let args = crate::game::say::tokenize(&text);
        self.game.argv1 = args.get(1).cloned().unwrap_or_default();
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
            "getchallenge" => self.get_challenge(from, now),
            "connect" => self.direct_connect(from, arg(1), now),
            "rcon" => self.remote_command(from, &args, now),
            // Matched and ignored (1).
            "disconnect" => {}
            other => log::debug!("cod2: {from}: unhandled oob {other:?}"),
        }
    }

    /// `SV_GetChallenge` (0x808d18e) on its LAN path: the slot for the
    /// address, else a new one (`engine::challenge`), answered at once. Off
    /// the LAN retail waits on the authorize server first (1.1, 1.3), which
    /// vcod does not ask.
    fn get_challenge(&mut self, from: SocketAddr, _now: Instant) {
        if self.refuse_banned(from) {
            return;
        }
        let (svs, xor) = (self.core.sv_time_ms, self.svs_time_ms());
        let rng = &mut self.core.rng;
        // Retail's process has one `rand()`: a level's challenge draws come
        // off the game's stream (cod2-combat.md 9.6).
        let game = self.game.level.as_ref().map(|l| l.combat.rand.clone());
        let challenge = self.core.challenges.issue(from, svs, || match &game {
            Some(r) => ((r.rand() << 16) ^ r.rand()) as i32 ^ xor,
            None => {
                (crate::engine::core::rand_from(rng) << 16)
                    ^ crate::engine::core::rand_from(rng)
                    ^ xor
            }
        });
        self.send_oob(from, &format!("challengeResponse {challenge}"));
    }

    /// `SV_AuthorizeIpPacket`'s ban checks at the `accept` (0x808da2a):
    /// `ban.txt`, then the temporary table, each refusal printed and sent,
    /// the challenge cleared. vcod runs them at `getchallenge`, which it
    /// answers with no authorize detour, on the stand-in guid
    /// ([`crate::engine::bans::guid_of`]): a LAN client's 0 is never refused.
    fn refuse_banned(&mut self, from: SocketAddr) -> bool {
        let guid = crate::engine::bans::guid_of(crate::engine::Lan::Cod2, from);
        let kind = if self.core.bans.is_banned(guid) {
            "permanently"
        } else if self.core.temp_bans.banned(
            guid,
            self.core.sv_time_ms,
            crate::game::builtins::cvar::atof(self.game.cvars.get("sv_kickBanTime")),
        ) {
            "temporarily"
        } else {
            return false;
        };
        log::info!("rejected connection from {kind} banned GUID {guid}");
        self.core.challenges.forget(from);
        self.send_oob(
            from,
            &format!("error\n\u{15}You are {kind} banned from this server"),
        );
        true
    }

    /// `SV_DirectConnect` (0x808e2aa) on a plain-text `connect "<userinfo>"`
    /// (1.2, 17.1): the protocol, the reconnect limit, the challenge and its
    /// ping off the LAN, the slot (the private ones for `sv_privatePassword`),
    /// then `ClientConnect`'s `g_password`.
    fn direct_connect(&mut self, from: SocketAddr, userinfo: &str, now: Instant) {
        let svs = self.core.sv_time_ms;
        let key = |k: &str| vcod_common::net::info_value_for_key(userinfo, k).unwrap_or("");
        let atoi = crate::game::builtins::cvar::atoi;
        if atoi(key("protocol")) != PROTOCOL {
            self.send_oob(from, "error\nEXE_SERVER_IS_DIFFERENT_VER\x151.3\n");
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
            log::debug!("cod2: {from}: reconnect rejected: too soon");
            return;
        }
        let Some((ci, ping)) = self.core.challenges.connect(from, challenge, svs) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        log::info!("cod2: {from} connecting with {ping} challenge ping");
        if !crate::engine::Lan::Cod2.is_lan(from.ip()) {
            let (min, max) = (self.int("sv_minPing"), self.int("sv_maxPing"));
            if min != 0 && ping < min {
                self.send_oob(from, "error\nEXE_ERR_HIGH_PING_ONLY");
                return;
            }
            if max != 0 && max < ping {
                self.send_oob(from, "error\nEXE_ERR_LOW_PING_ONLY");
                return;
            }
        }
        // The same client again takes its old slot; anyone else the first
        // free one, from 0 with `sv_privatePassword` (an empty one matches
        // an empty `password`), else past the private ones.
        let slot = match self
            .core
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(same))
        {
            Some(slot) => slot,
            None => {
                let start = if key("password") == self.game.cvars.get("sv_privatePassword") {
                    0
                } else {
                    (self.int("sv_privateClients").max(0) as usize).min(self.core.clients.len())
                };
                let Some(slot) =
                    (start..self.core.clients.len()).find(|&i| self.core.clients[i].is_none())
                else {
                    self.send_oob(from, "error\nEXE_SERVERISFULL");
                    return;
                };
                slot
            }
        };
        // `ClientConnect` (0x80f8e7a): `g_password` unless empty or `none`,
        // compared case-sensitively.
        let gp = self.game.cvars.get("g_password");
        if !gp.is_empty() && !gp.eq_ignore_ascii_case("none") && gp != key("password") {
            if let Some(old) = self.core.clients[slot].take()
                && old.state > State::Zombie
                && let Some(level) = self.game.level.as_mut()
            {
                level.disconnect(slot);
            }
            self.send_oob(from, "error\nGAME_INVALIDPASSWORD");
            return;
        }
        self.admit(slot, from, qport, challenge, userinfo);
        // The connect took a client: the stored ping goes (0x808e2aa).
        self.core.challenges.taken(ci);
        if let Some(c) = self.core.clients[slot].as_mut() {
            c.last_heard = now;
        }
        log::info!("cod2: {from} connected as client {slot} ({})", key("name"));
        self.send_oob(from, "connectResponse");
        let n = self.client_count();
        if n == 1 || n == self.core.clients.len() {
            self.core.masters.force();
        }
    }

    /// `mod`: 1 off pure, with `fs_game`, or with a referenced iwd that is
    /// not a stock `iw_` one (1.4).
    fn modded(&self) -> bool {
        !self.pure()
            || !self.game.cvars.get("fs_game").is_empty()
            || self.game.iwds.referenced.iter().any(|(_, n)| {
                !n.strip_prefix("main/iw_")
                    .is_some_and(|d| d.len() == 2 && d.bytes().all(|b| b.is_ascii_digit()))
            })
    }

    /// `SVC_Info` (0x809537c), its keys in its order (1.4, 9).
    pub(super) fn svc_info(&mut self, challenge: &str) -> Option<String> {
        let int = |s: &Self, n: &str| s.int(n);
        // `clients` past `CS_ZOMBIE`; `sv_maxclients` the public slots plus
        // the private ones in use (17.1).
        let clients = self.client_count();
        let private = (self.int("sv_privateClients").max(0) as usize).min(self.core.clients.len());
        let private_used = self.core.clients[..private]
            .iter()
            .flatten()
            .filter(|c| c.state >= State::Connected)
            .count();
        let max_shown = self.core.clients.len() - (private - private_used);
        let mut i = vcod_common::net::connectionless::Info::new();
        i.set("challenge", challenge);
        i.set("protocol", PROTOCOL);
        i.set("hostname", &self.game.cfg.hostname);
        i.set("mapname", &self.game.cfg.map);
        if clients > 0 {
            i.set("clients", clients);
        }
        if max_shown > 0 {
            i.set("sv_maxclients", max_shown);
        }
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
        i.set("game", self.game.cvars.get("fs_game"));
        for (key, cvar) in [
            ("sv_allowAnonymous", "sv_allowAnonymous"),
            ("con_disabled", "sv_disableClientConsole"),
        ] {
            let v = int(self, cvar);
            if v != 0 {
                i.set(key, v);
            }
        }
        if !self.game.cvars.get("g_password").is_empty() {
            i.set("pswrd", 1);
        }
        for (key, cvar) in [("ff", "scr_friendlyfire"), ("kc", "scr_killcam")] {
            let v = int(self, cvar);
            if v != 0 {
                i.set(key, v);
            }
        }
        i.set("hw", 1);
        i.set("mod", u8::from(self.modded()));
        i.set("voice", int(self, "sv_voice"));
        i.set("pb", int(self, "sv_punkbuster"));
        Some(format!("infoResponse\n{i}"))
    }

    /// `SVC_Status` (0x8094c84): the serverinfo with `challenge`, `pswrd`
    /// and `mod` appended, then a `score ping "name"` line a client (1.5).
    pub(super) fn svc_status(&mut self, challenge: &str) -> Option<String> {
        let mut info = self.core.configstrings[0].clone();
        let mut add = |k: &str, v: String| {
            if !v.is_empty() {
                info.push_str(&format!("\\{k}\\{v}"));
            }
        };
        add("challenge", challenge.to_string());
        add(
            "pswrd",
            u8::from(!self.game.cvars.get("g_password").is_empty()).to_string(),
        );
        add("mod", u8::from(self.modded()).to_string());
        let mut text = format!("statusResponse\n{info}\n");
        for c in self
            .core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state >= State::Connected)
        {
            let name = vcod_common::net::info_value_for_key(&c.link.userinfo, "name").unwrap_or("");
            text.push_str(&format!("0 {} \"{name}\"\n", c.link.ping));
        }
        // The `mod` test tokenizes `sv_referencedIwdNames` when the server
        // is pure with no `fs_game` (0x8094c84), so the next status reads
        // its second name as `challenge`.
        if self.pure()
            && self.game.cvars.get("fs_game").is_empty()
            && !self.game.iwds.referenced.is_empty()
        {
            self.game.argv1 = self
                .game
                .iwds
                .referenced
                .get(1)
                .map_or(String::new(), |(_, n)| n.clone());
        }
        Some(text)
    }

    /// `SVC_RemoteCommand` (0x8097188): `rcon_password`, 1.1's 500 ms
    /// window, CoD4's refusals, the output back as `print` packets (1.6).
    fn remote_command(&mut self, from: SocketAddr, args: &[String], now: Instant) {
        let password = self.game.cvars.get("rcon_password").to_string();
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
        for text in out.finish() {
            self.send_oob(from, &text);
        }
    }

    /// The console (`SV_AddOperatorCommands`, Mac 0x172136): `status`,
    /// `serverinfo`, `systeminfo`, `dumpuser`, `say`, `tell`, the kicks,
    /// the map commands, `quit`, `set` and a bare cvar name to read or
    /// write. An unknown command prints nothing back (cod2-protocol.md 1.6).
    pub(super) fn exec(&mut self, line: &str, out: &mut Redirect) {
        let args = crate::game::say::tokenize(line);
        self.game.argv1 = args.get(1).cloned().unwrap_or_default();
        let Some(cmd) = args.first() else {
            return;
        };
        let arg = |i: usize| args.get(i).map_or("", String::as_str);
        match cmd.to_ascii_lowercase().as_str() {
            "status" => self.status(out),
            "serverinfo" | "systeminfo" => {
                // `SV_Serverinfo_f`, `SV_Systeminfo_f` and `Info_Print`: the
                // key padded to 20.
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
            // `SV_DumpUser_f` (Mac 0x171f5a).
            "dumpuser" => {
                if args.len() != 2 {
                    out.print("Usage: info <userid>\n");
                } else if let Some(slot) = self.player_by_name(arg(1), out) {
                    out.print("userinfo\n--------\n");
                    let ui = &self.core.clients[slot].as_ref().unwrap().link.userinfo;
                    for (k, v) in vcod_common::net::connectionless::info_pairs(ui) {
                        out.print(&format!("{k:<20}{v}\n"));
                    }
                }
            }
            // `SV_ConSay_f` (Mac 0x171d16): `h` to everyone, the arguments
            // joined.
            "say" if args.len() > 1 => {
                let text = format!("h \"\u{15}console: {}\"", console_args(&args[1..]));
                for slot in 0..self.core.clients.len() {
                    if self.core.clients[slot]
                        .as_ref()
                        .is_some_and(|c| c.state != State::Connected)
                    {
                        self.send_server_command(slot, &text);
                    }
                }
            }
            // `SV_ConTell_f` (Mac 0x171de2): the same to one client in the
            // world; anything else is silent.
            "tell" if args.len() > 2 => {
                let to = crate::game::builtins::cvar::atoi(arg(1));
                if let Ok(slot) = usize::try_from(to)
                    && self
                        .core
                        .clients
                        .get(slot)
                        .and_then(Option::as_ref)
                        .is_some_and(|c| c.state == State::Active)
                {
                    let text = format!("h \"\u{15}console: {}\"", console_args(&args[2..]));
                    self.send_server_command(slot, &text);
                }
            }
            "say" | "tell" => {}
            // `SV_KickUser_f` (0x808c3bc) by name, or `all`: `onlykick`
            // alone; `kick` and `tempBanUser` are `SV_TempBan_f` (0x808c55e),
            // which bans the kicked guid for `sv_kickBanTime` (17.7).
            "kick" | "onlykick" | "tempbanuser" => {
                if args.len() != 2 {
                    out.print(&format!(
                        "Usage: {cmd} <player name>\n{cmd} all = kick everyone\n"
                    ));
                } else if let Some(slot) = self.player_by_name(arg(1), out) {
                    let ban = !cmd.eq_ignore_ascii_case("onlykick");
                    self.kick_and_temp_ban(slot, ban, out);
                } else if arg(1).eq_ignore_ascii_case("all") {
                    // The kick returns no guid for `all`: no ban.
                    for slot in 0..self.core.clients.len() {
                        if self.core.clients[slot].is_some() {
                            self.kick(slot, out);
                        }
                    }
                }
            }
            // `SV_KickClient_f` (0x808c4d0) by slot: `clientkick` alone,
            // `tempBanClient` (`SV_TempBanNum_f` 0x808c6bc) with the ban.
            "clientkick" | "tempbanclient" => {
                if args.len() != 2 {
                    out.print(&format!("Usage: {cmd} <client number>\n"));
                } else if let Some(slot) = self.player_by_num(arg(1), out) {
                    let ban = cmd.eq_ignore_ascii_case("tempbanclient");
                    self.kick_and_temp_ban(slot, ban, out);
                }
            }
            // `SV_Ban_f` (0x808c5a8) and `SV_BanNum_f` (0x808c5fc) into
            // `SV_BanClient`.
            "banuser" | "banclient" => {
                let by_name = cmd.eq_ignore_ascii_case("banuser");
                if args.len() != 2 {
                    out.print(if by_name {
                        "Usage: banUser <player name>\n"
                    } else {
                        "Usage: banClient <client number>\n"
                    });
                } else if let Some(slot) = if by_name {
                    self.player_by_name(arg(1), out)
                } else {
                    self.player_by_num(arg(1), out)
                } {
                    self.ban_client(slot, out);
                }
            }
            // `SV_Unban_f` (0x808c650) into `SV_UnbanClient` (0x808d886).
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
            // `SV_Map_f` (Mac 0x172038): the map's `.d3dbsp` must be on the
            // search path, the reply checked against retail's rcon; the load
            // and `sv_cheats` are `map_command`'s (`cycle.rs`).
            "map" | "devmap" => {
                let name = arg(1).to_ascii_lowercase();
                if name.is_empty() {
                    return;
                }
                let path = format!("maps/mp/{name}.d3dbsp");
                if !self.game.fs.as_ref().is_some_and(|fs| fs.contains(&path)) {
                    out.print(&format!("Can't find map {path}\n"));
                    return;
                }
                let devmap = cmd.eq_ignore_ascii_case("devmap");
                if let Err(e) = self.map_command(&name, devmap) {
                    log::error!("cod2: map {name}: {e:#}");
                }
            }
            // `SV_MapRestart_f`, `SV_FastRestart_f` and `SV_MapRotate_f`.
            "map_restart" | "fast_restart" | "map_rotate" => {
                let r = match cmd.to_ascii_lowercase().as_str() {
                    "map_restart" => self.map_restart(),
                    "fast_restart" => self.fast_restart(),
                    _ => self.map_rotate(),
                };
                if let Err(e) = r {
                    log::error!("cod2: {cmd}: {e:#}");
                }
            }
            // vcod's bot management (`server/bot_admin.rs`, docs/bots.md).
            "addbot" => bot_admin::add_bot_command(self, &args[1..], out),
            "removebot" | "kickbot" => {
                let arg = (args.len() > 1).then(|| args[1..].join(" "));
                bot_admin::remove_bot_command(self, arg.as_deref(), out);
            }
            "listbots" => bot_admin::list_bots(self, out),
            "heartbeat" => self.core.masters.force(),
            "quit" => {
                // `SV_Shutdown`: each client's last message goes out at once,
                // then the master's `flatline`.
                for slot in 0..self.core.clients.len() {
                    if self.core.clients[slot].is_some() {
                        self.drop_client(slot, "EXE_SERVERQUIT");
                        self.send_client_message(slot);
                        if let Some(c) = self.core.clients[slot].as_mut() {
                            let to = c.link.addr;
                            let rest: Vec<_> = c.link.unsent.drain(..).collect();
                            self.core.outbox.extend(rest.into_iter().map(|p| (to, p)));
                        }
                    }
                }
                self.shutdown_game_log();
                self.heartbeat(true);
                self.core.quit = true;
            }
            "set" | "seta" | "sets" => {
                if args.len() >= 3 {
                    self.set_cvar(arg(1), &args[2..].join(" "));
                }
            }
            name => match self.game.cvars.lookup(name) {
                Some(_) if args.len() >= 2 => self.set_cvar(name, &args[1..].join(" ")),
                Some((n, v, d)) => out.print(&format!("\"{n}\" is:\"{v}^7\" default:\"{d}^7\"\n")),
                None => log::info!("cod2: unknown command {line:?}"),
            },
        }
    }

    /// `SV_Status_f` (Mac 0x171a62): a row per client, the name padded to
    /// 16 drawn characters and the address to 22, the port printed as a
    /// signed short as on 1.1 (live, cod2-protocol.md 1.6).
    fn status(&mut self, out: &mut Redirect) {
        out.print(&format!("map: {}\n", self.game.cfg.map));
        out.print(
            "num score ping guid   name            lastmsg address               qport rate\n",
        );
        out.print(
            "--- ----- ---- ------ --------------- ------- --------------------- ----- -----\n",
        );
        for slot in 0..self.core.clients.len() {
            let score = self.game.level.as_mut().map_or(0, |l| {
                l.rt.client_field(slot, "score")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
            });
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            let ping = match c.state {
                State::Connected => "CNCT ".to_string(),
                State::Zombie => "ZMBI ".to_string(),
                _ => format!(
                    "{:4} ",
                    crate::engine::link::calc_ping(&c.link.ping_ring).min(9999)
                ),
            };
            let name = client_name(c);
            let pad = 16usize.saturating_sub(draw_len(name));
            // `NET_AdrToString` of a bot's address reads the IPX form
            // (VERIFIED live, a test client in retail's `status`).
            let addr = if c.bot {
                "00000000.000000000000:0".to_string()
            } else {
                format!("{}:{}", c.link.addr.ip(), c.link.addr.port() as i16)
            };
            let rate = vcod_common::net::info_value_for_key(&c.link.userinfo, "rate")
                .map_or(0, crate::game::builtins::cvar::atoi);
            out.print(&format!(
                "{slot:3} {score:5} {ping}{:6} {name}^7{}{:7} {addr}{}{:5} {rate:5}\n",
                if c.bot {
                    0
                } else {
                    crate::engine::bans::guid_of(crate::engine::Lan::Cod2, c.link.addr)
                },
                " ".repeat(pad),
                c.last_heard.elapsed().as_millis(),
                " ".repeat(22usize.saturating_sub(addr.len())),
                c.link.netchan.qport,
            ));
        }
        out.print("\n");
    }

    /// A kick: the drop and its `<slot>:<name> <reason>` console line.
    fn kick(&mut self, slot: usize, out: &mut Redirect) {
        let name = self.core.clients[slot].as_ref().map_or(String::new(), |c| {
            vcod_common::net::info_value_for_key(&c.link.userinfo, "name")
                .unwrap_or("")
                .to_string()
        });
        self.drop_client(slot, "EXE_PLAYERKICKED");
        out.print(&format!("{slot}:{name} EXE_PLAYERKICKED\n"));
    }

    /// The kick (`FUN_0808c316`), then for `ban` and a non-zero guid
    /// `%s (guid %i) was kicked for cheating` and the guid into the
    /// temporary table (`SV_TempBan_f` 0x808c55e).
    fn kick_and_temp_ban(&mut self, slot: usize, ban: bool, out: &mut Redirect) {
        let Some((guid, name)) = self.guid_and_name(slot) else {
            return;
        };
        self.kick(slot, out);
        if ban && guid != 0 {
            out.print(&format!("{name} (guid {guid}) was kicked for cheating\n"));
            self.core.temp_bans.add(guid, self.core.sv_time_ms);
        }
    }

    /// `SV_BanClient` (0x808d75a): a guid of 0 or one already in `ban.txt`
    /// is refused with a print; otherwise `%i %s\r\n` is appended and the
    /// client kicked.
    fn ban_client(&mut self, slot: usize, out: &mut Redirect) {
        let Some((guid, name)) = self.guid_and_name(slot) else {
            return;
        };
        if guid == 0 {
            out.print("Can't ban user, GUID is 0\n");
        } else if self.core.bans.is_banned(guid) {
            out.print(&format!("This GUID ({guid}) is already banned\n"));
        } else {
            self.core.bans.append(guid, &name);
            self.kick(slot, out);
        }
    }

    /// A client's guid and its name as the bans write it (`I_CleanStr`
    /// over the first 63 characters).
    fn guid_and_name(&self, slot: usize) -> Option<(i32, String)> {
        let c = self.core.clients[slot].as_ref()?;
        let name = client_name(c);
        let guid = if c.bot {
            0
        } else {
            crate::engine::bans::guid_of(crate::engine::Lan::Cod2, c.link.addr)
        };
        Some((guid, crate::game::say::clean_name(name)))
    }

    /// `SV_GetPlayerByName` (Mac 0x1713d8): a name as sent, or cleaned of
    /// colours, case folded.
    fn player_by_name(&self, name: &str, out: &mut Redirect) -> Option<usize> {
        let found = self.core.clients.iter().position(|c| {
            c.as_ref().is_some_and(|c| {
                let n = client_name(c);
                n.eq_ignore_ascii_case(name)
                    || crate::game::say::clean_name(n).eq_ignore_ascii_case(name)
            })
        });
        if found.is_none() {
            out.print(&format!("Player {name} is not on the server\n"));
        }
        found
    }

    /// `SV_GetPlayerByNum` (Mac 0x1714f6): digits only, below
    /// `sv_maxclients`, a slot in use.
    fn player_by_num(&self, arg: &str, out: &mut Redirect) -> Option<usize> {
        if !arg.bytes().all(|b| b.is_ascii_digit()) {
            out.print(&format!("Bad slot number: {arg}\n"));
            return None;
        }
        let n = crate::game::builtins::cvar::atoi(arg);
        if n < 0 || n as usize >= self.core.clients.len() {
            out.print(&format!("Bad client slot: {n}\n"));
            return None;
        }
        if self.core.clients[n as usize].is_none() {
            out.print(&format!("Client {n} is not active\n"));
            return None;
        }
        Some(n as usize)
    }
}

/// The name the console reads (`cl+0x20c48`): empty for a zombie, whose
/// userinfo `SV_FreeClient` (0x808e1ea) set to "" at the drop; live, a
/// second `banUser` of a kicked name read `Player <name> is not on the
/// server` (17.7).
fn client_name(c: &super::Client) -> &str {
    if c.state == State::Zombie {
        return "";
    }
    vcod_common::net::info_value_for_key(&c.link.userinfo, "name").unwrap_or("")
}

/// `Cmd_Args` past the command as `SV_ConSay_f` reads it: the arguments
/// joined by one space, a leading quote and the last character dropped.
fn console_args(args: &[String]) -> String {
    let text = args.join(" ");
    match text.strip_prefix('"') {
        Some(rest) => {
            let mut rest = rest.to_string();
            rest.pop();
            rest
        }
        None => text,
    }
}

/// `I_DrawStrlen`: the characters drawn, `^` and the colour digit after it
/// not counted.
fn draw_len(s: &str) -> usize {
    let b = s.as_bytes();
    let (mut i, mut n) = (0, 0);
    while i < b.len() {
        if b[i] == b'^' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
            i += 2;
        } else {
            n += 1;
            i += 1;
        }
    }
    n
}
