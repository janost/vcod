//! Effect builtins. `loadFX` allocates an effect configstring, mirroring
//! `G_EffectIndex`; `playFX` raises the temp entity the client plays it
//! off, `playFXOnTag` an event on the tagged entity's ring and
//! `playLoopedFx` an `ET_FX` entity the client replays on its own
//! (docs/research/cod11-events-and-fx.md 2).

use crate::game::host::GameHost;
use crate::game::temp_entity::{Quake, Scope, TempEntity};
use vcod_gsc::{Cx, ErrorKind, Host, Target, Value};

pub use vcod_common::net::event_ids::EV_PLAY_FX;
pub use vcod_common::net::event_ids::EV_PLAY_FX_DIR;
pub use vcod_common::net::event_ids::EV_PLAY_FX_ON_TAG;

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("loadfx", load_fx),
    ("playfx", play_fx),
    ("playfxontag", play_fx_on_tag),
    ("playloopedfx", play_looped_fx),
    ("grenadeexplosioneffect", grenade_explosion_effect),
    ("earthquake", earthquake),
];

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

/// `loadFX(path)`: intern the effect path into the effect configstring
/// range and hand back its effect id, which is what `level._effect[...]`
/// stores and `playFX` takes back. A new effect past the script's first
/// wait is retail's script error (`G_EffectIndex` 0x65fa4 under a clear
/// `level.initializing`); one already loaded, in any case, is found.
pub fn load_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::String(path)) = args.first() else {
        return Err(ErrorKind::BadType("loadFX takes an effect path"));
    };
    let path = *path;
    let text = cx.resolve(path).to_string();
    let id = host
        .allocators
        .effect_index(&mut host.configstrings, &text, host.initializing)?;
    Ok(Value::Int(id))
}

/// `playFX(fx, origin[, forward])` (`.so` 0x5b148): a global call, no
/// receiver. A `G_TempEntity` at `origin` carrying `EV_PLAY_FX` with the
/// effect id in `eventParm`, or with a forward vector `EV_PLAY_FX_DIR` and
/// its `DirToByte` in `scale` (`docs/research/cod11-events-and-fx.md`,
/// section 2). The effect's own `Sound` block is what the client plays,
/// so without this event a scripted explosion is silent as well as unseen.
pub fn play_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Int(id)), Some(Value::Vector(origin)), 2..=3) =
        (args.first(), args.get(1), args.len())
    else {
        return Err(ErrorKind::BadType(
            "playFX takes an fx handle, an origin and an optional forward vector",
        ));
    };
    let (event, scale) = match args.get(2) {
        Some(Value::Vector(forward)) => {
            let dir = glam::Vec3::from(*forward).normalize_or_zero();
            if dir == glam::Vec3::ZERO {
                return Err(ErrorKind::BadType(
                    "playFx called with (0 0 0) forward direction",
                ));
            }
            let byte = vcod_common::net::events::dir_to_byte(dir.to_array());
            (EV_PLAY_FX_DIR, byte)
        }
        Some(_) => return Err(ErrorKind::BadType("playFX's forward must be a vector")),
        None => (EV_PLAY_FX, 0),
    };
    host.add_temp_entity(
        cx,
        TempEntity {
            event,
            parm: id & 0xff,
            surf_type: 0,
            other: 0,
            attacker: 0,
            weapon: 0,
            client_num: 0,
            scale,
            origin: *origin,
            scope: Scope::Pvs,
            quake: None,
            origin2: None,
        },
    );
    Ok(Value::Undefined)
}

/// `earthquake(scale, duration, source, radius)` (`.so` 0x5f3d8): an
/// `EV_EARTHQUAKE` temp entity at `source` carrying the scale, the
/// duration in ms (`duration * 1000 + 0.5`, truncated) and the radius,
/// which the client shakes its view by (`play::quake`). Nothing is
/// checked; the client drops a scale that is not positive.
pub fn earthquake(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let num = |i: usize| match args.get(i) {
        Some(Value::Int(n)) => Ok(*n as f32),
        Some(Value::Float(f)) => Ok(*f),
        _ => Err(ErrorKind::BadType(
            "earthquake takes a scale, a duration, an origin and a radius",
        )),
    };
    let (scale, duration, radius) = (num(0)?, num(1)?, num(3)?);
    let Some(Value::Vector(origin)) = args.get(2) else {
        return Err(ErrorKind::BadType("earthquake's source must be a vector"));
    };
    host.add_temp_entity(
        cx,
        TempEntity {
            event: vcod_common::net::event_ids::EV_EARTHQUAKE,
            parm: 0,
            surf_type: 0,
            other: 0,
            attacker: 0,
            weapon: 0,
            client_num: 0,
            scale: 0,
            origin: *origin,
            scope: Scope::Pvs,
            quake: Some(Quake {
                scale,
                duration_ms: (duration * 1000.0 + 0.5) as i32,
                radius,
            }),
            origin2: None,
        },
    );
    Ok(Value::Undefined)
}

/// `grenadeExplosionEffect(origin)` (`.so` 0x5aea4, combat doc 13.4): an
/// `EV_GRENADE_EXPLODE` temp entity a unit above `origin` with an up
/// `eventParm` and the surface 16 units under `origin`, then
/// `Concussive_fx`'s entity (0x54840), each a number off `G_Spawn`. No stock
/// script calls it.
pub fn grenade_explosion_effect(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Value::Vector(origin)) = args.first() else {
        return Err(ErrorKind::BadType("grenadeExplosionEffect takes an origin"));
    };
    let at = glam::Vec3::from(*origin) + glam::Vec3::Z;
    // `trap_Trace(at, 0, 0, at - 17 z, ENTITYNUM_NONE, 0x11)` (0x5af43,
    // the 17.0 at rodata 0x778ec); no water test, unlike `G_ExplodeMissile`.
    let surf_type = host.world.as_ref().map_or(0, |w| {
        let down = w.collision.point_trace(
            at,
            at - glam::Vec3::Z * 17.0,
            vcod_common::collision::MASK_MISSILE,
            false,
        );
        vcod_common::collision::sound_material(down.surface_flags)
    });
    host.add_temp_entity(
        cx,
        TempEntity {
            event: vcod_common::net::event_ids::EV_GRENADE_EXPLODE,
            parm: vcod_common::net::events::dir_to_byte([0.0, 0.0, 1.0]),
            surf_type,
            other: 0,
            attacker: 0,
            weapon: 0,
            client_num: 0,
            scale: 0,
            origin: at.to_array(),
            scope: Scope::Pvs,
            quake: None,
            origin2: None,
        },
    );
    host.spawn_concussive(cx);
    Ok(Value::Undefined)
}

/// `playFxOnTag(fx, entity, tag)` (`.so` 0x5b280), a global call: the
/// effect id must be 1..63, the entity must have a model carrying the tag
/// and the tag no `"`. `"%02d%s"` of the id and the tag goes into the
/// configstrings from 844 and `G_AddEvent` puts `EV_PLAY_FX_ON_TAG` with
/// that index on the entity (0x5b3b0), its playerstate's ring for a client.
pub fn play_fx_on_tag(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let [Value::Int(fx), Value::Entity(ent), Value::String(tag)] = args else {
        return Err(ErrorKind::BadType(
            "USAGE: playFxOnTag <effect id from loadFx> <entity> <tag name>",
        ));
    };
    let (fx, ent) = (*fx, *ent);
    if !(1..64).contains(&fx) {
        return Err(ErrorKind::BadType("effect id is invalid"));
    }
    let model = cx.intern_folded("model");
    let has_model =
        matches!(host.get_field(cx, ent, model), Value::String(m) if !cx.resolve(m).is_empty());
    if !has_model {
        return Err(ErrorKind::BadType("cannot play fx on entity with no model"));
    }
    let tag = cx.resolve(*tag).to_string();
    if tag.contains('"') {
        return Err(ErrorKind::BadType("cannot use \" characters in tag names"));
    }
    // `DObjGetBoneIndex`; a host with no paks has no skeleton to ask.
    if host.fs.is_some() && crate::game::link::tag_check(host, cx, ent, &tag).is_err() {
        return Err(ErrorKind::BadType("tag does not exist on entity"));
    }
    let parm = host
        .allocators
        .fx_tag_index(&mut host.configstrings, fx, &tag)?;
    let Some(e) = host.ents.get_mut(ent) else {
        return Ok(Value::Undefined);
    };
    if e.client.is_some() {
        host.client_sim_ops.push((
            ent.0 as usize,
            crate::game::host::SimOp::Event {
                event: EV_PLAY_FX_ON_TAG,
                parm,
            },
        ));
    } else {
        e.events.add(EV_PLAY_FX_ON_TAG, parm);
    }
    Ok(Value::Undefined)
}

/// What `playLoopedFx` writes into its `ET_FX` entity's state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopedFx {
    /// `es.scale`, the effect id's low byte.
    pub id: i32,
    /// `es.origin2`: the normalized forward, zero for none.
    pub forward: [f32; 3],
    /// `es.angles2[0]`, 0 for never culled.
    pub cull: f32,
    /// `es.angles2[1]`, the repeat delay in whole ms.
    pub period_ms: i32,
    /// `es.eventParm`: CoD2's `DirToByte` of the up vector, 0 for none
    /// and always 0 on CoD1.
    pub up: i32,
}

/// `playLoopedFx(fx, delay, origin[, cull[, forward]])` (`.so` 0x5b3c0):
/// a `G_Spawn` turned `ET_FX` (0x5b536) and `SVF_BROADCAST` (0x5b53d),
/// `G_SetOrigin` at `origin`, the fields of [`LoopedFx`], linked, and
/// handed back. The delay goes to ms as `delay * 1000 + 0.5` truncated
/// (the 1000.0 and 0.5 at rodata 0x77cc4/0x77cc8). A zero forward is an
/// error; nothing checks the delay.
pub fn play_looped_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "USAGE: playLoopedFx <effect id from loadFx> <repeat delay> <vector position of effect> <optional cull distance (0 = never cull)> <optional forward vector>";
    let float = |v: Option<&Value>| match v {
        Some(Value::Int(n)) => Ok(*n as f32),
        Some(Value::Float(f)) => Ok(*f),
        _ => Err(ErrorKind::BadType(USAGE)),
    };
    let (3..=5, Some(Value::Int(fx)), Some(Value::Vector(origin))) =
        (args.len(), args.first(), args.get(2))
    else {
        return Err(ErrorKind::BadType(USAGE));
    };
    let forward = match args.get(4) {
        Some(Value::Vector(v)) => {
            let dir = glam::Vec3::from(*v).normalize_or_zero();
            if dir == glam::Vec3::ZERO {
                return Err(ErrorKind::BadType(
                    "playLoopedFx called with (0 0 0) forward direction",
                ));
            }
            dir.to_array()
        }
        Some(_) => return Err(ErrorKind::BadType(USAGE)),
        None => [0.0; 3],
    };
    let cull = match args.get(3) {
        Some(v) => float(Some(v))?,
        None => 0.0,
    };
    let period_ms = (float(args.get(1))? * 1000.0 + 0.5) as i32;
    spawn_looped_fx(
        host,
        cx,
        *origin,
        LoopedFx {
            id: fx & 0xff,
            forward,
            cull,
            period_ms,
            up: 0,
        },
    )
}

/// The `ET_FX` entity both games' `playLoopedFx` spawn: `G_Spawn`,
/// broadcast, `G_SetOrigin` at `origin`, `fx` for the wire, linked, and
/// handed back.
pub fn spawn_looped_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    origin: [f32; 3],
    fx: LoopedFx,
) -> Result<Value, ErrorKind> {
    let id = host.ents.spawn(cx)?;
    let classname = cx.intern_folded("classname");
    let noclass = Value::String(cx.intern_exact("noclass"));
    host.set_field(cx, id, classname, noclass)?;
    let og = cx.intern_folded("origin");
    host.set_field(cx, id, og, Value::Vector(origin))?;
    if let Some(e) = host.ents.get_mut(id) {
        e.broadcast = true;
    }
    host.looped_fx.insert(id, fx);
    Ok(Value::Entity(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// `loadFX` interns the effect path into the effect configstring range
    /// and hands back its effect id, the offset from 780 that `G_EffectIndex`
    /// returns and `EV_PLAY_FX` carries, not the configstring number.
    #[test]
    fn loadfx_allocates_an_effect_configstring_and_returns_its_id() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let p = Value::String(cx.intern_exact("fx/impacts/newimps/minefield.efx"));
            assert_eq!(load_fx(&mut host, cx, None, &[p]).unwrap(), Value::Int(1));
            assert_eq!(host.configstrings[781], "fx/impacts/newimps/minefield.efx");
            let again = Value::String(cx.intern_exact("fx/impacts/newimps/minefield.efx"));
            assert_eq!(
                load_fx(&mut host, cx, None, &[again]).unwrap(),
                Value::Int(1)
            );
        });
    }

    /// Past `level.initializing` (probe_loadfx_wait on retail) a loaded
    /// effect is still found, in any case, and a new one is the
    /// `not precached` error with nothing allocated.
    #[test]
    fn loadfx_after_the_first_wait_finds_only_loaded_effects() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let p = Value::String(cx.intern_exact("fx/explosions/mp_bomb.efx"));
            assert_eq!(load_fx(&mut host, cx, None, &[p]).unwrap(), Value::Int(1));
            host.initializing = false;
            let upper = Value::String(cx.intern_exact("FX/Explosions/MP_BOMB.efx"));
            assert_eq!(
                load_fx(&mut host, cx, None, &[upper]).unwrap(),
                Value::Int(1)
            );
            let fresh = Value::String(cx.intern_exact("fx/smoke/chimneysmoke.efx"));
            assert_eq!(
                load_fx(&mut host, cx, None, &[fresh]),
                Err(ErrorKind::Custom(
                    "effect \"fx/smoke/chimneysmoke.efx\" not precached".into()
                ))
            );
            assert_eq!(host.configstrings[782], "");
        });
    }

    /// `playFX` is a temp entity at the origin: `EV_PLAY_FX` with the id in
    /// `eventParm`, or with a forward vector `EV_PLAY_FX_DIR` and the
    /// vector's `DirToByte` in `scale`. A zero forward is retail's error.
    #[test]
    fn playfx_raises_the_effect_event_at_the_origin() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let at = Value::Vector([10.0, 20.0, 30.0]);
            play_fx(&mut host, cx, None, &[Value::Int(3), at]).unwrap();
            let up = Value::Vector([0.0, 0.0, 5.0]);
            play_fx(&mut host, cx, None, &[Value::Int(3), at, up]).unwrap();
            let zero = Value::Vector([0.0; 3]);
            assert!(play_fx(&mut host, cx, None, &[Value::Int(3), at, zero]).is_err());
        });
        let [plain, dir] = &host.temp_entities[..] else {
            panic!("two events, not {}", host.temp_entities.len());
        };
        let (plain, dir) = (&plain.te, &dir.te);
        assert_eq!((plain.event, plain.parm, plain.scale), (EV_PLAY_FX, 3, 0));
        assert_eq!(plain.origin, [10.0, 20.0, 30.0]);
        assert_eq!(plain.scope, Scope::Pvs);
        let up = vcod_common::net::events::dir_to_byte([0.0, 0.0, 1.0]);
        assert_eq!((dir.event, dir.parm, dir.scale), (EV_PLAY_FX_DIR, 3, up));
    }

    /// `earthquake` is a temp entity at the source carrying the scale, the
    /// duration rounded to whole ms and the radius, and its wire state puts
    /// them in `angles2[0]`, `time` (UO: `angles2[2]`) and `angles2[1]`.
    #[test]
    fn earthquake_raises_its_event_with_the_three_fields() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let at = Value::Vector([1.0, 2.0, 3.0]);
            let args = [Value::Float(0.3), Value::Float(0.0026), at, Value::Int(850)];
            earthquake(&mut host, cx, None, &args).unwrap();
            assert!(earthquake(&mut host, cx, None, &args[..3]).is_err());
        });
        let [quake] = &host.temp_entities[..] else {
            panic!("one event, not {}", host.temp_entities.len());
        };
        let te = &quake.te;
        assert_eq!(te.event, vcod_common::net::event_ids::EV_EARTHQUAKE);
        assert_eq!(te.origin, [1.0, 2.0, 3.0]);
        assert_eq!(
            te.quake,
            Some(Quake {
                scale: 0.3,
                duration_ms: 3,
                radius: 850.0
            })
        );
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let es = crate::game::temp_entity::build(te, 70, p);
        assert_eq!(es.field_f32(p, "angles2[0]"), 0.3);
        assert_eq!(es.field_i32(p, "time"), 3);
        assert_eq!(es.field_f32(p, "angles2[1]"), 850.0);
        // UO's carries the duration as a float in `angles2[2]`, as the
        // retail tank death's `earthquake(0.25, 3, ...)` arrives with
        // 3000.0 there and `time` 0 (`*-uo-wreck.bin`).
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let es = crate::game::temp_entity::build(te, 70, p);
        assert_eq!(es.field_f32(p, "angles2[2]"), 3.0);
        assert_eq!(es.field_i32(p, "time"), 0);
        assert_eq!(es.field_f32(p, "angles2[1]"), 850.0);
    }

    /// `playLoopedFx` hands back an `ET_FX` entity carrying the id in
    /// `scale`, the normalized forward in `origin2` and the cull and the
    /// whole-ms period in `angles2`, sent to every client. A zero forward
    /// and a missing origin are errors.
    #[test]
    fn playloopedfx_spawns_a_broadcast_fx_entity() {
        let (mut vm, mut host) = fixture();
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let (plain, aimed, states) = vm.with_cx(|cx| {
            let at = Value::Vector([1984.0, -210.0, 512.0]);
            let plain =
                play_looped_fx(&mut host, cx, None, &[Value::Int(2), Value::Float(0.2), at])
                    .unwrap();
            let fwd = Value::Vector([0.0, 3.0, 4.0]);
            let args = [
                Value::Int(2),
                Value::Float(1.2345),
                at,
                Value::Int(1000),
                fwd,
            ];
            let aimed = play_looped_fx(&mut host, cx, None, &args).unwrap();
            let zero = Value::Vector([0.0; 3]);
            let bad = [Value::Int(2), Value::Float(1.0), at, Value::Int(0), zero];
            assert!(play_looped_fx(&mut host, cx, None, &bad).is_err());
            assert!(
                play_looped_fx(&mut host, cx, None, &[Value::Int(2), Value::Float(1.0)]).is_err()
            );
            let states = crate::game::wire::packet_entities(&mut host, cx, p);
            (plain, aimed, states)
        });
        let (Value::Entity(plain), Value::Entity(aimed)) = (plain, aimed) else {
            panic!("{plain:?} {aimed:?}");
        };
        assert!(host.ents.get(plain).unwrap().broadcast);
        let es = &states[&plain.0];
        assert_eq!(es.field_i32(p, "eType"), vcod_common::net::flags::ET_FX);
        assert_eq!(es.field_i32(p, "_union.scale"), 2);
        assert_eq!(es.origin(p), [1984.0, -210.0, 512.0]);
        assert_eq!(es.field_f32(p, "origin2[2]"), 0.0);
        assert_eq!(es.field_f32(p, "angles2[0]"), 0.0);
        assert_eq!(es.field_f32(p, "angles2[1]"), 200.0);
        let es = &states[&aimed.0];
        assert_eq!(es.field_f32(p, "origin2[1]"), 0.6);
        assert_eq!(es.field_f32(p, "origin2[2]"), 0.8);
        assert_eq!(es.field_f32(p, "angles2[0]"), 1000.0);
        // 1234.5 + 0.5, truncated.
        assert_eq!(es.field_f32(p, "angles2[1]"), 1235.0);
    }

    /// `playFxOnTag(fx, ent, tag)` is a function, not a method: it interns
    /// `"%02d%s"` into the configstrings from 844 and puts the index on the
    /// entity's own ring as `EV_PLAY_FX_ON_TAG`. A bad id, a model-less
    /// entity and a quote in the tag are errors.
    #[test]
    fn playfxontag_rides_the_entitys_event_ring() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let ent = host.ents.spawn(cx).unwrap();
            let tag = Value::String(cx.intern_exact("tag_flash"));
            let args = [Value::Int(7), Value::Entity(ent), tag];
            assert!(
                play_fx_on_tag(&mut host, cx, None, &args).is_err(),
                "no model"
            );
            let model = cx.intern_folded("model");
            let name = Value::String(cx.intern_exact("xmodel/mp_bomb1"));
            host.set_field(cx, ent, model, name).unwrap();
            play_fx_on_tag(&mut host, cx, None, &args).unwrap();
            let e = host.ents.get(ent).unwrap();
            assert_eq!(
                (e.events.events[0], e.events.parms[0]),
                (EV_PLAY_FX_ON_TAG, 1)
            );
            assert_eq!(host.configstrings[845], "07tag_flash");
            let bad_id = [Value::Int(64), Value::Entity(ent), tag];
            assert!(play_fx_on_tag(&mut host, cx, None, &bad_id).is_err());
            let quoted = Value::String(cx.intern_exact("tag\"x"));
            let quoted = [Value::Int(7), Value::Entity(ent), quoted];
            assert!(play_fx_on_tag(&mut host, cx, None, &quoted).is_err());
        });
    }
}
