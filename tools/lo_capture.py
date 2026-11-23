#!/usr/bin/env python3
"""Logs every IPv4 UDP datagram and TCP segment on `lo`, a small pcap for a
rootless network namespace (`unshare -rn` grants CAP_NET_RAW there, and the
namespace has no other traffic).

    lo_capture.py OUT.txt [SERVER_PORT]

Each UDP line is `<ms> <c2s|s2c|udp> <peer> <hex>` (the format of
`tools/cod4_handshake_probe.py`, so `cargo run -p vcod-common --example
cod4_capture` reads it): `c2s` is a datagram to SERVER_PORT, `s2c` one from
it, `peer` the client's address; `udp` is any other, with `src>dst` as the
peer. A TCP segment with a payload or a SYN logs as `<ms> tcp <src>><dst>
<flags> <hex>`. Runs until killed.
"""

import socket
import struct
import sys
import time

ETH_P_ALL = 0x0003
PACKET_OUTGOING = 4


def main():
    out = open(sys.argv[1], "w", buffering=1)
    port = int(sys.argv[2]) if len(sys.argv) > 2 else None
    s = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(ETH_P_ALL))
    s.bind(("lo", 0))
    t0 = time.monotonic()
    while True:
        frame, addr = s.recvfrom(262144)
        # Loopback shows each packet twice, leaving and arriving; keep one.
        if addr[2] == PACKET_OUTGOING or len(frame) < 34:
            continue
        if struct.unpack_from("!H", frame, 12)[0] != 0x0800:
            continue
        ip = frame[14:]
        ihl = (ip[0] & 15) * 4
        proto = ip[9]
        src = socket.inet_ntoa(ip[12:16])
        dst = socket.inet_ntoa(ip[16:20])
        body = ip[ihl : struct.unpack_from("!H", ip, 2)[0]]
        ms = int((time.monotonic() - t0) * 1000)
        if proto == 17:
            sp, dp = struct.unpack_from("!HH", body)
            data = body[8:]
            if port is not None and dp == port:
                out.write(f"{ms} c2s {src}:{sp} {data.hex()}\n")
            elif port is not None and sp == port:
                out.write(f"{ms} s2c {dst}:{dp} {data.hex()}\n")
            else:
                out.write(f"{ms} udp {src}:{sp}>{dst}:{dp} {data.hex()}\n")
        elif proto == 6:
            sp, dp = struct.unpack_from("!HH", body)
            flags = body[13]
            data = body[(body[12] >> 4) * 4 :]
            if data or flags & 0x02:
                out.write(f"{ms} tcp {src}:{sp}>{dst}:{dp} {flags:02x} {data.hex()}\n")


if __name__ == "__main__":
    main()
