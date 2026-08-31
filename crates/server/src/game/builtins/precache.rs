//! Precache builtins. Each interns a name into the configstring range its
//! engine indexer walks, so a name precached twice takes one slot and the
//! slots come out in call order, which is what the retail capture pins.
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
fn name_arg(cx: &Cx, args: &[Value], what: &'static str) -> Result<String, ErrorKind> {
    match args.first() {
        Some(Value::String(a)) | Some(Value::Localized(a)) => Ok(cx.resolve(*a).to_string()),
        _ => Err(ErrorKind::BadType(what)),
    }
}

/// Allocates into `range` and returns the slot, the shape every precache
/// but the icons share.
fn precache_into(
    host: &mut GameHost,
    cx: &mut Cx,
    args: &[Value],
    range: CsRange,
    what: &'static str,
) -> Result<Value, ErrorKind> {
    let text = name_arg(cx, args, what)?;
    let slot = host
        .allocators
        .index(&mut host.configstrings, range, &text)?;
    Ok(Value::Int(slot as i32))
}

/// `precacheModel(name)`, mirroring `G_ModelIndex` (0x66ed8).
pub fn precache_model(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    precache_into(
        host,
        cx,
        args,
        CsRange::Model,
        "precacheModel takes a model name",
    )
}

/// `precacheShader(name)`, mirroring `G_ShaderIndex` (0x65ee8).
pub fn precache_shader(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    precache_into(
        host,
        cx,
        args,
        CsRange::Shader,
        "precacheShader takes a shader name",
    )
}

/// `precacheString(&"KEY")`, mirroring `G_LocalizedStringIndex` (0x65e30).
pub fn precache_string(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    precache_into(
        host,
        cx,
        args,
        CsRange::Localized,
        "precacheString takes a string key",
    )
}

/// `precacheMenu(name)`, mirroring `GScr_GetScriptMenuIndex` (0x5c73c).
pub fn precache_menu(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    precache_into(
        host,
        cx,
        args,
        CsRange::Menu,
        "precacheMenu takes a menu name",
    )
}

/// `precacheHeadIcon(name)`. `GScr_GetHeadIconIndex` (0x5c840) ends
/// `lea 0x1(%ebx),%eax`, so the icon builtins answer a 1-based icon number
/// rather than the configstring slot; that number is what `self.headicon`
/// stores.
pub fn precache_head_icon(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    icon_index(
        host,
        cx,
        args,
        CsRange::HeadIcon,
        "precacheHeadIcon takes an icon name",
    )
}

/// `precacheStatusIcon(name)`, same 1-based answer as the head icon
/// (`GScr_GetStatusIconIndex` 0x5c7c8).
pub fn precache_status_icon(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    icon_index(
        host,
        cx,
        args,
        CsRange::StatusIcon,
        "precacheStatusIcon takes an icon name",
    )
}

fn icon_index(
    host: &mut GameHost,
    cx: &mut Cx,
    args: &[Value],
    range: CsRange,
    what: &'static str,
) -> Result<Value, ErrorKind> {
    let text = name_arg(cx, args, what)?;
    let slot = host
        .allocators
        .index(&mut host.configstrings, range, &text)?;
    Ok(Value::Int((slot - range.bounds().0 + 1) as i32))
}

/// `precacheItem(name)`, mirroring `RegisterItem` (0x4e504): registers
/// `name` in the item table and rewrites the whole configstring 8
/// bitstring (`SaveRegisteredItems`, 0x4ef08) from it.
pub fn precache_item(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let name = name_arg(cx, args, "precacheItem takes an item name")?;
    host.items.register(&name);
    host.configstrings[8] = host.items.bitstring();
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
    /// first shader at 1501.
    #[test]
    fn each_precache_allocates_the_first_slot_of_its_range() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let icon = Value::String(cx.intern_exact("gfx/hud/hud@status_dead.tga"));
            assert_eq!(
                precache_status_icon(&mut host, cx, None, &[icon]).unwrap(),
                Value::Int(1)
            );
            assert_eq!(host.configstrings[21], "gfx/hud/hud@status_dead.tga");

            let head = Value::String(cx.intern_exact("gfx/hud/headicon@quickmessage"));
            assert_eq!(
                precache_head_icon(&mut host, cx, None, &[head]).unwrap(),
                Value::Int(1)
            );
            assert_eq!(host.configstrings[29], "gfx/hud/headicon@quickmessage");

            let menu = Value::String(cx.intern_exact("team_russiangerman"));
            precache_menu(&mut host, cx, None, &[menu]).unwrap();
            assert_eq!(host.configstrings[1180], "team_russiangerman");

            let s = Value::Localized(cx.intern_exact("MPSCRIPT_KILLCAM"));
            precache_string(&mut host, cx, None, &[s]).unwrap();
            assert_eq!(host.configstrings[1245], "MPSCRIPT_KILLCAM");

            let sh = Value::String(cx.intern_exact("black"));
            precache_shader(&mut host, cx, None, &[sh]).unwrap();
            assert_eq!(host.configstrings[1501], "black");

            let m = Value::String(cx.intern_exact("xmodel/crate_misc2a"));
            precache_model(&mut host, cx, None, &[m]).unwrap();
            assert_eq!(host.configstrings[269], "xmodel/crate_misc2a");
        });
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
            let first = precache_string(&mut host, cx, None, &[a]).unwrap();
            assert_eq!(precache_string(&mut host, cx, None, &[b]).unwrap(), first);
        });
    }
}
