//! Mover pushes, team moves, blocked doors and link anchors.

use super::*;

impl ScriptRuntime {
    /// `G_MoverPush` over the players in `sims` and the items `step`
    /// listed (`crate::push::push_with`); a blocked push holds the mover a frame.
    /// A player a sine mover crushes takes `G_Damage(player, mover, mover,
    /// NULL, NULL, 99999, 0, MOD_CRUSH)` (game.mp 0x55617) before the
    /// mover's end-of-period test.
    pub fn push_mover(
        &mut self,
        step: &crate::game::mover::Step,
        sims: &mut [(usize, &mut crate::spectate::ClientSim)],
        world: &vcod_common::collision::CollisionWorld,
    ) {
        let host = &mut self.host;
        let crushed = self
            .vm
            .with_cx(|cx| crate::push::push_with(host, cx, step, sims, world, &mut Vec::new()));
        let crushed = match crushed {
            Ok(crushed) => crushed,
            Err(obstacle) => {
                let host = &mut self.host;
                self.vm
                    .with_cx(|cx| crate::game::mover::stall(host, cx, step));
                self.door_blocked(step.ent, obstacle);
                return;
            }
        };
        for slot in crushed {
            self.deliver_world_hit(slot, step.ent, CRUSH_DAMAGE, 0, "MOD_CRUSH");
        }
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::mover::reached(host, cx, step.ent));
    }

    /// `G_MoverTeam` (0x55684) for a moving team master: `first`, its own
    /// push, then each part evaluated and pushed in chain order, all under one
    /// pushed list. A block puts back every entity pushed so far, holds every
    /// part a frame (`mover::stall_team`) and calls the master's `blocked`
    /// with the player who stopped it; otherwise every part's `reached`
    /// runs, then the master's think.
    pub fn team_move(
        &mut self,
        master: EntId,
        first: Option<crate::game::mover::Step>,
        sims: &mut [(usize, &mut crate::spectate::ClientSim)],
        world: &vcod_common::collision::CollisionWorld,
    ) {
        use crate::game::mover::{self, door};
        let parts = door::chain(&self.host, master);
        let mut pushed = Vec::new();
        let mut steps: Vec<mover::Step> = Vec::new();
        for (i, &part) in parts.iter().enumerate() {
            let step = if i == 0 {
                first.clone()
            } else {
                let host = &mut self.host;
                let (done, step) = self.vm.with_cx(|cx| mover::evaluate_part(host, cx, part));
                for d in done {
                    self.host.engine_notifies.push((d.ent, d.event, Vec::new()));
                }
                step
            };
            let Some(step) = step else {
                continue;
            };
            let host = &mut self.host;
            let result = self
                .vm
                .with_cx(|cx| crate::push::push_with(host, cx, &step, sims, world, &mut pushed));
            steps.push(step.clone());
            match result {
                Ok(crushed) => {
                    for slot in crushed {
                        self.deliver_world_hit(slot, part, CRUSH_DAMAGE, 0, "MOD_CRUSH");
                    }
                }
                Err(obstacle) => {
                    let host = &mut self.host;
                    self.vm
                        .with_cx(|cx| mover::stall_team(host, cx, &parts, &steps));
                    self.door_blocked(master, obstacle);
                    return;
                }
            }
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            for &part in &parts {
                mover::reached_part(host, cx, part);
            }
            mover::run_think(host, cx, master);
        });
    }

    /// `ent->blocked(ent, obstacle)` for a door team stopped by the player in
    /// `slot`: `Blocked_Door`'s or `Blocked_DoorRotate`'s damage, then the
    /// team turns back.
    fn door_blocked(&mut self, master: EntId, slot: usize) {
        use crate::game::mover::door;
        if !door::has_blocked(&self.host, master) {
            return;
        }
        let dead = self
            .host
            .clients
            .get(slot)
            .map(|c| &c.vitals)
            .is_some_and(|v| v.health <= 0);
        for dmg in door::blocked_damage(&self.host, master, dead) {
            self.deliver_world_hit(slot, master, dmg, 0, "MOD_CRUSH");
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| door::reverse(host, cx, master));
    }

    /// Where a link parent is this frame, origin and angles, `None` once it
    /// has been freed. `G_RunFrame` runs a linked entity's parent ahead of it
    /// (0x50939-0x50955), so a mover parent is read on its own clock, the
    /// level time, a frame ahead of the `.origin` script reads
    /// (docs/research/cod11-movers.md, section 13).
    pub fn link_anchor(&mut self, id: EntId) -> Option<([f32; 3], [f32; 3])> {
        use vcod_gsc::Host;
        let origin = self.entity_origin_of(id)?;
        let host = &mut self.host;
        let angles = self.vm.with_cx(|cx| {
            let field = cx.intern_folded("angles");
            match host.get_field(cx, id, field) {
                Value::Vector(v) => v,
                _ => [0.0; 3],
            }
        });
        let (pos, apos) = self
            .host
            .movers
            .pose_at(id, self.host.level_time_ms)
            .unwrap_or((None, None));
        Some((
            pos.map_or(origin, |v| v.to_array()),
            apos.map_or(angles, |v| v.to_array()),
        ))
    }
}
