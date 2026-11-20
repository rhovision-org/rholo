import Rchain.Casper.Fringe
import Rchain.Progress

/-!
# Law 52 — finality's partition and its quorum

Law 14's gate asks two questions of the bonds (the port's `liveness` module, `#70`): what a candidate
message must have been **seen by** (`partition`), and what the supermajority is measured **against**
(`quorum`). Law 52 is what the two answers have to be, and it is the law this repository needed on
2026-09-29 and did not have: a plan for that increment proposed handing the *live* weight set as both —
"so a silent validator leaves numerator and denominator together" — which would have destroyed safety,
and nothing in the tree could refuse it.

## Clause a — the denominator is the whole bonded stake

Two disjoint groups cannot each hold a strict supermajority of the same total: `3·s₁ > 2·t` and
`3·s₂ > 2·t` give `t < s₁ + s₂` (`supermajorities_overlap`). That is the arithmetic the counterexample
below instantiates, and it is *why* only the partition may shrink:

| | partition `{A,B}` | quorum `{A,B}` | quorum `{A,B,C,D}` |
|---|---|---|---|
| candidates seen by `A,B` only | 200 of 200 → **advances** | 200 of 400 → refused |
| candidates seen by all four | — | 300 of 400 → advances |

With the live set as the *denominator*, each of two disjoint views is a supermajority of itself, so under
a network partition each side would finalise its own view — two conflicting finalisations. Shrinking the
**partition** is what restores liveness (C174: a silent validator stops blocking the cut) and the
denominator is what keeps safety.

## Clause b — the requirement must be one a state can satisfy

With the whole bonded set as the *partition*, a validator that produces no message makes the filter
unsatisfiable — not slow, **empty**: `allBonded` requires every seer's seen set to contain the bonded set,
and no seer can have seen a validator that never spoke. That is C174's measured shape (the survivors at
80 % could not lift the gate at all), and it is the `Void` shape of Law 51 with its proof obligation
discharged here (`silent_validator_blocks_the_partition`).

## The hypotheses a liveness claim needs

Named rather than left implicit, because a liveness statement without its hypothesis is the Law 20 trap
(`law20_deadlock_freedom` was an axiom until it was deleted as unprovable as stated). Every liveness claim
in this file carries the one it uses:

- **`Participation`** — every bonded validator speaks within the staleness window. What C174's fix trades
  away is *not* this hypothesis but its strictness: the partition ranges over `Participation`'s live
  subset, while the quorum still needs the whole bonded stake.
- **`Delivery`** — a message an honest validator sends reaches another. #105's state-hash divergence is a
  violation of the *agreement* half of this hypothesis, not of any rule.
- **`StalenessBound`** — "current" is `heightsBehind tip m ≤ w`, so the window is a bounded claim about
  the view rather than an appeal to history (Law 51b's `Historic`).
-/

namespace Rchain

/-! ## The hypotheses, named -/

/-- **Participation**: every bonded validator's latest message is within the window of the tip. The
    liveness clause below needs the *live* form of this (the subset that is speaking), which is what
    C174's fix substituted; the full form is what a net with no absent validator has. -/
def Participation (bonds : Bonds) (latest : Sender → Option Nat) (tip w : Nat) : Prop :=
  ∀ v ∈ bondedSenders bonds, ∃ h, latest v = some h ∧ h + w ≥ tip

/-- **Delivery**: what an honest validator sends reaches another. Named because the two ways it fails are
    different findings: *silence* (a peer never answers) and *disagreement* (the nodes' states differ, so
    the message arrives and is refused) — #105's run was the second. -/
def Delivery (sent received : Nat → Nat → Prop) : Prop := ∀ a b, sent a b → received a b

/-- **StalenessBound**: the window, as a predicate rather than a convention — a message is current iff it
    is at most `w` heights behind the tip. The predicate a liveness rule must read (Law 51b: reading a map
    that keeps history instead is what made C170's guard count a validator that had *ever* spoken). -/
def StalenessBound (tip h w : Nat) : Prop := h + w ≥ tip

/-! ## Clause a — the safety arithmetic, and the two-sided counterexample -/

/-- **Two supermajorities of one total overlap** — `3·s₁ > 2·t` and `3·s₂ > 2·t` force `t < s₁ + s₂`. This
    is the whole of clause a: a denominator shared by two groups cannot be a supermajority *of each*, so a
    group that finalises against the whole bonded stake commits the chain to a history the other group
    cannot match — and a group that finalises against only its own participants commits to nothing. -/
theorem supermajorities_overlap (s₁ s₂ t : Nat) (h₁ : isSuperMajority s₁ t)
    (h₂ : isSuperMajority s₂ t) : t < s₁ + s₂ := by
  unfold isSuperMajority at h₁ h₂
  omega

/-- The four-bond fixture: A, B, C and D at 100 each — and the two sides of a partition of it. -/
def bonds4 : Bonds := [(0, 100), (1, 100), (2, 100), (3, 100)]

/-- The `{A, B}` side of a partition of the fixture. -/
def bondsAB : Bonds := [(0, 100), (1, 100)]

/-- The `{C, D}` side of the same partition — disjoint from `bondsAB`, and the point of clause a. -/
def bondsCD : Bonds := [(2, 100), (3, 100)]

/-- A support map in which each of `candidates` has one entry per `seer` in `seers`, and every seer's seen
    set is `seers` itself — the shape a full partition of `seers` has. -/
def suppOf (candidates seers : List Sender) : SupportMap :=
  candidates.map fun c => (c, seers.map fun s => (s, seers))

/-- **Clause a's counterexample, both halves `decide`d on one fixture.** Two disjoint views each reach a
    strict supermajority *of themselves* — 200 of 200 — while neither reaches one of the bonded total,
    where 200 of 400 is not a supermajority. So a denominator taken from the live set lets each side
    finalise its own history, and the bonded denominator does not. -/
theorem the_live_denominator_lets_two_sides_finalise :
    calculateFringe (suppOf [0, 1] [0, 1]) bondsAB bondsAB = true ∧
    calculateFringe (suppOf [2, 3] [2, 3]) bondsCD bondsCD = true ∧
    calculateFringe (suppOf [0, 1] [0, 1]) bondsAB bonds4 = false ∧
    calculateFringe (suppOf [2, 3] [2, 3]) bondsCD bonds4 = false := by
  refine ⟨?_, ?_, ?_, ?_⟩ <;> decide

/-- **…and the same fixture at the bonded denominator**: a partition over all four bonded validators
    advance (300 of 400), and the partial partition does not (200 of 400). The positive half is what
    keeps the counterexample from being read as "the gate can never fire". -/
theorem the_quorum_is_the_whole_bonded_map :
    calculateFringe (suppOf [0, 1, 2] [0, 1, 2, 3]) bonds4 bonds4 = true ∧
    calculateFringe (suppOf [0, 1] [0, 1]) bonds4 bonds4 = false := by
  refine ⟨?_, ?_⟩ <;> decide

/-! ## Clause b — a silent validator makes the whole-bonded partition empty

The general statement first, then its instance on the same fixture. The general one is the shape C174
measured at 80 %: *not* "the gate is slow", but "no state satisfies it". -/

/-- **One seer that omits a bonded validator makes `allBonded` false** — the predicate demands that a
    seer's seen set *contain* the bonded set, and a set omitting a member cannot. -/
theorem allBonded_false_of_a_silent_seer (bonded : List Sender)
    (seenBy : List (Sender × List Sender)) (v : Sender) (hv : v ∈ bonded)
    (q : Sender × List Sender) (hq : q ∈ seenBy) (hmiss : v ∉ q.2) :
    allBonded bonded seenBy = false := by
  cases h : allBonded bonded seenBy with
  | false => rfl
  | true =>
    have hconj : ¬ seenBy = [] ∧ ∀ x ∈ seenBy, ∀ b ∈ bonded, b ∈ x.2 := by
      simpa [allBonded] using h
    exact absurd (hconj.2 q hq v hv) hmiss

/-- **…and if every seer of a candidate omits it, no candidate is a full partition** — the empty seer
    list included, which `allBonded` refuses for its own reason. -/
theorem allBonded_false_of_seers_omitting (bonded : List Sender)
    (seers : List (Sender × List Sender)) (v : Sender) (hv : v ∈ bonded)
    (hmiss : ∀ q ∈ seers, v ∉ q.2) : allBonded bonded seers = false := by
  rcases seers with _ | ⟨q, qs⟩
  · simp [allBonded]
  · exact allBonded_false_of_a_silent_seer bonded (q :: qs) v hv q (by simp) (hmiss q (by simp))

/-- **…so nothing is ever supported, and the gate cannot fire.** The stake the full-partition filter sums
    is zero when a bonded validator is silent — this is C174's shape as an *implication from the
    predicate* rather than a fixture, which is what makes it a statement about the rule: the requirement
    is empty, not slow. -/
theorem fullPartitionStake_eq_zero_of_a_silent_bonded (supp : SupportMap) (partition quorum : Bonds)
    {v : Sender} (hv : v ∈ bondedSenders partition)
    (hsilent : ∀ p ∈ supp, ∀ q ∈ p.2, v ∉ q.2) :
    fullPartitionStake supp partition quorum = 0 := by
  unfold fullPartitionStake bondedSupport
  induction supp with
  | nil => rfl
  | cons p ps ih =>
    have hfalse : allBonded (bondedSenders partition) p.2 = false :=
      allBonded_false_of_seers_omitting _ p.2 v hv (hsilent p (by simp))
    have ih' := ih (fun r hr => hsilent r (by simp [hr]))
    rw [List.filter_cons_of_neg (p := fun x : Sender × List (Sender × List Sender) =>
      allBonded (bondedSenders partition) x.2) (by simp [hfalse])]
    exact ih'

/-- **Clause b, as a `decide`d pair on the fixture.** With D silent the gate refuses the three speaking
    validators' candidates *whatever their stake* — 300 of 400, a supermajority — and the identical
    fixture advances the moment D speaks. The second half is the non-vacuity half: the filter is not
    always false, which is what makes the first half a finding rather than a definition. -/
theorem the_whole_bonded_partition_is_unsatisfiable :
    calculateFringe (suppOf [0, 1, 2] [0, 1, 2]) bonds4 bonds4 = false ∧
    calculateFringe (suppOf [0, 1, 2] [0, 1, 2, 3]) bonds4 bonds4 = true := by
  refine ⟨?_, ?_⟩ <;> decide

/-! ## The finality decision as a phase space

The Rust `FinalityReport` (`block-storage/src/dag/liveness.rs`) records the same coordinates this
structure names: the whole bonded total, the quorum denominator the gate was given, the live
partition, and the supporting stake the gate computed. A trace is an inhabitant; the evaluator
recomputes `finalitySafe` from it.
-/

/-- The coordinates the finality decision reads. -/
structure FinalityDecision where
  /-- The whole bonded stake: the denominator safety needs. -/
  bondedTotal : Nat
  /-- The denominator the gate actually used. Equal to `bondedTotal` iff the rule is safe. -/
  quorumTotal : Nat
  /-- The live partition: who must have seen a candidate. -/
  partition : List Sender
  /-- The bonded senders, so partition membership is checkable. -/
  bondedSenders : List Sender
  /-- The stake the gate found behind the candidates. -/
  supporting : Nat
  /-- Whether the fringe advanced. -/
  advanced : Bool

/-- **The safe predicate**: the denominator is the whole bonded map, every partition member is
    bonded, and an advance carries a strict supermajority of that denominator. -/
def finalitySafe (d : FinalityDecision) : Prop :=
  d.quorumTotal = d.bondedTotal ∧
  (∀ v ∈ d.partition, v ∈ d.bondedSenders) ∧
  (d.advanced = true → isSuperMajority d.supporting d.quorumTotal)

/-- A safe advance is a supermajority of the whole bonded stake. -/
theorem a_safe_advance_has_a_supermajority (d : FinalityDecision) (h : finalitySafe d)
    (ha : d.advanced = true) : isSuperMajority d.supporting d.quorumTotal :=
  h.2.2 ha

/-- A safe decision's denominator is the whole bonded total. -/
theorem finality_safe_denominator (d : FinalityDecision) (h : finalitySafe d) :
    d.quorumTotal = d.bondedTotal := h.1

/-- A safe decision's partition is drawn from the bonded senders. -/
theorem finality_safe_partition (d : FinalityDecision) (h : finalitySafe d) (v : Sender)
    (hv : v ∈ d.partition) : v ∈ d.bondedSenders := h.2.1 v hv

/-- The outcome type: advance or refuse, total over the input. -/
inductive FinalityOutcome where
  | advanced
  | refused

/-- The total decision function. -/
def finalityOutcome (d : FinalityDecision) : FinalityOutcome :=
  if d.advanced then .advanced else .refused

/-- **Totality**: every input has exactly one outcome. -/
theorem finality_decision_is_total (d : FinalityDecision) :
    ∃! o : FinalityOutcome, finalityOutcome d = o := by
  refine ⟨finalityOutcome d, rfl, ?_⟩
  intro o ho
  exact ho.symm

/-- **The unsafe witness**: the same 200-stake side that advances against its own 200 denominator
    is not safe against the bonded 400, and `the_live_denominator_lets_two_sides_finalise` proves
    the gate really does advance there. -/
def liveDenominatorWitness : FinalityDecision :=
  { bondedTotal := 400, quorumTotal := 200, partition := [0, 1],
    bondedSenders := [0, 1, 2, 3], supporting := 200, advanced := true }

theorem the_live_denominator_is_the_unsafe_witness : ¬ finalitySafe liveDenominatorWitness := by
  intro h
  have hden : (200 : Nat) = 400 := h.1
  exact absurd hden (by decide)

end Rchain
