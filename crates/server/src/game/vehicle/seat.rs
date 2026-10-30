//! Getting in, out and around a vehicle (docs/research/coduo-vehicles.md
//! 1 and 11): the use key's pick, `VEH_LinkPlayer` (0x8998d),
//! `VEH_UnlinkPlayer` (0x89fb2), `G_VEH_CycleSlot` (0x8da1e), and where a
//! seated player sits each frame (`G_PlayerVehiclePositionAndBlend`
//! 0x9048f). The host keeps the vehicle's half; what the player's sim must
//! do comes out as [`SeatOp`]s, which the server applies in the cmd that
//! raised them.

use super::info::{KIND_FOUR_WHEEL, KIND_TANK};
use super::{MAX_SEATS, Vehicle};
use crate::game::host::GameHost;
use glam::{Quat, Vec3};
use std::rc::Rc;
use vcod_common::skeleton::{PoseBuffer, Skeleton};
use vcod_gsc::{Cx, EntId, Host, Value};

/// The use key reaches this far along the view (`G_GetActivateEnt`).
const USE_REACH: f32 = 128.0;
/// `G_IsVehicleUsable`: no boarding past 100 units/s.
const BOARD_SPEED: f32 = 100.0;
/// No boarding while falling faster (-80, 0xa70cc).
const BOARD_FALL: f32 = -80.0;
/// `client+0x468c`'s windows: a change waits 250 ms, a seat's stamp sits
/// 1500 ms ahead, and a tank refuses an exit within 1500 ms of it.
const CHANGE_GAP_MS: i32 = 250;
const STAMP_AHEAD_MS: i32 = 1500;
const TANK_EXIT_DELAY_MS: i32 = 1500;
/// `GMI_GAME_VEHICLE_DELAY_IGNORE` at most once per 500 ms.
const DELAY_MESSAGE_MS: i32 = 500;
/// A tank driver's exit inside its coast window pops up at 200.
const EXIT_POP: f32 = 200.0;
/// `ENTITYNUM_NONE`.
const NONE: i32 = 1023;
/// A player's `r.mins`/`r.maxs`, what the dismount search fits.
pub const PLAYER_BOX: ([f32; 3], [f32; 3]) = (
    [
        -vcod_common::pmove::HALF_WIDTH,
        -vcod_common::pmove::HALF_WIDTH,
        0.0,
    ],
    [
        vcod_common::pmove::HALF_WIDTH,
        vcod_common::pmove::HALF_WIDTH,
        vcod_common::pmove::HEIGHT_STAND,
    ],
);

/// A rider's clocks (`client+0x468c`, `0x4690`, `0x4730`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RiderClock {
    pub stamp: i32,
    pub message: i32,
    pub cycles: i32,
}

/// What a seat change does to the player's sim and playerstate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeatOp {
    /// `VEH_LinkPlayer`'s player half: `SetClientOrigin` at the seat tag
    /// (+1 z, the teleport bit), `SetClientViewAngle` at the view tag, the
    /// seat bits, `viewlocked_entNum`, the sight flag off.
    Enter {
        slot: usize,
        vehicle: u32,
        seat: i32,
        kind: i32,
        origin: [f32; 3],
        view: [f32; 3],
        angles: [f32; 3],
        /// `BG_GetVehiclePosOffset(type, requested)`, the link's offset.
        link_offset: [f32; 3],
    },
    /// `VEH_UnlinkPlayer`'s: the bits off, `viewlocked_entNum` 1023,
    /// `vehPos` 0 (`vehType` kept), the sight off; a voluntary exit moves
    /// the body to the dismount spot (+1 z) and maybe pops it up.
    Exit {
        slot: usize,
        origin: Option<[f32; 3]>,
        velocity: Option<[f32; 3]>,
    },
}

/// A vehicle model loaded once: its skeleton for the tags and the model
/// for its collision surfaces.
pub struct Rig {
    pub skel: Skeleton,
    pub model: vcod_common::model::Model,
}

/// A vehicle posed as its wire fields have it this frame, for its tags.
pub struct Posed {
    rig: Rc<Rig>,
    worlds: Vec<(Vec3, Quat)>,
    pub origin: Vec3,
    pub rot: Quat,
    pub kind: i32,
}

impl Posed {
    /// A tag's world origin and rotation.
    pub fn tag(&self, name: &str) -> Option<(Vec3, Quat)> {
        let (p, r) = *self.worlds.get(self.rig.skel.bone_index(name)?)?;
        Some((self.origin + self.rot * p, self.rot * r))
    }

    /// Where seat `seat`'s rider sits: the seat tag shifted by
    /// `BG_GetVehiclePosOffset` in the tag's axes.
    pub fn seat(&self, seat: i32) -> Option<(Vec3, Quat)> {
        vcod_common::vehiclepose::seat_frame(
            &self.rig.skel,
            &self.worlds,
            (self.origin, self.rot),
            self.kind,
            seat,
        )
    }

    /// The model's collision triangles where the posed bones carry them, in
    /// the vehicle's own frame.
    pub fn collision(
        &self,
    ) -> (
        Vec<vcod_common::collision::ModelTri>,
        Vec<vcod_common::collision::ModelSurf>,
    ) {
        let (mut out, mut surfs) = (Vec::new(), Vec::new());
        vcod_common::vehiclepose::posed_collision_surfs(
            &self.rig.model,
            &self.worlds,
            &mut out,
            &mut surfs,
        );
        (out, surfs)
    }
}

pub(super) fn model_name(host: &mut GameHost, cx: &mut Cx, id: EntId) -> String {
    let atom = cx.intern_folded("model");
    match host.get_field(cx, id, atom) {
        Value::String(s) => cx.resolve(s).to_string(),
        _ => String::new(),
    }
}

/// The vehicle's rig, loaded once per model.
fn rig(host: &mut GameHost, model: &str) -> Option<Rc<Rig>> {
    if let Some(s) = host.vehicles.rigs.get(model) {
        return s.clone();
    }
    let rig = host.fs.clone().and_then(|fs| {
        let name = model.replace('\\', "/");
        let name = name.trim_start_matches("xmodel/");
        vcod_common::model::load(&*fs, name)
            .map_err(|e| log::warn!("vehicle model {name}: {e}"))
            .ok()
            .map(|m| {
                Rc::new(Rig {
                    skel: Skeleton::build(&[&m]),
                    model: m,
                })
            })
    });
    host.vehicles.rigs.insert(model.to_string(), rig.clone());
    rig
}

/// The server's pose of a vehicle: the DObj with `Scr_Vehicle_Controller`'s
/// controls (0x8c2d2) from the wire fields, at the vehicle's origin and
/// angles.
pub fn posed(host: &mut GameHost, cx: &mut Cx, id: EntId) -> Option<Posed> {
    posed_as(host, cx, id, false)
}

/// [`posed`] with the controls the frame's think started from, the pose a
/// DObj cached before the step keeps for the rest of the frame.
pub fn posed_as(host: &mut GameHost, cx: &mut Cx, id: EntId, cached: bool) -> Option<Posed> {
    let model = model_name(host, cx, id);
    let rig = rig(host, &model)?;
    let skel = &rig.skel;
    let v = host.vehicles.live.get(&id)?;
    let wire = if cached { v.posed_wire } else { v.wire };
    let kind = host.vehicles.info_of(v).kind;
    let fields = vcod_common::vehiclepose::VehicleFields {
        body_packed: [wire.origin2[0] as i32, wire.origin2[2] as i32],
        turret: [wire.angles2[0], wire.angles2[1]],
        gunner: [wire.angles2[2], wire.origin2[1]],
        steer: wire.leanf,
        tread_ms: wire.time,
        kind,
        gunner_spans: [wire.torso_pitch, wire.waist_pitch],
    };
    let pose_now = vcod_common::vehiclepose::lerp(&fields, &fields, 0.0);
    let mut pose = PoseBuffer::new(skel);
    vcod_common::vehiclepose::apply_hull(&mut pose, skel, &pose_now);
    let worlds = pose.bone_worlds(skel);
    Some(Posed {
        worlds,
        origin: Vec3::from(v.pose.origin),
        rot: vcod_common::turretpose::angles_quat(v.pose.angles),
        kind,
        rig,
    })
}

/// `vectoangles` of a rotation's forward, roll 0 (`Axis4ToAngles`, the
/// roll overwritten).
fn view_of(rot: Quat) -> [f32; 3] {
    let f = rot * Vec3::X;
    let yaw = f.y.atan2(f.x).to_degrees();
    let pitch = (-f.z).atan2(f.x.hypot(f.y)).to_degrees();
    let norm = |a: f32| if a < 0.0 { a + 360.0 } else { a };
    [norm(pitch), norm(yaw), 0.0]
}

/// The free-seat search 0x88c78: from `start` stepping forward (or back),
/// wrapping 6 to 1, never `start` itself; seat 2 needs a gunner's seat,
/// seat n from 3 a passenger count over n - 3.
pub fn next_free_seat(
    v: &Vehicle,
    has_gunner: bool,
    passengers: i32,
    start: usize,
    back: bool,
) -> Option<usize> {
    let valid = |s: usize| match s {
        1 => true,
        2 => has_gunner,
        s => passengers > s as i32 - 3,
    };
    let mut s = start;
    for _ in 0..=MAX_SEATS {
        s = if back {
            if s <= 1 { MAX_SEATS } else { s - 1 }
        } else if s >= MAX_SEATS {
            1
        } else {
            s + 1
        };
        if s == start {
            return None;
        }
        if valid(s) && v.seats[s].is_none() {
            return Some(s);
        }
    }
    None
}

fn free_seat(host: &GameHost, id: EntId, start: usize, back: bool) -> Option<usize> {
    let v = host.vehicles.live.get(&id)?;
    let info = host.vehicles.info_of(v);
    next_free_seat(v, info.has_gunner, info.num_passengers, start, back)
}

fn team(host: &mut GameHost, cx: &mut Cx, slot: usize) -> String {
    let Some(ent) = host.ents.handle(slot as u32) else {
        return String::new();
    };
    let atom = cx.intern_folded("sessionteam");
    match host.get_field(cx, ent, atom) {
        Value::String(s) => cx.resolve(s).to_string(),
        _ => String::new(),
    }
}

/// `G_IsVehicleUsable` (0x8b673): not seated, a seat free, the vehicle slow
/// and alive and usable, no grenade in hand, and every rider a teammate.
pub fn usable(host: &mut GameHost, cx: &mut Cx, id: EntId, slot: usize) -> bool {
    if host.vehicles.seat_of(slot).is_some() || free_seat(host, id, 0, false).is_none() {
        return false;
    }
    let Some(v) = host.vehicles.live.get(&id) else {
        return false;
    };
    if v.contents & super::CONTENTS_USABLE == 0 || v.speed > BOARD_SPEED {
        return false;
    }
    let riders: Vec<usize> = v.seats.iter().flatten().copied().collect();
    if crate::game::builtins::vehicle::health_of(host, cx, id) < 1 {
        return false;
    }
    if host.client_grenade_ms.get(slot).copied().unwrap_or(0) != 0 {
        return false;
    }
    let mine = team(host, cx, slot);
    riders.into_iter().all(|r| {
        let theirs = team(host, cx, r);
        theirs == mine && (theirs == "axis" || theirs == "allies")
    })
}

/// The use key's vehicle (`G_GetActivateEnt`'s vehicle arm): the usable
/// vehicle the view's 128-unit ray meets first, before the world, and how
/// far along.
pub fn aimed_vehicle(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    eye: [f32; 3],
    view: [f32; 3],
) -> Option<(EntId, f32)> {
    let fwd = Vec3::from(crate::game::spawn::angle_forward(view));
    let eye = Vec3::from(eye);
    let wall = host.world.as_ref().map_or(USE_REACH, |w| {
        let tr = w.collision.point_trace(
            eye,
            eye + fwd * USE_REACH,
            vcod_common::collision::CONTENTS_SOLID,
            false,
        );
        tr.fraction * USE_REACH
    });
    let mut best: Option<(EntId, f32)> = None;
    for id in host.vehicles.in_slot_order() {
        if host.ents.get(id).is_none_or(|e| e.unlinked) {
            continue;
        }
        let Some(v) = host.vehicles.live.get(&id) else {
            continue;
        };
        let info = host.vehicles.info_of(v);
        let rot = vcod_common::turretpose::angles_quat(v.pose.angles);
        let o = Vec3::from(v.pose.origin);
        let (lo, hi) = (Vec3::from(info.mins), Vec3::from(info.maxs));
        let local_eye = rot.inverse() * (eye - o);
        let local_dir = rot.inverse() * fwd;
        let Some(t) = ray_box(local_eye, local_dir, lo, hi) else {
            continue;
        };
        if t > USE_REACH || t > wall + 1.0 || best.is_some_and(|(_, b)| b <= t) {
            continue;
        }
        if usable(host, cx, id, slot) {
            best = Some((id, t));
        }
    }
    best
}

/// The entry distance of a ray into a box, 0 from inside.
fn ray_box(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, f32::MAX);
    for i in 0..3 {
        if d[i].abs() < 1e-6 {
            if o[i] < lo[i] || o[i] > hi[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

fn clock(host: &mut GameHost, slot: usize) -> &mut RiderClock {
    host.vehicles.riders.entry(slot).or_default()
}

fn send(host: &mut GameHost, slot: usize, cmd: &str) {
    host.client_commands.push((slot, cmd.to_string()));
}

/// Raised for the next frame's threads, as an engine notify from a cmd.
fn notify(host: &mut GameHost, on: EntId, event: &'static str, args: Vec<Value>) {
    host.engine_notifies.push((on, event, args));
}

/// `Scr_Vehicle_Use` (0x8dc43) for a player not seated: `VEH_LinkPlayer`
/// at the first free seat, then the change count reset.
pub fn board(host: &mut GameHost, cx: &mut Cx, id: EntId, slot: usize, on_ground: bool, fall: f32) {
    link_player(host, cx, id, slot, 0, on_ground, fall);
    clock(host, slot).cycles = 0;
}

/// `VEH_LinkPlayer` (0x8998d). `requested` 0 takes the first free seat.
pub fn link_player(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    slot: usize,
    requested: usize,
    on_ground: bool,
    fall: f32,
) {
    let now = host.level_time_ms;
    let Some(player) = host.ents.handle(slot as u32) else {
        return;
    };
    // The owner lock (a bought tank's), on an empty driver's seat.
    let owner_atom = cx.intern_folded("vehicle_owner");
    if let Value::Entity(owner) = host.get_field(cx, id, owner_atom)
        && host.ents.get(owner).is_some_and(|e| e.client.is_some())
        && owner != player
        && host
            .vehicles
            .live
            .get(&id)
            .is_some_and(|v| v.seats[1].is_none())
    {
        send(host, slot, "e \"GMI_GAME_VEHICLE_NOT_OWNER\"");
        return;
    }
    let c = *clock(host, slot);
    if !(on_ground || fall >= BOARD_FALL) || c.stamp > now - CHANGE_GAP_MS {
        return;
    }
    let Some(v) = host.vehicles.live.get(&id) else {
        return;
    };
    let info = host.vehicles.info_of(v).clone();
    let tank = info.kind == KIND_TANK;
    clock(host, slot).stamp = if tank && c.cycles == 0 {
        now
    } else {
        now + STAMP_AHEAD_MS
    };
    let seat = if requested != 0 && host.vehicles.live[&id].seats[requested].is_none() {
        Some(requested)
    } else {
        free_seat(host, id, 0, false)
    };
    let Some(seat) = seat else {
        log::error!("VEH_LinkPlayer: Vehicle is full");
        return;
    };
    let posed = posed(host, cx, id);
    let seat_tag = vcod_common::vehiclepose::seat_tag(seat as i32).unwrap_or("tag_player");
    let view_tag = match seat {
        1 => "tag_player",
        2 => "tag_secondary_gun",
        _ => "tag_body",
    };
    let pose = host.vehicles.live[&id].pose;
    let origin = posed
        .as_ref()
        .and_then(|p| p.tag(seat_tag))
        .map_or(pose.origin, |(o, _)| o.to_array());
    let view = posed
        .as_ref()
        .and_then(|p| p.tag(view_tag))
        .map_or([0.0, pose.angles[1], 0.0], |(_, r)| view_of(r));
    host.vehicles.stale_seated.remove(&slot);
    let v = host.vehicles.live.get_mut(&id).unwrap();
    v.seats[seat] = Some(slot);
    v.wire.anim_movetype |= 1 << seat;
    if seat == 1 {
        v.driven = true;
        v.wire.driven_flag = true;
        v.wire.other = slot as i32;
    }
    v.wire.torso_pitch = info.gunner.vert_up;
    v.wire.waist_pitch = info.gunner.vert_down;
    let angles = v.pose.angles;
    host.vehicles.seat_ops.push(SeatOp::Enter {
        slot,
        vehicle: id.0,
        seat: seat as i32,
        kind: info.kind,
        origin,
        view,
        angles,
        link_offset: vcod_common::vehiclepose::seat_offset(info.kind, requested as i32).to_array(),
    });
    send(host, slot, "v cl_run 1");
    send(host, slot, "v cl_stance 0");
    notify(
        host,
        id,
        "activated",
        vec![Value::Int(seat as i32), Value::Entity(player)],
    );
    notify(
        host,
        player,
        "vehicle_activated",
        vec![Value::Int(seat as i32), Value::Entity(id)],
    );
}

/// `VEH_UnlinkPlayer` (0x89fb2). A voluntary exit (the use key) can be
/// refused: a tank inside its delay, or no dismount spot. Returns whether
/// the player is out. `from` and `bounds` are the player's origin and box.
pub fn unlink_player(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    voluntary: bool,
    from: [f32; 3],
    bounds: ([f32; 3], [f32; 3]),
) -> bool {
    let now = host.level_time_ms;
    let Some((id, seat)) = host.vehicles.seat_of(slot) else {
        return true;
    };
    let Some(player) = host.ents.handle(slot as u32) else {
        return false;
    };
    let info = host.vehicles.info_of(&host.vehicles.live[&id]).clone();
    let c = *clock(host, slot);
    let alive = crate::game::builtins::vehicle::health_of(host, cx, id) >= 1;
    if alive
        && voluntary
        && c.stamp > now - TANK_EXIT_DELAY_MS
        && c.cycles <= 1
        && info.kind == KIND_TANK
    {
        send(host, slot, "e \"GMI_GAME_VEHICLE_EXIT_DELAY\"");
        return false;
    }
    let mut spot = None;
    if voluntary {
        let velocity = host.vehicles.live[&id].velocity;
        spot = super::dismount_spot(host, id, from, bounds, Some(velocity), Some(slot));
        if spot.is_none() {
            send(host, slot, "e \"GMI_GAME_VEHICLE_UNLINK_BLOCKED\"");
            return false;
        }
    }
    let v = host.vehicles.live.get_mut(&id).unwrap();
    v.seats[seat] = None;
    v.wire.anim_movetype &= !(1 << seat);
    if seat == 1 {
        v.wire.driven_flag = false;
        v.wire.other = NONE;
        v.wire.head_icon[0] = 0;
        // A tank coasts on for 3 s after its driver leaves.
        if info.kind == KIND_TANK {
            v.coast_until = now + 3000;
            v.phys.driver_left(&info, now);
        }
    }
    if v.wire.other == slot as i32 {
        v.wire.other = NONE;
    }
    let pop = voluntary && now < v.coast_until;
    // `isInVehicle` reads the entity state's bit, which only a dismount's
    // `SetClientOrigin` updates before the end frame
    // (`PlayerCmd_isInVehicle` 0x46ca0).
    if spot.is_none() {
        host.vehicles.stale_seated.insert(slot);
    }
    host.vehicles.seat_ops.push(SeatOp::Exit {
        slot,
        origin: spot,
        velocity: pop.then_some([0.0, 0.0, EXIT_POP]),
    });
    send(host, slot, "v cl_run 1");
    send(host, slot, "v cl_stance 0");
    notify(host, id, "deactivated", vec![Value::Entity(player)]);
    notify(host, player, "vehicle_deactivated", vec![Value::Entity(id)]);
    true
}

/// `G_VEH_CycleSlot` (0x8da1e), `nextvehslot` / `prevvehslot`: a forced
/// exit and an entry at the next free seat, 1750 ms apart at the most.
pub fn cycle(host: &mut GameHost, cx: &mut Cx, slot: usize, back: bool) {
    let now = host.level_time_ms;
    let Some((id, seat)) = host.vehicles.seat_of(slot) else {
        return;
    };
    if free_seat(host, id, 0, false).is_none() {
        return;
    }
    let kind = host.vehicles.info_of(&host.vehicles.live[&id]).kind;
    if kind == KIND_TANK && clock(host, slot).cycles == 0 {
        clock(host, slot).stamp = 0;
    }
    let c = *clock(host, slot);
    if c.stamp > now - CHANGE_GAP_MS {
        if c.message < now - DELAY_MESSAGE_MS {
            send(host, slot, "e \"GMI_GAME_VEHICLE_DELAY_IGNORE\"");
            clock(host, slot).message = now;
        }
        return;
    }
    {
        let c = clock(host, slot);
        c.stamp = 0;
        c.cycles += 1;
    }
    unlink_player(host, cx, slot, false, [0.0; 3], ([0.0; 3], [0.0; 3]));
    let Some(next) = free_seat(host, id, seat, back) else {
        return;
    };
    clock(host, slot).stamp = 0;
    // A cycle's ground test passes: the seated body falls at 0.
    link_player(host, cx, id, slot, next, false, 0.0);
    clock(host, slot).stamp = now + STAMP_AHEAD_MS;
}

/// `ClientEndFrame`'s `vehMotion`: 1 under 20 units/s, else 2 forward and
/// 3 back, off the hull's own forward speed.
pub fn motion(forward_speed: f32) -> i32 {
    if forward_speed.abs() < 20.0 {
        1
    } else if forward_speed > 0.0 {
        2
    } else {
        3
    }
}

/// Whether the engine's snapshot build leaves a seated player out for
/// everyone (`coduo_lnxded` 0x8096b97): a tank's driver.
pub fn hidden_rider(kind: i32, seat: i32) -> bool {
    kind == KIND_TANK && seat == 1
}

/// `BG_AllowPlayerWeaponAtVehiclePos`: only a four-wheel's seat 3 fires
/// the rider's own weapon.
pub fn own_weapon(kind: i32, seat: i32) -> bool {
    kind == KIND_FOUR_WHEEL && seat == 3
}
