# gdb python for the CoD4 1.7 Windows client (iw3mp.exe) under
# tools/cod4_wine/run.sh: drives its own player to a death and dumps every
# step of the ragdoll world (world 2) from the first ragdoll to rest
# (cod4-player-anims.md 10.4; the line format is
# crates/cod4/tests/fixtures/README.md's, ragdoll/).
#
#   COD4_DIR=<install> RAGDOLL_OUT=$PWD/tmp/ragdoll.txt \
#       GDB_PY=$PWD/tools/re/cod4_ragdoll_gdb.py \
#       tools/cod4_wine/run.sh +devmap mp_crash +set fixedtime 17
#
# fixedtime goes after devmap: it is a cheat dvar, and devmap's load sets
# sv_cheats first (the driver sets it again on its first frame). The driver counts frames at Com_ModifyMsec (0x4ffce0)
# from the first with cl_ingame set (dvar at 0xc5f8f0) and appends each
# command to local client 0's command buffer as Cbuf_AddText (0x4f8d90)
# does: data, size and length at 0x1410bd4, 0x1410bd8, 0x1410bdc.
#
#   RAGDOLL_OUT   the dump (default ragdoll-gdb.txt in the game root)
#   RAGDOLL_POS   "x y z yaw" for setviewpos before the kill (default a spot
#                 on mp_crash's ground, 207 2291 296, yaw 0)
#   RAGDOLL_STEPS steps to dump past the first ragdoll's start (default 400;
#                 ragdoll_max_life 4500 ms is 265 at 17 ms)
import os
import struct

import gdb

OUT = open(os.environ.get("RAGDOLL_OUT", "ragdoll-gdb.txt"), "w", buffering=1 << 16)
POS = os.environ.get("RAGDOLL_POS", "207 2291 296 0")
MAX_STEPS = int(os.environ.get("RAGDOLL_STEPS", "400"))

CL_INGAME = 0xC5F8F0  # dvar_t*, bool value at +0xc
CBUF = 0x1410BD4  # local client 0: data, maxsize, cursize
POOL, SLOT_SIZE, SLOTS = 0xCC78A00, 0x9D4, 32  # ragdoll slots (Ragdoll_Update 0x5becd1..0x5bed0d)
WORLDS = 0xCC54478  # dxWorld* by world
MSEC_STEP = 0x71E798  # int by world, Phys_RunToTime 0x5a6e0e
RAGDOLL_TIME = 0xD5EC534  # the ragdoll clock Ragdoll_Update adds each frame's msec to

inf = None
st = {"frame": 0, "ingame": None, "steps": 0, "started": False, "seen": set(), "done": False}

# frame after cl_ingame -> console command
SCRIPT = [
    (1, "set fixedtime 17"),
    (60, "set cg_thirdPerson 1"),
    (60, "openScriptMenu team_marinesopfor autoassign"),
    (120, "openScriptMenu changeclass assault_mp,0"),
    (420, "setviewpos " + POS),
    (480, "kill"),
]


def mem(a, n):
    return bytes(inf.read_memory(a, n))


def u32(a):
    return struct.unpack("<I", mem(a, 4))[0]


def i32(a):
    return struct.unpack("<i", mem(a, 4))[0]


def words(a, n):
    return " ".join("%08x" % w for w in struct.unpack("<%dI" % n, mem(a, 4 * n)))


def reg(n):
    return int(gdb.parse_and_eval("$" + n)) & 0xFFFFFFFF


def log(s):
    OUT.write(s + "\n")


def cbuf_add(text):
    data, size, cur = struct.unpack("<3I", mem(CBUF, 12))
    b = text.encode() + b"\n"
    if cur + len(b) >= size:
        log("# cbuf full, dropped: " + text)
        return
    inf.write_memory(data + cur, b + b"\0")
    inf.write_memory(CBUF + 8, struct.pack("<I", cur + len(b)))


def on_frame():
    ingame = mem(u32(CL_INGAME) + 0xC, 1)[0]
    if st["ingame"] is None:
        if not ingame:
            return
        st["ingame"] = 0
    st["frame"] += 1
    for f, cmd in SCRIPT:
        if f == st["frame"]:
            log("# frame %d ragdoll_time %d: %s" % (f, i32(RAGDOLL_TIME), cmd))
            cbuf_add(cmd)


def live_slots():
    for i in range(SLOTS):
        s = POOL + i * SLOT_SIZE
        state = u32(s + 0x24)
        if state >= 3:
            yield i, s, state


def bodies(s):
    n = i32(s + 0x34)
    for b in range(max(0, min(n, 14))):
        body = u32(s + 0x38 + b * 0x1C + 8)
        if body:  # an idle slot (state 5) has let its bodies go
            yield b, body


def body_line(tag, i, b, body):
    # flags +0x18, idle time left +0xd8, pos +0xe0, q +0xf0, lvel +0x130, avel +0x140
    return "%s %d %d %s %s | %s | %s | %s | %s" % (
        tag, i, b, words(body + 0x18, 1), words(body + 0xD8, 1), words(body + 0xE0, 3),
        words(body + 0xF0, 4), words(body + 0x130, 3), words(body + 0x140, 3))


def world_seed():
    return u32(u32(WORLDS + 8) + 0x30)


def on_run_to_time():
    # Phys_RunToTime(lc, world, time) entry: the bodies a slot built this frame
    if st["done"] or u32(reg("esp") + 8) != 2:
        return
    for i, s, state in live_slots():
        for b, body in bodies(s):
            if body not in st["seen"]:
                if not st["started"]:
                    st["started"] = True
                    log("world %08x seed %08x msec_step %d" % (u32(WORLDS + 8), world_seed(), i32(MSEC_STEP + 8)))
                st["seen"].add(body)
                log(body_line("start", i, b, body))


def on_step():
    # after a step's Phys_RunFrame and the msecStep pick (0x5a6e69): ebx the
    # step's msec, ebp the time run to, edi the world's clock block,
    # [esp+0x20] the world
    if st["done"] or not st["started"] or u32(reg("esp") + 0x20) != 2:
        return
    st["steps"] += 1
    edi = reg("edi")
    log("step %d msec %d sim %d to %d seed %08x next %d" % (
        st["steps"], reg("ebx"), i32(edi + 4), reg("ebp"), world_seed(), i32(MSEC_STEP + 8)))
    for i, s, state in live_slots():
        log("slot %d state %d msec %d frames %d" % (i, state, i32(s + 0x28), i32(s + 0x2C)))
        for b, body in bodies(s):
            log(body_line("body", i, b, body))
    states = [state for _, _, state in live_slots()]
    if st["steps"] >= MAX_STEPS or not states or all(state == 5 for state in states):
        st["done"] = True
        log("end")
        OUT.flush()


class B(gdb.Breakpoint):
    def __init__(self, addr, fn):
        super().__init__("*0x%x" % addr, internal=True)
        self.fn = fn

    def stop(self):
        try:
            self.fn()
        except gdb.error as e:
            log("# error at 0x%x: %s" % (reg("pc"), e))
            return True
        return st["done"]


# displaced stepping would decode the 32-bit code as the 64-bit process's
for c in ("set pagination off", "set confirm off", "set displaced-stepping off"):
    gdb.execute(c)
for sig in ("SIGSEGV", "SIGUSR1", "SIGUSR2", "SIGILL", "SIGBUS", "SIGFPE", "SIGPIPE", "SIGALRM", "SIGIO", "SIGCHLD", "SIGWINCH", "SIGQUIT", "SIGSYS"):
    gdb.execute("handle %s nostop noprint pass" % sig)
inf = gdb.selected_inferior()
B(0x4FFCE0, on_frame)
B(0x5A6D50, on_run_to_time)
B(0x5A6E69, on_step)
gdb.execute("continue")
# a stop returned True: the dump is complete. Detached, the client runs on
# showing the body at rest for run.sh's SHOTS=1 grab.
OUT.close()
gdb.execute("detach")
