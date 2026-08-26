//! Server-side spectator state: usercmds in, wire playerstate out.

use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg::{self, UserCmd};
use vcod_common::net::protocol::Protocol;
use vcod_common::pmove;

/// ANGLE2SHORT units per degree (codextended shared.h).
const ANGLE2SHORT: f32 = 65536.0 / 360.0;

fn short_deg(v: i32) -> f32 {
    let deg = v as f32 / ANGLE2SHORT;
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// One spectator's simulated state: Q3 spectator fly
/// ([`pmove::spectator_move`]), angles straight off the latest cmd.
pub struct SpectatorSim {
    pub ps: pmove::PlayerState,
}

impl SpectatorSim {
    pub fn new(origin: [f32; 3], yaw_deg: f32) -> Self {
        SpectatorSim {
            ps: pmove::PlayerState::spawn(Vec3::from(origin), yaw_deg),
        }
    }

    /// Advance one frame. Without a collision world the state stays put; the
    /// axes arrive quantized to ±127/0 and dt comes off the frame clock.
    pub fn step(&mut self, cmd: &UserCmd, world: Option<&CollisionWorld>, dt: f32) {
        let Some(world) = world else { return };
        self.ps.yaw = short_deg(cmd.angles[1]).to_radians();
        // Wire pitch is positive down; the sim stores the camera's convention.
        self.ps.pitch = -short_deg(cmd.angles[0]).to_radians();
        pmove::spectator_move(
            &mut self.ps,
            f32::from(cmd.forward) / 127.0,
            f32::from(cmd.right) / 127.0,
            f32::from(cmd.up) / 127.0,
            world,
            dt,
        );
    }

    /// The wire playerstate of a captured retail spectator, pinned in the
    /// plan header; dynamic fields come from the sim. `commandTime` mirrors
    /// the last processed cmd's server time.
    pub fn to_wire(&self, p: &Protocol, client_num: i32, command_time: i32) -> msg::PlayerState {
        let mut w = msg::PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            w.fields[msg::PlayerState::field_index(p, name).unwrap()] = v;
        };
        // Pinned capture values; provenance in the design doc.
        set("pm_type", 4);
        set("speed", pmove::SPEED_SPECTATOR as i32);
        set("eFlags", 24);
        set("clientNum", client_num);
        set("commandTime", command_time);
        set("mins[0]", (-15f32).to_bits() as i32);
        set("mins[1]", (-15f32).to_bits() as i32);
        set("maxs[0]", 15f32.to_bits() as i32);
        set("maxs[1]", 15f32.to_bits() as i32);
        set("maxs[2]", 70f32.to_bits() as i32);
        set("proneViewHeight", 11);
        set("crouchViewHeight", 40);
        set("standViewHeight", 60);
        set("deadViewHeight", 8);
        set("walkSpeedScale", 0.4f32.to_bits() as i32);
        set("runSpeedScale", 1f32.to_bits() as i32);
        set("proneSpeedScale", 0.15f32.to_bits() as i32);
        set("crouchSpeedScale", 0.65f32.to_bits() as i32);
        set("strafeSpeedScale", 0.8f32.to_bits() as i32);
        set("backSpeedScale", 0.7f32.to_bits() as i32);
        set("leanSpeedScale", 0.4f32.to_bits() as i32);
        set("friction", 1f32.to_bits() as i32);
        // Dynamic.
        for (i, axis) in ["origin[0]", "origin[1]", "origin[2]"].iter().enumerate() {
            set(axis, self.ps.origin[i].to_bits() as i32);
        }
        for (i, axis) in ["velocity[0]", "velocity[1]", "velocity[2]"]
            .iter()
            .enumerate()
        {
            set(axis, self.ps.velocity[i].to_bits() as i32);
        }
        let pitch = -self.ps.pitch.to_degrees(); // wire convention
        let yaw = self.ps.yaw.to_degrees();
        set("viewangles[0]", pitch.to_bits() as i32);
        set("viewangles[1]", yaw.to_bits() as i32);
        // Retail keeps delta_angles = sent view - cmd angles; both sides come
        // off the same cmd here, so prediction matches what we send.
        set(
            "delta_angles[0]",
            ((pitch * ANGLE2SHORT).round() as i32) & 0xffff,
        );
        set(
            "delta_angles[1]",
            ((yaw * ANGLE2SHORT).round() as i32) & 0xffff,
        );
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::bsp;
    use vcod_common::net::protocol::PROTOCOL_V1;

    fn cmd(forward: i8, pitch_short: i32, yaw_short: i32) -> UserCmd {
        UserCmd {
            forward,
            angles: [pitch_short, yaw_short, 0],
            ..Default::default()
        }
    }

    #[test]
    fn wire_carries_the_pinned_constants_and_dynamic_fields() {
        let p = &PROTOCOL_V1;
        let sim = SpectatorSim::new([10.0, 20.0, 30.0], 90.0);
        let w = sim.to_wire(p, 3, 114_800);
        assert_eq!(w.field_i32(p, "pm_type"), 4);
        assert_eq!(w.field_i32(p, "speed"), 400);
        assert_eq!(w.field_i32(p, "clientNum"), 3);
        assert_eq!(w.field_i32(p, "commandTime"), 114_800);
        assert_eq!(w.field_i32(p, "eFlags"), 24);
        assert_eq!(w.field_f32(p, "origin[0]"), 10.0);
        assert_eq!(w.field_f32(p, "origin[2]"), 30.0);
        // View heights are -8-bit int fields on the wire, not float bits.
        assert_eq!(w.field_i32(p, "standViewHeight"), 60);
        assert_eq!(w.field_i32(p, "crouchViewHeight"), 40);
        assert_eq!(w.field_i32(p, "proneViewHeight"), 11);
        assert_eq!(w.field_i32(p, "deadViewHeight"), 8);
        assert_eq!(w.field_f32(p, "walkSpeedScale"), 0.4);
        assert_eq!(w.field_i32(p, "mins[0]"), (-15f32).to_bits() as i32);
        assert_eq!(w.field_f32(p, "maxs[2]"), 70.0);
        assert_eq!(w.field_i32(p, "gravity"), 0);
        assert_eq!(w.field_i32(p, "bobCycle"), 0);
    }

    #[test]
    fn viewangles_follow_the_client_camera_convention() {
        let p = &PROTOCOL_V1;
        let mut sim = SpectatorSim::new([0.0; 3], 0.0);
        sim.ps.yaw = 90f32.to_radians();
        // Sim pitch is camera convention (up positive); the wire is down
        // positive, so a sim looking 45 deg up writes viewangles[0] = -45.
        sim.ps.pitch = 45f32.to_radians();
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.viewangles(p)[1], 90.0);
        assert_eq!(w.viewangles(p)[0], -45.0);
    }

    #[test]
    fn delta_angles_track_the_sent_view() {
        let p = &PROTOCOL_V1;
        let mut sim = SpectatorSim::new([0.0; 3], 0.0);
        sim.ps.yaw = 90f32.to_radians();
        let w = sim.to_wire(p, 0, 0);
        let expected_yaw = (90.0f32 * ANGLE2SHORT).round() as i32 & 0xffff;
        assert_eq!(w.field_i32(p, "delta_angles[1]") & 0xffff, expected_yaw);
        assert_eq!(w.field_i32(p, "delta_angles[0]"), 0);
    }

    #[test]
    fn step_turns_cmds_into_motion_on_a_real_map() {
        let Some(data) = vcod_common::testing::real_bsp() else {
            return;
        };
        let parsed = bsp::parse(&data).unwrap();
        let world = CollisionWorld::build(&parsed, &[]);
        let mut sim = SpectatorSim::new([0.0, 0.0, 64.0], 0.0);
        let c = cmd(127, 0, 0);
        for _ in 0..125 {
            sim.step(&c, Some(&world), 1.0 / 125.0);
        }
        assert!(
            sim.ps.velocity.truncate().length() > 250.0,
            "v {:?}",
            sim.ps.velocity
        );
    }

    #[test]
    fn step_without_a_world_freezes_the_sim() {
        let mut sim = SpectatorSim::new([1.0, 2.0, 3.0], 0.0);
        sim.step(&cmd(127, 0, 0), None, 0.05);
        assert_eq!(sim.ps.origin, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(sim.ps.velocity, Vec3::ZERO);
    }
}
