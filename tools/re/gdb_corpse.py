# gdb script for the retail UO 1.51b dedicated server (`coduo_lnxded`):
# each client cmd's vehicle overlap test in `ClientThink_real`
# (game.mp.uo.i386.so 0x40c91, coduo-vehicles.md 11.6) and the `Pmove`
# that follows it, and a dead body's steps through `PmoveSingle`, read-only
# (coduo-vehicles.md 11.6.1).
#
#   GDB_OUT=<file> gdb -q -batch -ex 'source tools/re/gdb_corpse.py' \
#       -ex run --args <coduo_lnxded> ...
#
# tools/run_probe.sh and tools/run_server.sh run the server this way under
# `GDB_PY=tools/re/gdb_corpse.py`. Three stops in the function's frame
# (ent at ebp+8, its client, the ps, at ebp-0xc, the cmd at ebp+0xc):
#   0x41631, past the `trap_TraceCapsule` position test: the trace's
#     allsolid (ebp-0x27a), startsolid (ebp-0x279) and entityNum
#     (ebp-0x280), and for an `eType` 12 entity its `r.currentOrigin`
#     (+0x13c) and the three floats after it;
#   0x41a80, the `Pmove` call: the entity `ClientThink_real` unlinked
#     (ebp-0x2ac, 0 for none) and the velocity the push left;
#   0x41a85, past it: the ps the move left.
# A seated client jumps past the test (0x41554) and logs only the last two.
# Floats as their bits in hex and as decimals:
#   test t=<level.time> ent=<n> st=<cmd serverTime> ct=<ps commandTime>
#    pm_type= pm_flags= origin= vel= solid=<allsolid><startsolid> hit=<n>
#    [veh=<r.currentOrigin x3, +0x148 x3>]
#   move t= ent= st= unlinked=<n> vel=
#   moved t= ent= ct= pm_flags= ground=<ps+0x58> origin= vel=
#   pm <point> ent=<ps clientNum> ct= pm_flags= ground= origin= vel=
#    box=<pm mins, maxs> psbox=<ps+0x55c x6>
# The `pm` lines come from the `PM_POINTS` stops below.
import gdb, struct, os
out = open(os.environ['GDB_OUT'], 'w')
st = {'base': None}
def rd(a, n): return bytes(gdb.selected_inferior().read_memory(a, n))
def u32(a): return struct.unpack('<I', rd(a, 4))[0]
def i32(a): return struct.unpack('<i', rd(a, 4))[0]
def f32(a): return struct.unpack('<f', rd(a, 4))[0]
def fl(a, n): return ','.join('%08x(%.9g)' % (u32(a + 4 * i), f32(a + 4 * i)) for i in range(n))
def reg(r): return int(gdb.parse_and_eval('$' + r)) & 0xffffffff
def log(s): out.write(s + '\n'); out.flush()
class B(gdb.Breakpoint):
    def __init__(self, spec, fn): super().__init__(spec, internal=True); self.fn = fn
    def stop(self): self.fn(); return False
def frame():
    e = reg('ebp')
    ent = u32(e + 8)
    return e, ent, u32(e - 0xc), u32(e + 0xc), i32(st['level'] + 0x1ec), u32(ent) & 0xffff
def at_test():
    e, ent, ps, cmd, t, n = frame()
    hit = struct.unpack('<H', rd(e - 0x280, 2))[0]
    line = 'test t=%d ent=%d st=%d ct=%d pm_type=%d pm_flags=0x%x origin=%s vel=%s solid=%d%d hit=%d' % (
        t, n, i32(cmd), i32(ps), i32(ps + 4), u32(ps + 0xc), fl(ps + 0x14, 3), fl(ps + 0x20, 3),
        rd(e - 0x27a, 1)[0], rd(e - 0x279, 1)[0], hit)
    if hit < 1022:
        v = st['ents'] + hit * 0x34c
        if u32(v + 4) == 12:
            line += ' veh=%s' % fl(v + 0x13c, 6)
    log(line)
def at_move():
    e, ent, ps, cmd, t, n = frame()
    un = u32(e - 0x2ac)
    log('move t=%d ent=%d st=%d unlinked=%d vel=%s' % (
        t, n, i32(cmd), (u32(un) & 0xffff) if un else 0, fl(ps + 0x20, 3)))
def at_moved():
    e, ent, ps, cmd, t, n = frame()
    log('moved t=%d ent=%d ct=%d pm_flags=0x%x ground=%d origin=%s vel=%s' % (
        t, n, i32(ps), u32(ps + 0xc), i32(ps + 0x58), fl(ps + 0x14, 3), fl(ps + 0x20, 3)))
# Inside `PmoveSingle`'s default arm, for a dead body (`pm_type` 6 and up)
# only: the ps at each named point, `pm` at [[ebx+0xfe8]], its ps at pm+0.
PM_POINTS = {
    0x2dfcf: 'ground0', 0x2dfe8: 'pitch', 0x2dfed: 'pitched', 0x2e002: 'deadmove',
    0x2e011: 'ladder', 0x2e044: 'moved', 0x2e049: 'ground1', 0x2e1bb: 'snap',
}
def at_pm(name):
    pm = u32(u32(reg('ebx') + 0xfe8))
    ps = u32(pm)
    if i32(ps + 4) < 6: return
    log('pm %s ent=%d ct=%d pm_flags=0x%x ground=%d origin=%s vel=%s box=%s psbox=%s' % (
        name, i32(ps + 0xd4), i32(ps), u32(ps + 0xc), i32(ps + 0x58), fl(ps + 0x14, 3),
        fl(ps + 0x20, 3), fl(pm + 0xd8, 6), fl(ps + 0x55c, 6)))
def arm():
    if st['base'] is not None: return
    st['base'] = (int(gdb.parse_and_eval('(unsigned int)&ClientThink_real')) & 0xffffffff) - 0x40c91
    st['level'] = int(gdb.parse_and_eval('(unsigned int)&level')) & 0xffffffff
    st['ents'] = int(gdb.parse_and_eval('(unsigned int)&g_entities')) & 0xffffffff
    log('base %x' % st['base'])
    B('*0x%x' % (st['base'] + 0x41631), at_test)
    B('*0x%x' % (st['base'] + 0x41a80), at_move)
    B('*0x%x' % (st['base'] + 0x41a85), at_moved)
    for a, name in PM_POINTS.items():
        B('*0x%x' % (st['base'] + a), lambda name=name: at_pm(name))
gdb.execute('set breakpoint pending on')
class Arm(gdb.Breakpoint):
    def __init__(self): super().__init__('ClientThink_real', internal=True)
    def stop(self): arm(); self.enabled = False; return False
Arm()
