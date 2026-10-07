//! `G_Say` (`game.mp.i386.so` 0x46c14): the `h`/`i` chat line one speaker's
//! text becomes, and who it reaches. The client's `say`, `say_team` and
//! `tell` and the script's `sayAll` and `sayTeam` all end here
//! (docs/research/cod11-chat.md).

use crate::game::host::GameHost;
use vcod_gsc::Cx;

/// The three modes `G_Say` takes as its third argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SayMode {
    All,
    Team,
    Tell,
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
        let mode = match mode {
            SayMode::Team if !matches!(team, "axis" | "allies") => SayMode::All,
            m => m,
        };
        let name = clean_name(self.client_names.get(speaker).map_or("", String::as_str));
        let health = self.client_vitals.get(speaker).map_or(0, |v| v.health);
        let status = if team == "spectator" {
            "\u{15}(\u{14}GAME_SPECTATOR\u{15})"
        } else if health < 1 {
            "\u{15}(\u{14}GAME_DEAD\u{15})"
        } else {
            "\u{15}"
        };
        // Team_GetLocationMsg finds no `target_location` in any stock map, so
        // the `(location)` forms are left out.
        let (head, colour) = match mode {
            SayMode::Team => {
                let side = if team == "axis" {
                    "GAME_AXIS"
                } else {
                    "GAME_ALLIES"
                };
                (format!("{status}(\u{14}{side}\u{15}){name}^7: "), '5')
            }
            SayMode::All => (format!("{status}{name}^7: "), '7'),
            SayMode::Tell => (format!("{status}[{name}]^7: "), '3'),
        };
        let body = truncate(text, MAX_SAY_TEXT);
        log::info!("say: {name}: {body}");
        let letter = if mode == SayMode::Team { 'i' } else { 'h' };
        let cmd = format!("{letter} \"\u{15}{head}^{colour}{body}\"");
        let speaker_playing = self.playing(cx, speaker);
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
            // A dead or spectating speaker reaches only the other clients
            // out of play.
            if !speaker_playing && self.playing(cx, slot) {
                continue;
            }
            self.client_commands.push((slot, cmd.clone()));
        }
    }

    /// `sessionState == SESS_PLAYING`, read off `.sessionstate` the way the
    /// follow pass reads it.
    fn playing(&mut self, cx: &mut Cx, slot: usize) -> bool {
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

/// `Q_CleanStr`: colour codes and anything unprintable out.
fn clean_name(name: &str) -> String {
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|&n| n != '^') {
            chars.next();
        } else if (' '..='~').contains(&c) {
            out.push(c);
        }
    }
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

/// `Cmd_TokenizeString` for a client command's arguments: whitespace splits,
/// a `"` opens a token that runs to the next `"`.
pub(crate) fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c <= ' ' {
            chars.next();
        } else if c == '"' {
            chars.next();
            tokens.push(chars.by_ref().take_while(|&c| c != '"').collect());
        } else {
            let mut tok = String::new();
            while let Some(&c) = chars.peek().filter(|&&c| c > ' ' && c != '"') {
                tok.push(c);
                chars.next();
            }
            tokens.push(tok);
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
        let mut host = GameHost::new(vec![String::new(); 2048]);
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

    #[test]
    fn a_client_not_yet_in_the_world_hears_nothing() {
        let mut vm = vcod_gsc::Vm::new();
        let mut host = host(&mut vm, &["allies", "allies"]);
        host.client_roster[1] = Some(RosterSlot::default());
        say(&mut vm, &mut host, 0, SayMode::All, "hi");
        assert_eq!(host.client_commands.len(), 1);
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
