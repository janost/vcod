#!/usr/bin/env bash
# Launches the retail 1.1d Linux dedicated server for protocol captures and
# controlled visual tests.
#
# The server binary, cod_lnxded (the 1.1d Linux dedicated server), is not in the
# repo and has to be obtained separately. It is a 32-bit ELF and needs a 32-bit
# glibc on the host. The engine loads its game module, main/game.mp.i386.so, from
# the homepath, so the homepath must hold the 1.1 MP game module; the game install
# supplies the pk3s through fs_basepath.
#
#   COD_DIR          game install, the directory containing main/ (required)
#   COD_LNXDED       server binary (default private/reference/cod-lnxded-1.1d/cod_lnxded)
#   COD_LNXDED_HOME  homepath holding main/game.mp.i386.so (default private/server)
#   PORT             UDP port (default 28960)
#
#   tools/run_server.sh [map]    map defaults to mp_carentan
set -euo pipefail
cd "$(dirname "$0")/.."
if [ -z "${COD_DIR:-}" ]; then
    echo "COD_DIR is not set; point it at the game install (the directory containing main/)" >&2
    exit 1
fi
[ -d "$COD_DIR/main" ] || { echo "COD_DIR=$COD_DIR has no main/ directory" >&2; exit 1; }
BIN="${COD_LNXDED:-private/reference/cod-lnxded-1.1d/cod_lnxded}"
HOMEPATH="${COD_LNXDED_HOME:-private/server}"
MAP="${1:-mp_carentan}"
[ -x "$BIN" ] || { echo "server binary $BIN missing or not executable; set COD_LNXDED" >&2; exit 1; }
[ -f "$HOMEPATH/main/game.mp.i386.so" ] || {
    echo "no game module at $HOMEPATH/main/game.mp.i386.so; set COD_LNXDED_HOME" >&2; exit 1; }
if [ ! -e /lib/ld-linux.so.2 ]; then
    echo "32-bit loader /lib/ld-linux.so.2 missing; the server needs a 32-bit glibc" >&2
    exit 1
fi
HOMEPATH="$(cd "$HOMEPATH" && pwd)"
exec "$BIN" \
    +set dedicated 1 \
    +set fs_basepath "$COD_DIR" \
    +set fs_homepath "$HOMEPATH" \
    +set net_port "${PORT:-28960}" \
    +set sv_maxclients 8 \
    +set sv_pure 0 \
    +map "$MAP"
