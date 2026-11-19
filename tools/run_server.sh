#!/usr/bin/env bash
# Launches the retail 1.1d Linux dedicated server for protocol captures and
# controlled visual tests. COD_VERSION=1.5 runs the 1.5b server instead
# (protocol 6, docs/research/cod15-protocol-diff.md); COD_DIR must then be a
# 1.5 install, since a 1.1 pak0 fails the 1.5 server's checksums.
# COD_VERSION=uo runs the United Offensive 1.51b server (protocol 22,
# docs/research/coduo-diff.md); COD_DIR must hold the 1.5 main/ and the UO
# uo/ paks, and the default map becomes mp_cassino.
#
# The server binary, cod_lnxded (the 1.1d Linux dedicated server), is not in the
# repo and has to be obtained separately. It is a 32-bit ELF and needs a 32-bit
# glibc on the host. The engine loads its game module, main/game.mp.i386.so, from
# the homepath, so the homepath must hold the 1.1 MP game module; the game install
# supplies the pk3s through fs_basepath.
#
#   COD_DIR          game install, the directory containing main/ (required)
#   COD_VERSION      1.1 (default), 1.5 or uo
#   COD_LNXDED       server binary (default private/retail/cod-lnxded-1.1d/cod_lnxded,
#                    cod-lnxded-1.5b/cod_lnxded for 1.5,
#                    coduo-lnxded-1.51b/coduo_lnxded for uo)
#   GDB_PY           a gdb python script the server runs under (tools/re/gdb_*.py)
#   COD_LNXDED_HOME  homepath holding main/game.mp.i386.so, or uo/game.mp.uo.i386.so.
#                    Unset, the module comes from private/server (private/server-1.5,
#                    private/server-uo) and the server runs in a homepath of its port's
#                    own, private/homes/<that name>-<port>, with a fresh archived
#                    config each start (tools/retail_home.sh); games_mp.log is there.
#
# The 1.5b and UO binaries link libstdc++.so.5 (gcc 3.2), which current
# distributions no longer ship; the script puts the binary's lib/ directory
# (private/retail/cod-lnxded-1.5b/lib, coduo-lnxded-1.51b/lib) on
# LD_LIBRARY_PATH, so drop a 32-bit copy there (Debian's libstdc++5 i386).
#   PORT             UDP port (default 28960)
#   PRE_MAP_ARGS     engine arguments placed before +map, as shell words; the ones
#                    after the map land after the load, too late for sv_pure 1
#
#   tools/run_server.sh [map] [+set cvar value ...]
#     map defaults to mp_carentan (mp_cassino for uo); anything after it is passed to the engine
#     verbatim, which takes its +set arguments in any order. The tdm hit
#     capture uses `+set g_gametype tdm +set scr_friendlyfire 1`.
set -euo pipefail
cd "$(dirname "$0")/.."
PORT_USED="${PORT:-28960}"
. tools/retail_home.sh
MAP="${1:-$MAP_DEFAULT}"
# Split as shell words without eval: xargs honours the quotes.
mapfile -t PRE_ARGS < <(printf '%s' "${PRE_MAP_ARGS:-}" | xargs -r printf '%s\n')
exec "${RUN[@]}" "$BIN" \
    +set dedicated 1 \
    +set fs_basepath "$COD_DIR" \
    +set fs_homepath "$HOMEPATH" \
    +set net_port "$PORT_USED" \
    +set sv_maxclients 8 \
    +set sv_pure 0 \
    +set sv_allowDownload 1 \
    "${PRE_ARGS[@]}" \
    +map "$MAP" \
    "${@:2}"
