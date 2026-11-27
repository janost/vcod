#!/usr/bin/env bash
# feature_drift.sh [package...]: lists, per package, the (crate, features)
# pairs `cargo tree -p <package>` resolves and `cargo tree --workspace` does
# not, for builds and for test builds. Each one is a unit `cargo build -p` or
# `cargo test -p` compiles a second time, with every crate above it, instead
# of sharing the workspace build's. Default: every workspace member but
# vcod-workspace-hack, which nothing selects.
#
# Exit 1 on any drift. The fix is to name the workspace's features where the
# selection sees them: on the `[workspace.dependencies]` line when a member
# depends on the crate, else in crates/workspace-hack/Cargo.toml.
# Reads Cargo.lock only; builds nothing.
set -u
cd "$(dirname "$0")/.." || exit 2
nodes() {
	cargo tree --locked -q -e "features,$1" -f '{p} [{f}]' --prefix none "${@:2}" |
		grep -v ' feature "' | sed 's/ (\*)$//' | sort -u
}
if (($# == 0)); then
	# shellcheck disable=SC2046
	set -- $(cargo tree --locked -q --workspace --depth 0 --prefix none -f '{p}' | cut -d' ' -f1 | grep -vx vcod-workspace-hack | sort -u)
fi
(($# > 0)) || { echo "feature_drift: no packages (cargo tree failed)" >&2; exit 2; }
rc=0
for edges in normal,build normal,build,dev; do
	ws=$(nodes "$edges" --workspace)
	[[ -n $ws ]] || { echo "feature_drift: cargo tree --workspace printed nothing" >&2; exit 2; }
	for p in "$@"; do
		sel=$(nodes "$edges" -p "$p")
		[[ -n $sel ]] || { echo "feature_drift: cargo tree -p $p printed nothing" >&2; exit 2; }
		extra=$(comm -23 <(echo "$sel") <(echo "$ws"))
		if [[ -n $extra ]]; then
			rc=1
			echo "$p ($edges):"
			echo "${extra//$'\n'/$'\n'    }" | sed '1s/^/    /'
		fi
	done
done
((rc == 0)) && echo "feature_drift: $# packages resolve as the workspace does"
exit $rc
