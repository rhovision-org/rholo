# Audit attestations

`rnode audit` reproduces a checkpoint from genesis and reports `AGREE`, `DISAGREE` or `INCOMPLETE`
([Sync operations](sync-operations.md)). By itself that report is a local file: anyone can copy it,
alter it, or repeat the same claim for free, and a peer cannot tell a verified replay from an
assertion. An **audit attestation** fixes that: the auditor signs the result, the claim is chained
to its previous claim, and a node accepts a claim only after reproducing it.

## The claim

An `AuditAttestation` is a signed, domain-tagged statement containing:

- `witnessType` — `signed-hash-chain-v1` (a future ZK witness would name another value);
- the shard, the checkpoint digest, the anchor and its height;
- `fromHeight = 0` and `toHeight = anchorHeight` — an audit is a reproduction from genesis, not a
  range that starts at an unverified state;
- the certificate's `metadataRoot` and `historyRoot` (required; a legacy uncommitted certificate
  cannot be attested);
- `derivedRoot` — the state root the replay **actually derived**;
- `replayHead` — a cumulative hash over the anchor's canonical ancestor closure: a domain-tagged
  genesis value seeded by the checkpoint digest, then one domain-tagged step per block, sorted by
  `(height, hash)`, covering the block hash and its post-state root;
- `previous`, `sequence`, `nonce`, signer, signature algorithm and signature.

The canonical preimage is length-prefixed. The `previous` field carries a presence byte, and an
explicit zero is not the absent previous. The signature is over
`blake2b256(lp("rchain-audit-attestation-v1") || digest)`, so a signature made for another
statement type cannot be replayed here. A golden digest pins the encoding.

## What the replay establishes first

Three inputs are established before the replay can produce a verdict, and none is allowed to look
like a disagreement:

- **The genesis post-state.** The genesis body carries its own blessed deploys, so the audit
  re-executes them and requires the derived root to equal the body's claimed post-state. An
  auditor needs no ceremony key; a body whose deploys do not derive its own state is **INCOMPLETE**.
- **Every committed header's fringe claim.** `metadataRoot` covers each ancestor's fringe
  membership and fringe state root, and a body carries neither. The audit reads **every** header in
  the anchor's ancestor closure from `GET /api/v1/block/{hash}/header`, checks the route's pre/post
  state roots against the body it served, and verifies the commitment over the hydrated set. This is
  O(ancestor closure) requests; a header-range batch is a performance follow-up.
- **The validation window.** `historyRoot` commits the last eight headers' pre/post/fringe state
  roots; the hydrated set is what the window is recomputed from. A header the audit cannot read is
  **INCOMPLETE**; a served set that reconstructs but does not match the statement is a disagreement.
  The genesis header is separately checked against the downloaded body (genesis is excluded from
  the metadata set).

## Acceptance is reproduction

A claim is not a fact because it is signed. A node accepts an attestation only after its **own**
replay of the same checkpoint agrees on `derivedRoot` and `replayHead`; the accepted store has a
private constructor reachable only from that verification. There is no public path from a wire claim
to the accepted set.

A claim that does not match is stored as **falsification evidence** — the signed claim plus the
report digest of the local replay — and is never accepted. A replayed nonce is refused, and a claim
whose `previous` does not match the signer's accepted head cannot extend the chain.

**The accepted record commits in one transaction.** The claim, the signer's new head, the spent
nonce and the answer the service returned are four rows in one store, written in one batch: a crash
leaves either the whole record or none of it, never a visible claim with an outdated head or an
unconsumed nonce. A nonce is spent once and a head only moves to a strictly greater sequence; both
read-then-write decisions are held under one in-process gate, so two concurrent acceptances cannot
both pass. A refusal is one row in a separate conflicts store and never enters the accepted set.
(One node per store is the assumption; a second process writing the same directory is outside the
gate.)

**A retry after a restart returns the same answer.** Because the response is written with the claim,
the service reads the durable row before replaying: the same nonce returns the original report and
claim even in a new process. A nonce that is spent with no stored answer — a legacy or foreign
acceptance — is a defined `409`, not a replay attempt.

## No spam

There is no gossip and no broadcast. The only trigger is an opt-in, rate-limited request:

```text
POST /api/v1/audit/request          { checkpoint, nonce, previous?, sequence? }
GET  /api/v1/audit/attestation/{id} the signed result, when ready
GET  /api/v1/audit/attestations     claims this node itself reproduced
GET  /api/v1/audit/conflicts        claims this node refused, with the local reason
POST /api/v1/checkpoint/verify      this node's verifier applied to a certificate
```

`POST /api/v1/checkpoint/verify` takes a certificate and returns the node's verdict: the recomputed
digest, the anchor, and the bonded stake whose signatures named it, with `3s > 2t` applied. It exists
for consumers that cannot verify secp256k1 themselves — the mature-join benchmark is one — so a
verdict about signatures is never a string comparison. A refusal is an error response naming the
first fact that failed.

A request is idempotent on its nonce, runs one replay at a time in a fresh scratch directory, and
never touches the live store. The service is off by default; enabling it needs a key:

| Setting | Meaning |
|---|---|
| `--audit-attest` | Serve the attested-audit routes |
| `--audit-key-path` | The signing key (hex, `0600`); falls back to the validator key |
| `--audit-bootstrap` | The URL the node audits itself through (default `http://127.0.0.1:<port-http>`) |
| `--audit-requests-per-hour` | The request budget (default 12) |

A node that does not enable the service answers `404` on these routes. A missing key is a startup
error rather than an open signing oracle. Because a consumer must reproduce a claim before accepting
it, a false claim cannot propagate: it costs the claimant its service relationship and leaves
durable, attributable evidence — the node's own `GET /api/v1/audit/conflicts` is that record for
claims it refused.

## CLI

```sh
rnode audit --bootstrap http://host:40403 --data-dir /var/tmp/audit-scratch \
  --out /var/tmp/audit-out --attest-key-path /etc/rnode/audit.key \
  --challenge <nonce> --attest-out /var/tmp/audit-out/attestation.json

rnode verify-audit-attestation --report /var/tmp/audit-b/audit.json \
  < /var/tmp/audit-a/attestation.json
```

`rnode audit` exits 0 on `AGREE` and 2 otherwise. `verify-audit-attestation` exits 0 when the claim
matches the report it is given and 2 otherwise.

## ZK, and why this is a hash chain

A zero-knowledge proof that a deploy range executed correctly would make verification cheap, but it
also means building a circuit or a zkVM for Rholang execution, choosing a proving system, and adding
a new trusted verifier to the protocol. This build has none of that stack. The signed hash-chain
attestation proves the same two facts a consumer needs now — *who* claims the replay, and *what* the
replay produced — and makes the claim reproducible. `witnessType` is part of the signed preimage so
a future proof can be added as another witness type without changing verification semantics; until
then no ZK claim is made.

## Evidence

The local-devnet run (`spec/audit/evidence/n-audit-chain-20261115-r1r5/`) shows the service
reproducing two checkpoints, each signed claim verified offline against its own local report, and the
second claim chained to the first — both entering the node's accepted store with no conflict
recorded. It is evidence for the mechanism, not an independent implementation and not a live-network
acceptance; the run has one signer and no restart control.
