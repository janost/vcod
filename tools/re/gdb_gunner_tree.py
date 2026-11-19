# gdb script for the retail CoD1 dedicated servers (1.1d `cod_lnxded`,
# 1.5b `cod_lnxded`, UO 1.51b `coduo_lnxded`): players' anim trees and
# skeleton records, read-only (cod11-turrets.md 13.9, 13.10).
#
#   GDB_GAME=11|15|uo GDB_MODE=grid|duel GDB_ENT=<entity> GDB_OUT=<file> \
#       gdb -q -batch -ex 'source tools/re/gdb_gunner_tree.py' -ex run \
#       --args <server> ...
#
# tools/run_probe.sh and tools/run_server.sh run the server this way under
# `GDB_PY=tools/re/gdb_gunner_tree.py`.
#
# `grid` (the default): entity GDB_ENT once a frame,
#   step t=<level time> dobj=<addr>   at his weight step
#    ps ...                           his playerstate, view at ps+0xc0 (1.1d only)
#    n <node> <info> <0x44 info bytes>   each node with an info record
#   skel t= dobj= nb= ctrl= built=    at the first skeleton build in the
#    r <bone> <32 record bytes>          frame's first thread pass
# `duel`: entities 0 and 1, the first skeleton build on each after the
#   frame's weight steps (the entity and end-frame passes), each as
#   `skel t= ent= ...` with its `n` and `r` lines.
#
# An info record: node time +0x28, goal time +0x34, goal +0x38, weight +0x3c,
# rate +0x40 (1.1d 0x80bb130, 1.5b 0x80c788a, UO 0x80bcc1e). A record: quat
# x y z w, weight, trans x y z. Python breakpoints cost a few ms a stop, so
# the skeleton breakpoint is armed only for the window it reads.
import gdb
import os
import struct

GAMES = {
    # info table, a node's info index at tree + off + 2 * node, the weight
    # step's entry (dobj, dt), entity -> DObj (slots, DObjs, size), the
    # skeleton build's entry (dobj, ...).
    '11': dict(info=0x82f59a0, idx=8, step=0x80bef88, slots=0x812a600,
               dobjs=0x8113c80, dobj_size=0x58, skel=0x80c337c,
               # game module: turret_think_client, G_RunFrame, level,
               # level.time, g_entities, gentity size, ->client
               ttc=0x52340, run_frame=0x50478, level=0x190340, time=0x1e8,
               ents=0x21df80, ent_size=0x314, client=0x158),
    '15': dict(info=0x830b000, idx=0xc, step=0x80c9d88, slots=0x8129bc0,
               dobjs=0x8112240, dobj_size=0x5c, skel=0x80d0902,
               ttc=0x52626, run_frame=0x505f5, level=0x18f220, time=0x1e8),
    'uo': dict(info=0x8434140, idx=0xc, step=0x80bf11c, slots=0x8252d00,
               dobjs=0x823b300, dobj_size=0x5c, skel=0x80c5c96,
               ttc=0x5be63, run_frame=0x59c21, level=0x23b520, time=0x1ec),
}
G = GAMES[os.environ.get('GDB_GAME', '11')]
MODE = os.environ.get('GDB_MODE', 'grid')
ENT = int(os.environ.get('GDB_ENT', '1'))
OUT = open(os.environ['GDB_OUT'], 'w', buffering=1)
NINFO = 2048

state = {'base': None, 'last_skel': None, 'seen': set()}


def rd(a, n):
    return bytes(gdb.selected_inferior().read_memory(a, n))


def u32(a):
    return struct.unpack('<I', rd(a, 4))[0]


def esp():
    return int(gdb.parse_and_eval('$esp')) & 0xffffffff


def dobj_of(num):
    slot = struct.unpack('<h', rd(G['slots'] + 2 * num, 2))[0]
    return G['dobjs'] + G['dobj_size'] * slot if slot else None


def level_time():
    return u32(state['base'] + G['level'] + G['time'])


def write_tree(d):
    tree = u32(d)
    # The tree's node count, its anims' header `+4` (254 on 1.1d).
    nodes = struct.unpack('<H', rd(u32(tree) + 4, 2))[0]
    info = rd(G['info'], 0x44 * NINFO)
    idx = rd(tree + G['idx'], 2 * nodes)
    for k in range(nodes):
        i = struct.unpack_from('<H', idx, 2 * k)[0]
        if i and i < NINFO:
            OUT.write(' n %d %d %s\n' % (k, i, info[0x44 * i:0x44 * i + 0x44].hex()))


def write_records(d):
    nb = rd(d, 0x60)[0x17]
    skel = u32(d + 4)
    recs = rd(skel + 0x30 + nb * 64, nb * 32)
    for i in range(nb):
        OUT.write(' r %d %s\n' % (i, recs[32 * i:32 * i + 32].hex()))


def skel_head(d):
    nb = rd(d, 0x60)[0x17]
    hdr = rd(u32(d + 4), 0x30)
    return 'nb=%d ctrl=%s built=%s' % (nb, hdr[0x10:0x20].hex(), hdr[0x20:0x30].hex())


class Step(gdb.Breakpoint):
    def stop(self):
        if state['base'] is None:
            return False
        d = u32(esp() + 4)
        if MODE == 'duel':
            if d == dobj_of(1):
                state['seen'] = set()
                SKEL.enabled = True
            return False
        if d != dobj_of(ENT):
            return False
        SKEL.enabled = False
        if u32(d) == 0:
            return False
        OUT.write('step t=%d dobj=%x\n' % (level_time(), d))
        if 'client' in G:
            ent = state['base'] + G['ents'] + G['ent_size'] * ENT
            ps = rd(u32(ent + G['client']), 0xd0)
            o = struct.unpack_from('<3f', ps, 0x14)
            v = struct.unpack_from('<3f', ps, 0x20)
            i32 = lambda off: struct.unpack_from('<i', ps, off)[0]
            view = struct.unpack_from('<3f', ps, 0xc0)
            OUT.write(' ps ct=%d pmf=%x pmt=%d origin=%.6f,%.6f,%.6f vel=%.3f,%.3f,%.3f'
                      ' ground=%d legsTimer=%d view=%.6f,%.6f,%.6f es=%s\n' % (
                          i32(0), i32(0xc), i32(0x10), *o, *v, i32(0x54), i32(0x6c),
                          *view, rd(ent, 0x30).hex()))
        write_tree(d)
        return False


class CalcSkel(gdb.Breakpoint):
    def stop(self):
        if state['base'] is None:
            return False
        d = u32(esp() + 4)
        t = level_time()
        if MODE == 'duel':
            who = [n for n in (0, 1) if dobj_of(n) == d]
            if not who or who[0] in state['seen']:
                return False
            state['seen'].add(who[0])
            if len(state['seen']) == 2:
                self.enabled = False
            OUT.write('skel t=%d ent=%d dobj=%x %s\n' % (t, who[0], d, skel_head(d)))
            write_tree(d)
            write_records(d)
            return False
        if d != dobj_of(ENT) or state['last_skel'] == t:
            return False
        state['last_skel'] = t
        self.enabled = False
        OUT.write('skel t=%d dobj=%x %s\n' % (t, d, skel_head(d)))
        write_records(d)
        return False


class RunFrame(gdb.Breakpoint):
    def stop(self):
        SKEL.enabled = MODE == 'grid' and dobj_of(ENT) is not None
        return False


def on_objfile(_ev):
    if state['base'] is not None:
        return
    try:
        a = int(gdb.parse_and_eval('(unsigned int)&turret_think_client')) & 0xffffffff
    except gdb.error:
        return
    state['base'] = a - G['ttc']
    RunFrame('*0x%x' % (state['base'] + G['run_frame']), internal=True)


for sig in ('SIGPIPE', 'SIGUSR1', 'SIGUSR2', 'SIGALRM'):
    gdb.execute('handle %s nostop noprint pass' % sig)
gdb.execute('set pagination off')
gdb.events.new_objfile.connect(on_objfile)
Step('*0x%x' % G['step'], internal=True)
SKEL = CalcSkel('*0x%x' % G['skel'], internal=True)
SKEL.enabled = False
