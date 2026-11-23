import Rchain.Progress

/-!
# Law 76 — a concluded stop stops

**Why this module exists.** The node has two places that conclude it "is stopping" — the self-reset's
failed act (`casper/src/engine/reset.rs`, whose doc says a node in that shape "must not keep serving") and
the LFS sync terminal path (`casper/src/engine/node_launch.rs`, "this node is stopping rather than serving
a chain it never synced"). Both are a `return Err` from a task **nobody joins**: the launched
`node_launch` future's error is only logged, and `NodeProgram::serve` never awaits it, so the block
receiver, the processor, the proposer and the API keep running. A logged line is not a stop.

**The law.** Once a fault the node cannot serve through is reported, the node is not serving and cannot
return to serving: the report and the stop are **one step**, and the step is the last one available. The
model is the smallest machine with that content — a latch and a serving flag — because the property is
about the *reachable set*, not about how the stop is implemented.

**The falsifier is the port's old shape.** `noteOnlyStep` is the same machine where reporting the fault
leaves the node serving — the shape the port had — and
`a_note_without_a_stop_leaves_the_node_serving` exhibits the reachable state it admits.
-/

namespace Rchain

/-- The node's stop state: `fatal` — a fault that concludes the node cannot serve has been reported;
    `serving` — the listeners are still up and answering. -/
structure ServeState where
  fatal : Bool
  serving : Bool
deriving DecidableEq, Repr

/-- The state a healthy node is in: nothing reported, serving. -/
def serveFresh : ServeState := ⟨false, true⟩

/-- **The step, and there is only one**: reporting the fault sets the latch *and* stops the listeners
    together, and nothing leaves the stopped state. That the report and the stop are one step is the
    whole content — a report that is not also a stop is a step that leaves the node serving a state it
    chose to leave. -/
def serveStep (s s' : ServeState) : Prop :=
  s.fatal = false ∧ s' = ⟨true, false⟩

/-- A step is available exactly while no fault has been reported. -/
def serveEnabled (s : ServeState) : Bool := !s.fatal

/-- **The node's stop, as a `System`.** -/
def serveSystem : System where
  State := ServeState
  Step := serveStep
  enabled := serveEnabled
  enabled_iff := by
    intro σ
    constructor
    · intro h
      cases hf : σ.fatal
      · exact ⟨⟨true, false⟩, ⟨hf, rfl⟩⟩
      · simp [serveEnabled, hf] at h
    · rintro ⟨σ', hs⟩
      unfold serveStep at hs
      rcases hs with ⟨h1, _⟩
      simpa [serveEnabled, h1]

/-- The step lemma: the target of the step is not serving. -/
theorem serve_target_is_not_serving (s s' : ServeState) (hs : serveStep s s') :
    s'.serving = false := by
  unfold serveStep at hs
  rcases hs with ⟨_, h2⟩
  rw [h2]

/-- **The law, in one line.** No reachable state has a reported fault while the node is still serving:
    the report *is* the stop. -/
theorem a_reported_fault_stops_the_node (σ : ServeState)
    (hr : serveSystem.Reach serveFresh σ) (hf : σ.fatal = true) : σ.serving = false := by
  induction hr with
  | refl => exact absurd hf (by simp [serveFresh])
  | tail _ hstep _ => exact serve_target_is_not_serving _ _ hstep

/-- **Non-vacuity: a reported fault reaches a stopped state.** -/
theorem a_reported_fault_reaches_a_stopped_state :
    ∃ σ, serveSystem.Reach serveFresh σ ∧ σ.fatal = true ∧ σ.serving = false :=
  ⟨⟨true, false⟩, Relation.ReflTransGen.single ⟨rfl, rfl⟩, rfl, rfl⟩

/-- **The mutant the port had**: the fault is reported — and only recorded — while the listeners stay up. -/
def noteOnlyStep (s s' : ServeState) : Prop :=
  s.fatal = false ∧ s' = { s with fatal := true }

/-- **The falsifier, fired.** Under the port's old shape a state is reachable with the fault reported and
    the node still serving — the "this node is stopping" sentence with the process still answering. -/
theorem a_note_without_a_stop_leaves_the_node_serving :
    ∃ σ, Relation.ReflTransGen noteOnlyStep serveFresh σ ∧ σ.fatal = true ∧ σ.serving = true :=
  ⟨⟨true, true⟩, Relation.ReflTransGen.single ⟨rfl, rfl⟩, rfl, rfl⟩

/-!
## The safe halt — stopping production, not service

A spent restore budget is a divergence the node cannot repair without naming an anchor the network
has not agreed. The node must stop **producing**, but it is still a truthful read-only server of the
chain it holds. That is a third state beside the fatal stop above: `proposing` goes false, `serving`
stays true, and the only way back is an operator's act.

The model is the smallest machine with that content. The theorem that matters is the *absence* of an
automatic exit: no safe-halt step sets `proposing` back to true, so a halt a crash left on disk is
still a halt at the next boot.
-/

/-- A node that may still serve and may or may not propose. `fatal` is the law-76 stop above; a
    safe halt leaves it false. -/
structure HaltState where
  fatal : Bool
  serving : Bool
  proposing : Bool
deriving DecidableEq, Repr

/-- A healthy node: nothing fatal, serving, proposing. -/
def haltFresh : HaltState := ⟨false, true, true⟩

/-- **The safe halt**: production stops, service does not. -/
def safeHaltStep (s s' : HaltState) : Prop :=
  s.fatal = false ∧ s.proposing = true ∧ s.serving = true ∧ s' = { s with proposing := false }

/-- **The operator's resume**: the only step that turns production back on. -/
def operatorResumeStep (s s' : HaltState) : Prop :=
  s.fatal = false ∧ s.proposing = false ∧ s.serving = true ∧ s' = { s with proposing := true }

/-- The halt machine's step relation: either a safe halt or the operator's resume. -/
def haltStep (s s' : HaltState) : Prop := safeHaltStep s s' ∨ operatorResumeStep s s'

/-- **The safe halt stops production and keeps the node serving.** -/
theorem a_safe_halt_stops_production_not_serving {s s' : HaltState}
    (h : safeHaltStep s s') : s'.proposing = false ∧ s'.serving = true := by
  rcases h with ⟨_, _, hserv, rfl⟩
  exact ⟨rfl, hserv⟩

/-- **A safe halt has no automatic exit.** From the halted state, no safe-halt step is enabled; only
    the operator's resume can leave it. -/
theorem a_safe_halt_has_no_automatic_exit :
    ¬ ∃ s', safeHaltStep ({ haltFresh with proposing := false }) s' := by
  rintro ⟨s', h⟩
  have hp : ({ haltFresh with proposing := false } : HaltState).proposing = true := h.2.1
  simp [haltFresh] at hp

/-- **Non-vacuity: an operator can resume a safe halt.** -/
theorem a_safe_halt_can_be_resumed_by_an_operator :
    ∃ s s', safeHaltStep haltFresh s ∧ operatorResumeStep s s' ∧ s'.proposing = true := by
  refine ⟨{ haltFresh with proposing := false }, { haltFresh with proposing := true }, ?_, ?_, rfl⟩
  · exact ⟨rfl, rfl, rfl, rfl⟩
  · exact ⟨rfl, rfl, rfl, rfl⟩

end Rchain
