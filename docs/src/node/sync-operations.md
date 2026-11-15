# Sync operations

How to join, restart, resync, audit, and run or check an archive. The model is stated in
[Sync, checkpoints and history](sync-and-history.md); this page is the procedure.

## Before you start

- **Same genesis inputs.** Every node on a chain is given the same bonds, wallets, multi-signature
  list and quorum, bond bounds, epoch and quarantine lengths, and participation/executor shares. A
  node that omits one computes a different genesis and refuses every block above it.
- **Inbound from every peer**, not only the bootstrap: 40400 and 40404. A node that admits only the
  bootstrap receives traffic and applies none of it.
- **The same build.** The checkpoint wire is not mixed-version. Validators and checkpoint collectors
  run the current build before joiners do.

## Join from a checkpoint

A joiner needs a bootstrap that serves a certificate:

```sh
rnode --profile docker run --no-upnp --host <public-ip> \
  --data-dir /var/lib/rnode \
  --bonds-file /var/lib/rnode/genesis/bonds.txt \
  --wallets-file /var/lib/rnode/genesis/wallets.txt \
  --pos-multi-sig-public-keys <same list> --pos-multi-sig-quorum <same> \
  --bootstrap rnode://<node-id>@<host>?protocol=40400&discovery=40404
```

Add `--validator-private-key-path` and `--propose-on-deploy` only when the node is a validator; an
observer joins with the same command minus the key.

What happens:

1. the node fetches `/api/v1/checkpoint` from the bootstrap and verifies the certificate against
   the genesis bonds;
2. it asks for the fringe anchored at the certified anchor and walks the ancestor headers to
   genesis, checking the set against `metadataRoot`;
3. it hydrates the checkpoint post-state and exactly the certified validation window, fetches the
   genesis body, and publishes the header history;
4. it enters Running from the anchor's root. `CURRENT_ROOT` is that root by construction.

Success is the node serving with `/api/v1/status` reporting a checkpoint anchor
(`checkpoint.kind = "checkpoint"`, its digest, anchor and height), `syncPhase` no longer Syncing,
and `/api/last-finalized-block` advancing. The header walk's failure log names the missing headers
and the peers they were requested from.

Do **not** expect a replay log. Sync does not replay; replay is `rnode audit`.

## Join a genesis-only network

A chain with no finalized fringe has no certificate to serve. The joiner anchors on the **pinned
genesis** — the genesis block must be the one the operator pinned; its state pages are hydrated and
the runtime must know the root. No certificate fetch is attempted, so there is no retry loop to
wait out. If the genesis does not match the pin, the node refuses.

## Restart

A restart re-opens the store and **re-verifies the stored certificate** before Running. A store
that holds a verified certificate boots and re-verifies it against the accepted fringe; a
genesis-only DAG anchors on its pinned genesis.

A populated DAG above genesis with **no verified stored certificate** is refused: the node stops and
names the missing trust anchor. That is the intended safe fallback, not a corrupt store. A pre-change
populated store has no certificate, so it refuses under the current binary.

## Resync after a refusal

The old history is not continued. The supported migration is:

1. keep the old data directory (or a copy) for the audit;
2. start a **fresh** data directory and join from a checkpoint (or from the pinned genesis on a
   genesis-only network), as above;
3. audit the old history separately with `rnode audit` against a node that still serves it.

Record the new store's identity, the checkpoint digest and anchor it accepted, and the audit
verdict. Do not edit store markers to make an unattested DAG look accepted.

## Run an audit

The audit replays the bodies from genesis in a scratch runtime and compares the state it derives
with the checkpoint's claim. It never writes to a live store.

```sh
rnode audit \
  --bootstrap http://127.0.0.1:40403 \
  --data-dir /var/tmp/audit-scratch \
  --out /var/tmp/audit-out \
  [--checkpoint <checkpoint-digest>]
```

- `--checkpoint` audits a specific certificate by digest; omit it to audit whatever certificate the
  bootstrap serves.
- `--data-dir` must be scratch, not a live node's data directory.
- `--out` receives `audit.json`.

Verdicts:

| Verdict | Meaning | Exit |
|---|---|---|
| `AGREE` | The replay derived the claimed root; the metadata and window commitments (when present) matched the bodies. | 0 |
| `DISAGREE` | The replay ran and derived something else, or a commitment did not match. This is an alarm on an accepted state; it never un-accepts it. | 2 |
| `INCOMPLETE` | The history could not be obtained. Missing evidence is **not** agreement. | 2 |

`metadataCommitted` and `windowCommitted` report whether the audited certificate carried the
commitments. A legacy certificate audits with `windowCommitted: false`; it is still checked by
replay, but the sync path would refuse it.

A verdict can be signed and chained so another node can verify that a replay was reproduced:
[Audit attestations](audit-attestations.md).

## Verify an archive

An archive advertises its floor at `/api/v1/capabilities` and `/api/v1/archives`. Challenge it with
a nonce you choose:

```sh
curl 'http://<archive>:40403/api/v1/history/challenge?nonce=<your-nonce>&count=64&max_height=<height>'
```

The sample is deterministic from the nonce, so the archive cannot pick the easy heights. For every
entry, reconstruct the body and check that it hashes to the reported block hash and has the
reported height; a missing body is a per-height gap. A challenge proves possession **at challenge
time**. Use the live checker (`node/tests/history_challenge.rs`, gated by
`RNODE_CHALLENGE_URL`) for an automated pass.

## Run an archive

```sh
--archive                 advertise and serve the archive tier
--history-retention N     retain N heights (0 = all)
--no-backfill-bodies      keep headers only below the floor
```

An archive keeps bodies, advertises what it holds, and answers challenges and history ranges. A
validator can archive too. Bodies below the floor are served through the history service, which is
metered and paid; headers and the checkpoint metadata are not metered, so joining stays cheap. The
funding rail is the direct REV service fee (decision of record, 2026-11-15, owner: Patrick); there
is no consensus change, no new token and no new slashing rule. A failed challenge is fee forfeiture
and selection, not consensus punishment.

## Troubleshooting

| Symptom | Cause to check first |
|---|---|
| Stuck Syncing, no certificate | The bootstrap serves no certificate; validators are not on the current build; or the certificate is below quorum |
| Refused: metadata commitment | The walked header set does not match `metadataRoot`; try another peer, then treat it as a refusal |
| Refused: validation window | The committed window could not be hydrated; do not widen it |
| Boot refuses a populated store | No verified certificate; resync from a checkpoint and audit the old history |
| Audit `INCOMPLETE` | The bootstrap cannot serve the bodies; this is not agreement |
| Audit `DISAGREE` | Replay derived another root or a commitment mismatched; preserve the report before acting |

## Rollout order

1. validators and checkpoint collectors: current build, then let certificates form;
2. joiners: current build, joining from a checkpoint;
3. archives: current build, metering and challenge enabled;
4. audit services: current build, on keys the operator controls.

Rolling back to a pre-change binary on a chain that produces commitments is not supported: its
digest differs and it cannot combine attestations with the current set. Roll the whole fleet, not a
mixed one.
