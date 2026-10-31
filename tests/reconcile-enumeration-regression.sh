#!/usr/bin/env bash
#
# `reconcile-network.sh` must audit **every** row of its node table, and must say so when it cannot.
#
# This pins issue 488's first defect with a transport that behaves like the real thing: `ssh` reads
# stdin unless `-n` is given. Remove `-n` from the script and the fake consumes the remaining rows,
# the loop ends after one node, the row-count guard refuses — and this test fails if the guard is
# also removed. The fixture is four non-local hosts, so every call goes through the fake ssh.
#
# No Docker, no network, no real ssh: PATH contains only fakes plus the system binaries the prelude
# needs.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/tools/reconcile-network.sh"
if [ "${1:-}" = "--script" ]; then SCRIPT="${2:?--script needs a path}"; fi
bash -n "$SCRIPT"

FAILS=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1" >&2; FAILS=$((FAILS + 1)); }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/reconcile-enum.XXXXXX")
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

FX="$WORK/fx"; BIN="$WORK/bin"; mkdir -p "$FX" "$BIN"
export FX

# --- fixtures -------------------------------------------------------------------------------------
# Four non-local hosts; heights 51..57 so MAXH is unambiguous. Paths are keyed
# <host>_<port>_<path with / -> _>.
hash50="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
for spec in "node-a 1 51" "node-b 2 53" "node-c 3 55" "node-d 4 57"; do
  set -- $spec; host=$1; port=$2; height=$3
  printf '{"latestBlockNumber":%s}\n' "$height" > "$FX/${host}_${port}_api_status"
  printf '{"blockInfo":{"blockNumber":50,"blockHash":"%s"}}\n' "$hash50" \
    > "$FX/${host}_${port}_api_last-finalized-block"
  printf '[{"blockNumber":50,"blockHash":"%s","sender":"s","postStateHash":"p","deployCount":0,"rejectedDeploys":[]}]\n' \
    "$hash50" > "$FX/${host}_${port}_api_blocks_0_57"
done

# --- fakes ----------------------------------------------------------------------------------------
cat > "$BIN/ssh" <<'FAKE'
#!/usr/bin/env bash
# The behaviour that ate the table: real ssh reads stdin unless -n is given.
n_flag=0
for a in "$@"; do [ "$a" = "-n" ] && n_flag=1; done
[ "$n_flag" = "0" ] && cat >/dev/null
host=""; cmd=""
for a in "$@"; do case "$a" in root@*) host=${a#root@};; esac; cmd="$a"; done
url=$(printf '%s' "$cmd" | grep -oE 'http://127\.0\.0\.1:[0-9]+[^ "'"'"']*' | head -1)
[ -z "$url" ] && exit 0
rest=${url#http://127.0.0.1:}; port=${rest%%/*}; path=${rest#"$port"}
key="${host}_${port}_$(printf '%s' "${path#/}" | tr '/' '_')"
[ -f "$FX/$key" ] && cat "$FX/$key"
exit 0
FAKE
chmod +x "$BIN/ssh"

# --- the prelude and §1, up to §2 ----------------------------------------------------------------
slice() { # script
  awk '/^# --- 2\. reported equivocation/{exit} {print}' "$1"
}

table() { # path
  cat > "$1" <<'ROWS'
node-a 1 rnode A master
node-b 2 rnode B
node-c 3 rnode C
node-d 4 rnode D
ROWS
}

run_slice() { # script table out
  local script=$1 tbl=$2 out=$3
  PATH="$BIN:$PATH" HOME="$WORK" TMPDIR="$WORK" \
    RECONCILE_NODES="$tbl" RECONCILE_CONTROL=docker \
    bash -c "$(slice "$script")" > "$out" 2>&1 </dev/null || true
}

table "$WORK/nodes"
run_slice "$SCRIPT" "$WORK/nodes" "$WORK/out"

if grep -q '^  A ' "$WORK/out" && grep -q '^  B ' "$WORK/out" \
   && grep -q '^  C ' "$WORK/out" && grep -q '^  D ' "$WORK/out"; then
  pass "all four node rows are read"
else
  fail "fewer than four node rows read"; sed -n '1,12p' "$WORK/out" >&2
fi

if grep -q 'snapshot: MAXH=57 taken at ' "$WORK/out"; then
  pass "the snapshot height is stated"
else
  fail "no snapshot line"; sed -n '1,12p' "$WORK/out" >&2
fi

# The defect: remove -n and the same run must refuse, naming both counts, before any section.
mutant="$WORK/mutant.sh"
sed 's/^SSH="ssh -n /SSH="ssh /' "$SCRIPT" > "$mutant"
run_slice "$mutant" "$WORK/nodes" "$WORK/out-mutant"
if grep -q 'read 1 of 4 node row(s)' "$WORK/out-mutant" \
   && ! grep -q '== 2\.' "$WORK/out-mutant"; then
  pass "a transport that eats the table is refused, naming both counts"
else
  fail "the no--n mutant did not refuse"; sed -n '1,12p' "$WORK/out-mutant" >&2
fi

# A table that repeats a name weights one validator twice; it must refuse too.
cat > "$WORK/dup" <<'ROWS'
node-a 1 rnode A master
node-a 1 rnode A
ROWS
run_slice "$SCRIPT" "$WORK/dup" "$WORK/out-dup"
if grep -q "node name 'A' appears more than once" "$WORK/out-dup"; then
  pass "a duplicate node name is refused"
else
  fail "the duplicate table was not refused"; sed -n '1,12p' "$WORK/out-dup" >&2
fi

if [ "$FAILS" -ne 0 ]; then
  echo "reconcile-enumeration-regression: $FAILS failure(s)" >&2
  exit 1
fi
echo "reconcile-enumeration-regression: ok"
