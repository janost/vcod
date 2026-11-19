//! Turret mounts, a gunner's think, rounds and releases.

use super::*;

impl ScriptRuntime {
    /// `turret_use` (0x52a9c)'s record half for the mount `item_pass` just
    /// queued for `slot`, off the state of the cmd that pressed use. The
    /// record is manned at once, so a second client's press in the same
    /// frame finds the gun busy. Returns what the sim half needs: the
    /// turret's entity number, its stance and the view to snap to.
    pub fn take_turret_mounts(
        &mut self,
        slot: usize,
        origin: [f32; 3],
        stance: vcod_common::pmove::Stance,
        view: [f32; 3],
    ) -> Vec<(u32, crate::game::turret::TurretStance, [f32; 3])> {
        let ops = std::mem::take(&mut self.host.turret_ops);
        let mut out = Vec::new();
        for op in ops {
            let TurretOp::Mount { slot: s, turret } = op else {
                self.host.turret_ops.push(op);
                continue;
            };
            debug_assert_eq!(s, slot, "a mount is drained after its own cmd's pass");
            let host = &mut self.host;
            let angles = self
                .vm
                .with_cx(|cx| crate::game::item::angles_of(host, cx, turret));
            let Some(rec) = self.host.turrets.get_mut(&turret) else {
                continue;
            };
            let view = crate::game::turret::mount(rec, s, origin, stance, view, angles);
            out.push((turret.0, rec.def.stance, view));
        }
        out
    }

    /// `turret_think_client` (0x52340, turrets doc 6) for the gun `slot`
    /// mans, once per server frame in `ClientEndFrame`: the aim, the fire
    /// and the loop sound, on the record, the gun's entity and the gunner's
    /// sim. `attack_held` is the frame's last cmd's attack bit. `body` is
    /// what the body placement reads the gunner's anims from; without it the
    /// body stays where it is. Returns the round the gun fired, for the
    /// server to trace.
    pub fn turret_think_client(
        &mut self,
        slot: usize,
        sim: &mut crate::spectate::ClientSim,
        attack_held: bool,
        body: Option<(
            &vcod_common::animtree::PlayerAnims,
            &mut crate::game::hitrig::HitRigs,
        )>,
    ) -> Option<TurretShot> {
        use crate::game::turret::{AimRule, aim, fire_tick, loop_tick, muzzle};
        // `ClientEndFrame` calls it only on the mounted bits (0x41494).
        sim.mounted_on?;
        let id = *self
            .host
            .turrets
            .iter()
            .find(|(_, r)| r.owner == Some(slot))?
            .0;
        // A gun asked to let go, or a gunner no longer playing (dead,
        // spectating), is released instead (0x5235c, 0x5236b).
        let playing = sim.pm_type == crate::spectate::PmType::Normal && !sim.dead;
        if self.host.turrets[&id].busy != 1 || !playing {
            for te in self.release_turret(slot, sim).unwrap_or_default() {
                self.push_temp_entity(te);
            }
            self.teleport_link(slot, sim);
            return None;
        }
        let host = &mut self.host;
        let (origin, angles) = self.vm.with_cx(|cx| {
            (
                crate::game::item::origin_of(host, cx, id),
                crate::game::item::angles_of(host, cx, id),
            )
        });
        let rec = self.host.turrets.get_mut(&id)?;
        let (loop_index, stop_index) = crate::game::turret::sound_indices(
            rec,
            &self.host.configstrings,
            self.host.allocators.layout(),
        );

        // 1.5's aim and gunner placement (0x62305) wait out a match timeout
        // with the fire.
        let rule = AimRule::of(self.host.defs.proto);
        let held = rule == AimRule::V15 && self.host.frozen;
        if !held {
            if rule.locks_view() {
                sim.viewlocked = 1;
                sim.gunfx = 0;
            }
            sim.viewlocked_ent = id.0;
            if let Some(view) = aim(rec, rule, sim.view_angles(), angles) {
                sim.set_view_angle(view);
            }
        }
        let placed = match (body, &rec.tags, self.host.fs.as_deref()) {
            (Some((anims, rigs)), Some(tags), Some(fs)) if !held => {
                let tag_weapon = crate::game::turret::tag_weapon_local(tags, rec.angles2);
                let turret = (
                    glam::Vec3::from(origin),
                    vcod_common::turretpose::angles_quat(angles),
                );
                // The leaves' root deltas at the group's own time: a firing
                // gunner's `fire_30left` leaves key a second spot (turrets
                // doc 7.2).
                let seconds = sim.legs_seconds(self.host.level_time_ms);
                vcod_common::turretpose::place_gunner_at(
                    vcod_common::turretpose::GunnerRule::Cod1,
                    anims,
                    |name| rigs.clip(fs, name),
                    sim.legs_anim(),
                    tag_weapon,
                    turret,
                    sim.ps.origin,
                    rec.def.anim_hor_rotate_inc,
                    seconds,
                )
            }
            _ => None,
        };
        // Held, the blend 0x61389 last set stands.
        if !held {
            sim.set_gunner_blend(placed.as_ref(), self.host.level_time_ms);
        }
        if let Some(mut at) = placed.as_ref().map(|g| g.origin) {
            if let Some(world) = &self.host.world {
                at = crate::game::turret::lift_onto_floor(&world.collision, at, origin[2]);
            }
            sim.ps.origin = at;
        }
        // 1.5's fire block (0x52305) runs only off a match timeout, its
        // cooldown left standing.
        sim.firing = !self.host.frozen && fire_tick(rec, attack_held);
        let mut shot = None;
        if sim.firing {
            if rule.locks_view() {
                sim.viewlocked = 2;
            }
            let forward = crate::game::spawn::angle_forward(rule.fire_angles(
                sim.view_angles(),
                rec.angles2,
                angles,
            ));
            // A missing tag skips the round and its event, not the flags.
            let at = match (rule, &rec.tags, &rec.gun) {
                (AimRule::V1, Some(tags), _) => {
                    Some(muzzle(tags, origin, angles, rec.angles2, forward))
                }
                (AimRule::V15 | AimRule::Uo, Some(_), Some(gun)) => Some(
                    crate::game::turret::muzzle_stored(gun, origin, angles, rec.angles2, forward),
                ),
                _ => None,
            };
            if let Some(at) = at {
                shot = Some(TurretShot {
                    slot,
                    gun: id.0,
                    muzzle: at.into(),
                    dir: forward.into(),
                    damage: rec.dmg,
                    rifle_bullet: rec.def.rifle_bullet,
                });
            }
        }
        let (loop_sound, stop) = loop_tick(rec, loop_index, self.host.frozen);
        let fired = shot.is_some();
        if let Some(ent) = self.host.ents.get_mut(id) {
            if fired {
                ent.events.add(EV_FIRE_WEAPON_MG42, 0);
            }
            ent.loop_sound = loop_sound;
            if stop {
                ent.events.add(EV_SOUND_ALIAS, stop_index);
            }
        }
        if let Some(g) = &placed {
            self.set_client_origin(slot, sim.origin());
            self.set_client_yaw(slot, g.yaw());
            // The placement's `AxisToAngles` is `r.currentAngles`, which
            // places the DObj for every trace until his next cmd writes
            // `(0, view yaw, 0)` back (turrets doc 13.10).
            if let Some(s) = self.host.dobj_posed.get_mut(slot) {
                s.set_angles(vcod_common::turretpose::quat_angles(g.rot));
            }
        }
        shot
    }

    /// The UO half of a turret round `slot` fired from `muzzle`
    /// (`Bullet_Fire_Extended`, game.mp.uo 0x7a920): with no player hit the
    /// gun's fire event takes parm 0x80 (the client traces the round
    /// itself); each player hit may draw an `EV_BULLET_TRACER` from the
    /// muzzle to it, `g_tracerChanceLMG` for an `lmg`/`hmg`/`umg` gun, else
    /// `g_tracerChance`.
    pub fn uo_turret_round(&mut self, slot: usize, muzzle: [f32; 3], hits: &[[f32; 3]]) {
        let Some((&id, rec)) = self
            .host
            .turrets
            .iter()
            .find(|(_, r)| r.owner == Some(slot))
        else {
            return;
        };
        let weapon = self.host.defs.weapon_index(&rec.weapon).unwrap_or(0) as i32;
        if hits.is_empty() {
            if let Some(ent) = self.host.ents.get_mut(id) {
                let i = (ent.events.seq.wrapping_sub(1) & 3) as usize;
                ent.events.parms[i] |= TURRET_ROUND_UNTRACED;
            }
            return;
        }
        let heavy = self
            .host
            .weapons
            .get(weapon as usize)
            .is_some_and(|d| matches!(d.ammo_type.as_str(), "lmg" | "hmg" | "umg"));
        let chance = crate::game::builtins::cvar::atof(self.host.cvars.get(if heavy {
            "g_tracerChanceLMG"
        } else {
            "g_tracerChance"
        }));
        for &at in hits {
            // A pinned outcome still takes its draw, so later draws stay put.
            let drawn = self.host.rand_unit() < chance;
            if self.host.tracer_pins.pop_front().unwrap_or(drawn) {
                self.push_temp_entity(crate::game::temp_entity::TempEntity {
                    event: vcod_common::net::event_ids::EV_BULLET_TRACER,
                    parm: weapon,
                    surf_type: 0,
                    other: 0,
                    attacker: 0,
                    weapon: 0,
                    client_num: 0,
                    scale: 0,
                    origin: muzzle,
                    scope: crate::game::temp_entity::Scope::Pvs,
                    quake: None,
                    origin2: Some(at),
                });
            }
        }
    }

    /// `G_ClientStopUsingTurret` (0x53054, turrets doc 8) on the gun `slot`
    /// mans, if any: the record freed, the gun's loop cut and the gunner put
    /// back where it mounted, its new origin mirrored to script. Returns
    /// `TeleportPlayer`'s temp entities for the caller to queue, `None` when
    /// `slot` mans no gun. The area tree's link is the caller's
    /// ([`ScriptRuntime::teleport_link`]).
    pub fn release_turret(
        &mut self,
        slot: usize,
        sim: &mut crate::spectate::ClientSim,
    ) -> Option<Vec<crate::game::temp_entity::TempEntity>> {
        let (id, rec) = self
            .host
            .turrets
            .iter_mut()
            .find(|(_, r)| r.owner == Some(slot))?;
        let id = *id;
        let (_, origin, stance) = crate::game::turret::release(rec)?;
        if let Some(ent) = self.host.ents.get_mut(id) {
            ent.loop_sound = 0;
        }
        let temps = crate::game::turret::release_sim(sim, slot, origin, stance);
        self.set_client_origin(slot, sim.origin());
        Some(temps)
    }

    /// `TeleportPlayer`'s link (1.1d 0x513dc..0x51477, 1.5b 0x50dc8,
    /// UO 0x5a3f8): a client the tree held is unlinked and linked again at
    /// the released origin, unsnapped, so it heads its node's list there
    /// until its next cmd links it (combat doc 14.7). One it did not hold
    /// stays out. `ClientSpawn`'s release skips this: its own unlink and
    /// link at the spawn point follow, which the `spawn` builtin made.
    pub fn teleport_link(&mut self, slot: usize, sim: &crate::spectate::ClientSim) {
        if self.host.area.last_link(slot as u32).is_none() {
            return;
        }
        let contents = self.host.area.contents(slot as u32);
        self.host.area.unlink(slot as u32);
        let playing = sim.pm_type == crate::spectate::PmType::Normal && !sim.dead;
        let origin = sim.link_origin().to_array();
        let bounds = (sim.ps.mins().to_array(), sim.ps.maxs().to_array());
        self.host
            .link_client(slot, origin, bounds, contents, playing);
        self.set_client_link_origin(slot, origin);
    }

    /// The sim half of every release `GameHost::free_entity` queued for
    /// `slot` (`G_FreeTurret`, turrets doc 8), each temp entity queued. The
    /// link was the freeing thread's.
    pub fn apply_turret_releases(&mut self, slot: usize, sim: &mut crate::spectate::ClientSim) {
        let ops = std::mem::take(&mut self.host.turret_ops);
        for op in ops {
            match op {
                TurretOp::Release {
                    slot: s,
                    origin,
                    stance,
                } if s == slot => {
                    for te in crate::game::turret::release_sim(sim, slot, origin, stance) {
                        self.push_temp_entity(te);
                    }
                    self.set_client_origin(slot, sim.origin());
                }
                op => self.host.turret_ops.push(op),
            }
        }
    }

    /// Drops the releases queued for a slot with no sim to apply them to.
    pub fn drop_turret_releases(&mut self) {
        self.host
            .turret_ops
            .retain(|op| !matches!(op, TurretOp::Release { .. }));
    }
}
