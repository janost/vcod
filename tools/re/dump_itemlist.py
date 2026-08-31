#!/usr/bin/env python3
"""Dump `bg_itemlist`, the classname table `BG_FindItem` (0x2e214) searches
for `precacheItem`/`G_RegisterItem` (0x4e504).

    python3 tools/re/dump_itemlist.py game.mp.i386.so

`bg_itemlist` is at `.data` 0x7b9d8, stride 0x30 (classname at +0x0, matching
`BG_FindItem`'s strcmp target), `bg_numItems` = 70 (`.rodata` 0x70804).

Unlike the function-pointer tables in `dump_builtins.py`, these are pointers
into the object's own `.rodata`, not calls to an exported symbol: the linker
emits an `R_386_RELATIVE` relocation whose stored placeholder already holds
the correct link-time address for a base-0 load (the convention this whole
toolkit's `Elf.vf`/`Elf.cstr` already assume), so the raw dword read from the
file *is* the string's vaddr directly. `Elf.rels`, built from `.dynsym`,
resolves function-pointer relocations by name; here it would report the
empty-name symbol (index 0, used for `R_386_RELATIVE`) for every row and go
unused, which is a second, distinct trap from the null-pointer one
`dump_builtins.py` documents for exported function pointers.

Only indices 65-69 carry a real compiled-in classname: `item_ammo_stiel-
handgranate_open/closed` (65, 66) and `item_health_small/_health/_large`
(67-69), matching `docs/research/cod11-sound-system.md`'s independent find
from the cgame DLL's copy of this table. Indices 1-64 hold placeholder
classnames `emptyitem_"wNN"` — real weapon classnames like `mp40_mp` are
absent from the binary entirely (grep it) and only reach these slots at
runtime, from the mounted paks' weapon files, in an order this static dump
cannot recover. See docs/research/cod11-gsc-object-model.md for the
consequence for `Items::register`.
"""
import struct
import sys

ITEMLIST = 0x7B9D8
STRIDE = 0x30
NUM_ITEMS = 70


class Elf:
    def __init__(self, path):
        d = self.d = open(path, "rb").read()
        shoff, = struct.unpack("<I", d[0x20:0x24])
        entsize, = struct.unpack("<H", d[0x2E:0x30])
        num, = struct.unpack("<H", d[0x30:0x32])
        stridx, = struct.unpack("<H", d[0x32:0x34])
        self.secs = []
        for i in range(num):
            o = shoff + i * entsize
            nm, typ, _, addr, off, size = struct.unpack("<IIIIII", d[o:o + 24])
            self.secs.append(dict(nm=nm, typ=typ, addr=addr, off=off, size=size))
        base = self.secs[stridx]["off"]
        for s in self.secs:
            o = base + s["nm"]
            s["n"] = d[o:d.index(b"\0", o)].decode()

    def sec(self, n):
        return next((s for s in self.secs if s["n"] == n), None)

    def vf(self, v):
        for s in self.secs:
            if s["addr"] and s["addr"] <= v < s["addr"] + s["size"] and s["typ"] != 8:
                return s["off"] + (v - s["addr"])
        return None

    def cstr(self, v):
        f = self.vf(v) if v else None
        return None if f is None else self.d[f:self.d.index(b"\0", f)].decode("latin1")

    def word(self, v):
        f = self.vf(v)
        return struct.unpack("<I", self.d[f:f + 4])[0]


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    e = Elf(sys.argv[1])
    num_items = e.word(0x70804)
    print(f"# bg_itemlist: {ITEMLIST:#x} stride {STRIDE:#x}, bg_numItems={num_items}")
    for i in range(NUM_ITEMS):
        row = ITEMLIST + i * STRIDE
        classname = e.cstr(e.word(row)) or ""
        pickup_sound = e.cstr(e.word(row + 4)) or ""
        print(f"  [{i:3}] {classname:<40} pickup_sound={pickup_sound!r}")


if __name__ == "__main__":
    main()
