# Sourced by the scripts beside it: preflight checks that name what is
# missing and exit 2, before a run waits on something that will never come.

# need <command...>: each must be on PATH.
need() {
	local t
	for t; do
		command -v "$t" > /dev/null || { echo "${0##*/}: $t not found on PATH" >&2; exit 2; }
	done
}

# need_built <path> <how>: a binary the script runs, and the command that
# builds it.
need_built() {
	[[ -x $1 ]] || { echo "${0##*/}: $1 missing; build it with: $2" >&2; exit 2; }
}
