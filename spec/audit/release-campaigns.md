# Release claims

Emitted from `spec/audit/release-claims.json` by `tools/build-mainnet-gates.py`.

Each claim is a gate with one executable test over a decision trace, exercised by named
**arms**. An arm has its own environment, evidence locality and next PR; its state is a
measured **reading** of the newest committed record. `safe` is the only reading that
closes an arm. `unsafe-point` names a false predicate and its Rust and Lean anchors;
`model-incomplete` names the trace that is the model gap; `evidence-insufficient` names
the missing trace or coordinate; `precondition` and `not-observed` name the run that
cannot yet be made. No status is authored.

- generated: 2026-11-20
- claims: 7
- arms: 25
- review: `spec/audit/release-campaign-red-team-20261119.md`

| claim | arm | env | locality | reading | next action | next PR |
|---|---|---|---|---|---|---|
| `RC-1` | `local` | local | local-devnet | **safe** | — | — |
| `RC-1` | `public` | public | public-live | **not-observed** | Supply the four-host public manifest and run `tools/release-check.py run --claim RC-1 --env public`; the driver submits the deploy and records retention across two epoch boundaries | `rc-public-fleet` |
| `RC-1` | `bond-leave` | local | local-devnet | **not-observed** | Run `python3 tools/release-check.py run --claim RC-1 --arm bond-leave` and commit its record; the arm's harness lands in `rc-bond-leave`. | `rc-bond-leave` |
| `RC-2` | `local` | local | local-devnet | **safe** | — | — |
| `RC-2` | `public` | public | public-live | **not-observed** | Run the operator's declared partition window on the public fleet. | `rc-public-fleet` |
| `RC-2` | `multi-host` | local | multi-host | **not-observed** | Run `python3 tools/release-check.py run --claim RC-2 --arm multi-host` and commit its record; the arm's harness lands in `rc-multi-host`. | `rc-multi-host` |
| `RC-2` | `equivocation` | local | multi-host | **not-observed** | Run `python3 tools/release-check.py run --claim RC-2 --arm equivocation` and commit its record; the arm's harness lands in `rc-equivocation`. | `rc-equivocation` |
| `RC-2` | `validator-set` | local | local-devnet | **not-observed** | Run `python3 tools/release-check.py run --claim RC-2 --arm validator-set` and commit its record; the arm's harness lands in `rc-bond-leave`. | `rc-bond-leave` |
| `RC-3` | `local` | local | local-devnet | **safe** | — | — |
| `RC-3` | `public` | public | public-live | **not-observed** | Supply the four-host public manifest (deploy id, marker, provenance) and run `python3 tools/release-check.py run --claim RC-3 --env public --public-manifest <manifest.json>`. | `rc-public-fleet` |
| `RC-3` | `availability` | public | public-live | **not-observed** | Run the finalised-block availability sweep on the public fleet: every finalised body and finalised-head state from every validator. | `rc-public-fleet` |
| `RC-4` | `local` | local | local-devnet | **safe** | — | — |
| `RC-4` | `public` | public | public-live | **not-observed** | Run `tools/release-check.py run --claim RC-4 --env public`; the driver performs the single-validator stop and restart and captures the boot ids itself. | `rc-public-fleet` |
| `RC-4` | `multi-host` | local | multi-host | **not-observed** | Run `python3 tools/release-check.py run --claim RC-4 --arm multi-host` and commit its record; the arm's harness lands in `rc-multi-host`. | `rc-multi-host` |
| `RC-4` | `failover` | local | off-host-claim | **not-observed** | Run `python3 tools/release-check.py run --claim RC-4 --arm failover` and commit its record; the arm's harness lands in `rc-failover`. | `rc-failover` |
| `RC-4` | `returner` | local | local-devnet | **not-observed** | Run `python3 tools/release-check.py run --claim RC-4 --arm returner` and commit its record; the arm's harness lands in `rc-bond-leave`. | `rc-bond-leave` |
| `RC-5` | `local` | local | local-devnet | **safe** | — | — |
| `RC-5` | `public` | public | public-live | **not-observed** | Supply the capped/unset pressure record from the public endpoint and proxy. | `rc-public-fleet` |
| `RC-5` | `transport` | local | local-devnet | **not-observed** | Run `python3 tools/release-check.py run --claim RC-5 --arm transport` and commit its record; the arm's harness lands in `rc-transport`. | `rc-transport` |
| `RC-6` | `local` | local | local-devnet | **safe** | — | — |
| `RC-6` | `public` | public | public-live | **not-observed** | Supply the operator's completed public join record. | `rc-public-fleet` |
| `RC-6` | `off-host` | local | off-host-claim | **not-observed** | Run `python3 tools/release-check.py run --claim RC-6 --arm off-host` and commit its record; the arm's harness lands in `rc-offhost`. | `rc-offhost` |
| `RC-6` | `availability` | public | public-live | **not-observed** | Bootstrap a joiner from a non-producer on the public fleet, restore the served anchor and audit it. | `rc-public-fleet` |
| `RC-7` | `local` | local | local-devnet | **safe** | — | — |
| `RC-7` | `public` | public | public-live | **not-observed** | Record the rolling fleet swap's mixed-window block agreement and the post-swap finalised head. | `rc-fleet-upgrade` |

## Next PRs

| PR | closes | scope |
|---|---|---|
| `rc-confirm-local` | RC-1, RC-2, RC-3, RC-4, RC-5, RC-6 | Re-run the baseline local arms with the current image and commit the records. Read each failure with tools/trace-evaluate.py: unsafe-point names a predicate and its anchor; model-incomplete names a model gap; evidence-insufficient names the missing trace. Close an arm only when its committed record reads safe. |
| `rc-harness-old-image` | RC-7 | Build and pin a prior image for the mixed fleet, set RNODE_OLD_IMAGE, and run RC-7's local mixed-store arm. |
| `rc-public-fleet` | RC-1, RC-2, RC-3, RC-4, RC-5, RC-6, RC-7 | Bring up the four-host public fleet on a coordinated new genesis, perform the deploy and the single-validator restart through tools/fleet-run.py, and run RC-1/public and RC-4/public on the candidate. The source set and the N1/N4/N8 differential are recorded as diagnostics in the same campaign record; the remaining public arms keep this campaign as their next PR. |
| `rc-bond-leave` | RC-1, RC-2, RC-4 | One bond/leave campaign feeds RC-1 bond-leave, RC-2 validator-set and RC-4 returner. |
| `rc-offhost` | RC-6 | Run the pre-registered M3 two-host join and record RC-6 off-host, including the loopback negative. |
| `rc-failover` | RC-4 | Add an ordered seed list to bootstrap resolution and run RC-4 failover with the first seed down. |
| `rc-multi-host` | RC-2, RC-4 | Add the multi-host driver the M4 runbook owes, run N=4 and N=8 on declared hosts, and record RC-2/RC-4 multi-host from the one campaign. |
| `rc-equivocation` | RC-2 | Inject an equivocation under a validator key on the M4 topology, add the finality coordinate, and record RC-2 equivocation. |
| `rc-transport` | RC-5 | Drive one deploy id through HTTP, direct HTTP, gRPC and a proxy and record RC-5 transport. |
| `rc-fleet-upgrade` | RC-7 | Roll the public fleet through the mixed window to the new image and record RC-7 public. |
| `launch-gate` | RC-1, RC-2, RC-3, RC-4, RC-5, RC-6, RC-7 | Aggregate the required arms and the external obligations into the launch gate; refuse ready while any external item is open without a dated disposition. |

---

## RC-1 — Truthful settlement

**Claim reading.** `not-observed` (the worst required arm).

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

**Arm `local`** (local, `local-devnet`) — the acceptance drill retains a confirmed marker through two further epoch boundaries

Reading: `safe` — spec/audit/evidence/rc1-local-20261122-5a0e755/raw/claim.json
- No action: the reading is safe; carry the record forward.

**Arm `public`** (public, `public-live`) — the same settlement and retention reading on the four-host public fleet

Reading: `not-observed` — no committed run record
- `RC-1/public/observe` — no committed run record. **Next:** Supply the four-host public manifest and run `tools/release-check.py run --claim RC-1 --env public`; the driver submits the deploy and records retention across two epoch boundaries.
- Next PR: `rc-public-fleet`

**Arm `bond-leave`** (local, `local-devnet`) — a bond and a withdrawal settle, activate at an epoch boundary, pay after quarantine, and the settlement marker is retained

Reading: `not-observed` — no committed run record
- `RC-1/bond-leave/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-1 --arm bond-leave` and commit its record; the arm's harness lands in `rc-bond-leave`.
- Next PR: `rc-bond-leave`

**Supersedes.** G584-G2, G584-G3, TA-C1

**Does not claim.** A local pass is a mechanism check; the public claim needs testnet-4. The bond-leave arm covers a bond and a withdrawal; it does not claim a validator can leave with an unpaid bond or skip quarantine.

---

## RC-2 — Safety

**Claim reading.** `not-observed` (the worst required arm).

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

**Arm `local`** (local, `local-devnet`) — partition/rejoin with a producing victim; containment and no regression

Reading: `safe` — spec/audit/evidence/rc2-local-20261122-5a0e755/raw/claim.json
- No action: the reading is safe; carry the record forward.

**Arm `public`** (public, `public-live`) — the operator's partition window on the public fleet

Reading: `not-observed` — no committed run record
- `RC-2/public/observe` — no committed run record. **Next:** Run the operator's declared partition window on the public fleet.
- Next PR: `rc-public-fleet`

**Arm `multi-host`** (local, `multi-host`) — N=4 and N=8 on distinct declared hosts: partition, host loss, approved-root retention, and a safe halt beyond tolerance with no conflicting finality

Reading: `not-observed` — no committed run record
- `RC-2/multi-host/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-2 --arm multi-host` and commit its record; the arm's harness lands in `rc-multi-host`.
- Next PR: `rc-multi-host`

**Arm `equivocation`** (local, `multi-host`) — two signed blocks at one height under the <=1/3 bound produce no conflicting finality and the fault surfaces

Reading: `not-observed` — no committed run record
- `RC-2/equivocation/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-2 --arm equivocation` and commit its record; the arm's harness lands in `rc-equivocation`.
- Next PR: `rc-equivocation`

**Arm `validator-set`** (local, `local-devnet`) — every validator agrees on the bond set at each epoch boundary and the finality denominator is that set

Reading: `not-observed` — no committed run record
- `RC-2/validator-set/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-2 --arm validator-set` and commit its record; the arm's harness lands in `rc-bond-leave`.
- Next PR: `rc-bond-leave`

**Supersedes.** G428-M4, TA-C1

**Does not claim.** The claim assumes the <=1/3 Byzantine/equivocation bound; without it an equivocator can back two quorums. The multi-host arm covers N=4 and N=8 on declared hosts, not an arbitrary topology.

---

## RC-3 — State agreement across a merge

**Claim reading.** `not-observed` (the worst required arm).

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

**Arm `local`** (local, `local-devnet`) — state agreement across a merge on the local four-validator devnet

Reading: `safe` — spec/audit/evidence/rc3-local-20261122-5a0e755/raw/claim.json
- No action: the reading is safe; carry the record forward.

**Arm `public`** (public, `public-live`) — state agreement on the public fleet

Reading: `not-observed` — no committed run record
- `RC-3/public/observe` — no committed run record. **Next:** Supply the four-host public manifest (deploy id, marker, provenance) and run `python3 tools/release-check.py run --claim RC-3 --env public --public-manifest <manifest.json>`.
- Next PR: `rc-public-fleet`

**Arm `availability`** (public, `public-live`) — every finalised block body and finalised-head state is servable from every validator; not-yet-available never hides a loss

Reading: `not-observed` — no committed run record
- `RC-3/availability/observe` — no committed run record. **Next:** Run the finalised-block availability sweep on the public fleet: every finalised body and finalised-head state from every validator.
- Next PR: `rc-public-fleet`

**Supersedes.** G428-M4

**Does not claim.** The claim is conditional on the same binary and conflict scope; block-merge.md records that two binaries can reject different chains at a merge.

---

## RC-4 — Recovery without loss

**Claim reading.** `not-observed` (the worst required arm).

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

**Arm `local`** (local, `local-devnet`) — restart under load: accepted work survives and the victim rejoins

Reading: `safe` — spec/audit/evidence/rc4-local-20261122-5a0e755/raw/claim.json
- No action: the reading is safe; carry the record forward.

**Arm `public`** (public, `public-live`) — a real host stop on the public fleet

Reading: `not-observed` — no committed run record
- `RC-4/public/observe` — no committed run record. **Next:** Run `tools/release-check.py run --claim RC-4 --env public`; the driver performs the single-validator stop and restart and captures the boot ids itself.
- Next PR: `rc-public-fleet`

**Arm `multi-host`** (local, `multi-host`) — host loss, restart/replay and the recovery arm on the M4 topology with no lost accepted deploy

Reading: `not-observed` — no committed run record
- `RC-4/multi-host/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-4 --arm multi-host` and commit its record; the arm's harness lands in `rc-multi-host`.
- Next PR: `rc-multi-host`

**Arm `failover`** (local, `off-host-claim`) — a joiner with an ordered seed list joins when the first seed is down and the second is live

Reading: `not-observed` — no committed run record
- `RC-4/failover/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-4 --arm failover` and commit its record; the arm's harness lands in `rc-failover`.
- Next PR: `rc-failover`

**Arm `returner`** (local, `local-devnet`) — a validator down across a bond-change boundary rejoins the same finalised head without manual surgery

Reading: `not-observed` — no committed run record
- `RC-4/returner/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-4 --arm returner` and commit its record; the arm's harness lands in `rc-bond-leave`.
- Next PR: `rc-bond-leave`

**Supersedes.** G584-G4, TA-C2, G428-M4

**Does not claim.** A store loss needs a fresh data dir and a checkpoint join; 'no operator action' covers a verified restart, not a lost store. The returner arm covers a validator that missed a boundary, not one that missed the bond change entirely.

---

## RC-5 — Overload refusal

**Claim reading.** `not-observed` (the worst required arm).

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

**Arm `local`** (local, `local-devnet`) — capped and unset pressure on the local node

Reading: `safe` — spec/audit/evidence/rc5-local-20261122-5a0e755/raw/claim.json
- No action: the reading is safe; carry the record forward.

**Arm `public`** (public, `public-live`) — pressure on the public endpoint and proxy

Reading: `not-observed` — no committed run record
- `RC-5/public/observe` — no committed run record. **Next:** Supply the capped/unset pressure record from the public endpoint and proxy.
- Next PR: `rc-public-fleet`

**Arm `transport`** (local, `local-devnet`) — one deploy id through HTTP, direct HTTP, gRPC and a proxy returns the same id, status, settlement stage and error class

Reading: `not-observed` — no committed run record
- `RC-5/transport/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-5 --arm transport` and commit its record; the arm's harness lands in `rc-transport`.
- Next PR: `rc-transport`

**Supersedes.** G428-M2

**Does not claim.** The opt-in in-flight cap is not the shipping default; the pool's count/byte/term/share/rate bounds are the intended throttle. The transport arm proves id, status, settlement stage and error-class equivalence across the four surfaces; it does not claim byte-identical payloads or gas accounting.

---

## RC-6 — Join and audit

**Claim reading.** `not-observed` (the worst required arm).

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

**Arm `local`** (local, `local-devnet`) — genesis join and audit on the local devnet

Reading: `safe` — spec/audit/evidence/rc6-local-20261122-5a0e755/raw/claim.json
- No action: the reading is safe; carry the record forward.

**Arm `public`** (public, `public-live`) — a public join and audit

Reading: `not-observed` — no committed run record
- `RC-6/public/observe` — no committed run record. **Next:** Supply the operator's completed public join record.
- Next PR: `rc-public-fleet`

**Arm `off-host`** (local, `off-host-claim`) — host B joins host A through a routable advertised address with the six-stage chain, and a loopback advertisement is refused

Reading: `not-observed` — no committed run record
- `RC-6/off-host/observe` — no committed run record. **Next:** Run `python3 tools/release-check.py run --claim RC-6 --arm off-host` and commit its record; the arm's harness lands in `rc-offhost`.
- Next PR: `rc-offhost`

**Arm `availability`** (public, `public-live`) — a joiner bootstraps from a non-producer, restores the served anchor and audits it

Reading: `not-observed` — no committed run record
- `RC-6/availability/observe` — no committed run record. **Next:** Bootstrap a joiner from a non-producer on the public fleet, restore the served anchor and audit it.
- Next PR: `rc-public-fleet`

**Supersedes.** G428-M3

**Does not claim.** The join half proves the six-stage chain for one routable advertised address; the off-host arm is that address and a loopback negative, not every transport. Alternative-peer failover is RC-4's arm; the audit half does not claim a replay of a chain the audited node never held.

---

## RC-7 — Upgrade compatibility

**Claim reading.** `not-observed` (the worst required arm).

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

**Arm `local`** (local, `local-devnet`) — mixed N/N+1 binaries agree on a block's state, a populated store migrates, and a newer store is refused

Reading: `safe` — spec/audit/evidence/rc7-local-20261122-8c3fa6d/raw/claim.json
- No action: the reading is safe; carry the record forward.

**Arm `public`** (public, `public-live`) — a rolling fleet swap keeps one finalised chain through the mixed window and reaches the same head after the swap

Reading: `not-observed` — no committed run record
- `RC-7/public/observe` — no committed run record. **Next:** Record the rolling fleet swap's mixed-window block agreement and the post-swap finalised head.
- Next PR: `rc-fleet-upgrade`

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
| `G584-G1` | retired as a closing criterion; a public campaign precondition | release-claims.json RC-1/public and RC-4/public (the manifest each run must record) |
| `G584-G2` | retired as a closing criterion; claim RC-1/public | release-claims.json RC-1 |
| `G584-G3` | retired as a closing criterion; claim RC-1/public | release-claims.json RC-1 |
| `G584-G4` | retired as a closing criterion; claim RC-4/public | release-claims.json RC-4 |
| `G584-G5` | retired as a closing criterion; the evidence rule | tools/evidence-pack.py and docs/src/spec/gate-measurement.md |
| `G584-G6` | retired; the claim reading is the verdict, not a named reviewer | tools/release-check.py --assert-ready and release-claims.json RC-1/RC-4 |
| `TA-C1` | claims RC-1 and RC-2; attestation is an evidence rule | release-claims.json RC-1/RC-2 |
| `TA-C2` | claims RC-1/RC-2/RC-4 bond-leave arms | release-claims.json RC-1 bond-leave, RC-2 validator-set, RC-4 returner |

## Formerly out of scope — now arms

| behaviour | covering arms |
|---|---|
| Off-host routable transport and the loopback refusal (M3). | `RC-6/off-host` |
| Alternative-peer failover (M3-11). | `RC-4/failover` |
| Host loss and the multi-host N=4/N=8 envelope (M4). | `RC-2/multi-host`, `RC-4/multi-host` |
| Bond and leave (TA-C2). | `RC-1/bond-leave`, `RC-2/validator-set`, `RC-4/returner` |
| Transport correctness across HTTP, direct HTTP, gRPC and a proxy. | `RC-5/transport` |
| Finalised block body and state availability on every validator. | `RC-3/availability`, `RC-6/availability` |
| Equivocation injection under a validator key. | `RC-2/equivocation` |

`check-release-campaigns.py --assert-ready` exits 3 unless every required arm reads
`safe`; every open reading names its next action and PR. A closed register is still not a
release authorization.
