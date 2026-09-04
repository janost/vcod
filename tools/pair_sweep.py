#!/usr/bin/env python3
"""Pair a `--probe-sweep` run's taps with the hits the target saw and the
labels the server logged, and print the height each bullet crossed the
target's origin plane at.

    tools/pair_sweep.py <shooter.log> <target.log> <labels.txt>

`shooter.log` and `target.log` are the two probes' stdout; `labels.txt` is
the server's `D;`/`K;` lines in order (retail: `games_mp.log`; vcod: the
`script: D;` lines of its stdout with the prefix cut). A hit is a health
drop with `damageEvent` set, so suicides do not count. The pitch is the
tap's own offset on top of the eye-to-eye aim, both eyes 60 above their
origins, rather than the echoed `viewangles`, which vcod does not send and
retail echoes a snapshot late. What the runs measured is in
docs/research/cod11-combat.md, section 3.4.
"""
import math
import re
import sys


def kv(line):
    return dict(re.findall(r"(\w+)=(\S+)", line))


def main(shooter, target, labels_path):
    rows = [kv(l) for l in open(shooter) if "phase=sweep" in l]
    hits, prev = [], None
    for l in open(target):
        if "!trace" not in l:
            continue
        d = kv(l)
        h = int(d["health"])
        if prev is not None and 0 <= h < prev and int(d["damageEvent"]) > 0:
            hits.append((int(d["serverTime"]), prev, h))
        prev = h
    labels = [
        l.strip().split(";")[-1]
        for l in open(labels_path)
        if re.search(r"\b[DK];", l) and "SUICIDE" not in l
    ]
    for i, (st, before, after) in enumerate(hits):
        r = [x for x in rows if int(x["serverTime"]) <= st][-1]
        so = [float(v) for v in r["origin"].split(",")]
        to = [float(v) for v in r["target_origin"].split(",")]
        off = float(r["pitchOffset"])
        horiz = math.hypot(to[0] - so[0], to[1] - so[1])
        eye_s, eye_t = so[2] + 60, to[2] + 60
        base = math.degrees(math.atan2(eye_s - eye_t, horiz))
        z = eye_s - horiz * math.tan(math.radians(base + off)) - to[2]
        label = labels[i] if i < len(labels) else "?"
        print(
            f"st={st} offset={off:5.1f} range={horiz:4.0f} "
            f"z_above_origin={z:5.1f} dmg={before - after:3d} label={label}"
        )


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
