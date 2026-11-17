import Rchain.Casper.MergeScope
import Mathlib.Data.Finset.Basic
import Mathlib.Tactic

/-!
# Law 116 — a post-certificate sibling is admitted, and an unheld root is refused

**The change this law covers.** A checkpoint joiner holds the anchor's state and the certified
validation-window roots, not every historical pre-state. A same-height sibling of the anchor is
**not** committed history — the anchor has not seen it — so the pre-replay gate must not refuse it
on a height rule. But the sibling's pre-state (the anchor's own pre-state) may be certified and
still **not held**; that case must be refused as an uncertified pre-state before replay, never
faulted as an internal `unknown root` that latches readiness.

The Rust split is `is_committed_history` (`casper/src/blocks/block_processor.rs:886-896`) and
`AcceptedWindow::permits_replay_root` (`casper/src/engine/anchor.rs:269`), with
`permits_block` composing the held-root and justification checks. This module models the verdict
those two produce.

**What it does not claim.** That a sibling validates — only that admission never turns an unheld
root into a fault. The state itself is still a quorum claim (Law 85's residual), and the audit is
the counterweight.
-/

namespace Rchain
namespace Sync
namespace Admission

/-- The admission view: the anchor's committed history, the roots this node holds, and the
    certificate's certified window. -/
structure View where
  committed : Finset Nat
  held : Finset Nat
  certified : Finset Nat
  justificationsCommitted : Bool
  deriving DecidableEq

/-- **The pre-replay gate.** A committed-history block is refused before replay; a block whose
    pre-state is held is replayed; every other pre-state is refused as uncertified. There is no
    fault branch. -/
inductive Verdict where
  | replay
  | refusedCommittedHistory
  | refusedUncertifiedPreState
  | faultUnknownRoot
  deriving DecidableEq

/-- A block is committed history iff it is in the anchor's committed set. The Rust
    `is_committed_history` returns false for the anchor itself (the anchor is the accepted base,
    handled by the separate anchor check); this predicate is the set the pre-replay gate uses. It
    is a `Bool`, like the Rust function, so the verdict is computable. -/
def isCommittedHistory (v : View) (id : Nat) : Bool := decide (id ∈ v.committed)

/-- The admission verdict. The final branch is a refusal, not the `unknown root` fault. -/
def admission (v : View) (id pre : Nat) : Verdict :=
  if isCommittedHistory v id then Verdict.refusedCommittedHistory
  else if pre ∈ v.held then Verdict.replay
  else if v.justificationsCommitted then Verdict.replay
  else Verdict.refusedUncertifiedPreState

/-- **The mutant: fault an unheld root.** This is the pre-fix shape the live joiner hit: the
    pipeline asked for a root it did not hold and recorded an internal `unknown root`. -/
def admissionFaulting (v : View) (id pre : Nat) : Verdict :=
  if isCommittedHistory v id then Verdict.refusedCommittedHistory
  else if pre ∈ v.held then Verdict.replay
  else if v.justificationsCommitted then Verdict.replay
  else Verdict.faultUnknownRoot

/-- **No admission path faults.** Every verdict is a replay or a refusal. -/
theorem no_admission_path_faults (v : View) (id pre : Nat) :
    admission v id pre ≠ Verdict.faultUnknownRoot := by
  unfold admission
  by_cases h : isCommittedHistory v id = true
  · simp [h]
  · by_cases hh : pre ∈ v.held
    · simp [h, hh]
    · by_cases hj : v.justificationsCommitted <;> simp [h, hh, hj]

/-- **A held-root sibling is replayable.** It is not committed history and its pre-state is held. -/
theorem a_held_sibling_is_replayable (v : View) (id pre : Nat)
    (hcommitted : isCommittedHistory v id = false) (hheld : pre ∈ v.held) :
    admission v id pre = Verdict.replay := by
  simp [admission, hcommitted, hheld]

/-- **A certified-but-unheld root is refused, not faulted.** This is the sibling case the
    2026-11-17 join hit: the root is in the certificate's window, the joiner does not hold it, and
    the refusal is a pre-state verdict. -/
theorem a_certified_unheld_root_is_refused_not_faulted (v : View) (id pre : Nat)
    (hcommitted : isCommittedHistory v id = false) (_hcert : pre ∈ v.certified)
    (hheld : pre ∉ v.held) (hjust : v.justificationsCommitted = false) :
    admission v id pre = Verdict.refusedUncertifiedPreState := by
  simp [admission, hcommitted, hheld, hjust]

/-- **An uncertified root is refused too.** The verdict does not depend on the window for the
    refusal: what is not held is never replayed. -/
theorem an_uncertified_root_is_refused (v : View) (id pre : Nat)
    (hcommitted : isCommittedHistory v id = false) (hheld : pre ∉ v.held)
    (hjust : v.justificationsCommitted = false) :
    admission v id pre = Verdict.refusedUncertifiedPreState := by
  simp [admission, hcommitted, hheld, hjust]

/-- **Law 116's statement.** No admission path faults; a held sibling replays; everything else is
    refused as an uncertified pre-state. -/
def law116_statement : Prop :=
  (∀ (v : View) (id pre : Nat), admission v id pre ≠ Verdict.faultUnknownRoot) ∧
  (∀ (v : View) (id pre : Nat),
    isCommittedHistory v id = false → pre ∈ v.held → admission v id pre = Verdict.replay) ∧
  (∀ (v : View) (id pre : Nat),
    isCommittedHistory v id = false → pre ∉ v.held → v.justificationsCommitted = true →
      admission v id pre = Verdict.replay) ∧
  (∀ (v : View) (id pre : Nat),
    isCommittedHistory v id = false → pre ∉ v.held → v.justificationsCommitted = false →
      admission v id pre = Verdict.refusedUncertifiedPreState)

theorem law116 : law116_statement := by
  refine ⟨?_, ?_, ?_, ?_⟩
  · exact no_admission_path_faults
  · intro v id pre hc hh
    exact a_held_sibling_is_replayable v id pre hc hh
  · intro v id pre hc hh hj
    simp [admission, hc, hh, hj]
  · intro v id pre hc hh hj
    exact an_uncertified_root_is_refused v id pre hc hh hj

/-- **The mutant proposition**: a certified-but-unheld root is faulted. -/
def law116_mutant : Prop :=
  ∃ (v : View) (id pre : Nat),
    isCommittedHistory v id = false ∧ pre ∈ v.certified ∧ pre ∉ v.held ∧
      v.justificationsCommitted = false ∧
      admissionFaulting v id pre = Verdict.faultUnknownRoot

/-- **The falsifier, fired.** Committed `{1}`, nothing held, root `7` certified: the law refuses,
    the mutant faults. -/
theorem law116_falsifier : law116_mutant := by
  refine ⟨⟨{1}, ∅, {7}, false⟩, 6, 7, ?_, ?_, ?_, ?_, ?_⟩ <;>
    simp [isCommittedHistory, admissionFaulting]

end Admission
end Sync

end Rchain
