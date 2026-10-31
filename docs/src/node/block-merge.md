# Block merging (RCHIP-02)

CBC-Casper lets a block have **multiple parents**, so validators can build on concurrent blocks
instead of serializing on one. That is only useful if a validator can turn *N* parent states into
**one** merged state — deterministically, on every node.

[RCHIP-02](https://github.com/rchain/rchip-proposals/issues/23) specifies how: don't re-**run** the
other parents' deploys, **merge their event logs**. This page is the map from that proposal to the
Rust node, plus the invariants that make the merge safe to finalize.

## The problem the RCHIP solves

The naive merge replays every deploy of every parent on top of the chosen base parent. Since parents
are re-merged at each step, the same deploys are executed again and again — the cost of a merge grows
with the *history*, not with the *divergence*.

RCHIP-02's proposal: keep, for each block, the changes it made (**event log**); when merging, detect
which changes **conflict**; apply only the non-conflicting ones; and mark the losers **denied** in the
block. The block still gets replayed **once** to check that its event log is correct — but merging no
longer re-executes other branches' deploys.

## The model

| RCHIP-02 | Rust |
|---|---|
| conflict set (events back to the last finalized block) | the **merge scope** — `MergeScope { final_scope, conflict_scope }` built from the merge fringe and the final fringe (`casper/src/merging.rs`) |
| per-deploy event log | `EventLogIndex` (`rspace/src/merger/event_log_index.rs`): the produces/consumes/joins a deploy touched, each classified (created / destroyed / copied by peek / existed in pre-state / touches a pre-state join), plus `NumberChannelsDiff` for mergeable channels |
| a block's changes | `BlockIndex` → `Vec<DeployChainIndex>`; each chain carries its `StateChange` (the trie-level diff), its `EventLogIndex` **and its own `native_effects`** — the native writes of the deploys *it* carries, and of no other chain's |
| denied deploys written into the block | `BlockMessage::rejected_deploys` (the `rejectedDeploys` field) |
| apply the surviving changes to the base state | `MergeScope::compute_merged_state` → `StateChange::combine` → `compute_trie_actions` (`rspace/src/merger/state_change_merger.rs`) → `HotStoreTrieAction`s → one checkpoint on the base history |
| "the deployer is not charged, the validator is not rewarded" | see *Open items* — the effects are excluded from the merged state; the accounting consequence is not implemented |

## The mergeability rules

Conflicts are decided from event logs, not by executing anything (`rspace/src/merger/event_log_merging_logic.rs`,
a port of `EventLogMergingLogic.scala`):

- `produces_created`, `consumes_created`, `produces_created_and_not_destroyed` classify a log's
  actions;
- `are_conflicting(a, b)` — two logs conflict when one creates/destroys what the other reads or
  writes in an incompatible way;
- `depends(a, b)` — one log depends on the other's effects, so they must both be kept, in order;
- a **shared deploy id** is always a conflict (`DeployChainIndex::deploys_are_conflicting`).

That is the counterpart of `MergeabilityRules.scala`; the RSpace-level rules that the Scala
implementation was missing are here, which is why the merge can be applied
without replay.

## The diff unit is the channel, not the event

`StateChange::apply` computes **one** pre→post diff per affected channel (produces) or channel set
(consumes), because that is the trie leaf the merge writes. A consume-and-replace deploy records the
consumed datum (the `Comm`, in `produces_consumed`) and the produced datum (the `Produce`, in
`produces_linear`) on the **same** channel with different event hashes; a per-event pass diffs that
channel twice and appends the produced datum twice
([Incident history](incident-history.md#the-finalized-merge-duplicate)). Both collectors are
`BTreeSet`s for this reason — `Vec::dedup` would not do, because entries arrive in event-hash order, not
grouped by channel. The joins map is keyed by the channel set and is idempotent.

## Conflict resolution is a function, not a choice

`sdk/src/dag/merging.rs` computes the resolution:

1. `compute_conflicts_map` / `compute_dependency_map` → `compute_relation_map_for_merge_set`;
2. `compute_rejection_options` → the candidate sets of deploys to drop;
3. `compute_optimal_rejection` → **the unique minimum**, compared lexicographically by
   `(total cost, size, sorted deploy set)`;
4. `resolve_conflict_set` → `(accepted, rejected)`, honouring the finalized scope
   (`incompatible_with_final`), dependency closure (`with_dependencies`) and the mergeable-channel
   balances.

Because step 3 is a total, lexicographic minimum over a finite set, *every* node picks the same
winner: **merge determinism is Law 17**, and it is exactly why the tie-break includes the sorted set
rather than just the cost.

`MergeScope::compute_merged_state` then combines the accepted chains' `StateChange`s and applies the
mergeable-channel arithmetic — a channel whose value is a number merges by **summing the diffs**
(`rholang/src/merging.rs`; the diffs are checked for `i64` overflow rather than wrapped — `spec/audit/passes.md` §6).

## Validation: replay once, and check the denied set

A validating node does **not** trust a block's claimed merge. In `casper/src/interpreter_util.rs`
(`validate_block_checkpoint`):

1. it recomputes the **pre-state** from the block's parents (`get_pre_state_for_parents`), and rejects
   the block if the claimed `pre_state_hash` differs;
2. it rejects the block with `BlockStatus::InvalidRejectedDeploy` if the block's `rejected_deploys`
   set differs from the one the parents' merge produces — a validator cannot deny a deploy that
   should have survived (or keep one that should have been denied);
3. it **replays** the block once at that pre-state and checks the resulting state hash.

So the event-log merge is what makes the *state*; the single replay is the audit that the event log
was honest. Both are required, and both are deterministic.

## Determinism

- the merge scope, the conflict/dependency maps, the rejection options and the optimum are all
  functions of the DAG and the event logs — no clocks, no maps with undefined iteration order
  (`BTreeMap`/`BTreeSet` throughout);
- **and a chain's fate does not move when the scope does.** "A function of the DAG" is true of the
  resolution and was not sufficient: the native relation is on each chain's **own** native writes, the
  whole-block rule is deleted rather than narrowed, and the acceptance of a chain that wrote no
  contended slot is the same in every scope it can appear in. Its law is **17c**
  (`spec/INVENTORY.md`); the `tests/` half is
  `casper/src/merging.rs::boundary_merge_tests`. The epoch-boundary write loss that motivated this is
  in [Incident history](incident-history.md#k8-write-loss-at-an-epoch-boundary);
- `StateChange::apply` diffs each affected channel once. This is a **deliberate deviation from the
  pinned Scala oracle**: `StateChange.scala::apply` at `1b7583649` maps the affected produces/consumes
  to sequences and appends `curVal.added ++ change.added`, so a consume-and-replace is diffed twice and
  the merged state holds two copies. **Hard fork:** a merge containing a consume-and-replace (or two
  continuations on one channel set) now produces a different merged state hash; every validator must
  run the fix from the fork point, and the deployed testnet chain is already bagged and cannot be
  repaired. `rejectedDeploys` does not move; the divergence appears as a pre-state-hash disagreement
  (`spec/audit/passes.md` §6; [Incident history](incident-history.md#the-finalized-merge-duplicate));
- `StateChange::combine` is a **monoid** and the mergeable-channel sum is commutative (**Law 9**), so
  independent logs merge in any order;
- the trie update is content-addressed (**Law 10**), so the merged state hash is a function of the
  changes, not of the order they were applied in;
- `calculate_num_channel_diff` is checked, so a number channel cannot silently wrap into a wrong
  merged state (`spec/audit/passes.md` §6).

## Open items

- **Charging a denied deploy.** RCHIP-02 says the deployer should not be charged and the validator
  should not be rewarded for a denied deploy. The node records the denied set and excludes its
  effects from the merged state; the *fee* consequence is not implemented — the merge's objective is
  `DeployChainIndex.deployChainCost`'s sum (`DeployChainIndex.scala:73`, `MergeScope.scala:87`) over
  the denied set as much as the included one, so a denied deploy's cost still counts. **This is now
  law 48 in `spec/INVENTORY.md`**, with the decision recorded there: leave it open *with the reason*
  rather than implement a proposal and call it a port, because closing it means choosing an objective
  and a refund path the Scala does not have — a consensus change with no oracle. Treat as an open
  design question, not a settled behaviour.

  **Laws 44–47 change what "not rewarded" means.** The deployer's phlo is deposited into the staking
  vault (`Pos.rhox:397-404`) and distributed by the epoch to the active set, so the RCHIP's second
  half is false in an *indirect* way: the denied deploy's phlo reaches the validators at the next
  epoch boundary, and this port's rewards are per-epoch, with no per-deploy counterpart in which "not
  rewarded *for this deploy*" could be stated.
- **The RCHIP's `RuntimeManager` obstacle is addressed differently.** The proposal notes that a global
  lock plus a singleton interpreter with global cost state prevented parallel execution. In this port
  block validation **forks a replay runtime** at the block's pre-state
  (`fork_replay_runtime`), so validations are self-contained; the scheduler's Laws 20–29 carry the
  concurrency story on the execution side.
- **The fast path is not benchmarked here.** The event-log merge avoids re-execution by construction,
  but this tree has no merge-cost benchmark; `docs/src/node/operating.md` covers running a node, not
  merge throughput.

> **Formal.** Merge monoid and commuting logs are **Law 9**; Merkle determinism is **Law 10**; replay
> determinism is **Law 11**; merge determinism (unique min-cost rejection) is **Law 17**. See
> [The laws](../formal/laws.md), [Determinism](../formal/determinism.md) and
> [`spec/INVENTORY.md`](../../../spec/INVENTORY.md).
