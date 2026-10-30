//! CoD2's `hudelem_s` as the cgame reads it: float positions, the packed
//! `alignOrg` / `alignScreen` words and the new `z` and `foreground`
//! (docs/research/cod2-protocol.md 5.1, docs/research/cod2-fonts-menus.md 3).

use crate::net::HudElem2;
use crate::net::fields::HUDELEM_FIELDS;

/// `hudelem_s`, read off its 32 netfield words.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudElem {
    /// `he_type_t`: 0 free, 1 text, 2 value, 6 material, 7..12 timers
    /// (`CG_GetHudElemInfo`).
    pub kind: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub font_scale: f32,
    /// 0 `default`, 1 `bigfixed`, 2 `smallfixed`.
    pub font: i32,
    /// `alignx << 2 | aligny`: 0 left/top, 1 centre, 2 right/bottom.
    pub align_org: i32,
    /// `horzalign << 3 | vertalign`, `HORIZONTAL_ALIGN_*` / `VERTICAL_ALIGN_*`.
    pub align_screen: i32,
    pub color: [u8; 4],
    pub from_color: [u8; 4],
    pub fade_start_time: i32,
    pub fade_time: i32,
    pub label: i32,
    pub width: i32,
    pub height: i32,
    pub material: i32,
    pub from_width: i32,
    pub from_height: i32,
    pub scale_start_time: i32,
    pub scale_time: i32,
    pub from_x: f32,
    pub from_y: f32,
    pub from_align_org: i32,
    pub from_align_screen: i32,
    pub move_start_time: i32,
    pub move_time: i32,
    pub time: i32,
    pub duration: i32,
    pub value: f32,
    pub text: i32,
    pub sort: f32,
    pub foreground: bool,
}

impl HudElem {
    /// The element a snapshot's 32 words hold.
    pub fn from_wire(w: &HudElem2) -> HudElem {
        let word = |name: &str| {
            let i = HUDELEM_FIELDS.iter().position(|f| f.name == name);
            w[i.unwrap_or_else(|| panic!("no hudelem field {name}"))]
        };
        let int = |name: &str| word(name) as i32;
        let float = |name: &str| f32::from_bits(word(name));
        let rgba = |name: &str| word(name).to_le_bytes();
        HudElem {
            kind: int("type"),
            x: float("x"),
            y: float("y"),
            z: float("z"),
            font_scale: float("fontScale"),
            font: int("font"),
            align_org: int("alignOrg"),
            align_screen: int("alignScreen"),
            color: rgba("color.rgba"),
            from_color: rgba("fromColor.rgba"),
            fade_start_time: int("fadeStartTime"),
            fade_time: int("fadeTime"),
            label: int("label"),
            width: int("width"),
            height: int("height"),
            material: int("materialIndex"),
            from_width: int("fromWidth"),
            from_height: int("fromHeight"),
            scale_start_time: int("scaleStartTime"),
            scale_time: int("scaleTime"),
            from_x: float("fromX"),
            from_y: float("fromY"),
            from_align_org: int("fromAlignOrg"),
            from_align_screen: int("fromAlignScreen"),
            move_start_time: int("moveStartTime"),
            move_time: int("moveTime"),
            time: int("time"),
            duration: int("duration"),
            value: float("value"),
            text: int("text"),
            sort: float("sort"),
            foreground: word("foreground") != 0,
        }
    }

    pub fn align_x(&self) -> i32 {
        self.align_org >> 2 & 3
    }

    pub fn align_y(&self) -> i32 {
        self.align_org & 3
    }

    pub fn horz_align(&self) -> i32 {
        self.align_screen >> 3 & 7
    }

    pub fn vert_align(&self) -> i32 {
        self.align_screen & 7
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_fields_by_their_table_slot() {
        let mut w: HudElem2 = [0; 32];
        let set = |w: &mut HudElem2, name: &str, v: u32| {
            w[HUDELEM_FIELDS.iter().position(|f| f.name == name).unwrap()] = v;
        };
        set(&mut w, "x", 12.5f32.to_bits());
        set(&mut w, "y", (-3.0f32).to_bits());
        set(&mut w, "z", 1.0f32.to_bits());
        set(&mut w, "alignOrg", 1 << 2 | 2);
        set(&mut w, "alignScreen", 3 << 3 | 4);
        set(&mut w, "color.rgba", u32::from_le_bytes([255, 128, 0, 200]));
        set(&mut w, "foreground", 1);
        set(&mut w, "type", 6);
        let e = HudElem::from_wire(&w);
        assert_eq!((e.x, e.y, e.z), (12.5, -3.0, 1.0));
        assert_eq!((e.align_x(), e.align_y()), (1, 2));
        assert_eq!((e.horz_align(), e.vert_align()), (3, 4));
        assert_eq!(e.color, [255, 128, 0, 200]);
        assert!(e.foreground);
        assert_eq!(e.kind, 6);
    }
}
