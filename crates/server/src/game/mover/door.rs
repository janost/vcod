//! `func_door`, `func_door_rotating` and `func_rotating`: their spawn
//! functions, the binary mover's states and thinks, the touch trigger, the
//! use key's `G_TryDoor` and a blocked team's reversal
//! (docs/research/cod11-movers.md 26). Addresses are 1.1d's; 1.5b and UO
//! behave the same, their offsets shifted (same section).

use super::{Mover, Plan, bg_evaluate, stationary};
use crate::game::host::GameHost;
use glam::Vec3;
use vcod_common::net::trajectory::{TR_LINEAR, TR_LINEAR_STOP, TR_STATIONARY, Trajectory};
use vcod_gsc::{Cx, EntId, Host, Value};

/// `moverState` (`ent+0x174`), RTCW's order (jump table 0x75c1c).
pub const POS1: u8 = 0;
pub const POS2: u8 = 1;
pub const ONE_TO_TWO: u8 = 3;
pub const TWO_TO_ONE: u8 = 4;
pub const POS1_ROTATE: u8 = 7;
pub const POS2_ROTATE: u8 = 8;
pub const ONE_TO_TWO_ROTATE: u8 = 9;
pub const TWO_TO_ONE_ROTATE: u8 = 10;

/// Which spawn function built the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// `SP_func_door` (0x56bd4).
    Slide,
    /// `SP_func_door_rotating` (0x57124).
    Rotate,
    /// `SP_func_rotating` (0x582fc): `Use_Func_Rotate` and no `reached`.
    Rotating,
}

/// `ent->think` of a door (`+0x200`); `G_RunThink` runs it once `nextthink`
/// (`+0x1fc`) is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Think {
    FinishSpawning,
    MatchTeam,
    SpawnTrigger,
    AutoTrigger,
    ReturnToPos1,
    ReturnToPos1Rotate,
}

/// The sound alias bytes `SP_func_door` and `SP_func_door_rotating` fill
/// (`+0x162..+0x16d`), in their `G_SoundAliasIndex` order (0x56be1..).
const SOUNDS: [(&str, usize); 11] = [
    ("door_opening", OPENING),
    ("door_open_end", OPEN_END),
    ("door_closing", CLOSING),
    ("door_close_end", CLOSE_END),
    ("door_open_loop", OPEN_LOOP),
    ("door_close_loop", CLOSE_LOOP),
    ("door_locked", LOCKED),
    ("door_opening_quiet", 8),
    ("door_open_quiet_end", 9),
    ("door_closing_quiet", 10),
    ("door_close_quiet_end", 11),
];
const CLOSE_END: usize = 0;
const OPENING: usize = 1;
const CLOSING: usize = 2;
const OPEN_END: usize = 3;
const OPEN_LOOP: usize = 5;
const CLOSE_LOOP: usize = 6;
const LOCKED: usize = 7;

/// A door's `gentity_t` fields past what the plans hold.
#[derive(Clone, Debug, PartialEq)]
pub struct Door {
    pub class: Class,
    pub state: u8,
    pos1: Vec3,
    pos2: Vec3,
    /// `gDuration`, `gDurationBack` (`+0x1f4`, `+0x1f8`), and whether a
    /// `closespeed` picks the second for the 2-to-1 leg.
    duration: i32,
    duration_back: i32,
    closespeed: bool,
    /// `angle` (`+0x1d0`, the `degrees` field) and `rotate` (`+0x280`).
    angle: f32,
    rotate: Vec3,
    dmg: i32,
    spawnflags: i32,
    /// `flags & 0x80`, spawnflags 2.
    toggle: bool,
    /// `+0x2a8`, any `key` key.
    locked: bool,
    /// `+0x172`: `G_TryDoor` sets it, `POS1` and the 2-to-1 turn clear it.
    active: bool,
    /// `takedamage` (`+0x171`), a `health` key.
    takedamage: bool,
    think: Option<Think>,
    nextthink: i32,
    sounds: [i32; 12],
}

impl Door {
    fn blank(class: Class, spawnflags: i32) -> Self {
        Door {
            class,
            state: POS1,
            pos1: Vec3::ZERO,
            pos2: Vec3::ZERO,
            duration: 1,
            duration_back: 1,
            closespeed: false,
            angle: 0.0,
            rotate: Vec3::ZERO,
            dmg: 0,
            spawnflags,
            toggle: false,
            locked: false,
            active: false,
            takedamage: false,
            think: None,
            nextthink: 0,
            sounds: [0; 12],
        }
    }
}

fn float_field(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str) -> f32 {
    let atom = cx.intern_folded(name);
    match host.get_field(cx, id, atom) {
        Value::Float(f) => f,
        Value::Int(i) => i as f32,
        _ => 0.0,
    }
}

fn vec_field(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str) -> Vec3 {
    let atom = cx.intern_folded(name);
    match host.get_field(cx, id, atom) {
        Value::Vector(v) => Vec3::from(v),
        _ => Vec3::ZERO,
    }
}

fn set_field(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str, v: Value) {
    let atom = cx.intern_folded(name);
    let _ = host.write_field(cx, id, atom, v);
}

fn door(host: &GameHost, id: EntId) -> Option<&Door> {
    host.movers.rows.get(&id)?.door.as_ref()
}

fn door_mut(host: &mut GameHost, id: EntId) -> Option<&mut Door> {
    host.movers.rows.get_mut(&id)?.door.as_mut()
}

/// Whether `id` is a door or a `func_rotating`.
pub fn is_door(host: &GameHost, id: EntId) -> bool {
    door(host, id).is_some()
}

/// A `func_door` or `func_door_rotating` (`Cmd_Activate_f`'s classname
/// test, 0x484f6): the class and its `moverState`.
pub fn binary_state(host: &GameHost, id: EntId) -> Option<(Class, u8, bool, bool)> {
    door(host, id)
        .filter(|d| d.class != Class::Rotating)
        .map(|d| (d.class, d.state, d.toggle, d.locked))
}

/// The team from `master` down `teamchain`, the master first.
pub fn chain(host: &GameHost, master: EntId) -> Vec<EntId> {
    let mut out = vec![master];
    let mut at = host.ents.get(master).and_then(|e| e.team.chain);
    while let Some(n) = at {
        let Some(id) = host.ents.handle(n) else {
            break;
        };
        if out.contains(&id) {
            break;
        }
        out.push(id);
        at = host.ents.get(id).and_then(|e| e.team.chain);
    }
    out
}

fn sound_index(host: &mut GameHost, alias: &str) -> i32 {
    crate::game::trigger::hurt_sound_index(host, alias) & 0xff
}

/// `G_PlaySoundAlias(ent, alias)` (0x67b30): nothing for alias 0, else an
/// `EV_SOUND_ALIAS` on the entity's own ring.
fn play(host: &mut GameHost, id: EntId, slot: usize) {
    let Some(alias) = door(host, id).map(|d| d.sounds[slot]) else {
        return;
    };
    if alias == 0 {
        return;
    }
    if let Some(e) = host.ents.get_mut(id) {
        e.events
            .add(vcod_common::net::event_ids::EV_SOUND_ALIAS, alias);
    }
}

fn set_loop(host: &mut GameHost, id: EntId, slot: Option<usize>) {
    let alias = slot.and_then(|s| door(host, id).map(|d| d.sounds[s]));
    if let Some(e) = host.ents.get_mut(id) {
        e.loop_sound = alias.unwrap_or(0);
    }
}

/// The 11 `G_SoundAliasIndex` calls both door spawns open with.
fn door_sounds(host: &mut GameHost) -> [i32; 12] {
    let mut s = [0; 12];
    for (alias, slot) in SOUNDS {
        s[slot] = sound_index(host, alias);
    }
    s
}

/// `G_SetMovedir` (0x6773c) on `r.currentAngles`: `(0, -1, 0)` is up,
/// `(0, -2, 0)` down, anything else `AngleVectors`' forward; the angles are
/// cleared.
fn set_movedir(host: &mut GameHost, cx: &mut Cx, id: EntId) -> Vec3 {
    let a = vec_field(host, cx, id, "angles");
    let dir = if a == Vec3::new(0.0, -1.0, 0.0) {
        Vec3::Z
    } else if a == Vec3::new(0.0, -2.0, 0.0) {
        -Vec3::Z
    } else {
        Vec3::from(crate::game::spawn::angle_forward(a.to_array()))
    };
    set_field(host, cx, id, "angles", Value::Vector([0.0; 3]));
    dir
}

/// The submodel's `r.mins`, `r.maxs` (`trap_SetBrushModel`: the lump box a
/// unit out, cod11-movers.md 18).
fn brush_box(host: &GameHost, id: EntId) -> (Vec3, Vec3) {
    let model = host.ents.get(id).and_then(|e| e.brush_model).unwrap_or(0) as usize;
    let (lo, hi) = host
        .model_bounds
        .get(model)
        .copied()
        .unwrap_or(([0.0; 3], [0.0; 3]));
    (Vec3::from(lo), Vec3::from(hi))
}

/// `trap_SetBrushModel`'s link: the model's bounds, contents -1.
fn link_brush(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let (mins, maxs) = brush_box(host, id);
    let shape = crate::game::entity::LinkShape {
        kind: crate::game::entity::LinkKind::Brush,
        contents: -1,
        mins: mins.to_array(),
        maxs: maxs.to_array(),
    };
    host.link_shaped(cx, id, shape);
}

/// The truncating `fistp` of `InitMover`'s leg time, at least 1.
fn leg_ms(dist: f32, speed: f32) -> i32 {
    let ms = (f64::from(dist) * 1000.0 / f64::from(speed)) as i32;
    ms.max(1)
}

/// The spawn of `classname` on `id` from its map block's `keys`, or a
/// script `spawn`'s, which reads none (`G_ParseSpawnVars` cleared them) and
/// keeps model 0, the world's (movers doc 26.10): false for a classname not
/// one of the three.
pub fn spawn(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    classname: &str,
    keys: &[(String, String)],
) -> bool {
    let class = match classname {
        "func_door" => Class::Slide,
        "func_door_rotating" => Class::Rotate,
        "func_rotating" => Class::Rotating,
        _ => return false,
    };
    use crate::game::builtins::cvar::{atof, atoi};
    let key = |name: &str| vcod_common::bsp::spawn_string(keys, name);
    // A script `spawn`'s third argument is the field; a map block's key
    // reaches it through the field pass too.
    let spawnflags = match vcod_common::bsp::field_value(keys, "spawnflags") {
        Some(v) => atoi(v),
        None => float_field(host, cx, id, "spawnflags") as i32,
    };
    let mut d = Door::blank(class, spawnflags);
    let stored = host.movers.stored;
    let mut pos = stationary(Vec3::ZERO);
    let mut apos = stationary(Vec3::ZERO);
    match class {
        Class::Slide => {
            d.sounds = door_sounds(host);
            let mut speed = float_field(host, cx, id, "speed");
            if speed == 0.0 {
                speed = 400.0;
            }
            let mut wait = float_field(host, cx, id, "wait");
            if wait == 0.0 {
                wait = 2.0;
            }
            set_field(host, cx, id, "wait", Value::Float(wait * 1000.0));
            // `G_SpawnInt("key", "")`'s return, not its value (0x56d38).
            d.locked = key("key").is_some();
            let lip = atof(key("lip").unwrap_or("8"));
            d.dmg = atoi(key("dmg").unwrap_or("2"));
            set_field(host, cx, id, "dmg", Value::Int(d.dmg));
            let origin = vec_field(host, cx, id, "origin");
            d.pos1 = origin;
            link_brush(host, cx, id);
            // The spawn left the `angles` key in `apos.trBase` too, which
            // `G_SetMovedir` does not clear: the first `G_MoverTeam` turns
            // the door by it (movers doc 26, "The angles key").
            apos.base = vec_field(host, cx, id, "angles");
            let dir = set_movedir(host, cx, id);
            let (mins, maxs) = brush_box(host, id);
            let size = maxs - mins;
            let dist = (f64::from(dir.x.abs()) * f64::from(size.x)
                + f64::from(dir.y.abs()) * f64::from(size.y)
                + f64::from(dir.z.abs()) * f64::from(size.z)
                - f64::from(lip)) as f32;
            d.pos2 = (origin.as_dvec3() + dir.as_dvec3() * f64::from(dist)).as_vec3();
            let mut closespeed = float_field(host, cx, id, "closespeed");
            if spawnflags & 1 != 0 {
                let old2 = d.pos2;
                d.pos2 = origin;
                d.pos1 = old2;
                if closespeed != 0.0 {
                    std::mem::swap(&mut speed, &mut closespeed);
                }
                d.sounds.swap(OPENING, CLOSING);
                d.sounds.swap(CLOSE_END, OPEN_END);
                d.sounds.swap(OPEN_LOOP, CLOSE_LOOP);
                d.sounds.swap(8, 10);
                d.sounds.swap(9, 11);
            }
            set_field(host, cx, id, "speed", Value::Float(speed));
            set_field(host, cx, id, "closespeed", Value::Float(closespeed));
            d.toggle = spawnflags & 2 != 0;
            init_mover(host, cx, id, &mut d, &mut pos, keys);
        }
        Class::Rotate => {
            let mut flags = spawnflags;
            if flags & 1 != 0 {
                flags |= 2;
            }
            flags &= !0x40;
            d.spawnflags = flags;
            set_field(host, cx, id, "spawnflags", Value::Int(flags));
            d.sounds = door_sounds(host);
            let mut speed = float_field(host, cx, id, "speed");
            if speed == 0.0 {
                speed = 1000.0;
            }
            let mut angle = float_field(host, cx, id, "degrees");
            if angle == 0.0 {
                angle = 90.0;
            }
            if flags & 0x10 != 0 {
                angle = -angle;
            }
            d.angle = angle;
            set_field(host, cx, id, "degrees", Value::Float(angle));
            d.toggle = flags & 2 != 0;
            d.locked = key("key").is_some();
            d.rotate = if flags & 0xc == 0xc {
                Vec3::Y
            } else if flags & 4 != 0 {
                Vec3::Z
            } else if flags & 8 != 0 {
                Vec3::X
            } else {
                Vec3::Y
            };
            set_field(host, cx, id, "rotate", Value::Vector(d.rotate.to_array()));
            let mut wait = float_field(host, cx, id, "wait");
            if wait == 0.0 {
                wait = 2.0;
            }
            set_field(host, cx, id, "wait", Value::Float(wait * 1000.0));
            link_brush(host, cx, id);
            // `InitMoverRotate` (0x5674c).
            d.state = POS1_ROTATE;
            super::set_constant_light(host, id, Some(keys));
            link_brush(host, cx, id);
            pos.base = vec_field(host, cx, id, "origin");
            if speed == 0.0 {
                speed = 100.0;
            }
            set_field(host, cx, id, "speed", Value::Float(speed));
            let ms = (speed as i32).max(1);
            apos.tr_duration = ms;
            d.duration = ms;
            d.duration_back = ms;
            apos.base = vec_field(host, cx, id, "angles");
        }
        Class::Rotating => {
            let mut speed = float_field(host, cx, id, "speed");
            if speed == 0.0 {
                speed = 100.0;
            }
            set_field(host, cx, id, "speed", Value::Float(speed));
            apos.tr_type = TR_LINEAR;
            if spawnflags & 1 != 0 {
                apos.delta = rotating_axis(spawnflags) * speed;
            }
            if atoi(key("dmg").unwrap_or("0")) == 0 {
                set_field(host, cx, id, "dmg", Value::Int(2));
            }
            link_brush(host, cx, id);
            init_mover(host, cx, id, &mut d, &mut pos, keys);
            apos.base = vec_field(host, cx, id, "angles");
            if spawnflags & 2 != 0 {
                if let Some(e) = host.ents.get_mut(id) {
                    e.team.slave = true;
                }
                crate::game::teams::unlink_entity(host, id);
            }
        }
    }
    if class != Class::Rotating {
        if host.ents.get(id).is_some_and(|e| !e.team.slave) {
            d.takedamage = atoi(key("health").unwrap_or("0")) != 0;
        }
        d.think = Some(Think::FinishSpawning);
        d.nextthink = host.level_time_ms + FINISH_MS;
    }
    let plan = |current: Trajectory, started: bool| Plan {
        current,
        stored,
        started,
        ..Plan::default()
    };
    host.movers.rows.insert(
        id,
        Mover {
            pos: plan(pos, true),
            apos: plan(apos, true),
            init_mover: true,
            door: Some(d),
            ..Mover::default()
        },
    );
    let (origin, angles) = super::pose(host, cx, id);
    if let Some(model) = super::submodel(host, id)
        && let Some(world) = &host.world
    {
        world.collision.set_model_pose(model, origin, angles);
    }
    true
}

/// `SP_func_rotating`'s and `Use_Func_Rotate`'s axis: spawnflags 4 roll, 8
/// pitch, else yaw.
fn rotating_axis(spawnflags: i32) -> Vec3 {
    if spawnflags & 4 != 0 {
        Vec3::Z
    } else if spawnflags & 8 != 0 {
        Vec3::X
    } else {
        Vec3::Y
    }
}

/// `SP_func_door`'s and `SP_func_door_rotating`'s `nextthink`, and
/// `finishSpawningKeyedMover`'s.
const FINISH_MS: i32 = 100;

/// `InitMover` (0x56454) for a `func_door` or `func_rotating`: `noise`,
/// `r.currentOrigin = pos1` (zero for a `func_rotating`, which writes no
/// `pos1`), `moverState` 0, the link, `pos.trBase = pos1`, a zero speed
/// made 100 and the leg times.
fn init_mover(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    d: &mut Door,
    pos: &mut Trajectory,
    keys: &[(String, String)],
) {
    if let Some(noise) = vcod_common::bsp::spawn_string(keys, "noise") {
        let alias = sound_index(host, noise);
        if let Some(e) = host.ents.get_mut(id) {
            e.loop_sound = alias;
        }
    }
    super::set_constant_light(host, id, Some(keys));
    set_field(host, cx, id, "origin", Value::Vector(d.pos1.to_array()));
    d.state = POS1;
    host.link_entity(cx, id);
    pos.base = d.pos1;
    let mut speed = float_field(host, cx, id, "speed");
    if speed == 0.0 {
        speed = 100.0;
        set_field(host, cx, id, "speed", Value::Float(speed));
    }
    let dist = (d.pos2 - d.pos1).as_dvec3().length() as f32;
    pos.tr_duration = leg_ms(dist, speed);
    d.duration = pos.tr_duration;
    d.duration_back = pos.tr_duration;
    let closespeed = float_field(host, cx, id, "closespeed");
    if closespeed != 0.0 {
        d.closespeed = true;
        d.duration_back = leg_ms(dist, closespeed);
    }
}

/// `SetMoverState` (0x55904): `moverState`, both `trTime`s, the group the
/// state moves, then `r.currentOrigin` off `pos` at the level time and a
/// link. 1.1d keeps `1000 / trDuration` on the stack; 1.5b and UO round it
/// to a float (1.5b 0x56dea).
pub fn set_state(host: &mut GameHost, cx: &mut Cx, id: EntId, state: u8, t: i32) {
    let level = host.level_time_ms;
    let angles = vec_field(host, cx, id, "angles");
    let Some(m) = host.movers.rows.get_mut(&id) else {
        return;
    };
    let Some(d) = m.door.as_mut() else {
        return;
    };
    let stored = m.pos.stored;
    d.state = state;
    m.pos.current.tr_time = t;
    m.apos.current.tr_time = t;
    let per = |dur: i32| {
        let f = 1000.0 / f64::from(dur);
        if stored { f64::from(f as f32) } else { f }
    };
    let leg = |from: Vec3, to: Vec3, dur: i32, rounded: bool| {
        let f = per(dur);
        let diff = |a: f32, b: f32| {
            let v = f64::from(b) - f64::from(a);
            if rounded || stored {
                f64::from(v as f32)
            } else {
                v
            }
        };
        Vec3::new(
            (diff(from.x, to.x) * f) as f32,
            (diff(from.y, to.y) * f) as f32,
            (diff(from.z, to.z) * f) as f32,
        )
    };
    match state {
        POS1 => {
            m.pos.current.base = d.pos1;
            m.pos.current.tr_type = TR_STATIONARY;
            d.active = false;
        }
        POS2 => {
            m.pos.current.base = d.pos2;
            m.pos.current.tr_type = TR_STATIONARY;
        }
        ONE_TO_TWO => {
            m.pos.current.base = d.pos1;
            m.pos.current.tr_duration = d.duration;
            m.pos.current.delta = leg(d.pos1, d.pos2, d.duration, false);
            m.pos.current.tr_type = TR_LINEAR_STOP;
        }
        TWO_TO_ONE => {
            let dur = if d.closespeed {
                d.duration_back
            } else {
                d.duration
            };
            m.pos.current.base = d.pos2;
            m.pos.current.tr_duration = dur;
            m.pos.current.delta = leg(d.pos2, d.pos1, dur, true);
            m.pos.current.tr_type = TR_LINEAR_STOP;
        }
        POS1_ROTATE | POS2_ROTATE => {
            m.apos.current.base = angles;
            m.apos.current.tr_type = TR_STATIONARY;
        }
        ONE_TO_TWO_ROTATE => {
            m.apos.current.base = Vec3::ZERO;
            m.apos.current.tr_duration = d.duration;
            let k = per(d.duration) * f64::from(d.angle);
            let k = if stored { f64::from(k as f32) } else { k };
            m.apos.current.delta = (d.rotate.as_dvec3() * k).as_vec3();
            m.apos.current.tr_type = TR_LINEAR_STOP;
        }
        TWO_TO_ONE_ROTATE => {
            let base = (d.rotate.as_dvec3() * f64::from(d.angle)).as_vec3();
            m.apos.current.base = base;
            m.apos.current.tr_duration = d.duration;
            m.apos.current.delta = (base.as_dvec3() * -per(d.duration)).as_vec3();
            m.apos.current.tr_type = TR_LINEAR_STOP;
            d.active = false;
        }
        _ => {}
    }
    m.pos.started = true;
    if matches!(state, ONE_TO_TWO_ROTATE | TWO_TO_ONE_ROTATE) {
        m.apos.started = true;
    }
    let origin = bg_evaluate(m.pos.stored, &m.pos.current, level);
    set_field(host, cx, id, "origin", Value::Vector(origin.to_array()));
    super::verb_linked(host, cx, id);
}

/// `MatchTeam` (0x577fc): every part from `master` to `state` at `t`.
fn match_team(host: &mut GameHost, cx: &mut Cx, master: EntId, state: u8, t: i32) {
    for part in chain(host, master) {
        set_state(host, cx, part, state, t);
    }
}

/// `Reached_BinaryMover` (0x55c9c) on a door: a 1-to-2 leg or turn
/// settles open and arms `ReturnToPos1` or `ReturnToPos1Rotate` `wait` on
/// (never under a toggle, never at a `wait` of -1000), a 2-to-1 one
/// settles closed; any other state drops the level.
pub fn reached(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let Some((class, state)) = door(host, id).map(|d| (d.class, d.state)) else {
        return;
    };
    if class == Class::Rotating {
        return;
    }
    let level = host.level_time_ms;
    set_loop(host, id, None);
    match state {
        ONE_TO_TWO | ONE_TO_TWO_ROTATE => {
            let (open, think) = if state == ONE_TO_TWO {
                (POS2, Think::ReturnToPos1)
            } else {
                (POS2_ROTATE, Think::ReturnToPos1Rotate)
            };
            set_state(host, cx, id, open, level);
            play(host, id, OPEN_END);
            let wait = super::mover_wait(host, cx, id);
            let Some(d) = door_mut(host, id) else {
                return;
            };
            if d.toggle {
                d.active = false;
                d.think = Some(think);
                d.nextthink = 0;
            } else if let Some(w) = wait {
                d.think = Some(think);
                d.nextthink = level + w;
            }
        }
        TWO_TO_ONE | TWO_TO_ONE_ROTATE => {
            let closed = if state == TWO_TO_ONE {
                POS1
            } else {
                POS1_ROTATE
            };
            set_state(host, cx, id, closed, level);
            if state == TWO_TO_ONE || player_in_pvs(host, cx, id) {
                play(host, id, CLOSE_END);
            }
        }
        _ => {
            if host.game_error.is_none() {
                host.game_error = Some("Reached_BinaryMover: bad moverState".to_string());
            }
        }
    }
}

/// `G_RunThink` (0x50d08) for a door: a due `nextthink` is cleared and its
/// think run.
pub fn run_think(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let level = host.level_time_ms;
    let Some(d) = door_mut(host, id) else {
        return;
    };
    if d.nextthink <= 0 || d.nextthink > level {
        return;
    }
    d.nextthink = 0;
    let Some(think) = d.think else {
        return;
    };
    match think {
        Think::FinishSpawning => finish_spawning(host, cx, id),
        Think::MatchTeam => {
            let state = door(host, id).map_or(POS1, |d| d.state);
            match_team(host, cx, id, state, level);
        }
        Think::SpawnTrigger => spawn_trigger(host, cx, id),
        Think::AutoTrigger => auto_trigger(host, cx, id),
        Think::ReturnToPos1 => {
            match_team(host, cx, id, TWO_TO_ONE, level);
            play(host, id, CLOSING);
            set_loop(host, id, Some(CLOSE_LOOP));
        }
        Think::ReturnToPos1Rotate => {
            match_team(host, cx, id, TWO_TO_ONE_ROTATE, level);
            if player_in_pvs(host, cx, id) {
                play(host, id, CLOSING);
            }
            set_loop(host, id, Some(CLOSE_LOOP));
        }
    }
}

/// The test `ReturnToPos1Rotate` (0x57cc7..0x57cf2) and the 2-to-1 turn's
/// end in `Reached_BinaryMover` (0x55eb1..0x55ee4) put on their sound:
/// `G_Find(NULL, classname, "player")`, the first in-use player in entity
/// order, then `trap_InPVS` from his `r.currentOrigin` to the door's.
/// No player, no sound (docs/research/cod11-movers.md 26.8).
fn player_in_pvs(host: &mut GameHost, cx: &mut Cx, id: EntId) -> bool {
    let classname = cx.intern_folded("classname");
    let ids: Vec<EntId> = host.ents.iter_inuse().map(|(e, _)| e).collect();
    let Some(player) = ids.into_iter().find(|&e| {
        matches!(host.get_field(cx, e, classname), Value::String(a) if cx.resolve(a) == "player")
    }) else {
        return false;
    };
    let from = vec_field(host, cx, player, "origin").to_array();
    let to = vec_field(host, cx, id, "origin").to_array();
    host.world.as_ref().is_none_or(|w| in_pvs(&w.vis, from, to))
}

/// `SV_inPVS` (cod_lnxded, the game's syscall 45 at 0x80883e0): `p2`'s
/// cluster through `p1`'s PVS row, then both leafs' areas connected.
pub fn in_pvs(vis: &vcod_common::mapworld::Visibility, p1: [f32; 3], p2: [f32; 3]) -> bool {
    vis.visible(vis.cluster_at(p1), vis.cluster_at(p2))
        && vcod_common::mapworld::areas_connected(vis.area_at(p1), vis.area_at(p2))
}

/// `finishSpawningKeyedMover` (0x57e68): another 100 ms, then a think by
/// `takedamage`, class and spawnflags 8; the master's lock goes to the team.
fn finish_spawning(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let level = host.level_time_ms;
    let slave = host.ents.get(id).is_some_and(|e| e.team.slave);
    let Some(d) = door_mut(host, id) else {
        return;
    };
    d.nextthink = level + FINISH_MS;
    if slave {
        return;
    }
    d.think = Some(if d.takedamage {
        Think::MatchTeam
    } else if d.class == Class::Rotate {
        Think::AutoTrigger
    } else if d.spawnflags & 8 != 0 {
        Think::SpawnTrigger
    } else {
        Think::MatchTeam
    });
    let locked = d.locked;
    let _ = cx;
    for part in chain(host, id).into_iter().skip(1) {
        if let Some(p) = door_mut(host, part) {
            p.locked = locked;
        }
    }
}

/// `Think_SpawnNewAutoDoorTrigger` (0x57da4): a start-open rotating door
/// turns its yaw by `angle`, takes `ReturnToPos1Rotate` with no
/// `nextthink` and sits open; nothing else does anything.
fn auto_trigger(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let Some(d) = door(host, id) else {
        return;
    };
    if d.spawnflags & 1 == 0 {
        return;
    }
    let angle = d.angle;
    let mut a = vec_field(host, cx, id, "angles");
    a.y = (f64::from(a.y) + f64::from(angle)) as f32;
    set_field(host, cx, id, "angles", Value::Vector(a.to_array()));
    if let Some(d) = door_mut(host, id) {
        d.think = Some(Think::ReturnToPos1Rotate);
    }
    let level = host.level_time_ms;
    set_state(host, cx, id, POS2_ROTATE, level);
}

/// `CONTENTS_TRIGGER`, the door trigger's contents (0x56b5b).
const CONTENTS_TRIGGER: i32 = 0x4000_0000;
/// How far the trigger reaches past the team on its thinnest axis
/// (0x75ccc).
const TRIGGER_REACH: f32 = 120.0;

/// `Think_SpawnNewDoorTrigger` (0x57d8c, its body at 0x56a14): the team
/// takes damage, a `G_Spawn`ed trigger over the team's link boxes grown 120
/// on their thinnest axis touches with `Touch_DoorTrigger`, then
/// `MatchTeam` at the door's state.
fn spawn_trigger(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let parts = chain(host, id);
    let mut bounds: Option<(Vec3, Vec3)> = None;
    for &part in &parts {
        if let Some(d) = door_mut(host, part) {
            d.takedamage = true;
        }
        let Some(link) = host.area.last_link(part.0) else {
            continue;
        };
        let (lo, hi) = (Vec3::from(link.absmin), Vec3::from(link.absmax));
        bounds = Some(match bounds {
            None => (lo, hi),
            Some((a, b)) => (a.min(lo).min(hi), b.max(lo).max(hi)),
        });
    }
    let (mut mins, mut maxs) = bounds.unwrap_or((Vec3::ZERO, Vec3::ZERO));
    let mut best = 0;
    for i in 1..3 {
        if maxs[i] - mins[i] < maxs[best] - mins[best] {
            best = i;
        }
    }
    maxs[best] += TRIGGER_REACH;
    mins[best] -= TRIGGER_REACH;
    if let Ok(trigger) = host.ents.spawn(cx) {
        host.triggers.register(
            trigger,
            crate::game::trigger::TriggerKind::Door { parent: id.0 },
            crate::game::trigger::TriggerShape::boxed(mins.to_array(), maxs.to_array()),
            0,
        );
        let shape = crate::game::entity::LinkShape {
            kind: crate::game::entity::LinkKind::Brush,
            contents: CONTENTS_TRIGGER,
            mins: mins.to_array(),
            maxs: maxs.to_array(),
        };
        host.link_shaped(cx, trigger, shape);
    }
    let level = host.level_time_ms;
    let state = door(host, id).map_or(POS1, |d| d.state);
    match_team(host, cx, id, state, level);
}

/// `Touch_DoorTrigger` (0x57d48): an unlocked door at `POS1`, `POS1ROTATE`
/// or turning shut is used by the toucher.
pub fn touch_trigger(host: &mut GameHost, cx: &mut Cx, parent: u32, toucher: EntId) {
    let Some(id) = host.ents.handle(parent) else {
        return;
    };
    let Some(d) = door(host, id) else {
        return;
    };
    if d.locked || !matches!(d.state, POS1 | POS1_ROTATE | TWO_TO_ONE_ROTATE) {
        return;
    }
    use_binary(host, cx, id, Some(toucher));
}

/// `G_TryDoor` (0x57718), the use key's arm for a door (`Cmd_Activate_f`
/// 0x4851e): a door at rest and not `active` notifies `"trigger"` with the
/// player and is used, a locked one plays its locked sound. A slave's use
/// goes to its master.
pub fn try_door(host: &mut GameHost, cx: &mut Cx, id: EntId, player: EntId) {
    let Some(m) = host.movers.rows.get(&id) else {
        return;
    };
    let Some(d) = m.door.as_ref() else {
        return;
    };
    if m.apos.current.tr_type != TR_STATIONARY || m.pos.current.tr_type != TR_STATIONARY || d.active
    {
        return;
    }
    if d.locked {
        play(host, id, LOCKED);
        return;
    }
    host.engine_notifies
        .push((id, "trigger", vec![Value::Entity(player)]));
    let team = cx.intern_folded("teamname");
    let teamed = matches!(host.get_field(cx, id, team), Value::String(_));
    let master = host
        .ents
        .get(id)
        .and_then(|e| e.team.master)
        .and_then(|n| host.ents.handle(n));
    let target = match master {
        Some(m) if teamed && m != id => m,
        _ => id,
    };
    if let Some(d) = door_mut(host, target) {
        d.active = true;
    }
    use_binary(host, cx, target, Some(player));
}

/// `IsBinaryMoverBlocked` (0x55f54): a `func_door_rotating` without
/// spawnflags 0x20 opens the other way when the activator stands on the side
/// its turn swings toward.
fn opens_into(host: &mut GameHost, cx: &mut Cx, id: EntId, activator: Option<EntId>) -> bool {
    let Some(d) = door(host, id) else {
        return false;
    };
    if d.class != Class::Rotate || d.spawnflags & 0x20 != 0 {
        return false;
    }
    let (angle, rotate) = (d.angle, d.rotate);
    let Some(link) = host.area.last_link(id.0) else {
        return false;
    };
    let pos = (Vec3::from(link.absmin) + Vec3::from(link.absmax)) * 0.5;
    let origin = vec_field(host, cx, id, "origin");
    let mut a = Vec3::from(crate::game::builtins::math::vectoangles(
        (pos - origin).to_array(),
    ));
    if rotate.y != 0.0 {
        a.y += angle;
    } else if rotate.x != 0.0 {
        a.x += angle;
    } else if rotate.z != 0.0 {
        a.z += angle;
    }
    let forward = Vec3::from(crate::game::spawn::angle_forward(a.to_array()));
    let Some(act) = activator else {
        return false;
    };
    let at = vec_field(host, cx, act, "origin");
    let v = (at - pos).normalize_or_zero();
    v.dot(forward) >= 0.0
}

/// `Use_BinaryMover` (0x560ec), every door's `use`, from a `useBy`, the
/// trigger or the use key; `Use_Func_Rotate` (0x57848) for a
/// `func_rotating`. Returns false for anything else.
pub fn use_binary(host: &mut GameHost, cx: &mut Cx, id: EntId, activator: Option<EntId>) -> bool {
    let Some(d) = door(host, id) else {
        return false;
    };
    if d.class == Class::Rotating {
        use_func_rotate(host, cx, id);
        return true;
    }
    let level = host.level_time_ms;
    let nosound = level <= 4000;
    let slave = host.ents.get(id).is_some_and(|e| e.team.slave);
    if slave {
        if let Some(master) = host
            .ents
            .get(id)
            .and_then(|e| e.team.master)
            .and_then(|n| host.ents.handle(n))
        {
            use_binary(host, cx, master, activator);
        }
        return true;
    }
    let state = d.state;
    let sound = |host: &mut GameHost, slot| {
        if !nosound {
            play(host, id, slot);
        }
    };
    let open_loop = |host: &mut GameHost| {
        set_loop(host, id, None);
        if !nosound {
            set_loop(host, id, Some(OPEN_LOOP));
        }
    };
    if matches!(state, POS1 | POS1_ROTATE) && opens_into(host, cx, id, activator) {
        // `MatchTeamReverseAngleOnSlaves` inline (0x5617a).
        for part in chain(host, id) {
            if let Some(p) = door_mut(host, part) {
                p.angle = -p.angle;
            }
            set_state(host, cx, part, ONE_TO_TWO_ROTATE, level + 50);
        }
        sound(host, OPENING);
        open_loop(host);
        return true;
    }
    match state {
        POS1 | POS1_ROTATE => {
            let leg = if state == POS1 {
                ONE_TO_TWO
            } else {
                ONE_TO_TWO_ROTATE
            };
            match_team(host, cx, id, leg, level + 50);
            sound(host, OPENING);
            open_loop(host);
        }
        POS2 | POS2_ROTATE => {
            let wait = super::mover_wait(host, cx, id);
            let Some(d) = door_mut(host, id) else {
                return true;
            };
            if d.toggle {
                d.nextthink = level + 50;
            } else if state == POS2_ROTATE {
                let w = wait.unwrap_or(-1000);
                d.nextthink = level + w;
            } else if let Some(w) = wait {
                d.nextthink = level + w;
            }
        }
        TWO_TO_ONE | ONE_TO_TWO => {
            reverse(host, cx, id);
            sound(
                host,
                if state == TWO_TO_ONE {
                    OPENING
                } else {
                    CLOSING
                },
            );
        }
        TWO_TO_ONE_ROTATE | ONE_TO_TWO_ROTATE => {
            reverse(host, cx, id);
            sound(
                host,
                if state == TWO_TO_ONE_ROTATE {
                    OPENING
                } else {
                    CLOSING
                },
            );
        }
        _ => {}
    }
    true
}

/// `Use_Func_Rotate` (0x57848): the axis's speed, spawnflags 2 clears the
/// slave bit, a link.
fn use_func_rotate(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let Some(d) = door(host, id) else {
        return;
    };
    let flags = d.spawnflags;
    let speed = float_field(host, cx, id, "speed");
    if let Some(m) = host.movers.rows.get_mut(&id) {
        let axis = rotating_axis(flags);
        let delta = &mut m.apos.current.delta;
        if axis == Vec3::Z {
            delta.z = speed;
        } else if axis == Vec3::X {
            delta.x = speed;
        } else {
            delta.y = speed;
        }
    }
    if flags & 2 != 0
        && let Some(e) = host.ents.get_mut(id)
    {
        e.team.slave = false;
    }
    host.link_entity(cx, id);
}

/// Every linked door with `takedamage` (a `health` key, or a part of a
/// team `Think_SpawnNewDoorTrigger` ran on), by entity number: what a
/// blast or a round can reach.
pub fn damageable(host: &GameHost) -> Vec<EntId> {
    let mut ids: Vec<EntId> = host
        .movers
        .rows
        .keys()
        .copied()
        .filter(|&id| is_damageable(host, id))
        .collect();
    ids.sort_by_key(|id| id.0);
    ids
}

/// Whether `id` is one of [`damageable`].
pub fn is_damageable(host: &GameHost, id: EntId) -> bool {
    door(host, id).is_some_and(|d| d.takedamage) && host.area.last_link(id.0).is_some()
}

/// `G_Damage`'s `eType` 5 arm (0x49e93..0x49ee1) on a door with
/// `takedamage`: at `moverState` 0 the `"trigger"` notify with the
/// attacker, then its `use` with the attacker as activator; at any other
/// state nothing, so a rotating door (7 at rest) never opens to damage. No
/// `health` comes off. `None` for anything not a door with `takedamage`;
/// the notifies are the caller's to raise.
pub fn damaged(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    attacker: EntId,
) -> Option<Vec<(&'static str, Vec<Value>)>> {
    let d = door(host, id).filter(|d| d.takedamage)?;
    if d.state != POS1 {
        return Some(Vec::new());
    }
    use_binary(host, cx, id, Some(attacker));
    Some(vec![("trigger", vec![Value::Entity(attacker)])])
}

/// Every linked door's last link box, by entity number, which the snapshot
/// cull reads (`crate::archive::WorldFrame::links`).
pub fn links(host: &GameHost) -> std::collections::BTreeMap<u32, ([f32; 3], [f32; 3])> {
    host.movers
        .rows
        .iter()
        .filter(|(_, m)| m.door.is_some())
        .filter_map(|(id, _)| {
            let l = host.area.last_link(id.0)?;
            Some((id.0, (l.absmin, l.absmax)))
        })
        .collect()
}

/// The damage `Blocked_Door` (0x578a0) or `Blocked_DoorRotate` (0x56934)
/// deals the player who stopped the team at `master`, in order: a rotating
/// door first finishes off a dead one with 99999, then each deals its `dmg`.
pub fn blocked_damage(host: &GameHost, master: EntId, dead: bool) -> Vec<i32> {
    let Some(d) = door(host, master) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match d.class {
        Class::Slide => {}
        Class::Rotate => {
            if dead {
                out.push(99999);
            }
        }
        Class::Rotating => return out,
    }
    if d.dmg != 0 {
        out.push(d.dmg);
    }
    out
}

/// The tail of `Blocked_Door` and `Blocked_DoorRotate`: every part turns
/// back from where it is, the leg's remaining time mirrored
/// (`level.time - (trDuration - (level.time - trTime))`), and links. A
/// sliding crusher (spawnflags 4) never turns back.
pub fn reverse(host: &mut GameHost, cx: &mut Cx, master: EntId) {
    let Some(d) = door(host, master) else {
        return;
    };
    if d.class == Class::Rotating || (d.class == Class::Slide && d.spawnflags & 4 != 0) {
        return;
    }
    let level = host.level_time_ms;
    for part in chain(host, master) {
        let Some(m) = host.movers.rows.get(&part) else {
            continue;
        };
        let Some(p) = m.door.as_ref() else {
            continue;
        };
        let rotating = p.class == Class::Rotate;
        let tr = if rotating {
            &m.apos.current
        } else {
            &m.pos.current
        };
        let t = level - (tr.tr_duration - (level - tr.tr_time));
        let next = match (rotating, p.state) {
            (false, ONE_TO_TWO) => TWO_TO_ONE,
            (false, _) => ONE_TO_TWO,
            (true, ONE_TO_TWO_ROTATE) => TWO_TO_ONE_ROTATE,
            (true, _) => ONE_TO_TWO_ROTATE,
        };
        set_state(host, cx, part, next, t);
        host.link_entity(cx, part);
    }
}

/// Whether `master` has a `blocked` (`Blocked_Door`, `Blocked_DoorRotate`).
pub fn has_blocked(host: &GameHost, master: EntId) -> bool {
    door(host, master).is_some_and(|d| d.class != Class::Rotating)
}
