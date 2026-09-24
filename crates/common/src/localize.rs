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
            } else if let Some(rest) = line.strip_prefix("LANG_ENGLISH") {
                if let (Some(name), Some(value)) = (reference.take(), quoted(rest)) {
                    self.strings.insert(format!("{prefix}_{name}"), value);
                }
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

    #[test]
    fn stock_tables_carry_the_menu_strings() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let l = Localized::load(&fs);
        assert_eq!(l.get("MPMENU_1_AMERICAN"), Some("1. American"));
        assert!(l.get("MPMENU_1_M1A1_CARBINE").is_some());
    }
}
