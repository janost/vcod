#!/usr/bin/env bash
# Measures message pacing off the LAN, which neither server applies to a
# loopback or private address. Rootless: a user + network namespace holds the
# server at 203.0.113.1, a child namespace holds the probe at 198.51.100.2, a
# veth pair joins them, and tools/udp_relay.py (beside the probe) logs every
# datagram with its time and size. Nothing leaves the host.
#
#   tools/offlan_capture.sh retail|ours OUTDIR [RATE:SNAPS ...]
#     retail runs tools/run_server.sh (same env: COD_DIR, COD_LNXDED,
#     COD_LNXDED_HOME) on port 30182, ours target/release/vcod-server on 30184
#     (OFFLAN_PORT overrides either).
#     Each RATE:SNAPS pair (default 5000:20 25000:10 1000:30) is one 25 s
#     spectator join through the relay, logged to OUTDIR/relay-RATE-SNAPS.log.
#     Build target/release/vcod (and vcod-server for ours) first.
# Read in docs/research/cod11-server-handshake.md, "Message pacing".
set -u
if [ -z "${OFFLAN_INNER:-}" ]; then
    exec env OFFLAN_INNER=1 unshare -rn --fork bash "$0" "$@"
fi
cd "$(dirname "$0")/.."
KIND=$1; OUT=$(realpath -m "$2"); shift 2
PAIRS=("$@"); [ ${#PAIRS[@]} -eq 0 ] && PAIRS=(5000:20 25000:10 1000:30)
mkdir -p "$OUT"
ip link set lo up
unshare -n sleep 3600 & BPID=$!
sleep 0.5
ip link add veth0 type veth peer name veth1 netns $BPID
ip addr add 203.0.113.1/24 dev veth0; ip link set veth0 up
ip route add 198.51.100.0/24 dev veth0
nsenter -t $BPID -n sh -c 'ip link set lo up; ip addr add 198.51.100.2/24 dev veth1;
    ip link set veth1 up; ip route add 203.0.113.0/24 dev veth1'
if [ "$KIND" = retail ]; then
    PORT=${OFFLAN_PORT:-30182}
    PORT=$PORT tools/run_server.sh mp_carentan > "$OUT/server.log" 2>&1 & SPID=$!
else
    PORT=${OFFLAN_PORT:-30184}
    # sv_pure 0, as tools/run_server.sh runs retail's.
    target/release/vcod-server mp_carentan --port $PORT --set sv_pure=0 > "$OUT/server.log" 2>&1 & SPID=$!
fi
sleep 10
for pair in "${PAIRS[@]}"; do
    rate=${pair%:*}; snaps=${pair#*:}
    nsenter -t $BPID -n python3 -I tools/udp_relay.py 40000 203.0.113.1 $PORT \
        "$OUT/relay-$rate-$snaps.log" 32 & RP=$!
    sleep 0.5
    nsenter -t $BPID -n target/release/vcod --net-probe 127.0.0.1:40000 \
        --probe-rate "$rate" --probe-snaps "$snaps" --probe-secs 25 \
        > "$OUT/probe-$rate-$snaps.log" 2>&1
    wait $RP
    sleep 3
done
kill $SPID $BPID
wait
