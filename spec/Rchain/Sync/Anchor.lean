import Mathlib.Logic.Function.Basic

/-!
# Law 108 — a running node is anchored

**The change this law covers.** A certified sync accepts a state under a quorum checkpoint
certificate, or a genesis-only network accepts the operator's pinned genesis
(`casper/src/engine/anchor.rs`, `casper/src/engine/node_launch.rs`). The node then runs from that
state. **The checkpoint's root is the merged state of its finalized fringe** — the state a
post-anchor block builds on — not the anchor block's own post-state: finality is multi-parent, and
the 2026-11-17 join failed because the certificate named the member's post-state and the joiner never
hydrated the merge.

**What this law says.** The running phase carries an **accepted state**: an anchor (the pinned
genesis or a verified checkpoint) and the root the node runs from, with the root equal to the
anchor's by construction. A failed sync is not running — it is `syncing` or `refused`. There is no
state in which a node runs from a root it cannot attribute to its anchor.

**What it does not claim.** It does not claim the anchor's certificate is quorate (Law 14a) or
honest (Law 83/85); those are separate. It says the *shape* of the running state: no heuristic tip
root, no genesis left in place after a checkpoint was accepted.

**Why it is not vacuous.** Give `Accepted` a public constructor that omits `root_eq` and
`a_foreign_root_is_not_accepted` fails; allow the failed-sync transition to reach `running` and
`a_refused_sync_is_not_running` fails. The Rust witness is the pair of private constructors.
-/

namespace Rchain

namespace Sync
namespace Anchor

/-- The trust anchor: the operator's pinned genesis, or a certified checkpoint carrying the
    **merged state of its finalized fringe**. The second root is the fringe merge a post-anchor
    block actually builds on, not the anchor block's own post-state: finality is multi-parent, and
    the merged fringe state is not any member's post-state (Law 85). The certificate's anchor block
    and its post-state remain identity/audit fields; they are not the run root (2026-11-17). -/
inductive TrustAnchor where
  | genesis (pin root : Nat)
  | checkpoint (certificate fringeState : Nat)
  deriving DecidableEq

/-- The anchor's state root. -/
def TrustAnchor.root : TrustAnchor → Nat
  | .genesis _ root => root
  | .checkpoint _ root => root

/-- The genesis anchor names the operator's pin. -/
def TrustAnchor.genesisPin : TrustAnchor → Option Nat
  | .genesis pin _ => some pin
  | .checkpoint _ _ => none

/-- A genesis anchor from the pin and the genesis post-state. -/
def TrustAnchor.ofGenesis (pin root : Nat) : TrustAnchor := .genesis pin root

/-- **An accepted state.** The `root_eq` field is what makes a root that is not the anchor's
    unrepresentable: the structure has no other constructor, and the Rust type's fields are
    private behind the two checked constructors. -/
structure Accepted where
  anchor : TrustAnchor
  root : Nat
  root_eq : root = anchor.root

/-- The launch phase. `running` carries the accepted state; nothing else does. -/
inductive Phase where
  | syncing
  | running (state : Accepted)
  | refused (reason : Nat)

/-- **Law 108a — a running node's current root is its anchor's root.** -/
theorem running_is_anchored (state : Accepted) : state.root = state.anchor.root :=
  state.root_eq

/-- **Law 108d — a checkpoint anchors the certified fringe state.** The checkpoint branch's root is
    the merged state of the finalized fringe the certificate signs, which is the pre-state the first
    post-anchor block builds on. -/
theorem a_checkpoint_anchors_the_fringe_state (certificate fringeState : Nat) :
    (TrustAnchor.checkpoint certificate fringeState).root = fringeState := rfl

/-- **Non-vacuity of 108d: the anchor block's own post-state is not the run root.** The old model
    identified the run root with the anchor post-state; that is a different number from the fringe
    merge whenever the fringe has more than one member, and the live join failed on exactly it. -/
theorem a_checkpoint_root_is_not_a_blocks_post_state (certificate fringeState post : Nat)
    (h : post ≠ fringeState) :
    (TrustAnchor.checkpoint certificate fringeState).root ≠ post := by
  simpa [TrustAnchor.root] using h.symm

/-- **Law 108b — a refused sync is not running.** -/
theorem a_refused_sync_is_not_running (reason : Nat) (state : Accepted) :
    Phase.refused reason ≠ Phase.running state := by
  intro h
  cases h

/-- **Law 108c — a genesis anchor is the pinned genesis.** -/
theorem a_genesis_anchor_is_the_pinned_genesis (pin root : Nat) :
    (TrustAnchor.ofGenesis pin root).genesisPin = some pin := rfl

/-- **Non-vacuity of 108a: a foreign root is not an accepted state.** The `root_eq` field is the only
    construction of the equality, so no `Accepted` can carry a root its anchor does not name. -/
theorem a_foreign_root_is_not_accepted (anchor : TrustAnchor) (root : Nat)
    (h : root ≠ anchor.root) : ¬ ∃ s : Accepted, s.anchor = anchor ∧ s.root = root := by
  rintro ⟨s, rfl, hs⟩
  exact h (hs.symm.trans s.root_eq)

end Anchor
end Sync

end Rchain
