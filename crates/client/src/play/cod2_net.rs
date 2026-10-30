//! A CoD2 session's link counters for the F3 overlay and `--net-probe`:
//! `cl.snap.ping` as `CL_ParseSnapshot` (Mac 0x1d5cb2) measures it, and
//! the last second's snapshots, messages, packets sent, packets lost and
//! fragments (docs/research/cod2-protocol.md 18.4); the lagometer's two
//! rings and the frames' snapshot pairs (18.5, 18.6).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// `cl.outPackets`' depth, `PACKET_BACKUP` 32.
const PACKET_BACKUP: usize = 32;
/// `cl.snap.ping` when no kept packet carried a cmd the snapshot ran.
pub const NO_PING: i32 = 999;

#[derive(Default)]
pub struct NetMeter {
    /// Each sent packet's newest cmd time (`p_serverTime`, 0 for a packet
    /// with none) and when it went (`p_realtime`), newest last.
    out: VecDeque<(i32, Instant)>,
    /// `cl.snap.ping` of the newest snapshot.
    pub ping: i32,
    /// Instants of the last second's events.
    snaps: VecDeque<Instant>,
    messages: VecDeque<Instant>,
    sent: VecDeque<Instant>,
    lost: VecDeque<(Instant, u32)>,
    frags: VecDeque<Instant>,
    /// Since the session started.
    pub fragments: u64,
    pub lost_total: u64,
}

/// The last second's counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rates {
    pub snaps: usize,
    pub messages: usize,
    pub sent: usize,
    pub lost: u32,
    pub fragments: usize,
}

impl NetMeter {
    /// A packet went out at `now`; `last_cmd` the time of the newest cmd
    /// in it.
    pub fn sent(&mut self, last_cmd: Option<i32>, now: Instant) {
        self.out.push_back((last_cmd.unwrap_or(0), now));
        while self.out.len() > PACKET_BACKUP {
            self.out.pop_front();
        }
        self.sent.push_back(now);
    }

    /// A datagram of a sequenced message came in; `fragment` when it is one
    /// piece of a bigger one, `lost` the sequence numbers it skipped.
    pub fn packet(&mut self, fragment: bool, lost: u32, now: Instant) {
        if fragment {
            self.fragments += 1;
            self.frags.push_back(now);
        }
        if lost > 0 {
            self.lost_total += u64::from(lost);
            self.lost.push_back((now, lost));
        }
    }

    /// A whole message was parsed.
    pub fn message(&mut self, now: Instant) {
        self.messages.push_back(now);
    }

    /// A snapshot came in whose playerstate ran the cmds up to
    /// `command_time`: the ping is the age of the newest kept packet none
    /// of whose cmds is past it (`CL_ParseSnapshot`'s walk back over
    /// `cl.outPackets`, at most 32).
    pub fn snapshot(&mut self, command_time: i32, now: Instant) {
        self.snaps.push_back(now);
        self.ping = self
            .out
            .iter()
            .rev()
            .find(|(t, _)| *t <= command_time)
            .map_or(NO_PING, |(_, at)| (now - *at).as_millis() as i32);
    }

    /// The counts of the second before `now`.
    pub fn rates(&mut self, now: Instant) -> Rates {
        let since = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
        let prune = |q: &mut VecDeque<Instant>| {
            while q.front().is_some_and(|t| *t < since) {
                q.pop_front();
            }
            q.len()
        };
        while self.lost.front().is_some_and(|(t, _)| *t < since) {
            self.lost.pop_front();
        }
        Rates {
            snaps: prune(&mut self.snaps),
            messages: prune(&mut self.messages),
            sent: prune(&mut self.sent),
            lost: self.lost.iter().map(|(_, n)| n).sum(),
            fragments: prune(&mut self.frags),
        }
    }
}

/// The lagometer's rings' depth, `& 0x7f`.
const LAG_RING: usize = 128;

/// `CG_AddLagometerFrameInfo` and `CG_AddLagometerSnapshotInfo` (Mac
/// 0x1cd7c0, 0x1cd7f2): each frame's `cg.time` less the newest snapshot's
/// time as the last `CG_ProcessSnapshots` read it, and each snapshot the
/// cgame asked for, its ping and `snapFlags`, or -1 for one refused.
#[derive(Clone)]
pub struct Lagometer {
    frames: [i32; LAG_RING],
    frame_count: usize,
    snaps: [(i32, u8); LAG_RING],
    snap_count: usize,
    /// `cg.latestSnapshotTime`.
    pub latest: i32,
    /// Each parsed snapshot's message number, `cl.snap.ping` and
    /// `snapFlags`, newest last, at most [`PACKET_BACKUP`].
    parsed: VecDeque<(i32, i32, u8)>,
}

impl Default for Lagometer {
    fn default() -> Self {
        Lagometer {
            frames: [0; LAG_RING],
            frame_count: 0,
            snaps: [(0, 0); LAG_RING],
            snap_count: 0,
            latest: 0,
            parsed: VecDeque::new(),
        }
    }
}

impl Lagometer {
    /// A snapshot came in with this ping (`CL_ParseSnapshot`).
    pub fn parsed(&mut self, message_num: i32, ping: i32, flags: u8) {
        self.parsed.push_back((message_num, ping, flags));
        while self.parsed.len() > PACKET_BACKUP {
            self.parsed.pop_front();
        }
    }

    /// A frame at cg time `t`, before its `CG_ProcessSnapshots`.
    pub fn frame(&mut self, t: i32) {
        self.frames[self.frame_count % LAG_RING] = t - self.latest;
        self.frame_count += 1;
    }

    /// The message numbers `CG_ProcessSnapshots` asked for, each with
    /// whether a snapshot came back.
    pub fn read(&mut self, read: &[(i32, bool)]) {
        for &(n, ok) in read {
            let got = ok
                .then(|| self.parsed.iter().find(|p| p.0 == n))
                .flatten()
                .map_or((-1, 0), |&(_, ping, flags)| (ping, flags));
            self.snaps[self.snap_count % LAG_RING] = got;
            self.snap_count += 1;
        }
    }

    /// The newest `n` frame offsets, newest first.
    pub fn frames(&self, n: usize) -> impl Iterator<Item = i32> + '_ {
        (1..=n.min(LAG_RING)).map(|i| self.frames[self.frame_count.wrapping_sub(i) % LAG_RING])
    }

    /// The newest `n` snapshot entries (ping or -1, `snapFlags`), newest
    /// first. Unwritten slots read 0, as the cgame's zeroed arrays do.
    pub fn snaps(&self, n: usize) -> impl Iterator<Item = (i32, u8)> + '_ {
        (1..=n.min(LAG_RING)).map(|i| self.snaps[self.snap_count.wrapping_sub(i) % LAG_RING])
    }
}

/// How the frames drew their snapshot pair, counted since the session
/// started: `CG_ProcessSnapshots`' pair, and for comparison the newest two
/// alone, which vcod drew before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LerpStats {
    pub frames: u64,
    /// The time sat before the pair's older snapshot: drawn held there.
    pub held: u64,
    /// The time ran past the newest snapshot: nothing to lerp to.
    pub extrapolated: u64,
    /// The same two counts had only the newest two snapshots been kept.
    pub two_held: u64,
    pub two_extrapolated: u64,
}

impl LerpStats {
    /// One frame at clock time `t`, drawn at `drawn` between `cur` and
    /// `next` (server times; the same when there is nothing to lerp to),
    /// the newest two snapshots at `prev` and `newest`.
    pub fn frame(
        &mut self,
        t: i32,
        drawn: i32,
        (cur, next): (i32, i32),
        (prev, newest): (i32, i32),
    ) {
        self.frames += 1;
        if t < cur {
            self.held += 1;
        } else if cur == next && drawn > cur {
            self.extrapolated += 1;
        }
        if t < prev {
            self.two_held += 1;
        } else if t > newest {
            self.two_extrapolated += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_is_the_newest_packet_the_snapshot_ran() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut m = NetMeter::default();
        m.sent(Some(1000), ms(0));
        m.sent(Some(1033), ms(33));
        m.sent(Some(1066), ms(66));
        // The server ran up to 1040: the packet with 1033 is the newest
        // whose cmds it had, sent 33 ms in.
        m.snapshot(1040, ms(120));
        assert_eq!(m.ping, 87);
        m.snapshot(900, ms(130));
        assert_eq!(m.ping, NO_PING);
    }

    #[test]
    fn rates_count_the_last_second() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut m = NetMeter::default();
        for i in 0..30 {
            m.snapshot(0, ms(i * 50));
        }
        m.packet(true, 2, ms(1400));
        let r = m.rates(ms(1450));
        assert_eq!(r.snaps, 21);
        assert_eq!((r.lost, r.fragments), (2, 1));
    }

    /// Frames record how far the clock sat from the newest snapshot;
    /// snapshots their ping, a refused one -1.
    #[test]
    fn lagometer_rings() {
        let mut l = Lagometer {
            latest: 1000,
            ..Default::default()
        };
        l.frame(970);
        l.frame(1012);
        assert_eq!(l.frames(3).collect::<Vec<_>>(), [12, -30, 0]);
        l.parsed(7, 80, 0);
        l.parsed(9, 95, 1);
        l.read(&[(7, true), (8, false), (9, true)]);
        assert_eq!(
            l.snaps(4).collect::<Vec<_>>(),
            [(95, 1), (-1, 0), (80, 0), (0, 0)]
        );
    }
}
