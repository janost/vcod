//! Server-side client state: usercmds in, wire playerstate out.

use crate::game::host::SimOp;
use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg::{self, UserCmd};
use vcod_common::net::protocol::{Protocol, ENTITYNUM_NONE, ENTITYNUM_WORLD};
use vcod_common::net::trajectory;
use vcod_common::pmove::{self, PmEvent, PmInput};
use vcod_common::weapon::WeaponDef;

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
/// The per-life toggle. VERIFIED: retail alternates `eFlags` 16 and 24
/// across a player's lives (`mp_carentan-dm-hit-target.txt` reads 115 samples
/// at 16 against 101 at 24). INFERRED: a client breaks interpolation on the
/// changed word, so without it a respawn smears from the corpse to the spawn.
/// The same bit `bodies::EFLAGS_ANIM_TOGGLE` inverts per body-queue push, for
/// the same reason: a changed `eFlags` is what makes a client stop carrying
/// the previous occupant of that entity number forward.
const EF_TELEPORT_BIT: i32 = 0x8;
const PMF_DUCKED: i32 = 0x2;
const PMF_PRONE: i32 = 0x1;
/// Held jump, retail's 0x8 (set @0x2ec34, cleared @0x34135); the capture reads
/// `pm_flags` 0x40008 on the first airborne frame.
const PMF_JUMP_HELD: i32 = 0x8;
/// Backpedalling, retail's 0x40. The anim selection reads this bit rather than
/// the usercmd, and both captures carry it at `run_back` and nowhere else.
const PMF_BACKWARDS_RUN: i32 = 0x40;

/// The dead `pm_type`, read off both retail deaths (combat doc, section 8).
pub const PM_DEAD: i32 = 6;
/// `EV_PAIN` and `EV_DEATH` (`docs/research/cod11-events-and-fx.md`).
const EV_PAIN: i32 = 187;
const EV_DEATH: i32 = 189;
/// One `EV_PAIN` per 700 ms (combat doc, section 6, step 9).
const PAIN_DEBOUNCE_MS: i32 = 700;
/// `finishPlayerDamage`'s knockback (combat doc, 4.5): the stance scales,
/// the cap, and `g_knockback / 250` at the cvar's stock 1000.
const KNOCKBACK_STAND: f32 = 0.3;
const KNOCKBACK_DUCKED: f32 = 0.15;
const KNOCKBACK_PRONE: f32 = 0.02;
const KNOCKBACK_MAX: i32 = 60;
const KNOCKBACK_UNITS: f32 = 1000.0 / 250.0;

/// `finishPlayerDamage`'s per-frame accumulator (combat doc, 4.5): what the
/// frame's hits added up to and where the last one came from, which
/// `P_DamageFeedback` turns into the four wire fields at end-frame.
#[derive(Clone, Copy, Default)]
struct DamageAccum {
    taken: i32,
    /// The normalised damage direction, or `None` for a hit that carried
    /// no direction, which the feedback marks with 255/255.
    from: Option<Vec3>,
}

/// `ps.damageEvent`, `damageCount`, `damageYaw`, `damagePitch`. They keep
/// their last values between hits: the client detects a hit by
/// `damageEvent` changing (combat doc, section 6).
#[derive(Clone, Copy, Default)]
struct DamageFeedback {
    event: i32,
    count: i32,
    yaw: i32,
    pitch: i32,
}

/// `serverCursorHintString`'s no-hint sentinel, which is retail's -1 in an
/// 8-bit netfield. Object model doc, section 20.
const NO_CURSOR_HINT: i32 = 0xff;

/// `stats[3]`'s "no teammate": retail's -1 in six raw bits, so the wire
/// cannot tell it from client 63 (docs/protocol-1.1.md, "Block 1").
const NO_TEAMMATE: i32 = 63;

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
        attack: cmd.buttons & msg::BUTTON_ATTACK != 0,
        reload: cmd.wbuttons & msg::WBUTTON_RELOAD != 0,
        ads: cmd.buttons & msg::BUTTON_ADS != 0,
        use_button: cmd.buttons & msg::BUTTON_USE != 0,
        weapon: cmd.weapon,
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
    /// `ps.eventSequence`, the count of events ever raised. Eight bits on the
    /// wire; kept wide here and masked at the wire.
    pub event_sequence: i32,
    /// `ps.events` and `ps.eventParms`, the four-slot ring the counter
    /// indexes. Cleared at a respawn along with the counter: retail's own
    /// respawn frame reads an empty ring at sequence 0
    /// (`docs/research/cod11-combat.md` 9.2).
    pub events: [i32; 4],
    pub event_parms: [i32; 4],
    /// The model configstring index `setViewmodel` left on the client,
    /// mirrored from the script host every frame the way the weapons are.
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
    /// The animation channels, driven by `update_anims` after each frame's
    /// moves and read straight into the wire playerstate.
    anim: vcod_common::animscript::AnimState,
    /// Whether the sim was off the ground last frame, so `update_anims` can
    /// raise `jump` and `land` on the edges rather than every frame.
    was_airborne: bool,
    /// The animscript's `strafing` condition, which is state rather than a
    /// reading of this frame's cmd: retail updates it only when the cmd asks
    /// for movement and leaves it alone when both axes are zero
    /// (`game.mp.i386.so` 0x32504).
    strafing: Option<vcod_common::animscript::Side>,
    /// A jump impulse taken since the last `update_anims`, so a tick that ran
    /// several moves still raises the event. Leaving the ground is not enough:
    /// a ledge and a ladder do that without a jump.
    jumped: bool,
    /// `ps.stats[0]` and `stats[2]`, mirrored from the host's vitals every
    /// frame; the host is where the script's `self.health` lands.
    pub health: i32,
    pub max_health: i32,
    /// Killed and not yet respawned: `PM_DEAD` on the wire, the dead move,
    /// no weapon step, no feedback.
    pub dead: bool,
    /// `ps.aimSpreadScale`, 0..255: what a shot and a hit add and the
    /// weapon's decay rate takes away (combat doc, 2.1 and 6.5). A netfield
    /// (`fields_v1.rs`), written as a float in `to_wire`; the spread reads
    /// it too.
    pub aim_spread_scale: f32,
    damage: DamageAccum,
    feedback: DamageFeedback,
    /// `ps.stats[1]`, the yaw toward the killer (combat doc, 5.1, item 11).
    dead_yaw: i32,
    /// When the next `EV_PAIN` may fire.
    pain_after_ms: i32,
    /// `EF_TELEPORT_BIT`'s current state, flipped by every player spawn.
    teleport_bit: bool,
    /// `ps.stats[5]`, the spawn counter retail's `ClientSpawn` carries across
    /// its own memset (docs/protocol-1.1.md, "Block 1"). A byte on the wire,
    /// so it wraps; every spawn of either mode bumps it, which puts the lone
    /// spectator's capture at 1.
    spawn_count: u8,
}

/// Everything the animscript needs that the sim does not own: the script
/// itself, the name-to-index lookup, and the weapon the client holds.
pub struct AnimInputs<'a> {
    pub anims: &'a vcod_common::animtree::PlayerAnims,
    pub weapon: &'a str,
    pub weapon_class: &'a str,
}

/// The `EVENTS` block a weapon event raises, or `None` for one the script has
/// nothing for. Retail's `PM_Weapon` raises `BG_AnimScriptEvent` where it
/// enters the state (docs/research/cod11-combat.md, sections 1.5, 1.7 and
/// 1.8); the block's clauses are what pick the anim.
fn weapon_anim_event(event: i32) -> Option<&'static str> {
    use vcod_common::pmove::weapon::{
        EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT, EV_PUTAWAY_WEAPON, EV_RAISE_WEAPON, EV_RELOAD,
        EV_RELOAD_FROM_EMPTY, EV_RELOAD_START,
    };
    Some(match event {
        EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT => "fireweapon",
        EV_RELOAD | EV_RELOAD_FROM_EMPTY | EV_RELOAD_START => "reload",
        EV_PUTAWAY_WEAPON => "dropweapon",
        EV_RAISE_WEAPON => "raiseweapon",
        _ => return None,
    })
}

/// Horizontal speed below which the animscript sees an idle player, whatever
/// the input. A player pressed into geometry keeps a few units a second and
/// retail's capture animates it standing; the value sits between that
/// capture's 8 u/s blocked run and its 13 u/s prone crawl
/// (`crates/server/tests/playerstate_motion_ab.rs`).
pub const ANIM_IDLE_SPEED: f32 = 10.0;

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
            event_sequence: 0,
            events: [0; 4],
            event_parms: [0; 4],
            viewmodel_index: 0,
            delta_angles: spawn_delta_angles(yaw_deg, cmd_angles),
            view_lerp_start: None,
            anim: Default::default(),
            was_airborne: false,
            strafing: None,
            jumped: false,
            health: 0,
            max_health: 0,
            dead: false,
            aim_spread_scale: 0.0,
            damage: DamageAccum::default(),
            feedback: DamageFeedback::default(),
            dead_yaw: 0,
            pain_after_ms: 0,
            // Set, so the first player spawn's flip clears it: retail's first
            // life reads `eFlags` 16 and each later one alternates.
            teleport_bit: true,
            // The constructor is the connect, before any spawn: the script's
            // own `spawnSpectator` is what takes it to the capture's 1.
            spawn_count: 0,
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
        // A respawned player does not resume the anim it died in.
        self.anim = Default::default();
        self.was_airborne = false;
        self.strafing = None;
        self.jumped = false;
        // `ClientSpawn`'s memset: the damage fields read 0 again after a
        // respawn (combat doc, 8.4), and so does the dead yaw.
        self.dead = false;
        self.aim_spread_scale = 0.0;
        self.damage = DamageAccum::default();
        self.feedback = DamageFeedback::default();
        self.dead_yaw = 0;
        self.pain_after_ms = 0;
        self.spawn_count = self.spawn_count.wrapping_add(1);
        // Retail's respawn frame reads an empty ring at sequence 0
        // (combat doc, 9.2).
        self.event_sequence = 0;
        self.events = [0; 4];
        self.event_parms = [0; 4];
        // A spectator's `eFlags` is a constant on the wire, so only a player
        // spawn consumes a flip.
        if mode == PmType::Normal {
            self.teleport_bit = !self.teleport_bit;
        }
    }

    /// A live player's `eFlags`: the base word, the per-life teleport bit and
    /// nothing else. The stance bits ride on the playerstate copy only, which
    /// is where the motion capture measured them.
    fn eflags(&self) -> i32 {
        PLAYER_EFLAGS
            | if self.teleport_bit {
                EF_TELEPORT_BIT
            } else {
                0
            }
    }

    /// Writes `events[seq & 3]` and then bumps the counter, the order both
    /// retail captures measured (`docs/research/cod11-combat.md` section 7).
    pub fn add_event(&mut self, event: i32, parm: i32) {
        let slot = (self.event_sequence & 3) as usize;
        self.events[slot] = event;
        self.event_parms[slot] = parm;
        self.event_sequence = self.event_sequence.wrapping_add(1);
    }

    /// Advance one frame, returning the events the move raised, already in the
    /// ring. The axes arrive quantized to ±127/0 and dt comes off the cmd
    /// clocks. `weapons` is the map's weapon table, which only a player reads.
    pub fn step(
        &mut self,
        cmd: &UserCmd,
        dt: f32,
        world: Option<&CollisionWorld>,
        weapons: &[Option<WeaponDef>],
    ) -> Vec<PmEvent> {
        // A dead player's view is frozen and its body falls and slides;
        // nothing it presses reaches the mover or the weapon (combat doc,
        // 1.12 and 6, the `pm_type > 5` returns).
        if self.dead {
            if let Some(w) = world {
                pmove::dead_move(&mut self.ps, w, dt);
            }
            return Vec::new();
        }
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
                let events = pmove::pmove(&mut self.ps, &pm_input(cmd), w, dt, weapons);
                self.jumped |= self.ps.jumped;
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
                // `PM_AdjustAimSpreadScale` (combat doc, 2.1), reduced to
                // the fire add and the decay: the turn and move terms and
                // the stance decay multipliers are not carried.
                if let Some(def) = weapons
                    .get(self.ps.weapon as usize)
                    .and_then(|d| d.as_ref())
                {
                    use vcod_common::pmove::weapon::{EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT};
                    let shots = events
                        .iter()
                        .filter(|e| e.event == EV_FIRE_WEAPON || e.event == EV_FIRE_WEAPON_LASTSHOT)
                        .count();
                    self.aim_spread_scale = (self.aim_spread_scale
                        - def.hip_spread_decay_rate * dt
                        + def.hip_spread_fire_add * shots as f32)
                        .clamp(0.0, 255.0);
                }
                for e in &events {
                    self.add_event(e.event, e.parm);
                }
                return events;
            }
        }
        // A spectator raises none: it has no weapon and no footsteps.
        Vec::new()
    }

    /// Picks this frame's animation from the state the moves just produced.
    /// Called once per frame after the moves.
    ///
    /// Retail's shape, read out of the selection function at `game.mp.i386.so`
    /// 0x322c8: below 10 units/s of horizontal speed the player is idle
    /// whatever it asked for, above it the movetype is the stance crossed with
    /// the backpedal latch, and off the ground nothing is selected at all. The
    /// usercmd reaches the selection only through two latches, `pm_flags` 0x40
    /// in pmove and the `strafing` condition here; the selection itself never
    /// reads it (docs/research/player-model-anim-system.md, "How retail picks
    /// the movetype").
    ///
    /// A spectator animates nothing; it is sent to nobody and its own
    /// playerstate carries zeros in the capture.
    pub fn update_anims(
        &mut self,
        inputs: &AnimInputs,
        cmd: &UserCmd,
        now_ms: i32,
        events: &[PmEvent],
    ) {
        use vcod_common::animscript::{Conditions, Movetype, Side};
        // A dead body keeps the death anim `take_damage` chose: retail's
        // selection returns on `pm_type > 5`, and only the corpse clone reads
        // the channels after that.
        if self.pm_type != PmType::Normal || self.dead {
            return;
        }
        // Retail's condition 8 (@0x32504): any forward component clears the
        // strafe, diagonals included; a cmd that is sideways only sets the
        // side; a cmd asking for neither leaves the condition as it was.
        if cmd.forward != 0 {
            self.strafing = None;
        } else if cmd.right != 0 {
            self.strafing = Some(if cmd.right < 0 {
                Side::Left
            } else {
                Side::Right
            });
        }
        // Retail's compare is `xyspeed < 10.0`, so the constant itself is
        // moving.
        let moving = self.ps.velocity.truncate().length() >= ANIM_IDLE_SPEED;
        let back = self.ps.backwards_run;
        // The stance crossed with the backpedal latch, every frame. CoD 1 has
        // no walk key, so retail's walk bit is never set and the `walk*`
        // blocks are unreachable; the prone arm never reads that bit at all
        // (@0x326f1), which is why prone moves through `walkprone`.
        let movetype = match (self.ps.stance, moving, back) {
            // A climber is off the ground and still selects: retail's ladder
            // flag bypasses the airborne early-out (@0x323af).
            _ if self.ps.on_ladder && self.ps.velocity.z >= 0.0 => Movetype::ClimbUp,
            _ if self.ps.on_ladder => Movetype::ClimbDown,
            (pmove::Stance::Prone, false, _) => Movetype::IdleProne,
            (pmove::Stance::Prone, true, true) => Movetype::WalkProneBk,
            (pmove::Stance::Prone, true, false) => Movetype::WalkProne,
            (pmove::Stance::Crouch, false, _) => Movetype::IdleCr,
            (pmove::Stance::Crouch, true, true) => Movetype::RunCrBk,
            (pmove::Stance::Crouch, true, false) => Movetype::RunCr,
            (pmove::Stance::Stand, false, _) => Movetype::Idle,
            (pmove::Stance::Stand, true, true) => Movetype::RunBk,
            (pmove::Stance::Stand, true, false) => Movetype::Run,
        };
        let conditions = Conditions {
            movetype,
            weapon: inputs.weapon.to_ascii_lowercase(),
            weapon_class: inputs.weapon_class.to_ascii_lowercase(),
            ads: pm_input(cmd).ads,
            strafing: self.strafing,
            mounted: None,
            firing: self.ps.weaponstate == vcod_common::pmove::weapon::WEAPON_FIRING,
        };
        let resolve = |name: &str| inputs.anims.wire_of(name);
        let length = |name: &str| inputs.anims.length_ms(name);
        // The two ground edges, before the continuous state: an event anim
        // holds the channel, so raising it first is what keeps the restart
        // toggle flipping once per landing rather than twice.
        //
        // The takeoff is raised by the jump impulse and not by becoming
        // airborne: retail's own mp_pavlov capture backs off a ledge at
        // `run_back` and reads the run loop while airborne, so a fall and a
        // mounted ladder animate whatever they were doing.
        let jumped = std::mem::take(&mut self.jumped);
        let script = &inputs.anims.script;
        match (self.ps.on_ground, self.was_airborne) {
            (true, true) => {
                let sel = script.select_event("land", &conditions);
                self.anim.event(&sel, now_ms, resolve, length);
            }
            (false, false) if jumped && !self.ps.on_ladder => {
                let event = if back { "jumpbk" } else { "jump" };
                let sel = script.select_event(event, &conditions);
                self.anim.event(&sel, now_ms, resolve, length);
            }
            _ => {}
        }
        // The weapon channel: retail's `PM_Weapon` raises a script event on
        // entering a state, and the `EVENTS` block's own clauses pick the
        // torso index by stance, weapon and class
        // (docs/research/player-model-anim-system.md, "The weapon channel").
        for e in events {
            let Some(name) = weapon_anim_event(e.event) else {
                continue;
            };
            let sel = script.select_event(name, &conditions);
            self.anim.event(&sel, now_ms, resolve, length);
        }
        // Nothing is selected while off the ground -- retail returns before
        // the selection unless the ladder flag is set (@0x323a2), which is
        // what gives a climber its `climbup`/`climbdown` -- so a jump owns
        // the legs until the landing.
        if self.ps.on_ground || self.ps.on_ladder {
            let mut sel = script.select("combat", &conditions);
            // Retail leaves `torsoAnim` 0 in every settled pose of both
            // captures, although the clauses reached here are `both`. That 0
            // is a write: it is what a weapon event's anim gives the channel
            // back to when it runs out.
            sel.torso = None;
            self.anim.set(&sel, now_ms, resolve);
        }
        // Outside the airborne early-out: a shot fired in the air would
        // otherwise hold its torso until the landing.
        self.anim.clear_torso(now_ms);
        self.was_airborne = !self.ps.on_ground;
    }

    /// The anim conditions of the moment, for an event raised outside the
    /// frame's own selection: the stance, and the weapon the inputs name.
    fn event_conditions(&self, inputs: &AnimInputs) -> vcod_common::animscript::Conditions {
        use vcod_common::animscript::{Conditions, Movetype};
        let moving = self.ps.velocity.truncate().length() >= ANIM_IDLE_SPEED;
        let movetype = match (self.ps.stance, moving, self.ps.backwards_run) {
            (pmove::Stance::Prone, _, _) => Movetype::IdleProne,
            (pmove::Stance::Crouch, false, _) => Movetype::IdleCr,
            (pmove::Stance::Crouch, true, _) => Movetype::RunCr,
            (pmove::Stance::Stand, false, _) => Movetype::Idle,
            (pmove::Stance::Stand, true, true) => Movetype::RunBk,
            (pmove::Stance::Stand, true, false) => Movetype::Run,
        };
        Conditions {
            movetype,
            weapon: inputs.weapon.to_ascii_lowercase(),
            weapon_class: inputs.weapon_class.to_ascii_lowercase(),
            ads: false,
            strafing: self.strafing,
            mounted: None,
            firing: false,
        }
    }

    /// The sim's half of `finishPlayerDamage` (combat doc, 4.5): the
    /// knockback, the frame's damage accumulated for `end_frame`, and on a
    /// killing hit what `player_die` does to the playerstate (5.1):
    /// `EV_DEATH` with parm 0, the dead yaw into `stats[1]`, the death anim.
    /// The health itself is the host's and arrives through the mirror.
    /// `rng` is the server's own draw state: the two anims here are the ones
    /// the script lists several of, and retail picks among them at random.
    pub fn take_damage(
        &mut self,
        op: &SimOp,
        anims: Option<&AnimInputs>,
        rng: &mut u64,
        now_ms: i32,
    ) {
        let SimOp::Damaged {
            damage,
            dir,
            knockback,
            attacker_origin,
            fatal,
            ..
        } = *op;
        let dir = Vec3::from(dir);
        let has_dir = dir.length_squared() > 0.0;
        // Read before the knockback, which is this frame's impulse and not
        // movement: retail's conditions are the ones the last pmove left
        // (`BG_UpdateConditionValue`), so a standing player shot off his feet
        // still dies a standing death.
        let conditions = anims.map(|inputs| self.event_conditions(inputs));
        if knockback {
            let scale = if self.ps.stance == pmove::Stance::Prone {
                KNOCKBACK_PRONE
            } else if self.ps.ducked {
                KNOCKBACK_DUCKED
            } else {
                KNOCKBACK_STAND
            };
            let kb = ((damage as f32 * scale) as i32).min(KNOCKBACK_MAX);
            if kb > 0 && has_dir {
                self.ps.velocity += dir * (kb as f32 * KNOCKBACK_UNITS);
            }
        }
        self.damage.taken += damage;
        self.damage.from = has_dir.then_some(dir);
        if !fatal {
            // The stock script's `pain` clauses are all `both`; the retail
            // hit frame keeps `torsoAnim` 0 through one, so only the legs
            // take it (combat doc, 8.4).
            if let (Some(inputs), Some(c)) = (anims, &conditions) {
                let mut sel = inputs.anims.script.select_event_random("pain", c, rng);
                sel.torso = None;
                let resolve = |name: &str| inputs.anims.wire_of(name);
                let length = |name: &str| inputs.anims.length_ms(name);
                self.anim.event(&sel, now_ms, resolve, length);
            }
            return;
        }
        self.dead = true;
        self.add_event(EV_DEATH, 0);
        // `vectoyaw(attacker->origin - self->origin)` truncated, the body's
        // own yaw when there is no attacker (5.1, item 11).
        self.dead_yaw = match attacker_origin {
            Some(a) => vec_to_yaw(Vec3::from(a) - self.ps.origin) as i32,
            None => self.ps.yaw.to_degrees() as i32,
        };
        // The death anim on the legs, and the torso restarted on 0: both
        // retail deaths read `torsoAnim` 512 beside the death `legsAnim`
        // (combat doc, 8.1 and 8.4). Nothing selects for this sim again --
        // `update_anims` returns on a dead one -- so the drawn index is what
        // the corpse clone carries.
        if let (Some(inputs), Some(c)) = (anims, &conditions) {
            let mut sel = inputs.anims.script.select_event_random("death", c, rng);
            sel.torso = None;
            let resolve = |name: &str| inputs.anims.wire_of(name);
            let length = |name: &str| inputs.anims.length_ms(name);
            self.anim.event(&sel, now_ms, resolve, length);
        }
        self.anim.restart_torso_empty();
    }

    /// `P_DamageFeedback` (combat doc, section 6), once per frame after the
    /// ops: the frame's damage becomes `damageCount`, its direction the two
    /// angle bytes, `damageEvent` counts up, `EV_PAIN` carries the health
    /// left. A dead player's feedback never runs, so the killing hit leaves
    /// all four fields as the last surviving hit left them (8.4).
    pub fn end_frame(&mut self, now_ms: i32) {
        if self.dead || self.damage.taken <= 0 || self.max_health <= 0 {
            return;
        }
        let count = (self.damage.taken * 100 / self.max_health).min(127);
        self.aim_spread_scale = (self.aim_spread_scale + count as f32).min(255.0);
        match self.damage.from {
            None => {
                self.feedback.yaw = 255;
                self.feedback.pitch = 255;
            }
            Some(d) => {
                let (pitch, yaw) = vec_to_angles(d);
                self.feedback.pitch = (pitch / 360.0 * 256.0) as i32;
                self.feedback.yaw = (yaw / 360.0 * 256.0) as i32;
            }
        }
        if now_ms.wrapping_sub(self.pain_after_ms) > 0 {
            let percent = (self.health as f32 * 100.0 / self.max_health as f32) as i32;
            self.add_event(EV_PAIN, percent.clamp(0, 100));
            self.pain_after_ms = now_ms.wrapping_add(PAIN_DEBOUNCE_MS);
        }
        self.feedback.event = self.feedback.event.wrapping_add(1);
        self.feedback.count = count;
        self.damage.taken = 0;
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
    /// The event ring rides here as well as on the playerstate: it is how
    /// another client hears this one's shots and footsteps.
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
        set("eFlags", self.eflags());
        set("solid", PLAYER_SOLID);
        set("legsAnim", self.anim.legs());
        // The torso does travel: the shoot, reload and putaway poses are the
        // weapon's, and the next task is what gives it a value.
        set("torsoAnim", self.anim.torso());
        set("weapon", i32::from(self.ps.weapon));
        set("eventSequence", self.event_sequence & 0xff);
        for (i, (ev, parm)) in self.events.iter().zip(&self.event_parms).enumerate() {
            set(&format!("events[{i}]"), *ev);
            set(&format!("eventParms[{i}]"), *parm);
        }
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
        // The legs' heading off the view, retail's `ps.movementDir` verbatim
        // (`BG_PlayerStateToEntityStateExtrapolate` @0x2d06d). Without it a
        // strafing player runs sideways with its legs pointing forward.
        set("angles2[1]", (self.ps.movement_dir as f32).to_bits() as i32);
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
        set(
            "pm_type",
            match (player, self.dead) {
                (true, true) => PM_DEAD,
                (true, false) => 0,
                (false, _) => 4,
            },
        );
        // The 0x8 a spectator carries is the same bit `EF_TELEPORT_BIT` names;
        // a spectator's value is the capture's constant, not a mechanism.
        let stance_eflags = match self.ps.stance {
            pmove::Stance::Stand => 0,
            pmove::Stance::Crouch => EF_CROUCH,
            pmove::Stance::Prone => EF_PRONE,
        };
        set(
            "eFlags",
            if player {
                self.eflags() | stance_eflags
            } else {
                24
            },
        );
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
            // `PM_CheckDuck` (cgame 0x30009de0) targets `deadViewHeight` for
            // `pm_type >= 6`; the client re-derives it, so this only keeps the
            // wire honest.
            let target = if self.dead {
                pmove::VIEW_DEAD
            } else {
                self.ps.stance.view_height()
            };
            set("viewHeightTarget", target as i32);
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
            let backwards = if self.ps.backwards_run {
                PMF_BACKWARDS_RUN
            } else {
                0
            };
            set(
                "pm_flags",
                PMF_OWN_VIEW | stance_pmflags | jump_held | backwards,
            );
            // The client predicts its own eye lerp; without these it restarts
            // from our value every snapshot and the view shakes for as long
            // as the lerp lasts.
            set("viewHeightLerpTarget", self.ps.view_lerp_target as i32);
            // The stance lerp's stamp; the dead eye's drop is not one.
            set(
                "viewHeightLerpTime",
                if self.dead {
                    0
                } else {
                    self.view_lerp_start.unwrap_or(0)
                },
            );
            // The four feedback fields hold their last values (section 6).
            set("damageEvent", self.feedback.event & 0xff);
            set("damageCount", self.feedback.count);
            set("damageYaw", self.feedback.yaw & 0xff);
            set("damagePitch", self.feedback.pitch & 0xff);
            // The body's own yaw while prone; the client centres its view cone
            // on it, so a zero here aims the cone at world north.
            set("proneDirection", self.ps.prone_direction.to_bits() as i32);
            // 8 bits on the wire, so a leftward angle travels as its
            // unsigned byte; `angles2[1]` on the entity carries the signed
            // value as a float.
            set("movementDir", self.ps.movement_dir & 0xff);
            set("viewHeightLerpDown", i32::from(self.ps.view_lerp_down));
            // -1..1, left negative, the same convention retail sends.
            set("leanf", (self.ps.lean / pmove::LEAN_MAX).to_bits() as i32);
            set("serverCursorHintString", NO_CURSOR_HINT);
            set("viewmodelIndex", self.viewmodel_index);
            set("legsAnim", self.anim.legs());
            set("torsoAnim", self.anim.torso());
            // Both are netfields the client predicts from, so a constant
            // here fights its own prediction once a snapshot: the ADS lerp
            // restarts and the spread reads a stale scale. The fraction is
            // INFERRED (combat doc, 9.4); the scale is the sim's own.
            set("fWeaponPosFrac", self.ps.weapon_pos_frac.to_bits() as i32);
            set("aimSpreadScale", self.aim_spread_scale.to_bits() as i32);
        }
        // What the client holds; both layouts are in the object model doc,
        // section 20. The sim owns all of it: the script host's copy is
        // mirrored into `ps` every frame, and the weapon machine writes
        // `ps.weapon` itself through a switch.
        set("weapons[0]", self.ps.weapons_held as u32 as i32);
        set("weapons[1]", (self.ps.weapons_held >> 32) as u32 as i32);
        let [lo, hi] = pmove::weapon::slot_words(&self.ps);
        set("weaponslots[0]", lo);
        set("weaponslots[4]", hi);
        set("weapon", i32::from(self.ps.weapon));
        // The weapon machine's own state (combat doc, section 1), and the
        // event ring both this and the entity carry.
        set("weaponstate", i32::from(self.ps.weaponstate));
        set("weapAnim", self.ps.weap_anim);
        set("weaponTime", self.ps.weapon_time_ms);
        set("weaponDelay", self.ps.weapon_delay_ms);
        set("weaponrechamber[0]", self.ps.weapon_rechamber as u32 as i32);
        set(
            "weaponrechamber[1]",
            (self.ps.weapon_rechamber >> 32) as u32 as i32,
        );
        set("eventSequence", self.event_sequence & 0xff);
        for (i, (ev, parm)) in self.events.iter().zip(&self.event_parms).enumerate() {
            set(&format!("events[{i}]"), *ev);
            set(&format!("eventParms[{i}]"), *parm);
        }
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
        set("deadViewHeight", pmove::VIEW_DEAD as i32);
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
        // The two ammo arrays are not netfields: they travel in the
        // playerstate's array blocks (docs/protocol-1.1.md, "How `ammo[]` and
        // `ammoclip[]` are indexed"), so they are written last, once the
        // field setter's borrow is over.
        for (i, (ammo, clip)) in self.ps.ammo.iter().zip(&self.ps.ammoclip).enumerate() {
            w.set_ammo(i, *ammo);
            w.set_clip(i, *clip);
        }
        // `stats[0]`, `stats[2]` and the dead yaw in `stats[1]`
        // (docs/protocol-1.1.md, "Block 1").
        w.set_health(self.health);
        w.set_max_health(self.max_health);
        w.arrays.stats[1] = self.dead_yaw;
        // `stats[3]` is six raw bits, so "nobody" travels as 63; the compass
        // has no teammate to name until `TeamplayInfoMessage` exists.
        w.arrays.stats[3] = NO_TEAMMATE;
        w.arrays.stats[5] = i32::from(self.spawn_count);
        w
    }
}

/// Q3's `vectoyaw` in degrees, 0..360, and 0 for a vector with no
/// horizontal part.
fn vec_to_yaw(v: Vec3) -> f32 {
    if v.x == 0.0 && v.y == 0.0 {
        return 0.0;
    }
    let yaw = v.y.atan2(v.x).to_degrees();
    if yaw < 0.0 {
        yaw + 360.0
    } else {
        yaw
    }
}

/// `vectoangles` as `(pitch, yaw)`, both 0..360, the reading the gsc
/// `vectorToAngles` builtin is measured to (`crate::game::builtins::math`):
/// a slightly downward direction is a pitch just under 360.
fn vec_to_angles(v: Vec3) -> (f32, f32) {
    let yaw = vec_to_yaw(v);
    let mut pitch = if v.x == 0.0 && v.y == 0.0 {
        if v.z > 0.0 {
            90.0
        } else {
            270.0
        }
    } else {
        v.z.atan2(v.truncate().length()).to_degrees()
    };
    if pitch < 0.0 {
        pitch += 360.0;
    }
    (pitch, yaw)
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

    /// One frame of the anim machine with the state set by hand, for the
    /// clauses the retail capture cannot reach: it was taken with one rifle,
    /// never held two axes at once and never strafed crouched. Names rather
    /// than indices -- `animtree.rs` is what pins the index order.
    #[test]
    fn the_conditions_pick_the_clause_the_script_names() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        use pmove::Stance::{Crouch, Prone, Stand};
        // label, weapon, class, stance, backpedalling, forward, right, anim
        type Case = (
            &'static str,
            &'static str,
            &'static str,
            pmove::Stance,
            bool,
            i8,
            i8,
            &'static str,
        );
        let cases: &[Case] = &[
            (
                "a pistol runs its own loop",
                "colt_mp",
                "pistol",
                Stand,
                false,
                127,
                0,
                "pb_sprint",
            ),
            (
                "a diagonal is a forward run, not a strafe",
                "m1carbine_mp",
                "rifle",
                Stand,
                false,
                127,
                127,
                "pb_combatrun_forward_loop",
            ),
            (
                "a crouched strafe has its own clause",
                "m1carbine_mp",
                "rifle",
                Crouch,
                false,
                0,
                -127,
                "pb_crouch_run_left",
            ),
            (
                "and the weapon still picks inside it",
                "colt_mp",
                "pistol",
                Stand,
                false,
                0,
                -127,
                "pb_combatrun_left_loop_pistol",
            ),
            (
                "a crouched backpedal is its own movetype",
                "m1carbine_mp",
                "rifle",
                Crouch,
                true,
                -127,
                0,
                "pb_crouch_run_back",
            ),
            (
                "so is a backwards crawl",
                "m1carbine_mp",
                "rifle",
                Prone,
                true,
                -127,
                0,
                "pb_prone_crawl_back",
            ),
        ];
        for (label, weapon, class, stance, back, forward, right, want) in cases {
            let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
            sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
            sim.ps.stance = *stance;
            sim.ps.on_ground = true;
            sim.ps.backwards_run = *back;
            sim.ps.velocity = Vec3::new(120.0, 0.0, 0.0);
            let cmd = UserCmd {
                forward: *forward,
                right: *right,
                ..NULL_USERCMD
            };
            let inputs = AnimInputs {
                anims: &anims,
                weapon,
                weapon_class: class,
            };
            sim.update_anims(&inputs, &cmd, 1000, &[]);
            assert_eq!(anims.name(sim.anim.legs()), Some(*want), "{label}");
        }
    }

    /// Retail selects nothing while off the ground (@0x323a2), so the takeoff
    /// anim stays until the landing rather than falling back to the standing
    /// idle when the jump event's 5 ms duration runs out.
    #[test]
    fn a_jump_owns_the_legs_until_the_landing() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, 1000, &[]);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_stand_alert"));

        // The impulse pmove reports, not merely leaving the ground.
        sim.ps.on_ground = false;
        sim.jumped = true;
        sim.update_anims(&inputs, &NULL_USERCMD, 1050, &[]);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_standjump_takeoff"));
        // Well past the takeoff clause's `duration 5`.
        for t in [1100, 1150, 1200, 1250] {
            sim.update_anims(&inputs, &NULL_USERCMD, t, &[]);
            assert_eq!(
                anims.name(sim.anim.legs()),
                Some("pb_standjump_takeoff"),
                "the flight kept selecting at {t}"
            );
        }
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, 1300, &[]);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_standjump_land"));
    }

    /// Leaving the ground is not jumping. Retail's own mp_pavlov capture backs
    /// off a ledge at `run_back` and reads the run loop (index 93) while
    /// airborne, so a fall keeps whatever the legs were doing; raising the
    /// takeoff on the edge would freeze the standing jump for the whole fall.
    #[test]
    fn running_off_a_ledge_keeps_the_run_loop() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let running = UserCmd {
            forward: 127,
            ..NULL_USERCMD
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.ps.velocity = Vec3::new(190.0, 0.0, 0.0);
        sim.update_anims(&inputs, &running, 1000, &[]);
        assert_eq!(
            anims.name(sim.anim.legs()),
            Some("pb_combatrun_forward_loop")
        );

        // Off the edge: airborne, no impulse.
        sim.ps.on_ground = false;
        for t in [1050, 1100, 1150, 1200] {
            sim.ps.velocity.z -= 40.0;
            sim.update_anims(&inputs, &running, t, &[]);
            assert_eq!(
                anims.name(sim.anim.legs()),
                Some("pb_combatrun_forward_loop"),
                "the fall changed the anim at {t}"
            );
        }
    }

    /// A climber is off the ground and still animates: retail's ladder flag
    /// bypasses the airborne early-out, and the two climb blocks are the only
    /// thing that reads the climb direction. Mounting is not a jump either.
    #[test]
    fn a_ladder_climbs_rather_than_freezing() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.update_anims(&inputs, &NULL_USERCMD, 1000, &[]);

        // Mounted: off the ground without an impulse, climbing.
        sim.ps.on_ground = false;
        sim.ps.on_ladder = true;
        sim.ps.velocity = Vec3::new(0.0, 0.0, 60.0);
        sim.update_anims(&inputs, &NULL_USERCMD, 1050, &[]);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_climbup"));

        sim.ps.velocity.z = -60.0;
        sim.update_anims(&inputs, &NULL_USERCMD, 1100, &[]);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_climbdown"));
    }

    /// The backpedal latch picks the event as well as the movetype: retail's
    /// `jumpbk` block gates its first clause on the crouched and prone
    /// movetypes, which is the only place the two events differ for a
    /// rifleman.
    #[test]
    fn a_backwards_crouched_jump_raises_jumpbk() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let back = UserCmd {
            forward: -127,
            ..NULL_USERCMD
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.stance = pmove::Stance::Crouch;
        sim.ps.backwards_run = true;
        sim.ps.on_ground = true;
        sim.ps.velocity = Vec3::new(120.0, 0.0, 0.0);
        sim.update_anims(&inputs, &back, 1000, &[]);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_crouch_run_back"));

        sim.ps.on_ground = false;
        sim.jumped = true;
        sim.update_anims(&inputs, &back, 1050, &[]);
        assert_eq!(
            anims.name(sim.anim.legs()),
            Some("pb_chicken_dance_crouch"),
            "`jump` would have given the standing takeoff"
        );
    }

    /// Retail updates the strafe condition only from a cmd that asks for
    /// movement: a cmd asking for neither axis leaves it alone, so a player
    /// coasting out of a strafe keeps strafing (@0x32504).
    #[test]
    fn a_cmd_asking_for_nothing_leaves_the_strafe_condition_alone() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.ps.velocity = Vec3::new(120.0, 0.0, 0.0);
        let strafe = UserCmd {
            right: 127,
            ..NULL_USERCMD
        };
        sim.update_anims(&inputs, &strafe, 1000, &[]);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_combatrun_right_loop"));
        // Still sliding, no longer asking: retail does not touch the condition.
        sim.update_anims(&inputs, &NULL_USERCMD, 1050, &[]);
        assert_eq!(anims.name(sim.anim.legs()), Some("pb_combatrun_right_loop"));
        // A forward cmd clears it, even though nothing about the velocity
        // changed.
        let forward = UserCmd {
            forward: 127,
            ..NULL_USERCMD
        };
        sim.update_anims(&inputs, &forward, 1100, &[]);
        assert_eq!(
            anims.name(sim.anim.legs()),
            Some("pb_combatrun_forward_loop")
        );
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
            sim.step(&prone(t, 0.0), 0.05, Some(&w), &[]);
        }
        assert_eq!(sim.ps.stance, pmove::Stance::Prone);
        let before = sim.delta_angles[1];
        assert_eq!(
            sim.to_wire(p, 0, 0).field_i32(p, "proneDirection"),
            sim.ps.prone_direction.to_bits() as i32
        );

        // Well past the cone: the body cannot swing the whole way in one
        // frame, so the rest comes off the view.
        sim.step(&prone(t + 50, 150.0), 0.05, Some(&w), &[]);
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
            sim.step(&NULL_USERCMD, 0.05, Some(&w), &[]);
        }
        assert_eq!(lerp_time(&sim), 0, "a settled eye carries no stamp");

        t += 50;
        sim.step(&crouch(t), 0.05, Some(&w), &[]);
        assert_eq!(lerp_time(&sim), t, "the stamp is the cmd that started it");
        let started = t;
        t += 50;
        sim.step(&crouch(t), 0.05, Some(&w), &[]);
        assert_eq!(
            lerp_time(&sim),
            started,
            "and it does not move while lerping"
        );

        for _ in 0..20 {
            t += 50;
            sim.step(&crouch(t), 0.05, Some(&w), &[]);
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

    /// The weapon bits reach the weapon half of pmove, and nothing else: CoD 1
    /// has a single move speed with no walk key, so no input reaches pmove's
    /// walk scale.
    #[test]
    fn weapon_bits_reach_the_weapon_input_only() {
        let all = pm_input(&UserCmd {
            buttons: 0xff,
            wbuttons: msg::WBUTTON_RELOAD,
            weapon: 7,
            ..Default::default()
        });
        assert!(all.attack && all.reload && all.ads && all.use_button);
        assert_eq!(all.weapon, 7);
        assert!(!all.walk_slow);
        assert_eq!(
            PmInput {
                attack: false,
                reload: false,
                ads: false,
                use_button: false,
                weapon: 0,
                ..all
            },
            PmInput::default()
        );
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
        sim.step(&c, 0.05, None, &[]);
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
        sim.step(&cmd, 0.05, None, &[]);
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
        sim.step(&cmd(0, 0, 0), 0.05, None, &[]);
        assert_eq!(sim.ps.yaw.to_degrees(), 90.0);
    }

    /// The retail hit (combat doc, 8.4): the shooter at (1810, 2109.5), the
    /// target at (1800, 2696), a level carbine round to the head.
    fn hit_op(damage: i32, fatal: bool) -> SimOp {
        // A hair downward: the capture's `damagePitch` 255 is a pitch just
        // short of 360, which a perfectly level shot would read as 0.
        let dir = Vec3::new(-10.0, 586.5, -0.5).normalize();
        SimOp::Damaged {
            damage,
            point: [1800.0, 2681.0, 36.1],
            dir: dir.into(),
            knockback: true,
            attacker: Some(1),
            attacker_origin: Some([1810.0, 2109.5, -23.9]),
            fatal,
        }
    }

    fn target() -> ClientSim {
        let mut sim = ClientSim::spectator([1800.0, 2696.0, -23.9], 0.0, NULL_USERCMD.angles);
        sim.become_player([1800.0, 2696.0, -23.9], 0.0, NULL_USERCMD.angles);
        sim.ps.on_ground = true;
        sim.health = 100;
        sim.max_health = 100;
        sim
    }

    /// One surviving hit writes the four feedback fields, the pain event
    /// and the knockback the capture measured; a second within 700 ms
    /// counts but raises no second `EV_PAIN`; one after does.
    #[test]
    fn a_hit_writes_the_feedback_the_capture_measured() {
        let p = &PROTOCOL_V1;
        let mut sim = target();
        sim.take_damage(&hit_op(67, false), None, &mut 1, 1000);
        sim.health = 33; // the host's mirror
        sim.end_frame(1000);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageEvent"), 1);
        assert_eq!(w.field_i32(p, "damageCount"), 67);
        assert_eq!(w.field_i32(p, "damageYaw"), 64);
        assert_eq!(w.field_i32(p, "damagePitch"), 255);
        assert_eq!(w.field_i32(p, "eventSequence"), 1);
        assert_eq!(w.field_i32(p, "events[0]"), EV_PAIN);
        assert_eq!(w.field_i32(p, "eventParms[0]"), 33);
        assert_eq!(w.field_i32(p, "pm_type"), 0);
        assert_eq!(w.health(), 33);
        assert!(
            (sim.ps.velocity.y - 80.0).abs() < 0.5 && sim.ps.velocity.x.abs() < 2.0,
            "80 u/s along the bearing, got {:?}",
            sim.ps.velocity
        );

        sim.take_damage(&hit_op(10, false), None, &mut 1, 1300);
        sim.health = 23;
        sim.end_frame(1300);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageEvent"), 2);
        assert_eq!(w.field_i32(p, "damageCount"), 10);
        assert_eq!(
            w.field_i32(p, "eventSequence"),
            1,
            "no second EV_PAIN inside 700 ms"
        );

        sim.take_damage(&hit_op(10, false), None, &mut 1, 1800);
        sim.health = 13;
        sim.end_frame(1800);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "eventSequence"), 2);
        assert_eq!(w.field_i32(p, "events[1]"), EV_PAIN);
        assert_eq!(w.field_i32(p, "eventParms[1]"), 13);

        // A frame with no damage changes none of the four.
        sim.end_frame(1850);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageEvent"), 3);
        assert_eq!(w.field_i32(p, "damageYaw"), 64);
    }

    /// The killing hit: `EV_DEATH` with parm 0, `pm_type` 6, the dead yaw
    /// toward the attacker in `stats[1]`, and the feedback left as the last
    /// surviving hit wrote it (combat doc, 8.4). Then the body: no input
    /// moves it, and the eye drops 9 units a frame to `deadViewHeight`.
    #[test]
    fn a_fatal_hit_kills_freezes_the_feedback_and_drops_the_eye() {
        let p = &PROTOCOL_V1;
        let w_test = vcod_common::collision::test_world(&[]);
        // On the test floor, with the attacker at the capture's bearing.
        let mut sim = ClientSim::spectator([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        sim.health = 100;
        sim.max_health = 100;
        let op = |fatal: bool| {
            let mut op = hit_op(67, fatal);
            let SimOp::Damaged {
                attacker_origin, ..
            } = &mut op;
            *attacker_origin = Some([10.0, -586.5, 8.0]);
            op
        };
        for _ in 0..20 {
            sim.step(&NULL_USERCMD, 0.05, Some(&w_test), &[]);
        }
        sim.take_damage(&op(false), None, &mut 1, 1000);
        sim.health = 33;
        sim.end_frame(1000);
        for _ in 0..10 {
            sim.step(&NULL_USERCMD, 0.05, Some(&w_test), &[]);
        }
        assert_eq!(sim.ps.velocity.length(), 0.0, "the knockback has decayed");

        sim.take_damage(&op(true), None, &mut 1, 2000);
        sim.health = 0;
        sim.end_frame(2000);
        assert!(sim.dead);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "pm_type"), PM_DEAD);
        assert_eq!(w.health(), 0);
        assert_eq!(
            w.arrays.stats[1], 270,
            "the attacker sits at bearing 270.98"
        );
        assert_eq!(w.field_i32(p, "eventSequence"), 2);
        assert_eq!(w.field_i32(p, "events[1]"), EV_DEATH);
        assert_eq!(w.field_i32(p, "eventParms[1]"), 0);
        assert_eq!(
            w.field_i32(p, "damageEvent"),
            1,
            "the killing hit runs no feedback"
        );
        assert_eq!(w.field_i32(p, "damageCount"), 67);
        assert_eq!(w.field_i32(p, "deadViewHeight"), 8);
        assert_eq!(w.field_f32(p, "viewHeightCurrent"), 60.0);
        assert_eq!(
            w.field_i32(p, "torsoAnim"),
            512,
            "the torso restarts on nothing"
        );

        let run = UserCmd {
            forward: 127,
            buttons: msg::BUTTON_ATTACK,
            ..NULL_USERCMD
        };
        for expect in [51.0, 42.0, 33.0, 24.0, 15.0, 8.0, 8.0] {
            let events = sim.step(&run, 0.05, Some(&w_test), &[]);
            assert!(events.is_empty(), "a dead player fires nothing");
            assert_eq!(
                sim.to_wire(p, 0, 0).field_f32(p, "viewHeightCurrent"),
                expect
            );
        }
        // The body slid on the killing hit's knockback and friction has
        // stopped it; from here only input could move it, and none does.
        assert_eq!(sim.ps.velocity.truncate(), glam::Vec2::ZERO);
        let before = sim.ps.origin;
        for _ in 0..5 {
            sim.step(&run, 0.05, Some(&w_test), &[]);
        }
        assert_eq!(sim.ps.origin, before, "input does not move a body");

        // A respawn clears all of it.
        sim.become_player([0.0, 0.0, 8.0], 0.0, NULL_USERCMD.angles);
        let w = sim.to_wire(p, 0, 0);
        assert!(!sim.dead);
        assert_eq!(w.field_i32(p, "pm_type"), 0);
        assert_eq!(w.field_i32(p, "damageEvent"), 0);
        assert_eq!(w.arrays.stats[1], 0);
        // Retail's first frame of a new life reads an empty ring at sequence
        // 0 (combat doc, 9.2).
        assert_eq!(
            w.field_i32(p, "eventSequence"),
            0,
            "the respawn did not clear the ring"
        );
        assert_eq!(w.field_i32(p, "events[0]"), 0);
    }

    /// No direction is the 255/255 sentinel, and no knockback moves nothing.
    #[test]
    fn a_hit_with_no_direction_marks_both_angles_255() {
        let p = &PROTOCOL_V1;
        let mut sim = target();
        let op = SimOp::Damaged {
            damage: 20,
            point: [0.0; 3],
            dir: [0.0; 3],
            knockback: false,
            attacker: None,
            attacker_origin: None,
            fatal: false,
        };
        sim.take_damage(&op, None, &mut 1, 500);
        sim.end_frame(500);
        let w = sim.to_wire(p, 0, 0);
        assert_eq!(w.field_i32(p, "damageYaw"), 255);
        assert_eq!(w.field_i32(p, "damagePitch"), 255);
        assert_eq!(w.field_i32(p, "damageCount"), 20);
        assert_eq!(sim.ps.velocity, Vec3::ZERO);
    }

    /// A spectator noclips, so flight needs no collision world and a server
    /// whose map failed to load still flies its spectators.
    #[test]
    fn step_flies_without_a_collision_world() {
        let mut sim = ClientSim::spectator([1.0, 2.0, 3.0], 0.0, NULL_USERCMD.angles);
        let c = cmd(127, 0, 0);
        for _ in 0..125 {
            sim.step(&c, 1.0 / 125.0, None, &[]);
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
