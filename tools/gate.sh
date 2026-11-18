#!/usr/bin/env bash
# gate.sh [area...]: what a branch passes before it merges, assembled from
# docs/dev/testing.md. fmt and clippy, then the workspace suite once per
# install the environment names, then the ignored gates of each area.
#
# Installs (each optional; absolute paths, docs/dev/testing.md):
#   COD_DIR       CoD1 1.1: the workspace suite runs here, with COD2_DIR
#                 and COD4_DIR, and VCOD_REQUIRE_INSTALL names all three
#   COD_UO_DIR    a stock-only tree with uo/ beside 1.5's main/: the
#                 vcod-server suite runs again here, COD4_DIR unset (two
#                 CoD4 client passes under one memory cap swap for an hour)
#   COD2_DIR, COD4_DIR
#   With none set the suite runs as CI does: no pak test, no A/B gate.
#
# Areas (the ignored gates, testing.md "Ignored tests"):
#   nav     nav_census at COD_DIR and COD_UO_DIR
#   bots    bots:: at both, bot_threads::, cod4::bots:: with COD4_DIR
#   render  renderer::pipeline_check (needs a Vulkan adapter; LVP_ICD
#           names lavapipe's ICD json to use it)
#   cod2    cod2_gametype_census with COD2_DIR
#   cod4    cod4_every_map_poses_both_teams with COD4_DIR
#
# Env:
#   CARGO_WRAP  a command every cargo run goes through, word split (a
#               memory-capped scope); default none
#   DRY=1       print each step's command instead of running it
#   LOG         where cargo's output goes; default a temp file, kept on a
#               failure
#
# A test step passes only on a `test result: ok` line with tests run and
# no FAILED: a build that ran nothing, or a skip, is not a pass.
# The suite skips the examples (`--lib --bins --tests`): clippy's
# --all-targets already compiles them, and linking them costs a GB.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
read -ra wrap <<< "${CARGO_WRAP:-}"
if [[ ${DRY:-} == 1 ]]; then
	log=/dev/null
else
	log=${LOG:-$(mktemp -t vcod-gate.XXXXXX)}
	: > "$log"
fi
for a in "$@"; do
	case $a in
		nav | bots | render | cod2 | cod4) ;;
		*) echo "gate: unknown area $a (nav, bots, render, cod2, cod4)" >&2; exit 2 ;;
	esac
done
for v in COD_DIR COD_UO_DIR COD2_DIR COD4_DIR; do
	p=${!v:-}
	if [[ -n $p && $p != /* ]]; then
		echo "gate: $v=$p is relative; tests run in their crate's directory" >&2
		exit 2
	fi
done

fail() {
	echo "RED: $1 (log: $log)"
	grep -E "^test .*FAILED|panicked at" "$log" | head -20
	exit 1
}

# run <name> <env...> -- <cargo args...>: one cargo step.
run() {
	local name=$1 envs=()
	shift
	while [[ $1 != -- ]]; do envs+=("$1"); shift; done
	shift
	if [[ ${DRY:-} == 1 ]]; then
		echo "$name: env ${envs[*]} ${wrap[*]} cargo $*"
		return 0
	fi
	env "${envs[@]}" "${wrap[@]}" cargo "$@" >> "$log" 2>&1
}

# check <name> <env...> -- <cargo test args...>: a test step held to the
# `test result` rule.
check() {
	local name=$1 from
	[[ ${DRY:-} == 1 ]] && { run "$@"; return; }
	from=$(wc -l < "$log")
	run "$@"
	local results passed
	results=$(tail -n +"$((from + 1))" "$log" | grep "^test result")
	passed=$(echo "$results" | awk '{s += $4} END {print s + 0}')
	if ! echo "$results" | grep -q "test result: ok" \
		|| echo "$results" | grep -q FAILED || ((passed == 0)); then
		fail "$name"
	fi
	echo "$name: $passed passed"
}

# The installs the 1.1 pass holds the suite to.
req=()
[[ -n ${COD_DIR:-} ]] && req+=(cod1 cod11)
[[ -n ${COD2_DIR:-} ]] && req+=(cod2)
[[ -n ${COD4_DIR:-} ]] && req+=(cod4)
require=$(IFS=,; echo "${req[*]}")
base=(VCOD_REQUIRE_INSTALL="$require")

run fmt -- fmt --all --check || fail fmt
run clippy -- clippy --workspace --all-targets -- -D warnings || fail clippy
[[ ${DRY:-} == 1 ]] || echo "fmt, clippy: ok"
check suite "${base[@]}" -- test --workspace --lib --bins --tests --no-fail-fast
uo=(COD_DIR="${COD_UO_DIR:-}" COD4_DIR= VCOD_REQUIRE_INSTALL=cod1,uo)
[[ -n ${COD_UO_DIR:-} ]] && check "suite (UO)" "${uo[@]}" -- test -p vcod-server --no-fail-fast

srv=(test --release -p vcod-server --test server --)
for area in "$@"; do
	case $area in
		nav)
			[[ -n ${COD_DIR:-} ]] && check "nav_census" "${base[@]}" -- "${srv[@]}" --ignored nav_census::
			[[ -n ${COD_UO_DIR:-} ]] && check "nav_census (UO)" "${uo[@]}" -- "${srv[@]}" --ignored nav_census::
			;;
		bots)
			[[ -n ${COD_DIR:-} ]] && check "bots" "${base[@]}" -- "${srv[@]}" --ignored bots::
			[[ -n ${COD_UO_DIR:-} ]] && check "bots (UO)" "${uo[@]}" -- "${srv[@]}" --ignored bots::
			check "bot_threads" "${base[@]}" -- "${srv[@]}" bot_threads::
			[[ -n ${COD4_DIR:-} ]] && check "cod4 bots" "${base[@]}" -- \
				test --release -p vcod-server --lib -- --ignored cod4::bots::
			;;
		render)
			lvp=()
			[[ -n ${LVP_ICD:-} ]] && lvp=(VK_DRIVER_FILES="$LVP_ICD" WGPU_BACKEND=vulkan GALLIVM_PERF=nopt)
			check "pipeline_check" "${base[@]}" "${lvp[@]}" -- test -p vcod --bins -- renderer::pipeline_check
			;;
		cod2)
			[[ -n ${COD2_DIR:-} ]] && check "cod2_gametype_census" "${base[@]}" -- \
				test --release -p vcod-server --lib -- --ignored cod2_gametype_census
			;;
		cod4)
			[[ -n ${COD4_DIR:-} ]] && check "cod4 hit boxes" "${base[@]}" -- \
				test --release -p vcod-server --lib -- --ignored cod4_every_map_poses_both_teams
			;;
	esac
done
[[ ${DRY:-} == 1 ]] && exit 0
echo "ALL GREEN $(git rev-parse --short HEAD)"
[[ -n ${LOG:-} ]] || rm -f "$log"
