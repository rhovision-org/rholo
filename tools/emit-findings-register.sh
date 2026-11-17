#!/usr/bin/env bash
# Render the audit check-off into `spec/AUDIT.md` from `spec/findings.tsv` and the review ledger.
#
# **Why this exists, in one measurement.** `spec/AUDIT.md` was 4,366 lines organised by audit *pass*,
# and a finding's state was a bolded phrase at an arbitrary position inside a 2 KB cell -- `**Fixed`
# 108 times, `**Decided` 9, `**Registered` 4, and a different vocabulary on top in each pass section.
# So "what is left?" took twenty-one sections and a hand merge, nothing could answer it, and the answer
# nobody could see was that six of §13's Low findings had sat unaddressed since pass 4.
#
# Three states now, in the order a person works: `todo`, `in progress`, `done`. The state is authored
# in the TSV; this renders it; `--check` refuses a check-off that disagrees.
#
# Usage:
#   tools/emit-findings-register.sh            # write the check-off into spec/AUDIT.md
#   tools/emit-findings-register.sh --check    # exit 1 if the committed check-off is stale
#
# Exit: 0 ok · 1 stale or malformed · 2 usage.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TSV="$ROOT/spec/findings.tsv"
AUDIT="$ROOT/spec/AUDIT.md"
LEDGER="$ROOT/spec/review-ledger.tsv"

# Everything below this marker in `spec/AUDIT.md` is generated; everything above it is the frame a
# human wrote. Splitting on a marker rather than on a line number means the frame can grow without
# the emitter and the file disagreeing about where the check-off starts.
MARKER='<!--EMPTY: the emitter writes the check-off below this line-->'
# **`|`-separated, not space-separated, because one of the states has a space in it.** The first
# version split the list on spaces, so the accepted tokens were `todo`, `in`, `progress` and `done` --
# and `in progress`, which is the state a person is actually in when they set one, was rejected by the
# vocabulary it belongs to. `tools/todo.sh start` is what found it, on its first run.
VOCAB="todo|in progress|done"

check=0
case "${1:-}" in
  --check) check=1 ;;
  "") ;;
  *) printf 'usage: %s [--check]\n' "$0" >&2; exit 2 ;;
esac

[[ -f "$TSV" ]] || { printf 'emit-findings-register: no %s\n' "$TSV" >&2; exit 1; }

rows="$(grep -v '^#' "$TSV" | grep -v '^[[:space:]]*$')"
n_total=$(printf '%s\n' "$rows" | wc -l)

# --- the state vocabulary, and the condition each word carries -----------------------------------
#
# **Parsed by awk, not by `read`.** `read` with `IFS=$'\t'` collapses runs of the delimiter, because
# tab is *IFS whitespace* -- so a row with an empty `alias` cell had every field shifted left and the
# state arrived as the title. The first run of this check reported all 197 rows as out of vocabulary,
# which was the check's own defect and not the register's.
bad_vocab="$(printf '%s\n' "$rows" | awk -F'\t' -v vocab="$VOCAB" '
  BEGIN { n = split(vocab, v, "|"); for (i = 1; i <= n; i++) ok[v[i]] = 1 }
  { if (!($4 in ok)) printf " %s=%s", $1, ($4 == "" ? "(empty)" : $4) }')"
bad_shape="$(printf '%s\n' "$rows" | awk -F'\t' '
  NF != 7 && NF != 8 { printf " %s(%d fields)", $1, NF }
  $3 == "" || $4 == "" || $5 == "" { printf " %s(empty)", $1 }
  # **The id must start with a letter**, and this is a check the register needed for two months and
  # did not have. Every other column is validated -- fields, vocabulary, section, evidence, the owed
  # cell -- and the one identifier a *citation* resolves through was not, so a row written as `290`
  # instead of `C290` passed every gate and rendered into `spec/AUDIT.md` as `` `290` ``, while two
  # documents cited `C290`. A citation that resolves to nothing is the class this register exists to
  # refuse, and nothing was asking.
  $1 !~ /^[A-Z]/ { printf " %s(not an id)", $1 }')"

# **A `todo` row that does not say what would close it is the defect this column exists for.** The
# register recorded five findings as "owed" without saying what was owed, which is how a row stays
# open for a year and reads as addressed.
todo_unowed="$(printf '%s\n' "$rows" | awk -F'\t' '$4 == "todo" && ($7 == "-" || $7 == "") { printf " %s", $1 }')"

# --- the law column (8th): which law a finding is an instance of ---------------------------------
#
# **A finding that names no law is a defect of the register rather than of the code.** Four findings of
# 2026-09-28/29 (C170-C174) were the same property failing, argued in prose and filed as one-offs, and
# nothing could tell an instance of a known property from a novelty; the column is that tell. It is
# validated in both directions -- a name that resolves to nothing is refused, the `evidence` check's
# lesson -- and the residue is a **number printed on every run** rather than silence, which is how the
# T1 coverage half already turns its own gap into a count.
#
# **The ceiling is a ratchet.** It may be lowered as rows are classified and may not be raised: a change
# that adds rows naming no law is exactly what it exists to make visible, and the open rows (a `todo`)
# must name one, because an open finding without a law cannot be classified at all.
LAWLESS_CEILING="${LAWLESS_CEILING:-206}"
LAWS_TSV="$ROOT/spec/laws.tsv"
[[ -f "$LAWS_TSV" ]] || {
  printf 'emit-findings-register: no %s, so a law citation cannot be resolved\n' "$LAWS_TSV" >&2
  exit 1
}
# **By column name, not by index** — the register's own lesson about `rustWitness`, where `$14` was a
# fact about the writer that the reader could not check.
_lawcol() { head -1 "$LAWS_TSV" | tr '\t' '\n' | grep -nx "$1" | cut -d: -f1; }
num_col="$(_lawcol number)"
clause_col="$(_lawcol clause)"
if [[ -z "$num_col" || -z "$clause_col" ]]; then
  printf 'emit-findings-register: %s has no `number`/`clause` column\n' "$LAWS_TSV" >&2
  exit 1
fi
laws_known="$(awk -F'\t' -v n="$num_col" -v c="$clause_col" 'NR > 1 { printf "%s%s\n", $n, $c }' "$LAWS_TSV" | sort -u)"
lawless="$(printf '%s\n' "$rows" | awk -F'\t' '$4 == "done" && ($8 == "" || $8 == "-")' | wc -l)"
laws_named="$(printf '%s\n' "$rows" | awk -F'\t' '$8 != "" && $8 != "-"' | wc -l)"
todo_lawless="$(printf '%s\n' "$rows" | awk -F'\t' '($4 == "todo" || $4 == "in progress") && ($8 == "" || $8 == "-") { printf " %s", $1 }')"
bad_law="$(printf '%s\n' "$rows" | awk -F'\t' -v known="$laws_known" '
  BEGIN { n = split(known, k, "\n"); for (i = 1; i <= n; i++) ok[k[i]] = 1 }
  $8 != "" && $8 != "-" {
    n = split($8, t, ",")
    for (i = 1; i <= n; i++) {
      q = t[i]; gsub(/^[ \t]+|[ \t]+$/, "", q)
      if (q ~ /^candidate:[a-z0-9-]+$/) continue
      if (!(q in ok)) printf " %s(%s)", $1, q
    }
  }')"

# --- every `evidence` token names something that exists ------------------------------------------
#
# **This is the check that makes `done` mean anything.** An `evidence` cell claims the tree contains
# what holds the fix; a token resolving to nothing is a citation to a test that was renamed, deleted,
# or never written -- the class pass 9 found three of (C152, C153, C154). One pass over the tracked
# tree, ~1.5 s, 22,878 identifiers. `legacy/` and the generated `docs/book/` are excluded: a name that
# only exists in the unported Scala is not evidence that the port holds the fix.
#
# **Three ways this check was a tautology, all three repaired together**, because fixing any one alone
# still passes on a cell that resolves to nothing:
#
#   1. **It indexed its own source of truth.** The tokens are read *out of* `spec/findings.tsv`, so
#      including that file here made every token resolve against its own cell. `spec/AUDIT.md` is the
#      rendered form of the same cells. Both are excluded.
#   2. **It read the working tree, not the commit.** `git ls-files | xargs grep` greps the *files on
#      disk*, so a test that has never been committed resolves — which is how C182's guard came to be
#      cited while it lived only in an uncommitted edit. `git grep HEAD` reads the committed blobs.
#   3. **Any mention satisfied it.** The subject is "does this identifier occur anywhere in the tree",
#      and a *prose mention* is an occurrence. The first draft of this very comment contained the
#      underscored form of a filename and so kept its row green — which is the check failing on the case
#      it exists for, demonstrated by the person fixing it. Hence no identifier in this comment.
#
# What the check now means, honestly: *the named identifier exists in the committed tree somewhere* —
# not that it names the right thing, not that it is a definition, and not that the file still exists
# under that name. It catches a renamed or deleted symbol; it does not catch a wrong reference.
tree_ids="$(mktemp)"
{
  # Symbols *inside* the sources that can hold a fix.
  git -C "$ROOT" grep -hoE '[A-Za-z_][A-Za-z0-9_]{5,}' HEAD -- \
    '*.rs' '*.sh' '*.py' '*.lean' '*.toml' '*.proto' '*.rho' '*.ts' '*.c' '*.h' 2>/dev/null
  # And the *paths themselves*: a cell may name a file that holds the fix, which is what the check's
  # own doc says it accepts ("an evidence cell holds forms like `casper/src/gateway/ledger.rs::...`").
  # Both spellings of a filename: with its extension (`.` stripped by the normaliser turns `x.sh`
  # into `xsh`, which matches no symbol) and without it, so `check-rust-witnesses.sh` and
  # `check_rust_witnesses` are the same subject — which is what the doc above says they are.
  git -C "$ROOT" ls-tree -r --name-only HEAD 2>/dev/null \
    | awk -F/ '{ print $NF; e = $NF; sub(/\..*$/, "", e); print e }'
} | awk '{ n = $0; gsub(/[^A-Za-z0-9]/, "", n); print $0; print n }' | sort -u > "$tree_ids"
# **Separator-insensitive on both sides**, because the doc's own rule is "the subject is the *name*,
# not the path spelling" — `check-rust-witnesses.sh` and `check_rust_witnesses` are the same subject,
# and a check that refused the second would be enforcing a spelling rather than a fact. One awk pass,
# not a subshell per token: the first draft of this line ran for minutes on ~25k identifiers.
# The subject is the *name*, not the path spelling: an evidence cell holds forms like
# `casper/src/gateway/ledger.rs::record_vote` and `tokio::task::spawn_blocking`, and the question worth
# asking of either is whether the symbol at the end of it is in this tree.
unresolved="$(
  printf '%s\n' "$rows" \
    | awk -F'\t' '$6 != "" && $6 != "-" {
        n = split($6, t, "/")
        for (i = 1; i <= n; i++) {
          q = t[i]; sub(/^.*::/, "", q); gsub(/[^A-Za-z0-9_]/, "", q); gsub(/_/, "", q)
          if (length(q) >= 6) print $1 "\t" q
        }
      }' \
    | while IFS=$'\t' read -r eid etok; do
        [[ -n "$eid" ]] || continue
        grep -qxF "$etok" "$tree_ids" || printf ' %s(%s)' "$eid" "$etok"
      done)"
rm -f "$tree_ids"

# --- the `section` column: the pass that carries the finding's account ---------------------------
#
# **A `section` that resolves to nothing is the defect this check exists for.** The column "is the
# `## N.` pass in `spec/audit/passes.md` that carries the full account", and until this check nothing
# verified that the pass existed: the OCapN wave cited §67–§71 while `passes.md` ended at §66, so eleven
# findings pointed at five accounts no reader could open. It is the same class as the evidence check
# above — a name that resolves to nothing is refused rather than left to read as a citation.
PASSES_MD="$ROOT/spec/audit/passes.md"
[[ -f "$PASSES_MD" ]] || {
  printf 'emit-findings-register: no %s, so a `section` citation cannot be resolved\n' "$PASSES_MD" >&2
  exit 1
}
declared_passes="$(grep -oE '^## [0-9]+\.' "$PASSES_MD" | grep -oE '[0-9]+' | sort -u | tr '\n' ' ')"
bad_section="$(printf '%s\n' "$rows" | awk -F'\t' -v have="$declared_passes" '
  BEGIN { n = split(have, a, " "); for (i = 1; i <= n; i++) if (a[i] != "") ok[a[i]] = 1 }
  $3 != "" && $3 != "-" && !($3 in ok) { printf " %s(§%s)", $1, $3 }')"

fail=0
if [[ -n "$bad_vocab" ]]; then
  printf 'emit-findings-register: state outside the closed vocabulary:%s\n' "$bad_vocab" >&2
  printf '  the three words are: %s\n' "$VOCAB" >&2
  fail=1
fi
if [[ -n "$bad_shape" ]]; then
  printf 'emit-findings-register: malformed row(s):%s\n' "$bad_shape" >&2
  fail=1
fi
if [[ -n "$todo_unowed" ]]; then
  printf 'emit-findings-register: todo row(s) naming nothing that would close them:%s\n' "$todo_unowed" >&2
  fail=1
fi
if [[ -n "$unresolved" ]]; then
  printf 'emit-findings-register: evidence naming nothing in the tree:%s\n' "$unresolved" >&2
  fail=1
fi
if [[ -n "$bad_section" ]]; then
  printf 'emit-findings-register: finding(s) naming a pass spec/audit/passes.md does not have:%s\n' "$bad_section" >&2
  printf '  write the `## N.` pass, or point the row at the one that carries its account\n' >&2
  fail=1
fi
if [[ -n "$todo_lawless" ]]; then
  printf 'emit-findings-register: open row(s) naming no law — an open finding cannot be classified:%s\n' "$todo_lawless" >&2
  printf '  write a law id (its `number`+`clause` in spec/laws.tsv) or `candidate:<slug>`\n' >&2
  fail=1
fi
if [[ -n "$bad_law" ]]; then
  printf 'emit-findings-register: law cell(s) naming a law the register does not define:%s\n' "$bad_law" >&2
  fail=1
fi
if (( lawless > LAWLESS_CEILING )); then
  printf 'emit-findings-register: %s done rows name no law, above the ceiling of %s\n' "$lawless" "$LAWLESS_CEILING" >&2
  printf '  classify one, or lower-then-raise the ceiling deliberately (it is a ratchet)\n' >&2
  fail=1
fi
if [[ "$fail" == "1" ]]; then exit 1; fi

# --- the coverage half ---------------------------------------------------------------------------
#
# Read straight from `spec/review-ledger.tsv`. Its emitter and the 46-second join that proved its rows
# and the tree's rosters agreed were deleted 2026-09-27 with the rest of the gate apparatus; since
# 2026-10-01 a cheap replacement (`tools/check-review-ledger.sh`) enforces the `ceiling` rows against
# the counts and every T1 row against the tree, so the headline below is a count of real things. What
# *this* needs is a count over the rows, which is an awk, and that is all there is.
t1_total=$(awk -F'\t' '$3 == "T1"' "$LEDGER" 2>/dev/null | wc -l)
t1_deferred=$(awk -F'\t' '$3 == "T1" && $4 == "deferred"' "$LEDGER" 2>/dev/null | wc -l)

# --- the check-off -------------------------------------------------------------------------------
count_of() { printf '%s\n' "$rows" | awk -F'\t' -v s="$1" '$4 == s' | wc -l; }

tbl_todo() {
  printf '%s\n' "$rows" | awk -F'\t' '$4 == "todo"' | sort -t$'\t' -k1,1V \
    | awk -F'\t' '{ printf "| `%s` | %s | %s | §%s |\n", $1, $5, $7, $3 }'
}
tbl_prog() {
  printf '%s\n' "$rows" | awk -F'\t' '$4 == "in progress"' | sort -t$'\t' -k1,1V \
    | awk -F'\t' '{ printf "| `%s` | %s | %s | §%s |\n", $1, $5, $7, $3 }'
}
tbl_done() {
  printf '%s\n' "$rows" | awk -F'\t' '$4 == "done"' | sort -t$'\t' -k1,1V \
    | awk -F'\t' '{ e = ($6 == "" || $6 == "-" ? "—" : $6); printf "| `%s` | %s | %s | §%s |\n", $1, $5, e, $3 }'
}
tbl_unread() {
  awk -F'\t' '$3 == "T1" && $4 == "deferred" { printf "| `%s` |\n", $2 }' "$LEDGER" 2>/dev/null | sort
}

# **A closed half says so.** "0 of 89 T1 modules unread" is the same fact as "89 of 89 read" and
# reads like a bug rather than a result; the line is the one a reader takes away, so it is phrased by
# which way it is.
if (( t1_deferred == 0 )); then
  t1_line="**Coverage  all $t1_total T1 modules read**"
else
  t1_line="**Coverage  $t1_deferred of $t1_total T1 modules unread**"
fi

panel="$(mktemp)"
{
  printf '## Check-off\n\n'
  printf '**Findings  TODO %s · IN PROGRESS %s · DONE %s** &nbsp;&nbsp;·&nbsp;&nbsp; %s\n\n' \
    "$(count_of todo)" "$(count_of 'in progress')" "$(count_of done)" "$t1_line"
  # **The law residue is a number, printed every run.** The half that is classified is the half a reader
  # can trace to a property; the other is the honest backlog, and a backlog nobody prints is the silence
  # this column exists to replace.
  printf '**Laws  %s of %s findings name one** (ceiling %s; %s done row(s) unclassified)\n\n' \
    "$laws_named" "$n_total" "$LAWLESS_CEILING" "$lawless"
  # **The line a reader takes away says which state the audit is in**, not what a closed one would
  # mean — the same reason the coverage half is phrased by which way it reads.
  if (( $(count_of todo) == 0 && t1_deferred == 0 )); then
    printf '**Both halves are closed.** A `done` row is settled -- fixed, assessed faithful, a\n'
    printf 'deliberate deviation, or refuted -- and names what holds it where there is evidence to\n'
    printf 'name. What that does *not* mean is stated under each half below.\n\n'
  else
    printf 'Closed when both halves are zero. A **done** row is settled -- fixed, assessed faithful, a\n'
    printf 'deliberate deviation, or refuted -- and names what holds it. A **todo** row names what would\n'
    printf 'close it.\n\n'
  fi

  # Same rule as the coverage half below: **an empty section is not printed as a zero-row table.**
  # `### TODO — findings (0)` over a bare header is the same fact as "nothing is open" and reads like
  # a section that failed to render.
  if (( $(count_of todo) > 0 )); then
    printf '### TODO — findings (%s)\n\n| id | what | what closes it | account |\n|---|---|---|---|\n' "$(count_of todo)"
    tbl_todo; printf '\n'
  else
    printf '### Findings — closed\n\n'
    printf 'All %s are settled: **%s name the evidence that holds them** and **%s do not** — the\n' \
      "$n_total" \
      "$(printf '%s\n' "$rows" | awk -F'\t' '$6 != "" && $6 != "-"' | wc -l)" \
      "$(printf '%s\n' "$rows" | awk -F'\t' '$6 == "" || $6 == "-"' | wc -l)"
    printf 'second number is the honest residual, and a column rather than an implication. A `done`\n'
    printf 'row says the fix is in the tree or that the decision was taken; it does not say either is\n'
    printf 'right. Read a row that matters at the § its account cites.\n\n'
  fi
  # **An empty section is not printed as a zero-row table.** Until 2026-09-27 this always emitted
  # `### TODO — unread T1 modules (0)` over a header with no rows, which reads like a section that
  # failed to render rather than one that has nothing left in it. A closed half says so.
  if (( t1_deferred > 0 )); then
    printf '### TODO — unread T1 modules (%s)\n\n' "$t1_deferred"
    printf 'The modules that can fork the chain or lose funds, and that nobody has read. In remit and not\n'
    printf 'yet read, which is what `deferred` means in [`review-ledger.tsv`](review-ledger.tsv). Its\n'
    printf 'rendering and the gate that checked it were deleted on 2026-09-27; the data is the file.\n\n'
    printf '| module |\n|---|\n'
    tbl_unread; printf '\n'
  else
    printf '### T1 coverage — closed\n\n'
    printf 'All %s rows for the modules that can fork the chain or lose funds have been read: %s carry a\n' \
      "$t1_total" "$(awk -F'\t' '$3 == "T1" && $4 == "cleared"' "$LEDGER" 2>/dev/null | wc -l)"
    printf 'verdict of `cleared`, %s produced a finding, and %s are `exempt` with a reason class.\n' \
      "$(awk -F'\t' '$3 == "T1" && $4 == "finding"' "$LEDGER" 2>/dev/null | wc -l)" \
      "$(awk -F'\t' '$3 == "T1" && $4 == "exempt"' "$LEDGER" 2>/dev/null | wc -l)"
    printf 'The twenty reads of the 2026-09-27 coverage pass are in the pass record, and two of them found\n'
    printf 'defects this register had not recorded (C164, C165).\n\n'
  fi
  # The same rule again: `### IN PROGRESS (0)` over a bare header is the third place the closed
  # state was printed as the shape of an open one.
  if (( $(count_of 'in progress') > 0 )); then
    printf '### IN PROGRESS (%s)\n\n| id | what | what closes it | account |\n|---|---|---|---|\n' "$(count_of 'in progress')"
    tbl_prog; printf '\n'
  else
    printf '### In progress — none\n\n'
    printf 'Nothing is in flight. The state exists because a person mid-read needs somewhere to say so;\n'
    printf 'that it is empty is the fact, and it is said rather than shown as a table with no rows.\n\n'
  fi
  printf '### DONE (%s)\n\n| id | what | evidence | account |\n|---|---|---|---|\n' "$(count_of done)"
  tbl_done; printf '\n'
} > "$panel"

# --- splice, or check ------------------------------------------------------------------------------
render_or_check() {
  local frame_from from to committed
  from=$(grep -nF "$MARKER" "$AUDIT" | head -1 | cut -d: -f1)
  if [[ -z "$from" ]]; then
    printf 'emit-findings-register: %s carries no marker to write below\n' "spec/AUDIT.md" >&2
    return 1
  fi

  if [[ "$check" == "0" ]]; then
    { head -n "$from" "$AUDIT"; printf '\n'; cat "$panel"; } > "$AUDIT.tmp"
    mv "$AUDIT.tmp" "$AUDIT"
    printf 'emit-findings-register: %s findings -> spec/AUDIT.md (%s todo, %s in progress, %s done)\n' \
      "$n_total" "$(count_of todo)" "$(count_of 'in progress')" "$(count_of done)"
    return 0
  fi

  committed="$(mktemp)"
  # `sed '1{/^$/d}'` drops the one blank line the splice writes between the marker and the panel. The
  # whole rendered region is compared, not a hash of it, because the diff is what tells a reader which
  # row moved.
  tail -n "+$((from + 1))" "$AUDIT" | sed '1{/^$/d}' > "$committed"
  if ! diff -q "$committed" "$panel" >/dev/null; then
    printf 'emit-findings-register: the check-off is stale against spec/findings.tsv\n' >&2
    diff "$committed" "$panel" | head -30 >&2
    rm -f "$committed"; return 1
  fi
  rm -f "$committed"
  # **A cited id must be an id the register could hold.** One failure minted ids in commit subjects
  # and code comments without ever adding rows; this check refuses any citation above the register
  # maximum, so the same phantom cannot be merged again. Retired ids below the maximum are warnings.
  "$ROOT/tools/check-finding-citations.sh" || return 1
  printf 'emit-findings-register: ok — spec/AUDIT.md matches spec/findings.tsv (%s rows: %s todo, %s done)\n' \
    "$n_total" "$(count_of todo)" "$(count_of done)"
  return 0
}

render_or_check
rc=$?
rm -f "$panel"
exit $rc

