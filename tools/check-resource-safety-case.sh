#!/usr/bin/env bash
# check-resource-safety-case — the resource safety case's own completeness claim, checked against the page.
#
# Why this exists. The safety case is a claim about a document: every hazard is dispositioned against a
# threat, at least one barrier, a consequence, a residual risk and an evidence item; and it never claims
# release readiness. Prose is not a check, so this reads the page.
#
# Contract (docs/src/spec/resource-safety-case.md):
#   | ID | Threat | Existing barrier | New barrier | Consequence | Residual | Evidence |
#   | H-RS-01 | T-RS-1 ... | ... | B31 | C-RS-1 ... | ... | owed: unit A2 — path::test |
#
# Checks:
#   1  the page carries the required sections: Top event, Hazard log, Barrier independence, ALARP
#      argument, Residual risk, Non-claims
#   2  at least ten H-RS rows exist (the scan is not vacuous)
#   3  every row names a T-RS- threat, a C-RS- consequence, a non-empty existing barrier, a non-empty
#      new barrier (or an explicit "out of scope"/"none" disposition) and a non-empty residual
#   4  every Evidence cell is either `owed:` (visible debt, counted) or a `path::symbol` whose file
#      exists and whose symbol occurs in it
#   5  no Evidence cell claims a pass, certification or release authorization
#   6  --require-evidence refuses every `owed:` cell (used once the programme's units have landed)
#   7  the HAZOP study node U17 exists in docs/src/spec/failure-hazop.md
#
# Usage: tools/check-resource-safety-case.sh [--gate] [--require-evidence] [<page>]
#   --gate             exit non-zero on a violation (CI)
#   --require-evidence eliminate the `owed:` allowance (the completion gate)
#   without --gate     print the report; violations still name themselves; exit 0
set -euo pipefail

gate=0
require_evidence=0
page=""
for arg in "$@"; do
  case "$arg" in
    --gate) gate=1 ;;
    --require-evidence) require_evidence=1 ;;
    -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
    *) page="$arg" ;;
  esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
page="${page:-$root/docs/src/spec/resource-safety-case.md}"
hazop="$root/docs/src/spec/failure-hazop.md"
[[ -r "$page" ]] || { echo "check-resource-safety-case: no page at $page" >&2; exit 2; }

bad=0
problem() { printf '  VIOLATION  %s\n' "$1"; bad=$((bad + 1)); }

for heading in \
  "## Top event" \
  "## Hazard log" \
  "## Barrier independence" \
  "## ALARP argument" \
  "## Residual risk" \
  "## Non-claims"
do
  grep -qF -- "$heading" "$page" || problem "missing required section: $heading"
done

grep -qF -- "### U17 —" "$hazop" || problem "HAZOP study node U17 is missing from failure-hazop.md"
grep -qE -- "^\| \*\*F-U17-" "$hazop" || problem "failure-hazop.md has no F-U17 rows"

echo "check-resource-safety-case: $page"
awk -v root="$root" -v require_evidence="$require_evidence" '
  BEGIN { FS = "|"; rows = 0; owed = 0; bad = 0 }
  function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
  function p(msg) { printf "  VIOLATION  %s\n", msg; bad++ }
  /^\| H-RS-/ {
    rows++
    if (NF != 9) { p("row " $2 ": expected 7 cells, got " (NF - 2)); next }
    id = trim($2); threat = trim($3); existing = trim($4); newer = trim($5)
    consequence = trim($6); residual = trim($7); evidence = trim($8)
    if (id !~ /^H-RS-[0-9]+$/) p("bad id [" id "]")
    if (threat !~ /T-RS-/) p(id ": threat does not name a T-RS- id")
    if (consequence !~ /C-RS-/) p(id ": consequence does not name a C-RS- id")
    if (existing == "") p(id ": empty existing-barrier cell")
    if (newer == "") p(id ": empty new-barrier cell")
    if (residual == "") p(id ": empty residual cell")
    if (evidence == "") { p(id ": empty evidence cell"); next }
    if (tolower(evidence) ~ /(pass|certif|release authori)/) p(id ": evidence claims a pass/certification")
    if (evidence ~ /^owed:/) {
      owed++
      if (require_evidence) p(id ": evidence is still owed: " evidence)
      next
    }
    # path::symbol, both must exist
    okp = 0; oksym = 0; anycol = 0; lastp = ""; lastsym = ""
    m = evidence
    while (match(m, /[A-Za-z0-9_.\/-]+\.(rs|lean|md|sh|tsv|txt|json|toml|v)/)) {
      path = substr(m, RSTART, RLENGTH); m = substr(m, RSTART + RLENGTH)
      sub(/:.*$/, "", path)
      if (system("test -e \"" root "/" path "\"") != 0) continue
      okp = 1; lastp = path
      if (substr(m, 1, 2) == "::") {
        anycol = 1
        if (match(substr(m, 3), /^[A-Za-z0-9_]+/)) {
          sym = substr(substr(m, 3), RSTART, RLENGTH); lastsym = sym
          if (system("grep -qF -- \"" sym "\" \"" root "/" path "\"") == 0) oksym = 1
        }
      }
    }
    if (!okp) p(id ": evidence names no path that exists in this tree")
    else if (anycol && !oksym) p(id ": evidence names symbol [" lastsym "] absent from " lastp)
  }
  END {
    if (rows < 10) p("only " rows " H-RS rows; the scan would pass vacuously")
    printf "\n  totals: %d hazards, %d evidence items owed, %d violations\n", rows, owed, bad
    exit (bad > 0)
  }
' "$page"
status=$?

if (( status == 0 )); then
  echo "check-resource-safety-case: every hazard names a threat, a barrier, a consequence, a residual and evidence."
  exit 0
fi
if (( gate == 1 )); then
  echo "check-resource-safety-case: the safety case does not hold." >&2
  exit 1
fi
echo "check-resource-safety-case: report only (use --gate to fail)."
exit 0
