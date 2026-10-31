import Rchain.Progress
import Mathlib.Data.Finset.Basic

/-!
# Law 80 — `CURRENT_ROOT` names only an accepted block

**Why this module exists.** The node's durable root pointer (`CURRENT_ROOT`) is moved by
`RootsStore::record_root` (`rspace/src/history/roots_store.rs:81`) at every checkpoint — including the
checkpoint a **replay** writes while validating a candidate block (`casper/src/runtime_replay.rs:302`)
and the checkpoint the merge/play path writes (`casper/src/runtime_manager.rs`). The block is inserted
into the DAG *afterwards* (`casper/src/blocks/block_processor.rs:118`): validation replays and
checkpoints first, then `dag.insert` accepts or refuses. A refused or dropped block therefore leaves
`CURRENT_ROOT` naming a post-state no accepted block references — a durable pointer that is not a
function of the chain the node is running. The class is the same one `reset_volatile`'s doc names for
the merge (`history_repository.rs:346-358`).

**The law.** Recording a root as **known** is not publishing it. A checkpoint may write the root tag
(the store knows the state); `CURRENT_ROOT` may move only when a block whose post-state is that root is
**accepted**. The model below has those two steps separately, a boot step that may republish only an
accepted root, and a stutter for a crash. The headline theorem is that every reachable state's
`current` is either absent or in the accepted set.

**The falsifier is the port's current order.** `earlyPublishStep` moves `current` at the *record*
step — the replay-checkpoint shape — and `an_early_publish_reaches_an_orphan_root` exhibits a reachable
state whose current root no accepted block references. The fix is to defer the publish, not to add a
read-side guard, which would leave the orphan representable.

**What this does not model.** The cross-store window between the insert and the publish (no WAL), and
the acceptance *order* beyond a non-decreasing height: the pointer is monotone in the model, and the
Rust publish additionally refuses an older block's post-state. The law states the property the code
must have, not that a crash between the two stores is impossible.
-/

namespace Rchain

/-- The publication state. `current` is the root `CURRENT_ROOT` names; `known` is the set of roots a
    checkpoint has recorded as known; `accepted` is the set of roots a block has been accepted with;
    `lastHeight` is the height of the most recent acceptance. -/
structure PubState where
  current : Option Nat
  known : Finset Nat
  accepted : Finset Nat
  lastHeight : Nat
deriving DecidableEq

/-- Before any checkpoint or block: no current root, nothing known, nothing accepted. -/
def pubInitial : PubState := ⟨none, ∅, ∅, 0⟩

/-- **The steps.** `record` writes a candidate root as known and leaves `current` alone; `accept`
    publishes a known root at a non-decreasing height and records it accepted; `boot` may republish an
    accepted root (or do nothing); a crash is a stutter. -/
def pubStep (s s' : PubState) : Prop :=
  (∃ r, s' = { s with known := insert r s.known })
  ∨ (∃ r h, r ∈ s.known ∧ h ≥ s.lastHeight
      ∧ s' = { s with current := some r, accepted := insert r s.accepted, lastHeight := h })
  ∨ (∃ r, r ∈ s.accepted ∧ s' = { s with current := some r })
  ∨ s' = s

/-- The system: a step is always enabled, because the stutter is one. -/
def pubSystem : System where
  State := PubState
  Step := pubStep
  enabled := fun _ => true
  enabled_iff := by
    intro σ
    constructor
    · intro _
      exact ⟨σ, Or.inr (Or.inr (Or.inr rfl))⟩
    · intro _
      rfl

/-- **The law, in one line.** Every reachable state's `current` — when it names a root at all — names a
    root some accepted block carried. Proved per step: `record` does not move `current`, `accept` takes
    the root from `known` and inserts it into `accepted`, `boot` may only take an accepted root, and a
    stutter changes nothing. -/
theorem a_current_root_is_an_accepted_state
    (σ : PubState) (hr : pubSystem.Reach pubInitial σ) {r : Nat} (h : σ.current = some r) :
    r ∈ σ.accepted := by
  induction hr with
  | refl => simp [pubInitial] at h
  | tail _ hstep ih =>
      unfold pubStep at hstep
      rcases hstep with ⟨r0, hrec⟩ | ⟨r0, _h, hmem, _hge, hacc⟩ | ⟨r0, hmem, hboot⟩ | hstut
      · rw [hrec] at h ⊢
        exact ih h
      · rw [hacc] at h ⊢
        have hr0 : r0 = r := Option.some.inj h
        subst hr0
        simp
      · rw [hboot] at h ⊢
        have hr0 : r0 = r := Option.some.inj h
        rw [← hr0]
        exact hmem
      · subst hstut
        exact ih h

/-- **A boot republishes only an accepted root.** The current pointer is either absent or accepted, so
    a restart that republishes the latest accepted post-state cannot mint a root the store did not
    accept. -/
theorem a_boot_republishes_only_an_accepted_root
    (σ : PubState) (hr : pubSystem.Reach pubInitial σ) :
    σ.current = none ∨ ∃ r, σ.current = some r ∧ r ∈ σ.accepted := by
  cases h : σ.current with
  | none => exact Or.inl rfl
  | some r => exact Or.inr ⟨r, rfl, a_current_root_is_an_accepted_state σ hr (r := r) h⟩

/-- **Non-vacuity: a root can be recorded, accepted and published.** So the law is a statement about
    something the machine reaches, not about a state nothing does. -/
def pubRecorded : PubState := ⟨none, {7}, ∅, 0⟩
def pubPublished : PubState := ⟨some 7, {7}, {7}, 0⟩

theorem publication_can_advance :
    ∃ σ, pubSystem.Reach pubInitial σ ∧ σ.current = some 7 ∧ 7 ∈ σ.accepted := by
  refine ⟨pubPublished, ?_, rfl, Finset.mem_singleton_self 7⟩
  have h1 : pubStep pubInitial pubRecorded := by
    refine Or.inl ⟨7, ?_⟩
    simp [pubInitial, pubRecorded]
  have h2 : pubStep pubRecorded pubPublished := by
    refine Or.inr (Or.inl ⟨7, 0, ?_, ?_, ?_⟩)
    · simp [pubRecorded]
    · unfold pubRecorded
      rfl
    · rfl
  exact Relation.ReflTransGen.tail (Relation.ReflTransGen.single h1) h2

/-- **The mutant the port has today**: `record` publishes the candidate root — the replay checkpoint at
    `casper/src/runtime_replay.rs:302` writes `CURRENT_ROOT` before `dag.insert`. Same machine, one
    disjunct changed. -/
def earlyPublishStep (s s' : PubState) : Prop :=
  (∃ r, s' = { s with known := insert r s.known, current := some r })
  ∨ (∃ r h, r ∈ s.known ∧ h ≥ s.lastHeight
      ∧ s' = { s with current := some r, accepted := insert r s.accepted, lastHeight := h })
  ∨ (∃ r, r ∈ s.accepted ∧ s' = { s with current := some r })
  ∨ s' = s

/-- **The falsifier, fired.** Under the port's order a `CURRENT_ROOT` is reachable naming a post-state
    no accepted block carried — the state a refused or dropped block leaves behind after its replay.
    `a_current_root_is_an_accepted_state` is exactly what rules it out. -/
theorem an_early_publish_reaches_an_orphan_root :
    ∃ σ, Relation.ReflTransGen earlyPublishStep pubInitial σ
      ∧ σ.current = some 7 ∧ 7 ∉ σ.accepted := by
  refine ⟨⟨some 7, {7}, ∅, 0⟩, ?_, rfl, ?_⟩
  · exact Relation.ReflTransGen.single (Or.inl ⟨7, by simp [pubInitial]⟩)
  · simp

end Rchain
