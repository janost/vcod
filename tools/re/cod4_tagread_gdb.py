# gdb python for cod4_lnxded 1.7 (not the package's cod4_lnxded-bin, 1.6):
# a player body's end-frame swings, controller runs and script tag reads,
# every float as its bits, into $SKEL_OUT (cod4-player-anims.md 8.1,
# cod4-combat.md 6.1). Lines:
#   "srand SEED": each srand(); the second is G_InitGame's, a Pair's seed.
#   "angles TIME ES CI": BG_PlayerAngles (0x804c636), then its three
#   "swing TIME DEST TOL CLAMP SPEED ANGLE SWINGING -> ANGLE SWINGING"
#     (BG_SwingAngles 0x804b3ba to its return): torso yaw, legs yaw, pitch.
#   "ctlin TIME ENT FRAMETIME EFLAGS ES+0xdc ES+0xe0 | CONTROLS" at
#     BG_Player_DoControllersInternal (0x804cf90), CONTROLS the 24 words at
#     CI+0x3f8, then "ci ENT WORDS" (the whole clientInfo_t) and
#     "ctlout CONTROLS" as G_PlayerController (0x80a484d) got them back.
#   "tagmat TIME ENT BONE QUAT TRANS TRANSWEIGHT | ORIGIN ANGLES": the bone's
#     DObjAnimMat G_DObjGetWorldTagMatrix (0x80cc7f6) read, the entity's
#     r.currentOrigin and r.currentAngles; "tagout ROWS ORIGIN" its result
#     as a script's getTag* got it (0x80c0053).
# Run the server as `gdb -batch -nx -x tools/re/cod4_tagread_gdb.py --args
# ./cod4_lnxded <the capture's server line>` inside the capture's namespace,
# then the capture's probes; replayed on ours as a `capture_ab` Pair (the
# second seed), the dump says which frame and which word part.
import os
import struct

import gdb

OUT = open(os.environ.get("SKEL_OUT", "skel-gdb.txt"), "w", buffering=1 << 20)
LEVEL_TIME = 0x0836F8AC
inf = None
st = {"swing": [], "tagout": None}


def mem(a, n):
    return bytes(inf.read_memory(a, n))


def u32(a):
    return struct.unpack("<I", mem(a, 4))[0]


def words(a, n):
    return " ".join("%08x" % w for w in struct.unpack("<%dI" % n, mem(a, 4 * n)))


def reg(n):
    return int(gdb.parse_and_eval("$" + n)) & 0xFFFFFFFF


def arg(i):
    return u32(reg("esp") + 4 + 4 * i)


def now():
    return struct.unpack("<i", mem(LEVEL_TIME, 4))[0]


class B(gdb.Breakpoint):
    def __init__(self, addr, fn):
        super().__init__("*0x%x" % addr, internal=True)
        self.fn = fn

    def stop(self):
        global inf
        inf = gdb.selected_inferior()
        try:
            self.fn()
        except Exception as e:  # keep the server running
            OUT.write("# err %r\n" % e)
        return False


def on_srand():
    OUT.write("srand %d\n" % arg(0))


def on_ctlin():
    es, ci, ft = arg(0), arg(1), arg(2)
    OUT.write(
        "ctlin %d %d %d %08x %s | %s\n"
        % (now(), u32(es), ft, u32(es + 8), words(es + 0xDC, 2), words(ci + 0x3F8, 24))
    )
    OUT.write("ci %d %s\n" % (u32(es), words(ci, 0x4C4 // 4)))


def on_ctlout():
    OUT.write("ctlout %s\n" % words(reg("ebx") + 4 + 0x3F8, 24))


def on_swing():
    ang, swg = reg("eax"), reg("edx")
    st["swing"].append((ang, swg))
    OUT.write(
        "swing %d %s %08x %08x" % (now(), words(reg("esp") + 4, 4), u32(ang), u32(swg))
    )


def on_swing_ret():
    if not st["swing"]:
        return
    ang, swg = st["swing"].pop()
    OUT.write(" -> %08x %08x\n" % (u32(ang), u32(swg)))


def on_angles():
    OUT.write("angles %d %08x %08x\n" % (now(), arg(0), arg(1)))


def on_tagmat():
    ent = reg("edi")
    base, off = reg("eax"), reg("ebx")
    OUT.write(
        "tagmat %d %d %d %s | %s %s\n"
        % (now(), u32(ent), off >> 5, words(base + off, 8), words(ent + 0x13C, 3), words(ent + 0x148, 3))
    )


def on_tagmat_entry():
    st["tagout"] = arg(2)


def on_tagret():
    if st["tagout"] is not None:
        OUT.write("tagout %s\n" % words(st["tagout"], 12))
        st["tagout"] = None


B(0x0804A20C, on_srand)
B(0x0804CF90, on_ctlin)
B(0x080A484D, on_ctlout)
B(0x0804B3BA, on_swing)
for r in (0x0804C712, 0x0804C7CD, 0x0804CC68, 0x0804CE0C, 0x0804CF56):
    B(r, on_swing_ret)
B(0x0804C636, on_angles)
B(0x080CC7BA, on_tagmat_entry)
B(0x080CC7F6, on_tagmat)
B(0x080C0053, on_tagret)
gdb.execute("set pagination off")
gdb.execute("handle SIGPIPE nostop noprint pass")
gdb.execute("handle SIGUSR1 nostop noprint pass")
gdb.execute("run")
OUT.close()
