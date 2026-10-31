#!/usr/bin/env bash
# Retain exact-source stdout/stderr and the first failure; never retry into green.
set -euo pipefail
out=${1:?usage: capture-genesis-restart.sh OUTPUT_DIR [REPETITIONS]}
count=${2:-5}
[[ "$count" =~ ^([1-9]|1[0-9]|20)$ ]] || { echo 'repetitions must be 1..20' >&2; exit 2; }
mkdir -p "$out"
git rev-parse HEAD > "$out/source-sha.txt"
git diff --exit-code --quiet
git diff --cached --exit-code --quiet
{
  rustc --version
  uname -a
  ulimit -a
  df -Pk "$out"
} > "$out/environment.txt" 2>&1
for ((attempt=1; attempt<=count; attempt++)); do
  set +e
  RUST_BACKTRACE=full cargo test --locked -p rchain-node --test pinned_genesis \
    populated_dag_restart_accepts_its_pin_and_refuses_a_different_pin \
    -- --exact --nocapture --test-threads=1 > "$out/attempt-$attempt.log" 2>&1
  status=$?
  set -e
  printf '%s\n' "$status" > "$out/attempt-$attempt.exit-code"
  if ((status != 0)); then
    cat "$out/attempt-$attempt.log"
    exit "$status"
  fi
  printf 'pinned genesis restart %s/%s PASS\n' "$attempt" "$count"
done
