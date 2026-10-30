#!/usr/bin/env python3
"""Dump the gsc builtin, method, field and spawn tables of CoD2 1.3's Linux
dedicated server, `cod2_lnxded` (md5 fefed4c4909fb7c5c13b13a003f5ccf7).

    python3 tools/re/dump_cod2_gsc.py cod2_lnxded [names.syms] [table|all]

The game module is linked into the executable (no separate game `.so`), the
file is ET_EXEC and `.data` pointers hold real addresses, so the tables are
read straight off the two PT_LOAD segments. The binary is stripped; the table
addresses below were read off the code that walks each one
(docs/research/cod2-gsc-diff.md section 1 names the walker of each). An
optional `addr kind name ...` symbol file names the handlers.

Every builtin record is 12 bytes {char *name; void (*fn)(); int developer},
methods included (1.1's methods were 8). Field records:

    entity   16 bytes {name; offset; type; setter}
    client   20 bytes {name; offset; type; setter; getter}
    hudelem  28 bytes {name; offset; type; mask; shift; setter; getter}

`type` is `fieldtype_t`, the same enum as 1.1 (TYPES).
"""
import hashlib
import struct
import sys

MD5 = "fefed4c4909fb7c5c13b13a003f5ccf7"

# kind -> (vaddr, stride, count or None for null-terminated, walker)
TABLES = {
    "functions": (0x8188040, 12, 145, "Scr_GetFunction 0x8117cb2"),
    "player_methods": (0x8158a40, 12, 59, "0x80fddac (Player_GetMethod)"),
    "scriptent_methods": (0x815e3a0, 12, 12, "0x811a1fa (ScriptEnt_GetMethod)"),
    "entity_methods": (0x8188720, 12, 59, "0x8117d50 (generic entity methods)"),
    "hudelem_methods": (0x815a180, 12, 18, "0x81047fe (HudElem_GetMethod)"),
    "entity_fields": (0x815e820, 16, None, "0x811ac48 (GScr_AddFieldsForEntity)"),
    "client_fields": (0x8157ce0, 20, None, "0x80f84be (GScr_AddFieldsForClient)"),
    "hudelem_fields": (0x8159ee0, 28, None, "0x8103dca (GScr_AddFieldsForHudElems)"),
    "spawns": (0x8188a00, 8, None, "G_CallSpawn's classname loop"),
}

TYPES = ["int", "float", "lstring", "string", "vector", "entity",
         "vectorhack", "object", "model"]


class Image:
    def __init__(self, path):
        self.d = open(path, "rb").read()
        if hashlib.md5(self.d).hexdigest() != MD5:
            sys.exit(f"{path}: not cod2_lnxded 1.3 (md5 {MD5})")
        phoff, = struct.unpack("<I", self.d[0x1C:0x20])
        phent, phnum = struct.unpack("<HH", self.d[0x2A:0x2E])
        self.segs = []
        for i in range(phnum):
            o = phoff + i * phent
            typ, off, va, _, filesz = struct.unpack("<IIIII", self.d[o:o + 20])
            if typ == 1:
                self.segs.append((va, off, filesz))

    def off(self, va):
        for v, o, s in self.segs:
            if v <= va < v + s:
                return o + va - v
        raise ValueError(f"{va:#x} outside the file")

    def u32(self, va):
        return struct.unpack("<I", self.d[self.off(va):self.off(va) + 4])[0]

    def cstr(self, va):
        o = self.off(va)
        return self.d[o:self.d.index(b"\0", o)].decode("latin1")


def load_names(path):
    names = {}
    for line in open(path):
        if line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) >= 3:
            names[int(parts[0], 16)] = parts[2]
    return names


def fn(names, va):
    if va == 0:
        return "-"
    return names.get(va, f"{va:#x}")


def dump(img, names, kind):
    va, stride, count, walker = TABLES[kind]
    print(f"# {kind}: {va:#x} stride {stride}, walked by {walker}")
    i = 0
    while count is None or i < count:
        w = [img.u32(va + i * stride + 4 * k) for k in range(stride // 4)]
        if w[0] == 0:
            break
        name = img.cstr(w[0])
        if kind.endswith("methods") or kind == "functions":
            row = f"{fn(names, w[1]):<28} dev={w[2]}"
        elif kind == "spawns":
            row = fn(names, w[1])
        else:
            t = TYPES[w[2]] if w[2] < len(TYPES) else str(w[2])
            row = f"off={w[1]:<6} {t:<8}"
            if kind == "hudelem_fields":
                row += f" mask={w[3]:#x} shift={w[4]}"
                hooks = w[5:7]
            else:
                hooks = w[3:5]
            row += " " + " ".join(fn(names, h) for h in hooks)
        print(f"  [{i:3}] {name:<28} {row}")
        i += 1
    print(f"# {i} records\n")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    img = Image(sys.argv[1])
    args = sys.argv[2:]
    names = {}
    if args and not args[0] in TABLES and args[0] != "all":
        names = load_names(args.pop(0))
    which = args[0] if args else "all"
    for kind in TABLES if which == "all" else [which]:
        dump(img, names, kind)


if __name__ == "__main__":
    main()
