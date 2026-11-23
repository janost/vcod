#!/usr/bin/env python3
"""Cuts a committed net fixture out of a tools/lo_capture.py log.

    python3 -I tools/cod4x_fixture.py LO_TXT OUT --peer ADDR [--peer ADDR]...
        [--until MS] [--note TEXT]...

Keeps the lines of the named peers (the clients' addresses as the log
names them), up to capture time MS, with times rebased to the first line
kept; each --note becomes a `#` header line ahead of them. The layout is the
one vcod_cod4::net::capture::parse reads.
"""

import argparse
import sys


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("log")
    ap.add_argument("out")
    ap.add_argument("--peer", action="append", required=True)
    ap.add_argument("--until", type=int)
    ap.add_argument("--note", action="append", default=[])
    a = ap.parse_args()
    kept = []
    with open(a.log) as f:
        for line in f:
            parts = line.split()
            if len(parts) != 4 or parts[1] not in ("c2s", "s2c"):
                continue
            ms = int(parts[0])
            if parts[2] not in a.peer or (a.until is not None and ms > a.until):
                continue
            kept.append((ms, parts[1], parts[2], parts[3]))
    if not kept:
        print("no lines kept", file=sys.stderr)
        return 1
    t0 = kept[0][0]
    with open(a.out, "w") as f:
        for n in a.note:
            f.write(f"# {n}\n")
        for ms, d, peer, hexs in kept:
            f.write(f"{ms - t0} {d} {peer} {hexs}\n")
    print(f"{len(kept)} lines, {kept[-1][0] - t0} ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
