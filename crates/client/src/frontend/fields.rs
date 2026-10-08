//! Edit fields (`ITEM_TYPE_EDITFIELD`) while one has the keyboard, as RTCW's
//! `Item_TextField_HandleKey` drives them: printable characters append up to
//! `maxchars`, Backspace deletes, and Enter, Tab or Esc hand the keys back.
//! Every change goes straight to the item's cvar.

use winit::keyboard::KeyCode;

/// The edit field taking keys: its menu and item, and the text so far.
pub struct Editing {
    pub menu: usize,
    pub item: usize,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EditKey {
    Changed,
    Done,
    Ignored,
}

impl Editing {
    /// One key press with the text it typed; `max_chars` 0 is no cap.
    pub fn key(&mut self, code: KeyCode, typed: Option<&str>, max_chars: usize) -> EditKey {
        match code {
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Tab | KeyCode::Escape => {
                return EditKey::Done;
            }
            KeyCode::Backspace => {
                return match self.text.pop() {
                    Some(_) => EditKey::Changed,
                    None => EditKey::Ignored,
                };
            }
            _ => {}
        }
        let mut changed = false;
        // A `"` would end the quoted `set` the value travels in.
        for c in typed
            .unwrap_or("")
            .chars()
            .filter(|&c| (' '..='~').contains(&c) && c != '"')
        {
            if max_chars != 0 && self.text.chars().count() >= max_chars {
                break;
            }
            self.text.push(c);
            changed = true;
        }
        if changed {
            EditKey::Changed
        } else {
            EditKey::Ignored
        }
    }
}

/// The part of `text` a field shows: the last `max_paint` characters, so the
/// end being typed stays in view; 0 shows everything.
pub fn painted(text: &str, max_paint: usize) -> &str {
    let n = text.chars().count();
    if max_paint == 0 || n <= max_paint {
        return text;
    }
    let skip = text.char_indices().nth(n - max_paint).map_or(0, |(i, _)| i);
    &text[skip..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_respects_maxchars_and_ends_on_enter() {
        let mut e = Editing {
            menu: 0,
            item: 0,
            text: String::new(),
        };
        assert_eq!(e.key(KeyCode::KeyA, Some("ab\"c"), 3), EditKey::Changed);
        assert_eq!(e.text, "abc");
        assert_eq!(e.key(KeyCode::KeyD, Some("d"), 3), EditKey::Ignored);
        assert_eq!(e.key(KeyCode::Backspace, None, 3), EditKey::Changed);
        assert_eq!(e.text, "ab");
        assert_eq!(e.key(KeyCode::Escape, None, 3), EditKey::Done);
        assert_eq!(painted("abcdef", 4), "cdef");
        assert_eq!(painted("abc", 0), "abc");
    }
}
