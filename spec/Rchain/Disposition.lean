/-!
# Law 88 — block disposition and isolation

The live private-chain incident left a stored block body with no DAG entry, no disposition row and
no retry, while the proposing node kept extending a chain no peer had accepted. PR #497 made the
first state representable (a durable `BlockDisposition` ledger) and added a phase the surfaces can
read; this module states the two relations those fixes enforce.

**88a — a stored body is accounted for or visible.** A body in the block store is either a DAG
block (its metadata is the durable verdict) or carries an unsettled `Pending`/`Faulted` row. The
start-up audit adopts every unaccounted body, so reaching `Running` cannot skip a stored body. The
law says **visible**, not **settled**: a body whose justifications are not in the DAG stays
`Pending` by design, and the start-up drain is bounded — a missing parent does not hold boot.

**88b — a proposal is admitted only in `Running` and within the finality bound.** The gate refuses
outside the running phase and one height beyond `MAX_PROPOSAL_FINALITY_LAG`, which is what bounds a
private chain when no peer accepts it.

The model is deliberately node-local (durable ingestion and a proposal policy), not consensus
state: there is no wire or derived-value relation here, so the register rows are `proved-model`
rather than tied to a conformance corpus. What stays with Rust and its tests: LMDB I/O errors, the
crash windows around the marker, body decoding, retry budgets, the finality algorithm itself and
the drain's duration (a liveness policy, not a state predicate). A body whose bytes cannot be
decoded is **not** adopted by the Rust scan; it remains an orphan and is the periodic count audit's
signal — the model assumes readable bodies and the register note says so.
-/

namespace Rchain

namespace Disposition

/-- Block hashes and heights are abstract numbers in the model; the Rust side uses the refined
    `BlockHash`/`BlockHeight` types. -/
abbrev BlockHash := Nat
abbrev Height := Nat

/-- The node phase launch owns (`casper/src/engine/node_phase.rs`). -/
inductive Phase where
  | starting
  | syncing
  | running
  | blocked
  deriving DecidableEq, Repr

/-- The ledger's unsettled states (`casper/src/block_disposition.rs`). `Accepted` and `Refused`
    are not rows — they are the DAG metadata — so they are not constructors here. -/
inductive Disposition where
  | pending (since : Nat)
  | faulted (attempts : Nat)
  deriving DecidableEq, Repr

/-- A stored body: the key is the hash, and the justifications are the dependency set. -/
structure Body where
  hash : BlockHash
  number : Height
  justifications : List BlockHash
  deriving DecidableEq, Repr

/-- One unsettled ledger row. -/
structure Row where
  hash : BlockHash
  disposition : Disposition
  deriving DecidableEq, Repr

/-- The node's durable view: the stored bodies, the DAG's block set, the ledger, and the phase.
    The DAG set stands for block metadata (accepted **and** refused blocks both have an entry). -/
structure State where
  bodies : List Body
  dag : List BlockHash
  ledger : List Row
  phase : Phase := .starting
  deriving Repr

namespace State

/-- The body's hash is a DAG block. A `Bool` so the audit's adoption step is a decision procedure;
    the predicates below expose it as a proposition. -/
def inDag (s : State) (h : BlockHash) : Bool := s.dag.contains h

/-- The ledger carries a row for this hash. -/
def hasRow (s : State) (h : BlockHash) : Bool := s.ledger.any (fun r => r.hash == h)

/-- The block store holds a body for this hash. -/
def stored (s : State) (h : BlockHash) : Bool := s.bodies.any (fun b => b.hash == h)

/-- **88a's vocabulary.** Accounted for by the DAG or visible as an unsettled row. -/
def Disposed (s : State) (h : BlockHash) : Prop := s.inDag h = true ∨ s.hasRow h = true

/-- The adoption step is a decision procedure; the disjunction is decidable because both arms are
    `Bool` equalities. -/
instance (s : State) (h : BlockHash) : Decidable (s.Disposed h) := by
  unfold State.Disposed
  infer_instance

/-- An orphan: a stored body with neither record. This is the state the live incident had. -/
def Orphan (s : State) (h : BlockHash) : Prop := s.stored h = true ∧ ¬ s.Disposed h

/-- Every stored body is accounted for or visible. -/
def Clean (s : State) : Prop := ∀ b ∈ s.bodies, s.Disposed b.hash

end State

open State

/-- **The audit's step**: leave a body that is already in the DAG or already has a row alone;
    otherwise adopt it as a `Pending` row. This is the start-up scan's adoption, not a send. -/
def adoptBody (s : State) (b : Body) : State :=
  if s.Disposed b.hash then
    s
  else
    { s with ledger := s.ledger ++ [{ hash := b.hash, disposition := .pending 0 }] }

/-- The audit over a list, after `s` has already audited the bodies before it. -/
def auditFrom (s : State) : List Body → State
  | [] => s
  | b :: rest => auditFrom (adoptBody s b) rest

/-- Audit every stored body. The Rust traversal is `scan_legacy_bodies` + the ledger re-offer; the
    model's audit is their durable effect (a row for every unaccounted body). -/
def audit (s : State) : State := auditFrom s s.bodies

/-- The phase transition the start-up path takes **after** the audit. -/
def startup (s : State) : State := { (audit s) with phase := .running }

/-- The defect shape: flip the phase without auditing. -/
def skipAudit (s : State) : State := { s with phase := .running }

/-- The defect shape for a row deleted without a DAG record. -/
def dropRow (s : State) (h : BlockHash) : State :=
  { s with ledger := s.ledger.filter (fun r => r.hash != h) }

/-! ## Structural facts about the audit -/

theorem adoptBody_dag (s : State) (b : Body) : (adoptBody s b).dag = s.dag := by
  unfold adoptBody
  split <;> rfl

theorem adoptBody_bodies (s : State) (b : Body) : (adoptBody s b).bodies = s.bodies := by
  unfold adoptBody
  split <;> rfl

/-! ## 88a — the audit accounts for every stored body -/

/-- The adopted body has a record: it was already disposed, or the new row disposes it. -/
theorem adoptBody_disposes (s : State) (b : Body) : (adoptBody s b).Disposed b.hash := by
  unfold adoptBody
  split
  · assumption
  · rw [State.Disposed]
    right
    simp [State.hasRow, List.any_append]

/-- A disposition survives every audit step (the DAG set is untouched and the ledger only grows). -/
theorem adoptBody_preserves (s : State) (b : Body) {h : BlockHash} (hd : s.Disposed h) :
    (adoptBody s b).Disposed h := by
  unfold adoptBody
  split
  · exact hd
  · rw [State.Disposed] at hd ⊢
    rcases hd with hd | hd
    · left
      simpa [State.inDag] using hd
    · right
      rw [State.hasRow, List.any_append]
      have hd' : s.ledger.any (fun r => r.hash == h) = true := by
        simpa [State.hasRow] using hd
      rw [hd']
      simp

/-- A disposition survives the whole audit. -/
theorem auditFrom_preserves {s : State} {h : BlockHash} (hd : s.Disposed h) :
    ∀ bs : List Body, (auditFrom s bs).Disposed h
  | [] => hd
  | b :: rest => auditFrom_preserves (adoptBody_preserves s b hd) rest

/-- Every body in the audited list is disposed in the final state. -/
theorem disposed_of_mem_audit (bs : List Body) (s : State) (b : Body) (hb : b ∈ bs) :
    (auditFrom s bs).Disposed b.hash := by
  induction bs generalizing s with
  | nil => simp at hb
  | cons x rest ih =>
      simp only [List.mem_cons] at hb
      cases hb with
      | inl hx =>
          rw [hx]
          exact auditFrom_preserves (adoptBody_disposes s x) rest
      | inr hrest =>
          exact ih (adoptBody s x) hrest

/-- The audit never changes the body set. -/
theorem audit_bodies (s : State) : (audit s).bodies = s.bodies := by
  suffices h : ∀ bs : List Body, (auditFrom s bs).bodies = s.bodies by
    exact h s.bodies
  intro bs
  induction bs generalizing s with
  | nil => rfl
  | cons b rest ih =>
      change (auditFrom (adoptBody s b) rest).bodies = s.bodies
      rw [ih (adoptBody s b), adoptBody_bodies]

/-- **88a's main theorem**: after the audit, no stored body is unaccounted for. -/
theorem audit_covers (s : State) : (audit s).Clean := by
  intro b hb
  have hb' : b ∈ s.bodies := by
    simpa [audit_bodies] using hb
  exact disposed_of_mem_audit s.bodies s b hb'

/-- The audit never changes the DAG set: only the ledger grows. -/
theorem audit_does_not_change_dag (s : State) : (audit s).dag = s.dag := by
  suffices h : ∀ bs : List Body, (auditFrom s bs).dag = s.dag by
    exact h s.bodies
  intro bs
  induction bs generalizing s with
  | nil => rfl
  | cons b rest ih =>
      change (auditFrom (adoptBody s b) rest).dag = s.dag
      rw [ih (adoptBody s b), adoptBody_dag]

/-- An unaccounted body becomes visible: after the audit it has a row. -/
theorem audit_makes_an_unbacked_body_visible (s : State) (b : Body)
    (hb : b ∈ s.bodies) (hnot : ¬ s.Disposed b.hash) :
    (audit s).hasRow b.hash = true := by
  have hd : (audit s).Disposed b.hash := by
    have hb' : b ∈ s.bodies := hb
    exact disposed_of_mem_audit s.bodies s b hb'
  rcases hd with h | h
  · apply absurd _ hnot
    left
    simpa [State.inDag, audit_does_not_change_dag] using h
  · exact h

/-- **Reaching `Running` through `startup` implies a clean store.** -/
theorem startup_clean (s : State) : (startup s).Clean := by
  simpa [startup, State.Clean, audit_bodies] using audit_covers s

/-- **The defect shape, in the tree**: flipping the phase without auditing leaves the stored body
    an orphan. `orphanState` has one body, no DAG entry and no ledger row. -/
def orphanState : State :=
  { bodies := [{ hash := 7, number := 1, justifications := [] }],
    dag := [],
    ledger := [] }

theorem startup_without_audit_leaves_an_orphan : (skipAudit orphanState).Orphan 7 := by
  simp [skipAudit, orphanState, State.Orphan, State.Disposed, State.inDag, State.stored,
    State.hasRow]

/-- **The other defect shape**: a row deleted without a DAG record returns the body to orphan. -/
theorem a_dropped_row_is_an_orphan :
    ∃ s : State, s.Disposed 7 ∧ (dropRow s 7).Orphan 7 := by
  refine ⟨{ bodies := [{ hash := 7, number := 1, justifications := [] }],
            dag := [],
            ledger := [{ hash := 7, disposition := .pending 0 }] }, ?_, ?_⟩
  · simp [State.Disposed, State.hasRow]
  · simp [dropRow, State.Orphan, State.Disposed, State.inDag, State.stored, State.hasRow]

/-! ## 88b — the proposal gate -/

/-- The bound the Rust proposer uses (`casper/src/blocks/proposer/proposer.rs`). Policy, not a
    measured optimum. -/
def MAX_PROPOSAL_FINALITY_LAG : Nat := 32

/-- Nat subtraction saturates, as the Rust `saturating_sub` does for nonnegative heights. -/
def withinProposalFinalityBound (tip finalized : Nat) : Bool :=
  decide (tip - finalized ≤ MAX_PROPOSAL_FINALITY_LAG)

inductive Refusal where
  | notRunning
  | tooFarAhead
  deriving DecidableEq, Repr

inductive GateResult where
  | allow
  | refuse (reason : Refusal)
  deriving DecidableEq, Repr

/-- **The gate**: allow only in `Running` and within the bound. -/
def gate (phase : Phase) (tip finalized : Nat) : GateResult :=
  if phase ≠ .running then .refuse .notRunning
  else if withinProposalFinalityBound tip finalized = true then .allow
  else .refuse .tooFarAhead

/-- A gate that checks the bound but not the phase. -/
def legacyGate (_phase : Phase) (tip finalized : Nat) : GateResult :=
  if withinProposalFinalityBound tip finalized = true then .allow else .refuse .tooFarAhead

/-- A gate with no bound at all. -/
def unboundedGate (_phase : Phase) (_tip _finalized : Nat) : GateResult := .allow

theorem gate_admits_only_in_running_and_within_bound (phase : Phase) (tip finalized : Nat)
    (h : gate phase tip finalized = .allow) :
    phase = .running ∧ withinProposalFinalityBound tip finalized = true := by
  unfold gate at h
  by_cases hp : phase = .running
  · have hnot : ¬ (phase ≠ .running) := by
      intro hne
      exact hne hp
    rw [if_neg hnot] at h
    by_cases hb : withinProposalFinalityBound tip finalized = true
    · rw [if_pos hb] at h
      exact ⟨hp, hb⟩
    · rw [if_neg hb] at h
      simp at h
  · rw [if_pos hp] at h
    simp at h

/-- At the bound the gate admits (`32 - 0 = 32`). -/
theorem gate_admits_at_the_bound :
    gate .running MAX_PROPOSAL_FINALITY_LAG 0 = .allow := by
  decide

/-- One height beyond the bound the gate refuses. -/
theorem gate_refuses_beyond_the_bound :
    gate .running (MAX_PROPOSAL_FINALITY_LAG + 1) 0 = .refuse .tooFarAhead := by
  decide

/-- Outside `Running` the gate refuses, whatever the heights. -/
theorem gate_refuses_outside_running (phase : Phase) (tip finalized : Nat)
    (hp : phase ≠ .running) : gate phase tip finalized = .refuse .notRunning := by
  simp [gate, hp]

/-- **88b's falsifier, in the tree**: a gate without the phase check admits a syncing node. -/
theorem a_gate_without_the_phase_admits_a_syncing_node :
    legacyGate .syncing 5 5 = .allow := by
  decide

/-- **88b's other falsifier**: a gate without the bound accepts a private chain. -/
theorem a_gate_without_the_bound_admits_a_private_chain :
    unboundedGate .running (MAX_PROPOSAL_FINALITY_LAG + 1) 0 = .allow := by
  decide

/-- **The height the gate measures from when the view has no finalised block**: genesis. The Rust is
`finalized_floor` (`casper/src/blocks/proposer/proposer.rs`), and it is the same answer
`RewindPlan` gives — "no anchor" is not "no bound". -/
def finalizedFloor : Option Nat → Nat
  | none => 0
  | some f => f

/-- The floor is a floor, not a sentinel: it never exceeds the anchor it is given. -/
theorem floor_never_exceeds_the_anchor (f : Option Nat) : finalizedFloor f ≤ f.getD 0 := by
  cases f <;> simp [finalizedFloor]

/-- The gate as the Rust spells it now: the anchor is an `Option`, and the height measured from is
`finalizedFloor`. -/
def totalGate (phase : Phase) (tip : Nat) (anchor : Option Nat) : GateResult :=
  gate phase tip (finalizedFloor anchor)

/-- **88b's third falsifier, and the defect C351 names**: a gate that *skips* the bound when there
is no anchor accepts a private chain — the shape the Rust had behind `if let Some(finalized)`. -/
def skipWithoutAnchor (anchor : Option Nat) (tip finalized : Nat) : GateResult :=
  match anchor with
  | none => .allow
  | some _ => gate .running tip finalized

theorem a_gate_that_skips_the_bound_without_an_anchor_admits_a_private_chain :
    skipWithoutAnchor none (MAX_PROPOSAL_FINALITY_LAG + 1) 0 = .allow := by
  decide

/-- **A view with no finalised block is still bounded**: the total gate refuses one height past the
bound with no anchor, exactly as it does with genesis. -/
theorem a_view_without_a_finalized_block_is_still_bounded :
    totalGate .running (MAX_PROPOSAL_FINALITY_LAG + 1) none = .refuse .tooFarAhead := by
  decide

/-- An anchor that exists is passed through unchanged, so totalising the gate did not move it. -/
theorem the_anchor_is_passed_through (f tip : Nat) :
    totalGate .running tip (some f) = gate .running tip f := by
  simp [totalGate, finalizedFloor]

/-! ## 88c — an overflow refusal is a deferral, not an orphan

The receiver's capacity path stores the body and writes the `Pending` row **before** it can refuse,
so the refusal is a deferral the ledger already records. What was missing was a reader: the fault
loop excludes `Pending`, and the boot sweep skips a row whose parents are absent. A runtime sweep
over aged `Stored` rows is that reader; it claims the receiver's in-memory state, so a held body is
dispatched once, and it moves the row in flight before dispatch, so a second sweep takes nothing.

The model is the row's eligibility and the claim, not the store I/O: a missing body, a read error
and the receiver's queue are Rust facts the register note names. -/

namespace Recovery

/-- Where a pending row is. `Retry` is "in flight"; only `Stored` is the sweep's question. -/
inductive Stage where
  | stored
  | validating
  | retry
  deriving DecidableEq, Repr

/-- One pending row: its stage and the instant it was written. -/
structure Row where
  stage : Stage
  since : Nat
  deriving DecidableEq, Repr

/-- The age floor: a stored row must be 60 s old before the sweep will take it. -/
def age_floor : Nat := 60000

/-- **The sweep's predicate**: a `Stored` row at or past the floor. -/
def eligible (r : Row) (now : Nat) : Bool :=
  r.stage == Stage.stored && age_floor ≤ now - r.since

/-- **The capacity refusal changes no row.** The obligation survives the error by construction. -/
def refuse (r : Row) : Row := r

theorem an_overflow_refusal_keeps_the_row (r : Row) : refuse r = r := rfl

/-- The receiver's in-memory claim: held and not yet claimed. -/
structure Receiver where
  held : Bool
  claimed : Bool
  deriving DecidableEq, Repr

/-- Take the claim if it is free. The second answer is the state after the attempt. -/
def claim (r : Receiver) : Bool × Receiver :=
  if r.held && !r.claimed then (true, { r with claimed := true }) else (false, r)

/-- **A claim is taken once**: whatever the first attempt did, the second takes nothing. -/
theorem a_claim_is_taken_once (r : Receiver) : (claim (claim r).2).1 = false := by
  unfold claim
  split <;> simp_all

/-- A re-offered row is `retry`, so a second sweep does not take it. -/
def reoffer (r : Row) : Row := { r with stage := Stage.retry }

theorem reoffer_is_not_eligible (r : Row) (now : Nat) :
    eligible (reoffer r) now = false := by
  simp [eligible, reoffer]

/-- The floor, both sides: one millisecond under is not eligible; at it, is. -/
theorem the_age_floor_gates_eligibility :
    eligible ⟨Stage.stored, 0⟩ 59999 = false ∧
      eligible ⟨Stage.stored, 0⟩ 60000 = true := by
  decide

/-- A row already in flight is never eligible. -/
theorem an_inflight_row_is_not_eligible (now : Nat) :
    eligible ⟨Stage.retry, 0⟩ now = false := by
  simp [eligible]

/-! ### 88d — the regression check's classification

The check cannot be evaluated when a justification is absent from the message map. The current rule
**passes** (the Scala's own `None`), and that verdict is deliberate; what changes is that the outcome
is *named*, so a later failure is recorded as a missing justification rather than an unclassified
internal fault. Making the unresolved case a refusal would be a consensus change, which is why the
model proves it does not refuse. -/

inductive Outcome where
  | pass
  | regression
  | unresolved
  | noOwn
  deriving DecidableEq, Repr

/-- The check's answer from whether the justification is in the message map and whether the
    regression relation holds. -/
def classify (in_msg_map regression : Bool) : Outcome :=
  match in_msg_map with
  | false => Outcome.unresolved
  | true => if regression then Outcome.regression else Outcome.pass

/-- The outcome's corpus name. -/
def outcomeName : Outcome → String
  | Outcome.pass => "pass"
  | Outcome.regression => "regression"
  | Outcome.unresolved => "unresolved"
  | Outcome.noOwn => "no_own"

/-- Whether the outcome refuses the block. Only `regression` does. -/
def refuses : Outcome → Bool
  | Outcome.regression => true
  | _ => false

/-- **An unresolved justification does not refuse**, whatever the relation would have said. -/
theorem unresolved_does_not_refuse (regression : Bool) :
    refuses (classify false regression) = false := by
  cases regression <;> rfl

theorem regression_refuses : refuses (classify true true) = true := by
  decide

theorem pass_does_not_refuse : refuses (classify true false) = false := by
  decide

end Recovery

end Disposition

end Rchain
