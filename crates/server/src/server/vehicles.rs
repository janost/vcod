//! UO vehicle glue: seats, vehicle fire, crushing and riders.

use super::*;

/// The seat changes the host made this cmd, onto the riders' sims
/// (`crate::game::vehicle::seat::SeatOp`), and the script's copy of where
/// each one now stands.
pub(super) fn apply_seat_ops(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    use crate::game::vehicle::seat::SeatOp;
    for op in std::mem::take(&mut rt.host.vehicles.seat_ops) {
        let slot = op.slot();
        let Some(sim) = clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        else {
            continue;
        };
        match op {
            SeatOp::Enter {
                vehicle,
                seat,
                kind,
                origin,
                view,
                angles,
                link_offset,
                ..
            } => sim.enter_seat(
                crate::spectate::Seat {
                    vehicle,
                    pos: seat,
                    kind,
                    // `VEH_LinkPlayer` leaves `vehMotion` as the last ride
                    // or the spawn's memset left it; the end frame sets it.
                    motion: sim.veh_motion,
                    angles,
                    hull_view: true,
                    overheated: false,
                    link_offset,
                },
                origin,
                view,
            ),
            SeatOp::Exit {
                origin, velocity, ..
            } => sim.leave_seat(origin, velocity),
        }
        rt.set_client_origin(slot, sim.origin());
    }
}

/// What [`vehicle_fire`] traces and delivers with.
pub(super) struct VehicleFireCtx<'a> {
    pub(super) world: Option<&'a World>,
    pub(super) hitlocs: &'a crate::game::combat::HitLocTable,
    pub(super) fs: Option<&'a vcod_common::pk3::Pk3Fs>,
    pub(super) anims: Option<&'a vcod_common::animtree::PlayerAnims>,
    pub(super) rigs: &'a mut crate::game::hitrig::HitRigs,
    pub(super) weapons: &'a crate::weapons::WeaponTable,
    pub(super) rng: &'a mut u64,
    pub(super) now_ms: i32,
}

/// `GameHost::dobj_posed`, which an engine trace stamps its bodies in
/// (`combat::DobjStamps`); empty on a host with no script.
pub(super) fn dobj_stamps(
    script: Option<&mut script::ScriptRuntime>,
) -> &mut [crate::game::combat::DobjStamps] {
    match script {
        Some(rt) => &mut rt.host.dobj_posed,
        None => &mut [],
    }
}

/// The rounds `fireTurret`, `fireAltTurret` and `fireGunner` asked for
/// (`crate::game::vehicle::fire`): aimed off the posed vehicle, traced from
/// its muzzle against the bodies, charged to the rider with the vehicle as
/// the inflictor. A vehicle round leaves no impact of its own: the client
/// traces it (event parm bit 7).
pub(super) fn vehicle_fire(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    ctx: VehicleFireCtx,
) {
    let ops = std::mem::take(&mut rt.host.vehicles.fire_ops);
    if ops.is_empty() {
        return;
    }
    let VehicleFireCtx {
        world,
        hitlocs,
        fs,
        anims,
        rigs,
        weapons,
        rng,
        now_ms,
    } = ctx;
    for (id, gun) in ops {
        let Some(shot) = rt.with_host(|h, cx| crate::game::vehicle::fire::prepare(h, cx, id, gun))
        else {
            continue;
        };
        // The fire call spawned a shell gun's round itself
        // (`vehicle::fire::launch_shell`).
        if shot.projectile {
            continue;
        }
        let name = crate::items::item_name(rt.host.defs, shot.weapon).unwrap_or_default();
        let falloff = rt
            .host
            .weapons
            .get(shot.weapon)
            .map(crate::game::combat::Falloff::of);
        // `CMD_VEH_FireTurret` and kin pass the vehicle (0x8ee02): the
        // round skips it and its riders, owned by it.
        let pass = crate::game::combat::Pass {
            ent: id.0,
            owner: rt
                .host
                .vehicles
                .live
                .get(&id)
                .and_then(|v| v.seats[1])
                .map(|s| s as u32),
        };
        let models = rt.shot_models();
        let effects = {
            let sims: Vec<(usize, &ClientSim)> = clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                .collect();
            let mut bones = crate::game::combat::BoneTraceCtx::of(
                fs,
                anims,
                &mut *rigs,
                now_ms,
                &mut rt.host.dobj_posed,
                vcod_common::patch::Arith::for_protocol(rt.host.defs.proto),
            );
            crate::game::combat::bullet_fire_to(
                shot.attacker,
                pass,
                shot.muzzle,
                shot.end,
                shot.dir,
                shot.damage,
                shot.rifle_bullet,
                falloff,
                name,
                &sims,
                &rt.host.missiles.shot_targets(),
                world.map(|w| &w.collision),
                &models,
                hitlocs,
                bones.as_mut(),
            )
        };
        let level_ms = rt.host.level_time_ms;
        for &id in &effects.struck {
            rt.host.missiles.kill(id, level_ms);
        }
        let effects = effects.effects;
        let effects: Vec<Effect> = effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::Hit(mut h) => {
                    h.inflictor = Some(id);
                    Some(Effect::Hit(h))
                }
                Effect::EntityHit(mut h) => {
                    h.inflictor = Some(id);
                    Some(Effect::EntityHit(h))
                }
                Effect::TriggerSweep(t) => Some(Effect::TriggerSweep(t)),
                Effect::Impact(_) => None,
            })
            .collect();
        let struck = effects.iter().any(|e| matches!(e, Effect::Hit(_)));
        rt.with_host(|h, _| crate::game::vehicle::fire::landed(h, &shot, struck));
        deliver_turret_rounds(
            clients,
            rt,
            effects,
            shot.attacker,
            anims,
            weapons,
            rng,
            now_ms,
        );
    }
}

/// `G_VehicleClientThink` (`crate::game::vehicle::think::run_driven`),
/// handed each rider's last cmd and view; then what this frame's thinks,
/// the undriven ones' in the entity pass included, left for the riders:
/// dead riders out, the hull's angles for the next cmds' clamps,
/// `vehMotion`, the hull-view bit and the gunner's overheat.
pub(super) fn vehicle_frame(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    if rt.host.vehicles.live.is_empty() {
        return;
    }
    let sims: Vec<(usize, &crate::spectate::ClientSim)> = clients
        .iter()
        .enumerate()
        .filter_map(|(slot, c)| Some((slot, c.as_ref()?.sim.as_ref()?)))
        .collect();
    let inputs = crate::game::vehicle::think::rider_inputs(&rt.host, &sims);
    rt.vehicle_deaths();
    rt.with_host(|host, cx| crate::game::vehicle::think::run_driven(host, cx, &inputs));
    rt.vehicle_deaths();
    apply_seat_ops(clients, rt);
    crush(clients, rt);
    for (slot, c) in clients.iter_mut().enumerate() {
        let Some(c) = c.as_mut() else { continue };
        let Some(seat) = c.sim.as_mut().and_then(|s| s.seat.as_mut()) else {
            continue;
        };
        let Some(v) = rt
            .host
            .vehicles
            .live
            .iter()
            .find(|(id, _)| id.0 == seat.vehicle)
            .map(|(_, v)| v)
        else {
            continue;
        };
        seat.angles = v.pose.angles;
        seat.motion = crate::game::vehicle::seat::motion(v.forward_speed);
        let motion = seat.motion;
        // The driver's think sets the bit, and clears it on a tank or
        // under `upmove` (0x87cd3).
        if seat.pos == 1 {
            seat.hull_view = v.hull_view;
        }
        seat.overheated = rt.host.vehicles.overheated_gunners.contains(&slot);
        if let Some(sim) = c.sim.as_mut() {
            sim.veh_motion = motion;
        }
    }
}

/// `VEH_PlayerCollision` (game.mp.uo 0x8213f) for every body a vehicle's
/// probes struck this frame: knocked away along the vehicle's motion, then
/// [`run_over`].
fn crush(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    let now = rt.host.level_time_ms;
    for (veh, strike) in std::mem::take(&mut rt.host.vehicles.strikes) {
        let slot = strike.entity as usize;
        let Some(sim) = clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        else {
            continue;
        };
        // A strike that killed him made him a corpse (`CONTENTS_CORPSE`),
        // out of the later probes' mask (0x2000000).
        if sim.seat.is_some() || sim.dead || rt.client_vitals(slot).health < 1 {
            continue;
        }
        let Some(push) = crate::game::vehicle::physics::player_push(
            strike.vehicle_origin,
            strike.vehicle_velocity,
            sim.ps.origin,
            sim.ps.velocity,
        ) else {
            continue;
        };
        sim.ps.velocity = push.velocity;
        let prone = sim.ps.stance == vcod_common::pmove::Stance::Prone;
        run_over(rt, slot, veh, push, prone, sim.ps.origin, now);
    }
}

/// `VEH_PlayerCollision`'s tail once the push is dealt: at most every
/// 500 ms the body is run over for `150 * min(speed / 400, 1)`, 0 when the
/// vehicle moves away from him (`VEH_PlayerDamage`), then, unless he lies
/// prone (`pm_flags` 1), the `vehicle_hit_person` alias at `at`, `1500 (1 -
/// f)` up, broadcast (`svFlags |= 8`). The 500 ms runs off
/// `client+0x4728`, zero from the spawn, which every strike past it
/// restarts, a 0-damage one too (coduo-vehicles.md 13).
pub(super) fn run_over(
    rt: &mut script::ScriptRuntime,
    slot: usize,
    veh: vcod_gsc::EntId,
    push: crate::game::vehicle::physics::PlayerPush,
    prone: bool,
    at: glam::Vec3,
    now: i32,
) {
    const CRUSH_GAP_MS: i32 = 500;
    let last = rt.host.vehicles.crushed.get(&slot).copied().unwrap_or(0);
    if last >= now - CRUSH_GAP_MS {
        return;
    }
    rt.host.vehicles.crushed.insert(slot, now);
    let Some(hit_person) = rt
        .host
        .vehicles
        .live
        .get(&veh)
        .map(|v| rt.host.vehicles.info_of(v).hit_person_index)
    else {
        return;
    };
    vehicle_player_damage(rt, slot, veh, push.damage, at);
    if !prone && hit_person != 0 {
        rt.with_host(|host, cx| {
            host.add_temp_entity(
                cx,
                crate::game::temp_entity::TempEntity {
                    event: crate::game::script::EV_SOUND_ALIAS,
                    parm: hit_person,
                    surf_type: 0,
                    other: 0,
                    attacker: 0,
                    weapon: 0,
                    client_num: 0,
                    scale: 0,
                    origin: push.sound.to_array(),
                    scope: crate::game::temp_entity::Scope::Broadcast,
                    quake: None,
                    origin2: None,
                },
            )
        });
    }
}

/// `VEH_PlayerDamage(player, vehicle, damage)` (game.mp.uo 0x91ffd):
/// `G_Damage` with the vehicle as the inflictor and its `r.ownerNum` (the
/// driver, 1023 without one: the vehicle itself) as the attacker, no
/// direction, the body's `r.currentOrigin` `at` as the point, dflags 0 and
/// `MOD_CRUSH_JEEP` off a four-wheel (`info+0x40` 1), `MOD_CRUSH_TANK`
/// else.
pub(super) fn vehicle_player_damage(
    rt: &mut script::ScriptRuntime,
    slot: usize,
    veh: vcod_gsc::EntId,
    damage: i32,
    at: glam::Vec3,
) {
    let Some(v) = rt.host.vehicles.live.get(&veh) else {
        return;
    };
    let four_wheel =
        rt.host.vehicles.info_of(v).kind == crate::game::vehicle::info::KIND_FOUR_WHEEL;
    let mod_ = if four_wheel {
        "MOD_CRUSH_JEEP"
    } else {
        "MOD_CRUSH_TANK"
    };
    let weapon = crate::items::item_name(rt.host.defs, v.wire.weapon as usize)
        .unwrap_or("none")
        .to_string();
    match v.seats[1] {
        Some(driver) => {
            let hit = crate::game::combat::Hit {
                victim: slot,
                attacker: driver,
                inflictor: Some(veh),
                damage,
                dflags: 0,
                mod_,
                weapon,
                point: at.into(),
                dir: [0.0; 3],
                hitloc: "none",
            };
            rt.deliver_hits(vec![hit]);
        }
        None => rt.deliver_unowned_hit(
            slot,
            [veh, veh],
            damage,
            0,
            mod_,
            &weapon,
            Some(at.into()),
            Some([0.0; 3]),
        ),
    }
}

/// `G_PlayerVehiclePositionAndBlend` (game.mp.uo 0x9048f) at the end of a
/// rider's `ClientEndFrame`: `ps.origin` and `r.currentOrigin` at its seat
/// tag of the vehicle as posed now, shifted by the seat's offset, and
/// turned with the tag (`r.currentAngles`, the frame a trace poses the body
/// in).
pub(super) fn place_rider(rt: &mut script::ScriptRuntime, slot: usize, sim: &mut ClientSim) {
    let Some(seat) = sim.seat else {
        return;
    };
    let Some((at, steer)) = rt.with_host(|host, cx| {
        let id = host.ents.handle(seat.vehicle)?;
        let at = crate::game::vehicle::seat::posed_as(host, cx, id, true)?.seat(seat.pos)?;
        Some((at, host.vehicles.live.get(&id)?.wire.leanf))
    }) else {
        return;
    };
    sim.seat_steer = steer;
    sim.ps.origin = at.0;
    sim.place_seat_angles(vcod_common::turretpose::quat_angles(at.1));
    rt.set_client_origin(slot, at.0.to_array());
}
