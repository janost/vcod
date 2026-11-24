//! `Phase::Live`'s frame: the snapshots posed, predicted, viewed, heard
//! and drawn.

use super::*;
use crate::redraw::Tick;

impl App {
    /// `Phase::Live`'s frame between two gamestates: the snapshots drawn.
    pub(crate) fn live_frame(
        &mut self,
        tick: &Tick,
        prof: &mut frame_prof::Span,
        vm: &mut Option<renderer::VmDraw>,
        hud_quads: &mut Vec<hud::HudQuad>,
    ) -> renderer::Frame {
        let Tick {
            time, cull, aspect, ..
        } = *tick;
        let Some(r) = &mut self.renderer else {
            unreachable!()
        };
        let Mode::Online {
            net,
            cam,
            input,
            ring,
            predictor,
            view,
            phase,
            join,
            ..
        } = &mut self.mode
        else {
            unreachable!()
        };
        let Phase::Live(live) = phase else {
            unreachable!()
        };
        let LivePhase {
            world,
            weapons,
            scene,
            events,
            predicted_events,
            clock,
            last_loop_snap,
            looped_fx,
            vehicle_trails,
            drawn_pos,
            drawn_vehicle_angles,
            seat_turn,
            gunner_sway,
            turret_sway,
            seat_frac,
        } = &mut **live;
        let viewer = Viewer::of(net);
        let Viewer { p, .. } = viewer;
        let mut cx = LiveCx {
            fs: &self.fs,
            shell: &self.shell,
            session: &self.session,
            audio: &mut self.audio,
            fx: &mut self.fx,
            flame: &mut self.flame,
            client_bullets: &mut self.client_bullets,
            hud: &mut self.hud,
            foreign_events: &mut self.foreign_events,
            interp_misses: &mut self.interp_misses,
            build_ms: &mut self.build_ms,
            viewmodel: &mut self.viewmodel,
            look_zoom: &mut self.look_zoom,
            ev_seen: &mut self.ev_seen,
            ev_unknown: &mut self.ev_unknown,
            fx_ms: &mut self.fx_ms,
            quick_chat: &self.quick_chat,
        };
        let Posed {
            mut instances,
            mut muzzles,
            mut muzzle_poses,
            mut brass,
            shot_fallbacks,
            weapon_flash,
            entity_pos,
            entity_angles,
            vehicle_angles,
            vehicle_wheels,
            heads,
            turret_eye,
            shadow_marks,
            mut view_lean,
            mut feet_z,
            render_time,
            intermission_roll,
        } = cx.pose(
            prof, r, net, cam, input, world, scene, clock, drawn_pos, seat_frac, viewer,
        );

        let (seat_now, predicted, view_ps, evs) = cx.predict(
            tick,
            viewer,
            prof,
            r,
            net,
            cam,
            input,
            ring,
            predictor,
            view,
            join,
            world,
            weapons,
            events,
            predicted_events,
            drawn_pos,
            drawn_vehicle_angles,
            render_time,
            &entity_pos,
            &vehicle_angles,
            &mut feet_z,
            &mut view_lean,
        );

        let (view_roll, shock) = cx.view(
            tick,
            viewer,
            prof,
            r,
            net,
            cam,
            input,
            view,
            join,
            world,
            weapons,
            scene,
            seat_turn,
            gunner_sway,
            turret_sway,
            seat_frac,
            &entity_pos,
            &vehicle_angles,
            turret_eye,
            view_lean,
            feet_z,
            render_time,
            intermission_roll,
            seat_now,
            &predicted,
            &view_ps,
        );

        let (local_player, fov, cam_forward, cam_right, cam_up, fx_t0) = cx.gun(
            tick,
            viewer,
            prof,
            vm,
            net,
            cam,
            view,
            world,
            weapons,
            scene,
            looped_fx,
            vehicle_trails,
            &mut muzzles,
            &mut muzzle_poses,
            &mut brass,
            &vehicle_wheels,
            render_time,
            &predicted,
            &view_ps,
            view_roll,
            shock,
        );

        prof.next(frame_prof::Stage::Events);
        let (screen_w, screen_h) = r.screen_size();

        let no_clients = BTreeMap::new();
        let no_entities = BTreeMap::new();
        let newest = net.snapshots().newest();
        let entity_origin = |num: i32| {
            let num = u32::try_from(num).ok()?;
            entity_pos.get(&num).map(|v| v.to_array())
        };
        // Retail reads `cg_entities[n].currentState`;
        // the newest snapshot stands in.
        let turret_weapon = newest.and_then(|s| {
            let int = |n: &str| s.ps.field_i32(p, n);
            let num = turret::ridden(
                int("eFlags"),
                int("viewlocked"),
                int("viewlocked_entNum"),
                p.is_uo(),
            )?;
            let w = s.entities.get(&num)?.field_i32(p, "weapon");
            usize::try_from(w).ok()
        });
        // UO's barrel icon turns with the ridden
        // vehicle's `tag_turret` (uo cgame 0x30031e20).
        let vehicle_turret_yaw = newest
            .filter(|s| s.ps.field_i32(p, "eFlags") & net::flags::EF_IN_VEHICLE_V22 != 0)
            .and_then(|s| {
                let num = u32::try_from(s.ps.field_i32(p, "viewlocked_entNum")).ok()?;
                let f = scene.tag_pose(&self.fs, num, "tag_turret")?.axis[0];
                Some(f.y.atan2(f.x).to_degrees())
            });
        let client_cvar = |c: &str| join.cvars.get(c, net.configstrings(), &self.shell);
        let server_address = net.transport().peer().map(|a| a.to_string());
        let hud_frame = hud::HudFrame {
            now: time,
            screen_w,
            screen_h,
            configstrings: net.configstrings(),
            clients: newest.map_or(&no_clients, |s| &s.clients),
            protocol: p,
            // The render clock, so hudelem tweens and
            // timers move between snapshots.
            server_time: render_time.unwrap_or(0),
            snap_time: render_time
                .and_then(|t| net.snapshots().two_for_time(t))
                .map_or(0, |(a, _)| a.server_time.raw()),
            ps: newest.map(|s| &s.ps),
            entities: newest.map_or(&no_entities, |s| &s.entities),
            predicted: predicted.as_ref().filter(|_| local_player).map(|v| &v.pred),
            local_player,
            weapons,
            localized: &self.localized,
            view_yaw: cam.yaw.to_degrees(),
            eye: cam.pos.to_array(),
            fov,
            entity_origin: &entity_origin,
            turret_weapon,
            vehicle_turret_yaw,
            cvar: &client_cvar,
            bound_key: &|cmd| bound_key(&self.shell, &self.localized, p, cmd),
            draw: hud::DrawToggles {
                crosshair: self.shell.cvar_f32("cg_drawCrosshair") as i32 != 0,
                status: self.shell.cvar_f32("cg_drawStatus") as i32 != 0,
            },
            weapon_select: input.selected(),
            own_client: net
                .gamestate()
                .and_then(|g| u32::try_from(g.client_num).ok()),
            server_address: server_address.as_deref(),
        };

        cx.events(
            tick,
            viewer,
            vm,
            net,
            cam,
            input,
            world,
            weapons,
            scene,
            &muzzles,
            &brass,
            &shot_fallbacks,
            &weapon_flash,
            &entity_pos,
            &predicted,
            evs,
            local_player,
            newest,
            &hud_frame,
        );

        cx.sound(
            tick,
            viewer,
            prof,
            r,
            net,
            view,
            world,
            weapons,
            scene,
            last_loop_snap,
            &mut instances,
            &muzzles,
            &entity_pos,
            &entity_angles,
            render_time,
            newest,
        );

        let fdc = cx.flame(
            tick,
            viewer,
            r,
            net,
            cam,
            input,
            world,
            weapons,
            &muzzles,
            &entity_pos,
            &heads,
            &shadow_marks,
            render_time,
            cam_right,
            cam_up,
            fx_t0,
            newest,
        );

        prof.next(frame_prof::Stage::Hud);
        if let Some(hud) = &mut self.hud {
            let hud_t0 = Instant::now();
            hud.set_message_widths(
                self.shell.cvar_f32("cg_gameMessageWidth") as i32,
                self.shell.cvar_f32("cg_gameBoldMessageWidth") as i32,
                screen_w as u32,
            );
            hud.set_chat_rules(hud::chat::Rules::read(|c| self.shell.cvar(c), p.is_uo()));
            *hud_quads = hud.build(&hud_frame);
            self.hud_ms = hud_t0.elapsed().as_secs_f32() * 1000.0;
            // The vote draw's `player_talk` on a
            // changed time or count.
            if std::mem::take(&mut hud.vote_sound) {
                self.audio.play_local(&self.fs, "player_talk");
            }
        }
        // After the HUD, which borrows the snapshot ring.
        if let Some(n) = fdc {
            net.send_reliable(&format!("fdc {n}"));
        }

        renderer::Frame {
            view_proj: camera::view_proj_from(cam.pos, cam.yaw, cam.pitch, view_roll, fov, aspect),
            eye: cam.pos,
            fwd: cam_forward,
            up: Vec3::Z,
            time,
            fov_x: fov,
            cull,
            hud_lines: Vec::new(),
        }
    }
}

/// The viewer's values every step of the live frame reads.
#[derive(Clone, Copy)]
struct Viewer {
    p: &'static net::protocol::Cod1Wire,
    client_num: i32,
    ps_client: i32,
    demo: bool,
    following: bool,
    pm_type: i32,
    snapshot_view: bool,
    skip_num: i32,
}

impl Viewer {
    /// Off the newest snapshot: who the camera is and how it views.
    fn of(net: &net::NetClient<net::UdpTransport>) -> Viewer {
        let p = net.protocol();
        let client_num = net.gamestate().map_or(-1, |g| g.client_num);
        // While following, ps.clientNum is the followed
        // client's (docs/research/cod11-events-and-fx.md,
        // section 7); the camera sits inside that body, so
        // skip it instead of ours.
        let ps_client = net
            .snapshots()
            .newest()
            .map_or(-1, |s| s.ps.field_i32(p, "clientNum"));
        // A demo draws its recorder as a followed
        // player, off the snapshots
        // (`CG_InterpolatePlayerState` under
        // `cg.demoPlayback`).
        let demo = net.is_demo();
        let following = ps_client >= 0 && (ps_client != client_num || demo);
        // Intermission (5) and dead (6, 7) take no view
        // from the cmd (Q3 `PM_UpdateViewAngles`), so
        // those draw the snapshot's view, as following does.
        let pm_type = net
            .snapshots()
            .newest()
            .map_or(0, |s| s.ps.field_i32(p, "pm_type"));
        let snapshot_view = following || (5..=7).contains(&pm_type);
        let skip_num = if following { ps_client } else { client_num };
        Viewer {
            p,
            client_num,
            ps_client,
            demo,
            following,
            pm_type,
            snapshot_view,
            skip_num,
        }
    }
}

/// A UO seat off the newest snapshot: the seat, the vehicle, its angles.
type Seated = (play::seat::Seat, u32, [f32; 3]);

/// `LiveCx::pose`'s locals, which the steps after it read.
struct Posed {
    instances: Vec<DynamicModelInstance>,
    muzzles: HashMap<u32, (Vec3, Vec3)>,
    muzzle_poses: HashMap<u32, fx::sim::TagPose>,
    brass: HashMap<u32, (Vec3, Vec3)>,
    shot_fallbacks: HashMap<u32, Vec3>,
    weapon_flash: HashMap<i32, String>,
    entity_pos: HashMap<u32, Vec3>,
    entity_angles: HashMap<u32, Vec3>,
    vehicle_angles: HashMap<u32, [f32; 3]>,
    vehicle_wheels: Vec<fx::vehicle_trails::Wheels>,
    heads: HashMap<u32, Vec3>,
    turret_eye: Option<crate::turret::TurretEye>,
    shadow_marks: Vec<crate::marks::MarkRequest>,
    view_lean: f32,
    feet_z: Option<f32>,
    render_time: Option<i32>,
    /// The intermission view's roll, the snapshots' `viewangles[2]`
    /// (radians).
    intermission_roll: f32,
}

/// The App fields the live frame's steps use, borrowed apart from the
/// mode and the renderer and named as App names them.
struct LiveCx<'a> {
    fs: &'a Pk3Fs,
    shell: &'a console::shell::Shell,
    session: &'a session::Session,
    audio: &'a mut audio::AudioSystem,
    fx: &'a mut fx::sim::FxSystem,
    flame: &'a mut fx::flame::FlameSystem,
    client_bullets: &'a mut fx::client_bullet::ClientBullets,
    hud: &'a mut Option<hud::Hud>,
    foreign_events: &'a mut HashSet<i32>,
    interp_misses: &'a mut u64,
    build_ms: &'a mut f32,
    viewmodel: &'a mut Arc<[model::Model]>,
    look_zoom: &'a mut (f32, bool),
    ev_seen: &'a mut u64,
    ev_unknown: &'a mut u64,
    fx_ms: &'a mut f32,
    quick_chat: &'a crate::quick_chat::QuickChat,
}

impl LiveCx<'_> {
    /// Poses the drawn pair's entities and puts the camera on it.
    #[expect(clippy::too_many_arguments)]
    fn pose(
        &mut self,
        prof: &mut frame_prof::Span,
        r: &mut Renderer,
        net: &mut Box<net::NetClient<net::UdpTransport>>,
        cam: &mut FlyCamera,
        input: &mut Box<play::input::PlayInput>,
        world: &mut collision::CollisionWorld,
        scene: &mut entities::EntityScene,
        clock: &mut play::clock::ServerClock,
        drawn_pos: &mut HashMap<u32, Vec3>,
        seat_frac: &mut f32,
        viewer: Viewer,
    ) -> Posed {
        let Viewer {
            p,
            client_num,
            following,
            pm_type,
            snapshot_view,
            skip_num,
            ..
        } = viewer;
        // Rebuilt every frame so absent entities (PVS churn) drop out.
        let mut instances: Vec<DynamicModelInstance> = Vec::new();
        // Per-shooter muzzle transforms for flashes and tracers.
        let mut muzzles: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
        let mut muzzle_poses: HashMap<u32, fx::sim::TagPose> = HashMap::new();
        // Per-shooter `tag_brass`, for spent cases.
        let mut brass: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
        // Bodies with no muzzle tag, where their tracers start.
        let mut shot_fallbacks: HashMap<u32, Vec3> = HashMap::new();
        let mut weapon_flash: HashMap<i32, String> = HashMap::new();
        let mut entity_pos: HashMap<u32, Vec3> = HashMap::new();
        let mut entity_angles: HashMap<u32, Vec3> = HashMap::new();
        let mut vehicle_angles: HashMap<u32, [f32; 3]> = HashMap::new();
        let mut vehicle_wheels = Vec::new();
        let mut heads: HashMap<u32, Vec3> = HashMap::new();
        let mut turret_eye = None;
        let mut shadow_marks = Vec::new();
        // The drawn `leanf`; the intermission view
        // takes none (cgame 0x30033413).
        let mut view_lean = 0.0;
        let mut intermission_roll = 0.0;
        // The drawn origin's height, under which
        // the eye never goes (cgame 0x30032de5).
        let mut feet_z = None;

        let render_time = net
            .snapshots()
            .newest()
            .and(clock.delta())
            .map(|_| clock.server_time());
        if let Some(render_time) = render_time
            && let Some((a, b)) = net.snapshots().two_for_time(render_time)
        {
            // No straddling pair: one frame, held.
            if std::ptr::eq(a, b) {
                *self.interp_misses += 1;
            }
            let oa = Vec3::from(a.ps.origin(p));
            let ob = Vec3::from(b.ps.origin(p));
            let f = ((Msec::from_raw(render_time) - a.server_time) as f32
                / (b.server_time - a.server_time).max(1) as f32)
                .clamp(0.0, 1.0);
            let t0 = Instant::now();
            prof.next(frame_prof::Stage::Pose);
            // Last frame's drawn bodies, for the gunner's
            // trace down.
            let bodies = play::predict::solid_bodies(p, &b.entities, client_num as u32, drawn_pos);
            scene.swing_speed = self.shell.cvar_f32("bg_swingSpeed");
            // The view a ridden UO gun turns with: ours
            // off the cmd, a followed one's off the
            // snapshots.
            scene.view = Some(if snapshot_view {
                let (va, vb) = (a.ps.viewangles(p), b.ps.viewangles(p));
                std::array::from_fn(|i| camera::lerp_angle(va[i], vb[i], f))
            } else {
                let delta = net::DELTA_ANGLE_FIELDS.map(|name| b.ps.field_i32(p, name));
                let (y, pt, r) = own_view(input.cmd_angles(), delta);
                [-pt.to_degrees(), y.to_degrees(), r.to_degrees()]
            });
            // The ridden body draws in a UO seat the view
            // leaves (cgame 0x3003392b), and is posed
            // undrawn otherwise. It is not in the snapshot,
            // so it is built from the playerstate, as
            // `cg.predictedPlayerEntity` is.
            let seat_body = b.ps.field_i32(p, "pm_type") != 5
                && play::seat::of_snapshot(p, b)
                    .is_some_and(|(seat, ..)| play::seat::own_body_drawn(seat, *seat_frac));
            let own_pair =
                play::seat::adds_view_body(b.ps.field_i32(p, "pm_flags"), seat_body, following)
                    .then(|| {
                        (
                            play::seat::own_body(p, &a.ps),
                            play::seat::own_body(p, &b.ps),
                        )
                    });
            let view = |s, own| entities::SnapView { snap: s, own };
            let built = entities::build_instances(
                scene,
                (
                    view(a, own_pair.as_ref().map(|o| &o.0)),
                    view(b, own_pair.as_ref().map(|o| &o.1)),
                    f,
                ),
                render_time,
                net.snapshots()
                    .newest()
                    .map_or(b.server_time, |s| s.server_time)
                    .raw(),
                if seat_body { -1 } else { skip_num },
                net.configstrings(),
                self.fs,
                Some(MoveWorld::new(world, &bodies, u32::MAX)),
                r,
                p,
            );
            *self.build_ms = t0.elapsed().as_secs_f32() * 1000.0;
            prof.next(frame_prof::Stage::Net);
            instances = built.instances;
            muzzles = built.muzzles;
            muzzle_poses = built.muzzle_poses;
            shot_fallbacks = built.shot_fallbacks;
            brass = built.brass;
            weapon_flash = built.weapon_flash;
            entity_pos = built.entity_pos;
            entity_angles = built.entity_angles;
            vehicle_angles = built.vehicle_angles;
            vehicle_wheels = built.vehicle_wheels;
            heads = built.heads;
            turret_eye = built.turret_eye;
            frame_digest::submodels(&built.submodels);
            r.set_submodels(&built.submodels);
            r.set_entity_lights(built.scene_lights);
            // `cg_shadows` 2 is the renderer's
            // stencil path; only 1 places blobs.
            if self.shell.cvar_f32("cg_shadows") as i32 == 1 {
                shadow_marks = built
                    .shadow_casters
                    .iter()
                    .filter_map(|&o| blob_shadow::player_shadow(&*world, oa, o))
                    .collect();
            }
            // Over 512 u is a teleport, not motion.
            let pos = if oa.distance(ob) > 512.0 {
                ob
            } else {
                oa.lerp(ob, f)
            };
            // viewHeightCurrent: eye offset above the feet origin.
            let vh = a.ps.field_f32(p, "viewHeightCurrent") * (1.0 - f)
                + b.ps.field_f32(p, "viewHeightCurrent") * f;
            cam.pos = pos + Vec3::Z * play::view::eye_height(pm_type, vh);
            feet_z = Some(pos.z);
            // `CG_InterpolatePlayerState` lerps `leanf`
            // too (cgame 0x3002936a).
            if pm_type != 5 {
                view_lean = a.ps.field_f32(p, "leanf") * (1.0 - f) + b.ps.field_f32(p, "leanf") * f;
            }
            if snapshot_view {
                let va_a = a.ps.viewangles(p);
                let va_b = b.ps.viewangles(p);
                cam.yaw = camera::lerp_angle(va_a[1], va_b[1], f).to_radians();
                cam.pitch = -camera::lerp_angle(va_a[0], va_b[0], f).to_radians();
                if pm_type == 5 {
                    intermission_roll = camera::lerp_angle(va_a[2], va_b[2], f).to_radians();
                }
            }
        }
        Posed {
            instances,
            muzzles,
            muzzle_poses,
            brass,
            shot_fallbacks,
            weapon_flash,
            entity_pos,
            entity_angles,
            vehicle_angles,
            vehicle_wheels,
            heads,
            turret_eye,
            shadow_marks,
            view_lean,
            feet_z,
            render_time,
            intermission_roll,
        }
    }

    /// The seat, the prediction, the view's playerstate and the snapshot's events.
    #[expect(clippy::too_many_arguments)]
    fn predict(
        &mut self,
        tick: &Tick,
        viewer: Viewer,
        prof: &mut frame_prof::Span,
        r: &mut Renderer,
        net: &mut Box<net::NetClient<net::UdpTransport>>,
        cam: &mut FlyCamera,
        input: &mut Box<play::input::PlayInput>,
        ring: &mut play::cmds::CmdRing,
        predictor: &mut Box<play::predict::Predictor>,
        view: &mut Box<play::view::OnlineView>,
        join: &mut Box<play::join::Join>,
        world: &mut collision::CollisionWorld,
        weapons: &[Option<weapon::WeaponDef>],
        events: &mut net::events::EventTracker,
        predicted_events: &mut play::events::PredictedEvents,
        drawn_pos: &mut HashMap<u32, Vec3>,
        drawn_vehicle_angles: &mut HashMap<u32, [f32; 3]>,
        render_time: Option<i32>,
        entity_pos: &HashMap<u32, Vec3>,
        vehicle_angles: &HashMap<u32, [f32; 3]>,
        feet_z: &mut Option<f32>,
        view_lean: &mut f32,
    ) -> (
        Option<Seated>,
        Option<play::predict::PredictedView>,
        Option<play::view::ViewPs>,
        Vec<net::events::GameEvent>,
    ) {
        let Tick { local_ms, .. } = *tick;
        let Viewer {
            p,
            client_num,
            ps_client,
            demo,
            pm_type,
            ..
        } = viewer;
        // A UO vehicle seat (docs/research/coduo-vehicles.md).
        let seat_now = net
            .snapshots()
            .newest()
            .and_then(|s| play::seat::of_snapshot(p, s));
        predictor.seat_angles = seat_now.map_or([0.0; 3], |(_, num, vang)| {
            drawn_vehicle_angles.get(&num).copied().unwrap_or(vang)
        });
        predictor.vehicles.clear();
        if let Some(s) = net.snapshots().newest().filter(|_| p.is_uo()) {
            predictor.vehicles.extend(
                s.entities
                    .iter()
                    .filter(|(_, e)| e.field_i32(p, "eType") == net::flags::ET_VEHICLE_V22)
                    .map(|(&n, e)| {
                        let at = drawn_pos.get(&n).copied();
                        (n, at.unwrap_or_else(|| Vec3::from(e.origin(p))))
                    }),
            );
        }
        // The cgame's `bg_*` cvars, off the 140/204 mirror.
        // UO's cgame never registers `bg_nofatigue`.
        predictor.bg = pmove::BgCvars {
            nofatigue: false,
            ..pmove::BgCvars::from_cvars(|c| join.cvars.get(c, net.configstrings(), self.shell))
        };
        if let Some(t) = render_time {
            predictor.set_time(t);
        }
        let predicted = if ps_client == client_num && !demo {
            net.snapshots().newest().and_then(|s| {
                let bodies =
                    play::predict::solid_bodies(p, &s.entities, client_num as u32, drawn_pos);
                let movers = (
                    pmove::movers::SnapshotMovers::from_entities(p, &s.entities),
                    s.server_time.raw(),
                );
                predictor.predict(p, &s.ps, ring, world, &bodies, &movers, weapons, local_ms)
            })
        } else {
            predictor.reset();
            None
        };
        drawn_pos.clone_from(entity_pos);
        drawn_vehicle_angles.clone_from(vehicle_angles);
        if let Some(v) = &predicted {
            cam.pos = v.origin + Vec3::Z * play::view::eye_height(pm_type, v.view_height);
            *feet_z = Some(v.origin.z);
            *view_lean = v.pred.ps.lean;
        }
        if let Some(s) = net.snapshots().newest() {
            let spawn = (s.ps.field_i32(p, "clientNum"), s.ps.arrays.stats[5]);
            view.track_spawn(spawn);
            if let Some(hud) = &mut self.hud {
                hud.track_spawn(spawn);
            }
            let mut held = play::input::Held::from_ps(&s.ps, p, weapons);
            // `cg.snap`, the older of the drawn pair.
            if let Some((snap, _)) = render_time.and_then(|t| net.snapshots().two_for_time(t)) {
                held.cg_snap(&s.ps, &snap.ps, p);
            }
            input.sync(&held);
        }
        // A followed player's view weapon, zoom and scope
        // come off the snapshot, as retail draws them.
        let view_ps = net.snapshots().newest().and_then(|s| {
            pmove::predict::on_foot(pm_type).then(|| {
                let mut v = match &predicted {
                    Some(v) => play::view::ViewPs::from_predicted(
                        &v.pred,
                        s.ps.field_i32(p, "viewmodelIndex"),
                    ),
                    None => play::view::ViewPs::from_snapshot(p, &s.ps),
                };
                v.seat = seat_now.map(|s| s.0);
                v
            })
        });
        if let Some(ps) = &view_ps
            && let Some(models) = view.sync_rig(self.fs, net.configstrings(), ps)
        {
            r.set_viewmodel(self.fs, &models);
            *self.viewmodel = models;
        }
        // `CG_TransitionPlayerState` feeds a hit ahead of
        // the snapshot's events and the view, mounted
        // or not (hud doc "Damage direction"): off the
        // prediction's newest playerstate, or a followed
        // player's at `cg.snap`'s transition, stamped
        // with `cg.snap`'s time either way.
        if let (Some(hud), Some(newest)) = (&mut self.hud, net.snapshots().newest()) {
            let own = pmove::predict::on_foot(pm_type) && predicted.is_some();
            let cg_snap = render_time
                .and_then(|t| net.snapshots().two_for_time(t))
                .map_or(newest, |(a, _)| a);
            let fed = if own { newest } else { cg_snap };
            hud.feed_damage(
                p,
                &fed.ps,
                render_time.unwrap_or(0),
                cg_snap.server_time.raw(),
            );
        }
        // Retail runs a snapshot's events in
        // `CG_ProcessSnapshots`, ahead of the view
        // (cgame 0x30033c14, 0x30033d9b): a quake or a
        // landing moves the frame it arrives on.
        prof.next(frame_prof::Stage::Events);
        let mut evs = Vec::new();
        if let Some(newest) = net.snapshots().newest() {
            // Our own ring plays off the prediction, and
            // the snapshot's copy of it is skipped.
            let own = ps_client == client_num && !demo;
            evs = play::events::frame_events(
                predicted_events,
                events,
                newest,
                p,
                own,
                own && pmove::predict::predictable(pm_type),
                predicted.as_ref().map(|v| &v.pred),
                ps_client,
            );
            if let Some(ps) = &view_ps {
                for _ in 0..play::events::view_kicks(&evs, ps_client) {
                    view.fire(weapons, ps);
                }
            }
            let fall = predictor.fall_heights;
            for ev in &evs {
                let Some(now) = render_time else {
                    break;
                };
                // At the event body's drawn origin; ours
                // is never in `entity_pos`.
                let body = entity_pos
                    .get(&ev.entity_num)
                    .copied()
                    .unwrap_or(Vec3::from(ev.pos));
                if let Some(q) =
                    play::quake::from_event(p, ev, newest.entities.get(&ev.entity_num), body)
                {
                    view.start_quake(q, now);
                }
                if ev.client_num == ps_client
                    && let Some(c) = play::view::land_change(ev.event, ev.parm, fall)
                {
                    view.land(c, now);
                }
            }
        }
        (seat_now, predicted, view_ps, evs)
    }

    /// `CG_CalcViewValues`' angles and eye: the kicks, the seat, the gun, the quake and the shellshock.
    #[expect(clippy::too_many_arguments)]
    fn view(
        &mut self,
        tick: &Tick,
        viewer: Viewer,
        prof: &mut frame_prof::Span,
        r: &mut Renderer,
        net: &mut Box<net::NetClient<net::UdpTransport>>,
        cam: &mut FlyCamera,
        input: &mut Box<play::input::PlayInput>,
        view: &mut Box<play::view::OnlineView>,
        join: &mut Box<play::join::Join>,
        world: &mut collision::CollisionWorld,
        weapons: &[Option<weapon::WeaponDef>],
        scene: &mut entities::EntityScene,
        seat_turn: &mut play::seat::FourWheelTurn,
        gunner_sway: &mut play::seat::GunnerSway,
        turret_sway: &mut play::seat::GunnerSway,
        seat_frac: &mut f32,
        entity_pos: &HashMap<u32, Vec3>,
        vehicle_angles: &HashMap<u32, [f32; 3]>,
        turret_eye: Option<crate::turret::TurretEye>,
        view_lean: f32,
        feet_z: Option<f32>,
        render_time: Option<i32>,
        intermission_roll: f32,
        seat_now: Option<Seated>,
        predicted: &Option<play::predict::PredictedView>,
        view_ps: &Option<play::view::ViewPs>,
    ) -> (f32, play::shock::ShockFrame) {
        let Tick { dt, .. } = *tick;
        let Viewer {
            p,
            pm_type,
            snapshot_view,
            ..
        } = viewer;
        prof.next(frame_prof::Stage::Vm);
        view.step_recoil(weapons, view_ps.as_ref(), render_time);
        // Next frame's cmds carry this kick, as
        // `CL_FinishMove` reads last frame's syscall 0x56.
        input.kick = view.view_kick();
        // `CG_CalcViewValues` draws the predicted
        // `viewangles[2]`, the kick's roll included; its
        // intermission arm the snapshot's, and returns before
        // the seat and the gun (uo cgame 0x30041a75).
        let intermission = pm_type == 5;
        let mut view_roll = intermission_roll;
        if !snapshot_view {
            let delta = match &predicted {
                Some(v) => v.delta_angles,
                None => net.snapshots().newest().map_or([0; 3], |s| {
                    net::DELTA_ANGLE_FIELDS.map(|name| s.ps.field_i32(p, name))
                }),
            };
            (cam.yaw, cam.pitch, view_roll) = own_view(input.cmd_angles(), delta);
        }
        // `CG_OffsetFirstPersonView` (cgame 0x30032b58)
        // first adds the damage kick and the sight's
        // bob; the intermission arm skips it.
        let mut damage_kick = [0.0; 3];
        if pm_type != 5
            && let (Some(hud), Some(s)) = (&mut self.hud, net.snapshots().newest())
        {
            let own = pmove::predict::on_foot(pm_type);
            let [kp, ky, kr] = hud.view_kick(
                p,
                &s.ps,
                predicted.as_ref().filter(|_| own).map(|v| &v.pred),
                render_time.unwrap_or(0),
                weapons,
            );
            damage_kick = [kp, ky, kr];
            cam.pitch -= kp.to_radians();
            cam.yaw += ky.to_radians();
            view_roll += kr.to_radians();
            // Then the walk bob, whose amplitudes a 1.5
            // server sets through the 140/204 mirror
            // (cod15-movement.md 6).
            hud.bob = if pmove::MoveRules::for_protocol(p).v15() {
                pmove::BobCvars::from_cvars(|c| join.cvars.get(c, net.configstrings(), self.shell))
            } else {
                pmove::BobCvars::default()
            };
            let kicked = [
                -cam.pitch.to_degrees(),
                cam.yaw.to_degrees(),
                view_roll.to_degrees(),
            ];
            cam.pos += hud.eye_bob(
                p,
                &s.ps,
                predicted.as_ref().filter(|_| own).map(|v| &v.pred),
                render_time.unwrap_or(0),
                kicked,
            );
        }
        // Then the landing's dip (0x30032ca1), and it
        // ends on `AddLeanToPosition` (cgame
        // 0x30032dda): a lean moves the eye and rolls
        // nothing.
        if pm_type != 5
            && let Some(now) = render_time
        {
            cam.pos.z += view.land_dip(now);
        }
        cam.pos =
            pmove::aim::add_lean(cam.pos, cam.yaw, view_lean, vcod_common::patch::Arith::Msvc);
        if pm_type != 5
            && let Some(z) = feet_z
        {
            cam.pos.z = cam.pos.z.max(z + 8.0);
        }
        // In a vehicle seat `CG_VehicleView` (uo cgame
        // 0x30040810) replaces the first-person offsets.
        if let Some((seat, num, vang)) = seat_now.filter(|_| !intermission) {
            let delta = match &predicted {
                Some(v) => v.delta_angles,
                None => net.snapshots().newest().map_or([0; 3], |s| {
                    net::DELTA_ANGLE_FIELDS.map(|name| s.ps.field_i32(p, name))
                }),
            };
            let newest_ps = &net.snapshots().newest().expect("seated").ps;
            let frac = predicted.as_ref().map_or_else(
                || newest_ps.field_f32(p, "fWeaponPosFrac"),
                |v| v.pred.ps.weapon_pos_frac,
            );
            *seat_frac = frac;
            // A followed rider's view is the snapshot's.
            let wire_view = |input: &play::input::PlayInput| {
                let (y, pt) = if snapshot_view {
                    (cam.yaw, cam.pitch)
                } else {
                    let (y, pt, _) = own_view_at(input.cmd_angles(), delta, SEAT_PITCH_CLAMP);
                    (y, pt)
                };
                [-pt.to_degrees(), y.to_degrees(), 0.0]
            };
            let mut tag = |name: &str| scene.tag_pose(self.fs, num, name);
            // 0x30040580: a four-wheel turns its riders by
            // the vehicle's `lerpAngles` (0x3004060f).
            let drawn_angles = vehicle_angles.get(&num).copied().unwrap_or(vang);
            let mut seat_roll = 0.0;
            if seat.class == play::seat::CLASS_FOUR_WHEEL
                && seat.pos != 2
                && !seat.own_weapon()
                && newest_ps.field_i32(p, "weapon") != 0
            {
                seat_roll = play::seat::seat_roll(wire_view(input), drawn_angles);
                if snapshot_view {
                    seat_turn.reset();
                } else if let Some(v) = seat_turn.step(
                    wire_view(input),
                    drawn_angles,
                    frac != 0.0,
                    newest_ps.field_i32(p, "eFlags") & net::flags::EF_HULL_VIEW_V22 != 0,
                ) {
                    input.set_view(v, delta);
                }
            } else {
                seat_turn.reset();
            }
            let origin = entity_pos.get(&num).copied().unwrap_or(cam.pos);
            // The gunner's sway off a `thompson_MP` copy of
            // the predicted playerstate, and its shake off
            // `cg.snap`'s (coduo-vehicles.md 5).
            let mut gunner = play::seat::GunnerEye::default();
            if seat.pos == 2
                && let Some(now) = render_time
            {
                gunner.time_ms = now;
                let thompson = net
                    .configstring(7)
                    .split(' ')
                    .position(|w| w.eq_ignore_ascii_case("thompson_mp"))
                    .and_then(|i| weapons.get(i + 1)?.as_ref());
                if let (Some(v), Some(def)) = (&predicted, thompson) {
                    gunner.sway = gunner_sway.step(&v.pred.ps, def, wire_view(input), now);
                }
                let snaps = net.snapshots();
                if let Some(s) = snaps.two_for_time(now).map(|(a, _)| a).or(snaps.newest()) {
                    let movetype = s
                        .entities
                        .get(&num)
                        .map_or(0, |e| e.field_i32(p, "animMovetype"));
                    if play::seat::gunner_shakes(s.ps.field_i32(p, "eFlags"), movetype) {
                        gunner.shake = Some(play::seat::shake_phase(now));
                    }
                }
            }
            if let Some(sc) =
                play::seat::camera(seat, wire_view(input), frac, origin, gunner, &mut tag)
            {
                if !snapshot_view {
                    if let Some(v) = sc.snap {
                        input.set_view(v, delta);
                    }
                    (cam.yaw, cam.pitch, _) =
                        own_view_at(input.cmd_angles(), delta, SEAT_PITCH_CLAMP);
                }
                // A four-wheel's first person (`fWeaponPosFrac` 0) draws the
                // predicted roll; its chase camera zeroes it (0x30040dc0).
                view_roll = if frac == 0.0 {
                    seat_roll.to_radians()
                } else {
                    0.0
                };
                if let Some([pitch, roll]) = sc.gun_pitch_roll {
                    cam.pitch = -pitch.to_radians();
                    view_roll = roll.to_radians();
                }
                cam.yaw += sc.yaw_shake.to_radians();
                let half = Vec3::splat(8.0);
                cam.pos = play::seat::clip(&sc, &mut |a, b, mask| {
                    let t = world.move_trace(a, b, -half, half, mask);
                    (t.fraction, t.endpos, t.startsolid)
                });
            }
        } else {
            seat_turn.reset();
        }
        // On a mounted gun the view rides the gun's
        // `tag_player` and barrel, not the cmd's angles
        // (docs/research/cod11-turrets.md section 14).
        // 1.5's and UO's (1.5 cgame 0x300348c0, uo
        // 0x30041550) hold the view inside the gun's
        // arcs; UO's sways the eye.
        if let Some(eye) = turret_eye
            .filter(|_| !intermission)
            .map(|e| e.kicked(damage_kick))
        {
            cam.pos = eye.pos;
            (cam.yaw, cam.pitch) = eye.view();
            view_roll = eye.angles[2].to_radians();
            if !snapshot_view && let Some([pt, y]) = eye.snap {
                let delta = match &predicted {
                    Some(v) => v.delta_angles,
                    None => net.snapshots().newest().map_or([0; 3], |s| {
                        net::DELTA_ANGLE_FIELDS.map(|name| s.ps.field_i32(p, name))
                    }),
                };
                // The cmd adds the kick back on.
                input.set_view([pt - input.kick[0], y - input.kick[1]], delta);
            }
            if let Some(axis) = eye.axis {
                let thompson = net
                    .configstring(7)
                    .split(' ')
                    .position(|w| w.eq_ignore_ascii_case("thompson_mp"))
                    .and_then(|i| weapons.get(i + 1)?.as_ref());
                if let (Some(v), Some(def), Some(now)) = (&predicted, thompson, render_time) {
                    let o =
                        turret_sway.step(&v.pred.ps, def, scene.view.unwrap_or(eye.angles), now);
                    cam.pos += turret::sway_offset(axis, o);
                }
            }
        }
        // The quake shakes whatever view that left
        // (cgame 0x30033da9, after `CG_CalcViewValues`).
        if let Some(now) = render_time {
            let [sp, sy, sr] = view.shake(now, cam.pos);
            cam.pitch -= sp.to_radians();
            cam.yaw += sy.to_radians();
            view_roll += sr.to_radians();
        }
        // The shellshock off `cg.snap`'s playerstate
        // (cgame 0x3002f7d0, 0x3002e900); its kick
        // turns the view after the quake (0x30033dbd).
        let shock = match render_time {
            Some(now) => {
                if let Some(args) = view.shock_command.take() {
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    if let Err(e) = view.shock.command(self.fs, &args, now) {
                        console::log::print(&e);
                    }
                }
                let snaps = net.snapshots();
                let fields = snaps
                    .two_for_time(now)
                    .map(|(a, _)| a)
                    .or(snaps.newest())
                    .map_or((0, 0, 0), |s| {
                        let f = |n: &str| s.ps.field_i32(p, n);
                        (
                            f("shellshockIndex"),
                            f("shellshockTime"),
                            f("shellshockDuration"),
                        )
                    });
                let cs = net.configstrings();
                view.shock.frame(
                    self.fs,
                    |i| {
                        p.cs.slot(net::protocol::CsRange::ShellShock, i)
                            .and_then(|k| cs.get(k))
                            .map(String::as_str)
                    },
                    fields,
                    now,
                )
            }
            None => play::shock::ShockFrame::rest(),
        };
        if shock.kick != [0.0; 2] {
            let pitch;
            [pitch, cam.yaw, view_roll] = play::shock::kicked_angles(
                [
                    -cam.pitch.to_degrees(),
                    cam.yaw.to_degrees(),
                    view_roll.to_degrees(),
                ],
                shock.kick,
            )
            .map(f32::to_radians);
            cam.pitch = -pitch;
        }
        input.shellshock = shock.slowed;
        if let Some(hud) = &mut self.hud {
            hud.drawn_view = Some([
                -cam.pitch.to_degrees(),
                cam.yaw.to_degrees(),
                view_roll.to_degrees(),
            ]);
        }
        input.set_mouse_caps(dt * 1000.0, [shock.mouse.max_pitch, shock.mouse.max_yaw]);
        self.audio.set_shellshock(self.fs, &shock.sound);
        r.set_screen_blend(shock.blend_ms);
        (view_roll, shock)
    }

    /// The view weapon, the listener and the muzzles, then the effects' step.
    #[expect(clippy::too_many_arguments)]
    fn gun(
        &mut self,
        tick: &Tick,
        viewer: Viewer,
        prof: &mut frame_prof::Span,
        vm: &mut Option<renderer::VmDraw>,
        net: &mut Box<net::NetClient<net::UdpTransport>>,
        cam: &mut FlyCamera,
        view: &mut Box<play::view::OnlineView>,
        world: &mut collision::CollisionWorld,
        weapons: &[Option<weapon::WeaponDef>],
        scene: &mut entities::EntityScene,
        looped_fx: &mut fx::looped::LoopedFx,
        vehicle_trails: &mut fx::vehicle_trails::VehicleTrails,
        muzzles: &mut HashMap<u32, (Vec3, Vec3)>,
        muzzle_poses: &mut HashMap<u32, fx::sim::TagPose>,
        brass: &mut HashMap<u32, (Vec3, Vec3)>,
        vehicle_wheels: &[fx::vehicle_trails::Wheels],
        render_time: Option<i32>,
        predicted: &Option<play::predict::PredictedView>,
        view_ps: &Option<play::view::ViewPs>,
        view_roll: f32,
        shock: play::shock::ShockFrame,
    ) -> (bool, f32, Vec3, Vec3, Vec3, Instant) {
        let Tick {
            dt,
            time,
            local_ms,
            aspect,
            widescreen,
            ..
        } = *tick;
        let Viewer {
            p,
            client_num,
            ps_client,
            pm_type,
            skip_num,
            ..
        } = viewer;
        // `0x300371f0` poses the view weapon off the
        // finished view, the gun's aim stepped once for
        // it and the scope (combat doc 17.4).
        let local_player = ps_client == client_num && pmove::predict::on_foot(pm_type);
        if let Some(hud) = &mut self.hud {
            hud.set_shock_left(shock.left_ms);
        }
        let gun = match (&mut self.hud, net.snapshots().newest()) {
            (Some(hud), Some(s)) => hud.step_gun(
                p,
                &s.ps,
                predicted.as_ref().filter(|_| local_player).map(|v| &v.pred),
                render_time.unwrap_or(0),
                weapons,
                view.gun_kick(),
            ),
            _ => None,
        };
        let world_up = {
            let [f, l, u] = pmove::aim::angles_to_axis_msvc([
                -cam.pitch.to_degrees(),
                cam.yaw.to_degrees(),
                view_roll.to_degrees(),
            ]);
            Vec3::new(f[2], l[2], u[2])
        };
        let cg_fov = cg_fov(self.shell);
        view.draw_gun = play::view::DrawGun::from_cvar(self.shell.cvar_f32("cg_drawGun") as i32);
        let (mut vm_draw, fov) =
            view.frame(cg_fov, weapons, view_ps.as_ref(), local_ms, gun, world_up);
        // `0x30033e97`: the shellshock's sensitivity
        // scale rides the zoom's, on the 4:3 fovs.
        *self.look_zoom = (
            fov / cg_fov * shock.mouse.sensitivity,
            view_ps.as_ref().is_some_and(|v| v.mounted),
        );
        let fov = camera::view_fov_x(fov, aspect, widescreen);
        if let Some(d) = &mut vm_draw {
            d.fov_x = fov;
        }
        *vm = vm_draw;

        let (cam_forward, cam_right, cam_up) = camera::basis(cam.yaw, cam.pitch);
        // The ridden body is `skip_num`, never in `entity_pos`,
        // so the listener is told which entity rides the camera.
        self.audio.set_listener(
            cam.pos,
            cam_forward,
            cam_right,
            cam_up,
            audio::cues::ps_entity(ps_client),
        );

        // The view weapon's tags turn with the whole view, its roll too:
        // the viewmodel refEntity's axis (cgame 0x3020cbb8) poses them.
        let gun_basis = {
            let [f, l, u] = pmove::aim::angles_to_axis_msvc([
                -cam.pitch.to_degrees(),
                cam.yaw.to_degrees(),
                view_roll.to_degrees(),
            ]);
            (Vec3::from(f), -Vec3::from(l), Vec3::from(u))
        };
        let (muzzle_pos, muzzle_dir) = view
            .muzzle(cam.pos, gun_basis)
            .unwrap_or_else(|| view_muzzle(cam.pos, cam_forward, cam_right, cam_up));
        let muzzle_pose = view
            .muzzle_pose(cam.pos, gun_basis)
            .unwrap_or_else(|| fx::sim::TagPose::along(muzzle_pos, muzzle_dir));
        // Under a scope our own shots draw no flash: with
        // no muzzle the fire event resolves to nothing.
        if !view.scoped() {
            muzzles.insert(u32::MAX, (muzzle_pos, muzzle_dir));
            muzzle_poses.insert(u32::MAX, muzzle_pose);
        }
        // Bullet hits carry the shooter's number in
        // `other_entity_num`, and the body the camera rides
        // (ours, or the followed player's) is excluded from
        // the muzzle map, so key the view muzzle under it too
        // for the whizby; its tracer is `view_body`'s call.
        if let Ok(num) = u32::try_from(skip_num) {
            muzzles.insert(num, (muzzle_pos, muzzle_dir));
            muzzle_poses.insert(num, muzzle_pose);
        }
        // The ridden body's cases leave the viewmodel's
        // `tag_brass` (`CG_EjectBrass`, cgame 0x30038a30).
        if let Some(tag) = view.brass(cam.pos, gun_basis) {
            self.fx.set_view_brass(tag.0);
            brass.insert(u32::MAX, tag);
            if let Ok(num) = u32::try_from(skip_num) {
                brass.insert(num, tag);
            }
        }

        // Step before this frame's events spawn, or the
        // [now-dt, now] integration would move particles born
        // at `now` (a tracer would start ahead of its muzzle).
        let fx_t0 = Instant::now();
        prof.next(frame_prof::Stage::FxStep);
        self.fx.set_view(cam.pos);
        let bolts: HashMap<u32, fx::sim::TagPose> = muzzles
            .iter()
            .map(|(&k, &(pos, dir))| {
                let pose = muzzle_poses.get(&k).copied();
                (k, pose.unwrap_or_else(|| fx::sim::TagPose::along(pos, dir)))
            })
            .collect();
        self.fx.set_bolts(&bolts);
        // The `playFxOnTag` tags a live effect rides,
        // posed as drawn (CoDMP.exe 0x49e8d0, a bolt
        // posed once a frame).
        let tag_poses = self
            .fx
            .tag_bolts()
            .into_iter()
            .filter_map(|b| {
                let tag = fx::registry::bolt_tag(net.configstrings(), &p.cs, b.slot)?;
                Some((b, scene.tag_pose(self.fs, b.entity, tag)?))
            })
            .collect();
        self.fx.set_tag_bolts(&tag_poses);
        self.fx.step(dt, time, Some(&*world));
        prof.next(frame_prof::Stage::Fx);
        let chained = self.fx.spawn_chained(self.fs, time);
        // Placed by the wall-clock step (the brass).
        frame_digest::clocked(|| self.audio.play_fx(self.fs, chained));
        // UO's wheel dust (uo cgame 0x30020540): the
        // world under each wheel, mask 0x211, then
        // every other vehicle's collmap where drawn.
        fx::vehicle_trails::place_collmaps(world, vehicle_wheels);
        let trails = vehicle_trails.frame(
            self.shell.cvar_f32("cg_vehicletrails") as i32 != 0,
            vehicle_wheels,
            |n, a, b| {
                let t = fx::vehicle_trails::wheel_trace(world, n, a, b);
                (t.fraction < 1.0).then_some(t.surface_flags)
            },
        );
        for play in trails {
            let at = fx::sim::SpawnAt::Directed {
                pos: play.pos,
                dir: play.dir,
            };
            let sounds = self.fx.spawn(self.fs, play.path, at, time);
            self.audio.play_fx(self.fs, sounds);
        }
        // `ET_FX` off the drawn snapshot (`CG_FX`, cgame
        // 0x3001cf20), on the drawn server time.
        if let Some(t) = render_time
            && let Some((a, _)) = net.snapshots().two_for_time(t)
        {
            for play in looped_fx.frame(a, p, t, cam.pos, net.configstrings()) {
                let sounds = self
                    .fx
                    .spawn_marked(self.fs, &play.path, play.at, None, time, true);
                self.audio.play_fx(self.fs, sounds);
            }
        }
        (local_player, fov, cam_forward, cam_right, cam_up, fx_t0)
    }

    /// The tracers' rules, then each event: its round, the HUD's share, the stance and the weapon, its sound and effects.
    #[expect(clippy::too_many_arguments)]
    fn events(
        &mut self,
        tick: &Tick,
        viewer: Viewer,
        vm: &Option<renderer::VmDraw>,
        net: &net::NetClient<net::UdpTransport>,
        cam: &mut FlyCamera,
        input: &mut Box<play::input::PlayInput>,
        world: &mut collision::CollisionWorld,
        weapons: &Vec<Option<weapon::WeaponDef>>,
        scene: &mut entities::EntityScene,
        muzzles: &HashMap<u32, (Vec3, Vec3)>,
        brass: &HashMap<u32, (Vec3, Vec3)>,
        shot_fallbacks: &HashMap<u32, Vec3>,
        weapon_flash: &HashMap<i32, String>,
        entity_pos: &HashMap<u32, Vec3>,
        predicted: &Option<play::predict::PredictedView>,
        evs: Vec<net::events::GameEvent>,
        local_player: bool,
        newest: Option<&net::snapshot::Snapshot>,
        hud_frame: &hud::HudFrame,
    ) {
        let Tick {
            dt,
            time,
            blood,
            eject_brass,
            ..
        } = *tick;
        let Viewer {
            p,
            client_num,
            ps_client,
            ..
        } = viewer;
        // UO's tracers: the length gate, and `sv_night`
        // off the systeminfo (fx::sim::TracerRules).
        let uo = p.is_uo();
        self.fx.set_tracer_rules(fx::sim::TracerRules {
            min_length: uo,
            night: uo
                && net::info_value_for_key(net.configstring(1), "sv_night")
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .is_some_and(|v| v as i32 != 0),
        });
        // Events use the newest snapshot, not the interpolation
        // pair; they must not wait out the interp delay.
        if let Some(newest) = newest {
            let ctx = fx::registry::ResolveCtx {
                brass,
                weapons: weapons.as_slice(),
                eject_brass,
                configstrings: net.configstrings(),
                cs: &p.cs,
                blood,
                muzzles,
                shot_fallbacks,
                weapon_flash,
                view_flash: vm.is_some().then_some(weapons.as_slice()),
                view_body: fx::registry::view_body(newest.ps.field_i32(p, "pm_flags"), ps_client),
            };
            // A client-traced round's hit joins the
            // queue after its fire event, with the
            // tracer's tag as its shooter's muzzle.
            let mut evs: std::collections::VecDeque<_> =
                evs.into_iter().map(|e| (e, None)).collect();
            let bodies =
                play::predict::solid_bodies(p, &newest.entities, client_num as u32, entity_pos);
            // Brush models and vehicles' collmaps where
            // the newest snapshot has them, as the
            // cgame's entity clip meets them.
            let movers = pmove::movers::SnapshotMovers::from_entities(p, &newest.entities);
            movers.place(world, newest.server_time.raw());
            let round_trace = |s: Vec3, e: Vec3, skip: u32| {
                let w = MoveWorld::new(world, &bodies, skip);
                let t = w.box_trace(
                    s,
                    e,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    fx::client_bullet::MASK_CLIENT_SHOT,
                );
                fx::client_bullet::RoundTrace {
                    fraction: t.fraction,
                    endpos: t.endpos,
                    normal: t.normal,
                    surface_flags: t.surface_flags,
                    entity: w.entity_num(&t),
                }
            };
            // The second trace stops only on a
            // vehicle; the coaxial's passes its own.
            let vehicle_trace = |s: Vec3, e: Vec3, skip: Option<u32>| {
                let own = skip.map(collision::Pusher::Copy);
                let t = world.point_trace_except(
                    s,
                    e,
                    fx::client_bullet::MASK_VEHICLE_SHOT,
                    false,
                    own,
                );
                let entity = world.entity_num(&t);
                newest
                    .entities
                    .get(&entity)
                    .is_some_and(|v| v.field_i32(p, "eType") == net::flags::ET_VEHICLE_V22)
                    .then_some(fx::client_bullet::RoundTrace {
                        fraction: t.fraction,
                        endpos: t.endpos,
                        normal: t.normal,
                        surface_flags: t.surface_flags,
                        entity,
                    })
            };
            let view = fx::client_bullet::View {
                origin: cam.pos,
                angles: [0, 1, 2].map(|i| newest.ps.field_f32(p, &format!("viewangles[{i}]"))),
            };
            while let Some((ev, round_muzzle)) = evs.pop_front() {
                let ctx = match &round_muzzle {
                    Some(m) => fx::registry::ResolveCtx { muzzles: m, ..ctx },
                    None => fx::registry::ResolveCtx { ..ctx },
                };
                let muzzles = ctx.muzzles;
                if let Some(round) = fx::registry::client_round(&ev, &ctx)
                    && let Some(pose) = scene.tag_pose(self.fs, round.entity, round.tag)
                {
                    let def = usize::try_from(round.weapon)
                        .ok()
                        .and_then(|w| weapons.get(w)?.as_ref());
                    match self.client_bullets.fire(
                        time.to_bits(),
                        &round,
                        def,
                        &pose,
                        Some(view),
                        &round_trace,
                        &vehicle_trace,
                    ) {
                        Some(fx::client_bullet::Outcome::Hit {
                            pos,
                            normal,
                            reflect,
                            surf_type,
                        }) => {
                            // No tag, no tracer (0x30048b60).
                            let tracer: HashMap<u32, (Vec3, Vec3)> = scene
                                .tag_pose(self.fs, round.entity, round.tracer_tag)
                                .map(|t| (round.entity, (t.origin, t.axis[0])))
                                .into_iter()
                                .collect();
                            evs.push_front((
                                fx::client_bullet::hit_event(
                                    &round, pos, normal, reflect, surf_type,
                                ),
                                Some(tracer),
                            ));
                        }
                        Some(fx::client_bullet::Outcome::Miss { start, end }) => {
                            let heavy = def.is_some_and(fx::registry::heavy_round);
                            self.fx
                                .spawn_tracer((start, end), false, (heavy, true), time, dt);
                        }
                        None => {}
                    }
                }
                *self.ev_seen += 1;
                if let Some(hud) = &mut self.hud {
                    hud.on_game_event(&ev, hud_frame);
                }
                if let Some(s) = play::events::forced_stance(
                    &ev,
                    client_num,
                    // Retail's `+prone` / `+movedown`
                    // set it 1 while held (0x40aa60).
                    self.shell.cvar_f32("cl_stanceTemp") as i32 != 0 || input.stance_held(),
                ) {
                    input.force_stance(s);
                }
                if let Some(w) = play::events::picked_up_weapon(&ev, ctx.view_body, net.protocol())
                    && input.selected() == 0
                {
                    input.picked_up(w);
                    if let Some(hud) = &mut self.hud {
                        hud.weapon_selected();
                    }
                }
                if play::events::runs_out_of_ammo(&ev, ctx.view_body) {
                    // Off the predicted playerstate, as
                    // 0x300381b0 reads it.
                    if let Some(v) = predicted.as_ref().filter(|_| local_player) {
                        input.predict_held(&v.pred.ps, weapons);
                    }
                    input.out_of_ammo();
                }
                self.audio.on_game_event(
                    self.fs,
                    &ev,
                    net.configstrings(),
                    muzzles,
                    ctx.shot_fallbacks,
                );
                for r in fx::registry::resolve(&ev, &ctx) {
                    match r {
                        fx::registry::Resolved::Spawn {
                            path,
                            at,
                            nomarks,
                            bolt,
                            brass,
                        } => {
                            // `cullrange` from the origin the
                            // cgame hands syscall 0xe5: the
                            // view weapon's last `tag_brass`
                            // for a case, the shooter's
                            // `lerpOrigin` for a flash.
                            let cull_from = if brass {
                                Some(self.fx.view_brass())
                            } else {
                                bolt.filter(|&n| n != u32::MAX)
                                    .and_then(|n| scene.drawn_origin(n))
                            };
                            let sounds = self.fx.spawn_marked_from(
                                self.fs, &path, at, bolt, time, !nomarks, cull_from,
                            );
                            self.audio.play_fx(self.fs, sounds);
                        }
                        fx::registry::Resolved::OnTag {
                            path,
                            entity,
                            tag,
                            slot,
                        } => {
                            // The playerState ring's
                            // events are the view
                            // body's, posed undrawn.
                            let entity = match entity {
                                u32::MAX => ps_client as u32,
                                n => n,
                            };
                            // `cullrange` from its `lerpOrigin`
                            // (cgame 0x3001db80).
                            let origin = scene.drawn_origin(entity);
                            if let Some(pose) = scene.tag_pose(self.fs, entity, &tag) {
                                let bolt = fx::sim::TagBolt { entity, slot };
                                let sounds = self
                                    .fx
                                    .spawn_on_tag_from(self.fs, &path, bolt, pose, time, origin);
                                self.audio.play_fx(self.fs, sounds);
                            }
                        }
                        fx::registry::Resolved::Tracer {
                            muzzle,
                            impact,
                            flesh,
                            heavy,
                            roll,
                        } => {
                            self.fx
                                .spawn_tracer((muzzle, impact), flesh, (heavy, roll), time, dt);
                        }
                        fx::registry::Resolved::Known => {}
                        fx::registry::Resolved::Unknown => {
                            *self.ev_unknown += 1;
                            let foreign = ev.event & net::event_ids::EV_FOREIGN;
                            if foreign != 0 && self.foreign_events.insert(ev.event & !foreign) {
                                log::info!(
                                    "protocol {} event {} not played",
                                    p.transport.version,
                                    ev.event & !foreign
                                );
                            }
                            log::debug!("unknown event {} parm {}", ev.event, ev.parm);
                        }
                    }
                }
            }
        }
    }

    /// The effects' shakes, the loop and blend sounds, then the effects' models.
    #[expect(clippy::too_many_arguments)]
    fn sound(
        &mut self,
        tick: &Tick,
        viewer: Viewer,
        prof: &mut frame_prof::Span,
        r: &mut Renderer,
        net: &net::NetClient<net::UdpTransport>,
        view: &mut Box<play::view::OnlineView>,
        world: &mut collision::CollisionWorld,
        weapons: &[Option<weapon::WeaponDef>],
        scene: &mut entities::EntityScene,
        last_loop_snap: &mut Option<u32>,
        instances: &mut Vec<DynamicModelInstance>,
        muzzles: &HashMap<u32, (Vec3, Vec3)>,
        entity_pos: &HashMap<u32, Vec3>,
        entity_angles: &HashMap<u32, Vec3>,
        render_time: Option<i32>,
        newest: Option<&net::snapshot::Snapshot>,
    ) {
        let Tick { dt, time, .. } = *tick;
        let Viewer { p, .. } = viewer;
        // A UO effect's `CameraShake` feeds the same
        // shake core as an earthquake (uo cgame
        // vmMain 16, 0x3002b0e0).
        for sh in self.fx.take_shakes() {
            if let Some(now) = render_time {
                view.start_quake(
                    play::quake::Quake {
                        start_ms: 0,
                        scale: sh.scale,
                        duration_ms: sh.duration_ms,
                        radius: sh.radius,
                        origin: sh.origin,
                    },
                    now,
                );
            }
        }

        prof.next(frame_prof::Stage::Sound);
        // `es.loopSound` (docs/research/cod11-sound-system.md,
        // section 9) and each missile's `projectileSound`
        // (section 8), reconciled once per snapshot. An entity
        // that left the snapshot is absent from the map, which
        // is what stops its loop.
        if let Some(newest) = newest
            && *last_loop_snap != Some(newest.message_num)
        {
            *last_loop_snap = Some(newest.message_num);
            let loops: HashMap<u32, (i32, Vec3)> = newest
                .entities
                .iter()
                .filter_map(|(&num, e)| {
                    let idx = e.field_i32(p, "loopSound");
                    (idx != 0).then(|| (num, (idx, Vec3::from(e.origin(p)))))
                })
                .collect();
            self.audio
                .set_loop_sounds(self.fs, net.configstrings(), &loops);
            self.audio.set_projectile_sounds(
                self.fs,
                net.configstrings(),
                &audio::missile_sounds(p, newest),
            );
        }

        // UO's sound blend entities, every frame
        // off the drawn pair (uo cgame 0x30021860).
        if let Some(t) = render_time
            && let Some((a, b)) = net.snapshots().two_for_time(t)
        {
            let f = ((Msec::from_raw(t) - a.server_time) as f32
                / (b.server_time - a.server_time).max(1) as f32)
                .clamp(0.0, 1.0);
            // A blend restarts when its voice ran out,
            // on the wall clock.
            frame_digest::clocked(|| {
                self.audio.step_sound_blends(
                    self.fs,
                    p,
                    (a, b, f),
                    (net.configstrings(), weapons),
                    entity_pos,
                )
            });
        }

        // After the drain, so new voices get this frame's positions.
        self.audio
            .step_fire_loops(self.fs, dt * 1000.0, entity_pos, muzzles);
        self.audio.step(entity_pos, entity_angles, Some(&*world));

        prof.next(frame_prof::Stage::Pose);
        // The brass below steps on wall-clock time.
        frame_digest::models(instances);
        // After the events, so a case ejected this frame
        // draws this frame.
        for m in self.fx.models(time) {
            if let Some(model) = scene.fx_model(r, self.fs, &m.model) {
                let [f, l, u] = m.axis;
                instances.push(DynamicModelInstance {
                    model,
                    transform: glam::Mat4::from_cols(
                        f.extend(0.0),
                        l.extend(0.0),
                        u.extend(0.0),
                        m.origin.extend(1.0),
                    ),
                    bones: None,
                    light_origin: m.origin,
                    shader_time: None,
                });
            }
        }
        frame_prof::count(frame_prof::Count::Models, instances.len());
        frame_prof::count(frame_prof::Count::Particles, self.fx.counts().0);
        r.set_dynamic_models(instances);
        prof.next(frame_prof::Stage::FxDraw);
    }

    /// UO's flame and its report, then the effects' draw; the `fdc` to send.
    #[expect(clippy::too_many_arguments)]
    fn flame(
        &mut self,
        tick: &Tick,
        viewer: Viewer,
        r: &mut Renderer,
        net: &net::NetClient<net::UdpTransport>,
        cam: &mut FlyCamera,
        input: &mut Box<play::input::PlayInput>,
        world: &mut collision::CollisionWorld,
        weapons: &[Option<weapon::WeaponDef>],
        muzzles: &HashMap<u32, (Vec3, Vec3)>,
        entity_pos: &HashMap<u32, Vec3>,
        heads: &HashMap<u32, Vec3>,
        shadow_marks: &[crate::marks::MarkRequest],
        render_time: Option<i32>,
        cam_right: Vec3,
        cam_up: Vec3,
        fx_t0: Instant,
        newest: Option<&net::snapshot::Snapshot>,
    ) -> Option<u32> {
        let Tick { dt, time, .. } = *tick;
        let Viewer {
            p,
            client_num,
            ps_client,
            ..
        } = viewer;
        // `cg.snap`'s own player and solid bodies, the
        // older of the drawn pair (coduo-diff.md 19):
        // the chunks clip them and the report tests
        // the player.
        let (victim, bodies) = if p.is_uo() {
            let snap = render_time
                .and_then(|t| net.snapshots().two_for_time(t))
                .map(|(a, _)| a)
                .or(newest);
            let victim = snap.and_then(|s| {
                let v = |n: &str| {
                    Vec3::from_array([0, 1, 2].map(|i| s.ps.field_f32(p, &format!("{n}[{i}]"))))
                };
                Some(fx::flame::Victim {
                    client: u32::try_from(s.ps.field_i32(p, "clientNum")).ok()?,
                    origin: v("origin"),
                    mins: v("mins"),
                    maxs: v("maxs"),
                })
            });
            let bodies = snap
                .zip(victim)
                .map(|(s, v)| play::predict::solid_bodies(p, &s.entities, v.client, entity_pos))
                .unwrap_or_default();
            (victim, bodies)
        } else {
            (None, Vec::new())
        };
        // UO's flamethrower streams off each firing
        // body's `tag_flash` (fx::flame).
        let nozzles = newest
            .map(|s| fx::flame::nozzles(p, s, ps_client, weapons, muzzles))
            .unwrap_or_default();
        if !nozzles.is_empty() || !self.flame.is_empty() {
            let sounds = self.flame.step(
                time * 1000.0,
                dt * 1000.0,
                &nozzles,
                cam.pos,
                Some(&fx::flame::ChunkWorld {
                    world,
                    bodies: &bodies,
                    local: victim,
                    max_clients: fx::flame::max_clients(net.configstring(0)),
                }),
            );
            for s in sounds {
                use fx::flame::FlameSound;
                let cue = |alias: &str, owner: u32, pos: Vec3| audio::cues::Cue {
                    alias: alias.to_string(),
                    source: audio::cues::Source::Entity { num: owner, pos },
                    delay_s: 0.0,
                };
                match s {
                    FlameSound::Start { owner, pos } => {
                        self.audio
                            .play(self.fs, cue("flamethrower_start", owner, pos));
                    }
                    FlameSound::Fire { owner, pos } => {
                        self.audio.hold(self.fs, owner, "flamethrower_fire", pos)
                    }
                    FlameSound::Stop { owner, pos } => {
                        self.audio.release(owner);
                        self.audio
                            .play(self.fs, cue("flamethrower_cooldown", owner, pos));
                    }
                }
            }
        }
        // UO's flame report on `cg.snap`'s player.
        let mut fdc = None;
        if p.is_uo() {
            let flame_trace = |s: Vec3, e: Vec3| {
                let w = MoveWorld::new(world, &bodies, u32::MAX);
                let t = w.box_trace(
                    s,
                    e,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    fx::client_bullet::MASK_CLIENT_SHOT,
                );
                (t.fraction, w.entity_num(&t))
            };
            let report = self.flame.report(
                time * 1000.0,
                victim,
                net.flood_window_open(),
                Some(&flame_trace),
            );
            input.in_flame = report.in_flame;
            fdc = report.fdc;
        }
        let mut fx_quads = self.fx.build_quads(cam.pos, cam_right, cam_up, time);
        fx_quads.extend(self.flame.quads(time * 1000.0, cam_right, cam_up));
        if let Some(newest) = newest {
            fx_quads.extend(head_icon::quads(
                &head_icon::Scene {
                    protocol: p,
                    entities: &newest.entities,
                    clients: &newest.clients,
                    configstrings: net.configstrings(),
                    viewer: ps_client,
                    own: client_num,
                    eye: cam.pos,
                    heads,
                    entity_pos,
                    riding: (newest.ps.field_i32(p, "eFlags") & net::flags::EF_IN_VEHICLE_V22 != 0)
                        .then(|| u32::try_from(newest.ps.field_i32(p, "viewlocked_entNum")).ok())
                        .flatten(),
                    voice: &|num| self.quick_chat.sprite(num, time),
                },
                cam_right,
                cam_up,
            ));
            fx::sim::sort_back_to_front(&mut fx_quads, cam.pos);
        }
        r.set_fx_quads(self.fs, fx_quads, time);
        // CoD2's snow (fx::cod2_cloud), tested against the map's roofs
        let clouds = self.fx.clouds(cam.pos, cam_right, cam_up, time);
        let map = self.session.map_name();
        r.set_fx_clouds(self.fs, map, &clouds, time);
        r.set_marks(
            self.fs,
            &self.fx.take_marks(time),
            shadow_marks,
            time,
            self.shell.cvar_f32("cg_marks") != 0.0,
        );
        r.set_fx_lights(&self.fx.lights(cam.pos, time));
        r.set_fog(
            net::FogParams::parse(net.configstring(net.protocol().cs.fog)),
            time as f32,
        );
        *self.fx_ms = fx_t0.elapsed().as_secs_f32() * 1000.0;
        fdc
    }
}
