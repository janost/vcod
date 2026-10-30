//! CoD2's dropped weapons on the shared item path (`game::item`,
//! `game::pickup`): `Drop_Item`'s launch (Mac 0x1b1128), whose speeds are
//! cvars. The use key's swap (`Touch_Item` with `touched` 0, Mac
//! 0x1b1bea) is `game::cod2_use`'s. docs/research/cod2-combat.md 6.3.

use glam::Vec3;
use vcod_gsc::{Cx, EntId, ErrorKind};

use super::host::GameHost;

/// `g_dropForwardSpeed`, `g_dropUpSpeedBase` and `g_dropUpSpeedRand`'s
/// defaults (`Dvar_RegisterFloat`, 10, 10 and 5). VERIFIED.
const FORWARD: f32 = 10.0;
const UP_BASE: f32 = 10.0;
const UP_RAND: f32 = 5.0;

fn cvar(host: &GameHost, name: &str, default: f32) -> f32 {
    match host.cvars.get(name) {
        "" => default,
        v => super::builtins::cvar::atof(v),
    }
}

/// `Drop_Item(player, item, 0, 0)`'s launch (lnxded 0x81063d4): from the
/// player's `r.currentOrigin` raised by half its box's height, along
/// `AngleVectors((0, yaw, 0))`'s forward times `g_dropForwardSpeed`,
/// climbing at `g_dropUpSpeedBase + crandom() * g_dropUpSpeedRand` off the
/// game's `rand()`, under `TR_GRAVITY` from this frame. VERIFIED, the
/// stores. `yaw` is in degrees.
pub fn launch_off(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    index: usize,
    yaw: f32,
) -> Result<EntId, ErrorKind> {
    let forward = cvar(host, "g_dropForwardSpeed", FORWARD);
    let base = cvar(host, "g_dropUpSpeedBase", UP_BASE);
    let rand = cvar(host, "g_dropUpSpeedRand", UP_RAND);
    let [dir, _, _] = vcod_common::x87::angle_vectors(Vec3::new(0.0, yaw, 0.0));
    let mut velocity = (dir.as_dvec3() * f64::from(forward)).as_vec3();
    let c = host.cod2_crandom();
    velocity.z = (f64::from(velocity.z) + c * f64::from(rand) + f64::from(base)) as f32;
    let body = host.cod2().clients[slot].body;
    let player = host
        .ents
        .handle(slot as u32)
        .ok_or(ErrorKind::BadType("no such player"))?;
    let mut origin = match body {
        Some(b) => b.origin,
        None => Vec3::from(super::item::origin_of(host, cx, player)),
    };
    // `z + 0.5 * (maxs.z - mins.z)` on the stack, one store.
    let half = body.map_or(35.0, |b| 0.5 * (f64::from(b.maxs.z) - f64::from(b.mins.z)));
    origin.z = (f64::from(origin.z) + half) as f32;
    super::item::launch(host, cx, index, origin, velocity, Some(slot as u8))
}

/// A thrown weapon whose start waits for the dropper's posed tag: the
/// script call that dropped it cannot reach the body's pose, so
/// [`super::cod2::Cod2Level`] places it before the next frame
/// ([`Cod2Level::place_drops`](super::cod2::Cod2Level)).
#[derive(Clone, Debug)]
pub struct TagDrop {
    pub item: EntId,
    pub slot: usize,
    pub tag: String,
}

/// `Drop_Weapon`'s spin (lnxded 0x81064de): three `crandom()` draws, the
/// first the roll's at 60 degrees a second, then the yaw's at 40, then
/// the pitch's at 50, each stored. VERIFIED, the calls and rodata
/// 0x815a3e8..0x815a3f0.
pub fn spin(host: &mut GameHost) -> Vec3 {
    let roll = host.cod2_crandom();
    let yaw = host.cod2_crandom();
    let pitch = host.cod2_crandom();
    Vec3::new(
        (50.0 * pitch) as f32,
        (40.0 * yaw) as f32,
        (60.0 * roll) as f32,
    )
}

/// `G_RunItem`'s flight (lnxded 0x8107de6), CoD2's own: the arc read 50 ms
/// ahead (a unit lower when that is under 0.1 squared off), swept from
/// `origin`; a stop eases the item there over 50 ms (`TR_LINEAR_STOP` at 20
/// times the move) after a slide along a steep face met at once (under
/// 0.01 of the way, a normal's z under 0.5); a stop under 0.01 of the way
/// rests it on a face whose normal climbs, else frees it, as does a rest in
/// `CONTENTS_NODROP`. VERIFIED, the calls, the stores and rodata
/// 0x815a538..0x815a540; INFERRED, which `trace_t` byte is allsolid.
pub fn run_flight(
    world: &vcod_common::collision::CollisionWorld,
    pos: &mut vcod_common::net::trajectory::Trajectory,
    origin: &mut Vec3,
    (mins, maxs): (Vec3, Vec3),
    mask: u32,
    now_ms: i32,
) -> super::item::Ran {
    use super::item::Ran;
    use vcod_common::net::trajectory::{TR_LINEAR_STOP, Trajectory};
    const CONTENTS_NODROP: u32 = 0x8000_0000;
    let lerp = |a: Vec3, b: Vec3, f: f32| {
        (a.as_dvec3() + (b.as_dvec3() - a.as_dvec3()) * f64::from(f)).as_vec3()
    };
    let mut to = pos.evaluate(now_ms + 50);
    if (origin.as_dvec3() - to.as_dvec3()).length_squared() < f64::from(0.1f32) {
        to.z -= 1.0;
    }
    let mut tr = world.item_trace(*origin, to, mins, maxs, mask);
    if tr.fraction < 1.0 {
        let mut end = lerp(*origin, to, tr.fraction);
        if !tr.allsolid && tr.fraction < 0.01 && tr.normal.z < 0.5 {
            let d = to - *origin;
            let s = (1.0 - vcod_common::x87::dot(d, tr.normal)) as f32;
            to = (to.as_dvec3() + tr.normal.as_dvec3() * f64::from(s)).as_vec3();
            tr = world.item_trace(end, to, mins, maxs, mask);
            end = lerp(end, to, tr.fraction);
        }
        *pos = Trajectory {
            tr_type: TR_LINEAR_STOP,
            tr_time: now_ms,
            tr_duration: 50,
            base: *origin,
            delta: (end - *origin) * 20.0,
        };
        *origin = end;
    } else {
        *origin = to;
    }
    if tr.fraction >= 0.01 {
        return Ran::Flew;
    }
    if tr.normal.z <= 0.0 || world.point_contents(*origin) & CONTENTS_NODROP != 0 {
        return Ran::NoDrop;
    }
    Ran::Landed {
        ground: world.entity_num(&tr),
        normal: tr.normal,
    }
}
