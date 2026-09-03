//! The one pseudo-random step the server draws everything from: the anim
//! clause a death or a pain picks, `randomFloat`, the bullet spread and the
//! connect challenge. Retail's own generator is glibc's `rand()`, which
//! nothing here has to reproduce -- only the shape of the draw and its
//! reproducibility from a seed.

/// One xorshift64* step: advances `state` and returns the scrambled output.
/// A zero state is the degenerate one xorshift never leaves, so a caller
/// seeds with anything else.
pub fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
