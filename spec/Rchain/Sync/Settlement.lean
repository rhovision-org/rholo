import Mathlib.Tactic

/-!
# Law 111 — settlement is determined by the possession-challenge outcome

**The change this law covers.** An archive's paid period ends with the caller's possession challenge
(`GET /api/v1/history/challenge`, Law 107). The meter's `settle_period`
(`block-storage/src/history_meter.rs`) applies the outcome: a pass carries unused credit and leaves
the archive selected; a failure forfeits the period's remaining credit and marks the peer, excluding
it from selection until the operator's cooldown ends.

**What this law says.** Settlement is a function of the outcome. A failure forfeits the period and
marks; a pass refunds/carries and does not mark. The two outcomes are observably different, so a
settlement cannot be confused with the other one.

**What it does not claim.** The direct REV payment is not reversed on-chain, and a failed challenge
is **not** a consensus slash: the consequence is fee forfeiture, credit write-off and selection.
Possession is proved at challenge time, not retention for ever (Law 107's boundary).

**Why it is not vacuous.** Make `settle` constant and `the_two_outcomes_differ` fails; swap the
branches and both the forfeit and the carry theorems fail. The Rust witness is the meter's settled
outcome tests.
-/

namespace Rchain

namespace Sync
namespace Settlement

/-- The possession challenge's two outcomes. -/
inductive Outcome where
  | pass
  | fail
  deriving DecidableEq

/-- What a period's settlement does: bytes refunded/carried, bytes forfeited, and whether the peer is
    marked. -/
structure Settlement where
  refund : Nat
  forfeit : Nat
  marked : Bool
  deriving DecidableEq

/-- **Settlement is a function of the outcome.** -/
def settle (fee : Nat) : Outcome → Settlement
  | .pass => ⟨fee, 0, false⟩
  | .fail => ⟨0, fee, true⟩

/-- **Law 111a — a failed challenge forfeits the period and marks the peer.** -/
theorem a_failed_challenge_forfeits (fee : Nat) :
    (settle fee .fail).forfeit = fee ∧ (settle fee .fail).marked = true :=
  ⟨rfl, rfl⟩

/-- **Law 111b — a passed challenge carries the unused fee and does not mark.** -/
theorem a_passed_challenge_refunds (fee : Nat) :
    (settle fee .pass).refund = fee ∧ (settle fee .pass).marked = false :=
  ⟨rfl, rfl⟩

/-- **Law 111c — the outcome determines the settlement**: equal outcomes settle equally. -/
theorem settlement_is_determined (fee : Nat) (o₁ o₂ : Outcome) (h : o₁ = o₂) :
    settle fee o₁ = settle fee o₂ := by
  rw [h]

/-- **Non-vacuity of 111c: the two outcomes are observably different.** A non-zero period has a
    settlement no other outcome shares. -/
theorem the_two_outcomes_differ (fee : Nat) (h : 0 < fee) :
    settle fee .pass ≠ settle fee .fail := by
  intro equal
  have marked_equal : false = true := by
    simpa [settle] using congrArg Settlement.marked equal
  exact Bool.false_ne_true marked_equal

end Settlement
end Sync

end Rchain
