#!/usr/bin/env python3
"""Summarise a ragdoll dump of tools/re/cod4_ragdoll_gdb.py (the format is
crates/cod4/tests/fixtures/README.md's): when each body went to sleep, how far
the root moved, the rest pose's extent. With a second dump, where the two part.

  python3 -I tools/re/cod4_ragdoll_summary.py DUMP [OTHER_DUMP]
"""
import math
import struct
import sys


def f32(h):
    return struct.unpack("<f", struct.pack("<I", int(h, 16)))[0]


def parse(path):
    """-> (start {bone: rec}, [(step header words, slot state, {bone: rec})])
    where rec = (flags, idle, pos, q, lvel, avel)."""
    start, steps = {}, []
    for line in open(path):
        w = line.split()
        if not w or w[0].startswith("#"):
            continue
        if w[0] in ("start", "body"):
            parts = line.split("|")
            head = parts[0].split()
            rec = (int(head[3], 16), f32(head[4])) + tuple(tuple(f32(x) for x in p.split()) for p in parts[1:])
            (start if w[0] == "start" else steps[-1][2])[int(head[2])] = rec
        elif w[0] == "step":
            steps.append([w, None, {}])
        elif w[0] == "slot":
            steps[-1][1] = int(w[3])
    return start, steps


def dist(a, b):
    return math.dist(a, b)


def summary(path):
    start, steps = parse(path)
    msec = {int(s[0][3]) for s in steps}
    print("%s: %d bodies, %d steps of %s ms" % (path, len(start), len(steps), sorted(msec)))
    asleep = {}
    for i, (_, _, bodies) in enumerate(steps, 1):
        for b, rec in bodies.items():
            if rec[0] & 4 and b not in asleep:
                asleep[b] = i
    rest = next((i for i, s in enumerate(steps, 1) if s[1] == 5), None)
    print("  asleep at step (body: step): %s" % ", ".join("%d: %d" % kv for kv in sorted(asleep.items())))
    print("  slot idle (state 5) at step %s, %s ms after the start" % (rest, rest and (rest - 1) * min(msec)))
    last = next(s[2] for s in reversed(steps) if s[2])
    r0, r1 = start[0][2], last[0][2]
    print("  root: start (%.2f %.2f %.2f) rest (%.2f %.2f %.2f), moved %.2f (z %+.2f)" % (r0 + r1 + (dist(r0, r1), r1[2] - r0[2])))
    ps = [rec[2] for rec in last.values()]
    lo = [min(p[k] for p in ps) for k in range(3)]
    hi = [max(p[k] for p in ps) for k in range(3)]
    print("  rest pose: bodies' centres span x %.2f y %.2f z %.2f (z %.2f..%.2f)" % (hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2], lo[2], hi[2]))
    first = steps[0][2]
    print("  first step: fastest body %.1f u/s, %.2f rad/s" % (
        max(math.hypot(*r[4]) for r in first.values()), max(math.hypot(*r[5]) for r in first.values())))
    return start, steps


def compare(a, b):
    (sa, ta), (sb, tb) = a, b
    d0 = max(dist(sa[k][2], sb[k][2]) for k in sa if k in sb)
    print("start poses differ by at most %.6g" % d0)
    for i, (x, y) in enumerate(zip(ta, tb), 1):
        d = max((dist(x[2][k][2], y[2][k][2]) for k in x[2] if k in y[2]), default=0.0)
        if d > 0:
            print("first difference at step %d: %.6g units" % (i, d))
            break
    else:
        print("identical over %d steps" % min(len(ta), len(tb)))
    lx, ly = ta[-1][2] or ta[-2][2], tb[-1][2] or tb[-2][2]
    print("rest poses differ by at most %.4g units" % max(dist(lx[k][2], ly[k][2]) for k in lx if k in ly))


if __name__ == "__main__":
    runs = [summary(p) for p in sys.argv[1:3]]
    if len(runs) == 2:
        compare(*runs)
