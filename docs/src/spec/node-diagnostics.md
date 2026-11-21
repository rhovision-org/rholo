# Node diagnostics

**Status:** the standard, versioned 2026-10-31 (`GET /health` v2). The machine side is
`node/src/diagnostics/health_contract.rs`; the route is `node/src/web/http.rs::api_health`. This page
is normative for the field set, the regime vocabulary and the invariants. It is also a gap register:
§5 names the diagnostics the standard calls for and the node does **not** yet publish.

## 1. Why this exists

The node's failure record is full of **silent** failures: a joining node that never formed (#100,
#457), an 80%-stake wedge where "nothing is refused… production simply stops" (#213),
`quorum_reachable=false` while a supermajority runs (#315), a listener that never bound (#86), memory
retention found by sampling RSS (#18/#23/#117/#85), a block refused with no re-attempt (#103/#223/
#522/#528). Most of them were found by a human reading logs or a test failing, not by a field.

A diagnostic is not a log line. It is a **named, typed, versioned reading with a declared regime, a
declared scope and a stated invariant** — so a probe can ask the question, and a reader can tell a
current view from a history.

## 2. The rules

1. **One shape per surface.** `GET /health` has exactly the key set in
   `PUBLIC_HEALTH_KEYS`; a route test compares the served JSON against it. Adding or renaming a
   field without moving the contract fails the build. The one exception is the `503` body (§4.3),
   declared in `UNAVAILABLE_HEALTH_KEYS` — the snapshot could not be assembled, so it is a different
   shape by necessity rather than by drift.
2. **Regimes are declared.** A *gauge* is the current value; a *counter* is monotone since its
   declared epoch; an *alarm* is a derived predicate with hysteresis; a *latch* is an alarm state
   that withholds work. Every latch has an inverse (the transition that clears it), a minimum dwell,
   and an episode counter.
3. **Stale is not healthy.** A reading whose sample is older than its freshness bound is `unknown`,
   a third state. It is never folded into `normal`. (Where a field cannot yet distinguish these, §5
   says so.)
4. **Scope is explicit.** `shard`, `process`, `node` or `host`. No process field is derived from one
   shard; per-shard facts are maps keyed by shard id.
5. **Alarm reasons are closed.** A boolean is a verdict; the reason is a machine-readable code from
   a fixed vocabulary (`ready_reasons`). Free text is a diagnosis beside the alarm, never the alarm.
6. **Names carry units and lifetime.** `*_total` is monotone since process start; `*_episodes` is
   durable and monotone; `*_current` is a live view; `*_age_ms`/`*_at_ms` carry time.
7. **`PublicStatus` is a subset; `/health` is a separate view.** Every field of
   `GET /api/v1/status` is flattened into `GET /api/v1/status/operator`, and operator-only state
   never reaches the public listener (tested twice: the key set, and a forbidden-key list on
   `PublicStatus`). `GET /health` is a *readiness-shaped diagnostic view*, **not** a subset of the
   operator envelope: the seven process gates (§4.2) appear on both, while `diagVersion`, `ready`,
   `readyReasons`, `finalizedHeight`, `finalizedHeightError`, `finalizedLag` and the phase map are
   health-only. The seam is stated rather than implied — the earlier draft of this page claimed a
   superset that the implementation does not have.

## 3. The surface

| surface | listener | shape | purpose |
|---|---|---|---|
| `GET /health` | public | the diagnostic snapshot, v2 | can this node include work now, and if not, why; plus the finality fence and the stall alarm a peer may read |
| `GET /api/v1/status` | public | identity, mode, pool, `degraded` | the lightweight answer a wallet or a hostlist probe reads |
| `GET /api/v1/status/operator` | admin | `PublicStatus` + operator state + the readiness gates | the complete view: divergence, halt, reset, sync, resources, and the gates that withheld `ready` |
| `GET /metrics` | configurable | Prometheus counters/gauges | trends and scrapes |

`/health` v2 is **additive**: every v1 key keeps its name and type.

## 4. The field standard

Regime: G gauge · C counter · A alarm · L latch. Scope: S shard · P process · N node · H host.
Freshness is the maximum age at which the reading is still `known`.

### 4.1 Public: `GET /health`

**Every `S`-scoped row is the primary shard's reading.** The only per-shard map is `shardPhases`;
the scalar progress, finality and gate fields are the primary's, the same flattening the operator
surface uses. On a multi-shard node a scalar row is not the node's, and a consumer that needs one
shard's reading must say which.

| field | group | type | regime | scope | freshness | invariant |
|---|---|---|---|---|---|---|
| `diagVersion` | identity | integer | G | N | at read | equals the schema version the body is written against |
| `bootId` | identity | string | G | P | at read | `<pid>-<start-millis>`; stable within a process, different after a restart; correlation only, not authentication |
| `ok` | readiness | bool | A | P | ≤1 s | equals `ready`; carried for older clients |
| `ready` | readiness | bool | A | P | ≤1 s | the conjunction of the four gates below; false always names a reason |
| `readyReasons` | readiness | string[] | A | P | ≤1 s | closed vocabulary; empty iff `ready` |
| `phase` / `nodePhase` | readiness | string | G | S | ≤1 s | the primary shard's **effective** `NodePhase` label: `blocked` while a durable pipeline fault is outstanding, even if the launch cell says `running`; `catching-up` while the node's own validated tip is outside the catch-up window of the peer tip it has observed (Laws 121/123) |
| `catchUp` | readiness | object? | G | S | ≤1 s | `{supported, own, peerTip}` while the primary shard is `catching-up`: `own` is the highest latest message this node has validated, `peerTip` is the frontier a connected peer reported through a bounded `BlockRange` probe (`-1` until one answers; a joined boot starts catching up before its first probe), and `supported` is the quorum-supported height over the node's own messages (a diagnostic — a healthy leader's quorum height can sit at a laggard's height). `null` otherwise; a catching-up shard is never `ready` |
| `shardPhases` | readiness | map | G | S | ≤1 s | one entry per member shard, each its effective phase; an absent shard is not claimed running |
| `pipelineFaults` | ingestion | integer | C | P | at read | monotone since process start; evidence, never a gate |
| `pipelineFaultsOutstanding` | ingestion | integer | L | P | at read | current obligation; **zero iff no block owes a verdict**; seeded from the durable ledger at boot, unreadable ledger fails closed |
| `lastPipelineFault` | ingestion | string? | G | P | at read | the last fault's block and stage; null until one fires |
| `deferredBlocks` | ingestion | integer | L | P | at read | blocks held because a justification is not yet validated — an obligation to fetch the missing ancestry, not a fault or a verdict; the disposition sweep re-offers them and a successful insert clears the entry (Law 116, RC-4 item C). Not a readiness term |
| `lastDeferralReason` | ingestion | string? | G | P | at read | the last deferred block's hash and height and its first missing justification; null until one is deferred |
| `orphanBodies` | ingestion | integer | L | P | ≤5 s | confirmed on three consecutive audit samples; zero clears readiness |
| `orphanDetections` | ingestion | integer | C | P | ≤5 s | monotone; the history beside the current count |
| `proposalHalted` | consensus | bool | L | S | ≤1 s | set when the proposer is isolated; cleared by restart or operator resume |
| `proposalHaltReason` | consensus | string? | G | S | at read | a closed code from the proposer's isolation reason |
| `selfValidationRefusal` | consensus | object? | G | S | at read | the proposer's current **counted** self-validation refusal: `{code, blockNumber, seqNum, atUnixMs}` or null. `code` is a closed kebab-case vocabulary from `BlockStatus::code()`, `internal-error`, or a replay code (`replay-cost-mismatch`, `replay-status-mismatch`, `unused-comm-event`, `system-deploy-error-mismatch`); cleared by the successful propose that clears `consecutiveSelfValidationFailures` and by `POST /api/v1/autopropose/resume`; a divergence refusal and a stale-snapshot self-equivocation are **not** recorded. **The public summary deliberately omits the replay detail** (cost pair, deploy ids, attempts, exclusion) — that is operator-only, because the route is unauthenticated. Not derived from logs; not a readiness term |
| `finalityStalled` | finality | bool | L | S | ≤1 s | sustained refusals, not one partial layer; the field to alarm on |
| `finalityStallEpisodes` | finality | integer | C | S | at read | durable, monotone; the durability qualifier `finalityCountersDurable` is **operator-only**, so a peer reading this field cannot verify it (§5) |
| `lastBlockAgeMs` | progress | integer? | G | S | at read | milliseconds since a **new highest** height was accepted; **null until the first observation, never 0**; a same-height sibling does not reset it |
| `lastFinalityAgeMs` | progress | integer? | G | S | at read | milliseconds since finality last advanced; null until the first advance, never 0 |
| `lastPeerAgeMs` | progress | integer? | G | P | at read | milliseconds since an **authenticated, same-network** peer message; a connection-table row is not contact; null until one |
| `height` | progress | integer | G | S | ≤1 s | **one past the highest observed height** (the DAG's `latestBlockNumber` convention); `0` on an empty DAG. Not the tip height — use `maxObservedHeight` to compare with a block number |
| `maxObservedHeight` | progress | integer? | G | S | ≤1 s | the observed tip height, i.e. `height − 1` when `height > 0`; **null on an empty DAG, never `−1`** |
| `finalizedHeight` | progress | integer? | G | S | ≤1 s | the finalized fence (a block's own number); **null means unknown, never zero**; read `finalizedHeightError` for why |
| `finalizedHeightError` | progress | string? | G | S | ≤1 s | the read error when the fence is unknown; null when the read answered. An erased error is the `silent` class the type-system gate forbids |
| `finalizedLag` | progress | integer? | G | S | ≤1 s | **`maxObservedHeight − finalizedHeight`** (equal units: both are block heights), so a fully finalised tip reads `0`; null when either side is unknown |

These finality rows are **node-local**: a peer's sample is not quorum, and `ready` deliberately does
not include finality. The client contract is [Finality is not admission](../node/admission.md#finality-is-not-admission).
| `degraded` | resource | string | G | H | ≤5 s | advisory word from the fallback machine; never a 503 |
| `acceptsDeploys` | resource | bool | G | P | ≤1 s | pool capacity only (Law 86a) |
| `pool` | resource | object | G | P | ≤1 s | depth and bounds, so `acceptsDeploys = false` is explainable |
| `blockPipeline` | ingestion | object | G | S | ≤5 s | `{capacity, pending, sampledPeak, validatedQueue, incomingQueue}`: the **aggregate** ingest backlog. `pending`/`sampledPeak` sample the block-receiver `blocks_st` queue (the one the 1024 cap is on); `validatedQueue`/`incomingQueue` sample the two mpsc stages. Advisory: no readiness term |

### 4.2 Operator additions and the seam

The operator surface carries **every `PublicStatus` field** (flattened), plus `PipelineGates`
(`pipelineFaults`, `pipelineFaultsOutstanding`, `lastPipelineFault`, `orphanBodies`,
`orphanDetections`, `proposalHalted`, `proposalHaltReason`) and the operator-only state already
defined by `OperatorStatus` (divergence, halt, reset marker, sync, resources, per-shard health).

It is **not** a superset of `/health`. The two surfaces overlap in three groups, and nothing else:

1. **`PublicStatus`, flattened** — `version`, `address`, `networkId`, `shardId`, `peers`, `nodes`,
   `minPhloPrice`, `latestBlockNumber`, `autopropose`, `proposeOnDeploy`, `manualPropose`, `devMode`,
   `proposer`, `degraded`, `acceptsDeploys`, `pool`;
2. **the seven process gates** (`OPERATOR_GATE_KEYS`);
3. **`finalityStalled` and `finalityStallEpisodes`**, the peer-liveness projection as well as the
   operator alarm (a latch and a counter, not two latches).

`selfValidationRefusal` is also on both: flat on the operator surface (the primary's) and inside
`shards` (every member's), beside the operator-only `consecutiveSelfValidationFailures` it explains
(#485).

**The exact key set** is `OPERATOR_STATUS_KEYS` in
`node/src/diagnostics/health_contract.rs`; the route's test serializes the fixture and compares the
sorted keys, so a field renamed on either half of the flattened DTO fails the build. The C343-LIVE
snapshot reads five fields beyond the seven gates: `syncPhase` (this shard's sync state), `sync`
(the LFS sync task's own state), `resetMarker` (the durable reset), `finalityStalled`, and `shards`
(every member's health). `OPERATOR_GATE_KEYS` is a subset of the set.

**The operator object is the detailed one** (#564). On `/api/v1/status/operator` — and only there —
the refusal object also carries `initialCost` and `replayCost` (the `ReplayCostMismatch` pair, null
for every other code), `deployIds` (base16, capped at 16; `deployCount` is the true count),
`attempts` (the run length for this exact pre-state, deploy set and code), `excluded` (whether the set
is held out of the proposer's selection at the refusing pre-state), `stateHashMatched` (the shadow
replay's verdict on the declared post-state, null when it did not reach the checkpoint),
`replayedStateHash`, and the durable record's view: `refusedBodyAvailable`/`refusedBodyLen` (the
refused block body is capped; the length is the true one, so an over-cap body is visible rather than
implied), `resetAttempted` (the fault's finalised anchor has had its one reset) and `resetAnchor`.
`poolQuarantined` beside it is the number of pooled deploy ids currently held out. **These are
proposer-local operating facts, not consensus state**: the deploys stay in the pool, another
validator may include them, and they expire with the pool at height + 50.

**The recovery fields, and why the refusal can stand while the node is healthy** (#564). A self-replay
refusal is followed by an attestation-only block, so a successful propose does **not** by itself clear
the refusal record (it stays as evidence until the suspect set is resolved); a normal success still
clears it with the counter. The operator snapshot adds `selfReplayFaultHalted` and
`selfReplayFaultReason`: set only when the attestation fallback also failed, the node-level rung. The
latch is read by the autopropose timer, the attest-on-new-blocks tap and every `propose`, so it stops
production even with `--no-autopropose`. **The reset's completed catch-up clears it automatically**:
once the walk passes the finalised anchor the record moves to `Recovered` store-first and the parked
timer is woken with the same notify the operator route uses; `POST /api/v1/autopropose/resume`
remains the fallback (reset disabled, no finalised anchor, or the anchor already had its one reset),
and it clears the timer halt with the record, store-first. The refused block body (capped), the
verdict and the suspect ids are in the durable `SelfReplayRecord`; a `ResetRequested`/`Halted` stage
re-seeds the latch at boot, and the evidence fields above are seeded from the record whether or not
it latches. The public `/health` object keeps the four-field summary above; deploy ids, costs, the
refused body, the reset attempt and the halt sentence are not published on an unauthenticated
listener.

The health-only fields are `diagVersion`, `bootId`, `ok`, `ready`, `readyReasons`, `phase`,
`nodePhase`, `shardPhases`, `height`, `maxObservedHeight`, `finalizedHeight`, `finalizedHeightError`,
`finalizedLag`, `lastBlockAgeMs`, `lastFinalityAgeMs` and `lastPeerAgeMs`. (Corrected twice on
2026-11-01: the first draft claimed a superset; the second listed the two finality fields as
health-only and understated the `PublicStatus` overlap.)

**The `height` alias.** `health.height` and `PublicStatus.latestBlockNumber` are the *same* one-past
convention under two names, and `maxObservedHeight = height − 1` is the tip. A consumer that compares
`latestBlockNumber` with a block number is off by one — this is the seam that made a finalised chain
report a lag of one before 2026-11-01.

### 4.3 The unavailable body — `503`

When the node cannot read its own status, `/health` cannot assemble the snapshot and answers
`503` with exactly `{ok: false, ready: false, phase: "unavailable", error}` — the key set in
`UNAVAILABLE_HEALTH_KEYS`. It **shares `ok`, `ready` and `phase` with the snapshot and adds
`error`**; every other snapshot field is absent because it could not be produced. `error` is the
read error; no age, fence or phase map is claimed. A client must treat this as **unknown**, not as a
not-ready node: the two readings differ, and only one of them is the node's verdict.

### 4.4 Public: the anchor, phase and audit reading

`GET /api/v1/status` carries three fields beyond the raw counts:

- `syncPhase` — `starting`, `syncing`, `running` or `blocked` for this shard. It answers "is this
  node serving a chain", which `latestBlockNumber` cannot: a node can hold blocks and serve none.
- `checkpoint` — the trust anchor. `kind` is `checkpoint` (with the certificate `digest`) or
  `genesis` (digest `null`); it carries `anchor`, `anchorHeight`, `ageBlocks`,
  `validationWindowHeights` (14), and whether the statement committed to the validation window
  (`historyRoot`) and the ancestor metadata (`metadataRoot`). `null` means the node has **no
  anchor**, and a node with no anchor is not running (Law 108).
- `audit` — whether this node serves audit attestations (`available`), whether a replay is
  `running`, and the last `verdict`/`checkpoint`/`runAtUnixMs`.

All three fields are `#[serde(default)]`: a pre-change transport decodes them as `null`, and a
reader must not read that as "no anchor". They are public because a client choosing where to ask for
history, or whether to ask this node for an audit, needs them; no operator-only data is added.

Regime: sampled at request time. Scope: the phase and anchor are this shard's; the audit tier is
process-wide. Freshness: immediate; `ageBlocks` is a height difference, not a wall clock.

## 5. The gap register

These are diagnostics this standard calls for and the node does **not** publish yet. Each is a
counted debt, not an implied field.

| owed diagnostic | why it matters | issue evidence |
|---|---|---|
| `syncBlockLegAgeMs`, `syncTrieLegAgeMs` | the trie leg has no idle bound, so a join can hang at h=0 with no per-leg age. (`syncPhase` is now published on `/api/v1/status` — §4.4.) | #100, #457, #13 |
| `unrestorableRecords` acting rule | the counter is published but **nothing in the node acts on it**; an operator can see a permanent divergence and do nothing with it. (An external consumer does act: the N8 partition/rejoin harness fails with `FAIL_PERMANENT_DIVERGENCE` when it is non-zero — `tests/consensus-partition-rejoin.py`.) | #287, #320 |
| `peerStatus` / per-peer observations | **the surface exists — this is a consumer gap.** `/health` is already the peer-visible projection (§6), and the only in-tree reader of a peer's `/health` is the acceptance harness's container-loopback read. No connection decision reads it, and no node polls a peer's health. What is missing is a consumer and a reason to trust it, not another endpoint. | this standard, §6 |
| `epoch` / `livenessEpoch` | RCHIP-55's clock is not on the health route, so a jailed validator's boundary distance is invisible | #428, RCHIP-55 |
| `finalityCountersDurable` on the peer view | `finalityStallEpisodes` is peer-visible but its durability qualifier is operator-only; a peer cannot tell the shard's history from this process's running total | this standard, §6 |

A field lands here first, with the issue that motivates it, and moves to §4 when a route test can
assert it.

## 6. Peer-visible subset

`finalityStalled` and `finalityStallEpisodes` are the **deliberate peer-liveness exception**
(W-N3-01): a peer may read whether this validator has stopped finalizing. Nothing else on `/health`
is intended for peer consumption, and no field here may be used to accept or refuse a block, to
slash, or to jail — those read the on-chain finalised fringe (Law 89, RCHIP-55 §5). A future peer
signal is self-report only and advisory; it is **not** part of this version.

**Two caveats a peer reader must carry.** `finalityCountersDurable` is operator-only, so a peer
reading `finalityStallEpisodes` cannot tell whether the count is the shard's history or this
process's running total — it is a liveness hint, not an assertion about the shard's record. And
`lastPeerAgeMs` is process-scoped and refreshed by **any** authenticated same-network peer,
including one that is not a validator: it says "something is talking to me", not "a validator is
healthy". Neither caveat is a licence to promote the field into a decision.

`selfValidationRefusal` is published for probes and operators, not as a peer-liveness signal: a
peer may not use it to accept or refuse a block, to slash, or to jail. It names a local proposal
failure, not a verdict about the sender.

## 7. Non-claims

This is a field standard, not a certification. It says nothing about detection completeness: most
silent classes in §1 still have no firing test. It does not make an alarm correct; it makes the alarm
*nameable, scoped and falsifiable*. `bootId` is a correlation id, not an authentication token.
`finalizedLag` is a difference of **block heights** (`maxObservedHeight − finalizedHeight`, both
block numbers), not a maturity measure and not a rate; it reads `0` when the fence is at or ahead of
the observed tip. `selfValidationRefusal` is **process-local with a durable floor**: it is cleared by
a successful propose or an operator resume, and after a restart the `attempts` and `excluded` run
counters start again, so those two name this process's run, not the shard's. But the fact of the
refusal — the code, the block, the cost pair, the ids and, for a self-replay fault, the refused body's
availability and the reset attempt — is seeded from the durable `SelfReplayRecord` when one exists.
It is not a root-cause diagnosis. `poolQuarantined` and the selection exclusion are process-local:
the exclusion is re-derived from the tracker after a restart, while the record keeps the evidence.
The deploy-side contract is the other half:
`acceptsDeploys` is local pool capacity, never a finality signal, and the four deploy states are in
[Finality is not admission](../node/admission.md#finality-is-not-admission).

## 8. Reproduce

```sh
cargo test -p rchain-node --lib health_contract
cargo test -p rchain-node --lib health_
cargo test -p rchain-node --lib self_validation
cargo test -p rchain-casper --lib self_validation
cargo test -p rchain-node --lib the_operator_status_carries_every_readiness_gate
cargo test -p rchain-casper --test reset
tools/check-node-diagnostics.py --gate
```

The recovery fields are also exercised live, on a fresh three-validator devnet:

```sh
tools/self-replay-drill.sh
```
