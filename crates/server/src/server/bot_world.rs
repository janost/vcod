//! CoD1's bots on the driver (`crate::bots::driver`): what the brains see
//! of a 1.1, 1.5 or UO level, read off the clients' sims, the weapon table,
//! the script's objectives and the world.

use super::*;

impl Server {
    /// The bots' pass of the tick ([`crate::bots::driver::step`]).
    pub(super) fn step_bots(&mut self) {
        crate::bots::driver::step(self);
    }

    /// Whether the graph stands over the brushes the script links now, with
    /// no rebuild asked for or under way: a room behind a gametype's walls
    /// reads as a pit only until the rebuild over their removal lands.
    pub(super) fn nav_settled(&self) -> bool {
        let links = self
            .game
            .module
            .script
            .as_ref()
            .map_or(0, |rt| rt.host.brush_links);
        self.game.module.nav_rebuild.is_none()
            && self.game.module.nav_relink.is_none()
            && links == self.game.module.nav_links
    }

    /// Rebuilds the navigation graph over the brushes the script has linked
    /// and unlinked since it was built, once they have held still for
    /// [`NAV_RELINK_MS`]: United Offensive's `bas.gsc` deletes a base's
    /// walls when its health runs out (`BaseAssault_base_think`, the
    /// `script_idnumber` brushes), which opens the way to the charge. The
    /// bots keep the old graph until the new one lands.
    pub(super) fn relink_nav(&mut self, links: u32) {
        let Some(w) = self
            .game
            .world
            .as_ref()
            .filter(|_| self.game.module.driver.nav.is_some())
        else {
            return;
        };
        if self.game.module.nav_rebuild.is_none() && links != self.game.module.nav_links {
            match self.game.module.nav_relink {
                Some((l, at)) if l == links => {
                    if self.core.sv_time_ms.wrapping_sub(at) >= NAV_RELINK_MS {
                        let rules = self.move_rules();
                        self.game.module.nav_rebuild =
                            Some(crate::nav::NavJob::start(&self.game.cfg.map, w, rules));
                        self.game.module.nav_links = links;
                        self.game.module.nav_relink = None;
                    }
                }
                _ => self.game.module.nav_relink = Some((links, self.core.sv_time_ms)),
            }
        }
        if let Some(job) = self.game.module.nav_rebuild.as_mut()
            && let Some(g) = job.poll(!self.game.module.driver.nav_background)
        {
            log::info!("bots: navigation graph rebuilt over the script's brushes");
            self.game.module.driver.nav = Some(g);
            self.game.module.nav_rebuild = None;
            self.game.module.driver.paths.clear();
            self.game.module.driver.sites.clear();
            self.game.module.bot_breach_stands.clear();
        }
    }

    /// [`crate::bots::BotView::lob`] for the body in `slot` throwing grenade
    /// `weapon` at `target`: of the throws from 10 degrees down to 85 up,
    /// every 5, the one whose frag, flown by the missile code to its burst
    /// (`Missiles::predict_flight`), goes off nearest `target`, within
    /// [`crate::bots::LOB_MISS`]. The aim as (pitch, yaw), wire degrees.
    /// A frag leaves at `projectileSpeed` whatever the pitch, so the pitch
    /// alone sets where it comes down: lobbed 5 to 40 degrees over the
    /// line, frags flew on to a median 1.8 times the range and went off a
    /// median 490 units from where they went (bot-combat.md 12). A throw
    /// whose early arc meets a body is not taken: a UO frag bounces off one
    /// at full speed. With friendly fire on, None while a live teammate,
    /// where he stands or where his pace takes him, is within the frag's
    /// `explosionRadius` of `target` or of where it may go off.
    pub(super) fn lob_probe(
        &self,
        slot: usize,
        weapon: u8,
        target: [f32; 3],
        teams: &[i32],
    ) -> Option<[f32; 2]> {
        use crate::bots::{LOB_MISS, LOB_REACH_SLACK, LOB_SLACK};
        use vcod_common::net::trajectory::DEFAULT_GRAVITY;
        let sim = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        let def = self.game.module.weapon_table.get(weapon as usize)?;
        let rt = self.game.module.script.as_ref()?;
        let my_team = teams.get(slot).copied().unwrap_or(0);
        let radius = def.explosion_radius;
        let target = glam::Vec3::from(target);
        // Teammates matter only with friendly fire on (1.1's `tdm` ships
        // with it off; 2 and 3 hurt the thrower or both).
        let teamplay = (my_team == script::TEAM_AXIS || my_team == script::TEAM_ALLIES)
            && crate::game::builtins::cvar::atoi(&self.live_cvar("scr_friendlyfire", "0")) != 0;
        let mates: Vec<(glam::Vec3, glam::Vec3)> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter(|(i, _)| teamplay && *i != slot && teams.get(*i).copied() == Some(my_team))
            .filter_map(|(_, c)| c.as_ref()?.sim.as_ref())
            .filter(|o| o.pm_type == crate::spectate::PmType::Normal && !o.dead)
            .map(|o| (o.ps.origin, o.ps.velocity))
            .collect();
        // A teammate where he stands or where his pace takes him while the
        // frag flies: one running after the same enemy came into the blast.
        let mate_near = |at: glam::Vec3| {
            mates.iter().any(|(m, v)| {
                (0..=crate::bots::MATE_LEAD_S).any(|t| (*m + *v * t as f32).distance(at) < radius)
            })
        };
        if mate_near(target) {
            return None;
        }
        let world = &self.game.world.as_ref()?.collision;
        let eye = crate::game::combat::muzzle_point(&sim.ps);
        let to = target - eye;
        let range = to.truncate().length();
        let yaw = to.y.atan2(to.x).to_degrees();
        let bodies: Vec<(glam::Vec3, glam::Vec3)> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != slot)
            .filter_map(|(_, c)| c.as_ref()?.sim.as_ref())
            .filter(|o| o.pm_type == crate::spectate::PmType::Normal && !o.dead)
            .map(|o| {
                let grow = glam::Vec3::splat(crate::bots::LOB_BODY_GROW);
                (
                    o.ps.origin + o.ps.mins() - grow,
                    o.ps.origin + o.ps.maxs() + grow,
                )
            })
            .collect();
        // When it goes off: a 1.1 or 1.5 fuse runs in full from the
        // release; a UO cook burns `COOK_MEAN_MS` of it in the hand on the
        // mean and up to `COOK_MOST_MS`, so a burst near the thrower is
        // looked for from that much short of the full fuse on. On
        // mp_cassino a UO stick grenade lobbed at an enemy 550 off met a
        // wall, came back and went off 340 from the wounded thrower, whose
        // run from it fell short of the 350 radius (seed 19, bot-combat.md
        // 6.5).
        let fuse = vcod_common::weapon::stored_ms(def.fuse_time);
        let (burst_ms, earliest) = if def.cook_off_hold {
            (
                fuse - crate::bots::COOK_MEAN_MS,
                fuse - crate::bots::COOK_MOST_MS,
            )
        } else {
            (fuse, fuse)
        };
        let (lo, hi) = (sim.ps.origin + sim.ps.mins(), sim.ps.origin + sim.ps.maxs());
        let half_g = 0.5 * DEFAULT_GRAVITY;
        // Where a throw along `aim` goes off, and whether it stays clear of
        // the thrower and the teammates from `earliest` on, wherever a cook
        // that long or short leaves the burst; None when the flight is cut
        // short (out of the world, or onto the sky).
        let fly = |aim: [f32; 2]| {
            let (origin, velocity) = crate::game::missile::throw_velocity(&sim.ps, aim, def);
            let flight = rt.missiles().predict_flight(
                world,
                weapon,
                origin,
                velocity,
                sim.ps.velocity,
                fuse,
            );
            let burst = *flight.get((burst_ms / FRAME_MS).max(1) as usize - 1)?;
            let clear = flight
                .iter()
                .enumerate()
                .filter(|(i, _)| (*i as i32 + 1) * FRAME_MS >= earliest)
                .all(|(_, p)| p.clamp(lo, hi).distance(*p) > radius && !mate_near(*p));
            Some((burst, clear))
        };
        let mut best: Option<([f32; 2], f32)> = None;
        for up in (-2..=17).map(|i| i as f32 * 5.0) {
            let aim = [-up, yaw];
            let (origin, velocity) = crate::game::missile::throw_velocity(&sim.ps, aim, def);
            // Where the arc comes down to the target's height with nothing
            // in its way: only a throw that lands near the range is flown.
            let disc = velocity.z * velocity.z - 4.0 * half_g * (target.z - origin.z);
            if disc < 0.0 {
                continue;
            }
            let down = (velocity.z + disc.sqrt()) / (2.0 * half_g);
            if (velocity.truncate().length() * down - range).abs() > LOB_REACH_SLACK {
                continue;
            }
            // The arc's first 1.5 s meets a body before the world.
            let at = |t: f32| origin + velocity * t - glam::Vec3::Z * (half_g * t * t);
            let mut from = origin;
            let mut body = false;
            for i in 1..=30 {
                let next = at(i as f32 * 0.05);
                let tr = world.missile_trace(from, next);
                body = bodies
                    .iter()
                    .filter_map(|(lo, hi)| crate::game::combat::ray_box(from, next, *lo, *hi))
                    .any(|f| f < tr.fraction);
                if body || tr.fraction < 1.0 {
                    break;
                }
                from = next;
            }
            if body {
                continue;
            }
            let Some((burst, true)) = fly(aim) else {
                continue;
            };
            let miss = burst.distance(target);
            if miss >= LOB_MISS {
                continue;
            }
            // The release may be `LOB_SLACK` off the probed pitch: a frag
            // let go half a degree under a lob the probe flew clear struck
            // something 110 units out and went off 220 behind its thrower
            // (bot-combat.md 12).
            let slack = [-LOB_SLACK, LOB_SLACK];
            if !slack
                .iter()
                .all(|d| fly([aim[0] + d, yaw]).is_some_and(|(_, c)| c))
            {
                continue;
            }
            if best.is_none_or(|(_, m)| miss < m) {
                best = Some((aim, miss));
            }
        }
        best.map(|(aim, _)| aim)
    }

    /// The rocket launchers lying about, on a `bas` level only: what a bot
    /// breaches a base with (bot-objectives.md 7.2); and the bullet
    /// weapons, what a bot with nothing left to fire takes up. Each with a
    /// round in its clip or its reserve.
    pub(super) fn bot_launchers(
        &mut self,
        uo: Option<&script::UoObjectives>,
    ) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
        if !matches!(uo, Some(script::UoObjectives::Bas(_))) {
            return (Vec::new(), Vec::new());
        }
        let weapons = self.game.module.weapon_table.clone();
        let Some(rt) = self.game.module.script.as_mut() else {
            return (Vec::new(), Vec::new());
        };
        let (mut launchers, mut guns) = (Vec::new(), Vec::new());
        for (w, at, empty) in rt.weapon_items() {
            match weapons.get(w as usize) {
                _ if empty => {}
                Some(d) if d.weapon_class == "rocketlauncher" => launchers.push(at),
                Some(d) if d.weapon_type == "bullet" => guns.push(at),
                _ => {}
            }
        }
        (launchers, guns)
    }

    /// Where a rocket from `slot`'s muzzle is aimed to burst within
    /// [`crate::bots::BREACH_NEAR`] of a base at `at`, between
    /// [`crate::bots::BREACH_MIN`] and [`crate::bots::BREACH_RANGE`] off:
    /// the base's origin, or a point under it on the floor it sits on. A
    /// script model links as a point, so a rocket aimed at the origin flies
    /// through it and bursts on what lies behind.
    pub(super) fn breach_shot(
        &self,
        slot: usize,
        at: [f32; 3],
        models: &[crate::game::combat::PlacedModel],
    ) -> Option<[f32; 3]> {
        let sim = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        let world = &self.game.world.as_ref()?.collision;
        let muzzle = crate::game::combat::muzzle_point(&sim.ps);
        let spot = breach_aim(world, models, muzzle, at.into())?;
        // A body on the way bursts the rocket there, on the shooter as
        // often as not: no shot while any other player's box crosses the
        // line.
        let bodies: Vec<(glam::Vec3, glam::Vec3)> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != slot)
            .filter_map(|(_, c)| c.as_ref()?.sim.as_ref())
            .filter(|o| o.pm_type == crate::spectate::PmType::Normal && !o.dead)
            .map(|o| (o.ps.origin + o.ps.mins(), o.ps.origin + o.ps.maxs()))
            .collect();
        (!crate::bots::friend_in_line(muzzle, spot.into(), &bodies)).then_some(spot)
    }

    /// The script models and vehicles within [`BREACH_MODEL_NEAR`] of a
    /// `bas` base, on such a level only: what a rocket fired at one meets
    /// besides the world, the base's own model among them.
    pub(super) fn bot_breach_models(
        &mut self,
        uo: Option<&script::UoObjectives>,
    ) -> Vec<crate::game::combat::PlacedModel> {
        let Some(script::UoObjectives::Bas(bases)) = uo else {
            return Vec::new();
        };
        let Some(rt) = self.game.module.script.as_mut() else {
            return Vec::new();
        };
        rt.shot_models()
            .into_iter()
            .filter(|m| {
                bases
                    .iter()
                    .any(|b| m.origin.distance(glam::Vec3::from(b.at)) < BREACH_MODEL_NEAR)
            })
            .collect()
    }

    /// Up to one graph node in each of [`BREACH_SECTORS`] sectors round a
    /// base, [`crate::bots::BREACH_MIN`] to [`BREACH_STAND_FAR`] off, from
    /// whose standing eye [`breach_aim`] finds a shot: where a bot with a
    /// rocket walks to. Empty until the graph is up.
    pub(super) fn breach_stands(
        &mut self,
        at: [f32; 3],
        models: &[crate::game::combat::PlacedModel],
    ) -> Vec<[f32; 3]> {
        use crate::bots::BREACH_MIN;
        if let Some((_, s)) = self
            .game
            .module
            .bot_breach_stands
            .iter()
            .find(|(b, _)| *b == at)
        {
            return s.clone();
        }
        let (Some(g), Some(w)) = (
            self.game.module.driver.nav.clone(),
            self.game.world.as_ref(),
        ) else {
            return Vec::new();
        };
        let base = glam::Vec3::from(at);
        // On the graph's main component, as `site_stand`'s.
        let comp = g.component_ids();
        let mut sizes: BTreeMap<u32, usize> = BTreeMap::new();
        for c in comp {
            *sizes.entry(*c).or_default() += 1;
        }
        let main = sizes
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(c, _)| *c);
        // Each sector's node of least score, the first in graph order on a
        // tie, among those with a shot: the candidates go by sector, score
        // and order, and each sector takes the first whose `breach_aim`
        // finds one, so the traces stop there. Tracing every node in range
        // held a `bas` tick 240-290 ms after each rebuild; the sectors now
        // search on threads of their own (`first_passes`, map-cycle doc
        // 4.9).
        let mut candidates: Vec<(usize, f32, usize)> = Vec::new();
        for (i, n) in g.nodes.iter().enumerate() {
            let d = n.truncate().distance(base.truncate());
            if !(BREACH_MIN..=BREACH_STAND_FAR).contains(&d) || Some(comp[i]) != main {
                continue;
            }
            let a = (n.y - base.y)
                .atan2(n.x - base.x)
                .rem_euclid(std::f32::consts::TAU);
            let k = ((a / std::f32::consts::TAU * BREACH_SECTORS as f32) as usize)
                .min(BREACH_SECTORS - 1);
            candidates.push((k, (d - BREACH_STAND_IDEAL).abs(), i));
        }
        candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        let sectors: Vec<&[(usize, f32, usize)]> = candidates.chunk_by(|a, b| a.0 == b.0).collect();
        let collision = &w.collision;
        let eye = |i: usize| g.nodes[i] + glam::Vec3::Z * vcod_common::pmove::VIEW_STAND;
        let found = first_passes(&sectors, |&(_, _, i)| {
            breach_aim(collision, models, eye(i), base).is_some()
        });
        let mut best: [Option<glam::Vec3>; BREACH_SECTORS] = [None; BREACH_SECTORS];
        for (run, f) in sectors.iter().zip(found) {
            best[run[0].0] = f.map(|f| g.nodes[run[f].2]);
        }
        let stands: Vec<[f32; 3]> = best.iter().flatten().map(|n| (*n).into()).collect();
        self.game
            .module
            .bot_breach_stands
            .push((at, stands.clone()));
        stands
    }

    /// What one bot's body sees this tick, from its sim and the weapon
    /// table. `None` between levels, where the null cmd is all a bot can
    /// send.
    pub(super) fn bot_view(
        &self,
        slot: usize,
        enemy: Option<crate::bots::EnemyView>,
    ) -> Option<crate::bots::BotView> {
        let sim = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        let weapons = self.game.module.weapon_table.clone();
        let def = weapons.get(sim.ps.weapon as usize);
        let held = |pick: &dyn Fn(&vcod_common::weapon::WeaponDef) -> bool| {
            (1u8..64).find(|i| {
                sim.ps.weapons_held >> i & 1 == 1 && weapons.get(*i as usize).is_some_and(pick)
            })
        };
        // A frag: UO's smoke grenades and satchel charge are grenades too,
        // in slots of their own.
        let grenade = held(&|d| d.weapon_type == "grenade" && d.weapon_slot == "grenade");
        let pistol = held(&|d| d.weapon_slot == "pistol");
        let ms = |s: f32| (s * 1000.0) as i32;
        // A segmented reload's first round lands after its start segment
        // and one loop segment.
        let reload_ms = def.map_or(0, |d| {
            if d.segmented_reload {
                ms(d.reload_start_time + d.reload_time)
            } else if d.reload_empty_time > 0.0 {
                ms(d.reload_empty_time)
            } else {
                ms(d.reload_time)
            }
        });
        let draw_ms = pistol
            .and_then(|p| weapons.get(p as usize))
            .map_or(0, |p| ms(def.map_or(0.0, |d| d.drop_time) + p.raise_time));
        let loaded = (1u8..64)
            .filter(|&i| sim.ps.weapons_held >> i & 1 == 1)
            .filter(|&i| {
                weapons.get(i as usize).is_some_and(|d| {
                    let reserve = if d.clip_only {
                        0
                    } else {
                        sim.ps.ammo[d.ammo_index]
                    };
                    sim.ps.ammoclip[d.clip_index] as i32 + reserve as i32 > 0
                })
            })
            .fold(0u64, |m, i| m | 1 << i);
        // The one in hand first: with two launchers carried, the other
        // read as the rocket and the one in hand fired at players. Else
        // any loaded one: an empty bazooka carried beside a loaded
        // panzerschreck read as no rocket, and the bot traded launcher
        // after launcher away fetching more (mp_foy, seed 15).
        let is_launcher = |w: u8| {
            weapons
                .get(w as usize)
                .is_some_and(|d| d.weapon_class == "rocketlauncher")
        };
        let rocket = held_rocket(sim.ps.weapon, sim.ps.weapons_held & loaded, is_launcher);
        let launchers = (1u8..64)
            .filter(|&w| sim.ps.weapons_held >> w & 1 == 1 && is_launcher(w))
            .fold(0u64, |m, w| m | 1 << w);
        let gun = (1u8..64).find(|&i| {
            loaded >> i & 1 == 1
                && weapons
                    .get(i as usize)
                    .is_some_and(|d| d.weapon_type == "bullet" && d.weapon_slot != "pistol")
        });
        Some(crate::bots::BotView {
            origin: sim.ps.origin.into(),
            eye_height: sim.ps.view_height(),
            eye_settled: sim.ps.view_height_settled(),
            lean_offset: vcod_common::pmove::aim::lean_offset(sim.ps.yaw, sim.ps.lean).into(),
            delta_angles: sim.delta_angles(),
            view: sim.view_angles(),
            gun_offset: sim.gun_offset(),
            weapon: sim.ps.weapon,
            weapons_held: sim.ps.weapons_held,
            clip: def.map_or(-1, |d| sim.ps.ammoclip[d.clip_index]),
            fire_time_ms: def.map_or(0, |d| (d.fire_time * 1000.0) as i32),
            busy_ms: sim.ps.weapon_time_ms,
            automatic: def.is_some_and(|d| !d.semi_auto),
            has_ads: def.is_some_and(|d| d.aim_down_sight),
            sniper: def.is_some_and(|d| d.ads_overlay_shader.is_some()),
            lmg: sim.ps.rules == vcod_common::pmove::MoveRules::Uo
                && def.is_some_and(|d| d.is_lmg()),
            deployed: sim.ps.deployed(),
            deploy_yaw: sim.ps.prone_direction,
            ads_frac: sim.ps.weapon_pos_frac,
            spread_scale: sim.ps.aim_spread_scale,
            speed: sim.ps.velocity.truncate().length(),
            velocity: sim.ps.velocity.into(),
            dead: sim.dead,
            playing: sim.pm_type == crate::spectate::PmType::Normal && !sim.dead,
            enemy,
            hurt: self.bot_hurt(sim),
            // Checked in pass 2 against this tick's teammates.
            friend_in_line: false,
            sidestep: None,
            cover: None,
            hunt: None,
            use_takes_weapon: sim.cursor_hint >= self.game.proto.cursor_hints.weapon,
            grenade,
            grenade_cooks: grenade
                .and_then(|g| weapons.get(g as usize))
                .is_some_and(|d| d.cook_off_hold),
            grenade_ready: sim.ps.grenade_time_left_ms > 0 && sim.ps.weapon_delay_ms <= 1,
            frag_near: self.game.module.script.as_ref().and_then(|rt| {
                let at = sim.ps.origin;
                let eye = glam::Vec3::from(sim.eye_origin());
                rt.missiles()
                    .armed()
                    .filter(|(_, w, _, _)| {
                        weapons.get(*w as usize).is_some_and(|d| {
                            d.explosion_radius > 0.0 && d.explosion_inner_damage > 0
                        })
                    })
                    // A cook dropped by a thrower killed holding it is seen
                    // fall: one went off by a teammate 3.5 s on and killed
                    // him (bot-combat.md 12).
                    .filter(|(o, _, owner, dropped)| {
                        *owner == Some(slot)
                            || *dropped
                            || self.game.world.as_ref().is_none_or(|w| {
                                crate::bots::notices_frag(eye, sim.view_angles()[1], *o, |a, b| {
                                    w.collision.shot_trace(a, b).fraction >= 1.0
                                })
                            })
                    })
                    .map(|(o, _, _, _)| o)
                    .filter(|o| {
                        (o.z - at.z).abs() < crate::bots::FLEE_HEIGHT
                            && (*o - at).truncate().length() < crate::bots::FLEE_REACH
                    })
                    .min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)))
                    .map(Into::into)
            }),
            post: None,
            give_way: None,
            // Probed in pass 3, on the ticks the brain throws.
            lob: None,
            waypoint: None,
            hazard_ahead: std::array::from_fn(|i| {
                self.game.world.as_ref().is_some_and(|w| {
                    let (s, c) = (i as f32 * 45.0).to_radians().sin_cos();
                    let ahead = glam::Vec3::new(c, s, 0.0) * crate::bots::HAZARD_LOOK;
                    crate::nav::hazard(&w.hazards, sim.ps.origin + ahead)
                })
            }),
            // Felt in pass 3, on the ticks the brain reads it.
            drop_ahead: [None; 8],
            // Felt in pass 3, on the ticks the brain reads it.
            wall_ahead: [false; 8],
            linked: sim.link_to.is_some(),
            stranded: self.game.module.driver.nav.as_ref().is_some_and(|g| {
                let at = sim.ps.origin.into();
                let spawns = self
                    .game
                    .world
                    .as_ref()
                    .map_or(&[][..], |w| &w.spawn_points);
                g.dead_end(at) || (self.nav_settled() && g.pit(at, spawns))
            }) || self
                .game
                .module
                .driver
                .paths
                .get(&slot)
                .is_some_and(|f| f.penned()),
            on_ladder: sim.ps.on_ladder,
            ladder_normal: sim.ps.ladder_normal.into(),
            tops_out: false,
            ladder_middle: self
                .game
                .module
                .driver
                .nav
                .as_ref()
                .filter(|_| sim.ps.on_ladder)
                .and_then(|g| g.ladder_middle(sim.ps.origin))
                .map(Into::into),
            back_face: None,
            on_ground: sim.ps.on_ground,
            jump: false,
            sliding: sim.ps.sliding(),
            leap: false,
            lip: false,
            crouch_jump: false,
            stop: false,
            mate_ahead: false,
            squad: Default::default(),
            run_off: false,
            mantle: false,
            move_scale: def
                .map(|d| d.move_speed_scale)
                .filter(|&m| m > 0.0)
                .unwrap_or(1.0),
            fatigue: (sim.ps.rules == vcod_common::pmove::MoveRules::Uo).then_some(sim.ps.fatigue),
            sprinting: sim.ps.sprinting,
            sprint_room: 0.0,
            push_off: None,
            unpin: None,
            detour: false,
            pistol,
            rocket,
            primaries: [sim.ps.weapon_slots[1], sim.ps.weapon_slots[2]],
            launchers,
            gun,
            reload_ms,
            draw_ms,
            loaded,
            sd: None,
            re: None,
            bel: None,
            uo: None,
            noise: None,
            // Stock `sd.gsc` and `re.gsc` spawn the dead as spectators.
            one_life: self
                .game
                .level_cvars
                .as_ref()
                .is_some_and(|(g, _)| g == "sd" || g == "re"),
            survive: crate::bots::retreat::Vitals {
                health: sim.health,
                max_health: sim.max_health,
                clip_size: def.map_or(0, |d| d.clip_size.min(i16::MAX as u32) as i16),
                enemy_eye: enemy.and_then(|e| {
                    let s = self.core.clients.get(e.slot)?.as_ref()?.sim.as_ref()?;
                    Some(s.ps.view().eye.into())
                }),
                heal: None,
                cover: None,
            },
            // Filled for the level's vehicles in pass 2.
            vehicles: Vec::new(),
            seat: None,
            armor: None,
        })
    }

    /// The S&D objectives for this frame's bot views, on an `sd` level only:
    /// the attacking and defending team values and the view with a
    /// placeholder role. Read fresh every frame, since a `map_restart`
    /// respawns the zones under new handles.
    pub(super) fn bot_sd(&mut self) -> Option<(i32, i32, crate::bots::SdView)> {
        if self
            .game
            .level_cvars
            .as_ref()
            .is_none_or(|(g, _)| g != "sd")
        {
            return None;
        }
        let o = self.game.module.script.as_mut()?.sd_objectives();
        let team = |t: &str| match t {
            "axis" => script::TEAM_AXIS,
            "allies" => script::TEAM_ALLIES,
            _ => -1,
        };
        let sites = o
            .sites
            .iter()
            .map(|&(mins, maxs)| crate::bots::SiteView {
                mins,
                maxs,
                stand: self.site_stand(mins, maxs),
            })
            .collect();
        let bomb = o.bomb.map(|(origin, (lo, hi))| crate::bots::BombView {
            origin,
            aim: std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5),
        });
        Some((
            team(&o.attackers),
            team(&o.defenders),
            crate::bots::SdView {
                role: crate::bots::ObjRole::Attack,
                sites,
                bomb,
                rank: 0,
                lead: false,
                cover: None,
                key: None,
            },
        ))
    }

    /// United Offensive's `ctf`, `dom`, `hq` and `bas` state for this
    /// frame's bot views, on such a level only. Test-facing too.
    pub fn bot_uo(&mut self) -> Option<script::UoObjectives> {
        let gametype = self.game.level_cvars.as_ref()?.0.clone();
        let clients = self.core.clients.len();
        self.game
            .module
            .script
            .as_mut()?
            .uo_objectives(&gametype, clients)
    }

    /// One bot's [`crate::bots::UoView`] off [`Self::bot_uo`]'s state: its
    /// team's objectives against the other's, each zone with the point the
    /// graph reaches in it.
    pub(super) fn bot_uo_view(
        &mut self,
        uo: &script::UoObjectives,
        slot: usize,
        team: i32,
        rank: usize,
        breach: &crate::bots::Breach,
    ) -> crate::bots::UoView {
        let rocket = breach.rocket;
        use crate::bots::*;
        let origin_of = |c: usize| -> Option<[f32; 3]> {
            Some(
                self.core
                    .clients
                    .get(c)?
                    .as_ref()?
                    .sim
                    .as_ref()?
                    .ps
                    .origin
                    .into(),
            )
        };
        let escort = match uo {
            script::UoObjectives::Ctf(flags) => flags
                .iter()
                .find(|f| f.team != team)
                .and_then(|f| f.carrier)
                .filter(|c| *c != slot)
                .and_then(origin_of),
            _ => None,
        };
        let mut site = |(mins, maxs): script::Bounds| SiteView {
            mins,
            maxs,
            stand: self.site_stand(mins, maxs),
        };
        let mid = |(lo, hi): script::Bounds| -> [f32; 3] {
            std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5)
        };
        match uo {
            script::UoObjectives::Ctf(flags) => {
                let mut flag = |f: &script::CtfFlag| CtfFlagView {
                    home: f.home.map(&mut site),
                    dropped: f.dropped.map(&mut site),
                    goal: site(f.goal),
                    carried: f.carrier.is_some(),
                };
                let ours = flags.iter().find(|f| f.team == team);
                let theirs = flags.iter().find(|f| f.team != team);
                let (Some(ours), Some(theirs)) = (ours, theirs) else {
                    return UoView::Dom(DomView {
                        flags: Vec::new(),
                        rank,
                    });
                };
                UoView::Ctf(Box::new(CtfView {
                    ours: flag(ours),
                    theirs: flag(theirs),
                    carrying: theirs.carrier == Some(slot),
                    escort,
                    rank,
                }))
            }
            script::UoObjectives::Dom(flags) => UoView::Dom(DomView {
                flags: flags
                    .iter()
                    .map(|f| DomFlagView {
                        site: site(f.bounds),
                        ours: f.team == team,
                        contested: false,
                    })
                    .collect(),
                rank,
            }),
            script::UoObjectives::Hq { radios, next } => UoView::Hq(HqView {
                radio: radios.first().map(|r| HqRadioView {
                    at: r.at,
                    radius: r.radius,
                    height: crate::bots::RADIO_HEIGHT,
                    stand: r.at,
                    ours: r.team == team,
                }),
                next: *next,
                rank,
            }),
            script::UoObjectives::Bas(bases) => UoView::Bas(BasView {
                bases: bases
                    .iter()
                    .map(|b| BasBaseView {
                        at: b.at,
                        ours: b.team == team,
                        breached: b.breached,
                        planted: b.planted,
                        blows_in: b.blows_in,
                        charge: b.charge.map(|(bounds, model)| BombView {
                            origin: model,
                            aim: mid(bounds),
                        }),
                        shot: (rocket && b.team != team && !b.breached)
                            .then(|| self.breach_shot(slot, b.at, breach.models))
                            .flatten(),
                        stands: if rocket && b.team != team && !b.breached {
                            let _p = crate::tick_profile::phase("uo/breach stands");
                            self.breach_stands(b.at, breach.models)
                        } else {
                            Vec::new()
                        },
                    })
                    .collect(),
                launchers: breach.launchers.to_vec(),
                guns: breach.guns.to_vec(),
                rank,
            }),
        }
    }

    /// The live objective records as `(index, teamNum, origin)`, on a `bel`
    /// level only: the allied players' compass markers.
    pub(super) fn bot_bel(&self) -> Option<Vec<(usize, i32, [f32; 3])>> {
        if self
            .game
            .level_cvars
            .as_ref()
            .is_none_or(|(g, _)| g != "bel")
        {
            return None;
        }
        let rt = self.game.module.script.as_ref()?;
        Some(
            rt.host
                .objectives
                .iter()
                .enumerate()
                .filter(|(_, o)| o.state != 0)
                .map(|(i, o)| (i, o.team_num, o.origin_f32()))
                .collect(),
        )
    }

    /// The retrieval objectives for this frame's bot views, on an `re` level
    /// only: the attacking and defending team values and each objective,
    /// with `mine` and the compass filter left for the caller.
    pub(super) fn bot_re(&mut self) -> Option<(i32, i32, Vec<ReObjCarried>)> {
        if self
            .game
            .level_cvars
            .as_ref()
            .is_none_or(|(g, _)| g != "re")
        {
            return None;
        }
        let clients = self.core.clients.len();
        let o = self.game.module.script.as_mut()?.re_objectives(clients);
        let team = |t: &str| match t {
            "axis" => script::TEAM_AXIS,
            "allies" => script::TEAM_ALLIES,
            _ => -1,
        };
        let objectives = o
            .objectives
            .iter()
            .map(|r| {
                let mid = |(lo, hi): ([f32; 3], [f32; 3])| -> [f32; 3] {
                    std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5)
                };
                let (lo, hi) = r.goal;
                let goal = self.site_stand(lo, hi);
                // The farthest corner, flat, and a body's half width past it.
                let goal_clear = [lo[0], hi[0]]
                    .iter()
                    .flat_map(|&x| [lo[1], hi[1]].map(|y| (x - goal[0]).hypot(y - goal[1])))
                    .fold(0.0, f32::max)
                    + 16.0;
                let carrier_at = r.carrier.and_then(|slot| {
                    let sim = self.core.clients.get(slot)?.as_ref()?.sim.as_ref()?;
                    Some(sim.ps.origin.into())
                });
                let view = crate::bots::ReObjView {
                    pickup: r.pickup.map(mid),
                    goal,
                    goal_clear,
                    carried: r.carrier.is_some(),
                    mine: false,
                    carrier_at,
                    laid: r.laid,
                };
                (view, r.carrier, r.shown_to)
            })
            .collect();
        Some((team(&o.attackers), team(&o.defenders), objectives))
    }

    /// Where a bot stands in a zone (a bombzone, a retrieval goal): the
    /// graph node inside the bounds nearest their middle, from the
    /// component holding the most nodes so a bot anywhere can reach it,
    /// else the nearest such node to the middle. Cached per zone; the zones
    /// keep their bounds across rounds.
    pub(super) fn site_stand(&mut self, mins: [f32; 3], maxs: [f32; 3]) -> [f32; 3] {
        let mid = [
            (mins[0] + maxs[0]) * 0.5,
            (mins[1] + maxs[1]) * 0.5,
            mins[2],
        ];
        if let Some(&(_, _, p)) = self
            .game
            .module
            .driver
            .sites
            .iter()
            .find(|(lo, hi, _)| *lo == mins && *hi == maxs)
        {
            return p;
        }
        let Some(g) = self.game.module.driver.nav.clone() else {
            return mid;
        };
        let p = g.zone_stand(mins, maxs).unwrap_or(mid);
        self.game.module.driver.sites.push((mins, maxs, p));
        p
    }

    /// The live, playing client on another team with a clear eye line that
    /// the bot in `slot` fights: the one who hit it within
    /// [`crate::bots::HURT_MS`], else the target it tracks while in sight,
    /// unless a new one in the sight cone stands nearer than
    /// [`crate::bots::RETARGET_RATIO`] of its range, else the nearest in
    /// the cone ([`crate::bots::Skill::fov_deg`]). Team rules: axis and
    /// allies only fight each other; everyone else is fair game to everyone
    /// (`dm` carries no teams).
    pub(super) fn bot_enemy(&self, slot: usize, teams: &[i32]) -> Option<crate::bots::EnemyView> {
        if !self.game.module.bot_admin.shoot {
            return None;
        }
        let me = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        if me.pm_type != crate::spectate::PmType::Normal || me.dead {
            return None;
        }
        let my_team = teams.get(slot).copied().unwrap_or(0);
        let team_enemy = |their: i32| match my_team {
            script::TEAM_AXIS => their != script::TEAM_AXIS,
            script::TEAM_ALLIES => their != script::TEAM_ALLIES,
            _ => true,
        };
        let my_eye: glam::Vec3 = me.eye_origin().into();
        let world = self.game.world.as_ref().map(|w| &w.collision);
        let bot = self.game.module.driver.bots.get(&slot);
        let tracked = bot.and_then(|b| b.target_slot());
        let aware = bot.is_none_or(|b| b.aware);
        let attacker = self.bot_hurt(me).map(|h| h.slot).filter(|_| aware);
        let tracked = tracked.filter(|_| aware);
        // A seat's view is the seat's (a tank's driver looks down the
        // hull), not where the bot turns its head: crews see all round.
        let fov = match bot {
            Some(b) if b.aware && me.seat.is_none() => b.skill.fov_deg,
            Some(_) => 360.0,
            None => crate::bots::Skill::default().fov_deg,
        };
        let view = me.view_angles();
        // In-range candidates nearest first: the tracked target and the
        // attacker wherever they stand, the rest only inside the cone.
        let mut candidates: Vec<(f32, glam::Vec3, crate::bots::EnemyView)> = Vec::new();
        for (i, c) in self.core.clients.iter().enumerate() {
            if i == slot {
                continue;
            }
            let Some(s) = c.as_ref().and_then(|c| c.sim.as_ref()) else {
                continue;
            };
            if s.pm_type != crate::spectate::PmType::Normal || s.dead {
                continue;
            }
            if !team_enemy(teams.get(i).copied().unwrap_or(0)) {
                continue;
            }
            // A tank's driver is out of every client's sight and every
            // round's reach (docs/research/coduo-vehicles.md 13).
            if self
                .game
                .module
                .script
                .as_ref()
                .is_some_and(|rt| rt.host.vehicles.drives_tank(i))
            {
                continue;
            }
            let d = crate::bots::dist_sq(me.ps.origin.into(), s.ps.origin.into());
            if d > crate::bots::SHOOT_RANGE * crate::bots::SHOOT_RANGE {
                continue;
            }
            let chest = crate::bots::chest(s.ps.origin.into(), s.ps.view_height());
            let known = Some(i) == tracked || Some(i) == attacker;
            if !known && !crate::bots::in_sight_cone(my_eye, view, chest.into(), fov) {
                continue;
            }
            candidates.push((
                d,
                s.ps.view().eye,
                crate::bots::EnemyView {
                    slot: i,
                    origin: chest,
                    velocity: s.ps.velocity.into(),
                },
            ));
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let sighted = |(_, eye, enemy): &(f32, glam::Vec3, crate::bots::EnemyView)| {
            let Some(w) = world else {
                return Some(*enemy);
            };
            let clear = |to: glam::Vec3| w.shot_trace(my_eye, to).fraction >= 1.0;
            let origin = crate::bots::aim_point(enemy.origin, (*eye).into(), |p| clear(p.into()))?;
            Some(crate::bots::EnemyView { origin, ..*enemy })
        };
        let order: Vec<(f32, usize)> = candidates.iter().map(|c| (c.0, c.2.slot)).collect();
        crate::bots::pick_enemy(&order, attacker, tracked, |i| sighted(&candidates[i]))
    }

    /// [`crate::bots::BotView::hurt`] for the body `sim`: its last hit by
    /// another client within [`crate::bots::HURT_MS`], while he still
    /// plays.
    pub(super) fn bot_hurt(&self, sim: &crate::spectate::ClientSim) -> Option<crate::bots::Hurt> {
        let (a, at, dir) = sim.last_hit?;
        let ago_ms = self.core.sv_time_ms.wrapping_sub(at);
        if !(0..crate::bots::HURT_MS).contains(&ago_ms) {
            return None;
        }
        let s = self.core.clients.get(a)?.as_ref()?.sim.as_ref()?;
        if s.pm_type != crate::spectate::PmType::Normal || s.dead {
            return None;
        }
        Some(crate::bots::Hurt {
            slot: a,
            ago_ms,
            dir,
        })
    }

    /// [`crate::bots::BotView::friend_in_line`] for the body in `slot`
    /// shooting at `target`: its live axis or allies teammates' boxes this
    /// tick (the enemy is cached, the friends are not); and when one is in
    /// the line, [`crate::bots::BotView::sidestep`], a graph node near
    /// whose eye clears it: `held`, the spot and pick time the bot is
    /// already walking to, while it still clears and is not yet reached.
    pub(super) fn bot_friend_in_line(
        &self,
        slot: usize,
        target: [f32; 3],
        teams: &[i32],
        held: Option<([f32; 3], i32)>,
    ) -> (bool, Option<[f32; 3]>) {
        let my_team = teams.get(slot).copied().unwrap_or(0);
        if my_team != script::TEAM_AXIS && my_team != script::TEAM_ALLIES {
            return (false, None);
        }
        let Some(me) = self.core.clients[slot]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
        else {
            return (false, None);
        };
        let friends: Vec<(glam::Vec3, glam::Vec3)> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != slot && teams.get(*i).copied() == Some(my_team))
            .filter_map(|(_, c)| c.as_ref()?.sim.as_ref())
            .filter(|o| o.pm_type == crate::spectate::PmType::Normal && !o.dead)
            // A crew mate in the bot's own vehicle sits beside its gun.
            .filter(|o| {
                me.seat.is_none() || o.seat.map(|s| s.vehicle) != me.seat.map(|s| s.vehicle)
            })
            .map(|o| (o.ps.origin + o.ps.mins(), o.ps.origin + o.ps.maxs()))
            .collect();
        let eye = crate::game::combat::muzzle_point(&me.ps);
        let target = glam::Vec3::from(target);
        if !crate::bots::friend_in_line(eye, target, &friends) {
            return (false, None);
        }
        let (Some(g), Some(w)) = (
            self.game.module.driver.nav.as_ref(),
            self.game.world.as_ref(),
        ) else {
            return (true, None);
        };
        let lift = eye - me.ps.origin;
        let clear = |n: glam::Vec3| {
            let eye = n + lift;
            !crate::bots::friend_in_line(eye, target, &friends)
                && w.collision.shot_trace(eye, target).fraction >= 1.0
        };
        if let Some(p) = crate::bots::held_sidestep(held, self.core.sv_time_ms, me.ps.origin, clear)
        {
            return (true, Some(p));
        }
        let step = g.sidestep(me.ps.origin, target - me.ps.origin, clear);
        (true, step.map(Into::into))
    }
}

/// The objective state [`Server`]'s bots read once a tick: S&D, retrieval,
/// Behind Enemy Lines' markers, UO's four, and a `bas` level's launchers
/// and guns lying about.
pub(crate) struct Cod1Frame {
    sd: Option<(i32, i32, crate::bots::SdView)>,
    re: Option<(i32, i32, Vec<ReObjCarried>)>,
    bel: Option<Vec<(usize, i32, [f32; 3])>>,
    uo: Option<script::UoObjectives>,
    launchers: Vec<[f32; 3]>,
    guns: Vec<[f32; 3]>,
}

/// CoD1's game under the bots' driver (1.1, 1.5 and UO, vehicles included).
impl crate::bots::driver::BotWorld for Server {
    type Frame = Cod1Frame;
    type Cmd = UserCmd;

    fn driver(&self) -> &crate::bots::driver::BotDriver {
        &self.game.module.driver
    }

    fn driver_mut(&mut self) -> &mut crate::bots::driver::BotDriver {
        &mut self.game.module.driver
    }

    fn now(&self) -> i32 {
        self.core.sv_time_ms
    }

    fn server_id(&self) -> i32 {
        i32::from(self.game.server_id)
    }

    fn world(&self) -> Option<&World> {
        self.game.world.as_deref()
    }

    fn connected(&self, slot: usize) -> bool {
        self.core.clients[slot].is_some()
    }

    fn take_noises(&mut self) -> Vec<crate::bots::Noise> {
        let mut noises = std::mem::take(&mut self.game.module.driver.noises);
        // A script's `radiusDamage` (the S&D bomb, an exploder) is nobody's.
        if let Some(rt) = self.game.module.script.as_mut() {
            noises.extend(
                rt.take_blast_noises()
                    .into_iter()
                    .map(|at| crate::bots::Noise {
                        at: [at[0], at[1], at[2] + 40.0],
                        source: usize::MAX,
                        radius: crate::bots::HEAR_BLAST,
                    }),
            );
        }
        noises
    }

    fn manage(&mut self) {
        bot_admin::manage(self);
    }

    fn nav_tick(&mut self) {
        let links = self
            .game
            .module
            .script
            .as_ref()
            .map_or(0, |rt| rt.host.brush_links);
        if self.game.module.driver.nav.is_none()
            && let Some(w) = self.game.world.as_ref()
        {
            let rules = self.move_rules();
            let job = self.game.module.driver.nav_job.get_or_insert_with(|| {
                self.game.module.nav_links = links;
                crate::nav::NavJob::start(&self.game.cfg.map, w, rules)
            });
            self.game.module.driver.nav = job.poll(!self.game.module.driver.nav_background);
        }
        self.relink_nav(links);
        self.park_vehicles();
        self.warm_hull_fits();
    }

    fn teams(&mut self) -> Vec<i32> {
        (0..self.core.clients.len())
            .map(|slot| {
                self.game
                    .module
                    .script
                    .as_mut()
                    .map_or(0, |rt| rt.client_team(slot))
            })
            .collect()
    }

    fn read_commands(
        &mut self,
        slot: usize,
        sid: i32,
        replies: &mut Vec<(usize, String)>,
    ) -> crate::bots::driver::Joining {
        use crate::bots::driver::Joining;
        // `_teams::restrict` reads `!getcvar(...)`, a numeric coercion; with
        // no script nothing restricts.
        let cvars = self.game.module.script.as_ref().map(|rt| rt.cvars());
        let allowed = |name: &str| {
            cvars.is_none_or(|c| crate::game::builtins::cvar::atof(c.get(name)) != 0.0)
        };
        let Some(c) = self.core.clients[slot].as_ref() else {
            return Joining::No;
        };
        let Some(bot) = self.game.module.driver.bots.get_mut(&slot) else {
            return Joining::No;
        };
        let fresh: Vec<(i32, &str)> = (bot.last_seen_seq + 1
            ..=c.link.netchan.reliable_sequence as i32)
            .map(|seq| {
                (
                    seq,
                    c.link.netchan.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)].as_str(),
                )
            })
            .collect();
        bot.last_seen_seq = c.link.netchan.reliable_sequence as i32;
        // 1.5's gametypes open `serverinfo_<gametype>` first and the team
        // menu once it answers `close` (cod15-protocol-diff.md 7).
        let (serverinfo, fresh): (Vec<_>, Vec<_>) = fresh.into_iter().partition(|(_, text)| {
            text.strip_prefix("t ")
                .and_then(|i| i.trim().parse::<usize>().ok())
                .and_then(|i| {
                    self.core
                        .configstrings
                        .get(self.game.proto.cs.bounds(configstrings::CsRange::Menu).0 + i)
                })
                .is_some_and(|m| m.starts_with("serverinfo_"))
        });
        for (_, text) in serverinfo {
            replies.push((slot, format!("mr {sid} {} close", &text[2..])));
        }
        // 1.5's `map_restart` sends `B` where 1.1 sends `n`.
        if fresh.iter().any(|(_, text)| *text == "B") {
            bot.rearm();
        }
        // The menu a `t` opens is the one its configstring names: UO's
        // stock scripts send the weapon menu's `t` ahead of the
        // `g_scriptMainMenu` that names it, so the bot is told first.
        let menus = self.game.proto.cs.bounds(configstrings::CsRange::Menu).0;
        let named: Vec<(i32, String)> = fresh
            .iter()
            .flat_map(|&(seq, text)| {
                let menu = text
                    .strip_prefix("t ")
                    .and_then(|i| i.trim().parse::<usize>().ok())
                    .and_then(|i| self.core.configstrings.get(menus + i))
                    .filter(|m| !m.is_empty())
                    .map(|m| (seq, format!("v g_scriptMainMenu {m}")));
                menu.into_iter().chain([(seq, text.to_string())])
            })
            .collect();
        let named: Vec<(i32, &str)> = named.iter().map(|(q, t)| (*q, t.as_str())).collect();
        replies.extend(
            bot.observe(sid, &named, &allowed)
                .into_iter()
                .map(|r| (slot, r)),
        );
        match c.state {
            ClientState::Connected => Joining::Joined,
            // Primed and no sim yet: the entering cmd, which is not
            // simulated (enter_world consumes it).
            ClientState::Primed if c.sim.is_none() => Joining::Entering,
            _ => Joining::No,
        }
    }

    fn frame(&mut self) -> Cod1Frame {
        let sd = self.bot_sd();
        let re = self.bot_re();
        let bel = self.bot_bel();
        let uo = self.bot_uo();
        let (launchers, guns) = self.bot_launchers(uo.as_ref());
        Cod1Frame {
            sd,
            re,
            bel,
            uo,
            launchers,
            guns,
        }
    }

    fn health_packs(&mut self) -> Vec<[f32; 3]> {
        self.game
            .module
            .script
            .as_mut()
            .map_or(Vec::new(), |rt| rt.health_items())
    }

    fn body(&self, slot: usize) -> Option<crate::bots::driver::Body> {
        let sim = self.core.clients.get(slot)?.as_ref()?.sim.as_ref()?;
        Some(crate::bots::driver::Body {
            origin: sim.ps.origin,
            velocity: sim.ps.velocity,
            playing: sim.pm_type == crate::spectate::PmType::Normal && !sim.dead,
            on_ground: sim.ps.on_ground,
            on_ladder: sim.ps.on_ladder,
        })
    }

    fn enemy(&self, slot: usize, teams: &[i32]) -> Option<crate::bots::EnemyView> {
        self.bot_enemy(slot, teams)
    }

    fn view(
        &self,
        slot: usize,
        enemy: Option<crate::bots::EnemyView>,
    ) -> Option<crate::bots::BotView> {
        self.bot_view(slot, enemy)
    }

    fn friend_in_line(
        &self,
        slot: usize,
        target: [f32; 3],
        teams: &[i32],
        held: Option<([f32; 3], i32)>,
    ) -> (bool, Option<[f32; 3]>) {
        self.bot_friend_in_line(slot, target, teams, held)
    }

    fn cover_body(&self, slot: usize, threat: usize) -> Option<crate::bots::driver::CoverBody> {
        let me = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        let threat = self.core.clients[threat].as_ref()?.sim.as_ref()?;
        Some(crate::bots::driver::CoverBody {
            origin: me.ps.origin,
            threat_eye: threat.ps.view().eye,
            lean: if me.ps.rules.v15() { 0.5 } else { 1.0 },
            mask: vcod_common::collision::MASK_PLAYERSOLID,
        })
    }

    fn deal(
        &mut self,
        frame: Cod1Frame,
        teams: &[i32],
        solo: &[usize],
        round: u32,
        views: &mut Vec<(usize, crate::bots::BotView)>,
    ) {
        let Cod1Frame {
            sd,
            re,
            bel,
            uo,
            launchers,
            guns,
        } = frame;
        if let Some((attackers, defenders, objectives)) = &re {
            let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
            let slots: Vec<usize> = views.iter().map(|(s, _)| *s).collect();
            // The attackers by each carrier who keep by him (`team::escorts`).
            let escorts: Vec<usize> = objectives
                .iter()
                .filter_map(|(o, carrier, _)| Some((o.carrier_at?, (*carrier)?)))
                .flat_map(|(at, carrier)| {
                    let mates: Vec<(usize, [f32; 3])> = views
                        .iter()
                        .filter(|(s, v)| {
                            *s != carrier && team(*s) == team(carrier) && v.playing && !v.dead
                        })
                        .map(|(s, v)| (*s, v.origin))
                        .collect();
                    crate::bots::team::escorts(at, &mates)
                })
                .collect();
            for (slot, view) in views.iter_mut() {
                let mine = team(*slot);
                let role = if mine == *attackers {
                    crate::bots::ObjRole::Attack
                } else if mine == *defenders {
                    crate::bots::ObjRole::Defend
                } else {
                    continue;
                };
                let mates: Vec<usize> =
                    slots.iter().copied().filter(|s| team(*s) == mine).collect();
                let alone = solo.contains(slot);
                view.re = Some(crate::bots::ReView {
                    role,
                    rank: if alone {
                        mates.iter().filter(|s| *s < slot).count()
                    } else {
                        crate::bots::team::round_rank(*slot, &mates, mine, round)
                    },
                    escort: alone || escorts.contains(slot),
                    objectives: objectives
                        .iter()
                        .map(|(o, carrier, shown_to)| crate::bots::ReObjView {
                            mine: *carrier == Some(*slot),
                            // What the bot's compass shows.
                            carrier_at: o
                                .carrier_at
                                .filter(|_| *shown_to == 0 || *shown_to == mine),
                            ..*o
                        })
                        .collect(),
                });
            }
        }
        if let Some(markers) = &bel {
            for (slot, view) in views.iter_mut() {
                let mine = teams.get(*slot).copied().unwrap_or(0);
                if mine != script::TEAM_AXIS && mine != script::TEAM_ALLIES {
                    continue;
                }
                view.bel = Some(crate::bots::BelView {
                    hunted: mine == script::TEAM_ALLIES,
                    // What the compass shows the team (`objectives_for`).
                    markers: markers
                        .iter()
                        .filter(|(_, t, _)| *t == 0 || *t == mine)
                        .map(|(_, _, at)| *at)
                        .collect(),
                    // `make_obj_marker` numbers a player's record
                    // entnum + 1 (`bel.gsc` 1574).
                    trail: markers
                        .iter()
                        .find(|(n, _, _)| *n == *slot + 1)
                        .map(|(_, _, at)| *at),
                });
            }
        }
        let p = crate::tick_profile::phase("deal/vehicles");
        self.bot_vehicle_views(views, teams);
        drop(p);
        let p = crate::tick_profile::phase("deal/breach models");
        let breach_models = if views.iter().any(|(_, v)| v.rocket.is_some()) {
            self.bot_breach_models(uo.as_ref())
        } else {
            Vec::new()
        };
        drop(p);
        let p = crate::tick_profile::phase("deal/uo");
        if let Some(uo) = &uo {
            let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
            let slots: Vec<usize> = views.iter().map(|(s, _)| *s).collect();
            for (slot, view) in views.iter_mut() {
                let mine = team(*slot);
                if mine != script::TEAM_AXIS && mine != script::TEAM_ALLIES {
                    continue;
                }
                let rank = slots
                    .iter()
                    .filter(|s| team(**s) == mine && *s < slot)
                    .count();
                let breach = crate::bots::Breach {
                    rocket: view.rocket.is_some(),
                    launchers: &launchers,
                    guns: &guns,
                    models: &breach_models,
                };
                view.uo = Some(self.bot_uo_view(uo, *slot, mine, rank, &breach));
            }
        }
        drop(p);
        if let Some((attackers, defenders, sd)) = &sd {
            let team = |slot: usize| teams.get(slot).copied().unwrap_or(0);
            // The team's bots that play, by slot, and each one's distance
            // to the bomb.
            let bomb_dist = |v: &crate::bots::BotView| {
                sd.bomb
                    .map_or(0.0, |b| crate::bots::dist_sq(v.origin, b.origin))
            };
            let side: Vec<(usize, i32, f32, bool)> = views
                .iter()
                .map(|(slot, v)| (*slot, team(*slot), bomb_dist(v), v.playing))
                .collect();
            // Who the script holds planting in a zone (`team::cover`).
            let planting: Vec<(usize, [f32; 3], bool)> = views
                .iter()
                .map(|(slot, v)| {
                    let held = v.playing
                        && v.linked
                        && sd
                            .sites
                            .iter()
                            .any(|z| crate::bots::inside_site(v.origin, z));
                    (*slot, v.origin, held)
                })
                .collect();
            for (slot, view) in views.iter_mut() {
                let mine = team(*slot);
                let role = if mine == *attackers {
                    crate::bots::ObjRole::Attack
                } else if mine == *defenders {
                    crate::bots::ObjRole::Defend
                } else {
                    continue;
                };
                let mates: Vec<usize> = side.iter().filter(|s| s.1 == mine).map(|s| s.0).collect();
                let attack = role == crate::bots::ObjRole::Attack;
                let alone = solo.contains(slot);
                let rank = if alone {
                    mates.iter().filter(|s| *s < slot).count()
                } else {
                    crate::bots::team::site_rank(attack, *slot, &mates, mine, round)
                };
                let cover = (attack && !alone)
                    .then(|| {
                        let mates: Vec<_> = planting
                            .iter()
                            .filter(|p| team(p.0) == mine)
                            .copied()
                            .collect();
                        crate::bots::team::cover(*slot, view.origin, &mates)
                    })
                    .flatten();
                // Nearest first, the lower slot on a tie.
                let lead = side
                    .iter()
                    .filter(|s| s.1 == mine && s.3)
                    .min_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)))
                    .is_some_and(|s| s.0 == *slot);
                view.sd = Some(crate::bots::SdView {
                    role,
                    rank,
                    lead,
                    cover,
                    ..sd.clone()
                });
            }
        }
    }

    fn move_ps(&self, slot: usize) -> Option<vcod_common::pmove::PlayerState> {
        self.core.clients[slot]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .map(|sim| sim.ps)
    }

    fn fall_kills(&self, slot: usize) -> Option<f32> {
        self.core.clients[slot]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .map(|sim| {
                crate::nav::fall_that_kills(
                    sim.health,
                    sim.max_health,
                    self.game.module.fall_heights,
                )
            })
    }

    fn blocked_nodes(&self) -> std::sync::Arc<[u32]> {
        self.game.module.bot_parked.blocked.clone()
    }

    fn cmd(&mut self, _: usize, cmd: UserCmd, _: &crate::bots::BotView) -> Option<UserCmd> {
        Some(cmd)
    }

    fn weapon_defs(&self) -> &[Option<vcod_common::weapon::WeaponDef>] {
        self.game.module.weapon_table.defs()
    }

    fn drive_waypoint(
        &mut self,
        slot: usize,
        goal: crate::bots::Goal,
        seat: &crate::bots::SeatView,
    ) -> Option<[f32; 3]> {
        Server::drive_waypoint(self, slot, goal, seat)
    }

    fn forget_drive(&mut self, slot: usize) {
        self.game.module.bot_drives.forget(slot);
    }

    fn lob_probe(
        &self,
        slot: usize,
        weapon: u8,
        target: [f32; 3],
        teams: &[i32],
    ) -> Option<[f32; 2]> {
        Server::lob_probe(self, slot, weapon, target, teams)
    }

    fn take_orders(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.game.module.bot_admin.orders)
    }

    fn bot_command(&mut self, slot: usize, text: String) {
        if let Some(bot) = self.game.module.driver.bots.get_mut(&slot) {
            let seq = bot.next_command_seq;
            bot.next_command_seq += 1;
            // Consumes its own ack bookkeeping; a bot is its own client.
            self.client_command(slot, seq, text);
        }
    }

    fn menu_index(&self, name: &str) -> Option<usize> {
        configstrings::script_menu_index(&self.core.configstrings, &self.game.proto.cs, name)
    }

    fn killer(&self, slot: usize) -> Option<usize> {
        let (a, at, _) = self
            .core
            .clients
            .get(slot)?
            .as_ref()?
            .sim
            .as_ref()?
            .last_hit?;
        (0..1000)
            .contains(&self.core.sv_time_ms.wrapping_sub(at))
            .then_some(a)
    }

    fn begin(&mut self, slot: usize) {
        self.send_gamestate(slot);
    }

    fn bot_packet(&mut self) {
        self.game.packet_seq += 1;
    }

    fn send_cmd(&mut self, slot: usize, cmd: UserCmd, on_ladder: bool) {
        // Stamped here, not in the brain: the cmd rides the server's own
        // clock, one 50 ms step per tick, five on a ladder.
        let cmds = crate::nav::tick_times(self.core.sv_time_ms.wrapping_sub(FRAME_MS), on_ladder)
            .map(|server_time| UserCmd { server_time, ..cmd })
            .collect();
        // Enters the world on the Primed pass, queues on Active.
        self.user_move(slot, cmds);
    }

    fn enter(&mut self, slot: usize) {
        // The entering cmd carries the tick's clock: entry sets the
        // client's cmd base to it, so the first simulated cmd steps a
        // sane 50 ms.
        self.user_move(
            slot,
            vec![UserCmd {
                server_time: self.core.sv_time_ms,
                ..NULL_USERCMD
            }],
        );
    }

    fn ack(&mut self, slot: usize) {
        if let Some(c) = self.core.clients[slot].as_mut() {
            c.link.reliable_ack = c.link.netchan.reliable_sequence as i32;
        }
    }
}
