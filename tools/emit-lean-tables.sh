#!/usr/bin/env bash
#
# emit-lean-tables.sh — regenerate the C343/C357 contract tables from the Lean.
#
# `spec/audit/phase-diagnosis.tsv`, `phase-diagnosis-cases.tsv`, `c357-acceptance.tsv` and
# `c357-acceptance-cases.tsv` are committed because the collector and the acceptance checker read
# them. `lake exe rchain-tables` is the one writer: the Python tools read these files and do not
# carry a second copy of the priority or the requirement list.
#
# Usage: tools/emit-lean-tables.sh [--check]
set -euo pipefail

ulimit -s 65536 2>/dev/null || true

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPEC="$ROOT/spec"
TABLES=(
  spec/audit/phase-diagnosis.tsv
  spec/audit/phase-diagnosis-cases.tsv
  spec/audit/c357-acceptance.tsv
  spec/audit/c357-acceptance-cases.tsv
)

( cd "$SPEC" && lake exe rchain-tables --out-dir "$SPEC/audit" )
for t in "${TABLES[@]}"; do
  printf 'emitted  %s\n' "$ROOT/$t"
done

if [[ "${1:-}" == "--check" ]]; then
  dirty="$(cd "$ROOT" && git status --porcelain -- "${TABLES[@]}")"
  if [[ -z "$dirty" ]]; then
    echo "ok    the committed contract tables match the Lean"
  else
    echo "FAIL  a contract table is not what the Lean says — re-emit and commit:" >&2
    printf '%s\n' "$dirty" | sed 's/^/      /' >&2
    exit 1
  fi
fi
