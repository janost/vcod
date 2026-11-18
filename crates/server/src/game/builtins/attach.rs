//! Model attachment builtins, all five operating on `GEntity::attachments`,
//! a per-entity list of `(model, tag)` pairs. `setViewModel` used to live
//! here too; it hangs off a `gclient_t` in retail and now lives beside the
//! other player methods in `client.rs`.

use crate::configstrings::CsRange;
use crate::game::builtins::entity::entity_receiver;
use crate::game::host::GameHost;
use vcod_gsc::{Cx, ErrorKind, Target, Value};

use super::Builtin;

pub const NAMES: &[(&str, Builtin)] = &[
    ("attach", attach),
    ("detach", detach),
    ("detachall", detach_all),
    ("getattachmodelname", get_attach_model_name),
    ("getattachsize", get_attach_size),
    ("getattachtagname", get_attach_tag_name),
    ("getattachignorecollision", get_attach_ignore_collision),
];

/// The six attachment slots of a `gentity_t` (`+0x2ec` models, `+0x2f2`
/// tags): `G_EntAttach` (0x66190) refuses a seventh.
pub const ATTACH_SLOTS: usize = 6;

/// `self attach(model[, tag])` (0x5d334): a pair already on the entity is
/// detached and then a script error, a seventh model is one too, and the
/// model indexes through `G_ModelIndex`'s gate. The tag is kept as its
/// lower-case script string; nothing indexes it here, `ClientEndFrame` does
/// for a client's own (`client_attachments`). With no tag, retail derives
/// the root bone from the xmodel (docs/research/player-model-anim-system.md);
/// the empty tag stands in. docs/research/cod11-gsc-object-model.md,
/// "attach and detach".
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
    let text = cx.resolve(*model).to_string();
    let tag_text = match args.get(1) {
        Some(Value::String(t)) => cx.resolve(*t).to_string(),
        Some(_) => return Err(ErrorKind::BadType("attach takes a tag string")),
        None => String::new(),
    };
    // 1.5's third argument; 1.1's `attach` reads two.
    let ignore = match args.get(2) {
        Some(v) => v.as_bool()?,
        None => false,
    };
    if detach_pair(host, cx, id, &text, &tag_text)? {
        crate::game::link::model_changed(host, cx, id);
        return Err(ErrorKind::Custom(format!(
            "model '{text}' already attached to tag '{tag_text}'"
        )));
    }
    let count = host.ents.get(id).map_or(0, |e| e.attachments.len());
    if count >= ATTACH_SLOTS {
        return Err(ErrorKind::Custom("maximum attached models exceeded".into()));
    }
    // `G_EntAttach` indexes the model the way `G_ModelIndex` does, gate
    // included, and keeps the index: the name reads back as the precached
    // spelling. An empty name is index 0, a slot `getAttachSize` stops at
    // and the next attach takes again: nothing to keep.
    if text.is_empty() {
        return Ok(Value::Undefined);
    }
    let slot = host.allocators.index_precached(
        &mut host.configstrings,
        CsRange::Model,
        &text,
        host.initializing,
    )?;
    let model = cx.intern_exact(&host.configstrings[slot]);
    let tag = cx.intern_exact(&tag_text.to_ascii_lowercase());
    let e = host
        .ents
        .get_mut_quiet(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    if ignore {
        e.attach_ignore_collision |= 1 << e.attachments.len();
    }
    e.attachments.push((model, tag));
    // `G_EntAttach` ends in `G_DObjUpdate` (0x662bc).
    crate::game::link::model_changed(host, cx, id);
    Ok(Value::Undefined)
}

/// `G_EntDetach` (0x67ed0): the first slot carrying `tag` (lower-cased)
/// and `model` (any case) goes, the slots behind it close up with their
/// `ignoreCollision` bits (UO 0x8818a). Whether one went.
fn detach_pair(
    host: &mut GameHost,
    cx: &mut Cx,
    id: vcod_gsc::EntId,
    model: &str,
    tag: &str,
) -> Result<bool, ErrorKind> {
    let tag = tag.to_ascii_lowercase();
    let e = host
        .ents
        .get_mut_quiet(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    let Some(i) = e
        .attachments
        .iter()
        .position(|(m, t)| cx.resolve(*t) == tag && cx.resolve(*m).eq_ignore_ascii_case(model))
    else {
        return Ok(false);
    };
    e.attachments.remove(i);
    let low = e.attach_ignore_collision & ((1 << i) - 1);
    e.attach_ignore_collision = low | (e.attach_ignore_collision >> (i + 1)) << i;
    Ok(true)
}

/// `self detach(model[, tag])` (0x59bac): `G_EntDetach`, and when nothing
/// matched the console lists the attachments and the call is a script
/// error.
pub fn detach(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let Some(Value::String(model)) = args.first() else {
        return Err(ErrorKind::BadType("detach takes a model"));
    };
    let text = cx.resolve(*model).to_string();
    let tag_text = match args.get(1) {
        Some(Value::String(t)) => cx.resolve(*t).to_string(),
        Some(_) => return Err(ErrorKind::BadType("detach takes a tag string")),
        None => String::new(),
    };
    if detach_pair(host, cx, id, &text, &tag_text)? {
        // `G_EntDetach` ends in `G_DObjUpdate`.
        crate::game::link::model_changed(host, cx, id);
        return Ok(Value::Undefined);
    }
    if let Some(e) = host.ents.get(id) {
        log::info!("Current attachments:");
        for (m, t) in &e.attachments {
            log::info!("model: '{}', tag: '{}'", cx.resolve(*m), cx.resolve(*t));
        }
    }
    Err(ErrorKind::Custom(format!(
        "failed to detach model '{text}' from tag '{tag_text}'"
    )))
}

/// `self detachAll()`: clears the attachment list.
pub fn detach_all(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let e = host
        .ents
        .get_mut_quiet(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    e.attachments.clear();
    e.attach_ignore_collision = 0;
    // `G_EntDetachAll` ends in `G_DObjUpdate` (0x68022).
    crate::game::link::model_changed(host, cx, id);
    Ok(Value::Undefined)
}

/// The index argument of the three `getAttach*` readers, checked against
/// the attachment count: past it is retail's "bad index" script error.
fn attach_index(host: &GameHost, id: vcod_gsc::EntId, args: &[Value]) -> Result<usize, ErrorKind> {
    let Some(Value::Int(i)) = args.first() else {
        return Err(ErrorKind::BadType("takes an attachment index"));
    };
    let count = host.ents.get(id).map_or(0, |e| e.attachments.len());
    match usize::try_from(*i) {
        Ok(i) if i < count => Ok(i),
        _ => Err(ErrorKind::Custom("bad index".into())),
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

/// `self getAttachModelName(index)`: "bad index" past the end (UO
/// 0x67e71 checks the slot's model).
pub fn get_attach_model_name(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let i = attach_index(host, id, args)?;
    let e = host
        .ents
        .get(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    Ok(Value::String(e.attachments[i].0))
}

/// `self getAttachTagName(index)`, the tag half of the same pair.
pub fn get_attach_tag_name(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let i = attach_index(host, id, args)?;
    let e = host
        .ents
        .get(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    Ok(Value::String(e.attachments[i].1))
}

/// 1.5's `self getAttachIgnoreCollision(index)` (game.mp 0x5ceeb): the
/// attachment's `ignoreCollision` bit, as `saveModel` saves it for the
/// killcam's body.
pub fn get_attach_ignore_collision(
    host: &mut GameHost,
    _cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let i = attach_index(host, id, args)?;
    let e = host
        .ents
        .get(id)
        .ok_or(ErrorKind::BadType("no such entity"))?;
    Ok(Value::Int(i32::from(
        e.attach_ignore_collision >> i & 1 != 0,
    )))
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
            // The tag indexes nothing at the attach: `ClientEndFrame` does,
            // for a player's own (`ScriptRuntime::client_attachments`).
            assert_eq!(host.configstrings[109], "");

            detach_all(&mut host, cx, t, &[]).unwrap();
            assert_eq!(
                get_attach_size(&mut host, cx, t, &[]).unwrap(),
                Value::Int(0)
            );
        });
    }

    /// 1.5's third `attach` argument rides per attachment and comes back
    /// through `getAttachIgnoreCollision`; `detachAll` clears it.
    #[test]
    fn attach_keeps_ignore_collision_per_slot() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let t = Some(Target::Entity(e));
            let model = Value::String(cx.intern_exact("xmodel/gear_US_load1"));
            let tag = Value::String(cx.intern_exact("tag_origin"));
            let tag2 = Value::String(cx.intern_exact("tag_weapon"));
            attach(&mut host, cx, t, &[model, tag]).unwrap();
            attach(&mut host, cx, t, &[model, tag2, Value::Int(1)]).unwrap();
            let ignore = |host: &mut GameHost, cx: &mut Cx, i| {
                get_attach_ignore_collision(host, cx, t, &[Value::Int(i)]).unwrap()
            };
            assert_eq!(ignore(&mut host, cx, 0), Value::Int(0));
            assert_eq!(ignore(&mut host, cx, 1), Value::Int(1));
            assert!(get_attach_ignore_collision(&mut host, cx, t, &[Value::Int(2)]).is_err());
            detach_all(&mut host, cx, t, &[]).unwrap();
            attach(&mut host, cx, t, &[model, tag]).unwrap();
            assert_eq!(ignore(&mut host, cx, 0), Value::Int(0));
        });
    }

    /// Retail's three refusals: a pair already on the entity (taken off
    /// first), a seventh model, a reader past the count; the tag reads
    /// back lower-cased (cod11-gsc-object-model.md 24).
    #[test]
    fn attach_refuses_as_retail_does() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let t = Some(Target::Entity(e));
            let model = Value::String(cx.intern_exact("xmodel/crate_misc2a"));
            let upper = Value::String(cx.intern_exact("XMODEL/crate_misc2a"));
            let tag = Value::String(cx.intern_exact("TAG_A"));
            attach(&mut host, cx, t, &[model, tag]).unwrap();
            match get_attach_tag_name(&mut host, cx, t, &[Value::Int(0)]).unwrap() {
                Value::String(a) => assert_eq!(cx.resolve(a), "tag_a"),
                v => panic!("{v:?}"),
            }
            let lower = Value::String(cx.intern_exact("tag_a"));
            assert_eq!(
                attach(&mut host, cx, t, &[upper, lower]),
                Err(ErrorKind::Custom(
                    "model 'XMODEL/crate_misc2a' already attached to tag 'tag_a'".into()
                ))
            );
            assert_eq!(host.ents.get(e).unwrap().attachments.len(), 0);
            for i in 0..ATTACH_SLOTS {
                let tag = Value::String(cx.intern_exact(&format!("tag_{i}")));
                attach(&mut host, cx, t, &[model, tag]).unwrap();
            }
            let tag = Value::String(cx.intern_exact("tag_6"));
            assert_eq!(
                attach(&mut host, cx, t, &[model, tag]),
                Err(ErrorKind::Custom("maximum attached models exceeded".into()))
            );
            assert_eq!(
                get_attach_model_name(&mut host, cx, t, &[Value::Int(6)]),
                Err(ErrorKind::Custom("bad index".into()))
            );
            assert_eq!(
                detach(&mut host, cx, t, &[model, tag]),
                Err(ErrorKind::Custom(
                    "failed to detach model 'xmodel/crate_misc2a' from tag 'tag_6'".into()
                ))
            );
        });
    }

    /// `detach` drops the matching pair and shifts the rest, ignore bits
    /// with them; a pair that is not there is an error.
    #[test]
    fn detach_drops_one_attachment_and_keeps_the_rest_in_order() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let e = host.ents.spawn(cx).unwrap();
            let s = |cx: &mut Cx, t: &str| cx.intern_exact(t);
            let pairs = [("a", "tag_a"), ("flag", "tag_flag"), ("c", "tag_c")];
            let atoms: Vec<_> = pairs.iter().map(|(m, t)| (s(cx, m), s(cx, t))).collect();
            let ent = host.ents.get_mut(e).unwrap();
            ent.attachments = atoms.clone();
            ent.attach_ignore_collision = 0b100;
            let t = Some(Target::Entity(e));
            let args = [
                Value::String(s(cx, "FLAG")),
                Value::String(s(cx, "tag_flag")),
            ];
            detach(&mut host, cx, t, &args).unwrap();
            let ent = host.ents.get(e).unwrap();
            assert_eq!(ent.attachments, vec![atoms[0], atoms[2]]);
            assert_eq!(ent.attach_ignore_collision, 0b10);
            assert!(detach(&mut host, cx, t, &args).is_err());
        });
    }
}
