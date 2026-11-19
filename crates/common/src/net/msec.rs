//! [`Msec`]: a wire or level clock in milliseconds.
//!
//! Retail's clocks (`usercmd_t.serverTime`, `snapshot_t.serverTime`,
//! `level.time`, `svs.time`) are C `int`s on x86, and their sums and
//! differences wrap. A bare `i32` traps on overflow in vcod's dev profile
//! and wraps in release; `Msec` wraps in both. Durations and constants stay
//! plain `i32`.
//!
//! Two kinds of compare, picked per site as retail picks them:
//! - `<`, `>=` (the derived `Ord`): the plain signed `cmp` retail makes on
//!   two clocks. Not wrap-aware. `SV_UserMove`'s skip of a cmd at or below
//!   `lastUsercmd.serverTime` (`cod_lnxded` 1.1d `jle` at 0x8087286;
//!   `cod11-combat.md` 3.6), and the cgame's
//!   `snap.serverTime <= cg.time < nextSnap.serverTime` walk
//!   (`CG_ProcessSnapshots`, `cod2-protocol.md` 18.5).
//! - `-` between two clocks, [`Msec::since`]: the `int` difference, which
//!   wraps, then compared to a duration or divided. `CG_SetFrameInterpolation`
//!   (`cod2-protocol.md` 18.5) and `G_Spawn`'s `level.time - freetime > 499`
//!   (CoD2 0x811e9ad, `cod2-protocol.md` 15.1). Wrap-aware within half the
//!   clock's range.

use std::ops::{Add, AddAssign, Sub, SubAssign};

/// A clock in ms as retail's `int` holds it. Arithmetic wraps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Msec(i32);

impl Msec {
    pub const ZERO: Msec = Msec(0);
    pub const MIN: Msec = Msec(i32::MIN);
    pub const MAX: Msec = Msec(i32::MAX);

    /// The clock a raw wire word or legacy `i32` holds.
    pub const fn from_raw(ms: i32) -> Msec {
        Msec(ms)
    }

    /// The raw `int`, as the wire carries it.
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// `self - earlier` as retail's `int` subtraction: wraps, so a clock
    /// just past `i32::MAX` is still a few ms after one just below it.
    pub const fn since(self, earlier: Msec) -> i32 {
        self.0.wrapping_sub(earlier.0)
    }
}

/// The raw ms, as a capture or log line prints the `int`.
impl std::fmt::Display for Msec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Add<i32> for Msec {
    type Output = Msec;
    fn add(self, ms: i32) -> Msec {
        Msec(self.0.wrapping_add(ms))
    }
}

impl Sub<i32> for Msec {
    type Output = Msec;
    fn sub(self, ms: i32) -> Msec {
        Msec(self.0.wrapping_sub(ms))
    }
}

/// The wrapping difference, [`Msec::since`].
impl Sub for Msec {
    type Output = i32;
    fn sub(self, earlier: Msec) -> i32 {
        self.since(earlier)
    }
}

impl AddAssign<i32> for Msec {
    fn add_assign(&mut self, ms: i32) {
        *self = *self + ms;
    }
}

impl SubAssign<i32> for Msec {
    fn sub_assign(&mut self, ms: i32) {
        *self = *self - ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_wraps_as_a_c_int() {
        assert_eq!(Msec::MAX + 1, Msec::MIN);
        assert_eq!(Msec::MIN - 1, Msec::MAX);
        let mut t = Msec::MAX;
        t += 10;
        assert_eq!(t.raw(), i32::MIN + 9);
        t -= 10;
        assert_eq!(t, Msec::MAX);
        assert_eq!(Msec::MIN - Msec::MAX, 1);
        assert_eq!(Msec::MAX - Msec::MIN, -1);
    }

    #[test]
    fn since_is_wrap_aware_and_ord_is_not() {
        let before = Msec::MAX - 5;
        let after = before + 10;
        assert_eq!(after.since(before), 10);
        // The plain signed compare retail's `jg`/`jle` make reads the
        // wrapped clock as the earlier one.
        assert!(after < before);
    }
}
