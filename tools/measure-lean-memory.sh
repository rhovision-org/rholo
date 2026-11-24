#!/usr/bin/env bash
#
# measure-lean-memory.sh — the peak RSS of each law-register chunk, measured on its own.
#
# `spec/Rchain/Laws.lean` is assembled from one module per law-number range
# (`spec/Rchain/Laws/Chunk01..09.lean`). The split exists because a single module holding all 187
# entries elaborated at 15.1 GiB RSS on the 16 GiB CI runner (measured 2026-11-23: `ps` peak
# 15,102,104 KiB); a `def` boundary inside one module does not bound that memory, but a module
# boundary does, because Lake invokes a separate `lean` process per module.
#
# This tool compiles each chunk with the same `lean` invocation Lake uses (olean, ilean and C output),
# but writes those artefacts to a temporary directory, so it measures the build without touching the
# tree. It is the local counterpart to the formal gate's peak guard, and the tool to run in a
# constrained container before concluding a register change is too large. It needs GNU time
# (`/usr/bin/time -v`); the CI runner has it.
#
# Usage:
#   tools/measure-lean-memory.sh                 # print the table
#   tools/measure-lean-memory.sh --gate 4096     # and fail if any chunk peaks above 4096 MiB
set -euo pipefail

# Same stack budget as the gate: `Rchain/Sort.lean`'s comparator blocks need more than 8 MB.
ulimit -s 65536 2>/dev/null || true

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPEC="$ROOT/spec"

gate_mb=0
if [[ "${1:-}" == "--gate" ]]; then
  gate_mb="${2:-}"
  if [[ ! "$gate_mb" =~ ^[0-9]+$ ]]; then
    echo "usage: $0 [--gate <MiB>]" >&2
    exit 2
  fi
fi

if ! command -v /usr/bin/time >/dev/null 2>&1; then
  echo "FAIL  /usr/bin/time is not installed; this measurement needs GNU time -v" >&2
  exit 2
fi

cd "$SPEC"

# The chunks import only Base; build that one olean and nothing else.
if ! lake build Rchain.Laws.Base >/tmp/measure-lean-base.log 2>&1; then
  echo "FAIL  lake build Rchain.Laws.Base — see /tmp/measure-lean-base.log" >&2
  tail -n 40 /tmp/measure-lean-base.log >&2 || true
  exit 1
fi

printf '%-28s %5s %10s\n' module rows 'peak MiB'
export LEAN_PATH="$(lake env printenv LEAN_PATH)"
export LD_LIBRARY_PATH="$(lake env printenv LD_LIBRARY_PATH || true)"
LEAN="$(elan which lean 2>/dev/null || command -v lean)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
worst=0
status=0
launch=()
if command -v taskset >/dev/null 2>&1; then
  launch=(taskset -c 0-3)
fi
for file in Rchain/Laws/Chunk*.lean; do
  [[ -e "$file" ]] || continue
  module="${file%.lean}"
  module="${module//\//.}"
  rows="$(grep -c 'number := ' "$file" || true)"
  log="$(mktemp)"
  if /usr/bin/time -v "${launch[@]}" "$LEAN" -DautoImplicit=false "$file" -R . \
      -o "$work/$module.olean" -i "$work/$module.ilean" -c "$work/$module.c" \
      >"$log" 2>&1; then
    peak_kb="$(awk '/Maximum resident set size/ { print $NF }' "$log")"
    peak_mb=$((peak_kb / 1024))
    printf '%-28s %5s %10s\n' "$module" "$rows" "$peak_mb"
    (( peak_mb > worst )) && worst=$peak_mb
    if (( gate_mb > 0 && peak_mb > gate_mb )); then
      echo "FAIL  $module peaked at ${peak_mb} MiB, over the ${gate_mb} MiB budget — split this chunk at the next law-number boundary" >&2
      status=1
    fi
  else
    echo "FAIL  $module did not build — see below" >&2
    tail -n 30 "$log" >&2 || true
    status=1
  fi
  rm -f "$log"
done

printf '\nworst chunk: %s MiB\n' "$worst"
if (( gate_mb > 0 && status == 0 )); then
  echo "ok    every chunk is under the ${gate_mb} MiB budget"
fi
exit "$status"
