#!/usr/bin/env python3
"""Disassemble one function of a PIC ELF with its relocations resolved.

The game module is position-independent, so every call and every global data
reference in `objdump -d` output is a placeholder (`e8 fc ff ff ff`) or an
ebx-relative offset, and the real target lives in `.rel.text`. Reading a
function without resolving those is reading it blind: the traces, the cvars
and the tunables are exactly the operands that go missing.

Usage: annotate_func.py <elf> <symbol|0xaddr> [--raw]
"""
import re
import subprocess
import sys


def relocations(elf):
    out = subprocess.run(
        ["readelf", "-r", elf], capture_output=True, text=True, check=True
    ).stdout
    rel = {}
    for line in out.splitlines():
        f = line.split()
        if len(f) >= 4 and re.fullmatch(r"[0-9a-f]{8}", f[0]):
            rel[int(f[0], 16)] = (f[2], f[4] if len(f) > 4 else "")
    return rel


def symbol(elf, name):
    out = subprocess.run(
        ["nm", "-D", "--defined-only", "-S", elf], capture_output=True, text=True
    ).stdout
    for line in out.splitlines():
        f = line.split()
        if len(f) == 4 and f[3] == name:
            return int(f[0], 16), int(f[1], 16)
    raise SystemExit(f"no symbol {name}")


def main():
    elf, what = sys.argv[1], sys.argv[2]
    if what.startswith("0x"):
        start, size = int(what, 16), 0x400
    else:
        start, size = symbol(elf, what)
    rel = relocations(elf)
    dis = subprocess.run(
        [
            "objdump", "-d", "-M", "intel",
            f"--start-address={start:#x}", f"--stop-address={start + size:#x}", elf,
        ],
        capture_output=True, text=True, check=True,
    ).stdout
    for line in dis.splitlines():
        m = re.match(r"\s+([0-9a-f]+):\t", line)
        if not m:
            print(line)
            continue
        addr = int(m.group(1), 16)
        # A relocation applies to an operand inside the instruction, so look
        # a few bytes past its start rather than at it.
        hit = next(
            ((rel[a][1], rel[a][0]) for a in range(addr, addr + 8) if a in rel), None
        )
        print(line + (f"   ; {hit[0]} ({hit[1]})" if hit else ""))


if __name__ == "__main__":
    main()
