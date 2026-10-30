#!/usr/bin/env bash
# cod4_perf.sh <outdir> <map> <secs> [vcod args...]: the client's frame
# time in live play (CoD4 by default; GAME=cod2 for CoD2, GAME=cod1 or
# GAME=uo for CoD1 1.1/1.5 or United Offensive). Runs vcod-server (GAMETYPE, BOTS bots that
# shoot) on <map> and a windowed vcod joined to it with
# VCOD_FRAME_PROFILE set, both inside a loopback-only `unshare -rn`, the
# client in a nested `kwin_wayland --virtual` (its own dbus session), for
# <secs> seconds of play after the join, then prints the summary
# (tools/cod4_perf.py <outdir>, which reads <outdir>/profile.txt).
#
# The client joins as a spectator, which on CoD4's dm at the stock
# scr_game_spectatetype 1 follows a bot, unless the vcod args pass --team.
# A CoD2 free spectator, and a CoD1 or UO one at the map's spectator
# spot, stand still.
# It runs at com_maxfps 0 and r_swapInterval 0, so a frame's time is its
# own. Env:
#   GAME       cod4 (default), cod2, cod1 (COD_DIR's main/) or uo
#              (COD_DIR's uo/ over main/)
#   COD4_DIR   the 1.7 install (required for cod4)
#   COD2_DIR   the 1.3 install (required for cod2)
#   COD_DIR    the CoD1 or UO install (required for cod1 and uo)
#   BIN        the directory holding release vcod and vcod-server
#              (default: target/release beside this script's checkout)
#   GAMETYPE   default dm
#   BOTS       bots on the server (default 12)
#   PORT       server port (default 28970)
#   SIZE       the client's window WxH (`vcod --window`, which leaves the
#              saved r_mode alone; default 1280x800), the nested display
#              64 larger each way for the title bar. With SESSION=1 and no
#              SIZE the window takes the game's saved video mode.
#   SKIP_SECS  seconds after the join left out of the summary (default 10)
#   SERVER_ARGS extra vcod-server arguments, word split (e.g. `--set
#              sv_cheats=1`, so `--exec-in-game "setviewpos x y z yaw
#              pitch"` holds the view on one spot)
#   VK_DRIVER_FILES / WGPU_BACKEND pass through: a Vulkan adapter with BC
#   textures (lavapipe will do; its GPU ms are CPU rasterization, on
#   LP_NUM_THREADS threads, default 4 here).
#   SESSION=1  run in the current desktop session instead, for a real GPU:
#              no namespace and no nested kwin, the client a window on
#              your own display (WAYLAND_DISPLAY or DISPLAY as set), the
#              server on 127.0.0.1:PORT, so pick a free PORT. VK_DRIVER_FILES
#              and LP_NUM_THREADS are left as they are.
#   TURN       degrees a second the client's view yaws on its own
#              (VCOD_PERF_TURN, CoD4 only): a moving view, as in a fight,
#              where the held view's cull and world bundles never hold
#              (cod4-client-perf.md 36)
#   SWEEP=1    one client session, one setting toggled a pass (CoD4 only,
#              cod4-client-perf.md 25): <secs> is each pass's measured
#              time (default 10), SETTLE the wait before it (default 3).
#              Defaults to war, no bots, `sv_cheats 1` and, on
#              mp_crossfire, the held view `setviewpos 5232 -888 180 225
#              0` (VIEW="x y z yaw pitch" another); BOTS=12 adds the
#              fight. Passes: base, aa4, prepass0, sunshadow0, zfeather0,
#              distortion0, fxdraw0, specular0, normalmap0, fog0, glow0,
#              dof0, film0, base2 (PASSES="base fog0 ..." picks some).
#              Each starts from the defaults (r_aaSamples 1), sets its
#              dvar, settles, measures, sets it back; the vcod_mp.cfg
#              files under the install are restored at the end. Prints
#              `cod4_perf.py --sweep`'s table.
# Writes server.log, client.log, kwin.log (nested only) and profile.txt
# under <outdir>. Kills only the processes it started.
set -u
SESSION=${SESSION:-0}
if [[ $SESSION != 1 && ${PERF_INNER:-} != 1 ]]; then
	PERF_INNER=1 exec unshare -rn dbus-run-session -- "$0" "$@"
fi
OUT=$(realpath -m "$1") MAP=$2 SECS=${3:-10}
shift 3 2> /dev/null || shift $#
SWEEP=${SWEEP:-0}
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${BIN:-$HERE/../target/release}
GAME=${GAME:-cod4}
case $GAME in
cod4) : "${COD4_DIR:?set COD4_DIR to the CoD4 1.7 install}"; DIR=$COD4_DIR GARGS=(--game cod4) ;;
cod2) : "${COD2_DIR:?set COD2_DIR to the CoD2 1.3 install}"; DIR=$COD2_DIR GARGS=(--game cod2) ;;
cod1) DIR=${COD_DIR:?set COD_DIR to the CoD1 install} GARGS=() ;;
uo) DIR=${COD_DIR:?set COD_DIR to the UO install} GARGS=(--mod-dir uo) ;;
*) echo "GAME is cod4, cod2, cod1 or uo" >&2; exit 2 ;;
esac
[[ $SESSION == 1 ]] || export LP_NUM_THREADS=${LP_NUM_THREADS:-4}
if [[ $SWEEP == 1 ]]; then
	[[ $GAME == cod4 ]] || { echo "SWEEP=1 is CoD4 only" >&2; exit 2; }
	GAMETYPE=${GAMETYPE:-war} BOTS=${BOTS:-0}
	SERVER_ARGS="--set sv_cheats=1 ${SERVER_ARGS:-}"
fi
WINDOW=()
[[ $SESSION == 1 && -z ${SIZE:-} ]] || WINDOW=(--window "${SIZE:=1280x800}")
GAMETYPE=${GAMETYPE:-dm} BOTS=${BOTS:-12} PORT=${PORT:-28970}
mkdir -p "$OUT"
[[ $SESSION == 1 ]] || ip link set lo up
"$BIN/vcod-server" "${GARGS[@]}" --game-dir "$DIR" --port "$PORT" \
	--gametype "$GAMETYPE" --max-clients $((BOTS + 2)) --bots "$BOTS" --bots-shoot ${SERVER_ARGS:-} "$MAP" \
	> "$OUT/server.log" 2>&1 &
spid=$!
for _ in $(seq 1200); do
	grep -q " on udp/" "$OUT/server.log" 2> /dev/null && break
	sleep 0.1
done
sleep 2
kp=
if [[ $SESSION != 1 ]]; then
	S=wayland-vcodperf-$$
	export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
	unset DISPLAY WAYLAND_DISPLAY
	# room round the window for kwin's title bar, so it opens at SIZE
	kwin_wayland --virtual --socket "$S" --width $((${SIZE%x*} + 64)) --height $((${SIZE#*x} + 64)) \
		> "$OUT/kwin.log" 2>&1 &
	kp=$!
	for _ in $(seq 200); do
		[[ -S $XDG_RUNTIME_DIR/$S ]] && break
		sleep 0.1
	done
	export WAYLAND_DISPLAY=$S
fi
JOIN=(--team spectator)
for a in "$@"; do [[ $a == --team ]] && JOIN=(); done
SWEEP_ARGS=()
if [[ $SWEEP == 1 ]]; then
	# each pass: name, the lines that set it, the lines that set it back
	declare -A ON=(
		[base]="" [base2]=""
		[aa4]="set r_aaSamples 4;vid_restart"
		[prepass0]="set r_vc_depthprepass 0"
		[sunshadow0]="set sm_enable 0"
		[zfeather0]="set r_zFeather 0"
		[distortion0]="set r_distortion 0"
		[fxdraw0]="set fx_draw 0"
		[specular0]="set r_specular 0"
		[normalmap0]="set r_vc_normalMaps 0"
		[fog0]="set r_fog 0"
		[glow0]="set r_glow 0"
		[dof0]="set r_dof_enable 0"
		[film0]="set r_vc_film 0"
	)
	declare -A OFF=(
		[aa4]="set r_aaSamples 1;vid_restart"
		[prepass0]="set r_vc_depthprepass 1"
		[sunshadow0]="set sm_enable 1"
		[zfeather0]="set r_zFeather 1"
		[distortion0]="set r_distortion 1"
		[fxdraw0]="set fx_draw 1"
		[specular0]="set r_specular 1"
		[normalmap0]="set r_vc_normalMaps 1"
		[fog0]="set r_fog 1"
		[glow0]="set r_glow 1"
		[dof0]="set r_dof_enable 1"
		[film0]="set r_vc_film 1"
	)
	PASSES=${PASSES:-base aa4 prepass0 sunshadow0 zfeather0 distortion0 fxdraw0 specular0 normalmap0 fog0 glow0 dof0 film0 base2}
	VIEW=${VIEW:-}
	[[ -z $VIEW && $MAP == mp_crossfire ]] && VIEW="5232 -888 180 225 0"
	SETTLE=${SETTLE:-3}
	# the defaults every pass starts from, whatever the config holds
	SWEEP_ARGS+=(--exec-in-game "set r_aaSamples 1")
	for v in r_vc_depthprepass sm_enable r_zFeather r_distortion fx_draw r_specular \
		r_vc_normalMaps r_fog r_glow r_glow_allowed r_dof_enable r_vc_film; do
		SWEEP_ARGS+=(--exec-in-game "set $v 1")
	done
	[[ -n $VIEW ]] && SWEEP_ARGS+=(--exec-in-game "setviewpos $VIEW")
	SWEEP_ARGS+=(--exec-after "0 vid_restart")
	total=0
	for p in $PASSES; do
		[[ -v "ON[$p]" ]] || { echo "unknown pass $p" >&2; exit 2; }
		IFS=';' read -ra on <<< "${ON[$p]}"
		IFS=';' read -ra off <<< "${OFF[$p]:-}"
		for l in "${on[@]}"; do SWEEP_ARGS+=(--exec-after "0 $l"); done
		# a vid_restart drops the held view; set it again
		[[ ${ON[$p]} == *vid_restart* && -n $VIEW ]] && SWEEP_ARGS+=(--exec-after "1 setviewpos $VIEW")
		SWEEP_ARGS+=(--exec-after "$SETTLE echo sweep $p measure")
		SWEEP_ARGS+=(--exec-after "$SECS echo sweep $p end")
		for l in "${off[@]}"; do SWEEP_ARGS+=(--exec-after "0 $l"); done
		[[ ${OFF[$p]:-} == *vid_restart* && -n $VIEW ]] && SWEEP_ARGS+=(--exec-after "1 setviewpos $VIEW")
		total=$((total + SETTLE + SECS + 2))
	done
	SWEEP_ARGS+=(--exec-after "1 quit")
	# every vcod_mp.cfg the sweep's sets would rewrite, put back at exit
	CFGS=()
	while IFS= read -r -d '' f; do CFGS+=("$f"); done < <(find "$DIR" -maxdepth 5 -name vcod_mp.cfg -print0 2> /dev/null)
	mkdir -p "$OUT/cfg"
	for i in "${!CFGS[@]}"; do cp -p "${CFGS[$i]}" "$OUT/cfg/$i.cfg"; done
	restore() {
		for i in "${!CFGS[@]}"; do cp -p "$OUT/cfg/$i.cfg" "${CFGS[$i]}"; done
	}
	trap restore EXIT
fi
[[ -n ${TURN:-} ]] && export VCOD_PERF_TURN=$TURN
VCOD_FRAME_PROFILE="$OUT/profile.txt" \
	"$BIN/vcod" "${GARGS[@]}" --game-dir "$DIR" --no-audio "${WINDOW[@]}" \
	--connect "127.0.0.1:$PORT" "${JOIN[@]}" \
	--exec-in-game "set com_maxfps 0" --exec-in-game "set r_swapInterval 0" \
	"${SWEEP_ARGS[@]}" "$@" \
	> "$OUT/client.log" 2>&1 &
cp=$!
# The join: the first profiled frame that draws a model (`models`, found
# by its place in the `# counts` header; the counts end each frame line).
for _ in $(seq 1200); do
	awk '$1 == "#" && $2 == "counts" { n = NF - 2; for (i = 3; i <= NF; i++) if ($i == "models") m = i - 2 }
		$1 == "f" && m && $(NF - n + m) > 0 { found = 1; exit } END { exit !found }' \
		"$OUT/profile.txt" 2> /dev/null && break
	kill -0 "$cp" 2> /dev/null || break
	sleep 0.1
done
if [[ $SWEEP == 1 ]]; then
	# the queue ends in `quit`; the vid_restarts and the join on top
	for _ in $(seq $(((total + 240) * 10))); do
		kill -0 "$cp" 2> /dev/null || break
		sleep 0.1
	done
else
	sleep "$SECS"
fi
kill -INT "$cp" 2> /dev/null
for _ in $(seq 20); do kill -0 "$cp" 2> /dev/null || break; sleep 0.5; done
kill "$cp" 2> /dev/null
wait "$cp" 2> /dev/null
kill $kp "$spid" 2> /dev/null
wait $kp "$spid" 2> /dev/null
if [[ $SWEEP == 1 ]]; then
	python3 "$HERE/cod4_perf.py" --sweep "$OUT/profile.txt" | tee "$OUT/sweep.tsv"
else
	python3 "$HERE/cod4_perf.py" --skip-secs "${SKIP_SECS:-10}" "$OUT/profile.txt"
fi
