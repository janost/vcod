//! Votes: the `callvote` and `vote` client commands and the frame check
//! that ends one (`game.mp.i386.so` 1.1d `Cmd_CallVote_f` 0x47888,
//! `Cmd_Vote_f` 0x48088, `CheckVote` 0x501f0; 1.5b 0x43c00, 0x450aa,
//! 0x4fd1b). Layout, strings and the measured runs are in
//! docs/research/cod11-votes.md.

use crate::game::host::GameHost;
use vcod_common::asset_source::RawFile;
use vcod_common::text::atoi;
use vcod_gsc::Cx;

/// Configstring 15 (UO 16, `CsLayout::vote`): when the vote ends. 1.1
/// writes the level time, 1.5 the milliseconds left, which its cgame adds
/// to its own clock. The three after it are 1.1's numbers too.
pub const CS_VOTE_TIME: usize = 15;
/// Configstring 16: what the vote is for, a localized message.
pub const CS_VOTE_STRING: usize = 16;
/// Configstrings 17 and 18: the yes and no counts.
pub const CS_VOTE_YES: usize = 17;
pub const CS_VOTE_NO: usize = 18;

impl GameHost {
    /// One of the vote's four slots in the running protocol's table,
    /// `CS_VOTE_*` shifted to `CsLayout::vote`.
    fn set_vote_cs(&mut self, v1: usize, text: String) {
        let at = self.allocators.layout().vote + v1 - CS_VOTE_TIME;
        self.configstrings.set(at, text);
    }
}

/// `ps.eFlags` 0x20000: this client's vote is in. A call clears it on
/// every slot and sets it on the caller; a spawn keeps it.
pub const EF_VOTED: i32 = 0x20000;

/// How long a vote runs.
const VOTE_MS: i32 = 30_000;
/// How long after a pass the command runs.
const EXECUTE_DELAY_MS: i32 = 3_000;

/// `level`'s vote block (0x318..0xb28), zeroed with `level`, so a load or a
/// `map_restart` drops a running vote without a word.
#[derive(Clone, Debug, Default)]
pub struct Vote {
    /// `level.voteTime`: when the running vote ends, 0 when none runs.
    end_ms: i32,
    /// `level.voteExecuteTime`: when a passed vote's command runs, 0 when
    /// none waits.
    execute_ms: i32,
    /// `level.voteString`: the console command a pass runs.
    command: String,
    yes: i32,
    no: i32,
}

/// How long `allowComplaint` keeps the victim's complaint open (0x4366d,
/// `level.time + 0x5014`).
const COMPLAINT_MS: i32 = 20_500;

/// One client's teamkill complaint state, `pers` 0x2164..0x216c in both
/// modules. `ClientConnect` zeroes the count and sets the other two to -1.
#[derive(Clone, Copy, Debug)]
pub struct Complaint {
    /// Complaints filed against this client (0x2164).
    pub count: i32,
    /// The slot this client may complain about (0x2168), -1 for none.
    pub against: i32,
    /// When this client's window closes (0x216c); open while past
    /// `level.time`.
    pub end_ms: i32,
}

impl Default for Complaint {
    fn default() -> Complaint {
        Complaint {
            count: 0,
            against: -1,
            end_ms: -1,
        }
    }
}

/// What a vote command or the frame check sends: `Some(slot)` to one
/// client, `None` to everyone, in send order. The caller sends the vote
/// configstrings after them, as the `trap_SetConfigstring`s follow the
/// `trap_SendServerCommand`s in all three functions.
pub type Sends = Vec<(Option<usize>, String)>;

/// `Cmd_Vote_f`'s yes test, which reads the second character for two of
/// its three cases: `y` first, or `Y` or `1` second. `vote Yes` and
/// `vote 1` count as no on both servers (measured).
fn is_yes(arg: &str) -> bool {
    let b = arg.as_bytes();
    b.first() == Some(&b'y') || matches!(b.get(1), Some(b'Y' | b'1'))
}

/// The kinds 1.1 knows, in `Cmd_CallVote_f`'s compare order.
const KINDS_V1: &[&str] = &[
    "map_restart",
    "map_rotate",
    "typemap",
    "map",
    "g_gametype",
    "kick",
    "clientkick",
];

/// 1.5's kinds, each behind its own `g_allowVote*` gate.
const KINDS_V6: &[(&str, &str)] = &[
    ("map_restart", "g_allowVoteMapRestart"),
    ("map_rotate", "g_allowVoteMapRotate"),
    ("typemap", "g_allowVoteTypeMap"),
    ("map", "g_allowVoteMap"),
    ("g_gametype", "g_allowVoteGameType"),
    ("kick", "g_allowVoteKick"),
    ("clientkick", "g_allowVoteClientKick"),
    ("tempBanUser", "g_allowVoteTempBanUser"),
    ("tempBanClient", "g_allowVoteTempBanClient"),
    ("drawfriend", "g_allowVoteDrawFriend"),
    ("killcam", "g_allowVoteKillCam"),
    ("friendlyfire", "g_allowVoteFriendlyFire"),
];

/// CoD2's kinds under `g_oldVoting` (its default): 1.1's and the two
/// temporary bans, all behind `g_allowVote` (`Cmd_CallVote_f`, lnxded
/// 0x80ffbb2, VERIFIED).
const KINDS_COD2: &[&str] = &[
    "map_restart",
    "map_rotate",
    "typemap",
    "map",
    "g_gametype",
    "kick",
    "clientkick",
    "tempBanUser",
    "tempBanClient",
];

const COMMANDS_V1: &str = "e \"GAME_VOTECOMMANDSARE\u{15} map_restart, map_rotate, map <mapname>, \
    g_gametype <typename>, typemap <typename> <mapname>, kick <player>, clientkick <clientnum>\"";
const COMMANDS_V6: &str = "e \"GAME_VOTECOMMANDSARE\u{15} map_restart, map_rotate, map <mapname>, \
    g_gametype <typename>, typemap <typename> <mapname>, kick <player>, clientkick <clientnum>, \
    tempBanUser <player>, tempBanClient <clientNum>, drawfriend <value>, killcam <value>, \
    friendlyfire <value>\"";
const COMMANDS_COD2: &str = "e \"GAME_VOTECOMMANDSARE\u{15} map_restart, map_rotate, map <mapname>, \
    g_gametype <typename>, typemap <typename> <mapname>, kick <player>, clientkick <clientnum>, \
    tempBanUser <player>, tempBanClient <clientNum>\"";
/// The engine vote's rules (`Cmd_CallVote_f`, `Cmd_Vote_f`, `CheckVote`).
pub struct VoteRules {
    /// `;` in the third argument refuses the call too (CoD2's).
    pub checks_third_arg: bool,
    /// One kind list behind `g_allowVote` and its usage line; `None` runs
    /// 1.1's or 1.5's off `patch_1_5`.
    pub own_kinds: Option<(&'static [&'static str], &'static str)>,
    /// `typemap` with no map still asks for one (CoD2's, as 1.5's).
    pub typemap_needs_map: bool,
    /// The command a passed `map_restart` runs (CoD2's `fast_restart`);
    /// `None` runs the kind as called.
    pub restart_command: Option<&'static str>,
    /// `Cmd_Vote_f` has the complaint arm (CoD2's, lnxded 0x8100960, has
    /// none).
    pub complaints: bool,
    /// The vote ends on a running majority and weighs the abstainers at
    /// its end (`cod2_vote_verdict`).
    pub running_majority: bool,
    /// Under `g_oldVoting` 0 the script runs the votes
    /// (`ScriptRuntime::cod2_script_vote`).
    pub script_votes: bool,
    /// `CheckVote` runs inside `G_RunFrame` and queues its line with the
    /// game's commands, ahead of the configstring 15 it clears (CoD2's);
    /// otherwise [`GameHost::check_vote`] returns it.
    pub queues_lines: bool,
    /// A call needs this many clients connected
    /// (`GAME_VOTINGNOTENOUGHPLAYERS`, CoD4's 2).
    pub min_connected: usize,
    /// Every map passes (CoD4's fastfile build asks for none).
    pub maps_unchecked: bool,
    /// Configstring 15's time is `<voteTime> <sv_serverId>` (CoD4's).
    pub time_with_server_id: bool,
    /// The gametypes are `maps/mp/gametypes/_gametypes.txt`'s, read off
    /// the zones (CoD4's `Scr_ParseGameTypeList_FastFile`).
    pub gametype_list: bool,
}

impl VoteRules {
    pub const COD1: VoteRules = VoteRules {
        checks_third_arg: false,
        own_kinds: None,
        typemap_needs_map: false,
        restart_command: None,
        complaints: true,
        running_majority: false,
        script_votes: false,
        queues_lines: false,
        min_connected: 0,
        maps_unchecked: false,
        time_with_server_id: false,
        gametype_list: false,
    };

    /// CoD2's engine vote (cod2-protocol.md 3.4.2).
    pub const COD2: VoteRules = VoteRules {
        checks_third_arg: true,
        own_kinds: Some((KINDS_COD2, COMMANDS_COD2)),
        typemap_needs_map: true,
        restart_command: Some("fast_restart"),
        complaints: false,
        running_majority: true,
        script_votes: true,
        queues_lines: true,
        min_connected: 0,
        maps_unchecked: false,
        time_with_server_id: false,
        gametype_list: false,
    };

    /// CoD4 1.7's (`Cmd_CallVote_f` 0x80adbf2, `Cmd_Vote_f` 0x80ada2a,
    /// `CheckVote` 0x80b423a; cod4-map-cycle.md 4): CoD2's kinds, strings,
    /// verdict and `g_oldVoting`, two clients connected, no map check,
    /// and the server id beside the vote's end.
    pub const COD4: VoteRules = VoteRules {
        min_connected: 2,
        maps_unchecked: true,
        time_with_server_id: true,
        gametype_list: true,
        ..VoteRules::COD2
    };
}

const NO_MAP: &str = "e \"\u{15}the server doesn't have that map\"";

impl GameHost {
    /// The clock the vote runs on: 1.1's level time, 1.5's
    /// `trap_Milliseconds` (`svs_ms`), which a match timeout does not stop.
    fn vote_clock(&self, svs_ms: i32) -> i32 {
        // CoD4's host reads as 1.5 (its `protocol` is 6 too); its vote runs
        // on `level.time` (0x836f8ac).
        if self.patch_1_5 && !self.profile.votes.time_with_server_id {
            svs_ms
        } else {
            self.level_time_ms
        }
    }

    fn is_spectator(&mut self, cx: &mut Cx, slot: usize) -> bool {
        self.client_field(cx, slot, "sessionteam").as_deref() == Some("spectator")
    }

    /// A slot the game counts as `CON_CONNECTED`: in the world.
    fn in_world(&self, slot: usize) -> bool {
        self.client_roster
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|r| r.active)
    }

    /// `level.numVotingClients` as `CalculateRanks` (0x50c60) counts it:
    /// clients in the world off the spectator team. `CalculateRanks` runs
    /// at every connect, begin, disconnect and `sessionteam` write, so a
    /// count taken now is the one it last left. Unlike retail it skips
    /// bots, which never vote and would hold every vote short of a pass.
    fn voting_clients(&mut self, cx: &mut Cx) -> i32 {
        let mut n = 0;
        for slot in 0..self.client_roster.len() {
            let bot = self.client_roster[slot].is_some_and(|r| r.bot);
            if self.in_world(slot) && !bot && !self.is_spectator(cx, slot) {
                n += 1;
            }
        }
        n
    }

    fn netname(&self, slot: usize) -> &str {
        self.client_names.get(slot).map_or("", String::as_str)
    }

    /// `Scr_GetGameTypeNameForScript` for a name `Scr_IsValidGameType`
    /// accepts: the first token of `maps/mp/gametypes/<name>.txt` for a
    /// `<name>.gsc` not starting with `_` (`Scr_ParseGameTypeList`
    /// 0x5c21c). `None` for a gametype the paks lack.
    fn gametype_name(&self, name: &str) -> Option<String> {
        let name = name.to_ascii_lowercase();
        if self.profile.votes.gametype_list {
            return self.listed_gametype_name(&name);
        }
        let fs = self.fs.as_ref()?;
        if name.starts_with('_') || !fs.contains(&format!("maps/mp/gametypes/{name}.gsc")) {
            return None;
        }
        let text = fs
            .load::<RawFile>(&format!("maps/mp/gametypes/{name}.txt"))
            .ok()
            .map(|b| b.text().into_owned())
            .unwrap_or_default();
        Some(
            crate::game::say::tokenize(&text)
                .into_iter()
                .next()
                .unwrap_or_default(),
        )
    }

    /// CoD4's `Scr_GetGameTypeNameForScript` over the list
    /// `_gametypes.txt` names: a listed name's `<name>.txt`, its first
    /// token. `None` for one the list lacks.
    fn listed_gametype_name(&self, name: &str) -> Option<String> {
        let assets = self.assets.as_ref()?;
        let read = |path: &str| {
            assets
                .read(path)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        };
        let list = read("maps/mp/gametypes/_gametypes.txt")?;
        crate::game::say::tokenize(&list)
            .iter()
            .find(|g| g.eq_ignore_ascii_case(name))?;
        let text = read(&format!("maps/mp/gametypes/{name}.txt")).unwrap_or_default();
        Some(
            crate::game::say::tokenize(&text)
                .into_iter()
                .next()
                .unwrap_or_default(),
        )
    }

    /// 1.5's `trap_MapExists`, which vcod asks on 1.1 too.
    fn map_exists(&self, name: &str) -> bool {
        if self.profile.votes.maps_unchecked {
            return true;
        }
        self.fs
            .as_ref()
            .is_some_and(|fs| fs.resolve_map(name).is_some())
    }

    /// The kick target: by cleaned name (`kick`, `tempBanUser`, the last
    /// match winning) or by slot (`clientkick`, `tempBanClient`), in the
    /// world either way. `None` is `GAME_CLIENTNOTONSERVER`.
    fn vote_target(&self, by_name: bool, arg: &str) -> Option<usize> {
        if by_name {
            return (0..64usize).rev().find(|&s| {
                self.in_world(s)
                    && crate::game::say::clean_name(self.netname(s)).eq_ignore_ascii_case(arg)
            });
        }
        let n = atoi(arg);
        if n == 0 && !arg.eq_ignore_ascii_case("0") {
            return None;
        }
        let n = usize::try_from(n).ok().filter(|&n| n < 64)?;
        self.in_world(n).then_some(n)
    }

    /// `Cmd_CallVote_f`. `argv` is the whole command, `callvote` first.
    pub fn call_vote(&mut self, cx: &mut Cx, slot: usize, argv: &[String], svs_ms: i32) -> Sends {
        let reply = |m: &str| vec![(Some(slot), m.to_string())];
        if crate::game::builtins::cvar::atoi(self.cvars.get("g_allowVote")) == 0 {
            return reply("e \"GAME_VOTINGNOTENABLED\"");
        }
        let connected = self.client_roster.iter().flatten().count();
        if connected < self.profile.votes.min_connected {
            return reply("e \"GAME_VOTINGNOTENOUGHPLAYERS\"");
        }
        if self.vote.end_ms != 0 {
            return reply("e \"GAME_VOTEALREADYINPROGRESS\"");
        }
        // `client->pers.voteCount` (0x215c) is tested against 3 here and
        // written nowhere in either module: GAME_MAXVOTESCALLED never goes.
        if self.is_spectator(cx, slot) {
            return reply("e \"GAME_NOSPECTATORCALLVOTE\"");
        }
        let arg = |i: usize| argv.get(i).map_or("", String::as_str);
        let (kind, arg2) = (arg(1).to_string(), arg(2).to_string());
        let rules = &self.profile.votes;
        if kind.contains(';')
            || arg2.contains(';')
            || (rules.checks_third_arg && arg(3).contains(';'))
        {
            return reply("e \"GAME_INVALIDVOTESTRING\"");
        }
        let gate = if let Some((kinds, _)) = rules.own_kinds {
            kinds
                .iter()
                .any(|k| k.eq_ignore_ascii_case(&kind))
                .then_some("g_allowVote")
        } else if self.patch_1_5 {
            KINDS_V6
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&kind))
                .map(|(_, cvar)| *cvar)
        } else {
            KINDS_V1
                .iter()
                .any(|k| k.eq_ignore_ascii_case(&kind))
                .then_some("g_allowVote")
        };
        let Some(gate) = gate else {
            let list = if let Some((_, commands)) = rules.own_kinds {
                commands
            } else if self.patch_1_5 {
                COMMANDS_V6
            } else {
                COMMANDS_V1
            };
            return vec![
                (Some(slot), "e \"GAME_INVALIDVOTESTRING\"".into()),
                (Some(slot), list.into()),
            ];
        };
        if crate::game::builtins::cvar::atoi(self.cvars.get(gate)) == 0 {
            return reply("e \"GAME_VOTINGNOTENABLED\"");
        }
        // A passed vote still waiting runs now, before this one replaces it.
        if self.vote.execute_ms != 0 {
            self.vote.execute_ms = 0;
            self.console.push(self.vote.command.clone());
        }
        let (command, display) = match self.vote_strings(&kind, &arg2, arg(3)) {
            Ok(v) => v,
            Err(m) => return reply(&m),
        };
        self.vote.command = command;
        let now = self.vote_clock(svs_ms);
        self.vote.end_ms = now.wrapping_add(VOTE_MS);
        self.vote.yes = 1;
        self.vote.no = 0;
        self.client_voted.fill(false);
        if let Some(v) = self.client_voted.get_mut(slot) {
            *v = true;
        }
        let time = if self.profile.votes.time_with_server_id {
            let id = crate::game::builtins::cvar::atoi(self.cvars.get("sv_serverid"));
            format!("{} {id}", self.vote.end_ms)
        } else if self.patch_1_5 {
            VOTE_MS.to_string()
        } else {
            self.vote.end_ms.to_string()
        };
        self.set_vote_cs(CS_VOTE_TIME, time);
        self.set_vote_cs(CS_VOTE_STRING, display);
        self.set_vote_cs(CS_VOTE_YES, "1".into());
        self.set_vote_cs(CS_VOTE_NO, "0".into());
        vec![(
            None,
            format!("e \"GAME_CALLEDAVOTE\u{15}{}\"", self.netname(slot)),
        )]
    }

    /// The console command a pass runs and the configstring 16 text, per
    /// kind, or the reply that refuses the call.
    fn vote_strings(&self, kind: &str, arg2: &str, arg3: &str) -> Result<(String, String), String> {
        let is = |k: &str| kind.eq_ignore_ascii_case(k);
        if is("typemap") {
            let Some(type_name) = self.gametype_name(arg2) else {
                return Err("e \"GAME_INVALIDGAMETYPE\"".into());
            };
            let gametype = if arg2.eq_ignore_ascii_case(self.cvars.get("g_gametype")) {
                ""
            } else {
                arg2
            };
            // 1.1's typemap without a map keeps the map (`GAME_TYPEMAP_NOCHANGE`
            // in the 1.1d capture); 1.5 and CoD2 ask for it even then.
            if (self.patch_1_5 || self.profile.votes.typemap_needs_map || !arg3.is_empty())
                && !self.map_exists(arg3)
            {
                return Err(NO_MAP.into());
            }
            let map = if arg3.eq_ignore_ascii_case(self.cvars.get("mapname")) {
                ""
            } else {
                arg3
            };
            return match (gametype.is_empty(), map.is_empty()) {
                (true, true) => Err("e \"GAME_TYPEMAP_NOCHANGE\"".into()),
                (false, true) => Ok((
                    format!("g_gametype {gametype}; map_restart"),
                    format!("GAME_VOTE_GAMETYPE\u{14}{type_name}"),
                )),
                (true, false) => Ok((format!("map {map}"), format!("GAME_VOTE_MAP\u{15}{map}"))),
                (false, false) => Ok((
                    format!("g_gametype {gametype}; map {map}"),
                    format!(
                        "GAME_VOTE_GAMETYPE\u{14}{type_name}\u{15} - \u{14}GAME_VOTE_MAP\u{15}{map}"
                    ),
                )),
            };
        }
        if is("g_gametype") {
            let Some(type_name) = self.gametype_name(arg2) else {
                return Err("e \"GAME_INVALIDGAMETYPE\"".into());
            };
            return Ok((
                format!("{kind} {arg2}; map_restart"),
                format!("GAME_VOTE_GAMETYPE\u{14}{type_name}"),
            ));
        }
        if is("map_restart") {
            let command = self.profile.votes.restart_command.unwrap_or(kind);
            return Ok((command.to_string(), "GAME_VOTE_MAPRESTART".into()));
        }
        if is("map_rotate") {
            return Ok((kind.to_string(), "GAME_VOTE_NEXTMAP".into()));
        }
        if is("map") {
            if !self.map_exists(arg2) {
                return Err(NO_MAP.into());
            }
            return Ok((
                format!("{kind} {arg2}"),
                format!("GAME_VOTE_MAP\u{15}{arg2}"),
            ));
        }
        let by_name = is("kick") || is("tempBanUser");
        if by_name || is("clientkick") || is("tempBanClient") {
            let Some(n) = self.vote_target(by_name, arg2) else {
                return Err("e \"GAME_CLIENTNOTONSERVER\"".into());
            };
            let verb = if kind.starts_with(['t', 'T']) {
                "tempBanClient"
            } else {
                "clientkick"
            };
            return Ok((
                format!("{verb} \"{n}\""),
                format!("GAME_VOTE_KICK\u{15}({n}){}", self.netname(n)),
            ));
        }
        // 1.5's three settings votes. drawfriend's and killcam's value test
        // asks for an argument that is both "0" and "1", so any value passes it.
        let on_off = |v: &str| {
            if v.starts_with('1') {
                "MENU_ON"
            } else {
                "MENU_OFF"
            }
        };
        let not_valid = |v: &str| format!("e \"PATCH_1_5_VOTE_ARG_NOTVALID\u{15}{v}\"");
        if is("drawfriend") {
            if arg2.eq_ignore_ascii_case(self.cvars.get("scr_drawfriend")) {
                return Err("e \"PATCH_1_5_DRAWFRIEND_NOCHANGE\"".into());
            }
            return Ok((
                format!("setdrawfriend {arg2}"),
                format!("PATCH_1_5_VOTE_DRAWFRIEND\u{14}{}", on_off(arg2)),
            ));
        }
        if is("killcam") {
            if arg2.eq_ignore_ascii_case(self.cvars.get("scr_killcam")) {
                return Err("e \"PATCH_1_5_KILLCAM_NOCHANGE\"".into());
            }
            return Ok((
                format!("setkillcam {arg2}"),
                format!("PATCH_1_5_VOTE_KILLCAM\u{14}{}", on_off(arg2)),
            ));
        }
        // friendlyfire, the last kind on the list.
        if arg2.eq_ignore_ascii_case(self.cvars.get("scr_friendlyfire")) {
            return Err("e \"PATCH_1_5_FRIENDLYFIRE_NOCHANGE\"".into());
        }
        let mode = match arg2.to_ascii_lowercase().as_str() {
            "0" => "MENU_OFF",
            "1" => "MENU_ON",
            "2" => "MENU_REFLECT",
            "3" => "MENU_SHARED",
            _ => return Err(not_valid(arg2)),
        };
        Ok((
            format!("setfriendlyfire {arg2}"),
            format!("PATCH_1_5_VOTE_FRIENDLYFIRE\u{14}{mode}"),
        ))
    }

    /// `self allowComplaint(attacker)` (1.1 0x43578, 1.5 0x3e86c): the
    /// victim may answer `vote` about `attacker` for 20.5 s, and is told
    /// with `m <attacker>`. Nothing when the two are one. Retail's `m -4`
    /// for a listen server's own client never goes: vcod has none.
    pub fn allow_complaint(&mut self, victim: usize, attacker: usize) {
        if victim == attacker {
            return;
        }
        self.client_commands.push((victim, format!("m {attacker}")));
        if let Some(c) = self.client_complaints.get_mut(victim) {
            c.against = attacker as i32;
            c.end_ms = self.level_time_ms.wrapping_add(COMPLAINT_MS);
        }
    }

    /// `ClientDisconnect`'s complaint pass (1.1 0x42b78, UO 0x45433): the
    /// lowest slot whose window names the slot going away loses it and
    /// hears `m -2`; a later one keeps its window. 1.5 and UO skip slots
    /// not connected, 1.1 does not, so there a stale window on a slot
    /// since left can take the clear.
    pub fn complaints_about_gone(&mut self, gone: usize) {
        let v15 = self.patch_1_5;
        let hit = (0..self.client_complaints.len()).find(|&slot| {
            self.client_complaints[slot].against == gone as i32
                && (!v15 || self.client_roster.get(slot).is_some_and(Option::is_some))
        });
        if let Some(slot) = hit {
            let c = &mut self.client_complaints[slot];
            c.against = -1;
            c.end_ms = 0;
            self.client_commands.push((slot, "m -2".into()));
        }
    }

    /// `Cmd_Vote_f`'s complaint arm, taken while the voter's window is
    /// open: a yes files a complaint against the window's client, a no
    /// withdraws. At `g_complaintlimit` complaints the offender is dropped
    /// (`complaint_drop`); short of it they are warned with the count
    /// left. `None` when the window is shut, which `vote` then closes.
    fn complain(&mut self, slot: usize, arg: &str) -> Option<Sends> {
        let c = *self.client_complaints.get(slot)?;
        if c.end_ms.wrapping_sub(self.level_time_ms) <= 0 {
            return None;
        }
        // A window on a client no longer in the world is left open and the
        // vote is ignored (0x480bc..0x480c9).
        let target = usize::try_from(c.against)
            .ok()
            .filter(|&t| self.in_world(t));
        let Some(target) = target else {
            return Some(Vec::new());
        };
        self.client_complaints[slot].against = -1;
        self.client_complaints[slot].end_ms = -1;
        let arg: String = arg.chars().take(63).collect();
        if !is_yes(&arg) {
            return Some(vec![(Some(slot), "m -2".into())]);
        }
        self.client_complaints[target].count += 1;
        let limit = atoi(self.cvars.get("g_complaintlimit"));
        let left = limit.wrapping_sub(self.client_complaints[target].count);
        if left <= 0 {
            self.complaint_drop = Some(target);
            return Some(vec![(Some(slot), "m -1".into())]);
        }
        Some(vec![
            (
                Some(target),
                format!(
                    "e \"\u{15}^1\u{14}GAME_WARNING\u{15}^7: \u{14}GAME_COMPLAINTFILEDAGAINST\u{15}{left}\""
                ),
            ),
            (Some(slot), "m -1".into()),
        ])
    }

    /// `Cmd_Vote_f`: the complaint arm while the voter's window is open,
    /// else the window closed and the vote cast.
    pub fn cast_vote(&mut self, cx: &mut Cx, slot: usize, arg: &str) -> Sends {
        // CoD2's `Cmd_Vote_f` (lnxded 0x8100960) has no complaint arm.
        if self.profile.votes.complaints
            && let Some(sends) = self.complain(slot, arg)
        {
            return sends;
        }
        if let Some(c) = self.client_complaints.get_mut(slot) {
            c.against = -1;
            c.end_ms = -1;
        }
        let reply = |m: &str| vec![(Some(slot), format!("e \"{m}\""))];
        if self.vote.end_ms == 0 {
            return reply("GAME_NOVOTEINPROGRESS");
        }
        if self.client_voted.get(slot).copied().unwrap_or(false) {
            return reply("GAME_VOTEALREADYCAST");
        }
        if self.is_spectator(cx, slot) {
            return reply("GAME_NOSPECTATORVOTE");
        }
        if let Some(v) = self.client_voted.get_mut(slot) {
            *v = true;
        }
        // `trap_Argv` into a 64-byte buffer.
        let arg: String = arg.chars().take(63).collect();
        if is_yes(&arg) {
            self.vote.yes += 1;
            self.set_vote_cs(CS_VOTE_YES, self.vote.yes.to_string());
        } else {
            self.vote.no += 1;
            self.set_vote_cs(CS_VOTE_NO, self.vote.no.to_string());
        }
        reply("GAME_VOTECAST")
    }

    /// CoD2's `CheckVote` verdict (Mac 0x1aeb2a): while the vote runs a
    /// majority of `n / 2 + 1` passes it and a `no` count past
    /// `n - majority` fails it; at its end it passes when the yes count
    /// beats the no count with the abstainers' share
    /// (`ceil(abstained * g_voteAbstainWeight)`, default 0.5) added.
    /// VERIFIED. `None` while undecided.
    fn cod2_vote_verdict(&self, now: i32, n: i32) -> Option<&'static str> {
        let (yes, no) = (self.vote.yes, self.vote.no);
        let pass = Some("e \"GAME_VOTEPASSED\"");
        let fail = Some("e \"GAME_VOTEFAILED\"");
        if now.wrapping_sub(self.vote.end_ms) < 0 {
            let majority = n / 2 + 1;
            return if yes >= majority {
                pass
            } else if no <= n - majority {
                None
            } else {
                fail
            };
        }
        let weight = match self.cvars.get("g_voteAbstainWeight") {
            "" => 0.5,
            v => crate::game::builtins::cvar::atof(v),
        };
        let abstain = ((n - (yes + no)) as f32 * weight).ceil();
        if ((abstain + no as f32) as i32) < yes {
            pass
        } else {
            fail
        }
    }

    /// `CheckVote`, once a frame after `CheckTeamStatus`: a passed vote's
    /// command goes to the console once its delay is up, and a running vote
    /// passes, fails or times out. Returns the lines for everyone (none
    /// under [`VoteRules::queues_lines`]); true in the second slot when
    /// configstring 15 was written.
    pub fn check_vote(&mut self, cx: &mut Cx, svs_ms: i32) -> (Vec<String>, bool) {
        let now = self.vote_clock(svs_ms);
        if self.vote.execute_ms != 0 && self.vote.execute_ms < now {
            self.vote.execute_ms = 0;
            self.console.push(self.vote.command.clone());
        }
        if self.vote.end_ms == 0 {
            return (Vec::new(), false);
        }
        let n = self.voting_clients(cx);
        let (yes, no) = (self.vote.yes, self.vote.no);
        let line = if self.profile.votes.running_majority {
            match self.cod2_vote_verdict(now, n) {
                Some(line) => line,
                None => return (Vec::new(), false),
            }
        } else if now.wrapping_sub(self.vote.end_ms) >= 0 {
            "e \"GAME_VOTEFAILED\""
        } else if self.patch_1_5 {
            // 1.5 (0x4fd1b): a majority of `n / 2 + 1` passes, and fails
            // once no more than `n - majority` would be needed against it.
            let majority = n / 2 + 1;
            if yes >= majority {
                "e \"GAME_VOTEPASSED\""
            } else if no > n - majority {
                "e \"GAME_VOTEFAILED\""
            } else {
                return (Vec::new(), false);
            }
        } else if yes > n / 2 {
            "e \"GAME_VOTEPASSED\""
        } else if no >= n / 2 {
            "e \"GAME_VOTEFAILED\""
        } else {
            return (Vec::new(), false);
        };
        if line.contains("PASSED") {
            self.vote.execute_ms = now.wrapping_add(EXECUTE_DELAY_MS);
        }
        self.vote.end_ms = 0;
        let lines = if self.profile.votes.queues_lines {
            let all = crate::game::commands::ALL_CLIENTS;
            self.client_commands.push((all, line.to_string()));
            Vec::new()
        } else {
            vec![line.to_string()]
        };
        self.set_vote_cs(CS_VOTE_TIME, String::new());
        (lines, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::scoreboard::RosterSlot;
    use vcod_gsc::{Host, Value};

    /// Clients in slots 0..teams.len(), in the world, on those teams.
    fn host(vm: &mut vcod_gsc::Vm, teams: &[&str]) -> GameHost {
        let mut host = GameHost::new(&crate::game::profile::COD1_11, vec![String::new(); 2048]);
        host.cvars.set("g_allowVote", "1");
        for (slot, team) in teams.iter().enumerate() {
            vm.with_cx(|cx| {
                host.ents.spawn_client(cx, slot, None).expect("a client");
                let ent = host.ents.handle(slot as u32).unwrap();
                let atom = cx.intern_folded("sessionteam");
                let value = Value::String(cx.intern_exact(team));
                host.set_field(cx, ent, atom, value).unwrap();
            });
            host.client_roster[slot] = Some(RosterSlot {
                active: true,
                ..Default::default()
            });
            host.client_names[slot] = ["Able", "Baker", "Charlie", "Dog"][slot].into();
        }
        host
    }

    fn call(vm: &mut vcod_gsc::Vm, host: &mut GameHost, slot: usize, line: &str) -> Sends {
        let argv = crate::game::say::tokenize(line);
        vm.with_cx(|cx| host.call_vote(cx, slot, &argv, 0))
    }

    fn vote(vm: &mut vcod_gsc::Vm, host: &mut GameHost, slot: usize, arg: &str) -> Sends {
        vm.with_cx(|cx| host.cast_vote(cx, slot, arg))
    }

    /// The lines `CheckVote` returned or queued.
    fn check(vm: &mut vcod_gsc::Vm, host: &mut GameHost) -> Vec<String> {
        let mut lines = vm.with_cx(|cx| host.check_vote(cx, 0)).0;
        let queued = host.client_commands.take_stamped();
        lines.extend(queued.into_iter().map(|(_, _, line)| line));
        lines
    }

    fn to(slot: usize, m: &str) -> (Option<usize>, String) {
        (Some(slot), format!("e \"{m}\""))
    }

    /// CoD2's engine vote (cod2-protocol.md 3.4.2): the third argument's
    /// `;`, its kind list, `fast_restart` for a restart, no complaint arm,
    /// and the running majority.
    #[test]
    fn cod2_votes_take_their_own_rules() {
        let mut vm = vcod_gsc::Vm::new();
        let mut h = host(&mut vm, &["allies", "axis", "allies"]);
        h.profile = &crate::game::profile::COD2;
        assert_eq!(
            call(&mut vm, &mut h, 0, "callvote typemap dm a;b"),
            vec![to(0, "GAME_INVALIDVOTESTRING")]
        );
        let out = call(&mut vm, &mut h, 0, "callvote drawfriend 1");
        assert_eq!(out[1], (Some(0), COMMANDS_COD2.to_string()));
        let out = call(&mut vm, &mut h, 0, "callvote map_restart");
        assert_eq!(
            out,
            vec![(None, "e \"GAME_CALLEDAVOTE\u{15}Able\"".to_string())]
        );
        assert_eq!(h.vote.command, "fast_restart");
        assert_eq!(
            vote(&mut vm, &mut h, 0, "yes"),
            vec![to(0, "GAME_VOTEALREADYCAST")]
        );
        // Three voters, a majority of two: one no leaves it open.
        assert_eq!(vote(&mut vm, &mut h, 1, "no"), vec![to(1, "GAME_VOTECAST")]);
        assert!(check(&mut vm, &mut h).is_empty());
        vote(&mut vm, &mut h, 2, "no");
        // The line queues ahead of the configstring 15 the end clears, as
        // retail sent them (cod2-protocol.md 3.4.2).
        h.configstrings.take_writes_stamped();
        let (lines, _) = vm.with_cx(|cx| h.check_vote(cx, 0));
        assert!(lines.is_empty());
        let queued = h.client_commands.take_stamped();
        let writes = h.configstrings.take_writes_stamped();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].2, "e \"GAME_VOTEFAILED\"");
        assert_eq!((writes.len(), writes[0].2.as_str()), (1, ""));
        assert!(queued[0].0 < writes[0].0);
    }

    /// At its end a CoD2 vote counts half the abstainers against it.
    #[test]
    fn a_cod2_vote_weighs_the_abstainers_at_its_end() {
        for (weight, line) in [
            ("0.5", "e \"GAME_VOTEFAILED\""),
            ("0", "e \"GAME_VOTEPASSED\""),
        ] {
            let mut vm = vcod_gsc::Vm::new();
            let mut h = host(&mut vm, &["allies", "axis", "allies"]);
            h.profile = &crate::game::profile::COD2;
            h.cvars.set("g_voteAbstainWeight", weight);
            call(&mut vm, &mut h, 0, "callvote map_rotate");
            h.level_time_ms = VOTE_MS;
            assert_eq!(check(&mut vm, &mut h), vec![line], "weight {weight}");
        }
    }

    #[test]
    fn the_yes_test_reads_the_second_character() {
        assert!(is_yes("yes") && is_yes("y") && is_yes("xY") && is_yes("01"));
        assert!(!is_yes("Yes") && !is_yes("1") && !is_yes("no") && !is_yes(""));
    }

    /// The 1.1d capture's refusals, in its order.
    #[test]
    fn a_call_is_refused_as_11d_refused_it() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "allies", "spectator"]);
        host.cvars.set("g_gametype", "tdm");
        let refused = call(&mut vm, &mut host, 0, "callvote");
        assert_eq!(refused[0], to(0, "GAME_INVALIDVOTESTRING"));
        assert!(
            refused[1]
                .1
                .starts_with("e \"GAME_VOTECOMMANDSARE\u{15} map_restart")
        );
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote kick Nobody"),
            [to(0, "GAME_CLIENTNOTONSERVER")]
        );
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote clientkick 9"),
            [to(0, "GAME_CLIENTNOTONSERVER")]
        );
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote g_gametype bogus"),
            [to(0, "GAME_INVALIDGAMETYPE")]
        );
        assert_eq!(
            call(&mut vm, &mut host, 2, "callvote map_restart"),
            [to(2, "GAME_NOSPECTATORCALLVOTE")]
        );
        host.cvars.set("g_allowVote", "0");
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote map_restart"),
            [to(0, "GAME_VOTINGNOTENABLED")]
        );
    }

    /// The kick vote of the 1.1d capture: called by name, the configstrings
    /// as it sent them, a spectator turned away, and one no of two voters
    /// failing it.
    #[test]
    fn a_kick_vote_fails_on_one_no_of_two() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "allies", "spectator"]);
        host.level_time_ms = 19650;
        let called = call(&mut vm, &mut host, 0, "callvote kick baker");
        assert_eq!(called, [(None, "e \"GAME_CALLEDAVOTE\u{15}Able\"".into())]);
        assert_eq!(
            &host.configstrings[15..19],
            ["49650", "GAME_VOTE_KICK\u{15}(1)Baker", "1", "0"]
        );
        assert_eq!(host.vote.command, "clientkick \"1\"");
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote map_restart"),
            [to(0, "GAME_VOTEALREADYINPROGRESS")]
        );
        assert_eq!(
            vote(&mut vm, &mut host, 0, "yes"),
            [to(0, "GAME_VOTEALREADYCAST")]
        );
        assert_eq!(
            vote(&mut vm, &mut host, 2, "yes"),
            [to(2, "GAME_NOSPECTATORVOTE")]
        );
        assert!(check(&mut vm, &mut host).is_empty());
        assert_eq!(vote(&mut vm, &mut host, 1, "no"), [to(1, "GAME_VOTECAST")]);
        assert_eq!(host.configstrings[18], "1");
        assert_eq!(check(&mut vm, &mut host), ["e \"GAME_VOTEFAILED\""]);
        assert_eq!(host.configstrings[15], "");
        assert_eq!(
            vote(&mut vm, &mut host, 1, "yes"),
            [to(1, "GAME_NOVOTEINPROGRESS")]
        );
    }

    /// A pass queues its command 3 s on, and a vote nobody answers fails
    /// 30 s after the call.
    #[test]
    fn a_pass_runs_its_command_after_the_delay() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "allies"]);
        host.level_time_ms = 1000;
        call(&mut vm, &mut host, 0, "callvote map_restart");
        vote(&mut vm, &mut host, 1, "y");
        assert_eq!(check(&mut vm, &mut host), ["e \"GAME_VOTEPASSED\""]);
        host.level_time_ms = 4000;
        check(&mut vm, &mut host);
        assert!(host.console.is_empty());
        host.level_time_ms = 4050;
        check(&mut vm, &mut host);
        assert_eq!(host.console, ["map_restart"]);

        call(&mut vm, &mut host, 0, "callvote map_rotate");
        host.level_time_ms = 34_000;
        assert!(check(&mut vm, &mut host).is_empty());
        host.level_time_ms = 34_050;
        assert_eq!(check(&mut vm, &mut host), ["e \"GAME_VOTEFAILED\""]);
    }

    /// A map vote for a map the server lacks is refused on 1.1 as on 1.5,
    /// where retail 1.1 passes it and the load then fails.
    #[test]
    fn a_map_vote_needs_the_map() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis"]);
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote map mp_nowhere"),
            [(Some(0), NO_MAP.into())]
        );
        assert_eq!(host.configstrings[15], "");
    }

    /// Bots are not voters: one human against two bots passes on the
    /// caller's own yes.
    #[test]
    fn bots_do_not_hold_a_vote() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "allies", "axis"]);
        for slot in 1..3 {
            host.client_roster[slot].as_mut().unwrap().bot = true;
        }
        call(&mut vm, &mut host, 0, "callvote map_restart");
        assert_eq!(check(&mut vm, &mut host), ["e \"GAME_VOTEPASSED\""]);
    }

    /// The 1.1d complaint run (cod11-votes.md section 3): a no withdraws,
    /// a closed window falls through to the vote, two yeses warn the
    /// offender with the count left and the third drops them; an offender
    /// leaving shuts the window with `m -2`.
    #[test]
    fn a_complaint_warns_then_drops_as_11d_did() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "axis"]);
        host.cvars.set("g_complaintlimit", "3");
        host.level_time_ms = 1000;
        let warn = |n: i32| {
            (
                Some(0),
                format!(
                    "e \"\u{15}^1\u{14}GAME_WARNING\u{15}^7: \u{14}GAME_COMPLAINTFILEDAGAINST\u{15}{n}\""
                ),
            )
        };
        let m = |s: &str| vec![(Some(1), s.to_string())];
        host.allow_complaint(1, 1);
        assert!(host.client_commands.is_empty());
        host.allow_complaint(1, 0);
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(1, "m 0".into())]
        );
        assert_eq!(vote(&mut vm, &mut host, 1, "no"), m("m -2"));
        assert_eq!(
            vote(&mut vm, &mut host, 1, "yes"),
            [to(1, "GAME_NOVOTEINPROGRESS")]
        );
        for left in [2, 1] {
            host.allow_complaint(1, 0);
            assert_eq!(
                vote(&mut vm, &mut host, 1, "yes"),
                [warn(left), (Some(1), "m -1".into())]
            );
        }
        host.allow_complaint(1, 0);
        assert_eq!(vote(&mut vm, &mut host, 1, "yes"), m("m -1"));
        assert_eq!(host.complaint_drop, Some(0));

        // The window shuts 20.5 s on, and goes with the offender.
        host.client_commands.clear();
        host.allow_complaint(1, 0);
        host.level_time_ms += 20_500;
        assert_eq!(
            vote(&mut vm, &mut host, 1, "yes"),
            [to(1, "GAME_NOVOTEINPROGRESS")]
        );
        host.allow_complaint(1, 0);
        host.client_commands.clear();
        host.complaints_about_gone(0);
        assert_eq!(host.client_commands, [(1, "m -2".into())]);
        assert_eq!(
            vote(&mut vm, &mut host, 1, "yes"),
            [to(1, "GAME_NOVOTEINPROGRESS")]
        );
    }

    /// `ClientDisconnect` clears one window, the lowest slot's: 1.1 takes a
    /// slot since left, 1.5 and UO only a connected one (1.1d 0x42baa,
    /// UO 0x45602).
    #[test]
    fn an_offender_leaving_clears_the_lowest_window_only() {
        for v15 in [false, true] {
            let mut vm = vcod_gsc::Vm::new();
            let mut host = host(&mut vm, &["axis", "allies", "allies", "allies"]);
            host.patch_1_5 = v15;
            for slot in 1..4 {
                host.allow_complaint(slot, 0);
            }
            host.client_roster[1] = None;
            host.client_commands.clear();
            host.complaints_about_gone(0);
            let cleared = if v15 { 2 } else { 1 };
            assert_eq!(
                host.client_commands,
                [(cleared, "m -2".into())],
                "1.5 {v15}"
            );
            assert_eq!(host.client_complaints[3].against, 0, "1.5 {v15}");
        }
    }

    /// With three voters 1.1 fails on the first no and 1.5 waits for two.
    #[test]
    fn three_voters_split_the_two_versions() {
        for (v15, after_one_no) in [(false, true), (true, false)] {
            let mut vm = vcod_gsc::Vm::new();
            let mut host = host(&mut vm, &["axis", "allies", "axis"]);
            host.patch_1_5 = v15;
            call(&mut vm, &mut host, 0, "callvote map_restart");
            vote(&mut vm, &mut host, 1, "no");
            assert_eq!(
                !check(&mut vm, &mut host).is_empty(),
                after_one_no,
                "1.5 {v15}"
            );
        }
    }

    /// 1.5's per-kind gates and its settings votes, as the 1.5b capture
    /// answered them.
    #[test]
    fn a_15_vote_reads_its_own_gate() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "allies"]);
        host.patch_1_5 = true;
        for (name, value) in [
            ("g_allowVoteClientKick", "0"),
            ("g_allowVoteKillCam", "1"),
            ("g_allowVoteFriendlyFire", "1"),
            ("g_allowVoteDrawFriend", "1"),
            ("scr_killcam", "1"),
            ("scr_friendlyfire", "0"),
            ("scr_drawfriend", "0"),
        ] {
            host.cvars.set(name, value);
        }
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote clientkick 1"),
            [to(0, "GAME_VOTINGNOTENABLED")]
        );
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote killcam 1"),
            [to(0, "PATCH_1_5_KILLCAM_NOCHANGE")]
        );
        assert_eq!(
            call(&mut vm, &mut host, 0, "callvote friendlyfire 5"),
            [to(0, "PATCH_1_5_VOTE_ARG_NOTVALID\u{15}5")]
        );
        call(&mut vm, &mut host, 0, "callvote drawfriend 7");
        assert_eq!(
            &host.configstrings[15..17],
            ["30000", "PATCH_1_5_VOTE_DRAWFRIEND\u{14}MENU_OFF"]
        );
        assert_eq!(host.vote.command, "setdrawfriend 7");
    }
}
