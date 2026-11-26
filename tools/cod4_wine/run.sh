#!/usr/bin/env bash
# Runs the retail CoD4 1.7 Windows client, iw3mp.exe, headless under Wine:
# no network (its own net namespace), no host display (a KWin --virtual
# compositor of its own) and no host audio (ALSA's null PCM only).
# docs/dev/retail.md, "The CoD4 Windows client".
#
#   COD4_DIR       CoD4 1.7 install holding iw3mp.exe, zone/ and main/ (required,
#                  only ever read)
#   COD4_WINE_DIR  state (default <repo>/tmp/cod4_wine): prefix/ the Wine prefix,
#                  made on the first run; root/ the game root the client runs
#                  in, the install's files copied or linked; run/ this run's logs
#   SECS           seconds the client runs before teardown (default 120)
#   GDB_PY         a gdb python script attached to the client once it starts
#                  (tools/re/cod4_*_gdb.py); its end, or the client's exit, ends
#                  the run early
#   SHOTS          1: screenshot each window just before teardown into run/
#   PROFILE        the player profile (default vcod)
#
#   tools/cod4_wine/run.sh [game args...]
#     e.g. SECS=60 tools/cod4_wine/run.sh +devmap mp_crash
# The client gets `+set logfile 2 +set r_fullscreen 0` and then the args. Its
# console log lands in run/console_mp.log, the gdb script's in run/gdb.log.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
D=${COD4_WINE_DIR:-$REPO/tmp/cod4_wine}
SECS=${SECS:-120}
PROFILE=${PROFILE:-vcod}
. "$REPO/tools/lib.sh"

if [[ -z ${COD4_WINE_INNER:-} ]]; then
	need wine wineserver wineboot kwin_wayland dbus-launch unshare
	[[ -n ${GDB_PY:-} ]] && need gdb
	[[ ${SHOTS:-0} == 1 ]] && need import xprop
	[[ -n ${COD4_DIR:-} && -f $COD4_DIR/iw3mp.exe && -d $COD4_DIR/zone ]] ||
		{ echo "${0##*/}: COD4_DIR must name a CoD4 1.7 install (iw3mp.exe, zone/)" >&2; exit 2; }
	[[ $COD4_DIR == /* && $D == /* ]] || { echo "${0##*/}: COD4_DIR and COD4_WINE_DIR must be absolute" >&2; exit 2; }
	mkdir -p "$D"
	# -m: the tmpfs mounts below stay inside; -n: no network but a down lo
	exec unshare -rnm env COD4_WINE_INNER=1 "$0" "$@"
fi
grep -qE '^ *0 +0 +4294967295$' /proc/self/uid_map &&
	{ echo "${0##*/}: the inner half runs only inside its own unshare; unset COD4_WINE_INNER" >&2; exit 1; }

RUN=$D/run
rm -rf "$RUN"
mkdir -p "$RUN" "$D/prefix" "$D/root"
# a private X socket dir and no sound devices: the client sees ALSA's null PCM only
mount -t tmpfs tmpfs /tmp/.X11-unix || exit 1
mount -t tmpfs tmpfs /dev/snd || exit 1
# short: KWin's sockets under it overflow sun_path from a deep worktree
XDG_RUNTIME_DIR=$(mktemp -d /tmp/cod4wine-XXXXXX) || exit 1
export XDG_RUNTIME_DIR
unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS PULSE_SERVER PIPEWIRE_REMOTE
eval "$(dbus-launch --sh-syntax 2> /dev/null)" || true
export WINEPREFIX=$D/prefix
export WINEDLLOVERRIDES="mscoree,mshtml=;winepulse.drv,wineoss.drv=d"
export ALSA_CONFIG_PATH=$HERE/alsa-null.conf
export WINEDEBUG=${WINEDEBUG:--all}

KPID= WPID= GPID=
teardown() {
	[[ -n $GPID ]] && kill "$GPID" 2> /dev/null
	wineserver -k 2> /dev/null
	[[ -n $WPID ]] && wait "$WPID" 2> /dev/null
	[[ -n $GPID ]] && wait "$GPID" 2> /dev/null
	[[ -n $KPID ]] && { kill "$KPID" 2> /dev/null; wait "$KPID" 2> /dev/null; }
	[[ -n ${DBUS_SESSION_BUS_PID:-} ]] && kill "$DBUS_SESSION_BUS_PID" 2> /dev/null
	cp "$D/root/main/console_mp.log" "$RUN/" 2> /dev/null
	rm -rf "$XDG_RUNTIME_DIR"
}
trap teardown EXIT

kwin_wayland --virtual --xwayland --socket wayland-cod4wine --width 1024 --height 768 \
	> "$RUN/kwin.log" 2>&1 &
KPID=$!
for _ in $(seq 100); do compgen -G '/tmp/.X11-unix/X*' > /dev/null && break; sleep 0.2; done
X=$(ls /tmp/.X11-unix | head -1)
[[ -n $X ]] || { echo "${0##*/}: KWin brought up no Xwayland (run/kwin.log)" >&2; exit 1; }
export DISPLAY=:${X#X} WAYLAND_DISPLAY=wayland-cod4wine

if [[ ! -f $WINEPREFIX/system.reg ]]; then
	echo "making the Wine prefix"
	wineboot -i > "$RUN/wineboot.log" 2>&1 || { echo "${0##*/}: wineboot failed (run/wineboot.log)" >&2; exit 1; }
	# `wineserver -w` never returns here: the prefix's explorer stays up
	wineserver -k
fi

# The game root: the executable and its DLLs copied (Wine takes the game's
# directory from the exe's own path), the data linked, everything the game
# writes (main/console_mp.log, players/, the __iw3mp crash marker) local.
R=$D/root
for f in "$COD4_DIR"/*.exe "$COD4_DIR"/*.dll "$COD4_DIR"/*.inf "$COD4_DIR"/localization.txt; do
	[[ -f $f ]] && cp -u "$f" "$R/"
done
for f in zone miles; do [[ -e $COD4_DIR/$f && ! -e $R/$f ]] && ln -s "$COD4_DIR/$f" "$R/$f"; done
mkdir -p "$R/main" "$R/players/profiles/$PROFILE"
for f in "$COD4_DIR"/main/*.iwd "$COD4_DIR"/main/video; do
	[[ -e $R/main/${f##*/} ]] || ln -s "$f" "$R/main/${f##*/}"
done
echo -n "$PROFILE" > "$R/players/profiles/active.txt"
P=$R/players/profiles/$PROFILE/config_mp.cfg
[[ -f $P ]] || printf 'seta r_fullscreen "0"\nseta r_mode "800x600"\nseta ragdoll_enable "1"\nseta cl_punkbuster "0"\n' > "$P"
# the marker brings up "Run In Safe Mode?"
rm -f "$R/__iw3mp" "$R/main/console_mp.log"

cd "$R" || exit 1
wine iw3mp.exe +set logfile 2 +set r_fullscreen 0 "$@" > "$RUN/wine.out" 2>&1 &
WPID=$! # start.exe, which waits on the game
# the game itself: Wine starts it detached, so find it by name in this prefix
game_pid() {
	local p
	for p in /proc/[0-9]*; do
		[[ $(< "$p/comm") == iw3mp.exe ]] 2> /dev/null &&
			grep -qaxz "WINEPREFIX=$WINEPREFIX" "$p/environ" 2> /dev/null && echo "${p#/proc/}"
	done
}
for _ in $(seq 300); do GAME=$(game_pid) && [[ -n $GAME ]] && break; sleep 0.1; done
echo "iw3mp.exe pid ${GAME:-none}, DISPLAY=$DISPLAY"
if [[ -n ${GDB_PY:-} && -n $GAME ]]; then
	# the exe is mapped once its console log opens
	for _ in $(seq 300); do [[ -s $R/main/console_mp.log ]] && break; sleep 0.1; done
	gdb -batch -nx -iex 'set debuginfod enabled off' -p "$GAME" -x "$GDB_PY" > "$RUN/gdb.log" 2>&1 &
	GPID=$!
fi
for ((i = 0; i < SECS * 5; i++)); do
	kill -0 "$WPID" 2> /dev/null || break
	[[ -n $GPID ]] && ! kill -0 "$GPID" 2> /dev/null && break
	sleep 0.2
done
if [[ ${SHOTS:-0} == 1 ]]; then
	S=$RUN/shots
	mkdir -p "$S"
	import -window root "$S/root.png" 2> /dev/null
	n=0
	for w in $(xprop -root _NET_CLIENT_LIST | grep -o '0x[0-9a-f]*'); do
		echo "window $w $(xprop -id "$w" WM_NAME 2> /dev/null)"
		import -window "$w" "$S/window-$n.png" 2> /dev/null && n=$((n + 1))
	done
fi
