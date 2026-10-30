#!/usr/bin/env bash
# cod1_soak.sh <outdir> [map...]: the CoD1 client soak
# (docs/research/cod11-client-soak.md). Per map, inside a loopback-only
# `unshare -rn`: vcod-server loads it with six shooting bots and
# `sv_mapRotation` naming the map once per gametype (the first again last),
# and a windowed vcod on a nested `kwin_wayland --virtual` (lavapipe) joins
# as axis with the kar98k and plays through `main`'s whole frame under `VCOD_SOAK`
# (`play::cod1_soak`) and `VCOD_FRAME_PROFILE`. The map's run ends once the
# rotation comes back to the first gametype. Each map's files land in
# <outdir>/<map>/; `tools/cod1_soak.py <outdir>` writes the report.
#
# Env:
#   COD_DIR       the install (required): 1.1, or a UO one with UO=1
#   UO=1          United Offensive: `--mod-dir uo`, the UO map list
#   GAMETYPES     space-separated gametypes, default every one the map's
#                 arena lists (`mp/*.arena` in the install's paks)
#   MINUTES       each level's time limit (2)
#   JOBS          maps at once (1, at most 2)
#   BIN           a dir holding release vcod and vcod-server (default: this
#                 checkout's target/release, built first)
#   LVP_ICD       lavapipe's ICD json (required for VK_DRIVER_FILES)
#   CLIENT_LOG    the client's RUST_LOG (info,vcod=debug: vcod's debug lines
#                 name missing aliases, tags and menu items)
# Everything runs under `nice -n 10` pinned to cores CORES (0-7). Kills only the
# PIDs it started.
set -u
if [[ ${SOAK_INNER:-} == map ]]; then
	# One map, inside its own network namespace.
	out=$1 map=$2 gts=$3
	ip link set lo up
	sb=$out/sandbox
	rm -rf "$sb"
	mkdir -p "$sb/main"
	for f in "$COD_DIR"/main/*.pk3; do ln -s "$(readlink -f "$f")" "$sb/main/"; done
	cfg_dir=$sb/main ga=(--game-dir "$sb")
	if [[ ${UO:-} == 1 ]]; then
		mkdir -p "$sb/uo"
		for f in "$COD_DIR"/uo/*.pk3; do ln -s "$(readlink -f "$f")" "$sb/uo/"; done
		cfg_dir=$sb/uo ga=(--game-dir "$sb" --mod-dir uo)
	fi
	printf 'seta r_mode "-1"\nseta r_customwidth "640"\nseta r_customheight "480"\nseta r_fullscreen "0"\n' > "$cfg_dir/vcod_mp.cfg"
	read -r -a g <<< "$gts"
	rot=""
	for x in "${g[@]:1}" "${g[0]}"; do rot+="gametype $x map $map "; done
	sets=(--set "sv_mapRotation=$rot")
	for x in dm tdm sd re bel ctf dom bas hq; do
		sets+=(--set "scr_${x}_timelimit=${MINUTES:-2}" --set "scr_${x}_roundlimit=1"
			--set "scr_${x}_roundlength=${MINUTES:-2}")
	done
	# The per-snapshot trace lines are dropped; the per-second `tick:` ones
	# are what the report reads.
	RUST_LOG=info "$BIN/vcod-server" "$map" "${ga[@]}" --vanilla --gametype "${g[0]}" \
		--bots 6 --bots-shoot --max-clients 8 --trace "${sets[@]}" \
		> >(grep --line-buffered -v '] trace c' > "$out/server.log") 2>&1 &
	spid=$!
	for _ in $(seq 1200); do
		ss -uln 2> /dev/null | grep -q ':28960 ' && break
		sleep 0.5
	done
	(
		export XDG_RUNTIME_DIR=$SOAK_RUNTIME
		unset DISPLAY WAYLAND_DISPLAY
		sock=wayland-vcodsoak-$$
		dbus-run-session -- kwin_wayland --virtual --socket "$sock" --width 1024 --height 768 > "$out/kwin.log" 2>&1 &
		kp=$!
		for _ in $(seq 200); do
			[[ -S $XDG_RUNTIME_DIR/$sock ]] && break
			sleep 0.1
		done
		WAYLAND_DISPLAY=$sock VK_DRIVER_FILES=$LVP_ICD WGPU_BACKEND=vulkan GALLIVM_PERF=nopt \
			LP_NUM_THREADS=${LP_NUM_THREADS:-3} VCOD_SOAK=$out/soak.txt \
			VCOD_FRAME_PROFILE=$out/frames.txt RUST_LOG=${CLIENT_LOG:-info,vcod=debug} \
			"$BIN/vcod" "${ga[@]}" --vanilla --no-audio --connect 127.0.0.1:28960 \
			--team axis --weapon kar98k_mp > "$out/client.log" 2>&1 &
		cp=$!
		echo "$cp" > "$out/client.pid"
		# Done once the rotation is back to the first gametype, or past
		# the deadline.
		end=$(($(date +%s) + (${#g[@]} + 1) * (${MINUTES:-2} * 60 + 240)))
		while kill -0 "$cp" 2> /dev/null && [[ $(date +%s) -lt $end ]]; do
			n=$(grep -c ' level ' "$out/soak.txt" 2> /dev/null)
			[[ ${n:-0} -gt ${#g[@]} ]] && break
			sleep 5
		done
		kill -INT "$cp" 2> /dev/null
		for _ in $(seq 20); do kill -0 "$cp" 2> /dev/null || break; sleep 1; done
		kill -9 "$cp" 2> /dev/null
		wait "$cp"
		echo "client exit $?" >> "$out/client.log"
		kill "$kp" 2> /dev/null
		wait "$kp" 2> /dev/null
	)
	kill -INT "$spid" 2> /dev/null
	wait "$spid" 2> /dev/null
	rm -rf "$sb"
	exit 0
fi

out=${1:?usage: cod1_soak.sh <outdir> [map...]}
shift
: "${COD_DIR:?set COD_DIR to the install}"
: "${LVP_ICD:?set LVP_ICD to the lavapipe ICD json}"
here=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
if [[ -z ${BIN:-} ]]; then
	(cd "$here" && nice -n 10 cargo build --release -p vcod -p vcod-server) > "$out/build.log" 2>&1 ||
		{ echo "soak: build failed ($out/build.log)" >&2; exit 1; }
	# Copied out, so the checkout can change under a long run.
	mkdir -p "$out/bin"
	cp "$here/target/release/vcod" "$here/target/release/vcod-server" "$out/bin/"
	BIN=$out/bin
fi
export BIN COD_DIR LVP_ICD
# The user's runtime dir: inside `unshare -r` the uid reads 0.
export SOAK_RUNTIME=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
base=main
[[ ${UO:-} == 1 ]] && base=uo
# Each stock map with the gametypes its arena lists.
declare -A arena
while read -r m gl; do arena[$m]=$gl; done < <(
	for p in "$COD_DIR"/$base/*.pk3; do
		for a in $(unzip -Z1 "$p" 2> /dev/null | grep -i '^mp/.*\.arena$'); do unzip -p "$p" "$a"; done
	done | tr -s ' \t\r\n' ' ' | sed 's/}/}\n/g' |
		sed -nE 's/.*map "([^"]*)".*gametype "([^"]*)".*/\1 \2/p')
maps=("$@")
if [[ ${#maps[@]} -eq 0 ]]; then
	for p in "$COD_DIR"/$base/*.pk3; do unzip -Z1 "$p" 2> /dev/null; done |
		sed -nE 's|^maps/mp/(mp_[a-z_0-9]+)\.bsp$|\1|Ip' | sort -u > "$out/maps.txt"
	mapfile -t maps < "$out/maps.txt"
fi
run_map() {
	local m=$1 gts=${GAMETYPES:-${arena[$1]:-dm tdm}}
	mkdir -p "$out/$m"
	echo "$gts" > "$out/$m/gametypes.txt"
	echo "soak: $m ($gts)" >&2
	SOAK_INNER=map nice -n 10 taskset -c "${CORES:-0-7}" unshare -rn "$0" "$out/$m" "$m" "$gts"
}
jobs=${JOBS:-1}
running=0
for m in "${maps[@]}"; do
	run_map "$m" &
	running=$((running + 1))
	if [[ $running -ge $jobs ]]; then
		wait -n
		running=$((running - 1))
	fi
done
wait
