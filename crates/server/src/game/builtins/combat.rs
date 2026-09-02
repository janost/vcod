//! Combat builtins: real damage queuing and a real collision trace, both
//! global calls (no receiver).

use crate::game::combat::{mod_index, MOD_FLAGGED};
use crate::game::damage::DamageEvent;
use crate::game::host::GameHost;
use crate::game::temp_entity::{Scope, TempEntity};
use glam::Vec3;
use vcod_common::net::protocol::ENTITYNUM_WORLD;
use vcod_gsc::{ArrayKey, Cx, ErrorKind, Host, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("bullettrace", bullet_trace),
    ("obituary", obituary),
    ("radiusdamage", radius_damage),
];

/// `EV_OBITUARY`, the killfeed event
/// (`docs/research/cod11-events-and-fx.md` section 1).
const EV_OBITUARY: i32 = 201;

/// `obituary(victim, attacker, weapon, meansOfDeath)` (`.so` 0x5a750): one
/// broadcast temp entity, encoded as `docs/research/cod11-hud-protocol.md`
/// sections 1 and 2 read it off the builtin.
pub fn obituary(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (
        Some(Value::Entity(victim)),
        Some(attacker),
        Some(Value::String(weapon)),
        Some(Value::String(means)),
    ) = (args.first(), args.get(1), args.get(2), args.get(3))
    else {
        return Err(ErrorKind::BadType(
            "obituary takes a victim, an attacker, a weapon name and a means of death",
        ));
    };
    let (victim, weapon, means) = (*victim, *weapon, *means);
    // Only a player entity is named as the attacker; anything else is the
    // world, which is the branch retail takes on the script type.
    let attacker = match attacker {
        Value::Entity(a) if host.ents.get(*a).is_some_and(|e| e.client.is_some()) => a.0 as i32,
        _ => ENTITYNUM_WORLD as i32,
    };
    let parm = match mod_index(cx.resolve(means)) {
        Some(m) if MOD_FLAGGED.contains(&m) => 0x80 | m,
        _ => crate::configstrings::weapon_index(cx.resolve(weapon)).map_or(0, |i| i as i32),
    };
    let origin_atom = cx.intern_folded("origin");
    let origin = match host.get_field(cx, victim, origin_atom) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    };
    host.temp_entities.push(TempEntity {
        event: EV_OBITUARY,
        parm,
        surf_type: 0,
        other: victim.0,
        attacker,
        origin,
        scope: Scope::Broadcast,
    });
    Ok(Value::Undefined)
}

pub fn lookup(folded: &str) -> Option<Builtin> {
    NAMES.iter().find(|(n, _)| *n == folded).map(|(_, f)| *f)
}

fn as_f32(v: &Value) -> Result<f32, ErrorKind> {
    match v {
        Value::Int(i) => Ok(*i as f32),
        Value::Float(f) => Ok(*f),
        _ => Err(ErrorKind::BadType("expected a number")),
    }
}

/// `radiusDamage(origin, radius, maxDamage, minDamage)`. A builtin must
/// never reenter the VM, so this queues a `DamageEvent` rather than calling
/// `CodeCallback_PlayerDamage` inline; stage 6 drains the queue.
pub fn radius_damage(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let [Value::Vector(origin), radius, max_damage, min_damage] = args else {
        return Err(ErrorKind::BadType(
            "radiusDamage takes an origin, radius, max damage and min damage",
        ));
    };
    let radius = as_f32(radius)?;
    let max_damage = as_f32(max_damage)?;
    let min_damage = as_f32(min_damage)?;
    host.damage.push(DamageEvent {
        origin: *origin,
        radius,
        max_damage,
        min_damage,
        attacker: None,
    });
    Ok(Value::Undefined)
}

/// `bulletTrace(start, end, hitCharacters, ignoreEnt)`, the corpus's own
/// arity (`bulletTrace(loc, (loc-(0,0,5000)), false, undefined)`,
/// `bullettrace(nGunPos, nPlayerPos, 1, eMG42)`). `hitCharacters` and
/// `ignoreEnt` are accepted for the right shape but not acted on:
/// character hits need entity bounds (stage 5) and excluding `ignoreEnt`
/// needs the trace to carry entity identity, neither of which exists yet.
///
/// The result is a `Value::Array`, not a struct: `LoadIndex`/`StoreIndex`
/// (`vcod_gsc::interp`) are what the corpus indexes a bullet trace result
/// with (`["position"]` 31 call sites, `["fraction"]` 13, `["entity"]` 8,
/// `["surfacetype"]` 3 in the extracted corpus), and array keys intern
/// exactly, not folded, matching how any other string index does.
pub fn bullet_trace(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (
        Some(Value::Vector(from)),
        Some(Value::Vector(to)),
        Some(_hit_characters),
        Some(_ignore_ent),
    ) = (args.first(), args.get(1), args.get(2), args.get(3))
    else {
        return Err(ErrorKind::BadType(
            "bulletTrace takes a start, an end, hitCharacters and ignoreEnt",
        ));
    };
    let (from, to) = (*from, *to);

    let arr = cx.new_array();
    let position = ArrayKey::Str(cx.intern_exact("position"));
    let fraction = ArrayKey::Str(cx.intern_exact("fraction"));
    let entity = ArrayKey::Str(cx.intern_exact("entity"));
    let surfacetype = ArrayKey::Str(cx.intern_exact("surfacetype"));

    match &host.world {
        Some(world) => {
            let start = Vec3::new(from[0], from[1], from[2]);
            let end = Vec3::new(to[0], to[1], to[2]);
            let t = world.collision.shot_trace(start, end);
            cx.set_index(arr, fraction, Value::Float(t.fraction));
            cx.set_index(
                arr,
                position,
                Value::Vector([t.endpos.x, t.endpos.y, t.endpos.z]),
            );
        }
        None => {
            cx.set_index(arr, fraction, Value::Float(1.0));
            cx.set_index(arr, position, Value::Vector(to));
        }
    }
    // `entity` needs entity bounds to resolve which gentity the trace
    // stopped on (stage 5); `surfacetype` needs the surface-name table
    // retail derives from `surface_flags`, which nothing here maps yet.
    // Both stay undefined rather than guessing a value.
    cx.set_index(arr, entity, Value::Undefined);
    cx.set_index(arr, surfacetype, Value::Undefined);
    Ok(Value::Array(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;
    use crate::world::World;
    use std::rc::Rc;

    /// A builtin must never reenter the VM, so `radiusDamage` queues an
    /// event the server drains after `run_frame` rather than calling
    /// `CodeCallback_PlayerDamage` inline. That is a deliberate divergence
    /// and it is observable: a script that damages and then reads
    /// `self.health` sees the pre-callback value.
    #[test]
    fn radiusdamage_queues_rather_than_calling_back() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let at = Value::Vector([0.0, 0.0, 0.0]);
            radius_damage(
                &mut host,
                cx,
                None,
                &[at, Value::Int(300), Value::Int(2000), Value::Int(50)],
            )
            .unwrap();
            assert_eq!(host.damage.len(), 1);
            assert_eq!(host.damage[0].radius, 300.0);
            assert_eq!(host.damage[0].max_damage, 2000.0);
        });
    }

    /// `bulletTrace` runs a real trace against the collision world when the
    /// server has one, and reports a clean miss when it does not: there is
    /// no map in a unit test, so `fraction` is 1 and `position` is the end
    /// point. The result is indexed as an array, matching how the corpus
    /// reads it back.
    #[test]
    fn bullettrace_with_no_world_reports_a_clean_miss() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let from = Value::Vector([0.0, 0.0, 0.0]);
            let to = Value::Vector([100.0, 0.0, 0.0]);
            let args = [from, to, Value::Int(0), Value::Undefined];
            let Value::Array(arr) = bullet_trace(&mut host, cx, None, &args).unwrap() else {
                panic!()
            };
            let f = ArrayKey::Str(cx.intern_exact("fraction"));
            assert_eq!(cx.get_index(arr, f), Value::Float(1.0));
            let p = ArrayKey::Str(cx.intern_exact("position"));
            assert_eq!(cx.get_index(arr, p), Value::Vector([100.0, 0.0, 0.0]));
        });
    }

    /// With a real collision world, a trace straight down through the test
    /// floor (`vcod_common::collision::test_world`, top at z=0) stops short
    /// of the end point: `fraction < 1`.
    #[test]
    fn bullettrace_with_a_world_hits_real_geometry() {
        let (mut vm, mut host) = fixture();
        host.world = Some(Rc::new(World {
            collision: vcod_common::collision::test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        }));
        vm.with_cx(|cx| {
            let from = Value::Vector([0.0, 0.0, 100.0]);
            let to = Value::Vector([0.0, 0.0, -100.0]);
            let args = [from, to, Value::Int(0), Value::Undefined];
            let Value::Array(arr) = bullet_trace(&mut host, cx, None, &args).unwrap() else {
                panic!()
            };
            let f = ArrayKey::Str(cx.intern_exact("fraction"));
            let Value::Float(fraction) = cx.get_index(arr, f) else {
                panic!()
            };
            assert!(fraction < 1.0, "expected a hit, got fraction {fraction}");
        });
    }

    /// The killfeed's two namespaces: an ordinary means of death sends the
    /// weapon's configstring 7 index, a flagged one sends `0x80 | mod`.
    #[test]
    fn obituary_encodes_a_weapon_index_or_a_flagged_mod() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let victim = host.ents.spawn_client(cx, 3).unwrap();
            let attacker = host.ents.spawn_client(cx, 5).unwrap();
            let origin = cx.intern_folded("origin");
            host.set_field(cx, victim, origin, Value::Vector([10.0, 20.0, 30.0]))
                .unwrap();
            let weapon = Value::String(cx.intern_exact("m1carbine_mp"));
            let args = |m: &str, cx: &mut Cx| {
                [
                    Value::Entity(victim),
                    Value::Entity(attacker),
                    weapon,
                    Value::String(cx.intern_exact(m)),
                ]
            };

            let a = args("MOD_RIFLE_BULLET", cx);
            obituary(&mut host, cx, None, &a).unwrap();
            let te = host.temp_entities.last().unwrap();
            assert_eq!(te.event, EV_OBITUARY);
            assert_eq!(te.parm, 12, "m1carbine_mp is configstring 7's index 12");
            assert_eq!(te.other, 3);
            assert_eq!(te.attacker, 5);
            assert_eq!(te.origin, [10.0, 20.0, 30.0]);
            assert_eq!(te.scope, Scope::Broadcast);

            let a = args("MOD_HEAD_SHOT", cx);
            obituary(&mut host, cx, None, &a).unwrap();
            assert_eq!(host.temp_entities.last().unwrap().parm, 0x88);

            let a = args("MOD_SUICIDE", cx);
            obituary(&mut host, cx, None, &a).unwrap();
            assert_eq!(host.temp_entities.last().unwrap().parm, 0x96);
        });
    }

    /// An attacker that is not a player is `ENTITYNUM_WORLD`: the client
    /// draws the victim alone. Retail takes the same branch for anything
    /// whose script type is not a player entity.
    #[test]
    fn a_non_player_attacker_is_entitynum_world() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let victim = host.ents.spawn_client(cx, 1).unwrap();
            let weapon = Value::String(cx.intern_exact("m1carbine_mp"));
            let m = Value::String(cx.intern_exact("MOD_FALLING"));
            let args = [Value::Entity(victim), Value::Undefined, weapon, m];
            obituary(&mut host, cx, None, &args).unwrap();
            let te = host.temp_entities.last().unwrap();
            assert_eq!(te.attacker, 1022);
            assert_eq!(te.parm, 0x95);
        });
    }
}
