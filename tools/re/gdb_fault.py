# gdb script for the retail CoD1 dedicated servers: where a fatal signal
# lands. On a SIGSEGV (or SIGABRT, SIGFPE) it writes the faulting pc, the
# registers and a backtrace, each frame's pc also as an offset into the
# module mapped there, to GDB_OUT (stderr unset), then the run ends.
#
#   GDB_OUT=<file> GDB_PY=tools/re/gdb_fault.py tools/run_probe.sh ...
#
# coduo-vehicles.md 17.4.1 read `freeVehicle`'s fault this way.
import gdb
import os
import sys

OUT = os.environ.get('GDB_OUT')


def module_of(pc):
    for line in gdb.execute('info proc mappings', to_string=True).splitlines():
        f = line.split()
        if len(f) >= 5 and f[0].startswith('0x'):
            lo, hi = int(f[0], 16), int(f[1], 16)
            off = int(f[3], 16)
            if lo <= pc < hi:
                path = f[-1] if len(f) >= 5 and not f[-1].startswith('0x') else '?'
                return '%s+0x%x' % (os.path.basename(path), pc - lo + off)
    return '?'


def on_stop(ev):
    if not isinstance(ev, gdb.SignalEvent):
        return
    if ev.stop_signal not in ('SIGSEGV', 'SIGABRT', 'SIGFPE', 'SIGBUS'):
        return
    w = open(OUT, 'a') if OUT else sys.stderr
    pc = int(gdb.parse_and_eval('$pc'))
    w.write('GDB fault %s pc 0x%x %s\n' % (ev.stop_signal, pc, module_of(pc)))
    w.write(gdb.execute('info registers', to_string=True))
    frame = gdb.newest_frame()
    n = 0
    while frame is not None and n < 16:
        p = frame.pc()
        w.write('GDB frame %d 0x%x %s\n' % (n, p, module_of(p)))
        frame = frame.older()
        n += 1
    w.flush()


gdb.execute('set pagination off')
gdb.execute('handle SIGPIPE nostop noprint pass')
gdb.events.stop.connect(on_stop)
