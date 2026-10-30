//! Precache builtins. Each interns a name into the configstring range its
//! engine indexer walks, so a name precached twice (in any case) takes one
//! slot and the slots come out in call order, which is what the retail
//! capture pins. None of them answers anything: retail pushes no return
//! value, so the call reads `undefined`.
//!
//! Model, shellshock, item, shader and string precaches are refused past the
//! script's first wait (`level.initializing` clear); menus and icons are not
//! (docs/research/cod11-gsc-language.md, "Precache builtins").
//!
//! `precacheItem` (below) is the exception: it registers a `bg_itemlist`
//! entry (`crate::items::Items`) and writes the whole configstring 8
//! bitstring rather than allocating the next slot of a range, so it is not
//! a range allocator like its neighbours here.

use crate::configstrings::CsRange;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("precachemodel", precache_model),
    ("precacheshellshock", precache_shell_shock),
    ("precacheshader", precache_shader),
    ("precachestring", precache_string),
    ("precachemenu", precache_menu),
    ("precacheheadicon", precache_head_icon),
    ("precachestatusicon", precache_status_icon),
    ("precacheitem", precache_item),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// The name argument, accepting both a plain string and a localized
/// literal: `precacheString(&"MPSCRIPT_KILLCAM")` passes `Value::Localized`
/// and the two spellings name the same configstring text.
pub(crate) fn name_arg(cx: &Cx, args: &[Value], what: &'static str) -> Result<String, ErrorKind> {
    match args.first() {
        Some(Value::String(a)) | Some(Value::Localized(a)) => Ok(cx.resolve(*a).to_string()),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// The check each gated builtin makes first, before it reads its argument
/// (game.mp 0x5ecb8..0x5ee00, strings 0x77580..0x77740).
pub(crate) fn before_any_wait(host: &GameHost, builtin: &str) -> Result<(), ErrorKind> {
    if host.initializing {
        return Ok(());
    }
    Err(ErrorKind::Custom(format!(
        "{builtin} must be called before any wait statements in the gametype or level script"
    )))
}

/// The gate, then the name interned into `range`. An empty name takes no
/// slot: every indexer behind these returns 0 for one.
fn gated_precache(
    host: &mut GameHost,
    cx: &mut Cx,
    args: &[Value],
    builtin: &str,
    range: CsRange,
    what: &'static str,
) -> Result<Value, ErrorKind> {
    before_any_wait(host, builtin)?;
    let text = name_arg(cx, args, what)?;
    if !text.is_empty() {
        host.allocators
            .index(&mut host.configstrings, range, &text)?;
    }
    Ok(Value::Undefined)
}

/// `precacheModel(name)` (0x5ecb8), into `G_ModelIndex` (0x66ed8).
pub fn precache_model(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    gated_precache(
        host,
        cx,
        args,
        "precacheModel",
        CsRange::Model,
        "precacheModel takes a model name",
    )
}

/// `precacheShellShock(name)` (0x5ecf0), into `G_ShellShockIndex`
/// (0x67540). No stock MP script calls it.
pub fn precache_shell_shock(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    gated_precache(
        host,
        cx,
        args,
        "precacheShellShock",
        CsRange::ShellShock,
        "precacheShellShock takes a shellshock name",
    )
}

/// `precacheShader(name)` (0x5eda8), into `G_ShaderIndex` (0x65ee8). An
/// empty name is a parameter error.
pub fn precache_shader(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    before_any_wait(host, "precacheShader")?;
    let text = name_arg(cx, args, "precacheShader takes a shader name")?;
    if text.is_empty() {
        return Err(ErrorKind::Custom("Shader name string is empty".into()));
    }
    let text = host.shader_name(&text);
    host.allocators
        .index(&mut host.configstrings, CsRange::Shader, &text)?;
    Ok(Value::Undefined)
}

/// `precacheString(&"KEY")` (0x5ee00), into `G_LocalizedStringIndex`
/// (0x65e30); an empty key is skipped.
pub fn precache_string(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    gated_precache(
        host,
        cx,
        args,
        "precacheString",
        CsRange::Localized,
        "precacheString takes a string key",
    )
}

/// `precacheMenu(name)` (0x5a92c). Not gated; the overflow message is the
/// menu's own wording.
pub fn precache_menu(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let text = name_arg(cx, args, "precacheMenu takes a menu name")?;
    ungated_precache(
        host,
        &text,
        CsRange::Menu,
        "the menu",
        "Too many menus precached. Max allowed menus is",
    )
}

/// `precacheHeadIcon(name)` (0x5aae4). The 1-based icon number
/// `self.headicon` stores comes from `GScr_GetHeadIconIndex` (0x5c840), not
/// from here.
pub fn precache_head_icon(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let text = name_arg(cx, args, "precacheHeadIcon takes an icon name")?;
    ungated_precache(
        host,
        &text,
        CsRange::HeadIcon,
        "the player head icon",
        "Too many player head icons precached. Max allowed is",
    )
}

/// `precacheStatusIcon(name)` (0x5aa08).
pub fn precache_status_icon(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let text = name_arg(cx, args, "precacheStatusIcon takes an icon name")?;
    ungated_precache(
        host,
        &text,
        CsRange::StatusIcon,
        "the player status icon",
        "Too many player status icons precached. Max allowed is",
    )
}

/// The menu and icon builtins: a name already there (`Q_stricmp`) is a
/// developer print and nothing else, a full range is a script error naming
/// the range's size.
fn ungated_precache(
    host: &mut GameHost,
    text: &str,
    range: CsRange,
    what: &str,
    too_many: &str,
) -> Result<Value, ErrorKind> {
    let (lo, hi) = host.allocators.layout().bounds(range);
    let taken = host.configstrings[lo..=hi]
        .iter()
        .any(|s| s.eq_ignore_ascii_case(text));
    if !taken && host.configstrings[lo..=hi].iter().all(|s| !s.is_empty()) {
        return Err(ErrorKind::Custom(format!("{too_many} {}", hi - lo + 1)));
    }
    if taken {
        log::debug!("Script tried to precache {what} '{text}' more than once");
    } else {
        host.allocators
            .index(&mut host.configstrings, range, text)?;
    }
    Ok(Value::Undefined)
}

/// `precacheItem(name)` (0x5ed28), the script's way into `RegisterItem`
/// (`GameHost::register_item`). `BG_FindItem` is a `strcmp`; a miss is a
/// script error.
pub fn precache_item(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    before_any_wait(host, "precacheItem")?;
    let name = name_arg(cx, args, "precacheItem takes an item name")?;
    if !crate::items::is_item(host.defs, &name) {
        return Err(ErrorKind::Custom(format!("unknown item '{name}'")));
    }
    host.register_item(&name)?;
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// Each precache builtin allocates into its own range, and the first
    /// allocation lands on the range's first slot, which is what the
    /// retail capture holds: `status_dead` at 21, the head icon at 29,
    /// the first menu at 1180, the first localized string at 1245 and the
    /// first shader at 1501. None answers a value.
    #[test]
    fn each_precache_allocates_the_first_slot_of_its_range() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let calls: [(Builtin, Value, usize, &str); 7] = [
                (
                    precache_status_icon,
                    Value::String(cx.intern_exact("gfx/hud/hud@status_dead.tga")),
                    21,
                    "gfx/hud/hud@status_dead.tga",
                ),
                (
                    precache_head_icon,
                    Value::String(cx.intern_exact("gfx/hud/headicon@quickmessage")),
                    29,
                    "gfx/hud/headicon@quickmessage",
                ),
                (
                    precache_menu,
                    Value::String(cx.intern_exact("team_russiangerman")),
                    1180,
                    "team_russiangerman",
                ),
                (
                    precache_string,
                    Value::Localized(cx.intern_exact("MPSCRIPT_KILLCAM")),
                    1245,
                    "MPSCRIPT_KILLCAM",
                ),
                (
                    precache_shader,
                    Value::String(cx.intern_exact("black")),
                    1501,
                    "black",
                ),
                (
                    precache_model,
                    Value::String(cx.intern_exact("xmodel/crate_misc2a")),
                    269,
                    "xmodel/crate_misc2a",
                ),
                (
                    precache_shell_shock,
                    Value::String(cx.intern_exact("default")),
                    1101,
                    "default",
                ),
            ];
            for (f, arg, slot, text) in calls {
                assert_eq!(f(&mut host, cx, None, &[arg]).unwrap(), Value::Undefined);
                assert_eq!(host.configstrings[slot], text);
            }
        });
    }

    /// probe_precache_wait on retail: past the first wait the model, shader,
    /// string, item and shellshock precaches are script errors even for a
    /// name already precached, while menus and icons still allocate.
    #[test]
    fn past_the_first_wait_only_menus_and_icons_precache() {
        let (mut vm, mut host) = fixture();
        host.initializing = false;
        vm.with_cx(|cx| {
            let gated: [(Builtin, &str); 5] = [
                (precache_model, "precacheModel"),
                (precache_shell_shock, "precacheShellShock"),
                (precache_item, "precacheItem"),
                (precache_shader, "precacheShader"),
                (precache_string, "precacheString"),
            ];
            for (f, name) in gated {
                let arg = Value::String(cx.intern_exact("item_health"));
                assert_eq!(
                    f(&mut host, cx, None, &[arg]),
                    Err(ErrorKind::Custom(format!(
                        "{name} must be called before any wait statements in the gametype or level script"
                    )))
                );
            }
            for f in [precache_menu, precache_head_icon, precache_status_icon] {
                let arg = Value::String(cx.intern_exact("late"));
                assert_eq!(f(&mut host, cx, None, &[arg]).unwrap(), Value::Undefined);
            }
        });
        assert_eq!(host.configstrings[269], "");
        assert_eq!(
            (
                &host.configstrings[21][..],
                &host.configstrings[29][..],
                &host.configstrings[1180][..]
            ),
            ("late", "late", "late")
        );
    }

    /// A status icon in another case is the one already there
    /// (`Q_stricmp`), and the ninth distinct one is the range's script
    /// error (probe_precache_icon_full).
    #[test]
    fn status_icons_fold_case_and_stop_at_eight() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            for i in 1..=8 {
                let arg = Value::String(cx.intern_exact(&format!("icon_{i}")));
                precache_status_icon(&mut host, cx, None, &[arg]).unwrap();
            }
            let again = Value::String(cx.intern_exact("ICON_1"));
            assert_eq!(
                precache_status_icon(&mut host, cx, None, &[again]).unwrap(),
                Value::Undefined
            );
            let ninth = Value::String(cx.intern_exact("icon_9"));
            assert_eq!(
                precache_status_icon(&mut host, cx, None, &[ninth]),
                Err(ErrorKind::Custom(
                    "Too many player status icons precached. Max allowed is 8".into()
                ))
            );
        });
        assert_eq!(host.configstrings[21], "icon_1");
    }

    /// probe_precache_empty and probe_precache_unknown_item: an empty model
    /// or string is skipped, an empty shader and an unknown item are errors.
    #[test]
    fn empty_names_and_unknown_items_answer_as_retail() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let empty = Value::String(cx.intern_exact(""));
            precache_model(&mut host, cx, None, std::slice::from_ref(&empty)).unwrap();
            precache_string(&mut host, cx, None, std::slice::from_ref(&empty)).unwrap();
            assert_eq!(
                precache_shader(&mut host, cx, None, &[empty]),
                Err(ErrorKind::Custom("Shader name string is empty".into()))
            );
            let bogus = Value::String(cx.intern_exact("probe_no_such_item"));
            assert_eq!(
                precache_item(&mut host, cx, None, &[bogus]),
                Err(ErrorKind::Custom(
                    "unknown item 'probe_no_such_item'".into()
                ))
            );
        });
        assert_eq!(host.configstrings[269], "");
        assert_eq!(host.configstrings[1245], "");
    }

    /// `RegisterItem` precaches the item's own two model fields
    /// (`bg_itemlist` +0x8 and +0xc) as well as setting its bit, which is
    /// why retail's model block carries `xmodel/health_medium` between the
    /// map's props and the team player models.
    #[test]
    fn precache_item_takes_a_model_slot_for_the_items_world_model() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let item = Value::String(cx.intern_exact("item_health"));
            precache_item(&mut host, cx, None, &[item]).unwrap();
        });
        assert_eq!(host.configstrings[269], "xmodel/health_medium");
        assert_eq!(host.configstrings[270], "");
    }

    /// `precacheString` takes a localized literal (`&"KEY"`), which is
    /// `Value::Localized`, not `Value::String`. Both spellings resolve to
    /// the same atom text, so both are accepted.
    #[test]
    fn precache_string_accepts_a_localized_literal_and_a_plain_string() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let a = Value::Localized(cx.intern_exact("MPSCRIPT_KILLCAM"));
            let b = Value::String(cx.intern_exact("MPSCRIPT_KILLCAM"));
            precache_string(&mut host, cx, None, &[a]).unwrap();
            precache_string(&mut host, cx, None, &[b]).unwrap();
        });
        assert_eq!(host.configstrings[1245], "MPSCRIPT_KILLCAM");
        assert_eq!(host.configstrings[1246], "");
    }
}
