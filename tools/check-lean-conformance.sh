#!/usr/bin/env bash
#
# check-lean-conformance.sh — the formal gate.
#
# The repo's standard is "the oracle is the mathematical specification, not the Scala code"
# (`AGENTS.md`'s *Prime directive*) — and until this script existed the specification was checked by
# nothing: no CI job built the Lean, and the Coq had never been built outside a developer's shell. A
# theorem that stopped holding, or a module that quietly fell out of the library, was noticed by no one.
#
# What it checks, in order:
#   1. **The library builds** — `lake build` in `spec/`.
#   2. **Nothing is admitted, and nothing is assumed by another name** — no `sorry`/`admit` and no
#      `opaque`/`unsafe`/`partial`/`extern`/`implemented_by`, anywhere under `spec/` outside the build
#      tree. The tree holds zero today, so this is a ratchet: it can only stay that way. Its sibling
#      (step 2b) counts `native_decide`, the one legal route by which an assumption still arrives: it
#      proves by evaluation in compiled code, admitting the result through `Lean.ofReduceBool`, so the
#      compiler joins the trusted base. Four sites remain, all in `Rchain/Corpus.lean`'s corpus
#      verdicts where the kernel does not reduce the goal at all, and the count may only go down.
#   3. **The library is complete** — every `.lean` under `spec/` is the library root, imported by
#      `Rchain.lean`, or a declared `lean_exe` root; there is no third kind (AUDIT C75, C76).
#   4. **The Coq builds** — `make` in `spec/coq/` (the second formalization of laws 2, 3, 5, 6).
#   5. **The conformance corpora are current** — `lake exe rchain-corpus` re-emits them and
#      `git diff --exit-code` proves the committed corpora are exactly what the Lean definitions
#      produce. A stale corpus is a check that stopped checking.
#   6. **The Rust agrees 1:1** — the consumer tests read the corpora and fail on any disagreement.
#   7. **The static audits** — protocol agreement and channel balance over the vendored sources.
#   8. **The law register is current** — `lake exe rchain-laws` re-emits `spec/laws.tsv` and
#      `spec/LAWS.md`, and `git status` proves they are what `Rchain/Laws.lean` says. The register's own
#      checks — numbering 1..`lawCeiling`, reference integrity, axiom accounting, and that every
#      *proved* law carries a falsifiability witness — run in step 1, because they need the elaborated
#      environment. The documents that quote the register's totals are re-derived from `spec/laws.tsv`
#      here too (`tools/emit-lean-counts.sh`), so a number in prose cannot drift from it.
#
# Usage: tools/check-lean-conformance.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPEC="$ROOT/spec"
failures=0
fail() { printf 'FAIL  %s\n' "$*"; failures=$((failures + 1)); }
ok() { printf 'ok    %s\n' "$*"; }

# the stack the elaborator needs (measured): see the note below the corpus-to-test mapping (AUDIT C76).
ulimit -s 65536 2>/dev/null || true

# --- 1. the library builds -----------------------------------------------------
# The executable targets are built too, and that is load-bearing: a module that is an `lean_exe` root
# (the corpus emitter) is *not* pulled in by the library target, so `lake build` alone would compile
# the corpus module's theorems never — a `decide`d case could rot unread. Verified by breaking the
# model on purpose and watching this fail.
#
# `rchain-laws` is built here for the same reason and one more: its `run_cmd` checks (the register's
# numbering, reference integrity and axiom accounting) only run when the module is elaborated, so
# *building* it is the check. A `sorry` scan cannot see an `axiom`; this step is what can.
if command -v lake >/dev/null 2>&1; then
  : >/tmp/lean-build.log
  : >/tmp/lean-memory.log
  # **A memory/RSS sample beside the build.** The formal job is intermittently
  # SIGTERM'd with no `ok`/`FAIL` line (issue #417). The runner is discarded, so
  # the workflow's `if: failure()` step tails this file; without it the only
  # surviving evidence is the step's absence.
  (
    while sleep 20; do
      date -Is >>/tmp/lean-memory.log
      free -m >>/tmp/lean-memory.log 2>&1 || true
      ps -eo pid,rss,pcpu,comm --sort=-rss | head -8 >>/tmp/lean-memory.log 2>&1 || true
      echo >>/tmp/lean-memory.log
    done
  ) &
  sampler=$!
  printf 'lake build: starting %s\n' "$(date -Is)"
  # **The register is the memory peak.** `Rchain/Laws.lean` is one 103-row list
  # whose elaboration is a unification problem (its own comment records the
  # heartbeats it outgrew). Building it alone leaves that peak unshared with the
  # rest of the library; the second build then uses the cached module.
  if (cd "$SPEC" && timeout 1200 lake build Rchain.Laws) >>/tmp/lean-build.log 2>&1; then
    ok "lake build Rchain.Laws (the register alone)"
  else
    fail "lake build Rchain.Laws — see /tmp/lean-build.log (a 124 there is the 1200 s bound above, not a proof error)"
    tail -n 120 /tmp/lean-build.log || true
  fi
  if (cd "$SPEC" && timeout 1800 bash -c 'lake build && lake build rchain-corpus && lake build rchain-laws') >>/tmp/lean-build.log 2>&1; then
    ok "lake build (spec/, library + executables + the law-register checks)"
  else
    fail "lake build (spec/) — see /tmp/lean-build.log (a 124 there is the 1800 s bound above, not a proof error)"
    tail -n 120 /tmp/lean-build.log || true
  fi
  kill "$sampler" 2>/dev/null || true
  wait "$sampler" 2>/dev/null || true
  printf 'lake build: finished %s\n' "$(date -Is)"
else
  fail "lake is not installed (elan: https://github.com/leanprover/elan)"
fi

# --- 2. nothing is admitted, and nothing is assumed by another name ------------
# `sorry` (and `admit`, its tactic spelling) is how a proof obligation becomes a promise. The tree
# holds zero today, so this is a ratchet: it can only stay that way. Comments are stripped first —
# the words occur in prose ("the grammar allows one", "admit one"), and a gate that fires on prose is
# a gate people learn to work around.
#
if awk ' BEGIN { SQ = sprintf("%c", 39) }
  { out = ""; i = 1
    while (i <= length($0)) { c = substr($0, i, 1)
      if (depth > 0) { if (substr($0,i,2)=="/-") { depth++; i+=2 } else if (substr($0,i,2)=="-/") { depth--; i+=2 } else i++ }
      else if (instring) { if (c=="\\") i+=2; else { if (c=="\"") instring=0; i++ } }
      else {
        if (substr($0,i,2)=="--") break
        if (substr($0,i,2)=="/-") { depth++; i+=2 } else if (c=="\"") { if (substr($0,i-1,1)==SQ) { i++ } else { instring=1; i++ } } else { out=out c; i++ }
      } }
    if (out ~ /(^|[^A-Za-z_])(sorry|admit|opaque|unsafe|partial|extern|implemented_by)([^A-Za-z_]|$)/) printf "%s:%d:%s\n", FILENAME, FNR, out
  }' $(find "$SPEC" -name '*.lean' -not -path '*/.lake/*') >/tmp/lean-sorry.log 2>/dev/null; then
  if [[ -s /tmp/lean-sorry.log ]]; then
    fail "an assumption appeared under spec/Rchain (sorry/admit/opaque/unsafe/partial/extern/implemented_by) — see /tmp/lean-sorry.log"
  else
    ok "no sorry/admit/opaque/unsafe/partial/extern/implemented_by under spec/Rchain"
  fi
else
  fail "the assurance scan could not run"
fi

# --- 2b. the compiler-trust census --------------------------------------------
# `native_decide` proves a goal by evaluating it in compiled code and admitting the result through the
# axiom `Lean.ofReduceBool` — the toolchain's words: "the Lean compiler and interpreter become part of
# your trusted code base. This is extra 30k lines of code" (`Init/Core.lean`). It is *legal*, so the scan
# above does not refuse it, and the register's axiom accounting cannot see it (that folds the environment
# for axioms named `Rchain`). It is therefore an assumption arriving by a route nothing counted — AUDIT
# C70's class, which is why that entry widened the token set.
#
# Measured 2026-09-25: 36 occurrences in six modules. 30 became ordinary `decide` reductions in the same
# change (the kernel reduces those goals), and the four that remain are `Rchain/Corpus.lean`'s corpus
# verdicts — where `decide` does not reduce the goal at all (`tactic 'decide' failed for proposition`,
# the measurement each of the four docstrings now carries), so the compiler is genuinely part of what the
# *tie* rests on. Those four are invisible to `Rchain/LawsMain.lean`'s check 6b, because `Corpus.lean` is
# an executable root rather than part of the `Rchain` library; this census is the net under both.
#
# A ceiling in `COQ_AXIOM_CEILING`'s style: it may only go down, and lowering it belongs to the change
# that converts a site. Comments are stripped first, for step 2's reason.
NATIVE_DECIDE_CEILING=4
n_native_decide="$(awk ' BEGIN { SQ = sprintf("%c", 39) }
  { out = ""; i = 1
    while (i <= length($0)) { c = substr($0, i, 1)
      if (depth > 0) { if (substr($0,i,2)=="/-") { depth++; i+=2 } else if (substr($0,i,2)=="-/") { depth--; i+=2 } else i++ }
      else if (instring) { if (c=="\\") i+=2; else { if (c=="\"") instring=0; i++ } }
      else {
        if (substr($0,i,2)=="--") break
        if (substr($0,i,2)=="/-") { depth++; i+=2 } else if (c=="\"") { if (substr($0,i-1,1)==SQ) { i++ } else { instring=1; i++ } } else { out=out c; i++ }
      } }
    if (out ~ /(^|[^A-Za-z_])native_decide([^A-Za-z_]|$)/) n++
  } END { print n+0 }' $(find "$SPEC" -name '*.lean' -not -path '*/.lake/*') 2>/dev/null || echo 0)"
if (( n_native_decide > NATIVE_DECIDE_CEILING )); then
  fail "the compiler trust surface grew: $n_native_decide \`native_decide\` site(s), ceiling $NATIVE_DECIDE_CEILING — replace it with \`decide\` where the kernel reduces the goal, or name the site and its measurement"
elif (( n_native_decide < NATIVE_DECIDE_CEILING )); then
  ok "compiler trust surface: $n_native_decide \`native_decide\` site(s), below the ceiling $NATIVE_DECIDE_CEILING — lower the ceiling in this change (a discharge recorded is a ratchet kept)"
else
  ok "compiler trust surface: $n_native_decide \`native_decide\` site(s), ceiling $NATIVE_DECIDE_CEILING"
fi

# --- 3. the library is complete ------------------------------------------------
# The library root is the import list itself, and is not imported by anything (`grep -v` here).
expected="$(cd "$SPEC" && find . -name '*.lean' -not -path './.lake/*' | sed -e 's#^\./##' -e 's/\.lean$//' -e 's#/#.#g' | grep -v '^Rchain$' | sort)"
# `lean_exe` roots (e.g. the corpus emitter) are executables, not library content: exclude them.
exe_roots="$(grep -oE 'root = "[^"]+"' "$SPEC/lakefile.toml" | sed -e 's/root = "//' -e 's/"//' \
  | sed -e 's/\./\//g' | sed -e 's#$#.lean#')"
if [[ -n "$exe_roots" ]]; then
  while read -r root; do
    [[ -n "$root" ]] || continue
    expected="$(printf '%s\n' "$expected" | grep -v "^$(printf '%s' "$root" | sed -e 's/\.lean$//' -e 's#/#.#g')$")"
  done <<< "$exe_roots"
fi
actual="$(grep -oE '^import Rchain[.A-Za-z]*' "$SPEC/Rchain.lean" | sed -e 's/^import //' | sort)"
if [[ "$expected" == "$actual" ]]; then
  ok "Rchain.lean imports every module ($(printf '%s\n' "$expected" | wc -l) files)"
else
  fail "Rchain.lean's imports differ from the tree:"
  diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") | sed 's/^/      /' || true
fi

# --- 4. the Coq builds ---------------------------------------------------------
# The version is pinned in `spec/coq/coq-version` and asserted here rather than installed by the gate:
# CI used to take whatever `apt-get install coq` gave the runner image, so the tested and the deployed
# Coq agreed by luck, and a Coq upgrade could have changed what the files mean without anything
# noticing. The file is the checkable half; how CI provision that version is CI's business.
COQ_AXIOM_CEILING=8    # the trust surface of spec/coq/, counted by step 4b below. Lower it with a discharge.
if [[ -f "$SPEC/coq/Makefile" ]]; then
  if ! command -v coqc >/dev/null 2>&1; then
    fail "spec/coq exists but coqc is not installed"
  else
    want="$(tr -d '[:space:]' <"$SPEC/coq/coq-version" 2>/dev/null || true)"
    have="$(coqc --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
    if [[ -n "$want" && "$want" != "$have" ]]; then
      fail "Coq is $have but spec/coq/coq-version pins $want — the definitions are checked against one Coq, not 'a Coq 8.x'"
    else
      ok "coq version ($have, pinned)"
    fi
    if (cd "$SPEC/coq" && make >/tmp/coq-build.log 2>&1); then
      ok "coq build (spec/coq/)"
    else
      fail "coq build (spec/coq/) — see /tmp/coq-build.log"
    fi
  fi
fi

# --- 4b. the Coq's own trust surface -------------------------------------------
# Nothing read the Coq's axioms before this step: the Lean `sorry` scan covers `spec/Rchain/**` and no
# check looked at a `.v` file at all, so the 15 `Axiom`s there — one of them *false* as written
# (`binds_at_most_once`, deleted 2026-09-23; AUDIT C26 said so and Coq still carried it) — were
# invisible. Counted rather than forbidden, and printed on *every* run: all of them are there today, so
# a zero-tolerance rule would be red on arrival and switched off on day one, and an unchanged ceiling
# that nobody can see is a ratchet in name only. `Admitted` counts too, because it is an axiom in the
# kernel; `Unset Guard|Positivity Checking` counts because that is how a non-terminating recursion or an
# ill-founded inductive is smuggled past the checker.
if [[ -f "$SPEC/coq/Makefile" ]]; then
  # The comment stripper uses `index` and never re-scans from the front of a shortened string. The
  # `match`-based version looked equivalent and was not: after stripping `(* … *)` from a whole-line
  # comment it left a shorter line, and `match("", /\(\*/)` in mawk *re-entered* the loop, so every
  # paragraph after the first such comment was treated as comment text and silently dropped from the
  # count (it reported 5 of 14). A count that under-reports is worse than none. This loop consumes at
  # least two characters per iteration, so it terminates.
  coq_axioms="$(awk '
    function strip(line,   i, j, out) {
      out = ""
      while (1) {
        i = index(line, "(*")
        if (i == 0) return out line
        out = out substr(line, 1, i - 1)
        line = substr(line, i + 2)
        j = index(line, "*)")
        if (j == 0) { inblock = 1; return out }
        line = substr(line, j + 2)
      }
    }
    { line = $0
      if (inblock) {
        j = index(line, "*)")
        if (j == 0) next
        line = strip(substr(line, j + 2)); inblock = 0
        if (line == "") next
      } else { line = strip(line) }
      if (line ~ /(^|[^A-Za-z_])(Axiom|Axioms|Parameter|Parameters|Conjecture|Admitted|admit|Unset[ \t]+(Guard|Positivity)[ \t]+Checking)([^A-Za-z_]|$)/) {
        printf "%s:%d: %s\n", FILENAME, FNR, line
      }
    }' "$SPEC"/coq/*.v)"
  n_coq="$(printf '%s\n' "$coq_axioms" | grep -c . || true)"
  if ((n_coq > COQ_AXIOM_CEILING)); then
    fail "the Coq trust surface grew: $n_coq axiom(s)/admit(s), ceiling $COQ_AXIOM_CEILING:"
    printf '%s\n' "$coq_axioms" | sed 's/^/      /'
  else
    ok "coq trust surface: $n_coq axiom(s)/admit(s), ceiling $COQ_AXIOM_CEILING"
  fi
fi

# --- 5/6/7. the corpora and their consumers ------------------------------------
# Each of these steps is added with the artefacts it needs, in the same change: a step whose inputs do
# not exist would make this script vacuously green, which is the failure mode it is here to prevent.
# The consumer list is driven by the corpora on disk, so a new layer cannot ship without its consumer
# (a corpus nothing reads is not a check) — while a layer not yet started is simply absent.
if [[ -x "$ROOT/tools/emit-lean-corpus.sh" ]]; then
  if "$ROOT/tools/emit-lean-corpus.sh" >/tmp/lean-corpus.log 2>&1; then
    ok "corpora emitted from the Lean definitions"
  else
    fail "corpus emission failed — see /tmp/lean-corpus.log"
  fi
  # `git status --porcelain` rather than `git diff`: an untracked corpus (a layer just added, not yet
  # committed) has no diff to show, and a check that passes on an uncommitted file is the same check
  # that would pass on a stale one.
  dirty="$(cd "$ROOT" && git status --porcelain -- spec/conformance)"
  if [[ -z "$dirty" ]]; then
    ok "committed corpora match the Lean definitions"
  else
    fail "spec/conformance is not what the Lean defines — re-emit and commit:"
    printf '%s\n' "$dirty" | sed 's/^/      /'
  fi
fi

if [[ -d "$ROOT/spec/conformance" ]]; then
  for corpus in "$ROOT"/spec/conformance/*.tsv; do
    [[ -e "$corpus" ]] || continue
    layer="$(basename "$corpus" .tsv)"
    case "$layer" in
      flags)    test_name="lean_normalize_corpus"; crate="rchain-rholang" ;;
      parse)    test_name="lean_parse_corpus";     crate="rchain-rholang" ;;
      match)    test_name="lean_match_corpus";     crate="rchain-rholang" ;;
      silence)  test_name="lean_silence_corpus";   crate="rchain-rholang" ;;
      store)    test_name="lean_store_corpus";     crate="rchain-rholang" ;;
      c21)      test_name="lean_c21_corpus";       crate="rchain-rholang" ;;
      sort)     test_name="lean_sort_corpus";      crate="rchain-rholang" ;;
      body)     test_name="lean_body_corpus";      crate="rchain-node" ;;
      closed)   test_name="lean_closed_corpus";    crate="rchain-rholang" ;;
      stake)    test_name="lean_stake_corpus";     crate="rchain-node" ;;
      block)    test_name="lean_block_corpus";     crate="rchain-node" ;;
      liveness) test_name="lean_liveness_corpus";  crate="rchain-node" ;;
      admission) test_name="lean_admission_corpus"; crate="rchain-casper" ;;
      decimal)  test_name="lean_decimal_corpus";   crate="rchain-models" ;;
      hostlist) test_name="lean_hostlist_corpus";  crate="rchain-node" ;;
      recovery) test_name="lean_recovery_corpus";  crate="rchain-casper" ;;
      protocol) test_name="lean_protocol_corpus";  crate="rchain-rholang" ;;
      envelope) test_name="lean_envelope_corpus";  crate="rchain-node" ;;
      lex)      test_name="lean_lex_corpus";       crate="rchain-node" ;;
      json)     test_name="lean_json_corpus";      crate="rchain-node" ;;
      syrup)    test_name="lean_syrup_corpus";    crate="rchain-node" ;;
      *)        fail "corpus spec/conformance/$layer.tsv has no consumer mapping"; continue ;;
    esac
    if [[ ! -f "$ROOT/rholang/tests/$test_name.rs" && ! -f "$ROOT/node/tests/$test_name.rs" && ! -f "$ROOT/casper/tests/$test_name.rs" && ! -f "$ROOT/models/tests/$test_name.rs" ]]; then
      fail "spec/conformance/$layer.tsv exists but no consumer test ($test_name) does — the corpus has no consumer"
      continue
    fi
    # Consumers live in `rholang/tests`, `node/tests`, `casper/tests` or `models/tests`; find which,
    # so the ok line stays honest.
    if [[ -f "$ROOT/rholang/tests/$test_name.rs" ]]; then
      dir=rholang
    elif [[ -f "$ROOT/node/tests/$test_name.rs" ]]; then
      dir=node
    elif [[ -f "$ROOT/casper/tests/$test_name.rs" ]]; then
      dir=casper
    else
      dir=models
    fi
    if (cd "$ROOT" && cargo test -p "$crate" --test "$test_name" >/tmp/"$test_name".log 2>&1); then
      ok "$dir/tests/$test_name.rs agrees with spec/conformance/$layer.tsv"
    else
      fail "$dir/tests/$test_name.rs disagrees with the corpus — see /tmp/$test_name.log"
    fi
  done
fi

# **The scope of the two steps above (the token scan and the completeness check) is every `.lean`
# under `spec/` except the build tree, and that is a decision rather than an accident of where the walk
# starts** (2026-09-24, AUDIT C75). Both used to look at `Rchain/**` only, so a module at the top of
# `spec/` — which nothing imports and `lake build` therefore never compiles — was invisible to the
# completeness check, to the token scan, and to the build: a `sorry` written in such a file was
# invisible to the ratchet that exists to find it. Falsified with two probes (`spec/ProbeSorry.lean`
# with `sorry`, `spec/ProbeOpaque.lean` with an `opaque def`): the scan reported 0 hits and
# `expected`/`actual` compared equal with both present. The policy the widened scope enforces: a Lean
# file under `spec/` is either the library root, a module the root imports, or a declared `lean_exe`
# root — there is no third kind, and a scratch file belongs outside `spec/`. `.lake/` is excluded
# because it holds 5,668 generated `.lean` files, which is why the old scope looked reasonable.
#
# This note sits BELOW the corpus-to-test mapping on purpose: `Laws.lean` cites a *line* of this
# script for law 30, and a line-anchored citation into a script moves whenever a step above it grows.
#
# **That is not a stylistic preference — it was paid for twice** (AUDIT C70, C76). C70's first version
# grew the region above the mapping by 40 lines and the register audit failed on the stale window; C75
# added a 13-line `ulimit` comment at line 36 for the very measurement below, and moved law 30's
# citation `208 → 222`, past the audit's ±8 window — while C75's own commit message and AUDIT row
# claimed the audit was green. **Nothing in the tree could tell**: the gate does not run the audit, and
# the audit's own record of the citation was prose. The durable fix is the symbol form
# (`tools/check-lean-conformance.sh:lean_parse_corpus`, which the register is landing); until it lands,
# **any line added above the mapping re-stales law 30's citation**, so keep the header block at its
# current length (the mapping must stay within ±8 lines of the cited line 207).

# --- the stack the elaborator needs, measured ----------------------------------
# `Rchain/Sort.lean`'s comparator blocks are elaborated with tactic cascades over a 24-constructor
# match, and the elaborator's recursion over them is **deeper than the default process stack**. The
# measurement (2026-09-24): with the default 8 MB the module aborts — `Stack overflow detected.
# Aborting.`, exit 134, and `lake build` reports it as a plain build failure with no line number — and
# with 64 MB the *same* file elaborates cleanly (no errors, 16 minutes under a concurrent build).
#
# So this is a budget with a measurement, not a workaround for a broken proof (which is what the abort
# looks like): raising the *soft* limit is the fix, `|| true` because a restricted environment may
# refuse it, and the limit is inherited by every `lake`/`lean` this script spawns. `lean` has the knob
# directly (`-s/--tstack`, in Kb) if a caller would rather pass it than raise the process limit. The
# `ulimit` line itself stays above the mapping with step 1; only this prose could move down.
#
# --- and step 1's own bound, in the same shape: a measurement, not a guess ----------------------
# Step 1 used to run `lake build` **unbounded**, so a module that cannot finish looked like a gate that
# has not finished — indistinguishable, to a reader waiting on it, from a slow one. The bound is
# `timeout 1800` around the whole three-target chain, and the measurement behind the number is CI's:
# the Lean conformance job gets **60 minutes for everything** (`ci.yml:117`), and the cold library build
# is the largest single item in it. A separate measurement from the sort unit's reland attempt, which is
# why the bound is generous: that tree did not finish in 1200 s and reached line 719 of 2,448 — an
# *unfinished* build, not a slow one — so the bound has to be loose enough to pass a real cold build and
# tight enough to become a loud failure rather than a hang. 1800 s does both; if a future build legitimately
# needs more, raise it *with* the measurement that says so, the way this comment does.

# --- 5b. the law register is current -------------------------------------------
# `spec/laws.tsv` and `spec/LAWS.md` are generated from `Rchain/Laws.lean`, and the documents' law counts
# and statuses are supposed to come from them. Committed and re-emitted here for the same reason the
# corpora are: a register that no longer matches the Lean is a catalog that stopped being true — which is
# exactly how both documents came to say "29 laws" while the tree held 43, and how they came to disagree
# with each other on Laws 5 and 24.
if [[ -x "$ROOT/tools/emit-lean-laws.sh" ]]; then
  if "$ROOT/tools/emit-lean-laws.sh" >/tmp/lean-laws.log 2>&1; then
    ok "law register emitted from the Lean"
  else
    fail "law register emission failed — see /tmp/lean-laws.log"
  fi
  dirty="$(cd "$ROOT" && git status --porcelain -- spec/laws.tsv spec/LAWS.md)"
  if [[ -z "$dirty" ]]; then
    ok "committed law register matches the Lean"
  else
    fail "spec/laws.tsv / spec/LAWS.md are not what the Lean defines — re-emit and commit:"
    printf '%s\n' "$dirty" | sed 's/^/      /'
  fi

  # --- 5c. and the documents that quote the register still agree with it ---------
  # `tools/emit-lean-counts.sh` rewrites the totals inside `<!-- counts:KEY -->…<!-- counts:end -->`
  # spans in the reader-facing documents from `spec/laws.tsv`, and fails on a register total written by
  # hand outside one. It is the same emit-and-diff discipline as above, one layer out: the *numbers* in
  # `TYPE-SYSTEM.md`'s and `laws.md`'s prose now come off the register, which is what stops the
  # class of defect this pass found there (three "30 element-comparator axioms" claims, a page saying
  # "48 laws and 57 entries" beside its own "49 laws, 58 entries", a gate comment reading "1..43").
  # Same two defects as the witness step below, found by running the gate: the log was a fixed
  # `/tmp/lean-counts.log` and the verdict was `tail -1` of it. Two artefacts, both measured
  # 2026-09-25 — the double prefix the gate printed until now, `ok    ok    every marked count matches
  # the register (49 laws and 58 entries)` (the child's own line already carries `ok    `, and this
  # step added a second), and, falsified with two scratch children, a **foreign verdict**: the short
  # run printed the other run's line, a different count, as its own. Per-run log; the summary matched
  # as a summary and printed once (the child's own prefix *is* the verdict); a stable path on failure.
  if [[ -x "$ROOT/tools/emit-lean-counts.sh" ]]; then
    counts_log="$(mktemp)"
    counts_rc=0
    "$ROOT/tools/emit-lean-counts.sh" --check >"$counts_log" 2>&1 || counts_rc=$?
    counts_summary="$(grep -E '^ok    every marked count matches the register \(.*\)$' "$counts_log" \
                       | tail -1 || true)"
    if (( counts_rc == 0 )) && [[ -n "$counts_summary" ]]; then
      printf '%s\n' "$counts_summary"
      rm -f "$counts_log"
    else
      kept_counts="/tmp/lean-counts.log"
      cp "$counts_log" "$kept_counts" 2>/dev/null || kept_counts="$counts_log"
      fail "the documents' law counts are not the register's — see $kept_counts:"
      sed 's/^/      /' "$counts_log"
      if (( counts_rc == 0 )); then
        fail "the counts check exited 0 but printed no summary line — the log holds no verdict of its own"
      fi
      rm -f "$counts_log"
    fi
  fi
fi

# --- 5c. the trigger list that runs this gate, checked against what it reads -------------------
#
# `.github/workflows/formal-on-change.yml` runs *this* script when one of a hand-maintained list of
# paths changes, and `nightly.yml` is the backstop. A consumer of a corpus that is not on the list
# re-opens the hole the workflow was built to close: the corpus moves, the consumer is not re-run, and
# the first failure arrives at 03:37, detached from the change that caused it (AUDIT C262). The
# residue the row recorded is exactly this — "the path list is hand-maintained and a new corpus
# consumer not added to it re-opens the hole silently" — and the hole is live: `rholang/src/
# system_processes.rs` reads `spec/conformance/protocol.tsv` from outside the list.
#
# **Why the check is here and not in a new CI step**: this script is itself a listed path, so the
# check travels with the gate it guards and needs no workflow edit to be kept honest. **Why it greps
# rather than reading the consumer mapping above**: the mapping is the same kind of hand-maintained
# list, so a consumer discovered from it would be a consumer already believed in. The grep looks for
# the *code* form (`../spec/conformance/…`, which is how a path is resolved from the manifest dir) and
# not the bare path, because the bare path appears in doc comments in half these files — and a class
# that fires on prose is one people learn to work around.
WF="$ROOT/.github/workflows/formal-on-change.yml"
if [[ -f "$WF" ]]; then
  # The workflow's `paths:` block, as the globs it is written in. The end pattern is the next
  # top-level key, which in this file is `workflow_dispatch:` — so a YAML reshuffle that moved the
  # block would be caught by the empty-parse branch below rather than read as "no drift".
  mapfile -t trigger_globs < <(sed -n '/^ *paths:/,/^ *[a-z_]*:/p' "$WF" | grep -oE '"[^"]+"' | tr -d '"')
  if (( ${#trigger_globs[@]} == 0 )); then
    fail "no paths parsed from .github/workflows/formal-on-change.yml — assume the parser is wrong before the workflow is"
  else
    # Every consumer of a corpus in the tree, plus the inputs this gate reads and runs (which only the
    # gate knows, so it declares them here rather than leaving them to whoever edits the workflow).
    consumers="$(cd "$ROOT" && git grep -lF '../spec/conformance/' -- '*.rs' '*.sh' '*.py' 2>/dev/null | sort -u)"
    gate_inputs="$(printf '%s\n' \
      spec/API-SCHEMA.md \
      tools/emit-lean-laws.sh \
      tools/emit-lean-counts.sh \
      tools/check-rust-witnesses.sh)"
    uncovered=()
    while IFS= read -r f; do
      [[ -n "$f" ]] || continue
      hit=0
      for g in "${trigger_globs[@]}"; do
        # `**` is `*` to bash and `*` crosses `/` in a `[[ == ]]` pattern, so the match is GitHub's
        # for every glob in this list and *wider* for the `**` ones — wider is the safe direction.
        [[ "$f" == $g ]] && { hit=1; break; }
      done
      (( hit )) || uncovered+=("$f")
    done <<< "$consumers
$gate_inputs"
    if (( ${#uncovered[@]} == 0 )); then
      ok "every corpus consumer and gate input is on the formal gate's trigger list"
    else
      fail "formal-on-change.yml does not run this gate for: ${uncovered[*]} — a change there moves a corpus or a gate input without re-running the gate, and the first failure is the nightly"
    fi
    # The other direction: a glob that names nothing is a trigger for a path that moved or was
    # deleted — coverage that covers nothing, which reads as coverage. Expanded in a subshell because
    # `globstar` is process-wide and the sections below expand globs of their own; `fail` is called in
    # *this* shell because a count incremented in a subshell is lost.
    stale="$( (
      shopt -s globstar nullglob
      for g in "${trigger_globs[@]}"; do
        m=( $g )
        (( ${#m[@]} > 0 )) || printf '%s ' "$g"
      done
    ) )"
    if [[ -z "$stale" ]]; then
      ok "every path the formal gate triggers on names something in the tree"
    else
      fail "the formal gate triggers on ${stale}— matching nothing: a path that moved or was deleted, still reading as coverage"
    fi
  fi
fi

# --- the reply catalog is the schema's table ---------------------------------
#
# Law 39's catalog is data in Lean (`spec/Rchain/Protocol.lean`'s `replyCatalog`) and a probe per row
# in Rust; what it must not become is a *second* copy of `spec/API-SCHEMA.md` free to drift from it.
# Every urn the catalog names must have a row there — by name, or by its family, since the schema
# tables the qucalc/gov urns as one row per family.
if [[ -f "$ROOT/spec/conformance/protocol.tsv" ]]; then
  missing=0
  while IFS=$'\t' read -r layer urn _args _kind _slots; do
    [[ "$layer" == "protocol" ]] || continue
    family="${urn%:*}"
    if grep -qF -- "$urn" "$ROOT/spec/API-SCHEMA.md" \
      || grep -qF -- "$family" "$ROOT/spec/API-SCHEMA.md"; then
      :
    else
      fail "law 39's catalog names $urn and spec/API-SCHEMA.md has no row for it (nor for $family)"
      missing=$((missing + 1))
    fi
  done <"$ROOT/spec/conformance/protocol.tsv"
  if ((missing == 0)); then
    ok "every urn in law 39's catalog has a row in spec/API-SCHEMA.md"
  fi
fi

# --- the register's Rust half, run by name -------------------------------------
#
# Step 9 reads the register's *citations* and 5b/5c its counts, but a `provedModel` row's `rust` cell
# names a *file* — and a file does not rot yet is also not evidence: the tests that assert a row's case
# were prose in `falsifiable`/`note`, where nothing read them. `tools/check-rust-witnesses.sh` runs each
# one by name and refuses a name that matches no test. That refusal is the point: `cargo test <filter>`
# exits 0 on an empty match, so a registry of renamed, deleted or `#[ignore]`d tests would otherwise
# run green while checking nothing — the clause checks 9 and 10 also carry.
#
# The list comes from the register's own `rustWitness` column — one source of truth, so a
# witness list kept anywhere else could not become a second copy of the register's judgement. That
# was the shape this step first landed in (`tools/rust-witnesses.txt`, on a red tree where `laws.tsv`
# could not be emitted) and the file is deleted now that the column exists.
#
# **The column is found by name, not by index.** `$14` was the column's position in a schema emitted
# from `Rchain/Laws.lean`, and a position is a fact about the *emitter* that this reader cannot check:
# `audit-test-register.sh` read the same register through named fields for the same reason, before it
# was deleted on 2026-09-27 (and its `IFS=$'\034'` note is the neighbouring trap — a tab-separated
# consumer that lets `read` collapse an empty field shifts every later column silently). A column that
# cannot be found now names itself
# instead of running over an empty list.
#
# **The log is per-run, and the verdict reported is the checker's own summary.** `>/tmp/rust-witnesses.log`
# was shared state of exactly the kind `tools/check-rust-witnesses.sh` itself carried until 2026-09-24:
# two gate runs in this checkout at the same time (routine) opened the same path, and the line *this
# step reports as the gate's verdict* was `tail -1` of a file another process could write. Measured
# 2026-09-25 with a stub `cargo` over two scratch registers (3 and 40 witnesses, the short run started
# second): the short run's `tail -1` read the *long* run's most recently written line and this step
# printed, verbatim, `ok    ` + that line — a witness that is not in its own register, offered as its
# result:
#
#   ok    <stale-offset padding>ok    rholang/src/property_tests.rs:law5_a_pattern_that_binds_a_variable_twice_never_matches (rchain-rholang, 1 test(s))
#
# while its own checker had printed `all 3 Rust witness(es) ran and passed`. Both runs exited 0: the
# gate counted the step green and printed someone else's line. (The padding before the foreign line is
# the hole left when the second run truncated a file the first run's checker still held open at a
# higher offset — the same truncate-under-a-live-writer shape, visible in the artefact.) Now each run
# writes its own `mktemp` file, the success line is a line *matched as the checker's summary* rather
# than whatever sits last in the file, and the failure path still copies the log to the stable
# `/tmp/rust-witnesses.log` a reader has always been pointed at.
witness_col="$(awk -F'\t' 'NR == 1 { for (i = 1; i <= NF; i++) if ($i == "rustWitness") { print i; exit } }' \
                 "$ROOT/spec/laws.tsv")"
if [[ -z "$witness_col" ]]; then
  fail "spec/laws.tsv has no \`rustWitness\` column — the register's Rust half cannot be run (the column is emitted from Rchain/Laws.lean)"
else
  witness_log="$(mktemp)"
  witness_rc=0
  awk -F'\t' -v c="$witness_col" \
    'NR > 1 && $c != "-" { n = split($c, a, ", "); for (i = 1; i <= n; i++) print a[i] }' \
    "$ROOT/spec/laws.tsv" | "$ROOT/tools/check-rust-witnesses.sh" >"$witness_log" 2>&1 || witness_rc=$?
  # `|| true` on a grep that legitimately matches nothing: under this script's `set -euo pipefail` a
  # no-match would abort the gate *before* it reported why.
  witness_summary="$(grep -E '^(all [0-9]+ Rust witness\(es\) ran and passed|===== [0-9]+ of [0-9]+ Rust witness\(es\) FAILED =====)$' \
                       "$witness_log" | tail -1 || true)"
  if (( witness_rc == 0 )) && [[ -n "$witness_summary" ]]; then
    ok "$witness_summary"
    rm -f "$witness_log"
  else
    kept="/tmp/rust-witnesses.log"
    cp "$witness_log" "$kept" 2>/dev/null || kept="$witness_log"
    fail "the register's Rust witnesses do not run — see $kept"
    grep '^FAIL' "$witness_log" | sed 's/^/      /' | head -5 || true
    # A green exit with no summary is not a verdict: it is what the checker's output looks like when
    # something *else* wrote the file it was read from. Say which of the two it was.
    if (( witness_rc == 0 )); then
      fail "the witness checker exited 0 but printed no summary line — the log holds no verdict of its own"
    fi
    rm -f "$witness_log"
  fi
fi

# (`tools/audit-protocols.sh` was to be a *static* audit of the vendored protocol content — a
# "linear consume with no paired produce" walk for law 41 and a call-shape table for law 40. Law 41's
# half was retired unshipped rather than committed with an exception list over vendored content; the
# reasoning is AUDIT §17 C22 item 1. Law 40's call-shape table is still owed, and it lands as a corpus
# layer like the others, not as a hook here: a conditional check for a file that does not exist is a
# green that means nothing.)

echo ""
if (( failures > 0 )); then
  echo "===== $failures formal check(s) FAILED ====="
  exit 1
fi
echo "===== the formal gate is green ====="
