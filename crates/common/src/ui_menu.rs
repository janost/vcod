//! The front end's menu files: `ui_mp/*.menu` and `ui/*.menu`, the full
//! `menuDef` / `itemDef` grammar the UI module reads, as far as the main menu
//! the server browser and the options screens use it
//! (docs/research/cod11-front-end.md). Unlike
//! [`crate::menu`], which keeps only what a script menu needs, this keeps the
//! layout: rects, colours, styles, text placement and the item scripts.

use std::collections::HashMap;

use crate::asset_source::{Asset, AssetKind, AssetSource};
use crate::game::Game;
use crate::menu::{CvarGate, block, tokenize};

/// `ITEM_TYPE_*` from `menudef.h`.
pub const ITEM_TYPE_TEXT: i32 = 0;
pub const ITEM_TYPE_BUTTON: i32 = 1;
pub const ITEM_TYPE_EDITFIELD: i32 = 4;
pub const ITEM_TYPE_LISTBOX: i32 = 6;
pub const ITEM_TYPE_OWNERDRAW: i32 = 8;
pub const ITEM_TYPE_NUMERICFIELD: i32 = 9;
pub const ITEM_TYPE_SLIDER: i32 = 10;
pub const ITEM_TYPE_YESNO: i32 = 11;
pub const ITEM_TYPE_MULTI: i32 = 12;
pub const ITEM_TYPE_BIND: i32 = 13;
/// CoD2's `ITEM_TYPE_DVARENUM` (its 13), which CoD1 lacks; CoD2's types
/// past its bind (14) read as `100 +` their number.
pub const ITEM_TYPE_DVARENUM: i32 = 113;
/// CoD2's `ITEM_TYPE_DECIMALFIELD` (its 17): an edit field that shows its
/// float dvar to two places.
pub const ITEM_TYPE_DECIMALFIELD: i32 = 117;

/// `WINDOW_STYLE_*`.
pub const WINDOW_STYLE_FILLED: i32 = 1;
pub const WINDOW_STYLE_SHADER: i32 = 3;
/// The shader the item's `cvar` names (`viewmap`'s layout image).
pub const WINDOW_STYLE_CVAR_SHADER: i32 = 6;

/// `ITEM_ALIGN_*`.
pub const ITEM_ALIGN_CENTER: i32 = 1;
pub const ITEM_ALIGN_RIGHT: i32 = 2;

/// `FEEDER_*` from `ui_mp/menudef.h`.
pub const FEEDER_SERVERS: i32 = 2;
pub const FEEDER_ALLMAPS: i32 = 4;
pub const FEEDER_PLAYER_LIST: i32 = 7;
pub const FEEDER_MODS: i32 = 9;
pub const FEEDER_SERVERSTATUS: i32 = 13;
/// CoD2's `FEEDER_PLAYER_PROFILES` (`ui/menudefinition.h`).
pub const FEEDER_PLAYER_PROFILES: i32 = 0x18;

/// One `columns` entry of a list box: x offset, width and the character cap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    pub pos: f32,
    pub width: f32,
    pub max_chars: usize,
}

/// `cvarFloat "name" default min max`: a slider's range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderRange {
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

/// `x y w h` in the 640x480 grid.
pub type Rect = [f32; 4];

#[derive(Debug, Clone)]
pub struct UiItem {
    pub name: String,
    pub group: String,
    pub kind: i32,
    pub style: i32,
    /// Relative to the menu's rect until [`UiMenu`] places it.
    pub rect: Rect,
    pub text: String,
    pub text_scale: f32,
    pub text_align: i32,
    pub text_align_x: f32,
    pub text_align_y: f32,
    pub fore: [f32; 4],
    pub back: [f32; 4],
    pub border: i32,
    pub border_size: f32,
    pub border_color: [f32; 4],
    pub outline_color: [f32; 4],
    pub background: Option<String>,
    pub visible: bool,
    pub decoration: bool,
    /// Text wraps at the rect's width (`autowrapped`).
    pub autowrap: bool,
    pub cvar: Option<String>,
    pub gate: Option<CvarGate>,
    pub ownerdraw: i32,
    pub ownerdraw_flag: i32,
    pub feeder: Option<i32>,
    pub element_height: f32,
    pub columns: Vec<Column>,
    /// `cvarFloatList`: an `ITEM_TYPE_MULTI`'s labels and the values they set.
    pub float_list: Vec<(String, f32)>,
    /// `cvarStrList`: the same with string values.
    pub str_list: Vec<(String, String)>,
    /// `cvarFloat`: an `ITEM_TYPE_SLIDER`'s cvar is `cvar`, this its range.
    pub slider: Option<SliderRange>,
    /// CoD2's `dvarEnumList`: the enum dvar an `ITEM_TYPE_DVARENUM` steps
    /// its `dvar` through (item +0x2ec, `ItemParse_dvarEnumList` Mac
    /// 0x1a9bd4).
    pub enum_list: Option<String>,
    /// CoD2's `ITEM_TYPE_VALIDFILEFIELD`: an edit field that takes
    /// letters, digits, `_` and `-` only (`I_isforfilename`, Mac 0x2942c).
    pub file_name_only: bool,
    /// CoD2's `accept`: the script Enter runs in the edit field
    /// (`Item_TextField_HandleKey`, Mac 0x16ee5e).
    pub accept: Vec<String>,
    /// An edit field's `maxChars` (0: no cap) and `maxPaintChars`.
    pub max_chars: usize,
    pub max_paint_chars: usize,
    /// Scripts as token streams; [`script_commands`] splits one.
    pub action: Vec<String>,
    pub double_click: Vec<String>,
    pub mouse_enter: Vec<String>,
    pub mouse_exit: Vec<String>,
    /// `onFocus` runs when the item takes the focus, `leaveFocus` whenever
    /// any item of its menu loses it (`Menu_ClearFocus`, 0x4000fe70).
    pub on_focus: Vec<String>,
    pub leave_focus: Vec<String>,
    /// `focusSound`: the sound alias the item plays as it takes the focus.
    pub focus_sound: Option<String>,
    /// `maxCharsGotoNext`: an edit field that fills to `maxChars` steps to
    /// the next item.
    pub max_chars_goto_next: bool,
    /// CoD2's `rect` alignment: `HORIZONTAL_ALIGN_*`, `VERTICAL_ALIGN_*`
    /// (0, the default, with neither given).
    pub align: [i32; 2],
}

impl Default for UiItem {
    fn default() -> Self {
        UiItem {
            name: String::new(),
            group: String::new(),
            kind: ITEM_TYPE_TEXT,
            style: 0,
            rect: [0.0; 4],
            text: String::new(),
            text_scale: 0.55,
            text_align: 0,
            text_align_x: 0.0,
            text_align_y: 0.0,
            fore: [1.0; 4],
            back: [0.0; 4],
            border: 0,
            border_size: 1.0,
            border_color: [0.0; 4],
            outline_color: [0.0; 4],
            background: None,
            visible: false,
            decoration: false,
            autowrap: false,
            cvar: None,
            gate: None,
            ownerdraw: 0,
            ownerdraw_flag: 0,
            feeder: None,
            element_height: 0.0,
            columns: Vec::new(),
            float_list: Vec::new(),
            str_list: Vec::new(),
            slider: None,
            enum_list: None,
            file_name_only: false,
            accept: Vec::new(),
            max_chars: 0,
            max_paint_chars: 0,
            action: Vec::new(),
            double_click: Vec::new(),
            mouse_enter: Vec::new(),
            mouse_exit: Vec::new(),
            on_focus: Vec::new(),
            leave_focus: Vec::new(),
            focus_sound: None,
            max_chars_goto_next: false,
            align: [0; 2],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UiMenu {
    pub name: String,
    pub rect: Rect,
    pub fullscreen: bool,
    pub popup: bool,
    /// `outOfBoundsClick` (0x40018620 sets `WINDOW_OOB_CLICK`, 0x20000): a
    /// click off the focused menu closes it. No stock menu sets it.
    pub out_of_bounds_click: bool,
    /// `visible`: the cgame paints only visible `hud.menu` menus.
    pub visible: bool,
    pub focus_color: [f32; 4],
    pub on_open: Vec<String>,
    pub on_close: Vec<String>,
    pub on_esc: Vec<String>,
    /// `execKey "c"` / `execKeyInt n`: a script per key number, which a key
    /// press runs ahead of the focus keys (`Menu_HandleKey`, 0x400136b0).
    pub exec_keys: Vec<(u8, Vec<String>)>,
    /// Rects already offset by the menu's origin.
    pub items: Vec<UiItem>,
    /// CoD2's `rect` alignment, as [`UiItem::align`].
    pub align: [i32; 2],
    /// CoD2's `blurWorld`: the world blurred behind the menu, 0 for none.
    pub blur_world: f32,
}

/// Inlines `#include "path"` (read through `include`) and collects
/// `#define NAME value`; every other `#` line is dropped. Includes nest up
/// to a fixed depth, so a cycle stops instead of recursing forever.
fn preprocess(
    text: &str,
    include: &dyn Fn(&str) -> Option<String>,
    defines: &mut HashMap<String, Vec<String>>,
    depth: u32,
) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#include") {
            let path = rest.trim().trim_matches('"');
            match include(path) {
                Some(inc) if depth < 8 => {
                    out.push_str(&preprocess(&inc, include, defines, depth + 1));
                }
                _ => log::warn!("ui: cannot include {path}"),
            }
        } else if let Some(rest) = trimmed.strip_prefix("#define") {
            // Tokenized like the file, so a quoted body is one token.
            let mut words = tokenize(rest).into_iter();
            if let Some(name) = words.next() {
                defines.insert(name, words.collect());
            }
        } else if !trimmed.starts_with('#') {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// `text` as tokens after the preprocessor: `#include`s inlined through
/// `include`, and every token a `#define` names replaced by its body.
pub(crate) fn expand(text: &str, include: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut defines = HashMap::new();
    let text = preprocess(text, include, &mut defines, 0);
    let mut out = Vec::new();
    for t in tokenize(&text) {
        substitute(t, &defines, 0, &mut out);
    }
    out
}

/// `t` with its `#define` body pushed in its place, and that body's own
/// names in turn (CoD2's `HORIZONTAL_ALIGN_DEFAULT` names
/// `HORIZONTAL_ALIGN_SUBLEFT`), to a fixed depth so a cycle ends.
fn substitute(
    t: String,
    defines: &HashMap<String, Vec<String>>,
    depth: u32,
    out: &mut Vec<String>,
) {
    match defines.get(&t) {
        Some(body) if depth < 8 => {
            for b in body {
                substitute(b.clone(), defines, depth + 1, out);
            }
        }
        _ => out.push(t),
    }
}

/// Every `menuDef` in `text`. `include` reads an `#include`d file.
pub fn parse_file(text: &str, include: &dyn Fn(&str) -> Option<String>) -> Vec<UiMenu> {
    parse_menu_file(text, include).menus
}

/// A menu file: its menus and what its `assetGlobalDef` sets.
#[derive(Debug, Default)]
pub struct MenuFile {
    pub menus: Vec<UiMenu>,
    /// `itemFocusSound`, the focus sound of an item with no `focusSound`
    /// (`ui_mp_x86.dll` 0x40007fe8).
    pub item_focus_sound: Option<String>,
    /// `cursor`, the shader the UI draws at the mouse while a menu is up
    /// (`ui_mp_x86.dll` 0x40008060).
    pub cursor: Option<String>,
    /// What retail's parser would refuse: a CoD2 file's keywords its
    /// keyword tables lack. Not checked for CoD1.
    pub errors: Vec<String>,
}

/// Uncached: the UI and the HUD keep the menus they take out of it. The
/// file and each `#include` read through [`AssetSource::read`], so a
/// [`crate::pk3::Pk3Fs`] marks their paks.
impl Asset for MenuFile {
    const KIND: AssetKind = AssetKind::Menu;

    fn find(src: &dyn AssetSource, path: &str) -> anyhow::Result<Self> {
        let read = |p: &str| {
            src.read(p)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        };
        let Some(text) = read(path) else {
            anyhow::bail!("{path} not found");
        };
        Ok(parse_menu_file_in(src.game(), &text, &read))
    }
}

/// [`parse_file`] with the file's `assetGlobalDef` keys vcod reads.
pub fn parse_menu_file(text: &str, include: &dyn Fn(&str) -> Option<String>) -> MenuFile {
    parse_menu_file_in(Game::Cod1, text, include)
}

/// [`parse_menu_file`] in `game`'s grammar. CoD2's renames each `cvar`
/// keyword `dvar` (read onto the same fields), gives `rect` two optional
/// alignment numbers and adds keywords vcod skips by their arguments
/// (docs/research/cod2-fonts-menus.md, section 2).
pub fn parse_menu_file_in(
    game: Game,
    text: &str,
    include: &dyn Fn(&str) -> Option<String>,
) -> MenuFile {
    let tokens = expand(text, include);
    let mut file = MenuFile::default();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].eq_ignore_ascii_case("menuDef") {
            i += 1;
            let menu = parse_menu(block(&tokens, &mut i), game, &mut file.errors);
            file.menus.push(menu);
        } else if tokens[i].eq_ignore_ascii_case("assetGlobalDef") {
            i += 1;
            let body = block(&tokens, &mut i);
            let value = |key: &str| {
                let k = body.iter().position(|t| t.eq_ignore_ascii_case(key))?;
                body.get(k + 1).cloned()
            };
            file.item_focus_sound = value("itemFocusSound").or(file.item_focus_sound);
            file.cursor = value("cursor").or(file.cursor);
        } else {
            i += 1;
        }
    }
    file
}

/// `ui_mp/menus.txt`'s file names in order, as `UI_LoadMenus`
/// (`ui_mp_x86.dll` 0x400085b0) reads a menu list: tokens up to the first
/// bare `}`, each `loadMenu { ... }` block naming one or more files, other
/// tokens skipped (docs/research/cod11-front-end.md, section 1).
pub fn menu_list(text: &str) -> Vec<String> {
    let tokens = expand(text, &|_| None);
    let mut files = Vec::new();
    let mut i = 0;
    while let Some(t) = tokens.get(i) {
        if t == "}" {
            break;
        }
        i += 1;
        if !t.eq_ignore_ascii_case("loadMenu") {
            continue;
        }
        // A `loadMenu` without its block ends the list.
        if tokens.get(i).map(String::as_str) != Some("{") {
            break;
        }
        i += 1;
        while let Some(f) = tokens.get(i) {
            i += 1;
            if f == "}" {
                break;
            }
            files.push(f.clone());
        }
    }
    files
}

/// A game type the UI lists: its script's name (`dm`, the key a server's
/// `gametype` is matched against) and the localization reference its
/// `<script>.txt` names (`DM_DEATHMATCH`), or the script's name without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameType {
    pub script: String,
    pub name: String,
}

/// `UI_LoadGameTypes` (`ui_mp_x86.dll` 0x4000cf80): every
/// `maps/mp/gametypes/*.gsc` whose name does not start with `_`, at most 31
/// (the join list's 32 slots hold "All" first), each named by the first
/// token of its `.txt` (docs/research/cod11-front-end.md, "The game type
/// filter"), in `FS_GetFileList`'s search-path order ([`Pk3Fs::file_list`]).
///
/// [`Pk3Fs::file_list`]: crate::pk3::Pk3Fs::file_list
pub fn game_types(fs: &crate::pk3::Pk3Fs) -> Vec<GameType> {
    const DIR: &str = "maps/mp/gametypes/";
    const MAX: usize = 31;
    let mut out = Vec::new();
    for path in fs.file_list(DIR, ".gsc") {
        let name = &path[DIR.len()..];
        let script = &name[..name.len() - ".gsc".len()];
        if script.starts_with('_') {
            continue;
        }
        if out.len() == MAX {
            log::warn!("Too many game type scripts found! Only loading the first {MAX}");
            break;
        }
        let txt = format!("{DIR}{script}.txt");
        let name = fs
            .read(&txt)
            .and_then(|b| tokenize(&String::from_utf8_lossy(&b)).into_iter().next())
            .unwrap_or_else(|| {
                log::warn!("menu file not found: {txt}, using default");
                script.to_string()
            });
        out.push(GameType {
            script: script.to_string(),
            name,
        });
    }
    if out.is_empty() {
        log::warn!("No game type scripts found in maps/mp/gametypes folder");
    }
    out
}

/// `cl_language`'s names by index (CoDMP.exe table at 0x571b30, read through
/// UI syscall 2 at 0x4a9a00; an index past the table reads `english`).
const LANGUAGES: [&str; 14] = [
    "english",
    "french",
    "german",
    "italian",
    "spanish",
    "british",
    "russian",
    "polish",
    "korean",
    "taiwanese",
    "japanese",
    "chinese",
    "thai",
    "leet",
];

/// The path the UI tries before a menu file's own (`ui_mp_x86.dll`
/// 0x400084f7..0x40008566 for a list's files, 0x400083d6 for a script
/// menu): `<dir>/<language>/<file>` when `cl_language` reads non-zero
/// through `atoi`, else none. English is 0, so stock installs never try it
/// (docs/research/cod11-front-end.md, section 1).
pub fn localized_menu_path(path: &str, cl_language: &str) -> Option<String> {
    let t = cl_language.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i32, |n, d| {
            n.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
        });
    let index = if neg { n.wrapping_neg() } else { n };
    if index == 0 {
        return None;
    }
    let language = usize::try_from(index)
        .ok()
        .and_then(|i| LANGUAGES.get(i))
        .unwrap_or(&LANGUAGES[0]);
    let split = path.rfind('/').map_or(0, |i| i + 1);
    let (dir, file) = path.split_at(split);
    Some(format!("{dir}{language}/{file}"))
}

/// Q3's `atof`/`atoi` plus the `0x` hex `menudef.h` uses for flags.
fn num(t: Option<&String>) -> f32 {
    let Some(t) = t else { return 0.0 };
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).unwrap_or(0) as f32;
    }
    t.parse().unwrap_or(0.0)
}

fn nums<const N: usize>(tokens: &[String], i: usize) -> [f32; N] {
    std::array::from_fn(|k| num(tokens.get(i + 1 + k)))
}

/// CoD2's `rect`: four numbers, then up to two alignment integers on the
/// same line. The tokens carry no lines, but no keyword is a number.
fn rect_align(tokens: &[String], i: usize, align: &mut [i32; 2]) -> usize {
    let mut n = 0;
    while n < 2 {
        match tokens.get(i + 5 + n).and_then(|t| t.parse::<i32>().ok()) {
            Some(v) => align[n] = v,
            None => break,
        }
        n += 1;
    }
    5 + n
}

fn parse_menu(body: &[String], game: Game, errors: &mut Vec<String>) -> UiMenu {
    let cod2 = game == Game::Cod2;
    let mut menu = UiMenu {
        focus_color: [1.0; 4],
        ..UiMenu::default()
    };
    let mut i = 0;
    while i < body.len() {
        let key = body[i].to_ascii_lowercase();
        if cod2 && key != "{" && cod2::menu_arity(&key).is_none() {
            errors.push(format!("{}: unknown menu keyword {}", menu.name, body[i]));
        }
        match key.as_str() {
            "name" => {
                menu.name = body.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "rect" => {
                menu.rect = nums::<4>(body, i);
                i += if cod2 {
                    rect_align(body, i, &mut menu.align)
                } else {
                    5
                };
            }
            "blurworld" if cod2 => {
                menu.blur_world = num(body.get(i + 1));
                i += 2;
            }
            "fullscreen" => {
                menu.fullscreen = num(body.get(i + 1)) != 0.0;
                i += 2;
            }
            "popup" => {
                menu.popup = true;
                i += 1;
            }
            "outofboundsclick" => {
                menu.out_of_bounds_click = true;
                i += 1;
            }
            "visible" => {
                menu.visible = num(body.get(i + 1)) != 0.0;
                i += 2;
            }
            "focuscolor" => {
                menu.focus_color = nums::<4>(body, i);
                i += 5;
            }
            "onopen" | "onclose" | "onesc" => {
                i += 1;
                let script = block(body, &mut i).to_vec();
                match key.as_str() {
                    "onopen" => menu.on_open = script,
                    "onclose" => menu.on_close = script,
                    _ => menu.on_esc = script,
                }
            }
            "itemdef" => {
                i += 1;
                let mut item = parse_item(block(body, &mut i), game, errors);
                item.rect[0] += menu.rect[0];
                item.rect[1] += menu.rect[1];
                menu.items.push(item);
            }
            // `MenuParse_execKey` (0x40018780) keys on the string's first
            // byte, `execKeyInt` (0x400187c0) on the number.
            "execkey" | "execkeyint" => {
                let arg = body.get(i + 1).cloned().unwrap_or_default();
                let n = if key == "execkey" {
                    arg.bytes().next().unwrap_or(0)
                } else {
                    num(Some(&arg)) as u8
                };
                i += 2;
                let script = block(body, &mut i).to_vec();
                if n != 0 {
                    menu.exec_keys.retain(|(k, _)| *k != n);
                    menu.exec_keys.push((n, script));
                }
            }
            // Any other block is skipped whole.
            "{" => {
                block(body, &mut i);
            }
            _ if cod2 => cod2::skip(body, &mut i, cod2::menu_arity(&key)),
            _ => i += 1,
        }
    }
    menu
}

fn parse_item(tokens: &[String], game: Game, errors: &mut Vec<String>) -> UiItem {
    let cod2 = game == Game::Cod2;
    let mut item = UiItem::default();
    let mut cvar_test: Option<String> = None;
    let mut show: Option<(bool, Vec<String>)> = None;
    let mut i = 0;
    let s = |i: usize| tokens.get(i + 1).cloned().unwrap_or_default();
    while i < tokens.len() {
        let key = tokens[i].to_ascii_lowercase();
        if cod2 && key != "{" && cod2::item_arity(&key).is_none() {
            errors.push(format!(
                "{}: unknown menu item keyword {}",
                item.name, tokens[i]
            ));
        }
        let mut skip = 2;
        match key.as_str() {
            "name" => item.name = s(i),
            "group" => item.group = s(i),
            "type" => {
                item.kind = num(tokens.get(i + 1)) as i32;
                if cod2 {
                    // `ITEM_TYPE_VALIDFILEFIELD` (16): an edit field that
                    // takes `I_isforfilename`'s characters only.
                    item.file_name_only = item.kind == 16;
                    item.kind = cod2::item_type(item.kind);
                }
            }
            "style" => item.style = num(tokens.get(i + 1)) as i32,
            "rect" => {
                item.rect = nums::<4>(tokens, i);
                skip = if cod2 {
                    rect_align(tokens, i, &mut item.align)
                } else {
                    5
                };
            }
            // `ItemParse_origin`: shifts the rect parsed so far; a `rect`
            // after it starts over.
            "origin" => {
                let [x, y] = nums::<2>(tokens, i);
                item.rect[0] += x;
                item.rect[1] += y;
                skip = 3;
            }
            "text" => item.text = s(i),
            "textscale" => item.text_scale = num(tokens.get(i + 1)),
            "textalign" => item.text_align = num(tokens.get(i + 1)) as i32,
            "textalignx" => item.text_align_x = num(tokens.get(i + 1)),
            "textaligny" => item.text_align_y = num(tokens.get(i + 1)),
            "forecolor" | "backcolor" | "bordercolor" | "outlinecolor" => {
                let c = nums::<4>(tokens, i);
                match key.as_str() {
                    "forecolor" => item.fore = c,
                    "backcolor" => item.back = c,
                    "bordercolor" => item.border_color = c,
                    _ => item.outline_color = c,
                }
                skip = 5;
            }
            "border" => item.border = num(tokens.get(i + 1)) as i32,
            "bordersize" => item.border_size = num(tokens.get(i + 1)),
            "background" => item.background = Some(s(i)),
            "visible" => item.visible = num(tokens.get(i + 1)) != 0.0,
            "decoration" => {
                item.decoration = true;
                skip = 1;
            }
            "autowrapped" => {
                item.autowrap = true;
                skip = 1;
            }
            "cvar" | "dvar" => item.cvar = Some(s(i)),
            "cvartest" | "dvartest" => cvar_test = Some(s(i)),
            "showcvar" | "hidecvar" | "showdvar" | "hidedvar" => {
                let shown = key.starts_with("show");
                i += 1;
                show = Some((shown, block(tokens, &mut i).to_vec()));
                continue;
            }
            "ownerdraw" => {
                item.ownerdraw = num(tokens.get(i + 1)) as i32;
                // An ownerdraw is its own item type (`ITEM_TYPE_OWNERDRAW`).
                item.kind = ITEM_TYPE_OWNERDRAW;
            }
            "ownerdrawflag" => item.ownerdraw_flag |= num(tokens.get(i + 1)) as i32,
            "feeder" => item.feeder = Some(num(tokens.get(i + 1)) as i32),
            "elementheight" => item.element_height = num(tokens.get(i + 1)),
            "columns" => {
                let n = num(tokens.get(i + 1)) as usize;
                item.columns = (0..n)
                    .map(|c| {
                        let at = i + 2 + c * 3;
                        Column {
                            pos: num(tokens.get(at)),
                            width: num(tokens.get(at + 1)),
                            max_chars: num(tokens.get(at + 2)) as usize,
                        }
                    })
                    .collect();
                skip = 2 + n * 3;
            }
            "action" | "doubleclick" | "mouseenter" | "mouseexit" | "onfocus" | "leavefocus"
            | "accept" => {
                i += 1;
                let script = block(tokens, &mut i).to_vec();
                match key.as_str() {
                    "action" => item.action = script,
                    "accept" => item.accept = script,
                    "doubleclick" => item.double_click = script,
                    "mouseenter" => item.mouse_enter = script,
                    "onfocus" => item.on_focus = script,
                    "leavefocus" => item.leave_focus = script,
                    _ => item.mouse_exit = script,
                }
                continue;
            }
            "cvarfloat" | "dvarfloat" => {
                item.cvar = Some(s(i));
                let [default, min, max] = std::array::from_fn(|k| num(tokens.get(i + 2 + k)));
                item.slider = Some(SliderRange { default, min, max });
                skip = 5;
            }
            "focussound" => item.focus_sound = Some(s(i)),
            "dvarenumlist" => item.enum_list = Some(s(i)),
            // 0x40017b10 sets the edit data's `+0x14`.
            "maxcharsgotonext" => {
                item.max_chars_goto_next = true;
                skip = 1;
            }
            "maxchars" => item.max_chars = num(tokens.get(i + 1)) as usize,
            "maxpaintchars" => item.max_paint_chars = num(tokens.get(i + 1)) as usize,
            "cvarstrlist" | "dvarstrlist" => {
                i += 1;
                // The stock lists separate their entries with commas.
                let words: Vec<&String> = block(tokens, &mut i)
                    .iter()
                    .filter(|t| t.as_str() != ",")
                    .collect();
                item.str_list = words
                    .chunks(2)
                    .map(|p| {
                        (
                            p[0].clone(),
                            p.get(1).map_or(String::new(), |v| v.to_string()),
                        )
                    })
                    .collect();
                continue;
            }
            "cvarfloatlist" | "dvarfloatlist" => {
                i += 1;
                item.float_list = block(tokens, &mut i)
                    .chunks(2)
                    .map(|p| (p[0].clone(), num(p.get(1))))
                    .collect();
                continue;
            }
            "{" => {
                // An unhandled key's block.
                block(tokens, &mut i);
                continue;
            }
            _ if cod2 => {
                cod2::skip(tokens, &mut i, cod2::item_arity(&key));
                continue;
            }
            _ => skip = 1,
        }
        i += skip;
    }
    if let (Some(cvar), Some((show, values))) = (cvar_test, show) {
        item.gate = Some(CvarGate {
            cvar,
            show,
            values: values.into_iter().filter(|v| v != ";").collect(),
        });
    }
    item
}

/// CoD2's menu grammar: the keyword tables (`itemParseKeywords` and
/// `menuParseKeywords`, CoD2MP Mac 1.3 0x34c8a0 and 0x34cbc0) with what
/// each keyword reads after it.
mod cod2 {
    use crate::menu::block;

    #[derive(Clone, Copy)]
    pub(super) enum Arity {
        /// That many tokens.
        Args(usize),
        /// A `{ }` script or list.
        Block,
        /// One token, then a block (`execKey`).
        ArgBlock,
        /// `rect`: four numbers and up to two alignments.
        Rect,
        /// `columns n` and three numbers per column.
        Columns,
    }
    use Arity::*;

    /// Item keywords, lowercased, in the table's order.
    const ITEM: [(&str, Arity); 65] = [
        ("name", Args(1)),
        ("text", Args(1)),
        ("textfile", Args(1)),
        ("textsavegame", Args(0)),
        ("group", Args(1)),
        ("rect", Rect),
        ("origin", Args(2)),
        ("style", Args(1)),
        ("decoration", Args(0)),
        ("notselectable", Args(0)),
        ("noscrollbars", Args(0)),
        ("wrapped", Args(0)),
        ("autowrapped", Args(0)),
        ("horizontalscroll", Args(0)),
        ("type", Args(1)),
        ("elementwidth", Args(1)),
        ("elementheight", Args(1)),
        ("feeder", Args(1)),
        ("elementtype", Args(1)),
        ("columns", Columns),
        ("border", Args(1)),
        ("bordersize", Args(1)),
        ("visible", Args(1)),
        ("ownerdraw", Args(1)),
        ("align", Args(1)),
        ("textalign", Args(1)),
        ("textalignx", Args(1)),
        ("textaligny", Args(1)),
        ("textscale", Args(1)),
        ("textstyle", Args(1)),
        ("textfont", Args(1)),
        ("backcolor", Args(4)),
        ("forecolor", Args(4)),
        ("bordercolor", Args(4)),
        ("outlinecolor", Args(4)),
        ("background", Args(1)),
        ("onfocus", Block),
        ("leavefocus", Block),
        ("mouseenter", Block),
        ("mouseexit", Block),
        ("mouseentertext", Block),
        ("mouseexittext", Block),
        ("action", Block),
        ("accept", Block),
        ("special", Args(1)),
        ("dvar", Args(1)),
        ("maxchars", Args(1)),
        ("maxcharsgotonext", Args(0)),
        ("maxpaintchars", Args(1)),
        ("focussound", Args(1)),
        ("dvarfloat", Args(4)),
        ("dvarstrlist", Block),
        ("dvarfloatlist", Block),
        ("dvarenumlist", Args(1)),
        ("ownerdrawflag", Args(1)),
        ("enabledvar", Block),
        ("dvartest", Args(1)),
        ("disabledvar", Block),
        ("showdvar", Block),
        ("hidedvar", Block),
        ("focusdvar", Block),
        ("cinematic", Args(1)),
        ("doubleclick", Block),
        ("execkey", ArgBlock),
        ("execkeyint", ArgBlock),
    ];

    /// Menu keywords, lowercased, in the table's order.
    const MENU: [(&str, Arity); 32] = [
        ("name", Args(1)),
        ("fullscreen", Args(1)),
        ("rect", Rect),
        ("style", Args(1)),
        ("visible", Args(1)),
        ("onopen", Block),
        ("onclose", Block),
        ("onesc", Block),
        ("border", Args(1)),
        ("bordersize", Args(1)),
        ("backcolor", Args(4)),
        ("forecolor", Args(4)),
        ("bordercolor", Args(4)),
        ("focuscolor", Args(4)),
        ("disablecolor", Args(4)),
        ("outlinecolor", Args(4)),
        ("background", Args(1)),
        ("ownerdraw", Args(1)),
        ("ownerdrawflag", Args(1)),
        ("outofboundsclick", Args(0)),
        ("soundloop", Args(1)),
        ("itemdef", Block),
        ("cinematic", Args(1)),
        ("popup", Args(0)),
        ("fadeclamp", Args(1)),
        ("fadecycle", Args(1)),
        ("fadeamount", Args(1)),
        ("fadeinamount", Args(1)),
        ("execkey", ArgBlock),
        ("execkeyint", ArgBlock),
        ("blurworld", Args(1)),
        ("legacysplitscreenscale", Args(0)),
    ];

    fn find(table: &[(&str, Arity)], key: &str) -> Option<Arity> {
        table.iter().find(|(k, _)| *k == key).map(|&(_, a)| a)
    }

    pub(super) fn item_arity(key: &str) -> Option<Arity> {
        find(&ITEM, key)
    }

    pub(super) fn menu_arity(key: &str) -> Option<Arity> {
        find(&MENU, key)
    }

    /// Steps `*i` past the keyword at it and what it reads; an unknown
    /// keyword (`None`, already reported) past itself alone.
    pub(super) fn skip(tokens: &[String], i: &mut usize, arity: Option<Arity>) {
        *i += 1;
        match arity {
            None => {}
            Some(Args(n)) => *i += n,
            Some(Block) => {
                block(tokens, i);
            }
            Some(ArgBlock) => {
                *i += 1;
                block(tokens, i);
            }
            Some(Rect) => *i += super::rect_align(tokens, *i - 1, &mut [0; 2]) - 1,
            Some(Columns) => {
                let n = super::num(tokens.get(*i)) as usize;
                *i += 1 + n * 3;
            }
        }
    }

    /// CoD2's `ITEM_TYPE_*` onto vcod's: 0..12 are CoD1's, its bind (14)
    /// is CoD1's 13, its file name field (16) an edit field
    /// ([`super::UiItem::file_name_only`]), the rest `100 +` their number.
    pub(super) fn item_type(t: i32) -> i32 {
        match t {
            0..=12 => t,
            14 => super::ITEM_TYPE_BIND,
            16 => super::ITEM_TYPE_EDITFIELD,
            _ => 100 + t,
        }
    }
}

/// A script's commands: the token stream split on `;`. Retail splits by
/// each command's argument count instead, so a missing `;` is fine there;
/// [`script_commands`] handles that case by starting a new command at any
/// known command word.
pub fn script_commands(script: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for t in script {
        if t == ";" {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if !cur.is_empty() && is_command(t) && args_done(&cur) {
            out.push(std::mem::take(&mut cur));
        }
        match cod1_command(t) {
            Some(c) if cur.is_empty() => cur.push(c.to_string()),
            _ => cur.push(t.clone()),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// CoD2's renamed script commands (`commandList`, CoD2MP Mac 1.3
/// 0x34b260) as CoD1's, so one runner serves both.
fn cod1_command(t: &str) -> Option<&'static str> {
    Some(match t.to_ascii_lowercase().as_str() {
        "setdvar" => "setcvar",
        "execondvarintvalue" => "execoncvarintvalue",
        "execondvarfloatvalue" => "execoncvarfloatvalue",
        _ => return None,
    })
}

/// The script command words the stock front-end menus use.
fn is_command(t: &str) -> bool {
    cod1_command(t).is_some()
        || matches!(
            t.to_ascii_lowercase().as_str(),
            "play"
                | "open"
                | "close"
                | "show"
                | "hide"
                | "exec"
                | "setcvar"
                | "uiscript"
                | "ingameclose"
                | "setitemcolor"
                | "setfocus"
                | "fadein"
                | "fadeout"
                | "execoncvarintvalue"
                | "execoncvarfloatvalue"
                | "scriptmenuresponse"
        )
}

/// Whether `cur` has all the arguments its command word takes, so the next
/// command word starts a new command rather than being an argument.
fn args_done(cur: &[String]) -> bool {
    let want = match cur[0].to_ascii_lowercase().as_str() {
        "setcvar" => 2,
        "setitemcolor" => 6,
        "execoncvarintvalue" | "execoncvarfloatvalue" => 3,
        "uiscript" => 1,
        _ => 1,
    };
    cur.len() > want
}

#[cfg(test)]
mod tests {
    use super::*;

    const MENU: &str = r#"
#define FOCUS .98 .96 .39 1
{
  \\ Server Join \\
  menuDef {
    name "main"
    rect 10 20 640 480
    focusColor FOCUS
    onESC { uiScript closeJoin
      close joinserver ; open main }
    #include "bg.menu"
    itemDef {
      name play
      text "@MENU_JOIN_GAME"
      type 1
      rect 385 190 250 15
      textscale .4
      textaligny 14
      forecolor .9 .9 .9 .9
      visible 1
      action { play "mouse_click"; close main ; open joinserver }
      cvarTest "cl_ingame"
      showCVar { "0" }
    }
    itemDef {
      name serverlist
      type 6
      rect 19 145 600 302
      elementheight 15
      feeder 0x02
      columns 2  2 20 20  21 40 40
      cvarFloatList { "@MENU_56K" 4000 }
      doubleClick { uiScript JoinServer }
      visible 1
    }
  }
}
"#;

    fn parse(text: &str) -> Vec<UiMenu> {
        parse_file(text, &|p| {
            (p == "bg.menu").then(|| {
                "itemDef { name main_back_top style 3 rect 0 0 640 320 background \"ui_mp/assets/main_back_top_mp.tga\" visible 1 decoration }".to_string()
            })
        })
    }

    #[test]
    fn parses_layout_scripts_and_includes() {
        let menus = parse(MENU);
        assert_eq!(menus.len(), 1);
        let m = &menus[0];
        assert_eq!(m.name, "main");
        assert_eq!(m.focus_color, [0.98, 0.96, 0.39, 1.0]);
        assert_eq!(m.items.len(), 3);
        let bg = &m.items[0];
        assert_eq!(bg.style, WINDOW_STYLE_SHADER);
        assert!(bg.decoration);
        assert_eq!(bg.rect, [10.0, 20.0, 640.0, 320.0]);
        let play = &m.items[1];
        assert_eq!(play.kind, ITEM_TYPE_BUTTON);
        assert_eq!(play.rect, [395.0, 210.0, 250.0, 15.0]);
        assert_eq!(play.fore, [0.9, 0.9, 0.9, 0.9]);
        assert_eq!(play.text_align_y, 14.0);
        let gate = play.gate.as_ref().unwrap();
        assert_eq!((gate.cvar.as_str(), gate.show), ("cl_ingame", true));
        assert_eq!(
            script_commands(&play.action),
            vec![
                vec!["play", "mouse_click"],
                vec!["close", "main"],
                vec!["open", "joinserver"]
            ]
        );
        let list = &m.items[2];
        assert_eq!(list.feeder, Some(FEEDER_SERVERS));
        assert_eq!(list.columns.len(), 2);
        assert_eq!(list.columns[1].pos, 21.0);
        assert_eq!(list.columns[1].max_chars, 40);
        assert_eq!(list.float_list, vec![("@MENU_56K".to_string(), 4000.0)]);
        assert!(list.visible);
    }

    #[test]
    fn exec_keys_and_focus_scripts_parse() {
        let menus = parse(
            r#"{ menuDef { name m outOfBoundsClick
                execKey "q" { close m }
                execKeyInt 9 { open x }
                execKey "q" { open y }
                itemDef { name a onFocus { play "mouse_over" } leaveFocus { hide b }
                  focusSound "sound/misc/menu2.wav" visible 1 }
            } }"#,
        );
        let m = &menus[0];
        assert!(m.out_of_bounds_click);
        let keys: Vec<(u8, Vec<Vec<String>>)> = (m.exec_keys.iter())
            .map(|(k, s)| (*k, script_commands(s)))
            .collect();
        let cmd = |a: &str, b: &str| vec![vec![a.to_string(), b.to_string()]];
        assert_eq!(keys, vec![(9, cmd("open", "x")), (b'q', cmd("open", "y"))]);
        let a = &m.items[0];
        assert_eq!(script_commands(&a.on_focus), cmd("play", "mouse_over"));
        assert_eq!(script_commands(&a.leave_focus), cmd("hide", "b"));
        assert_eq!(a.focus_sound.as_deref(), Some("sound/misc/menu2.wav"));
        assert!(a.visible);
    }

    #[test]
    fn item_focus_sound_cursor_and_max_chars_goto_next_parse() {
        let file = parse_menu_file(
            r#"{ assetGlobalDef { font "fonts/normalFont" 16 itemFocusSound "mouse_over"
                cursor "ui/assets/3_cursor3" }
                menuDef { name m itemDef { name a maxchars 4 maxCharsGotoNext visible 1 } } }"#,
            &|_| None,
        );
        assert_eq!(file.item_focus_sound.as_deref(), Some("mouse_over"));
        assert_eq!(file.cursor.as_deref(), Some("ui/assets/3_cursor3"));
        let a = &file.menus[0].items[0];
        assert!(a.max_chars_goto_next && a.visible);
        assert_eq!(a.max_chars, 4);
    }

    #[test]
    fn cod2_reads_dvar_keywords_onto_the_cvar_fields() {
        let text = r#"{ menuDef { name m rect 0 0 640 480 4 4 blurWorld 2.5 fadeClamp 1
            soundLoop "music_mainmenu_mp" legacySplitScreenScale
            itemDef { name a type 14 rect 10 20 30 40 2 1 textfont 1 textstyle 3
              dvar "r_gamma" dvarTest "ui_x" showDvar { "1" ; "2" }
              enableDvar { "3" } elementtype 0 elementwidth 5 columns 1 2 190 25
              dvarFloatList { "@MENU_YES" 1 "@MENU_NO" 0 } special 0
              action { setdvar ui_x 1; execOnDvarIntValue ui_x 1 "quit" } visible 1 }
            itemDef { name b type 13 cvar "x" dvarEnumList "r_mode" } } }"#;
        let file = parse_menu_file_in(Game::Cod2, text, &|_| None);
        let m = &file.menus[0];
        assert_eq!(
            (m.rect, m.align, m.blur_world),
            ([0.0, 0.0, 640.0, 480.0], [4, 4], 2.5)
        );
        let a = &m.items[0];
        assert_eq!(a.kind, ITEM_TYPE_BIND);
        assert_eq!((a.rect, a.align), ([10.0, 20.0, 30.0, 40.0], [2, 1]));
        assert_eq!(a.cvar.as_deref(), Some("r_gamma"));
        let gate = a.gate.as_ref().unwrap();
        assert_eq!((gate.cvar.as_str(), gate.show), ("ui_x", true));
        assert_eq!(gate.values, ["1", "2"]);
        assert_eq!(a.float_list.len(), 2);
        assert!(a.visible);
        assert_eq!(
            script_commands(&a.action),
            vec![
                vec!["setcvar", "ui_x", "1"],
                vec!["execoncvarintvalue", "ui_x", "1", "quit"]
            ]
        );
        assert_eq!(m.items[1].kind, ITEM_TYPE_DVARENUM);
        assert_eq!(m.items[1].enum_list.as_deref(), Some("r_mode"));
        // `cvar` is CoD1's; CoD2's table has only `dvar`.
        assert_eq!(file.errors, ["b: unknown menu item keyword cvar"]);
        // CoD1's grammar takes the alignments as stray tokens and checks nothing.
        let cod1 = parse_menu_file(text, &|_| None);
        assert!(cod1.errors.is_empty());
        assert_eq!(cod1.menus[0].align, [0, 0]);
    }

    /// Every stock CoD2 MP menu parses with no keyword its tables lack:
    /// the files `ui_mp/menus.txt`, `ingame.txt` and `hud.txt` load and
    /// every other `ui_mp/` menu (the script menus).
    #[test]
    fn stock_cod2_menus_parse_without_errors() {
        use crate::asset_source::AssetSource;
        let Some(fs) = crate::testing::cod2_fs() else {
            return;
        };
        let read = |p: &str| fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned());
        let mut files: Vec<String> = ["ui_mp/menus.txt", "ui_mp/ingame.txt", "ui_mp/hud.txt"]
            .iter()
            .flat_map(|l| menu_list(&read(l).unwrap_or_else(|| panic!("{l} missing"))))
            .collect();
        let listed = files.len();
        // The rest; `button_mainmenu.menu` and kin are `#include` pieces.
        files.extend(fs.list("ui_mp", ".menu"));
        let (mut menus, mut errors) = (0, Vec::new());
        for (n, path) in files.iter().enumerate() {
            // `menus.txt` names files the iwds lack, as CoD1's does.
            let Some(text) = read(path) else { continue };
            let file = parse_menu_file_in(Game::Cod2, &text, &read);
            assert!(n >= listed || !file.menus.is_empty(), "{path}: no menuDef");
            menus += file.menus.len();
            errors.extend(file.errors.into_iter().map(|e| format!("{path}: {e}")));
        }
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(menus >= 60, "{menus} menus in {} files", files.len());
        let main = parse_menu_file_in(Game::Cod2, &read("ui_mp/main.menu").unwrap(), &read);
        let resume = (main.menus.iter())
            .flat_map(|m| &m.items)
            .find(|i| i.name == "resume" && i.gate.is_some())
            .expect("main.menu's resume item and its cl_ingame gate");
        assert_eq!(resume.gate.as_ref().unwrap().cvar, "cl_ingame");
    }

    #[test]
    fn a_missing_semicolon_still_splits_commands() {
        let m = &parse(MENU)[0];
        assert_eq!(
            script_commands(&m.on_esc),
            vec![
                vec!["uiScript", "closeJoin"],
                vec!["close", "joinserver"],
                vec!["open", "main"]
            ]
        );
    }

    #[test]
    fn stock_front_end_menus_parse() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        let read = |p: &str| fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned());
        let main = parse_file(&read("ui_mp/main.menu").unwrap(), &read);
        let m = main.iter().find(|m| m.name == "main").unwrap();
        // menu_background.menu's two backdrop halves come first.
        assert_eq!(
            m.items[0].background.as_deref(),
            Some("ui_mp/assets/main_back_top_mp.tga")
        );
        let join = m
            .items
            .iter()
            .find(|i| i.text == "@MENU_JOIN_GAME")
            .unwrap();
        assert!(
            script_commands(&join.action)
                .iter()
                .any(|c| c == &["open", "joinserver"])
        );
        let js = parse_file(&read("ui_mp/joinserver.menu").unwrap(), &read);
        let list = js[0]
            .items
            .iter()
            .find(|i| i.feeder == Some(FEEDER_SERVERS))
            .unwrap();
        assert_eq!(list.columns.len(), 6);
        assert_eq!(list.element_height, 15.0);
        assert_eq!(
            script_commands(&list.double_click),
            vec![vec!["uiScript", "JoinServer"]]
        );
        let quit = parse_file(&read("ui/quit.menu").unwrap(), &read);
        let yes = quit[0].items.iter().find(|i| i.name == "yes").unwrap();
        assert!(
            script_commands(&yes.action)
                .iter()
                .any(|c| c == &["uiScript", "quit"])
        );
        // Popups place their items inside their own rect.
        assert_eq!(yes.rect[0], 204.0 + 44.0);
    }

    #[test]
    fn origin_shifts_the_rect_before_it_only() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        let read = |p: &str| fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned());
        let menus = parse_file(&read("ui_mp/createserver.menu").unwrap(), &read);
        let op = menus.iter().find(|m| m.name == "createserver_op").unwrap();
        let at = |text: &str| op.items.iter().find(|i| i.text == text).unwrap().rect;
        // The game type's `origin` comes before its `rect` and is lost; the
        // fields below put it after and stack 15 apart from it.
        assert_eq!(at("@MENU_GAME_TYPE")[..2], [10.0, 158.0]);
        assert_eq!(at("@MENU_SERVER_NAME")[..2], [10.0, 173.0]);
        assert_eq!(at("@MENU_DEDICATED")[..2], [10.0, 188.0]);
        let maps = menus
            .iter()
            .find(|m| m.name == "createserver_maps")
            .unwrap();
        let list = maps
            .items
            .iter()
            .find(|i| i.feeder == Some(FEEDER_ALLMAPS))
            .unwrap();
        assert_eq!(list.rect, [403.0, 273.0, 192.0, 130.0]);
    }

    #[test]
    fn stock_options_controls_parse() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let read = |p: &str| fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned());
        let find = |file: &str, text: &str| {
            parse_file(&read(file).unwrap(), &read)[0]
                .items
                .iter()
                .find(|i| i.text == text)
                .cloned()
                .unwrap()
        };
        let sens = find("ui/options_look.menu", "@MENU_MOUSE_SENSITIVITY");
        assert_eq!(sens.kind, ITEM_TYPE_SLIDER);
        assert_eq!(sens.cvar.as_deref(), Some("sensitivity"));
        assert_eq!(
            sens.slider,
            Some(SliderRange {
                default: 5.0,
                min: 1.0,
                max: 30.0
            })
        );
        // OPTIONS_WINDOW_POS 5 75 plus the item's 5 145.
        assert_eq!(sens.rect, [10.0, 220.0, 350.0, 13.0]);
        let filter = find("ui/options_graphics.menu", "@MENU_TEXTURE_FILTER");
        assert_eq!(filter.str_list.len(), 2);
        assert_eq!(filter.str_list[1].1, "GL_LINEAR_MIPMAP_LINEAR");
        let name = find("ui_mp/options_multi.menu", "@MENU_PLAYER_NAME");
        assert_eq!(name.kind, ITEM_TYPE_EDITFIELD);
        assert_eq!((name.max_chars, name.max_paint_chars), (32, 18));
        let fwd = find("ui/options_move.menu", "@MENU_FORWARD");
        assert_eq!(fwd.kind, ITEM_TYPE_BIND);
        assert_eq!(fwd.cvar.as_deref(), Some("+forward"));
        let perf = parse_file(&read("ui/options_performance.menu").unwrap(), &read);
        assert!(script_commands(&perf[0].on_close).iter().any(|c| c
            == &[
                "execOnCvarIntValue",
                "ui_lod",
                "4",
                "set r_lodscale 4;set r_lodbias -200"
            ]));
    }

    #[test]
    fn menu_list_reads_load_menu_blocks_to_the_closing_brace() {
        let text = r#"// multiplayer menu defs
{
    loadMenu { "ui_mp/main.menu" }
    loadMenu { "ui/quit.menu" "ui/error.menu" }	//TEMP
    something
}
    loadMenu { "ui_mp/after.menu" }
"#;
        assert_eq!(
            menu_list(text),
            ["ui_mp/main.menu", "ui/quit.menu", "ui/error.menu"]
        );
        assert!(menu_list("{ loadMenu \"x.menu\" }").is_empty());
    }

    #[test]
    fn a_menu_is_tried_under_the_language_directory_first() {
        assert_eq!(localized_menu_path("ui_mp/main.menu", "0"), None);
        assert_eq!(localized_menu_path("ui_mp/main.menu", ""), None);
        assert_eq!(
            localized_menu_path("ui_mp/main.menu", "2"),
            Some("ui_mp/german/main.menu".into())
        );
        assert_eq!(
            localized_menu_path("ui_mp/scriptmenus/team_american.menu", " 1x"),
            Some("ui_mp/scriptmenus/french/team_american.menu".into())
        );
        assert_eq!(
            localized_menu_path("main.menu", "99"),
            Some("english/main.menu".into())
        );
    }

    #[test]
    fn game_types_list_the_scripts_without_an_underscore_named_by_their_txt() {
        let mut fs = crate::pk3::Pk3Fs::empty();
        for f in ["dm.gsc", "_teams.gsc", "sd.gsc", "dm.txt", "mine.gsc"] {
            let body: &[u8] = match f {
                "dm.txt" => b"\"DM_DEATHMATCH\"\r\n",
                _ => b"",
            };
            fs.overlay(&format!("maps/MP/gametypes/{f}"), body.to_vec());
        }
        let got: Vec<(String, String)> = game_types(&fs)
            .into_iter()
            .map(|g| (g.script, g.name))
            .collect();
        let want = [("dm", "DM_DEATHMATCH"), ("mine", "mine"), ("sd", "sd")];
        assert_eq!(got, want.map(|(a, b)| (a.to_string(), b.to_string())));
    }

    /// A newer pak's game types come first, as `FS_GetFileList` walks the
    /// search path, not in name order.
    #[test]
    fn game_types_follow_the_search_path() {
        let dir = tempfile::tempdir().unwrap();
        let pak = |name: &str, scripts: &[&str]| {
            let entries: Vec<(String, &[u8])> = scripts
                .iter()
                .map(|s| (format!("maps/mp/gametypes/{s}.gsc"), &b""[..]))
                .collect();
            let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), *b)).collect();
            std::fs::write(dir.path().join(name), crate::testing::pk3_with(&refs)).unwrap();
        };
        pak("pak5.pk3", &["dm", "sd"]);
        pak("zzz_mod.pk3", &["tdm"]);
        let fs = crate::pk3::Pk3Fs::open(dir.path()).unwrap();
        let got: Vec<String> = game_types(&fs).into_iter().map(|g| g.script).collect();
        assert_eq!(got, ["tdm", "dm", "sd"]);
    }

    #[test]
    fn the_stock_game_types_are_the_five_in_pak5() {
        let Some(fs) = crate::testing::stock_fs_11() else {
            return;
        };
        let names: Vec<String> = game_types(&fs).into_iter().map(|g| g.name).collect();
        assert_eq!(
            names,
            [
                "BEL_BEHIND_ENEMY_LINES",
                "DM_DEATHMATCH",
                "RE_RETRIEVAL",
                "SD_SEARCH_AND_DESTROY",
                "TDM_TEAM_DEATHMATCH"
            ]
        );
    }

    #[test]
    fn script_menu_response_is_a_command_word() {
        let script = tokenize(r#"play "mouse_click" scriptMenuResponse "allies" close team_x"#);
        assert_eq!(
            script_commands(&script),
            [
                vec!["play", "mouse_click"],
                vec!["scriptMenuResponse", "allies"],
                vec!["close", "team_x"],
            ]
        );
    }

    /// Every stock script menu parses into one menu of its file's name whose
    /// scripts answer the server, through an item's action or an `execKey`.
    #[test]
    fn every_stock_script_menu_answers_the_server() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let read = |p: &str| fs.read(p).map(|b| String::from_utf8_lossy(&b).into_owned());
        let mut checked = 0;
        for path in fs.list_prefix("ui_mp/scriptmenus/") {
            let Some(name) = path
                .strip_prefix("ui_mp/scriptmenus/")
                .and_then(|n| n.strip_suffix(".menu"))
            else {
                continue;
            };
            let menus = parse_file(&read(&path).unwrap(), &read);
            assert_eq!(menus.len(), 1, "{path}");
            let m = &menus[0];
            assert!(m.name.eq_ignore_ascii_case(name), "{path}: {}", m.name);
            let answers = |s: &[String]| {
                script_commands(s)
                    .iter()
                    .any(|c| c[0].eq_ignore_ascii_case("scriptMenuResponse") && c.len() == 2)
            };
            assert!(
                m.items.iter().any(|i| answers(&i.action))
                    || m.exec_keys.iter().any(|(_, s)| answers(s)),
                "{path}"
            );
            checked += 1;
        }
        assert!(checked >= 15, "{checked}");
    }
}
