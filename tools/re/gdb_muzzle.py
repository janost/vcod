# gdb script for the retail UO 1.51b dedicated server (`coduo_lnxded`):
# every `CalcMuzzlePoint` (game.mp.uo.i386.so 0x7b8ea), its playerstate
# inputs and the bob it added, read-only (cod11-combat.md 14.8).
#
#   GDB_OUT=<file> gdb -q -batch -ex 'source tools/re/gdb_muzzle.py' \
#       -ex run --args <coduo_lnxded> ...
#
# tools/run_probe.sh and tools/run_server.sh run the server this way under
# `GDB_PY=tools/re/gdb_muzzle.py`. One stop at 0x7bc4f, the shared exit,
# with the frame's locals still on the stack: the cycle `BG_GetBobCycle`
# (0x7baee) returned at ebp-0x2c, the speed `BG_GetSpeed` (0x7bb12) at
# ebp-0x30, the vertical and horizontal factors (0x7bb41, 0x7bb83) at
# ebp-0xc and ebp-0x10, `bg_bobMax` the cvar at [ebx+0xde4]+8. The ps is
# `ent->client` (ent+0x160). Floats as their bits in hex and as decimals:
#   muzzle t=<level.time> ent=<n> ct=<ps commandTime> bob=<ps+8 byte>
#    pm_flags=<ps+0xc> eflags=<ps+0x84>
#    origin=<ps+0x14 x3> vel=<ps+0x20 x3> view=<ps+0xe8 x3> eye=<ps+0xf8>
#    cycle= speed= max= up= side= out=<the muzzle x3>
import gdb, struct, os
out = open(os.environ['GDB_OUT'], 'w')
st = {'base': None}
def rd(a, n): return bytes(gdb.selected_inferior().read_memory(a, n))
def u32(a): return struct.unpack('<I', rd(a, 4))[0]
def f32(a): return struct.unpack('<f', rd(a, 4))[0]
def fl(a, n): return ','.join('%08x(%.9g)' % (u32(a + 4 * i), f32(a + 4 * i)) for i in range(n))
def reg(r): return int(gdb.parse_and_eval('$' + r)) & 0xffffffff
def log(s): out.write(s + '\n'); out.flush()
class B(gdb.Breakpoint):
    def __init__(self, spec, fn): super().__init__(spec, internal=True); self.fn = fn
    def stop(self): self.fn(); return False
def at_exit():
    e = reg('ebp'); ebx = reg('ebx')
    ent = u32(e + 8); ps = u32(ent + 0x160); o = u32(e + 0xc)
    # level.time at level+0x1ec (the `mov 0x1ec(%eax)` at 0x7bafc).
    t = struct.unpack('<i', rd(st['level'] + 0x1ec, 4))[0]
    log('muzzle t=%d ent=%d ct=%d bob=%d pm_flags=0x%x eflags=0x%x origin=%s vel=%s view=%s eye=%s '
        'cycle=%s speed=%s max=%s up=%s side=%s out=%s' % (
            t, u32(ent) & 0xffff, struct.unpack('<i', rd(ps, 4))[0], rd(ps + 8, 1)[0],
            u32(ps + 0xc), u32(ps + 0x84), fl(ps + 0x14, 3), fl(ps + 0x20, 3), fl(ps + 0xe8, 3),
            fl(ps + 0xf8, 1), fl(e - 0x2c, 1), fl(e - 0x30, 1), fl(u32(ebx + 0xde4) + 8, 1),
            fl(e - 0xc, 1), fl(e - 0x10, 1), fl(o, 3)))
def arm():
    if st['base'] is not None: return
    st['base'] = (int(gdb.parse_and_eval('(unsigned int)&CalcMuzzlePoint')) & 0xffffffff) - 0x7b8ea
    st['level'] = int(gdb.parse_and_eval('(unsigned int)&level')) & 0xffffffff
    log('base %x' % st['base'])
    B('*0x%x' % (st['base'] + 0x7bc4f), at_exit)
gdb.execute('set breakpoint pending on')
class Arm(gdb.Breakpoint):
    def __init__(self): super().__init__('CalcMuzzlePoint', internal=True)
    def stop(self): arm(); self.enabled = False; return False
Arm()
