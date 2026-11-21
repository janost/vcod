#!/usr/bin/env bash
# baselines.sh <out-dir>: records the refactor baselines into <out-dir>, so a
# later tip's record diffs against them.
# baselines.sh --compare <old-dir> <new-dir>: prints what differs, exit 1 if
# anything does.
#
# Records, a file each:
#   bot_<game>.txt  bot_match's `digest`/`net` lines, seeds 1 and 2, 6000
#                   ticks, DIGEST and NETDIGEST every 600
#   nav_<game>.txt  nav_build's line per map, build time cut (default maps)
#   tests_<crate>.txt  every test of `cargo test -p <crate> --tests --
#                   list` (lib, bins, integration), `<target> <name>`, sorted
# A step that fails writes `FAILED (exit n)` into its file, so the compare
# shows it. Logs go to <out-dir>/log/, the commit to <out-dir>/meta.txt;
# neither is compared.
#
# Installs (absolute; a game whose variable is unset is skipped):
#   COD_DIR      CoD1 1.1
#   COD_UO_DIR   a stock tree with uo/ beside 1.5's main/
#   COD2_DIR, COD4_DIR
# CARGO_WRAP: a command every cargo run goes through, word split (a
# memory-capped scope), as tools/gate.sh takes it.
set -uo pipefail

if [[ ${1:-} == --compare ]]; then
	[[ $# == 3 && -d $2 && -d $3 ]] || { echo "usage: baselines.sh --compare <old-dir> <new-dir>" >&2; exit 2; }
	diff -ru --exclude=log --exclude=meta.txt "$2" "$3"
	s=$?
	((s == 0)) && echo "baselines: identical"
	exit "$s"
fi
[[ $# == 1 ]] || { echo "usage: baselines.sh <out-dir> | --compare <old-dir> <new-dir>" >&2; exit 2; }
out=$(realpath -m "$1")
cd "$(dirname "$0")/.." || exit 2
mkdir -p "$out/log"
read -ra wrap <<< "${CARGO_WRAP:-}"
for v in COD_DIR COD_UO_DIR COD2_DIR COD4_DIR; do
	p=${!v:-}
	if [[ -n $p && $p != /* ]]; then
		echo "baselines: $v=$p is relative" >&2
		exit 2
	fi
done
echo "commit $(git rev-parse HEAD)$(git diff --quiet HEAD || echo ' (dirty)')" > "$out/meta.txt"
date -u +%FT%TZ >> "$out/meta.txt"
failed=0
start=$SECONDS

# step <name> <filter> <env...> -- <cargo args...>: runs cargo with the env,
# its output through <filter> into <out>/<name>.txt.
step() {
	local name=$1 filter=$2 envs=() t=$SECONDS
	shift 2
	while [[ $1 != -- ]]; do envs+=("$1"); shift; done
	shift
	env -u COD_DIR -u COD2_DIR -u COD4_DIR -u UO -u GAME -u MAPS -u MAP -u GAMETYPE -u BOTS -u SET "${envs[@]}" \
		"${wrap[@]}" cargo "$@" > "$out/log/$name.log" 2>&1
	local s=$?
	if ((s == 0)); then
		$filter < "$out/log/$name.log" > "$out/$name.txt"
		echo "$name: $(wc -l < "$out/$name.txt") lines, $((SECONDS - t)) s"
	else
		echo "FAILED (exit $s)" > "$out/$name.txt"
		echo "$name: FAILED (exit $s), log $out/log/$name.log"
		failed=1
	fi
}

bot_lines() { grep -E '^(digest|net)(@[0-9]+)? '; }
nav_lines() { grep -E '^mp_\w+: ' | sed -E 's/, [0-9]+ ms,/,/'; }
# `cargo test -- --list` names its binary on a `Running` line, then lists
# `name: test`.
test_lines() {
	awk '/^ *Running /{t=($2 == "unittests" ? $2" "$3 : $2)} /: test$/{sub(/: test$/, ""); print t" "$0}' |
		LC_ALL=C sort
}

bot=(run --release -p vcod-server --example bot_match -- 1 2)
bot_env=(DIGEST=600 NETDIGEST=600 TICKS=6000 JOBS=2)
nav=(run --release -p vcod-server --example nav_build)
[[ -n ${COD_DIR:-} ]] && step bot_cod11 bot_lines COD_DIR="$COD_DIR" "${bot_env[@]}" -- "${bot[@]}"
[[ -n ${COD_UO_DIR:-} ]] && step bot_uo bot_lines COD_DIR="$COD_UO_DIR" UO=1 "${bot_env[@]}" -- "${bot[@]}"
[[ -n ${COD2_DIR:-} ]] && step bot_cod2 bot_lines COD2_DIR="$COD2_DIR" GAME=cod2 "${bot_env[@]}" -- "${bot[@]}"
[[ -n ${COD4_DIR:-} ]] && step bot_cod4 bot_lines COD4_DIR="$COD4_DIR" GAME=cod4 "${bot_env[@]}" -- "${bot[@]}"
[[ -n ${COD_DIR:-} ]] && step nav_cod11 nav_lines COD_DIR="$COD_DIR" -- "${nav[@]}"
[[ -n ${COD_UO_DIR:-} ]] && step nav_uo nav_lines COD_DIR="$COD_UO_DIR" UO=1 -- "${nav[@]}"
[[ -n ${COD4_DIR:-} ]] && step nav_cod4 nav_lines COD4_DIR="$COD4_DIR" GAME=cod4 -- "${nav[@]}"
for toml in crates/*/Cargo.toml; do
	crate=$(sed -nE '0,/^name *= *"(.*)"/s//\1/p' "$toml")
	step "tests_$crate" test_lines -- test -p "$crate" --tests -- --list
done

echo "baselines: $out in $((SECONDS - start)) s"
exit "$failed"
