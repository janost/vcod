//! Contains routines ported from the ioquake3 GPL source, Copyright (C) 1999-2005 Id Software, Inc. and the ioquake3 contributors.
//! See NOTICE.
//!
//! The connectionless query limiter all three games put in front of
//! `getinfo`, `getstatus` and `getchallenge`, so a spoofed `getstatus`
//! cannot use the server as a reflector. ioq3's `SVC_RateLimitAddress` and
//! `outboundLeakyBucket`; retail 1.1, CoD2 1.3 and CoD4 1.7 answer every
//! query (docs/protocol-1.1.md, "What vcod's server does that retail does
//! not"). A limited request is dropped silently.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Per source ip: a burst of 10 queries, one back per second.
pub const ADDR_BURST: u32 = 10;
pub const ADDR_PERIOD: Duration = Duration::from_secs(1);
/// All `getinfo` and `getstatus` replies: a burst of 10, one back per 100 ms.
pub const GLOBAL_BURST: u32 = 10;
pub const GLOBAL_PERIOD: Duration = Duration::from_millis(100);
/// Source ips tracked at once; the least recently seen is evicted.
pub const MAX_BUCKETS: usize = 1024;

/// ioq3 `leakyBucket_t`: `used` tokens, one drained per period.
struct Bucket {
    last: Instant,
    used: u32,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Bucket { last: now, used: 0 }
    }

    /// `SVC_RateLimit`. True when this request is over the limit.
    fn limited(&mut self, now: Instant, burst: u32, period: Duration) -> bool {
        let interval = now.saturating_duration_since(self.last);
        let expired = interval.as_micros() / period.as_micros();
        if expired > u128::from(self.used) {
            self.used = 0;
            self.last = now;
        } else {
            self.used -= expired as u32;
            self.last += period * expired as u32;
        }
        if self.used < burst {
            self.used += 1;
            return false;
        }
        true
    }
}

/// A bucket per source ip in front of one for all status replies.
#[derive(Default)]
pub struct RateLimiter {
    /// Started at the first query, on the caller's clock.
    global: Option<Bucket>,
    addrs: HashMap<IpAddr, Bucket>,
}

impl RateLimiter {
    /// Whether the connectionless command `cmd` (lower case) from `from`
    /// goes unanswered. `getinfo` and `getstatus` take the address's bucket
    /// and then the global one, in ioq3's order; `getchallenge` only the
    /// address's, so a flood of spoofed status queries cannot hold joins
    /// off. Anything else passes.
    pub fn limited(&mut self, cmd: &str, from: IpAddr, now: Instant) -> bool {
        match cmd {
            "getinfo" | "getstatus" => {
                self.addr_limited(from, now)
                    || self.global.get_or_insert_with(|| Bucket::new(now)).limited(
                        now,
                        GLOBAL_BURST,
                        GLOBAL_PERIOD,
                    )
            }
            "getchallenge" => self.addr_limited(from, now),
            _ => false,
        }
    }

    /// `SVC_RateLimitAddress`. The global bucket is untouched by a request
    /// this rejects.
    fn addr_limited(&mut self, from: IpAddr, now: Instant) -> bool {
        if !self.addrs.contains_key(&from) && self.addrs.len() >= MAX_BUCKETS {
            let oldest = *self.addrs.iter().min_by_key(|(_, b)| b.last).unwrap().0;
            self.addrs.remove(&oldest);
        }
        self.addrs
            .entry(from)
            .or_insert_with(|| Bucket::new(now))
            .limited(now, ADDR_BURST, ADDR_PERIOD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(i: u32) -> IpAddr {
        IpAddr::from(i.to_be_bytes())
    }

    #[test]
    fn the_address_table_is_bounded() {
        let now = Instant::now();
        let mut l = RateLimiter::default();
        for i in 0..MAX_BUCKETS as u32 + 8 {
            // Spread over time so the global bucket stays open and the
            // eviction order is defined.
            assert!(!l.limited("getinfo", ip(i), now + GLOBAL_PERIOD * i));
        }
        assert_eq!(l.addrs.len(), MAX_BUCKETS);
        // The least recently seen addresses went first.
        assert!(!l.addrs.contains_key(&ip(0)));
        assert!(l.addrs.contains_key(&ip(MAX_BUCKETS as u32 + 7)));
    }

    #[test]
    fn a_challenge_is_not_held_off_by_a_status_flood() {
        let now = Instant::now();
        let mut l = RateLimiter::default();
        let answered = (0..100)
            .filter(|&i| !l.limited("getstatus", ip(i), now))
            .count();
        assert_eq!(answered, GLOBAL_BURST as usize);
        assert!(!l.limited("getchallenge", ip(1000), now));
        // Its own address's bucket still applies.
        for _ in 1..ADDR_BURST {
            assert!(!l.limited("getchallenge", ip(1000), now));
        }
        assert!(l.limited("getchallenge", ip(1000), now));
    }
}
