# gdb python for cod2_lnxded 1.3: each client's anim tree around the end
# frame's anim set (0x80dbdbc, from ClientEndFrame), the infos that hold a
# weight or a time. Load with `gdb -batch -x tools/re/cod2_anim_gdb.py
# --args cod2_lnxded ...`; CODANIM_OUT names the output file.
# "end LEVELTIME CLIENT legs LEGS torso TORSO": the entity's anims going in;
# "pre|post NODE INFO <the 0x28-byte info as words>": each info of the
# client's tree (ci +0x4a4) before and after the set. cod2-player-anims.md 7.
import os
import struct

import gdb

OUT = open(os.environ.get("CODANIM_OUT", "cod2-anim-gdb.txt"), "w")
LEVEL_TIME = 0x0864C56C
CI = 0x087035BC
CI_SIZE = 0x4B8
INFOS = 0x085BC900
inf = None
state = {"ci": 0}


def mem(addr, size):
    return bytes(inf.read_memory(addr, size))


def u32(addr):
    return struct.unpack("<I", mem(addr, 4))[0]


def u16(addr):
    return struct.unpack("<H", mem(addr, 2))[0]


def reg(name):
    return int(gdb.parse_and_eval("$" + name)) & 0xFFFFFFFF


def arg(i):
    return u32(reg("esp") + 4 + 4 * i)


def infos(ci, tag):
    tree = u32(ci + 0x4A4)
    if tree == 0:
        return
    count = u32(u32(tree) + 4)
    for n in range(count):
        idx = u16(tree + 8 + 2 * n)
        if idx == 0:
            continue
        words = struct.unpack("<10I", mem(INFOS + 0x28 * idx, 0x28))
        OUT.write("%s %d %d %s\n" % (tag, n, idx, " ".join("%08x" % x for x in words)))


class Bp(gdb.Breakpoint):
    def __init__(self, addr, fn):
        super().__init__("*0x%x" % addr, internal=True)
        self.fn = fn

    def stop(self):
        global inf
        inf = gdb.selected_inferior()
        try:
            self.fn()
        except gdb.MemoryError as e:
            OUT.write("# error %s\n" % e)
        return False


def enter():
    es, ci = arg(1), arg(2)
    state["ci"] = ci
    client = (ci - CI) // CI_SIZE
    OUT.write(
        "end %d %d legs %d torso %d\n"
        % (u32(LEVEL_TIME), client, u32(es + 0xCC), u32(es + 0xD0))
    )
    infos(ci, "pre")


def leave():
    infos(state["ci"], "post")
    OUT.flush()


Bp(0x80DBDBC, enter)
Bp(0x80DBEAE, leave)
gdb.execute("handle SIGPIPE nostop noprint pass")
gdb.execute("run")
OUT.close()
