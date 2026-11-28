#!/usr/bin/env python3
"""Runs cod4_lnxded 1.7's position test of a capsule in a triangle (inlined
in the AABB position walk 0x811546a, from 0x81158fd per triangle;
cod4-collision.md 3.1) in tools/re/x87emu.py on generated inputs and writes
one line per case: `pos v0 v1 v2 start radius sphere_z -> startsolid`,
every float as its f32 bits in hex. x87emu runs at 53-bit precision where
the server's x87 runs at 64, so a case decided within a rounding of the
radius could read otherwise on the server; the generated ones are not.
Cases are kept for each new path through the compares, then random ones.
No triangle has two equal corners: there the server's kept `v2 - v0`
leaves a normal the client's stored one zeroes, and the two part
(cod4-collision.md 3.1).

    python3 -I tools/re/cod4_postest_server_vectors.py <server>/cod4_lnxded \
        > crates/cod4/tests/fixtures/clip/postest-1.7-server.txt
"""
import os
import random
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cod4_postest_vectors  # noqa: E402
from cod4_postest_vectors import bits, capsule  # noqa: E402
from x87emu import RET_SENTINEL, Emu, f32  # noqa: E402

VERTS = 0x889DE9C  # the clip map's vertex array (0x81156c3, 0x8115959)
LOOP = 0x81158FD  # the per-triangle test's head in 0x811546a's leaf loop
FRAME = 0x2F000000
KEEP_PATHS, EXTRA, TRIES = 160, 40, 8000


def triangle():
    """The client's generator's triangles, but none with two corners equal."""
    while True:
        v = cod4_postest_vectors.triangle()
        if v[0] != v[1] and v[1] != v[2] and v[0] != v[2]:
            return v


def run_pos(e):
    """One triangle through the leaf loop: the walk's frame holds the trace
    work (ebp-0x114), the trace (-0x118) and the triangles left (-0x110),
    edi the indices; the loop returns once the count runs out."""
    v = triangle()
    start, radius = capsule()
    sz = 0.0 if random.random() < 0.5 else f32(random.uniform(0.5, 20))
    tw = e.alloc(0x100)
    for i, c in enumerate(start):
        e.wf32(tw + 4 * i, c)
    e.wf32(tw + 0x88, radius)
    e.wf32(tw + 0x8C, sz)
    tr = e.alloc(0x40)
    e.wu(VERTS, 4, e.put_f32s([c for p in v for c in p], "v"))
    ip = e.alloc(8)
    for i in range(3):
        e.wu(ip + 2 * i, 2, i)
    e.wu(FRAME - 0x114, 4, tw)
    e.wu(FRAME - 0x118, 4, tr)
    e.wu(FRAME - 0x110, 4, 1)
    e.wu(FRAME + 4, 4, RET_SENTINEL)
    e.r.update(edi=ip, ebp=FRAME, esp=FRAME - 0x138)
    pc = LOOP
    while pc != RET_SENTINEL:
        e.steps += 1
        if e.steps > 200000:
            raise RuntimeError("runaway at %#x" % pc)
        mn, ops, nxt = e.insn(pc)
        e.pc = pc
        pc = e.step(pc, mn, ops, nxt)
    return v + [start, [radius, sz]], ["%d" % e.ru(tr + 0x29, 1)]


def main():
    e = Emu(sys.argv[1], sym=True)
    random.seed(17)
    print("# cod4_lnxded 1.7's triangle position test run by tools/re/x87emu.py on")
    print("# generated inputs (tools/re/cod4_postest_server_vectors.py); a line is")
    print("# the inputs and the startsolid byte, floats as f32 bits")
    seen, extra = set(), 0
    for _ in range(TRIES):
        e.reset()
        e.shadow.clear()
        e.labels.clear()
        ins, outs = run_pos(e)
        key = tuple(e.rels)
        if key in seen or len(seen) >= KEEP_PATHS:
            if extra >= EXTRA:
                continue
            extra += 1
        seen.add(key)
        print("pos", " ".join(bits(x) for group in ins for x in group), "->", " ".join(outs))
    print("# pos: %d paths" % len(seen))


if __name__ == "__main__":
    main()
