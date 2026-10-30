#!/usr/bin/env python3
"""Read CoD4 1.7's static Huffman tree out of `cod4_lnxded`.

The Linux server never sorts `msg_hData`: its tree is built by straight-line
code at 0x812af70 (called from the `Huffman Took %d Milliseconds` init at
0x8130a5a) that stores every node's weight, symbol and links into the
huffman state as constants. This replays those stores and prints each
symbol's code, root first, `0` for the node's first link
(docs/research/cod4-handshake.md, section 4).

    python3 -I tools/re/cod4_huffman.py cod4_lnxded          # symbol, weight, code
    python3 -I tools/re/cod4_huffman.py cod4_lnxded --rust   # huffman.rs's CODES

Symbol 256 is the NYT leaf, the decoder's end mark.

Needs `objdump` on the path.
"""
import re
import subprocess
import sys

BUILD = 0x812AF70
NODE = 0x14  # {first, second, parent, weight, symbol}

path = sys.argv[1]
mode = sys.argv[2] if len(sys.argv) > 2 else ""
asm = subprocess.run(
    ["objdump", "-d", "--no-show-raw-insn", f"--start-address={BUILD:#x}",
     f"--stop-address={BUILD + 0x8000:#x}", path],
    capture_output=True, text=True, check=True).stdout

regs = {}
mem = {}  # offset into the state -> int or ("p", offset)
MEMREF = re.compile(r"^(-?0x[0-9a-f]+)?\(%ebx\)$")


def off(s):
    m = MEMREF.match(s)
    if not m:
        return None
    return int(m.group(1), 16) if m.group(1) else 0


started = False
for line in asm.splitlines():
    parts = line.split("\t")
    if len(parts) < 2:
        continue
    ins = parts[-1].split("<")[0].strip()
    if not ins:
        continue
    op, _, args = ins.partition(" ")
    args = [a.strip() for a in re.split(r",(?![^(]*\))", args.strip())] if args else []
    if op == "ret":
        break
    if op == "call":
        started = True  # the memset of the state
        continue
    if not started:
        continue
    if op == "lea":
        o = off(args[0])
        if o is not None:
            regs[args[1]] = ("p", o)
    elif op in ("mov", "movl"):
        src, dst = args
        if src.startswith("$"):
            val = int(src[1:], 16)
        elif src.startswith("%"):
            val = regs.get(src)
        else:
            o = off(src)
            val = mem.get(o, 0) if o is not None else None
        o = off(dst)
        if o is not None:
            mem[o] = val
        elif dst.startswith("%"):
            regs[dst] = val


def field(node, i):
    return mem.get(node + 4 * i, 0)


# The node pool: the NYT at 0x414, leaf `s` at 0x428 + s * NODE, the
# internal nodes after them; the state's tree word (+8) points at the root,
# the pool's last node.
BASE = 0x414
root = mem[8][1]
assert root == BASE + 512 * NODE and field(root, 2) == 0

codes = {}
stack = [(root, "")]
while stack:
    n, code = stack.pop()
    kids = [field(n, i) for i in (0, 1)]
    if isinstance(kids[0], tuple):
        for b, k in enumerate(kids):
            assert field(k[1], 2) == ("p", n)
            stack.append((k[1], code + str(b)))
    else:
        codes[field(n, 4)] = (code, field(n, 3))
assert sorted(codes) == list(range(257))

if mode == "--rust":
    print("pub const CODES: [&str; 257] = [")
    row = []
    for s in range(257):
        row.append('"%s",' % codes[s][0])
        if len(row) == 8 or s == 256:
            print("    " + " ".join(row))
            row = []
    print("];")
else:
    for s in range(257):
        print(s, codes[s][1], codes[s][0])
