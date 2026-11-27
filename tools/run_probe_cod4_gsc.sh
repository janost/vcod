#!/usr/bin/env bash
# Runs one script-semantics probe on the retail CoD4 1.7 Linux server and
# prints its `PROBE` lines, tools/run_probe_cod2_gsc.sh's job for CoD4: the
# probe is installed as `maps/mp/gametypes/war.gsc` in an fs_game iwd, runs
# at the map load and logs through logPrint, no client joins. The server runs
# headless inside a loopback-only network namespace (`unshare -rn`).
#
# A CoD1 probe runs as is, with two changes: its `SetupCallbacks()` line is
# dropped (CoD4's would install the stock globallogic callbacks over the
# probe's `level.callback*`), and `level.script = getDvar("mapname");` goes on
# main's opening brace (stock war.gsc sets it, the map's _art.gsc reads it).
#
#   COD4_DIR         the CoD4 1.7 install, holding main/ and zone/ (required)
#   COD4_LNXDED_DIR  the retail 1.7 dedicated server's directory, holding
#                    cod4_lnxded and its libgcc_s.so.1 / libstdc++.so.6
#                    (required)
#   COD4_HOME        server homepath and scratch tree (default
#                    tmp/cod4-gsc); no `+` in it: the engine splits its
#                    command line on `+`
#   PORT             the server's port (default 28975)
#   DEVELOPER        the `developer` cvar (default 1)
#   PRE_MAP_ARGS     extra engine arguments placed before the map, e.g.
#                    `+set developer 0`; the last `developer` wins
#   PROBE_UNTIL      ERE: stop once games_mp.log has a matching line (default
#                    `PROBE [a-z_]*done`); SECS (default 25) bounds the run.
#                    An error in the probe's own file stops it too
#
#   tools/run_probe_cod4_gsc.sh <probe.gsc> [map]
set -euo pipefail
cd "$(dirname "$0")/.."
SRC="${1:?usage: run_probe_cod4_gsc.sh <probe.gsc> [map]}"
MAP="${2:-mp_crash}"
: "${COD4_DIR:?point COD4_DIR at the CoD4 1.7 install}"
: "${COD4_LNXDED_DIR:?point COD4_LNXDED_DIR at the directory holding cod4_lnxded}"
[ -f "$SRC" ] || { echo "no probe at $SRC" >&2; exit 1; }
[ -x "$COD4_LNXDED_DIR/cod4_lnxded" ] || { echo "no cod4_lnxded in COD4_LNXDED_DIR" >&2; exit 1; }
[ -d "$COD4_DIR/zone" ] || { echo "COD4_DIR has no zone/" >&2; exit 1; }
HOMEP="$(realpath -m "${COD4_HOME:-tmp/cod4-gsc}")"
case "$HOMEP" in *+*) echo "COD4_HOME must not contain +" >&2; exit 1 ;; esac
# basepath tree: the server binary beside symlinks to the install's data
TREE="$HOMEP/tree"
HOMES="$HOMEP/home"
if [ ! -x "$TREE/cod4_lnxded" ]; then
  mkdir -p "$TREE/main"
  cp "$COD4_LNXDED_DIR/cod4_lnxded" "$TREE/"
  for f in libgcc_s.so.1 libstdc++.so.6; do
    ln -sfn "$(realpath "$COD4_LNXDED_DIR/$f")" "$TREE/$f"
  done
  for f in "$COD4_DIR"/main/*.iwd; do ln -sfn "$(realpath "$f")" "$TREE/main/"; done
  ln -sfn "$(realpath "$COD4_DIR/zone")" "$TREE/zone"
fi
rm -rf "$HOMES"
mkdir -p "$HOMES/mods/vcodprobe"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/maps/mp/gametypes"
# the extra statement shares main's brace line, keeping line numbers
awk '/^main\(\)/ { m = 1 } /_callbacksetup::SetupCallbacks/ { next } m && /^\{/ { print "{ level.script = getDvar(\"mapname\");"; m = 0; next } { print }' "$SRC" \
  > "$STAGE/maps/mp/gametypes/war.gsc"
python3 -I -c 'import sys, zipfile; z = zipfile.ZipFile(sys.argv[1], "w"); z.write(sys.argv[2], "maps/mp/gametypes/war.gsc"); z.close()' \
  "$HOMES/mods/vcodprobe/z_vcodprobe.iwd" "$STAGE/maps/mp/gametypes/war.gsc"
export TREE HOMES MAP
export PORT="${PORT:-28975}" SECS="${SECS:-25}" DEVELOPER="${DEVELOPER:-1}"
export PRE_MAP_ARGS="${PRE_MAP_ARGS:-}"
export PROBE_UNTIL="${PROBE_UNTIL:-PROBE [a-z_]*done}"
CONSOLE="$HOMEP/console.log"
FATAL_AWK='c > 0 { print; c--; next }
  /script runtime error \*/ { getline a; if (a ~ /war\.gsc/) { print; print a; c = 9 } next }
  /^Error: [^:]*: \(file .maps\/mp\/gametypes\/war\.gsc/ { print; c = 3 }'
export CONSOLE FATAL_AWK
# The inner shell is its pid namespace's init: the server goes with it.
unshare -rn --pid --fork --kill-child --mount-proc bash -c '
ip link set lo up
cd "$TREE"
# PRE_MAP_ARGS is split on purpose: it is a list of engine arguments
# shellcheck disable=SC2086
LD_LIBRARY_PATH=. ./cod4_lnxded +set fs_basepath "$TREE" +set fs_homepath "$HOMES" \
  +set fs_game mods/vcodprobe +set dedicated 1 +set developer "$DEVELOPER" \
  +set logfile 2 +set g_log games_mp.log +set g_logSync 1 +set sv_pure 0 \
  +set net_ip 127.0.0.1 +set net_port "$PORT" +set g_gametype war \
  $PRE_MAP_ARGS +map "$MAP" > "$CONSOLE" 2>&1 &
SERVER=$!
for _ in $(seq $((SECS * 5))); do
  kill -0 $SERVER 2>/dev/null || break
  LOG=$(find "$HOMES" -name games_mp.log | head -1)
  [ -n "$LOG" ] && grep -qaE -- "$PROBE_UNTIL" "$LOG" && { sleep 0.3; break; }
  [ -n "$(awk "$FATAL_AWK" "$CONSOLE")" ] && { sleep 0.3; break; }
  sleep 0.2
done
kill $SERVER 2>/dev/null || true
wait $SERVER 2>/dev/null || true
'
LOG="$(find "$HOMES" -name games_mp.log | head -1)"
# The engine stamps each log line with an elapsed "m:ss ".
[ -n "$LOG" ] && sed -n 's/^[[:space:]]*[0-9]*:[0-9]* \(PROBE .*\)$/\1/p' "$LOG"
# An error in the probe's own file: a runtime error's banner (developer 1;
# below it the engine prints only a stack trace and runs on) or a compile
# error line. Stock scripts around the probe may fail after it ends.
ERR="$(awk "$FATAL_AWK" "$CONSOLE")"
if [ -n "$ERR" ]; then
  echo "PROBE_FATAL"
  echo "$ERR" | sed 's/^/PROBE_ERR /'
fi
