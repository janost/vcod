//! United Offensive's client-traced rounds. A vehicle's MG fire event sets
//! parm bit 7 and the server sends no bullet-hit temp entity for the round:
//! the cgame traces it itself from the gun's tag (uo cgame 0x30045550,
//! docs/research/coduo-diff.md 19, "Client-traced rounds").

use glam::Vec3;
use vcod_common::collision::sound_material;
use vcod_common::weapon::WeaponDef;

use crate::fx::registry::{ClientRound, EV_UO_BULLET_HIT};
use crate::fx::sim::{Rng, TagPose};
use crate::fx::vectoangles;
use vcod_common::net::events::{GameEvent, dir_to_byte};
use vcod_common::net::protocol::ENTITYNUM_WORLD;
use vcod_common::x87::angle_vectors;

/// `CG_Trace`'s mask for the round (0x30045992).
pub const MASK_CLIENT_SHOT: u32 = 0x2802031;
/// The second trace's mask (0x300459f2): a vehicle's collmap brushes,
/// which the first mask passes (docs/research/coduo-vehicles.md 10.3).
pub const MASK_VEHICLE_SHOT: u32 = 0x10000;
/// The round's reach (0x3007bf70).
const RANGE: f32 = 10000.0;
/// How far the coaxial gun's aim point is traced from the view (0x3007c3ec).
const ALTFIRE_AIM_RANGE: f32 = 10240.0;
/// How far the coaxial gun turns toward the view's aim point, pitch and
/// yaw in degrees (0x3007c3e8, 0x3007bde0, 0x3007c150, 0x3007bce4).
const ALTFIRE_TURN: [f32; 2] = [5.0, 2.0];
/// Spreads drawn before a round that keeps finding a player is dropped.
const ATTEMPTS: usize = 8;
/// Entity numbers below this are players, whose hits the server reports.
const MAX_CLIENTS: u32 = 64;
/// A surface that takes no impact (`surfaceFlags` 0x4, the sky).
const SURF_SKY: u32 = 0x4;

/// The tag the coaxial gun fires from, which aims off the view.
pub const TAG_ALTFIRE: &str = "tag_altfire";

/// What `CG_Trace` reports for a round: the contact and the entity hit
/// (1022 the world, 1023 nothing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundTrace {
    pub fraction: f32,
    pub endpos: Vec3,
    pub normal: Vec3,
    pub surface_flags: u32,
    pub entity: u32,
}

/// Where a client-traced round went.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// The world or a non-player entity: `CG_BulletHitEvent` (0x30048e60)
    /// with the trace's normal and the shot's reflection off it.
    Hit {
        pos: Vec3,
        normal: Vec3,
        reflect: Vec3,
        surf_type: i32,
    },
    /// Nothing hit, or the sky: a tracer from the tag to the end when the
    /// chance roll passes (0x30048260).
    Miss { start: Vec3, end: Vec3 },
}

/// The view the coaxial gun's aim turns toward: the refdef's origin and the
/// snapshot playerState's `viewangles`.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub origin: Vec3,
    pub angles: [f32; 3],
}

/// The once-per-frame gate and the spread's random draws.
pub struct ClientBullets {
    /// The frame and entity last traced: the cgame traces one round per
    /// entity per frame (0x3004556e).
    last: Option<(u64, u32)>,
    rng: Rng,
}

impl Default for ClientBullets {
    fn default() -> Self {
        ClientBullets {
            last: None,
            rng: Rng::new(0x5eed_b011),
        }
    }
}

/// `AngleSubtract` (0x3004bd70): `a - b` into (-180, 180].
fn angle_sub(a: f32, b: f32) -> f32 {
    let mut d = vcod_common::fguard::turns(a - b);
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

impl ClientBullets {
    /// The round `round.entity` fires from `round.tag`, posed at `pose`
    /// this frame (`frame` names the frame), `None` when it is the entity's second this
    /// frame or every spread found a player. `trace(start, end, skip)` is
    /// `CG_Trace` under [`MASK_CLIENT_SHOT`], returning the hit entity;
    /// `vehicles(start, end, skip)` is its second trace under
    /// [`MASK_VEHICLE_SHOT`], `Some` only when it stopped on a vehicle
    /// (`eType` 12), which then replaces the first when it is shorter
    /// (0x300459fc..0x30045a46). Only the coaxial gun's second trace skips
    /// the shooter.
    /// The coaxial gun's `tag_altfire` aims off `view`: toward where the
    /// view's line meets the world, turned at most [`ALTFIRE_TURN`] off the
    /// tag (0x30045638..0x30045806).
    pub fn fire(
        &mut self,
        frame: u64,
        round: &ClientRound,
        def: Option<&WeaponDef>,
        pose: &TagPose,
        view: Option<View>,
        trace: &dyn Fn(Vec3, Vec3, u32) -> RoundTrace,
        vehicles: &dyn Fn(Vec3, Vec3, Option<u32>) -> Option<RoundTrace>,
    ) -> Option<Outcome> {
        let (entity, tag) = (round.entity, round.tag);
        if self.last == Some((frame, entity)) {
            return None;
        }
        self.last = Some((frame, entity));
        let start = pose.origin;
        let altfire = tag.eq_ignore_ascii_case(TAG_ALTFIRE);
        let axis = match view.filter(|_| altfire) {
            Some(v) => {
                let [fwd, _, _] = angle_vectors(Vec3::from(v.angles));
                let aim = trace(v.origin, v.origin + fwd * ALTFIRE_AIM_RANGE, entity);
                let gun = vectoangles(pose.axis[0]);
                let want = vectoangles((aim.endpos - start).normalize_or_zero());
                let turn = |i: usize| {
                    let d = angle_sub(gun[i], want[i]);
                    angle_sub(gun[i], d.clamp(-ALTFIRE_TURN[i], ALTFIRE_TURN[i]))
                };
                angle_vectors(Vec3::new(turn(0), turn(1), gun[2]))
            }
            None => pose.axis,
        };
        // `hipSpreadStandMin` plus a draw of `hipSpreadMax` (the turret
        // files' `minSpread` and `maxSpread`), in degrees.
        let (min, max) = def.map_or((0.0, 0.0), WeaponDef::spread_range);
        for _ in 0..ATTEMPTS {
            let spread = self.rng.next_f32() * max + min;
            let r = spread.to_radians().tan() * RANGE;
            // 0x30049270: an angle and a radius, both uniform.
            let theta = self.rng.next_f32() * std::f32::consts::TAU;
            let k = self.rng.next_f32();
            let (s, c) = theta.sin_cos();
            let end = start + axis[0] * RANGE + axis[1] * (c * k * r) + axis[2] * (s * k * r);
            let mut t = trace(start, end, entity);
            if let Some(v) = vehicles(start, end, altfire.then_some(entity))
                && v.fraction < t.fraction
            {
                t = v;
            }
            if t.entity < MAX_CLIENTS {
                continue;
            }
            if t.fraction >= 1.0 || t.surface_flags & SURF_SKY != 0 {
                return Some(Outcome::Miss {
                    start,
                    end: t.endpos,
                });
            }
            let dir = (t.endpos - start).normalize_or_zero();
            let reflect = dir - 2.0 * dir.dot(t.normal) * t.normal;
            return Some(Outcome::Hit {
                pos: t.endpos,
                normal: t.normal,
                reflect,
                surf_type: sound_material(t.surface_flags),
            });
        }
        None
    }
}

/// The bullet hit a round's [`Outcome::Hit`] raises, as the server's
/// `EV_BULLET_HIT` would carry it: the shooter in `otherEntityNum`, the
/// normal and the reflection as byte dirs, the surface. Retail hands the
/// vectors over whole (0x30045b72); the byte dirs round them.
pub fn hit_event(
    round: &ClientRound,
    pos: Vec3,
    normal: Vec3,
    reflect: Vec3,
    surf_type: i32,
) -> GameEvent {
    GameEvent {
        event: EV_UO_BULLET_HIT,
        parm: dir_to_byte(normal.to_array()),
        entity_num: ENTITYNUM_WORLD,
        client_num: -1,
        weapon: round.weapon,
        surf_type,
        pos: pos.to_array(),
        dir: [0.0; 3],
        scale: dir_to_byte(reflect.to_array()),
        other_entity_num: round.entity,
        attacker_entity_num: -1,
        e_type: vcod_common::net::flags::ET_GENERAL,
        eflags: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn round(entity: u32, tag: &'static str) -> ClientRound {
        ClientRound {
            entity,
            weapon: 1,
            tag,
            tracer_tag: tag,
        }
    }

    fn pose() -> TagPose {
        TagPose {
            origin: Vec3::new(0.0, 0.0, 100.0),
            axis: [Vec3::X, Vec3::Y, Vec3::Z],
        }
    }

    fn wall(start: Vec3, end: Vec3, x: f32, entity: u32) -> RoundTrace {
        let f = ((x - start.x) / (end.x - start.x)).clamp(0.0, 1.0);
        RoundTrace {
            fraction: f,
            endpos: start + (end - start) * f,
            normal: Vec3::NEG_X,
            surface_flags: 6 << 20,
            entity: if f < 1.0 { entity } else { 1023 },
        }
    }

    fn no_vehicle(_: Vec3, _: Vec3, _: Option<u32>) -> Option<RoundTrace> {
        None
    }

    fn turret_def() -> WeaponDef {
        let map = [("minSpread", "0.1"), ("maxSpread", "2")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into_iter()
            .collect();
        WeaponDef::from_map(&map)
    }

    /// A round into a wall hits it with the wall's normal and surface,
    /// inside the turret file's spread; a second round of the entity in
    /// the same frame is not traced.
    #[test]
    fn a_round_hits_the_wall_inside_its_spread_once_a_frame() {
        let def = turret_def();
        let mut b = ClientBullets::default();
        let trace = |s: Vec3, e: Vec3, _skip: u32| wall(s, e, 500.0, 1022);
        let out = b.fire(
            1,
            &round(200, "tag_flash"),
            Some(&def),
            &pose(),
            None,
            &trace,
            &no_vehicle,
        );
        let Some(Outcome::Hit {
            pos,
            normal,
            reflect,
            surf_type,
        }) = out
        else {
            panic!("{out:?}");
        };
        assert_eq!((normal, surf_type), (Vec3::NEG_X, 6));
        assert!((pos.x - 500.0).abs() < 1e-3);
        let off = (pos - Vec3::new(500.0, 0.0, 100.0)).length();
        assert!(off <= 500.0 * 2.1f32.to_radians().tan() + 1e-3, "{off}");
        assert!(reflect.x < 0.0);
        assert_eq!(
            b.fire(
                1,
                &round(200, "tag_flash"),
                Some(&def),
                &pose(),
                None,
                &trace,
                &no_vehicle
            ),
            None
        );
        assert!(
            b.fire(
                1,
                &round(201, "tag_flash"),
                Some(&def),
                &pose(),
                None,
                &trace,
                &no_vehicle
            )
            .is_some()
        );
    }

    /// A round that finds a player draws again, eight times at most; the
    /// sky and an open shot are misses.
    #[test]
    fn a_player_hit_redraws_and_the_sky_is_a_miss() {
        let def = turret_def();
        let mut b = ClientBullets::default();
        let calls = RefCell::new(0);
        let player = |s: Vec3, e: Vec3, _: u32| {
            *calls.borrow_mut() += 1;
            wall(s, e, 300.0, 5)
        };
        assert_eq!(
            b.fire(
                1,
                &round(200, "tag_flash"),
                Some(&def),
                &pose(),
                None,
                &player,
                &no_vehicle
            ),
            None
        );
        assert_eq!(*calls.borrow(), ATTEMPTS);
        let sky = |s: Vec3, e: Vec3, _: u32| RoundTrace {
            surface_flags: SURF_SKY,
            ..wall(s, e, 300.0, 1022)
        };
        assert!(matches!(
            b.fire(
                2,
                &round(200, "tag_flash"),
                Some(&def),
                &pose(),
                None,
                &sky,
                &no_vehicle
            ),
            Some(Outcome::Miss { .. })
        ));
        let open = |s: Vec3, e: Vec3, _: u32| wall(s, e, 1e9, 1022);
        let Some(Outcome::Miss { start, end }) = b.fire(
            3,
            &round(200, "tag_flash"),
            Some(&def),
            &pose(),
            None,
            &open,
            &no_vehicle,
        ) else {
            panic!();
        };
        assert_eq!(start, pose().origin);
        assert!((end.x - RANGE).abs() < 1.0);
    }

    /// The coaxial gun turns toward the view's aim point, two degrees of
    /// yaw at most.
    #[test]
    fn the_coaxial_gun_turns_toward_the_view_at_most_two_degrees() {
        let mut b = ClientBullets::default();
        // No spread: the round flies the turned axis.
        let open = |s: Vec3, e: Vec3, _: u32| wall(s, e, 1e9, 1022);
        let shot = |b: &mut ClientBullets, frame, yaw: f32| {
            let view = View {
                origin: Vec3::new(0.0, 0.0, 150.0),
                angles: [0.0, yaw, 0.0],
            };
            // The view's line stops 1000 out.
            let aim = |s: Vec3, e: Vec3, n: u32| match s == view.origin {
                true => wall(s, e, 1000.0 * yaw.to_radians().cos(), 1022),
                false => open(s, e, n),
            };
            match b.fire(
                frame,
                &round(200, TAG_ALTFIRE),
                None,
                &pose(),
                Some(view),
                &aim,
                &no_vehicle,
            ) {
                Some(Outcome::Miss { end, .. }) => end.y.atan2(end.x).to_degrees(),
                o => panic!("{o:?}"),
            }
        };
        assert!((shot(&mut b, 1, 1.0) - 1.0).abs() < 0.05);
        assert!((shot(&mut b, 2, 30.0) - 2.0).abs() < 1e-3);
        assert!((shot(&mut b, 3, -30.0) + 2.0).abs() < 1e-3);
    }

    /// A vehicle's collmap the first trace passed stops the round when it
    /// is nearer than what the first trace hit, and draws the hit on it;
    /// a farther one leaves the wall's. Only the coaxial gun's second
    /// trace skips its own vehicle.
    #[test]
    fn a_vehicle_nearer_than_the_wall_stops_the_round() {
        let def = turret_def();
        let mut b = ClientBullets::default();
        let wall_at = |s: Vec3, e: Vec3, _: u32| wall(s, e, 500.0, 1022);
        let skips = RefCell::new(Vec::new());
        let seen = &skips;
        let hull = |x: f32| {
            move |s: Vec3, e: Vec3, skip: Option<u32>| {
                seen.borrow_mut().push(skip);
                Some(RoundTrace {
                    normal: Vec3::NEG_X,
                    surface_flags: 13 << 20,
                    ..wall(s, e, x, 300)
                })
            }
        };
        let near = hull(200.0);
        let Some(Outcome::Hit { pos, surf_type, .. }) = b.fire(
            1,
            &round(200, "tag_secondary_flash"),
            Some(&def),
            &pose(),
            None,
            &wall_at,
            &near,
        ) else {
            panic!();
        };
        assert!((pos.x - 200.0).abs() < 1e-3 && surf_type == 13, "{pos}");
        let far = hull(800.0);
        let Some(Outcome::Hit { pos, surf_type, .. }) = b.fire(
            2,
            &round(200, TAG_ALTFIRE),
            Some(&def),
            &pose(),
            None,
            &wall_at,
            &far,
        ) else {
            panic!();
        };
        assert!((pos.x - 500.0).abs() < 1e-3 && surf_type == 6, "{pos}");
        assert_eq!(*skips.borrow(), [None, Some(200)]);
    }

    /// mp_cassino's tanks in the retail tank capture (`net/snapshots-uo-
    /// tank.bin`), their collmaps placed off the snapshot as the client
    /// places them: a round across a hull passes it under the shot mask
    /// (the collmap brushes carry 0x28031640), stops on it under the second
    /// trace's and names the tank, and the coaxial's skip passes its own.
    #[test]
    fn the_uo_tank_capture_collmaps_stop_only_the_second_trace() {
        use vcod_common::collision::{CollisionWorld, Pusher};
        use vcod_common::net::flags::ET_VEHICLE_V22;
        use vcod_common::net::protocol::PROTOCOL_V22;
        use vcod_common::pmove::movers::SnapshotMovers;
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let p = &PROTOCOL_V22;
        let bsp = vcod_common::bsp::parse(&fs.read("maps/mp/mp_cassino.bsp").unwrap()).unwrap();
        let world = CollisionWorld::build(&bsp, &[]);
        let frames = vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-uo-tank.bin",
            "net/snapshots-uo-tank.bin",
        );
        let s = &frames.last().unwrap().snapshot;
        SnapshotMovers::from_entities(p, &s.entities).place(&world, s.server_time.raw());
        let tanks: Vec<_> = s
            .entities
            .iter()
            .filter(|(_, e)| e.field_i32(p, "eType") == ET_VEHICLE_V22)
            .collect();
        assert!(!tanks.is_empty());
        for (&n, e) in tanks {
            let o = Vec3::from(e.origin(p)) + Vec3::Z * 40.0;
            let (start, end) = (o + Vec3::X * 150.0, o - Vec3::X * 150.0);
            let shot = world.point_trace(start, end, MASK_CLIENT_SHOT, false);
            assert_eq!(shot.fraction, 1.0, "{n}: the shot mask met the collmap");
            let hull = world.point_trace(start, end, MASK_VEHICLE_SHOT, false);
            assert!(hull.fraction < 0.4, "{n}: {}", hull.fraction);
            assert_eq!(world.entity_num(&hull), n);
            let own = world.point_trace_except(
                start,
                end,
                MASK_VEHICLE_SHOT,
                false,
                Some(Pusher::Copy(n)),
            );
            assert_eq!(own.fraction, 1.0, "{n}");
        }
    }
}
