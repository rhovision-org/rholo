import Mathlib.Data.Finset.Card
import Mathlib.Tactic

/-!
# Law 107 — history availability

**The change this law covers.** Below a checkpoint, history is a **service**: an archive keeps
bodies, advertises which heights it holds (`/api/v1/capabilities`, `/api/v1/archives`), and answers
`/api/v1/history/challenge` with a deterministic sample (`node/src/web/archives.rs`,
`WebApiImpl::history_challenge`).

**What this law says.** A challenge answer proves the archive held the sampled body at challenge
time; a gap is detected when the sample covers it; a funded, retained history can reconstruct the
bodies a light node lacks; and an archive with no coverage reports `unavailable` rather than
`available`.

**What it does not claim.** The probability bound in `a_gap_is_detected_with_probability` is
**hypothesis-shaped**: the model proves the deterministic half — a sample that covers a nonempty gap
intersects it — and the sampling law (64 distinct heights detect a 10% gap with ≈99.9% probability) is
a combinatorial statement about the sampling procedure, cited in the note rather than proved here.
Possession at challenge time is not retention for ever.

**Why it is not vacuous.** Make `challenge` answer a height it does not hold and
`a_challenge_answer_proves_possession` fails; make `availability` report `available` on a nonzero floor
and `an_unavailable_archive_is_reported` fails.
-/

namespace Rchain

namespace Sync
namespace Availability

/-- Whether the archive holds the body at `height`. -/
def Holds (bodies : Nat → Bool) (height : Nat) : Prop := bodies height = true

/-- The challenge answer for one sampled height: a body, or nothing. -/
def challenge (bodies : Nat → Bool) (height : Nat) : Option Nat :=
  if bodies height = true then some height else none

/-- **Law 107a — an answer proves possession.** -/
theorem a_challenge_answer_proves_possession (bodies : Nat → Bool) (height : Nat) (answer : Nat)
    (h : challenge bodies height = some answer) : Holds bodies height := by
  unfold Holds
  by_contra hne
  simp [challenge, hne] at h

/-- **Law 107b — a covered gap is detected.** If the challenge's sampled set contains the gap set,
    the sample and the gap intersect. (The probabilistic half — how likely a sample of `k` heights is
    to cover a gap of `g` in `n` — is the sampling law, cited in the row's note.) -/
theorem a_gap_is_detected_with_probability (sample gap : Finset Nat) (hsub : gap ⊆ sample)
    (hg : gap.Nonempty) : (sample ∩ gap).Nonempty := by
  rw [Finset.inter_eq_right.mpr hsub]
  exact hg

/-- Reconstructibility: the body at the height is available. -/
def Reconstructs (bodies : Nat → Option Nat) (height : Nat) : Prop :=
  (bodies height).isSome

/-- **Law 107c — funded retention can reconstruct.** An archive that retained the body can supply
    it to a node that lacks it. Retention is the hypothesis (the funding decision), not a theorem. -/
theorem funded_retention_can_reconstruct (bodies : Nat → Option Nat) (height : Nat) (body : Nat)
    (h : bodies height = some body) : Reconstructs bodies height := by
  simp [Reconstructs, h]

/-- The advertised availability of a body floor: `0` is full coverage. -/
inductive Availability
  | available
  | unavailable

/-- **The status surface's rule**: a nonzero floor is not full coverage. -/
def availability (floor : Nat) : Availability :=
  if floor = 0 then Availability.available else Availability.unavailable

/-- **Law 107d — an unavailable archive is reported.** -/
theorem an_unavailable_archive_is_reported (floor : Nat) (h : floor ≠ 0) :
    availability floor = Availability.unavailable := by
  simp [availability, h]

end Availability
end Sync

end Rchain
