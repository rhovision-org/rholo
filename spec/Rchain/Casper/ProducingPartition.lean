import Mathlib.Data.Finset.Basic
import Mathlib.Tactic

set_option autoImplicit false

/-!
# Law 120 — the live escape parent set sees the live partition

A quiet bonded sender can hold the round boundary open. The stalled-round escape
(`block-storage/src/dag/message_state.rs::parents_for_new_block_escaping`) must then justify every
live sender's newest message, not only the escaping sender's own message with the round snapshot's
stale peers. The finality gate's full-partition filter
(`block-storage/src/dag/finalizer.rs::sees_the_whole_partition`) credits a candidate only when every
member of the live partition has seen it; an escape that justifies only `{own latest, stale peers}`
can never credit the online supermajority.

This law states the parent-policy half: the live escape's seen set covers the live partition, and
the own-only policy does not.
-/

namespace Rchain
namespace ProducingPartition

abbrev Sender := Nat

/-- The union of every live sender's latest-message seen set. -/
def liveEscapeSeen (latest : Sender → Finset Sender) (partition : Finset Sender) : Finset Sender :=
  partition.biUnion latest

/-- **Law 120** — if every live sender's latest message contains its own sender, a block built from
    the live escape sees every member of the live partition. -/
def law120_statement : Prop :=
  ∀ (latest : Sender → Finset Sender) (partition : Finset Sender),
    (∀ s ∈ partition, s ∈ latest s) →
    partition ⊆ liveEscapeSeen latest partition

theorem law120 : law120_statement := by
  intro latest partition hlatest x hx
  rw [liveEscapeSeen]
  exact Finset.mem_biUnion.mpr ⟨x, hx, hlatest x hx⟩

/-- The own-only escape: the parent set is only the sender's own latest message. -/
def ownOnlySeen (latest : Sender → Finset Sender) (sender : Sender) : Finset Sender :=
  latest sender

/-- The own-only policy misses a live partition of at least two senders. -/
def law120_mutant : Prop :=
  ∃ (latest : Sender → Finset Sender) (partition : Finset Sender) (sender : Sender),
    (∀ s ∈ partition, s ∈ latest s) ∧
    sender ∈ partition ∧
    ¬ partition ⊆ ownOnlySeen latest sender

theorem law120_falsifier : law120_mutant := by
  refine ⟨fun s => ({s} : Finset Sender), ({0, 1} : Finset Sender), 0, ?_, ?_, ?_⟩
  · intro s _hs
    simp
  · simp
  · intro h
    have h1 : (1 : Sender) ∈ ({0, 1} : Finset Sender) := by simp
    have hmem : (1 : Sender) ∈ ownOnlySeen (fun s => ({s} : Finset Sender)) 0 := h h1
    simpa [ownOnlySeen] using hmem

end ProducingPartition
end Rchain
