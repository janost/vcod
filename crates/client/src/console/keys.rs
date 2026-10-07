//! Retail key names (CoDMP.exe's keynames table, `KEY_*` strings at
//! 0x567f94..0x5682ec) for the keys winit reports. Letters are written
//! uppercase, as retail's `config_mp.cfg` has them.

use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// Every name `bind` accepts.
const NAMES: &[&str] = &[
    "TAB",
    "ENTER",
    "ESCAPE",
    "SPACE",
    "BACKSPACE",
    "UPARROW",
    "DOWNARROW",
    "LEFTARROW",
    "RIGHTARROW",
    "ALT",
    "CTRL",
    "SHIFT",
    "CAPSLOCK",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "INS",
    "DEL",
    "PGDN",
    "PGUP",
    "HOME",
    "END",
    "MOUSE1",
    "MOUSE2",
    "MOUSE3",
    "MOUSE4",
    "MOUSE5",
    "MWHEELUP",
    "MWHEELDOWN",
    "KP_HOME",
    "KP_UPARROW",
    "KP_PGUP",
    "KP_LEFTARROW",
    "KP_5",
    "KP_RIGHTARROW",
    "KP_END",
    "KP_DOWNARROW",
    "KP_PGDN",
    "KP_ENTER",
    "KP_INS",
    "KP_DEL",
    "KP_SLASH",
    "KP_MINUS",
    "KP_PLUS",
    "KP_NUMLOCK",
    "KP_STAR",
    "KP_EQUALS",
    "PAUSE",
    "SEMICOLON",
];

/// The printable keys that name themselves.
const CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789`-=[]\\',./";

/// `name` as the bind table keys it, or `None` for a name retail does not
/// have. `~` is the shifted backquote, one physical key here.
pub fn canonical(name: &str) -> Option<String> {
    let up = name.to_ascii_uppercase();
    if up == "~" {
        return Some("`".into());
    }
    if NAMES.contains(&up.as_str()) {
        return Some(up);
    }
    let mut chars = up.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if CHARS.contains(c) => Some(up),
        _ => None,
    }
}

pub fn key_name(code: KeyCode) -> Option<&'static str> {
    use KeyCode::*;
    Some(match code {
        KeyA => "A",
        KeyB => "B",
        KeyC => "C",
        KeyD => "D",
        KeyE => "E",
        KeyF => "F",
        KeyG => "G",
        KeyH => "H",
        KeyI => "I",
        KeyJ => "J",
        KeyK => "K",
        KeyL => "L",
        KeyM => "M",
        KeyN => "N",
        KeyO => "O",
        KeyP => "P",
        KeyQ => "Q",
        KeyR => "R",
        KeyS => "S",
        KeyT => "T",
        KeyU => "U",
        KeyV => "V",
        KeyW => "W",
        KeyX => "X",
        KeyY => "Y",
        KeyZ => "Z",
        Digit0 => "0",
        Digit1 => "1",
        Digit2 => "2",
        Digit3 => "3",
        Digit4 => "4",
        Digit5 => "5",
        Digit6 => "6",
        Digit7 => "7",
        Digit8 => "8",
        Digit9 => "9",
        Backquote => "`",
        Minus => "-",
        Equal => "=",
        BracketLeft => "[",
        BracketRight => "]",
        Backslash => "\\",
        Semicolon => "SEMICOLON",
        Quote => "'",
        Comma => ",",
        Period => ".",
        Slash => "/",
        Tab => "TAB",
        Enter => "ENTER",
        Escape => "ESCAPE",
        Space => "SPACE",
        Backspace => "BACKSPACE",
        ArrowUp => "UPARROW",
        ArrowDown => "DOWNARROW",
        ArrowLeft => "LEFTARROW",
        ArrowRight => "RIGHTARROW",
        AltLeft | AltRight => "ALT",
        ControlLeft | ControlRight => "CTRL",
        ShiftLeft | ShiftRight => "SHIFT",
        CapsLock => "CAPSLOCK",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        Insert => "INS",
        Delete => "DEL",
        PageDown => "PGDN",
        PageUp => "PGUP",
        Home => "HOME",
        End => "END",
        Pause => "PAUSE",
        Numpad7 => "KP_HOME",
        Numpad8 => "KP_UPARROW",
        Numpad9 => "KP_PGUP",
        Numpad4 => "KP_LEFTARROW",
        Numpad5 => "KP_5",
        Numpad6 => "KP_RIGHTARROW",
        Numpad1 => "KP_END",
        Numpad2 => "KP_DOWNARROW",
        Numpad3 => "KP_PGDN",
        NumpadEnter => "KP_ENTER",
        Numpad0 => "KP_INS",
        NumpadDecimal => "KP_DEL",
        NumpadDivide => "KP_SLASH",
        NumpadSubtract => "KP_MINUS",
        NumpadAdd => "KP_PLUS",
        NumLock => "KP_NUMLOCK",
        NumpadMultiply => "KP_STAR",
        NumpadEqual => "KP_EQUALS",
        _ => return None,
    })
}

pub fn mouse_name(button: MouseButton) -> Option<&'static str> {
    Some(match button {
        MouseButton::Left => "MOUSE1",
        MouseButton::Right => "MOUSE2",
        MouseButton::Middle => "MOUSE3",
        MouseButton::Back => "MOUSE4",
        MouseButton::Forward => "MOUSE5",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reported_key_is_bindable_under_its_own_name() {
        use KeyCode::*;
        for code in [
            KeyA,
            KeyZ,
            Digit0,
            Backquote,
            Semicolon,
            Slash,
            Tab,
            ControlRight,
            F12,
            Numpad5,
            NumpadEnter,
            NumpadEqual,
            Pause,
        ] {
            let name = key_name(code).unwrap();
            assert_eq!(canonical(name).as_deref(), Some(name), "{code:?}");
        }
        for b in [MouseButton::Left, MouseButton::Forward] {
            let name = mouse_name(b).unwrap();
            assert_eq!(canonical(name).as_deref(), Some(name));
        }
    }

    #[test]
    fn names_fold_case_and_tilde_is_backquote() {
        assert_eq!(canonical("mwheelup").as_deref(), Some("MWHEELUP"));
        assert_eq!(canonical("w").as_deref(), Some("W"));
        assert_eq!(canonical("~").as_deref(), Some("`"));
        assert_eq!(canonical("KEY_W"), None);
        assert_eq!(canonical("ww"), None);
    }
}
