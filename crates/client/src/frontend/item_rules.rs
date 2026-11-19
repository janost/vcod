//! Item rules the two menu runtimes share: CoD1 and CoD2's [`super::Ui`]
//! and CoD4's `play::cod4_menus::ScriptMenus` port the same
//! `Item_*_HandleKey` and `Item_Slider_Paint` bodies onto different item
//! structs. Plain values in and out; each runtime reads its item and
//! writes its cvar. Where the games' handlers differ, each keeps its own.

/// `Item_Slider_Paint`'s bar, 96 x 16, and its thumb, 10 x 20, whose centre
/// travels 84 units from 6 in (ui_mp_x86.dll 0x400150a0;
/// cod4-hud.md 12.6).
pub const SLIDER_W: f32 = 96.0;
pub const SLIDER_H: f32 = 16.0;
pub const THUMB_W: f32 = 10.0;
pub const THUMB_H: f32 = 20.0;
pub const THUMB_TRAVEL: f32 = 84.0;
pub const THUMB_INSET: f32 = 6.0;

/// The thumb's centre on a bar at `x` for a value `frac` of the range.
pub fn thumb_x(x: f32, frac: f32) -> f32 {
    x + THUMB_INSET + frac * THUMB_TRAVEL
}

/// The value `frac` of the way along a slider's range.
pub fn slider_value(frac: f32, min: f32, max: f32) -> f32 {
    frac * (max - min) + min
}

/// `Item_DvarEnum_EnumIndex`: the entry an enum dvar's value names, read
/// through the game's `atoi` as an index in range, else as an entry's name
/// in any case, else 0.
pub fn enum_index(entries: &[String], value: &str, atoi: fn(&str) -> i32) -> usize {
    match usize::try_from(atoi(value)) {
        Ok(i) if i < entries.len() => i,
        _ => entries
            .iter()
            .position(|e| e.eq_ignore_ascii_case(value))
            .unwrap_or(0),
    }
}

/// The entry after `at` of `n` (the one before with `back`), wrapping, as
/// the multi and enum handlers step; `None` with no entries.
pub fn step(at: usize, n: usize, back: bool) -> Option<usize> {
    match n {
        0 => None,
        _ if back => Some((at + n - 1) % n),
        _ => Some((at + 1) % n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_enum_value_names_its_entry_by_index_then_by_name() {
        let e: Vec<String> = ["640x480", "800x600"].map(String::from).to_vec();
        let atoi = vcod_common::text::atoi_msvc;
        assert_eq!(enum_index(&e, "1", atoi), 1);
        assert_eq!(enum_index(&e, "800X600", atoi), 1);
        assert_eq!(enum_index(&e, "2", atoi), 0);
        assert_eq!(enum_index(&e, "-1", atoi), 0);
        assert_eq!(enum_index(&[], "0", atoi), 0);
    }

    #[test]
    fn a_step_wraps_both_ways_and_none_has_no_entries() {
        assert_eq!(step(2, 3, false), Some(0));
        assert_eq!(step(0, 3, true), Some(2));
        assert_eq!(step(0, 1, false), Some(0));
        assert_eq!(step(0, 0, true), None);
    }
}
