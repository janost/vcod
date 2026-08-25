#!/usr/bin/env python3
"""Find {char* name; int offset; int bits} netfield tables in a PE by scanning for
runs of entries whose name pointer resolves to a plausible field-name string.

    python3 tools/re/netfields.py CoDMP.exe
"""
import sys, struct, re

path = sys.argv[1]
d = open(path, 'rb').read()
pe = struct.unpack_from('<I', d, 0x3c)[0]
nsec = struct.unpack_from('<H', d, pe + 6)[0]
optsz = struct.unpack_from('<H', d, pe + 20)[0]
imagebase = struct.unpack_from('<I', d, pe + 24 + 28)[0]
secs = []
off = pe + 24 + optsz
for i in range(nsec):
    name = d[off:off+8].rstrip(b'\0').decode()
    vs, va, rs, pr = struct.unpack_from('<IIII', d, off + 8)
    secs.append((name, va, vs, pr, rs))
    off += 40

def va2off(va):
    rva = va - imagebase
    for name, sva, vs, pr, rs in secs:
        if sva <= rva < sva + rs:
            return pr + (rva - sva)
    return None

ident = re.compile(rb'[A-Za-z_][A-Za-z0-9_\[\]\.]{1,31}\x00')

def name_at(va):
    o = va2off(va)
    if o is None:
        return None
    m = ident.match(d, o)
    if not m:
        return None
    return d[o:m.end()-1].decode()

tables = []
for sname, sva, vs, pr, rs in secs:
    if sname not in ('.rdata', '.data'):
        continue
    i = pr
    end = pr + rs - 12
    while i <= end:
        p, off_, bits = struct.unpack_from('<Iii', d, i)
        n = name_at(p) if p > imagebase else None
        if n and -32 <= bits <= 32 and 0 <= off_ < 4096:
            run = []
            j = i
            while j <= end:
                p2, o2, b2 = struct.unpack_from('<Iii', d, j)
                n2 = name_at(p2) if p2 > imagebase else None
                if n2 is None or not (-32 <= b2 <= 32) or not (0 <= o2 < 4096):
                    break
                run.append((n2, o2, b2))
                j += 12
            if len(run) >= 10:
                tables.append((imagebase + sva + (i - pr), run))
            i = j if j > i else i + 12
        else:
            i += 4

for va, run in tables:
    print("== table VA 0x%08x, %d entries" % (va, len(run)))
    for k, (n, o, b) in enumerate(run):
        print("  %3d %-24s off=%-4d bits=%d" % (k, n, o, b))
