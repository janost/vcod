//! Localized string tables: `localizedstrings/<language>/*.str`, looked up
//! as `<FILE>_<REFERENCE>` the way a `@` reference in a menu or a
//! localized configstring names them.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::pk3::Pk3Fs;

const LANGUAGE_DIR: &str = "localizedstrings/english/";

#[derive(Default)]
pub struct Localized {
    /// Uppercased `<FILE>_<REFERENCE>` to the English text.
    strings: HashMap<String, String>,
}

impl Localized {
    /// Every `.str` under the English directory; a missing or unreadable file
    /// is skipped, so a stripped install still resolves what it has.
    pub fn load(fs: &Pk3Fs) -> Localized {
        let mut l = Localized::default();
        for path in fs.list_prefix(LANGUAGE_DIR) {
            let Some(stem) = path
                .strip_prefix(LANGUAGE_DIR)
                .and_then(|p| p.strip_suffix(".str"))
            else {
                continue;
            };
            if let Some(bytes) = fs.read(&path) {
                l.parse_into(stem, &String::from_utf8_lossy(&bytes));
            }
        }
        l
    }

    pub fn parse_into(&mut self, file_stem: &str, text: &str) {
        let prefix = file_stem.to_ascii_uppercase();
        let mut reference: Option<String> = None;
        for line in text.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix("REFERENCE") {
                reference = Some(name.trim().to_ascii_uppercase());
            } else if let Some(rest) = line.strip_prefix("LANG_ENGLISH")
                && let (Some(name), Some(value)) = (reference.take(), quoted(rest))
            {
                self.strings.insert(format!("{prefix}_{name}"), value);
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.strings
            .get(&key.to_ascii_uppercase())
            .map(String::as_str)
    }

    /// `@KEY` to its text, or the bare key when the table lacks it; any other
    /// text passes through.
    pub fn translate<'a>(&'a self, text: &'a str) -> Cow<'a, str> {
        match text.strip_prefix('@') {
            Some(key) => Cow::Borrowed(self.get(key).unwrap_or(key)),
            None => Cow::Borrowed(text),
        }
    }

    /// `SEH_LocalizeTextMessage` (CoDMP.exe 0x4aa040), the cgame's trap
    /// 0x39: a server message built by `Scr_ConstructMessageString` or
    /// `G_Say`, as text. `\x14` opens a part that is a localization key,
    /// `\x15` one that is literal, and the message opens on a key. A part
    /// fills the first `%s` still open in what came before it, or is
    /// appended when none is open; only a part's first `%s` opens. A `\x16`
    /// after a separator, or as one, keeps the next part's `%s` literal. A
    /// key the table lacks shows as itself, as retail does with
    /// `loc_warnings` 0 (docs/research/cod11-chat.md).
    pub fn message(&self, text: &str) -> String {
        const ESCAPED: char = '\u{16}';
        let mut out = String::new();
        // `%s` in `out` still waiting for a part.
        let mut open = 0usize;
        let mut translate = true;
        let mut placeholders = true;
        let mut rest = text;
        loop {
            let end = rest
                .find(['\u{14}', '\u{15}', ESCAPED])
                .unwrap_or(rest.len());
            let (part, tail) = rest.split_at(end);
            if !part.is_empty() {
                let text = if translate {
                    self.get(part).unwrap_or(part)
                } else {
                    part
                };
                let mut own = false;
                let mut fixed = String::with_capacity(text.len());
                let mut chars = text.chars().peekable();
                while let Some(c) = chars.next() {
                    if c == '%' && chars.peek() == Some(&'s') {
                        if placeholders && !own {
                            own = true;
                            fixed.push('%');
                        } else {
                            fixed.push(ESCAPED);
                        }
                    } else {
                        fixed.push(c);
                    }
                }
                open += usize::from(own);
                match out.find("%s") {
                    Some(at) if open > usize::from(own) => {
                        out.replace_range(at..at + 2, &fixed);
                        open -= 1;
                    }
                    _ => out.push_str(&fixed),
                }
            }
            let mut chars = tail.chars();
            rest = match chars.next() {
                None => break,
                Some('\u{14}') => {
                    translate = true;
                    chars.as_str()
                }
                Some('\u{15}') => {
                    translate = false;
                    chars.as_str()
                }
                Some(_) => tail,
            };
            placeholders = !rest.starts_with(ESCAPED);
            if !placeholders {
                rest = &rest[ESCAPED.len_utf8()..];
            }
        }
        out.replace(ESCAPED, "%")
    }
}

/// The text between the first and the last `"`, with `\n` and `\"` unescaped.
fn quoted(s: &str) -> Option<String> {
    let start = s.find('"')?;
    let end = s.rfind('"')?;
    (end > start).then(|| s[start + 1..end].replace("\\n", "\n").replace("\\\"", "\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "// comment\r\nVERSION \"1\"\r\nCONFIG \"C:\\x\\y.cfg\"\r\nFILENOTES \"\"\r\n\r\nREFERENCE MAIN_MENU\r\nLANG_ENGLISH \"Main Menu\"\r\n\r\nREFERENCE 1_AMERICAN\r\nLANG_ENGLISH \"1. American\"\r\n";

    #[test]
    fn parses_references_under_the_file_prefix() {
        let mut l = Localized::default();
        l.parse_into("mpmenu", SAMPLE);
        assert_eq!(l.get("MPMENU_1_AMERICAN"), Some("1. American"));
        assert_eq!(l.get("mpmenu_main_menu"), Some("Main Menu"));
        assert_eq!(l.get("MPMENU_NOPE"), None);
    }

    #[test]
    fn translate_resolves_only_an_at_reference() {
        let mut l = Localized::default();
        l.parse_into("mpmenu", SAMPLE);
        assert_eq!(l.translate("@MPMENU_MAIN_MENU"), "Main Menu");
        assert_eq!(l.translate("plain"), "plain");
        // An unknown reference shows its key, as retail does.
        assert_eq!(l.translate("@MPMENU_NOPE"), "MPMENU_NOPE");
    }

    /// The shapes the retail captures in docs/research/cod11-chat.md carry.
    #[test]
    fn a_server_message_localizes_part_by_part() {
        let mut l = Localized::default();
        l.parse_into(
            "game",
            "REFERENCE DEAD\nLANG_ENGLISH \"Dead\"\nREFERENCE ALLIES\nLANG_ENGLISH \"Allies\"\n",
        );
        l.parse_into("mpscript", "REFERENCE WINS\nLANG_ENGLISH \"%s WINS!\"\n");
        l.parse_into("x", "REFERENCE TWO\nLANG_ENGLISH \"%s and %s\"\n");
        // `say`: two literal openers and the line.
        assert_eq!(l.message("\u{15}\u{15}vcod^7: ^7hi"), "vcod^7: ^7hi");
        assert_eq!(
            l.message("\u{15}\u{15}(\u{14}GAME_DEAD\u{15})(\u{14}GAME_ALLIES\u{15})Bob^7: ^5go"),
            "(Dead)(Allies)Bob^7: ^5go"
        );
        // A key and the player that fills it.
        assert_eq!(l.message("MPSCRIPT_WINS\u{15}vcod^7"), "vcod^7 WINS!");
        // `sayAll("plain words")`: a key nothing translates shows as itself.
        assert_eq!(l.message("\u{14}plain words"), "plain words");
        // Only a part's first `%s` opens; the second stays literal.
        assert_eq!(l.message("X_TWO\u{15}a"), "a and %s");
        // A `\x16` keeps a literal part's `%s` out of the count.
        assert_eq!(l.message("\u{15}\u{16}100%s"), "100%s");
    }

    #[test]
    fn stock_tables_carry_the_menu_strings() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let l = Localized::load(&fs);
        assert_eq!(l.get("MPMENU_1_AMERICAN"), Some("1. American"));
        assert!(l.get("MPMENU_1_M1A1_CARBINE").is_some());
    }

    /// The drop reasons `NetClient` hands on, through the stock tables:
    /// `w <key>`, a bare `w`, an OOB `error` message and vcod's own text.
    #[test]
    fn stock_tables_localize_drop_reasons() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let l = Localized::load(&fs);
        let cases = [
            (
                "EXE_SERVERDISCONNECTREASON\u{14}EXE_TIMEDOUT",
                "Server Disconnected - Timed out",
            ),
            (
                "EXE_SERVERDISCONNECTREASON\u{14}kicked by admin",
                "Server Disconnected - kicked by admin",
            ),
            ("EXE_SERVER_DISCONNECTED", "Server Disconnected"),
            ("GAME_INVALIDPASSWORD", "Invalid Password."),
            ("EXE_SERVERISFULL", "Server is full."),
            (
                "EXE_SERVER_IS_DIFFERENT_VER\u{15}1.1",
                "Server is a different version:\n1.1",
            ),
            ("server timed out", "server timed out"),
        ];
        for (wire, text) in cases {
            assert_eq!(l.message(wire), text, "{wire:?}");
        }
    }
}
