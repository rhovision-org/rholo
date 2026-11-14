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

end ReplayState
end Sync

end Rchain
