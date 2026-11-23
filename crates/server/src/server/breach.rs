//! Bot breach aim and shell arcs: geometry with no server state.

/// Sectors round a `bas` base the breach stands are spread over.
pub(super) const BREACH_SECTORS: usize = 16;

/// How far off a breach stand may lie, and the distance it is picked
/// nearest to.
pub(super) const BREACH_STAND_FAR: f32 = 1000.0;

pub(super) const BREACH_STAND_IDEAL: f32 = 600.0;

/// Script models and vehicles this near a base are in its breach traces:
/// anywhere between it and a stand. A rocket from mp_italy's stand by the
/// allied spawn struck the Sherman parked 190 units on, 1290 from the
/// base, and killed its shooter.
pub(super) const BREACH_MODEL_NEAR: f32 = crate::bots::BREACH_RANGE + 64.0;

/// Where a rocket from `muzzle` is aimed at a base whose script model
/// stands at `base`, from [`crate::bots::BREACH_MIN`] to
/// [`crate::bots::BREACH_RANGE`] off: a point on the model's mesh the
/// rocket strikes first (`G_MissileImpact`'s direct `G_Damage`), or one
/// whose burst on the world lands within [`crate::bots::BREACH_NEAR`] of
/// the origin (the splash). Tried up the bunker, then at its foot.
pub(super) fn breach_aim(
    world: &vcod_common::collision::CollisionWorld,
    models: &[crate::game::combat::PlacedModel],
    muzzle: glam::Vec3,
    base: glam::Vec3,
) -> Option<[f32; 3]> {
    use crate::bots::{BREACH_MIN, BREACH_NEAR, BREACH_RANGE};
    let d = muzzle.distance(base);
    if !(BREACH_MIN..=BREACH_RANGE).contains(&d) {
        return None;
    }
    [40.0, 72.0, 16.0, 0.0].into_iter().find_map(|up| {
        let spot = base + glam::Vec3::Z * up;
        let dir = (spot - muzzle).normalize_or_zero();
        let ok = breach_line(world, models, muzzle, spot + dir * BREACH_NEAR, base)
            && cone_clear(
                world,
                models,
                muzzle,
                dir,
                BREACH_SPREAD_DEG,
                crate::bots::BREACH_CLEAR,
            );
        ok.then_some(spot.into())
    })
}

/// Whether a round from `muzzle` toward `end` strikes the mesh of the
/// base at `base` or bursts on the world within
/// [`crate::bots::BREACH_NEAR`] of it, past [`crate::bots::BREACH_CLEAR`]:
/// the base's mesh reaches out toward the shooter, and a strike on it
/// inside the rocket's own splash took the shooter too.
fn breach_line(
    world: &vcod_common::collision::CollisionWorld,
    models: &[crate::game::combat::PlacedModel],
    muzzle: glam::Vec3,
    end: glam::Vec3,
    base: glam::Vec3,
) -> bool {
    use crate::bots::{BREACH_CLEAR, BREACH_NEAR};
    let tr = world.missile_trace(muzzle, end);
    let mut best = (tr.fraction, None);
    for m in models {
        if let Some((f, ..)) = m.clip(
            muzzle,
            end,
            crate::game::missile::MISSILE_MODEL_MASK,
            best.0,
        ) {
            best = (f, Some(m.origin));
        }
    }
    let burst = muzzle + (end - muzzle) * best.0;
    burst.distance(muzzle) >= BREACH_CLEAR
        && match best.1 {
            Some(o) => o == base,
            None => best.0 < 1.0 && burst.distance(base) < BREACH_NEAR,
        }
}

/// Where a tank's shell from `muzzle` at `speed` is aimed at a base whose
/// script model stands at `base`, [`crate::bots::BREACH_MIN`] to
/// [`SHELL_RANGE`] off, as [`breach_aim`] finds a rocket's: a shell has no
/// spread to clear, but it falls (`TR_GRAVITY`), so the aim is lifted by
/// the fall over the flight and the arc is traced ([`shell_lands`]). `hull`
/// is the tank, which its own shell passes.
pub(super) fn shell_aim(
    world: &vcod_common::collision::CollisionWorld,
    models: &[crate::game::combat::PlacedModel],
    hull: vcod_gsc::EntId,
    muzzle: glam::Vec3,
    speed: f32,
    base: glam::Vec3,
) -> Option<[f32; 3]> {
    let d = muzzle.distance(base);
    if !(crate::bots::BREACH_MIN..=SHELL_RANGE).contains(&d) || speed <= 0.0 {
        return None;
    }
    [40.0, 72.0, 16.0, 0.0].into_iter().find_map(|up| {
        let spot = base + glam::Vec3::Z * up;
        let t = spot.distance(muzzle) / speed;
        let aim =
            spot + glam::Vec3::Z * (0.5 * vcod_common::net::trajectory::DEFAULT_GRAVITY * t * t);
        let dir = (aim - muzzle).normalize_or_zero();
        shell_lands(world, models, hull, muzzle, dir, speed, base).then_some(aim.into())
    })
}

/// Whether a shell from `muzzle` along `dir` at `speed` strikes the base
/// at `base` as [`breach_line`] takes it ([`shell_strike`]). A straight
/// line grazed the rise in front of mp_foy's western axis bunker, where
/// every shell from the south fell into it 345 units short.
pub(super) fn shell_lands(
    world: &vcod_common::collision::CollisionWorld,
    models: &[crate::game::combat::PlacedModel],
    hull: vcod_gsc::EntId,
    muzzle: glam::Vec3,
    dir: glam::Vec3,
    speed: f32,
    base: glam::Vec3,
) -> bool {
    let reach = muzzle.distance(base) + crate::bots::BREACH_NEAR;
    let Some((burst, model)) = shell_strike(world, models, hull, muzzle, dir, speed, reach) else {
        return false;
    };
    burst.distance(muzzle) >= crate::bots::BREACH_CLEAR
        && match model {
            Some(m) => m.origin == base,
            None => burst.distance(base) < crate::bots::BREACH_NEAR,
        }
}

/// Where a shell from `muzzle` along `dir` at `speed`, falling at
/// `DEFAULT_GRAVITY`, first strikes the world or one of `models` but its
/// own `hull` within `reach` units of flight, and the model it strikes;
/// `None` when it strikes nothing. Traced along its arc in
/// [`SHELL_ARC_STEPS`] chords.
pub(super) fn shell_strike<'m>(
    world: &vcod_common::collision::CollisionWorld,
    models: &'m [crate::game::combat::PlacedModel],
    hull: vcod_gsc::EntId,
    muzzle: glam::Vec3,
    dir: glam::Vec3,
    speed: f32,
    reach: f32,
) -> Option<(glam::Vec3, Option<&'m crate::game::combat::PlacedModel>)> {
    let flight = reach / speed;
    let at = |i: usize| {
        let t = flight * i as f32 / SHELL_ARC_STEPS as f32;
        muzzle + dir * speed * t
            - glam::Vec3::Z * (0.5 * vcod_common::net::trajectory::DEFAULT_GRAVITY * t * t)
    };
    for i in 0..SHELL_ARC_STEPS {
        let (a, b) = (at(i), at(i + 1));
        let tr = world.missile_trace(a, b);
        let mut best = (tr.fraction, None);
        for m in models.iter().filter(|m| m.id != hull) {
            if let Some((f, ..)) = m.clip(a, b, crate::game::missile::MISSILE_MODEL_MASK, best.0) {
                best = (f, Some(m));
            }
        }
        if best.0 < 1.0 {
            return Some((a + (b - a) * best.0, best.1));
        }
    }
    None
}

/// Chords a shell's arc is traced in: over 2000 units it falls 16, so
/// each chord sags under half a unit.
const SHELL_ARC_STEPS: usize = 8;

/// How far off a tank shells a base: inside it a shell at 10000 units/s
/// falls under 16 units on the way.
const SHELL_RANGE: f32 = 2000.0;

/// Per group, the index of its first item `pass` takes, as a search of
/// each group in order would find it, on up to [`BREACH_THREADS`] threads:
/// [`Server::breach_stands`]' sectors. The items go out by their place in
/// their group, every group's first, then every group's second, and one
/// behind a pass its group already found is skipped. One sector after
/// another the traces held a `bas` tick 25 to 30 ms (map-cycle doc 4.9).
pub(super) fn first_passes<T: Sync>(
    groups: &[&[T]],
    pass: impl Fn(&T) -> bool + Sync,
) -> Vec<Option<usize>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut order: Vec<(usize, usize)> = groups
        .iter()
        .enumerate()
        .flat_map(|(g, items)| (0..items.len()).map(move |i| (i, g)))
        .collect();
    order.sort_unstable();
    let found: Vec<AtomicUsize> = groups
        .iter()
        .map(|_| AtomicUsize::new(usize::MAX))
        .collect();
    let next = AtomicUsize::new(0);
    let work = || loop {
        let Some(&(i, g)) = order.get(next.fetch_add(1, Ordering::Relaxed)) else {
            return;
        };
        if found[g].load(Ordering::Relaxed) > i && pass(&groups[g][i]) {
            found[g].fetch_min(i, Ordering::Relaxed);
        }
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(BREACH_THREADS)
        .min(order.len());
    if threads <= 1 {
        work();
    } else {
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(work);
            }
        });
    }
    found
        .into_iter()
        .map(|f| Some(f.into_inner()).filter(|&i| i != usize::MAX))
        .collect()
}

/// The most threads [`first_passes`] runs.
const BREACH_THREADS: usize = 8;

/// The loaded rocket launcher a bot carries ([`crate::bots::BotView::rocket`]):
/// the one in `hand` if loaded, else the first loaded one in `loaded`
/// (the held weapons with a round left).
pub(super) fn held_rocket(hand: u8, loaded: u64, launcher: impl Fn(u8) -> bool) -> Option<u8> {
    let ok = |w: u8| w < 64 && loaded >> w & 1 == 1 && launcher(w);
    Some(hand)
        .filter(|&w| ok(w))
        .or_else(|| (1u8..64).find(|&w| ok(w)))
}

/// The launchers' sighted spread standing (`adsSpread`: 4 for the
/// bazooka and the panzerschreck, 3.5 for the panzerfaust).
const BREACH_SPREAD_DEG: f32 = 4.0;

/// Whether rockets along `dir` and the edge of a `spread_deg` cone round
/// it meet nothing within `reach` of the muzzle. A breach stand checks the
/// sighted cone out to [`crate::bots::BREACH_CLEAR`]: the line's middle
/// alone left a bot now and then killed by a rocket the spread carried
/// into the cover by its stand. Only the near part of the cone counts;
/// where the rest bursts the base's checks already rule.
pub(crate) fn cone_clear(
    world: &vcod_common::collision::CollisionWorld,
    models: &[crate::game::combat::PlacedModel],
    muzzle: glam::Vec3,
    dir: glam::Vec3,
    spread_deg: f32,
    reach: f32,
) -> bool {
    let right = dir.cross(glam::Vec3::Z).normalize_or(glam::Vec3::X);
    let up = right.cross(dir);
    let r = spread_deg.to_radians().tan();
    (0..9).all(|k| {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let edge = if k == 8 {
            glam::Vec3::ZERO
        } else {
            (right * a.cos() + up * a.sin()) * r
        };
        let ray = (dir + edge).normalize();
        let end = muzzle + ray * reach;
        world.missile_trace(muzzle, end).fraction >= 1.0
            && models.iter().all(|m| {
                m.clip(muzzle, end, crate::game::missile::MISSILE_MODEL_MASK, 1.0)
                    .is_none()
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell falls: one laid level grazes a lip in front of the base
    /// and bursts on it 400 units short, while the aim lifted by the fall
    /// clears the lip and lands on the wall by the base (mp_foy's western
    /// axis bunker from the south, bot-objectives.md 7.2).
    #[test]
    fn a_shell_aim_clears_a_lip_its_fall_would_strike() {
        use glam::Vec3;
        let world = vcod_common::collision::test_world(&[
            (Vec3::new(500.0, -64.0, 0.0), Vec3::new(520.0, 64.0, 36.0)),
            (Vec3::new(900.0, -64.0, 0.0), Vec3::new(920.0, 64.0, 200.0)),
        ]);
        let hull = vcod_gsc::EntId(0, 0);
        let (muzzle, base) = (Vec3::new(-900.0, 0.0, 40.0), Vec3::new(900.0, 0.0, 0.0));
        assert!(!super::shell_lands(
            &world,
            &[],
            hull,
            muzzle,
            Vec3::X,
            10000.0,
            base
        ));
        let aim = super::shell_aim(&world, &[], hull, muzzle, 10000.0, base).expect("a shot");
        assert!(aim[2] > 40.0, "{aim:?}");
        let dir = (Vec3::from(aim) - muzzle).normalize();
        assert!(super::shell_lands(
            &world,
            &[],
            hull,
            muzzle,
            dir,
            10000.0,
            base
        ));
    }

    /// The threaded search finds each group's first pass, as one group
    /// after another searched in order would, and nothing for a group with
    /// none.
    #[test]
    fn first_passes_finds_what_a_serial_search_finds() {
        let groups: Vec<Vec<u32>> = (0..16u32)
            .map(|g| (0..(g * 7 % 40)).map(|i| (g * 131 + i * 17) % 97).collect())
            .collect();
        let pass = |v: &u32| v % 13 == 5;
        let views: Vec<&[u32]> = groups.iter().map(|g| &g[..]).collect();
        let serial: Vec<Option<usize>> = groups.iter().map(|g| g.iter().position(pass)).collect();
        assert!(serial.iter().any(Option::is_none) && serial.iter().any(Option::is_some));
        assert_eq!(first_passes(&views, pass), serial);
    }

    /// A loaded launcher counts whatever else is carried: an empty bazooka
    /// beside a loaded panzerschreck, the pistol in hand.
    #[test]
    fn a_loaded_launcher_beside_an_empty_one_is_the_rocket() {
        let launcher = |w: u8| w == 5 || w == 43;
        let loaded = (1 << 9) | (1 << 43);
        assert_eq!(held_rocket(9, loaded, launcher), Some(43));
        assert_eq!(held_rocket(5, loaded, launcher), Some(43));
        assert_eq!(held_rocket(43, loaded | (1 << 5), launcher), Some(43));
        assert_eq!(held_rocket(9, 1 << 9, launcher), None);
    }

    /// A launcher's widest hip cone (12 degrees) out to its 256 splash and
    /// the margin (`Server::armor_view`): a wall 200 ahead of the muzzle
    /// fails it, one 400 ahead does not, and neither does the floor under
    /// a standing body's level view.
    #[test]
    fn a_wall_inside_the_splash_by_the_muzzle_closes_the_cone() {
        let muzzle = glam::Vec3::new(0.0, 0.0, 64.0);
        let reach = 256.0 + crate::bots::ROCKET_CLEAR_MARGIN;
        let wall = |x: f32| {
            vcod_common::collision::test_world(&[(
                glam::Vec3::new(x, -512.0, 0.0),
                glam::Vec3::new(x + 16.0, 512.0, 512.0),
            )])
        };
        let clear = |w: &vcod_common::collision::CollisionWorld| {
            cone_clear(w, &[], muzzle, glam::Vec3::X, 12.0, reach)
        };
        assert!(!clear(&wall(200.0)));
        assert!(clear(&wall(400.0)));
        // A wall beside the line, inside the cone's edge.
        let beside = vcod_common::collision::test_world(&[(
            glam::Vec3::new(100.0, 40.0, 0.0),
            glam::Vec3::new(300.0, 56.0, 512.0),
        )]);
        assert!(!clear(&beside));
        assert!(cone_clear(&beside, &[], muzzle, glam::Vec3::X, 4.0, reach));
    }
}
