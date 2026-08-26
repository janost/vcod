//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc.,
//! and the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company. See NOTICE.
//!
//! Q3/RTCW-derived player movement on top of `collision::box_trace`. Every
//! constant's provenance: docs/research/bsp-ibsp59-format.md, "Movement
//! constants and their provenance".

use crate::collision::CollisionWorld;
use glam::Vec3;

pub const GRAVITY: f32 = 800.0;
pub const SPEED_RUN: f32 = 190.0;
pub const SCALE_WALK: f32 = 0.4;
pub const SCALE_CROUCH: f32 = 0.65;
pub const SCALE_PRONE: f32 = 0.15;
pub const JUMP_VELOCITY: f32 = 250.0;
pub const PM_ACCELERATE: f32 = 10.0;
pub const PM_AIRACCELERATE: f32 = 1.0;
pub const PM_FRICTION: f32 = 6.0;
// Not Q3's 100; the doc section explains the ratio and the stance scaling in `friction`.
pub const PM_STOPSPEED: f32 = 60.0;
pub const STEPSIZE: f32 = 18.0;
pub const OVERCLIP: f32 = 1.001;
pub const MIN_WALK_NORMAL: f32 = 0.7;
pub const MAX_CLIP_PLANES: usize = 5;
pub const HALF_WIDTH: f32 = 15.0; // bbox is (-15,-15,0)..(15,15,height)
pub const HEIGHT_STAND: f32 = 70.0;
pub const HEIGHT_CROUCH: f32 = 50.0;
pub const HEIGHT_PRONE: f32 = 30.0;
pub const VIEW_STAND: f32 = 60.0;
pub const VIEW_CROUCH: f32 = 40.0;
pub const VIEW_PRONE: f32 = 11.0;
pub const MAX_FRAME_MS: f32 = 66.0;
pub const LEAN_MAX: f32 = 28.0; // eye offset in units; roll is lean/2 degrees
pub const LEAN_TIME_TO_MS: f32 = 280.0;
pub const LEAN_TIME_FROM_MS: f32 = 350.0;

// Water: RTCW-MP bg_pmove.c multipliers against CoD's absolute speeds.
// Swim cap is SCALE_SWIM * SPEED_RUN; no lava/slime exists in CoD maps.
pub const SCALE_SWIM: f32 = 0.5;
pub const WATER_ACCELERATE: f32 = 4.0;
pub const WATER_FRICTION: f32 = 1.0;
/// Idle wish toward the bottom while swimming.
pub const WATER_SINK_SPEED: f32 = 60.0;
pub const WATERJUMP_FORWARD: f32 = 200.0;
pub const WATERJUMP_UP: f32 = 350.0;
pub const WATERJUMP_TIME_MS: f32 = 2000.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stance {
    Stand,
    Crouch,
    Prone,
}

impl Stance {
    /// Bbox height above the feet.
    pub fn height(self) -> f32 {
        match self {
            Stance::Stand => HEIGHT_STAND,
            Stance::Crouch => HEIGHT_CROUCH,
            Stance::Prone => HEIGHT_PRONE,
        }
    }

    /// Fraction of `SPEED_RUN`. Slow-walk is an input modifier, not a stance.
    pub fn speed_scale(self) -> f32 {
        match self {
            Stance::Stand => 1.0,
            Stance::Crouch => SCALE_CROUCH,
            Stance::Prone => SCALE_PRONE,
        }
    }

    /// Eye height above the feet.
    pub fn view_height(self) -> f32 {
        match self {
            Stance::Stand => VIEW_STAND,
            Stance::Crouch => VIEW_CROUCH,
            Stance::Prone => VIEW_PRONE,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlayerState {
    pub origin: Vec3, // feet
    pub velocity: Vec3,
    pub yaw: f32, // radians, same convention as FlyCamera
    pub pitch: f32,
    pub stance: Stance,
    pub on_ground: bool,
    pub ground_normal: Vec3,
    pub lean: f32, // -LEAN_MAX..LEAN_MAX
    /// 0 dry, 1 feet, 2 waist, 3 eyes under (RTCW waterlevel).
    pub water_level: u32,
    /// Remaining control lock while flying out of water; 0 when free.
    pub waterjump_ms: f32,
}

impl PlayerState {
    pub fn spawn(origin: Vec3, yaw_deg: f32) -> Self {
        PlayerState {
            origin,
            velocity: Vec3::ZERO,
            yaw: yaw_deg.to_radians(),
            pitch: 0.0,
            stance: Stance::Stand,
            on_ground: false,
            ground_normal: Vec3::Z,
            lean: 0.0,
            water_level: 0,
            waterjump_ms: 0.0,
        }
    }

    pub fn mins(&self) -> Vec3 {
        Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0)
    }

    pub fn maxs(&self) -> Vec3 {
        Vec3::new(HALF_WIDTH, HALF_WIDTH, self.stance.height())
    }

    pub fn view_height(&self) -> f32 {
        self.stance.view_height()
    }

    /// Eye and angles with the lean offset; roll = lean/2 degrees (RTCW).
    pub fn view(&self) -> ViewParams {
        let right = Vec3::new(self.yaw.sin(), -self.yaw.cos(), 0.0);
        ViewParams {
            eye: self.origin + Vec3::Z * self.view_height() + right * self.lean,
            yaw: self.yaw,
            pitch: self.pitch,
            roll: (self.lean * 0.5).to_radians(),
        }
    }
}

pub struct ViewParams {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

#[derive(Default, Clone, Copy)]
pub struct PmInput {
    pub forward: f32, // -1..1
    pub right: f32,   // -1..1
    pub jump: bool,
    pub crouch: bool, // held
    pub prone: bool,  // toggled state, main.rs owns the toggle
    pub walk_slow: bool,
    pub lean_left: bool,
    pub lean_right: bool,
}

/// `dt` in seconds, clamped to `MAX_FRAME_MS`.
pub fn pmove(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
    let dt = dt.min(MAX_FRAME_MS / 1000.0);
    update_stance(ps, input, world);
    update_lean(ps, input, world, dt);
    set_water_level(ps, world);
    ground_trace(ps, world);
    if ps.waterjump_ms > 0.0 {
        ps.waterjump_ms -= dt * 1000.0;
        if ps.waterjump_ms < 0.0 {
            ps.waterjump_ms = 0.0;
        }
    }
    if ps.waterjump_ms > 0.0 {
        water_jump_move(ps, world, dt);
    } else if ps.water_level > 1 {
        water_move(ps, input, world, dt);
    } else {
        if input.jump && ps.on_ground && ps.stance == Stance::Stand {
            ps.velocity.z = JUMP_VELOCITY;
            ps.on_ground = false;
        }
        if ps.on_ground {
            friction(ps, dt);
            walk_move(ps, input, world, dt);
        } else {
            air_move(ps, input, world, dt);
        }
    }
    ground_trace(ps, world);
    set_water_level(ps, world);
}

/// Standing back up needs headroom for the taller bbox.
fn update_stance(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld) {
    let desired = if input.prone {
        Stance::Prone
    } else if input.crouch {
        Stance::Crouch
    } else {
        Stance::Stand
    };
    if desired.height() <= ps.stance.height() {
        ps.stance = desired;
        return;
    }
    let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, desired.height());
    let t = world.box_trace(ps.origin, ps.origin, ps.mins(), maxs);
    if !t.startsolid {
        ps.stance = desired;
    }
}

/// RTCW `bg_pmove.c` `PM_UpdateLean`. Differences: leans while moving (no
/// `!cmd->forwardmove` gate), and prone blocks leaning.
fn update_lean(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
    let msec = dt * 1000.0;
    let mut dir = 0.0f32;
    if input.lean_left {
        dir -= 1.0;
    }
    if input.lean_right {
        dir += 1.0;
    }
    if ps.stance == Stance::Prone {
        dir = 0.0;
    }

    if dir == 0.0 {
        // return to center
        let step = msec / LEAN_TIME_FROM_MS * LEAN_MAX;
        ps.lean = if ps.lean > 0.0 {
            (ps.lean - step).max(0.0)
        } else {
            (ps.lean + step).min(0.0)
        };
        return;
    }
    ps.lean = (ps.lean + dir * (msec / LEAN_TIME_TO_MS) * LEAN_MAX).clamp(-LEAN_MAX, LEAN_MAX);

    // wall clamp with RTCW's lean box
    let start = ps.origin + Vec3::Z * ps.view_height();
    let mut right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    right.z = if ps.lean < 0.0 { 0.25 } else { -0.25 };
    let end = start + right * ps.lean;
    let t = world.box_trace(
        start,
        end,
        Vec3::new(-12.0, -12.0, -6.0),
        Vec3::new(12.0, 12.0, 10.0),
    );
    ps.lean *= t.fraction;
}

/// Q3 `bg_pmove.c` `PM_GroundTrace`.
fn ground_trace(ps: &mut PlayerState, world: &CollisionWorld) {
    let t = world.box_trace(ps.origin, ps.origin - Vec3::Z * 0.25, ps.mins(), ps.maxs());
    // A plain `velocity.z <= 0.0` test fails: OVERCLIP leaves a hair of
    // positive z after a floor clip and the player would stay airborne.
    let thrown_off = ps.velocity.z > 0.0 && ps.velocity.dot(t.normal) > 10.0;
    if t.fraction < 1.0 && t.normal.z >= MIN_WALK_NORMAL && !thrown_off {
        ps.on_ground = true;
        ps.ground_normal = t.normal;
        // RTCW clears the waterjump lock on touching walkable ground
        ps.waterjump_ms = 0.0;
    } else {
        ps.on_ground = false;
        ps.ground_normal = Vec3::Z;
    }
}

/// Feet, waist and eye point-contents samples; swimming starts at waist-deep.
fn set_water_level(ps: &mut PlayerState, world: &CollisionWorld) {
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

/// RTCW `bg_pmove.c` `PM_Friction`: ground friction only while walking in
/// water level <= 1, plus a water term that already applies while wading.
fn friction(ps: &mut PlayerState, dt: f32) {
    // when walking, slope movement along z does not count toward the speed
    let mut planar = ps.velocity;
    if ps.on_ground {
        planar.z = 0.0;
    }
    let speed = planar.length();
    if speed < 1.0 {
        ps.velocity.x = 0.0;
        ps.velocity.y = 0.0;
        return;
    }
    let mut drop = 0.0;
    if ps.on_ground && ps.water_level <= 1 {
        // Stop-speed floor scaled by stance so slow stances reach their wish
        // speed. Standing must stay under the ~54 u/s a shallow wall slide
        // leaves, or the player stalls on the wall.
        let control = speed.max(PM_STOPSPEED * ps.stance.speed_scale());
        drop += control * PM_FRICTION * dt;
    }
    if ps.water_level > 0 {
        drop += speed * WATER_FRICTION * ps.water_level as f32 * dt;
    }
    let scale = ((speed - drop) / speed).max(0.0);
    ps.velocity *= scale;
}

/// Desired direction (world space, unit or zero) and speed.
fn wish(ps: &PlayerState, input: &PmInput) -> (Vec3, f32) {
    let fwd = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let dir = (fwd * input.forward + right * input.right).normalize_or_zero();
    let scale = match ps.stance {
        Stance::Stand if input.walk_slow => SCALE_WALK,
        stance => stance.speed_scale(),
    };
    (dir, SPEED_RUN * scale)
}

/// Q3 `bg_pmove.c` `PM_Accelerate`.
fn accelerate(ps: &mut PlayerState, wishdir: Vec3, wishspeed: f32, accel: f32, dt: f32) {
    let current = ps.velocity.dot(wishdir);
    let add = wishspeed - current;
    if add <= 0.0 {
        return;
    }
    let accel_speed = (accel * dt * wishspeed).min(add);
    ps.velocity += wishdir * accel_speed;
}

/// Q3 `bg_pmove.c` `PM_ClipVelocity`.
fn clip_velocity(vel: Vec3, normal: Vec3) -> Vec3 {
    let backoff = vel.dot(normal)
        * if vel.dot(normal) < 0.0 {
            OVERCLIP
        } else {
            1.0 / OVERCLIP
        };
    vel - normal * backoff
}

/// Q3 `bg_pmove.c` `PM_WalkMove`.
fn walk_move(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
    // eye-deep and looking up an upward slope: swim instead of trudging
    if ps.water_level > 2 && forward3(ps).dot(ps.ground_normal) > 0.0 {
        water_move(ps, input, world, dt);
        return;
    }
    let (dir, mut wishspeed) = wish(ps, input);
    // wading clamp: linear from full speed at the ankles to the swim scale
    // with water at the eyes (RTCW `PM_WalkMove`)
    if ps.water_level > 0 {
        let ws = 1.0 - (1.0 - SCALE_SWIM) * (ps.water_level as f32 / 3.0);
        wishspeed = wishspeed.min(SPEED_RUN * ws);
    }
    // along the slope, so it costs no speed
    let dir = clip_velocity(dir, ps.ground_normal).normalize_or_zero();
    accelerate(ps, dir, wishspeed, PM_ACCELERATE, dt);
    ps.velocity = clip_velocity(ps.velocity, ps.ground_normal);
    if ps.velocity.x == 0.0 && ps.velocity.y == 0.0 {
        return;
    }
    step_slide_move(ps, world, dt, false);
}

/// Look direction including pitch; pitch is up-positive.
fn forward3(ps: &PlayerState) -> Vec3 {
    Vec3::new(
        ps.pitch.cos() * ps.yaw.cos(),
        ps.pitch.cos() * ps.yaw.sin(),
        ps.pitch.sin(),
    )
}

/// Q3 `bg_pmove.c` `PM_CmdScale`: input magnitude in command units. Q3 only
/// reads forward/right here, so a lone up key would scale to zero and the
/// sink wish would win; CoD-style play expects jump alone to swim up, so up
/// joins the magnitude.
fn cmd_scale(input: &PmInput) -> f32 {
    let up = if input.jump { 1.0 } else { 0.0 };
    let max = input.forward.abs().max(input.right.abs()).max(up);
    if max <= 0.0 {
        return 0.0;
    }
    let total = (input.forward * input.forward + input.right * input.right + up * up).sqrt();
    total / max * (127.0 / 128.0)
}

/// RTCW `bg_pmove.c` `PM_WaterMove`. No gravity: buoyancy is implicit, the
/// idle sink is a wish toward the bottom, jump is the up command.
fn water_move(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
    if try_start_water_jump(ps, world) {
        water_jump_move(ps, world, dt);
        return;
    }
    friction(ps, dt);
    let m = cmd_scale(input) * 127.0;
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    let wishvel = if m == 0.0 {
        Vec3::new(0.0, 0.0, -WATER_SINK_SPEED)
    } else {
        let mut w = forward3(ps) * (input.forward * m) + right * (input.right * m);
        w.z += if input.jump { m } else { 0.0 };
        w
    };
    let wishspeed = wishvel.length();
    let wishdir = if wishspeed > 0.0 {
        wishvel / wishspeed
    } else {
        Vec3::ZERO
    };
    let cap = SPEED_RUN * SCALE_SWIM;
    let wishspeed = if wishspeed > cap { cap } else { wishspeed };
    accelerate(ps, wishdir, wishspeed, WATER_ACCELERATE, dt);

    // crawl up underwater slopes without losing speed. RTCW also re-scales
    // the clipped velocity to its old length here, which mirrors a full sink
    // into the floor into a full-power launch the frame grounding starts
    // (their "FIXME: still have z friction underwater?" marks this spot);
    // dropping the re-scale keeps bottom contact settled.
    if ps.on_ground && ps.velocity.dot(ps.ground_normal) < 0.0 {
        ps.velocity = clip_velocity(ps.velocity, ps.ground_normal);
    }
    slide_move(ps, world, dt, false);
}

/// RTCW `bg_pmove.c` `PM_CheckWaterJump`: chest-deep against a low lip, the
/// probe 4 units up must hit solid and 20 units up must be clear.
fn try_start_water_jump(ps: &mut PlayerState, world: &CollisionWorld) -> bool {
    use crate::collision::CONTENTS_SOLID;
    if ps.waterjump_ms > 0.0 || ps.water_level != 2 {
        return false;
    }
    let flat = Vec3::new(ps.yaw.cos(), ps.yaw.sin(), 0.0);
    let spot = ps.origin + flat * 30.0 + Vec3::Z * 4.0;
    if world.point_contents(spot) & CONTENTS_SOLID == 0 {
        return false;
    }
    if world.point_contents(spot + Vec3::Z * 16.0) != 0 {
        return false;
    }
    ps.velocity = forward3(ps) * WATERJUMP_FORWARD;
    ps.velocity.z = WATERJUMP_UP;
    ps.waterjump_ms = WATERJUMP_TIME_MS;
    true
}

/// RTCW `bg_pmove.c` `PM_WaterJumpMove`: no control, extra gravity, cancels
/// once falling again (landing clears via ground_trace).
fn water_jump_move(ps: &mut PlayerState, world: &CollisionWorld, dt: f32) {
    step_slide_move(ps, world, dt, true);
    ps.velocity.z -= GRAVITY * dt;
    if ps.velocity.z < 0.0 {
        ps.waterjump_ms = 0.0;
    }
}

/// Q3 `bg_pmove.c` `PM_AirMove`.
fn air_move(ps: &mut PlayerState, input: &PmInput, world: &CollisionWorld, dt: f32) {
    let (dir, wishspeed) = wish(ps, input);
    accelerate(ps, dir, wishspeed, PM_AIRACCELERATE, dt);
    step_slide_move(ps, world, dt, true);
}

/// Q3 `bg_slidemove.c` `PM_SlideMove`. Returns true if anything was hit.
fn slide_move(ps: &mut PlayerState, world: &CollisionWorld, dt: f32, gravity: bool) -> bool {
    const NUM_BUMPS: usize = 4;
    let (mins, maxs) = (ps.mins(), ps.maxs());

    // average of start and end velocity, matching the analytic parabola
    let mut end_velocity = ps.velocity;
    if gravity {
        end_velocity.z -= GRAVITY * dt;
        ps.velocity.z = (ps.velocity.z + end_velocity.z) * 0.5;
        if ps.on_ground {
            ps.velocity = clip_velocity(ps.velocity, ps.ground_normal);
        }
    }

    let mut planes: Vec<Vec3> = Vec::with_capacity(MAX_CLIP_PLANES);
    // never turn against the ground plane or back against the original move
    if ps.on_ground {
        planes.push(ps.ground_normal);
    }
    planes.push(ps.velocity.normalize_or_zero());

    let mut time_left = dt;
    let mut bumps = 0;
    for _ in 0..NUM_BUMPS {
        let end = ps.origin + ps.velocity * time_left;
        let t = world.box_trace(ps.origin, end, mins, maxs);
        if t.allsolid {
            // trapped in solid: keep the horizontal control, kill the fall
            ps.velocity.z = 0.0;
            return true;
        }
        if t.fraction > 0.0 {
            ps.origin = t.endpos;
        }
        if t.fraction == 1.0 {
            break;
        }
        bumps += 1;
        time_left -= time_left * t.fraction;

        if planes.len() >= MAX_CLIP_PLANES {
            ps.velocity = Vec3::ZERO;
            return true;
        }
        // same plane again: nudge out along it (epsilon on non-axial planes)
        if planes.iter().any(|p| t.normal.dot(*p) > 0.99) {
            ps.velocity += t.normal;
            continue;
        }
        planes.push(t.normal);

        for (i, &plane_i) in planes.iter().enumerate() {
            if ps.velocity.dot(plane_i) >= 0.1 {
                continue;
            }
            let mut clipped = clip_velocity(ps.velocity, plane_i);
            let mut end_clipped = clip_velocity(end_velocity, plane_i);

            for (j, &plane_j) in planes.iter().enumerate() {
                if j == i || clipped.dot(plane_j) >= 0.1 {
                    continue;
                }
                clipped = clip_velocity(clipped, plane_j);
                end_clipped = clip_velocity(end_clipped, plane_j);
                if clipped.dot(plane_i) >= 0.0 {
                    continue;
                }
                // two planes: slide along their crease
                let crease = plane_i.cross(plane_j).normalize_or_zero();
                clipped = crease * crease.dot(ps.velocity);
                end_clipped = crease * crease.dot(end_velocity);

                // three planes: nowhere left to go
                if planes
                    .iter()
                    .enumerate()
                    .any(|(k, &p)| k != i && k != j && clipped.dot(p) < 0.1)
                {
                    ps.velocity = Vec3::ZERO;
                    return true;
                }
            }

            ps.velocity = clipped;
            end_velocity = end_clipped;
            break;
        }
    }

    if gravity {
        ps.velocity = end_velocity;
    }
    bumps != 0
}

/// Q3 `bg_slidemove.c` `PM_StepSlideMove`.
fn step_slide_move(ps: &mut PlayerState, world: &CollisionWorld, dt: f32, gravity: bool) {
    let start_o = ps.origin;
    let start_v = ps.velocity;
    if !slide_move(ps, world, dt, gravity) {
        return;
    }
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let (down_o, down_v) = (ps.origin, ps.velocity);

    // `allsolid`, not `startsolid`: a bbox merely touching the floor is
    // startsolid, and stepping up is how that resolves.
    let up = world.box_trace(start_o, start_o + Vec3::Z * STEPSIZE, mins, maxs);
    let step = up.endpos.z - start_o.z;
    if up.allsolid || step <= 0.0 {
        return;
    }

    ps.origin = up.endpos;
    ps.velocity = start_v;
    slide_move(ps, world, dt, gravity);

    let down = world.box_trace(ps.origin, ps.origin - Vec3::Z * step, mins, maxs);
    if !down.allsolid {
        ps.origin = down.endpos;
    }
    let stepped = (ps.origin.truncate() - start_o.truncate()).length_squared();
    let flat = (down_o.truncate() - start_o.truncate()).length_squared();
    if down.normal.z < MIN_WALK_NORMAL || flat > stepped {
        // into air, onto a too-steep slope, or the flat slide got further
        ps.origin = down_o;
        ps.velocity = down_v;
        return;
    }
    if down.fraction < 1.0 {
        ps.velocity = clip_velocity(ps.velocity, down.normal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::test_world;

    fn flat() -> CollisionWorld {
        test_world(&[])
    }

    /// A pool: dry ground west, deep water (surface z=36) over a floor at
    /// -74, a chest-deep shelf (top 0) and an ankle-deep shelf (top 30), an
    /// exit wall at x=200 rising to 15, and landing ground east of it.
    fn pool_world() -> CollisionWorld {
        crate::collision::synthetic_world(
            &[
                ("textures/test/solid", crate::collision::CONTENTS_SOLID, 0),
                ("textures/common/water", crate::collision::CONTENTS_WATER, 0),
            ],
            &[
                (0, [-600.0, -300.0, -16.0], [-200.0, 300.0, 0.0]),
                (0, [-200.0, -300.0, -90.0], [200.0, 300.0, -74.0]),
                (0, [-160.0, -100.0, -84.0], [185.0, 100.0, 0.0]),
                (0, [-200.0, 150.0, -84.0], [200.0, 260.0, 30.0]),
                (0, [200.0, -300.0, -90.0], [216.0, 300.0, 15.0]),
                (0, [240.0, -300.0, -16.0], [600.0, 300.0, 0.0]),
                (1, [-200.0, -300.0, -74.0], [200.0, 300.0, 36.0]),
            ],
        )
    }

    fn tick(ps: &mut PlayerState, input: &PmInput, w: &CollisionWorld, n: usize) {
        for _ in 0..n {
            pmove(ps, input, w, 1.0 / 125.0);
        }
    }

    #[test]
    fn falls_and_lands_on_floor() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 50.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 250);
        assert!(ps.on_ground);
        assert!(ps.origin.z.abs() < 0.5, "settled at {}", ps.origin.z);
        assert!(ps.velocity.length() < 1.0);
    }

    #[test]
    fn accelerates_to_run_speed_cap() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            250,
        );
        let h = ps.velocity.truncate().length();
        assert!((h - SPEED_RUN).abs() < 5.0, "run speed {h}");
        // yaw 0 => moving along +X
        assert!(ps.velocity.x > 100.0 && ps.velocity.y.abs() < 1.0);
    }

    #[test]
    fn friction_stops_the_player() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            250,
        );
        tick(&mut ps, &PmInput::default(), &w, 200);
        assert!(
            ps.velocity.length() < 1.0,
            "still moving at {}",
            ps.velocity.length()
        );
    }

    #[test]
    fn jump_apex_matches_constants() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let mut apex = 0.0f32;
        let jump = PmInput {
            jump: true,
            ..Default::default()
        };
        pmove(&mut ps, &jump, &w, 1.0 / 125.0);
        for _ in 0..200 {
            pmove(&mut ps, &PmInput::default(), &w, 1.0 / 125.0);
            apex = apex.max(ps.origin.z);
        }
        let expect = JUMP_VELOCITY * JUMP_VELOCITY / (2.0 * GRAVITY); // ~39
        assert!(
            (apex - expect).abs() < 3.0,
            "apex {apex}, expected ~{expect}"
        );
        assert!(ps.on_ground);
    }

    #[test]
    fn steps_up_low_ledge_but_not_high_one() {
        // z=16 ledge ahead (+X), z=40 ledge behind (-X), both reaching the
        // floor edge: 300 ticks covers ~440 units
        let w = test_world(&[
            (Vec3::new(50.0, -200.0, 0.0), Vec3::new(1024.0, 200.0, 16.0)),
            (
                Vec3::new(-1024.0, -200.0, 0.0),
                Vec3::new(-50.0, 200.0, 40.0),
            ),
        ]);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            300,
        );
        assert!(
            ps.origin.x > 60.0 && (ps.origin.z - 16.0).abs() < 0.5,
            "should stand on the ledge, at {}",
            ps.origin
        );

        let mut ps = PlayerState::spawn(Vec3::ZERO, 180.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            300,
        );
        assert!(
            ps.origin.x > -50.0 - HALF_WIDTH - 1.0 && ps.origin.z < 1.0,
            "blocked by the wall, at {}",
            ps.origin
        );
    }

    #[test]
    fn slides_along_wall() {
        let w = test_world(&[(Vec3::new(50.0, -400.0, 0.0), Vec3::new(100.0, 400.0, 100.0))]);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                right: 0.3,
                ..Default::default()
            },
            &w,
            250,
        );
        assert!(ps.origin.x < 50.0 - HALF_WIDTH + 1.0);
        assert!(
            ps.origin.y.abs() > 50.0,
            "should have slid along the wall, at {}",
            ps.origin
        );
    }

    #[test]
    fn crouch_lowers_speed_and_ceiling_blocks_standing() {
        // z=60 ceiling: crouch fits, standing does not; wide enough for the
        // ~240-unit run
        let w = test_world(&[(
            Vec3::new(-1024.0, -1024.0, 60.0),
            Vec3::new(1024.0, 1024.0, 100.0),
        )]);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        ps.stance = Stance::Crouch;
        let crouched = PmInput {
            forward: 1.0,
            crouch: true,
            ..Default::default()
        };
        tick(&mut ps, &crouched, &w, 250);
        let h = ps.velocity.truncate().length();
        assert!(
            (h - SPEED_RUN * SCALE_CROUCH).abs() < 5.0,
            "crouch speed {h}"
        );
        tick(&mut ps, &PmInput::default(), &w, 10);
        assert_eq!(ps.stance, Stance::Crouch);
    }

    #[test]
    fn prone_is_slowest() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let prone = PmInput {
            forward: 1.0,
            prone: true,
            ..Default::default()
        };
        tick(&mut ps, &prone, &w, 400);
        let h = ps.velocity.truncate().length();
        assert!((h - SPEED_RUN * SCALE_PRONE).abs() < 3.0, "prone speed {h}");
    }

    #[test]
    fn lean_ramps_up_clamps_and_returns() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50); // settle
        let lean_r = PmInput {
            lean_right: true,
            ..Default::default()
        };
        // 280 ms to full lean; run 500 ms
        tick(&mut ps, &lean_r, &w, 63);
        assert!((ps.lean - LEAN_MAX).abs() < 0.5, "lean {}", ps.lean);
        // full return within 350 ms; run 500 ms
        tick(&mut ps, &PmInput::default(), &w, 63);
        assert!(ps.lean.abs() < 0.5, "lean {}", ps.lean);
    }

    #[test]
    fn lean_left_is_negative_and_wall_limits_lean() {
        // Wall on +Y (the left side at yaw 0), past y=12 so the lean box does
        // not start inside it, which would pin fraction at 0.
        let w = test_world(&[(Vec3::new(-200.0, 20.0, 0.0), Vec3::new(200.0, 70.0, 200.0))]);
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        let lean_l = PmInput {
            lean_left: true,
            ..Default::default()
        };
        tick(&mut ps, &lean_l, &w, 125);
        assert!(ps.lean < 0.0);
        assert!(
            ps.lean > -LEAN_MAX + 1.0,
            "wall should limit lean, got {}",
            ps.lean
        );
    }

    #[test]
    fn prone_blocks_lean() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        let input = PmInput {
            prone: true,
            lean_right: true,
            ..Default::default()
        };
        tick(&mut ps, &input, &w, 125);
        assert_eq!(ps.lean, 0.0);
    }

    #[test]
    fn walks_on_mp_pavlov_terrain() {
        let Some(data) = crate::testing::real_bsp() else {
            return;
        };
        let bsp = crate::bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let (origin, yaw) = crate::bsp::find_spawn(&bsp.entities).unwrap();
        let mut ps = PlayerState::spawn(Vec3::from(origin) + Vec3::Z * 2.0, yaw);
        // must land on terrain triangles near the spawn, not fall to bedrock
        tick(&mut ps, &PmInput::default(), &world, 250);
        assert!(ps.on_ground, "player should land");
        assert!(
            (ps.origin.z - origin[2]).abs() < 40.0,
            "landed at z={}, spawn z={}",
            ps.origin.z,
            origin[2]
        );
        let start = ps.origin;
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &world,
            500,
        );
        assert!(
            ps.origin.truncate().distance(start.truncate()) > 100.0,
            "should cover ground"
        );
        assert!(
            ps.origin.z > -100.0,
            "must not fall through the map, z={}",
            ps.origin.z
        );
    }

    #[test]
    fn view_reflects_stance_and_lean() {
        let w = flat();
        let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0);
        tick(&mut ps, &PmInput::default(), &w, 50);
        let v = ps.view();
        assert!((v.eye.z - VIEW_STAND).abs() < 1.0);
        assert_eq!(v.roll, 0.0);
        tick(
            &mut ps,
            &PmInput {
                lean_right: true,
                ..Default::default()
            },
            &w,
            125,
        );
        let v = ps.view();
        assert!(v.roll > 0.1, "leaning right should roll the view");
        assert!(v.eye.y < -1.0, "yaw 0 lean right offsets eye toward -Y");
    }

    // --- water ---

    #[test]
    fn water_level_tracks_depth() {
        let w = pool_world();
        // deep floor: only the eye stays above the surface? no - fully under
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -73.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 3, "on the pool bottom at {}", ps.origin);

        // chest-deep shelf: feet+1 and waist wet, eyes dry
        let mut ps = PlayerState::spawn(Vec3::new(150.0, 0.0, 0.2), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 2, "on the shelf at {}", ps.origin);

        // ankle-deep shelf
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 200.0, 30.2), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 1, "ankle-deep at {}", ps.origin);

        // dry ground west of the pool
        let mut ps = PlayerState::spawn(Vec3::new(-400.0, 0.0, 1.0), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 0);
    }

    #[test]
    fn swimming_caps_at_the_swim_speed() {
        let w = pool_world();
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -20.0), 0.0);
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            100,
        );
        let h = ps.velocity.truncate().length();
        assert!(
            (h - SPEED_RUN * SCALE_SWIM).abs() < 6.0,
            "swim speed {h}, expected ~{}",
            SPEED_RUN * SCALE_SWIM
        );
    }

    #[test]
    fn idle_player_sinks_without_freefall() {
        let w = pool_world();
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -20.0), 0.0);
        let start = ps.origin.z;
        let input = PmInput::default();
        // the sink wish is an acceleration target, not a velocity cap: speed
        // builds over ~half a second, so watch the whole descent
        let mut worst_fall = 0.0f32;
        for _ in 0..150 {
            pmove(&mut ps, &input, &w, 1.0 / 125.0);
            worst_fall = worst_fall.max(-ps.velocity.z);
        }
        assert!(
            ps.origin.z < start - 40.0,
            "should reach the bottom, at {}",
            ps.origin
        );
        assert!(ps.on_ground, "settled on the pool floor at {}", ps.origin);
        assert!(worst_fall < 130.0, "sink must not freefall: {worst_fall}");
    }

    #[test]
    fn jump_key_swims_up() {
        let w = pool_world();
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -30.0), 0.0);
        tick(
            &mut ps,
            &PmInput {
                jump: true,
                ..Default::default()
            },
            &w,
            150,
        );
        // hovers chest-deep: above that line the waist sample dries, swim
        // gives way to air, and he sinks back into it
        assert!(
            (-6.0..15.0).contains(&ps.origin.z),
            "should bob at the chest line, at {}",
            ps.origin
        );
    }

    #[test]
    fn wading_is_slower_than_dry_running() {
        let w = pool_world();
        let mut ps = PlayerState::spawn(Vec3::new(-150.0, 200.0, 30.2), 0.0);
        tick(&mut ps, &PmInput::default(), &w, 5);
        assert_eq!(ps.water_level, 1);
        // sample while still on the shelf: the run must not reach its edge
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            100,
        );
        assert!(ps.origin.x < 100.0, "still on the shelf at {}", ps.origin);
        let h = ps.velocity.truncate().length();
        let expect = SPEED_RUN * (1.0 - (1.0 - SCALE_SWIM) * (1.0 / 3.0));
        assert!(
            (h - expect).abs() < 6.0,
            "wade speed {h}, expected ~{expect}"
        );
        assert_eq!(ps.water_level, 1);
    }

    #[test]
    fn looking_up_while_submerged_walk_turns_into_swim() {
        let w = pool_world();
        let mut ps = PlayerState::spawn(Vec3::new(0.0, -200.0, -73.0), 0.0);
        ps.pitch = 20.0f32.to_radians();
        tick(
            &mut ps,
            &PmInput {
                forward: 1.0,
                ..Default::default()
            },
            &w,
            30,
        );
        assert!(
            ps.velocity.z > 5.0 || ps.origin.z > -60.0,
            "should swim up off the bottom: vz {} z {}",
            ps.velocity.z,
            ps.origin.z
        );
    }

    #[test]
    fn water_jump_leaps_out_of_the_pool() {
        let w = pool_world();
        // far enough from the lip that the bbox clears it only after rising
        let mut ps = PlayerState::spawn(Vec3::new(172.0, 0.0, 0.2), 0.0);
        let input = PmInput {
            forward: 1.0,
            jump: true,
            ..Default::default()
        };
        pmove(&mut ps, &input, &w, 1.0 / 125.0);
        assert!(
            ps.waterjump_ms > 0.0,
            "waterjump should trigger from the shelf"
        );
        assert!(ps.velocity.z > 300.0, "boost vz {}", ps.velocity.z);
        // hands off for the flight: holding jump would bunny-hop after landing
        tick(&mut ps, &PmInput::default(), &w, 120);
        assert!(
            ps.origin.x > 255.0 && ps.origin.z < 20.0,
            "should land east of the wall, at {}",
            ps.origin
        );
        assert!(ps.on_ground && ps.waterjump_ms == 0.0, "{:?}", ps.origin);
    }
}
