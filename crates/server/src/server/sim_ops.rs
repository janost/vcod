//! The script runtime and the client sims, mirrored both ways each frame.

use super::*;

/// `ClientEvents`' fall damage before `G_Damage`'s location multiplier: the
/// landing pain's percent of `max_health` (`ps.stats[2]`), 1.1 past 99,
/// truncated (player-clip doc 8.8). 1.1's x87 product is exact in `f64`,
/// which is what keeps 25 percent of 100 at 24; 1.5 and UO store the share
/// and the product as floats first (`stored`), which puts 21 percent at 21.
pub(super) fn fall_damage(percent: i32, max_health: i32, stored: bool) -> i32 {
    let share = if percent > 99 {
        f64::from(1.1f32)
    } else {
        f64::from(percent) * f64::from(0.01f32)
    };
    let damage = f64::from(max_health)
        * if stored {
            f64::from(share as f32)
        } else {
            share
        };
    if stored {
        damage as f32 as i32
    } else {
        damage as i32
    }
}

/// What each client holds, from the host onto its sim, and the sim's origin
/// back to script.
pub(super) fn mirror_weapons(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    for (slot, c) in clients.iter_mut().enumerate() {
        if let Some(sim) = c.as_mut().and_then(|c| c.sim.as_mut()) {
            mirror_weapons_of(sim, rt, slot);
        }
    }
    rt.clear_dobj_updates();
}

/// [`mirror_weapons`] for one client.
fn mirror_weapons_of(sim: &mut ClientSim, rt: &mut script::ScriptRuntime, slot: usize) {
    // UO's script-time DObj updates, on the weapon the entity had then.
    for a in rt.take_dobj_updates_of(slot) {
        sim.update_dobj(&a);
    }
    let w = rt.client_weapons(slot);
    sim.ps.weapons_held = w.held;
    sim.ps.weapon_slots = w.slots;
    sim.ps.weapon = w.current;
    sim.viewmodel_index = rt.client_viewmodel(slot);
    sim.shellshock = rt.client_shellshock(slot);
    // The body, head and helmet the character script dressed the
    // client in: what a shot at it is traced against.
    if let Some(a) = rt.client_assembly(slot)
        && a != sim.assembly
    {
        sim.assembly = a;
    }
    // And back the other way: the sim owns where a player is, so the
    // script's copy is written from it every frame.
    rt.set_client_origin(slot, sim.origin());
}

/// The weapon ops script queued, each applied once.
pub(super) fn apply_weapon_ops(clients: &mut [Option<Client>], rt: &mut script::ScriptRuntime) {
    for (slot, op) in rt.take_weapon_ops() {
        if let Some(c) = clients.get_mut(slot).and_then(Option::as_mut)
            && let Some(sim) = c.sim.as_mut()
        {
            apply_weapon_op(sim, op);
        }
    }
}

/// What script did to each sim, applied once: events, `setOrigin`,
/// `setPlayerAngles` and the damage the callback did.
pub(super) fn apply_sim_ops(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    now_ms: i32,
) {
    for (slot, op) in rt.take_sim_ops() {
        let Some(c) = clients.get_mut(slot).and_then(Option::as_mut) else {
            continue;
        };
        let st = c.last_processed_st;
        let Some(sim) = c.sim.as_mut() else {
            continue;
        };
        apply_sim_op(sim, rt, slot, op, anims, weapons, rng, (now_ms, st));
    }
}

/// One op of [`apply_sim_ops`]. `cmd_ms` is the client's last processed
/// cmd's serverTime, the clock its anim timers run on.
fn apply_sim_op(
    sim: &mut ClientSim,
    rt: &mut script::ScriptRuntime,
    slot: usize,
    op: crate::game::host::SimOp,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    (now_ms, cmd_ms): (i32, i32),
) {
    use crate::game::host::SimOp;
    match op {
        SimOp::Event { event, parm } => sim.add_event(event, parm),
        // The host's copy too: the weapon mirror put the pre-teleport
        // origin back over the builtin's write, and a callback ahead of the
        // client's next cmd reads it.
        SimOp::SetOrigin { origin } => {
            sim.teleport(origin);
            rt.set_client_origin(slot, sim.origin());
        }
        SimOp::SetViewAngles { angles } => sim.set_view_angle(angles),
        SimOp::Mount {
            turret,
            stance,
            view,
        } => {
            let rule = crate::game::turret::AimRule::of(rt.host.defs.proto);
            crate::game::turret::mount_sim(sim, rule, turret, stance, view);
        }
        SimOp::WeaponDisabled(on) => sim.ps.weapon_disabled = on,
        SimOp::ViewKick { force, from } => sim.view_kick(force, from),
        SimOp::VehiclePain { damage, from } => sim.vehicle_pain(damage, from),
        SimOp::Damaged {
            attacker,
            damage,
            dir,
            ..
        } => {
            if let Some(a) = attacker.filter(|&a| a != slot && damage > 0) {
                sim.last_hit = Some((a, now_ms, dir));
            }
            let index = sim.ps.weapon as usize;
            let inputs = anims.map(|anims| crate::spectate::AnimInputs {
                anims,
                weapon: crate::items::item_name(weapons.list(), index).unwrap_or_default(),
                weapon_class: weapons.class(index),
            });
            sim.take_damage(&op, inputs.as_ref(), rng, (now_ms, cmd_ms));
        }
    }
}

/// An attack's effects in the order it raised them: an impact goes on the
/// wire and a hit runs the damage callback there and then, so the flesh pair
/// a leg's callback raises numbers below the next leg's wall impact.
fn apply_effects(rt: &mut script::ScriptRuntime, effects: Vec<Effect>) {
    for e in effects {
        match e {
            Effect::Impact(te) => rt.push_temp_entity(te),
            Effect::Hit(h) => rt.deliver_hits(vec![h]),
            Effect::EntityHit(h) => rt.deliver_entity_hit(&h),
            Effect::TriggerSweep(t) => rt.trigger_sweep(&t),
        }
    }
}

/// One gunner's rounds delivered inside its own `ClientEndFrame`, as
/// `turret_think_client` -> `Bullet_Fire` -> `G_Damage` runs the callback
/// there (combat doc 16.2): what it queued is applied before the next
/// slot's turn, so a later gunner's trace passes a corpse it made and a
/// higher slot's aim trace and pose read the move. A victim above the
/// gunner takes its feedback this frame; one below had its end frame
/// already and takes it on the next, and a gunner below that dies keeps
/// the gun until its own next turn releases it.
pub(super) fn deliver_turret_rounds(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    effects: Vec<Effect>,
    gunner: usize,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    now_ms: i32,
) {
    if effects.is_empty() {
        return;
    }
    let feedback_now: Vec<usize> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Hit(h) if h.victim > gunner => Some(h.victim),
            _ => None,
        })
        .collect();
    mirror_roster(clients, rt);
    apply_effects(rt, effects);
    mirror_weapons(clients, rt);
    apply_weapon_ops(clients, rt);
    apply_sim_ops(clients, rt, anims, weapons, rng, now_ms);
    mirror_vitals(clients, rt);
    for slot in feedback_now {
        if let Some(sim) = clients[slot].as_mut().and_then(|c| c.sim.as_mut()) {
            sim.end_frame(now_ms);
        }
    }
    // A body the callback killed or moved, for the later slots' aim traces.
    for (slot, c) in clients.iter().enumerate() {
        let sim = c.as_ref().and_then(|c| c.sim.as_ref());
        rt.set_client_body(slot, sim.and_then(|s| s.hit_body(slot)));
        rt.set_client_dobj(slot, sim.and_then(|s| s.dobj(slot)));
    }
}

/// What the follow reads for `slot`: the script's fields when the slot has
/// a client entity, the sim's own mode when it has none.
pub(super) fn session_of(
    rt: Option<&mut script::ScriptRuntime>,
    slot: usize,
    sim: &ClientSim,
) -> follow::Session {
    rt.and_then(|rt| rt.client_session(slot))
        .unwrap_or_else(|| follow::Session::unscripted(sim.pm_type == PmType::Spectator))
}

/// Whether `slot` holds a client a follow may copy: `GetFollowPlayerState`
/// (`game.mp.i386.so` 0x415c4) refuses one whose own-view bit is off.
pub(super) fn followable(clients: &[Option<Client>], slot: usize) -> bool {
    clients
        .get(slot)
        .and_then(Option::as_ref)
        .and_then(|c| c.sim.as_ref())
        .is_some_and(|s| s.own_view)
}

/// `DeathmatchScoreboardMessage` (`.so` 0x459c0) for these clients: the
/// script's own, or every row zero without one.
pub(super) fn scoreboard(
    clients: &[Option<Client>],
    rt: Option<&mut script::ScriptRuntime>,
) -> String {
    match rt {
        Some(rt) => {
            mirror_roster(clients, rt);
            rt.scoreboard()
        }
        None => {
            let rows = roster(clients)
                .enumerate()
                .filter_map(|(slot, r)| {
                    r.map(|r| crate::game::scoreboard::Row {
                        slot,
                        connecting: r.connecting,
                        spectator: false,
                        score: 0,
                        deaths: 0,
                        ping: r.ping,
                        icon: 0,
                    })
                })
                .collect();
            crate::game::scoreboard::text(rows, [0, 0])
        }
    }
}

/// Each slot's connection and follow target, as the scoreboard and
/// `player_die`'s walk read them.
fn roster(
    clients: &[Option<Client>],
) -> impl Iterator<Item = Option<crate::game::scoreboard::RosterSlot>> + '_ {
    clients.iter().map(|c| {
        c.as_ref().map(|c| crate::game::scoreboard::RosterSlot {
            connecting: c.state == ClientState::Connected,
            active: c.state == ClientState::Active,
            following: c.sim.as_ref().and_then(|s| s.follow.target),
            ping: c.link.ping,
            bot: c.is_bot,
        })
    })
}

/// [`roster`] onto the host, ahead of every script entry that can kill: the
/// walk runs inside the VM, the moment the killed callback returns.
pub(super) fn mirror_roster(clients: &[Option<Client>], rt: &mut script::ScriptRuntime) {
    rt.host.client_roster = roster(clients).collect();
    for (name, c) in rt.host.client_names.iter_mut().zip(clients) {
        name.clear();
        if let Some(c) = c {
            name.push_str(&c.name);
        }
    }
}

/// The host's vitals onto every sim.
pub(super) fn mirror_vitals(clients: &mut [Option<Client>], rt: &script::ScriptRuntime) {
    for (slot, c) in clients.iter_mut().enumerate() {
        if let Some(sim) = c.as_mut().and_then(|c| c.sim.as_mut()) {
            mirror_vitals_of(sim, rt, slot);
        }
    }
}

/// [`mirror_vitals`] for one client.
fn mirror_vitals_of(sim: &mut ClientSim, rt: &script::ScriptRuntime, slot: usize) {
    sim.max_speed = rt.host.client_speed(slot);
    sim.gravity = rt.host.client_gravity();
    sim.knockback = crate::game::builtins::cvar::atof(rt.host.cvars.get("g_knockback"));
    sim.ps.bg = rt.host.bg_cvars();
    // Health onto a playing sim only. Neither `ClientEndFrame`'s intermission
    // arm nor `SpectatorClientEndFrame` copies `ent->health` into the
    // playerstate, so both keep the zero their own spawn left (map-cycle doc,
    // 6.2; `spectate.rs`, `become_spectator`).
    if sim.pm_type != crate::spectate::PmType::Normal {
        return;
    }
    let v = rt.client_vitals(slot);
    sim.health = v.health;
    sim.max_health = v.max_health;
    if v.dead {
        sim.die();
    } else {
        sim.dead = false;
    }
}

/// What a damage or kill callback reads of a client that no builtin can
/// reach, off its sim as its cmds so far left it.
pub(super) fn mirror_for_callback(
    rt: &mut script::ScriptRuntime,
    sim: &ClientSim,
    proto: &'static Cod1Wire,
    slot: usize,
    st: i32,
) {
    // What `cloneplayer` and the dropped cook read.
    rt.set_client_entity_state(slot, Some(sim.to_entity(proto, slot, st)));
    rt.set_client_grenade_ms(slot, sim.ps.grenade_time_left_ms);
    rt.set_client_height(slot, sim.box_height());
    // What `dropItem` hands the dropped weapon, and where it starts.
    rt.set_client_ammo(slot, sim.ps.ammo, sim.ps.ammoclip);
    rt.set_client_dobj(slot, sim.dobj(slot));
}

/// What a callback queued for `slot`, applied to its sim before its next cmd:
/// retail's script writes the playerstate there and then. `at` is the frame
/// clock and the client's last processed cmd's serverTime.
pub(super) fn apply_callback_ops(
    rt: &mut script::ScriptRuntime,
    sim: &mut ClientSim,
    slot: usize,
    anims: Option<&vcod_common::animtree::PlayerAnims>,
    weapons: &crate::weapons::WeaponTable,
    rng: &mut u64,
    at: (i32, i32),
) {
    let (weapon_ops, sim_ops) = rt.take_ops_of(slot);
    for op in weapon_ops {
        apply_weapon_op(sim, op);
    }
    for op in sim_ops {
        apply_sim_op(sim, rt, slot, op, anims, weapons, rng, at);
    }
    mirror_weapons_of(sim, rt, slot);
    mirror_vitals_of(sim, rt, slot);
}

/// `G_RunClient` (0x40660) for a linked client on its turn in the entity
/// pass: its origin is the parent's plus the offset it linked at, turned by
/// the parent's angles (`G_SetFixedLink`'s mode 2), and a held walk input
/// moves it not at all; then the relink (0x40713). The parent has had its
/// own turn (movers doc, section 13). Its velocity is whatever it linked
/// with: retail's plant capture holds 184,27 across the abort's two linked
/// seconds and 0 under 92 forward cmds (object-model doc 23.2). A parent
/// that is gone releases the link: `sd.gsc`'s plant success never unlinks,
/// the bombzone's `delete()` is what frees the record. This tick's per-cmd
/// touch passes already ran at the last frame's anchor, as retail's do.
pub(super) fn run_client_link(rt: &mut script::ScriptRuntime, slot: usize, sim: &mut ClientSim) {
    if let (None, Some(seat)) = (sim.link_to, sim.seat) {
        run_rider_link(rt, slot, sim, seat);
        return;
    }
    let Some(link) = sim.link_to else {
        return;
    };
    match rt.link_anchor(link.parent) {
        Some((p, angles)) => {
            let [f, l, u] = vcod_common::x87::angles_to_axis(angles.into());
            let [x, y, z] = link.offset;
            sim.ps.origin = glam::Vec3::from(p) + f * x + l * y + u * z;
            sim.ps.velocity = link.velocity.into();
            rt.set_client_origin(slot, sim.origin());
            link_client(rt, slot, sim);
        }
        None => sim.link_to = None,
    }
}

/// `G_RunClient` (game.mp.uo 0x41dfc) on a seated rider: `G_SetFixedLink`
/// mode 2 puts `r.currentOrigin` and `ps.origin` at the seat tag of the
/// vehicle as it stands, shifted by the link's own offset, and links him
/// there unsnapped. His end frame puts `ps.origin` and `r.currentOrigin` back on the
/// seat (coduo-vehicles.md 13.7, "The tank driver's link offset").
fn run_rider_link(
    rt: &mut script::ScriptRuntime,
    slot: usize,
    sim: &mut ClientSim,
    seat: crate::spectate::Seat,
) {
    let Some(at) = rt.with_host(|host, cx| {
        let id = host.ents.handle(seat.vehicle)?;
        let posed = crate::game::vehicle::seat::posed(host, cx, id)?;
        let (p, r) = posed.tag(vcod_common::vehiclepose::seat_tag(seat.pos)?)?;
        Some(p + r * glam::Vec3::from(seat.link_offset))
    }) else {
        return;
    };
    sim.seat_linked_at(at);
    rt.set_client_origin(slot, at.to_array());
    link_client(rt, slot, sim);
}

/// A moving team master's turn (`G_MoverTeam` over the chain): as
/// [`push_mover`], every part in turn.
pub(super) fn push_team(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    master: vcod_gsc::EntId,
    step: Option<crate::game::mover::Step>,
    collision: &vcod_common::collision::CollisionWorld,
) {
    let mut sims: Vec<(usize, &mut ClientSim)> = clients
        .iter_mut()
        .enumerate()
        .filter_map(|(i, c)| Some((i, c.as_mut()?.sim.as_mut()?)))
        .collect();
    let before: Vec<glam::Vec3> = sims.iter().map(|(_, s)| s.ps.origin).collect();
    rt.team_move(master, step, &mut sims, collision);
    for ((slot, sim), was) in sims.iter().zip(before) {
        if sim.ps.origin != was {
            rt.set_client_origin(*slot, sim.origin());
        }
    }
}

/// A brush model mover's push on its turn in the entity pass: it carries its
/// riders and shoves the bodies and items in its way, or holds a frame when
/// a body fits nowhere (`G_MoverTeam`; movers doc, section 12).
pub(super) fn push_mover(
    clients: &mut [Option<Client>],
    rt: &mut script::ScriptRuntime,
    step: &crate::game::mover::Step,
    collision: &vcod_common::collision::CollisionWorld,
) {
    let mut sims: Vec<(usize, &mut ClientSim)> = clients
        .iter_mut()
        .enumerate()
        .filter_map(|(i, c)| Some((i, c.as_mut()?.sim.as_mut()?)))
        .collect();
    let before: Vec<glam::Vec3> = sims.iter().map(|(_, s)| s.ps.origin).collect();
    rt.push_mover(step, &mut sims, collision);
    for ((slot, sim), was) in sims.iter().zip(before) {
        if sim.ps.origin != was {
            rt.set_client_origin(*slot, sim.origin());
        }
    }
}

/// `ClientThink_real`'s link after a cmd's move and its shots, at the
/// snapped origin with the contents the last end frame wrote (combat doc
/// 14.1, 14.7).
pub(super) fn link_client(rt: &mut script::ScriptRuntime, slot: usize, sim: &ClientSim) {
    // The intermission arm returns ahead of the link; a spectator's think
    // unlinks, which its contents 0 does here.
    if sim.pm_type == crate::spectate::PmType::Intermission {
        return;
    }
    let playing = sim.pm_type == crate::spectate::PmType::Normal && !sim.dead;
    let bounds = (sim.ps.mins().into(), sim.ps.maxs().into());
    // A death earlier this tick linked `player_die`'s corpse contents, which
    // `r.contents` holds until the end frame and the sim takes only at the
    // next mirror.
    let contents = if rt.client_vitals(slot).dead && !sim.dead {
        rt.host.area.contents(slot as u32)
    } else {
        sim.contents as i32
    };
    rt.host
        .link_client(slot, sim.link_origin().into(), bounds, contents, playing);
}

/// `slot`'s entry in the movers' body list, off its sim: a death's
/// `CONTENTS_CORPSE` is outside every mover's mask.
pub(super) fn relink(
    bodies: &mut Vec<vcod_common::movetrace::Body>,
    clients: &[Option<Client>],
    slot: usize,
) {
    bodies.retain(|b| b.entity != slot as u32);
    if let Some(sim) = clients[slot].as_ref().and_then(|c| c.sim.as_ref()) {
        bodies.extend(sim.body(slot as u32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_damage_is_retails_share_of_max_health() {
        for (percent, max, damage) in [
            (25, 100, 24),
            (40, 100, 39),
            (77, 100, 76),
            (43, 200, 85),
            (100, 100, 110),
            (13, 100, 12),
            (18, 200, 35),
            (41, 100, 40),
        ] {
            assert_eq!(
                fall_damage(percent, max, false),
                damage,
                "{percent}% of {max}"
            );
        }
    }

    /// 1.5b `ClientEvents` 0x38f2a and 0x38f54 store each step as a float.
    #[test]
    fn fall_damage_on_15_rounds_each_step_to_a_float() {
        for (percent, max, damage) in [(21, 100, 21), (25, 100, 25), (66, 100, 66), (40, 100, 39)] {
            assert_eq!(
                fall_damage(percent, max, true),
                damage,
                "{percent}% of {max}"
            );
        }
    }
}
