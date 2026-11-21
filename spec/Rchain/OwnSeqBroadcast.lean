import Rchain.OwnSeq

/-!
# Law 124 — the broadcast-time claim: never announce a sequence the network already carries

Law 122 made the *signing* decision sound: a validator proceeds only above its durable own-sequence
record. The drill found the remaining edge. A joined validator signed sequence 196 from a snapshot the
rewind had left behind; the network's copy of its own earlier block at 196 arrived from gossip during
the multi-second replay, and the node broadcast the new block. The network read two blocks by one key
at one sequence — an equivocation — and slashed the bond (`e3f5195c9`, 18:38:10).

The rule is not about signing; it is about **announcing**. Between the signature and the hash
announcement there is a window in which the network's copy can arrive. `mayBroadcast` reads the
observed claims at the block's sequence immediately before the announcement and refuses when a
different hash owns it. The cell is fed at signature-verified receipt, so the copy that arrives during
replay is seen; the pre-fix path (`broadcastUnchecked`) announces unconditionally because the
signing-time check had already passed.

This is a defence in depth behind Law 122, not a replacement: the durable record prevents the
re-sign; the claim prevents the announcement; the H-1 insert gate remains the consensus backstop.
-/

namespace Rchain
namespace OwnSeqBroadcast

open Rchain.OwnSeq

/-- A block identifier in the model. -/
abbrev Hash := Nat

/-- A signed block: its sequence and hash. -/
structure Signed where
  seq : Seq
  hash : Hash
  deriving DecidableEq, Repr

/-- The observed claims: the hash the network has shown this validator's key at a sequence. The Rust
    cell is fed only by blocks that pass the signature check, so a peer cannot forge an entry. -/
def Claims := Seq → Option Hash

/-- **The broadcast decision.** Announce only when no claim exists at the block's sequence, or when
    the claim is this very block. -/
def mayBroadcast (c : Claims) (b : Signed) : Prop :=
  match c b.seq with
  | none => True
  | some h => h = b.hash

/-- The pre-fix path: the signing-time check passed, so the announcement is unconditional. -/
def broadcastUnchecked (_c : Claims) (_b : Signed) : Bool := true

/-- **The guard.** A broadcast that proceeds has no conflicting observed claim at its sequence. -/
theorem a_broadcast_matches_its_claim (c : Claims) (b : Signed) (h : mayBroadcast c b) :
    ∀ h' : Hash, c b.seq = some h' → h' = b.hash := by
  intro h' hsome
  cases hc : c b.seq with
  | none =>
      rw [hc] at hsome
      simp at hsome
  | some hgt =>
      simp [mayBroadcast, hc] at h
      rw [hc] at hsome
      injection hsome with heq
      rw [← heq]
      exact h

/-- A same-hash claim does not block the announcement. -/
theorem a_matching_claim_permits_the_broadcast (c : Claims) (b : Signed)
    (h : c b.seq = some b.hash) : mayBroadcast c b := by
  simp [mayBroadcast, h]

/-- A conflicting claim refuses it. -/
theorem a_conflicting_claim_refuses_the_broadcast (c : Claims) (b : Signed) (h' : Hash)
    (hc : c b.seq = some h') (hne : h' ≠ b.hash) : ¬ mayBroadcast c b := by
  intro h
  have hh := a_broadcast_matches_its_claim c b h h' hc
  exact hne hh

/-! ### The falsifier

The drill's timing, as a theorem. The signed block is sequence 196 with hash 222. The network's copy
of the pre-crash block at 196 has hash 111. The signing-time check has already passed (the local DAG
was rewound), and the unchecked announcement fires. -/

/-- The mutant: a conflicting claim exists and the unchecked path still announces. -/
def law124_mutant : Prop :=
  ∃ (c : Claims) (b : Signed) (h' : Hash),
    c b.seq = some h' ∧ h' ≠ b.hash ∧ broadcastUnchecked c b = true

/-- The falsifier: sequence 196, the pre-crash hash 111, the new block's hash 222. -/
theorem law124_falsifier : law124_mutant := by
  refine ⟨fun s => if s = 196 then some 111 else none, { seq := 196, hash := 222 }, 111, ?_, ?_, rfl⟩
  · simp
  · decide

/-- **Law 124's statement.** The guarded broadcast matches the claim; a conflicting claim refuses;
    and the unchecked path is the mutant. -/
def law124_statement : Prop :=
  (∀ (c : Claims) (b : Signed), mayBroadcast c b →
      ∀ h' : Hash, c b.seq = some h' → h' = b.hash) ∧
  (∀ (c : Claims) (b : Signed) (h' : Hash), c b.seq = some h' → h' ≠ b.hash →
      ¬ mayBroadcast c b) ∧
  law124_mutant

theorem law124 : law124_statement := by
  refine ⟨?_, ?_, law124_falsifier⟩
  · intro c b h h' hsome
    exact a_broadcast_matches_its_claim c b h h' hsome
  · intro c b h' hc hne
    exact a_conflicting_claim_refuses_the_broadcast c b h' hc hne

end OwnSeqBroadcast
end Rchain
