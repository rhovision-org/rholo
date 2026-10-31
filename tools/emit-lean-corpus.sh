#!/usr/bin/env bash
#
# emit-lean-corpus.sh — regenerate the conformance corpora from the Lean specification.
#
# The corpora under `spec/conformance/` are committed, because they are what the Rust is checked
# against: `tools/check-lean-conformance.sh` re-emits them and fails if `git diff` shows a change, so a
# corpus that no longer matches the model cannot be committed-and-forgotten. This script is the "write
# the new truth" half of that pair; run it when the *model* changed on purpose, and read the diff.
#
# Usage: tools/emit-lean-corpus.sh [--check]
set -euo pipefail

# `Rchain/Sort.lean`'s comparator blocks need more than the default 8 MB process stack (measured
# 2026-09-24: 8 MB aborts with "Stack overflow detected", 64 MB elaborates the same file cleanly), and
# `lake exe` builds before it runs. Same fix, same comment, as `tools/check-lean-conformance.sh`.
ulimit -s 65536 2>/dev/null || true

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPEC="$ROOT/spec"
OUT="$SPEC/conformance"

# One entry per corpus layer: `<layer>` maps to `lake exe rchain-corpus --layer <layer>`.
LAYERS=(flags match silence store c21 protocol json syrup envelope lex sort parse body closed stake block liveness admission decimal hostlist recovery)

mkdir -p "$OUT"
for layer in "${LAYERS[@]}"; do
  ( cd "$SPEC" && lake exe rchain-corpus --layer "$layer" --out "$OUT/$layer.tsv" )
  printf 'emitted  %s (%s lines)\n' "$OUT/$layer.tsv" "$(wc -l < "$OUT/$layer.tsv")"
done

if [[ "${1:-}" == "--check" ]]; then
  if ( cd "$ROOT" && git diff --exit-code -- spec/conformance >/dev/null 2>&1 ); then
    echo "ok    the committed corpora match the Lean definitions"
  else
    echo "FAIL  spec/conformance is stale — commit the re-emitted corpus (it IS the check)" >&2
    exit 1
  fi
fi
