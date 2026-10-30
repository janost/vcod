#!/usr/bin/env python3
"""Dump iw3mp.exe 1.7's menu command table and the word reads of each handler.

Usage: cod4_menu_commands.py <iw3mp.exe>

The table at 0x6b4940 holds (name, handler) pairs up to the first name that
is not a string. For each handler the script prints, in address order, its
calls to the menu script's word readers and its backward jumps (a loop):

- 0x54b510 reads one word into a buffer (the menu's string reader),
- 0x571400 reads the next word and returns it (the token reader),
- 0x558f40 reads a word through 0x571400 as an int.

Then `UI_RunMenuScript` (0x545bc0): each name it compares the script's word
with (`push <name>` before the compare at 0x571a90), and the calls on the
branch that name takes, to its `ret` or `jmp`, with the readers among them
and the calls into helpers that read.

A handler ends at a `ret` that no jump inside it passes, at int3 padding
or at the next handler; calls it makes into the menu
code (0x54b000..0x54e000) are followed one level deep and shown in
parentheses.
cod4-front-end.md 2 reads the counts off this output.
"""
import re
import struct
import subprocess
import sys

TABLE = 0x6B4940
# Calls into the menu code that the dump follows, one level deep.
HELPERS = (0x54B000, 0x54E000)
READERS = {0x54B510: "word", 0x571400: "token", 0x558F40: "int"}
UI_SCRIPT = (0x545BC0, 0x546E70)
COMPARE = "0x571a90"


def sections(path):
    out = subprocess.run(["objdump", "-h", path], capture_output=True, text=True).stdout
    rows = re.findall(r"^\s*\d+\s+(\S+)\s+([0-9a-f]+)\s+([0-9a-f]+)\s+[0-9a-f]+\s+([0-9a-f]+)", out, re.M)
    return [(int(va, 16), int(size, 16), int(off, 16)) for _, size, va, off in rows]


def main(path):
    data = open(path, "rb").read()
    secs = sections(path)

    def at(va, n):
        for base, size, off in secs:
            if base <= va < base + size:
                return data[off + va - base : off + va - base + n]
        return None

    def cstr(va):
        b = at(va, 256)
        if not b or b"\0" not in b:
            return None
        s = b[: b.index(b"\0")]
        return s.decode() if s and all(32 <= c < 127 for c in s) else None

    rows = []
    va = TABLE
    while True:
        name, handler = struct.unpack("<II", at(va, 8))
        text = cstr(name)
        if text is None or text.startswith("EXE_"):
            break
        rows.append((va, text, handler))
        va += 8

    def body(start):
        """The function at `start`: up to a `ret` no jump inside it passes,
        int3 padding or the next handler in the table."""
        asm = subprocess.run(
            ["objdump", "-d", "-M", "intel", "--no-show-raw-insn",
             f"--start-address={start:#x}", f"--stop-address={start + 0x600:#x}", path],
            capture_output=True, text=True).stdout
        reach = start
        for line in asm.splitlines():
            m = re.match(r"\s*([0-9a-f]+):\s+(\S+)\s*(.*)", line)
            if not m:
                continue
            addr, op = int(m[1], 16), m[2]
            if op == "int3" or (addr != start and addr in starts):
                return
            target = re.match(r"(0x[0-9a-f]+)$", m[3].strip())
            target = int(target[1], 16) if target else None
            if op.startswith("j") and target is not None and start < target < start + 0x600:
                reach = max(reach, target)
            yield addr, op, target
            if op == "ret" and addr >= reach:
                return

    def reads(start, depth):
        out = []
        for addr, op, target in body(start):
            if op == "call" and target in READERS:
                out.append(f"{READERS[target]}@{addr:#x}")
            elif op == "call" and target is not None and HELPERS[0] <= target < HELPERS[1] and depth:
                inner = reads(target, depth - 1)
                if inner:
                    out.append(f"{target:#x}({' '.join(inner)})")
            elif op.startswith("j") and target is not None and start <= target < addr:
                out.append(f"loop->{target:#x}")
        return out

    starts = {handler for _, _, handler in rows}
    for row, name, handler in rows:
        print(f"{row:#x} {name:<36} {handler:#x} {' '.join(reads(handler, 1))}")

    print()
    asm = subprocess.run(
        ["objdump", "-d", "-M", "intel", "--no-show-raw-insn",
         f"--start-address={UI_SCRIPT[0]:#x}", f"--stop-address={UI_SCRIPT[1]:#x}", path],
        capture_output=True, text=True).stdout
    ins = []
    for line in asm.splitlines():
        m = re.match(r"\s*([0-9a-f]+):\s+(\S+)\s*(.*)", line)
        if m:
            ins.append((int(m[1], 16), m[2], m[3].strip()))
    index = {addr: i for i, (addr, _, _) in enumerate(ins)}
    sites = []
    for i, (addr, op, arg) in enumerate(ins):
        if op != "push" or not re.match(r"0x6[0-9a-f]{5}$", arg):
            continue
        near = ins[i + 1 : i + 8]
        if not any(o == "call" and a == COMPARE for _, o, a in near):
            continue
        branch = next(((o, int(a, 16)) for _, o, a in near if o in ("je", "jne")), None)
        if branch:
            jump = i + 1 + next(k for k, (_, o, _) in enumerate(near) if o in ("je", "jne"))
            sites.append((addr, cstr(int(arg, 16)), branch, jump))
    site_starts = {addr for addr, _, _, _ in sites}
    for addr, name, (op, target), jump in sites:
        i = jump + 1 if op == "jne" else index.get(target)
        calls = []
        while i is not None and i < len(ins):
            a, o, arg = ins[i]
            if a in site_starts and calls is not None and o == "push":
                break
            if o == "call" and re.match(r"0x[0-9a-f]+$", arg):
                t = int(arg, 16)
                if t in READERS:
                    calls.append(f"{READERS[t]}@{a:#x}")
                else:
                    inner = reads(t, 0)
                    if any(not r.startswith("loop") for r in inner):
                        calls.append(f"{t:#x}({' '.join(inner)})")
            if o in ("ret", "jmp"):
                break
            i += 1
        print(f"uiScript {name:<28} {addr:#x} {' '.join(calls)}")


if __name__ == "__main__":
    main(sys.argv[1])
