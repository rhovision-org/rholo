#!/usr/bin/env bash
#
# check-rust-witnesses.sh — run every registered Rust witness *by name*.
#
# The law register (`spec/Rchain/Laws.lean` → `spec/laws.tsv`) names, for each proved row, the Lean
# declarations its status rests on (`witness`, checked by name against the elaborated environment).
# The Rust half of the same claim had no machine half at all: a row's `rust` cell names a *file*, and
# a file does not rot but it is also not evidence — the tests that actually assert a row's case were
# named in prose (`falsifiable`/`note`) where nothing read them. `rustWitness` is that half, entries
# of the shape `path.rs:symbol`, and this is what makes it *executable*: the named test is run, and a
# name that matches no test fails.
#
# **The zero-match refusal is the point, not a detail.** `cargo test <filter>` exits 0 when the filter
# matches nothing, so a registry of renamed, deleted or `#[ignore]`d tests would run green while
# checking nothing — the fifth blind instrument this pass has found, and the same clause
# `tools/audit-test-register.sh`'s checks 9 and 10 carried, before that gate was deleted on
# 2026-09-27 ("a check that matched nothing is not evidence"). A witness whose symbol is not even
# *in* its file is refused before cargo runs.
#
# Usage:  tools/check-rust-witnesses.sh [file]      (default: stdin; `-` also means stdin)
#         the input is one `path.rs:symbol` per line; blank lines and `#` comments are skipped.
#
# **Duplicate entries are executed once; the census still counts every entry.** The register may cite
# one witness from more than one row (420 entries, 388 distinct on 2026-11-24). Running the same test
# twice proves nothing the first run did not, so execution is deduplicated — but the verdict is still
# printed once per *input* entry, in input order, and the summary counts the register's claims (420),
# not the distinct tests. Otherwise a duplicated row would silently change the census's total.
#
# `RUST_WITNESS_JOBS` (default 1) bounds how many witnesses run at once. Each worker runs exactly the
# `cargo test` the serial census always ran, writes its own per-run log and verdict file, and the
# parent prints the verdicts in input order. With 1 the output is byte-identical to the old serial
# loop; with N the log order, the verdicts and the summary are unchanged. The census prints its
# slowest witnesses at the end — that is the measurement a future parallelism change should rest on,
# not an assumption that more jobs are faster.
#
# The register-side caller is `tools/check-lean-conformance.sh` once the field is populated:
#   awk -F'\t' 'NR>1 { n=split($N, a, ", "); for (i=1;i<=n;i++) if (a[i] != "-") print a[i] }' \
#     spec/laws.tsv | tools/check-rust-witnesses.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INPUT="${1:--}"

# --- this run's logs ---------------------------------------------------------------------------
# **A log path derived from the witness alone is shared state.** Two gate runs in this checkout at
# the same time is routine (a local gate run and a peer session's), and they compute the *same*
# `/tmp/rust-witness-<slug>.log` for the same witness: the second run's `>"$log"` truncates on open,
# so if it opens the file between the first run's cargo exiting and the first run's `grep -c`, the
# first run reads an empty file and reports `matched no test` for a test that ran and passed.
# Observed 2026-09-24: a concurrent gate run reported six witnesses as `matched no test` and a
# serial re-run of the same six found two of them `ok (1 test)` with the harness line plainly in the
# log. The checker therefore manufactured *false failures* — worse than missing one, because a
# defect in the instrument is read as a defect in the tree.
#
# So the work happens in a per-run directory, and nothing is shared. A failing witness's log is
# copied out to the stable, greppable name the FAIL line quotes (the same name the script has always
# printed) before the run directory is cleaned up; a passing run leaves nothing behind.
LOGDIR="$(mktemp -d "${TMPDIR:-/tmp}/rust-witness.XXXXXX")"
trap 'rm -rf "$LOGDIR"' EXIT
slug_of() { printf '%s' "$1" | tr -c 'A-Za-z0-9' '-'; }

# The first few lines of a failed log worth showing a human. **Never fails**: `grep` that matches
# nothing exits 1, and a pipeline is a single command to `set -e`, so a bare call here would end the
# run at the first build-failing witness (see the call site). A build error is exactly the case with
# nothing to match, which is why this cannot be left out.
show_log_extract() {
  grep -E '^(test .* FAILED|^error(\[|:)|assertion|thread .* panicked)' "$1" 2>/dev/null \
    | head -4 | sed 's/^/      /' || true
  return 0
}

# --- the crate a path belongs to ---------------------------------------------------------------
# Derived from the path's first segment rather than a per-witness field: the workspace is one crate
# per sbt module, so the path already says which crate the symbol compiles into, and a second copy
# of that mapping in the register would be a second thing to drift.
crate_of() {
  case "${1%%/*}" in
    sdk)           echo rchain-sdk ;;
    shared)        echo rchain-shared ;;
    crypto)        echo rchain-crypto ;;
    graphz)        echo rchain-graphz ;;
    models)        echo rchain-models ;;
    block-storage) echo rchain-block-storage ;;
    comm)          echo rchain-comm ;;
    rspace)        echo rchain-rspace ;;
    rholang)       echo rchain-rholang ;;
    casper)        echo rchain-casper ;;
    ocapn)         echo rchain-ocapn ;;
    node)          echo rchain-node ;;
    *)             echo "" ;;
  esac
}

total=0
failures=0
passed=0
skip_note=""

# --- read, deduplicate, and bound the pool -----------------------------------------------------
# The input is read once, in order. `input` holds every entry the census must count; `unique` holds
# the distinct entries whose tests are actually run; `entry_unique[i]` maps an input entry to its
# unique index. The associative array is keyed by the whole `path.rs:symbol` line, because that is
# the unit the register cites.
input=()
unique=()
declare -A entry_unique=()
while IFS= read -r line || [[ -n "$line" ]]; do
  [[ -z "${line// }" || "${line:0:1}" == "#" ]] && continue
  total=$((total + 1))
  input+=("$line")
  if [[ -z "${entry_unique["$line"]+x}" ]]; then
    entry_unique["$line"]=${#unique[@]}
    unique+=("$line")
  fi
done < <(if [[ "$INPUT" == "-" ]]; then cat; else cat "$INPUT"; fi)

# A check that scanned nothing is not evidence, exactly as in the register's checks 9 and 10.
if (( total == 0 )); then
  echo "FAIL  no witnesses on the input — this check is vacuous${skip_note}"
  exit 1
fi

# The only concurrency knob. A malformed or zero value means 1 — the serial behaviour — rather than
# an error, so a typo cannot silently serialise a run meant to be parallel or spawn an unbounded
# number of cargo processes.
jobs="${RUST_WITNESS_JOBS:-1}"
if [[ ! "$jobs" =~ ^[0-9]+$ ]] || (( jobs < 1 )); then
  jobs=1
fi

# --- one witness, one process ------------------------------------------------------------------
# The worker writes the text to print (`print.<idx>`) and a machine-readable status
# (`status.<idx>`: `OK`/`FAIL` and elapsed millis), both private to this run. It prints nothing to
# stdout: the parent owns output order, which is what keeps `RUST_WITNESS_JOBS=1` byte-identical to
# the old serial loop. A stderr progress line is the only live signal, and it is not a verdict.
run_witness() {
  local idx="$1"
  local line="${unique[$idx]}"
  local path="${line%%:*}"
  local symbol="${line#*:}"
  local crate target started elapsed log kept matched
  local -a cargo_args=()
  crate="$(crate_of "$path")"
  # An integration test lives in `<crate>/tests/<file>.rs` and is named by its target; a unit test
  # is in the library, and `--lib` keeps cargo out of the integration targets' build.
  if [[ "$path" == */tests/* ]]; then
    target="${path##*/tests/}"
    cargo_args=(--test "${target%.rs}")
  else
    cargo_args=(--lib)
  fi
  log="$LOGDIR/witness.$idx.log"
  started="$(date +%s%3N)"
  printf 'progress %s\n' "$line" >&2
  if ! (cd "$ROOT" && cargo test -p "$crate" "${cargo_args[@]}" "$symbol" >"$log" 2>&1); then
    kept="/tmp/rust-witness-$(slug_of "$path:$symbol").log"
    cp "$log" "$kept" 2>/dev/null || true
    elapsed=$(( $(date +%s%3N) - started ))
    {
      printf 'FAIL  %s:%s — the test failed (see %s)\n' "$path" "$symbol" "$kept"
      # **Failure-tolerant on purpose.** This extraction matches nothing whenever cargo failed
      # *before* running a test — a build error is a failing witness whose log has no `test …
      # FAILED` line. Under `set -euo pipefail` a grep that matches nothing exits 1 and would kill
      # the worker, losing the verdict it exists to report. The `|| true` is inside
      # `show_log_extract` for the same reason it always was; the census count is the thing that
      # must survive.
      show_log_extract "$kept"
    } >"$LOGDIR/print.$idx"
    printf 'FAIL %s\n' "$elapsed" >"$LOGDIR/status.$idx"
    return 0
  fi
  # The clause that makes the rest evidence: the run must have *run* something. Counted from the
  # harness's own per-test lines, and the symbol's own name must be among them (a substring filter
  # may match a family, which is fine — a witness that matches only others is not).
  # `ok$` and not a bare ` \.\.\.` (AUDIT C128): libtest prints `test <name> ... ignored` for a
  # filtered-in `#[ignore]`d test, which matched the looser pattern and made an ignored witness read
  # as a passing one — and `cargo test` exits 0 when the filter selects only ignored tests, so
  # nothing else noticed. A probe witness `#[test] #[ignore] fn lawZZ_probe_ignored_witness() {
  # panic!("never runs") }` printed `all 1 Rust witness(es) ran and passed`, exit 0, before this
  # line; it now matches nothing and fails here. The file's own header promises exactly that ("a
  # registry of renamed, deleted or `#[ignore]`d tests would run green while checking nothing").
  matched="$(grep -cE "^test (.*::)?${symbol} \.\.\. ok$" "$log" || true)"
  elapsed=$(( $(date +%s%3N) - started ))
  if (( matched == 0 )); then
    printf 'FAIL  %s:%s — matched no test (`cargo test` exits 0 on an empty filter)\n' \
      "$path" "$symbol" >"$LOGDIR/print.$idx"
    printf 'FAIL %s\n' "$elapsed" >"$LOGDIR/status.$idx"
    return 0
  fi
  printf 'ok    %s:%s (%s, %s test(s))\n' "$path" "$symbol" "$crate" "$matched" \
    >"$LOGDIR/print.$idx"
  printf 'OK %s\n' "$elapsed" >"$LOGDIR/status.$idx"
}

# --- the entries that cannot reach cargo -------------------------------------------------------
# The old loop's pre-flight refusals, in its exact words, decided synchronously so no cargo is paid
# for. They are recorded in the same print/status shape as a worker's verdict, so the count below
# has one path.
schedulable=()
for idx in "${!unique[@]}"; do
  line="${unique[$idx]}"
  path="${line%%:*}"
  symbol="${line#*:}"
  if [[ -z "$path" || -z "$symbol" || "$path" == "$line" ]]; then
    printf 'FAIL  `%s` is not `path.rs:symbol`\n' "$line" >"$LOGDIR/print.$idx"
    printf 'FAIL 0\n' >"$LOGDIR/status.$idx"
  elif [[ ! -f "$ROOT/$path" ]]; then
    printf 'FAIL  %s: no such file\n' "$path" >"$LOGDIR/print.$idx"
    printf 'FAIL 0\n' >"$LOGDIR/status.$idx"
  elif ! grep -qE "fn ${symbol}[^A-Za-z0-9_]" "$ROOT/$path"; then
    # The symbol must be *in* the file: cheap, and it catches a rename before cargo is paid for.
    printf 'FAIL  %s:%s — no `fn %s` in that file\n' "$path" "$symbol" "$symbol" \
      >"$LOGDIR/print.$idx"
    printf 'FAIL 0\n' >"$LOGDIR/status.$idx"
  elif [[ -z "$(crate_of "$path")" ]]; then
    printf 'FAIL  %s: %s is not a workspace crate\n' "$path" "${path%%/*}" \
      >"$LOGDIR/print.$idx"
    printf 'FAIL 0\n' >"$LOGDIR/status.$idx"
  else
    schedulable+=("$idx")
  fi
done

# --- run the schedulable witnesses, at most `jobs` at a time -----------------------------------
running=0
for idx in "${schedulable[@]}"; do
  run_witness "$idx" &
  running=$((running + 1))
  if (( running >= jobs )); then
    wait -n || true
    running=$((running - 1))
  fi
done
wait || true

# --- print every input entry's verdict, in input order, and count it ---------------------------
# A duplicate entry prints the same verdict again (it is the same test) but still counts as its own
# claim. A missing status file is a worker that died between spawning and recording; it is a FAIL
# rather than a silent gap, because `passed + failures == total` is what makes the summary a census.
for line in "${input[@]}"; do
  idx="${entry_unique["$line"]}"
  if [[ ! -f "$LOGDIR/status.$idx" ]]; then
    printf 'FAIL  %s — no verdict was recorded for this witness\n' "$line" >"$LOGDIR/print.$idx"
    printf 'FAIL 0\n' >"$LOGDIR/status.$idx"
  fi
  cat "$LOGDIR/print.$idx"
  read -r kind elapsed <"$LOGDIR/status.$idx"
  if [[ "$kind" == "OK" ]]; then
    passed=$((passed + 1))
  else
    failures=$((failures + 1))
  fi
done

# The slowest *distinct* witnesses, not the slowest input lines: a duplicate entry would otherwise
# appear twice and the pre-flight refusals (which never ran cargo) would pad the list with 0 s.
slow=()
for idx in "${!unique[@]}"; do
  [[ -f "$LOGDIR/status.$idx" ]] || continue
  read -r kind elapsed <"$LOGDIR/status.$idx"
  (( elapsed > 0 )) && slow+=("$elapsed ${unique[$idx]}")
done
printf 'metric witnesses=%s unique=%s jobs=%s\n' "$total" "${#unique[@]}" "$jobs"
if (( ${#slow[@]} > 0 )); then
  # `|| true` is load-bearing: `head -5` closes the pipe after five lines, `sort` then exits 141
  # (SIGPIPE), and under `set -o pipefail` that status would end the script *after* the census and
  # *before* its summary — the exact failure shape the file's header warns about. Observed here with
  # 388 distinct witnesses on 2026-11-24.
  printf '%s\n' "${slow[@]}" | sort -rn | head -5 | while read -r ms witness; do
    printf 'metric slowest %ss %s\n' "$((ms / 1000))" "$witness"
  done || true
fi

# A check that scanned nothing is not evidence, exactly as in the register's checks 9 and 10.
if (( total == 0 )); then
  echo "FAIL  no witnesses on the input — this check is vacuous${skip_note}"
  exit 1
fi
echo ""
# Every witness read must reach a verdict, and the summary must print whatever it is: a *census* is
# only a census if it counts itself. Before 2026-09-24 the failing-test branch could end the script
# early, and a truncated run's silence about the remaining witnesses was indistinguishable from their
# passing. `total == passed + failures` is the invariant that makes the summary below meaningful, and
# a future `continue` path that increments neither is exactly how this instrument would go quiet
# again — so it fails rather than under-reporting.
if (( passed + failures != total )); then
  echo "FAIL  census incomplete: $total witness(es) read but only $((passed + failures)) reached a verdict — a path skipped one"
  exit 1
fi
if (( failures > 0 )); then
  echo "===== $failures of $total Rust witness(es) FAILED ====="
  exit 1
fi
echo "all $total Rust witness(es) ran and passed"
