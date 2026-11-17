import Mathlib.Data.Finset.Basic
import Mathlib.Tactic

/-!
# Law 114 — the certificate statement determines what a joiner must hold

**The change this law covers.** A checkpoint joiner must validate the first post-anchor block
without replaying the historical chain. That validation reads the anchor body, the bodies of the
fringe the anchor finalised, and — for a boundary re-index — those members' own finalised fringes.
The certificate names the anchor and the finalised fringe; checkpoint v3 commits the fringe
members' headers under `fringeHeadersRoot`, which is how a joiner obtains each member's own fringe.
`join_input_set` (`casper/src/engine/node_syncing.rs`) assembles exactly this set, fetches the
bodies, and adds their committed roots to the hydration set before the sync can report ready.

**What this law says.** Acceptance is a structure that contains the required set: an accepted join
with a missing body is not representable. The required set is a function of the statement — the
anchor, the fringe, the anchor's own fringe and the members' committed fringes — not of the chain's
arbitrary ancestry. The v2 statement, which names the fringe but does not commit the members'
headers, omits a body the v3 statement requires; that omission is the mutant and the measured
cold-join failure.

**What it does not claim.** That the fetched bodies are authentic — the bodies are hash-addressed
and their roots are committed (Laws 104/109), and the audit is the counterweight. The quorum's
honesty remains Law 85's residual.
-/

namespace Rchain
namespace Sync
namespace JoinInputs

/-- The statement fields a joiner derives its required bodies from. `memberFringe` is the `fringe`
    field of each finalised-fringe member's header: v3 commits those headers under
    `fringeHeadersRoot`, v2 does not. -/
structure Statement where
  anchor : Nat
  anchorFringe : Finset Nat
  fringe : Finset Nat
  memberFringe : Nat → Finset Nat

/-- The bodies `join_input_set` fetches: the anchor, the certificate's fringe, the anchor header's
    own fringe, and each fringe member's own fringe. -/
def requiredBodies (s : Statement) : Finset Nat :=
  insert s.anchor (s.fringe ∪ s.anchorFringe ∪ s.fringe.biUnion s.memberFringe)

/-- The v2 statement's required set: it names the fringe but does not commit the members' headers,
    so it cannot derive their own fringes. This is the mutant. -/
def v2RequiredBodies (s : Statement) : Finset Nat :=
  insert s.anchor (s.fringe ∪ s.anchorFringe)

/-- An accepted join holds every required body. The Rust `join_input_set` assembles the set and the
    sync refuses a missing body before it reports ready. -/
structure Accepted (s : Statement) (held : Finset Nat) : Prop where
  holds : requiredBodies s ⊆ held

/-- **An incomplete joiner is not representable.** Acceptance contains the required set, so a
    missing body is a contradiction, not a reachable ready state. -/
theorem an_incomplete_joiner_is_not_representable (s : Statement) (held : Finset Nat)
    (missing : Nat) (hm : missing ∈ requiredBodies s) (hnot : missing ∉ held) :
    ¬ Accepted s held := by
  intro h
  exact hnot (h.holds hm)

/-- **The statement determines the inputs.** Two statements with the same anchor, fringe and
    committed member fringes require the same bodies: the input set is a function of what the
    certificate commits, not of the chain's length. -/
theorem the_statement_determines_the_join_inputs (s t : Statement)
    (hAnchor : s.anchor = t.anchor) (hFringe : s.fringe = t.fringe)
    (hAnchorFringe : s.anchorFringe = t.anchorFringe)
    (hMembers : ∀ m, s.memberFringe m = t.memberFringe m) :
    requiredBodies s = requiredBodies t := by
  have hfun : s.memberFringe = t.memberFringe := funext hMembers
  simp [requiredBodies, hAnchor, hFringe, hAnchorFringe, hfun]

/-- **The v2 mutant**: a member's own fringe is required by the v3 statement and absent from the
    v2 one. The 2026-11-17 cold join failed on exactly this input (a same-height sibling's
    pre-state). -/
def law114_mutant : Prop :=
  ∃ (s : Statement) (x : Nat), x ∈ requiredBodies s ∧ x ∉ v2RequiredBodies s

/-- **The falsifier, fired.** Anchor `1`, fringe `{2}`, member `2`'s own fringe `{3}`: the v3 set
    requires `3`, the v2 set does not. -/
theorem law114_falsifier : law114_mutant := by
  refine ⟨⟨1, ∅, {2}, fun m => if m = 2 then {3} else ∅⟩, 3, ?_, ?_⟩ <;>
    simp [requiredBodies, v2RequiredBodies]

/-- **Law 114's statement.** Acceptance contains the required set, and the v2 statement omits a
    member's fringe. -/
def law114_statement : Prop :=
  (∀ (s : Statement) (held : Finset Nat) (missing : Nat),
    missing ∈ requiredBodies s → missing ∉ held → ¬ Accepted s held) ∧ law114_mutant

theorem law114 : law114_statement := by
  refine ⟨?_, law114_falsifier⟩
  intro s held missing hm hnot
  exact an_incomplete_joiner_is_not_representable s held missing hm hnot

end JoinInputs
end Sync
end Rchain
