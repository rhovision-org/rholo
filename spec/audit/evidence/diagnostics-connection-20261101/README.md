# Diagnostics × health × connection: the combined review, 2026-11-01

A review of two merged changes — **PR #537** (elastic N8 partition/rejoin check, `841cb842`) and
**PR #539** (node diagnostics contract, `1705392df`) — and of the **connection layer**, to find the
seam where diagnostics, health and node connection can compose instead of each being built blind.

## Method

Four independent reviewers, read-only, each returning findings with a named consumer, an evidence
citation and a test that would fail:

| reviewer | scope |
|---|---|
| **A — contract↔consumer** | the diagnostics standard and constants vs both drill files and the checker |
| **B — visibility inventory** | every connection event with no reading, across `comm`, `casper` and the node wiring |
| **C — decision safety** | what a connection decision could read, its consensus reach, its gaming surface |
| **D — adversarial** | self-report, Sybil churn, oscillation, amplification, and the harness's own failure modes |
| **E — referee** | dedup, rank, verify every "off-consensus" claim against the call graph, land/defer/reject |

**Referee verdict: `accept_with_corrections`.** It re-checked the four reviews, marked several
findings stale where the branch had already fixed them mid-review, and returned ten land items,
fourteen deferrals and seven rejections. Its corrections are applied below; the ones that changed a
disposition are called out. The reviewers read the tree while it was being edited, so the quotes are
evidence, not line numbers; a claim is only "landed" if this branch contains the change and a test
that fails without it.

## What the review found

### Contract ↔ consumer (A)

- **An undeclared second body shape.** `/health` answers `503` with `{ok, ready, phase, error}`, a key
  set the contract did not declare. A reader had no way to tell the exception from drift. **Landed:**
  `UNAVAILABLE_HEALTH_KEYS`, §4.3, a disjointness test and a checker rule.
- **The harness read `latestBlockNumber` as a tip.** The contract says it is one past the highest
  observed height; `maxObservedHeight` is the tip. **Landed:** `status_row` and `victim_sample` read
  `maxObservedHeight`; the drill asserts `tip == height − 1`.
- **Nine duplicate reads.** `status_row` fetched `/health` and the operator route and read nine
  fields twice. **Landed:** each value is read once, from the surface that declares it. The second
  HTTP call stays — `ready`/`readyReasons` are health-only by the operator seam test.
- **The readiness vocabulary was a hand copy.** The drill's mapping of the node's four codes to its
  own verdict names could drift. **Landed:** a test asserts the two sets are equal.
- **The consumer scan could not see subscript reads.** `status["latestBlockNumber"]` was invisible
  to the first version of the subset test. **Landed:** the scan covers `.get("…")` and `["…"]`.
- **`finalityStallEpisodes` is peer-visible; `finalityCountersDurable` is not.** A peer cannot tell
  whether the episode count is the shard's history. **Landed as prose (§6).**
- **`finalizedHeight`/`finalizedLag` were recorded but no verdict read them.** They remain evidence;
  a verdict that reads them is deferred until the age fields have been observed on a live net.
- **Claims no check can fail.** §7 said `finalizedLag` was "a difference of round heights" (it is a
  difference of block heights); §4.2 hid the `height`/`latestBlockNumber` alias. **Landed as prose.**

### Visibility (B) — the connection layer had no readings

The inventory found no `/metrics` family for `comm` at all. Landed in this branch:

- **dial attempts / failures / rounds / budget shortfall** in `find_and_connect` (B01);
- **evictions** at `MAX_CONNECTIONS`, counted on the branch that evicts (B02, C6);
- **heartbeat pings / failures** in `clear_connections` (B03);
- **routing queue depth**, sampled *before* a send so a full queue is visible (B07/D-13);
- **relay offers / failures**, replacing a discarded `Result` (B20);
- **peer-contact age**, stamped only by an authenticated, same-network inbound message (C5).

Deferred with a reason:

- **Broadcast outcomes** (B09), **stream outcomes** (B10), **LFS walk progress and unanswered-request
  ages** (B11–B13), **Kademlia ping/lookup/refusal readings** (B14–B17): each needs a return path or
  a per-peer table; they are the next visibility tranche, not this one.
- **Per-peer `lastSeen`** (B24): a per-peer map is new state on the hot path; the process-level
  `lastPeerAgeMs` buys most of the value.
- **Hostlist probe failure reasons and cycle truncation** (B18/B19): the list is client-facing and
  Law 90's freshness already covers membership; the counters are a separate, law-touching change.

### Decision safety (C) and adversarial (D)

The advisory fence held, and the review kept it:

- **Rejected — a peer's `/health` as an input to dial, evict, retrieve or relay.** Self-report with
  no in-tree consumer; the only consumer is the acceptance harness's loopback read.
- **Rejected — health, height or capacity in the hostlist derivation.** Law 90a–c: membership stays
  the four-condition predicate and the order stays ascending by node id.
- **Rejected — a self-report attracting relay fanout.** The relay remains table-order; the counters
  observe, they do not select.
- **Deferred — hostlist health annotation**, **observed-delivery selection**, **per-IP churn bounds**:
  each is a public-contract or R2 decision, not an observability one.

The adversarial pass found three defects that are now fixed:

- **`AlreadyPublished` must not refresh `lastFinalityAgeMs`.** It shares the `partition == None` arm
  with a real advance, but the fence has not moved; stamping there would reset the age on a quiet
  chain. **Fixed:** the stamp is gated on `reason.is_none()`.
- **`0` is a valid age, not "never observed".** The sentinel was `0`; a real `nano_time` reading can
  be `0`. **Fixed:** `i64::MIN` is the unset sentinel for all three stamps.
- **`finalizedLag` could underflow** when the fence is ahead of the observed tip. **Fixed:**
  `saturating_sub`, and §7 says `0` means "no gap, including at-or-ahead".

## What landed, by layer

| layer | change | falsifier |
|---|---|---|
| contract | `bootId`, `UNAVAILABLE_HEALTH_KEYS`, the three age fields | `health_contract` tests; `check-node-diagnostics.py` 28 ↔ 28 |
| `/health` | `lastBlockAgeMs`, `lastFinalityAgeMs`, `lastPeerAgeMs`, `bootId`; 503 body declared | `health_reports_readiness_and_pool_capacity` (key set), the ages-null test |
| casper | `ShardHealth` stamps: block height (new maximum only), finality advance (not `AlreadyPublished`) | `a_fresh_cell_has_no_progress_age_and_only_an_advance_stamps_finality` |
| comm | `ConnectionStats`: dials, failures, budget, evictions, heartbeats, relay, routing depth, contact age | `comm::diagnostics` tests |
| node | the 5 s publisher writes `rchain.connection.*`; the relay counts both outcomes | the metric family is the observation; type-system `silent` gate |
| harness | `maxObservedHeight` as tip, `ready`/`readyReasons`, consistency cross-checks, subset scan | 51 controls; 13 regressions |

### Where this branch deviates from the referee's land list

The referee's **E08 (broadcast and block-request attribution)** is carried as a deferral, not
silently dropped: `send_to_peers` and `broadcast_request_for_block` discard every per-peer outcome,
and giving them a return path is a signature change whose only consumer would be a counter. The
process-level dial/relay counters cover the half that has a decision behind it (a dial attempt and a
relay offer), and the per-peer attribution is in the deferral register with B09/B11–B13. **E09's
routing-queue peak** is likewise deferred: the depth gauge is sampled before an enqueue; a peak needs
a sampler, not one more atomic on the send path.

## What this review does not claim

- **No consensus reach.** Every landed signal is advisory; no block validity, attestation, finality or
  RCHIP-55 path reads it. The one consumer of a peer's `/health` is still the acceptance harness.
- **No detection completeness.** The connection counters make events countable; they do not prove a
  class is detected. A live instrumented run has not been done.
- **No Law 43 movement.** `/health` has no envelope row; the operator envelope is unchanged.
- **No Law 90 movement.** The hostlist is untouched.
- **The referee's corrections are applied, not restated.** Where it found a review stale (the 503
  declaration, the vocabulary pin, the routing-depth sample), the branch had already fixed it; where
  it found a gap (the `AlreadyPublished` regression test, the lag arithmetic, the hostlist lie test,
  the advisory source gate, the `_total` names), this branch adds it.

## Reproduce

```sh
cargo test -p rchain-comm --lib diagnostics
cargo test -p rchain-casper --lib a_fresh_cell_has_no_progress_age
cargo test -p rchain-node --lib health_
python3 -m unittest discover -s tests -p test_recovery_harness_controls.py
python3 tests/consensus-drill-regression.py
python3 tools/check-node-diagnostics.py --gate
tools/audit-type-system.sh
```

The cheapest falsifier of the connection-synergy hypothesis is now a real test, not a promise:
`node/src/web/node_list.rs::a_lying_height_or_capacity_does_not_change_the_served_set_or_order` gives
`derive_hostlist` one peer claiming `i64::MAX` height and one claiming `i64::MIN` with opposite
capacity claims, and asserts the ordered ids are identical to the honest bodies. It holds today
because no health value enters the predicate; temporarily adding `latestBlockNumber` or
`acceptsDeploys` to `valid` turns it red. The optional live confidence check is the N=8 rig: pause one
validator for 60 s and assert the connection counters/ages move within one probe interval while
`/api/v1/nodes` membership and order are identical modulo `generatedAt`.
