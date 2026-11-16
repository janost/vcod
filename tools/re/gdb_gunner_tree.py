# gdb script for the retail 1.1d `cod_lnxded`: one player's anim tree and
# skeleton records, read-only, once a frame (cod11-turrets.md 13.9, "The
# mount's blend in").
#
#   GDB_ENT=<entity number> GDB_OUT=<file> gdb -q -batch \
#       -ex 'source tools/re/gdb_gunner_tree.py' -ex run --args cod_lnxded ...
#
# Run it as `COD_LNXDED` for tools/run_probe.sh through a wrapper that execs
# gdb with these arguments. Per frame it writes:
#
#   step t=<level time> dobj=<addr>   at the entity's weight step (0x80bef88)
#    ps ct= pmf= pmt= origin= vel= ground= legsTimer= es=<entity bytes 0..0x30>
#    n <node> <info> <0x44 info bytes>   each node with an info record
#   skel t= dobj= nb= ctrl= built=    at the first DObjCalcSkel (0x80c337c)
#    r <bone> <32 record bytes>          in the frame's first thread pass
#
# An info record: node time +0x28, goal time +0x34, goal +0x38, weight +0x3c,
# rate +0x40 (0x80bb130). A record: quat x y z w, weight, trans x y z.
# Python breakpoints cost a few ms a stop: the skeleton breakpoint is armed
# only from G_RunFrame's entry to the entity's weight step, or a server with
# traces every frame hitches.
import gdb
import os
import struct

ENT = int(os.environ.get('GDB_ENT', '1'))
OUT = open(os.environ['GDB_OUT'], 'w', buffering=1)
INFO = 0x82f59a0  # the anim info table, 0x44 a record
NINFO = 2048
NODES = 254  # animtrees/multiplayer.atr's nodes (the tree header's +4)
# Entity number to DObj (0x806e498).
SLOTS, DOBJS, DOBJ_SIZE = 0x812a600, 0x8113c80, 0x58
# game.mp.i386.so offsets, off the module's load address.
TURRET_THINK_CLIENT, G_RUN_FRAME, G_ENTITIES, LEVEL = 0x52340, 0x50478, 0x21df80, 0x190340
LEVEL_TIME, GENTITY_SIZE, GENTITY_CLIENT = 0x1e8, 0x314, 0x158

state = {'base': None, 'last_skel': None}


def rd(a, n):
    return bytes(gdb.selected_inferior().read_memory(a, n))


def u32(a):
    return struct.unpack('<I', rd(a, 4))[0]


def esp():
    return int(gdb.parse_and_eval('$esp')) & 0xffffffff


def dobj():
    slot = struct.unpack('<h', rd(SLOTS + 2 * ENT, 2))[0]
    return DOBJS + DOBJ_SIZE * slot if slot else None


def level_time():
    return u32(state['base'] + LEVEL + LEVEL_TIME)


class Step(gdb.Breakpoint):
    def stop(self):
        if state['base'] is None:
            return False
        d = u32(esp() + 4)
        if d != dobj():
            return False
        SKEL.enabled = False
        tree = u32(d)
        if tree == 0:
            return False
        OUT.write('step t=%d dobj=%x\n' % (level_time(), d))
        ent = state['base'] + G_ENTITIES + GENTITY_SIZE * ENT
        ps = rd(u32(ent + GENTITY_CLIENT), 0x80)
        o = struct.unpack_from('<3f', ps, 0x14)
        v = struct.unpack_from('<3f', ps, 0x20)
        i32 = lambda off: struct.unpack_from('<i', ps, off)[0]
        OUT.write(' ps ct=%d pmf=%x pmt=%d origin=%.6f,%.6f,%.6f vel=%.3f,%.3f,%.3f'
                  ' ground=%d legsTimer=%d es=%s\n' % (
                      i32(0), i32(0xc), i32(0x10), *o, *v, i32(0x54), i32(0x6c),
                      rd(ent, 0x30).hex()))
        info = rd(INFO, 0x44 * NINFO)
        idx = rd(tree + 8, 2 * NODES)
        for k in range(NODES):
            i = struct.unpack_from('<H', idx, 2 * k)[0]
            if i and i < NINFO:
                OUT.write(' n %d %d %s\n' % (k, i, info[0x44 * i:0x44 * i + 0x44].hex()))
        return False


class CalcSkel(gdb.Breakpoint):
    def stop(self):
        d = u32(esp() + 4)
        if state['base'] is None or d != dobj():
            return False
        t = level_time()
        if state['last_skel'] == t:
            return False
        state['last_skel'] = t
        self.enabled = False
        head = rd(d, 0x60)
        nb = head[0x17]
        skel = u32(d + 4)
        hdr = rd(skel, 0x30)
        recs = rd(skel + 0x30 + nb * 64, nb * 32)
        OUT.write('skel t=%d dobj=%x nb=%d ctrl=%s built=%s\n' % (
            t, d, nb, hdr[0x10:0x20].hex(), hdr[0x20:0x30].hex()))
        for i in range(nb):
            OUT.write(' r %d %s\n' % (i, recs[32 * i:32 * i + 32].hex()))
        return False


class RunFrame(gdb.Breakpoint):
    def stop(self):
        SKEL.enabled = dobj() is not None
        return False


def on_objfile(_ev):
    if state['base'] is not None:
        return
    try:
        a = int(gdb.parse_and_eval('(unsigned int)&turret_think_client')) & 0xffffffff
    except gdb.error:
        return
    state['base'] = a - TURRET_THINK_CLIENT
    RunFrame('*0x%x' % (state['base'] + G_RUN_FRAME), internal=True)


for sig in ('SIGPIPE', 'SIGUSR1', 'SIGUSR2', 'SIGALRM'):
    gdb.execute('handle %s nostop noprint pass' % sig)
gdb.execute('set pagination off')
gdb.events.new_objfile.connect(on_objfile)
Step('*0x80bef88', internal=True)
SKEL = CalcSkel('*0x80c337c', internal=True)
SKEL.enabled = False
