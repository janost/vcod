//! `--net-probe ADDR --probe-clock`: a quiet spectator that runs the client's
//! clock ([`crate::play::clock::ServerClock`]) on real snapshot arrivals and
//! prints a `CLOCK` line a second: arrival spacing, snapshots lost, how far
//! behind the newest snapshot the drawn time sits, how often it ran past it,
//! and how unevenly it stepped against the local clock. The same numbers for
//! the fixed 100 ms re-anchoring clock vcod drew with before go on an `OLD`
//! line. Pair with `VCOD_NETSIM` to measure a bad link. Writes no fixture.

use crate::play::clock::ServerClock;
use std::time::{Duration, Instant};
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetState};

/// The render clock this branch replaced: newest snapshot time plus local
/// time since it arrived, 100 ms back, re-anchored on every snapshot and
/// never stepped back.
struct OldClock {
    anchor: Option<(i32, i32)>,
    drawn: i32,
}

impl OldClock {
    fn frame(&mut self, realtime: i32, newest: i32) -> i32 {
        if self.anchor.is_none_or(|(t, _)| t != newest) {
            self.anchor = Some((newest, realtime));
        }
        let (t, at) = self.anchor.unwrap();
        self.drawn = self.drawn.max(t + (realtime - at) - 100);
        self.drawn
    }
}

/// One clock's frames over a window.
#[derive(Default)]
struct Steps {
    frames: u32,
    behind_sum: i64,
    behind_min: i32,
    behind_max: i32,
    /// Frames drawn at or past the newest snapshot: nothing to lerp to.
    past: u32,
    /// Frames the drawn time did not move while local time did.
    held: u32,
    /// Sum and largest `|drawn step - local step|`.
    jerk_sum: i64,
    jerk_max: i32,
    last: Option<(i32, i32)>,
}

impl Steps {
    fn add(&mut self, realtime: i32, t: i32, newest: i32) {
        let behind = newest - t;
        if self.frames == 0 {
            self.behind_min = behind;
            self.behind_max = behind;
        }
        self.frames += 1;
        self.behind_sum += i64::from(behind);
        self.behind_min = self.behind_min.min(behind);
        self.behind_max = self.behind_max.max(behind);
        self.past += u32::from(behind <= 0);
        if let Some((r0, t0)) = self.last {
            let (dr, dt) = (realtime - r0, t - t0);
            self.held += u32::from(dt == 0 && dr > 0);
            let jerk = (dt - dr).abs();
            self.jerk_sum += i64::from(jerk);
            self.jerk_max = self.jerk_max.max(jerk);
        }
        self.last = Some((realtime, t));
    }

    fn line(&self) -> String {
        let n = f64::from(self.frames.max(1));
        format!(
            "behind {}/{:.1}/{} past {:.1}% held {:.1}% jerk {:.2}/{}",
            self.behind_min,
            self.behind_sum as f64 / n,
            self.behind_max,
            100.0 * f64::from(self.past) / n,
            100.0 * f64::from(self.held) / n,
            self.jerk_sum as f64 / n,
            self.jerk_max
        )
    }

    fn reset(&mut self) {
        *self = Steps {
            last: self.last,
            ..Steps::default()
        };
    }
}

/// Snapshot arrivals over a window.
#[derive(Default)]
struct Arrivals {
    count: u32,
    gaps: Vec<i32>,
    lost: u32,
}

impl Arrivals {
    fn line(&self) -> String {
        let n = self.gaps.len().max(1) as f64;
        let mean = self.gaps.iter().map(|&g| f64::from(g)).sum::<f64>() / n;
        let var = self
            .gaps
            .iter()
            .map(|&g| (f64::from(g) - mean).powi(2))
            .sum::<f64>()
            / n;
        format!(
            "snaps {} lost {} gap {:.1}+-{:.1} max {}",
            self.count,
            self.lost,
            mean,
            var.sqrt(),
            self.gaps.iter().max().copied().unwrap_or(0)
        )
    }
}

pub fn run(addr: &str, secs: u64, frame_ms: u64) -> anyhow::Result<()> {
    let mut client = NetClient::connect(addr)?;
    let start = Instant::now();
    let mut clock = ServerClock::default();
    let mut old = OldClock {
        anchor: None,
        drawn: i32::MIN,
    };
    let (mut new_steps, mut old_steps) = (Steps::default(), Steps::default());
    let (mut total_new, mut total_old) = (Steps::default(), Steps::default());
    let mut arrivals = Arrivals::default();
    let mut total_arrivals = Arrivals::default();
    let mut newest: Option<(i32, i32)> = None;
    let mut last_report = start;
    while start.elapsed() < Duration::from_secs(secs) {
        if crate::quit::requested() {
            break;
        }
        let now = Instant::now();
        let realtime = now.duration_since(start).as_millis() as i32;
        for e in client.pump_at(now) {
            if let vcod_common::net::NetEvent::Dropped(why) = e {
                anyhow::bail!("dropped: {why}");
            }
        }
        if client.state() == NetState::Active {
            client.send_frame(&UserCmd::default());
        }
        if let Some(s) = client.snapshots().newest() {
            let snap = s.server_time;
            if newest.is_none_or(|(t, _)| t != snap) {
                if let Some((t, at)) = newest {
                    for a in [&mut arrivals, &mut total_arrivals] {
                        a.count += 1;
                        a.gaps.push(realtime - at);
                        a.lost += ((snap - t) / 50 - 1).max(0) as u32;
                    }
                }
                newest = Some((snap, realtime));
            }
            let t = clock.update(realtime, snap, 0);
            let o = old.frame(realtime, snap);
            new_steps.add(realtime, t, snap);
            total_new.add(realtime, t, snap);
            old_steps.add(realtime, o, snap);
            total_old.add(realtime, o, snap);
        }
        if now.duration_since(last_report) >= Duration::from_secs(1) && newest.is_some() {
            last_report = now;
            println!(
                "CLOCK {} | {} | {:?}",
                arrivals.line(),
                new_steps.line(),
                clock.stats
            );
            println!("OLD   {}", old_steps.line());
            arrivals = Arrivals::default();
            new_steps.reset();
            old_steps.reset();
        }
        std::thread::sleep(Duration::from_millis(frame_ms));
    }
    client.disconnect();
    println!("TOTAL {}", total_arrivals.line());
    println!("TOTAL new {} | {:?}", total_new.line(), clock.stats);
    println!("TOTAL old {}", total_old.line());
    Ok(())
}
