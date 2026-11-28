#!/usr/bin/env python3
"""A small i386 + x87 interpreter over objdump's Intel listing of a PE or an
i386 ELF, for running a retail leaf function on chosen inputs
(cod4-collision.md 3).

The x87 runs at 53-bit precision, as iw3mp.exe's client does
(cod4-player-anims.md 11): every stack value is a Python float, an m32
store rounds to f32. Only the integer and x87 instructions the 1.7 clip
code uses are implemented; anything else raises.

    from x87emu import Emu
    e = Emu("<install>/iw3mp.exe")
    p = e.alloc(16); e.wf32(p, 1.0)
    e.call(0x4f0f90, args=[p], regs={"ecx": tri})
"""
import math
import re
import struct
import subprocess

REGS = ("eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi")
SUB = {
    "ax": ("eax", 0, 0xFFFF), "cx": ("ecx", 0, 0xFFFF), "dx": ("edx", 0, 0xFFFF),
    "bx": ("ebx", 0, 0xFFFF), "si": ("esi", 0, 0xFFFF), "di": ("edi", 0, 0xFFFF),
    "bp": ("ebp", 0, 0xFFFF),
    "al": ("eax", 0, 0xFF), "cl": ("ecx", 0, 0xFF), "dl": ("edx", 0, 0xFF), "bl": ("ebx", 0, 0xFF),
    "ah": ("eax", 8, 0xFF), "ch": ("ecx", 8, 0xFF), "dh": ("edx", 8, 0xFF), "bh": ("ebx", 8, 0xFF),
}
SIZES = {"BYTE": 1, "WORD": 2, "DWORD": 4, "QWORD": 8, "TBYTE": 10}
RET_SENTINEL = 0xDEAD0000


def f32(x):
    """`x` rounded to the nearest f32, as an m32 store does."""
    try:
        return struct.unpack("<f", struct.pack("<f", x))[0]
    except OverflowError:
        return float("inf") if x > 0 else float("-inf")


class Sym:
    """A stack value with the expression that made it (`Emu.sym`): kept
    arithmetic stays inline, each m32 store names a temporary."""

    __slots__ = ("v", "e")

    def __init__(self, v, e):
        self.v, self.e = float(v), e

    def __float__(self):
        return self.v

    def __repr__(self):
        return "%s=%r" % (self.e, self.v)

    def _bin(self, o, op, rev=False):
        ov, oe = (o.v, o.e) if isinstance(o, Sym) else (float(o), repr(float(o)))
        a, b = (ov, self.v) if rev else (self.v, ov)
        ae, be = (oe, self.e) if rev else (self.e, oe)
        return Sym(Emu.arith(op, a, b), "(%s %s %s)" % (ae, {"add": "+", "sub": "-", "mul": "*", "div": "/"}[op], be))

    def __add__(self, o): return self._bin(o, "add")
    def __radd__(self, o): return self._bin(o, "add", True)
    def __sub__(self, o): return self._bin(o, "sub")
    def __rsub__(self, o): return self._bin(o, "sub", True)
    def __mul__(self, o): return self._bin(o, "mul")
    def __rmul__(self, o): return self._bin(o, "mul", True)
    def __truediv__(self, o): return self._bin(o, "div")
    def __rtruediv__(self, o): return self._bin(o, "div", True)
    def __neg__(self): return Sym(-self.v, "-" + self.e)
    def __abs__(self): return Sym(abs(self.v), "|%s|" % self.e)
    def __lt__(self, o): return self.v < float(o)
    def __gt__(self, o): return self.v > float(o)
    def __le__(self, o): return self.v <= float(o)
    def __ge__(self, o): return self.v >= float(o)
    def __eq__(self, o): return self.v == float(o)
    def __ne__(self, o): return self.v != float(o)
    __hash__ = None


class Emu:
    def __init__(self, exe, sym=False):
        self.exe = exe
        d = open(exe, "rb").read()
        self.image = []
        if d[:4] == b"\x7fELF":
            # an i386 ELF (cod4_lnxded): its PT_LOAD segments
            phoff, = struct.unpack_from("<I", d, 0x1C)
            phentsize, phnum = struct.unpack_from("<HH", d, 0x2A)
            for i in range(phnum):
                kind, off, va, _, filesz = struct.unpack_from("<IIIII", d, phoff + i * phentsize)
                if kind == 1:
                    self.image.append((va, d[off:off + filesz]))
        else:
            pe = struct.unpack_from("<I", d, 0x3C)[0]
            nsec = struct.unpack_from("<H", d, pe + 6)[0]
            optsz = struct.unpack_from("<H", d, pe + 20)[0]
            base = struct.unpack_from("<I", d, pe + 24 + 28)[0]
            off = pe + 24 + optsz
            for _ in range(nsec):
                vs, va, rs, pr = struct.unpack_from("<IIII", d, off + 8)
                self.image.append((base + va, d[pr:pr + min(vs, rs)]))
                off += 40
        self.code = {}
        self.mem = {}
        self.sym = sym
        self.shadow = {}  # address -> (width, Sym) of the last x87 store there
        self.labels = {}  # address -> name of an input float
        self.heap = 0x20000000
        self.reset()

    def reset(self):
        self.r = {k: 0 for k in REGS}
        self.r["esp"] = 0x30000000
        self.st = []
        self.c = (0, 0, 0)  # C0, C2, C3
        self.flags = {"ZF": 0, "SF": 0, "CF": 0, "OF": 0, "PF": 0}
        self.steps = 0
        self.defs = []  # (temporary, expression) per m32 store, symbolic mode
        self.path = []  # each x87 compare and its outcome, symbolic mode
        self.rels = []  # (address, outcome) of each, symbolic mode

    # Memory.
    def rb(self, a):
        page = self.mem.get(a >> 12)
        if page is not None:
            return page[a & 0xFFF]
        for va, data in self.image:
            if va <= a < va + len(data):
                return data[a - va]
        return 0

    def read(self, a, n):
        return bytes(self.rb(a + i) for i in range(n))

    def write(self, a, b):
        for k in [k for k, (w, _) in self.shadow.items() if a - w < k < a + len(b)]:
            del self.shadow[k]
        for i, x in enumerate(b):
            page = self.mem.setdefault((a + i) >> 12, bytearray(4096))
            page[(a + i) & 0xFFF] = x

    def ru(self, a, n):
        return int.from_bytes(self.read(a, n), "little")

    def wu(self, a, n, v):
        self.write(a, (v & ((1 << (8 * n)) - 1)).to_bytes(n, "little"))

    def rf32(self, a):
        return struct.unpack("<f", self.read(a, 4))[0]

    def wf32(self, a, x):
        self.write(a, struct.pack("<f", x))

    def rf64(self, a):
        return struct.unpack("<d", self.read(a, 8))[0]

    def alloc(self, n, align=16):
        self.heap = (self.heap + align - 1) & ~(align - 1)
        a = self.heap
        self.heap += n
        return a

    def put_f32s(self, xs, name=None):
        """Floats at a new address, each labelled `name.x` (.y, .z, then [i])
        in symbolic mode."""
        a = self.alloc(4 * len(xs))
        for i, x in enumerate(xs):
            self.wf32(a + 4 * i, x)
            if name:
                self.labels[a + 4 * i] = name + (".%s" % "xyz"[i] if len(xs) <= 3 else "[%d]" % i)
        return a

    def mem_f(self, a, width):
        v = self.rf32(a) if width == 4 else self.rf64(a)
        if not self.sym:
            return v
        sh = self.shadow.get(a)
        if sh is not None and sh[0] == width:
            return sh[1]
        return Sym(v, self.labels.get(a, "m%x" % a))

    def store_f(self, a, x, width):
        if width == 4:
            self.wf32(a, f32(float(x)))
        else:
            self.write(a, struct.pack("<d", float(x)))
        if self.sym and isinstance(x, Sym):
            if width == 4 and (f32(x.v) != x.v or not re.fullmatch(r"[\w.\[\]]+", x.e)):
                name = "t%d" % len(self.defs)
                self.defs.append((name, "r(%s)" % x.e))
                x = Sym(f32(x.v), name)
            self.shadow[a] = (width, x)

    # Code.
    def insn(self, a):
        if a not in self.code or self.code[a][2] is None:
            self.disasm(a)
        return self.code[a]

    def disasm(self, a):
        out = subprocess.run(
            ["objdump", "-d", "--no-show-raw-insn", "-M", "intel",
             "--start-address=%#x" % a, "--stop-address=%#x" % (a + 0x1000), self.exe],
            check=True, capture_output=True, text=True).stdout
        prev = None
        for line in out.splitlines():
            m = re.match(r"^\s*([0-9a-f]+):\t(\S+)\s*(.*)$", line)
            if not m:
                continue
            addr = int(m.group(1), 16)
            if addr >= a + 0x1000 - 16:
                break
            ops = re.sub(r"\s*<[^>]*>", "", m.group(3).split("#")[0]).strip()
            ops = [o.strip() for o in re.split(r",(?![^\[]*\])", ops)] if ops else []
            if prev is not None:
                self.code[prev] = self.code[prev][:2] + (addr,)
            self.code[addr] = (m.group(2), ops, None)
            prev = addr

    # Operands.
    def ea(self, op):
        m = re.search(r"\[(.*)\]", op)
        if m:
            total = 0
            for term in re.findall(r"[+-]?[^+-]+", m.group(1)):
                sign = -1 if term.startswith("-") else 1
                term = term.lstrip("+-")
                if "*" in term:
                    reg, scale = term.split("*")
                    total += sign * self.r[reg] * int(scale, 0)
                elif term in self.r:
                    total += sign * self.r[term]
                else:
                    total += sign * int(term, 0)
            return total & 0xFFFFFFFF
        m = re.search(r"ds:(0x[0-9a-f]+)", op)
        return int(m.group(1), 16)

    def size(self, op, default=4):
        w = op.split(" ")[0]
        return SIZES.get(w, default)

    def get(self, op, n=None):
        if op in self.r:
            return self.r[op]
        if op in SUB:
            reg, sh, mask = SUB[op]
            return (self.r[reg] >> sh) & mask
        if "PTR" in op or "[" in op or "ds:" in op:
            return self.ru(self.ea(op), n or self.size(op))
        return int(op, 0) & 0xFFFFFFFF

    def put(self, op, v, n=None):
        if op in self.r:
            self.r[op] = v & 0xFFFFFFFF
        elif op in SUB:
            reg, sh, mask = SUB[op]
            self.r[reg] = (self.r[reg] & ~(mask << sh)) | ((v & mask) << sh)
        else:
            self.wu(self.ea(op), n or self.size(op), v)

    def width(self, op):
        if op in self.r:
            return 4
        if op in SUB:
            return 2 if SUB[op][2] == 0xFFFF else 1
        return self.size(op)

    def fval(self, op):
        """An x87 source: st(i) or an m32/m64 operand."""
        m = re.fullmatch(r"st(?:\((\d)\))?", op)
        if m:
            return self.st[int(m.group(1) or 0)]
        if op.startswith("DWORD"):
            return self.mem_f(self.ea(op), 4)
        if op.startswith("QWORD"):
            return self.mem_f(self.ea(op), 8)
        raise ValueError("x87 operand " + op)

    def sti(self, op):
        m = re.fullmatch(r"st(?:\((\d)\))?", op)
        return None if m is None else int(m.group(1) or 0)

    # Flags.
    def setflags(self, res, n, cf=0, of=0):
        mask = (1 << (8 * n)) - 1
        res &= mask
        self.flags.update(ZF=int(res == 0), SF=(res >> (8 * n - 1)) & 1, CF=cf, OF=of,
                          PF=int(bin(res & 0xFF).count("1") % 2 == 0))

    def cond(self, cc):
        f = self.flags
        return {
            "e": f["ZF"], "z": f["ZF"], "ne": not f["ZF"], "nz": not f["ZF"],
            "p": f["PF"], "np": not f["PF"], "s": f["SF"], "ns": not f["SF"],
            "b": f["CF"], "ae": not f["CF"], "be": f["CF"] or f["ZF"], "a": not (f["CF"] or f["ZF"]),
            "l": f["SF"] != f["OF"], "ge": f["SF"] == f["OF"],
            "le": f["ZF"] or f["SF"] != f["OF"], "g": not f["ZF"] and f["SF"] == f["OF"],
        }[cc]

    def fcompare(self, a, b):
        if self.sym:
            a, b = (x if isinstance(x, Sym) else Sym(x, repr(float(x))) for x in (a, b))
            rel = "?" if a != a or b != b else "<" if a < b else "==" if a == b else ">"
            self.path.append("%x: %s %s %s" % (self.pc, a.e, rel, b.e))
            self.rels.append((self.pc, rel))
        a, b = float(a), float(b)
        if a != a or b != b:
            self.c = (1, 1, 1)
        elif a < b:
            self.c = (1, 0, 0)
        elif a == b:
            self.c = (0, 0, 1)
        else:
            self.c = (0, 0, 0)

    def push(self, v):
        self.r["esp"] = (self.r["esp"] - 4) & 0xFFFFFFFF
        self.wu(self.r["esp"], 4, v)

    def pop(self):
        v = self.ru(self.r["esp"], 4)
        self.r["esp"] = (self.r["esp"] + 4) & 0xFFFFFFFF
        return v

    # Running.
    def call(self, addr, args=(), regs=None, hooks=None, max_steps=200000):
        """Runs the function at `addr` with `args` pushed right to left (u32s;
        a float is pushed as its f32 bits) and `regs` set; returns eax. The
        x87 stack it leaves is `self.st`."""
        for a in reversed(args):
            self.push(a if isinstance(a, int) else struct.unpack("<I", struct.pack("<f", a))[0])
        for k, v in (regs or {}).items():
            self.r[k] = v
        self.push(RET_SENTINEL)
        hooks = hooks or {}
        pc = addr
        while pc != RET_SENTINEL:
            self.steps += 1
            if self.steps > max_steps:
                raise RuntimeError("runaway at %#x" % pc)
            if pc in hooks:
                hooks[pc](self)
                pc = self.pop()
                continue
            mn, ops, nxt = self.insn(pc)
            self.pc = pc
            pc = self.step(pc, mn, ops, nxt)
        return self.r["eax"]

    def step(self, pc, mn, ops, nxt):
        st = self.st
        if mn in ("int3",) or mn.startswith("(bad"):
            raise RuntimeError("%s at %#x" % (mn, pc))
        if mn == "nop":
            return nxt
        if mn == "mov":
            n = self.width(ops[0])
            self.put(ops[0], self.get(ops[1], n), n)
        elif mn in ("movzx", "movsx"):
            n = self.size(ops[1], 1)
            v = self.get(ops[1], n)
            if mn == "movsx" and v >> (8 * n - 1):
                v -= 1 << (8 * n)
            self.put(ops[0], v)
        elif mn == "lea":
            self.put(ops[0], self.ea(ops[1]))
        elif mn in ("add", "sub", "cmp", "and", "or", "xor", "test", "adc", "sbb"):
            n = self.width(ops[0])
            a, b = self.get(ops[0], n), self.get(ops[1], n)
            mask = (1 << (8 * n)) - 1
            b &= mask
            sbit = 1 << (8 * n - 1)
            if mn in ("add", "adc"):
                res = a + b + (self.flags["CF"] if mn == "adc" else 0)
                of = int(((a ^ res) & (b ^ res) & sbit) != 0)
                self.setflags(res, n, cf=int(res > mask), of=of)
            elif mn in ("sub", "cmp", "sbb"):
                res = a - b - (self.flags["CF"] if mn == "sbb" else 0)
                of = int(((a ^ b) & (a ^ res) & sbit) != 0)
                self.setflags(res, n, cf=int(res < 0), of=of)
            else:
                res = {"and": a & b, "test": a & b, "or": a | b, "xor": a ^ b}[mn]
                self.setflags(res, n)
            if mn not in ("cmp", "test"):
                self.put(ops[0], res & mask, n)
        elif mn in ("shl", "shr", "sar"):
            n = self.width(ops[0])
            a, k = self.get(ops[0], n), self.get(ops[1]) & 31
            if mn == "sar" and a >> (8 * n - 1):
                a -= 1 << (8 * n)
            res = a << k if mn == "shl" else a >> k
            self.setflags(res, n)
            self.put(ops[0], res, n)
        elif mn == "neg":
            n = self.width(ops[0])
            a = self.get(ops[0], n)
            self.setflags(-a, n, cf=int(a != 0))
            self.put(ops[0], -a, n)
        elif mn == "imul":
            a = self.get(ops[1]) if len(ops) > 1 else None
            b = self.get(ops[2]) if len(ops) > 2 else self.get(ops[0])
            self.put(ops[0], (a or 0) * b)
        elif mn == "push":
            self.push(self.get(ops[0]))
        elif mn == "pop":
            self.put(ops[0], self.pop())
        elif mn == "call":
            self.push(nxt)
            return int(ops[0], 16)
        elif mn == "ret":
            to = self.pop()
            if ops:
                self.r["esp"] += int(ops[0], 0)
            return to
        elif mn == "jmp":
            return int(ops[0], 16)
        elif mn.startswith("j"):
            return int(ops[0], 16) if self.cond(mn[1:]) else nxt
        elif mn.startswith("set"):
            self.put(ops[0], int(bool(self.cond(mn[3:]))), 1)
        # x87.
        elif mn == "fld":
            v = self.fval(ops[0])
            st.insert(0, v)
        elif mn == "fldz":
            st.insert(0, Sym(0.0, "0") if self.sym else 0.0)
        elif mn == "fld1":
            st.insert(0, Sym(1.0, "1") if self.sym else 1.0)
        elif mn in ("fst", "fstp"):
            i = self.sti(ops[0])
            if i is not None:
                st[i] = st[0]
            elif ops[0].startswith("DWORD"):
                self.store_f(self.ea(ops[0]), st[0], 4)
            elif ops[0].startswith("QWORD"):
                self.store_f(self.ea(ops[0]), st[0], 8)
            else:
                raise ValueError(ops[0])
            if mn == "fstp":
                st.pop(0)
        elif mn == "fxch":
            i = self.sti(ops[0]) if ops else 1
            st[0], st[i] = st[i], st[0]
        elif mn == "fchs":
            st[0] = -st[0]
        elif mn == "fabs":
            st[0] = abs(st[0])
        elif mn == "fsqrt":
            v = math.sqrt(float(st[0])) if st[0] >= 0 else float("nan")
            st[0] = Sym(v, "sqrt(%s)" % st[0].e) if isinstance(st[0], Sym) else v
        elif mn[:4] in ("fadd", "fsub", "fmul", "fdiv"):
            op = mn[1:4]
            rev = mn[4:5] == "r"
            popit = mn.endswith("p")
            if not ops:
                ops = ["st(1)", "st"]
            if len(ops) == 1:
                dst, a, b = 0, st[0], self.fval(ops[0])
            else:
                dst = self.sti(ops[0])
                a, b = st[dst], self.fval(ops[1])
            if rev:
                a, b = b, a
            st[dst] = self.arith(op, a, b)
            if popit:
                st.pop(0)
        elif mn in ("fcom", "fcomp", "fucom", "fucomp"):
            self.fcompare(st[0], self.fval(ops[0]) if ops else st[1])
            if mn.endswith("p"):
                st.pop(0)
        elif mn in ("fcompp", "fucompp"):
            self.fcompare(st[0], st[1])
            st.pop(0)
            st.pop(0)
        elif mn in ("fcomi", "fcomip", "fucomi", "fucomip"):
            # the compare straight into ZF, PF, CF (cod4_lnxded's x87 code)
            self.fcompare(st[0], st[self.sti(ops[1])] if len(ops) > 1 else st[1])
            c0, c2, c3 = self.c
            self.flags.update(ZF=c3, PF=c2, CF=c0)
            if mn.endswith("p"):
                st.pop(0)
        elif mn.startswith("fcmov"):
            cc = {"b": "b", "nb": "ae", "e": "e", "ne": "ne", "be": "be", "nbe": "a", "u": "p", "nu": "np"}[mn[5:]]
            if self.cond(cc):
                st[0] = st[self.sti(ops[1])]
        elif mn == "fnstsw":
            c0, c2, c3 = self.c
            self.put(ops[0], (c0 << 8) | (c2 << 10) | (c3 << 14))
        else:
            raise NotImplementedError("%s %s at %#x" % (mn, ops, pc))
        if len(st) > 8:
            raise RuntimeError("x87 stack overflow at %#x" % pc)
        return nxt

    @staticmethod
    def arith(op, a, b):
        if op == "add":
            return a + b
        if op == "sub":
            return a - b
        if op == "mul":
            return a * b
        if b == 0:
            if a == 0 or a != a:
                return float("nan")
            return math.copysign(float("inf"), math.copysign(1, a) * math.copysign(1, b))
        return a / b
