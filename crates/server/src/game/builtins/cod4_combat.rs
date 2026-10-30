//! CoD4 1.7's damage, death and drop builtins (docs/research/cod4-combat.md
//! 4..6): `finishPlayerDamage`, `obituary`, `clonePlayer`, `dropItem` and
//! the item and ammo queries the death path reaches. The playerstate half
//! of a hit and a kill goes to the level as a `Damage4`
//! (`game::cod4::combat`); `cod4_combat_ab` checks it against the kill
//! capture.

use vcod_cod4::pmove::anim::EventAnim;
use vcod_gsc::{Cx, EntId, ErrorKind, Target, Value};

use super::client::client_receiver;
use super::cod4_client::{weapon_arg, with_loadout};
use super::score::Builtin;
use crate::game::cod4::combat::{Damage4, HITLOC_NAMES, Item4, ev};
use crate::game::host::GameHost;
use crate::game::temp_entity::{Scope, TempEntity};

pub const NAMES: &[(&str, Builtin)] = &[
    ("finishplayerdamage", finish_player_damage),
    ("obituary", obituary),
    ("cloneplayer", clone_player),
    ("suicide", suicide),
    ("dropitem", drop_item),
    ("itemweaponsetammo", item_weapon_set_ammo),
    ("anyammoforweaponmodes", any_ammo_for_weapon_modes),
    ("startragdoll", start_ragdoll),
    ("isragdoll", is_ragdoll),
    ("playrumbleonentity", nothing),
    ("isonladder", is_on_ladder),
    ("ismantling", is_mantling),
    ("getplayerangles", get_player_angles),
    ("getxuid", super::client::get_guid),
];

/// `self getPlayerAngles()`: the view as the last cmd left it.
fn get_player_angles(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    Ok(Value::Vector(
        host.cod4()
            .view_angles
            .get(slot)
            .copied()
            .unwrap_or([0.0; 3]),
    ))
}

/// 1.7's ragdoll `trType`s: `TR_RAGDOLL` 9, `TR_RAGDOLL_GRAVITY` 10,
/// `TR_RAGDOLL_INTERPOLATE` 11 (cod4-combat.md 5.1).
const TR_RAGDOLL: i32 = 9;
const TR_RAGDOLL_GRAVITY: i32 = 10;
const TR_RAGDOLL_INTERPOLATE: i32 = 11;

/// `ent startRagdoll(<int>)` (0x80c4f08, VERIFIED): the argument read and
/// dropped, then `pos` and `apos` each turned into a ragdoll type,
/// `TR_INTERPOLATE` to 11, `TR_GRAVITY` to 10, anything else to 9. The
/// client runs the ragdoll once it sees one (cod4-combat.md 5.1). Only a
/// corpse carries a body here; on any other entity it does nothing.
fn start_ragdoll(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::entity::entity_receiver(recv)?;
    let p = &crate::game::cod4::COD4_HOST_PROTO;
    if let Some(b) = host.bodies.body_mut(id.0) {
        for name in ["pos.trType", "apos.trType"] {
            if let Some(i) = vcod_common::net::msg::EntityState::field_index(p, name) {
                b.state.fields[i] = match b.state.fields[i] {
                    1 => TR_RAGDOLL_INTERPOLATE,
                    5 => TR_RAGDOLL_GRAVITY,
                    _ => TR_RAGDOLL,
                };
            }
        }
    }
    Ok(Value::Undefined)
}

/// `ent isRagdoll()` (0x80c4eb4, VERIFIED): whether `pos.trType` is one of
/// the three ragdoll types.
fn is_ragdoll(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::entity::entity_receiver(recv)?;
    let p = &crate::game::cod4::COD4_HOST_PROTO;
    let tr = host
        .bodies
        .body_mut(id.0)
        .map_or(0, |b| b.state.field_i32(p, "pos.trType"));
    Ok(Value::Int(
        (TR_RAGDOLL..=TR_RAGDOLL_INTERPOLATE).contains(&tr) as i32,
    ))
}

/// A builtin whose effect is client-side (rumble).
fn nothing(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

fn number(v: &Value) -> Result<i32, ErrorKind> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Float(f) => Ok(*f as i32),
        _ => Err(ErrorKind::BadType("expected a number")),
    }
}

/// Where a hit leaves its victim, decided on the health it leaves
/// (`cod4_lnxded` 0x80ac8e9, 0x80acc2a; cod4-combat.md 4).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Hurt,
    /// Last Stand's first killing hit.
    Down,
    Killed,
}

/// `finishPlayerDamage`'s arguments, read in 1.7's order (0x80ac5f3 ..
/// 0x80ac6d0). The means of death, weapon and hit location keep their
/// script values for the callbacks.
struct Hit {
    inflictor: EntId,
    attacker: EntId,
    /// The attacker's slot when he is a player.
    attacker_slot: Option<usize>,
    damage: i32,
    dflags: i32,
    means: Value,
    means_name: String,
    weapon: Value,
    weapon_num: usize,
    /// The impact point, the origin when the script passes none.
    point: [f32; 3],
    /// The direction normalized, `None` when the script passes none.
    dir: Option<[f32; 3]>,
    hit_loc: Value,
    head: bool,
    time_offset: i32,
}

impl Hit {
    /// An undefined inflictor or attacker reads as the world (entity
    /// 1022, 0x80ac61e).
    fn read(
        host: &mut GameHost,
        cx: &mut Cx,
        args: &[Value],
        damage: i32,
    ) -> Result<Hit, ErrorKind> {
        let world = host.ents.world(cx);
        let entity_or_world = |v: &Value| match v {
            Value::Entity(e) => *e,
            _ => world,
        };
        let string = |cx: &Cx, v: &Value| match v {
            Value::String(s) => Some(cx.resolve(*s).to_string()),
            _ => None,
        };
        let attacker = entity_or_world(&args[1]);
        let is_player = matches!(args[1], Value::Entity(a)
            if host.ents.get(a).is_some_and(|e| e.client.is_some()));
        let weapon_num = string(cx, &args[5])
            .and_then(|w| host.cod4().table.index(&w))
            .unwrap_or(0);
        let vector = |v: &Value| match v {
            Value::Vector(p) => Some(*p),
            _ => None,
        };
        Ok(Hit {
            inflictor: entity_or_world(&args[0]),
            attacker,
            attacker_slot: is_player.then_some(attacker.0 as usize),
            damage,
            dflags: number(&args[3])?,
            means: args[4],
            means_name: string(cx, &args[4]).unwrap_or_default(),
            weapon: args[5],
            weapon_num,
            point: vector(&args[6]).unwrap_or_default(),
            dir: vector(&args[7]).map(|d| glam::Vec3::from(d).normalize_or_zero().to_array()),
            hit_loc: args[8],
            head: string(cx, &args[8]).as_deref() == Some(HITLOC_NAMES[2]),
            time_offset: args.get(9).map(number).transpose()?.unwrap_or(0),
        })
    }

    /// The seven arguments both callbacks open with: inflictor, attacker,
    /// damage, means of death, weapon, direction (0 when none), hit
    /// location (0x80c293a, 0x80c2a64).
    fn callback_args(&self) -> Vec<Value> {
        vec![
            Value::Entity(self.inflictor),
            Value::Entity(self.attacker),
            Value::Int(self.damage),
            self.means,
            self.weapon,
            Value::Vector(self.dir.unwrap_or_default()),
            self.hit_loc,
        ]
    }

    /// The playerstate half the level applies at its end frame.
    fn op(&self, slot: usize, fatal: bool, death: Option<EventAnim>) -> Damage4 {
        Damage4 {
            slot,
            damage: self.damage,
            dflags: self.dflags,
            dir: self.dir,
            fatal,
            attacker: self.attacker_slot,
            death,
        }
    }

    /// `rifleBullet` of a bullet weapon (`weapType` 0) fired as
    /// `MOD_PISTOL_BULLET`, `MOD_RIFLE_BULLET` or `MOD_HEAD_SHOT` (1, 2, 8:
    /// 0x80acaf8..0x80acde4), the only hits that raise flesh events.
    fn rifle_bullet(&self, host: &GameHost) -> Option<bool> {
        const BULLET_MEANS: [&str; 3] = ["MOD_PISTOL_BULLET", "MOD_RIFLE_BULLET", "MOD_HEAD_SHOT"];
        if self.weapon_num == 0 || !BULLET_MEANS.contains(&self.means_name.as_str()) {
            return None;
        }
        let c4 = host.cod4();
        let name = &c4.table.defs.get(self.weapon_num)?.name;
        let def = c4.weapon_files.get(name)?.pm.as_ref()?;
        (def.weap_type == 0).then_some(def.rifle_bullet)
    }
}

/// `self finishPlayerDamage(eInflictor, eAttacker, iDamage, iDFlags,
/// sMeansOfDeath, sWeapon, vPoint, vDir, sHitLoc, psOffsetTime)`
/// (0x80ac58e; cod4-combat.md 4, "Last Stand"). Nothing lands inside
/// Last Stand's 500 ms or for a damage under 1; a dead victim is an
/// error. A bullet raises the flesh events, the health comes off,
/// `damage` is notified with the damage and the attacker, and then the
/// victim lives, goes down (Last Stand) or dies. The knockback, the blood
/// and the dead playerstate go to the level as a [`Damage4`].
fn finish_player_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    if args.len() < 9 {
        return Err(ErrorKind::BadType("finishPlayerDamage takes ten arguments"));
    }
    let now = host.level_time_ms;
    let client = &host.cod4().clients[slot];
    let shielded = client.last_stand && client.last_stand_time > now;
    let damage = number(&args[2])?;
    if shielded || damage <= 0 {
        return Ok(Value::Undefined);
    }
    let hit = Hit::read(host, cx, args, damage)?;
    if host.client_vitals[slot].dead {
        return Err(ErrorKind::Custom(
            "Trying to do damage to a client that is already dead".into(),
        ));
    }
    let outcome = {
        let client = &host.cod4().clients[slot];
        if host.client_vitals[slot].health > damage {
            Outcome::Hurt
        } else if !client.last_stand && client.perks & PERK_PISTOL_DEATH != 0 {
            Outcome::Down
        } else {
            Outcome::Killed
        }
    };
    flesh_events(host, cx, slot, &hit, outcome == Outcome::Killed);
    host.client_vitals[slot].health -= damage;
    if let Some(r) = recv {
        let event = cx.intern_folded("damage");
        cx.notify(
            r,
            event,
            vec![Value::Int(damage), Value::Entity(hit.attacker)],
        );
    }
    match outcome {
        Outcome::Hurt => host.cod4_mut().damage_ops.push(hit.op(slot, false, None)),
        Outcome::Down => go_down(host, cx, slot, recv, &hit),
        Outcome::Killed => kill(host, cx, slot, recv, &hit),
    }
    Ok(Value::Undefined)
}

/// A bullet's two flesh events at the impact (0x80acb07..0x80acc04,
/// cod4-effects.md): `EV_BULLET_HIT` for everyone but the victim, its parm
/// the direction's byte and its scale the headshot bit, 2 added on a kill
/// (0x80acca4); `EV_BULLET_HIT_CLIENT_SMALL` or `_LARGE` by
/// `rifleBullet` for the victim alone. Both carry the flesh surface (7),
/// the attacker and the weapon.
fn flesh_events(host: &mut GameHost, cx: &mut Cx, slot: usize, hit: &Hit, killed: bool) {
    let Some(rifle) = hit.rifle_bullet(host) else {
        return;
    };
    let event = |event, parm, scale, client_num, scope| TempEntity {
        event,
        parm,
        surf_type: 7,
        other: hit.attacker.0,
        attacker: 0,
        weapon: hit.weapon_num as i32,
        client_num,
        scale,
        origin: hit.point,
        scope,
        quake: None,
        origin2: None,
    };
    let dir_byte = vcod_common::net::events::dir_to_byte(hit.dir.unwrap_or_default());
    let scale = i32::from(hit.head) | i32::from(killed) << 1;
    let seen = event(ev::BULLET_HIT, dir_byte, scale, 0, Scope::AllBut(slot));
    let felt = event(
        ev::BULLET_HIT_CLIENT_SMALL + i32::from(rifle),
        0,
        0,
        slot as i32,
        Scope::Only(slot),
    );
    host.add_temp_entity(cx, seen);
    host.add_temp_entity(cx, felt);
}

/// Last Stand's arm (0x80acc2a..0x80acc90): the flag up, no damage for
/// 500 ms, the playerstate takes the hit as any other, and
/// `CodeCallback_PlayerLastStand` (pushed at 0x80c293a) runs with the
/// killed callback's arguments and a `deathAnimDuration` of 0. The end
/// frame puts the client in `PM_LASTSTAND`.
fn go_down(host: &mut GameHost, cx: &mut Cx, slot: usize, recv: Option<Target>, hit: &Hit) {
    let until = host.level_time_ms + 500;
    let c4 = host.cod4_mut();
    c4.clients[slot].last_stand = true;
    c4.clients[slot].last_stand_time = until;
    c4.damage_ops.push(hit.op(slot, false, None));
    let mut args = hit.callback_args();
    args.extend([Value::Int(hit.time_offset), Value::Int(0)]);
    let callback = cx.func_ref(
        crate::game::script::CALLBACK_SETUP,
        "CodeCallback_PlayerLastStand",
    );
    cx.spawn_then(callback, recv, args, slot as u32);
}

/// `player_die` for a killing hit: the death grenades, the death anim,
/// health 0, `death` with the attacker, then `CodeCallback_PlayerKilled`
/// with the time offset and the anim's duration (cod4-combat.md 4).
fn kill(host: &mut GameHost, cx: &mut Cx, slot: usize, recv: Option<Target>, hit: &Hit) {
    death_grenades(host, cx, slot, recv, &hit.means_name);
    let death = draw_death_anim(host, slot);
    host.cod4_mut().damage_ops.push(hit.op(slot, true, death));
    host.die(slot);
    host.client_vitals[slot].health = 0;
    host.cod4_mut().callback = crate::game::host::Cod2CallbackArgs {
        time_offset: hit.time_offset,
        death_anim_duration: death.map_or(0, |d| d.duration),
    };
    if let Some(r) = recv {
        let event = cx.intern_folded("death");
        cx.notify(r, event, vec![Value::Entity(hit.attacker)]);
    }
    let killed = cx.func_ref(
        crate::game::script::CALLBACK_SETUP,
        "CodeCallback_PlayerKilled",
    );
    let args = host.callback_args("CodeCallback_PlayerKilled", hit.callback_args());
    cx.spawn_then(killed, recv, args, slot as u32);
    host.cod4_mut().callback = Default::default();
}

/// `player_die`'s death anim: one of the clause's commands by `rand() % n`
/// off the level's stream, set on the corpse to come (cod4-combat.md 5,
/// 10.9).
fn draw_death_anim(host: &mut GameHost, slot: usize) -> Option<EventAnim> {
    let n = host.cod4().death_anims[slot].len();
    if n == 0 {
        return None;
    }
    let k = host.cod4_mut().rand() as usize % n;
    let c4 = host.cod4_mut();
    let anim = c4.death_anims[slot][k];
    if let Some(corpse) = c4.corpse_source[slot].as_mut() {
        corpse.legs_anim = anim.legs_anim;
        corpse.torso_anim = anim.torso_anim;
    }
    Some(anim)
}

/// `self suicide()` (`PlayerCmd_Suicide`): health 0 and
/// `player_die` with the client as inflictor and attacker, 100000 damage
/// and `MOD_SUICIDE`, the death anim drawn as any death's, so the corpse
/// the killed callback clones falls (the stock team and spectator switches
/// kill a live player this way, cod4-combat.md 5).
fn suicide(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    if host.client_vitals.get(slot).is_none_or(|v| v.dead) {
        return Ok(Value::Undefined);
    }
    death_grenades(host, cx, slot, recv, "MOD_SUICIDE");
    let death = draw_death_anim(host, slot);
    host.cod4_mut().damage_ops.push(Damage4 {
        slot,
        damage: 0,
        dflags: crate::game::cod4::combat::DFLAG_NO_KNOCKBACK,
        dir: None,
        fatal: true,
        attacker: Some(slot),
        death,
    });
    host.die(slot);
    host.client_vitals[slot].health = 0;
    let Some(me) = host.ents.handle(slot as u32) else {
        return Ok(Value::Undefined);
    };
    let me = Value::Entity(me);
    if let Some(r) = recv {
        let event = cx.intern_folded("death");
        cx.notify(r, event, vec![me]);
    }
    host.cod4_mut().callback = crate::game::host::Cod2CallbackArgs {
        time_offset: 0,
        death_anim_duration: death.map_or(0, |d| d.duration),
    };
    let none = Value::String(cx.intern_exact("none"));
    let killed = cx.func_ref(
        crate::game::script::CALLBACK_SETUP,
        "CodeCallback_PlayerKilled",
    );
    let args = host.callback_args(
        "CodeCallback_PlayerKilled",
        vec![
            me,
            me,
            Value::Int(100_000),
            Value::String(cx.intern_exact("MOD_SUICIDE")),
            none,
            Value::Undefined,
            none,
        ],
    );
    cx.spawn_then(killed, recv, args, slot as u32);
    host.cod4_mut().callback = Default::default();
    Ok(Value::Undefined)
}

/// `perks` bit 7, `specialty_pistoldeath`; bit 6, `specialty_grenadepulldeath`.
const PERK_PISTOL_DEATH: u32 = 0x80;
const PERK_GRENADE_PULL_DEATH: u32 = 0x40;

/// `G_crandom` (0x80cb57a): `rand() * 2^-31` doubled less 1 on the stack,
/// stored as a float.
fn g_crandom(host: &mut GameHost) -> f32 {
    let r = host.cod4_mut().rand();
    (f64::from(r) * f64::from(1.0f32 / 2_147_483_648.0) * 2.0 - 1.0) as f32
}

/// `player_die`'s death grenade drop (`cod4_lnxded` 0x80b1af7, VERIFIED):
/// a grenade held (`grenadeTimeLeft` not 0; the offhand under
/// `PWF_USING_OFFHAND`) drops live with its time left, then, but for a
/// suicide, Martyrdom (`perks` 0x40) drops `perk_grenadeDeath` on its own
/// fuse. Each from 40 over the body at three `G_crandom`s times 160,
/// drawn here, ahead of the death anim's draw; the level fires them.
pub(crate) fn death_grenades(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    recv: Option<Target>,
    mod_name: &str,
) {
    let origin = match recv {
        Some(Target::Entity(id)) => {
            let f = cx.intern_folded("origin");
            match vcod_gsc::Host::get_field(host, cx, id, f) {
                Value::Vector(o) => glam::Vec3::from(o),
                _ => return,
            }
        }
        _ => return,
    };
    let spot = origin + glam::Vec3::Z * 40.0;
    let velocity = |host: &mut GameHost| {
        let v = [g_crandom(host), g_crandom(host), g_crandom(host)];
        glam::Vec3::from(v.map(|x| x * 160.0))
    };
    let c = &host.cod4().clients[slot];
    let (held, perks) = (c.held_grenade, c.perks);
    if let Some((weapon, time)) = held {
        let velocity = velocity(host);
        host.cod4_mut()
            .death_grenades
            .push(crate::game::cod4::combat::DeathGrenade4 {
                slot,
                weapon,
                spot,
                velocity,
                time,
            });
    }
    if mod_name == "MOD_SUICIDE" || perks & PERK_GRENADE_PULL_DEATH == 0 {
        return;
    }
    let name = match host.cvars.get("perk_grenadeDeath") {
        "" => "frag_grenade_short_mp".to_string(),
        n => n.to_string(),
    };
    let Some(weapon) = host.cod4().table.index(&name) else {
        log::warn!("cod4: perk_grenadeDeath {name} is no weapon");
        return;
    };
    let fuse = host
        .cod4()
        .weapon_files
        .get(&name)
        .and_then(|f| f.pm.as_ref())
        .map_or(0, |d| d.fuse_time);
    let velocity = velocity(host);
    host.cod4_mut()
        .death_grenades
        .push(crate::game::cod4::combat::DeathGrenade4 {
            slot,
            weapon: weapon as u32,
            spot,
            velocity,
            time: fuse,
        });
}

/// `obituary(victim, attacker, sWeapon, sMeansOfDeath)` (`GScr_Obituary`):
/// `EV_OBITUARY` broadcast at the world's origin, the victim
/// in `otherEntityNum`, an entity attacker in `attackerEntityNum` (1022
/// otherwise), the parm the weapon's number, or `0x80 |` the means of
/// death for a melee, headshot, crush, fall, suicide or `MOD_IMPACT`
/// (7, 8, 9, 11, 12, 15). The kill capture's parm 27 is the M16's number.
fn obituary(
    host: &mut GameHost,
    cx: &mut Cx,
    _recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let (Some(Value::Entity(victim)), Some(attacker), Some(Value::String(weapon)), Some(means)) =
        (args.first(), args.get(1), args.get(2), args.get(3))
    else {
        return Err(ErrorKind::BadType(
            "obituary takes a victim, an attacker, a weapon name and a means of death",
        ));
    };
    let attacker = match attacker {
        Value::Entity(a) => a.0 as i32,
        _ => 1022,
    };
    let means = match means {
        Value::String(m) => host.defs.mod_index(cx.resolve(*m)).unwrap_or(0),
        _ => 0,
    };
    let parm = if matches!(means, 7 | 8 | 9 | 11 | 12 | 15) {
        0x80 | means
    } else {
        host.cod4().table.index(cx.resolve(*weapon)).unwrap_or(0) as i32
    };
    host.add_temp_entity(
        cx,
        TempEntity {
            event: ev::OBITUARY,
            parm,
            surf_type: 0,
            other: victim.0,
            attacker,
            weapon: 0,
            client_num: 0,
            scale: 0,
            origin: [0.0; 3],
            scope: Scope::Broadcast,
            quake: None,
            origin2: None,
        },
    );
    Ok(Value::Undefined)
}

/// `self clonePlayer(deathAnimDuration)` (`PlayerCmd_ClonePlayer`): a corpse
/// (`eType` 2) in the body queue from 64 (the kill
/// capture's entity 64, VERIFIED live), the client's body where it
/// stands: its `eFlags` with `EF_DEAD`, its anims, the view's yaw, the
/// client number, the death anim `player_die` drew. The fall to rest and
/// the death anim's root motion are left out (cod4-combat.md 5).
fn clone_player(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let p = &crate::game::cod4::COD4_HOST_PROTO;
    // The clone's think, `BodyEnd`, takes the fresh bit out
    // `deathAnimDuration` after its birth (cod4-combat.md 5).
    let duration = match args.first() {
        Some(Value::Int(d)) => *d,
        Some(Value::Float(d)) => *d as i32,
        _ => 0,
    };
    let Some(c) = host.cod4().corpse_source.get(slot).copied().flatten() else {
        return Ok(Value::Undefined);
    };
    let mut e = vcod_common::net::msg::EntityState::null(p);
    let mut set = |name: &str, v: i32| {
        if let Some(i) = vcod_common::net::msg::EntityState::field_index(p, name) {
            e.fields[i] = v;
        }
    };
    set("eType", vcod_common::net::flags::ET_CORPSE);
    set("clientNum", slot as i32);
    // `EF_DEAD` and the fresh corpse's 0x80000 over the body's own (the
    // kill capture's 0xa0002 off the target's 0x2, VERIFIED live).
    set("eFlags", (c.e_flags | 0xa_0000) as i32);
    set("legsAnim", c.legs_anim as i32);
    set("torsoAnim", c.torso_anim as i32);
    set("apos.trBase[1]", c.yaw.to_bits() as i32);
    let now = host.level_time_ms;
    let number = host
        .bodies
        .push_raw(e, now, None, p, Some((0x8_0000, duration)));
    // The clone falls from the body's origin at its velocity; the level's
    // corpse pass lands it and moves it by its death anim
    // (`game::cod4::corpse`, cod4-combat.md 5.2).
    if let Some(b) = host.bodies.body_mut(number) {
        crate::game::cod4::corpse::launch(
            b,
            glam::Vec3::from(c.origin),
            glam::Vec3::from(c.velocity),
            now,
            p,
        );
    }
    let id = host.ents.spawn_body(cx, number);
    host.cod4_mut().corpse_anims.insert(id.0, c.legs_anim);
    Ok(Value::Entity(id))
}

/// What `clonePlayer` reads of a client: its body as the death left it.
#[derive(Clone, Copy, Debug, Default)]
pub struct CorpseSource {
    pub origin: [f32; 3],
    /// `ps.velocity`, the clone's launch.
    pub velocity: [f32; 3],
    pub yaw: f32,
    pub e_flags: u32,
    pub legs_anim: u32,
    pub torso_anim: u32,
}

/// `self dropItem(name [, tag])` (`PlayerCmd_dropItem`, `Drop_Weapon`,
/// `LaunchItem`): a weapon the client holds with ammo to hand
/// over goes into the world as an item (`ET_ITEM`, its class
/// `weapon_<name>`) from the middle of the body's box, along the body's
/// yaw at `g_dropForwardSpeed` (10) and up at `g_dropUpSpeedBase` (10),
/// plus `crandom() * g_dropUpSpeedRand` (5) up and `crandom() *
/// g_dropHorzSpeedRand` (100) on x and y, three draws in that order
/// (0x80915b3..0x80915f9), and leaves the client's weapons
/// (`BG_TakePlayerWeapon`). The start at the tag is left out
/// (cod4-combat.md 6).
fn drop_item(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(weapon) = weapon_arg(host, cx, args.first())? else {
        return Ok(Value::Undefined);
    };
    let has = with_loadout(host, slot, |l, t| {
        let d = t.get(weapon);
        l.has(weapon) && d.is_some_and(|d| l.ammoclip[d.clip_slot] > 0 || l.ammo[d.ammo_slot] > 0)
    });
    if !has {
        with_loadout(host, slot, |l, _| l.take(weapon));
        return Ok(Value::Undefined);
    }
    let Some(id) = launch_weapon(host, cx, slot, weapon)? else {
        return Ok(Value::Undefined);
    };
    with_loadout(host, slot, |l, _| l.take(weapon));
    Ok(Value::Entity(id))
}

/// `Drop_Item`'s launch of `weapon` out of the client in `slot` (see
/// [`drop_item`]): the item, its counts 0 until `itemWeaponSetAmmo`; the
/// client's weapons are left alone. `None` before the client's first end
/// frame.
pub(crate) fn launch_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    weapon: usize,
) -> Result<Option<EntId>, ErrorKind> {
    let Some(c) = host.cod4().corpse_source.get(slot).copied().flatten() else {
        return Ok(None);
    };
    let id = host
        .ents
        .spawn(cx)
        .map_err(|_| ErrorKind::Custom("G_Spawn: no free entities".into()))?;
    let name = host.cod4().table.defs[weapon].name.clone();
    let classname = cx.intern_folded("classname");
    let class = Value::String(cx.intern_exact(&format!("weapon_{name}")));
    let _ = vcod_gsc::Host::set_field(host, cx, id, classname, class);
    let yaw = c.yaw;
    let fwd = glam::Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
    let base = glam::Vec3::from(c.origin) + glam::Vec3::Z * 35.0;
    let cvar = |host: &GameHost, name: &str, default: f32| match host.cvars.get(name) {
        "" => default,
        v => crate::game::builtins::cvar::atof(v),
    };
    let (forward, up_base) = (
        cvar(host, "g_dropForwardSpeed", 10.0),
        cvar(host, "g_dropUpSpeedBase", 10.0),
    );
    let (up_rand, horz) = (
        cvar(host, "g_dropUpSpeedRand", 5.0),
        cvar(host, "g_dropHorzSpeedRand", 100.0),
    );
    // `crandom` (0x8194626): `2 rand() / 2^31 - 1` on the stack, times the
    // float dvar and added to the stored axis.
    let mut crandom =
        || f64::from(host.cod4_mut().rand()) * f64::from(1.0f32 / 2_147_483_648.0) * 2.0 - 1.0;
    let mut delta = fwd * forward;
    delta.z = (crandom() * f64::from(up_rand) + f64::from(up_base) + f64::from(delta.z)) as f32;
    delta.x = (crandom() * f64::from(horz) + f64::from(delta.x)) as f32;
    delta.y = (crandom() * f64::from(horz) + f64::from(delta.y)) as f32;
    let now = host.level_time_ms;
    host.cod4_mut().items.push(Item4 {
        id,
        weapon: weapon as u32,
        owner: slot as u32,
        clear_owner_at: now + 1000,
        base,
        delta,
        yaw,
        start_ms: now,
        resting: false,
        clip: 0,
        stock: 0,
    });
    Ok(Some(id))
}

/// `item itemWeaponSetAmmo(clip, stock [, alt])`: the counts the item
/// hands its taker (the alt's are not kept).
fn item_weapon_set_ammo(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(Target::Entity(id)) = recv else {
        return Ok(Value::Undefined);
    };
    if args.len() != 2 {
        return Ok(Value::Undefined);
    }
    let (clip, stock) = (number(&args[0])?, number(&args[1])?);
    if let Some(it) = host.cod4_mut().items.iter_mut().find(|i| i.id == id) {
        it.clip = clip;
        it.stock = stock;
    }
    Ok(Value::Undefined)
}

/// `self anyAmmoForWeaponModes(name)`: rounds in the clip or the reserve
/// of the weapon or its alt.
fn any_ammo_for_weapon_modes(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    args: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(weapon) = weapon_arg(host, cx, args.first())? else {
        return Ok(Value::Int(0));
    };
    let any = with_loadout(host, slot, |l, t| {
        let mut w = weapon;
        for _ in 0..4 {
            let Some(d) = t.get(w) else { break };
            if l.ammoclip[d.clip_slot] > 0 || l.ammo[d.ammo_slot] > 0 {
                return true;
            }
            w = d.alt;
            if w == 0 || w == weapon {
                break;
            }
        }
        false
    });
    Ok(Value::Int(i32::from(any)))
}

/// `self isOnLadder()`: `pm_flags`' ladder bit as the last cmd left it.
fn is_on_ladder(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let pm = host.cod4().pm_flags.get(slot).copied().unwrap_or(0);
    Ok(Value::Int(i32::from(
        pm & vcod_cod4::pmove::pmf::LADDER != 0,
    )))
}

/// `self isMantling()`: never; the mantle is not modelled.
fn is_mantling(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    client_receiver(host, recv)?;
    Ok(Value::Int(0))
}
