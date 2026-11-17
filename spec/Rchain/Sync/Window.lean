import Mathlib.Data.Finset.Basic

/-!
# Law 109 — validation roots are certified

**The change this law covers.** A checkpoint statement now commits to a **validation window**
(`historyRoot`): the recent state roots a post-anchor block — the PoS close — may read
(`casper/src/checkpoint.rs`, `models/src/casper/protocol/checkpoint.rs`). The joiner hydrates exactly
that set; it does not guess a window or widen one.

**What this law says.** A block validates only against the checkpoint root plus the certified window.
The checkpoint root is the **merged state of the finalized fringe** (Law 108), so the first
post-fringe block's pre-state — a fresh merge of the fringe members — *is* the checkpoint root, not a
root the joiner would have to guess or widen the window for. A root outside the set is refused, not
fetched into a bigger window. Under collision-freedom a changed window is a different commitment, so
a peer cannot substitute it under the same statement.

**What it does not claim.** The window size is a protocol constant, not a theorem about how many
blocks a PoS close reads; a future builder that needs a deeper root must change the constant (and the
committer) rather than widen the joiner. The state roots themselves are still a quorum claim (Law 85's
residual); the audit is the counterweight.

**Why it is not vacuous.** Admit any root into `validates` and `an_uncertified_root_is_refused` fails;
make the commitment non-injective and `the_history_root_binds_the_window` fails. The Rust witness
recomputes the window from the walked headers and refuses a shortened set.
-/

namespace Rchain

namespace Sync
namespace Window

/-- The certified validation window: the distinct state roots a post-anchor block may read. -/
structure CertifiedWindow where
  roots : Finset Nat
  deriving DecidableEq

/-- The certified root set: the checkpoint root plus the window. -/
def certifiedRoots (checkpoint : Nat) (w : CertifiedWindow) : Finset Nat :=
  insert checkpoint w.roots

/-- A block validates only if its pre-state root is in the certified set. -/
def validates (preState checkpoint : Nat) (w : CertifiedWindow) : Prop :=
  preState ∈ certifiedRoots checkpoint w

/-- **Law 109a — a validated block reads only certified roots.** -/
theorem a_validated_block_reads_only_certified_roots (pre checkpoint : Nat)
    (w : CertifiedWindow) (h : validates pre checkpoint w) :
    pre ∈ certifiedRoots checkpoint w :=
  h

/-- **Law 109b — an uncertified root is refused, not widened into the window.** -/
theorem an_uncertified_root_is_refused (pre checkpoint : Nat) (w : CertifiedWindow)
    (h : pre ∉ certifiedRoots checkpoint w) : ¬ validates pre checkpoint w :=
  h

/-- The commitment over a window. -/
def commitment (hash : Finset Nat → Nat) (w : CertifiedWindow) : Nat :=
  hash w.roots

/-- **Law 109c — the committed window is the window.** Collision-freedom is a hypothesis, not an
    axiom: with an injective commitment, equal commitments name the same roots. -/
theorem the_history_root_binds_the_window (hash : Finset Nat → Nat)
    (hash_inj : ∀ a b, hash a = hash b → a = b) (w₁ w₂ : CertifiedWindow)
    (h : commitment hash w₁ = commitment hash w₂) : w₁.roots = w₂.roots := by
  unfold commitment at h
  exact hash_inj _ _ h

/-- **Law 109d — the first post-fringe block builds on the certified checkpoint root.** Its
    pre-state is the fringe merge the certificate signs, so it is the checkpoint member of the
    certified set; the window is for historical reads, not for this base. -/
theorem the_first_post_fringe_block_builds_on_the_certified_root (checkpoint : Nat)
    (w : CertifiedWindow) : validates checkpoint checkpoint w := by
  simp [validates, certifiedRoots]

/-- Non-vacuity: a window member validates against the checkpoint that certifies it. -/
theorem a_window_root_validates (checkpoint r : Nat) (w : CertifiedWindow)
    (h : r ∈ w.roots) : validates r checkpoint w := by
  simp [validates, certifiedRoots, h]

/-- **Non-vacuity of 109b: a shortened window refuses a root it dropped.** -/
theorem a_shortened_window_refuses (checkpoint r : Nat) (w w' : CertifiedWindow)
    (_in_w : r ∈ w.roots) (not_in_w' : r ∉ w'.roots) (not_checkpoint : r ≠ checkpoint) :
    ¬ validates r checkpoint w' := by
  simp [validates, certifiedRoots, not_in_w', not_checkpoint]

end Window
end Sync

end Rchain
