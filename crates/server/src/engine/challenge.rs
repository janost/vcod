//! `challenge_t`: the table `SV_GetChallenge`, `SV_DirectConnect` and
//! `SV_DropClient` share. All four retail servers keep it the same way
//! (cod11-server-handshake.md, "The challenge table"): 1024 slots, no
//! expiry, a new challenge over the slot with the lowest creation time,
//! every lookup on the whole address, port included. 1.1d's table is at
//! 0x83b67f8 (`SV_GetChallenge` 0x8084d90), 1.5b's at 0x83ccde0
//! (0x80889d0), UO 1.51b's at 0x84f7060 (0x80897d4), CoD2 1.3's at
//! 0x84230dc (0x808d18e). Times are the caller's server clock in ms.

use std::hash::{BuildHasher, RandomState};
use std::net::SocketAddr;

/// `MAX_CHALLENGES`: the loop bound 0x400 in all four.
pub const MAX_CHALLENGES: usize = 1024;
/// CoD4 1.7's slot hex: `Q_strncpyz` (0x81a9e1c) into +0x2c with size
/// 0x21 (0x816f1ba), so at most 32 chars.
const GUID_MAX: usize = 32;

#[derive(Clone, Default)]
struct Slot {
    /// `None` is a slot never written (retail's zeroed `netadr_t`).
    addr: Option<SocketAddr>,
    challenge: i32,
    /// When the slot was written (+0x18); the replacement's key.
    time: i32,
    /// When the last `challengeResponse` went out (+0x1c).
    ping_time: i32,
    /// The first connect's `time - ping_time` (+0x24), 0 until then.
    first_ping: i32,
    /// A client connected off it (+0x28).
    connected: bool,
    /// CoD4's `getchallenge` hex, the GUID its connect takes (+0x2c).
    guid: String,
}

pub struct Challenges {
    slots: Vec<Slot>,
    /// vcod's mask over each drawn challenge: SipHash of the address and
    /// the draw under a per-process key from the OS, so a challenge says
    /// nothing of the `rand()` stream it came off and one sent to a spoofed
    /// address cannot be guessed. Retail sends the draw itself. `None`
    /// sends the draw (the A/B gates that compare it with retail's).
    key: Option<RandomState>,
}

impl Default for Challenges {
    fn default() -> Self {
        Challenges {
            slots: vec![Slot::default(); MAX_CHALLENGES],
            key: Some(RandomState::new()),
        }
    }
}

impl Challenges {
    /// `SV_GetChallenge` on its answer-at-once path: the first slot not
    /// connected whose address is `from`, else the slot with the lowest
    /// time (the first of a tie; an unwritten slot's is 0) overwritten
    /// with `draw()` under [`Self::key`]'s mask. Either way `pingTime` is
    /// `now`. Returns the challenge to send. `draw` runs only for a new
    /// slot, so the caller's `rand()` draws are retail's.
    pub fn issue(&mut self, from: SocketAddr, now: i32, draw: impl FnOnce() -> i32) -> i32 {
        let mut oldest = 0;
        let mut oldest_time = i32::MAX;
        let mut found = None;
        for (i, s) in self.slots.iter().enumerate() {
            if !s.connected && s.addr == Some(from) {
                found = Some(i);
                break;
            }
            if s.time < oldest_time {
                oldest_time = s.time;
                oldest = i;
            }
        }
        let i = found.unwrap_or_else(|| {
            let drawn = draw();
            let mask = self.key.as_ref().map_or(0, |k| k.hash_one((from, drawn)));
            self.slots[oldest] = Slot {
                addr: Some(from),
                challenge: drawn ^ mask as i32,
                time: now,
                ..Slot::default()
            };
            oldest
        });
        self.slots[i].ping_time = now;
        self.slots[i].challenge
    }

    /// `SV_DirectConnect`'s lookup: the first slot, connected or not,
    /// whose address is `from` and whose challenge is `challenge`. Marks it
    /// connected and returns its index and the ping, measured on the first
    /// connect and kept for later ones until [`Challenges::taken`].
    pub fn connect(&mut self, from: SocketAddr, challenge: i32, now: i32) -> Option<(usize, i32)> {
        let i = self
            .slots
            .iter()
            .position(|s| s.addr == Some(from) && s.challenge == challenge)?;
        let s = &mut self.slots[i];
        if s.first_ping == 0 {
            s.first_ping = now.wrapping_sub(s.ping_time);
        }
        s.connected = true;
        Some((i, s.first_ping))
    }

    /// CoD4's `SV_GetChallenge` copies the request's hex into the open slot
    /// for `from`, cut to [`GUID_MAX`] chars (cod4-handshake.md 15).
    pub fn keep_guid(&mut self, from: SocketAddr, guid: &str) {
        if let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| !s.connected && s.addr == Some(from))
        {
            s.guid = guid.chars().take(GUID_MAX).collect();
        }
    }

    /// The hex slot `i` holds ([`Challenges::keep_guid`]).
    pub fn guid(&self, i: usize) -> &str {
        &self.slots[i].guid
    }

    /// A connect that took a slot clears the stored ping (1.1d 0x8085c16),
    /// so a retry on the same challenge measures again.
    pub fn taken(&mut self, i: usize) {
        self.slots[i].first_ping = 0;
    }

    /// `SV_DropClient` for a client that is not a bot: the first slot whose
    /// address is `addr`, connected or not, is no longer connected.
    pub fn release(&mut self, addr: SocketAddr) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.addr == Some(addr)) {
            s.connected = false;
        }
    }

    /// A ban refusal's cleared challenge: the open slot for `from` zeroed.
    pub fn forget(&mut self, from: SocketAddr) {
        if let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| !s.connected && s.addr == Some(from))
        {
            *s = Slot::default();
        }
    }

    /// Sends each draw unmasked, as retail does: the A/B gates, and
    /// `bot_match`'s `NETDIGEST`, whose bytes the per-process key would
    /// change every run.
    #[cfg(any(test, feature = "testing"))]
    pub fn unmasked(&mut self) {
        self.key = None;
    }

    /// `SV_Shutdown`'s zeroed table.
    pub fn clear(&mut self) {
        self.slots.fill(Slot::default());
    }

    /// The challenge an open slot holds for `from`.
    pub fn open(&self, from: SocketAddr) -> Option<i32> {
        self.slots
            .iter()
            .find(|s| !s.connected && s.addr == Some(from))
            .map(|s| s.challenge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(ip: u8, port: u16) -> SocketAddr {
        SocketAddr::from(([203, 0, 113, ip], port))
    }

    /// A table that sends its draws as retail does.
    fn table() -> Challenges {
        let mut t = Challenges::default();
        t.unmasked();
        t
    }

    /// Two servers drawing the same `rand()` values send different
    /// challenges, and each takes back the one it sent.
    #[test]
    fn a_challenge_is_masked_per_process() {
        let (mut a, mut b) = (Challenges::default(), Challenges::default());
        let ca = a.issue(addr(1, 5), 0, || 7);
        let cb = b.issue(addr(1, 5), 0, || 7);
        assert!(ca != cb && ca != 7, "{ca} {cb}");
        assert!(a.connect(addr(1, 5), 7, 10).is_none());
        assert!(a.connect(addr(1, 5), ca, 10).is_some());
    }

    #[test]
    fn an_asking_address_keeps_its_slot_and_nothing_expires() {
        let mut t = table();
        let a = t.issue(addr(1, 5), 1000, || 7);
        // An hour on, the slot is still there and still the same.
        assert_eq!(t.issue(addr(1, 5), 3_601_000, || unreachable!()), a);
        assert_eq!(t.connect(addr(1, 5), 7, 3_601_050), Some((0, 50)));
    }

    #[test]
    fn the_whole_address_matches_port_included() {
        let mut t = table();
        t.issue(addr(1, 5), 0, || 7);
        assert_eq!(t.connect(addr(1, 6), 7, 10), None);
        assert_eq!(t.connect(addr(2, 5), 7, 10), None);
        assert_eq!(t.issue(addr(1, 6), 0, || 8), 8);
        assert!(t.connect(addr(1, 6), 8, 10).is_some());
    }

    #[test]
    fn a_full_table_overwrites_the_oldest_written_slot() {
        let mut t = table();
        for i in 0..MAX_CHALLENGES {
            t.issue(addr(1, i as u16), 100 + i as i32, || i as i32);
        }
        // Asking again does not make a slot younger.
        t.issue(addr(1, 0), 99_999, || unreachable!());
        assert_eq!(t.issue(addr(9, 9), 100_000, || -1), -1);
        assert_eq!(t.open(addr(1, 0)), None);
        assert_eq!(t.open(addr(1, 1)), Some(1));
        // A connected slot is overwritten as readily.
        t.connect(addr(1, 1), 1, 100_000);
        t.issue(addr(9, 10), 100_001, || -2);
        assert!(t.connect(addr(1, 1), 1, 100_001).is_none());
    }

    #[test]
    fn a_connected_slot_is_passed_over_until_the_drop() {
        let mut t = table();
        t.issue(addr(1, 5), 10, || 7);
        assert_eq!(t.connect(addr(1, 5), 7, 50), Some((0, 40)));
        // Its ping stays for a retry until the connect takes a client.
        assert_eq!(t.connect(addr(1, 5), 7, 100), Some((0, 40)));
        t.taken(0);
        assert_eq!(t.connect(addr(1, 5), 7, 100), Some((0, 90)));
        // Connected: a new getchallenge makes a second slot.
        assert_eq!(t.issue(addr(1, 5), 100, || 8), 8);
        t.release(addr(1, 5));
        assert_eq!(t.open(addr(1, 5)), Some(7));
    }

    #[test]
    fn the_guid_lives_in_the_slot_cut_to_32_chars() {
        let mut t = table();
        t.issue(addr(1, 5), 0, || 7);
        t.keep_guid(addr(1, 5), &"ab".repeat(500));
        let (i, _) = t.connect(addr(1, 5), 7, 10).unwrap();
        assert_eq!(t.guid(i), "ab".repeat(16));
        // A flood of distinct senders holds no more than the table's slots.
        for n in 0..100_000u32 {
            let from = SocketAddr::from((n.to_be_bytes(), 1));
            t.issue(from, n as i32, || 1);
            t.keep_guid(from, &"x".repeat(1000));
        }
        assert!(t.slots.iter().all(|s| s.guid.len() <= GUID_MAX));
        assert_eq!(t.slots.len(), MAX_CHALLENGES);
    }

    #[test]
    fn a_ban_refusal_forgets_the_open_slot() {
        let mut t = table();
        t.issue(addr(1, 5), 0, || 7);
        t.forget(addr(1, 5));
        assert_eq!(t.connect(addr(1, 5), 7, 10), None);
    }
}
