//! CoD2 1.3's voice relay (docs/research/cod2-voice.md 3): the client's
//! out-of-band `v` packets in, each frame queued for the clients
//! `G_BroadcastVoice`'s rules let hear it (`crate::voice`, CoD4's without
//! Eavesdrop), and each client's queue out of band after its snapshot.
//! Addresses are `cod2_lnxded`'s.

use std::net::SocketAddr;

use vcod_cod2::net::voice as wire;

use super::{Cod2Server, State};
use crate::voice::{ClientVoice, Rules, Voicer, hears, session_of, team_of};

impl Cod2Server {
    /// `SV_VoicePacket` (0x8094b56): the slot whose address and qport
    /// match, its port taken when only that changed; a zombie's packet
    /// goes on to the next slot. A match's `lastPacketTime` is refreshed,
    /// then a client short of `CS_ACTIVE` goes to `SV_PreGameUserVoice`,
    /// an active one to `SV_UserVoice`, each of which returns at once with
    /// `sv_voice` off. The qport is read as a signed short and compared
    /// unmasked, so a client on a qport of 0x8000 or more matches no slot
    /// (cod2-voice.md 3, VERIFIED live).
    pub(super) fn voice_packet(&mut self, from: SocketAddr, body: &[u8]) {
        let Some(qport) = body.get(..2).map(|q| i16::from_le_bytes([q[0], q[1]])) else {
            return;
        };
        let Some(slot) = self.core.clients.iter().position(|c| {
            c.as_ref().is_some_and(|c| {
                c.state != State::Zombie
                    && c.link.addr.ip() == from.ip()
                    && i32::from(c.link.netchan.qport) == i32::from(qport)
            })
        }) else {
            return;
        };
        let svs = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        if c.link.addr.port() != from.port() {
            log::info!("SV_ReadPackets: fixing up a translated port");
            c.link.addr = from;
        }
        c.link.last_packet_ms = svs;
        let active = c.state == State::Active;
        if self.int("sv_voice") == 0 {
            return;
        }
        let read = match active {
            true => wire::parse_client(body),
            false => wire::parse_client_pregame(body),
        };
        for f in &read.map_or_else(Vec::new, |(_, f)| f) {
            if active {
                self.broadcast_voice(slot, f);
            } else {
                self.pre_game_voice(slot, f);
            }
        }
    }

    /// `SV_PreGameUserVoice` (0x809c3c6): to every other client past
    /// `CS_ZOMBIE` that has not muted the talker and wants voice, no game
    /// rule asked.
    fn pre_game_voice(&mut self, talker: usize, frame: &[u8]) {
        for (n, c) in self.core.clients.iter_mut().enumerate() {
            let Some(c) = c.as_mut() else { continue };
            if n != talker && c.state > State::Zombie && !c.voice.muted[talker] && c.voice.send {
                c.voice.queue(talker, frame);
            }
        }
    }

    /// `SV_UserVoice` (0x809c2ee) into `G_BroadcastVoice` (0x80f9976): the
    /// talker's `lastVoiceTime` stamped, then each client of the level the
    /// rules pass ([`hears`]), by number.
    fn broadcast_voice(&mut self, talker: usize, frame: &[u8]) {
        let rules = Rules {
            global: self.int("voice_global") != 0,
            local_echo: self.int("voice_localEcho") != 0,
            dead_chat: self.int("voice_deadChat") != 0,
            parabolic_radius: 0.0,
            parabolic_angle: 0.0,
        };
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        let now = level.level_time();
        if let Some(c) = level.rt.host.cod2_mut().clients.get_mut(talker) {
            c.last_voice_time = Some(now);
        }
        let voicers: Vec<Option<Voicer>> = (0..self.core.clients.len())
            .map(|i| {
                level.players.get(i)?.as_ref()?;
                let c = self.core.clients[i].as_ref()?;
                let mut field = |n: &str| level.rt.client_field(i, n).unwrap_or_default();
                Some(Voicer {
                    session: session_of(&field("sessionstate")),
                    team: team_of(&field("sessionteam")),
                    muted: c.voice.muted[talker],
                    wants: c.voice.send,
                    ..Voicer::default()
                })
            })
            .collect();
        let Some(t) = voicers.get(talker).copied().flatten() else {
            return;
        };
        for (i, l) in voicers.iter().enumerate() {
            if let Some(l) = l
                && hears(&rules, &t, l, i == talker)
            {
                self.core.clients[i]
                    .as_mut()
                    .unwrap()
                    .voice
                    .queue(talker, frame);
            }
        }
    }

    /// `SV_SendClientVoiceData` (0x809c0a2), after the slot's snapshot (not
    /// after a fragment, `SV_SendClientMessages` Mac 0x197382): an active
    /// client's queue in one out-of-band `v`, then emptied.
    pub(super) fn send_voice(&mut self, slot: usize) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        if c.state != State::Active || c.bot {
            return;
        }
        if let Some(p) = c.voice.take_packet() {
            let to = c.link.addr;
            self.core.outbox.push((to, p));
        }
    }
}

/// A fresh slot's voice: `sendVoice` off the userinfo and the rate
/// `SV_UserinfoChanged` settled on (0x80907cc: a LAN client's 99999, else
/// the userinfo's clamped to 1000..90000, 5000 when it has none).
pub(super) fn client_voice(userinfo: &str, rate: i32) -> ClientVoice {
    ClientVoice::new(crate::voice::wants(userinfo, rate))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use vcod_cod2::net::oob::{self, Reply};
    use vcod_cod2::net::voice as wire;
    use vcod_cod2::net::{Client, latin1};
    use vcod_common::pk3::Pk3Fs;

    use super::super::{Cod2Server, FRAME_MS, tests::cfg};

    /// One of the test's clients: its wire state, the team it answers the
    /// team menu with, and every voice frame it heard.
    struct Peer {
        addr: SocketAddr,
        cl: Client,
        team: &'static str,
        asked_team: bool,
        heard: Vec<(u8, Vec<u8>)>,
    }

    fn join(sv: &mut Cod2Server, n: u16, team: &'static str, now: Instant) -> Peer {
        let addr = SocketAddr::from(([127, 0, 0, 1], 28961 + n));
        sv.handle_packet(addr, &oob::getchallenge(None), now);
        let out = sv.take_outgoing();
        let Reply::Challenge(c) = oob::parse_reply(oob::payload(&out[0].1).unwrap()) else {
            panic!("no challenge");
        };
        let qport = 4660 + n;
        let ui = format!("\\name\\p{n}\\rate\\25000\\snaps\\20\\cl_punkbuster\\0\\cl_voice\\1");
        sv.handle_packet(addr, &oob::connect(&ui, c, qport), now);
        sv.take_outgoing();
        Peer {
            addr,
            cl: Client::new(qport, c),
            team,
            asked_team: false,
            heard: Vec::new(),
        }
    }

    /// One 50 ms frame: every peer but those in `quiet` sends a cmd, the
    /// server steps, and each datagram goes to its peer: a `v` into
    /// `heard`, a message through the parser, its gamestate answered with
    /// the `cp` and its team and weapon menus with the peer's picks.
    fn frame(
        sv: &mut Cod2Server,
        peers: &mut [Peer],
        quiet: &[usize],
        fs: Option<&Pk3Fs>,
        now: &mut Instant,
    ) {
        for (i, p) in peers.iter_mut().enumerate() {
            if quiet.contains(&i) {
                continue;
            }
            let pkt = if p.cl.parser.gamestate.is_some() {
                let base = p.cl.parser.snapshot().map_or(0, |s| s.server_time);
                let cmd = p.cl.idle_cmd(base + FRAME_MS);
                p.cl.build_packet(&[cmd]).unwrap()
            } else {
                p.cl.build_packet(&[]).unwrap()
            };
            sv.handle_packet(p.addr, &pkt, *now);
        }
        *now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(*now, 1);
        for (to, pkt) in sv.take_outgoing() {
            let Some(p) = peers.iter_mut().find(|p| p.addr == to) else {
                continue;
            };
            if let Some(body) = oob::payload(&pkt).and_then(wire::oob_body) {
                p.heard.extend(wire::parse_server(body));
                continue;
            }
            let Ok(Some(parsed)) = p.cl.receive(&pkt) else {
                continue;
            };
            if parsed.gamestate {
                let gs = p.cl.parser.gamestate.as_ref().unwrap();
                let si = gs.configstring(1).to_string();
                let cp = match fs {
                    Some(fs) => {
                        fs.clear_references();
                        fs.touch("maps/mp/mp_toujane.d3dbsp");
                        vcod_cod2::net::pure::pure_command(fs, gs.checksum_feed, &si)
                    }
                    None => vcod_cod2::net::pure::cp_command(&[], gs.checksum_feed),
                };
                p.cl.add_reliable(&cp);
            }
            for (_, text) in &parsed.commands {
                let text = latin1(text);
                let Some(n) = text
                    .strip_prefix("t ")
                    .and_then(|n| n.trim().parse::<u16>().ok())
                else {
                    continue;
                };
                let menu =
                    p.cl.parser
                        .gamestate
                        .as_ref()
                        .unwrap()
                        .configstring(1246 + n);
                let weapon = match menu.strip_prefix("weapon_") {
                    Some("german") => "kar98k_mp",
                    Some("british") => "enfield_mp",
                    Some("american") => "m1garand_mp",
                    Some("russian") => "mosin_nagant_mp",
                    _ => continue,
                };
                let reply = format!("mr {} {n} {weapon}", p.cl.server_id());
                p.cl.add_reliable(&reply);
            }
            if fs.is_some() && parsed.snapshot.is_some() && !p.asked_team {
                p.asked_team = true;
                let gs = p.cl.parser.gamestate.as_ref().unwrap();
                let menu = (0..32)
                    .find(|&i| gs.configstring(1246 + i).starts_with("team_"))
                    .expect("a team menu");
                let mr = format!("mr {} {menu} {}", p.cl.server_id(), p.team);
                p.cl.add_reliable(&mr);
            }
        }
    }

    /// The bare server (no level, as CI runs it): a client still loading
    /// reaches every other client through `SV_PreGameUserVoice`, which
    /// reads short lengths, so the client's own byte-length packet misreads
    /// and is dropped; nothing goes with `sv_voice` off.
    #[test]
    fn cod2_voice_from_a_loading_client_reads_short_lengths() {
        let mut now = Instant::now();
        let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
        sv.load(None).unwrap();
        let mut peers = vec![join(&mut sv, 0, "", now)];
        for _ in 0..20 {
            frame(&mut sv, &mut peers, &[], None, &mut now);
        }
        assert!(
            peers[0].cl.parser.snapshot().is_some(),
            "the listener is in"
        );
        // The talker connects and sends no packet but voice.
        peers.push(join(&mut sv, 1, "", now));
        let talker = peers[1].addr;
        let byte_len = wire::client_packet(4661, &[vec![0x11, 0x22, 0x33]]);
        let mut short_len = vec![0xff, 0xff, 0xff, 0xff, b'v', 0, 0x35, 0x12, 1, 3, 0];
        short_len.extend_from_slice(&[0x44, 0x55, 0x66]);
        for sv_voice in ["0", "1"] {
            sv.set_cvar("sv_voice", sv_voice);
            sv.handle_packet(talker, &byte_len, now);
            sv.handle_packet(talker, &short_len, now);
            for _ in 0..2 {
                frame(&mut sv, &mut peers, &[1], None, &mut now);
            }
        }
        assert_eq!(peers[0].heard, vec![(1, vec![0x44, 0x55, 0x66])]);
        assert!(peers[1].heard.is_empty());
    }

    /// `SV_VoicePacket`'s qport is a signed short against the slot's
    /// unsigned one: a client on qport 0x8000 or more is never heard, as
    /// retail's 40000 was not (cod2-voice.md 6).
    #[test]
    fn cod2_voice_from_a_high_qport_matches_no_slot() {
        let mut now = Instant::now();
        let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
        sv.set_cvar("sv_voice", "1");
        sv.load(None).unwrap();
        let mut peers = vec![join(&mut sv, 0, "", now)];
        for _ in 0..20 {
            frame(&mut sv, &mut peers, &[], None, &mut now);
        }
        for (port, qport) in [(29000, 0x7fffu16), (29001, 0x8000)] {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            sv.handle_packet(addr, &oob::getchallenge(None), now);
            let out = sv.take_outgoing();
            let Reply::Challenge(c) = oob::parse_reply(oob::payload(&out[0].1).unwrap()) else {
                panic!("no challenge");
            };
            let ui = "\\name\\hi\\rate\\25000\\cl_voice\\1";
            sv.handle_packet(addr, &oob::connect(ui, c, qport), now);
            sv.take_outgoing();
            let mut pkt = vec![0xff, 0xff, 0xff, 0xff, b'v', 0];
            pkt.extend_from_slice(&qport.to_le_bytes());
            pkt.extend_from_slice(&[1, 1, 0, 0x77]);
            sv.handle_packet(addr, &pkt, now);
            for _ in 0..2 {
                frame(&mut sv, &mut peers, &[], None, &mut now);
            }
            let heard = std::mem::take(&mut peers[0].heard);
            match qport {
                0x7fff => assert_eq!(heard, vec![(1, vec![0x77])]),
                _ => assert!(heard.is_empty(), "{heard:?}"),
            }
        }
    }

    /// Three players on mp_toujane's tdm with `sv_voice 1`: the talker's
    /// frames reach his teammate under his client number, not the enemy
    /// nor himself; `voice_global 1` crosses teams; the teammate's
    /// `muteplayer 0` stops them for him; the talker's `lastVoiceTime` is
    /// stamped for `isTalking`. Needs `COD2_DIR`.
    #[test]
    fn cod2_voice_reaches_the_team_and_voice_global_the_enemy() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let fs = Rc::new(fs);
        let mut now = Instant::now();
        let mut sv = Cod2Server::new(
            super::super::Cod2Config {
                gametype: "tdm".into(),
                ..cfg("mp_toujane")
            },
            7,
            now,
        );
        sv.set_cvar("sv_voice", "1");
        sv.load(Some(fs.clone())).unwrap();
        let mut peers = vec![
            join(&mut sv, 0, "allies", now),
            join(&mut sv, 1, "allies", now),
            join(&mut sv, 2, "axis", now),
        ];
        let playing = |sv: &Cod2Server| (0..3).all(|i| sv.game.level.as_ref().unwrap().playing(i));
        for _ in 0..20 * 20 {
            frame(&mut sv, &mut peers, &[], Some(&fs), &mut now);
            if playing(&sv) {
                break;
            }
        }
        assert!(playing(&sv), "all three spawned");
        let talk = |sv: &mut Cod2Server, peers: &mut Vec<Peer>, now: &mut Instant, tag: u8| {
            for p in peers.iter_mut() {
                p.heard.clear();
            }
            let pkt = wire::client_packet(4660, &[vec![tag; 10], vec![tag, 1]]);
            sv.handle_packet(peers[0].addr, &pkt, *now);
            for _ in 0..2 {
                frame(sv, peers, &[], Some(&fs), now);
            }
        };
        talk(&mut sv, &mut peers, &mut now, 0xa1);
        assert_eq!(
            peers[1].heard,
            vec![(0, vec![0xa1; 10]), (0, vec![0xa1, 1])]
        );
        assert!(peers[2].heard.is_empty(), "the enemy heard us");
        assert!(peers[0].heard.is_empty(), "no echo without voice_localEcho");
        let level = sv.game.level.as_ref().unwrap();
        let stamped = level.rt.host.cod2().clients[0].last_voice_time;
        assert!(
            stamped.is_some_and(|t| level.level_time() - t < 200),
            "{stamped:?}"
        );

        sv.set_cvar("voice_global", "1");
        talk(&mut sv, &mut peers, &mut now, 0xa2);
        assert_eq!(peers[2].heard.len(), 2, "voice_global reaches the enemy");
        sv.set_cvar("voice_global", "0");

        peers[1].cl.add_reliable("muteplayer 0");
        for _ in 0..2 {
            frame(&mut sv, &mut peers, &[], Some(&fs), &mut now);
        }
        talk(&mut sv, &mut peers, &mut now, 0xa3);
        assert!(peers[1].heard.is_empty(), "muted, yet heard");
        let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
        assert!(aborts.is_empty(), "{}", aborts.join("\n"));
    }
}
