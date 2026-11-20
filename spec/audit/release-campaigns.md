# Release claims

Emitted from `spec/audit/release-claims.json` by `tools/build-mainnet-gates.py`.

**These are not gates.** A gate is a boolean test of the current build. A release claim
is one observable node behaviour with one executable test, re-runnable on the standard
local devnet or on `testnet-4`. A claim's status is `pass`, `fail` or `not run`: it is
measured from that environment's newest committed run record by re-running the test's
pure `check(record)`. No status is authored, and a claim without an existing test is
refused. The 12 old campaign rows are deleted; their fate is in the `superseded` table.

- generated: 2026-11-20
- claims: 7
- review: `spec/audit/release-campaign-red-team-20261119.md`

| claim | local | public | test | owner |
|---|---|---|---|---|
| `RC-1` — Truthful settlement | **fail** `0669419` | **not run** | `tests/release-claim-1.py` | Aria / operator |
| `RC-2` — Safety | **not run** | **not run** | `tests/release-claim-2.py` | Jim / release coordination |
| `RC-3` — State agreement across a merge | **pass** `0669419` | **not run** | `tests/release-claim-3.py` | Jim / release coordination |
| `RC-4` — Recovery without loss | **fail** `0669419` | **not run** | `tests/release-claim-4.py` | Jim / release coordination |
| `RC-5` — Overload refusal | **pass** `0669419` | **not run** | `tests/release-claim-5.py` | Jim / release coordination |
| `RC-6` — Join and audit | **fail** `0669419` | **not run** | `tests/release-claim-6.py` | Aria / evidence work |
| `RC-7` — Upgrade compatibility | **not run** | **not run** | `tests/release-claim-7.py` | Jim / release coordination |

---

## RC-1 — Truthful settlement

**Claim.** If the settlement endpoint reports finalizedSuccess and settlementConfirmed for a deploy, that deploy's effect is present at the finalised head on every validator and remains present through two further epoch boundaries.

**Pass.** For one deploy id: the settlement response is finalizedSuccess with settlementConfirmed true; every validator reports the same finalised (height, hash); a marker written by the deploy reads back at that finalised block and at every later finalised head sampled; the deploy id never appears in a later block's rejectedDeploys.

**Fail.** A confirmed settlement whose effect is absent at the finalised head on any validator; validators disagree on the finalised hash; or the deploy id appears in a later rejectedDeploys.

**Observe.** The settlement response, each validator's finalised (height, hash), the marker read at that block, and the per-block rejectedDeploys.

**Control.** A marker absent before the write; a fixture where the effect is absent at the finalised head; a fixture where the deploy is final-scope rejected.

**Test.** `tests/release-claim-1.py`

**Local.** local-4 (tools/devnet.sh; 2 GiB per validator, below the documented tier)

**Public.** testnet-4: four independent hosts on the selected release

**Re-run (local).** `python3 tools/release-check.py run --claim RC-1 --env local`

**Re-run (public).** `python3 tools/release-check.py run --claim RC-1 --env public --public-manifest <manifest.json>`

- **local:** fail — spec/audit/evidence/rc1-local-20261120-0669419/raw/claim.json
  - the acceptance drill did not pass (marker retention)
  - the absent-marker control did not hold before the write
  - settlements: missing or empty
  - finalised: missing or empty
  - rejected: missing or not a list
  - no settlement reported finalizedSuccess with settlementConfirmed true
- **public:** not run — no committed run record

**Supersedes.** G584-G2, G584-G3, TA-C1

**Does not claim.** A local pass is a mechanism check; the public claim needs testnet-4.

---

## RC-2 — Safety

**Claim.** No two conflicting finalised histories: every finalised block held by one validator is finalised by every validator at or above its height, and no finalised (height, hash) regresses across restart, replay or partition.

**Pass.** For every sampled finalised block, every validator at or above its height reports it finalised; no finalised (height, hash) regresses; no two conflicting finalised histories are observed; the isolated victim keeps producing while its finalised fence stays frozen.

**Fail.** A finalised block not finalised by another validator at or above its height; a finalised regression; or two conflicting finalised histories.

**Observe.** Per-validator finalised heads at each sample, the cross-node containment answers, and the finalised history over the window.

**Control.** A producing split (the victim proposes while isolated); an equivocation injection under the allowed Byzantine bound.

**Test.** `tests/release-claim-2.py`

**Local.** local-4 (tools/devnet.sh) with a producing victim

**Public.** testnet-4 with a real partition and the operator window

**Re-run (local).** `python3 tools/release-check.py run --claim RC-2 --env local`

**Re-run (public).** `python3 tools/release-check.py run --claim RC-2 --env public --public-manifest <manifest.json>`

- **local:** not run — spec/audit/evidence/rc2-local-20261120-0669419/raw/claim.json
  - RC-1's settlement-retention precondition failed; no partition was installed
- **public:** not run — no committed run record

**Supersedes.** G428-M4, TA-C1

**Does not claim.** The claim assumes the <=1/3 Byzantine/equivocation bound; without it an equivocator can back two quorums.

---

## RC-3 — State agreement across a merge

**Claim.** For the same accepted block on the same binary, every validator derives the same preStateHash and postStateHash and the same kept/rejected ledger; kept tuple-space effects are present and rejected effects absent.

**Pass.** Across a defined window, every validator reports the same preStateHash, postStateHash and rejectedDeploys for every accepted block, and a kept marker is readable at the same block on every validator.

**Fail.** Any preStateHash, postStateHash or rejectedDeploys disagreement for the same block; a same-block marker present on one validator and absent on another.

**Observe.** The per-block preStateHash, postStateHash and rejectedDeploys from every validator, plus a same-block marker read.

**Control.** A fixture where the ledger differs; a native-write case (native absolute writes are excluded from FinalizedMerge.lean's unqualified law).

**Test.** `tests/release-claim-3.py`

**Local.** local-4 (tools/devnet.sh)

**Public.** testnet-4 on the selected release

**Re-run (local).** `python3 tools/release-check.py run --claim RC-3 --env local`

**Re-run (public).** `python3 tools/release-check.py run --claim RC-3 --env public --public-manifest <manifest.json>`

- **local:** pass — spec/audit/evidence/rc3-local-20261120-0669419/raw/claim.json
- **public:** not run — no committed run record

**Supersedes.** G428-M4

**Does not claim.** The claim is conditional on the same binary and conflict scope; block-merge.md records that two binaries can reject different chains at a merge.

---

## RC-4 — Recovery without loss

**Claim.** After one validator is stopped and restarted, or a partition heals, the survivors keep finalising; every deploy accepted by a survivor is resolved and its effect is in the post-recovery finalised state; the victim rejoins the same finalised head; no spurious self-replay or own-key halt fires.

**Pass.** With one validator stopped, the survivors finalise past the kill; every deploy submitted to a survivor resolves and its effect is present at the post-recovery finalised head; after restart the victim reports the same finalised (height, hash) at least three heights above the baseline; no finalised height regresses and no fixed height changes hash; no own-key halt or self-replay is observed.

**Fail.** Finality stalls; accepted work is lost; the rejoiner needs manual surgery; a finalised height regresses; or a spurious halt fires.

**Observe.** The survivors' finalised heads, every accepted deploy's resolution and effect at the post-recovery finalised head, the victim's boot ID and finalised head, and the own-key/self-replay counters.

**Control.** The wrong-genesis and interrupted-restart arms; a deploy accepted only by the killed validator is not recoverable by design and must be declared, not counted.

**Test.** `tests/release-claim-4.py`

**Local.** local-4 (tools/devnet.sh; tests/consensus-restart-under-load.py)

**Public.** testnet-4 with a real host stop

**Re-run (local).** `python3 tools/release-check.py run --claim RC-4 --env local`

**Re-run (public).** `python3 tools/release-check.py run --claim RC-4 --env public --public-manifest <manifest.json>`

- **local:** fail — spec/audit/evidence/rc4-local-20261120-0669419/raw/claim.json
  - the restart harness did not pass: 'FAIL'
  - post-recovery heads were not read on every validator
  - the victim was not restarted (its boot time did not change)
  - evidence-insufficient: the failed restart carries no boot trace
- **public:** not run — no committed run record

**Supersedes.** G584-G4, TA-C2, G428-M4

**Does not claim.** A store loss needs a fresh data dir and a checkpoint join; 'no operator action' covers a verified restart, not a lost store.

---

## RC-5 — Overload refusal

**Claim.** At the configured admission/pool bound, the node refuses new work with HTTP 429 and Retry-After: 1, or gRPC RESOURCE_EXHAUSTED (ingress-rate and pool-bound classes stated separately), returns no 5xx, stays alive, and loses no deploy it accepted.

**Pass.** With the bound configured: every permit is held, further probes are refused with the named class and retry signal, no 5xx is returned, the unset/default control admits, the handler is reachable after release, and every accepted deploy id is still readable or explicitly failed.

**Fail.** A 5xx under pressure; an accepted deploy that later disappears; an unbounded queue; an unset default that refuses; or a refusal class outside the vocabulary.

**Observe.** Accepted/refused counts, refusal class and retry signal, handler reachability after release, and the status of every accepted deploy id.

**Control.** The unset-default arm must admit; the four named queue bounds; a refusal class outside the vocabulary.

**Test.** `tests/release-claim-5.py`

**Local.** local-4 (one node, direct listener and in-process gRPC)

**Public.** testnet-4 public endpoint and proxy

**Re-run (local).** `python3 tools/release-check.py run --claim RC-5 --env local`

**Re-run (public).** `python3 tools/release-check.py run --claim RC-5 --env public --public-manifest <manifest.json>`

- **local:** pass — spec/audit/evidence/rc5-local-20261120-0669419/raw/claim.json
- **public:** not run — no committed run record

**Supersedes.** G428-M2

**Does not claim.** The opt-in in-flight cap is not the shipping default; the pool's count/byte/term/share/rate bounds are the intended throttle. Transport correctness across HTTP, direct HTTP, gRPC and a proxy is out of scope.

---

## RC-6 — Join and audit

**Claim.** A fresh node joining from a bootstrap receives the certified anchor, applies and hydrates it, and reaches the same finalised head and state; rnode audit reproduces the checkpoint claim from genesis.

**Pass.** The joiner's served, received, restored and walked hashes agree for the certified anchor (equality with the served root is required only for a genesis anchor); a legacy certificate with no commitments is refused; rnode audit returns AGREE, its audited digest equals the accepted digest when the anchor is a checkpoint, and its derived root equals the checkpoint claim.

**Fail.** The joiner receives but never applies; the restored state differs from the certified anchor; a certificate with no commitments is accepted; audit returns DISAGREE, a different digest, or a derivation over a different checkpoint.

**Observe.** The six-stage join chain, the certified anchor's post-state root and fringeState, the audited digest, and the audit verdict.

**Control.** A send-log-only chain; a served/restored hash mismatch; a legacy certificate with no commitments.

**Test.** `tests/release-claim-6.py`

**Local.** local-4 (genesis-only join and audit)

**Public.** testnet-4 with a public join

**Re-run (local).** `python3 tools/release-check.py run --claim RC-6 --env local`

**Re-run (public).** `python3 tools/release-check.py run --claim RC-6 --env public --public-manifest <manifest.json>`

- **local:** fail — spec/audit/evidence/rc6-local-20261120-0669419/raw/claim.json
  - rnode audit exited 1: Configuration error: `run` subcommand is required to build a node configuration
  - rnode audit verdict is None, not AGREE
  - the audit consumed no bodies
  - the audit's digest does not equal the joiner's accepted digest
- **public:** not run — no committed run record

**Supersedes.** G428-M3

**Does not claim.** A routable address is setup, not the claim; off-host transport and the loopback refusal are out of scope. Alternative-peer failover (M3-11) is separate and never landed.

---

## RC-7 — Upgrade compatibility

**Claim.** A mixed-version fleet either keeps one finalised chain and one state for a block produced during the window, or refuses on the consensus path; a populated store migrates; a binary refuses a store written by a newer format.

**Pass.** During a mixed-version window, a block is produced and finalised with both binaries deriving the same postStateHash and rejectedDeploys, or the old node refuses on the consensus path; a populated-store copy/rollback/restart drill passes; a build refuses a store whose format marker is higher than the one it writes; after the swap the upgraded node reaches the same finalised head.

**Fail.** Two versions finalise different blocks or derive different post-state hashes without a refusal; the old binary runs on a newer store; or the populated-store drill fails.

**Observe.** The mixed-version block's hashes and rejection ledger on both binaries, the store-format refusal, the populated-store drill verdict, and the post-swap finalised head.

**Control.** A fresh-genesis run must not substitute for the populated-store arm; a legacy/no-commitment certificate must be refused.

**Test.** `tests/release-claim-7.py`

**Local.** local-4 (N and N+1 binaries); the G520 populated-store drill

**Public.** testnet-4 fleet

**Re-run (local).** `python3 tools/release-check.py run --claim RC-7 --env local [--old-image rnode:old]`

**Re-run (public).** `python3 tools/release-check.py run --claim RC-7 --env public --public-manifest <manifest.json>`

- **local:** not run — spec/audit/evidence/rc7-local-20261120-0669419/raw/claim.json
  - no --old-image/RNODE_OLD_IMAGE: the mixed-version arm needs a separately built prior image
- **public:** not run — no committed run record

**Does not claim.** For a fresh-genesis launch this is post-launch; it becomes a launch blocker only over a populated store or a mixed fleet. The mixed arm needs a separately built prior image.

---

## Superseded rows

| old id | fate | home |
|---|---|---|
| `C343` | runbook | spec/audit/runbooks/c343-live-readonly-20261119.md |
| `G428-M1` | sizing guidance | docs/src/node/validator-requirements.md and the M1 runbook |
| `G428-M2` | claim RC-5 | release-claims.json RC-5 |
| `G428-M3` | claim RC-6 | release-claims.json RC-6 |
| `G428-M4` | claims RC-2, RC-3, RC-4 | release-claims.json RC-2/RC-3/RC-4 |
| `G584-G1` | setup precondition | the preflight before every live claim |
| `G584-G2` | claim RC-1 | release-claims.json RC-1 |
| `G584-G3` | claim RC-1 | release-claims.json RC-1 |
| `G584-G4` | claim RC-4 | release-claims.json RC-4 |
| `G584-G5` | evidence rule | tools/evidence-pack.py and docs/src/spec/gate-measurement.md |
| `TA-C1` | claims RC-1 and RC-2; attestation is an evidence rule | release-claims.json RC-1/RC-2 |
| `TA-C2` | claim RC-4; bond/leave are out of scope | release-claims.json RC-4 |

## Not covered by these seven claims

- Off-host routable transport and the loopback refusal (M3).
- Alternative-peer failover (M3-11).
- Host loss and the multi-host N=4/N=8 envelope (M4).
- Bond and leave (TA-C2).
- Transport correctness across HTTP, direct HTTP, gRPC and a proxy.
- Finalised block body and state availability on every validator.
- Equivocation injection under a validator key.

`check-release-campaigns.py --assert-ready` exits 3 unless every claim is `pass` in both
environments, so a closed gate register is never a release authorization.
