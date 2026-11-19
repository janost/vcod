//! What CoD2 1.3's and CoD4 1.7's pmove ports share token for token:
//! `pml_t`, the touch list, and the playerstate both name alike
//! ([`PmState`]) with the few functions that read only it. Everything whose
//! float handling or retail evidence differs stays in each game's crate
//! (docs/research/cod4-pmove.md 1). The body's stance is
//! [`crate::pmove::Stance`], CoD1's too.

use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
use glam::Vec3;

/// `pml_t`: what one `PmoveSingle` keeps beside the playerstate. `T` is the
/// game's trace.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pml<T> {
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub frametime: f32,
    pub msec: i32,
    pub walking: bool,
    pub ground_plane: bool,
    pub almost_ground_plane: bool,
    pub ground_trace: T,
    pub previous_origin: Vec3,
    pub previous_velocity: Vec3,
}

/// `touchents` and `numtouch`'s cap.
pub const MAX_TOUCH: usize = 32;

/// `pmove_t.touchents`: every entity the cmd met, first meeting first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TouchList(Vec<u32>);

impl TouchList {
    /// `PM_AddTouchEnt`: once each, never the world or no entity, at most
    /// [`MAX_TOUCH`].
    pub fn add(&mut self, entity: u32) {
        if entity != ENTITYNUM_WORLD
            && entity != ENTITYNUM_NONE
            && self.0.len() < MAX_TOUCH
            && !self.0.contains(&entity)
        {
            self.0.push(entity);
        }
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn into_vec(self) -> Vec<u32> {
        self.0
    }
}

/// The playerstate fields CoD2's and CoD4's pmove name alike, and the
/// `pm_flags` bits each game numbers its own way.
pub trait PmState {
    const PMF_PRONE: u32;
    const PMF_DUCKED: u32;
    const PMF_LADDER: u32;
    const PMF_LADDER_LEFT: u32;
    const PMF_JUMPING: u32;

    fn command_time(&mut self) -> &mut i32;
    fn pm_flags(&mut self) -> &mut u32;
    fn pm_time(&mut self) -> &mut i32;
    fn origin(&mut self) -> &mut Vec3;
    fn velocity(&mut self) -> &mut Vec3;
    fn ground_entity_num(&mut self) -> &mut i32;
    fn jump_origin_z(&mut self) -> &mut f32;
    /// `legsTimer` and `torsoTimer`.
    fn timers(&mut self) -> [&mut i32; 2];
}

/// `PM_ClearLadderFlag` (CoD2 0x80e8026): a player taken off the ladder
/// notes it.
pub fn leave_ladder<P: PmState>(ps: &mut P) {
    let flags = ps.pm_flags();
    if *flags & P::PMF_LADDER != 0 {
        *flags |= P::PMF_LADDER_LEFT;
    }
    *flags &= !P::PMF_LADDER;
}

/// `Jump_ClearState` (CoD2 0x80dc718).
pub fn jump_clear<P: PmState>(ps: &mut P) {
    *ps.pm_flags() &= !P::PMF_JUMPING;
    *ps.jump_origin_z() = 0.0;
}

/// `PM_GroundTrace`'s way off the ground: no ground entity, no plane, no
/// walk.
pub fn airborne<P: PmState, T>(ps: &mut P, pml: &mut Pml<T>) {
    *ps.ground_entity_num() = ENTITYNUM_NONE as i32;
    pml.ground_plane = false;
    pml.almost_ground_plane = false;
    pml.walking = false;
}
