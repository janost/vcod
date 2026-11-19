#!/usr/bin/env bash
# Measures message pacing off the LAN, which neither server applies to a
# loopback or private address. Rootless: a user + network namespace holds the
# server at 203.0.113.1, a child namespace holds the probe at 198.51.100.2, a
# veth pair joins them, and tools/udp_relay.py (beside the probe) logs every
# datagram with its time and size. Nothing leaves the host.
#
#   tools/offlan_capture.sh retail|ours OUTDIR [RATE:SNAPS ...]
#     retail runs tools/run_server.sh (same env: COD_DIR, COD_LNXDED,
#     COD_LNXDED_HOME) on port 30182, ours target/debug/vcod-server on 30184
#     (OFFLAN_PORT overrides either).
#     Each RATE:SNAPS pair (default 5000:20 25000:10 1000:30) is one 25 s
#     spectator join through the relay, logged to OUTDIR/relay-RATE-SNAPS.log.
#     Build target/debug/vcod (and vcod-server for ours) first, as the
#     CoD4 script does.
# Read in docs/research/cod11-server-handshake.md, "Message pacing".
set -u
. "$(dirname "$0")/lib.sh"
if [ -z "${OFFLAN_INNER:-}" ]; then
    need unshare nsenter ip python3
    need_built "$(dirname "$0")/../target/debug/vcod" "cargo build -p vcod"
    [ "${1:-}" = retail ] || need_built "$(dirname "$0")/../target/debug/vcod-server" "cargo build -p vcod-server"
    # A pid namespace whose init is the inner half: when it ends, or the
    # PID a caller captured (this unshare) is killed, every process of the
    # run goes with it. TERM and INT reach the inner half's trap.
    exec env OFFLAN_INNER=1 unshare -rn --pid --fork --kill-child --mount-proc \
        --forward-signals bash "$0" "$@"
fi
offlan_inner
trap 'exit 143' TERM INT
cd "$(dirname "$0")/.."
KIND=$1; OUT=$(realpath -m "$2"); shift 2
PAIRS=("$@"); [ ${#PAIRS[@]} -eq 0 ] && PAIRS=(5000:20 25000:10 1000:30)
mkdir -p "$OUT"
offlan_veth
if [ "$KIND" = retail ]; then
    PORT=${OFFLAN_PORT:-30182}
    PORT=$PORT tools/run_server.sh mp_carentan > "$OUT/server.log" 2>&1 & SPID=$!
else
    PORT=${OFFLAN_PORT:-30184}
    # sv_pure 0, as tools/run_server.sh runs retail's.
    target/debug/vcod-server mp_carentan --port $PORT --set sv_pure=0 > "$OUT/server.log" 2>&1 & SPID=$!
fi
sleep 10
for pair in "${PAIRS[@]}"; do
    rate=${pair%:*}; snaps=${pair#*:}
    nsenter -t $BPID -n python3 -I tools/udp_relay.py 40000 203.0.113.1 $PORT \
        "$OUT/relay-$rate-$snaps.log" 32 & RP=$!
    sleep 0.5
    nsenter -t $BPID -n target/debug/vcod --net-probe 127.0.0.1:40000 \
        --probe-rate "$rate" --probe-snaps "$snaps" --probe-secs 25 \
        > "$OUT/probe-$rate-$snaps.log" 2>&1
    wait $RP
    sleep 3
done
kill $SPID $BPID
wait
