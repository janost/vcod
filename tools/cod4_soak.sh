#!/usr/bin/env bash
# cod4_soak.sh <outdir> [map...]: the CoD4 client soak
# (`play::cod4_sweep::cod4_soak`, docs/research/cod4-map-sweep.md), one
# (PAIR=2: two) a process, every stock map without arguments. GAME=cod2
# runs the CoD2 one instead (`play::cod2_soak`, tools/cod2_soak.sh). Each group's
# `soak.md` and `warnings.txt` land in <outdir>/<map>[+<map>]/, its log
# beside it; a group that fails (a panic, a kill) is listed at the end
# and the next one runs.
#
# Env:
#   COD4_DIR      the 1.7 install (required; COD2_DIR the 1.3 one with GAME=cod2)
#   WRAP          a command each map's run goes through (a memory-capped
#                 scope, `nice`), word split; default `nice -n 10`
#   VCOD_SOAK_MINUTES, VCOD_SOAK_RENDER_HZ, VCOD_SWEEP_GAMETYPES,
#   VCOD_SWEEP_LEVEL_SECS, VCOD_SOAK_TICK_LOG_MS, VCOD_SOAK_CLIENT_LOG_MS,
#   VCOD_SOAK_TICK_WATCH
#   pass through (the test's header says what they do)
#   VK_DRIVER_FILES / WGPU_BACKEND pass through: the soak draws, so it
#   needs a Vulkan adapter with BC textures (lavapipe will do, with
#   GALLIVM_PERF=nopt; LP_NUM_THREADS defaults to 4 here).
# The run is pinned to cores 0-7 (`taskset`), which also caps the nav
# build's workers.
set -u
out=${1:?usage: cod4_soak.sh <outdir> [map...]}
shift
: "${GAME:=cod4}"
case $GAME in
  cod4) : "${COD4_DIR:?set COD4_DIR to the CoD4 1.7 install}" ;;
  cod2) : "${COD2_DIR:?set COD2_DIR to the CoD2 1.3 install}" ;;
  *) echo "soak: GAME is cod4 or cod2" >&2; exit 2 ;;
esac
here=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
maps=("$@")
if [ ${#maps[@]} -eq 0 ] && [ "$GAME" = cod4 ]; then
  for f in "$COD4_DIR"/zone/english/mp_*.ff; do
    m=$(basename "$f" .ff)
    case $m in *_load) continue ;; esac
    maps+=("$m")
  done
elif [ ${#maps[@]} -eq 0 ]; then
  while read -r m; do maps+=("$m"); done < <(
    for f in "$COD2_DIR"/main/iw_*.iwd; do unzip -Z1 "$f"; done |
      sed -n 's|^maps/mp/\(mp_[^/]*\)\.d3dbsp$|\1|p' | sort -u)
fi
export LP_NUM_THREADS=${LP_NUM_THREADS:-4}
read -r -a wrap <<<"${WRAP:-nice -n 10}"
# One build, copied out, so the checkout can change under a long run.
bin=$(cd "$here" && "${wrap[@]}" cargo test -p vcod --bins --no-run 2>&1 |
  sed -n 's/.*Executable unittests src\/main.rs (\(.*\))/\1/p' | tail -1)
[ -n "$bin" ] || { echo "soak: build failed" >&2; exit 1; }
cp "$here/$bin" "$out/vcod-tests"
# One map a process; PAIR=2 runs two (`VCOD_SWEEP_JOBS`, at most 2).
per=${PAIR:-1}
failed=()
i=0
while [ $i -lt ${#maps[@]} ]; do
  group=$(IFS=,; echo "${maps[*]:i:per}")
  i=$((i + per))
  echo "soak: $group" >&2
  if ! (cd "$here/crates/client" && VCOD_SWEEP_MAPS=$group VCOD_SWEEP_JOBS=$per \
      VCOD_SWEEP_OUT="$out/${group//,/+}" "${wrap[@]}" taskset -c 0-7 "$out/vcod-tests" \
      --ignored "${GAME}_soak" --nocapture --test-threads=1) >"$out/${group//,/+}.log" 2>&1; then
    failed+=("$group")
  fi
done
if [ ${#failed[@]} -gt 0 ]; then
  echo "soak: failed: ${failed[*]}" >&2
  exit 1
fi
