//! The built graphs kept per map in memory, and the job that builds one
//! off the server's thread.

use super::*;

/// The graphs built so far this process, by map. A map cycle comes back
/// to a map without a second build, and the test gates that start a server
/// per test share one.
pub(super) type CacheKey = (
    String,
    MoveRules,
    Option<String>,
    usize,
    usize,
    usize,
    usize,
    Vec<[u32; 3]>,
    Vec<[u32; 3]>,
    Vec<bool>,
    Vec<u32>,
);

pub(super) struct Cache {
    graphs: Vec<(CacheKey, Arc<NavGraph>)>,
    /// The keys a thread is building now, outside the lock.
    building: Vec<CacheKey>,
}

static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(Cache {
    graphs: Vec::new(),
    building: Vec::new(),
});

/// Signalled each time a build under [`Cache::building`] ends.
static BUILT: std::sync::Condvar = std::sync::Condvar::new();

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The key carries the collision's counts and the spawn points too, so a
/// test world built without the static props is a different graph from the
/// real one, and which submodels clip: each gametype deletes the others'
/// brushes (`_gameobjects.gsc`), so one map's graph differs by gametype.
/// Where the movers, world copies and vehicles stand is in it as well: a
/// rebuild walks round them, and two servers on one map hold them apart,
/// so a graph keyed without them made a gate's result ride which test
/// built first. Fall heights other than CoD1's ([`with_falls`]) name
/// themselves in the map's part, so CoD1's keys stay as they were.
fn cache_key(
    map: &str,
    c: &CollisionWorld,
    world: &crate::world::World,
    rules: MoveRules,
) -> CacheKey {
    let mut map = map.to_ascii_lowercase();
    let f = falls();
    if f != FallHeights::COD1 {
        map.push_str(&format!(" (falls {} {})", f.min, f.max));
    }
    (
        map,
        rules,
        own_step().map(|s| s.key()),
        c.brushes.len(),
        c.tris.len(),
        c.model_tris.len(),
        world.hazards.len(),
        world
            .spawn_points
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect(),
        world
            .nav_cover
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect(),
        (0..c.model_count()).map(|m| c.model_linked(m)).collect(),
        c.placements(),
    )
}

/// The cached graph under `key`, else `build`'s, cached: the graph and
/// how it came (`built`, or `loaded` off disk). The build runs outside the
/// lock under a mark on its key: a second asker for the same key waits for
/// it, so two servers asking at once build it once, and one asking for
/// another key goes on. A build that panics or is cancelled
/// ([`NavJob`]'s drop) takes its mark with it, and a waiter builds.
fn cached(key: CacheKey, build: impl FnOnce() -> (NavGraph, &'static str)) -> Arc<NavGraph> {
    let mut c = cache();
    loop {
        if let Some((_, g)) = c.graphs.iter().find(|(k, _)| *k == key) {
            return g.clone();
        }
        if !c.building.contains(&key) {
            break;
        }
        c = BUILT.wait(c).unwrap_or_else(|e| e.into_inner());
    }
    c.building.push(key.clone());
    drop(c);
    struct Mark<'a>(&'a CacheKey);
    impl Drop for Mark<'_> {
        fn drop(&mut self) {
            cache().building.retain(|k| k != self.0);
            BUILT.notify_all();
        }
    }
    let mark = Mark(&key);
    let t = std::time::Instant::now();
    let (g, how) = build();
    let g = Arc::new(g);
    log::info!(
        "nav graph for {}: {} nodes, {} edges, spacing {}, {how} in {} ms",
        key.0,
        g.len(),
        g.edge_count(),
        g.spacing,
        t.elapsed().as_millis()
    );
    // A rebuild keys on where the vehicles stood, so it is seldom asked
    // for again; the oldest go once the cache holds CACHE_GRAPHS.
    let mut c = cache();
    if c.graphs.len() >= CACHE_GRAPHS {
        c.graphs.remove(0);
    }
    c.graphs.push((key.clone(), g.clone()));
    drop(c);
    drop(mark);
    g
}

/// The most graphs [`cached`] keeps.
const CACHE_GRAPHS: usize = 24;

/// Test hook: forgets every cached graph, so a gate can run a match as a
/// fresh process would after another match filled the cache.
#[doc(hidden)]
pub fn clear_cache() {
    cache().graphs.clear();
}

/// `map`'s graph under the rules in force ([`with_rules`]), built on
/// first use on the calling thread.
pub fn graph_for(map: &str, world: &crate::world::World) -> Arc<NavGraph> {
    let rules = RULES.get();
    cached(cache_key(map, &world.collision, world, rules), || {
        let g = NavGraph::build(
            &world.collision,
            &world.spawn_points,
            &world.nav_cover,
            &world.hazards,
            rules,
        );
        (g, "built")
    })
}

/// Where a mantle off a node lands: the body's feet at the node, facing the
/// yaw (degrees), and the feet where the climb leaves it standing, if the
/// wall ahead takes one. The CoD2 bots hand one in
/// (`crate::bots::cod2::mantle_probe`); CoD1 has no mantle.
pub type MantleProbe = Arc<dyn Fn(&CollisionWorld, Vec3, f32) -> Option<Vec3> + Send + Sync>;

/// A graph on its way: built on a thread of its own over a snapshot of the
/// collision, so the server keeps ticking through a build of a second or
/// more (bot-navigation.md, "Build times"). Dropped before it lands (a map
/// change), it cancels the build at the next pass ([`par_map`]).
pub struct NavJob {
    ready: Option<Arc<NavGraph>>,
    building: Option<std::thread::JoinHandle<Arc<NavGraph>>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for NavJob {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The unwind a cancelled build's thread ends in.
pub(super) struct Cancelled;

impl NavJob {
    /// The cached graph when there is one, else a build started over a copy
    /// of `world`'s collision as it stands now: the links and poses scripts
    /// change later never reach the build. The walks move by `rules` and
    /// judge drops by the calling thread's fall heights ([`with_falls`]). The
    /// build thread first looks for the graph on disk ([`set_disk_cache`]),
    /// keyed by the world's map file (`CollisionWorld::source`).
    pub fn start(map: &str, world: &crate::world::World, rules: MoveRules) -> NavJob {
        Self::start_with(map, world, rules, None, None)
    }

    /// [`Self::start`] for a graph with mantle edges off `mantle`, its
    /// bodies moving by `step` ([`NavGraph::build_with`]).
    pub fn start_with(
        map: &str,
        world: &crate::world::World,
        rules: MoveRules,
        mantle: Option<MantleProbe>,
        step: Option<Arc<dyn WalkStep>>,
    ) -> NavJob {
        Self::start_opts(map, world, rules, mantle, step, true)
    }

    /// [`Self::start_with`] for bots that read a leap's lip or not
    /// ([`NavGraph::build_opts`]).
    pub fn start_opts(
        map: &str,
        world: &crate::world::World,
        rules: MoveRules,
        mantle: Option<MantleProbe>,
        step: Option<Arc<dyn WalkStep>>,
        lip: bool,
    ) -> NavJob {
        let collision = world.collision.clone();
        for &m in &world.rubble {
            collision.set_model_linked(m, false);
        }
        let mut key = with_step(step.clone(), || cache_key(map, &collision, world, rules));
        if mantle.is_some() {
            key.0.push_str(" (mantles)");
        }
        if !lip {
            key.0.push_str(" (no lip)");
        }
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // A build in progress holds no lock; the job's thread waits on one
        // for the same key, never the tick.
        if let Some((_, g)) = cache().graphs.iter().find(|(k, _)| *k == key) {
            return NavJob {
                ready: Some(g.clone()),
                building: None,
                cancel,
            };
        }
        let falls = falls();
        let seeds = world.spawn_points.clone();
        let cover = world.nav_cover.clone();
        let hazards = world.hazards.clone();
        let cancelled = cancel.clone();
        let build = move || {
            // One core stays the tick's, so a build doesn't jitter the
            // schedule it runs beside.
            SPARE_CORE.set(true);
            CANCEL.set(Some(cancelled));
            FALLS.set(falls);
            let disk = disk::key(&key, collision.source());
            cached(key, || {
                if let Some(g) = disk
                    .as_ref()
                    .and_then(|d| disk::load(d, rules, step.clone()))
                {
                    return (g, "loaded");
                }
                let g = NavGraph::build_opts(
                    &collision,
                    &seeds,
                    &cover,
                    &hazards,
                    rules,
                    mantle.as_ref(),
                    step,
                    lip,
                );
                if let Some(d) = &disk {
                    disk::store(d, &g);
                }
                (g, "built")
            })
        };
        NavJob {
            ready: None,
            building: Some(std::thread::spawn(build)),
            cancel,
        }
    }

    /// The graph once it is built; `wait` blocks until it is. A build that
    /// panicked leaves the job empty for good, its bots on their own
    /// wander, unless `wait` asked for the graph.
    pub fn poll(&mut self, wait: bool) -> Option<Arc<NavGraph>> {
        if let Some(h) = self.building.take_if(|h| wait || h.is_finished()) {
            match h.join() {
                Ok(g) => self.ready = Some(g),
                Err(e) if wait => std::panic::resume_unwind(e),
                Err(_) => log::warn!("the nav graph build panicked: bots go without one"),
            }
        }
        self.ready.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::build::tests::map_world;
    use crate::nav::graph::tests::tiny;

    fn test_key(name: &str) -> CacheKey {
        let v = Vec::new;
        (
            name.into(),
            MoveRules::V1_1,
            None,
            0,
            0,
            0,
            0,
            v(),
            v(),
            Vec::new(),
            Vec::new(),
        )
    }

    /// A build that panics takes its key's mark with it: the next asker
    /// builds instead of waiting for good.
    #[test]
    fn a_panicked_build_leaves_its_key_free() {
        let key = test_key("panicked build");
        let k = key.clone();
        let failed = std::thread::spawn(move || cached(k, || panic!("bad geometry"))).join();
        assert!(failed.is_err());
        assert_eq!(cached(key, || (tiny(), "built")).len(), 5);
    }

    /// A build in progress holds no lock: another key is served meanwhile.
    #[test]
    fn a_build_in_progress_does_not_hold_other_keys() {
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let slow = std::thread::spawn(move || {
            cached(test_key("slow build"), move || {
                wait.recv().unwrap();
                (tiny(), "built")
            })
        });
        while !cache().building.contains(&test_key("slow build")) {
            std::thread::yield_now();
        }
        assert_eq!(
            cached(test_key("other build"), || (tiny(), "built")).len(),
            5
        );
        go.send(()).unwrap();
        assert_eq!(slow.join().unwrap().len(), 5);
    }

    /// A job whose build panicked polls empty, its bots on their own,
    /// instead of taking the tick down.
    #[test]
    fn a_panicked_job_polls_empty() {
        let mut job = NavJob {
            ready: None,
            building: Some(std::thread::spawn(|| -> Arc<NavGraph> {
                panic!("bad geometry")
            })),
            cancel: Default::default(),
        };
        while job.building.as_ref().is_some_and(|h| !h.is_finished()) {
            std::thread::yield_now();
        }
        assert!(job.poll(false).is_none());
        assert!(job.poll(false).is_none());
    }

    /// A dropped job's flag stops its build at the next pass.
    #[test]
    fn a_cancelled_build_unwinds_at_its_next_pass() {
        let job = NavJob {
            ready: None,
            building: None,
            cancel: Default::default(),
        };
        let flag = job.cancel.clone();
        drop(job);
        let ran = std::thread::spawn(move || {
            CANCEL.set(Some(flag));
            par_map(&[1, 2], |x| *x)
        })
        .join();
        assert!(ran.unwrap_err().is::<Cancelled>());
    }

    /// A rebuild walks round the vehicles, movers and world copies where
    /// they stand, so the cache keys on each: keyed without them, a bas
    /// gate got whichever graph a test beside it built first.
    #[test]
    fn the_cache_key_covers_every_placement() {
        let Some(world) = map_world("mp_carentan") else {
            return;
        };
        let c = &world.collision;
        let key = |c: &CollisionWorld| cache_key("mp_carentan", c, &world, MoveRules::V1_1);
        let bare = key(c);
        let at = |x: f32| Some((Vec3::new(x, 0.0, 0.0), Vec3::ZERO, 1));
        c.set_model_instance(300, 1, at(0.0));
        let parked = key(c);
        assert_ne!(parked, bare);
        c.set_model_instance(300, 1, at(64.0));
        assert_ne!(key(c), parked, "a vehicle that moved");
        c.set_model_instance(300, 1, None);
        assert_eq!(key(c), bare);
        c.set_model_pose(1, Vec3::new(0.0, 0.0, 512.0), Vec3::ZERO);
        assert_ne!(key(c), bare, "a mover off its spawn");
        c.reset_model_pose(1);
        assert_eq!(key(c), bare);
        c.set_world_copy(301, Some((Vec3::ZERO, Vec3::ZERO, 1)));
        assert_ne!(key(c), bare, "a world copy");
        c.set_world_copy(301, None);
        c.set_arith(vcod_common::patch::Arith::Stored);
        assert_ne!(key(c), bare, "another engine's arithmetic");
    }
}
