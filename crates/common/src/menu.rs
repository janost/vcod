//! Stock script menus: `ui_mp/scriptmenus/*.menu`, Q3's `menuDef` grammar as
//! the shipped files use it. Only the keys a client needs to drive the team
//! and weapon pickers are recognised; layout, style and `onFocus` sound cues
//! are ignored.

#[derive(Debug, Clone)]
pub struct CvarGate {
    pub cvar: String,
    /// `true` for `showCvar` (visible when the value is listed), `false` for
    /// `hideCvar` (visible when it is not).
    pub show: bool,
    pub values: Vec<String>,
}

impl CvarGate {
    /// A cvar the client has never heard of reads as the empty string, same
    /// as retail's `Cvar_VariableString` on an unknown name.
    pub fn passes(&self, value: Option<&str>) -> bool {
        let v = value.unwrap_or("");
        self.values.iter().any(|x| x == v) == self.show
    }
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub name: String,
    pub text: String,
    pub visible: bool,
    pub response: Option<String>,
    /// The menu an `open` in the action names: the team and weapon menus'
    /// "Main Menu" tab is `close <self>; open main` with no response.
    pub opens: Option<String>,
    pub gate: Option<CvarGate>,
}

impl MenuItem {
    /// The tab that leaves the script menu for the UI's `main` menu.
    pub fn opens_main(&self) -> bool {
        self.response.is_none()
            && self
                .opens
                .as_deref()
                .is_some_and(|m| m.eq_ignore_ascii_case("main"))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Menu {
    pub name: String,
    pub background: Option<String>,
    pub items: Vec<MenuItem>,
    /// Key text (`"1"`) to the `scriptMenuResponse` it sends.
    pub exec_keys: Vec<(String, String)>,
}

impl Menu {
    /// Items a client can actually pick: visible, a response other than
    /// `open`/`close` or the "Main Menu" tab, and whose cvar gate passes.
    pub fn choices(&self, cvar: impl Fn(&str) -> Option<String>) -> Vec<&MenuItem> {
        self.items
            .iter()
            .filter(|i| i.visible)
            .filter(|i| {
                i.opens_main()
                    || !matches!(i.response.as_deref(), None | Some("open") | Some("close"))
            })
            .filter(|i| {
                i.gate
                    .as_ref()
                    .is_none_or(|g| g.passes(cvar(&g.cvar).as_deref()))
            })
            .collect()
    }
}

/// `//` and `/* */` comments dropped, `#`-lines skipped, quoted strings kept
/// as one token, `{`, `}` and `;` as their own tokens.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Byte tests only, all ASCII, so every slice boundary is a char boundary.
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'#' || (c == b'/' && bytes.get(i + 1) == Some(&b'/')) {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else if c == b'"' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            tokens.push(text[start..i].to_string());
            i += 1;
        } else if c == b'{' || c == b'}' || c == b';' {
            tokens.push((c as char).to_string());
            i += 1;
        } else {
            let start = i;
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_ascii_whitespace() || matches!(c, b'{' | b'}' | b';' | b'"') {
                    break;
                }
                i += 1;
            }
            tokens.push(text[start..i].to_string());
        }
    }
    tokens
}

/// The tokens strictly between the `{` at `tokens[*i]` and its matching `}`,
/// advancing `*i` past the closing brace. Empty and `*i` unchanged if
/// `tokens[*i]` is not `{`.
pub(crate) fn block<'a>(tokens: &'a [String], i: &mut usize) -> &'a [String] {
    if tokens.get(*i).map(String::as_str) != Some("{") {
        return &[];
    }
    let start = *i + 1;
    let mut depth = 1;
    let mut j = start;
    while j < tokens.len() && depth > 0 {
        match tokens[j].as_str() {
            "{" => depth += 1,
            "}" => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    // Unbalanced input (a brace with no match) takes everything left rather
    // than panicking: a menu cut off mid-block still parses what it has.
    let end = if depth == 0 { j - 1 } else { j };
    *i = j;
    &tokens[start..end]
}

/// The `scriptMenuResponse` argument closest to the front of an action-like
/// block.
fn first_response(block: &[String]) -> Option<String> {
    block
        .iter()
        .position(|t| t == "scriptMenuResponse")
        .and_then(|p| block.get(p + 1))
        .cloned()
}

/// Parses one `itemDef` block, returning the item and its `background` value
/// (only meaningful for the `window_background` item, which the caller
/// promotes to `Menu::background`).
fn parse_item(tokens: &[String]) -> (MenuItem, Option<String>) {
    let mut item = MenuItem {
        name: String::new(),
        text: String::new(),
        visible: false,
        response: None,
        opens: None,
        gate: None,
    };
    let mut background = None;
    let mut cvar: Option<String> = None;
    let mut show_values: Option<(bool, Vec<String>)> = None;
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "name" => {
                item.name = tokens.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "text" => {
                item.text = tokens.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "visible" => {
                item.visible = tokens.get(i + 1).map(String::as_str) == Some("1");
                i += 2;
            }
            "background" => {
                background = tokens.get(i + 1).cloned();
                i += 2;
            }
            "cvartest" => {
                cvar = tokens.get(i + 1).cloned();
                i += 2;
            }
            "showCvar" | "hideCvar" => {
                let show = tokens[i] == "showCvar";
                i += 1;
                let values = block(tokens, &mut i).to_vec();
                show_values = Some((show, values));
            }
            "action" => {
                i += 1;
                let inner = block(tokens, &mut i);
                item.response = first_response(inner);
                item.opens = inner
                    .windows(2)
                    .enumerate()
                    .find(|(p, w)| {
                        w[0].eq_ignore_ascii_case("open")
                            && (*p == 0 || inner[p - 1] != "scriptMenuResponse")
                    })
                    .map(|(_, w)| w[1].clone());
            }
            _ => i += 1,
        }
    }
    if let (Some(cvar), Some((show, values))) = (cvar, show_values) {
        item.gate = Some(CvarGate { cvar, show, values });
    }
    (item, background)
}

/// [`parse_with`] with no `#include`s resolved.
pub fn parse(text: &str) -> Menu {
    parse_with(text, &|_| Some(String::new()))
}

/// One script menu, after the preprocessor: `include` reads an `#include`d
/// file, so a mod's `visible MENU_TRUE` or `#define`d response resolves as
/// retail's precompiler resolves it.
pub fn parse_with(text: &str, include: &dyn Fn(&str) -> Option<String>) -> Menu {
    let tokens = crate::ui_menu::expand(text, include);
    let mut menu = Menu::default();
    let Some(def_pos) = tokens.iter().position(|t| t == "menuDef") else {
        return menu;
    };
    let mut i = def_pos + 1;
    let body = block(&tokens, &mut i).to_vec();

    let mut i = 0;
    while i < body.len() {
        match body[i].as_str() {
            "name" => {
                menu.name = body.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "execKey" => {
                let key = body.get(i + 1).cloned().unwrap_or_default();
                i += 2;
                let inner = block(&body, &mut i);
                if let Some(response) = first_response(inner) {
                    menu.exec_keys.push((key, response));
                }
            }
            "itemDef" => {
                i += 1;
                let inner = block(&body, &mut i);
                let (item, background) = parse_item(inner);
                if item.name == "window_background" {
                    menu.background = background;
                }
                menu.items.push(item);
            }
            "onOpen" | "onClose" | "onEsc" | "onFocus" | "onTimer" => {
                i += 1;
                block(&body, &mut i);
            }
            _ => i += 1,
        }
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: &str = r#"
#include "ui_mp/menudef.h"
#define ORIGIN_MENUWINDOW 96 96
{
  menuDef
  {
    name "team_americangerman"
    onOpen { scriptMenuResponse "open"; }
    onEsc { scriptMenuResponse "close"; close team_americangerman; }
    itemDef { name "window_background" visible 1 style WINDOW_STYLE_SHADER background "ui_mp/assets/hud@window_background.tga" decoration }
    itemDef { name "button_weapon" visible 1 text "@MPMENU_WEAPON" cvartest "scr_showweapontab" showCvar { "1" }
      action { play "mouse_click"; scriptMenuResponse "weapon"; } }
    itemDef { name "button_american" visible 1 text "@MPMENU_1_AMERICAN" // trailing comment
      action { play "mouse_click"; scriptMenuResponse "allies"; close team_americangerman; } }
    execKey "1" { play "mouse_click"; scriptMenuResponse "allies"; close team_americangerman }
    itemDef { name "info" visible 0 text "hidden" }
  }
}
"#;

    #[test]
    fn parses_items_responses_and_exec_keys() {
        let m = parse(TEAM);
        assert_eq!(m.name, "team_americangerman");
        assert_eq!(
            m.background.as_deref(),
            Some("ui_mp/assets/hud@window_background.tga")
        );
        assert_eq!(m.exec_keys, vec![("1".to_string(), "allies".to_string())]);
        let american = m
            .items
            .iter()
            .find(|i| i.name == "button_american")
            .unwrap();
        assert_eq!(american.text, "@MPMENU_1_AMERICAN");
        assert_eq!(american.response.as_deref(), Some("allies"));
        assert!(american.visible);
    }

    #[test]
    fn open_and_close_never_become_choices() {
        let m = parse(TEAM);
        let responses: Vec<_> = m
            .choices(|_| None)
            .iter()
            .map(|i| i.response.clone().unwrap())
            .collect();
        // Weapon tab hidden: scr_showweapontab unknown reads as "", not "1".
        assert_eq!(responses, vec!["allies"]);
    }

    #[test]
    fn the_main_menu_tab_is_a_choice() {
        let m = parse(
            r#"{ menuDef { name "weapon_x"
      itemDef { name "button_mainmenu" visible 1 text "@MPMENU_MAIN_MENU" action { play "mouse_click"; close weapon_x; open main; } }
      itemDef { name "b" visible 1 text "Garand" action { scriptMenuResponse "m1garand_mp"; } }
      itemDef { name "c" visible 1 text "Map" action { open viewmap; } }
    } }"#,
        );
        let c = m.choices(|_| None);
        assert_eq!(c.len(), 2);
        assert!(c[0].opens_main());
        assert!(!c[1].opens_main());
    }

    #[test]
    fn show_cvar_reveals_the_weapon_tab() {
        let m = parse(TEAM);
        let n = m
            .choices(|c| (c == "scr_showweapontab").then(|| "1".to_string()))
            .len();
        assert_eq!(n, 2);
    }

    #[test]
    fn cvar_gate_hides_disallowed_weapon() {
        let gate = CvarGate {
            cvar: "scr_allow_m1carbine".into(),
            show: false,
            values: vec!["0".into()],
        };
        assert!(!gate.passes(Some("0")));
        assert!(gate.passes(Some("1")));
        assert!(gate.passes(None));
    }

    #[test]
    fn defines_and_includes_expand_before_parsing() {
        // A mod menu built the way the stock UI menus are, with `menudef.h`
        // constants and its own macros.
        let text = r#"
#include "ui_mp/menudef.h"
#define RIFLE_RESPONSE "kar98k_mp"
{ menuDef { name "mod_weapons"
    itemDef { name "a" visible MENU_TRUE text "Kar98k" action { scriptMenuResponse RIFLE_RESPONSE; } }
    execKey "1" { scriptMenuResponse RIFLE_RESPONSE; }
} }"#;
        let include =
            |p: &str| (p == "ui_mp/menudef.h").then(|| "#define MENU_TRUE 1\n".to_string());
        let m = parse_with(text, &include);
        assert_eq!(m.items.len(), 1);
        assert!(m.items[0].visible);
        assert_eq!(m.items[0].response.as_deref(), Some("kar98k_mp"));
        assert_eq!(
            m.exec_keys,
            vec![("1".to_string(), "kar98k_mp".to_string())]
        );
        // Without the header `MENU_TRUE` stays a word, which reads as hidden.
        assert!(!parse(text).items[0].visible);
    }

    #[test]
    fn a_menu_cut_off_mid_block_parses_what_it_has() {
        // A `{` with no matching `}` must not panic, whatever depth it's at.
        parse("menuDef {");
        parse("{ menuDef { itemDef {");
        let m = parse("{ menuDef { name \"x\"");
        assert_eq!(m.name, "x");
    }

    #[test]
    fn non_ascii_text_tokenizes_without_panicking() {
        // 0x85 and 0xA0 are UTF-8 continuation bytes that read as whitespace as a char.
        assert_eq!(parse("{ menuDef { name à } }").name, "à");
        parse("{ menuDef { itemDef { text Å } } }");
        parse("{ menuDef { name\u{a0}\"x\" } }");
    }

    #[test]
    fn every_stock_team_and_weapon_menu_offers_choices() {
        let Some(fs) = crate::testing::game_fs() else {
            return;
        };
        let names = fs.list_prefix("ui_mp/scriptmenus/");
        let mut checked = 0;
        for path in names.iter().filter(|p| p.ends_with(".menu")) {
            let stem = path
                .trim_start_matches("ui_mp/scriptmenus/")
                .trim_end_matches(".menu");
            if !(stem.starts_with("team_") || stem.starts_with("weapon_")) {
                continue;
            }
            let text = String::from_utf8_lossy(&fs.read(path).unwrap()).into_owned();
            let m = parse(&text);
            assert_eq!(m.name, stem);
            let all_on = |_: &str| Some("1".to_string());
            assert!(m.choices(all_on).len() >= 3, "{stem}: {:?}", m.items);
            checked += 1;
        }
        assert!(checked >= 11, "only {checked} stock menus");
        let w = parse(&String::from_utf8_lossy(
            &fs.read("ui_mp/scriptmenus/weapon_american.menu").unwrap(),
        ));
        let carbine = w
            .items
            .iter()
            .find(|i| i.response.as_deref() == Some("m1carbine_mp"))
            .unwrap();
        assert_eq!(carbine.gate.as_ref().unwrap().cvar, "scr_allow_m1carbine");
        assert!(w.choices(|_| None).iter().any(|i| i.opens_main()));
    }
}
