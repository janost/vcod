use super::*;
use super::{
    angles::*, footsteps::*, frame::*, ground::*, jump::*, ladder::*, land::*, prone::*, slide::*,
    stance::*, state::*, walk::*,
};
use crate::collision::MASK_PLAYERSOLID;
use crate::collision::{CollisionWorld, test_world};
use crate::movetrace::MoveWorld;
use crate::movetrace::{Body, CONTENTS_BODY};
use crate::net::event_ids::EV_STEP_VIEW;
use crate::net::event_ids::{
    EV_FOOTSTEP_RUN_BASE, EV_FOOTSTEP_WALK_BASE, EV_JUMP_BASE, EV_LANDING_BASE,
};
use crate::net::event_ids::{EV_STANCE_FORCE_CROUCH, EV_STANCE_FORCE_PRONE, EV_STANCE_FORCE_STAND};
use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
use crate::weapon::WeaponDef;
use crate::x87;

mod angles;
mod bodies;
mod footsteps;
mod frame;
mod ground;
mod jump;
mod ladder;
mod land;
mod lean;
mod prone;
mod slide;
mod spectator;
mod stance;
mod uo;
mod walk;

fn flat() -> CollisionWorld {
    test_world(&[])
}

/// Flat ground whose material carries the dirt sound surface (6).
fn dirt_flat() -> CollisionWorld {
    crate::collision::synthetic_world(
        &[(
            "textures/test/dirt",
            crate::collision::CONTENTS_SOLID,
            6 << 20,
        )],
        &[(0, [-2048.0, -2048.0, -16.0], [2048.0, 2048.0, 0.0])],
    )
}

fn tick(ps: &mut PlayerState, input: &PmInput, w: &MoveWorld, n: usize) {
    for _ in 0..n {
        pmove(ps, input, w, 1.0 / 125.0, &[]);
    }
}

fn body(x: f32, y: f32, z: f32) -> Body {
    Body {
        entity: 7,
        origin: Vec3::new(x, y, z),
        mins: Vec3::new(-15.0, -15.0, 0.0),
        maxs: Vec3::new(15.0, 15.0, 70.0),
        contents: CONTENTS_BODY,
    }
}

fn v15(origin: Vec3) -> PlayerState {
    PlayerState {
        rules: MoveRules::V1_5,
        ..PlayerState::spawn(origin, 0.0)
    }
}
