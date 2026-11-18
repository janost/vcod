#!/usr/bin/env bash
# Runs one script-semantics probe on the retail 1.1d Linux dedicated server
# (1.5b with COD_VERSION=1.5) and prints what it logged, for the A/B in
# crates/gsc/tests/semantics_ab.rs.
#
# A probe is a gametype script in crates/gsc/tests/fixtures/semantics/. This
# installs it (plus the one-line .txt description the engine demands) into the
# server homepath, boots the server on a map long enough for the script to run,
# and prints every `PROBE ` line out of games_mp.log with the engine's leading
# timestamp stripped.
#
# `logPrint` is the only channel a dedicated server with no clients shows:
# `print` and `println` produce no console output even with developer 1, and
# `iPrintLn` needs a client. A script runtime error takes the whole server
# down, so a probe that dies mid-run is itself a measurement: the tail of the
# console log holds the error, and this prints it.
#
#   COD_DIR          game install, the directory containing main/ (required)
#   COD_VERSION      1.1 (default), 1.5 or uo, as tools/run_server.sh takes it
#   COD_LNXDED       server binary (default private/retail/cod-lnxded-1.1d/cod_lnxded,
#                    cod-lnxded-1.5b/cod_lnxded for 1.5)
#   COD_LNXDED_HOME  homepath holding main/game.mp.i386.so (uo/game.mp.uo.i386.so).
#                    Unset, the module comes from private/server (private/server-1.5,
#                    private/server-uo) and the run gets a homepath of its port's own,
#                    private/homes/<that name>-<port>, so concurrent probes on two
#                    ports never share a games_mp.log (tools/retail_home.sh). The
#                    path goes to stderr.
#   PORT             UDP port (default 28970, clear of run_server.sh's 28960)
#   PRE_MAP_ARGS     engine arguments placed before +map, as shell words, for
#                    what has to be set before the load (a latched cvar, sv_pure):
#                    PRE_MAP_ARGS='+set sv_pure 1 +set sv_hostname "two words"'
#   SECS             how long to let the server run (default 25)
#   PROBE_SECS       alias for SECS, so a recipe can name either
#
#   tools/run_probe.sh <probe-name> [map] [+set cvar value ...]
#     probe-name may carry a subdirectory (client-probes/probe_trigger); the
#     gametype is installed under the basename either way.
#     map defaults to mp_pavlov; anything after it is passed to the engine
#     verbatim, the way run_server.sh does. The persistence probes need
#     `+set sv_mapRotation "gametype <probe> map mp_pavlov map mp_pavlov"`,
#     which is why the pass-through exists: a probe that ends the map has
#     nothing to load next without it.
set -euo pipefail
cd "$(dirname "$0")/.."
PROBE="${1:?usage: tools/run_probe.sh <probe-name> [map]}"
# A client-probe lives in a subdirectory; the engine takes a bare gametype name.
NAME="$(basename "$PROBE")"
MAP="${2:-mp_pavlov}"
# Everything past the map goes to the engine; ${@:3} is empty when there is none.
ENGINE_ARGS=("${@:3}")
SRC="crates/gsc/tests/fixtures/semantics/$PROBE.gsc"
[ -f "$SRC" ] || { echo "no probe at $SRC" >&2; exit 1; }
if [ -z "${COD_DIR:-}" ]; then
    echo "COD_DIR is not set; point it at the game install (the directory containing main/)" >&2
    exit 1
fi
[ -d "$COD_DIR/main" ] || { echo "COD_DIR=$COD_DIR has no main/ directory" >&2; exit 1; }
EXE=cod_lnxded GAMEDIR=main MODULE=game.mp.i386.so
case "${COD_VERSION:-1.1}" in
    1.1) REF=private/retail/cod-lnxded-1.1d HOME_DEFAULT=private/server ;;
    1.5) REF=private/retail/cod-lnxded-1.5b HOME_DEFAULT=private/server-1.5
         export LD_LIBRARY_PATH="$PWD/$REF/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
    # United Offensive's gametypes, log and module live under uo/.
    uo) REF=private/retail/coduo-lnxded-1.51b HOME_DEFAULT=private/server-uo
        EXE=coduo_lnxded GAMEDIR=uo MODULE=game.mp.uo.i386.so
        export LD_LIBRARY_PATH="$PWD/$REF/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
    *) echo "COD_VERSION must be 1.1, 1.5 or uo" >&2; exit 1 ;;
esac
BIN="${COD_LNXDED:-$REF/$EXE}"
[ -x "$BIN" ] || { echo "server binary $BIN missing or not executable; set COD_LNXDED" >&2; exit 1; }
if [ ! -e /lib/ld-linux.so.2 ]; then
    echo "32-bit loader /lib/ld-linux.so.2 missing; the server needs a 32-bit glibc" >&2
    exit 1
fi
PORT_USED="${PORT:-28970}"
MODULE="$GAMEDIR/$MODULE"
. tools/retail_home.sh
# Parsed as shell words, so a quoted value with spaces survives; xargs
# splits them without eval.
mapfile -t PRE_ARGS < <(printf '%s' "${PRE_MAP_ARGS:-}" | xargs -r printf '%s\n')

# Loose files under the homepath shadow nothing in the paks: no stock gametype
# is named like a probe. The .txt is the description file the engine warns
# about and then refuses to load the map without.
GT="$HOMEPATH/$GAMEDIR/maps/mp/gametypes"
mkdir -p "$GT"
# The engine keeps only the first 31 loose gametype scripts and falls back to
# dm for anything past that without saying so on the probe's own channel, so
# probes left behind by earlier runs eventually crowd out the one being
# installed. Only probe_* files are ours to remove.
rm -f "$GT"/probe_*.gsc "$GT"/probe_*.txt
cp "$SRC" "$GT/$NAME.gsc"
printf '"%s"\r\n' "$(echo "$NAME" | tr '[:lower:]' '[:upper:]')" > "$GT/$NAME.txt"

LOG="$HOMEPATH/$GAMEDIR/games_mp.log"
CONSOLE="$(mktemp)"
trap 'rm -f "$CONSOLE"' EXIT
rm -f "$LOG"

timeout "${PROBE_SECS:-${SECS:-25}}" "$BIN" \
    +set dedicated 1 \
    +set developer 1 \
    +set logfile 2 \
    +set g_log games_mp.log \
    +set g_logSync 1 \
    +set fs_basepath "$COD_DIR" \
    +set fs_homepath "$HOMEPATH" \
    +set net_port "$PORT_USED" \
    +set sv_maxclients 8 \
    +set sv_pure 0 \
    +set g_gametype "$NAME" \
    "${PRE_ARGS[@]}" \
    +map "$MAP" \
    "${ENGINE_ARGS[@]}" > "$CONSOLE" 2>&1 || true

# The engine stamps each log line with an elapsed "m:ss ".
sed -n 's/^[[:space:]]*[0-9]*:[0-9]* \(PROBE .*\)$/\1/p' "$LOG" 2>/dev/null || true

# A G_Error / Com_Error drop (ERR_DROP) prints one `ERROR: ` line; the
# text after it starts with a 0x15 byte, dropped here.
sed -n 's/^ERROR: \(.*\)$/PROBE_ERROR \1/p' "$CONSOLE" | tr -d '\025'

if grep -q "script runtime error" "$CONSOLE"; then
    echo "PROBE_FATAL"
    sed -n '/script runtime error/,/\*\{20\}/p' "$CONSOLE" | sed 's/^/PROBE_ERR /'
elif [ ! -s "$LOG" ] && ! grep -q '^ERROR: ' "$CONSOLE"; then
    # No game log and no error: the server never got as far as the level (a
    # missing library, a bound port, a missing map). An empty section here
    # would read as a probe that logged nothing.
    echo "run_probe: the server wrote no $LOG; its console ends:" >&2
    tail -n 20 "$CONSOLE" >&2
    exit 1
fi
