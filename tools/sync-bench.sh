#!/usr/bin/env bash
#
# sync-bench — measure a cold LFS restore against a **retained** mature chain.
#
# Why this exists. The 2026-11-14 C339 campaign rebuilt a 2,436-height chain from genesis before it
# could measure a restore, and then timed the restore on a host shared with another build. Both
# costs are avoidable: build each ladder point once, keep the bootstrap's data volume, and start the
# measured network from that volume with autopropose off, so the bootstrap serves state instead of
# competing with it.
#
# What it does:
#   1. For each requested height, if `synbuild<h>-bootstrap-data` does not exist, build a
#      one-validator chain to that height (autopropose) and stop the network **without** `-v`, which
#      retains the bootstrap's volume.
#   2. Run `tools/c339_mature_replay.py` `--runs` times against that volume: bootstrap from the
#      retained store, one empty validator wiped and cold-restored. The joiner is validator 1.
#   3. Print the run directories; pack the ones you want with `tools/evidence-pack.py`.
#
# The measured network uses its own prefix, network and ports, so it cannot collide with a devnet
# another session is running. The build network uses the same fixed ports for every height because
# the builds are sequential.
#
# Usage:
#   tools/sync-bench.sh --heights 256,756,2432 --runs 3 --out target/sync-bench
#   tools/sync-bench.sh --heights 756 --runs 1 --rebuild
#
# Requires: docker, curl, python3, and the `rnode:local` image (`tools/devnet.sh build`).

set -euo pipefail

cd "$(dirname "$0")/.."

HEIGHTS="256,756,2432"
RUNS=3
OUT="target/sync-bench"
REBUILD=false
BUILD_HTTP_BASE=61403
MEASURE_PREFIX="syncmeas"
MEASURE_NETWORK="syncmeas"
MEASURE_GRPC_BASE=62402
MEASURE_HTTP_BASE=62403
MEASURE_ADMIN_BASE=62405

while [[ $# -gt 0 ]]; do
  case "$1" in
    --heights) HEIGHTS="${2:?}"; shift 2 ;;
    --runs) RUNS="${2:?}"; shift 2 ;;
    --out) OUT="${2:?}"; shift 2 ;;
    --rebuild) REBUILD=true; shift ;;
    *) echo "sync-bench: unknown argument $1" >&2; exit 2 ;;
  esac
done

height_now() { # <http base>
  curl -s --max-time 5 "http://127.0.0.1:${1}/health" 2>/dev/null \
    | python3 -c 'import json,sys; print(json.load(sys.stdin).get("maxObservedHeight") or 0)' 2>/dev/null \
    || echo 0
}

build_chain() { # <height>
  local h="$1" prefix="synbuild${h}" vol="${prefix}-bootstrap-data" i
  if $REBUILD; then docker volume rm "$vol" >/dev/null 2>&1 || true; fi
  if docker volume inspect "$vol" >/dev/null 2>&1; then
    echo "==> ${h}: retained chain ${vol} present"
    return
  fi
  echo "==> ${h}: building a chain from genesis (one validator, autopropose)"
  DEVNET_PREFIX="$prefix" DEVNET_NETWORK="$prefix" \
  DEVNET_GRPC_BASE=$((BUILD_HTTP_BASE - 1)) DEVNET_HTTP_BASE="$BUILD_HTTP_BASE" \
  DEVNET_ADMIN_BASE=$((BUILD_HTTP_BASE + 2)) \
    tools/devnet.sh up --validators 1 --fresh --autopropose --admin
  for (( i = 0; i < 86400; i++ )); do
    if (( $(height_now "$BUILD_HTTP_BASE") >= h )); then break; fi
    sleep 2
  done
  if (( $(height_now "$BUILD_HTTP_BASE") < h )); then
    echo "sync-bench: chain did not reach height ${h}" >&2
    DEVNET_PREFIX="$prefix" DEVNET_NETWORK="$prefix" tools/devnet.sh down >/dev/null 2>&1 || true
    exit 1
  fi
  # Stop without -v: the bootstrap's data volume is the retained artifact.
  DEVNET_PREFIX="$prefix" DEVNET_NETWORK="$prefix" tools/devnet.sh down
  if ! docker volume inspect "$vol" >/dev/null 2>&1; then
    echo "sync-bench: ${vol} was not retained" >&2
    exit 1
  fi
  echo "==> ${h}: chain retained as ${vol}"
}

measure() { # <height> <run>
  local h="$1" run="$2"
  local vol="synbuild${h}-bootstrap-data"
  local dir="${OUT}/${h}-run${run}"
  echo "==> ${h}: cold restore run ${run} -> ${dir}"
  DEVNET_PREFIX="$MEASURE_PREFIX" DEVNET_NETWORK="$MEASURE_NETWORK" \
  DEVNET_GRPC_BASE="$MEASURE_GRPC_BASE" DEVNET_HTTP_BASE="$MEASURE_HTTP_BASE" \
  DEVNET_ADMIN_BASE="$MEASURE_ADMIN_BASE" \
    python3 tools/c339_mature_replay.py \
      --out "$dir" \
      --target-height "$h" \
      --validators 2 --validator 1 \
      --bootstrap-volume "$vol" \
      --no-autopropose --no-propose-on-deploy \
      --restore-timeout 1800
  DEVNET_PREFIX="$MEASURE_PREFIX" DEVNET_NETWORK="$MEASURE_NETWORK" \
    tools/devnet.sh down -v >/dev/null 2>&1 || true
  echo "    result: ${dir}/raw/result.json"
}

echo "==> sync-bench: heights ${HEIGHTS}, ${RUNS} run(s) each, out ${OUT}"
for h in ${HEIGHTS//,/ }; do
  build_chain "$h"
  for (( r = 1; r <= RUNS; r++ )); do
    measure "$h" "$r"
  done
done
echo "==> sync-bench: done. Pack a run with:"
echo "    python3 tools/evidence-pack.py pack --run-dir ${OUT}/<height>-run<run> \\"
echo "      --record spec/audit/evidence/sync-<height>-<date> --gate-id C339 \\"
echo "      --verdict PASS|FAIL|INCONCLUSIVE --locality local-devnet"
