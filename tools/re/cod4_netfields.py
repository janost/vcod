#!/usr/bin/env python3
"""Dump CoD4 1.7's netfield tables from a binary: `cod4_lnxded` (ELF) or
`iw3mp.exe` (PE). An entry is 16 bytes, {char *name; int offset; int bits;
int changeHints}. A table starts where something holds a pointer to it (code
or the per-eType list) and ends at the next table or the first entry whose
name pointer reads no identifier.

    python3 -I tools/re/cod4_netfields.py cod4_lnxded          # summary
    python3 -I tools/re/cod4_netfields.py cod4_lnxded --rust   # crates/cod4/src/net/fields.rs
    python3 -I tools/re/cod4_netfields.py cod4_lnxded --rows   # every row, for diffing two binaries

The per-eType list is 18 {table, count} pairs whose pointers all land on
entity tables; it is found by scanning for that shape
(docs/research/cod4-netfields.md).
"""
import re
import struct
import sys

path = sys.argv[1]
mode = sys.argv[2] if len(sys.argv) > 2 else ""
d = open(path, "rb").read()

# (va, file offset, size) of every initialised mapped section.
secs = []
if d[:4] == b"\x7fELF":
    shoff = struct.unpack_from("<I", d, 0x20)[0]
    shentsize, shnum, _ = struct.unpack_from("<HHH", d, 0x2e)
    for i in range(shnum):
        _, typ, _, addr, off, size = struct.unpack_from("<IIIIII", d, shoff + i * shentsize)
        if typ == 1 and addr:
            secs.append((addr, off, size))
else:
    pe = struct.unpack_from("<I", d, 0x3C)[0]
    nsec = struct.unpack_from("<H", d, pe + 6)[0]
    optsz = struct.unpack_from("<H", d, pe + 20)[0]
    base = struct.unpack_from("<I", d, pe + 24 + 28)[0]
    off = pe + 24 + optsz
    for i in range(nsec):
        vs, va, rs, pr = struct.unpack_from("<IIII", d, off + 8)
        secs.append((base + va, pr, min(vs, rs) if vs else rs))
        off += 40


def v2o(va):
    for a, o, s in secs:
        if a <= va < a + s:
            return o + va - a
    return None


def o2v(fo):
    for a, o, s in secs:
        if o <= fo < o + s:
            return a + fo - o
    return None


IDENT = re.compile(rb"[A-Za-z_][A-Za-z0-9_\[\]\.]{0,47}\x00")


def name_at(va):
    o = v2o(va)
    if o is None:
        return None
    m = IDENT.match(d, o)
    return d[o : m.end() - 1].decode() if m else None


def entry(va):
    o = v2o(va)
    if o is None or o + 16 > len(d):
        return None
    p, off, bits, hint = struct.unpack_from("<IiiI", d, o)
    name = name_at(p)
    if name is None or not 0 <= off < 0x4000 or not -128 <= bits <= 64 or hint > 3:
        return None
    return (name, off, bits, hint)


# Every word in the file (code operands too, so any alignment) that points
# into a mapped section.
pointed = set()
for i in range(0, len(d) - 3):
    v = struct.unpack_from("<I", d, i)[0]
    if v >= 0x400000 and v2o(v) is not None:
        pointed.add(v)

FIRST = {"eType", "modelindex", "commandTime", "origin[0]", "color.rgba", "absmin[1]"}

# Runs of valid entries, split where something points at an entry that
# opens a table (code also points inside a table, at a field it reads).
tables = {}
for a, o, s in secs:
    va = a
    while va + 16 <= a + s:
        if entry(va) is None:
            va += 4
            continue
        run = []
        while entry(va + 16 * len(run)) is not None:
            run.append(entry(va + 16 * len(run)))
        if len(run) >= 5:
            start = 0
            for k in range(1, len(run) + 1):
                if k == len(run) or (va + 16 * k in pointed and run[k][0] in FIRST):
                    if va + 16 * start in pointed:
                        tables[va + 16 * start] = run[start:k]
                    start = k
            va += 16 * len(run)
        else:
            va += 4

# The per-eType list: 18 consecutive {ptr, count} pairs, each a table start
# whose count equals that table's length.
etype_list = None
for a, o, s in secs:
    for fo in range(o, o + s - 18 * 8, 4):
        ok = True
        for k in range(18):
            p, n = struct.unpack_from("<II", d, fo + 8 * k)
            if p not in tables or len(tables[p]) != n or tables[p][0][0] != "eType":
                ok = False
                break
        if ok:
            etype_list = (o2v(fo), [struct.unpack_from("<I", d, fo + 8 * k)[0] for k in range(18)])
            break
    if etype_list:
        break


def kind(va, rows):
    first = rows[0][0]
    if first == "eType":
        return "entity"
    return {
        "modelindex": "client",
        "commandTime": "player",
        "origin[0]": "objective",
        "color.rgba": "hudelem",
        "absmin[1]": "archived",
    }.get(first)


ENT_NAMES = [
    "GENERAL", "PLAYER", "PLAYER_CORPSE", "ITEM", "MISSILE", "INVISIBLE",
    "SCRIPTMOVER", "SOUND_BLEND", "FX", "LOOP_FX", "PRIMARY_LIGHT", "MG42",
    "HELICOPTER", "PLANE", "VEHICLE", "VEHICLE_COLLMAP", "VEHICLE_CORPSE", "EVENTS",
]

named = {}
for va, rows in sorted(tables.items()):
    k = kind(va, rows)
    if k and k != "entity":
        named[k] = va
ent_tables = []
if etype_list:
    for va in etype_list[1]:
        if va not in ent_tables:
            ent_tables.append(va)

if mode == "--rows":
    for k in ["client", "player", "objective", "hudelem"]:
        for i, r in enumerate(tables[named[k]]):
            print(k, i, *r)
    for t, va in enumerate(etype_list[1]):
        for i, r in enumerate(tables[va]):
            print("entity", ENT_NAMES[t], i, *r)
elif mode == "--rust":
    mod = path.rsplit("/", 1)[-1]
    print(f"//! CoD4 1.7's netfield tables, generated by `tools/re/cod4_netfields.py` from")
    print(f"//! `{mod}`; do not edit by hand (docs/research/cod4-netfields.md).")
    print("")
    print("use super::delta::Field;")
    print("")

    def emit(name, va, rows, doc):
        print(f"/// {doc} (0x{va:x}, {len(rows)} fields).")
        print(f"pub const {name}: [Field; {len(rows)}] = [")
        for n, off, bits, hint in rows:
            print(f'    Field::new("{n}", {off}, {bits}, {hint}),')
        print("];")
        print("")

    emit("CLIENT_STATE", named["client"], tables[named["client"]], "`clientState_t`")
    emit("PLAYER_STATE", named["player"], tables[named["player"]], "`playerState_t`")
    emit("OBJECTIVE", named["objective"], tables[named["objective"]], "`objective_t`")
    emit("HUDELEM", named["hudelem"], tables[named["hudelem"]], "`hudelem_t`")
    for i, va in enumerate(ent_tables):
        users = [ENT_NAMES[t] for t, v in enumerate(etype_list[1]) if v == va]
        emit(f"ENTITY_{users[0]}", va, tables[va], "Entity table for eType " + ", ".join(users))
    print(f"/// The table each eType 0..17 reads with (0x{etype_list[0]:x}); a higher eType")
    print("/// (an event entity) reads with the last.")
    print("pub const ENTITY_BY_TYPE: [&[Field]; 18] = [")
    refs = [f"&ENTITY_{ENT_NAMES[etype_list[1].index(va)]}," for va in etype_list[1]]
    width = max(map(len, refs))
    for t, r in enumerate(refs):
        print(f"    {r:<{width}} // {t} {ENT_NAMES[t]}")
    print("];")
else:
    for va, rows in sorted(tables.items()):
        print(f"0x{va:x} {len(rows):4} {kind(va, rows)!s:9} {rows[0][0]} .. {rows[-1][0]}")
    if etype_list:
        print(f"eType list at 0x{etype_list[0]:x}:")
        for t, va in enumerate(etype_list[1]):
            print(f"  {t:2} {ENT_NAMES[t]:16} 0x{va:x} {len(tables[va])}")
    else:
        print("no eType list found")
