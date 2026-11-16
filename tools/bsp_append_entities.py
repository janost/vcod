#!/usr/bin/env python3
"""Copy an IBSP59 map with entity blocks appended to its entity lump.

The grown lump (29) goes at the end of the file and the lump directory is
repointed, the bytes `vcod_common::testing::bsp_with_entities` builds. Zip
the result under the map's own path into a `zzz_*.pk3` in a retail
homepath's `main/` (`uo/` for UO) to run a stock map with extra entities.

    tools/bsp_append_entities.py in.bsp out.bsp blocks.ent
"""
import struct
import sys

src, dst, extra = sys.argv[1:4]
b = bytearray(open(src, "rb").read())
assert b[:4] == b"IBSP", "not an IBSP map"
d = 8 + 29 * 8
length, off = struct.unpack_from("<II", b, d)
ents = bytes(b[off:off + length]).rstrip(b"\0")
ents = ents + open(extra, "rb").read() + b"\0"
while len(b) % 4:
    b.append(0)
start = len(b)
b += ents
struct.pack_into("<II", b, d, len(ents), start)
open(dst, "wb").write(b)
