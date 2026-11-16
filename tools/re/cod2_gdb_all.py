# gdb python for cod2_lnxded 1.3: cod2_trace_gdb.py, cod2_anim_gdb.py and
# cod2_skel_gdb.py in one session, each writing its own file
# (CODTRACE_OUT, CODANIM_OUT, CODSKEL_OUT). Their breakpoints slow the
# server enough that it runs frames back to back with no packet or
# snapshot between, which the antilag-slow capture records
# (cod2-player-anims.md 7). Load with `gdb -batch -x
# tools/re/cod2_gdb_all.py --args cod2_lnxded ...` (GDB_PY of
# tools/run_probe_cod2.sh).
import os

import gdb

HERE = os.path.dirname(os.path.realpath(__file__))
outs = []
for name in ("cod2_trace_gdb.py", "cod2_anim_gdb.py", "cod2_skel_gdb.py"):
    src = open(os.path.join(HERE, name)).read()
    src = src.replace('gdb.execute("run")', "").replace("OUT.close()", "")
    g = {"__name__": name}
    exec(compile(src, name, "exec"), g)
    outs.append(g["OUT"])
gdb.execute("run")
for o in outs:
    o.close()
