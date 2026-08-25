#!/usr/bin/env python3
"""Find the EV_* name pointer array in a CoD cgame DLL and print it in index order.

    python3 tools/re/evtab.py cgame_mp_x86.dll
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

def rva2off(rva):
    for name, va, vs, pr, rs in secs:
        if va <= rva < va + max(vs, rs):
            return pr + (rva - va)
    return None

def va2off(va):
    return rva2off(va - imagebase)

# map: VA -> string, for every EV_ / SURF_ style ascii string
def cstr(o):
    e = d.index(b'\0', o)
    return d[o:e].decode('latin1')

strvas = {}
for m in re.finditer(rb'EV_[A-Z0-9_]+\x00', d):
    o = m.start()
    for name, va, vs, pr, rs in secs:
        if pr <= o < pr + rs:
            strvas[imagebase + va + (o - pr)] = cstr(o)
            break

print("# %d EV_ strings, imagebase=0x%x" % (len(strvas), imagebase))

# scan .rdata/.data for runs of pointers into strvas
best = None
for name, va, vs, pr, rs in secs:
    if name not in ('.rdata', '.data'):
        continue
    i = pr
    end = pr + rs
    while i + 4 <= end:
        p = struct.unpack_from('<I', d, i)[0]
        if p in strvas:
            run = []
            j = i
            while j + 4 <= end:
                q = struct.unpack_from('<I', d, j)[0]
                if q in strvas:
                    run.append(strvas[q])
                    j += 4
                else:
                    break
            if best is None or len(run) > len(best[2]):
                best = (name, imagebase + va + (i - pr), run)
            i = j
        else:
            i += 4

name, tabva, run = best
print("# table at VA 0x%08x in %s, %d entries" % (tabva, name, len(run)))
for i, s in enumerate(run):
    print("%d\t%s" % (i, s))
