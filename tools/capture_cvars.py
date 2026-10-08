#!/usr/bin/env python3
"""Prints a running retail server's cvar registry as the Rust rows of
`crates/server/src/cvar_registry.rs`.

Runs `cvarlist` over rcon for the names and flag letters, then queries each
name for the default `Cvar_Command` prints. Start the server with
`tools/run_server.sh <map> +set rconPassword <pw>` and no client.

    tools/capture_cvars.py <port> <rconPassword> > rows.txt

Left out: the `scr_*` cvars and the seven other `set` lines of
`default_mp.cfg`, which `Cvars::exec_cfg` creates from the paks, and the
rows `cvars.rs` already seeds (`ENGINE_MIRRORED`, `ENGINE_DEFAULTS`). The
defaults retail takes from the host (`fs_basepath`, `fs_homepath`,
`username`) are written empty.
"""
import re
import socket
import sys

CFG_SETS = {"sensitivity", "cl_freelook", "ui_mousepitch", "m_pitch", "m_filter",
            "cl_mouseaccel", "ui_allow_sniperrifles"}
HOST = {"fs_basepath", "fs_homepath", "username"}
SEEDED = re.compile(r'^\s+\("([A-Za-z0-9_]+)",')


def rcon(port, pw, cmd):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(0.5)
    s.sendto(b"\xff\xff\xff\xffrcon " + pw.encode() + b" " + cmd.encode("latin1"),
             ("127.0.0.1", port))
    out = b""
    try:
        while True:
            d, _ = s.recvfrom(65536)
            out += d[len(b"\xff\xff\xff\xffprint\n"):]
    except socket.timeout:
        pass
    return out.decode("latin1")


def seeded():
    names = set()
    with open("crates/server/src/cvars.rs") as f:
        for line in f:
            m = SEEDED.match(line)
            if m:
                names.add(m.group(1).lower())
    return names


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    port, pw = int(sys.argv[1]), sys.argv[2]
    skip = seeded()
    for line in rcon(port, pw, "cvarlist").split("\n"):
        if len(line) < 9 or line[7] != " " or '"' not in line:
            continue
        flags, name = line[:7].replace(" ", ""), line[8:].split(" ", 1)[0]
        low = name.lower()
        if low.startswith("scr_") or low in CFG_SETS or low in skip:
            continue
        default = ""
        if low not in HOST:
            q = rcon(port, pw, name)
            m = re.match(r'"[^"]*" is:".*\^7" default:"(.*)\^7"', q)
            if not m:
                sys.exit(f"no default for {name}: {q!r}")
            default = m.group(1)
        print(f'    ("{name}", "{flags}", "{default}"),')


if __name__ == "__main__":
    main()
