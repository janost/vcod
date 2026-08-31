//! Server-side client state: usercmds in, wire playerstate out.

use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg::{self, UserCmd};
use vcod_common::net::protocol::{Protocol, ENTITYNUM_NONE, ENTITYNUM_WORLD};
use vcod_common::pmove::{self, PmInput};

/// ANGLE2SHORT units per degree (codextended shared.h).
const ANGLE2SHORT: f32 = 65536.0 / 360.0;

fn short_deg(v: i32) -> f32 {
    let deg = v as f32 / ANGLE2SHORT;
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// The movement path a client is on, and with it the half of the wire
/// playerstate that a spectator and a player disagree about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PmType {
    /// `pm_type` 0 on the wire.
    Normal,
    /// `pm_type` 4 on the wire, the value every client carries before it
    /// answers the team menu.
    Spectator,
}

/// One client's simulated state. `pm_type` selects the movement path, the way
/// retail's own `playerState_t` does: a client is a spectator before the menu
/// and a player after, within one connection.
pub struct ClientSim {
    pub ps: pmove::PlayerState,
    pub pm_type: PmType,
}

impl ClientSim {
    /// A client entering the world: Q3 spectator fly
    /// ([`pmove::spectator_move`]), angles straight off the latest cmd.
    pub fn spectator(origin: [f32; 3], yaw_deg: f32) -> Self {
        ClientSim {
            ps: pmove::PlayerState::spawn(Vec3::from(origin), yaw_deg),
            pm_type: PmType::Spectator,
        }
    }

    /// The mode change `spawnPlayer()` makes: the same sim, restarted at the
    /// spawn point the script chose.
    pub fn become_player(&mut self, origin: [f32; 3], yaw_deg: f32) {
        self.ps = pmove::PlayerState::spawn(Vec3::from(origin), yaw_deg);
        self.pm_type = PmType::Normal;
    }

    /// Advance one frame. The axes arrive quantized to ±127/0 and dt comes off
    /// the cmd clocks.
    pub fn step(&mut self, cmd: &UserCmd, dt: f32, world: Option<&CollisionWorld>) {
        self.ps.yaw = short_deg(cmd.angles[1]).to_radians();
        // Wire pitch is positive down; the sim stores the camera's convention.
        self.ps.pitch = -short_deg(cmd.angles[0]).to_radians();
        match (self.pm_type, world) {
            // A spectator noclips, so it needs no world. `(Normal, None)` is
            // the two cases where a player has none either: a unit test that
            // mounts no map, and a server whose world failed to load, which
            // `Server::FALLBACK_SPAWN` keeps running. Both fly rather than
            // collide, so a player on a failed load noclips.
            (PmType::Spectator, _) | (PmType::Normal, None) => pmove::spectator_move(
                &mut self.ps,
                f32::from(cmd.forward) / 127.0,
                f32::from(cmd.right) / 127.0,
                f32::from(cmd.up) / 127.0,
                dt,
            ),
            (PmType::Normal, Some(w)) => {
                let input = PmInput {
                    forward: f32::from(cmd.forward) / 127.0,
                    right: f32::from(cmd.right) / 127.0,
                    ..PmInput::default()
                };
                pmove::pmove(&mut self.ps, &input, w, dt);
            }
        }
    }

    /// The wire playerstate. Fields the two modes disagree about are measured
    /// on both sides: the spectator's from the retail capture in
    /// `crates/common/tests/fixtures/net/snapshots.bin`, whose zeros
    /// `parses_captured_snapshot_run` pins, the player's from
    /// `crates/server/tests/fixtures/playerstate/*.txt`, which the
    /// `playerstate_ab` gate diffs against. `commandTime` mirrors the last
    /// processed cmd's server time.
    pub fn to_wire(&self, p: &Protocol, client_num: i32, command_time: i32) -> msg::PlayerState {
        let player = self.pm_type == PmType::Normal;
        let mut w = msg::PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            w.fields[msg::PlayerState::field_index(p, name).unwrap()] = v;
        };
        set("clientNum", client_num);
        set("commandTime", command_time);
        // Mode-dependent.
        set("pm_type", if player { 0 } else { 4 });
        // The 0x8 the spectator carries on top of the player's 0x10 is
        // unaccounted for; both values are the captures', not a mechanism.
        set("eFlags", if player { 16 } else { 24 });
        set(
            "speed",
            if player {
                pmove::SPEED_RUN
            } else {
                pmove::SPEED_SPECTATOR
            } as i32,
        );
        set("gravity", if player { pmove::GRAVITY as i32 } else { 0 });
        if player {
            // Retail leaves all three at zero for a spectator.
            set("viewHeightCurrent", self.ps.view_height().to_bits() as i32);
            set("viewHeightTarget", self.ps.stance.view_height() as i32);
            let ground = match self.ps.on_ground {
                true => ENTITYNUM_WORLD,
                false => ENTITYNUM_NONE,
            };
            set("groundEntityNum", ground as i32);
        }
        // Mode-independent: both captures agree on all of these.
        let (mins, maxs) = (self.ps.mins(), self.ps.maxs());
        for (i, (lo, hi)) in ["mins[0]", "mins[1]", "mins[2]"]
            .iter()
            .zip(["maxs[0]", "maxs[1]", "maxs[2]"])
            .enumerate()
        {
            set(lo, mins[i].to_bits() as i32);
            set(hi, maxs[i].to_bits() as i32);
        }
        set("proneViewHeight", pmove::VIEW_PRONE as i32);
        set("crouchViewHeight", pmove::VIEW_CROUCH as i32);
        set("standViewHeight", pmove::VIEW_STAND as i32);
        // No pmove constant: nothing simulates a dead player's eye yet.
        set("deadViewHeight", 8);
        set("walkSpeedScale", pmove::SCALE_WALK.to_bits() as i32);
        set("runSpeedScale", 1f32.to_bits() as i32);
        set("proneSpeedScale", pmove::SCALE_PRONE.to_bits() as i32);
        set("crouchSpeedScale", pmove::SCALE_CROUCH.to_bits() as i32);
        // The three axis scales the mover does not read; captured values.
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
        // delta_angles stay zero: we never force-turn a client, so there is
        // no teleport correction; the client's cmd angles are already the
        // absolute view `step` simulates from.
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::PROTOCOL_V1;

    fn cmd(forward: i8, pitch_short: i32, yaw_short: i32) -> UserCmd {
        UserCmd {
            forward,
            angles: [pitch_short, yaw_short, 0],
            ..Default::default()
        }
    }

    /// One field of the retail player capture the `playerstate_ab` gate diffs
    /// against. Both fixtures agree on everything this module derives, so one
    /// of them is enough here.
    fn retail_player(field: &str) -> i32 {
        let text = include_str!("../tests/fixtures/playerstate/mp_pavlov-dm.txt");
        let (_, value) = text
            .lines()
            .filter_map(|l| l.split_once(' '))
            .find(|(name, _)| *name == field)
            .unwrap_or_else(|| panic!("no {field} in the capture"));
        value
            .parse()
            .unwrap_or_else(|e| panic!("{field} is {value:?}, not an i32: {e}"))
    }

    /// The player half of `to_wire`, against that capture. The gate cannot
    /// reach this branch until a client can answer the team menu and spawn,
    /// so until then this is what pins the values.
    #[test]
    fn a_standing_player_carries_the_captured_values() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0);
        sim.become_player([0.0, 0.0, 64.0], 0.0);
        // The capture is of a player standing still on the floor.
        sim.ps.on_ground = true;
        let w = sim.to_wire(p, 0, 0);
        for f in [
            "pm_type",
            "eFlags",
            "speed",
            "gravity",
            "viewHeightCurrent",
            "viewHeightTarget",
            "groundEntityNum",
        ] {
            assert_eq!(w.field_i32(p, f), retail_player(f), "{f}");
        }
    }

    #[test]
    fn wire_carries_the_pinned_constants_and_dynamic_fields() {
        let p = &PROTOCOL_V1;
        let sim = ClientSim::spectator([10.0, 20.0, 30.0], 90.0);
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
        let mut sim = ClientSim::spectator([0.0; 3], 0.0);
        sim.ps.yaw = 90f32.to_radians();
        // Sim pitch is camera convention (up positive); the wire is down
        // positive, so a sim looking 45 deg up writes viewangles[0] = -45.
        sim.ps.pitch = 45f32.to_radians();
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.viewangles(p)[1], 90.0);
        assert_eq!(w.viewangles(p)[0], -45.0);
    }

    /// A spectator carries the captured spectator constants; a player does
    /// not. The values `to_wire` used to pin unconditionally are the ones the
    /// retail player capture disagrees with once the client is alive, which is
    /// why they move behind the mode instead of staying literals.
    #[test]
    fn the_wire_constants_follow_the_mode() {
        let p = &PROTOCOL_V1;
        let spec = ClientSim::spectator([0.0, 0.0, 64.0], 0.0);
        let ps = spec.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "pm_type"), 4);

        let mut player = ClientSim::spectator([0.0, 0.0, 64.0], 0.0);
        player.become_player([0.0, 0.0, 64.0], 0.0);
        let ps = player.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "pm_type"), 0);
        assert_ne!(
            ps.field_i32(p, "speed"),
            pmove::SPEED_SPECTATOR as i32,
            "a player still moves at the spectator's speed"
        );
    }

    /// A spectator noclips and a player collides. With no world a player still
    /// simulates rather than panicking, because every unit test here mounts no
    /// map.
    #[test]
    fn a_player_without_a_world_still_steps() {
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0);
        sim.become_player([0.0, 0.0, 64.0], 0.0);
        let cmd = UserCmd {
            forward: 127,
            ..UserCmd::default()
        };
        sim.step(&cmd, 0.05, None);
    }

    #[test]
    fn delta_angles_stay_zero() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0; 3], 0.0);
        sim.ps.yaw = 90f32.to_radians();
        let w = sim.to_wire(p, 0, 0);
        // No force-turn means no correction term; the client's compensation
        // must be identity.
        assert_eq!(w.field_i32(p, "delta_angles[0]"), 0);
        assert_eq!(w.field_i32(p, "delta_angles[1]"), 0);
        assert_eq!(w.viewangles(p)[1], 90.0);
    }

    /// A spectator noclips, so flight needs no collision world and a server
    /// whose map failed to load still flies its spectators.
    #[test]
    fn step_flies_without_a_collision_world() {
        let mut sim = ClientSim::spectator([1.0, 2.0, 3.0], 0.0);
        let c = cmd(127, 0, 0);
        for _ in 0..125 {
            sim.step(&c, 1.0 / 125.0, None);
        }
        assert!(
            sim.ps.origin.x > 1.0,
            "yaw 0 faces +X, origin {:?}",
            sim.ps.origin
        );
        assert!(
            sim.ps.velocity.truncate().length() > 250.0,
            "and accelerates to spectator speed, v {:?}",
            sim.ps.velocity
        );
    }
}
