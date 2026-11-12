#!/usr/bin/env bash
# One CoD2 movement capture: the retail cod2_lnxded with the probe_moves
# gametype (crates/cod2/tests/fixtures/moves/probe_moves.gsc) spawning every
# client at a fixed spot, and `move_probe` playing one script against it,
# both inside a loopback-only network namespace (`unshare -rn`: the retail
# server heartbeats whatever `dedicated` says).
#
#   COD2_DIR      the CoD2 1.3 install, holding main/ (required)
#   COD2_LNXDED   the 1.3 Linux dedicated server binary (required)
#   COD2_LIB      directory with libstdc++.so.5 (default
#                 private/reference/cod-lnxded-1.5b/lib)
#   COD2_HOME     server homepath (default private/homes/cod2-moves); the
#                 gametype goes into its main/ as loose files, and
#                 games_mp.log and console.log land there. No `+` in it: the
#                 engine splits its command line on `+`.
#   SRV           extra engine arguments, placed before +map
#   GT            the gametype: `moves` (default, probe_moves.gsc) or
#                 `combat` (crates/cod2/tests/fixtures/combat/probe_combat.gsc)
#   SECOND        a second move_probe script, joined 2 s before the first
#                 (so it takes client 0) and written to <out>-b.txt
#   PROBE_GSC     a gsc file installed as the gametype in place of the GT
#                 one (crates/cod2/tests/fixtures/turret/probe_turret.gsc puts
#                 the player behind the turret nearest x y z)
#   PROBE_ARGS    extra move_probe arguments for the first script's client
#                 (`--lag 100` holds its cmd clock 100 ms back)
#   RAND_TRACE    1 loads tools/re/cod2_rand_trace.c (built into the
#                 homepath, gcc -m32) into the server, which appends every
#                 srand() and rand() it made to <out> as `# srand` and
#                 `# rand` lines (cod2-combat.md 9.6)
#
#   tools/run_probe_cod2.sh <script> <map> <x> <y> <z> <yaw> <out.txt>
#
# Build first: cargo build -p vcod-cod2 --example move_probe
set -euo pipefail
cd "$(dirname "$0")/.."
SCRIPT="${1:?usage: run_probe_cod2.sh <script> <map> <x> <y> <z> <yaw> <out>}"
MAP="$2" X="$3" Y="$4" Z="$5" YAW="$6" OUT="$7"
: "${COD2_DIR:?point COD2_DIR at the CoD2 1.3 install}"
: "${COD2_LNXDED:?point COD2_LNXDED at cod2_lnxded}"
LIB="$(realpath "${COD2_LIB:-private/reference/cod-lnxded-1.5b/lib}")"
HOMEP="$(realpath -m "${COD2_HOME:-private/homes/cod2-moves}")"
PROBE="$PWD/target/debug/examples/move_probe"
[ -x "$PROBE" ] || { echo "build move_probe first" >&2; exit 1; }
case "$HOMEP" in *+*) echo "COD2_HOME must not contain +" >&2; exit 1 ;; esac
GT_DIR="$HOMEP/main/maps/mp/gametypes"
mkdir -p "$GT_DIR"
rm -f "$HOMEP/main/games_mp.log" "$HOMEP/main/console_mp.log"
GTNAME="probe_${GT:-moves}"
cp "${PROBE_GSC:-crates/cod2/tests/fixtures/${GT:-moves}/$GTNAME.gsc}" "$GT_DIR/$GTNAME.gsc"
echo "\"$GTNAME\"" > "$GT_DIR/$GTNAME.txt"
OUT="$(realpath -m "$OUT")"
LOG="$HOMEP/server.log"
export COD2_DIR COD2_LNXDED LIB HOMEP PROBE MAP X Y Z YAW OUT LOG SCRIPT GTNAME
export SECOND="${SECOND:-}"
export SRV="${SRV:-}"
export PROBE_ARGS="${PROBE_ARGS:-}"
PRELOAD=""
if [ "${RAND_TRACE:-}" = 1 ]; then
  PRELOAD="$HOMEP/cod2_rand_trace.so"
  gcc -m32 -shared -fPIC -O2 -o "$PRELOAD" tools/re/cod2_rand_trace.c -ldl
  rm -f "$HOMEP/rand.log"
fi
export PRELOAD
unshare -rn bash -c '
ip link set lo up
LD_LIBRARY_PATH="$LIB" LD_PRELOAD="$PRELOAD" RAND_TRACE_OUT="$HOMEP/rand.log" "$COD2_LNXDED" +set fs_basepath "$COD2_DIR" +set fs_homepath "$HOMEP" \
  +set net_ip 127.0.0.1 +set net_port 28970 +set dedicated 1 +set sv_punkbuster 0 \
  +set developer 1 +set logfile 1 +set g_log games_mp.log +set g_logSync 1 +set sv_maxclients 4 \
  +set probe_x "$X" +set probe_y "$Y" +set probe_z "$Z" +set probe_yaw "$YAW" \
  +set g_gametype $GTNAME $SRV +map "$MAP" > "$LOG" 2>&1 &
SERVER=$!
for _ in $(seq 60); do
  grep -q "Hunk_Clear\|Sys_Error\|Server: " "$LOG" 2>/dev/null && break
  sleep 0.5
done
sleep 3
status=0
if [ -n "$SECOND" ]; then
  "$PROBE" 127.0.0.1:28970 --script "$SECOND" --name target --out "${OUT%.txt}-b.txt" --game-dir "$COD2_DIR" &
  B=$!
  sleep 2
fi
"$PROBE" 127.0.0.1:28970 --script "$SCRIPT" --out "$OUT" --game-dir "$COD2_DIR" $PROBE_ARGS || status=$?
[ -n "$SECOND" ] && { wait $B || status=$?; }
sleep 0.5
kill $SERVER 2>/dev/null; sleep 1; kill -9 $SERVER 2>/dev/null || true
# The gametype'"'"'s PROBE log lines ride the end of the capture as comments.
grep -a PROBE "$HOMEP/main/games_mp.log" 2>/dev/null | sed "s/^/# /" | tee -a "$OUT" || true
[ -n "$PRELOAD" ] && sed "s/^/# /" "$HOMEP/rand.log" >> "$OUT"
exit $status
'
# The fixture's header: what the gate needs (the map) and how to remake it.
HOW="${RAND_TRACE:+RAND_TRACE=$RAND_TRACE }${GT:+GT=$GT }${SECOND:+SECOND=$SECOND }${PROBE_GSC:+PROBE_GSC=$PROBE_GSC }${PROBE_ARGS:+PROBE_ARGS='$PROBE_ARGS' }tools/run_probe_cod2.sh $SCRIPT $MAP $X $Y $Z $YAW <out>"
sed -i "1i # map $MAP\\n# made by: $HOW (retail cod2_lnxded 1.3, pure, defaults${SRV:+, $SRV})" "$OUT"
if [ -n "${SECOND:-}" ] && [ -f "${OUT%.txt}-b.txt" ]; then
  sed -i "1i # map $MAP\\n# made by: $HOW, the second client (retail cod2_lnxded 1.3, pure, defaults${SRV:+, $SRV})" "${OUT%.txt}-b.txt"
fi
