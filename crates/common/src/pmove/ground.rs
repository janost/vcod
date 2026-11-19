//! `PM_GroundTrace`, `PM_CorrectAllSolid` and the water level.

use super::MIN_WALK_NORMAL;
use super::state::{MoveRules, PlayerState, PmInput};
use crate::movetrace::MoveWorld;
use glam::Vec3;

// Water. Retail 1.1 MP has no swim and no water jump: `PmoveSingle`
// dispatches to the ladder, walk and air moves only (0x34305-0x34322), and
// the water level reaches the move through the wade scale, the friction and
// the fall damage (docs/research/cod11-mantle.md, "Water").
pub const WATER_FRICTION: f32 = 1.0;

/// A `jump` (`jumpbk` on a backward cmd) anim event raised inside a move:
/// by `PM_CheckJump`, forced, by `PM_GroundTrace`'s kickoff, unforced (it
/// takes the legs only once their timer reads under 50), or by
/// `PM_GroundTraceMissed` over a drop of more than 64, forced. The event's
/// timer is the clause's duration + 50; one raised by the move's first
/// ground trace is run down by the same move's `PM_DropTimers`
/// (player-model-anim-system.md, "The takeoff and the landing").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Takeoff {
    pub forced: bool,
    /// `cmd.forwardmove < 0`, which picks `jumpbk`.
    pub back: bool,
    /// Raised ahead of the move's `PM_DropTimers`.
    pub before_timers: bool,
}

impl Takeoff {
    pub(super) fn raised(forced: bool, input: &PmInput) -> Self {
        Takeoff {
            forced,
            back: input.forward < 0.0,
            before_timers: false,
        }
    }
}

/// Q3 `bg_pmove.c` `PM_GroundTrace`'s kickoff test: the player's own velocity
/// is carrying it off the plane it is standing on. A plain `velocity.z <= 0.0`
/// test fails in its place: OVERCLIP leaves a hair of positive z after a floor
/// clip and the player would stay airborne.
fn thrown_off_ground(ps: &PlayerState, normal: Vec3) -> bool {
    ps.velocity.z > 0.0 && ps.velocity.dot(normal) > 10.0
}

/// `PM_CorrectAllSolid`'s nudges (`.rodata 0x70a40`), tried in this order.
const ALL_SOLID_NUDGES: [[f32; 3]; 26] = [
    [0.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [0.0, -1.0, 1.0],
    [1.0, 0.0, 1.0],
    [0.0, 1.0, 1.0],
    [-1.0, 0.0, 0.0],
    [0.0, -1.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 0.0, -1.0],
    [0.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
    [-1.0, -1.0, 0.0],
    [1.0, -1.0, 0.0],
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
];

/// The mover's own trace: 1.5's and UO's `PM_trace`
/// ([`MoveWorld::pm_trace`]), 1.1's plain `pm->trace`.
pub(super) fn pm_trace(
    ps: &PlayerState,
    world: &MoveWorld,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    mask: u32,
) -> crate::collision::Trace {
    if ps.rules.v15() {
        world.pm_trace(start, end, mins, maxs, mask)
    } else {
        world.box_trace(start, end, mins, maxs, mask)
    }
}

/// `PM_CorrectAllSolid` (`game.mp.i386.so` 0x30214): the first one-unit
/// nudge whose box is clear, the origin moved there and dropped up to a
/// unit, and that drop's trace as the ground trace. `None` when every nudge
/// is in solid, which leaves the player in the air with no jump origin, so
/// the slide's allsolid return and the step's gate hold it where it is
/// (docs/research/cod11-mantle.md, "A start inside a solid").
fn correct_all_solid(
    ps: &mut PlayerState,
    world: &MoveWorld,
    mask: u32,
) -> Option<crate::collision::Trace> {
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let free = ALL_SOLID_NUDGES
        .iter()
        .map(|n| ps.origin + Vec3::from_array(*n))
        .find(|p| !pm_trace(ps, world, *p, *p, mins, maxs, mask).startsolid)?;
    let t = pm_trace(ps, world, free, free - Vec3::Z, mins, maxs, mask);
    ps.origin = t.endpos;
    Some(t)
}

/// `PM_GroundTraceMissed`'s `pml.almostGroundPlane` on 1.5 (0x21a17) and
/// UO (0x2656d): a body already off the ground is near it when a unit's
/// drop meets a floor, one that has just left it when a 64-unit drop
/// meets one within that unit. 1.1's (0x3037c) clears the field it lacks.
fn near_ground_missed(ps: &PlayerState, world: &MoveWorld, mask: u32) -> bool {
    if !ps.rules.v15() {
        return false;
    }
    let reach = if ps.on_ground { 64.0 } else { 1.0 };
    let t = world.box_trace(
        ps.origin,
        ps.origin - Vec3::Z * reach,
        ps.mins(),
        ps.maxs(),
        world.pm_mask(mask),
    );
    if ps.on_ground {
        t.fraction < 1.0 / 64.0
    } else {
        t.fraction < 1.0
    }
}

/// Q3 `bg_pmove.c` `PM_GroundTrace`.
pub(super) fn ground_trace(ps: &mut PlayerState, world: &MoveWorld, mask: u32, input: &PmInput) {
    // From a quarter unit above the origin to a quarter below (0x304c0..
    // 0x304d3, `.rodata 0x70ba8`): a box resting on a face starts clear. On
    // a mounted gun from the origin to a unit below (0x304a5..0x304b6).
    let reach = if ps.mounted.is_some() {
        (0.0, 1.0)
    } else {
        (0.25, 0.25)
    };
    let (x, y) = (ps.origin.x, ps.origin.y);
    let end = ps.origin - Vec3::Z * reach.1;
    let mut t = pm_trace(
        ps,
        world,
        ps.origin + Vec3::Z * reach.0,
        end,
        ps.mins(),
        ps.maxs(),
        mask,
    );
    if t.allsolid {
        match correct_all_solid(ps, world, mask) {
            Some(drop) => t = drop,
            None => {
                ps.jump_origin_z = 0.0;
                ps.jump_penalty = false;
                lose_ground(ps);
                return;
            }
        }
    }
    // A start in solid that is not all solid (a terrain clip's, which never
    // reads all solid) is traced again from a thousandth under the origin
    // (0x30540..0x3058c, `.rodata 0x70bac`); a start in solid there too
    // leaves the ground, the jump's origin kept (0x3065e). 1.5 0x21e26 and
    // UO alike (cod11-mantle.md, "A start in solid on the ground trace").
    // The x, y and end are the first trace's, the z the corrected origin's.
    if t.startsolid {
        let from = Vec3::new(x, y, ps.origin.z - 0.001);
        t = pm_trace(ps, world, from, end, ps.mins(), ps.maxs(), mask);
        if t.startsolid {
            lose_ground(ps);
            return;
        }
    }
    // Any hit ends the jump's step allowance, thrown off or not (0x305c8).
    // 1.5 keeps the takeoff height through a landing (docs/research/
    // cod15-movement.md, "The jump penalty").
    if t.fraction < 1.0 && ps.rules == MoveRules::V1_1 {
        ps.jump_origin_z = 0.0;
    }
    // The thrown-off arm opens with `pm_flags` 0x10 (1.1 0x305d1, 1.5
    // 0x21f2c): a body on the rungs rising off the floor keeps the ground
    // (cod11-mantle.md, "Ladders").
    let thrown_off = !ps.on_ladder && thrown_off_ground(ps, t.normal);
    // 1.5's steep branch ends the penalty (0x220f5, 0x22107).
    if ps.rules.v15() && t.fraction < 1.0 && !thrown_off && t.normal.z < MIN_WALK_NORMAL {
        ps.jump_penalty = false;
        ps.jump_origin_z = 0.0;
    }
    // The kickoff raises the takeoff anim unforced, a step off a drop of
    // more than 64 forced (`PM_GroundTrace` 1.1 0x30474, 1.5 0x31ccd's
    // caller, UO alike; `PM_GroundTraceMissed` 1.1 0x3037c, 1.5 0x31a17).
    if t.fraction < 1.0 && thrown_off {
        ps.takeoff.get_or_insert(Takeoff::raised(false, input));
    } else if t.fraction >= 1.0 && ps.on_ground && !ps.on_ladder {
        // `PM_GroundTraceMissed` traces without `PM_trace`, under the
        // move's mask as it stands.
        let drop = world.box_trace(
            ps.origin,
            ps.origin - Vec3::Z * 64.0,
            ps.mins(),
            ps.maxs(),
            world.pm_mask(mask),
        );
        if drop.fraction >= 1.0 {
            ps.takeoff.get_or_insert(Takeoff::raised(true, input));
        }
    }
    ps.ground_plane = (t.fraction < 1.0 && !thrown_off).then_some(t.normal);
    ps.near_ground = if t.fraction < 1.0 {
        !thrown_off
    } else {
        near_ground_missed(ps, world, mask)
    };
    if t.fraction < 1.0 && t.normal.z >= MIN_WALK_NORMAL && !thrown_off {
        ps.on_ground = true;
        // `groundEntityNum` is the trace's own entity (0x30732).
        ps.ground_entity = world.entity_num(&t);
        ps.ground_normal = t.normal;
        ps.ground_surface_flags = t.surface_flags;
    } else {
        ps.on_ground = false;
        ps.ground_normal = Vec3::Z;
        ps.ground_surface_flags = 0;
    }
}

/// What a ground trace that cannot stand the body clears: `groundEntityNum`
/// 1023 and `pml.groundPlane`, `pml.walking` and 1.5's near flag.
fn lose_ground(ps: &mut PlayerState) {
    ps.ground_plane = None;
    ps.near_ground = false;
    ps.on_ground = false;
    ps.ground_normal = Vec3::Z;
    ps.ground_surface_flags = 0;
}

/// Feet, waist and eye point-contents samples.
pub(super) fn set_water_level(ps: &mut PlayerState, world: &MoveWorld) {
    use crate::collision::CONTENTS_WATER;
    ps.water_level = 0;
    let o = ps.origin;
    if world.point_contents(Vec3::new(o.x, o.y, o.z + 1.0)) & CONTENTS_WATER == 0 {
        return;
    }
    ps.water_level = 1;
    let vh = ps.view_height();
    let at = |z: f32| world.point_contents(Vec3::new(o.x, o.y, z)) & CONTENTS_WATER != 0;
    if at(o.z + vh * 0.5) {
        ps.water_level = 2;
        if at(o.z + vh) {
            ps.water_level = 3;
        }
    }
}

/// The ground trace hit a `surfaceparm slick` material (pml+0x50 bit 0x2).
pub(super) fn on_slick(ps: &PlayerState) -> bool {
    ps.ground_surface_flags & crate::collision::SURF_SLICK != 0
}
