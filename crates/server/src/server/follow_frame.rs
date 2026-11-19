//! The end frame's follow pass and the followers a dropped client leaves.

use super::*;

/// An archived follow copy's velocity and teleport bit, read in the field
/// table the archive was written in.
fn archived_motion(ps: &msg::PlayerState, p: &Cod1Wire) -> ([f32; 3], bool) {
    let velocity = [
        ps.field_f32(p, "velocity[0]"),
        ps.field_f32(p, "velocity[1]"),
        ps.field_f32(p, "velocity[2]"),
    ];
    let teleport = ps.field_i32(p, "eFlags") & crate::spectate::EF_TELEPORT_BIT != 0;
    (velocity, teleport)
}

/// `ClientEndFrame`'s follow half for one slot, in slot order: a playing or
/// dead client takes the own-view bit a follow copies, or is spawned where a
/// copy still in its playerstate left it, and a spectator runs
/// `SpectatorClientEndFrame` (0x40760). A forced follow's copy is taken
/// `archivetime` back, retried 50 ms younger each time it fails down to a
/// live one, and the age it settled on is written back; a forced follow that
/// finds nothing writes -1 back into `spectatorclient`, and so does a follow
/// that finds nothing to copy, which is `StopFollowing`.
pub(super) fn follow_end_frame(
    clients: &mut [Option<Client>],
    proto: &Cod1Wire,
    slot: usize,
    rt: &mut script::ScriptRuntime,
    archive: &crate::archive::Archive,
    collision: Option<&vcod_common::collision::CollisionWorld>,
    now: i32,
) {
    use crate::archive::Source;
    let Some(sim) = clients[slot].as_ref().and_then(|c| c.sim.as_ref()) else {
        return;
    };
    let session = session_of(Some(rt), slot, sim);
    fn sim_mut(clients: &mut [Option<Client>], slot: usize) -> &mut ClientSim {
        clients[slot].as_mut().and_then(|c| c.sim.as_mut()).unwrap()
    }
    match session.state {
        follow::SessionState::Playing | follow::SessionState::Dead => {
            let c = clients[slot].as_mut().unwrap();
            let cmd_angles = c.last_cmd.angles;
            let sim = c.sim.as_mut().unwrap();
            // The copy is still the playerstate, whose `clientNum` is not
            // this client's: `ClientSpawn` at its feet and yaw (0x40f82),
            // whose own think runs the client up to the frame's clock.
            if let (true, Some(copied)) = (sim.follow.on, sim.follow.copied) {
                let playing = session.state == follow::SessionState::Playing;
                let world = collision.map(vcod_common::movetrace::MoveWorld::bare);
                sim.spawn_from_copy(&copied, playing, cmd_angles, world);
                c.last_processed_st = now;
                rt.engine_client_spawn(slot, copied.origin, copied.angles[1]);
            }
            sim.own_view = true;
            sim.spectate_flags = 0;
            sim.follow = Default::default();
            return;
        }
        follow::SessionState::Intermission => {
            let sim = sim_mut(clients, slot);
            sim.own_view = false;
            sim.spectate_flags = 0;
            sim.follow = Default::default();
            return;
        }
        follow::SessionState::Spectator => {}
    }
    sim_mut(clients, slot).own_view = false;
    // 1.5's `allowSpectateTeam` rules (1.5 game.mp 0x39edd): a follow of a
    // team the mask refuses fails, and without free flight a spectator
    // following nobody cycles onto someone. A 1.1 script never sets the mask.
    let mask = session.no_spectate;
    let teams: Vec<i32> = if mask == 0 {
        Vec::new()
    } else {
        (0..clients.len()).map(|t| rt.client_team(t)).collect()
    };
    // A script's `spectatorclient` may name any number; past the slots it
    // finds nothing and is written back to -1.
    let allowed = |t: usize| {
        mask == 0
            || teams
                .get(t)
                .is_some_and(|&team| follow::can_spectate(mask, team))
    };
    let mut age = session.archive_ms;
    let mut forced = session.spectator_client;
    let mut target = sim_mut(clients, slot).follow.target;
    let mut source = Source::None;
    if forced >= 0 {
        let t = forced as usize;
        target = Some(t);
        loop {
            age = age.max(0);
            source = archive.player_state(t, &mut age, followable(clients, t));
            if !allowed(t) {
                source = Source::None;
            }
            if source != Source::None || age == 0 {
                break;
            }
            age -= 50;
        }
        if source == Source::None {
            rt.set_client_spectator_client(slot, -1);
            forced = -1;
            target = None;
        }
    }
    if target.is_none() && !follow::can_spectate(mask, follow::FREELOOK) {
        target = follow::cycle(None, 1, clients.len(), |t| {
            followable(clients, t) && allowed(t)
        });
    }
    if source == Source::None
        && let Some(t) = target
        && allowed(t)
    {
        source = archive.player_state(t, &mut age, followable(clients, t));
    }
    if age != session.archive_ms {
        rt.set_client_archive_ms(slot, age);
    }
    let copied = match (target, source) {
        (Some(t), Source::Live) => {
            let ts = clients[t].as_ref().and_then(|c| c.sim.as_ref()).unwrap();
            Some(follow::Copied {
                eye: ts.ps.view().eye.into(),
                angles: ts.view_angles(),
                origin: ts.origin(),
                velocity: ts.ps.velocity.into(),
                teleport_bit: ts.teleport_bit(),
                frame: None,
            })
        }
        (Some(_), Source::Archived { frame, view }) => {
            let (velocity, teleport_bit) = archived_motion(&view.ps, proto);
            Some(follow::Copied {
                eye: view.eye,
                angles: view.angles,
                origin: view.origin,
                velocity,
                teleport_bit,
                frame: Some(frame),
            })
        }
        _ => None,
    };
    let can = |team| follow::can_spectate(mask, team);
    match copied {
        Some(copied) => {
            let sim = sim_mut(clients, slot);
            sim.follow = follow::Follow {
                target,
                on: true,
                forced: forced >= 0,
                copied: Some(copied),
            };
            sim.spectate_flags = if forced < 0 && can(follow::FREELOOK) {
                flags::PMF_MAY_FREE
            } else {
                0
            };
        }
        None => {
            let sim = sim_mut(clients, slot);
            sim.stop_following(collision);
            sim.spectate_flags =
                if can(script::TEAM_NONE) || can(script::TEAM_AXIS) || can(script::TEAM_ALLIES) {
                    flags::PMF_MAY_CYCLE
                } else {
                    0
                };
            if session.spectator_client != -1 {
                rt.set_client_spectator_client(slot, -1);
            }
        }
    }
}

/// `ClientDisconnect`'s pass over the spectators following `gone`
/// (0x42b25..0x42b5b): each cycles on to the next client under `cycle`
/// (1.1), or stops.
pub(super) fn pass_followers_on(
    clients: &mut [Option<Client>],
    gone: usize,
    mut rt: Option<&mut script::ScriptRuntime>,
    collision: Option<&vcod_common::collision::CollisionWorld>,
    cycle: bool,
) {
    for slot in 0..clients.len() {
        let Some(sim) = clients[slot].as_ref().and_then(|c| c.sim.as_ref()) else {
            continue;
        };
        if sim.follow.target != Some(gone) {
            continue;
        }
        let session = session_of(rt.as_deref_mut(), slot, sim);
        if session.state != follow::SessionState::Spectator {
            continue;
        }
        let next = (cycle && session.spectator_client < 0)
            .then(|| follow::cycle(Some(gone), 1, clients.len(), |t| followable(clients, t)))
            .flatten();
        let sim = clients[slot].as_mut().and_then(|c| c.sim.as_mut()).unwrap();
        match next {
            Some(t) => sim.follow.target = Some(t),
            None => {
                sim.stop_following(collision);
                if let (Some(rt), true) = (rt.as_deref_mut(), session.spectator_client != -1) {
                    rt.set_client_spectator_client(slot, -1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// An archived follow copy is read in the field table it was written
    /// in: on UO `velocity[2]` is not `proneDirection`'s index (BUG-136).
    #[test]
    fn an_archived_copy_reads_its_own_protocols_fields() {
        use vcod_common::net::protocol::{PROTOCOL_V6, PROTOCOL_V22};
        for p in [&PROTOCOL_V1, &PROTOCOL_V6, &PROTOCOL_V22] {
            let mut ps = msg::PlayerState::null(p);
            let mut set = |name: &str, v: i32| {
                ps.fields[msg::PlayerState::field_index(p, name).unwrap()] = v;
            };
            set("velocity[0]", 1.0f32.to_bits() as i32);
            set("velocity[1]", 2.0f32.to_bits() as i32);
            set("velocity[2]", 3.0f32.to_bits() as i32);
            set("proneDirection", 90.0f32.to_bits() as i32);
            set("eFlags", crate::spectate::EF_TELEPORT_BIT);
            let (v, teleport) = archived_motion(&ps, p);
            assert_eq!(v, [1.0, 2.0, 3.0], "{}", p.transport.version);
            assert!(teleport);
        }
    }

    /// A spectator in slot 0 on a real netchan and two players put straight
    /// into slots 1 and 2 with no socket, under a script that does nothing
    /// but give every client an entity. The players' `sessionstate` is set
    /// to playing by hand, which is what makes them followable.
    struct FollowRig {
        sv: Server,
        nc: Netchan,
        ring: SnapshotRing,
        chain: MoveChain,
        now: Instant,
        st: i32,
        /// What the rig's client decodes its frames with.
        decode: &'static Cod1Wire,
    }

    const FOLLOW_P1: [f32; 3] = [200.0, 0.0, 0.0];

    const FOLLOW_P2: [f32; 3] = [-300.0, 100.0, 0.0];

    impl FollowRig {
        fn new() -> Self {
            Self::with_script(crate::game::script::ScriptRuntime::for_test("main() {}"))
        }

        fn with_script(rt: crate::game::script::ScriptRuntime) -> Self {
            Self::with_protocol(rt, &PROTOCOL_V1, &PROTOCOL_V1)
        }

        /// A server speaking `p`, its frames decoded through `decode`.
        fn with_protocol(
            rt: crate::game::script::ScriptRuntime,
            p: &'static Cod1Wire,
            decode: &'static Cod1Wire,
        ) -> Self {
            let now = Instant::now();
            let mut sv = Server::new(cfg(), now);
            sv.set_protocol(p);
            sv.load_world(World {
                collision: test_world(&[]),
                vis: vcod_common::bsp::Visibility::single_cluster(),
                spawn: ([0.0, 0.0, 64.0], 0.0),
                spawn_points: Vec::new(),
                hazards: Vec::new(),
                rubble: Vec::new(),
                nav_cover: Vec::new(),
            });
            install_script(&mut sv, rt);
            let nc = begun(&mut sv, now);
            let chain = MoveChain::new(sv.core.checksum_feed);
            let mut rig = FollowRig {
                sv,
                nc,
                ring: SnapshotRing::new(),
                chain,
                now,
                st: 0,
                decode,
            };
            rig.add_player(1, FOLLOW_P1, 90.0);
            rig.add_player(2, FOLLOW_P2, 180.0);
            rig.step(0);
            for slot in 1..3 {
                rig.script().set_client_state_for_test(slot, "playing");
            }
            rig.step(0);
            rig
        }

        fn script(&mut self) -> &mut crate::game::script::ScriptRuntime {
            self.sv.game.module.script.as_mut().unwrap()
        }

        fn add_player(&mut self, slot: usize, origin: [f32; 3], yaw: f32) {
            let mut c = Client::new(
                addr(6 + slot as u16),
                0x3000 + slot as u16,
                0,
                format!("\\name\\p{slot}"),
                self.now,
            );
            c.is_bot = true;
            let mut sim = ClientSim::spectator(origin, yaw, [0; 3]);
            sim.become_player(origin, yaw, [0; 3]);
            c.sim = Some(sim);
            self.sv.core.clients[slot] = Some(c);
            self.script().push_client_event(ClientEvent::Connect {
                slot,
                name: format!("p{slot}"),
            });
        }

        fn sim(&self, slot: usize) -> &ClientSim {
            self.sv.core.clients[slot]
                .as_ref()
                .unwrap()
                .sim
                .as_ref()
                .unwrap()
        }

        fn sim_mut(&mut self, slot: usize) -> &mut ClientSim {
            self.sv.core.clients[slot]
                .as_mut()
                .unwrap()
                .sim
                .as_mut()
                .unwrap()
        }

        /// One cmd from the spectator holding `buttons`, and the frame it
        /// lands in.
        fn step(&mut self, buttons: u8) -> vcod_common::net::snapshot::Snapshot {
            self.st += 50;
            self.now += std::time::Duration::from_millis(50);
            let ack = self.nc.incoming_sequence as i32;
            let cmd = UserCmd {
                server_time: self.st,
                buttons,
                ..Default::default()
            };
            let ops = self.chain.ops(ack, &[cmd]);
            let pkt = self
                .nc
                .build_out(
                    i32::from(self.sv.game.server_id),
                    ack,
                    0,
                    &ops,
                    &Huffman::new(),
                )
                .unwrap();
            self.sv.handle_packet(addr(5), &pkt, self.now);
            latest_snapshot_as(
                &mut self.sv,
                &mut self.nc,
                &mut self.ring,
                self.now,
                self.decode,
            )
        }

        /// One frame with no input, and every server command it carried, as
        /// `<reliable sequence>:<text>`, each once: a command leaves in a
        /// packet of its own and again in the snapshot's.
        fn commands(&mut self) -> Vec<String> {
            self.st += 50;
            self.now += std::time::Duration::from_millis(50);
            let huff = Huffman::new();
            self.sv.tick(self.now);
            let mut out = Vec::new();
            for (_, pkt) in self.sv.take_outgoing() {
                if let Ok(Some(m)) = self.nc.process_in(&pkt, &huff) {
                    let mut r = MsgReader::new(&m[4..], &huff);
                    while !r.is_overflowed() && r.read_byte() == msg::SVC_SERVER_COMMAND {
                        let seq = r.read_long();
                        out.push(format!("{seq}:{}", r.read_big_string()));
                    }
                }
            }
            out.sort();
            out.dedup();
            out
        }

        /// A press and the release after it; the frame of the press.
        fn press(&mut self, buttons: u8) -> vcod_common::net::snapshot::Snapshot {
            let s = self.step(buttons);
            self.step(0);
            s
        }
    }

    fn ps_i32(s: &vcod_common::net::snapshot::Snapshot, name: &str) -> i32 {
        s.ps.field_i32(&PROTOCOL_V1, name)
    }

    fn v6_rig() -> FollowRig {
        FollowRig::with_protocol(
            crate::game::script::ScriptRuntime::for_test("main() {}"),
            &PROTOCOL_V6,
            &RAW_V6,
        )
    }

    /// 1.5's spectator bits on the wire: a free spectator may cycle
    /// (0x40000, as every spectator frame of the 1.5b capture reads), a free
    /// follow may cycle and fly (0x40000 | 0x80000), and a playing client's
    /// own view is 0x20000.
    #[test]
    fn protocol_6_sends_15s_spectator_bits() {
        let mut rig = v6_rig();
        let free = rig.step(0);
        assert_eq!(ps_i32(&free, "pm_flags") & 0x1f0000, 0x40000);
        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(
            ps_i32(&s, "pm_flags") & 0x1f0000,
            0x10000 | 0x40000 | 0x80000
        );
        assert_eq!(ps_i32(&s, "speed"), 190);
    }

    /// Free flight refused (`allowSpectateTeam("freelook", false)`): a
    /// spectator following nobody is cycled onto a player, the sight edge
    /// no longer lets go, and the follow loses 0x80000.
    #[test]
    fn protocol_6_without_freelook_the_spectator_follows() {
        let mut rig = v6_rig();
        rig.script().host.client_no_spectate[0] = 0x10;
        let s = rig.step(0);
        let s = if ps_i32(&s, "clientNum") == 0 {
            rig.step(0)
        } else {
            s
        };
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x1f0000, 0x10000 | 0x40000);
        let s = rig.press(msg::BUTTON_ADS);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
    }

    /// A team the mask refuses is skipped by the cycle; with every team
    /// refused the spectator stays free and may not cycle.
    #[test]
    fn protocol_6_refused_teams_are_not_followed() {
        let mut rig = v6_rig();
        rig.script().set_client_team_for_test(1, "axis");
        rig.script().set_client_team_for_test(2, "allies");
        rig.script().host.client_no_spectate[0] = 0x2;
        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 2, "slot 1 is axis");
        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 2);
        rig.script().host.client_no_spectate[0] = 0x7;
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x1f0000, 0);
    }

    /// 1.5b's match timeout (cod15-protocol-diff.md, section 12): the
    /// game clock and every snapshot's `serverTime` stand from the frame
    /// after the call while snapshots keep coming, until the recovery runs
    /// out on `svs.time`; then the clock moves on from where it stood.
    #[test]
    fn protocol_6_match_timeout_holds_the_game_clock() {
        let mut rig = v6_rig();
        {
            let host = &mut rig.script().host;
            host.set_profile(&crate::game::profile::COD1_15);
            host.cvars.set("g_timeoutsAllowed", "1");
            host.cvars.set("g_timeoutLength", "300");
            host.cvars.set("g_timeoutRecovery", "200");
            host.timeout = crate::game::timeout::MatchTimeout::new(180_000);
        }
        rig.script().set_client_team_for_test(1, "axis");
        let called_at = rig.step(0).server_time;
        let seq = rig.sv.core.clients[1]
            .as_ref()
            .unwrap()
            .link
            .last_client_command
            + 1;
        assert!(rig.sv.client_command(1, seq, "matchtimeout".into()));
        let cmds = rig.commands();
        // The rig's own unacked `d 0` and `d 1` ride along; the timeout's
        // are the rest, in send order.
        let texts: Vec<&str> = cmds
            .iter()
            .map(|c| c.split_once(':').unwrap().1)
            .filter(|t| !t.starts_with("d 0 ") && !t.starts_with("d 1 "))
            .collect();
        assert_eq!(
            texts,
            [
                "d 1628 300",
                "d 1629 PATCH_1_5_TIMEOUT_CALLED\u{14}GAME_AXIS\u{15}",
                "e \"PATCH_1_5_TIMEOUT_CALLED_PLAYERNAME\u{15}p1\"",
            ]
        );
        // `commands` ran the first frozen frame; 300 ms of timeout and 200
        // of recovery later the clock moves again.
        let mut frozen = 1;
        loop {
            let s = rig.step(0);
            if s.server_time != called_at {
                assert_eq!(s.server_time, called_at + FRAME_MS);
                break;
            }
            frozen += 1;
            assert!(frozen < 40, "the clock never moved again");
        }
        assert_eq!(
            frozen, 11,
            "300 + 200 ms on svs.time, and the frame past it"
        );
        assert_eq!(rig.sv.configstring(1628), "");
        assert_eq!(rig.sv.game.paused_ms, frozen * FRAME_MS);
    }

    /// Retail's run (docs/research/cod11-spectator-follow.md): an attack
    /// press from free flight lands on the first playing slot after 0, and
    /// the frame is that client's playerstate with the own-view bit swapped
    /// for the follow bit. The followed client's own entity is not sent,
    /// the other one still is.
    #[test]
    fn attack_rides_the_next_playing_client_with_its_playerstate() {
        let mut rig = FollowRig::new();
        let free = rig.step(0);
        assert_eq!(ps_i32(&free, "clientNum"), 0);
        assert!(free.entities.contains_key(&1) && free.entities.contains_key(&2));

        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_type"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x10000);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), FOLLOW_P1);
        assert!(!s.entities.contains_key(&1), "the followed client's entity");
        assert!(s.entities.contains_key(&2));
    }

    #[test]
    fn attack_cycles_forward_and_melee_back() {
        let mut rig = FollowRig::new();
        assert_eq!(ps_i32(&rig.press(msg::BUTTON_ATTACK), "clientNum"), 1);
        let s = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&s, "clientNum"), 2);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), FOLLOW_P2);
        assert_eq!(ps_i32(&rig.press(msg::BUTTON_MELEE), "clientNum"), 1);
        // A held button is one press.
        rig.step(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&rig.step(msg::BUTTON_ATTACK), "clientNum"), 2);
    }

    /// The sight bit's press ends the follow, and the spectator is left
    /// where `StopFollowing` puts it off the followed eye.
    #[test]
    fn a_sight_press_ends_a_free_follow_behind_the_eye() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        let (eye, view) = (rig.sim(1).ps.view().eye, rig.sim(1).view_angles());
        let s = rig.step(msg::BUTTON_ADS);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x10000, 0);
        let (spot, _) = crate::follow::stop_spot(None, eye.into(), view);
        let o = s.ps.origin(&PROTOCOL_V1);
        assert!(
            (glam::Vec3::from(o) - glam::Vec3::from(spot)).length() < 0.5,
            "{o:?} against {spot:?}"
        );
        assert!(s.entities.contains_key(&1));
        // The release is the other edge, and there is nothing left to end.
        assert_eq!(ps_i32(&rig.step(0), "clientNum"), 0);
    }

    /// The single-client scope is tested against the frame's
    /// `ps.clientNum`, so the follower of a hit client is sent that client's
    /// own 176 and not the 174 everyone else gets.
    #[test]
    fn a_follower_is_sent_the_followed_clients_own_flesh_impact() {
        use crate::game::temp_entity::{Scope, TempEntity};
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        let te = |event, scope| TempEntity {
            event,
            parm: 0,
            surf_type: 7,
            other: 2,
            attacker: 2,
            weapon: 0,
            client_num: 0,
            scale: 0,
            origin: FOLLOW_P1,
            scope,
            quake: None,
            origin2: None,
        };
        rig.sv.test_push_temp_entity(te(176, Scope::Only(1)));
        rig.sv.test_push_temp_entity(te(174, Scope::AllBut(1)));
        let s = rig.step(0);
        let events: Vec<i32> = s
            .entities
            .values()
            .map(|e| e.field_i32(&PROTOCOL_V1, "eType") - temp_entity::ET_EVENTS)
            .collect();
        assert!(events.contains(&176), "{events:?}");
        assert!(!events.contains(&174), "{events:?}");
    }

    /// A dead client keeps its own-view bit, so the follow stays on the
    /// body; the frame it goes spectator it has lost the bit and the
    /// follower is let go behind the dead eye (the retail sd run).
    #[test]
    fn a_dead_client_is_still_followed_and_a_spectating_one_lets_go() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.script().host.client_vitals[1].dead = true;
        rig.script().set_client_state_for_test(1, "dead");
        rig.step(0);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_type"), 6);

        let (eye, view) = (rig.sim(1).ps.view().eye, rig.sim(1).view_angles());
        rig.script().set_client_state_for_test(1, "spectator");
        rig.sim_mut(1)
            .become_spectator([0.0, 0.0, 300.0], 0.0, [0; 3]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        let (spot, _) = crate::follow::stop_spot(None, eye.into(), view);
        let o = s.ps.origin(&PROTOCOL_V1);
        assert!(
            (glam::Vec3::from(o) - glam::Vec3::from(spot)).length() < 0.5,
            "{o:?} against {spot:?}"
        );
    }

    /// `player_die`'s walk (combat doc, 5.1 step 9) off a `suicide` inside
    /// the script frame: the spectator following the victim is sent the
    /// scoreboard the moment the killed callback returns, behind what that
    /// callback queued and ahead of what a thread started after the kill
    /// queues, with the rows as the callback left them. A death of a client
    /// it does not follow sends it nothing.
    #[test]
    fn a_script_death_sends_its_followers_the_scoreboard_when_the_callback_returns() {
        let rt = crate::game::script::ScriptRuntime::for_test_at(
            crate::game::script::CALLBACK_SETUP,
            r#"main() { thread killer(2); thread killer(1); }
               killer(n) {
                   while (!isdefined(level.kill) || level.kill != n) wait 0.05;
                   // Never spawned, so still G_InitGentity's "noclass".
                   players = getentarray("noclass", "classname");
                   for (i = 0; i < players.size; i++)
                       if (players[i] getentitynumber() == n)
                           victim = players[i];
                   victim suicide();
                   thread talker(victim, n);
               }
               talker(victim, n) { victim.score = 8; iprintln("talker" + n); }
               CodeCallback_PlayerKilled(a, b, c, d, e, f, g) {
                   self.score = 7;
                   iprintln("killed");
                   wait 0.05;
               }"#,
        );
        let mut rig = FollowRig::with_script(rt);
        rig.press(msg::BUTTON_ATTACK);
        let frame = |rig: &mut FollowRig, slot: i32| -> Vec<String> {
            rig.script()
                .set_level_field_for_test("kill", vcod_gsc::Value::Int(slot));
            let mut cmds: Vec<(u32, String)> = rig
                .commands()
                .into_iter()
                .map(|c| {
                    let (seq, text) = c.split_once(':').unwrap();
                    (seq.parse().unwrap(), text.to_string())
                })
                .collect();
            cmds.sort();
            cmds.into_iter().map(|(_, t)| t).collect()
        };
        let other = frame(&mut rig, 2);
        assert!(other.iter().any(|t| t.contains("talker2")), "{other:?}");
        assert!(other.iter().all(|t| !t.starts_with("b ")), "{other:?}");
        let texts = frame(&mut rig, 1);
        let at = |needle: &str| texts.iter().position(|t| t.contains(needle));
        let b = at("b ").expect("no scoreboard");
        assert_eq!(texts.iter().filter(|t| t.starts_with("b ")).count(), 1);
        assert!(at("killed").unwrap() < b, "{texts:?}");
        assert!(b < at("talker1").unwrap(), "{texts:?}");
        let cells: Vec<i64> = texts[b]
            .split(' ')
            .skip(4)
            .map(|n| n.parse().unwrap())
            .collect();
        let row = cells.chunks(5).find(|r| r[0] == 1).unwrap();
        assert_eq!(row[1], 7, "the row reads the callback's score: {texts:?}");
    }

    /// `ClientDisconnect` moves every follower of the leaving client on to
    /// the next one, and lets it go when there is none.
    #[test]
    fn a_dropped_client_passes_its_followers_on() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.sv.drop_client(1, "EXE_DISCONNECTED");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 2);
        rig.sv.drop_client(2, "EXE_DISCONNECTED");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x10000, 0);
    }

    /// 1.5's and UO's `ClientDisconnect` stops a follower outright, with
    /// another client still there to cycle to (1.5b 0x4cf5f, UO 0x55433).
    #[test]
    fn a_dropped_client_stops_its_followers_on_1_5() {
        let rt = crate::game::script::ScriptRuntime::for_test("main() {}");
        let mut rig = FollowRig::with_protocol(rt, &PROTOCOL_V6, &PROTOCOL_V6);
        assert_eq!(ps_i32(&rig.press(msg::BUTTON_ATTACK), "clientNum"), 1);
        rig.sv.drop_client(1, "EXE_DISCONNECTED");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
    }

    /// Cmds keep running as the clock nears `i32::MAX`: the hold on a cmd's
    /// clock no longer crosses its bounds there (BUG-060: it panicked past
    /// `i32::MAX - 200`).
    #[test]
    fn cmds_run_as_the_clock_nears_i32_max() {
        let mut rig = FollowRig::new();
        rig.sv.start_clock_at(i32::MAX - 300);
        rig.st = i32::MAX - 400;
        for _ in 0..5 {
            rig.step(0);
        }
        assert!(rig.sv.core.sv_time_ms > i32::MAX - 200);
        assert_eq!(
            crate::game::hold_cmd_time(i32::MAX, i32::MAX - 100),
            i32::MAX
        );
        assert_eq!(
            crate::game::hold_cmd_time(0, i32::MAX - 100),
            i32::MAX - 1100
        );
        assert_eq!(crate::game::hold_cmd_time(i32::MIN, 5000), 4000);
        assert_eq!(crate::game::hold_cmd_time(9000, 5000), 5200);
    }

    /// A script's `spectatorclient` is a forced follow: both bits on the
    /// frame, the buttons do nothing to it, and one naming a slot with
    /// nothing to follow is written back to -1.
    #[test]
    fn a_forced_follow_carries_both_bits_and_an_empty_slot_is_cleared() {
        let mut rig = FollowRig::new();
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(2));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 2);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x30000);
        assert_eq!(ps_i32(&rig.press(msg::BUTTON_ATTACK), "clientNum"), 2);
        assert_eq!(ps_i32(&rig.step(msg::BUTTON_ADS), "clientNum"), 2);

        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(5));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(
            rig.script().client_field(0, "spectatorclient").as_deref(),
            Some("-1")
        );
    }

    /// Under a 1.5 spectate mask a forced follow past the slot count finds
    /// nothing and is written back to -1 (BUG-020: it indexed the team list).
    #[test]
    fn protocol_6_a_forced_follow_past_the_slots_is_cleared() {
        let mut rig = v6_rig();
        rig.script().host.client_no_spectate[0] = 0x2;
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(64));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(
            rig.script().client_field(0, "spectatorclient").as_deref(),
            Some("-1")
        );
    }

    /// With no archive (no `setarchive(true)`) a killcam's age finds no
    /// frame, the retry lowers it to 0 and the copy is the live one; the 0 is
    /// what the stock `archivetime <= delay` check reads back.
    #[test]
    fn with_the_archive_off_a_killcam_follows_live_and_reads_back_zero() {
        let mut rig = FollowRig::new();
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(9.0));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x30000);
        assert_eq!(ps_i32(&s, "deltaTime"), 0);
        assert_eq!(
            rig.script().client_field(0, "archivetime").as_deref(),
            Some("0")
        );
    }

    /// Steps `frames` frames with the archive on, players 1 and 2 moving
    /// 10 units a frame, and returns where each stood by server time.
    fn archive_moving(rig: &mut FollowRig, frames: i32) -> BTreeMap<i32, ([f32; 3], [f32; 3])> {
        rig.script().host.archive_on = true;
        let mut trail = BTreeMap::new();
        for i in 0..frames {
            let (a, b) = ([10.0 * i as f32, 0.0, 0.0], [0.0, 10.0 * i as f32, 0.0]);
            rig.sv.test_set_client_origin(1, a);
            rig.sv.test_set_client_origin(2, b);
            let s = rig.step(0);
            trail.insert(s.server_time, (a, b));
        }
        trail
    }

    /// The killcam proper: the forced follow's copy is the followed client
    /// as the archive had it `archivetime` ago, its `deltaTime` the age, and
    /// the entity list is that frame's too, times shifted (0x808ef7c,
    /// 0x808f130).
    #[test]
    fn a_killcam_replays_the_followed_client_from_archivetime_ago() {
        let mut rig = FollowRig::new();
        let trail = archive_moving(&mut rig, 60);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(1.0));
        rig.sv.test_set_client_origin(1, [-50.0, -50.0, 0.0]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x30000);
        assert_eq!(ps_i32(&s, "deltaTime"), 1000);
        let (then_1, then_2) = trail[&(s.server_time - 1000)];
        assert_eq!(s.ps.origin(&PROTOCOL_V1), then_1);
        assert!(!s.entities.contains_key(&1));
        let e2 = &s.entities[&2];
        assert_eq!(
            e2.origin(&PROTOCOL_V1),
            then_2,
            "entity 2 as the frame had it"
        );
        let tr = e2.field_i32(&PROTOCOL_V1, "pos.trTime");
        let archived_tr = rig
            .sv
            .game
            .archive
            .frame(rig.sv.game.archive.lookup(&mut 1000).unwrap())
            .unwrap()
            .world
            .culled[&2]
            .field_i32(&PROTOCOL_V1, "pos.trTime");
        if archived_tr != 0 {
            assert_eq!(tr, archived_tr + 1000);
        }
        assert_eq!(
            rig.script().client_field(0, "archivetime").as_deref(),
            Some("1")
        );
        // The age holds: the next frame replays the next archived one.
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "deltaTime"), 1000);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), trail[&(s.server_time - 1000)].0);
    }

    /// A stock killcam asks for nine seconds; a level younger than that
    /// trims the age to what it has, which is what the script reads back.
    #[test]
    fn a_young_archive_trims_the_age_the_script_reads_back() {
        let mut rig = FollowRig::new();
        archive_moving(&mut rig, 40);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(9.0));
        let s = rig.step(0);
        let age = ps_i32(&s, "deltaTime");
        assert!((1800..=2000).contains(&age), "{age}");
        assert_eq!(
            rig.script().client_field(0, "archivetime").as_deref(),
            Some(format!("{}", age as f32 / 1000.0).as_str())
        );
    }

    /// A frame where the followed client had no view of its own (it was
    /// spectating) fails, and the age is lowered 50 ms at a time until one
    /// answers (0x407c1..0x40859).
    #[test]
    fn a_killcam_steps_younger_past_frames_the_followed_client_is_missing_from() {
        let mut rig = FollowRig::new();
        rig.script().host.archive_on = true;
        rig.script().set_client_state_for_test(1, "spectator");
        for _ in 0..40 {
            rig.step(0);
        }
        rig.script().set_client_state_for_test(1, "playing");
        for _ in 0..10 {
            rig.step(0);
        }
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(9.0));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        let age = ps_i32(&s, "deltaTime");
        assert!((450..=550).contains(&age), "{age}");
    }

    /// sd's killcam ends without the move back to `dead`: `spectatorclient`
    /// -1 and `archivetime` 0 leave the follow on the same client, live and
    /// no longer forced (the retail sd run, spectator-follow doc section 12).
    #[test]
    fn a_killcam_left_as_a_spectator_follows_the_same_client_live() {
        let mut rig = FollowRig::new();
        archive_moving(&mut rig, 60);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(1.0));
        rig.step(0);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(-1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(0.0));
        rig.sv.test_set_client_origin(1, [-50.0, -50.0, 0.0]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x10000);
        assert_eq!(ps_i32(&s, "deltaTime"), 0);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), [-50.0, -50.0, 0.0]);
    }

    /// A spectator with an `archivetime` and nobody to follow (sd's bomb
    /// camera) keeps its own view and is sent the archived frame's entities
    /// (0x808f130 reads the age for every client).
    #[test]
    fn an_archivetime_without_a_follow_replays_only_the_entities() {
        let mut rig = FollowRig::new();
        let trail = archive_moving(&mut rig, 60);
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(1.0));
        rig.sv.test_set_client_origin(2, [-50.0, -50.0, 0.0]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x10000, 0);
        assert_eq!(
            s.entities[&2].origin(&PROTOCOL_V1),
            trail[&(s.server_time - 1000)].1
        );
    }

    /// The stock killcam's end: `spectatorclient` -1, `archivetime` 0 and
    /// `sessionstate` `dead` with the replay still in the playerstate is
    /// `ClientEndFrame`'s `ClientSpawn` arm, which leaves a dead client where
    /// the copy stood.
    #[test]
    fn the_killcam_s_end_spawns_a_dead_client_where_the_copy_stood() {
        let mut rig = FollowRig::new();
        archive_moving(&mut rig, 60);
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(1.0));
        let last = rig.step(0);
        assert_eq!(ps_i32(&last, "clientNum"), 1);
        rig.script().host.client_vitals[0].dead = true;
        rig.script()
            .set_client_field_for_test(0, "spectatorclient", vcod_gsc::Value::Int(-1));
        rig.script()
            .set_client_field_for_test(0, "archivetime", vcod_gsc::Value::Float(0.0));
        rig.script().set_client_state_for_test(0, "dead");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 6);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x40000);
        // Horizontally: the rig has no floor under the copy for the spawn's
        // dead think to settle on.
        let (o, l) = (s.ps.origin(&PROTOCOL_V1), last.ps.origin(&PROTOCOL_V1));
        assert_eq!(o[..2], l[..2]);
        assert_eq!(ps_i32(&s, "deltaTime"), 0);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "pm_type"), 6);
    }

    /// Script moving a free follower to `playing` with no spawn of its own:
    /// `ClientEndFrame` finds the copy still in the playerstate and spawns
    /// the client there, alive (spectator-follow doc, 12.7).
    #[test]
    fn a_follower_script_puts_in_play_is_spawned_where_the_copy_stood() {
        let mut rig = FollowRig::new();
        let last = rig.press(msg::BUTTON_ATTACK);
        assert_eq!(ps_i32(&last, "clientNum"), 1);
        rig.script().set_client_state_for_test(0, "playing");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 0);
        assert_eq!(ps_i32(&s, "pm_flags") & 0x70000, 0x40000);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), FOLLOW_P1);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 0);
    }

    /// `StopFollowing` (0x46a28) writes the spot, the view, `clientNum` and
    /// a handful of weapon fields, and never the velocity; the rest of the
    /// copy stays in the spectator's playerstate until something writes it
    /// (the retail dm run: `health` 100 and `weapon` 9 on every free frame
    /// after the sight press).
    #[test]
    fn a_stopped_follow_keeps_the_rest_of_the_copy() {
        let mut rig = FollowRig::new();
        rig.script().host.client_vitals[1].health = 100;
        rig.script().set_client_weapon(1, 9);
        rig.sim_mut(1).ps.velocity = glam::Vec3::new(120.0, 0.0, 0.0);
        rig.sim_mut(1).ps.ducked = true;
        rig.press(msg::BUTTON_ATTACK);
        let copy = rig.step(0);
        assert_eq!((copy.ps.health(), ps_i32(&copy, "weapon")), (100, 9));
        assert_eq!(ps_i32(&copy, "pm_flags"), 0x10000 | 0x2);
        let stop = rig.step(msg::BUTTON_ADS);
        assert_eq!(ps_i32(&stop, "clientNum"), 0);
        assert_eq!(ps_i32(&stop, "pm_type"), 4);
        // `StopFollowing` takes 0x10020 off the copy's flags and leaves the
        // rest (0x46bb1), and `SetClientViewAngle` pitches the view down 15,
        // which the stop cmd's own `PM_UpdateViewAngles` keeps: retail's stop
        // frame read pitch 15.
        assert_eq!(ps_i32(&stop, "pm_flags"), 0x2);
        let pitch = stop.ps.field_f32(&PROTOCOL_V1, "viewangles[0]");
        assert!((pitch - 15.0).abs() < 0.01, "{pitch}");
        let vx = stop.ps.field_f32(&PROTOCOL_V1, "velocity[0]");
        assert!(
            vx > 0.0 && vx < 120.0,
            "the copy's velocity, one flight step on: {vx}"
        );
        // The flight writes the eye heights: retail's stop frame read 0 for
        // both where the copy read 60.
        assert_eq!(ps_i32(&stop, "viewHeightTarget"), 0);
        assert_eq!(stop.ps.field_f32(&PROTOCOL_V1, "viewHeightCurrent"), 0.0);
        for s in [stop, rig.step(msg::BUTTON_ADS), rig.step(0)] {
            assert_eq!(ps_i32(&s, "clientNum"), 0);
            assert_eq!((s.ps.health(), ps_i32(&s, "weapon")), (100, 9));
        }
    }

    /// The same stop from the end frame, the followed client gone
    /// spectator: until the follower's next cmd runs `SpectatorThink`'s
    /// free-flight arm the frame keeps the copy's `pm_type` (the retail sd
    /// run: 6, then 4).
    #[test]
    fn an_end_frame_stop_keeps_the_copy_s_pm_type_until_the_next_cmd() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.script().host.client_vitals[1].dead = true;
        rig.script().set_client_state_for_test(1, "dead");
        rig.step(0);
        rig.step(0);
        rig.script().set_client_state_for_test(1, "spectator");
        rig.sim_mut(1)
            .become_spectator([0.0, 0.0, 300.0], 0.0, [0; 3]);
        // This frame's cmd runs while the follow is still on; the stop is
        // the end frame's.
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 6);
        assert_eq!(ps_i32(&rig.step(0), "pm_type"), 4);
    }

    /// `G_RunFrame`'s end-frame loop runs in slot order, so a follower
    /// numbered below its target copies it before the target's own
    /// `ClientEndFrame` has run: what that end frame writes reaches the
    /// follower a frame late. The retail dm run: the death frame read
    /// `health` 0 and `pm_type` 0 in slot 0 and 6 in slot 2.
    #[test]
    fn a_follower_below_its_target_reads_the_end_frame_s_fields_a_frame_late() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.script().host.client_vitals[1].dead = true;
        // What a stock killed callback writes; the dead arm keys on it.
        rig.script().set_client_state_for_test(1, "dead");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(s.ps.health(), 0);
        assert_eq!(ps_i32(&s, "pm_type"), 0);
        assert_eq!(ps_i32(&rig.step(0), "pm_type"), 6);
    }

    /// `P_DamageFeedback` puts `EV_PAIN` on the ring inside the target's own
    /// end frame, so a follower below it copies the ring without it and sees
    /// the pain a frame late (spectator-follow doc, 5).
    #[test]
    fn a_follower_below_its_target_sees_its_pain_a_frame_late() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        let seq = ps_i32(&rig.step(0), "eventSequence");
        rig.script().host.client_vitals[1].health = 67;
        rig.script().host.client_vitals[1].max_health = 100;
        rig.script().host.client_sim_ops.push((
            1,
            crate::game::host::SimOp::Damaged {
                damage: 33,
                point: FOLLOW_P1,
                dir: [1.0, 0.0, 0.0],
                knockback: false,
                attacker: Some(2),
                dead_yaw: 0,
                fatal: false,
            },
        ));
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(s.ps.health(), 67);
        assert_eq!(ps_i32(&s, "eventSequence"), seq, "the pain rode early");
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "eventSequence"), seq + 1);
        assert_eq!(ps_i32(&s, &format!("events[{}]", seq & 3)), 187);
        assert_eq!(ps_i32(&s, &format!("eventParms[{}]", seq & 3)), 67);
    }

    /// `ClientSpawn` runs the spawned client's own `ClientEndFrame`
    /// (0x42a75), whose playing arm sets the own-view bit, so a follower
    /// numbered below it copies the new life on the spawn's frame rather
    /// than finding nothing and letting go (the retail dm run: slot 0 read
    /// slot 1's respawn frame).
    #[test]
    fn a_follower_below_its_target_rides_the_target_s_respawn() {
        let mut rig = FollowRig::new();
        rig.press(msg::BUTTON_ATTACK);
        rig.sim_mut(1).become_player(FOLLOW_P2, 0.0, [0; 3]);
        let s = rig.step(0);
        assert_eq!(ps_i32(&s, "clientNum"), 1);
        assert_eq!(s.ps.origin(&PROTOCOL_V1), FOLLOW_P2);
    }
}
