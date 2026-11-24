#!/usr/bin/env python3
"""Reads a retail-client capture (docs/dev/retail.md, "Retail client
captures"): vcod-server's --client-log, the probe's games_mp.log and the
client's screenshots. Each subcommand prints what the step measured and
exits 1 when the capture cannot answer its question.

  analyze.py kick CLIENT_LOG [--standin]
  analyze.py intermission CLIENT_LOG GAME_LOG SHOTS_DIR [--standin]
  analyze.py hint CLIENT_LOG GAME_LOG SHOTS_DIR [--standin]
  analyze.py fdc CLIENT_LOG GAME_LOG [--standin]
  analyze.py fdc-fixture CLIENT_LOG   (the fdc gate's fixture, on stdout)
  analyze.py shot TGA X Y W H       (mean colour of a 640x480 virtual rect)

--standin is the dry run's: vcod's probe client stands in for the retail
one, so what only the retail client makes (a view kick, screenshots) is
reported but not required.
"""

import os
import re
import struct
import sys

# --- the client log ------------------------------------------------------


def fields(line):
    """A client-log line as (kind, {key: value}); `text=` and `name=` run
    to the end."""
    kind, _, rest = line.rstrip("\n").partition(" ")
    out = {}
    while rest:
        if rest.startswith(("text=", "name=")):
            out[rest[:4]] = rest[5:]
            break
        token, _, rest = rest.partition(" ")
        key, _, value = token.partition("=")
        out[key] = value
    return kind, out


def read_log(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        return [fields(line) for line in f if line.strip() and not line.startswith("#")]


def cmd_stream(log, slot):
    """The slot's usercmds once each, in the order the client made them.

    A packet repeats the last one's cmds (cl_packetdup), so a cmd whose
    serverTime is not past the newest one before its packet is a copy;
    retail's server runs only those past it too.
    """
    out, newest, before = [], None, None
    for kind, f in log:
        if kind == "move" and f["slot"] == str(slot):
            before = newest
        elif kind == "cmd" and f["slot"] == str(slot):
            st = int(f["st"])
            if before is not None and st <= before:
                continue
            a = [int(x) for x in f["a"].split(",")]
            out.append({"st": st, "b": int(f["b"]), "wb": int(f["wb"]), "a": a})
            newest = st if newest is None else max(newest, st)
    return out


def human_slots(log):
    return sorted({int(f["slot"]) for k, f in log if k == "enter"})


def short_delta(a, b):
    """b - a for two ANGLE2SHORT values, wrapped to -32768..32767."""
    return (b - a + 32768) % 65536 - 32768


SHORT_DEG = 360.0 / 65536.0
FIRE_EVENTS = {159, 160, 161, 168, 169, 170}


def kick_shots(log, slot):
    """Each shot the server ran from the slot's cmds: the index of its cmd
    in the stream, and every later cmd's view change from the one before
    the shot."""
    cmds = cmd_stream(log, slot)
    index = {c["st"]: i for i, c in enumerate(cmds)}
    fired = []
    for kind, f in log:
        if kind != "ran" or f["slot"] != str(slot):
            continue
        events = [int(e.split(":")[0]) for e in f["ev"].split(",")]
        st = int(f["st"])
        if FIRE_EVENTS & set(events) and st in index:
            fired.append(index[st])
    shots = []
    for i in fired:
        if i < 1:
            continue
        base = cmds[i - 1]["a"]
        still = all(cmds[j]["a"] == base for j in range(max(0, i - 6), i))
        trail = []
        for j in range(i, min(len(cmds), i + 40)):
            d = [short_delta(base[k], cmds[j]["a"][k]) for k in range(2)]
            trail.append({"st": cmds[j]["st"], "b": cmds[j]["b"], "d": d})
        first = next((n for n, t in enumerate(trail) if t["d"] != [0, 0]), None)
        shots.append({"i": i, "st": cmds[i]["st"], "still": still, "kick": first, "trail": trail})
    return cmds, shots


def report_kick(log_path, standin):
    log = read_log(log_path)
    slots = human_slots(log)
    if not slots:
        return fail("no client entered the world")
    slot = slots[0]
    cmds, shots = kick_shots(log, slot)
    if len(cmds) < 2:
        return fail("no usercmds logged")
    steps = sorted(b["st"] - a["st"] for a, b in zip(cmds, cmds[1:]))
    print(f"slot {slot}: {len(cmds)} cmds, frame median {steps[len(steps) // 2]} ms, "
          f"{len(shots)} shots the server fired")
    print("shot st      still  kick-at  pitch/yaw change per cmd from the shot (shorts; 1 short = 0.0055 deg)")
    for s in shots:
        trail = " ".join(f"{t['d'][0]}/{t['d'][1]}" for t in s["trail"][:12])
        print(f"{s['st']:>10} {'yes' if s['still'] else 'NO':>5}  {str(s['kick']):>7}  {trail}")
    clean = [s for s in shots if s["still"]]
    kicked = [s for s in clean if s["kick"] is not None]
    offsets = sorted({s["kick"] for s in kicked})
    print(f"clean shots {len(clean)}, kicked {len(kicked)}, first-change offsets {offsets} "
          f"(0 = the shot's own cmd, 1 = the next frame's)")
    if len(shots) < 8:
        return fail(f"{len(shots)} shots; take at least 8")
    if len(clean) < 6:
        return fail("the view moved before most shots: keep the mouse still")
    if not standin and len(kicked) < len(clean) // 2:
        return fail("no view kick in the cmds: was it the carbine, fired from the hip?")
    return 0


# --- the game log ---------------------------------------------------------


PROBE = re.compile(r"PROBE (\S+) (-?\d+) ?(.*)")


def probe_lines(path, what):
    """`PROBE <what> <time> <rest>` lines of a games_mp.log as (time, rest)."""
    out = []
    if not os.path.exists(path):
        return out
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            m = PROBE.search(line)
            if m and m.group(1) == what:
                out.append((int(m.group(2)), m.group(3).strip()))
    return out


def phase_spans(game_log):
    """(name, start, end) for each `PROBE phase` span; the last runs on."""
    ph = probe_lines(game_log, "phase")
    spans = []
    for n, (t, rest) in enumerate(ph):
        end = ph[n + 1][0] if n + 1 < len(ph) else 1 << 40
        spans.append((rest.split()[0], t, end))
    return spans


def snaps(log, slot):
    return [f for k, f in log if k == "snap" and f["slot"] == str(slot)]


# --- screenshots ----------------------------------------------------------


def read_tga(path):
    """(width, height, rows) of an uncompressed or RLE truecolor TGA, rows
    top-down, each a bytes of RGB triples."""
    with open(path, "rb") as f:
        data = f.read()
    (idlen, cmaptype, itype, _cmap_first, cmap_len, cmap_bits, _x, _y, w, h, bpp,
     desc) = struct.unpack("<BBBHHBHHHHBB", data[:18])
    if cmaptype != 0 or itype not in (2, 10) or bpp not in (24, 32):
        raise ValueError(f"{path}: TGA type {itype}/{bpp} bpp not read")
    px = bpp // 8
    pos = 18 + idlen + cmap_len * ((cmap_bits + 7) // 8)
    need = w * h * px
    if itype == 2:
        raw = data[pos:pos + need]
    else:
        out = bytearray()
        while len(out) < need:
            head = data[pos]
            pos += 1
            n = (head & 0x7F) + 1
            if head & 0x80:
                out += data[pos:pos + px] * n
                pos += px
            else:
                out += data[pos:pos + n * px]
                pos += n * px
        raw = bytes(out[:need])
    if len(raw) < need:
        raise ValueError(f"{path}: truncated")
    rows = []
    for y in range(h):
        row = raw[y * w * px:(y + 1) * w * px]
        rgb = bytearray(w * 3)
        rgb[0::3] = row[2::px]
        rgb[1::3] = row[1::px]
        rgb[2::3] = row[0::px]
        rows.append(bytes(rgb))
    if not desc & 0x20:
        rows.reverse()
    return w, h, rows


def to_pixels(w, h, x, y):
    """A 640x480 virtual point on a w by h screenshot (the cgame stretches)."""
    return int(x * w / 640.0), int(y * h / 480.0)


def mean_rgb(shot, x, y, rw, rh):
    w, h, rows = shot
    x0, y0 = to_pixels(w, h, x, y)
    x1, y1 = to_pixels(w, h, x + rw, y + rh)
    tot, n = [0, 0, 0], 0
    for py in range(y0, max(y1, y0 + 1)):
        row = rows[py]
        for pxl in range(x0, max(x1, x0 + 1)):
            for c in range(3):
                tot[c] += row[pxl * 3 + c]
            n += 1
    return [t / n / 255.0 for t in tot]


def is_colour(rgb, want):
    return all((v > 0.7) if c else (v < 0.3) for v, c in zip(rgb, want))


def shots_in(directory, prefix):
    if not os.path.isdir(directory):
        return []
    return sorted(os.path.join(directory, n) for n in os.listdir(directory)
                  if n.startswith(prefix) and n.lower().endswith(".tga"))


# The two boxes probe_rc_intermission.gsc draws (virtual x, y, size, colour);
# sampled on their middle half.
BOXES = [("level", 16, 216, 48, (1, 0, 1)), ("client", 576, 216, 48, (0, 1, 0))]


def boxes_seen(path):
    shot = read_tga(path)
    seen = {}
    for name, x, y, size, colour in BOXES:
        rgb = mean_rgb(shot, x + size / 4, y + size / 4, size / 2, size / 2)
        seen[name] = is_colour(rgb, colour)
    return seen


def report_intermission(log_path, game_log, shots_dir, standin):
    log = read_log(log_path)
    slots = human_slots(log)
    if not slots:
        return fail("no client entered the world")
    slot = slots[0]
    spans = phase_spans(game_log)
    bad = False
    for name, t0, t1 in spans:
        ss = [s for s in snaps(log, slot) if t0 <= int(s["time"]) < t1]
        pm = sorted({s["pm_type"] for s in ss})
        hud = sorted({s["hud"] for s in ss})
        print(f"phase {name}: {len(ss)} snapshots, pm_type {pm}, hudelems archived,current {hud}")
        want = "5" if name == "intermission" else "0"
        if not ss or want not in pm:
            print(f"  expected pm_type {want} in this phase")
            bad = True
    if bad:
        return fail("the server side of a phase did not happen")
    verdict = {}
    for phase in ("play", "intermission"):
        files = shots_in(shots_dir, f"rc_{phase}_")
        for path in files:
            seen = boxes_seen(path)
            print(f"{os.path.basename(path)}: " + ", ".join(
                f"{k} box {'drawn' if v else 'not drawn'}" for k, v in seen.items()))
            verdict.setdefault(phase, []).append(seen)
        if not files:
            print(f"no rc_{phase}_*.tga screenshot in {shots_dir}")
    if not verdict.get("play") or not verdict.get("intermission"):
        return 0 if standin else fail("screenshots missing for a phase")
    if not all(all(s.values()) for s in verdict["play"]):
        return fail("a play screenshot lacks a box: the sampling is off, check the shot by eye")
    drawn = {k: any(s[k] for s in verdict["intermission"]) for k, _, _, _, _ in BOXES}
    print("retail at intermission: " + ", ".join(
        f"{k} hudelem {'DRAWN' if v else 'NOT drawn'}" for k, v in drawn.items()))
    return 0


# The hint icon's rect on the 640x480 virtual screen (hud::player
# cursor_hint): 40 by 40 at (300, 325); a wide one is 80 from 280. Its left
# edge is read off the band below the hint text, left of where the gun
# might reach.
HINT_BAND = (330, 360)
HINT_SCAN = (250, 330)
DIFF_MIN = 40


def icon_left(shot, base):
    """The leftmost virtual x in the scan where `shot` differs from `base`
    in the band, or None."""
    w, h, rows = shot
    bw, bh, brows = base
    if (w, h) != (bw, bh):
        raise ValueError("screenshots of different sizes")
    _, y0 = to_pixels(w, h, 0, HINT_BAND[0])
    _, y1 = to_pixels(w, h, 0, HINT_BAND[1])
    x0, _ = to_pixels(w, h, HINT_SCAN[0], 0)
    x1, _ = to_pixels(w, h, HINT_SCAN[1], 0)
    for px in range(x0, x1):
        for py in range(y0, y1):
            a, b = rows[py], brows[py]
            if max(abs(a[px * 3 + c] - b[px * 3 + c]) for c in range(3)) > DIFF_MIN:
                return px * 640.0 / w
    return None


def report_hint(log_path, game_log, shots_dir, standin):
    log = read_log(log_path)
    slots = human_slots(log)
    if not slots:
        return fail("no client entered the world")
    slot = slots[0]
    bad = False
    for name, t0, t1 in phase_spans(game_log):
        ss = [s for s in snaps(log, slot) if t0 <= int(s["time"]) < t1]
        hints = sorted({s["hint"] for s in ss})
        print(f"phase {name}: {len(ss)} snapshots, serverCursorHint,Val,String {hints}")
        lit = [h for h in hints if not h.startswith("0,")]
        if name != "none" and not lit:
            print("  expected a hint in this phase")
            bad = True
    if bad:
        return fail("the server sent no hint in a phase")
    bases = shots_in(shots_dir, "rc_none_")
    if not bases:
        print(f"no rc_none_*.tga screenshot in {shots_dir}")
        return 0 if standin else fail("screenshots missing")
    base = read_tga(bases[0])
    missing = False
    for phase in ("unowned_wide", "owned_wide", "unowned_narrow", "owned_narrow"):
        files = shots_in(shots_dir, f"rc_{phase}_")
        lefts = [icon_left(read_tga(p), base) for p in files]
        known = [x for x in lefts if x is not None]
        if not known:
            print(f"{phase}: no icon found ({len(files)} shots)")
            missing = True
            continue
        left = min(known)
        print(f"{phase}: icon left edge x {left:.0f} -> {'WIDE' if left < 292 else 'narrow'} "
              f"({len(files)} shots, edges {['-' if x is None else round(x) for x in lefts]})")
    if missing and not standin:
        return fail("a phase's icon was not found; check its shots by eye")
    return 0


def report_fdc(log_path, game_log, standin):
    log = read_log(log_path)
    slots = human_slots(log)
    if not slots:
        return fail("no client entered the world")
    victim = slots[0]
    fdc = [f for k, f in log if k == "ccmd" and f["slot"] == str(victim)
           and f.get("text", "").split(" ")[0] == "fdc"]
    times = [int(f["svs"]) for f in fdc]
    # Arrival on the wall clock; svs steps in whole server frames.
    wall = [float(f["w"]) for f in fdc]
    gaps = [round(b - a) for a, b in zip(wall, wall[1:])]
    dropped = sum(1 for f in fdc if f["ok"] == "0")
    burns = probe_lines(game_log, "damage")
    pulses = probe_lines(game_log, "pulse")
    print(f"victim slot {victim}: {len(fdc)} fdc ({dropped} inside the flood window), "
          f"{len(pulses)} shooter pulses, {len(burns)} damage callbacks")
    print("fdc svs: " + " ".join(str(t) for t in times))
    print("arrival gaps ms: " + " ".join(str(g) for g in gaps))
    if not pulses:
        return fail("the shooter never fired: did the vcod shooter join?")
    if len(fdc) < 5:
        return fail(f"{len(fdc)} fdc reports; the victim must stand in the stream")
    return 0


def fdc_frames(log):
    """The victim's frames from its first raised flag to its last fdc, as
    (serverTime, wbuttons 0x1, carried an fdc). A cmd's serverTime is the
    cg.time of the draw before it, the one whose report set the flag, and
    an fdc that draw sends rides the same packet."""
    victim = human_slots(log)[0]
    # The newest cmd of the packet after each fdc command.
    sent, pending, carrier, newest = set(), False, False, None
    for kind, f in log + [("move", {"slot": str(victim)})]:
        if f.get("slot") != str(victim):
            continue
        if kind == "ccmd" and f.get("text", "").split(" ")[0] == "fdc":
            pending = True
        elif kind == "move":
            if carrier and newest is not None:
                sent.add(newest)
            carrier, pending, newest = pending, False, None
        elif kind == "cmd":
            newest = int(f["st"])
    frames = [(c["st"], c["wb"] & 1) for c in cmd_stream(log, victim)]
    first = next(i for i, (_, wb) in enumerate(frames) if wb)
    last = max(i for i, (st, _) in enumerate(frames) if st in sent)
    return [(st, wb, st in sent) for st, wb in frames[first:last + 1]]


def report_fdc_fixture(log_path):
    frames = fdc_frames(read_log(log_path))
    steps = [b[0] - a[0] for a, b in zip(frames, frames[1:])]
    if any(not 0 < s < 10 for s in steps):
        return fail(f"a frame step outside 1..9 ms: {sorted(set(steps))}")
    print(f"start {frames[0][0]}")
    for i in range(0, len(steps), 100):
        print("steps " + "".join(str(s) for s in steps[i:i + 100]))
    for i in range(0, len(frames), 100):
        print("flags " + "".join(str(wb) for _, wb, _ in frames[i:i + 100]))
    print("fdc " + " ".join(str(st) for st, _, fdc in frames if fdc))
    return 0


# --- main -----------------------------------------------------------------


def fail(msg):
    print(f"CAPTURE NOT USABLE: {msg}")
    return 1


def report_shot(path, x, y, w, h):
    rgb = mean_rgb(read_tga(path), float(x), float(y), float(w), float(h))
    print(" ".join(f"{v:.3f}" for v in rgb))
    return 0


def main(argv):
    standin = "--standin" in argv
    args = [a for a in argv if a != "--standin"]
    if not args:
        print(__doc__)
        return 2
    cmd, rest = args[0], args[1:]
    if cmd == "kick" and len(rest) == 1:
        return report_kick(rest[0], standin)
    if cmd == "intermission" and len(rest) == 3:
        return report_intermission(*rest, standin)
    if cmd == "hint" and len(rest) == 3:
        return report_hint(*rest, standin)
    if cmd == "fdc" and len(rest) == 2:
        return report_fdc(*rest, standin)
    if cmd == "fdc-fixture" and len(rest) == 1:
        return report_fdc_fixture(rest[0])
    if cmd == "shot" and len(rest) == 5:
        return report_shot(*rest)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
