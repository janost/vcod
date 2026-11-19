//! The view and the entities to draw ([`Scene`]), lerped between the two
//! newest snapshots, and what posing a body needs ([`BodyView`]).

use glam::Vec3;
use vcod_cod2::player::{BodyFrame, Character, eflags as cod2_eflags};
use vcod_cod2::trajectory::{self as tr2, Trajectory2};
use vcod_cod4::net::conn::angle2short;
use vcod_cod4::net::delta::{EntityState, PlayerState};
use vcod_cod4::net::events::event_name;
use vcod_cod4::net::parse::{Gamestate, Snapshot};
use vcod_cod4::pmove::weapon::WeaponDefs;
use vcod_cod4::pmove::{self as pm, Cmd, WeaponMove};

use super::events::{Fired, force_stance, source_client};
use super::{
    CS_MODELS, CS_TAGS, Cod4Session, Dry, EF_FIRING, EF_TELEPORT, ET_GENERAL, ET_HELICOPTER,
    ET_ITEM, ET_MG42, ET_MISSILE, ET_PLANE, ET_PLAYER, ET_PLAYER_CORPSE, ET_SCRIPTMOVER,
    MAX_MODELS, SOLID_BMODEL, Source, client_perks, client_team, client_u32, ent_f32, ent_u32,
    ps_f32, vec3, weapon_list,
};
use crate::play::cod4_predict::{self, Solid};
use crate::play::cod4_third_person::{self, ThirdPerson};
use crate::play::{cod4_kick, cod4_physics};

/// What an entity draws as.
#[derive(Clone, Debug, PartialEq)]
pub enum Visual {
    /// An xmodel by name (its box when no zone holds it).
    Model(String),
    /// Inline brush model `*N`.
    Submodel(usize),
    /// Nothing the client could name: its type's box.
    Box,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub number: u32,
    pub etype: u32,
    pub visual: Visual,
    pub origin: Vec3,
    /// Pitch, yaw, roll in degrees.
    pub angles: Vec3,
    /// A player's or corpse's pose inputs; `None` for anything else, or a
    /// body whose client state names no model.
    pub body: Option<BodyView>,
    /// A helicopter's controllers and damage stage; `None` for anything
    /// else.
    pub vehicle: Option<VehicleView>,
    /// A mounted gun's `lerp.u.turret.gunAngles` (pitch, yaw, the flash's
    /// pitch), lerped between the two snapshots; `None` for anything else.
    pub turret: Option<[f32; 3]>,
    /// The newer snapshot's `lerp.eFlags` 0x40: a gun firing
    /// (`CG_mg42_PreControllers`, cod4-client-view.md 13).
    pub firing: bool,
    /// The newer snapshot's `partBits`: the bones `hidePart` hid, whose
    /// surfaces the model draws without (cod4-client-view.md 15).
    pub part_bits: [u32; 4],
}

/// What a helicopter is posed and smoked from (the pose controllers and
/// the vehicle effects step, cod4-client-view.md 11): its
/// `lerp.u.vehicle` angles between the two snapshots, in degrees, and its
/// `un1.helicopterStage`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleView {
    pub body_pitch: f32,
    pub body_roll: f32,
    pub gun_pitch: f32,
    pub gun_yaw: f32,
    pub stage: u32,
}

/// What a player or corpse is posed from: its entity's anims, flags and
/// angles in CoD2's numbering (the shared pose code's), the models its
/// client state names and the weapon it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyView {
    pub frame: BodyFrame,
    pub character: Character,
    /// The weapon list's name (`m16_gl_mp`), `None` empty handed.
    pub weapon: Option<String>,
    /// Its `weaponModel`, the camo: the `worldModel[]` slot the held gun
    /// draws (`iw3mp.exe` 0x407401..0x40746a, cod4-client-view.md 4.7).
    pub weapon_model: usize,
    /// An `ET_PLAYER_CORPSE`: its models are the ones copied when it
    /// arrived, not its client's live ones (`play::cod4_players`).
    pub corpse: bool,
    /// Its `pos.trType` is one of `startRagdoll`'s (9 to 11): the client
    /// simulates its bones (`play::cod4_players`, cod4-combat.md 5.1).
    pub ragdoll: bool,
    /// A gunner's gun (`eFlags` 0x300): `otherEntityNum` when it names a
    /// general entity (`CG_PlayerTurretPositionAndBlend`'s test), and the
    /// gun's weapon when the snapshot carries it.
    pub gun: Option<(u32, Option<String>)>,
}

/// The frame to draw: the eye, its angles and the entities.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// The player `CG_ScanForCrosshairEntity`'s trace met this frame
    /// ([`crosshair_scan`]).
    pub crosshair_hit: Option<CrosshairHit>,
    pub eye: Vec3,
    /// The view weapon's lighting origin: the eye before its floor, the
    /// origin raised by `viewHeightCurrent` and leaned 16 and 20
    /// (`CG_AddViewWeapon`, iw3mp.exe 0x456dd7, cod4-world.md 3.3).
    pub light_eye: Vec3,
    /// Pitch, yaw, roll.
    pub angles: Vec3,
    pub entities: Vec<Drawn>,
    /// The map's dynamic entities that draw: the def's index and the pose
    /// (`vcod_cod4::dynent`).
    pub dynents: Vec<(u32, Vec3, glam::Quat)>,
    /// The time of the newer snapshot drawn (`cg.nextSnap`), whose
    /// states the entities carry.
    pub server_time: i32,
    /// The time drawn: between the two newest snapshots' (`cg.time`).
    pub render_time: i32,
    /// The view is another client's (a followed player, the killcam), not
    /// our own body's.
    pub following: bool,
    /// The view starts over between the two snapshots drawn (a spawn, a
    /// teleport, another client's view): drawn at the newer, unlerped.
    #[cfg_attr(not(test), allow(dead_code))]
    pub view_reset: bool,
    /// What the view weapon reads off the drawn playerstate.
    pub view: crate::cod4_viewmodel::ViewPs,
    /// `CS_MODELS + viewmodelIndex`, the hands the playerstate names.
    pub hands: Option<String>,
    /// The newer snapshot's `dof*` fields (`crate::cod4_dof`).
    pub dof: crate::cod4_dof::PsDof,
    /// `cg.renderingThirdPerson`: the camera drawn instead of the eye
    /// (`play::cod4_third_person`); `None` in first person.
    pub third_person: Option<ThirdPerson>,
}

impl Cod4Session {
    /// The view and entities at this frame's `cl.serverTime`
    /// ([`Self::step`]'s clock), lerped between the snapshots either side
    /// of it, as `CG_ProcessSnapshots` and `CG_InterpolatePlayerState` do
    /// with no prediction (cod4-client-view.md 2); our own view predicted
    /// over them (section 5), which steps the predictor, so once a frame.
    pub fn scene(&mut self) -> Option<Scene> {
        // The source alone, so the predictor can be stepped beside it.
        let c = source_client(&self.source);
        let gs = c.gamestate.as_ref()?;
        let time = self.cg_time?;
        // `cg.snap`: the newest at or before the time (the first when the
        // time is before them all); `cg.nextSnap` the first after it, or
        // `cg.snap` again when none has come.
        let at = c
            .snapshots
            .iter()
            .rposition(|s| s.server_time <= time)
            .unwrap_or(0);
        let old = &c.snapshots[at];
        let new = c.snapshots[at + 1..]
            .iter()
            .find(|s| s.server_time > old.server_time)
            .unwrap_or(old);
        let time = time.max(old.server_time);
        // `CG_SetFrameInterpolation`.
        let frac = match new.server_time - old.server_time {
            0 => 0.0,
            span => ((time - old.server_time) as f32 / span as f32).max(0.0),
        };
        // A respawn, a teleport or another client's view starts over:
        // `CG_SetNextSnap` copies the new playerstate over the old.
        let ps_u = |ps: &PlayerState, f: &str| ps.get(f).unwrap_or(0);
        let reset = (ps_u(&old.ps, "eFlags") ^ ps_u(&new.ps, "eFlags")) & EF_TELEPORT != 0
            || ps_u(&old.ps, "clientNum") != ps_u(&new.ps, "clientNum")
            || old.ps.stats[4] != new.ps.stats[4];
        let old_ps = if reset { &new.ps } else { &old.ps };
        let f = |ps: &PlayerState, name: &str| ps_f32(ps, name);
        let lerp1 = |name: &str| f(old_ps, name) + (f(&new.ps, name) - f(old_ps, name)) * frac;
        let mut view_height = lerp1("viewHeightCurrent");
        let mut eye = vec3(|k| lerp1(&format!("origin[{k}]"))) + Vec3::Z * view_height;
        // Our own view is the newest cmd's over the next snapshot's
        // `delta_angles` (`PM_UpdateViewAngles`, unlerped); one we follow
        // (the killcam) lerps its own.
        let own = new.ps.get("clientNum") == Some(gs.client_num as u32);
        let mut angles = match self.cmd_angles.filter(|_| own) {
            Some(a) => vec3(|k| {
                let d = angle2short(f(&new.ps, &format!("delta_angles[{k}]")));
                f32::from(a[k].wrapping_add(d) as i16) * (360.0 / 65536.0)
            }),
            None => {
                let view = |ps: &PlayerState| vec3(|k| f(ps, &format!("viewangles[{k}]")));
                lerp_angles(view(old_ps), view(&new.ps), frac)
            }
        };
        // A demo's own view: the recorder's rows lerped at the drawn time,
        // all read ahead; his eye the playerstate's lerp. Rows read with
        // their messages lag the snapshots, so the view and eye stood
        // still between them and stepped at the snapshot rate.
        if own
            && let Source::Demo(d) = &self.source
            && let Some(view) = d.archived_view(time)
        {
            angles = view;
        }
        let weapons = weapon_list(gs);
        let render_time = time;
        // World 1 steps to the drawn time before the parts are placed.
        if let Some(w) = &self.world {
            self.physics.advance(w, render_time);
        }
        let mut entities = Vec::new();
        for e in &new.entities {
            let Some(visual) = visual(gs, new, &weapons, e) else {
                continue;
            };
            let place = |e: &EntityState| {
                (
                    vec3(|k| ent_f32(e, &format!("lerp.pos.trBase[{k}]"))),
                    vec3(|k| ent_f32(e, &format!("lerp.apos.trBase[{k}]"))),
                )
            };
            let (o1, a1) = place(e);
            // One new to the snapshots, or whose teleport bit toggled,
            // starts from its new state (`CG_ResetEntity`).
            let prev = old
                .entities
                .iter()
                .find(|p| p.number() == e.number() && p.etype() == e.etype())
                .filter(|p| {
                    (ent_u32(p, "lerp.eFlags") ^ ent_u32(e, "lerp.eFlags")) & EF_TELEPORT == 0
                });
            // A `physicsLaunch`ed part flies on this client alone, from
            // the first frame its current state reads `TR_PHYSICS`
            // (`CG_CalcEntityPhysicsPositions`, cod4-gsc.md 3.6).
            let flown = match (&visual, &self.world) {
                (Visual::Model(m), Some(w)) if cod4_physics::launched(prev.unwrap_or(e)) => {
                    self.physics.place(prev.unwrap_or(e), m, w, render_time)
                }
                _ => None,
            };
            let (origin, angles) = if let Some(f) = flown {
                f
            } else {
                match prev {
                    Some(p) if interpolates(p, e) => {
                        let (o0, a0) = place(p);
                        (o0.lerp(o1, frac), lerp_angles(a0, a1, frac))
                    }
                    // `BG_EvaluateTrajectory` on the older state at the drawn
                    // time (`CG_CalcEntityLerpPositions`).
                    Some(p) => (
                        trajectory(p, "pos").evaluate(render_time),
                        trajectory(p, "apos").evaluate(render_time),
                    ),
                    None if interpolates(e, e) => (o1, a1),
                    None => (
                        trajectory(e, "pos").evaluate(render_time),
                        trajectory(e, "apos").evaluate(render_time),
                    ),
                }
            };
            let mut body = body_view(gs, new, &weapons, e, origin, angles);
            // `CG_InterpolateEntityPosition`: a player's lean and move
            // direction step between the two snapshots with its angles
            // (cod4-player-anims.md 8).
            if let (Some(b), Some(p)) = (body.as_mut(), prev)
                && e.etype() == ET_PLAYER
                && interpolates(p, e)
            {
                let lean = ent_f32(p, "lerp.u.player.leanf");
                b.frame.lean = lerp_wrapped(lean, b.frame.lean, frac);
                let dir = movement_dir(p);
                b.frame.movement_dir = lerp_wrapped(dir, b.frame.movement_dir, frac);
            }
            let vehicle = (e.etype() == ET_HELICOPTER).then(|| {
                let a = |s: &EntityState, n: &str| ent_f32(s, &format!("lerp.u.vehicle.{n}"));
                let lerp = |n: &str| {
                    let to = a(e, n);
                    prev.map_or(to, |p| lerp_angles(Vec3::X * a(p, n), Vec3::X * to, frac).x)
                };
                VehicleView {
                    body_pitch: lerp("bodyPitch"),
                    body_roll: lerp("bodyRoll"),
                    gun_pitch: lerp("gunPitch"),
                    gun_yaw: lerp("gunYaw"),
                    stage: ent_u32(e, "un1.helicopterStage"),
                }
            });
            // `turret_controller`'s angles as `CG_mg42` lerps them.
            let turret = (e.etype() == ET_MG42).then(|| {
                let gun = |s: &EntityState| {
                    Vec3::from_array(std::array::from_fn(|k| {
                        ent_f32(s, &format!("lerp.u.anonymous.data[{k}]"))
                    }))
                };
                prev.map_or(gun(e), |p| lerp_angles(gun(p), gun(e), frac))
                    .to_array()
            });
            entities.push(Drawn {
                number: e.number(),
                etype: e.etype(),
                visual,
                origin,
                angles,
                body,
                vehicle,
                turret,
                firing: ent_u32(e, "lerp.eFlags") & EF_FIRING != 0,
                part_bits: std::array::from_fn(|k| ent_u32(e, &format!("partBits[{k}]"))),
            });
        }
        // `CG_PredictPlayerState`: our own view, not one we follow
        // (`otherFlags` 2) nor a linked body, is the snapshot's playerstate
        // with our cmds replayed on it (cod4-client-view.md 5).
        if let Some(w) = &self.world {
            let list = weapons.join(" ");
            if self.level_defs.as_ref().is_none_or(|(l, _)| *l != list) {
                self.level_defs = Some((list, w.level_defs(&weapons)));
            }
        }
        // A part whose entity left the snapshot is gone with it.
        self.physics
            .retain(|n| new.entities.iter().any(|e| e.number() == n));
        // `cg.demoPlayback` predicts nothing (cod4-demos.md 5).
        let demo = matches!(self.source, Source::Demo(_));
        let predicts = predicts(self.world.is_some() && !demo, gs, &new.ps);
        // The snapshot's solid entities but our own, which the prediction
        // and the client-side rounds trace (cod4-client-view.md 5).
        let solids: Vec<Solid> = entities
            .iter()
            .filter(|d| d.etype != ET_ITEM && d.number != gs.client_num as u32)
            .filter_map(|d| {
                let e = new.entities.iter().find(|e| e.number() == d.number)?;
                let solid = ent_u32(e, "solid");
                // A script mover with no `solid` meets rounds by its
                // model's collision (`CG_LocationalTraceDObj`).
                if d.etype == ET_SCRIPTMOVER && solid == 0 {
                    let name = model_cs(gs, ent_u32(e, "index"))?.to_ascii_lowercase();
                    let surfs = self.world.as_ref()?.model_coll.get(&name)?.clone();
                    return Some(Solid {
                        number: d.number,
                        origin: d.origin,
                        angles: d.angles,
                        kind: cod4_predict::SolidKind::Model {
                            surfs,
                            hidden: std::array::from_fn(|k| ent_u32(e, &format!("partBits[{k}]"))),
                        },
                    });
                }
                Solid::of(
                    d.number,
                    d.etype == ET_PLAYER,
                    solid,
                    ent_u32(e, "lerp.eFlags"),
                    ent_u32(e, "index"),
                    d.origin,
                    d.angles,
                )
            })
            .collect();
        let predicted = match &self.world {
            Some(w) if predicts => {
                let now = {
                    let t = (self.now_ms + self.cmd_delta.unwrap_or(0.0)).floor() as i32;
                    self.cmd_last = self.cmd_last.max(t);
                    self.cmd_last
                };
                // The cmds built by now, and the frame's own, stamped now:
                // live, the last sent's keys and view; a capture's, those
                // its client sent for this moment (the first cmd at or past
                // it), where a frame-rate client would have built one, its
                // view eased from the cmd before (replay-only smoothing,
                // cod4-client-view.md 6).
                let (cmds, next): (Vec<Cmd>, Option<Cmd>) = match &self.source {
                    Source::Capture { cmds, .. } => {
                        let k = cmds.partition_point(|c| c.server_time <= now);
                        let from = k.saturating_sub(cod4_predict::CMD_BACKUP);
                        let next = cmds.get(k).map(|n| match k.checked_sub(1) {
                            Some(i) => eased_cmd(&cmds[i], n, now),
                            None => *n,
                        });
                        (cmds[from..k].to_vec(), next)
                    }
                    Source::Live { .. } | Source::Demo(_) => {
                        (self.sent_cmds.iter().copied().collect(), None)
                    }
                };
                let frame = next.or(cmds.last().copied()).map(|c| Cmd {
                    server_time: now,
                    ..c
                });
                let world = cod4_predict::Solids {
                    clip: &w.clip,
                    solids: &solids,
                };
                let rules = vcod_cod4::pmove::Rules {
                    // The server's slowed walk under a concussion
                    // (cod4-combat.md 10.8).
                    shock_slows: w.shock_slows(|i| gs.configstring(i)),
                    mantle_anims: w.mantle,
                    ..cod4_predict::rules_from_mirror(|i| gs.configstring(i))
                };
                let weapon = |n: u32| {
                    weapon_name(&weapons, n)
                        .and_then(|name| w.weapon_move(name))
                        .unwrap_or(WeaponMove::NONE)
                };
                let cut = (
                    ps_u(&new.ps, "eFlags") & EF_TELEPORT,
                    ps_u(&new.ps, "clientNum"),
                    new.ps.stats[4],
                );
                self.predictor.note_damage(&new.ps, new.server_time);
                Some(self.predictor.predict(
                    &new.ps,
                    cut,
                    &cmds,
                    frame.as_ref(),
                    &world,
                    rules,
                    &weapon,
                    self.level_defs.as_ref().map(|(_, d)| d as &dyn WeaponDefs),
                    self.now_ms,
                ))
            }
            _ => {
                self.predictor.reset();
                None
            }
        };
        self.shot_solids = solids;
        // `CG_TransitionPlayerState` on the predicted state: its ring's new
        // events fire this frame, and the snapshots' copies of them never
        // (`walk_snapshot` leaves a predicted ring alone).
        // `cg.lastFrame.aimSpreadScale`: the last frame's, which an own
        // shot's spread reads (cod4-effects.md 12.2).
        let last_scale = self
            .last_predicted
            .as_ref()
            .map_or(0.0, |p| p.aim_spread_scale);
        if let Some(p) = &predicted {
            self.last_predicted = Some(p.ps.clone());
        }
        if let Some(p) = &predicted {
            let ring = cod4_predict::Ring::of(&p.ps);
            let from = self.fired.len();
            if let Some(old) = &self.ps_ring {
                for k in cod4_predict::new_events(&ring, old) {
                    let id = p.ps.events[k];
                    let Some(name) = event_name(id).filter(|_| id != 0) else {
                        continue;
                    };
                    if name == "EV_NOAMMO" {
                        self.ran_dry = Some(Dry::of(&p.ps));
                    }
                    self.fired.push(Fired {
                        id,
                        name,
                        entity: None,
                        server_time: p.ps.command_time,
                        parm: p.ps.event_parms[k],
                        origin: p.ps.origin,
                        state: None,
                        weapon: p.ps.weapon,
                        perks: client_perks(new, p.ps.client_num),
                        aim: name
                            .starts_with("EV_FIRE_WEAPON")
                            .then(|| self.level_def(p.ps.weapon))
                            .flatten()
                            .map(|d| crate::cod4_fx::Aim {
                                from: p.event_aims[k],
                                ..crate::cod4_fx::Aim::own(&p.ps, d, last_scale, p.event_times[k])
                            }),
                    });
                }
            }
            self.ps_ring = Some(ring);
            let own = self.fired.split_off(from);
            force_stance(&mut self.input, &own);
            self.fired.extend(own);
        }
        let u = |name: &str| ps_u(&new.ps, name);
        // `CG_KickAngles` on this frame, then the own shots' recoil and
        // landings fired since the last (cod4-client-view.md 14).
        {
            let ps = predicted.as_ref().filter(|p| p.armed).map(|p| &p.ps);
            let frac = ps.map_or(f(&new.ps, "fWeaponPosFrac"), |p| p.weapon_pos_frac);
            let restrict = ps.map_or(u("weaponRestrictKickTime") as i32, |p| {
                p.weapon_restrict_kick_time
            });
            let weapon = if ps.map_or(u("weapFlags"), |p| p.weap_flags) & 2 != 0 {
                u("offHandIndex")
            } else {
                ps.map_or(u("weapon"), |p| p.weapon)
            };
            let def =
                weapon_name(&weapons, weapon).and_then(|n| self.world.as_ref()?.kick(n).cloned());
            let other = u("otherFlags");
            self.kick.check_respawn(u("clientNum"), new.ps.stats[4]);
            self.kick.step(render_time, def.as_ref(), frac, other);
            // Read only on a frame with a new event to land.
            let heights = if self.fired.len() > self.kick_seen {
                let r = cod4_predict::rules_from_mirror(|i| gs.configstring(i));
                (r.fall_damage_min_height, r.fall_damage_max_height)
            } else {
                Default::default()
            };
            for fd in self.fired.iter().skip(self.kick_seen) {
                if fd.entity.is_some() {
                    continue;
                }
                if let Some(d) = def.as_ref().filter(|_| cod4_kick::is_fire(fd.name)) {
                    self.kick.fire(d, other, frac, restrict);
                }
                self.kick.land(fd.name, fd.parm, heights, render_time);
            }
            self.kick_seen = self.fired.len();
        }
        let mut velocity = Vec3::new(lerp1("velocity[0]"), lerp1("velocity[1]"), 0.0);
        let mut feet = vec3(|k| lerp1(&format!("origin[{k}]")));
        if let Some(p) = &predicted {
            let ps = &p.ps;
            feet = p.origin;
            view_height = ps.view_height_current;
            eye = p.origin + Vec3::Z * view_height;
            angles = Vec3::from_array(ps.view_angles);
            velocity = ps.velocity;
        }
        // `CG_OffsetFirstPersonView`'s landing dip, then its lean
        // (`AddLeanToPosition` with 16
        // and 20, nothing rolls the view), then the floor 8 over the feet
        // (cod4-client-view.md 7).
        let land = self.kick.landing(render_time);
        // In third person `CG_CalcViewValues` skips the whole first-person
        // offset (0x451c69..0x451c77): the eye is the origin raised by
        // the view height alone.
        let third = cod4_third_person::rendering(self.third_person, u("pm_type"));
        // Not on a gun (`eFlags` 0x300, 0x450063).
        if !third && u("eFlags") & 0x300 == 0 {
            eye.z += cod4_kick::eye_dip(land);
        }
        let leanf = predicted.as_ref().map_or(lerp1("leanf"), |p| p.ps.leanf);
        let leaned = leanf != 0.0 && !third;
        if leaned {
            eye = vcod_common::pmove::aim::add_lean(
                eye,
                vcod_common::x87::radians(angles.y),
                leanf,
                vcod_common::patch::Arith::Msvc,
            );
        }
        let light_eye = eye;
        if leaned {
            eye.z = eye.z.max(feet.z + 8.0);
        }
        let pick = |pred: fn(&pm::PlayerState) -> u32, name: &str| {
            predicted.as_ref().map_or(u(name), |p| pred(&p.ps))
        };
        // The weapon half's fields, where it ran.
        let armed = predicted.as_ref().filter(|p| p.armed);
        let pick_armed =
            |pred: fn(&pm::PlayerState) -> u32, name: &str| armed.map_or(u(name), |p| pred(&p.ps));
        let view = crate::cod4_viewmodel::ViewPs {
            client_num: u("clientNum"),
            pm_type: u("pm_type"),
            pm_flags: pick(|p| p.pm_flags, "pm_flags"),
            eflags: pick(|p| p.e_flags, "eFlags"),
            weap_flags: pick_armed(|p| p.weap_flags, "weapFlags"),
            weapon: pick_armed(|p| p.weapon, "weapon"),
            off_hand: u("offHandIndex"),
            weap_anim: pick_armed(|p| p.weap_anim, "weapAnim"),
            weaponstate: pick_armed(|p| p.weapon_state as u32, "weaponstate"),
            weapon_time: pick_armed(|p| p.weapon_time as u32, "weaponTime") as i32,
            pos_frac: armed.map_or(lerp1("fWeaponPosFrac"), |p| p.ps.weapon_pos_frac),
            bob_cycle: pick(|p| p.bob_cycle as u32, "bobCycle") & 0xff,
            xyspeed: predicted
                .as_ref()
                .map_or(velocity.length(), |p| cod4_predict::xyspeed(&p.ps)),
            speed: u("speed") as i32 as f32,
            view_height_target: pick(|p| p.view_height_target as u32, "viewHeightTarget") as i32,
            view_height_current: predicted
                .as_ref()
                .map_or(f(&new.ps, "viewHeightCurrent"), |p| {
                    p.ps.view_height_current
                }),
            leanf: predicted.as_ref().map_or(lerp1("leanf"), |p| p.ps.leanf),
            viewmodel_index: u("viewmodelIndex"),
            angles,
            ammoclip: armed.map_or(new.ps.ammoclip, |p| p.ps.ammoclip),
            weapon_models: new.ps.weapon_models,
            aim_spread_scale: armed.map_or(f(&new.ps, "aimSpreadScale"), |p| p.ps.aim_spread_scale),
            perks: u("perks"),
            grenade_time_left: pick_armed(|p| p.grenade_time_left as u32, "grenadeTimeLeft") as i32,
            spread_override: u("spreadOverride") as i32,
            spread_override_state: u("spreadOverrideState") as i32,
            origin: feet,
            other_flags: u("otherFlags"),
            gun_kick: self.kick.gun_kick(),
            land,
            damage: None,
            viewlocked: u("viewlocked"),
            viewlocked_ent: u("viewlocked_entNum"),
            gun_dir: None,
            name_seen: 0,
        };
        let hands = model_cs(gs, view.viewmodel_index);
        let third_person = third.then(|| {
            let dead_yaw = (u("pm_type") >= 7).then_some(new.ps.stats[1] as f32);
            let (origin, angles) = self.third_person_camera(eye, angles, dead_yaw, u("clientNum"));
            ThirdPerson {
                origin,
                angles,
                dead_yaw,
                view_height,
            }
        });
        // `CG_AddPacketEntities` adds `cg.predictedPlayerEntity`, the
        // drawn playerstate's own body, under `otherFlags & 6`
        // (0x452af0..0x452b6d); `CG_Player` (0x445370) draws it only in
        // third person, and never with `eFlags` 0x20 or 0x20000, a dead
        // one (0x44537e..0x4453b7): the corpse is the snapshot's.
        if third && u("otherFlags") & 6 != 0 {
            let ps = predicted
                .as_ref()
                .map_or_else(|| new.ps.clone(), |p| p.ps.to_wire(&new.ps));
            let e = vcod_cod4::pmove::wire::player_entity(u("clientNum") as usize, &ps);
            if ent_u32(&e, "lerp.eFlags") & 0x2_0020 == 0
                && let Some(visual) = visual(gs, new, &weapons, &e)
            {
                entities.push(Drawn {
                    number: e.number(),
                    etype: ET_PLAYER,
                    visual,
                    origin: feet,
                    angles,
                    body: body_view(gs, new, &weapons, &e, feet, angles),
                    vehicle: None,
                    turret: None,
                    firing: false,
                    part_bits: [0; 4],
                });
            }
        }
        let crosshair_hit = self.world.as_ref().and_then(|w| {
            let team = |n: u32| {
                new.clients
                    .iter()
                    .find(|c| c.0[0] == n)
                    .map_or(0, client_team)
            };
            let world = cod4_predict::Solids {
                clip: &w.clip,
                solids: &self.shot_solids,
            };
            crosshair_scan(&world, eye, angles, view.client_num, team, |n| {
                self.shot_solids
                    .iter()
                    .find(|s| s.number == n)
                    .map(|s| s.origin)
            })
        });
        // World 0 steps to the drawn time (`DynEntCl_ProcessEntities`).
        let mut dynents = Vec::new();
        if let (Some(d), Some(w)) = (&mut self.dynents, &self.world) {
            let mut trace = vcod_cod4::physics::clip_trace_masked(&w.clip);
            let mut solids = vcod_cod4::physics::clip_solids_masked(&w.clip);
            d.advance_in(render_time, &mut trace, &mut solids);
            dynents.extend(
                d.ents
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.drawn)
                    .map(|(i, e)| (i as u32, e.origin, e.rot)),
            );
        }
        Some(Scene {
            crosshair_hit,
            eye,
            light_eye,
            angles,
            entities,
            dynents,
            server_time: new.server_time,
            render_time,
            following: !own,
            view_reset: reset,
            view,
            hands,
            dof: crate::cod4_dof::PsDof {
                scripted: [
                    "dofNearStart",
                    "dofNearEnd",
                    "dofFarStart",
                    "dofFarEnd",
                    "dofNearBlur",
                    "dofFarBlur",
                ]
                .map(|n| f(&new.ps, n)),
                viewmodel: ["dofViewmodelStart", "dofViewmodelEnd"].map(|n| f(&new.ps, n)),
            },
            third_person,
        })
    }

    /// `CG_OffsetThirdPersonView`'s camera off `vieworg` (raised by the
    /// view height) and the view's angles, its traces `CG_TraceCapsule`'s
    /// through the clip map and the snapshot's solids but `pass`
    /// (0x44fa00); unclipped before the level's clip map is in.
    pub fn third_person_camera(
        &self,
        vieworg: Vec3,
        angles: Vec3,
        dead_yaw: Option<f32>,
        pass: u32,
    ) -> (Vec3, Vec3) {
        use vcod_cod4::pmove::PmWorld;
        let world = self.world.as_ref().map(|w| cod4_predict::Solids {
            clip: &w.clip,
            solids: &self.shot_solids,
        });
        let (mins, maxs) = cod4_third_person::BOX;
        let trace = |a: Vec3, b: Vec3| {
            world.as_ref().map_or(1.0, |w| {
                w.trace(a, b, mins, maxs, pass, cod4_third_person::MASK)
                    .fraction
            })
        };
        cod4_third_person::camera(vieworg, angles, dead_yaw, self.third_person, &trace)
    }
}

/// A player under the crosshair: his client number, whether he is a
/// teammate (`None` while we spectate, team 3), and how far he stands
/// from the eye, squared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrosshairHit {
    pub client: u32,
    pub friendly: Option<bool>,
    pub dist_sq: f32,
}

/// `CG_ScanForCrosshairEntity`'s trace (`iw3mp.exe` 0x42dbf0,
/// cod4-hud.md 17): a point from the eye 8192 (0x70baf8) along the view
/// under 0x2803001, passing our own body, that must end on a client
/// (an entity under 64). A teammate shares our team, team 0 sharing
/// none.
pub fn crosshair_scan(
    world: &impl vcod_cod4::pmove::PmWorld,
    eye: Vec3,
    angles: Vec3,
    own: u32,
    team: impl Fn(u32) -> i32,
    origin: impl Fn(u32) -> Option<Vec3>,
) -> Option<CrosshairHit> {
    let (p, y) = (angles.x.to_radians(), angles.y.to_radians());
    let forward = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
    let t = world.trace(
        eye,
        eye + forward * 8192.0,
        Vec3::ZERO,
        Vec3::ZERO,
        own,
        0x280_3001,
    );
    if t.entity_num >= 64 {
        return None;
    }
    let mine = team(own);
    let friendly = match mine {
        3 => None,
        _ => Some(mine != 0 && team(t.entity_num) == mine),
    };
    Some(CrosshairHit {
        client: t.entity_num,
        friendly,
        dist_sq: origin(t.entity_num).map_or(f32::MAX, |o| (o - eye).length_squared()),
    })
}

/// `CG_PredictPlayerState` predicts `ps` (cod4-client-view.md 5): our own
/// view, not one we follow (`otherFlags` 2) nor a linked body, once a
/// prediction world was handed in.
pub(super) fn predicts(world: bool, gs: &Gamestate, ps: &PlayerState) -> bool {
    let u = |f: &str| ps.get(f).unwrap_or(0);
    let linked = matches!(
        u("pm_type") as i32,
        pm::pm_type::NORMAL_LINKED | pm::pm_type::DEAD_LINKED
    );
    world && u("clientNum") == gs.client_num as u32 && u("otherFlags") & 2 == 0 && !linked
}

/// The longest gap between a capture's cmds [`eased_cmd`] eases across,
/// ms, and the fastest turn, degrees a ms: a faster one is a snap (a
/// scripted view, or the client making up a `delta_angles` change),
/// which the view takes at once.
const EASE_MAX_MS: i32 = 250;
const EASE_MAX_TURN: f32 = 0.36;

/// `next` with its view eased from `prev`'s by how far `now` is between
/// their times, each angle the short way round in `SHORT` units: a
/// capture's client sent a cmd every 16 to 100 ms, and its view would
/// otherwise turn in those steps. Replay-only smoothing.
pub(super) fn eased_cmd(prev: &Cmd, next: &Cmd, now: i32) -> Cmd {
    let span = next.server_time - prev.server_time;
    let d: [i16; 3] = std::array::from_fn(|k| next.angles[k].wrapping_sub(prev.angles[k]) as i16);
    let fastest = d.iter().map(|d| f32::from(*d).abs()).fold(0.0, f32::max) * (360.0 / 65536.0);
    if span <= 0 || span > EASE_MAX_MS || fastest > EASE_MAX_TURN * span as f32 {
        return *next;
    }
    let f = ((now - prev.server_time) as f32 / span as f32).clamp(0.0, 1.0);
    let angles = std::array::from_fn(|k| {
        prev.angles[k].wrapping_add((f32::from(d[k]) * f).round() as i16 as u16)
    });
    Cmd { angles, ..*next }
}

/// Weapon `n`'s name.
pub fn weapon_name(weapons: &[String], n: u32) -> Option<&str> {
    weapons
        .get((n as usize).checked_sub(1)?)
        .map(String::as_str)
}

pub(super) fn model_cs(gs: &Gamestate, index: u32) -> Option<String> {
    let i = u16::try_from(index)
        .ok()
        .filter(|&i| i > 0 && i < MAX_MODELS)?;
    let s = String::from_utf8_lossy(gs.configstring(CS_MODELS + i)).into_owned();
    (!s.is_empty()).then_some(s)
}

/// A client's body model: its `clientState.modelindex`.
fn body_model(gs: &Gamestate, snap: &Snapshot, client: u32) -> Option<String> {
    let c = snap.clients.iter().find(|c| c.0[0] == client)?;
    model_cs(gs, client_u32(c, "modelindex"))
}

/// What `e` draws as; `None` for what is never drawn (event entities, an
/// entity with no model, a missile's or item's that has none).
pub(super) fn visual(
    gs: &Gamestate,
    snap: &Snapshot,
    weapons: &[String],
    e: &EntityState,
) -> Option<Visual> {
    let index = ent_u32(e, "index");
    let weapon = |n: u32| weapon_name(weapons, n).map(str::to_string);
    Some(match e.etype() {
        ET_PLAYER | ET_PLAYER_CORPSE => {
            let client = ent_u32(e, "clientNum");
            body_model(gs, snap, client).map_or(Visual::Box, Visual::Model)
        }
        ET_SCRIPTMOVER if ent_u32(e, "solid") == SOLID_BMODEL => {
            (index > 0).then_some(Visual::Submodel(index as usize))?
        }
        // `CG_ScriptMover` draws a plane its `index` model
        // (`CG_ProcessEntity`).
        ET_SCRIPTMOVER | ET_GENERAL | ET_PLANE | ET_MG42 => Visual::Model(model_cs(gs, index)?),
        // A helicopter's model rides `un2`, its `index` 0
        // (`play-1.7-hardpoint.txt`, cod4-gsc.md 3.2); a vcod-server from
        // before that capture sent it in `index`.
        ET_HELICOPTER => {
            let model = match ent_u32(e, "un2") {
                0 => index,
                m => m,
            };
            Visual::Model(model_cs(gs, model)?)
        }
        // An item's `index` is its weapon plus 128 times its camo (the kill
        // capture's dropped M16, cod4-netfields.md 10); the client draws
        // `worldModel[index >> 7]` (`iw3mp.exe` 0x4328bf..0x4328e7,
        // cod4-client-view.md 4.7), keyed `weapon:<name>#<slot>` past slot 0.
        ET_ITEM => {
            let name = weapon(index & 0x7f)?;
            Visual::Model(match index >> 7 {
                0 => format!("weapon:{name}"),
                slot => format!("weapon:{name}#{slot}"),
            })
        }
        ET_MISSILE => Visual::Model(format!("projectile:{}", weapon(ent_u32(e, "weapon"))?)),
        _ => return None,
    })
}

/// Whether an entity lerps between its two snapshots: its older state is
/// `TR_INTERPOLATE` (the newer not `TR_PHYSICS`), or it is a client on
/// `TR_LINEAR_STOP`; anything else runs its trajectory
/// (cod4-player-anims.md 6; INFERRED, not yet confirmed on 1.7).
fn interpolates(old: &EntityState, new: &EntityState) -> bool {
    let t = ent_u32(old, "lerp.pos.trType");
    (t == TR_INTERPOLATE && ent_u32(new, "lerp.pos.trType") != TR_PHYSICS)
        || (t == TR_LINEAR_STOP && old.number() < 64)
}

/// CoD4's `trType`s (`trType_t`): CoD2's up to `TR_GRAVITY`, then
/// `TR_ACCELERATE` 6, `TR_DECELERATE` 7, `TR_PHYSICS` 8 and the ragdoll
/// types 9 to 11.
const TR_INTERPOLATE: u32 = 1;
const TR_LINEAR_STOP: u32 = 3;
const TR_PHYSICS: u32 = 8;

/// `lerp.<prefix>` as a CoD2 trajectory, CoD4's type renumbered onto
/// CoD2's (`BG_EvaluateTrajectory` treats `TR_PHYSICS` and
/// `TR_RAGDOLL_INTERPOLATE` as stationary, `TR_FIRST_RAGDOLL` as linear and
/// `TR_RAGDOLL_GRAVITY` as gravity).
pub(super) fn trajectory(e: &EntityState, prefix: &str) -> Trajectory2 {
    let f = |name: &str| ent_u32(e, &format!("lerp.{prefix}.{name}"));
    let v = |name: &str| vec3(|k| ent_f32(e, &format!("lerp.{prefix}.{name}[{k}]")));
    let tr_type = match f("trType") {
        t @ 0..=5 => t as i32,
        6 => tr2::TR_ACCELERATE,
        7 => tr2::TR_DECELERATE,
        9 => tr2::TR_LINEAR,
        10 => tr2::TR_GRAVITY,
        _ => tr2::TR_STATIONARY,
    };
    Trajectory2 {
        tr_type,
        tr_time: f("trTime") as i32,
        tr_duration: f("trDuration") as i32,
        base: v("trBase"),
        delta: v("trDelta"),
    }
}

/// CoD4's `eFlags` in CoD2's numbering: the same bits but the mantle,
/// 0x8000 on CoD4 and 0x4000 on CoD2 (cod4-pmove.md 17.1,
/// cod4-player-anims.md 3; INFERRED, not yet confirmed on 1.7).
pub fn cod2_flags(e: u32) -> i32 {
    const MANTLE: u32 = 0x8000;
    let kept = (e & !(MANTLE | 0x4000)) as i32;
    if e & MANTLE != 0 {
        kept | cod2_eflags::MANTLE
    } else {
        kept
    }
}

/// A player's or corpse's [`BodyView`]: the client state's body and its
/// six attachments on their tags (an empty tag grafts by the
/// attachment's root bone: cod4-player-anims.md 5, INFERRED, not yet
/// confirmed on 1.7), the entity's anims, flags and angles.
fn body_view(
    gs: &Gamestate,
    snap: &Snapshot,
    weapons: &[String],
    e: &EntityState,
    origin: Vec3,
    angles: Vec3,
) -> Option<BodyView> {
    if !matches!(e.etype(), ET_PLAYER | ET_PLAYER_CORPSE) {
        return None;
    }
    let client = ent_u32(e, "clientNum");
    let c = snap.clients.iter().find(|c| c.0[0] == client)?;
    let body = model_cs(gs, client_u32(c, "modelindex"))?;
    let attachments = (0..6)
        .filter_map(|i| {
            let model = model_cs(gs, client_u32(c, &format!("attachModelIndex[{i}]")))?;
            let tag = client_u32(c, &format!("attachTagIndex[{i}]"));
            let tag = u16::try_from(tag)
                .ok()
                .map(|t| String::from_utf8_lossy(gs.configstring(CS_TAGS + t)).into_owned())
                .filter(|t| !t.is_empty());
            Some((model, tag))
        })
        .collect();
    let frame = BodyFrame {
        origin,
        angles: angles.to_array(),
        legs_anim: ent_u32(e, "legsAnim") as i32,
        torso_anim: ent_u32(e, "torsoAnim") as i32,
        eflags: cod2_flags(ent_u32(e, "lerp.eFlags")),
        movement_dir: movement_dir(e),
        lean: ent_f32(e, "lerp.u.player.leanf"),
        torso_height: 0.0,
        torso_pitch: ent_f32(e, "fTorsoPitch"),
        waist_pitch: ent_f32(e, "fWaistPitch"),
        client_num: client as i32,
    };
    Some(BodyView {
        frame,
        character: Character {
            body,
            attachments,
            ignore_collision: 0,
        },
        // `BG_UpdatePlayerDObj` builds a gunner's body without his own
        // weapon (cod4-player-anims.md 5).
        weapon: weapon_name(weapons, ent_u32(e, "weapon"))
            .filter(|_| ent_u32(e, "lerp.eFlags") & 0x300 == 0)
            .map(str::to_string),
        weapon_model: ent_u32(e, "weaponModel") as usize,
        corpse: e.etype() == ET_PLAYER_CORPSE,
        ragdoll: (9..=11).contains(&ent_u32(e, "lerp.pos.trType")),
        gun: Some(ent_u32(e, "otherEntityNum"))
            .filter(|&g| ent_u32(e, "lerp.eFlags") & 0x300 != 0 && (64..1023).contains(&g))
            .map(|g| {
                let gun = snap.entities.iter().find(|x| x.number() == g);
                let weapon = gun.and_then(|x| weapon_name(weapons, ent_u32(x, "weapon")));
                (g, weapon.map(str::to_string))
            }),
    })
}

/// A player's move direction: a signed byte of degrees
/// (`BG_PlayerStateToEntityState`).
pub(super) fn movement_dir(e: &EntityState) -> f32 {
    f32::from(ent_u32(e, "lerp.u.player.movementDir") as u8 as i8)
}

/// `a` to `b` by `t` the way the cgame steps an angle: the difference
/// wrapped into -180..180 through `floor`.
pub(super) fn lerp_wrapped(a: f32, b: f32, t: f32) -> f32 {
    let d = (b - a) * (1.0 / 360.0);
    (d - (d + 0.5).floor()) * 360.0 * t + a
}

/// Angles stepped the short way round.
pub(super) fn lerp_angles(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    let d = |x: f32, y: f32| {
        let mut d = (y - x) % 360.0;
        if d > 180.0 {
            d -= 360.0;
        } else if d < -180.0 {
            d += 360.0;
        }
        x + d * t
    };
    Vec3::new(d(a.x, b.x), d(a.y, b.y), d(a.z, b.z))
}
