//! A bullet round's spread, shared by the server's `Bullet_Fire` and the
//! client's `DrawBulletImpacts` so both draw the same end for one shot
//! (docs/research/cod4-combat.md 3, cod4-effects.md 12.2): the spread's
//! two ends by stance, the amount between them by the aim spread scale,
//! and the end off the spread disc drawn from the seed.

use glam::Vec3;

use crate::pmove::PlayerState;
use crate::pmove::weapon::WeaponDef;

/// A single bullet's range; a spread weapon's pellets go to its
/// `minDamageRange` (`DrawBulletImpacts`, iw3mp.exe 0x456bca..0x456bec).
pub const BULLET_RANGE: f32 = 8192.0;
/// `WEAPCLASS_SPREAD`.
pub const WEAPCLASS_SPREAD: i32 = 3;

/// One step of the Park-Miller "minimal standard" generator, `seed *
/// 16807 mod (2^31 - 1)`, as 32-bit arithmetic that wraps: iw3mp.exe
/// takes `seed / 127773` by a magic multiply and subtracts that many
/// moduli from `seed * 16807` (0x4562d2..0x4562f5).
fn park_miller(seed: i32) -> i32 {
    const MODULUS: i32 = 0x7fff_ffff;
    let n = seed
        .wrapping_mul(16807)
        .wrapping_sub((seed / 127_773).wrapping_mul(MODULUS));
    if n < 0 { n.wrapping_add(MODULUS) } else { n }
}

/// `G_GoodRandomFloat` (iw3mp.exe 0x4562b0): a draw in [0, 1) off
/// `seed`, which it steps 41 times. The seed is negated, eight steps are
/// thrown away, the next 32 fill a shuffle table from its last entry
/// down, and one more step is left in `seed`; the draw is the entry the
/// first entry's top five bits pick, times 2^-31, kept under 1.
pub fn good_random(seed: &mut i32) -> f32 {
    // 2^-31 as a float: iw3mp.exe multiplies the integer by the double at
    // 0x70b500 on the x87, which lands a bit off this product for about
    // one draw in seventy
    const SCALE: f32 = 4.656_613e-10;
    // 0x70b4f8
    const BELOW_ONE: f32 = 0.999_999_9;
    *seed = park_miller(seed.wrapping_neg());
    for _ in 1..8 {
        *seed = park_miller(*seed);
    }
    let mut table = [0i32; 32];
    for entry in table.iter_mut().rev() {
        *seed = park_miller(*seed);
        *entry = *seed;
    }
    *seed = park_miller(*seed);
    let pick = table[(table[0] / 0x400_0000) as usize];
    (pick as f32 * SCALE).min(BELOW_ONE)
}

/// `Bullet_RandomDir`, the spread disc draw: a point in the unit
/// disc off `seed`.
pub fn random_dir(mut seed: i32) -> (f32, f32) {
    let theta = good_random(&mut seed) * 360.0;
    let r = good_random(&mut seed);
    let t = theta * 0.017_453_292;
    (r * t.cos(), r * t.sin())
}

/// `BG_GetSpreadForWeapon` (iw3mp.exe calls it at 0x4566bb):
/// the hip spread's two ends by the eye's height between prone (11),
/// crouched (40) and standing (60), the script's override, Steady Aim's
/// 0.65.
pub fn spread_for_weapon(ps: &PlayerState, d: &WeaponDef) -> (f32, f32) {
    if ps.spread_override_state == 2 {
        let s = ps.spread_override as f32;
        return (s, s);
    }
    let h = ps.view_height_current;
    let (min, mut max) = if h <= 40.0 {
        let f = (h - 11.0) / 29.0;
        (
            (d.hip_spread_ducked_min - d.hip_spread_prone_min) * f + d.hip_spread_prone_min,
            (d.hip_spread_ducked_max - d.hip_spread_prone_max) * f + d.hip_spread_prone_max,
        )
    } else {
        let f = (h - 40.0) / 20.0;
        (
            (d.hip_spread_stand_min - d.hip_spread_ducked_min) * f + d.hip_spread_ducked_min,
            (d.hip_spread_stand_max - d.hip_spread_ducked_max) * f + d.hip_spread_ducked_max,
        )
    };
    if ps.spread_override_state == 1 {
        max = ps.spread_override as f32;
    }
    if ps.perks & 2 != 0 {
        return (min * 0.65, max * 0.65);
    }
    (min, max)
}

/// The spread in degrees between `min` and `max` (`adsSpread` and `max`
/// in the sight) by `scale`, the aim spread scale over 255
/// (iw3mp.exe 0x456b6f..0x456b81, 0x45682d..0x456842).
pub fn spread_amount(d: &WeaponDef, (min, max): (f32, f32), scale: f32, ads: bool) -> f32 {
    if ads {
        (max - d.ads_spread) * scale + d.ads_spread
    } else {
        (max - min) * scale + min
    }
}

/// How many rounds a shot fires and how far each goes: a spread weapon
/// its `shotCount` to `minDamageRange`, anything else one to 8192.
pub fn shots_and_range(d: &WeaponDef) -> (i32, f32) {
    if d.weap_class == WEAPCLASS_SPREAD {
        (d.shot_count.max(1), d.min_damage_range)
    } else {
        (1, BULLET_RANGE)
    }
}

/// `CG_BulletEndpos` / `Bullet_Endpos` (iw3mp.exe 0x456470,
/// The spread disc draw at 0x4a66e0): `range` along `axis[0]` from `start`,
/// pushed along the right and up axes by the disc's point times
/// `tan(spread) * range`.
pub fn bullet_end(seed: i32, spread: f32, start: Vec3, axis: &[Vec3; 3], range: f32) -> Vec3 {
    let aim = (spread * 0.017_453_292).tan() * range;
    let (x, y) = random_dir(seed);
    start + axis[0] * range + axis[1] * (x * aim) + axis[2] * (y * aim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_draws_the_same_point_inside_the_disc() {
        let a = random_dir(12345);
        assert_eq!(a, random_dir(12345));
        assert_ne!(a, random_dir(12346));
        for s in 0..200 {
            let (x, y) = random_dir(s * 977);
            assert!(x * x + y * y <= 1.0);
        }
    }

    #[test]
    fn no_spread_ends_on_the_view() {
        let axis = [Vec3::X, -Vec3::Y, Vec3::Z];
        let e = bullet_end(77, 0.0, Vec3::ZERO, &axis, 100.0);
        assert!((e - Vec3::X * 100.0).length() < 1e-4);
        let e = bullet_end(77, 3.0, Vec3::ZERO, &axis, 100.0);
        let off = (e - Vec3::X * 100.0).length();
        assert!(off > 0.0 && off <= (3.0f32).to_radians().tan() * 100.0 + 1e-3);
    }

    /// FNV-1a over a stream of words: the pins below hash every output's bits.
    fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
        words.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, w| {
            (h ^ u64::from(w)).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// The shuffle's draws and the seed it leaves, bit for bit.
    #[test]
    fn pins_the_good_random_draws() {
        let mut out = Vec::new();
        for s in [
            0,
            1,
            -1,
            12345,
            i32::MAX,
            i32::MIN,
            977,
            -40_000,
            0x0400_0000,
        ] {
            for k in 0..64 {
                let mut seed = s.wrapping_add(k * 7919);
                out.push(good_random(&mut seed).to_bits());
                out.push(seed as u32);
                out.push(good_random(&mut seed).to_bits());
            }
        }
        assert_eq!(fnv(out), PIN_DRAWS);
    }
    const PIN_DRAWS: u64 = 13839422206756683158;
}
