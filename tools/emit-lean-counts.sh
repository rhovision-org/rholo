#!/usr/bin/env bash
#
# emit-lean-counts.sh — keep the prose's law/axiom counts equal to the register's.
#
# The drift this closes is old and repeated: two documents said "29 laws" while the tree held 43; the
# register's own header said "all 43 laws" after 49 existed; `laws.md` carried both "49 laws, 58
# entries today" and, three paragraphs later, "48 laws and 57 entries"; `TYPE-SYSTEM.md` said "30
# element-comparator axioms" in three places two programmes after that number became 4. Every one of
# those was a *live* claim about a machine-emitted total, restated by hand, and nothing read them.
#
# So the totals are generated. A prose file marks the span it wants filled:
#
#     ... counts *entries* rather than laws (<!-- counts:laws -->49 laws<!-- counts:end -->, ...
#
# and this script rewrites every marked span from `spec/laws.tsv` (the register's own emission). Keys:
#
#     laws, entries, laws-entries, axioms, summary,
#     proved-laws, proved-tied-laws, proved-model-laws, owed-laws, open-laws,
#     orphaned-laws, vacuous-laws, axiom-by-design-laws,
#     proved-model-entries, proved-tied-entries   (rows, for a sentence decomposing `entries`)
#
# `--check` (what the gate runs) rewrites into a scratch copy and refuses a diff, the same discipline
# `emit-lean-laws.sh` uses for `spec/laws.tsv` itself. It also fails on a **hand-written** register total
# in one of these files: a digit followed by `laws`/`entries`/`axioms` where the digit is one of the
# register's own totals (current or known-past) and the line carries no marker.
#
# The limit of that scan, stated rather than implied: it catches the class that actually rots — a writer
# copying a total out of the register — and not every possible count. "the 29 laws" *was* the name of a
# section (laws 1–29) while that section existed; the folder's pages are now one per layer with the whole
# set in `laws.md`, so 29 is on the stale list like the rest, and a page that wants to name a *range*
# uses an en dash (`30–43`), which the number step does not consume. "four (30, 31, 33, 36) are open" is
# spelled out and therefore outside it; and a sentence about history ("the tree grew to 43 laws while
# both said 29") is a record, not a claim, which is why the scan is scoped to the reader-facing documents
# and not to `spec/Rchain/`'s module docs, where those records live. A count a reader is meant to believe
# gets a marker.
#
# Usage: tools/emit-lean-counts.sh [--check]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPEC="$ROOT/spec"
TSV="$SPEC/laws.tsv"
check=0
[[ "${1:-}" == "--check" ]] && check=1

if [[ ! -f "$TSV" ]]; then
  echo "FAIL  $TSV does not exist — run tools/emit-lean-laws.sh first" >&2
  exit 1
fi

# --- the totals, straight off the register's own emission -------------------------------------------
col() { awk -F'\t' -v s="$1" 'NR>1 && $4==s {print $1}' "$TSV" | sort -u | wc -l | tr -d ' '; }
# The same count **by row rather than by law**, for a sentence that is decomposing `entries`. The two
# differ wherever a law has clauses (law 56 is one number and three rows), so a `-laws` marker in an
# entry-decomposing sentence would silently understate — measured 2026-10-02, when `proved-model` read 37
# laws against 48 entries and the wrong key put 37 into `AGENTS.md`.
col_rows() { awk -F'\t' -v s="$1" 'NR>1 && $4==s' "$TSV" | wc -l | tr -d ' '; }
entries="$(awk 'NR>1' "$TSV" | wc -l | tr -d ' ')"
n_laws="$(awk -F'\t' 'NR>1{print $1}' "$TSV" | sort -u | wc -l | tr -d ' ')"
n_axioms="$(awk -F'\t' 'NR>1 && $6 != "-" {print $6}' "$TSV" \
  | tr ',' '\n' | sed -e 's/^ *//' -e 's/ *$//' | grep -v '^$' | sort -u | wc -l | tr -d ' ')"
n_proved_tied="$(col proved-tied)"
n_proved_model="$(col proved-model)"
n_proved="$(awk -F'\t' 'NR>1 && ($4=="proved-tied" || $4=="proved-model") {print $1}' "$TSV" \
  | sort -u | wc -l | tr -d ' ')"
# A law with more than one row carries clause letters, and `entries` is larger than `laws` by exactly
# this population's extra rows. Stated separately so the arithmetic sentence is generated rather than
# hand-written (2026-10-26: the page said eighteen while the register held sixteen).
n_clause_laws="$(awk -F'\t' 'NR>1 {c[$1]++} END {n=0; for (k in c) if (c[k]>1) n++; print n+0}' "$TSV")"

value_of() {
  case "$1" in
    laws)                 printf '%s laws' "$n_laws" ;;
    entries)              printf '%s entries' "$entries" ;;
    laws-entries)         printf '%s laws and %s entries' "$n_laws" "$entries" ;;
    axioms)               printf '%s axioms' "$n_axioms" ;;
    summary)              head -1 "$SPEC/LAWS.md" ;;
    proved-laws)          printf '%s' "$n_proved" ;;
    proved-tied-laws)     printf '%s' "$n_proved_tied" ;;
    proved-model-laws)    printf '%s' "$n_proved_model" ;;
    proved-model-entries) printf '%s' "$(col_rows proved-model)" ;;
    proved-tied-entries)  printf '%s' "$(col_rows proved-tied)" ;;
    clause-laws)          printf '%s' "$n_clause_laws" ;;
    owed-laws)            col owed ;;
    open-laws)            col open ;;
    orphaned-laws)        col orphaned ;;
    vacuous-laws)         col vacuous ;;
    axiom-by-design-laws) col axiom-by-design ;;
    *) return 1 ;;
  esac
}

FILES=(
  "$SPEC/README.md"
  "$SPEC/TYPE-SYSTEM.md"
  "$SPEC/INVENTORY.md"
  "$SPEC/coq/README.md"
  "$ROOT/docs/src/ai-entrypoint.md"
  # The authoritative intent document, and the one most likely to be read first. It drifted precisely
  # because it was *outside* this list: its Status section carried a stale comparator-axiom total
  # (and, worse, cited Lean declarations the tree no longer has — a class this scan cannot see, found
  # by hand on 2026-09-24). It is in the list now, so its totals are the register's.
  "$ROOT/AGENTS.md"
  # The contributor-facing architecture page had the same defect for the same reason: a "Remaining"
  # bullet restating per-law statuses that had moved on. That bullet now points at the register; this
  # entry is what keeps it pointing.
  "$ROOT/docs/src/contributor/architecture.md"
  # The two documents a reader meets first, and the reason this list has grown by exactly this rule:
  # a count stated outside it drifts, because nothing recomputes it. `README.md` said "the 29 laws"
  # and `introduction.md` said "29 laws … plus laws 30–43" while the register held 49 — found by hand,
  # like `AGENTS.md`'s four stale declarations before them.
  "$ROOT/README.md"
  "$ROOT/docs/src/introduction.md"
  # **And every other page that states a total by hand, added 2026-09-26 the same way** (AUDIT C106):
  # each of these carried a stale "29 laws" — `quantum-to-rho.md`, `why-rust.md` (twice),
  # `laws-to-rust.md` (twice), `rholang/reference.md`, `RHO-CALCULUS.md`,
  # `RUST-VS-SCALA.md` and `TEST-COVERAGE.md` (twice) — and all of them were outside this list, which
  # is exactly why they drifted. The rule this list grows by is the finding: a count stated outside it
  # is one nothing recomputes.
  "$ROOT/docs/src/qucalc/quantum-to-rho.md"
  "$ROOT/docs/src/contributor/why-rust.md"
  "$ROOT/docs/src/contributor/laws-to-rust.md"
  "$ROOT/docs/src/rholang/reference.md"
  "$SPEC/RHO-CALCULUS.md"
  "$SPEC/RUST-VS-SCALA.md"
  "$SPEC/TEST-COVERAGE.md"
  # The two methodology drafts (2026-10-15) state the law total with a marker, and they are in the list
  # by the same rule the rest are: a count stated outside it is one nothing recomputes.
  "$ROOT/docs/src/methodology/law-first-port.md"
  "$ROOT/docs/src/methodology/agentic-assurance.md"
  # The laws split (2026-10-26): the human page is caught by the `formal/*.md` glob below; the agent
  # reference states the law/entry total and is added explicitly. The operator setup guide is in the
  # list for the same rule — it is reader-facing and cites laws.
  "$ROOT/docs/src/ai/laws-reference.md"
  "$ROOT/docs/src/node/validator-observer-setup.md"
)
for f in "$ROOT"/docs/src/formal/*.md; do FILES+=("$f"); done

# The map as `key=value` lines, reused for both the rewrite and the scan.
map="$(for k in laws entries laws-entries axioms summary proved-laws proved-tied-laws proved-model-laws \
                 owed-laws open-laws orphaned-laws vacuous-laws axiom-by-design-laws \
                 proved-model-entries proved-tied-entries clause-laws; do
          printf '%s=%s\n' "$k" "$(value_of "$k")"
        done)"

# --- the scan for hand-written totals --------------------------------------------------------------
# Current totals plus the ones that have already gone stale at least once, so a document that states one
# of them by hand is caught on the next run rather than after the next programme.
stale_numbers="$(printf '%s\n' "$n_laws" "$entries" "$n_axioms" "$n_proved" 29 43 48 57 30 34 49 58 21 \
  50 51 52 53 59 60 63 65 66 \
  | sort -u | paste -sd'|' -)"
# The scan reads **paragraphs**, not lines, and that is load-bearing: the first version was line-based
# and matched only `N axioms`, so it missed two of `TYPE-SYSTEM.md`'s three stale claims (one wrapped as
# `30 residual element-comparator` / `comparator-law axioms`). A scan that misses the instances it exists
# for is worse than none, so the paragraph is joined before matching.
#
# It is two linear steps — a stale number at a word boundary, then the noun within the next 80
# characters — rather than one pattern with `(word ){0,3}` between them. That is not style: the
# one-pattern version (`(49|…)[- ]([A-Za-z_*.\`-]+[- ]){0,3}(laws?|…)`) *hung*, because a `+` inside a
# bounded repetition is the classic exponential-backtracking shape and the paragraphs here are long
# bullets that nearly match. A gate step that can hang is worse than no gate step.
#
# The match is `[[ … =~ ]]` and not `awk`/`grep -P` for the same reason the pattern had to change: the
# `awk` on Debian and Ubuntu is mawk 1.3.4, which *panics* (`REcompile() - panic: parser returns ERR_7`)
# on an interval following a group.
numpat='(^|[^A-Za-z0-9])('"$stale_numbers"')[- ]'
# The noun may not be followed by a hyphen: `law-42` and `law-5` are cross-references to a named law, and
# a first draft that accepted the hyphen flagged "listed law 34 ... carried the law-42 sentence" as a
# count. The hyphenated *count* form (`the 29-law catalog`) is handled by the number step, which consumes
# the `29-` and leaves `law catalog` as the window.
nounpat='(laws?|entries|axioms?)([^A-Za-z0-9-]|$)'
scan_hits=0
for f in "${FILES[@]}"; do
  [[ -f "$f" ]] || continue
  buf=""
  file_hits=""
  scan_buf() { # $1 = the paragraph
    [[ -n "$1" ]] || return 0
    # **A generated span is removed, not a licence to skip the paragraph it sits in** (AUDIT C106).
    # This line used to `return 0` on any paragraph carrying a marker, which is how
    # `docs/src/ai-entrypoint.md` went on saying "the 29 calculus laws" *immediately after* a correct,
    # generated "59 entries" span: the span's presence hid the hand-written total beside it.
    local paragraph
    paragraph="$(printf '%s' "$1" | sed -E 's/<!-- counts:[a-z-]* -->[^<]*<!-- counts:end -->//g')"
    [[ "$paragraph" =~ $numpat ]] || return 0
    # The matched number goes in the message: a report that says "this paragraph states a total by
    # hand" and stops leaves the reader to find *which* number, and a check whose output needs
    # archaeology is one people learn to ignore.
    local matched="${BASH_REMATCH[0]}"
    # **A citation, not a total: `law 51`, `laws 30-43`.** The noun-before-number form is how these files
    # *cite* a law, and the 80-character window reaches into the next sentence — so every citation to a
    # law whose number was ever a total reads as a hand-written total. Found on 2026-09-30 by widening
    # the stale list to 50-53 (the numbers this sweep was about): `laws.md`'s "the first instance of law
    # 51 is the one the vocabulary was written for, and it is the law this repository needed" was flagged
    # on the noun of a sentence three clauses later. The guard keeps C106's window *and* stops reading
    # citations as counts, which is the trade that window was said to be forced into.
    local prefix="${paragraph%%"${matched}"*}"
    [[ "${prefix: -10}" =~ (laws?)[-\ ]?$ ]] && return 0
    local rest="${paragraph#*"${matched}"}"
    if [[ "${rest:0:80}" =~ $nounpat ]]; then
      file_hits+="      '${matched%[- ]}' … ${1:0:140}"$'\n'
    fi
  }
  while IFS= read -r line || [[ -n "$line" ]]; do
    if [[ -z "${line//[[:space:]]/}" ]]; then
      scan_buf "$buf"; buf=""
    else
      buf+="$line "
    fi
  done < "$f"
  scan_buf "$buf"
  if [[ -n "$file_hits" ]]; then
    printf 'FAIL  %s states a register total by hand — wrap it in <!-- counts:… -->:\n%s' \
      "${f#"$ROOT"/}" "$file_hits" >&2
    scan_hits=1
  fi
done

# --- rewrite or check ------------------------------------------------------------------------------
# Keys are validated before anything is written: a typo in a marker must be a message a person can act
# on, not a `???` committed into a document.
known_keys=" $(printf '%s\n' "$map" | cut -d= -f1 | paste -sd' ' -) "
for f in "${FILES[@]}"; do
  [[ -f "$f" ]] || continue
  while IFS= read -r key; do
    [[ -z "$key" ]] && continue
    if [[ "$known_keys" != *" $key "* ]]; then
      printf 'FAIL  %s has <!-- counts:%s -->, which is not a key (known: %s)\n' \
        "${f#"$ROOT"/}" "$key" "${known_keys# }" >&2
      exit 1
    fi
  done < <(grep -o '<!-- counts:[a-z-]* -->' "$f" | grep -v 'counts:end' \
    | sed -e 's/<!-- counts://' -e 's/ -->//' || true)
  if grep -q '<!-- counts:' "$f"; then
    # every opener must have its closer on the same line, since only single-line spans are generated
    if ! awk '/<!-- counts:[a-z-]+ -->/ && !/<!-- counts:end -->/' "$f" | grep -q .; then :; else
      printf 'FAIL  %s has a counts block with no <!-- counts:end --> on the same line\n' \
        "${f#"$ROOT"/}" >&2
      exit 1
    fi
  fi
done

changed=0
for f in "${FILES[@]}"; do
  [[ -f "$f" ]] || continue
  tmp="$(mktemp)"
  printf '%s\n' "$map" | awk -v mapfile=/dev/stdin '
    BEGIN { while ((getline line < mapfile) > 0) {
              i = index(line, "="); if (i > 0) V[substr(line,1,i-1)] = substr(line,i+1) } }
    {
      # The span is *consumed* as it is replaced — marker, value and closer all go into `out`, and the
      # search continues past them. The first version searched the rewritten line again, re-matched the
      # marker it had just handled and looped forever: a gate step that hangs is worse than no gate step.
      out = ""; rest = $0
      while (match(rest, /<!-- counts:[a-z-]+ -->/)) {
        key = substr(rest, RSTART, RLENGTH); gsub(/<!-- counts:| -->/, "", key)
        pre = substr(rest, 1, RSTART + RLENGTH - 1)
        tail = substr(rest, RSTART + RLENGTH)
        if (!match(tail, /<!-- counts:end -->/)) { out = out rest; rest = ""; break }
        out = out pre (key in V ? V[key] : "???") "<!-- counts:end -->"
        rest = substr(tail, RSTART + RLENGTH)
      }
      print out rest
    }' "$f" > "$tmp"
  if ! cmp -s "$f" "$tmp"; then
    if [[ $check -eq 1 ]]; then
      printf 'FAIL  %s is not what the register says — re-emit:\n' "${f#"$ROOT"/}" >&2
      diff "$f" "$tmp" | head -12 | sed 's/^/      /' >&2 || true
      changed=1
    else
      cp "$tmp" "$f"
      printf 'emitted  %s\n' "${f#"$ROOT"/}"
    fi
  fi
  rm -f "$tmp"
done

if [[ $check -eq 1 ]]; then
  if [[ $scan_hits -eq 0 && $changed -eq 0 ]]; then
    printf 'ok    every marked count matches the register (%s)\n' \
      "$(printf '%s' "$(value_of laws-entries)")"
  else
    exit 1
  fi
fi
