import Rchain.Casper.Fringe
import Mathlib.Data.Finset.Basic
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

end MergeScope
end Rchain
