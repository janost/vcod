//! `challenge_t`: the table `SV_GetChallenge`, `SV_DirectConnect` and
//! `SV_DropClient` share. All four retail servers keep it the same way
//! (cod11-server-handshake.md, "The challenge table"): 1024 slots, no
//! expiry, a new challenge over the slot with the lowest creation time,
//! every lookup on the whole address, port included. 1.1d's table is at
//! 0x83b67f8 (`SV_GetChallenge` 0x8084d90), 1.5b's at 0x83ccde0
//! (0x80889d0), UO 1.51b's at 0x84f7060 (0x80897d4), CoD2 1.3's at
//! 0x84230dc (0x808d18e). Times are the caller's server clock in ms.

use std::net::SocketAddr;

/// `MAX_CHALLENGES`: the loop bound 0x400 in all four.
pub const MAX_CHALLENGES: usize = 1024;

#[derive(Clone, Copy, Default)]
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
}

pub struct Challenges {
    slots: Vec<Slot>,
}

impl Default for Challenges {
    fn default() -> Self {
        Challenges {
            slots: vec![Slot::default(); MAX_CHALLENGES],
        }
    }
}

impl Challenges {
    /// `SV_GetChallenge` on its answer-at-once path: the first slot not
    /// connected whose address is `from`, else the slot with the lowest
    /// time (the first of a tie; an unwritten slot's is 0) overwritten
    /// with `draw()`. Either way `pingTime` is `now`. Returns the
    /// challenge to send. `draw` runs only for a new slot, so the caller's
    /// `rand()` draws are retail's.
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
            self.slots[oldest] = Slot {
                addr: Some(from),
                challenge: draw(),
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

    #[test]
    fn an_asking_address_keeps_its_slot_and_nothing_expires() {
        let mut t = Challenges::default();
        let a = t.issue(addr(1, 5), 1000, || 7);
        // An hour on, the slot is still there and still the same.
        assert_eq!(t.issue(addr(1, 5), 3_601_000, || unreachable!()), a);
        assert_eq!(t.connect(addr(1, 5), 7, 3_601_050), Some((0, 50)));
    }

    #[test]
    fn the_whole_address_matches_port_included() {
        let mut t = Challenges::default();
        t.issue(addr(1, 5), 0, || 7);
        assert_eq!(t.connect(addr(1, 6), 7, 10), None);
        assert_eq!(t.connect(addr(2, 5), 7, 10), None);
        assert_eq!(t.issue(addr(1, 6), 0, || 8), 8);
        assert!(t.connect(addr(1, 6), 8, 10).is_some());
    }

    #[test]
    fn a_full_table_overwrites_the_oldest_written_slot() {
        let mut t = Challenges::default();
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
        let mut t = Challenges::default();
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
    fn a_ban_refusal_forgets_the_open_slot() {
        let mut t = Challenges::default();
        t.issue(addr(1, 5), 0, || 7);
        t.forget(addr(1, 5));
        assert_eq!(t.connect(addr(1, 5), 7, 10), None);
    }
}
