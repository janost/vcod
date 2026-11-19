//! What CoD2's and CoD4's `Player_GetUseList` share: the area box, the
//! reach and weights, the score off an entry's middle, `compare_use`'s
//! sort and the list's two cuts. CoD2 reads its constants at lnxded rodata
//! 0x815ee4c..0x815ee64 (cod2-gsc-diff.md 5.3), CoD4 at `cod4_lnxded`
//! rodata 0x820e278, 0x8214ac8 and 0x8220950 (cod4-gsc.md 9.1). Each game
//! sums the score's terms in its own order.

use glam::Vec3;

/// `+activate` (0x8) and `+usereload` (0x20): `useButtonPressed`'s bits.
pub const USE_BUTTONS: i32 = 0x28;
/// The `CM_AreaEntities` box about the eye; its contents are
/// [`crate::game::trigger::CONTENTS_USE`].
pub const AREA_BOX: Vec3 = Vec3::new(192.0, 192.0, 96.0);
/// The reach from the eye to an entry's middle.
pub const REACH: f32 = 128.0;
/// The view-angle weight; a `trigger_use` scores this much under its
/// angle and distance, a touch trigger this much under nothing.
pub const ANGLE_WEIGHT: f32 = 256.0;
/// What an ungrabbable item or a blocked sight adds before its cut.
pub const PENALTY: f32 = 10000.0;
/// The sight trace's mask (`G_TraceCapsuleComplete(.., 0x11)`).
pub const SIGHT_MASK: u32 = 0x11;

/// A use-list entry's score, which the sort and the cuts read and the
/// blocked sight raises.
pub trait Scored: Clone {
    fn score(&self) -> f32;
    fn score_mut(&mut self) -> &mut f32;
}

/// An entry's view-angle term `((dot + 1) * -0.5 + 1) * 256` and its
/// distance off its middle `centre`, `None` past `reach`.
pub fn score(eye: Vec3, forward: Vec3, centre: Vec3, reach: f32) -> Option<(f32, f32)> {
    let to = centre - eye;
    let dist = to.length();
    if dist > reach {
        return None;
    }
    let dir = if dist > 0.0 { to / dist } else { Vec3::ZERO };
    Some((((dir.dot(forward) + 1.0) * -0.5 + 1.0) * ANGLE_WEIGHT, dist))
}

/// `compare_use` (CoD2 Mac 0x1cc8e2, `cod4_lnxded` 0x80d2c10): the score
/// difference truncated to an int, under glibc's merge sort.
pub fn sort<T: Scored>(list: &mut [T]) {
    crate::game::item::msort(list, |a, b| (a.score() - b.score()) as i32 <= 0);
}

/// The list's two cuts: sorted, and the `cut` entries the penalty pushed
/// last dropped; then each entry whose sight trace `blocked` reports cut
/// short takes the penalty too, and the list is sorted and cut again.
pub fn sort_and_cut<T: Scored>(list: &mut Vec<T>, cut: usize, mut blocked: impl FnMut(&T) -> bool) {
    sort(list);
    list.truncate(list.len() - cut);
    let mut n = 0;
    for e in list.iter_mut() {
        if blocked(e) {
            *e.score_mut() += PENALTY;
            n += 1;
        }
    }
    sort(list);
    list.truncate(list.len() - n);
}
