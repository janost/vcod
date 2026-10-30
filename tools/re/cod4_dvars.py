#!/usr/bin/env python3
"""Dump dvar registrations out of the stripped retail `cod4_lnxded` (1.7).

The binary has no symbols, so the registration functions are found by the
dvars they register: `Dvar_RegisterFloat` is the call that takes
`"jump_height"`, `Dvar_RegisterBool` the one taking `"jump_slowdownEnable"`,
`Dvar_RegisterInt` the one taking `"player_sprintForwardMinimum"`. gcc
builds each call's arguments with `mov DWORD PTR [esp+N], <imm or reg>`
(a register loaded from an immediate or zeroed with `xor` earlier in the
same run of stores), so the arguments are read back from the stores between
two calls of an `objdump -d -M intel` listing:

    float  name, value, min, max, flags, description
    bool   name, value, flags, description
    int    name, value, min, max, flags, description

    python3 -I tools/re/cod4_dvars.py <cod4_lnxded> [name-prefix ...]

Prints `name type default min max flags` per registration, the ones whose
name starts with a prefix when any are given (docs/research/cod4-pmove.md,
"Dvars", ran it with `bg_ jump_ player_ friction stopspeed inertia
mantle_ perk_`). A dvar registered twice prints twice. Needs binutils'
`objdump` and `readelf`.
"""
import re
import struct
import subprocess
import sys

ANCHORS = {
    "jump_height": "float",
    "jump_slowdownEnable": "bool",
    "player_sprintForwardMinimum": "int",
}
ARGS = {
    "float": ("value", "min", "max", "flags"),
    "bool": ("value", "flags"),
    "int": ("value", "min", "max", "flags"),
}

STORE = re.compile(r"mov\s+DWORD PTR \[esp(?:\+0x([0-9a-f]+))?\],(\S+)$")
REG_IMM = re.compile(r"mov\s+(e[a-ds][xip]|e[sd]i),0x([0-9a-f]+)$")
REG_XOR = re.compile(r"xor\s+(e\w\w),\1$")
CALL = re.compile(r"call\s+([0-9a-f]+)")
FLD = re.compile(r"fld\s+DWORD PTR ds:0x([0-9a-f]+)$")
FSTP = re.compile(r"fstp\s+DWORD PTR \[esp(?:\+0x([0-9a-f]+))?\]$")


def sections(path):
    """(vaddr, offset, size) of each section with file bytes, from readelf."""
    out = subprocess.run(["readelf", "-SW", path], capture_output=True, text=True, check=True)
    rows = []
    for line in out.stdout.splitlines():
        m = re.search(r"\]\s+\S+\s+PROGBITS\s+([0-9a-f]{8})\s+([0-9a-f]{6,8})\s+([0-9a-f]{6,8})", line)
        if m:
            rows.append(tuple(int(g, 16) for g in m.groups()))
    return rows


def cstring(data, secs, va):
    for base, off, size in secs:
        if base and base <= va < base + size:
            o = off + va - base
            end = data.find(b"\0", o)
            if end < 0:
                return None
            s = data[o:end]
            if s and all(32 <= c < 127 for c in s):
                return s.decode()
    return None


def dword(data, secs, va):
    for base, off, size in secs:
        if base and base <= va < base + size - 3:
            return struct.unpack_from("<I", data, off + va - base)[0]
    return None


def calls(path, data, secs):
    """Each call's target and the `[esp+N]` arguments stored before it."""
    asm = subprocess.run(
        ["objdump", "-d", "-M", "intel", "--no-show-raw-insn", path],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    args, regs, top = {}, {}, None
    for line in asm.splitlines():
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        ins = parts[-1].strip()
        if m := CALL.match(ins):
            yield int(m.group(1), 16), args
            # The callee clobbers eax, ecx and edx; ebx, esi and edi live on.
            args, top = {}, None
            regs = {r: v for r, v in regs.items() if r in ("ebx", "esi", "edi")}
        elif m := FLD.match(ins):
            top = dword(data, secs, int(m.group(1), 16))
        elif ins in ("fldz", "fld1"):
            top = 0 if ins == "fldz" else 0x3F800000
        elif m := FSTP.match(ins):
            if top is not None:
                args[int(m.group(1) or "0", 16)] = top
            top = None
        elif m := STORE.match(ins):
            off = int(m.group(1) or "0", 16)
            src = m.group(2)
            if src.startswith("0x"):
                args[off] = int(src, 16)
            elif src in regs:
                args[off] = regs[src]
        elif m := REG_IMM.match(ins):
            regs[m.group(1)] = int(m.group(2), 16)
        elif m := REG_XOR.match(ins):
            regs[m.group(1)] = 0
        elif ins.startswith(("ret", "jmp")):
            args, regs, top = {}, {}, None


def show(kind, field, v):
    if v is None:
        return "?"
    if field == "flags":
        return hex(v)
    if kind == "float":
        return f"{struct.unpack('<f', struct.pack('<I', v))[0]:g}"
    if kind == "int" and v >= 1 << 31:
        v -= 1 << 32
    return str(v)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    path, prefixes = sys.argv[1], sys.argv[2:]
    data = open(path, "rb").read()
    secs = sections(path)
    found = []
    kinds = {}
    for target, args in calls(path, data, secs):
        name = cstring(data, secs, args.get(0, 0))
        if name is None:
            continue
        found.append((target, name, args))
        if name in ANCHORS:
            kinds[target] = ANCHORS[name]
    missing = set(ANCHORS.values()) - set(kinds.values())
    if missing:
        sys.exit(f"no registration function found for {sorted(missing)}")
    for target, name, args in found:
        kind = kinds.get(target)
        if kind is None or (prefixes and not name.startswith(tuple(prefixes))):
            continue
        vals = [show(kind, f, args.get(4 * (i + 1))) for i, f in enumerate(ARGS[kind])]
        if kind == "bool":
            vals = [vals[0], "", "", vals[1]]
        print(f"{name} {kind} {' '.join(v or '-' for v in vals)}")


if __name__ == "__main__":
    main()
