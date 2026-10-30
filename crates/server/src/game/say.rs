//! `G_Say` (`game.mp.i386.so` 0x46c14): the `h`/`i` chat line one speaker's
//! text becomes, and who it reaches. The client's `say`, `say_team` and
//! `tell` and the script's `sayAll` and `sayTeam` all end here
//! (docs/research/cod11-chat.md).

use crate::game::host::GameHost;
use vcod_gsc::Cx;

/// The modes `G_Say` takes as its third argument. `Squad` is 1.5's
/// `say_squad`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SayMode {
    All,
    Team,
    Tell,
    Squad,
}

/// `Q_strncpyz(text, 0x96)`: the message keeps 149 bytes.
const MAX_SAY_TEXT: usize = 149;

impl GameHost {
    /// `G_Say(speaker, target, mode, text)`: queues the chat line for every
    /// recipient. A team say from a speaker on neither side is a plain say.
    /// `target` is `tell`'s one recipient; `None` walks every client.
    pub fn say(
        &mut self,
        cx: &mut Cx,
        speaker: usize,
        target: Option<usize>,
        mode: SayMode,
        text: &str,
    ) {
        let team = self.client_field(cx, speaker, "sessionteam");
        let team = team.as_deref().unwrap_or("");
        let squad = self.client_squad.get(speaker).copied().unwrap_or(0);
        let mode = match mode {
            SayMode::Team if !matches!(team, "axis" | "allies") => SayMode::All,
            // 1.5's `G_Say` (0x42dc9) sends a squad say from a speaker in
            // no squad to the team, without the plain-say fallback above.
            SayMode::Squad if squad == 0 => SayMode::Team,
            m => m,
        };
        let name = clean_name(self.client_names.get(speaker).map_or("", String::as_str));
        // CoD4's `G_Say` (0x80ae962) logs a plain or team line before it
        // sends anything (`say;` 0x821957d, `sayteam;` 0x821958e); a tell
        // writes none (VERIFIED live, games_mp.log).
        let word = match mode {
            SayMode::All => "say",
            SayMode::Team => "sayteam",
            SayMode::Squad => "saysquad",
            SayMode::Tell => "",
        };
        if mode == SayMode::Tell {
        } else if self.profile.game == vcod_common::game::Game::Cod4 {
            let (who, guid) = crate::game::cod4::log_client(self, speaker);
            crate::game::cod4::log_printf(self, format!("{word};{guid};{speaker};{who};{text}\n"));
        } else if self.logs_1_1() {
            // 1.1's `G_Say` (0x46c14): `say: %s: %s` or `sayteam: %s: %s`
            // with the cleaned name (cod11-chat.md).
            self.log_printf(format!("{word}: {name}: {text}\n"));
        } else {
            // 1.5's, UO's and CoD2 1.3's `%s;%d;%d;%s;%s` (cod2_lnxded
            // 0x110f1e): the GUID, 0 with no authorize, the slot, the
            // cleaned name, the text (cod11-map-cycle.md 9).
            self.log_printf(format!("{word};0;{speaker};{name};{text}\n"));
        }
        let speaker_playing = self.playing(cx, speaker);
        // CoD4 colours the name by the recipient's side instead: `^8` on the
        // speaker's team, `^9` off it (the walk capture's teammate reads
        // `^8walker`, the spectator `^9walker`, VERIFIED live); the line is
        // built with `^8` and recoloured per recipient below.
        let cod4 = self.profile.game == vcod_common::game::Game::Cod4;
        let status = if team == "spectator" {
            "\u{15}(\u{14}GAME_SPECTATOR\u{15})".to_string()
        } else if self.patch_1_5 {
            // 1.5 colours the name by side and reads death off the session
            // state, not the health.
            let side = match team {
                "axis" | "allies" if cod4 => "^8",
                "axis" => "^9",
                "allies" => "^8",
                _ => "",
            };
            let dead = if speaker_playing {
                ""
            } else {
                "(\u{14}GAME_DEAD\u{15})"
            };
            format!("\u{15}{side}{dead}")
        } else if self.client_vitals.get(speaker).map_or(0, |v| v.health) < 1 {
            "\u{15}(\u{14}GAME_DEAD\u{15})".to_string()
        } else {
            "\u{15}".to_string()
        };
        // Team_GetLocationMsg finds no `target_location` in any stock map, so
        // the `(location)` forms are left out.
        let (head, colour) = match mode {
            SayMode::Team | SayMode::Squad => {
                let side = if team == "axis" {
                    "GAME_AXIS"
                } else {
                    "GAME_ALLIES"
                };
                if mode == SayMode::Squad {
                    let label = if squad == 1 {
                        "PATCH_1_5_SQUADALPHA"
                    } else {
                        "PATCH_1_5_SQUADBRAVO"
                    };
                    let head =
                        format!("{status}(\u{14}{side}\u{15} \u{14}{label}\u{15}){name}^7: ");
                    (head, '5')
                } else {
                    (format!("{status}(\u{14}{side}\u{15}){name}^7: "), '5')
                }
            }
            SayMode::All => (format!("{status}{name}^7: "), '7'),
            SayMode::Tell => (format!("{status}[{name}]^7: "), '3'),
        };
        let body = truncate(text, MAX_SAY_TEXT);
        log::info!("say: {name}: {body}");
        let letter = if mode == SayMode::Team { 'i' } else { 'h' };
        let cmd = format!("{letter} \"\u{15}{head}^{colour}{body}\"");
        // 1.5's `g_deadChat` lets the dead and the spectators reach everyone.
        let dead_chat =
            self.patch_1_5 && crate::game::builtins::cvar::atoi(self.cvars.get("g_deadChat")) != 0;
        let recipients: Vec<usize> = match target {
            Some(t) => vec![t],
            None => (0..self.client_roster.len()).collect(),
        };
        for slot in recipients {
            if !self
                .client_roster
                .get(slot)
                .copied()
                .flatten()
                .is_some_and(|r| r.active)
            {
                continue;
            }
            if mode == SayMode::Team && !self.same_team(cx, speaker, slot) {
                continue;
            }
            // `InSameSquad` (1.5b 0x69987): the same team and the same squad.
            if mode == SayMode::Squad
                && !(self.same_team(cx, speaker, slot)
                    && self.client_squad.get(slot).copied() == Some(squad))
            {
                continue;
            }
            // A dead or spectating speaker reaches only the other clients
            // out of play, unless 1.5's `g_deadChat` is on.
            if !speaker_playing && !dead_chat && self.playing(cx, slot) {
                continue;
            }
            let line = if cod4
                && matches!(team, "axis" | "allies")
                && !self.same_team(cx, speaker, slot)
            {
                cmd.replacen("\u{15}\u{15}^8", "\u{15}\u{15}^9", 1)
            } else {
                cmd.clone()
            };
            self.client_commands.push((slot, line));
        }
    }

    /// `sessionState == SESS_PLAYING`, read off `.sessionstate` the way the
    /// follow pass reads it.
    pub(crate) fn playing(&mut self, cx: &mut Cx, slot: usize) -> bool {
        !matches!(
            self.client_field(cx, slot, "sessionstate").as_deref(),
            Some("spectator" | "dead" | "intermission")
        )
    }

    /// `OnSameTeam` (0x64aec): both on one team, and that team not 0.
    fn same_team(&mut self, cx: &mut Cx, a: usize, b: usize) -> bool {
        let ta = self.client_field(cx, a, "sessionteam");
        let tb = self.client_field(cx, b, "sessionteam");
        match (ta, tb) {
            (Some(ta), Some(tb)) => {
                ta == tb && matches!(ta.as_str(), "axis" | "allies" | "spectator")
            }
            _ => false,
        }
    }
}

/// A name `Q_CleanStr`'d into 64 bytes, as `G_Say` reads it
/// (cod11-chat.md): 63 and the terminator.
pub(crate) fn clean_name(name: &str) -> String {
    let mut out = vcod_common::text::clean_str(name);
    out.truncate(63);
    out
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A client command's arguments joined the way `Cmd_Say_f` (0x47050) joins
/// `argv[1..]`: tokenized, one space between, capped at 0x3fe bytes. `None`
/// with no argument at all, which `Cmd_Say_f` ignores; `say ""` says nothing
/// out loud but still goes out.
pub fn concat_args(args: &str) -> Option<String> {
    let tokens = tokenize(args);
    if tokens.is_empty() {
        return None;
    }
    let mut out = String::new();
    for tok in tokens {
        if out.len() + tok.len() > 0x3fe {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&tok);
    }
    Some(out)
}

/// `Cmd_TokenizeString` (cod_lnxded 0x805b398, the same in 1.5b and UO),
/// what a client command, an rcon line, a console line and a query's line
/// are split with: any byte up to a space or past 127 (a signed `char`)
/// separates, a `"` opens a token that runs to the next `"` or the end,
/// `//` ends the line and `/* */` is skipped, whose `*` may close it
/// (`/*/`). Inside quotes neither comment counts. At most 256 tokens
/// (docs/research/cod11-chat.md, section 1).
pub(crate) fn tokenize(s: &str) -> Vec<String> {
    const MAX_TOKENS: usize = 256;
    let b = s.as_bytes();
    let gap = |c: u8| c <= b' ' || c > 127;
    let comment = |i: usize| b[i] == b'/' && matches!(b.get(i + 1), Some(b'/' | b'*'));
    let mut tokens = Vec::new();
    let mut i = 0;
    while tokens.len() < MAX_TOKENS {
        while i < b.len() && gap(b[i]) {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if comment(i) {
            if b[i + 1] == b'/' {
                break;
            }
            // From the `*`: the first `*/` after it, the `*` itself counting.
            match b[i + 1..].windows(2).position(|w| w == b"*/") {
                Some(p) => i += 1 + p + 2,
                None => break,
            }
            continue;
        }
        // Every boundary below is an ASCII byte, so the slices are `char` aligned.
        if b[i] == b'"' {
            let start = i + 1;
            i = start;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            tokens.push(s[start..i].to_string());
            i += 1;
        } else {
            let start = i;
            while i < b.len() && !gap(b[i]) && b[i] != b'"' && !comment(i) {
                i += 1;
            }
            tokens.push(s[start..i].to_string());
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::scoreboard::RosterSlot;
    use vcod_gsc::{Host, Value};

    /// Clients in slots 0..teams.len(), in the world, on those teams and
    /// playing, each named `p<slot>`.
    fn host(vm: &mut vcod_gsc::Vm, teams: &[&str]) -> GameHost {
        let mut host = GameHost::new(&crate::game::profile::COD1_11, vec![String::new(); 2048]);
        for (slot, team) in teams.iter().enumerate() {
            vm.with_cx(|cx| {
                let id = host.ents.spawn_client(cx, slot, None).expect("a client");
                set(&mut host, cx, slot, "sessionteam", team);
                set(&mut host, cx, slot, "sessionstate", "playing");
                id
            });
            host.client_roster[slot] = Some(RosterSlot {
                active: true,
                ..Default::default()
            });
            host.client_names[slot] = format!("^1p{slot}");
            host.client_vitals[slot].health = 100;
        }
        host
    }

    fn set(host: &mut GameHost, cx: &mut Cx, slot: usize, field: &str, value: &str) {
        let ent = host.ents.handle(slot as u32).expect("a client entity");
        let atom = cx.intern_folded(field);
        let value = Value::String(cx.intern_exact(value));
        host.set_field(cx, ent, atom, value)
            .expect("a client field");
    }

    fn say(vm: &mut vcod_gsc::Vm, host: &mut GameHost, slot: usize, mode: SayMode, text: &str) {
        vm.with_cx(|cx| host.say(cx, slot, None, mode, text));
    }

    /// The lines the retail captures in docs/research/cod11-chat.md hold.
    #[test]
    fn a_say_reaches_everyone_and_a_team_say_its_side() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["allies", "axis", "allies"]);
        say(&mut vm, &mut host, 0, SayMode::All, "hi");
        let all = "h \"\u{15}\u{15}p0^7: ^7hi\"".to_string();
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(0, all.clone()), (1, all.clone()), (2, all)]
        );
        say(&mut vm, &mut host, 0, SayMode::Team, "go");
        let team = "i \"\u{15}\u{15}(\u{14}GAME_ALLIES\u{15})p0^7: ^5go\"".to_string();
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(0, team.clone()), (2, team)]
        );
    }

    #[test]
    fn a_speaker_out_of_play_reaches_only_others_out_of_play() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["allies", "axis", "spectator"]);
        vm.with_cx(|cx| {
            set(&mut host, cx, 0, "sessionstate", "dead");
            set(&mut host, cx, 2, "sessionstate", "spectator");
        });
        host.client_vitals[0].health = 0;
        say(&mut vm, &mut host, 0, SayMode::All, "x");
        let dead = "h \"\u{15}\u{15}(\u{14}GAME_DEAD\u{15})p0^7: ^7x\"".to_string();
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(0, dead.clone()), (2, dead)]
        );
        // A spectator's team say is a plain say.
        say(&mut vm, &mut host, 2, SayMode::Team, "y");
        let spec = "h \"\u{15}\u{15}(\u{14}GAME_SPECTATOR\u{15})p2^7: ^7y\"".to_string();
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(0, spec.clone()), (2, spec)]
        );
    }

    /// `G_Say`'s log line: 1.1's `say: %s: %s` and `sayteam: %s: %s` with
    /// the cleaned name (the retail 1.1d run, cod11-map-cycle.md 9); 1.5's
    /// `say;%d;%d;%s;%s` with GUID 0 and the slot; a tell writes none here.
    #[test]
    fn a_say_logs_the_patch_s_line() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["allies", "axis"]);
        host.level_time_ms = 19_000;
        say(&mut vm, &mut host, 0, SayMode::All, "hello log");
        say(&mut vm, &mut host, 0, SayMode::Team, "team log");
        vm.with_cx(|cx| host.say(cx, 0, Some(0), SayMode::Tell, "private"));
        assert_eq!(
            std::mem::take(&mut host.game_log),
            [
                (19_000, "say: p0: hello log\n".to_string()),
                (19_000, "sayteam: p0: team log\n".to_string())
            ]
        );
        host.patch_1_5 = true;
        say(&mut vm, &mut host, 1, SayMode::Team, "go");
        assert_eq!(
            std::mem::take(&mut host.game_log),
            [(19_000, "sayteam;0;1;p1;go\n".to_string())]
        );
    }

    /// 1.5b's line for a playing axis speaker, from the live capture
    /// (cod15-protocol-diff.md, section 13): the side's colour ahead of the
    /// name.
    #[test]
    fn a_15_say_colours_the_side() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "allies"]);
        host.patch_1_5 = true;
        host.client_names[0] = "vcod".into();
        say(&mut vm, &mut host, 0, SayMode::All, "hi");
        let line = "h \"\u{15}\u{15}^9vcod^7: ^7hi\"".to_string();
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(0, line.clone()), (1, line)]
        );
        // A squad say from a speaker in no squad is a team say.
        say(&mut vm, &mut host, 1, SayMode::Squad, "go");
        let team = "i \"\u{15}\u{15}^8(\u{14}GAME_ALLIES\u{15})p1^7: ^5go\"".to_string();
        assert_eq!(std::mem::take(&mut host.client_commands), [(1, team)]);
    }

    /// A speaker in a squad (`sessionsquad`, 1.5b 0x3b515) says to his
    /// squad on his side only, as an `h` line with the squad after the side
    /// (`G_Say` 0x42dc9, format 0x787d0; `InSameSquad` 0x69987). On 1.1 the
    /// field is a plain script variable.
    #[test]
    fn a_15_squad_say_reaches_the_squad_on_the_side() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "axis", "axis", "allies"]);
        host.patch_1_5 = true;
        host.set_profile(&crate::game::profile::COD1_15);
        vm.with_cx(|cx| {
            for (slot, squad) in [(0, "squad_bravo"), (1, "squad_alpha"), (2, "squad_bravo")] {
                set(&mut host, cx, slot, "sessionsquad", squad);
            }
            set(&mut host, cx, 3, "sessionsquad", "squad_bravo");
            let field = cx.intern_folded("sessionsquad");
            let ent = host.ents.handle(1).unwrap();
            assert_eq!(
                host.get_field(cx, ent, field),
                Value::String(cx.intern_exact("squad_alpha"))
            );
            let bad = Value::String(cx.intern_exact("squad_charlie"));
            assert!(host.set_field(cx, ent, field, bad).is_err());
        });
        say(&mut vm, &mut host, 0, SayMode::Squad, "go");
        let line = "h \"\u{15}\u{15}^9(\u{14}GAME_AXIS\u{15} \u{14}PATCH_1_5_SQUADBRAVO\u{15})p0^7: ^5go\""
            .to_string();
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(0, line.clone()), (2, line)]
        );
        // `ClientConnect` clears it (1.5b 0x4c680).
        vm.with_cx(|cx| {
            let ent = host.connect_client(cx, 0, "p0").unwrap();
            let field = cx.intern_folded("sessionsquad");
            assert_eq!(
                host.get_field(cx, ent, field),
                Value::String(cx.intern_exact("none"))
            );
        });
    }

    /// 1.5 reads death off the session state, and `g_deadChat` lets the dead
    /// reach the living.
    #[test]
    fn a_15_dead_speaker_and_dead_chat() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["axis", "allies"]);
        host.patch_1_5 = true;
        host.cvars = crate::cvars::Cvars::for_protocol(6);
        vm.with_cx(|cx| set(&mut host, cx, 0, "sessionstate", "dead"));
        say(&mut vm, &mut host, 0, SayMode::All, "x");
        let dead = "h \"\u{15}\u{15}^9(\u{14}GAME_DEAD\u{15})p0^7: ^7x\"".to_string();
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(0, dead.clone())]
        );
        host.cvars.set("g_deadChat", "1");
        say(&mut vm, &mut host, 0, SayMode::All, "x");
        assert_eq!(
            std::mem::take(&mut host.client_commands),
            [(0, dead.clone()), (1, dead)]
        );
    }

    #[test]
    fn a_client_not_yet_in_the_world_hears_nothing() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["allies", "allies"]);
        host.client_roster[1] = Some(RosterSlot::default());
        say(&mut vm, &mut host, 0, SayMode::All, "hi");
        assert_eq!(host.client_commands.len(), 1);
    }

    /// What retail 1.1d, 1.5b and UO 1.51b put on the chat line for a
    /// client's `say` (cod11-chat.md, section 1).
    #[test]
    fn say_text_loses_comments_as_retail_did() {
        let cases = [
            ("http://x", Some("http:")),
            ("a /* b */ c", Some("a c")),
            ("a/*b", Some("a")),
            ("\"x//y\" z", Some("x//y z")),
            ("a /*/ b", Some("a b")),
            ("a.. b", Some("a.. b")),
            ("a;b\\c", Some("a;b\\c")),
            ("a//", Some("a")),
            ("//a", None),
            ("a/b /c", Some("a/b /c")),
            ("a*/b", Some("a*/b")),
            ("\"a b\" \"c", Some("a b c")),
            ("a%b", Some("a%b")),
        ];
        for (text, line) in cases {
            assert_eq!(concat_args(text).as_deref(), line, "{text:?}");
        }
    }

    #[test]
    fn a_byte_past_127_separates_and_tokens_stop_at_256() {
        assert_eq!(tokenize("a\u{e9}b \"\u{e9}\""), ["a", "b", "\u{e9}"]);
        assert_eq!(tokenize(&"x ".repeat(300)).len(), 256);
    }

    #[test]
    fn say_arguments_join_like_cmd_say_f() {
        assert_eq!(concat_args(""), None);
        assert_eq!(concat_args("   "), None);
        assert_eq!(
            concat_args("a   b \"c d\" 100%").as_deref(),
            Some("a b c d 100%")
        );
        assert_eq!(concat_args("\"\"").as_deref(), Some(""));
    }
}
