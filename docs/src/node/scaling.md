# Scaling and performance limits

This page describes the **shape** of this node's limits — which ones are constants in the protocol,
which are policy choices that could be changed, and which are structural properties of the design. It
is written from measurements recorded in the tree, not from a production benchmark: **no in-repo
production benchmark exists**, and the numbers a devnet produces on a laptop or a pair of 1 GB hosts
are not a capacity plan.

For *sizing a machine* (storage map sizes, RAM per block, start-up cost) see
[Running a validator: hardware requirements](validator-requirements.md). That page answers "how big a
box"; this one answers "how fast, and what stops it".

Every figure below is tagged. **Measured** means it was read off a running node and the artifact is
committed under `spec/audit/evidence/`. **Derived** means it is arithmetic on a constant in the code.
**Estimate** means it is an extrapolation and is labelled as one.

## The throughput ceiling — and where it comes from

A block carries at most **255 deploys**. That is not a considered protocol bound; it is the width of
the per-deploy randomness-seed index, which is a `u8` — `MAX_BLOCK_DEPLOYS` in
`casper/src/blocks/proposer/proposer.rs`, and the comment on the constant says so. Slashes share the
same budget, so the proposer's allowance is `per_block_deploy_budget(slashes) = 255 - slashes`, and the
receiving side enforces the same bound in `deploy_count` (`casper/src/validate.rs`). The matching cost
cap is derived from the same number: `MAX_BLOCK_PHLO = 25_500_000_000`, i.e. 255 × the 1e8 default
per-deploy phlo limit.

So the chain's deploy ceiling is exactly:

> **255 deploys per block × the block rate**

and the block rate is what the rest of this page is about.

| Cadence | Rate as measured | Where |
|---|---|---|
| `--autopropose` timer (default interval 2 s, `AUTOPROPOSE_INTERVAL` in `node/src/runtime/node_runtime.rs`) | 0.5 blocks/s (**derived** — the timer's period) | — |
| all-live three-validator devnet | **12–16 heights/min** (~1 height per 4–5 s) — *a height rate, not blocks* | `spec/audit/evidence/n127-campaign-results.md` |
| the fixed proposer | **~38 heights/min** — *derived; provenance in the pass record* | `spec/audit/evidence/n127-proposer-fix-results.md` |
| attestation storm (a defect, not a capability) | ~4 blocks/s, of mostly *empty* blocks | [Incident history](incident-history.md#the-attestation-storm) |

**Read the column as a height rate, not a block rate.** A block's number is derived from its
justification set — `casper/src/validate.rs:232-253` requires `max(justification height) + 1` — so
every validator in a round derives the same height and **several blocks share a height**. The two
measured rows record `latestBlockNumber` only, so they understate blocks by a factor the artifact
cannot recover; the ~38 row is the same substitution on another table (height at T+120 s ÷ 120 × 60).
The derived band of 60–160 deploys/s inherits both errors and is an order of magnitude only; the 2 s
timer row is the only constant. The fixed-proposer number is 2.5–3× the pre-fix one, so treat it as a
direction rather than a current capability.

The default node config is neither continuous nor on-deploy: `autopropose = false` and
`propose-on-deploy = false`, with `attest-on-new-blocks = true` supplying the liveness trigger. The
public testnet runs deliberately idle — blocks appear when a deploy arrives.

## Finality is a fringe estimate, not a block depth

There is no fixed "final after N blocks" rule. Finality is CBC-Casper **fringe** finality: a block is
finalized when it enters the seen-set of a fringe that more than **two-thirds of bonded stake** has
attested to — `is_super_majority` (`sdk/src/consensus.rs`) is the strict `3 × stake > 2 × total`, so
exactly 2/3 is *not* sufficient — applied by the finalizer in `block-storage/src/dag/finalizer.rs`.
Which validators must be seen is the live weight set, within `LIVENESS_WINDOW = 5` heights of the tip
(`block-storage/src/dag/liveness.rs`), while the quorum *denominator* stays the whole bonded map.

What matters for latency is the **gap** between height and finalized height, and it is measured:

| Condition | Gap (blocks) | Where |
|---|---|---|
| fixed proposer (`de4e9af02`) | **3–4** | `spec/audit/evidence/n127-proposer-fix-results.md` |
| live testnet, three validators, all attesting | 7 | `docs/src/node/testnet.md` |
| pre-fix, under load | 15–61 | `spec/audit/evidence/n127-proposer-fix-results.md` |

Finality latency is therefore **gap × block time**: roughly **6–20 s** across the measured band
(derived). On an idle, deploy-driven chain with `--propose-on-deploy`, a deploy waits about one block
for inclusion and a few more for finality.

## The merge, and why per-block cost is not constant

Every proposal and every validation runs a **merge** over the parent justifications — `MergeScope::merge`
in `casper/src/merging.rs`, with the scope built by `from_dag`/`from_fringes`. The merge's conflict
resolution is a bounded search, `SearchBudget::NODE`, and the bound is **node-local policy, not the
oracle's**.

The cost of that search is measured, and it is the reason block time is not a constant:

| Metric | Measured |
|---|---|
| median merge scope (9 node-runs) | **32 chains** (`spec/audit/evidence/n127-campaign-results.md`) |
| widest scope | 37–43 chains |
| states expanded, widest single merge | **389,977 → 2,026,511** (`n127-campaign-results.md`) |
| growth | roughly **2× per additional chain** (`n117-after-fix-results.md`) |

That doubling is the thing to watch: at 33 chains the census read 411,199 states, at 35 it read
1,663,395. A 40-chain fork was measured at `2^21` states taking **34.6 s** in the pre-rewrite build.

There is a fix in the tree for the *symmetric* case — enumerating maximal independent sets instead of
subsets turns 40 chains from `2^21` states / 34.6 s into **60 states / 0.7 ms**
(`spec/audit/evidence/n117-heap-profile-results.md`). **It declines on the real node**, because
`resolve_conflict_set` unions each key's dependencies into its conflict set and the resulting relation
is *directed*, not symmetric — every one of nine node-runs on the frozen reproduction showed
asymmetric pairs. So the node runs on the dedup alone and is one wider fork from the same ceiling.
This is recorded as a known merge-scaling residue in [`spec/AUDIT.md`](../../../spec/AUDIT.md), and it is the single largest
determinant of whether block time stays flat as the DAG widens.

## Residency is the structural limit

The DAG retains, per message, the set of messages it has seen, constructed as the union of its
justifications' seen sets plus its own id. Because a block's seen-set is its whole ancestry, total
residency is **N(N+1)/2** in the number of blocks ever accepted — and the message map is never pruned.

Measured on a 5,885-block chain: `seen_entries` **17,319,555** (exactly N(N+1)/2), `logical_bytes`
**556 MB**, inside a **1.18 GiB** process, advancing about **1.3 MB per block** at that height
(`spec/audit/passes.md`).

The consequence is worth stating plainly, because it is the limit that decides whether a validator runs
for a day or for a year: at a two-second block interval, **16 GB of resident memory is roughly twelve
hours away** ([Security audit](security-audit.md)). At the faster cadences in the table above it is
sooner.

**This is a decision, not an oversight.** Bounding it means designing pruning of finalized ancestry
into the DAG and its liveness rules — a change to the rules the finalizer depends on, not a patch — and
the recorded decision is to **accept and record the rate rather than bound it**, so that
an operator can size for it and a later pass can revisit with a number. The parked design is a dense
bitset over a hash→index table, which reduces the representation ~32× but does **not** change the
asymptotics. Law 15 pins `seen`'s *value*, not its representation, so the representation is free to
change; what is not free is deciding what may be forgotten.

Anything that raises the block rate without addressing this trades a throughput number for a shorter
time-to-ceiling. The two are not independent.

## What is already implemented but switched off

Two of the levers below are not research problems — the code exists.

**Concurrency on the block path.** The design's central claim is that independent comm events reduce in
parallel, and the sound concurrent effect schedulers are specified and implemented as three
`EffectMode`s (`--effect-scheduler`). But `relaxed` is **off-chain only: the Casper block paths
hard-reject it** ([Effect scheduling](../formal/scheduling.md)). On-chain execution is therefore the
sequential reference today, not the concurrent one. Lifting that is the on-chain validated-speculation
extension — a correctness problem before it is a performance one.

**Sharding.** The node ships single-shard (`/root`) by default, and the multi-shard machinery is real
rather than stubbed: deploy routing by shard id (`node/src/api/shard_routing.rs`), cross-shard invoke
(`casper/src/shard_invoke.rs`), and a two-phase-commit gateway and transaction coordinator
(`casper/src/txn_coordinator.rs`). The gateway's HTTP routes are gated behind `enable-txn-api = false`
(`node/src/configuration/defaults.conf`). Sharding multiplies throughput across shards; it does not
make any single shard faster.

## Reading the limits

| Limit | Value | What kind of thing it is |
|---|---|---|
| Deploys per block | 255 | **Artifact** — a `u8` seed index, not a protocol bound |
| Block rate | 12–38 per minute — **one row a height rate, one unverified** (see the correction above) | **Mixed** — the 2 s timer is a constant; the rest is the proposer and the merge |
| Single-shard deploys/s | ~60–160 | **Derived** from the two above |
| Finality | gap 3–4 blocks | **Measured**, best case observed |
| Merge cost | up to ~2M states/merge | **Structural** — the directed case is unsolved |
| DAG residency | Θ(N²) | **Structural** — accepted and recorded, not bounded |
| Start-up replay | ~0.2 s, ~0.25 MB per block | **Measured**, and superlinear in practice (see [hardware requirements](validator-requirements.md)) |
| On-chain concurrency | not enabled | **Policy** — implemented, hard-rejected on block paths |
| Sharding | off (`/root`) | **Policy** — implemented, default off |

## What would move the numbers

Ranked by leverage against cost, honestly:

1. **Widen the seed index.** `MAX_BLOCK_DEPLOYS` is 255 because the index is a `u8`. A wider index
   lifts the single-shard ceiling by orders of magnitude for a type change and a wire field, and the
   derived `MAX_BLOCK_PHLO` follows it. Cheapest throughput win available — but see the residency
   section before taking it.
2. **Checkpoint the start-up path.** Replay is from genesis every time. Every long-running chain solves
   this with snapshots; it is independent of the consensus work and turns an O(N) restart into
   O(recent).
3. **The directed case.** Output-sensitive enumeration of the terminal states of a digraph. The
   symmetric rewrite is already 1000× faster; this is what makes it apply to the node's real input.
4. **Ancestry pruning.** The structural one. Changes the class of the node from "restart it daily" to
   "leave it running", and it touches the liveness rules, which is why it is deferred.
5. **Turn on the sharded configuration.** Multiplies throughput by shard count. Untested at scale, and
   it does not relieve any single-shard limit.
6. **The on-chain speculation extension.** The design's actual thesis. Largest payoff, and it is
   specified rather than implemented.

One non-lever worth naming: **the fee schedule tracks work only part of the way.** A 229 KB deploy was
measured buying roughly 98 CPU-seconds for 13 phlo ([Security audit](security-audit.md) §9, as measured
in September; the union instance is since bounded). The October re-audit found the *class* still live in
the collection conversions — a sort for a flat 18 phlo — and the same remediated it: `toSet`, `toMap`,
`set`, `keys` and the set/map unions now charge three phlo per element of the output they build, in both
operands for a union (Law 91a); the register that followed found two more of the same class (`set.add`,
the tallies' option lists — Law 91c) and now makes an arm that declares no charge a build failure
([the cost model](../formal/cost-model.md)). **What remains is the shape of the charge, not a flat one**:
it is linear for `O(n log n)` work, so a phlo buys work bounded by `charge · log charge` — bounded, but
not an exact comparison count — and a collection *literal* is still uncharged because its bytes were
charged at parse. Until gas is cost-linked all the way down, "contract throughput" is not a single
number, it is whatever the remaining mispricing admits, which matters more for the credibility of an
estimate than for its magnitude.
