# Sourced by run_server.sh and run_probe.sh: picks the retail server, checks
# what it needs, and picks its homepath.
#
# In: COD_DIR, COD_VERSION (1.1, 1.5 or uo), COD_LNXDED, GDB_PY, PORT_USED.
# Out: EXE, GAMEDIR (main or uo), MODULE (the game module under the homepath),
# MAP_DEFAULT, BIN (the server binary), RUN (the words BIN runs under: gdb
# with GDB_PY sourced, or none), LD_LIBRARY_PATH, HOMEPATH (absolute).
#
# GDB_PY names a gdb python script for the CoD1 servers (tools/re/gdb_*.py):
# the server runs under `gdb -q -batch -ex 'source GDB_PY' -ex run --args`.
#
# The 1.5b and UO binaries link libstdc++.so.5 (gcc 3.2); the binary's lib/
# directory goes on LD_LIBRARY_PATH when it exists.
#
# COD_LNXDED_HOME, when set, is used as is. Otherwise each port gets its own
# homepath, private/homes/<basename of HOME_DEFAULT>-<port>, holding links to
# everything in HOME_DEFAULT's game dir except its logs, its archived config
# and loose maps/, so two servers on two ports never write one games_mp.log or
# load each other's probe gametypes. The archived config is deleted on every
# start: a cvar an earlier run archived (sv_hostname, scr_friendlyfire,
# sv_wwwDownload) would otherwise reach the next run's serverinfo.
# `pwd -P` resolves a worktree's private/ link to the main checkout, whose path
# has no '+' for the engine's command line to split on.
if [ -z "${COD_DIR:-}" ]; then
    echo "COD_DIR is not set; point it at the game install (the directory containing main/)" >&2
    exit 1
fi
[ -d "$COD_DIR/main" ] || { echo "COD_DIR=$COD_DIR has no main/ directory" >&2; exit 1; }
EXE=cod_lnxded GAMEDIR=main MODULE=main/game.mp.i386.so MAP_DEFAULT=mp_carentan
case "${COD_VERSION:-1.1}" in
    1.1) REF=private/retail/cod-lnxded-1.1d HOME_DEFAULT=private/server ;;
    1.5) REF=private/retail/cod-lnxded-1.5b HOME_DEFAULT=private/server-1.5 ;;
    # United Offensive's gametypes, log and module live under uo/.
    uo) REF=private/retail/coduo-lnxded-1.51b HOME_DEFAULT=private/server-uo
        EXE=coduo_lnxded GAMEDIR=uo MODULE=uo/game.mp.uo.i386.so MAP_DEFAULT=mp_cassino
        [ -d "$COD_DIR/uo" ] || { echo "COD_DIR=$COD_DIR has no uo/ directory" >&2; exit 1; } ;;
    *) echo "COD_VERSION must be 1.1, 1.5 or uo" >&2; exit 1 ;;
esac
[ -d "$REF/lib" ] && export LD_LIBRARY_PATH="$PWD/$REF/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
BIN="${COD_LNXDED:-$REF/$EXE}"
[ -x "$BIN" ] || { echo "server binary $BIN missing or not executable; set COD_LNXDED" >&2; exit 1; }
if [ ! -e /lib/ld-linux.so.2 ]; then
    echo "32-bit loader /lib/ld-linux.so.2 missing; the server needs a 32-bit glibc" >&2
    exit 1
fi
RUN=()
if [ -n "${GDB_PY:-}" ]; then
    [ -f "$GDB_PY" ] || { echo "no gdb script at $GDB_PY" >&2; exit 1; }
    RUN=(gdb -q -batch -ex "source $GDB_PY" -ex run --args)
fi
if [ -n "${COD_LNXDED_HOME:-}" ]; then
    [ -f "$COD_LNXDED_HOME/$MODULE" ] || {
        echo "no game module at $COD_LNXDED_HOME/$MODULE; fix COD_LNXDED_HOME" >&2; exit 1; }
    HOMEPATH="$(cd "$COD_LNXDED_HOME" && pwd)"
else
    [ -f "$HOME_DEFAULT/$MODULE" ] || {
        echo "no game module at $HOME_DEFAULT/$MODULE; set COD_LNXDED_HOME" >&2; exit 1; }
    _src="$(cd "$HOME_DEFAULT" && pwd -P)"
    HOMEPATH="$(cd "$_src/.." && pwd -P)/homes/$(basename "$_src")-$PORT_USED"
    mkdir -p "$HOMEPATH/$GAMEDIR"
    for _f in "$_src/$GAMEDIR"/*; do
        case "$(basename "$_f")" in
            *.log | *.cfg | maps) continue ;;
        esac
        ln -sfn "$_f" "$HOMEPATH/$GAMEDIR/"
    done
    rm -f "$HOMEPATH/$GAMEDIR"/*config_mp_server.cfg
    unset _src _f
fi
case "$HOMEPATH" in
    *+*) echo "homepath $HOMEPATH contains '+', which the engine's command line splits on;" \
        "set COD_LNXDED_HOME to a path without one" >&2; exit 1 ;;
esac
echo "retail homepath: $HOMEPATH" >&2
