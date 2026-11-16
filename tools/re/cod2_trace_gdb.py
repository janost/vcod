# gdb python for cod2_lnxded 1.3: dumps each DObjTraceline (0x80bb27e) a
# locational trace runs on a player, the format
# crates/cod2/tests/fixtures/trace/ heads with. Load with
# `gdb -batch -x tools/re/cod2_trace_gdb.py --args cod2_lnxded ...`
# (tools/run_probe_cod2.sh's GDB_PY); CODTRACE_OUT names the output file.
import os
import struct

import gdb

OUT = open(os.environ.get("CODTRACE_OUT", "cod2-trace-gdb.txt"), "w")
LEVEL_TIME = 0x864C56C
# SV_ResetSkeletonCache's stamp (0x8091edc).
STAMP = 0x849F630
inf = None
state = {"n": 0, "ent": -1}


def mem(addr, size):
    return bytes(inf.read_memory(addr, size))


def u32(addr):
    return struct.unpack("<I", mem(addr, 4))[0]


def u8(addr):
    return mem(addr, 1)[0]


def reg(name):
    return int(gdb.parse_and_eval("$" + name)) & 0xFFFFFFFF


def arg(i):
    # cdecl at a function's first instruction: [esp] the return address.
    return u32(reg("esp") + 4 + 4 * i)


def words(b):
    return " ".join("%08x" % x for x in struct.unpack("<%dI" % (len(b) // 4), b))


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


def caller():
    # 0x809d1e8, SV_PointTraceToEntity's call: the clip's world line
    # ([ebp+8] +0, +0xc) and the entity's axis and origin (ebp-0xc8).
    ebp = reg("ebp")
    clip = u32(ebp + 8)
    ent = u32(ebp - 0xC)
    state["ent"] = u32(ent)
    OUT.write(
        "point %d ent %d line %s axis %s\n"
        % (u32(LEVEL_TIME), state["ent"], words(mem(clip, 24)), words(mem(ebp - 0xC8, 48)))
    )


def trace_start():
    dobj, start, end, prio, result = (arg(i) for i in range(5))
    state["n"] += 1
    n = u8(dobj + 0x19)
    skel = u32(dobj + 4)
    OUT.write(
        "trace %d time %d stamp %d ent %d map %08x mask %02x bones %d start %s end %s frac %08x\n"
        % (
            state["n"],
            u32(LEVEL_TIME),
            u32(STAMP),
            state["ent"],
            prio,
            u8(dobj + 0x1A),
            n,
            words(mem(start, 12)),
            words(mem(end, 12)),
            u32(result),
        )
    )
    raw = mem(skel + 0x30, 0x20 * n)
    for i in range(n):
        OUT.write("mat %d %s\n" % (i, words(raw[0x20 * i : 0x20 * i + 0x20])))
    state["ent"] = -1


def build():
    # 0x811e3f9: G_DObjCalcPose found no skeleton under the stamp; it builds.
    ent = u32(u32(reg("ebp") + 8))
    OUT.write("build time %d stamp %d ent %d\n" % (u32(LEVEL_TIME), u32(STAMP), ent))


def trace_end():
    # 0x80bbb29: every path's exit; the result's fraction, normal and the
    # part name and location shorts (+0x14, +0x16).
    result = u32(reg("ebp") + 0x18)
    OUT.write("result %d %s\n" % (state["n"], words(mem(result, 0x18))))
    OUT.flush()


Bp(0x809D1E8, caller)
Bp(0x80BB27E, trace_start)
Bp(0x80BBB29, trace_end)
Bp(0x811E3F9, build)
gdb.execute("handle SIGPIPE nostop noprint pass")
gdb.execute("run")
OUT.close()
