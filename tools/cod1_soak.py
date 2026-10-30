#!/usr/bin/env python3
"""cod1_soak.py <outdir>: the report of a `tools/cod1_soak.sh` run
(docs/research/cod11-client-soak.md). Per (map, gametype) it joins the
client's soak lines (`play::cod1_soak`, frame-stamped) with its frame
profile (`VCOD_FRAME_PROFILE`) and the two logs, and writes
<outdir>/soak.md: the table, then the stalls by part and action, the
client's and the server's warnings, script errors and panics ranked by
count.

A frame stalls when its CPU side passes 100 ms: the profile's stages from
`net` to `ui` plus the time outside every span (`other`); the render
(`cull` to `submit`, lavapipe) is counted apart. A server tick stalls when
its `--trace` second's slowest tick passes 100 ms, the seconds of a map
load excepted."""
import collections
import datetime
import os
import re
import sys

STALL_MS = 100.0
CPU = ["net", "scene", "bodies", "pose", "vm", "events", "fx", "fxstep", "sound",
       "fxdraw", "hud", "ui"]
RENDER = ["cull", "gather", "build", "acquire", "encode", "finish", "submit"]
PROBLEM = ["unknown", "missing", "not found", "no such", "fail", "error", "invalid",
           "cannot", "not in the", "no alias"]
STAMP = re.compile(r"^\[(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z (\w+)\s+([^\]]*)\] (.*)$")


def shape(line):
    return re.sub(r"\d+", "#", line)


def stamp(ts):
    return datetime.datetime.fromisoformat(ts).replace(
        tzinfo=datetime.timezone.utc).timestamp()


def read_frames(path):
    """frame -> (wall ms, {stage: ms})"""
    names, frames = [], {}
    try:
        f = open(path)
    except OSError:
        return frames
    with f:
        for line in f:
            if line.startswith("# stages"):
                names = line.split()[2:]
            elif line.startswith("f "):
                p = line.split()
                st = dict(zip(names, map(float, p[3:3 + len(names)])))
                frames[int(p[1])] = (float(p[2]), st)
    return frames


def read_log(path):
    """[(unix secs, level, target, text)], a panic as level PANIC."""
    out = []
    last = 0.0
    try:
        f = open(path, errors="replace")
    except OSError:
        return out
    with f:
        for line in f:
            line = line.rstrip("\n")
            m = STAMP.match(line)
            if m:
                last = stamp(m.group(1))
                out.append((last, m.group(2), m.group(3), m.group(4)))
            elif "panicked at" in line:
                out.append((last, "PANIC", "", line))
    return out


class Pair:
    def __init__(self):
        self.secs = 0.0
        self.acts = collections.Counter()
        self.frames = 0
        self.stalls = collections.Counter()
        self.worst = 0.0
        self.render_stalls = 0
        self.first = None
        self.rss = []
        self.audio_miss = 0
        self.client_warn = collections.Counter()
        self.server_warn = collections.Counter()
        self.script = collections.Counter()
        self.panics = []
        self.server_stalls = 0
        self.server_worst = 0.0


def soak_map(d):
    """{gametype: Pair} for one map's directory."""
    pairs = collections.OrderedDict()
    frames = read_frames(os.path.join(d, "frames.txt"))
    # The soak's lines: levels, with their first frame and wall clock.
    levels = []  # (frame, gametype, time, wall)
    acts = []  # (frame, word, rest)
    try:
        lines = open(os.path.join(d, "soak.txt")).read().splitlines()
    except OSError:
        lines = []
    for line in lines:
        p = line.split()
        if len(p) < 2:
            continue
        fr, word = int(p[0]), p[1]
        if word == "level":
            levels.append((fr, p[3], float(p[4]), float(p[8])))
        else:
            acts.append((fr, word, p[2:]))

    def level_at_frame(fr):
        cur = None
        for i, lv in enumerate(levels):
            if lv[0] <= fr:
                cur = i
        return cur

    def level_at_wall(t):
        cur = None
        for i, lv in enumerate(levels):
            if lv[3] <= t:
                cur = i
        return cur

    # A level seen twice (the rotation's last) is a second pair of its own.
    keys = []
    seen = collections.Counter()
    for lv in levels:
        seen[lv[1]] += 1
        keys.append(lv[1] if seen[lv[1]] == 1 else f"{lv[1]}#{seen[lv[1]]}")
    for k in keys:
        pairs[k] = Pair()
    for i, lv in enumerate(levels):
        end = levels[i + 1][2] if i + 1 < len(levels) else None
        if end is not None:
            pairs[keys[i]].secs = end - lv[2]
    acted = collections.defaultdict(list)
    for fr, word, rest in acts:
        i = level_at_frame(fr)
        if i is None:
            continue
        p = pairs[keys[i]]
        if word == "rss":
            p.rss.append(int(rest[1]))
        elif word == "audio_miss":
            p.audio_miss += int(rest[0])
        elif word == "first":
            p.first = (fr, float(rest[0]))
        else:
            p.acts[word] += 1
            acted[fr].append(word)
    for i, lv in enumerate(levels):
        pairs[keys[i]].rss.insert(0, None)
    # Level starts' rss rides the level line itself.
    for line in lines:
        p = line.split()
        if len(p) > 6 and p[1] == "level":
            i = level_at_frame(int(p[0]))
            pairs[keys[i]].rss[0] = int(p[6])
    # Frames: wall clock since the level began, by summing walls.
    since = {}
    t = 0.0
    starts = {lv[0]: i for i, lv in enumerate(levels)}
    level_t = 0.0
    for fr in sorted(frames):
        wall, st = frames[fr]
        if fr in starts:
            level_t = t
        since[fr] = (t - level_t) / 1000.0
        t += wall
    for fr in sorted(frames):
        i = level_at_frame(fr)
        if i is None:
            continue
        p = pairs[keys[i]]
        wall, st = frames[fr]
        parts = {k: st.get(k, 0.0) for k in CPU}
        parts["other"] = max(0.0, wall - sum(st.values()))
        cpu = sum(parts.values())
        render = sum(st.get(k, 0.0) for k in RENDER)
        p.frames += 1
        if render > STALL_MS:
            p.render_stalls += 1
        if p.first and fr in range(p.first[0], p.first[0] + 10):
            if not hasattr(p, "first_ms") or cpu > p.first_ms[0]:
                p.first_ms = (cpu, max(parts, key=parts.get))
        if cpu > STALL_MS:
            part = max(parts, key=parts.get)
            did = list(acted.get(fr, []))
            if since.get(fr, 99) < 5.0:
                did.append("level start")
            key = part + (f" ({', '.join(did)})" if did else "")
            p.stalls[key] += 1
            p.worst = max(p.worst, cpu)
    for side, log in (("client", read_log(os.path.join(d, "client.log"))),
                      ("server", read_log(os.path.join(d, "server.log")))):
        loads = [w for (w, lvl, tgt, text) in log
                 if side == "server" and ("loading" in text.lower() or "map_rotate" in text
                                          or "map " in text.lower() and "load" in text.lower()
                                          # a rotation's load: the gamestates it resends
                                          or ": gamestate, " in text)]
        for (w, lvl, tgt, text) in log:
            i = level_at_wall(w)
            if i is None:
                i = 0 if levels else None
            if i is None:
                continue
            p = pairs[keys[i]]
            if lvl == "PANIC":
                p.panics.append(f"{side}: {text.strip()}")
                continue
            if side == "server" and text.startswith("tick: "):
                m = re.search(r"max ([\d.]+) ms", text)
                if m and float(m.group(1)) > STALL_MS:
                    if any(abs(w - l) < 3.0 for l in loads):
                        continue
                    p.server_stalls += 1
                    p.server_worst = max(p.server_worst, float(m.group(1)))
                continue
            if "gsc: thread aborted" in text or "gsc: script error" in text:
                p.script[shape(text)] += 1
                continue
            # A server command's echo carries player names ("Unknown Soldier").
            if lvl == "DEBUG" and text.startswith("serverCommand "):
                continue
            if lvl in ("WARN", "ERROR") or (lvl == "DEBUG" and any(
                    k in text.lower() for k in PROBLEM)):
                (p.client_warn if side == "client" else p.server_warn)[
                    shape(f"{lvl} {tgt}: {text}")] += 1
    return pairs


def main():
    out = sys.argv[1]
    results = collections.OrderedDict()
    for m in sorted(os.listdir(out)):
        d = os.path.join(out, m)
        if os.path.isdir(d) and os.path.exists(os.path.join(d, "gametypes.txt")):
            results[m] = soak_map(d)
    o = []
    o.append("| map | gametype | secs | spawns | deaths | board | esc | frames | stalls | worst ms "
             "| first frame ms (part) | renders > 100 ms | audio miss | client warn | server warn "
             "| script errors | server stalls (worst) | RSS MB first -> last | panic |")
    o.append("|" + "---|" * 19)
    for m, pairs in results.items():
        for g, p in pairs.items():
            rss = [r for r in p.rss if r is not None]
            fm = getattr(p, "first_ms", None)
            o.append(
                f"| {m} | {g} | {p.secs:.0f} | {p.acts['spawn']} | {p.acts['death']} "
                f"| {p.acts['board']} | {p.acts['esc']} | {p.frames} "
                f"| {sum(p.stalls.values())} | {p.worst:.0f} "
                f"| {f'{fm[0]:.0f} ({fm[1]})' if fm else '-'} | {p.render_stalls} "
                f"| {p.audio_miss} | {sum(p.client_warn.values())} "
                f"| {sum(p.server_warn.values())} | {sum(p.script.values())} "
                f"| {p.server_stalls} ({p.server_worst:.0f}) "
                f"| {f'{rss[0]} -> {rss[-1]}' if rss else '-'} "
                f"| {'; '.join(p.panics).replace('|', '/')[:200]} |")

    def rank(title, f):
        allc = collections.defaultdict(lambda: [0, []])
        for m, pairs in results.items():
            for g, p in pairs.items():
                for k, n in f(p).items():
                    allc[k][0] += n
                    allc[k][1].append(f"{m}/{g}")
        o.append(f"\n## {title}\n")
        for k, (n, at) in sorted(allc.items(), key=lambda x: (-x[1][0], x[0])):
            where = f"{len(at)} pairs" if len(at) > 6 else ", ".join(at)
            o.append(f"- {n} x `{k[:300]}` ({where})")

    rank("Client stalls, by part", lambda p: p.stalls)
    rank("Script errors", lambda p: p.script)
    rank("Client warnings and problem lines", lambda p: p.client_warn)
    rank("Server warnings and problem lines", lambda p: p.server_warn)
    text = "\n".join(o) + "\n"
    open(os.path.join(out, "soak.md"), "w").write(text)
    print(text)


if __name__ == "__main__":
    main()
