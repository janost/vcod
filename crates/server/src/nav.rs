//! The bots' navigation graph: a grid flood-filled out of the map's spawn
//! points, each edge proven by running pmove along it, and A* over it.
//! Design and build times: `docs/research/bot-navigation.md`.

use crate::game::trigger::{BrushHull, box_contacts_hulls};
use crate::server::FRAME_MS;
use glam::Vec3;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use vcod_common::collision::{CollisionWorld, MASK_PLAYERSOLID, Prim};
use vcod_common::movetrace::MoveWorld;
use vcod_common::net::msg::{self, NULL_USERCMD, UserCmd};
use vcod_common::pmove::cmd::{ANGLE2SHORT, EventRing, chop, player_step};
use vcod_common::pmove::{FallHeights, MoveRules, PlayerState};

mod disk;
pub use disk::set_dir as set_disk_cache;

mod build;
mod cache;
mod follower;
mod graph;
mod probe;
mod search;
mod walk;
#[cfg(test)]
use build::ladders;
use build::{
    ARRIVE, ASIDE_DROP, COAST_TICKS, LADDER_TICKS, LEVEL, MAX_EXPANSIONS, MISS_EYE, SPACING_MAX,
    SPACING_MIN, Z_MERGE, max_drop, paced_drop, stands,
};
pub use build::{LADDER_PITCH, SIDESTEP_MAX, SIDESTEP_MIN};
use cache::{CacheKey, Cancelled};
pub use cache::{MantleProbe, NavJob, clear_cache, graph_for};
pub(crate) use follower::PLAN_SLICE;
use follower::{
    CLEAR_STEPS, CLEAR_STEPS_FAR, JUMP_PASS, OFF_PATH_STEPS, PULL_PROBE, REACH, RETARGET,
    RISK_ASIDE, RISK_COST, ROAM_MIN, WALKABLE_TRIES, deadly_fall,
};
pub use follower::{
    CROWD_COST, DANGER_COST, FLANK_COST, Follower, Footing, LEAP_FOOT, fall_that_kills,
};
use graph::seed_bounds;
pub use graph::{Edge, NavGraph};
pub(crate) use probe::{CreaseCache, CreaseRead, drop_ahead, hazard, wall_ahead};
pub use probe::{Creases, UNPIN_SPEED, UNPIN_TICKS, lip_ahead, push_off, unpin_yaw};
use probe::{jump_clear, straight_floor, under_ground};
pub use search::{NOISE_CELL, Plan, ROUTE_NOISE, Search};
pub(crate) use walk::WalkRules;
use walk::{
    CANCEL, CREEP_TOP_PACE, FALLS, Gait, LADDER_NEAR, RULES, SIDESTEP, SPARE_CORE, Sim, Trail,
    Walked, a_miss_hurts, as_a_bot, beside_a_fall_that_hurts, body, box_flat, coasts_off,
    creep_takes_a_turn, drains, ladder_near, own_step, paced_key_for, par_map, settle, walk,
    walk_as, walk_sliding, walk_to, walk_with,
};
pub use walk::{
    CREEP, Creep, PINNED, PinJump, PinTick, WalkCarry, WalkStep, back_move, climb_yaw, falls,
    jump_key, jump_ready, jump_under, leap_cue, on_jump_top, paced_key, ready_to_leap, tick_times,
    with_falls, with_rules, with_step,
};

/// Every spawn point in an ents lump: the gametypes' `mp_*_spawn*` classes
/// and `info_player_start`.
pub fn spawn_points(entities: &str) -> Vec<[f32; 3]> {
    vcod_common::bsp::entity_blocks(entities)
        .iter()
        .filter(|b| {
            b.get("classname").is_some_and(|c| {
                c == "info_player_start" || (c.starts_with("mp_") && c.contains("_spawn"))
            })
        })
        .filter_map(|b| vcod_common::bsp::parse_vec3(b.get("origin")?))
        .collect()
}
