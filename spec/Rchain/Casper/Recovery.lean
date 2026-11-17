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

**What it does not claim.** It does not model the replay's cause (that is RC-B, open), the cost/state
ordering (law 93), or the reset's durable bracket (laws 72/73/76). It says only that the fault state
has an exit and that the exit is not a no-op. The Rust witnesses are the proposer's recovery/halt
tests and the durable-record codec.

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

/-- The proposal triggers that must read the latch. -/
inductive Trigger where
  | autopropose
  | adminPropose
  | deployPropose
  | boundary
deriving DecidableEq, Repr

/-- The latch: a halted proposer admits no trigger. -/
def triggerAdmitted (halted : Bool) (_t : Trigger) : Bool := !halted

theorem a_halted_proposer_admits_no_trigger (t : Trigger) :
    triggerAdmitted true t = false := rfl

/-- The defect shape: a trigger that does not read the latch. -/
def triggerSkipsLatch (_halted : Bool) (_t : Trigger) : Bool := true

theorem a_trigger_that_skips_the_latch_admits_a_block (t : Trigger) :
    triggerSkipsLatch true t = true := rfl

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

/-- A reset names one anchor, its finality, and the anchors already attempted. -/
structure Reset where
  anchor : Nat
  finalised : Bool
  attempted : Finset Nat
deriving DecidableEq

def canReset (r : Reset) : Bool :=
  r.finalised && !(decide (r.anchor ∈ r.attempted))

theorem the_reset_anchor_is_finalised (r : Reset) (h : canReset r = true) :
    resetAdmitted r.finalised := by
  unfold resetAdmitted
  unfold canReset at h
  by_cases hf : r.finalised = true
  · exact hf
  · simp [hf] at h

theorem at_most_one_reset_per_anchor (r : Reset) (h : r.anchor ∈ r.attempted) :
    canReset r = false := by
  simp [canReset, h]

/-- The resume is admitted only after catch-up and only against a finalised anchor. -/
def resumeAdmitted (caughtUp anchorFinalised : Bool) : Bool :=
  caughtUp && anchorFinalised

theorem resume_requires_catch_up (a : Bool) : resumeAdmitted false a = false := rfl

theorem resume_requires_a_finalised_anchor (c : Bool) : resumeAdmitted c false = false := by
  cases c <;> rfl

/-- **Law 94's mutant**: the incident's unchanged retry, represented as a step. -/
inductive RecoveryStepMutant where
  | retrySameInput
  | other
deriving DecidableEq, Repr

def nextStateMutant : RecoveryState → RecoveryStepMutant → RecoveryState
  | .refused, .retrySameInput => .refused
  | _, _ => .halted

def law94_mutant : Prop :=
  nextStateMutant .refused .retrySameInput = .refused

theorem law94_falsifier : law94_mutant := rfl

/-- **Law 94's statement.** The refused state has no unchanged retry; every fault reaches a safe
    state; the attestation-only rebuild has no user deploy; the refused set is held out; the
    tracker key is the attempt identity; the latch is read by every trigger; the durable record
    precedes an observable halt; the reset anchor is finalised and attempted once; and the resume
    waits for catch-up and a finalised anchor. -/
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
  (∀ r : Reset, canReset r = true → resetAdmitted r.finalised) ∧
  (∀ r : Reset, r.anchor ∈ r.attempted → canReset r = false) ∧
  (∀ a : Bool, resumeAdmitted false a = false) ∧
  (∀ c : Bool, resumeAdmitted c false = false) ∧
  law94_mutant

theorem law94 : law94_statement := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, law94_falsifier⟩
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
  · intro r h
    exact the_reset_anchor_is_finalised r h
  · intro r h
    exact at_most_one_reset_per_anchor r h
  · intro a
    exact resume_requires_catch_up a
  · intro c
    exact resume_requires_a_finalised_anchor c

end Rchain
