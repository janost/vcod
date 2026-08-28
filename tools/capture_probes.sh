#!/usr/bin/env bash
# Runs every probe in crates/gsc/tests/fixtures/semantics/ and writes the
# combined capture the A/B test reads. One `# <probe>` header per probe,
# then that probe's PROBE lines, then `PROBE_FATAL <message>` if the script
# killed the server.
#
# Same environment as tools/run_probe.sh. Takes a couple of minutes: each
# probe boots the retail server.
#
#   tools/capture_probes.sh > crates/gsc/tests/fixtures/semantics/retail-captures.txt
set -euo pipefail
cd "$(dirname "$0")/.."
# Pins the glob's sort order: under a UTF-8 locale the underscore is ignored
# at the first collation level, which reorders sections against the committed
# file and turns any regeneration into a whole-file diff.
export LC_ALL=C
for src in crates/gsc/tests/fixtures/semantics/probe_*.gsc; do
    name="$(basename "$src" .gsc)"
    echo "# $name"
    out="$(tools/run_probe.sh "$name" "${1:-mp_pavlov}")"
    # Keep the measurements and collapse the fatal block to its one message
    # line: the rest is a stack trace naming line numbers that move whenever
    # a probe is edited.
    echo "$out" | grep '^PROBE ' || true
    if echo "$out" | grep -q '^PROBE_FATAL'; then
        echo "$out" | sed -n "s/^PROBE_ERR \(.*\): (file .*/PROBE_FATAL \1/p" | head -1
    fi
    echo
done
