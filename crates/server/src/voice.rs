//! The voice relay CoD2 1.3 and CoD4 1.7 share (docs/research/cod2-voice.md
//! 3, cod4-voice.md 3): each client's queue, mute list and `sendVoice`, and
//! `G_BroadcastVoice`'s test of one listener. Each game's server reads its
//! packets and fills [`Voicer`]s (`cod2::voice`, `cod4::voice`). CoD2 has no
//! Eavesdrop: its `G_BroadcastVoice` (`cod2_lnxded` 0x80f9976) is CoD4's
//! without the perk branch, so a CoD2 [`Voicer`] carries no perks.

use vcod_common::net::voice as wire;

/// One client's voice state (`client_t.voicePackets`, `muteList`,
/// `sendVoice`).
#[derive(Clone, Debug)]
pub struct ClientVoice {
    pub queued: Vec<(u8, Vec<u8>)>,
    /// `muteList`: 65 entries, as `SV_MutePlayer_f` takes 0..=64 (CoD4
    /// `cmp eax,0x40; ja` at 0x81702ae, CoD2 `uVar2 < 0x41` at 0x8090a5a).
    pub muted: [bool; 65],
    /// `sendVoice`.
    pub send: bool,
}

impl ClientVoice {
    /// A fresh slot: nothing queued or muted, `sendVoice` as given.
    pub fn new(send: bool) -> Self {
        ClientVoice {
            queued: Vec::new(),
            muted: [false; 65],
            send,
        }
    }

    /// `muteplayer <n>` / `unmuteplayer <n>` (`SV_MutePlayer_f`,
    /// `SV_UnmutePlayer_f`): `atoi`, 0..=64 kept, anything else `Invalid
    /// mute client %i`.
    pub fn mute_command(&mut self, arg: &str, mute: bool) {
        let n = crate::game::builtins::cvar::atoi(arg);
        match usize::try_from(n).ok().filter(|&n| n <= 64) {
            Some(n) => self.muted[n] = mute,
            None if mute => log::info!("Invalid mute client {n}"),
            None => log::info!("Invalid unmute client {n}"),
        }
    }

    /// `SV_QueueVoicePacket`: at most 40 a frame.
    pub fn queue(&mut self, talker: usize, frame: &[u8]) {
        if self.queued.len() < wire::MAX_QUEUED {
            self.queued.push((talker as u8, frame.to_vec()));
        }
    }

    /// `SV_SendClientVoiceData`'s datagram for the queue, which it empties;
    /// `None` with nothing queued.
    pub fn take_packet(&mut self) -> Option<Vec<u8>> {
        if self.queued.is_empty() {
            return None;
        }
        let p = wire::server_packet(&self.queued);
        self.queued.clear();
        Some(p)
    }
}

/// `SV_UserinfoChanged`'s `sendVoice` (CoD2 0x80907cc, CoD4 0x81703c6):
/// the userinfo's `cl_voice` above 0, cleared under a `rate` of 5000.
pub fn wants(userinfo: &str, rate: i32) -> bool {
    let cl_voice = vcod_common::net::info_value_for_key(userinfo, "cl_voice").unwrap_or("");
    crate::game::builtins::cvar::atoi(cl_voice) > 0 && rate >= 5000
}

/// What `G_BroadcastVoice` reads of one client.
#[derive(Clone, Copy, Debug, Default)]
pub struct Voicer {
    /// `sess.sessionState`: 0 playing, 1 dead, 2 spectator, 3
    /// intermission.
    pub session: i32,
    /// `sess.cs.team`: 0 free, 1 axis, 2 allies, 3 spectator.
    pub team: i32,
    /// CoD4's `ps.perks`; 0 on CoD2.
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
pub struct Rules {
    pub global: bool,
    pub local_echo: bool,
    pub dead_chat: bool,
    /// CoD4's Eavesdrop dvars; CoD2 never reads them.
    pub parabolic_radius: f32,
    pub parabolic_angle: f32,
}

/// Eavesdrop's bit in CoD4's `ps.perks` (`parabolic`, 9; the test of byte
/// 0x5fd bit 1 at `cod4_lnxded` 0x80a72aa).
pub const PERK_PARABOLIC: u32 = 1 << 9;

/// `sessionState` 1 and 3.
const SESSION_DEAD: i32 = 1;
const SESSION_INTERMISSION: i32 = 3;

/// `G_BroadcastVoice` for one listener: whether `talker`'s frame is queued
/// for `listener` (cod4-voice.md 3, cod2-voice.md 3, steps 1 to 4). `same`
/// is the talker himself.
pub fn hears(r: &Rules, talker: &Voicer, listener: &Voicer, same: bool) -> bool {
    let delivered = listener.wants && !listener.muted && (r.local_echo || !same);
    delivered && sessions_mix(r, talker.session, listener.session) && in_reach(r, talker, listener)
}

/// Step 1: the intermission and `voice_global` reach everyone; otherwise a
/// free talker, one on the listener's team (`OnSameTeam`), or, on CoD4,
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

/// CoD4's Eavesdrop (`cod4_lnxded` 0x80a72b7..0x80a7379): the talker no
/// farther than `perk_parabolicRadius` and no more than
/// `perk_parabolicAngle` off the listener's facing. A talker on the
/// listener's own spot has no direction (a dot of 0), so only a cone of 90
/// degrees or wider takes him.
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

/// A script `sessionstate` as the number `G_BroadcastVoice` compares.
pub fn session_of(s: &str) -> i32 {
    match s {
        "dead" => 1,
        "spectator" => 2,
        "intermission" => 3,
        _ => 0,
    }
}

/// A script `sessionteam` as the number `OnSameTeam` compares.
pub fn team_of(s: &str) -> i32 {
    match s {
        "axis" => 1,
        "allies" => 2,
        "spectator" => 3,
        _ => 0,
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
    fn voice_reaches_the_team_alive_unmuted() {
        let r = rules();
        let (allies, axis) = (v(2, 0), v(1, 0));
        assert!(hears(&r, &allies, &allies, false));
        assert!(!hears(&r, &allies, &axis, false));
        // Not himself unless `voice_localEcho`, not muted, not without
        // `cl_voice`.
        assert!(!hears(&r, &allies, &allies, true));
        let echo = Rules {
            local_echo: true,
            ..r
        };
        assert!(hears(&echo, &allies, &allies, true));
        let muted = Voicer {
            muted: true,
            ..allies
        };
        assert!(!hears(&r, &allies, &muted, false));
        let deaf = Voicer {
            wants: false,
            ..allies
        };
        assert!(!hears(&r, &allies, &deaf, false));
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
        // The intermission passes the team rule; spectators hear each
        // other (team 3 both).
        assert!(hears(&r, &v(2, 3), &v(1, 3), false));
        assert!(hears(&r, &v(3, 2), &v(3, 2), false));
        assert!(!hears(&r, &v(3, 2), &v(2, 0), false));
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
        // A CoD2 listener carries no perks: the enemy stays unheard.
        assert!(!hears(&r, &talker, &v(1, 0), false));
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
    fn voice_mute_takes_0_to_64() {
        let mut c = ClientVoice::new(wants("\\cl_voice\\1", 25000));
        assert!(c.send);
        c.mute_command("64", true);
        c.mute_command("65", true);
        c.mute_command("-1", true);
        assert!(c.muted[64] && c.muted.iter().filter(|m| **m).count() == 1);
        c.mute_command("64", false);
        assert!(!c.muted[64]);
        assert!(!wants("\\cl_voice\\1", 4999));
        assert!(!wants("\\rate\\25000", 25000));
        assert!(!wants("\\cl_voice\\0", 25000));
    }

    #[test]
    fn the_queue_holds_40_and_goes_in_one_packet() {
        let mut c = ClientVoice::new(true);
        assert_eq!(c.take_packet(), None);
        for i in 0..45u8 {
            c.queue(usize::from(i % 3), &[i]);
        }
        let p = c.take_packet().unwrap();
        assert_eq!(p[6], 40);
        assert_eq!(wire::parse_server(&p[6..]).len(), 40);
        assert!(c.queued.is_empty());
    }
}
