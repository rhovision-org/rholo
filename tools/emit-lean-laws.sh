#!/usr/bin/env bash
#
# emit-lean-laws.sh — regenerate the law register's emitted artefacts from the Lean specification.
#
# `spec/laws.tsv` (the data) and `spec/LAWS.md` (the tables) are committed, because they are what the
# documents and any other consumer are checked against: `tools/check-lean-conformance.sh` re-emits them
# and fails if `git status` shows a change. This is the "write the new truth" half of that pair — run it
# when the *register* changed on purpose (`spec/Rchain/Laws.lean`), and read the diff.
#
# The *checks* are not here. `Rchain/LawsMain.lean` verifies numbering, reference integrity and axiom
# accounting at compile time (it needs the elaborated environment), so `lake build rchain-laws` fails
# before this script ever runs. This script only writes what that register says.
#
# Usage: tools/emit-lean-laws.sh [--check]
set -euo pipefail

# `Rchain/Sort.lean`'s comparator blocks need more than the default 8 MB process stack (measured
# 2026-09-24: 8 MB aborts with "Stack overflow detected", 64 MB elaborates the same file cleanly), and
# `lake exe` builds before it runs. Same fix, same comment, as `tools/check-lean-conformance.sh`.
ulimit -s 65536 2>/dev/null || true

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPEC="$ROOT/spec"

( cd "$SPEC" && lake exe rchain-laws --out "$SPEC/laws.tsv" )
printf 'emitted  %s (%s rows, plus a header)\n' "$SPEC/laws.tsv" "$(($(wc -l < "$SPEC/laws.tsv") - 1))"

( cd "$SPEC" && lake exe rchain-laws --format md --out "$SPEC/LAWS.md" )
printf 'emitted  %s (%s lines)\n' "$SPEC/LAWS.md" "$(wc -l < "$SPEC/LAWS.md")"

( cd "$SPEC" && lake exe rchain-laws --format vocabulary --out "$SPEC/progress-vocabulary.tsv" )
printf 'emitted  %s (%s rows, plus a header)\n' "$SPEC/progress-vocabulary.tsv" \
  "$(($(wc -l < "$SPEC/progress-vocabulary.tsv") - 1))"

( cd "$SPEC" && lake exe rchain-laws --format crosswalk --out "$SPEC/progress-crosswalk.tsv" )
printf 'emitted  %s (%s rows, plus a header)\n' "$SPEC/progress-crosswalk.tsv" \
  "$(($(wc -l < "$SPEC/progress-crosswalk.tsv") - 1))"

( cd "$SPEC" && lake exe rchain-laws --format crosswalk-md --out "$SPEC/PROGRESS-CROSSWALK.md" )
printf 'emitted  %s (%s lines)\n' "$SPEC/PROGRESS-CROSSWALK.md" \
  "$(wc -l < "$SPEC/PROGRESS-CROSSWALK.md")"

if [[ "${1:-}" == "--check" ]]; then
  # `git status --porcelain` rather than `git diff`, for the reason the corpus check gives: an untracked
  # file has no diff to show, and a check that passes on an uncommitted artefact is the same check that
  # would pass on a stale one.
  dirty="$(cd "$ROOT" && git status --porcelain -- spec/laws.tsv spec/LAWS.md \
    spec/progress-vocabulary.tsv spec/progress-crosswalk.tsv spec/PROGRESS-CROSSWALK.md)"
  if [[ -z "$dirty" ]]; then
    echo "ok    the committed law register matches the Lean"
  else
    echo "FAIL  a committed register artefact is not what the Lean says — re-emit and commit:" >&2
    printf '%s\n' "$dirty" | sed 's/^/      /' >&2
    exit 1
  fi
fi
