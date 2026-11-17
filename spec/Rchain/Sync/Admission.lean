import Rchain.Casper.MergeScope
import Mathlib.Data.Finset.Basic
import Mathlib.Tactic

/-!
# Law 116 — the pre-replay gate: a held root replays, a committed-history block is refused

`casper/src/blocks/block_processor.rs:824-857` installs the accepted window and then admits a block
to replay iff it is **not** committed history and either

* `permits_block` holds: its pre-state is the anchor root or a **derived** root (a root whose state
  this node holds), **or** its justification set is nonempty and every justification is the anchor
  block or a validated block (`casper/src/engine/anchor.rs:250-272`); or
* every justification is committed: nonempty, held by the DAG, and at or below the anchor height
  (`block_processor.rs:862-880`).

A block whose pre-state is **certified but not derived** is named by the certificate but its state
is not held; it is not a replay base, so it is refused with `UncertifiedPreState` — not faulted.
The model separates the three predicates rather than collapsing them into one `held` set.
-/

namespace Rchain
namespace Sync
namespace Admission

open Classical

open Rchain.MergeScope

/-- The accepted window's predicates, as `AcceptedWindow` holds them. `derived` is the roots whose
    state the node holds; `certified` is the roots the certificate names; `validated` is the set of
    blocks that have been validated; `held` is the DAG's block set the justification lookup reads. -/
structure Window where
  anchorRoot : Nat
  anchorBlock : Nat
  anchorHeight : Nat
  derived : Finset Nat
  certified : Finset Nat
  validated : Finset Nat
  held : Finset Nat

/-- The verdict of the pre-replay gate. `faultUnknownRoot` is the retryable internal fault (no
    window installed); the other three are the Rust's statuses. -/
inductive Verdict where
  | replay
  | refusedCommittedHistory
  | refusedUncertifiedPreState
  | faultUnknownRoot
  deriving DecidableEq, Repr

/-- **The replay-base rule**: the anchor root, or a root whose state is held. A certified root is
    deliberately *not* a replay base until its state is derived. -/
def permits_replay_root (w : Window) (pre : Nat) : Prop :=
  pre = w.anchorRoot ∨ pre ∈ w.derived

/-- **The weaker membership rule** (`AcceptedWindow::permits`): the anchor root, a derived root, or
    a certified root. The pre-replay gate uses `permits_replay_root`, not this. -/
def permits (w : Window) (pre : Nat) : Prop :=
  permits_replay_root w pre ∨ pre ∈ w.certified

/-- **The ancestry rule**: nonempty, and every justification is the anchor block or a validated
    block (`anchor.rs:250-255`). -/
def permits_justifications (w : Window) (js : Finset Nat) : Prop :=
  js ≠ ∅ ∧ ∀ j ∈ js, j = w.anchorBlock ∨ j ∈ w.validated

/-- **The pre-replay gate's rule**: a held replay root **or** an anchored justification set. -/
def permits_block (w : Window) (pre : Nat) (js : Finset Nat) : Prop :=
  permits_replay_root w pre ∨ permits_justifications w js

/-- **Committed justifications**: nonempty, each held by the DAG, each at or below the anchor
    height (`block_processor.rs:862-880`). This is a height rule, not the closure rule. -/
def justificationsCommitted (w : Window) (js : Finset Nat) (heightOf : Nat → Nat) : Prop :=
  js ≠ ∅ ∧ ∀ j ∈ js, j ∈ w.held ∧ heightOf j ≤ w.anchorHeight

/-- The Rust predicate's shape: a block the anchor has seen, excluding the anchor itself. This is
    Law 109's `replayRefusedHistory` on the anchor. -/
def isCommittedHistory (w : Window) (dag : Dag) (id : Nat) : Prop :=
  IsReplayRefused dag w.anchorBlock id

/-- **The pre-replay gate.** A missing window is a retryable internal fault; committed history is
    refused; a permitted pre-state or committed justifications replay; anything else is refused as an
    uncertified pre-state. -/
noncomputable def admission (w : Option Window) (dag : Dag) (id pre : Nat) (js : Finset Nat)
    (heightOf : Nat → Nat) : Verdict :=
  match w with
  | none => Verdict.faultUnknownRoot
  | some w =>
      if _hcommitted : isCommittedHistory w dag id then
        Verdict.refusedCommittedHistory
      else if _hperm : permits_block w pre js ∨ justificationsCommitted w js heightOf then
        Verdict.replay
      else
        Verdict.refusedUncertifiedPreState

/-- The mutant: a gate that turns an unheld root into a fault. The Rust's retryable fault is
    reserved for the missing window; an unheld root is a refusal. -/
noncomputable def admissionFaulting (w : Option Window) (dag : Dag) (id pre : Nat) (js : Finset Nat)
    (heightOf : Nat → Nat) : Verdict :=
  match w with
  | none => Verdict.faultUnknownRoot
  | some w =>
      if _hcommitted : isCommittedHistory w dag id then
        Verdict.refusedCommittedHistory
      else if _hperm : permits_block w pre js ∨ justificationsCommitted w js heightOf then
        Verdict.replay
      else
        Verdict.faultUnknownRoot

/-- **An installed window never faults.** The fault is reserved for the missing window. -/
theorem no_admission_path_faults (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat)
    (heightOf : Nat → Nat) :
    admission (some w) dag id pre js heightOf ≠ Verdict.faultUnknownRoot := by
  unfold admission
  by_cases hc : isCommittedHistory w dag id
  · simp [hc]
  · by_cases hp : permits_block w pre js ∨ justificationsCommitted w js heightOf
    · simp [hc, hp]
    · simp [hc, hp]

/-- **A missing window is the retryable fault the Rust raises** (`ValidateError::Internal`). -/
theorem a_missing_window_is_a_retryable_fault (dag : Dag) (id pre : Nat) (js : Finset Nat)
    (heightOf : Nat → Nat) :
    admission none dag id pre js heightOf = Verdict.faultUnknownRoot := rfl

/-- **The exact characterization.** Admission to replay is exactly "not committed history, and the
    pre-state is permitted or the justifications are committed". -/
theorem admission_replays_iff (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat)
    (heightOf : Nat → Nat) :
    admission (some w) dag id pre js heightOf = Verdict.replay ↔
      ¬ isCommittedHistory w dag id ∧
        (permits_block w pre js ∨ justificationsCommitted w js heightOf) := by
  unfold admission
  by_cases hc : isCommittedHistory w dag id
  · simp [hc]
  · by_cases hp : permits_block w pre js ∨ justificationsCommitted w js heightOf
    · simp [hc, hp]
    · simp [hc, hp]

/-- **Committed history is refused**, whatever the pre-state. -/
theorem committed_history_is_refused (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat)
    (heightOf : Nat → Nat) (h : isCommittedHistory w dag id) :
    admission (some w) dag id pre js heightOf = Verdict.refusedCommittedHistory := by
  simp [admission, h]

/-- **A held sibling is replayable.** A post-certificate block the anchor has not seen, whose
    pre-state is the anchor root or a derived root, is admitted. -/
theorem a_held_sibling_is_replayable (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat)
    (heightOf : Nat → Nat) (hc : ¬ isCommittedHistory w dag id)
    (hp : permits_replay_root w pre) :
    admission (some w) dag id pre js heightOf = Verdict.replay := by
  have hpb : permits_block w pre js := Or.inl hp
  simp [admission, hc, hpb]

/-- **A certified-but-unheld root is refused, not faulted.** The certificate names the root, but its
    state is not in `derived`, so it is not a replay base; with no justifications the gate refuses it
    as an uncertified pre-state. `certified` is the field that makes the distinction visible. -/
theorem a_certified_unheld_root_is_refused_not_faulted (w : Window) (dag : Dag) (id pre : Nat)
    (_hcert : pre ∈ w.certified) (hc : ¬ isCommittedHistory w dag id)
    (hnotroot : ¬ permits_replay_root w pre) :
    admission (some w) dag id pre ∅ (fun _ => 0) = Verdict.refusedUncertifiedPreState := by
  have hnjust : ¬ permits_justifications w ∅ := by simp [permits_justifications]
  have hcommitted : ¬ justificationsCommitted w ∅ (fun _ => 0) := by
    simp [justificationsCommitted]
  have hpb : ¬ permits_block w pre ∅ := by
    intro h
    rcases h with h | h
    · exact hnotroot h
    · exact hnjust h
  simp [admission, hc, hpb, hcommitted]

/-- **An uncertified root is refused too.** The refusal does not depend on certification; the
    certified case above is the interesting one because the root is *named* yet still not held. -/
theorem an_uncertified_root_is_refused (w : Window) (dag : Dag) (id pre : Nat)
    (hc : ¬ isCommittedHistory w dag id) (hnotroot : ¬ permits_replay_root w pre) :
    admission (some w) dag id pre ∅ (fun _ => 0) = Verdict.refusedUncertifiedPreState := by
  have hnjust : ¬ permits_justifications w ∅ := by simp [permits_justifications]
  have hcommitted : ¬ justificationsCommitted w ∅ (fun _ => 0) := by
    simp [justificationsCommitted]
  have hpb : ¬ permits_block w pre ∅ := by
    intro h
    rcases h with h | h
    · exact hnotroot h
    · exact hnjust h
  simp [admission, hc, hpb, hcommitted]

/-- **A merged pre-state over validated justifications is replayed.** The pre-state is not a held
    root, but every justification is the anchor or a validated block. -/
theorem merged_pre_state_over_validated_justifications_is_replayed
    (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat) (heightOf : Nat → Nat)
    (hc : ¬ isCommittedHistory w dag id) (_hnotroot : ¬ permits_replay_root w pre)
    (hj : permits_justifications w js) :
    admission (some w) dag id pre js heightOf = Verdict.replay := by
  have hpb : permits_block w pre js := Or.inr hj
  simp [admission, hc, hpb]

/-- **Committed justifications are replayed.** A block whose justifications are all held at or
    below the anchor height is admitted even when its pre-state is a fresh merge. -/
theorem all_justifications_committed_is_replayed (w : Window) (dag : Dag) (id pre : Nat)
    (js : Finset Nat) (heightOf : Nat → Nat) (hc : ¬ isCommittedHistory w dag id)
    (_hnotroot : ¬ permits_replay_root w pre)
    (hcommitted : justificationsCommitted w js heightOf) :
    admission (some w) dag id pre js heightOf = Verdict.replay := by
  have hpb : permits_block w pre js ∨ justificationsCommitted w js heightOf := Or.inr hcommitted
  simp [admission, hc, hpb]

/-- **The weaker `permits` is the replay-root rule plus the certified set.** This is the relation
    between `AcceptedWindow::permits` and the stricter replay base. -/
theorem permits_is_the_root_or_certified (w : Window) (pre : Nat) :
    permits w pre ↔ permits_replay_root w pre ∨ pre ∈ w.certified :=
  Iff.rfl

/-- **The mutant proposition**: a windowed gate that faults on an unheld root. -/
def law116_mutant : Prop :=
  ∃ (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat) (heightOf : Nat → Nat),
    ¬ isCommittedHistory w dag id ∧
      ¬ permits_block w pre js ∧
      ¬ justificationsCommitted w js heightOf ∧
      admissionFaulting (some w) dag id pre js heightOf = Verdict.faultUnknownRoot

/-- **The falsifier, fired.** A window with no derived roots, a pre-state that is neither the anchor
    nor derived, and no justifications: the faulting gate returns `faultUnknownRoot`, the real gate
    `refusedUncertifiedPreState`. -/
theorem law116_falsifier : law116_mutant := by
  refine ⟨⟨0, 0, 0, ∅, {7}, ∅, ∅⟩, (fun _ => none), 6, 7, ∅, (fun _ => 0), ?_, ?_, ?_, ?_⟩
  · have hseen : Rchain.MergeScope.seenOf (fun _ => none) 0 = (∅ : Finset Nat) := rfl
    simp [isCommittedHistory, IsReplayRefused, replayRefusedHistory, committedHistory, hseen]
  · simp [permits_block, permits_replay_root, permits_justifications]
  · simp [justificationsCommitted]
  · have hseen : Rchain.MergeScope.seenOf (fun _ => none) 0 = (∅ : Finset Nat) := rfl
    simp [admissionFaulting, isCommittedHistory, IsReplayRefused, replayRefusedHistory,
      committedHistory, hseen, permits_block, permits_replay_root, permits_justifications,
      justificationsCommitted]

/-- **Law 116's statement.** An installed window never faults; admission is exactly the two
    permitted arms; committed history is refused; a certified-but-unheld root is refused rather than
    faulted; and the faulting gate is the mutant. -/
def law116_statement : Prop :=
  (∀ (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat) (heightOf : Nat → Nat),
    admission (some w) dag id pre js heightOf ≠ Verdict.faultUnknownRoot) ∧
  (∀ (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat) (heightOf : Nat → Nat),
    admission (some w) dag id pre js heightOf = Verdict.replay ↔
      ¬ isCommittedHistory w dag id ∧
        (permits_block w pre js ∨ justificationsCommitted w js heightOf)) ∧
  (∀ (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat) (heightOf : Nat → Nat),
    isCommittedHistory w dag id →
      admission (some w) dag id pre js heightOf = Verdict.refusedCommittedHistory) ∧
  (∀ (w : Window) (dag : Dag) (id pre : Nat) (js : Finset Nat) (heightOf : Nat → Nat),
    ¬ isCommittedHistory w dag id → permits_block w pre js ∨ justificationsCommitted w js heightOf →
      admission (some w) dag id pre js heightOf = Verdict.replay) ∧
  law116_mutant

theorem law116 : law116_statement := by
  refine ⟨?_, ?_, ?_, ?_, law116_falsifier⟩
  · intro w dag id pre js heightOf
    exact no_admission_path_faults w dag id pre js heightOf
  · intro w dag id pre js heightOf
    exact admission_replays_iff w dag id pre js heightOf
  · intro w dag id pre js heightOf h
    exact committed_history_is_refused w dag id pre js heightOf h
  · intro w dag id pre js heightOf hc hp
    unfold admission
    simp [hc, hp]

end Admission
end Sync
end Rchain
