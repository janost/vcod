//! CoD2's hudelems on the real screen: an element's `alignOrg` shifts it by
//! its own size on the 640x480 grid, then its `alignScreen` places it
//! against the screen (`CalcScreenPlacement`). CoD2 only; CoD1's hudelems
//! are [`super::hudelem`]. docs/research/cod2-fonts-menus.md, section 3.

use super::HudQuad;
use super::font::{self, Slot, UiFonts};
use vcod_cod2::hudelem::HudElem;

/// `ScreenPlacement` as `SetScreenScaling` (Mac 0x18eb38) fills it for the
/// whole window as the safe area and square pixels: the 4:3 column of the
/// window's height, centred, is the grid's `SUBLEFT` frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenPlacement {
    pub virtual_to_real: [f32; 2],
    virtual_to_full: [f32; 2],
    real_to_virtual: [f32; 2],
    sub_screen_left: f32,
    pub real_size: [f32; 2],
    /// [`crate::camera::Cod4Screen::strip_scale`]: 1 but for CoD4 under
    /// `vc_windowAspect 1`.
    pub strip_scale: f32,
}

/// CoD4's display pixel aspect (0xcc9d100), its strip scale and `wideScreen`
/// ([`crate::camera::Cod4Screen`]) as of the frame: `cls.vidConfig`, a
/// global in 1.7 too. Square pixels, no scale and 4:3 until a CoD4 frame
/// sets them.
#[derive(Clone, Copy)]
struct Cod4Globals {
    pixel_aspect: f32,
    strip_scale: f32,
    wide_screen: bool,
}

const COD4_DEFAULT: Cod4Globals = Cod4Globals {
    pixel_aspect: 1.0,
    strip_scale: 1.0,
    wide_screen: false,
};

#[cfg(not(test))]
static COD4_SCREEN: std::sync::Mutex<Cod4Globals> = std::sync::Mutex::new(COD4_DEFAULT);

// Per thread under test, so a test that sets a screen leaves the others'.
#[cfg(test)]
thread_local! {
    static COD4_SCREEN: std::cell::Cell<Cod4Globals> = const { std::cell::Cell::new(COD4_DEFAULT) };
}

fn cod4_globals() -> Cod4Globals {
    #[cfg(not(test))]
    return *COD4_SCREEN.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(test)]
    COD4_SCREEN.get()
}

/// The CoD4 frame's screen for [`ScreenPlacement::cod4`].
pub fn set_cod4_screen(s: &crate::camera::Cod4Screen) {
    let g = Cod4Globals {
        pixel_aspect: s.pixel_aspect,
        strip_scale: s.strip_scale,
        wide_screen: s.wide_screen,
    };
    #[cfg(not(test))]
    {
        *COD4_SCREEN.lock().unwrap_or_else(|e| e.into_inner()) = g;
    }
    #[cfg(test)]
    COD4_SCREEN.set(g);
}

/// The `wideScreen` dvar of the frame's CoD4 screen.
pub fn cod4_wide_screen() -> bool {
    cod4_globals().wide_screen
}

impl ScreenPlacement {
    pub fn new(w: f32, h: f32) -> ScreenPlacement {
        ScreenPlacement::scr_place(w, h, 1.0)
    }

    /// CoD4's 2D layer in a `w` x `h` window at the frame's screen
    /// ([`set_cod4_screen`]).
    pub fn cod4(w: f32, h: f32) -> ScreenPlacement {
        let g = cod4_globals();
        ScreenPlacement {
            strip_scale: g.strip_scale,
            ..ScreenPlacement::scr_place(w, h, g.pixel_aspect)
        }
    }

    /// `ScrPlace_SetupFloatViewport` (iw3mp.exe 0x477420): the 4:3 column
    /// is `h * 4/3 / pixel_aspect` wide, no wider than the window, and
    /// centred; `LEFT` and `RIGHT` place against the window's edges.
    pub fn scr_place(w: f32, h: f32, pixel_aspect: f32) -> ScreenPlacement {
        let column = (h * 4.0 / 3.0 / pixel_aspect).min(w);
        ScreenPlacement {
            virtual_to_real: [column / 640.0, h / 480.0],
            virtual_to_full: [w / 640.0, h / 480.0],
            real_to_virtual: [640.0 / column, 480.0 / h],
            sub_screen_left: 0.5 * (w - column),
            real_size: [w, h],
            strip_scale: 1.0,
        }
    }

    /// The grid's scale on `axis` stretched over the whole window
    /// (`scrPlaceFull`).
    pub fn virtual_to_full(&self, axis: usize) -> f32 {
        self.virtual_to_full[axis]
    }

    /// One axis of `CalcScreenPlacement` (Mac 0x18ed96): a grid position and
    /// length to real pixels by `HORIZONTAL_ALIGN_*` (`axis` 0) or
    /// `VERTICAL_ALIGN_*` (1). The safe area is the window, so `LEFT` and
    /// `TOP` add 0, `RIGHT` and `BOTTOM` the window's size.
    pub fn axis(&self, axis: usize, at: f32, len: f32, align: i32) -> (f32, f32) {
        let s = self.virtual_to_real[axis];
        let size = self.real_size[axis];
        // SUBLEFT shifts into the 4:3 column; SUBTOP has no shift.
        let sub = if axis == 0 { self.sub_screen_left } else { 0.0 };
        match align {
            1 => (at * s, len * s),
            2 | 7 => (at * s + 0.5 * size, len * s),
            3 => (at * s + size, len * s),
            4 => (
                at * self.virtual_to_full[axis],
                len * self.virtual_to_full[axis],
            ),
            5 => (at, len),
            6 => (
                at * self.real_to_virtual[axis],
                len * self.real_to_virtual[axis],
            ),
            _ => (at * s + sub, len * s),
        }
    }
}

/// `CG_AlignHudElemX` / `Y` (Mac 0x1e0178, 0x1e01c2): 1 centres on `at`,
/// 2 ends there, anything else starts there.
fn align_org(align: i32, at: f32, len: f32) -> f32 {
    match align {
        1 => at - 0.5 * len,
        2 => at - len,
        _ => at,
    }
}

/// The real-pixel top-left of an element `w` by `h` grid units placed by
/// `org` and `screen` at grid `(x, y)`.
fn placed(
    sp: &ScreenPlacement,
    (x, y): (f32, f32),
    (w, h): (f32, f32),
    org: i32,
    screen: i32,
) -> [f32; 2] {
    let px = sp
        .axis(0, align_org(org >> 2 & 3, x, w), w, screen >> 3 & 7)
        .0;
    let py = sp.axis(1, align_org(org & 3, y, h), h, screen & 7).0;
    [px, py]
}

/// Where `e` is drawn at `now`: mid-move it lerps from its `from*` placement
/// to its own, both placed whole first (`CG_GetHudElemInfo`, Mac 0x1e047c).
pub fn position(sp: &ScreenPlacement, e: &HudElem, size: (f32, f32), now: i32) -> [f32; 2] {
    let to = placed(sp, (e.x, e.y), size, e.align_org, e.align_screen);
    if e.move_time <= 0 {
        return to;
    }
    let t = now - e.move_start_time;
    let frac = if t < 1 {
        0.0
    } else if t < e.move_time {
        t as f32 / e.move_time as f32
    } else {
        return to;
    };
    let from = placed(
        sp,
        (e.from_x, e.from_y),
        size,
        e.from_align_org,
        e.from_align_screen,
    );
    [
        from[0] + (to[0] - from[0]) * frac,
        from[1] + (to[1] - from[1]) * frac,
    ]
}

/// `e`'s colour at `now`, fading from `fromColor` over `fadeTime`.
fn colour(e: &HudElem, now: i32) -> [f32; 4] {
    let to = e.color.map(|c| f32::from(c) / 255.0);
    let t = now - e.fade_start_time;
    if e.fade_time <= 0 || t >= e.fade_time {
        return to;
    }
    let frac = (t.max(0) as f32 / e.fade_time as f32).clamp(0.0, 1.0);
    let from = e.from_color.map(|c| f32::from(c) / 255.0);
    std::array::from_fn(|i| from[i] + (to[i] - from[i]) * frac)
}

/// The UI font and text scale an element's `font` picks: `default` at
/// `0.25 * fontScale`, `bigfixed` at `0.5 *` and `smallfixed` at a third.
fn text_font<'a>(fonts: &'a UiFonts, e: &HudElem, screen_h: f32) -> (&'a font::Font, f32) {
    let (slot, scale) = font_slot(e);
    (fonts.pick(slot, scale, screen_h), scale)
}

/// The font handle and text scale `e` draws with.
fn font_slot(e: &HudElem) -> (Slot, f32) {
    match e.font {
        1 => (Slot::BigFixed, 0.5 * e.font_scale),
        2 => (Slot::SmallFixed, e.font_scale / 3.0),
        _ => (Slot::Default, 0.25 * e.font_scale),
    }
}

/// A clock's needle without a `duration` turns once a minute (Mac
/// 0x326644).
const CLOCK_DEG_PER_MS: f32 = 0.006;

/// A material or clock element's grid size at `now` (`CG_Draw2dHudElems`,
/// Mac 0x1e11b8): `width` and `height`, scaled from `fromWidth` and
/// `fromHeight` over `scaleTime` past `scaleStartTime`; a size of 0 reads
/// as the element's font height (`CG_GetHudElemInfo`'s `UI_TextHeight`),
/// so a bar `scaleOverTime`d from `setShader(m, 0, h)` grows from there.
fn scaled_size(e: &HudElem, now: i32) -> (f32, f32) {
    let scale = font_slot(e).1;
    let text_h = (48.0 * scale) as i32 as f32;
    let or = |v: i32| if v != 0 { v as f32 } else { text_h };
    let (mut w, mut h) = (or(e.width), or(e.height));
    let dt = now - e.scale_start_time;
    if e.scale_time > 0 && dt < e.scale_time {
        let t = (dt as f32 * (1.0 / e.scale_time as f32)).clamp(0.0, 1.0);
        w = (w - or(e.from_width)) * t + or(e.from_width);
        h = (h - or(e.from_height)) * t + or(e.from_height);
    }
    (w, h)
}

/// Draws the `foreground` pass of `elems` (archived first, then current)
/// sorted by `sort`: text kinds through `text_of(e)` (the label and its
/// text, already formatted) and materials through `material_of(index)`.
pub fn build(
    elems: &[HudElem],
    foreground: bool,
    fonts: &UiFonts,
    sp: &ScreenPlacement,
    now: i32,
    text_of: &dyn Fn(&HudElem) -> String,
    material_of: &dyn Fn(i32) -> Option<String>,
) -> Vec<HudQuad> {
    let mut order: Vec<&HudElem> = elems
        .iter()
        .filter(|e| e.kind != 0 && e.kind != 13 && e.foreground == foreground)
        .collect();
    order.sort_by(|a, b| a.sort.total_cmp(&b.sort));
    let mut out = Vec::new();
    for e in order {
        let rgba = colour(e, now);
        if rgba[3] == 0.0 {
            continue;
        }
        match e.kind {
            // Text, value, player and map names, game type and timers (0x7be).
            1..=5 | 7..=10 => {
                let s = text_of(e);
                if s.is_empty() {
                    continue;
                }
                let (f, scale) = text_font(fonts, e, sp.real_size[1]);
                let px = scale / f.unit_scale();
                let w = font::measure(f, &s, px);
                // `UI_TextHeight`: the font's em at `scale`, whole units.
                let h = (48.0 * scale) as i32 as f32;
                let [x, y] = position(sp, e, (w, h), now);
                let k = sp.virtual_to_real[1];
                font::layout(f, &s, x, y, px * k, rgba, &mut out);
            }
            6 | 11 | 12 => {
                let Some(name) = material_of(e.material) else {
                    continue;
                };
                let (w, h) = scaled_size(e, now);
                let [x, y] = position(sp, e, (w, h), now);
                let rw = sp.axis(0, 0.0, w, e.align_screen >> 3 & 7).1;
                let rh = sp.axis(1, 0.0, h, e.align_screen & 7).1;
                let quad = |angle: f32, texture: String| {
                    let (s, c) = angle.to_radians().sin_cos();
                    let (hw, hh) = (0.5 * rw, 0.5 * rh);
                    let (cx, cy) = (x + hw, y + hh);
                    let corner = |sx: f32, sy: f32| {
                        [
                            cx + c * hw * sx - s * hw * sy,
                            cy + s * hh * sx + c * hh * sy,
                        ]
                    };
                    HudQuad {
                        verts: [
                            corner(-1.0, -1.0),
                            corner(1.0, -1.0),
                            corner(1.0, 1.0),
                            corner(-1.0, 1.0),
                        ],
                        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                        rgba,
                        texture,
                    }
                };
                if e.kind == 6 {
                    out.push(quad(0.0, name));
                    continue;
                }
                // A clock: its face, then `<face>needle` turned about the
                // centre, once a minute or once a `duration`.
                let ms = match e.kind {
                    11 => e.time - now,
                    _ => now - e.time,
                }
                .max(0) as f32;
                let angle = if e.duration == 0 {
                    ms * CLOCK_DEG_PER_MS
                } else {
                    ms * 360.0 / e.duration as f32
                }
                .rem_euclid(360.0);
                let needle = format!("{name}needle");
                out.push(quad(0.0, name));
                out.push(quad(angle, needle));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1920x1080 window: the 4:3 column is 1440 wide, 240 in from the left.
    fn wide() -> ScreenPlacement {
        ScreenPlacement::new(1920.0, 1080.0)
    }

    #[test]
    fn a_bar_scaled_from_no_width_grows_from_the_font_height() {
        // sd's plant bar: `setShader("white", 0, 8)`, then
        // `scaleOverTime(5, 192, 8)` at 1000.
        let e = HudElem {
            width: 192,
            height: 8,
            from_width: 0,
            from_height: 8,
            scale_start_time: 1000,
            scale_time: 5000,
            font_scale: 1.0,
            ..HudElem::default()
        };
        assert_eq!(scaled_size(&e, 1000), (12.0, 8.0));
        assert_eq!(scaled_size(&e, 3500), (102.0, 8.0));
        assert_eq!(scaled_size(&e, 6000), (192.0, 8.0));
    }

    #[test]
    fn each_screen_alignment_places_its_own_frame() {
        let sp = wide();
        // SUBLEFT: the 4:3 column; LEFT: the window's edge.
        assert_eq!(sp.axis(0, 0.0, 640.0, 0), (240.0, 1440.0));
        assert_eq!(sp.axis(0, 10.0, 0.0, 1).0, 22.5);
        assert_eq!(sp.axis(0, 0.0, 0.0, 2).0, 960.0);
        assert_eq!(sp.axis(0, -10.0, 0.0, 3).0, 1920.0 - 22.5);
        // FULLSCREEN stretches the grid over the window, NOSCALE is pixels.
        assert_eq!(sp.axis(0, 640.0, 640.0, 4), (1920.0, 1920.0));
        assert_eq!(sp.axis(0, 7.0, 3.0, 5), (7.0, 3.0));
        assert_eq!(sp.axis(1, 480.0, 0.0, 0).0, 1080.0);
        assert_eq!(sp.axis(1, -20.0, 0.0, 3).0, 1080.0 - 45.0);
        // A 4:3 window: SUBLEFT and LEFT agree.
        let sq = ScreenPlacement::new(640.0, 480.0);
        assert_eq!(sq.axis(0, 5.0, 1.0, 0), sq.axis(0, 5.0, 1.0, 1));
    }

    /// CoD4 at 3840x1600: 1.7 fullscreen at `auto` (16:9) stretches the
    /// 4:3 column to 2880 (pixel aspect 16/9 * 1600/3840), windowed keeps
    /// it square at 2133, as does `vc_windowAspect 1`; `LEFT`, `CENTER`
    /// and `RIGHT` place against the window's edges and middle in each.
    #[test]
    fn cod4_placement_follows_the_display_pixel_aspect() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.05;
        let full = ScreenPlacement::scr_place(3840.0, 1600.0, 16.0 / 9.0 * 1600.0 / 3840.0);
        let (x, w) = full.axis(0, 0.0, 640.0, 0);
        assert!(close(x, 480.0) && close(w, 2880.0), "{x} {w}");
        assert_eq!(full.axis(0, 0.0, 0.0, 1).0, 0.0);
        assert_eq!(full.axis(0, 0.0, 0.0, 3).0, 3840.0);
        let square = ScreenPlacement::scr_place(3840.0, 1600.0, 1.0);
        assert_eq!(square, ScreenPlacement::new(3840.0, 1600.0));
        let (x, w) = square.axis(0, 0.0, 640.0, 0);
        assert!(close(x, 853.33) && close(w, 2133.33), "{x} {w}");
        // the compass's -8 and the weapon info's right edge
        assert!(close(square.axis(0, -8.0, 0.0, 1).0, -26.67));
        assert_eq!(square.axis(0, 0.0, 0.0, 3).0, 3840.0);
        // a piece centred on the grid's middle stays on the window's
        let (x, w) = square.axis(0, -250.0, 500.0, 2);
        assert!(close(x + 0.5 * w, 1920.0), "{x} {w}");
        // FULLSCREEN spans the window
        assert_eq!(square.axis(0, 0.0, 640.0, 4), (0.0, 3840.0));
        // 1.7 fullscreen `standard` on a 16:9 screen: the column is the screen
        let std = ScreenPlacement::scr_place(1920.0, 1080.0, 4.0 / 3.0 * 1080.0 / 1920.0);
        assert!(close(std.axis(0, 0.0, 640.0, 0).1, 1920.0));
    }

    #[test]
    fn org_alignment_shifts_by_the_elements_own_size() {
        let sp = ScreenPlacement::new(640.0, 480.0);
        let e = |org, screen| HudElem {
            x: 100.0,
            y: 50.0,
            align_org: org,
            align_screen: screen,
            ..HudElem::default()
        };
        // Centre and right of x, bottom of y.
        assert_eq!(
            position(&sp, &e(1 << 2 | 2, 0), (40.0, 10.0), 0),
            [80.0, 40.0]
        );
        assert_eq!(position(&sp, &e(2 << 2, 0), (40.0, 10.0), 0), [60.0, 50.0]);
        // RIGHT / BOTTOM screen alignment with negative offsets.
        let br = HudElem {
            x: -10.0,
            y: -10.0,
            ..e(2 << 2 | 2, 3 << 3 | 3)
        };
        assert_eq!(position(&sp, &br, (40.0, 10.0), 0), [590.0, 460.0]);
    }

    #[test]
    fn a_move_lerps_between_both_placements() {
        let sp = ScreenPlacement::new(640.0, 480.0);
        let e = HudElem {
            x: 100.0,
            from_x: 0.0,
            move_start_time: 1000,
            move_time: 200,
            ..HudElem::default()
        };
        assert_eq!(position(&sp, &e, (0.0, 0.0), 1000)[0], 0.0);
        assert_eq!(position(&sp, &e, (0.0, 0.0), 1100)[0], 50.0);
        assert_eq!(position(&sp, &e, (0.0, 0.0), 1200)[0], 100.0);
    }

    #[test]
    fn foreground_splits_the_passes_and_sort_orders_them() {
        let fonts = super::super::hudelem::tests::test_fonts();
        let sp = ScreenPlacement::new(640.0, 480.0);
        let mat = |sort, foreground| HudElem {
            kind: 6,
            width: 10,
            height: 10,
            color: [255; 4],
            sort,
            foreground,
            material: sort as i32,
            ..HudElem::default()
        };
        let elems = [mat(2.0, false), mat(1.0, false), mat(3.0, true)];
        let name = |i: i32| Some(format!("m{i}"));
        let back = build(&elems, false, &fonts, &sp, 0, &|_| String::new(), &name);
        let names: Vec<&str> = back.iter().map(|q| q.texture.as_str()).collect();
        assert_eq!(names, ["m1", "m2"]);
        let front = build(&elems, true, &fonts, &sp, 0, &|_| String::new(), &name);
        assert_eq!(front.len(), 1);
    }
}
