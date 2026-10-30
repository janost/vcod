#!/usr/bin/env python3
"""CoD4 1.7 handshake probe: walks a retail cod4_lnxded through
getinfo, getstatus, getchallenge, connect, the seven stats packets and the
gamestate, and logs every datagram both ways as raw hex.

Run it only against a server on loopback inside `unshare -rn` (the recipe is
in docs/research/cod4-handshake.md, section 1). It decodes nothing past the
out-of-band text; `cargo run -p vcod-common --example cod4_capture` reads the
netchan side of the log.

    cod4_handshake_probe.py HOST:PORT OUT.txt [--auth 127.0.0.2]

Log lines: `<ms since start> <c2s|s2c|auth> <peer> <hex>`. `auth` lines are
datagrams the server sent to the address `--auth` listens on (the master and
authorize name resolved there through the namespace's hosts file).
"""

import hashlib
import select
import socket
import struct
import sys
import time

QPORT = 0x1234
USERINFO = (
    "\\cg_predictItems\\1\\cl_punkbuster\\0\\cl_anonymous\\0\\color\\4"
    "\\head\\default\\model\\multi\\snaps\\20\\rate\\25000\\name\\vcodprobe"
)
STAT_PACKETS = 7
STAT_BYTES = 0x2000
STAT_CHUNK = 1240


class Probe:
    def __init__(self, server, out, auth_ip):
        self.server = server
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", 0))
        self.auth = []
        if auth_ip:
            for port in (20800, 20810):
                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                s.bind((auth_ip, port))
                self.auth.append(s)
        self.out = open(out, "w")
        self.t0 = time.monotonic()

    def log(self, tag, peer, data):
        ms = int((time.monotonic() - self.t0) * 1000)
        self.out.write(f"{ms} {tag} {peer[0]}:{peer[1]} {data.hex()}\n")
        self.out.flush()

    def send(self, data):
        self.sock.sendto(data, self.server)
        self.log("c2s", self.server, data)

    def oob(self, text):
        self.send(b"\xff\xff\xff\xff" + text)

    def recv(self, timeout):
        """Next datagram from the server, or None. Logs authorize traffic."""
        end = time.monotonic() + timeout
        while True:
            left = end - time.monotonic()
            if left <= 0:
                return None
            r, _, _ = select.select([self.sock] + self.auth, [], [], left)
            for s in r:
                data, peer = s.recvfrom(65536)
                if s is self.sock:
                    self.log("s2c", peer, data)
                    return data
                self.log("auth", s.getsockname(), data)

    def oob_reply(self, prefix, timeout, resend=None, every=3.0):
        """Waits for an OOB reply starting with `prefix`, resending `resend`."""
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if resend:
                self.oob(resend)
            step = min(every, end - time.monotonic())
            stop = time.monotonic() + step
            while time.monotonic() < stop:
                d = self.recv(stop - time.monotonic())
                if d and d[:4] == b"\xff\xff\xff\xff" and d[4:].startswith(prefix):
                    return d[4:]
        return None


def main():
    host, port = sys.argv[1].rsplit(":", 1)
    auth_ip = sys.argv[sys.argv.index("--auth") + 1] if "--auth" in sys.argv else None
    p = Probe((host, int(port)), sys.argv[2], auth_ip)

    info = p.oob_reply(b"infoResponse", 6, b"getinfo xxx")
    p.oob_reply(b"statusResponse", 6, b"getstatus xxx")
    protocol = "6"
    if info:
        kv = info.split(b"\n", 1)[1].decode("latin-1").split("\\")[1:]
        protocol = dict(zip(kv[::2], kv[1::2])).get("protocol", protocol)

    key = hashlib.md5(b"vcodprobe").hexdigest()
    chal = p.oob_reply(b"challengeResponse", 30, f'getchallenge 0 "{key}"'.encode())
    if not chal:
        print("no challengeResponse", file=sys.stderr)
        return 1
    challenge = int(chal.split()[1])

    # One connect with the next protocol number first, for the refusal text.
    bad = f"{USERINFO}\\protocol\\{int(protocol) + 1}\\challenge\\{challenge}\\qport\\{QPORT}"
    p.oob_reply(b"error", 3, f'connect "{bad}"'.encode(), every=3.0)

    ui = f"{USERINFO}\\protocol\\{protocol}\\challenge\\{challenge}\\qport\\{QPORT}"
    if not p.oob_reply(b"connectResponse", 10, f'connect "{ui}"'.encode()):
        print("no connectResponse", file=sys.stderr)
        return 1

    # Stats: one packet per index the server still wants, until its mask is 0.
    want = (1 << STAT_PACKETS) - 1
    deadline = time.monotonic() + 15
    while want and time.monotonic() < deadline:
        idx = (want & -want).bit_length() - 1
        size = min(STAT_CHUNK, STAT_BYTES - STAT_CHUNK * idx)
        p.oob(b"stats\0" + struct.pack("<hB", QPORT, idx) + bytes(size))
        stop = time.monotonic() + 0.5
        while (r := p.recv(max(0.0, stop - time.monotonic()))) is not None:
            if r[4:].startswith(b"statResponse"):
                want = int(r[4:].split()[1])
                break

    # Netchan: an empty message (the 9-byte header only) every 50 ms. It acks
    # the newest sequence seen until the first fragmented message (the
    # gamestate) starts, then holds that sequence: a later ack with the old
    # serverId makes the server resend the gamestate.
    seq = 1
    ack = 0
    held = None
    quiet_until = time.monotonic() + 8
    hard_stop = time.monotonic() + 20
    while time.monotonic() < min(quiet_until, hard_stop):
        hdr = struct.pack("<IhBii", seq, QPORT, 0, ack, 0)
        p.send(hdr)
        seq += 1
        stop = time.monotonic() + 0.05
        while True:
            d = p.recv(max(0.0, stop - time.monotonic()))
            if d is None:
                break
            if d[:4] != b"\xff\xff\xff\xff":
                word = struct.unpack_from("<I", d)[0]
                if word & 0x80000000 and held is None:
                    held = word & 0x7FFFFFFF
                if word & 0x80000000 and struct.unpack_from("<H", d, 8)[0] < 1300:
                    # The gamestate's last fragment: a second of keepalives more.
                    hard_stop = min(hard_stop, time.monotonic() + 1.0)
                ack = held if held is not None else max(ack, word)
                quiet_until = max(quiet_until, time.monotonic() + 3)
    return 0


if __name__ == "__main__":
    sys.exit(main())
