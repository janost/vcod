//! Combat builtins: the damage path's script half, real damage queuing and
//! a real collision trace.

use crate::game::builtins::client::client_receiver;
use crate::game::combat::{mod_index, DFLAG_NO_KNOCKBACK, MOD_FLAGGED};
use crate::game::damage::DamageEvent;
use crate::game::host::{GameHost, SimOp};
use crate::game::script::CALLBACK_SETUP;
use crate::game::temp_entity::{Scope, TempEntity};
use glam::Vec3;
use vcod_common::net::protocol::ENTITYNUM_WORLD;
use vcod_gsc::{ArrayKey, Cx, ErrorKind, Host, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("bullettrace", bullet_trace),
    ("finishplayerdamage", finish_player_damage),
    ("obituary", obituary),
    ("radiusdamage", radius_damage),
    ("suicide", suicide),
];

/// `EV_OBITUARY`, the killfeed event
/// (`docs/research/cod11-events-and-fx.md` section 1).
const EV_OBITUARY: i32 = 201;

/// `self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags,
/// sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc)` (`.so` 0x4376c), where a
/// player's damage lands (combat doc, section 4.5): the health comes off the
/// host's vitals here, and everything the sim does with it -- knockback,
/// the feedback fields, `EV_PAIN` or `EV_DEATH` -- goes out as one `SimOp`.
/// A killing hit runs `player_die` (5.1): the callback into
/// `CodeCallback_PlayerKilled` is spawned so it runs before this builtin's
/// caller continues, which is what lets the stock damage callback read
/// `self.sessionstate` on its next line and find it `"dead"`.
///
/// Retail also raises the flesh impact events here; `bullet_fire` raises
/// them with the shot instead, so a hit the script refuses (friendly fire
/// off) still shows an impact. The 250 clamp and `pm_time` of 4.5's
/// knockback are the sim's.
pub fn finish_player_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let [inflictor, attacker, damage, dflags, mod_, weapon, point, dir, hitloc] = args else {
        return Err(ErrorKind::BadType(
            "finishPlayerDamage takes nine arguments",
        ));
    };
    let damage = as_i32(damage)?;
    let dflags = as_i32(dflags)?;
    // `iDamage <= 0` returns without doing anything (4.5).
    if damage <= 0 {
        return Ok(Value::Undefined);
    }
    let attacker_slot = match attacker {
        Value::Entity(a) if host.ents.get(*a).is_some_and(|e| e.client.is_some()) => {
            Some(a.0 as usize)
        }
        _ => None,
    };
    let attacker_origin = match attacker {
        Value::Entity(a) => {
            let origin = cx.intern_folded("origin");
            match host.get_field(cx, *a, origin) {
                Value::Vector(v) => Some(v),
                _ => None,
            }
        }
        _ => None,
    };
    let v = &mut host.client_vitals[slot];
    if v.dead {
        return Ok(Value::Undefined);
    }
    v.health -= damage;
    let fatal = v.health <= 0;
    if fatal {
        v.health = 0;
        v.dead = true;
    }
    let dir = match dir {
        Value::Vector(d) => Vec3::from(*d).normalize_or_zero().into(),
        _ => [0.0; 3],
    };
    host.client_sim_ops.push((
        slot,
        SimOp::Damaged {
            damage,
            point: match point {
                Value::Vector(p) => *p,
                _ => [0.0; 3],
            },
            dir,
            knockback: dflags & DFLAG_NO_KNOCKBACK == 0,
            attacker: attacker_slot,
            attacker_origin,
            fatal,
        },
    ));
    if fatal {
        let killed = cx.func_ref(CALLBACK_SETUP, "CodeCallback_PlayerKilled");
        cx.spawn(
            killed,
            recv,
            vec![
                *inflictor,
                *attacker,
                Value::Int(damage),
                *mod_,
                *weapon,
                Value::Vector(dir),
                *hitloc,
            ],
        );
    }
    Ok(Value::Undefined)
}

fn as_i32(v: &Value) -> Result<i32, ErrorKind> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Float(f) => Ok(*f as i32),
        _ => Err(ErrorKind::BadType("expected a number")),
    }
}

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

/// `self suicide()` (`.so` 0x45358): the kill a player asks for. Retail
/// routes it through the same `player_die` every other death takes; here the
/// health comes off the vitals directly and the death callback is spawned
/// with `MOD_SUICIDE`, the same shape `finish_player_damage` uses for a
/// killing hit.
///
/// The damage is zero and the knockback off, so the sim raises `EV_DEATH`
/// and nothing else: the dead yaw stays 0, which is what the retail hit
/// capture's own suicides read (`docs/research/cod11-combat.md` section 8.4,
/// `stats[1]` 0).
pub fn suicide(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let v = &mut host.client_vitals[slot];
    if v.dead {
        return Ok(Value::Undefined);
    }
    v.health = 0;
    v.dead = true;
    host.client_sim_ops.push((
        slot,
        SimOp::Damaged {
            damage: 0,
            point: [0.0; 3],
            dir: [0.0; 3],
            knockback: false,
            attacker: Some(slot),
            attacker_origin: None,
            fatal: true,
        },
    ));
    let me = Value::Entity(vcod_gsc::EntId(slot as u32));
    let weapon = host.client_weapons[slot].current as usize;
    let weapon = crate::items::item_name(weapon).unwrap_or("none");
    let args = vec![
        me,
        me,
        Value::Int(0),
        Value::String(cx.intern_exact("MOD_SUICIDE")),
        Value::String(cx.intern_exact(weapon)),
        Value::Vector([0.0; 3]),
        Value::String(cx.intern_exact("none")),
    ];
    let killed = cx.func_ref(CALLBACK_SETUP, "CodeCallback_PlayerKilled");
    cx.spawn(killed, recv, args);
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
    use crate::game::combat::Hit;
    use crate::game::host::{ClientEvent, Vitals};
    use crate::game::script::ScriptRuntime;
    use crate::game::testing::fixture;
    use crate::world::World;
    use std::rc::Rc;

    const CALLBACKS: &str = r#"
        main() {}
        CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
            self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
            self.seen = self.sessionstate;
            self.left = self.health;
        }
        CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {
            self.sessionstate = "dead";
            self.mod = sMeansOfDeath;
            self.killer = eAttacker getEntityNumber();
            wait 2;
            self.sessionstate = "buried";
        }
    "#;

    fn hit(damage: i32) -> Hit {
        Hit {
            victim: 0,
            attacker: 1,
            damage,
            dflags: 0,
            mod_: "MOD_RIFLE_BULLET",
            weapon: "m1carbine_mp".into(),
            point: [0.0; 3],
            dir: [1.0, 0.0, 0.0],
            hitloc: "torso_upper",
        }
    }

    fn two_clients() -> ScriptRuntime {
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, CALLBACKS);
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "victim".into(),
        });
        rt.push_client_event(ClientEvent::Connect {
            slot: 1,
            name: "killer".into(),
        });
        rt.run_frame(0);
        rt
    }

    /// A killing `finishPlayerDamage` starts `CodeCallback_PlayerKilled`
    /// before it returns, so a script reading `self.sessionstate` on the
    /// next line sees what the callback wrote. Whole path through the VM:
    /// the test script defines the two callbacks, the host delivers one hit.
    #[test]
    fn a_fatal_finishplayerdamage_runs_the_killed_callback_first() {
        let mut rt = two_clients();
        rt.host.client_vitals[0] = Vitals {
            health: 10,
            max_health: 100,
            dead: false,
        };
        rt.deliver_hits(vec![hit(45)], 50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "seen").as_deref(), Some("dead"));
        assert_eq!(
            rt.client_field(0, "mod").as_deref(),
            Some("MOD_RIFLE_BULLET")
        );
        assert_eq!(rt.client_field(0, "killer").as_deref(), Some("1"));
        assert_eq!(rt.client_field(0, "left").as_deref(), Some("0"));
        assert_eq!(rt.client_vitals(0).health, 0);
        assert!(rt.client_vitals(0).dead);
        let ops = rt.take_sim_ops();
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            ops[0],
            (
                0,
                SimOp::Damaged {
                    fatal: true,
                    damage: 45,
                    attacker: Some(1),
                    knockback: true,
                    ..
                }
            )
        ));
        // Drained: nothing is applied twice.
        assert!(rt.take_sim_ops().is_empty());
    }

    /// A surviving hit takes the health off, queues its op and starts no
    /// killed callback; a second hit on a dead player does nothing at all.
    #[test]
    fn a_surviving_hit_takes_health_and_a_dead_player_takes_nothing() {
        let mut rt = two_clients();
        rt.host.client_vitals[0] = Vitals {
            health: 100,
            max_health: 100,
            dead: false,
        };
        rt.deliver_hits(vec![hit(67)], 50);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_vitals(0).health, 33);
        assert!(!rt.client_vitals(0).dead);
        assert_eq!(rt.client_field(0, "left").as_deref(), Some("33"));
        assert_eq!(
            rt.client_field(0, "mod").as_deref(),
            Some("Undefined"),
            "no killed callback ran"
        );
        let ops = rt.take_sim_ops();
        assert!(matches!(ops[0].1, SimOp::Damaged { fatal: false, .. }));

        rt.deliver_hits(vec![hit(67)], 100);
        assert!(rt.client_vitals(0).dead);
        rt.deliver_hits(vec![hit(67)], 150);
        assert_eq!(
            rt.take_sim_ops().len(),
            1,
            "the dead player took no second op"
        );
    }

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

    /// `suicide` is a death with no attacker but the player itself: the
    /// vitals go to zero, the sim gets a fatal `Damaged` with no damage and
    /// no knockback, and `CodeCallback_PlayerKilled` runs before the calling
    /// thread continues -- the same ordering a fatal `finishPlayerDamage`
    /// gets. `MOD_SUICIDE` and the held weapon are what reach the callback.
    #[test]
    fn suicide_kills_the_player_and_runs_the_killed_callback() {
        const SCRIPT: &str = r#"
            main() {}
            CodeCallback_PlayerConnect() {
                self giveWeapon("m1carbine_mp");
                self setSpawnWeapon("m1carbine_mp");
                self suicide();
                self.after = self.sessionstate;
            }
            CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {
                self.sessionstate = "dead";
                self.mod = sMeansOfDeath;
                self.weap = sWeapon;
                self.killer = eAttacker getEntityNumber();
            }
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        rt.host.client_vitals[0] = Vitals {
            health: 100,
            max_health: 100,
            dead: false,
        };
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "victim".into(),
        });
        rt.run_frame(0);

        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "after").as_deref(), Some("dead"));
        assert_eq!(rt.client_field(0, "mod").as_deref(), Some("MOD_SUICIDE"));
        assert_eq!(rt.client_field(0, "weap").as_deref(), Some("m1carbine_mp"));
        assert_eq!(rt.client_field(0, "killer").as_deref(), Some("0"));
        assert_eq!(rt.client_vitals(0).health, 0);
        assert!(rt.client_vitals(0).dead);
        let ops = rt.take_sim_ops();
        assert_eq!(ops.len(), 1);
        let (
            slot,
            SimOp::Damaged {
                damage,
                knockback,
                fatal,
                ..
            },
        ) = ops[0];
        assert_eq!((slot, damage, knockback, fatal), (0, 0, false, true));
    }
}
