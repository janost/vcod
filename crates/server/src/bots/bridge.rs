//! The CoD1-body bridge CoD2's and CoD4's bots share: a nav walk's CoD1
//! body carried on a CoD2-family body ([`walk_cmd`], [`walk_body`],
//! [`read_walk_body`]), the follower's CoD1 body ([`follower_body`]), the
//! mantle probe's walk ([`mantle_walk`]). Each game keeps what it
//! moves by in `bots/cod2.rs` and `bots/cod4.rs`: its pmove, its cmd, its
//! traces and its `delta_angles`.

use std::sync::Arc;

use glam::Vec3;
use vcod_cod2::pmove as pm2;
use vcod_cod4::pmove as pm4;
use vcod_common::clipmap::{CONTENTS_MANTLE, SURF_MANTLE_ON, SURF_MANTLE_OVER};
use vcod_common::net::protocol::ENTITYNUM_NONE;
use vcod_common::pmove::{MoveRules, PlayerState as Cod1Ps, Stance};
use vcod_common::pmshared::PmState;

use crate::nav::WalkCarry;

/// How far ahead of a node a wall flagged for the mantle is looked for
/// before the climb is walked: past half the graph's widest pitch.
pub(crate) const MANTLE_LOOK: f32 = 64.0;
/// The walk to the wall's ticks, and the whole probe's: the slowest
/// mantle (`mp_mantle_up_57` and `_over_high`) runs under 1.5 s.
pub(crate) const MANTLE_WALK_TICKS: usize = 20;
pub(crate) const MANTLE_TICKS: usize = 70;

/// `mantleState.flags`' hint, 0x8 in both games: a mantle is available
/// this frame.
const MANTLE_HINT: u32 = 0x8;

/// What the bridge reads off a CoD2-family body.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seen {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub view_angles: [f32; 3],
    pub ground_entity_num: i32,
    pub pm_flags: u32,
    pub mantle_flags: u32,
    pub ladder_vec: Vec3,
    pub command_time: i32,
    pub jump_time: i32,
    pub pm_time: i32,
    pub jump_origin_z: f32,
    /// `PM_GetEffectiveStance`.
    pub stance: Stance,
}

impl Seen {
    fn on_ground(&self) -> bool {
        self.ground_entity_num != ENTITYNUM_NONE as i32
    }
}

/// A CoD2 1.3 or CoD4 1.7 playerstate as the bridge reads ([`Seen`]) and
/// writes it, beside what [`PmState`] names. The two name these fields
/// alike; the bits and view heights are each game's.
pub(crate) trait BridgePs: PmState + Clone + Send + Sync + 'static {
    const PMF_MANTLE: u32;
    const VIEW_CROUCH: i32;
    const VIEW_PRONE: i32;

    /// A living player standing at `origin`, its clock at `time`.
    fn spawn(origin: Vec3, yaw: f32, time: i32) -> Self;
    fn seen(&self) -> Seen;
    fn view_angles(&mut self) -> &mut [f32; 3];
    fn old_velocity(&mut self) -> &mut [f32; 2];
    fn speed(&mut self) -> &mut i32;
    fn gravity(&mut self) -> &mut i32;
    fn ladder_vec(&mut self) -> &mut Vec3;
    fn jump_time(&mut self) -> &mut i32;
    /// `viewHeightTarget` and `viewHeightCurrent`.
    fn view_height(&mut self) -> (&mut i32, &mut f32);
}

macro_rules! bridge_ps {
    ($pm:ident) => {
        impl BridgePs for $pm::PlayerState {
            const PMF_MANTLE: u32 = $pm::pmf::MANTLE;
            const VIEW_CROUCH: i32 = $pm::VIEW_CROUCH;
            const VIEW_PRONE: i32 = $pm::VIEW_PRONE;

            fn spawn(origin: Vec3, yaw: f32, time: i32) -> Self {
                $pm::PlayerState::spawn(origin, yaw, time)
            }
            fn seen(&self) -> Seen {
                Seen {
                    origin: self.origin,
                    velocity: self.velocity,
                    view_angles: self.view_angles,
                    ground_entity_num: self.ground_entity_num,
                    pm_flags: self.pm_flags,
                    mantle_flags: self.mantle.flags,
                    ladder_vec: self.ladder_vec,
                    command_time: self.command_time,
                    jump_time: self.jump_time,
                    pm_time: self.pm_time,
                    jump_origin_z: self.jump_origin_z,
                    stance: self.effective_stance(),
                }
            }
            fn view_angles(&mut self) -> &mut [f32; 3] {
                &mut self.view_angles
            }
            fn old_velocity(&mut self) -> &mut [f32; 2] {
                &mut self.old_velocity
            }
            fn speed(&mut self) -> &mut i32 {
                &mut self.speed
            }
            fn gravity(&mut self) -> &mut i32 {
                &mut self.gravity
            }
            fn ladder_vec(&mut self) -> &mut Vec3 {
                &mut self.ladder_vec
            }
            fn jump_time(&mut self) -> &mut i32 {
                &mut self.jump_time
            }
            fn view_height(&mut self) -> (&mut i32, &mut f32) {
                (&mut self.view_height_target, &mut self.view_height_current)
            }
        }
    };
}

bridge_ps!(pm2);
bridge_ps!(pm4);

/// A trace as the bridge reads it, whichever world the game traces.
pub(crate) struct Hit {
    pub startsolid: bool,
    pub fraction: f32,
    pub normal: Vec3,
    pub surface_flags: u32,
}

impl From<vcod_common::collision::Trace> for Hit {
    fn from(t: vcod_common::collision::Trace) -> Hit {
        Hit {
            startsolid: t.startsolid,
            fraction: t.fraction,
            normal: t.normal,
            surface_flags: t.surface_flags,
        }
    }
}

impl From<vcod_common::clipmap::ClipTrace> for Hit {
    fn from(t: vcod_common::clipmap::ClipTrace) -> Hit {
        Hit {
            startsolid: t.startsolid,
            fraction: t.fraction,
            normal: t.normal,
            surface_flags: t.surface_flags,
        }
    }
}

/// The body a walk carries between cmds, the cmd before it (`C`, the
/// game's), and where it left the walk's CoD1 body.
#[derive(Clone)]
struct Carried<P, C> {
    ps: P,
    old: C,
    origin: Vec3,
    velocity: Vec3,
}

/// One cmd of a nav walk ([`crate::nav::WalkStep::step`]) on a game body:
/// the one `carry` holds while the walk's CoD1 body `ps` is where it left
/// it, else a fresh one off `ps` ([`walk_body`], its clock at `from_ms`).
/// `step` moves it by the game's pmove (its second argument the cmd
/// before) and says whether the jump key was down; `below` traces the
/// body's box from its first point to its second ([`read_walk_body`]).
pub(crate) fn walk_cmd<P: BridgePs, C: Clone + Default + Send + Sync + 'static>(
    ps: &mut Cod1Ps,
    carry: &mut WalkCarry,
    from_ms: i32,
    step: impl FnOnce(&mut P, &mut C) -> bool,
    below: impl FnOnce(&P, Vec3, Vec3) -> Hit,
) {
    // The walk's own CoD1 body is the truth when it moved it since.
    let mut body = carry
        .take()
        .and_then(|c| c.downcast::<Carried<P, C>>().ok())
        .map(Arc::unwrap_or_clone)
        .filter(|b| b.origin == ps.origin && b.velocity == ps.velocity)
        .unwrap_or_else(|| Carried {
            ps: walk_body(ps, from_ms),
            old: C::default(),
            origin: ps.origin,
            velocity: ps.velocity,
        });
    let jump_held = step(&mut body.ps, &mut body.old);
    read_walk_body(&body.ps, ps, jump_held, |a, b| below(&body.ps, a, b));
    body.origin = ps.origin;
    body.velocity = ps.velocity;
    *carry = Some(Arc::new(body));
}

/// A walk's first game body off its CoD1 one, its clock at `time`: moving
/// as the CoD1 body moves (`old_velocity` its pace, as a body that has run
/// so for a while), its last jump `since_jump_ms` back.
pub(crate) fn walk_body<P: BridgePs>(ps: &Cod1Ps, time: i32) -> P {
    let mut b = P::spawn(ps.origin, ps.yaw.to_degrees(), time);
    b.view_angles()[0] = ps.pitch.to_degrees();
    *b.velocity() = ps.velocity;
    *b.old_velocity() = [ps.velocity.x, ps.velocity.y];
    *b.speed() = ps.speed as i32;
    *b.gravity() = ps.gravity as i32;
    *b.ground_entity_num() = if ps.on_ground {
        ps.ground_entity as i32
    } else {
        ENTITYNUM_NONE as i32
    };
    if ps.on_ladder {
        *b.pm_flags() |= P::PMF_LADDER;
        *b.ladder_vec() = ps.ladder_normal;
    }
    let view = match ps.stance {
        Stance::Stand => None,
        Stance::Crouch => Some((P::PMF_DUCKED, P::VIEW_CROUCH)),
        Stance::Prone => Some((P::PMF_PRONE, P::VIEW_PRONE)),
    };
    if let Some((flag, height)) = view {
        *b.pm_flags() |= flag;
        let (target, current) = b.view_height();
        *target = height;
        *current = height as f32;
    }
    // CoD2's `Jump_Check` and 1.7's `PM_CheckJump` refuse a jump inside
    // 500 ms of `jumpTime`.
    *b.jump_time() = time - ps.since_jump_ms.min(10_000.0) as i32;
    *b.jump_origin_z() = ps.jump_origin_z;
    if ps.jump_penalty {
        *b.pm_flags() |= P::PMF_JUMPING;
        *b.pm_time() = ps.knockback_ms as i32;
    }
    b
}

/// What the walk reads off the game body `b` into its CoD1 body `ps`; a
/// body off the ground over ground too steep to stand on reads sliding,
/// off `below`'s trace a quarter unit down.
pub(crate) fn read_walk_body<P: BridgePs>(
    b: &P,
    ps: &mut Cod1Ps,
    jump_held: bool,
    below: impl FnOnce(Vec3, Vec3) -> Hit,
) {
    let b = b.seen();
    ps.origin = b.origin;
    ps.velocity = b.velocity;
    ps.yaw = b.view_angles[1].to_radians();
    ps.pitch = b.view_angles[0].to_radians();
    ps.on_ground = b.on_ground();
    if ps.on_ground {
        ps.ground_entity = b.ground_entity_num as u32;
    }
    ps.on_ladder = b.pm_flags & P::PMF_LADDER != 0;
    ps.ladder_normal = b.ladder_vec;
    ps.stance = b.stance;
    ps.ducked = b.pm_flags & (P::PMF_DUCKED | P::PMF_PRONE) != 0;
    ps.since_jump_ms = (b.command_time - b.jump_time).max(0) as f32;
    ps.jump_penalty = b.pm_flags & P::PMF_JUMPING != 0;
    ps.knockback_ms = b.pm_time as f32;
    ps.jump_origin_z = b.jump_origin_z;
    ps.jump_latched = jump_held;
    let plane = if ps.on_ground {
        Some(Vec3::Z)
    } else {
        let t = below(b.origin, b.origin - Vec3::Z * 0.25);
        (!t.startsolid && t.fraction < 1.0).then_some(t.normal)
    };
    ps.set_ground_plane(plane);
}

/// The CoD1 body the follower's foot and jump tests read off the game body
/// `b` at level time `now`: where it stands and how it moves, its last
/// jump's age off `jump_time`, and the jump penalty `rules` jump under:
/// the game's `JUMPING` with its `pm_time` is 1.5's 0x2000
/// (cod2-movement.md), as [`walk_body`] maps it.
pub(crate) fn follower_body<P: BridgePs>(b: &P, now: i32, rules: MoveRules) -> Cod1Ps {
    let s = b.seen();
    let mut f = Cod1Ps::spawn(s.origin, s.view_angles[1]);
    f.rules = rules;
    f.velocity = s.velocity;
    f.on_ground = s.on_ground();
    f.on_ladder = s.pm_flags & P::PMF_LADDER != 0;
    f.ladder_normal = s.ladder_vec;
    f.since_jump_ms = if s.jump_time > 0 {
        now.saturating_sub(s.jump_time).max(0) as f32
    } else {
        f32::MAX
    };
    f.jump_penalty = s.pm_flags & P::PMF_JUMPING != 0;
    f.knockback_ms = s.pm_time as f32;
    f.jump_origin_z = s.jump_origin_z;
    f
}

/// Where a game body standing at `from` and run at the wall along `yaw`
/// stands once it mantles (cod2-movement.md 9, cod4-pmove.md 17): only off
/// a wall `wall` meets flagged `MANTLE_ON` or `MANTLE_OVER` within
/// [`MANTLE_LOOK`], traced with a box `radius` across; the body walks at
/// it, presses the jump key on the first step pmove offers the climb
/// (`Mantle_CheckLedge`'s hint), as a bot does, and the climb's end on the
/// ground is the answer. `step(ps, server_time, jump, forward)` runs one
/// cmd of the game's pmove, the yaw its own.
pub(crate) fn mantle_walk<P: BridgePs>(
    from: Vec3,
    yaw: f32,
    radius: f32,
    wall: impl FnOnce(Vec3, Vec3, Vec3, Vec3, u32) -> Hit,
    mut step: impl FnMut(&mut P, i32, bool, bool),
) -> Option<Vec3> {
    let (s, c) = yaw.to_radians().sin_cos();
    let dir = Vec3::new(c, s, 0.0);
    let r = radius.max(1.0);
    let t = wall(
        from,
        from + dir * MANTLE_LOOK,
        Vec3::new(-r, -r, 0.0),
        Vec3::new(r, r, 70.0),
        CONTENTS_MANTLE,
    );
    if t.startsolid
        || t.fraction >= 1.0
        || t.surface_flags & (SURF_MANTLE_ON | SURF_MANTLE_OVER) == 0
    {
        return None;
    }
    let mut ps = P::spawn(from + Vec3::Z * 0.125, yaw, 0);
    let mut mantled = false;
    for tick in 0..MANTLE_TICKS {
        if !mantled && tick >= MANTLE_WALK_TICKS {
            return None;
        }
        let s = ps.seen();
        let jump = !mantled && s.mantle_flags & MANTLE_HINT != 0;
        step(&mut ps, s.command_time + 50, jump, !mantled);
        let s = ps.seen();
        if s.pm_flags & P::PMF_MANTLE != 0 {
            mantled = true;
        } else if mantled && s.on_ground() {
            return Some(s.origin);
        } else if !mantled && jump {
            // The jump key jumped instead.
            return None;
        }
    }
    None
}
