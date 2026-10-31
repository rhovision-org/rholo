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
   field without moving the contract fails the build.
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
7. **The public surface is a subset.** Every public field is also on the operator surface; operator-
   only state never reaches the public listener (tested twice: the key set, and a forbidden-key
   list on `PublicStatus`).

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

| field | group | type | regime | scope | freshness | invariant |
|---|---|---|---|---|---|---|
| `diagVersion` | identity | integer | G | N | at read | equals the schema version the body is written against |
| `ok` | readiness | bool | A | P | ≤1 s | equals `ready`; carried for older clients |
| `ready` | readiness | bool | A | P | ≤1 s | the conjunction of the four gates below; false always names a reason |
| `readyReasons` | readiness | string[] | A | P | ≤1 s | closed vocabulary; empty iff `ready` |
| `phase` / `nodePhase` | readiness | string | G | S | ≤1 s | the primary shard's `NodePhase` label |
| `shardPhases` | readiness | map | G | S | ≤1 s | one entry per member shard; an absent shard is not claimed running |
| `pipelineFaults` | ingestion | integer | C | P | at read | monotone since process start; evidence, never a gate |
| `pipelineFaultsOutstanding` | ingestion | integer | L | P | at read | current obligation; **zero iff no block owes a verdict**; seeded from the durable ledger at boot, unreadable ledger fails closed |
| `lastPipelineFault` | ingestion | string? | G | P | at read | the last fault's block and stage; null until one fires |
| `orphanBodies` | ingestion | integer | L | P | ≤5 s | confirmed on three consecutive audit samples; zero clears readiness |
| `orphanDetections` | ingestion | integer | C | P | ≤5 s | monotone; the history beside the current count |
| `proposalHalted` | consensus | bool | L | S | ≤1 s | set when the proposer is isolated; cleared by restart or operator resume |
| `proposalHaltReason` | consensus | string? | G | S | at read | a closed code from the proposer's isolation reason |
| `finalityStalled` | finality | bool | L | S | ≤1 s | sustained refusals, not one partial layer; the field to alarm on |
| `finalityStallEpisodes` | finality | integer | C | S | at read | durable, monotone; read with `finalityCountersDurable` |
| `height` | progress | integer | G | S | ≤1 s | the latest **round** (not finalized, not a block count) |
| `finalizedHeight` | progress | integer? | G | S | ≤1 s | the finalized fence; **null means unknown, never zero** |
| `finalizedLag` | progress | integer? | G | S | ≤1 s | `height − finalizedHeight`; a lag indicator, not a rate |
| `degraded` | resource | string | G | H | ≤5 s | advisory word from the fallback machine; never a 503 |
| `acceptsDeploys` | resource | bool | G | P | ≤1 s | pool capacity only (Law 86a) |
| `pool` | resource | object | G | P | ≤1 s | depth and bounds, so `acceptsDeploys = false` is explainable |

### 4.2 Operator additions

The operator surface carries everything public, plus `PipelineGates` (`pipelineFaults`,
`pipelineFaultsOutstanding`, `lastPipelineFault`, `orphanBodies`, `orphanDetections`,
`proposalHalted`, `proposalHaltReason`) and the operator-only state already defined by
`OperatorStatus` (divergence, halt, reset marker, sync, resources, per-shard health).

## 5. The gap register

These are diagnostics this standard calls for and the node does **not** publish yet. Each is a
counted debt, not an implied field.

| owed diagnostic | why it matters | issue evidence |
|---|---|---|
| `lastFinalityAgeMs`, `lastBlockAgeMs` | distinguishes "slow" from "stopped" without a second probe; today a reader must sample twice | #213, #315 |
| `syncPhase`, `syncBlockLegAgeMs`, `syncTrieLegAgeMs` | the trie leg has no idle bound, so a join can hang at h=0 with no field | #100, #457, #13 |
| `unrestorableRecords` acting rule | the counter is published but **nothing acts on it**; an operator can see a permanent divergence and do nothing with it | #287, #320 |
| `peerStatus` / per-peer observations | a node has no view of a peer's freshness; retrieval asks a random subset | this standard, §6 |
| `bootId` | every counter's epoch is prose ("since process start"); a restart should be readable from the body | all counters above |
| `epoch` / `livenessEpoch` | RCHIP-55's clock is not on the health route, so a jailed validator's boundary distance is invisible | #428, RCHIP-55 |

A field lands here first, with the issue that motivates it, and moves to §4 when a route test can
assert it.

## 6. Peer-visible subset

`finalityStalled` and `finalityStallEpisodes` are the **deliberate peer-liveness exception**
(W-N3-01): a peer may read whether this validator has stopped finalizing. Nothing else on `/health`
is intended for peer consumption, and no field here may be used to accept or refuse a block, to
slash, or to jail — those read the on-chain finalised fringe (Law 89, RCHIP-55 §5). A future peer
signal is self-report only and advisory; it is **not** part of this version.

## 7. Non-claims

This is a field standard, not a certification. It says nothing about detection completeness: most
silent classes in §1 still have no firing test. It does not make an alarm correct; it makes the alarm
*nameable, scoped and falsifiable*. The counters' epochs are declared in prose where `bootId` is
still owed. `finalizedLag` is a difference of round heights, not a maturity measure.

## 8. Reproduce

```sh
cargo test -p rchain-node --lib health_contract
cargo test -p rchain-node --lib health_
cargo test -p rchain-node --lib the_operator_status_carries_every_readiness_gate
tools/check-node-diagnostics.py --gate
```
