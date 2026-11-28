#!/usr/bin/env python3
"""Runs five of the 1.7 client's trace kernels (iw3mp.exe, cod4-collision.md
3) in tools/re/x87emu.py on generated inputs and writes one line per case:
the kernel, its inputs and what it left, every float as its f32 bits in hex.
Cases are kept for each new path through the kernel's compares (address and
outcome), then random contacts (cases that wrote the trace) up to the count.

    python3 -I tools/re/cod4_kernel_vectors.py <install>/iw3mp.exe \
        > crates/cod4/tests/fixtures/clip/kernels-1.7-client.txt

A line is `<kernel> <inputs> -> <code> <fraction> <normal> <startsolid>
<walkable>`; the code is the edge kernel's return (0 struck, 1 none, 2 and 3
past an end), `-` for the others. Each kernel's inputs, floats then ints:

- `edge` 0x4eff70: sphere from along sweep radius fraction ; walkable
- `vert` 0x4f0370: sphere corner sweep sweep_len2 radius fraction ; walk walkable
- `point` 0x4efce0: v0 v1 v2 start sweep fraction ; walkable
- `border` 0x4f1ae0: border(7) start sweep sweep_len sweep_len2 radius
  sphere_z fraction ; walkable
- `bsph` 0x4f19f0: border(7) start sweep sweep_len2 radius dz fraction ;
  walkable

`fraction` and `walkable` are the trace's before the call; its normal and
`startsolid` start zero. A border is `dist_eq[3] z_base z_slope start
length` (0x1c bytes, read at +0..+0x18).
"""
import math
import os
import random
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from x87emu import Emu, Sym, f32  # noqa: E402

KEEP_PATHS, EXTRA, TRIES = 150, 40, 8000
CRT_SQRT = 0x68AEC0  # 0x401050's root: fsqrt at 53 bits (0x68aefc)


def bits(x):
    return "%08x" % struct.unpack("<I", struct.pack("<f", x))[0]


def sqrt_hook(e):
    x = e.st[0]
    assert float(x) >= 0, "root of %r at %#x" % (x, e.pc)
    v = math.sqrt(float(x))
    e.st[0] = Sym(v, "sqrt(%s)" % x.e) if isinstance(x, Sym) else v


HOOKS = {CRT_SQRT: sqrt_hook}


def coord(k):
    """Mostly floats, some whole numbers for the ties."""
    return float(random.randint(-k, k)) if random.random() < 0.2 else f32(random.uniform(-k, k))


def vec(k):
    return [coord(k) for _ in range(3)]


def add(a, b):
    return [f32(x + y) for x, y in zip(a, b)]


def sub(a, b):
    return [f32(x - y) for x, y in zip(a, b)]


def scale(a, s):
    return [f32(x * s) for x in a]


def unit(a):
    n = math.sqrt(sum(x * x for x in a)) or 1.0
    return [f32(x / n) for x in a]


def cross(a, b):
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]


def lens(sw):
    """|sweep|^2 and |sweep| as the trace set-up stores them."""
    l2 = f32((sw[0] * sw[0] + sw[1] * sw[1]) + sw[2] * sw[2])
    return f32(math.sqrt(l2)), l2


def radius():
    return float(random.randint(1, 16)) if random.random() < 0.3 else f32(random.uniform(0.5, 16))


def fraction():
    return 1.0 if random.random() < 0.6 else f32(random.uniform(0, 1))


def a_sweep():
    sw = vec(60)
    if random.random() < 0.15:  # axial
        sw = [0.0, 0.0, 0.0]
        sw[random.randrange(3)] = coord(60) or 7.0
    return sw if any(sw) else [5.0, -3.0, 2.0]


def toward(target, at_t, sw=None):
    """A sweep that reaches `target` near its fraction `at_t`: its start
    and the sweep."""
    sw = sw or a_sweep()
    return sub(target, scale(sw, at_t)), sw


def at_t():
    return random.choice([random.uniform(-0.4, 0.05), random.uniform(0, 1), random.uniform(0.9, 1.4)])


def trace_work(e, start, sw, slen, slen2, r, sz):
    tw = e.alloc(0x100)
    for i in range(3):
        e.wf32(tw + 4 * i, start[i])
        e.wf32(tw + 0x24 + 4 * i, sw[i])
    e.wf32(tw + 0x30, slen)
    e.wf32(tw + 0x34, slen2)
    e.wf32(tw + 0x88, r)
    e.wf32(tw + 0x8C, sz)
    return tw


def trace_t(e, frac, walk):
    tr = e.alloc(0x40)
    e.wf32(tr, frac)
    e.wu(tr + 0x2A, 1, walk)
    e.before = left(e, tr)[1:]
    return tr


def left(e, tr, code="-"):
    return [code] + [bits(e.rf32(tr + 4 * i)) for i in range(4)] + [
        "%d" % e.ru(tr + 0x29, 1), "%d" % e.ru(tr + 0x2A, 1)]


def near_line(a, b, reach):
    """A point by the segment a..b, now and then past an end, within about
    two reaches of its line."""
    u = random.choice([random.uniform(-0.3, 1.3), random.uniform(-0.05, 0.05), random.uniform(0.95, 1.05)])
    on = [a[i] + u * (b[i] - a[i]) for i in range(3)]
    perp = unit(cross(sub(b, a), vec(10)))
    d = random.choice([random.uniform(0, reach), random.uniform(reach * 0.9, reach * 1.1), random.uniform(0, 2 * reach)])
    return [f32(on[i] + d * perp[i]) for i in range(3)]


def run_edge(e):
    r = radius()
    frm = vec(40)
    to = add(frm, vec(40)) if random.random() < 0.9 else add(frm, [0.0, 0.0, coord(40) or 9.0])
    along = sub(frm, to)
    sphere, sw = toward(near_line(frm, to, r + 0.125), at_t())
    frac, walk = fraction(), random.randint(0, 1)
    tw = trace_work(e, [0.0] * 3, sw, 0.0, 0.0, r, 0.0)
    tr = trace_t(e, frac, walk)
    ps, pf, pa = e.put_f32s(sphere, "sph"), e.put_f32s(frm, "from"), e.put_f32s(along, "along")
    code = e.call(0x4EFF70, args=[tw, pa, tr], regs={"ecx": ps, "eax": pf}, hooks=HOOKS)
    return [sphere, frm, along, sw, [r, frac]], [walk], left(e, tr, "%d" % code)


def run_vert(e):
    r = radius()
    corner = vec(40)
    reach = r + 0.125
    d = random.choice([random.uniform(0, reach), random.uniform(reach * 0.9, reach * 1.1), random.uniform(0, 2 * reach)])
    target = add(corner, scale(unit(vec(10)), d))
    sphere, sw = toward(target, at_t())
    _, slen2 = lens(sw)
    frac, walk, arg = fraction(), random.randint(0, 1), random.randint(0, 1)
    tw = trace_work(e, [0.0] * 3, sw, 0.0, slen2, r, 0.0)
    tr = trace_t(e, frac, walk)
    ps, pc = e.put_f32s(sphere, "sph"), e.put_f32s(corner, "v")
    e.call(0x4F0370, args=[arg], regs={"ecx": ps, "eax": pc, "edi": tw, "esi": tr}, hooks=HOOKS)
    return [sphere, corner, sw, [slen2, r, frac]], [arg, walk], left(e, tr)


VERTS = 0x140991C  # the clip map's vertex array (0x4efcee)


def run_point(e):
    v = [vec(30) for _ in range(3)]
    if random.random() < 0.15:  # a wall
        v[1][0], v[1][1] = v[0][0], v[0][1]
    elif random.random() < 0.15:  # flat
        v[1][2] = v[2][2] = v[0][2]
    a, b = random.uniform(-0.3, 1.1), random.uniform(-0.3, 1.1)
    if random.random() < 0.7 and a + b > 1:
        a, b = 1 - a, 1 - b
    target = [f32(v[0][i] + a * (v[1][i] - v[0][i]) + b * (v[2][i] - v[0][i])) for i in range(3)]
    start, sw = toward(target, at_t())
    frac, walk = fraction(), random.randint(0, 1)
    e.wu(VERTS, 4, e.put_f32s([c for p in v for c in p], "v"))
    ip = e.alloc(8)
    for i in range(3):
        e.wu(ip + 2 * i, 2, i)
    tw = trace_work(e, start, sw, 0.0, 0.0, 0.0, 0.0)
    tr = trace_t(e, frac, walk)
    e.call(0x4EFCE0, args=[tr], regs={"eax": ip, "edx": tw}, hooks=HOOKS)
    return v + [start, sw, [frac]], [walk], left(e, tr)


def border():
    """A wall line a x + b y = c with its start, length and top."""
    ang = random.uniform(0, 2 * math.pi)
    if random.random() < 0.15:
        ang = random.choice([0, math.pi / 2, math.pi, 3 * math.pi / 2])
    nx, ny = f32(math.cos(ang)), f32(math.sin(ang))
    dist = coord(40)
    start = coord(40)
    length = f32(random.uniform(1, 60)) if random.random() < 0.85 else float(random.randint(1, 60))
    zb = coord(30)
    slope = 0.0 if random.random() < 0.3 else f32(random.uniform(-0.8, 0.8))
    return [nx, ny, dist, zb, slope, start, length]


def put_border(e, b):
    return e.put_f32s(b, "b")


def by_border(b, r, sz):
    """A capsule centre near the wall, its top or an end."""
    nx, ny, dist, zb, slope, start, length = b
    al = random.choice([random.uniform(start - 25, start + length + 25), start + random.uniform(-2, 2),
                        start + length + random.uniform(-2, 2)])
    d = random.choice([random.uniform(-2, r + 1), random.uniform(0, 3 * r)])
    x = dist * nx + ny * al + nx * d
    y = dist * ny - nx * al + ny * d
    top = zb + slope * min(max(al - start, 0), length)
    z = top + random.choice([random.uniform(-(sz + r + 4), sz + r + 4), random.uniform(sz - 1, sz + r + 1),
                             -random.uniform(sz - 1, sz + r + 1)])
    return [f32(x), f32(y), f32(z)]


def run_border(e):
    b = border()
    r, sz = radius(), (0.0 if random.random() < 0.05 else f32(random.uniform(0.5, 30)))
    sw = a_sweep()
    out = sw[0] * b[0] + sw[1] * b[1]
    if out > 0 and random.random() < 0.8:  # mostly at the wall
        sw = [f32(sw[0] - 2 * out * b[0]), f32(sw[1] - 2 * out * b[1]), sw[2]]
    start, sw = toward(by_border(b, r, sz), at_t(), sw)
    slen, slen2 = lens(sw)
    frac, walk = fraction(), random.randint(0, 1)
    tw = trace_work(e, start, sw, slen, slen2, r, sz)
    tr = trace_t(e, frac, walk)
    e.call(0x4F1AE0, args=[tr], regs={"edi": tw, "esi": put_border(e, b)}, hooks=HOOKS)
    return [b, start, sw, [slen, slen2, r, sz, frac]], [walk], left(e, tr)


def run_bsph(e):
    b = border()
    r, sz = radius(), f32(random.uniform(0.5, 30))
    dz = sz if random.random() < 0.5 else -sz
    nx, ny, dist, zb, slope, s0, length = b
    ends = [[dist * nx + ny * s0, dist * ny - nx * s0, zb],
            [dist * nx + ny * (s0 + length), dist * ny - nx * (s0 + length), zb + slope * length]]
    sphere, sw = toward(near_line(ends[0], ends[1], r + 0.125), at_t())
    start = [sphere[0], sphere[1], f32(sphere[2] - dz)]
    _, slen2 = lens(sw)
    frac, walk = fraction(), random.randint(0, 1)
    tw = trace_work(e, start, sw, 0.0, slen2, r, sz)
    tr = trace_t(e, frac, walk)
    e.call(0x4F19F0, args=[tw, dz], regs={"eax": put_border(e, b), "ecx": tr}, hooks=HOOKS)
    return [b, start, sw, [slen2, r, dz, frac]], [walk], left(e, tr)


KERNELS = (("edge", run_edge), ("vert", run_vert), ("point", run_point), ("border", run_border),
           ("bsph", run_bsph))


def main():
    e = Emu(sys.argv[1], sym=True)
    random.seed(23)
    print("# iw3mp.exe 1.7's edge, corner, point, border and border sphere kernels")
    print("# run by tools/re/x87emu.py on generated inputs")
    print("# (tools/re/cod4_kernel_vectors.py); a line is the kernel, its inputs")
    print("# and the trace it left, floats as f32 bits")
    for name, run in KERNELS:
        seen, extra = set(), 0
        for _ in range(TRIES):
            e.reset()
            e.shadow.clear()
            e.labels.clear()
            floats, ints, outs = run(e)
            key = tuple(e.rels)
            if any(rel == "?" for _, rel in key):
                continue
            if key in seen or len(seen) >= KEEP_PATHS:
                # the extras are contacts: cases that wrote the trace
                if extra >= EXTRA or outs[1:] == e.before:
                    continue
                extra += 1
            seen.add(key)
            flat = [bits(x) for group in floats for x in group] + ["%d" % i for i in ints]
            print(name, " ".join(flat), "->", " ".join(outs))
        print("# %s: %d paths" % (name, len(seen)))


if __name__ == "__main__":
    main()
