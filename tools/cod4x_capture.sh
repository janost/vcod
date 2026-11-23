#!/usr/bin/env bash
# Runs a CoD4x dedicated server release binary on loopback and captures every
# packet on `lo` while a client joins it (cod4x-protocol.md 1). Rootless: a
# user, net, mount and pid namespace holds everything, a hosts file maps the
# Activision and CoD4x master and update names to 127.0.0.2, and the install
# is bind-mounted read-only, since the server's main/ links into it. Nothing
# leaves the host.
#
#   tools/cod4x_capture.sh SERVER_BIN OUTDIR [CLIENT_CMD]
#     SERVER_BIN  the release's cod4x18_dedrun (or the 1.7 package's
#                 cod4_lnxded, for the same run against retail)
#     OUTDIR      gets tree/, home/, server.log, lo.txt (tools/lo_capture.py's
#                 format, server port as c2s/s2c) and client.log
#     CLIENT_CMD  run through bash once the map is up, with $ADDR set to
#                 127.0.0.1:$PORT; empty leaves the server up CAP_SECS
#   PORT (28990), MAP (mp_crash), CAP_SECS (20), SERVER_SETS (extra
#   `+set name value` words, before +map), COD4_DIR.
set -u
. "$(dirname "$0")/lib.sh"
if [ -z "${COD4X_INNER:-}" ]; then
    need unshare ip python3
    [ $# -ge 2 ] || { sed -n 2,18p "$0"; exit 2; }
    exec env COD4X_INNER=1 unshare -rnm --pid --fork --kill-child --mount-proc \
        --forward-signals bash "$0" "$@"
fi
offlan_inner
trap 'exit 143' TERM INT
TOOLS=$(cd "$(dirname "$0")" && pwd)
BIN=$(realpath "$1"); OUT=$(realpath -m "$2"); CLIENT=${3:-}
PORT=${PORT:-28990}; MAP=${MAP:-mp_crash}; SECS=${CAP_SECS:-20}
: "${COD4_DIR:?set COD4_DIR to the 1.7 install}"
mkdir -p "$OUT/home" "$OUT/tree/main"

ip link set lo up
ip addr add 127.0.0.2/8 dev lo 2>/dev/null
printf '127.0.0.1 localhost\n' > "$OUT/hosts"
for h in cod4master.activision.com cod4update.cod4x.ovh cod4master.cod4x.ovh \
    raw.githubusercontent.com cod4x.ovh; do
    printf '127.0.0.2 %s\n' "$h" >> "$OUT/hosts"
done
mount --bind "$OUT/hosts" /etc/hosts
mount -t tmpfs none /run/systemd/resolve 2>/dev/null
# The server writes under its basepath's main/; keep it off the install.
mount --bind "$COD4_DIR" "$COD4_DIR" && mount -o remount,bind,ro "$COD4_DIR"

TREE=$OUT/tree
cp -f "$BIN" "$TREE/"; EXE=$(basename "$BIN"); chmod 755 "$TREE/$EXE"
for f in libgcc_s.so.1 libstdc++.so.6; do
    [ -e "$(dirname "$BIN")/$f" ] && ln -sfn "$(dirname "$BIN")/$f" "$TREE/$f"
done
for f in "$COD4_DIR"/main/*.iwd; do ln -sfn "$f" "$TREE/main/"; done
ln -sfn "$COD4_DIR/zone" "$TREE/zone"

python3 -I "$TOOLS/lo_capture.py" "$OUT/lo.txt" "$PORT" & CPID=$!
# shellcheck disable=SC2086
(cd "$TREE" && LD_LIBRARY_PATH=. exec "./$EXE" +set fs_basepath "$TREE" \
    +set fs_homepath "$OUT/home" +set net_ip 127.0.0.1 +set net_port "$PORT" \
    +set dedicated 2 +set sv_punkbuster 0 +set logfile 2 +set sv_maxclients 8 \
    +set developer 0 ${SERVER_SETS:-} +map "$MAP" < /dev/null > "$OUT/server.log" 2>&1) & SPID=$!
# Up once it answers getinfo (the map is loaded by then).
python3 -I -c '
import socket, subprocess, sys, time
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(1)
for _ in range(120):
    if subprocess.run(["kill", "-0", sys.argv[2]]).returncode: sys.exit("server exited")
    s.sendto(b"\xff\xff\xff\xffgetinfo boot", ("127.0.0.1", int(sys.argv[1])))
    try:
        if b"infoResponse" in s.recv(4096): sys.exit(0)
    except OSError: pass
    time.sleep(1)
sys.exit("server never answered getinfo")
' "$PORT" "$SPID" || { kill $SPID $CPID; wait; exit 1; }
sleep 2
if [ -n "$CLIENT" ]; then
    ADDR=127.0.0.1:$PORT bash -c "$CLIENT" > "$OUT/client.log" 2>&1
else
    sleep "$SECS"
fi
kill $SPID $CPID 2>/dev/null
wait
