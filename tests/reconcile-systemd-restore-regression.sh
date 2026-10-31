#!/usr/bin/env bash
#
# F-U16-02: the systemd restore copies beside the live data dir and swaps only once the copy has
# arrived. The row was "pinned by reading" because the function had no harness; this drives the real
# `restore_chain_state_from_master` through a fake `ssh` that runs each remote command locally.
#
# Two arms:
#   * a master whose tar fails mid-stream leaves the joiner byte-identical (the old form streamed
#     straight into the live directory and printed "unchanged" while it had half-written it);
#   * a successful copy swaps the joiner's environments and moves a joiner-only environment aside
#     (`transaction.reconcile-orphan`), rather than leaving the divergence residue in place.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/tools/reconcile-network.sh"
bash -n "$SCRIPT"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/reconcile-systemd.XXXXXX")
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

export MASTER_DIR="$WORK/master" JOINER_DIR="$WORK/joiner"
mkdir -p "$MASTER_DIR/blockstorage" "$MASTER_DIR/dagstorage" "$MASTER_DIR/rspace/history" \
         "$JOINER_DIR/blockstorage" "$JOINER_DIR/transaction"
printf 'survivor' > "$MASTER_DIR/blockstorage/data.mdb"
printf 'stale'    > "$JOINER_DIR/blockstorage/data.mdb"
printf 'joiner-only' > "$JOINER_DIR/transaction/tx"
# The survivor does not hold `transaction`; real tar exits 2 on a missing member and a partial archive
# is exactly the case this branch must survive, so ask tar to continue and say so.
export TAR_OPTIONS=--ignore-failed-read

BIN="$WORK/bin"; mkdir -p "$BIN"
cat > "$BIN/ssh" <<'FAKE'
#!/usr/bin/env bash
host=""; cmd=""
for a in "$@"; do case "$a" in root@*) host=${a#root@};; esac; cmd="$a"; done
if [ "$host" = "master" ] && [ -n "${FAIL_MASTER:-}" ] && [[ "$cmd" == tar* ]]; then
  # A truncated stream: some bytes, then a non-zero exit. The joiner must not swap anything.
  printf 'partial'
  exit 1
fi
exec bash -c "$cmd"
FAKE
cat > "$BIN/systemctl" <<'SYSCTL'
#!/usr/bin/env bash
# `systemctl cat <unit>`; the remote command pipes this through grep, so the raw ExecStart line is
# what the fake prints and the real extraction still runs.
case "${2:-}" in
  master-unit) printf 'ExecStart=/usr/bin/rnode --data-dir %s run\n' "$MASTER_DIR" ;;
  joiner-unit) printf 'ExecStart=/usr/bin/rnode --data-dir %s run\n' "$JOINER_DIR" ;;
esac
SYSCTL
chmod +x "$BIN/ssh" "$BIN/systemctl"
export PATH="$BIN:$PATH"

declare -A HOST UNIT
HOST[master]=master; HOST[J]=joiner
UNIT[master]=master-unit; UNIT[J]=joiner-unit
CONTROL=systemd; MASTER=master
CHAIN_ENVS="blockstorage dagstorage rspace/history transaction"
SSH="ssh -n -i unused"
source <(sed -n '/^restore_chain_state_from_master() {/,/^}/p' "$SCRIPT")

test_mid_stream_failure_leaves_joiner_unchanged() {
  export FAIL_MASTER=1
  if restore_chain_state_from_master J >"$WORK/fail.out" 2>"$WORK/fail.err"; then
    unset FAIL_MASTER; return 1
  fi
  unset FAIL_MASTER
  grep -q 'unless the swap had begun' "$WORK/fail.err" || return 1
  [ "$(cat "$JOINER_DIR/blockstorage/data.mdb")" = "stale" ] || return 1
  [ -f "$JOINER_DIR/transaction/tx" ] || return 1
}
test_mid_stream_failure_leaves_joiner_unchanged
echo "PASS a mid-stream failure leaves the joiner unchanged"

fail_with() { echo "FAIL $1" >&2; echo "--- stdout" >&2; cat "$WORK/ok.out" >&2 2>/dev/null; echo "--- stderr" >&2; cat "$WORK/ok.err" >&2 2>/dev/null; return 1; }
test_success_swaps_only_after_copy() {
  restore_chain_state_from_master J >"$WORK/ok.out" 2>"$WORK/ok.err" \
    || fail_with "the success arm returned non-zero" || return 1
  [ "$(cat "$JOINER_DIR/blockstorage/data.mdb")" = "survivor" ] \
    || fail_with "the joiner's blockstorage was not replaced" || return 1
  [ ! -e "$JOINER_DIR/transaction" ] || fail_with "the joiner-only transaction was left standing" || return 1
  [ -f "$JOINER_DIR/transaction.reconcile-orphan/tx" ] \
    || fail_with "the joiner-only environment was not moved aside" || return 1
  grep -q 'moved aside here: transaction' "$WORK/ok.out" \
    || fail_with "the aside is not named in the run" || return 1
  grep -q 'adopted blockstorage:' "$WORK/ok.out" \
    || fail_with "the adopted manifest is missing" || return 1
}
test_success_swaps_only_after_copy
echo "PASS the success arm swaps the copy and moves the joiner-only environment aside"
