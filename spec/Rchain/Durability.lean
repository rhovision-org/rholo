import Rchain.Progress

/-!
# Law 72 — a completion record brackets every durable effect

**Why this module exists.** The node's self-reset applies **four** durable effects — it rewinds the DAG's
metadata above the anchor, deletes the dropped block bodies, moves the runtime's durable root to the
anchor's state, and cleans the **derived deploy and deployer indexes** of rows that point at blocks the
DAG no longer holds — and the record under its completion marker must bracket all four. The first two are
written by `rewind_and_purge` (`casper/src/engine/reset.rs:68`); the runtime root is written by the reset's
caller through `HistoryRepository::reset` → `roots_repository.validate_and_set_current_root`
(`rspace/src/history/history_repository.rs:337`); the derived indexes are cleaned by
`BlockDagKeyValueStorage::drop_above` (`casper/src/dag.rs:410`). A record written before any of them has
returned leaves a `Rewound` marker claiming "the stores agree" while a durable store does not — and the
boot guard, reading `Rewound`, re-applies **nothing** (`reset.rs:140-151`). The port already treats this
class as real elsewhere: `reset_volatile`'s doc (`history_repository.rs:346-358`) exists because a crash
between a merge's two commit points "leaves `CURRENT_ROOT` naming an ancestor … resumes from neither
state".

**The fourth effect and the port's own witness.** `drop_above` returns early when the metadata drop is
empty (`casper/src/dag.rs:414-416`), so a rewind that removed no metadata skips the index cleanup — and
the reset still records completion. That is the shape of `a_skipped_index_cleanup_reaches_a_record_that_lies`
below; it is the D2 finding (HAZOP row F-U17-11), and it is why the law names the indexes explicitly
rather than saying "every durable effect" and leaving the reader to guess the set.

**The law.** A completion record is a **bracket**, not a flag: it may be written only once *every* effect
it brackets has been applied. The model below is the port's own sequence — write the intent, apply the
effects in the port's forced order, record completion — with a boot repair that re-applies the whole set
from any partially-applied state (idempotent, which is why it is a single step). The headline theorem is
that **no reachable state has the record written while an effect is missing**, so the window the port has
today is not a race but a modelling error, and the fix is to move the record's write, not to add a guard.

**The falsifiers are the port's current orders.** `earlyCommitStep` records completion once the rewind and
the purge have returned, without waiting for the root — the shape the port had before the first half of
this law landed; `skippedIndexStep` is the same machine with the index effect optional, the shape the
early return admits today. Each exhibits a reachable `Rewound` record with an effect missing. A law whose
statement the current code satisfies would be the register's `vacuous`; these are red on the code as it
stands, which is what makes it a finding.
-/

namespace Rchain

/-- The phase of a reset's completion record. `idle` — no record; `rewinding` — the intent is durable and
    the effects may be partially applied; `rewound` — the completion is recorded. -/
inductive ResetPhase where
  | idle
  | rewinding
  | rewound
deriving DecidableEq, Repr

/-- A reset's durable state: the completion record's phase and the four effects the port applies —
    `rewound` (the DAG's metadata above the anchor is gone), `purged` (the dropped block bodies are
    deleted), `rooted` (the runtime's durable root names the anchor's state), and `indexed` (the derived
    deploy and deployer indexes hold no row for a dropped block). -/
structure ResetState where
  phase : ResetPhase
  rewound : Bool
  purged : Bool
  rooted : Bool
  indexed : Bool
deriving DecidableEq, Repr

/-- The state a node is in before it has ever reset: no record, no effect. -/
def resetInitial : ResetState := ⟨.idle, false, false, false, false⟩

/-- **The steps of a reset, in the port's order.** The intent is written before any effect (`start`); the
    four effects follow in the order the port forces them — the rewind, then the purge whose input is the
    rewind's output, then the runtime root, then the derived-index cleanup; the completion is recorded
    **only after every effect has returned** (`commit`); and the boot guard re-applies the whole set from
    any partially-applied state (`repair`), which is why it is one step that sets all four at once. -/
def resetStep (s s' : ResetState) : Prop :=
  (s.phase = .idle ∧ s' = { s with phase := .rewinding })
  ∨ (s.phase = .rewinding ∧ s.rewound = false ∧ s' = { s with rewound := true })
  ∨ (s.phase = .rewinding ∧ s.rewound = true ∧ s.purged = false ∧ s' = { s with purged := true })
  ∨ (s.phase = .rewinding ∧ s.purged = true ∧ s.rooted = false ∧ s' = { s with rooted := true })
  ∨ (s.phase = .rewinding ∧ s.rooted = true ∧ s.indexed = false ∧ s' = { s with indexed := true })
  ∨ (s.phase = .rewinding ∧ s.rewound = true ∧ s.purged = true ∧ s.rooted = true
        ∧ s.indexed = true ∧ s' = { s with phase := .rewound })
  ∨ (s.phase = .rewinding
        ∧ s' = { s with rewound := true, purged := true, rooted := true, indexed := true })

/-- A step is available while the reset is in progress: from `idle` the intent can be written, from
    `rewinding` either the next effect, the completion, or the repair; from `rewound` nothing — the reset
    is done. So `enabled` is exactly "the record does not yet say `rewound`". -/
def resetEnabled (s : ResetState) : Bool :=
  match s.phase with
  | .idle => true
  | .rewinding => true
  | .rewound => false

theorem resetEnabled_iff_phase (s : ResetState) :
    resetEnabled s = true ↔ s.phase ≠ .rewound := by
  cases hp : s.phase <;> simp [resetEnabled, hp]

/-- **The reset, as a `System`** — the four effects under one completion record. -/
def resetSystem : System where
  State := ResetState
  Step := resetStep
  enabled := resetEnabled
  enabled_iff := by
    intro σ
    constructor
    · intro h
      have hne : σ.phase ≠ .rewound := (resetEnabled_iff_phase σ).mp h
      cases hp : σ.phase
      · exact ⟨_, Or.inl ⟨hp, rfl⟩⟩
      · -- From `rewinding` the repair step is always available; naming it keeps the witness
        -- independent of which effects have landed.
        exact ⟨_, Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (Or.inr ⟨hp, rfl⟩)))))⟩
      · exact absurd hp hne
    · rintro ⟨σ', hs⟩
      rcases hs with ⟨h1, _⟩ | ⟨h1, _, _⟩ | ⟨h1, _, _, _⟩ | ⟨h1, _, _, _⟩ | ⟨h1, _, _, _⟩
        | ⟨h1, _, _, _, _, _⟩ | ⟨h1, _⟩
      all_goals simp [resetEnabled, h1]

/-- The step lemma the law rests on: a step whose *target* has the record written has every effect
    applied. The completion step (`commit`) is the only one that can write `rewound`, and it demands all
    four effects; every other step leaves the phase as it found it. -/
theorem step_target_has_completion (s s' : ResetState) (hs : resetStep s s') (hph : s'.phase = .rewound) :
    s'.rewound = true ∧ s'.purged = true ∧ s'.rooted = true ∧ s'.indexed = true := by
  unfold resetStep at hs
  rcases hs with ⟨h1, h2⟩ | ⟨h1, h2, h3⟩ | ⟨h1, h2, h3, h4⟩ | ⟨h1, h2, h3, h4⟩
    | ⟨h1, h2, h3, h4⟩ | ⟨h1, h2, h3, h4, h5, h6⟩ | ⟨h1, h2⟩
  · rw [h2] at hph; exact absurd hph (by simp [h1])
  · rw [h3] at hph; exact absurd hph (by simp [h1])
  · rw [h4] at hph; exact absurd hph (by simp [h1])
  · rw [h4] at hph; exact absurd hph (by simp [h1])
  · rw [h4] at hph; exact absurd hph (by simp [h1])
  · rw [h6, h2, h3, h4, h5] at hph ⊢; exact ⟨rfl, rfl, rfl, rfl⟩
  · rw [h2] at hph; exact absurd hph (by simp [h1])

/-- **The law, in one line.** Every reachable state whose record says `rewound` has **every** effect
    applied — the rewind, the purge, the runtime root, and the derived-index cleanup. Proved per step
    rather than by induction on the trace: the only step that writes `rewound` requires all four effects
    first, and no step ever moves an effect back to `false`, so the target of any step satisfies the
    conclusion outright. -/
theorem a_rewound_record_implies_every_effect_applied
    (σ : ResetState) (hr : resetSystem.Reach resetInitial σ) (hph : σ.phase = .rewound) :
    σ.rewound = true ∧ σ.purged = true ∧ σ.rooted = true ∧ σ.indexed = true := by
  induction hr with
  | refl => exact absurd hph (by simp [resetInitial])
  | tail _ hstep _ => exact step_target_has_completion _ _ hstep hph

/-- **Non-vacuity: a reset can complete.** The correct system does reach a `Rewound` record with all
    four effects — so the law above is a statement about something, not about a state nothing reaches. -/
theorem a_reset_can_complete :
    ∃ σ, resetSystem.Reach resetInitial σ ∧ σ.phase = .rewound
      ∧ σ.rewound = true ∧ σ.purged = true ∧ σ.rooted = true ∧ σ.indexed = true := by
  refine ⟨⟨.rewound, true, true, true, true⟩, ?_, rfl, rfl, rfl, rfl, rfl⟩
  have h1 : resetStep resetInitial ⟨.rewinding, false, false, false, false⟩ := Or.inl ⟨rfl, rfl⟩
  have h2 : resetStep ⟨.rewinding, false, false, false, false⟩ ⟨.rewinding, true, false, false, false⟩ :=
    Or.inr (Or.inl ⟨rfl, rfl, rfl⟩)
  have h3 : resetStep ⟨.rewinding, true, false, false, false⟩ ⟨.rewinding, true, true, false, false⟩ :=
    Or.inr (Or.inr (Or.inl ⟨rfl, rfl, rfl, rfl⟩))
  have h4 : resetStep ⟨.rewinding, true, true, false, false⟩ ⟨.rewinding, true, true, true, false⟩ :=
    Or.inr (Or.inr (Or.inr (Or.inl ⟨rfl, rfl, rfl, rfl⟩)))
  have h5 : resetStep ⟨.rewinding, true, true, true, false⟩ ⟨.rewinding, true, true, true, true⟩ :=
    Or.inr (Or.inr (Or.inr (Or.inr (Or.inl ⟨rfl, rfl, rfl, rfl⟩))))
  have h6 : resetStep ⟨.rewinding, true, true, true, true⟩ ⟨.rewound, true, true, true, true⟩ :=
    Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (Or.inl ⟨rfl, rfl, rfl, rfl, rfl, rfl⟩)))))
  exact Relation.ReflTransGen.tail
    (Relation.ReflTransGen.tail
      (Relation.ReflTransGen.tail
        (Relation.ReflTransGen.tail
          (Relation.ReflTransGen.tail
            (Relation.ReflTransGen.single h1) h2) h3) h4) h5) h6

/-- **The mutant the port had before the root half of this law**: the completion is recorded once the
    rewind and the purge have returned, without waiting for the runtime's root. Same machine, one
    disjunct changed. -/
def earlyCommitStep (s s' : ResetState) : Prop :=
  (s.phase = .idle ∧ s' = { s with phase := .rewinding })
  ∨ (s.phase = .rewinding ∧ s.rewound = false ∧ s' = { s with rewound := true })
  ∨ (s.phase = .rewinding ∧ s.rewound = true ∧ s.purged = false ∧ s' = { s with purged := true })
  ∨ (s.phase = .rewinding ∧ s.rewound = true ∧ s.purged = true ∧ s' = { s with phase := .rewound })

/-- **The root falsifier, fired.** Under the port's earlier order a `Rewound` record is reachable with
    the runtime root **unmoved** — the state `resume_after_reset` reads as "the stores agree" and repairs
    nothing. It is `decide`-able and it was red on the code as it stood, which is the register's
    `falsifiable` for this law made a theorem. -/
theorem an_early_commit_reaches_a_record_that_lies :
    ∃ σ, Relation.ReflTransGen earlyCommitStep resetInitial σ
      ∧ σ.phase = .rewound ∧ σ.rooted = false := by
  refine ⟨⟨.rewound, true, true, false, false⟩, ?_, rfl, rfl⟩
  have h1 : earlyCommitStep resetInitial ⟨.rewinding, false, false, false, false⟩ := Or.inl ⟨rfl, rfl⟩
  have h2 : earlyCommitStep ⟨.rewinding, false, false, false, false⟩ ⟨.rewinding, true, false, false, false⟩ :=
    Or.inr (Or.inl ⟨rfl, rfl, rfl⟩)
  have h3 : earlyCommitStep ⟨.rewinding, true, false, false, false⟩ ⟨.rewinding, true, true, false, false⟩ :=
    Or.inr (Or.inr (Or.inl ⟨rfl, rfl, rfl, rfl⟩))
  have h4 : earlyCommitStep ⟨.rewinding, true, true, false, false⟩ ⟨.rewound, true, true, false, false⟩ :=
    Or.inr (Or.inr (Or.inr ⟨rfl, rfl, rfl, rfl⟩))
  exact Relation.ReflTransGen.tail
    (Relation.ReflTransGen.tail
      (Relation.ReflTransGen.tail (Relation.ReflTransGen.single h1) h2) h3) h4

/-- **The index falsifier: the shape the port admits today.** The same machine with the commit step
    **not** requiring `indexed` — the early return at `casper/src/dag.rs:414-416`, where a rewind whose
    metadata drop is empty skips the derived-index cleanup and the reset records completion anyway. The
    reachable state is a `Rewound` record with `indexed = false`; `a_rewound_record_implies_every_effect_applied`
    is exactly what rules it out. -/
def skippedIndexStep (s s' : ResetState) : Prop :=
  (s.phase = .idle ∧ s' = { s with phase := .rewinding })
  ∨ (s.phase = .rewinding ∧ s.rewound = false ∧ s' = { s with rewound := true })
  ∨ (s.phase = .rewinding ∧ s.rewound = true ∧ s.purged = false ∧ s' = { s with purged := true })
  ∨ (s.phase = .rewinding ∧ s.purged = true ∧ s.rooted = false ∧ s' = { s with rooted := true })
  ∨ (s.phase = .rewinding ∧ s.rooted = true ∧ s.indexed = false ∧ s' = { s with indexed := true })
  ∨ (s.phase = .rewinding ∧ s.rewound = true ∧ s.purged = true ∧ s.rooted = true
        ∧ s' = { s with phase := .rewound })
  ∨ (s.phase = .rewinding
        ∧ s' = { s with rewound := true, purged := true, rooted := true, indexed := true })

theorem a_skipped_index_cleanup_reaches_a_record_that_lies :
    ∃ σ, Relation.ReflTransGen skippedIndexStep resetInitial σ
      ∧ σ.phase = .rewound ∧ σ.indexed = false := by
  refine ⟨⟨.rewound, true, true, true, false⟩, ?_, rfl, rfl⟩
  have h1 : skippedIndexStep resetInitial ⟨.rewinding, false, false, false, false⟩ := Or.inl ⟨rfl, rfl⟩
  have h2 : skippedIndexStep ⟨.rewinding, false, false, false, false⟩ ⟨.rewinding, true, false, false, false⟩ :=
    Or.inr (Or.inl ⟨rfl, rfl, rfl⟩)
  have h3 : skippedIndexStep ⟨.rewinding, true, false, false, false⟩ ⟨.rewinding, true, true, false, false⟩ :=
    Or.inr (Or.inr (Or.inl ⟨rfl, rfl, rfl, rfl⟩))
  have h4 : skippedIndexStep ⟨.rewinding, true, true, false, false⟩ ⟨.rewinding, true, true, true, false⟩ :=
    Or.inr (Or.inr (Or.inr (Or.inl ⟨rfl, rfl, rfl, rfl⟩)))
  have h5 : skippedIndexStep ⟨.rewinding, true, true, true, false⟩ ⟨.rewound, true, true, true, false⟩ :=
    Or.inr (Or.inr (Or.inr (Or.inr (Or.inr (Or.inl ⟨rfl, rfl, rfl, rfl, rfl⟩)))))
  exact Relation.ReflTransGen.tail
    (Relation.ReflTransGen.tail
      (Relation.ReflTransGen.tail
        (Relation.ReflTransGen.tail (Relation.ReflTransGen.single h1) h2) h3) h4) h5

end Rchain
