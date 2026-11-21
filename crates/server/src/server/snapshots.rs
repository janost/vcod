//! The world frame, snapshot build and send, and the archive.

use super::*;
use vcod_common::net::Msec;

/// [`Server::fog_cull`] off the fields, for a caller holding the script.
pub(super) fn fog_cull_of(p: &Cod1Wire, configstrings: &[String]) -> Option<f32> {
    (!p.is_v1())
        .then(|| crate::world::fog_cull(configstrings.get(p.cs.fog).map_or("", String::as_str)))
        .flatten()
}

/// A teammate's link as the compass cull reads it, built once a frame: the
/// compass pass changes nothing it is built from.
type FriendLink = (vcod_common::mapworld::LinkClusters, [f32; 3], [f32; 3]);

/// `ClientEndFrame`'s call of `G_GetNonPVSFriendlyInfo` for `slot`
/// (`crate::compass`): `None` when its session takes no end frame that
/// makes the call (spectator, intermission), else the packed teammate, its
/// slot and its ping bit, or `Some(None)` for nobody. The viewer needs a
/// team other than none or spectator; a candidate is a playing client on
/// it whose entity is not in a snapshot from the viewer's leaned eye.
/// `broadcast` is [`script::ScriptRuntime::in_snapshot_broadcast`] by slot.
#[allow(clippy::too_many_arguments)]
pub(super) fn compass_friend(
    clients: &[Option<Client>],
    slot: usize,
    sessions: &[Option<follow::SessionState>],
    teams: &[i32],
    broadcast: &[bool],
    cull: Option<(&vcod_common::bsp::Visibility, Option<f32>)>,
    time: i32,
    p: &Cod1Wire,
    links: &mut [Option<FriendLink>],
) -> Option<Option<(i32, u32, bool)>> {
    use follow::SessionState::{Dead, Playing};
    let sim = clients[slot].as_ref()?.sim.as_ref()?;
    if !matches!(sessions[slot], Some(Playing | Dead)) {
        return None;
    }
    let team = teams[slot];
    if team == script::TEAM_NONE || team == script::TEAM_SPECTATOR {
        return Some(None);
    }
    let eye: [f32; 3] = sim.ps.view().eye.into();
    let viewer = std::cell::OnceCell::new();
    let mut pinged = false;
    let found = compass::next_friend(sim.last_friend, [eye[0], eye[1]], |n| {
        let c = clients.get(n)?.as_ref()?;
        let other = c.sim.as_ref()?;
        // A higher slot's entity is as its last end frame wrote it: its own
        // has not run yet (cod11-hud-protocol.md, "Compass friendlies").
        let live = if n < slot {
            sessions[n] == Some(Playing) && other.pm_type == PmType::Normal
        } else {
            other.compass_candidate
        };
        if !live || teams[n] != team {
            return None;
        }
        // No map: nothing to cull against, so everything is in view.
        let (vis, fog) = cull?;
        if broadcast[n] {
            return None;
        }
        let link = links[n].get_or_insert_with(|| {
            let e = other.to_entity(p, n, c.last_processed_st);
            crate::world::entity_link(vis, &e, time, p)
        });
        let at = || *viewer.get_or_init(|| crate::world::Viewer::at(vis, eye));
        if crate::world::link_in_view(vis, at, link, p.is_v1(), fog) {
            return None;
        }
        pinged = other.ping;
        Some(compass::Candidate {
            at: [other.ps.origin.x, other.ps.origin.y],
            yaw: other.view_angles()[1],
        })
    });
    Some(found.map(|(info, n)| (info, n, pinged)))
}

impl Server {
    /// The snapshot cull's fog distance, squared: 1.5's and UO's, never
    /// 1.1's (`crate::world::fog_cull`).
    pub(super) fn fog_cull(&self) -> Option<f32> {
        fog_cull_of(self.game.proto, &self.core.configstrings)
    }

    /// What every snapshot of this frame takes its entities and roster
    /// from, and what `SV_ArchiveSnapshot` keeps of the frame.
    pub(super) fn build_world_frame(&mut self) -> crate::archive::WorldFrame {
        // One clientState entry per online client, rebuilt each frame; slot ==
        // index. `snapshot::write` deltas this against each client's own
        // base roster, or sends it full when that client has none.
        // The body model rides here, not on the entity: without it another
        // client is sent a player it can name but cannot draw
        // (`docs/research/clientstate-wire-format.md`).
        // The team rides here too: it is what the receiving client colours
        // names and tells friend from foe with, and script owns it through
        // `.sessionteam`. Without a script there is nothing to ask, so the
        // slot keeps the value retail's `ClientConnect` leaves in the field.
        type Roster = (i32, Vec<(i32, i32)>, i32);
        let per_slot: Vec<Roster> = match self.game.module.script.as_mut() {
            Some(rt) => (0..self.core.clients.len())
                .map(|slot| {
                    (
                        rt.client_model_index(slot),
                        rt.client_attachments(slot),
                        rt.client_team(slot),
                    )
                })
                .collect(),
            None => vec![(0, Vec::new(), script::TEAM_SPECTATOR); self.core.clients.len()],
        };
        let roster: BTreeMap<u32, msg::ClientState> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let c = c.as_ref()?;
                let (model, attachments, team) = &per_slot[i];
                let mut cs = msg::ClientState::named(self.game.proto, 0, *team, &c.name);
                if let Some(idx) = msg::ClientState::field_index(self.game.proto, "modelindex") {
                    cs.fields[idx] = *model;
                }
                for (n, (am, at)) in attachments.iter().enumerate() {
                    for (name, v) in [
                        (format!("attachModelIndex[{n}]"), am),
                        (format!("attachTagIndex[{n}]"), at),
                    ] {
                        if let Some(idx) = msg::ClientState::field_index(self.game.proto, &name) {
                            cs.fields[idx] = *v;
                        }
                    }
                }
                Some((i as u32, cs))
            })
            .collect();
        // The map's own entities, plus whatever --test-entities adds: the
        // scripted ones exist to drive the wire path and have no gameplay
        // meaning, so they sit beside the object table's rather than
        // replacing it.
        let mut entities: BTreeMap<u32, msg::EntityState> = self
            .game
            .module
            .script
            .as_mut()
            .map_or_else(BTreeMap::new, |rt| rt.packet_entities(self.game.proto));
        if let Some(te) = self.game.module.test_entities.as_ref() {
            entities.extend(te.at(self.game.proto, self.core.sv_time_ms));
        }

        // One entity per client with a sim, so a client can be told what the
        // others are doing. The receiver's own is dropped below: retail sends
        // a client no entity for itself.
        let client_entities: BTreeMap<u32, msg::EntityState> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let sim = c.as_ref()?.sim.as_ref()?;
                // A spectator, an intermission camera and a dead player are
                // each unlinked or `SVF_NOCLIENT`, so nobody is sent an
                // entity for one ([`crate::spectate::ClientSim::linked`]).
                // The state this frame's own end frame copied, which a
                // higher slot's turret round after it changes only in the
                // playerstate: its knockback, and a death, which still goes
                // out that frame, `player_die`'s relink as a corpse clearing
                // `solid` (combat doc 5.1 step 13, 5.4; turrets doc 13.1).
                let copied = sim
                    .end_frame_entity
                    .as_ref()
                    .filter(|(t, _)| *t == self.core.sv_time_ms)
                    .map(|(_, e)| e.clone());
                if sim.linked() {
                    let mut e = copied.unwrap_or_else(|| {
                        sim.to_entity(self.game.proto, i, c.as_ref().unwrap().last_processed_st)
                    });
                    // Killed after its end frame copied it, its session
                    // still `playing` there: the corpse relink still lands.
                    if sim.dead
                        && let Some(f) = msg::EntityState::field_index(self.game.proto, "solid")
                    {
                        e.fields[f] = sim.linked_solid;
                    }
                    Some((i as u32, e))
                } else if sim.dead {
                    let mut e = copied?;
                    if let Some(f) = msg::EntityState::field_index(self.game.proto, "solid") {
                        e.fields[f] = 0;
                    }
                    Some((i as u32, e))
                } else {
                    None
                }
            })
            .collect();

        // A body born since the last build is re-read from its source
        // client's sim once, here: the script clones the player before it
        // raises the death animation, so the state the clone copied is a
        // frame stale (`crate::game::bodies`). Then the queue's corpses join the map's
        // entities; they are culled per client like anything else.
        let proto = self.game.proto;
        let collision = self.game.world.as_ref().map(|w| &w.collision);
        let clients = &self.core.clients;
        if let Some(rt) = self.game.module.script.as_mut() {
            // Straight off the sim, not `client_entities`: the source is
            // dead by now and has no entity there.
            rt.bodies_mut().refresh_newborn(
                |slot| {
                    let c = clients.get(slot)?.as_ref()?;
                    Some(c.sim.as_ref()?.to_entity(proto, slot, c.last_processed_st))
                },
                collision,
                proto,
            );
            entities.extend(rt.body_entities(proto));
        }

        // Every live temp entity, at the number `G_TempEntity` took for it,
        // on every snapshot until the entity pass frees it.
        let (temps, temp_states): (Vec<temp_entity::Scope>, Vec<(u32, msg::EntityState)>) = self
            .game
            .module
            .script
            .as_ref()
            .map(|rt| {
                rt.temp_entities()
                    .map(|(n, te)| (te.scope, (n, temp_entity::build(te, n, self.game.proto))))
                    .unzip()
            })
            .unwrap_or_default();

        // What every snapshot this frame takes its entities from, and what
        // the archive keeps of the frame. A missile carries `SVF_BROADCAST`
        // (combat doc, 11.1), so it skips the cull the way a broadcast temp
        // entity does.
        // So does anything `playSound` touched, a player included.
        let entity = |e| crate::archive::FrameEntity::new(e, proto);
        let mut culled: BTreeMap<u32, crate::archive::FrameEntity> =
            entities.into_iter().map(|(n, e)| (n, entity(e))).collect();
        culled.extend(client_entities.into_iter().map(|(n, e)| (n, entity(e))));
        let mut broadcast: BTreeMap<u32, crate::archive::FrameEntity> = self
            .game
            .module
            .script
            .as_ref()
            .map_or_else(BTreeMap::new, |rt| {
                rt.missiles()
                    .entities(self.game.proto)
                    .map(|(n, e)| (n, entity(e)))
                    .collect()
            });
        if let Some(rt) = self.game.module.script.as_ref() {
            for n in rt.broadcast_entities(self.core.sv_time_ms) {
                if let Some(e) = culled.remove(&n) {
                    broadcast.insert(n, e);
                }
            }
        }
        // A tank's driver is sent to no one unless the broadcast tests
        // ahead of the skip took him (coduo_lnxded 0x8096d6b..0x8096db8:
        // `eType` 1, `eFlags` 0x100000, his `r.ownerNum`'s `surfType` 2,
        // his own `surfType & 7` 1; coduo-diff.md 9).
        for (slot, c) in self.core.clients.iter().enumerate() {
            let seat = c.as_ref().and_then(|c| c.sim.as_ref()).and_then(|s| s.seat);
            if seat.is_some_and(|s| crate::game::vehicle::seat::hidden_rider(s.kind, s.pos)) {
                culled.remove(&(slot as u32));
            }
        }
        crate::archive::WorldFrame {
            culled,
            temps: temps
                .into_iter()
                .zip(temp_states)
                .map(|(scope, (_, e))| (scope, entity(e)))
                .collect(),
            broadcast,
            roster: Rc::new(roster),
            time: self.core.sv_time_ms,
            fog: self.fog_cull(),
            links: self
                .game
                .module
                .script
                .as_ref()
                .map(|rt| crate::game::mover::door::links(&rt.host))
                .unwrap_or_default(),
        }
    }

    /// `SV_SendClientMessages`' head ([`crate::engine::send_messages`]):
    /// every entity state built once from the world the moves and the
    /// script frame left, to be culled and written per client, and each
    /// client's follow frame and archive source. `moved` and `wall` are
    /// `--trace`'s: what each slot replayed, the wall gap since the last
    /// tick and the frames this tick ran, so a catch-up tick's gap reads
    /// against them.
    pub(super) fn snapshots_begin(
        &mut self,
        moved: Vec<MoveSummary>,
        wall: (Option<f32>, u32),
    ) -> Cod1Snapshots {
        let _p = crate::tick_profile::phase("messages/begin");
        let live = self.build_world_frame();
        // The game clock: what 1.5b and UO compare `r.broadcastTime` with
        // (cod15-protocol-diff.md 15), which a match timeout stands still.
        let now = self.core.sv_time_ms;
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.expire_broadcast_times(now);
        }
        let teams: Vec<i32> = (0..self.core.clients.len())
            .map(|slot| {
                self.game
                    .module
                    .script
                    .as_mut()
                    .map_or(script::TEAM_SPECTATOR, |rt| rt.client_team(slot))
            })
            .collect();

        // A follower's frame is the followed client's: its playerstate with
        // the follow flags patched in, its eye for the cull and its number
        // for the single-client scopes (`SpectatorClientEndFrame` 0x40896,
        // `SV_BuildClientSnapshot` 0x808f25f). A copy the end frame took out
        // of the archive is that frame's, its times shifted by the age
        // (0x808ef7c). Built up front, since the loop below holds each slot
        // mutably.
        let now = self.core.sv_time_ms;
        let follow_frames: Vec<Option<FollowFrame>> = (0..self.core.clients.len())
            .map(|slot| {
                let sim = self.core.clients[slot].as_ref()?.sim.as_ref()?;
                let t = sim.follow.target.filter(|_| sim.follow.on)?;
                let own = sim.to_wire(self.game.proto, slot as i32, 0);
                let ef = msg::PlayerState::field_index(self.game.proto, "eFlags").unwrap();
                let (mut ps, eye, replay) = match sim.follow.copied.and_then(|c| c.frame) {
                    Some(index) => {
                        let f = self.game.archive.frame(index)?;
                        let view = f.clients.get(t)?.as_ref()?;
                        let ps = crate::archive::replayed_ps(view, now - f.time, self.game.proto);
                        (ps, view.eye, true)
                    }
                    None => {
                        let tc = self.core.clients.get(t)?.as_ref()?;
                        let ts = tc.sim.as_ref()?;
                        let mut ps = ts.to_wire(self.game.proto, t as i32, tc.last_processed_st);
                        // The end-frame loop runs in slot order.
                        if let (true, Some(last)) = (slot < t, ts.end_frame_wire.as_ref()) {
                            follow::before_end_frame(&mut ps, last, self.game.proto);
                        }
                        // `P_DamageFeedback`'s `EV_PAIN` is that end frame's too.
                        if let (true, Some(ring)) = (slot < t, ts.ring_before_pain) {
                            ring.write(&mut |name, v| {
                                let i =
                                    msg::PlayerState::field_index(self.game.proto, name).unwrap();
                                ps.fields[i] = v;
                            });
                        }
                        (ps, ts.eye_origin(), false)
                    }
                };
                follow::patch_wire(&mut ps, self.game.proto, sim.follow.forced, own.fields[ef]);
                Some(FollowFrame {
                    target: t,
                    ps,
                    eye,
                    replay,
                })
            })
            .collect();

        // What a follow leaves in the spectator's playerstate when it stops,
        // and each followable client's frame as this end frame left it.
        for (slot, f) in follow_frames.iter().enumerate() {
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            let command_time = c.last_processed_st;
            let Some(sim) = c.sim.as_mut() else {
                continue;
            };
            if let Some(f) = f {
                sim.follow_wire = Some(f.ps.clone());
            }
            sim.end_frame_wire = sim
                .own_view
                .then(|| sim.to_wire(self.game.proto, slot as i32, command_time));
            sim.ring_before_pain = None;
        }

        // `SV_BuildClientSnapshot` reads each client's `archivetime` again and
        // takes the entities and the roster from the frame it names, times
        // shifted by the age, whether or not the client follows anyone
        // (0x808f1ab..0x808f211); the trim is written back as it is there.
        let sources: Vec<Option<(i32, i32)>> = (0..self.core.clients.len())
            .map(|slot| {
                let rt = self.game.module.script.as_mut()?;
                let asked = rt.client_archive_ms(slot);
                let mut age = asked;
                let index = self.game.archive.lookup(&mut age);
                if age != asked {
                    rt.set_client_archive_ms(slot, age);
                }
                let f = self.game.archive.frame(index?)?;
                Some((index?, now - f.time))
            })
            .collect();

        Cod1Snapshots {
            live,
            teams,
            follow_frames,
            sources,
            open: Box::new(self.download_opener()),
            www: self.www_config(),
            max_rate: self.live_int_cvar("sv_maxRate"),
            moved,
            wall,
        }
    }

    /// Whether `slot` takes a turn in `SV_SendClientMessages` (0x809045c):
    /// a client with a socket, in the world or not, or a zombie, once its
    /// `nextSnapshotTime` came. A bot has no socket to write to; its slot
    /// still counts toward the roster and entity lists the real clients
    /// are sent.
    pub(super) fn message_due(&self, slot: usize) -> bool {
        let svs_ms = self.svs_time_ms();
        match (&self.core.clients[slot], &self.game.zombies[slot]) {
            (Some(c), _) => !c.is_bot && svs_ms >= c.link.next_message_ms,
            (None, Some(z)) => svs_ms >= z.link.next_message_ms,
            (None, None) => false,
        }
    }

    /// A turn with fragments left sends the next one instead of a
    /// snapshot ([`Client::next_fragment`]). A zombie's message goes out
    /// whole on its turn.
    pub(super) fn next_fragment(&mut self, slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
        let svs_ms = self.svs_time_ms();
        let max_rate = self.live_int_cvar("sv_maxRate");
        let c = self.core.clients[slot].as_mut()?;
        let rate = c.send_rate(self.game.dedicated, max_rate);
        c.next_fragment(svs_ms, rate).map(|pkt| (c.link.addr, pkt))
    }

    /// `SV_SendClientSnapshot` (0x808f844), which serves every state: a
    /// zombie's message ([`Self::send_zombie`]), a client not in the
    /// world's ([`Self::send_unentered`]), or for a client in the world a
    /// delta against the frame it last acked when one is still in its
    /// ring, uncompressed otherwise, culled to what it can see, then its
    /// download's blocks.
    pub(super) fn send_client_snapshot(&mut self, slot: usize, snaps: &Cod1Snapshots) {
        match &self.core.clients[slot] {
            None => return self.send_zombie(slot, snaps.max_rate),
            Some(c) if c.sim.is_none() => return self.send_unentered(slot, snaps),
            Some(_) => {}
        }
        let _p = crate::tick_profile::phase("messages/snapshot");
        let svs_ms = self.svs_time_ms();
        let collision_vis = self.game.world.as_ref().map(|w| &w.vis);
        let Cod1Snapshots {
            live,
            teams,
            follow_frames,
            sources,
            open,
            www,
            max_rate,
            moved,
            wall,
        } = snaps;
        let follow_frame = follow_frames[slot].as_ref();
        {
            let Some(c) = self.core.clients[slot].as_mut() else {
                return;
            };
            let rate = c.send_rate(self.game.dedicated, *max_rate);
            let Some(sim) = c.sim.as_ref() else {
                return;
            };
            // Exactly the serverTime of the last cmd the sim consumed, and
            // nothing else: the client replays everything past it, so a
            // commandTime we never simulated drops that slice of its input
            // and its prediction judders (docs/protocol-1.1.md).
            c.link.was_active = true;
            let command_time = c.last_processed_st;
            let message_num = c.link.netchan.outgoing_sequence;
            // The frame's `ps.clientNum`, which the single-client flags test
            // against and whose entity the frame leaves out.
            let client_num = follow_frame.map_or(slot, |f| f.target);

            // Retail sends a client only what its own position can see, so
            // the list is per client rather than one list cloned into every
            // frame (docs/protocol-1.1.md, "Which entities a client is sent").
            let eye = follow_frame.map_or_else(|| sim.eye_origin(), |f| f.eye);
            let (world, shift) = match sources[slot] {
                Some((index, shift)) => (&self.game.archive.frame(index).unwrap().world, shift),
                None => (live, 0),
            };
            let cull = crate::tick_profile::phase("messages/snapshot/cull");
            let visible =
                world.entities_for(client_num, eye, collision_vis, shift, self.game.proto);
            let roster = world.roster.clone();
            drop(cull);

            let mut ps = match follow_frame {
                Some(f) => f.ps.clone(),
                None => sim.to_wire(self.game.proto, client_num as i32, command_time),
            };
            let pm = msg::PlayerState::field_index(self.game.proto, "pm_flags").unwrap();
            ps.fields[pm] |= sim.spectate_flags;
            // The script's HUD elements, filtered for this client the way
            // `HudElem_UpdateClient` filters them; retail rebuilds both
            // arrays into the playerstate once per client per frame, so
            // they are read here rather than carried on the sim. A follower
            // keeps the followed client's archived half and objectives, which
            // ride the copied playerstate, and gets its own unarchived half.
            let team_of = |s: usize| teams.get(s).copied().unwrap_or(script::TEAM_SPECTATOR);
            let team = team_of(slot);
            if let Some(rt) = self.game.module.script.as_mut() {
                let (archived, current) = rt.hud_elems(slot, team);
                ps.arrays.hud_current = current;
                match follow_frame {
                    None => {
                        ps.arrays.hud_archived = archived;
                        ps.arrays.objectives = rt.objectives_for(slot, team);
                    }
                    Some(f) if !f.replay => {
                        let t = client_num;
                        ps.arrays.hud_archived = rt.hud_elems(t, team_of(t)).0;
                        ps.arrays.objectives = rt.objectives_for(t, team_of(t));
                    }
                    Some(_) => {}
                }
            }
            crate::game::wire::ps_to_protocol(self.game.proto, &mut ps);
            // `ClientSpawn`'s memset ran after the frame's HUD update
            // (the retail killcam's end frame reads both arrays empty).
            if sim.hud_cleared {
                ps.arrays.hud_archived.clear();
                ps.arrays.hud_current.clear();
            }
            let frame = snapshot::SentSnapshot {
                server_time: Msec::from_raw(self.core.sv_time_ms),
                message_num,
                delta_num: -1,
                snap_flags: self.game.snap_flag_server_bit | u32::from(c.link.rate_delayed),
                ps,
                entities: visible,
                clients: roster,
                valid: true,
            };

            // The base is the frame the client's last move acked, unless it
            // asked for none, if it is still in the ring and close enough for
            // the byte-wide deltaNum offset.
            // Safe at a full SV_PACKET_BACKUP depth (no margin, unlike
            // retail's SV_WriteSnapshotToClient, which keeps PACKET_BACKUP -
            // 3) only because both this ring and the client's own read a slot
            // before either side can overwrite it.
            let base = u32::try_from(c.delta_message)
                .ok()
                .and_then(|n| c.sent_frame(n))
                .filter(|b| {
                    let back = message_num.saturating_sub(b.message_num);
                    (1..=255).contains(&back)
                });

            let _w = crate::tick_profile::phase("messages/snapshot/write");
            let mut w = MsgWriter::new(&self.core.huff);
            write_pending_commands(&mut w, &c.link.netchan, c.link.reliable_ack);
            self.game.proto.write_snapshot(
                &mut w,
                base,
                &frame,
                frame.head(base),
                &self.game.baselines,
            );
            // The base is read in place out of the ring; the frame files
            // into it once the write is done.
            let base_num = base.map(|b| b.message_num);
            c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
            c.record_frame(frame);
            write_download(c, &mut w, svs_ms, rate, &open, www.as_ref());

            let ops = w.into_ops();

            if self.game.cfg.trace {
                // The prediction contract: a client replays every usercmd
                // newer than ps.commandTime, so `lead` at or below zero means
                // we claim to have simulated past our own frame clock and the
                // client has nothing left to replay. Retail's captures run a
                // lead of 0..34 ms, never negative (examples/snapshot_timing).
                let lead = self.core.sv_time_ms - command_time;
                let queued = c.pending.len();
                let m = moved.get(slot).copied().unwrap_or_default();
                let processed = m.processed;
                let span = match (m.first_cmd_st, m.last_cmd_st) {
                    (Some(a), Some(b)) => format!("{a}..{b}"),
                    _ => "-".into(),
                };
                let ack_behind = message_num as i64 - i64::from(c.message_ack);
                let base_desc = match base_num {
                    Some(b) => format!("d{}", message_num - b),
                    None => "uncompressed".into(),
                };
                // The walker's own state, for chasing a prediction error a
                // client reports against a headless run that cannot see it.
                let walk = c.sim.as_ref().map_or(String::new(), |s| {
                    format!(
                        " z {:.2} vz {:.1} ground {} ads {} frac {:.3}",
                        s.ps.origin.z,
                        s.ps.velocity.z,
                        u8::from(s.ps.on_ground),
                        u8::from(s.ps.ads_active),
                        s.ps.weapon_pos_frac
                    )
                });
                log::info!(
                    "trace c{slot} msg {message_num} wall {} frames {} sv {} ct {} lead {} \
cmds {processed} span {span} queued {queued} ack {} behind {ack_behind} {base_desc} {} B{walk}",
                    wall.0.map_or("-".into(), |w| format!("{w:.1}")),
                    wall.1,
                    self.core.sv_time_ms,
                    command_time,
                    lead,
                    c.message_ack,
                    ops.len(),
                );
            }
            c.stamp_sent(message_num, svs_ms);
            let sent = c
                .link
                .netchan
                .transmit(c.link.last_client_command, &ops, &self.core.huff);
            let msec = c.snapshot_msec();
            c.pace(svs_ms, sent.iter().map(Vec::len).sum(), rate, msec);
            if let Some(pkt) = c.first_packet(sent) {
                self.core.outbox.push((c.link.addr, pkt));
            }
        }
    }

    /// After every client's turn: the HUD a spawn cleared is back, and the
    /// frame the snapshots were built from goes into the archive.
    pub(super) fn snapshots_end(&mut self, snaps: Cod1Snapshots) {
        for c in self.core.clients.iter_mut().flatten() {
            if let Some(sim) = c.sim.as_mut() {
                sim.hud_cleared = false;
            }
        }
        self.archive_frame(snaps.live);
    }

    /// `SV_ArchiveSnapshot` (`cod_lnxded` 0x808fb84), after the frame's
    /// snapshots: the entities and roster they were built from, and every
    /// client's own view as `GetFollowPlayerState` would answer it, which a
    /// spectating or intermission client has none of.
    pub(super) fn archive_frame(&mut self, world: crate::archive::WorldFrame) {
        if !self.game.archive.is_on() {
            return;
        }
        let clients = (0..self.core.clients.len())
            .map(|slot| {
                let c = self.core.clients[slot].as_ref()?;
                let sim = c.sim.as_ref().filter(|s| s.own_view)?;
                let mut ps = sim.to_wire(self.game.proto, slot as i32, c.last_processed_st);
                if let Some(rt) = self.game.module.script.as_mut() {
                    let team = rt.client_team(slot);
                    ps.arrays.hud_archived = rt.hud_elems(slot, team).0;
                    ps.arrays.objectives = rt.objectives_for(slot, team);
                }
                Some(Rc::new(crate::archive::ArchivedView {
                    ps,
                    eye: sim.ps.view().eye.into(),
                    angles: sim.view_angles(),
                    origin: sim.origin(),
                }))
            })
            .collect();
        self.game.archive.push(crate::archive::Frame {
            time: self.core.sv_time_ms,
            world,
            clients,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::*;

    /// Every bot's cmds of a tick go in as one packet, so `replay_moves`
    /// poses the bodies their rounds meet under one DObj stamp, as retail's
    /// single bump ahead of `SV_Frame`'s bot pass does (movers doc 15).
    #[test]
    fn every_bots_cmds_of_a_tick_share_one_packet() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let bsp_path = fs.resolve_map("mp_carentan").expect("map in the paks");
        let bsp = crate::world::World::from_bsp(
            &vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap(),
            Some(&fs),
        );
        let mut now = Instant::now();
        let mut sv = Server::new(ServerConfig { bots: 3, ..cfg() }, now);
        sv.load_world(bsp);
        sv.load_scripts(Rc::new(fs)).expect("load the scripts");
        for _ in 0..100 {
            now += std::time::Duration::from_millis(FRAME_MS as u64);
            sv.tick(now);
        }
        let before = sv.game.packet_seq;
        crate::nav::with_rules(sv.move_rules(), || sv.step_bots());
        let packets: Vec<u64> = sv
            .core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.is_bot)
            .flat_map(|c| c.pending.iter().map(|q| q.packet))
            .collect();
        assert!(packets.len() >= 3, "every bot queued a cmd: {packets:?}");
        assert!(packets.iter().all(|&p| p == before + 1), "{packets:?}");
    }

    /// A tank driver goes to no one, but the broadcast tests run ahead of
    /// that skip, so a sound on him sends him to everyone while it lasts
    /// (coduo_lnxded 0x8096d0a..0x8096db8, coduo-diff.md 9).
    #[test]
    fn a_tank_driver_is_sent_only_while_broadcast() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        install_script(
            &mut sv,
            ScriptRuntime::for_test_at(
                CALLBACK_SETUP,
                "main() {}\n\
                 CodeCallback_PlayerConnect() { self waittill(\"begin\"); \
                   self.sessionstate = \"playing\"; \
                   self spawn((0, 0, 64), (0, 0, 0)); }\n",
            ),
        );
        begun(&mut sv, now);
        for ms in (0..=200).step_by(50) {
            sv.tick(now + Duration::from_millis(ms));
        }
        let sent = |sv: &mut Server| {
            let f = sv.build_world_frame();
            (f.culled.contains_key(&0), f.broadcast.contains_key(&0))
        };
        assert_eq!(sent(&mut sv), (true, false), "on foot");
        let sim = sv.core.clients[0].as_mut().unwrap().sim.as_mut().unwrap();
        sim.seat = Some(crate::spectate::Seat {
            vehicle: 100,
            pos: 1,
            kind: crate::game::vehicle::info::KIND_TANK,
            motion: 1,
            angles: [0.0; 3],
            hull_view: false,
            overheated: false,
            link_offset: [0.0; 3],
        });
        assert_eq!(sent(&mut sv), (false, false), "driving");
        let rt = sv.game.module.script.as_mut().unwrap();
        let id = rt.host.ents.handle(0).unwrap();
        rt.host.ents.get_mut(id).unwrap().broadcast_time = -1;
        assert_eq!(sent(&mut sv), (false, true), "driving under a loop sound");
    }

    /// `r.broadcastTime` is held to the game clock, not `svs.time`: through
    /// a 1.5 match timeout, the game clock standing, a sound's 300 ms
    /// never pass (1.5b 0x8095d98, UO 0x8096d25; cod15-protocol-diff.md 15).
    #[test]
    fn a_broadcast_time_waits_on_the_game_clock() {
        use crate::game::script::{CALLBACK_SETUP, ScriptRuntime};
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.set_protocol(&PROTOCOL_V6);
        install_script(
            &mut sv,
            ScriptRuntime::for_test_at(
                CALLBACK_SETUP,
                "main() {}\n\
                 CodeCallback_PlayerConnect() { self waittill(\"begin\"); \
                   self.sessionstate = \"playing\"; \
                   self spawn((0, 0, 64), (0, 0, 0)); }\n",
            ),
        );
        begun(&mut sv, now);
        for ms in (0..=200).step_by(50) {
            sv.tick(now + Duration::from_millis(ms));
        }
        let stamp = sv.core.sv_time_ms + 300;
        let rt = sv.game.module.script.as_mut().unwrap();
        let id = rt.host.ents.handle(0).unwrap();
        rt.host.ents.get_mut(id).unwrap().broadcast_time = stamp;
        // A second of timeout: the network clock ran on, the game's stood.
        sv.game.paused_ms += 1000;
        let f = sv.build_world_frame();
        assert!(f.broadcast.contains_key(&0), "still broadcast");
        sv.snapshots_begin(Vec::new(), (None, 0));
        let rt = sv.game.module.script.as_ref().unwrap();
        assert_eq!(rt.host.ents.get(id).unwrap().broadcast_time, stamp);
    }

    #[test]
    fn a_client_in_the_world_receives_snapshots_and_flies_forward() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // A flat floor to fly over; without a world the sim freezes.
        sv.load_world(World {
            collision: test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
            rubble: Vec::new(),
            nav_cover: Vec::new(),
            prop_areas: None,
        });
        let mut nc = begun(&mut sv, now);

        let mut ring = SnapshotRing::new();
        let s = latest_snapshot(&mut sv, &mut nc, &mut ring, now);
        assert!(s.valid && s.delta_num == -1);
        let p = &PROTOCOL_V1;
        assert_eq!(s.ps.field_i32(p, "pm_type"), 4);
        assert_eq!(s.clients[&0].name(p), "vcod");

        // Forward nudge; yaw 0 faces +X.
        let later = now + std::time::Duration::from_millis(50);
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.core.checksum_feed,
                    ack,
                    UserCmd {
                        server_time: 50,
                        forward: 127,
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, later);
        let moved = s2.ps.origin(p)[0] - s.ps.origin(p)[0];
        assert!(moved > 1.0, "should have flown +X, dx {moved}");
    }

    /// A script's `playSound` on a player reaches that client's own event
    /// ring, and through it the playerstate: the builtin only queues a
    /// `SimOp`, so the drain after `run_frame` is what completes the path.
    /// `_minefields.gsc`'s warning click is exactly this call.
    #[test]
    fn a_script_playsound_on_a_player_reaches_that_client_s_ring() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // The clients never spawn, so they are still G_InitGentity's "noclass".
        install_script(
            &mut sv,
            crate::game::script::ScriptRuntime::for_test(
                "main() { \
                   while (1) { \
                     wait 0.05; \
                     players = getentarray(\"noclass\", \"classname\"); \
                     if (players.size > 0) { \
                       players[0] playsound(\"minefield_click\"); \
                       return; \
                     } \
                   } \
                 }",
            ),
        );
        let _nc = begun(&mut sv, now);
        for _ in 0..4 {
            sv.tick(now);
        }
        let ring = sv.core.clients[0]
            .as_ref()
            .and_then(|c| c.sim.as_ref())
            .expect("slot 0 has a sim")
            .ring;
        assert_eq!(ring.seq, 1, "one event, the slot written below it");
        assert_eq!(ring.events[0], 172, "EV_SOUND_ALIAS");
        let alias = ring.parms[0] as usize + 524;
        assert_eq!(sv.configstring(alias), "minefield_click");
    }

    /// `SV_SendClientMessages` (0x809045c) is one pass in slot order
    /// whatever each slot's state: a client not in the world at slot 0,
    /// or a zombie there, is sent its message before slot 1's snapshot
    /// (cod11-server-handshake.md, "The order of a frame's messages").
    #[test]
    fn a_frames_messages_go_out_in_slot_order() {
        let t = Instant::now();
        let low = addr(5);
        // Another host: the netchan finds a client by ip and qport.
        let high = SocketAddr::from(([127, 0, 0, 2], 5));
        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "secret");
        connected(&mut sv, low, t);
        active_at(&mut sv, high, t);
        sv.take_outgoing();
        let order = |sv: &mut Server, at| {
            sv.tick(at);
            let out = sv.take_outgoing();
            out.iter().map(|(to, p)| (*to, p.len())).collect::<Vec<_>>()
        };
        let sent = order(&mut sv, t);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], (low, 9), "slot 0's bare message first: {sent:?}");
        assert_eq!(sent[1].0, high);

        let mut sv = Server::new(cfg(), t);
        sv.set_cvar("rconpassword", "secret");
        active_at(&mut sv, low, t);
        active_at(&mut sv, high, t);
        sv.take_outgoing();
        rcon(&mut sv, "secret clientkick 0", t);
        let sent = order(&mut sv, t);
        assert!(sv.game.zombies[0].is_some());
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0].0, low, "the zombie's message first: {sent:?}");
        assert_eq!(sent[1].0, high);
    }
}
