# Dumps cod4_lnxded 1.7's weaponDefFields ({name*, offset, type}) and the
# weapon field type name tables. usage: dump_wfields.py <cod4_lnxded>
import struct, sys
d = open(sys.argv[1], 'rb').read()
BASE = 0x8048000  # vaddr - file offset for .rodata/.data (non-PIC exe)
def cstr(va):
    o = va - BASE
    if not (0 <= o < len(d)): return None
    e = d.find(b'\0', o)
    try: return d[o:e].decode('ascii')
    except: return None
name_va = d.find(b'\0displayName\0') + 1 + BASE
pat = struct.pack('<III', name_va, 4, 0)
at = d.find(pat)
assert at > 0
start = at
rows = []
o = start
while True:
    n, off, ty = struct.unpack_from('<III', d, o)
    s = cstr(n)
    if s is None or off > 0x2000 or ty > 64: break
    rows.append((s, off, ty)); o += 12
print('# table at', hex(start + BASE), len(rows), 'rows', file=sys.stderr)
for s, off, ty in rows: print(f'{s}\t{off}\t{ty}')
