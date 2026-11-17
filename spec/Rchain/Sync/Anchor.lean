import Mathlib.Logic.Function.Basic
import Mathlib.Data.Finset.Basic

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

/-- **The anchor header's committed projection**: the fringe it names and the fringe-state root it
    commits. The checkpoint statement's signed `fringeState` must be this root, and its signed
    `fringe_hashes` this fringe: that is the equality `current_checkpoint` constructs
    (`casper/src/api/block_api_impl.rs:1298-1312`) and the joiner re-checks against the walked
    header (`casper/src/engine/node_syncing.rs:858-865`). -/
structure HeaderProjection where
  fringe : Finset Nat
  fringeStateHash : Nat
  deriving DecidableEq

/-- The signed attestation fields of a checkpoint statement. -/
structure Attested where
  fringe : Finset Nat
  fringeState : Nat
  deriving DecidableEq

/-- **The attestation check.** The signed root is the header's committed fringe state and the signed
    fringe is the header's fringe. Both equalities are checked before acceptance. -/
def attests (header : HeaderProjection) (statement : Attested) : Prop :=
  statement.fringeState = header.fringeStateHash ∧ statement.fringe = header.fringe

/-- **The signed root is the anchor header's fringe state.** This is the value the boot re-check
    reads; it is not the anchor block's post-state (Law 108d). -/
theorem the_signed_root_is_the_anchors_fringe_state (header : HeaderProjection)
    (statement : Attested) (h : attests header statement) :
    statement.fringeState = header.fringeStateHash :=
  h.1

/-- **The signed fringe is the anchor header's fringe.** The statement cannot name a different set
    of members than the header it is a statement about. -/
theorem the_signed_fringe_is_the_anchors_fringe (header : HeaderProjection)
    (statement : Attested) (h : attests header statement) :
    statement.fringe = header.fringe :=
  h.2

/-- **A signed root that is not the header's fringe state is refused.** The check is an equality,
    so a statement carrying a different root cannot satisfy it. -/
theorem a_signed_root_that_is_not_the_fringes_state_is_refused (header : HeaderProjection)
    (statement : Attested) (h : statement.fringeState ≠ header.fringeStateHash) :
    ¬ attests header statement := by
  intro hattests
  exact h hattests.1

/-- **Law 117's statement.** An attested statement's signed root and fringe are exactly the header's
    committed root and fringe. -/
def law117_statement : Prop :=
  ∀ (header : HeaderProjection) (statement : Attested),
    attests header statement →
      statement.fringeState = header.fringeStateHash ∧ statement.fringe = header.fringe

theorem law117 : law117_statement := by
  intro header statement h
  exact h

/-- **The mutant proposition**: a statement whose signed root differs from the header's committed
    fringe state nevertheless attests. The check forbids it. -/
def law117_mutant : Prop :=
  ∃ (header : HeaderProjection) (statement : Attested),
    statement.fringeState ≠ header.fringeStateHash ∧ attests header statement

/-- **The falsifier, fired.** The mutant is unsatisfiable: the attestation's first conjunct is the
    equality the mutant negates. -/
theorem law117_falsifier : ¬ law117_mutant := by
  rintro ⟨header, statement, hne, hattests⟩
  exact hne hattests.1

end Anchor
end Sync

end Rchain
