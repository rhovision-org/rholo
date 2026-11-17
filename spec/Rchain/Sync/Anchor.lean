import Mathlib.Logic.Function.Basic
import Mathlib.Data.Finset.Basic
import Mathlib.Data.Finset.Image

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

The certificate's quorum is Law 104's subject and its honesty Law 85's. This law states the *shape*
of the running state: no heuristic tip root, no genesis left in place after a checkpoint was
accepted.

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

/-- A block header's checkpoint-visible projection: its hash, height, its own finalised fringe,
    and the two state roots the certificate commits. -/
structure Header where
  hash : Nat
  height : Nat
  fringe : Finset Nat
  fringeStateHash : Nat
  postStateHash : Nat
  deriving DecidableEq

/-- The signed checkpoint statement: every field `current_checkpoint` signs
    (`casper/src/api/block_api_impl.rs:1277-1357`). The three roots are optional on the wire so a
    legacy statement is still representable; `metadataRoot` and `historyRoot` are required by the
    joiner, `fringeHeadersRoot` is the v3 addition. -/
structure Statement where
  shardId : Nat
  anchor : Nat
  anchorHeight : Nat
  anchorPostState : Nat
  fringe : Finset Nat
  fringeState : Option Nat
  metadataRoot : Option Nat
  historyRoot : Option Nat
  fringeHeadersRoot : Option Nat
  deriving DecidableEq

/-- The material the joiner fetches and derives the commitments from. The three root functions are
    the opaque commitment hashes (`metadata_root`, `validation_window_root`,
    `fringe_headers_root`, `models/src/casper/protocol/checkpoint.rs`); the joiner compares hashes,
    so the model needs equality, not injectivity. -/
structure Projection where
  shardId : Nat
  anchorHeader : Header
  fringeHeaders : Finset Header
  ancestorHeaders : Finset Header
  metadataRoot : Finset Header → Nat
  historyRoot : Finset Header → Nat
  fringeHeadersRoot : Finset Header → Nat

/-- The Rust anchor rule: the anchor is the max `(height, hash)` member of the certificate's fringe
    that the walk holds (`max_by_key(|m| (m.height, m.id))`, `block_api_impl.rs:1280-1283`). -/
def isMaxOf (h : Header) (headers : Finset Header) : Prop :=
  h ∈ headers ∧ ∀ h' ∈ headers, h'.height < h.height ∨ (h'.height = h.height ∧ h'.hash ≤ h.hash)

/-- **The joiner's verification is exactly the checks `verify_*` performs.** Each field is a
    conjunction member, so a differing field is not a `Verifies` value; the optional fields are the
    v2/v3 wire shape (`verify_fringe_headers` accepts `None`, the metadata/window roots are
    required). -/
structure Verifies (s : Statement) (p : Projection) : Prop where
  shard : s.shardId = p.shardId
  anchor_hash : p.anchorHeader.hash = s.anchor
  anchor_max : isMaxOf p.anchorHeader p.fringeHeaders
  fringe_held : s.fringe ⊆ p.fringeHeaders.image (fun h => h.hash)
  post_state : s.anchorPostState = p.anchorHeader.postStateHash
  fringe_state : s.fringeState = none ∨ s.fringeState = some p.anchorHeader.fringeStateHash
  metadata : s.metadataRoot = some (p.metadataRoot p.ancestorHeaders)
  history : s.historyRoot = some (p.historyRoot p.ancestorHeaders)
  fringe_headers : s.fringeHeadersRoot = none ∨
    s.fringeHeadersRoot = some (p.fringeHeadersRoot p.fringeHeaders)

/-- A produced statement has a merge base: a nonempty finalised fringe and a non-zero fringe state.
    This is `current_checkpoint`'s refusal ("the anchor has finalised no fringe"), the condition
    the restore test's readiness poll waits out. -/
def wellFormed (s : Statement) : Prop :=
  s.fringe ≠ ∅ ∧ s.fringeState ≠ some 0

/-- **The anchor is the max held finalised-fringe member.** -/
theorem the_anchor_is_the_max_held_fringe_member (s : Statement) (p : Projection)
    (h : Verifies s p) : isMaxOf p.anchorHeader p.fringeHeaders :=
  h.anchor_max

/-- **The signed anchor post-state is the walked anchor header's.** -/
theorem the_anchor_post_state_is_the_headers (s : Statement) (p : Projection)
    (h : Verifies s p) : s.anchorPostState = p.anchorHeader.postStateHash :=
  h.post_state

/-- **The signed fringe state is the anchor header's committed fringe state.** -/
theorem the_signed_fringe_state_is_the_headers (s : Statement) (p : Projection)
    (h : Verifies s p) : s.fringeState = none ∨
      s.fringeState = some p.anchorHeader.fringeStateHash :=
  h.fringe_state

/-- **The signed metadata root is the ancestors' commitment.** -/
theorem the_metadata_root_is_the_ancestors_commitment (s : Statement) (p : Projection)
    (h : Verifies s p) : s.metadataRoot = some (p.metadataRoot p.ancestorHeaders) :=
  h.metadata

/-- **The signed history root is the validation window's commitment.** -/
theorem the_history_root_is_the_windows_commitment (s : Statement) (p : Projection)
    (h : Verifies s p) : s.historyRoot = some (p.historyRoot p.ancestorHeaders) :=
  h.history

/-- **The signed fringe-headers root is the members' commitment** when it is present. -/
theorem the_fringe_headers_root_is_the_members_commitment (s : Statement) (p : Projection)
    (h : Verifies s p) : s.fringeHeadersRoot = none ∨
      s.fringeHeadersRoot = some (p.fringeHeadersRoot p.fringeHeaders) :=
  h.fringe_headers

/-- **A differing metadata root is refused.** -/
theorem a_differing_metadata_root_is_refused (s : Statement) (p : Projection)
    (h : s.metadataRoot ≠ some (p.metadataRoot p.ancestorHeaders)) :
    ¬ Verifies s p :=
  fun hv => h hv.metadata

/-- **A differing history root is refused.** -/
theorem a_differing_history_root_is_refused (s : Statement) (p : Projection)
    (h : s.historyRoot ≠ some (p.historyRoot p.ancestorHeaders)) :
    ¬ Verifies s p :=
  fun hv => h hv.history

/-- **A missing metadata root is refused** (`MissingMetadataRoot`). -/
theorem a_missing_metadata_root_is_refused (s : Statement) (p : Projection)
    (h : s.metadataRoot = none) : ¬ Verifies s p := by
  intro hv
  have hfield := hv.metadata
  rw [h] at hfield
  exact Option.noConfusion hfield

/-- **A missing history root is refused** (`MissingValidationWindow`). -/
theorem a_missing_history_root_is_refused (s : Statement) (p : Projection)
    (h : s.historyRoot = none) : ¬ Verifies s p := by
  intro hv
  have hfield := hv.history
  rw [h] at hfield
  exact Option.noConfusion hfield

/-- **A differing present fringe state is refused.** -/
theorem a_differing_fringe_state_is_refused (s : Statement) (p : Projection) (x : Nat)
    (hs : s.fringeState = some x) (hx : x ≠ p.anchorHeader.fringeStateHash) :
    ¬ Verifies s p := by
  intro hv
  rcases hv.fringe_state with hnone | hsome
  · rw [hs] at hnone
    exact Option.noConfusion hnone
  · rw [hs] at hsome
    exact hx (Option.some.inj hsome)

/-- **A differing present fringe-headers root is refused.** -/
theorem a_differing_fringe_headers_root_is_refused (s : Statement) (p : Projection) (x : Nat)
    (hs : s.fringeHeadersRoot = some x)
    (hx : x ≠ p.fringeHeadersRoot p.fringeHeaders) :
    ¬ Verifies s p := by
  intro hv
  rcases hv.fringe_headers with hnone | hsome
  · rw [hs] at hnone
    exact Option.noConfusion hnone
  · rw [hs] at hsome
    exact hx (Option.some.inj hsome)

/-- **An absent v3 root does not refuse**: `verify_fringe_headers` accepts a legacy statement. -/
theorem an_absent_fringe_headers_root_does_not_refuse (s : Statement) (p : Projection)
    (h : s.fringeHeadersRoot = none) :
    s.fringeHeadersRoot = none ∨
      s.fringeHeadersRoot = some (p.fringeHeadersRoot p.fringeHeaders) :=
  Or.inl h

/-- **An anchor without a finalised fringe is not a produced statement.** -/
theorem an_anchor_without_a_finalised_fringe_is_not_well_formed (s : Statement)
    (h : s.fringe = ∅) : ¬ wellFormed s := by
  simp [wellFormed, h]

/-- **An anchor with a zero fringe state is not a produced statement.** -/
theorem an_anchor_with_a_zero_fringe_state_is_not_well_formed (s : Statement)
    (h : s.fringeState = some 0) : ¬ wellFormed s := by
  simp [wellFormed, h]

/-- **The mutant proposition**: a statement whose signed metadata root differs from the fetched
    ancestors' commitment nevertheless verifies. -/
def law117_mutant : Prop :=
  ∃ (s : Statement) (p : Projection),
    s.metadataRoot ≠ some (p.metadataRoot p.ancestorHeaders) ∧ Verifies s p

/-- **The falsifier, fired.** The mutant is unsatisfiable: `Verifies.metadata` is the equality the
    mutant negates. -/
theorem law117_falsifier : ¬ law117_mutant := by
  rintro ⟨s, p, hne, hv⟩
  exact hne hv.metadata

/-- **Law 117's statement.** Every field the joiner verifies is exactly the fetched material's
    commitment; the anchor is the max held fringe member; a produced statement has a merge base;
    and a statement with a differing signed field cannot verify. -/
def law117_statement : Prop :=
  (∀ (s : Statement) (p : Projection), Verifies s p →
    s.metadataRoot = some (p.metadataRoot p.ancestorHeaders)) ∧
  (∀ (s : Statement) (p : Projection), Verifies s p →
    s.historyRoot = some (p.historyRoot p.ancestorHeaders)) ∧
  (∀ (s : Statement) (p : Projection), Verifies s p →
    s.anchorPostState = p.anchorHeader.postStateHash) ∧
  (∀ (s : Statement) (p : Projection), Verifies s p →
    (s.fringeState = none ∨ s.fringeState = some p.anchorHeader.fringeStateHash)) ∧
  (∀ (s : Statement) (p : Projection), Verifies s p →
    (s.fringeHeadersRoot = none ∨
      s.fringeHeadersRoot = some (p.fringeHeadersRoot p.fringeHeaders))) ∧
  (∀ (s : Statement) (p : Projection), Verifies s p →
    isMaxOf p.anchorHeader p.fringeHeaders) ∧
  (∀ s : Statement, s.fringe = ∅ → ¬ wellFormed s) ∧
  (∀ s : Statement, s.fringeState = some 0 → ¬ wellFormed s) ∧
  ¬ law117_mutant

theorem law117 : law117_statement := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, law117_falsifier⟩
  · intro s p h; exact h.metadata
  · intro s p h; exact h.history
  · intro s p h; exact h.post_state
  · intro s p h; exact h.fringe_state
  · intro s p h; exact h.fringe_headers
  · intro s p h; exact h.anchor_max
  · intro s h; exact an_anchor_without_a_finalised_fringe_is_not_well_formed s h
  · intro s h; exact an_anchor_with_a_zero_fringe_state_is_not_well_formed s h

end Anchor
end Sync

end Rchain
