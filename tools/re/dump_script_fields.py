#!/usr/bin/env python3
"""Dump the CoD 1.1 script field tables out of the dedicated-server game module.

`GScr_AddFieldsFor*` walk static arrays of field records and register each one
with the script engine, which is what decides whether a field name on a script
object is engine-backed or a plain script-defined key. This prints those arrays.

    python3 tools/re/dump_script_fields.py game.mp.i386.so [entity|client|hudelem|all]

Record layouts (both end at a null name pointer):

    entity   16 bytes  {char *name; int offset; int type; void (*set)();}
    client   20 bytes  {char *name; int offset; int type; void (*set)(); void (*get)();}
    hudelem  20 bytes  same as client

A hook is a pointer stored in `.data`, so it reads as 0 in the file whenever a
relocation supplies it; this reads `.rel.data` through dump_builtins.Elf and
prints the symbol name in that case.

`type` selects the storage conversion in Scr_GetGenericField/Scr_SetGenericField
(0x6248c / 0x6257c); TYPES below is that switch's jump table at 0x7968c, read in
index order. A record whose type the registering function rejects is skipped and
never becomes a script field: entity and hudelem take 0-5, 7 and 8, client takes
0-5 and 7. Client field ids are tagged `| 0xC000`, which is how
Scr_GetEntityField (0x62824) routes an id to `ent->client` instead of the entity
table; the other two are plain indices.
"""
import struct
import sys

from dump_builtins import Elf

# Table virtual addresses, from the `mov $imm,%ebx` that starts each walk.
TABLES = {
    "entity": (0x78E68, 16, 0x0000, "GScr_AddFieldsForEntity 0x62400"),
    "client": (0x72ED4, 20, 0xC000, "GScr_AddFieldsForClient 0x41700"),
    "hudelem": (0x744E0, 20, 0x0000, "GScr_AddFieldsForHudElems 0x4bf84"),
}

# Scr_GetGenericField's switch, in index order. The parenthesised note is what
# the script sees; "undefined" means a zero stored value reads as undefined.
TYPES = {
    0: "int",
    1: "float",
    2: "string (char[] in place)",
    3: "string (u16 script-string, 0 -> undefined)",
    4: "vector (3 floats)",
    5: "entity (gentity_t*, null -> undefined)",
    6: "vector (0, float, 0)",
    7: "object (u16 handle, 0 -> undefined)",
    8: "string (u8 model index via G_ModelName)",
}

ACCEPTED = {
    "entity": lambda t: t <= 5 or t in (7, 8),
    "client": lambda t: t <= 5 or t == 7,
    "hudelem": lambda t: t <= 5 or t in (7, 8),
}


def dump(e, kind):
    table, stride, tag, origin = TABLES[kind]
    base = e.vf(table)
    if base is None:
        sys.exit(f"table {table:#x} is not in a loaded section")
    print(f"# {kind}: {table:#x} stride {stride}, from {origin}")
    i = 0
    while True:
        v = table + i * stride
        rec = e.d[base + i * stride: base + i * stride + stride]
        name_ptr, offset, typ = struct.unpack("<Iii", rec[:12])
        if name_ptr == 0:
            break
        hooks = []
        for j in range(12, stride, 4):
            word, = struct.unpack("<I", rec[j:j + 4])
            sym = e.rels.get(v + j)
            if sym or word:
                hooks.append(sym or f"{word:#x}")
        kept = ACCEPTED[kind](typ)
        print(f"  [{i:3}] id={tag | i:#06x} {e.cstr(name_ptr)!s:<22}"
              f" off={offset:<5} type={typ} {TYPES.get(typ, '?'):<40}"
              f"{'' if kept else ' SKIPPED'}"
              f"{(' hooks ' + ' '.join(hooks)) if hooks else ''}")
        i += 1
    print(f"# {i} records\n")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    e = Elf(sys.argv[1])
    which = sys.argv[2] if len(sys.argv) > 2 else "all"
    for kind in (TABLES if which == "all" else [which]):
        dump(e, kind)


if __name__ == "__main__":
    main()
