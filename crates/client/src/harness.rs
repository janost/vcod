//! The live tests' harness: a client thread and an in-process server
//! on one [`sim`] clock ([`run_held`], [`serve`]), and the slots that cap
//! how many live servers run at once ([`live_slot`]).

use std::time::{Duration, Instant};

/// How many live CoD4 servers this test binary runs at once: each holds
/// a loaded map, a client and its session, about 2.5 GB at the peak
/// (docs/dev/testing.md, "CoD4 live tests"). `VCOD_LIVE_SLOTS` sets it.
fn live_slots() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("VCOD_LIVE_SLOTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3)
            .max(1)
    })
}

static LIVE: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

thread_local! {
    /// This thread holds a slot: a test's second server rides its first's.
    static HOLDS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A slot of [`live_slots`], released on drop.
pub(crate) struct LiveSlot(bool);

/// Waits for a free slot before a test builds a live server, so a run
/// on every core holds at most [`live_slots`] maps whatever its
/// `--test-threads`. Taken before the sim clock starts: a wait inside
/// one would trip its stall asserts.
pub(crate) fn live_slot() -> LiveSlot {
    if HOLDS.get() {
        return LiveSlot(false);
    }
    let mut n = LIVE.0.lock().unwrap_or_else(|e| e.into_inner());
    while *n >= live_slots() {
        n = LIVE.1.wait(n).unwrap_or_else(|e| e.into_inner());
    }
    *n += 1;
    HOLDS.set(true);
    LiveSlot(true)
}

impl Drop for LiveSlot {
    fn drop(&mut self) {
        if self.0 {
            HOLDS.set(false);
            *LIVE.0.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
            LIVE.1.notify_one();
        }
    }
}

/// The clock a live test's client thread and its in-process server
/// share, so the test runs as fast as both compute instead of in real
/// time. Only the client moves it, with every sleep ([`clk_sleep`]),
/// and each sleep waits until [`serve`] has read every packet sent
/// before and run every server frame due by then; the session's own
/// clock reads it ([`clock_now`]), so the two run in lockstep however
/// slow a frame is or however long the client computes between
/// sleeps (an asset load). The join waits and times out on it too, a
/// frame a wait (`conn::set_thread_clock`), so a client wait that
/// does not sleep on it never ends: [`Clock::turn`] fails one that
/// leaves the clock still for [`STALL`].
pub(crate) mod sim {
    use std::cell::RefCell;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// The wall time the client may leave the clock still.
    pub(crate) const STALL: Duration = Duration::from_secs(600);

    #[derive(Clone)]
    pub(crate) struct Clock(Arc<(Mutex<State>, Condvar)>);

    struct State {
        /// The clock.
        now: Instant,
        /// What the server has run up to.
        served: Instant,
    }

    thread_local! {
        static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
    }

    impl Clock {
        pub(crate) fn new() -> Clock {
            let t = Instant::now();
            let st = State { now: t, served: t };
            Clock(Arc::new((Mutex::new(st), Condvar::new())))
        }

        pub(crate) fn now(&self) -> Instant {
            self.0.0.lock().unwrap().now
        }

        /// Moves the clock on by `d` and waits for the server to catch
        /// up to it.
        fn sleep(&self, d: Duration) {
            let (m, cv) = &*self.0;
            let mut st = m.lock().unwrap();
            st.now += d;
            cv.notify_all();
            let want = st.now;
            let real = Instant::now();
            while st.served < want {
                assert!(
                    real.elapsed() < Duration::from_secs(60),
                    "the server stopped serving the clock"
                );
                st = cv.wait_timeout(st, Duration::from_millis(100)).unwrap().0;
            }
        }

        /// The server's turn: waits for the client to move the clock,
        /// and returns the time to run up to, or `None` once
        /// `finished` says the client is done.
        pub(crate) fn turn(&self, finished: impl Fn() -> bool) -> Option<Instant> {
            let (m, cv) = &*self.0;
            let mut st = m.lock().unwrap();
            let real = Instant::now();
            while st.served >= st.now {
                if finished() {
                    return None;
                }
                assert!(
                    real.elapsed() < STALL,
                    "the client left the clock still for {STALL:?}: a wait that does not sleep on it"
                );
                st = cv.wait_timeout(st, Duration::from_millis(50)).unwrap().0;
            }
            Some(st.now)
        }

        /// The server has run up to `t`.
        pub(crate) fn served(&self, t: Instant) {
            let (m, cv) = &*self.0;
            let mut st = m.lock().unwrap();
            st.served = st.served.max(t);
            cv.notify_all();
        }
    }

    /// A client thread on `clock`.
    pub(crate) fn spawn<T: Send + 'static>(
        clock: &Clock,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> std::thread::JoinHandle<T> {
        let c = clock.clone();
        std::thread::spawn(move || {
            install(Some(c));
            f()
        })
    }

    /// Puts this thread on `clock` (`None`: back on the wall), the
    /// join's waits and deadlines too, a frame a wait as a client
    /// reads its packets once a frame.
    pub(crate) fn install(clock: Option<Clock>) {
        CLOCK.with(|x| *x.borrow_mut() = clock);
        vcod_cod4::net::conn::set_thread_clock(super::clk_now, |_| {
            super::clk_sleep(Duration::from_millis(16))
        });
    }

    /// This thread's clock, for a thread it starts to run on too.
    pub(crate) fn current() -> Option<Clock> {
        CLOCK.with(|c| c.borrow().clone())
    }

    /// This thread's clock, if it runs on one.
    pub(crate) fn now() -> Option<Instant> {
        CLOCK.with(|c| c.borrow().as_ref().map(Clock::now))
    }

    pub(crate) fn sleep(d: Duration) -> bool {
        CLOCK.with(|c| c.borrow().as_ref().map(|c| c.sleep(d)).is_some())
    }
}

/// Now on this thread's [`sim`] clock, else the wall clock.
pub(crate) fn clk_now() -> Instant {
    sim::now().unwrap_or_else(Instant::now)
}

/// Sleeps on this thread's [`sim`] clock, else the wall clock.
pub(crate) fn clk_sleep(d: Duration) {
    if !sim::sleep(d) {
        std::thread::sleep(d);
    }
}

/// [`Instant::elapsed`] on [`clk_now`].
pub(crate) fn since(t: Instant) -> Duration {
    clk_now().saturating_duration_since(t)
}

/// What [`serve`] runs a game's in-process server through.
pub(crate) trait Held {
    /// Ahead of a turn's packets.
    fn turn(&mut self) {}
    /// One server frame at `at`.
    fn frame(&mut self, at: Instant);
    fn frame_ms(&self) -> u64;
    fn handle_packet(&mut self, from: std::net::SocketAddr, pkt: &[u8], now: Instant);
    fn take_outgoing(&mut self) -> Vec<(std::net::SocketAddr, Vec<u8>)>;
}

impl Held for vcod_server::cod4::Cod4Server {
    fn turn(&mut self) {
        // The bots' graph lands on the frame that asks for it, not
        // when a build thread is done.
        if let Some(b) = self.bots_mut() {
            b.wait_for_graph();
        }
    }

    fn frame(&mut self, at: Instant) {
        self.tick_frames(at, 1);
    }

    fn frame_ms(&self) -> u64 {
        vcod_server::cod4::Cod4Server::frame_ms(self) as u64
    }

    fn handle_packet(&mut self, from: std::net::SocketAddr, pkt: &[u8], now: Instant) {
        vcod_server::cod4::Cod4Server::handle_packet(self, from, pkt, now);
    }

    fn take_outgoing(&mut self) -> Vec<(std::net::SocketAddr, Vec<u8>)> {
        vcod_server::cod4::Cod4Server::take_outgoing(self)
    }
}

impl Held for vcod_server::cod2::Cod2Server {
    fn turn(&mut self) {
        if let Some(b) = self.bots_mut() {
            b.wait_for_graph();
        }
    }

    /// The binary's frame: `Cbuf_Execute`, then `SV_Frame`.
    fn frame(&mut self, at: Instant) {
        self.drain_console(at);
        self.tick_frames(at, 1);
    }

    fn frame_ms(&self) -> u64 {
        vcod_server::cod2::FRAME_MS as u64
    }

    fn handle_packet(&mut self, from: std::net::SocketAddr, pkt: &[u8], now: Instant) {
        vcod_server::cod2::Cod2Server::handle_packet(self, from, pkt, now);
    }

    fn take_outgoing(&mut self) -> Vec<(std::net::SocketAddr, Vec<u8>)> {
        vcod_server::cod2::Cod2Server::take_outgoing(self)
    }
}

/// An in-process server's loop under `client`, on `clock`: each turn
/// reads the packets the client sent (stamped with the time it sent
/// them), runs every frame ([`Held::frame_ms`]) due by the clock and
/// sends what they queued. `tick` runs ahead of each frame, at its
/// time, between the packets' cmds and the frame (it may move the next
/// frame's time); once it returns false the server goes quiet for
/// good: nothing read, run or sent.
pub(crate) fn serve<S: Held, T>(
    sv: &mut S,
    sock: &std::net::UdpSocket,
    clock: &sim::Clock,
    client: &std::thread::JoinHandle<T>,
    tick: impl FnMut(&mut S, Instant, &mut Instant) -> bool,
) {
    serve_until(sv, sock, clock, || client.is_finished(), tick);
}

/// [`serve`] until `finished`.
pub(crate) fn serve_until<S: Held>(
    sv: &mut S,
    sock: &std::net::UdpSocket,
    clock: &sim::Clock,
    finished: impl Fn() -> bool,
    mut tick: impl FnMut(&mut S, Instant, &mut Instant) -> bool,
) {
    let mut buf = [0u8; 4096];
    let mut quiet = false;
    let mut next = clock.now();
    let mut heard = next;
    while let Some(until) = clock.turn(&finished) {
        sv.turn();
        while !quiet && let Ok((n, from)) = sock.recv_from(&mut buf) {
            sv.handle_packet(from, &buf[..n], heard);
        }
        while !quiet && next <= until {
            let at = next;
            if !tick(sv, at, &mut next) {
                quiet = true;
                break;
            }
            sv.frame(at);
            next += Duration::from_millis(sv.frame_ms());
            for (to, p) in sv.take_outgoing() {
                let _ = sock.send_to(&p, to);
            }
        }
        // The connectionless answers the packets queued.
        for (to, p) in sv.take_outgoing().into_iter().filter(|_| !quiet) {
            let _ = sock.send_to(&p, to);
        }
        heard = until;
        clock.served(until);
    }
}

/// `client` against `sv` on one fresh [`sim`] clock, through
/// [`serve`]: its frames, the server's frames and every packet between
/// them land on the same times run to run whatever the wall does.
/// `tick` is [`serve`]'s.
pub(crate) fn run_held<S: Held, T: Send + 'static>(
    sv: &mut S,
    client: impl FnOnce(std::net::SocketAddr) -> T + Send + 'static,
    tick: impl FnMut(&mut S, Instant, &mut Instant) -> bool,
) -> T {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let handle = sim::spawn(&clock, move || client(addr));
    serve(sv, &sock, &clock, &handle, tick);
    handle.join().unwrap()
}
