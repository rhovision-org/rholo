import Rchain.Liveness

/-!
# Law 122 — the own-sequence record: a validator never re-signs a sequence it has signed

The RC-4 HAZOP J8, measured. A light-joined validator restored its state, caught up to height 165,
and then signed block #155 at sequence 152 — the sequence it had already signed before the crash.
The network read it correctly as an equivocation and slashed the key 100%; the pool dropped from
four validators to three and every certificate collector then refused on bond-set drift.

The cause is that the round snapshot is not a memory. A rewind is allowed to drop the block that
carried a signature, so the snapshot names an older sequence, and a proposer deriving from it alone
re-signs. The fix is a durable record of the highest sequence this node has ever signed, written
before the proposal is broadcast; the proposer **waits** while the snapshot is behind it and never
signs at or below it. This module is that rule.

The mutant is the pre-fix derivation, `nextSeqUnchecked`; the falsifier is the drill's own record
(sequence 152, snapshot 151).
-/

namespace Rchain
namespace OwnSeq

/-- Sequences are numbers in the model. -/
abbrev Seq := Nat

/-- The durable record: the highest block height and sequence this validator has signed. -/
structure Record where
  height : Seq
  signed : Seq
  deriving DecidableEq, Repr

/-- What the round snapshot holds for this validator: its latest sequence, or no entry. -/
abbrev Snapshot := Option Seq

/-- The proposer's decision. `wait` carries the record and what the snapshot showed, so the wait is
    a named coordinate rather than a silence. -/
inductive Decision where
  | proceed (seq : Seq)
  | wait (signed signedHeight : Seq) (snapshot : Snapshot)
  deriving DecidableEq, Repr

/-- The pre-fix derivation: the round snapshot alone. The mutant. -/
def nextSeqUnchecked (snapshot : Snapshot) : Seq :=
  match snapshot with
  | some s => s + 1
  | none => 0

/-- **The guarded derivation.** With no record the node has never signed here and derives from the
    snapshot. With a record it proceeds only when the snapshot has caught up to it. -/
def nextSeq (record : Option Record) (snapshot : Snapshot) : Decision :=
  match record with
  | none => .proceed (nextSeqUnchecked snapshot)
  | some r =>
      match snapshot with
      | some s =>
          if r.signed ≤ s then .proceed (s + 1) else .wait r.signed r.height (some s)
      | none => .wait r.signed r.height none

/-- The record only ever rises: a new signature raises it, nothing lowers it. -/
def raise (r : Record) (height signed : Seq) : Record :=
  { height := max r.height height, signed := max r.signed signed }

/-- A rewind may drop the block that carried a signature; it cannot drop the record. -/
def afterRewind (r : Record) : Record := r

/-- **Law 122's statement.** A proceed signs strictly above the record; a record with no snapshot
    entry waits; the record is monotone under `raise`; and a rewind leaves the decision unchanged. -/
def law122_statement : Prop :=
  (∀ (r : Record) (s : Seq) (n : Seq),
      r.signed ≤ s → nextSeq (some r) (some s) = .proceed n → r.signed < n) ∧
  (∀ (r : Record), nextSeq (some r) none = .wait r.signed r.height none) ∧
  (∀ (r : Record) (h s : Seq), r.signed ≤ (raise r h s).signed) ∧
  (∀ (r : Record) (s : Seq), nextSeq (afterRewind r) (some s) = nextSeq (some r) (some s))

theorem law122 : law122_statement := by
  refine ⟨?_, ?_, ?_, ?_⟩
  · intro r s n h hn
    have hproceed : nextSeq (some r) (some s) = .proceed (s + 1) := by
      simp [nextSeq, h]
    rw [hproceed] at hn
    injection hn with hseq
    rw [← hseq]
    exact Nat.lt_succ_of_le h
  · intro r
    rfl
  · intro r h s
    simp [raise]
  · intro r s
    rfl

/-! ### The falsifier

`nextSeqUnchecked` signs the snapshot's next sequence whatever the record says. The drill's record
is sequence 152 with the snapshot at 151: the mutant re-signs 152, a sequence already signed. -/

def law122_mutant : Prop :=
  ∃ (r : Record) (s : Seq),
    s < r.signed ∧ nextSeqUnchecked (some s) ≤ r.signed

theorem law122_falsifier : law122_mutant := by
  refine ⟨{ height := 155, signed := 152 }, 151, ?_, ?_⟩
  · decide
  · decide

end OwnSeq
end Rchain
