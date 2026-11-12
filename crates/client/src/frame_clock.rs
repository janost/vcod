//! The client's frame clock, retail's `cls.realtime`: the wall clock, except
//! that a `cl_avidemo` frame moves it a fixed step however long the frame
//! took to draw and write (`crate::screenshot::avidemo_step`). Behind the
//! wall clock (or ahead of it) once an avidemo has run; the gap stays when
//! it stops, so the clock never jumps. The main thread's frame, its demo
//! clocks and the CoD2 and CoD4 sessions read it.

use std::cell::Cell;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct State {
    /// Wall minus frame clock, in ns (negative when the clock runs ahead).
    skew_ns: i128,
    /// The frame's time while a fixed step holds it.
    held: Option<Instant>,
    /// The last frame's time.
    last: Option<Instant>,
}

thread_local! {
    static STATE: Cell<State> = const {
        Cell::new(State {
            skew_ns: 0,
            held: None,
            last: None,
        })
    };
}

fn shift(t: Instant, ns: i128) -> Instant {
    let d = Duration::from_nanos(ns.unsigned_abs() as u64);
    if ns >= 0 {
        t.checked_sub(d).unwrap_or(t)
    } else {
        t + d
    }
}

/// Now on the frame clock: the frame's own time inside an avidemo frame.
pub fn now() -> Instant {
    let s = STATE.with(Cell::get);
    s.held.unwrap_or_else(|| shift(Instant::now(), s.skew_ns))
}

/// Starts a frame at `wall`: the clock moves `step` past the last frame
/// under `cl_avidemo`, else with the wall. Returns the frame's time.
pub fn tick(wall: Instant, step: Option<Duration>) -> Instant {
    STATE.with(|c| {
        let mut s = c.get();
        let t = match (step, s.last) {
            (Some(d), Some(last)) => {
                let t = last + d;
                s.skew_ns = signed_ns(wall, t);
                s.held = Some(t);
                t
            }
            _ => {
                s.held = None;
                shift(wall, s.skew_ns)
            }
        };
        s.last = Some(t);
        c.set(s);
        t
    })
}

/// `a - b` in ns, signed.
fn signed_ns(a: Instant, b: Instant) -> i128 {
    match a.checked_duration_since(b) {
        Some(d) => d.as_nanos() as i128,
        None => -(b.duration_since(a).as_nanos() as i128),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_step_holds_the_frame_and_the_gap_stays_after() {
        let wall = Instant::now();
        assert_eq!(tick(wall, None), wall);
        // two slow avidemo frames: 33 ms each on the clock, 500 on the wall
        let ms = Duration::from_millis;
        assert_eq!(tick(wall + ms(500), Some(ms(33))), wall + ms(33));
        assert_eq!(now(), wall + ms(33), "held through the frame");
        assert_eq!(tick(wall + ms(1000), Some(ms(33))), wall + ms(66));
        // off again: the wall's steps from where the clock stood
        assert_eq!(tick(wall + ms(1010), None), wall + ms(76));
        assert!(now() < Instant::now());
    }
}
