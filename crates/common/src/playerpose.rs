//! Posing a player: the clip a wire `legsAnim`/`torsoAnim` value names, and
//! the aim layer over it. Shared by the client's draw path and the server's
//! locational trace. `docs/research/player-model-anim-system.md`.

use crate::animtree::{AnimTree, PlayerAnims};
use crate::net::flags::{EF_CROUCH, EF_DEAD, EF_FIRING, EF_MOUNTED, EF_PRONE};
use crate::pmove::MoveRules;
use crate::pmove::aim::{
    angle_normalize_180, angle_normalize_360 as angle_mod, angle_subtract, lean_fraction,
};
use crate::skeleton::{AnimBinding, Blend, PoseBuffer, Record, Skeleton};
use crate::turretpose::TurretRow;
use crate::xanim::XAnim;
use glam::{Quat, Vec3};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Pitch,
    Yaw,
}

/// `15down` -> pitch +15, `30left` -> yaw -30. Pitch is down-positive (engine
/// view pitch), yaw right-positive (offset from the body facing).
fn suffix_angle(tok: &str) -> Option<(Axis, f32)> {
    match tok {
        "level" => return Some((Axis::Pitch, 0.0)),
        "forward" => return Some((Axis::Yaw, 0.0)),
        _ => {}
    }
    for (word, axis, sign) in [
        ("down", Axis::Pitch, 1.0),
        ("up", Axis::Pitch, -1.0),
        ("left", Axis::Yaw, -1.0),
        ("right", Axis::Yaw, 1.0),
    ] {
        if let Some(num) = tok.strip_suffix(word)
            && let Ok(deg) = num.parse::<f32>()
        {
            return Some((axis, sign * deg));
        }
    }
    None
}

/// The angle `name`'s last token on `axis` annotates (`..._30right_15down`:
/// pitch +15, yaw +30).
fn name_angle(name: &str, axis: Axis) -> Option<f32> {
    name.split('_').rev().find_map(|tok| {
        suffix_angle(tok)
            .filter(|&(a, _)| a == axis)
            .map(|(_, v)| v)
    })
}

/// Picks the child nearest the requested angle on whichever axis every child
/// annotates and disagrees on (MG42: pitch rows, then yaw columns). Falls back
/// to the middle child.
fn pick_child(tree: &AnimTree, children: &[usize], pitch_deg: f32, yaw_deg: f32) -> usize {
    for (axis, want) in [(Axis::Pitch, pitch_deg), (Axis::Yaw, yaw_deg)] {
        let angles: Option<Vec<f32>> = children
            .iter()
            .map(|&c| name_angle(&tree.nodes[c].name, axis))
            .collect();
        let Some(angles) = angles else { continue };
        if angles.iter().all(|a| *a == angles[0]) {
            continue; // shared annotation, carries no choice
        }
        let best = angles
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (*a - want).abs().total_cmp(&(*b - want).abs()))
            .map(|(i, _)| i)
            .unwrap_or(0);
        return children[best];
    }
    children[children.len() / 2]
}

/// Walks an aim group down to a leaf. Only the MG42 gunner groups are
/// non-leaves in the shipped tree.
pub fn descend_aim(tree: &AnimTree, node: usize, pitch_deg: f32, yaw_deg: f32) -> usize {
    let mut node = node;
    // cycle guard; the shipped tree nests two levels
    for _ in 0..8 {
        let children = &tree.nodes[node].children;
        if children.is_empty() {
            return node;
        }
        node = pick_child(tree, children, pitch_deg, yaw_deg);
    }
    node
}

/// `bg_swingSpeed`'s default, the speed the torso and legs yaw swing at.
pub const BG_SWING_SPEED: f32 = 0.2;

/// What the angle updater reads off the entity and the client's record:
/// `ClientEndFrame` (0x41273..0x412a6) copies `ps.viewangles`, the entity's
/// `angles2[1]` (`movementDir`) and its `leanf` into the record.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BodyInput {
    /// `ps.viewangles` (the entity's `apos` on the client): engine degrees,
    /// pitch down positive, yaw left positive.
    pub view: [f32; 3],
    /// `ps.movementDir`: the legs' heading off the view, degrees.
    pub movement_dir: f32,
    /// The entity's `eFlags`.
    pub eflags: i32,
    /// The legs anim's record (`BG_ParseCommands`' flags).
    pub legs: crate::animscript::AnimRecord,
    /// UO: what the rider's seat lets him hold. Read only under a rider
    /// bit; 1.1's and 1.5's updaters have no such test.
    pub seat_weapon: SeatWeapon,
}

/// UO's seat test in the angle updater (game.mp.uo 0x1dbac, uo cgame
/// 0x30004550): `BG_AllowPlayerWeaponAtVehiclePos` on the entity's seat
/// (`s+0x88`: a four-wheel's seat 3) and the entity's `weapon` (`s+0xcc`).
/// coduo-vehicles.md 13, "The angle updater on a rider".
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum SeatWeapon {
    /// On foot, on a turret, or in a seat that holds no weapon: a rider
    /// here swings all three angles every frame.
    #[default]
    Barred,
    /// The weapon seat, empty handed.
    Allowed,
    /// The weapon seat with a weapon in hand: the legs swing toward the
    /// seat tag's yaw (the record's `+0x3f8`) before the rider's yaw
    /// overwrites them.
    Held { seat_yaw: f32 },
}

impl SeatWeapon {
    /// The test off the seat's `(kind, seat)` and the entity's `weapon`.
    pub fn of(kind: i32, seat: i32, weapon: i32, seat_yaw: f32) -> Self {
        match (kind == 1 && seat == 3, weapon != 0) {
            (false, _) => SeatWeapon::Barred,
            (true, false) => SeatWeapon::Allowed,
            (true, true) => SeatWeapon::Held { seat_yaw },
        }
    }
}

impl BodyInput {
    fn dead(&self) -> bool {
        self.eflags & EF_DEAD != 0
    }
    fn mounted(&self) -> bool {
        self.eflags & EF_MOUNTED != 0
    }
    /// On a turret or, on UO, in a vehicle seat: UO's updater tests
    /// `eFlags & 0x106000` (game.mp.uo 0x1dbac, uo cgame 0x30004550), 1.1's
    /// and 1.5's `0xc000` (cgame 0x300040e0, 1.5 cgame 0x30004330).
    fn rides(&self) -> bool {
        self.eflags & (EF_MOUNTED | crate::net::flags::EF_IN_VEHICLE_V22) != 0
    }
    /// A rider the updater swings all three angles for: any but one in the
    /// seat that allows his weapon.
    fn rides_bare(&self) -> bool {
        self.rides() && self.seat_weapon == SeatWeapon::Barred
    }
    /// The seat yaw a rider's legs swing toward instead of the view's.
    fn held_seat_yaw(&self) -> Option<f32> {
        match self.seat_weapon {
            SeatWeapon::Held { seat_yaw } if self.rides() => Some(seat_yaw),
            _ => None,
        }
    }
    fn prone(&self) -> bool {
        self.eflags & EF_PRONE != 0
    }
}

/// `eFlags` 0x200, which `BG_PlayerStateToEntityState` sets off `pm_flags`
/// 0x20, the sight flag (0x2cdb6).
pub const EF_ADS: i32 = 0x200;

/// The record's `movetype` bits for `climbup` and `climbdown`.
const MOVETYPE_CLIMB: u32 = 0x30000;
/// CoD4's: its movetype list puts the climbs at 18 and 19 (1.7's
/// `BG_Player_DoControllersInternal` tests 0xc0000 at 0x804cff8).
const MOVETYPE_CLIMB_COD4: u32 = 0xc0000;

/// CoD4's lean dvars as `BG_Player_DoControllersInternal` reads them: the
/// `tag_origin` shift a unit of lean (`player_lean_shift_*`) and the
/// spine's and head's roll scale (`player_lean_rotate_*`), left and right,
/// standing and crouched (cod4-player-anims.md 8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeanDvars {
    pub shift_left: f32,
    pub shift_right: f32,
    pub shift_crouch_left: f32,
    pub shift_crouch_right: f32,
    pub rotate_left: f32,
    pub rotate_right: f32,
    pub rotate_crouch_left: f32,
    pub rotate_crouch_right: f32,
}

impl Default for LeanDvars {
    /// 1.7's defaults, as the server's dvar mirror carries them.
    fn default() -> Self {
        LeanDvars {
            shift_left: 5.0,
            shift_right: 9.0,
            shift_crouch_left: 4.0,
            shift_crouch_right: 15.0,
            rotate_left: 0.8,
            rotate_right: 0.4,
            rotate_crouch_left: 0.8,
            rotate_crouch_right: 0.3,
        }
    }
}
/// `idle` and `idlecr`.
const MOVETYPE_IDLE: u32 = 0x6;

/// The body's three swings, kept in the client's record across frames:
/// legs yaw and its flag at `+0x37c`/`+0x380`, torso yaw at `+0x3ac`/`+0x3b0`,
/// torso pitch at `+0x3b4`/`+0x3b8`, and the two conditions the updater
/// reads, `movetype` (`+0x410`) and `firing` (`+0x428`).
/// `docs/research/cod11-combat.md` 16.3 and 16.4.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BodyAngles {
    pub legs_yaw: f32,
    pub legs_yawing: bool,
    pub torso_yaw: f32,
    pub torso_yawing: bool,
    /// Degrees, engine convention (down positive), folded to 0..360.
    pub pitch: f32,
    pub pitching: bool,
    pub movetype: u32,
    pub firing: bool,
}

impl BodyAngles {
    /// The condition half of `BG_PlayerAnimation` (`game.mp.i386.so`
    /// 0x2b328): `movetype` from the legs anim's record when it carries any
    /// bit, `firing` from `eFlags` 0x400. Retail runs it after
    /// [`Self::step`], so the step reads the previous frame's; a server also
    /// writes `movetype` from pmove ahead of the step.
    pub fn update_conditions(&mut self, input: &BodyInput) {
        if input.legs.movetypes != 0 {
            self.movetype = input.legs.movetypes;
        }
        self.firing = input.eflags & EF_FIRING != 0;
    }

    /// The angle updater (`game.mp.i386.so` 0x2af78, cgame 0x300040e0) for
    /// one frame of `frametime_ms`: the torso yaw toward the view (less 0.3
    /// of the legs' heading off it), the legs toward their heading once 40
    /// degrees off it, the torso pitch toward 0.6 of the view's. A rider
    /// faces the view with no pitch; the UO server's and cgame's copies
    /// (game.mp.uo 0x1dbac, uo cgame 0x30004550) add the seat test of
    /// [`SeatWeapon`].
    pub fn step(&mut self, input: &BodyInput, frametime_ms: i32, swing_speed: f32) {
        self.step_by(Updater::Cod1, input, frametime_ms, swing_speed);
    }

    /// [`Self::step`] under `updater`'s rules: CoD2's
    /// (`cod2_lnxded` 0x80da9c8, inlined in the Mac `BG_PlayerAnimation`
    /// 0x183ef2) adds the mantle and clamps a swing to its full limit.
    /// docs/research/cod2-player-anims.md 5.
    pub fn step_by(
        &mut self,
        updater: Updater,
        input: &BodyInput,
        frametime_ms: i32,
        swing_speed: f32,
    ) {
        let ft = frametime_ms as f32;
        let m = input.movement_dir;
        let yaw = angle_mod(input.view[1]);
        let climb = match updater {
            Updater::Cod4(_) => MOVETYPE_CLIMB_COD4,
            _ => MOVETYPE_CLIMB,
        };
        let climbing = self.movetype & climb != 0;
        let (mantle, margin, pitch_fraction) = match updater {
            Updater::Cod1 => (Mantle::default(), 1.0, 0.6),
            Updater::Cod2(mantle) => (mantle, 0.0, 0.6),
            // `fadd %st(0),%st` at 0x804cefe.
            Updater::Cod4(mantle) => (mantle, 0.0, 2.0),
        };
        let swing_angles =
            |dest, tolerance, clamp: f32, speed, ft, angle: &mut f32, on: &mut bool| {
                swing_angles(
                    dest,
                    tolerance,
                    (clamp, clamp - margin),
                    speed,
                    ft,
                    angle,
                    on,
                )
            };
        if !input.rides_bare() && !climbing && !mantle.on && self.movetype & MOVETYPE_IDLE != 0 {
            if self.firing {
                self.torso_yawing = true;
                self.pitching = true;
            }
        } else {
            self.torso_yawing = true;
            self.pitching = true;
            self.legs_yawing = true;
        }

        let (torso_dest, torso_clamp) = if input.dead() {
            (yaw, 90.0)
        } else if climbing {
            (yaw + m, 0.0)
        } else if mantle.on || input.prone() {
            // CoD2 tests the mantle ahead of the stance.
            (yaw, 90.0)
        } else if input.eflags & EF_FIRING != 0 {
            (yaw, 45.0)
        } else if input.eflags & EF_ADS != 0 {
            (yaw, 90.0)
        } else {
            (yaw + m * 0.3, 90.0)
        };
        swing_angles(
            torso_dest,
            0.0,
            torso_clamp,
            swing_speed,
            ft,
            &mut self.torso_yaw,
            &mut self.torso_yawing,
        );

        let legs = |dest: f32, tolerance: f32, a: &mut Self| {
            swing_angles(
                dest,
                tolerance,
                150.0,
                swing_speed,
                ft,
                &mut a.legs_yaw,
                &mut a.legs_yawing,
            );
        };
        // An armed rider's legs swing toward the seat, which only moves
        // their flag: the rider's yaw below overwrites the angle.
        let seat = input.held_seat_yaw();
        // A mantling body's legs head for the view, not the move.
        let legs_dest = if mantle.on && !climbing { yaw } else { yaw + m };
        if input.dead() {
            legs(seat.unwrap_or(yaw), 0.0, self);
        } else if input.prone() {
            self.legs_yawing = false;
            self.legs_yaw = m + yaw;
        } else if input.legs.strafe {
            self.legs_yawing = false;
            legs(yaw, 0.0, self);
        } else if self.legs_yawing {
            legs(seat.unwrap_or(legs_dest), 0.0, self);
        } else {
            legs(seat.unwrap_or(legs_dest), 40.0, self);
        }
        if input.rides() {
            self.torso_yaw = yaw;
            self.legs_yaw = yaw;
        } else if climbing {
            self.torso_yaw = yaw + m;
            self.legs_yaw = yaw + m;
        }

        let pitch_dest = if input.dead() || input.rides() || climbing || mantle.alone {
            0.0
        } else {
            let p = input.view[0];
            (if p > 180.0 { p - 360.0 } else { p }) * pitch_fraction
        };
        swing_angles(
            pitch_dest,
            0.0,
            45.0,
            0.15,
            ft,
            &mut self.pitch,
            &mut self.pitching,
        );
    }
}

/// Which game's angle updater [`BodyAngles::step_by`] runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Updater {
    Cod1,
    Cod2(Mantle),
    /// 1.7's `BG_PlayerAngles` (`cod4_lnxded` 0x804c636): CoD2's with the
    /// climbs at CoD4's movetypes and the torso pitch toward twice the
    /// view's (cod4-player-anims.md 8).
    Cod4(Mantle),
}

/// CoD2's mantle (`eFlags` 0x4000), which 1.1's numbering has no bit for:
/// `on` swings the torso and legs at the view; `alone`, the mantle as the
/// entity's only flag (`cmpl $0x4000` at 0x80daf19, an equality, not a bit
/// test), also holds the pitch level.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mantle {
    pub on: bool,
    pub alone: bool,
}

/// `BG_SwingAngles` (`game.mp.i386.so` 0x2ae00, cgame 0x30003ec0): Q3's
/// `CG_SwingAngles` with the step scale `max(|swing| * 0.05, 0.5)`. A swing
/// past `clamp` is set back to `limit` off `dest`: 1.1 puts it a degree
/// inside, CoD2 (0x80da7dc) on the clamp itself.
fn swing_angles(
    dest: f32,
    tolerance: f32,
    (clamp, limit): (f32, f32),
    speed: f32,
    frametime_ms: f32,
    angle: &mut f32,
    swinging: &mut bool,
) {
    if !*swinging {
        let swing = angle_subtract(*angle, dest);
        if swing > tolerance || swing < -tolerance {
            *swinging = true;
        }
    }
    if *swinging {
        let swing = angle_subtract(dest, *angle);
        let scale = (swing.abs() * 0.05).max(0.5);
        if swing >= 0.0 {
            let mut step = frametime_ms * scale * speed;
            if step >= swing {
                step = swing;
                *swinging = false;
            }
            *angle = angle_mod(*angle + step);
        } else {
            let mut step = -frametime_ms * scale * speed;
            if step <= swing {
                step = swing;
                *swinging = false;
            }
            *angle = angle_mod(*angle + step);
        }
    }
    let swing = angle_subtract(dest, *angle);
    if swing > clamp {
        *angle = angle_mod(dest - limit);
    } else if swing < -clamp {
        *angle = angle_mod(dest + limit);
    }
}

/// What the entity adds to [`BodyInput`] for the controllers: the lean
/// (`leanf`, -1..1, right positive) and the prone slope terms
/// `fTorsoHeight`, `fTorsoPitch`, `fWaistPitch`.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BodySlope {
    pub lean: f32,
    pub torso_height: f32,
    pub torso_pitch: f32,
    pub waist_pitch: f32,
}

/// A UO rider's seat as the controllers read it: the seat tag's world
/// angles, which `G_PlayerVehiclePositionAndBlend` (game.mp.uo 0x9048f)
/// writes into the client's record (`+0x3f4`) and the entity's
/// `r.currentAngles`, and whether the seat holds a weapon
/// (`BG_AllowPlayerWeaponAtVehiclePos` 0x23148: a four-wheel's seat 3).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct BodySeat {
    /// Engine degrees, `AxisToAngles` of the tag's axis.
    pub angles: [f32; 3],
    pub weapon: bool,
    /// The vehicle's kind (its `surfType`: 1 four-wheel, 2 tank, 5
    /// artillery) and the seat, which pick the seat blend.
    pub kind: i32,
    pub seat: i32,
    /// The vehicle's `s.leanf` (entity `+0xd8`), its front wheels' steer,
    /// that a four-wheel's driver and passenger blend by.
    pub steer: f32,
    /// The steer the end child takes its whole weight at:
    /// [`STEER_FULL_GAME`] or [`STEER_FULL_CGAME`].
    pub steer_full: f32,
}

/// The server's full-blend steer (game.mp.uo `.rodata` 0xa7b10).
pub const STEER_FULL_GAME: f32 = 45.0;
/// The cgame's (uo_cgame_mp_x86.dll 0x3007c2c8, `1 / 60`), which draws a
/// rider's turn shallower than the server poses it.
pub const STEER_FULL_CGAME: f32 = 60.0;

impl BodySeat {
    /// The server's seat. `BG_AllowPlayerWeaponAtVehiclePos`: vehicle kind
    /// 1 (a four-wheel), seat 3.
    pub fn new(angles: [f32; 3], kind: i32, seat: i32, steer: f32) -> Self {
        BodySeat {
            angles,
            weapon: kind == 1 && seat == 3,
            kind,
            seat,
            steer,
            steer_full: STEER_FULL_GAME,
        }
    }

    /// The seat as the cgame draws it (0x30032fe0).
    pub fn drawn(self) -> Self {
        BodySeat {
            steer_full: STEER_FULL_CGAME,
            ..self
        }
    }

    /// The controllers turn the torso with the view only for a rider who
    /// holds a weapon: entity `weapon` non-zero (game.mp.uo 0x1e761, cgame
    /// 0x30004da0).
    pub fn armed(self, weapon: i32) -> Self {
        BodySeat {
            weapon: self.weapon && weapon != 0,
            ..self
        }
    }
}

/// `G_PlayerVehiclePositionAndBlend`'s goal weights (game.mp.uo 0x9048f)
/// on a rider's `turretanim` legs anim, for a four-wheel's seats 1..3, an
/// artillery piece's and a tank's seat 2: under the anim's child at
/// `numChildren / 2`, its own middle child at `1 - s` and, for a
/// four-wheel's seat 1 or 3, an end child at `s = min(|steer| / full, 1)`
/// (`full` [`BodySeat::steer_full`]), the last for a positive steer and
/// the first for a negative, in the engine's child order (the file's
/// reversed). The leaves and their weights; `None` where the engine sets
/// none and the anim plays as the tree has it. The cgame's copy
/// (0x30032fe0) sets the same goal weights. docs/research/coduo-vehicles.md
/// 13.
pub fn seat_leaves(
    anims: &PlayerAnims,
    legs_anim: i32,
    seat: &BodySeat,
) -> Option<Vec<(usize, f32)>> {
    let blends = match seat.kind {
        1 => (1..=3).contains(&seat.seat),
        5 => true,
        2 => seat.seat == 2,
        _ => false,
    };
    let tree = &anims.tree;
    let node = anims.index.node_id(legs_anim)?;
    if !blends || !anims.script.is_turret_anim(&tree.nodes[node].name) {
        return None;
    }
    // Engine child `i` of a node: the file's `len - 1 - i`.
    let child = |n: usize, i: usize| {
        let c = &tree.nodes[n].children;
        c[c.len() - 1 - i]
    };
    let n = tree.nodes[node].children.len();
    if n == 0 {
        return None;
    }
    let group = child(node, n / 2);
    let m = tree.nodes[group].children.len();
    if m == 0 {
        return None;
    }
    let mid = child(group, m / 2);
    let s = if seat.kind == 1 && seat.seat != 2 {
        (seat.steer.abs() / seat.steer_full).min(1.0)
    } else {
        0.0
    };
    if s == 0.0 {
        return Some(vec![(mid, 1.0)]);
    }
    let end = child(group, if seat.steer > 0.0 { m - 1 } else { 0 });
    Some(vec![(mid, 1.0 - s), (end, s)])
}

/// [`seat_leaves`] keyed by the legs anim's node, as
/// [`PoseInputs::seat_leaves`] takes them.
pub fn seat_inputs(
    anims: &PlayerAnims,
    legs_anim: i32,
    seat: &BodySeat,
) -> Option<(usize, Vec<(usize, f32)>)> {
    let node = anims.index.node_id(legs_anim)?;
    Some((node, seat_leaves(anims, legs_anim, seat)?))
}

/// The server's seat blend as the engine plays it: each frame
/// `G_PlayerVehiclePositionAndBlend` (game.mp.uo 0x9048f) clears the legs
/// anim's tree to goal 0 at once and sets [`seat_leaves`]' goal weights
/// at a rate of `1 / (10 |goal - weight|)`, which the setter turns into a
/// goal time of `|goal - weight|` times that: every leaf runs in a straight
/// line to its goal over 100 ms from where the frame found it (a new
/// node's from none), and a leaf no longer set lands on 0 at the next
/// frame's step
/// (docs/research/coduo-vehicles.md 11.4, cod11-movers.md 15 "The weight
/// ramp"). The cgame's copy sets its goals over one frame, so a drawn rider
/// plays the goal weights. The node a legs change leaves keeps its leaves:
/// nothing clears them, the old legs clip fades out on them
/// (coduo-vehicles.md 13, "The glide back"), and they wait on their goals
/// for a return to it ([`Self::fade_held`]; coduo-vehicles.md 14, "The
/// seat blend's ramp").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeatRamp {
    node: Option<usize>,
    /// Leaf, weight at `start_ms`, goal, and the ms to reach it.
    leaves: Vec<(usize, f32, f32, f32)>,
    /// The node time of each leaf [`PlayerAnims::leaf_motion`] knows, from
    /// when it was last given weight from none.
    times: Vec<(usize, crate::animclock::LeafTime)>,
    start_ms: i32,
    /// The ramp of the node last left, its own `held` dropped.
    held: Option<Box<SeatRamp>>,
    /// Held under a legs node faded to 0: its weights sit on their goals
    /// and its times do not step ([`Self::fade_held`]).
    still: bool,
    /// The last end frame's level time.
    last_ms: i32,
}

/// `G_PlayerVehiclePositionAndBlend`'s goal time, `1 / 10` s (0xa7b28 is
/// 10.0).
const SEAT_RAMP_MS: f32 = 100.0;

impl SeatRamp {
    fn weight(&self, i: usize, now_ms: i32) -> f32 {
        let (_, from, to, ms) = self.leaves[i];
        if ms <= 0.0 {
            return to;
        }
        let s = now_ms.wrapping_sub(self.start_ms) as f32 / ms;
        if s >= 1.0 {
            to
        } else {
            from + (to - from) * s.max(0.0)
        }
    }

    /// One end frame's goals at `now_ms`: [`seat_inputs`]' node and leaves,
    /// `None` for a body with no seat blend. `motion` is
    /// [`PlayerAnims::leaf_motion`]: a leaf it knows starts its node time at
    /// 0 whenever it is set from no weight, as the engine allocates it
    /// afresh (UO `coduo_lnxded` 0x80c05a2, cod11-combat.md 3.1).
    pub fn set(
        &mut self,
        goal: Option<(usize, Vec<(usize, f32)>)>,
        now_ms: i32,
        motion: impl Fn(usize) -> Option<(f32, bool)>,
    ) {
        for (_, t) in self.times.iter_mut() {
            t.settle(now_ms);
        }
        if let Some(h) = self.held.as_mut() {
            for (_, t) in h.times.iter_mut() {
                if h.still {
                    t.resume(now_ms);
                } else {
                    t.settle(now_ms);
                }
            }
        }
        self.last_ms = now_ms;
        let fresh = |leaf: usize| {
            motion(leaf)
                .map(|(f, looping)| (leaf, crate::animclock::LeafTime::new(now_ms, f, looping)))
        };
        let mut back = None;
        if self.node.is_some() && goal.as_ref().map(|g| g.0) != self.node {
            back = self.held.take().map(|h| *h);
            self.held = Some(Box::new(std::mem::take(self)));
            self.last_ms = now_ms;
        }
        let Some((node, goal)) = goal else {
            self.node = None;
            self.leaves.clear();
            self.times.clear();
            return;
        };
        if self.node != Some(node) {
            // A node just entered: its leaves ramp from the weights it
            // still holds (the node last left's), none if it was never set,
            // as on any other frame (coduo-vehicles.md 14, "The seat
            // blend's ramp").
            self.node = Some(node);
            self.leaves.clear();
            self.times.clear();
            if self.held.as_ref().is_some_and(|h| h.node == Some(node)) {
                back = self.held.take().map(|h| *h);
            }
            if let Some(b) = back.filter(|b| b.node == Some(node)) {
                self.leaves = b.leaves;
                self.times = b.times;
                self.start_ms = b.start_ms;
            }
            self.still = false;
        }
        let mut next: Vec<(usize, f32, f32, f32)> = Vec::new();
        let mut times = Vec::new();
        for &(leaf, to) in &goal {
            let from = (0..self.leaves.len())
                .find(|&i| self.leaves[i].0 == leaf)
                .map_or(0.0, |i| self.weight(i, now_ms));
            let kept = self.times.iter().find(|t| t.0 == leaf && from != 0.0);
            times.extend(kept.copied().or_else(|| fresh(leaf)));
            // The setter snaps a weight within 0.001 of its goal and
            // raises a zero one to 0.001 of it (`cod_lnxded` 0x080bd674).
            let from = if from == 0.0 { 0.001 * to } else { from };
            let ms = if (to - from).abs() < 0.001 {
                0.0
            } else {
                SEAT_RAMP_MS
            };
            next.push((leaf, from, to, ms));
        }
        for i in 0..self.leaves.len() {
            let leaf = self.leaves[i].0;
            let w = self.weight(i, now_ms);
            if w != 0.0 && goal.iter().all(|&(l, _)| l != leaf) {
                // Cleared with no time: it holds until the next step.
                next.push((leaf, w, 0.0, 1.0));
                times.extend(self.times.iter().find(|t| t.0 == leaf).copied());
            }
        }
        self.leaves = next;
        self.times = times;
        self.start_ms = now_ms;
    }

    /// Stills the node last left once `weighted` says its legs node has
    /// faded to 0: the step then snaps each leaf to its goal, frees those
    /// whose goal is 0 and steps no time under it (UO `coduo_lnxded`
    /// 0x80bcc1e, 0x80bca32, 0x80bd884), so a return to it ramps from
    /// those goals and resumes the times where they stopped
    /// (coduo-vehicles.md 14, "The seat blend's ramp").
    pub fn fade_held(&mut self, weighted: impl Fn(usize) -> bool) {
        let last_ms = self.last_ms;
        let Some(h) = self.held.as_mut().filter(|h| !h.still) else {
            return;
        };
        if h.node.is_some_and(&weighted) {
            return;
        }
        // The step that reached 0 stepped no time: they stop at the last
        // frame's.
        for (_, t) in h.times.iter_mut() {
            t.settle(last_ms);
        }
        let goals: Vec<_> = h
            .leaves
            .iter()
            .filter(|l| l.2 != 0.0)
            .map(|&(l, _, to, _)| (l, to, to, 0.0))
            .collect();
        h.times.retain(|t| goals.iter().any(|g| g.0 == t.0));
        h.leaves = goals;
        h.still = true;
    }

    /// Each leaf's node time at `now_ms` where [`Self::set`]'s `motion`
    /// knew it, the node last left's too, as
    /// [`PoseInputs::leaf_times`] takes them.
    pub fn leaf_times(&self, now_ms: i32) -> Vec<(usize, f32)> {
        self.times
            .iter()
            .map(|(l, t)| (*l, t.at(now_ms)))
            .chain(self.held.iter().flat_map(|h| h.leaf_times(now_ms)))
            .collect()
    }

    /// The node and the leaves' weights at `now_ms`, then the node last
    /// left's, as [`PoseInputs::seat_leaves`] takes them; empty for a ramp
    /// never set.
    pub fn at(&self, now_ms: i32) -> Vec<(usize, Vec<(usize, f32)>)> {
        let own = self.node.map(|node| {
            (
                node,
                (0..self.leaves.len())
                    .map(|i| (self.leaves[i].0, self.weight(i, now_ms)))
                    .filter(|&(_, w)| w > 0.0)
                    .collect(),
            )
        });
        own.into_iter()
            .chain(self.held.iter().flat_map(|h| h.at(now_ms)))
            .collect()
    }
}

/// `BG_Player_DoControllers` (`game.mp.i386.so` 0x2b7f8): the local tag on
/// `tag_origin` and the control angles of the spine, each `[pitch, yaw,
/// roll]` in engine degrees. `docs/research/cod11-combat.md` 16.3 and 16.4.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Controllers {
    /// `tag_origin`'s local translation and angles.
    pub origin_trans: Vec3,
    pub origin: [f32; 3],
    pub pelvis: [f32; 3],
    pub back_low: [f32; 3],
    pub back_mid: [f32; 3],
    pub back_up: [f32; 3],
    pub neck: [f32; 3],
    pub head: [f32; 3],
}

impl Controllers {
    /// `None` for a mounted body, which runs none (0x2b804).
    pub fn new(a: &BodyAngles, input: &BodyInput, slope: &BodySlope) -> Option<Self> {
        if input.mounted() {
            return None;
        }
        Some(Self::body(Split::V1_1, a, input, slope))
    }

    /// The controls a patch's hook computes for a body that is not a rider:
    /// 1.1's [`Self::new`]; 1.5's 0x1a389 (`game.mp.i386.so` 1.5b) zero for
    /// a mounted body (`eFlags & 0xc000`), else the newer split; UO's
    /// 0x1e761 (game.mp.uo) the newer split whatever the mount.
    /// docs/research/coduo-vehicles.md 14, "The controllers".
    pub fn target(
        rules: MoveRules,
        a: &BodyAngles,
        input: &BodyInput,
        slope: &BodySlope,
    ) -> Option<Self> {
        match rules {
            MoveRules::V1_1 => Self::new(a, input, slope),
            MoveRules::V1_5 if input.mounted() => Some(Self::default()),
            _ => Some(Self::body(Split::Later, a, input, slope)),
        }
    }

    /// CoD2's (`BG_Player_DoControllers`, Mac 0x182b82, `cod2_lnxded`
    /// 0x80db362): zero for a gunner (`eFlags & 0x300`, here
    /// [`EF_MOUNTED`]), else 1.5's split with CoD2's lean. The caller eases
    /// toward it ([`Self::ease_toward`]), as CoD2 does at 0.36 a ms.
    /// docs/research/cod2-player-anims.md 6.
    pub fn cod2(a: &BodyAngles, input: &BodyInput, slope: &BodySlope) -> Self {
        if input.mounted() {
            return Self::default();
        }
        Self::body(Split::Cod2, a, input, slope)
    }

    /// CoD4's (1.7's `BG_Player_DoControllersInternal`, `cod4_lnxded`
    /// 0x804cf90): CoD2's split and twist with the lean's shift and roll
    /// scales read off `lean`'s dvars, the climbs at CoD4's movetypes.
    /// The caller eases toward it at 0.36 a ms. cod4-player-anims.md 8.
    pub fn cod4(a: &BodyAngles, input: &BodyInput, slope: &BodySlope, lean: &LeanDvars) -> Self {
        if input.mounted() {
            return Self::default();
        }
        Self::body(Split::Cod4(*lean), a, input, slope)
    }

    fn body(split: Split, a: &BodyAngles, input: &BodyInput, slope: &BodySlope) -> Self {
        let prone = input.prone();
        let crouch = input.eflags & EF_CROUCH != 0;
        let mut t = [0.0, a.torso_yaw, 0.0];
        let climb = match split {
            Split::Cod4(_) => MOVETYPE_CLIMB_COD4,
            _ => MOVETYPE_CLIMB,
        };
        if a.movetype & climb == 0 {
            t[0] = a.pitch;
            if prone {
                let p = angle_normalize_180(t[0]);
                t[0] = p * if p > 0.0 { 0.5 } else { 0.25 };
            }
        }
        let h = [0usize, 1, 2].map(|i| angle_subtract(input.view[i], t[i]));
        let l = [0.0, a.legs_yaw, 0.0];
        t = [0usize, 1, 2].map(|i| angle_subtract(t[i], l[i]));
        let legs_off = (!input.dead()).then_some(input.view[1]);
        Self::split(split, t, h, l, legs_off, prone, crouch, slope)
    }

    /// A UO rider's (`eFlags` 0x100000, game.mp.uo 0x1e761): the body's frame
    /// is the seat tag (`r.currentAngles`, [`BodySeat::angles`]), so
    /// `tag_origin` takes no turn; the torso faces the tag, or the view in
    /// the one seat that holds a weapon; the head faces the view, a yaw past
    /// 90 off the torso folded back in front of it.
    pub fn rider(input: &BodyInput, seat: &BodySeat, slope: &BodySlope) -> Self {
        let tag = seat.angles;
        let torso = if seat.weapon { input.view } else { tag };
        let mut head = input.view;
        let mut d = angle_subtract(head[1], torso[1]);
        if d > 90.0 {
            d = 180.0 - d;
        } else if d < -90.0 {
            d = -180.0 - d;
        }
        head[1] = angle_normalize_180(d + torso[1]);
        let h = [0usize, 1, 2].map(|i| angle_subtract(head[i], torso[i]));
        let t = [0usize, 1, 2].map(|i| angle_subtract(torso[i], tag[i]));
        let crouch = input.eflags & EF_CROUCH != 0;
        Self::split(Split::Later, t, h, [0.0; 3], None, false, crouch, slope)
    }

    /// The lean and the spine's split of the torso and head angles, off the
    /// legs' `l`; `legs_off` is the view yaw `tag_origin`'s yaw is taken
    /// off, `None` for a dead body or a rider.
    #[allow(clippy::too_many_arguments)]
    fn split(
        split: Split,
        mut t: [f32; 3],
        mut h: [f32; 3],
        mut l: [f32; 3],
        legs_off: Option<f32>,
        prone: bool,
        crouch: bool,
        slope: &BodySlope,
    ) -> Self {
        let mut trans = Vec3::new(0.0, 0.0, slope.torso_height);
        let f = lean_fraction(slope.lean);
        let roll = f * 50.0 * 0.925;
        match split {
            Split::V1_1 if f == 0.0 => {
                t[2] = 0.0;
                h[2] = 0.0;
            }
            Split::V1_1 => {
                let crouch_scale = if f > 0.0 { 1.5 } else { 1.8 };
                t[2] = if crouch { roll * crouch_scale } else { roll };
                trans.y = -f
                    * if prone {
                        2.5
                    } else if crouch {
                        1.5
                    } else {
                        1.0
                    };
                h[2] = if crouch {
                    roll * crouch_scale
                } else if prone {
                    roll * 0.5
                } else {
                    roll
                };
            }
            // 1.5's and UO's: the roll scaled 0.8 standing and 1.5
            // crouched leaning right, 1.25 leaning left, the head's halved
            // prone; the shift 2.5 a unit of lean whatever the stance.
            // CoD2's: the shift 2.5 a unit of lean right whatever the
            // stance, 5 left standing, 12.5 left crouched; the roll 1.25
            // but crouched leaning right, the head's halved prone.
            Split::Cod2 => {
                t[2] = roll;
                h[2] = roll;
                trans.y = -f
                    * match (crouch, f > 0.0) {
                        (_, true) => 2.5,
                        (false, false) => 5.0,
                        (true, false) => 12.5,
                    };
                if f != 0.0 {
                    if prone {
                        h[2] *= 0.5;
                    } else if !crouch || f <= 0.0 {
                        t[2] *= 1.25;
                        h[2] *= 1.25;
                    }
                }
            }
            // CoD4's: the shift and, standing or crouched, the roll scale
            // off the dvars by side; prone halves the head's roll only.
            Split::Cod4(d) => {
                t[2] = roll;
                h[2] = roll;
                if f != 0.0 {
                    let (shift, rotate) = match (crouch, f > 0.0) {
                        (true, true) => (d.shift_crouch_right, d.rotate_crouch_right),
                        (true, false) => (d.shift_crouch_left, d.rotate_crouch_left),
                        (false, true) => (d.shift_right, d.rotate_right),
                        (false, false) => (d.shift_left, d.rotate_left),
                    };
                    trans.y = -f * shift;
                    if prone {
                        h[2] *= 0.5;
                    } else {
                        t[2] *= rotate;
                        h[2] *= rotate;
                    }
                }
            }
            Split::Later => {
                t[2] = roll;
                h[2] = roll;
                trans.y = -2.5 * f;
                if f != 0.0 {
                    if prone {
                        h[2] *= 0.5;
                    } else {
                        let scale = match (crouch, f > 0.0) {
                            (_, false) => 1.25,
                            (false, true) => 0.8,
                            (true, true) => 1.5,
                        };
                        t[2] *= scale;
                        h[2] *= scale;
                    }
                }
            }
        }
        if let Some(view_yaw) = legs_off {
            l[1] = angle_subtract(l[1], view_yaw);
        }
        if prone {
            l[0] += slope.torso_pitch;
            if split != Split::V1_1 {
                // The prone twist swings `tag_origin` round the elbows:
                // 0x1e761's `sincos(pi * T1 / 180)` arm.
                let (sin, cos) = (std::f32::consts::PI * t[1] / 180.0).sin_cos();
                trans.x += -24.0 * (1.0 - cos);
                trans.y += -12.0 * sin;
                if f * sin > 0.0 {
                    trans.y += 16.0 * (1.0 - cos) * -f;
                }
            }
        } else {
            l[2] += f * 50.0 * 0.075;
        }
        let sloped = slope.torso_pitch != 0.0 || slope.waist_pitch != 0.0;
        let bend = if sloped {
            angle_subtract(slope.torso_pitch, slope.waist_pitch)
        } else {
            0.0
        };
        let (back_low, back_mid, back_up) = if prone {
            (
                [bend, t[2] * -1.2, t[2] * 0.3],
                [0.0, t[1] * 0.1 - t[2] * 0.2, t[2] * 0.2],
                [t[0], t[1] * 0.8 + t[2], t[2] * -0.2],
            )
        } else {
            (
                [t[0] * 0.2 + bend, t[1] * 0.4, t[2] * 0.5],
                [t[0] * 0.3, t[1] * 0.4, t[2] * 0.5],
                [t[0] * 0.5, t[1] * 0.2, t[2] * -0.6],
            )
        };
        let pelvis = if sloped {
            angle_subtract(slope.waist_pitch, slope.torso_pitch)
        } else {
            0.0
        };
        Controllers {
            origin_trans: trans,
            origin: l,
            pelvis: [pelvis, 0.0, 0.0],
            back_low,
            back_mid,
            back_up,
            neck: [h[0] * 0.3, h[1] * 0.3, 0.0],
            head: [h[0] * 0.7, h[1] * 0.7, h[2] * -0.3],
        }
    }

    /// Whether a patch's hook eases its controls toward their targets
    /// ([`Self::ease_toward`]) rather than setting them: 1.5 and UO do,
    /// 1.1 (0x2b7f8) does not.
    pub fn eased(rules: MoveRules) -> bool {
        rules != MoveRules::V1_1
    }

    /// One run of 1.5's and UO's `BG_Player_DoControllers` (game.mp.uo
    /// 0x1f2fe, 1.5b 0x1ad71; cgame uo 0x30005730, 1.5 0x300051c0) over
    /// `frametime_ms`: every control angle, `tag_origin`'s included, steps
    /// toward its target by at most `0.36 * frametime` degrees, each
    /// component on its own and with no wrap (0x1f107), and `tag_origin`'s
    /// translation moves toward its target by at most `0.1 * frametime`
    /// units along the line between them (`BG_LerpOffset` 0x1f1e7).
    pub fn ease_toward(&mut self, target: &Controllers, frametime_ms: i32) {
        let ft = frametime_ms as f32;
        let step = 0.36 * ft;
        let ease = |cur: &mut [f32; 3], goal: &[f32; 3]| {
            for (c, g) in cur.iter_mut().zip(goal) {
                let d = g - *c;
                *c = if step < d {
                    *c + step
                } else if d < -step {
                    *c - step
                } else {
                    *g
                };
            }
        };
        ease(&mut self.back_low, &target.back_low);
        ease(&mut self.back_mid, &target.back_mid);
        ease(&mut self.back_up, &target.back_up);
        ease(&mut self.neck, &target.neck);
        ease(&mut self.head, &target.head);
        ease(&mut self.pelvis, &target.pelvis);
        ease(&mut self.origin, &target.origin);
        let d = target.origin_trans - self.origin_trans;
        let len_sq = d.length_squared();
        if len_sq != 0.0 {
            let s = 0.1 * ft * q_rsqrt(len_sq);
            if s < 1.0 {
                self.origin_trans += d * s;
            } else {
                self.origin_trans = target.origin_trans;
            }
        }
    }
}

/// Which `BG_Player_DoControllers` split a body is posed by: 1.1's
/// (0x2ba74), 1.5's and UO's, which differ in the lean and the prone
/// twist (docs/research/coduo-vehicles.md 14, "The controllers"), or
/// CoD2's, 1.5's with its own lean (cod2-player-anims.md 6), or CoD4's,
/// CoD2's with the lean off its dvars (cod4-player-anims.md 8).
#[derive(Clone, Copy, PartialEq)]
enum Split {
    V1_1,
    Later,
    Cod2,
    Cod4(LeanDvars),
}

/// `Q_rsqrt`: the one-iteration inverse square root `BG_LerpOffset` and
/// the cgame's copy (0x30005670) take the distance with.
fn q_rsqrt(x: f32) -> f32 {
    let y = f32::from_bits(0x5f37_59df - (x.to_bits() >> 1));
    y * (1.5 - 0.5 * x * y * y)
}

/// `YawToQuaternion` ⊗ `PitchToQuaternion` ⊗ `RollToQuaternion`, the order
/// `G_DObjSetControlTagAngles` and `G_DObjSetLocalTag` compose them in
/// (0x6721c, 0x67128): pitch down positive about Y, yaw left positive about
/// Z, roll right-side-down positive about X.
fn control_quat([p, y, r]: [f32; 3]) -> Quat {
    Quat::from_rotation_z(y.to_radians())
        * Quat::from_rotation_y(p.to_radians())
        * Quat::from_rotation_x(r.to_radians())
}

/// Poses [`Controllers`] onto `pose`. Call after the clips and before
/// `skin_matrices`.
///
/// `tag_origin`'s local tag replaces its local rotation and translation. A
/// control bone's rotation replaces its local one and turns it in model
/// space: its world rotation is the control's times its parent's
/// (`docs/research/cod11-combat.md` 16.3). The bones are walked parent
/// first, so each is measured off the bends above it, and `set_local_rot`
/// overwrites, so nothing accumulates across frames. Missing bones are
/// skipped.
pub fn apply_controllers(pose: &mut PoseBuffer, skel: &Skeleton, c: &Controllers) {
    if let Some(bi) = skel.bone_index("tag_origin") {
        let record = Record::tag(c.origin, c.origin_trans, false);
        pose.set_control(bi, c.origin_trans, control_quat(c.origin), record);
    }
    for (name, angles) in [
        ("pelvis", c.pelvis),
        ("back_low", c.back_low),
        ("back_mid", c.back_mid),
        ("back_up", c.back_up),
        ("neck", c.neck),
        ("head", c.head),
    ] {
        let Some(bi) = skel.bone_index(name) else {
            continue;
        };
        let parent = match usize::try_from(skel.bones()[bi].parent) {
            Ok(p) => pose.bone_world(skel, p).1,
            Err(_) => Quat::IDENTITY,
        };
        let rot = parent.inverse() * control_quat(angles) * parent;
        let record = Record::tag(angles, Vec3::ZERO, true);
        pose.set_control(bi, pose.local(bi).0, rot, record);
    }
}

/// True when every child carries an aim annotation (an MG42 aim group, not a
/// container like `main` or `legs`).
fn is_aim_group(tree: &AnimTree, node: usize) -> bool {
    let children = &tree.nodes[node].children;
    !children.is_empty()
        && children.iter().all(|&c| {
            let name = &tree.nodes[c].name;
            name_angle(name, Axis::Pitch).is_some() || name_angle(name, Axis::Yaw).is_some()
        })
}

/// The leaf a UO vehicle seat group (`vehicleJeepDriverIdle`,
/// `vehicleJeepGunnerAim`) plays with the hull's steer and the gun level:
/// `G_PlayerVehiclePositionAndBlend` (game.mp.uo 0x9048f) takes the group's
/// child at `numChildren / 2` and gives that one's middle child the weight
/// `1 - min(|steer| / 45, 1)`, an end child the rest, in the engine's child
/// order (the file's reversed). docs/research/coduo-vehicles.md 11.4.
pub fn seat_group_leaf(tree: &AnimTree, node: usize) -> usize {
    let mut node = node;
    for _ in 0..8 {
        let children = &tree.nodes[node].children;
        if children.is_empty() {
            break;
        }
        node = children[children.len() - 1 - children.len() / 2];
    }
    node
}

/// The clip a wire `legsAnim`/`torsoAnim` value names, descending MG42 aim
/// groups by aim and a vehicle seat group to its level leaf. `None` for out
/// of range or a container node.
pub fn clip_name(anims: &PlayerAnims, wire: i32, pitch_deg: f32, yaw_deg: f32) -> Option<&str> {
    let tree = &anims.tree;
    let node = anims.index.node_id(wire)?;
    let leaf = if tree.nodes[node].children.is_empty() {
        node
    } else if is_aim_group(tree, node) {
        descend_aim(tree, node, pitch_deg, yaw_deg)
    } else if tree.nodes[node].complete {
        seat_group_leaf(tree, node)
    } else {
        return None;
    };
    Some(&tree.nodes[leaf].name)
}

/// What a player's pose is made of: the two wire anim indices, how far into
/// its clip each is, and the controllers the spine layer bends by.
pub struct PoseInputs<'a> {
    pub anims: &'a PlayerAnims,
    /// Wire `legsAnim` / `torsoAnim`, restart toggle included.
    pub legs: i32,
    pub torso: i32,
    /// Each channel's clips with their times and weights, those blending
    /// out first (`crate::animclock::ChannelClock::layers`).
    pub legs_layers: Vec<crate::animclock::Layer>,
    pub torso_layers: Vec<crate::animclock::Layer>,
    /// The `torso` and `legs` nodes' weights over them
    /// (`crate::animclock::ChannelClock::node_weights` on the torso clock).
    pub nodes: crate::animclock::NodeWeights,
    /// Engine degrees, down positive: what an MG42 aim group descends by
    /// when no placement blend is given.
    pub group_pitch: f32,
    /// A gunner's legs: the turret group and its rows and leaves with the
    /// weights the body placement gave them
    /// (`turretpose::GunnerPlacement::rows`), in place of the group's clip.
    pub turret_leaves: Option<(usize, &'a [TurretRow])>,
    /// The turret group a gunner's legs last left, and the rows its
    /// placement last gave it, which it blends out on
    /// (cod11-turrets.md 13.9).
    pub turret_left: Option<(usize, &'a [TurretRow])>,
    /// `None` for a mounted or dead body.
    pub controllers: Option<Controllers>,
    /// A UO rider's [`seat_leaves`]: each node they were set under with its
    /// leaves and their weights, played in place of any layer of either
    /// channel on that node (the torso of a `both` anim shares it). The
    /// current seat's comes first, then the node a seat change left, which
    /// the old legs clip fades out on.
    pub seat_leaves: Vec<(usize, Vec<(usize, f32)>)>,
    /// Under [`Self::node_time`], the node time of each seat leaf that steps
    /// one of its own ([`SeatRamp::leaf_times`]); a leaf not listed samples
    /// at its layer's seconds.
    pub leaf_times: Vec<(usize, f32)>,
    /// Sample each clip at its engine node time ([`crate::animclock::Layer::time`])
    /// rather than its seconds: the 1.5 and UO server's stored pose
    /// (cod11-combat.md 3.1, "The clip's float time").
    pub node_time: bool,
    /// The skeleton's bindings kept from pose to pose; `None` binds each
    /// clip afresh.
    pub binds: Option<&'a crate::skeleton::BindCache>,
}

/// The legs and torso clips as the tree walk blends `main`'s two nodes
/// (`PoseBuffer::blend_nodes`), then the controllers. `pl_*`/`pb_*` clips
/// key the whole body and `pt_*` only the upper body, so a bone the torso
/// clip does not key takes the legs' pose, and one it keys takes the torso's
/// with the legs' at the legs node's 0.01 (player-model-anim-system.md,
/// "Legs/torso split"; cod11-movers.md 15, "Torso over legs").
///
/// `clip` resolves a clip name to the loaded xanim. The server's hit and
/// tag poses go through this, the client's drawn bodies through
/// [`pose_player_drawn`].
pub fn pose_player(
    skel: &Skeleton,
    inputs: &PoseInputs,
    clip: impl FnMut(&str) -> Option<Rc<XAnim>>,
) -> PoseBuffer {
    pose_player_onto(PoseBuffer::new(skel), skel, inputs, clip)
}

/// [`pose_player`] for a body only drawn: the same locals, without the
/// 1.5/UO engine records (`PoseBuffer::drawn`), whose walk is most of a
/// pose's cost.
pub fn pose_player_drawn(
    skel: &Skeleton,
    inputs: &PoseInputs,
    clip: impl FnMut(&str) -> Option<Rc<XAnim>>,
) -> PoseBuffer {
    pose_player_onto(PoseBuffer::drawn(skel), skel, inputs, clip)
}

fn pose_player_onto(
    mut pose: PoseBuffer,
    skel: &Skeleton,
    inputs: &PoseInputs,
    mut clip: impl FnMut(&str) -> Option<Rc<XAnim>>,
) -> PoseBuffer {
    let tree = &inputs.anims.tree;
    // The legs clips, or a gunner's placement rows as nodes of their own
    // under the turret group.
    let mut legs: Vec<(Rc<XAnim>, f32, f32)> = Vec::new();
    let mut shape: Vec<LegsShape> = Vec::new();
    match inputs.turret_leaves {
        Some(current) => {
            // The turret anim's group poses by the rows its placement
            // weighted, and a turret group still blending out by the rows
            // its own placement left (`turret_left`); any other clip, the
            // one the mount blends out of included, poses as itself. Every
            // leaf takes its group's time (UO 0x80bd7e4).
            let layers = &inputs.legs_layers;
            for l in layers {
                let node = inputs.anims.index.node_id(l.index);
                let group = [Some(current), inputs.turret_left]
                    .into_iter()
                    .flatten()
                    .find(|(n, _)| Some(*n) == node)
                    .map(|(_, w)| w);
                let Some(group) = group else {
                    for c in layer_clips(inputs, std::slice::from_ref(l), &mut clip) {
                        shape.push(LegsShape::Clip(legs.len()));
                        legs.push(c);
                    }
                    continue;
                };
                let node_t = l.time.filter(|_| inputs.node_time);
                let mut group_rows = Vec::new();
                for (rw, leaves) in group {
                    let mut ids = Vec::new();
                    for &(leaf, w) in leaves {
                        let Some(anim) = tree.nodes.get(leaf).and_then(|n| clip(&n.name)) else {
                            continue;
                        };
                        let frame = match node_t {
                            Some(nt) => nt * anim.frame_count.saturating_sub(1) as f32,
                            None => anim.frame_pos(l.seconds, anim.looping),
                        };
                        ids.push(legs.len());
                        legs.push((anim, frame, w));
                    }
                    group_rows.push((*rw, ids));
                }
                shape.push(LegsShape::Group(l.weight, group_rows));
            }
        }
        None => {
            for c in layer_clips(inputs, &inputs.legs_layers, &mut clip) {
                shape.push(LegsShape::Clip(legs.len()));
                legs.push(c);
            }
        }
    }
    let torso = layer_clips(inputs, &inputs.torso_layers, &mut clip);
    let bind = |clips: &[(Rc<XAnim>, f32, f32)]| -> Vec<_> {
        clips
            .iter()
            .map(|(a, ..)| match inputs.binds {
                Some(kept) => kept.bind(skel, a),
                None => Rc::new(skel.bind(a)),
            })
            .collect()
    };
    let (legs_bind, torso_bind) = (bind(&legs), bind(&torso));
    let (legs, torso) = (clips_at(&legs, &legs_bind), clips_at(&torso, &torso_bind));
    let legs_node = shape
        .iter()
        .map(|s| match s {
            LegsShape::Clip(i) => Blend::Clip(legs[*i]),
            LegsShape::Group(w, rows) => Blend::Node(
                *w,
                rows.iter()
                    .map(|(rw, ids)| {
                        Blend::Node(*rw, ids.iter().map(|&i| Blend::Clip(legs[i])).collect())
                    })
                    .collect(),
            ),
        })
        .collect();
    let torso_node = torso.iter().map(|&c| Blend::Clip(c)).collect();
    pose.blend_tree(&[
        Blend::Node(inputs.nodes.legs, legs_node),
        Blend::Node(inputs.nodes.torso, torso_node),
    ]);
    if let Some(c) = &inputs.controllers {
        apply_controllers(&mut pose, skel, c);
    }
    pose
}

/// The legs node's children by index into its clips: a clip, or a turret
/// group at its weight over its rows, each a weight over its leaves.
enum LegsShape {
    Clip(usize),
    Group(f32, Vec<(f32, Vec<usize>)>),
}

fn clips_at<'a>(
    clips: &'a [(Rc<XAnim>, f32, f32)],
    binds: &'a [Rc<AnimBinding>],
) -> Vec<crate::skeleton::ClipAt<'a>> {
    clips
        .iter()
        .zip(binds)
        .map(|((a, frame, w), b)| (a.as_ref(), &**b, *frame, *w))
        .collect()
}

/// A channel's layers as clips at their frames and weights.
fn layer_clips(
    inputs: &PoseInputs,
    layers: &[crate::animclock::Layer],
    clip: &mut impl FnMut(&str) -> Option<Rc<XAnim>>,
) -> Vec<(Rc<XAnim>, f32, f32)> {
    let mut out = Vec::new();
    for l in layers {
        let node = inputs.anims.index.node_id(l.index);
        if let Some((_, leaves)) = inputs.seat_leaves.iter().find(|(n, _)| Some(*n) == node) {
            // The walk normalizes a node's weighted children: only the
            // leaves' shares count (cod11-movers.md 15, "Torso over legs").
            let sum: f32 = leaves.iter().map(|l| l.1).sum();
            for &(leaf, w) in leaves {
                let w = if sum > 0.0 { w / sum } else { w };
                if let Some(anim) = clip(&inputs.anims.tree.nodes[leaf].name) {
                    // Each leaf under the seat's node steps a time of its
                    // own, which the channel clock does not keep.
                    let own = inputs.leaf_times.iter().find(|t| t.0 == leaf);
                    let frame = match own.filter(|_| inputs.node_time) {
                        Some(&(_, t)) => t * anim.frame_count.saturating_sub(1) as f32,
                        None => anim.frame_pos(l.seconds, anim.looping),
                    };
                    out.push((anim, frame, l.weight * w));
                }
            }
            continue;
        }
        let Some(anim) =
            clip_name(inputs.anims, l.index, inputs.group_pitch, 0.0).and_then(&mut *clip)
        else {
            continue;
        };
        let frame = layer_frame(inputs, l, &anim);
        out.push((anim, frame, l.weight));
    }
    out
}

/// Where in `anim` a layer samples: off its seconds, or under
/// [`PoseInputs::node_time`] the node time times `numframes`, stored, as
/// the sampler takes it (UO `coduo_lnxded` 0x80b9b72).
fn layer_frame(inputs: &PoseInputs, l: &crate::animclock::Layer, anim: &XAnim) -> f32 {
    match l.time.filter(|_| inputs.node_time) {
        Some(t) => t * anim.frame_count.saturating_sub(1) as f32,
        None => anim.frame_pos(l.seconds, anim.looping),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bone, Model};
    use crate::skeleton::PoseBuffer;

    const MG42_SAMPLE: &str = r#"
main
{
    legs
    {
        standMG42_aim : complete nonloopsync
        {
            standMG42_aim_15down
            {
                pb_standMG42gunner_aim_30left_15down
                pb_standMG42gunner_aim_forward_15down
                pb_standMG42gunner_aim_30right_15down
            }
            standMG42_aim_level
            {
                pb_standMG42gunner_aim_30left_level
                pb_standMG42gunner_aim_forward_level
                pb_standMG42gunner_aim_30right_level
            }
            standMG42_aim_15up
            {
                pb_standMG42gunner_aim_30left_15up
                pb_standMG42gunner_aim_forward_15up
                pb_standMG42gunner_aim_30right_15up
            }
        }
    }
}
"#;

    #[test]
    fn aim_group_descends_by_suffix() {
        let t = AnimTree::parse(MG42_SAMPLE).unwrap();
        let group = t.index_of("standMG42_aim").unwrap();
        let leaf_name = |leaf: usize| t.nodes[leaf].name.as_str();
        // Pitch -4 deg is nearest "level"; yaw offset +25 deg nearest 30right.
        assert_eq!(
            leaf_name(descend_aim(&t, group, -4.0, 25.0)),
            "pb_standMG42gunner_aim_30right_level"
        );
        // Pitch is down-positive (engine view pitch), so looking up picks _15up.
        assert_eq!(
            leaf_name(descend_aim(&t, group, -20.0, -25.0)),
            "pb_standMG42gunner_aim_30left_15up"
        );
        assert_eq!(
            leaf_name(descend_aim(&t, group, 40.0, 0.0)),
            "pb_standMG42gunner_aim_forward_15down"
        );
        // A leaf descends to itself.
        let leaf = t.index_of("pb_standMG42gunner_aim_forward_level").unwrap();
        assert_eq!(descend_aim(&t, leaf, 0.0, 0.0), leaf);
        // Unannotated levels (main > legs) fall back to the middle child and keep descending.
        let main = t.index_of("main").unwrap();
        assert_eq!(
            leaf_name(descend_aim(&t, main, 0.0, 0.0)),
            "pb_standMG42gunner_aim_forward_level"
        );
    }

    /// Each push composes against the bones before it, so `world` holds the
    /// full ancestor chain.
    fn bone(name: &str, parent: i32, local_pos: Vec3, local_rot: Quat, world: &[Bone]) -> Bone {
        let (pos, rot) = if parent >= 0 {
            let p = &world[parent as usize];
            (p.pos + p.rot * local_pos, p.rot * local_rot)
        } else {
            (local_pos, local_rot)
        };
        Bone {
            name: name.into(),
            parent,
            pos,
            rot,
            local_pos,
            local_rot,
            hit_mins: Vec3::ZERO,
            hit_maxs: Vec3::ZERO,
            hit_location: 0,
        }
    }

    /// tag_origin > pelvis > back_low > back_mid > back_up on +Z with identity
    /// binds, so local Y is the lateral axis.
    fn spine_fixture() -> Model {
        let mut bones = vec![bone("tag_origin", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        for (name, parent) in [
            ("pelvis", 0i32),
            ("back_low", 1),
            ("back_mid", 2),
            ("back_up", 3),
        ] {
            let b = bone(name, parent, Vec3::Z, Quat::IDENTITY, &bones);
            bones.push(b);
        }
        Model::rig("spine", bones)
    }

    /// Bind rotations are identity, so the skin matrix's rotation is the posed
    /// world rotation.
    fn world_rot_of(pose: &PoseBuffer, skel: &Skeleton, bone: usize) -> Quat {
        Quat::from_mat4(&pose.skin_matrices(skel, 0)[bone])
    }

    fn near(x: f32, y: f32) -> bool {
        (x - y).abs() < 0.01
    }

    fn idle() -> crate::animscript::AnimRecord {
        crate::animscript::AnimRecord {
            movetypes: 1 << 1,
            ..Default::default()
        }
    }

    /// A standing idle body: the record reads `idle`, the view as given.
    fn standing(view: [f32; 3]) -> (BodyAngles, BodyInput) {
        let input = BodyInput {
            view,
            legs: idle(),
            ..Default::default()
        };
        let mut a = BodyAngles::default();
        a.update_conditions(&input);
        (a, input)
    }

    #[test]
    fn controllers_bend_the_spine_in_model_space() {
        let m = spine_fixture();
        let skel = Skeleton::build(&[&m]);
        let mut pose = PoseBuffer::new(&skel);
        // A settled swing: the torso takes 0.6 of a 50-degree view.
        let a = BodyAngles {
            pitch: 30.0,
            ..Default::default()
        };
        let input = BodyInput {
            view: [50.0, 0.0, 0.0],
            ..Default::default()
        };
        let c = Controllers::new(&a, &input, &BodySlope::default()).unwrap();
        apply_controllers(&mut pose, &skel, &c);
        // back_up accumulates all three weights, the full torso pitch, and
        // positive pitch tips the spine's top toward +X, nose down.
        let up = skel.bone_index("back_up").unwrap();
        let v = world_rot_of(&pose, &skel, up) * Vec3::Z;
        assert!(near(v.angle_between(Vec3::Z).to_degrees(), 30.0), "{v}");
        assert!(v.x > 0.0, "{v}");
        // Again: `set_local_rot` overwrites, so nothing accumulates.
        let first = world_rot_of(&pose, &skel, up);
        apply_controllers(&mut pose, &skel, &c);
        assert!(first.abs_diff_eq(world_rot_of(&pose, &skel, up), 1e-5));
    }

    /// `tag_origin`'s local tag turns the whole body by the legs' yaw off
    /// the view and shifts it by the lean; the spine's yaw controllers turn
    /// it back by the torso's, so `back_up` ends at the torso's yaw.
    #[test]
    fn the_local_tag_turns_the_body_to_the_legs_and_the_spine_back() {
        let m = spine_fixture();
        let skel = Skeleton::build(&[&m]);
        let mut pose = PoseBuffer::new(&skel);
        let a = BodyAngles {
            legs_yaw: 30.0,
            torso_yaw: 10.0,
            ..Default::default()
        };
        let input = BodyInput::default();
        let c = Controllers::new(&a, &input, &BodySlope::default()).unwrap();
        apply_controllers(&mut pose, &skel, &c);
        let yaw_of = |name: &str| {
            let f = world_rot_of(&pose, &skel, skel.bone_index(name).unwrap()) * Vec3::X;
            f.y.atan2(f.x).to_degrees()
        };
        assert!(near(yaw_of("tag_origin"), 30.0), "{}", yaw_of("tag_origin"));
        assert!(near(yaw_of("back_low"), 30.0 - 8.0));
        assert!(near(yaw_of("back_mid"), 30.0 - 16.0));
        assert!(near(yaw_of("back_up"), 10.0), "{}", yaw_of("back_up"));
    }

    /// `BG_SwingAngles` on the pitch channel at the server's 50 ms frame:
    /// the first step covers `51 * 0.05 * 50 * 0.15` of a flip to 85, and
    /// the swing lands on `0.6 * 85` seven frames later (combat doc 16.3).
    #[test]
    fn the_torso_pitch_eases_after_the_view() {
        let (mut s, input) = standing([85.0, 0.0, 0.0]);
        s.step(&input, 50, BG_SWING_SPEED);
        assert!(near(s.pitch, 19.125), "{s:?}");
        assert!(s.pitching);
        let mut frames = 1;
        while s.pitching {
            s.step(&input, 50, BG_SWING_SPEED);
            frames += 1;
        }
        assert_eq!(frames, 7);
        assert!(near(s.pitch, 51.0), "{s:?}");
        // Back up: the angle folds through 360 on the way to 0.
        s.step(
            &BodyInput {
                view: [0.0; 3],
                ..input
            },
            50,
            BG_SWING_SPEED,
        );
        assert!(near(s.pitch, 31.875), "{s:?}");
        // A dead body heads for level whatever its view.
        let mut r = BodyAngles::default();
        let dead = BodyInput {
            view: [-60.0, 0.0, 0.0],
            eflags: EF_DEAD,
            ..Default::default()
        };
        r.step(&dead, 50, BG_SWING_SPEED);
        assert_eq!(r.pitch, 0.0);
    }

    /// A short frame cannot leave the torso more than 45 degrees off its
    /// destination: the clamp puts it 44 inside.
    #[test]
    fn the_torso_pitch_is_clamped_to_45_off_the_view() {
        let (mut s, input) = standing([85.0, 0.0, 0.0]);
        s.step(&input, 1, BG_SWING_SPEED);
        assert!(near(s.pitch, 7.0), "{s:?}");
        let (mut up, input) = standing([275.0, 0.0, 0.0]); // -85 as 0..360
        up.step(&input, 1, BG_SWING_SPEED);
        assert!(near(up.pitch, 360.0 - 7.0), "{up:?}");
    }

    /// The torso yaw always swings, at `bg_swingSpeed` 0.2: a 50 ms step is
    /// `max(|d| * 0.5, 5)`. The idle legs hold until the view is 40 off them.
    #[test]
    fn the_torso_turns_with_the_view_and_the_idle_legs_wait_for_40() {
        let (mut s, input) = standing([0.0, 30.0, 0.0]);
        s.step(&input, 50, BG_SWING_SPEED);
        assert!(near(s.torso_yaw, 15.0), "{s:?}");
        assert_eq!(s.legs_yaw, 0.0, "30 off is inside the legs' 40");
        s.step(&input, 50, BG_SWING_SPEED);
        assert!(near(s.torso_yaw, 22.5), "{s:?}");

        let (mut s, input) = standing([0.0, 50.0, 0.0]);
        s.step(&input, 50, BG_SWING_SPEED);
        assert!(near(s.legs_yaw, 25.0), "{s:?}");
        assert!(s.legs_yawing);
        // Once swinging they keep going until they land.
        s.step(
            &BodyInput {
                view: [0.0, 30.0, 0.0],
                ..input
            },
            50,
            BG_SWING_SPEED,
        );
        assert!(near(s.legs_yaw, 30.0), "{s:?}");
        assert!(!s.legs_yawing);
    }

    /// A moving body's legs always follow, toward the view plus
    /// `movementDir`, and its torso toward the view plus 0.3 of it; a strafe
    /// anim's legs face the view; prone legs snap to their heading; a
    /// mounted body snaps everything to the view.
    #[test]
    fn the_legs_follow_their_heading_by_movetype() {
        let run = BodyInput {
            view: [0.0, 0.0, 0.0],
            movement_dir: 20.0,
            legs: crate::animscript::AnimRecord {
                movetypes: 1 << 10,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut s = BodyAngles::default();
        s.update_conditions(&run);
        s.step(&run, 50, BG_SWING_SPEED);
        assert!(near(s.legs_yaw, 10.0), "{s:?}");
        assert!(near(s.torso_yaw, 5.0), "{s:?}");

        let strafe = BodyInput {
            movement_dir: 90.0,
            legs: crate::animscript::AnimRecord {
                movetypes: 1 << 10,
                strafe: true,
                ..Default::default()
            },
            ..run
        };
        let mut s = BodyAngles::default();
        s.update_conditions(&strafe);
        s.step(&strafe, 50, BG_SWING_SPEED);
        assert_eq!(s.legs_yaw, 0.0);

        let prone = BodyInput {
            eflags: EF_PRONE,
            movement_dir: 60.0,
            ..run
        };
        let mut s = BodyAngles::default();
        s.step(&prone, 50, BG_SWING_SPEED);
        assert_eq!(s.legs_yaw, 60.0);

        let mounted = BodyInput {
            view: [30.0, 200.0, 0.0],
            eflags: EF_MOUNTED,
            ..run
        };
        let mut s = BodyAngles::default();
        s.step(&mounted, 50, BG_SWING_SPEED);
        assert!(near(s.legs_yaw, 200.0) && near(s.torso_yaw, 200.0));
        assert_eq!(s.pitch, 0.0);
        assert!(Controllers::new(&s, &mounted, &BodySlope::default()).is_none());
    }

    /// UO's updater swings all three angles for a rider unless his seat
    /// allows his weapon, and an armed one's legs toward the seat's yaw:
    /// only the legs' flag shows it, on the frame he gets out.
    #[test]
    fn a_uo_riders_seat_decides_whether_his_legs_swing_out() {
        use crate::net::flags::EF_IN_VEHICLE_V22;
        let legs_after_exit = |seat_weapon| {
            let mut s = BodyAngles {
                legs_yaw: 100.0,
                torso_yaw: 100.0,
                movetype: MOVETYPE_IDLE,
                ..Default::default()
            };
            let seated = BodyInput {
                view: [0.0, 100.0, 0.0],
                movement_dir: 30.0,
                eflags: EF_IN_VEHICLE_V22,
                seat_weapon,
                ..Default::default()
            };
            s.step(&seated, 50, BG_SWING_SPEED);
            assert!(near(s.legs_yaw, 100.0) && near(s.torso_yaw, 100.0));
            let out = BodyInput {
                eflags: 0,
                seat_weapon: SeatWeapon::Barred,
                ..seated
            };
            s.step(&out, 50, BG_SWING_SPEED);
            s.legs_yaw
        };
        // Forced, the flag outlives the seat: 15 of the 30 off the view.
        assert!(near(legs_after_exit(SeatWeapon::Barred), 115.0));
        // Inside the 40 degree tolerance the legs never start.
        assert!(near(legs_after_exit(SeatWeapon::Allowed), 100.0));
        let held = SeatWeapon::of(1, 3, 5, 190.0);
        assert_eq!(held, SeatWeapon::Held { seat_yaw: 190.0 });
        assert!(near(legs_after_exit(held), 115.0));
        assert_eq!(SeatWeapon::of(1, 2, 5, 0.0), SeatWeapon::Barred);
    }

    /// The split: the back takes the torso pitch 0.2 / 0.3 / 0.5 and its yaw
    /// off the legs 0.4 / 0.4 / 0.2, the neck and head 0.3 / 0.7 of the view
    /// past the torso, and `tag_origin` the legs' yaw off the view, so the
    /// yaws sum to nothing and the head faces the view.
    #[test]
    fn the_controllers_split_pitch_and_yaw_between_back_and_head() {
        let a = BodyAngles {
            pitch: 30.0,
            torso_yaw: 20.0,
            legs_yaw: 350.0,
            ..Default::default()
        };
        let input = BodyInput {
            view: [50.0, 40.0, 0.0],
            ..Default::default()
        };
        let c = Controllers::new(&a, &input, &BodySlope::default()).unwrap();
        assert!(near(c.back_low[0], 6.0) && near(c.back_mid[0], 9.0) && near(c.back_up[0], 15.0));
        assert!(near(c.neck[0], 6.0) && near(c.head[0], 14.0), "{c:?}");
        assert!(near(c.back_low[1], 12.0) && near(c.back_mid[1], 12.0));
        assert!(near(c.back_up[1], 6.0) && near(c.neck[1], 6.0) && near(c.head[1], 14.0));
        assert!(near(c.origin[1], -50.0), "{c:?}");
        let sum =
            c.origin[1] + c.back_low[1] + c.back_mid[1] + c.back_up[1] + c.neck[1] + c.head[1];
        assert!(near(sum, 0.0), "{sum}");
        assert_eq!(c.pelvis, [0.0; 3]);
        assert_eq!(c.origin_trans, Vec3::ZERO);
    }

    /// The lean through `GetLeanFraction`: 46.25 degrees of roll at full
    /// lean, split 0.5 / 0.5 / -0.6 down the back with the head countering
    /// 0.3, 3.75 on `tag_origin` and a one-unit shift; crouched it is 1.5
    /// times (1.8 to the left) and the shift 1.5, prone the shift 2.5.
    #[test]
    fn the_lean_rolls_the_back_and_shifts_the_body() {
        let a = BodyAngles::default();
        let slope = BodySlope {
            lean: 0.5,
            ..Default::default()
        };
        let f = 0.75; // (2 - 0.5) * 0.5
        let roll = f * 46.25;
        let stand = Controllers::new(&a, &BodyInput::default(), &slope).unwrap();
        assert!(near(stand.back_low[2], roll * 0.5) && near(stand.back_mid[2], roll * 0.5));
        assert!(near(stand.back_up[2], roll * -0.6) && near(stand.head[2], roll * -0.3));
        assert!(near(stand.origin[2], f * 3.75) && near(stand.origin_trans.y, -f));

        let crouch = BodyInput {
            eflags: EF_CROUCH,
            ..Default::default()
        };
        let c = Controllers::new(&a, &crouch, &slope).unwrap();
        assert!(near(c.back_low[2], roll * 1.5 * 0.5) && near(c.origin_trans.y, -f * 1.5));
        let left = BodySlope {
            lean: -0.5,
            ..Default::default()
        };
        let c = Controllers::new(&a, &crouch, &left).unwrap();
        assert!(near(c.head[2], roll * 1.8 * 0.3), "{c:?}");

        let prone = BodyInput {
            eflags: EF_PRONE,
            ..Default::default()
        };
        let c = Controllers::new(&a, &prone, &slope).unwrap();
        assert!(near(c.origin_trans.y, -f * 2.5) && near(c.origin[2], 0.0));
        assert!(near(c.back_low[1], roll * -1.2) && near(c.back_up[1], roll));
        assert!(near(c.head[2], roll * 0.5 * -0.3));
    }

    /// 1.5's and UO's split (game.mp.uo 0x1e761, 1.5b 0x1a389): the roll
    /// 0.8 times standing and 1.5 crouched to the right, 1.25 to the
    /// left, the shift 2.5 in every stance, and prone the torso's twist
    /// swings `tag_origin` round by `(-24 (1 - cos), -12 sin)`.
    #[test]
    fn the_later_split_scales_the_lean_and_swings_the_prone_twist() {
        let a = BodyAngles::default();
        let slope = BodySlope {
            lean: 0.5,
            ..Default::default()
        };
        let f = 0.75;
        let roll = f * 46.25;
        let stand = BodyInput::default();
        let c = Controllers::target(MoveRules::Uo, &a, &stand, &slope).unwrap();
        assert!(near(c.back_low[2], roll * 0.8 * 0.5) && near(c.head[2], roll * 0.8 * -0.3));
        assert!(near(c.origin_trans.y, -2.5 * f) && near(c.origin[2], f * 3.75));
        let left = BodySlope {
            lean: -0.5,
            ..Default::default()
        };
        let c = Controllers::target(MoveRules::V1_5, &a, &stand, &left).unwrap();
        assert!(near(c.back_low[2], -roll * 1.25 * 0.5), "{c:?}");
        let crouch = BodyInput {
            eflags: EF_CROUCH,
            ..Default::default()
        };
        let c = Controllers::target(MoveRules::Uo, &a, &crouch, &slope).unwrap();
        assert!(near(c.back_low[2], roll * 1.5 * 0.5) && near(c.origin_trans.y, -2.5 * f));

        let twisted = BodyAngles {
            torso_yaw: 30.0,
            ..Default::default()
        };
        let prone = BodyInput {
            eflags: EF_PRONE,
            ..Default::default()
        };
        let c =
            Controllers::target(MoveRules::Uo, &twisted, &prone, &BodySlope::default()).unwrap();
        let (sin, cos) = 30f32.to_radians().sin_cos();
        assert!(near(c.origin_trans.x, -24.0 * (1.0 - cos)), "{c:?}");
        assert!(near(c.origin_trans.y, -12.0 * sin), "{c:?}");
        // 1.1's prone body takes no such swing.
        let c =
            Controllers::target(MoveRules::V1_1, &twisted, &prone, &BodySlope::default()).unwrap();
        assert!(near(c.origin_trans.x, 0.0) && near(c.origin_trans.y, 0.0));
    }

    /// A mounted body: 1.1 runs no controls, 1.5 aims them all at zero,
    /// UO computes them as on foot.
    #[test]
    fn each_patch_treats_a_gunner_its_own_way() {
        let a = BodyAngles {
            pitch: 10.0,
            ..Default::default()
        };
        let gunner = BodyInput {
            view: [20.0, 0.0, 0.0],
            eflags: crate::net::flags::EF_MOUNTED_STAND,
            ..Default::default()
        };
        let s = BodySlope::default();
        assert!(Controllers::target(MoveRules::V1_1, &a, &gunner, &s).is_none());
        assert_eq!(
            Controllers::target(MoveRules::V1_5, &a, &gunner, &s),
            Some(Controllers::default())
        );
        let c = Controllers::target(MoveRules::Uo, &a, &gunner, &s).unwrap();
        assert!(near(c.neck[0], 10.0 * 0.3), "{c:?}");
    }

    /// 1.5's and UO's hook steps every control angle 0.36 degrees a ms
    /// toward its target, component by component and without a wrap, and
    /// `tag_origin`'s translation 0.1 units a ms along the line.
    #[test]
    fn the_controls_ease_at_the_hooks_rates() {
        let target = Controllers {
            back_low: [0.0, 40.0, -5.0],
            origin: [0.0, -170.0, 0.0],
            origin_trans: Vec3::new(0.0, -3.0, 4.0),
            ..Default::default()
        };
        let mut c = Controllers {
            origin: [0.0, 170.0, 0.0],
            ..Default::default()
        };
        c.ease_toward(&target, 50);
        assert!(
            near(c.back_low[1], 18.0) && near(c.back_low[2], -5.0),
            "{c:?}"
        );
        assert!(
            near(c.origin[1], 152.0),
            "the yaw walks the long way: {c:?}"
        );
        assert!((c.origin_trans.length() - 5.0).abs() < 0.01, "{c:?}");
        assert!((c.origin_trans - target.origin_trans).length() < 0.01);
        c.ease_toward(&target, 50);
        c.ease_toward(&target, 50);
        assert_eq!(c.back_low, target.back_low);
        let mut half = Controllers::default();
        half.ease_toward(&target, 10);
        assert!((half.origin_trans.length() - 1.0).abs() < 0.01, "{half:?}");
    }

    /// The server's seat blend: every goal runs there in a straight line
    /// over 100 ms from where the frame found it, a zero weight (a new
    /// node's too) from a thousandth of its goal; a leaf no longer set drops
    /// to 0 at the next step. The node left keeps its ramp; once its legs
    /// node fades out its leaves sit on their goals, and a return to it
    /// ramps from there.
    #[test]
    fn the_seat_blend_ramps_over_100_ms() {
        let w = |r: &SeatRamp, t: i32, i: usize, leaf: usize| {
            r.at(t)[i]
                .1
                .iter()
                .find(|&&(l, _)| l == leaf)
                .map_or(0.0, |&(_, w)| w)
        };
        let close = |x: f32, y: f32| (x - y).abs() < 1e-4;
        let mut r = SeatRamp::default();
        r.set(Some((7, vec![(1, 1.0)])), 1000, |_| None);
        assert!(close(w(&r, 1000, 0, 1), 0.001) && close(w(&r, 1050, 0, 1), 0.5005));
        r.set(Some((7, vec![(1, 0.6), (2, 0.4)])), 1050, |_| None);
        assert!(close(w(&r, 1050, 0, 2), 0.0004));
        assert!(close(w(&r, 1100, 0, 1), 0.55025) && close(w(&r, 1100, 0, 2), 0.2002));
        assert!(close(w(&r, 1150, 0, 1), 0.6) && close(w(&r, 1150, 0, 2), 0.4));
        r.set(Some((7, vec![(1, 0.6), (3, 0.4)])), 1100, |_| None);
        assert!(close(w(&r, 1100, 0, 2), 0.2002), "held until the step");
        assert_eq!(w(&r, 1150, 0, 2), 0.0);
        r.set(Some((9, vec![(4, 1.0)])), 1150, |_| None);
        assert_eq!(r.at(1150)[0], (9, vec![(4, 0.001)]));
        assert_eq!(r.at(1150)[1].0, 7, "the node left keeps its leaves");
        assert!(close(w(&r, 1150, 1, 1), 0.575125) && close(w(&r, 1150, 1, 3), 0.2002));
        // Node 7's legs weight reaches 0: its leaves land on their goals.
        r.fade_held(|n| n == 9);
        assert_eq!(r.at(1200)[1], (7, vec![(1, 0.6), (3, 0.4)]));
        r.set(Some((7, vec![(1, 1.0)])), 1200, |_| None);
        assert_eq!(r.at(1200)[1].0, 9);
        assert!(close(w(&r, 1250, 0, 1), 0.8), "back on it, from its goals");
        assert!(close(w(&r, 1200, 0, 3), 0.4) && w(&r, 1250, 0, 3) == 0.0);
    }

    /// The node left keeps stepping its leaves' times until its legs node
    /// fades out; from then they stand still, and a return resumes them.
    #[test]
    fn a_faded_seat_node_s_leaf_times_stand_still() {
        let motion = |_| Some((1.0, true));
        let time = |r: &SeatRamp, t: i32| r.leaf_times(t).iter().find(|l| l.0 == 1).map(|l| l.1);
        let mut r = SeatRamp::default();
        r.set(Some((7, vec![(1, 1.0)])), 1000, motion);
        r.set(Some((7, vec![(1, 1.0)])), 1050, motion);
        r.set(Some((9, vec![(4, 1.0)])), 1100, motion);
        assert!(near(time(&r, 1100).unwrap(), 0.1));
        // The step at 1150 lands node 7's weight on 0 and steps no time.
        r.fade_held(|n| n == 9);
        r.set(Some((9, vec![(4, 1.0)])), 1150, motion);
        for t in [1200, 1250, 1300] {
            r.fade_held(|n| n == 9);
            r.set(Some((9, vec![(4, 1.0)])), t, motion);
            assert!(near(time(&r, t).unwrap(), 0.1), "{t}");
        }
        r.set(Some((7, vec![(1, 1.0)])), 1350, motion);
        assert!(near(time(&r, 1350).unwrap(), 0.1));
        assert!(near(time(&r, 1400).unwrap(), 0.15));
    }

    /// A seat leaf's node time starts at 0 on the frame it is given weight
    /// from none and steps every frame after, through a reweight; one set
    /// again after its weight reached 0 starts over, and a leaf with no
    /// known clip keeps none.
    #[test]
    fn a_seat_leaf_steps_its_own_time_from_its_allocation() {
        let motion = |leaf: usize| match leaf {
            1 => Some((1.0, true)),
            2 => Some((30.0, false)),
            _ => None,
        };
        let time = |r: &SeatRamp, t: i32, leaf: usize| {
            r.leaf_times(t).iter().find(|l| l.0 == leaf).map(|l| l.1)
        };
        let mut r = SeatRamp::default();
        r.set(Some((7, vec![(1, 1.0), (3, 0.0)])), 1000, motion);
        assert_eq!(time(&r, 1000, 1), Some(0.0));
        assert_eq!(time(&r, 1049, 1), Some(0.0));
        assert_eq!(time(&r, 1050, 1), Some(0.05));
        assert_eq!(time(&r, 1000, 3), None);
        // Reweighted with leaf 2 joining: 1 keeps counting, 2 starts at 0.
        r.set(Some((7, vec![(1, 0.5), (2, 0.5)])), 1100, motion);
        assert_eq!(time(&r, 1100, 1), Some(0.1));
        assert_eq!(time(&r, 1100, 2), Some(0.0));
        // Two frames at 1.5 a frame clamp at the clip's end.
        assert_eq!(time(&r, 1200, 2), Some(1.0));
        // Leaf 2 dropped: it reads 0 at the next step, so set again it is
        // a fresh allocation; leaf 1, never at 0, runs on.
        r.set(Some((7, vec![(1, 1.0)])), 1200, motion);
        r.set(Some((7, vec![(1, 0.5), (2, 0.5)])), 1250, motion);
        assert_eq!(time(&r, 1250, 2), Some(0.0));
        let mut t = 0.0f32;
        for _ in 0..5 {
            t = (f64::from(t) + f64::from(0.05f32)) as f32;
        }
        assert_eq!(time(&r, 1250, 1), Some(t));
    }

    /// Prone halves (down) or quarters (up) the torso pitch onto `back_up`
    /// alone; the slope bends `back_low` and `pelvis` against each other
    /// and tips `tag_origin` by the torso's pitch, raised by its height.
    #[test]
    fn prone_puts_the_pitch_on_back_up_and_the_slope_on_the_pelvis() {
        let a = BodyAngles {
            pitch: 30.0,
            ..Default::default()
        };
        let prone = BodyInput {
            view: [50.0, 0.0, 0.0],
            eflags: EF_PRONE,
            ..Default::default()
        };
        let slope = BodySlope {
            torso_height: 3.0,
            torso_pitch: 4.0,
            waist_pitch: 1.0,
            ..Default::default()
        };
        let c = Controllers::new(&a, &prone, &slope).unwrap();
        assert!(near(c.back_up[0], 15.0) && near(c.back_mid[0], 0.0));
        assert!(near(c.back_low[0], 3.0) && near(c.pelvis[0], -3.0));
        assert!(near(c.head[0], 35.0 * 0.7), "{c:?}");
        assert!(near(c.origin[0], 4.0) && near(c.origin_trans.z, 3.0));
        let up = BodyAngles {
            pitch: 340.0,
            ..Default::default()
        };
        let c = Controllers::new(&up, &prone, &BodySlope::default()).unwrap();
        assert!(near(c.back_up[0], -5.0));
    }

    /// UO's jeep seat groups (`animtrees/multiplayer.atr`): `complete`
    /// groups whose leaves carry no aim annotation.
    const JEEP_SAMPLE: &str = r#"
main
{
    legs
    {
        vehicleJeepDriverIdle : complete
        {
            vehicleJeepDriverIdle_aim
            {
                c_mp_jeep_driver_turnL
                c_mp_jeep_driver_idle
                c_mp_jeep_driver_turnR
            }
        }
        vehicleJeepGunnerAim : complete
        {
            vehicleJeepGunnerAim_level
            {
                c_mp_jeep_gunner_idle
            }
        }
        c_mp_jeep_passenger_idle
    }
}
"#;

    /// A parked jeep's driver and gunner play their groups' level leaves,
    /// the ones `G_PlayerVehiclePositionAndBlend` weights 1 at no steer,
    /// not the bind pose a group without a clip left the body in.
    #[test]
    fn a_seat_group_plays_its_level_leaf() {
        let t = AnimTree::parse(JEEP_SAMPLE).unwrap();
        let leaf = |group: &str| {
            let n = t.index_of(group).unwrap();
            assert!(t.nodes[n].complete);
            t.nodes[seat_group_leaf(&t, n)].name.clone()
        };
        // The driver's `_aim` level reads as no aim annotation; the
        // gunner's `_level` does, and descends as an MG42 row would.
        let driver = t.index_of("vehicleJeepDriverIdle").unwrap();
        assert!(!is_aim_group(&t, driver));
        assert_eq!(leaf("vehicleJeepDriverIdle"), "c_mp_jeep_driver_idle");
        assert_eq!(leaf("vehicleJeepGunnerAim"), "c_mp_jeep_gunner_idle");
        let legs = t.index_of("legs").unwrap();
        assert!(!t.nodes[legs].complete);
    }

    /// `G_PlayerVehiclePositionAndBlend`'s weights on UO's stock tree: the
    /// jeep driver blends toward `turnL` for a left (positive) steer and
    /// `turnR` for a right, all the way at 45; the jeep gunner's and the
    /// tank gunner's groups play their middle leaf whatever the aim; the
    /// passenger's plain clip and a tank driver get none.
    #[test]
    fn a_rider_plays_the_seat_blend_of_his_seat() {
        let Some(fs) = crate::testing::uo_fs() else {
            return;
        };
        let anims = PlayerAnims::load(&fs).expect("the player anims");
        let wire = |name: &str| anims.index.wire_of(&anims.tree, name).unwrap();
        let named = |leaves: Option<Vec<(usize, f32)>>| -> Vec<(String, f32)> {
            leaves
                .unwrap_or_default()
                .into_iter()
                .map(|(n, w)| (anims.tree.nodes[n].name.to_lowercase(), w))
                .collect()
        };
        let seat = |kind, pos, steer| BodySeat::new([0.0; 3], kind, pos, steer);
        let driver = wire("vehicleJeepDriverIdle");
        assert_eq!(
            named(seat_leaves(&anims, driver, &seat(1, 1, 0.0))),
            [("c_mp_jeep_driver_idle".to_string(), 1.0)]
        );
        assert_eq!(
            named(seat_leaves(&anims, driver, &seat(1, 1, 22.5))),
            [
                ("c_mp_jeep_driver_idle".to_string(), 0.5),
                ("c_mp_jeep_driver_turnl".to_string(), 0.5)
            ]
        );
        // The cgame draws the same steer shallower: half way at 30.
        assert_eq!(
            named(seat_leaves(&anims, driver, &seat(1, 1, 30.0).drawn())),
            [
                ("c_mp_jeep_driver_idle".to_string(), 0.5),
                ("c_mp_jeep_driver_turnl".to_string(), 0.5)
            ]
        );
        assert_eq!(
            named(seat_leaves(&anims, driver, &seat(1, 1, -90.0))),
            [
                ("c_mp_jeep_driver_idle".to_string(), 0.0),
                ("c_mp_jeep_driver_turnr".to_string(), 1.0)
            ]
        );
        assert_eq!(
            named(seat_leaves(
                &anims,
                wire("vehicleJeepGunnerAim"),
                &seat(1, 2, 30.0)
            )),
            [("c_mp_jeep_gunner_idle".to_string(), 1.0)]
        );
        let mg = wire("standMG42_aim");
        assert_eq!(
            named(seat_leaves(&anims, mg, &seat(2, 2, 0.0))),
            [("pb_standmg42gunner_aim_forward_level".to_string(), 1.0)]
        );
        assert!(seat_leaves(&anims, mg, &seat(2, 1, 0.0)).is_none());
        let passenger = wire("c_mp_jeep_passenger_idle");
        assert!(seat_leaves(&anims, passenger, &seat(1, 3, 10.0)).is_none());
    }

    /// A rider's body is framed by the seat tag: facing it, no control
    /// turns; the driver's torso stays on the tag while the head turns to
    /// the view, a look behind folded in front; the four-wheel's seat 3
    /// turns the torso with the view.
    #[test]
    fn a_rider_turns_the_head_off_the_seat_tag() {
        let tag = [0.0, 75.0, -0.85];
        let still = BodyInput {
            view: tag,
            ..Default::default()
        };
        let driver = BodySeat::new(tag, 1, 1, 0.0);
        let c = Controllers::rider(&still, &driver, &BodySlope::default());
        assert_eq!(c.origin, [0.0; 3]);
        for a in [c.back_low, c.back_mid, c.back_up, c.neck, c.head] {
            assert!(a.iter().all(|x| near(*x, 0.0)), "{c:?}");
        }
        let look = BodyInput {
            view: [10.0, 75.0 + 40.0, 0.0],
            ..Default::default()
        };
        let c = Controllers::rider(&look, &driver, &BodySlope::default());
        assert!(
            near(c.back_up[1], 0.0) && near(c.head[1], 40.0 * 0.7),
            "{c:?}"
        );
        assert!(near(c.neck[0], 10.0 * 0.3), "{c:?}");
        let behind = BodyInput {
            view: [0.0, 75.0 + 150.0, 0.0],
            ..Default::default()
        };
        let c = Controllers::rider(&behind, &driver, &BodySlope::default());
        assert!(near(c.head[1], 30.0 * 0.7), "{c:?}");
        let passenger = BodySeat::new(tag, 1, 3, 0.0);
        assert!(passenger.weapon && !BodySeat::new(tag, 2, 3, 0.0).weapon);
        assert!(passenger.armed(5).weapon && !passenger.armed(0).weapon);
        let c = Controllers::rider(&look, &passenger, &BodySlope::default());
        assert!(
            near(c.back_up[1], 40.0 * 0.2) && near(c.head[1], 0.0),
            "{c:?}"
        );
    }

    /// CoD2's body: the six controls keep 1.1's names, set between the
    /// `j_*` joints they bend (`playerbody_german_africa01`'s order,
    /// cod2-player-anims.md 2), so each turns every joint above it.
    fn cod2_spine() -> Model {
        let mut bones = vec![bone("tag_origin", -1, Vec3::ZERO, Quat::IDENTITY, &[])];
        for name in [
            "j_mainroot",
            "pelvis",
            "torso_stabilizer",
            "j_spine1",
            "back_low",
            "j_spine2",
            "back_mid",
            "j_spine3",
            "back_up",
            "j_spine4",
            "j_neck",
            "neck",
            "j_head",
            "head",
        ] {
            let parent = bones.len() as i32 - 1;
            let b = bone(name, parent, Vec3::Z, Quat::IDENTITY, &bones);
            bones.push(b);
        }
        Model::rig("cod2_spine", bones)
    }

    #[test]
    fn cod2_controls_bend_the_j_joints_above_them() {
        let m = cod2_spine();
        let skel = Skeleton::build(&[&m]);
        let c = Controllers {
            back_low: [0.0, 30.0, 0.0],
            neck: [0.0, 10.0, 0.0],
            ..Default::default()
        };
        let mut pose = PoseBuffer::new(&skel);
        apply_controllers(&mut pose, &skel, &c);
        let yaw = |name: &str| {
            let (_, r) = pose.bone_world(&skel, skel.bone_index(name).unwrap());
            let v = r * Vec3::X;
            v.y.atan2(v.x).to_degrees()
        };
        for (name, want) in [
            ("j_spine1", 0.0),
            ("back_low", 30.0),
            ("j_spine2", 30.0),
            ("j_spine4", 30.0),
            ("j_neck", 30.0),
            ("j_head", 40.0),
            ("head", 40.0),
        ] {
            assert!(near(yaw(name), want), "{name}: {}", yaw(name));
        }
    }

    /// CoD2's lean (Mac 0x182b82): the shift 2.5 a unit right, 5 left
    /// standing and 12.5 left crouched; the roll 1.25 but crouched right.
    #[test]
    fn cod2_lean_shifts_and_rolls_by_its_own_table() {
        let a = BodyAngles::default();
        let input = |eflags: i32| BodyInput {
            eflags,
            ..Default::default()
        };
        let slope = |lean: f32| BodySlope {
            lean,
            ..Default::default()
        };
        let f = lean_fraction(0.5);
        let roll = f * 50.0 * 0.925;
        for (eflags, lean, shift, scale) in [
            (0, 0.5, -2.5, 1.25),
            (0, -0.5, 5.0, 1.25),
            (EF_CROUCH, 0.5, -2.5, 1.0),
            (EF_CROUCH, -0.5, 12.5, 1.25),
        ] {
            let c = Controllers::cod2(&a, &input(eflags), &slope(lean));
            let sign = lean.signum();
            assert!(near(c.origin_trans.y, shift * f), "{eflags} {lean}: {c:?}");
            assert!(near(c.back_low[2], sign * roll * scale * 0.5), "{c:?}");
            assert!(near(c.head[2], sign * roll * scale * -0.3), "{c:?}");
            assert!(near(c.origin[2], sign * f * 50.0 * 0.075), "{c:?}");
        }
        // A gunner runs none.
        let gunner = Controllers::cod2(&a, &input(EF_MOUNTED), &slope(0.5));
        assert_eq!(gunner, Controllers::default());
    }

    /// CoD2's updater holds a mantling body's pitch level only while the
    /// mantle is its one flag (`cmpl $0x4000`, 0x80daf19), and swings its
    /// legs at the view.
    #[test]
    fn cod2_mantle_levels_the_pitch_only_alone() {
        let swing = |alone: bool| {
            let (mut a, mut input) = standing([30.0, 0.0, 0.0]);
            input.movement_dir = 45.0;
            let mantle = Mantle { on: true, alone };
            for _ in 0..40 {
                a.step_by(Updater::Cod2(mantle), &input, 50, BG_SWING_SPEED);
            }
            a
        };
        let alone = swing(true);
        assert!(near(alone.pitch, 0.0), "{alone:?}");
        assert!(near(alone.legs_yaw, 0.0), "{alone:?}");
        let with_others = swing(false);
        assert!(near(with_others.pitch, 18.0), "{with_others:?}");
    }

    /// A swing past its clamp lands a degree inside on 1.1 and on the clamp
    /// on CoD2 (0x80da7dc).
    #[test]
    fn cod2_clamps_a_swing_onto_its_limit() {
        let clamped = |updater: Updater| {
            let input = BodyInput {
                view: [0.0, 100.0, 0.0],
                eflags: EF_FIRING,
                legs: idle(),
                ..Default::default()
            };
            let mut a = BodyAngles::default();
            a.update_conditions(&input);
            a.step_by(updater, &input, 1, BG_SWING_SPEED);
            a.torso_yaw
        };
        assert!(near(clamped(Updater::Cod1), 56.0));
        assert!(near(clamped(Updater::Cod2(Mantle::default())), 55.0));
    }

    /// 1.7's updater swings the torso pitch toward twice the view's
    /// (0x804cefe), CoD2's toward 0.6 of it; CoD4's split then counters
    /// it on the neck and head.
    #[test]
    fn cod4_torso_pitches_twice_the_view() {
        let settle = |updater: Updater, pitch: f32| {
            let (mut a, input) = standing([pitch, 0.0, 0.0]);
            for _ in 0..80 {
                a.step_by(updater, &input, 50, BG_SWING_SPEED);
            }
            a.pitch
        };
        let cod4 = Updater::Cod4(Mantle::default());
        assert!(near(settle(cod4, 30.0), 60.0));
        assert!(near(angle_normalize_180(settle(cod4, 330.0)), -60.0));
        assert!(near(settle(Updater::Cod2(Mantle::default()), 30.0), 18.0));
    }
}
