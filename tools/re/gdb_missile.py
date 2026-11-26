# gdb script for the retail 1.1d dedicated server (`cod_lnxded`): every
# G_RunMissile move trace and ground-snap trace, read-only
# (cod11-combat.md 12.4).
#
#   GDB_OUT=<file> gdb -q -batch -ex 'source tools/re/gdb_missile.py' \
#       -ex run --args <cod_lnxded> ...
#
# tools/run_probe.sh and tools/run_server.sh run the server this way under
# `GDB_PY=tools/re/gdb_missile.py`. Addresses are game.mp.i386.so 1.1d's:
# 0x540c6 / 0x540cb around the move trace's call, 0x542a4 / 0x542a9 around
# the snap's, both into the trace at ebp-0x48 from `r.currentOrigin`
# (ent+0x134) to the end at ebp-0xc. Lines, floats as their bits in hex:
#   move|snap t=<level.time> ent=<n> from x y z to x y z
#    tr f <fraction> end x y z n x y z ent <entityNum> bone <id> loc <hitLoc> solid <all> <start>
import gdb, struct, os
out = open(os.environ['GDB_OUT'], 'w')
st = {'base': None}
def rd(a, n): return bytes(gdb.selected_inferior().read_memory(a, n))
def u32(a): return struct.unpack('<I', rd(a, 4))[0]
def u16(a): return struct.unpack('<H', rd(a, 2))[0]
def hx(a, n): return ' '.join('%08x' % u32(a + 4 * i) for i in range(n))
def reg(r): return int(gdb.parse_and_eval('$' + r)) & 0xffffffff
def log(s): out.write(s + '\n'); out.flush()
class B(gdb.Breakpoint):
    def __init__(self, spec, fn): super().__init__(spec, internal=True); self.fn = fn
    def stop(self): self.fn(); return False
def level_time():
    if 'level' not in st:
        st['level'] = int(gdb.parse_and_eval('(unsigned int)&level')) & 0xffffffff
    # level.time at level+0x1e8 (the `mov 0x1e8,%eax` relocation in G_RunMissile).
    return struct.unpack('<i', rd(st['level'] + 0x1e8, 4))[0]
def before(kind):
    def f():
        e = reg('ebp'); ent = reg('edi')
        log('%s t=%d ent=%d from %s to %s' % (kind, level_time(), u16(ent), hx(ent + 0x134, 3), hx(e - 0xc, 3)))
    return f
def after():
    t = reg('ebp') - 0x48
    log(' tr f %s end %s n %s ent %d bone %d loc %d solid %d %d' % (hx(t, 1), hx(t + 4, 3), hx(t + 16, 3),
        u16(t + 40), u16(t + 42), u16(t + 44), rd(t + 46, 1)[0], rd(t + 47, 1)[0]))
def arm():
    if st['base'] is not None: return
    st['base'] = (int(gdb.parse_and_eval('(unsigned int)&G_RunMissile')) & 0xffffffff) - 0x53fcc
    st['level'] = int(gdb.parse_and_eval('(unsigned int)&level')) & 0xffffffff
    b = st['base']
    log('base %x' % b)
    B('*0x%x' % (b + 0x540c6), before('move'))
    B('*0x%x' % (b + 0x540cb), after)
    B('*0x%x' % (b + 0x542a4), before('snap'))
    B('*0x%x' % (b + 0x542a9), after)
gdb.execute('set breakpoint pending on')
class Arm(gdb.Breakpoint):
    def __init__(self): super().__init__('G_RunMissile', internal=True)
    def stop(self): arm(); self.enabled = False; return False
Arm()
