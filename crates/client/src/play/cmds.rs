//! The usercmd clock and the outgoing ring
//! (docs/protocol-1.1.md, "How long a cmd is simulated for"; AGENTS.md, "66
//! ms is a pmove chop").

use std::collections::VecDeque;
use vcod_common::net::MAX_MOVE_CMDS;
use vcod_common::net::msg::UserCmd;

/// The cmd interval a 125 fps retail client produces, one cmd per frame.
/// Retail has no fixed sim step; this client picks that rate.
pub const CMD_MS: i32 = 8;
/// `MAX_RELIABLE_COMMANDS`-sized backup ring the ring below is capped to.
pub const CMD_BACKUP: usize = 64;

/// Which server times to build cmds for, one call per rendered frame. Ticks
/// at multiples of [`CMD_MS`] since the last call; a small backward step in
/// the estimate (`NetClient::server_clock_ms` re-anchoring on a
/// snapshot) is absorbed by waiting rather than re-ticking, and a step of a
/// second or more (a new gamestate) restarts the clock instead.
#[derive(Default)]
pub struct CmdClock {
    last: Option<i32>,
}

impl CmdClock {
    /// The server times of the cmds to build now, oldest first. First call
    /// (or a restart) returns `[server_now]`; capped at [`MAX_MOVE_CMDS`],
    /// keeping the most recent ticks when a hitch produced more.
    pub fn due(&mut self, server_now: i32) -> Vec<i32> {
        let Some(last) = self.last else {
            self.last = Some(server_now);
            return vec![server_now];
        };
        if server_now - last <= -1000 {
            self.last = Some(server_now);
            return vec![server_now];
        }
        if server_now < last {
            return Vec::new();
        }
        let k_max = (server_now - last) / CMD_MS;
        if k_max == 0 {
            return Vec::new();
        }
        self.last = Some(last + CMD_MS * k_max);
        let count = (k_max as usize).min(MAX_MOVE_CMDS);
        let start_k = k_max - count as i32 + 1;
        (start_k..=k_max).map(|k| last + CMD_MS * k).collect()
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// `cl_maxpackets`' clamp and `cl_packetdup`'s (CoDMP.exe 0x40b940 and
/// 0x40ba50).
pub const MAX_PACKETS: (i32, i32) = (15, 100);
pub const MAX_PACKET_DUP: i32 = 5;

/// The outgoing cmd history: a capped backup for prediction's replay
/// ([`CmdRing::since`]) and the packets it went out in, so each packet also
/// carries the cmds of the `cl_packetdup` packets before it and a dropped
/// packet's cmds ride the next one (`CL_WritePacket`, CoDMP.exe 0x40ba50;
/// docs/protocol-1.1.md, "The client's send rate").
#[derive(Default)]
pub struct CmdRing {
    backup: VecDeque<UserCmd>,
    /// Per packet sent, newest last: its local ms and the time of the newest
    /// cmd it carried (retail's `outPackets[]`).
    sent: VecDeque<(i32, i32)>,
}

impl CmdRing {
    pub fn push(&mut self, cmd: UserCmd) {
        if self.backup.len() == CMD_BACKUP {
            self.backup.pop_front();
        }
        self.backup.push_back(cmd);
    }

    /// `CL_ReadyToSendPacket` (CoDMP.exe 0x40b940): a LAN server gets every
    /// frame's packet, anything else one per `1000 / cl_maxpackets` ms.
    pub fn packet_due(&self, realtime: i32, lan: bool, max_packets: i32) -> bool {
        let Some(&(last, _)) = self.sent.back() else {
            return true;
        };
        let (lo, hi) = MAX_PACKETS;
        lan || realtime - last >= 1000 / max_packets.clamp(lo, hi)
    }

    /// The cmds the packet sent at `realtime` carries: every one past the
    /// newest of the packet `dup` before the last, at most [`MAX_MOVE_CMDS`]
    /// with the most recent kept.
    pub fn packet(&mut self, realtime: i32, dup: i32) -> Vec<UserCmd> {
        let dup = dup.clamp(0, MAX_PACKET_DUP) as usize;
        let after = self
            .sent
            .len()
            .checked_sub(dup + 1)
            .map_or(i32::MIN, |i| self.sent[i].1);
        let mut packet: Vec<UserCmd> = self.since(after).copied().collect();
        if packet.len() > MAX_MOVE_CMDS {
            packet.drain(0..packet.len() - MAX_MOVE_CMDS);
        }
        let newest = self.backup.back().map_or(after, |c| c.server_time);
        if self.sent.len() > MAX_PACKET_DUP as usize {
            self.sent.pop_front();
        }
        self.sent.push_back((realtime, newest));
        packet
    }

    /// Cmds with `server_time >` the argument, oldest first.
    pub fn since(&self, server_time: i32) -> impl Iterator<Item = &UserCmd> {
        self.backup
            .iter()
            .filter(move |c| c.server_time > server_time)
    }

    pub fn clear(&mut self) {
        self.backup.clear();
        self.sent.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_builds_one_cmd_now() {
        let mut c = CmdClock::default();
        assert_eq!(c.due(1000), vec![1000]);
        assert!(c.due(1005).is_empty());
        assert_eq!(c.due(1017), vec![1008, 1016]);
    }

    #[test]
    fn ticks_are_strictly_increasing() {
        let mut c = CmdClock::default();
        c.due(0);
        let t = c.due(10_000);
        assert_eq!(t.len(), MAX_MOVE_CMDS);
        assert!(t.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(*t.last().unwrap(), 10_000 - 10_000 % CMD_MS);
    }

    #[test]
    fn a_small_step_back_waits() {
        let mut c = CmdClock::default();
        c.due(5000);
        assert!(c.due(4960).is_empty());
        assert_eq!(c.due(5010), vec![5008]);
    }

    #[test]
    fn a_clock_that_goes_back_restarts() {
        let mut c = CmdClock::default();
        c.due(5000);
        assert_eq!(c.due(100), vec![100]);
    }

    fn cmd(t: i32) -> UserCmd {
        UserCmd {
            server_time: t,
            ..Default::default()
        }
    }

    fn times(p: &[UserCmd]) -> Vec<i32> {
        p.iter().map(|c| c.server_time).collect()
    }

    #[test]
    fn packet_repeats_the_previous_packets_cmds() {
        let mut r = CmdRing::default();
        r.push(cmd(8));
        r.push(cmd(16));
        assert_eq!(times(&r.packet(0, 1)), vec![8, 16]);
        r.push(cmd(24));
        assert_eq!(times(&r.packet(33, 1)), vec![8, 16, 24]);
        r.push(cmd(32));
        assert_eq!(times(&r.packet(66, 1)), vec![24, 32]);
        // No new cmd: the last packet's go again.
        assert_eq!(times(&r.packet(99, 1)), vec![32]);
        r.push(cmd(40));
        assert_eq!(times(&r.packet(132, 0)), vec![40]);
        r.push(cmd(48));
        assert_eq!(times(&r.packet(165, 2)), vec![40, 48]);
    }

    #[test]
    fn packets_wait_for_maxpackets_off_the_lan() {
        let mut r = CmdRing::default();
        assert!(r.packet_due(0, false, 30));
        r.packet(0, 1);
        assert!(!r.packet_due(32, false, 30));
        assert!(r.packet_due(33, false, 30));
        assert!(r.packet_due(1, true, 30));
        // Clamped to 15..=100.
        assert!(r.packet_due(10, false, 1000));
        assert!(!r.packet_due(65, false, 1));
        assert!(r.packet_due(66, false, 1));
    }

    #[test]
    fn since_returns_newer_cmds_oldest_first() {
        let mut r = CmdRing::default();
        for t in (8..=80).step_by(8) {
            r.push(cmd(t));
        }
        assert_eq!(
            r.since(56).map(|c| c.server_time).collect::<Vec<_>>(),
            vec![64, 72, 80]
        );
    }

    #[test]
    fn ring_keeps_the_last_cmd_backup() {
        let mut r = CmdRing::default();
        for t in 0..100 {
            r.push(cmd(t));
        }
        assert_eq!(r.since(i32::MIN).count(), CMD_BACKUP);
        assert_eq!(r.since(i32::MIN).next().unwrap().server_time, 36);
    }
}
