//! An xanim's root motion between two node times, what `G_RunCorpseMove`
//! (cod4_lnxded 0x80b97e0) moves a corpse by: `XAnimCalcDelta`'s
//! translation, the root's offset at the new time less the old one,
//! turned by the inverse of the root's yaw at the new time
//! (docs/research/cod4-combat.md 5.2). The translation is the mantle's
//! `XAnim_CalcDeltaForTime` ([`crate::pmove::mantle`]); the turn is CoD2's
//! measured shape (`vcod_common::xanim::unturn_by_yaw`), INFERRED on 1.7.

use glam::Vec3;

use crate::assets::model::XAnim;
use crate::pmove::mantle::Clip;

/// One clip's root tracks and its rate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RootClip {
    trans: Clip,
    /// Key frames and the root's yaw as a half quaternion's (z, w).
    rot: Vec<(u16, [f32; 2])>,
    numframes: u16,
    /// `XAnim.frequency`: node time a second at rate 1.
    pub frequency: f32,
    pub looped: bool,
}

impl RootClip {
    /// The clip's root tracks; `None` for a clip without a root.
    pub fn of(x: &XAnim) -> Option<RootClip> {
        let root = x.root.as_ref()?;
        Some(RootClip {
            trans: Clip::of(x),
            rot: root
                .rot
                .iter()
                .map(|&(f, [z, w])| (f, [f32::from(z), f32::from(w)]))
                .collect(),
            numframes: x.num_frames,
            frequency: x.frequency,
            looped: x.looped,
        })
    }

    /// The node time `dtime` seconds on from `time` at rate 1: past 1 a
    /// clip that does not loop holds its end, one that loops wraps.
    pub fn step(&self, time: f32, dtime: f32) -> f32 {
        let t = (f64::from(time) + f64::from(dtime) * f64::from(self.frequency)) as f32;
        if t < 1.0 {
            t
        } else if self.looped {
            t.fract()
        } else {
            1.0
        }
    }

    /// The yaw half quaternion at fraction `t`, keys lerped.
    fn yaw(&self, t: f32) -> [f32; 2] {
        let Some(&(_, last)) = self.rot.last() else {
            return [0.0, 0.0];
        };
        if self.rot.len() == 1 || t >= 1.0 {
            return last;
        }
        let frame = t * f32::from(self.numframes);
        let k = self
            .rot
            .partition_point(|&(f, _)| f32::from(f) <= frame)
            .saturating_sub(1)
            .min(self.rot.len() - 2);
        let ((f0, a), (f1, b)) = (self.rot[k], self.rot[k + 1]);
        let s = ((frame - f32::from(f0)) / f32::from(f1.max(f0 + 1) - f0)).clamp(0.0, 1.0);
        [a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s]
    }

    /// The root's move from node time `old` to `new`, in the clip's frame
    /// at `new` (x forward, y left): the offset's difference, plus one
    /// whole pass of the clip when a looping one wrapped, turned back by
    /// the root's yaw at `new` (cod4-combat.md 5.2).
    pub fn delta(&self, old: f32, new: f32) -> Vec3 {
        let at = |t| self.trans.abs_delta(t);
        let mut travel = at(new) - at(old);
        if self.looped && new < old {
            travel += at(1.0) - at(0.0);
        }
        vcod_common::xanim::unturn_by_yaw(travel, self.yaw(new))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::model::DeltaPart;

    fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
        words.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, w| {
            (h ^ u64::from(w)).wrapping_mul(0x100_0000_01b3)
        })
    }

    fn clip(looped: bool, rot: Vec<(u16, [i16; 2])>) -> RootClip {
        let x = XAnim {
            name: "t".into(),
            num_frames: 10,
            looped,
            delta: true,
            bone_count: [0; 10],
            framerate: 30.0,
            frequency: 3.0,
            asset_type: 0,
            names: Vec::new(),
            notify: Vec::new(),
            data_byte: Vec::new(),
            data_short: Vec::new(),
            data_int: Vec::new(),
            random_data_short: Vec::new(),
            random_data_byte: Vec::new(),
            random_data_int: Vec::new(),
            indices: Vec::new(),
            root: Some(DeltaPart {
                trans: vec![(0, [0.0; 3]), (4, [0.0; 3]), (10, [0.0; 3])],
                rot,
                trans_mins: [-12.0, 3.0, -1.5],
                trans_size: [0.002, 0.0011, 0.0007],
                trans_quant: vec![[0, 0, 0], [30000, 100, 65535], [65535, 40000, 2000]],
            }),
        };
        RootClip::of(&x).unwrap()
    }

    /// The root's move over a grid of time pairs, wraps included, under
    /// no turn, one key and a turning track, bit for bit.
    #[test]
    fn pins_the_root_delta() {
        let clips = [
            clip(false, Vec::new()),
            clip(true, vec![(0, [9000, 30000])]),
            clip(
                true,
                vec![(0, [0, 32767]), (3, [5000, 32000]), (10, [23170, 23170])],
            ),
            clip(false, vec![(0, [-3000, 32600]), (10, [0, 0])]),
        ];
        let times = [0.0, 0.05, 0.3, 0.41, 0.77, 0.999, 1.0];
        let mut out = Vec::new();
        for c in &clips {
            for &a in &times {
                for &b in &times {
                    out.extend(c.delta(a, b).to_array().map(f32::to_bits));
                }
            }
        }
        assert_eq!(fnv(out), PIN_DELTA);
    }
    const PIN_DELTA: u64 = 7926085840049331376;
}
