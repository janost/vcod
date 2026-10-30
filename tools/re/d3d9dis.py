#!/usr/bin/env python3
"""A small D3D9 shader bytecode disassembler (vs/ps 2.x and 3.0), enough to
read a CoD4 technique's pixel shader: instructions, `def` constants and the
constant table's names. The bytecode comes from a zone through
`cargo run -p vcod-cod4 --example techset_shaders <map> <techset> <dir>`.

    python3 tools/re/d3d9dis.py <file.ps|file.vs>
    python3 tools/re/d3d9dis.py --count <file>...

`--count` prints, per file, the instructions (no `dcl`, `def` or
comments), the texture fetches among them, the flow control ones and the
instruction slots, with each macro at the slots ps/vs 3.0 expand it to
(`nrm` 3, `pow` 3, `lrp` 2, `m4x4` 4, `sincos` 8; SLOTS), for
cod4-client-perf.md 26's table.
"""
import struct
import sys

OPS = {
    0: "nop", 1: "mov", 2: "add", 3: "sub", 4: "mad", 5: "mul", 6: "rcp",
    7: "rsq", 8: "dp3", 9: "dp4", 10: "min", 11: "max", 12: "slt", 13: "sge",
    14: "exp", 15: "log", 16: "lit", 17: "dst", 18: "lrp", 19: "frc",
    20: "m4x4", 21: "m4x3", 22: "m3x4", 23: "m3x3", 24: "m3x2", 25: "call",
    26: "callnz", 27: "loop", 28: "ret", 29: "endloop", 30: "label", 31: "dcl",
    32: "pow", 33: "crs", 34: "sgn", 35: "abs", 36: "nrm", 37: "sincos",
    38: "rep", 39: "endrep", 40: "if", 41: "ifc", 42: "else", 43: "endif",
    44: "break", 45: "breakc", 46: "mova", 47: "defb", 48: "defi", 66: "texld",
    81: "def", 88: "cmp", 89: "bem", 90: "dp2add", 91: "dsx", 92: "dsy",
    93: "texldd", 94: "setp", 95: "texldl", 96: "breakp",
}
REG = {
    0: "r", 1: "v", 2: "c", 3: "t", 4: "rasout", 5: "attrout", 6: "o",
    7: "input", 8: "oC", 9: "oDepth", 10: "s", 11: "c", 12: "c", 13: "c",
    14: "c", 15: "b", 16: "aL", 17: "vFace", 18: "misc", 19: "label", 20: "p",
}
# source-only instructions, and ifc / breakc's comparison (control bits 16..18)
NO_DEST = {25, 26, 27, 38, 40, 41, 45, 96}
CMP = {1: "_gt", 2: "_eq", 3: "_ge", 4: "_lt", 5: "_ne", 6: "_le"}
SRC_MOD = {1: "-{}", 2: "{}_bias", 4: "{}_bx2", 6: "1-{}", 11: "abs({})", 12: "-abs({})", 13: "!{}"}


TEX = {66, 93, 95}
FLOW = {25, 26, 27, 28, 29, 30, 38, 39, 40, 41, 42, 43, 44, 45, 96}
SLOTS = {20: 4, 21: 3, 22: 4, 23: 3, 24: 2, 18: 2, 32: 3, 33: 2, 36: 3, 37: 8, 17: 2, 16: 3}


def count(path):
    d = open(path, "rb").read()
    w = struct.unpack("<%dI" % (len(d) // 4), d)
    n = tex = flow = slots = 0
    i = 1
    while i < len(w):
        t = w[i]
        if t == 0xFFFF:
            break
        if t & 0xFFFF == 0xFFFE:
            i += 1 + ((t >> 16) & 0x7FFF)
            continue
        op, ln = t & 0xFFFF, (t >> 24) & 0xF
        if op not in (31, 81, 47, 48, 0):
            n += 1
            tex += op in TEX
            flow += op in FLOW
            slots += SLOTS.get(op, 1)
        i += 1 + ln
    return n, tex, flow, slots


def reg(t, dst):
    kind = ((t >> 28) & 7) | ((t >> 8) & 0x18)
    s = REG.get(kind, f"?{kind}") + str(t & 0x7FF)
    if dst:
        mask = (t >> 16) & 0xF
        if mask != 0xF:
            s += "." + "".join("xyzw"[i] for i in range(4) if mask >> i & 1)
        mod = (t >> 20) & 0xF
        if mod & 1:
            s += "_sat"
        if mod & 2:
            s += "_pp"
        return s
    sw = (t >> 16) & 0xFF
    c = "".join("xyzw"[(sw >> (2 * i)) & 3] for i in range(4))
    if c != "xyzw":
        s += "." + (c[0] if len(set(c)) == 1 else c)
    return SRC_MOD.get((t >> 24) & 0xF, "{}").format(s)


def main(path):
    d = open(path, "rb").read()
    w = struct.unpack("<%dI" % (len(d) // 4), d)
    print("version %08x" % w[0])
    i = 1
    while i < len(w):
        t = w[i]
        if t == 0xFFFF:
            break
        if t & 0xFFFF == 0xFFFE:
            n = (t >> 16) & 0x7FFF
            blob = d[4 * (i + 1):4 * (i + 1 + n)]
            if blob[:4] == b"CTAB":
                c = blob[4:]
                hdr = struct.unpack_from("<7I", c, 0)
                for k in range(hdr[3]):
                    name, rs, ri, rc, _, _, _ = struct.unpack_from("<IHHHHII", c, hdr[4] + 20 * k)
                    print("  ; %s %s%d (%d)" % (c[name:c.index(b"\0", name)].decode(), "bics"[rs], ri, rc))
            i += 1 + n
            continue
        op, n = t & 0xFFFF, (t >> 24) & 0xF
        args = w[i + 1:i + 1 + n]
        if op == 81:
            print("def c%d" % (args[0] & 0x7FF), struct.unpack("<4f", struct.pack("<4I", *args[1:5])))
        elif op == 31:
            print("dcl_%x %s" % (args[0], reg(args[1], True)))
        elif op in NO_DEST:
            cmp = CMP.get((t >> 16) & 7, "") if op in (41, 45) else ""
            print(OPS.get(op, f"op{op}") + cmp, ", ".join(reg(a, False) for a in args))
        elif n:
            print(OPS.get(op, f"op{op}"), ", ".join([reg(args[0], True)] + [reg(a, False) for a in args[1:]]))
        else:
            print(OPS.get(op, f"op{op}"))
        i += 1 + n


if __name__ == "__main__":
    if sys.argv[1] == "--count":
        print("file instructions tex flow slots")
        for p in sys.argv[2:]:
            print(p, *count(p))
    else:
        main(sys.argv[1])
