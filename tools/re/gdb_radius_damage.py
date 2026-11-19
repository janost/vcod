# gdb script for the retail 1.1d dedicated server (`cod_lnxded`): every
# G_RadiusDamage call, each candidate past the ignore test with its
# takedamage, r.bmodel and link box, CanDamage's result, the second chance's
# trace fraction, and every G_Damage with its target's eType, moverState and
# use (cod11-movers.md 26.9).
#
#   GDB_OUT=<file> gdb -q -batch -ex 'source tools/re/gdb_radius_damage.py' \
#       -ex run --args <cod_lnxded> ...
#
# tools/run_probe.sh and tools/run_server.sh run the server this way under
# `GDB_PY=tools/re/gdb_radius_damage.py`. Addresses are game.mp.i386.so 1.1d's:
# 0x4a4d3 (the takedamage test), 0x4a606 (past the CanDamage call),
# 0x4a749 (past the second chance's trap_Trace).
import gdb, struct, os
out = open(os.environ['GDB_OUT'], 'w')
st = {'base': None}
def rd(a, n): return bytes(gdb.selected_inferior().read_memory(a, n))
def u32(a): return struct.unpack('<I', rd(a, 4))[0]
def f32(a): return struct.unpack('<f', rd(a, 4))[0]
def reg(r): return int(gdb.parse_and_eval('$' + r)) & 0xffffffff
def log(s): out.write(s + '\n'); out.flush()
class B(gdb.Breakpoint):
    def __init__(self, spec, fn): super().__init__(spec, internal=True); self.fn = fn
    def stop(self): self.fn(); return False
def arm():
    if st['base'] is not None: return
    st['base'] = (int(gdb.parse_and_eval('(unsigned int)&G_RadiusDamage')) & 0xffffffff) - 0x4a3f4
    b = st['base']
    log('base %x' % b)
    B('*0x%x' % (b + 0x4a4d3), cand)
    B('*0x%x' % (b + 0x4a606), cand_ret)
    B('*0x%x' % (b + 0x4a749), trace_ret)
def radius():
    arm(); e = reg('ebp'); o = u32(e + 8)
    log('RD %.2f %.2f %.2f inner %.1f outer %.1f r %.1f' % (f32(o), f32(o+4), f32(o+8), f32(e+0x14), f32(e+0x18), f32(e+0x1c)))
def cand():
    x = reg('ebx'); log(' cand %d takedamage %d bmodel %d abs %s %s' % (u32(x), rd(x+0x171,1)[0], u32(x+0xfc),
        [round(f32(x+0x11c+4*i),1) for i in range(3)], [round(f32(x+0x128+4*i),1) for i in range(3)]))
def cand_ret():
    log(' candamage %s' % gdb.parse_and_eval('$st0'))
def trace_ret():
    e = reg('ebp'); log(' second trace fraction %f' % f32(e - 0x106c))
def damage():
    e = reg('ebp'); t = u32(e + 8)
    log('GD targ %d dmg %d mod %d etype %d state %d use %x' % (u32(t), u32(e+0x1c), u32(e+0x24), u32(t+4), rd(t+0x174,1)[0], u32(t+0x210)))
gdb.execute('set breakpoint pending on')
B('G_RadiusDamage', radius)
B('G_Damage', damage)
