import Rchain.Casper.Stake

/-!
# Law 85 — the fringe state is a claim about a fringe, not a field of a block

**Why this file exists.** A restore's `FinalizedFringe.state_hash` is the root the joiner hydrates its
tuple space at. It is served by three branches (`casper/src/engine/node_running.rs`):

* the **ordinary** answer — the max `(height, hash)` member of the latest finalised fringe's
  `fringe_state_hash`;
* a **recovery anchor** — the anchor block's `post_state_hash`;
* a **fresh shard** (no messages yet) — the genesis block's `post_state_hash`.

The ordinary branch is the one a joining node meets, and a member's `fringe_state_hash` is the state at
the fringe **it** finalises, one fringe earlier than the set it belongs to (`multi_parent_casper.rs`
records the distinction and the stall it caused when the reader took a member for a finaliser). So the
served root is a **claim about an earlier fringe**, not a field of the named block. A singleton pin
against that block's pre- or post-state is therefore false — it was implemented, the node's restore
suite refuted it (the genesis-branch claim was neither), and it was removed.

**What is proved.** The served root is always *some member's* claim, never a value the responder
invented; a finaliser's claim is, by definition, the state at the set it finalises; and a member's claim
need not equal any state its block carries (that is the refuted pin, as a theorem).

**What this model does not supply.** It does not implement the merge. The runtime
`engine::recovery::RecoveryReplay::verify` now recomputes metadata using the existing
validator and checks this relation before publishing recovered history (C339).
That Rust implementation is tested separately; no refinement proof is claimed here.
-/

namespace Rchain

/-- A block's fringe claim: the message set it finalises (its `fringe`), and the state it claims for
    that set (`fringe_state_hash`). -/
structure FringeClaim where
  /-- The block's id, ordered for the responder's max `(height, hash)` tie-break. -/
  blockId : Nat
  /-- The fringe this block finalises. -/
  fringe : List Nat
  /-- The state claimed for `fringe`. -/
  fringeState : Nat

variable (stateAt : List Nat → Nat)

/-- **A finaliser's claim is the state at the set it finalises.** This is the metadata write a
    validator makes once, when it validates the block whose `fringe` is that set. -/
def ClaimsTheStateAtItsFringe (b : FringeClaim) : Prop :=
  b.fringeState = stateAt b.fringe

/-- The member with the greater block id — the responder's max `(height, hash)` tie-break. -/
def later (a b : FringeClaim) : FringeClaim := if a.blockId < b.blockId then b else a

/-- The ordinary answer's root: the latest member's claim. `none` is a fringe with no members. -/
def servedRoot (members : List FringeClaim) : Option Nat :=
  match members with
  | [] => none
  | b :: rest => some (rest.foldl later b).fringeState

/-- `later` returns one of its arguments (the tie-break keeps the accumulator on equal ids). -/
theorem later_eq_left_or_right (a b : FringeClaim) : later a b = a ∨ later a b = b := by
  unfold later
  split <;> simp

/-- A fold of `later` stays inside the list it folded. -/
theorem foldl_later_mem (init : FringeClaim) (rest : List FringeClaim) :
    rest.foldl later init ∈ init :: rest := by
  induction rest generalizing init with
  | nil => simp
  | cons x xs ih =>
      simp only [List.foldl_cons]
      have h := ih (later init x)
      rcases List.mem_cons.mp h with h1 | h2
      · rcases later_eq_left_or_right init x with h' | h'
        · rw [h1, h']; simp
        · rw [h1, h']; simp
      · exact List.mem_cons_of_mem _ (List.mem_cons_of_mem _ h2)

/-- **The served root is some member's claim** — never a value the responder invented. -/
theorem the_served_root_is_a_members_claim (members : List FringeClaim) :
    servedRoot members = none ∨
      ∃ m ∈ members, servedRoot members = some m.fringeState := by
  cases members with
  | nil => exact Or.inl rfl
  | cons b rest =>
      refine Or.inr ⟨rest.foldl later b, ?_, rfl⟩
      exact foldl_later_mem b rest

/-- **The ordinary root is the state at the served member's own fringe** — one step earlier than the
    set the responder named, whenever that member is not a finaliser of the named set. -/
theorem the_served_root_is_the_state_at_the_members_fringe
    (members : List FringeClaim) (b : FringeClaim)
    (h : servedRoot members = some b.fringeState)
    (hc : ClaimsTheStateAtItsFringe stateAt b) :
    servedRoot members = some (stateAt b.fringe) := by
  rw [h, hc]

/-- **The refuted singleton pin, as a theorem.** A member's claim need not equal any state its block
    carries: take the empty fringe, whose state is `0`, and a block with pre-state `1` and post-state
    `2`. The claim is `0`, and neither field — which is why the pin that compared the two was false. -/
theorem a_members_claim_is_not_necessarily_a_block_state :
    ∃ (stateAt : List Nat → Nat) (b : FringeClaim) (pre post : Nat),
      ClaimsTheStateAtItsFringe stateAt b ∧ b.fringeState ≠ pre ∧ b.fringeState ≠ post := by
  refine ⟨fun _ => 0, ⟨0, [], 0⟩, 1, 2, ?_, ?_, ?_⟩ <;> simp [ClaimsTheStateAtItsFringe]

end Rchain
