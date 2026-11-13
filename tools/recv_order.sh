#!/usr/bin/env bash
# Prints a retail server's packet receive order as the `RECV` lines a
# fixture carries, from its console log under `+set showpackets 1`
# (docs/dev/retail.md, "Receive order"). Netchan_Process prints
# `server recv <size> : s=<sequence>` for every client packet before
# SV_ExecuteClientMessage runs it, in the order the server read them, and
# names no client; a probe's `CMDS` lines carry each cmd's sequence (`q=`),
# which ties a print to a client and a cmd. Tokens, 60 to a line:
#
#   <s>    a `server recv` print's sequence
#   c<n>   `Client <n> connecting`: slot n's sequences start here
#   o<s>   `Out of order packet <s>`: the packet before it was dropped
#   d<n>   `Dropped <n> packets`
#
#   tools/recv_order.sh <homepath>/main/console_mp_server.log >> fixture.txt
set -euo pipefail
LOG="${1:?usage: tools/recv_order.sh <console log>}"
awk '
/ recv +[0-9]+ : s=[0-9]+$/ { sub(/.*s=/, ""); tok($0); next }
/^Client [0-9]+ connecting/ { tok("c" $2); next }
/:Out of order packet / { match($0, /packet -?[0-9]+/); tok("o" substr($0, RSTART + 7, RLENGTH - 7)); next }
/:Dropped [0-9]+ packets/ { match($0, /Dropped [0-9]+/); tok("d" substr($0, RSTART + 8, RLENGTH - 8)); next }
function tok(t) { line = line (n ? "," : "") t; if (++n == 60) flush() }
function flush() { if (n) print "RECV " line; line = ""; n = 0 }
END { flush() }
' "$LOG"
