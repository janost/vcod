#!/usr/bin/env python3
"""Dump iw3mp.exe 1.7's vision file field table (0x727610).

Usage: cod4_vision_fields.py <iw3mp.exe>

Rows are (name, offset, type) words, type 0 a bool, 1 a float, 2 a vec3,
up to the first row whose name is not a string. cod4-world.md 11.2 and
`vcod_cod4::look`'s field table read this output.
"""
import struct
import sys

TABLE = 0x727610
SECTIONS = [(0x401000, 0x1000, 0x28F429), (0x691000, 0x291000, 0x8987A), (0x71B000, 0x31B000, 0x10000)]
KINDS = {0: "bool", 1: "float", 2: "vec3"}


def main(path):
    data = open(path, "rb").read()

    def off(va):
        for base, at, size in SECTIONS:
            if base <= va < base + size:
                return at + va - base
        return None

    def cstr(va):
        o = off(va)
        if o is None:
            return None
        s = data[o : data.index(b"\0", o)]
        return s.decode() if s and all(32 <= c < 127 for c in s) else None

    va = TABLE
    while True:
        name, field, kind = struct.unpack_from("<III", data, off(va))
        text = cstr(name)
        if text is None:
            break
        print(f"{va:#x} {text:<28} {field:>3} {KINDS.get(kind, kind)}")
        va += 12


if __name__ == "__main__":
    main(sys.argv[1])
