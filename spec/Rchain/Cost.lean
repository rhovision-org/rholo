/-!
# Law 91 — the *magnitude* of a charge, and the two shapes that stopped tracking it

`Rchain/Charging.lean` models the **sequence** of charges — where a refund lands relative to the
charge it lowers, and why the peak and not the sum is what placement changes. This module models
their **magnitude**, for the two operations the October 2026 re-audit measured buying unbounded
work for a flat or sublinear price (AUDIT C345, C346).

The properties are small on purpose, and each is the one its defect violated:

* `canonicalizationCost_covers_the_operand` — a canonicalisation charges for the elements it sorts;
  the defect was a flat charge, so a phlo bought an unbounded number of them.
* `unionCost_symmetric` — a union charges both operands; the defect charged the **right** one only,
  so `bigSet ++ {}` was free while it still rebuilt `bigSet`. `rightOnlyUnionCost` is the old
  formula, kept as the falsifier that makes the symmetry statement non-vacuous.
* `censureCost_exceeds_the_universe` and `censureCost_monotone` — the governance fold's charge is
  cubic in a universe it is proved to cover; the defect charged a square for a Θ(U³) fixed point.

**What this does not model.** The Rust computes these charges from *counts* before doing the work;
the correspondence between `canonicalizationCost n` and `Costs::canonicalization_cost n` is the
prose tie, and the exact phlo the node charges is pinned by Rust witnesses
(`rholang/src/reduce.rs`, `rholang/src/system_processes.rs`) rather than by a conformance corpus —
the functions are closed forms, so a corpus of the same numbers would restate the witness. The sort
is `O(n log n)` and the charge is linear; a phlo therefore buys work bounded by `charge · log
charge`, which is bounded and is not the defect, but is not an exact comparison count either.
-/

namespace Rchain

/-- The phlo a canonicalisation of `n` elements charges: three per element, the same constant the
cost table uses for a lookup, an addition or a removal. -/
def canonicalizationCost (n : Nat) : Nat := 3 * n

/-- The phlo a set or map union charges: both operands are canonicalised into one result. -/
def unionCost (left right : Nat) : Nat := canonicalizationCost (left + right)

/-- The phlo `rho:gov:censure` charges over a member universe of `u`: the fixed point runs at most
`u + 1` rounds and each round scans the universe per member. -/
def censureCost (u : Nat) : Nat := (u + 1) * (u + 1) * (u + 1)

/-- The charge is the table's constant three per element, at every size. -/
theorem canonicalizationCost_eq (n : Nat) : canonicalizationCost n = 3 * n := rfl

/-- A canonicalisation charges for the elements it canonicalises — the statement the flat charge
violated: with a constant charge this would fail for the first `n` above it. -/
theorem canonicalizationCost_covers_the_operand (n : Nat) : n ≤ canonicalizationCost n := by
  unfold canonicalizationCost
  omega

/-- A union charges exactly the two canonicalisations, added — not one of them. -/
theorem unionCost_is_the_sum_of_its_operands (left right : Nat) :
    unionCost left right = canonicalizationCost left + canonicalizationCost right := by
  unfold unionCost canonicalizationCost
  omega

/-- **The property the old charge lacked**: a union's charge does not depend on which operand is
which, so `bigSet ++ {}` and `{} ++ bigSet` cost the same. -/
theorem unionCost_symmetric (left right : Nat) : unionCost left right = unionCost right left := by
  unfold unionCost canonicalizationCost
  exact congrArg (fun total => 3 * total) (Nat.add_comm left right)

/-- The old charge, kept so the symmetry statement above has a model that fails it. -/
def rightOnlyUnionCost (_left right : Nat) : Nat := 3 * right

/-- The old formula is **not** symmetric, and the pair is exhibited rather than asserted: a
right-only charge is the same statement as "the left operand is free". -/
theorem rightOnlyUnionCost_is_not_symmetric :
    ∃ left right, rightOnlyUnionCost left right ≠ rightOnlyUnionCost right left :=
  ⟨1, 2, by decide⟩

/-- The charge covers the universe it folds over: every member is paid for at least once. -/
theorem censureCost_exceeds_the_universe (u : Nat) : u + 1 ≤ censureCost u := by
  unfold censureCost
  have h : 1 ≤ u + 1 := Nat.succ_le_succ (Nat.zero_le u)
  have step : (u + 1) * 1 * 1 ≤ (u + 1) * (u + 1) * (u + 1) :=
    Nat.mul_le_mul (Nat.mul_le_mul (Nat.le_refl (u + 1)) h) h
  simpa using step

/-- A larger universe never costs less. -/
theorem censureCost_monotone {a b : Nat} (h : a ≤ b) : censureCost a ≤ censureCost b := by
  unfold censureCost
  exact Nat.mul_le_mul (Nat.mul_le_mul (Nat.succ_le_succ h) (Nat.succ_le_succ h))
    (Nat.succ_le_succ h)

/-- The charge is **not** the square the defect charged: at a universe of two the two differ. -/
theorem censureCost_is_not_the_square : ∃ u, censureCost u ≠ u * u :=
  ⟨2, by decide⟩

/-- The phlo a tally charges (C354). A **ranked** tally scans every ranked entry once per round and a
round may eliminate only one option, so there are at most `longest` rounds: the worst case is
`longest * entries`. An **approval** tally counts every approved entry once. The floor is the approval
side, and it also keeps a one-round ranked ballot from costing nothing. -/
def tallyCost (longest entries : Nat) : Nat := max (longest * entries) entries

/-- A tally charges every entry it scans, whatever the ballot shape. -/
theorem tallyCost_covers_the_entries (longest entries : Nat) : entries ≤ tallyCost longest entries :=
  Nat.le_max_right _ _

/-- And a ranked tally charges the **worst case** of its rounds, not one pass. -/
theorem tallyCost_is_the_worst_case (longest entries : Nat) :
    longest * entries ≤ tallyCost longest entries :=
  Nat.le_max_left _ _

end Rchain
