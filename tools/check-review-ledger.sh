#!/usr/bin/env bash
#
# check-review-ledger — the coverage claim is a claim about the tree, so check it against the tree.
#
# **Why it exists.** The ledger's own header says it: *"nothing regenerates this file any more, and
# nothing checks it against the tree."* Its checker, `tools/audit-test-register.sh` (1,453 lines, 78 s,
# 17 checks), was deleted 2026-09-27 with the rest of the gate apparatus — and behind it the census went
# stale. So `all 89 T1 modules read`, which `tools/audit-status.sh` prints and `spec/AUDIT.md` renders, is
# a claim about *modules that can fork the chain or lose funds* that nothing verified.
#
# **This is the cheap half of that check, and deliberately not the whole of it.** It enforces what the
# headline actually rests on:
#
#   1. `ceiling <kind> <n>` equals the number of rows of that kind — the ceilings are hand-maintained
#      today, and a ceiling that does not match its rows is the drift the old join existed to refuse;
#   2. every **T1** row names something that exists — a tracked path, or a law id in `spec/laws.tsv` —
#      so "89 read" is a count of real things;
#   3. the census' completeness is **printed, not gated**: tracked `.rs` files with no row at all.
#
# Why (3) is not a gate: it demands a row for every file added since the last seeding, and a gate that
# can only be satisfied by bookkeeping is the thing this check is meant to avoid. The headline's claim is
# about the T1 tier, so the tier is what is refused; the roster is what is reported.
#
# Usage: tools/check-review-ledger.sh          # report
#        tools/check-review-ledger.sh --gate   # exit non-zero on a violation (for CI)

set -uo pipefail
cd "$(dirname "$0")/.."

LEDGER=spec/review-ledger.tsv
LAWS=spec/laws.tsv
[[ -r "$LEDGER" ]] || { echo "check-review-ledger: no $LEDGER" >&2; exit 2; }

violations=0
note() { printf '  %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; violations=$((violations + 1)); }

echo "check-review-ledger: the coverage claim against the tree"
echo

# --- 1. the ceilings are the counts ---------------------------------------------------------------
# `ceiling <kind> <n>` rows are the ledger's own ratchet: a kind cannot silently grow or shrink without
# the number beside it moving, which is what makes the census' size a statement rather than a residue.
while read -r _ kind declared; do
  actual=$(awk -F'\t' -v k="$kind" '$1 == k' "$LEDGER" | wc -l)
  if [[ "$declared" != "$actual" ]]; then
    fail "ceiling $kind says $declared, the ledger holds $actual row(s)"
  else
    printf '  ok    ceiling %-9s %s\n' "$kind" "$actual"
  fi
done < <(awk -F'\t' '$1 == "ceiling"' "$LEDGER")

# --- 2. every T1 row names something that exists ---------------------------------------------------
# The claim is "all N T1 modules read". A row whose subject has been renamed or deleted makes that a
# count of things that are not there, which is the shape the four rows the header admits to already have.
while IFS=$'\t' read -r kind id tier verdict _rest; do
  [[ "$tier" == "T1" ]] || continue
  case "$kind" in
    law)
      # A law row's id is `number` + optional `clause` (`10`, `1a`, `26c`), and `laws.tsv` carries them
      # in two columns — so the id is split rather than grepped as one token.
      num="${id%%[a-z]*}"; clause="${id#"$num"}"
      awk -F'\t' -v n="$num" -v c="$clause" '$1 == n && $2 == c { found = 1 } END { exit !found }' "$LAWS" \
        || fail "T1 law row '$id' is not in $LAWS"
      ;;
    file)
      git ls-files --error-unmatch "$id" >/dev/null 2>&1 \
        || fail "T1 file row '$id' is not tracked (verdict: $verdict)"
      ;;
    *)
      # `config` rows name a HOCON key and the others name things outside the tree; only the two kinds
      # the claim is actually made of are checked by path.
      ;;
  esac
done < <(awk -F'\t' '$1 != "ceiling" && $1 !~ /^#/' "$LEDGER")
printf '  ok    every T1 row names something that exists (%s row(s))\n' \
  "$(awk -F'\t' '$3 == "T1"' "$LEDGER" | wc -l)"

# --- 3. the process roster is the code's roster ----------------------------------------------------
# **The contract layer's roster is derived, not hand-maintained** (C301's close-out). A review that is
# a list of names is only as good as the list: the ledger held 30 process rows while the node
# installed 31 definitions — `rho:rchain:ertp:ledger` (issue #249) had no row — which is exactly the
# drift the roster-self row recorded and nothing refused. The set is read from the `urn:` strings in
# the definitions the node installs, so a new system process, a removed one, or a renamed URN fails
# here until the ledger is updated. The config and tool rosters remain authored: their ids are not
# the tree's names (a ledger `api-host` is `api-server.host`), and a derivation that guessed the
# mapping would be the bookkeeping gate this check exists to avoid.
derived_processes=$(grep -oE 'urn: "[^"]+"' rholang/src/system_processes.rs | cut -d'"' -f2 | sort -u)
ledgered_processes=$(awk -F'\t' '$1 == "process" { print $2 }' "$LEDGER" | sort -u)
process_diff=$(comm -3 <(printf '%s\n' "$derived_processes") <(printf '%s\n' "$ledgered_processes"))
if [[ -n "$process_diff" ]]; then
  fail "the process roster differs from the code (left: code, right: ledger): $(printf '%s ' "$process_diff")"
else
  printf '  ok    the process roster is the code'"'"'s (%s URN(s))\n' \
    "$(printf '%s\n' "$derived_processes" | wc -l)"
fi

# --- 4. the census' completeness, reported ---------------------------------------------------------
# Not a gate, by the reasoning in the header. But it is the number that says whether the roster is
# current, and a number nobody prints is the silence this whole register keeps replacing.
git ls-files '*.rs' | grep -v '^legacy/' | sort > /tmp/ledger_tracked.$$
awk -F'\t' '$1 == "file" { print $2 }' "$LEDGER" | sort -u > /tmp/ledger_rows.$$
missing=$(comm -23 /tmp/ledger_tracked.$$ /tmp/ledger_rows.$$ | wc -l)
total=$(wc -l < /tmp/ledger_tracked.$$)
rm -f /tmp/ledger_tracked.$$ /tmp/ledger_rows.$$
printf '  note  %s of %s tracked .rs files have no row (census seeded 2026-09-27)\n' "$missing" "$total"

echo
if (( violations > 0 )); then
  cat >&2 <<'EOF'
check-review-ledger: the coverage claim does not hold against the tree.

The claim `all N T1 modules read` is printed by `tools/audit-status.sh` and rendered into
`spec/AUDIT.md`, and it is the one place the audit says what it has *not* covered. A ceiling that does
not match its rows, or a T1 row naming a file that is gone, makes that claim a count of things that are
not there.

Fix the ledger (the data), not this check: correct the `ceiling` line, or re-point the row at what the
module became, or mark it `exempt` with a `deleted` reason and a note saying what it was.
EOF
  [[ "${1:-}" == "--gate" ]] && exit 1
  exit 0
fi

echo "check-review-ledger: the ceilings match their rows, every T1 row names something real, and the process roster is the code's."
