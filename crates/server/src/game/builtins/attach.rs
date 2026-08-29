//! Model attachment builtins. Five operate on `GEntity::attachments`, a
//! per-entity list of `(model, tag)` pairs: `attach`, `detachAll`,
//! `getAttachModelName`, `getAttachSize`, `getAttachTagName`. The other two,
//! `getViewModel`/`setViewModel`, are a single model name on the entity's
//! own field store; retail's real viewmodel plumbing hangs off a client,
//! which stage 4 gives entities, so until then this is a plain field round
//! trip rather than anything a client would render.

use crate::configstrings::CsRange;
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Host, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("attach", attach),
    ("detachall", detach_all),
    ("getattachmodelname", get_attach_model_name),
    ("getattachsize", get_attach_size),
    ("getattachtagname", get_attach_tag_name),
    ("getviewmodel", get_view_model),
    ("setviewmodel", set_view_model),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// `self attach(model)` or `self attach(model, tag)`. Retail derives the
/// root bone name from the xmodel when no tag is given
/// (docs/research/player-model-anim-system.md); parsing an xmodel is out of
/// this stage's reach, so a call with no tag stores an empty one rather than
/// guessing a bone name. A real tag still allocates its configstring
/// through `G_TagIndex`.
pub fn attach(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let Some(Value::String(model)) = args.first() else {
        return Err(ErrorKind::BadType("attach takes a model"));
    };
    let model = *model;
    let tag = match args.get(1) {
        Some(Value::String(t)) => {
            let t = *t;
            let text = cx.resolve(t).to_string();
            host.allocators
                .index(&mut host.configstrings, CsRange::Tag, &text)?;
            t
        }
        _ => cx.intern_exact(""),
    };
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.attachments.push((model, tag));
    Ok(Value::Undefined)
}

/// `self detachAll()`: clears the attachment list.
pub fn detach_all(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let e = host
        .ents
        .get_mut(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.attachments.clear();
    Ok(Value::Undefined)
}

fn attach_index(args: &[Value]) -> Result<usize, ErrorKind> {
    match args.first() {
        Some(Value::Int(i)) if *i >= 0 => Ok(*i as usize),
        _ => Err(ErrorKind::BadType("takes an attachment index")),
    }
}

/// `self getAttachSize()`: how many attachments are on the entity.
pub fn get_attach_size(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let e = host
        .ents
        .get(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    Ok(Value::Int(e.attachments.len() as i32))
}

/// `self getAttachModelName(index)`: `undefined` past the end, same as a
/// missed array index.
pub fn get_attach_model_name(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let i = attach_index(args)?;
    let e = host
        .ents
        .get(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    Ok(e.attachments
        .get(i)
        .map_or(Value::Undefined, |(model, _)| Value::String(*model)))
}

/// `self getAttachTagName(index)`, the tag half of the same pair.
pub fn get_attach_tag_name(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let i = attach_index(args)?;
    let e = host
        .ents
        .get(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    Ok(e.attachments
        .get(i)
        .map_or(Value::Undefined, |(_, tag)| Value::String(*tag)))
}

/// `self setViewModel(name)`: stored under a script field the way
/// `setModel` stores `.model`.
pub fn set_view_model(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let Some(Value::String(name)) = args.first() else {
        return Err(ErrorKind::BadType("setViewModel takes a model name"));
    };
    let name = *name;
    let field = cx.intern_folded("viewmodel");
    host.set_field(cx, id, field, Value::String(name))?;
    Ok(Value::Undefined)
}

/// `self getViewModel()`, the read side of `setViewModel`.
pub fn get_view_model(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let field = cx.intern_folded("viewmodel");
    Ok(host.get_field(cx, id, field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// `attach` appends to the entity's attachment list; `getAttachSize`,
    /// `getAttachModelName` and `getAttachTagName` read it back in order,
    /// and `detachAll` empties it.
    #[test]
    fn attach_builds_a_list_detachall_clears_it() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let t = Some(Target::Entity(e));
            let head = Value::String(cx.intern_exact("xmodel/head_allied"));
            attach(&mut host, cx, t, &[head]).unwrap();
            let helmet = Value::String(cx.intern_exact("xmodel/USAirborneHelmet"));
            let tag = Value::String(cx.intern_exact("tag_head"));
            attach(&mut host, cx, t, &[helmet, tag]).unwrap();

            assert_eq!(
                get_attach_size(&mut host, cx, t, &[]).unwrap(),
                Value::Int(2)
            );
            match get_attach_model_name(&mut host, cx, t, &[Value::Int(1)]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), "xmodel/USAirborneHelmet"),
                v => panic!("{v:?}"),
            }
            match get_attach_tag_name(&mut host, cx, t, &[Value::Int(1)]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), "tag_head"),
                v => panic!("{v:?}"),
            }
            assert_eq!(host.configstrings[109], "tag_head");

            detach_all(&mut host, cx, t, &[]).unwrap();
            assert_eq!(
                get_attach_size(&mut host, cx, t, &[]).unwrap(),
                Value::Int(0)
            );
        });
    }

    /// `setViewModel`/`getViewModel` round-trip through the entity's own
    /// field store.
    #[test]
    fn view_model_round_trips() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let t = Some(Target::Entity(e));
            let name = Value::String(cx.intern_exact("viewmodel/kar98k"));
            set_view_model(&mut host, cx, t, &[name]).unwrap();
            match get_view_model(&mut host, cx, t, &[]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), "viewmodel/kar98k"),
                v => panic!("{v:?}"),
            }
        });
    }
}
