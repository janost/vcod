//! `vc_bot_skill`'s levels: presets of [`Skill`]. Level 1 is
//! `Skill::default()`, the values every seeded bot gate was measured at.
//! docs/research/bot-combat.md section 3 has what each level measures at.

use super::Skill;

/// The level a bot plays at unless `vc_bot_skill` or `addbot ... skill`
/// says otherwise.
pub const DEFAULT_LEVEL: u8 = 1;
/// The highest level; a cvar or argument past it clamps here.
pub const MAX_LEVEL: u8 = 3;

impl Skill {
    /// The preset for `level`: 0 easy, 1 normal, 2 hard, 3 expert. Past
    /// [`MAX_LEVEL`] reads as it.
    pub fn level(level: u8) -> Skill {
        match level {
            0 => Skill {
                reaction_min: 8,
                reaction_spread: 6,
                forget_ticks: 16,
                turn_deg: 10.0,
                error_start: 64.0,
                // Under `HIT_RADIUS` standing: a floor at or past it holds
                // the trigger off for good.
                error_floor: 5.0,
                error_decay: 0.91,
                error_range: 900.0,
                error_moving: 1.9,
                track_lag: 0.09,
                flick_spread: 0.15,
                recoil_share: 0.5,
                recoil_ticks: 4,
                footwork: 0.25,
                memory_ticks: 70,
                noise_ticks: 140,
                hearing: 0.75,
                fov_deg: 100.0,
                walk_turn_deg: 18.0,
                ..Skill::default()
            },
            1 => Skill::default(),
            2 => Skill {
                reaction_min: 3,
                reaction_spread: 4,
                forget_ticks: 30,
                turn_deg: 22.0,
                error_start: 32.0,
                error_floor: 2.5,
                error_decay: 0.84,
                error_range: 1300.0,
                error_moving: 1.5,
                track_lag: 0.045,
                flick_spread: 0.075,
                recoil_share: 0.8,
                recoil_ticks: 2,
                footwork: 0.6,
                memory_ticks: 140,
                noise_ticks: 260,
                hearing: 1.25,
                fov_deg: 140.0,
                walk_turn_deg: 30.0,
                ..Skill::default()
            },
            _ => Skill {
                reaction_min: 2,
                reaction_spread: 2,
                forget_ticks: 40,
                turn_deg: 30.0,
                error_start: 24.0,
                error_floor: 1.5,
                error_decay: 0.8,
                error_range: 1600.0,
                error_moving: 1.3,
                track_lag: 0.03,
                flick_spread: 0.05,
                recoil_share: 0.9,
                recoil_ticks: 2,
                footwork: 0.75,
                memory_ticks: 180,
                noise_ticks: 300,
                hearing: 1.5,
                fov_deg: 160.0,
                walk_turn_deg: 36.0,
                ..Skill::default()
            },
        }
    }

    /// The level these values are the preset of, if any.
    pub fn level_of(&self) -> Option<u8> {
        (0..=MAX_LEVEL).find(|&l| Skill::level(l) == *self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_level_is_the_default_skill() {
        assert_eq!(Skill::level(DEFAULT_LEVEL), Skill::default());
        assert_eq!(Skill::default().level_of(), Some(DEFAULT_LEVEL));
    }

    #[test]
    fn each_level_is_no_worse_than_the_one_below() {
        for l in 1..=MAX_LEVEL {
            let (lo, hi) = (Skill::level(l - 1), Skill::level(l));
            assert!(hi.reaction_min + hi.reaction_spread < lo.reaction_min + lo.reaction_spread);
            assert!(hi.turn_deg > lo.turn_deg);
            assert!(hi.error_start < lo.error_start && hi.error_floor < lo.error_floor);
            assert!(hi.error_decay < lo.error_decay && hi.error_range > lo.error_range);
            assert!(hi.error_moving < lo.error_moving && hi.track_lag < lo.track_lag);
            assert!(hi.flick_spread < lo.flick_spread && hi.recoil_share > lo.recoil_share);
            assert!(hi.recoil_ticks <= lo.recoil_ticks);
            assert!(hi.footwork > lo.footwork);
            assert!(hi.forget_ticks > lo.forget_ticks && hi.memory_ticks > lo.memory_ticks);
            assert!(hi.noise_ticks > lo.noise_ticks && hi.hearing > lo.hearing);
            assert!(hi.fov_deg > lo.fov_deg);
            assert!(hi.walk_turn_deg > lo.walk_turn_deg);
        }
        assert_eq!(Skill::level(MAX_LEVEL + 5), Skill::level(MAX_LEVEL));
    }
}
