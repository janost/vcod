//! The playing client's own events, played off the prediction and not again
//! off the snapshot that confirms them: retail cgame's predictable events,
//! deduped per sequence and event the way Q3's
//! `CG_CheckChangedPredictableEvents` does.

use vcod_common::net::events::{EVENT_RING, EventTracker, GameEvent};
use vcod_common::net::protocol::Cod1Wire;
use vcod_common::net::snapshot::Snapshot;
use vcod_common::pmove::predict::Predicted;

use super::input::Stance;

/// How many sequences back a played event is remembered: Q3's
/// `MAX_PREDICTED_EVENTS`. Divides 256, so `seq % REMEMBERED` survives the wrap.
const REMEMBERED: usize = 16;

/// `seq - from` on the wire's 8-bit ring, signed: negative is behind.
fn ahead(seq: i32, from: i32) -> i32 {
    i32::from((seq - from) as u8 as i8)
}

#[derive(Default)]
pub struct PredictedEvents {
    /// One past the newest sequence played, `None` while not predicting.
    played_to: Option<i32>,
    /// `(seq, event)` played, at `seq % REMEMBERED`. `None`: nothing played
    /// at that slot since tracking started.
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
    /// play: false for the copy of one played at that sequence, and for
    /// one [`REMEMBERED`] or more behind the prediction, which retail's
    /// `CG_CheckChangedPredictableEvents` (cgame 0x30028c8e) no longer
    /// compares. Recording that one would overwrite a newer sequence's
    /// slot, and the newer one would play again when the snapshot catches
    /// up: a UO winded sprint puts five fatigue events in one snapshot.
    pub fn filter_snapshot(&mut self, seq: i32, event: i32) -> bool {
        let seq = seq & 0xff;
        let behind = self.played_to.map(|mark| ahead(seq, mark));
        if behind.is_some_and(|b| b <= -(REMEMBERED as i32)) {
            return false;
        }
        let below = behind.is_some_and(|b| b < 0);
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

/// One frame's events in play order: the new slots of our predicted ring,
/// then the newest snapshot's events less the copies of slots the
/// prediction played. `own`: the playerstate is ours, not a followed
/// player's; `predicting`: ours, of a `pm_type` the client predicts;
/// `predicted`: this frame's prediction, when it ran. The frame we reach a
/// state the client does not predict still skips the copies, then tracking
/// stops.
pub fn frame_events(
    tracked: &mut PredictedEvents,
    tracker: &mut EventTracker,
    newest: &Snapshot,
    p: &Cod1Wire,
    own: bool,
    predicting: bool,
    predicted: Option<&Predicted>,
    ps_client: i32,
) -> Vec<GameEvent> {
    let mut evs = Vec::new();
    if !own {
        tracked.stop();
    } else if let Some(pred) = predicted.filter(|_| predicting) {
        tracked.start_after(tracker.ps_sequence());
        evs.extend(
            tracked
                .take_predicted(pred.ring.seq, pred.ring.events, pred.ring.parms)
                .into_iter()
                .map(|(_, event, parm)| game_event(pred, ps_client, event, parm)),
        );
    }
    for (seq, ev) in tracker.drain_seq(newest, p) {
        if own && seq.is_some_and(|s| !tracked.filter_snapshot(s, ev.event)) {
            continue;
        }
        evs.push(ev);
    }
    if !predicting {
        tracked.stop();
    }
    evs
}

/// How many times this frame's events kick the view: `CG_FireWeapon` kicks
/// only for the view's own body (`0x30038bc8`), off the playerstate events
/// `CG_PredictPlayerState` (or a follow's snapshot transition) runs before
/// `CG_KickAngles` steps it (0x30033d58).
pub fn view_kicks(evs: &[GameEvent], ps_client: i32) -> usize {
    evs.iter()
        .filter(|ev| ev.entity_num == u32::MAX || ev.entity_num == ps_client as u32)
        .map(|ev| super::recoil::fire_calls(ev.event))
        .sum()
}

/// Whether a playerstate or entity event rides the body the view rides.
fn on_view_body(ev: &GameEvent, view_body: Option<u32>) -> bool {
    view_body.is_some_and(|b| ev.entity_num == u32::MAX || ev.entity_num == b)
}

/// The weapon an own pickup event names: `CG_ItemPickup` (cgame 0x3001da80)
/// is reached from the three pickup events on the body the view rides (call
/// at 0x3001e0ff) and acts on a weapon row only, items 1..=`max_weapons`
/// (64, UO 128), configstring 7's weapons
/// (docs/research/cod11-gsc-object-model.md, section 15).
/// `PlayInput::picked_up` decides whether it becomes the selection.
pub fn picked_up_weapon(ev: &GameEvent, view_body: Option<u32>, p: &Cod1Wire) -> Option<u8> {
    use vcod_common::net::event_ids::{EV_AMMO_PICKUP, EV_ITEM_PICKUP, EV_ITEM_PICKUP_QUIET};
    let pickup = matches!(
        ev.event,
        EV_ITEM_PICKUP | EV_ITEM_PICKUP_QUIET | EV_AMMO_PICKUP
    );
    let weapon = (1..=p.max_weapons as i32).contains(&ev.parm);
    (pickup && on_view_body(ev, view_body) && weapon).then_some(ev.parm as u8)
}

/// `EV_NOAMMO` (149) and `EV_DROPWEAPON` (196) on the body the view rides
/// run the out-of-ammo switch 0x300381b0 (arms 0x3001e1d4 and 0x3001e1f9,
/// gate 0x3001e21b, jump 0x3001e241), `PlayInput::out_of_ammo`.
pub fn runs_out_of_ammo(ev: &GameEvent, view_body: Option<u32>) -> bool {
    use vcod_common::net::event_ids::{EV_DROPWEAPON, EV_NOAMMO};
    matches!(ev.event, EV_NOAMMO | EV_DROPWEAPON) && on_view_body(ev, view_body)
}

/// The stance an `EV_STANCE_FORCE_*` puts the client in: retail cgame's
/// handler (0x3001df6d) sets `cl_stance` to 0, 1 or 2 for an event of its
/// own client while `cl_stanceTemp` is 0 (`stance_temp` false), and ignores
/// another client's.
pub fn forced_stance(ev: &GameEvent, own_client: i32, stance_temp: bool) -> Option<Stance> {
    use vcod_common::net::event_ids::{
        EV_STANCE_FORCE_CROUCH, EV_STANCE_FORCE_PRONE, EV_STANCE_FORCE_STAND,
    };
    if ev.client_num != own_client || stance_temp {
        return None;
    }
    match ev.event {
        EV_STANCE_FORCE_STAND => Some(Stance::Stand),
        EV_STANCE_FORCE_CROUCH => Some(Stance::Crouch),
        EV_STANCE_FORCE_PRONE => Some(Stance::Prone),
        _ => None,
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
        scale: 0,
        other_entity_num: u32::MAX,
        attacker_entity_num: -1,
        e_type: 0,
        eflags: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::registry::{EV_FIRE_WEAPON as FIRE, EV_PAIN as PAIN, EV_RELOAD as RELOAD};

    const PICKUP: i32 = 20;

    fn event(event: i32, parm: i32, entity_num: u32) -> GameEvent {
        GameEvent {
            event,
            parm,
            entity_num,
            client_num: 3,
            weapon: 0,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            scale: 0,
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
            e_type: 0,
            eflags: 0,
        }
    }

    #[test]
    fn an_own_weapon_pickup_names_the_weapon() {
        use vcod_common::net::event_ids::{EV_AMMO_PICKUP, EV_ITEM_PICKUP};
        use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V22};
        let (v1, uo) = (&PROTOCOL_V1, &PROTOCOL_V22);
        let me = Some(3);
        assert_eq!(
            picked_up_weapon(&event(EV_ITEM_PICKUP, 6, 3), me, v1),
            Some(6)
        );
        assert_eq!(
            picked_up_weapon(&event(EV_AMMO_PICKUP, 6, u32::MAX), me, v1),
            Some(6)
        );
        // Health, another player, no body, not a pickup.
        assert_eq!(
            picked_up_weapon(&event(EV_ITEM_PICKUP, 68, 3), me, v1),
            None
        );
        assert_eq!(picked_up_weapon(&event(EV_ITEM_PICKUP, 6, 5), me, v1), None);
        assert_eq!(
            picked_up_weapon(&event(EV_ITEM_PICKUP, 6, 3), None, v1),
            None
        );
        assert_eq!(picked_up_weapon(&event(FIRE, 6, 3), me, v1), None);
        // UO's 68 is a weapon row, its small health 131.
        assert_eq!(
            picked_up_weapon(&event(EV_ITEM_PICKUP, 68, 3), me, uo),
            Some(68)
        );
        assert_eq!(
            picked_up_weapon(&event(EV_ITEM_PICKUP, 131, 3), me, uo),
            None
        );
    }

    #[test]
    fn an_own_dry_trigger_or_drop_runs_the_out_of_ammo_switch() {
        use vcod_common::net::event_ids::{EV_DROPWEAPON, EV_NOAMMO};
        let me = Some(3);
        assert!(runs_out_of_ammo(&event(EV_NOAMMO, 0, u32::MAX), me));
        assert!(runs_out_of_ammo(&event(EV_DROPWEAPON, 0, 3), me));
        assert!(!runs_out_of_ammo(&event(EV_NOAMMO, 0, 5), me));
        assert!(!runs_out_of_ammo(&event(EV_NOAMMO, 0, 3), None));
        assert!(!runs_out_of_ammo(&event(RELOAD, 0, 3), me));
    }

    /// Retail cgame's `cl_stance` write (0x3001df6d): our own client's
    /// forced-stance events set the stance, another client's do nothing.
    #[test]
    fn a_forced_stance_event_sets_our_stance() {
        use vcod_common::net::event_ids::{
            EV_STANCE_FORCE_CROUCH, EV_STANCE_FORCE_PRONE, EV_STANCE_FORCE_STAND,
        };
        let ev = |event: i32, client_num: i32| GameEvent {
            event,
            parm: 0,
            entity_num: u32::MAX,
            client_num,
            weapon: 0,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            scale: 0,
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
            e_type: 0,
            eflags: 0,
        };
        assert_eq!(
            forced_stance(&ev(EV_STANCE_FORCE_STAND, 2), 2, false),
            Some(Stance::Stand)
        );
        assert_eq!(
            forced_stance(&ev(EV_STANCE_FORCE_CROUCH, 2), 2, false),
            Some(Stance::Crouch)
        );
        assert_eq!(
            forced_stance(&ev(EV_STANCE_FORCE_PRONE, 2), 2, false),
            Some(Stance::Prone)
        );
        assert_eq!(
            forced_stance(&ev(EV_STANCE_FORCE_CROUCH, 5), 2, false),
            None
        );
        assert_eq!(forced_stance(&ev(FIRE, 2), 2, false), None);
        // `cl_stanceTemp` non-zero leaves `cl_stance` alone (0x3001dee5).
        assert_eq!(forced_stance(&ev(EV_STANCE_FORCE_CROUCH, 2), 2, true), None);
    }

    /// Retail 1.1d and UO 1.51b (`net/snapshots-follow-stance{,-uo}.bin`,
    /// 2026-11-14): a spectator (client 1) following client 0 through
    /// `probe_pronedrop`'s lifts reads the followed body's airborne 141s
    /// (UO's 143), and on 1.1d its 140 too, in its own playerstate's ring, under
    /// the followed `clientNum`. The cgame's arm takes them as another
    /// client's and leaves `cl_stance` alone (cod11-mantle.md, "The
    /// forced-stance events").
    #[test]
    fn a_follower_keeps_its_stance_through_the_followed_bodys_events() {
        use vcod_common::net::event_ids::{
            EV_STANCE_FORCE_CROUCH, EV_STANCE_FORCE_PRONE, EV_STANCE_FORCE_STAND,
        };
        use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V22};
        use vcod_common::testing::{fixture_frames, fixture_gamestate};
        for (p, tag) in [
            (PROTOCOL_V1, "follow-stance"),
            (PROTOCOL_V22, "follow-stance-uo"),
        ] {
            let (gs, snaps) = (
                format!("net/gamestate-{tag}.bin"),
                format!("net/snapshots-{tag}.bin"),
            );
            let own = fixture_gamestate(&p, &gs).client_num;
            assert_eq!(own, 1, "{tag}");
            let mut tracker = EventTracker::new();
            let mut stance = Vec::new();
            for frame in fixture_frames(&p, &gs, &snaps) {
                for ev in tracker.drain(&frame.snapshot, &p) {
                    if matches!(
                        ev.event,
                        EV_STANCE_FORCE_STAND | EV_STANCE_FORCE_CROUCH | EV_STANCE_FORCE_PRONE
                    ) {
                        assert_eq!((ev.entity_num, ev.client_num), (u32::MAX, 0), "{tag}");
                        assert_eq!(forced_stance(&ev, own, false), None, "{tag}");
                        stance.push(ev.event);
                    }
                }
            }
            assert!(
                stance.contains(&EV_STANCE_FORCE_CROUCH),
                "{tag}: {stance:?}"
            );
            if tag == "follow-stance" {
                assert!(stance.contains(&EV_STANCE_FORCE_STAND), "{tag}: {stance:?}");
            }
        }
    }

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

    /// A snapshot that lags the prediction by a remembered window or more
    /// neither plays nor records: recording seq 68 over seq 84's slot made
    /// 84's shot play again once the snapshots caught up (the loopback
    /// gate's UO lossy link, where a winded sprint floods the ring).
    #[test]
    fn a_snapshot_a_window_behind_is_skipped_and_not_recorded() {
        let mut e = tracking(64);
        for seq in 65..=85 {
            let ring: [i32; 4] = std::array::from_fn(|i| if i == 0 { FIRE } else { PAIN });
            e.take_predicted(seq, ring, [0; 4]);
        }
        assert!(!e.filter_snapshot(68, FIRE), "17 behind the prediction");
        assert!(!e.filter_snapshot(84, FIRE), "played by the prediction");
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
        assert!(
            e.take_predicted(4, [FIRE, FIRE, FIRE, PAIN], [0; 4])
                .is_empty()
        );
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

    /// A UO shot confirmed by its snapshot plays once. The replay restarts
    /// from the snapshot's ring, which UO numbers its own way (its
    /// `EV_FIRE_WEAPON` is 163, 1.1's `EV_EJECT_BRASS`); read raw, the slot
    /// looked changed, played as a brass, and the snapshot's own copy of
    /// the fire then played a second shot, sound, flash and kick.
    #[test]
    fn a_uo_snapshot_confirming_a_shot_plays_nothing_more() {
        use vcod_common::net::event_ids::event_v1_to_v22;
        use vcod_common::net::msg::PlayerState;
        use vcod_common::net::protocol::PROTOCOL_V22;
        use vcod_common::pmove::predict::from_wire;
        let p = &PROTOCOL_V22;
        let mut e = tracking(0);
        assert_eq!(e.take_predicted(1, [FIRE, 0, 0, 0], [0; 4]), [(0, FIRE, 0)]);
        let mut w = PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            w.fields[PlayerState::field_index(p, name).unwrap()] = v;
        };
        set("events[0]", event_v1_to_v22(FIRE));
        set("eventSequence", 1);
        let ring = from_wire(p, &w, None).ring;
        assert!(
            e.take_predicted(ring.seq, ring.events, ring.parms)
                .is_empty()
        );
        // The drain maps the snapshot's copy to 1.1's numbering.
        assert!(!e.filter_snapshot(0, FIRE));
    }

    /// A prediction running well ahead of the drained snapshot, as automatic
    /// fire and footsteps do at a few hundred ms of round trip: every copy
    /// the snapshots bring back is still remembered and skipped.
    #[test]
    fn twelve_ahead_every_copy_is_skipped_once() {
        let mut e = tracking(0);
        let event_at = |s: i32| if s % 3 == 0 { FOOTSTEP } else { FIRE };
        let mut ring = [0; 4];
        for s in 0..12 {
            ring[(s & 3) as usize] = event_at(s);
            assert_eq!(e.take_predicted(s + 1, ring, [0; 4]), [(s, event_at(s), 0)]);
        }
        for s in 0..12 {
            assert!(!e.filter_snapshot(s, event_at(s)), "copy of {s} replayed");
        }
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
