//! CoD4 1.7's script traces through the level's whole world: the clip map
//! and its static models, then the brush entities and (with
//! `CONTENTS_BODY` in the mask) the bodies the last frame linked, as
//! `G_LocationalTrace` and `G_SightTrace` walk them (docs/research/
//! cod4-gsc.md 3.3). The masks are the immediates 1.7's handlers load
//! (`cod4_lnxded` 0x80c6640, 0x80beed8, 0x80c01de). The
//! bullet traces are locational: a body is its hit boxes as the last frame
//! posed them under `bulletPriorityMap`, a helicopter its model's
//! collision; the sight trace meets a body's linked box.

use glam::Vec3;
use vcod_cod4::pmove::{CONTENTS_BODY, PmWorld, Tr};
use vcod_gsc::{ArrayKey, Cx, ErrorKind, Target, Value};

use super::score::Builtin;
use crate::game::cod4::clip::Cod4Clip;
use crate::game::host::GameHost;

pub const NAMES: &[(&str, Builtin)] = &[
    ("bullettrace", bullet_trace),
    ("bullettracepassed", bullet_trace_passed),
    ("sighttracepassed", sight_trace_passed),
];

/// `bulletTrace`'s and `bulletTracePassed`'s mask, `MASK_SHOT`
/// (0x80c6640, 0x80beed8).
const MASK_BULLET: u32 = 0x280_6831;
/// `sightTracePassed`'s (0x80c01de).
const MASK_SIGHT: u32 = 0x280_1803;
/// `ENTITYNUM_NONE` and `ENTITYNUM_WORLD`.
const NONE: u32 = 1023;
const WORLD: u32 = 1022;

/// A trace through the level's world passing entity `pass`, `None` with
/// no clip map. A point trace meets the static models too.
pub(crate) fn trace4(
    host: &GameHost,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    pass: u32,
    mask: u32,
) -> Option<Tr> {
    let c4 = host.cod4();
    let bodies = if mask & CONTENTS_BODY != 0 {
        &c4.trace_bodies[..]
    } else {
        &[]
    };
    trace4_with(host, start, end, mins, maxs, pass, mask, bodies)
}

/// `G_LocationalTrace` for a point: [`trace4`] with each posed
/// body (`Cod4HostState::posed_bodies`) scored on its hit boxes under
/// `bulletPriorityMap` in place of its box, and the helicopters' model
/// collision under 0x2080. A struck body reads the flesh surface (7).
pub(crate) fn locational4(
    host: &GameHost,
    start: Vec3,
    end: Vec3,
    pass: u32,
    mask: u32,
) -> Option<Tr> {
    let c4 = host.cod4();
    let posed = if mask & CONTENTS_BODY != 0 {
        &c4.posed_bodies[..]
    } else {
        &[]
    };
    let boxes: Vec<_> = c4
        .trace_bodies
        .iter()
        .filter(|b| mask & b.contents != 0 && !posed.iter().any(|p| p.entity == b.entity))
        .copied()
        .collect();
    let mut t = trace4_with(host, start, end, Vec3::ZERO, Vec3::ZERO, pass, mask, &boxes)?;
    let mut limit = if t.startsolid { 0.0 } else { t.fraction };
    for p in posed.iter().filter(|p| p.entity != pass) {
        let priority = &crate::game::cod4::combat::BULLET_PRIORITY;
        if let Some((h, n)) = p.trace(start, end, priority, limit)
            && h.fraction < limit
        {
            limit = h.fraction;
            t.fraction = h.fraction;
            t.normal = n;
            t.surface_flags = 7 << 20;
            t.contents = CONTENTS_BODY;
            t.entity_num = p.entity;
            t.startsolid = h.start_solid;
        }
    }
    let skip = (pass < WORLD).then(|| host.ents.handle(pass)).flatten();
    if let Some(h) = crate::game::cod4::vehicle::heli_trace(host, start, end, limit, skip, mask) {
        t.fraction = h.fraction;
        t.normal = h.normal;
        t.surface_flags = h.surface_flags;
        t.contents = 0x2080;
        t.entity_num = h.id.0;
        t.startsolid = false;
    }
    Some(t)
}

/// [`trace4`] over `bodies`.
#[allow(clippy::too_many_arguments)]
fn trace4_with(
    host: &GameHost,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    pass: u32,
    mask: u32,
    bodies: &[crate::game::cod4::clip::Body4],
) -> Option<Tr> {
    let c4 = host.cod4();
    let clip = c4.clip.as_deref()?;
    let world = Cod4Clip {
        clip,
        bodies,
        brushes: &c4.trace_brushes,
    };
    let mut t = world.trace(start, end, mins, maxs, pass, mask);
    if mins == Vec3::ZERO && maxs == Vec3::ZERO {
        let mut ct = vcod_common::clipmap::ClipTrace::clear();
        ct.fraction = t.fraction;
        if clip
            .trace_static_models(start, end, mask, &mut ct)
            .is_some()
            && ct.fraction < t.fraction
        {
            t.fraction = ct.fraction;
            t.normal = ct.normal;
            t.surface_flags = ct.surface_flags;
            t.entity_num = WORLD;
            t.startsolid = ct.startsolid;
        }
    }
    if t.fraction >= 1.0 && !t.startsolid {
        t.entity_num = NONE;
    }
    Some(t)
}

/// One script trace's inputs: the segment, the mask and the entity it
/// passes through.
struct Query {
    start: Vec3,
    end: Vec3,
    mask: u32,
    pass: u32,
}

impl Query {
    /// `(start, end, hitCharacters, ignoreEnt)` as all three read them
    /// (`cod4_lnxded` 0x80c6603..0x80c6655, 0x80bee9b, 0x80c01a1): a false
    /// `hitCharacters` takes `CONTENTS_BODY` off `mask`, and an
    /// `ignoreEnt` that is no entity passes `ENTITYNUM_NONE`.
    fn read(a: &[Value], mask: u32) -> Result<Query, ErrorKind> {
        const USAGE: &str = "takes start, end, hitCharacters and ignoreEnt";
        let point = |i: usize| match a.get(i) {
            Some(Value::Vector(v)) => Ok(Vec3::from(*v)),
            _ => Err(ErrorKind::BadType(USAGE)),
        };
        let (start, end) = (point(0)?, point(1)?);
        let hits_bodies = a.get(2).ok_or(ErrorKind::BadType(USAGE))?.as_bool()?;
        let pass = match a.get(3) {
            Some(Value::Entity(id)) => id.0,
            _ => NONE,
        };
        Ok(Query {
            start,
            end,
            mask: if hits_bodies {
                mask
            } else {
                mask & !CONTENTS_BODY
            },
            pass,
        })
    }

    /// `G_LocationalTrace` under the bullet mask.
    fn locational(&self, host: &GameHost) -> Tr {
        locational4(host, self.start, self.end, self.pass, self.mask).unwrap_or_default()
    }
}

/// Nothing met on the way.
fn clear(t: &Tr) -> Value {
    Value::Int(i32::from(t.fraction >= 1.0 && !t.startsolid))
}

/// `bulletTrace(start, end, hitCharacters, ignoreEnt)` (0x80c65fa): an
/// array of `fraction`, `position` (the segment at that fraction),
/// `entity` (undefined for the world or nothing, 0x80c66f6), `normal`
/// and `surfacetype`, keyed in that order. A hit (fraction under 1,
/// 0x80c6743) gives the struck plane and its surface's name off
/// `surfaceFlags` bits 20..24 (0x80c6806); a miss gives the segment's
/// direction (0 for a point) and `none`.
fn bullet_trace(
    host: &mut GameHost,
    cx: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let q = Query::read(a, MASK_BULLET)?;
    let t = q.locational(host);
    let hit_entity = (t.entity_num < WORLD)
        .then(|| host.ents.handle(t.entity_num))
        .flatten();
    let (normal, surface) = if t.fraction < 1.0 {
        let index = (t.surface_flags >> 20 & 0x1f) as usize;
        let name = vcod_cod4::sound::SURFACES.get(index).copied();
        (t.normal, name.unwrap_or("default"))
    } else {
        ((q.end - q.start).normalize_or_zero(), "none")
    };
    let fields = [
        ("fraction", Value::Float(t.fraction)),
        (
            "position",
            Value::Vector(q.start.lerp(q.end, t.fraction).to_array()),
        ),
        ("entity", hit_entity.map_or(Value::Undefined, Value::Entity)),
        ("normal", Value::Vector(normal.to_array())),
        ("surfacetype", Value::String(cx.intern_exact(surface))),
    ];
    let result = cx.new_array();
    for (name, value) in fields {
        let key = ArrayKey::Str(cx.intern_exact(name));
        cx.set_index(result, key, value);
    }
    Ok(Value::Array(result))
}

/// `bulletTracePassed(start, end, hitCharacters, ignoreEnt)` (0x80bee92):
/// the bullet trace met nothing.
fn bullet_trace_passed(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    Ok(clear(&Query::read(a, MASK_BULLET)?.locational(host)))
}

/// `sightTracePassed(start, end, hitCharacters, ignoreEnt)` (0x80c0198):
/// `G_SightTrace` under the sight mask met nothing; a body is its box.
fn sight_trace_passed(
    host: &mut GameHost,
    _: &mut Cx,
    _: Option<Target>,
    a: &[Value],
) -> Result<Value, ErrorKind> {
    let q = Query::read(a, MASK_SIGHT)?;
    let t = trace4(host, q.start, q.end, Vec3::ZERO, Vec3::ZERO, q.pass, q.mask);
    Ok(clear(&t.unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::cod4::clip::{Body4, Brush4};

    /// The mp_crash host as `cod4_boot` loads it, with a body linked; `None`
    /// without `COD4_DIR`.
    fn level() -> Option<crate::game::script::ScriptRuntime> {
        crate::game::cod4::boot::booted_for_test()
    }

    #[test]
    fn a_body_cuts_a_trace_that_hits_characters() {
        let Some(mut rt) = level() else {
            return;
        };
        // Open air over the map's middle.
        let (s, e) = (
            Vec3::new(429.0, 126.0, 1500.0),
            Vec3::new(629.0, 126.0, 1500.0),
        );
        let body = Body4 {
            entity: 3,
            origin: Vec3::new(529.0, 126.0, 1460.0),
            mins: Vec3::new(-15.0, -15.0, 0.0),
            maxs: Vec3::new(15.0, 15.0, 70.0),
            contents: CONTENTS_BODY,
            owner: None,
        };
        let host = &mut rt.host;
        let clear = trace4(host, s, e, Vec3::ZERO, Vec3::ZERO, NONE, MASK_BULLET).unwrap();
        assert_eq!(clear.fraction, 1.0, "the spot must be open air");
        host.cod4_mut().trace_bodies = vec![body];
        let hit = trace4(host, s, e, Vec3::ZERO, Vec3::ZERO, NONE, MASK_BULLET).unwrap();
        assert!((hit.fraction - 0.425).abs() < 0.01, "{}", hit.fraction);
        assert_eq!(hit.entity_num, 3);
        // Without the characters, or passing the body, it goes through.
        let off = trace4(
            host,
            s,
            e,
            Vec3::ZERO,
            Vec3::ZERO,
            NONE,
            MASK_BULLET & !CONTENTS_BODY,
        );
        assert_eq!(off.unwrap().fraction, 1.0);
        let pass = trace4(host, s, e, Vec3::ZERO, Vec3::ZERO, 3, MASK_BULLET).unwrap();
        assert_eq!(pass.fraction, 1.0);
        // A brush entity with no model of its own clips nothing.
        host.cod4_mut().trace_brushes = vec![Brush4 {
            entity: 90,
            model: 0,
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
        }];
        let t = trace4(host, s, e, Vec3::ZERO, Vec3::ZERO, 3, MASK_SIGHT).unwrap();
        assert_eq!(t.fraction, 1.0);
    }

    /// `bulletTrace`'s array, keyed as 0x80c65fa adds them: a miss reads
    /// the segment's direction and `none`, a hit the struck plane and a
    /// named surface; the world's entity reads undefined either way.
    #[test]
    fn bullet_trace_fills_its_array_for_a_miss_and_a_hit() {
        let Some(mut rt) = level() else {
            return;
        };
        rt.with_host(|host, cx| {
            let call = |host: &mut GameHost, cx: &mut Cx, s: Vec3, e: Vec3| {
                let a = [
                    Value::Vector(s.to_array()),
                    Value::Vector(e.to_array()),
                    Value::Int(0),
                    Value::Undefined,
                ];
                let Ok(Value::Array(arr)) = bullet_trace(host, cx, None, &a) else {
                    panic!("bulletTrace returns an array");
                };
                let keys: Vec<String> = cx
                    .array_keys_newest_first(arr)
                    .into_iter()
                    .rev()
                    .map(|k| match k {
                        ArrayKey::Str(a) => cx.resolve(a).to_string(),
                        k => panic!("{k:?}"),
                    })
                    .collect();
                assert_eq!(
                    keys,
                    ["fraction", "position", "entity", "normal", "surfacetype"]
                );
                let field = |cx: &mut Cx, k: &str| {
                    let key = ArrayKey::Str(cx.intern_exact(k));
                    cx.get_index(arr, key)
                };
                let surface = match field(cx, "surfacetype") {
                    Value::String(a) => cx.resolve(a).to_string(),
                    v => panic!("{v:?}"),
                };
                let (Value::Float(f), Value::Vector(n), entity) = (
                    field(cx, "fraction"),
                    field(cx, "normal"),
                    field(cx, "entity"),
                ) else {
                    panic!("fraction and normal");
                };
                assert_eq!(entity, Value::Undefined);
                (f, Vec3::from(n), surface)
            };
            // Open air over the map's middle.
            let s = Vec3::new(429.0, 126.0, 1500.0);
            let (f, n, surface) = call(host, cx, s, s + Vec3::new(0.0, 200.0, 0.0));
            assert_eq!((f, n, surface.as_str()), (1.0, Vec3::Y, "none"));
            // Straight down onto the ground.
            let (f, n, surface) = call(host, cx, s, s - Vec3::new(0.0, 0.0, 4000.0));
            assert!(f < 1.0, "{f}");
            assert!(n.z > 0.5, "{n:?}");
            assert_ne!(surface, "none");
        });
    }
}
