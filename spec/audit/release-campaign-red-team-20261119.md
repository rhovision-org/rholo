# Release campaign red-team review — 2026-11-19

**Review only. No campaign is closed, no gate is promoted, and no release is authorized.** This
document attacks the 12 remaining release items, the five local checks proposed in
`release-checks-20261119.md`, and the failed local check 1. It separates what is a product claim
from laboratory setup, sizing guidance and evidence rules, and it records what the node is
designed to do. It does not fix anything.

## Method and authority

Four independent reviewers attacked the G428 family, the G584 family, TA/C343, and the cross-cutting
mainnet risks. A challenger then attacked every verdict. The authority is the node's own design —
`spec/Rchain/**/*.lean`, `docs/src/node/*.md`, the Rust, and the passing integration tests. The
campaign prose is not authority. The standard is the one the 47 passing gates use: **one observable,
one named falsifier, one named control.**

The review asks of every item: what is measured, what user-visible failure it prevents, what the
design says, what would prove it false, whether PASS means the node works or the lab is arranged,
what can still go wrong after a PASS, whether it is already covered, and whether the evidence is
current.

## The evidence finding comes first

### The check-1 record overstates its own evidence

The packaged record `spec/audit/evidence/local-check1-finality-20261119/` says the marker was absent
at finalised height **46**, hash `9b6f8b58`, on **all four** validators, and that
`/api/is-finalized/33507f7a` returned true on all four. The raw record does not support those
claims:

- The drill failed at its pre-registered target, finalised height **34**, block `be511bec`. The
  health envelope and the last observation both say 34.
- At height 34 the marker was read on **validator 0 only**. The harness raises on the first failing
  validator (`tools/consensus-acceptance-drill.py:351-372`), so validators 1–3 were never probed at
  that height.
- `9b6f8b58` appears **nowhere** in the raw record. The height-46 and all-four statements came from
  manual queries made after the run and were never packaged.
- The harness's `--local` mode does not exist at the record's source pin `91cde8190`; it was added
  in `fc757f9c9`. The record cannot reproduce itself at its own pin, and the observations lack the
  deploy-status records the later harness writes.

**This is a record defect, and it is exactly the overclaiming this review exists to catch.** The
README is superseded by the raw evidence below and must be corrected before the record is cited.

### The raw finding still stands

On the current node binary, the raw record shows a user-visible failure:

- The marker deploy `@"acceptance:5b5487bf…"!(42)` was written in block `33507f7a` at height 13.
- The marker read back as `42` at the finalised block `c6f8f074` at height 14 on **all four**
  validators. Block `c6f8f074` carries only a `Nil` deploy, so the `42` came from the base — the
  height-13 state.
- By the finalised head `be511bec` at height 34, the marker read back as `[]` on **validator 0**.
  Validators 1–3 were not probed.
- All four node logs name the deploy signature `304402202bfefb49…` as riding a **final-scope
  rejected chain**: "2 deploy(s) rode on a final-scope rejected chain". Finaliser block `43addb16`
  at height 17 records both the deploy id and block `33507f7a…02` in `rejectedDeploys`.

The node's design says the opposite of what the user saw: `spec/AUDIT.md` C260 says "**No write is
dropped**", `spec/Rchain/Casper/FinalizedMerge.lean` says a finalised base write survives unless a
kept chain removes it, and `docs/src/node/admission.md:136-140` says an included deploy is not
silently lost. The settlement endpoint is the user surface: by code, it reports
`settlementConfirmed: true` when `is_finalized(inclusion_block)` is true
(`node/src/web/http.rs:1557,1593`). **The raw run did not sample that endpoint.** A manual post-run
probe found `is_finalized(33507f7a)` true, but that probe is not packaged, so the record cannot
establish that a user would have been told "settled". A re-run must sample the settlement endpoint.

**Two readings are possible and the raw record cannot choose between them:** the merge unapplied a
write that was in the base (a Law 113 violation), or a later base state was rebuilt without it.
Both are release blockers. The record carries no `fringe_state_hash` lineage, so the specific
mechanism must be established by a re-run. C260's premise that "honest siblings cannot produce" a
final-scope rejection of a finalised block is refuted by this run and must be reopened or re-scoped.

**Required before the record is reused:** a committed harness, a probe of the late head on every
validator at a fixed `(height, hash)`, a settlement-endpoint sample, and fringe-state lineage.
**Provenance is current for the node:** source `91cde8190` is the dev tip at run time, binary
`d664fc03…`, image `32f28ab5…`; the only later commit is this review's docs/tests/evidence commit,
with no Rust change. The run is not from an older node.

## The 12 items: verdicts

| item | verdict | reason and new home |
|---|---|---|
| **C343** | **DEMOTE** | The live B/C/D snapshot is an operator incident diagnosis, not a release claim. The shipped `/health` and `/api/v1/status/operator` contracts stay in the current-build gate (`node/src/diagnostics/health_contract.rs`, the operator-key tests, `tools/check-node-diagnostics.py --gate`). The live snapshot is an open precondition; the installed build's 4xx is a NO-GO for that host, not a criterion. |
| **G428-M1** | **DEMOTE** | RAM/CPU/disk/peak/replay is sizing guidance; the docs already call it an estimate with no production benchmark. The host-failure half is **MISSING** until a multi-host driver exists: one Docker host cannot exercise one-host loss. |
| **G428-M2** | **REWRITE** | The claim is overload refusal at the **configured admission/pool bound**, not the opt-in malformed-JSON cap. State the two gRPC refusal classes separately (ingress-rate has no `retry-after`; the pool bound does), keep the unset-default control, and observe that no accepted deploy disappears. |
| **G428-M3** | **REWRITE** | The claim is join and apply, not "advertise a public address". For a certified checkpoint the accepted root differs by design; equality is required only for a genesis anchor. Off-host routable delivery and the loopback refusal are a separate **MISSING** live item; alternative-peer failover (M3-11) is another. |
| **G428-M4** | **REWRITE** | The claim is containment and no finalised regression, not "no two finalised blocks at any height" (honest siblings are allowed). The harness's victim is non-producing and the criterion reads only `finalized_anchor` booleans. A producing split or equivocator arm, host loss and the multi-host driver are **MISSING**. |
| **G584-G1** | **DEMOTE** | Setup precondition: one selected source/artifact/binary, four distinct identities, pinned genesis. It gates every other claim; it is not a node behaviour. |
| **G584-G2** | **REWRITE** | Lifecycle: one signed deploy is pooled, included/executed, and `finalizedSuccess` on every validator; accepted-but-never-finalised is **FAIL**, not PRECONDITION. Keep the signed native-REV subject; native absolute writes are excluded from `FinalizedMerge.lean` and must be observed separately. |
| **G584-G3** | **REWRITE** | Keep the false-positive half only: before finality no surface settles; `acceptsDeploys` is not settlement; the finality endpoint agrees with the four observations. The retention half moves to claim 1. |
| **G584-G4** | **REWRITE** | Use the restart harness, but it checks `ProcessedWithSuccess`, which `admission.md:116` says is not settlement. Add a finalised-effect read at the post-restart head on every survivor and the victim; scope "accepted" to deploys submitted to a survivor or included in a surviving block. |
| **G584-G5** | **DEMOTE** | An evidence rule: raw responses, body digests, four endpoint records, manifest and `SHA256SUMS`. Keep the pin-binding and single-route negative controls; without them a fabricated run directory passes. |
| **TA-C1** | **REWRITE** | The criterion checks the attestation's shape, not the claim; its own fixture passes on a C343 record. Merge the claim into claim 1, preserve the A1.1/A1.2/A1.4/A1.5 and A1.3 observations, and make the reviewer attestation an evidence rule. |
| **TA-C2** | **REWRITE** | Same shape-only defect. Merge the survival/rejoin half into claim 4. Bond and leave are user-visible and **MISSING** as separate observations. "No operator action" is true for a restart with a verified certificate, not for a store loss (`sync-operations.md:62-76`). |

## The proposed clean set

Each entry is one claim, one falsifier, one control, and the surfaces that must be read. The set is
small on purpose; anything that is setup, sizing or recording moves to the notes after it.

### Claim 1 — Truthful settlement

**Claim:** when `GET /api/v1/deploy-settlement/{id}` returns `stage: finalizedSuccess` and
`settlementConfirmed: true`, the deploy's inclusion block is finalised on every validator, and the
deploy's effect is readable at that block and at every later finalised head on every validator.

**Failure prevented:** a wallet is told a transfer settled, then the effect is absent (the check-1
class); or the endpoint confirms a block the merge later rejects.

**Design:** `docs/src/node/admission.md:107-117,136-144`; the settlement route
`node/src/web/http.rs:1511-1610`; `spec/Rchain/Casper/FinalizedMerge.lean` (tuple-space only);
`spec/AUDIT.md` C260, which this run refutes and which must be reopened.

**Falsifier:** the same deploy id has a confirmed settlement, but a marker read at the finalised
`(height, hash)` is absent on any validator; validators disagree on the finalised hash; or the
inclusion block appears in a later block's final-scope rejection set. The last invariant — **a
finalised inclusion block must not appear in a later `rejectedDeploys`** — is in none of the 12 and
is the exact check-1 defect.

**Control:** a unique marker, an absent-marker read before the write, and a failed fixture.

**Environment:** `local-4`/`local-8` for the mechanism; `testnet-4` for the release claim.

**Current status:** fails the raw check-1 finding on validator 0 at height 34; an all-validator
re-run and a settlement sample are required.

### Claim 2 — Safety

**Claim:** no two conflicting finalised histories. Every finalised block held by one validator is
finalised by every validator at or above its height, and no finalised `(height, hash)` regresses
across restart, replay or partition.

**Failure prevented:** a double-spend or an irreversible split.

**Design:** `docs/src/node/consensus.md:36-45,53-62,88-92`; `spec/Rchain/Casper/Liveness.lean`
Law 52; the met mechanism gates for the live weight set, equivocation refusal and full-partition
containment. The claim assumes the ≤1/3 Byzantine/equivocation bound; without it an equivocator
with more than 1/3 stake can back two quorums.

**Falsifier:** a finalised block not finalised by another validator at or above its height; a
finalised regression; two conflicting finalised histories.

**Control:** a producing split (the current harness's victim is non-producing, so the hazard is
excluded by construction) and an equivocation injection.

**Environment:** `local-4`/`local-8` mechanism with a producing victim; `testnet-4` for a real
partition.

### Claim 3 — State agreement across a merge

**Claim:** for the same accepted block on the same binary, every validator derives the same
`preStateHash` and `postStateHash` and the same kept/rejected ledger; kept tuple-space effects are
present and rejected effects absent. Native absolute writes are excluded from
`FinalizedMerge.lean`'s unqualified law and need their own guard.

**Failure prevented:** validators derive different state; a write is duplicated or lost; finality
stalls on a merge disagreement.

**Design:** `docs/src/node/block-merge.md:60-78,95-141`; `MergeAccounting.lean`; Law 9/10/17c;
`FinalizedMerge.lean:1-22` for the native-write exclusion. Note `block-merge.md:111-113`: two
binaries can reject different chains at a merge; that is a hard-fork hazard, not a solved property.

**Falsifier:** any `postStateHash` or `rejectedDeploys` disagreement across validators for the same
block; a same-block marker present on one validator and absent on another.

**Control:** a fixture where the ledger differs; a native-write case.

**Environment:** `local-4`/`local-8`. The passing `merge_determinism` tests are fixtures and
arrival orders, not a cross-validator live check.

### Claim 4 — Recovery without loss

**Claim:** after one validator is stopped and restarted, or a partition heals, the survivors keep
finalising; every deploy accepted by a survivor is resolved and its effect is in the post-recovery
finalised state; the victim rejoins the same finalised head; no spurious self-replay or own-key
halt fires.

**Failure prevented:** finality stalls, accepted work is lost, the rejoiner needs manual surgery, or
a spurious halt fires.

**Design:** `docs/src/node/admission.md:136-144`; `spec/LAWS.md` Law 89; the restart harness
`tests/consensus-restart-under-load.py` and the partition harness.

**Falsifier:** an accepted deploy unresolved or absent at the post-recovery finalised head; no new
boot ID; a finalised regression; an own-key halt or self-replay.

**Scope:** "accepted" means submitted to a surviving validator or included in a surviving block. A
deploy that existed only in the killed validator's local pool is not recoverable by design. A store
loss needs a fresh data dir and a checkpoint join (`sync-operations.md:62-76`); "no operator action"
does not cover it.

**Control:** the wrong-genesis and interrupted-restart arms; a producing partition with a
pre-registered resumption bound.

**Environment:** `local-4`; `testnet-4` for host loss.

### Claim 5 — Overload refusal

**Claim:** at the configured admission/pool bound, the node refuses new work with HTTP 429 and
`Retry-After: 1`, or gRPC `RESOURCE_EXHAUSTED` (the ingress-rate and pool-bound classes stated
separately), returns no 5xx, stays alive, and loses no deploy it accepted.

**Failure prevented:** silent accepted-write loss, unbounded queueing, ambiguous acceptance, 5xx
under pressure, or OOM.

**Design:** `docs/src/node/admission.md:13-59`; `spec/Rchain/Admission.lean`; the pool bounds in
`casper/src/pool.rs`; the refusal mapping in `node/src/web/http.rs:2599-2631`.

**Falsifier:** a 5xx; an accepted deploy that later disappears; an unbounded queue; an unset default
that refuses; a refusal class outside the vocabulary.

**Control:** the unset-default arm (it must admit), and the four named queue bounds.

**Environment:** `local-4` with the direct listener and the in-process gRPC bound; `testnet-4` for
the public endpoint and proxy. Transport correctness across HTTP, direct HTTP, gRPC and a proxy is
**MISSING**.

### Claim 6 — Join and audit

Two observations, because they fail differently:

**6a Join.** A fresh node joining from a bootstrap receives the certified anchor, applies and
hydrates it, and reaches the same finalised head and state. For a certified checkpoint the accepted
root is the certified anchor's post-state root and `fringeState`; equality with the served root is
required only for a genesis anchor. A legacy certificate with no commitments must be refused.
**Falsifier:** the joiner receives but never applies; the restored state differs from the certified
anchor; a certificate with no commitments is accepted.

**6b Audit.** `rnode audit` reproduces the checkpoint claim from genesis: the verdict is `AGREE`,
the audited digest equals the joiner's accepted digest, and the derived root equals the checkpoint
claim. **Falsifier:** `DISAGREE`, a different digest, or a derivation over a different checkpoint.

**Design:** `docs/src/node/sync-and-history.md:3-5,17-20,27-47`;
`docs/src/node/sync-operations.md:16-62,80-91`.

**Environment:** `local-4` (checkpoint join, genesis-only audit); `testnet-4` for public join.

### Claim 7 — Upgrade compatibility (conditional)

**Claim:** a mixed-version fleet either keeps one finalised chain and one state for a block produced
during the window (both binaries derive the same hashes and rejection ledger), or refuses on the
consensus path; a populated store migrates; a downgrade is refused.

**Failure prevented:** a rolling upgrade splits the chain or corrupts the store; an old binary runs
on a newer store.

**Design:** `docs/src/node/updating.md:12-24,109-122,139-144`; `block-merge.md:111-113`;
`sync-operations.md:13-14`; `G520-MIGRATION`.

**Falsifier:** two versions finalise different blocks or derive different post-state hashes; the old
binary runs on a newer store; a populated-store drill fails.

**Condition:** for a fresh-genesis launch this is **post-launch**, because every host is already on
the selected build (the G1 precondition). It is a **launch blocker** only if the release is
installed over a populated store or a mixed fleet. State the condition; it cannot be both.

**Environment:** `local-4` mechanism (N and N+1); `testnet-4` for the fleet.

## What is demoted, and where it goes

| item | new home |
|---|---|
| C343 live B/C/D snapshot | `runbooks/c343-live-readonly-20261119.md`; the diagnostics contract stays in the current-build gate |
| G428-M1 tier measurement | `docs/src/node/validator-requirements.md` and the M1 runbook; host-failure is a missing multi-host arm under safety/recovery |
| G584-G1 installed pins | standard setup precondition before every live claim |
| G584-G5 observation record | evidence standard (`tools/evidence-pack.py`, `docs/src/spec/gate-measurement.md`) |
| TA-C1/C2 reviewer attestation | evidence rule on the record; it names who saw which bytes, it does not make a claim pass |

## What is missing

These are user-visible and are not in the seven claims:

1. **Off-host routable transport and the loopback refusal** (M3). The node accepts any bootstrap
   URI; the off-host claim needs a real second host, a routable address and a delivered dial-back,
   and a `127.0.0.1` advertised from another host must fail clearly.
2. **Alternative-peer failover (M3-11).** Never landed and default-off; a node whose first peer is
   unavailable must use another.
3. **Host loss and the multi-host N=4/N=8 envelope** (M4). One Docker host cannot lose a physical
   host; the multi-host driver is owed.
4. **Bond and leave** (TA-C2). A new validator bonds and produces; a leaving validator is paid out
   after quarantine. Both are user-visible and separate from the restart claim.
5. **Transport correctness.** The same deploy through HTTP, direct HTTP, gRPC and a proxy must give
   the same id, status, settlement stage and error classes.
6. **Finalised block body and state availability.** Every validator must be able to serve every
   finalised body and a state read at the finalised head; `Block not yet available` must not hide a
   missing finalised block.
7. **Equivocation injection under a validator key.** Two signed blocks at one height must not
   produce conflicting finality; a fault must surface.
8. **Populated-store migration and downgrade refusal** (claim 7b), if claim 7 is in scope.
9. **Audit reproducibility** (claim 6b), if it is kept separate from join.

## Provenance

| evidence | source | binary | image | current? |
|---|---|---|---|---|
| local check 1 (raw) | `91cde8190` (dev tip at run; later commit is docs/tests only, no Rust) | `d664fc03…` | `32f28ab5…` | **Node: current. Record: stale/incorrect** — the `--local` harness is not at the pin; the README's height 46/all-four/is-finalized claims are not in the raw record. |
| the 12 campaign rows | no live run | none | none | **Stale/absent.** Evidence blocks are null; the rehearsal is `4e097cc8…`/`a353225e…`, `local-devnet`. |
| the five local-check proposals | no committed run | none | none | **Not evidence.** Check 1 is the only run, and its record needs correction and an all-validator re-run. |
| M2/M3 local rehearsals | `46c98b67…` | `d98c9a13…` | none | **Stale**, `local-devnet`, INCONCLUSIVE. |

## What this review does not claim

- No campaign is closed, no gate is promoted, and no release is authorized.
- The review does not fix the check-1 defect, and it does not decide whether the merge unapplied a
  base write or a later base was rebuilt without it. It records the user-visible failure and the
  evidence defect; the mechanism needs a re-run with lineage.
- The raw check-1 finding is one run, on validator 0's late head. It is not an all-validator result
  and not a theorem.
- The clean set is a proposal. A claim is not passed until its observation runs on the current
  binary and the record is packaged with pins.
- A well-formed attestation is not a measurement. A local devnet is not a public testnet. A closed
  gate register is not release authorization.
- No code change, no testnet run, no public deployment, no key, no transfer, and no announcement
  decision.
