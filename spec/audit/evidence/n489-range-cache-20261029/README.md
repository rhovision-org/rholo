# Reconciliation range snapshots during recovery

The baseline is PR #489 at `40efb9599a44bf36f877143a3cb94ccda56700bc`.
Its range cache is populated before recovery and reused by both post-recovery verification phases.
The block content is immutable; a node's inventory of those blocks changes while it recovers.

The [regression suite](../../../../tests/reconcile-cache-regression.py) executes the actual
`blocks_at` helper, range priming and verification phases with disposable API fixtures. No live
node, SSH transport, firewall, validator, service controller or faucet is invoked.

The critical paired controls are:

| Cached inventory | Current API inventory | Required result |
|---|---|---|
| Diverged | Converged | Accept current convergence |
| Agreed | Diverged | Reject current divergence |
| Ends at the original plan height | Finality advanced beyond it | Check the new finalized block |
| Nonempty error, invalid elements, truncated JSON or failed transfer | Per-height endpoint works | Discard the failed snapshot and use live answers |

The corrected script refreshes ranges for each verification round, extends the finality snapshot
through the heights currently being checked, and admits only successful JSON-list transfers into
the cache. A failed refresh removes the previous snapshot. The opt-out reads live per-height data.
One valid range fetch per node is retained for the planning scans.

Run from a clean checkout:

```sh
python3 tests/reconcile-cache-regression.py
python3 tests/reconcile-regression.py
bash -n tools/reconcile-network.sh
```

The exact same new suite is also run against the unmodified baseline extracted with `git show`;
the retained [before log](before.txt) and [after log](after.txt) distinguish regression reproduction
from the control cases. CI executes the existing disposable Docker lifecycle in addition to these
offline cases. The PR links the final source SHA and executable CI run.

This correction does not establish live reconciliation success or repair node-side block-receiver
overflow, proposer justification regression, or mature-chain fresh join. No live apply, chain reset,
validator restart, key, Genesis, service or firewall modification was performed.

The range admission check also rejects an empty `[]` inventory. A non-empty-height node serving an empty list must use live per-height fallback, not cache an apparent absence of blocks. `test_empty_range_does_not_hide_live_blocks` covers this response.
