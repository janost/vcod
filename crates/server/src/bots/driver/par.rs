//! The bots' pass on worker threads (docs/dev/server-tick.md, "The bots'
//! threads"): how many, and a map over per-bot work run on them under the
//! ticking thread's walk rules.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The most threads a pass runs on unless `VCOD_BOT_THREADS` says more:
/// a dozen bots' pass took as long on eight, its slowest bot's plan the
/// floor (bot-navigation.md section 3, "The pass on threads").
const MAX_THREADS: usize = 4;

thread_local! {
    /// [`with_threads`]'s count for this thread's passes.
    static FORCED: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The threads a bots' pass runs on, the ticking one among them: 1 runs it
/// all on the ticking thread, as before the pass had workers.
/// `VCOD_BOT_THREADS=<n>`, else the cores up to [`MAX_THREADS`]; never more
/// than the cores.
pub(crate) fn threads() -> usize {
    static ENV: OnceLock<usize> = OnceLock::new();
    FORCED.get().unwrap_or_else(|| {
        *ENV.get_or_init(|| {
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
            std::env::var("VCOD_BOT_THREADS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(MAX_THREADS)
                .clamp(1, cores)
        })
    })
}

/// Runs `f` with this thread's bots' passes on `n` threads (1: serial),
/// over `VCOD_BOT_THREADS`: the determinism gate's two runs.
pub fn with_threads<R>(n: usize, f: impl FnOnce() -> R) -> R {
    let was = FORCED.replace(Some(n.max(1)));
    let r = f();
    FORCED.set(was);
    r
}

/// `f` over `items` on up to [`threads`] threads, this one among them,
/// each under this thread's walk rules (`nav::WalkRules`); the results in
/// item order. One item, or one thread, runs here alone.
pub(crate) fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = threads();
    if n.min(items.len()) <= 1 {
        return items.iter().map(f).collect();
    }
    let rules = crate::nav::WalkRules::here();
    // Handed out one at a time: a bot that plans a long path or walks a
    // proof costs many times one that only looks.
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(items.len()));
    // Each result with its item's index: the workers finish in any order.
    let job = || {
        rules.enter(|| {
            let mut done = Vec::new();
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(i) else {
                    break;
                };
                done.push((i, f(item)));
            }
            lock(&out).extend(done);
        })
    };
    POOL.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_ref().is_none_or(|p| p.workers.len() != n - 1) {
            *p = Some(Pool::new(n - 1));
        }
        p.as_ref().expect("made above").run(&job);
    });
    let mut done: Vec<(usize, R)> = out.into_inner().unwrap_or_else(|e| e.into_inner());
    done.sort_unstable_by_key(|d| d.0);
    done.into_iter().map(|d| d.1).collect()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

thread_local! {
    /// This ticking thread's workers, made on its first pass on threads;
    /// [`map`] holds it, so a job never maps again.
    static POOL: RefCell<Option<Pool>> = const { RefCell::new(None) };
}

/// Wakes this thread's workers ahead of the pass's first [`map`], so they
/// are on a core by then: a worker woken by the job itself joined it a
/// third of a millisecond late, past most of the work. They spin until
/// [`rest`] or [`SPIN`].
pub(crate) fn wake() {
    if threads() > 1 {
        POOL.with_borrow(|p| p.iter().for_each(Pool::wake));
    }
}

/// Parks this thread's workers until the next [`wake`] or job.
pub(crate) fn rest() {
    POOL.with_borrow(|p| {
        p.iter()
            .for_each(|p| p.shared.awake.store(false, Ordering::Relaxed))
    });
}

/// How long a woken worker spins for a job before it parks.
const SPIN: std::time::Duration = std::time::Duration::from_millis(4);

/// A ticking thread's worker threads, parked between its passes: a pass's
/// worth of threads started afresh each tick cost more than the plans they
/// ran, each new thread's first traces filling its own scratch.
struct Pool {
    shared: Arc<PoolState>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

struct PoolState {
    /// Bumped per job: each worker runs each job once.
    epoch: AtomicU64,
    /// The job under way: the address of [`Pool::run`]'s reference to it,
    /// set before `epoch` moves and kept until the job is `closed` and
    /// `busy` is 0.
    job: AtomicPtr<()>,
    /// Workers in the job. A worker joins it by counting itself in, then
    /// finding it open; the job closes, then waits for the count to drain:
    /// one of the two sees the other's write, so a worker woken too late
    /// stays out of a job already gone.
    busy: AtomicUsize,
    closed: AtomicBool,
    /// Spin for the next job rather than park.
    awake: AtomicBool,
    panicked: AtomicBool,
    quit: AtomicBool,
}

type JobRef<'a> = &'a (dyn Fn() + Sync + 'a);

impl Pool {
    fn new(n: usize) -> Pool {
        let shared = Arc::new(PoolState {
            epoch: AtomicU64::new(0),
            job: AtomicPtr::new(std::ptr::null_mut()),
            busy: AtomicUsize::new(0),
            closed: AtomicBool::new(true),
            awake: AtomicBool::new(true),
            panicked: AtomicBool::new(false),
            quit: AtomicBool::new(false),
        });
        let workers = (0..n)
            .map(|_| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name("bot plans".into())
                    .spawn(move || shared.work())
                    .expect("start a bots' worker")
            })
            .collect();
        Pool { shared, workers }
    }

    fn wake(&self) {
        self.shared.awake.store(true, Ordering::Relaxed);
        self.workers.iter().for_each(|w| w.thread().unpark());
    }

    /// `job` on every worker and here at once; back when all have left it.
    fn run(&self, job: JobRef) {
        let s = &self.shared;
        s.job.store(
            std::ptr::from_ref(&job).cast_mut().cast(),
            Ordering::Release,
        );
        s.closed.store(false, Ordering::SeqCst);
        s.awake.store(true, Ordering::Relaxed);
        s.epoch.fetch_add(1, Ordering::Release);
        self.workers.iter().for_each(|w| w.thread().unpark());
        let here = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        // Every item is taken: what the workers in it have left is short.
        s.closed.store(true, Ordering::SeqCst);
        while s.busy.load(Ordering::SeqCst) > 0 {
            std::hint::spin_loop();
        }
        if let Err(e) = here {
            std::panic::resume_unwind(e);
        }
        assert!(
            !s.panicked.swap(false, Ordering::Relaxed),
            "a bots' worker panicked"
        );
    }
}

impl PoolState {
    /// A worker's life: each job once, spinning while awake, else parked,
    /// until the pool goes.
    fn work(&self) {
        let mut seen = 0;
        let mut since = std::time::Instant::now();
        loop {
            loop {
                if self.quit.load(Ordering::Relaxed) {
                    return;
                }
                if self.epoch.load(Ordering::Acquire) != seen {
                    break;
                }
                if self.awake.load(Ordering::Relaxed) && since.elapsed() < SPIN {
                    std::hint::spin_loop();
                } else {
                    std::thread::park();
                    since = std::time::Instant::now();
                }
            }
            seen = self.epoch.load(Ordering::Acquire);
            self.busy.fetch_add(1, Ordering::SeqCst);
            if !self.closed.load(Ordering::SeqCst) {
                // SAFETY: the job is open with this worker counted in:
                // `Pool::run` holds the reference `job` points at until
                // the count drains, which this worker's leaving comes
                // after.
                let job = unsafe { *self.job.load(Ordering::Acquire).cast::<JobRef>() };
                let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_ok();
                self.panicked.fetch_or(!ok, Ordering::Relaxed);
            }
            self.busy.fetch_sub(1, Ordering::SeqCst);
            since = std::time::Instant::now();
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        for w in self.workers.drain(..) {
            w.thread().unpark();
            let _ = w.join();
        }
    }
}
