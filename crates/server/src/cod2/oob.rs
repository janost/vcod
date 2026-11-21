//! CoD2's connectionless packets (docs/research/cod2-protocol.md 1):
//! `getchallenge`, the plain-text `connect`, `getinfo`, `getstatus` and
//! `rcon`, and the console the last runs.

use std::net::SocketAddr;
use std::time::Instant;

use super::{Cod2Server, State};
use crate::rcon::Redirect;
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
        let cmd = cmd.to_ascii_lowercase();
        // vcod's own limiter (`engine::ratelimit`); retail answers every query.
        if self.core.limiter.limited(&cmd, from.ip(), now) {
            log::debug!("cod2: {from}: {cmd} rate limited");
            return;
        }
        match cmd.as_str() {
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
        let (svs, xor) = (self.core.sv_time_ms, self.wall_ms());
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
    /// (1.2, 17.1): the engine's checks and slot
    /// ([`crate::engine::conn`]), `ClientConnect`, `connectResponse`, then
    /// `SV_Heartbeat_f` when the clients past `CS_ZOMBIE` number 1 or
    /// `sv_maxclients`.
    fn direct_connect(&mut self, from: SocketAddr, userinfo: &str, now: Instant) {
        let Some(a) = self.connect_slot(from, userinfo) else {
            return;
        };
        self.admit(a.slot, from, a.qport, a.challenge, userinfo);
        // The connect took a client: the stored ping goes (0x808e2aa).
        self.core.challenges.taken(a.ci);
        if let Some(c) = self.core.clients[a.slot].as_mut() {
            c.extra.last_heard = now;
        }
        let name = vcod_common::net::info_value_for_key(userinfo, "name").unwrap_or("");
        log::info!("cod2: {from} connected as client {} ({name})", a.slot);
        self.send_oob(from, "connectResponse");
        let n = self.client_count();
        if n == 1 || n == self.core.clients.len() {
            self.core.masters.force();
        }
    }

    /// The `mod` test tokenizes `sv_referencedIwdNames` when the server is
    /// pure with no `fs_game` (`SVC_Status` 0x8094c84), so the next status
    /// reads its second name as `challenge`.
    pub(super) fn status_argv1(&mut self) {
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
    }

    /// `mod`: 1 off pure, with `fs_game`, or with a referenced iwd that is
    /// not a stock `iw_` one (1.4).
    pub(super) fn modded(&self) -> bool {
        !self.pure()
            || !self.game.cvars.get("fs_game").is_empty()
            || self.game.iwds.referenced.iter().any(|(_, n)| {
                !n.strip_prefix("main/iw_")
                    .is_some_and(|d| d.len() == 2 && d.bytes().all(|b| b.is_ascii_digit()))
            })
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
            "serverinfo" | "systeminfo" | "dumpuser" | "tell" | "banuser" | "banclient"
            | "unbanuser" => {
                self.console_common(&args, out);
            }
            // `SV_ConSay_f` (Mac 0x171d16): `h` to everyone, the arguments
            // joined.
            "say" if args.len() > 1 => {
                let text = format!(
                    "h \"\u{15}console: {}\"",
                    crate::engine::conn::console_args(&args[1..])
                );
                for slot in 0..self.core.clients.len() {
                    if self.core.clients[slot]
                        .as_ref()
                        .is_some_and(|c| c.state != State::Connected)
                    {
                        self.send_server_command(slot, &text);
                    }
                }
            }
            "say" => {}
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
                let r = self.map_command(&name, devmap);
                self.load_failed(&format!("map {name}"), r);
            }
            // `SV_MapRestart_f`, `SV_FastRestart_f` and `SV_MapRotate_f`.
            "map_restart" | "fast_restart" | "map_rotate" => {
                let r = match cmd.to_ascii_lowercase().as_str() {
                    "map_restart" => self.map_restart(),
                    "fast_restart" => self.fast_restart(),
                    _ => self.map_rotate(),
                };
                self.load_failed(cmd, r);
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
                if let Some((name, value)) = crate::cvars::set_args(&args) {
                    self.console_set(name, &value, out);
                }
            }
            name => match self.game.cvars.lookup(name) {
                Some(_) if args.len() >= 2 => self.console_set(name, &args[1..].join(" "), out),
                Some((n, v, d)) => out.print(&format!("\"{n}\" is:\"{v}^7\" default:\"{d}^7\"\n")),
                None => log::info!("cod2: unknown command {line:?}"),
            },
        }
    }

    /// A console write of a dvar: `set_cvar`, but a file root
    /// (`crate::cvars::FILE_ROOTS`) prints the dvar set's refusal
    /// (0x80b2c22) and keeps its value.
    fn console_set(&mut self, name: &str, value: &str, out: &mut Redirect) {
        if crate::cvars::is_file_root(name) {
            out.print(&format!("{name} is write protected.\n"));
        } else {
            self.set_cvar(name, value);
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
                c.extra.last_heard.elapsed().as_millis(),
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
    pub(super) fn ban_client(&mut self, slot: usize, out: &mut Redirect) {
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
    pub(super) fn player_by_name(&self, name: &str, out: &mut Redirect) -> Option<usize> {
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
    pub(super) fn player_by_num(&self, arg: &str, out: &mut Redirect) -> Option<usize> {
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
