//! Combat builtins: the damage path's script half, the blast radius and a
//! real collision trace.

use crate::game::builtins::client::client_receiver;
use crate::game::combat::{mod_index, DFLAG_NO_KNOCKBACK, DFLAG_RADIUS, MOD_FLAGGED};
use crate::game::host::{GameHost, SimOp};
use crate::game::script::CALLBACK_SETUP;
use crate::game::temp_entity::{Scope, TempEntity};
use glam::Vec3;
use vcod_common::net::protocol::{ENTITYNUM_WORLD, PROTOCOL_V1};
use vcod_gsc::{ArrayKey, Cx, EntId, ErrorKind, Host, Target, Value};

pub type Builtin = fn(&mut GameHost, &mut Cx, Option<Target>, &[Value]) -> Result<Value, ErrorKind>;

pub const NAMES: &[(&str, Builtin)] = &[
    ("bullettrace", bullet_trace),
    ("finishplayerdamage", finish_player_damage),
    ("obituary", obituary),
    ("radiusdamage", radius_damage),
    (
        "setplayerignoreradiusdamage",
        set_player_ignore_radius_damage,
    ),
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
/// A killing hit runs `player_die` (5.1): a grenade still cooking is dropped
/// live, and the callback into
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
        drop_cooking_grenade(host, cx, slot);
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

/// The z `player_die` raises the drop's origin by (combat doc, 5.1 step 5).
const DEATH_DROP_LIFT: f32 = 40.0;
/// The speed the three draws are scaled by (combat doc, 11.3).
const DEATH_DROP_SPEED: f32 = 160.0;

/// Combat doc, 5.1 step 5: a player killed with a grenade cooking drops it
/// live from `r.currentOrigin` with z raised, on whatever fuse was left. The
/// velocity is 11.3's arithmetic over three `rand()` draws, whose negative
/// constant puts x and y in (-480, -160] and z in (-160, 0]; the skew is
/// retail's and is not a random direction.
///
/// The origin is the state the tick's moves left, the same one the corpse is
/// cloned from, and the fuse the mirror `Server::replay_moves` wrote with it.
fn drop_cooking_grenade(host: &mut GameHost, cx: &mut Cx, slot: usize) {
    let fuse = host.client_grenade_ms.get(slot).copied().unwrap_or(0);
    if fuse == 0 {
        return;
    }
    host.client_grenade_ms[slot] = 0;
    let Some(state) = host.client_entity_states[slot].as_ref() else {
        return;
    };
    let origin = Vec3::from(state.origin(&PROTOCOL_V1)) + Vec3::Z * DEATH_DROP_LIFT;
    // `self->s.weapon`: the grenade still in hand, since the drop runs ahead
    // of everything the death callback takes away.
    let weapon = host.client_weapons[slot].current;
    let weapons = host.weapons.clone();
    let Some(def) = weapons.get(weapon as usize) else {
        return;
    };
    // `rand() * -2^-31`, one draw per component in retail's order.
    let (rx, ry, rz) = (-host.rand_unit(), -host.rand_unit(), -host.rand_unit());
    // Truncated the way `fire_grenade` truncates a throw's delta (11.2);
    // `missile::throw_velocity` is where the throw's own truncation sits.
    let velocity = Vec3::new(
        DEATH_DROP_SPEED * (2.0 * rx - 1.0),
        DEATH_DROP_SPEED * (2.0 * ry - 1.0),
        DEATH_DROP_SPEED * rz,
    )
    .trunc();
    let name = def.projectile_model.clone().unwrap_or_default();
    let model = crate::configstrings::weapon_model_index(&host.configstrings, &name);
    if model == 0 && !name.is_empty() {
        log::warn!(
            "the grenade client {slot} died holding carries {name:?}, which nothing precached"
        );
    }
    let now = host.level_time_ms;
    let spawned = host.missiles.fire_grenade(
        &mut host.ents,
        cx,
        model,
        slot,
        weapon,
        origin,
        velocity,
        fuse,
        now,
    );
    if let Err(e) = spawned {
        log::warn!("the grenade client {slot} died holding was not dropped: {e:?}");
    }
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
        weapon: 0,
        origin,
        scope: Scope::Broadcast,
    });
    Ok(Value::Undefined)
}

/// The three effects a suicide has, for the two callers that need them: the
/// `suicide` builtin and `Cmd_Kill_f`. Writes the vitals, queues the sim's
/// `Damaged` op and returns the `CodeCallback_PlayerKilled` argument vector,
/// leaving only the invocation to the caller -- a builtin spawns the thread
/// so it runs before its own next instruction, while the client command has
/// nothing waiting on it and starts one.
///
/// `None` for a client that is already dead. The damage is zero and the
/// knockback off, so the sim raises `EV_DEATH` and nothing else: the dead yaw
/// stays 0, which is what the retail hit capture's own suicides read
/// (`docs/research/cod11-combat.md` section 8.4, `stats[1]` 0).
pub fn suicide_effects(host: &mut GameHost, cx: &mut Cx, slot: usize) -> Option<Vec<Value>> {
    let v = host.client_vitals.get_mut(slot)?;
    if v.dead {
        return None;
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
    drop_cooking_grenade(host, cx, slot);
    let me = Value::Entity(vcod_gsc::EntId(slot as u32));
    let weapon = host.client_weapons[slot].current as usize;
    let weapon = crate::items::item_name(weapon).unwrap_or("none");
    Some(vec![
        me,
        me,
        Value::Int(0),
        Value::String(cx.intern_exact("MOD_SUICIDE")),
        Value::String(cx.intern_exact(weapon)),
        Value::Vector([0.0; 3]),
        Value::String(cx.intern_exact("none")),
    ])
}

/// `self suicide()` (`.so` 0x45358): the kill a player asks for. Retail
/// routes it through the same `player_die` every other death takes; here the
/// health comes off the vitals directly and the death callback is spawned
/// with `MOD_SUICIDE`, the same shape `finish_player_damage` uses for a
/// killing hit.
pub fn suicide(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(args) = suicide_effects(host, cx, slot) else {
        return Ok(Value::Undefined);
    };
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

/// `radiusDamage(origin, range, maxDamage, minDamage)` (`functions[72]`,
/// `.so` 0x5eef4): the blast a script sets off, with the world as the
/// attacker and `MOD_EXPLOSIVE` as the means of death.
/// `CodeCallback_PlayerDamage` runs on every live player the falloff and the
/// line of sight reach, spawned so each runs before the calling thread's
/// next line, the way `finishPlayerDamage` starts the killed callback.
///
/// The damage itself is `crate::game::combat::radius_damage`, the same
/// `G_RadiusDamage` a grenade's blast goes through (combat doc, 14.1). What
/// this call adds is `setPlayerIgnoreRadiusDamage`'s level flag (14.2),
/// which skips every candidate with a client and so, here, every candidate
/// there is.
///
/// The position each victim is measured at is its `origin` field, the copy
/// `Server` mirrors from the sim every frame; its box and eye are the
/// standing ones, since the host does not carry a stance.
pub fn radius_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    // Retail reads its four arguments by index (`Scr_GetVector(0)` and
    // `Scr_GetFloat(1)` to `(3)`), so anything past them is ignored rather
    // than refused.
    let [Value::Vector(origin), radius, max_damage, min_damage, ..] = args else {
        return Err(ErrorKind::BadType(
            "radiusDamage takes an origin, a range, a max damage and a min damage",
        ));
    };
    let at = Vec3::from(*origin);
    let radius = as_f32(radius)?;
    let (max_damage, min_damage) = (as_f32(max_damage)?, as_f32(min_damage)?);
    if host.ignore_radius_damage {
        return Ok(Value::Undefined);
    }
    let candidates: Vec<EntId> = host
        .ents
        .iter_inuse()
        .filter(|(id, e)| e.client.is_some() && !host.client_vitals[id.0 as usize].dead)
        .map(|(id, _)| id)
        .collect();
    let origin_field = cx.intern_folded("origin");
    let mut victims = Vec::new();
    for id in candidates {
        let Value::Vector(stands) = host.get_field(cx, id, origin_field) else {
            continue;
        };
        victims.push(standing_victim(id.0 as usize, Vec3::from(stands)));
    }
    let world = host.world.clone();
    let hits = crate::game::combat::radius_damage(
        at,
        radius,
        max_damage,
        min_damage,
        None,
        None,
        "none",
        "MOD_EXPLOSIVE",
        &victims,
        world.as_deref().map(|w| &w.collision),
    );
    let (mod_, none) = (cx.intern_exact("MOD_EXPLOSIVE"), cx.intern_exact("none"));
    let callback = cx.func_ref(CALLBACK_SETUP, "CodeCallback_PlayerDamage");
    for hit in hits {
        let args = vec![
            // The world is the attacker and its own inflictor, and it reaches
            // script as `undefined`: `Scr_PlayerDamage` (0x5ca18) calls
            // `Scr_AddUndefined` for a null attacker or inflictor rather than
            // substituting an entity, and `Scr_PlayerKilled` (0x5cb30) does
            // the same. VERIFIED, the two null compares and both call sites.
            Value::Undefined,
            Value::Undefined,
            Value::Int(hit.damage),
            Value::Int(DFLAG_RADIUS),
            Value::String(mod_),
            Value::String(none),
            Value::Vector(*origin),
            Value::Vector(hit.dir),
            Value::String(none),
        ];
        cx.spawn(
            callback,
            Some(Target::Entity(EntId(hit.victim as u32))),
            args,
        );
    }
    Ok(Value::Undefined)
}

/// A client as a blast candidate, standing: the box and the eye height
/// `CanDamage` needs (combat doc, 14.3), which is all a script-side victim
/// has, its stance living on the sim rather than on the host.
fn standing_victim(slot: usize, origin: Vec3) -> crate::game::combat::BlastVictim {
    use vcod_common::pmove::{Stance, HALF_WIDTH};
    crate::game::combat::BlastVictim {
        slot,
        origin,
        mins: Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0),
        maxs: Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height()),
        eye: origin + Vec3::Z * Stance::Stand.view_height(),
    }
}

/// `setPlayerIgnoreRadiusDamage(bool)` (`functions[73]`, `.so` 0x5ef6c): one
/// flag on the level, which the `radiusDamage` builtin above is the only
/// reader of (combat doc, 14.2).
pub fn set_player_ignore_radius_damage(
    host: &mut GameHost,
    _cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(v) = args.first() else {
        return Err(ErrorKind::BadType(
            "setPlayerIgnoreRadiusDamage takes a boolean",
        ));
    };
    host.ignore_radius_damage = v.as_bool()?;
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
            inflictor: None,
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

    /// `dm.gsc`'s own shape for a death with no player behind it: the killed
    /// callback calls `isPlayer(attacker)` unguarded, the way line 492 does.
    const WORLD_BLAST: &str = r#"
        main() {}
        CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
            self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
        }
        CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {
            level.was_player = isPlayer(eAttacker);
            level.finished = 1;
        }
        mine() { radiusDamage((0,0,0), 300, 2000, 50); }
    "#;

    /// A blast a script sets off names the world as the attacker, and the
    /// world is an *entity*: retail hands `g_entities[ENTITYNUM_WORLD]` over,
    /// so `isPlayer(attacker)` answers false and the death runs on. Passing
    /// `undefined` instead aborts the thread at the `isPlayer` call, which is
    /// what `_minefields.gsc`'s `radiusDamage` did to every mine kill.
    #[test]
    fn a_world_blast_names_the_world_entity_as_the_attacker() {
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, WORLD_BLAST);
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "victim".into(),
        });
        rt.run_frame(0);
        rt.host.client_vitals[0] = Vitals {
            health: 100,
            max_health: 100,
            dead: false,
        };
        rt.set_client_origin(0, [0.0, 0.0, 0.0]);
        let victim = rt.client_entity(0).expect("the client has an entity");
        rt.start_thread_for_test(victim, "mine", 0);
        rt.run_frame(0);

        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.level_field("was_player"), Value::Int(0));
        assert_eq!(
            rt.level_field("finished"),
            Value::Int(1),
            "the callback has to run past the isPlayer call, not abort on it"
        );
        assert!(rt.client_vitals(0).dead, "2000 damage at zero range kills");
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

    /// `radiusDamage` runs `CodeCallback_PlayerDamage` on every live client
    /// inside the radius, inline, before the calling thread's next line:
    /// the near client takes the falloff's damage and dies of it, the one
    /// 1000 units out takes nothing, and the flags the script is handed
    /// carry `DFLAG_RADIUS`. The attacker is the world, which reaches the
    /// callback as `undefined`.
    ///
    /// The second blast is the dead check: a corpse is not damaged again,
    /// so the near client's callback ran once.
    #[test]
    fn radiusdamage_damages_every_live_client_inside_the_radius() {
        const SCRIPT: &str = r#"
            main() {
                wait 1;
                radiusDamage((0, 0, 0), 300, 2000, 50);
                wait 1;
                radiusDamage((0, 0, 0), 300, 2000, 50);
            }
            CodeCallback_PlayerConnect() {}
            CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
                if (!isdefined(self.hits))
                    self.hits = 0;
                self.hits = self.hits + 1;
                self.took = iDamage;
                self.flags = iDFlags;
                self.mod = sMeansOfDeath;
                self.killer = isdefined(eAttacker);
                self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
            }
            CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "victim".into(),
        });
        rt.push_client_event(ClientEvent::Connect {
            slot: 1,
            name: "killer".into(),
        });
        rt.run_frame(50);
        for slot in [0, 1] {
            rt.host.client_vitals[slot] = Vitals {
                health: 100,
                max_health: 100,
                dead: false,
            };
        }
        rt.set_client_origin(0, [100.0, 0.0, 0.0]);
        rt.set_client_origin(1, [1000.0, 0.0, 0.0]);
        rt.run_frame(1100);
        rt.run_frame(2200);

        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        // 2000 - (2000 - 50) * 100/300, the linear falloff.
        assert_eq!(rt.client_field(0, "took").as_deref(), Some("1350"));
        assert_eq!(rt.client_field(0, "flags").as_deref(), Some("1"));
        assert_eq!(rt.client_field(0, "mod").as_deref(), Some("MOD_EXPLOSIVE"));
        assert_eq!(
            rt.client_field(0, "killer").as_deref(),
            Some("0"),
            "the world set this blast off, so the callback's attacker is undefined"
        );
        assert_eq!(rt.client_field(0, "hits").as_deref(), Some("1"));
        assert_eq!(rt.client_vitals(0).health, 0);
        assert!(rt.client_vitals(0).dead);

        assert_eq!(
            rt.client_field(1, "took").as_deref(),
            Some("Undefined"),
            "1000 units out, past the radius"
        );
        assert_eq!(rt.client_vitals(1).health, 100);
        assert!(!rt.client_vitals(1).dead);
    }

    /// `setPlayerIgnoreRadiusDamage` (combat doc, 14.2) is a level flag the
    /// `radiusDamage` builtin alone reads: with it set the same blast that
    /// killed the near client above damages nobody, and clearing it lets the
    /// next one through.
    #[test]
    fn an_ignoring_level_takes_no_client_damage() {
        const SCRIPT: &str = r#"
            main() {
                wait 1;
                setPlayerIgnoreRadiusDamage(true);
                radiusDamage((0, 0, 0), 300, 2000, 50);
                wait 1;
                setPlayerIgnoreRadiusDamage(false);
                radiusDamage((0, 0, 0), 300, 2000, 50);
            }
            CodeCallback_PlayerConnect() {}
            CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
                self.took = iDamage;
                self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
            }
            CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "victim".into(),
        });
        rt.run_frame(50);
        rt.host.client_vitals[0] = Vitals {
            health: 100,
            max_health: 100,
            dead: false,
        };
        rt.set_client_origin(0, [100.0, 0.0, 0.0]);

        rt.run_frame(1100);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(
            rt.client_field(0, "took").as_deref(),
            Some("Undefined"),
            "the flag skipped every candidate with a client"
        );
        assert_eq!(rt.client_vitals(0).health, 100);

        rt.run_frame(2200);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "took").as_deref(), Some("1350"));
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
            let victim = host.ents.spawn_client(cx, 3, None).unwrap();
            let attacker = host.ents.spawn_client(cx, 5, None).unwrap();
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
            let victim = host.ents.spawn_client(cx, 1, None).unwrap();
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
        ) = ops[0]
        else {
            panic!("a killing hit queues a Damaged op");
        };
        assert_eq!((slot, damage, knockback, fatal), (0, 0, false, true));
    }
}
