//! Per-phase wall time of a server tick, for finding a slow tick's cause:
//! [`enable`] on the thread that ticks, a [`phase`] guard around each part
//! (nested parts name themselves `outer/inner`, so an outer phase's time
//! includes its inner ones), then [`take`] after the tick for its phases
//! in first-finished order. The soak and `bot_match` print the breakdown of
//! every tick past their threshold (cod4-server-frame.md 9).
//!
//! Off (no thread enabled) a phase costs one relaxed load and a branch.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set once any thread enables a profile; never cleared.
static ON: AtomicBool = AtomicBool::new(false);

/// A phase's name, time and how many times it ran.
pub type PhaseTime = (&'static str, Duration, u32);

thread_local! {
    static PHASES: RefCell<Option<Vec<PhaseTime>>> = const { RefCell::new(None) };
}

/// Profiles this thread's ticks from now on.
pub fn enable() {
    PHASES.with(|p| *p.borrow_mut() = Some(Vec::new()));
    ON.store(true, Ordering::Relaxed);
}

/// The phases since the last call, in first-finished order: name, time,
/// how many times it ran. Empty on a thread never enabled.
pub fn take() -> Vec<PhaseTime> {
    PHASES.with(|p| {
        p.borrow_mut()
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    })
}

/// One line of `phases` at or past `min_ms`, longest first:
/// `name 12.3 ms (x2), ...`.
pub fn format(phases: &[PhaseTime], min_ms: f32) -> String {
    let mut v: Vec<_> = phases
        .iter()
        .filter(|p| p.1.as_secs_f32() * 1000.0 >= min_ms)
        .collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.1));
    v.iter()
        .map(|(n, d, k)| {
            let ms = d.as_secs_f32() * 1000.0;
            if *k > 1 {
                format!("{n} {ms:.1} ms (x{k})")
            } else {
                format!("{n} {ms:.1} ms")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// This thread's scheduler and fault counters (Linux's
/// `/proc/thread-self`): time on a CPU and waiting on a run queue, in ns
/// (`schedstat`), minor and major page faults (`stat`), and voluntary
/// context switches (`status`). A slow tick's wall time less run and
/// queued is time the thread slept: on a lock, a page fault waiting on
/// the address space's lock or a read, a file write. `None` off Linux.
#[derive(Clone, Copy)]
pub struct Sched {
    run_ns: u64,
    queued_ns: u64,
    minflt: u64,
    majflt: u64,
    nvcsw: u64,
}

pub fn sched() -> Option<Sched> {
    let s = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    let mut it = s.split_whitespace().map(|v| v.parse::<u64>().ok());
    let (run_ns, queued_ns) = (it.next()??, it.next()??);
    let stat = std::fs::read_to_string("/proc/thread-self/stat").ok()?;
    // Past the `(comm)`: field 3 is the state, 10 minflt, 12 majflt.
    let f: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let (minflt, majflt) = (f.get(7)?.parse().ok()?, f.get(9)?.parse().ok()?);
    let status = std::fs::read_to_string("/proc/thread-self/status").ok()?;
    let nvcsw = status
        .lines()
        .find_map(|l| l.strip_prefix("voluntary_ctxt_switches:"))?
        .trim()
        .parse()
        .ok()?;
    Some(Sched {
        run_ns,
        queued_ns,
        minflt,
        majflt,
        nvcsw,
    })
}

/// A tick's wall time split by [`sched`] readings before and after it.
#[derive(Clone, Copy, Debug)]
pub struct Split {
    /// On a CPU.
    pub run_ms: f32,
    /// Runnable, waiting for a CPU.
    pub queued_ms: f32,
    /// The rest of the wall time: asleep.
    pub asleep_ms: f32,
    pub minflt: u64,
    pub majflt: u64,
    /// Voluntary context switches: the times it slept.
    pub sleeps: u64,
}

impl Split {
    /// From the reading `before` the tick to now, over `wall_ms`.
    pub fn since(before: Option<Sched>, wall_ms: f32) -> Option<Split> {
        let (a, b) = (before?, sched()?);
        let run_ms = (b.run_ns - a.run_ns) as f32 / 1e6;
        let queued_ms = (b.queued_ns - a.queued_ns) as f32 / 1e6;
        Some(Split {
            run_ms,
            queued_ms,
            asleep_ms: (wall_ms - run_ms - queued_ms).max(0.0),
            minflt: b.minflt - a.minflt,
            majflt: b.majflt - a.majflt,
            sleeps: b.nvcsw - a.nvcsw,
        })
    }
}

impl std::fmt::Display for Split {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "run {:.1} ms, queued {:.1} ms, asleep {:.1} ms, faults {}/{}, sleeps {}",
            self.run_ms, self.queued_ms, self.asleep_ms, self.minflt, self.majflt, self.sleeps
        )
    }
}

/// [`Split::since`] as a line, `no schedstat` without one.
pub fn split(before: Option<Sched>, wall_ms: f32) -> String {
    Split::since(before, wall_ms).map_or_else(|| "no schedstat".into(), |s| s.to_string())
}

/// A sampler for what a slow tick waits on (Linux): while [`Watch::begin`]
/// is in force, another thread reads the ticking thread's kernel wait
/// channel (`/proc/self/task/<tid>/wchan`) every millisecond, and the
/// syscall every other thread of the process is in
/// (`/proc/self/task/*/syscall`), so a tick that slept names the kernel
/// function it slept in and what the rest of the process did meanwhile.
pub struct Watch {
    busy: std::sync::Arc<AtomicBool>,
    samples: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, u32>>>,
}

impl Watch {
    /// Watches the calling thread; `None` off Linux.
    pub fn start() -> Option<Watch> {
        let me = std::fs::read_link("/proc/thread-self").ok()?;
        let tid = me.file_name()?.to_str()?.to_string();
        let busy = std::sync::Arc::new(AtomicBool::new(false));
        let samples = std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
        let (b, out) = (busy.clone(), samples.clone());
        std::thread::Builder::new()
            .name("tick watch".into())
            .spawn(move || {
                let mine = std::fs::read_link("/proc/thread-self")
                    .ok()
                    .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
                loop {
                    std::thread::sleep(Duration::from_millis(1));
                    if !b.load(Ordering::Relaxed) {
                        continue;
                    }
                    let read = |t: &str, f: &str| {
                        std::fs::read_to_string(format!("/proc/self/task/{t}/{f}"))
                            .unwrap_or_default()
                    };
                    let wchan = read(&tid, "wchan");
                    if wchan.is_empty() || wchan == "0" {
                        continue;
                    }
                    let mut keys = vec![format!("tick: {wchan}")];
                    if let Ok(dir) = std::fs::read_dir("/proc/self/task") {
                        for t in dir.flatten() {
                            let t = t.file_name().to_string_lossy().into_owned();
                            if t == tid || Some(&t) == mine.as_ref() {
                                continue;
                            }
                            let sc = read(&t, "syscall");
                            let nr = sc.split_whitespace().next().unwrap_or("");
                            // Off a CPU in a wait, or running user code.
                            if matches!(
                                nr,
                                "" | "running" | "202" | "7" | "232" | "230" | "35" | "281" | "271"
                            ) {
                                continue;
                            }
                            let comm = read(&t, "comm");
                            keys.push(format!("other: {} syscall {nr}", comm.trim()));
                        }
                    }
                    let mut o = out.lock().unwrap();
                    for k in keys {
                        *o.entry(k).or_default() += 1;
                    }
                }
            })
            .ok()?;
        Some(Watch { busy, samples })
    }

    /// Starts a tick's sampling.
    pub fn begin(&self) {
        self.samples.lock().unwrap().clear();
        self.busy.store(true, Ordering::Relaxed);
    }

    /// Ends it: each sample (`tick: <wchan>`, `other: <thread> syscall
    /// <nr>`) and its count of milliseconds, most first.
    pub fn end(&self) -> String {
        self.busy.store(false, Ordering::Relaxed);
        let mut v: Vec<_> = std::mem::take(&mut *self.samples.lock().unwrap())
            .into_iter()
            .collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        v.iter()
            .map(|(k, n)| format!("{k} x{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Times its scope into `name` when this thread profiles.
pub struct Phase(Option<(&'static str, Instant)>);

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some((name, t0)) = self.0 {
            let d = t0.elapsed();
            PHASES.with(|p| {
                if let Some(v) = p.borrow_mut().as_mut() {
                    match v.iter_mut().find(|e| e.0 == name) {
                        Some(e) => {
                            e.1 += d;
                            e.2 += 1;
                        }
                        None => v.push((name, d, 1)),
                    }
                }
            });
        }
    }
}

/// `let _p = phase("frame/scripts");`
#[inline(always)]
pub fn phase(name: &'static str) -> Phase {
    if !ON.load(Ordering::Relaxed) {
        return Phase(None);
    }
    Phase(enabled().then(|| (name, Instant::now())))
}

#[cold]
fn enabled() -> bool {
    PHASES.with(|p| p.borrow().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_sum_by_name_in_first_finished_order() {
        enable();
        let _ = take();
        for _ in 0..2 {
            let _a = phase("a");
            let _b = phase("a/b");
        }
        {
            let _c = phase("c");
        }
        let got: Vec<_> = take().into_iter().map(|(n, _, k)| (n, k)).collect();
        assert_eq!(got, [("a/b", 2), ("a", 2), ("c", 1)]);
        assert!(take().is_empty());
    }
}
