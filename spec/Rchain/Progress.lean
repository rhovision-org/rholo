import Rchain.Silence
import Mathlib.Logic.Relation

/-!
# Law 51 — progress: the shapes of non-progress

**Why this module exists.** Between 2026-09-28 and 2026-09-29 the same *kind* of defect arrived four
times under four names — C174 (a fringe partition no state could satisfy), C173 (a refusal no rule
clears), C171 (steps forever, finality unmoved), C170 (a liveness predicate read off a map that keeps
history) — each argued in prose, fixed alone, and filed as a one-off. Nothing stated the general
property being violated, so two readings of one paragraph could disagree about a fix (a plan proposed
shrinking finality's quorum denominator, which would have destroyed safety) with nothing in the tree
able to settle it.

This module is that general statement: a vocabulary for **non-progress**, parameterised so that one set
of definitions serves the ρ-calculus and the protocol built on it.

## Two axes, not one list

The defects fall on two axes, and the pairing is diagnostic — it says which *kind* of fix applies:

**The shape** — why the obligation cannot be met, all four finite and `decide`-able:

| shape | form | C-finding | the fix it implies |
|---|---|---|---|
| `Void` | no state satisfies the goal | C174 | change the requirement |
| `Terminal` | a satisfying state was left **and** no continuation restores it | C173, C172 | give the refused state an inverse |
| `Drift` | a run whose measure never moves, however long | C171 | bound the rate — but see the cause |

**The cause** — how the predicate stating the obligation is wrong:

| cause | form | C-finding |
|---|---|---|
| `Historic` | a predicate that disagrees with itself on two histories agreeing on the current view | C170 |
| `Split` | two readers of one state that a step makes disagree | C172 |

And the causes *produce* the shapes: C170's `Historic` predicate is what made C171's `Drift` (the guard
read "has ever spoken" and so never suppressed), and C172's `Split` is what made a `Terminal` stall.
The triage table in `docs/src/formal/progress.md` is keyed on both columns.

**Starvation is named, not modelled**, and that is a theorem of this tree rather than a preference: "an
enabled step that no schedule takes" is unstatable over a transition *relation*, and
`reduce_not_deterministic` (`Rchain/Concurrent.lean:329`) is the repo's own proof that the flat calculus
fixes no schedule at all. So `Fair` is a named hypothesis with no definition to state it against, and
the register carries the shape with `status := .open`.

## The correspondence with the calculus, kept honest

`enabled` is the calculus's `takesStep`: a `Bool` answering "is a step available *now*", tied to the
relation by `enabled_iff` — for the calculus, `takesStep_iff_reduces` **is** that field
(`silenceSystem` below). No row of the correspondence may claim agreement without saying which kind it
is, the same discipline the register applies to `status`:

| calculus | here | kind |
|---|---|---|
| `Reduce` / `ReduceP` | a `System`'s `Step` | **instance** (`silenceSystem`) |
| `Relation.ReflTransGen Reduce`, already used by `Rchain/Concurrent.lean` | `Reach` | **definitional** — `Reach` *is* that closure |
| `takesStep p = true ↔ ∃ q', ReduceP p q'` | `enabled_iff` | **theorem**, per instance: each supplies its own sound/complete pair |
| a `for` that matches nothing: no step, **no error** (Law 38/40) | `Void`: a requirement nothing satisfies, no rule fires and nothing errors | **theorem**, one layer up: C174's block was never refused, the gate simply never fired |
| a **join** (`for (x <- a; y <- b)`) — outside the model, as `Rchain/Silence.lean` says | an inactivity leak — outside *this* model | **not modelled, and a decision rather than a debt** (2026-10-02, §59): the protocol side has an **income-only** participation rule instead (law 44), because a leak burns a silent validator's *bond* and a partition is the canonical way a staker loses a bond through no fault of its own |

The layers differ in exactly one way, and it is the distinction this vocabulary exists to keep sharp: in
the **closed** calculus nothing outside the term can add a send, so a lone receive is silent *forever*;
at the **protocol** level delivery is itself a step, so the same shape is a *wait* — satisfiable by an
event — and only becomes `Terminal` when a rule takes the event away.
-/

namespace Rchain

/-- A transition system: states, a step relation, and a **decidable** predicate saying whether a step is
    available now — tied to the relation by `enabled_iff`, so the predicate cannot drift from the
    semantics. That field is the whole interface: `Rchain.Silence`'s `takesStep_iff_reduces` supplies it
    for the calculus (`silenceSystem`), and an instance supplies it for itself.

    A predicate that answered `false` where a step exists would make every shape below vacuous, which is
    what the field refuses by construction. -/
structure System where
  State : Type
  Step : State → State → Prop
  enabled : State → Bool
  enabled_iff : ∀ σ : State, enabled σ = true ↔ ∃ σ' : State, Step σ σ'

namespace System

variable {S : System}

/-- Reachability — the reflexive-transitive closure of the step relation, which is the primitive
    `Rchain/Concurrent.lean` already uses over the reducer's `Reduce`. -/
def Reach (S : System) (σ σ' : S.State) : Prop := Relation.ReflTransGen S.Step σ σ'

/-- A step is available in this state. -/
def Enabled (S : System) (σ : S.State) : Prop := S.enabled σ = true

/-- **`Stuck`** — no step leaves this state at all. -/
def Stuck (S : System) (σ : S.State) : Prop := ¬ ∃ σ', S.Step σ σ'

/-- **`Silent`** — no continuation however long satisfies the goal. Every non-progress defect is this;
    the shapes below differ in *why*, not in whether. -/
def Silent (S : System) (want : S.State → Prop) (σ : S.State) : Prop :=
  ∀ σ', S.Reach σ σ' → ¬ want σ'

/-- **`Void`** — the goal is unsatisfiable *from the start*: no state satisfies it, so no rule and no
    schedule could help. C174's shape: the full-partition filter demands a message from every bonded
    validator, and a validator that never speaks has none — the requirement is **empty**, not late. -/
def Void (want : S.State → Prop) : Prop := ∀ σ, ¬ want σ

/-- **`Terminal`** — the goal *was* reachable, a step was taken out of it, and no continuation of the
    state that step produced satisfies the goal. **Two obligations, and that pairing is the point**: the
    state was destroyed *and* no rule restores it. Stated as a pair, C173 (an attributable failure
    absorbing a bonded validator's chain) and C172 (a dropped `Internal` with nothing to re-queue it)
    are the same defect; stated as "the last satisfying state is gone", they look like two. -/
def Terminal (S : System) (want : S.State → Prop) (σ₀ : S.State) : Prop :=
  ∃ σ σ', S.Reach σ₀ σ ∧ want σ ∧ S.Step σ σ' ∧ ∀ τ, S.Reach σ' τ → ¬ want τ

/-- **`Unrestorable`** — once lost, a property is never regained: a refusal no rule clears (C173), or a
    window that has passed. **Named `Unrestorable` rather than "absorbing" because the port already uses
    *absorbing* for the other polarity** — `an_abort_is_absorbing` and `a_commit_is_absorbing`
    (`Rchain/CrossShard.lean`) mean *once true, always true*, which is the dual below — so two names for
    two polarities, and a reader cannot take one for the other. -/
def Unrestorable (S : System) (want : S.State → Prop) : Prop :=
  ∀ σ σ', S.Step σ σ' → ¬ want σ → ¬ want σ'

/-- **`Persistent`** — once true, always true: the port's own sense of *absorbing* for 2PC's terminal
    records, and the shape C173's refusal has (a validator the node has marked failed is never unmarked).
    Its consequence is what the laws consume: a goal that `want` forbids is unreachable, for good, from
    every state that holds `want`. -/
def Persistent (S : System) (want : S.State → Prop) : Prop :=
  ∀ σ σ', S.Step σ σ' → want σ → want σ'

/-- A **run** — a finite trace of states, oldest first. `List.Chain'` is where this tree already puts a
    run (`Rchain/SchedulerOnchain.lean`'s `DFSSerializable`). -/
def Run (S : System) (l : List S.State) : Prop := List.Chain' S.Step l

/-- **`Drift`** — a run of `n` steps in which the measure never moves. `n` is a parameter because the
    model has no infinitary runs and does not need them: the defect is that the number of steps is not a
    function of the work done, and a *measurement* gives the `n` (C171's is 126 blocks of height with the
    finality measure at 8). -/
def Drift (S : System) (measure : S.State → Nat) (n : Nat) : Prop :=
  ∃ σ : S.State, ∃ rest : List S.State,
    S.Run (σ :: rest) ∧ rest.length = n ∧ ∀ t ∈ σ :: rest, measure t = measure σ

/-- **`Paced`** — the repair *shape*: every run longer than `k` steps moves the measure somewhere. A
    pace condition that does not imply this has not bounded anything, which is why C171's fix is owed
    this shape rather than a smaller number of steps per round. -/
def Paced (S : System) (measure : S.State → Nat) (k : Nat) : Prop :=
  ∀ σ : S.State, ∀ rest : List S.State, S.Run (σ :: rest) → k < rest.length →
    ∃ t ∈ σ :: rest, measure t ≠ measure σ


/-- **`Split`** — two readers of one state that a reachable step makes disagree. C172's shape:
    `has_all_deps` asks the in-memory index while `block_summary` asks the persisted store. -/
def Split (S : System) (readA readB : S.State → Bool) (σ₀ : S.State) : Prop :=
  ∃ σ, S.Reach σ₀ σ ∧ readA σ ≠ readB σ

/-! ### The lemmas that make this a language

`autoImplicit` is off project-wide (`spec/lakefile.toml`), so every binder below is explicit — which is
also what makes a mistyped name in a statement a compile error rather than a silent implicit.

Each instance below is a non-vacuity witness: a shape stated over a type nothing inhabits would be the
register's `vacuous`. -/

theorem enabled_of_step {S : System} {σ σ' : S.State} (h : S.Step σ σ') : S.Enabled σ :=
  (S.enabled_iff σ).mpr ⟨σ', h⟩

theorem reach_refl (S : System) (σ : S.State) : S.Reach σ σ := Relation.ReflTransGen.refl

theorem reach_step {S : System} {σ σ' : S.State} (h : S.Step σ σ') : S.Reach σ σ' :=
  Relation.ReflTransGen.single h

theorem Reach.trans {S : System} {σ σ' σ'' : S.State} (h : S.Reach σ σ')
    (h' : S.Reach σ' σ'') : S.Reach σ σ'' := Relation.ReflTransGen.trans h h'

/-- **A stuck state reaches only itself** — so "no step leaves here" is a statement about the whole
    forward cone, not about one moment. -/
theorem stuck_reaches_only_itself {S : System} {σ : S.State} (h : S.Stuck σ) :
    ∀ σ', S.Reach σ σ' → σ' = σ := by
  intro σ' hr
  induction hr with
  | refl => rfl
  | tail hprev hstep ih => exact absurd (⟨_, ih ▸ hstep⟩ : ∃ σ', S.Step σ σ') h

/-- **…and therefore it is silent about any goal it does not already satisfy.** The asymmetry is the
    vocabulary's: `Stuck` is about steps and `Silent` about goals, so a stuck state with the goal
    already met is not a defect. -/
theorem stuck_of_not_want_is_silent {S : System} {σ : S.State} {want : S.State → Prop}
    (h : S.Stuck σ) (hσ : ¬ want σ) : S.Silent want σ := by
  intro σ' hr
  rw [S.stuck_reaches_only_itself h σ' hr]
  exact hσ

/-- **`Void` is silent, everywhere.** -/
theorem void_is_silent {S : System} {σ : S.State} {want : S.State → Prop} (h : S.Void want) :
    S.Silent want σ := fun σ' _ => h σ'

/-- **`Terminal` gives a point after which the goal is unreachable** — the honest content of the pair: it
    does not say the goal was never reached (it was), it says nothing after the destroying step ever
    satisfies it again. -/
theorem terminal_blocks_the_wait {S : System} {σ₀ : S.State} {want : S.State → Prop}
    (h : S.Terminal want σ₀) : ∃ σ', S.Reach σ₀ σ' ∧ ∀ τ, S.Reach σ' τ → ¬ want τ := by
  obtain ⟨_σ, σ', hσ, _hwant, hstep, hnever⟩ := h
  exact ⟨σ', hσ.trans (reach_step hstep), hnever⟩

/-- **Unrestorable ⇒ permanence**: a property no step restores, once lost, is lost forever. This is the
    second obligation of `Terminal`, and at the protocol level it is a property of the refusal rules
    (`Rchain.Casper.Validate`): C173's whole content. -/
theorem unrestorable_lost_is_never_regained {S : System} {want : S.State → Prop}
    (h : S.Unrestorable want) {σ : S.State} (hσ : ¬ want σ) : ∀ σ', S.Reach σ σ' → ¬ want σ' := by
  intro σ' hr
  induction hr with
  | refl => exact hσ
  | tail _ hstep ih => exact h _ _ hstep ih

/-- **Persistent ⇒ it stays true along every run** — the form the goals below consume. -/
theorem persistent_of_reaches {S : System} {want : S.State → Prop} (h : S.Persistent want)
    {σ : S.State} (hσ : want σ) : ∀ σ', S.Reach σ σ' → want σ' := by
  intro σ' hr
  induction hr with
  | refl => exact hσ
  | tail _ hstep ih => exact h _ _ hstep ih

/-- **A persistent property that forbids the goal makes it permanently unreachable.** C173's shape in one
    line: the refusal persists (`Persistent`), the rules refuse a block above it (`want → ¬ goal`), so
    from a state that holds the refusal *no* reachable state satisfies the goal — the node is `Terminal`,
    not slow. -/
theorem persistent_blocks_the_goal {S : System} {want goal : S.State → Prop}
    (hpers : S.Persistent want) (hgoal : ∀ σ, want σ → ¬ goal σ) {σ : S.State} (hσ : want σ) :
    ¬ ∃ σ', S.Reach σ σ' ∧ goal σ' := by
  rintro ⟨σ', hr, hg⟩
  exact hgoal σ' (S.persistent_of_reaches hpers hσ σ' hr) hg

/-- **The `Void` shape, as an implication from the predicate** — nothing satisfies the goal's requirement,
    so nothing satisfies the goal, so nothing is ever `Waiting` on it. What makes this a statement about
    a protocol is *proving* the requirement empty of its rules, which is the instance's obligation
    (C174's is the partition filter). -/
theorem void_goal_is_never_waiting {S : System} {σ : S.State} {want goal : S.State → Prop}
    (h : ∀ σ, ¬ want σ) (hgoal : ∀ σ, goal σ → want σ) :
    ¬ ∃ σ', S.Reach σ σ' ∧ goal σ' := by
  rintro ⟨σ', _, hg⟩
  exact h σ' (hgoal σ' hg)


/-- **A `Split` is a disagreement a reader can observe** — the shape C172 needs, stated so that the fix
    ("order the writes", or "make both readers ask one side") is exactly the negation. -/
theorem split_refutes_agreement {S : System} {readA readB : S.State → Bool} {σ₀ : S.State}
    (h : S.Split readA readB σ₀) : ¬ ∀ σ, S.Reach σ₀ σ → readA σ = readB σ := by
  obtain ⟨σ, hr, hne⟩ := h
  exact fun hagree => hne (hagree σ hr)

end System

/-! ## The two predicate causes, which need no step relation

Both are properties of a *history* — a list of views, oldest first — so they are polymorphic rather than
`System`-relative, and an instance states them over its own history type. -/

/-- **`Historic`** — a predicate with two histories that agree on the current view and disagree on the
    predicate. C170's shape: the attestation guard read liveness off `latest_msgs`, a map that keeps a
    silent sender's last message indefinitely, so its verdict depended on what had *ever* happened rather
    than on what is true now. -/
def Historic {σ : Type} (P : List σ → Bool) : Prop :=
  ∃ l l' : List σ, l.getLast? = l'.getLast? ∧ P l ≠ P l'

/-- **`ReadsTheView`** — what a liveness predicate must have, and what `Historic` refutes. -/
def ReadsTheView {σ : Type} (P : List σ → Bool) : Prop :=
  ∀ l l' : List σ, l.getLast? = l'.getLast? → P l = P l'

/-- **A `Historic` predicate cannot be reading the view** — the two are complements, and this is the
    direction C170 needs: whatever the guard measured, it was not the current view. -/
theorem historic_refutes_reading_the_view {σ : Type} {P : List σ → Bool} (h : Historic P) :
    ¬ ReadsTheView P := by
  obtain ⟨l, l', hsame, hne⟩ := h
  exact fun hread => hne (hread l l' hsame)

/-! ## The calculus as an instance, and the smallest system with each shape

These are the correspondence's checkable half and the non-vacuity witnesses. -/

/-- **The calculus, as a `System`**: the closed reduction, with `takesStep` as the decidable predicate
    and `takesStep_iff_reduces` **as** the interface field. That is the whole of the "shared names"
    bridge — no embedding is proved, and the field is what makes the sharing checkable. -/
def silenceSystem : System where
  State := Par
  Step := ReduceP
  enabled := takesStep
  enabled_iff := takesStep_iff_reduces

/-- **Silence in the calculus *is* stuckness, by the tie.** This is what the `enabled_iff` field buys: a
    term that reports no step has no step, so the two vocabularies agree at the calculus layer rather
    than merely being spelled the same. -/
theorem a_silent_term_is_stuck (p : Par) (h : takesStep p = false) : silenceSystem.Stuck p := by
  rintro ⟨q, hq⟩
  have ht : takesStep p = true := (takesStep_iff_reduces p).mpr ⟨q, hq⟩
  rw [ht] at h
  exact Bool.noConfusion h

/-- The 2-cycle: two states, each stepping only to the other — the smallest system with a `Drift`. -/
def twoCycle : System where
  State := Bool
  Step := fun a b => (a = false ∧ b = true) ∨ (a = true ∧ b = false)
  enabled := fun _ => true
  enabled_iff := by intro σ; cases σ <;> simp

/-- **The `Drift` shape, inhabited**: a constant measure never moves over the 2-cycle's run, so no
    `Paced` bound holds — C171's shape at its smallest. The pace fix's obligation is the negation
    (`Paced`), which is why the register's `falsifiable` for that clause names this. -/
theorem the_two_cycle_drifts : ¬ twoCycle.Paced (fun _ => 7) 2 := by
  intro h
  have s1 : twoCycle.Step false true := Or.inl ⟨rfl, rfl⟩
  have s2 : twoCycle.Step true false := Or.inr ⟨rfl, rfl⟩
  have s3 : twoCycle.Step false true := Or.inl ⟨rfl, rfl⟩
  have hrun : twoCycle.Run [false, true, false, true] :=
    List.Chain.cons s1 (List.Chain.cons s2 (List.Chain.cons s3 List.Chain.nil))
  obtain ⟨_t, _ht, hne⟩ := h false [true, false, true] hrun (by decide)
  exact hne rfl

/-! ## The paced gate (Law 51d)

A time-gated action — the proposer's escape from a round that is not closing — must **consume** its
permit. A gate that banks the permit ("the interval has elapsed") fires on every subsequent tick, so
the bound stops pacing anything and a fast producer can exhaust the finality-lag bound while its peers
are still assembling the next shared justification layer. The model is the two-state permit below; the
conformance corpus ties it to `take_escape_permit` in the proposer. -/

namespace Pacing

/-- The gate's state: when the blocked round was first seen, if it has been. -/
structure Permit where
  since : Option Nat
  deriving DecidableEq, Repr

/-- The interval: 15 000 ms (`ROUND_STALL_ESCAPE`). -/
def bound : Nat := 15000

/-- One tick of the correct gate: start the clock; after a full interval escape **and move the
    sighting to now**; otherwise wait with the sighting unchanged. -/
def take (p : Permit) (now : Nat) : Bool × Permit :=
  match p.since with
  | none => (false, { since := some now })
  | some first =>
      if bound ≤ now - first then (true, { since := some now }) else (false, p)

/-- The banked gate — the defect: the interval licenses every later tick. -/
def banked (p : Permit) (now : Nat) : Bool × Permit :=
  match p.since with
  | none => (false, { since := some now })
  | some first => (bound ≤ now - first, p)

/-- **A permit is consumed by the escape that uses it.** -/
theorem a_permit_is_consumed (p : Permit) (now : Nat) :
    (take p now).1 = true → (take p now).2.since = some now := by
  unfold take
  split
  · simp
  · split <;> simp_all

/-- **The banked gate fires every tick**, which is what the correct gate must not do. -/
theorem the_banked_gate_fires_every_tick :
    (banked ⟨some 0⟩ 15000).1 = true ∧ (banked ⟨some 0⟩ 15001).1 = true ∧
      (banked ⟨some 0⟩ 30000).1 = true := by decide

/-- **The correct gate paces**: after the escape at 15 000 ms, the next tick does not escape and the
    sighting is the escape instant; the following full interval does. -/
theorem the_correct_gate_waits_a_full_interval :
    (take ⟨some 0⟩ 15000).1 = true ∧
      (take (take ⟨some 0⟩ 15000).2 15001).1 = false ∧
      (take (take ⟨some 0⟩ 15000).2 30000).1 = true := by decide

end Pacing

end Rchain
