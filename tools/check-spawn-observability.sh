#!/usr/bin/env bash
#
# check-spawn-observability — every task the node detaches on the ingress path is either observed or
# named with a reason.
#
# **What this exists for, measured.** The audit's structural pass found the node had **no supervisor**:
# every long-lived background task was a bare `tokio::spawn` whose `JoinHandle` was dropped, so nothing
# could tell a task that was running from one that had died at start-up. A panic reached stderr through
# the default hook; a plain early `return` — the loop that exits on its first error, the future that
# completes when its channel closes — produced **no line anywhere**, and the node then served with that
# subsystem silently gone (AUDIT C254). `rchain_shared::supervise::spawn_supervised` is the fix at the
# six sites the audit named. What a fix at six sites does not do is stop the seventh: this is the check
# the row's own close condition asks for — "a spawned task that matters is either awaited, held in a
# `JoinSet`, or **listed with a reason by a source check**".
#
# **Why a source check rather than a test.** The fact is in the *shape* of the call: whether a handle is
# kept. A test cannot assert it — it would have to observe the absence of an observation — and the
# compiler cannot either, because discarding a `JoinHandle` is legal and idiomatic. `AUDIT C175`'s
# ingress-queue bound is the same kind of fact and got the same kind of check
# (`tools/check-bounded-ingress-queues.sh`); this one is wider in file scope and narrower in rule.
#
# **The three verdicts, and why only the third needs a roster.**
#
#   - **supervised** — `rchain_shared::supervise::spawn_supervised(`, whose death increments
#     `tasksPanicked`/`tasksExited` and prints. Nothing to review.
#   - **held** — the handle is bound (`let mut x = tokio::spawn(…)`, `handles.push(tokio::spawn(…))`),
#     which is what the serve loops do so they can be `select!`ed on and awaited at shutdown. The
#     binding is the observation — the *test* is that something does something with the binding, and
#     that is review, not grep.
#   - **bare** — the handle is dropped. This is where a subsystem can stop without a word, so each one
#     is on the roster below **with the reason it is acceptable**, or the run fails.
#
# **Scope: the ingress path**, not the tree — 37 production spawn sites exist today and most are not
# this shape of risk. The files are listed in `INGRESS`; adding one is a deliberate act.
#
# **What a green run does not mean.** That every task here matters-and-is-observed: it means every task
# here is *classified*, and the bare ones carry a reason a reviewer wrote. A reason can be wrong, and a
# task can matter in a way its author did not foresee; what the check prevents is the silent addition,
# not the wrong judgement.
#
# Usage: tools/check-spawn-observability.sh          # report
#        tools/check-spawn-observability.sh --gate   # exit non-zero on a violation (for CI)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
gate=0
[[ "${1:-}" == "--gate" ]] && gate=1

# The ingress path: what a peer's traffic reaches, and what the node runs to keep serving. A bare spawn
# here is the C254 shape with the most consequence — the subsystem stops and the node keeps answering.
INGRESS=(
  node/src/runtime/node_runtime.rs
  comm/src/transport/grpc_transport_receiver.rs
  casper/src/blocks/block_receiver.rs
  casper/src/blocks/block_processor.rs
  casper/src/engine/node_launch.rs
  casper/src/engine/node_syncing.rs
  node/src/api/ocapn.rs
  node/src/instances/proposer_instance.rs
)

# **The roster of bare spawns, each with the reason it is allowed to be bare.**
#
# Keyed `file;;fn` — the enclosing function, not a line number (a line-keyed entry fails *open*: the
# site moves and the entry silently matches a different one, which is the lesson the panic class in
# `tools/audit-type-system.sh` paid for). A rename therefore stales the entry *and* the site, and the
# run reports an entry that claims no site rather than passing on it.
SPAWN_ALLOW=(
  'node/src/runtime/node_runtime.rs;;wire_block_processing_observed;;the block pipelines loops (load_blocks, processor) and the three U19 tasks (start-up re-offer, fault retry, stored-body audit): they run for the life of the process. A stopped pipeline shows as a queue that stops draining while the node still answers (the shape C175 bounds by size); a stopped retry/audit leaves the disposition ledger readable, and the fault/orphan surfaces on /health are the evidence that the task that would have cleared them is not running'
  'node/src/runtime/node_runtime.rs;;spawn_peer_message_router;;the peer-message router: its counterpart is the transport receiver that feeds it, so a router that has stopped leaves sends refused at the receiver rather than silence'
  'node/src/runtime/node_runtime.rs;;build_gateway;;the gateway in-flight recovery: a one-shot taken at boot, and its outcome is the coordinator ledger rather than the task staying alive'
  'node/src/runtime/node_runtime.rs;;setup_shard_runtime;;two sites: the node_launch wrapper, which logs the error it exits with, and the proposer-stream drain, whose death shows as a proposer whose stream stops being consumed'
  'node/src/runtime/node_runtime.rs;;setup_shard;;the deployer-index backfill: a one-shot, and its progress is the index the API reads by deployer'
  'node/src/runtime/node_runtime.rs;;deploy_relay_from_env;;a one-shot task per admitted deploy: its failure is a peer that did not receive the offer, and the deploy stays in the local pool for the normal block path — it is not a subsystem this node needs to keep serving'
  'node/src/runtime/node_runtime.rs;;tap_validated_blocks;;the deploy tap (consume_observed_queue): the pool it fills is readable through the deploys endpoint, so a dead tap reads as a pool that never grows'
  'casper/src/blocks/block_receiver.rs;;apply_with_queue_observer;;the two block-pipeline consumers (incoming_blocks, validated_blocks): each has a sender for its counterpart, and a consumer that has stopped leaves its sender with nobody to refuse'
  'casper/src/engine/node_launch.rs;;start_catchup_if_anchored;;the catch-up driver: it holds the ingest window and releases it on every exit path, so its death restores the ordinary ingest rather than leaving a narrowed one. **It moved out of `apply` in #320** — the running phase now starts one per phase (a reset re-runs the walk from the anchor it rewound to), so the spawn is its own function and this entry moved with it'
  'casper/src/engine/node_syncing.rs;;on_finalized_fringe_message;;the LFS sync loop: it signals finished or terminal, and the boot path awaits both, so the task being dropped is not the same as it being unobserved'
  'comm/src/transport/grpc_transport_receiver.rs;;accept_tls;;the per-connection handshake: it ends when its connection does, which is the C254 note that a return is not a fault, and supervising it would count every healthy handshake as a death'
  'comm/src/transport/grpc_transport_receiver.rs;;send;;the per-connection dispatch task: as above'
  'comm/src/transport/grpc_transport_receiver.rs;;stream;;the per-connection blob stream: as above'
  'node/src/api/ocapn.rs;;accept_transport;;a per-session OCapN serve loop: it ends with its session'
  'node/src/instances/proposer_instance.rs;;create_with_retry;;the proposer not-due retry: a paced one-shot, and the proposer state it feeds is the health cell the status surface reads'
)

# The panic-class scan's shape, for the same reason: a `#[cfg(test)]` module is full of spawns that
# exist to be dropped, and a class that fires on tests is one people learn to work around.
SPAWN_SITES_AWK='
function braces(s,   o, c, i, ch) { o = 0; c = 0; for (i = 1; i <= length(s); i++) { ch = substr(s, i, 1); if (ch == "{") o++; else if (ch == "}") c++ } return o - c }
BEGIN { skip = 0; depth = 0; fn = "(module scope)"; fndepth = 0; nstack = 0 }
{
  if (skip == 0) {
    if ($0 ~ /^[[:space:]]*#\[cfg\(test\)\]/) { skip = 1; depth = 0; next }
    if ($0 ~ /^[[:space:]]*#\[(tokio::)?test\]/) { skip = 1; depth = braces($0); if (depth <= 0) skip = 0; next }
  } else {
    depth += braces($0)
    if (depth <= 0) skip = 0
    next
  }
  if ($0 ~ /^[[:space:]]*\/\//) next
  # **The enclosing function, tracked as a stack.** A spawn inside a nested `fn` must be keyed by that
  # inner name, and the name must go back to the outer one when the inner body closes — otherwise the
  # key of every later site in the file changes the moment anybody adds a helper, which would report
  # as a stale roster entry on an unrelated change.
  if ($0 ~ /^[[:space:]]*(pub\(crate\) |pub )?(async )?fn [A-Za-z_]/) {
    name = $0
    sub(/^[[:space:]]*(pub\(crate\) |pub )?(async )?fn /, "", name)
    sub(/[^A-Za-z0-9_].*$/, "", name)
    if (name != "") { depth += braces($0); nstack++; stackname[nstack] = fn; stackdepth[nstack] = depth; fn = name; fndepth = depth; next }
  }
  depth += braces($0)
  while (nstack > 0 && depth < stackdepth[nstack]) { fn = stackname[nstack]; nstack-- }
  if ($0 !~ /spawn_supervised\(|tokio::spawn\(/) next
  print fn "\t" FNR "\t" $0
}
'
note() { printf 'FAIL  %s\n' "$*"; violations=$((violations + 1)); }
violations=0
sites=0
supervised=0
held=0
bare=0

echo "check-spawn-observability: detached tasks on the ingress path, in ${#INGRESS[@]} file(s)"

while IFS= read -r f; do
  [[ -f "$ROOT/$f" ]] || { note "$f is listed as an ingress file and is not in the tree"; continue; }
  while IFS=$'\t' read -r fn line text; do
    [[ -n "$text" ]] || continue
    sites=$((sites + 1))
    if [[ "$text" == *spawn_supervised\(* ]]; then
      supervised=$((supervised + 1))
      continue
    fi
    # **Held** — the handle is bound or pushed, which is how the serve loops are `select!`ed on. The
    # regexes are the two shapes the census found: `let [mut] x = tokio::spawn(` and
    # `…push(tokio::spawn(`; anything else is bare and needs a reason.
    if [[ "$text" =~ let[[:space:]]+(mut[[:space:]]+)?[A-Za-z_][A-Za-z0-9_]*[[:space:]]*=[[:space:]]*tokio::spawn\( ]] \
       || [[ "$text" == *\.push\(tokio::spawn\(* ]]; then
      held=$((held + 1))
      continue
    fi
    bare=$((bare + 1))
    claimed=0
    reason=""
    for entry in "${SPAWN_ALLOW[@]}"; do
      efile="${entry%%;;*}"
      rest="${entry#*;;}"
      efn="${rest%%;;*}"
      ereason="${rest#*;;}"
      if [[ "$f" == "$efile" && "$fn" == "$efn" ]]; then
        claimed=$((claimed + 1))
        reason="$ereason"
      fi
    done
    if (( claimed == 0 )); then
      note "$f:$line ($fn): a bare tokio::spawn with no reason on the roster — its death would be \
silent, which is C254's whole subject"
    elif (( claimed > 1 )); then
      note "$f:$line ($fn): claimed by $claimed roster entries; an entry must name one site"
    else
      printf '  ok    bare, with a reason: %s:%s (%s)\n        %s\n' "$f" "$line" "$fn" "$reason"
    fi
  done < <(awk -F'\t' "$SPAWN_SITES_AWK" "$ROOT/$f")
done < <(printf '%s\n' "${INGRESS[@]}")

# **The roster is checked against the sites, not only the sites against the roster.** An entry whose
# function no longer contains a bare spawn suppresses nothing while reading as a review — the same
# staleness clause the panic class carries, for the same reason.
while IFS= read -r entry; do
  [[ -n "$entry" ]] || continue
  efile="${entry%%;;*}"
  rest="${entry#*;;}"
  efn="${rest%%;;*}"
  hit=0
  while IFS=$'\t' read -r fn _line text; do
    [[ "$fn" == "$efn" ]] || continue
    [[ "$text" == *spawn_supervised\(* ]] && continue
    if [[ "$text" =~ let[[:space:]]+(mut[[:space:]]+)?[A-Za-z_][A-Za-z0-9_]*[[:space:]]*=[[:space:]]*tokio::spawn\( ]] \
       || [[ "$text" == *\.push\(tokio::spawn\(* ]]; then continue; fi
    hit=1
  done < <(awk -F'\t' "$SPAWN_SITES_AWK" "$ROOT/$efile" 2>/dev/null)
  (( hit )) || note "roster entry $efile::$efn claims no bare spawn (stale): the site moved, was \
converted, or was renamed, and this entry is now a review of nothing"
done < <(printf '%s\n' "${SPAWN_ALLOW[@]}")

printf '\n  %s site(s): %s supervised, %s held, %s bare\n' "$sites" "$supervised" "$held" "$bare"
if (( sites == 0 )); then
  note "the scan found no spawn site at all — an empty scan is not a green result, it is a broken one"
fi

if (( violations > 0 )); then
  printf '\ncheck-spawn-observability: %s violation(s)\n' "$violations" >&2
  (( gate )) && exit 1
  exit 0
fi
echo "check-spawn-observability: every detached task on the ingress path is observed or reasoned"
