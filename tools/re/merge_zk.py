#!/usr/bin/env python3
"""Fold zk_libcod's hand-named 1.3 addresses into a symmap.py output.

usage: merge_zk.py SYMS functions.hpp OUT

An address only zk names is added with method "zk". Where both name it
differently, zk wins over the weak propagation methods (calls, callers,
callees, data, scalars) and loses to table/strings/strset; either way the
line notes the other name.
"""
import re
import sys

WEAK = {"calls", "callers", "callees", "data", "scalars"}
rows = {}
head = []
for line in open(sys.argv[1]):
    if line.startswith("#"):
        head.append(line)
        continue
    a, kind, name, method, mac = line.split()
    rows[int(a, 16)] = [kind, name, method, mac, ""]
zk = {int(m.group(2), 16): m.group(1) for m in re.finditer(
    r"static const \w+ (\w+) = \(\w+\)(0x[0-9A-Fa-f]+);", open(sys.argv[2]).read())}
added = over = kept = 0
for a, name in zk.items():
    r = rows.get(a)
    if r is None:
        rows[a] = ["T", name, "zk", "-", ""]
        added += 1
    elif r[1].lower() != name.lower():
        if r[2] in WEAK:
            rows[a] = ["T", name, "zk", r[3], f"was:{r[1]}"]
            over += 1
        else:
            r[4] = f"zk:{name}"
            kept += 1
with open(sys.argv[3], "w") as fo:
    fo.writelines(head)
    for a in sorted(rows):
        fo.write(f"{a:08x} " + " ".join(x for x in rows[a] if x) + "\n")
print(f"zk added {added}, zk over weak {over}, ours kept with zk note {kept}, total {len(rows)}")
