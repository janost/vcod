//! `Sys_IsLANAddress`: an IPv4 address is on the LAN when it shares its
//! class's network part with one of the host's own addresses
//! (docs/research/cod11-server-handshake.md, "Rate").

use std::net::{IpAddr, Ipv4Addr};
use std::sync::OnceLock;

/// CoDMP.exe 0x464be0 / cod_lnxded 0x80c72f8 against `locals`. Class A
/// compares the first octet, B two, C three; 172.16/12 and 192.168/16 also
/// match a local address anywhere in the same block. The client's extra
/// `127.0.0.1` check is covered by loopback being in every table.
pub fn is_lan_with(ip: Ipv4Addr, locals: &[Ipv4Addr]) -> bool {
    let a = ip.octets();
    if a == [127, 0, 0, 1] {
        return true;
    }
    locals.iter().any(|l| {
        let l = l.octets();
        if a[0] & 0x80 == 0 {
            a[0] == l[0]
        } else if a[0] & 0xc0 == 0x80 {
            a[..2] == l[..2]
                || (a[0] == 172 && l[0] == 172 && a[1] & 0xf0 == 16 && l[1] & 0xf0 == 16)
        } else {
            a[..3] == l[..3] || (a[..2] == [192, 168] && l[..2] == [192, 168])
        }
    })
}

/// [`is_lan_with`] against this host's addresses. IPv6 counts only as
/// loopback; retail has no IPv6.
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_lan_with(v4, local_addresses()),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// The host's IPv4 addresses, read once. Retail's `NET_GetLocalAddress`
/// resolves its own hostname (at most 16); vcod lists the interfaces
/// instead, since a Linux hostname often resolves to 127.0.1.1 alone.
/// Loopback is always in it.
pub fn local_addresses() -> &'static [Ipv4Addr] {
    static LOCALS: OnceLock<Vec<Ipv4Addr>> = OnceLock::new();
    LOCALS.get_or_init(|| {
        let mut v = vec![Ipv4Addr::LOCALHOST];
        for ip in interface_addresses() {
            if !v.contains(&ip) {
                v.push(ip);
            }
        }
        log::debug!("local addresses: {v:?}");
        v
    })
}

#[cfg(unix)]
fn interface_addresses() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we walk read-only and free
    // once; each `ifa_addr` is checked for null and its family before the cast.
    unsafe {
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            let addr = (*cur).ifa_addr;
            if !addr.is_null() && i32::from((*addr).sa_family) == libc::AF_INET {
                let sin = &*(addr as *const libc::sockaddr_in);
                out.push(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)));
            }
            cur = (*cur).ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

/// Retail's own way on Windows: whatever the computer name resolves to.
#[cfg(not(unix))]
fn interface_addresses() -> Vec<Ipv4Addr> {
    use std::net::ToSocketAddrs;
    let Ok(name) = std::env::var("COMPUTERNAME") else {
        return Vec::new();
    };
    (name.as_str(), 0)
        .to_socket_addrs()
        .map(|it| {
            it.filter_map(|a| match a.ip() {
                IpAddr::V4(v4) => Some(v4),
                IpAddr::V6(_) => None,
            })
            .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn the_class_decides_how_much_of_the_address_matches() {
        let locals = [ip("10.1.2.3"), ip("130.5.6.7"), ip("200.1.2.3")];
        assert!(
            is_lan_with(ip("10.200.0.1"), &locals),
            "class A: first octet"
        );
        assert!(!is_lan_with(ip("11.1.2.3"), &locals));
        assert!(
            is_lan_with(ip("130.5.99.1"), &locals),
            "class B: two octets"
        );
        assert!(!is_lan_with(ip("130.6.6.7"), &locals));
        assert!(
            is_lan_with(ip("200.1.2.99"), &locals),
            "class C: three octets"
        );
        assert!(!is_lan_with(ip("200.1.3.3"), &locals));
    }

    #[test]
    fn private_blocks_need_a_local_address_in_the_same_block() {
        assert!(is_lan_with(ip("172.20.0.1"), &[ip("172.31.9.9")]));
        assert!(!is_lan_with(ip("172.20.0.1"), &[ip("10.0.0.1")]));
        assert!(is_lan_with(ip("192.168.7.1"), &[ip("192.168.0.2")]));
        assert!(!is_lan_with(ip("192.168.7.1"), &[ip("127.0.0.1")]));
        // 10/8 has no shortcut of its own; it is class A.
        assert!(!is_lan_with(ip("10.0.0.1"), &[ip("192.168.0.2")]));
    }

    #[test]
    fn loopback_is_always_lan() {
        assert!(is_lan_with(ip("127.0.0.1"), &[]));
        assert!(local_addresses().contains(&Ipv4Addr::LOCALHOST));
        assert!(is_lan(ip("127.0.0.5").into()));
    }
}
