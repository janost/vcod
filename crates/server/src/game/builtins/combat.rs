//! Combat builtins: the damage path's script half, the blast radius and a
//! real collision trace.

use crate::game::builtins::client::client_receiver;
use crate::game::combat::{DFLAG_NO_KNOCKBACK, DFLAG_RADIUS, MOD_FLAGGED, mod_index};
use crate::game::host::{GameHost, SimOp};
use crate::game::script::CALLBACK_SETUP;
use crate::game::temp_entity::{Scope, TempEntity};
use glam::Vec3;
use vcod_common::net::protocol::ENTITYNUM_WORLD;
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
/// `self.sessionstate` on its next line and find it `"dead"`. The follower
/// walk runs when that callback returns ([`GameHost::player_die_walk`]).
///
/// A bullet weapon's hit raises its two flesh impacts here and nowhere else,
/// so a hit the script refuses (friendly fire off) shows none. The 250 clamp
/// and `pm_time` of 4.5's knockback are the sim's.
pub fn finish_player_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let [
        inflictor,
        attacker,
        damage,
        dflags,
        mod_,
        weapon,
        point,
        dir,
        hitloc,
    ] = args
    else {
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
    let dir = match dir {
        Value::Vector(d) => Vec3::from(*d).normalize_or_zero().into(),
        _ => [0.0; 3],
    };
    let point = match point {
        Value::Vector(p) => *p,
        _ => [0.0; 3],
    };
    let bullet = match weapon {
        Value::String(w) => crate::configstrings::weapon_index(cx.resolve(*w))
            .and_then(|i| host.weapons.get(i))
            .filter(|d| d.weapon_type == "bullet")
            .map(|d| d.sounds.rifle_bullet),
        _ => None,
    };
    if let Some(rifle_bullet) = bullet {
        // No attacker entity reads as `g_entities[ENTITYNUM_WORLD]` (4.5).
        let other = match attacker {
            Value::Entity(a) => a.0,
            _ => ENTITYNUM_WORLD,
        };
        let pair = crate::game::combat::flesh_impacts(point, dir, rifle_bullet, other, slot);
        host.temp_entities.extend(pair);
    }
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
    // A client `player_die` already ran on (a blast that reaches it before
    // its end frame, 14.5) loses health as any other, clamped at -999
    // (0x43c5c), and dies no second time: `player_die` cleared `die`.
    let fatal = !v.dead && v.health - damage <= 0;
    v.health = (v.health - damage).max(-999);
    if fatal {
        host.die(slot);
    }
    host.client_sim_ops.push((
        slot,
        SimOp::Damaged {
            damage,
            point,
            dir,
            knockback: dflags & DFLAG_NO_KNOCKBACK == 0,
            attacker: attacker_slot,
            attacker_origin,
            fatal,
        },
    ));
    if fatal {
        drop_cooking_grenade(host, cx, slot);
        let weapon = gun_credit(host, cx, attacker_slot, *weapon).unwrap_or(*weapon);
        // Both start as `g_entities[ENTITYNUM_WORLD]` (0x43778) and only an
        // entity argument replaces them, so a fall's death names the world
        // twice (player-clip doc 8.10). An entity inflictor is replaced by
        // the attacker argument, `Scr_GetEntity(1)` (0x43827), so a
        // grenade's kill names the thrower twice (combat doc 4.4). What
        // retail does with an entity inflictor and no attacker entity is
        // not read; the world stands in.
        let world = Value::Entity(host.ents.world(cx));
        let attacker = match attacker {
            Value::Entity(_) => *attacker,
            _ => world,
        };
        let inflictor = match inflictor {
            Value::Entity(_) => attacker,
            _ => world,
        };
        let killed = cx.func_ref(CALLBACK_SETUP, "CodeCallback_PlayerKilled");
        cx.spawn_then(
            killed,
            recv,
            vec![
                inflictor,
                attacker,
                Value::Int(damage),
                *mod_,
                weapon,
                Value::Vector(dir),
                *hitloc,
            ],
            slot as u32,
        );
    }
    Ok(Value::Undefined)
}

/// `player_die`'s kill credit (turrets doc 11): a killer on a gun has the
/// kill put on the gun's weapon, provided the weapon he was credited with
/// names one at all.
fn gun_credit(
    host: &GameHost,
    cx: &mut Cx,
    attacker: Option<usize>,
    weapon: Value,
) -> Option<Value> {
    let Value::String(name) = weapon else {
        return None;
    };
    crate::configstrings::weapon_index(cx.resolve(name)).filter(|&i| i != 0)?;
    let attacker = attacker?;
    let rec = host.turrets.values().find(|r| r.owner == Some(attacker))?;
    Some(Value::String(cx.intern_exact(&rec.weapon)))
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
/// The origin is the player's `origin` field, `r.currentOrigin`: snapped
/// inside the dying player's own cmd, the unsnapped `ps.origin` everywhere
/// else (5.5). The fuse is the one `Server::replay_moves` mirrored.
fn drop_cooking_grenade(host: &mut GameHost, cx: &mut Cx, slot: usize) {
    let fuse = host.client_grenade_ms.get(slot).copied().unwrap_or(0);
    if fuse == 0 {
        return;
    }
    host.client_grenade_ms[slot] = 0;
    let Some(player) = host.ents.handle(slot as u32) else {
        return;
    };
    let field = cx.intern_folded("origin");
    let Value::Vector(at) = host.get_field(cx, player, field) else {
        return;
    };
    let origin = Vec3::from(at) + Vec3::Z * DEATH_DROP_LIFT;
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
        client_num: 0,
        scale: 0,
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
/// `None` for a client that is already dead. The sim's damage is zero and the
/// knockback off, so the sim raises `EV_DEATH` and nothing else: the dead yaw
/// stays 0, which is what the retail hit capture's own suicides read
/// (`docs/research/cod11-combat.md` section 8.4, `stats[1]` 0). The callback
/// gets what both retail callers hand `player_die`: damage 100000, weapon 0
/// (`"none"`), no direction and hit location 0 (5.1).
pub fn suicide_effects(host: &mut GameHost, cx: &mut Cx, slot: usize) -> Option<Vec<Value>> {
    if host.client_vitals.get(slot)?.dead {
        return None;
    }
    host.die(slot);
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
    let me = Value::Entity(host.ents.handle(slot as u32)?);
    let none = Value::String(cx.intern_exact("none"));
    Some(vec![
        me,
        me,
        Value::Int(100000),
        Value::String(cx.intern_exact("MOD_SUICIDE")),
        none,
        Value::Undefined,
        none,
    ])
}

/// `self suicide()` (`.so` 0x45358): the kill a player asks for. Retail
/// routes it through the same `player_die` every other death takes; here the
/// health comes off the vitals directly and the death callback is spawned
/// with `MOD_SUICIDE`, the same shape `finish_player_damage` uses for a
/// killing hit, follower walk included.
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
    cx.spawn_then(killed, recv, args, slot as u32);
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
///
/// The damage itself is [`crate::game::combat::Blast`], the same
/// `G_RadiusDamage` a grenade's blast goes through (combat doc, 14.1). The
/// builtin copies `setPlayerIgnoreRadiusDamage`'s flag into the word the
/// walk tests and zeroes that word when the walk ends (14.2); the flag
/// itself stays set.
///
/// The walk is retail's (14.5): the candidates are the client entities in
/// the blast's box when it goes off, each is tested for `takedamage` and
/// measured when its turn comes, and its `CodeCallback_PlayerDamage` runs to
/// its first `wait` before the next candidate is measured. Each callback is
/// a `spawn_then` whose return ([`blast_step`]) takes the next turn, so the
/// whole walk still ends before the calling thread's next line.
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
    let blast = crate::game::combat::Blast::new(
        Vec3::from(*origin),
        as_f32(radius)?,
        as_f32(max_damage)?,
        as_f32(min_damage)?,
        None,
        None,
        "none",
        "MOD_EXPLOSIVE",
    );
    let candidates = blast_candidates(host, cx, &blast);
    host.blast_noises.push(*origin);
    host.radius_ignore_active = host.ignore_radius_damage;
    host.blasts.push(ScriptBlast { blast, candidates });
    blast_step(host, cx);
    Ok(Value::Undefined)
}

/// `trap_EntitiesInBox` over the blast's box, in the area tree's order
/// (combat doc 14.7), kept to what the walk can damage: the clients and the
/// turrets.
fn blast_candidates(
    host: &mut GameHost,
    cx: &mut Cx,
    blast: &crate::game::combat::Blast,
) -> Vec<EntId> {
    let (mins, maxs) = blast.search_box();
    let turrets: Vec<EntId> = host.blast_entities(cx).into_iter().map(|v| v.id).collect();
    host.area
        .entities_in_box(mins, maxs, -1)
        .into_iter()
        .filter_map(|n| host.ents.handle(n))
        .filter(|id| host.ents.get(*id).is_some_and(|e| e.client.is_some()) || turrets.contains(id))
        .collect()
}

/// One `radiusDamage` walk: the blast and the candidates whose turn has not
/// come yet, in walk order.
pub struct ScriptBlast {
    blast: crate::game::combat::Blast,
    candidates: Vec<EntId>,
}

/// The `spawn_then` token of a blast victim's damage callback. Every other
/// token is a slot (`GameHost::spawn_returned`).
pub const BLAST_TOKEN: u32 = u32::MAX;

/// The innermost walk's next turns, up to the first victim the blast
/// reaches, whose damage callback is queued to run before the walk goes on;
/// or, with no candidate left, the walk's end.
pub fn blast_step(host: &mut GameHost, cx: &mut Cx) {
    let origin_field = cx.intern_folded("origin");
    loop {
        let Some(walk) = host.blasts.last_mut() else {
            return;
        };
        if walk.candidates.is_empty() {
            host.blasts.pop();
            host.radius_ignore_active = false;
            return;
        }
        let id = walk.candidates.remove(0);
        let slot = id.0 as usize;
        let Some(ent) = host.ents.get(id) else {
            continue;
        };
        if ent.client.is_none() {
            blast_entity(host, cx, id);
            continue;
        }
        if !host.client_vitals[slot].takedamage || host.radius_ignore_active {
            continue;
        }
        let Value::Vector(stands) = host.get_field(cx, id, origin_field) else {
            continue;
        };
        let link = Vec3::from(host.client_link_origin[slot]);
        let victim = standing_victim(slot, Vec3::from(stands), link);
        let bodies = blast_bodies(host, cx);
        let world = host.world.clone();
        let models = host.placed_script_models(cx);
        let (fs, anims) = (host.fs.clone(), host.anims.clone());
        let mut bones = match (fs.as_deref(), anims.as_deref()) {
            (Some(fs), Some(anims)) => Some(crate::game::combat::BoneTraceCtx {
                fs,
                anims,
                rigs: &mut host.hit_rigs,
                now_ms: host.level_time_ms,
            }),
            _ => None,
        };
        let walk = host.blasts.last().expect("the walk this turn came from");
        let Some(hit) = walk.blast.hit(
            &victim,
            world.as_deref().map(|w| &w.collision),
            &models,
            &bodies,
            bones.as_mut(),
        ) else {
            continue;
        };
        let (mod_, none) = (cx.intern_exact("MOD_EXPLOSIVE"), cx.intern_exact("none"));
        // The inflictor is NULL and the attacker `g_entities[ENTITYNUM_WORLD]`
        // (combat doc, 14.2), so the callbacks get `undefined` and the world.
        let world_ent = host.ents.world(cx);
        let args = vec![
            Value::Undefined,
            Value::Entity(world_ent),
            Value::Int(hit.damage),
            Value::Int(DFLAG_RADIUS),
            Value::String(mod_),
            Value::String(none),
            Value::Vector(hit.point),
            Value::Vector(hit.dir),
            Value::String(none),
        ];
        let callback = cx.func_ref(CALLBACK_SETUP, "CodeCallback_PlayerDamage");
        cx.spawn_then(callback, Some(Target::Entity(id)), args, BLAST_TOKEN);
        return;
    }
}

/// Every playing body, posed as the last end frame left it, at the origin a
/// `setOrigin` this frame may have moved it to. A client killed since then
/// is a corpse (`player_die`'s contents) and stops nothing.
fn blast_bodies(host: &mut GameHost, cx: &mut Cx) -> Vec<crate::game::combat::HitBody> {
    let origin_field = cx.intern_folded("origin");
    let mut bodies = Vec::new();
    for (other, body) in host.client_bodies.clone().into_iter().enumerate() {
        let Some(mut body) = body else { continue };
        if host.client_vitals[other].dead {
            continue;
        }
        let Some(handle) = host.ents.handle(other as u32) else {
            continue;
        };
        let Value::Vector(at) = host.get_field(cx, handle, origin_field) else {
            continue;
        };
        body.origin = Vec3::from(at);
        bodies.push(body);
    }
    bodies
}

/// A turret's turn in the walk: measured where it now stands, damaged
/// through `G_Damage`'s entity arm with the world as the attacker, and no
/// callback to wait for, so the walk goes straight on. The notifies wake
/// their waiters after the builtin returns, as retail's did
/// (`probe_victims`, combat doc 14.6).
fn blast_entity(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let Some(victim) = host.blast_entities(cx).into_iter().find(|v| v.id == id) else {
        return;
    };
    let bodies = blast_bodies(host, cx);
    let world = host.world.clone();
    let models = host.placed_script_models(cx);
    let (fs, anims) = (host.fs.clone(), host.anims.clone());
    let mut bones = match (fs.as_deref(), anims.as_deref()) {
        (Some(fs), Some(anims)) => Some(crate::game::combat::BoneTraceCtx {
            fs,
            anims,
            rigs: &mut host.hit_rigs,
            now_ms: host.level_time_ms,
        }),
        _ => None,
    };
    let walk = host.blasts.last().expect("the walk this turn came from");
    let Some(damage) = walk.blast.entity_damage(
        &victim,
        world.as_deref().map(|w| &w.collision),
        &models,
        &bodies,
        bones.as_mut(),
    ) else {
        return;
    };
    let attacker = host.ents.world(cx);
    for (event, args) in host.damage_entity(cx, id, damage, attacker) {
        let event = cx.intern_folded(event);
        cx.notify(Target::Entity(id), event, args);
    }
}

/// A client as a blast candidate, standing: the box and the eye height
/// `CanDamage` needs (combat doc, 14.3), which is all a script-side victim
/// has, its stance living on the sim rather than on the host. `link_origin`
/// is where its last link put it: a cmd's snapped origin, or a `setOrigin`'s.
fn standing_victim(
    slot: usize,
    origin: Vec3,
    link_origin: Vec3,
) -> crate::game::combat::BlastVictim {
    use vcod_common::pmove::{HALF_WIDTH, Stance};
    crate::game::combat::BlastVictim {
        slot,
        origin,
        link_origin,
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
///
/// The five keys and what each arm writes: docs/research/cod11-combat.md 2.7.
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
        Some(ignore_ent),
    ) = (args.first(), args.get(1), args.get(2), args.get(3))
    else {
        return Err(ErrorKind::BadType(
            "bulletTrace takes a start, an end, hitCharacters and ignoreEnt",
        ));
    };
    let (from, to) = (*from, *to);
    let ignore = match ignore_ent {
        Value::Entity(id) => Some(*id),
        _ => None,
    };

    let arr = cx.new_array();
    let position = ArrayKey::Str(cx.intern_exact("position"));
    let fraction = ArrayKey::Str(cx.intern_exact("fraction"));
    let entity = ArrayKey::Str(cx.intern_exact("entity"));
    let surfacetype = ArrayKey::Str(cx.intern_exact("surfacetype"));
    let normal = ArrayKey::Str(cx.intern_exact("normal"));

    let start = Vec3::new(from[0], from[1], from[2]);
    let end = Vec3::new(to[0], to[1], to[2]);
    let t = host
        .world
        .as_ref()
        .map(|w| w.collision.shot_trace(start, end));
    let mut hit = t
        .filter(|t| t.fraction < 1.0)
        .map(|t| (t.fraction, t.normal, None));
    if let Some(e) = script_model_hit(host, cx, start, end, ignore, hit.map_or(1.0, |h| h.0)) {
        hit = Some(e);
    }
    cx.set_index(arr, fraction, Value::Float(hit.map_or(1.0, |h| h.0)));
    cx.set_index(
        arr,
        position,
        Value::Vector(hit.map_or(to, |h| (start + (end - start) * h.0).to_array())),
    );
    // A player the trace stopped on is not resolved yet, and a hit's
    // `surfacetype` needs the surface-name table retail derives from
    // `surface_flags`, which nothing here maps yet. Both stay undefined
    // rather than guessing a value.
    let ent = hit
        .and_then(|h| h.2)
        .map_or(Value::Undefined, Value::Entity);
    cx.set_index(arr, entity, ent);
    match hit {
        Some((_, n, _)) => {
            cx.set_index(arr, normal, Value::Vector(n.to_array()));
            cx.set_index(arr, surfacetype, Value::Undefined);
        }
        None => {
            let dir = (end - start).normalize_or_zero();
            cx.set_index(arr, normal, Value::Vector(dir.to_array()));
            let none = Value::String(cx.intern_exact("none"));
            cx.set_index(arr, surfacetype, none);
        }
    }
    Ok(Value::Array(arr))
}

/// The nearest live `script_model` a `bulletTrace` segment crosses closer
/// than `best`: fraction, world normal, entity.
fn script_model_hit(
    host: &mut GameHost,
    cx: &mut Cx,
    start: Vec3,
    end: Vec3,
    ignore: Option<EntId>,
    best: f32,
) -> Option<(f32, Vec3, Option<EntId>)> {
    let mut hit = None;
    let mut best = best;
    for m in host.placed_script_models(cx) {
        if Some(m.id) == ignore {
            continue;
        }
        if let Some((f, n)) = m.clip(start, end, vcod_common::collision::MASK_SHOT, best) {
            best = f;
            hit = Some((f, n, Some(m.id)));
        }
    }
    hit
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

    /// The stock gametypes' own shape for a death with no player behind it:
    /// `sd.gsc:782` calls `attacker getEntityNumber()` ahead of any test,
    /// and `dm.gsc:492` calls `isPlayer(attacker)` unguarded.
    const WORLD_BLAST: &str = r#"
        main() {}
        CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
            self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
        }
        CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {
            level.playercam = eAttacker getEntityNumber();
            level.was_player = isPlayer(eAttacker);
            level.finished = 1;
        }
        mine() { radiusDamage((0,0,0), 300, 2000, 50); }
    "#;

    /// A blast a script sets off names the world as the attacker, and the
    /// world is an *entity*: retail hands `g_entities[ENTITYNUM_WORLD]` over,
    /// so `getEntityNumber` answers 1022, `isPlayer(attacker)` answers false
    /// and the death runs on. Passing `undefined` instead aborted `sd.gsc`'s
    /// killed callback at line 782 on every bomb kill.
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
            takedamage: true,
        };
        rt.place_client(0, [0.0, 0.0, 0.0]);
        let victim = rt.client_entity(0).expect("the client has an entity");
        rt.start_thread_for_test(victim, "mine", 0);
        rt.run_frame(0);

        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.level_field("playercam"), Value::Int(1022));
        assert_eq!(rt.level_field("was_player"), Value::Int(0));
        assert_eq!(
            rt.level_field("finished"),
            Value::Int(1),
            "the callback has to run past the isPlayer call, not abort on it"
        );
        assert!(rt.client_vitals(0).dead, "2000 damage at zero range kills");
    }

    const WEAPON_CALLBACKS: &str = r#"
        main() {}
        CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
            self.hurtby = sWeapon;
            self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
        }
        CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {
            self.killedby = sWeapon;
        }
    "#;

    /// `player_die`'s replacement (turrets doc 11, 12.6): a kill by a
    /// mounted gunner names the gun's weapon to the killed callback, while
    /// the damage callback still sees the gunner's own.
    #[test]
    fn a_gunners_kill_is_credited_to_the_gun() {
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, WEAPON_CALLBACKS);
        for (slot, name) in [(0, "victim"), (1, "gunner")] {
            rt.push_client_event(ClientEvent::Connect {
                slot,
                name: name.into(),
            });
        }
        rt.run_frame(0);
        let def = crate::game::turret::TurretDef::parse(
            "WEAPONFILE\\weaponClass\\turret\\damage\\60\\fireTime\\0.05",
        )
        .unwrap();
        let mut rec = crate::game::turret::TurretRecord::new(
            "mg42_bipod_stand_mp",
            def,
            Default::default(),
            0.0,
        );
        rec.owner = Some(1);
        rec.busy = 1;
        rt.host.turrets.insert(vcod_gsc::EntId(298, 0), rec);
        for health in [100, 10] {
            rt.host.client_vitals[0] = Vitals {
                health,
                max_health: 100,
                dead: false,
                takedamage: true,
            };
            rt.deliver_hits(vec![hit(53)], 50);
        }
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(
            rt.client_field(0, "hurtby").as_deref(),
            Some("m1carbine_mp")
        );
        assert_eq!(
            rt.client_field(0, "killedby").as_deref(),
            Some("mg42_bipod_stand_mp")
        );
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
            takedamage: true,
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
    /// killed callback. A hit on a player already dead, before its end frame
    /// clears `takedamage`, takes health below zero and kills no second
    /// time (combat doc, 14.5); after it, nothing reaches the player.
    #[test]
    fn a_surviving_hit_takes_health_and_a_dead_player_only_until_its_end_frame() {
        let mut rt = two_clients();
        rt.host.client_vitals[0] = Vitals {
            health: 100,
            max_health: 100,
            dead: false,
            takedamage: true,
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
        assert_eq!(rt.client_vitals(0).health, 0);
        rt.deliver_hits(vec![hit(67)], 100);
        assert_eq!(rt.client_vitals(0).health, -67);
        let ops = rt.take_sim_ops();
        assert!(matches!(
            ops[..],
            [
                (_, SimOp::Damaged { fatal: true, .. }),
                (_, SimOp::Damaged { fatal: false, .. })
            ]
        ));
        rt.deliver_hits(vec![hit(2000)], 100);
        assert_eq!(rt.client_vitals(0).health, -999, "clamped");
        rt.take_sim_ops();
        // The dead arm of its end frame.
        rt.set_client_takedamage(0, false);
        rt.deliver_hits(vec![hit(67)], 150);
        assert!(rt.take_sim_ops().is_empty(), "nothing reached it");
        assert_eq!(rt.client_vitals(0).health, -999);
    }

    /// Every `radiusDamage` is a noise the bots hear, whoever it hurt.
    #[test]
    fn radiusdamage_is_heard() {
        const SCRIPT: &str = r#"
            main() {
                wait 1;
                radiusDamage((10, 20, 30), 300, 2000, 50);
                radiusDamage((40, 50, 60), 300, 2000, 50);
            }
            CodeCallback_PlayerConnect() {}
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        rt.run_frame(1100);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(
            rt.take_blast_noises(),
            [[10.0, 20.0, 30.0], [40.0, 50.0, 60.0]]
        );
        assert!(rt.take_blast_noises().is_empty(), "heard twice");
    }

    /// `radiusDamage` runs `CodeCallback_PlayerDamage` on every live client
    /// inside the radius, inline, before the calling thread's next line:
    /// the near client takes the falloff's damage and dies of it, the one
    /// 1000 units out takes nothing, and the flags the script is handed
    /// carry `DFLAG_RADIUS`. The attacker is the world entity.
    ///
    /// The second blast is the dead check: a second later the dead client's
    /// end frame has cleared `takedamage`, so its callback ran once.
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
                takedamage: true,
            };
        }
        rt.place_client(0, [100.0, 0.0, 0.0]);
        rt.place_client(1, [1000.0, 0.0, 0.0]);
        rt.run_frame(1100);
        // The dead arm of the victim's end frame.
        rt.set_client_takedamage(0, false);
        rt.run_frame(2200);

        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        // 2000 - (2000 - 50) * 100/300, the linear falloff.
        assert_eq!(rt.client_field(0, "took").as_deref(), Some("1350"));
        assert_eq!(rt.client_field(0, "flags").as_deref(), Some("1"));
        assert_eq!(rt.client_field(0, "mod").as_deref(), Some("MOD_EXPLOSIVE"));
        assert_eq!(
            rt.client_field(0, "killer").as_deref(),
            Some("1"),
            "the world set this blast off, and the world is an entity"
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

    /// A live player standing between a scripted blast and another player
    /// shields it (combat doc, 14.4): its body is where script last put it,
    /// so a blocker `setOrigin`ed onto the line in the same frame as the
    /// blast stops every probe, and the victim behind it, inside the radius
    /// but past the second chance's reach, takes nothing. Moved off the line
    /// on the next frame, it lets the full falloff through. With no paks the
    /// body is its link box.
    #[test]
    fn a_body_in_the_line_shields_a_scripted_blast() {
        const SCRIPT: &str = r#"
            main() {
                wait 1;
                players = getentarray("player", "classname");
                for (i = 0; i < players.size; i++)
                {
                    if (players[i] getEntityNumber() == 1)
                        players[i] setorigin((50, 0, 0));
                }
                radiusDamage((0, 0, 8), 300, 20, 20);
                wait 1;
                for (i = 0; i < players.size; i++)
                {
                    if (players[i] getEntityNumber() == 1)
                        players[i] setorigin((50, 200, 0));
                }
                radiusDamage((0, 0, 8), 300, 20, 20);
            }
            CodeCallback_PlayerConnect() {}
            CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
                if (!isdefined(self.hits))
                    self.hits = 0;
                self.hits = self.hits + 1;
                self.took = iDamage;
            }
            CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        rt.host.world = Some(Rc::new(World {
            collision: vcod_common::collision::test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
        }));
        for (slot, name) in [(0, "victim"), (1, "blocker")] {
            rt.push_client_event(ClientEvent::Connect {
                slot,
                name: name.into(),
            });
        }
        rt.run_frame(50);
        let feet = [[100.0, 0.0, 0.0], [300.0, 300.0, 0.0]];
        for slot in [0, 1] {
            rt.host.client_vitals[slot] = Vitals {
                health: 100,
                max_health: 100,
                dead: false,
                takedamage: true,
            };
            rt.place_client(slot, feet[slot]);
            rt.set_client_body(
                slot,
                Some(crate::game::combat::HitBody {
                    slot,
                    origin: Vec3::from(feet[slot]),
                    yaw: 0.0,
                    mins: Vec3::new(-15.0, -15.0, 0.0),
                    maxs: Vec3::new(15.0, 15.0, 72.0),
                    pose: Default::default(),
                }),
            );
        }
        rt.run_frame(1100);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(
            rt.client_field(0, "took").as_deref(),
            Some("Undefined"),
            "the blocker's body stopped every probe"
        );
        assert_eq!(rt.client_field(1, "took").as_deref(), Some("20"));
        rt.run_frame(2200);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "took").as_deref(), Some("20"));
        assert_eq!(rt.client_field(0, "hits").as_deref(), Some("1"));
    }

    /// `probe_blastloop`'s rows on retail (combat doc, 14.5, 14.7). Both
    /// stand across one split and link in slot order, so the walk takes
    /// slot 1 first. A 100-health player in front of a 1000-health one, a
    /// lethal flat 200: with the front one walked first it dies before the
    /// back one is measured and its corpse shields nothing, so the back one
    /// takes 200; with the back one walked first the live front body stops
    /// every probe and it takes nothing. The death relinks the dead one to
    /// the head of the list, so a second blast in the same frame takes it
    /// first, and still reaches it, its health going below zero; after its
    /// end frame nothing does.
    #[test]
    fn a_blast_walks_its_victims_one_callback_at_a_time() {
        const SCRIPT: &str = r#"
            main() {
                level.log = "";
                wait 1;
                radiusDamage((0, 0, 8), 300, 200, 200);
                level.log = level.log + "|";
                radiusDamage((0, 0, 8), 300, 20, 20);
                wait 1;
                radiusDamage((0, 0, 8), 300, 20, 20);
            }
            CodeCallback_PlayerConnect() {}
            CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
                level.log = level.log + " " + self getEntityNumber() + ":" + iDamage;
                self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc);
            }
            CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
        "#;
        // `front` is the slot standing 50 units out, the other stands at 100.
        for front in [0, 1] {
            let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
            rt.host.world = Some(Rc::new(World {
                collision: vcod_common::collision::test_world(&[]),
                vis: vcod_common::bsp::Visibility::none(),
                spawn: ([0.0, 0.0, 64.0], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
            }));
            for (slot, name) in [(0, "a"), (1, "b")] {
                rt.push_client_event(ClientEvent::Connect {
                    slot,
                    name: name.into(),
                });
            }
            rt.run_frame(50);
            for slot in [0, 1] {
                let x = if slot == front { 50.0 } else { 100.0 };
                let health = if slot == front { 100 } else { 1000 };
                rt.host.client_vitals[slot] = Vitals {
                    health,
                    max_health: 100,
                    dead: false,
                    takedamage: true,
                };
                rt.place_client(slot, [x, 0.0, 0.0]);
                rt.set_client_body(
                    slot,
                    Some(crate::game::combat::HitBody {
                        slot,
                        origin: Vec3::new(x, 0.0, 0.0),
                        yaw: 0.0,
                        mins: Vec3::new(-15.0, -15.0, 0.0),
                        maxs: Vec3::new(15.0, 15.0, 72.0),
                        pose: Default::default(),
                    }),
                );
            }
            rt.run_frame(1100);
            assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
            let back = 1 - front;
            assert_eq!(rt.client_vitals(front).health, -20);
            assert!(rt.client_vitals(front).dead);
            // The dead arm of its end frame.
            rt.set_client_takedamage(front, false);
            rt.run_frame(2200);
            let want = if front == 0 {
                " 0:200| 0:20 1:20 1:20"
            } else {
                " 1:200 0:200| 1:20 0:20 0:20"
            };
            assert_eq!(rt.level_field_str("log"), want, "front {front}");
            let back_left = if front == 0 { 1000 - 40 } else { 1000 - 240 };
            assert_eq!(rt.client_vitals(back).health, back_left);
        }
    }

    /// `probe_blastorder` on retail (combat doc 14.7): four players set down
    /// round mp_carentan's second split by `setOrigin`, then moved one at a
    /// time, a flat blast after each. The walk is the area tree's, each
    /// `setOrigin` putting its player at the head of the node it lands in.
    #[test]
    fn radiusdamage_walks_its_victims_in_the_area_trees_order() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let bytes = fs.read("maps/mp/mp_carentan.bsp").unwrap();
        let bsp = vcod_common::bsp::parse(&bytes).unwrap();
        const SCRIPT: &str = r#"
            main() {
                level.log = "";
                wait 1;
                players = getentarray("player", "classname");
                for (i = 0; i < players.size; i++)
                    level.p[players[i] getEntityNumber()] = players[i];
                level.p[0] setorigin((-290, 2430, -32));
                level.p[1] setorigin((-260, 2480, -32));
                level.p[2] setorigin((-230, 2380, -32));
                level.p[3] setorigin((-230, 2540, -32));
                blast();
                level.p[0] setorigin((-290, 2440, -32));
                blast();
                level.p[2] setorigin((-200, 2430, -32));
                blast();
                level.p[1] setorigin((-290, 2380, -32));
                blast();
            }
            blast() {
                level.log = level.log + "|";
                radiusDamage((-176.8, 2473.1, 7), 500, 20, 20);
            }
            CodeCallback_PlayerConnect() {}
            CodeCallback_PlayerDamage(eInflictor, eAttacker, iDamage, iDFlags, sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc) {
                level.log = level.log + self getEntityNumber();
            }
            CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {}
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        rt.host.area = crate::area::AreaTree::for_map(&bsp, &fs);
        rt.host.world = Some(Rc::new(World {
            collision: vcod_common::collision::test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
        }));
        for slot in 0..4 {
            rt.push_client_event(ClientEvent::Connect {
                slot,
                name: format!("p{slot}"),
            });
        }
        rt.run_frame(50);
        // In play at the far end of the map, as sd's spawns left them.
        for slot in 0..4 {
            rt.host.client_vitals[slot] = Vitals {
                health: 1000,
                max_health: 100,
                dead: false,
                takedamage: true,
            };
            rt.place_client(slot, [2000.0 + 100.0 * slot as f32, -1000.0, 0.0]);
        }
        rt.run_frame(1100);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.level_field_str("log"), "|1032|0132|2013|2031");
    }

    /// `probe_victims`' `fpd` rows (combat doc, 4.4): an entity inflictor
    /// reaches the killed callback as the attacker argument, and none at
    /// all as the world.
    #[test]
    fn a_kill_names_the_attacker_as_its_inflictor() {
        const SCRIPT: &str = r#"
            main() {
                level.log = "";
                wait 0.1;
                nade = spawn("script_origin", (0, 0, 0));
                p = getentarray("player", "classname");
                p[0] finishPlayerDamage(nade, p[1], 500, 0, "MOD_GRENADE_SPLASH", "none", (0, 0, 0), (0, 0, 1), "none");
                p[1] finishPlayerDamage(undefined, p[0], 500, 0, "MOD_RIFLE_BULLET", "none", (0, 0, 0), (0, 0, 1), "none");
            }
            CodeCallback_PlayerConnect() {}
            CodeCallback_PlayerKilled(eInflictor, eAttacker, iDamage, sMeansOfDeath, sWeapon, vDir, sHitLoc) {
                level.log = level.log + " " + eInflictor getEntityNumber() + ":" + eAttacker getEntityNumber();
            }
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        for (slot, name) in [(0, "a"), (1, "b")] {
            rt.push_client_event(ClientEvent::Connect {
                slot,
                name: name.into(),
            });
        }
        rt.run_frame(50);
        for slot in [0, 1] {
            rt.host.client_vitals[slot] = Vitals {
                health: 100,
                max_health: 100,
                dead: false,
                takedamage: true,
            };
        }
        rt.run_frame(150);
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.level_field_str("log"), " 1:1 1022:0");
    }

    /// `probe_victims`' turret rows (combat doc, 14.6): three flat 60s take
    /// a turret from 100 to 40, -20 and -80, each raising `"damage"` with
    /// the world as the attacker, and every one at or below 0 `"death"` as
    /// well. Neither waiter runs before the builtin's caller goes on, and
    /// `"death"`'s, queued last, runs first.
    #[test]
    fn a_blast_damages_a_turret_and_notifies_it() {
        const SCRIPT: &str = r#"
            main() {
                level.log = "";
                wait 0.1;
                t = getentarray("misc_mg42", "classname")[0];
                t thread watch();
                t thread watchDeath();
                for (i = 0; i < 3; i++) {
                    radiusDamage(t.origin + (0, 70, 32), 300, 60, 60);
                    level.log = level.log + " h" + t.health;
                    wait 0.05;
                }
            }
            watch() {
                for (;;) {
                    self waittill("damage", amount, attacker);
                    level.log = level.log + " d" + amount + ":" + attacker getEntityNumber();
                }
            }
            watchDeath() {
                for (;;) {
                    self waittill("death", attacker);
                    level.log = level.log + " x" + attacker getEntityNumber();
                }
            }
            CodeCallback_PlayerConnect() {}
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        rt.host.world = Some(Rc::new(World {
            collision: vcod_common::collision::test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
        }));
        rt.place_turret([0.0, 0.0, 8.0]);
        for t in [50, 100, 150, 200, 250] {
            rt.run_frame(t);
        }
        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(
            rt.level_field_str("log"),
            " h40 d60:1022 h-20 x1022 d60:1022 h-80 x1022 d60:1022"
        );
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
            takedamage: true,
        };
        rt.place_client(0, [100.0, 0.0, 0.0]);

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

    /// `["normal"]` on a miss is the segment's own direction, normalised,
    /// and `["surfacetype"]` is `"none"` (0x5ad50..0x5adb3).
    #[test]
    fn bullettrace_miss_carries_the_segment_direction_as_its_normal() {
        let (mut vm, mut host) = fixture();
        vm.with_cx(|cx| {
            let from = Value::Vector([0.0, 0.0, 0.0]);
            let to = Value::Vector([0.0, 0.0, -18.0]);
            let args = [from, to, Value::Int(0), Value::Undefined];
            let Value::Array(arr) = bullet_trace(&mut host, cx, None, &args).unwrap() else {
                panic!()
            };
            let n = ArrayKey::Str(cx.intern_exact("normal"));
            assert_eq!(cx.get_index(arr, n), Value::Vector([0.0, 0.0, -1.0]));
            let st = ArrayKey::Str(cx.intern_exact("surfacetype"));
            let Value::String(name) = cx.get_index(arr, st) else {
                panic!("surfacetype is a string on a miss")
            };
            assert_eq!(cx.resolve(name), "none");
        });
    }

    /// With a real collision world, a trace straight down through the test
    /// floor (`vcod_common::collision::test_world`, top at z=0) stops short
    /// of the end point: `fraction < 1`, and `["normal"]` is the floor's.
    #[test]
    fn bullettrace_with_a_world_hits_real_geometry() {
        let (mut vm, mut host) = fixture();
        host.world = Some(Rc::new(World {
            collision: vcod_common::collision::test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
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
            let n = ArrayKey::Str(cx.intern_exact("normal"));
            assert_eq!(cx.get_index(arr, n), Value::Vector([0.0, 0.0, 1.0]));
        });
    }

    /// A linked `script_model` stops a `bulletTrace` on its xmodel's
    /// collision, placed at the entity's origin and angles, and the result
    /// names it; the same entity as `ignoreEnt` lets the trace through.
    #[test]
    fn bullettrace_clips_a_script_model_s_collision() {
        let (mut vm, mut host) = fixture();
        // A wall facing -X in the model's frame at x = 10.
        let wall = vcod_common::collision::ModelTri {
            tri: [
                Vec3::new(10.0, -50.0, 0.0),
                Vec3::new(10.0, -50.0, 100.0),
                Vec3::new(10.0, 150.0, 0.0),
            ],
            contents: 1,
            surface_flags: 0,
        };
        host.xmodel_collision
            .insert("xmodel/test_wall".into(), Some(Rc::from(vec![wall])));
        vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            for (name, v) in [
                ("classname", Value::String(cx.intern_exact("script_model"))),
                ("model", Value::String(cx.intern_exact("xmodel/test_wall"))),
                ("origin", Value::Vector([100.0, 0.0, 0.0])),
                ("angles", Value::Vector([0.0, 90.0, 0.0])),
            ] {
                let field = cx.intern_folded(name);
                host.set_field(cx, id, field, v).unwrap();
            }
            // Yawed 90 about the origin, the wall stands across +Y at y = 10.
            let from = Value::Vector([100.0, 0.0, 50.0]);
            let to = Value::Vector([100.0, 200.0, 50.0]);
            let args = [from, to, Value::Int(0), Value::Undefined];
            let Value::Array(arr) = bullet_trace(&mut host, cx, None, &args).unwrap() else {
                panic!()
            };
            let key = |cx: &mut Cx, k: &str| ArrayKey::Str(cx.intern_exact(k));
            let k = key(cx, "position");
            let Value::Vector(p) = cx.get_index(arr, k) else {
                panic!()
            };
            assert!((p[1] - (10.0 - 0.125)).abs() < 1e-3, "{p:?}");
            let k = key(cx, "normal");
            let Value::Vector(n) = cx.get_index(arr, k) else {
                panic!()
            };
            assert!(Vec3::from(n).abs_diff_eq(-Vec3::Y, 1e-5), "{n:?}");
            let k = key(cx, "entity");
            assert_eq!(cx.get_index(arr, k), Value::Entity(id));

            let args = [from, to, Value::Int(0), Value::Entity(id)];
            let Value::Array(arr) = bullet_trace(&mut host, cx, None, &args).unwrap() else {
                panic!()
            };
            let k = key(cx, "fraction");
            assert_eq!(cx.get_index(arr, k), Value::Float(1.0));
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
    /// gets. The callback sees `player_die`'s arguments from the builtin's
    /// call (0x453e5): damage 100000, `MOD_SUICIDE`, weapon 0 as `"none"`,
    /// no direction and hit location `"none"`, whatever the player holds.
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
                self.dmg = iDamage;
                self.dir = isDefined(vDir);
                self.loc = sHitLoc;
                self.killer = eAttacker getEntityNumber();
            }
        "#;
        let mut rt = ScriptRuntime::for_test_at(CALLBACK_SETUP, SCRIPT);
        rt.host.client_vitals[0] = Vitals {
            health: 100,
            max_health: 100,
            dead: false,
            takedamage: true,
        };
        rt.push_client_event(ClientEvent::Connect {
            slot: 0,
            name: "victim".into(),
        });
        rt.run_frame(0);

        assert!(rt.aborts().is_empty(), "{:?}", rt.aborts());
        assert_eq!(rt.client_field(0, "after").as_deref(), Some("dead"));
        assert_eq!(rt.client_field(0, "mod").as_deref(), Some("MOD_SUICIDE"));
        assert_eq!(rt.client_field(0, "weap").as_deref(), Some("none"));
        assert_eq!(rt.client_field(0, "dmg").as_deref(), Some("100000"));
        assert_eq!(rt.client_field(0, "dir").as_deref(), Some("0"));
        assert_eq!(rt.client_field(0, "loc").as_deref(), Some("none"));
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
