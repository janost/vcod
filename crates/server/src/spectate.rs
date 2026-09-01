//! Server-side client state: usercmds in, wire playerstate out.

use crate::weapons::PlayerWeapons;
use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg::{self, UserCmd};
use vcod_common::net::protocol::{Protocol, ENTITYNUM_NONE, ENTITYNUM_WORLD};
use vcod_common::net::trajectory;
use vcod_common::pmove::{self, PmInput};

/// `pm_flags`' own-body bit, third of the view-source group: a live client
/// looking out of its own body carries it and neither spectator view does
/// (docs/research/cod11-gsc-object-model.md, section 20).
/// `ET_PLAYER`, what another client sees a player as
/// (`crates/client/src/entities.rs` carries the table).
const ET_PLAYER: i32 = 1;

/// A player entity's `eFlags`, `solid` and `pos.trDuration`, transcribed from
/// the retail two-probe capture rather than derived. `solid` decodes as
/// `(maxs[2] + 32) << 16 | -mins[2] << 8 | half width`, which for 6684943 is
/// a 70-unit standing box one unit deep and 15 wide -- the pmove box, and the
/// same for every player, so a constant is faithful until something makes it
/// vary. INFERRED, from that one value against the known box.
const PLAYER_EFLAGS: i32 = 16;
const PLAYER_SOLID: i32 = 6684943;
const PLAYER_TR_DURATION: i32 = 50;

const PMF_OWN_VIEW: i32 = 0x40000;

/// Stance bits in `eFlags` and `pm_flags`, measured off the retail server
/// under each input (`crates/server/tests/fixtures/playerstate/*-motion.txt`):
/// standing reads `eFlags` 16 / `pm_flags` 0x40000 and crouched 48 / 0x40002.
/// The two `eFlags` bits are exclusive. `pm_flags` 0x1 marks prone and 0x2 is
/// a crouch latch rather than a stance bit: prone entered from a crouch reads
/// 0x40003 and prone entered from standing 0x40001, which is why
/// `PlayerState::ducked` carries it.
const EF_CROUCH: i32 = 0x20;
const EF_PRONE: i32 = 0x40;
const PMF_DUCKED: i32 = 0x2;
const PMF_PRONE: i32 = 0x1;
/// Held jump, retail's 0x8 (set @0x2ec34, cleared @0x34135); the capture reads
/// `pm_flags` 0x40008 on the first airborne frame.
const PMF_JUMP_HELD: i32 = 0x8;

/// `serverCursorHintString`'s no-hint sentinel, which is retail's -1 in an
/// 8-bit netfield. Object model doc, section 20.
const NO_CURSOR_HINT: i32 = 0xff;

/// ANGLE2SHORT units per degree (codextended shared.h).
const ANGLE2SHORT: f32 = 65536.0 / 360.0;

fn short_deg(v: i32) -> f32 {
    let deg = v as f32 / ANGLE2SHORT;
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// A usercmd's input words as pmove's per-frame input. The stance bits are
/// level, and a crouched or prone client holds `up` at -127 for as long as it
/// is down, so only a positive `up` is a jump. `walk_slow` has no wire source:
/// CoD 1 has one move speed and no walk key, and pmove's walk scale is
/// reachable only from the client's own fly mode. Bit table and evidence:
/// docs/protocol-1.1.md, "Usercmd input bits".
fn pm_input(cmd: &UserCmd) -> PmInput {
    PmInput {
        forward: f32::from(cmd.forward) / 127.0,
        right: f32::from(cmd.right) / 127.0,
        jump: cmd.up > 0,
        crouch: cmd.wbuttons & msg::WBUTTON_CROUCH != 0,
        prone: cmd.wbuttons & msg::WBUTTON_PRONE != 0,
        walk_slow: false,
        lean_left: cmd.wbuttons & msg::WBUTTON_LEAN_LEFT != 0,
        lean_right: cmd.wbuttons & msg::WBUTTON_LEAN_RIGHT != 0,
    }
}

/// `ANGLE2SHORT(spawn_angle) - cmd.angles`, RTCW's `SetClientViewAngle`
/// (docs/protocol-1.1.md, "Spectator view angles"). `cmd_angles` is the
/// client's last-known angles at the moment of this spawn. Only a fresh
/// connect's are zero; a client spawned by the script has been sending cmds
/// since it entered the world, and without the subtraction its spawn would
/// force-turn the view to the spawn yaw plus whatever it was already
/// looking at.
fn spawn_delta_angles(yaw_deg: f32, cmd_angles: [i32; 3]) -> [i32; 3] {
    [
        -cmd_angles[0],
        (yaw_deg * ANGLE2SHORT) as i32 - cmd_angles[1],
        -cmd_angles[2],
    ]
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
    /// What the client holds, mirrored from the script host every frame.
    /// `spawn` clears it and `giveWeapon`/`setSpawnWeapon` fill it in.
    pub weapons: PlayerWeapons,
    /// The model configstring index `setViewmodel` left on the client,
    /// mirrored from the script host every frame beside `weapons`.
    pub viewmodel_index: i32,
    /// The client's per-axis view offset, added back onto each cmd's angle
    /// by both `step` and the connected client itself.
    /// docs/protocol-1.1.md, "Spectator view angles".
    delta_angles: [i32; 3],
    /// `serverTime` the running eye-height lerp started at, cleared when it
    /// settles. Retail stamps it and the client runs the lerp from it, so a
    /// zero here makes the client's prediction snap to the target and then be
    /// dragged back once a snapshot. Invisible to a settled capture, which is
    /// why the motion gate cannot pin it (docs/protocol-1.1.md, "The
    /// view-height lerp").
    view_lerp_start: Option<i32>,
}

impl ClientSim {
    /// A client entering the world: Q3 spectator fly
    /// ([`pmove::spectator_move`]), angles straight off the latest cmd.
    /// `cmd_angles` are the angles of the usercmd that brought the client in
    /// -- `SV_ClientEnterWorld`'s `cmds[0]`, zero when there is none -- so
    /// the view it already had survives entry instead of snapping.
    pub fn spectator(origin: [f32; 3], yaw_deg: f32, cmd_angles: [i32; 3]) -> Self {
        ClientSim {
            ps: pmove::PlayerState::spawn(Vec3::from(origin), yaw_deg),
            pm_type: PmType::Spectator,
            weapons: PlayerWeapons::default(),
            viewmodel_index: 0,
            delta_angles: spawn_delta_angles(yaw_deg, cmd_angles),
            view_lerp_start: None,
        }
    }

    /// The mode change `spawnPlayer()` makes: the same sim, restarted at the
    /// spawn point the script chose. `cmd_angles` is the client's last-known
    /// cmd angles going into the spawn -- a spectator can have turned freely
    /// before answering the weapon menu, unlike a fresh connect, so the
    /// caller must supply the real value rather than assume zero.
    pub fn become_player(&mut self, origin: [f32; 3], yaw_deg: f32, cmd_angles: [i32; 3]) {
        self.respawn(PmType::Normal, origin, yaw_deg, cmd_angles);
    }

    /// The other half of the same builtin: `spawnSpectator()` parks a client
    /// at an intermission point through the very same `self spawn(origin,
    /// angles)`, so a spectator moves for exactly the reasons a player does.
    pub fn become_spectator(&mut self, origin: [f32; 3], yaw_deg: f32, cmd_angles: [i32; 3]) {
        self.respawn(PmType::Spectator, origin, yaw_deg, cmd_angles);
    }

    fn respawn(&mut self, mode: PmType, origin: [f32; 3], yaw_deg: f32, cmd_angles: [i32; 3]) {
        self.ps = pmove::PlayerState::spawn(Vec3::from(origin), yaw_deg);
        self.pm_type = mode;
        self.delta_angles = spawn_delta_angles(yaw_deg, cmd_angles);
    }

    /// Advance one frame. The axes arrive quantized to ±127/0 and dt comes off
    /// the cmd clocks.
    pub fn step(&mut self, cmd: &UserCmd, dt: f32, world: Option<&CollisionWorld>) {
        self.ps.yaw = short_deg(cmd.angles[1] + self.delta_angles[1]).to_radians();
        // Wire pitch is positive down; the sim stores the camera's convention.
        self.ps.pitch = -short_deg(cmd.angles[0] + self.delta_angles[0]).to_radians();
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
                pmove::pmove(&mut self.ps, &pm_input(cmd), w, dt);
                // Retail holds a prone view inside the cone around the body by
                // pushing `delta_angles`, so the client's own prediction lands
                // in the same place (docs/research/cod11-mantle.md, "Prone").
                if self.ps.view_yaw_correction != 0.0 {
                    self.delta_angles[1] = (self.delta_angles[1]
                        + (self.ps.view_yaw_correction * ANGLE2SHORT) as i32)
                        & 0xffff;
                }
                // The stamp is the serverTime the lerp began, so it is taken
                // on the frame the eye first trails its target.
                self.view_lerp_start = if self.ps.view_height_settled() {
                    None
                } else {
                    self.view_lerp_start.or(Some(cmd.server_time))
                };
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
    /// Where the player is: the sim owns it, the script mirrors it.
    pub fn origin(&self) -> [f32; 3] {
        self.ps.origin.into()
    }

    /// The point a snapshot is built from: the origin lifted by the current
    /// view height. `SV_BuildClientSnapshot` (0x808f288) adds the playerstate's
    /// view height to `origin[2]` before it looks the leaf up, so a client
    /// standing on a floor is tested from its eyes and not from its feet.
    pub fn eye_origin(&self) -> [f32; 3] {
        let o = self.ps.origin;
        [o[0], o[1], o[2] + self.ps.view_height()]
    }

    /// What other clients are sent about this one. Retail sends a client no
    /// entity for itself -- the playerstate carries it -- so this is only ever
    /// built for somebody else (`docs/protocol-1.1.md`, "Which entities a
    /// client is sent"). The body model is not here either: it rides the
    /// `clientState` roster's `modelindex`, which stage 4 already sends.
    ///
    /// Measured against a retail capture of one probe watching another
    /// (`crates/server/tests/fixtures/entities/mp_carentan-dm-players.txt`).
    /// A moving player travels as a trajectory, not as a point: `pos.trType`
    /// 3 with a delta and a 50 ms duration, so the receiving client carries
    /// the motion between snapshots instead of stepping to each one.
    ///
    /// What that capture carries and this does not is stage 6's: `legsAnim`,
    /// the event ring, and `angles2[1]`.
    pub fn to_entity(&self, p: &Protocol, slot: usize, command_time: i32) -> msg::EntityState {
        let mut e = msg::EntityState::null(p);
        e.number = slot as u32;
        let mut set = |name: &str, v: i32| {
            if let Some(i) = msg::EntityState::field_index(p, name) {
                e.fields[i] = v;
            }
        };
        set("eType", ET_PLAYER);
        set("clientNum", slot as i32);
        set("eFlags", PLAYER_EFLAGS);
        set("solid", PLAYER_SOLID);
        set("weapon", i32::from(self.weapons.current));
        set(
            "groundEntityNum",
            match self.ps.on_ground {
                true => ENTITYNUM_WORLD as i32,
                false => ENTITYNUM_NONE as i32,
            },
        );
        // The lean the other client draws, the same -1..1 the playerstate
        // carries. Without it a leaning player stands straight to everyone
        // else.
        set(
            "leanf",
            (self.ps.lean / vcod_common::pmove::LEAN_MAX).to_bits() as i32,
        );
        set("pos.trType", trajectory::TR_LINEAR_STOP);
        // The time the position was simulated at, not the frame's: retail's
        // capture has trTime 2 to 18 ms behind the snapshot's serverTime,
        // which is the last usercmd's clock. Sending the frame time makes the
        // receiving client extrapolate from a base in its own future, and a
        // player standing still on a slope shivers.
        set("pos.trTime", command_time);
        set("pos.trDuration", PLAYER_TR_DURATION);
        set("apos.trType", trajectory::TR_INTERPOLATE);
        for axis in 0..3 {
            set(
                &format!("pos.trBase[{axis}]"),
                self.ps.origin[axis].to_bits() as i32,
            );
            set(
                &format!("pos.trDelta[{axis}]"),
                self.ps.velocity[axis].to_bits() as i32,
            );
        }
        // The body yaw only: a player entity carries no pitch, which is what
        // the waist and head fields are for.
        set("apos.trBase[1]", self.ps.yaw.to_degrees().to_bits() as i32);
        e
    }

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
        let stance_eflags = match self.ps.stance {
            pmove::Stance::Stand => 0,
            pmove::Stance::Crouch => EF_CROUCH,
            pmove::Stance::Prone => EF_PRONE,
        };
        set("eFlags", if player { 16 | stance_eflags } else { 24 });
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
            // All three come out of one `ClientEndFrame` block a spectator
            // never reaches. Its guards are `sessionstate` playing,
            // `ps.clientNum == self` and, for the hint, `health > 0`;
            // nothing follows another client and nothing dies yet, so
            // `Normal` is the whole of that condition today. The hint is the
            // one that peels off first, at `health` 0.
            let stance_pmflags = if self.ps.ducked { PMF_DUCKED } else { 0 }
                | if self.ps.stance == pmove::Stance::Prone {
                    PMF_PRONE
                } else {
                    0
                };
            let jump_held = if self.ps.jump_latched {
                PMF_JUMP_HELD
            } else {
                0
            };
            set("pm_flags", PMF_OWN_VIEW | stance_pmflags | jump_held);
            // The client predicts its own eye lerp; without these it restarts
            // from our value every snapshot and the view shakes for as long
            // as the lerp lasts.
            set("viewHeightLerpTarget", self.ps.view_lerp_target as i32);
            set("viewHeightLerpTime", self.view_lerp_start.unwrap_or(0));
            // The body's own yaw while prone; the client centres its view cone
            // on it, so a zero here aims the cone at world north.
            set("proneDirection", self.ps.prone_direction.to_bits() as i32);
            set("viewHeightLerpDown", i32::from(self.ps.view_lerp_down));
            // -1..1, left negative, the same convention retail sends.
            set("leanf", (self.ps.lean / pmove::LEAN_MAX).to_bits() as i32);
            set("serverCursorHintString", NO_CURSOR_HINT);
            set("viewmodelIndex", self.viewmodel_index);
        }
        // What the client holds; both layouts are in the object model doc,
        // section 20.
        set("weapons[0]", self.weapons.held as u32 as i32);
        set("weapons[1]", (self.weapons.held >> 32) as u32 as i32);
        let [lo, hi] = self.weapons.slot_words();
        set("weaponslots[0]", lo);
        set("weaponslots[4]", hi);
        set("weapon", i32::from(self.weapons.current));
        // Mode-independent: both captures agree on all of these. The box is
        // the standing one whatever the stance: retail transmits `maxs[2]`
        // 70 while crouched and prone too, and the mover derives its own
        // collision box from the stance rather than from these
        // (`crates/server/tests/playerstate_motion_ab.rs`).
        let (mins, maxs) = (
            self.ps.mins(),
            Vec3::new(
                self.ps.maxs().x,
                self.ps.maxs().y,
                pmove::Stance::Stand.height(),
            ),
        );
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
        // viewangles stays unwritten; the view lives in delta_angles instead
        // and the client rebuilds it. docs/protocol-1.1.md, "Spectator view
        // angles".
        for (i, axis) in ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"]
            .iter()
            .enumerate()
        {
            set(axis, self.delta_angles[i]);
        }
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::msg::NULL_USERCMD;
    use vcod_common::net::protocol::PROTOCOL_V1;

    fn cmd(forward: i8, pitch_short: i32, yaw_short: i32) -> UserCmd {
        UserCmd {
            forward,
            angles: [pitch_short, yaw_short, 0],
            ..Default::default()
        }
    }

    /// A prone view past the 85-degree cone is pushed back by `delta_angles`,
    /// which is how retail enforces it (`PM_UpdateViewAngles` 0x331c8), and
    /// the body's own yaw goes out in `proneDirection`.
    #[test]
    fn a_prone_view_past_the_cap_pushes_delta_angles() {
        let p = &PROTOCOL_V1;
        let w = vcod_common::collision::test_world(&[]);
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let prone = |t: i32, yaw_deg: f32| UserCmd {
            server_time: t,
            wbuttons: msg::WBUTTON_PRONE,
            up: -127,
            angles: [0, (yaw_deg * ANGLE2SHORT) as i32 & 0xffff, 0],
            ..NULL_USERCMD
        };

        let mut t = 1000;
        for _ in 0..20 {
            t += 50;
            sim.step(&prone(t, 0.0), 0.05, Some(&w));
        }
        assert_eq!(sim.ps.stance, pmove::Stance::Prone);
        let before = sim.delta_angles[1];
        assert_eq!(
            sim.to_wire(p, 0, 0).field_i32(p, "proneDirection"),
            sim.ps.prone_direction.to_bits() as i32
        );

        // Well past the cone: the body cannot swing the whole way in one
        // frame, so the rest comes off the view.
        sim.step(&prone(t + 50, 150.0), 0.05, Some(&w));
        assert_ne!(
            sim.delta_angles[1], before,
            "a view past the cap must be pushed back"
        );
    }

    /// The eye-height lerp is stamped with the serverTime it started at and
    /// cleared when it settles, which is retail's own bookkeeping: a settled
    /// capture reads 0 either way, so only a trace through the transition
    /// shows it (docs/protocol-1.1.md, "The view-height lerp").
    #[test]
    fn the_view_height_lerp_carries_the_time_it_started() {
        let p = &PROTOCOL_V1;
        let w = vcod_common::collision::test_world(&[]);
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let lerp_time = |sim: &ClientSim| sim.to_wire(p, 0, 0).field_i32(p, "viewHeightLerpTime");
        let crouch = |t: i32| UserCmd {
            server_time: t,
            wbuttons: msg::WBUTTON_CROUCH,
            up: -127,
            ..NULL_USERCMD
        };

        let mut t = 1000;
        // Settle on the floor first, so the only thing moving is the eye.
        for _ in 0..20 {
            t += 50;
            sim.step(&NULL_USERCMD, 0.05, Some(&w));
        }
        assert_eq!(lerp_time(&sim), 0, "a settled eye carries no stamp");

        t += 50;
        sim.step(&crouch(t), 0.05, Some(&w));
        assert_eq!(lerp_time(&sim), t, "the stamp is the cmd that started it");
        let started = t;
        t += 50;
        sim.step(&crouch(t), 0.05, Some(&w));
        assert_eq!(
            lerp_time(&sim),
            started,
            "and it does not move while lerping"
        );

        for _ in 0..20 {
            t += 50;
            sim.step(&crouch(t), 0.05, Some(&w));
        }
        assert!(sim.ps.view_height_settled());
        assert_eq!(lerp_time(&sim), 0, "the stamp clears when the eye settles");
    }

    /// `leanf` is a fraction of `LEAN_MAX`, left negative, the convention the
    /// retail server sends. The value itself is spawn-dependent (the lean is
    /// clamped against nearby geometry), so `playerstate_motion_ab` cannot
    /// diff it and this pins the mapping instead.
    #[test]
    fn leanf_goes_out_as_a_signed_fraction_of_lean_max() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        let leanf =
            |sim: &ClientSim| f32::from_bits(sim.to_wire(p, 0, 0).field_i32(p, "leanf") as u32);
        assert_eq!(leanf(&sim), 0.0);
        sim.ps.lean = -pmove::LEAN_MAX;
        assert_eq!(leanf(&sim), -1.0, "a full left lean is -1");
        sim.ps.lean = pmove::LEAN_MAX / 2.0;
        assert_eq!(leanf(&sim), 0.5, "a half right lean is +0.5");
    }

    /// The bit table measured off a retail 1.1 client on 2026-09-01, one case
    /// per movement verb. Evidence and the full table:
    /// docs/protocol-1.1.md, "Usercmd input bits".
    #[test]
    fn wire_bits_map_to_movement_verbs() {
        let of = |buttons: u8, wbuttons: u8, up: i8| {
            pm_input(&UserCmd {
                buttons,
                wbuttons,
                up,
                ..Default::default()
            })
        };
        assert!(of(0, 0, 127).jump);
        let crouch = of(0, msg::WBUTTON_CROUCH, -127);
        assert!(crouch.crouch && !crouch.prone);
        let prone = of(0, msg::WBUTTON_PRONE, -127);
        assert!(prone.prone && !prone.crouch);
        assert!(of(0, msg::WBUTTON_LEAN_LEFT, 0).lean_left);
        assert!(of(0, msg::WBUTTON_LEAN_RIGHT, 0).lean_right);
        // A crouched or prone client holds `up` at -127 for as long as it
        // stays down, so only a positive `up` is a jump.
        assert!(!crouch.jump && !prone.jump);
    }

    /// The weapon bits share `buttons` with nothing this module reads, and
    /// CoD 1 has a single move speed with no walk key, so no input reaches
    /// pmove's walk scale.
    #[test]
    fn weapon_bits_and_walk_scale_are_untouched() {
        let all_weapon_bits = pm_input(&UserCmd {
            buttons: 0xff,
            wbuttons: msg::WBUTTON_RELOAD,
            ..Default::default()
        });
        assert_eq!(all_weapon_bits, PmInput::default());
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
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
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
        let sim = ClientSim::spectator([10.0, 20.0, 30.0], 90.0, NULL_USERCMD.angles);
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

    /// `step` reads cmd angles in the wire's positive-down convention and
    /// stores the sim's positive-up one; a spawn yaw of 0 keeps
    /// `delta_angles` at zero so the cmd angle passes straight through.
    #[test]
    fn step_applies_the_camera_pitch_convention() {
        let mut sim = ClientSim::spectator([0.0; 3], 0.0, NULL_USERCMD.angles);
        // 45 deg down on the wire, 90 deg yaw.
        let c = cmd(0, (45.0 * ANGLE2SHORT) as i32, (90.0 * ANGLE2SHORT) as i32);
        sim.step(&c, 0.05, None);
        assert_eq!(sim.ps.yaw.to_degrees(), 90.0);
        assert_eq!(sim.ps.pitch.to_degrees(), -45.0);
    }

    /// A spectator carries the captured spectator constants; a player does
    /// not. The values `to_wire` used to pin unconditionally are the ones the
    /// retail player capture disagrees with once the client is alive, which is
    /// why they move behind the mode instead of staying literals.
    #[test]
    fn the_wire_constants_follow_the_mode() {
        let p = &PROTOCOL_V1;
        let spec = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        let ps = spec.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "pm_type"), 4);

        let mut player = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        player.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
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
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 64.0], 0.0, NULL_USERCMD.angles);
        let cmd = UserCmd {
            forward: 127,
            ..UserCmd::default()
        };
        sim.step(&cmd, 0.05, None);
    }

    /// The three-part edit, pinned as one behaviour: a sim spawned facing 90
    /// degrees reports that yaw to a client whose cmd angles are zero,
    /// because the offset lives in `delta_angles` and the client adds it
    /// back; `step` must add it back the same way to keep simulating at the
    /// spawn yaw once a real cmd arrives. A partial edit fails this: dropping
    /// the delta write reports 0, keeping the `viewangles` write reports the
    /// spawn angle there instead of leaving it unwritten, and reverting
    /// `step` alone snaps the simulated yaw back to the cmd's raw 0 rather
    /// than 90. `16_384` is `ANGLE2SHORT(90)`, the same value the committed
    /// capture fixture carries (`crates/common/src/net/msg.rs:1826`). Pitch
    /// (`[0]`) is asserted on both fields too, a strict superset of the
    /// deleted `delta_angles_stay_zero`: only the yaw is spawn-dependent, so
    /// a `spawn_delta_angles` that put the yaw in the wrong slot, or a
    /// `viewangles[0]` write left behind, must fail here as well.
    #[test]
    fn delta_angles_carry_the_spawn_yaw_and_viewangles_stay_unwritten() {
        let p = &PROTOCOL_V1;
        let mut sim = ClientSim::spectator([0.0, 0.0, 64.0], 90.0, NULL_USERCMD.angles);
        let ps = sim.to_wire(p, 0, 0);
        assert_eq!(ps.field_i32(p, "delta_angles[0]"), 0);
        assert_eq!(ps.field_i32(p, "delta_angles[1]"), 16_384);
        assert_eq!(ps.field_i32(p, "viewangles[0]"), 0);
        assert_eq!(ps.field_i32(p, "viewangles[1]"), 0);

        // The probe that took the capture sends cmd.angles = [0,0,0] and
        // never moves its view; step must add delta_angles back or the sim
        // faces 0 instead of the spawn's 90.
        sim.step(&cmd(0, 0, 0), 0.05, None);
        assert_eq!(sim.ps.yaw.to_degrees(), 90.0);
    }

    /// A spectator noclips, so flight needs no collision world and a server
    /// whose map failed to load still flies its spectators.
    #[test]
    fn step_flies_without_a_collision_world() {
        let mut sim = ClientSim::spectator([1.0, 2.0, 3.0], 0.0, NULL_USERCMD.angles);
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
