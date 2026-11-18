//! `ET_FX` entities, what `playLoopedFx` leaves on the wire: cgame's
//! `CG_AddCEntity` case 10 (0x3001cf20) replays the effect every `period`
//! ms while the entity is in the drawn snapshot
//! (docs/research/cod11-events-and-fx.md 2, "Looped effects").

use crate::fx::registry::effect_path;
use crate::fx::sim::SpawnAt;
use glam::Vec3;
use std::collections::HashMap;
use vcod_common::net::flags::ET_FX;
use vcod_common::net::protocol::{Cod1Wire, CsLayout};
use vcod_common::net::snapshot::Snapshot;

/// One play: the `.efx` path and where it goes.
#[derive(Debug)]
pub struct LoopedPlay {
    pub path: String,
    pub at: SpawnAt,
}

/// The `ET_FX` fields `CG_FX` reads.
#[derive(Clone, Copy, Debug)]
pub struct FxEntity {
    /// `es.scale`, the effect id.
    pub id: i32,
    /// `lerpOrigin`; the entity never moves.
    pub origin: Vec3,
    /// `es.origin2`, the forward; zero for none.
    pub forward: Vec3,
    /// `es.angles2[0]`, 0 for never culled.
    pub cull: f32,
    /// `es.angles2[1]`, the period in ms.
    pub period: f32,
}

impl FxEntity {
    pub fn from_state(e: &vcod_common::net::msg::EntityState, p: &Cod1Wire) -> Option<FxEntity> {
        if e.field_i32(p, "eType") != ET_FX {
            return None;
        }
        Some(FxEntity {
            id: e.field_i32(p, "_union.scale"),
            origin: Vec3::from(e.origin(p)),
            forward: Vec3::new(
                e.field_f32(p, "origin2[0]"),
                e.field_f32(p, "origin2[1]"),
                e.field_f32(p, "origin2[2]"),
            ),
            cull: e.field_f32(p, "angles2[0]"),
            period: e.field_f32(p, "angles2[1]"),
        })
    }
}

/// `cent+0x21c` per entity number: the time of the last play. Nothing in
/// cgame resets it, so it outlives the entity; a new level starts a new
/// table.
#[derive(Default)]
pub struct LoopedFx {
    last: HashMap<u32, i32>,
}

impl LoopedFx {
    /// Every `ET_FX` entity in `snap` due a play at `time` (the drawn
    /// server time), seen from `view`.
    pub fn frame(
        &mut self,
        snap: &Snapshot,
        p: &Cod1Wire,
        time: i32,
        view: Vec3,
        configstrings: &[String],
    ) -> Vec<LoopedPlay> {
        snap.entities
            .iter()
            .filter_map(|(&num, e)| {
                let ent = FxEntity::from_state(e, p)?;
                self.step(num, &ent, time, view, configstrings, &p.cs)
            })
            .collect()
    }

    /// `CG_FX` for one entity: the timer first, then the cull, then the id.
    pub fn step(
        &mut self,
        num: u32,
        ent: &FxEntity,
        time: i32,
        view: Vec3,
        configstrings: &[String],
        cs: &CsLayout,
    ) -> Option<LoopedPlay> {
        let last = self.last.entry(num).or_insert(0);
        if time > *last {
            // `_ftol` truncates (0x3005a890).
            let period = ent.period as i32;
            if period <= 0 {
                // Retail's catch-up loop never ends on a zero period; a
                // play a frame is what it would draw if it did.
                *last = time;
            } else {
                let behind = time - *last;
                if behind < period {
                    return None;
                }
                // The loop adds the period at least once and until at most
                // one period is left.
                let steps = ((behind - period + period - 1) / period).max(1);
                *last += steps * period;
            }
        } else {
            *last = time;
        }
        // Played only inside the cull distance, the camera's
        // `cg.refdef.vieworg`; the timer above runs either way.
        if ent.cull != 0.0 && ent.origin.distance_squared(view) >= ent.cull * ent.cull {
            return None;
        }
        let path = effect_path(configstrings, cs, ent.id)?;
        let at = if ent.forward.length_squared() != 0.0 {
            SpawnAt::Directed {
                pos: ent.origin,
                dir: ent.forward,
            }
        } else {
            SpawnAt::Point { pos: ent.origin }
        };
        Some(LoopedPlay {
            path: path.to_string(),
            at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::{CS_EFFECTS_V1, CS_LAYOUT_V1};

    fn cs() -> Vec<String> {
        let mut cs = vec![String::new(); 2048];
        cs[CS_EFFECTS_V1 + 2] = "fx/smoke/chimneysmoke.efx".into();
        cs
    }

    fn ent(period: f32) -> FxEntity {
        FxEntity {
            id: 2,
            origin: Vec3::new(100.0, 0.0, 0.0),
            forward: Vec3::ZERO,
            cull: 0.0,
            period,
        }
    }

    /// Frames 50 ms apart: the first plays at once, the next on the
    /// period grid counted from 0, whatever the frame step.
    #[test]
    fn plays_on_first_sight_then_on_the_period_grid() {
        let (cs, mut l) = (cs(), LoopedFx::default());
        let e = ent(200.0);
        let played: Vec<i32> = (0..12)
            .map(|i| 10_030 + i * 50)
            .filter(|&t| l.step(5, &e, t, Vec3::ZERO, &cs, &CS_LAYOUT_V1).is_some())
            .collect();
        // 10030 catches the timer up to 10000; the next plays fall on the
        // first frames past 10200 and 10400.
        assert_eq!(played, vec![10_030, 10_230, 10_430]);
    }

    #[test]
    fn up_axis_without_a_forward_and_directed_with_one() {
        let (cs, mut l) = (cs(), LoopedFx::default());
        let play = l
            .step(1, &ent(100.0), 1000, Vec3::ZERO, &cs, &CS_LAYOUT_V1)
            .unwrap();
        assert_eq!(play.path, "fx/smoke/chimneysmoke.efx");
        assert!(matches!(play.at, SpawnAt::Point { pos } if pos.x == 100.0));
        let mut e = ent(100.0);
        e.forward = Vec3::Y;
        let play = l.step(2, &e, 1000, Vec3::ZERO, &cs, &CS_LAYOUT_V1).unwrap();
        assert!(matches!(play.at, SpawnAt::Directed { dir, .. } if dir == Vec3::Y));
    }

    /// The cull skips the play but not the timer, so walking into range
    /// waits out the rest of the period.
    #[test]
    fn culled_plays_still_advance_the_timer() {
        let (cs, mut l) = (cs(), LoopedFx::default());
        let mut e = ent(1000.0);
        e.cull = 50.0;
        // Caught up to 4000 out of range.
        assert!(
            l.step(1, &e, 4500, Vec3::ZERO, &cs, &CS_LAYOUT_V1)
                .is_none()
        );
        let near = Vec3::new(90.0, 0.0, 0.0);
        assert!(l.step(1, &e, 4900, near, &cs, &CS_LAYOUT_V1).is_none());
        assert!(l.step(1, &e, 5000, near, &cs, &CS_LAYOUT_V1).is_some());
    }

    #[test]
    fn an_id_out_of_range_or_unloaded_plays_nothing() {
        let (cs, mut l) = (cs(), LoopedFx::default());
        for id in [0, 3, 64] {
            let mut e = ent(100.0);
            e.id = id;
            assert!(
                l.step(id as u32, &e, 1000, Vec3::ZERO, &cs, &CS_LAYOUT_V1)
                    .is_none()
            );
        }
    }
}
