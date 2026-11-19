//! The ladder on CoD2 1.3: [`vcod_common::pmladder`] over [`Pmove`] with
//! the stored helpers `cod2_lnxded` calls (docs/research/cod2-movement.md
//! 11).

use glam::Vec3;
use vcod_common::pmladder::{LadderHit, LadderPm};
pub(crate) use vcod_common::pmladder::{
    cap_yaw, check_ladder_move, ladder_move, push_off, rung_footstep,
};

use super::fpu;
use super::moves::{accelerate, air_move, cmd_scale_flat, jump_check};
use super::slide::step_slide_move;
use super::{PlayerState, Pml, Pmove, Tr};

impl LadderPm for Pmove<'_> {
    type Ps = PlayerState;
    type Tr = Tr;

    const PRESS_NEEDS_MOTION: bool = false;

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
        f64::from(cmd_scale_flat(self.ps, fm, rm, &self.rules))
    }
    fn push_delta_yaw(&mut self, excess: f32) {
        self.ps.delta_angles[1] = self.ps.delta_angles[1].wrapping_add(fpu::shorts(excess));
    }
    fn rung_event(&mut self, surf: u32) {
        self.ps.add_event(surf as i32 + 1, 0)
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
        fpu::normalize360_accurate(a)
    }
}
