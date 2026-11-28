#!/usr/bin/env python3
"""Runs the 1.7 client's position test kernels (iw3mp.exe, cod4-collision.md
3.1) in tools/re/x87emu.py on generated inputs and writes one line per case:
the kernel, its inputs and its answer, every float as its f32 bits in hex.
Cases are kept for each new path through the kernel's compares (address and
outcome), then random ones up to the count.

    python3 -I tools/re/cod4_postest_vectors.py <install>/iw3mp.exe \
        > crates/cod4/tests/fixtures/clip/postest-1.7-client.txt

Kernels: `pt` 0x4f0f90 (v0 v1 v2 p -> squared distance), `ss` 0x4f10e0
(b0 d0 b1 d1 -> squared distance), `cap` 0x4f1400 (v0 v1 v2 top bottom r2
-> 0/1), `pos` 0x4f1930 (v0 v1 v2 start radius sphere_z -> 0/1).
"""
import os
import random
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from x87emu import Emu, f32  # noqa: E402

VERTS = 0x140991C  # the clip map's vertex array (0x4f0f96, 0x4f1406)
KEEP_PATHS, EXTRA, TRIES = 120, 40, 6000


def bits(x):
    return "%08x" % struct.unpack("<I", struct.pack("<f", x))[0]


def coord(k):
    """Mostly floats, some whole numbers for the ties."""
    return float(random.randint(-k, k)) if random.random() < 0.3 else f32(random.uniform(-k, k))


def vec(k):
    return [coord(k) for _ in range(3)]


def triangle():
    v = [vec(10) for _ in range(3)]
    roll = random.random()
    if roll < 0.15:  # a wall: one edge straight up
        v[1][0], v[1][1] = v[0][0], v[0][1]
    elif roll < 0.25:  # flat
        v[1][2] = v[2][2] = v[0][2]
    elif roll < 0.28:  # two corners on one
        v[2] = list(v[1])
    return v


def put_tri(e, v):
    e.wu(VERTS, 4, e.put_f32s([c for p in v for c in p], "v"))
    ip = e.alloc(8)
    for i in range(3):
        e.wu(ip + 2 * i, 2, i)
    return ip


def run_pt(e):
    v, p = triangle(), vec(14)
    ip = put_tri(e, v)
    e.call(0x4F0F90, args=[e.put_f32s(p, "p"), 0, 0], regs={"ecx": ip})
    return v + [p], [bits(f32(float(e.st[0])))]


def run_ss(e):
    b0, b1, d1 = vec(8), vec(8), vec(8)
    d0 = [0.0, 0.0, -f32(random.uniform(0.25, 40))]
    roll = random.random()
    if roll < 0.2:
        d1[0] = d1[1] = 0.0
    elif roll < 0.25:
        d1 = [0.0, 0.0, 0.0]
    regs = {"eax": e.put_f32s(d1, "d1"), "esi": e.put_f32s(b0, "b0"), "edx": e.put_f32s(b1, "b1")}
    e.call(0x4F10E0, args=[e.put_f32s(d0, "d0")], regs=regs)
    return [b0, d0, b1, d1], [bits(f32(float(e.st[0])))]


def capsule():
    start = vec(14)
    radius = f32(random.uniform(0.5, 8)) if random.random() < 0.8 else float(random.randint(1, 8))
    return start, radius


def run_cap(e):
    v = triangle()
    start, radius = capsule()
    sz = f32(random.uniform(0.5, 20))
    top = [start[0], start[1], f32(start[2] + sz)]
    bottom = [start[0], start[1], f32(start[2] - sz)]
    r2 = f32(radius * radius)
    ip = put_tri(e, v)
    out = e.call(0x4F1400, args=[e.put_f32s(top, "top"), e.put_f32s(bottom, "bot"), r2], regs={"ecx": ip})
    return v + [top, bottom, [r2]], ["%d" % (out & 0xFF)]


def run_pos(e):
    v = triangle()
    start, radius = capsule()
    sz = 0.0 if random.random() < 0.5 else f32(random.uniform(0.5, 20))
    ip = put_tri(e, v)
    tw = e.alloc(0x100)
    for i, c in enumerate(start):
        e.wf32(tw + 4 * i, c)
    e.wf32(tw + 0x88, radius)
    e.wf32(tw + 0x8C, sz)
    tr = e.alloc(0x40)
    e.call(0x4F1930, regs={"ecx": ip, "edx": tw, "esi": tr})
    return v + [start, [radius, sz]], ["%d" % e.ru(tr + 0x29, 1)]


def main():
    e = Emu(sys.argv[1], sym=True)
    random.seed(17)
    print("# iw3mp.exe 1.7's position test kernels run by tools/re/x87emu.py on")
    print("# generated inputs (tools/re/cod4_postest_vectors.py); a line is the")
    print("# kernel, its inputs and its answer, floats as f32 bits")
    for name, run in (("pt", run_pt), ("ss", run_ss), ("cap", run_cap), ("pos", run_pos)):
        seen, extra = set(), 0
        for _ in range(TRIES):
            e.reset()
            e.shadow.clear()
            e.labels.clear()
            ins, outs = run(e)
            key = tuple(e.rels)
            if key in seen or len(seen) >= KEEP_PATHS:
                if extra >= EXTRA:
                    continue
                extra += 1
            seen.add(key)
            flat = [bits(x) for group in ins for x in group]
            print(name, " ".join(flat), "->", " ".join(outs))
        print("# %s: %d paths" % (name, len(seen)))


if __name__ == "__main__":
    main()
