//! The per-slot mirrors the engine writes and reads, the session and
//! roster reads, client events and the carry.

use super::*;
use crate::game::commands::Recipient;

impl ScriptRuntime {
    /// `ClientEndFrame`'s `takedamage` write.
    pub fn set_client_takedamage(&mut self, slot: usize, on: bool) {
        if let Some(v) = self.host.clients.get_mut(slot).map(|c| &mut c.vitals) {
            v.takedamage = on;
        }
    }

    /// One client's health as the script left it, read every frame the way
    /// `HostClient::weapons` is.
    pub fn client_vitals(&self, slot: usize) -> crate::game::host::Vitals {
        self.host
            .clients
            .get(slot)
            .map(|c| &c.vitals)
            .copied()
            .unwrap_or_default()
    }

    /// The buttons of a client's last usercmd, for `useButtonPressed`.
    pub fn set_client_buttons(&mut self, slot: usize, buttons: u8) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.buttons = buttons;
        }
    }

    /// A client's last usercmd, for the `spawn` builtin's think.
    pub fn set_client_last_cmd(&mut self, slot: usize, cmd: vcod_common::net::msg::UserCmd) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.last_cmd = cmd;
        }
    }

    /// A client's wire `ps.pm_type` as the tick's moves left it, for the
    /// touch pass's gate.
    pub fn set_client_pm_type(&mut self, slot: usize, pm_type: i32) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.pm_type = pm_type;
        }
    }

    /// A client's `ps.on_ground` as the tick's moves left it, for
    /// `isOnGround` (docs/research/cod11-gsc-object-model.md, 23.5).
    pub fn set_client_on_ground(&mut self, slot: usize, on_ground: bool) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.on_ground = on_ground;
        }
    }

    /// A client's stance as the tick's moves left it, for `getStance`.
    pub fn set_client_stance(&mut self, slot: usize, stance: vcod_common::pmove::Stance) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.stance = stance;
        }
    }

    /// A client's prone cone as the tick's moves left it, for
    /// `setPlayerAngles`.
    pub fn set_client_prone_cone(
        &mut self,
        slot: usize,
        cone: Option<vcod_common::pmove::ProneCone>,
    ) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.prone_cone = cone;
        }
    }

    /// A client's eye (lean included) and `[pitch, yaw]` aim as the tick left
    /// them, for `aim_lookat`, and whether the held weapon is a `rifleBullet`
    /// one, which picks the priority map the trace ranks bones with.
    pub fn set_client_aim(&mut self, slot: usize, eye: [f32; 3], aim: [f32; 2], rifle: bool) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.aim = (eye, aim, rifle);
        }
    }

    /// A client's entity state as the tick's moves left it, for
    /// `cloneplayer`. `None` for a slot with no sim.
    pub fn set_client_entity_state(
        &mut self,
        slot: usize,
        state: Option<vcod_common::net::msg::EntityState>,
    ) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.entity_state = state;
        }
    }

    /// A client's body as the tick's moves left it, for a scripted blast.
    pub fn set_client_body(&mut self, slot: usize, body: Option<crate::game::combat::HitBody>) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.body = body;
        }
    }

    /// [`crate::spectate::ClientSim::land_gunner_goals`] on the host's
    /// copies of the slot's body, which script traces and tags read.
    pub fn land_gunner_goals(&mut self, slot: usize, legs: &crate::spectate::TurretLegs) {
        let Some(c) = self.host.clients.get_mut(slot) else {
            return;
        };
        for b in [&mut c.body, &mut c.dobj].into_iter().flatten() {
            b.pose.turret_leaves.clone_from(&legs.0);
            b.pose.turret_left.clone_from(&legs.1);
        }
    }

    /// A client's posed model, live or dead, for `dropItem`'s tag.
    pub fn set_client_dobj(&mut self, slot: usize, body: Option<crate::game::combat::HitBody>) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.dobj = body;
        }
    }

    /// The controls the slot's last skeleton build eased to
    /// (`GameHost::dobj_posed`).
    pub fn dobj_record(&self, slot: usize) -> Option<vcod_common::playerpose::Controllers> {
        self.host.dobj_posed.get(slot)?.record()
    }

    /// Starts a new skeleton stamp, as each client packet does.
    pub fn clear_dobj_stamps(&mut self) {
        self.host.clear_dobj_stamps();
    }

    /// The animtree a builtin poses a body with.
    pub fn set_player_anims(&mut self, anims: Option<Rc<vcod_common::animtree::PlayerAnims>>) {
        self.host.anims = anims;
    }

    /// A client's `ps.grenadeTimeLeft` as the tick's moves left it, for the
    /// death drop (`docs/research/cod11-combat.md` 5.1 step 5). 0 for a slot
    /// with no sim.
    pub fn set_client_grenade_ms(&mut self, slot: usize, ms: i32) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.grenade_ms = ms;
        }
    }

    /// A client's box height as the tick's moves left it, for `dropItem`.
    pub fn set_client_height(&mut self, slot: usize, height: f32) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.height = height;
        }
    }

    /// A client's origin at its last link, for the `radiusDamage` walk.
    pub fn set_client_link_origin(&mut self, slot: usize, origin: [f32; 3]) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.link_origin = origin;
        }
    }

    /// A client's ammo arrays as the tick's moves left them, for the item
    /// pass and `dropItem`. Ops still queued for that client's sim are
    /// re-applied on top, since the sim has not seen them yet.
    pub fn set_client_ammo(
        &mut self,
        slot: usize,
        ammo: [i16; vcod_common::pmove::weapon::NUM_AMMO],
        clip: [i16; vcod_common::pmove::weapon::NUM_AMMO],
    ) {
        let host = &mut self.host;
        let Some(a) = host.clients.get_mut(slot).map(|c| &mut c.ammo) else {
            return;
        };
        *a = crate::game::host::AmmoArrays { ammo, clip };
        for (_, op) in host.client_weapon_ops.iter().filter(|(s, _)| *s == slot) {
            a.apply(*op);
        }
    }

    /// `Cmd_MenuResponse_f` (0x486d8): notify the client's entity with the
    /// menu's **name** and the response. The name, not the index the client
    /// sent: retail reads configstring `CsRange::Menu.start + index` back
    /// and passes that string, which is why `dm.gsc` can both compare it
    /// against `game["menu_team"]` and hand it straight back to `openMenu`.
    ///
    /// The event name folds, the response does not: one is an event name,
    /// the other a string value the script compares against `"allies"` and
    /// weapon names.
    pub fn menu_response(&mut self, slot: usize, menu: &MenuRef, response: &str) {
        let Some(id) = self.client_entity(slot) else {
            return;
        };
        let menu = match menu {
            MenuRef::Index(i) => crate::configstrings::script_menu_name(
                &self.host.configstrings,
                self.host.allocators.layout(),
                *i,
            )
            .to_string(),
            MenuRef::Name(name) => name.clone(),
        };
        if !self.charge_client_strings(slot, &[&menu, response]) {
            return;
        }
        let (event, menu, response) = self.vm.with_cx(|cx| {
            (
                cx.intern_folded(MENURESPONSE_NOTIFY),
                cx.intern_exact(&menu),
                cx.intern_exact(response),
            )
        });
        self.vm
            .notify(id, event, &[Value::String(menu), Value::String(response)]);
    }

    /// Whether `slot` may add `texts` to the interner: what is not interned
    /// yet counts against [`CLIENT_STRING_BUDGET`]. A refusal is logged once
    /// a slot a level.
    fn charge_client_strings(&mut self, slot: usize, texts: &[&str]) -> bool {
        let interner = self.vm.interner();
        let new: usize = texts
            .iter()
            .filter(|t| interner.get_exact(t).is_none())
            .map(|t| t.len().max(1))
            .sum();
        if new == 0 {
            return true;
        }
        let (used, warned) = self.client_strings.entry(slot).or_default();
        if *used + new > CLIENT_STRING_BUDGET {
            if !*warned {
                *warned = true;
                log::warn!(
                    "client {slot}: past {CLIENT_STRING_BUDGET} bytes of new script strings this level; its menu answers and votes with new text are dropped"
                );
            }
            return false;
        }
        *used += new;
        true
    }

    /// An engine notify on client `slot`'s entity with string arguments,
    /// for the ones a game module raises outside a builtin (CoD2's
    /// binocular and vote notifies, `crate::game::cod2`). Its waiters run in
    /// the next thread pass.
    pub fn notify_client(&mut self, slot: usize, event: &str, args: &[&str]) {
        let Some(id) = self.client_entity(slot) else {
            return;
        };
        let (event, args) = self.vm.with_cx(|cx| {
            let args: Vec<Value> = args
                .iter()
                .map(|a| Value::String(cx.intern_exact(a)))
                .collect();
            (cx.intern_folded(event), args)
        });
        self.vm.notify(id, event, &args);
    }

    /// [`Self::notify_client`] with one value argument (CoD4's
    /// `confirm_location` vector).
    pub fn notify_client_value(&mut self, slot: usize, event: &str, arg: Option<Value>) {
        let Some(id) = self.client_entity(slot) else {
            return;
        };
        let event = self.vm.with_cx(|cx| cx.intern_folded(event));
        let args: Vec<Value> = arg.into_iter().collect();
        self.vm.notify(id, event, &args);
    }

    /// The per-client server commands the script queued this frame, in call
    /// order. `Server` sends them; nothing here can reach a netchan.
    pub fn take_client_commands(&mut self) -> Vec<(Recipient, String)> {
        self.host.client_commands.take()
    }

    /// [`Self::take_client_commands`] with each command's write stamp, the
    /// order CoD2's engine sends them in among the configstrings.
    pub fn take_client_commands_stamped(&mut self) -> Vec<(u64, Recipient, String)> {
        self.host.client_commands.take_stamped()
    }

    /// `GameHost::scoreboard` over the roster last mirrored in.
    pub fn scoreboard(&mut self) -> String {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.scoreboard(cx))
    }

    /// `G_Say` for a client's own `say`, `say_team` or `tell`; the lines it
    /// queued, in order.
    pub fn say(
        &mut self,
        slot: usize,
        target: Option<usize>,
        mode: crate::game::say::SayMode,
        text: &str,
    ) -> Vec<(Recipient, String)> {
        let from = self.host.client_commands.len();
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.say(cx, slot, target, mode, text));
        self.host.client_commands.split_off(from)
    }

    /// 1.5's `matchtimeout` (`timein` false) or `matchtimein` from `slot`:
    /// the server commands it sends, `None` addressing everyone
    /// ([`GameHost::match_timeout`]).
    pub fn match_timeout(
        &mut self,
        slot: usize,
        timein: bool,
        now_ms: i32,
    ) -> Vec<(Option<usize>, String)> {
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            if timein {
                host.match_timein(cx, slot, now_ms)
            } else {
                host.match_timeout(cx, slot, now_ms)
            }
        })
    }

    /// `Cmd_CallVote_f` (`callvote`) or `Cmd_Vote_f` (`vote`) for `slot`;
    /// `argv` is the whole command.
    pub fn vote_command(
        &mut self,
        slot: usize,
        argv: &[String],
        svs_ms: i32,
    ) -> crate::game::vote::Sends {
        if let Some(sends) = self.cod2_script_vote(slot, argv) {
            return sends;
        }
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            if argv[0].eq_ignore_ascii_case("vote") {
                host.cast_vote(cx, slot, argv.get(1).map_or("", String::as_str))
            } else {
                host.call_vote(cx, slot, argv, svs_ms)
            }
        })
    }

    /// CoD2's votes under `g_oldVoting` 0 (default 1): `Cmd_CallVote_f`
    /// (cod2_lnxded 0x80ffbb2) refuses under `g_allowVote` 0 or on a `;` in
    /// its three arguments (vcod also on a control byte) and otherwise
    /// notifies `call_vote` with them;
    /// `Cmd_Vote_f` (0x8100960) notifies `vote` with "yes" when the first
    /// letter is `y` or the second `Y` or `1` (Q3's test, as read), else
    /// "no". VERIFIED, both functions and the dvar defaults. `None` where
    /// the engine's own vote runs.
    fn cod2_script_vote(
        &mut self,
        slot: usize,
        argv: &[String],
    ) -> Option<crate::game::vote::Sends> {
        use crate::game::builtins::cvar::atoi;
        let flag = |v: &str| v.is_empty() || atoi(v) != 0;
        if !self.host.profile.votes.script_votes || flag(self.host.cvars.get("g_oldVoting")) {
            return None;
        }
        let arg = |i: usize| argv.get(i).map_or("", String::as_str);
        if argv[0].eq_ignore_ascii_case("vote") {
            let yes = crate::game::vote::is_yes(arg(1));
            self.notify_client(slot, "vote", &[if yes { "yes" } else { "no" }]);
            return Some(Vec::new());
        }
        if !flag(self.host.cvars.get("g_allowVote")) {
            return Some(vec![(Some(slot), "e \"GAME_VOTINGNOTENABLED\"".into())]);
        }
        // CoD4's two-client floor comes ahead of the script's call too
        // (0x80adc1d).
        if self
            .host
            .clients
            .iter()
            .filter_map(|c| c.roster.as_ref())
            .count()
            < self.host.profile.votes.min_connected
        {
            return Some(vec![(
                Some(slot),
                "e \"GAME_VOTINGNOTENOUGHPLAYERS\"".into(),
            )]);
        }
        let (a, b, c) = (arg(1), arg(2), arg(3));
        if [a, b, c].iter().any(|s| s.contains(';'))
            || crate::game::vote::holds_control_byte(&[a, b, c])
        {
            return Some(vec![(Some(slot), "e \"GAME_INVALIDVOTESTRING\"".into())]);
        }
        if self.charge_client_strings(slot, &[a, b, c]) {
            self.notify_client(slot, "call_vote", &[a, b, c]);
        }
        Some(Vec::new())
    }

    /// `CheckVote`: the lines for everyone and whether configstring 15
    /// changed.
    pub fn check_vote(&mut self, svs_ms: i32) -> (Vec<String>, bool) {
        let host = &mut self.host;
        self.vm.with_cx(|cx| host.check_vote(cx, svs_ms))
    }

    /// The spawns the script performed this frame, in call order. `Server`
    /// applies them to the client sims; nothing here can reach one.
    pub fn take_client_spawns(&mut self) -> Vec<SpawnRequest> {
        std::mem::take(&mut self.host.client_spawns)
    }

    /// One client's weapons as the script left them. Read every frame for the
    /// same reason the configstrings are: any thread can have changed them.
    pub fn client_weapons(&self, slot: usize) -> crate::weapons::PlayerWeapons {
        self.host
            .clients
            .get(slot)
            .map(|c| &c.weapons)
            .copied()
            .unwrap_or_default()
    }

    /// Writes back the weapon the sim's own machine switched to. The script
    /// host's copy is what `Server` mirrors into `ps.weapon` every frame, so
    /// a switch the usercmd's weapon byte asked for has to move it: without
    /// this the mirror puts the old weapon back the next frame and
    /// `PM_Weapon` starts the identical putaway again, once every
    /// `dropTime`, for as long as the client keeps asking.
    pub fn set_client_weapon(&mut self, slot: usize, weapon: u8) {
        if let Some(w) = self.host.clients.get_mut(slot).map(|c| &mut c.weapons) {
            w.current = weapon;
        }
    }

    /// Writes back the slots a select-fire switch moved (the putaway's alt
    /// arm, `docs/research/cod11-combat.md` 1.8), which the mirror would
    /// otherwise put back the next frame.
    pub fn set_client_slots(&mut self, slot: usize, slots: [u8; crate::weapons::NUM_SLOTS]) {
        if let Some(w) = self.host.clients.get_mut(slot).map(|c| &mut c.weapons) {
            w.slots = slots;
        }
    }

    /// `BG_TakePlayerWeapon`, which the last round of a `clipOnly` weapon
    /// with no reserve runs (`docs/research/cod11-combat.md` 1.5 step 9).
    /// `ps.weapon` is left alone: the switch path takes it to 0 on its own
    /// once the player no longer holds it (1.8).
    pub fn take_client_weapon(&mut self, slot: usize, weapon: u8) {
        if let Some(w) = self.host.clients.get_mut(slot).map(|c| &mut c.weapons) {
            w.take(weapon as usize);
        }
    }

    /// The ammo and current-weapon edges the weapon builtins made this frame,
    /// in call order. Drained rather than read, unlike `HostClient::weapons`: they
    /// are edges, and applying one twice would refill a spent clip.
    pub fn take_weapon_ops(&mut self) -> Vec<(usize, crate::game::host::WeaponOp)> {
        std::mem::take(&mut self.host.client_weapon_ops)
    }

    /// What UO's `G_DObjUpdate`s saw `slot` wear since the last call, in
    /// order (`GameHost::dobj_updates`).
    pub fn take_dobj_updates_of(&mut self, slot: usize) -> Vec<crate::game::hitrig::Assembly> {
        let (mine, rest) = std::mem::take(&mut self.host.dobj_updates)
            .into_iter()
            .partition(|(s, _)| *s == slot);
        self.host.dobj_updates = rest;
        mine.into_iter().map(|(_, a)| a).collect::<Vec<_>>()
    }

    /// Drops the queue's entries for clients without a sim.
    pub fn clear_dobj_updates(&mut self) {
        self.host.dobj_updates.clear();
    }

    /// Whether [`Self::take_ops_of`] would hand anything back for `slot`.
    pub fn has_ops_of(&self, slot: usize) -> bool {
        self.host.client_weapon_ops.iter().any(|(s, _)| *s == slot)
            || self.host.client_sim_ops.iter().any(|(s, _)| *s == slot)
    }

    /// `slot`'s share of [`Self::take_weapon_ops`] and
    /// [`Self::take_sim_ops`], for what the move pass lands before the
    /// client's next cmd; every other client's stay queued.
    pub fn take_ops_of(
        &mut self,
        slot: usize,
    ) -> (
        Vec<crate::game::host::WeaponOp>,
        Vec<crate::game::host::SimOp>,
    ) {
        fn split<T>(all: &mut Vec<(usize, T)>, slot: usize) -> Vec<T> {
            let (mine, rest) = std::mem::take(all)
                .into_iter()
                .partition::<Vec<_>, _>(|(s, _)| *s == slot);
            *all = rest;
            mine.into_iter().map(|(_, op)| op).collect()
        }
        (
            split(&mut self.host.client_weapon_ops, slot),
            split(&mut self.host.client_sim_ops, slot),
        )
    }

    /// One client's viewmodel index, read every frame for the same reason
    /// `HostClient::weapons` is.
    pub fn client_viewmodel(&self, slot: usize) -> i32 {
        self.host
            .clients
            .get(slot)
            .map(|c| c.viewmodel)
            .unwrap_or(0)
    }

    /// The slot's shellshock fields: index, start ms, duration ms.
    pub fn client_shellshock(&self, slot: usize) -> [i32; 3] {
        self.host
            .clients
            .get(slot)
            .map(|c| &c.shellshock)
            .copied()
            .unwrap_or([0; 3])
    }

    /// Reads a field off an entity through the same routing script uses,
    /// rendered as text by `render`.
    pub fn field_str(&mut self, ent: EntId, name: &str) -> String {
        use vcod_gsc::Host;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded(name);
            let v = host.get_field(cx, ent, atom);
            render(cx, v)
        })
    }

    /// One field off a client's entity, rendered the way `field_str` renders
    /// one. `None` when the slot holds no client entity.
    pub fn client_field(&mut self, slot: usize, name: &str) -> Option<String> {
        let ent = self.client_entity(slot)?;
        Some(self.field_str(ent, name))
    }

    /// A client's `(iHeadIcon, iHeadIconTeam)` (`GameHost::head_icon`).
    pub fn head_icon(&self, slot: usize) -> (i32, i32) {
        self.host.head_icon(slot)
    }

    /// One key out of a client's `.pers`, rendered the same way. Array keys
    /// are exact-cased, unlike the field name (`docs/research/
    /// cod11-gsc-language.md`), so the key is interned as written.
    pub fn client_pers(&mut self, slot: usize, key: &str) -> Option<String> {
        use vcod_gsc::Host;
        let ent = self.client_entity(slot)?;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let atom = cx.intern_folded("pers");
            let Value::Array(a) = host.get_field(cx, ent, atom) else {
                return None;
            };
            let k = vcod_gsc::ArrayKey::Str(cx.intern_exact(key));
            Some(render(cx, cx.get_index(a, k)))
        })
    }

    /// Queues one client lifecycle event for the next `run_frame`. The
    /// netcode's only way into script: a callback that ran inline from
    /// `SV_ClientCommand` would reenter the VM mid-frame.
    pub fn push_client_event(&mut self, ev: ClientEvent) {
        self.host.client_events.push(ev);
    }

    /// Writes a client's simulated origin onto its script entity. The sim
    /// owns a player's position and the script reads it: with `spawn` as the
    /// only writer, `positionWouldTelefrag` would test where each client last
    /// spawned rather than where it now stands.
    pub fn set_client_origin(&mut self, slot: usize, origin: [f32; 3]) {
        self.host.vehicles.lifted.remove(&slot);
        self.host.vehicles.stale_seated.remove(&slot);
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        self.set_vec_field(ent, "origin", origin);
    }

    /// Writes entity `id`'s vector field `name` through the script field
    /// write, no link.
    fn set_vec_field(&mut self, id: EntId, name: &str, v: [f32; 3]) {
        use vcod_gsc::Host;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded(name);
            let _ = host.set_field(cx, id, field, Value::Vector(v));
        });
    }

    /// Writes a client's `angles` field (`r.currentAngles`), no link.
    pub fn set_client_angles(&mut self, slot: usize, angles: [f32; 3]) {
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        self.set_vec_field(ent, "angles", angles);
    }

    /// Writes an entity's `origin` and `angles` as script would. Test-facing
    /// (`Server::test_place_entity`).
    pub fn place_entity(&mut self, num: u32, origin: [f32; 3], angles: [f32; 3]) {
        let Some(id) = self.host.ents.handle(num) else {
            return;
        };
        self.set_vec_field(id, "origin", origin);
        self.set_vec_field(id, "angles", angles);
    }

    /// Raises entity `num`'s `origin` by `dz` through the field write, and
    /// returns where it then stands.
    pub fn raise_entity(&mut self, num: u32, dz: f32) -> Option<[f32; 3]> {
        use vcod_gsc::Host;
        let id = self.host.ents.handle(num)?;
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let field = cx.intern_folded("origin");
            let Value::Vector(mut o) = host.get_field(cx, id, field) else {
                return None;
            };
            o[2] += dz;
            if dz != 0.0 {
                let _ = host.set_field(cx, id, field, Value::Vector(o));
            }
            Some(o)
        })
    }

    /// Writes a player's `angles` the way `ClientThink_real` does after every
    /// cmd it runs: pitch and roll 0, yaw the view's
    /// (docs/research/cod11-gsc-object-model.md 23.6).
    pub fn set_client_yaw(&mut self, slot: usize, yaw: f32) {
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        self.set_vec_field(ent, "angles", [0.0, yaw, 0.0]);
    }

    /// A client's body model as the roster's `modelindex`: the model number
    /// `configstring 268 + modelindex` resolves
    /// (`docs/research/clientstate-wire-format.md`). 0 when the client has no
    /// model yet, which is what a client still on the menus has.
    ///
    /// The model itself comes from the stock character scripts:
    /// `character/mp_american_airborne01.gsc` does
    /// `self setModel("xmodel/playerbody_american_airborne")`, called through
    /// `_teams::model()` on spawn.
    pub fn client_model_index(&mut self, slot: usize) -> i32 {
        let Some(name) = self.client_field(slot, "model") else {
            return 0;
        };
        // A client that has never had `setModel` reads model configstring
        // 0, "", which must not match the first unused model slot: retail
        // sends 0 for a spectator who never spawned.
        if name.is_empty() {
            return 0;
        }
        let (first, last) = self
            .host
            .allocators
            .layout()
            .bounds(crate::configstrings::CsRange::Model);
        self.host.configstrings[first..=last]
            .iter()
            .position(|cs| *cs == name)
            .map_or(0, |i| (i + 1) as i32)
    }

    /// A client's team as the roster carries it: `clientState.team`, the
    /// 2-bit field the client colours names and picks friend from foe with.
    /// Script owns it through `.sessionteam`, which `_teams.gsc` sets on a
    /// menu join and `dm.gsc` clears back to `"none"` on every spawn; the
    /// four names and the values they map to are in
    /// `docs/research/clientstate-wire-format.md`.
    ///
    /// A client whose `.sessionteam` is unset reads `TEAM_SPECTATOR`, which
    /// is what retail leaves in the field between `ClientConnect` and the
    /// first script write.
    pub fn client_team(&mut self, slot: usize) -> i32 {
        match self.client_field(slot, "sessionteam").as_deref() {
            Some("none") => TEAM_NONE,
            Some("axis") => TEAM_AXIS,
            Some("allies") => TEAM_ALLIES,
            _ => TEAM_SPECTATOR,
        }
    }

    /// What the follow pass reads off a client's script fields, `None` for a
    /// slot with no client entity. A `sessionstate` outside the four legal
    /// strings reads as playing, the arm `ClientEndFrame` falls through to.
    pub fn client_session(&mut self, slot: usize) -> Option<crate::follow::Session> {
        use crate::follow::{Session, SessionState};
        let state = match self.client_field(slot, "sessionstate")?.as_str() {
            "spectator" => SessionState::Spectator,
            "dead" => SessionState::Dead,
            "intermission" => SessionState::Intermission,
            _ => SessionState::Playing,
        };
        let spectator_client = self
            .client_field(slot, "spectatorclient")
            .and_then(|v| v.parse().ok())
            .unwrap_or(-1);
        let archive_ms = self.client_archive_ms(slot);
        let no_spectate = self
            .host
            .clients
            .get(slot)
            .map(|c| c.no_spectate)
            .unwrap_or(0);
        Some(Session {
            state,
            spectator_client,
            archive_ms,
            no_spectate,
        })
    }

    /// The character `slot`'s entity carries: its model and attachments,
    /// what a CoD2 or CoD4 hit rig is built from.
    pub fn client_character(&mut self, slot: usize) -> Option<vcod_cod2::player::Character> {
        let strip = |s: &str| s.strip_prefix("xmodel/").unwrap_or(s).to_string();
        let body = strip(&self.client_field(slot, "model")?);
        if body.is_empty() {
            return None;
        }
        let id = self.client_entity(slot)?;
        let (attachments, ignore_collision) = self.with_host(|h, cx| {
            h.ents.get(id).map_or_else(
                || (Vec::new(), 0),
                |e| {
                    let list = e
                        .attachments
                        .iter()
                        .map(|(m, t)| {
                            let tag = cx.resolve(*t).to_string();
                            (strip(cx.resolve(*m)), (!tag.is_empty()).then_some(tag))
                        })
                        .collect();
                    (list, u32::from(e.attach_ignore_collision))
                },
            )
        });
        Some(vcod_cod2::player::Character {
            body,
            attachments,
            ignore_collision,
        })
    }

    /// A client's `archivetime` in the milliseconds the engine reads, 0 for
    /// a slot with no client entity.
    pub fn client_archive_ms(&mut self, slot: usize) -> i32 {
        let i = crate::game::fields::archive_time_index();
        self.client_entity(slot)
            .and_then(|ent| self.host.ents.get(ent))
            .and_then(|e| e.client.as_ref())
            .map_or(0, |c| crate::game::host::archive_time_ms(c[i]))
    }

    /// The engine's own write to `archivetime`: the archive's trim and
    /// `SpectatorClientEndFrame`'s retry store milliseconds straight into the
    /// field (`crate::archive`).
    pub fn set_client_archive_ms(&mut self, slot: usize, ms: i32) {
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let i = crate::game::fields::archive_time_index();
        if let Some(c) = self.host.ents.get_mut(ent).and_then(|e| e.client.as_mut()) {
            c[i] = Value::Int(ms);
        }
    }

    /// `ClientEndFrame`'s own `ClientSpawn` (0x40f82): the memset takes the
    /// weapons and the ammo with the rest of the playerstate, and the
    /// entity is at the new spot. Unlike the `spawn` builtin it leaves the
    /// health, which the dead end of a killcam reads 0 on.
    pub fn engine_client_spawn(&mut self, slot: usize, origin: [f32; 3], yaw_deg: f32) {
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.weapons = crate::weapons::PlayerWeapons::default();
        }
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.ammo = crate::game::host::AmmoArrays::default();
        }
        if let Some(c) = self.host.clients.get_mut(slot) {
            c.shellshock = [0; 3];
        }
        self.set_client_origin(slot, origin);
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        self.set_vec_field(ent, "angles", [0.0, yaw_deg, 0.0]);
    }

    /// Whether script has turned the frame archive on (`setarchive`).
    pub fn archive_on(&self) -> bool {
        self.host.archive_on
    }

    /// The engine's own write to `spectatorclient`: `SpectatorClientEndFrame`
    /// and `StopFollowing` put -1 there without going through script.
    pub fn set_client_spectator_client(&mut self, slot: usize, v: i32) {
        let Some(ent) = self.client_entity(slot) else {
            return;
        };
        let i = crate::game::fields::spectator_client_index();
        if let Some(c) = self.host.ents.get_mut(ent).and_then(|e| e.client.as_mut()) {
            c[i] = Value::Int(v);
        }
    }

    /// A client's attachments as the roster carries them: up to six
    /// `(attachModelIndex, attachTagIndex)` pairs, the model resolving through
    /// `configstring 268 + index` and the tag through `108 + index`
    /// (`docs/research/clientstate-wire-format.md`). A head and a helmet are
    /// attachments, not part of the body: the stock character script does
    /// `attachFromArray(xmodelalias\head_allied::main())` and
    /// `self attach(self.hatModel)`, so a client sent none is headless.
    /// The tag takes its configstring here, not at `attach`: `ClientEndFrame`'s
    /// own-body arm calls `G_TagIndex` on each (1.1 0x413f6, UO
    /// `G_UpdateClientInfo` 0x52c81), so only a playing or dead client
    /// allocates one (docs/research/cod11-gsc-object-model.md, "attach and
    /// detach").
    pub fn client_attachments(&mut self, slot: usize) -> Vec<(i32, i32)> {
        let Some(ent) = self.client_entity(slot) else {
            return Vec::new();
        };
        let Some(e) = self.host.ents.get(ent) else {
            return Vec::new();
        };
        let pairs: Vec<(String, String)> = {
            self.vm.with_cx(|cx| {
                e.attachments
                    .iter()
                    .map(|(m, t)| (cx.resolve(*m).to_string(), cx.resolve(*t).to_string()))
                    .collect()
            })
        };
        // 1.7's `ClientEndFrame` (0x80a68b8) runs its inlined
        // `G_UpdateClientInfo` ahead of the session checks, as CoD2's
        // (0x80f7516) calls 0x80f72f8, so every connected client's tags
        // take their index (cod4-player-anims.md 5, cod2-player-anims.md 1).
        let own_body = matches!(
            self.host.profile.game,
            vcod_common::game::Game::Cod4 | vcod_common::game::Game::Cod2
        ) || self.client_session(slot).is_some_and(|s| {
            matches!(
                s.state,
                crate::follow::SessionState::Playing | crate::follow::SessionState::Dead
            )
        });
        let layout = self.host.allocators.layout();
        let mut out = Vec::with_capacity(pairs.len());
        for (m, t) in pairs.iter().take(ATTACH_SLOTS) {
            let tag = if t.is_empty() {
                None
            } else if own_body {
                self.host
                    .allocators
                    .index(
                        &mut self.host.configstrings,
                        crate::configstrings::CsRange::Tag,
                        t,
                    )
                    .ok()
            } else {
                self.host.allocators.find_folded(
                    &self.host.configstrings,
                    crate::configstrings::CsRange::Tag,
                    t,
                )
            };
            let model = if m.is_empty() {
                None
            } else {
                self.host.allocators.find_folded(
                    &self.host.configstrings,
                    crate::configstrings::CsRange::Model,
                    m,
                )
            };
            let rel =
                |slot: Option<usize>, range| slot.map_or(0, |s| (s - layout.base(range)) as i32);
            out.push((
                rel(model, crate::configstrings::CsRange::Model),
                rel(tag, crate::configstrings::CsRange::Tag),
            ));
        }
        out
    }

    /// The models a client wears, by name rather than by configstring index:
    /// what the locational trace poses. The body is `.model` and the head and
    /// helmet are attachments, the same pair `client_model_index` and
    /// `client_attachments` put on the roster.
    pub fn client_assembly(&mut self, slot: usize) -> Option<crate::game::hitrig::Assembly> {
        let ent = self.client_entity(slot)?;
        let host = &mut self.host;
        self.vm.with_cx(|cx| wear(host, cx, ent))
    }

    /// A client entity's `.origin` as the scripts read it.
    pub fn client_origin(&mut self, slot: usize) -> [f32; 3] {
        let Some(ent) = self.client_entity(slot) else {
            return [0.0; 3];
        };
        let host = &mut self.host;
        self.vm
            .with_cx(|cx| host.vec_field(cx, ent, "origin").to_array())
    }

    /// The client's entity, once `Connect` has been drained. The object
    /// table is the single owner of that fact: an entity at the slot's own
    /// number with a `client` store is one `spawn_client` made.
    pub fn client_entity(&self, slot: usize) -> Option<EntId> {
        let id = self.host.ents.handle(u32::try_from(slot).ok()?)?;
        self.host
            .ents
            .get(id)
            .filter(|e| e.client.is_some())
            .map(|_| id)
    }

    /// `SV_DropClient`'s `ClientDisconnect`, run inside the drop as retail
    /// runs it, not queued for the next frame's packet pass. The events the
    /// netcode queued ahead of it go first, in order, so a slot's `Connect`
    /// never lands after its own drop. The threads its notifies woke wait
    /// for the next frame's first pass.
    pub fn client_disconnect(&mut self, slot: usize) {
        for ev in std::mem::take(&mut self.host.client_events) {
            self.dispatch_client_event(ev);
        }
        self.dispatch_client_event(ClientEvent::Disconnect(slot));
    }

    /// `SV_SpawnServer`'s client pass (docs/research/cod11-map-cycle.md,
    /// section 3 step 22): the incoming level's `ClientConnect` for a client
    /// that is already on the netchan. The object table is the new level's,
    /// so this is a first connect as far as script is concerned. It runs
    /// rather than queues, because retail runs it inside the spawn, after the
    /// settle frames of step 20.
    pub fn reconnect_client(&mut self, slot: usize, name: String) {
        self.dispatch_client_event(ClientEvent::Connect { slot, name });
    }

    /// The animtrees the level's scripts name in `#using_animtree`.
    pub fn animtrees(&self) -> &[String] {
        self.scripts.as_ref().map_or(&[], |c| c.animtrees())
    }

    #[cfg(test)]
    pub(crate) fn compiled(&self) -> Option<&Rc<CompiledScripts>> {
        self.scripts.as_ref()
    }

    /// Drops the compiled closure a restart would reuse.
    pub(crate) fn forget_compiled(&mut self) {
        self.scripts = None;
    }

    /// `game[]` lifted for the next level, and every client's `pers[]` with
    /// it (docs/research/cod11-map-cycle.md section 1). Both leave this
    /// runtime intact; the caller drops it right after.
    pub fn take_carry(&self) -> Carry {
        let pers = (0..crate::server::MAX_CLIENTS)
            .map(|slot| {
                self.client_pers_array(slot)
                    .map(|id| self.vm.take_array(id))
            })
            .collect();
        Carry {
            game: Some(self.vm.take_game()),
            pers,
            items: Some(self.host.items.clone()),
            area: Some(self.host.area.clone()),
            map: self.map.clone(),
            scripts: self.scripts.clone(),
            gametype: self.scripts.as_ref().map(|s| s.gametype.clone()),
            seed: None,
            rand: None,
            spawn_pins: None,
            models: Some(crate::game::host::ModelCaches {
                collision: self.host.xmodel_collision.clone(),
                bounds: self.host.xmodel_bounds.clone(),
                rigs: self.host.vehicles.rigs.clone(),
                reads: self.host.xmodel_reads.clone(),
                untouched: self.host.xmodel_untouched.clone(),
                reload: None,
            }),
            weapon_files: match &self.host.ext {
                crate::game::host::HostExt::Cod2(c) => Some(c.weapon_files.clone()),
                _ => None,
            },
        }
    }

    /// A client's `.pers` array id, `None` for a slot with no client entity
    /// or one whose field is not an array.
    fn client_pers_array(&self, slot: usize) -> Option<vcod_gsc::ArrayId> {
        let id = self.client_entity(slot)?;
        let e = self.host.ents.get(id)?;
        match e.client.as_ref()?.get(crate::game::fields::pers_index())? {
            Value::Array(a) => Some(*a),
            _ => None,
        }
    }

    /// The console lines the level's builtins queued
    /// (`trap_SendConsoleCommand`), drained in call order. `Server` runs
    /// them at the top of its next tick, which is retail's `EXEC_APPEND`.
    pub fn take_console(&mut self) -> Vec<String> {
        std::mem::take(&mut self.host.console)
    }

    /// `level+0x20c`, taken and cleared: whether a score moved this frame,
    /// which is what arms the intermission scoreboard drain (map-cycle doc,
    /// 6.3). Retail's drain clears the flag whether or not any client was in
    /// intermission to receive one, so this takes rather than reads.
    pub fn take_ranks_dirty(&mut self) -> bool {
        std::mem::take(&mut self.host.ranks_dirty)
    }

    /// One queued client event. A callback the closure does not define is
    /// logged and skipped: a gametype without one is still a serving map,
    /// the same reading `load`'s missing-builtin pre-scan takes.
    pub(super) fn dispatch_client_event(&mut self, ev: ClientEvent) {
        match ev {
            ClientEvent::Connect { slot, name } => {
                let host = &mut self.host;
                let Some(id) = self.vm.with_cx(|cx| host.connect_client(cx, slot, &name)) else {
                    return;
                };
                self.start_callback("CodeCallback_PlayerConnect", id);
                // `ClientConnect` closes with `CalculateRanks` (1.1d 0x42677).
                self.host.ranks_dirty = true;
            }
            ClientEvent::Begin(slot) => {
                let Some(id) = self.client_entity(slot) else {
                    log::warn!("client {slot}: begin with no entity, connect never ran");
                    return;
                };
                // `ClientBegin` (1.1d 0x42f88): `CalculateRanks`, then the
                // notify, whose waiters run in the next frame's thread pass
                // on its clock (cod11-server-handshake.md, "When the game
                // hears a drop").
                self.host.ranks_dirty = true;
                let event = self.vm.with_cx(|cx| cx.intern_folded(BEGIN_NOTIFY));
                self.vm.notify(id, event, &[]);
            }
            ClientEvent::Disconnect(slot) => {
                // `ClientDisconnect` opens with `menuresponse` ("-1",
                // "disconnect") on the player (1.1d 0x42ad9, 1.5b 0x4cf5f,
                // UO 0x55433). Its waiters run on the next frame's clock;
                // the player's own die with it below
                // (cod11-server-handshake.md, "When the game hears a drop").
                if let Some(id) = self.client_entity(slot) {
                    let (event, menu, response) = self.vm.with_cx(|cx| {
                        (
                            cx.intern_folded(MENURESPONSE_NOTIFY),
                            cx.intern_exact("-1"),
                            cx.intern_exact("disconnect"),
                        )
                    });
                    self.vm
                        .notify(id, event, &[Value::String(menu), Value::String(response)]);
                }
                self.host.complaints_about_gone(slot);
                // `ClientDisconnect` lets go of a vehicle seat first (0x45433).
                self.with_host(|host, cx| {
                    crate::game::vehicle::seat::unlink_player(
                        host,
                        cx,
                        slot,
                        false,
                        [0.0; 3],
                        ([0.0; 3], [0.0; 3]),
                    )
                });
                self.host.vehicles.drop_seat_ops(slot);
                self.host.vehicles.riders.remove(&slot);
                self.host.vehicles.overheated_gunners.retain(|&g| g != slot);
                // Whatever the last level boundary lifted for this slot dies
                // with the client that owned it: the next occupant of the
                // slot is a stranger and must not connect into its `pers[]`.
                if let Some(c) = self.host.pers_carry.get_mut(slot) {
                    *c = None;
                }
                // The client's own HUD elements go before the callback, as
                // in retail's `ClientDisconnect` (cod11-hud-protocol.md, "A
                // client's elements die with it"): the thread that would
                // `destroy` them is killed below, and the next client into
                // the slot has the owner's number.
                self.host.ents.free_client_hud_elems(slot);
                // The callback runs first: it reads `self`, and freeing the
                // slot ahead of it would hand it a dead entity.
                if let Some(id) = self.client_entity(slot) {
                    self.start_callback("CodeCallback_PlayerDisconnect", id);
                    // Whatever was still running on the player dies with
                    // it: a dead player's `waitRespawnButton` polled a
                    // freed entity otherwise (`Vm::kill_threads_of`).
                    self.vm.kill_threads_of(Target::Entity(id));
                }
                // `G_FreeEntity` on the player (turrets doc 8): a gun it
                // manned is unowned, its loop left to the unmanned think.
                for rec in self.host.turrets.values_mut() {
                    if rec.owner == Some(slot) {
                        rec.owner = None;
                        rec.busy = 0;
                    }
                }
                // `G_FreeEntity`'s unlink, owner and ground sweeps
                // (`ClientDisconnect` 0x42bfc).
                self.host.area.unlink(slot as u32);
                self.host.missiles.free_owner_refs(slot);
                for b in &mut self.host.barrages {
                    if b.owner == Some(slot) {
                        b.owner = None;
                    }
                }
                self.host.free_ground_refs(slot as u32);
                self.host.ents.free_client(slot);
                // `CalculateRanks` closes it on all three (1.1d 0x42c1f).
                self.host.ranks_dirty = true;
            }
        }
    }

    /// One `_callbacksetup` entry point on a client's entity.
    fn start_callback(&mut self, name: &str, id: EntId) {
        if let Err(e) = self.start(CALLBACK_SETUP, name, Some(Target::Entity(id))) {
            log::error!("gsc: {e:#}");
        }
    }
}
