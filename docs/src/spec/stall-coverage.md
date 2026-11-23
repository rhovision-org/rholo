# Deadly embrace: representability, shape and root cause (HAZOP)

**What this page is.** A HAZOP study of one failure mode — a refusal that stops block production and is
not undone by any named safeguard. It is **not a release claim** and it carries no reading.

There are three surfaces, and this page is only the middle one:

- **The release-claim (RC) surface.** Seven environment-typed claims in
  [`spec/audit/release-claims.json`](../../../spec/audit/release-claims.json), their arms, and the
  readings the generated tracker derives from committed run records. This study does not change it. The
  placement table at the end is a cross-reference to existing arms, nothing more.
- **The HAZOP surface.** This page: guide words, deviations, safeguards and falsifiers. It classifies a
  stall; it does not measure one.
- **The Lean surface.** The vocabulary in [`spec/Rchain/Progress.lean`](../../../spec/Rchain/Progress.lean),
  the classification in `Rchain.Progress.Classification`, and the crosswalk emitted to
  [`spec/PROGRESS-CROSSWALK.md`](../../../spec/PROGRESS-CROSSWALK.md). The classification is the machine
  form of this page's axes; the crosswalk assigns every law a role and defaults the rest to
  `independent`, and the register's check 11 refuses a progress-family law left at that default.

The method is the one in [failure-mode HAZOP and disposition](failure-hazop.md): the eleven IEC guide
words, the R1–R4 disposition ladder, and the **Lean4-first rule** — a deviation no existing law already
captures earns its theorem before any Rust changes.

## Why this is a study, not a class of its own

ρ-calculus can express deadlock by construction, and the node executes ρ terms, so deadlock is
*representable* at the contract layer — `Rchain.silenceSystem` and `Rchain.a_silent_term_is_stuck`
prove the shape — while the node's own protocols (merge, finality, sync, admission, replay) can also
stop making progress. The taxonomy in this tree has around forty measured instances in seven structural
families and **two causes of the cause**:

1. **Self-referential gating.** The guard's predicate is a function of the surface it freezes, so the
   condition it waits for cannot become true again.
2. **Loss-blind refusal.** A bounded queue, budget or rejection drops the unit without a re-queue,
   reset or visible path, so a one-step refusal becomes permanent.

Both occur in different environments — a resting round is local, a partition needs several hosts, a
checkpoint boot needs a joiner — and each environment is owned by a different claim and campaign. A
separate claim for the class would either duplicate all seven environments or let a local arm claim
coverage of a public failure. The class is therefore carried by this HAZOP study and by the Lean
classification, and its measurements are placed in the existing claims' arms.

## Top event

**TE-7 — absorbing stall.** Block production stops and no named safeguard (R2 observation, R3
reset/catch-up, R4 operator action) restores it inside the campaign's observation window.

It is distinct from the failure HAZOP's existing top events: **TE-1** is a chain halt that recovery
*can* address, **TE-5** is a single node wedging while the network runs on, and **TE-6** is a
rewind-forcing divergence. TE-7 is the case where the refusal itself is the permanent state.

The foundation is **classification, attribution and containment, not "the chain always progresses"**.
A positive liveness theorem is not statable over a bare transition relation in this tree
(`spec/Rchain/Progress.lean`: `Fair` is a named but unmodelled hypothesis), so the laws prove that a
stall has a shape, that a refusal names its witness, and that a restoring rule enables a step.

## The axes

Every deviation in Table A is classified on three independent axes. The axes are the "venn": a deviation
is a point in `representation × shape × root cause`, and a repair is chosen from the rung it lands on.

- **Representation** — what kind of thing is stuck.
  - `closed-silence`: a closed ρ term with no counterpart. Nothing outside the term can add a send, so
    the silence is the calculus, not a node defect. TE-7 does **not** cover it.
  - `protocol-wait`: the protocol has a step that a delivery, round or peer message would enable. It is
    a wait, not a stall, until a rule suppresses or consumes the enabling event.
  - `protocol-stall`: no rule in the system restores progress inside the observation window. This is
    TE-7.
- **Shape** — why the obligation cannot be met. Law 51's vocabulary: `Void`, `Terminal`, `Drift`,
  `Split`, `Historic`, `Stuck`, `Silent`, `Persistent`, `Unrestorable`, `Paced`.
- **Root cause** — where the fix goes (the "cause of the cause"): `unsatisfiable-requirement`,
  `self-referential-gating`, `loss-blind-refusal`, `divergent-readers`, `historic-predicate`,
  `recovery-before-anchor`, `premature-verdict`, `unverified-trust`, `mixed-version`.
- **Safeguard** — the R1–R4 rung: `R1` unrepresentable, `R2` fail loudly, `R3` hard reset then catch up,
  `R4` runbook.

**Guide words.** row: NO, MORE, LESS, AS WELL AS, PART OF, OTHER THAN, REVERSE, EARLY, LATE, BEFORE, AFTER.

## Table A — deviations that can produce TE-7

A `Representation`, `Shape` or `Root cause` cell uses the canonical wire names above. A `Lean cell`
names a law in [`spec/laws.tsv`](../../../spec/laws.tsv) or a declaration under `spec/Rchain/`, or says
`owed:`. A `Falsifier` is a real `path::symbol` or `owed:`. There is deliberately **no claim column**:
where a deviation is measured belongs in the placement table, at the end.

| ID | Guide word | Deviation | RCA anchor | Root cause | Shape | Representation | Consequence | Safeguard | Lean cell | Falsifier |
|---|---|---|---|---|---|---|---|---|---|---|
| DE-01 | NO | A cost-accounting move composes onto a vault the merge base never funded, and the merge refuses every proposal | `casper/src/merging.rs::compose_cost_accounting`; measured 2026-11-23, finding registered with the fix | loss-blind-refusal | terminal | protocol-stall | No block is produced on that view; a restart does not clear it | R3: reject the unfundable chain and re-merge | Law 53b | owed: the regression test registered with the fix |
| DE-02 | REVERSE | A finalised-floor refusal repeats, and the refusal has no inverse | `casper/src/merging.rs::finalised_floor`; RC-2 local 30→31 (F-U4-06) | loss-blind-refusal | terminal | protocol-stall | Every retry refuses on the same loss; the deploy never settles | R3: classify the rejection against the kept set | `Rchain.FinalizedMerge.a_finalised_deploy_is_not_in_the_ledger` | owed: the duplicate-versus-loss fixture |
| DE-03 | NO | A round that comes to rest at one height is never answered, so no validator re-proposes | `C192`, `C209`; H-U1-01 | self-referential-gating | stuck | protocol-stall | The chain seals with a deploy unfinalised | R2: the tap is keyed per sender, not per height | Law 51b | owed: the resting-round fixture |
| DE-04 | PART OF | One silent bonded validator keeps its denominator share; the rest progress but cannot finalise | `C174`, `C183`; Law 52 | unsatisfiable-requirement | void | protocol-stall | Height advances, finality starves; part of the partition masks it | R2: observe the denominator and the leak | Law 52b, Law 89 | owed: the armed-network arm |
| DE-05 | LESS | A partition leaves neither side a strict supermajority | Law 52; `C344` | unsatisfiable-requirement | void | protocol-stall | Finality freezes while heights move | R3: partition-aware recovery | Law 52a | owed: the multi-host partition arm |
| DE-06 | LESS | The finality layer counts exactly two thirds (200 of 300) and refuses a strict supermajority | `spec/audit/rc-bond-leave-stall-rca-20261122.md`; Law 58 unproved | unsatisfiable-requirement | void | protocol-stall | The survivors cannot finalise the leave boundary | R2: the denominator and the leak are observable | `Rchain.Liveness.law89_statement` | owed: the bond-leave finality series |
| DE-07 | EARLY | An equivocation verdict latches before the evidence is a verdict, and honest peers stop proposing | `C367` | premature-verdict | terminal | protocol-stall | A local observation becomes a network-wide halt | R2: the verdict stage clears the latch | `Rchain.Disposition` | owed: the equivocation drill |
| DE-08 | OTHER THAN | Two nodes merge the same justifications to different pre-states, and the recovery route rewrites the committed claim | `C215`; Law 17a | divergent-readers | split | protocol-stall | A node cannot reproduce its own history | R1: one key for ordering, equality and hashing | `Rchain.MergeScope.scope_is_a_function_of_the_dag` | `casper/tests/merge_determinism.rs::an_arrival_order_moves_the_round_snapshot_and_not_the_scope_a_block_derives` |
| DE-09 | AFTER | Availability is read after the observation window, so a stall that ended is reported as none | F-U24 availability; `C340` | historic-predicate | historic | protocol-wait | The stall is invisible to the operator | R2: the status carries the stall episode | `Rchain.traceRefines` | owed: the delayed-read fixture |
| DE-10 | LATE | A deterministic self-replay refusal is retried as divergence for hours instead of taking its safe exit | `C357`, `C361`; Laws 92–95 | loss-blind-refusal | drift | protocol-stall | The validator never broadcasts; finality caps | R3: one rebuild, one reset per anchor | `Rchain.System.Persistent`, `Rchain.System.Terminal` | `casper/tests/reset.rs::a_self_replay_reset_returns_the_deploy_before_purging_its_only_block` |
| DE-11 | NO | A rejoining validator never retries a missing justification and stays behind | `C211` | loss-blind-refusal | stuck | protocol-stall | The returner never rejoins; the survivors run on | R2: the dependency set is every parent not yet held | `Rchain.System.Stuck` | owed: the returner arm |
| DE-12 | BEFORE | Recovery runs before the anchor it recovers to is valid | `C259`, `C274`; Law 53c | recovery-before-anchor | terminal | protocol-stall | The node rewinds into a state it cannot extend | R3: refuse until the anchor is verified | `Rchain.the_refusal_is_persistent` | owed: the failover drill |
| DE-13 | AS WELL AS | The block receiver is full, the block is dropped, and the chain progresses without it | `C340`; F-U8-02 | loss-blind-refusal | terminal | protocol-stall | A permanent drop with no re-queue and no counter | R2: re-offer, and the bound is visible | Law 53b | owed: the full-receiver fixture |
| DE-14 | MORE | An attestation storm outruns production, and the guard that would pace it is short-circuited | `C170`, `C171`; H-U2-01 | self-referential-gating | drift | protocol-stall | Production outruns finality and the queue | R2: the cadence guard reads its own witness | `Rchain.Pacing.take` | owed: the storm arm |
| DE-15 | NO | The sync requester is consumed by the first attempt, and a failed sync sits in NodeSyncing with a serving API and no error | `C181`, `C337`; Law 81 | loss-blind-refusal | stuck | protocol-stall | The joiner never enters Running | R2: retry loop with a terminal signal | `Rchain.Sync.Join` | owed: the sync-failure fixture |
| DE-16 | OTHER THAN | A join completes by trusting fringe metadata the joiner cannot check | `C338`, Law 83 | unverified-trust | split | protocol-stall | State at the anchor is not the state that was signed | R1: derivable fringe is refused | `Rchain.Sync.Trust` | owed: the lying-ancestry fixture |
| DE-17 | BEFORE | A populated DAG with no stored certificate refuses to boot before it asks for the adopted one | `C403`, `C404`; Law 108 | recovery-before-anchor | terminal | protocol-stall | A restarted validator cannot start during its jail | R3: re-verify or fetch the adopted certificate | `Rchain.Sync.Checkpoint.law104` | owed: the boot-certificate fixture |
| DE-18 | AS WELL AS | A mixed-store upgrade runs the new node while a prior-format contract path stalls | C402 residue; RC-7 local | mixed-version | terminal | protocol-stall | The upgrade reports success while the old path is dead | R3: refuse the mixed mode or migrate it | Law 53b | owed: the mixed-store fixture |
| DE-19 | AFTER | A fleet upgrade leaves one validator on the old image after the window | RC-7 public; no run | mixed-version | split | protocol-stall | The fleet forks by version and finality stalls | R4: the fleet inventory is checked after the window | Law 51a | owed: the fleet-upgrade arm |

## Table B — representability matrix

Representation is the first cut, because it decides whether TE-7 even applies. A `closed-silence` is
the calculus behaving as specified; a `protocol-wait` is not yet a stall; only a `protocol-stall` is
TE-7. The machine form is `Rchain.Progress.Classification.Representation` (consolidation).

| Representation | What can be stuck | Possible shapes | Lean module/law | Non-vacuity witness | What it is not |
|---|---|---|---|---|---|
| closed-silence | a closed ρ term whose receive has no matching send | silent, stuck | Law 51c | `Rchain.a_silent_term_is_stuck` | not a node stall; TE-7 excludes it |
| protocol-wait | a protocol state whose next step is a delivery, round or peer message | stuck, silent, drift, historic | Law 51a, Law 51d, `Rchain.Pacing` | `Rchain.Pacing.the_correct_gate_waits_a_full_interval` | not a stall until a rule consumes the event |
| protocol-stall | a protocol state no rule restores | terminal, persistent, unrestorable, void, split, drift | Law 51a, Law 53a | `Rchain.the_refusal_is_persistent` | not a safety failure; the denominator may not shrink |

## Table C — root-cause coverage

Every root cause in the vocabulary appears exactly once. A cause is a place a repair can go; the
classification module makes it a constructor, so the gate can refuse a cause the page invents.

| Root cause | Definition | DE rows | Canonical shapes | Classifying laws | Default rung |
|---|---|---|---|---|---|
| unsatisfiable-requirement | the requirement names a state nothing can satisfy | DE-04, DE-05, DE-06 | void | Law 51a, Law 52b | R1 |
| self-referential-gating | the guard's predicate is a function of the surface it freezes | DE-03, DE-14 | stuck, drift | Law 51b, Law 51d | R2 |
| loss-blind-refusal | a unit leaves the working set with no re-queue, reset or terminal surface | DE-01, DE-02, DE-10, DE-11, DE-13, DE-15 | terminal, persistent, drift, stuck | Law 51a, Law 53a, Law 53b | R3 |
| divergent-readers | two readers of one state disagree after a reachable step | DE-08 | split | Law 51b, Law 54b | R1 |
| historic-predicate | a predicate reads history rather than the current view | DE-09 | historic | Law 54a | R2 |
| recovery-before-anchor | a recovery step runs before the anchor it needs is valid | DE-12, DE-17 | terminal | Law 51a, Law 53c | R3 |
| premature-verdict | a verdict latches before the evidence is a verdict | DE-07 | terminal, persistent | Law 51a, Law 53a | R2 |
| unverified-trust | a join accepts metadata it cannot derive | DE-16 | split | Law 54b, Law 83 | R1 |
| mixed-version | two protocol versions interpret the same state differently | DE-18, DE-19 | terminal, split | Law 51a, Law 53a | R4 |

## Table D — law coverage

| Deviation | Law or shape that captures it | Why |
|---|---|---|
| DE-01, DE-13, DE-18 | Law 53b | A refused unit is re-queued or terminal-with-surface; no silent drop |
| DE-02 | Law 53a, Law 53b | A refusal with no inverse is terminal; the floor needs a restore rule |
| DE-03, DE-14 | Law 51b, Law 51d | A gate may not read the view it freezes; the pacing permit is the escape |
| DE-04, DE-05, DE-06 | Law 52a, Law 52b, Law 89 | The denominator is the bonded set; the requirement must be satisfiable |
| DE-07 | Law 51a, Law 53a | A latched verdict is a persistent refusal |
| DE-08 | `Rchain.MergeScope.scope_is_a_function_of_the_dag`, Law 54b | The DAG-derived scope is one function; two readers may not disagree |
| DE-09 | `Rchain.traceRefines`, Law 54a | The episode must be on the surface; the predicate reads the view |
| DE-10, DE-11, DE-15 | `Rchain.System.Persistent`, `Rchain.System.Stuck`, `Rchain.Sync.Join` | A retry without a terminal signal is a persistent refusal |
| DE-12, DE-17 | Law 53c, `Rchain.Sync.Checkpoint.law104` | A rewind is guarded by the anchor; a boot is guarded by the certificate |
| DE-16 | Law 83, `Rchain.Sync.Trust` | The joiner refuses fringe it cannot derive |
| DE-19 | Law 51a | Version skew is a split population; the shape is classified, not measured |
| All | Law 51a, Law 51b | The classification ontology: shape, cause of the cause, and the layer boundary |

## Table E — placement cross-reference (the RC is unchanged)

This table says where each deviation can be measured. It changes **no claim, no arm and no reading**.
The `Claim` and `Claim arm` cells must name a required arm already in
[`spec/audit/release-claims.json`](../../../spec/audit/release-claims.json); `Campaign` is that arm's
`next_pr`; `Measurement status` is `owed` (no stall run in that environment yet) or `live` (a committed
evidence record exists). A change to the RC surface is a separate campaign PR.

| DE | Claim | Claim arm | Campaign | Measurement status | Evidence |
|---|---|---|---|---|---|
| DE-01 | RC-1 | local | rc-confirm-local | owed | - |
| DE-01 | RC-1 | public | rc-public-fleet | owed | - |
| DE-02 | RC-1 | bond-leave | rc-bond-leave | owed | - |
| DE-03 | RC-2 | local | rc-confirm-local | owed | - |
| DE-04 | RC-2 | public | rc-public-fleet | owed | - |
| DE-05 | RC-2 | multi-host | rc-multi-host | owed | - |
| DE-07 | RC-2 | equivocation | rc-equivocation | owed | - |
| DE-06 | RC-2 | validator-set | rc-bond-leave | owed | - |
| DE-08 | RC-3 | local | rc-confirm-local | owed | - |
| DE-08 | RC-3 | public | rc-public-fleet | owed | - |
| DE-09 | RC-3 | availability | rc-public-fleet | owed | - |
| DE-10 | RC-4 | local | rc-confirm-local | owed | - |
| DE-10 | RC-4 | public | rc-public-fleet | owed | - |
| DE-05 | RC-4 | multi-host | rc-multi-host | owed | - |
| DE-12 | RC-4 | failover | rc-failover | owed | - |
| DE-11 | RC-4 | returner | rc-bond-leave | owed | - |
| DE-13 | RC-5 | local | rc-confirm-local | owed | - |
| DE-14 | RC-5 | public | rc-public-fleet | owed | - |
| DE-14 | RC-5 | transport | rc-transport | owed | - |
| DE-15 | RC-6 | local | rc-confirm-local | owed | - |
| DE-17 | RC-6 | public | rc-public-fleet | owed | - |
| DE-16 | RC-6 | off-host | rc-offhost | owed | - |
| DE-09 | RC-6 | availability | rc-public-fleet | owed | - |
| DE-18 | RC-7 | local | rc-harness-old-image | owed | - |
| DE-19 | RC-7 | public | rc-fleet-upgrade | owed | - |

**Debt.** deviations: 19 · placement rows owed: 25 · required arms placed: 25/25.

## Outside the diagram

The Lean classification is not total over the world, and saying so is part of the study. These are the
residues: facts that can stop a node and that the transition relation does not model. They are not
counted as TE-7 unless a campaign measures them in its own environment.

| Residue | Why Lean cannot state it | Witness |
|---|---|---|
| scheduler starvation | the relation fixes no schedule; `Fair` is named and unmodelled | `Rchain.reduce_not_deterministic` |
| physical network partition | delivery is a hypothesis, not an event the protocol controls | `docs/src/formal/progress.md` |
| I/O and storage errors | host facts outside the state relation | `docs/src/node/storage.md` |
| wall-clock windows | a timing hypothesis, not a transition | `docs/src/formal/progress.md` |
| operator action | the R4 runbook is outside the protocol | `docs/src/spec/failure-hazop.md` |
| fleet version skew | an external campaign fact, not a protocol state | `docs/src/node/updating.md` |

## What this study does not claim

- It does not claim positive liveness. The laws classify, attribute and contain; the note of each says
  it is not a liveness promise, exactly as Laws 53b/53c do.
- A green arm means the safeguard fired and progress resumed inside the observation window. A stall
  outside the injected deviation is not covered.
- The placement debt is the point: the ~40 measured instances have only a minority of live stall runs
  in their own environments today. This page makes that countable and puts each one in the campaign
  that can measure it.
- Public and host-loss rows need the four-host fleet; until those campaigns run, their placement rows
  stay `owed`.
