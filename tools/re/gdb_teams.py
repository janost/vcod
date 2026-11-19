# gdb script for the retail CoD1 dedicated servers (1.1d `cod_lnxded`,
# 1.5b `cod_lnxded`, UO 1.51b `coduo_lnxded`): the team pick of `Use_Item`
# and `RespawnItem`, read-only (cod11-items.md 14.8).
#
#   GDB_GAME=11|15|uo GDB_OUT=<file> \
#       gdb -q -batch -ex 'source tools/re/gdb_teams.py' -ex run \
#       --args <server> ...
#
# tools/run_probe.sh and tools/run_server.sh run the server this way under
# `GDB_PY=tools/re/gdb_teams.py`. Lines:
#   GDB use <entity>        Use_Item's self
#   GDB respawn <entity>    RespawnItem's self
#   GDB draw <rand()>       the team branch's draw
#   GDB pick <entity>       the entity the body then respawns
# An entity is its `s.number`, the gentity's first word.
import gdb
import os
import struct

GAMES = {
    # function offsets in the game module; each break is (offset, register
    # or None for the first stack argument at the function's entry)
    '11': dict(anchor='Use_Item', anchor_off=0x4efc8, use=0x4efc8,
               respawn=0x4ec7c, draws=(0x4f015, 0x4ecd0),
               picks=((0x4f029, 'ebx'), (0x4ece9, 'ebx'))),
    '15': dict(anchor='Use_Item', anchor_off=0x4c79c, use=0x4c79c,
               respawn=0x4b3fb, draws=(0x4b471,), picks=((0x4b4b1, 'eax'),)),
    'uo': dict(anchor='Use_Item', anchor_off=0x559af, use=0x559af,
               respawn=0x545db, draws=(0x54651,), picks=((0x54691, 'eax'),)),
}
G = GAMES[os.environ.get('GDB_GAME', '11')]
OUT = open(os.environ['GDB_OUT'], 'w', buffering=1)
state = {'base': None}


def u32(a):
    return struct.unpack('<I', bytes(gdb.selected_inferior().read_memory(a, 4)))[0]


def reg(name):
    return int(gdb.parse_and_eval('$' + name)) & 0xffffffff


class Entry(gdb.Breakpoint):
    def __init__(self, at, tag):
        super().__init__('*0x%x' % at, internal=True)
        self.tag = tag

    def stop(self):
        ent = u32(reg('esp') + 4)
        OUT.write('GDB %s %d\n' % (self.tag, u32(ent)))
        return False


class Draw(gdb.Breakpoint):
    def stop(self):
        OUT.write('GDB draw %d\n' % reg('eax'))
        return False


class Pick(gdb.Breakpoint):
    def __init__(self, at, r):
        super().__init__('*0x%x' % at, internal=True)
        self.r = r

    def stop(self):
        OUT.write('GDB pick %d\n' % u32(reg(self.r)))
        return False


def on_objfile(_ev):
    if state['base'] is not None:
        return
    try:
        a = int(gdb.parse_and_eval('(unsigned int)&%s' % G['anchor'])) & 0xffffffff
    except gdb.error:
        return
    b = a - G['anchor_off']
    state['base'] = b
    Entry(b + G['use'], 'use')
    Entry(b + G['respawn'], 'respawn')
    for d in G['draws']:
        Draw('*0x%x' % (b + d), internal=True)
    for at, r in G['picks']:
        Pick(b + at, r)


for sig in ('SIGPIPE', 'SIGUSR1', 'SIGUSR2', 'SIGALRM'):
    gdb.execute('handle %s nostop noprint pass' % sig)
gdb.execute('set pagination off')
gdb.events.new_objfile.connect(on_objfile)
