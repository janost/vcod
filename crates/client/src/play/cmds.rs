//! The usercmd clock and the outgoing ring
//! (docs/protocol-1.1.md, "How long a cmd is simulated for"; AGENTS.md, "66
//! ms is a pmove chop").

use vcod_common::net::msg::UserCmd;
use vcod_common::net::MAX_MOVE_CMDS;

/// The cmd interval a 125 fps retail client produces, one cmd per frame.
/// Retail has no fixed sim step; this client picks that rate.
pub const CMD_MS: i32 = 8;

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

/// The previous-packet-plus-new packing every CoD client sends
/// (`docs/protocol-1.1.md`, "Client to server message body"), so a dropped
/// packet's cmds ride the next one.
#[derive(Default)]
pub struct CmdRing {
    /// The `new` cmds handed to the last [`CmdRing::packet`] call, repeated
    /// ahead of the next one.
    last_new: Vec<UserCmd>,
}

impl CmdRing {
    /// The previous packet's new cmds followed by `new`, at most
    /// [`MAX_MOVE_CMDS`] with the most recent kept.
    pub fn packet(&mut self, new: &[UserCmd]) -> Vec<UserCmd> {
        let mut packet: Vec<UserCmd> = self
            .last_new
            .iter()
            .copied()
            .chain(new.iter().copied())
            .collect();
        if packet.len() > MAX_MOVE_CMDS {
            let excess = packet.len() - MAX_MOVE_CMDS;
            packet.drain(0..excess);
        }
        self.last_new = new.to_vec();
        packet
    }

    pub fn clear(&mut self) {
        self.last_new.clear();
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

    #[test]
    fn packet_repeats_the_previous_packets_cmds() {
        let mut r = CmdRing::default();
        let p1 = r.packet(&[cmd(8), cmd(16)]);
        assert_eq!(
            p1.iter().map(|c| c.server_time).collect::<Vec<_>>(),
            vec![8, 16]
        );
        let p2 = r.packet(&[cmd(24)]);
        assert_eq!(
            p2.iter().map(|c| c.server_time).collect::<Vec<_>>(),
            vec![8, 16, 24]
        );
        let p3 = r.packet(&[cmd(32)]);
        assert_eq!(
            p3.iter().map(|c| c.server_time).collect::<Vec<_>>(),
            vec![24, 32]
        );
    }
}
