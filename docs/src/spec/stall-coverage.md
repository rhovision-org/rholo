# Deadly embrace and catastrophic stall across the release campaigns

**What this page is.** A HAZOP study of one failure mode — a refusal that stops block production and
is not undone by any named safeguard. It is **not a new release claim** and it carries no reading. The
seven claims in [`spec/audit/release-claims.json`](../../../spec/audit/release-claims.json) are
environment-typed: a stall is evidence only in the environment where it can occur, so the measurements
belong in those claims' arms. This page is the map that says which stall deviation belongs in which
claim and where it is measured. The registers and the generated tracker decide; this page describes.

The method is the one in [failure-mode HAZOP and disposition](failure-hazop.md): the eleven IEC guide
words, the R1–R4 disposition ladder, and the **Lean4-first rule** — a deviation not already captured
by a law earns a Lean theorem with a witness before any Rust changes.

## Why this is a study, not a class of its own

ρ-calculus can express deadlock by construction, and the node executes ρ terms, so deadlock is
*representable* at the contract layer — `Rchain.silenceSystem` and `a_silent_term_is_stuck` prove the
shape — while the node's own protocols (merge, finality, sync, admission, replay) can also stop
making progress. The taxonomy in this tree has around forty measured instances in seven structural
families, and **two causes of the cause**:

1. **Self-referential gating.** The guard's predicate reads a surface whose only mover is the progress
   the guard is gating, so the condition it waits for cannot become true again.
2. **Loss-blind refusal.** A bounded queue, budget or rejection drops the unit without a re-queue,
   reset or visible path, so a one-step refusal becomes permanent.

Both occur in different environments — a resting round is local, a partition needs several hosts, a
checkpoint boot needs a joiner — and each environment is owned by a different claim and campaign. A
separate claim for the class would either duplicate all seven environments or let a local arm claim
coverage of a public failure. The class is therefore carried by this HAZOP study and by the Lean law
register (Laws 129–133, owed), and its arms are added to the existing claims.

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

**Guide words.** row: NO, MORE, LESS, AS WELL AS, PART OF, OTHER THAN, REVERSE, EARLY, LATE, BEFORE, AFTER.

## Table A — deviations that can produce TE-7

A `Claim arm` cell names the existing required arm whose environment can exhibit the deviation. A
`Stall arm` cell is either `arm:<id>` (a stall arm that already exists under that claim, in the claim
arm's environment) or `owed:` (the arm this study asks the campaign to add). A `Lean` cell names the
law or declaration that captures the deviation, or `Law <n> (owed)` where the Lean4-first rule has not
been satisfied yet. A `Falsifier` is a `path::symbol` that exists, or `owed:`.

| ID | Claim | Claim arm | Guide word | Deviation | Cause (RCA anchor) | Consequence (TE-7 shape) | Safeguard | Lean shape/law | Stall arm | Falsifier |
|---|---|---|---|---|---|---|---|---|---|---|
| DE-01 | RC-1 | local | NO | A cost-accounting move composes onto a vault the merge base never funded, and the merge refuses every proposal | `casper/src/merging.rs::compose_cost_accounting`; measured 2026-11-23, finding registered with the fix | No block is produced on that view; a restart does not clear it | R3: reject the unfundable chain and re-merge | Law 131 (owed) | owed: stall-merge-funding | owed: the regression test registered with the fix |
| DE-02 | RC-1 | bond-leave | REVERSE | A finalised-floor refusal repeats, and the refusal has no inverse | `casper/src/merging.rs::finalised_floor`; RC-2 local 30→31 (F-U4-06) | Every retry refuses on the same loss; the deploy never settles | R3: classify the rejection against the kept set | `Rchain.MergeScope.a_finalised_deploy_is_not_in_the_ledger` | owed: stall-floor | owed: the duplicate-versus-loss fixture |
| DE-03 | RC-2 | local | NO | A round that comes to rest at one height is never answered, so no validator re-proposes | `C192`, `C209`; H-U1-01 | The chain seals with a deploy unfinalised | R2: the tap is keyed per sender, not per height | Law 130 (owed) | owed: stall-selfgate | owed: the resting-round fixture |
| DE-04 | RC-2 | public | PART OF | One silent bonded validator keeps its denominator share; the rest progress but cannot finalise | `C174`, `C183`; Law 52 | Height advances, finality starves; part of the partition masks it | R2+R4: live partition, then the leak | Law 52, Law 89 | owed: stall-quorum | owed: the armed-network arm |
| DE-05 | RC-2 | multi-host | LESS | A partition leaves neither side a strict supermajority | Law 52; `C344` | Finality freezes while heights move | R3: partition-aware recovery | Law 52 | owed: stall-partition | owed: the multi-host partition arm |
| DE-06 | RC-2 | validator-set | LESS | The finality layer counts exactly two thirds (200 of 300) and refuses a strict supermajority | `spec/audit/rc-bond-leave-stall-rca-20261122.md`; Law 58 unproved | The survivors cannot finalise the leave boundary | R2: the denominator and the leak are observable | `Rchain.law89_statement` | owed: stall-quorum-bond | owed: the bond-leave finality series |
| DE-07 | RC-2 | equivocation | EARLY | An equivocation verdict latches before the evidence is a verdict, and honest peers stop proposing | `C367` | A local observation becomes a network-wide halt | R2: verdict stage clears the latch | `Rchain.Disposition` | owed: stall-verdict-latch | owed: the equivocation drill |
| DE-08 | RC-3 | local | OTHER THAN | Two nodes merge the same justifications to different pre-states, and the recovery route rewrites the committed claim | `C215`; Law 17a | A node cannot reproduce its own history | R1: one key for ordering, equality and hashing | `Rchain.MergeScope.scope_is_a_function_of_the_dag` | owed: stall-merge-divergence | `casper/tests/merge_determinism.rs::an_arrival_order_moves_the_round_snapshot_and_not_the_scope_a_block_derives` |
| DE-09 | RC-3 | availability | AFTER | Availability is read after the observation window, so a stall that ended is reported as none | F-U24 availability; `C340` | The stall is invisible to the operator | R2: the status carries the stall episode | `Rchain.traceRefines` | owed: stall-availability | owed: the delayed-read fixture |
| DE-10 | RC-4 | local | LATE | A deterministic self-replay refusal is retried as divergence for hours instead of taking its safe exit | `C357`, `C361`; Laws 92–95 | The validator never broadcasts; finality caps | R3: one rebuild, one reset per anchor | `Rchain.Progress.Persistent`, `Rchain.Progress.Terminal` | owed: stall-replay-exit | `casper/tests/reset.rs::a_self_replay_reset_returns_the_deploy_before_purging_its_only_block` |
| DE-11 | RC-4 | returner | NO | A rejoining validator never retries a missing justification and stays behind | `C211` | The returner never rejoins; the survivors run on | R2: the dependency set is every parent not yet held | `Rchain.Progress.Stuck` | owed: stall-rejoin | owed: the returner arm |
| DE-12 | RC-4 | failover | BEFORE | Recovery runs before the anchor it recovers to is valid | `C259`, `C274`; Law 53c | The node rewinds into a state it cannot extend | R3: refuse until the anchor is verified | `Rchain.Casper.Stranding.the_refusal_is_persistent` | owed: stall-failover-anchor | owed: the failover drill |
| DE-13 | RC-5 | local | AS WELL AS | The block receiver is full, the block is dropped, and the chain progresses without it | `C340`; F-U8-02 | A permanent drop with no re-queue and no counter | R2+R3: re-offer, and the bound is visible | Law 131 (owed) | owed: stall-ingress-drop | owed: the full-receiver fixture |
| DE-14 | RC-5 | public | MORE | An attestation storm outruns production, and the guard that would pace it is short-circuited | `C170`, `C171`; H-U2-01 | Production outruns finality and the queue | R2: the cadence guard reads its own witness | `Rchain.Pacing.take` | owed: stall-storm | owed: the storm arm |
| DE-15 | RC-6 | local | NO | The sync requester is consumed by the first attempt, and a failed sync sits in NodeSyncing with a serving API and no error | `C181`, `C337`; Law 81 | The joiner never enters Running | R2: retry loop with a terminal signal | `Rchain.Sync.Join` | owed: stall-sync-latch | owed: the sync-failure fixture |
| DE-16 | RC-6 | off-host | OTHER THAN | A join completes by trusting fringe metadata the joiner cannot check | `C338`, Law 83 | State at the anchor is not the state that was signed | R1: derivable fringe is refused | `Rchain.Sync.Trust` | owed: stall-join-trust | owed: the lying-ancestry fixture |
| DE-17 | RC-6 | public | BEFORE | A populated DAG with no stored certificate refuses to boot before it asks for the adopted one | `C403`, `C404`; Law 108 | A restarted validator cannot start during its jail | R3: re-verify or fetch the adopted certificate | `Rchain.law104` | owed: stall-checkpoint-boot | owed: the boot-certificate fixture |
| DE-18 | RC-7 | local | AS WELL AS | A mixed-store upgrade runs the new node while a prior-format contract path stalls | C402 residue; RC-7 local | The upgrade reports success while the old path is dead | R3: refuse the mixed mode or migrate it | Law 131 (owed) | owed: stall-mixed-image | owed: the mixed-store fixture |
| DE-19 | RC-7 | public | AFTER | A fleet upgrade leaves one validator on the old image after the window | RC-7 public; no run | The fleet forks by version and finality stalls | R4: the fleet inventory is checked after the window | Law 129 (owed) | owed: stall-version-refusal | owed: the fleet-upgrade arm |

## Table C — law coverage

| Deviation | Law or shape that captures it, or the law owed | Why |
|---|---|---|
| DE-01, DE-13, DE-18 | Law 131 (owed) | A refused unit is re-queued or terminal-with-surface; no silent drop |
| DE-03, DE-14 | Law 130 (owed) | A gate may not read the view it freezes; an external witness is the escape |
| DE-05, DE-06, DE-11 | Laws 51/52/89; Law 58 owed | The non-progress shapes and the finality denominator; positive liveness remains unproved |
| DE-10, DE-12 | Laws 53b/53c, 92–95; `Rchain.Progress` | A refusal is not persistent once a restoring rule exists; the note does not promise liveness |
| DE-04, DE-07, DE-09 | `Rchain.traceRefines`, `Rchain.Progress.ReadsTheView` | The refusal must name its witness; the episode must be on the surface |
| DE-02, DE-08, DE-16, DE-17 | Laws 113, 17a, 83, 104, 108 | The merge ledger, the DAG-derived scope, the join trust boundary, the checkpoint quorum |
| DE-15 | Law 81; `Rchain.Sync.Join` | A consumed walk key is not finished; the retry re-asks |
| All | Laws 129, 132, 133 (owed) | A stall has a shape; a restoring rule enables a step; user deadlock does not imply node stall |

## Coverage ledger — every required claim arm

One row per required arm in `spec/audit/release-claims.json`. The `Stall row` names the deviation that
arm is responsible for measuring; an arm whose row carries an `owed:` arm is debt in this study, not a
silent omission. `n/a (reason)` is allowed only where the failure mode cannot manifest in that
environment, with the reason written out.

| Claim | Arm | Environment | Stall row(s) |
|---|---|---|---|
| RC-1 | local | local | DE-01 |
| RC-1 | public | public | DE-01 |
| RC-1 | bond-leave | local | DE-02 |
| RC-2 | local | local | DE-03 |
| RC-2 | public | public | DE-04 |
| RC-2 | multi-host | local | DE-05 |
| RC-2 | equivocation | local | DE-07 |
| RC-2 | validator-set | local | DE-06 |
| RC-3 | local | local | DE-08 |
| RC-3 | public | public | DE-08 |
| RC-3 | availability | public | DE-09 |
| RC-4 | local | local | DE-10 |
| RC-4 | public | public | DE-10 |
| RC-4 | multi-host | local | DE-05 |
| RC-4 | failover | local | DE-12 |
| RC-4 | returner | local | DE-11 |
| RC-5 | local | local | DE-13 |
| RC-5 | public | public | DE-14 |
| RC-5 | transport | local | DE-14 |
| RC-6 | local | local | DE-15 |
| RC-6 | public | public | DE-17 |
| RC-6 | off-host | local | DE-16 |
| RC-6 | availability | public | DE-09 |
| RC-7 | local | local | DE-18 |
| RC-7 | public | public | DE-19 |

**Debt.** stall rows owed: 19 · ledger rows resting on an owed arm: 25.

## What this study does not claim

- It does not claim positive liveness. The laws classify, attribute and contain; the note of each says
  it is not a liveness promise, exactly as Laws 53b/53c do.
- A green arm means the safeguard fired and progress resumed inside the observation window. A stall
  outside the injected deviation is not covered.
- The debt line is the point: the ~40 measured instances have only a minority of live arms in their own
  environments today. This page makes that countable and puts each one in the campaign that can
  measure it.
- Public and host-loss rows need the four-host fleet; until those campaigns run, their arms read
  `not-observed`, never `safe`.
