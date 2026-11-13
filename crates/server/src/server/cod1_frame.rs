//! CoD1's `G_RunFrame` and each slot's `ClientEndFrame`, run by
//! `Server::game_frame`. The order is retail's
//! (AGENTS.md, "The server tick"); the engine runs the clock and the
//! client moves ahead of it and the configstring, command and scoreboard
//! traffic after it.

use super::cod1_game::{Cod1Module, FrameOut, Lend};
use super::*;

impl Cod1Module {
    pub(super) fn run_frame_cod1(
        &mut self,
        eng: Lend<'_>,
        moved: &[MoveSummary],
    ) -> Result<FrameOut, String> {
        let Lend {
            clients,
            world,
            fs,
            proto,
            sv_time_ms,
            svs_time_ms: svs_ms,
            rng,
            stuck_rand,
            archive,
            test_client_counter,
            ..
        } = eng;
        let mut timeout_written = false;
        let mut vote_lines: Vec<String> = Vec::new();
        let weapons = self.weapon_table.clone();

        let mut client_commands = Vec::new();
        let mut console_lines: Vec<String> = Vec::new();
        let mut ranks_dirty = false;
        let svs_now = svs_ms;
        if let Some(rt) = self.script.as_mut() {
            for (slot, c) in clients.iter().enumerate() {
                if let Some(c) = c {
                    rt.set_client_buttons(
                        slot,
                        moved[slot].last_buttons.unwrap_or(c.last_cmd.buttons),
                    );
                    rt.set_client_last_cmd(slot, c.last_cmd);
                }
            }
            // `addTestClient` can run on any script entry (a load's settle
            // frames too), so the counter is read back before it goes in.
            let counter = (*test_client_counter).max(rt.host.test_clients.counter);
            rt.host.test_clients.counter = counter;
            rt.set_network_time(svs_now);
            for (slot, port, name) in std::mem::take(&mut rt.host.test_clients.added) {
                Server::install_test_client(clients, slot, port, name, proto, sv_time_ms);
            }
            mirror_roster(clients, rt);
            crate::tick_trace::step!("Scr_RunThreads");
            rt.run_threads(sv_time_ms);
            *test_client_counter = rt.host.test_clients.counter;
            for (slot, port, name) in std::mem::take(&mut rt.host.test_clients.added) {
                Server::install_test_client(clients, slot, port, name, proto, sv_time_ms);
            }
            // The vehicle guns the threads fired (`fireTurret` and kin run
            // their rounds inside the call).
            vehicle_fire(
                clients,
                rt,
                VehicleFireCtx {
                    world: world.as_deref(),
                    hitlocs: &self.hitlocs,
                    fs: fs.as_deref(),
                    anims: self.anims.as_deref(),
                    rigs: &mut self.hit_rigs,
                    weapons: &weapons,
                    rng,
                    now_ms: sv_time_ms,
                },
            );
            // `self spawn(origin, angles)` moves the sim, which no builtin
            // can reach; this is where the queue lands. Before the weapons,
            // because a spawn resets the playerstate; the ammo arrays are
            // the weapon ops' alone, so the loadout survives the reset
            // whichever pass applied it.
            for s in rt.take_client_spawns() {
                crate::tick_trace::step!("ClientSpawn slot {}", s.slot);
                let Some(c) = clients.get_mut(s.slot).and_then(Option::as_mut) else {
                    continue;
                };
                // The client's own angles at this moment, not zero: a
                // spectator that looked around before answering the weapon
                // menu carries them, and `spawn_delta_angles` subtracts them
                // so the spawn preserves the view instead of force-turning it.
                let cmd_angles = c.last_cmd.angles;
                let last_cmd = c.last_cmd;
                let Some(sim) = c.sim.as_mut() else {
                    continue;
                };
                // `ClientSpawn` lets go of a gun first (turrets doc 8). The
                // teleport's events go out only for a spawn into play, the
                // `sessionstate` the script set ahead of the spawn.
                // A gun deleted this frame queued its release for here; the
                // spawn comes after it, as `G_FreeTurret` ran first in retail.
                rt.apply_turret_releases(s.slot, sim);
                let temps = rt.release_turret(s.slot, sim).unwrap_or_default();
                // And a vehicle seat (`ClientSpawn` 0x44df4).
                if sim.seat.is_some() {
                    rt.with_host(|host, cx| {
                        crate::game::vehicle::seat::unlink_player(
                            host,
                            cx,
                            s.slot,
                            false,
                            [0.0; 3],
                            ([0.0; 3], [0.0; 3]),
                        )
                    });
                    rt.host.vehicles.seat_ops.clear();
                    sim.leave_seat(None, None);
                }
                if s.mode == SpawnMode::Player {
                    for te in temps {
                        rt.push_temp_entity(te);
                    }
                }
                // The ammo is the weapon op queue's: the builtin queued the
                // memset's emptying there ahead of the loadout, and a queue
                // drained before this pass already landed both.
                let ammo = (sim.ps.ammo, sim.ps.ammoclip);
                match s.mode {
                    SpawnMode::Player => {
                        sim.become_player(s.origin, s.yaw_deg, cmd_angles);
                        // UO's `G_DObjUpdate` (0x87df9) rebuilds a client's
                        // DObj when the spawning thread sets his model.
                        sim.dobj_ahead = sim.ps.rules == vcod_common::pmove::MoveRules::Uo;
                        // The builtin's think on its fresh sim already ran
                        // this one's `ClientImpacts`.
                        let bodies = rt.host.move_bodies(s.slot);
                        let (think, _) = sim.spawn_think_on(
                            &last_cmd,
                            sv_time_ms,
                            world.as_ref().map(|w| {
                                vcod_common::movetrace::MoveWorld::new(
                                    &w.collision,
                                    &bodies,
                                    s.slot as u32,
                                )
                            }),
                        );
                        // `ClientSpawn` runs inside `self spawn()`, ahead of
                        // the loadout the script gives after it, so the
                        // think animates empty hands.
                        if let Some(anims) = self.anims.as_deref() {
                            let index = sim.ps.weapon as usize;
                            sim.update_anims(
                                &crate::spectate::AnimInputs {
                                    anims,
                                    weapon: crate::items::item_name(
                                        self.weapon_table.list(),
                                        index,
                                    )
                                    .unwrap_or_default(),
                                    weapon_class: self.weapon_table.class(index),
                                },
                                &think,
                                (sv_time_ms, sv_time_ms),
                                &[],
                                rng,
                            );
                        }
                    }
                    SpawnMode::Spectator => {
                        sim.become_spectator(s.origin, s.yaw_deg, cmd_angles);
                        sim.spawn_think_on(
                            &last_cmd,
                            sv_time_ms,
                            world
                                .as_ref()
                                .map(|w| vcod_common::movetrace::MoveWorld::bare(&w.collision)),
                        );
                    }
                    SpawnMode::Intermission => {
                        sim.become_intermission(s.origin, s.yaw_deg, cmd_angles, sv_time_ms)
                    }
                }
                (sim.ps.ammo, sim.ps.ammoclip) = ammo;
                // The spawn's own think runs a player or a spectator up to
                // the frame's clock; the intermission arm runs no pmove.
                if s.mode != SpawnMode::Intermission {
                    c.last_processed_st = sv_time_ms;
                }
            }
            // The machine's own switches first: `pickup` writes `ps.weapon`
            // when a drop ends, and the mirror below would put the old
            // weapon back and start the identical putaway again.
            for (slot, weapon) in self.weapon_changes.drain(..) {
                rt.set_client_weapon(slot, weapon);
            }
            // What the client holds comes across the same way the
            // configstrings do: re-read every frame, because any thread can
            // have changed them. The held bits have to be among them --
            // `PM_Weapon` disarms a player who no longer owns `ps.weapon`
            // (combat doc, section 1.8) -- and `ps.weapon` with them, so a
            // sim reset outside a move comes back armed. The write-back
            // above is what makes that safe: the host's copy already carries
            // whatever the machine switched to this tick.
            mirror_weapons(clients, rt);
            // The ammo and the current weapon, which are edges rather than
            // state: applying a full clip every frame would make the weapon
            // bottomless.
            apply_weapon_ops(clients, rt);
            // `linkTo` and `unlink`, before the entity pass re-anchors so a
            // link made this frame is already pinned on this frame's wire: both
            // retail captures read the new `pm_type` on the next snapshot
            // after the cmd (object-model doc, 23.2).
            for (slot, op) in rt.take_link_ops() {
                let Some(sim) = clients
                    .get_mut(slot)
                    .and_then(Option::as_mut)
                    .and_then(|c| c.sim.as_mut())
                else {
                    continue;
                };
                sim.link_to = match op {
                    crate::game::host::LinkOp::Link { parent, offset } => {
                        Some(crate::spectate::Link {
                            parent,
                            offset,
                            velocity: sim.ps.velocity.into(),
                        })
                    }
                    crate::game::host::LinkOp::Unlink => None,
                };
            }
            // A seat the frame's threads left (`Scr_Vehicle_Die` under a
            // `doDamage`) is left ahead of the entity pass, and ahead of
            // the knockback of any hit after it: the exit's pop sets the
            // velocity, the hit adds to it.
            apply_seat_ops(clients, rt);
            // What script did to each sim, applied once, then the health
            // mirror and the frame's damage feedback, in that order:
            // `P_DamageFeedback` reads the health the hit left.
            let anims = self.anims.as_deref();
            apply_sim_ops(clients, rt, anims, &weapons, rng, sv_time_ms);
            // `G_RunFrame`'s entity loop: items, links, movers, missiles,
            // thinks and linked clients one entity at a time by number
            // (combat doc 14.7), after the frame's threads and every client
            // move they made (spawns, `setOrigin`s, links), so a thread reads
            // a grenade's or a mover's last-frame origin, the flight is
            // traced past the bodies where script put them, and a blast walk
            // meets this frame's turn of every entity numbered below the
            // grenade (14.7, 16.2). A grenade thrown on this tick was spawned
            // inside its cmd on the last frame's `level.time`, so it has
            // already flown a frame by the time the snapshot goes out (combat
            // doc 11.4). Each blast is walked on its missile's turn, one
            // victim's callback before the next victim is measured (14.1,
            // 14.5); each brush model's move pushes its riders and the bodies
            // in its way on its turn, or holds a frame when a body fits
            // nowhere (`G_MoverTeam`; movers doc, section 12); a linked client
            // is re-anchored on its own turn, after its parent's.
            self.pending_explosions.clear();
            let collision = world.as_ref().map(|w| &w.collision);
            let (fs, anims, now_ms) = (fs.as_deref(), self.anims.as_deref(), sv_time_ms);
            let arith = vcod_common::patch::Arith::for_protocol(proto);
            // Where a callback earlier in the walk set a client down: its
            // link moved off the one its last cmd made (`setOrigin` relinks
            // at once, combat doc 14.7), and the walk measures it there.
            let set_down =
                |rt: &script::ScriptRuntime, slot: usize, s: &crate::spectate::ClientSim| {
                    let link = glam::Vec3::from(rt.host.client_link_origin[slot]);
                    (link != s.link_origin()).then_some(link)
                };
            let victim =
                |rt: &script::ScriptRuntime, slot: usize, s: &crate::spectate::ClientSim| {
                    let eye = s.ps.view().eye;
                    let mut v = crate::game::combat::BlastVictim {
                        slot,
                        origin: s.ps.origin,
                        link_origin: s.link_origin(),
                        mins: s.ps.mins(),
                        maxs: s.ps.maxs(),
                        eye,
                    };
                    if let Some(at) = set_down(rt, slot, s) {
                        v = crate::game::combat::BlastVictim {
                            origin: at,
                            link_origin: at,
                            eye: at + (eye - s.ps.origin),
                            ..v
                        };
                    }
                    v
                };
            // A rider is linked to his vehicle (`G_EntLinkToWithOffset`,
            // coduo-vehicles.md 13) as a `linkTo` client is to its parent.
            let client_links: Vec<(usize, vcod_gsc::EntId)> = clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let sim = c.as_ref()?.sim.as_ref()?;
                    let parent = match (sim.link_to, sim.seat) {
                        (Some(l), _) => l.parent,
                        (None, Some(seat)) => rt.host.ents.handle(seat.vehicle)?,
                        (None, None) => return None,
                    };
                    Some((i, parent))
                })
                .collect();
            crate::tick_trace::step!("G_RunFrame entities");
            let mut pass = rt.begin_entity_pass(client_links);
            loop {
                let sims: Vec<(usize, &crate::spectate::ClientSim)> = clients
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                    .collect();
                let Some(stop) = rt.run_entity_pass(&mut pass, collision, &sims, sv_time_ms) else {
                    break;
                };
                let x = match stop {
                    script::PassStop::Blast(x) => {
                        crate::tick_trace::step!("G_ExplodeMissile weapon {}", x.weapon);
                        x
                    }
                    script::PassStop::Push(step) => {
                        drop(sims);
                        crate::tick_trace::step!("G_RunMover push");
                        if let Some(world) = world.as_ref() {
                            push_mover(clients, rt, &step, &world.collision);
                        }
                        continue;
                    }
                    script::PassStop::Client(slot) => {
                        drop(sims);
                        crate::tick_trace::step!("G_RunClient link slot {slot}");
                        if let Some(sim) = clients
                            .get_mut(slot)
                            .and_then(Option::as_mut)
                            .and_then(|c| c.sim.as_mut())
                        {
                            run_client_link(rt, slot, sim);
                        }
                        continue;
                    }
                };
                self.driver.noises.push(crate::bots::Noise {
                    at: (x.at + glam::Vec3::Z * 40.0).into(),
                    source: x.owner.unwrap_or(usize::MAX),
                    radius: crate::bots::HEAR_BLAST,
                });
                let Some(def) = weapons.get(x.weapon as usize) else {
                    if x.concussive {
                        rt.spawn_concussive();
                    }
                    self.pending_explosions.push(x);
                    continue;
                };
                // A thrower who left is `r.ownerNum` 0x3ff: the direct hit's
                // `G_Damage` gets a NULL attacker. A UO vehicle shell keeps
                // its driver's slot (`+0x330`) after he left, and the freed
                // entity there reads number 0; its fuse blast names the
                // vehicle (combat doc 13.7).
                let owner = rt.charged_owner(x.owner);
                let attacker = owner.unwrap_or(crate::game::host::ENTITYNUM_NONE as usize);
                // A shell's direct hit lands before its splash, which skips
                // the client it struck (`G_MissileImpact`, coduo-diff.md 21).
                // UO's `G_MissileImpact` drops the inflictor with the owner
                // (game.mp.uo 0x5dc8e), so `G_DamageClient` names weapon 0
                // (combat doc 13.7); 1.1's hands the missile on.
                let uo = proto.is_uo();
                let (inflictor, weapon) = if x.owner.is_none() && uo {
                    (None, "none")
                } else {
                    (
                        Some(x.inflictor),
                        crate::items::item_name(crate::defs::Defs::of(proto), x.weapon as usize)
                            .unwrap_or_default(),
                    )
                };
                if let Some((victim, damage)) = x.direct
                    && rt.client_vitals(victim).takedamage
                    && sims.iter().any(|(slot, _)| *slot == victim)
                {
                    let hit = crate::game::combat::Hit {
                        victim,
                        attacker,
                        inflictor,
                        damage,
                        dflags: 0,
                        mod_: x.direct_mod,
                        weapon: weapon.to_string(),
                        point: x.at.into(),
                        // `BG_EvaluateTrajectoryDelta` at the contact: an
                        // artillery shell's is straight down.
                        dir: x.flight.normalize_or(-glam::Vec3::Z).into(),
                        hitloc: "none",
                    };
                    rt.deliver_hits(vec![hit]);
                }
                // Any other entity the shell struck that takes damage (a UO
                // vehicle, a `setTakeDamage` script model) takes it the same
                // way (`G_MissileImpact`'s `G_Damage`, 1.1 0x53ab4, game.mp.uo
                // 0x6db67, combat doc 13.1): the flight as the direction, the
                // contact as the point.
                if let Some(v) = x
                    .struck
                    .and_then(|n| rt.host.ents.handle(n))
                    .filter(|&v| rt.host.entity_takes_damage(v))
                    && x.shell_damage > 0
                {
                    rt.deliver_entity_hit(&crate::game::combat::EntityHit {
                        target: v,
                        attacker,
                        inflictor: Some(x.inflictor),
                        damage: x.shell_damage,
                        dflags: 0,
                        mod_: x.direct_mod,
                        point: x.contact.into(),
                        dir: x.flight.normalize_or_zero().into(),
                    });
                }
                // UO's `G_MissileImpact` walks the trigger_damages at the
                // contact alone: start and end are both the trace's end
                // position (game.mp.uo 0x5df1a, combat doc 14.8).
                if proto.is_uo() && x.struck.is_some() && x.shell_damage != 0 {
                    rt.trigger_sweep(&crate::game::combat::TriggerSweep {
                        attacker,
                        start: x.contact,
                        end: x.contact,
                        damage: x.shell_damage,
                        mod_: x.direct_mod,
                    });
                }
                // `G_RadiusDamage` returns before its walk when `parent`, its
                // attacker, is NULL: the thrower left (combat doc 13.7).
                let Some(owner) = owner else {
                    if x.concussive {
                        rt.spawn_concussive();
                    }
                    self.pending_explosions.push(x);
                    continue;
                };
                let blast = crate::game::combat::Blast::new(
                    x.at,
                    def.explosion_radius,
                    def.explosion_inner_damage as f32,
                    def.explosion_outer_damage as f32,
                    Some(owner),
                    Some(x.inflictor),
                    crate::items::item_name(crate::defs::Defs::of(proto), x.weapon as usize)
                        .unwrap_or_default(),
                    x.mod_,
                );
                // `trap_EntitiesInBox`' order (combat doc 14.7): the
                // clients and the turrets as the area tree lists them, taken
                // once. Each is measured on its turn, after every earlier
                // victim's callback (14.5).
                let (mins, maxs) = blast.search_box();
                let candidates: Vec<BlastCandidate> = rt
                    .host
                    .area
                    .entities_in_box(mins, maxs, -1)
                    .into_iter()
                    .filter_map(|n| {
                        if let Some(&(slot, s)) = sims.iter().find(|(slot, _)| *slot == n as usize)
                        {
                            return Some(BlastCandidate::Client(slot, s));
                        }
                        let id = rt.host.ents.handle(n)?;
                        Some(BlastCandidate::Entity(id))
                    })
                    .collect();
                // A client a callback of this walk killed is a corpse and
                // stops nothing.
                let live_bodies =
                    |rt: &script::ScriptRuntime| -> Vec<crate::game::combat::HitBody> {
                        if !rt.host.bodies_shield_blasts() {
                            return Vec::new();
                        }
                        sims.iter()
                            .filter(|(other, _)| !rt.client_vitals(*other).dead)
                            .filter_map(|&(other, s)| {
                                let mut body = s.hit_body(other)?;
                                if let Some(at) = set_down(rt, other, s) {
                                    body.origin = at;
                                }
                                Some(body)
                            })
                            .collect()
                    };
                for candidate in candidates {
                    match candidate {
                        BlastCandidate::Client(slot, s) => {
                            if !rt.client_vitals(slot).takedamage
                                || x.direct.is_some_and(|(v, _)| v == slot)
                            {
                                continue;
                            }
                            let v = victim(rt, slot, s);
                            if !blast.reaches(&v) {
                                continue;
                            }
                            let bodies = live_bodies(rt);
                            let models = match rt.client_entity(slot) {
                                Some(e) => rt.blast_models(e),
                                None => rt.placed_script_models(),
                            };
                            if let Some(hit) = blast.hit(
                                &v,
                                collision,
                                &models,
                                &bodies,
                                crate::game::combat::BoneTraceCtx::of(
                                    fs,
                                    anims,
                                    &mut self.hit_rigs,
                                    now_ms,
                                    &mut rt.host.dobj_posed,
                                    arith,
                                )
                                .as_mut(),
                            ) {
                                rt.deliver_hits(vec![hit]);
                            }
                        }
                        BlastCandidate::Entity(id) => {
                            // `G_RadiusDamage`'s ignore: what the shell struck.
                            if x.struck == Some(id.0) {
                                continue;
                            }
                            let Some(v) = rt.blast_entities().into_iter().find(|v| v.id == id)
                            else {
                                continue;
                            };
                            if !blast.reaches_entity(&v) {
                                continue;
                            }
                            let bodies = live_bodies(rt);
                            let models = rt.blast_models(id);
                            if let Some(damage) = blast.entity_damage(
                                &v,
                                collision,
                                &models,
                                &bodies,
                                crate::game::combat::BoneTraceCtx::of(
                                    fs,
                                    anims,
                                    &mut self.hit_rigs,
                                    now_ms,
                                    &mut rt.host.dobj_posed,
                                    arith,
                                )
                                .as_mut(),
                            ) {
                                if rt.host.vehicles.live.contains_key(&v.id) {
                                    rt.damage_vehicle(
                                        v.id,
                                        damage,
                                        owner,
                                        x.inflictor,
                                        x.at.into(),
                                        x.mod_,
                                    );
                                } else {
                                    rt.damage_entity(&v, &blast, damage, owner);
                                }
                            }
                        }
                    }
                }
                if x.concussive {
                    rt.spawn_concussive();
                }
                self.pending_explosions.push(x);
            }
            // A `G_Error` in the pass ends the frame here, and the engine
            // drops the level (docs/research/cod11-movers.md 18).
            if let Some(e) = rt.take_game_error() {
                return Err(e);
            }
            // What the blasts' callbacks did to the sims, as above.
            mirror_weapons(clients, rt);
            apply_weapon_ops(clients, rt);
            apply_sim_ops(
                clients,
                rt,
                self.anims.as_deref(),
                &weapons,
                rng,
                sv_time_ms,
            );
            mirror_vitals(clients, rt);
            // `G_RunClient`'s `pm_type` step down for a rider who left a
            // seat ahead of the entity pass; for one the vehicle think below
            // puts out, `ClientEndFrame`'s write does it.
            for sim in clients.iter_mut().flatten().filter_map(|c| c.sim.as_mut()) {
                sim.run_client();
            }
            // `G_VehicleClientThink` (game.mp.uo 0x8b0ce): after the entity
            // loop, before every `ClientEndFrame`.
            crate::tick_trace::step!("G_VehicleClientThink");
            vehicle_frame(clients, rt);
            archive.set_on(rt.archive_on());
            // `G_RunFrame`'s own slot order (0x50ab0-0x50ad7), not arrival
            // order (docs/research/cod11-player-clip.md 4.2, 6). No link
            // follows a CORPSE write here. The follow half goes first, so a
            // spectator reads the own-view bit a lower slot took this frame
            // and a higher slot's from the last one, as retail's loop does.
            let collision = world.as_ref().map(|w| &w.collision);
            for slot in 0..clients.len() {
                crate::tick_trace::step!("ClientEndFrame slot {slot}");
                follow_end_frame(clients, slot, rt, archive, collision, sv_time_ms);
                // `takedamage` off the session state (0x40ed9, 0x40f93,
                // 0x4107c, and `SpectatorClientEndFrame` 0x40788).
                let session = rt.client_session(slot);
                if let Some(session) = session {
                    let playing = session.state == follow::SessionState::Playing;
                    rt.set_client_takedamage(slot, playing);
                }
                let Some(sim) = clients[slot].as_mut().and_then(|c| c.sim.as_mut()) else {
                    continue;
                };
                sim.session = session.map(|s| s.state);
                sim.update_contents();
                let live = sim.pm_type == PmType::Normal && !sim.dead && sim.health > 0;
                if live {
                    let views: Vec<Option<StuckView>> =
                        clients.iter().map(Server::stuck_view).collect();
                    let rules = if proto.is_v1() {
                        StuckRules::V1_1
                    } else {
                        StuckRules::V1_5
                    };
                    let (rng, glibc) = (&mut *rng, &mut *stuck_rand);
                    let rand = || match glibc {
                        Some(g) => g.next_u31(),
                        None => (vcod_common::rng::xorshift(rng) >> 33) as u32,
                    };
                    if let Some(push) = stuck_in_client(slot, &views, rules, rand) {
                        let me = clients[slot].as_mut().and_then(|c| c.sim.as_mut()).unwrap();
                        me.ps.velocity.x = push.self_vel.x;
                        me.ps.velocity.y = push.self_vel.y;
                        me.ps.knockback_ms = 300.0;
                        me.ps.knockback_flags |= vcod_common::pmove::PMF_TIME_KNOCKBACK;
                        // The caller marks only self a corpse (0x411b8); the
                        // partner marks itself on its own turn through the scan.
                        me.contents = CONTENTS_CORPSE;
                        let other = clients[push.other]
                            .as_mut()
                            .and_then(|c| c.sim.as_mut())
                            .unwrap();
                        other.ps.velocity.x = push.other_vel.x;
                        other.ps.velocity.y = push.other_vel.y;
                        other.ps.knockback_ms = 300.0;
                        other.ps.knockback_flags |= vcod_common::pmove::PMF_TIME_KNOCKBACK;
                    }
                }
                let sim = clients[slot].as_mut().and_then(|c| c.sim.as_mut()).unwrap();
                sim.end_frame(sv_time_ms);
                place_rider(rt, slot, sim);
            }
            // `ClientEndFrame`'s `pingPlayer` clear (0x41024) and its compass
            // teammate (0x411fc), per slot in slot order: a lower slot reads
            // a higher one's ping bit as that slot's last end frame left it.
            let sessions: Vec<Option<follow::SessionState>> = (0..clients.len())
                .map(|slot| rt.client_session(slot).map(|s| s.state))
                .collect();
            let teams: Vec<i32> = (0..clients.len())
                .map(|slot| rt.client_team(slot))
                .collect();
            let fog = fog_cull_of(proto, rt.configstrings());
            let broadcast: Vec<bool> = (0..clients.len())
                .map(|slot| rt.in_snapshot_broadcast(slot as u32))
                .collect();
            for slot in 0..clients.len() {
                let Some(sim) = clients[slot].as_mut().and_then(|c| c.sim.as_mut()) else {
                    continue;
                };
                crate::tick_trace::step!("ClientEndFrame compass slot {slot}");
                sim.ping = rt.host.client_ping_until[slot] > rt.host.level_time_ms;
                sim.voted = rt.host.client_voted[slot];
                let found = compass_friend(
                    clients,
                    slot,
                    &sessions,
                    &teams,
                    &broadcast,
                    world.as_ref().map(|w| (&w.vis, fog)),
                    sv_time_ms,
                    proto,
                );
                let sim = clients[slot].as_mut().unwrap().sim.as_mut().unwrap();
                if let Some(found) = found {
                    sim.compass_friend = found.map_or(0, |(info, _, _)| info);
                    match found {
                        Some((_, n, ping)) => {
                            sim.last_friend = n;
                            sim.friend_ping = ping;
                        }
                        None => sim.last_friend = compass::NO_FRIEND,
                    }
                }
            }
            // `ClientEndFrame`'s own-body arm indexes the attachments' tags
            // (1.1 0x413f6, UO `G_UpdateClientInfo` 0x52c81), so the `d`
            // leads the first snapshot naming one (object-model doc 24).
            for (slot, _) in clients.iter().enumerate().filter(|(_, c)| c.is_some()) {
                rt.client_attachments(slot);
            }
            // `ClientEndFrame`'s aim trace and cursor hint, after the script
            // frame and the mirrors so they read the frame's final eye, aim
            // and items; the fire it raises wakes its waiters next frame
            // (object-model doc 23.1). The `pm_type` goes with it: a spawn
            // above changed it with no cmd, and the runtime's gate is what
            // clears a dead or spectating client's `isLookingAt` and hint.
            // Every body first, at the origin the sim ops and the re-anchor
            // left, since any of them can stand in another client's aim.
            for (slot, c) in clients.iter().enumerate() {
                let sim = c.as_ref().and_then(|c| c.sim.as_ref());
                rt.set_client_body(slot, sim.and_then(|s| s.hit_body(slot)));
                rt.set_client_dobj(slot, sim.and_then(|s| s.dobj(slot)));
            }
            // UO's `G_CheckFlameDamage` (game.mp.uo 0x3f4d4), from each
            // slot's `ClientEndFrame` past its cursor hint: a client whose
            // last cmd carries `wbuttons` 1 and that sent `fdc` inside 3 s
            // takes the flamethrower's damage from the named player, four
            // times over from anyone but itself (coduo-diff.md 21).
            if proto.is_uo() {
                let damage = crate::defs::UO
                    .weapon_index("flamethrower_mp")
                    .and_then(|i| weapons.get(i))
                    .map_or(0, |d| d.damage);
                let mut burns = Vec::new();
                for (slot, c) in clients.iter().enumerate() {
                    let Some(c) = c.as_ref() else { continue };
                    let Some((from, at)) = c.flame else { continue };
                    let from = from as usize;
                    if c.last_cmd.wbuttons & vcod_common::net::msg::WBUTTON_IN_FLAME_V22 == 0
                        || sv_time_ms - FLAME_REPORT_MS >= at
                        || clients.get(from).and_then(Option::as_ref).is_none()
                    {
                        continue;
                    }
                    let origin = |n: usize| {
                        clients[n]
                            .as_ref()
                            .and_then(|c| c.sim.as_ref())
                            .map_or(glam::Vec3::ZERO, |s| s.ps.origin)
                    };
                    let (me, them) = (origin(slot), origin(from));
                    burns.push(Effect::Hit(crate::game::combat::Hit {
                        victim: slot,
                        attacker: from,
                        inflictor: None,
                        damage: if from == slot { damage } else { damage * 4 },
                        dflags: 0,
                        mod_: "MOD_FLAME",
                        weapon: "flamethrower_mp".into(),
                        point: them.into(),
                        dir: (me - them).normalize_or_zero().into(),
                        hitloc: "none",
                    }));
                }
                for burn in burns {
                    let Effect::Hit(h) = &burn else { continue };
                    let victim = h.victim;
                    if !rt.client_vitals(victim).takedamage {
                        continue;
                    }
                    deliver_turret_rounds(
                        clients,
                        rt,
                        vec![burn],
                        victim,
                        self.anims.as_deref(),
                        &weapons,
                        rng,
                        sv_time_ms,
                    );
                    // Retail's `P_DamageFeedback` follows the burn inside the
                    // same `ClientEndFrame`; ours ran ahead of this block, so
                    // the burn's feedback is taken now, not a frame later.
                    if let Some(sim) = clients[victim].as_mut().and_then(|c| c.sim.as_mut()) {
                        sim.end_frame(sv_time_ms);
                    }
                }
            }
            // Per slot, `ClientEndFrame`'s tail: the aim trace and cursor
            // hint, `BG_PlayerAnimation`, then `turret_think_client`
            // (turrets doc 6.1). A gunner's rounds are traced and delivered
            // right there, so they meet a lower slot's new pose and a higher
            // slot's last-frame one, and a higher slot's turn sees what the
            // callback did (combat doc 16.1, 16.2).
            // `bg_swingSpeed` is a vmCvar the game refreshes every frame.
            let swing_speed = crate::game::builtins::cvar::atof(rt.cvars().get("bg_swingSpeed"));
            let mount_boxes = rt.mount_boxes();
            for slot in 0..clients.len() {
                let Some(c) = clients[slot].as_mut() else {
                    continue;
                };
                let buttons = moved
                    .get(slot)
                    .and_then(|m| m.last_buttons)
                    .unwrap_or(c.last_cmd.buttons);
                let Some(sim) = c.sim.as_mut() else { continue };
                crate::tick_trace::step!("ClientEndFrame aim slot {slot}");
                rt.set_client_pm_type(slot, sim.wire_pm_type());
                // The link the script frame made is only on the sim from
                // here, so the ground reading script sees next frame is
                // taken again after it.
                rt.set_client_on_ground(slot, sim.on_ground());
                rt.set_client_stance(slot, sim.ps.stance);
                rt.set_client_prone_cone(slot, vcod_common::pmove::ProneCone::of(&sim.ps));
                let rifle = self
                    .weapon_table
                    .get(sim.ps.weapon as usize)
                    .is_some_and(|d| d.sounds.rifle_bullet);
                rt.set_client_aim(slot, sim.ps.view().eye.into(), sim.aim_angles(), rifle);
                rt.aim_lookat(slot, sv_time_ms);
                let lmg = self
                    .weapon_table
                    .get(sim.ps.weapon as usize)
                    .filter(|d| sim.ps.rules == vcod_common::pmove::MoveRules::Uo && d.is_lmg())
                    .map(|d| crate::game::script::LmgHint {
                        origin: sim.ps.origin.into(),
                        yaw_deg: sim.view_angles()[1],
                        sighted: sim.ps.ads_active,
                        hint_string: Some(d.use_hint_string.as_str()).filter(|s| !s.is_empty()),
                        boxes: &mount_boxes,
                    });
                let h = rt.cursor_hints(slot, sim.ps.view().eye.into(), sim.view_angles(), lmg);
                sim.set_cursor_hint(h.hint, h.val);
                if let Some(string) = h.string {
                    sim.cursor_hint_string = string;
                }
                (sim.head_icon, sim.head_icon_team) = rt.head_icon(slot);
                // `BG_PlayerAnimation` (0x41486) runs after this slot's
                // aim trace: a higher slot's trace meets this frame's
                // pose, a lower one's met the last (combat doc 16.1).
                if let Some(c) = rt.dobj_record(slot) {
                    sim.keep_eased(c);
                }
                sim.commit_pose(
                    crate::spectate::EndFrame {
                        frametime_ms: FRAME_MS,
                        command_time: c.last_processed_st,
                        now_ms: sv_time_ms,
                        client_num: slot as i32,
                    },
                    swing_speed,
                    self.anims.as_deref(),
                );
                rt.set_client_body(slot, sim.hit_body(slot));
                rt.set_client_dobj(slot, sim.dobj(slot));
                // A gun freed this frame linked its gunner in the freeing
                // thread (`GameHost::free_entity`); only the sim half is left.
                rt.apply_turret_releases(slot, sim);
                let shot = rt.turret_think_client(
                    slot,
                    sim,
                    buttons & vcod_common::net::msg::BUTTON_ATTACK != 0,
                    self.anims.as_deref().map(|a| (a, &mut self.hit_rigs)),
                );
                sim.end_frame_entity = sim.linked().then(|| {
                    let e = sim.to_entity(proto, slot, c.last_processed_st);
                    (sv_time_ms, e)
                });
                let Some(shot) = shot else {
                    continue;
                };
                self.driver.noises.push(crate::bots::Noise {
                    at: shot.muzzle.into(),
                    source: shot.slot,
                    radius: crate::bots::HEAR_GUNFIRE,
                });
                let models = rt.shot_models();
                let sims: Vec<(usize, &crate::spectate::ClientSim)> = clients
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                    .collect();
                let collision = world.as_ref().map(|w| &w.collision);
                let mut bones = crate::game::combat::BoneTraceCtx::of(
                    fs,
                    self.anims.as_deref(),
                    &mut self.hit_rigs,
                    sv_time_ms,
                    &mut rt.host.dobj_posed,
                    vcod_common::patch::Arith::for_protocol(proto),
                );
                // 1.1 and 1.5 hand `G_Damage` the gunner as the inflictor,
                // so the callback is told his own weapon and `player_die`
                // credits the gun (turrets doc 12.6); UO hands it the gun,
                // whose weapon the callback is told (turrets doc 11).
                let uo = proto.is_uo();
                let gun = rt.host.turrets.iter().find(|(id, _)| id.0 == shot.gun);
                let carried = sims
                    .iter()
                    .find(|(s, _)| *s == shot.slot)
                    .map_or(0, |(_, sim)| sim.ps.weapon as usize);
                let weapon_name = match gun {
                    Some((_, rec)) if uo => rec.weapon.clone(),
                    _ => crate::items::item_name(crate::defs::Defs::of(proto), carried)
                        .unwrap_or_default()
                        .to_string(),
                };
                let gun_id = gun.map(|(id, _)| *id).filter(|_| uo);
                // UO's `Bullet_Fire_Extended` takes `Damage_Falloff` off the
                // turret's own weapon file (0x7b0f9; coduo-diff.md 21).
                let falloff = gun.filter(|_| uo).map(|(_, rec)| rec.def.falloff);
                let r = crate::game::combat::bullet_fire_from(
                    shot.slot,
                    shot.pass(),
                    shot.muzzle,
                    shot.dir,
                    crate::game::combat::bullet_range(proto),
                    shot.damage,
                    shot.rifle_bullet,
                    falloff,
                    &weapon_name,
                    &sims,
                    &rt.host.missiles.shot_targets(),
                    collision,
                    &models,
                    &self.hitlocs,
                    bones.as_mut(),
                );
                for &id in &r.struck {
                    rt.host.missiles.kill(id, sv_time_ms);
                }
                let mut effects = r.effects;
                // A world impact names the gun, `Bullet_Fire_Extended`'s
                // inflictor (1.1d 0x68b05), so the tracer leaves its tag.
                for e in &mut effects {
                    match e {
                        Effect::Impact(t) => t.other = shot.gun,
                        Effect::Hit(h) => h.inflictor = gun_id,
                        Effect::EntityHit(h) => h.inflictor = gun_id,
                        Effect::TriggerSweep(_) => {}
                    }
                }
                if proto.is_uo() {
                    // UO's turret round passes the gun as its own entity, so
                    // a world impact raises nothing and a round that met no
                    // player says so in its fire event's bit 7
                    // (`Bullet_Fire_Extended` 0x7a920, `turret_fire`
                    // 0x5aaa9; coduo-diff.md 21).
                    effects.retain(|e| !matches!(e, Effect::Impact(_)));
                    let hits: Vec<[f32; 3]> = effects
                        .iter()
                        .filter_map(|e| match e {
                            Effect::Hit(h) => Some(h.point),
                            _ => None,
                        })
                        .collect();
                    rt.uo_turret_round(shot.slot, shot.muzzle.into(), &hits);
                }
                deliver_turret_rounds(
                    clients,
                    rt,
                    effects,
                    slot,
                    self.anims.as_deref(),
                    &weapons,
                    rng,
                    sv_time_ms,
                );
            }
            rt.drop_turret_releases();
            // `CheckMatchTimeout`, past `CheckTeamStatus` and `CheckVote` at
            // the end of 1.5's `G_RunFrame`, on `trap_Milliseconds`.
            mirror_roster(clients, rt);
            crate::tick_trace::step!("CheckVote");
            vote_lines = rt.check_vote(svs_ms).0;
            timeout_written = rt.host.patch_1_5 && rt.host.check_match_timeout(svs_ms);
            console_lines = rt.take_console();
            client_commands = rt.take_client_commands();
            ranks_dirty = rt.take_ranks_dirty();
        }
        Ok(FrameOut {
            vote_lines,
            timeout_written,
            console_lines,
            client_commands,
            ranks_dirty,
        })
    }
}
