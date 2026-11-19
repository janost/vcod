//! Probes of the world round a step: hazards, drops, creases, walls,
//! lips and the unpin and push-off yaws.

use super::*;

/// Whether a standing body with its feet at `p` touches one of `hazards`.
pub(crate) fn hazard(hazards: &[BrushHull], p: Vec3) -> bool {
    use vcod_common::pmove::{HALF_WIDTH, Stance};
    let half = Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height() * 0.5);
    !hazards.is_empty() && box_contacts_hulls(p + Vec3::Z * half.z, half, Vec3::ZERO, hazards)
}

/// A wall whose normal meets a heading under this cosine (against it, more
/// than 45 degrees off square) is slid along, not stopped at.
const GRAZE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// How far apart along a heading [`drop_ahead`] feels for the floor.
const DROP_SAMPLE: f32 = 16.0;

/// How deep the first drop deeper than a jump (one a body cannot come
/// back up) is that a body standing at `p`, walking `look` units along the
/// flat unit vector `dir`, meets before a wall stops it; infinite past
/// the default `bg_fallDamageMinHeight`, where a fall starts to hurt.
/// Floors are felt every [`DROP_SAMPLE`], each against the one before, so
/// a stair down is no drop unless it runs more than a jump down within
/// the look. Each is felt with the standing capsule: one
/// standing on a lip it overhangs is held by it.
/// A floor too steep to stand on whose spot drains into a crease
/// ([`Creases`]) reads as a drop that hurts: a body sent down it never
/// comes out (mp_hurtgen's crease, bot-navigation.md section 3).
pub(crate) fn drop_ahead(
    world: &CollisionWorld,
    p: Vec3,
    dir: Vec3,
    look: f32,
    creases: &mut impl CreaseCache,
) -> Option<f32> {
    use vcod_common::pmove::{HALF_WIDTH, JUMP_HEIGHT, MIN_WALK_NORMAL, STEPSIZE, Stance};
    let mins = Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
    let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height());
    let up = p + Vec3::Z * STEPSIZE;
    let sweep = |dir: Vec3| world.box_trace(up, up + dir * look, mins, maxs - Vec3::Z * STEPSIZE);
    let mut t = sweep(dir);
    let mut dir = dir;
    // Against a wall the heading only grazes, the body slides along it:
    // feel along the slide instead (mp_ship's wall at (6111, -304, 97),
    // whose foot drops 41 to the deck).
    let n = t.normal.truncate().extend(0.0);
    let slide = dir - n * dir.dot(n);
    if t.fraction * look < DROP_SAMPLE && dir.dot(n) > -GRAZE && slide.length() > 0.1 {
        dir = slide.normalize();
        t = sweep(dir);
    }
    if t.startsolid || t.allsolid {
        return None;
    }
    let reach = t.fraction * look;
    let hurts = vcod_common::pmove::FallHeights::default().min;
    let mut floor = p.z;
    // A sample every DROP_SAMPLE, and half way between for a hole a body
    // falls through where the samples either side stand on its rims (a
    // point trace first, the capsule only where it finds no floor), and
    // one where the sweep stops: a body run at a wall ends up there, over
    // whatever lies before it (mp_ship's crow's nest plate, 11 units from
    // the top's face across the hatch, bot-navigation.md section 3).
    let half = DROP_SAMPLE / 2.0;
    let last = (reach > WALL_NEAR && reach % half > 1.0).then_some(reach);
    let mut d = half;
    while let Some(d_at) = (d <= reach).then_some(d).or(last.filter(|&r| d - half < r)) {
        let at = (p + dir * d_at).truncate().extend(floor + STEPSIZE);
        let between = d_at == d && d % DROP_SAMPLE != 0.0;
        d += half;
        if between
            && world
                .point_trace(
                    at,
                    at - Vec3::Z * (STEPSIZE + JUMP_HEIGHT),
                    MASK_PLAYERSOLID,
                    false,
                )
                .fraction
                < 1.0
        {
            continue;
        }
        let t = world.box_trace(at, at - Vec3::Z * (STEPSIZE + JUMP_HEIGHT), mins, maxs);
        if t.startsolid {
            return None;
        }
        if t.fraction >= 1.0 {
            let deep = world.box_trace(at, at - Vec3::Z * (STEPSIZE + hurts), mins, maxs);
            let depth = deep.fraction * (STEPSIZE + hurts) - STEPSIZE;
            return Some(if deep.fraction >= 1.0 {
                f32::INFINITY
            } else {
                depth
            });
        }
        if t.normal.z > 0.0 && t.normal.z < MIN_WALK_NORMAL && creases.drains(world, t.endpos) {
            return Some(f32::INFINITY);
        }
        floor = t.endpos.z;
        // A slope or a stair as steep runs as far down within the look:
        // mp_ship's hull slope at (3752, 340-414) runs 49 down in 75 to a
        // wedge off the graph.
        if floor < p.z - JUMP_HEIGHT {
            return Some(p.z - floor);
        }
    }
    None
}

/// Which spots on ground too steep to stand on drain into a crease
/// ([`held_off_ground`]), by 8-unit cell. A level's world is fixed, and the
/// test runs a body for up to 50 ticks, too dear for every bot every tick.
#[derive(Default)]
pub struct Creases(std::collections::HashMap<[i32; 3], bool>);

/// The crease verdicts [`drop_ahead`] reads and fills: a level's
/// [`Creases`], or a worker's [`CreaseRead`] over them.
pub(crate) trait CreaseCache {
    /// Whether the spot `p` drains into a crease, felt from `p` the first
    /// time its cell is read.
    fn drains(&mut self, world: &CollisionWorld, p: Vec3) -> bool;
}

fn crease_cell(p: Vec3) -> [i32; 3] {
    (p / 8.0).floor().as_ivec3().to_array()
}

impl CreaseCache for Creases {
    fn drains(&mut self, world: &CollisionWorld, p: Vec3) -> bool {
        *self
            .0
            .entry(crease_cell(p))
            .or_insert_with(|| drains(world, p))
    }
}

impl Creases {
    /// Keeps the cells a [`CreaseRead`] felt, when each reads as here
    /// wherever both hold it: then a probe that read them on a worker read
    /// what it would have read here. `false` keeps none.
    pub(crate) fn merge(&mut self, fresh: &[([i32; 3], bool)]) -> bool {
        if fresh
            .iter()
            .any(|(c, v)| self.0.get(c).is_some_and(|k| k != v))
        {
            return false;
        }
        for &(c, v) in fresh {
            self.0.entry(c).or_insert(v);
        }
        true
    }
}

/// [`Creases`] read from a worker thread: a cell they lack is felt into
/// `fresh`, in the order felt, for [`Creases::merge`].
pub(crate) struct CreaseRead<'a> {
    pub(crate) base: &'a Creases,
    pub(crate) fresh: Vec<([i32; 3], bool)>,
}

impl CreaseCache for CreaseRead<'_> {
    fn drains(&mut self, world: &CollisionWorld, p: Vec3) -> bool {
        let cell = crease_cell(p);
        if let Some(&v) = self.base.0.get(&cell) {
            return v;
        }
        if let Some(&(_, v)) = self.fresh.iter().find(|c| c.0 == cell) {
            return v;
        }
        let v = drains(world, p);
        self.fresh.push((cell, v));
        v
    }
}

/// How near a wall [`wall_ahead`] calls a heading blocked.
const WALL_NEAR: f32 = 4.0;

/// Whether a body standing at `p` meets a wall within [`WALL_NEAR`] along
/// the flat unit vector `dir`, square enough that it pins rather than
/// slides ([`GRAZE`]). [`drop_ahead`] reads such a heading as no drop.
pub(crate) fn wall_ahead(world: &CollisionWorld, p: Vec3, dir: Vec3) -> bool {
    use vcod_common::pmove::{HALF_WIDTH, STEPSIZE, Stance};
    let mins = Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
    let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height() - STEPSIZE);
    let up = p + Vec3::Z * STEPSIZE;
    let t = world.box_trace(up, up + dir * WALL_NEAR, mins, maxs);
    t.startsolid || (t.fraction < 1.0 && dir.dot(t.normal.truncate().extend(0.0)) <= -GRAZE)
}

/// Whether the body `ps` loses the ground in the coming tick running at
/// `target`: the tick's move, run on a copy. A leap jumps there
/// (`Gait::Leap`, and the bots on its edges). The move, not the pace: a
/// body speeding up from rest covers half again what its velocity says,
/// and a bot that runs faster than the walk's body (`weapons`' move
/// scale) walked off mp_depot's step to the documents' crate a tick
/// before the lip read (bot-navigation.md, "Jumps").
pub fn lip_ahead(
    world: &CollisionWorld,
    ps: &PlayerState,
    target: glam::Vec2,
    weapons: &[Option<vcod_common::weapon::WeaponDef>],
) -> bool {
    let mut ps = *ps;
    let to = target - ps.origin.truncate();
    let cmd = UserCmd {
        server_time: FRAME_MS,
        forward: paced_key_for(127, &ps, weapons),
        angles: [0, (to.y.atan2(to.x).to_degrees() * ANGLE2SHORT) as i32, 0],
        weapon: ps.weapon,
        ..NULL_USERCMD
    };
    let dt = FRAME_MS as f32 / 1000.0;
    if let Some(own) = own_step() {
        own.step(&mut ps, &mut [0; 3], &mut None, 0, &cmd, world, weapons);
        return !ps.on_ground && !ps.on_ladder;
    }
    let mw = MoveWorld::bare(world);
    player_step(
        &mut ps,
        &mut [0; 3],
        &mut EventRing::default(),
        &cmd,
        dt,
        &mw,
        weapons,
    );
    !ps.on_ground && !ps.on_ladder
}

/// Ticks an [`unpin_yaw`] heading is run for, and held by the bot.
pub const UNPIN_TICKS: usize = 4;

/// A body slower than this, units/s, on the ground short of its waypoint
/// is checked for a pin ([`unpin_yaw`]).
pub const UNPIN_SPEED: f32 = 20.0;

/// The rise per unit across a waypoint off the body's floor may take and
/// still be checked for a pin: a stair's 32 in 48 and a little more.
const UNPIN_GRADE: f32 = 0.75;

/// A heading that frees the body `ps`, standing still on the ground and
/// pinned short of `to` on its floor: `None` when the run straight at `to`
/// closes 2 units in [`UNPIN_TICKS`]; else, of the headings 10 to 90
/// degrees either side of it, every 10, and the compass axes within 90 of
/// it, the one whose run, held for those ticks,
/// closes on `to` most and by 8 or more, staying on the ground within a
/// step of the floors between the body and `to` and off `hazards`. A body
/// at rest in a crease where two faces meet does not move at all toward a
/// point a few degrees past one of them, and the same run with a running
/// start slides through (mp_ship's corner at x 5375.1, bot-navigation.md
/// section 3). A waypoint down or up a stair counts as the body's floor
/// when the rise is a walkable one: mp_uo_stanjel's stair turn at
/// (711, 235, 246) pins a body on the newel's corner, slower every tick.
pub fn unpin_yaw(
    world: &CollisionWorld,
    hazards: &[BrushHull],
    ps: &PlayerState,
    to: Vec3,
    weapons: &[Option<vcod_common::weapon::WeaponDef>],
) -> Option<f32> {
    let flat = (to - ps.origin).truncate();
    let step = vcod_common::pmove::STEPSIZE;
    if !ps.on_ground
        || ps.on_ladder
        || ps.velocity.truncate().length() >= UNPIN_SPEED
        || flat.length() < ARRIVE
        || (to.z - ps.origin.z).abs() > step + flat.length() * UNPIN_GRADE
    {
        return None;
    }
    let (lo, hi) = (to.z.min(ps.origin.z) - step, to.z.max(ps.origin.z) + step);
    let mw = MoveWorld::bare(world);
    let dt = FRAME_MS as f32 / 1000.0;
    // How much nearer `to` a run held at `yaw` ends, when it keeps its
    // footing on the floor all the way.
    let key = paced_key_for(127, ps, weapons);
    let own = own_step();
    let closes = |yaw: f32| {
        let mut ps = *ps;
        let mut delta = [0; 3];
        let mut ring = EventRing::default();
        let mut carry = None;
        for i in 0..UNPIN_TICKS {
            let cmd = UserCmd {
                server_time: FRAME_MS * (i as i32 + 1),
                forward: key,
                angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
                weapon: ps.weapon,
                ..NULL_USERCMD
            };
            match &own {
                Some(own) => {
                    let from = FRAME_MS * i as i32;
                    own.step(&mut ps, &mut delta, &mut carry, from, &cmd, world, weapons);
                }
                None => {
                    player_step(&mut ps, &mut delta, &mut ring, &cmd, dt, &mw, weapons);
                }
            }
            if !ps.on_ground || !(lo..=hi).contains(&ps.origin.z) || hazard(hazards, ps.origin) {
                return None;
            }
        }
        Some(flat.length() - (to - ps.origin).truncate().length())
    };
    let straight = flat.y.atan2(flat.x).to_degrees();
    if closes(straight).is_none_or(|d| d >= 2.0) {
        return None;
    }
    // Most faces are axial, and the run exactly along one slides where
    // one a hair into it pins: the axes go in as they are.
    let axes = [0.0f32, 90.0, 180.0, 270.0]
        .into_iter()
        .filter(|a| ((a - straight + 540.0).rem_euclid(360.0) - 180.0).abs() < 90.0);
    (1..=9)
        .flat_map(|i| [straight + i as f32 * 10.0, straight - i as f32 * 10.0])
        .chain(axes)
        .filter_map(|yaw| Some((closes(yaw)?, yaw)))
        .filter(|&(d, _)| d >= 8.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, yaw)| yaw)
}

/// Ticks a [`push_off`] flight is followed before it counts as failed.
const PUSH_OFF_TICKS: usize = 40;

/// The yaw of a push-off from the rungs `ps` hangs on toward `to`, a
/// floor behind the climber's back: `Some` when `to` lies 8 or more
/// units out from the face, between a jump under and a step over the
/// body, and the push-off run on a copy lands on that floor within 64
/// units of `to`. The flight is the jump facing the face, which
/// `PM_CheckJump` reflects straight out, then the forward key, held facing
/// the same way, until the body lands: facing out, `bg_ladder_yawcap`
/// would turn the view 80 degrees on the jump's frame and the jump with
/// it (cod11-mantle.md, "The ladder yaw cap"). A ladder
/// move climbs and strafes along the face but never steps off it, so a
/// floor behind the climber is left only by the jump, and the jump alone
/// carries a body about 60 units out (bot-navigation.md section 3, the
/// ladder at (2150, -214)).
pub fn push_off(
    world: &CollisionWorld,
    ps: &PlayerState,
    to: Vec3,
    weapons: &[Option<vcod_common::weapon::WeaponDef>],
) -> Option<f32> {
    let n = ps.ladder_normal.truncate();
    if !ps.on_ladder || n.length_squared() < 0.01 {
        return None;
    }
    let n = n.normalize();
    let dz = to.z - ps.origin.z;
    let out = (to.truncate() - ps.origin.truncate()).dot(n);
    if out < 8.0 || !(-vcod_common::pmove::JUMP_HEIGHT..=vcod_common::pmove::STEPSIZE).contains(&dz)
    {
        return None;
    }
    let yaw = (-n.y).atan2(-n.x).to_degrees();
    let mut ps = *ps;
    let mut delta = [0; 3];
    let mut ring = EventRing::default();
    let mw = MoveWorld::bare(world);
    let dt = FRAME_MS as f32 / 1000.0;
    let own = own_step();
    let mut carry = None;
    for i in 0..PUSH_OFF_TICKS {
        let cmd = UserCmd {
            server_time: FRAME_MS * (i as i32 + 1),
            forward: if i == 0 { 0 } else { 127 },
            up: if i == 0 { 127 } else { 0 },
            angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
            weapon: ps.weapon,
            ..NULL_USERCMD
        };
        match &own {
            Some(own) => {
                let from = FRAME_MS * i as i32;
                own.step(&mut ps, &mut delta, &mut carry, from, &cmd, world, weapons);
            }
            None => {
                player_step(&mut ps, &mut delta, &mut ring, &cmd, dt, &mw, weapons);
            }
        }
        if i == 0 && ps.on_ladder {
            return None;
        }
        if ps.on_ground || ps.on_ladder {
            break;
        }
    }
    let landed = ps.on_ground
        && !ps.on_ladder
        && (ps.origin.z - to.z).abs() <= vcod_common::pmove::STEPSIZE
        && ps.origin.truncate().distance(to.truncate()) < 64.0;
    landed.then_some(yaw)
}

/// Whether a jump from where `ps` stands could clear what pins it: room
/// ahead toward `target` at the top of the jump, or under a ceiling lower
/// than the jump's height, as high as the body rises. A wall to the
/// ceiling fails it, which keeps a jump off the walks that end at one.
/// A `crouched` jump looks for room at the crouched body's height.
pub(super) fn jump_clear(
    world: &CollisionWorld,
    ps: &PlayerState,
    target: glam::Vec2,
    crouched: bool,
) -> bool {
    let (mins, mut maxs) = (ps.mins(), ps.maxs());
    if crouched {
        maxs.z = vcod_common::pmove::Stance::Crouch.height();
    }
    let up = world.box_trace(
        ps.origin,
        ps.origin + Vec3::Z * (vcod_common::pmove::JUMP_HEIGHT + JUMP_SLACK),
        mins,
        maxs,
    );
    if up.startsolid {
        return false;
    }
    let top = up.endpos;
    let dir = (target - ps.origin.truncate())
        .normalize_or_zero()
        .extend(0.0);
    let ahead = world.box_trace(top, top + dir * 16.0, mins, maxs);
    !ahead.startsolid && ahead.fraction == 1.0
}

/// The room over [`JUMP_HEIGHT`](vcod_common::pmove::JUMP_HEIGHT) that
/// [`jump_clear`] allows: a jump out of mp_berlin's roof gutter from z
/// 612.26 lands on its ledge at 652.42, 40.2 up (UO rules, measured).
const JUMP_SLACK: f32 = 2.0;

/// Whether a standing body at `from` on the ground walks straight to `to`
/// on one floor: its capsule swept a step up clears, and every
/// [`PULL_PROBE`] units along the line there is ground a body stands on
/// within a step of the line's height ([`Follower::pull`]).
pub(super) fn straight_floor(world: &CollisionWorld, from: Vec3, to: Vec3) -> bool {
    let step = vcod_common::pmove::STEPSIZE;
    let ps = body(from, 0.0);
    let up = Vec3::Z * step;
    let sweep = world.move_trace(from + up, to + up, ps.mins(), ps.maxs(), MASK_PLAYERSOLID);
    if sweep.startsolid || sweep.fraction < 1.0 {
        return false;
    }
    let n = ((to - from).truncate().length() / PULL_PROBE).ceil() as usize;
    (1..n).all(|i| {
        let p = from.lerp(to, i as f32 / n as f32);
        let t = world.point_trace(p + up, p - up, MASK_PLAYERSOLID, false);
        !t.startsolid && t.fraction < 1.0 && t.normal.z >= vcod_common::pmove::MIN_WALK_NORMAL
    })
}

/// Whether `p` lies beneath a sheet of ground. Terrain and patches are
/// surfaces, not solids, so a run that finds a gap at a sheet's edge can
/// walk the map's floor under them (mp_hurtgen's, 48 units below its river
/// bed). From there the ray up either stops on the underside of a triangle
/// whose own face, `(b - a) x (c - a)`, points up, or passes through a
/// one-sided terrain triangle and the same ray back down hits it. Both rays
/// see what a player clips, not the brushes only a shot meets. Nothing else
/// the ray back down meets is ground over the point: a patch (mp_ship's
/// mast ladder stands 540 units under a spar's top facet), or a ceiling
/// just over the head that the ray starts against (a low doorway on
/// mp_pavlov kept 540 nodes behind it off the graph). The rays start a step
/// over the feet, not at the head: mp_carentan's terrain runs 38 units over
/// the base brush west of its boundary wall, and from the head a flood
/// seeded there walked under it into the town.
pub(super) fn under_ground(world: &CollisionWorld, p: Vec3) -> bool {
    let start = p + Vec3::Z * vcod_common::pmove::STEPSIZE;
    let up = world.point_trace(start, start + Vec3::Z * 8192.0, MASK_PLAYERSOLID, false);
    if let Some(Prim::Tri(t)) = up.hit {
        let [a, b, c] = world.tris[t as usize];
        if (b - a).cross(c - a).normalize_or_zero().z > 0.7 {
            return true;
        }
    }
    let down = world.point_trace(up.endpos - Vec3::Z, start, MASK_PLAYERSOLID, false);
    matches!(down.hit, Some(Prim::Tri(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::build::tests::map_world;

    /// West of mp_carentan's boundary wall the map's base brush runs on
    /// under the terrain; a flood seeded there reached the town at this
    /// point, with the terrain sheet under 72 units over the feet.
    #[test]
    fn carentans_floor_under_a_low_terrain_sheet_is_under_ground() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_carentan").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let p = Vec3::new(389.0, 575.0, -46.7);
        assert!(under_ground(w, p));
    }

    /// mp_hurtgen's minefields ring the axis spawn: a bot that wandered off
    /// it died about every 12 s and ended the Retrieval round.
    #[test]
    fn hurtgens_minefields_are_hazards() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let entry = fs
            .resolve_map("mp_hurtgen")
            .expect("a stock map every install ships");
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        assert!(
            hazard(&world.hazards, Vec3::new(6198.0, -302.0, 188.0)),
            "where the bot died"
        );
        assert!(
            !hazard(&world.hazards, Vec3::new(6304.0, 296.0, 96.125)),
            "the spawn beside it"
        );
    }

    /// mp_ship's hull slope north of (3762, 339, 352): no sample a jump
    /// under the one before, but 49 down within 75 units, to a wedge off
    /// the graph at (3752, 414, 303) that a seed 5 unstick heading ran a
    /// bot into.
    #[test]
    fn a_drop_ahead_is_felt_down_a_steep_slope() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let at = Vec3::new(3762.4, 338.9, 352.125);
        let depth = drop_ahead(&world.collision, at, Vec3::Y, 64.0, &mut Creases::default());
        assert!(depth.is_some_and(|d| d > 39.0), "{depth:?}");
    }

    /// mp_ship, seed 5: a bot against the slanted wall at (6111, -304, 97)
    /// headed south slid along it and off the ledge 41 units to the deck.
    /// The sweep due south touches the wall at once; the slide finds it.
    #[test]
    fn a_drop_ahead_is_felt_along_a_wall_it_slides_on() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let at = Vec3::new(6095.754, -303.89618, 97.125);
        let depth = drop_ahead(
            &world.collision,
            at,
            -Vec3::Y,
            64.0,
            &mut Creases::default(),
        );
        assert!(depth.is_some_and(|d| (d - 41.0).abs() < 1.0), "{depth:?}");
        assert_eq!(
            drop_ahead(&world.collision, at, Vec3::Y, 64.0, &mut Creases::default()),
            None
        );
    }

    /// mp_hurtgen, seed 67: an unstick heading north-east from
    /// (411.7, 2786, -57.7) ran a bot down the slope into the crease at
    /// (437, 2824, -97). The slope falls under a jump within the look, so
    /// it reads as a drop only through the crease it drains into.
    #[test]
    fn a_slope_draining_into_a_crease_reads_as_a_drop_that_hurts() {
        let Some(world) = map_world("mp_hurtgen") else {
            return;
        };
        let at = Vec3::new(411.7, 2786.0, -57.7);
        let ne = Vec3::new(1.0, 1.0, 0.0).normalize();
        let mut creases = Creases::default();
        let depth = drop_ahead(&world.collision, at, ne, 64.0, &mut creases);
        assert_eq!(depth, Some(f32::INFINITY));
        // Down the slope west of its foot there is no crease.
        let w = drop_ahead(&world.collision, at, -Vec3::X, 64.0, &mut creases);
        assert_eq!(w, None);
    }

    /// mp_ship's deck lip at (3285.5, -448.5, 56.125), where seed 5's
    /// unstick heading walked a bot off to the hull floor at -64: the deck
    /// runs on east and north, the drop lies west and south.
    #[test]
    fn a_drop_ahead_is_felt_off_a_decks_lip() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let lip = Vec3::new(3285.5413, -448.488, 56.125);
        let drops: Vec<bool> = (0..8)
            .map(|i| {
                let (s, c) = (i as f32 * 45.0f32).to_radians().sin_cos();
                drop_ahead(
                    &world.collision,
                    lip,
                    Vec3::new(c, s, 0.0),
                    64.0,
                    &mut Creases::default(),
                )
                .is_some()
            })
            .collect();
        assert_eq!((drops[0], drops[2]), (false, false), "{drops:?}");
        assert_eq!((drops[4], drops[6]), (true, true), "{drops:?}");
    }

    /// mp_depot, seed 7: a bot with a carbine (move scale over 1) at rest
    /// by the leap's foot on the step, running for the crate, covered 8.9
    /// units in the tick after the lip read clear off its pace of 5.1, and
    /// walked off the step's edge without the jump. The lip is the tick's
    /// move: it reads on the last tick on the ground, whatever the speed.
    #[test]
    fn the_lip_reads_on_the_last_tick_on_the_ground() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let fs = vcod_common::testing::game_fs().unwrap();
        let weapons = crate::weapons::WeaponTable::load(&fs, &crate::defs::V1);
        let w = &world.collision;
        let target = glam::Vec2::new(-1856.1, -538.4);
        let mut ps = PlayerState::spawn(Vec3::new(-1779.3, -607.2, 116.125), 0.0);
        ps.weapon = crate::defs::V1.weapon_index("m1carbine_mp").unwrap() as u8;
        let mw = MoveWorld::bare(w);
        let mut last = None;
        for tick in 1..20 {
            let lip = lip_ahead(w, &ps, target, weapons.defs());
            let was = ps.on_ground;
            let to = target - ps.origin.truncate();
            let cmd = UserCmd {
                server_time: tick * FRAME_MS,
                forward: 127,
                angles: [0, (to.y.atan2(to.x).to_degrees() * ANGLE2SHORT) as i32, 0],
                ..NULL_USERCMD
            };
            let dt = FRAME_MS as f32 / 1000.0;
            player_step(
                &mut ps,
                &mut [0; 3],
                &mut EventRing::default(),
                &cmd,
                dt,
                &mw,
                weapons.defs(),
            );
            if tick > 1 && was && !ps.on_ground {
                last = Some(lip);
                break;
            }
        }
        assert_eq!(
            last,
            Some(true),
            "the run leaves the step's edge, and the lip reads first"
        );
    }

    /// mp_ship's hold ladder: a floor at z 304 opens west of the shaft,
    /// its edge 15 units from where a body grabbed at x 4328.5 hangs. On
    /// the rungs only the strafe moves a body along the face: backing
    /// down facing the face and sliding toward the floor's node lands on
    /// it; without the slide the body passes the lip and hangs under it.
    /// mp_ship's ladder at (2150, -214): the floor at 104 lies behind a
    /// climber hanging level with it, 16 units out from the rungs. The
    /// push-off lands on it only braked; run on toward the floor, the
    /// jump carries the body past its edge into the hull at -64.
    #[test]
    fn a_push_off_lands_on_the_floor_behind_the_rungs() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let floor = Vec3::new(2144.741, -229.90549, 104.125);
        let mut ps = PlayerState::spawn(Vec3::new(2146.5, -214.1, 140.0), 90.0);
        let mut sim = Sim::default();
        // Let go of the key 6 above: a 95 units/s descent stops in 5.6.
        for _ in 0..80 {
            let down = if ps.origin.z > 109.0 { -127 } else { 0 };
            let cmd = UserCmd {
                forward: down,
                angles: [
                    (-LADDER_PITCH * ANGLE2SHORT) as i32,
                    (90.0 * ANGLE2SHORT) as i32,
                    0,
                ],
                ..NULL_USERCMD
            };
            sim.tick(w, &mut ps, &cmd);
        }
        assert!(
            ps.on_ladder && (ps.origin.z - floor.z).abs() < 4.0,
            "{}",
            ps.origin
        );
        let yaw = push_off(w, &ps, floor, &[]).expect("a push-off");
        assert!((yaw - 90.0).abs() < 1.0, "{yaw}");
        // Run on at the floor, the same jump overshoots.
        let mut free = ps;
        for i in 0..40 {
            let cmd = UserCmd {
                forward: if i == 0 { 0 } else { -127 },
                up: if i == 0 { 127 } else { 0 },
                angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
                ..NULL_USERCMD
            };
            sim.tick(w, &mut free, &cmd);
            if free.on_ground {
                break;
            }
        }
        assert!(free.on_ground && free.origin.z < 0.0, "{}", free.origin);
        // Climbing past the floor, or at its foot, there is none.
        let mut low = ps;
        low.origin.z = 60.0;
        assert_eq!(push_off(w, &low, floor, &[]), None);
        assert_eq!(
            push_off(w, &ps, Vec3::new(2160.0, -156.1, 216.1), &[]),
            None
        );
    }

    /// mp_ship's crow's nest head stands on the ladder's plate in a hatch
    /// about 40 by 46 units, 11 units from the top's face to the west. The
    /// sweep west stops at the face before the first sample, and a sample
    /// north-west stands on the hatch's rim: the probe read no drop, and a
    /// spell's heading ran bots down the hatch.
    #[test]
    fn the_drop_probe_feels_the_crows_nest_hatch() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let plate = Vec3::new(4963.9, 64.0, 1219.125);
        let mut creases = Creases::default();
        let mut drop = |yaw: f32| {
            let (s, c) = yaw.to_radians().sin_cos();
            drop_ahead(
                &world.collision,
                plate,
                Vec3::new(c, s, 0.0),
                64.0,
                &mut creases,
            )
        };
        assert_eq!(drop(180.0), Some(f32::INFINITY));
        assert_eq!(drop(135.0), Some(f32::INFINITY));
        // The exit along the rim stays open.
        assert_eq!(drop(270.0), None);
    }
}
