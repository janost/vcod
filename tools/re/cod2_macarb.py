#!/usr/bin/env python3
"""Retail CoD2's compiled shaders, as the Mac 1.3 build carries them: its
binary embeds an ARB program for each `materials/shaders/*.hlsl` file
(`<name>.vsn` and `.vsa` vertex programs, `<name>.pse` fragment programs,
each text followed by the names that use it), translated from the D3D9
bytecode. cod2-client-perf.md 8 counts them beside vcod's WGSL.

    python3 tools/re/cod2_macarb.py <cod2mp_mac13_i386> <outdir>
    python3 tools/re/cod2_macarb.py --count <file>...

The first writes each program under its names. `--count` prints, per
file, its instructions (no declarations, no `END`), the texture fetches
among them (`TEX`, `TXP`, `TXB`), and the instructions the translation adds
that the D3D shader does not hold: the fixed-function fog stand-in at a
fragment program's end (D3D's fog unit) and the `.vsa` programs' clears
of unwritten texcoords (`Null`).
"""
import os
import re
import sys

# the region of the Mac binary holding the programs and their names
LO, HI = 0x250000, 0x2CB000
DECL = ('!!ARB', 'OPTION', 'OUTPUT', 'TEMP', 'PARAM', 'ATTRIB', 'ADDRESS', 'END')


def extract(binary, out):
    data = open(binary, 'rb').read()
    os.makedirs(out, exist_ok=True)
    text, names = None, 0
    for chunk in data[LO:HI].split(b'\0'):
        try:
            s = chunk.decode('ascii')
        except UnicodeDecodeError:
            text = None
            continue
        if re.fullmatch(r'[A-Za-z0-9_]+\.(vsa|vsn|pse)', s):
            if text is not None:
                with open(os.path.join(out, s), 'w') as f:
                    f.write(text)
                names += 1
        elif s.startswith('!!ARB'):
            text = s
    print(names, 'names')


def count(path):
    instr = fetch = added = 0
    for line in open(path).read().split('\n'):
        for st in line.split(';'):
            st = st.strip()
            if not st or st.startswith(DECL):
                continue
            op = st.split()[0]
            if 'fogcoord' in st or 'oC0AfterFog' in st or 'state.fog' in st or 'r15.x' in st:
                added += 1
                continue
            if op == 'MOV' and ('Null' in st or st.startswith('MOV oFog')):
                added += 1
                continue
            instr += 1
            if op.rstrip('_SAT') in ('TEX', 'TXP', 'TXB'):
                fetch += 1
    return instr, fetch, added


def main():
    if len(sys.argv) > 1 and sys.argv[1] == '--count':
        print('file instr fetch alu added')
        for p in sys.argv[2:]:
            i, f, a = count(p)
            print(os.path.basename(p), i, f, i - f, a)
    elif len(sys.argv) == 3:
        extract(sys.argv[1], sys.argv[2])
    else:
        print(__doc__)
        sys.exit(2)


main()
