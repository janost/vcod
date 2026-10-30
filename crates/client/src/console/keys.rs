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

/// Retail's key number for a name (CoDMP.exe's keynames table at
/// 0x57b8d0, name and number pairs): the ASCII keys by their code, letters
/// lowercase, the rest from 128 up with the mouse at 200..206. A name the
/// table lacks sorts last.
pub fn number(name: &str) -> u32 {
    match name {
        "TAB" => 9,
        "ENTER" => 13,
        "ESCAPE" => 27,
        "SPACE" => 32,
        "BACKSPACE" => 127,
        "UPARROW" => 154,
        "DOWNARROW" => 155,
        "LEFTARROW" => 156,
        "RIGHTARROW" => 157,
        "ALT" => 158,
        "CTRL" => 159,
        "SHIFT" => 160,
        "CAPSLOCK" => 151,
        "F1" => 167,
        "F2" => 168,
        "F3" => 169,
        "F4" => 170,
        "F5" => 171,
        "F6" => 172,
        "F7" => 173,
        "F8" => 174,
        "F9" => 175,
        "F10" => 176,
        "F11" => 177,
        "F12" => 178,
        "INS" => 161,
        "DEL" => 162,
        "PGDN" => 163,
        "PGUP" => 164,
        "HOME" => 165,
        "END" => 166,
        "MOUSE1" => 200,
        "MOUSE2" => 201,
        "MOUSE3" => 202,
        "MOUSE4" => 203,
        "MOUSE5" => 204,
        "MWHEELDOWN" => 205,
        "MWHEELUP" => 206,
        "KP_HOME" => 182,
        "KP_UPARROW" => 183,
        "KP_PGUP" => 184,
        "KP_LEFTARROW" => 185,
        "KP_5" => 186,
        "KP_RIGHTARROW" => 187,
        "KP_END" => 188,
        "KP_DOWNARROW" => 189,
        "KP_PGDN" => 190,
        "KP_ENTER" => 191,
        "KP_INS" => 192,
        "KP_DEL" => 193,
        "KP_SLASH" => 194,
        "KP_MINUS" => 195,
        "KP_PLUS" => 196,
        "KP_NUMLOCK" => 197,
        "KP_STAR" => 198,
        "KP_EQUALS" => 199,
        "PAUSE" => 153,
        "SEMICOLON" => 59,
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.to_ascii_lowercase() as u32,
                _ => u32::MAX,
            }
        }
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

/// Whole wheel notches out of winit's scroll events. A wheel's notch is a
/// `LineDelta` of 1 (X11, or Wayland's `axis_discrete`); a high-resolution
/// wheel on Wayland adds a `PixelDelta` for every part of a notch before
/// the discrete one, and a touchpad sends only those. Retail turns one
/// notch into one `MWHEELUP`/`MWHEELDOWN`, so counting every event fired
/// `weapnext` several times a notch, often an even number of times.
#[derive(Default)]
pub struct WheelNotches {
    lines: f32,
    pixels: f32,
    /// A discrete notch came: this device's pixel parts are not notches.
    discrete: bool,
}

/// A touchpad's pixels to a notch, as the menus' wheel already counts.
const PIXELS_PER_NOTCH: f32 = 60.0;

impl WheelNotches {
    /// The whole notches one event completes, a `LineDelta`'s or a
    /// `PixelDelta`'s y (winit's sign, up positive); up positive.
    pub fn step(&mut self, line: Option<f32>, pixels: Option<f32>) -> i32 {
        let acc = if let Some(y) = line {
            self.discrete = true;
            self.pixels = 0.0;
            self.lines += y;
            &mut self.lines
        } else if let Some(y) = pixels {
            if self.discrete {
                return 0;
            }
            self.pixels += y / PIXELS_PER_NOTCH;
            &mut self.pixels
        } else {
            return 0;
        };
        let whole = acc.trunc();
        *acc -= whole;
        whole as i32
    }
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

    #[test]
    fn a_notch_is_one_press_whatever_the_wheel_sends() {
        let mut w = WheelNotches::default();
        // X11 or a plain wheel on Wayland: one line a notch.
        assert_eq!(w.step(Some(-1.0), None), -1);
        assert_eq!(w.step(Some(1.0), None), 1);
        // A high-resolution wheel on Wayland: seven parts, then the notch.
        let mut w = WheelNotches::default();
        w.step(Some(-1.0), None);
        let parts: i32 = (0..7).map(|_| w.step(None, Some(-1.875))).sum();
        assert_eq!(parts + w.step(Some(-1.0), None), -1);
        // A touchpad: pixels only, a notch every 60.
        let mut w = WheelNotches::default();
        let n: i32 = (0..8).map(|_| w.step(None, Some(15.0))).sum();
        assert_eq!(n, 2);
    }
}
