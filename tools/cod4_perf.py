#!/usr/bin/env python3
"""cod4_perf.py [--skip-secs N] [--json] <profile.txt|dir> [...]: summarizes a
VCOD_FRAME_PROFILE file (crates/client/src/frame_prof.rs), or the
profile.txt in a directory (cod4_perf.sh's output dir): per CPU stage
and GPU pass group the mean, median, p95 and p99 ms per frame, the frame's
wall time and fps, and the counters' means, over every frame and over the
slowest twentieth by GPU time (`gpu.scene` sums the groups that were one
`scene` before the split). Frames from the first one that
draws a model on, less the first --skip-secs seconds of them (default 10:
the load's tail and the first effects' uploads), count. With several files
it prints one table each, then one row per file of the totals.

--sweep reads one file of a `cod4_perf.sh` sweep (SWEEP=1): its `# exec`
lines `echo sweep <pass> measure` and `echo sweep <pass> end` bound each
pass's frames, and it prints one row a pass: frames, fps, wall ms, the
GPU totals, every `gpu.<group>` of the finer stamps (`# groups`, `q`
lines) that any pass spent time in, the draws, prepass draws, the
fragments shaded and the MSAA samples."""
import json
import os
import sys

SCENE = ["opaque", "split", "blend", "post"]


size = "?"


def pct(v, p):
    if not v:
        return 0.0
    v = sorted(v)
    return v[min(len(v) - 1, int(p / 100 * len(v)))]


def profile_path(path):
    """A directory names cod4_perf.sh's output dir: read its profile.txt."""
    return os.path.join(path, "profile.txt") if os.path.isdir(path) else path


def load(path, skip_secs):
    global size
    stages, counts, gpu_names, frag_names = [], [], [], []
    frames, gpu, frags = {}, {}, {}
    with open(path) as f:
        for line in f:
            w = line.split()
            if not w:
                continue
            if w[0] == "#" and w[1] == "stages":
                stages = w[2:]
            elif w[0] == "#" and w[1] == "counts":
                counts = w[2:]
            elif w[0] == "#" and w[1] == "gpu":
                gpu_names = w[2:]
            elif w[0] == "#" and w[1] == "frags":
                frag_names = w[2:]
            elif w[0] == "#" and w[1] == "size":
                size = " ".join(w[2:])
            elif w[0] == "f":
                frames[int(w[1])] = [float(x) for x in w[2:]]
            elif w[0] == "g":
                gpu[int(w[1])] = [float(x) for x in w[2:]]
            elif w[0] == "p":
                frags[int(w[1])] = [int(x) for x in w[2:]]
        models = 1 + len(stages) + counts.index("models")
    order = sorted(frames)
    start = next((n for n in order if frames[n][models] > 0), None)
    keep, t = [], 0.0
    for n in order:
        if start is None or n < start:
            continue
        t += frames[n][0]
        if t / 1000.0 >= skip_secs:
            keep.append(n)
    return stages, counts, gpu_names, frames, gpu, keep, (frag_names, frags)


def summarize(path, skip_secs):
    stages, counts, gpu_names, frames, gpu, keep, (frag_names, frags) = load(path, skip_secs)
    rows = []
    wall = [frames[n][0] for n in keep]
    rows.append(("wall", wall))
    for i, s in enumerate(stages):
        rows.append((s, [frames[n][1 + i] for n in keep]))
    other = [frames[n][0] - sum(frames[n][1 : 1 + len(stages)]) for n in keep]
    rows.append(("other", other))
    gk = [n for n in keep if n in gpu]
    for i, g in enumerate(gpu_names):
        rows.append(("gpu." + g, [gpu[n][i] for n in gk]))
    # The groups that were one `scene` before the profile split it.
    parts = [gpu_names.index(g) for g in SCENE if g in gpu_names]
    if len(parts) == len(SCENE):
        rows.append(("gpu.scene", [sum(gpu[n][i] for i in parts) for n in gk]))
    if gk:
        rows.append(("gpu.total", [sum(gpu[n]) for n in gk]))

    def counts_over(ns):
        return {
            c: sum(frames[n][1 + len(stages) + i] for n in ns) / max(1, len(ns))
            for i, c in enumerate(counts)
        }

    cnt = counts_over(keep)
    # What the slowest twentieth of the frames by GPU time draws.
    worst = sorted(gk, key=lambda n: sum(gpu[n]))[-max(1, len(gk) // 20) :] if gk else []
    worst_cnt = counts_over(worst)
    mean_wall = sum(wall) / max(1, len(wall))
    # The main pass's fragments by draw kind (`p` lines), means.
    fk = [n for n in keep if n in frags]
    frag_mean = {
        k: sum(frags[n][i] for n in fk) / len(fk) for i, k in enumerate(frag_names) if fk
    }
    return {
        "path": path,
        "frames": len(keep),
        "size": size,
        "fps": 1000.0 / mean_wall if mean_wall else 0.0,
        "rows": {
            k: {
                "mean": sum(v) / max(1, len(v)),
                "p50": pct(v, 50),
                "p95": pct(v, 95),
                "p99": pct(v, 99),
            }
            for k, v in rows
        },
        "counts": cnt,
        "frags": frag_mean,
        "worst_counts": worst_cnt,
    }


def sweep(path):
    """One row per pass of a sweep profile, as dicts in pass order."""
    stages, counts, gpu_names, groups = [], [], [], []
    frames, gpu, grp, frags, samples = {}, {}, {}, {}, {}
    marks = []
    cur_samples = 0
    with open(path) as f:
        for line in f:
            w = line.split()
            if not w:
                continue
            if w[0] == "#":
                if w[1] == "stages":
                    stages = w[2:]
                elif w[1] == "counts":
                    counts = w[2:]
                elif w[1] == "gpu":
                    gpu_names = w[2:]
                elif w[1] == "groups":
                    groups = w[2:]
                elif w[1] == "size" and "samples" in w:
                    cur_samples = int(w[w.index("samples") + 1])
                elif w[1] == "exec" and w[3:5] == ["echo", "sweep"] and len(w) >= 7:
                    marks.append((int(w[2]), w[5], w[6]))
            elif w[0] == "f":
                frames[int(w[1])] = [float(x) for x in w[2:]]
                samples[int(w[1])] = cur_samples
            elif w[0] == "g":
                gpu[int(w[1])] = [float(x) for x in w[2:]]
            elif w[0] == "q":
                grp[int(w[1])] = [float(x) for x in w[2:]]
            elif w[0] == "p":
                frags[int(w[1])] = sum(int(x) for x in w[2:])
    starts = {name: n for n, name, what in marks if what == "measure"}
    rows = []
    for n, name, what in marks:
        if what != "end" or name not in starts:
            continue
        keep = [k for k in sorted(frames) if starts[name] <= k < n]
        if not keep:
            continue

        def mean(v):
            return sum(v) / len(v) if v else 0.0

        wall = mean([frames[k][0] for k in keep])
        gk = [k for k in keep if k in gpu]
        row = {
            "pass": name,
            "frames": len(keep),
            "fps": 1000.0 / wall if wall else 0.0,
            "wall": wall,
            "gpu.total": mean([sum(gpu[k]) for k in gk]),
        }
        for i, g in enumerate(gpu_names):
            row["gpu." + g] = mean([gpu[k][i] for k in gk])
        qk = [k for k in keep if k in grp]
        for i, g in enumerate(groups):
            row["grp." + g] = mean([grp[k][i] for k in qk])
        base = 1 + len(stages)
        for c in ("draws", "prepass", "entdraws", "tris"):
            if c in counts:
                row[c] = mean([frames[k][base + counts.index(c)] for k in keep])
        row["frags"] = mean([frags[k] for k in keep if k in frags])
        row["samples"] = max(samples[k] for k in keep)
        rows.append(row)
    return rows, groups


def print_sweep(path):
    rows, groups = sweep(path)
    if not rows:
        sys.exit(f"{path}: no sweep passes (no `# exec ... echo sweep` lines)")
    used = [g for g in groups if any(r["grp." + g] >= 0.005 for r in rows)]
    passes = ["gpu.total", "gpu.vcshadow", "gpu.cookies", "gpu.sunshadow", "gpu.opaque"]
    passes += ["gpu.split", "gpu.blend", "gpu.post"]
    # the pass groups any pass spent time in, then the finer groups
    cols = ["fps", "wall"] + [c for c in passes if any(r.get(c, 0) >= 0.005 for r in rows)]
    cols += ["gpu." + g for g in used]
    cols += ["draws", "prepass", "frags", "samples"]
    head = ["pass", "frames"] + cols
    print("\t".join(head))
    for r in rows:
        vals = [r["pass"], str(r["frames"])]
        for c in cols:
            if c.startswith("gpu.") and c not in passes:
                v = r["grp." + c[4:]]
            else:
                v = r.get(c, 0.0)
            if c == "frags":
                vals.append(f"{v:.0f}")
            elif c in ("draws", "prepass", "samples"):
                vals.append(f"{v:.0f}")
            else:
                vals.append(f"{v:.2f}")
        print("\t".join(vals))


def main():
    args = sys.argv[1:]
    if args[:1] == ["--sweep"] and len(args) == 2:
        print_sweep(profile_path(args[1]))
        return
    skip, as_json = 10.0, False
    if "--skip-secs" in args:
        i = args.index("--skip-secs")
        skip = float(args[i + 1])
        del args[i : i + 2]
    if "--json" in args:
        args.remove("--json")
        as_json = True
    if not args:
        sys.exit(__doc__)
    out = [summarize(profile_path(p), skip) for p in args]
    if as_json:
        print(json.dumps(out, indent=1))
        return
    for s in out:
        print(f"{s['path']}: {s['frames']} frames, {s['fps']:.1f} fps, {s['size']}")
        print(f"  {'stage':<12}{'mean':>8}{'p50':>8}{'p95':>8}{'p99':>8}")
        for k, r in s["rows"].items():
            print(
                f"  {k:<12}{r['mean']:8.2f}{r['p50']:8.2f}{r['p95']:8.2f}{r['p99']:8.2f}"
            )
        print("  " + "  ".join(f"{k} {v:.0f}" for k, v in s["counts"].items()))
        if s["frags"]:
            # a fragment is a pixel's worth whatever the MSAA: over the
            # window's pixels, the screens drawn
            dims = s["size"].split()[0].split("x")
            px = int(dims[0]) * int(dims[1]) if len(dims) == 2 and all(d.isdigit() for d in dims) else 0
            total = sum(s["frags"].values())
            line = "  frags " + "  ".join(f"{k} {v:.0f}" for k, v in s["frags"].items())
            if px:
                line += f"  ({total / px:.2f} screens)"
            print(line)
        print(
            "  gpu-slowest 5%: "
            + "  ".join(f"{k} {v:.0f}" for k, v in s["worst_counts"].items())
        )
    if len(out) > 1:
        for s in out:
            r = s["rows"]
            g = r.get("gpu.total", {}).get("mean", 0.0)
            print(f"{s['path']}\t{s['fps']:.1f} fps\twall {r['wall']['mean']:.2f}\tgpu {g:.2f}")


if __name__ == "__main__":
    main()
