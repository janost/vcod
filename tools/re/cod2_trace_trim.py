# Trims a tools/re/cod2_trace_gdb.py dump into a fixture: every trace of
# entity 0 that struck a bone at a frame whose capture logs a `PROBE
# damage` round, and every EVERY-th trace besides; of each trace's bones only BOXED (retail's DObj
# indices of the bones whose hit box is not zero).
#   python3 -I tools/re/cod2_trace_trim.py <dump> <capture> <out> [EVERY]
import sys

BOXED = {int(i) for i in (
    "2 3 4 15 16 19 20 21 23 27 28 30 32 33 34 37 38 41 42 43 44 46 47 48 49 "
    "50 51 52 53 54 55 58 59 60 61 62 63 64 65 66 67 68 69 70 71 72 73 74 75 "
    "76 77").split()}
dump, capture, out = sys.argv[1:4]
every = int(sys.argv[4]) if len(sys.argv) > 4 else 25
rounds = set()
for line in open(capture):
    if "PROBE damage" in line:
        rounds.add(int(line.split("PROBE damage")[1].split()[1]))
blocks, cur = [], None
for line in open(dump):
    t = line.split()
    if t[0] == "point":
        cur = [line]
        blocks.append(cur)
    elif t[0] == "trace":
        if cur is None or cur[-1].startswith(("trace", "mat", "result")):
            cur = []
            blocks.append(cur)
        cur.append(line)
    elif t[0] == "build":
        continue
    elif t[0] == "mat":
        if int(t[1]) in BOXED:
            cur.append(line)
    else:
        cur.append(line)
        if t[0] == "result":
            cur = None
with open(out, "w") as f:
    for n, b in enumerate(blocks):
        tr = next(l.split() for l in b if l.startswith("trace"))
        res = next(l.split() for l in b if l.startswith("result"))
        struck = not res[7].endswith("0000")
        if (int(tr[3]) in rounds and tr[7] == "0" and struck) or n % every == 0:
            f.writelines(b)
