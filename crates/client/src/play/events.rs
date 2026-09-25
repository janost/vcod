//! The playing client's own events, played off the prediction and not again
//! off the snapshot that confirms them: retail cgame's predictable events,
//! deduped per sequence and event the way Q3's
//! `CG_CheckChangedPredictableEvents` does.

use vcod_common::net::events::GameEvent;
use vcod_common::pmove::predict::Predicted;

const EVENT_RING: i32 = 4;
/// Q3's `MAX_PREDICTED_EVENTS`: how far back a played event is remembered.
const REMEMBERED: usize = 8;

/// `seq - from` on the wire's 8-bit ring, signed: negative is behind.
fn ahead(seq: i32, from: i32) -> i32 {
    i32::from((seq - from) as u8 as i8)
}

#[derive(Default)]
pub struct PredictedEvents {
    /// One past the newest sequence played, `None` while not predicting.
    played_to: Option<i32>,
    /// `(seq, event)` played, at `seq & 7`. `None` is unknown: the drain,
    /// not the prediction, played it or nobody did.
    played: [Option<(i32, i32)>; REMEMBERED],
}

impl PredictedEvents {
    /// Not predicting: the snapshot drain plays the whole ring.
    pub fn stop(&mut self) {
        *self = PredictedEvents::default();
    }

    /// Starts tracking at the snapshot drain's own mark, so what it has not
    /// played yet plays here, once. A no-op while tracking.
    pub fn start_after(&mut self, drained: Option<i32>) {
        if self.played_to.is_none() {
            self.played_to = drained.map(|s| s & 0xff);
        }
    }

    fn was_played(&self, seq: i32, event: i32) -> bool {
        self.played[seq as usize % REMEMBERED] == Some((seq, event))
    }

    fn record(&mut self, seq: i32, event: i32) {
        self.played[seq as usize % REMEMBERED] = Some((seq, event));
    }

    /// `(seq, event, parm)` to play off the predicted ring, oldest first:
    /// slots past the mark, and slots under it whose event is not the one
    /// played there (a replay on a new snapshot changed it).
    pub fn take_predicted(
        &mut self,
        pred_seq: i32,
        events: [i32; 4],
        parms: [i32; 4],
    ) -> Vec<(i32, i32, i32)> {
        let pred_seq = pred_seq & 0xff;
        let Some(mark) = self.played_to else {
            self.played_to = Some(pred_seq);
            return Vec::new();
        };
        // A respawn restarts the ring at 0 (AGENTS.md, Gotchas).
        if ahead(pred_seq, mark) < -EVENT_RING {
            self.stop();
            self.played_to = Some(pred_seq);
            return Vec::new();
        }
        let mut out = Vec::new();
        for s in pred_seq - EVENT_RING..pred_seq {
            let seq = s & 0xff;
            let slot = (s & 3) as usize;
            let event = events[slot];
            let changed = self.played[seq as usize % REMEMBERED]
                .is_some_and(|(at, e)| at == seq && e != event);
            if ahead(seq, mark) >= 0 || changed {
                self.record(seq, event);
                out.push((seq, event, parms[slot]));
            }
        }
        if ahead(pred_seq, mark) > 0 {
            self.played_to = Some(pred_seq);
        }
        out
    }

    /// Whether a snapshot's playerstate-ring `event` at `seq` still has to
    /// play: false only for the copy of one played at that sequence.
    pub fn filter_snapshot(&mut self, seq: i32, event: i32) -> bool {
        let seq = seq & 0xff;
        let below = self.played_to.is_some_and(|mark| ahead(seq, mark) < 0);
        if below && self.was_played(seq, event) {
            return false;
        }
        self.record(seq, event);
        if self.played_to.is_some() && !below {
            self.played_to = Some((seq + 1) & 0xff);
        }
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
    use crate::fx::registry::{EV_FIRE_WEAPON as FIRE, EV_PAIN as PAIN, EV_RELOAD as RELOAD};

    const PICKUP: i32 = 20;
    const FOOTSTEP: i32 = 1;

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
        let ring = [FIRE, RELOAD, FIRE, 0];
        e.take_predicted(3, ring, [0; 4]);
        assert!((0..3).all(|s| !e.filter_snapshot(s, ring[s as usize])));
        // The drain's ring walk hands negative sequences across the wrap.
        let mut e = tracking(254);
        e.take_predicted(1, [FIRE; 4], [0; 4]);
        assert!([-2, -1, 0].into_iter().all(|s| !e.filter_snapshot(s, FIRE)));
    }

    /// After a hitch the drain carries sequences more than a ring below the
    /// mark: the ones the prediction played are skipped, the one it could not
    /// reach plays, and nothing resets.
    #[test]
    fn a_hitch_skips_what_was_played_and_plays_the_rest() {
        let mut e = tracking(10);
        let ring = [FIRE, RELOAD, FIRE, RELOAD];
        let played: Vec<_> = e.take_predicted(16, ring, [0; 4]);
        assert_eq!(
            played.iter().map(|p| p.0).collect::<Vec<_>>(),
            [12, 13, 14, 15]
        );
        assert!(e.filter_snapshot(11, ring[3]), "never played");
        for s in 12..=14 {
            assert!(!e.filter_snapshot(s, ring[(s & 3) as usize]), "{s}");
        }
        assert!(e.take_predicted(16, ring, [0; 4]).is_empty());
    }

    /// A respawn restarts the ring at 0 (AGENTS.md, Gotchas): the prediction
    /// resets the mark, or the new life's events are all swallowed.
    #[test]
    fn sequence_going_back_resets() {
        let mut e = tracking(0);
        e.take_predicted(40, [FIRE; 4], [0; 4]);
        assert!(e.take_predicted(1, [FIRE; 4], [0; 4]).is_empty());
        assert!(e.filter_snapshot(0, FIRE), "the drain plays the new life's");
        assert_eq!(e.take_predicted(2, [FIRE; 4], [0; 4]), [(1, FIRE, 0)]);

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
        assert!(e.filter_snapshot(3, PAIN), "above the mark");
        assert!(e
            .take_predicted(4, [FIRE, FIRE, FIRE, PAIN], [0; 4])
            .is_empty());
        assert_eq!(
            e.take_predicted(5, [FIRE, FIRE, FIRE, PAIN], [0; 4]),
            [(4, FIRE, 0)]
        );
    }

    /// The server put a pickup where the prediction put a fire: the pickup
    /// plays, off either path, once.
    #[test]
    fn a_server_event_under_a_predicted_one_plays() {
        let mut e = tracking(0);
        e.take_predicted(2, [FIRE, FIRE, 0, 0], [0; 4]);
        assert!(e.filter_snapshot(1, PICKUP));
        assert!(e.take_predicted(2, [FIRE, PICKUP, 0, 0], [0; 4]).is_empty());

        // The replay on the new snapshot sees it first.
        let mut e = tracking(0);
        e.take_predicted(2, [FIRE, FIRE, 0, 0], [0; 4]);
        assert_eq!(
            e.take_predicted(2, [FIRE, PICKUP, 0, 0], [0; 4]),
            [(1, PICKUP, 0)]
        );
        assert!(!e.filter_snapshot(1, PICKUP));
    }

    /// A predicted footstep the server never raised: its real fire at that
    /// sequence plays.
    #[test]
    fn a_real_event_over_a_phantom_plays() {
        let mut e = tracking(0);
        e.take_predicted(1, [FOOTSTEP, 0, 0, 0], [0; 4]);
        assert!(e.filter_snapshot(0, FIRE));
    }

    #[test]
    fn not_predicting_filters_nothing() {
        let mut e = tracking(0);
        e.take_predicted(3, [FIRE; 4], [0; 4]);
        e.stop();
        assert!((0..3).all(|s| e.filter_snapshot(s, FIRE)));
        // Back to predicting: from the drain's mark, not the stale one.
        e.stop();
        e.start_after(Some(5));
        assert_eq!(
            e.take_predicted(7, [0, FIRE, RELOAD, 0], [0; 4]),
            [(5, FIRE, 0), (6, RELOAD, 0)]
        );
        e.start_after(Some(0));
        assert!(!e.filter_snapshot(6, RELOAD), "a no-op while tracking");
    }
}
