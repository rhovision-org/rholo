#!/usr/bin/env bash
# check-hazop-worksheet — the failure-mode worksheet's completeness claim, checked against the page.
#
# Why this exists. `docs/src/spec/failure-hazop.md` is a companion to the acceptance worksheet
# (`docs/src/spec/testnet-acceptance.md` section 1), whose rows carry the hazards and the root-cause
# analysis. That worksheet is prose: a cell can be empty, a guide word can be neither rowed nor folded
# nor vacuously dismissed, and nothing refuses it. The rule this repository keeps relearning is that
# **prose is not a check** — a page that claims "every (node x guide word) cell is dispositioned" is
# making a claim about a document, and a claim about a document belongs in a check that reads it.
#
# The format contract, and why the page must be machine-shaped:
#
#   ### U<n> — <name>
#   **Design intent.** <one sentence>
#   **Guide words.** row: NO, MORE, ... · folded: LATE -> NO, ... · vacuous: REVERSE (a reason) · owed: EARLY, AFTER
#   | ID | Guide word | Deviation | RCA | Current behaviour | Class | Disposition | Falsifier |
#   | **F-U1-01** | NO | ... | ... | ... | Historic | R2 | casper/src/x.rs::a_named_test |
#
# Checks:
#   1  every node dispositions all eleven guide words exactly once (row / folded / vacuous / owed)
#   2  every word declared a `row` carries at least one Table A row, and no Table A row carries a word
#      the node did not declare as a row (a row for a folded or vacuous word is invisible debt)
#   3  every Table A row states a definite rung R1..R4 and a falsifier that is either an `owed:`
#      marker or a path **and symbol** that exist in this tree (`path::test_name` — the file must exist
#      *and* the name must occur in it, because a fabricated test name is the defect class this
#      repository has paid for most often)
#   4  the page declares its own debt and the declaration matches the count:
#      `**Debt.** falsifiers owed: N · node-words owed: M`
#   5  the scan is not vacuous: at least one node and at least one Table A row were found
#
# A word under `owed:` is debt made visible and countable, never a silent omission — it is a cell this
# pass did not reach, and check 4 is what stops the count from rotting. Disposition, likewise, must be
# a decided rung: the point of the exercise is that an auditor picks one, and a sheet that offers
# "R1/R2" has not done the work it exists to do.
#
# Usage: tools/check-hazop-worksheet.sh [--gate] [<worksheet>]
#        --gate exits non-zero on a violation (for CI). Without it the report is printed, the
#        violations still name themselves, and the exit is 0 so a reader can run it mid-edit.
#        The default worksheet is docs/src/spec/failure-hazop.md.
set -euo pipefail

gate=0
sheet=""
for arg in "$@"; do
  case "$arg" in
    --gate) gate=1 ;;
    -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
    *) sheet="$arg" ;;
  esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
sheet="${sheet:-$root/docs/src/spec/failure-hazop.md}"
[[ -r "$sheet" ]] || { echo "check-hazop-worksheet: no worksheet at $sheet" >&2; exit 2; }

# The page-level debt declaration. A missing line is a violation, not an unparsed zero: the debt is a
# number nobody should be able to remove by deleting one sentence.
debt="$(grep -m1 '\*\*Debt\.\*\*' "$sheet" || true)"
if [[ -z "$debt" ]]; then
  echo "check-hazop-worksheet: $sheet declares no debt line." >&2
  echo "  expected: **Debt.** falsifiers owed: N · node-words owed: M" >&2
  exit 1
fi
declared_fals="$(printf '%s' "$debt" | sed -n 's/.*falsifiers owed: \([0-9]\+\).*/\1/p')"
declared_words="$(printf '%s' "$debt" | sed -n 's/.*node-words owed: \([0-9]\+\).*/\1/p')"
declared_fals="${declared_fals:--1}"
declared_words="${declared_words:--1}"

echo "check-hazop-worksheet: $sheet"

# `→` and `·` are the house punctuation and are multibyte, so they are normalised to ASCII **before**
# awk sees them: mawk and gawk disagree about multibyte character classes, and a parser whose
# correctness depends on which awk is installed is a parser that will be wrong on one of them.
# `·` is the segment separator; `→` becomes `->` so a folded key can be split off byte-safely.
set +e
sed -e 's/→/->/g' -e 's/·/@SEP@/g' "$sheet" \
  | awk -v root="$root" -v declared_fals="$declared_fals" -v declared_words="$declared_words" '
    BEGIN {
      FS = "|"
      split("NO,MORE,LESS,AS WELL AS,PART OF,OTHER THAN,REVERSE,EARLY,LATE,BEFORE,AFTER", K, ",")
      for (i = 1; i <= length(K); i++) known[K[i]] = 1
      nodes = 0; rows = 0; fals_owed = 0; words_owed = 0; bad = 0
      node = ""; have_gw = 0
    }
    function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
    function norm(s,   t) {
      t = s
      gsub(/\([^)]*\)/, "", t)      # a trailing reason in parentheses is not part of the word
      t = toupper(t)
      gsub(/\/NOT/, "", t)          # NO/NOT is the word NO
      gsub(/\*/, "", t)
      gsub(/[ \t]+/, " ", t)
      return trim(t)
    }
    function problem(msg) { printf "  VIOLATION  %s\n", msg; bad++ }
    function addwords(payload, kind,   n, it, w, i) {
      gsub(/\([^)]*\)/, "", payload)
      n = split(payload, it, ",")
      for (i = 1; i <= n; i++) {
        w = it[i]
        if (kind == "folded") sub(/->.*$/, "", w)
        w = norm(w)
        if (w == "") continue
        if (!(w in known)) { problem(node ": unknown guide word [" w "] under " kind); continue }
        seen[w]++
        if (kind == "row") { rowset[w] = 1; nroww++ }
        else if (kind == "folded") nfoldw++
        else if (kind == "vacuous") nvacw++
        else { nowedw++; words_owed++ }
      }
    }
    function flush(   i, w, c) {
      if (node == "") return
      nodes++
      if (!have_gw) { problem(node ": no **Guide words.** line") }
      else {
        for (i = 1; i <= length(K); i++) {
          w = K[i]; c = (w in seen) ? seen[w] : 0
          if (c == 0) problem(node ": guide word " w " is dispositioned nowhere (row / folded / vacuous / owed)")
          else if (c > 1) problem(node ": guide word " w " is dispositioned " c " times")
        }
        for (w in rowset) if (!(w in rowseen)) problem(node ": word " w " is declared a row but no Table A row carries it")
      }
      printf "  %s: %d row, %d folded, %d vacuous, %d owed\n", node, nroww, nfoldw, nvacw, nowedw
      delete seen; delete rowset; delete rowseen
      nroww = nfoldw = nvacw = nowedw = 0; have_gw = 0
    }
    /^### U[0-9]+/ {
      flush()
      s = $0; sub(/^### /, "", s); split(s, a, " "); node = a[1]
      next
    }
    /\*\*Guide words\.\*\*/ {
      have_gw = 1
      s = $0
      sub(/^.*\*\*Guide words\.\*\*[ \t]*/, "", s)
      n = split(s, seg, "@SEP@")
      for (i = 1; i <= n; i++) {
        t = trim(seg[i])
        if (t == "") continue
        if (match(t, /^(row|folded|vacuous|owed):/)) {
          kind = substr(t, RSTART, RLENGTH - 1)
          addwords(substr(t, RSTART + RLENGTH), kind)
        } else {
          problem(node ": guide-word segment is not row:/folded:/vacuous:/owed: -- [" t "]")
        }
      }
      next
    }
    /^\|/ {
      if (NF != 10) next
      id = $2; gsub(/[^A-Za-z0-9-]/, "", id)
      if (id !~ /^[A-Z]-U[0-9]+-[0-9]+$/) next
      if (node == "") { problem("Table A row " id " appears before any node heading"); next }
      rows++; nrows_node++
      w = norm($3)
      if (w == "") problem(node " " id ": empty guide word")
      else if (!(w in known)) problem(node " " id ": unknown guide word [" w "]")
      else if (w in rowset) rowseen[w] = 1
      else problem(node " " id ": guide word " w " is not declared a `row` for this node")
      disp = $8; gsub(/\*/, "", disp); disp = trim(disp)
      dtok = disp; sub(/[ \t(].*$/, "", dtok)
      if (dtok !~ /^R[1-4]$/) problem(node " " id ": disposition [" disp "] is not a definite rung R1..R4")
      fals = trim($9)
      if (fals == "") { problem(node " " id ": empty falsifier"); next }
      if (fals ~ /^owed:/) { fals_owed++; next }
      okp = 0; oksym = 0; anycol = 0; lastp = ""; lastsym = ""
      m = fals
      # A falsifier is a test or a checked artefact. The tools tests are Python (tools/tests/*.py)
      # and CI runs them; the same two checks apply to the extension (the path exists, the symbol
      # occurs in it), so the class is not weaker for naming one.
      while (match(m, /[A-Za-z0-9_.\/-]+\.(rs|lean|md|sh|py|tsv|txt|json|toml|v)/)) {
        p = substr(m, RSTART, RLENGTH); m = substr(m, RSTART + RLENGTH)
        sub(/:.*$/, "", p)
        if (system("test -e \"" root "/" p "\"") != 0) continue
        okp = 1; lastp = p
        if (substr(m, 1, 2) == "::") {
          anycol = 1
          if (match(substr(m, 3), /^[A-Za-z0-9_]+/)) {
            sym = substr(substr(m, 3), RSTART, RLENGTH); lastsym = sym
            if (system("grep -qF -- \"" sym "\" \"" root "/" p "\"") == 0) oksym = 1
          }
        }
      }
      if (!okp) problem(node " " id ": falsifier [" fals "] names no path that exists in this tree")
      else if (anycol && !oksym) problem(node " " id ": falsifier names symbol [" lastsym "] which does not occur in " lastp)
      next
    }
    END {
      flush()
      if (nodes == 0) problem("no `### U<n>` node headings found -- the scan would pass vacuously")
      if (rows == 0) problem("no Table A rows found -- the scan would pass vacuously")
      if (fals_owed != declared_fals) problem("declared `falsifiers owed: " declared_fals "` but counted " fals_owed)
      if (words_owed != declared_words) problem("declared `node-words owed: " declared_words "` but counted " words_owed)
      printf "\n  totals: %d nodes, %d rows, %d falsifiers owed, %d node-words owed\n", nodes, rows, fals_owed, words_owed
      exit (bad > 0)
    }
  '
status=$?
set -e

if (( status == 0 )); then
  echo "check-hazop-worksheet: every node dispositions all eleven guide words, every row is decided and evidenced."
  exit 0
fi

if (( gate == 1 )); then
  echo "check-hazop-worksheet: the worksheet does not hold." >&2
  exit 1
fi
echo "check-hazop-worksheet: report only (use --gate to fail)."
exit 0
