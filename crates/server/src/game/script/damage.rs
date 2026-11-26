//! Damage into script: hits, blasts, falls, kills and who a hit is
//! charged to.

use super::*;
use crate::game::combat::Attacker;
use vcod_gsc::Atom;

/// `CodeCallback_PlayerDamage`'s nine arguments, in its order (combat doc
/// 4.4). Which a caller leaves undefined is retail's, per caller.
pub(crate) struct PlayerDamage {
    pub inflictor: Value,
    pub attacker: Value,
    pub damage: i32,
    pub dflags: i32,
    pub mod_: Atom,
    pub weapon: Atom,
    pub point: Value,
    pub dir: Value,
    pub hitloc: Atom,
}

impl PlayerDamage {
    pub fn into_args(self) -> Vec<Value> {
        vec![
            self.inflictor,
            self.attacker,
            Value::Int(self.damage),
            Value::Int(self.dflags),
            Value::String(self.mod_),
            Value::String(self.weapon),
            self.point,
            self.dir,
            Value::String(self.hitloc),
        ]
    }
}

impl ScriptRuntime {
    /// Starts `CodeCallback_PlayerDamage` on `victim` with `args`
    /// ([`PlayerDamage::into_args`]); a failed start is logged.
    fn player_damage(&mut self, victim: EntId, args: Vec<Value>) {
        if let Err(e) = self.start_with_args(
            CALLBACK_SETUP,
            "CodeCallback_PlayerDamage",
            Some(Target::Entity(victim)),
            args,
        ) {
            log::error!("gsc: {e:#}");
        }
    }

    /// `Scr_PlayerDamage` (combat doc, section 4.4) for every hit this
    /// frame's attacks made: `CodeCallback_PlayerDamage` on the victim's
    /// entity with the nine arguments. The inflictor is the attacker himself
    /// unless the hit names one, which a blast does: there the missile that
    /// went off is handed over, still alive for as long as its explode event
    /// rides the wire. A hit on a slot with no entity, or from one, is
    /// dropped: there is nobody to call and nobody to name. A NULL attacker
    /// (`None`) is a missile's whose thrower left, delivered with an
    /// undefined attacker (and on UO an undefined inflictor). So is one on a
    /// victim without `takedamage`, `G_Damage`'s first test (4.2 step 1),
    /// and one a vehicle seat shuts out ([`crate::game::vehicle::Vehicles::occupant_damage`]).
    pub fn deliver_hits(&mut self, hits: Vec<crate::game::combat::Hit>) {
        for mut hit in hits {
            if !self.client_vitals(hit.victim).takedamage {
                continue;
            }
            let Some(victim) = self.client_entity(hit.victim) else {
                continue;
            };
            // A missile whose thrower left hands `G_Damage` a NULL attacker,
            // which `Scr_PlayerDamage` passes as undefined (combat doc 13.7).
            // UO's drops the inflictor too, which `GScr_AddEntity` pushes as
            // undefined.
            let (attacker, inflictor) = match (
                hit.attacker.and_then(|a| self.attacker_entity(a)),
                hit.inflictor,
            ) {
                (Some(a), i) => (Value::Entity(a), Some(i.unwrap_or(a))),
                // A zeroed attacker with no entity 0 in use: retail pushes
                // the free entity's object, which vcod has none of.
                (None, i) if matches!(hit.attacker, None | Some(Attacker::Unspawned)) => {
                    (Value::Undefined, i)
                }
                _ => continue,
            };
            let Some(damage) = self
                .host
                .vehicles
                .occupant_damage(hit.victim, inflictor, hit.damage, hit.dflags)
            else {
                continue;
            };
            hit.damage = damage;
            let (mod_, weapon, hitloc) = self.vm.with_cx(|cx| {
                (
                    cx.intern_exact(hit.mod_),
                    cx.intern_exact(&hit.weapon),
                    cx.intern_exact(hit.hitloc),
                )
            });
            let args = PlayerDamage {
                inflictor: inflictor.map_or(Value::Undefined, Value::Entity),
                attacker,
                damage: hit.damage,
                dflags: hit.dflags,
                mod_,
                weapon,
                point: Value::Vector(hit.point),
                dir: Value::Vector(hit.dir),
                hitloc,
            };
            self.player_damage(victim, args.into_args());
        }
    }

    /// A round's, a swing's or a shell's `G_CheckHitTriggerDamage`
    /// (`crate::game::damage_trigger::sweep`), charged to the attacker's
    /// client entity, or the world when it has none.
    pub fn trigger_sweep(&mut self, t: &crate::game::combat::TriggerSweep) {
        let attacker = t.attacker.and_then(|a| self.attacker_entity(a));
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let attacker = attacker.unwrap_or_else(|| host.ents.world(cx));
            let mod_ = host.defs.mod_index(t.mod_).unwrap_or(0);
            crate::game::damage_trigger::sweep(
                host, cx, attacker, t.start, t.end, t.damage, mod_, false,
            );
        });
    }

    /// A round's `G_Damage` on an entity with no client (combat doc 2.4,
    /// step 4): a UO vehicle's branch ([`crate::game::vehicle::vehicle_damage`])
    /// with the shot's forward as the direction, any other entity's generic
    /// arm. The notifies wake their waiters with the frame's threads.
    pub fn deliver_entity_hit(&mut self, hit: &crate::game::combat::EntityHit) {
        let id = hit.target;
        if self.host.ents.get(id).is_none() {
            return;
        }
        let attacker = hit.attacker.and_then(|a| self.attacker_entity(a));
        let host = &mut self.host;
        let notifies = self.vm.with_cx(|cx| {
            let world = host.ents.world(cx);
            let attacker = attacker.unwrap_or(world);
            let inflictor = hit.inflictor.unwrap_or(attacker);
            let out = if host.vehicles.live.contains_key(&id) {
                crate::game::vehicle::vehicle_damage(
                    host,
                    cx,
                    id,
                    Some(inflictor),
                    Some(attacker),
                    hit.dir,
                    hit.point,
                    hit.damage,
                    hit.dflags,
                    hit.mod_,
                )
            } else {
                let entity_hit = crate::game::host::EntityHit {
                    inflictor,
                    mod_: hit.mod_.to_string(),
                    dir: hit.dir,
                    point: hit.point,
                };
                host.damage_nonclient(cx, id, hit.damage, attacker, &entity_hit)
            };
            out.into_iter()
                .map(|(event, args)| (cx.intern_folded(event), args))
                .collect::<Vec<_>>()
        });
        for (event, args) in notifies {
            self.vm.notify(Target::Entity(id), event, &args);
        }
        self.vehicle_deaths();
    }

    /// `Scr_Vehicle_Die` for every vehicle death queued outside a builtin
    /// (a round in a cmd, a blast in the entity pass, a collision in the
    /// think): each rider's probe callback runs to its first `wait` before
    /// the next rider's ([`crate::game::vehicle::die`]).
    pub fn vehicle_deaths(&mut self) {
        loop {
            let host = &mut self.host;
            let next = self
                .vm
                .with_cx(|cx| crate::game::vehicle::die::drain(host, cx));
            for (on, event, args) in std::mem::take(&mut self.host.vehicles.die_notifies) {
                let event = self.vm.with_cx(|cx| cx.intern_folded(event));
                self.vm.notify(Target::Entity(on), event, &args);
            }
            match next {
                Some((victim, args)) => self.player_damage(victim, args),
                None if self.host.vehicles.deaths.is_empty()
                    && self.host.vehicles.dying.is_empty() =>
                {
                    return;
                }
                None => {}
            }
        }
    }

    /// Damage whose attacker is not a player. `hurt_touch` (0x64dc4) hands
    /// `G_Damage` the trigger entity for both the inflictor and the attacker
    /// and zero for the point and the direction, which the immediates it
    /// pushes show directly (VERIFIED), and `Scr_PlayerDamage` passes a null
    /// point or direction as undefined; the stock callback tests
    /// `isPlayer(attacker)`, so a non-player attacker is a shape the corpus
    /// already handles. A blocked mover's crush is the same call.
    pub fn deliver_world_hit(
        &mut self,
        victim_slot: usize,
        inflictor: EntId,
        damage: i32,
        dflags: i32,
        mod_: &str,
    ) {
        self.deliver_unowned_hit(
            victim_slot,
            [inflictor, inflictor],
            damage,
            dflags,
            mod_,
            "none",
            None,
            None,
        );
    }

    /// An entity's int field, 0 when it holds none.
    pub(super) fn int_field(&mut self, id: EntId, name: &str) -> i32 {
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            match vcod_gsc::Host::get_field(host, cx, id, atom) {
                Value::Int(i) => i,
                _ => 0,
            }
        })
    }

    /// `G_PlaySoundAliasAtPoint`: an `EV_SOUND_ALIAS` temp entity at `at`,
    /// culled by PVS (`G_Spawn` and no `svFlags` on 1.1d 0x67a2c,
    /// `G_TempEntity` on 1.5b and UO).
    pub(super) fn add_sound_at(&mut self, at: [f32; 3], parm: i32) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            host.add_temp_entity(
                cx,
                crate::game::temp_entity::TempEntity {
                    event: EV_SOUND_ALIAS,
                    parm,
                    surf_type: 0,
                    other: 0,
                    attacker: 0,
                    weapon: 0,
                    client_num: 0,
                    scale: 0,
                    origin: at,
                    scope: crate::game::temp_entity::Scope::Pvs,
                    quake: None,
                    origin2: None,
                },
            )
        });
    }

    /// Damage from `[inflictor, attacker]`, neither a player:
    /// [`Self::deliver_world_hit`]'s shape with a weapon, a point and a
    /// direction, as a driverless vehicle's `VEH_PlayerDamage` passes them
    /// (a zero direction, coduo-vehicles.md 11.6). `None` reaches the
    /// callback undefined (`Scr_PlayerDamage`, game.mp 0x5ca18), and an
    /// undefined direction makes the stock gametypes add
    /// `DAMAGE_NO_KNOCKBACK` (cod11-movers.md 21.7).
    #[expect(clippy::too_many_arguments)]
    pub fn deliver_unowned_hit(
        &mut self,
        victim_slot: usize,
        [inflictor, attacker]: [EntId; 2],
        damage: i32,
        dflags: i32,
        mod_: &str,
        weapon: &str,
        point: Option<[f32; 3]>,
        dir: Option<[f32; 3]>,
    ) {
        if !self.client_vitals(victim_slot).takedamage {
            return;
        }
        let Some(victim) = self.client_entity(victim_slot) else {
            return;
        };
        let Some(damage) =
            self.host
                .vehicles
                .occupant_damage(victim_slot, Some(inflictor), damage, dflags)
        else {
            return;
        };
        let (mod_, weapon, hitloc) = self.vm.with_cx(|cx| {
            (
                cx.intern_exact(mod_),
                cx.intern_exact(weapon),
                cx.intern_exact("none"),
            )
        });
        let args = PlayerDamage {
            inflictor: Value::Entity(inflictor),
            attacker: Value::Entity(attacker),
            damage,
            dflags,
            mod_,
            weapon,
            point: point.map_or(Value::Undefined, Value::Vector),
            dir: dir.map_or(Value::Undefined, Value::Vector),
            hitloc,
        };
        self.player_damage(victim, args.into_args());
    }

    /// `ClientEvents`' fall damage (player-clip doc 8.8): `G_Damage` with no
    /// inflictor, attacker, direction or point, means `MOD_FALLING` and hit
    /// location 0, which the damage callback reads as four undefined
    /// arguments, weapon `"none"` and hit location `"none"` (8.10).
    pub fn deliver_fall(&mut self, victim_slot: usize, damage: i32) {
        let Some(victim) = self.client_entity(victim_slot) else {
            return;
        };
        let (mod_, none) = self
            .vm
            .with_cx(|cx| (cx.intern_exact("MOD_FALLING"), cx.intern_exact("none")));
        let args = PlayerDamage {
            inflictor: Value::Undefined,
            attacker: Value::Undefined,
            damage,
            dflags: 0,
            mod_,
            weapon: none,
            point: Value::Undefined,
            dir: Value::Undefined,
            hitloc: none,
        };
        self.player_damage(victim, args.into_args());
    }

    /// `Cmd_Kill_f`: the `kill` client command, which is the `suicide` builtin
    /// reached from outside the VM. Same three effects (the vitals, the
    /// `Damaged` op the sim reads, and `CodeCallback_PlayerKilled` with
    /// `MOD_SUICIDE`), but started as a thread rather than spawned, since
    /// nothing is waiting on it. A dead or spectating client is ignored, the
    /// way the builtin ignores a dead one. Returns whether the kill ran.
    pub fn kill_client(&mut self, slot: usize) -> bool {
        let Some(id) = self.client_entity(slot) else {
            return false;
        };
        // `sess.sessionState` against 0 (0x495d7, cod11-combat.md "The two
        // suicides"): a spectator who played before still has health.
        if self
            .client_session(slot)
            .is_some_and(|s| s.state != crate::follow::SessionState::Playing)
        {
            return false;
        }
        // A client that never spawned has no health to take.
        if self
            .host
            .clients
            .get(slot)
            .map(|c| &c.vitals)
            .is_none_or(|v| v.max_health <= 0)
        {
            return false;
        }
        let host = &mut self.host;
        let Some(args) = self
            .vm
            .with_cx(|cx| crate::game::builtins::combat::suicide_effects(host, cx, slot))
        else {
            return false;
        };
        // `player_die`'s "death" ahead of the callback, as the builtin's.
        let death = self.vm.with_cx(|cx| cx.intern_folded("death"));
        self.vm.notify(Target::Entity(id), death, &args[1..2]);
        if let Err(e) = self.start_with_args(
            CALLBACK_SETUP,
            "CodeCallback_PlayerKilled",
            Some(Target::Entity(id)),
            args,
        ) {
            log::error!("gsc: {e:#}");
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.player_die_walk(cx, slot));
        true
    }

    /// The entity `Scr_AddEntity` pushes for a damage's attacker: the
    /// slot's client, a live UO vehicle (a shell's fuse blast), or entity 0
    /// for [`Attacker::Unspawned`] (combat doc 13.7). `None` for the world,
    /// which a caller names itself.
    pub fn attacker_entity(&self, attacker: Attacker) -> Option<EntId> {
        match attacker {
            Attacker::Client(slot) => self.client_entity(slot),
            Attacker::Entity(id) => self.host.vehicles.live.contains_key(&id).then_some(id),
            Attacker::Unspawned => self.host.ents.handle(0),
            Attacker::World => None,
        }
    }

    /// A blast's owner as `G_Damage` is handed it: a slot with no client
    /// left (a UO vehicle shell keeps its driver's `+0x330` after he left)
    /// is the freed entity there, whose number reads 0
    /// ([`Attacker::Unspawned`], combat doc 13.7). A live vehicle, a
    /// shell's fuse blast, stays.
    pub fn charged_owner(&self, owner: Option<Attacker>) -> Option<Attacker> {
        owner.map(|a| match self.attacker_entity(a) {
            Some(_) => a,
            None => Attacker::Unspawned,
        })
    }

    /// `GameHost::damage_entity` for a grenade's blast, `attacker` the
    /// thrower's slot; a thrower with no entity left reads as the world,
    /// `G_Damage`'s stand-in (combat doc, 4.2). The waiters run with the
    /// frame's threads.
    pub fn damage_entity(
        &mut self,
        victim: &crate::game::combat::EntityVictim,
        blast: &crate::game::combat::Blast,
        damage: i32,
        attacker: Attacker,
    ) {
        let id = victim.id;
        let attacker = self.attacker_entity(attacker);
        let host = &mut self.host;
        let notifies = self.vm.with_cx(|cx| {
            let attacker = attacker.unwrap_or_else(|| host.ents.world(cx));
            let hit = victim.hit(blast, blast.inflictor.unwrap_or(attacker));
            host.damage_nonclient(cx, id, damage, attacker, &hit)
        });
        // Raised inside the pass, run by the next frame's threads with the
        // trigger queue's round (combat doc 14.8).
        for (event, args) in notifies {
            self.host.engine_notifies.push((id, event, args));
        }
    }

    /// A blast's `G_Damage` on a UO vehicle (`crate::game::vehicle`):
    /// `attacker` the owner's slot, `inflictor` the missile or the vehicle
    /// that fired it, `at` the blast's origin.
    pub fn damage_vehicle(
        &mut self,
        id: EntId,
        damage: i32,
        attacker: Attacker,
        inflictor: EntId,
        at: [f32; 3],
        mod_: &str,
    ) {
        let attacker = self.attacker_entity(attacker);
        let host = &mut self.host;
        let notifies = self.vm.with_cx(|cx| {
            // `G_RadiusDamage` hands over `r.currentOrigin` less the blast,
            // lifted 24, and `G_Damage` normalizes it (game.mp.uo 0x5098b,
            // 0x4fedb).
            let origin = host.vec_field(cx, id, "origin").to_array();
            let dir = (glam::Vec3::from(origin) - glam::Vec3::from(at) + glam::Vec3::Z * 24.0)
                .normalize_or_zero()
                .to_array();
            crate::game::vehicle::vehicle_damage(
                host,
                cx,
                id,
                Some(inflictor),
                attacker,
                dir,
                at,
                damage,
                crate::game::combat::DFLAG_RADIUS,
                mod_,
            )
            .into_iter()
            .map(|(event, args)| (cx.intern_folded(event), args))
            .collect::<Vec<_>>()
        });
        for (event, args) in notifies {
            self.vm.notify(Target::Entity(id), event, &args);
        }
    }
}
