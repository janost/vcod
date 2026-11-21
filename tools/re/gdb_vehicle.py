# gdb script for the retail UO 1.51b dedicated server (`coduo_lnxded`): a
# vehicle's physics state each think, read-only (coduo-vehicles.md 17.5).
#
#   GDB_OUT=<file> gdb -q -batch -ex 'source tools/re/gdb_vehicle.py' -ex run \
#       --args <server> ...
#
# tools/run_probe.sh and tools/run_server.sh run the server this way under
# `GDB_PY=tools/re/gdb_vehicle.py`. One line per think of a vehicle with a
# record, at the track clock's entry (0x86aa1, after the motion and the
# aims), each float as its bit pattern in hex:
#   GDB veh <entity> <level.time> o x y z a x y z v x y z lv x y z av x y z
#       sw <5 words> j <7 words>
# `o`, `a`, `v`, `lv` and `av` are the record's origin, angles, velocity,
# local velocity and angular velocity (`rec+0xb8` +0x00, +0x18, +0x30,
# +0x3c, +0x48); `sw` the five words at +0x54..+0x68, the steer and the
# body sway; `j` the jolt (`rec+0x2a4..0x2c0`: pitch, roll, duration,
# remaining, phase, speed, accel). And at the think's
# entry (`Scr_Vehicle_Think` 0x8c7cc), before the step, where another
# vehicle's hit has already written this one's velocity:
#   GDB think <entity> <level.time> v x y z o x y z c x y z ca x y z
# with the record's origin and the committed copy's origin and angles
# (`rec+0x170`, +0x00 and +0x18).
import gdb
import os
import struct

ANCHOR, ANCHOR_OFF = 'Scr_Vehicle_Think', 0x8c7cc
CLOCK = 0x86aa1
LEVEL_TIME = 0x23b520 + 0x1ec
OUT = open(os.environ['GDB_OUT'], 'w', buffering=1)
state = {'base': None}


def mem(a, n):
    return bytes(gdb.selected_inferior().read_memory(a, n))


def u32(a):
    return struct.unpack('<I', mem(a, 4))[0]


class Clock(gdb.Breakpoint):
    def stop(self):
        esp = int(gdb.parse_and_eval('$esp')) & 0xffffffff
        ent = u32(esp + 4)
        rec = u32(ent + 0x164)
        s = rec + 0xb8
        words = struct.unpack('<26I', mem(s, 0x68))
        jolt = struct.unpack('<7I', mem(rec + 0x2a4, 0x1c))
        pick = lambda o, n=3: ' '.join('%08x' % w for w in words[o // 4:o // 4 + n])
        OUT.write('GDB veh %d %d o %s a %s v %s lv %s av %s sw %s j %s\n' % (
            u32(ent), u32(state['base'] + LEVEL_TIME), pick(0), pick(0x18),
            pick(0x30), pick(0x3c), pick(0x48), pick(0x54, 5),
            ' '.join('%08x' % w for w in jolt)))
        return False


class Think(gdb.Breakpoint):
    def stop(self):
        esp = int(gdb.parse_and_eval('$esp')) & 0xffffffff
        ent = u32(esp + 4)
        rec = u32(ent + 0x164)
        if rec:
            hx = lambda a: ' '.join('%08x' % w for w in struct.unpack('<3I', mem(a, 12)))
            OUT.write('GDB think %d %d v %s o %s c %s ca %s\n' % (
                u32(ent), u32(state['base'] + LEVEL_TIME), hx(rec + 0xb8 + 0x30),
                hx(rec + 0xb8), hx(rec + 0x170), hx(rec + 0x170 + 0x18)))
        return False


def on_objfile(_ev):
    if state['base'] is not None:
        return
    try:
        a = int(gdb.parse_and_eval('(unsigned int)&%s' % ANCHOR)) & 0xffffffff
    except gdb.error:
        return
    state['base'] = a - ANCHOR_OFF
    Clock('*0x%x' % (state['base'] + CLOCK), internal=True)
    Think('*0x%x' % a, internal=True)


gdb.events.new_objfile.connect(on_objfile)
gdb.execute('set pagination off')
gdb.execute('set breakpoint pending on')
