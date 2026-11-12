//! The one pseudo-random step the server draws everything from: the anim
//! clause a death or a pain picks, `randomFloat`, the bullet spread and the
//! connect challenge. Retail's own generator is glibc's `rand()`, which
//! nothing here has to reproduce -- only the shape of the draw and its
//! reproducibility from a seed. [`GlibcRand`] is that generator, for a test
//! that replays a capture's draws from the seed recovered off it;
//! [`HoldRand`] is the second stream CoD2's game keeps.

/// One xorshift64* step: advances `state` and returns the scrambled output.
/// A zero state is the degenerate one xorshift never leaves, so a caller
/// seeds with anything else.
pub fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

/// glibc's `srand` / `rand` (the default `TYPE_3` additive generator), the
/// libc the retail Linux servers link. Their `G_InitGame` seeds it with
/// `srand(Com_Milliseconds())` (map-cycle doc 4.8), so a capture's draws
/// replay from the seed that fits them (player-clip doc 13.4).
#[derive(Clone, Debug)]
pub struct GlibcRand {
    r: [i32; 34],
    i: usize,
}

impl GlibcRand {
    /// `srand(seed)`: a seed of 0 is 1, as glibc's.
    pub fn new(seed: u32) -> Self {
        let mut r = [0i32; 34];
        r[0] = if seed == 0 { 1 } else { seed as i32 };
        for i in 1..31 {
            // 16807 * r mod (2^31 - 1), by Schrage's method as glibc's.
            let (hi, lo) = (r[i - 1] / 127773, r[i - 1] % 127773);
            let w = 16807 * lo - 2836 * hi;
            r[i] = if w < 0 { w + 2147483647 } else { w };
        }
        for i in 31..34 {
            r[i] = r[i - 31];
        }
        let mut g = GlibcRand { r, i: 0 };
        for _ in 0..310 {
            g.step();
        }
        g
    }

    fn step(&mut self) -> i32 {
        // The ring holds the last 34 words; word n is n-31 plus n-3.
        let n = self.i % 34;
        let v = self.r[(n + 3) % 34].wrapping_add(self.r[(n + 31) % 34]);
        self.r[n] = v;
        self.i += 1;
        v
    }

    /// `rand()`: 31 bits, `[0, RAND_MAX]`.
    pub fn next_u31(&mut self) -> u32 {
        (self.step() as u32) >> 1
    }
}

/// q_shared's `holdrand` stream (`Rand_Init`, `flrand`, `irand`): the
/// MSVC-style LCG CoD2 1.3's game draws its rocket wobble, grenade tumble
/// and script `randomInt`/`randomFloat` from, beside glibc's `rand()`.
/// `G_InitGame` seeds both with one wall-clock seed (cod2-combat.md 2.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HoldRand(pub u32);

impl HoldRand {
    fn step(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        self.0 >> 17
    }

    /// `flrand(min, max)` (lnxded 0x80aaa84): `(max - min) * (holdrand >>
    /// 17) / 32768 + min` on the stack, stored by the caller.
    pub fn flrand(&mut self, min: f32, max: f32) -> f32 {
        let v = f64::from(self.step());
        ((f64::from(max) - f64::from(min)) * v / 32768.0 + f64::from(min)) as f32
    }

    /// `irand(min, max)` (lnxded 0x80aaada): `(max - min) * (holdrand >>
    /// 17) >> 15`, plus `min`.
    pub fn irand(&mut self, min: i32, max: i32) -> i32 {
        let v = self.step() as i32;
        (max.wrapping_sub(min).wrapping_mul(v) >> 15) + min
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Rand_Init(seed)` then `flrand(-1, 1)` three times: the draws one
    /// retail rocket's first wobble took (cod2-combat.md 9.7).
    #[test]
    fn holdrand_draws_retail_rocket_wobble() {
        let mut h = HoldRand(3_500_530_290);
        let v: Vec<f32> = (0..3).map(|_| h.flrand(-1.0, 1.0)).collect();
        assert_eq!(v, [5576, 3961, 23305].map(|n| n as f32 / 16384.0 - 1.0));
    }

    /// A generator, not a constant: the sequence advances, repeats from a
    /// seed, and spreads over the whole word.
    #[test]
    fn the_step_advances_and_repeats_from_a_seed() {
        let mut a = 1u64;
        let first: Vec<u64> = (0..8).map(|_| xorshift(&mut a)).collect();
        let mut b = 1u64;
        let second: Vec<u64> = (0..8).map(|_| xorshift(&mut b)).collect();
        assert_eq!(first, second);
        assert_eq!(
            first
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            8
        );
        let mut c = 0x9e37_79b9_7f4a_7c15u64;
        let high = (0..64).filter(|_| xorshift(&mut c) >> 63 == 1).count();
        assert!(
            (16..48).contains(&high),
            "{high} of 64 draws had the top bit set"
        );
    }

    /// glibc 2.x's `srand(1)` sequence, and its seed 0 read as 1.
    #[test]
    fn glibc_rand_matches_glibc() {
        let mut g = GlibcRand::new(1);
        let first: Vec<u32> = (0..5).map(|_| g.next_u31()).collect();
        assert_eq!(
            first,
            [1804289383, 846930886, 1681692777, 1714636915, 1957747793]
        );
        let mut z = GlibcRand::new(0);
        assert_eq!(z.next_u31(), 1804289383);
    }
}
