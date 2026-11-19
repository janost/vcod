//! The HUD's fonts: the zones' font sizes (`UI_SMALL`, `UI_BIG`,
//! `UI_EXTRA_BIG`) picked by scale, and the console font.

use std::path::Path;

use vcod_cod4::hud::HudAssets;

use super::{UI_BIG, UI_EXTRA_BIG, UI_SMALL};
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::font::{Font, Glyph};

/// The UI fonts `UI_GetFontHandle` picks from.
pub(crate) struct Fonts {
    pub(super) small: Font,
    normal: Font,
    pub(super) console: Font,
    big: Font,
    bold: Font,
    extra_big: Font,
    pub(super) objective: Font,
}

impl Fonts {
    pub(super) fn load(a: &HudAssets) -> Option<Fonts> {
        let f = |name: &str| {
            let (f, page) = a.font(name)?;
            Some(to_font(f, page.as_deref()?))
        };
        Some(Fonts {
            small: f("fonts/smallFont")?,
            normal: f("fonts/normalFont")?,
            console: f("fonts/consoleFont")?,
            big: f("fonts/bigFont")?,
            bold: f("fonts/boldFont")?,
            extra_big: f("fonts/extraBigFont")?,
            objective: f("fonts/objectiveFont")?,
        })
    }

    /// `UI_GetFontHandle`: enums 2, 3, 5
    /// and 6 name a font, 4 picks small, normal or bold and anything else
    /// small, normal, big or extra big by the scale on a 480-line screen.
    pub(crate) fn pick(&self, font_enum: i32, scale: f32, sp: &ScreenPlacement) -> &Font {
        let s = sp.virtual_to_real[1] * scale;
        match font_enum {
            2 => &self.big,
            3 => &self.small,
            5 => &self.console,
            6 => &self.objective,
            4 if s <= UI_SMALL => &self.small,
            4 if s < UI_BIG => &self.normal,
            4 => &self.bold,
            _ if s <= UI_SMALL => &self.small,
            _ if s >= UI_EXTRA_BIG => &self.extra_big,
            _ if s >= UI_BIG => &self.big,
            _ => &self.normal,
        }
    }
}

/// A CoD4 `Font_s` as the HUD's [`Font`]: glyph pixels scaled by
/// `48 / pixelHeight` as CoD2's (`R_NormalizedTextScale`), on its page's
/// `.iwi`.
fn to_font(f: &vcod_cod4::assets::ui::Font, page: &str) -> Font {
    let none = Glyph {
        height: 0,
        width: 0,
        top: 0.0,
        bearing: 0.0,
        advance: 0.0,
        image_width: 0,
        image_height: 0,
        s: 0.0,
        t: 0.0,
        s2: 0.0,
        t2: 0.0,
        page: 0,
    };
    let mut glyphs = vec![none; 256];
    for g in &f.glyphs {
        if let Some(slot) = glyphs.get_mut(usize::from(g.letter)) {
            *slot = Glyph {
                height: i32::from(g.pixel_height),
                width: i32::from(g.pixel_width),
                top: -f32::from(g.y0),
                bearing: f32::from(g.x0),
                advance: f32::from(g.dx),
                image_width: i32::from(g.pixel_width),
                image_height: i32::from(g.pixel_height),
                s: g.st[0],
                t: g.st[1],
                s2: g.st[2],
                t2: g.st[3],
                page: 0,
            };
        }
    }
    // A letter the font lacks draws as `?`, as unknown bytes do.
    let q = glyphs[usize::from(b'?')];
    for g in glyphs.iter_mut().skip(32) {
        if g.advance == 0.0 {
            *g = q;
        }
    }
    let max_height = (0x20u8..=0x7e)
        .map(|b| glyphs[b as usize].top as i32)
        .max()
        .unwrap_or(0)
        .max(1);
    let glyph_scale = 48.0 / f.pixel_height.max(1) as f32;
    let page = format!("images/{page}.iwi");
    Font {
        size: f.pixel_height.max(1) as u32,
        glyphs,
        glyph_scale,
        line_advance: f.pixel_height as f32 * glyph_scale,
        max_height,
        page: page.clone(),
        pages: vec![page],
    }
}

/// The console's `fonts/consoleFont` (`cls.consoleFont`) out of
/// `localized_code_post_gfx_mp` of the install whose `main/` is
/// `main_dir` (cod4-hud.md 1); `None` (warned) when that zone lacks it.
pub(crate) fn console_font(main_dir: &Path) -> Option<Font> {
    let install = main_dir
        .parent()
        .and_then(vcod_cod4::install::Install::at)?;
    let a = HudAssets::load_zones(&install, ["localized_code_post_gfx_mp"])
        .inspect_err(|e| log::warn!("cod4 console: {e:#}"))
        .ok()?;
    let (f, page) = a.font("fonts/consoleFont")?;
    Some(to_font(f, page.as_deref()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CoD4's iwds hold no `.dat` font: the console draws in the zone's
    /// `consoleFont`, its typed line among the glyphs.
    #[test]
    fn cod4_console_draws_its_typed_line() {
        use winit::keyboard::KeyCode;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let font = console_font(&install.root().join("main")).expect("consoleFont");
        let mut c = crate::console::Console::with_font(Some(font));
        let glyphs = |c: &crate::console::Console| {
            c.build(640.0, 480.0, 512)
                .iter()
                .filter(|q| q.texture == "images/gamefonts_pc.iwi")
                .count()
        };
        c.toggle();
        c.update(1000.0, 3.0, false);
        let empty = glyphs(&c);
        assert!(empty > 0, "the prompt and the version draw");
        for (k, t) in [(KeyCode::KeyH, "h"), (KeyCode::KeyI, "i")] {
            assert_eq!(c.key(k, Some(t), |_| Vec::new()), None);
        }
        assert_eq!(glyphs(&c), empty + 2);
        let line = c.key(KeyCode::Enter, None, |_| Vec::new());
        assert_eq!(line.as_deref(), Some("hi"));
    }
}
