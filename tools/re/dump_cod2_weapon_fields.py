#!/usr/bin/env python3
"""Dump CoD2 1.3's weapon file field table and the enum name tables its
specific field types parse against, off the Linux dedicated server
`cod2_lnxded` (md5 in dump_cod2_gsc.py).

    python3 tools/re/dump_cod2_weapon_fields.py cod2_lnxded [rust]

`BG_LoadWeaponDefInternal` (0x80f3382; Mac 0xbd248) hands
`ParseConfigStringToStruct` (0x80b8a36; Mac 0x2975c) the table at
0x8186a60, 0x16e records of 12 bytes {char *name; int offset; int type}
into the 0x604-byte `WeaponDef`. Types 0..7 are the shared `cspFieldType`
(TYPES); 8..15 go to `BG_ParseWeaponDefSpecificFieldType` (0x80f2de8),
which looks the value up in a name table (ENUMS). `rust` prints the rows
of `vcod_cod2::weapon::FIELDS` and the enum arrays instead.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from dump_cod2_gsc import Image  # noqa: E402

FIELDS = 0x8186A60
COUNT = 0x16E

TYPES = ["string", "string1024", "string64", "string256", "int", "bool",
         "float", "ms", "weapType", "weapClass", "overlayReticle",
         "weapSlot", "stance", "projExplosion", "offhandClass",
         "playerAnimType"]

# specific type -> (name table vaddr, count); 15 reads
# mp/playeranimtypes.txt at run time instead.
ENUMS = {
    8: (0x81869E0, 4),
    9: (0x8186A20, 10),
    10: (0x8186A08, 5),
    11: (0x81869F0, 3),
    12: (0x8186A48, 3),
    13: (0x81869FC, 3),
    14: (0x8186A54, 3),
}

RUST_TYPES = {
    "string": "Str", "string1024": "Str", "string64": "Str",
    "string256": "Str", "int": "Int", "bool": "Bool", "float": "Float",
    "ms": "Ms", "weapType": "WeapType", "weapClass": "WeapClass",
    "overlayReticle": "OverlayReticle", "weapSlot": "Slot",
    "stance": "Stance", "projExplosion": "ProjExplosion",
    "offhandClass": "OffhandClass", "playerAnimType": "PlayerAnimType",
}


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    img = Image(sys.argv[1])
    rust = sys.argv[2:] == ["rust"]
    rows = []
    for i in range(COUNT):
        va = FIELDS + 12 * i
        rows.append((img.cstr(img.u32(va)), img.u32(va + 4), img.u32(va + 8)))
    enums = {t: [img.cstr(img.u32(va + 4 * k)) for k in range(n)]
             for t, (va, n) in ENUMS.items()}
    if rust:
        for name, off, t in rows:
            print(f'    ("{name}", {off:#x}, Ty::{RUST_TYPES[TYPES[t]]}),')
        for t, names in enums.items():
            quoted = ", ".join(f'"{n}"' for n in names)
            print(f"// {TYPES[t]}: [{quoted}]")
        return
    print(f"# weaponDefFields {FIELDS:#x}, {COUNT} records")
    for i, (name, off, t) in enumerate(rows):
        print(f"  [{i:3}] {name:<32} off={off:#06x} {TYPES[t]}")
    for t, names in enums.items():
        va, _ = ENUMS[t]
        print(f"# {TYPES[t]} ({t}) {va:#x}: {', '.join(names)}")


if __name__ == "__main__":
    main()
