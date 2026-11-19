//! The angle folds.

use super::*;

/// The 1.1 cgame passes `AngleNormalize180` the difference as a float
/// argument (cgame_mp_x86.dll 0x30007925-0x3000792b); 1.1d's
/// `AngleDelta` keeps it on the x87 stack (0x3ec9c). Here the kept
/// difference truncates to 1295 shorts and the float one rounds up to
/// 1296, so the Windows predictor reads a short more than the server.
#[test]
fn the_windows_1_1_predictor_stores_the_angle_delta() {
    let w = flat();
    let (a, b) = (-3.867_187_7_f32, -10.986_328_f32);
    let short = |n: f32| n * (360.0 / 65536.0);
    let delta =
        |w: &CollisionWorld| angle_delta(a, b, delta_stored(MoveRules::V1_1, &MoveWorld::bare(w)));
    assert_eq!(delta(&w), short(1295.0), "1.1d");
    w.set_arith(crate::patch::Arith::Msvc);
    assert_eq!(delta(&w), short(1296.0), "the 1.1 cgame");
}
