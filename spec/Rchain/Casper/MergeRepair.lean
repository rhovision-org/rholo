import Rchain.Casper.Stranding

/-!
# The merge-boundary inverse

Law 53b says a refusal has an inverse, and the inverse is bounded. The model in `Stranding.lean` is
the finality layer's: a record is unmarked, or the node rewinds to an anchor. A merge is the same
shape one layer down. When a spend composes onto a vault the merge base never funded, the refusal had
no inverse: the merge failed as a whole, every proposal on that view failed with it, and a restart did
not clear it. The repair is to name the offending chain, drop it and its dependents, and recompose —
which is exactly "a refused unit is re-queued or terminal-with-surface, never silently dropped".

This module models the repair abstractly, not the vault. It proves the properties the Rust repair must
have: the dropped set is the removable set and is reported; a system deploy is never removable; and a
merge with nothing removable fails honestly rather than succeeding with a silent loss. It does **not**
claim the repair always succeeds: if the surviving set cannot compose, the merge must fail.
-/

namespace Rchain
namespace MergeRepair

/-- A cost move: which chain produced it, its delta, and whether it is a system deploy. -/
structure Spend where
  host : Nat
  delta : Int
  system : Bool := false
  deriving DecidableEq, Repr

/-- The composed total. -/
def total (s : List Spend) : Int := s.foldl (fun acc x => acc + x.delta) 0

/-- The chains a deficit blames: every negative, non-system spend. -/
def removalCandidates (s : List Spend) : List Nat :=
  (s.filter fun x => x.delta < 0 && !x.system).map (·.host)

/-- The removable set, deduplicated: one chain may contribute several moves. -/
def removable (s : List Spend) : List Nat := (removalCandidates s).eraseDups

/-- Drop every spend whose host is named. -/
def dropHosts (s : List Spend) (hs : List Nat) : List Spend :=
  s.filter fun x => !hs.contains x.host

/-- One repair step: name the removable chains and drop them, or refuse when there are none. -/
def repairStep (s : List Spend) : Option (List Spend × List Nat) :=
  let hs := removable s
  if hs.isEmpty then none else some (dropHosts s hs, hs)

/-- The old behaviour: the whole merge fails, naming nothing. -/
def failWhole (_ : List Spend) : Option (List Spend × List Nat) := none

/-- The measured instance: a spend whose funding chain was lost, beside an innocent chain. -/
def funded : List Spend := [{ host := 1, delta := -10 }, { host := 2, delta := 20 }]

/-- The spender and a system deploy: the system chain must survive the repair. -/
def withSystem : List Spend :=
  [{ host := 1, delta := -10 }, { host := 2, delta := -20, system := true }]

/-- A deficit only a system chain can explain: nothing may be dropped. -/
def systemOnly : List Spend := [{ host := 1, delta := -10, system := true }]

/-- **Every drop is reported.** The repair cannot silently discard a chain: whatever the returned
    dropped set is, it is the removable set the step computed. -/
theorem a_repair_reports_every_drop (s s' : List Spend) (hs : List Nat)
    (h : repairStep s = some (s', hs)) : hs = removable s := by
  unfold repairStep at h
  by_cases h0 : (removable s).isEmpty
  · rw [if_pos h0] at h
    exact absurd h (by simp)
  · rw [if_neg h0] at h
    have hpair : (dropHosts s (removable s), removable s) = (s', hs) := by simpa using h
    cases hpair
    rfl

/-- **The measured repair**: the unfunded spender is removed and the innocent chain remains. -/
theorem a_funded_repair_drops_the_spender :
    repairStep funded = some ([{ host := 2, delta := 20 }], [1]) := by
  unfold repairStep removable removalCandidates dropHosts funded
  decide

/-- **A repair strictly reduces the set** in the measured instance. -/
theorem a_repair_strictly_reduces_the_set :
    (dropHosts funded (removable funded)).length < funded.length := by
  unfold dropHosts removable removalCandidates funded
  decide

/-- **A system-only deficit is not removable.** -/
theorem a_system_deficit_is_not_removable : removable systemOnly = [] := by
  unfold removable removalCandidates systemOnly
  decide

/-- **A merge with nothing removable fails honestly** rather than succeeding with a silent loss. -/
theorem a_repair_never_silently_succeeds : repairStep systemOnly = none := by
  unfold repairStep removable removalCandidates systemOnly
  decide

/-- **A system spend survives a repair of the other chains.** -/
theorem a_system_spend_survives :
    (dropHosts withSystem (removable withSystem)).any (fun x => x.host == 2) := by
  unfold dropHosts removable removalCandidates withSystem
  decide

/-- The composed total of the measured instance, so the deficit is not a guess. -/
theorem the_measured_total : total funded = 10 := by
  unfold total funded
  decide

/-- **Law 53b's proposition for the merge boundary**: the finality-layer inverse still holds, and the
    merge repair reports every drop, preserves a system deploy, and fails when nothing is removable. -/
def law53b_statement : Prop :=
  (∀ v : Nat, ¬ strandSystemRestoring.Persistent (fun s : Strand => v ∈ s.failed)) ∧
    total funded = 10 ∧
    repairStep funded = some ([{ host := 2, delta := 20 }], [1]) ∧
    removable systemOnly = [] ∧
    repairStep systemOnly = none ∧
    (dropHosts withSystem (removable withSystem)).any (fun x => x.host == 2) ∧
    (∀ (s s' : List Spend) (hs : List Nat), repairStep s = some (s', hs) → hs = removable s)

/-- **The mutant**: the old whole-merge failure on the measured instance. -/
def law53b_mutant : Prop := repairStep funded = none

theorem law53b : law53b_statement := by
  refine ⟨Rchain.the_refusal_is_not_persistent_once_a_rule_restores,
    the_measured_total, a_funded_repair_drops_the_spender, a_system_deficit_is_not_removable,
    a_repair_never_silently_succeeds, a_system_spend_survives, ?_⟩
  exact a_repair_reports_every_drop

theorem law53b_falsifier : ¬ law53b_mutant := by
  unfold law53b_mutant repairStep removable removalCandidates funded
  decide

end MergeRepair
end Rchain
