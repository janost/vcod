//! United Offensive's vehicles on the server: the definition files, the
//! 64-slot pool, spawn, seats, the drive step, the guns and the damage
//! (docs/research/coduo-vehicles.md, "The server"). Addresses are
//! `game.mp.uo.i386.so`'s.

pub mod die;
pub mod fire;
pub mod info;
pub mod physics;
pub mod seat;
pub mod think;

use crate::configstrings::CsRange;
use crate::game::host::GameHost;
use info::{VehicleInfo, VehicleInfos};
use std::collections::HashMap;
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Value};

/// The pool `G_SpawnVehicle` (0x8abfd) takes a slot of; past it retail's
/// `Com_Error "Hit max vehicle count"`.
pub const MAX_VEHICLES: usize = 64;
/// Seats 1..6: 1 the driver, 2 the gunner, 3.. passengers.
pub const MAX_SEATS: usize = 6;
/// `r.contents` at spawn (0x7c935), plus [`CONTENTS_USABLE`] under
/// spawnflag 1; a collmap's pairing makes it `0x800000 | 0x200000`.
pub const CONTENTS_VEHICLE: i32 = 0x2080;
pub const CONTENTS_USABLE: i32 = 0x0020_0000;
pub const CONTENTS_COLLMAP: i32 = 0x0080_0000 | CONTENTS_USABLE;
/// `ENTITYNUM_NONE`.
const NONE: i32 = 1023;
/// `Scr_Vehicle_Init`'s first think, 50 ms after the spawn.
pub const THINK_MS: i32 = 50;

/// What the vehicle entity carries on the wire beyond its pose
/// (docs/research/coduo-vehicles.md 12, "The entity").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleWire {
    /// Turret pitch and yaw off the hull, the gunner's pitch.
    pub angles2: [f32; 3],
    /// Packed body pitch (an int 0..255), the gunner's yaw, packed roll.
    pub origin2: [f32; 3],
    pub leanf: f32,
    /// The travel clock; the wire keeps 16 bits.
    pub time: i32,
    /// `trunc(1000 * suspensionTravel)`; the wire keeps 6 bits.
    pub time2: i32,
    /// The seat mask, bit n for seat n.
    pub anim_movetype: i32,
    /// The driver's entity number, 1023 with none.
    pub other: i32,
    /// UO raw 0x100000 while driven, in vcod's 1.1 layout.
    pub driven_flag: bool,
    pub head_icon: [i32; 2],
    pub loop_sound: i32,
    /// `fTorsoPitch`, `fWaistPitch`: the gunner's vertical spans, written at
    /// every seat link.
    pub torso_pitch: f32,
    pub waist_pitch: f32,
    /// `s.weapon`: the turret weapon at spawn, the gun that fired last.
    pub weapon: i32,
}

impl Default for VehicleWire {
    fn default() -> Self {
        VehicleWire {
            angles2: [0.0; 3],
            origin2: [127.0, 0.0, 127.0],
            leanf: 0.0,
            time: 0,
            time2: 0,
            anim_movetype: 0,
            other: NONE,
            driven_flag: false,
            head_icon: [0; 2],
            loop_sound: 0,
            torso_pitch: 0.0,
            waist_pitch: 0.0,
            weapon: 0,
        }
    }
}

/// Where the vehicle stands.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub origin: [f32; 3],
    pub angles: [f32; 3],
}

/// The three guns' timers and the two MGs' heat (`rec+0x240..0x268`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Guns {
    /// Main, alt and gunner fire timers in ms, 50 off a think.
    pub timers: [i32; 3],
    /// Alt and gunner heat, 0..
    pub heat: [f32; 2],
    pub overheating: [bool; 2],
    /// `rec+0x2e4`/`0x2f0`: the MG loops' hold, ms.
    pub loop_hold: [i32; 2],
}

/// Which gun a fire call or a notify is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gun {
    Main,
    Alt,
    Gunner,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TurretTarget {
    Point([f32; 3]),
    Entity(EntId, [f32; 3]),
}

/// `joltBody(point, force, speedFrac, decel)` (0x8e5dc) as asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jolt {
    pub from: [f32; 3],
    pub force: f32,
    pub speed_frac: f32,
    pub decel: f32,
}

/// One live vehicle: the pool slot's record.
#[derive(Clone, Debug)]
pub struct Vehicle {
    /// The pool slot, also `s.attackerEntityNum`.
    pub slot: usize,
    /// Into [`Vehicles::infos`].
    pub info: usize,
    /// The BSP block it spawned from, for `spawnDuplicate`
    /// (`G_BackupSpawnVars` 0x7a3d7).
    pub spawn_vars: Vec<(String, String)>,
    /// The four sound-blend entities `fn_7cebf` spawns: idle, engine and two
    /// for the guns' loops.
    pub blends: [Option<EntId>; 4],
    /// Each blend's `G_SetSoundBlend` state: the two aliases and the
    /// crossfade, `None` unlinked.
    pub sound_blends: [Option<(i32, i32, f32)>; 4],
    /// `rec+0x234`: the hint string's slot (configstring 1365 + n).
    pub hint_string: i32,
    /// Seat n's occupant by client slot, index 0 unused.
    pub seats: [Option<usize>; MAX_SEATS + 1],
    /// `ent+0x17e` 2: a player drives it, so it steps from
    /// `G_VehicleClientThink` every frame.
    pub driven: bool,
    /// The paired `script_vehicle_collmap`'s inline model, `s.index`.
    pub collmap: Option<u32>,
    /// `r.contents`.
    pub contents: i32,
    /// The weapon indices of the three guns, 0 for none.
    pub weapons: [usize; 3],
    /// The alt and gunner weapons' `loopFireSound`/`stopFireSound` aliases
    /// (`rec+0x2dc`, `0x2e0`, `0x2e8`, `0x2ec`).
    pub gun_sounds: [i32; 4],
    pub wire: VehicleWire,
    /// The wire fields as the last think found them: what the frame's
    /// first tag query posed the DObj with.
    pub posed_wire: VehicleWire,
    /// `r.currentOrigin` and `r.currentAngles`.
    pub pose: Pose,
    /// The record's physics and the entity fields the step owns
    /// (`physics`).
    pub phys: physics::Physics,
    pub guns: Guns,
    /// `ent+0x1f0`, the hull's forward speed's size; `rec+0xf4`'s x signed
    /// (what `vehMotion` reads), and the world velocity.
    pub speed: f32,
    pub forward_speed: f32,
    pub velocity: [f32; 3],
    /// `rec+0x390`: a driverless tank coasts on its last input until then.
    pub coast_until: i32,
    /// The driver's UO `eFlags` 0x200000 (the view turns with the hull),
    /// as the step left it.
    pub hull_view: bool,
    /// `rec+0x2c0`: `startEngineSound`/`stopEngineSound`.
    pub engine_on: bool,
    /// `rec+0x250`: `addVehicleToCompass`.
    pub on_compass: bool,
    /// `rec+0x284..0x2a0`: a script target for the turret
    /// (`setTurretTargetVec`/`Ent`), which a driver's aim overwrites.
    pub target: Option<TurretTarget>,
    /// `rec+0x28c`: where the driver's view meets the world, as the last
    /// think's reader found it; `None` with no driver.
    pub aim_point: Option<[f32; 3]>,
    /// `rec+0x270`: the last think's reader found the world between
    /// `tag_barrel` and `tag_flash`, so the guns fire straight from the
    /// barrel's root (`fire::prepare`).
    pub barrel_blocked: bool,
    /// `joltBody` calls not yet taken by the step.
    pub jolts: Vec<Jolt>,
    /// `nextthink`, and whether `Scr_Vehicle_Init` has run.
    pub next_think: i32,
    pub initialized: bool,
}

/// The level's vehicles: the files, the pool and each live vehicle.
#[derive(Clone)]
pub struct Vehicles {
    pub infos: VehicleInfos,
    pub slots: [Option<EntId>; MAX_VEHICLES],
    pub live: HashMap<EntId, Vehicle>,
    /// `fireTurret`, `fireAltTurret` and `fireGunner` calls on a bullet gun
    /// this frame, for the server to trace (it holds the bodies); a shell
    /// gun fires inside the call (`fire::launch_shell`).
    pub fire_ops: Vec<(EntId, Gun)>,
    /// Vehicles whose health ran out outside a builtin, for
    /// `Scr_Vehicle_Die` ([`die::drain`]).
    pub deaths: Vec<die::Death>,
    /// The deaths whose riders are being walked, innermost last.
    pub dying: Vec<die::Walk>,
    /// The walk's unlink notifies, for whichever context runs it to raise.
    pub die_notifies: Vec<(EntId, &'static str, Vec<Value>)>,
    /// A rider the walk put out, by slot: his origin under `G_SetOrigin`'s
    /// lift (`ps.origin`), until the next mirror of his origin.
    pub lifted: HashMap<usize, [f32; 3]>,
    /// Riders put out where they sat whose entity state still carries the
    /// seat bit, until their next end frame's mirror.
    pub stale_seated: std::collections::HashSet<usize>,
    /// Bodies the steps struck this frame, for the server to push
    /// (`VEH_PlayerCollision`).
    pub strikes: Vec<(EntId, physics::ClientStrike)>,
    /// When each client was last run over (`client+0x4728`).
    pub crushed: HashMap<usize, i32>,
    /// Each rider's seat clocks, by client slot.
    pub riders: HashMap<usize, seat::RiderClock>,
    /// What seat changes this cmd did to the players' sims.
    pub seat_ops: Vec<seat::SeatOp>,
    /// The gunners whose MG is overheated (`pm_flags` 0x800000).
    pub overheated_gunners: Vec<usize>,
    /// Each vehicle model's skeleton and collision, for its tags and the
    /// locational trace.
    pub rigs: HashMap<String, Option<std::rc::Rc<seat::Rig>>>,
    /// Stuck kills landed since the level loaded, for the bot gates.
    pub stuck_kills: u32,
}

impl Default for Vehicles {
    fn default() -> Self {
        Vehicles {
            infos: VehicleInfos::default(),
            slots: [None; MAX_VEHICLES],
            live: HashMap::new(),
            fire_ops: Vec::new(),
            deaths: Vec::new(),
            dying: Vec::new(),
            die_notifies: Vec::new(),
            lifted: HashMap::new(),
            stale_seated: std::collections::HashSet::new(),
            riders: HashMap::new(),
            strikes: Vec::new(),
            crushed: HashMap::new(),
            overheated_gunners: Vec::new(),
            seat_ops: Vec::new(),
            rigs: HashMap::new(),
            stuck_kills: 0,
        }
    }
}

impl Vehicles {
    pub fn info_of(&self, v: &Vehicle) -> &VehicleInfo {
        &self.infos.list[v.info]
    }

    /// The vehicle entities in pool-slot order.
    pub fn in_slot_order(&self) -> Vec<EntId> {
        self.slots.iter().flatten().copied().collect()
    }

    /// The vehicle `slot`'s client sits in, and the seat.
    pub fn seat_of(&self, client: usize) -> Option<(EntId, usize)> {
        self.live.iter().find_map(|(&id, v)| {
            (1..=MAX_SEATS)
                .find(|&s| v.seats[s] == Some(client))
                .map(|s| (id, s))
        })
    }

    /// `client` drives a tank: no round from outside reaches him
    /// ([`Self::occupant_damage`]), and the engine sends his body to nobody
    /// (section 11.4).
    pub fn drives_tank(&self, client: usize) -> bool {
        self.seat_of(client).is_some_and(|(veh, seat)| {
            seat == 1 && self.info_of(&self.live[&veh]).kind == info::KIND_TANK
        })
    }

    /// `G_DamageClient`'s rider branch (0x4fd4b) for `client`, out of every
    /// seat `Some(damage)`: [`rider_damage`].
    pub fn occupant_damage(
        &self,
        client: usize,
        inflictor: Option<EntId>,
        damage: i32,
        dflags: i32,
    ) -> Option<i32> {
        let Some((veh, seat)) = self.seat_of(client) else {
            return Some(damage);
        };
        let kind = self.info_of(&self.live[&veh]).kind;
        rider_damage(kind, seat, veh, inflictor, damage, dflags)
    }
}

/// `G_DamageClient`'s rider branch (0x4fd4b), ahead of the damage callback,
/// for a rider in `seat` of `veh`, a vehicle of `kind`: a blast is scaled
/// by [`occupant_radius_scale`] and truncated, then a tank's driver drops
/// anything whose inflictor is not his own tank
/// (`G_IsVehicleOccupantInvulnerable` 0x8b96b). A bullet or a swing names
/// its attacker as the inflictor; no inflictor (fall damage, the
/// `radiusDamage` builtin) passes. `None` when the hit is dropped
/// (docs/research/coduo-vehicles.md 13).
pub fn rider_damage(
    kind: i32,
    seat: usize,
    veh: EntId,
    inflictor: Option<EntId>,
    damage: i32,
    dflags: i32,
) -> Option<i32> {
    let mut damage = damage;
    if dflags & crate::game::combat::DFLAG_RADIUS != 0 {
        // x87 under a 0xc00 control word: the f32 scale times the damage,
        // truncated.
        damage = (f64::from(occupant_radius_scale(kind)) * f64::from(damage)).trunc() as i32;
    }
    if kind == info::KIND_TANK && seat == 1 && inflictor.is_some_and(|i| i != veh) {
        return None;
    }
    Some(damage)
}

/// `G_ParseScrVehicleInfo` at the top of `G_InitGame` (0x586cf): the files,
/// then their aliases ahead of every other alias of the level. United
/// Offensive only.
pub fn load_level(host: &mut GameHost) {
    if !host.defs.proto.is_uo() {
        return;
    }
    let Some(fs) = host.fs.clone() else {
        return;
    };
    let mut infos = VehicleInfos::load(&fs);
    infos.register_sounds(|alias| sound_alias(host, alias));
    host.vehicles = Vehicles {
        infos,
        // A restart's, carried (`ModelCaches`).
        rigs: std::mem::take(&mut host.vehicles.rigs),
        ..Vehicles::default()
    };
}

/// `G_SoundAliasIndex`: the alias's number, 0 when the block is full.
fn sound_alias(host: &mut GameHost, alias: &str) -> i32 {
    match host
        .allocators
        .index(&mut host.configstrings, CsRange::SoundAlias, alias)
    {
        Ok(slot) => (slot - host.allocators.layout().sounds()) as i32,
        Err(e) => {
            log::warn!("vehicle sound alias {alias:?} not indexed: {e:?}");
            0
        }
    }
}

fn field_string(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str) -> String {
    let atom = cx.intern_folded(name);
    match host.get_field(cx, id, atom) {
        Value::String(s) => cx.resolve(s).to_string(),
        _ => String::new(),
    }
}

pub(crate) fn set_field(host: &mut GameHost, cx: &mut Cx, id: EntId, name: &str, v: Value) {
    let atom = cx.intern_folded(name);
    if let Err(e) = host.set_field(cx, id, atom, v) {
        log::warn!("vehicle {}: {name}: {e:?}", id.0);
    }
}

/// `BG_GetWeaponIndexForName`: case-insensitive, 0 for none.
fn weapon_index(host: &GameHost, name: &str) -> usize {
    if name.is_empty() {
        return 0;
    }
    host.defs
        .weapon_index(&name.to_ascii_lowercase())
        .unwrap_or(0)
}

/// `SP_script_vehicle` (0x8dec6) on an entity whose fields are parsed:
/// `G_SpawnVehicle`'s slot, the entity (`fn_7c935`), the slot's record and
/// the sound-blend entities (`fn_7cebf`). `spawn_vars` is the BSP block, or
/// the backup a `spawnDuplicate` restores. The collmap pairs here only
/// after load ([`pair_collmaps`] runs once for the BSP pass).
pub fn spawn(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    spawn_vars: Vec<(String, String)>,
    loading: bool,
) -> Result<(), ErrorKind> {
    // `SP_script_vehicle`'s `G_SpawnFloat("delay", g_vehicleBurnTime)`
    // (game.mp.uo 0x9dec6): the first `strcmp` copy over the field pass's.
    let delay = match vcod_common::bsp::spawn_string(&spawn_vars, "delay") {
        Some(v) => crate::game::builtins::cvar::atof(v),
        None => crate::game::builtins::cvar::atof(host.cvars.get("g_vehicleBurnTime")),
    };
    set_field(host, cx, id, "delay", Value::Float(delay));
    let vehicletype = field_string(host, cx, id, "vehicletype");
    let Some(slot) = host.vehicles.slots.iter().position(Option::is_none) else {
        log::error!("Hit max vehicle count [{MAX_VEHICLES}]");
        return Ok(());
    };
    let Some(info_index) = host.vehicles.infos.find(&vehicletype) else {
        let targetname = field_string(host, cx, id, "targetname");
        log::error!("Can't find info for script vehicle [{targetname}]");
        return Ok(());
    };
    let info = host.vehicles.infos.list[info_index].clone();
    let spawnflags = vcod_common::bsp::field_value(&spawn_vars, "spawnflags")
        .map_or(0, crate::game::builtins::cvar::atoi);
    let weapons = [
        weapon_index(host, &info.turret_weapon),
        weapon_index(host, &info.turret_alt_weapon),
        weapon_index(host, &info.gunner_weapon),
    ];
    let contents = CONTENTS_VEHICLE
        | if spawnflags & 1 != 0 {
            CONTENTS_USABLE
        } else {
            0
        };
    // `RegisterItem` on the three guns: their models, nothing else.
    for w in [
        &info.turret_weapon,
        &info.turret_alt_weapon,
        &info.gunner_weapon,
    ] {
        if !w.is_empty() {
            host.register_item(&w.to_ascii_lowercase())?;
        }
    }
    let hint_string = crate::configstrings::hint_string_alloc(
        &mut host.configstrings,
        host.allocators.layout(),
        &info.use_hint_string,
    )
    .unwrap_or(-1);
    // B1 and B2 only with both of their aliases, B3 and B4 always (the
    // test on those two is a null check that never fires, 0x7d1e4).
    let mut blends = [None; 4];
    let wanted = [
        info.sound_index[0] != 0 && info.sound_index[1] != 0,
        info.sound_index[2] != 0 && info.sound_index[3] != 0,
        true,
        true,
    ];
    for (b, want) in blends.iter_mut().zip(wanted) {
        if want {
            match host.ents.spawn(cx) {
                // `SP_sound_blend`'s `svFlags |= 8` never reaches the cull:
                // the snapshot builder tests a blend as its vehicle
                // (`crate::game::wire::sound_blend_vehicle`).
                Ok(e) => *b = Some(e),
                Err(err) => log::error!("vehicle sound blend: {err:?}"),
            }
        }
    }
    let mut gun_sounds = [0; 4];
    for (k, w) in [weapons[1], weapons[2]].into_iter().enumerate() {
        let Some(def) = host.weapons.get(w).cloned() else {
            continue;
        };
        for (j, alias) in [&def.sounds.loop_fire, &def.sounds.stop_fire]
            .into_iter()
            .enumerate()
        {
            if let Some(a) = alias.as_deref().filter(|a| !a.is_empty()) {
                gun_sounds[k * 2 + j] = sound_alias(host, a);
            }
        }
    }
    let wire = VehicleWire {
        weapon: weapons[0] as i32,
        ..VehicleWire::default()
    };
    let pose = Pose {
        origin: crate::game::item::origin_of(host, cx, id),
        angles: crate::game::item::angles_of(host, cx, id),
    };
    let model_name = field_string(host, cx, id, "model");
    let phys = physics::Physics::spawn(
        pose.origin.into(),
        pose.angles.into(),
        vehicle_model(host, &model_name),
    );
    host.vehicles.slots[slot] = Some(id);
    host.vehicles.live.insert(
        id,
        Vehicle {
            slot,
            info: info_index,
            spawn_vars,
            blends,
            // `fn_7cebf` links each blend as it spawns it, silent; only the
            // think's `G_SetSoundBlend` unlinks one (0x87480).
            sound_blends: blends.map(|b| b.map(|_| (0, 0, 0.0))),
            hint_string,
            seats: [None; MAX_SEATS + 1],
            driven: false,
            collmap: None,
            contents,
            weapons,
            gun_sounds,
            wire,
            posed_wire: wire,
            pose,
            phys,
            guns: Guns::default(),
            speed: 0.0,
            forward_speed: 0.0,
            velocity: [0.0; 3],
            coast_until: 0,
            hull_view: true,
            engine_on: true,
            on_compass: false,
            target: None,
            jolts: Vec::new(),
            aim_point: None,
            barrel_blocked: false,
            next_think: host.level_time_ms + THINK_MS,
            initialized: false,
        },
    );
    if !loading {
        pair_collmap(host, cx, id);
    }
    link(host, cx, id);
    Ok(())
}

#[cfg(test)]
impl Vehicle {
    /// A bare record in pool slot `slot`, for a test that needs only a live
    /// vehicle.
    pub fn for_test(slot: usize) -> Vehicle {
        Vehicle {
            slot,
            info: 0,
            spawn_vars: Vec::new(),
            blends: [None; 4],
            sound_blends: [None; 4],
            hint_string: 0,
            seats: [None; MAX_SEATS + 1],
            driven: false,
            collmap: None,
            contents: 0,
            weapons: [0; 3],
            gun_sounds: [0; 4],
            wire: VehicleWire::default(),
            posed_wire: VehicleWire::default(),
            pose: Pose::default(),
            phys: physics::Physics::spawn(
                glam::Vec3::ZERO,
                glam::Vec3::ZERO,
                physics::VehicleModel::default(),
            ),
            guns: Guns::default(),
            speed: 0.0,
            forward_speed: 0.0,
            velocity: [0.0; 3],
            coast_until: 0,
            hull_view: true,
            engine_on: true,
            on_compass: false,
            target: None,
            aim_point: None,
            barrel_blocked: false,
            jolts: Vec::new(),
            next_think: 0,
            initialized: false,
        }
    }
}

/// The wheel and gun tags the step reads off the vehicle's model.
fn vehicle_model(host: &mut GameHost, model: &str) -> physics::VehicleModel {
    let Some(fs) = host.fs.clone() else {
        return physics::VehicleModel::default();
    };
    let name = model.replace('\\', "/");
    let name = name.trim_start_matches("xmodel/");
    match vcod_common::model::load_bones(&*fs, name) {
        Ok(bones) => physics::VehicleModel::from_bones(&bones),
        Err(e) => {
            log::warn!("vehicle model {name}: {e}");
            physics::VehicleModel::default()
        }
    }
}

/// Every linked vehicle's model collision as a locational trace meets it
/// now: the surfaces carried by the bones the controls pose (the turret's
/// with the turret), at the vehicle's origin and angles. The engine poses
/// the DObj just before it clips it (`coduo_lnxded` 0x809a72a, vmMain 15),
/// and the vehicle's contents 0xa00000 meet both the bullet and the blast
/// masks (docs/research/coduo-vehicles.md 13). A driven one is owned by its
/// driver.
pub fn placed_models(host: &mut GameHost, cx: &mut Cx) -> Vec<crate::game::combat::PlacedModel> {
    let mut out = Vec::new();
    for id in host.vehicles.in_slot_order() {
        // The entity clip skips an entity whose `r.contents` meet none of
        // the trace's mask (`coduo_lnxded` 0x809a72a): a wreck's
        // `setContents(0)` before its blast (coduo-vehicles.md 15.2).
        if host
            .ents
            .get(id)
            .is_none_or(|e| e.unlinked || e.link.as_ref().is_some_and(|l| l.contents == 0))
        {
            continue;
        }
        let Some(posed) = seat::posed(host, cx, id) else {
            continue;
        };
        let (tris, surfs) = posed.collision();
        if tris.is_empty() {
            continue;
        }
        let owner = host.vehicles.live[&id].seats[1].map(|s| s as u32);
        let model = seat::model_name(host, cx, id);
        let bounds = host
            .model_bounds_of(&model)
            .map(|(lo, hi)| (glam::Vec3::from(lo), glam::Vec3::from(hi)));
        out.push(crate::game::combat::PlacedModel {
            id,
            origin: posed.origin,
            axis: glam::Mat3::from_quat(posed.rot),
            tris: std::sync::Arc::from(tris),
            surfs: std::sync::Arc::from(surfs),
            owner,
            takes_damage: true,
            body: false,
            bounds,
        });
    }
    out
}

/// Files the vehicle in the area tree with its contents and its box, where
/// a blast's walk and `verifyPosition` find it: with a collmap a brush
/// model's link of its bounds (a cube of their radius once turned,
/// `SV_LinkEntity` `coduo_lnxded` 0x8099dee), else the file's box.
pub fn link(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    use crate::game::entity::LinkKind;
    let Some(v) = host.vehicles.live.get(&id) else {
        return;
    };
    let info = host.vehicles.info_of(v);
    let (kind, (mins, maxs)) = match v
        .collmap
        .and_then(|n| host.model_bounds.get(n as usize).copied())
    {
        Some(b) => (LinkKind::Vehicle, b),
        None => (LinkKind::Item, (info.mins, info.maxs)),
    };
    let shape = crate::game::entity::LinkShape {
        kind,
        contents: v.contents,
        mins,
        maxs,
    };
    if host.ents.get(id).is_some_and(|e| e.unlinked) {
        if let Some(e) = host.ents.get_mut(id) {
            e.link = Some(shape);
        }
        return;
    }
    host.link_shaped(cx, id, shape);
}

/// Puts the vehicle's copy of its collmap's brushes where it stands, under
/// its contents, naming it on their hits: what a player and another
/// vehicle meet of it. One inline model serves every vehicle of its model
/// (`VEH_SetupCollmap` 0x7d3e2), each `SetBrushModel`ed onto its own
/// entity, so each clips where its own vehicle is; the collmap entity
/// itself never links.
pub fn pose_collmap(host: &GameHost, id: EntId) {
    let (Some(v), Some(w)) = (host.vehicles.live.get(&id), host.world.as_ref()) else {
        return;
    };
    let Some(n) = v.collmap else {
        return;
    };
    let n = n as usize;
    w.collision.set_model_linked(n, false);
    let linked = host.ents.get(id).is_some_and(|e| !e.unlinked);
    w.collision.set_model_instance(
        id.0,
        n,
        linked.then(|| {
            (
                v.pose.origin.into(),
                v.pose.angles.into(),
                v.contents as u32,
            )
        }),
    );
}

/// `G_SetupScrVehicles` (0x8ab36) after the BSP pass: each vehicle in slot
/// order takes its collmap.
pub fn pair_collmaps(host: &mut GameHost, cx: &mut Cx) {
    for id in host.vehicles.in_slot_order() {
        pair_collmap(host, cx, id);
    }
}

/// `VEH_SetupCollmap` (0x7d3e2): the first in-use `script_vehicle_collmap`
/// by entity number whose `targetname` is the vehicle's model name, case
/// folded. Its `*N` goes onto the vehicle (`s.index`, `SetBrushModel`), and
/// the vehicle's contents become [`CONTENTS_COLLMAP`].
pub fn pair_collmap(host: &mut GameHost, cx: &mut Cx, id: EntId) {
    let model = field_string(host, cx, id, "model");
    let ids: Vec<EntId> = host.ents.iter_inuse().map(|(e, _)| e).collect();
    for e in ids {
        if field_string(host, cx, e, "classname") != "script_vehicle_collmap" {
            continue;
        }
        if !field_string(host, cx, e, "targetname").eq_ignore_ascii_case(&model) {
            continue;
        }
        let n = host.ents.get(e).and_then(|g| g.brush_model).unwrap_or(0);
        if n == 0 {
            log::warn!("WARNING: Cannot use empty vehicle collmap for [{model}]");
            return;
        }
        if let Some(v) = host.vehicles.live.get_mut(&id) {
            v.collmap = Some(n);
            v.contents = CONTENTS_COLLMAP;
        }
        pose_collmap(host, id);
        link(host, cx, id);
        return;
    }
}

/// `G_FreeVehicle` (0x8ad7d) ahead of the vehicle's own `G_FreeEntity`:
/// the blend entities go first, in order, then the slot.
pub fn free(host: &mut GameHost, id: EntId) {
    let Some(v) = host.vehicles.live.remove(&id) else {
        return;
    };
    host.vehicles.slots[v.slot] = None;
    if let (Some(n), Some(w)) = (v.collmap, host.world.as_ref()) {
        w.collision.set_model_instance(id.0, n as usize, None);
    }
    for b in v.blends.into_iter().flatten() {
        host.free_entity(b);
    }
}

/// The vehicle entity's own fields on the wire (0x7c935, the think's
/// writes; docs/research/coduo-vehicles.md 12). Its pose rides `pos` and
/// `apos` at `TR_INTERPOLATE`.
pub fn write_wire(host: &GameHost, id: EntId, model: i32, set: &mut dyn FnMut(&str, i32)) {
    let Some(v) = host.vehicles.live.get(&id) else {
        return;
    };
    let info = host.vehicles.info_of(v);
    let f = |x: f32| x.to_bits() as i32;
    set(
        "eType",
        crate::game::wire::ET_RAW | vcod_common::net::flags::ET_VEHICLE_V22,
    );
    set(
        "eFlags",
        if v.wire.driven_flag {
            vcod_common::net::flags::EF_IN_VEHICLE_V22
        } else {
            0
        },
    );
    set("pos.trType", TR_INTERPOLATE);
    set("apos.trType", TR_INTERPOLATE);
    set("xmodel", model);
    if let Some(n) = v.collmap {
        set("index", n as i32);
        set("solid", SOLID_BMODEL);
    }
    set("surfType", info.kind);
    set("weapon", v.wire.weapon);
    set("attackerEntityNum", v.slot as i32);
    set("otherEntityNum", v.wire.other);
    set("animMovetype", v.wire.anim_movetype);
    set("origin2[0]", v.wire.origin2[0] as i32);
    set("origin2[1]", f(v.wire.origin2[1]));
    set("origin2[2]", v.wire.origin2[2] as i32);
    for (i, a) in v.wire.angles2.iter().enumerate() {
        set(&format!("angles2[{i}]"), f(*a));
    }
    set("leanf", f(v.wire.leanf));
    set("time", v.wire.time);
    set("time2", v.wire.time2);
    set("fTorsoPitch", f(v.wire.torso_pitch));
    set("fWaistPitch", f(v.wire.waist_pitch));
    set("iHeadIcon", v.wire.head_icon[0]);
    set("iHeadIconTeam", v.wire.head_icon[1]);
    set("loopSound", v.wire.loop_sound);
}

/// `TR_INTERPOLATE`, both trajectories' type on a vehicle.
const TR_INTERPOLATE: i32 = 1;
/// `trap_SetBrushModel`'s packed `solid`.
const SOLID_BMODEL: i32 = 0x00ff_ffff;

/// `r.svFlags`-free link state: `unlinkFromWorld` and `linkIntoWorld`.
pub fn set_linked(host: &mut GameHost, cx: &mut Cx, id: EntId, linked: bool) {
    if let Some(e) = host.ents.get_mut(id).filter(|e| e.client.is_none()) {
        e.unlinked = !linked;
    }
    if linked {
        link(host, cx, id);
    } else {
        host.area.unlink(id.0);
    }
    pose_collmap(host, id);
}

/// The weapon an inflictor carries in `s.weapon`, which `G_DamageClient`
/// hands the damage callback: a vehicle's last gun, a player's weapon.
pub fn inflictor_weapon(host: &GameHost, id: Option<EntId>) -> usize {
    let Some(id) = id else {
        return 0;
    };
    if let Some(v) = host.vehicles.live.get(&id) {
        return v.wire.weapon as usize;
    }
    if host.ents.get(id).is_some_and(|e| e.client.is_some()) {
        return host
            .client_weapons
            .get(id.0 as usize)
            .map_or(0, |w| w.current as usize);
    }
    0
}

/// `G_IsVehicleImmune` (0x8b893): bullets need `bulletDamage`, grenades
/// `grenadeDamage`, projectiles and artillery `projectileDamage`; water,
/// explosives and collisions always land; every other means never does.
pub fn immune(info: &VehicleInfo, mod_: &str) -> bool {
    match mod_ {
        "MOD_PISTOL_BULLET" | "MOD_RIFLE_BULLET" => !info.bullet_damage,
        "MOD_GRENADE" | "MOD_GRENADE_SPLASH" => !info.grenade_damage,
        "MOD_PROJECTILE" | "MOD_PROJECTILE_SPLASH" | "MOD_ARTILLERY" | "MOD_ARTILLERY_SPLASH" => {
            !info.projectile_damage
        }
        "MOD_WATER" | "MOD_EXPLOSIVE" | "MOD_COLLISION" => false,
        _ => true,
    }
}

/// `Scr_Vehicle_DamageScale` (0x8ccad): the hit's direction off the hull
/// picks the front, side or rear scale; splash blends two by the angle and
/// takes 0.7 of it; bullets also take `dmgScaleBullet`. A grenade's splash
/// takes its inflictor's weapon's `vehiclescale` when that is positive
/// (`inflictor_scale`), else 1. `origin` and
/// `angles` are the record's physics pose (`rec+0xb8`, `rec+0xd0`), the
/// direction measured on `AnglesToAxis` of the whole angles; `maxs` is
/// `r.maxs`.
pub fn damage_scale(
    info: &VehicleInfo,
    origin: [f32; 3],
    angles: [f32; 3],
    maxs: [f32; 3],
    point: [f32; 3],
    mod_: &str,
    inflictor_scale: f32,
) -> f32 {
    let b = match mod_ {
        "MOD_PISTOL_BULLET" | "MOD_RIFLE_BULLET" => 0.01 * info.dmg_scale_bullet,
        _ => 1.0,
    };
    let (dx, dy) = (point[0] - origin[0], point[1] - origin[1]);
    let len = (dx * dx + dy * dy).sqrt();
    let v = if len > 0.0 {
        [dx / len, dy / len]
    } else {
        [0.0, 0.0]
    };
    let [f, l, _] = vcod_common::x87::angles_to_axis(angles.into()).map(|v| v.to_array());
    let fwd = v[0] * f[0] + v[1] * f[1];
    // The axis' second row is the left; only its size is read.
    let right = v[0] * l[0] + v[1] * l[1];
    let forward_wins = fwd.abs() >= right.abs();
    let front_or_back = |d: f32| {
        0.01 * if d < 0.0 {
            info.dmg_scale_behind
        } else {
            info.dmg_scale_front
        }
    };
    match mod_ {
        "MOD_GRENADE_SPLASH" => {
            let b = if inflictor_scale > 0.0 {
                inflictor_scale
            } else {
                1.0
            };
            let under = point[2] - 10.0 < origin[2] + 0.5 * maxs[2]
                && (point[0] - origin[0]).powi(2)
                    + (point[1] - origin[1]).powi(2)
                    + (point[2] - origin[2]).powi(2)
                    < (0.8 * maxs[1]).powi(2);
            if under {
                0.01 * info.dmg_scale_under * b
            } else {
                b
            }
        }
        "MOD_PROJECTILE_SPLASH"
        | "MOD_MORTAR_SPLASH"
        | "MOD_DYNAMITE_SPLASH"
        | "MOD_ARTILLERY_SPLASH" => {
            let (d, a, c) = if forward_wins {
                (fwd, front_or_back(fwd), 0.01 * info.dmg_scale_side)
            } else {
                (right, 0.01 * info.dmg_scale_side, front_or_back(fwd))
            };
            0.7 * ((1.0 - d.abs()) * c + d.abs() * a) * b
        }
        _ => {
            if !forward_wins {
                0.01 * info.dmg_scale_side * b
            } else if fwd >= 0.0 {
                0.01 * info.dmg_scale_front * b
            } else if -fwd >= 20f32.to_radians().cos() {
                0.01 * info.dmg_scale_behind * b
            } else {
                0.01 * info.dmg_scale_side * b
            }
        }
    }
}

/// `G_VehicleOccupantRadiusDamageScale` (0x8ba86): an occupant takes a
/// tenth of a blast, a fifth in artillery.
pub fn occupant_radius_scale(kind: i32) -> f32 {
    if kind == info::KIND_ARTILLERY {
        0.2
    } else {
        0.1
    }
}

/// `G_Damage` (0x4fedb) as `doDamage` reaches it for a vehicle or a
/// player: a vehicle takes its own branch, a player its damage callback (an
/// occupant's blast scaled, a tank driver untouchable but by his own tank).
/// `builtins::uo::do_damage` sends every other entity down the generic arm.
/// A missing attacker is the world's entity.
#[allow(clippy::too_many_arguments)]
pub fn g_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    target: EntId,
    inflictor: Option<EntId>,
    attacker: Option<EntId>,
    point: [f32; 3],
    damage: i32,
    dflags: i32,
    mod_: &str,
) {
    // A player's own origin: a rider the die walk lifted reads unlifted.
    let origin = match host.vehicles.lifted.get(&(target.0 as usize)) {
        Some(o) if host.ents.get(target).is_some_and(|e| e.client.is_some()) => *o,
        _ => crate::game::item::origin_of(host, cx, target),
    };
    let d = glam::Vec3::from(origin) - glam::Vec3::from(point);
    let dir = d.normalize_or_zero().to_array();
    if host.vehicles.live.contains_key(&target) {
        for (event, args) in vehicle_damage(
            host, cx, target, inflictor, attacker, dir, point, damage, dflags, mod_,
        ) {
            let event = cx.intern_folded(event);
            cx.notify(vcod_gsc::Target::Entity(target), event, args);
        }
        // The die callback runs inside `G_Damage`: the riders' probe hits
        // before the builtin returns.
        if let Some(i) = host.vehicles.deaths.iter().position(|d| d.id == target) {
            let death = host.vehicles.deaths.remove(i);
            die::begin(host, death);
            die_next(host, cx);
        }
        return;
    }
    if host.ents.get(target).is_none_or(|e| e.client.is_none()) {
        return;
    }
    let slot = target.0 as usize;
    if !host.client_vitals.get(slot).is_some_and(|v| v.takedamage) {
        return;
    }
    let Some(damage) = host
        .vehicles
        .occupant_damage(slot, inflictor, damage, dflags)
    else {
        return;
    };
    let weapon = inflictor_weapon(host, inflictor);
    let weapon = host.defs.weapon_name(weapon).unwrap_or("none").to_string();
    let world = host.ents.world(cx);
    let args = vec![
        inflictor.map_or(Value::Undefined, Value::Entity),
        Value::Entity(attacker.unwrap_or(world)),
        Value::Int(damage),
        Value::Int(dflags),
        Value::String(cx.intern_exact(mod_)),
        Value::String(cx.intern_exact(&weapon)),
        Value::Vector(point),
        Value::Vector(dir),
        Value::String(cx.intern_exact("none")),
    ];
    let callback = cx.func_ref(
        crate::game::script::CALLBACK_SETUP,
        "CodeCallback_PlayerDamage",
    );
    cx.spawn(callback, Some(vcod_gsc::Target::Entity(target)), args);
}

/// The walk's next rider callback, spawned to run before the calling
/// builtin goes on; its return comes back through `GameHost::spawn_returned`.
pub fn die_next(host: &mut GameHost, cx: &mut Cx) {
    let next = die::step(host, cx);
    for (on, event, args) in std::mem::take(&mut host.vehicles.die_notifies) {
        let event = cx.intern_folded(event);
        cx.notify(vcod_gsc::Target::Entity(on), event, args);
    }
    if let Some((victim, args)) = next {
        let callback = cx.func_ref(
            crate::game::script::CALLBACK_SETUP,
            "CodeCallback_PlayerDamage",
        );
        cx.spawn_then(
            callback,
            Some(vcod_gsc::Target::Entity(victim)),
            args,
            die::DIE_TOKEN,
        );
    }
}

/// `G_Damage`'s vehicle branch: immunity, a blast under 100 dropped, the
/// direction's scale, then the generic tail: `health` down, `damage` with
/// six arguments, and at 0 `death` and `Scr_Vehicle_Die`, queued: a builtin
/// walks it at once ([`die_next`]), the frame's other paths at
/// `ScriptRuntime::vehicle_deaths`.
#[allow(clippy::too_many_arguments)]
pub fn vehicle_damage(
    host: &mut GameHost,
    cx: &mut Cx,
    id: EntId,
    inflictor: Option<EntId>,
    attacker: Option<EntId>,
    dir: [f32; 3],
    point: [f32; 3],
    damage: i32,
    dflags: i32,
    mod_: &str,
) -> Vec<(&'static str, Vec<Value>)> {
    let Some(v) = host.vehicles.live.get(&id) else {
        return Vec::new();
    };
    let info = host.vehicles.info_of(v).clone();
    if immune(&info, mod_) {
        return Vec::new();
    }
    if dflags & crate::game::combat::DFLAG_RADIUS != 0 && damage < 100 {
        return Vec::new();
    }
    let origin = crate::game::item::origin_of(host, cx, id);
    let (pose, maxs) = host
        .vehicles
        .live
        .get(&id)
        .map_or(((origin, [0.0; 3]), info.maxs), |v| {
            let maxs = v
                .collmap
                .and_then(|n| host.model_bounds.get(n as usize))
                .map_or(info.maxs, |b| b.1);
            ((v.phys.s.origin.into(), v.phys.s.angles.into()), maxs)
        });
    // `s.weapon` of the inflictor: a thrown grenade's own weapon file.
    let inflictor_scale = inflictor
        .and_then(|i| host.missiles.weapon_of(i))
        .and_then(|w| host.weapons.get(w as usize))
        .map_or(0.0, |d| d.vehicle_scale);
    let scale = damage_scale(&info, pose.0, pose.1, maxs, point, mod_, inflictor_scale);
    // The f32 scale times the damage, `fistp` under a 0xc00 control word
    // (0x4ffbf..0x4ffd1): truncated; the generic arm then lifts 0 to 1.
    let damage = ((f64::from(scale) * f64::from(damage)).trunc() as i32).max(1);
    let health = crate::game::builtins::vehicle::health_of(host, cx, id);
    let left = health - damage;
    let world = host.ents.world(cx);
    let attacker = attacker.unwrap_or(world);
    let inflictor = inflictor.unwrap_or(world);
    set_field(host, cx, id, "health", Value::Int(left.max(-999)));
    let mut notifies = vec![(
        "damage",
        vec![
            Value::Int(damage),
            Value::Entity(attacker),
            Value::Vector(dir),
            Value::Vector(point),
            Value::String(cx.intern_exact(mod_)),
            Value::Entity(inflictor),
        ],
    )];
    if left < 1 {
        notifies.push(("death", vec![Value::Entity(attacker)]));
        host.vehicles.deaths.push(die::Death { id, attacker, dir });
    } else {
        // `Scr_Vehicle_Pain` (0x8d1ca): the attacker holding a grenade or a
        // launcher, `MOD_EXPLOSIVE`, or anything over 200 but a collision
        // jolts the hull.
        let held = inflictor_weapon(host, Some(attacker));
        let heavy = host
            .weapons
            .get(held)
            .is_some_and(|d| matches!(d.weapon_type.as_str(), "grenade" | "projectile"));
        if heavy || mod_ == "MOD_EXPLOSIVE" || (damage > 200 && mod_ != "MOD_COLLISION") {
            let now = host.level_time_ms;
            if let Some(v) = host.vehicles.live.get_mut(&id) {
                let d = glam::Vec3::from(dir);
                v.phys.jolt(&info, d, 1.0, 0.0, 0.0, now);
            }
        }
        // Then the driver (`otherEntityNum`, a connected client) takes the
        // damage as feedback alone: `damage_blood` up by it and
        // `damage_from` the vehicle's origin less the point, unnormalised,
        // or no direction when the vehicle is its own attacker. The op skips
        // a slot with no sim.
        let driver = host.vehicles.live.get(&id).map_or(-1, |v| v.wire.other);
        if (0..host.client_bodies.len() as i32).contains(&driver) {
            let from = (attacker != id).then(|| std::array::from_fn(|i| origin[i] - point[i]));
            host.client_sim_ops.push((
                driver as usize,
                crate::game::host::SimOp::VehiclePain { damage, from },
            ));
        }
    }
    notifies
}

/// `VEH_GetMinsMaxs`' box about the vehicle's origin, yaw only.
fn vehicle_box(host: &GameHost, id: EntId, origin: [f32; 3]) -> Option<([f32; 3], [f32; 3])> {
    let v = host.vehicles.live.get(&id)?;
    let info = host.vehicles.info_of(v);
    Some((
        std::array::from_fn(|i| origin[i] + info.mins[i]),
        std::array::from_fn(|i| origin[i] + info.maxs[i]),
    ))
}

fn boxes_touch(a: ([f32; 3], [f32; 3]), b: ([f32; 3], [f32; 3])) -> bool {
    (0..3).all(|i| a.0[i] <= b.1[i] && b.0[i] <= a.1[i])
}

/// `verifyPosition` on a vehicle (0x692e8): no player and no other vehicle
/// in its box.
pub fn position_clear(host: &mut GameHost, cx: &mut Cx, id: EntId) -> bool {
    let origin = crate::game::item::origin_of(host, cx, id);
    let Some(me) = vehicle_box(host, id, origin) else {
        return true;
    };
    for body in host.client_bodies.iter().flatten() {
        let o = body.origin;
        let b = ((o + body.mins).to_array(), (o + body.maxs).to_array());
        if boxes_touch(me, b) {
            return false;
        }
    }
    let others: Vec<EntId> = host
        .vehicles
        .live
        .keys()
        .copied()
        .filter(|&o| o != id)
        .collect();
    for o in others {
        if host.ents.get(o).is_some_and(|e| e.unlinked) {
            continue;
        }
        let at = crate::game::item::origin_of(host, cx, o);
        if vehicle_box(host, o, at).is_some_and(|b| boxes_touch(me, b)) {
            return false;
        }
    }
    true
}

/// `clearVehiclePosition`'s victims (0x6950e): dropped items in the box.
pub fn clutter_in_box(host: &mut GameHost, cx: &mut Cx, id: EntId) -> Vec<EntId> {
    let origin = crate::game::item::origin_of(host, cx, id);
    let Some(me) = vehicle_box(host, id, origin) else {
        return Vec::new();
    };
    let items: Vec<EntId> = host
        .ents
        .iter_inuse()
        .filter(|(_, e)| e.item.is_some_and(|i| !i.taken))
        .map(|(e, _)| e)
        .collect();
    items
        .into_iter()
        .filter(|&e| {
            let o = crate::game::item::origin_of(host, cx, e);
            boxes_touch(me, (o, o))
        })
        .collect()
}

/// `VEH_FindValidDismountSpot` (0x88dee), the moving vehicles' branch:
/// a point off the side the vehicle is not driving toward (or the side the
/// player is on), along the hull's tilted axes, then a 20-unit grid about
/// it at two heights 32 apart, four passes flipping forward and side. A spot must hold no body, have ground
/// within 256 below and be in clear line from the vehicle's middle.
/// `velocity` is the vehicle's, `from` the player's origin, `rider` the
/// player, whose own body the search passes.
pub fn dismount_spot(
    host: &GameHost,
    id: EntId,
    from: [f32; 3],
    (mins, maxs): ([f32; 3], [f32; 3]),
    velocity: Option<[f32; 3]>,
    rider: Option<usize>,
) -> Option<[f32; 3]> {
    use glam::Vec3;
    let v = host.vehicles.live.get(&id)?;
    let info = host.vehicles.info_of(v);
    let world = host.world.as_ref()?;
    let col = &world.collision;
    let origin = Vec3::from(v.pose.origin);
    // `AngleVectors` of the record's whole angles (`rec+0xd0`): on a
    // slope the spot tilts with the hull.
    let [fwd, right, _] = vcod_common::x87::angle_vectors(v.pose.angles.into());
    let vel = Vec3::from(velocity.unwrap_or([0.0; 3]));
    let mut d = if vel.length() > 0.0 {
        -vel
    } else {
        Vec3::from(from) - origin
    };
    d.z = 0.0;
    if d.length() < 1e-5 {
        d = Vec3::X;
    }
    let (mut rr, mut ff) = (right.dot(d), fwd.dot(d));
    let side = 60.0 + info.maxs[0] + maxs[1];
    let len = maxs[0] + info.collision_front_dist.max(info.collision_back_dist) + 100.0;
    if ff.abs() < rr.abs() {
        let k = side / rr.abs();
        rr *= k;
        ff *= k;
        ff = if ff < 0.0 {
            ff.min(-maxs[0])
        } else {
            ff.max(maxs[0])
        };
    } else {
        let k = len / ff.abs();
        rr *= k;
        ff *= k;
    }
    rr = rr.clamp(-side, side);
    ff = ff.clamp(-len, len);
    let (pmins, pmaxs) = (Vec3::from(mins), Vec3::from(maxs));
    let body_at = |p: Vec3| {
        host.client_bodies
            .iter()
            .flatten()
            .filter(|b| Some(b.slot) != rider)
            .any(|b| {
                let (lo, hi) = (b.origin + b.mins, b.origin + b.maxs);
                (0..3).all(|i| p[i] + pmins[i] <= hi[i] && lo[i] <= p[i] + pmaxs[i])
            })
    };
    let solid = vcod_common::collision::MASK_PLAYERSOLID;
    let ok = |p: Vec3| {
        if body_at(p) {
            return false;
        }
        let drop = col.move_trace(p, p - Vec3::Z * 256.0, pmins, pmaxs, solid);

        if drop.startsolid || drop.allsolid || drop.fraction >= 1.0 {
            return false;
        }
        let lift = Vec3::Z * 30.0;
        let short = Vec3::new(pmaxs.x, pmaxs.y, pmaxs.z - 30.0);
        let tr = match v.collmap {
            Some(_) => col.box_trace_except(
                origin + lift,
                p + lift,
                pmins,
                short,
                solid,
                vcod_common::collision::Pusher::Copy(id.0),
            ),
            None => col.move_trace(origin + lift, p + lift, pmins, short, solid),
        };
        !tr.startsolid && (tr.fraction >= 1.0 || col.entity_num(&tr) == id.0)
    };
    for k in 0..4 {
        let base = origin + Vec3::Z * 4.0 + fwd * ff + right * rr;
        let mut p = base;
        for dx in (-80..=60).step_by(20) {
            for dy in (-80..=60).step_by(20) {
                for h in [0.0, 32.0] {
                    let q = p + Vec3::Z * h;
                    if ok(q) {
                        return Some(q.to_array());
                    }
                }
                p = base + Vec3::new(dx as f32, dy as f32, 0.0);
            }
        }
        if k % 2 == 0 {
            ff = -ff;
        } else {
            rr = -rr;
        }
    }
    None
}

/// The vehicle a sound-blend entity belongs to and its blend state, when it
/// is linked.
pub fn sound_blend(host: &GameHost, id: EntId) -> Option<(EntId, (i32, i32, f32))> {
    host.vehicles.live.iter().find_map(|(&veh, v)| {
        let k = v.blends.iter().position(|b| *b == Some(id))?;
        Some((veh, v.sound_blends[k]?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::combat::DFLAG_RADIUS;
    use info::{KIND_ARTILLERY, KIND_FOUR_WHEEL, KIND_TANK};

    const TANK: EntId = EntId(186, 0);
    const SHOOTER: EntId = EntId(1, 0);

    #[test]
    fn a_tank_driver_takes_only_what_his_own_tank_inflicts() {
        assert_eq!(rider_damage(KIND_TANK, 1, TANK, Some(SHOOTER), 40, 0), None);
        assert_eq!(
            rider_damage(KIND_TANK, 1, TANK, Some(SHOOTER), 500, DFLAG_RADIUS),
            None
        );
        assert_eq!(
            rider_damage(KIND_TANK, 1, TANK, Some(TANK), 40, 0),
            Some(40)
        );
        assert_eq!(rider_damage(KIND_TANK, 1, TANK, None, 40, 0), Some(40));
        // The gunner and a jeep's driver are in the open.
        assert_eq!(
            rider_damage(KIND_TANK, 2, TANK, Some(SHOOTER), 40, 0),
            Some(40)
        );
        assert_eq!(
            rider_damage(KIND_FOUR_WHEEL, 1, TANK, Some(SHOOTER), 40, 0),
            Some(40)
        );
    }

    #[test]
    fn a_rider_takes_a_truncated_tenth_of_a_blast_a_fifth_in_artillery() {
        let blast = |kind, damage| rider_damage(kind, 2, TANK, Some(SHOOTER), damage, DFLAG_RADIUS);
        assert_eq!(blast(KIND_TANK, 100), Some(10));
        // 0.1f is a hair over a tenth: 30 lands on 3, 19 truncates to 1.
        assert_eq!(blast(KIND_TANK, 30), Some(3));
        assert_eq!(blast(KIND_FOUR_WHEEL, 19), Some(1));
        assert_eq!(blast(KIND_FOUR_WHEEL, 9), Some(0));
        assert_eq!(blast(KIND_ARTILLERY, 99), Some(19));
    }
}
