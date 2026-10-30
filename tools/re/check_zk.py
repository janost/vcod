#!/usr/bin/env python3
"""Score a symmap.py output against zk_libcod's hand-named 1.3 addresses.

usage: check_zk.py SYMS functions.hpp
"""
import re
import sys

syms = {}
for line in open(sys.argv[1]):
    if line.startswith("#"):
        continue
    a, kind, name, method, _ = line.split()
    syms[int(a, 16)] = (name, method)

zk = {}
for m in re.finditer(r"static const \w+ (\w+) = \(\w+\)(0x[0-9A-Fa-f]+);", open(sys.argv[2]).read()):
    zk[int(m.group(2), 16)] = m.group(1)

hit = agree = 0
for a, name in sorted(zk.items()):
    if a in syms:
        hit += 1
        ours, method = syms[a]
        if ours.lower() == name.lower():
            agree += 1
        else:
            print(f"differ {a:08x} zk={name} ours={ours} ({method})")
print(f"zk names {len(zk)}, at an address we named {hit}, same name {agree}")
