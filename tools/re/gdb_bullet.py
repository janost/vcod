# gdb script for the retail 1.1d dedicated server (`cod_lnxded`): every
# Bullet_Fire_Extended leg's trace at its glass test (game.mp.i386.so
# 0x68b08): the hit entity's number, eType, r.contents and takedamage, the
# trace's endpos, surfaceFlags and contents; and every G_Damage with its
# target's eType, moverState and use (cod11-movers.md 26.9).
#
#   GDB_OUT=<file> gdb -q -batch -ex 'source tools/re/gdb_bullet.py' \
#       -ex run --args <cod_lnxded> ...
#
# tools/run_probe.sh and tools/run_server.sh run the server this way under
# `GDB_PY=tools/re/gdb_bullet.py`.
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
    st['base'] = (int(gdb.parse_and_eval('(unsigned int)&Bullet_Fire_Extended')) & 0xffffffff) - 0x68890
    log('base %x' % st['base'])
    B('*0x%x' % (st['base'] + 0x68b08), at_glass)
def entry():
    arm()
def at_glass():
    e = reg('ebp'); x = reg('esi')
    log('BFE ent %d etype %d rcontents %x takedamage %d endpos %.2f %.2f %.2f sflags %x contents %x' % (
        u32(x), u32(x+4), u32(x+0x118), rd(x+0x171,1)[0], f32(e-0x2c), f32(e-0x28), f32(e-0x24), u32(e-0x14), u32(e-0x10)))
def damage():
    e = reg('ebp'); t = u32(e + 8)
    log('GD targ %d dmg %d mod %d etype %d state %d use %x' % (u32(t), u32(e+0x1c), u32(e+0x24), u32(t+4), rd(t+0x174,1)[0], u32(t+0x210)))
gdb.execute('set breakpoint pending on')
B('Bullet_Fire_Extended', entry)
B('G_Damage', damage)
