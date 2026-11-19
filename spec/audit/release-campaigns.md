# Release claims

Emitted from `spec/audit/release-claims.json` by `tools/build-mainnet-gates.py`.

**These are not gates.** A gate is a boolean test of the current build. A release claim
is one observable node behaviour with one falsifier and one control, re-runnable on the
standard local devnet or on `testnet-4`. The 12 old campaign rows are deleted; their fate
is in the `superseded` table at the end.

- generated: 2026-11-19
- claims: 7
- review: `spec/audit/release-campaign-red-team-20261119.md`

| claim | status | local | public | owner |
|---|---|---|---|---|
| `RC-1` — Truthful settlement | **owed** | local-4 and local-8 (tools/devnet.sh; 4 GiB / 2 GiB per validator, bel | testnet-4: four independent hosts on the selected release | Aria / operator |
| `RC-2` — Safety | **owed** | local-4 and local-8 (tools/devnet.sh) with a producing victim | testnet-4 with a real partition and the operator window | Jim / release coordination |
| `RC-3` — State agreement across a merge | **owed** | local-4 and local-8 (tools/devnet.sh) | testnet-4 on the selected release | Jim / release coordination |
| `RC-4` — Recovery without loss | **owed** | local-4 (tools/devnet.sh; tests/consensus-restart-under-load.py) | testnet-4 with a real host stop | Jim / release coordination |
| `RC-5` — Overload refusal | **open** | local-4 (one node, direct listener and in-process gRPC) | testnet-4 public endpoint and proxy | Jim / release coordination |
| `RC-6` — Join and audit | **owed** | local-4 (checkpoint join and genesis-only audit) | testnet-4 with a public join | Aria / evidence work |
| `RC-7` — Upgrade compatibility | **owed** | local-4 (N and N+1 binaries); the G520 populated-store drill | testnet-4 fleet | Jim / release coordination |

---

## RC-1 — Truthful settlement

**Claim.** If the settlement endpoint reports finalizedSuccess and settlementConfirmed for a deploy, that deploy's effect is present at the finalised head on every validator and remains present through two further epoch boundaries.

**Pass.** For one deploy id: the settlement response is finalizedSuccess with settlementConfirmed true; every validator reports the same finalised (height, hash); a marker written by the deploy reads back at that finalised block and at every later finalised head sampled; the inclusion block never appears in a later block's rejectedDeploys.

**Fail.** A confirmed settlement whose effect is absent at the finalised head on any validator; validators disagree on the finalised hash; or the inclusion block appears in a later rejectedDeploys.

**Observe.** The settlement response, each validator's finalised (height, hash), the marker read at that block, and the per-block rejectedDeploys.

**Control.** A marker absent before the write; a fixture where the effect is absent at the finalised head; a fixture where the inclusion block is final-scope rejected.

**Local.** local-4 and local-8 (tools/devnet.sh; 4 GiB / 2 GiB per validator, below the documented tier)

**Public.** testnet-4: four independent hosts on the selected release

**Re-run (local).** `tools/devnet.sh up --validators 4 --fresh; tools/consensus-acceptance-drill.py --local; settlement endpoint sample; finalised-head marker read on all validators`

**Re-run (public).** `same observation on testnet-4 with the selected pins and the operator endpoints`

**Tool.** tools/live-gate.py::check_g3 (settlement half) + settlement-retention observation (owed)

- `RC-1a` no false settlement — **open** — tools/live-gate.py::check_g3
- `RC-1b` finalised effect retained — **owed** — The settlement route trusts block finality; a per-validator finalised-head read and the rejectedDeploys invariant are not implemented. This is the check-1 defect.

**Supersedes.** G584-G2, G584-G3, TA-C1

**Does not claim.** A local pass is a mechanism check; the public claim needs testnet-4.

---

## RC-2 — Safety

**Claim.** No two conflicting finalised histories: every finalised block held by one validator is finalised by every validator at or above its height, and no finalised (height, hash) regresses across restart, replay or partition.

**Pass.** For every sampled finalised block, every validator at or above its height reports it finalised; no finalised (height, hash) regresses; no two conflicting finalised histories are observed.

**Fail.** A finalised block not finalised by another validator at or above its height; a finalised regression; or two conflicting finalised histories.

**Observe.** Per-validator finalised heads at each sample, the cross-node containment answers, and the finalised history over the window.

**Control.** A producing split (the current harness victim is non-producing); an equivocation injection under the allowed Byzantine bound.

**Local.** local-4 and local-8 (tools/devnet.sh) with a producing victim

**Public.** testnet-4 with a real partition and the operator window

**Re-run (local).** `tools/devnet.sh up --validators 4 --fresh; tests/consensus-partition-rejoin.py --local`

**Re-run (public).** `same observation on testnet-4 with independent hosts`

**Tool.** tests/consensus-partition-rejoin.py (containment assertion) + a criterion that consumes it (owed)

- `RC-2a` containment and no regression — **owed** — The harness asserts cross-node containment; the campaign criterion reads only finalized_anchor booleans. The criterion must consume the containment evidence, and the victim must be producing.

**Supersedes.** G428-M4, TA-C1

**Does not claim.** The claim assumes the <=1/3 Byzantine/equivocation bound; without it an equivocator can back two quorums.

---

## RC-3 — State agreement across a merge

**Claim.** For the same accepted block on the same binary, every validator derives the same preStateHash and postStateHash and the same kept/rejected ledger; kept tuple-space effects are present and rejected effects absent.

**Pass.** Across a defined window, every validator reports the same preStateHash, postStateHash and rejectedDeploys for every accepted block, and a kept marker is readable at the same block on every validator.

**Fail.** Any preStateHash, postStateHash or rejectedDeploys disagreement for the same block; a same-block marker present on one validator and absent on another.

**Observe.** The per-block preStateHash, postStateHash and rejectedDeploys from every validator, plus a same-block marker read.

**Control.** A fixture where the ledger differs; a native-write case (native absolute writes are excluded from FinalizedMerge.lean's unqualified law).

**Local.** local-4 and local-8 (tools/devnet.sh)

**Public.** testnet-4 on the selected release

**Re-run (local).** `compare every validator's preStateHash/postStateHash/rejectedDeploys for the same accepted block over a window`

**Re-run (public).** `same comparison on testnet-4`

**Tool.** state-agreement observation (owed)

- `RC-3a` same state and ledger — **owed** — No cross-validator live checker exists. The passing merge_determinism tests are fixtures and arrival orders, not a cross-validator run.

**Supersedes.** G428-M4

**Does not claim.** The claim is conditional on the same binary and conflict scope; block-merge.md records that two binaries can reject different chains at a merge.

---

## RC-4 — Recovery without loss

**Claim.** After one validator is stopped and restarted, or a partition heals, the survivors keep finalising; every deploy accepted by a survivor is resolved and its effect is in the post-recovery finalised state; the victim rejoins the same finalised head; no spurious self-replay or own-key halt fires.

**Pass.** With one validator stopped, the survivors finalise past the kill; every deploy submitted to a survivor resolves and its effect is present at the post-recovery finalised head; after restart the victim reports the same finalised (height, hash) at least three heights above the baseline; no finalised height regresses and no fixed height changes hash; no own-key halt or self-replay is observed.

**Fail.** Finality stalls; accepted work is lost; the rejoiner needs manual surgery; a finalised height regresses; or a spurious halt fires.

**Observe.** The survivors' finalised heads, every accepted deploy's resolution and effect at the post-recovery finalised head, the victim's boot ID and finalised head, and the own-key/self-replay counters.

**Control.** The wrong-genesis and interrupted-restart arms; a deploy accepted only by the killed validator is not recoverable by design and must be declared, not counted.

**Local.** local-4 (tools/devnet.sh; tests/consensus-restart-under-load.py)

**Public.** testnet-4 with a real host stop

**Re-run (local).** `tools/devnet.sh up --validators 4 --fresh; tests/consensus-restart-under-load.py --validators 4`

**Re-run (public).** `same observation on testnet-4 with an operator-approved host stop`

**Tool.** tests/consensus-restart-under-load.py + a finalised-effect read (owed)

- `RC-4a` finality and agreement after restart — **open** — tests/consensus-restart-under-load.py
- `RC-4b` accepted deploy effect at the post-recovery finalised head — **owed** — The harness checks ProcessedWithSuccess, which admission.md says is not settlement. A marker at the post-recovery finalised head on every survivor and the victim is not read.

**Supersedes.** G584-G4, TA-C2, G428-M4

**Does not claim.** A store loss needs a fresh data dir and a checkpoint join; 'no operator action' covers a verified restart, not a lost store.

---

## RC-5 — Overload refusal

**Claim.** At the configured admission/pool bound, the node refuses new work with HTTP 429 and Retry-After: 1, or gRPC RESOURCE_EXHAUSTED (ingress-rate and pool-bound classes stated separately), returns no 5xx, stays alive, and loses no deploy it accepted.

**Pass.** With the bound configured: every permit is held, further probes are refused with the named class and retry signal, no 5xx is returned, the unset/default control admits, the handler is reachable after release, and every accepted deploy id is still readable or explicitly failed.

**Fail.** A 5xx under pressure; an accepted deploy that later disappears; an unbounded queue; an unset default that refuses; or a refusal class outside the vocabulary.

**Observe.** Accepted/refused counts, refusal class and retry signal, handler reachability after release, and the status of every accepted deploy id.

**Control.** The unset-default arm must admit; the four named queue bounds; a refusal class outside the vocabulary.

**Local.** local-4 (one node, direct listener and in-process gRPC)

**Public.** testnet-4 public endpoint and proxy

**Re-run (local).** `tools/m2-pressure-arm.py phase --mode capped/--mode unset; tools/m2-pressure.py check --locality local-devnet`

**Re-run (public).** `same observation on testnet-4 with a public endpoint and no proxy`

**Tool.** tools/m2-pressure.py::check_pressure

- `RC-5a` configured bound refuses explicitly, no 5xx, no accepted loss — **open** — tools/m2-pressure.py::check_pressure

**Supersedes.** G428-M2

**Does not claim.** The opt-in in-flight cap is not the shipping default; the pool's count/byte/term/share/rate bounds are the intended throttle. Transport correctness across HTTP, direct HTTP, gRPC and a proxy is a separate owed observation.

---

## RC-6 — Join and audit

**Claim.** A fresh node joining from a bootstrap receives the certified anchor, applies and hydrates it, and reaches the same finalised head and state; rnode audit reproduces the checkpoint claim from genesis.

**Pass.** The joiner's served, received, restored and walked hashes agree for the certified anchor (equality with the served root is required only for a genesis anchor); a legacy certificate with no commitments is refused; rnode audit returns AGREE, its audited digest equals the accepted digest, and its derived root equals the checkpoint claim.

**Fail.** The joiner receives but never applies; the restored state differs from the certified anchor; a certificate with no commitments is accepted; audit returns DISAGREE, a different digest, or a derivation over a different checkpoint.

**Observe.** The six-stage join chain, the certified anchor's post-state root and fringeState, and the audit verdict and digest.

**Control.** A send-log-only chain; a served/restored hash mismatch; a legacy certificate with no commitments.

**Local.** local-4 (checkpoint join and genesis-only audit)

**Public.** testnet-4 with a public join

**Re-run (local).** `tools/devnet.sh up --validators 4 --fresh; tools/m3-join-chain.py build/check; rnode audit`

**Re-run (public).** `same observation on testnet-4`

**Tool.** tools/m3-join-chain.py::check_chain + rnode audit (wrapper owed)

- `RC-6a` join, apply and hydrate — **open** — tools/m3-join-chain.py::check_chain
- `RC-6b` audit reproduces the checkpoint — **owed** — rnode audit exists; a criterion binding AGREE and the digest equality to the joiner's accepted digest is not written.

**Supersedes.** G428-M3

**Does not claim.** A routable address is setup, not the claim; off-host transport and the loopback refusal are separate owed observations. Alternative-peer failover (M3-11) is separate and never landed.

---

## RC-7 — Upgrade compatibility

**Claim.** A mixed-version fleet either keeps one finalised chain and one state for a block produced during the window, or refuses on the consensus path; a populated store migrates; a downgrade is refused.

**Pass.** During a mixed-version window, a block is produced and finalised with both binaries deriving the same postStateHash and rejectedDeploys, or the new binary refuses the old peer; a populated-store copy/rollback/restart drill passes; an old binary refuses a newer store marker.

**Fail.** Two versions finalise different blocks or derive different post-state hashes; the old binary runs on a newer store; or the populated-store drill fails.

**Observe.** The mixed-version block's hashes and rejection ledger on both binaries, the store-marker refusal, and the populated-store drill verdict.

**Control.** A fresh-genesis run must not substitute for the populated-store arm; a legacy/no-commitment certificate must be refused.

**Local.** local-4 (N and N+1 binaries); the G520 populated-store drill

**Public.** testnet-4 fleet

**Re-run (local).** `tools/populated-store-migration-drill.py --gate; mixed-version devnet run`

**Re-run (public).** `same observation on testnet-4`

**Tool.** tools/populated-store-migration-drill.py::run_gate (migration half) + mixed-version observation (owed)

- `RC-7a` populated-store migration and downgrade refusal — **open** — tools/populated-store-migration-drill.py::run_gate
- `RC-7b` mixed-version fork boundary — **owed** — No mixed-version devnet run or criterion exists; block-merge.md records that two binaries can reject different chains at a merge.

**Does not claim.** For a fresh-genesis launch this is post-launch; it becomes a launch blocker only over a populated store or a mixed fleet.

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
| `TA-C2` | claim RC-4; bond/leave are separate owed observations | release-claims.json RC-4 |

## Missing observations

- Off-host routable transport and the loopback refusal (M3).
- Alternative-peer failover (M3-11).
- Host loss and the multi-host N=4/N=8 envelope (M4).
- Bond and leave (TA-C2).
- Transport correctness across HTTP, direct HTTP, gRPC and a proxy.
- Finalised block body and state availability on every validator.
- Equivocation injection under a validator key.

`check-release-campaigns.py --assert-ready` exits 3 while any claim or sub-observation
is not closed, so a closed gate register is never a release authorization.
