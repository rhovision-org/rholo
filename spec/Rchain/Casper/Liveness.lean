import Rchain.Casper.Bonds
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

The Rust `FinalityReport` (`block-storage/src/dag/liveness.rs`) and `FinalityTrace`
(`casper/src/multi_parent_casper.rs`) record the same coordinates this structure names: the whole
bonded total, the quorum denominator, the live partition, the supporting stake, and — since the
bond-leave stall — the refusal's own counts and the senders whose candidate was not seen by the whole
partition. A trace is an inhabitant; the evaluator recomputes these predicates from it.

**The gap this closes.** `FinalityOutcome.refused` was nullary and `supporting` was a free `Nat`, so a
reasonless refusal was representable and `finalitySafe` was trivially true for it. A bond-leave stall
that read `200 of 300 (3 of 4)` was recorded as `full_partitions: 0, candidates: 0` and read as a
model gap. A refusal now carries its coordinates, and a safe decision cannot refuse without one.
-/

/-- A refusal's coordinates: the gate's counts and the senders it could not cover. -/
structure NoAdvance where
  supporting : Nat
  total : Nat
  fullPartitions : Nat
  candidates : Nat
  partitionSize : Nat
  /-- Support refusal: senders whose candidate was not seen by the whole partition. -/
  notFull : List Sender
  /-- Coverage refusal: partition members with no message in the region. -/
  missing : List Sender
deriving DecidableEq

/-- The support map's senders that are not full partitions. -/
def notFullSenders (supp : SupportMap) (partition : Bonds) : List Sender :=
  (supp.map (·.1)).filter (fun s => !(bondedSupport supp (bondedSenders partition)).contains s)

/-- The refusal a gate computation produces, with its own numbers. -/
def noAdvance (supp : SupportMap) (partition quorum : Bonds) : NoAdvance :=
  { supporting := fullPartitionStake supp partition quorum
    total := totalStake quorum
    fullPartitions := (bondedSupport supp (bondedSenders partition)).length
    candidates := supp.length
    partitionSize := (bondedSenders partition).length
    notFull := notFullSenders supp partition
    missing := [] }

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
  /-- The refusal's coordinates; `none` on an advance. A **safe** refusal cannot leave this `none`. -/
  reason : Option NoAdvance := none

/-- **The safe predicate**: the denominator is the whole bonded map, every partition member is
    bonded, an advance carries a strict supermajority of that denominator, and a refusal names its
    coordinates. -/
def finalitySafe (d : FinalityDecision) : Prop :=
  d.quorumTotal = d.bondedTotal ∧
  (∀ v ∈ d.partition, v ∈ d.bondedSenders) ∧
  (d.advanced = true → isSuperMajority d.supporting d.quorumTotal) ∧
  (d.advanced = false → d.reason.isSome)

/-- A safe advance is a supermajority of the whole bonded stake. -/
theorem a_safe_advance_has_a_supermajority (d : FinalityDecision) (h : finalitySafe d)
    (ha : d.advanced = true) : isSuperMajority d.supporting d.quorumTotal :=
  h.2.2.1 ha

/-- A safe decision's denominator is the whole bonded total. -/
theorem finality_safe_denominator (d : FinalityDecision) (h : finalitySafe d) :
    d.quorumTotal = d.bondedTotal := h.1

/-- A safe decision's partition is drawn from the bonded senders. -/
theorem finality_safe_partition (d : FinalityDecision) (h : finalitySafe d) (v : Sender)
    (hv : v ∈ d.partition) : v ∈ d.bondedSenders := h.2.1 v hv

/-- **A safe refusal names its coordinates.** The silent stall is the `advanced = false`,
    `reason = none` state; `finalitySafe` excludes it. -/
theorem a_safe_refusal_names_its_reason (d : FinalityDecision) (h : finalitySafe d)
    (hr : d.advanced = false) : d.reason.isSome := h.2.2.2 hr

/-- The outcome type: an advance carries its supermajority proof, a refusal its reason. -/
inductive FinalityOutcome where
  | advanced (supporting quorumTotal : Nat) (h : isSuperMajority supporting quorumTotal)
  | refused (reason : NoAdvance)

/-- The decision function. A safe refusal always has a reason, so the default is unreachable. -/
def finalityOutcome (d : FinalityDecision) (h : finalitySafe d) : FinalityOutcome :=
  if hd : d.advanced = true then
    .advanced d.supporting d.quorumTotal (h.2.2.1 hd)
  else
    .refused (d.reason.getD (noAdvance [] [] []))

/-- **Totality**: every safe decision has exactly one outcome shape. -/
theorem finality_decision_is_total (d : FinalityDecision) (h : finalitySafe d) :
    (∃ s t p, finalityOutcome d h = .advanced s t p) ∨
    (∃ n, finalityOutcome d h = .refused n) := by
  by_cases hd : d.advanced = true
  · exact Or.inl ⟨d.supporting, d.quorumTotal, h.2.2.1 hd,
      by simp [finalityOutcome, hd]⟩
  · exact Or.inr ⟨d.reason.getD (noAdvance [] [] []),
      by simp [finalityOutcome, hd]⟩

/-! ### The gate's decision, with the numbers it was made of -/

/-- The decision the gate computes, carrying its own supporting stake and refusal coordinates. -/
def gateDecision (supp : SupportMap) (partition quorum : Bonds) : FinalityDecision :=
  { bondedTotal := totalStake quorum
    quorumTotal := totalStake quorum
    partition := bondedSenders partition
    bondedSenders := bondedSenders quorum
    supporting := fullPartitionStake supp partition quorum
    advanced := calculateFringe supp partition quorum
    reason := if calculateFringe supp partition quorum then none
              else some (noAdvance supp partition quorum) }

/-- **The support is the full-partition stake, not a free field.** -/
theorem the_support_is_the_full_partition_stake (supp : SupportMap) (partition quorum : Bonds) :
    (gateDecision supp partition quorum).supporting
      = fullPartitionStake supp partition quorum := rfl

/-- **The denominator is the active snapshot's bond total** (Law 16d's `bondsOfState`). -/
theorem the_gate_denominator_is_the_active_snapshot (active : ActiveBonds) (supp : SupportMap)
    (partition : Bonds) :
    (gateDecision supp partition (bondsOfState active)).bondedTotal
      = totalStake (bondsOfState active) := rfl

/-- **The gate's decision is safe** whenever the partition is drawn from the quorum: the denominator
    is the quorum total, an advance is a supermajority by construction, and a refusal carries a
    reason by construction. -/
theorem the_gate_decision_is_safe (supp : SupportMap) (partition quorum : Bonds)
    (hsub : ∀ v ∈ bondedSenders partition, v ∈ bondedSenders quorum) :
    finalitySafe (gateDecision supp partition quorum) := by
  refine ⟨rfl, hsub, ?_, ?_⟩
  · intro ha
    simp only [gateDecision] at ha
    exact of_decide_eq_true ha
  · intro ha
    simp only [gateDecision] at ha ⊢
    by_cases h : calculateFringe supp partition quorum = true
    · simp [h] at ha
    · simp [h]

/-- A refusal carries the gate's own supporting and total. -/
theorem a_refusal_carries_the_gates_numbers (supp : SupportMap) (partition quorum : Bonds)
    {n : NoAdvance} (hn : (gateDecision supp partition quorum).reason = some n) :
    n.supporting = fullPartitionStake supp partition quorum ∧ n.total = totalStake quorum := by
  by_cases h : calculateFringe supp partition quorum = true
  · simp [gateDecision, h] at hn
  · have heq : noAdvance supp partition quorum = n := by
      simpa [gateDecision, h] using hn
    subst heq
    exact ⟨rfl, rfl⟩

/-- A refusal is exactly the sub-supermajority arm. -/
theorem a_refusal_is_not_a_supermajority (supp : SupportMap) (partition quorum : Bonds)
    {n : NoAdvance} (hn : (gateDecision supp partition quorum).reason = some n) :
    ¬ isSuperMajority n.supporting n.total := by
  by_cases h : calculateFringe supp partition quorum = true
  · simp [gateDecision, h] at hn
  · have heq : noAdvance supp partition quorum = n := by
      simpa [gateDecision, h] using hn
    subst heq
    have hfalse : calculateFringe supp partition quorum = false := by
      cases hd : calculateFringe supp partition quorum <;> simp_all
    intro hs
    have hs' : isSuperMajority (fullPartitionStake supp partition quorum)
        (totalStake quorum) := by simpa [noAdvance] using hs
    have hdec : decide (isSuperMajority (fullPartitionStake supp partition quorum)
        (totalStake quorum)) = false := by simpa [calculateFringe] using hfalse
    rw [decide_eq_true hs'] at hdec
    exact Bool.true_eq_false.mp hdec

/-- A refusal names the senders whose candidate was not seen by the whole partition. -/
theorem a_refusal_names_the_not_full_senders (supp : SupportMap) (partition quorum : Bonds)
    {n : NoAdvance} (hn : (gateDecision supp partition quorum).reason = some n) :
    n.notFull = notFullSenders supp partition := by
  by_cases h : calculateFringe supp partition quorum = true
  · simp [gateDecision, h] at hn
  · have heq : noAdvance supp partition quorum = n := by
      simpa [gateDecision, h] using hn
    subst heq
    rfl

/-- **Every refusal is attributable**: it carries its numbers and the not-full senders. -/
theorem a_refusal_always_names_its_coordinates (supp : SupportMap) (partition quorum : Bonds)
    {n : NoAdvance} (hn : (gateDecision supp partition quorum).reason = some n) :
    n.notFull = notFullSenders supp partition ∧
    n.supporting = fullPartitionStake supp partition quorum ∧
    n.total = totalStake quorum := by
  have h1 := a_refusal_names_the_not_full_senders supp partition quorum hn
  have h2 := a_refusal_carries_the_gates_numbers supp partition quorum hn
  exact ⟨h1, h2.1, h2.2⟩

/-- **The bond-leave layer**: 100/100/50/50, the newcomer's candidate missed by one 100-stake
    original. The supporting stake is 100+50+50 = 200, exactly two thirds of 300. -/
def bondsJoin : Bonds := [(0, 100), (1, 100), (2, 50), (3, 50)]

def suppJoinPartial : SupportMap :=
  [ (0, [(0, [0, 1, 2, 3]), (1, [0, 1, 2, 3]), (2, [0, 1, 2, 3]), (3, [0, 1, 2, 3])]),
    (1, [(0, [0, 1, 2]), (1, [0, 1, 2, 3]), (2, [0, 1, 2, 3]), (3, [0, 1, 2, 3])]),
    (2, [(0, [0, 1, 2, 3]), (1, [0, 1, 2, 3]), (2, [0, 1, 2, 3]), (3, [0, 1, 2, 3])]),
    (3, [(0, [0, 1, 2, 3]), (1, [0, 1, 2, 3]), (2, [0, 1, 2, 3]), (3, [0, 1, 2, 3])]) ]

def suppJoinFull : SupportMap :=
  [ (0, [(0, [0, 1, 2, 3]), (1, [0, 1, 2, 3]), (2, [0, 1, 2, 3]), (3, [0, 1, 2, 3])]),
    (1, [(0, [0, 1, 2, 3]), (1, [0, 1, 2, 3]), (2, [0, 1, 2, 3]), (3, [0, 1, 2, 3])]),
    (2, [(0, [0, 1, 2, 3]), (1, [0, 1, 2, 3]), (2, [0, 1, 2, 3]), (3, [0, 1, 2, 3])]),
    (3, [(0, [0, 1, 2, 3]), (1, [0, 1, 2, 3]), (2, [0, 1, 2, 3]), (3, [0, 1, 2, 3])]) ]

theorem the_join_layer_is_exactly_two_thirds :
    fullPartitionStake suppJoinPartial bondsJoin bondsJoin = 200 ∧
    totalStake bondsJoin = 300 ∧ ¬ isSuperMajority 200 300 := by decide

theorem the_join_layer_refuses :
    calculateFringe suppJoinPartial bondsJoin bondsJoin = false := by decide

theorem the_join_layer_names_sender_one :
    (noAdvance suppJoinPartial bondsJoin bondsJoin).notFull = [1] := by decide

theorem the_join_layer_advances_once_all_see :
    calculateFringe suppJoinFull bondsJoin bondsJoin = true := by decide

/-! ### The inactivity leak — Law 129

The stall above is the exact-two-thirds boundary: sender 1's bond keeps the denominator at 300
while the supporting layer reaches 200. The leak retires the silent sender at a boundary, and the
survivors' 200 becomes the whole denominator. This is the priced instrument Law 52's clause b names.
-/

/-- The same bond set as an `ActiveBonds` snapshot (`pos:active`). -/
def bondsJoinActive : ActiveBonds :=
  [(⟨0⟩, 100), (⟨1⟩, 100), (⟨2⟩, 50), (⟨3⟩, 50)]

/-- **The leak lets the survivors finalise.** With sender 1 retired, the remaining 200 is the whole
    denominator, so the 200 supporting it is 200 of 200 — the positive counterpart to
    `the_join_layer_refuses`. -/
theorem the_join_layer_advances_after_leak :
    calculateFringe suppJoinPartial
      (bondsOfState (leak bondsJoinActive [1]))
      (bondsOfState (leak bondsJoinActive [1])) = true := by decide

/-- A speaker within the window is not retired: an empty silence set leaks nothing. -/
theorem a_speaker_within_the_window_does_not_leak :
    leak bondsJoinActive [] = bondsJoinActive := by decide

/-- **Law 129's proposition**: a leak retires only the named silent senders, never raises the
    denominator, and makes the bond-leave layer advance. -/
def law129_statement : Prop :=
  (∀ (active : ActiveBonds) (silent : List Sender),
      (∀ v ∈ silent, v ∉ bondedSenders (bondsOfState (leak active silent))) ∧
      totalStake (bondsOfState (leak active silent))
        ≤ totalStake (bondsOfState active)) ∧
  (calculateFringe suppJoinPartial
      (bondsOfState (leak bondsJoinActive [1]))
      (bondsOfState (leak bondsJoinActive [1])) = true)

theorem law129_statement_holds : law129_statement := by
  refine ⟨?_, the_join_layer_advances_after_leak⟩
  intro active silent
  exact ⟨fun v hv => the_leaked_sender_is_not_in_the_quorum active silent hv,
    totalStake_leak_le active silent⟩

/-- **The mutant**: a leak that retires every sender empties the quorum. -/
def law129_mutant : Prop :=
  ∃ (active : ActiveBonds) (silent : List Sender),
    (∀ b ∈ active, b.1.id ∈ silent) ∧ leak active silent = [] ∧ active ≠ []

theorem law129_falsifier : law129_mutant := by
  refine ⟨bondsJoinActive, [0, 1, 2, 3], ?_, ?_, ?_⟩ <;> decide

/-! ### The emitted trace refines the decision -/

inductive RefusalKind where
  | coverage
  | support
  | alreadyPublished
deriving DecidableEq

/-- The trace's fields, as `FinalityTrace` emits them. -/
structure FinalityTrace where
  bondedTotal : Nat
  quorumTotal : Nat
  supporting : Nat
  fullPartitions : Nat
  candidates : Nat
  partitionSize : Nat
  partition : List Sender
  bondedSenders : List Sender
  notFull : List Sender
  missing : List Sender
  advanced : Bool
  reasonKind : Option RefusalKind

/-- A trace refines a decision only if its refusal carries a reason and the counts cannot hide the
    missing senders. -/
def traceRefines (t : FinalityTrace) : Prop :=
  t.quorumTotal = t.bondedTotal ∧
  (t.advanced = true → isSuperMajority t.supporting t.quorumTotal ∧ t.reasonKind = none) ∧
  (t.advanced = false → t.reasonKind.isSome) ∧
  (t.reasonKind = some .support → t.notFull.length + t.fullPartitions = t.candidates)

/-- A refusing trace carries a reason. -/
theorem a_refusal_trace_carries_its_reason (t : FinalityTrace) (h : traceRefines t)
    (hr : t.advanced = false) : t.reasonKind.isSome := h.2.2.1 hr

/-- **A reasonless refusal is not a refinement** — the silent stall, unrepresentable. -/
theorem a_reasonless_refusal_is_not_a_refinement (t : FinalityTrace)
    (hr : t.advanced = false) (hn : t.reasonKind = none) : ¬ traceRefines t := by
  intro h
  have hisSome := h.2.2.1 hr
  simp [hn] at hisSome

/-- **The formal proposition**: the gate's denominator is the active snapshot's bond total. -/
def finalityDenominatorLaw : Prop :=
  ∀ (active : ActiveBonds) (supp : SupportMap) (partition : Bonds),
    (gateDecision supp partition (bondsOfState active)).bondedTotal
      = totalStake (bondsOfState active)

theorem finalityDenominatorLaw_holds : finalityDenominatorLaw := fun _ _ _ => rfl

/-- The stale-denominator decision: 250 quorum against 300 bonded. -/
def staleJoinDenominatorDecision : FinalityDecision :=
  { bondedTotal := 300, quorumTotal := 250, partition := [0, 1, 2],
    bondedSenders := [0, 1, 2, 3], supporting := 250, advanced := true }

/-- **The stale-denominator mutant proposition**: the unsafe decision is "safe". -/
def staleJoinDenominator : Prop := finalitySafe staleJoinDenominatorDecision

theorem staleJoinDenominator_is_not_safe : ¬ staleJoinDenominator := by
  intro h
  change finalitySafe staleJoinDenominatorDecision at h
  exact absurd h.1 (by decide)

/-- **The formal proposition for attribution**: a refusing trace carries a reason. -/
def attributableRefusalLaw : Prop :=
  ∀ (supp : SupportMap) (partition quorum : Bonds) (t : FinalityTrace),
    traceRefines t → t.advanced = false → t.reasonKind.isSome

theorem attributableRefusalLaw_holds : attributableRefusalLaw :=
  fun _ _ _ t h => a_refusal_trace_carries_its_reason t h

/-- The reasonless-refusal trace: the silent stall as a trace. -/
def silentRefusalTrace : FinalityTrace :=
  { bondedTotal := 300, quorumTotal := 300, supporting := 200, fullPartitions := 3,
    candidates := 4, partitionSize := 4, partition := [0, 1, 2, 3],
    bondedSenders := [0, 1, 2, 3], notFull := [1], missing := [],
    advanced := false, reasonKind := none }

/-- **The reasonless-refusal mutant proposition**: that trace refines a decision. -/
def silentRefusal : Prop := traceRefines silentRefusalTrace

theorem silentRefusal_is_not_a_refinement : ¬ silentRefusal := by
  intro h
  change traceRefines silentRefusalTrace at h
  exact a_reasonless_refusal_is_not_a_refinement silentRefusalTrace rfl rfl h

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
