#!/usr/bin/env python3
"""Find raw 4-byte immediates equal to a given VA anywhere in a PE, report section+VA.

    python3 tools/re/xref.py <pe> <va>
"""
import sys, struct

path, target = sys.argv[1], int(sys.argv[2], 0)
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

needle = struct.pack('<I', target)
i = 0
while True:
    i = d.find(needle, i)
    if i < 0:
        break
    for name, va, vs, pr, rs in secs:
        if pr <= i < pr + rs:
            print("%s file=0x%x VA=0x%08x" % (name, i, imagebase + va + (i - pr)))
            break
    i += 1
