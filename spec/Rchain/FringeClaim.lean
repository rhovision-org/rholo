import Mathlib.Tactic

/-!
# Law 127 — a block's declared fringe is a claim; the local read uses the recomputation

A block carries a `fringe` field, and a validator that ingests it recomputes the fringe from the
justifications. The trust model matters: if a surface read the **declared** field, a block could
feed a wrong or skipped-layer fringe into `is_finalized`; the port instead builds the validated
metadata's fringe from the merge's computed pre-state (`casper/src/interpreter_util.rs:1313,1506`),
and the restore/boot paths compare the restored metadata with the validating one
(`casper/src/engine/recovery.rs:361,403`, `casper/src/engine/node_launch.rs:758-791`). The register
had no model of that separation; this module is the model.

The law is deliberately about the **read**, not a new refusal: the declared field remains a claim
(the block is content-addressed with it), and the local finalized read uses the computation. A
future change that trusts the declared field is the mutant, and the falsifier shows a mismatched
claim that the trusted read rejects and the unchecked read accepts.
-/

namespace Rchain
namespace FringeClaim

/-- A block's fringe as declared on the wire and as recomputed by the validating node. -/
structure Block where
  declaredFringe : List Nat
  computedFringe : List Nat

/-- The read: `useComputed = true` is the validated node's read; `false` is the mutant that trusts
    the block's claim. -/
def readFringe (b : Block) (useComputed : Bool) : List Nat :=
  if useComputed then b.computedFringe else b.declaredFringe

/-- The validated node's read. -/
def trustedRead (b : Block) : List Nat :=
  readFringe b true

/-- The trusted read is the recomputation. -/
theorem the_trusted_read_is_the_computation (b : Block) :
    trustedRead b = b.computedFringe := rfl

/-- **A mismatched declared claim is not the trusted read.** -/
theorem a_mismatched_claim_is_not_the_trusted_read (b : Block)
    (h : b.declaredFringe ≠ b.computedFringe) :
    trustedRead b ≠ b.declaredFringe := by
  intro hc
  exact h (by simpa [trustedRead, readFringe] using hc.symm)

/-- The mutant: a mismatched declared fringe that the unchecked read would feed to the finalized
    read. -/
def law127_mutant : Prop :=
  ∃ (b : Block), b.declaredFringe ≠ b.computedFringe ∧
    readFringe b false = b.declaredFringe

/-- The falsifier: declared `[1]`, computed `[2]`. -/
theorem law127_falsifier : law127_mutant := by
  refine ⟨{ declaredFringe := [1], computedFringe := [2] }, by simp, rfl⟩

/-- **Law 127's statement.** The trusted read is the recomputation; a mismatch is not the trusted
    read; and the declared-trusting read is refuted. -/
def law127_statement : Prop :=
  (∀ b : Block, trustedRead b = b.computedFringe) ∧
  (∀ b : Block, b.declaredFringe ≠ b.computedFringe →
      trustedRead b ≠ b.declaredFringe) ∧
  law127_mutant

theorem law127 : law127_statement := by
  refine ⟨?_, ?_, ?_⟩
  · intro b
    rfl
  · intro b h
    exact a_mismatched_claim_is_not_the_trusted_read b h
  · exact law127_falsifier

end FringeClaim
end Rchain
