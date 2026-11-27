#!/usr/bin/env python3
"""Summarise a world 0 or 1 dump of tools/re/cod4_phys_gdb.py (the format is
crates/cod4/tests/fixtures/phys/README.md's): the seed at the first body,
each body's creator, mass and geoms, the step count and msecStep sequence,
when each body slept or went, and the contact, push and sound counts. With
--trim, also writes the dump cut one step past the last step with a body
awake, or after step N with --steps N; --every K keeps a body's line only
on every Kth step, its first step and the step it first reads asleep.

  python3 -I tools/re/cod4_phys_summary.py DUMP [--trim OUT [--steps N] [--every K]]
"""
import math
import struct
import sys


def f32(h):
    return struct.unpack("<f", struct.pack("<I", int(h, 16)))[0]


def fs(part):
    return [f32(x) for x in part.split()]


def parse(path):
    d = {"world": None, "bodies": {}, "steps": [], "collides": [], "cuts": [], "pushes": [], "sounds": []}
    cur = None
    for line in open(path):
        w = line.split()
        if not w or w[0].startswith("#"):
            continue
        k = w[0]
        parts = line.split("|")
        if k == "world":
            d["world"] = {"world": int(w[1]), "seed": w[4], "msec_step": int(w[6])}
        elif k == "new":
            # new ID ADDR WHO world_ok X flags F | mass .. | inv .. | force ..
            head = parts[0].split()
            f = {p.split()[0]: fs(" ".join(p.split()[1:])) for p in parts[1:]}
            d["bodies"][int(w[1])] = {"who": head[3], "world_ok": int(head[5]), "flags": int(head[7], 16),
                                      "mass": f["mass"], "pos": f["pos"], "lvel": f["lvel"], "avel": f["avel"],
                                      "geoms": [], "user": None, "sleep": None, "gone": None}
        elif k == "user":
            d["bodies"][int(w[1])]["user"] = w[2:]
        elif k == "geom":
            d["bodies"][int(w[1])]["geoms"].append([int(x, 16) for x in parts[1].split()])
        elif k == "step":
            # step N msec M sim S to E seed X next M2 awake A of B
            d["steps"].append({"n": int(w[1]), "msec": int(w[3]), "seed": w[9], "next": int(w[11]),
                               "awake": int(w[13]), "count": int(w[15]), "bodies": {}})
        elif k == "body":
            head = parts[0].split()
            d["steps"][-1]["bodies"][int(head[1])] = (int(head[2], 16), fs(parts[1]), fs(parts[3]), fs(parts[4]))
        elif k == "collide":
            cur = {"step": int(w[1]), "ids": (int(w[2]), int(w[3])), "classes": (int(w[4]), int(w[5])), "n": int(w[6]), "c": []}
            d["collides"].append(cur)
        elif k == "cut":
            cur = {"step": int(w[1]), "ids": (int(w[2]), int(w[3])), "n": int(w[4]), "c": []}
            d["cuts"].append(cur)
        elif k == "c":
            cur["c"].append(w[1:])
        elif k == "push":
            d["pushes"].append({"step": int(w[1]), "id": int(w[2]), "caller": w[3]})
        elif k == "sound":
            d["sounds"].append({"step": int(w[1]), "handle": w[2]})
    for s in d["steps"]:
        for i, b in d["bodies"].items():
            rec = s["bodies"].get(i)
            if rec is None and b["gone"] is None and any(i in t["bodies"] for t in d["steps"][: s["n"] - 1]):
                b["gone"] = s["n"]
            if rec and rec[0] & 4 and b["sleep"] is None:
                b["sleep"] = s["n"]
    return d


def runs(xs):
    out = []
    for x in xs:
        if out and out[-1][0] == x:
            out[-1][1] += 1
        else:
            out.append([x, 1])
    return " ".join("%dx%d" % (x, n) if n > 1 else str(x) for x, n in out)


def last_awake(d):
    return max((s["n"] for s in d["steps"] if s["awake"]), default=0)


def summary(path):
    d = parse(path)
    w = d["world"]
    print("%s: world %d, seed %s at the first body (%s), msecStep %d" % (
        path, w["world"], w["seed"], "the map load's" if w["seed"] == "89abcdef" else "NOT the map load's", w["msec_step"]))
    print("  %d bodies:" % len(d["bodies"]))
    for i, b in sorted(d["bodies"].items()):
        classes = ",".join(str(g[1]) for g in b["geoms"])
        print("    %d %s mass %.4g geoms [%s] speed %.1f u/s %.2f rad/s, asleep at step %s, gone at %s" % (
            i, b["who"], b["mass"][0], classes, math.hypot(*b["lvel"]), math.hypot(*b["avel"]), b["sleep"], b["gone"]))
    steps = d["steps"]
    la = last_awake(d)
    print("  %d steps, the last with a body awake %d (%d ms simulated)" % (
        len(steps), la, sum(s["msec"] for s in steps[:la])))
    print("  msec: %s" % runs([s["msec"] for s in steps[:la + 1]]))
    held = [s["n"] for i, s in enumerate(steps) if i and s["seed"] == steps[i - 1]["seed"]]
    print("  seed held on %d steps%s" % (len(held), (", first %d" % held[0]) if held else ""))
    nraw = sum(c["n"] for c in d["collides"])
    span = sorted({c["step"] for c in d["collides"]})
    print("  dCollide: %d calls, %d contacts over steps %s..%s; most from one call %d" % (
        len(d["collides"]), nraw, span[0] if span else "-", span[-1] if span else "-",
        max((c["n"] for c in d["collides"]), default=0)))
    print("  contact pass: %d calls, at most %d contacts" % (len(d["cuts"]), max((c["n"] for c in d["cuts"]), default=0)))
    print("  pushes: %d (%s)" % (len(d["pushes"]), ", ".join(sorted({"%s step %d" % (p["caller"], p["step"]) for p in d["pushes"]})[:6])))
    print("  impact sounds: %d at steps %s" % (len(d["sounds"]), " ".join(str(s["step"]) for s in d["sounds"][:20])))
    return d


def trim(path, out, keep=None, every=1):
    d = parse(path)
    keep = keep or last_awake(d) + 1
    marks = {(b["sleep"], i) for i, b in d["bodies"].items()}
    first = {}
    with open(out, "w") as o:
        step = 0
        for line in open(path):
            w = line.split()
            if w and w[0] == "step":
                step = int(w[1])
            if w and w[0] == "end":
                break
            if step > keep or (step == keep and w and w[0] in ("collide", "cut", "c", "sound", "push")):
                continue
            if w and w[0] == "body" and every > 1:
                i = int(w[1])
                if first.setdefault(i, step) != step and step % every and (step, i) not in marks:
                    continue
            o.write(line)
        o.write("end\n")
    print("trimmed to step %d: %s" % (keep, out))


if __name__ == "__main__":
    a = sys.argv[1:]
    summary(a[0])
    if "--trim" in a:
        trim(a[0], a[a.index("--trim") + 1], int(a[a.index("--steps") + 1]) if "--steps" in a else None,
             int(a[a.index("--every") + 1]) if "--every" in a else 1)
