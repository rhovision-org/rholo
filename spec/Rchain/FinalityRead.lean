import Mathlib.Data.List.Basic

/-!
# Law 125 — the finalized read is the certified fringe's seen closure

Every finalized answer a surface gives — `/health.finalizedHeight`, `/api/last-finalized-block`,
`/api/is-finalized/{hash}`, and the inclusion check behind `/api/v1/deploy-settlement/{id}` — derives
from one node-local value: `DagRepresentation.finalized_blocks_set`, the union of the `seen` sets of
the latest fringe's members (`block-storage/src/dag/representation.rs:57-83,141-143`). The register
had no model of that read. This module is the model.

Three facts are load-bearing:

* **The read is the seen closure, not the fringe's ids.** A block finalized at an earlier layer is
  still finalized because every later fringe member has seen it; reading only the fringe members
  would drop the history.
* **A fringe member is finalized when its own `seen` contains itself.** The Rust constructs a
  message's `seen` as the union of its justifications' seen sets plus its own hash
  (`casper/src/dag.rs:91-105`), so this hypothesis is the derivation's, not an assumption about an
  arbitrary value.
* **The observation is node-local.** The finality-observation endpoint reports `scope: nodeLocal`
  and `availability: unknown`; a local read is not a quorum claim, and the model keeps that label
  separate from the read.

The mutant is the read that trusts the fringe's ids without their `seen` sets: it reports a block as
finalized only while it is a fringe member, and the witness shows a seen-but-not-member block that
the law finalizes and the mutant does not. That is the shape a surface would produce if it read the
fringe instead of the closure.
-/

namespace Rchain
namespace FinalityRead

/-- A DAG message as the finalized read sees it: the `seen` cache is the set of message ids this
    message's ancestry covers, including itself (`block-storage/src/dag/finalizer.rs:24-46`). -/
structure Msg where
  id : Nat
  height : Nat
  seen : List Nat
  deriving DecidableEq

/-- The union of the fringe members' seen sets. -/
def fringeSeen (fringe : List Msg) : List Nat :=
  (fringe.map (fun m => m.seen)).join

/-- **The local finalized read.** A block is finalized exactly when it is in the seen closure of the
    latest fringe. -/
def isFinalized (fringe : List Msg) (h : Nat) : Prop :=
  h ∈ fringeSeen fringe

/-- The read that trusts only the fringe's own ids — the mutant. -/
def isFinalizedUnchecked (fringe : List Msg) (h : Nat) : Prop :=
  h ∈ fringe.map (fun m => m.id)

/-- The read is an existential over the fringe: a block is finalized when some fringe member has
    seen it. -/
theorem is_finalized_iff_exists_seen (fringe : List Msg) (h : Nat) :
    isFinalized fringe h ↔ ∃ m ∈ fringe, h ∈ m.seen := by
  simp [isFinalized, fringeSeen, List.mem_join, List.mem_map]

/-- A block that a fringe member has seen is finalized. -/
theorem a_seen_member_is_finalized (fringe : List Msg) (m : Msg) (hm : m ∈ fringe)
    (h : Nat) (hh : h ∈ m.seen) : isFinalized fringe h :=
  (is_finalized_iff_exists_seen fringe h).mpr ⟨m, hm, hh⟩

/-- **Every fringe member is finalized.** The Rust constructs `seen` with the message's own hash
    (`casper/src/dag.rs:95`), so this is the read's derivation, not a property of arbitrary data. -/
theorem a_fringe_member_is_finalized (fringe : List Msg)
    (hself : ∀ m ∈ fringe, m.id ∈ m.seen) :
    ∀ m ∈ fringe, isFinalized fringe m.id := by
  intro m hm
  exact a_seen_member_is_finalized fringe m hm m.id (hself m hm)

/-- The observation's availability, kept separate from the local read. -/
inductive Availability where
  | unknown
  | finalized
  | unfinalized
  deriving DecidableEq

/-- A finality observation: the endpoint's `scope` and `availability`. -/
structure Observation where
  scope : String
  availability : Availability

/-- The node-local observation the endpoint serves. -/
def localObservation : Observation :=
  { scope := "nodeLocal", availability := .unknown }

/-- **A node-local observation does not claim finality.** The local read is not a quorum verdict;
    the endpoint says so, and the model makes the label a value that cannot be `finalized`. -/
theorem a_local_observation_does_not_claim_finality :
    localObservation.availability ≠ Availability.finalized := by
  decide

/-- The mutant: the read trusts the fringe's ids and drops the seen closure. The witness is a block
    seen by the tip but not itself a fringe member. -/
def law125_mutant : Prop :=
  ∃ (fringe : List Msg) (h : Nat), isFinalized fringe h ∧ ¬ isFinalizedUnchecked fringe h

/-- The falsifier: a two-layer chain whose tip has seen height 0; the law finalizes 0, the
    ids-only read does not. -/
theorem law125_falsifier : law125_mutant := by
  refine ⟨[{ id := 2, height := 2, seen := [0, 1, 2] }], 0, ?_, ?_⟩
  · simp [isFinalized, fringeSeen]
  · simp [isFinalizedUnchecked]

/-- **Law 125's statement.** The read is the seen closure; each fringe member is finalized under the
    self-seen construction; the observation is node-local and does not claim finality; and the
    ids-only read is refuted by a seen-but-not-member block. -/
def law125_statement : Prop :=
  (∀ (fringe : List Msg) (h : Nat), isFinalized fringe h ↔ ∃ m ∈ fringe, h ∈ m.seen) ∧
  (∀ (fringe : List Msg), (∀ m ∈ fringe, m.id ∈ m.seen) →
      ∀ m ∈ fringe, isFinalized fringe m.id) ∧
  (localObservation.availability ≠ Availability.finalized) ∧
  law125_mutant

theorem law125 : law125_statement := by
  refine ⟨?_, ?_, ?_, ?_⟩
  · intro fringe h
    exact is_finalized_iff_exists_seen fringe h
  · intro fringe hself m hm
    exact a_fringe_member_is_finalized fringe hself m hm
  · exact a_local_observation_does_not_claim_finality
  · exact law125_falsifier

end FinalityRead
end Rchain
