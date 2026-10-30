//! The server's script menus as the zones' `menuDef_t`s give them
//! (`assets::ui::Menu`): what each answers (each item's
//! `scriptMenuResponse`, the menu's `onEsc` one, the menus its `onOpen`
//! opens in its place) and, in [`Layout`], what 1.7's UI draws and runs:
//! every item's rect, colours, material, text, expressions and scripts
//! (docs/research/cod4-hud.md 12). The scripts are read into [`Cmd`]s;
//! the expressions are [`crate::menuexp`]'s.

use crate::assets::Assets;
use crate::assets::ui::{ItemExps, Menu, Statement, TypeData};
use crate::map::stub_name;
use crate::menuexp::Value;

/// A menu's responses and redirects, and its layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptMenu {
    pub name: String,
    /// Each item's `scriptMenuResponse`, in item order, each once.
    pub responses: Vec<String>,
    /// `onEsc`'s `scriptMenuResponse`, if it sends one.
    pub esc: Option<String>,
    /// What `onOpen` opens: `(Some((dvar, value)), menu)` for an
    /// `openMenuOnDvar` that opens `menu` when `dvar` reads `value`,
    /// `(None, menu)` for a plain `open`.
    pub opens: Vec<(Option<(String, String)>, String)>,
    /// The menu as drawn; `None` for one known only by its responses.
    pub layout: Option<Layout>,
}

/// `menuDef_t`'s drawn and run parts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub rect: [f32; 4],
    /// `horzAlign`, `vertAlign`.
    pub align: [i32; 2],
    /// `fullScreen` (+160).
    pub fullscreen: bool,
    /// The menu window's `style` (+56) and `background` material: the
    /// `connect` menu's levelshot.
    pub style: i32,
    pub background: Option<String>,
    /// `focusColor` (+232), `disableColor` (+248).
    pub focus_color: [f32; 4],
    pub disable_color: [f32; 4],
    pub on_open: Vec<Cmd>,
    pub on_close: Vec<Cmd>,
    pub on_esc: Vec<Cmd>,
    /// `execKey` handlers by key code.
    pub on_key: Vec<(i32, Vec<Cmd>)>,
    /// `visibleExp` (+212).
    pub visible: Statement,
    /// `soundName` (+224), played as the menu opens.
    pub sound: Option<String>,
    pub items: Vec<LayoutItem>,
}

/// `itemDef_s`'s drawn and run parts (offsets into the struct).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutItem {
    pub name: String,
    /// `type` (+180): 0 text, 1 button, 8 owner draw, ..
    pub kind: i32,
    /// `window.rect` (+4): x, y, w, h on the 640x480 grid, then the
    /// alignments (+20, +24).
    pub rect: [f32; 4],
    pub align: [i32; 2],
    /// `style` (+56): 1 filled, 3 shader, 5 the dvar's material, 6 load
    /// bar.
    pub style: i32,
    /// `border` (+60) and `borderSize` (+72).
    pub border: i32,
    pub border_size: f32,
    /// `staticFlags` (+76) and the initial `dynamicFlags` (+80).
    pub static_flags: u32,
    pub dynamic_flags: u32,
    /// `foreColor` (+88), `backColor` (+104), `borderColor` (+120),
    /// `outlineColor` (+136).
    pub fore_color: [f32; 4],
    pub back_color: [f32; 4],
    pub border_color: [f32; 4],
    pub outline_color: [f32; 4],
    /// `background`'s material name.
    pub background: Option<String>,
    pub text: Option<String>,
    pub dvar: Option<String>,
    /// `fontEnum` (+192), `textAlignMode` (+196), `textalignx`, `y`
    /// (+200), `textscale` (+208), `textStyle` (+212).
    pub font: i32,
    pub text_align: i32,
    pub text_offset: [f32; 2],
    pub text_scale: f32,
    pub text_style: i32,
    pub action: Vec<Cmd>,
    pub on_focus: Vec<Cmd>,
    pub leave_focus: Vec<Cmd>,
    pub mouse_enter: Vec<Cmd>,
    pub mouse_exit: Vec<Cmd>,
    /// An edit field's `onAccept`, run on Enter.
    pub on_accept: Vec<Cmd>,
    pub on_key: Vec<(i32, Vec<Cmd>)>,
    /// `dvarFlags` (+284) with `enableDvar` and `dvarTest`.
    pub dvar_flags: i32,
    pub enable_dvar: Option<String>,
    pub dvar_test: Option<String>,
    pub exps: ItemExps,
    /// `ownerDraw` (+64) and a list box's `feeder` (`special`, +292).
    pub owner_draw: i32,
    pub feeder: f32,
    pub type_data: TypeData,
}

/// The item kinds (`type`, +180) the menus' controls use.
pub const ITEM_TYPE_LISTBOX: i32 = 6;
pub const ITEM_TYPE_OWNERDRAW: i32 = 8;
pub const ITEM_TYPE_EDITFIELD: i32 = 4;
pub const ITEM_TYPE_SLIDER: i32 = 10;
pub const ITEM_TYPE_YESNO: i32 = 11;
pub const ITEM_TYPE_MULTI: i32 = 12;
pub const ITEM_TYPE_DVARENUM: i32 = 13;
pub const ITEM_TYPE_BIND: i32 = 14;
/// The edit field's kin: digits only, a file name, a decimal, upper case
/// (`Item_TextField_HandleKey`).
pub const ITEM_TYPE_NUMERICFIELD: i32 = 9;
pub const ITEM_TYPE_VALIDFILEFIELD: i32 = 16;
pub const ITEM_TYPE_DECIMALFIELD: i32 = 17;
pub const ITEM_TYPE_UPREDITFIELD: i32 = 18;

/// `Item_IsTextField`: the kinds that take typed text and
/// paint through `Item_TextField_Paint`.
pub fn is_text_field(kind: i32) -> bool {
    matches!(
        kind,
        ITEM_TYPE_EDITFIELD
            | ITEM_TYPE_NUMERICFIELD
            | ITEM_TYPE_VALIDFILEFIELD
            | ITEM_TYPE_DECIMALFIELD
            | ITEM_TYPE_UPREDITFIELD
    )
}
/// The owner draw of the controls pages' "press a key" line.
pub const UI_KEYBINDSTATUS: i32 = 250;

/// `WINDOW_VISIBLE` in `dynamicFlags`.
pub const WINDOW_VISIBLE: u32 = 0x4;
/// `dynamicFlags` 0x10000: the shader styles draw in `foreColor`, else
/// white (`Window_Paint`).
pub const WINDOW_FORECOLORSET: u32 = 0x10000;
/// `staticFlags` 0x100000, `decoration`: never takes the focus
/// (`Item_SetFocus`).
pub const WINDOW_DECORATION: u32 = 0x100000;

/// One command of a menu script, as 1.7's `commandList` (`iw3mp.exe`
/// 0x6b4940, 42 words) names them.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Play(String),
    /// `scriptMenuResponse`: `mr` with the open menu's slot.
    Response(String),
    /// `close self` or `close <menu>`.
    Close(String),
    Open(String),
    SetFocus(String),
    SetDvar(String, String),
    /// `exec` and `execNow`: a console line.
    Exec(String),
    /// `setLocalVarBool`, `Int`, `Float`, `String`.
    SetLocal(String, Value),
    /// `show` and `hide`: an item of the menu by name, its
    /// `WINDOW_VISIBLE` set or cleared.
    Show(String),
    Hide(String),
    /// A `uiScript` other than `openMenuOnDvar`: its name, and for
    /// `update` the dvar it names.
    UiScript(String, Vec<String>),
    /// `uiScript openMenuOnDvar dvar value menu` (`Not`: when it differs).
    OpenMenuOnDvar {
        dvar: String,
        value: String,
        menu: String,
        not: bool,
    },
    /// `execOnDvarStringValue`, `IntValue`, `FloatValue` and their
    /// `execNow` twins: `line` runs when `dvar` reads `value` as `kind`
    /// compares them (`Script_ConditionalExecHandler`).
    ExecOnDvar {
        dvar: String,
        value: String,
        line: String,
        kind: DvarCompare,
    },
    /// A command vcod does not run, by its word.
    Other(String),
}

/// How `execOnDvar*Value` compares (`Script_ExecIfStringsEqual`,
/// `IntsEqual`, `FloatsEqual`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DvarCompare {
    /// Equal ignoring case.
    String,
    /// `atoi` of both equal.
    Int,
    /// `atof` of both within 1e-5.
    Float,
}

impl DvarCompare {
    /// Whether the dvar's `value` passes against `test`.
    pub fn passes(self, value: &str, test: &str) -> bool {
        use crate::menuexp::{atof, atoi};
        match self {
            DvarCompare::String => value.eq_ignore_ascii_case(test),
            DvarCompare::Int => atoi(value) == atoi(test),
            DvarCompare::Float => (atof(value) - atof(test)).abs() < 1e-5,
        }
    }
}

impl Cmd {
    /// A script's commands (`Item_RunScript`): words in quotes or bare,
    /// `;` between them; a word outside the command list goes to
    /// `UI_RunMenuScript`, which reads the next word as its name.
    pub fn parse(script: &str) -> Vec<Cmd> {
        let t = tokens(script);
        let mut out = Vec::new();
        let mut i = 0;
        let arg = |i: &mut usize| {
            let a = t.get(*i).cloned().unwrap_or_default();
            *i += 1;
            a
        };
        while i < t.len() {
            let word = arg(&mut i);
            let cmd = match word.to_ascii_lowercase().as_str() {
                "play" => Cmd::Play(arg(&mut i)),
                "scriptmenuresponse" => Cmd::Response(arg(&mut i)),
                "close" => Cmd::Close(arg(&mut i)),
                "open" => Cmd::Open(arg(&mut i)),
                "setfocus" => Cmd::SetFocus(arg(&mut i)),
                "show" => Cmd::Show(arg(&mut i)),
                "hide" => Cmd::Hide(arg(&mut i)),
                "setdvar" => {
                    let d = arg(&mut i);
                    Cmd::SetDvar(d, arg(&mut i))
                }
                "exec" | "execnow" => Cmd::Exec(arg(&mut i)),
                "setlocalvarbool" | "setlocalvarint" | "setlocalvarfloat" | "setlocalvarstring" => {
                    let name = arg(&mut i);
                    let v = arg(&mut i);
                    let v = match word.to_ascii_lowercase().as_str() {
                        "setlocalvarbool" => Value::Int((crate::menuexp::atoi(&v) != 0) as i32),
                        "setlocalvarint" => Value::Int(crate::menuexp::atoi(&v)),
                        "setlocalvarfloat" => Value::Float(crate::menuexp::atof(&v) as f32),
                        _ => Value::Str(v),
                    };
                    Cmd::SetLocal(name, v)
                }
                w if let Some(kind) = w
                    .strip_prefix("execnowondvar")
                    .or_else(|| w.strip_prefix("execondvar"))
                    .and_then(|k| match k {
                        "stringvalue" => Some(DvarCompare::String),
                        "intvalue" => Some(DvarCompare::Int),
                        "floatvalue" => Some(DvarCompare::Float),
                        _ => None,
                    }) =>
                {
                    let dvar = arg(&mut i);
                    let value = arg(&mut i);
                    Cmd::ExecOnDvar {
                        dvar,
                        value,
                        line: arg(&mut i),
                        kind,
                    }
                }
                w if let Some(&(_, args)) = COMMANDS.iter().find(|(c, _)| *c == w) => {
                    i = args.end(&t, i);
                    Cmd::Other(word)
                }
                _ => {
                    let name = arg(&mut i);
                    if name.eq_ignore_ascii_case("openMenuOnDvar")
                        || name.eq_ignore_ascii_case("openMenuOnDvarNot")
                    {
                        Cmd::OpenMenuOnDvar {
                            not: name.eq_ignore_ascii_case("openMenuOnDvarNot"),
                            dvar: arg(&mut i),
                            value: arg(&mut i),
                            menu: arg(&mut i),
                        }
                    } else if let Some(&(_, n)) = UI_SCRIPT_ARGS
                        .iter()
                        .find(|(s, _)| name.eq_ignore_ascii_case(s))
                    {
                        let args = (0..n).map(|_| arg(&mut i)).collect();
                        Cmd::UiScript(name, args)
                    } else {
                        Cmd::UiScript(name, Vec::new())
                    }
                }
            };
            out.push(cmd);
        }
        out
    }
}

/// The `uiScript`s that read words after their name, and how many:
/// `UI_RunMenuScript` (iw3mp.exe 0x545bc0) reads one for `update`
/// (0x546b2c), an int for `ServerSort` and `setPbClStatus` (0x5467aa,
/// 0x546b70) and three for the four menu-on-dvar scripts (0x5459e0); any
/// other reads none (`tools/re/cod4_menu_commands.py`, cod4-front-end.md 2).
const UI_SCRIPT_ARGS: [(&str, usize); 5] = [
    ("update", 1),
    ("closeMenuOnDvar", 3),
    ("closeMenuOnDvarNot", 3),
    ("ServerSort", 1),
    ("setPbClStatus", 1),
];

/// What a command vcod skips reads after its word.
#[derive(Clone, Copy)]
enum Args {
    Words(usize),
    /// A `(` ... `)` list when a `(` follows, else this many words: the
    /// stat commands' readers take their values in parentheses or bare.
    Listed(usize),
}

impl Args {
    /// The index past the arguments that start at `at` in `t`.
    fn end(self, t: &[String], at: usize) -> usize {
        match self {
            Args::Listed(_) if t.get(at).is_some_and(|w| w == "(") => {
                let mut depth = 0usize;
                for (i, w) in t.iter().enumerate().skip(at) {
                    match w.as_str() {
                        "(" => depth += 1,
                        ")" if depth == 1 => return i + 1,
                        ")" => depth -= 1,
                        _ => {}
                    }
                }
                t.len()
            }
            Args::Words(n) | Args::Listed(n) => at + n,
        }
    }
}

/// The rest of `commandList` (iw3mp.exe 0x6b4940, 42 rows), lowercased,
/// with what each handler reads (`tools/re/cod4_menu_commands.py`,
/// cod4-front-end.md 2): commands vcod skips. `setcolor` and
/// `setitemcolor` read up to four colour words after one and two.
const COMMANDS: [(&str, Args); 22] = [
    ("fadein", Args::Words(1)),
    ("fadeout", Args::Words(1)),
    ("showmenu", Args::Words(1)),
    ("hidemenu", Args::Words(1)),
    ("setcolor", Args::Words(5)),
    ("ingameopen", Args::Words(1)),
    ("ingameclose", Args::Words(1)),
    ("setbackground", Args::Words(1)),
    ("setitemcolor", Args::Words(6)),
    ("focusfirst", Args::Words(0)),
    ("setfocusbydvar", Args::Words(1)),
    ("scriptmenurespondondvarstringvalue", Args::Words(3)),
    ("scriptmenurespondondvarintvalue", Args::Words(3)),
    ("scriptmenurespondondvarfloatvalue", Args::Words(3)),
    ("feedertop", Args::Words(0)),
    ("feederbottom", Args::Words(0)),
    ("openforgametype", Args::Words(1)),
    ("closeforgametype", Args::Words(1)),
    ("statclearperknew", Args::Listed(1)),
    ("statsetusingtable", Args::Listed(2)),
    ("statclearbitmask", Args::Listed(2)),
    ("getautoupdate", Args::Words(0)),
];

impl ScriptMenu {
    /// The menu `m` out of the zone `a` it was walked from.
    pub fn of(m: &Menu, a: &Assets) -> ScriptMenu {
        let mut out = ScriptMenu {
            name: m.window.name.clone().unwrap_or_default(),
            layout: Some(Layout::of(m, a)),
            ..ScriptMenu::default()
        };
        for it in m.items.iter().flatten() {
            if let Some(r) = it.action.as_deref().and_then(response)
                && !out.responses.contains(&r)
            {
                out.responses.push(r);
            }
        }
        out.esc = m.on_esc.as_deref().and_then(response);
        if let Some(open) = &m.on_open {
            let t = tokens(open);
            let mut i = 0;
            while i < t.len() {
                let rest = &t[i..];
                if rest.len() >= 5
                    && rest[0].eq_ignore_ascii_case("uiScript")
                    && rest[1].eq_ignore_ascii_case("openMenuOnDvar")
                {
                    out.opens
                        .push((Some((rest[2].clone(), rest[3].clone())), rest[4].clone()));
                    i += 5;
                } else if rest.len() >= 2 && rest[0].eq_ignore_ascii_case("open") {
                    out.opens.push((None, rest[1].clone()));
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
        out
    }

    /// The menu that shows when this one opens: the first redirect whose
    /// dvar `dvar` reads as it asks, or this one.
    pub fn shown(&self, dvar: impl Fn(&str) -> Option<String>) -> &str {
        self.opens
            .iter()
            .find(|(when, _)| match when {
                Some((d, v)) => dvar(d).is_some_and(|x| x == *v),
                None => true,
            })
            .map_or(self.name.as_str(), |(_, m)| m.as_str())
    }
}

impl Layout {
    fn of(m: &Menu, a: &Assets) -> Layout {
        let r = Raw(&m.raw);
        let cmds = |s: &Option<String>| s.as_deref().map(Cmd::parse).unwrap_or_default();
        let keys = |k: &[(i32, String)]| k.iter().map(|(k, s)| (*k, Cmd::parse(s))).collect();
        let material = |id: Option<crate::assets::Id<crate::assets::material::Material>>| {
            id.map(|m| stub_name(&a.materials[m.index()].name).to_string())
        };
        Layout {
            rect: r.rect(4),
            align: [r.i(20), r.i(24)],
            fullscreen: r.i(160) != 0,
            style: r.i(56),
            background: material(m.window.background),
            focus_color: r.colour(232),
            disable_color: r.colour(248),
            on_open: cmds(&m.on_open),
            on_close: cmds(&m.on_close),
            on_esc: cmds(&m.on_esc),
            on_key: keys(&m.on_key),
            visible: m.visible_exp.clone(),
            sound: m.sound_name.clone(),
            items: m
                .items
                .iter()
                .flatten()
                .map(|it| {
                    let r = Raw(&it.raw);
                    LayoutItem {
                        name: it.window.name.clone().unwrap_or_default(),
                        kind: it.kind,
                        rect: r.rect(4),
                        align: [r.i(20), r.i(24)],
                        style: r.i(56),
                        border: r.i(60),
                        border_size: r.f(72),
                        static_flags: r.i(76) as u32,
                        dynamic_flags: r.i(80) as u32,
                        fore_color: r.colour(88),
                        back_color: r.colour(104),
                        border_color: r.colour(120),
                        outline_color: r.colour(136),
                        background: material(it.window.background),
                        text: it.text.clone(),
                        dvar: it.dvar.clone(),
                        font: r.i(192),
                        text_align: r.i(196),
                        text_offset: [r.f(200), r.f(204)],
                        text_scale: r.f(208),
                        text_style: r.i(212),
                        action: cmds(&it.action),
                        on_focus: cmds(&it.scripts.on_focus),
                        leave_focus: cmds(&it.scripts.leave_focus),
                        mouse_enter: cmds(&it.scripts.mouse_enter),
                        mouse_exit: cmds(&it.scripts.mouse_exit),
                        on_accept: cmds(&it.scripts.on_accept),
                        on_key: keys(&it.on_key),
                        dvar_flags: r.i(284),
                        enable_dvar: it.enable_dvar.clone(),
                        dvar_test: it.dvar_test.clone(),
                        exps: it.exps.clone(),
                        owner_draw: r.i(64),
                        feeder: r.f(292),
                        type_data: it.type_data.clone(),
                    }
                })
                .collect(),
        }
    }
}

/// A map of the arena file (`mp/cod2maps.arena`, a `code_post_gfx_mp`
/// rawfile): its `map`, `longname` (a localized key) and the game types
/// its `gametype` lists. The call vote's map lists read them
/// (`UI_LoadArenas`, cod4-hud.md 12.6).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arena {
    pub map: String,
    pub longname: String,
    pub gametypes: Vec<String>,
}

/// The arena file's `{ key "value" .. }` blocks.
pub fn arenas(text: &str) -> Vec<Arena> {
    let mut out = Vec::new();
    let mut cur: Option<Arena> = None;
    let words = tokens(text);
    let mut i = 0;
    while i < words.len() {
        match words[i].as_str() {
            "{" => cur = Some(Arena::default()),
            "}" => out.extend(cur.take()),
            key => {
                let value = words.get(i + 1).cloned().unwrap_or_default();
                if let Some(a) = cur.as_mut() {
                    match key.to_ascii_lowercase().as_str() {
                        "map" => a.map = value,
                        "longname" => a.longname = value,
                        "gametype" => {
                            a.gametypes = value.split_whitespace().map(str::to_string).collect()
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
        }
        i += 1;
    }
    out
}

/// Little-endian fields of a loaded struct; 0 past its end.
struct Raw<'a>(&'a [u8]);

impl Raw<'_> {
    fn i(&self, o: usize) -> i32 {
        self.0
            .get(o..o + 4)
            .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()))
    }
    fn f(&self, o: usize) -> f32 {
        f32::from_bits(self.i(o) as u32)
    }
    fn rect(&self, o: usize) -> [f32; 4] {
        std::array::from_fn(|k| self.f(o + 4 * k))
    }
    fn colour(&self, o: usize) -> [f32; 4] {
        self.rect(o)
    }
}

/// The `scriptMenuResponse` of an action script.
fn response(script: &str) -> Option<String> {
    let t = tokens(script);
    t.windows(2)
        .find(|w| w[0].eq_ignore_ascii_case("scriptMenuResponse"))
        .map(|w| w[1].clone())
}

/// A menu script's words: quoted strings unquoted, `;` dropped.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c == ';' {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut w = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                w.push(c);
            }
            out.push(w);
        } else {
            let mut w = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == ';' || c == '"' {
                    break;
                }
                w.push(c);
                chars.next();
            }
            out.push(w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `options_graphics`' onClose and the video restart popup's Yes
    /// (`ui_mp`): a console line on a dvar's value.
    #[test]
    fn exec_on_dvar_reads_its_test_and_line() {
        let c = Cmd::parse(
            "\"execOnDvarIntValue\" \"ui_zfeather\" 0 \"set r_zfeather 0;set r_depthPrepassModels 0\" ; \"execondvarstringvalue\" \"ui_zfeather\" 1 \"set cg_laserLight 1\" ; \"close\" \"self\" ; ",
        );
        assert_eq!(
            c,
            [
                Cmd::ExecOnDvar {
                    dvar: "ui_zfeather".into(),
                    value: "0".into(),
                    line: "set r_zfeather 0;set r_depthPrepassModels 0".into(),
                    kind: DvarCompare::Int,
                },
                Cmd::ExecOnDvar {
                    dvar: "ui_zfeather".into(),
                    value: "1".into(),
                    line: "set cg_laserLight 1".into(),
                    kind: DvarCompare::String,
                },
                Cmd::Close("self".into()),
            ]
        );
        assert!(DvarCompare::Int.passes("1.5", "1"));
        assert!(DvarCompare::String.passes("Auto", "auto"));
        assert!(DvarCompare::Float.passes("1", "1.000001"));
        assert!(!DvarCompare::Float.passes("1", "1.1"));
    }

    #[test]
    fn scripts_read_into_commands() {
        assert_eq!(
            Cmd::parse(
                "\"play\" \"mouse_over\" ; \"setLocalVarInt\" \"ui_highlight\" 3 ; \"setLocalVarString\" \"ui_choicegroup\" \"\" ; ; "
            ),
            [
                Cmd::Play("mouse_over".into()),
                Cmd::SetLocal("ui_highlight".into(), Value::Int(3)),
                Cmd::SetLocal("ui_choicegroup".into(), Value::Str(String::new())),
            ]
        );
        assert_eq!(
            Cmd::parse(
                "\"setLocalVarString\" \"ui_team\" \"marines\" \"uiScript\" \"openMenuOnDvar\" \"onlinegame\" 1 \"changeclass\" ; \"close\" \"self\" ; "
            ),
            [
                Cmd::SetLocal("ui_team".into(), Value::Str("marines".into())),
                Cmd::OpenMenuOnDvar {
                    dvar: "onlinegame".into(),
                    value: "1".into(),
                    menu: "changeclass".into(),
                    not: false
                },
                Cmd::Close("self".into()),
            ]
        );
        assert_eq!(
            Cmd::parse("\"execNow\" \"set a 1; set b 2\" \"setfocus\" \"auto_assign\""),
            [
                Cmd::Exec("set a 1; set b 2".into()),
                Cmd::SetFocus("auto_assign".into())
            ]
        );
    }

    #[test]
    fn ui_scripts_take_their_words() {
        assert_eq!(
            Cmd::parse(
                "\"uiScript\" \"closeMenuOnDvar\" \"ui_playerProfileAlreadyChosen\" 1 \"player_profile\" ; \"uiScript\" \"addPlayerProfiles\" ; \"uiScript\" \"closeJoin\" \"close\" \"self\""
            ),
            [
                Cmd::UiScript(
                    "closeMenuOnDvar".into(),
                    vec![
                        "ui_playerProfileAlreadyChosen".into(),
                        "1".into(),
                        "player_profile".into()
                    ]
                ),
                Cmd::UiScript("addPlayerProfiles".into(), Vec::new()),
                Cmd::UiScript("closeJoin".into(), Vec::new()),
                Cmd::Close("self".into()),
            ]
        );
    }

    #[test]
    fn arenas_read_their_blocks() {
        let a = arenas(
            "{\n\tmap\t\t\t\"mp_convoy\"\n\tlongname\t\"MPUI_CONVOY\"\n\tgametype\t\"dm war\"\n}\n\n{ map \"mp_bog\" }",
        );
        assert_eq!(
            a,
            [
                Arena {
                    map: "mp_convoy".into(),
                    longname: "MPUI_CONVOY".into(),
                    gametypes: vec!["dm".into(), "war".into()],
                },
                Arena {
                    map: "mp_bog".into(),
                    ..Arena::default()
                },
            ]
        );
    }

    #[test]
    fn scripts_split_into_words() {
        assert_eq!(
            response("\"play\" \"mouse_click\" ; \"scriptMenuResponse\" \"assault_mp,0\" ; ;"),
            Some("assault_mp,0".into())
        );
        assert_eq!(
            tokens("\"uiScript\" \"openMenuOnDvar\" \"onlinegame\" 1 \"changeclass\" ;"),
            [
                "uiScript",
                "openMenuOnDvar",
                "onlinegame",
                "1",
                "changeclass"
            ]
        );
    }

    /// The stat commands as `ui_mp`'s create-a-class menus write them: a
    /// list in parentheses or two bare words, the next command after.
    #[test]
    fn stat_commands_skip_their_lists() {
        let cmds = Cmd::parse(concat!(
            "\"statsetusingtable\" ( \"2402\" , \"tableLookup\" ( \"mp/statstable.csv\" , 4 , \"m16\" , 1 ) ) ; ",
            "\"statClearBitMask\" \"ui_tempStorage\" \"ui_tempMask\" ; ; ",
            "\"setDvar\" \"ui_tempStorage\" \"\" ; ",
            "\"statClearPerkNew\" ( \"c4_mp\" ) ; \"setbackground\" \"x\" \"play\" \"y\" ; ",
            "\"uiScript\" \"sortPlayerProfiles\" ; \"close\" \"self\"",
        ));
        assert_eq!(
            cmds,
            [
                Cmd::Other("statsetusingtable".into()),
                Cmd::Other("statClearBitMask".into()),
                Cmd::SetDvar("ui_tempStorage".into(), String::new()),
                Cmd::Other("statClearPerkNew".into()),
                Cmd::Other("setbackground".into()),
                Cmd::Play("y".into()),
                Cmd::UiScript("sortPlayerProfiles".into(), Vec::new()),
                Cmd::Close("self".into()),
            ]
        );
    }
}
