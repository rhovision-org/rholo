# Sync, checkpoints and history

A node that joins an existing chain does **not** re-execute it. Sync pulls the last finalized
state from the network and runs from it; replaying from genesis is an **audit** path (`rnode audit`,
run by observers as a checking service), not a sync path.

## The checkpoint certificate

A checkpoint is a signed statement: the finalized anchor block, the anchor's **post-state**, the
finalized fringe, and the epoch. Validators sign its digest with a domain tag of its own
(`rchain-checkpoint-attestation-v1`) and serve it at `/api/v1/checkpoint-attestation`; a node
assembles a **certificate** when the signatures reach the finality rule — strictly more than two
thirds of the bonded stake. `/api/v1/checkpoint` serves the certificate.

A joiner:

1. fetches the certificate and verifies every signature against the bond set it trusts (this build
   trusts the genesis active bonds and refuses a certificate whose anchor state's bonds have
   drifted from them);
2. asks for the fringe **anchored at the certified anchor**, so the signed statement and the
   served history are the same history;
3. walks the finalized DAG by **headers**, one request per frontier, until genesis;
4. hydrates exactly two state roots — the checkpoint's post-state (what it runs from) and the
   fringe's state (what its next proposal starts from);
5. fetches and content-verifies the **genesis body** (the one body this path downloads), then
   publishes the header history.

There is no unsigned fallback: a node that cannot obtain a certificate stays in sync and retries.

## History is a service, not a consensus rule

A header is not content-verifiable on its own (`hash_block` covers the deploy payloads), so the
header history is accepted under the certificate's quorum. That is a **quorum claim**, not a
verified state: a quorum that all computed the same wrong root still certifies it. The audit is the
counterweight.

After the join, bodies are a service:

- **full / archive** nodes backfill the bodies in the background (`--no-backfill-bodies` turns it
  off). Each body is checked against its accepted header by hash before it is indexed; a failure
  leaves the history floor unchanged and the node keeps serving from the checkpoint state.
- **light** nodes keep headers only and answer deploy questions from their floor (a lookup below it
  says "not indexed below height N", never "not found").
- **archives** advertise the heights they hold at `/api/v1/capabilities` and `/api/v1/archives`,
  and answer `GET /api/v1/history/challenge?nonce=…&count=…&max_height=…` with a deterministic
  sample of bodies. The sample is derived from the caller's nonce, so an archive cannot choose the
  easy heights, and the caller verifies each body with `hash_block`. A missing body is reported per
  height. This proves possession **at challenge time**, not retention for ever.

`/api/v1/status` reports `historyAvailability` (`available` | `backfilling` | `unavailable`) and
`checkpointAge` (latest block minus the served checkpoint's anchor) so a client can decide whether
to ask this node for history.

## Funding

An observer can join the history tier by staking **hardware, not tokens**, and earn a service fee;
validators can archive too. The funding rail is an external decision (a direct REV service fee is
the minimum viable one: no consensus change). No new token and no new slashing rule is introduced:
failing a challenge is fee forfeiture and reputation, not consensus punishment.

## Rollout

Checkpoint certificates are produced by this build. A chain whose validators do not run it has no
certificate to serve, and joiners fail closed rather than accepting an unsigned fringe — deploy
producers before joiners, as with any wire change.
