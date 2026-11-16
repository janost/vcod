# gdb python for cod2_lnxded 1.3: dumps each player DObj's anim calc and
# skeleton build under G_DObjCalcPose (0x811e3c0), the format
# crates/cod2/tests/fixtures/skel/ heads with. Load with
# `gdb -batch -x tools/re/cod2_skel_gdb.py --args cod2_lnxded ...`;
# CODSKEL_OUT names the output file, CODSKEL_EVERY keeps every n-th build,
# CODSKEL_BT=1 heads each build with its callers' return addresses.
import os
import struct

import gdb

OUT = open(os.environ.get("CODSKEL_OUT", "cod2-skel-gdb.txt"), "w")
EVERY = int(os.environ.get("CODSKEL_EVERY", "1"))
BT = os.environ.get("CODSKEL_BT") == "1"
LEVEL_TIME = 0x0864C56C
inf = None
state = {"on": False, "n": 0, "keep": False, "dobj": 0, "ent": 0}


def mem(addr, size):
    return bytes(inf.read_memory(addr, size))


def u32(addr):
    return struct.unpack("<I", mem(addr, 4))[0]


def u16(addr):
    return struct.unpack("<H", mem(addr, 2))[0]


def u8(addr):
    return mem(addr, 1)[0]


def cstr(addr):
    return mem(addr, 64).split(b"\0")[0].decode("latin-1")


def reg(name):
    return int(gdb.parse_and_eval("$" + name)) & 0xFFFFFFFF


def arg(i):
    # cdecl at a function's first instruction: [esp] the return address.
    return u32(reg("esp") + 4 + 4 * i)


def w(line):
    if state["keep"]:
        OUT.write(line + "\n")


def mats(dobj, tag):
    skel = u32(dobj + 4)
    n = u8(dobj + 0x19)
    raw = mem(skel + 0x30, 0x20 * n)
    for i in range(n):
        words = struct.unpack("<8I", raw[0x20 * i : 0x20 * i + 0x20])
        w("%s %d %s" % (tag, i, " ".join("%08x" % x for x in words)))


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


def pose_start():
    # 0x811e3f9: no skeleton under this stamp; the build runs.
    ent = u32(reg("ebp") + 8)
    state["on"] = True
    state["ent"] = u32(ent)
    state["n"] += 1
    state["keep"] = state["n"] % EVERY == 0
    if BT:
        # The ebp chain: each frame's saved ebp, then its return address.
        rets, ebp = [], reg("ebp")
        for _ in range(14):
            if not ebp:
                break
            rets.append("%08x" % u32(ebp + 4))
            ebp = u32(ebp)
        w("bt %s" % " ".join(rets))


def calc_anim():
    if not state["on"]:
        return
    dobj = arg(0)
    state["dobj"] = dobj
    models = []
    for m in range(u8(dobj + 0x18)):
        xm = u32(dobj + 0x1C + 4 * m)
        parts = u32(xm)
        if state["n"] == 1:
            OUT.write("# xmodel %d %s\n" % (m, mem(xm, 0x40).hex()))
        models.append("%d:%d" % (u16(parts), u16(parts + 2)))
    w(
        "calc %d ent %d bones %d models %s time %d"
        % (state["n"], state["ent"], u8(dobj + 0x19), ",".join(models), u32(LEVEL_TIME))
    )


def walk():
    if not state["on"]:
        return
    dobj, node, weight, _m, clear, nested = (arg(i) for i in range(6))
    tree = u32(dobj)
    xanim = u32(tree)
    info = u16(tree + 8 + 2 * node)
    ent = xanim + 0xC + 8 * node
    kids = u16(ent)
    t = u32(0x085BC90C + 0x28 * info) if info else 0
    iw = u32(0x085BC920 + 0x28 * info) if info else 0
    if kids == 0:
        parts = u32(ent + 4)
        w("walk %d %08x %d %d info %d %08x %08x leaf %s" % (node, weight, clear & 0xFF, nested & 0xFF, info, t, iw, cstr(u32(parts + 0x24))))
    else:
        first = u16(ent + 6)
        w("walk %d %08x %d %d info %d %08x %08x kids %d first %d" % (node, weight, clear & 0xFF, nested & 0xFF, info, t, iw, kids, first))


def leaf(kind):
    def f():
        if not state["on"]:
            return
        parts = arg(0)
        if kind == "one":
            w("leaf one %s w %08x" % (cstr(u32(parts + 0x24)), arg(2)))
        else:
            w("leaf %s %s t %08x w %08x" % (kind, cstr(u32(parts + 0x24)), arg(2), arg(3)))
    return f


def pre_skel():
    if not state["on"]:
        return
    dobj = state["dobj"]
    skel = u32(dobj + 4)
    ctrl = mem(skel + 0x10, 16).hex()
    w("ctrl %s" % ctrl)
    mats(dobj, "pre")


def post_skel():
    if not state["on"]:
        return
    mats(state["dobj"], "post")
    state["on"] = False
    OUT.flush()


def bone_start():
    # 0x811e472: G_DObjCalcBone's build (0x811e454), the bone's bits.
    ent = u32(reg("ebp") + 8)
    state["on"] = True
    state["ent"] = u32(ent)
    state["n"] += 1
    state["keep"] = True
    w("bone %d ent %d index %d" % (state["n"], state["ent"], u32(reg("ebp") + 0xC)))


def controllers():
    # BG_Player_DoControllers (0x80dbc70): its clientInfo_t +0x380..+0x4b8
    # and the frame time.
    if not state["on"]:
        return
    ci = arg(3)
    w("ci %d %s" % (arg(4), mem(ci + 0x380, 0x138).hex()))


def set_tag():
    # 0x80bbd20(obj, trans, angles, bone): a local tag set.
    if not state["on"]:
        return
    trans, angles, bone = arg(1), arg(2), arg(3)
    a = mem(angles, 12).hex() if angles else "-"
    w("tag %d trans %s angles %s" % (bone, mem(trans, 12).hex(), a))


def world_tag():
    # 0x811e5ed: G_DObjGetWorldTagMatrix's result, origin row last.
    out = u32(reg("ebp") + 0x10)
    OUT.write("worldtag ent %d %s\n" % (u32(u32(reg("ebp") + 8)), mem(out, 48).hex()))
    OUT.flush()


Bp(0x811E3F9, pose_start)
Bp(0x811E472, bone_start)
Bp(0x811E4D2, pre_skel)
Bp(0x811E4E4, post_skel)
Bp(0x80DBC70, controllers)
Bp(0x80BBD20, set_tag)
Bp(0x811E5ED, world_tag)
Bp(0x80C1260, calc_anim)
Bp(0x80BFC04, walk)
Bp(0x80C2FFA, leaf("lo"))
Bp(0x80C336E, leaf("hi"))
Bp(0x80BD20E, leaf("one"))
Bp(0x811E440, pre_skel)
Bp(0x811E452, post_skel)
gdb.execute("handle SIGPIPE nostop noprint pass")
gdb.execute("run")
OUT.close()
