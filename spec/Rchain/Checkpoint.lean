import Rchain.Progress

/-!
# Law 74 — a checkpoint does not drain an overlay it has not folded

**Why this module exists.** A checkpoint folds two overlays into the trie — the hot store's pending
changes and the native store's — and the port does not treat them alike. `create_checkpoint` reads the
hot-store changes (`rspace/src/rspace.rs:604-605`, a read-only snapshot) and then **clears** the native
overlay at step 2 (`native_store.drain_native()`, `:606-609`, which ends in `overlay.clear()`,
`rspace/src/native_store.rs:477`) — *before* the one fallible step, the fold `checkpoint_with_native`
(`:616-621`). So a fold that returns `Err` leaves the native mutations gone from memory and absent from
the trie, while the hot-store side is untouched, and `last_native_changes`/`last_native_drain` already
hold the failed checkpoint's drain. The two sides do not commit or fail together.

**The law.** An overlay is drained **after** it is folded, never before: no reachable state has an
overlay cleared while its changes are not in the trie — so an interrupted checkpoint loses nothing it has
not already committed. The model below is one overlay (the property is per-overlay, and applying it to
both is what "both overlays or neither" means); the port's defect is that the native overlay follows the
wrong order.

**The falsifier is the port's order.** `drainFirstStep` is the same machine with the overlay cleared
first — the shape `create_checkpoint` has today — and `a_drain_before_the_fold_reaches_a_state_that_lost_
the_overlay` exhibits the state it admits: the overlay empty and the changes unfolded.
-/

namespace Rchain

/-- One overlay's state at a checkpoint: `pending` — the overlay holds changes not yet in the trie;
    `folded` — those changes are in the trie. -/
structure CheckpointState where
  pending : Bool
  folded : Bool
deriving DecidableEq, Repr

/-- The state a checkpoint starts in: changes to fold, none folded. -/
def checkpointFresh : CheckpointState := ⟨true, false⟩

/-- **The checkpoint's steps, in the order that loses nothing.** Fold the changes into the trie first,
    then clear the overlay — so the clear is only ever a step that follows a successful fold. -/
def checkpointStep (s s' : CheckpointState) : Prop :=
  (s.pending = true ∧ s.folded = false ∧ s' = ⟨true, true⟩)
  ∨ (s.pending = true ∧ s.folded = true ∧ s' = ⟨false, true⟩)

/-- A step is available exactly while there are changes in the overlay. -/
def checkpointEnabled (s : CheckpointState) : Bool := s.pending

/-- **The checkpoint, as a `System`** — fold, then drain. -/
def checkpointSystem : System where
  State := CheckpointState
  Step := checkpointStep
  enabled := checkpointEnabled
  enabled_iff := by
    intro σ
    obtain ⟨p, f⟩ := σ
    constructor
    · intro h
      simp only [checkpointEnabled] at h
      cases f
      · exact ⟨⟨true, true⟩, Or.inl ⟨h, rfl, rfl⟩⟩
      · exact ⟨⟨false, true⟩, Or.inr ⟨h, rfl, rfl⟩⟩
    · rintro ⟨σ', hs⟩
      unfold checkpointStep at hs
      rcases hs with ⟨h1, _, _⟩ | ⟨h1, _, _⟩
      · exact h1
      · exact h1

/-- The step lemma: the target of a checkpoint step has the changes either still pending or already
    folded — never neither. -/
theorem checkpoint_target_keeps_the_changes (s s' : CheckpointState) (hs : checkpointStep s s') :
    s'.pending = true ∨ s'.folded = true := by
  unfold checkpointStep at hs
  rcases hs with ⟨_, _, h3⟩ | ⟨_, _, h3⟩
  · rw [h3]; exact Or.inr rfl
  · rw [h3]; exact Or.inr rfl

/-- **The law, in one line.** From a fresh checkpoint, no reachable state has the overlay drained while
    its changes are not in the trie: an interrupted checkpoint loses nothing it has not committed. -/
theorem a_checkpoint_does_not_drain_what_it_has_not_folded (σ : CheckpointState)
    (hr : checkpointSystem.Reach checkpointFresh σ) : σ.pending = true ∨ σ.folded = true := by
  induction hr with
  | refl => exact Or.inl rfl
  | tail _ hstep _ => exact checkpoint_target_keeps_the_changes _ _ hstep

/-- **Non-vacuity: a checkpoint can complete.** -/
theorem a_checkpoint_can_complete :
    ∃ σ, checkpointSystem.Reach checkpointFresh σ ∧ σ.pending = false ∧ σ.folded = true :=
  ⟨⟨false, true⟩,
    Relation.ReflTransGen.tail
      (Relation.ReflTransGen.single (Or.inl ⟨rfl, rfl, rfl⟩)) (Or.inr ⟨rfl, rfl, rfl⟩),
    rfl, rfl⟩

/-- **The mutant the port has today**: the overlay is cleared before the fold, so a failure at the fold
    loses changes that were never committed. -/
def drainFirstStep (s s' : CheckpointState) : Prop :=
  (s.pending = true ∧ s' = ⟨false, false⟩)
  ∨ (s.pending = false ∧ s.folded = false ∧ s' = ⟨false, true⟩)

/-- **The falsifier, fired.** Under the port's order a state is reachable whose overlay is empty and
    whose changes are unfolded — the checkpoint's failure mode, and the state the hot-store side never
    enters because it is read before the fold and rebuilt only after. -/
theorem a_drain_before_the_fold_reaches_a_state_that_lost_the_overlay :
    ∃ σ, Relation.ReflTransGen drainFirstStep checkpointFresh σ
      ∧ σ.pending = false ∧ σ.folded = false :=
  ⟨⟨false, false⟩, Relation.ReflTransGen.single (Or.inl ⟨rfl, rfl⟩), rfl, rfl⟩

/-! ## 74b — the soft checkpoint's base

A `SoftCheckpoint` carries no root (`rspace/src/checkpoint.rs:14-21`), so the port's
`revert_to_soft_checkpoint` re-reads `history.root()` at revert time (`rspace/src/rspace.rs:771-780`).
A hard checkpoint between the snapshot and the revert therefore rebuilds the old overlay over a new
base. The law models the missing field and the guard it enables. -/

/-- A soft checkpoint's base, the history root now, and the root a revert used (if one ran). -/
structure SoftState where
  base : Nat
  current : Nat
  used : Option Nat
deriving DecidableEq, Repr

/-- A snapshot over `base`, nothing reverted. -/
def softInitial (base : Nat) : SoftState := ⟨base, base, none⟩

/-- The guarded revert: only while the history root is still the base the snapshot was taken over. -/
def guardedRevert (s s' : SoftState) : Prop :=
  s.used = none ∧ s.current = s.base ∧ s' = { s with used := some s.base }

/-- A hard checkpoint moves the history root; it does not move the snapshot's base. -/
def hardCheckpoint (root : Nat) (s s' : SoftState) : Prop :=
  s' = { s with current := root }

/-- The guarded system's steps: a revert at the snapshot's base, or a hard checkpoint moving the root. -/
def guardedStep (root : Nat) (s s' : SoftState) : Prop :=
  guardedRevert s s' ∨ hardCheckpoint root s s'

/-- **74b — a revert uses the base it snapshotted.** In the guarded system, every root a revert used is
the snapshot's base; a hard checkpoint between the snapshot and the revert only makes the revert
unavailable, because it moves `current` away from `base`. -/
theorem a_guarded_revert_uses_the_snapshots_base (root base : Nat) (σ : SoftState) (r : Nat)
    (hr : Relation.ReflTransGen (guardedStep root) (softInitial base) σ)
    (h : σ.used = some r) : r = σ.base := by
  induction hr with
  | refl => simp [softInitial] at h
  | tail _ hstep ih =>
      rcases hstep with hg | hh
      · obtain ⟨_, _, hEq⟩ := hg
        rw [hEq] at h ⊢
        simp only [Option.some.injEq] at h
        exact h.symm
      · rw [hh] at h ⊢
        exact ih h

/-- The law as a named proposition, for the register's `formal` field. -/
def law74b_statement : Prop :=
  ∀ (root base : Nat) (σ : SoftState) (r : Nat),
    Relation.ReflTransGen (guardedStep root) (softInitial base) σ →
    σ.used = some r → r = σ.base

/-- The law. -/
theorem law74b : law74b_statement :=
  fun root base σ r hr h => a_guarded_revert_uses_the_snapshots_base root base σ r hr h

/-- **The port's unguarded revert**: re-read the history root at revert time and use it as the base. -/
def unguardedRevert (s s' : SoftState) : Prop :=
  s.used = none ∧ s' = { s with used := some s.current }

/-- The unguarded system's steps. -/
def unguardedStep (root : Nat) (s s' : SoftState) : Prop :=
  unguardedRevert s s' ∨ hardCheckpoint root s s'

/-- **The mutant**: a hard checkpoint after the snapshot, then an unguarded revert, uses the new root
as the base — the state U15-02 names. -/
def law74b_mutant : Prop :=
  ∃ σ, Relation.ReflTransGen (unguardedStep 1) (softInitial 0) σ
    ∧ σ.used = some 1 ∧ σ.base = 0

/-- **The falsifier, fired.** Under the port's rule the revert's base is the root read at revert time,
not the snapshot's: a hard checkpoint at `1` moves it, and the old overlay is rebuilt over `1`. -/
theorem law74b_falsifier : law74b_mutant := by
  refine ⟨⟨0, 1, some 1⟩, ?_, rfl, rfl⟩
  have h1 : unguardedStep 1 (softInitial 0) ⟨0, 1, none⟩ := Or.inr rfl
  have h2 : unguardedStep 1 ⟨0, 1, none⟩ ⟨0, 1, some 1⟩ := Or.inl ⟨rfl, rfl⟩
  exact Relation.ReflTransGen.tail (Relation.ReflTransGen.single h1) h2

end Rchain
