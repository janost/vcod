//! The front end's menu files: `ui_mp/*.menu` and `ui/*.menu`, the full
//! `menuDef` / `itemDef` grammar the UI module reads, as far as the main menu
//! the server browser and the options screens use it
//! (docs/research/cod11-front-end.md). Unlike
//! [`crate::menu`], which keeps only what a script menu needs, this keeps the
//! layout: rects, colours, styles, text placement and the item scripts.

use std::collections::HashMap;

use crate::menu::{CvarGate, block, tokenize};

/// `ITEM_TYPE_*` from `menudef.h`.
pub const ITEM_TYPE_TEXT: i32 = 0;
pub const ITEM_TYPE_BUTTON: i32 = 1;
pub const ITEM_TYPE_EDITFIELD: i32 = 4;
pub const ITEM_TYPE_LISTBOX: i32 = 6;
pub const ITEM_TYPE_OWNERDRAW: i32 = 8;
pub const ITEM_TYPE_SLIDER: i32 = 10;
pub const ITEM_TYPE_YESNO: i32 = 11;
pub const ITEM_TYPE_MULTI: i32 = 12;
pub const ITEM_TYPE_BIND: i32 = 13;

/// `WINDOW_STYLE_*`.
pub const WINDOW_STYLE_FILLED: i32 = 1;
pub const WINDOW_STYLE_SHADER: i32 = 3;

/// `ITEM_ALIGN_*`.
pub const ITEM_ALIGN_CENTER: i32 = 1;
pub const ITEM_ALIGN_RIGHT: i32 = 2;

/// `FEEDER_*` from `ui_mp/menudef.h`.
pub const FEEDER_SERVERS: i32 = 2;
pub const FEEDER_SERVERSTATUS: i32 = 13;

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
    /// An edit field's `maxChars` (0: no cap) and `maxPaintChars`.
    pub max_chars: usize,
    pub max_paint_chars: usize,
    /// Scripts as token streams; [`script_commands`] splits one.
    pub action: Vec<String>,
    pub double_click: Vec<String>,
    pub mouse_enter: Vec<String>,
    pub mouse_exit: Vec<String>,
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
            max_chars: 0,
            max_paint_chars: 0,
            action: Vec::new(),
            double_click: Vec::new(),
            mouse_enter: Vec::new(),
            mouse_exit: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UiMenu {
    pub name: String,
    pub rect: Rect,
    pub fullscreen: bool,
    pub popup: bool,
    pub focus_color: [f32; 4],
    pub on_open: Vec<String>,
    pub on_close: Vec<String>,
    pub on_esc: Vec<String>,
    /// Rects already offset by the menu's origin.
    pub items: Vec<UiItem>,
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
            let body = rest.split("//").next().unwrap_or("");
            let mut words = body.split_whitespace();
            if let Some(name) = words.next() {
                defines.insert(name.to_string(), words.map(str::to_string).collect());
            }
        } else if !trimmed.starts_with('#') {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Every `menuDef` in `text`. `include` reads an `#include`d file.
pub fn parse_file(text: &str, include: &dyn Fn(&str) -> Option<String>) -> Vec<UiMenu> {
    let mut defines = HashMap::new();
    let text = preprocess(text, include, &mut defines, 0);
    let tokens: Vec<String> = tokenize(&text)
        .into_iter()
        .flat_map(|t| match defines.get(&t) {
            Some(v) => v.clone(),
            None => vec![t],
        })
        .collect();
    let mut menus = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].eq_ignore_ascii_case("menuDef") {
            i += 1;
            menus.push(parse_menu(block(&tokens, &mut i)));
        } else {
            i += 1;
        }
    }
    menus
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

fn parse_menu(body: &[String]) -> UiMenu {
    let mut menu = UiMenu {
        focus_color: [1.0; 4],
        ..UiMenu::default()
    };
    let mut i = 0;
    while i < body.len() {
        let key = body[i].to_ascii_lowercase();
        match key.as_str() {
            "name" => {
                menu.name = body.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "rect" => {
                menu.rect = nums::<4>(body, i);
                i += 5;
            }
            "fullscreen" => {
                menu.fullscreen = num(body.get(i + 1)) != 0.0;
                i += 2;
            }
            "popup" => {
                menu.popup = true;
                i += 1;
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
                let mut item = parse_item(block(body, &mut i));
                item.rect[0] += menu.rect[0];
                item.rect[1] += menu.rect[1];
                menu.items.push(item);
            }
            // Any other block (`onFocus`, `execKey`) is skipped whole.
            "{" => {
                block(body, &mut i);
            }
            _ => i += 1,
        }
    }
    menu
}

fn parse_item(tokens: &[String]) -> UiItem {
    let mut item = UiItem::default();
    let mut cvar_test: Option<String> = None;
    let mut show: Option<(bool, Vec<String>)> = None;
    let mut i = 0;
    let s = |i: usize| tokens.get(i + 1).cloned().unwrap_or_default();
    while i < tokens.len() {
        let key = tokens[i].to_ascii_lowercase();
        let mut skip = 2;
        match key.as_str() {
            "name" => item.name = s(i),
            "group" => item.group = s(i),
            "type" => item.kind = num(tokens.get(i + 1)) as i32,
            "style" => item.style = num(tokens.get(i + 1)) as i32,
            "rect" => {
                item.rect = nums::<4>(tokens, i);
                skip = 5;
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
            "cvar" => item.cvar = Some(s(i)),
            "cvartest" => cvar_test = Some(s(i)),
            "showcvar" | "hidecvar" => {
                let shown = key == "showcvar";
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
            "action" | "doubleclick" | "mouseenter" | "mouseexit" => {
                i += 1;
                let script = block(tokens, &mut i).to_vec();
                match key.as_str() {
                    "action" => item.action = script,
                    "doubleclick" => item.double_click = script,
                    "mouseenter" => item.mouse_enter = script,
                    _ => item.mouse_exit = script,
                }
                continue;
            }
            "cvarfloat" => {
                item.cvar = Some(s(i));
                let [default, min, max] = std::array::from_fn(|k| num(tokens.get(i + 2 + k)));
                item.slider = Some(SliderRange { default, min, max });
                skip = 5;
            }
            "maxchars" => item.max_chars = num(tokens.get(i + 1)) as usize,
            "maxpaintchars" => item.max_paint_chars = num(tokens.get(i + 1)) as usize,
            "cvarstrlist" => {
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
            "cvarfloatlist" => {
                i += 1;
                item.float_list = block(tokens, &mut i)
                    .chunks(2)
                    .map(|p| (p[0].clone(), num(p.get(1))))
                    .collect();
                continue;
            }
            "{" => {
                // An unhandled key's block (`onFocus`).
                block(tokens, &mut i);
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
        cur.push(t.clone());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The script command words the stock front-end menus use.
fn is_command(t: &str) -> bool {
    matches!(
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
        let Some(fs) = crate::testing::game_fs() else {
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
}
