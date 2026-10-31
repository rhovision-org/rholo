# Incident history: what broke, what it cost, and what changed

This is a **dated record, not current behaviour.** Each entry says what was observed, what evidence
exists, and what changed. The page linked from the entry states how the node behaves now; current
disposition of a finding lives in [`spec/AUDIT.md`](../../../spec/AUDIT.md), not here. This page is
also the only node page that keeps GitHub issue and pull-request links, because a record needs to
point at the account it came from.

Not to be confused with [the history chain](history-chain.md), which is a read-only REV-allocation
chain, not a record of defects.

## How to read this page

- A date is the date of the observation, not of the fix.
- A measurement is true of the tree it names. Where a later build changed the tree, the entry says so
  and the technical page carries the current number.
- An entry's **operator consequence** is the part to read before a rebuild: it says what to look for
  and what not to conclude.
- The register row, pass section or evidence file is named where one exists.

## 2026-09-21/22 — the first live net

The first public network exposed the deploy-anchor rule: a CLI deploy that left
`valid_after_block_number` at `-1` was accepted and then swept from the pool once the chain passed
`DEPLOY_LIFESPAN`. The client now resolves the anchor from the node's height
([#58](https://github.com/rhovision-org/rholo/pull/58)); the pool still expires an unanchored deploy.

The same period established that a validator key must not travel in `argv` — a secret there is
readable by every local process — and added the key-file path. The wallet's testnet entry was fixed
in [rchain-wallet#12](https://github.com/rchain-community/rchain-wallet/issues/12).

## Genesis identity and rebuild churn

A chain's genesis parameters are its identity. Joiners whose parameters differed from the
founder's failed `validateBlockCheckpoint`; a joiner whose parameters happened to match a *different*
node on its host synced that node's chain instead. The procedure now treats the PoS parameters as
part of genesis, not as tuning.

The net was rebuilt several times while native PoS state was fixed
([#74](https://github.com/rhovision-org/rholo/issues/74)). The observed chronology: the 2026-09-26
one-validator rebuilds produced `e525129d…`; adding B's bond moved it to `6a6db0db…`; the 2026-09-29
rebuild (one bond and a content change) produced `9f09e7a0…`; the 2026-10-04 rebuild (four bonds at
250, cap 250, active set 4) produced `713c0ebb…`; the 2026-10-24 rebuild (shard renamed `/root` to
`/testnet`) produced `c596bef4…`; the 2026-10-29 rebuild (extended wallets) produced `133b1d30…`, the
current hash. Node identities changed with the TLS certificates, so a peer's bootstrap URL had to be
retargeted. The 2026-09-24 three-validator build and the pre-restart check on
[#373](https://github.com/rhovision-org/rholo/issues/373) are records of that churn, not setup steps.

## Finality and the live weight set

Until 2026-09-29 the finality partition was the **whole bonded set** (`all_bonded`): support counted
only when every validator in the bonded set had seen the message. One bonded validator that produced
no messages therefore capped finality whatever stake the others held. Measured on a three-validator
devnet at `100/100/50`, the two survivors at **80 %** of the active set did not resume finality while
the third was stopped — finality stayed at 8 while the height ran to 126.

The partition is now the **live weight set** — the bonded validators whose latest message is within
`LIVENESS_WINDOW` heights of the tip — while the quorum denominator remains the whole bonded stake.
The safety argument is unchanged: a quorum over the speaking subset alone would let any
self-consistent group finalise, and under a partition two groups would finalise different histories.
See [Consensus](consensus.md#finality--23-bonded-stake) and [#70](https://github.com/rhovision-org/rholo/issues/70).

## The attestation storm

An all-live three-validator devnet attested on every remote block, so with four deploys the chain
produced **126 blocks in about three minutes** (2026-09-29, [#70](https://github.com/rhovision-org/rholo/issues/70),
AUDIT C170/C171). A later measurement recorded **276** of mostly empty attestations in about a minute
([#105](https://github.com/rhovision-org/rholo/issues/105)); the number is recorded as "276 blocks in
about a minute" because the artifact's unit was never resolved. The block rate is not a throughput
capability — it is a defect that a live operator meets as a continually growing chain. The current
block-production modes are in [Operating the node](operating.md#block-production-modes); the
blocks-per-deploy arithmetic is in [the acceptance specification](../spec/testnet-acceptance.md).

## Start-up replay and the 3,200-block correction

Replay is the floor, and it is not linear. An early rule read the measurement as "~0.25 MB RSS and
~0.2 s per block"; the DAG's per-message ancestry sets are Θ(N²) in total, so the marginal cost rises
with the chain already in the DAG. The measured points did not lie on a line: 131 produced blocks at
~0.02 MB per block, a fresh 100–300-block devnet at 0.16–0.23 MB, and **3,200 blocks at ~1.8 GB peak
and ~17 minutes before the API opened** — the last on a pre-`b37410b2e` tree.

`b37410b2e` removed one of the two Θ(N²) terms: the metadata-index rebuild no longer folds through a
full `DagState` clone per block. The ancestry term remains. Two pre-registered rounds on 2026-10-01
([#154](https://github.com/rhovision-org/rholo/issues/154)) could not reach the old ceiling: the
no-load rig's widest scope was 9 chains (0 of 3 crossed on either arm), and the loaded rig reached
161–174 chains but peaked 647–689 MiB against a 3000 MiB threshold. The honest reading is **not "the
ramp is fixed" but "there is no ramp to be gone on this build"**: the 3,200-block row is a property
of a tree this repository no longer builds, and it is no longer a figure to size from. Expect
start-up to worsen superlinearly and treat a few thousand blocks as where a small host stops fitting.
See [hardware requirements](validator-requirements.md#ram-again--start-up-replay-is-the-floor-and-it-is-not-a-steady-state-number)
and [#68](https://github.com/rhovision-org/rholo/issues/68).

## Devnet memory ceiling

A four-way deploy storm on a three-validator devnet under a 4 GiB cgroup OOM-killed two of three
nodes (exit 137) seventeen seconds in, at height ~13 (2026-09-29). The measurement is why the
compose file has a memory ceiling: a node that runs away is killed inside its own cgroup instead of
dragging the host into swap. The ceiling is a test fixture, not production sizing.

The follow-up work separated the process's anonymous regions: 29 stacks of exactly 32 MiB holding
5.7 MiB of RSS between them, and 11 arena heaps of exactly 64 MiB. An earlier reading of the same
histogram inverted the residency (29 × 64 MiB = 1.86 GiB, more than the run's anonymous RSS); AUDIT
C176 corrected it. Read from inside the node, `retained` was **0** in every one of ~1,700 samples
across nine node-runs, with allocated bytes explaining 97–99 % of the cgroup's `anon` at 3.1–8.2 GiB
peaks. The arena cap (`MALLOC_ARENA_MAX=2`) is unproven and deliberately not shipped; see
[hardware requirements](validator-requirements.md#resident-memory-is-not-only-what-the-node-holds--glibcs-arenas)
and [#117](https://github.com/rhovision-org/rholo/issues/117).

## The active-validator draw

The proposer draw was uniform over the active set, which made a small number of keys buy
disproportionate sampling weight and left the selection rule the reference's own source marks as a
placeholder. On 2026-10-02 it became a **stake-weighted** draw over the aggregate bond
(`select_active`), with the epoch seed taken from the last finalised fringe. The residual questions —
what the cap does to a key that is drawn and does not produce, and how a delegator's stake is
attributed — are recorded as O1–O5 in [validator economics](validator-economics.md). See
[#193](https://github.com/rhovision-org/rholo/issues/193), law 57.

## Four equal stakes and losing A

On 2026-10-04 the public net was stopped one validator at a time, starting with **A**, the genesis
master and the node the public endpoint routes to. The other three kept finalising: finality ran
**65 → 90** while the height ran **69 → 94**, in lockstep, and A rejoined to the tip in **under
20 s** when restarted. The arithmetic is the point: four equal stakes of 250 make every validator
25 %, so any one can stop, while three equal stakes fail exactly on the `> 2/3` boundary. See
[the public testnet](testnet.md#routing-and-what-it-costs-if-a-dies) and
[#214](https://github.com/rhovision-org/rholo/issues/214), [#223](https://github.com/rhovision-org/rholo/issues/223).

## Faucet and the unfunded deployer key

The faucet was disabled until 2026-10-06. A fresh dev key could be funded but had no way to pay
phlo for its own deploy, and the deployer vault's `status` field exposed the difference
([#246](https://github.com/rhovision-org/rholo/issues/246),
[#247](https://github.com/rhovision-org/rholo/issues/247)). The drip is 0.3 REV, ten per address per
process, against a 10,000 REV allocation; a `getBalance` immediately after a drip can read 0 because
the read races the write. The current protocol is in [the public testnet](testnet.md#getting-rev--the-faucet).

## The governance handshake

A governance handshake probe stalled between stages 3 and 4 because an `if` that was not the first
term of its `par` reduced to nothing (AUDIT C21). It was not the genesis installation or the feature's
flow; the probe in
[Local devnet](devnet.md#genesis-content-and-what-a-consumer-can-hardcode) now reaches all four stages
on a fresh devnet. The lessons kept on that page are to log the feature's trace rather than drain it,
and to report the reply *value*, not the fact that it answered.

## The finalized-merge duplicate

`StateChange::apply` computed one pre→post diff per produce entry. A consume-and-replace deploy records
the consumed datum and the produced datum on the **same** channel with different event hashes, so a
per-event pass diffed that channel twice and appended the produced datum twice; the finalized merge held
two copies. Both collectors are `BTreeSet`s now — `Vec::dedup` would not do, because entries arrive in
event-hash order rather than grouped by channel. Because `rejected_deploys` is part of the hashed block
body, the fix is a hard-fork change: every validator must run it from the fork point, and a chain that
ran the old rule must be rebuilt. See [block merging](block-merge.md) and
[#420](https://github.com/rhovision-org/rholo/issues/420).

## K8 write loss at an epoch boundary

On 2026-10-08 a deploy landed in block 100 — an epoch boundary on a net with `epoch_length = 10`, so
every validator's block in that round runs `close_block`. Its write was `processedWithSuccess` and
was in the state at heights 100, 101 and 102, including the finalised block, then absent from 103
onward **on every node at once**. No peer refused a block and no node disagreed with another: the
disagreement was between the chain and its own past, because the same round resolves differently
once finality advances past it.

The cause was the merge's **native** relation, keyed on the *host block* — the union of every chain
that rode in on it — with a rejection closed over the whole block. At a boundary that makes every
concurrent pair conflict, including pairs whose user chain wrote no contended slot, and the user
chain died with its host. The merge had no logger and the whole-block rule fired with no counter, so
the only visible trace was a deploy absent from a state that reported it processed.

The relation is now on each chain's own writes, the whole-block rule is deleted, and the merge
returns a report that the validation path logs when the merge is not quiet. It does **not** repair
the chain it happened on: three of four validators agreed on the pre-state that dropped the write,
so recovery is a rollback below height 100 or accepting the loss. It is a **hard fork** —
`rejected_deploys` is part of the hashed block body — so a running chain must be rebuilt from genesis.
See [block merging](block-merge.md), [#280](https://github.com/rhovision-org/rholo/issues/280),
[#51](https://github.com/rhovision-org/rholo/issues/51) and
`spec/audit/evidence/n280-merge-loses-a-write-results.md`.

## Four divergent heads and reconciliation drills

On 2026-10-09 the public net reached four divergent heads with finality frozen at 101 for more than
thirteen hours. Nothing in-protocol recovers that state; the store-level restore is the only recovery
demonstrated for it. `tools/reconcile-network.sh --restore-from-master` stops the network (a live
LMDB copy is a torn snapshot), copies chain state and never identity, prints the plan before applying,
and refuses when it cannot prove its anchor.

Three register rows came out of the drills:

- **C272** — the anchor is the **meet**: the deepest block a strict supermajority of stake has
  finalised, computed from signed finality attestations and verified block preimages. It no longer
  requires every node to report the identical finalised block, which a producing chain cannot do;
  three survivors read finalised 102 / 97 / 110.
- **C269** — where nothing is finalised, the operator names `RECONCILE_ANCHOR`, and the tool says in
  the plan that the root is the operator's, not a computed meet.
- **C259** — windowed catch-up: a rejoining node walked 300 heights below the tip in 98 s, and a
  316-root restore took about 2.5 minutes. Restore is O(chain); sidecar caches are regenerated.

See [the public testnet](testnet.md#recovery), `docs/src/spec/testnet-acceptance.md` §TE-1, and
`spec/audit/evidence/te-1-2026-10-09-four-divergent-heads.md`.

## Testnet rebuild and receiver exhaustion

On 2026-10-29 both hosts were resized to 16 GiB and the chain was rebuilt with extended wallet
allocations. A then produced blocks while B/C/D remained at height 6 after receiver exhaustion (1024
pending entries); a shared early finalised block did not mean the net was healthy. The split was
still present at 18:11 UTC (A at height 353 / finalised 3; B/C/D at 6 / finalised 2), with D
repeatedly dropping blocks at its receiver limit. Available APIs and `degraded: normal` were not
evidence of consensus. Restarting under load is not a recovery strategy; see
[the public testnet](testnet.md) and [#479](https://github.com/rhovision-org/rholo/issues/479),
[#485](https://github.com/rhovision-org/rholo/issues/485).

## Ingress admission and the memory gate

The first admission design put a boolean **503** in front of every deploy and explore below the
throttle floor. On an undersized host it turned a local memory problem into a network-wide liveness
failure while the pool still had free slots. It was removed by
[#477](https://github.com/rhovision-org/rholo/issues/477): the **pool** is the throttle, memory
bounds **execution permits**, and a constrained node holds work and (optionally) relays it to peers.
The current contract — 429 + `Retry-After`, the permit formula, the opt-in HTTP bounds, the relay and
`GET /api/v1/admission` — is [Ingress admission](admission.md). The remaining resource-safety work is
the [resource-safety case](../spec/resource-safety-case.md).

## September 2026 audit close-out

The report is [the security audit](security-audit.md) §9; this is its close-out, kept apart so the
report stays a photograph. The **October 2026 re-audit** (§1–§8 of the same page, pass §148) is the
next pass: it re-derived this close-out, found the cost-model class live again in the collection
conversions, and opened C345–C352. **All eight were remediated in the same PR** (pass §149): the
conversions and unions charge their output, `censure` its cube (Law 91), the greeter and the websocket
accept are bounded, the body and its disposition row are one transaction, and the proposal finality
bound is total in its anchor. C348 is an accepted residual and C352 a corrected claim. **Pass §150 then
built the instrument the audit said was missing** — a register of every priced operation, a census that
refuses an arm declaring no charge, and a shape test — and its first census found **two more instances**
the arm-by-arm pass had missed: `set.add` (C353, a flat 3 phlo for a full re-canonicalisation) and both
tallies (C354, whose charge ignored the option lists entirely).

- The three P1s were one root cause — the gas model did not bound work — closed by a per-block phlo
  cap, charging the operations that charged less than their work, and charging the parse. The
  P2s closed by a DAG write order that writes data before the pointer, saturating arithmetic in the
  `qucalc` governance folds, and a block `version` predicate that had existed with no caller.
- One planned charge fix was **reverted because implementing it showed the finding was wrong**: the
  crypto builtins' inputs pass through a storage path that already charges proportionally. The
  remediation advice that named a `HashSet` also did not compile, because `Par` derives no `Hash`;
  the fix is sort-then-dedup with identical output.
- Signing and an SBOM closed as a decision: the release publishes a `sha256sum` and nothing further,
  because a signature needs a trust root that a code change should not choose silently. Cancellation
  inside a running builtin is not achievable on this design — a builtin runs its CPU synchronously
  with no `await` — which is why the remediation bounds the work rather than interrupting it.
- Of the ten findings the pass refuted, the one it opened expecting to lead with is among them ([§116
  of the pass record](../../../spec/audit/passes.md)).
- The register grew to **387 rows — 383 `done`, 4 `in progress`, 0 `todo`**; the Rust witness set
  from 88 to **260 entries over 239 distinct symbols**. The tree's three `#[ignore]`s are a diagnostic
  table printer (`casper/src/blocks/proposer/proposer.rs`) and two benchmarks/replays
  (`casper/tests/n420_rcash_replay.rs`, `casper/tests/startup_metadata_bench.rs`); none is a
  registered witness.

## Acceptance recipes and handoff records

The #447 acceptance work produced working records that are not book pages; their durable content is
summarised here and the files are in git history.

- **Application-state partition**: a 2+2 split with no cross-pair links, two cycles, distinct tips,
  then a heal that covers both histories and produces equal final states. The harnesses are
  `tools/n447_partition.py` and `tools/n420-fork-point.py`; the acceptance arms in
  [the acceptance specification](../spec/testnet-acceptance.md).
- **Same-channel conflict**: two signed deploys consume one linear datum; exactly one must appear in
  `rejectedDeploys`. The earlier read-only probe was a false-positive control, not a witness — it
  read data at a name by block hash rather than through the exploratory path.
- **Consumed-seed merge**: exact-byte multiset cancellation; the consumed datum must not be resurrected.
- **r-cash workload**: the upstream 17-step `install_testnet.py` at pinned SHA
  `0c1cc6e4…`; a verified installer is not the same as a four-validator partition acceptance.
- **Registry read**: at a pinned block, `rho:registry:lookup` must answer Nil at the ancestor and
  pre-state and 42 at post-state; a Nil at the same post-state is the reportable mismatch.

The **canonical N=4/N=8 handoff** proposed a review contract: run identity and immutable manifest
per arm, no PASS without a complete artifact bundle, fail-closed verdicts
(`INVALID` / `FAIL` / `INCOMPLETE` / `PASS`), and `MERGE-SAFE FOR REVIEW` only when every arm passes
on one candidate SHA. It was **not executed** and granted no approval; it lives in git history.

## The legacy archive

The upstream Scala tree was archived out of the working tree on 2026-09-28: 1,596 files, 26.2 MiB of
tracked content, which no CI job built, tested or scanned and which carried a 2020–21 dependency
manifest `cargo-deny` cannot see. It is readable at `1b7583649`. See
[Where the Scala went](../../../README.md#where-the-scala-went).

## What the record leaves behind

Each entry closed into something a later reader can run or read:

| incident | what holds it now |
|---|---|
| finality partition | `block-storage/src/dag/liveness.rs`; [consensus](consensus.md#finality--23-bonded-stake) |
| attestation storm | the cadence guard; [operating](operating.md#block-production-modes) |
| start-up replay | `tools/recount-artifact.sh` records the tree; [hardware](validator-requirements.md) |
| devnet memory | the compose memory ceiling; AUDIT C176; [#117](https://github.com/rhovision-org/rholo/issues/117) |
| admission gate | [admission](admission.md); the [resource-safety case](../spec/resource-safety-case.md) |
| K8 write loss | [block merging](block-merge.md); `n280-merge-loses-a-write-results.md` |
| divergent heads | `tools/reconcile-network.sh`; TE-1 evidence |
| audit close-out | [`spec/AUDIT.md`](../../../spec/AUDIT.md); [`spec/audit/passes.md`](../../../spec/audit/passes.md) |
