//! A client's link: the LAN test, the rate and snapshot interval its
//! userinfo sets, message pacing and the ping average
//! (docs/research/cod11-server-handshake.md, "Rate" and "Pings";
//! docs/research/cod2-protocol.md 17).

use std::net::IpAddr;

use vcod_common::text::atoi;

/// `Sys_IsLANAddress`, which the two engines answer differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lan {
    /// cod_lnxded 0x80c72f8 (CoDMP.exe 0x464be0), on 1.1, 1.5 and UO: the
    /// address shares its class's network part with one of the host's own
    /// (`vcod_common::net::lan`).
    Cod1,
    /// cod2_lnxded 0x80d5638 (cod4_lnxded 0x81d59c8 the same): 10/8,
    /// 127/8, 169.254/16, 172.16/12 and 192.168/16, or the first three
    /// octets of one of the host's own addresses.
    Cod2,
}

impl Lan {
    pub fn is_lan(self, ip: IpAddr) -> bool {
        match self {
            Lan::Cod1 => vcod_common::net::lan::is_lan(ip),
            Lan::Cod2 => {
                let IpAddr::V4(v4) = ip else {
                    return ip.is_loopback();
                };
                let a = v4.octets();
                a[0] == 10
                    || a[0] == 127
                    || (a[0] == 169 && a[1] == 254)
                    || (a[0] == 172 && a[1] & 0xf0 == 16)
                    || (a[0] == 192 && a[1] == 168)
                    || vcod_common::net::lan::local_addresses()
                        .iter()
                        .any(|l| l.octets()[..3] == a[..3])
            }
        }
    }
}

/// What `SV_UserinfoChanged` (cod_lnxded 0x8086ab4, cod2_lnxded
/// 0x80907cc) takes out of a userinfo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rates {
    /// `cl->rate`, see [`rate`].
    pub rate: i32,
    /// `cl->snapshotMsec`, see [`snapshot_msec`].
    pub snapshot_msec: i32,
    /// `cl_wwwDownload` non-zero.
    pub wants_www: bool,
}

impl Rates {
    /// `lan` is the [`Lan`] test of a client that has an address.
    pub fn of(userinfo: &str, lan: bool, dedicated: i32) -> Rates {
        Rates {
            rate: rate(userinfo, lan, dedicated),
            snapshot_msec: snapshot_msec(userinfo),
            wants_www: wants_www(userinfo),
        }
    }

    /// The rate under a non-zero `sv_maxRate`, see [`send_rate`].
    pub fn send_rate(&self, max_rate: i32) -> i32 {
        send_rate(self.rate, max_rate)
    }
}

fn key<'a>(userinfo: &'a str, k: &str) -> &'a str {
    vcod_common::net::info_value_for_key(userinfo, k).unwrap_or("")
}

/// `cl->rate`: 99999 for a LAN client on a server below `dedicated 2`,
/// anyone else the userinfo's `rate` clamped to 1000..90000, or 5000 when
/// it is empty. Both engines read it with a saturating `strtol`
/// (cod_lnxded 0x8086b93, cod2_lnxded `atoi` 0x809088a), so a value past
/// `i32` clamps to 90000.
pub fn rate(userinfo: &str, lan: bool, dedicated: i32) -> i32 {
    if dedicated != 2 && lan {
        return 99999;
    }
    match key(userinfo, "rate") {
        "" => 5000,
        r => atoi(r).clamp(1000, 90000),
    }
}

/// `cl->snapshotMsec` (cod_lnxded 0x8086cad..0x8086d44): 50 when `snaps`
/// is empty or missing, else 1000 over `snaps` clamped to 1..30.
pub fn snapshot_msec(userinfo: &str) -> i32 {
    match key(userinfo, "snaps") {
        "" => 50,
        s => 1000 / atoi(s).clamp(1, 30),
    }
}

/// `cl_wwwDownload` non-empty and its `atoi` not 0 (UO 0x808d2c7).
pub fn wants_www(userinfo: &str) -> bool {
    atoi(key(userinfo, "cl_wwwDownload")) != 0
}

/// The rate a message is paced and filled by: `rate` under a non-zero
/// `sv_maxRate`, which reads 1000 at the least (cod_lnxded 0x808f7a5).
pub fn send_rate(rate: i32, max_rate: i32) -> i32 {
    if max_rate == 0 {
        rate
    } else {
        rate.min(max_rate.max(1000))
    }
}

/// `SV_RateMsec` (cod2_lnxded 0x809aad0, inlined in cod_lnxded's
/// 0x808f680): the time `bytes` (capped at 1500, plus 48 of headers) take
/// at `rate`.
pub fn rate_msec(bytes: usize, rate: i32) -> i32 {
    (bytes.min(1500) as i32 * 1000 + 48000) / rate.max(1)
}

/// When a client is due its next message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// A LAN client: the next frame, whatever the clock does
    /// (`svs.time - 1`); `rateDelayed` is left as it was.
    Lan,
    /// `nextSnapshotTime` and `rateDelayed`.
    At { next: i32, delayed: bool },
}

/// `SV_SendMessageToClient`'s `nextSnapshotTime` (cod_lnxded 0x808f680,
/// cod2_lnxded 0x809aba2) after a message of `bytes`: the next frame for a
/// LAN client, else what the rate needs and at least the snapshot
/// interval, `rateDelayed` set when the rate was the larger (0x808f7d8),
/// and a second for an `idle` client, one neither in the world nor
/// downloading.
pub fn pace(
    svs_ms: i32,
    bytes: usize,
    lan: bool,
    rate: i32,
    snapshot_msec: i32,
    idle: bool,
) -> Pace {
    if lan {
        return Pace::Lan;
    }
    let msec = rate_msec(bytes, rate);
    let delayed = msec >= snapshot_msec;
    let mut next = svs_ms + msec.max(snapshot_msec);
    if idle {
        next = next.max(svs_ms + 1000);
    }
    Pace::At { next, delayed }
}

/// `SV_CalcPings`' average (cod_lnxded 0x808cab8, cod2_lnxded 0x80963c8)
/// over the `(sent, acked)` ring: every slot with a positive ack time
/// counts, capped at 999, and 999 with none.
pub fn calc_ping(ring: &[(i32, i32)]) -> i32 {
    let (sum, n) = ring
        .iter()
        .filter(|(_, acked)| *acked > 0)
        .fold((0i32, 0i32), |(s, n), (sent, acked)| {
            (s.wrapping_add(acked - sent), n + 1)
        });
    if n == 0 { 999 } else { (sum / n).min(999) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cod2_rates(userinfo: &str, lan: bool, dedicated: i32) -> Rates {
        Rates::of(userinfo, lan, dedicated)
    }

    #[test]
    fn rates_follow_sv_userinfo_changed() {
        let r = cod2_rates("\\rate\\25000\\snaps\\20", false, 1);
        assert_eq!((r.rate, r.snapshot_msec, r.wants_www), (25000, 50, false));
        assert_eq!(cod2_rates("", false, 1).rate, 5000);
        assert_eq!(cod2_rates("\\rate\\10", false, 1).rate, 1000);
        assert_eq!(cod2_rates("\\rate\\25000", true, 1).rate, 99999);
        assert_eq!(cod2_rates("\\rate\\25000", true, 2).rate, 25000);
        assert_eq!(cod2_rates("\\snaps\\10", false, 1).snapshot_msec, 100);
        assert_eq!(cod2_rates("\\snaps\\99", false, 1).snapshot_msec, 33);
        assert!(cod2_rates("\\cl_wwwDownload\\1", false, 1).wants_www);
        assert_eq!(cod2_rates("\\rate\\25000", false, 1).send_rate(500), 1000);
    }

    /// Past `i32` the read saturates as `strtol` does (cod_lnxded
    /// 0x8086b93, cod2_lnxded 0x809088a), so the clamps still hold.
    #[test]
    fn an_overflowing_rate_saturates_into_the_clamp() {
        let r = cod2_rates("\\rate\\99999999999\\snaps\\99999999999", false, 1);
        assert_eq!((r.rate, r.snapshot_msec), (90000, 33));
        let r = cod2_rates("\\rate\\-99999999999\\snaps\\-99999999999", false, 1);
        assert_eq!((r.rate, r.snapshot_msec), (1000, 1000));
        assert!(cod2_rates("\\cl_wwwDownload\\99999999999", false, 1).wants_www);
    }

    /// The off-LAN pacing captured on CoD2 retail (cod2-protocol.md 17.2):
    /// a 1300-byte fragment at rate 5000 waits 309 ms (the 350 ms seen on
    /// 50 ms frames), at 25000 61 ms (100 ms seen), at 1000 1548 ms; a
    /// 20-byte snapshot waits the snapshot interval.
    #[test]
    fn rate_msec_matches_the_retail_intervals() {
        assert_eq!(rate_msec(1500, 5000), 309);
        assert_eq!(rate_msec(4000, 25000), 61);
        assert_eq!(rate_msec(1500, 1000), 1548);
        let at = |next, delayed| Pace::At { next, delayed };
        assert_eq!(pace(1000, 24, false, 25000, 100, false), at(1100, false));
        assert_eq!(pace(1000, 1500, false, 1000, 50, false), at(2548, true));
        assert_eq!(pace(1000, 24, false, 25000, 50, true), at(2000, false));
        assert_eq!(pace(1000, 24, true, 25000, 50, true), Pace::Lan);
    }

    #[test]
    fn lan_is_the_private_blocks_and_the_hosts_slash_24() {
        let lan = |s: &str| Lan::Cod2.is_lan(s.parse().unwrap());
        assert!(lan("127.0.0.1"));
        assert!(lan("10.1.2.3"));
        assert!(lan("172.31.0.1"));
        assert!(!lan("172.32.0.1"));
        assert!(lan("192.168.9.9"));
        assert!(lan("169.254.1.1"));
        assert!(!lan("198.51.100.2"));
    }
}
