//! United Offensive's wheel and tread dust: the surface effects the vehicle
//! DObj callback (uo cgame 0x30020540) plays under each `tag_wheel_*` of a
//! drawn vehicle as it travels, gated by `cg_vehicletrails`
//! (docs/research/coduo-vehicles.md 15).

use crate::fx::sim::Rng;
use glam::Vec3;
use std::collections::HashMap;
use vcod_common::collision::{CollisionWorld, Pusher, Trace, sound_material};

/// The effects 0x3044cac0 holds, registered at load (uo cgame 0x3002c82c):
/// slots 1..6 a tank's, 7..12 a four-wheel's, by [`slot`]. Both snow slots
/// of each kind name the same file.
pub const EFFECTS: [&str; 12] = [
    "fx/map_mp/mp_tanktread_grass.efx",
    "fx/map_mp/mp_tanktread_sand.efx",
    "fx/map_mp/mp_tanktread_dirt.efx",
    "fx/map_mp/mp_tanktread_rock.efx",
    "fx/map_mp/mp_tanktread_snow.efx",
    "fx/map_mp/mp_tanktread_snow.efx",
    "fx/map_mp/mp_jeepwheel_grass.efx",
    "fx/map_mp/mp_jeepwheel_sand.efx",
    "fx/map_mp/mp_jeepwheel_dirt.efx",
    "fx/map_mp/mp_jeepwheel_rock.efx",
    "fx/map_mp/mp_jeepwheel_snow.efx",
    "fx/map_mp/mp_jeepwheel_snow.efx",
];

/// The surface switch at 0x3002118b: material to table slot, 0 for none.
fn slot(material: i32) -> usize {
    match material {
        10 => 1,      // grass
        18 => 2,      // sand
        6 => 3,       // dirt
        11 | 17 => 4, // gravel, rock
        12 => 5,      // ice
        19 => 6,      // snow
        _ => 0,
    }
}

/// The ground trace starts 40 above the wheel (0x3007c284) and reaches 16
/// past the suspension's `time2 / 1000` below it (0x3007bf00).
const TRACE_UP: f32 = 40.0;
const TRACE_DOWN: f32 = 16.0;
/// A wheel plays once it has gone more than 32 units since its last play
/// (0x3007bdd0); past 200 (0x3007bf44) the mark restarts where it is.
const STEP: f32 = 32.0;
const JUMP: f32 = 200.0;

/// One drawn vehicle's wheels this frame.
#[derive(Clone, Debug)]
pub struct Wheels {
    pub entity: u32,
    /// `surfType` 1.
    pub four_wheel: bool,
    /// The vehicle's forward and up axes as drawn.
    pub forward: Vec3,
    pub up: Vec3,
    /// `time2 / 1000`, the suspension's reach.
    pub reach: f32,
    /// Each of `vehiclepose::WHEEL_TAGS` in world space, `None` where the model has
    /// no such tag.
    pub points: [Option<Vec3>; 6],
    /// Its collmap's inline model and the drawn origin and angles, what
    /// the entity clip meets it at.
    pub collmap: Option<(usize, Vec3, Vec3)>,
}

/// One wheel's trace under mask 0x211: the world (statics left out), then
/// the entity clip past `entity`'s own collmap.
pub fn wheel_trace(world: &CollisionWorld, entity: u32, start: Vec3, end: Vec3) -> Trace {
    world.point_trace_except(start, end, 0x211, false, Some(Pusher::Copy(entity)))
}

/// Puts each drawn vehicle's own copy of its collmap where it is drawn,
/// for the wheel traces' entity clip (0x300350d0 clips a brush model at
/// the centity's lerped origin and angles, testing no contents).
pub fn place_collmaps(world: &CollisionWorld, vehicles: &[Wheels]) {
    for v in vehicles {
        if let Some((model, origin, angles)) = v.collmap {
            world.set_model_instance(v.entity, model, Some((origin, angles, !0)));
        }
    }
}

/// One effect to play, on `dir` as syscall 0xe8 takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct TrailPlay {
    pub path: &'static str,
    pub pos: Vec3,
    pub dir: Vec3,
}

/// `cent+0x23c`: each wheel's last play, per entity number, kept until a
/// new level (a new [`VehicleTrails`]); zero means unset.
pub struct VehicleTrails {
    last: HashMap<u32, [Vec3; 6]>,
    rng: Rng,
}

impl Default for VehicleTrails {
    fn default() -> Self {
        VehicleTrails {
            last: HashMap::new(),
            rng: Rng::new(0x7261_696c),
        }
    }
}

impl VehicleTrails {
    /// The effects this frame's wheels play. `ground` is the wheel trace
    /// (mask 0x211, the world and then the entity clip past the vehicle
    /// whose number it is handed): the struck surface's flags, or `None`
    /// for no hit. With `enabled` off nothing moves, as the callback
    /// skips the whole step.
    pub fn frame(
        &mut self,
        enabled: bool,
        vehicles: &[Wheels],
        mut ground: impl FnMut(u32, Vec3, Vec3) -> Option<u32>,
    ) -> Vec<TrailPlay> {
        // `unit` is `rand() / 32768.0`.
        let mut out = Vec::new();
        if !enabled {
            return out;
        }
        let VehicleTrails { last, rng } = self;
        let mut unit = || rng.next_f32();
        for v in vehicles {
            let last = last.entry(v.entity).or_insert([Vec3::ZERO; 6]);
            for (i, p) in v.points.iter().enumerate() {
                let Some(cur) = *p else { continue };
                let start = cur + v.up * TRACE_UP;
                let end = cur - v.up * (v.reach + TRACE_DOWN);
                let Some(flags) = ground(v.entity, start, end) else {
                    continue;
                };
                if last[i] == Vec3::ZERO {
                    last[i] = cur;
                    continue;
                }
                let moved = cur - last[i];
                let dist = moved.length();
                if dist > JUMP {
                    last[i] = cur;
                    continue;
                }
                if dist <= STEP {
                    continue;
                }
                let motion = moved / dist;
                last[i] = cur;
                let mut s = slot(sound_material(flags));
                let mut pos = cur;
                if v.four_wheel {
                    if s != 0 {
                        s += 6;
                    }
                    // A front wheel, or any wheel rolling backwards, drops
                    // its dust 10 down (0x3007bda4).
                    let front = i < 2;
                    if front || v.forward.dot(motion) < 0.0 {
                        pos.z -= 10.0;
                    } else {
                        pos.z += unit() * 8.0;
                    }
                } else {
                    pos.z += unit() * 8.0;
                }
                if s == 0 {
                    continue;
                }
                // Thrown back along the travel, each axis 0.25..0.75 of it.
                let mut dir = -motion;
                for k in 0..3 {
                    dir[k] *= unit() * 0.5 + 0.25;
                }
                out.push(TrailPlay {
                    path: EFFECTS[s - 1],
                    pos,
                    dir,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIRT: u32 = 6 << 20;

    fn tank(at: Vec3) -> Wheels {
        Wheels {
            entity: 7,
            four_wheel: false,
            forward: Vec3::X,
            up: Vec3::Z,
            reach: 0.024,
            points: [Some(at), None, None, None, None, None],
            collmap: None,
        }
    }

    fn run(t: &mut VehicleTrails, at: Vec3) -> Vec<TrailPlay> {
        t.frame(true, &[tank(at)], |_, _, _| Some(DIRT))
    }

    #[test]
    fn a_wheel_plays_every_32_units_on_its_surface() {
        let mut t = VehicleTrails::default();
        assert!(
            run(&mut t, Vec3::new(100.0, 0.0, 0.0)).is_empty(),
            "the first frame marks"
        );
        assert!(
            run(&mut t, Vec3::new(132.0, 0.0, 0.0)).is_empty(),
            "32 is not past 32"
        );
        let plays = run(&mut t, Vec3::new(140.0, 0.0, 0.0));
        assert_eq!(plays.len(), 1);
        let p = &plays[0];
        assert_eq!(p.path, "fx/map_mp/mp_tanktread_dirt.efx");
        assert!((0.0..8.0).contains(&p.pos.z), "{:?}", p.pos);
        assert!(p.dir.x <= -0.25 && p.dir.x >= -0.75, "{:?}", p.dir);
        assert_eq!((p.dir.y, p.dir.z), (-0.0, -0.0));
        assert!(
            run(&mut t, Vec3::new(150.0, 0.0, 0.0)).is_empty(),
            "marked at 140"
        );
    }

    #[test]
    fn a_jump_restarts_the_mark_and_a_miss_or_the_cvar_moves_nothing() {
        let mut t = VehicleTrails::default();
        run(&mut t, Vec3::new(0.0, 0.0, 0.0) + Vec3::X);
        assert!(run(&mut t, Vec3::new(500.0, 0.0, 0.0)).is_empty());
        // airborne: the mark stays at 500
        assert!(
            t.frame(true, &[tank(Vec3::new(560.0, 0.0, 0.0))], |_, _, _| None)
                .is_empty()
        );
        assert!(
            t.frame(false, &[tank(Vec3::new(560.0, 0.0, 0.0))], |_, _, _| Some(
                DIRT
            ))
            .is_empty()
        );
        assert_eq!(run(&mut t, Vec3::new(560.0, 0.0, 0.0)).len(), 1);
    }

    #[test]
    fn a_four_wheel_takes_the_jeep_row_and_drops_a_front_wheel() {
        let mut t = VehicleTrails::default();
        let jeep = |x: f32| Wheels {
            entity: 3,
            four_wheel: true,
            forward: Vec3::X,
            up: Vec3::Z,
            reach: 0.008,
            points: [
                Some(Vec3::new(x, 0.0, 0.0)),
                None,
                Some(Vec3::new(x - 100.0, 0.0, 0.0)),
                None,
                None,
                None,
            ],
            collmap: None,
        };
        let sand = 18 << 20;
        t.frame(true, &[jeep(1.0)], |_, _, _| Some(sand));
        let plays = t.frame(true, &[jeep(50.0)], |_, _, _| Some(sand));
        assert_eq!(plays.len(), 2);
        assert!(
            plays
                .iter()
                .all(|p| p.path == "fx/map_mp/mp_jeepwheel_sand.efx")
        );
        assert_eq!(plays[0].pos.z, -10.0, "the front wheel");
        assert!(plays[1].pos.z >= 0.0, "a back wheel rolling forward");
        // backing up drops the back wheel too
        let plays = t.frame(true, &[jeep(0.0)], |_, _, _| Some(sand));
        assert_eq!(plays[1].pos.z, -10.0);
        assert!(plays[1].dir.x > 0.0, "thrown against the travel");
    }

    #[test]
    fn a_surface_off_the_table_plays_nothing_but_moves_the_mark() {
        let mut t = VehicleTrails::default();
        let concrete = 5 << 20;
        t.frame(true, &[tank(Vec3::X)], |_, _, _| Some(concrete));
        assert!(
            t.frame(true, &[tank(Vec3::new(50.0, 0.0, 0.0))], |_, _, _| Some(
                concrete
            ))
            .is_empty()
        );
        assert!(
            run(&mut t, Vec3::new(70.0, 0.0, 0.0)).is_empty(),
            "marked at 50"
        );
    }

    /// The wheel trace meets another vehicle's collmap where that vehicle
    /// is drawn and passes its own: a tank's wheel inside its own hull reads
    /// the floor, one over the second tank its roof. Both tanks share
    /// inline model 1, as every vehicle of a model shares its collmap.
    #[test]
    fn a_wheel_reads_another_vehicles_collmap_and_never_its_own() {
        let world = vcod_common::collision::submodel_test_world(
            "{\n\"classname\" \"script_vehicle_collmap\"\n\"model\" \"*1\"\n}",
            &[([-32.0, -32.0, 0.0], [32.0, 32.0, 48.0])],
        );
        world.set_model_linked(1, false);
        let at = |entity: u32, x: f32| Wheels {
            collmap: Some((1, Vec3::new(x, 0.0, 0.0), Vec3::ZERO)),
            entity,
            ..tank(Vec3::new(x, 0.0, 8.0))
        };
        place_collmaps(&world, &[at(7, 0.0), at(8, 200.0)]);
        let down = |entity: u32, x: f32, z: f32| {
            let t = wheel_trace(
                &world,
                entity,
                Vec3::new(x, 0.0, z + 40.0),
                Vec3::new(x, 0.0, z - 16.0),
            );
            (t.fraction < 1.0).then_some(t.endpos.z.round())
        };
        assert_eq!(down(7, 0.0, 8.0), Some(0.0), "its own hull is passed");
        assert_eq!(down(7, 200.0, 50.0), Some(48.0), "the other tank's roof");
        assert_eq!(down(8, 200.0, 50.0), None, "nothing under its own");
    }

    /// The driver captures (`*-uo-tank.bin` on mp_cassino, `*-uo-jeep.bin`
    /// on mp_peaks) replayed through the posed wheels over each map's own
    /// ground, ten frames a snapshot: the driven vehicle throws its kind's
    /// dust on the surface under it, the parked ones none, and every effect
    /// the table names is in the paks. Needs `COD_DIR` at a UO install.
    #[test]
    fn the_uo_driver_captures_throw_dust_off_the_ground_they_cross() {
        use crate::entities::{EntityVisual, resolve_visual};
        use std::collections::{BTreeMap, BTreeSet};
        use vcod_common::collision::CollisionWorld;
        use vcod_common::net::flags::ET_VEHICLE_V22;
        use vcod_common::net::protocol::PROTOCOL_V22;
        use vcod_common::skeleton::{PoseBuffer, Skeleton};
        use vcod_common::turretpose::angles_quat;
        use vcod_common::vehiclepose::{self, VehicleFields};
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        for path in EFFECTS {
            let text = fs
                .read(path)
                .unwrap_or_else(|| panic!("{path} not in the paks"));
            crate::fx::efx::parse(&String::from_utf8_lossy(&text)).unwrap();
        }
        let p = &PROTOCOL_V22;
        // mp_cassino's square is cobblestone (rock), mp_peaks' road snow.
        for (tag, map, driven) in [
            ("uo-tank", "mp_cassino", "fx/map_mp/mp_tanktread_rock.efx"),
            ("uo-jeep", "mp_peaks", "fx/map_mp/mp_jeepwheel_snow.efx"),
        ] {
            let bsp =
                vcod_common::bsp::parse(&fs.read(&format!("maps/mp/{map}.bsp")).unwrap()).unwrap();
            let world = CollisionWorld::build(&bsp, &[]);
            for model in 1..world.model_count() {
                world.set_model_linked(model, false);
            }
            let gs =
                vcod_common::testing::fixture_gamestate(p, &format!("net/gamestate-{tag}.bin"));
            let frames = vcod_common::testing::fixture_frames(
                p,
                &format!("net/gamestate-{tag}.bin"),
                &format!("net/snapshots-{tag}.bin"),
            );
            let ridden = frames
                .iter()
                .map(|f| f.snapshot.ps.field_i32(p, "viewlocked_entNum"))
                .find(|&n| n > 0)
                .unwrap() as u32;
            let mut skels: BTreeMap<String, Skeleton> = BTreeMap::new();
            let mut trails = VehicleTrails::default();
            let mut plays: BTreeMap<u32, Vec<TrailPlay>> = BTreeMap::new();
            let mut materials = BTreeSet::new();
            for pair in frames.windows(2) {
                let (a, b) = (&pair[0].snapshot, &pair[1].snapshot);
                for step in 0..10 {
                    let f = step as f32 / 10.0;
                    let mut wheels = Vec::new();
                    for (&num, eb) in &b.entities {
                        if eb.field_i32(p, "eType") != ET_VEHICLE_V22 {
                            continue;
                        }
                        let ea = a.entities.get(&num).unwrap_or(eb);
                        let EntityVisual::Vehicle(model) =
                            resolve_visual(eb, &BTreeMap::new(), &gs.configstrings, p)
                        else {
                            panic!("{tag}: vehicle {num} has no model");
                        };
                        let skel = skels.entry(model.clone()).or_insert_with(|| {
                            Skeleton::build(&[&vcod_common::model::load(&fs, &model).unwrap()])
                        });
                        let pos = Vec3::from(ea.origin(p)).lerp(Vec3::from(eb.origin(p)), f);
                        let (aa, ab) = (ea.angles(p), eb.angles(p));
                        let ang = std::array::from_fn(|k| {
                            let d = (ab[k] - aa[k] + 540.0).rem_euclid(360.0) - 180.0;
                            aa[k] + d * f
                        });
                        let rot = angles_quat(ang);
                        let pose = vehiclepose::lerp(
                            &VehicleFields::read(ea, p),
                            &VehicleFields::read(eb, p),
                            f,
                        );
                        let mut bones = PoseBuffer::new(skel);
                        vehiclepose::apply(&mut bones, skel, &pose);
                        let worlds = bones.bone_worlds(skel);
                        wheels.push(Wheels {
                            entity: num,
                            four_wheel: pose.kind == vehiclepose::KIND_FOUR_WHEEL,
                            forward: rot * Vec3::X,
                            up: rot * Vec3::Z,
                            reach: eb.field_i32(p, "time2") as f32 / 1000.0,
                            points: vehiclepose::wheel_points(skel, &worlds, (pos, rot)),
                            collmap: Some((
                                eb.field_i32(p, "index") as usize,
                                pos,
                                Vec3::from(ang),
                            )),
                        });
                    }
                    place_collmaps(&world, &wheels);
                    for play in trails.frame(true, &wheels, |n, s, e| {
                        let t = wheel_trace(&world, n, s, e);
                        (t.fraction < 1.0).then(|| {
                            materials.insert(sound_material(t.surface_flags));
                            t.surface_flags
                        })
                    }) {
                        let num = wheels
                            .iter()
                            .find(|w| w.points.iter().flatten().any(|q| q.x == play.pos.x))
                            .unwrap()
                            .entity;
                        plays.entry(num).or_default().push(play);
                    }
                }
            }
            let own = plays.remove(&ridden).unwrap_or_default();
            assert!(
                plays.is_empty(),
                "{tag}: parked vehicles threw dust: {plays:?}"
            );
            // 191 and 179 plays at this writing
            assert!(
                own.len() >= 100,
                "{tag}: {} plays over {materials:?}",
                own.len()
            );
            for play in &own {
                assert_eq!(play.path, driven, "{tag}");
                assert!(
                    play.dir.length() > 0.2 && play.dir.length() < 0.8,
                    "{tag}: {play:?}"
                );
            }
        }
    }
}
