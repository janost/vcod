# gdb python for the CoD4 1.7 Windows client (iw3mp.exe) under
# tools/cod4_wine/run.sh: drives its own player through one physics scenario
# and dumps one ODE world (0 the map's dynents, 1 launched parts and FX
# models) step by step from its first body: each body as made (mass, geoms,
# userdata, which creator), each step's bodies, the raw dCollide contacts
# and the post-cut set over a step range, the pushes and the impact sounds
# (cod4-gsc.md 3.6.2; the line format is crates/cod4/tests/fixtures/phys/
# README.md's). The ragdoll world's twin is cod4_ragdoll_gdb.py.
#
#   COD4_DIR=<install> PHYS_OUT=$PWD/tmp/phys.txt PHYS_WORLD=0 PHYS_SCENARIO=round \
#       PHYS_POS="1035 816.1 198.7 90" PHYS_PITCH=46.4 GDB_PY=$PWD/tools/re/cod4_phys_gdb.py \
#       tools/cod4_wine/run.sh +set sv_connectTimeout 600 +set scr_game_playerwaittime 1 \
#       +set scr_game_matchstarttime 1 +devmap mp_crash +set fixedtime 17
#
# (docs/dev/retail.md, "Worlds 0 and 1 from the first push to rest", has the
# timeouts the loopback join needs on a loaded machine.)
#
#   PHYS_OUT       the dump (default phys-gdb.txt in the game root)
#   PHYS_WORLD     0 or 1 (default 0)
#   PHYS_SCENARIO  round: one frame of +attack; frag: +frag for 30 frames;
#                  burst: +attack for PHYS_FIRE frames
#   PHYS_POS       "x y z yaw" for setviewpos: the eye's position (the
#                  player then drops to the ground, the eye 60 over it)
#   PHYS_PITCH     degrees added to the view's pitch before the action
#                  (positive looks down)
#   PHYS_FIRE      burst's frames of +attack (default 120)
#   PHYS_AT        the frame of the +attack or +frag (default 700: past the
#                  prematch, with scr_game_playerwaittime and
#                  scr_game_matchstarttime 1 on the command line)
#   PHYS_STEPS     steps to dump past the first body (default 600)
#   PHYS_QUIET     steps with every body asleep or gone that end the dump
#                  (default 30)
#   PHYS_CONTACTS  "first-last": the steps whose contacts are dumped
#                  (default 1-40; the near callback stop is the slow one)
#   PHYS_CONTACTS_FROM  a creator (launch, fxmodel, ...): PHYS_CONTACTS
#                  counts from the step its first body appears in
#   PHYS_CLASS     the class the driver answers (default specops_mp,0:
#                  the MP5, one round a frame)
#   PHYS_PLACE     "ID=W,...;ID=W,..." : before its first step body ID is
#                  put at the 19 hex words (pos 3, q 4 w first, R 12 by
#                  rows), at rest with its accumulators cleared and its
#                  geoms marked moved (a drop the scenario only wakes)
#   PHYS_CONTACT_IDS  "ID ID ...": only these bodies' collide and cut lines
import os
import struct

import gdb

OUT = open(os.environ.get("PHYS_OUT", "phys-gdb.txt"), "w", buffering=1 << 16)
WORLD = int(os.environ.get("PHYS_WORLD", "0"))
SCENARIO = os.environ.get("PHYS_SCENARIO", "round")
POS = os.environ.get("PHYS_POS", "1035 816.1 198.7 90")
FIRE = int(os.environ.get("PHYS_FIRE", "120"))
AT = int(os.environ.get("PHYS_AT", "700"))
PITCH = float(os.environ.get("PHYS_PITCH", "0"))
MAX_STEPS = int(os.environ.get("PHYS_STEPS", "600"))
QUIET = int(os.environ.get("PHYS_QUIET", "30"))
C0, C1 = (int(x) for x in os.environ.get("PHYS_CONTACTS", "1-40").split("-"))
CLASS = os.environ.get("PHYS_CLASS", "specops_mp,0")
FROM = os.environ.get("PHYS_CONTACTS_FROM", "")
PLACE = {int(k): [int(x, 16) for x in v.split(",")] for k, v in
         (e.split("=") for e in os.environ.get("PHYS_PLACE", "").split(";") if e)}
assert all(len(v) == 19 for v in PLACE.values()), "PHYS_PLACE: 19 words a body"
CONTACT_IDS = {int(x) for x in os.environ.get("PHYS_CONTACT_IDS", "").split()}

CL_INGAME = 0xC5F8F0  # dvar_t*, bool value at +0xc
CBUF = 0x1410BD4  # local client 0: data, maxsize, cursize
WORLDS = 0xCC54478  # dxWorld* by world
CLOCKS = 0xCC54484  # per world 0xcc: previous time, simulated time, lerp frac
MSEC_STEP = 0x71E798  # int by world, Phys_RunToTime 0x5a6e0e
VIEWANGLES = 0xC84FD8  # cl.viewangles, pitch first
# return address of each Phys_ObjCreate (0x5a4fb0) call: who made the body
CREATORS = {
    0x434D4E: ("launch", 1),  # CG_CreatePhysicsObject
    0x48DCC8: ("dynent", 0),  # DynEntCl_CreatePhysObj
    0x48ED9F: ("jitter", 0),  # physicsJitter's body
    0x490FDA: ("pieces", 1),  # a broken dynent's pieces
    0x4A21BE: ("fxmodel", 1),  # an FX model element with flag 0x8000000
}

inf = None
st = {"frame": 0, "ingame": None, "steps": 0, "started": False, "done": False, "quiet": 0, "nc_world": -1, "c_base": None}
ids = {}  # body address -> id, in the order first met
made = {}  # body address -> creator
dumped = set()

SCRIPT = [
    (1, "set fixedtime 17"),
    (60, "openScriptMenu team_marinesopfor autoassign"),
    (120, "openScriptMenu changeclass " + CLASS),
    (400, "god"),
]
# setviewpos takes the eye's position and the yaw, pitch 0; the pitch is
# then added to cl.viewangles (0xc84fd8, CL_AdjustAngles 0x463117), so the
# view turns by it.
SCRIPT += [
    (420, "setviewpos " + POS),
    (AT - 20, "pitch"),
    (AT - 10, "viewpos"),
]
if SCENARIO == "frag":
    SCRIPT += [(430, "give frag_grenade_mp"), (AT, "+frag"), (AT + 30, "-frag")]
else:
    shots = 1 if SCENARIO == "round" else FIRE
    SCRIPT += [(AT, "+attack"), (AT + shots, "-attack"), (AT + 10 + shots, "viewpos")]


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


def bid(body):
    if not body:
        return 0
    if body not in ids:
        ids[body] = len(ids) + 1
    return ids[body]


def world_ptr():
    return u32(WORLDS + 4 * WORLD)


def world_bodies():
    # dxWorld +0 firstbody, dxBody +4 next: the walk at 0x5a6d92..0x5a6df2
    b, n = u32(world_ptr()), 0
    while b and n < 600:
        yield b
        b, n = u32(b + 4), n + 1


def on_frame():
    ingame = mem(u32(CL_INGAME) + 0xC, 1)[0]
    if st["ingame"] is None:
        if not ingame:
            return
        st["ingame"] = 0
    st["frame"] += 1
    for f, cmd in SCRIPT:
        if f == st["frame"]:
            log("# frame %d time %d: %s" % (f, i32(CLOCKS + WORLD * 0xCC + 4), cmd))
            if cmd == "pitch":
                p = struct.unpack("<f", mem(VIEWANGLES, 4))[0] + PITCH
                inf.write_memory(VIEWANGLES, struct.pack("<f", p))
                log("# pitch %g" % p)
            else:
                cbuf_add(cmd)
            OUT.flush()


def on_made():
    body = reg("eax")
    who, w = CREATORS[reg("pc")]
    if w != WORLD or not body:
        return
    made[body] = who
    if st["c_base"] is None and (not FROM or who == FROM):
        st["c_base"] = st["steps"]
    log("made %d %08x %s" % (bid(body), body, who))


def geom_lines(g, tag):
    # dxGeom: +4 class, +0x10 body, +0x14 the body's next geom, +0x18 pos*,
    # +0x1c R*, +0x2c aabb, +0x4c the class's own fields; 0xc0 bytes raw
    out = ["%s %08x | %s" % (tag, g, words(g, 48))]
    cls = u32(g + 4)
    if cls == 6:  # a transform: its inner geom and what that geom's pos and R point at
        inner = u32(g + 0x4C)
        if inner:
            out += geom_lines(inner, "inner")
            out.append("inner_pose %s | %s" % (words(u32(inner + 0x18), 3), words(u32(inner + 0x1C), 12)))
    elif cls == 12 and u32(g + 0x4C):  # a brush: its BrushWrapper, 0x50 bytes
        out.append("brush %s" % words(u32(g + 0x4C), 20))
    return out


def body_full(body):
    # flags +0x18, dMass +0x20 (17), invI.. +0x64 (13 to +0x98: invI, invMass),
    # force +0x98, torque +0xa8, idle time/steps +0xd0..+0xdc, pos +0xe0,
    # q +0xf0, R +0x100, lvel +0x130, avel +0x140; userdata (+0xc) 0x50 bytes
    w = u32(body)
    lines = [
        "new %d %08x %s world_ok %d flags %s | mass %s | inv %s | force %s | torque %s | idle %s | pos %s | q %s | R %s | lvel %s | avel %s"
        % (bid(body), body, made.get(body, "?"), int(w == world_ptr()), words(body + 0x18, 1), words(body + 0x20, 17),
           words(body + 0x64, 13), words(body + 0x98, 3), words(body + 0xA8, 3), words(body + 0xD0, 4),
           words(body + 0xE0, 3), words(body + 0xF0, 4), words(body + 0x100, 12), words(body + 0x130, 3),
           words(body + 0x140, 3)),
        "user %d %s" % (bid(body), words(u32(body + 0xC), 20)),
    ]
    g, n = u32(body + 0x1C), 0
    while g and n < 32:
        gl = geom_lines(g, "geom %d" % bid(body))
        if u32(g + 0x10) != body:
            gl[0] += " # geom body %08x" % u32(g + 0x10)
        lines += gl
        g, n = u32(g + 0x14), n + 1
    return lines


def body_line(body):
    # as ragdoll's: flags, idle time left +0xd8, pos, q, lvel, avel
    return "body %d %s %s | %s | %s | %s | %s" % (
        bid(body), words(body + 0x18, 1), words(body + 0xD8, 1), words(body + 0xE0, 3),
        words(body + 0xF0, 4), words(body + 0x130, 3), words(body + 0x140, 3))


def place(body):
    # pos +0xe0, q +0xf0, R +0x100; lvel, avel, force and torque zeroed;
    # each geom's gflags (+8) GEOM_DIRTY | GEOM_AABB_BAD, as dGeomMoved
    v = PLACE.get(bid(body))
    if v is None:
        return
    inf.write_memory(body + 0xE0, struct.pack("<3I", *v[:3]))
    inf.write_memory(body + 0xF0, struct.pack("<4I", *v[3:7]))
    inf.write_memory(body + 0x100, struct.pack("<12I", *v[7:]))
    for off in (0x98, 0xA8, 0x130, 0x140):
        inf.write_memory(body + off, b"\0" * 12)
    g, n = u32(body + 0x1C), 0
    while g and n < 32:
        inf.write_memory(g + 8, struct.pack("<I", u32(g + 8) | 3))
        g, n = u32(g + 0x14), n + 1
    log("# place %d %s" % (bid(body), " ".join("%08x" % x for x in v)))


def on_run_to_time():
    # Phys_RunToTime(lc, world, time) entry: the bodies made since the last
    if st["done"] or u32(reg("esp") + 8) != WORLD:
        return
    bodies = list(world_bodies())
    new = [b for b in bodies if b not in dumped]
    if new and not st["started"]:
        st["started"] = True
        wp = world_ptr()
        log("world %d %08x seed %s msec_step %d time %d nb %d nj %d gravity %s" % (
            WORLD, wp, words(wp + 0x30, 1), i32(MSEC_STEP + 4 * WORLD), u32(reg("esp") + 0xC),
            i32(wp + 8), i32(wp + 0xC), words(wp + 0x10, 3)))
    if new:
        log("frame %d time %d bodies %d nb %d" % (st["frame"], u32(reg("esp") + 0xC), len(bodies), i32(world_ptr() + 8)))
        st["quiet"] = 0
    for b in new:
        dumped.add(b)
        place(b)
        for line in body_full(b):
            log(line)


def on_step():
    # after a step (0x5a6e69): ebx its msec, ebp the time run to, edi the
    # world's clock block, [esp+0x20] the world
    if st["done"] or not st["started"] or u32(reg("esp") + 0x20) != WORLD:
        return
    st["steps"] += 1
    edi = reg("edi")
    bodies = list(world_bodies())
    log("step %d msec %d sim %d to %d seed %s next %d awake %d of %d" % (
        st["steps"], reg("ebx"), i32(edi + 4), reg("ebp"), words(world_ptr() + 0x30, 1),
        i32(MSEC_STEP + 4 * WORLD), sum(1 for b in bodies if not u32(b + 0x18) & 4), len(bodies)))
    for b in bodies:
        log(body_line(b))
    OUT.flush()
    if all(u32(b + 0x18) & 4 for b in bodies):
        st["quiet"] += 1
    else:
        st["quiet"] = 0
    if st["steps"] >= MAX_STEPS or st["quiet"] >= QUIET:
        st["done"] = True
        log("end")
        OUT.flush()


def contact_lines(buf, n):
    # dContactGeom, 0x30: pos +0, normal +0x10, depth +0x20, g1 +0x24,
    # g2 +0x28, surface flags +0x2c
    return ["c %s" % words(buf + 0x30 * k, 12) for k in range(n)]


def in_range():
    base = st["c_base"]
    return st["started"] and not st["done"] and base is not None and C0 <= st["steps"] + 1 - base <= C1


def on_collide():
    # the near callback 0x5a5f20 after its dCollide (0x5a5f9e): eax the
    # count, the contacts at esp+0x4c, edi and ebx the geoms' bodies, the
    # callback's data at [esp+0x3060] (+4 the world), its geoms at +0x3064
    esp = reg("esp")
    w = u32(u32(esp + 0x3060) + 4)
    st["nc_world"] = w
    if w != WORLD or not in_range():
        return
    n = reg("eax")
    if CONTACT_IDS and bid(reg("edi")) not in CONTACT_IDS:
        return
    g1, g2 = u32(esp + 0x3064), u32(esp + 0x3068)
    log("collide %d %d %d %d %d %d" % (st["steps"] + 1, bid(reg("edi")), bid(reg("ebx")), u32(g1 + 4), u32(g2 + 4), n))
    for line in contact_lines(esp + 0x4C, min(n, 0x80)):
        log(line)


def on_cut():
    # the contact pass 0x5a2760 entry: (contacts, body1, body2, surface,
    # world), the count at contacts +0x1800; the surface 0x2c bytes
    esp = reg("esp")
    if u32(esp + 0x14) != WORLD or not in_range():
        return
    buf = u32(esp + 4)
    if CONTACT_IDS and bid(u32(esp + 8)) not in CONTACT_IDS:
        return
    n = i32(buf + 0x1800)
    log("cut %d %d %d %d | %s" % (st["steps"] + 1, bid(u32(esp + 8)), bid(u32(esp + 0xC)), n, words(u32(esp + 0x10), 11)))
    for line in contact_lines(buf, min(max(n, 0), 0x80)):
        log(line)


def on_push():
    # Phys_ObjAddForce 0x5a5900: esi the world, [esp+4] the body, ecx the
    # force, edx the point, the caller at [esp]
    esp = reg("esp")
    if reg("esi") != WORLD or st["done"]:
        return
    body = u32(esp + 4)
    log("push %d %d %08x %s | %s | %s" % (st["steps"], bid(body), u32(esp), words(reg("edx"), 3), words(reg("ecx"), 3),
                                          words(MSEC_STEP + 4 * WORLD, 1)))


def on_sound():
    # 0x5a5f0e, the impact sound's call in 0x5a5de0: [esp] the handle, esi
    # the contacts' mean point; the world is the near callback's
    if st["nc_world"] != WORLD or st["done"] or not st["started"]:
        return
    log("sound %d %08x %s" % (st["steps"] + 1, u32(reg("esp")), words(reg("esi"), 3)))


def on_round():
    # DynEntCl_EntityImpactEvent 0x48e0f0: a round's leg walked the dynents
    log("# frame %d round leg" % st["frame"])
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
log("# cod4_phys_gdb world %d scenario %s pos %s fire %d contacts %d-%d class %s" % (WORLD, SCENARIO, POS, FIRE, C0, C1, CLASS))
if PLACE or CONTACT_IDS:
    log("# place %s contact ids %s" % (" ".join(str(k) for k in sorted(PLACE)), " ".join(str(k) for k in sorted(CONTACT_IDS))))
OUT.flush()
B(0x4FFCE0, on_frame)
for a in CREATORS:
    B(a, on_made)
B(0x48E0F0, on_round)
B(0x5A6D50, on_run_to_time)
B(0x5A6E69, on_step)
B(0x5A5F9E, on_collide)
B(0x5A2760, on_cut)
B(0x5A5900, on_push)
B(0x5A5F0E, on_sound)
gdb.execute("continue")
# a stop returned True: the dump is complete. Detached, the client runs on
# for run.sh's SHOTS=1 grab.
OUT.close()
gdb.execute("detach")
