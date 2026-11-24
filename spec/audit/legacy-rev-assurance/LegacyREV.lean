import Lean
import Init.Omega

set_option autoImplicit false

/- Research model for #412. Not a production law or an authorization oracle.
   Natural-number atomic units; trusted immutable entitlement registry and
   approval predicate. A successful settlement is one abstract atomic step. -/
namespace LegacyREV

def liquid (a t : Nat) : Nat := min a t
def locked (a t : Nat) : Nat := a - t

theorem allocation_conserves (a t : Nat) :
    liquid a t + locked a t = a := by
  by_cases h : a <= t
  · simp [liquid, locked, Nat.min_eq_left h, Nat.sub_eq_zero_of_le h]
  · have ht : t <= a := by omega
    simp [liquid, locked, Nat.min_eq_right ht]
    omega

theorem liquid_bounded (a t : Nat) : liquid a t <= t := by
  unfold liquid
  omega

def allocated (a t : Nat) : Nat := liquid a t + locked a t

def nsum : List Nat → Nat
  | [] => 0
  | a :: rest => a + nsum rest

theorem portfolio_conserves (xs : List Nat) (t : Nat) :
    nsum (xs.map (fun a => allocated a t)) = nsum xs := by
  induction xs with
  | nil => rfl
  | cons a rest ih =>
    simp [nsum, allocated, allocation_conserves, ih]

-- Same piecewise arithmetic as Law 79; a restricted schedule requires d > 0.
def vested (a d h : Nat) : Nat :=
  if d = 0 then a else if d <= h then a else a * h / d

theorem vesting_bounded (a d h : Nat) : vested a d h <= a := by
  unfold vested
  by_cases hd : d = 0
  · simp [hd]
  · rw [if_neg hd]
    by_cases hh : d <= h
    · simp [hh]
    · rw [if_neg hh]
      have hm : a * h <= d * a := by
        rw [Nat.mul_comm d a]
        exact Nat.mul_le_mul_left a (by omega)
      exact Nat.div_le_of_le_mul hm

theorem vesting_monotone (a d h k : Nat) (hk : h <= k) :
    vested a d h <= vested a d k := by
  by_cases hd : d = 0
  · simp [vested, hd]
  · by_cases he : d <= k
    · have hb := vesting_bounded a d h
      simpa [vested, hd, he] using hb
    · have hh : ¬ d <= h := by omega
      simp only [vested, if_neg hd, if_neg he, if_neg hh]
      apply (Nat.le_div_iff_mul_le (show 0 < d by omega)).2
      exact Nat.le_trans (Nat.div_mul_le_self (a * h) d) (Nat.mul_le_mul_left a hk)

theorem vesting_at_start (a d : Nat) (hd : 0 < d) : vested a d 0 = 0 := by
  have h0 : d ≠ 0 := by omega
  have h1 : ¬ d <= 0 := by omega
  simp [vested, h0, h1]

theorem vesting_at_end (a d : Nat) : vested a d d = a := by
  by_cases hd : d = 0 <;> simp [vested, hd]

def release (a d h already : Nat) : Option Nat :=
  if 0 < d ∧ already <= vested a d h then some (vested a d h - already) else none

theorem release_exact {a d h already delta : Nat}
    (hs : release a d h already = some delta) :
    already + delta = vested a d h ∧ already + delta <= a := by
  by_cases hg : 0 < d ∧ already <= vested a d h
  · simp [release, hg] at hs
    have hb := vesting_bounded a d h
    omega
  · simp [release, hg] at hs

theorem release_idempotent (a d h : Nat) (hd : 0 < d) :
    release a d h (vested a d h) = some 0 := by
  simp [release, hd]

theorem release_rejects_invalid (a d h already : Nat)
    (bad : d = 0 ∨ vested a d h < already) :
    release a d h already = none := by
  have hn : ¬ (0 < d ∧ already <= vested a d h) := by omega
  simp [release, hn]

structure Claim where
  manifest : Nat
  source : Nat
  owner : Nat
  amount : Nat
  deriving DecidableEq, Repr

structure State where
  manifest : Nat
  remaining : Nat
  paid : Nat
  consumed : List Nat
  deriving DecidableEq, Repr

def initial (m reserve : Nat) : State := ⟨m, reserve, 0, []⟩
def total (s : State) : Nat := s.remaining + s.paid

abbrev allowed (pending : List Claim) (approved : Claim → Bool)
    (s : State) (c : Claim) : Prop :=
  approved c = true ∧ c ∈ pending ∧ c.manifest = s.manifest ∧
    c.source ∉ s.consumed ∧ c.amount <= s.remaining

def next (s : State) (c : Claim) : State :=
  ⟨s.manifest, s.remaining - c.amount, s.paid + c.amount, c.source :: s.consumed⟩

def settle (pending : List Claim) (approved : Claim → Bool)
    (s : State) (c : Claim) : Option State :=
  if allowed pending approved s c then some (next s c) else none

theorem settle_guard {p : List Claim} {auth : Claim → Bool} {s s' : State} {c : Claim}
    (h : settle p auth s c = some s') : allowed p auth s c ∧ s' = next s c := by
  by_cases hg : allowed p auth s c
  · simp only [settle, if_pos hg, Option.some.injEq] at h
    exact ⟨hg, h.symm⟩
  · simp [settle, hg] at h

theorem settle_conserves {p : List Claim} {auth : Claim → Bool} {s s' : State} {c : Claim}
    (h : settle p auth s c = some s') : total s' = total s := by
  have hg := (settle_guard h).1
  have he := (settle_guard h).2
  have ha := hg.2.2.2.2
  rw [he]
  simp [total, next]
  omega

theorem settlement_requires_approval {p : List Claim} {auth : Claim → Bool}
    {s s' : State} {c : Claim} (h : settle p auth s c = some s') :
    auth c = true ∧ c ∈ p ∧ c.manifest = s.manifest := by
  have hg := (settle_guard h).1
  exact ⟨hg.1, hg.2.1, hg.2.2.1⟩

theorem settlement_marks_consumed {p : List Claim} {auth : Claim → Bool}
    {s s' : State} {c : Claim} (h : settle p auth s c = some s') :
    c.source ∈ s'.consumed := by
  rw [(settle_guard h).2]
  simp [next]

theorem settlement_retains_consumed {p : List Claim} {auth : Claim → Bool}
    {s s' : State} {c : Claim} {id : Nat}
    (h : settle p auth s c = some s') (hm : id ∈ s.consumed) :
    id ∈ s'.consumed := by
  rw [(settle_guard h).2]
  simp [next, hm]

theorem consumed_source_rejected (p : List Claim) (auth : Claim → Bool)
    (s : State) (c : Claim) (hm : c.source ∈ s.consumed) :
    settle p auth s c = none := by
  simp [settle, allowed, hm]

theorem same_source_replay_rejected {p : List Claim} {auth : Claim → Bool}
    {s s' : State} {c other : Claim}
    (h : settle p auth s c = some s') (hi : other.source = c.source) :
    settle p auth s' other = none := by
  apply consumed_source_rejected
  rw [hi]
  exact settlement_marks_consumed h

theorem unauthorized_rejected (p : List Claim) (auth : Claim → Bool)
    (s : State) (c : Claim) (ha : auth c = false) :
    settle p auth s c = none := by
  simp [settle, allowed, ha]

theorem reserve_exhaustion_rejected (p : List Claim) (auth : Claim → Bool)
    (s : State) (c : Claim) (ha : s.remaining < c.amount) :
    settle p auth s c = none := by
  have hn : ¬ c.amount <= s.remaining := by omega
  simp [settle, allowed, hn]

inductive Reachable (p : List Claim) (auth : Claim → Bool) (m reserve : Nat) : State → Prop
  | start : Reachable p auth m reserve (initial m reserve)
  | step {s s' : State} {c : Claim} :
      Reachable p auth m reserve s → settle p auth s c = some s' →
      Reachable p auth m reserve s'

theorem trace_conserves {p : List Claim} {auth : Claim → Bool} {m reserve : Nat}
    {s : State} (h : Reachable p auth m reserve s) : total s = reserve := by
  induction h with
  | start => simp [total, initial]
  | step _ hs ih => exact (settle_conserves hs).trans ih

-- A policy over addresses is not a policy over beneficial owners.
theorem split_identity_counterexample :
    liquid 100 100 + liquid 100 100 > liquid 200 100 := by decide

def c1 : Claim := ⟨1, 4, 3, 50⟩
def c2 : Claim := ⟨1, 5, 4, 20⟩
def registry : List Claim := [c1, c2]
def approveBoth (c : Claim) : Bool := decide (c = c1 ∨ c = c2)

example : settle registry approveBoth (initial 1 60) c1 =
    some ⟨1, 10, 50, [4]⟩ := by decide
example : settle registry approveBoth ⟨1, 10, 50, [4]⟩ c1 = none := by decide
example : settle registry approveBoth ⟨1, 10, 50, [4]⟩ c2 = none := by decide
example : settle registry approveBoth (initial 1 60) ⟨2, 4, 3, 50⟩ = none := by decide
example : settle registry (fun _ => false) (initial 1 60) c1 = none := by decide

#print axioms allocation_conserves
#print axioms portfolio_conserves
#print axioms vesting_bounded
#print axioms vesting_monotone
#print axioms release_exact
#print axioms trace_conserves
#print axioms same_source_replay_rejected
#print axioms settlement_requires_approval

end LegacyREV

open LegacyREV in
def main : IO Unit := do
  for a in [:33] do
    for t in [:33] do
      IO.println s!"ALLOC {a} {t} {liquid a t} {locked a t}"
    for d in [1:17] do
      for h in [:(d+3)] do
        IO.println s!"VEST {a} {d} {h} {vested a d h}"
  IO.println "FORMAL_WITNESS_END"
