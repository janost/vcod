#!/usr/bin/env bash
# Runs one script-semantics probe on the retail 1.1d Linux dedicated server
# and prints what it logged, for the A/B in crates/gsc/tests/semantics_ab.rs.
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
#   COD_LNXDED       server binary (default private/reference/cod-lnxded-1.1d/cod_lnxded)
#   COD_LNXDED_HOME  homepath holding main/game.mp.i386.so (default private/server)
#   PORT             UDP port (default 28970, clear of run_server.sh's 28960)
#   SECS             how long to let the server run (default 25)
#
#   tools/run_probe.sh <probe-name> [map]    map defaults to mp_pavlov
set -euo pipefail
cd "$(dirname "$0")/.."
PROBE="${1:?usage: tools/run_probe.sh <probe-name> [map]}"
MAP="${2:-mp_pavlov}"
SRC="crates/gsc/tests/fixtures/semantics/$PROBE.gsc"
[ -f "$SRC" ] || { echo "no probe at $SRC" >&2; exit 1; }
if [ -z "${COD_DIR:-}" ]; then
    echo "COD_DIR is not set; point it at the game install (the directory containing main/)" >&2
    exit 1
fi
[ -d "$COD_DIR/main" ] || { echo "COD_DIR=$COD_DIR has no main/ directory" >&2; exit 1; }
BIN="${COD_LNXDED:-private/reference/cod-lnxded-1.1d/cod_lnxded}"
HOMEPATH="${COD_LNXDED_HOME:-private/server}"
[ -x "$BIN" ] || { echo "server binary $BIN missing or not executable; set COD_LNXDED" >&2; exit 1; }
[ -f "$HOMEPATH/main/game.mp.i386.so" ] || {
    echo "no game module at $HOMEPATH/main/game.mp.i386.so; set COD_LNXDED_HOME" >&2; exit 1; }
HOMEPATH="$(cd "$HOMEPATH" && pwd)"

# Loose files under the homepath shadow nothing in the paks: no stock gametype
# is named like a probe. The .txt is the description file the engine warns
# about and then refuses to load the map without.
GT="$HOMEPATH/main/maps/mp/gametypes"
mkdir -p "$GT"
cp "$SRC" "$GT/$PROBE.gsc"
printf '"%s"\r\n' "$(echo "$PROBE" | tr '[:lower:]' '[:upper:]')" > "$GT/$PROBE.txt"

LOG="$HOMEPATH/main/games_mp.log"
CONSOLE="$(mktemp)"
trap 'rm -f "$CONSOLE"' EXIT
rm -f "$LOG"

timeout "${SECS:-25}" "$BIN" \
    +set dedicated 1 \
    +set developer 1 \
    +set logfile 2 \
    +set g_log games_mp.log \
    +set g_logSync 1 \
    +set fs_basepath "$COD_DIR" \
    +set fs_homepath "$HOMEPATH" \
    +set net_port "${PORT:-28970}" \
    +set sv_maxclients 8 \
    +set sv_pure 0 \
    +set g_gametype "$PROBE" \
    +map "$MAP" > "$CONSOLE" 2>&1 || true

# The engine stamps each log line with an elapsed "m:ss ".
sed -n 's/^[[:space:]]*[0-9]*:[0-9]* \(PROBE .*\)$/\1/p' "$LOG" 2>/dev/null || true

if grep -q "script runtime error" "$CONSOLE"; then
    echo "PROBE_FATAL"
    sed -n '/script runtime error/,/\*\{20\}/p' "$CONSOLE" | sed 's/^/PROBE_ERR /'
fi
