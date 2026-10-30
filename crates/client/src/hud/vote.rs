//! The vote lines, top left (cgame 1.1 0x30018120, 1.5 0x300191b0): what
//! the vote is for and the seconds left, then the yes and no counts with
//! the keys bound to `vote yes` and `vote no` until this client has voted
//! (docs/research/cod11-votes.md, "The client").

use super::HudQuad;
use super::font::{Slot, UiFonts};
use super::hudelem::Virtual;
use super::player::menu_text_rgba;
use vcod_common::localize::Localized;
use vcod_common::text::atoi_msvc;

/// Font 0 at 0.2083 (0x3e555555), as the chat lines.
const SCALE: f32 = 0.208_333_33;
/// Yellow.
const COLOR: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
/// `ps.eFlags` 0x20000: this client has voted.
pub const EF_VOTED: i32 = 0x20000;

/// What `CG_ConfigStringModified` keeps of configstrings 15..18 (UO's
/// 16..19).
#[derive(Default)]
pub struct Vote {
    /// When the vote ends: 1.1's server time as sent, or for 1.5 and UO the
    /// local ms the sent count runs out at; 0 when no vote runs.
    end: i32,
    yes: i32,
    no: i32,
    /// The vote string, localized raw (`SEH_LocalizeTextMessage`).
    text: String,
    /// A time or count changed since the last draw: the draw plays
    /// `player_talk` and clears it.
    modified: bool,
    /// The four raw values last read, to tell a change.
    seen: Option<[String; 4]>,
}

impl Vote {
    /// A new gamestate: the next [`Self::update`] reads the slots as
    /// `CG_Init` does, without the sound.
    pub fn on_gamestate(&mut self) {
        self.seen = None;
    }

    /// `CG_MapRestart` zeroes `cgs.voteTime` (0x301d21c0) and nothing
    /// else, so the lines go until a vote slot changes.
    pub fn on_map_restart(&mut self) {
        self.end = 0;
    }

    /// Follows the four slots from `first`. `relative` is 1.5's and UO's
    /// count of ms left, which the cgame adds to `trap_Milliseconds`
    /// (`now_ms`) at its arrival.
    pub fn update(&mut self, configstrings: &[String], first: usize, relative: bool, now_ms: i32) {
        let cs: [String; 4] =
            std::array::from_fn(|i| configstrings.get(first + i).cloned().unwrap_or_default());
        let fresh = self.seen.is_none();
        let old = self.seen.take().unwrap_or_default();
        let changed = |i: usize| fresh || old[i] != cs[i];
        if changed(0) {
            let t = atoi_msvc(&cs[0]);
            self.end = if relative && t != 0 {
                t.wrapping_add(now_ms)
            } else {
                t
            };
        }
        if changed(2) {
            self.yes = atoi_msvc(&cs[2]);
        }
        if changed(3) {
            self.no = atoi_msvc(&cs[3]);
        }
        if changed(1) {
            self.text = cs[1].clone();
        }
        if !fresh && (changed(0) || changed(2) || changed(3)) {
            self.modified = true;
        }
        self.seen = Some(cs);
    }

    /// The two lines while a vote runs: `"%s(%i):%s"` over `CGAME_VOTE`,
    /// the whole seconds left and the vote string, then `"%s(%s):%i,
    /// %s(%s):%i"` over `CGAME_YES`, its key, the count, `CGAME_NO`, its
    /// key, the count, or `"%s:%i, %s:%i"` once `voted`. `now` is the
    /// clock `end` is on: the drawn server time on 1.1, local ms otherwise.
    /// `key` names the key bound to a command, `None` while unbound, which
    /// shows the command itself.
    pub fn lines(
        &self,
        loc: &Localized,
        now: i32,
        voted: bool,
        key: &dyn Fn(&str) -> Option<String>,
    ) -> Option<[String; 2]> {
        if self.end == 0 {
            return None;
        }
        let word = |k: &str| loc.get(k).unwrap_or(k).to_string();
        let secs = (self.end.wrapping_sub(now) / 1000).max(0);
        let first = format!("{}({secs}):{}", word("CGAME_VOTE"), loc.message(&self.text));
        let (yes, no) = (word("CGAME_YES"), word("CGAME_NO"));
        let second = if voted {
            format!("{yes}:{}, {no}:{}", self.yes, self.no)
        } else {
            let k = |cmd: &str| key(cmd).unwrap_or_else(|| cmd.to_string());
            format!(
                "{yes}({}):{}, {no}({}):{}",
                k("vote yes"),
                self.yes,
                k("vote no"),
                self.no
            )
        };
        Some([first, second])
    }

    /// Draws the lines at x 8, baselines 200 and 210 (0x43480000,
    /// 0x43520000). True when the draw owes `player_talk`.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        fonts: &UiFonts,
        loc: &Localized,
        now: i32,
        voted: bool,
        key: &dyn Fn(&str) -> Option<String>,
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) -> bool {
        let Some(lines) = self.lines(loc, now, voted, key) else {
            return false;
        };
        let font = fonts.pick(Slot::Default, SCALE, 480.0 * v.scale);
        for (line, y) in lines.iter().zip([200.0, 210.0]) {
            menu_text_rgba(font, line, (8.0, y), SCALE, COLOR, v, out);
        }
        std::mem::take(&mut self.modified)
    }
}

/// The teamkill complaint prompt the server's `m` opens (cgame 1.1 `m`
/// handler 0x3002e2c6, draw 0x300181fb; docs/research/cod11-votes.md 5).
/// While it is up the vote lines are not drawn.
#[derive(Default)]
pub struct Complaint {
    /// When the prompt goes, on the HUD's clock; 0 for none.
    end: i32,
    /// The client to complain about, or -1..-4 for an outcome line.
    client: i32,
}

impl Complaint {
    /// `m <n>`: a prompt about client `n` for 20 s, or a negative `n`'s
    /// outcome line for 10 s.
    pub fn on_command(&mut self, arg: &str, now: i32) {
        self.client = atoi_msvc(arg);
        let ms = if self.client < 0 { 10_000 } else { 20_000 };
        self.end = now.wrapping_add(ms);
    }

    /// `n` (0x3002ca60) takes the prompt down.
    pub fn clear(&mut self) {
        *self = Complaint::default();
    }

    /// Whether the prompt is up at `now`, the vote lines held back.
    pub fn up(&self, now: i32) -> bool {
        self.end.wrapping_sub(now) > 0
    }

    /// The lines while up: an outcome, or the question about a client
    /// `name` knows and the keys bound to `vote yes` and `vote no`. Empty
    /// for a client with no info, which retail also leaves blank.
    pub fn lines(
        &self,
        loc: &Localized,
        now: i32,
        name: &dyn Fn(i32) -> Option<String>,
        key: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Vec<String>> {
        if !self.up(now) {
            return None;
        }
        let word = |k: &str| loc.get(k).unwrap_or(k).to_string();
        let outcome = match self.client {
            -4 => Some("CGAME_SERVERHOSTTEAMKILLED"),
            -3 => Some("CGAME_COMPLAINTSERVERHOST"),
            -2 => Some("CGAME_COMPLAINTDISMISSED"),
            -1 => Some("CGAME_COMPLAINTFILED"),
            _ => None,
        };
        if let Some(k) = outcome {
            return Some(vec![word(k)]);
        }
        let Some(who) = name(self.client) else {
            return Some(Vec::new());
        };
        let k = |cmd: &str| key(cmd).unwrap_or_else(|| cmd.to_string());
        let ask = word("CGAME_COMPLAINTTEAMKILLFILE").replacen("%s", &who, 1);
        let keys = word("CGAME_PRESSYESNO")
            .replacen("%s", &k("vote yes"), 1)
            .replacen("%s", &k("vote no"), 1);
        Some(vec![ask, keys])
    }

    /// Draws the lines where the vote's go; true while up.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &self,
        fonts: &UiFonts,
        loc: &Localized,
        now: i32,
        name: &dyn Fn(i32) -> Option<String>,
        key: &dyn Fn(&str) -> Option<String>,
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) -> bool {
        let Some(lines) = self.lines(loc, now, name, key) else {
            return false;
        };
        let font = fonts.pick(Slot::Default, SCALE, 480.0 * v.scale);
        for (line, y) in lines.iter().zip([200.0, 210.0]) {
            menu_text_rgba(font, line, (8.0, y), SCALE, COLOR, v, out);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(time: &str, text: &str, yes: &str, no: &str) -> Vec<String> {
        let mut cs = vec![String::new(); 32];
        cs[15] = time.into();
        cs[16] = text.into();
        cs[17] = yes.into();
        cs[18] = no.into();
        cs
    }

    fn unbound(_: &str) -> Option<String> {
        None
    }

    /// The 1.1d capture's kick vote: 49650 at serverTime 19700.
    #[test]
    fn an_11_vote_counts_down_on_server_time() {
        let loc = Localized::default();
        let mut v = Vote::default();
        v.update(&cs("", "", "", ""), 15, false, 0);
        assert!(v.lines(&loc, 0, false, &unbound).is_none());
        v.update(
            &cs("49650", "GAME_VOTE_KICK\u{15}(1)Baker", "1", "0"),
            15,
            false,
            0,
        );
        let f1 = |c: &str| (c == "vote yes").then(|| "F1".to_string());
        let [a, b] = v.lines(&loc, 19700, false, &f1).unwrap();
        assert_eq!(a, "CGAME_VOTE(29):GAME_VOTE_KICK(1)Baker");
        assert_eq!(b, "CGAME_YES(F1):1, CGAME_NO(vote no):0");
        let [_, b] = v.lines(&loc, 19700, true, &f1).unwrap();
        assert_eq!(b, "CGAME_YES:1, CGAME_NO:0");
        assert!(v.modified);
        v.update(
            &cs("", "GAME_VOTE_KICK\u{15}(1)Baker", "1", "1"),
            15,
            false,
            0,
        );
        assert!(v.lines(&loc, 20000, false, &unbound).is_none());
    }

    /// 1.5's `30000` runs from its arrival on the local clock; a gamestate
    /// read neither counts as a change nor owes the sound.
    #[test]
    fn a_15_vote_counts_from_its_arrival() {
        let loc = Localized::default();
        let mut v = Vote::default();
        v.on_gamestate();
        v.update(
            &cs("30000", "GAME_VOTE_MAPRESTART", "1", "0"),
            15,
            true,
            5000,
        );
        assert!(!v.modified);
        let [a, _] = v.lines(&loc, 20_000, false, &unbound).unwrap();
        assert_eq!(a, "CGAME_VOTE(15):GAME_VOTE_MAPRESTART");
        let [a, _] = v.lines(&loc, 90_000, false, &unbound).unwrap();
        assert_eq!(a, "CGAME_VOTE(0):GAME_VOTE_MAPRESTART");
        v.update(
            &cs("30000", "GAME_VOTE_MAPRESTART", "2", "0"),
            15,
            true,
            6000,
        );
        assert!(v.modified);
    }

    /// A prompt names the offender for 20 s; an outcome line stays 10 s.
    #[test]
    fn a_complaint_prompt_names_the_offender() {
        let loc = Localized::default();
        let mut c = Complaint::default();
        let name = |n: i32| (n == 0).then(|| "Able".to_string());
        assert!(c.lines(&loc, 0, &name, &unbound).is_none());
        c.on_command("0", 1000);
        let lines = c.lines(&loc, 20_999, &name, &unbound).unwrap();
        assert_eq!(lines, ["CGAME_COMPLAINTTEAMKILLFILE", "CGAME_PRESSYESNO"]);
        assert!(!c.up(21_000));
        c.on_command("-1", 30_000);
        assert_eq!(
            c.lines(&loc, 39_999, &name, &unbound).unwrap(),
            ["CGAME_COMPLAINTFILED"]
        );
        assert!(!c.up(40_000));
    }
}
