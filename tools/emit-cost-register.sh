#!/usr/bin/env bash
# Render the cost register into `spec/COST.md`, and refuse a stale rendering.
#
# **Why a rendered file and not just the TSV.** The register is *the* answer to "is this charge
# proportionate?", and the answer has to be readable by a person reviewing a diff: a table with the
# work beside the charge is what makes a shape claim reviewable. The TSV is the authored data; the
# rendering is what the gate compares, so the table cannot drift from the rows.
#
# Usage:  tools/emit-cost-register.sh          # write spec/COST.md
#         tools/emit-cost-register.sh --check  # exit 1 if the committed rendering is stale
# Exit:   0 ok · 1 stale or malformed · 2 usage
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TSV="$ROOT/spec/cost-register.tsv"
OUT="$ROOT/spec/COST.md"

check=0
case "${1:-}" in
  --check) check=1 ;;
  "") ;;
  *) printf 'usage: %s [--check]\n' "$0" >&2; exit 2 ;;
esac

[[ -f "$TSV" ]] || { printf 'emit-cost-register: no %s\n' "$TSV" >&2; exit 1; }

render() {
  cat <<'FRAME'
# The cost register

Every priced operation in the reducer and its builtins, the work it does, and the shape of the charge
against it. Authored in `spec/cost-register.tsv`, rendered here, checked by
`tools/check-cost-register.py` — an arm with no row, a row for nothing, and a declared charge that is
not where the row says are all build failures.

**What this is for.** The "flat charge for superlinear work" defect has been found twice — September
2026's union dedup, October 2026's collection conversions, union asymmetry and `censure` — and both
times by measurement, because nothing in the tree could see a charge that does not track its work. This
is that instrument's data. It makes each charge a *declared* shape a reviewer can read, and
`rholang/tests/cost_shapes.rs` measures the charge through the reducer so a shape that stops growing
fails. Passing the gate does not prove the work matches the charge: the charge is linear for `O(n log
n)` work, which is the recorded log-factor residual, and a `capped` operation is bounded by a constant
rather than priced by phlo.

| id | operation(s) | shape | basis | charge | evidence |
|---|---|---|---|---|---|
FRAME
  awk -F'\t' '
    NF == 10 && $1 != "id" {
      printf "| %s | `%s` | %s | %s | `%s` | %s |\n", $1, $3, $5, $6, $7, $8
    }
  ' "$TSV"
  printf '\nRows whose `residual` is not `-`:\n\n'
  awk -F'\t' '
    NF == 10 && $1 != "id" && $9 != "-" {
      printf "- **%s** (%s) — %s\n", $1, $9, $10
    }
  ' "$TSV"
}

rendered="$(mktemp)"
trap 'rm -f "$rendered"' EXIT
render > "$rendered"

if (( check )); then
  if ! diff -q "$rendered" "$OUT" >/dev/null 2>&1; then
    printf 'FAIL  %s is not what the register says — re-emit and commit:\n' "$OUT" >&2
    diff -u "$OUT" "$rendered" | sed -n '1,40p' >&2
    exit 1
  fi
  printf 'ok    %s matches spec/cost-register.tsv\n' "$OUT"
else
  mv "$rendered" "$OUT"
  printf 'emitted  %s (%s rows)\n' "$OUT" "$(($(awk -F'\t' 'NF==10 && $1!="id"' "$TSV" | wc -l) - 0))"
fi
