# M5-01 — the 840/841 source bundle: INCOMPLETE

**Status: INCOMPLETE, not a pass.** This record exists so the M5 replay's input is a named,
reproducible capture rather than a claim.

## Why it is incomplete

The bundle is the five block **preimages** (the four 840 parents and 841) plus the `Root` state reads
from two nodes. It is perishable: it lives only on a node that still retains blocks 835..845.

- No node retaining that range was reachable from the session that committed this record.
- The #420 thread's [Root witness comment](https://github.com/rhovision-org/rholo/issues/420#issuecomment-6377520882)
  has **no attachment**, and says the ~70 KB bundle was never committed. It is a provenance and
  prose record, not source-pinned bytes.
- **No fixture was substituted.** A fixture would make the replay green over data that is not the
  incident, which is the failure this record exists to avoid.

## The instrument

`tools/n420-root-capture.py` (committed with this record) captures the bundle from any node that
still has it:

```sh
python3 tools/n420-root-capture.py \
  --out spec/audit/evidence/n420-root-reads \
  --endpoint http://127.0.0.1:40403 --endpoint http://127.0.0.1:41403 \
  --block <841 full hex> --block <parent-1 full hex> --block <parent-2 full hex> \
  --block <parent-3 full hex> --block <parent-4 full hex> \
  --rnode ./target/release/rnode
```

The short prefixes in the issue are `e6feba1b`, `3600d92d`, `b358a796`, `b88855b9` (the four 840
parents) and `2ac6ec40` (841); the thread carries the full hashes. A prefix cannot be requested.

Per endpoint and block it writes:

- `raw/<endpoint>-<hash>.pb` — the node's `/api/v1/block/{hash}/raw` reply, with its SHA-256;
- the verdict of `rnode verify-block` over those bytes (`verify_ok`, `verify_output`);
- `light/<endpoint>-<hash>.json` — pre/post state, justifications and `rejectedDeploys`;
- `light/<endpoint>-finalized.json`;
- `capture.json` — every probe, its digest and its verifier result, with `complete`.

## What a completed capture must show

1. Every raw preimage re-hashes to its block hash (`rnode verify-block` exits 0).
2. The four parents' full hashes equal **841's justification set read from the verified bytes**, not
   from a `/api/block` summary.
3. The `Root.keys` reads from the two nodes are **byte-identical** at the chosen anchor(s).
4. `capture.json` says `complete: true`; any failed probe makes it false and the record stays
   INCOMPLETE.

Once those hold, the bundle is the input to M5-02 (the merge-replay surface) and M5-03 (per-chain
fates). Until then, M5's 840→841 recomputation is **unowned**, and no claim about it is made.

## Non-claims

This record does not recompute the merge, does not name a deploy fate, and does not close any of M5's
sentences. It records the absence of the input and the instrument that would capture it.
