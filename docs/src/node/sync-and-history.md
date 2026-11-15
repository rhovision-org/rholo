# Sync, checkpoints and history

A node that joins an existing chain does **not** re-execute it. Sync moves the node from a **trust
anchor** to the tip over authenticated data; replaying from genesis is the **audit** path (`rnode
audit`), not the sync path. This page states the model. The command-level procedure — joining,
restarting, resyncing after a refusal, auditing, and running or checking an archive — is
[Sync operations](sync-operations.md).

## The trust anchor

A node runs from exactly one of two anchors:

- the operator's **pinned genesis** — height zero, the pinned block hash and post-state, with the
  genesis state pages hydrated; a genesis-only network never asks for a certificate; or
- a **verified quorum checkpoint certificate** — the statement plus signatures reaching the finality
  rule, strictly more than two thirds of the bonded stake (`3s > 2t`) over the genesis active bonds.

There is no third anchor and no "best available root". A node that cannot name its anchor does not
run.

## The checkpoint certificate

A checkpoint is a signed statement of the finalized anchor: the anchor block, its **post-state**,
the finalized fringe, the shard, the epoch, and two optional commitments:

- **`metadataRoot`** — a set commitment over the anchor's transitive ancestor headers (genesis
  excluded, sorted by block hash, deduplicated, length-prefixed). Each header enters through a
  canonical projection that deliberately excludes `fringe`, `fringe_state_hash` and
  `body_available`, so the commitment covers what the certificate is claiming and nothing a peer
  could vary harmlessly.
- **`historyRoot`** — a commitment over the **validation window**: the pre-state, post-state and
  fringe-state roots of the most recent `VALIDATION_WINDOW_BLOCKS = 8` headers. These are the only
  historical roots a post-anchor block (the PoS close) may read.

Validators sign the statement digest with a domain tag of its own
(`rchain-checkpoint-attestation-v1`) and serve it at `/api/v1/checkpoint-attestation`. A node
assembles a certificate when the signatures reach the finality rule and serves it at
`/api/v1/checkpoint`.

Both commitments are **optional on the wire** so a legacy v1 certificate keeps its old digest. A
legacy certificate is accepted by the audit, which still checks the state by replay, but **not** by
sync: the sync path refuses a statement with no metadata or window commitment. A certificate with
the commitments present has a different digest from a legacy one; the two cannot be mixed.

## The join

For a checkpoint anchor, a joiner:

1. fetches the certificate and verifies every signature against the bond set it trusts — this build
   trusts the genesis active bonds and refuses a certificate whose anchor state's bonds have drifted;
2. asks for the fringe **anchored at the certified anchor**, so the signed statement and the served
   history are the same history;
3. walks the anchor's ancestor **headers** to genesis and checks the walked set against the
   statement's `metadataRoot`. Headers are not content-verifiable on their own (`hash_block` covers
   the deploy payloads), so the certificate's commitment, not the responding peer, is what makes the
   set acceptable;
4. hydrates the checkpoint's **post-state** — what the node runs from — and **exactly** the
   certified validation window. It never widens the window to make a block validate;
5. fetches and content-verifies the **genesis body** (the one body this path downloads), then
   publishes the header history.

The running state carries the accepted anchor and its root: `CURRENT_ROOT` is the anchor's root by
construction, not a value recovered from a DAG tip. The accepted state is the only way into the
running phase; a success signal without one is refused. A genesis anchor is accepted only when the
genesis block is the pinned one.

The header walk is bounded and retryable. The walk's cost is one round trip per generation on a
single-parent chain; the range-walk optimization batches a height window per request, and every
header it returns is still checked against `metadataRoot` and the walk shape, so the range is a
transport detail and never a trust root. A missing, malformed or mismatched input leaves the node
**Syncing**; when the retry budget is spent it stops in a terminal state rather than serving a state
it cannot attribute.

## The checkpoint store

The store separates **evidence** from **service**:

- `record` writes a digest-keyed certificate plus an anchor index and reports `Fresh`, `Again` or
  `Conflict`. Recording never moves the head a node serves.
- `adopt` moves the serving head and reports `Adopted`, `AlreadyHead`, `Conflict`, `Sibling`,
  `Stale` or `ForeignShard`. The head never moves backwards, across shards, or between two different
  statements at one anchor.

A sibling at one anchor is a conflict the operator can see; it is not silently adopted. A stored
certificate is re-verified at boot before the node runs.

## Safe fallbacks

| Situation | Result |
|---|---|
| Certificate missing, malformed, or below quorum | Refused; stay Syncing and retry |
| `metadataRoot` or `historyRoot` missing or mismatched | Refused; never widened or guessed |
| Window root cannot be hydrated | Refused before Running |
| Genesis target does not match the pin | Refused |
| Populated DAG above genesis with no verified stored certificate | **Boot refuses to run**, naming the missing trust anchor |
| Retry budget spent | Terminal stop; no serving from an unattributed state |

The supported recovery for a refused populated store is to re-sync from a checkpoint (or from the
pinned genesis on a genesis-only network) and audit the old history separately. Continuing a store
that has no anchor is exactly the state this design removes.

## History is a service

After the join, bodies are a service:

- **full / archive** nodes backfill bodies in the background (`--no-backfill-bodies` turns it off).
  Each body is checked against its accepted header by hash before it is indexed; a failure leaves
  the history floor unchanged.
- **observers that do not backfill** keep headers only and answer deploy questions from their floor.
  A lookup below the floor says "not indexed below height N", never "not found".
- **archives** advertise the heights they hold at `/api/v1/capabilities` and `/api/v1/archives` and
  answer `GET /api/v1/history/challenge?nonce=…&count=…&max_height=…` with a deterministic sample of
  bodies. The sample is derived from the caller's nonce, so an archive cannot choose the easy
  heights, and the caller verifies each body with `hash_block`. A missing body is reported per
  height. This proves possession **at challenge time**, not retention for ever.

`/api/v1/status` reports `historyAvailability` (`available` | `backfilling` | `unavailable`) and
`checkpointAge` so a client can decide whether to ask this node for history.

The archive tier's payment, metering and settlement are a service arrangement: a direct REV service
fee, no consensus change, no new token and no new slashing rule. Paying for history, and what
happens when a challenge fails, are stated in [The archive service and its
economics](archive-service.md); the funding decision of record is the direct REV service fee
(2026-11-15, owner: Patrick).

An audit's verdict can be signed and chained, so a verifier can tell a reproduced replay from a
claim: see [Audit attestations](audit-attestations.md).

## Rollout

Checkpoint certificates are produced by this build. A chain whose validators do not run it has no
certificate to serve, and joiners fail closed rather than accepting an unsigned fringe. The
checkpoint wire is **not mixed-version**: a pre-change validator computes a different digest, so its
attestations cannot combine with a current validator's, and a current joiner refuses a certificate
with no commitment. Deploy producers (validators and checkpoint collectors) before joiners, as with
any wire change. A mixed fleet is not an accepted deployment procedure.
