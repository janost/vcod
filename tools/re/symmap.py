#!/usr/bin/env python3
"""Transfer function and data names from a symbolized build of a program to
a stripped build of the same version: written for CoD2 1.3, the Mac MP
binary's i386 slice (`llvm-lipo -thin i386`) onto `cod2_lnxded`.

usage: symmap.py MAC_BIN MAC_FUNCS_TSV LNX_BIN LNX_FUNCS_TSV OUT

MAC_BIN is the named side (a Mach-O or ELF with symbols), LNX_BIN the
stripped one; both must be non-PIC 32-bit x86.

The *.funcs.tsv come from ExportFuncGraph.java. Passes, each recorded per
pair as its method:
  table   {name, fn, int} records (gsc builtins, commands) joined by name
  strings functions whose ordered string list is unique on both sides
  strset  same with the unordered string set
  calls   callee lists of matched pairs, diff-aligned; equal-length gaps
          pair positionally; a candidate is kept only if every vote agrees
  scalars functions whose set of immediates (struct offsets, constants) is
          unique on both sides, at least four of them
  calls   token streams (calls, strings, data refs in instruction order) of
          matched pairs, diff-aligned; inside each unaligned gap the unknown
          functions and data refs pair positionally when both sides hold as
          many of a kind; a candidate is kept only if every vote agrees
  callers unmatched functions whose matched-caller set is unique on both sides
  callees the same with the callee set, every callee matched
  data    the same with the data-ref set, every ref matched
Every pass but table rejects a pair whose sizes differ by more than 2x
(1.5x under 64 bytes): the two compilers inline differently, and a callee
folded into its caller looks like the caller to every pass. Without that
filter CoD2 1.3 matched a third more functions at about 8% wrong.
Data names are the named side's data symbols at 4-aligned refs the token
alignment paired.

Output lines: `addr kind name method named_addr`, kind T (function) or D.
"""
import collections
import difflib
import struct
import subprocess
import sys


class Image:
    """Address -> bytes for a 32-bit ELF (program headers) or Mach-O
    (LC_SEGMENT) image."""

    def __init__(self, path):
        d = self.d = open(path, "rb").read()
        self.segs = []
        if d[:4] == b"\x7fELF":
            phoff, = struct.unpack("<I", d[0x1C:0x20])
            phentsize, phnum = struct.unpack("<HH", d[0x2A:0x2E])
            for i in range(phnum):
                o = phoff + i * phentsize
                typ, off, vaddr, _, filesz = struct.unpack("<IIIII", d[o:o + 20])
                if typ == 1:
                    self.segs.append((vaddr, off, filesz))
        else:
            magic, _, _, _, ncmds, _, _ = struct.unpack("<7I", d[:28])
            assert magic == 0xFEEDFACE, hex(magic)
            o = 28
            for _ in range(ncmds):
                cmd, size = struct.unpack("<II", d[o:o + 8])
                if cmd == 1:
                    vaddr, _, fileoff, filesize = struct.unpack("<IIII", d[o + 24:o + 40])
                    self.segs.append((vaddr, fileoff, filesize))
                o += size

    def off(self, a):
        for v, o, n in self.segs:
            if v <= a < v + n:
                return o + a - v
        return None

    def u32(self, a):
        o = self.off(a)
        return None if o is None else struct.unpack("<I", self.d[o:o + 4])[0]

    def cstr(self, a):
        o = self.off(a)
        if o is None or a == 0:
            return None
        e = self.d.find(b"\0", o, o + 128)
        if e <= o:
            return None
        s = self.d[o:e]
        if not all(0x20 <= c < 0x7F for c in s):
            return None
        return s.decode()

    def records(self, funcs):
        """{name ptr, fn ptr, small int} runs of at least 4: name -> fn."""
        out = {}
        for v, o, n in self.segs:
            for a in range(v, v + n - 12, 4):
                run = []
                b = a
                while True:
                    p, f, x = (self.u32(b), self.u32(b + 4), self.u32(b + 8))
                    if p is None or f not in funcs or x is None or x > 1:
                        break
                    s = self.cstr(p)
                    if s is None:
                        break
                    run.append((s, f))
                    b += 12
                if len(run) >= 4:
                    for s, f in run:
                        out.setdefault(s, set()).add(f)
        return {s: next(iter(f)) for s, f in out.items() if len(f) == 1}


def load(path):
    funcs = {}
    for line in open(path, encoding="utf-8", errors="replace"):
        e, name, size, calls, strs, data, sc, toks = (line.rstrip("\n").split("\t") + [""] * 8)[:8]
        funcs[int(e, 16)] = dict(
            # Ghidra keeps the Mach-O C symbol's leading underscore
            name=name[1:] if name.startswith("_") and not name.startswith("__") else name,
            size=int(size),
            calls=[int(c.lstrip("&"), 16) for c in calls.split(",") if c],
            strs=[s for s in strs.split("\x1f") if s],
            data=[int(x, 16) for x in data.split(",") if x],
            sc=sc,
            toks=[(t[0], t[2:] if t[0] == "s" else int(t[2:], 16)) for t in toks.split("\x1e") if t],
        )
    return funcs


def mac_data_syms(path):
    out = {}
    nm = subprocess.run(["llvm-nm", "--defined-only", path], capture_output=True, text=True).stdout
    dem = subprocess.run(["llvm-cxxfilt"], input=nm, capture_output=True, text=True).stdout
    for line in dem.splitlines():
        parts = line.split(" ", 2)
        if len(parts) == 3 and parts[1] in "dDbBsS":
            name = parts[2]
            if name.endswith(".eh"):
                continue
            out.setdefault(int(parts[0], 16), name.lstrip("_") if not name.startswith("__") else name[1:])
    return out


def main():
    mac_bin, mac_tsv, lnx_bin, lnx_tsv, out = sys.argv[1:6]
    M, L = load(mac_tsv), load(lnx_tsv)
    match = {}  # mac -> lnx
    rev = {}
    how = {}

    def pair(m, l, method):
        if m in match or l in rev or m not in M or l not in L:
            return False
        if method != "table":
            # inlining differs between the two compilers: a callee folded
            # into its caller on one side looks like the caller to every pass
            a, b = sorted((M[m]["size"], L[l]["size"]))
            if b > 2 * a or (a < 64 and b > 1.5 * a):
                return False
        match[m], rev[l], how[m] = l, m, method
        return True

    # table: records joined by their name string
    mi, li = Image(mac_bin), Image(lnx_bin)
    mt, lt = mi.records(M), li.records(L)
    for s in mt.keys() & lt.keys():
        pair(mt[s], lt[s], "table")

    # strings: unique ordered list, then unique set
    for keyf, method in ((lambda f: tuple(f["strs"]), "strings"),
                         (lambda f: frozenset(f["strs"]), "strset")):
        km, kl = collections.defaultdict(list), collections.defaultdict(list)
        for a, f in M.items():
            if len(set(f["strs"])) >= (2 if method == "strset" else 1) and a not in match:
                km[keyf(f)].append(a)
        for a, f in L.items():
            if len(set(f["strs"])) >= (2 if method == "strset" else 1) and a not in rev:
                kl[keyf(f)].append(a)
        for k, ms in km.items():
            ls = kl.get(k)
            if len(ms) == 1 and ls and len(ls) == 1:
                pair(ms[0], ls[0], method)

    def unique_keys(keyf, method, minlen):
        """Pair functions whose key is unique on both sides."""
        km, kl = collections.defaultdict(list), collections.defaultdict(list)
        for a, f in M.items():
            if a not in match:
                k = keyf("m", a, f)
                if k is not None and len(k) >= minlen:
                    km[k].append(a)
        for a, f in L.items():
            if a not in rev:
                k = keyf("l", a, f)
                if k is not None and len(k) >= minlen:
                    kl[k].append(a)
        n = 0
        for k, ms in km.items():
            ls = kl.get(k)
            if len(ms) == 1 and ls and len(ls) == 1 and pair(ms[0], ls[0], method):
                n += 1
        return n

    def align():
        """Diff-align the token streams of every matched pair. Strings match
        themselves, functions and data match through the pairs known so far;
        inside each unaligned gap the unknown functions and the unknown data
        refs pair positionally when both sides hold as many of that kind.
        Returns unanimous (mac, lnx) votes per kind."""
        votes = {"c": collections.defaultdict(collections.Counter),
                 "d": collections.defaultdict(collections.Counter)}
        known = {"c": (match, rev), "d": (dmap, drev)}
        for m, l in match.items():
            a, b = M[m]["toks"], L[l]["toks"]
            ka = [t if t[0] == "s" else ((t[0], known[t[0]][0][t[1]]) if t[1] in known[t[0]][0] else ("?m",) + t)
                  for t in a]
            kb = [t if t[0] == "s" else (t if t[1] in known[t[0]][1] else ("?l",) + t) for t in b]
            sm = difflib.SequenceMatcher(None, ka, kb, autojunk=False)
            for tag, i1, i2, j1, j2 in sm.get_opcodes():
                if tag == "equal":
                    continue
                for kind in "cd":
                    xs = [t[1] for t in a[i1:i2] if t[0] == kind and t[1] not in known[kind][0]]
                    ys = [t[1] for t in b[j1:j2] if t[0] == kind and t[1] not in known[kind][1]]
                    if xs and len(xs) == len(ys):
                        for x, y in zip(xs, ys):
                            votes[kind][x][y] += 1
        out = {}
        for kind, v in votes.items():
            lv = collections.defaultdict(set)
            for x, c in v.items():
                for y in c:
                    lv[y].add(x)
            out[kind] = [(x, next(iter(c))) for x, c in v.items()
                         if len(c) == 1 and len(lv[next(iter(c))]) == 1]
        return out

    # scalars: struct offsets and constants survive both compilers
    unique_keys(lambda s, a, f: frozenset(f["sc"].split(",")) if f["sc"] else None, "scalars", 4)

    dmap, drev = {}, {}  # mac data addr <-> lnx data addr
    while True:
        added = 0
        votes = align()
        for m, l in votes["c"]:
            added += pair(m, l, "calls")
        for x, y in votes["d"]:
            if x not in dmap and y not in drev:
                dmap[x], drev[y] = y, x
                added += 1
        dl = set(drev)

        cm, cl = collections.defaultdict(set), collections.defaultdict(set)
        for a, f in M.items():
            if a in match:
                for c in f["calls"]:
                    cm[c].add(match[a])
        for a, f in L.items():
            if a in rev:
                for c in f["calls"]:
                    cl[c].add(a)
        added += unique_keys(lambda s, a, f: frozenset((cm if s == "m" else cl).get(a, ())) or None,
                             "callers", 2)
        added += unique_keys(
            lambda s, a, f: (frozenset(match[c] for c in f["calls"]) if all(c in match for c in f["calls"])
                             else None) if s == "m" else
                            (frozenset(f["calls"]) if all(c in rev for c in f["calls"]) else None),
            "callees", 2)
        added += unique_keys(
            lambda s, a, f: (frozenset(dmap[d] for d in f["data"]) if f["data"] and all(d in dmap for d in f["data"])
                             else None) if s == "m" else
                            (frozenset(f["data"]) if f["data"] and all(d in dl for d in f["data"]) else None),
            "data", 2)
        if not added:
            break

    msyms = mac_data_syms(mac_bin)
    # an unaligned pair is a string or byte the alignment mistook for a variable
    data = {y: msyms[x] for x, y in dmap.items() if x in msyms and y % 4 == 0 and x % 4 == 0}

    with open(out, "w") as fo:
        fo.write(f"# names transferred from {mac_bin} to {lnx_bin} (symmap.py)\n")
        fo.write("# addr kind name method named_addr\n")
        for m, l in sorted(match.items(), key=lambda x: x[1]):
            fo.write(f"{l:08x} T {M[m]['name'].replace(' ', '_')} {how[m]} {m:08x}\n")
        for l, n in sorted(data.items()):
            fo.write(f"{l:08x} D {n.replace(' ', '_')} data -\n")
    named = sum(1 for f in M.values() if not f["name"].startswith("FUN_"))
    print(f"mac funcs {len(M)} (named {named}), lnx funcs {len(L)}")
    print(f"matched {len(match)}: " + ", ".join(f"{k} {v}" for k, v in collections.Counter(how.values()).most_common()))
    print(f"data names {len(data)}")


if __name__ == "__main__":
    main()
