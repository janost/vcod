//! CoD4 1.7 builtins the builtin census (`game::cod4::census`) found the
//! stock scripts calling with no handler: the console stubs, the vector
//! math, the corpse's anim, the effects `_fx.gsc` spawns, the hud element
//! targets, the button reads, the location selector and the rest
//! (docs/research/cod4-gsc.md 3.1). A row's semantics are
//! INFERRED unless its comment says VERIFIED.

use glam::{Mat3, Vec3};
use vcod_gsc::{ArrayKey, Cx, EntId, ErrorKind, Host, Target, Value};

use super::client::client_receiver;
use super::entity::entity_receiver;
use super::score::Builtin;
use crate::game::cod4::Fx4;
use crate::game::host::GameHost;
use crate::game::temp_entity::{Scope, TempEntity};

pub const NAMES: &[(&str, Builtin)] = &[
    ("sendranks", empty),
    ("endparty", empty),
    ("endlobby", empty),
    ("fprintln", empty),
    ("devaddpitch", empty),
    ("devaddyaw", empty),
    ("devaddroll", empty),
    ("setprintchannel", empty),
    ("getassignedteam", zero),
    ("buttonpressed", zero),
    ("sqrt", sqrt),
    ("lengthsquared", length_squared),
    ("combineangles", combine_angles),
    ("pointonsegmentnearesttopoint", point_on_segment),
    ("getweaponmodel", get_weapon_model),
    ("weaponfiretime", weapon_fire_time),
    ("getanimlength", get_anim_length),
    ("animhasnotetrack", anim_has_notetrack),
    ("getnotetracktimes", get_notetrack_times),
    ("getcorpseanim", get_corpse_anim),
    ("geteye", get_eye),
    ("getvelocity", get_velocity),
    ("getammocount", get_ammo_count),
    ("kick", kick),
    ("setteamradar", set_team_radar),
    ("getteamradar", get_team_radar),
    ("physicstrace", physics_trace),
    ("playerphysicstrace", player_physics_trace),
    ("damageconetrace", damage_cone_trace),
    ("sightconetrace", sight_cone_trace),
    ("detonate", detonate),
    ("playsound", play_sound),
    ("playfx", play_fx),
    ("playsoundtoteam", play_sound_to_team),
    ("playsoundasmaster", play_sound_as_master),
    ("spawnfx", spawn_fx),
    ("triggerfx", trigger_fx),
    ("playloopedfx", play_looped_fx),
    ("gettagorigin", get_tag_origin),
    ("playfxontag", play_fx_on_tag),
    ("gettagangles", get_tag_angles),
    ("setdepthoffield", set_depth_of_field),
    ("usebuttonpressed", use_button_pressed),
    ("attackbuttonpressed", attack_button_pressed),
    ("adsbuttonpressed", ads_button_pressed),
    ("fragbuttonpressed", frag_button_pressed),
    ("meleebuttonpressed", melee_button_pressed),
    (
        "secondaryoffhandbuttonpressed",
        secondary_offhand_button_pressed,
    ),
    ("beginlocationselection", begin_location_selection),
    ("endlocationselection", end_location_selection),
    ("updatedmscores", update_dm_scores),
    ("setwaypoint", set_waypoint),
    ("settargetent", set_target_ent),
    ("cleartargetent", clear_target_ent),
    ("setplayernamestring", set_player_name_string),
    ("clearalltextafterhudelem", clear_all_text_after),
    (
        "clientclaimtrigger",
        super::cod2_trigger::client_claim_trigger,
    ),
    (
        "clientreleasetrigger",
        super::cod2_trigger::client_release_trigger,
    ),
    (
        "releaseclaimedtrigger",
        super::cod2_trigger::release_claimed_trigger,
    ),
    (
        "setteamfortrigger",
        super::cod2_trigger::set_team_for_trigger,
    ),
];

/// The host's element type for `HE_TYPE_PLAYERNAME` (1.7's 3, which the
/// host's own numbering spends on materials; `cod4::wire` maps it back).
pub const HOST_PLAYERNAME: i32 = 14;
/// `HE_TYPE_WAYPOINT`, the same 13 as CoD2's.
const HE_TYPE_WAYPOINT: i32 = 13;
/// 1.7's `EV_SOUND_ALIAS` and `EV_SOUND_ALIAS_AS_MASTER`
/// (`vcod_cod4::net::events`).
const EV_SOUND_ALIAS: i32 = 3;
const EV_SOUND_ALIAS_AS_MASTER: i32 = 4;
/// 1.7's `EV_PLAY_FX` (`vcod_cod4::net::events`).
const EV_PLAY_FX: i32 = 55;
/// `physicsTrace`'s and `playerPhysicsTrace`'s mask (`cod4_lnxded`
/// 0x80be7e0, 0x80c1e98).
const MASK_PHYSICS: u32 = 0x82_0011;
/// `damageConeTrace`'s and `sightConeTrace`'s masks (0x80c5d3a, 0x80c5d28).
const MASK_DAMAGE_CONE: u32 = 0x80_2011;
const MASK_SIGHT_CONE: u32 = 0x801;
/// `playerMins`, `playerMaxs` (0x820e024, 0x820e030).
const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// The handlers 1.7 leaves empty (`push ebp; mov ebp,esp; pop ebp; ret`,
/// VERIFIED: `sendRanks` 0x80bb058, `endParty` 0x80bb05e, `endLobby`
/// 0x80bb088, `fprintln` 0x80bb022, `devAddPitch` 0x8099f60, `devAddYaw`
/// 0x8099f66, `devAddRoll` 0x8099f6c), and `setPrintChannel`, a developer
/// row that does nothing at `developer 0`.
fn empty(_: &mut GameHost, _: &mut Cx, _: Option<Target>, _: &[Value]) -> Result<Value, ErrorKind> {
    Ok(Value::Undefined)
}

/// `getAssignedTeam` (0x80bb48e) and `buttonPressed` (0x80a9838) add 0,
/// VERIFIED.
fn zero(_: &mut GameHost, _: &mut Cx, _: Option<Target>, _: &[Value]) -> Result<Value, ErrorKind> {
    Ok(Value::Int(0))
}

fn num(v: Option<&Value>, what: &'static str) -> Result<f32, ErrorKind> {
    match v {
        Some(&Value::Int(i)) => Ok(i as f32),
        Some(&Value::Float(f)) => Ok(f),
        _ => Err(ErrorKind::BadType(what)),
    }
}

fn vec(v: Option<&Value>, what: &'static str) -> Result<Vec3, ErrorKind> {
    match v {
        Some(&Value::Vector(v)) => Ok(Vec3::from(v)),
        _ => Err(ErrorKind::BadType(what)),
    }
}

fn string(cx: &Cx, v: Option<&Value>, what: &'static str) -> Result<String, ErrorKind> {
    match v {
        Some(&Value::String(s)) => Ok(cx.resolve(s).to_string()),
        _ => Err(ErrorKind::BadType(what)),
    }
}

fn sqrt(_: &mut GameHost, _: &mut Cx, _: Option<Target>, a: &[Value]) -> Result<Value, ErrorKind> {
    Ok(Value::Float(num(a.first(), "sqrt takes a number")?.sqrt()))
}

fn length_squared(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Float(
        vec(a.first(), "lengthSquared takes a vector")?.length_squared(),
    ))
}

/// `AnglesToAxis`: forward, left, up as rows.
fn axis(angles: Vec3) -> [Vec3; 3] {
    let [f, r, u] = vcod_common::x87::angle_vectors(angles);
    [f, -r, u]
}

/// `AxisToAngles`: pitch and yaw off the forward row as `vectoangles`
/// reads them (0..360), roll off the left and up rows' heights.
fn axis_to_angles(a: [Vec3; 3]) -> Vec3 {
    let f = a[0];
    let mut yaw = f.y.atan2(f.x).to_degrees();
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let mut pitch = (-f.z).atan2(f.x.hypot(f.y)).to_degrees();
    if pitch < 0.0 {
        pitch += 360.0;
    }
    let roll = a[1].z.atan2(a[2].z).to_degrees();
    Vec3::new(pitch, yaw, roll)
}

/// `MatrixMultiply(a, b)`: each row of `a` through `b`'s rows.
fn mul(a: [Vec3; 3], b: [Vec3; 3]) -> [Vec3; 3] {
    a.map(|r| b[0] * r.x + b[1] * r.y + b[2] * r.z)
}

/// `combineAngles(a, b)`: `AxisToAngles(axis(b) * axis(a))`.
fn combine_angles(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "combineAngles takes two vectors";
    let (x, y) = (vec(a.first(), USAGE)?, vec(a.get(1), USAGE)?);
    Ok(Value::Vector(
        axis_to_angles(mul(axis(y), axis(x))).to_array(),
    ))
}

/// `pointOnSegmentNearestToPoint(a, b, p)`: the nearest point, clamped to
/// the ends; a zero-length segment is an error.
fn point_on_segment(
    _: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    const USAGE: &str = "pointOnSegmentNearestToPoint takes three vectors";
    let (s, e, p) = (
        vec(a.first(), USAGE)?,
        vec(a.get(1), USAGE)?,
        vec(a.get(2), USAGE)?,
    );
    let ba = e - s;
    let len = ba.length_squared();
    if len == 0.0 {
        return Err(ErrorKind::Custom(
            "Line segment must not have zero length".into(),
        ));
    }
    let f = ba.dot(p - s) / len;
    let out = if f < 0.0 {
        s
    } else if f > 1.0 {
        e
    } else {
        s + ba * f
    };
    Ok(Value::Vector(out.to_array()))
}

/// The registered weapon a name argument names, with its file.
fn weapon_file(
    host: &GameHost,
    cx: &Cx,
    v: Option<&Value>,
) -> Result<Option<std::rc::Rc<crate::game::cod4::WeaponFile>>, ErrorKind> {
    let name = string(cx, v, "takes a weapon name")?.to_ascii_lowercase();
    if host.cod4().table.index(&name).is_none() {
        return Ok(None);
    }
    Ok(host.cod4().weapon_files.get(&name).cloned())
}

/// `getWeaponModel(weapon[, index])`: `worldModel[index]` (index 0 when
/// that one is empty or out of range), `""` for an unknown weapon.
fn get_weapon_model(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(file) = weapon_file(host, cx, a.first())? else {
        return Ok(Value::String(cx.intern_exact("")));
    };
    let k = match a.get(1) {
        Some(&Value::Int(i)) => usize::try_from(i).ok().filter(|&i| i < 256).unwrap_or(0),
        _ => 0,
    };
    let pick = |k: usize| file.world_models.get(k).cloned().flatten();
    let name = pick(k).or_else(|| pick(0)).unwrap_or_default();
    Ok(Value::String(cx.intern_exact(&name)))
}

/// `weaponFireTime(weapon)`: `fireTime` in seconds, 0 for an unknown
/// weapon.
fn weapon_fire_time(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let t = weapon_file(host, cx, a.first())?
        .and_then(|f| f.pm.as_ref().map(|d| d.fire_time))
        .unwrap_or(0);
    Ok(Value::Float(t as f32 * 0.001))
}

/// An anim argument's clip, by its name.
fn clip_of(
    host: &GameHost,
    cx: &Cx,
    v: Option<&Value>,
) -> Result<std::rc::Rc<crate::game::cod4::AnimClip>, ErrorKind> {
    let name = match v {
        Some(&Value::Anim(a)) | Some(&Value::String(a)) => cx.resolve(a).to_ascii_lowercase(),
        _ => return Err(ErrorKind::BadType("takes an anim")),
    };
    host.cod4()
        .anim_clips
        .get(&name)
        .cloned()
        .ok_or_else(|| ErrorKind::Custom(format!("unknown anim '{name}'")))
}

/// `getAnimLength(anim)`: `XAnimGetLength`, `1 / frequency` seconds.
fn get_anim_length(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Float(clip_of(host, cx, a.first())?.length))
}

/// `animHasNotetrack(anim, name)`.
fn anim_has_notetrack(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let clip = clip_of(host, cx, a.first())?;
    let note = string(cx, a.get(1), "animHasNotetrack takes a notetrack")?;
    Ok(Value::Int(i32::from(
        clip.notes
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(&note)),
    )))
}

/// `getNotetrackTimes(anim, name)`: each of the note's times as a fraction
/// of the clip, in the clip's order.
fn get_notetrack_times(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let clip = clip_of(host, cx, a.first())?;
    let note = string(cx, a.get(1), "getNotetrackTimes takes a notetrack")?;
    let arr = cx.new_array();
    for (i, (_, t)) in clip
        .notes
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case(&note))
        .enumerate()
    {
        cx.set_index(arr, ArrayKey::Int(i as i32), Value::Float(*t));
    }
    Ok(Value::Array(arr))
}

/// `corpse getCorpseAnim()`: the body's `legsAnim` (toggle bit off) as the
/// player animtree names it; anything but a player corpse is an error.
fn get_corpse_anim(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let Some(&legs) = host.cod4().corpse_anims.get(&id.0) else {
        return Err(ErrorKind::Custom("Only valid on player corpses".into()));
    };
    let name = host
        .cod4()
        .anim_names
        .get((legs & 0x1ff) as usize)
        .cloned()
        .unwrap_or_default();
    Ok(Value::Anim(cx.intern_exact(&name)))
}

/// An entity's origin and angles as the script reads them.
fn place(host: &mut GameHost, cx: &mut Cx, id: vcod_gsc::EntId) -> (Vec3, Vec3) {
    let mut read = |n: &str| {
        let f = cx.intern_folded(n);
        match host.get_field(cx, id, f) {
            Value::Vector(v) => Vec3::from(v),
            _ => Vec3::ZERO,
        }
    };
    (read("origin"), read("angles"))
}

/// `ent getEye()` (0x80c2eea): `r.currentOrigin` 40 up (0x820ded4),
/// whatever the entity and its stance.
fn get_eye(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let (o, _) = place(host, cx, id);
    Ok(Value::Vector((o + Vec3::Z * 40.0).to_array()))
}

/// `player getVelocity()`: `ps.velocity` after its last cmd.
fn get_velocity(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    Ok(Value::Vector(host.cod4().velocity[slot]))
}

/// `player getAmmoCount(weapon)`: `BG_WeaponAmmo`, the clip and the
/// reserve (a clip-only weapon its clip); 0 for a weapon it does not have.
fn get_ammo_count(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let Some(w) = super::cod4_client::weapon_arg(host, cx, a.first())? else {
        return Ok(Value::Int(0));
    };
    let c4 = host.cod4();
    let l = &c4.clients[slot].loadout;
    if l.weapons[w / 32] & (1 << (w % 32)) == 0 {
        return Ok(Value::Int(0));
    }
    let d = &c4.table.defs[w];
    let clip = l.ammoclip[d.clip_slot];
    Ok(Value::Int(if d.clip_only {
        clip
    } else {
        clip + l.ammo[d.ammo_slot]
    }))
}

/// `kick(entnum)`: `tempBanClient <n>` on the console (`GScr_KickPlayer`'s
/// `Cbuf_AddText`), which the server runs after the frame.
fn kick(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    if let Some(&Value::Int(n)) = a.first() {
        host.console.push(format!("tempBanClient {n}"));
    }
    Ok(Value::Undefined)
}

/// `teamHasRadar`'s index for `allies` (2), `axis` (1) or `none` (0).
fn radar_team(cx: &Cx, v: Option<&Value>) -> Result<usize, ErrorKind> {
    let t = string(cx, v, "takes a team")?;
    match t.as_str() {
        "allies" => Ok(2),
        "axis" => Ok(1),
        "none" => Ok(0),
        _ => Err(ErrorKind::Custom(format!(
            "Illegal team string '{t}'. Must be allies, axis, or none."
        ))),
    }
}

/// `setTeamRadar(team, on)`: `level.teamHasRadar`, which each end frame
/// writes to the team's `ps.radarEnabled`.
fn set_team_radar(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let t = radar_team(cx, a.first())?;
    let on = a.get(1).map(|v| v.as_bool()).transpose()?.unwrap_or(false);
    host.cod4_mut().team_radar[t] = on;
    Ok(Value::Undefined)
}

fn get_team_radar(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let t = radar_team(cx, a.first())?;
    Ok(Value::Int(i32::from(host.cod4().team_radar[t])))
}

/// A trace through the level's world (`cod4_trace::trace4`: the clip
/// map, the brush entities and, under a body mask, the bodies), `None`
/// without a clip map.
fn world_trace(
    host: &GameHost,
    s: Vec3,
    e: Vec3,
    mins: Vec3,
    maxs: Vec3,
    mask: u32,
) -> Option<f32> {
    super::cod4_trace::trace4(host, s, e, mins, maxs, 1023, mask).map(|t| t.fraction)
}

fn traced(host: &GameHost, a: &[Value], mins: Vec3, maxs: Vec3) -> Result<Value, ErrorKind> {
    const USAGE: &str = "takes a start and an end";
    let (s, e) = (vec(a.first(), USAGE)?, vec(a.get(1), USAGE)?);
    let f = world_trace(host, s, e, mins, maxs, MASK_PHYSICS).unwrap_or(1.0);
    Ok(Value::Vector(s.lerp(e, f).to_array()))
}

/// `physicsTrace(start, end)`: a point trace under 0x820011, the point it
/// stops at.
fn physics_trace(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    traced(host, a, Vec3::ZERO, Vec3::ZERO)
}

/// `playerPhysicsTrace(start, end)`: the same with the player's box.
fn player_physics_trace(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    traced(host, a, PLAYER_MINS, PLAYER_MAXS)
}

/// `CanDamage` from `origin` to `ent`: a client's five points (the middle
/// of the body and four 15 to the sides, half the eye's height up and
/// down; `Cod4Level::body_share`'s), any other entity's origin; the
/// share of them in clear sight, 0 / 1/3 / 2/3 / 1.
fn cone_trace(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
    mask: u32,
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let from = vec(a.first(), "takes an origin")?;
    let (origin, _) = place(host, cx, id);
    let is_client = host.ents.get(id).is_some_and(|e| e.client.is_some());
    let points: Vec<Vec3> = if is_client {
        let eye = Vec3::from(host.client_aim[id.0 as usize].0);
        let half = (eye.z - origin.z) * 0.5;
        let mut fwd = from - origin;
        fwd.z = 0.0;
        let fwd = fwd.normalize_or_zero();
        let right = Vec3::new(-fwd.y, fwd.x, 0.0);
        let mid = (eye + origin) * 0.5;
        vec![
            mid,
            mid + right * 15.0 + Vec3::Z * half,
            mid + right * 15.0 - Vec3::Z * half,
            mid - right * 15.0 + Vec3::Z * half,
            mid - right * 15.0 - Vec3::Z * half,
        ]
    } else {
        vec![origin]
    };
    let hits = points
        .iter()
        .filter(|&&p| {
            world_trace(host, from, p, Vec3::ZERO, Vec3::ZERO, mask).unwrap_or(1.0) == 1.0
        })
        .count();
    let share = match (hits, points.len()) {
        (0, _) => 0.0,
        (h, n) if h == n || h > 3 => 1.0,
        (h, _) => h as f32 / 3.0,
    };
    Ok(Value::Float(share))
}

fn damage_cone_trace(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    cone_trace(host, cx, recv, a, MASK_DAMAGE_CONE)
}

fn sight_cone_trace(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    cone_trace(host, cx, recv, a, MASK_SIGHT_CONE)
}

/// `grenade detonate([attacker])`: the missile explodes on the level's
/// next missile pass (`G_ExplodeMissile`), its parent the attacker when
/// one is given (the world for `undefined`).
fn detonate(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    if !host.cod4().missiles.iter().any(|m| m.id == id) {
        return Err(ErrorKind::Custom("entity is not a grenade".into()));
    }
    let parent = match a.first() {
        None => None,
        Some(Value::Entity(p)) => match host.ents.get(*p) {
            Some(e) if e.client.is_some() => Some(Some(p.0 as usize)),
            _ => return Err(ErrorKind::Custom("Entity is not a player".into())),
        },
        Some(_) => Some(None),
    };
    host.cod4_mut().detonations.push((id, parent));
    Ok(Value::Undefined)
}

/// `G_SoundAliasIndex` and the alias's number as the event parm carries it.
fn alias_parm(host: &mut GameHost, cx: &mut Cx, a: &[Value]) -> Result<i32, ErrorKind> {
    Ok((super::sound::alloc_alias(host, cx, a)? - super::sound::sounds_base(host)) as i32)
}

fn sound_temp(origin: Vec3, event: i32, parm: i32, scope: Scope) -> TempEntity {
    TempEntity {
        event,
        parm,
        surf_type: 0,
        other: 0,
        attacker: 0,
        weapon: 0,
        client_num: 0,
        scale: 0,
        origin: origin.to_array(),
        scope,
        quake: None,
        origin2: None,
    }
}

/// `ent playSoundToTeam(alias, team[, ignore])`: an `EV_SOUND_ALIAS` temp
/// entity at the entity, broadcast, its `r.clientMask` open to the team's
/// clients but `ignore`.
fn play_sound_to_team(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let team = match string(cx, a.get(1), "playSoundToTeam takes a team")?.as_str() {
        t @ ("allies" | "axis") => t.to_string(),
        t => {
            return Err(ErrorKind::Custom(format!(
                "Illegal team string '{t}'. Must be allies, or axis."
            )));
        }
    };
    let ignore = match a.get(2) {
        Some(Value::Entity(p)) => match host.ents.get(*p) {
            Some(e) if e.client.is_some() => Some(p.0 as usize),
            _ => {
                return Err(ErrorKind::Custom(format!("entity {} is not a player", p.0)));
            }
        },
        _ => None,
    };
    let parm = alias_parm(host, cx, a)?;
    let (origin, _) = place(host, cx, id);
    let mut mask = 0u64;
    let sessionteam = cx.intern_folded("sessionteam");
    for slot in host.client_slots() {
        let Some(e) = host.ents.handle(slot as u32) else {
            continue;
        };
        let on = match host.get_field(cx, e, sessionteam) {
            Value::String(t) => cx.resolve(t) == team,
            _ => false,
        };
        if Some(slot) != ignore && on {
            mask |= 1 << slot;
        }
    }
    host.add_temp_entity(
        cx,
        sound_temp(origin, EV_SOUND_ALIAS, parm, Scope::Clients(mask)),
    );
    Ok(Value::Undefined)
}

/// `ent playSound(alias)`: an `EV_SOUND_ALIAS` temp entity at the
/// entity, broadcast (the script sound start, INFERRED).
/// The shared handler raised CoD1's event number in the entity's ring,
/// which no CoD4 client names.
fn play_sound(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let parm = alias_parm(host, cx, a)?;
    let (origin, _) = place(host, cx, id);
    host.add_temp_entity(
        cx,
        sound_temp(origin, EV_SOUND_ALIAS, parm, Scope::Broadcast),
    );
    Ok(Value::Undefined)
}

/// `ent playSoundAsMaster(alias)`: an `EV_SOUND_ALIAS_AS_MASTER` temp
/// entity at the entity, broadcast.
fn play_sound_as_master(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = entity_receiver(recv)?;
    let parm = alias_parm(host, cx, a)?;
    let (origin, _) = place(host, cx, id);
    host.add_temp_entity(
        cx,
        sound_temp(origin, EV_SOUND_ALIAS_AS_MASTER, parm, Scope::Broadcast),
    );
    Ok(Value::Undefined)
}

/// `Scr_SetFxAngles` (`cod4_lnxded` 0x80c6852): no direction (270, 0, 0),
/// pointing straight up (0x80c6869), a forward its `vectoangles`, a
/// forward and an up the axis they make (the up made square to the
/// forward first).
fn fx_angles(forward: Option<Vec3>, up: Option<Vec3>, fx: i32) -> Result<[f32; 3], ErrorKind> {
    let nonzero = |v: Vec3, what: &str| {
        let n = v.normalize_or_zero();
        if n == Vec3::ZERO {
            Err(ErrorKind::Custom(format!(
                "{what} called with (0 0 0) direction (effect {fx})"
            )))
        } else {
            Ok(n)
        }
    };
    let Some(f) = forward else {
        return Ok([270.0, 0.0, 0.0]);
    };
    let f = nonzero(f, "forward")?;
    let Some(u) = up else {
        return Ok(axis_to_angles([f, Vec3::ZERO, Vec3::Z]).to_array());
    };
    let u = nonzero(u, "up")?;
    let u = (u - f * f.dot(u)).normalize_or_zero();
    if u == Vec3::ZERO {
        return Err(ErrorKind::Custom(
            "forward and up vectors are the same direction or exact opposite directions".into(),
        ));
    }
    Ok(axis_to_angles([f, u.cross(f), u]).to_array())
}

/// A `G_Spawn`ed effect entity, broadcast, at `origin`.
fn spawn_fx_entity(
    host: &mut GameHost,
    cx: &mut Cx,
    origin: Vec3,
    fx: Fx4,
) -> Result<Value, ErrorKind> {
    let id = host.ents.spawn(cx)?;
    let classname = cx.intern_folded("classname");
    let noclass = Value::String(cx.intern_exact("noclass"));
    host.set_field(cx, id, classname, noclass)?;
    let og = cx.intern_folded("origin");
    host.set_field(cx, id, og, Value::Vector(origin.to_array()))?;
    host.cod4_mut().fx.insert(id, fx);
    Ok(Value::Entity(id))
}

/// `playFx(fx, origin[, forward[, up]])`: an `EV_PLAY_FX` temp entity at
/// `origin`, the id's low byte its parm and `Scr_SetFxAngles`' angles in
/// its `apos.trBase` (here its `origin2`, which `cod4::combat`'s
/// `temp_entities4` moves there) (`Scr_PlayFX`, INFERRED). The
/// shared handler raised CoD1's event number, which no CoD4 client names.
fn play_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    if !(2..=4).contains(&a.len()) {
        return Err(ErrorKind::Custom("Incorrect number of parameters".into()));
    }
    let fx = num(a.first(), "playFx takes an effect id")? as i32;
    let origin = vec(a.get(1), "playFx takes an origin")?;
    let forward = a
        .get(2)
        .map(|v| vec(Some(v), "playFx takes a forward"))
        .transpose()?;
    let up = a
        .get(3)
        .map(|v| vec(Some(v), "playFx takes an up"))
        .transpose()?;
    let angles = fx_angles(forward, up, fx)?;
    host.add_temp_entity(
        cx,
        TempEntity {
            origin2: Some(angles),
            ..sound_temp(origin, EV_PLAY_FX, fx & 0xff, Scope::Pvs)
        },
    );
    Ok(Value::Undefined)
}

/// `spawnFx(fx, origin[, forward[, up]])`: an `ET_FX` entity whose effect
/// waits for `triggerFx`.
fn spawn_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    if !(2..=4).contains(&a.len()) {
        return Err(ErrorKind::Custom("Incorrect number of parameters".into()));
    }
    let fx = num(a.first(), "spawnFx takes an effect id")? as i32;
    let origin = vec(a.get(1), "spawnFx takes an origin")?;
    let forward = a
        .get(2)
        .map(|v| vec(Some(v), "spawnFx takes a forward"))
        .transpose()?;
    let up = a
        .get(3)
        .map(|v| vec(Some(v), "spawnFx takes an up"))
        .transpose()?;
    let angles = fx_angles(forward, up, fx)?;
    let rec = Fx4 {
        etype: crate::game::cod4::ET_FX,
        fx: fx & 0xff,
        origin: origin.to_array(),
        angles,
        time2: 0,
        cull: 0.0,
        period_ms: 0,
    };
    spawn_fx_entity(host, cx, origin, rec)
}

/// `triggerFx(fx[, time])`: the effect starts at `time` seconds of level
/// time (`time2`), now without one.
fn trigger_fx(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let Some(&Value::Entity(id)) = a.first() else {
        return Err(ErrorKind::BadType("triggerFx takes an entity"));
    };
    let at = match a.get(1) {
        Some(v) => (num(Some(v), "triggerFx takes a time")? * 1000.0).round() as i32,
        None => host.level_time_ms,
    };
    match host.cod4_mut().fx.get_mut(&id) {
        Some(f) if f.etype == crate::game::cod4::ET_FX => f.time2 = at,
        _ => {
            return Err(ErrorKind::Custom(
                "entity wasn't created with 'newFx'".into(),
            ));
        }
    }
    Ok(Value::Undefined)
}

/// `playLoopedFx(fx, repeat, origin[, cull[, forward[, up]]])`: an
/// `ET_LOOP_FX` entity, its period in ms and cull distance in
/// `lerp.u.loopFx`.
fn play_looped_fx(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    if !(3..=6).contains(&a.len()) {
        return Err(ErrorKind::Custom("Incorrect number of parameters".into()));
    }
    let fx = num(a.first(), "playLoopedFx takes an effect id")? as i32;
    let period_ms = (num(a.get(1), "playLoopedFx takes a repeat")? * 1000.0).round() as i32;
    let origin = vec(a.get(2), "playLoopedFx takes an origin")?;
    let cull = a
        .get(3)
        .map(|v| num(Some(v), "playLoopedFx takes a cull distance"))
        .transpose()?;
    let forward = a
        .get(4)
        .map(|v| vec(Some(v), "playLoopedFx takes a forward"))
        .transpose()?;
    let up = a
        .get(5)
        .map(|v| vec(Some(v), "playLoopedFx takes an up"))
        .transpose()?;
    if period_ms <= 0 {
        return Err(ErrorKind::Custom(format!(
            "playLoopedFx called with repeat < 0.001 seconds (effect {fx})"
        )));
    }
    let rec = Fx4 {
        etype: crate::game::cod4::ET_LOOP_FX,
        fx: fx & 0xff,
        origin: origin.to_array(),
        angles: fx_angles(forward, up, fx)?,
        time2: 0,
        cull: cull.unwrap_or(0.0),
        period_ms,
    };
    spawn_fx_entity(host, cx, origin, rec)
}

/// A playing body's tag `name` as the last frame posed it
/// (`G_DObjGetWorldTagPos` over the client's DObj), its
/// attachments' bones included: (origin, axis rows). `None` for an entity
/// no frame posed or a bone it lacks.
pub(crate) fn posed_tag(host: &GameHost, id: EntId, name: &str) -> Option<(Vec3, [Vec3; 3])> {
    let p = host.cod4().posed_bodies.iter().find(|p| p.entity == id.0)?;
    let skel = &p.rig.skeleton;
    let i = skel
        .bones()
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case(name))?;
    let (pos, rot) = p.pose.bone_world(skel, i);
    let (_, turn, _) = p.placed.to_scale_rotation_translation();
    let m = Mat3::from_quat(turn * rot);
    Some((
        p.placed.transform_point3(pos),
        [m.x_axis, m.y_axis, m.z_axis],
    ))
}

/// The receiver's tag `name`: a playing body's posed ([`posed_tag`]),
/// anything else's in its model's bind pose placed by the entity's origin
/// and angles: (origin, axis rows).
fn tag(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<(Vec3, [Vec3; 3]), ErrorKind> {
    let id = entity_receiver(recv)?;
    let name = string(cx, a.first(), "takes a tag name")?;
    let model_atom = cx.intern_folded("model");
    let mut model = match host.get_field(cx, id, model_atom) {
        Value::String(s) => cx.resolve(s).to_string(),
        _ => String::new(),
    };
    if let Some(t) = posed_tag(host, id, &name) {
        return Ok(t);
    }
    let (mut origin, mut angles) = place(host, cx, id);
    if let Some(m) = host.cod4().missiles.iter().find(|m| m.id == id) {
        origin = m.origin;
        angles = m.angles;
        if model.is_empty() {
            let w = host
                .cod4()
                .weapons
                .get((m.weapon as usize).wrapping_sub(1))
                .cloned();
            model = w
                .and_then(|w| {
                    host.cod4()
                        .weapon_files
                        .get(&w.to_ascii_lowercase())
                        .cloned()
                })
                .and_then(|f| f.models.last().cloned())
                .unwrap_or_default();
        }
    }
    let tags = host
        .cod4()
        .model_tags
        .get(&model.to_ascii_lowercase())
        .cloned();
    let Some(&(_, pos, rot)) = tags
        .as_ref()
        .and_then(|t| t.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(&name)))
    else {
        return Err(ErrorKind::Custom(format!(
            "tag '{name}' does not exist in model '{model}' (or any attached submodels)"
        )));
    };
    let e = axis(angles);
    let m = Mat3::from_quat(rot);
    let bone = [m.x_axis, m.y_axis, m.z_axis];
    Ok((
        origin + e[0] * pos.x + e[1] * pos.y + e[2] * pos.z,
        mul(bone, e),
    ))
}

/// `ent getTagOrigin(tag)`.
fn get_tag_origin(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Vector(tag(host, cx, recv, a)?.0.to_array()))
}

/// `ent getTagAngles(tag)`.
fn get_tag_angles(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(Value::Vector(
        axis_to_angles(tag(host, cx, recv, a)?.1).to_array(),
    ))
}

/// 1.7's `EV_PLAY_FX_ON_TAG` (`vcod_cod4::net::events`, 56).
const EV_PLAY_FX_ON_TAG: i32 = 56;

/// `playFxOnTag(fx, entity, tag)`: the effect id 1..99 (`MAX_EFFECT_NAMES`
/// 100), the entity's model carrying the tag (its bind pose's bones,
/// `model_tags`), `"%02d%s"` of the id and the tag indexed into the effect
/// tags from 1698, and `EV_PLAY_FX_ON_TAG` with that index on the entity
/// (`Scr_PlayFXOnTag`). A player's event is left out: the level
/// keeps its own playerstate ring.
fn play_fx_on_tag(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let [Value::Int(fx), Value::Entity(ent), Value::String(tag)] = a else {
        return Err(ErrorKind::BadType(
            "USAGE: playFxOnTag <effect id from loadFx> <entity> <tag name>",
        ));
    };
    let (fx, ent) = (*fx, *ent);
    if !(1..100).contains(&fx) {
        return Err(ErrorKind::Custom(format!("effect id {fx} is invalid")));
    }
    let tag = cx.resolve(*tag).to_string();
    if tag.contains('"') {
        return Err(ErrorKind::Custom(
            "cannot use \" characters in tag names".into(),
        ));
    }
    let model_atom = cx.intern_folded("model");
    let model = match host.get_field(cx, ent, model_atom) {
        Value::String(s) => cx.resolve(s).to_ascii_lowercase(),
        _ => String::new(),
    };
    if model.is_empty() {
        return Err(ErrorKind::Custom(
            "cannot play fx on entity with no model".into(),
        ));
    }
    let known = host
        .cod4()
        .model_tags
        .get(&model)
        .is_some_and(|t| t.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(&tag)));
    if !known {
        return Err(ErrorKind::Custom(format!(
            "tag '{tag}' does not exist on entity with model '{model}'"
        )));
    }
    let parm =
        host.allocators
            .fx_tag_index(&mut host.configstrings, fx, &tag.to_ascii_lowercase())?;
    if let Some(e) = host.ents.get_mut(ent)
        && e.client.is_none()
    {
        e.events.add(EV_PLAY_FX_ON_TAG, parm);
    }
    Ok(Value::Undefined)
}

/// `player setDepthOfField(nearStart, nearEnd, farStart, farEnd,
/// nearBlur, farBlur)`: the playerstate's six `dof*` fields at the end
/// frame.
fn set_depth_of_field(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let mut v = [0.0f32; 6];
    for (i, x) in v.iter_mut().enumerate() {
        *x = num(a.get(i), "setDepthOfField takes six numbers")?;
    }
    host.cod4_mut().clients[slot].dof = Some(v);
    Ok(Value::Undefined)
}

/// `(buttons | buttonsSinceLastFrame) & bit` of the client's cmds.
fn button(host: &GameHost, recv: Option<Target>, bit: u32) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let c = &host.cod4();
    Ok(Value::Int(i32::from(
        (c.buttons[slot] | c.buttons_since[slot]) & bit != 0,
    )))
}

/// `useButtonPressed` (`cod4_lnxded` 0x80a9b46) tests 0x28, `+activate`
/// or `+usereload`; `attackButtonPressed` (0x80a9aae) 0x1 and
/// `adsButtonPressed` (0x80a9a14) 0x800. A killcam's follower's cmds
/// count too (its `SpectatorThink` stores them): the stock
/// `_killcam::waitSkipKillcamButton` polls the use key (cod4-combat.md 7).
fn use_button_pressed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    button(host, recv, 0x28)
}

fn attack_button_pressed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    button(host, recv, 0x1)
}

fn ads_button_pressed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    button(host, recv, 0x800)
}

fn frag_button_pressed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    button(host, recv, 0x4000)
}

fn melee_button_pressed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    button(host, recv, 0x4)
}

fn secondary_offhand_button_pressed(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    button(host, recv, 0x8000)
}

/// `player beginLocationSelection(material[, radius])`:
/// `ps.locationSelectionInfo` the material's place among the three
/// selector slots (1-based) and the radius over the minimap's height in
/// six bits (0.15 without one).
fn begin_location_selection(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let name = string(cx, a.first(), "beginLocationSelection takes a material")?;
    let base = crate::game::cod4::CS_LOC_SEL_MTLS;
    let index = (base..base + 3)
        .position(|i| host.configstrings[i].eq_ignore_ascii_case(&name))
        .ok_or_else(|| ErrorKind::Custom(format!("location selector '{name}' not precached")))?
        + 1;
    let radius = match a.get(1) {
        None => 0.15,
        Some(v) => {
            let r = num(Some(v), "beginLocationSelection takes a radius")?;
            if r <= 0.0 {
                return Err(ErrorKind::Custom(
                    "Radius of location selector must be greater than zero".into(),
                ));
            }
            let h = crate::game::cod4::compass(host).map_or(0.0, |c| c.world_size[1]);
            (if h <= 0.0 { r / 1000.0 } else { r / h }).clamp(0.0, 1.0)
        }
    };
    let bits = (radius * 63.0).round() as u32;
    host.cod4_mut().clients[slot].location_selection = Some(index as u32 | bits << 2);
    Ok(Value::Undefined)
}

fn end_location_selection(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    host.cod4_mut().clients[slot].location_selection = Some(0);
    Ok(Value::Undefined)
}

/// `player updateDMScores()`: `I <client> <score>` for the player, then
/// for the best other client by the scoreboard's order, ignorable.
fn update_dm_scores(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let slot = client_receiver(host, recv)?;
    let score_of = |host: &mut GameHost, cx: &mut Cx, s: usize| {
        let f = cx.intern_folded("score");
        match host
            .ents
            .handle(s as u32)
            .map(|id| host.get_field(cx, id, f))
        {
            Some(Value::Int(i)) => i,
            _ => 0,
        }
    };
    let mine = score_of(host, cx, slot);
    host.client_commands
        .push((slot, format!("I {slot} {mine}")));
    let best = host
        .client_slots()
        .into_iter()
        .filter(|&s| s != slot)
        .map(|s| (score_of(host, cx, s), s))
        .collect::<Vec<_>>();
    if let Some(&(score, s)) = best
        .iter()
        .max_by_key(|&&(sc, s)| (sc, std::cmp::Reverse(s)))
    {
        host.client_commands.push((slot, format!("I {s} {score}")));
    }
    Ok(Value::Undefined)
}

/// `hud setWaypoint(constantSize[, offscreenMaterial])`: type 13, the flag
/// in `value`, the material's index in `offscreenMaterialIdx` (0 without).
fn set_waypoint(
    host: &mut GameHost,
    cx: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::hud::hud_receiver(host, recv)?;
    let on = num(a.first(), "setWaypoint takes a boolean")? as i32;
    let material = match a.get(1) {
        Some(v) => {
            let name = string(cx, Some(v), "setWaypoint takes a material")?;
            let name = host.shader_name(&name);
            host.allocators
                .shader_index(&mut host.configstrings, &name, host.initializing)?
        }
        None => 0,
    };
    super::hud::with_hud(host, id, |h| {
        h.elem_type = HE_TYPE_WAYPOINT;
        h.value = on as f32;
        h.offscreen_material = material;
    })?;
    Ok(Value::Undefined)
}

/// `hud setTargetEnt(ent)`: `targetEntNum`.
fn set_target_ent(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::hud::hud_receiver(host, recv)?;
    let Some(&Value::Entity(t)) = a.first() else {
        return Err(ErrorKind::BadType("setTargetEnt takes an entity"));
    };
    super::hud::with_hud(host, id, |h| h.target_ent = t.0)?;
    Ok(Value::Undefined)
}

fn clear_target_ent(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::hud::hud_receiver(host, recv)?;
    super::hud::with_hud(host, id, |h| {
        h.target_ent = crate::game::entity::HUD_OWNER_ALL;
    })?;
    Ok(Value::Undefined)
}

/// `hud setPlayerNameString(player)`: the type settings cleared, type
/// `HE_TYPE_PLAYERNAME`, the client's number in `value`.
fn set_player_name_string(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::hud::hud_receiver(host, recv)?;
    let Some(&Value::Entity(p)) = a.first() else {
        return Ok(Value::Undefined);
    };
    if host.ents.get(p).is_none_or(|e| e.client.is_none()) {
        return Ok(Value::Undefined);
    }
    super::hud::with_hud(host, id, |h| {
        h.clear_payload();
        h.elem_type = HOST_PLAYERNAME;
        h.value = p.0 as f32;
    })?;
    Ok(Value::Undefined)
}

/// `hud clearAllTextAfterHudElem()`: every localized string past the
/// element's text emptied.
fn clear_all_text_after(
    host: &mut GameHost,
    _: &mut Cx,
    recv: Option<Target>,
    _: &[Value],
) -> Result<Value, ErrorKind> {
    let id = super::hud::hud_receiver(host, recv)?;
    let text = host.ents.get(id).and_then(|e| e.hud).map_or(0, |h| h.text);
    if text == 0 {
        return Err(ErrorKind::Custom(
            "Hud elem doesn't reference any text.  Make sure to call setText before using clearAllTextAfterHudElem.".into(),
        ));
    }
    let (first, last) = host
        .allocators
        .layout()
        .bounds(crate::configstrings::CsRange::Localized);
    let base = first - 1;
    for slot in base + text as usize + 1..=last {
        host.configstrings.set(slot, String::new());
    }
    Ok(Value::Undefined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 1e-3
    }

    #[test]
    fn axis_to_angles_undoes_angles_to_axis() {
        for a in [
            Vec3::new(10.0, 30.0, 0.0),
            Vec3::new(-20.0, 200.0, 15.0),
            Vec3::new(45.0, 90.0, -30.0),
        ] {
            let back = axis_to_angles(axis(a));
            let want = Vec3::new(a.x.rem_euclid(360.0), a.y.rem_euclid(360.0), a.z);
            assert!(close(back, want), "{a} -> {back}");
        }
    }

    #[test]
    fn combined_yaws_add() {
        let c = axis_to_angles(mul(
            axis(Vec3::new(0.0, 30.0, 0.0)),
            axis(Vec3::new(0.0, 45.0, 0.0)),
        ));
        assert!(close(c, Vec3::new(0.0, 75.0, 0.0)), "{c}");
    }

    #[test]
    fn fx_angles_point_along_the_forward() {
        let a = fx_angles(Some(Vec3::new(0.0, 1.0, 0.0)), None, 1).unwrap();
        assert!(close(Vec3::from(a), Vec3::new(0.0, 90.0, 0.0)), "{a:?}");
        let a = fx_angles(Some(Vec3::X), Some(Vec3::new(0.0, 1.0, 1.0)), 1).unwrap();
        assert!(close(Vec3::from(a), Vec3::new(0.0, 0.0, -45.0)), "{a:?}");
        assert!(fx_angles(Some(Vec3::X), Some(Vec3::X), 1).is_err());
        assert_eq!(fx_angles(None, None, 1).unwrap(), [270.0, 0.0, 0.0]);
    }
}
