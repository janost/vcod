//! Contains routines ported from the Open Dynamics Engine 0.5, Copyright (C) 2001-2004
//! Russell L. Smith, under its BSD license (LICENSE-ODE-BSD.txt beside this file). See NOTICE.
//!
//! The rigid body core iw3mp.exe 1.7 runs its ragdolls on: ODE 0.5 as IW
//! modified it, read off the binary (cod4-player-anims.md 10.4). Bodies,
//! masses, the ball, hinge, angular motor and contact joints, the island
//! walk and IW's QuickStep; and the ragdoll world's contacts, IW's capsule
//! colliders and near callback (`collide`, 10.6). Not wired into
//! `ragdoll` yet.

pub mod body;
pub mod collide;
pub mod joint;
pub mod mass;
pub mod math;
pub mod quickstep;
pub mod rotation;
pub mod world;

#[cfg(test)]
mod tests;

pub use body::{AutoDisable, Body};
pub use joint::{Joint, Kind};
pub use mass::Mass;
pub use world::{BodyId, JointId, Params, World};
