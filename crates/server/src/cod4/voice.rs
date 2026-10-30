//! CoD4 1.7's voice relay (docs/research/cod4-voice.md 3): the client's
//! out-of-band `v` packets in, each frame queued for the clients the
//! rules let hear it, and each client's queue out of band after its
//! message. Addresses are `cod4_lnxded`'s.

use std::net::SocketAddr;

use vcod_cod4::net::voice as wire;

use super::{Cod4Server, State};

/// One client's voice state (`client_t.voicePackets`, `muteList`,
/// `sendVoice`).
#[derive(Clone, Debug)]
pub(super) struct ClientVoice {
    pub queued: Vec<(u8, Vec<u8>)>,
    /// `muteList`: 65 entries, as `SV_MutePlayer_f` (0x81702ae) takes
    /// 0..=64 (`cmp eax,0x40; ja`).
    pub muted: [bool; 65],
    /// `sendVoice`.
    pub send: bool,
}

impl ClientVoice {
    /// A fresh slot: nothing queued or muted, `sendVoice` off its
    /// userinfo.
    pub fn of(userinfo: &str) -> Self {
        ClientVoice {
            queued: Vec::new(),
            muted: [false; 65],
            send: wants_voice(userinfo),
        }
    }

    /// `muteplayer <n>` / `unmuteplayer <n>` (0x81702ae, 0x817023c):
    /// `strtol`, 0..=64 kept, anything else `Invalid mute client %i`.
    pub fn mute_command(&mut self, arg: &str, mute: bool) {
        let n = crate::game::builtins::cvar::atoi(arg);
        match usize::try_from(n).ok().filter(|&n| n <= 64) {
            Some(n) => self.muted[n] = mute,
            None if mute => log::info!("Invalid mute client {n}"),
            None => log::info!("Invalid unmute client {n}"),
        }
    }

    /// `SV_QueueVoicePacket` (0x817ba20): at most 40 a frame.
    fn queue(&mut self, talker: usize, frame: &[u8]) {
        if self.queued.len() < wire::MAX_QUEUED {
            self.queued.push((talker as u8, frame.to_vec()));
        }
    }
}

/// `SV_UserinfoChanged` (0x81703c6..0x817040b): `sendVoice` is
/// `cl_voice` above 0 and a `rate` over 4999. A LAN client's rate reads
/// 99999 there, past the test.
pub(super) fn wants_voice(userinfo: &str) -> bool {
    let key = |k: &str| vcod_common::net::info_value_for_key(userinfo, k).unwrap_or("");
    let atoi = crate::game::builtins::cvar::atoi;
    let rate = match key("rate") {
        "" => 5000,
        r => atoi(r),
    };
    atoi(key("cl_voice")) > 0 && rate > 4999
}

/// What `G_BroadcastVoice` reads of one client.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Voicer {
    /// `sess.sessionState`: 0 playing, 1 dead, 2 spectator, 3
    /// intermission.
    pub session: i32,
    /// `sess.cs.team`: 0 free, 1 axis, 2 allies, 3 spectator.
    pub team: i32,
    pub perks: u32,
    pub origin: [f32; 3],
    /// The listener's angles (`clientinfo.playerAngles`; vcod reads the
    /// view, INFERRED as the same for the angle test).
    pub angles: [f32; 3],
    pub muted: bool,
    pub wants: bool,
}

/// The rule dvars.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    pub global: bool,
    pub local_echo: bool,
    pub dead_chat: bool,
    pub parabolic_radius: f32,
    pub parabolic_angle: f32,
}

/// Eavesdrop's bit in `ps.perks` (`parabolic`, 9; the test of byte 0x5fd
/// bit 1 at 0x80a72aa).
const PERK_PARABOLIC: u32 = 1 << 9;

/// `sessionState` 1 and 3.
const SESSION_DEAD: i32 = 1;
const SESSION_INTERMISSION: i32 = 3;

/// `G_BroadcastVoice` (0x80a7170) for one listener: whether `talker`'s
/// frame is queued for `listener` (cod4-voice.md 3, steps 1 to 4).
/// `same` is the talker himself.
pub(crate) fn hears(r: &Rules, talker: &Voicer, listener: &Voicer, same: bool) -> bool {
    let delivered = listener.wants && !listener.muted && (r.local_echo || !same);
    delivered && sessions_mix(r, talker.session, listener.session) && in_reach(r, talker, listener)
}

/// Step 1: the intermission and `voice_global` reach everyone; otherwise a
/// free talker, one on the listener's team (`OnSameTeam`, 0x80c8c88), or
/// one the listener eavesdrops on.
fn in_reach(r: &Rules, talker: &Voicer, listener: &Voicer) -> bool {
    if listener.session == SESSION_INTERMISSION || r.global {
        return true;
    }
    if talker.team == 0 || talker.team == listener.team {
        return true;
    }
    listener.perks & PERK_PARABOLIC != 0 && eavesdrops(r, talker, listener)
}

/// Step 2: one session state, or a dead one on either side under
/// `voice_deadChat`.
fn sessions_mix(r: &Rules, a: i32, b: i32) -> bool {
    a == b || (r.dead_chat && (a == SESSION_DEAD || b == SESSION_DEAD))
}

/// Eavesdrop (0x80a72b7..0x80a7379): the talker no farther than
/// `perk_parabolicRadius` and no more than `perk_parabolicAngle` off the
/// listener's facing. A talker on the listener's own spot has no
/// direction (a dot of 0), so only a cone of 90 degrees or wider takes him.
fn eavesdrops(r: &Rules, talker: &Voicer, listener: &Voicer) -> bool {
    let to: [f64; 3] = std::array::from_fn(|i| f64::from(talker.origin[i] - listener.origin[i]));
    let dist = to.iter().map(|v| v * v).sum::<f64>().sqrt();
    if dist > f64::from(r.parabolic_radius) {
        return false;
    }
    let [pitch, yaw] = [0, 1].map(|i| f64::from(listener.angles[i]).to_radians());
    let facing = [
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        -pitch.sin(),
    ];
    let along = match dist {
        0.0 => 0.0,
        _ => to.iter().zip(facing).map(|(a, b)| a * b).sum::<f64>() / dist,
    };
    along >= f64::from(r.parabolic_angle).to_radians().cos()
}

fn session_of(s: &str) -> i32 {
    match s {
        "dead" => 1,
        "spectator" => 2,
        "intermission" => 3,
        _ => 0,
    }
}

fn team_of(s: &str) -> i32 {
    match s {
        "axis" => 1,
        "allies" => 2,
        "spectator" => 3,
        _ => 0,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules {
            global: false,
            local_echo: false,
            dead_chat: false,
            parabolic_radius: 400.0,
            parabolic_angle: 180.0,
        }
    }

    fn v(team: i32, session: i32) -> Voicer {
        Voicer {
            session,
            team,
            wants: true,
            ..Voicer::default()
        }
    }

    #[test]
    fn cod4_voice_reaches_the_team_alive_unmuted() {
        let r = rules();
        let (allies, axis) = (v(2, 0), v(1, 0));
        assert!(hears(&r, &allies, &allies, false));
        assert!(!hears(&r, &allies, &axis, false));
        // Not himself, not muted, not without `cl_voice`.
        assert!(!hears(&r, &allies, &allies, true));
        assert!(!hears(
            &r,
            &allies,
            &Voicer {
                muted: true,
                ..allies
            },
            false
        ));
        assert!(!hears(
            &r,
            &allies,
            &Voicer {
                wants: false,
                ..allies
            },
            false
        ));
        // The dead hear only the dead, unless `voice_deadChat`.
        assert!(!hears(&r, &allies, &v(2, 1), false));
        let dc = Rules {
            dead_chat: true,
            ..r
        };
        assert!(hears(&dc, &allies, &v(2, 1), false));
        // A free talker (dm) reaches all; `voice_global` crosses teams.
        assert!(hears(&r, &v(0, 0), &axis, false));
        assert!(hears(&Rules { global: true, ..r }, &allies, &axis, false));
        // The intermission passes the team rule.
        assert!(hears(&r, &v(2, 3), &v(1, 3), false));
    }

    #[test]
    fn cod4_voice_eavesdrop_hears_the_enemy_in_its_radius() {
        let r = rules();
        let talker = Voicer {
            origin: [300.0, 0.0, 0.0],
            ..v(2, 0)
        };
        let ear = Voicer {
            perks: PERK_PARABOLIC,
            ..v(1, 0)
        };
        assert!(hears(&r, &talker, &ear, false));
        let far = Voicer {
            origin: [401.0, 0.0, 0.0],
            ..talker
        };
        assert!(!hears(&r, &far, &ear, false));
        // A 90 degree cone: facing away misses, facing it hears.
        let cone = Rules {
            parabolic_angle: 90.0,
            ..r
        };
        let away = Voicer {
            angles: [0.0, 180.0, 0.0],
            ..ear
        };
        assert!(!hears(&cone, &talker, &away, false));
        assert!(hears(&cone, &talker, &ear, false));
    }

    #[test]
    fn cod4_voice_mute_takes_0_to_64() {
        let mut c = ClientVoice::of("\\cl_voice\\1\\rate\\25000");
        assert!(c.send);
        c.mute_command("64", true);
        c.mute_command("65", true);
        c.mute_command("-1", true);
        assert!(c.muted[64] && c.muted.iter().filter(|m| **m).count() == 1);
        c.mute_command("64", false);
        assert!(!c.muted[64]);
        assert!(!wants_voice("\\cl_voice\\1\\rate\\4999"));
        assert!(!wants_voice("\\rate\\25000"));
    }
}
