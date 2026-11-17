import Mathlib.Data.Finset.Basic
import Mathlib.Tactic

/-!
# Law 114 — the certificate statement determines what a joiner must hold

**The change this law covers.** A checkpoint joiner must validate the first post-anchor block
without replaying the historical chain. That validation reads the anchor body, the bodies of the
fringe the anchor finalised, those members' own finalised fringes, and — for the deploy-duplication
check — every block body within `DEPLOY_LIFESPAN` heights of the anchor. The certificate names the
anchor and the finalised fringe; checkpoint v3 commits the fringe members' headers under
`fringeHeadersRoot`, which is how a joiner obtains each member's own fringe. `join_input_set`
(`casper/src/engine/node_syncing.rs:1534-1599`) assembles exactly this set, fetches the bodies, adds
the expanded replay layer's committed roots, and only then can the sync report ready.

**The model.** The body set is the expanded replay layer **plus** the deploy window above
`anchor_height - DEPLOY_LIFESPAN`; the root set is over the expanded replay layer only, because the
Rust collects roots before the deploy-window extension. The deploy window is parent-closed above the
floor, which is the `repeat_deploy` scan's stopping rule (`casper/src/validate.rs:525-572`). A
certificate pinned before its broadcast has exactly one sync target, its anchor; a later broadcast
that is not `[anchor]` is ignored (`casper/src/engine/node_syncing.rs::on_finalized_fringe_message`).
-/

namespace Rchain
namespace Sync
namespace JoinInputs

/-- The statement fields a joiner derives its required bodies from. `memberFringe` is the `fringe`
    field of each finalised-fringe member's header: v3 commits those headers under
    `fringeHeadersRoot`, v2 does not. `walkHeaders` and `headerHeight` are the metadata walk the
    deploy window is selected from. -/
structure Statement where
  anchor : Nat
  anchorFringe : Finset Nat
  fringe : Finset Nat
  memberFringe : Nat → Finset Nat
  anchorHeight : Nat
  headerHeight : Nat → Nat
  walkHeaders : Finset Nat
  deployLifespan : Nat

/-- The replay layer: the anchor, the certificate's fringe, and the anchor header's own fringe. -/
def replayLayer (s : Statement) : Finset Nat :=
  insert s.anchor (s.fringe ∪ s.anchorFringe)

/-- The replay layer **after** the one-pass member expansion. This is the set the Rust calls
    `replay_layer` when it collects roots. -/
def expandedReplayLayer (s : Statement) : Finset Nat :=
  replayLayer s ∪ (replayLayer s).biUnion s.memberFringe

/-- The deploy-duplication window: every walk header strictly above
    `anchor_height - DEPLOY_LIFESPAN`. The Rust floor is an `i64`, so it is modelled in `Int`; the
    comparison is exact even when the floor is negative. -/
def deployWindowBodies (s : Statement) : Finset Nat :=
  s.walkHeaders.filter
    (fun h => (s.headerHeight h : Int) > (s.anchorHeight : Int) - (s.deployLifespan : Int))

/-- The bodies `join_input_set` fetches: the expanded replay layer plus the deploy window. -/
def requiredBodies (s : Statement) : Finset Nat :=
  expandedReplayLayer s ∪ deployWindowBodies s

/-- The roots the Rust adds: pre-state, post-state and fringe state of **every expanded
    replay-layer header**, skipping the zero placeholder. The deploy-window bodies are fetched for
    `repeat_deploy`, not hydrated, so they contribute no roots. -/
def requiredRoots (s : Statement) (rootsOf : Nat → Finset Nat) : Finset Nat :=
  (expandedReplayLayer s).biUnion rootsOf

/-- The v2 statement's required set: it names the fringe but does not commit the members' headers,
    so it cannot derive their own fringes. This is the measured cold-join mutant. -/
def v2RequiredBodies (s : Statement) : Finset Nat :=
  replayLayer s

/-- **An accepted join holds every required body and every required root.** The Rust
    `join_input_set` returns both sets and the sync refuses a missing item before it reports ready. -/
structure Accepted (s : Statement) (rootsOf : Nat → Finset Nat) (heldBodies heldRoots : Finset Nat)
    : Prop where
  holdsBodies : requiredBodies s ⊆ heldBodies
  holdsRoots : requiredRoots s rootsOf ⊆ heldRoots

/-- **An incomplete joiner is not representable.** -/
theorem an_incomplete_joiner_is_not_representable (s : Statement)
    (rootsOf : Nat → Finset Nat) (heldBodies heldRoots : Finset Nat)
    (missing : Nat) (hm : missing ∈ requiredBodies s) (hnot : missing ∉ heldBodies) :
    ¬ Accepted s rootsOf heldBodies heldRoots := by
  intro h
  exact hnot (h.holdsBodies hm)

/-- **An unheld root is not representable either.** -/
theorem an_unheld_root_is_not_representable (s : Statement) (rootsOf : Nat → Finset Nat)
    (heldBodies heldRoots : Finset Nat) (missing : Nat)
    (hm : missing ∈ requiredRoots s rootsOf) (hnot : missing ∉ heldRoots) :
    ¬ Accepted s rootsOf heldBodies heldRoots := by
  intro h
  exact hnot (h.holdsRoots hm)

/-- **A missing deploy-window body is not representable.** The body is required even though it is
    not a replay-layer root: `repeat_deploy` reads bodies, not roots. -/
theorem a_missing_deploy_window_body_is_not_representable (s : Statement)
    (rootsOf : Nat → Finset Nat) (heldBodies heldRoots : Finset Nat) (missing : Nat)
    (hm : missing ∈ deployWindowBodies s) (hnot : missing ∉ heldBodies) :
    ¬ Accepted s rootsOf heldBodies heldRoots :=
  an_incomplete_joiner_is_not_representable s rootsOf heldBodies heldRoots missing
    (Finset.mem_union.mpr (Or.inr hm)) hnot

/-- **The statement determines the inputs.** Two statements with the same anchor, fringe, committed
    member fringes and deploy-window data require the same bodies: the input set is a function of
    what the certificate commits, not of the chain's length. -/
theorem the_statement_determines_the_join_inputs (s t : Statement)
    (hAnchor : s.anchor = t.anchor) (hFringe : s.fringe = t.fringe)
    (hAnchorFringe : s.anchorFringe = t.anchorFringe)
    (hMembers : ∀ m, s.memberFringe m = t.memberFringe m)
    (hAnchorHeight : s.anchorHeight = t.anchorHeight)
    (hHeaderHeight : ∀ x, s.headerHeight x = t.headerHeight x)
    (hWalk : s.walkHeaders = t.walkHeaders)
    (hLifespan : s.deployLifespan = t.deployLifespan) :
    requiredBodies s = requiredBodies t := by
  have hfun : s.memberFringe = t.memberFringe := funext hMembers
  have hheight : s.headerHeight = t.headerHeight := funext hHeaderHeight
  simp [requiredBodies, expandedReplayLayer, deployWindowBodies, replayLayer,
    hAnchor, hFringe, hAnchorFringe, hfun, hAnchorHeight, hheight, hWalk, hLifespan]

/-- **The v2 mutant**: a member's own fringe is required by the v3 statement and absent from the
    v2 one. The 2026-11-17 cold join failed on exactly this input (a same-height sibling's
    pre-state). -/
def law114_mutant : Prop :=
  ∃ (s : Statement) (x : Nat), x ∈ requiredBodies s ∧ x ∉ v2RequiredBodies s

/-- **The falsifier, fired.** Anchor `1`, fringe `{2}`, member `2`'s own fringe `{3}`: the v3 set
    requires `3`, the v2 set does not. -/
theorem law114_falsifier : law114_mutant := by
  refine ⟨⟨1, ∅, {2}, fun m => if m = 2 then {3} else ∅, 0, fun _ => 0, ∅, 50⟩, 3, ?_, ?_⟩ <;>
    simp [requiredBodies, expandedReplayLayer, replayLayer, v2RequiredBodies, deployWindowBodies]

/-- **The deploy-window mutant**: a joiner that fetches only the expanded replay layer omits a body
    the first post-anchor block's `repeat_deploy` scan reads. -/
def law114_deploy_window_mutant : Prop :=
  ∃ (s : Statement) (x : Nat), x ∈ requiredBodies s ∧ x ∉ expandedReplayLayer s

/-- **The deploy-window falsifier, fired.** Anchor height 100, lifespan 50, walk header 60:
    `60 > 50` puts it in the window, and it is not in the replay layer. -/
theorem law114_deploy_window_falsifier : law114_deploy_window_mutant := by
  refine ⟨⟨100, ∅, {101}, (fun _ => ∅), 100, fun h => h, {60}, 50⟩, 60, ?_, ?_⟩ <;>
    simp [requiredBodies, expandedReplayLayer, deployWindowBodies, replayLayer]

/-- **The deploy window is parent-closed above the floor.** If a window body's parent is in the
    walk and above the floor, the parent is a window body too. This is exactly the `repeat_deploy`
    scan's stopping rule: the BFS cannot leave the window. -/
theorem the_deploy_window_is_closed_under_parents (s : Statement) (parents : Nat → Finset Nat)
    (hwalk : ∀ x ∈ s.walkHeaders, parents x ⊆ s.walkHeaders) :
    ∀ x ∈ deployWindowBodies s, ∀ p ∈ parents x,
      (s.headerHeight p : Int) > (s.anchorHeight : Int) - (s.deployLifespan : Int) →
        p ∈ deployWindowBodies s := by
  intro x hx p hp hfloor
  simp only [deployWindowBodies, Finset.mem_filter] at hx ⊢
  exact ⟨hwalk x hx.1 hp, hfloor⟩

/-- **A pinned certificate targets exactly its anchor.** The sync target a pinned certificate fixes
    is `[anchor]`; a later `FinalizedFringe` broadcast that is not `[anchor]` is ignored. This is the
    handshake the 2026-11-17 cold join needed (`on_finalized_fringe_message`). -/
structure Target where
  certificate : Option Statement
  target : Finset Nat

def pin (s : Statement) : Target :=
  { certificate := some s, target := {s.anchor} }

def onFringeBroadcast (t : Target) (fringe : Finset Nat) : Target :=
  match t.certificate with
  | some s => if fringe = {s.anchor} then { t with target := fringe } else t
  | none => { t with target := fringe }

theorem a_pinned_certificate_targets_exactly_its_anchor (s : Statement) (fringe : Finset Nat)
    (h : fringe ≠ {s.anchor}) :
    (onFringeBroadcast (pin s) fringe).target = {s.anchor} := by
  simp [onFringeBroadcast, pin, h]

/-- **Law 114's statement.** Acceptance contains the required body and root sets; the v2 statement
    omits a member's fringe; the deploy window is parent-closed; and a pinned certificate's target
    is its anchor. -/
def law114_statement : Prop :=
  (∀ (s : Statement) (rootsOf : Nat → Finset Nat) (heldBodies heldRoots : Finset Nat)
      (missing : Nat),
    missing ∈ requiredBodies s → missing ∉ heldBodies →
      ¬ Accepted s rootsOf heldBodies heldRoots) ∧
  (∀ (s : Statement) (rootsOf : Nat → Finset Nat) (heldBodies heldRoots : Finset Nat)
      (missing : Nat),
    missing ∈ requiredRoots s rootsOf → missing ∉ heldRoots →
      ¬ Accepted s rootsOf heldBodies heldRoots) ∧
  (∀ (s : Statement) (parents : Nat → Finset Nat),
    (∀ x ∈ s.walkHeaders, parents x ⊆ s.walkHeaders) →
    ∀ x ∈ deployWindowBodies s, ∀ p ∈ parents x,
      (s.headerHeight p : Int) > (s.anchorHeight : Int) - (s.deployLifespan : Int) →
        p ∈ deployWindowBodies s) ∧
  (∀ (s : Statement) (fringe : Finset Nat), fringe ≠ {s.anchor} →
    (onFringeBroadcast (pin s) fringe).target = {s.anchor}) ∧
  law114_mutant ∧ law114_deploy_window_mutant

theorem law114 : law114_statement := by
  refine ⟨?_, ?_, ?_, ?_, law114_falsifier, law114_deploy_window_falsifier⟩
  · intro s rootsOf heldBodies heldRoots missing hm hnot
    exact an_incomplete_joiner_is_not_representable s rootsOf heldBodies heldRoots missing hm hnot
  · intro s rootsOf heldBodies heldRoots missing hm hnot
    exact an_unheld_root_is_not_representable s rootsOf heldBodies heldRoots missing hm hnot
  · intro s parents hwalk x hx p hp hfloor
    exact the_deploy_window_is_closed_under_parents s parents hwalk x hx p hp hfloor
  · intro s fringe h
    exact a_pinned_certificate_targets_exactly_its_anchor s fringe h

end JoinInputs
end Sync
end Rchain
