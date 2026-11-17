import Mathlib.Data.Finset.Basic
import Mathlib.Data.Finset.Image
import Mathlib.Data.Finset.Sort
import Mathlib.Algebra.BigOperators.Group.Finset
import Mathlib.Tactic

/-!
# Law 89 — liveness enforcement: jail first, a vote for the bond

A validator that stops keeps its bond and counts in the finality denominator for ever. The node's
response (RCHIP #55) is two steps at an **advancing** epoch boundary:

* a sustained absentee is **jailed** — removed from the active set for `J` boundaries and paid
  nothing — while its bond and its delegators' principal are untouched;
* only a **stake-weighted vote strictly above two thirds of the active stake**, opened against an
  already-jailed validator, may **slash** and permanently eject it.

The presence signal is derived, not a parameter: a validator is present at a boundary when it
**authored a finalised-fringe block or signed a deploy included in one**, at the member's height;
`participationMap` takes the **maximum** height per validator
(`casper/src/multi_parent_casper.rs:770-818`). The boundary's `quorumTip` is the greatest height
whose active supporting stake is strictly more than two thirds of the active total
(`rholang/src/pos_liveness.rs:221-246`), and a validator is absent when it has no entry or is more
than the threshold behind that tip. A stalled boundary (`tip ≤ anchor`) changes nothing.

The state machine is `step`: the vote tally runs first (against open windows and the current active
set), then the absence counter and the jail, then expiry (renew while absent, release while present),
then window aging. A jailed validator leaves `active`, so it is out of the vote denominator
(the exact-threshold arm's denominator is 900, not 1000); release re-enters it. The jail and the
expiry paths never write `pool` or `withdrawals`; only `tally` and `slashTarget` do, and only for a
target whose vote passed. The slash's distribution to delegators is Law 57's subject.
-/

namespace Rchain
namespace Liveness

open scoped BigOperators

/-- Validator identities are opaque numbers in the model; the Rust side uses 65-byte keys. -/
abbrev Validator := Nat

/-- The liveness parameters. `voteWindow = 0` disables vote windows (the restart-expiry arm);
    `slash = false` disables the slash action. -/
structure Params where
  absenceThreshold : Nat
  jailAfter : Nat
  jailEpochs : Nat
  voteWindow : Nat
  slash : Bool

/-- The liveness-relevant state. `pool` is the bond map (`none` = not bonded); `active` is the drawn
    finality set and a jailed validator is removed from it; `jailedSet` is the record key set;
    `windows` are the targets with an open vote window; `ballots` are the in-favour
    `(target, voter)` pairs. -/
structure State where
  pool : Validator → Option Nat
  active : Finset Validator
  banned : Finset Validator
  absent : Validator → Nat
  jailRemaining : Validator → Nat
  jailedSet : Finset Validator
  windows : Finset Validator
  windowLeft : Validator → Nat
  ballots : Finset (Validator × Validator)
  withdrawals : Validator → Nat
  anchorTip : Nat

/-- A validator is bonded at `stake`. -/
def stakeOf (s : State) (v : Validator) : Nat := (s.pool v).getD 0

/-- A validator is jailed iff its remaining term is positive. -/
def jailed (s : State) (v : Validator) : Prop := s.jailRemaining v > 0

/-- The total active stake — the vote denominator. A jailed validator is not in `active`. -/
def totalStake (s : State) : Nat := ∑ v in s.active, stakeOf s v

/-- The active stake whose in-favour ballot for `target` is recorded. -/
def forStake (s : State) (target : Validator) : Nat :=
  ∑ v in s.active.filter (fun v => (target, v) ∈ s.ballots), stakeOf s v

/-- **Strictly more than two thirds.** The same integer rule as `is_super_majority`. -/
def passes (s : State) (target : Validator) : Bool :=
  decide (3 * forStake s target > 2 * totalStake s)

/-- `f` updated at one key. -/
def setNat (f : Validator → Nat) (v : Validator) (x : Nat) : Validator → Nat :=
  fun w => if w = v then x else f w

/-! ### Presence and the quorum tip -/

/-- The presence heights recorded for one validator. -/
def heights (entries : Finset (Validator × Nat)) (v : Validator) : Finset Nat :=
  (entries.filter (fun e => e.1 = v)).image (fun e => e.2)

/-- The maximum height per validator — the Rust `participation_map`. It also unions the authors and
    the deploy signers at the member's height. -/
def participationMap (entries : Finset (Validator × Nat)) (v : Validator) : Option Nat :=
  if h : (heights entries v).Nonempty then some ((heights entries v).max' h) else none

/-- The presence signal the node supplies: the max over fringe authors and included deploy signers. -/
def presentFromAuthorsAndDeployers (authors deployers : Finset (Validator × Nat)) :
    Validator → Option Nat :=
  participationMap (authors ∪ deployers)

/-- Presence is a map entry. -/
def present (p : Validator → Option Nat) (v : Validator) : Bool := (p v).isSome

/-- **Absence is missing or more than `threshold` behind the tip** (`pos_liveness.rs:251-261`). The
    comparison is strict: exactly `threshold` behind is present. -/
def isAbsent (p : Validator → Option Nat) (tip threshold : Nat) (v : Validator) : Bool :=
  match p v with
  | none => true
  | some h => decide (tip - h > threshold)

/-- Whether a validator's presence is at least `h`. -/
def atOrAbove (p : Validator → Option Nat) (v : Validator) (h : Nat) : Bool :=
  match p v with
  | none => false
  | some x => decide (x ≥ h)

/-- The active supporting stake at height `h`. -/
def support (s : State) (p : Validator → Option Nat) (h : Nat) : Nat :=
  ∑ v in s.active.filter (fun v => atOrAbove p v h = true), stakeOf s v

/-- A height whose supporting active stake is strictly more than two thirds, as a decision. -/
def isQuorumHeightBool (s : State) (p : Validator → Option Nat) (h : Nat) : Bool :=
  decide (3 * support s p h > 2 * totalStake s)

/-- A height whose supporting active stake is strictly more than two thirds. -/
def isQuorumHeight (s : State) (p : Validator → Option Nat) (h : Nat) : Prop :=
  isQuorumHeightBool s p h = true

/-- The presence heights of the active validators. -/
def candidateHeights (s : State) (p : Validator → Option Nat) : Finset Nat :=
  s.active.biUnion (fun v => match p v with
    | none => ∅
    | some h => {h})

/-- **The quorum tip**: the greatest presence height with a strict supermajority, or 0. This is the
    value the Rust's descending accumulation returns at its first crossing. -/
noncomputable def quorumTip (s : State) (p : Validator → Option Nat) : Nat :=
  if h : ((candidateHeights s p).filter (fun h => isQuorumHeightBool s p h = true)).Nonempty then
    ((candidateHeights s p).filter (fun h => isQuorumHeightBool s p h = true)).max' h
  else 0

/-- A held entry is present. -/
theorem heights_mem (entries : Finset (Validator × Nat)) (v h : Nat)
    (hmem : (v, h) ∈ entries) : h ∈ heights entries v := by
  simp only [heights, Finset.mem_image, Finset.mem_filter]
  exact ⟨(v, h), ⟨hmem, rfl⟩, rfl⟩

/-- A recorded height is present at that height or above. -/
theorem participationMap_some (entries : Finset (Validator × Nat)) (v h : Nat)
    (hmem : (v, h) ∈ entries) :
    ∃ m, participationMap entries v = some m ∧ h ≤ m := by
  have hne : (heights entries v).Nonempty := ⟨h, heights_mem entries v h hmem⟩
  refine ⟨(heights entries v).max' hne, ?_, ?_⟩
  · simp [participationMap, hne]
  · exact Finset.le_max' (heights entries v) h (heights_mem entries v h hmem)

/-- A present map value is a recorded height. -/
theorem participationMap_mem (entries : Finset (Validator × Nat)) (v m : Nat)
    (h : participationMap entries v = some m) : (v, m) ∈ entries := by
  by_cases hne : (heights entries v).Nonempty
  · simp [participationMap, hne] at h
    have hm : (heights entries v).max' hne = m := by simpa using h
    have hm_mem : m ∈ heights entries v := by
      rw [← hm]
      exact Finset.max'_mem _ hne
    simp only [heights, Finset.mem_image, Finset.mem_filter] at hm_mem
    rcases hm_mem with ⟨e, ⟨he, hv⟩, hval⟩
    have heq : e = (v, m) := by
      cases e with
      | mk a b =>
        simp only at hv hval
        subst hv
        subst hval
        rfl
    simpa [heq] using he
  · simp [participationMap, hne] at h

/-- **Presence is derived from the maximum recorded height.** -/
theorem presence_is_the_max_participation_height (entries : Finset (Validator × Nat))
    (v m : Nat) (h : participationMap entries v = some m) :
    present (participationMap entries) v = true ∧ (v, m) ∈ entries ∧
      ∀ e ∈ entries, e.1 = v → e.2 ≤ m := by
  constructor
  · simp [present, h]
  constructor
  · exact participationMap_mem entries v m h
  · intro e he hv
    have hmem : e.2 ∈ heights entries v := by
      simp only [heights, Finset.mem_image, Finset.mem_filter]
      exact ⟨e, ⟨he, hv⟩, rfl⟩
    have hne : (heights entries v).Nonempty := ⟨e.2, hmem⟩
    have hm : (heights entries v).max' hne = m := by
      rw [participationMap, dif_pos hne] at h
      exact Option.some.inj h
    rw [← hm]
    exact Finset.le_max' (heights entries v) e.2 hmem

/-- An entry makes the validator present. -/
theorem present_of_mem (entries : Finset (Validator × Nat)) (v h : Nat)
    (hmem : (v, h) ∈ entries) : present (participationMap entries) v = true := by
  obtain ⟨m, hm, _⟩ := participationMap_some entries v h hmem
  simp [present, hm]

/-- **A deploy signature presents a jailed validator.** The validator cannot author (the proposer
    gate), but its key signed a deploy an unjailed proposer included. -/
theorem a_deploy_signer_can_present_a_jailed_validator
    (s : State) (authors deployers : Finset (Validator × Nat)) (v h : Nat)
    (_hjailed : s.jailRemaining v > 0) (hdeploy : (v, h) ∈ deployers) :
    present (presentFromAuthorsAndDeployers authors deployers) v = true := by
  exact present_of_mem (authors ∪ deployers) v h (Finset.mem_union.mpr (Or.inr hdeploy))

/-- The proposer gate: a jailed validator does not author. -/
def gatedSenders (s : State) (authors : Finset (Validator × Nat)) : Prop :=
  ∀ v h, s.jailRemaining v > 0 → (v, h) ∉ authors

/-- **The retired authors-only model**, kept as the mutant. Under it a jailed validator is never
    present and release is unreachable. -/
def presentFromSenders (authors : Finset (Validator × Nat)) : Validator → Option Nat :=
  participationMap authors

/-- Under the authors-only model a jailed validator is never present. -/
theorem a_jailed_validator_is_never_present (s : State) (authors : Finset (Validator × Nat))
    (hgate : gatedSenders s authors) (v : Nat) (hjailed : s.jailRemaining v > 0) :
    present (presentFromSenders authors) v = false := by
  unfold present presentFromSenders
  by_cases h : participationMap authors v = none
  · simp [h]
  · obtain ⟨m, hm⟩ : ∃ m, participationMap authors v = some m := by
      cases hp : participationMap authors v with
      | none => contradiction
      | some m => exact ⟨m, rfl⟩
    exact absurd (participationMap_mem authors v m hm) (hgate v m hjailed)

/-- **Absence is missing or strictly more than the threshold behind the tip.** -/
theorem absence_is_missing_or_behind_the_threshold (p : Validator → Option Nat) (tip threshold : Nat)
    (v : Validator) :
    isAbsent p tip threshold v = true ↔
      p v = none ∨ ∃ h, p v = some h ∧ tip - h > threshold := by
  unfold isAbsent
  cases p v with
  | none => simp
  | some h => simp

/-- **A present entry is not absent.** -/
theorem presence_is_not_absent (p : Validator → Option Nat) (tip threshold : Nat)
    (v h : Nat) (hp : p v = some h) (hthreshold : tip - h ≤ threshold) :
    isAbsent p tip threshold v = false := by
  simp only [isAbsent, hp]
  have hnot : ¬ tip - h > threshold := by omega
  simp [hnot]

/-- **Exactly the threshold behind is present.** -/
theorem exactly_the_threshold_is_present (p : Validator → Option Nat) (tip threshold : Nat)
    (v h : Nat) (hp : p v = some h) (heq : tip - h = threshold) :
    isAbsent p tip threshold v = false :=
  presence_is_not_absent p tip threshold v h hp (le_of_eq heq)

/-- **The quorum tip is the greatest height a strict supermajority reached.** -/
theorem quorum_tip_is_the_greatest_height (s : State) (p : Validator → Option Nat) :
    quorumTip s p = 0 ∨
      (isQuorumHeight s p (quorumTip s p) ∧
        ∀ h ∈ candidateHeights s p, h > quorumTip s p → ¬ isQuorumHeight s p h) := by
  unfold quorumTip
  by_cases hne :
      ((candidateHeights s p).filter (fun h => isQuorumHeightBool s p h = true)).Nonempty
  · right
    rw [dif_pos hne]
    have hmem := Finset.max'_mem _ hne
    simp only [Finset.mem_filter] at hmem
    refine ⟨by simpa [isQuorumHeight] using hmem.2, ?_⟩
    intro h hcand hgt hq
    have hinh :
        h ∈ (candidateHeights s p).filter (fun h => isQuorumHeightBool s p h = true) :=
      Finset.mem_filter.mpr ⟨hcand, by simpa [isQuorumHeight] using hq⟩
    have := Finset.le_max' _ h hinh
    omega
  · left
    rw [dif_neg hne]

/-! ### The state machine -/

/-- The boundaries advance only when the tip moves past the anchor. -/
def advances (s : State) (tip : Nat) : Bool :=
  decide (tip ≠ 0 ∧ tip > s.anchorTip)

/-- The absence counter this boundary would take. -/
def countedAbsent (params : Params) (p : Validator → Option Nat) (tip : Nat)
    (s : State) (v : Validator) : Nat :=
  if v ∈ s.active then
    if isAbsent p tip params.absenceThreshold v then s.absent v + 1 else 0
  else s.absent v

/-- The active validators whose counter reaches the jail threshold this boundary. -/
def newlyJailed (params : Params) (p : Validator → Option Nat) (tip : Nat)
    (s : State) : Finset Validator :=
  s.active.filter (fun v => s.jailRemaining v = 0 ∧ countedAbsent params p tip s v ≥ params.jailAfter)

/-- The jail step: remove the newly jailed from `active`, set their term, and open their window. -/
def jailNew (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
    (newly : Finset Validator) : State :=
  { s with
    absent := fun v => if v ∈ newly then 0 else countedAbsent params p tip s v,
    jailRemaining := fun v => if v ∈ newly then params.jailEpochs else s.jailRemaining v,
    jailedSet := s.jailedSet ∪ newly,
    active := s.active \ newly,
    windows := if params.voteWindow = 0 then s.windows else s.windows ∪ newly,
    windowLeft := fun v => if v ∈ newly then params.voteWindow else s.windowLeft v }

/-- The release path: remove the record, the window and the ballots, and re-enter `active`. -/
def releaseTarget (s : State) (v : Validator) : State :=
  { s with
    jailRemaining := setNat s.jailRemaining v 0,
    jailedSet := s.jailedSet.erase v,
    active := if (s.pool v).isSome then insert v s.active else s.active,
    windows := s.windows.erase v,
    windowLeft := setNat s.windowLeft v 0,
    ballots := s.ballots.filter (fun b => b.1 ≠ v) }

/-- The renewal path: the jail is re-imposed for the full term and the window re-opens. -/
def renewTarget (params : Params) (s : State) (v : Validator) : State :=
  { s with
    jailRemaining := setNat s.jailRemaining v params.jailEpochs,
    windows := if params.voteWindow = 0 then s.windows else insert v s.windows,
    windowLeft := setNat s.windowLeft v params.voteWindow }

/-- One jailed validator at its expiry boundary. -/
def expireTarget (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
    (v : Validator) : State :=
  if s.jailRemaining v ≤ 1 then
    if isAbsent p tip params.absenceThreshold v then renewTarget params s v else releaseTarget s v
  else
    { s with jailRemaining := setNat s.jailRemaining v (s.jailRemaining v - 1) }

/-- Expire every listed jail (the newly imposed ones are excluded). -/
def expireAll (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
    (targets : Finset Validator) : State :=
  (Finset.sort (· ≤ ·) targets).foldl (fun st v => expireTarget params p tip st v) s

/-- Close one window and drop its ballots. -/
def closeWindow (s : State) (v : Validator) : State :=
  { s with windows := s.windows.erase v, windowLeft := setNat s.windowLeft v 0, ballots := s.ballots.filter (fun b => b.1 ≠ v) }

/-- Decrement one window's remaining term. -/
def decrementWindow (s : State) (v : Validator) : State :=
  { s with windowLeft := setNat s.windowLeft v (s.windowLeft v - 1) }

/-- The window aging: close at zero, when the target is not jailed, or when it left the pool. -/
def ageWindow (s : State) (v : Validator) : State :=
  if s.jailRemaining v = 0 ∨ (s.pool v).isNone then closeWindow s v
  else if s.windowLeft v ≤ 1 then closeWindow s v
  else decrementWindow s v

/-- Age every window that was already open; the newly opened ones are skipped. -/
def ageAll (s : State) (newly : Finset Validator) : State :=
  (Finset.sort (· ≤ ·) (s.windows \ newly)).foldl (fun st v => ageWindow st v) s

/-- **The slash action**: remove the bond, active entry, jail record, window, ballots and staged
    withdrawal, and ban the validator. The distribution to delegators is Law 57's. -/
def slashTarget (s : State) (t : Validator) : State :=
  { s with
    pool := fun v => if v = t then none else s.pool v
    active := s.active.erase t
    banned := insert t s.banned
    absent := setNat s.absent t 0
    jailRemaining := setNat s.jailRemaining t 0
    jailedSet := s.jailedSet.erase t
    windows := s.windows.erase t
    windowLeft := setNat s.windowLeft t 0
    ballots := s.ballots.filter (fun b => b.1 ≠ t)
    withdrawals := setNat s.withdrawals t 0 }

/-- The windows whose vote passes this boundary, computed against the active set *before* any
    slash (the Rust tallies every open window first). -/
def passing (s : State) : Finset Validator :=
  s.windows.filter (fun t => (s.pool t).isSome ∧ passes s t = true)

/-- **The vote tally**: slash every passing target. With `slash = false` nothing is written. -/
def tally (params : Params) (s : State) : State :=
  if params.slash then
    (Finset.sort (· ≤ ·) (passing s)).foldl (fun st t => slashTarget st t) s
  else s

/-- **The boundary step.** Off-boundary, a zero tip, or a tip that did not advance is a no-op. On an
    advancing boundary: tally, jail, expire/renew, age, and record the new tip. -/
def step (params : Params) (p : Validator → Option Nat) (onBoundary : Bool) (tip : Nat)
    (s : State) : State :=
  if onBoundary && advances s tip then
    let s1 := tally params s
    let newly := newlyJailed params p tip s1
    let s2 := jailNew params p tip s1 newly
    let s3 := expireAll params p tip s2 (s2.jailedSet \ newly)
    let s4 := ageAll s3 newly
    { s4 with anchorTip := tip }
  else s

/-- A ballot is accepted only for an active voter and a bonded, non-banned target with an open
    window. A vote against an active validator (no window) is not recorded. -/
def castVote (s : State) (voter target : Validator) (inFavor : Bool) : State :=
  if inFavor && target ∈ s.windows && voter ∈ s.active && (s.pool target).isSome &&
      target ∉ s.banned && s.jailRemaining target > 0 then
    { s with ballots := insert (target, voter) s.ballots }
  else s

/-- A window is opened only for a jailed target. -/
def openWindow (s : State) (target : Validator) (length : Nat) : State :=
  if s.jailRemaining target > 0 then
    { s with windows := insert target s.windows, windowLeft := setNat s.windowLeft target length }
  else s

/-! ### The phase-space theorems -/

/-- **A stalled boundary changes nothing.** Off-boundary, `tip = 0`, or `tip ≤ anchorTip` is a no-op. -/
theorem the_stall_rule_changes_nothing (params : Params) (p : Validator → Option Nat)
    (onBoundary : Bool) (tip : Nat) (s : State)
    (h : ¬ (onBoundary && advances s tip)) : step params p onBoundary tip s = s := by
  simp [step, h]

/-- A non-advancing tip freezes the counters. -/
theorem a_stall_freezes_the_jail_clock (params : Params) (p : Validator → Option Nat)
    (onBoundary : Bool) (tip : Nat) (s : State) (h : tip ≤ s.anchorTip) :
    step params p onBoundary tip s = s := by
  have htip : ¬ tip > s.anchorTip := by omega
  have hzero : advances s tip = false := by
    unfold advances
    have : ¬ (tip ≠ 0 ∧ tip > s.anchorTip) := fun hc => htip hc.2
    simp [this]
  simp [step, hzero]

/-- **The jail never touches the bond or the staged withdrawal.** -/
theorem jail_never_touches_the_bond (params : Params) (p : Validator → Option Nat) (tip : Nat)
    (s : State) (newly : Finset Validator) :
    (jailNew params p tip s newly).pool = s.pool ∧
      (jailNew params p tip s newly).withdrawals = s.withdrawals := by
  simp [jailNew]

/-- **A newly jailed validator leaves the active set.** -/
theorem a_jailed_validator_leaves_the_active_set (params : Params) (p : Validator → Option Nat)
    (tip : Nat) (s : State) (newly : Finset Validator) (v : Validator) (hv : v ∈ newly) :
    v ∉ (jailNew params p tip s newly).active := by
  simp only [jailNew, Finset.mem_sdiff]
  intro h
  exact h.2 hv

/-- **A released validator re-enters the active set** when it is still bonded. -/
theorem a_released_validator_reenters (s : State) (v : Validator)
    (hbonded : (s.pool v).isSome) : v ∈ (releaseTarget s v).active := by
  simp [releaseTarget, hbonded]

/-- **Release clears the jail record, the window and the ballots.** -/
theorem release_clears_the_window_and_ballots (s : State) (v : Validator) :
    v ∉ (releaseTarget s v).jailedSet ∧ v ∉ (releaseTarget s v).windows ∧
      (releaseTarget s v).ballots = s.ballots.filter (fun b => b.1 ≠ v) := by
  simp [releaseTarget]

/-- **At expiry, presence releases.** -/
theorem a_returning_validator_is_released (params : Params) (p : Validator → Option Nat)
    (tip : Nat) (s : State) (v : Validator) (hterm : s.jailRemaining v ≤ 1)
    (hpresent : isAbsent p tip params.absenceThreshold v = false) :
    expireTarget params p tip s v = releaseTarget s v := by
  simp [expireTarget, hterm, hpresent]

/-- **At expiry, silence renews.** -/
theorem a_still_absent_validator_is_renewed (params : Params) (p : Validator → Option Nat)
    (tip : Nat) (s : State) (v : Validator) (hterm : s.jailRemaining v ≤ 1)
    (habsent : isAbsent p tip params.absenceThreshold v = true) :
    (expireTarget params p tip s v).jailRemaining v = params.jailEpochs := by
  simp [expireTarget, hterm, habsent, renewTarget, setNat]

/-- **Exactly two thirds does not pass.** -/
theorem exactly_two_thirds_does_not_pass (s : State) (target : Validator)
    (h : 3 * forStake s target = 2 * totalStake s) : passes s target = false := by
  unfold passes
  have hnot : ¬ 3 * forStake s target > 2 * totalStake s := by omega
  simp [hnot]

/-- **A strict supermajority passes.** -/
theorem a_strict_supermajority_passes (s : State) (target : Validator)
    (h : 3 * forStake s target > 2 * totalStake s) : passes s target = true := by
  simp [passes, h]

/-- **A passing vote slashes, ejects, and clears.** -/
theorem a_passing_vote_slashes_and_ejects (s : State) (target : Validator)
    (_h : target ∈ passing s) :
    (slashTarget s target).pool target = none ∧ target ∈ (slashTarget s target).banned ∧
      target ∉ (slashTarget s target).active ∧ target ∉ (slashTarget s target).windows ∧
      (slashTarget s target).withdrawals target = 0 := by
  simp [slashTarget, setNat]

/-- **A vote against an active validator is not recorded**: no open window, no ballot. -/
theorem a_vote_without_a_window_is_not_recorded (s : State) (voter target : Validator)
    (inFavor : Bool) (h : target ∉ s.windows) : castVote s voter target inFavor = s := by
  simp [castVote, h]

/-- **A vote is not opened without a jail.** -/
theorem a_vote_without_jail_is_not_opened (s : State) (target : Validator) (length : Nat)
    (h : s.jailRemaining target = 0) : openWindow s target length = s := by
  simp [openWindow, h]

/-- **A boundary at the jail threshold jails** a validator that reaches it. -/
theorem a_boundary_at_the_threshold_jails (params : Params) (p : Validator → Option Nat)
    (tip : Nat) (s : State) (v : Validator) (hactive : v ∈ s.active)
    (hnot : s.jailRemaining v = 0)
    (hthreshold : countedAbsent params p tip s v ≥ params.jailAfter) :
    v ∈ newlyJailed params p tip s := by
  simp [newlyJailed, hactive, hnot, hthreshold]

/-- The one-lapse fixture: one bonded active validator with no prior absence. -/
def lapseState : State :=
  { pool := fun v => if v = 1 then some 10 else none, active := {1}, banned := ∅,
    absent := fun _ => 0, jailRemaining := fun _ => 0, jailedSet := ∅, windows := ∅,
    windowLeft := fun _ => 0, ballots := ∅, withdrawals := fun _ => 0, anchorTip := 0 }

def lapseParams : Params := ⟨2, 2, 5, 6, true⟩

/-- **One lapse is not sustained** with `jailAfter = 2`. -/
theorem one_lapse_is_not_sustained :
    (1 : Nat) ∉ newlyJailed lapseParams (fun v => if v = 1 then some 0 else none) 10 lapseState := by
  simp [newlyJailed, countedAbsent, isAbsent, lapseParams, lapseState]

/-- **A banished validator cannot rebond.** -/
def rebond (s : State) (v : Validator) (stake : Nat) : State :=
  if v ∈ s.banned then s else { s with pool := fun w => if w = v then some stake else s.pool w }

theorem a_banished_validator_cannot_rebond (s : State) (v : Validator) (stake : Nat)
    (h : v ∈ s.banned) : rebond s v stake = s := by
  simp [rebond, h]

/-- The unguarded shape is what the rule forbids. -/
def unguardedRebond (s : State) (v : Validator) (stake : Nat) : State :=
  { s with pool := fun w => if w = v then some stake else s.pool w }

theorem an_unguarded_rebond_reaches_a_banished_validator :
    let s : State :=
      { pool := fun _ => none, active := ∅, banned := {1}, absent := fun _ => 0,
        jailRemaining := fun _ => 0, jailedSet := ∅, windows := ∅, windowLeft := fun _ => 0,
        ballots := ∅, withdrawals := fun _ => 0, anchorTip := 0 }
    (unguardedRebond s 1 10).pool 1 = some 10 := by
  simp [unguardedRebond]

/-- **The mutant proposition**: the authors-only presence leaves a deploy-presented jail in place. -/
def law89_mutant : Prop :=
  ∃ (s : State) (deployers : Finset (Validator × Nat)) (v h : Nat),
    s.jailRemaining v = 1 ∧ (v, h) ∈ deployers ∧
      present (presentFromSenders ∅) v = false ∧
      present (presentFromAuthorsAndDeployers ∅ deployers) v = true

/-- **The falsifier, fired.** A jailed validator whose key signed a deploy: the retired model leaves
    it absent, the corrected model presents it. -/
theorem law89_falsifier : law89_mutant := by
  let s : State :=
    { pool := fun v => if v = 1 then some 10 else none, active := ∅, banned := ∅,
      absent := fun _ => 0, jailRemaining := fun v => if v = 1 then 1 else 0, jailedSet := {1},
      windows := ∅, windowLeft := fun _ => 0, ballots := ∅, withdrawals := fun _ => 0,
      anchorTip := 0 }
  refine ⟨s, {(1, 0)}, 1, 0, by simp [s], by simp, ?_, ?_⟩
  · simp [presentFromSenders, participationMap, heights, present]
  · exact a_deploy_signer_can_present_a_jailed_validator s ∅ {(1, 0)} 1 0 (by simp [s]) (by simp)

/-- **Law 89's statement.** Presence is the max recorded height; absence is missing or strictly
    behind the tip; the tip is the greatest strict-supermajority height; the vote gate is strict;
    the stall is a no-op; the jail leaves the bond and withdrawal untouched and removes the target
    from `active`; a passing vote slashes and ejects; and release/renewal follow the derived
    presence. -/
def law89_statement : Prop :=
  (∀ (entries : Finset (Validator × Nat)) (v h : Nat), (v, h) ∈ entries →
    present (participationMap entries) v = true) ∧
  (∀ (p : Validator → Option Nat) (tip threshold : Nat) (v : Validator),
    isAbsent p tip threshold v = true ↔
      p v = none ∨ ∃ h, p v = some h ∧ tip - h > threshold) ∧
  (∀ (p : Validator → Option Nat) (tip threshold : Nat) (v h : Nat),
    p v = some h → tip - h = threshold → isAbsent p tip threshold v = false) ∧
  (∀ (s : State) (target : Validator), 3 * forStake s target = 2 * totalStake s →
    passes s target = false) ∧
  (∀ (s : State) (target : Validator), 3 * forStake s target > 2 * totalStake s →
    passes s target = true) ∧
  (∀ (params : Params) (p : Validator → Option Nat) (onBoundary : Bool) (tip : Nat) (s : State),
    ¬ (onBoundary && advances s tip) → step params p onBoundary tip s = s) ∧
  (∀ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
      (newly : Finset Validator),
    (jailNew params p tip s newly).pool = s.pool ∧
      (jailNew params p tip s newly).withdrawals = s.withdrawals) ∧
  (∀ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
      (newly : Finset Validator) (v : Validator), v ∈ newly →
    v ∉ (jailNew params p tip s newly).active) ∧
  (∀ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State) (v : Validator),
    s.jailRemaining v ≤ 1 → isAbsent p tip params.absenceThreshold v = false →
      expireTarget params p tip s v = releaseTarget s v) ∧
  (∀ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State) (v : Validator),
    s.jailRemaining v ≤ 1 → isAbsent p tip params.absenceThreshold v = true →
      (expireTarget params p tip s v).jailRemaining v = params.jailEpochs) ∧
  (∀ (s : State) (target : Validator), target ∈ passing s →
    (slashTarget s target).pool target = none ∧ target ∈ (slashTarget s target).banned ∧
      target ∉ (slashTarget s target).active ∧ target ∉ (slashTarget s target).windows ∧
      (slashTarget s target).withdrawals target = 0) ∧
  law89_mutant

theorem law89 : law89_statement := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, law89_falsifier⟩
  · intro entries v h hmem
    exact present_of_mem entries v h hmem
  · intro p tip threshold v
    exact absence_is_missing_or_behind_the_threshold p tip threshold v
  · intro p tip threshold v h hp heq
    exact exactly_the_threshold_is_present p tip threshold v h hp heq
  · intro s target h
    exact exactly_two_thirds_does_not_pass s target h
  · intro s target h
    exact a_strict_supermajority_passes s target h
  · intro params p onBoundary tip s h
    exact the_stall_rule_changes_nothing params p onBoundary tip s h
  · intro params p tip s newly
    exact jail_never_touches_the_bond params p tip s newly
  · intro params p tip s newly v hv
    exact a_jailed_validator_leaves_the_active_set params p tip s newly v hv
  · intro params p tip s v hterm hpresent
    exact a_returning_validator_is_released params p tip s v hterm hpresent
  · intro params p tip s v hterm habsent
    exact a_still_absent_validator_is_renewed params p tip s v hterm habsent
  · intro s target h
    exact a_passing_vote_slashes_and_ejects s target h

end Liveness
end Rchain
