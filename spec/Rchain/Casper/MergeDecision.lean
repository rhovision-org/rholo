import Rchain.Casper.FinalizedMerge
import Rchain.Casper.MergeScope

/-!
# The merge decision as a phase space

The three guards the Rust merge applies are separate theorems:

* `FinalizedMerge.pickBase` — no base on a disagreement (`choose_base_claim`);
* `MergeScope.guardedForkBlocks` — a finalising merge names no forks (`MergeMode::Fringe`);
* `FinalizedMerge.guardedRejected` — the ledger is `rejected \ accepted` (`finalised_floor`).

This module bundles them into one decision: the input type (`Decision`), the outcome type
(`Outcome`), the `Safe` predicate, the theorem that a safe decision preserves every accepted
deploy, the unsafe witness (the ledger without the subtraction), and totality. The Rust trace
`MergeTrace` inhabits `Decision`; the evaluator recomputes `Safe`'s conjuncts from its coordinates.
-/

namespace Rchain
namespace MergeDecision

open FinalizedMerge

/-- The coordinates the merge decision reads. `fringeMerge` is `MergeMode::Fringe`. -/
structure Decision where
  fringeMerge : Bool
  claims : List Claim
  certifiedBase : Option Nat
  forkBlocks : Finset Nat
  accepted : Finset Nat
  rejected : Finset Nat

/-- What the decision produces: the base it starts from and the ledger it carries. -/
structure Outcome where
  base : Option Nat
  rejected : Finset Nat

/-- **The safe predicate**: a base is certified or every claim agrees; a finalising merge names no
    fork; the accepted and rejected sets are disjoint. -/
def Safe (d : Decision) : Prop :=
  (d.certifiedBase.isSome ∨ Agree d.claims) ∧
  (d.fringeMerge = true → d.forkBlocks = ∅) ∧
  Disjoint d.accepted d.rejected

theorem safe_base_agrees (d : Decision) (h : Safe d) :
    d.certifiedBase.isSome ∨ Agree d.claims := h.1

theorem safe_has_no_forks (d : Decision) (h : Safe d) :
    d.fringeMerge = true → d.forkBlocks = ∅ := h.2.1

theorem safe_ledger_disjoint (d : Decision) (h : Safe d) :
    Disjoint d.accepted d.rejected := h.2.2

/-- The total decision: the guarded base picker and the guarded rejection ledger. -/
def decide (d : Decision) : Outcome :=
  { base :=
      if d.certifiedBase.isSome then d.certifiedBase
      else (pickBase d.claims).map Claim.hash
    rejected := guardedRejected d.accepted d.rejected }

/-- **A safe merge preserves every finalised deploy.** The ledger subtracts the accepted set, so an
    accepted deploy is not a value of it. -/
theorem safe_merge_preserves_every_finalised_deploy (d : Decision) (_h : Safe d) (x : Nat)
    (hx : x ∈ d.accepted) : x ∉ (decide d).rejected := by
  simp [decide, guardedRejected, hx]

/-- **Totality**: every input has exactly one outcome. -/
theorem merge_decision_is_total (d : Decision) : ∃! o : Outcome, decide d = o := by
  refine ⟨decide d, rfl, ?_⟩
  intro o ho
  exact ho.symm

/-- **The boundary.** The ledger without the subtraction names an accepted deploy; the guarded
    decision does not. This is the state RC-1 observed (`rejectedDeploys` naming a finalised
    marker). -/
theorem an_unguarded_decision_can_reject_a_finalised_deploy :
    ∃ (d : Decision) (x : Nat),
      x ∈ d.accepted ∧ x ∈ d.rejected ∧ x ∉ (decide d).rejected := by
  refine ⟨{ fringeMerge := true, claims := [], certifiedBase := none, forkBlocks := ∅,
            accepted := {7}, rejected := {7} }, 7, by simp, by simp, ?_⟩
  simp [decide, guardedRejected]

/-- The witness above is outside `Safe` (its accepted and rejected sets overlap). -/
theorem the_unguarded_witness_is_unsafe :
    ¬ Safe { fringeMerge := true, claims := [], certifiedBase := none, forkBlocks := ∅,
             accepted := {7}, rejected := {7} } := by
  intro h
  have hdisjoint := h.2.2
  have hmem : (7 : Nat) ∈ ({7} : Finset Nat) := by simp
  exact (Finset.disjoint_left.mp hdisjoint hmem) hmem

end MergeDecision
end Rchain
