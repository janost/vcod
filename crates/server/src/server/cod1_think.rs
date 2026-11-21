//! CoD1's `ClientThink_real` for every packet since the last frame, run by
//! `Server::replay_moves`: the cmds in arrival
//! order, each one's shots, throws, falls and touches inside it (combat
//! doc 16; docs/dev/server-tick.md, "CoD1's frame").

use super::cod1_game::{Cod1Module, Lend};
use super::*;
use vcod_common::movetrace::{Body, MoveWorld};
use vcod_common::pmove::PmEvent;

/// The tables one tick's replay reads and writes across every cmd, built
/// once ahead of the first.
struct Think {
    /// Every linked client's body, the mover's own rewritten after each
    /// of its steps: retail relinks after each `Pmove`. A blocked push
    /// leaves its pair out of the tree until each one's next cmd
    /// (movers doc 12, "The candidate trace").
    bodies: Vec<Body>,
    /// The server's pmove point contents meet the mount triggers
    /// (coduo-movement.md 11).
    mount_boxes: Vec<vcod_common::movetrace::ContentsBox>,
    weapons: std::rc::Rc<crate::weapons::WeaponTable>,
    now_ms: i32,
    /// The `level.time` a cmd runs on, the last frame's.
    level_ms: i32,
    fall_heights: FallHeights,
    max_clients: usize,
    moved: Vec<MoveSummary>,
    rounds: Vec<Round>,
    /// A client past the per-tick cap has had its middle cmds dropped.
    capped: Vec<bool>,
    /// `SpectatorThink` reads the other clients' own-view bits as the
    /// last end frame left them, and each spectator's forced follow.
    followable_now: Vec<bool>,
    spectating: Vec<Option<(i32, u8)>>,
    /// 1.5's `Cmd_FollowCycle_f` also asks whether the spectator may watch
    /// the candidate's team.
    teams_now: Vec<i32>,
    use_held: Vec<bool>,
}

impl Cod1Module {
    /// `SV_ExecuteClientMessage` for every packet since the last tick, in the
    /// order they arrived: a packet's `kill` first, then one pmove step per
    /// usercmd, dt off the cmd clocks, matching the client's own prediction.
    /// Each cmd is `ClientThink_real`: the move, then the shots and swings it
    /// raised, traced and delivered to the damage callback there and then
    /// against every client as its own packets so far left it, then the touch
    /// pass (combat doc, 16). A throw spawns its missile there too. Returns
    /// what each slot replayed, for the trace line `send_client_snapshot` writes.
    pub(super) fn client_think_cod1(&mut self, eng: &mut Lend<'_>) -> Vec<MoveSummary> {
        let mut tick = self.think_tables(eng);
        let mut stamp_packet: Option<u64> = None;
        while let Some((packet, is_cmd, slot)) = self.next_item(eng) {
            // `SV_PacketEvent` bumps the skeleton stamp ahead of each
            // packet (`cod_lnxded` 0x0808c870): the rounds its cmds fire
            // pose the bodies under a build of their own (movers doc 15).
            if stamp_packet != Some(packet) {
                stamp_packet = Some(packet);
                if let Some(rt) = self.script.as_mut() {
                    rt.clear_dobj_stamps();
                }
            }
            if is_cmd {
                self.client_think(eng, &mut tick, slot, packet);
            } else {
                self.client_command(eng, &mut tick, slot, packet);
            }
        }
        // A test's stand-in for this frame's script `setOrigin`, queued
        // past the moves ([`Server::test_script_set_origin`]).
        if let Some(rt) = self.script.as_mut() {
            rt.host.client_sim_ops.append(&mut self.test_sim_ops);
        }
        for (slot, round) in tick.rounds.iter_mut().enumerate() {
            self.close_round(eng, slot, round);
        }
        self.mirror_tick_end(eng);
        tick.moved
    }

    /// The tick's tables, read off the clients and the host as the last end
    /// frame left them.
    fn think_tables(&mut self, eng: &mut Lend<'_>) -> Think {
        let area = self.script.as_ref().map(|rt| &rt.host.area);
        let bodies: Vec<Body> = eng
            .clients
            .iter()
            .enumerate()
            .filter(|(s, _)| area.is_none_or(|a| a.last_link(*s as u32).is_some()))
            .filter_map(|(s, c)| c.as_ref()?.sim.as_ref()?.body(s as u32))
            .collect();
        let mount_boxes = self
            .script
            .as_mut()
            .map_or_else(Vec::new, |rt| rt.mount_boxes());
        let weapons = self.weapon_table.clone();
        let now_ms = eng.sv_time_ms;
        let fall_heights = self.fall_heights;
        let max_clients = eng.clients.len();
        let moved = vec![MoveSummary::default(); max_clients];
        let rounds: Vec<Round> = (0..max_clients).map(|_| Round::default()).collect();
        let capped = vec![false; max_clients];
        let followable_now: Vec<bool> = (0..max_clients)
            .map(|slot| followable(eng.clients, slot))
            .collect();
        let spectating: Vec<Option<(i32, u8)>> = (0..max_clients)
            .map(|slot| {
                let sim = eng.clients[slot].as_ref()?.sim.as_ref()?;
                let s = session_of(self.script.as_mut(), slot, sim);
                (s.state == follow::SessionState::Spectator)
                    .then_some((s.spectator_client, s.no_spectate))
            })
            .collect();
        let teams_now: Vec<i32> = (0..max_clients)
            .map(|slot| self.script.as_mut().map_or(0, |rt| rt.client_team(slot)))
            .collect();
        let use_held: Vec<bool> = (0..max_clients)
            .map(|slot| {
                self.script.as_ref().is_some_and(|rt| {
                    rt.client_old_buttons(slot) & vcod_common::net::msg::BUTTON_USE != 0
                })
            })
            .collect();
        Think {
            bodies,
            mount_boxes,
            weapons,
            now_ms,
            level_ms: now_ms.wrapping_sub(FRAME_MS),
            fall_heights,
            max_clients,
            moved,
            rounds,
            capped,
            followable_now,
            spectating,
            teams_now,
            use_held,
        }
    }

    /// The oldest packet's next item across every client, its `kill` ahead
    /// of its cmds: `(packet, is_cmd, slot)`.
    fn next_item(&mut self, eng: &mut Lend<'_>) -> Option<(u64, bool, usize)> {
        let mut next: Option<(u64, bool, usize)> = None;
        for (slot, c) in eng.clients.iter_mut().enumerate() {
            let Some(c) = c.as_mut() else {
                continue;
            };
            if c.sim.is_none() {
                // A primed client's menu answer still reaches the game.
                for (_, command) in c.commands_at.drain(..) {
                    if let PacketCommand::MenuResponse(menu, response) = command
                        && let Some(rt) = self.script.as_mut()
                    {
                        rt.menu_response(slot, &menu, &response);
                    }
                }
                continue;
            }
            let kill = c.commands_at.front().map(|(p, _)| (*p, false));
            let cmd = c.pending.first().map(|q| (q.packet, true));
            if let Some(item) = kill.into_iter().chain(cmd).min()
                && next.is_none_or(|(p, is_cmd, _)| item < (p, is_cmd))
            {
                next = Some((item.0, item.1, slot));
            }
        }
        next
    }

    /// `SV_ExecuteClientCommand` for a packet's command ahead of its cmds.
    fn client_command(&mut self, eng: &mut Lend<'_>, tick: &mut Think, slot: usize, packet: u64) {
        let (_, command) = eng.clients[slot]
            .as_mut()
            .unwrap()
            .commands_at
            .pop_front()
            .unwrap();
        crate::tick_trace::step!("SV_ExecuteClientCommand slot {slot} packet {packet}");
        match command {
            PacketCommand::Kill => {
                self.close_round(eng, slot, &mut tick.rounds[slot]);
                self.run_kill(eng, slot, &tick.weapons);
                self.land_queued_ops(eng, &mut tick.bodies, &tick.weapons);
            }
            PacketCommand::CycleSeat { back } => self.cycle_seat(eng, slot, back),
            PacketCommand::MenuResponse(menu, response) => {
                if let Some(rt) = self.script.as_mut() {
                    rt.menu_response(slot, &menu, &response);
                }
            }
        }
        relink(&mut tick.bodies, eng.clients, slot);
    }

    /// `ClientThink_real` for `slot`'s next cmd, each step in retail's order
    /// (combat doc 16, player-clip doc 13.6, coduo-vehicles.md 11.6).
    fn client_think(&mut self, eng: &mut Lend<'_>, tick: &mut Think, slot: usize, packet: u64) {
        let Some((cmd, dt_ms)) = take_cmd(eng, tick, slot, packet) else {
            return;
        };
        let (held, frozen) = begin_cmd(eng, tick, slot, &cmd);
        // The cmd's `Pmove`: its mask and its touch list span its steps.
        let pm = vcod_common::movetrace::Pm::default();
        let overlap = if frozen {
            None
        } else {
            self.aim_and_overlap(eng, tick, slot, dt_ms)
        };
        // The run-over and the prone 10 land ahead of the pmove, before
        // anything the cmd raises: a kill there runs this cmd's pmove on
        // the corpse (coduo-vehicles.md 11.6).
        if let Some(o) = overlap.filter(|o| o.struck.is_some() || o.prone) {
            self.vehicle_overlap_hits(
                eng,
                slot,
                o,
                &mut tick.rounds,
                &mut tick.bodies,
                &tick.weapons,
            );
        }
        let (raised, raised_movetype) = if frozen {
            (Vec::new(), Vec::new())
        } else {
            pmove(eng, tick, slot, &cmd, overlap, &pm)
        };
        player_events(eng, &tick.weapons, slot, &raised);
        let (attacks, take) = self.client_events(eng, tick, slot, &cmd, &raised);
        let t = end_move(
            eng,
            tick,
            slot,
            cmd,
            held,
            &raised,
            raised_movetype,
            take,
            &pm,
        );
        self.use_press(eng, tick, slot, &cmd);
        self.mirror_cmd(eng, &t);
        for attack in attacks {
            self.fire(
                eng,
                attack,
                &mut tick.rounds,
                &mut tick.bodies,
                &tick.weapons,
            );
        }
        self.resnap_origin(eng, &t);
        self.touch_pass(eng, tick.now_ms, &t);
        self.land_cmd_ops(eng, tick, slot);
    }

    /// The aim block and, on UO, the vehicle overlap test, for a cmd whose
    /// client is not frozen on a follow.
    fn aim_and_overlap(
        &mut self,
        eng: &mut Lend<'_>,
        tick: &mut Think,
        slot: usize,
        dt_ms: i32,
    ) -> Option<crate::spectate::VehicleOverlap> {
        let collision = eng.world.as_ref().map(|w| &w.collision);
        let sim = eng.clients[slot].as_mut().unwrap().sim.as_mut().unwrap();
        // The aim block runs once per cmd, on the whole cmd, before
        // the chop (`ClientThink_real` 0x40169-0x40456), on the last
        // frame's `level.time` (combat doc 15.1).
        sim.update_aim(dt_ms, tick.level_ms, tick.weapons.defs());
        // A UO body inside a vehicle's collmap: the vehicle may sit
        // out this cmd's pmove (coduo-vehicles.md 11.6).
        match (collision, self.script.as_ref()) {
            (Some(w), Some(rt))
                if sim.ps.rules == vcod_common::pmove::MoveRules::Uo
                    && tick.spectating[slot].is_none() =>
            {
                sim.vehicle_overlap(MoveWorld::new(w, &tick.bodies, slot as u32), |e| {
                    let id = rt.host.ents.handle(e)?;
                    let v = rt.host.vehicles.live.get(&id)?;
                    Some((v.pose.origin.into(), v.velocity.into()))
                })
            }
            _ => None,
        }
    }

    /// `ClientEvents` (0x3fd24): this cmd's shots, swings, throws and
    /// falls, in the order it raised them, fired by the caller before its
    /// touch pass. Also the `clipOnly` weapon the last shot emptied.
    fn client_events(
        &mut self,
        eng: &mut Lend<'_>,
        tick: &Think,
        slot: usize,
        cmd: &UserCmd,
        raised: &[PmEvent],
    ) -> (Vec<Attack>, Option<u8>) {
        use vcod_common::pmove::weapon::{EV_FIRE_MELEE, EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT};
        use vcod_common::pmove::{EV_LANDING_PAIN_BASE, EV_LANDING_PAIN_LAST};
        let weapons = &tick.weapons;
        let now_ms = tick.now_ms;
        let sim = eng.clients[slot].as_ref().unwrap().sim.as_ref().unwrap();
        let mut take = None;
        let mut attacks: Vec<Attack> = Vec::new();
        for e in raised {
            let weapon = sim.ps.weapon;
            let grenade = weapons
                .get(weapon as usize)
                .is_some_and(|d| d.weapon_type == "grenade");
            let spotter = weapons
                .get(weapon as usize)
                .is_some_and(|d| d.weapon_type == "spotter");
            let gas = weapons
                .get(weapon as usize)
                .is_some_and(|d| d.weapon_type == "gas");
            let rocket = weapons
                .get(weapon as usize)
                .is_some_and(|d| d.weapon_type == "projectile");
            match e.event {
                // UO's `FireWeapon` (game.mp.uo 0x7bd05) does nothing
                // for weapon type 4, `gas`: the flamethrower's damage is
                // the burnt client's `fdc` report (coduo-diff.md 21).
                EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT if gas => {}
                EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT if spotter => {
                    attacks.push(Attack::Strike(Shot {
                        slot,
                        weapon,
                        ads: sim.ps.weapon_pos_frac == 1.0,
                        aim: sim.aim_angles(),
                        stance: vcod_common::pmove::weapon::SpreadStance::of(
                            &sim.ps,
                            cmd.server_time,
                            now_ms,
                        ),
                    }))
                }
                EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT if rocket => {
                    self.driver.noises.push(crate::bots::Noise {
                        at: (sim.ps.origin + glam::Vec3::Z * 40.0).into(),
                        source: slot,
                        radius: crate::bots::HEAR_GUNFIRE,
                    });
                    attacks.push(Attack::Rocket(Shot {
                        slot,
                        weapon,
                        ads: sim.ps.weapon_pos_frac == 1.0,
                        aim: sim.aim_angles(),
                        stance: vcod_common::pmove::weapon::SpreadStance::of(
                            &sim.ps,
                            cmd.server_time,
                            now_ms,
                        ),
                    }))
                }
                // A grenade's fire event is the throw, and the parm is
                // what is left of the fuse (combat doc, 1.11).
                EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT if grenade => {
                    attacks.push(Attack::Throw {
                        slot,
                        weapon,
                        fuse_left_ms: e.parm,
                        aim: sim.aim_angles(),
                    })
                }
                EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT => {
                    self.driver.noises.push(crate::bots::Noise {
                        at: (sim.ps.origin + glam::Vec3::Z * 40.0).into(),
                        source: slot,
                        radius: crate::bots::HEAR_GUNFIRE,
                    });
                    attacks.push(Attack::Shot(Shot {
                        slot,
                        weapon,
                        ads: sim.ps.weapon_pos_frac == 1.0,
                        aim: sim.aim_angles(),
                        stance: vcod_common::pmove::weapon::SpreadStance::of(
                            &sim.ps,
                            cmd.server_time,
                            now_ms,
                        ),
                    }));
                }
                EV_FIRE_MELEE => attacks.push(Attack::Swing {
                    slot,
                    weapon,
                    aim: sim.aim_angles(),
                }),
                ev if (EV_LANDING_PAIN_BASE..=EV_LANDING_PAIN_LAST).contains(&ev) => {
                    attacks.push(Attack::Fall {
                        slot,
                        percent: e.parm,
                    })
                }
                vcod_common::net::event_ids::EV_UO_GRENADE_SUICIDE => {
                    attacks.push(Attack::Suicide { slot })
                }
                _ => {}
            }
            // A `clipOnly` weapon with nothing left is taken away (combat
            // doc, 1.5 step 9), and 1.8's switch path then takes
            // `ps.weapon` to 0 on its own. Hung off the last shot, not
            // off `EV_NOAMMO`, which a dry trigger raises too and keeps
            // the weapon.
            if e.event == EV_FIRE_WEAPON_LASTSHOT
                && let Some(def) = weapons.get(weapon as usize)
                && def.clip_only
                && def.weapon_class != "spotter"
                && sim.ps.ammo[def.ammo_index] == 0
            {
                take = Some(weapon);
            }
        }
        (attacks, take)
    }

    /// A use press: the anim a use press's mount would change is picked off
    /// the state ahead of it.
    fn use_press(&mut self, eng: &mut Lend<'_>, tick: &mut Think, slot: usize, cmd: &UserCmd) {
        let use_down = cmd.buttons & vcod_common::net::msg::BUTTON_USE != 0;
        let pressed = use_down && !tick.use_held[slot];
        tick.use_held[slot] = use_down;
        if pressed && self.script.is_some() {
            self.close_round(eng, slot, &mut tick.rounds[slot]);
        }
    }

    /// What the cmd left, on the host before the callbacks and the
    /// touch pass read it: the host's copy is otherwise only mirrored
    /// from the sim after the script frame.
    fn mirror_cmd(&mut self, eng: &mut Lend<'_>, t: &Touched) {
        let proto = eng.proto;
        if let Some(rt) = self.script.as_mut() {
            let c = eng.clients[t.slot].as_ref().unwrap();
            let sim = c.sim.as_ref().unwrap();
            // The ammo among it is what the item pass reads; the pass
            // itself moves the host's copy as it grabs.
            mirror_for_callback(rt, sim, proto, t.slot, c.last_processed_st);
            // `r.currentOrigin` is the snapped `s.pos.trBase` through
            // `ClientEvents` and `G_TouchTriggers` (combat doc 2.1, 5.5):
            // what a callback or a death drop in them reads of the mover.
            rt.set_client_origin(t.slot, glam::Vec3::from(t.origin).trunc().into());
            if let Some(yaw) = t.yaw {
                rt.set_client_yaw(t.slot, yaw);
            }
            rt.set_client_pm_type(t.slot, t.pm_type);
            rt.set_client_on_ground(t.slot, t.on_ground);
            rt.set_client_stance(t.slot, t.stance);
            rt.set_client_prone_cone(t.slot, t.prone_cone);
            // Ahead of `weapon_changes`, which lands after the script
            // frame: a grab tests `ps.weapon` as this cmd left it.
            if let Some(w) = t.weapon {
                rt.set_client_weapon(t.slot, w);
            }
            if let Some(slots) = t.slots {
                rt.set_client_slots(t.slot, slots);
            }
            // Retail takes it inside `PM_Weapon`, ahead of the touch.
            if let Some(w) = t.take {
                rt.take_client_weapon(t.slot, w);
            }
        }
    }

    /// A callback in the cmd's attacks (a fall's damage) mirrored the sim's
    /// unsnapped origin back; the touch pass's callbacks still read
    /// the snapped one unless a `setOrigin` moved him.
    fn resnap_origin(&mut self, eng: &mut Lend<'_>, t: &Touched) {
        if let Some(rt) = self.script.as_mut()
            && let Some(sim) = eng.clients[t.slot].as_ref().and_then(|c| c.sim.as_ref())
            && sim.origin() == t.origin
        {
            rt.set_client_origin(t.slot, glam::Vec3::from(t.origin).trunc().into());
        }
    }

    /// Retail runs the touch pass per usercmd inside `ClientThink_real`
    /// (0x405b3), right after the link. The item half follows the
    /// trigger half, and the use key after both.
    fn touch_pass(&mut self, eng: &mut Lend<'_>, now_ms: i32, t: &Touched) {
        if let Some(rt) = self.script.as_mut() {
            // The link sits between `ClientEvents`, whose shots traced
            // against the last links, and `G_TouchTriggers` (0x40595).
            // `ps.origin` as a callback in `ClientEvents` left it: a
            // `setOrigin` there moved him (player-clip doc 13.6).
            let mut origin = t.origin;
            if let Some(sim) = eng.clients[t.slot].as_ref().and_then(|c| c.sim.as_ref()) {
                link_client(rt, t.slot, sim);
                origin = sim.origin();
            }
            let shift = glam::Vec3::from(origin) - glam::Vec3::from(t.origin);
            let eye = (glam::Vec3::from(t.eye) + shift).to_array();
            mirror_roster(eng.clients, rt);
            rt.touch_triggers_at(t.slot, now_ms, origin);
            // `ps.origin` back into `r.currentOrigin` past the touch
            // (0x405c7), ahead of the use key's `Cmd_Activate_f`. The
            // item half reads `ps.origin` either way.
            rt.set_client_origin(t.slot, origin);
            // `ClientImpacts` (0x40614), between the two.
            rt.client_impacts(t.slot, &t.impacts);
            rt.item_pass(t.slot, t.buttons, eye, t.view);
            // A vehicle's seat changes land inside the use cmd too.
            apply_seat_ops(eng.clients, rt);
            // The mount lands inside the use cmd (turrets doc 12.1), so
            // it reaches the sim before the next cmd's pass runs.
            let rule = crate::game::turret::AimRule::of(rt.host.defs.proto);
            for (turret, stance, view) in rt.take_turret_mounts(t.slot, origin, t.stance, t.view) {
                if let Some(sim) = eng.clients[t.slot].as_mut().and_then(|c| c.sim.as_mut()) {
                    crate::game::turret::mount_sim(sim, rule, turret, stance, view);
                }
            }
        }
    }

    /// The cmd's death and queued ops: what the passes queued for the mover
    /// reaches its sim before its next cmd, as `Touch_Item` and `G_Damage`
    /// write the playerstate inside this one (items doc, 13.2).
    fn land_cmd_ops(&mut self, eng: &mut Lend<'_>, tick: &mut Think, slot: usize) {
        let now_ms = tick.now_ms;
        // A `trigger_hurt` that killed the mover ran `player_die` inside
        // this touch pass, ahead of the cmds behind it.
        let died = self
            .script
            .as_ref()
            .is_some_and(|rt| rt.client_vitals(slot).dead)
            && eng.clients[slot]
                .as_ref()
                .and_then(|c| c.sim.as_ref())
                .is_some_and(|s| !s.dead && s.pm_type == crate::spectate::PmType::Normal);
        if died {
            self.close_round(eng, slot, &mut tick.rounds[slot]);
        }
        let st = eng.clients[slot]
            .as_ref()
            .map_or(0, |c| c.last_processed_st);
        let mut landed = died;
        if let (Some(rt), Some(sim)) = (
            self.script.as_mut(),
            eng.clients[slot].as_mut().and_then(|c| c.sim.as_mut()),
        ) && (died || rt.has_ops_of(slot))
        {
            apply_callback_ops(
                rt,
                sim,
                slot,
                self.anims.as_deref(),
                &tick.weapons,
                eng.rng,
                (now_ms, st),
            );
            landed = true;
        }
        if landed {
            relink(&mut tick.bodies, eng.clients, slot);
        }
        self.land_queued_ops(eng, &mut tick.bodies, &tick.weapons);
    }

    /// The state each player ended the tick in, mirrored onto the host for
    /// `cloneplayer` and for the bodies a scripted blast traces: a builtin
    /// cannot reach a sim, and the corpse is the dying player's entity
    /// state (`crate::game::bodies`). Written here,
    /// before the script frame, because that is the frame the script clones
    /// in; `snapshots_begin` re-reads a newborn body afterwards so the death
    /// animation the script raised after the clone still lands on it.
    fn mirror_tick_end(&mut self, eng: &mut Lend<'_>) {
        let proto = eng.proto;
        if let Some(rt) = self.script.as_mut() {
            for (slot, c) in eng.clients.iter().enumerate() {
                let sim = c
                    .as_ref()
                    .and_then(|c| Some((c.sim.as_ref()?, c.last_processed_st)));
                rt.set_client_entity_state(slot, sim.map(|(s, st)| s.to_entity(proto, slot, st)));
                // The cook a death drops, off the same state: both kill paths
                // run later in this tick, so what they read is this frame's
                // and not the last one's.
                rt.set_client_grenade_ms(slot, sim.map_or(0, |(s, _)| s.ps.grenade_time_left_ms));
                rt.set_client_body(slot, sim.and_then(|(s, _)| s.hit_body(slot)));
                rt.set_client_dobj(slot, sim.and_then(|(s, _)| s.dobj(slot)));
                if let Some((s, _)) = sim {
                    rt.set_client_height(slot, s.box_height());
                    rt.set_client_link_origin(slot, s.link_origin().into());
                }
            }
        }
    }

    /// The end of a run of one client's cmds: the animation the state and
    /// events they left imply, and the weapon the machine switched to.
    fn close_round(&mut self, eng: &mut Lend<'_>, slot: usize, round: &mut Round) {
        self.flush_anims(eng, slot, round);
        if let Some(sim) = eng.clients[slot].as_ref().and_then(|c| c.sim.as_ref())
            && round.held.is_some_and(|h| h != sim.ps.weapon)
        {
            self.weapon_changes.push((slot, sim.ps.weapon));
        }
        *round = Round::default();
    }

    /// The animation the client should be playing, from the state its cmds so
    /// far produced and the input that produced it.
    fn flush_anims(&mut self, eng: &mut Lend<'_>, slot: usize, round: &mut Round) {
        let events = std::mem::take(&mut round.events);
        let (Some(cmd), Some(anims)) = (round.last_cmd.take(), self.anims.as_deref()) else {
            return;
        };
        let Some(sim) = eng.clients[slot].as_mut().and_then(|c| c.sim.as_mut()) else {
            return;
        };
        let index = sim.ps.weapon as usize;
        sim.update_anims(
            &crate::spectate::AnimInputs {
                anims,
                weapon: crate::items::item_name(self.weapon_table.list(), index)
                    .unwrap_or_default(),
                weapon_class: self.weapon_table.class(index),
            },
            &cmd,
            (eng.sv_time_ms, cmd.server_time),
            &events,
            eng.rng,
        );
    }

    /// `G_VEH_CycleSlot` for a living rider (coduo-vehicles.md 11.3).
    fn cycle_seat(&mut self, eng: &mut Lend<'_>, slot: usize, back: bool) {
        if let Some(rt) = self.script.as_mut()
            && rt.client_vitals(slot).health > 0
        {
            rt.with_host(|host, cx| crate::game::vehicle::seat::cycle(host, cx, slot, back));
            apply_seat_ops(eng.clients, rt);
        }
    }

    /// `Cmd_Kill_f`, and what it did to the sim before the cmds behind it
    /// run: the drop takes the weapon they switch away from, and `EV_DEATH`
    /// goes on the ring ahead of their events. `pm_type` stays until the end
    /// frame, so they still move alive (combat doc, 9.2).
    fn run_kill(&mut self, eng: &mut Lend<'_>, slot: usize, weapons: &crate::weapons::WeaponTable) {
        self.callback(
            eng,
            slot,
            weapons,
            None,
            |_, _| Some(()),
            |rt, _, ()| rt.kill_client(slot),
        );
    }

    /// A damage or kill callback for `slot` inside a cmd, in the bracket
    /// retail's builtins meet it in: the roster and the slot's sim mirrored
    /// onto the host, `deliver`, then what the callback queued landed on the
    /// sim. `prepare` runs on the sim ahead of the mirror. A missing host,
    /// client or sim, `None` from `prepare` or `false` from `deliver` ends
    /// the bracket there. With `bodies`, the slot is relinked and every
    /// client's queued ops land after.
    fn callback<T>(
        &mut self,
        eng: &mut Lend<'_>,
        slot: usize,
        weapons: &crate::weapons::WeaponTable,
        bodies: Option<&mut Vec<Body>>,
        prepare: impl FnOnce(&mut script::ScriptRuntime, &mut ClientSim) -> Option<T>,
        deliver: impl FnOnce(&mut script::ScriptRuntime, &mut ClientSim, T) -> bool,
    ) {
        let proto = eng.proto;
        let now_ms = eng.sv_time_ms;
        let Some(rt) = self.script.as_mut() else {
            return;
        };
        mirror_roster(eng.clients, rt);
        let Some(c) = eng.clients[slot].as_mut() else {
            return;
        };
        let st = c.last_processed_st;
        let Some(sim) = c.sim.as_mut() else { return };
        let Some(prepared) = prepare(rt, sim) else {
            return;
        };
        mirror_for_callback(rt, sim, proto, slot, st);
        if !deliver(rt, sim, prepared) {
            return;
        }
        apply_callback_ops(
            rt,
            sim,
            slot,
            self.anims.as_deref(),
            weapons,
            eng.rng,
            (now_ms, st),
        );
        if let Some(bodies) = bodies {
            relink(bodies, eng.clients, slot);
            self.land_queued_ops(eng, bodies, weapons);
        }
    }

    /// What a callback inside a cmd queued for any other client, on that
    /// client's sim and body before the next cmd runs: retail's builtins
    /// write the playerstate there and then, so a player a callback moved
    /// meets the cmds behind it where it now stands (player-clip doc 13.6).
    fn land_queued_ops(
        &mut self,
        eng: &mut Lend<'_>,
        bodies: &mut Vec<vcod_common::movetrace::Body>,
        weapons: &crate::weapons::WeaponTable,
    ) {
        let now_ms = eng.sv_time_ms;
        let Some(rt) = self.script.as_mut() else {
            return;
        };
        for slot in 0..eng.clients.len() {
            if !rt.has_ops_of(slot) {
                continue;
            }
            let Some(c) = eng.clients[slot].as_mut() else {
                continue;
            };
            let st = c.last_processed_st;
            let Some(sim) = c.sim.as_mut() else { continue };
            apply_callback_ops(
                rt,
                sim,
                slot,
                self.anims.as_deref(),
                weapons,
                eng.rng,
                (now_ms, st),
            );
            relink(bodies, eng.clients, slot);
        }
    }

    /// `FireWeapon`'s grenade arm: `fire_grenade` from the muzzle this cmd
    /// left, stamped with the `level.time` a cmd runs under, the frame before
    /// the one being built (combat doc, 11.3 and 11.4).
    fn throw(
        &mut self,
        eng: &mut Lend<'_>,
        slot: usize,
        weapon: u8,
        fuse_left_ms: i32,
        aim: [f32; 2],
        weapons: &crate::weapons::WeaponTable,
    ) {
        let (Some(me), Some(def)) = (
            eng.clients[slot].as_ref().and_then(|c| c.sim.as_ref()),
            weapons.get(weapon as usize),
        ) else {
            return;
        };
        let (origin, velocity) = crate::game::missile::throw_velocity(&me.ps, aim, def);
        let carried = me.ps.velocity;
        // The projectile's model, indexed when the item was registered
        // (`GameHost::register_item`). A miss means the map load stopped
        // registering it and the client has no model to draw the grenade
        // with, which is silent on the wire.
        let name = def.projectile_model.as_deref().unwrap_or_default();
        let model =
            crate::configstrings::weapon_model_index(eng.configstrings, &eng.proto.cs, name);
        if model == 0 && !name.is_empty() {
            log::warn!("the grenade client {slot} threw carries {name:?}, which nothing precached");
        }
        let level_ms = eng.sv_time_ms.wrapping_sub(FRAME_MS);
        if let Some(rt) = self.script.as_mut() {
            rt.fire_grenade(
                slot,
                weapon,
                model,
                origin,
                velocity,
                carried,
                fuse_left_ms,
                level_ms,
            );
        }
    }

    /// `Weapon_RocketLauncher_Fire` (0x68c40, combat doc 13.6): a rocket from
    /// the muzzle along the spread aim 16 out (`combat::rocket_aim`), stamped
    /// with the `level.time` a cmd runs under, then the shooter pushed 64
    /// back along the view.
    fn launch(&mut self, eng: &mut Lend<'_>, shot: Shot, weapons: &crate::weapons::WeaponTable) {
        let Some(def) = weapons.get(shot.weapon as usize) else {
            return;
        };
        let Some(sim) = eng.clients[shot.slot].as_mut().and_then(|c| c.sim.as_mut()) else {
            return;
        };
        let rng = &mut eng.rng;
        let draw = self
            .spread_draws
            .pop_front()
            .unwrap_or_else(|| crate::game::combat::gun_random(rng));
        let (muzzle, forward, dir) =
            crate::game::combat::rocket_aim(sim, def, shot.ads, &shot.stance, shot.aim, draw);
        sim.ps.velocity += forward * ROCKET_KICK;
        let level_ms = eng.sv_time_ms.wrapping_sub(FRAME_MS);
        let Some(rt) = self.script.as_mut() else {
            return;
        };
        rt.with_host(|h, cx| {
            let shell = crate::game::missile::Shell {
                drop_at_ms: None,
                speed: def.projectile_speed,
                damage: def.damage,
                vehicle: None,
            };
            let kind = crate::game::missile::explosion_type(&def.proj_explosion_type);
            if let Err(e) = h.missiles.fire_rocket(
                &mut h.ents,
                cx,
                shot.slot,
                shot.weapon,
                muzzle,
                dir,
                shell,
                kind,
                false,
                level_ms,
            ) {
                log::warn!(
                    "the rocket client {} fired was not spawned: {e:?}",
                    shot.slot
                );
            }
        });
    }

    /// `Weapon_ArtilleryStrike_Fire` (game.mp.uo 0x7b758): the aim traced
    /// 10000 out with the weapon's spread; ground starts a barrage there,
    /// a miss gives the round back (`Add_Ammo(1)`; coduo-diff.md 21). Runs
    /// on the `level.time` a cmd runs under.
    fn strike(&mut self, eng: &mut Lend<'_>, shot: Shot, weapons: &crate::weapons::WeaponTable) {
        let Some(def) = weapons.get(shot.weapon as usize) else {
            return;
        };
        let models = self
            .script
            .as_mut()
            .map_or_else(Vec::new, |rt| rt.shot_models());
        let Some(me) = eng.clients[shot.slot].as_ref().and_then(|c| c.sim.as_ref()) else {
            return;
        };
        let (muzzle, _, end) = crate::game::combat::aim_end(
            me,
            def,
            shot.ads,
            &shot.stance,
            shot.aim,
            crate::game::combat::UO_BULLET_RANGE,
            None,
            eng.rng,
        );
        // Mask 0x2802031 is the bullet's: a live body stops it too.
        let sims: Vec<(usize, &crate::spectate::ClientSim)> = eng
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
            .collect();

        let targets = self
            .script
            .as_ref()
            .map(|rt| rt.host.missiles.shot_targets())
            .unwrap_or_default();
        let mut bones = crate::game::combat::BoneTraceCtx::of(
            eng.fs.as_deref(),
            self.anims.as_deref(),
            &mut self.hit_rigs,
            eng.sv_time_ms,
            dobj_stamps(self.script.as_mut()),
            vcod_common::patch::Arith::for_protocol(eng.proto),
        );
        let point = crate::game::combat::strike_point(
            muzzle,
            end,
            shot.slot,
            &sims,
            &targets,
            eng.world.as_ref().map(|w| &w.collision),
            &models,
            bones.as_mut(),
        );
        let level_ms = eng.sv_time_ms.wrapping_sub(FRAME_MS);
        match point {
            Some((at, false)) => {
                if let Some(rt) = self.script.as_mut() {
                    rt.fire_artillery_barrage(shot.slot, shot.weapon, at, level_ms);
                }
            }
            _ => {
                let Some(sim) = eng.clients[shot.slot].as_mut().and_then(|c| c.sim.as_mut()) else {
                    return;
                };
                let c = &mut sim.ps.ammoclip[def.clip_index];
                *c = (*c + 1).min(def.clip_size as i16);
            }
        }
    }

    /// `ClientEvents`' landing-pain arm (player-clip doc, 8.8), inside the cmd
    /// that landed: `pain_debounce_time` to `level.time + 200`, which keeps
    /// the end frame's `EV_PAIN` off a fall, then `G_Damage` with the percent
    /// taken of `ps.stats[2]` and no inflictor, attacker, point or direction.
    fn fall(
        &mut self,
        eng: &mut Lend<'_>,
        slot: usize,
        percent: i32,
        rounds: &mut [Round],
        bodies: &mut Vec<vcod_common::movetrace::Body>,
        weapons: &crate::weapons::WeaponTable,
    ) {
        // A zero share is the one `ClientEvents` skips (0x3fda8).
        if percent == 0 {
            return;
        }
        let now_ms = eng.sv_time_ms;
        let none = self.hitlocs.multiplier("none");
        self.close_round(eng, slot, &mut rounds[slot]);
        self.callback(
            eng,
            slot,
            weapons,
            Some(bodies),
            |_, sim| {
                // A cmd runs on the last frame's `level.time`.
                let level_ms = now_ms.wrapping_sub(FRAME_MS);
                sim.debounce_pain(level_ms.wrapping_add(FALL_PAIN_DEBOUNCE_MS));
                Some(crate::game::combat::located_damage(
                    fall_damage(percent, sim.max_health, sim.ps.rules.v15()),
                    none,
                ))
            },
            |rt, _, damage| {
                rt.deliver_fall(slot, damage);
                true
            },
        );
    }

    /// The damage `ClientThink_real` deals a body inside a vehicle ahead of
    /// its pmove (game.mp.uo 0x415e4..0x419c3, coduo-vehicles.md 11.6): a
    /// moving vehicle's run-over ([`run_over`]), and to a prone body, every
    /// cmd, `VEH_PlayerDamage(player, vehicle, 10)`, then `(player, player,
    /// 10)` when the first left his health alone. Its clock,
    /// `client+0x472c`, is written nowhere, so past the level's first
    /// 500 ms nothing holds it back.
    fn vehicle_overlap_hits(
        &mut self,
        eng: &mut Lend<'_>,
        slot: usize,
        overlap: crate::spectate::VehicleOverlap,
        rounds: &mut [Round],
        bodies: &mut Vec<vcod_common::movetrace::Body>,
        weapons: &crate::weapons::WeaponTable,
    ) {
        const PRONE_DAMAGE: i32 = 10;
        let now_ms = eng.sv_time_ms;
        self.close_round(eng, slot, &mut rounds[slot]);
        self.callback(
            eng,
            slot,
            weapons,
            Some(bodies),
            |rt, _| rt.host.ents.handle(overlap.entity),
            |rt, sim, veh| {
                // The cmd runs on the last frame's `level.time`.
                let level_ms = now_ms.wrapping_sub(FRAME_MS);
                if let Some(push) = overlap.struck {
                    run_over(rt, slot, veh, push, overlap.prone, overlap.at, level_ms);
                }
                if overlap.prone && 0 < level_ms - 500 {
                    let health = rt.client_vitals(slot).health;
                    vehicle_player_damage(rt, slot, veh, PRONE_DAMAGE, overlap.at);
                    if rt.client_vitals(slot).health == health {
                        // `VEH_PlayerDamage` of a non-vehicle: `G_Damage` with the
                        // player as inflictor and attacker, `MOD_SUICIDE`.
                        let weapon = crate::items::item_name(rt.host.defs, sim.ps.weapon as usize)
                            .unwrap_or("none")
                            .to_string();
                        rt.deliver_hits(vec![crate::game::combat::Hit {
                            victim: slot,
                            attacker: Some(crate::game::combat::Attacker::Client(slot)),
                            inflictor: None,
                            damage: PRONE_DAMAGE,
                            dflags: 0,
                            mod_: "MOD_SUICIDE",
                            weapon,
                            point: overlap.at.into(),
                            dir: [0.0; 3],
                            hitloc: "none",
                        }]);
                    }
                }
                true
            },
        );
    }

    /// `FireWeapon` inside the cmd that raised it: the trace against the
    /// world and every client as it stands now, and each impact and damage
    /// callback in the order the round met them, so a player one round kills
    /// is out of the way of the next (combat doc, 16). A throw spawns its
    /// missile here instead (11.4).
    fn fire(
        &mut self,
        eng: &mut Lend<'_>,
        attack: Attack,
        rounds: &mut [Round],
        bodies: &mut Vec<vcod_common::movetrace::Body>,
        weapons: &crate::weapons::WeaponTable,
    ) {
        let (attacker, weapon) = match attack {
            Attack::Shot(s) => (s.slot, s.weapon),
            Attack::Swing { slot, weapon, .. } => (slot, weapon),
            Attack::Throw {
                slot,
                weapon,
                fuse_left_ms,
                aim,
            } => {
                self.throw(eng, slot, weapon, fuse_left_ms, aim, weapons);
                return;
            }
            Attack::Fall { slot, percent } => {
                self.fall(eng, slot, percent, rounds, bodies, weapons);
                return;
            }
            Attack::Suicide { slot } => {
                self.run_kill(eng, slot, weapons);
                relink(bodies, eng.clients, slot);
                self.land_queued_ops(eng, bodies, weapons);
                return;
            }
            Attack::Strike(shot) => {
                self.strike(eng, shot, weapons);
                return;
            }
            Attack::Rocket(shot) => {
                self.launch(eng, shot, weapons);
                return;
            }
        };
        let Some(def) = weapons.get(weapon as usize) else {
            return;
        };
        // A UO round rolls its tracer on each leg (coduo-diff.md 21).
        let uo_round = match self.script.as_ref() {
            Some(rt) if eng.proto.is_uo() => {
                let heavy = matches!(def.ammo_type.as_str(), "lmg" | "hmg" | "umg");
                let chance = crate::game::builtins::cvar::atof(rt.cvars().get(if heavy {
                    "g_tracerChanceLMG"
                } else {
                    "g_tracerChance"
                }));
                Some(crate::game::combat::UoRound {
                    weapon: i32::from(weapon),
                    tracer_chance: chance,
                })
            }
            _ => None,
        };
        // Script models and UO's vehicles as the trace meets them (combat doc 2.7).
        let models = self
            .script
            .as_mut()
            .map_or_else(Vec::new, |rt| rt.shot_models());
        let targets = self
            .script
            .as_ref()
            .map(|rt| rt.host.missiles.shot_targets())
            .unwrap_or_default();
        // What a UO bipod's muzzle trace meets besides the world (combat doc 2.1).
        let deployed = eng.clients[attacker]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .is_some_and(|s| s.ps.deployed());
        let muzzle_boxes = match self.script.as_mut() {
            Some(rt) if eng.proto.is_uo() && deployed => {
                rt.muzzle_boxes(crate::game::combat::BIPOD_MUZZLE_MASK)
            }
            _ => Vec::new(),
        };
        let damageable = match (&attack, self.script.as_ref()) {
            (Attack::Swing { .. }, Some(rt)) => rt.host.damageable_brushes(),
            _ => Vec::new(),
        };
        // The bodies are posed off the last end frame (combat doc, 16.1).
        let result = {
            let sims: Vec<(usize, &crate::spectate::ClientSim)> = eng
                .clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| Some((i, c.as_ref()?.sim.as_ref()?)))
                .collect();
            let collision = eng.world.as_ref().map(|w| &w.collision);
            // The locational trace's context, absent on a host with no paks
            // or no animtree; a shot then lands at hit location `none`.
            let mut bones = crate::game::combat::BoneTraceCtx::of(
                eng.fs.as_deref(),
                self.anims.as_deref(),
                &mut self.hit_rigs,
                eng.sv_time_ms,
                dobj_stamps(self.script.as_mut()),
                vcod_common::patch::Arith::for_protocol(eng.proto),
            );
            let name = crate::items::item_name(weapons.list(), weapon as usize).unwrap_or_default();
            match attack {
                Attack::Shot(shot) => crate::game::combat::bullet_fire(
                    attacker,
                    def,
                    name,
                    uo_round,
                    shot.ads,
                    &shot.stance,
                    shot.aim,
                    &sims,
                    &targets,
                    collision,
                    &muzzle_boxes,
                    &models,
                    &self.hitlocs,
                    bones.as_mut(),
                    eng.rng,
                ),
                Attack::Swing { aim, .. } => crate::game::combat::melee_fire(
                    attacker,
                    def,
                    name,
                    weapon,
                    aim,
                    &sims,
                    &targets,
                    &damageable,
                    collision,
                    &models,
                    &self.hitlocs,
                    bones.as_mut(),
                    eng.rng,
                ),
                Attack::Throw { .. }
                | Attack::Fall { .. }
                | Attack::Suicide { .. }
                | Attack::Strike(_)
                | Attack::Rocket(_) => {
                    unreachable!()
                }
            }
        };
        let now_ms = eng.sv_time_ms;
        if let Some(rt) = self.script.as_mut() {
            // `G_Damage` inside the cmd, on the last frame's `level.time`.
            for id in result.struck {
                rt.host.missiles.kill(id, now_ms.wrapping_sub(FRAME_MS));
            }
        }
        for e in result.effects {
            let hit = match e {
                Effect::Impact(te) => {
                    if let Some(rt) = self.script.as_mut() {
                        rt.push_temp_entity(te);
                    }
                    continue;
                }
                Effect::EntityHit(h) => {
                    if let Some(rt) = self.script.as_mut() {
                        rt.deliver_entity_hit(&h);
                    }
                    continue;
                }
                Effect::TriggerSweep(t) => {
                    if let Some(rt) = self.script.as_mut() {
                        rt.trigger_sweep(&t);
                    }
                    continue;
                }
                Effect::Hit(h) => h,
            };
            let victim = hit.victim;
            // The victim's own cmds so far are its state now; the callback
            // lands between them and the ones behind.
            self.close_round(eng, victim, &mut rounds[victim]);
            self.callback(
                eng,
                victim,
                weapons,
                Some(&mut *bodies),
                |_, _| Some(()),
                |rt, _, ()| {
                    rt.deliver_hits(vec![hit]);
                    true
                },
            );
        }
    }
}

/// The cmd at the head of `slot`'s queue, its clock held to the frame's and
/// its span off the last processed one: `None` when the per-tick cap dropped
/// it or it is stale.
fn take_cmd(
    eng: &mut Lend<'_>,
    tick: &mut Think,
    slot: usize,
    packet: u64,
) -> Option<(UserCmd, i32)> {
    let c = eng.clients[slot].as_mut().unwrap();
    let m = &mut tick.moved[slot];
    if m.processed >= MAX_CMDS_PER_TICK && !tick.capped[slot] {
        // vcod's own bound, not retail's: only the newest cmd runs
        // on, and its chop covers the dropped span once from the
        // untouched `last_processed_st`.
        c.pending.drain(..c.pending.len() - 1);
        tick.capped[slot] = true;
        return None;
    }
    let mut cmd = c.pending.remove(0).cmd;
    crate::tick_trace::step!(
        "ClientThink slot {slot} packet {packet} cmd {}",
        cmd.server_time
    );
    let fresh = cmd.server_time > c.last_usercmd_st;
    if fresh {
        c.last_usercmd_st = cmd.server_time;
    }
    // `ClientThink_real` holds the cmd's clock within 1000 ms behind
    // and 200 ms ahead of the `level.time` it runs on, the last
    // frame's (player-clip doc 8.12).
    cmd.server_time = crate::game::hold_cmd_time(cmd.server_time, tick.level_ms);
    // Stale cmds (dt <= 0) are skipped whole; a long one is chopped
    // rather than clamped away.
    let dt_ms = cmd.server_time.wrapping_sub(c.last_processed_st);
    if dt_ms <= 0 {
        if let Some(sim) = c.sim.as_mut().filter(|_| fresh) {
            sim.stale_cmd(&cmd);
        }
        return None;
    }
    Some((cmd, dt_ms))
}

/// The cmd's head: the fall heights onto the playerstate, the weapon held
/// going in, and `SpectatorThink`. Returns that weapon and whether a follow
/// keeps the cmd out of its pmove.
fn begin_cmd(eng: &mut Lend<'_>, tick: &mut Think, slot: usize, cmd: &UserCmd) -> (u8, bool) {
    let collision = eng.world.as_ref().map(|w| &w.collision);
    let sim = eng.clients[slot].as_mut().unwrap().sim.as_mut().unwrap();
    sim.ps.fall_heights = tick.fall_heights;
    let round = &mut tick.rounds[slot];
    // What the client held going in, so a switch the machine made is
    // told apart from a playerstate reset between ticks.
    let held = *round.held.get_or_insert(sim.ps.weapon);
    let prev_buttons = std::mem::replace(&mut sim.last_buttons, cmd.buttons);
    // A spectator's buttons move its follow, and a follow the last
    // end frame landed runs no pmove at all.
    let mut frozen = false;
    if let Some((forced, no_spectate)) = tick.spectating[slot] {
        sim.spectator_think(
            forced,
            no_spectate,
            prev_buttons,
            cmd.buttons,
            tick.max_clients,
            |t| tick.followable_now[t] && follow::can_spectate(no_spectate, tick.teams_now[t]),
            collision,
        );
        frozen = sim.follow.on;
    }
    (held, frozen)
}

/// The cmd's `Pmove`, chopped into steps: the events they raised, each with
/// the movetype it selects its anim by.
fn pmove(
    eng: &mut Lend<'_>,
    tick: &mut Think,
    slot: usize,
    cmd: &UserCmd,
    overlap: Option<crate::spectate::VehicleOverlap>,
    pm: &vcod_common::movetrace::Pm,
) -> (Vec<PmEvent>, Vec<vcod_common::animscript::Movetype>) {
    let collision = eng.world.as_ref().map(|w| &w.collision);
    let c = eng.clients[slot].as_mut().unwrap();
    let sim = c.sim.as_mut().unwrap();
    let mut raised = Vec::new();
    let mut raised_movetype = Vec::new();
    let unlinked = match (overlap, collision) {
        (Some(o), Some(w)) if o.unlink => w.unlink_model_instance(o.entity),
        _ => None,
    };
    // A hitching client's gap is simulated, not discarded: see
    // `cmd::chop`.
    vcod_common::pmove::cmd::chop(c.last_processed_st, cmd, |step, dt| {
        let events = sim.step(
            step,
            dt,
            collision.map(|w| {
                MoveWorld::new(w, &tick.bodies, slot as u32)
                    .with_boxes(&tick.mount_boxes)
                    .in_pmove(pm)
            }),
            tick.weapons.defs(),
        );
        let movetype = sim.note_footsteps(step);
        raised_movetype.extend(events.iter().map(|_| movetype));
        raised.extend(events);
        tick.bodies.retain(|b| b.entity != slot as u32);
        tick.bodies.extend(sim.body(slot as u32));
        sim.ps.jump_latched
    });
    if let (Some(u), Some(w)) = (unlinked, collision) {
        w.relink_model_instance(u);
    }
    (raised, raised_movetype)
}

/// `BG_PlayerStateToEntityState` hands each new event to
/// `G_PlayerEvent` after `Pmove`, ahead of `ClientEvents`: on 1.5
/// and UO a fire event kicks the gun spring the next cmd's aim
/// reads (cod15-movement.md 6).
fn player_events(
    eng: &mut Lend<'_>,
    weapons: &crate::weapons::WeaponTable,
    slot: usize,
    raised: &[PmEvent],
) {
    use vcod_common::net::event_ids::{EV_FIRE_WEAPON_MG42, EV_FIRE_WEAPONB, EV_UO_FIRE_WEAPONC};
    use vcod_common::pmove::weapon::{EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT};
    let sim = eng.clients[slot].as_mut().unwrap().sim.as_mut().unwrap();
    for e in raised {
        if matches!(
            e.event,
            EV_FIRE_WEAPON
                | EV_FIRE_WEAPONB
                | EV_FIRE_WEAPON_LASTSHOT
                | EV_FIRE_WEAPON_MG42
                | EV_UO_FIRE_WEAPONC
        ) {
            sim.fire_recoil(weapons.defs(), || rand_from(eng.rng));
        }
    }
}

/// What the cmd left: its events onto the round, the [`Touched`] its host
/// mirror and touch pass read, the entity state and the cmd's clocks.
#[expect(clippy::too_many_arguments)]
fn end_move(
    eng: &mut Lend<'_>,
    tick: &mut Think,
    slot: usize,
    cmd: UserCmd,
    held: u8,
    raised: &[PmEvent],
    raised_movetype: Vec<vcod_common::animscript::Movetype>,
    take: Option<u8>,
    pm: &vcod_common::movetrace::Pm,
) -> Touched {
    use vcod_common::pmove::weapon::EV_WEAPON_ALT;
    let c = eng.clients[slot].as_mut().unwrap();
    let sim = c.sim.as_mut().unwrap();
    let round = &mut tick.rounds[slot];
    let m = &mut tick.moved[slot];
    let alt_switch = raised.iter().any(|e| e.event == EV_WEAPON_ALT);
    round.events.extend(
        raised
            .iter()
            .zip(raised_movetype)
            .map(|(&e, m)| (e, cmd.server_time, m)),
    );
    round.switched |= sim.ps.weapon != held;
    let t = Touched {
        slot,
        origin: sim.origin(),
        buttons: cmd.buttons,
        pm_type: sim.wire_pm_type(),
        on_ground: sim.on_ground(),
        yaw: (sim.pm_type == crate::spectate::PmType::Normal).then(|| sim.view_angles()[1]),
        weapon: round.switched.then_some(sim.ps.weapon),
        slots: alt_switch.then_some(sim.ps.weapon_slots),
        take,
        eye: sim.ps.view().eye.into(),
        view: sim.view_angles(),
        stance: sim.ps.stance,
        prone_cone: vcod_common::pmove::ProneCone::of(&sim.ps),
        // `SpectatorThink` runs no `ClientImpacts`.
        impacts: if sim.pm_type == crate::spectate::PmType::Normal {
            pm.touched()
        } else {
            Vec::new()
        },
    };
    round.last_cmd = Some(cmd);
    sim.pers_cmd = cmd;
    // `ClientThink_real`'s `BG_PlayerStateToEntityState`, past the
    // move and ahead of the use key.
    sim.write_entity();
    c.last_processed_st = cmd.server_time;
    m.first_cmd_st.get_or_insert(cmd.server_time);
    m.last_cmd_st = Some(cmd.server_time);
    m.last_buttons = Some(cmd.buttons);
    m.processed += 1;
    t
}
