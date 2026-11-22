import Mathlib.Tactic

/-!
# Law 128 — recovery preserves accepted work

The RC-4/public claim: after one validator is stopped and restarted, or a partition heals, the
survivors keep finalising and **every deploy accepted by a survivor is resolved**, and its effect is
in the post-recovery finalized state. The #564 incident's recovery also depends on the same rule: a
reset returns the deploys it removes from the DAG to the pool (`return_unfinalised_deploys`), and the
reset inventory refuses to rewind past an absent body rather than silently dropping it
(C366/C359/C365).

This module models the three recovery shapes the Rust implements:

* **pool replay** — a restart re-reads the persisted pool, so an accepted deploy is still pooled;
* **reset with return** — blocks above the anchor are dropped, but every accepted deploy above the
  anchor is returned to the pool (or stays finalized below it);
* **silent drop** — the mutant, which empties the pool and the DAG and loses the accepted deploy.

The invariant is `wellFormed`: an accepted deploy is already resolved (pooled, included or
finalized). The law proves the two real recoveries preserve it and the silent drop does not. The
Rust witness is RC-4 local's accepted-before/after and effect-at-finalized-head checks, plus the
reset inventory tests.
-/

namespace Rchain
namespace RecoveryPreservation

/-- The dispositions a node's state can hold for a deploy. -/
structure State where
  accepted : List Nat
  pooled : List Nat
  included : List Nat
  finalized : List Nat

/-- A deploy is resolved when it is still held anywhere the node can act on it. -/
def resolved (s : State) (d : Nat) : Prop :=
  d ∈ s.pooled ∨ d ∈ s.included ∨ d ∈ s.finalized

/-- **The precondition of a recovery claim:** every accepted deploy is resolved. A node that has
    already lost one cannot be certified by a recovery. -/
def wellFormed (s : State) : Prop :=
  ∀ d ∈ s.accepted, resolved s d

/-- A recovery preserves accepted work when every deploy accepted before it is resolved after it. -/
def preserves (before after : State) : Prop :=
  ∀ d ∈ before.accepted, resolved after d

/-- A restart that re-reads the persisted pool: the state is unchanged. -/
def replay (s : State) : State := s

/-- A reset to `anchor` that returns every unfinalized deploy above the anchor to the pool. -/
def reset (s : State) (anchor : List Nat) : State :=
  { accepted := s.accepted
    pooled := s.pooled ++ s.included.filter (fun d => d ∉ anchor) ++
      s.finalized.filter (fun d => d ∉ anchor)
    included := []
    finalized := anchor }

/-- **A pool replay preserves accepted work.** -/
theorem a_pool_replay_preserves (s : State) (h : wellFormed s) :
    preserves s (replay s) := by
  intro d hd
  exact h d hd

/-- **A reset that returns unfinalized deploys preserves accepted work.** A deploy below the anchor
    stays finalized; one above it is returned to the pool. -/
theorem a_reset_preserves (s : State) (anchor : List Nat) (h : wellFormed s) :
    preserves s (reset s anchor) := by
  intro d hd
  have hr := h d hd
  rcases hr with hp | hi | hf
  · exact Or.inl (by simp [reset, hp])
  · by_cases ha : d ∈ anchor
    · exact Or.inr (Or.inr (by simp [reset, ha]))
    · exact Or.inl (by simp [reset, ha, hi])
  · by_cases ha : d ∈ anchor
    · exact Or.inr (Or.inr (by simp [reset, ha]))
    · exact Or.inl (by simp [reset, ha, hf])

/-- The mutant: a recovery that empties the pool and the DAG. -/
def silentDrop (s : State) : State :=
  { accepted := s.accepted, pooled := [], included := [], finalized := [] }

/-- **A silent drop does not preserve accepted work.** -/
theorem a_silent_drop_does_not_preserve (s : State) (d : Nat) (hd : d ∈ s.accepted) :
    ¬ preserves s (silentDrop s) := by
  intro h
  have := h d hd
  simpa [silentDrop, resolved] using this

/-- The falsifier: one accepted deploy and the silent drop. -/
def law128_mutant : Prop :=
  ∃ (s : State) (d : Nat), d ∈ s.accepted ∧ ¬ preserves s (silentDrop s)

theorem law128_falsifier : law128_mutant := by
  refine ⟨{ accepted := [1], pooled := [], included := [], finalized := [] }, 1, by simp, ?_⟩
  exact a_silent_drop_does_not_preserve _ 1 (by simp)

/-- **Law 128's statement.** A pool replay and a reset-with-return preserve accepted work; the
    silent drop is refuted. -/
def law128_statement : Prop :=
  (∀ (s : State), wellFormed s → preserves s (replay s)) ∧
  (∀ (s : State) (anchor : List Nat), wellFormed s → preserves s (reset s anchor)) ∧
  law128_mutant

theorem law128 : law128_statement := by
  refine ⟨?_, ?_, ?_⟩
  · intro s h
    exact a_pool_replay_preserves s h
  · intro s anchor h
    exact a_reset_preserves s anchor h
  · exact law128_falsifier

end RecoveryPreservation
end Rchain
