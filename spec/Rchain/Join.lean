import Rchain.Liveness

/-!
# Law 120 — the joining admission: a bond does not buy a vote before the validator can vote

Law 89 covers a validator that *was* active and went silent: the jail removes it from `active` and
the vote denominator, after its window. It does not cover the validator that has just bonded and has
never been in the draw. `apply_liveness_prelude` counts absence **only for validators in the previous
active set**, so a joiner is not jailed — it simply has to be kept out of the draw until it
participates.

That is the join ladder's second rung. The timing is the existing RCHIP #55 parameters, armed
through the five `--liveness-*` flags; no new genesis field is added. At an armed epoch boundary, a
pool member that is **not in the previous active set** and is **absent** from the participation map
(no entry, or strictly more than `absenceThreshold` behind the quorum tip) is excluded from the
active draw. It enters at the first boundary after it participates.

**Why the absence conjunct, and not merely "not active".** A validator can bond and author a block
inside the same epoch. Excluding it for the boundary that follows would deny a validator that has
already met the criterion, which is the ladder's promise: the entry test is participation, not the
bond. The mutant below is exactly that rule without the conjunct, and it excludes the participant.

**Default unchanged.** With `jailAfter = 0` the set is empty; the shipped format-2 record and the
draw are byte-for-byte what they were. Pre-mainnet arming is a genesis decision.
-/

namespace Rchain
namespace Join

open Liveness

/-- **The joining admission.** Bonded (`candidates` is the pool's key set), outside the previous
    active set, and absent from the participation map at the quorum tip. Empty when unarmed. -/
def joining (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
    (candidates : Finset Validator) : Finset Validator :=
  if params.jailAfter = 0 then
    ∅
  else
    candidates.filter fun v =>
      decide (v ∉ s.active) && isAbsent p tip params.absenceThreshold v

/-- The rule without the participation test: not active is enough. The mutant. -/
def joiningUnfiltered (params : Params) (_p : Validator → Option Nat) (_tip : Nat) (s : State)
    (candidates : Finset Validator) : Finset Validator :=
  if params.jailAfter = 0 then ∅ else candidates.filter fun v => decide (v ∉ s.active)

/-- Membership in the joining set, as the three conjuncts and the armed guard. -/
theorem joining_mem (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
    (candidates : Finset Validator) (v : Validator) :
    v ∈ joining params p tip s candidates ↔
      params.jailAfter ≠ 0 ∧ v ∈ candidates ∧ v ∉ s.active ∧
        isAbsent p tip params.absenceThreshold v = true := by
  by_cases hz : params.jailAfter = 0
  · simp [joining, hz]
  · simp only [joining, if_neg hz, Finset.mem_filter, Bool.and_eq_true,
      decide_eq_true_eq, not_not]
    constructor
    · rintro ⟨hmem, hna, hab⟩
      exact ⟨hz, hmem, hna, hab⟩
    · rintro ⟨_, hmem, hna, hab⟩
      exact ⟨hmem, hna, hab⟩

/-! ### The statement -/

/-- **Law 120's statement.** A joining validator is outside the previous active set and absent at
    the quorum tip; any draw that excludes the joining set contains none of them; an unarmed network
    (the shipped record) excludes nobody; and a validator that has participated is not joining. -/
def law120_statement : Prop :=
  (∀ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
      (candidates : Finset Validator) (v : Validator),
    v ∈ joining params p tip s candidates ↔
      params.jailAfter ≠ 0 ∧ v ∈ candidates ∧ v ∉ s.active ∧
        isAbsent p tip params.absenceThreshold v = true) ∧
  (∀ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
      (candidates drawn : Finset Validator) (v : Validator),
    drawn ⊆ candidates \ joining params p tip s candidates →
      v ∈ joining params p tip s candidates → v ∉ drawn) ∧
  (∀ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
      (candidates : Finset Validator),
    params.jailAfter = 0 → joining params p tip s candidates = ∅) ∧
  (∀ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
      (candidates : Finset Validator) (v : Validator),
    isAbsent p tip params.absenceThreshold v = false →
      v ∉ joining params p tip s candidates)

theorem law120 : law120_statement := by
  refine ⟨joining_mem, ?_, ?_, ?_⟩
  · intro params p tip s candidates drawn v hdrawn hvjoining hvdrawn
    have hmem := hdrawn hvdrawn
    simp only [Finset.mem_sdiff] at hmem
    exact hmem.2 hvjoining
  · intro params p tip s candidates hz
    exact if_pos hz
  · intro params p tip s candidates v hpresent hjoining
    have habsent := ((joining_mem params p tip s candidates v).mp hjoining).2.2.2
    rw [hpresent] at habsent
    exact Bool.false_ne_true habsent

/-! ### The falsifier

The mutant uses "not active" alone. A validator that bonded and has already authored a block is
outside the previous active set and *present* — the mutant excludes it from the next boundary, which
is the wrong rule. -/

/-- The mutant's witness: a present, non-active, bonded validator is in the unfiltered set. -/
def law120_mutant : Prop :=
  ∃ (params : Params) (p : Validator → Option Nat) (tip : Nat) (s : State)
      (candidates : Finset Validator) (v : Validator),
    params.jailAfter ≠ 0 ∧ v ∈ candidates ∧ v ∉ s.active ∧
      isAbsent p tip params.absenceThreshold v = false ∧
      v ∈ joiningUnfiltered params p tip s candidates

theorem law120_falsifier : law120_mutant := by
  let params : Params :=
    { absenceThreshold := 0, jailAfter := 1, jailEpochs := 1, voteWindow := 0, slash := false }
  let s : State :=
    { pool := fun _ => some 10, active := ∅, banned := ∅, absent := fun _ => 0,
      jailRemaining := fun _ => 0, jailedSet := ∅, windows := ∅, windowLeft := fun _ => 0,
      ballots := ∅, withdrawals := fun _ => 0, anchorTip := 0 }
  refine ⟨params, fun _ => some 0, 0, s, {0}, 0, ?_, ?_, ?_, ?_, ?_⟩
  · simp [params]
  · simp
  · simp [s]
  · simp [params, isAbsent]
  · simp [joiningUnfiltered, params, s]

end Join
end Rchain
