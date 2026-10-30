#!/usr/bin/env bash
# cod2_soak.sh <outdir> [map...]: the CoD2 1.3 client soak
# (`play::cod2_soak`, docs/research/cod2-client-perf.md "The soak") on
# tools/cod4_soak.sh's runner, one map (PAIR=2: two) a process, every
# stock map without arguments. Needs COD2_DIR; the rest of the env as
# cod4_soak.sh's.
exec env GAME=cod2 "$(dirname "$0")/cod4_soak.sh" "$@"
