#!/usr/bin/env bash
# Runs one script-semantics probe on the retail CoD2 1.3 Linux server and
# prints its `PROBE` lines, tools/run_probe.sh's job for CoD2: the gametype
# runs at the map load and logs through logPrint, no client joins. The server
# runs headless inside a loopback-only network namespace (`unshare -rn`).
#
#   COD2_DIR      the CoD2 1.3 install, holding main/ (required)
#   COD2_LNXDED   the 1.3 Linux dedicated server binary (required)
#   COD2_LIB      directory with libstdc++.so.5 (default
#                 private/retail/cod-lnxded-1.5b/lib)
#   COD2_HOME     server homepath (default private/homes/cod2-gsc); no `+`
#                 in it: the engine splits its command line on `+`
#   PORT          the server's port (default 28970)
#   PROBE_UNTIL   ERE: stop once games_mp.log has a matching line (default
#                 `PROBE [a-z_]*done`); SECS (default 25) bounds the run
#
#   tools/run_probe_cod2_gsc.sh <probe.gsc> [map]
set -euo pipefail
cd "$(dirname "$0")/.."
SRC="${1:?usage: run_probe_cod2_gsc.sh <probe.gsc> [map]}"
MAP="${2:-mp_toujane}"
: "${COD2_DIR:?point COD2_DIR at the CoD2 1.3 install}"
: "${COD2_LNXDED:?point COD2_LNXDED at cod2_lnxded}"
[ -f "$SRC" ] || { echo "no probe at $SRC" >&2; exit 1; }
LIB="$(realpath "${COD2_LIB:-private/retail/cod-lnxded-1.5b/lib}")"
HOMEP="$(realpath -m "${COD2_HOME:-private/homes/cod2-gsc}")"
case "$HOMEP" in *+*) echo "COD2_HOME must not contain +" >&2; exit 1 ;; esac
NAME="$(basename "$SRC" .gsc)"
GT_DIR="$HOMEP/main/maps/mp/gametypes"
mkdir -p "$GT_DIR"
rm -f "$GT_DIR"/probe_*.gsc "$GT_DIR"/probe_*.txt "$HOMEP/main/games_mp.log"
cp "$SRC" "$GT_DIR/$NAME.gsc"
echo "\"$NAME\"" > "$GT_DIR/$NAME.txt"
export COD2_DIR COD2_LNXDED LIB HOMEP MAP NAME
export PORT="${PORT:-28970}" SECS="${SECS:-25}"
export PROBE_UNTIL="${PROBE_UNTIL:-PROBE [a-z_]*done}"
CONSOLE="$HOMEP/server.log"
export CONSOLE
# The inner shell is its pid namespace's init: the server goes with it.
unshare -rn --pid --fork --kill-child --mount-proc bash -c '
ip link set lo up
LOG="$HOMEP/main/games_mp.log"
LD_LIBRARY_PATH="$LIB" "$COD2_LNXDED" +set fs_basepath "$COD2_DIR" +set fs_homepath "$HOMEP" \
  +set net_ip 127.0.0.1 +set net_port $PORT +set dedicated 1 +set sv_punkbuster 0 \
  +set developer 1 +set logfile 1 +set g_log games_mp.log +set g_logSync 1 \
  +set g_gametype "$NAME" +map "$MAP" > "$CONSOLE" 2>&1 &
SERVER=$!
for _ in $(seq $((SECS * 5))); do
  kill -0 $SERVER 2>/dev/null || break
  [ -f "$LOG" ] && grep -qaE -- "$PROBE_UNTIL" "$LOG" && { sleep 0.3; break; }
  sleep 0.2
done
kill $SERVER 2>/dev/null || true
wait $SERVER 2>/dev/null || true
'
# The engine stamps each log line with an elapsed "m:ss ".
sed -n 's/^[[:space:]]*[0-9]*:[0-9]* \(PROBE .*\)$/\1/p' "$HOMEP/main/games_mp.log" 2>/dev/null || true
if grep -q "script runtime error\|script compile error" "$CONSOLE"; then
  echo "PROBE_FATAL"
  grep -A12 "script runtime error\|script compile error" "$CONSOLE" | sed 's/^/PROBE_ERR /'
fi
