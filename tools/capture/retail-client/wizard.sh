#!/usr/bin/env bash
# Walks one session at a retail Windows client (CoD 1.1 CoDMP.exe, United
# Offensive 1.51 CoDUOMP.exe) through the captures docs/dev/retail.md lists
# under "Retail client captures". For each step it starts vcod-server here
# with the step's probe gametype and --client-log, prints what to do in the
# client, waits, checks the capture with analyze.py and moves on.
#
#   tools/capture/retail-client/wizard.sh [options]
#
#   --addr IP       the address the client connects to; the server binds it
#                   (default 127.0.0.1: a client under Wine on this box). For
#                   a Windows PC on the LAN give this box's LAN address.
#   --port N        UDP port (default 28963)
#   --from STEP     start at STEP (number or name), redoing it and the rest
#   --only STEP     run STEP alone
#   --skip LIST     comma-separated steps to leave out
#   --out DIR       captures go here (default tmp/captures/retail-client-<date>)
#   --shots-11 DIR  where the 1.1 client's screenshots land, read from here
#                   (Wine: <install>/main/screenshots); without it the wizard
#                   asks you to copy them over at the end of the step
#   --shots-uo DIR  the same for the UO client (<install>/uo/screenshots)
#   --standin       dry run: vcod's probe client plays the retail client
#
# Env, as tools/gate.sh takes them (absolute paths):
#   COD_DIR      a CoD 1.1 install (main/ with the 1.1 paks)
#   COD_UO_DIR   a stock tree with uo/ beside 1.5's main/
#
# Steps (a done step is skipped on a rerun unless --from or --only names it):
#   1 kick          1.1  usercmd angles around hip shots (backlog 4, 15)
#   2 intermission  1.1  hudelems drawn at pm_type 5 (backlog 14)
#   3 hint          1.1  cursor hint icon width, unowned and owned (backlog 36)
#   4 hint-uo       UO   the same on UO's cgame (backlog 36)
#   5 fdc           UO   the flame report's fdc cadence (backlog 55)
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
HERE="$ROOT/tools/capture/retail-client"
PROBES="$ROOT/crates/gsc/tests/fixtures/semantics/client-probes"
SERVER="$ROOT/target/debug/vcod-server"
CLIENT="$ROOT/target/debug/vcod"
STEPS=(kick intermission hint hint-uo fdc)

ADDR=127.0.0.1 PORT=28963 FROM="" ONLY="" SKIP="" OUT="" SHOTS11="" SHOTSUO="" STANDIN=0
while [ $# -gt 0 ]; do
	case "$1" in
	--addr) ADDR=$2; shift 2 ;;
	--port) PORT=$2; shift 2 ;;
	--from) FROM=$2; shift 2 ;;
	--only) ONLY=$2; shift 2 ;;
	--skip) SKIP=$2; shift 2 ;;
	--out) OUT=$2; shift 2 ;;
	--shots-11) SHOTS11=$2; shift 2 ;;
	--shots-uo) SHOTSUO=$2; shift 2 ;;
	--standin) STANDIN=1; shift ;;
	-h|--help) sed -n '2,/^set -uo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
	*) echo "unknown option $1 (--help lists them)" >&2; exit 2 ;;
	esac
done
OUT=${OUT:-$ROOT/tmp/captures/retail-client-$(date +%F)}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

# A step's number or name as its index into STEPS.
step_index() {
	local i
	for i in "${!STEPS[@]}"; do
		[ "$1" = "${STEPS[$i]}" ] || [ "$1" = "$((i + 1))" ] && { echo "$i"; return 0; }
	done
	echo "no step $1 (one of 1-${#STEPS[@]} or ${STEPS[*]})" >&2
	exit 2
}

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
note() { printf '  %s\n' "$*"; }
# The owner's Enter; the stand-in's dry run waits `$1` seconds instead.
enter() {
	if [ $STANDIN = 1 ]; then sleep "${1:-1}"; return 0; fi
	read -r -p "  [Enter] " REPLY < /dev/tty
}

need_install() {
	local var=$1 path=${!1:-}
	case "$path" in
	/*) ;;
	"") echo "$var is not set (absolute path to the install, see --help)" >&2; exit 2 ;;
	*) echo "$var must be absolute: $path" >&2; exit 2 ;;
	esac
}

build() {
	if [ ! -x "$SERVER" ] || [ ! -x "$CLIENT" ]; then
		say "Building vcod-server and vcod (once)"
		(cd "$ROOT" && cargo build -p vcod-server -p vcod) || exit 1
	fi
}

SPID="" CPIDS=()
stop_all() {
	local p
	for p in "${CPIDS[@]}"; do kill "$p" 2> /dev/null; done
	CPIDS=()
	if [ -n "$SPID" ]; then
		echo quit >&3 2> /dev/null
		for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$SPID" 2> /dev/null || break; sleep 0.5; done
		kill "$SPID" 2> /dev/null
		wait "$SPID" 2> /dev/null
		SPID=""
	fi
	exec 3>&- 2> /dev/null
}
trap 'stop_all; exit 130' INT TERM
trap 'stop_all' EXIT

# start_server <dir> <game 11|uo> <map> <probe> [--set ...]: vcod-server in
# the background, its console on fd 3, its log in <dir>.
start_server() {
	local dir=$1 game=$2 map=$3 probe=$4
	shift 4
	local install args
	if [ "$game" = uo ]; then
		install=$COD_UO_DIR
		args=(--game-dir "$install" --mod-dir uo)
	else
		install=$COD_DIR
		args=(--game-dir "$install")
	fi
	rm -f "$dir/fifo"
	mkfifo "$dir/fifo"
	# The stock paks only, and sv_pure 1 (vcod-server's default) keeps the
	# client on them too, whatever else its install holds.
	"$SERVER" "${args[@]}" --vanilla --port "$PORT" --set "net_ip=$ADDR" \
		--set "fs_homepath=$dir" --set g_log=games_mp.log --set g_logSync=1 \
		--gametype-script "$PROBES/$probe.gsc" --client-log "$dir/client.log" \
		"$@" "$map" < "$dir/fifo" > "$dir/server.log" 2>&1 &
	SPID=$!
	exec 3> "$dir/fifo"
	local log
	for _ in $(seq 1 120); do
		log=$(game_log "$dir")
		if [ -n "$log" ] && grep -q InitGame "$log"; then
			note "server up: $(grep -o 'protocol\\[0-9]*' "$log" | head -1 | tr '\\' ' '), map $map, $ADDR:$PORT"
			return 0
		fi
		kill -0 "$SPID" 2> /dev/null || break
		sleep 0.5
	done
	echo "vcod-server did not come up; its log:" >&2
	tail -20 "$dir/server.log" >&2
	return 1
}

# games_mp.log under the homepath: main/ on 1.1, uo/ on UO.
game_log() {
	ls "$1"/*/games_mp.log 2> /dev/null | head -1
}

# The first client's `enter` line, waited for.
wait_enter() {
	local dir=$1 want_proto=$2 n=0 line
	while ! line=$(grep -m1 '^enter ' "$dir/client.log" 2> /dev/null); do
		kill -0 "$SPID" 2> /dev/null || { echo "vcod-server exited" >&2; return 1; }
		n=$((n + 1))
		[ $((n % 60)) = 0 ] && note "still waiting for the client to join ($((n / 2)) s)"
		sleep 0.5
	done
	local proto=${line#*proto=}
	proto=${proto%% *}
	if [ "$proto" != "$want_proto" ]; then
		echo "the client joined on protocol $proto, this step needs $want_proto (wrong game or patch)" >&2
		return 1
	fi
	note "client in: ${line#*name=} (protocol $proto)"
}

# standin <args...>: vcod's probe client in place of the retail one.
standin() {
	"$CLIENT" --net-probe "$ADDR:$PORT" "$@" > "$DIR/standin-$((${#CPIDS[@]} + 1)).log" 2>&1 &
	CPIDS+=($!)
}

connect_line() {
	local exe=$1
	if [ $STANDIN = 1 ]; then
		note "(dry run: vcod's probe client joins)"
		return
	fi
	note "In $exe: open the console (~) and enter"
	note "    /connect $ADDR:$PORT"
	note "(or start it with  $exe +connect $ADDR:$PORT). Leave the mouse alone once you are in."
}

# The console line that takes a phase's three screenshots with the console
# shut: the probe puts `screenshot rc_<phase>_1; ...` in the client's
# rc_shot cvar on every phase change.
SHOT_LINE='toggleconsole; wait 60; vstr rc_shot'

# phase_shots <phase> <shots dir>: the phase's screenshots. With the
# client's screenshot folder readable from here, waits for them and copies
# them into $DIR.
phase_shots() {
	local phase=$1 src=$2
	if [ $STANDIN = 0 ]; then
		note "Screenshots: open the console (~) and enter"
		note "    $SHOT_LINE"
		note "(after the first time: ~, Up, Enter). If no rc_${phase}_1..3 files appear,"
		note "enter  screenshot rc_${phase}_1  (and _2, _3) with the console shut by hand."
	fi
	if [ -z "$src" ]; then
		if [ $STANDIN = 1 ]; then sleep 2; else enter; fi
		return
	fi
	local mark=$DIR/.mark-$phase t=0
	touch "$mark"
	note "waiting for rc_${phase}_3.tga in $src (Enter stops waiting)"
	until [ "$src/rc_${phase}_3.tga" -nt "$mark" ]; do
		if [ $STANDIN = 1 ]; then
			sleep 1
			t=$((t + 1))
			[ $t -ge 30 ] && break
		else
			read -r -t 1 REPLY < /dev/tty && break
		fi
	done
	sleep 0.5
	cp "$src"/rc_"$phase"_[123].tga "$DIR"/ 2> /dev/null
	note "$(ls "$DIR"/rc_"$phase"_*.tga 2> /dev/null | wc -l) screenshot(s) of $phase kept"
}

# inbox_shots <shots dir> <phase...>: without a readable screenshot folder
# the step's rc_*.tga files are copied over by hand.
inbox_shots() {
	local src=$1 phase n missing=0
	shift
	[ -n "$src" ] || [ $STANDIN = 1 ] && return 0
	say "Copy this step's screenshots"
	note "From the client's screenshots folder (main\\screenshots; uo\\screenshots for UO)"
	note "copy every rc_*.tga file into"
	note "    $DIR"
	enter
	for phase; do
		for n in 1 2 3; do
			[ -e "$DIR/rc_${phase}_$n.tga" ] || { note "missing rc_${phase}_$n.tga"; missing=1; }
		done
	done
	return $missing
}

set_phase() {
	echo "set rc_phase $1" >&3
	sleep 1.5
}

analyze() {
	local extra=()
	[ $STANDIN = 1 ] && extra=(--standin)
	python3 -I "$HERE/analyze.py" "$@" "${extra[@]}" | tee "$DIR/analysis.txt"
	return "${PIPESTATUS[0]}"
}

# --- the steps -------------------------------------------------------------

step_kick() {
	need_install COD_DIR
	say "Step kick (CoD 1.1): the view kick in your usercmds around a hip shot"
	local sets=()
	[ $STANDIN = 1 ] && sets=(--set rc_drive=1)
	start_server "$DIR" 11 mp_carentan probe_rc_kick "${sets[@]}" || return 1
	connect_line CoDMP.exe
	[ $STANDIN = 1 ] && standin --probe-team allies --probe-uo-weapon direct --probe-secs 30
	wait_enter "$DIR" 1 || return 1
	note "You spawn as allies with the M1 carbine, view level. Do not touch the mouse"
	note "or the movement keys. Fire single shots from the hip with the left button,"
	note "one every 2 seconds, until the counter reaches 10 (reload if the clip runs dry)."
	note "(Enter ends the step early.)"
	local n=0 want=10 waited=0
	while [ "$n" -lt $want ]; do
		n=$(grep -cE '^ran .* ev=(.*,)?(159|161):' "$DIR/client.log")
		printf '\r  shots the server saw: %s/%s ' "$n" "$want"
		if [ $STANDIN = 1 ]; then
			sleep 1
			waited=$((waited + 1))
			[ $waited -gt 40 ] && break
		else
			read -r -t 1 REPLY < /dev/tty && break
		fi
	done
	echo
	note "Done. Stay connected while the wizard checks."
	# The last shot's kick runs on for a few hundred ms of cmds.
	sleep 2
	stop_all
	analyze kick "$DIR/client.log"
}

step_intermission() {
	need_install COD_DIR
	say "Step intermission (CoD 1.1): which hudelems the client draws at intermission"
	start_server "$DIR" 11 mp_carentan probe_rc_intermission || return 1
	connect_line CoDMP.exe
	[ $STANDIN = 1 ] && standin --probe-team allies --probe-secs 40
	wait_enter "$DIR" 1 || return 1
	note "You spawn alive with two coloured squares at the screen's left and right edge."
	note "Phase 1 of 2: alive."
	set_phase play
	phase_shots play "$SHOTS11"
	note "Phase 2 of 2: the server puts you in intermission (the scoreboard comes up)."
	set_phase intermission
	sleep 1
	phase_shots intermission "$SHOTS11"
	stop_all
	inbox_shots "$SHOTS11" play intermission || return 1
	analyze intermission "$DIR/client.log" "$(game_log "$DIR")" "$DIR"
}

# hint_step <11|uo> <exe> <shots dir>
hint_step() {
	local game=$1 exe=$2 shots=$3 map=mp_carentan proto=1
	[ "$game" = uo ] && map=mp_cassino proto=22
	start_server "$DIR" "$game" "$map" probe_rc_hint || return 1
	connect_line "$exe"
	if [ $STANDIN = 1 ]; then
		local gargs=()
		[ "$game" = uo ] && gargs=(--game-dir "$COD_UO_DIR" --mod-dir uo)
		standin "${gargs[@]}" --probe-team allies --probe-uo-weapon direct --probe-secs 45
	fi
	wait_enter "$DIR" "$proto" || return 1
	note "The server holds you looking down at the floor in front of you; leave the"
	note "mouse alone. Each phase puts an (invisible) weapon there, so the use icon"
	note "shows under the crosshair. Five phases."
	local phases=(none unowned_wide owned_wide unowned_narrow owned_narrow) i=0
	for p in "${phases[@]}"; do
		i=$((i + 1))
		note "Phase $i of ${#phases[@]}: $p"
		set_phase "$p"
		phase_shots "$p" "$shots"
	done
	stop_all
	inbox_shots "$shots" "${phases[@]}" || return 1
	analyze hint "$DIR/client.log" "$(game_log "$DIR")" "$DIR"
}

step_hint() {
	need_install COD_DIR
	say "Step hint (CoD 1.1): the cursor hint icon's width"
	hint_step 11 CoDMP.exe "$SHOTS11"
}

step_hint_uo() {
	need_install COD_UO_DIR
	say "Step hint-uo (United Offensive): the cursor hint icon's width"
	hint_step uo CoDUOMP.exe "$SHOTSUO"
}

step_fdc() {
	need_install COD_UO_DIR
	say "Step fdc (United Offensive): how often the client reports flame damage"
	start_server "$DIR" uo mp_cassino probe_rc_flame || return 1
	connect_line CoDUOMP.exe
	local uo=(--game-dir "$COD_UO_DIR" --mod-dir uo)
	[ $STANDIN = 1 ] && standin "${uo[@]}" --probe-team allies --probe-flame --probe-secs 60
	wait_enter "$DIR" 22 || return 1
	note "You spawn; a vcod client joins as a flamethrower and the server sets it"
	note "100 units in front of you every 2 s. You take no damage. Do not move."
	standin "${uo[@]}" --probe-team allies --probe-uo-weapon direct --probe-secs 65
	local t
	for t in $(seq 1 50); do
		printf '\r  fdc reports so far: %s (%s s of 50) ' \
			"$(grep -c '^ccmd .* text=fdc' "$DIR/client.log")" "$t"
		sleep 1
	done
	echo
	stop_all
	analyze fdc "$DIR/client.log" "$(game_log "$DIR")"
}

# --- the session -------------------------------------------------------------

build
first=0 last=$((${#STEPS[@]} - 1))
[ -n "$FROM" ] && first=$(step_index "$FROM")
[ -n "$ONLY" ] && first=$(step_index "$ONLY") last=$first
say "Captures go to $OUT"
[ $STANDIN = 1 ] && note "DRY RUN: vcod's probe client stands in for the retail client"
for i in $(seq "$first" "$last"); do
	name=${STEPS[$i]}
	case ",$SKIP," in *",$name,"*|*",$((i + 1)),"*) note "skipping $name"; continue ;; esac
	DIR="$OUT/$((i + 1))-$name"
	if [ -e "$DIR/DONE" ] && [ -z "$FROM$ONLY" ]; then
		note "$name done already ($DIR); --from $name redoes it"
		continue
	fi
	while :; do
		rm -rf "$DIR"
		mkdir -p "$DIR"
		if "step_${name//-/_}"; then
			date -Is > "$DIR/DONE"
			say "Step $name captured: $DIR"
			break
		fi
		stop_all
		say "Step $name did not capture (above). Enter retries it, s skips it, q quits."
		[ $STANDIN = 1 ] && break
		read -r -p "  [Enter/s/q] " REPLY < /dev/tty
		case "$REPLY" in s) break ;; q) exit 1 ;; esac
	done
done
say "Session over. $OUT holds one directory per step; docs/dev/retail.md says how each becomes a fixture."
