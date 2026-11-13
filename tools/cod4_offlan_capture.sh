#!/usr/bin/env bash
# Measures a CoD4 server's message pacing off the LAN (cod4-server-frame.md
# 8). Rootless: a user, net and mount namespace holds the server at
# 203.0.113.1, a child net namespace holds cod4_probe at 198.51.100.2, a
# veth pair joins them, so neither address is private or shares three octets
# with the other. A hosts file maps cod4master.activision.com to 127.0.0.2,
# where a stand-in authorize server answers every `getIpAuthorize` with
# `accept`, so retail lets the probe in at once (cod4-handshake.md 15).
# Nothing leaves the host.
#
#   tools/cod4_offlan_capture.sh retail|ours OUTDIR [SVFPS:SNAPS ...]
#     retail runs $COD4_LNXDED (the 1.7 package's cod4_lnxded) on
#     $COD4_DIR's data, ours target/debug/vcod-server on it. Each SVFPS:SNAPS pair
#     (default 20:20 40:20 30:20 40:30 20:30) sets sv_fps over rcon and joins
#     a spectator for --secs (OFFLAN_SECS, 12) with that snaps, its datagrams
#     in OUTDIR/probe-SVFPS-SNAPS.txt (cod4_probe's tap format).
#     OFFLAN_STOP=AT:FOR runs each join through tools/udp_relay.py
#     (OUTDIR/relay-SVFPS-SNAPS.log) and stops the probe AT s in for FOR s.
#     Build the cod4_probe example (and vcod-server for ours) first.
set -u
if [ -z "${OFFLAN_INNER:-}" ]; then
    exec env OFFLAN_INNER=1 unshare -rnm --fork bash "$0" "$@"
fi
cd "$(dirname "$0")/.."
KIND=$1; OUT=$(realpath -m "$2"); shift 2
PAIRS=("$@"); [ ${#PAIRS[@]} -eq 0 ] && PAIRS=(20:20 40:20 30:20 40:30 20:30)
SECS=${OFFLAN_SECS:-12}
PORT=28960; PW=offlan
mkdir -p "$OUT/home"
printf '127.0.0.1 localhost\n127.0.0.2 cod4master.activision.com\n' > "$OUT/hosts"
mount --bind "$OUT/hosts" /etc/hosts
mount -t tmpfs none /run/systemd/resolve 2>/dev/null
ip link set lo up
unshare -n sleep 3600 & BPID=$!
sleep 0.5
ip link add veth0 type veth peer name veth1 netns $BPID
ip addr add 203.0.113.1/24 dev veth0; ip link set veth0 up
ip route add 198.51.100.0/24 dev veth0
nsenter -t $BPID -n sh -c 'ip link set lo up; ip addr add 198.51.100.2/24 dev veth1;
    ip link set veth1 up; ip route add 203.0.113.0/24 dev veth1'

# The authorize server: `ipAuthorize <challenge> accept KEY_IS_GOOD 0 <hex>`,
# the hex the slot holds (SV_AuthorizeIpPacket, 0x816fd62).
python3 -I -c '
import socket, sys, time
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(("127.0.0.2", 20800))
h = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); h.bind(("127.0.0.2", 20810))
import select
log = open(sys.argv[1], "w", buffering=1); t0 = time.monotonic()
while True:
    for r in select.select([s, h], [], [])[0]:
        d, a = r.recvfrom(4096)
        log.write(f"{(time.monotonic()-t0)*1000:.0f} {r.getsockname()[1]} {a} {d[4:]!r}\n")
        w = d[4:].decode("latin1").split()
        if r is s and w and w[0] == "getIpAuthorize":
            hexkey = d.decode("latin1").rsplit("\"", 2)[-2] if d.count(b"\"") >= 4 else ""
            s.sendto(b"\xff\xff\xff\xffipAuthorize %s accept KEY_IS_GOOD 0 %s" % (w[1].encode(), hexkey.encode()), a)
' "$OUT/auth.log" & APID=$!

if [ "$KIND" = retail ]; then
    # The binary and its two libraries beside the full install's iwds and
    # zones (cod4-handshake.md 1).
    PKG=$(dirname "${COD4_LNXDED:?set COD4_LNXDED to cod4_lnxded}")
    TREE=$OUT/tree; mkdir -p "$TREE/main"
    for f in cod4_lnxded libgcc_s.so.1 libstdc++.so.6; do ln -sfn "$PKG/$f" "$TREE/$f"; done
    for f in "${COD4_DIR:?}"/main/*.iwd; do ln -sfn "$f" "$TREE/main/"; done
    ln -sfn "$COD4_DIR/zone" "$TREE/zone"
    (cd "$TREE" && LD_LIBRARY_PATH=. exec ./cod4_lnxded +set fs_basepath "$TREE" \
        +set fs_homepath "$OUT/home" +set net_ip 203.0.113.1 +set net_port $PORT \
        +set sv_punkbuster 0 +set sv_pure 0 +set logfile 2 +set sv_maxclients 8 \
        +set rcon_password $PW +set scr_war_timelimit 0 +map mp_crash \
        > "$OUT/server.log" 2>&1) & SPID=$!
    sleep 30
else
    target/debug/vcod-server mp_crash --game cod4 --game-dir "${COD4_DIR:?}" --port $PORT --set sv_pure=0 \
        --set rcon_password=$PW --set scr_war_timelimit=0 > "$OUT/server.log" 2>&1 & SPID=$!
    sleep 15
fi
rcon() {
    python3 -I -c '
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(1)
s.sendto(b"\xff\xff\xff\xffrcon %s %s" % (sys.argv[1].encode(), sys.argv[2].encode()), ("203.0.113.1", int(sys.argv[3])))
try: print(s.recv(4096)[4:].decode("latin1", "replace"))
except OSError: print("(no rcon answer)")
' "$PW" "$1" "$PORT"
}
for pair in "${PAIRS[@]}"; do
    fps=${pair%:*}; snaps=${pair#*:}
    rcon "sv_fps $fps" >> "$OUT/rcon.log"
    sleep 1
    if [ -n "${OFFLAN_STOP:-}" ]; then
        # The probe through tools/udp_relay.py, stopped AT s after the join
        # for FOR s: what the server sends a client that acks nothing.
        at=${OFFLAN_STOP%:*}; for=${OFFLAN_STOP#*:}
        nsenter -t $BPID -n python3 -I tools/udp_relay.py 40000 203.0.113.1 $PORT \
            "$OUT/relay-$fps-$snaps.log" $((at + for + SECS + 10)) & RP=$!
        sleep 0.5
        nsenter -t $BPID -n target/debug/examples/cod4_probe 127.0.0.1:40000 \
            "$OUT/probe-$fps-$snaps.txt" --team spectator --secs $((at + for + SECS)) \
            --snaps "$snaps" > "$OUT/probe-$fps-$snaps.log" 2>&1 & PP=$!
        sleep "$at"; kill -STOP $PP; sleep "$for"; kill -CONT $PP
        wait $PP; kill $RP; wait $RP
    else
        nsenter -t $BPID -n target/debug/examples/cod4_probe 203.0.113.1:$PORT \
            "$OUT/probe-$fps-$snaps.txt" --team spectator --secs "$SECS" --snaps "$snaps" \
            > "$OUT/probe-$fps-$snaps.log" 2>&1
    fi
    sleep 3
done
kill $SPID $APID $BPID
wait
