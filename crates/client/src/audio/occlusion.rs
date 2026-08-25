//! Line-of-sight gain for spatial voices.
//!
//! Retail 1.1 has none (research doc, section 11): this is a deliberate
//! divergence, like the audible `asphault` rows. One zero-extent trace per
//! voice every few frames, staggered by voice id, with the result approached
//! over a handful of frames so walls do not zip.

use std::collections::HashMap;

use glam::Vec3;
use vcod_common::collision::CollisionWorld;

use crate::audio::voices::VoiceId;

/// Multiplicative gain on a blocked path, about -12 dB.
const OCCLUDED_GAIN: f32 = 0.25;

/// Fraction of the remaining gap closed per step; ~5 steps to settle at
/// 60 fps.
const APPROACH: f32 = 0.35;

/// Re-trace a given voice every this many steps (~100 ms at 60 fps),
/// staggered across voices by id.
const TRACE_PERIOD: u64 = 6;

/// Below this emitter distance nothing can stand between.
const NEAR_SQ: f32 = 16.0;

#[derive(Clone, Copy)]
struct State {
    /// Latest trace verdict.
    target: f32,
    /// What the mix actually sees this frame.
    smooth: f32,
}

#[derive(Default)]
pub struct Occlusion {
    states: HashMap<VoiceId, State>,
    frame: u64,
}

impl Occlusion {
    /// Per-voice gain for this frame. Voices absent from `emitters` are
    /// forgotten; new ones are traced immediately.
    pub fn compute(
        &mut self,
        emitters: &[(VoiceId, Vec3)],
        listener: Vec3,
        world: &CollisionWorld,
    ) -> HashMap<VoiceId, f32> {
        self.frame += 1;
        self.states
            .retain(|id, _| emitters.iter().any(|(e, _)| e == id));
        for (id, pos) in emitters.iter() {
            // Fixed residue per voice spreads the retraces across frames.
            let due = self.frame % TRACE_PERIOD == id % TRACE_PERIOD;
            let entry = self.states.entry(*id);
            let known = matches!(&entry, std::collections::hash_map::Entry::Occupied(_));
            if known && !due {
                continue;
            }
            let target = if listener.distance_squared(*pos) < NEAR_SQ {
                Some(1.0)
            } else {
                let tr = world.box_trace(listener, *pos, Vec3::ZERO, Vec3::ZERO);
                // Inside solid geometry the fraction says nothing about the
                // path; keep whatever we believed before.
                if tr.startsolid {
                    None
                } else if tr.fraction >= 1.0 {
                    Some(1.0)
                } else {
                    Some(OCCLUDED_GAIN)
                }
            };
            match (target, known) {
                (Some(t), _) => {
                    let st = entry.or_insert(State {
                        target: t,
                        smooth: t,
                    });
                    st.target = t;
                }
                (None, false) => {
                    entry.or_insert(State {
                        target: 1.0,
                        smooth: 1.0,
                    });
                }
                (None, true) => {}
            }
        }
        let mut out = HashMap::with_capacity(self.states.len());
        for (id, st) in &mut self.states {
            st.smooth += (st.target - st.smooth) * APPROACH;
            out.insert(*id, st.smooth);
        }
        out
    }

    /// No collision world (fly mode): everything open.
    pub fn reset(&mut self) {
        self.states.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::collision::test_world;

    fn wall_world() -> CollisionWorld {
        test_world(&[(Vec3::new(-8.0, -512.0, -16.0), Vec3::new(8.0, 512.0, 160.0))])
    }

    #[test]
    fn behind_the_wall_is_quiet_after_settling() {
        let mut o = Occlusion::default();
        let world = wall_world();
        let listener = Vec3::new(-64.0, 0.0, 48.0);
        let hidden = Vec3::new(64.0, 0.0, 48.0);
        let open = Vec3::new(-64.0, 700.0, 48.0);
        let voices = [(1u64, hidden), (2u64, open)];
        let mut gains = HashMap::new();
        for _ in 0..24 {
            gains = o.compute(&voices, listener, &world);
        }
        assert!(
            (gains[&1] - OCCLUDED_GAIN).abs() < 0.01,
            "hidden: {:?}",
            gains[&1]
        );
        assert!((gains[&2] - 1.0).abs() < 1e-4, "open: {:?}", gains[&2]);
    }

    #[test]
    fn the_gain_approaches_rather_than_snapping() {
        let mut o = Occlusion::default();
        let world = wall_world();
        let listener = Vec3::new(-64.0, 0.0, 48.0);
        let open = Vec3::new(-64.0, 700.0, 48.0);
        let hidden = Vec3::new(64.0, 0.0, 48.0);
        assert_eq!(o.compute(&[(7u64, open)], listener, &world)[&7], 1.0);
        // Wall slides onto the path: within one stagger window the mix has
        // begun moving toward the muffled target, without snapping to it.
        let mut moved = 1.0;
        for _ in 0..TRACE_PERIOD {
            moved = o.compute(&[(7u64, hidden)], listener, &world)[&7];
            if moved < 1.0 {
                break;
            }
        }
        assert!(
            moved > OCCLUDED_GAIN + 0.05 && moved < 1.0,
            "transition frame: {moved}"
        );
    }

    #[test]
    fn an_emitter_on_the_listener_is_never_occluded() {
        let mut o = Occlusion::default();
        let world = wall_world();
        let listener = Vec3::new(-64.0, 0.0, 48.0);
        let gains = o.compute(&[(9u64, listener)], listener, &world);
        assert_eq!(gains[&9], 1.0);
    }

    #[test]
    fn reset_reopens_everything_and_forgets() {
        let mut o = Occlusion::default();
        let world = wall_world();
        let listener = Vec3::new(-64.0, 0.0, 48.0);
        for _ in 0..24 {
            o.compute(&[(1u64, Vec3::new(64.0, 0.0, 48.0))], listener, &world);
        }
        o.reset();
        assert!(o.states.is_empty());
    }
}
