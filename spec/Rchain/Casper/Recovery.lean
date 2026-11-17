import Rchain.Casper.Validate
import Mathlib.Data.Finset.Basic

/-!
# Law 94 — a local fault has a bounded exit

The 2026-11-08 incident (#564) was not a cost-arithmetic incident. A validator built block #1115,
its own replay refused it, and the node rebuilt the **same deterministic block** for ~45 hours,
never broadcast it, and capped finality at #1110. The refusal was reproducible, so the retry could
not succeed; the proposer had no transition that changed the input, produced an attestation, reset to
a finalised anchor, or halted every trigger. That is a recovery-state defect, not a replay defect.

**The law.** From the refused state, the model has **no transition back to itself**: a local fault
either changes the proposal input (the refused set is held out; the immediate rebuild is an
attestation with no user deploy), moves to a reset of a **finalised** anchor, or halts. An
unrecognised transition is a halt, which is the safe fallback. Every fault therefore reaches a safe
state in one step, and "retry the unchanged input" has no representation to write.

**The way back.** A reset is not a terminal state. Once the catch-up walk has passed the finalised
anchor it rewound to, the fault is spent and the node returns to `.healthy` by
`resumeAfterCatchup`; the operator's explicit resume remains the fallback when no reset was named
(no finalised anchor), when the reset was disabled, or when the anchor has already had its one
attempt.

**The state-derived layer.** The five-state ladder says the fault has an exit. The row also claims
the content of that exit, and those claims are modelled here against the Rust: the latch is a state
a transition produces; the four proposal sources each read it; the resume is
`self_replay_resume_is_due` (`casper/src/engine/reset.rs`), not a pair of free Bools; the reset is
`reset_is_recurrence` (`casper/src/blocks/proposer/proposer.rs`), an anchor identity plus one
attempt; and a transition is observable only after its durable record.

**What it does not claim.** It does not model the replay's cause (that is RC-B, open), the cost/state
ordering (law 93), or the storage-level crash bracket (laws 72/73/76). It says only that the fault
state has an exit, that the exit is not a no-op, and that the exit's predicates are the ones the
Rust runs. The Rust witnesses are the proposer's recovery/halt tests and the durable-record codec.

Layer `Casper`.
-/

namespace Rchain

/-- The recovery state of a proposer after its own block fails replay. -/
inductive RecoveryState where
  | healthy
  | refused
  | recovering
  | resetRequested
  | halted
deriving DecidableEq, Repr

/-- What the proposer may do next. `retrySameInput` is deliberately **not a constructor**: the state
the incident reached has no representation to write. -/
inductive RecoveryStep where
  | changeInput
  | attestOnly
  | resetFinalised
  /-- The reset completed and the catch-up walk passed the finalised anchor: the fault is spent. -/
  | resumeAfterCatchup
  | halt
deriving DecidableEq, Repr

/-- The transition. An unrecognised step is a halt — the safe fallback, not a retry. -/
def nextState : RecoveryState → RecoveryStep → RecoveryState
  | .refused, .changeInput => .recovering
  | .refused, .attestOnly => .recovering
  | .recovering, .resetFinalised => .resetRequested
  | .resetRequested, .resumeAfterCatchup => .healthy
  | .recovering, .halt => .halted
  | _, _ => .halted

/-- The states a fault transition may land in. -/
def RecoverySafe : RecoveryState → Prop
  | .recovering => True
  | .resetRequested => True
  | .halted => True
  | _ => False

/-- **No step retries the refused state.** The incident's loop has no transition. -/
theorem no_unchanged_retry (s : RecoveryState) (step : RecoveryStep) :
    s = .refused → nextState s step ≠ .refused := by
  intro h
  subst h
  cases step <;> decide

/-- **Every fault reaches a safe state in one transition.** -/
theorem every_fault_reaches_safe (step : RecoveryStep) :
    RecoverySafe (nextState .refused step) := by
  cases step <;> trivial

/-- **A completed reset resumes to healthy.** The step is the automatic post-catch-up resume; the
Rust predicate (`self_replay_resume_is_due`) admits it only when the walk's frontier has passed the
finalised anchor and the record names that anchor. -/
theorem a_caught_up_reset_resumes :
    nextState .resetRequested .resumeAfterCatchup = .healthy := rfl

/-- **A resume is not a way out of the fault itself.** It is admitted only from `resetRequested`,
so it cannot be used to skip the reset rung. -/
theorem resume_requires_a_reset :
    nextState .refused .resumeAfterCatchup = .halted := rfl

/-- A reset is admitted only when its anchor is finalised. -/
def resetAdmitted (anchorFinalised : Bool) : Prop := anchorFinalised = true

/-- **A non-finalised anchor is refused.** -/
theorem a_non_finalised_anchor_is_refused : ¬ resetAdmitted false := by
  unfold resetAdmitted
  decide

/-- **A finalised anchor is admitted.** -/
theorem a_finalised_anchor_is_admitted : resetAdmitted true := rfl

/-! ## The recovery ladder's concrete arms

The five-state ladder above says the fault has an exit. The row also claims the **content** of
the exit: the attestation-only rebuild carries no user deploy, the refused set is held out, the
latch is read by every trigger, the durable record precedes the observable transition, the reset
anchor is finalised and attempted once, and the resume waits for catch-up. Those claims are
modelled here, without replacing the ladder or its theorem names. -/

/-- A deploy identity. The model needs only equality. -/
abbrev Deploy := Nat

/-- The proposal input the refused build is rebuilt from. -/
structure Input where
  deploys : Finset Deploy
  preState : Nat
  code : Nat
deriving DecidableEq

/-- The attestation-only rebuild: the same pre-state and code, no user deploy. -/
def attestOnly (i : Input) : Input := { i with deploys := ∅ }

theorem the_attest_only_rebuild_has_no_user_deploy (i : Input) :
    (attestOnly i).deploys = ∅ := rfl

/-- The change-input step holds the refused set out of the rebuild. -/
def changeInput (i : Input) (refused : Finset Deploy) : Input :=
  { i with deploys := i.deploys \ refused }

theorem the_refused_deploy_is_not_in_the_rebuild (i : Input) (refused : Finset Deploy) :
    ∀ d ∈ refused, d ∉ (changeInput i refused).deploys := by
  intro d hd
  simp [changeInput, hd]

/-- The tracker's key: two attempts with the same pre-state, deploy set and code are the same
    attempt, not a new one. -/
structure AttemptKey where
  preState : Nat
  deploys : Finset Deploy
  code : Nat
deriving DecidableEq

def keyOf (i : Input) : AttemptKey :=
  { preState := i.preState, deploys := i.deploys, code := i.code }

theorem a_retry_of_the_same_key_is_not_a_new_attempt (i j : Input) (h : keyOf i = keyOf j) :
    i = j := by
  cases i with
  | mk di pi ci =>
    cases j with
    | mk dj pj cj =>
      simp [keyOf] at h
      obtain ⟨hp, hd, hc⟩ := h
      subst hp; subst hd; subst hc; rfl

/-! ## The latch is a state a transition produces -/

/-- The proposer's latch. `halt` sets it; only the post-catch-up resume clears it. -/
inductive LatchState where
  | clear
  | latched
deriving DecidableEq, Repr

/-- **The latch transition.** A halt latches; a resume clears a latched state; every other step
leaves the latch alone. -/
def latchAfter : LatchState → RecoveryStep → LatchState
  | _, .halt => .latched
  | .latched, .resumeAfterCatchup => .clear
  | s, _ => s

theorem a_halt_latches (s : LatchState) :
    latchAfter s .halt = .latched := by
  cases s <;> rfl

theorem a_resume_clears_the_latch :
    latchAfter .latched .resumeAfterCatchup = .clear := rfl

theorem a_non_resume_step_does_not_clear_the_latched_state
    (step : RecoveryStep) (h : step ≠ .resumeAfterCatchup) :
    latchAfter .latched step = .latched := by
  cases step <;> simp [latchAfter] at h ⊢

/-! ## The four proposal sources read the latch

The Rust's latch check sits at the top of `propose`; its comment names the four arrivals: the
autopropose timer, the attest-on-new-blocks tap, the not-due retry and an explicit propose. Each
arrival is a path with its own fire predicate; the latch is the second conjunct, not an argument
the path is free to ignore. -/

/-- The four sources the Rust's proposer comment names. -/
inductive Trigger where
  | autoproposeTimer
  | attestOnNewBlocks
  | notDueRetry
  | explicitPropose
deriving DecidableEq, Repr

/-- The source list, as data, so coverage is a theorem rather than a comment. -/
def rustTriggerSources : List Trigger :=
  [.autoproposeTimer, .attestOnNewBlocks, .notDueRetry, .explicitPropose]

/-- A trigger path: the source and whether its own precondition fired. -/
structure TriggerPath where
  source : Trigger
  fires : Bool
deriving DecidableEq, Repr

/-- **The admission predicate.** A path admits only when it fires **and** the latch is clear. -/
def admitted (p : TriggerPath) (l : LatchState) : Bool :=
  p.fires && decide (l = .clear)

/-- The compatibility predicate the register already names. -/
def triggerAdmitted (halted : Bool) (t : Trigger) : Bool :=
  admitted ⟨t, true⟩ (if halted then .latched else .clear)

theorem the_four_triggers_are_the_rust_sources (t : Trigger) :
    t ∈ rustTriggerSources := by
  cases t <;> simp [rustTriggerSources]

theorem every_trigger_reads_the_latch (p : TriggerPath) :
    admitted p .latched = false := by
  simp [admitted]

theorem a_halted_proposer_admits_no_trigger (t : Trigger) :
    triggerAdmitted true t = false := by
  simp [triggerAdmitted, admitted]

/-- The defect shape: a path that does not read the latch. -/
def triggerSkipsLatch (_halted : Bool) (_t : Trigger) : Bool := true

theorem a_trigger_that_skips_the_latch_admits_a_block (t : Trigger) :
    triggerSkipsLatch true t = true := rfl

/-- **The exact defect the latch forbids**: the explicit-propose path firing while latched. -/
def explicitProposeSkipsLatch (p : TriggerPath) : Bool :=
  match p.source with
  | .explicitPropose => p.fires
  | _ => admitted p .latched

theorem the_latch_skipping_path_admits :
    explicitProposeSkipsLatch ⟨.explicitPropose, true⟩ = true := rfl

/-! ## The resume is `self_replay_resume_is_due` -/

/-- The reset marker's phase, as the Rust enum carries it. -/
inductive ResumePhase where
  | pending
  | rewound
deriving DecidableEq, Repr

/-- The reset marker: the phase, the anchor and the height the reset rewound to. -/
structure ResetMarker where
  phase : ResumePhase
  anchor : Nat
  height : Nat
deriving DecidableEq, Repr

/-- The self-replay record's stage, as the Rust enum carries it. -/
inductive SelfReplayStage where
  | refused
  | recovered
  | resetRequested
  | halted
deriving DecidableEq, Repr

/-- The durable self-replay record. -/
structure SelfReplayRecord where
  stage : SelfReplayStage
  resetAttempted : Bool
  resetAnchor : Option Nat
deriving DecidableEq, Repr

/-- The resume's state: the marker, the catch-up frontier and the record. -/
structure ResumeState where
  marker : ResetMarker
  frontier : Nat
  record : Option SelfReplayRecord
deriving DecidableEq, Repr

/-- **`self_replay_resume_is_due`**, as the Rust spells it: the marker is `Rewound`, the frontier
has strictly passed the marker's height, and the record is present, in a reset/halt stage, has had
its attempt, and names the marker's anchor. -/
def resetResumeIsDue (s : ResumeState) : Bool :=
  decide (s.marker.phase = .rewound)
    && decide (s.marker.height < s.frontier)
    && (match s.record with
        | none => false
        | some r =>
            (r.stage == .resetRequested || r.stage == .halted)
              && r.resetAttempted
              && (r.resetAnchor == some s.marker.anchor))

def resumeAdmitted (s : ResumeState) : Bool := resetResumeIsDue s

theorem resume_requires_catch_up (s : ResumeState) (h : s.frontier ≤ s.marker.height) :
    resumeAdmitted s = false := by
  have hlt : decide (s.marker.height < s.frontier) = false := by
    simp [Nat.not_lt.mpr h]
  unfold resumeAdmitted resetResumeIsDue
  rw [hlt]
  cases decide (s.marker.phase = .rewound) <;> simp

theorem resume_requires_a_rewound_marker (s : ResumeState) (h : s.marker.phase ≠ .rewound) :
    resumeAdmitted s = false := by
  have hp : decide (s.marker.phase = .rewound) = false := by
    simp [h]
  unfold resumeAdmitted resetResumeIsDue
  rw [hp]
  simp

/-- The register's existing name: a finalised anchor is the `Rewound` marker. -/
theorem resume_requires_a_finalised_anchor (s : ResumeState) (h : s.marker.phase ≠ .rewound) :
    resumeAdmitted s = false :=
  resume_requires_a_rewound_marker s h

theorem resume_requires_a_record (s : ResumeState) (h : s.record = none) :
    resumeAdmitted s = false := by
  unfold resumeAdmitted resetResumeIsDue
  rw [h]
  simp

theorem resume_requires_the_reset_attempt (s : ResumeState) (r : SelfReplayRecord)
    (hr : s.record = some r) (h : r.resetAttempted = false) :
    resumeAdmitted s = false := by
  unfold resumeAdmitted resetResumeIsDue
  rw [hr]
  simp [h]

theorem resume_requires_the_same_anchor (s : ResumeState) (r : SelfReplayRecord)
    (hr : s.record = some r) (h : r.resetAnchor ≠ some s.marker.anchor) :
    resumeAdmitted s = false := by
  have hb : (r.resetAnchor == some s.marker.anchor) = false := by
    cases hEq : (r.resetAnchor == some s.marker.anchor)
    · rfl
    · exact absurd ((beq_iff_eq _ _).mp hEq) h
  unfold resumeAdmitted resetResumeIsDue
  rw [hr]
  simp [hb]

theorem a_resume_below_the_anchor_is_refused (s : ResumeState) (h : s.frontier ≤ s.marker.height) :
    resumeAdmitted s = false :=
  resume_requires_catch_up s h

theorem a_marker_from_another_anchors_reset_does_not_resume (s : ResumeState)
    (r : SelfReplayRecord) (hr : s.record = some r) (h : r.resetAnchor ≠ some s.marker.anchor) :
    resumeAdmitted s = false :=
  resume_requires_the_same_anchor s r hr h

/-! ## The reset is `reset_is_recurrence` -/

/-- The reset attempt as the Rust record stores it: one flag and one anchor slot. -/
structure ResetAttemptRecord where
  resetAttempted : Bool
  resetAnchor : Option Nat
deriving DecidableEq, Repr

/-- **`reset_is_recurrence`**: the attempt flag is set and the recorded anchor is this anchor. -/
def resetIsRecurrence (r : ResetAttemptRecord) (anchor : Nat) : Bool :=
  r.resetAttempted && (r.resetAnchor == some anchor)

/-- A reset names an anchor, the anchor's height, the node's finality, and the one attempt. -/
structure Reset where
  anchor : Nat
  anchorHeight : Nat
  finality : Nat
  attempted : Bool
  attemptedAnchor : Option Nat
deriving DecidableEq, Repr

/-- The anchor is finalised when its height is at or below the node's finality. -/
def resetAnchorFinalised (r : Reset) : Bool :=
  decide (r.anchorHeight ≤ r.finality)

/-- **The reset gate**: the anchor is finalised and this anchor has not had its attempt. -/
def canReset (r : Reset) : Bool :=
  resetAnchorFinalised r &&
    !(resetIsRecurrence { resetAttempted := r.attempted, resetAnchor := r.attemptedAnchor }
        r.anchor)

theorem the_reset_anchor_is_finalised (r : Reset) (h : canReset r = true) :
    r.anchorHeight ≤ r.finality := by
  unfold canReset at h
  simp only [Bool.and_eq_true] at h
  simpa [resetAnchorFinalised] using h.1

theorem at_most_one_reset_per_anchor (r : Reset)
    (h : resetIsRecurrence { resetAttempted := r.attempted, resetAnchor := r.attemptedAnchor }
          r.anchor = true) :
    canReset r = false := by
  simp [canReset, h]

theorem a_second_reset_at_the_same_anchor_is_refused (r : Reset)
    (h : r.attempted = true) (ha : r.attemptedAnchor = some r.anchor) :
    canReset r = false := by
  have hr : resetIsRecurrence { resetAttempted := r.attempted, resetAnchor := r.attemptedAnchor }
      r.anchor = true := by
    simp [resetIsRecurrence, h, ha]
  simp [canReset, hr]

theorem a_reset_at_a_different_anchor_is_not_a_recurrence (r : ResetAttemptRecord)
    (anchor : Nat) (h : r.resetAnchor ≠ some anchor) :
    resetIsRecurrence r anchor = false := by
  have hb : (r.resetAnchor == some anchor) = false := by
    cases hEq : (r.resetAnchor == some anchor)
    · rfl
    · exact absurd ((beq_iff_eq _ _).mp hEq) h
  simp [resetIsRecurrence, hb]

theorem a_recurrence_names_the_attempted_anchor (r : ResetAttemptRecord) (anchor : Nat)
    (h : resetIsRecurrence r anchor = true) :
    r.resetAnchor = some anchor := by
  unfold resetIsRecurrence at h
  simp only [Bool.and_eq_true] at h
  exact (beq_iff_eq _ _).mp h.2

/-- Raising the reset records the anchor and consumes the one attempt. -/
def raiseReset (r : Reset) : Reset :=
  { r with attempted := true, attemptedAnchor := some r.anchor }

theorem raiseReset_records_the_anchor (r : Reset) :
    (raiseReset r).attempted = true ∧ (raiseReset r).attemptedAnchor = some r.anchor := by
  simp [raiseReset]

/-! ## The observable transition is recorded first

The Rust writes the durable record before it sets the halt observable. The trace model makes that
order a property of the sequence, not a free `recorded` flag on one step. -/

/-- A transition is observable only after its durable record. -/
structure Transition where
  recorded : Bool
  next : RecoveryState
deriving DecidableEq, Repr

def transitionStep (t : Transition) : Option RecoveryState :=
  if t.recorded then some t.next else none

theorem an_observable_halt_has_a_durable_record (t : Transition)
    (h : transitionStep t = some .halted) : t.recorded = true := by
  unfold transitionStep at h
  split at h
  · assumption
  · simp at h

theorem a_record_after_the_transition_is_not_a_bracket :
    transitionStep { recorded := false, next := .halted } = none := rfl

/-- One item of the record/transition trace. -/
inductive TraceItem where
  | write (stage : SelfReplayStage)
  | step (step : RecoveryStep)
deriving DecidableEq, Repr

/-- Walk the trace with the "a write is outstanding" flag. A step consumes the write; a step with
no outstanding write is not well formed. -/
def wellFormedFrom : Bool → List TraceItem → Bool
  | _, [] => true
  | _, .write _ :: rest => wellFormedFrom true rest
  | seen, .step _ :: rest => seen && wellFormedFrom false rest

def wellFormedTrace (items : List TraceItem) : Bool := wellFormedFrom false items

/-- The no-anchor halt: refused and halted records, then the halt step. -/
def noAnchorTrace : List TraceItem :=
  [.write .refused, .write .halted, .step .halt]

/-- The reset path: refused, reset-requested, the reset step, halted, the halt step. -/
def resetTrace : List TraceItem :=
  [.write .refused, .write .resetRequested, .step .resetFinalised,
   .write .halted, .step .halt]

/-- The defect shape: a step before any write. -/
def badTrace : List TraceItem := [.step .halt, .write .halted]

theorem a_well_formed_trace_has_no_step_before_a_write (step : RecoveryStep) :
    wellFormedTrace [.step step] = false := rfl

theorem the_no_anchor_trace_records_before_the_halt :
    wellFormedTrace noAnchorTrace = true := by
  decide

theorem the_reset_trace_records_before_each_transition :
    wellFormedTrace resetTrace = true := by
  decide

theorem the_bad_trace_is_not_well_formed :
    wellFormedTrace badTrace = false := by
  decide

/-! ## The mutant and the statement -/

/-- **Law 94's mutant**: the incident's unchanged retry, represented as a step. -/
inductive RecoveryStepMutant where
  | retrySameInput
  | other
deriving DecidableEq, Repr

def nextStateMutant : RecoveryState → RecoveryStepMutant → RecoveryState
  | .refused, .retrySameInput => .refused
  | _, _ => .halted

/-- The bounded-recovery property the mutant violates. -/
def BoundedRecovery (next : RecoveryState → RecoveryStepMutant → RecoveryState) : Prop :=
  ∀ step, next .refused step ≠ .refused

def law94_mutant : Prop :=
  nextStateMutant .refused .retrySameInput = .refused

/-- **The falsifier, fired**: the mutant reaches the refused state from itself **and** violates
the bounded-recovery property. -/
theorem law94_falsifier : law94_mutant ∧ ¬ BoundedRecovery nextStateMutant := by
  constructor
  · rfl
  · intro h
    exact (h .retrySameInput) rfl

theorem law94_trace_falsifier : wellFormedTrace badTrace = false := the_bad_trace_is_not_well_formed

/-- The state-derived layer the row claims: the latch, the four sources, the resume, the reset and
the record order. -/
def law94_recovery_layer : Prop :=
  (∀ s : LatchState, latchAfter s .halt = .latched) ∧
  (latchAfter .latched .resumeAfterCatchup = .clear) ∧
  (∀ (step : RecoveryStep), step ≠ .resumeAfterCatchup →
    latchAfter .latched step = .latched) ∧
  (∀ t : Trigger, t ∈ rustTriggerSources) ∧
  (∀ p : TriggerPath, admitted p .latched = false) ∧
  (∀ s : ResumeState, s.frontier ≤ s.marker.height → resumeAdmitted s = false) ∧
  (∀ s : ResumeState, s.marker.phase ≠ .rewound → resumeAdmitted s = false) ∧
  (∀ s : ResumeState, s.record = none → resumeAdmitted s = false) ∧
  (∀ (s : ResumeState) (r : SelfReplayRecord), s.record = some r →
    r.resetAttempted = false → resumeAdmitted s = false) ∧
  (∀ (s : ResumeState) (r : SelfReplayRecord), s.record = some r →
    r.resetAnchor ≠ some s.marker.anchor → resumeAdmitted s = false) ∧
  (∀ r : Reset, canReset r = true → r.anchorHeight ≤ r.finality) ∧
  (∀ r : Reset,
    resetIsRecurrence { resetAttempted := r.attempted, resetAnchor := r.attemptedAnchor }
      r.anchor = true → canReset r = false) ∧
  (∀ r : Reset, r.attempted = true → r.attemptedAnchor = some r.anchor → canReset r = false) ∧
  (∀ r : ResetAttemptRecord, ∀ anchor : Nat, resetIsRecurrence r anchor = true →
    r.resetAnchor = some anchor) ∧
  (wellFormedTrace noAnchorTrace = true) ∧
  (wellFormedTrace resetTrace = true) ∧
  (wellFormedTrace badTrace = false)

theorem law94_recovery_layer_proof : law94_recovery_layer := by
  refine ⟨a_halt_latches, a_resume_clears_the_latch, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_,
    ?_, ?_, ?_, ?_⟩
  · intro step h
    exact a_non_resume_step_does_not_clear_the_latched_state step h
  · intro t
    exact the_four_triggers_are_the_rust_sources t
  · intro p
    exact every_trigger_reads_the_latch p
  · intro s h
    exact resume_requires_catch_up s h
  · intro s h
    exact resume_requires_a_rewound_marker s h
  · intro s h
    exact resume_requires_a_record s h
  · intro s r hr h
    exact resume_requires_the_reset_attempt s r hr h
  · intro s r hr h
    exact resume_requires_the_same_anchor s r hr h
  · intro r h
    exact the_reset_anchor_is_finalised r h
  · intro r h
    exact at_most_one_reset_per_anchor r h
  · intro r h ha
    exact a_second_reset_at_the_same_anchor_is_refused r h ha
  · intro r anchor h
    exact a_recurrence_names_the_attempted_anchor r anchor h
  · exact the_no_anchor_trace_records_before_the_halt
  · exact the_reset_trace_records_before_each_transition
  · exact the_bad_trace_is_not_well_formed

/-- **Law 94's statement.** The refused state has no unchanged retry; every fault reaches a safe
    state; the attestation-only rebuild has no user deploy; the refused set is held out; the
    tracker key is the attempt identity; the latch is a transition state read by all four Rust
    sources; the durable record precedes an observable transition; the reset is the Rust's
    `reset_is_recurrence` against a finalised anchor with one attempt; and the resume is the
    Rust's `self_replay_resume_is_due`, including its catch-up, stage and anchor-identity arms. -/
def law94_statement : Prop :=
  (∀ (s : RecoveryState) (step : RecoveryStep), s = .refused →
    nextState s step ≠ .refused) ∧
  (∀ step : RecoveryStep, RecoverySafe (nextState .refused step)) ∧
  (∀ i : Input, (attestOnly i).deploys = ∅) ∧
  (∀ (i : Input) (refused : Finset Deploy) (d : Deploy),
    d ∈ refused → d ∉ (changeInput i refused).deploys) ∧
  (∀ (i j : Input), keyOf i = keyOf j → i = j) ∧
  (∀ t : Trigger, triggerAdmitted true t = false) ∧
  (∀ t : Transition, transitionStep t = some .halted → t.recorded = true) ∧
  law94_recovery_layer ∧
  law94_mutant

theorem law94 : law94_statement := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, law94_recovery_layer_proof, law94_falsifier.1⟩
  · intro s step h
    exact no_unchanged_retry s step h
  · intro step
    exact every_fault_reaches_safe step
  · intro i
    exact the_attest_only_rebuild_has_no_user_deploy i
  · intro i refused d hd
    exact the_refused_deploy_is_not_in_the_rebuild i refused d hd
  · intro i j h
    exact a_retry_of_the_same_key_is_not_a_new_attempt i j h
  · intro t
    exact a_halted_proposer_admits_no_trigger t
  · intro t h
    exact an_observable_halt_has_a_durable_record t h

end Rchain
