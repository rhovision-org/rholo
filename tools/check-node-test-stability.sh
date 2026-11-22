#!/usr/bin/env bash
# Re-run the node test binaries that have faulted in CI, and stop on the first fault.
#
# `ocapn_listener` and `node_api` each died with `signal: 11` on `dev` (2026-11-22), on different
# tests and without a Rust panic line. A single green run is not evidence against an intermittent
# fault: the loop is the check. On failure, capture a core and a backtrace before re-running —
# a passing retry is not a fix.
#
# Usage: tools/check-node-test-stability.sh [runs]
set -euo pipefail

runs="${1:-12}"
targets=(node_api ocapn_listener gateway_routes)

cd "$(dirname "$0")/.."

for run in $(seq 1 "$runs"); do
  echo "=== node-test stability run $run/$runs"
  if ! cargo test --locked -p rchain-node \
      --test node_api --test ocapn_listener --test gateway_routes \
      -- --test-threads=10; then
    echo "A node test binary faulted on run $run." >&2
    echo "Capture a core/backtrace (gdb --batch -ex run -ex 'thread apply all bt full')" >&2
    echo "on the failing test before re-running. A green retry is not a fix." >&2
    exit 1
  fi
done

echo "all $runs runs passed"
