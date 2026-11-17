import Rchain.Casper.Fringe
import Mathlib.Data.Finset.Basic
import Mathlib.Data.Set.Basic
import Mathlib.Tactic

/-!
# Law 113's scope — the final scope is the trimmed layer, and a fork is rejected

The Rust merge scope (`casper/src/merging.rs`, `MergeScope::from_dag`/`from_fringes`) is the
**trimmed finalised layer** `between(final_fringe, prune_fringe)` plus the conflict scope
`between(merge_fringe, final_fringe)`. The C400 guard replaced the trimmed layer with the whole
finalised closure whenever a conflict-scope block had not seen the final fringe. That widening is
unbounded by construction: `between(final_fringe, ∅)` is every ancestor of the final fringe, so the
merge indexes the whole finalised history. On a checkpoint joiner those pre-states are not hydrated,
and the 2026-11-17 cold-rejoin run failed with `unknown root` on a height-23 block
(`spec/audit/evidence/c344-cold-rejoin-20261117/`).

This module states the replacement rule that the Rust must implement:

* the final scope is the trimmed layer, never the closure;
* a conflict-scope block that does not descend from the final fringe is a **fork**, and a fork's
  chains are rejected rather than merged against a replayed history;
* the scope is a function of the DAG, not of which states the local node happens to hold.

The tuple-space consequence — a rejected slice is not applied — is Law 113
(`Rchain.FinalizedMerge`). This module supplies the block-set half that was previously prose.
-/

namespace Rchain
namespace MergeScope

open Finset

/-- The DAG as the message map the merge reads. `none` is a block this node does not hold; the
port's `between` filters those out (`block-storage/src/dag/message_map.rs:31-50`). -/
abbrev Dag := Nat → Option Message

/-- The seen set of a block, or empty when the DAG does not hold it. -/
def seenOf (dag : Dag) (id : Nat) : Finset Nat :=
  match dag id with
  | some m => m.seen.toFinset
  | none => ∅

/-- `between upper lower = upper.seen \ lower.seen`, the port's scope construction. -/
def between (dag : Dag) (upper lower : Finset Nat) : Finset Nat :=
  (upper.biUnion (seenOf dag)) \ (lower.biUnion (seenOf dag))

/-- **The descent predicate**: the whole final fringe is in the block's seen set
(`final_fringe.is_subset(m.seen)`, `casper/src/merging.rs`). -/
def descends (dag : Dag) (finalFringe : Finset Nat) (id : Nat) : Prop :=
  finalFringe ⊆ seenOf dag id

/-- **The final scope is the trimmed layer.** No widening case exists in this definition. -/
def finalScope (dag : Dag) (finalFringe pruneFringe : Finset Nat) : Finset Nat :=
  between dag finalFringe pruneFringe

/-- The conflict scope: what the merge must reconcile. -/
def conflictScope (dag : Dag) (mergeFringe finalFringe : Finset Nat) : Finset Nat :=
  between dag mergeFringe finalFringe

/-- **The kept-conflict rule.** A kept set is a subset of the conflict scope, and every
non-descendant conflict block is outside it: a fork's chains are rejected, not merged. -/
def KeptRule
    (dag : Dag) (mergeFringe finalFringe kept : Finset Nat) : Prop :=
  kept ⊆ conflictScope dag mergeFringe finalFringe ∧
    ∀ id ∈ conflictScope dag mergeFringe finalFringe,
      ¬ descends dag finalFringe id → id ∉ kept

/-- **The rule implies every kept conflict block descends from the final fringe.** This is the
property the C400 widening was buying, obtained without reading the closure. -/
theorem kept_chains_descend
    (dag : Dag) (mergeFringe finalFringe kept : Finset Nat)
    (h : KeptRule dag mergeFringe finalFringe kept) :
    ∀ id ∈ kept, descends dag finalFringe id := by
  intro id hid
  by_contra hnd
  exact h.2 id (h.1 hid) hnd hid

/-- **A non-descendant conflict block is not kept.** -/
theorem a_non_descendant_is_not_kept
    (dag : Dag) (mergeFringe finalFringe kept : Finset Nat) (id : Nat)
    (h : KeptRule dag mergeFringe finalFringe kept)
    (hmem : id ∈ conflictScope dag mergeFringe finalFringe)
    (hnd : ¬ descends dag finalFringe id) :
    id ∉ kept :=
  h.2 id hmem hnd

/-- **The final scope is the trimmed layer, definitionally** — the theorem is the anti-regression
witness against reintroducing a widening case. -/
theorem final_scope_is_the_trimmed_layer
    (dag : Dag) (finalFringe pruneFringe : Finset Nat) :
    finalScope dag finalFringe pruneFringe = between dag finalFringe pruneFringe := rfl

/-- **The mutant: widen the final scope to the whole finalised closure.** This is the C400 guard's
`between(final_fringe, ∅)`; the theorem below exhibits a block it reads that the trimmed layer does
not. -/
def finalScopeWidened (dag : Dag) (finalFringe : Finset Nat) : Finset Nat :=
  between dag finalFringe ∅

/-- A fixture: `g ← p ← f`, with `p` the prune frontier and `f` the final fringe. -/
def fixtureDag : Dag
  | 1 => some ⟨1, 0, 0, 0, [], [1]⟩
  | 2 => some ⟨2, 1, 0, 0, [1], [1, 2]⟩
  | 3 => some ⟨3, 2, 0, 0, [2], [1, 2, 3]⟩
  | _ => none

/-- **The widening reads the closure.** The genesis block is in the widened scope and not in the
trimmed layer `between({f}, {p}) = {f}`. This is the unbounded read the live joiner could not
hydrate. -/
theorem widening_reads_the_closure :
    (1 : Nat) ∈ finalScopeWidened fixtureDag {3} ∧
      (1 : Nat) ∉ finalScope fixtureDag {3} {2} := by
  decide

/-- **The committed history of an anchor**: the anchor itself and every block the anchor has seen.
This is the closure a checkpoint certificate anchors; it is *not* every block at or below the
anchor's height. -/
def committedHistory (dag : Dag) (anchor : Nat) : Finset Nat :=
  insert anchor (seenOf dag anchor)

/-- **The admission predicate.** A block is committed history for a checkpoint joiner iff it is the
anchor or in the anchor's seen closure. -/
def IsCommittedHistory (dag : Dag) (anchor id : Nat) : Prop :=
  id ∈ committedHistory dag anchor

/-- **A same-height sibling is not committed history.** A block the anchor has not seen, however
close its height, is a live message the merge must be able to read — not history a joiner may refuse.
The Rust `is_committed_at_or_below_anchor` keyed on height alone, which refused the post-certificate
siblings and made every height-81 block fail `InvalidPreStateHash`. -/
theorem a_sibling_is_not_committed_history
    (dag : Dag) (anchor sibling : Nat)
    (hsib : sibling ∉ seenOf dag anchor) (hne : sibling ≠ anchor) :
    ¬ IsCommittedHistory dag anchor sibling := by
  intro h
  unfold IsCommittedHistory committedHistory at h
  rcases Finset.mem_insert.mp h with hsame | hseen
  · exact hne hsame
  · exact hsib hseen

/-- **The pre-replay refusal set.** The Rust `is_committed_history`
    (`casper/src/blocks/block_processor.rs:886-896`) returns **false for the anchor itself**: the
    anchor is the accepted base, handled by the separate anchor check, not a block a joiner refuses
    as history. This is the anchor's seen set — `committedHistory` without the anchor. -/
def replayRefusedHistory (dag : Dag) (anchor : Nat) : Finset Nat := seenOf dag anchor

/-- **The anchor boundary is deliberate.** The anchor is committed history (the certificate's
    closure includes it) but not in the pre-replay refusal set (the node runs from its state).
    The `hself` hypothesis is the DAG's own property: a message's seen set does not contain itself
    (`block-storage/src/dag/message_state.rs`), which the free model must state. -/
theorem the_anchor_is_committed_history_but_not_replay_refused
    (dag : Dag) (anchor : Nat) (hself : anchor ∉ seenOf dag anchor) :
    IsCommittedHistory dag anchor anchor ∧ anchor ∉ replayRefusedHistory dag anchor := by
  constructor
  · simp [IsCommittedHistory, committedHistory]
  · simpa [replayRefusedHistory] using hself

/-- **The refusal set is the committed history without the anchor.** This is the shape the Rust
    predicate has; the closure the certificate commits is the larger set. -/
theorem replay_refusal_is_committed_history_without_the_anchor
    (dag : Dag) (anchor : Nat) (hself : anchor ∉ seenOf dag anchor) :
    replayRefusedHistory dag anchor = committedHistory dag anchor \ {anchor} := by
  ext id
  by_cases hid : id = anchor <;> simp [replayRefusedHistory, committedHistory, hself, hid]

/-- **The scope is a function of the DAG.** Two nodes that hold the same message map derive the same
scope, regardless of which states they have hydrated; a node that lacks a pre-state must refuse,
not derive a different scope. -/
theorem scope_is_a_function_of_the_dag
    {dag dag' : Dag} (h : dag = dag')
    (mergeFringe finalFringe pruneFringe : Finset Nat) :
    finalScope dag finalFringe pruneFringe = finalScope dag' finalFringe pruneFringe ∧
      conflictScope dag mergeFringe finalFringe =
        conflictScope dag' mergeFringe finalFringe := by
  rw [h]
  exact ⟨rfl, rfl⟩

/-- **The fork set**: the conflict-scope blocks that have not seen the whole final fringe. This is
    the membership rule of `MergeScope.fork_blocks` (`casper/src/merging.rs:2054-2062`), the set
    `merge` rejects before the conflict search. A `Set` rather than a `Finset` because the DAG is an
    opaque function here: membership is the predicate the Rust set is built from. -/
def forkBlocks (dag : Dag) (mergeFringe finalFringe : Finset Nat) : Set Nat :=
  { id | id ∈ conflictScope dag mergeFringe finalFringe ∧ ¬ descends dag finalFringe id }

/-- **The fork set is a function of the DAG and the two fringes.** No hydrated state, no arrival
    order: a producer and a joiner that hold the same message map derive the same forks. This is the
    determinism the cold-join fix relies on. -/
theorem fork_blocks_are_a_function_of_the_dag
    {dag dag' : Dag} (h : dag = dag') (mergeFringe finalFringe : Finset Nat) :
    forkBlocks dag mergeFringe finalFringe = forkBlocks dag' mergeFringe finalFringe := by
  rw [h]

/-- A fork block is a conflict-scope block that does not descend from the final fringe. -/
theorem a_fork_block_is_a_non_descendant_conflict_block
    (dag : Dag) (mergeFringe finalFringe : Finset Nat) (id : Nat)
    (h : id ∈ forkBlocks dag mergeFringe finalFringe) :
    id ∈ conflictScope dag mergeFringe finalFringe ∧ ¬ descends dag finalFringe id := by
  simpa [forkBlocks] using h

/-- **The dependency-closure spec.** The Rust `with_dependencies` computes the least set containing
    the fork chains and closed under the dependency map (`sdk/src/dag/merging.rs:31-50`). This
    structure states that least-closure property; the merge reads only the two projections below. -/
structure DependencyClosure (dep : Nat → Finset Nat) (roots closure : Finset Nat) : Prop where
  contains_roots : roots ⊆ closure
  closed : ∀ x ∈ closure, dep x ⊆ closure
  least : ∀ c, roots ⊆ c → (∀ x ∈ c, dep x ⊆ c) → closure ⊆ c

/-- **Rejection is closed under dependency.** A chain in the forced set brings every chain it
    depends on with it: the merge cannot keep a dependent of a rejected fork. -/
theorem rejection_is_closed_under_dependency
    (dep : Nat → Finset Nat) (roots closure : Finset Nat)
    (h : DependencyClosure dep roots closure) :
    ∀ x ∈ closure, dep x ⊆ closure :=
  h.closed

/-- **A fork is rejected in the forced set.** The roots of the closure are the fork blocks' chains,
    so every root is in the set the merge removes from `to_merge`. -/
theorem a_fork_block_is_rejected
    (dep : Nat → Finset Nat) (forks closure : Finset Nat)
    (h : DependencyClosure dep forks closure) :
    forks ⊆ closure :=
  h.contains_roots

/-- **The closure is unique.** Two sets satisfying the least-closure spec are equal, so a producer
    and a joiner that compute the same dependency map reject the same set: the fork rule is
    deterministic, not an artefact of fold order. -/
theorem the_dependency_closure_is_unique
    (dep : Nat → Finset Nat) (roots c₁ c₂ : Finset Nat)
    (h₁ : DependencyClosure dep roots c₁) (h₂ : DependencyClosure dep roots c₂) : c₁ = c₂ := by
  apply Finset.Subset.antisymm
  · exact h₁.least c₂ h₂.contains_roots h₂.closed
  · exact h₂.least c₁ h₁.contains_roots h₁.closed

/-- **The mutant: classify a fork by the anchor's committed closure instead of by descent.** The
    closure rule calls a conflict block that *does* descend from the final fringe a fork — it is
    above the anchor and not in the anchor's seen set — so a legitimate merge participant is
    rejected. The descent rule keeps it. -/
def closureKeyedFork (dag : Dag) (anchor id : Nat) : Prop :=
  id ∉ committedHistory dag anchor

/-- The fork fixtures: `2` is the anchor (seen `{1}`), `6` descends from it, `5` is the merge
    block, and `1` is genesis. -/
def forkFixtureDag : Dag
  | 1 => some ⟨1, 0, 0, 0, [], [1]⟩
  | 2 => some ⟨2, 1, 0, 0, [1], [1, 2]⟩
  | 5 => some ⟨5, 4, 0, 0, [6], [1, 2, 6, 5]⟩
  | 6 => some ⟨6, 3, 0, 0, [1, 2], [1, 2, 6]⟩
  | _ => none

/-- **The falsifier, fired.** Block `6` sees the final fringe `{2}`, so it is not a fork under the
    descent rule; the closure rule calls it one because `6` is not in the anchor's seen set. -/
theorem a_descendant_is_not_a_fork_but_the_closure_rule_calls_it_one :
    (6 : Nat) ∈ conflictScope forkFixtureDag {5} {2} ∧ descends forkFixtureDag {2} 6 ∧
      (6 : Nat) ∉ forkBlocks forkFixtureDag {5} {2} ∧ closureKeyedFork forkFixtureDag 2 6 := by
  simp [conflictScope, between, seenOf, forkFixtureDag, descends, forkBlocks, closureKeyedFork,
    committedHistory]

/-- **Law 115's statement.** The fork set is a function of the DAG; its chains are rejected with
    their dependency closure; and the closure is unique. -/
def law115_statement : Prop :=
  (∀ (dag dag' : Dag), dag = dag' → ∀ mergeFringe finalFringe : Finset Nat,
    forkBlocks dag mergeFringe finalFringe = forkBlocks dag' mergeFringe finalFringe) ∧
  (∀ (dep : Nat → Finset Nat) (forks closure : Finset Nat),
    DependencyClosure dep forks closure → forks ⊆ closure) ∧
  (∀ (dep : Nat → Finset Nat) (roots c₁ c₂ : Finset Nat),
    DependencyClosure dep roots c₁ → DependencyClosure dep roots c₂ → c₁ = c₂)

theorem law115 : law115_statement := by
  refine ⟨?_, ?_, ?_⟩
  · intro dag dag' h mergeFringe finalFringe
    exact fork_blocks_are_a_function_of_the_dag h mergeFringe finalFringe
  · intro dep forks closure h
    exact a_fork_block_is_rejected dep forks closure h
  · intro dep roots c₁ c₂ h₁ h₂
    exact the_dependency_closure_is_unique dep roots c₁ c₂ h₁ h₂

/-- **The mutant proposition**: a legitimate conflict-scope descendant is classified as a fork by
    the closure rule. -/
def law115_mutant : Prop :=
  ∃ (dag : Dag) (anchor id : Nat),
    id ∈ conflictScope dag {5} {2} ∧ descends dag {2} id ∧ closureKeyedFork dag anchor id

/-- **The falsifier, fired.** The fixture above: `6` descends from the final fringe, so the closure
    rule's classification is false. -/
theorem law115_falsifier : law115_mutant := by
  refine ⟨forkFixtureDag, 2, 6, ?_, ?_, ?_⟩
  · simp [conflictScope, between, seenOf, forkFixtureDag]
  · simp [descends, seenOf, forkFixtureDag]
  · simp [closureKeyedFork, committedHistory, seenOf, forkFixtureDag]

end MergeScope
end Rchain
