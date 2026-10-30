//! 1.5's match timeout: the `matchtimeout` and `matchtimein` client
//! commands and the frame check that ends one (`game.mp.i386.so` 1.5b
//! `Cmd_MatchTimeout_f` 0x45a91, `Cmd_MatchTimein_f` 0x45f42,
//! `CheckMatchTimeout` 0x4fea8; docs/research/cod15-protocol-diff.md,
//! section 12). The pause itself is `timescale 0`, which the engine reads
//! when it steps the game clock (`Server::game_step_ms`).

use crate::game::host::GameHost;
use vcod_gsc::Cx;

/// Configstring 1628: the milliseconds the banner counts down from, empty
/// once play resumes.
pub const CS_TIMEOUT_MS: usize = 1628;
/// Configstring 1629: the banner's message.
pub const CS_TIMEOUT_TEXT: usize = 1629;

/// `level`'s timeout block (0x1744..0x1760), zeroed with `level` by
/// `G_InitGame`, which then fills both banks from `g_timeoutBank`.
#[derive(Clone, Debug, Default)]
pub struct MatchTimeout {
    /// Milliseconds of timeout left per side, allies then axis.
    bank: [i32; 2],
    /// Timeouts called per side, allies then axis.
    called: [i32; 2],
    /// The running timeout's length; 0 when none runs.
    length_ms: i32,
    /// `trap_Milliseconds` at the call.
    started_ms: i32,
    /// When the recovery ends; 0 when none runs.
    recovery_until_ms: i32,
    /// The calling side as `sessionTeam` numbers it: 1 axis, 2 allies.
    team: i32,
}

impl MatchTimeout {
    /// `G_InitGame`'s two writes.
    pub fn new(bank_ms: i32) -> Self {
        Self {
            bank: [bank_ms; 2],
            ..Self::default()
        }
    }
}

fn int(host: &GameHost, name: &str) -> i32 {
    crate::game::builtins::cvar::atoi(host.cvars.get(name))
}

/// `sessionTeam` as the game module numbers it.
fn team_number(team: Option<&str>) -> i32 {
    match team {
        Some("axis") => 1,
        Some("allies") => 2,
        Some("spectator") => 3,
        _ => 0,
    }
}

impl GameHost {
    fn set_timeout_configstrings(&mut self, ms: &str, text: Option<&str>) {
        let at = self
            .allocators
            .layout()
            .match_timeout
            .unwrap_or(CS_TIMEOUT_MS);
        self.configstrings.set(at, ms);
        if let Some(t) = text {
            self.configstrings.set(at + 1, t);
        }
    }

    /// Whether `slot` is playing on a side, the test both commands open
    /// with (`sessionState == SESS_PLAYING`, `sessionTeam != TEAM_SPECTATOR`).
    fn timeout_side(&mut self, cx: &mut Cx, slot: usize) -> Option<i32> {
        let state = self.client_field(cx, slot, "sessionstate");
        let team = team_number(self.client_field(cx, slot, "sessionteam").as_deref());
        (state.as_deref() == Some("playing") && team != 3).then_some(team)
    }

    /// `Cmd_MatchTimeout_f`. Returns the server commands in send order:
    /// `Some(slot)` to the caller, `None` to everyone. The configstrings
    /// and `timescale` are written here; the caller sends the configstring
    /// updates ahead of the commands, as `trap_SetConfigstring` does.
    pub fn match_timeout(
        &mut self,
        cx: &mut Cx,
        slot: usize,
        now_ms: i32,
    ) -> Vec<(Option<usize>, String)> {
        let reply = |m: &str| vec![(Some(slot), format!("e \"{m}\""))];
        if int(self, "g_timeoutsAllowed") < 1 || int(self, "g_timeoutLength") < 1 {
            return reply("PATCH_1_5_TIMEOUT_NOTENABLED");
        }
        let Some(team) = self.timeout_side(cx, slot) else {
            return reply("PATCH_1_5_TIMEOUT_MUSTBEPLAYING");
        };
        if self.timeout.length_ms != 0 || self.timeout.recovery_until_ms != 0 {
            return reply("PATCH_1_5_TIMEOUT_ALREADYINPROGRESS");
        }
        let (side, name) = match team {
            2 => (0, "GAME_ALLIES"),
            1 => (1, "GAME_AXIS"),
            _ => return reply("PATCH_1_5_TIMEOUT_INVALIDGAMETYPE"),
        };
        if int(self, "g_timeoutsAllowed") <= self.timeout.called[side] {
            return vec![(
                Some(slot),
                format!("e \"PATCH_1_5_TIMEOUT_MAXCALLED\u{14}{name}\""),
            )];
        }
        if self.timeout.bank[side] < 1 {
            return vec![(
                Some(slot),
                format!("e \"PATCH_1_5_TIMEOUT_MAXTIMEUSED\u{14}{name}\""),
            )];
        }
        self.timeout.called[side] += 1;
        self.timeout.team = team;
        self.timeout.length_ms = self.timeout.bank[side].min(int(self, "g_timeoutLength"));
        self.timeout.started_ms = now_ms;
        self.cvars.set("timescale", "0");
        let length = self.timeout.length_ms.to_string();
        let text = format!("PATCH_1_5_TIMEOUT_CALLED\u{14}{name}\u{15}");
        self.set_timeout_configstrings(&length, Some(&text));
        let who = self.timeout_name(slot);
        vec![(
            None,
            format!("e \"PATCH_1_5_TIMEOUT_CALLED_PLAYERNAME\u{15}{who}\""),
        )]
    }

    /// `Cmd_MatchTimein_f`: the calling side ends its timeout early and
    /// the recovery starts. Retail charges the allied bank whichever side
    /// called, both arms of its team test subtracting from 0x1744.
    pub fn match_timein(
        &mut self,
        cx: &mut Cx,
        slot: usize,
        now_ms: i32,
    ) -> Vec<(Option<usize>, String)> {
        let reply = |m: &str| vec![(Some(slot), format!("e \"{m}\""))];
        if int(self, "g_timeoutsAllowed") < 1 {
            return reply("PATCH_1_5_TIMEOUT_NOTENABLED");
        }
        if self.timeout.length_ms == 0 {
            return reply("PATCH_1_5_TIMEOUT_NONEINPROGRESS");
        }
        let Some(team) = self.timeout_side(cx, slot) else {
            return reply("PATCH_1_5_TIMEIN_MUSTBEPLAYING");
        };
        if team != self.timeout.team {
            return reply("PATCH_1_5_TIMEIN_WRONGTEAM");
        }
        self.timeout.bank[0] -= now_ms.wrapping_sub(self.timeout.started_ms);
        self.timeout.length_ms = 0;
        let recovery = int(self, "g_timeoutRecovery");
        self.timeout.recovery_until_ms = now_ms.wrapping_add(recovery);
        self.set_timeout_configstrings(
            &recovery.to_string(),
            Some("PATCH_1_5_TIMEOUT_ENDING\u{15}"),
        );
        let who = self.timeout_name(slot);
        vec![(
            None,
            format!("e \"PATCH_1_5_TIMEIN_CALLED_PLAYERNAME\u{15}{who}\""),
        )]
    }

    /// `CheckMatchTimeout`, once a frame after the vote check: the end of a
    /// recovery puts `timescale` back to 1 and clears the banner's count;
    /// a timeout that ran its length out charges the caller's bank and
    /// starts the recovery. True when it wrote a configstring.
    pub fn check_match_timeout(&mut self, now_ms: i32) -> bool {
        let t = &mut self.timeout;
        if t.recovery_until_ms != 0 && t.recovery_until_ms < now_ms {
            t.recovery_until_ms = 0;
            self.set_timeout_configstrings("", None);
            self.cvars.set("timescale", "1");
            return true;
        }
        if t.length_ms == 0 || now_ms < t.started_ms.wrapping_add(t.length_ms) {
            return false;
        }
        let side = if t.team == 2 { 0 } else { 1 };
        t.bank[side] -= t.length_ms;
        t.length_ms = 0;
        let recovery = int(self, "g_timeoutRecovery");
        self.timeout.recovery_until_ms = now_ms.wrapping_add(recovery);
        self.set_timeout_configstrings(
            &recovery.to_string(),
            Some("PATCH_1_5_TIMEOUT_ENDING\u{15}"),
        );
        for slot in self.client_slots() {
            self.client_commands
                .push((slot, "e \"PATCH_1_5_TIMEOUT_EXPIRED\"".to_string()));
        }
        true
    }

    /// The caller's name as the banner line names it: `Q_CleanStr` of the
    /// session name.
    fn timeout_name(&self, slot: usize) -> String {
        crate::game::say::clean_name(self.client_names.get(slot).map_or("", String::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_gsc::{Host, Value};

    fn host(vm: &mut vcod_gsc::Vm, teams: &[&str]) -> GameHost {
        let mut host = GameHost::new(&crate::game::profile::COD1_11, vec![String::new(); 2048]);
        host.cvars = crate::cvars::Cvars::for_protocol(6);
        host.cvars.set("g_timeoutsAllowed", "2");
        host.cvars.set("g_timeoutLength", "15000");
        host.cvars.set("g_timeoutRecovery", "5000");
        host.timeout = MatchTimeout::new(180_000);
        for (slot, team) in teams.iter().enumerate() {
            vm.with_cx(|cx| {
                host.ents.spawn_client(cx, slot, None).expect("a client");
                for (field, value) in [("sessionteam", *team), ("sessionstate", "playing")] {
                    let ent = host.ents.handle(slot as u32).unwrap();
                    let atom = cx.intern_folded(field);
                    let v = Value::String(cx.intern_exact(value));
                    host.set_field(cx, ent, atom, v).unwrap();
                }
            });
            host.client_names[slot] = format!("^2p{slot}");
        }
        host
    }

    fn call(
        vm: &mut vcod_gsc::Vm,
        host: &mut GameHost,
        slot: usize,
        timein: bool,
        now: i32,
    ) -> Vec<(Option<usize>, String)> {
        vm.with_cx(|cx| {
            if timein {
                host.match_timein(cx, slot, now)
            } else {
                host.match_timeout(cx, slot, now)
            }
        })
    }

    /// The 1.5b capture's sequence: a call, a second call refused, the
    /// side's own timein, then the recovery ending on the clock.
    #[test]
    fn a_timeout_runs_as_the_15b_capture_did() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "allies"]);
        let out = call(&mut vm, &mut host, 0, false, 8000);
        assert_eq!(
            out,
            [(
                None,
                "e \"PATCH_1_5_TIMEOUT_CALLED_PLAYERNAME\u{15}p0\"".to_string()
            )]
        );
        assert_eq!(host.configstrings[CS_TIMEOUT_MS], "15000");
        assert_eq!(
            host.configstrings[CS_TIMEOUT_TEXT],
            "PATCH_1_5_TIMEOUT_CALLED\u{14}GAME_AXIS\u{15}"
        );
        assert_eq!(host.cvars.get("timescale"), "0");
        assert_eq!(
            call(&mut vm, &mut host, 0, false, 9000),
            [(
                Some(0),
                "e \"PATCH_1_5_TIMEOUT_ALREADYINPROGRESS\"".to_string()
            )]
        );
        assert_eq!(
            call(&mut vm, &mut host, 1, true, 10_000),
            [(Some(1), "e \"PATCH_1_5_TIMEIN_WRONGTEAM\"".to_string())]
        );
        let out = call(&mut vm, &mut host, 0, true, 22_000);
        assert_eq!(
            out,
            [(
                None,
                "e \"PATCH_1_5_TIMEIN_CALLED_PLAYERNAME\u{15}p0\"".to_string()
            )]
        );
        assert_eq!(host.configstrings[CS_TIMEOUT_MS], "5000");
        assert_eq!(
            host.configstrings[CS_TIMEOUT_TEXT],
            "PATCH_1_5_TIMEOUT_ENDING\u{15}"
        );
        // The axis call charged the allied bank, as retail's timein does.
        assert_eq!(host.timeout.bank, [180_000 - 14_000, 180_000]);
        assert_eq!(
            call(&mut vm, &mut host, 0, true, 24_000),
            [(
                Some(0),
                "e \"PATCH_1_5_TIMEOUT_NONEINPROGRESS\"".to_string()
            )]
        );
        assert!(!host.check_match_timeout(27_000));
        assert_eq!(host.cvars.get("timescale"), "0");
        assert!(host.check_match_timeout(27_001));
        assert_eq!(host.configstrings[CS_TIMEOUT_MS], "");
        assert_eq!(host.cvars.get("timescale"), "1");
    }

    /// A timeout left to run out charges its own side's bank, says so to
    /// everyone and starts the recovery; the side's allowance then runs out.
    #[test]
    fn a_timeout_that_runs_out_charges_its_side() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis"]);
        host.cvars.set("g_timeoutsAllowed", "1");
        call(&mut vm, &mut host, 0, false, 1000);
        assert!(!host.check_match_timeout(15_999));
        assert!(host.check_match_timeout(16_000));
        assert_eq!(host.timeout.bank, [180_000, 165_000]);
        assert_eq!(host.configstrings[CS_TIMEOUT_MS], "5000");
        assert_eq!(
            host.client_commands,
            [(0, "e \"PATCH_1_5_TIMEOUT_EXPIRED\"".to_string())]
        );
        assert!(host.check_match_timeout(21_001));
        assert_eq!(
            call(&mut vm, &mut host, 0, false, 30_000),
            [(
                Some(0),
                "e \"PATCH_1_5_TIMEOUT_MAXCALLED\u{14}GAME_AXIS\"".to_string()
            )]
        );
    }

    #[test]
    fn only_a_playing_side_calls_one() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["spectator", "none"]);
        assert_eq!(
            call(&mut vm, &mut host, 0, false, 0),
            [(Some(0), "e \"PATCH_1_5_TIMEOUT_MUSTBEPLAYING\"".to_string())]
        );
        assert_eq!(
            call(&mut vm, &mut host, 1, false, 0),
            [(
                Some(1),
                "e \"PATCH_1_5_TIMEOUT_INVALIDGAMETYPE\"".to_string()
            )]
        );
        host.cvars.set("g_timeoutsAllowed", "0");
        assert_eq!(
            call(&mut vm, &mut host, 1, false, 0),
            [(Some(1), "e \"PATCH_1_5_TIMEOUT_NOTENABLED\"".to_string())]
        );
    }
}
