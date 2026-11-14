import Mathlib.Logic.Function.Basic

/-!
# Law 106 — audit equivalence

**The change this law covers.** `rnode audit` (`node/src/runtime/audit.rs`) downloads the bodies,
replays them from genesis through the validator, and compares the state it derives with the state the
checkpoint claimed. It writes `AGREE`, `DISAGREE` or `INCOMPLETE`.

**What this law says.** The three verdicts partition the outcomes and mean what they say: agreement
implies the derived root equals the claimed one; a difference is reported as a disagreement; a history
the audit could not obtain is **not** agreement. The last clause is the one that matters — missing
evidence must not be read as a pass.

**What it does not claim.** It does not claim the replay itself is faithful (that is Law 102 and the
Rust's own rules), nor that agreement is proof the state is honest (a quorum that signed a wrong root
is Law 85's residual). It says only that the audit's *verdict* is sound with respect to the roots it
compares.

**Why it is not vacuous.** Make `judge` return `agree` on a covered difference and
`a_replay_that_agrees_derives_the_checkpoint_root` fails; return `agree` on an uncovered history and
`an_incomplete_history_is_not_agreement` fails.
-/

namespace Rchain

namespace Sync
namespace Audit

/-- The three verdicts. -/
inductive Verdict
  | agree
  | disagree
  | incomplete

/-- **The audit's judgement**: incomplete first (missing evidence is never agreement), then the root
    comparison. -/
def judge (derived claimed : Nat) (covered : Bool) : Verdict :=
  if covered = true then
    if derived = claimed then Verdict.agree else Verdict.disagree
  else
    Verdict.incomplete

/-- **Law 106a — a replay that agrees derives the checkpoint root.** -/
theorem a_replay_that_agrees_derives_the_checkpoint_root (derived claimed : Nat)
    (h : judge derived claimed true = Verdict.agree) : derived = claimed := by
  by_contra hne
  simp [judge, hne] at h

/-- **Law 106b — a divergence is reported.** -/
theorem a_divergence_is_reported (derived claimed : Nat) (h : derived ≠ claimed) :
    judge derived claimed true = Verdict.disagree := by
  simp [judge, h]

/-- **Law 106c — an incomplete history is not agreement.** -/
theorem an_incomplete_history_is_not_agreement (derived claimed : Nat) :
    judge derived claimed false ≠ Verdict.agree := by
  simp [judge]

/-- Non-vacuity of 106c: the same call reaches `incomplete`, so the inequality is about a value the
    function produces rather than an unreachable one. -/
theorem an_incomplete_history_is_incomplete (derived claimed : Nat) :
    judge derived claimed false = Verdict.incomplete := by
  simp [judge]

end Audit
end Sync

end Rchain
