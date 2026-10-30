//! Load-time helpers: a stage stopwatch whose one log line says where a
//! map load spent its time, the load profile that puts those lines on one
//! timeline from a connect to the first frame in game, and an
//! order-keeping parallel map for the independent decodes inside one.

use std::sync::Mutex;
use std::time::Instant;

/// The load running ([`begin_load`] to [`end_load`]), if any.
static LOAD: Mutex<Option<Load>> = Mutex::new(None);

struct Load {
    what: String,
    start: Instant,
    /// (from ms, to ms, text) off `start`, in the order they closed.
    rows: Vec<(f64, f64, String)>,
}

fn ms_since(t0: Instant, t: Instant) -> f64 {
    t.saturating_duration_since(t0).as_secs_f64() * 1000.0
}

/// Starts the load profile: a connect, a map change or a client start
/// with `--connect`. A load already running is dropped unreported.
pub fn begin_load(what: impl Into<String>) {
    begin_load_at(what, Instant::now());
}

/// [`begin_load`] from `start`, an instant already past.
pub fn begin_load_at(what: impl Into<String>, start: Instant) {
    *LOAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(Load {
        what: what.into(),
        start,
        rows: Vec::new(),
    });
}

/// True while a load profile runs.
pub fn loading() -> bool {
    LOAD.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// A step of the running load that began at `from` and ends now, as
/// `text`; nothing without one.
pub fn span(from: Instant, text: impl FnOnce() -> String) {
    let now = Instant::now();
    if let Some(l) = LOAD.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let row = (ms_since(l.start, from), ms_since(l.start, now), text());
        l.rows.push(row);
    }
}

/// A moment of the running load (the gamestate, the first snapshot).
pub fn mark(text: impl FnOnce() -> String) {
    span(Instant::now(), text);
}

/// Ends the running load at its first frame in game: logs the timeline
/// at info as `load profile <what>: <ms> ms to the first frame` and one
/// line a step, and with `VCOD_LOAD_PROFILE=<path>` appends the same
/// lines to that file. Returns the lines; nothing without a load
/// running.
pub fn end_load() -> Option<String> {
    let l = LOAD.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    let total = ms_since(l.start, Instant::now());
    let mut text = format!(
        "load profile {}: {total:.0} ms to the first frame\n",
        l.what
    );
    let mut rows = l.rows;
    rows.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    for (from, to, what) in rows {
        if to - from < 0.5 {
            text.push_str(&format!("  {to:7.0}            {what}\n"));
        } else {
            text.push_str(&format!("  {from:7.0} {:7.0} ms  {what}\n", to - from));
        }
    }
    log::info!("{}", text.trim_end());
    if let Some(path) = std::env::var_os("VCOD_LOAD_PROFILE") {
        use std::io::Write;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(text.as_bytes()));
    }
    Some(text)
}

/// Named stages of one load, each timed from the end of the last.
/// [`Laps::finish`] logs them on one `load` line at info.
pub struct Laps {
    what: String,
    start: Instant,
    last: Instant,
    rows: Vec<(&'static str, f64)>,
}

impl Laps {
    pub fn new(what: impl Into<String>) -> Self {
        let now = Instant::now();
        Laps {
            what: what.into(),
            start: now,
            last: now,
            rows: Vec::new(),
        }
    }

    /// Closes the stage running since the last lap as `name`.
    pub fn lap(&mut self, name: &'static str) {
        let now = Instant::now();
        self.rows
            .push((name, now.duration_since(self.last).as_secs_f64() * 1000.0));
        self.last = now;
    }

    /// Milliseconds since [`Laps::new`].
    pub fn total_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }

    /// Logs `load <what>: <total> ms (<stage> <ms>, ...)`.
    pub fn finish(self) {
        let stages: Vec<String> = self
            .rows
            .iter()
            .map(|(n, ms)| format!("{n} {ms:.1}"))
            .collect();
        log::info!(
            "load {}: {:.1} ms ({})",
            self.what,
            self.total_ms(),
            stages.join(", ")
        );
        span(self.start, || {
            format!("{} ({})", self.what, stages.join(", "))
        });
    }
}

/// `f` over `items` on the machine's cores, results in `items` order.
/// Items are handed out one at a time, so a slow one does not hold up a
/// whole chunk.
pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len());
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        for h in handles {
            match h.join() {
                Ok(done) => {
                    for (i, r) in done {
                        out[i] = Some(r);
                    }
                }
                Err(p) => std::panic::resume_unwind(p),
            }
        }
    });
    out.into_iter()
        .map(|r| r.expect("every item mapped"))
        .collect()
}

/// A scoped thread's result, its panic carried on into the caller.
pub fn joined<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_load_profile_puts_its_steps_on_one_timeline() {
        let t0 = Instant::now();
        begin_load_at("test", t0);
        mark(|| "gamestate".into());
        let mut laps = Laps::new("map");
        laps.lap("bsp");
        laps.finish();
        span(t0, || "handshake".into());
        let text = end_load().expect("a load ran");
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("load profile test: "), "{text}");
        // ordered by start: the handshake from 0 comes first
        assert!(lines[1].ends_with("handshake"), "{text}");
        assert!(
            text.contains("gamestate") && text.contains("map (bsp "),
            "{text}"
        );
        assert!(end_load().is_none() && !loading());
    }

    #[test]
    fn par_map_keeps_the_items_order() {
        let items: Vec<u32> = (0..1000).collect();
        assert_eq!(
            par_map(&items, |i| i * 2),
            items.iter().map(|i| i * 2).collect::<Vec<_>>()
        );
        assert!(par_map(&[] as &[u32], |i| *i).is_empty());
    }
}
