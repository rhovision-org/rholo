#!/usr/bin/env bash
#
# check-bounded-ingress-queues — every queue on the ingress -> validate -> process path is bounded.
#
# **Why a source check and not a test.** The defect it guards (C175) is visible in a *type*:
# `wire_block_processing` returns `mpsc::UnboundedSender<BlockMessage>` for the validated-blocks queue,
# so a unit test could only assert the boundedness by failing to compile against today's signature, and
# a test that does not compile is not a test. The function's other arguments need `CommState`/`ShardParts`
# fixtures, which makes a runtime test expensive for a fact that is textual. So the assertion lives here,
# in the repo's `tools/check-*.sh` idiom.
#
# **It was written red, and it is green now** (C175 landed 2026-10-01): all four queues on the path are
# bounded, and the check runs in the lint job with `--gate`, so this is a gate rather than a report. The
# history is kept because a reader who finds a `--gate` invocation should know whether it was always
# satisfied; it was not, and the four bindings below are the ones that changed.
#
# What it looks for, and why each is on this path: a peer streams valid-signed blocks, so any unbounded
# queue between the transport and the processor lets a peer fill memory faster than the node drains it.
# R15 (`spec/findings.tsv`) closed exactly this for the processor-input channel; C175 is the half it
# missed, and the tap channel beside it.
#
# Usage: tools/check-bounded-ingress-queues.sh          # report
#        tools/check-bounded-ingress-queues.sh --gate   # exit non-zero on a violation (for CI)
#        tools/check-bounded-ingress-queues.sh --source PATH [--gate]  # test a fixture
set -uo pipefail
cd "$(dirname "$0")/.."

GATE=0
SRC=node/src/runtime/node_runtime.rs
while (( $# )); do
  case "$1" in
    --gate) GATE=1 ;;
    --source) shift; SRC="${1:-}" ;;
    --source=*) SRC="${1#*=}" ;;
    *) echo "check-bounded-ingress-queues: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done
[[ -r "$SRC" ]] || { echo "check-bounded-ingress-queues: no $SRC" >&2; exit 2; }

violations=0
report() {
  printf '  %-28s %-14s %s\n' "$1" "$2" "$3"
  [[ "$2" != "bounded" ]] && violations=$((violations + 1))
}

echo "check-bounded-ingress-queues: queues on the ingress -> validate -> process path in $SRC"
echo
printf '  %-28s %-14s %s\n' QUEUE BOUND SITE
printf '  %-28s %-14s %s\n' ----- ----- ----

# Each queue is identified by the binding that creates it, not by its constructor call — the
# constructor text alone appears three times (a test's channel among them) and would report the wrong
# site for two of the three queues.
declare -a QUEUES=(
  "incoming_blocks|let (incoming_blocks_tx"
  "processor_input|let (processor_input_tx"
  "validated_blocks|let (validated_blocks_tx"
  "tap_validated_blocks|let (tap_tx"
)
while IFS='|' read -r name binding; do
  site="$(grep -nF "$binding" "$SRC" | head -1)"
  if [[ -z "$site" ]]; then
    report "$name" "absent" "no binding matching '$binding' — has the pipeline been renamed?"
    continue
  fi
  line="${site%%:*}"
  # The constructor often sits on the line after the destructuring, so read the whole statement
  # up to its `;`, not just the binding line.
  statement="$(awk -v start="$line" 'NR >= start { printf "%s ", $0; if (index($0, ";")) exit }' "$SRC")"
  if [[ "$statement" == *"UnboundedSender"* \
        || "$statement" == *"unbounded_channel"* \
        || "$statement" == *"mpsc::unbounded"* ]]; then
    report "$name" "unbounded" "${SRC}:${line}"
  elif [[ "$statement" =~ mpsc::channel[[:space:]]*\([[:space:]]*([A-Za-z_][A-Za-z0-9_:]*)[[:space:]]*\) ]]; then
    bound="${BASH_REMATCH[1]}"
    if [[ "$bound" == "usize::MAX" ]]; then
      report "$name" "unpinned" "${SRC}:${line} (channel(usize::MAX))"
    else
      report "$name" "bounded" "${SRC}:${line} ($bound)"
    fi
  elif [[ "$statement" == *"mpsc::channel"* ]]; then
    report "$name" "unpinned" "${SRC}:${line} (channel argument is not a named constant)"
  else
    report "$name" "unknown" "${SRC}:${line} (no mpsc channel constructor in the statement)"
  fi
done < <(printf '%s\n' "${QUEUES[@]}")

echo
if (( violations > 0 )); then
  cat >&2 <<EOF
check-bounded-ingress-queues: $violations violation(s) (absent, unbounded, unpinned or unknown) on the ingress -> validate -> process path.

This is C175 (spec/findings.tsv), and it is the defect this check exists to gate. R15's fix is the
pattern: bound the queue with backpressure -- \`mpsc::channel(N)\` and an awaiting \`send\` -- and change
\`wire_block_processing\`'s return type from \`mpsc::UnboundedSender<BlockMessage>\` to
\`mpsc::Sender<BlockMessage>\` so the type carries the bound.

**The observation half landed in #120** (a queue observer at the validated, autopropose and attestation
stages), so the depth is readable for the first time and the queue is no longer unmeasured. What C175
owed was the bound itself, and this is the check that row closes on.

The bound is `mpsc::channel(N)` plus an awaiting `send`, which is what makes the producer wait rather
than the queue grow. If this fires again, that is what to restore.
EOF
  (( GATE )) && exit 1
  exit 0
fi

echo "check-bounded-ingress-queues: every queue on the path is bounded."
