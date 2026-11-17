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
digest binds the anchor, the parent and **the finality fringe's merged state**, so a signature over
one checkpoint is not a signature over another. The merged fringe state is the root a post-anchor
block actually builds on; without it in the digest, the certificate's run root would be a member's
post-state that no first post-fringe block uses (Law 108's defect found on 2026-11-17).

**What it does not claim.** It does not claim a quorum is honest, or that the root it signs is
correct: a quorum that all computed the same wrong root still certifies it. That residual is Law 85's,
and the audit (Law 106) is its counterweight. The model carries **both** forms: `Quorate` is the
equal-weight projection over a fixed `Fin n` set, and `WeightedQuorate` states the stake rule the Rust
applies — two weighted quorums intersect in positive weight
(`two_weighted_quorums_intersect`), which is the law's proposition.

**The shape rules are modelled, not assumed.** An explicit zero parent is a different statement from
an absent parent (`the_absence_of_a_parent_is_not_the_zero_parent`), and a negative epoch is not well
shaped (`a_negative_epoch_is_not_well_shaped`) — the two input-shape gaps the stage-one audit found.

**Why it is not vacuous.** The strict inequality is load-bearing at the boundary: `law104_falsifier`
exhibits a certificate at exactly two thirds that the `>=` rule admits and the law's `>` refuses (the
Rust's `a_certificate_at_exactly_two_thirds_is_refused` reddens for the same change). Intersection itself
survives `>=` for every `n` — two sets each at least two thirds of a total must meet — so the threshold,
not the intersection, is the falsifiable edge. Drop the digest's anchor projection and
`the_digest_binds_the_anchor_and_parent` fails.
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

/-- **A signer set is quorate by stake** when it holds strictly more than two thirds of the total
    bonded weight. This is the rule the Rust verifier applies (`3 * supporting > 2 * total` over
    `i128`), and it is the model the equal-weight `Quorate` projects. -/
def WeightedQuorate {n : Nat} (w : Fin n → Nat) (s : Finset (Fin n)) : Prop :=
  3 * s.sum w > 2 * Finset.univ.sum w

/-- **Law 104a at stake: two weighted quorums intersect in positive weight.** This is the weighted
    form of `two_quorums_intersect`, and it is the statement the node's rule actually needs: a set
    whose weight is more than two thirds of the total cannot be disjoint in weight from another such
    set, whatever the individual weights. The proof is the sum analogue of the cardinality
    pigeonhole: `a.sum + b.sum = (a ∪ b).sum + (a ∩ b).sum`, and `(a ∪ b).sum ≤ total`. -/
theorem two_weighted_quorums_intersect {n : Nat} (w : Fin n → Nat) (a b : Finset (Fin n))
    (ha : WeightedQuorate w a) (hb : WeightedQuorate w b) :
    (a ∩ b).sum w > 0 := by
  unfold WeightedQuorate at ha hb
  have hunion : (a ∪ b).sum w ≤ Finset.univ.sum w :=
    Finset.sum_le_univ_sum_of_nonneg (fun x => Nat.zero_le (w x))
  have hident : (a ∪ b).sum w + (a ∩ b).sum w = a.sum w + b.sum w :=
    Finset.sum_union_inter
  by_contra h
  rw [Nat.not_lt] at h
  have hzero : (a ∩ b).sum w = 0 := Nat.eq_zero_of_le_zero h
  rw [hzero, add_zero] at hident
  omega

/-- **A parent is either absent or present.** The Rust digest encodes absence as 32 zero bytes and
    refuses an explicit zero parent as malformed; a presence bit is the model of that sentinel, so
    `absent` and `present 0` are different statements rather than one aliased digest. -/
structure Parent where
  present : Bool
  digest : Nat
deriving DecidableEq

/-- One checkpoint statement: the anchor, the parent, the anchor's post-state, the fringe, the
    **merged state of that fringe**, and the epoch. `fringeState` is the state a block whose
    justifications are the fringe builds on — the finality fringe's merge, which is not any member's
    post-state when the fringe has more than one member (Law 85). The epoch is an `Int` on purpose:
    the Rust verifier now rejects a negative epoch, and the model states that shape rule instead of
    assuming it away with a `Nat`. -/
structure Statement where
  anchor : Nat
  parent : Parent
  post : Nat
  fringe : Nat
  fringeState : Nat
  epoch : Int
deriving DecidableEq

/-- **The statement shape the verifier accepts**: an absent or nonzero parent, and a nonnegative
    epoch. -/
def WellShaped (s : Statement) : Prop :=
  (s.parent.present = true → s.parent.digest ≠ 0) ∧ 0 ≤ s.epoch

/-- **An absent parent is not an explicit zero parent.** The presence bit makes the two distinct
    statements; the Rust refuses the explicit zero rather than hashing it. -/
theorem the_absence_of_a_parent_is_not_the_zero_parent :
    ({ present := false, digest := 0 } : Parent) ≠ { present := true, digest := 0 } := by
  simp

/-- **A negative epoch is not well shaped.** The producer derives a nonnegative epoch; a signed
    statement that carries a negative one is refused before hashing. -/
theorem a_negative_epoch_is_not_well_shaped (s : Statement) (h : s.epoch < 0) :
    ¬ WellShaped s := by
  rintro ⟨_, hnonneg⟩
  omega

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

/-- **Law 104d — the digest binds the fringe's merged state.** A signature over one statement is
    not a signature over another that names a different fringe merge, which is what makes the
    certificate's run root attributable rather than a member's assumed claim. -/
theorem the_digest_binds_the_fringe_state (s t : Statement) (h : digest s = digest t) :
    s.fringeState = t.fringeState := by
  unfold digest at h
  exact congrArg Statement.fringeState h

/-- Non-vacuity of 104c: two statements with different anchors have different digests. -/
theorem the_digest_separates_two_anchors (s t : Statement) (h : s.anchor ≠ t.anchor) :
    digest s ≠ digest t := by
  intro heq
  exact h (the_digest_binds_the_anchor_and_parent s t heq).1

/-! ## The formal proposition, and the mutation that breaks it

The register's `formal`/`mutant`/`falsifier` columns point here: `law104_statement` is the law as a
`Prop`, `law104_threshold_mutant` is the threshold with `≥` in place of `>`, and `law104_falsifier` is
the theorem that the mutant admits a set the law refuses. -/

/-- **Law 104's proposition, at stake.** Two signer sets each holding more than two thirds of the
bonded weight intersect in positive weight — the weighted rule, not the equal-weight projection. -/
def law104_statement : Prop :=
  ∀ {n : Nat} (w : Fin n → Nat) (a b : Finset (Fin n)),
    WeightedQuorate w a → WeightedQuorate w b → (a ∩ b).sum w > 0

/-- The law, as its named proposition. -/
theorem law104 : law104_statement :=
  fun w a b ha hb => two_weighted_quorums_intersect w a b ha hb

/-- The rule with `≥` in place of `>`. It is weaker, so the intersection theorem survives — the mutation
that matters is the *admission* threshold, not intersection. -/
def QuorateAtLeast {n : Nat} (s : Finset (Fin n)) : Prop := 3 * s.card ≥ 2 * n

/-- **The mutant the falsifier refutes**: some certificate is admitted by `≥` that the law's `>` refuses.
The witness is a set of exactly two thirds of a three-signer set — precisely the case the Rust's
`a_certificate_at_exactly_two_thirds_is_refused` pins. -/
def law104_threshold_mutant : Prop :=
  ∃ (n : Nat) (s : Finset (Fin n)), QuorateAtLeast s ∧ ¬ Quorate s

/-- **The falsifier.** At three signers, `{0, 1}` is exactly two thirds: `3 * 2 ≥ 2 * 3` holds, while
`3 * 2 > 2 * 3` does not. So the strict inequality is load-bearing at the boundary, and a rule that
weakened it would admit a certificate the law's statement refuses. -/
theorem law104_falsifier : law104_threshold_mutant := by
  have hcard : ({0, 1} : Finset (Fin 3)).card = 2 := by decide
  refine ⟨3, ({0, 1} : Finset (Fin 3)), ?_, ?_⟩
  · simp [QuorateAtLeast, hcard]
  · simp [Quorate, hcard]

end Checkpoint
end Sync

end Rchain
