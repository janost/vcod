#!/usr/bin/env python3
"""Dump the CoD 1.1 script builtin tables and the spawn classname table.

    python3 tools/re/dump_builtins.py game.mp.i386.so [table|all]

Builtins are not one table. `Scr_GetFunction` (0x5c15c) walks `functions`
alone; `Scr_GetMethod` (0x5f724) tries Player, ScriptEnt and HudElem in that
order and falls back to the generic entity methods, so a name in an earlier
table shadows the same name in a later one. Each walk is a linear strcmp over
a fixed count, which is why the counts below are the loop bounds and not a
null terminator.

`functions` records are 12 bytes {char *name; void (*fn)(); int flag} and the
method records are 8 bytes {char *name; void (*fn)()}; a fixed 8-byte stride
over the whole region therefore garbles `functions`. `flag` is returned to
Scr_GetFunction's caller as a second out-parameter.

`spawns` (0x7eb30) is the classname table `G_CallSpawn` searches: a classname
in it becomes an engine entity built by its SP_ function, one not in it stays
a bare script-visible entity. Its records are 8 bytes and end at a null name.
"""
import struct
import sys

# (vaddr, stride, count or None for null-terminated, description)
TABLES = {
    "functions": (0x7E508, 12, 106, "Scr_GetFunction 0x5c15c"),
    "player_methods": (0x733DC, 8, 46, "Player_GetMethod 0x448e4"),
    "scriptent_methods": (0x78D40, 8, 12, "ScriptEnt_GetMethod 0x60f60"),
    "hudelem_methods": (0x749B4, 8, 14, "HudElem_GetMethod 0x4be38"),
    "entity_methods": (0x7EA00, 8, 38, "Scr_GetFunction+0x64 0x5c1c0"),
    "spawns": (0x7EB30, 8, None, "G_CallSpawn 0x615e4"),
}


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
        # A pointer stored in .data is 0 in the file when a relocation supplies
        # it, so the symbol name has to come from .rel.data, not the stored word.
        dyn = self.sec(".dynsym")
        dstr = self.sec(".dynstr")
        syms = []
        for i in range(dyn["size"] // 16):
            o = dyn["off"] + i * 16
            nm, = struct.unpack("<I", d[o:o + 4])
            syms.append(d[dstr["off"] + nm:d.index(b"\0", dstr["off"] + nm)].decode())
        self.rels = {}
        for name in (".rel.text", ".rel.data", ".rel.rodata"):
            s = self.sec(name)
            if not s:
                continue
            for i in range(s["size"] // 8):
                o = s["off"] + i * 8
                off, info = struct.unpack("<II", d[o:o + 8])
                self.rels[off] = syms[info >> 8]

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


def dump(e, kind):
    addr, stride, count, origin = TABLES[kind]
    print(f"# {kind}: {addr:#x} stride {stride}, from {origin}")
    i = 0
    while count is None or i < count:
        v = addr + i * stride
        name = e.cstr(e.word(v))
        if name is None:
            break
        fn = e.rels.get(v + 4) or f"{e.word(v + 4):#x}"
        extra = f"  flag={e.word(v + 8)}" if stride == 12 else ""
        print(f"  [{i:3}] {name:<28} {fn}{extra}")
        i += 1
    print(f"# {i} entries\n")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    e = Elf(sys.argv[1])
    which = sys.argv[2] if len(sys.argv) > 2 else "all"
    for kind in (TABLES if which == "all" else [which]):
        dump(e, kind)


if __name__ == "__main__":
    main()
