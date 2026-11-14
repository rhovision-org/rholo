import Mathlib.Data.Finset.Card
import Mathlib.Tactic

/-!
# Law 104 — checkpoint quorum soundness

**The change this law covers.** Sync accepts a finalized state on a **quorum of signed checkpoint
attestations** (`casper/src/checkpoint.rs::verify_checkpoint_certificate`) instead of replaying the
chain. The statement is a `Checkpoint`; validators sign its digest with a domain tag of its own; a
certificate is quorate under the finality rule's exact integer form, `3 * supporting > 2 * total`.

**What this law says.** Two quorums intersect, so two conflicting checkpoints at one parent require a
signer that signed both — an equivocator, which is what makes the certificate attributable. And the
digest binds the anchor and the parent, so a signature over one checkpoint is not a signature over
another.

**What it does not claim.** It does not claim a quorum is honest, or that the root it signs is
correct: a quorum that all computed the same wrong root still certifies it. That residual is Law 85's,
and the audit (Law 106) is its counterweight. This model assumes equal weight per signer over a fixed
`Fin n` set; the Rust weighs stake, and `two_quorums_intersect` is the equal-weight projection of the
same strict-majority arithmetic.

**Why it is not vacuous.** Make the rule `>=` and `two_quorums_intersect` fails on two disjoint halves
of an even set; drop the digest's anchor projection and `the_digest_binds_the_anchor_and_parent` fails.
-/

namespace Rchain

namespace Sync
namespace Checkpoint

variable {n : Nat}

/-- A signer set is quorate when it holds strictly more than two thirds of the signers. -/
def Quorate (s : Finset (Fin n)) : Prop := 3 * s.card > 2 * n

/-- **Law 104a — two quorums intersect.** -/
theorem two_quorums_intersect (a b : Finset (Fin n)) (ha : Quorate a) (hb : Quorate b) :
    (a ∩ b).Nonempty := by
  unfold Quorate at ha hb
  have hcard : a.card + b.card = (a ∪ b).card + (a ∩ b).card :=
    (Finset.card_union_add_card_inter a b).symm
  have hub : (a ∪ b).card ≤ n := by
    simpa using Finset.card_le_univ (a ∪ b)
  have : 0 < (a ∩ b).card := by omega
  exact Finset.card_pos.mp this

/-- One checkpoint statement: the anchor, the parent digest, the anchor's post-state and the fringe. -/
structure Statement where
  anchor : Nat
  parent : Nat
  post : Nat
  fringe : Nat

/-- A certificate is a statement and the signers it holds. -/
structure Certificate (n : Nat) where
  statement : Statement
  signers : Finset (Fin n)

/-- Two certificates conflict when they name the same parent and different statements. -/
def Conflicting {n : Nat} (c d : Certificate n) : Prop :=
  c.statement.parent = d.statement.parent ∧ c.statement ≠ d.statement

/-- **Law 104b — a conflicting checkpoint at one parent requires an equivocator.** The two
    certificates' signer sets intersect, so some signer signed both statements. -/
theorem a_conflicting_checkpoint_requires_an_equivocator {n : Nat} (c d : Certificate n)
    (hc : Quorate c.signers) (hd : Quorate d.signers) (_h : Conflicting c d) :
    ∃ v, v ∈ c.signers ∩ d.signers := by
  obtain ⟨v, hv⟩ := two_quorums_intersect c.signers d.signers hc hd
  exact ⟨v, hv⟩

/-- The digest is the statement (a structural model of `blake2b256` over every field). -/
def digest (s : Statement) : Statement := s

/-- **Law 104c — the digest binds the anchor and the parent.** -/
theorem the_digest_binds_the_anchor_and_parent (s t : Statement) (h : digest s = digest t) :
    s.anchor = t.anchor ∧ s.parent = t.parent := by
  unfold digest at h
  exact ⟨congrArg Statement.anchor h, congrArg Statement.parent h⟩

/-- Non-vacuity of 104c: two statements with different anchors have different digests. -/
theorem the_digest_separates_two_anchors (s t : Statement) (h : s.anchor ≠ t.anchor) :
    digest s ≠ digest t := by
  intro heq
  exact h (the_digest_binds_the_anchor_and_parent s t heq).1

end Checkpoint
end Sync

end Rchain
