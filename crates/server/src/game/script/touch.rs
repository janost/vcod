//! The touch and use passes: triggers, items, look-at triggers and the
//! cursor hints.

use super::*;

impl ScriptRuntime {
    /// `G_TouchTriggers` for one client, which retail runs once per usercmd
    /// from `ClientThink_real` (0x405b3): the triggers and the items it
    /// touches, in the area tree's order (`trigger::touched`). The notifies
    /// only mark the threads parked on them runnable; they run in this
    /// tick's script frame.
    ///
    /// Only a client at `ps.pm_type <= 1` touches anything: the function's
    /// own second guard, which takes the dead, a spectator and the
    /// intermission camera out before the pass runs
    /// (docs/research/cod11-gsc-object-model.md 8.2).
    ///
    /// The touch is taken at the client's `origin` field;
    /// `touch_triggers_at` takes it at an explicit `ps.origin` instead.
    pub fn touch_triggers(&mut self, slot: usize, now_ms: i32) {
        let origin = self.client_origin(slot);
        self.touch_triggers_at(slot, now_ms, origin);
    }

    /// `ClientImpacts` for `slot`'s cmd: [`GameHost::client_impacts`].
    pub fn client_impacts(&mut self, slot: usize, touched: &[u32]) {
        self.host.client_impacts(slot, touched);
    }

    /// `G_TouchTriggers` with its boxes on `origin`, `ps.origin`, while the
    /// client's `origin` field holds the snapped `r.currentOrigin` a hurt's
    /// callbacks read (`docs/research/cod11-combat.md` 5.5).
    pub fn touch_triggers_at(&mut self, slot: usize, now_ms: i32, origin: [f32; 3]) {
        let Some(client) = self.client_entity(slot) else {
            return;
        };
        if self.host.clients.get(slot).map(|c| c.pm_type).unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return;
        }
        let host = &mut self.host;
        let hits = self
            .vm
            .with_cx(|cx| crate::game::trigger::touched(host, cx, slot, origin));
        log::trace!(target: "touch_pass", "slot {slot} at {origin:?}: {hits:?}");
        // One walk, in the tree's order: each entity's two `"touch"`
        // notifies (0x3fa42, 0x3fa61), then its touch function, an item's
        // `Touch_Item` or a trigger's fire.
        for id in hits {
            let Some(e) = self.host.ents.get(id) else {
                continue;
            };
            let item = e.item.is_some();
            self.host
                .engine_notifies
                .push((id, "touch", vec![Value::Entity(client)]));
            self.host
                .engine_notifies
                .push((client, "touch", vec![Value::Entity(id)]));
            if item {
                let host = &mut self.host;
                self.vm
                    .with_cx(|cx| crate::game::item::touch(host, cx, id, slot, true));
                continue;
            }
            if let Some(crate::game::trigger::TriggerKind::Door { parent }) =
                self.host.triggers.get(id).map(|t| t.kind)
            {
                let host = &mut self.host;
                self.vm.with_cx(|cx| {
                    crate::game::mover::door::touch_trigger(host, cx, parent, client)
                });
                continue;
            }
            // `Touch_Multi` skips its wait arm on a trigger whose think is
            // `enableLinkTo`'s `Think_GeneralLink` (0x65aa2).
            let linked = self.host.links.is_enabled(id);
            // `hurt_touch` reads the trigger's `dmg` and `spawnflags` at
            // every touch (cod11-combat.md 5.9).
            let is_hurt = self
                .host
                .triggers
                .get(id)
                .is_some_and(|t| t.kind == crate::game::trigger::TriggerKind::Hurt);
            let hurt = is_hurt.then(|| {
                let (dmg, flags) = (self.int_field(id, "dmg"), self.int_field(id, "spawnflags"));
                (dmg, self.host.triggers.arm_hurt(id, flags), flags)
            });
            let fire = self.host.triggers.fire(id, now_ms, linked);
            if !fire.fired() {
                continue;
            }
            // `Touch_Multi` arms the free off `level.time`, which inside a
            // cmd is still the last frame's.
            if fire == crate::game::trigger::Fire::Spent {
                let at = self.host.level_time_ms + crate::game::trigger::SPENT_FREE_MS;
                self.host
                    .ents
                    .schedule(id, crate::game::entity::ThinkFn::Free, at);
            }
            if !self.host.queue_trigger(id, client) {
                self.host
                    .engine_notifies
                    .push((id, "trigger", vec![Value::Entity(client)]));
            }
            if let Some((damage, dflags, flags)) = hurt {
                // `G_PlaySoundAliasAtPoint` at the toucher's origin, a temp
                // entity taken ahead of the damage callback's.
                let sound = self.host.triggers.get(id).map_or(0, |t| t.sound);
                if flags & crate::game::trigger::HURT_SILENT == 0 && sound != 0 {
                    let at = self.client_origin(slot);
                    self.add_sound_at(at, sound);
                }
                self.deliver_world_hit(
                    slot,
                    id,
                    damage,
                    dflags,
                    crate::game::trigger::MOD_TRIGGER_HURT,
                );
            }
        }
    }

    /// Where the `radiusDamage` calls since the last ask went off, and the
    /// client each named as its attacker.
    pub fn take_blast_noises(&mut self) -> Vec<([f32; 3], Option<usize>)> {
        std::mem::take(&mut self.host.blast_noises)
    }

    /// Engine code that needs the host and the VM's context together,
    /// outside a builtin: a vehicle's seat change or step. A thread it
    /// spawns is dropped (`Cx::spawn`), so it raises notifies instead.
    pub fn with_host<R>(&mut self, f: impl FnOnce(&mut GameHost, &mut vcod_gsc::Cx) -> R) -> R {
        let host = &mut self.host;
        self.vm.with_cx(|cx| f(host, cx))
    }

    /// The buttons of the last cmd `item_pass` saw from `slot`, which its
    /// use edge is taken against.
    pub fn client_old_buttons(&self, slot: usize) -> u8 {
        self.host
            .clients
            .get(slot)
            .map(|c| c.old_buttons)
            .unwrap_or(0)
    }

    /// `Cmd_Activate_f` for one cmd (docs/research/cod11-items.md, section
    /// 2): on the use key's rising edge, the item, turret or `trigger_use`
    /// the aim picks. Gated as the touch pass is, and on the player being
    /// alive, `G_CheckForCursorHints`' own gate. The items the player walks
    /// onto are touched in `touch_triggers_at`, in the area tree's order
    /// among the triggers.
    pub fn item_pass(&mut self, slot: usize, buttons: u8, eye: [f32; 3], view: [f32; 3]) {
        let Some(client) = self.client_entity(slot) else {
            return;
        };
        let old = std::mem::replace(&mut self.host.clients[slot].old_buttons, buttons);
        let pressed = buttons & !old & vcod_common::net::msg::BUTTON_USE != 0;
        // `G_IsVehicleUnusable` first: a rider's press is the vehicle's use,
        // the exit (game.mp.uo 0x4dffb).
        if pressed
            && let Some((veh, _)) = self.host.vehicles.seat_of(slot)
            && self.host.vehicles.live[&veh].contents & crate::game::vehicle::CONTENTS_USABLE != 0
        {
            let host = &mut self.host;
            self.vm.with_cx(|cx| {
                // `ps.origin`, which the cmd's touch pass left in the
                // entity's `origin`.
                let from = host.vec_field(cx, client, "origin").to_array();
                crate::game::vehicle::seat::unlink_player(
                    host,
                    cx,
                    slot,
                    true,
                    from,
                    crate::game::vehicle::seat::PLAYER_BOX,
                )
            });
            return;
        }
        // `Cmd_Activate_f`'s busy byte (0x4848e), which has no `pm_type`
        // gate: a gunner's press asks for the release and does nothing else
        // (turrets doc 4.1).
        let mut release_asked = false;
        if pressed
            && let Some(rec) = self
                .host
                .turrets
                .values_mut()
                .find(|r| r.owner == Some(slot))
        {
            rec.busy = 2;
            release_asked = true;
        }
        if self.host.clients.get(slot).map(|c| c.pm_type).unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return;
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            if !pressed || release_asked {
                return;
            }
            let v = host.clients[slot].vitals;
            if v.health <= 0 || v.dead {
                return;
            }
            match crate::game::item::activate_ent(host, cx, slot, eye, view) {
                Some(Activate::Item(id)) => {
                    host.engine_notifies
                        .push((id, "touch", vec![Value::Entity(client)]));
                    crate::game::item::touch(host, cx, id, slot, false);
                }
                Some(Activate::Turret(turret)) => {
                    host.turret_ops.push(TurretOp::Mount { slot, turret });
                }
                // `Cmd_Activate_f` notifies `trigger` on the entity the hint
                // pass chose on the spot (0x48547), not through the trigger
                // queue, and with no `wait` gate
                // (docs/research/cod11-gametypes-re-bel.md 4).
                Some(Activate::Trigger(id)) => {
                    host.engine_notifies
                        .push((id, "trigger", vec![Value::Entity(client)]))
                }
                // `Cmd_Activate_f`'s classname arm (0x484f6), past the hint's
                // door arm (`G_CheckForCursorHints` 0x4f7d0), which gives a
                // door no hint unless it is shut or a toggle sits open.
                Some(Activate::Door(id)) => {
                    use crate::game::mover::door::{Class, POS1, POS1_ROTATE, POS2, POS2_ROTATE};
                    let usable = match crate::game::mover::door::binary_state(host, id) {
                        Some((Class::Slide, s, toggle, _)) => s == POS1 || (s == POS2 && toggle),
                        Some((Class::Rotate, s, toggle, _)) => {
                            s == POS1_ROTATE || (s == POS2_ROTATE && toggle)
                        }
                        _ => false,
                    };
                    if usable {
                        crate::game::mover::door::try_door(host, cx, id, client);
                    }
                }
                Some(Activate::Vehicle(veh)) => {
                    let on_ground = host.clients.get(slot).map(|c| c.on_ground).unwrap_or(false);
                    crate::game::vehicle::seat::board(host, cx, veh, slot, on_ground, 0.0);
                }
                None => {}
            }
        });
    }

    /// `ClientEndFrame`'s aim trace (`G_CheckForPreventFriendlyFire`,
    /// 0x4f88c): once per frame per playing client, after the script frame,
    /// the lookat the aim enters first is stored for `isLookingAt` and fired
    /// through the same notify the touch pass raises, whose waiters run on
    /// the next frame (docs/research/cod11-gsc-object-model.md 23.1). Every
    /// frame it is aimed at, since `G_Trigger` gates nothing.
    ///
    /// The `pm_type` gate skips a spectator and the intermission camera, as
    /// `ClientEndFrame`'s own arms do. It also clears a dead client (`pm_type`
    /// 6/7), where retail's sessionstate 1 reaches the trace subject to the
    /// unread `ent+0x172` byte (23.1); every stock `isLookingAt` caller also
    /// tests `isAlive`.
    pub fn aim_lookat(&mut self, slot: usize, now_ms: i32) {
        let Some(client) = self.client_entity(slot) else {
            if let Some(c) = self.host.clients.get_mut(slot) {
                c.lookat = None;
            }
            return;
        };
        if self.host.clients.get(slot).map(|c| c.pm_type).unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            self.host.clients[slot].lookat = None;
            return;
        }
        let (eye, aim, rifle) = self.host.clients[slot].aim;
        let host = &mut self.host;
        let hit = self
            .vm
            .with_cx(|cx| crate::game::trigger::aim_trace(host, cx, slot, eye, aim, rifle, now_ms));
        self.host.clients[slot].lookat = hit;
        if let Some(id) = hit
            && self.host.triggers.fire(id, now_ms, false).fired()
            && !self.host.queue_trigger(id, client)
        {
            self.host
                .engine_notifies
                .push((id, "trigger", vec![Value::Entity(client)]));
        }
    }

    /// `G_CheckForCursorHints` (0x4f59c) as `ClientEndFrame` runs it every
    /// frame: the hint for what the use key would pick now, 0 for none or for
    /// a player not alive (docs/research/cod11-items.md, section 2.3), and
    /// `serverCursorHintString`: `None` leaves it as it was, which a dead
    /// player and a gunner do (turrets doc 4.3), `Some(-1)` is no string.
    pub fn cursor_hint_pass(
        &mut self,
        slot: usize,
        eye: [f32; 3],
        view: [f32; 3],
    ) -> (i32, Option<i32>) {
        let h = self.cursor_hints(slot, eye, view, None);
        (h.hint, h.string)
    }

    /// [`crate::game::trigger::mount_boxes`]: what the server's point
    /// contents add to pmove's and the hint's.
    pub fn mount_boxes(&mut self) -> Vec<vcod_common::movetrace::ContentsBox> {
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| crate::game::trigger::mount_boxes(host, cx))
    }

    /// [`Self::cursor_hint_pass`] with United Offensive's LMG arm
    /// (game.mp.uo 0x67aa0): a sighted LMG, the bipod down, skips the pass
    /// and reads 0; one held otherwise reads hint 7 (`HINT_LMG`) with its
    /// [`crate::game::trigger::lmg_mount`] value and the weapon's
    /// `useHintString` where a mount is ahead, unless an activate entity's
    /// hint replaces the 7, which keeps the value
    /// (docs/research/coduo-movement.md 11).
    pub fn cursor_hints(
        &mut self,
        slot: usize,
        eye: [f32; 3],
        view: [f32; 3],
        lmg: Option<LmgHint<'_>>,
    ) -> CursorHint {
        let hint = |hint, string| CursorHint {
            hint,
            val: 0,
            string,
        };
        let v = self
            .host
            .clients
            .get(slot)
            .map(|c| &c.vitals)
            .copied()
            .unwrap_or_default();
        if self.client_entity(slot).is_none() || v.health <= 0 || v.dead {
            return hint(0, None);
        }
        if self.host.turrets.values().any(|r| r.owner == Some(slot)) {
            return hint(0, None);
        }
        // `G_CheckForCursorHints` skips everything for a rider (0x575bb).
        if self.host.vehicles.seat_of(slot).is_some() {
            return hint(0, Some(-1));
        }
        if self.host.clients.get(slot).map(|c| c.pm_type).unwrap_or(0) > TOUCH_MAX_PM_TYPE {
            return hint(0, Some(-1));
        }
        let mut miss = hint(0, Some(-1));
        if let Some(l) = lmg {
            if l.sighted {
                return miss;
            }
            let (sin, cos) = l.yaw_deg.to_radians().sin_cos();
            let point = glam::Vec3::from(l.origin)
                + glam::Vec3::new(cos, sin, 0.0) * LMG_PROBE_AHEAD
                + glam::Vec3::Z;
            if let Some(val) = crate::game::trigger::lmg_mount(&self.host, l.boxes, point) {
                let string = l
                    .hint_string
                    .and_then(|name| {
                        crate::configstrings::hint_string_index(
                            &self.host.configstrings,
                            self.host.allocators.layout(),
                            name,
                        )
                    })
                    .unwrap_or(-1);
                miss = CursorHint {
                    hint: HINT_LMG,
                    val,
                    string: Some(string),
                };
            }
        }
        let val = miss.val;
        let found = |h: i32, string: i32| CursorHint {
            hint: h,
            val,
            string: Some(string),
        };
        let host = &mut self.host;
        let hit = self
            .vm
            .with_cx(|cx| crate::game::item::activate_ent(host, cx, slot, eye, view));
        let id = match hit {
            None => return miss,
            Some(Activate::Turret(id)) => {
                let string = self.host.turrets[&id]
                    .def
                    .use_hint_string
                    .as_deref()
                    .and_then(|name| {
                        crate::configstrings::hint_string_index(
                            &self.host.configstrings,
                            self.host.allocators.layout(),
                            name,
                        )
                    });
                return found(HINT_MG42, string.unwrap_or(-1));
            }
            Some(Activate::Trigger(id)) => {
                let Some(t) = self.host.triggers.get(id) else {
                    return miss;
                };
                return found(t.cursor_hint, t.hint_string);
            }
            Some(Activate::Item(id)) => id,
            // The door arm (0x4f7d0): 4, or 5 locked, on a shut door or an
            // open toggle; any other state is no hint.
            Some(Activate::Door(id)) => {
                use crate::game::mover::door::{Class, POS1, POS1_ROTATE, POS2, POS2_ROTATE};
                return match crate::game::mover::door::binary_state(&self.host, id) {
                    Some((Class::Slide, s, toggle, locked))
                        if s == POS1 || (s == POS2 && toggle) =>
                    {
                        found(if locked { 5 } else { 4 }, -1)
                    }
                    Some((Class::Rotate, s, toggle, locked))
                        if s == POS1_ROTATE || (s == POS2_ROTATE && toggle) =>
                    {
                        found(if locked { 5 } else { 4 }, -1)
                    }
                    _ => miss,
                };
            }
            // `hintActivate` with the vehicle file's `useHintString`.
            Some(Activate::Vehicle(id)) => {
                let string = self
                    .host
                    .vehicles
                    .live
                    .get(&id)
                    .map_or(-1, |v| v.hint_string);
                return found(HINT_ACTIVATE, string);
            }
        };
        let Some(item) = self.host.ents.get(id).and_then(|e| e.item) else {
            return miss;
        };
        let Some(kind) = crate::game::pickup::item_kind(self.host.defs, item.index as usize) else {
            return miss;
        };
        let owned = self.host.clients[slot].weapons.holds(item.index as usize);
        CursorHint {
            hint: crate::game::pickup::cursor_hint(self.host.defs.proto, kind, owned),
            ..miss
        }
    }
}
