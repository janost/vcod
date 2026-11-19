//! The ladder on CoD4 1.7: [`vcod_common::pmladder`] over [`Pmove`] with
//! the kept helpers `cod4_lnxded` calls (cod4-pmove.md 7, 8 and 9).

use glam::Vec3;
use vcod_common::pmladder::{LadderHit, LadderPm};
pub(crate) use vcod_common::pmladder::{
    cap_yaw, check_ladder_move, ladder_move, push_off, rung_footstep,
};

use super::fpu::{self, d};
use super::moves::{accelerate, air_move, free_scale, jump_check};
use super::slide::step_slide_move;
use super::{PlayerState, Pml, Pmove, Tr, ev};

impl LadderPm for Pmove<'_> {
    type Ps = PlayerState;
    type Tr = Tr;

    const PRESS_NEEDS_MOTION: bool = true;

    fn ps(&mut self) -> &mut PlayerState {
        self.ps
    }
    fn bounds(&self) -> (Vec3, Vec3) {
        (self.mins, self.maxs)
    }
    fn tracemask(&self) -> u32 {
        self.tracemask
    }
    fn cmd(&self) -> (i32, i8, i8) {
        (
            self.cmd.server_time,
            self.cmd.forward_move,
            self.cmd.right_move,
        )
    }
    fn ladder_yawcap(&self) -> f32 {
        self.rules.ladder_yawcap
    }
    fn ladder_push_vel(&self) -> f32 {
        self.rules.jump_ladder_push_vel
    }
    fn ladder_trace(
        &mut self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> LadderHit {
        let t = self.trace_box(start, end, mins, maxs, mask);
        LadderHit {
            fraction: t.fraction,
            normal: t.normal,
            surface_flags: t.surface_flags,
        }
    }

    fn jump_check(&mut self, pml: &mut Pml) -> bool {
        jump_check(self, pml)
    }
    fn air_move(&mut self, pml: &mut Pml) {
        air_move(self, pml)
    }
    fn accelerate(&mut self, pml: &Pml, wishdir: Vec3, wishspeed: f32, accel: f32) {
        accelerate(self, pml, wishdir, wishspeed, accel)
    }
    fn step_slide_move(&mut self, pml: &mut Pml) {
        step_slide_move(self, pml, false)
    }
    fn jump_anim(&mut self) {
        super::anim::event(self, super::anim::Event::Jump, false, true)
    }

    fn cmd_scale(&mut self, fm: i8, rm: i8) -> f64 {
        free_scale(self.ps, &self.rules, &[f32::from(fm), f32::from(rm)])
    }
    fn push_delta_yaw(&mut self, excess: f32) {
        self.ps.delta_angles[1] = (d(self.ps.delta_angles[1]) + d(excess)) as f32;
    }
    fn rung_event(&mut self, surf: u32) {
        self.ps.add_event(ev::FOOTSTEP_RUN, surf)
    }

    fn normalize(v: Vec3) -> (Vec3, f32) {
        fpu::normalize(v)
    }
    fn normalize_to(v: Vec3) -> (Vec3, f32) {
        fpu::normalize_to(v)
    }
    fn vectoyaw(v: Vec3) -> f32 {
        fpu::vectoyaw(v)
    }
    fn angle_delta(a: f32, b: f32) -> f32 {
        fpu::angle_delta(a, b)
    }
    fn angle_normalize360(a: f32) -> f32 {
        fpu::angle_normalize360(a)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use glam::Vec3;
    use vcod_common::animtree::PlayerAnims;
    use vcod_common::asset_source::{AssetCache, AssetSource};
    use vcod_common::game::Game;

    use super::super::anim::{ANIM_TOGGLEBIT, AnimInput};
    use super::super::{Cmd, ENTITYNUM_NONE, PlayerState, PmWorld, Pmove, Rules, Tr, pmf, pmove};

    /// The player tree and script as loose text.
    struct Text(HashMap<&'static str, &'static str>, AssetCache);

    impl AssetSource for Text {
        fn game(&self) -> Game {
            Game::Cod4
        }
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.get(path).map(|s| s.as_bytes().to_vec())
        }
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn list(&self, _: &str, _: &str) -> Vec<String> {
            Vec::new()
        }
        fn cache(&self) -> &AssetCache {
            &self.1
        }
    }

    /// Nothing to meet: every trace runs its full length.
    struct Open;

    impl PmWorld for Open {
        fn trace(&self, _: Vec3, _: Vec3, _: Vec3, _: Vec3, _: u32, _: u32) -> Tr {
            Tr::default()
        }
    }

    /// A body held on the rungs in the air whose probe meets no ladder
    /// lets go and raises the jump anim, forced past the legs' timer
    /// (cod4-pmove.md 7, 0x805a0a7..0x805a0cc).
    #[test]
    fn letting_go_in_the_air_forces_the_jump_anim() {
        let fs = Text(
            HashMap::from([
                ("animtrees/multiplayer.atr", "main { legs { pb_letgo } }"),
                (
                    "mp/playeranim.script",
                    "ANIMATIONS\nSTATE COMBAT\n{\n}\nEVENTS\njump\n{\n\tdefault\n\t{\n\t\tlegs pb_letgo duration 300\n\t}\n}\n",
                ),
            ]),
            AssetCache::default(),
        );
        let anims = PlayerAnims::load(&fs).unwrap();
        let letgo = anims.wire_of("pb_letgo").unwrap() as u32;
        let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, 64.0), 0.0, 1000);
        ps.pm_flags |= pmf::LADDER;
        ps.ground_entity_num = ENTITYNUM_NONE;
        ps.ladder_vec = Vec3::NEG_X;
        ps.legs_timer = 500;
        let cmd = Cmd {
            server_time: 1050,
            ..Default::default()
        };
        let old = Cmd {
            server_time: 1000,
            ..Default::default()
        };
        let mut pm = Pmove::new(&mut ps, cmd, old, &Open, Rules::default());
        pm.anim = Some(AnimInput {
            anims: &anims,
            player_anim_type: 0,
            weapon_class: 0,
        });
        pmove(&mut pm);
        assert_eq!(
            ps.pm_flags & (pmf::LADDER | pmf::LADDER_LEFT),
            pmf::LADDER_LEFT
        );
        assert_eq!(ps.legs_anim & !ANIM_TOGGLEBIT, letgo);
        assert_eq!(ps.legs_timer, 350);
    }
}
