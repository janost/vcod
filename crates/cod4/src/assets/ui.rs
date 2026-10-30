//! Fonts, menu lists and menus.

use super::material::{Material, material};
use super::sound::sound;
use super::{Id, asset, push};
use crate::walk::{Follow, Handle, S, W};
use crate::zone::{Block, Pointer};
use anyhow::Result;
use std::io::BufRead;

/// `Font_s`, 0x18.
#[derive(Clone, Debug)]
pub struct Font {
    pub name: String,
    pub pixel_height: i32,
    pub material: Option<Id<Material>>,
    pub glow_material: Option<Id<Material>>,
    pub glyphs: Vec<Glyph>,
}

/// `Glyph`, 0x18.
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub letter: u16,
    pub x0: i8,
    pub y0: i8,
    pub dx: u8,
    pub pixel_width: u8,
    pub pixel_height: u8,
    pub st: [f32; 4],
}

pub fn font<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<Font>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x18)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let mat = material(w, s.ptr(12), s.slot(12))?;
        let glow = material(w, s.ptr(16), s.slot(16))?;
        let n = s.u32(8) as usize;
        let glyphs = match w.follow(s.ptr(20), 4)? {
            Follow::Inline => {
                let g = w.load(0x18 * n)?;
                (0..n)
                    .map(|i| {
                        let e = g.elem(i, 0x18);
                        Glyph {
                            letter: e.u16(0),
                            x0: e.i8(2),
                            y0: e.i8(3),
                            dx: e.u8(4),
                            pixel_width: e.u8(5),
                            pixel_height: e.u8(6),
                            st: [e.f32(8), e.f32(12), e.f32(16), e.f32(20)],
                        }
                    })
                    .collect()
            }
            _ => {
                w.unresolved("glyphs");
                Vec::new()
            }
        };
        w.pop()?;
        Ok(push(
            &mut w.out.fonts,
            Font {
                name,
                pixel_height: s.i32(4),
                material: mat,
                glow_material: glow,
                glyphs,
            },
        ))
    })
}

/// `MenuList`, 0xC: name, `menuCount`, menu handles.
#[derive(Clone, Debug)]
pub struct MenuList {
    pub name: String,
    pub menus: Vec<Option<Id<Menu>>>,
}

pub fn menu_list<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<MenuList>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0xC)?;
        w.push(Block::Virtual);
        let name = w.xstring(s.ptr(0))?.unwrap_or_default();
        let mut menus = Vec::new();
        if w.inline_only(s.ptr(8), 4)? {
            let n = s.u32(4) as usize;
            let a = w.load(4 * n)?;
            for i in 0..n {
                menus.push(menu(w, a.ptr(4 * i), a.slot(4 * i))?);
            }
        }
        w.pop()?;
        Ok(push(&mut w.out.menu_lists, MenuList { name, menus }))
    })
}

/// `menuDef_t`, 0x11C.
#[derive(Clone, Debug)]
pub struct Menu {
    pub window: Window,
    pub font: Option<String>,
    pub on_open: Option<String>,
    pub on_close: Option<String>,
    pub on_esc: Option<String>,
    /// `onKey`'s `execKey` handlers: key code and script.
    pub on_key: Vec<(i32, String)>,
    /// `visibleExp` (+212).
    pub visible_exp: Statement,
    /// `soundName` (+224): the alias `Menus_Open` plays, the main menu's
    /// music.
    pub sound_name: Option<String>,
    pub items: Vec<Option<Item>>,
    /// The whole struct as loaded.
    pub raw: Vec<u8>,
}

/// `windowDef_t`, 0x9C, embedded in menus and items.
#[derive(Clone, Debug)]
pub struct Window {
    pub name: Option<String>,
    pub group: Option<String>,
    pub background: Option<Id<Material>>,
}

/// `itemDef_s`, 0x174.
#[derive(Clone, Debug)]
pub struct Item {
    pub window: Window,
    pub kind: i32,
    pub text: Option<String>,
    /// The string operands of `textExp` (`exp text(...)`), in order.
    pub text_exp: Vec<String>,
    pub action: Option<String>,
    pub dvar: Option<String>,
    /// The scripts at +236..+264: `mouseEnterText`, `mouseExitText`,
    /// `mouseEnter`, `mouseExit`, `action`'s neighbours `onAccept`,
    /// `onFocus` and `leaveFocus`.
    pub scripts: ItemScripts,
    /// `dvarTest` (+272), `enableDvar` (+280), `dvarFlags` (+284).
    pub dvar_test: Option<String>,
    pub enable_dvar: Option<String>,
    pub on_key: Vec<(i32, String)>,
    /// `visibleExp` .. `forecolorAExp` (+308 .. +364).
    pub exps: ItemExps,
    /// `typeData` (+300), the struct its `type` points it at.
    pub type_data: TypeData,
    pub raw: Vec<u8>,
}

/// What `itemDef_s.typeData` holds for the item's `type`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum TypeData {
    #[default]
    None,
    /// `editFieldDef_s` (0x20): the slider's, yes/no's, bind's and the
    /// edit fields'.
    Edit(EditField),
    /// `multiDef_s` (0x188).
    Multi(MultiDef),
    /// `ITEM_TYPE_DVARENUM`'s enum dvar name.
    Enum(String),
    /// `listBoxDef_s` (0x154).
    ListBox(ListBox),
}

/// `editFieldDef_s`: `minVal`, `maxVal`, `defVal`, `range`, `maxChars`,
/// `maxCharsGotoNext`, `maxPaintChars`, `paintOffset`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditField {
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub range: f32,
    pub max_chars: i32,
    pub max_chars_goto_next: i32,
    pub max_paint_chars: i32,
}

/// `multiDef_s`: `count` entries of `dvarList` (the labels) over
/// `dvarStr` (`strDef` 1) or `dvarValue` (`strDef` 0).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiDef {
    pub labels: Vec<String>,
    pub strings: Vec<String>,
    pub values: Vec<f32>,
    pub str_def: bool,
}

/// `listBoxDef_s`'s drawn parts: `elementWidth` +16, `elementHeight`
/// +20, `elementStyle` +24, `numColumns` +28, `columnInfo` +32 (16 of
/// `pos`, `width`, `maxChars`, `alignment`), `doubleClick` +288,
/// `notselectable` +292, `noScrollBars` +296, `usePaging` +300,
/// `selectBorder` +304, `disableColor` +320.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListBox {
    pub element_width: f32,
    pub element_height: f32,
    pub element_style: i32,
    /// `pos`, `width`, `maxChars`, `alignment` of each column.
    pub columns: Vec<[i32; 4]>,
    pub double_click: Option<String>,
    pub not_selectable: bool,
    pub no_scroll_bars: bool,
    pub select_border: [f32; 4],
    pub disable_color: [f32; 4],
}

/// An item's event scripts.
#[derive(Clone, Debug, Default)]
pub struct ItemScripts {
    pub mouse_enter_text: Option<String>,
    pub mouse_exit_text: Option<String>,
    pub mouse_enter: Option<String>,
    pub mouse_exit: Option<String>,
    pub on_accept: Option<String>,
    pub on_focus: Option<String>,
    pub leave_focus: Option<String>,
}

/// An item's eight expressions, each empty when the menu gave none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemExps {
    pub visible: Statement,
    pub text: Statement,
    pub material: Statement,
    pub rect_x: Statement,
    pub rect_y: Statement,
    pub rect_w: Statement,
    pub rect_h: Statement,
    pub forecolor_a: Statement,
}

/// A compiled menu expression: `expressionEntry`s in source (infix)
/// order.
pub type Statement = Vec<ExpEntry>;

/// `expressionEntry`, 0xC: type 0 an operator (`operationEnum`), type 1
/// an operand of data type 0 int, 1 float, 2 string.
#[derive(Clone, Debug, PartialEq)]
pub enum ExpEntry {
    Op(i32),
    Int(i32),
    Float(f32),
    Str(String),
    /// An entry the walk could not read (shared, or an unknown type).
    Unknown,
}

pub fn menu<R: BufRead>(w: &mut W<R>, p: Pointer, slot: u32) -> Result<Option<Id<Menu>>> {
    asset(w, p, slot, Handle::ASSET, |w| {
        let s = w.load(0x11C)?;
        w.push(Block::Virtual);
        let window = window(w, &s)?;
        let font = w.xstring(s.ptr(156))?;
        let on_open = w.xstring(s.ptr(196))?;
        let on_close = w.xstring(s.ptr(200))?;
        let on_esc = w.xstring(s.ptr(204))?;
        let on_key = key_handlers(w, s.ptr(208))?;
        let visible_exp = statement(w, &s, 212)?;
        w.xstring(s.ptr(220))?; // allowedBinding
        let sound_name = w.xstring(s.ptr(224))?;
        statement(w, &s, 264)?;
        statement(w, &s, 272)?;
        let mut items = Vec::new();
        if w.inline_only(s.ptr(280), 4)? {
            let n = s.i32(164).max(0) as usize;
            let a = w.load(4 * n)?;
            for i in 0..n {
                items.push(if w.inline_only(a.ptr(4 * i), 4)? {
                    Some(item(w)?)
                } else {
                    None
                });
            }
        }
        w.pop()?;
        Ok(push(
            &mut w.out.menus,
            Menu {
                window,
                font,
                on_open,
                on_close,
                on_esc,
                on_key,
                visible_exp,
                sound_name,
                items,
                raw: s.b,
            },
        ))
    })
}

fn window<R: BufRead>(w: &mut W<R>, s: &S) -> Result<Window> {
    let name = w.xstring(s.ptr(0))?;
    let group = w.xstring(s.ptr(52))?;
    let background = material(w, s.ptr(152), s.slot(152))?;
    Ok(Window {
        name,
        group,
        background,
    })
}

/// `ItemKeyHandler`, 0xC: a chain of `{ key, action, next }`.
fn key_handlers<R: BufRead>(w: &mut W<R>, mut p: Pointer) -> Result<Vec<(i32, String)>> {
    let mut out = Vec::new();
    while w.inline_only(p, 4)? {
        let k = w.load(0xC)?;
        if let Some(a) = w.xstring(k.ptr(4))? {
            out.push((k.i32(0), a));
        }
        p = k.ptr(8);
    }
    Ok(out)
}

/// `statement_s`, 8, at `o`: `numEntries` slots of 0xC-byte
/// `expressionEntry`, whose string operands (type 1, data type 2) load
/// their string.
fn statement<R: BufRead>(w: &mut W<R>, s: &S, o: usize) -> Result<Statement> {
    let mut out = Vec::new();
    if !w.inline_only(s.ptr(o + 4), 4)? {
        return Ok(out);
    }
    let n = s.i32(o).max(0) as usize;
    let a = w.load(4 * n)?;
    for i in 0..n {
        if !w.inline_only(a.ptr(4 * i), 4)? {
            out.push(ExpEntry::Unknown);
            continue;
        }
        let e = w.load(0xC)?;
        out.push(match (e.i32(0), e.i32(4)) {
            (0, _) => ExpEntry::Op(e.i32(4)),
            (1, 0) => ExpEntry::Int(e.i32(8)),
            (1, 1) => ExpEntry::Float(e.f32(8)),
            (1, 2) => w
                .xstring(e.ptr(8))?
                .map_or(ExpEntry::Unknown, ExpEntry::Str),
            _ => ExpEntry::Unknown,
        });
    }
    Ok(out)
}

/// `itemDef_s.textExp`'s offset; the statements run `visibleExp` (308)
/// to `forecolorAExp` (364).
const TEXT_EXP: usize = 316;

fn item<R: BufRead>(w: &mut W<R>) -> Result<Item> {
    let s = w.load(0x174)?;
    let window = window(w, &s)?;
    let text = w.xstring(s.ptr(224))?;
    let mut v = Vec::with_capacity(10);
    for o in (236..=272).step_by(4) {
        v.push(w.xstring(s.ptr(o))?);
    }
    let mut v = v.into_iter();
    let mut next = || v.next().flatten();
    let scripts = ItemScripts {
        mouse_enter_text: next(),
        mouse_exit_text: next(),
        mouse_enter: next(),
        mouse_exit: next(),
        ..ItemScripts::default()
    };
    let action = next();
    let scripts = ItemScripts {
        on_accept: next(),
        on_focus: next(),
        leave_focus: next(),
        ..scripts
    };
    let dvar = next();
    let dvar_test = next();
    let on_key = key_handlers(w, s.ptr(276))?;
    let enable_dvar = w.xstring(s.ptr(280))?;
    sound(w, s.ptr(288), s.slot(288))?;
    let kind = s.i32(180);
    let colour = |l: &S, o: usize| std::array::from_fn(|k| l.f32(o + 4 * k));
    let type_data = match kind {
        // ITEM_TYPE_LISTBOX
        6 => {
            if w.inline_only(s.ptr(300), 4)? {
                let l = w.load(0x154)?;
                let double_click = w.xstring(l.ptr(288))?;
                material(w, l.ptr(336), l.slot(336))?;
                let n = l.i32(28).clamp(0, 16) as usize;
                TypeData::ListBox(ListBox {
                    element_width: l.f32(16),
                    element_height: l.f32(20),
                    element_style: l.i32(24),
                    columns: (0..n)
                        .map(|c| std::array::from_fn(|k| l.i32(32 + 16 * c + 4 * k)))
                        .collect(),
                    double_click,
                    not_selectable: l.i32(292) != 0,
                    no_scroll_bars: l.i32(296) != 0,
                    select_border: colour(&l, 304),
                    disable_color: colour(&l, 320),
                })
            } else {
                TypeData::None
            }
        }
        // The edit field kinds: text, edit, numeric, slider, yes/no, bind,
        // valid file, decimal, upper-case edit.
        0 | 4 | 9 | 10 | 11 | 14 | 16 | 17 | 18 => {
            if w.inline_only(s.ptr(300), 4)? {
                let e = w.load(0x20)?;
                TypeData::Edit(EditField {
                    min: e.f32(0),
                    max: e.f32(4),
                    default: e.f32(8),
                    range: e.f32(12),
                    max_chars: e.i32(16),
                    max_chars_goto_next: e.i32(20),
                    max_paint_chars: e.i32(24),
                })
            } else {
                TypeData::None
            }
        }
        // ITEM_TYPE_MULTI: 32 labels, 32 strings, 32 values, the count.
        12 => {
            if w.inline_only(s.ptr(300), 4)? {
                let m = w.load(0x188)?;
                let labels = w.xstrings_in(&m, 0, 32)?;
                let strings = w.xstrings_in(&m, 128, 32)?;
                let n = m.i32(384).clamp(0, 32) as usize;
                TypeData::Multi(MultiDef {
                    labels: labels[..n]
                        .iter()
                        .map(|l| l.clone().unwrap_or_default())
                        .collect(),
                    strings: strings[..n]
                        .iter()
                        .map(|l| l.clone().unwrap_or_default())
                        .collect(),
                    values: (0..n).map(|i| m.f32(256 + 4 * i)).collect(),
                    str_def: m.i32(388) != 0,
                })
            } else {
                TypeData::None
            }
        }
        // ITEM_TYPE_DVARENUM
        13 => w
            .xstring(s.ptr(300))?
            .map_or(TypeData::None, TypeData::Enum),
        _ => TypeData::None,
    };
    let mut exps = Vec::with_capacity(8);
    for o in (308..=364).step_by(8) {
        exps.push(statement(w, &s, o)?);
    }
    let text_exp = exps[(TEXT_EXP - 308) / 8]
        .iter()
        .filter_map(|e| match e {
            ExpEntry::Str(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let mut exps = exps.into_iter();
    let mut next = || exps.next().unwrap_or_default();
    let exps = ItemExps {
        visible: next(),
        text: next(),
        material: next(),
        rect_x: next(),
        rect_y: next(),
        rect_w: next(),
        rect_h: next(),
        forecolor_a: next(),
    };
    Ok(Item {
        window,
        kind,
        text,
        text_exp,
        action,
        dvar,
        scripts,
        dvar_test,
        enable_dvar,
        on_key,
        exps,
        type_data,
        raw: s.b,
    })
}
