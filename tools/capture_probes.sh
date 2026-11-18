#!/usr/bin/env bash
# Runs every probe in crates/gsc/tests/fixtures/semantics/ and writes the
# combined capture the A/B test reads. One `# <probe>` header per probe,
# then that probe's PROBE lines, then `PROBE_FATAL <message>` if the script
# killed the server.
#
# Same environment as tools/run_probe.sh. Takes a couple of minutes: each
# probe boots the retail server. A server that never reaches the level stops
# the capture (run_probe.sh exits nonzero), rather than writing an empty
# section.
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
    # a probe is edited. A message that ends in its own newline (the
    # precache builtins' "must be called before any wait") puts the
    # `: (file` on the next line, so the message is the line after the
    # header either way.
    echo "$out" | grep '^PROBE ' || true
    if echo "$out" | grep -q '^PROBE_FATAL'; then
        echo "$out" | awk '/^PROBE_ERR \*+ script runtime error/ {
            getline; sub(/^PROBE_ERR /, ""); sub(/: \(file .*/, "")
            print "PROBE_FATAL " $0; exit }'
    fi
    echo
done
