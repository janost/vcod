//! A bad network on demand, for measuring the client off loopback without
//! root: `VCOD_NETSIM="ping=100,jitter=20,loss=2"` delays, jitters and drops
//! every datagram a [`super::UdpTransport`] sends or receives. `ping` is the
//! added round trip in ms, half of it each way; each datagram's one-way delay
//! varies by up to `jitter / 2` either side of that; `loss` is the percentage
//! dropped in each direction. Datagrams keep their order, as queueing jitter
//! does.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NetSimConfig {
    pub ping_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
}

impl NetSimConfig {
    /// `key=value` pairs separated by commas; `None` when nothing is set or
    /// a pair does not parse.
    pub fn parse(s: &str) -> Option<NetSimConfig> {
        let mut c = NetSimConfig::default();
        for pair in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=')?;
            let v: f64 = v.trim().parse().ok().filter(|v: &f64| *v >= 0.0)?;
            match k.trim() {
                "ping" => c.ping_ms = v,
                "jitter" => c.jitter_ms = v,
                "loss" => c.loss_pct = v.min(100.0),
                _ => return None,
            }
        }
        (c != NetSimConfig::default()).then_some(c)
    }

    pub fn from_env() -> Option<NetSimConfig> {
        let s = std::env::var("VCOD_NETSIM").ok()?;
        let c = Self::parse(&s);
        if c.is_none() {
            log::warn!("VCOD_NETSIM={s:?} not understood; want ping=MS,jitter=MS,loss=PCT");
        }
        c
    }
}

/// One direction's queue.
#[derive(Default)]
struct Lane {
    queue: VecDeque<(Instant, Vec<u8>)>,
}

impl Lane {
    fn push(&mut self, at: Instant, data: &[u8]) {
        // Order kept: a datagram never leaves before the one ahead of it.
        let at = self.queue.back().map_or(at, |&(last, _)| at.max(last));
        self.queue.push_back((at, data.to_vec()));
    }

    fn pop_due(&mut self, now: Instant) -> Option<Vec<u8>> {
        if self.queue.front()?.0 > now {
            return None;
        }
        self.queue.pop_front().map(|(_, d)| d)
    }
}

pub struct NetSim {
    cfg: NetSimConfig,
    rng: u64,
    incoming: Lane,
    outgoing: Lane,
}

impl NetSim {
    pub fn new(cfg: NetSimConfig, seed: u64) -> NetSim {
        NetSim {
            cfg,
            rng: seed | 1,
            incoming: Lane::default(),
            outgoing: Lane::default(),
        }
    }

    /// Uniform in [0, 1).
    fn next(&mut self) -> f64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// When a datagram handed over at `now` comes out, `None` if it is lost.
    fn schedule(&mut self, now: Instant) -> Option<Instant> {
        if self.next() * 100.0 < self.cfg.loss_pct {
            return None;
        }
        let ms = self.cfg.ping_ms / 2.0 + (self.next() - 0.5) * self.cfg.jitter_ms;
        Some(now + Duration::from_secs_f64(ms.max(0.0) / 1000.0))
    }

    /// A datagram the socket received at `now`.
    pub fn arrive(&mut self, now: Instant, data: &[u8]) {
        if let Some(at) = self.schedule(now) {
            self.incoming.push(at, data);
        }
    }

    /// The next received datagram due by `now`.
    pub fn take_incoming(&mut self, now: Instant) -> Option<Vec<u8>> {
        self.incoming.pop_due(now)
    }

    /// A datagram the client sends at `now`.
    pub fn depart(&mut self, now: Instant, data: &[u8]) {
        if let Some(at) = self.schedule(now) {
            self.outgoing.push(at, data);
        }
    }

    /// The next sent datagram due on the wire by `now`.
    pub fn take_outgoing(&mut self, now: Instant) -> Option<Vec<u8>> {
        self.outgoing.pop_due(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_knobs() {
        assert_eq!(
            NetSimConfig::parse("ping=100, jitter=20,loss=2"),
            Some(NetSimConfig {
                ping_ms: 100.0,
                jitter_ms: 20.0,
                loss_pct: 2.0
            })
        );
        assert_eq!(NetSimConfig::parse(""), None);
        assert_eq!(NetSimConfig::parse("ping=x"), None);
        assert_eq!(NetSimConfig::parse("lag=5"), None);
    }

    #[test]
    fn delays_jitters_drops_and_keeps_order() {
        let cfg = NetSimConfig {
            ping_ms: 100.0,
            jitter_ms: 20.0,
            loss_pct: 10.0,
        };
        let mut sim = NetSim::new(cfg, 7);
        let t0 = Instant::now();
        for i in 0..1000u32 {
            sim.arrive(t0 + Duration::from_millis(u64::from(i)), &i.to_le_bytes());
        }
        assert!(sim.take_incoming(t0 + Duration::from_millis(39)).is_none());
        let mut got = Vec::new();
        while let Some(d) = sim.take_incoming(t0 + Duration::from_secs(5)) {
            got.push(u32::from_le_bytes(d[..4].try_into().unwrap()));
        }
        assert!(got.windows(2).all(|w| w[0] < w[1]), "order kept");
        let lost = 1000 - got.len();
        assert!((60..140).contains(&lost), "about 10% lost: {lost}");
    }
}
