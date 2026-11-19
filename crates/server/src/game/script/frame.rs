//! The frame: threads, trigger rounds and `G_RunFrame`'s entity pass.

use super::*;

/// Where `G_RunFrame`'s entity loop stands this frame
/// ([`ScriptRuntime::run_entity_pass`]).
pub struct EntityPass {
    next: u32,
    /// Every entity that has had its `G_RunEntity` this frame (0x502cb).
    ran: HashSet<EntId>,
    /// Each linked client's slot and parent, for `G_RunClient`'s turn.
    client_links: Vec<(usize, EntId)>,
}

/// A turn of the entity pass that needs the client sims, which
/// `Cod1Module::run_frame_cod1` holds: the pass returns it and is called again for the
/// next turn.
pub enum PassStop {
    /// A grenade went off on its turn (`G_RunMissile`); walk the blast.
    Blast(crate::game::missile::Explosion),
    /// A brush model mover moved on its turn (`G_MoverTeam`): push what it
    /// listed ([`ScriptRuntime::push_mover`]).
    Push(crate::game::mover::Step),
    /// A moving team master's turn: its push, if it has one, then its
    /// parts' ([`ScriptRuntime::team_move`]).
    Team(EntId, Option<crate::game::mover::Step>),
    /// `G_RunClient` (0x40660) for a linked client, its parent already run:
    /// re-anchor it.
    Client(usize),
}

impl ScriptRuntime {
    /// One server frame of script with no clients to trace a missile
    /// against: [`ScriptRuntime::run_threads`], then the whole entity pass.
    /// The blasts it sets off go unwalked and its movers push no one;
    /// `Cod1Module::run_frame_cod1` runs the halves itself.
    pub fn run_frame(&mut self, now_ms: i32) {
        let p = crate::tick_profile::phase("scripts/threads");
        self.run_threads(now_ms);
        drop(p);
        let _p = crate::tick_profile::phase("scripts/entity pass");
        let world = self.host.world.clone();
        let collision = world.as_deref().map(|w| &w.collision);
        let mut pass = self.begin_entity_pass(Vec::new());
        while let Some(stop) = self.run_entity_pass(&mut pass, collision, &[], now_ms) {
            match stop {
                // No bodies to block a mover: its push always goes through.
                PassStop::Push(step) => {
                    if let Some(c) = collision {
                        self.push_mover(&step, &mut [], c);
                    }
                }
                PassStop::Team(master, step) => {
                    if let Some(c) = collision {
                        self.team_move(master, step, &mut [], c);
                    }
                }
                PassStop::Blast(_) => self.spawn_concussive(),
                PassStop::Client(_) => {}
            }
        }
    }

    /// `G_RunFrame` up to its entity loop: the packet pass, `level.time`,
    /// the touch, item and mover notifies, and the thread pass.
    pub fn run_threads(&mut self, now_ms: i32) {
        // The packet pass, on the clock the netcode raised its events with:
        // retail runs `ClientBegin`, `ClientCommand` and `ClientDisconnect`
        // from `SV_ExecuteClientMessage` (map-cycle doc, 4.4), before
        // `G_RunFrame` advances `level.time`. A callback runs to its first
        // suspend inside its event; what it or a command woke, and a `wait 0`
        // it took, run in this frame's first pass below, picked with the
        // threads due on the clock (`client-probes/probe_threadorder`,
        // cod11-gsc-language.md, "The packet pass"). That is what parks
        // `sd.gsc`'s connect callback on its `waittill("menuresponse")`
        // before the `openMenu` it just queued can be answered; one frame
        // later and the answer notifies nothing and the client never leaves
        // the team menu.
        //
        // Client events in the order the netcode raised them: a `Begin`
        // drained ahead of its own `Connect` finds no thread parked on the
        // notify and strands the client silently.
        let mut _p = crate::tick_profile::phase("threads/client events");
        for ev in std::mem::take(&mut self.host.client_events) {
            self.dispatch_client_event(ev);
        }
        _p = crate::tick_profile::phase("threads/notifies");
        // `G_RunFrame` starts under a new skeleton stamp, with the DObjs
        // where the last frame left them.
        self.host.dobj_time_ms = self.host.level_time_ms;
        self.host.clear_dobj_stamps();
        self.host.level_time_ms = now_ms;
        self.host.ents.set_clock(now_ms);
        // The notifies retail raises where they happen: the last entity
        // pass's movers and the cmds' touches, use key and item takes. They
        // only wake their waiters, which run in the first round below with
        // the trigger queue's (docs/research/cod11-gsc-language.md, "Engine
        // notifies and the trigger queue").
        for (id, event, args) in std::mem::take(&mut self.host.engine_notifies) {
            if self.host.ents.get(id).is_some() {
                let event = self.vm.with_cx(|cx| cx.intern_folded(event));
                self.vm.notify(Target::Entity(id), event, &args);
            }
        }
        _p = crate::tick_profile::phase("threads/triggers");
        self.trigger_rounds();
        _p = crate::tick_profile::phase("threads/vm");
        // 1.1's second thread pass follows `G_DObjUpdateServerTime`; 1.5's
        // and UO's one pass runs before `G_XAnimUpdate` (1.5b 0x50821,
        // 0x50840; UO 0x59e4d, 0x59e6c), on the last frame's DObjs.
        if !self.host.profile.threads_on_last_dobjs {
            self.host.dobj_time_ms = now_ms;
        }
        // The VM logs each error itself (`Vm::record_error`).
        self.vm
            .run_frame(&mut self.host, now_ms - self.script_lag_ms);
        self.host.dobj_time_ms = now_ms;
    }

    /// `G_RunFrame`'s trigger queue (0x50540..0x50665), on the clock of the
    /// frame after the touch: retail's plant bar carries the
    /// `scaleStartTime` of the first snapshot it is on
    /// (docs/research/cod11-gsc-object-model.md 23.1). Each round notifies
    /// `"trigger"` once per entity, in queue order with a sent entry
    /// swapped for the last, keeps a repeat for the next round, and runs
    /// `Scr_RunCurrentThreads` (0x50653); the rounds go on while an entry
    /// was kept. An entry whose entity or activator is gone is dropped.
    /// What a round's threads queue joins the queue, and is dropped with it
    /// once a round keeps nothing (0x50665).
    fn trigger_rounds(&mut self) {
        let event = self.vm.with_cx(|cx| cx.intern_folded("trigger"));
        loop {
            let mut sent = HashSet::new();
            let mut kept = false;
            let mut i = 0;
            while i < self.host.trigger_fires.len() {
                let (id, other) = self.host.trigger_fires[i];
                if self.host.ents.get(id).is_some() && self.host.ents.get(other).is_some() {
                    if !sent.insert(id) {
                        kept = true;
                        i += 1;
                        continue;
                    }
                    self.vm
                        .notify(Target::Entity(id), event, &[Value::Entity(other)]);
                }
                self.host.trigger_fires.swap_remove(i);
            }
            self.vm.run_current(&mut self.host);
            if !kept {
                break;
            }
        }
        self.host.trigger_fires.clear();
    }

    /// The start of `G_RunFrame`'s entity loop (0x50912), after the threads
    /// and every client write they made. `client_links` is each linked
    /// client's slot and parent.
    pub fn begin_entity_pass(&mut self, client_links: Vec<(usize, EntId)>) -> EntityPass {
        crate::game::link::prune(&mut self.host);
        EntityPass {
            next: 0,
            ran: HashSet::new(),
            client_links,
        }
    }

    /// `G_RunFrame`'s entity loop (0x50930..0x50975): `G_RunEntity` once per
    /// in-use entity by number, its link parent first, so items, links,
    /// movers, missiles, thinks and linked clients interleave the way
    /// retail's do (combat doc 14.7). Returns at each turn that needs the
    /// client sims ([`PassStop`]), which the caller handles before calling
    /// again; `None` once the loop is past the last entity. An entity spawned
    /// on the way runs this frame when its number is still ahead of the loop.
    pub fn run_entity_pass(
        &mut self,
        pass: &mut EntityPass,
        world: Option<&vcod_common::collision::CollisionWorld>,
        sims: &[(usize, &crate::spectate::ClientSim)],
        now_ms: i32,
    ) -> Option<PassStop> {
        use crate::game::bodies::{BODY_FIRST, BODY_QUEUE_SIZE};
        // A `G_Error` longjmps out of the frame.
        while pass.next < self.host.ents.num_entities() && self.host.game_error.is_none() {
            let n = pass.next;
            // `G_RunClient`'s arm (0x50422), for the one case it does work
            // in: a linked client, re-anchored by the caller.
            if let Some(&(slot, parent)) = pass.client_links.iter().find(|(s, _)| *s == n as usize)
            {
                if pass.ran.insert(parent)
                    && let Some(x) = self.run_entity(&pass.ran, parent, world, sims, now_ms)
                {
                    return Some(x);
                }
                pass.next += 1;
                return Some(PassStop::Client(slot));
            }
            // The body queue is not in the object table; a corpse's only
            // work is its think.
            if (BODY_FIRST..BODY_FIRST + BODY_QUEUE_SIZE as u32).contains(&n) {
                self.host
                    .bodies
                    .run_think(n, now_ms, &vcod_common::net::protocol::PROTOCOL_V1);
                pass.next += 1;
                continue;
            }
            let Some(id) = self.host.ents.handle(n) else {
                pass.next += 1;
                continue;
            };
            // 0x50939..0x50949. The parent's turn stamps it, so its own
            // turn later in the loop is a no-op (0x502cb).
            if let Some(parent) = self.host.links.get(id).map(|l| l.parent)
                && pass.ran.insert(parent)
                && let Some(x) = self.run_entity(&pass.ran, parent, world, sims, now_ms)
            {
                // `pass.next` stays, so the next call runs the child.
                return Some(x);
            }
            pass.next += 1;
            if pass.ran.insert(id)
                && let Some(x) = self.run_entity(&pass.ran, id, world, sims, now_ms)
            {
                return Some(x);
            }
        }
        // The end frames' traces stay under `G_RunFrame`'s stamp; the next
        // client packet starts its own.
        None
    }

    /// One `G_RunEntity` (0x502bc): a temp entity's free once its event has
    /// aged out (0x502f9..0x5031d), then the first arm the entity takes: a
    /// missile (0x50375), a link (0x50392, and `G_RunMover`'s 0x57623), a
    /// mover (0x5040e) or an item (0x503f5), and its think after.
    fn run_entity(
        &mut self,
        ran: &HashSet<EntId>,
        id: EntId,
        world: Option<&vcod_common::collision::CollisionWorld>,
        sims: &[(usize, &crate::spectate::ClientSim)],
        now_ms: i32,
    ) -> Option<PassStop> {
        use crate::game::missile::Turn;
        // A vehicle's turn is its think (`G_RunThink`): an undriven one's
        // 50 ms step runs here, by entity number among the rest
        // (coduo-vehicles.md 12, "The think's turn").
        if self.host.vehicles.live.contains_key(&id) {
            // A think script set (a `delete`'s free) replaces the
            // vehicle's own.
            let _ = self.host.run_entity_think(id, now_ms);
            if !self.host.vehicles.live.contains_key(&id) {
                return None;
            }
            let inputs = crate::game::vehicle::think::rider_inputs(&self.host, sims);
            let host = &mut self.host;
            let ran = self
                .vm
                .with_cx(|cx| crate::game::vehicle::think::run_undriven(host, cx, id, &inputs));
            if ran {
                self.vehicle_deaths();
            }
            return None;
        }
        let host = &mut self.host;
        // A missile meets the script models and UO's vehicles where they
        // stand on its turn.
        let models = if host.missiles.contains(id) {
            self.vm.with_cx(|cx| host.shot_models(cx))
        } else {
            Vec::new()
        };
        let (fs, anims) = (host.fs.clone(), host.anims.clone());
        let mut bones = crate::game::combat::BoneTraceCtx::of(
            fs.as_deref(),
            anims.as_deref(),
            &mut host.hit_rigs,
            now_ms,
            &mut host.dobj_posed,
            vcod_common::patch::Arith::for_protocol(host.defs.proto),
        );
        // `G_RunMissile` ends in `G_RunThink`: a free set on the missile's
        // entity replaces its fuse (combat doc 13.8).
        if host.missiles.contains(id)
            && let Some(at) = host.ents.take_free_think(id)
        {
            host.missiles.schedule_free(id, at);
        }
        let grenade = host.missiles.grenade_at(id);
        let turn = host
            .missiles
            .run_one(id, world, sims, &models, bones.as_mut(), now_ms);
        // `G_GrenadeTouchTriggerDamage` over the move, after its link and
        // ahead of the fuse's think, charged to the grenade with its
        // `explosionInnerDamage` (combat doc 12.2, 14.8).
        if let Some((from, weapon)) = grenade
            && matches!(turn, Some(Turn::Flew | Turn::Exploded(_)))
            && let Some(to) = host.missiles.origin_of(id)
        {
            let damage = host
                .weapons
                .get(weapon as usize)
                .map_or(0, |d| d.explosion_inner_damage);
            let grenade_mod = host.defs.mod_index("MOD_GRENADE").unwrap_or(3);
            self.vm.with_cx(|cx| {
                crate::game::damage_trigger::sweep(
                    host,
                    cx,
                    id,
                    from,
                    to,
                    damage,
                    grenade_mod,
                    true,
                )
            });
        }
        // A water entry splash is `G_TempEntity`'d inside the move (12.1).
        for te in host.missiles.take_splashes() {
            self.vm.with_cx(|cx| host.add_temp_entity(cx, te));
        }
        match turn {
            Some(Turn::Flew) => {
                host.missiles.sync_origin(&mut host.ents, id);
                host.link_shootable(id);
                return None;
            }
            Some(Turn::Exploded(x)) => {
                host.missiles.sync_origin(&mut host.ents, id);
                if x.struck
                    .and_then(|n| host.ents.handle(n))
                    .is_some_and(|v| host.vehicles.live.contains_key(&v))
                {
                    host.missiles.struck_vehicle(id);
                }
                return Some(PassStop::Blast(x));
            }
            Some(Turn::Freed) => {
                host.free_entity(id);
                return None;
            }
            None => {}
        }
        if host.run_temp_entity(id, now_ms) {
            return None;
        }
        if self.vm.with_cx(|cx| host.run_barrage(cx, id, now_ms)) {
            return None;
        }
        // A parent that has not had its turn is where script left it.
        let fresh = crate::game::link::fresh(host, ran, id);
        let (done, step, think) = self.vm.with_cx(|cx| {
            if crate::game::link::run_linked(host, cx, id, fresh) {
                return (Vec::new(), None, true);
            }
            if crate::game::teams::idle_slave(host, cx, id) {
                return (Vec::new(), None, false);
            }
            if crate::game::mover::is_mover(host, id) {
                use crate::game::mover::Turn;
                let (done, turn) = crate::game::mover::run_turn(host, cx, id);
                let stop = match turn {
                    Turn::Done => None,
                    Turn::Push(step) => Some(PassStop::Push(step)),
                    Turn::Team(step) => Some(PassStop::Team(id, step)),
                };
                return (done, stop, true);
            }
            crate::game::item::run_item(host, cx, id, now_ms);
            (Vec::new(), None, true)
        });
        // `G_RunThink`, which every arm past the missile's ends on.
        if think && host.run_entity_think(id, now_ms).is_some() {
            self.vm
                .with_cx(|cx| crate::game::item::misc_spawner_think(host, cx, id));
        }
        host.run_turret_think(id);
        // Raised inside the pass, run by the next frame's threads.
        for d in done {
            host.engine_notifies.push((d.ent, d.event, Vec::new()));
        }
        step
    }
}
