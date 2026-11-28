# gdb script for the retail UO 1.51b dedicated server (`coduo_lnxded`):
# every `G_CheckHitTriggerDamage` (game.mp.uo.i386.so 0x77130), the walk a
# missile impact or a bullet runs over the `trigger_damage`s, read-only
# (cod11-combat.md 14.8.3).
#
#   GDB_OUT=<file> gdb -q -batch -ex 'source tools/re/gdb_trigger_walk.py' \
#       -ex run --args <coduo_lnxded> ...
#
# tools/run_probe.sh runs the server this way under
# `GDB_PY=tools/re/gdb_trigger_walk.py`. One stop past the prologue (gdb's
# stop on the name; 0x77130 pushes ebp first), the cdecl arguments off
# ebp+8 on: the attacker entity (its number, `s.number` at
# +0), start and end (three floats each), damage and the means of death:
#   walk t=<level.time> attacker=<n> start=<x,y,z> end=<x,y,z> dmg=<n> mod=<n>
# Floats as their bits in hex and as decimals.
import gdb, struct, os
out = open(os.environ['GDB_OUT'], 'w')
st = {'level': None}
def rd(a, n): return bytes(gdb.selected_inferior().read_memory(a, n))
def u32(a): return struct.unpack('<I', rd(a, 4))[0]
def i32(a): return struct.unpack('<i', rd(a, 4))[0]
def f32(a): return struct.unpack('<f', rd(a, 4))[0]
def fl(a, n): return ','.join('%08x(%.9g)' % (u32(a + 4 * i), f32(a + 4 * i)) for i in range(n))
def reg(r): return int(gdb.parse_and_eval('$' + r)) & 0xffffffff
def log(s): out.write(s + '\n'); out.flush()
class Walk(gdb.Breakpoint):
    def __init__(self): super().__init__('G_CheckHitTriggerDamage', internal=True)
    def stop(self):
        if st['level'] is None:
            st['level'] = int(gdb.parse_and_eval('(unsigned int)&level')) & 0xffffffff
        sp = reg('ebp') + 4
        attacker = u32(sp + 4)
        # level.time at level+0x1ec, as gdb_muzzle.py reads it.
        log('walk t=%d attacker=%d start=%s end=%s dmg=%d mod=%d' % (
            i32(st['level'] + 0x1ec), (u32(attacker) & 0xffff) if attacker else -1,
            fl(u32(sp + 8), 3), fl(u32(sp + 12), 3), i32(sp + 16), i32(sp + 20)))
        return False
gdb.execute('set breakpoint pending on')
Walk()
