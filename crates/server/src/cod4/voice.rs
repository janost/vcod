//! CoD4 1.7's voice relay (docs/research/cod4-voice.md 3): the client's
//! out-of-band `v` packets in, each frame queued for the clients the
//! rules let hear it, and each client's queue out of band after its
//! message. The queue and the rules are `crate::voice`'s, shared with
//! CoD2. Addresses are `cod4_lnxded`'s.

use std::net::SocketAddr;

use vcod_cod4::net::voice as wire;

use super::{Cod4Server, State};
use crate::voice::{Rules, Voicer, hears, session_of, team_of};

pub(super) use crate::voice::ClientVoice;

/// A fresh slot's voice: `sendVoice` off its userinfo.
pub(super) fn client_voice(userinfo: &str) -> ClientVoice {
    ClientVoice::new(wants_voice(userinfo))
}

/// `SV_UserinfoChanged` (0x81703c6..0x817040b): `sendVoice` is
/// `cl_voice` above 0 and a `rate` over 4999, read off the userinfo (5000
/// when it has none). A LAN client's rate reads 99999 there, past the test.
pub(super) fn wants_voice(userinfo: &str) -> bool {
    let rate = match vcod_common::net::info_value_for_key(userinfo, "rate").unwrap_or("") {
        "" => 5000,
        r => crate::game::builtins::cvar::atoi(r),
    };
    crate::voice::wants(userinfo, rate)
}

impl Cod4Server {
    /// `SV_VoicePacket` (0x817771f): the slot by address and qport, a
    /// zombie's ignored, `lastPacketTime` refreshed; before the world
    /// `SV_PreGameUserVoice`, in it `SV_UserVoice`, both only with
    /// `sv_voice` on.
    pub(super) fn voice_packet(&mut self, from: SocketAddr, body: &[u8]) {
        let Some(qport) = body.get(..2).map(|q| u16::from_le_bytes([q[0], q[1]])) else {
            return;
        };
        let Some(slot) = self.slot_of(from, qport) else {
            return;
        };
        let svs = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        if c.state == State::Zombie {
            return;
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
        let frames = read.map_or_else(Vec::new, |(_, f)| f);
        for f in &frames {
            if active {
                self.broadcast_voice(slot, f);
            } else {
                self.pre_game_voice(slot, f);
            }
        }
    }

    /// `SV_PreGameUserVoice` (0x817bc58): to every other client past
    /// `CS_CONNECTED`'s start that has not muted the talker and wants
    /// voice, the frames read with short lengths
    /// ([`wire::parse_client_pregame`]).
    fn pre_game_voice(&mut self, talker: usize, frame: &[u8]) {
        for (n, c) in self.core.clients.iter_mut().enumerate() {
            let Some(c) = c.as_mut() else { continue };
            if n != talker && c.state > State::Zombie && !c.voice.muted[talker] && c.voice.send {
                c.voice.queue(talker, frame);
            }
        }
    }

    /// `SV_UserVoice` (0x817b95e) into `G_BroadcastVoice` (0x80a7170):
    /// the talker's `lastVoiceTime` stamped, then each client of the
    /// level the rules pass ([`hears`]), by number.
    fn broadcast_voice(&mut self, talker: usize, frame: &[u8]) {
        let f = |me: &Self, n: &str| crate::game::builtins::cvar::atof(me.game.cvars().get(n));
        let rules = Rules {
            global: self.int("voice_global") != 0,
            local_echo: self.int("voice_localEcho") != 0,
            dead_chat: self.int("voice_deadChat") != 0,
            parabolic_radius: f(self, "perk_parabolicRadius"),
            parabolic_angle: f(self, "perk_parabolicAngle"),
        };
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        let now = level.level_time();
        if let Some(c) = level.rt.host.cod4_mut().clients.get_mut(talker) {
            c.last_voice_time = Some(now);
        }
        let n = self.core.clients.len();
        let mut voicers: Vec<Option<Voicer>> = vec![None; n];
        for (i, v) in voicers.iter_mut().enumerate() {
            let Some(p) = level.players.get(i).and_then(Option::as_ref) else {
                continue;
            };
            let f = |n: &str| p.ps.get_f32(n).unwrap_or(0.0);
            let origin = [f("origin[0]"), f("origin[1]"), f("origin[2]")];
            let angles = [f("viewangles[0]"), f("viewangles[1]"), f("viewangles[2]")];
            let session = session_of(&level.rt.client_field(i, "sessionstate").unwrap_or_default());
            let team = team_of(&level.rt.client_field(i, "sessionteam").unwrap_or_default());
            let perks = level.rt.host.cod4().clients.get(i).map_or(0, |c| c.perks);
            let Some(c) = self.core.clients[i].as_ref() else {
                continue;
            };
            *v = Some(Voicer {
                session,
                team,
                perks,
                origin,
                angles,
                muted: c.voice.muted[talker],
                wants: c.voice.send,
            });
        }
        let Some(t) = voicers[talker] else {
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

    /// `SV_SendClientVoiceData` (0x817bab2), after the slot's message: an
    /// active client's queue in one out-of-band `v`, then emptied.
    pub(super) fn send_voice(&mut self, slot: usize) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        if c.state != State::Active || c.bot || c.voice.queued.is_empty() {
            return;
        }
        let p = wire::server_packet(&c.voice.queued);
        c.voice.queued.clear();
        let to = c.link.addr;
        self.core.outbox.push((to, p));
    }
}
