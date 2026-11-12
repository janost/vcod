#!/usr/bin/env python3
"""Print the create-a-class item table of CoD4 1.7's `iw3mp.exe`.

A create-a-class stat write (`statset`, `statsetusingtable` on 200..249)
passes the check at 0x4fc440, which looks the value up in the table at
0x724bb8 (0x4fc340): 61 rows of three words, the item's `mp/statstable.csv`
number, the rank it unlocks at and the slots it may fill (1 primary, 2
secondary, 4 perk 1, 8 perk 2, 0x10 perk 3, 0x20 special grenade).
docs/research/cod4-front-end.md 4.1 reads it.

    python3 -I tools/re/cod4_cac_items.py iw3mp.exe          # value rank slots rows
    python3 -I tools/re/cod4_cac_items.py iw3mp.exe --rust   # play::cod4_menus::cac table
"""
import struct
import sys

EXE = sys.argv[1]
RUST = "--rust" in sys.argv

TABLE = 0x724BB8
# 0x4fc35e: the walk stops at 0x2dc bytes.
SIZE = 0x2DC


def file_offset(d, va):
    pe = struct.unpack_from("<I", d, 0x3C)[0]
    n = struct.unpack_from("<H", d, pe + 6)[0]
    opt = struct.unpack_from("<H", d, pe + 20)[0]
    base = struct.unpack_from("<I", d, pe + 52)[0]
    o = pe + 24 + opt
    for _ in range(n):
        vs, rva, rs, pr = struct.unpack_from("<IIII", d, o + 8)
        if base + rva <= va < base + rva + min(vs, rs):
            return pr + va - base - rva
        o += 40
    raise SystemExit(f"{va:#x} is not in the file")


d = open(EXE, "rb").read()
at = file_offset(d, TABLE)
rows = [struct.unpack_from("<3i", d, at + 12 * i) for i in range(SIZE // 12)]
for value, rank, slots in rows:
    if RUST:
        print(f"    ({value}, {rank}, {slots:#x}),")
    else:
        print(value, rank, hex(slots))
