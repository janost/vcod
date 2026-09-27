//! Spectator follow: which client a spectator's view rides, how the buttons
//! move it, and where the spectator is left when it lets go
//! (docs/research/cod11-spectator-follow.md).

use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg;
use vcod_common::net::protocol::Protocol;

/// `pm_flags` on a frame copied from the followed client.
pub const PMF_FOLLOW: i32 = 0x10000;
/// `pm_flags` on a follow the script asked for (`spectatorclient >= 0`).
pub const PMF_FORCED_FOLLOW: i32 = 0x20000;
/// The own-view bit, which the copy clears.
const PMF_OWN_VIEW: i32 = 0x40000;
/// The one `eFlags` bit the spectator keeps from its own word across the copy.
const EF_KEPT_ACROSS_FOLLOW: i32 = 0x20000;

/// A client's `sessionstate`, the word `ClientEndFrame` branches on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionState {
    Playing,
    Dead,
    Spectator,
    Intermission,
}

/// What script left on a client that the follow reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Session {
    pub state: SessionState,
    /// `spectatorclient`, the forced follow; negative for none.
    pub spectator_client: i32,
    /// The script asked for a replay (`archivetime` above 0), which vcod has
    /// no archive to serve.
    pub killcam: bool,
}

impl Session {
    /// A client with no script entity: the state its sim is in, no forced
    /// follow.
    pub fn unscripted(spectator: bool) -> Self {
        Session {
            state: if spectator {
                SessionState::Spectator
            } else {
                SessionState::Playing
            },
            spectator_client: -1,
            killcam: false,
        }
    }
}

/// The follow state `gclient_t` keeps for a spectator.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Follow {
    /// `client+0x21d4`, the client being followed or cycled to.
    pub target: Option<usize>,
    /// `pm_flags` 0x10000 as the last end frame left it: the copy landed.
    pub on: bool,
    /// `pm_flags` 0x20000 as the last end frame left it.
    pub forced: bool,
    /// The followed client's eye and view angles at the last copy, which is
    /// what `StopFollowing` reads off the spectator's copied playerstate.
    pub view: Option<([f32; 3], [f32; 3])>,
}

/// `Cmd_FollowCycle_f` (`game.mp.i386.so` 0x4902c): step from the current
/// target (slot 0 when there is none) in `dir`, wrapping at `max_clients`,
/// and take the first slot `followable` accepts. The start slot is tried
/// last, so a lone candidate is found from anywhere.
pub fn cycle(
    current: Option<usize>,
    dir: i32,
    max_clients: usize,
    followable: impl Fn(usize) -> bool,
) -> Option<usize> {
    let n = max_clients as i32;
    if n == 0 {
        return None;
    }
    let start = current.map_or(0, |c| c as i32);
    let mut i = start;
    loop {
        i += dir;
        if i >= n {
            i = 0;
        }
        if i < 0 {
            i = n - 1;
        }
        if followable(i as usize) {
            return Some(i as usize);
        }
        if i == start {
            return None;
        }
    }
}

/// `StopFollowing`'s spot (`game.mp.i386.so` 0x46a28): a capsule of radius 8
/// swept from the followed eye 40 units back along the view and 10 up, and
/// the view pitched 15 degrees down. `view` is in degrees, wire convention.
pub fn stop_spot(
    collision: Option<&CollisionWorld>,
    eye: [f32; 3],
    view: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let axis = vcod_common::pmove::aim::angles_to_axis(view);
    let eye = Vec3::from(eye);
    let end = eye + Vec3::from(axis[0]) * -40.0 + Vec3::from(axis[2]) * 10.0;
    let spot = match collision {
        Some(w) => {
            w.box_trace(eye, end, Vec3::splat(-8.0), Vec3::splat(8.0))
                .endpos
        }
        None => end,
    };
    (spot.into(), [view[0] + 15.0, view[1], view[2]])
}

/// The flag patch `SpectatorClientEndFrame` puts on the copied playerstate
/// (0x408e0..0x40910): own view off, follow on, forced as asked, and
/// `eFlags` 0x20000 from the spectator's own word.
pub fn patch_wire(ps: &mut msg::PlayerState, p: &Protocol, forced: bool, own_eflags: i32) {
    let idx = |name| msg::PlayerState::field_index(p, name).unwrap();
    let (pm, ef) = (idx("pm_flags"), idx("eFlags"));
    ps.fields[pm] =
        (ps.fields[pm] & !PMF_OWN_VIEW) | PMF_FOLLOW | if forced { PMF_FORCED_FOLLOW } else { 0 };
    ps.fields[ef] = (ps.fields[ef] & !EF_KEPT_ACROSS_FOLLOW) | (own_eflags & EF_KEPT_ACROSS_FOLLOW);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retail run in the research doc: spectator in slot 2 of 8, players
    /// in 0 and 1. Attack from nothing lands on 1, attack again on 0, melee
    /// back on 1.
    #[test]
    fn the_cycle_walks_the_slots_the_retail_run_walked() {
        let playing = |s: usize| s < 2;
        let first = cycle(None, 1, 8, playing);
        assert_eq!(first, Some(1));
        let second = cycle(first, 1, 8, playing);
        assert_eq!(second, Some(0));
        assert_eq!(cycle(second, -1, 8, playing), Some(1));
    }

    #[test]
    fn the_start_slot_is_tried_last() {
        assert_eq!(cycle(Some(3), 1, 8, |s| s == 3), Some(3));
        assert_eq!(cycle(None, 1, 8, |s| s == 0), Some(0));
        assert_eq!(cycle(Some(3), -1, 8, |_| false), None);
    }

    #[test]
    fn the_stop_spot_backs_off_the_eye_and_pitches_down() {
        let (spot, view) = stop_spot(None, [720.0, -32.0, 52.1], [0.0, 0.0, 0.0]);
        assert_eq!(spot, [680.0, -32.0, 62.1]);
        assert_eq!(view, [15.0, 0.0, 0.0]);
    }

    #[test]
    fn a_wall_behind_shortens_the_back_off() {
        let wall = vcod_common::collision::test_world(&[(
            Vec3::new(600.0, -200.0, -100.0),
            Vec3::new(690.0, 200.0, 200.0),
        )]);
        let (spot, _) = stop_spot(Some(&wall), [720.0, -32.0, 52.1], [0.0, 0.0, 0.0]);
        assert!(spot[0] > 690.0 && spot[0] < 720.0, "{spot:?}");
    }

    #[test]
    fn the_patch_swaps_own_view_for_follow() {
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let mut ps = msg::PlayerState::null(p);
        let pm = msg::PlayerState::field_index(p, "pm_flags").unwrap();
        let ef = msg::PlayerState::field_index(p, "eFlags").unwrap();
        ps.fields[pm] = PMF_OWN_VIEW | 0x2;
        ps.fields[ef] = 0x20018;
        patch_wire(&mut ps, p, false, 0x18);
        assert_eq!((ps.fields[pm], ps.fields[ef]), (0x10002, 0x18));
        patch_wire(&mut ps, p, true, 0x20000);
        assert_eq!((ps.fields[pm], ps.fields[ef]), (0x30002, 0x20018));
    }
}
