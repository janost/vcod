//! United Offensive's sound blend entities (`eType` 9): a vehicle's engine
//! and its MG loops, two aliases crossfaded by `angles2[1]`
//! (docs/research/coduo-diff.md 16).

use std::collections::HashMap;

use glam::Vec3;
use vcod_common::net::flags::{EF_NODRAW, ET_MISSILE, ET_SOUND_BLEND_V22, PM_INTERMISSION};
use vcod_common::net::protocol::{Cod1Wire, ENTITYNUM_NONE};
use vcod_common::net::snapshot::Snapshot;
use vcod_common::net::trajectory::Trajectory;
use vcod_common::weapon::WeaponDef;

/// One blend entity's sound this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendSource {
    pub entity: u32,
    /// The two alias names, `CS_SOUNDS + eventParm` and `CS_SOUNDS +
    /// es.scale`.
    pub aliases: [String; 2],
    /// The second alias's share, 0..1.
    pub blend: f32,
    /// A playback rate factor, 1 unless the game packed a pitch into the
    /// blend ([`split`]).
    pub pitch_scale: f32,
    pub pos: Vec3,
}

/// The cgame's lerp of `angles2[1]` over the snapshot pair (uo cgame
/// 0x30021860): a plain lerp from 0 up; below 0 the whole and the
/// fractional parts of `|v|` lerp apart, the fraction held under 0.99 (at
/// or past it, 1), and the sum comes back negated.
pub fn lerp_blend(cur: f32, next: f32, frac: f32) -> f32 {
    if next >= 0.0 {
        return (next - cur) * frac + cur;
    }
    let (a, b) = (next.abs(), cur.abs());
    let fract = |v: f32| v - v.floor();
    let mut part = (fract(a) - fract(b)) * frac + fract(b);
    if part >= 0.99 {
        part = 1.0;
    }
    let whole = ((a.floor() - b.floor()) * frac + b.floor()).floor();
    -whole - part
}

/// Syscall 199's reading of the lerped value (`CoDUOMP.exe` 0x452ba0): from
/// 0 up the blend itself at pitch 1; below 0 the fraction of `|v|` is the
/// blend and its whole part a pitch in hundredths, how
/// `G_SetSoundBlendAndPitch` packs one (no stock code calls it).
pub fn split(v: f32) -> (f32, f32) {
    if v >= 0.0 {
        return (v, 1.0);
    }
    let a = v.abs();
    (a - a.floor(), a.floor() * 0.01)
}

/// Every blend entity of the snapshot pair at `frac`, as the cgame's case 9
/// hands it to syscall 199. `a` is the older snapshot (`cg.snap`), whose
/// entities play; one missing from `b` holds its value. An entity tied to
/// another (`otherEntityNum`, the vehicle) plays at that one's drawn
/// position (`CG_CalcEntityLerpPositions` 0x30021d30 copies its
/// `lerpOrigin`), else at its own. Under intermission only the untied
/// ones play. Either alias offset 0 or a missing configstring plays
/// nothing.
pub fn sources(
    p: &Cod1Wire,
    (a, b, frac): (&Snapshot, &Snapshot, f32),
    cs_sounds: usize,
    configstrings: &[String],
    entity_pos: &HashMap<u32, Vec3>,
) -> Vec<BlendSource> {
    let intermission = a.ps.field_i32(p, "pm_type") == PM_INTERMISSION;
    let mut out = Vec::new();
    for (&num, e) in &a.entities {
        if e.field_i32(p, "eType") != ET_SOUND_BLEND_V22 {
            continue;
        }
        let owner = e.field_i32(p, "otherEntityNum") as u32;
        if intermission && owner != ENTITYNUM_NONE {
            continue;
        }
        let alias = |field: &str| -> Option<String> {
            let off = e.field_i32(p, field);
            if off == 0 {
                return None;
            }
            let name = configstrings.get(cs_sounds + usize::try_from(off).ok()?)?;
            (!name.is_empty()).then(|| name.clone())
        };
        let (Some(first), Some(second)) = (alias("eventParm"), alias("_union.scale")) else {
            continue;
        };
        let cur = e.field_f32(p, "angles2[1]");
        let next = b
            .entities
            .get(&num)
            .map_or(cur, |n| n.field_f32(p, "angles2[1]"));
        let (blend, pitch_scale) = split(lerp_blend(cur, next, frac));
        let pos = match owner {
            ENTITYNUM_NONE => entity_pos.get(&num).copied(),
            o => entity_pos
                .get(&o)
                .copied()
                .or_else(|| a.entities.get(&o).map(|e| Vec3::from(e.origin(p)))),
        }
        .unwrap_or_else(|| Vec3::from(e.origin(p)));
        out.push(BlendSource {
            entity: num,
            aliases: [first, second],
            blend,
            pitch_scale,
            pos,
        });
    }
    out
}

/// A shell's speed and closing rate are held to this, units per second
/// (uo cgame 0x3007c144, 0x3007c140).
const SHELL_MAX_SPEED: f32 = 4000.0;
/// Inside this distance the closing rate fades with it (0x3007c13c,
/// 0x3007c138).
const SHELL_NEAR: f32 = 700.0;

/// The missile case's blend (uo cgame 0x3001edb0): the share of the
/// second alias for a shell at `pos` moving at `vel` (its `pos.trDelta`)
/// heard from `view`. The velocity is held to 4000 units a second, its
/// component toward the view clamped to ±4000 and, inside 700 units,
/// scaled by the distance over 700; `(closing / 4000 + 1) / 2` is the
/// blend, 1 for a shell flying straight at the view, 0 for one flying
/// away.
pub fn shell_blend(pos: Vec3, vel: Vec3, view: Vec3) -> f32 {
    let to_view = view - pos;
    let dist = to_view.length();
    let dir = to_view.normalize_or_zero();
    let vel = if vel.length() > SHELL_MAX_SPEED {
        vel.normalize_or_zero() * SHELL_MAX_SPEED
    } else {
        vel
    };
    let mut closing = vel.dot(dir).clamp(-SHELL_MAX_SPEED, SHELL_MAX_SPEED);
    if dist < SHELL_NEAR {
        closing *= dist / SHELL_NEAR;
    }
    (closing / SHELL_MAX_SPEED + 1.0) * 0.5
}

/// Every drawn missile of `a` (`cg.snap`) whose weapon names a
/// `projectileSoundBlend1`/`2` pair: United Offensive's tank and gun
/// shells, `artillery_whiz_low` and `_high`, at the missile's drawn
/// position and [`shell_blend`]'s share, through the same syscall 199 as
/// a blend entity (uo cgame 0x3001edb0, the call at 0x3001efa1;
/// docs/research/coduo-diff.md 16). A missile with `EF_NODRAW` plays
/// nothing.
pub fn shell_sources(
    p: &Cod1Wire,
    a: &Snapshot,
    weapons: &[Option<WeaponDef>],
    entity_pos: &HashMap<u32, Vec3>,
    view: Vec3,
) -> Vec<BlendSource> {
    let mut out = Vec::new();
    for (&num, e) in &a.entities {
        if e.field_i32(p, "eType") != ET_MISSILE || e.field_i32(p, "eFlags") & EF_NODRAW != 0 {
            continue;
        }
        let Some(pair) = usize::try_from(e.field_i32(p, "weapon"))
            .ok()
            .and_then(|w| weapons.get(w)?.as_ref())
            .and_then(|d| d.sounds.projectile_blend.clone())
        else {
            continue;
        };
        let pos = entity_pos
            .get(&num)
            .copied()
            .unwrap_or_else(|| Vec3::from(e.origin(p)));
        let vel = Trajectory::read(e, p, "pos").delta;
        out.push(BlendSource {
            entity: num,
            aliases: pair,
            blend: shell_blend(pos, vel, view),
            pitch_scale: 1.0,
            pos,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_blend_lerps_and_plays_at_pitch_one() {
        assert_eq!(lerp_blend(0.25, 0.75, 0.5), 0.5);
        assert_eq!(split(0.5), (0.5, 1.0));
        assert_eq!(split(0.0), (0.0, 1.0));
    }

    /// A shell flying at the view plays the second alias, one flying away
    /// the first, one passing across the even mix; inside 700 units the
    /// closing rate fades with the distance.
    #[test]
    fn a_shell_blends_on_its_closing_rate() {
        let view = Vec3::ZERO;
        let at = |x: f32, vx: f32, vy: f32| {
            shell_blend(Vec3::new(x, 0.0, 0.0), Vec3::new(vx, vy, 0.0), view)
        };
        assert_eq!(at(2000.0, -9000.0, 0.0), 1.0);
        assert_eq!(at(2000.0, 9000.0, 0.0), 0.0);
        assert_eq!(at(2000.0, 0.0, 3000.0), 0.5);
        assert!((at(2000.0, -2000.0, 0.0) - 0.75).abs() < 1e-6);
        assert!((at(350.0, -4000.0, 0.0) - 0.75).abs() < 1e-6);
    }

    /// `G_SetSoundBlendAndPitch`'s packing, `-floor(100 * pitch) - blend`:
    /// pitch 1.2 at blend 0.25 reads back whole.
    #[test]
    fn a_packed_pitch_splits_off_the_blend() {
        let v = -120.25;
        let (blend, pitch) = split(lerp_blend(v, v, 0.3));
        assert!((blend - 0.25).abs() < 1e-3, "{blend}");
        assert!((pitch - 1.2).abs() < 1e-6, "{pitch}");
        // A lerped fraction of 0.99 or more reads as a whole 1.
        assert_eq!(lerp_blend(-100.5, -100.995, 1.0), -101.0);
    }
}
