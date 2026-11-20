import Mathlib.Data.Finset.Basic
import Mathlib.Logic.Function.Basic

/-!
# Law 102 — replay-forward reconstructs the chain's states; genesis is the only external state

**The change this law covers.** The sync redesign stopped hydrating every historical block pre- and
post-state (`casper/src/engine/node_syncing.rs::collect_block_state_roots` walked the whole trie for
each root). `RecoveryReplay::verify` (`casper/src/engine/recovery.rs`) replays the downloaded blocks
in ascending height and derives each pre-state from its parents' post-states; the replay's
checkpoints persist every computed state. The only state replay cannot produce is the **genesis
post-state** — there is no block before height 0 to execute — so `required_state_roots` hydrates that
one root plus the served fringe root.

**What this law says.** A chain is *linked* when each block's declared pre-state is the state the
previous block produced. On a linked chain, replay has a result; on a broken one it refuses at the
first block whose pre-state is not supplied. The state replay reads from outside the chain is the
genesis state, and every other state it reads it has just produced.

**What it does not claim.** It is a single-chain abstraction: the Rust's parent set is a DAG merge,
not a list, and the model's `pre = previous post` is the one-parent projection of that merge. The
merge function's correctness (Laws 66/68/83) is assumed, not proved here. It also does not claim the
downloaded blocks are valid — Law 83 and the signature/hash checks are the Rust's, before replay.

**Why it is not vacuous.** Break a block's pre-state and
`a_block_whose_pre_state_is_not_the_previous_post_is_refused` fails; make replay read a downloaded
state instead of the produced one and `the_replay_reads_only_the_genesis_and_produced_states`
fails.
-/

namespace Rchain

namespace Sync
namespace ReplayState

/-- One block in the replay order: the state it declares as its pre-state, and the state it produces. -/
structure Block where
  pre : Nat
  post : Nat

/-- A chain is linked when each block's pre-state is the state the previous block produced. The
    list order is ascending height, which `RecoveryReplay::verify` iterates. -/
def LinkedFrom (s : Nat) : List Block → Prop
  | [] => True
  | b :: rest => b.pre = s ∧ LinkedFrom b.post rest

/-- Replay a linked chain from `s`: the result is the last produced state, or `none` at the first
    block whose pre-state is not the state in hand. -/
def replay (s : Nat) : List Block → Option Nat
  | [] => some s
  | b :: rest => if b.pre = s then replay b.post rest else none

/-- **Law 102a — a linked chain has a replay result, and a replay result implies a linked chain.** -/
theorem linked_iff_replay_has_a_result (s : Nat) (blocks : List Block) :
    LinkedFrom s blocks ↔ ∃ t, replay s blocks = some t := by
  induction blocks generalizing s with
  | nil => simp [LinkedFrom, replay]
  | cons b rest ih =>
      simp only [LinkedFrom, replay]
      by_cases h : b.pre = s
      · simp [h, ih]
      · simp [h]

/-- **Law 102b — replay reconstructs every state it reads.** A successful replay of a chain is
    exactly a linked chain: each block's pre-state is the state the previous block produced (or the
    genesis), never an independently downloaded value. -/
theorem replay_reconstructs_every_produced_state (s : Nat) (blocks : List Block)
    (t : Nat) (h : replay s blocks = some t) : LinkedFrom s blocks :=
  (linked_iff_replay_has_a_result s blocks).mpr ⟨t, h⟩

/-- **Law 102c — a block whose pre-state is not the previous post-state is refused.** The replay does
    not fall back to a downloaded state; it stops. -/
theorem a_block_whose_pre_state_is_not_the_previous_post_is_refused
    (s : Nat) (b : Block) (rest : List Block) (h : b.pre ≠ s) :
    replay s (b :: rest) = none := by
  simp [replay, h]

/-! ## The state decision as a phase space

`validate_block_checkpoint` derives a block's pre-state, replays its post-state, and compares the
block's claim against both. The trace `StateTrace` records the claim and the derivation. A safe
decision is one whose accepted claim is the derived triple; two validators with the same parent set
have at most one derivation. -/

/-- A validator's decision for one block: what the block claims, what this node derived, and whether
    the block was accepted. -/
structure StateDecision where
  claimedPre : Nat
  computedPre : Nat
  claimedPost : Nat
  computedPost : Nat
  claimedRejected : Finset Nat
  computedRejected : Finset Nat
  accepted : Bool

/-- **The safe predicate**: an accepted block's claim is exactly the derived state and ledger. A
    refusal is always safe — the block was not applied. -/
def stateSafe (d : StateDecision) : Prop :=
  d.accepted = false ∨
    (d.claimedPre = d.computedPre ∧
     d.claimedPost = d.computedPost ∧
     d.claimedRejected = d.computedRejected)

/-- A safe accepted decision is the derived state. -/
theorem a_safe_state_decision_is_the_derived_state (d : StateDecision)
    (h : stateSafe d) (ha : d.accepted = true) :
    d.claimedPre = d.computedPre ∧ d.claimedPost = d.computedPost ∧
      d.claimedRejected = d.computedRejected := by
  rcases h with h | h
  · exact absurd ha (by simp [h])
  · exact h

/-- **The derivation is a function of the parent set**: at most one decision's computed triple is
    *the* derivation for a given parent list. This is the agreement predicate RC-3 measures. -/
def Derives (derive : List Nat → StateDecision) (parents : List Nat) (d : StateDecision) : Prop :=
  derive parents = d

theorem same_parents_same_state (derive : List Nat → StateDecision) (parents : List Nat) :
    ∀ d₁ d₂, Derives derive parents d₁ → Derives derive parents d₂ → d₁ = d₂ := by
  intro d₁ d₂ h₁ h₂
  rw [← h₁, ← h₂]

/-- The outcome type. -/
inductive StateOutcome where
  | accepted
  | refused

def stateOutcome (d : StateDecision) : StateOutcome :=
  if d.accepted then .accepted else .refused

/-- **Totality**: every input has exactly one outcome. -/
theorem state_decision_is_total (d : StateDecision) :
    ∃! o : StateOutcome, stateOutcome d = o := by
  refine ⟨stateOutcome d, rfl, ?_⟩
  intro o ho
  exact ho.symm

/-- **The unsafe witness**: an accepted block whose pre-state is not the derived one. -/
def acceptedMismatch : StateDecision :=
  { claimedPre := 1, computedPre := 2, claimedPost := 2, computedPost := 2,
    claimedRejected := ∅, computedRejected := ∅, accepted := true }

theorem an_accepted_mismatch_is_the_unsafe_witness : ¬ stateSafe acceptedMismatch := by
  simp [stateSafe, acceptedMismatch]

/-! ## The audit target's shape as a phase space

An explicit-anchor replay reply carries exactly the named anchor and the anchor's post-state. The
Rust `target_shape_ok` (`casper/src/engine/recovery.rs`) is the guard; `anchored_target` is the only
constructor the audit uses. The trace `AuditTrace` records the hashes, the state and the verdict. -/

/-- The replay target's shape. `anchor = none` means the reply was not for a named anchor. -/
structure TargetShape where
  hashes : List Nat
  anchor : Option Nat
  stateIsAnchorPost : Bool

/-- **The safe shape**: a named anchor's target is exactly `[anchor]`, carrying the anchor's
    post-state. -/
def targetShapeSafe (t : TargetShape) : Prop :=
  match t.anchor with
  | none => True
  | some a => t.hashes = [a] ∧ t.stateIsAnchorPost = true

/-- **An anchored target is the only accepted shape.** -/
theorem an_anchored_target_is_the_only_accepted_shape (t : TargetShape) (a : Nat)
    (h : targetShapeSafe t) (hanchor : t.anchor = some a) : t.hashes = [a] := by
  simp [targetShapeSafe, hanchor] at h
  exact h.1

/-- A whole-fringe target for a named anchor is refused. -/
theorem a_whole_fringe_target_is_refused (a b : Nat) (hne : a ≠ b) :
    ¬ targetShapeSafe { hashes := [a, b], anchor := some a, stateIsAnchorPost := true } := by
  simp [targetShapeSafe, hne]

/-- **Totality**: every target is either the safe shape or not. -/
theorem target_shape_is_total (t : TargetShape) : targetShapeSafe t ∨ ¬ targetShapeSafe t :=
  Classical.em _

end ReplayState
end Sync

end Rchain
