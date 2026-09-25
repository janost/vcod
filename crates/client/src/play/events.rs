//! The playing client's own events, played off the prediction and not again
//! off the snapshot that confirms them (retail cgame's predictable events).

use vcod_common::net::events::GameEvent;
use vcod_common::pmove::predict::Predicted;

const EVENT_RING: i32 = 4;

/// `seq - from` on the wire's 8-bit ring, signed: negative is behind.
fn ahead(seq: i32, from: i32) -> i32 {
    i32::from((seq - from) as u8 as i8)
}

/// The high-water mark of the playerstate ring's events already played:
/// one past the newest sequence played, `None` while not predicting.
#[derive(Default)]
pub struct PredictedEvents {
    played_to: Option<i32>,
}

impl PredictedEvents {
    /// Not predicting: the snapshot drain plays the whole ring.
    pub fn stop(&mut self) {
        self.played_to = None;
    }

    /// Starts tracking at the snapshot drain's own mark, so what it has not
    /// played yet plays here, once. A no-op while tracking.
    pub fn start_after(&mut self, drained: Option<i32>) {
        if self.played_to.is_none() {
            self.played_to = drained.map(|s| s & 0xff);
        }
    }

    /// `(seq, event, parm)` for the predicted ring's slots not played yet,
    /// oldest first.
    pub fn take_predicted(
        &mut self,
        pred_seq: i32,
        events: [i32; 4],
        parms: [i32; 4],
    ) -> Vec<(i32, i32, i32)> {
        let pred_seq = pred_seq & 0xff;
        let Some(from) = self.played_to else {
            self.played_to = Some(pred_seq);
            return Vec::new();
        };
        let n = ahead(pred_seq, from);
        if n <= 0 {
            if n < -EVENT_RING {
                self.played_to = Some(pred_seq);
            }
            return Vec::new();
        }
        self.played_to = Some(pred_seq);
        (pred_seq - n.min(EVENT_RING)..pred_seq)
            .map(|s| {
                let slot = (s & 3) as usize;
                (s & 0xff, events[slot], parms[slot])
            })
            .collect()
    }

    /// Whether a snapshot's playerstate-ring event at `seq` still has to
    /// play: false for a copy of one already played.
    pub fn filter_snapshot(&mut self, seq: i32) -> bool {
        let seq = seq & 0xff;
        let Some(from) = self.played_to else {
            return true;
        };
        let d = ahead(seq, from);
        if (-EVENT_RING..0).contains(&d) {
            return false;
        }
        self.played_to = Some((seq + 1) & 0xff);
        true
    }
}

/// A predicted ring event in the form the snapshot drain gives the
/// playerstate ring's (`EventTracker::drain`).
pub fn game_event(pred: &Predicted, client_num: i32, event: i32, parm: i32) -> GameEvent {
    GameEvent {
        event,
        parm,
        entity_num: u32::MAX,
        client_num,
        weapon: i32::from(pred.ps.weapon),
        surf_type: 0,
        pos: pred.ps.origin.to_array(),
        dir: [0.0; 3],
        other_entity_num: u32::MAX,
        attacker_entity_num: -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRE: i32 = 159;
    const RELOAD: i32 = 150;
    const PAIN: i32 = 191;

    fn tracking(at: i32) -> PredictedEvents {
        let mut e = PredictedEvents::default();
        e.start_after(Some(at));
        e
    }

    #[test]
    fn plays_each_sequence_once() {
        let mut e = tracking(0);
        let ring = [FIRE, RELOAD, 0, 0];
        assert_eq!(
            e.take_predicted(1, ring, [0; 4]),
            [(0, FIRE, 0)],
            "the slot below the sequence"
        );
        assert!(e.take_predicted(1, ring, [0; 4]).is_empty(), "replayed");
        assert_eq!(e.take_predicted(2, ring, [0, 7, 0, 0]), [(1, RELOAD, 7)]);

        // More than a ring behind: the newest four, across the wrap.
        let mut e = tracking(250);
        let got = e.take_predicted(1, [10, 11, 12, 13], [0; 4]);
        assert_eq!(got, [(253, 11, 0), (254, 12, 0), (255, 13, 0), (0, 10, 0)]);
    }

    #[test]
    fn first_sight_plays_nothing() {
        let mut e = PredictedEvents::default();
        assert!(e.take_predicted(3, [FIRE; 4], [0; 4]).is_empty());
        assert_eq!(e.take_predicted(4, [FIRE; 4], [0; 4]), [(3, FIRE, 0)]);
    }

    #[test]
    fn snapshot_copy_is_skipped() {
        let mut e = tracking(0);
        e.take_predicted(3, [FIRE, RELOAD, FIRE, 0], [0; 4]);
        assert!((0..3).all(|s| !e.filter_snapshot(s)));
        // The drain's ring walk hands negative sequences across the wrap.
        let mut e = tracking(254);
        e.take_predicted(1, [FIRE; 4], [0; 4]);
        assert!([-2, -1, 0].into_iter().all(|s| !e.filter_snapshot(s)));
    }

    /// A respawn restarts the ring at 0 (AGENTS.md, Gotchas): the mark goes
    /// back with it, or the new life's events are all swallowed.
    #[test]
    fn sequence_going_back_resets() {
        let mut e = tracking(0);
        e.take_predicted(40, [FIRE; 4], [0; 4]);
        assert!(e.take_predicted(1, [FIRE; 4], [0; 4]).is_empty());
        assert_eq!(e.take_predicted(2, [FIRE; 4], [0; 4]), [(1, FIRE, 0)]);

        let mut e = tracking(0);
        e.take_predicted(40, [FIRE; 4], [0; 4]);
        assert!(e.filter_snapshot(0));
        assert!(e.filter_snapshot(1));
        assert!(
            e.take_predicted(2, [FIRE; 4], [0; 4]).is_empty(),
            "the drain played those"
        );

        // A misprediction a few behind is not a reset.
        let mut e = tracking(0);
        e.take_predicted(10, [FIRE; 4], [0; 4]);
        assert!(e.take_predicted(9, [FIRE; 4], [0; 4]).is_empty());
        assert!(e.take_predicted(10, [FIRE; 4], [0; 4]).is_empty());
    }

    /// A pain event the client cannot predict passes, and the replay that
    /// carries it forward from that snapshot does not play it again.
    #[test]
    fn server_only_events_still_play() {
        let mut e = tracking(0);
        e.take_predicted(3, [FIRE, FIRE, FIRE, 0], [0; 4]);
        assert!(e.filter_snapshot(3), "above the mark");
        assert!(e
            .take_predicted(4, [FIRE, FIRE, FIRE, PAIN], [0; 4])
            .is_empty());
        assert_eq!(e.take_predicted(5, [FIRE, 0, 0, 0], [0; 4]), [(4, FIRE, 0)]);
    }

    #[test]
    fn not_predicting_filters_nothing() {
        let mut e = tracking(0);
        e.take_predicted(3, [FIRE; 4], [0; 4]);
        e.stop();
        assert!((0..3).all(|s| e.filter_snapshot(s)));
        // Back to predicting: from the drain's mark, not the stale one.
        e.start_after(Some(5));
        assert_eq!(
            e.take_predicted(7, [0, FIRE, RELOAD, 0], [0; 4]),
            [(5, FIRE, 0), (6, RELOAD, 0)]
        );
        e.start_after(Some(0));
        assert!(!e.filter_snapshot(6), "a no-op while tracking");
    }
}
