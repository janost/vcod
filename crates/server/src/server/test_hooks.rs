//! The test-facing seams on [`Server`]: the `test_*` and `pin_*` methods the
//! gates under `tests/`, the examples and the client's tests call. Most sit
//! behind the `testing` feature; the `pin_*` draw queues do not.

use super::*;

impl Server {
    /// Test-facing: each client slot's playing spawns, in order, go to these
    /// `(origin, angles)` instead of the script's random pick (`None` keeps
    /// the pick), across every later level. A gate replays a retail
    /// capture's picks with it: retail seeds the script's draws from the
    /// wall clock (map-cycle doc 4.8).
    pub fn pin_spawns(&mut self, pins: Vec<Vec<crate::game::host::SpawnPin>>) {
        let queues = pins.into_iter().map(Into::into).collect();
        let pins = Rc::new(std::cell::RefCell::new(queues));
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.spawn_pins = Some(pins.clone());
        }
        self.game.module.spawn_pins = Some(pins);
    }

    /// Test-facing: `StuckInClient` draws its jitter from glibc's `rand()`
    /// seeded `seed`, past `skip` draws. A gate replays a retail capture's
    /// pushes with the seed recovered off it: retail seeds `rand()` from
    /// the clock and draws only the jitter after the joins (player-clip
    /// doc 13.4).
    pub fn pin_stuck_rand(&mut self, seed: u32, skip: usize) {
        let mut g = vcod_common::rng::GlibcRand::new(seed);
        for _ in 0..skip {
            g.next_u31();
        }
        self.game.stuck_rand = Some(g);
    }

    /// Mounts `slot` on the turret numbered `gun` as a use press would, from
    /// `mount_origin` and past the reach and arc tests. Test-facing, like
    /// `place_client`: it puts a second gunner on a map with one reachable
    /// gun. False when either is missing or the gun is manned.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_mount(&mut self, slot: usize, gun: u32, mount_origin: [f32; 3]) -> bool {
        let Some(rt) = self.game.module.script.as_mut() else {
            return false;
        };
        let Some(sim) = self
            .core
            .clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        else {
            return false;
        };
        let Some(turret) = rt.host.turrets.keys().copied().find(|id| id.0 == gun) else {
            return false;
        };
        if rt.host.turrets[&turret].busy != 0 {
            return false;
        }
        rt.host
            .turret_ops
            .push(crate::game::turret::TurretOp::Mount { slot, turret });
        let mounts = rt.take_turret_mounts(
            slot,
            mount_origin,
            vcod_common::pmove::Stance::Stand,
            sim.view_angles(),
        );
        let rule = crate::game::turret::AimRule::of(rt.host.defs.proto);
        for (turret, stance, view) in &mounts {
            crate::game::turret::mount_sim(sim, rule, *turret, *stance, *view);
        }
        !mounts.is_empty()
    }

    /// Writes a client's `sessionteam` and nothing else. Test-facing: a 1.5
    /// match timeout needs a side, which a `dm` probe gametype gives no one.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_set_client_team(&mut self, slot: usize, team: &str) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.set_client_team_for_test(slot, team);
        }
    }

    /// Writes a client's `sessionstate` as a script would. Test-facing.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_set_client_session(&mut self, slot: usize, state: &str) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.set_client_string_for_test(slot, "sessionstate", state);
        }
    }

    /// Moves a client's playerstate origin and nothing else: no teleport
    /// bit, no event, no view. Test-facing: a replay starts from a retail
    /// capture's settled origin where the join left ours elsewhere.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_set_client_origin(&mut self, slot: usize, origin: [f32; 3]) {
        if let Some(sim) = self
            .core
            .clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        {
            sim.ps.origin = origin.into();
        }
    }

    /// Sets a client's playerstate velocity and nothing else. Test-facing: a
    /// placement that shares the spawn's frame keeps the velocity the cmds
    /// run at the spawn point left (cod11-combat.md 2.1, "The street
    /// placement"), and a replay hands ours retail's from the capture.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_set_client_velocity(&mut self, slot: usize, velocity: [f32; 3]) {
        if let Some(sim) = self
            .core
            .clients
            .get_mut(slot)
            .and_then(Option::as_mut)
            .and_then(|c| c.sim.as_mut())
        {
            sim.ps.velocity = velocity.into();
        }
    }

    /// Queues a client `setorigin` as the script frame would, applied at the
    /// next tick's sim-op pass: after that tick's moves and before its end
    /// frame. Test-facing: the stuck gate overlaps two players the way
    /// `probe_bump.gsc` did on retail.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_script_set_origin(&mut self, slot: usize, origin: [f32; 3]) {
        assert!(
            self.game.module.script.is_some(),
            "a script runtime to queue the setorigin on"
        );
        self.game
            .module
            .test_sim_ops
            .push((slot, crate::game::host::SimOp::SetOrigin { origin }));
    }

    /// Moves the entity numbered `num` as script would. Test-facing, like
    /// `test_mount`: carentan's second gun sits out of the first one's arc.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_place_entity(&mut self, num: u32, origin: [f32; 3], angles: [f32; 3]) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.place_entity(num, origin, angles);
        }
    }

    /// Test-facing: every item landing asks `f(level_time, floor)` for its
    /// lift instead of drawing retail's random half unit (items doc 14.2), so
    /// a gate can see each landing and rest it where a retail capture's draw
    /// put it.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_item_lifts(&mut self, f: crate::game::host::ItemLift) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.test_item_lift = Some(f);
        }
    }

    /// Test-facing: the `rand()` values the next item draws take in place of
    /// the server's own (`game::item`'s `unit_draw`), so a gate can launch a
    /// drop with the climb and spin a retail capture shows.
    pub fn pin_item_draws(&mut self, draws: Vec<i32>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.item_draw_pins = draws.into();
        }
    }

    /// Test-facing: the missile pool's next draws, a retail capture's
    /// tumble rates and bounce factors (`Missiles::pin_draws`).
    pub fn pin_missile_draws(&mut self, draws: Vec<f64>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.missiles.pin_draws(draws);
        }
    }

    /// Test-facing: the points the next artillery incomings land on
    /// (`GameHost::barrage_pins`).
    pub fn pin_barrage_points(&mut self, points: Vec<[f32; 3]>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.barrage_pins = points.into_iter().map(glam::Vec3::from).collect();
        }
    }

    /// Test-facing: the running level's `holdrand` from `seed`, as
    /// `G_InitGame`'s `Rand_Init(trap_Milliseconds())` left it (map-cycle
    /// doc 4.8): a gate replays a retail capture's script draws with the
    /// seed recovered off them.
    pub fn pin_script_rand(&mut self, seed: u32) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.holdrand = vcod_common::rng::HoldRand(seed);
        }
    }

    /// Test-facing: the `gunrandom` pairs the next rockets' spread takes,
    /// read off retail under gdb (combat doc 13.6).
    pub fn pin_spread_draws(&mut self, draws: Vec<(f32, f32)>) {
        self.game.module.spread_draws = draws.into();
    }

    /// Test-facing: the next rockets' `trDelta`, a retail capture's
    /// (`Missiles::pin_rocket_deltas`).
    pub fn pin_rocket_deltas(&mut self, deltas: Vec<[f32; 3]>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host
                .missiles
                .pin_rocket_deltas(deltas.into_iter().map(glam::Vec3::from));
        }
    }

    /// Test-facing: the outcomes of the next UO turret rounds' tracer rolls,
    /// one per player hit, as a retail capture drew them.
    pub fn pin_tracer_rolls(&mut self, rolls: Vec<bool>) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.tracer_pins = rolls.into();
        }
    }

    /// Test-facing: raise entity `num` by `dz` (an `origin` write) and return
    /// its origin; `dz` 0 only reads it.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_raise_entity(&mut self, num: u32, dz: f32) -> Option<[f32; 3]> {
        self.game.module.script.as_mut()?.raise_entity(num, dz)
    }

    /// Test-facing: `self pingPlayer()` on `slot`, with no script to call
    /// it.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_ping_player(&mut self, slot: usize) {
        if let Some(rt) = self.game.module.script.as_mut() {
            let until = rt.host.level_time_ms + 3000;
            if let Some(p) = rt.host.client_ping_until.get_mut(slot) {
                *p = until;
            }
        }
    }

    /// Test-facing: `self giveWeapon(name)` on `slot` without its ammo, for
    /// a weapon no stock loadout hands out. False for an unknown name.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_give_weapon(&mut self, slot: usize, name: &str) -> bool {
        let Some(rt) = self.game.module.script.as_mut() else {
            return false;
        };
        let Some(index) = rt.host.defs.weapon_index(name) else {
            return false;
        };
        let weapons = rt.host.weapons.clone();
        rt.host.client_weapons[slot].give_chain(&weapons, index, weapons.slot(index));
        true
    }

    /// Test-facing: `self giveMaxAmmo(name)` on `slot`, a full clip and
    /// reserve, as the frame's weapon pass applies it. False for an unknown
    /// name.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_give_max_ammo(&mut self, slot: usize, name: &str) -> bool {
        use crate::game::host::WeaponOp;
        let Some(rt) = self.game.module.script.as_mut() else {
            return false;
        };
        let Some(index) = rt.host.defs.weapon_index(name) else {
            return false;
        };
        let weapons = rt.host.weapons.clone();
        let Some(def) = weapons.get(index) else {
            return false;
        };
        if !def.clip_only {
            rt.host.weapon_op(
                slot,
                WeaponOp::SetAmmo {
                    ammo_index: def.ammo_index,
                    rounds: def.max_ammo as i16,
                },
            );
        }
        rt.host.weapon_op(
            slot,
            WeaponOp::SetClip {
                clip_index: def.clip_index,
                rounds: def.clip_size as i16,
            },
        );
        true
    }

    /// Test-facing: the seed the spread and the melee rolls draw from, so a
    /// gate that needs a shot to land where it did once can have it again.
    /// The bots added after it draw their own streams off `seed` too.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_seed_rng(&mut self, seed: u64) {
        self.core.rng = seed | 1;
        self.game.module.bot_admin.seed_rng = bot_admin::seed_stream(seed);
    }

    /// Whether a shot fired from eye height at `origin` along `yaw_deg`
    /// reaches `dist` without hitting anything. Test-facing, like
    /// `place_client`: a gate that puts one client in front of another has to
    /// say the sightline between them is real, or a spawn point that moved
    /// into a wall reads as "the damage path is broken". A server with no
    /// collision world answers yes.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_clear_line(&self, origin: [f32; 3], yaw_deg: f32, dist: f32) -> bool {
        let Some(world) = self.game.world.as_ref() else {
            return true;
        };
        let stand = vcod_common::pmove::Stance::Stand.view_height();
        let eye = glam::Vec3::from(origin) + glam::Vec3::Z * stand;
        let (s, c) = yaw_deg.to_radians().sin_cos();
        let end = eye + glam::Vec3::new(c, s, 0.0) * dist;
        world.collision.shot_trace(eye, end).fraction >= 1.0
    }

    /// Test-facing, beside `test_clear_line`: `CanDamage`'s fraction for a
    /// standing player whose feet are at `feet`, from a blast at `at`
    /// (combat doc, 14.3), against the world alone: the script models the
    /// live blast also meets are left out. 0 is a player the blast cannot
    /// see at all.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_can_damage(&self, at: [f32; 3], feet: [f32; 3]) -> f32 {
        let Some(world) = self.game.world.as_ref() else {
            return 1.0;
        };
        let v = standing_blast_victim(feet);
        crate::game::combat::can_damage(glam::Vec3::from(at), &v, &world.collision, &[], &[], None)
    }

    /// Test-facing, beside `test_can_damage`: the same standing victim at
    /// `feet`, against the world, the script models and every live player's
    /// posed body but `slot`'s own, which is the victim's pass entity
    /// (combat doc, 14.4).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_can_damage_among_players(
        &mut self,
        at: [f32; 3],
        feet: [f32; 3],
        slot: usize,
    ) -> f32 {
        let Some(world) = self.game.world.as_ref() else {
            return 1.0;
        };
        let bodies: Vec<crate::game::combat::HitBody> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref()?.sim.as_ref()?.hit_body(i))
            .collect();
        let models = self
            .game
            .module
            .script
            .as_mut()
            .map_or_else(Vec::new, |rt| rt.placed_script_models());
        let mut bones = crate::game::combat::BoneTraceCtx::of(
            self.game.fs.as_deref(),
            self.game.module.anims.as_deref(),
            &mut self.game.module.hit_rigs,
            self.core.sv_time_ms,
            dobj_stamps(self.game.module.script.as_mut()),
            vcod_common::patch::Arith::for_protocol(self.game.proto),
        );
        let mut v = standing_blast_victim(feet);
        v.slot = slot;
        crate::game::combat::can_damage(
            glam::Vec3::from(at),
            &v,
            &world.collision,
            &models,
            &bodies,
            bones.as_mut(),
        )
    }

    /// Test-facing: the length of the bots' planned route from the graph
    /// node nearest `from` to the one nearest `to`, or `None` when either
    /// point is off the graph, no route exists, or the graph is not built
    /// yet. What a bot scenario needs to put a walker where it can reach its
    /// goal. On the graph means within a pitch of a node on the same floor:
    /// `nearest` alone answers from a bank for a point down in a ditch.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_nav_route(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
        let g = self.game.module.driver.nav.as_ref()?;
        let on = |p: [f32; 3]| {
            let n = g.nearest(p)?;
            let d = g.nodes[n as usize] - glam::Vec3::from(p);
            (d.truncate().length() <= g.spacing && d.z.abs() < 18.0).then_some(n)
        };
        let path = g.path(on(from)?, on(to)?)?;
        Some(
            path.windows(2)
                .map(|w| g.nodes[w[0] as usize].distance(g.nodes[w[1] as usize]))
                .sum(),
        )
    }

    /// Test-facing: what a standing box dropped from `p` lands on, and
    /// where, as `CollisionWorld::describe` names it.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_floor_under(&self, p: [f32; 3]) -> Option<(f32, String)> {
        let world = self.game.world.as_ref()?;
        use vcod_common::pmove::{HALF_WIDTH, Stance};
        let mins = glam::Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
        let maxs = glam::Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height());
        let start = glam::Vec3::from(p);
        let t = world
            .collision
            .box_trace(start, start - glam::Vec3::Z * 256.0, mins, maxs);
        Some((
            t.endpos.z,
            t.hit
                .map(|h| world.collision.describe(h))
                .unwrap_or_default(),
        ))
    }

    /// Test-facing: entity `ent`'s box in the area tree (`r.absmin`,
    /// `r.absmax`), `None` while it is not linked.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_area_link(&self, ent: u32) -> Option<([f32; 3], [f32; 3])> {
        let link = self.game.module.script.as_ref()?.host.area.last_link(ent)?;
        Some((link.absmin, link.absmax))
    }

    /// Test-facing: whether submodel `m`'s brushes clip.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_model_linked(&self, m: usize) -> bool {
        self.game
            .world
            .as_ref()
            .is_some_and(|w| w.collision.model_linked(m))
    }

    /// Test-facing: where a standing player dropped at `p` comes to rest,
    /// or `None` when it starts inside geometry or finds no floor within
    /// 256 units. What a test needs to put a second client somewhere the map
    /// actually holds one.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_ground_under(&self, p: [f32; 3]) -> Option<[f32; 3]> {
        let world = self.game.world.as_ref()?;
        use vcod_common::pmove::{HALF_WIDTH, Stance};
        let mins = glam::Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
        let maxs = glam::Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height());
        let start = glam::Vec3::from(p);
        let t = world
            .collision
            .box_trace(start, start - glam::Vec3::Z * 256.0, mins, maxs);
        (!t.startsolid && !t.allsolid && t.fraction < 1.0).then_some(t.endpos.into())
    }

    /// Clones a client into the body queue and returns the corpse's entity
    /// number. Test-facing, like `place_client`: `cloneplayer` is the script
    /// path that will call this, and it does not exist yet.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_push_body(&mut self, slot: usize) -> Option<u32> {
        let c = self.core.clients.get(slot)?.as_ref()?;
        let state = c
            .sim
            .as_ref()?
            .to_entity(self.game.proto, slot, c.last_processed_st);
        let (now, proto) = (self.core.sv_time_ms, self.game.proto);
        let collision = self.game.world.as_ref().map(|w| &w.collision);
        self.game
            .module
            .script
            .as_mut()
            .map(|rt| rt.bodies_mut().push(state, None, now, collision, proto))
    }

    /// Raises one event for the next snapshot build. Test-facing, like
    /// `test_push_body`: the script builtins are what raise these in earnest.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_push_temp_entity(&mut self, te: temp_entity::TempEntity) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.push_temp_entity(te);
        }
    }

    /// Test-facing: turns a bot's retreat on or off (`Bot::set_retreat`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_retreat(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.set_retreat(on);
        }
    }

    /// Test-facing: whether a bot varies its peeks and hunts from a
    /// vantage (`Bot::wary`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_wary(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.wary = on;
        }
    }

    /// Test-facing: whether a bot fights from cover (`bots::cover`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_cover(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.cover.enabled = on;
        }
    }

    /// Test-facing: whether a bot moves while it fights (`Bot::footwork`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_footwork(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.footwork = on;
        }
    }

    /// Test-facing: whether a bot aims as a hand does (`Bot::human_aim`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_human_aim(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.human_aim = on;
        }
    }

    /// Test-facing: whether a bot reacts to hits, keeps its target and
    /// sees in a cone (`Bot::aware`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_aware(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.aware = on;
        }
    }

    /// Test-facing: whether a bot plays as one of its team (`bots::team`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_team_play(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.team_play = on;
        }
    }

    /// Test-facing: whether a bot keeps apart from its team (`Bot::squad`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_squad(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.squad = on;
        }
    }

    /// Test-facing: whether an S&D defender converges on a site under
    /// attack (`Bot::converge`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_converge(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.converge = on;
        }
    }

    /// Test-facing: whether a bot walks to its objective by a way in
    /// dealt apart from its teammates' (`Bot::lanes`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_lanes(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.lanes = on;
        }
    }

    /// Test-facing: whether a bot plays its objective by posts
    /// (`Bot::tactics`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_tactics(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.tactics = on;
        }
    }

    /// Test-facing: pins a bot's goal to a point (`Bot::force_goal`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_goal(&mut self, slot: usize, p: Option<[f32; 3]>) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.force_goal(p);
        }
    }

    /// Test-facing: whether a bot guards from the ways in (`Bot::far`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_far(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.far = on;
        }
    }

    /// Test-facing: whether a bot's guards answer cues (`Bot::cues`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_cues(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.cues = on;
        }
    }

    /// Test-facing: whether a bot keeps farther apart on the way
    /// (`Bot::travel`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_travel(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.travel = on;
        }
    }

    /// Test-facing: whether a `ctf` carrier keeps for home and waits in
    /// cover (`Bot::ctf_carry`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_ctf_carry(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.ctf_carry = on;
        }
    }

    /// Test-facing: a `ctf` carrier's escorts, safer route and hold from
    /// an enemy ahead (`Bot::ctf_escort`, `ctf_route`, `ctf_hold`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_ctf_escort(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.ctf_escort = on;
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_ctf_route(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.ctf_route = on;
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_ctf_hold(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.ctf_hold = on;
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_ctf_sprint(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.ctf_sprint = on;
        }
    }

    /// Test-facing: a `ctf` flag's guards once it is taken, on the
    /// carrier's way out or closing on him, and nearer outposts
    /// (`Bot::ctf_watch`, `ctf_close`, `ctf_near`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_ctf_watch(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.ctf_watch = on;
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_ctf_close(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.ctf_close = on;
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_ctf_near(&mut self, slot: usize, on: bool) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            bot.ctf_near = on;
        }
    }

    /// Test-facing: a client command a bot sends with its next replies.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bot_command(&mut self, slot: usize, cmd: &str) {
        self.game
            .module
            .bot_admin
            .orders
            .push((slot, cmd.to_string()));
    }

    /// Test seam: the paks a download is served from, with no level load.
    /// Every pak in `fs` is listed in the systeminfo as if the level had
    /// read it, so a client checks a transfer against its checksum.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_mount_paks(&mut self, fs: Rc<vcod_common::pk3::Pk3Fs>) {
        self.game.paks = configstrings::PakLists::of(
            false,
            fs.paks_read().map(|(p, _)| (p, true)),
            self.game.proto,
            "",
        );
        self.game.fs = Some(fs);
        self.set_protocol(self.game.proto);
    }
}
