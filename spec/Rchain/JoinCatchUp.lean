import Rchain.Liveness

/-!
# Law 121 — the catch-up obligation: a joined node is never `running` while it is behind

The RC-4 drill's light join restores the certified state and then, at `84d43d8fe`, the victim stopped
advancing: tip 183 while the other validators ran to 306. Its `/health` read `running`, its pipeline
was clean and its attestation was licensed. Nothing was false and nothing was named: a live node can
be frozen, and no reader could see it.

**The pre-fix predicate was the defect, and it failed in both directions.** It compared the highest
latest message the node held with the quorum-supported height over *those same messages*. Both are
read from the node's validated state, so a node that has validated nothing above height 198 reads
`own - supported = 0` and declares itself current while its peers are at 335 (the drill's silent
freeze). The same predicate, used as a *global* proposer gate, froze a healthy network on a six-height
skew: the leaders' quorum-supported height sat at the laggards' height, so the leaders read stale and
stopped proposing (drill run `565c8dec6`, removed in `5524f2b25`).

The fix is structural and self-relative. `running` carries a proof that the node's **own validated
tip** is within the catch-up window of the **peer tip it has observed**. `supported` is retained as a
diagnostic coordinate, not as the test. A view outside the window is `catchingUp supported own`, a
value that names both coordinates; there is no third constructor, so "running and behind" is not
writable. The watchdog is the safe fallback: a catch-up that observes no progress re-enters `blocked`,
which is the launch's fail-closed rung.

The continuous re-entry and the proposer gate are Law 123 (`spec/Rchain/JoinContinuous.lean`); this
row keeps the boot-time launcher and the phase's soundness. The Rust side is
`casper/src/engine/node_phase.rs` and the launcher's phase decision; the HAZOP is
`spec/audit/evidence/rc4-join-catch-up/hazop.md`.
-/

namespace Rchain
namespace JoinCatchUp

/-- Heights are numbers in the model. -/
abbrev Height := Nat

/-- The catch-up window — the same value the Rust `LIVENESS_WINDOW` carries. -/
def catchUpWindow : Height := 5

/-- How many attempts a catch-up may observe without progress before the watchdog acts. -/
def watchdogAfter : Nat := 3

/-- What the node can see.

* `own` is the highest latest message **this node has validated** — the DAG's message state, never the
  metadata maximum (metadata counts blocks the node has not validated).
* `peerTip` is the highest block number the node has observed from a connected peer, from a bounded
  `BlockRange` probe answer. It is the coordinate the pre-fix model lacked.
* `supported` is the quorum-supported height over the node's validated latest messages. It is a
  diagnostic: a healthy leader's quorum height can sit at a laggard's height, so it is not a measure
  of the leader's own progress. -/
structure View where
  own : Height
  peerTip : Height
  supported : Height
  deriving DecidableEq, Repr

/-- **The view is current** when the node's own validated tip is within the window of the peer tip it
    has observed. -/
def withinWindow (v : View) : Prop := v.peerTip - v.own ≤ catchUpWindow

instance (v : View) : Decidable (withinWindow v) := by
  unfold withinWindow
  infer_instance

/-- **The pre-fix predicate**, kept so the defect is a theorem rather than prose: the gap between the
    node's validated tip and the quorum-supported height computed from the same stale messages. It
    reads a frozen node as current and a healthy leader as stale. -/
def withinWindowUnchecked (v : View) : Prop := v.own - v.supported ≤ catchUpWindow

instance (v : View) : Decidable (withinWindowUnchecked v) := by
  unfold withinWindowUnchecked
  infer_instance

/-- The phase. `running` can only be constructed for a current view; a live, non-running phase names
    the coordinates a reader compares. `blocked` is the fail-closed rung. -/
inductive Phase where
  | starting
  | syncing (anchor : Height)
  | catchingUp (supported own : Height)
  | running (v : View) (h : withinWindow v)
  | blocked

/-- Whether this phase is the serving one. -/
def Phase.isRunning : Phase → Bool
  | .running _ _ => true
  | _ => false

/-- **The blocker coordinate** every live non-running phase names: the node's own validated tip. A
    reader with this and the peer tip can say whether the node is progressing; `none` is reserved for
    `running` (nothing to block), `starting` (launch is deciding), and `blocked` (the named failure is
    the phase itself). -/
def Phase.blocker : Phase → Option Height
  | .starting => none
  | .syncing anchor => some anchor
  | .catchingUp _ own => some own
  | .running _ _ => none
  | .blocked => none

/-- **The only launcher of the serving phase.** It cannot produce `running` for a stale view: the
    stale arm is `catchingUp` with its supported height and own tip, and the proof cannot be forged. -/
def afterRestore (v : View) : Phase :=
  if h : withinWindow v then .running v h else .catchingUp v.supported v.own

/-- **The watchdog.** A catch-up that has observed more than `watchdogAfter` attempts without progress
    fails closed; a running phase is never touched (continuity is Law 123). -/
def watchdog (p : Phase) (idle : Nat) : Phase :=
  match p with
  | .catchingUp _ _ => if idle > watchdogAfter then .blocked else p
  | other => other

/-- **Law 121's statement.** The launcher never serves a stale view; a stale view is a catch-up that
    names its coordinates; the watchdog fires only from a catch-up and only to blocked; and a running
    phase carries a current view. -/
def law121_statement : Prop :=
  (∀ (v : View), (afterRestore v).isRunning = true → withinWindow v) ∧
  (∀ (v : View), ¬ withinWindow v → afterRestore v = Phase.catchingUp v.supported v.own) ∧
  (∀ (s o : Height), Phase.blocker (Phase.catchingUp s o) = some o) ∧
  (∀ (s o : Height) (idle : Nat), idle > watchdogAfter →
      watchdog (Phase.catchingUp s o) idle = Phase.blocked) ∧
  (∀ (s o : Height) (idle : Nat), idle ≤ watchdogAfter →
      watchdog (Phase.catchingUp s o) idle = Phase.catchingUp s o) ∧
  (∀ (v : View) (h : withinWindow v) (idle : Nat),
      watchdog (Phase.running v h) idle = Phase.running v h)

theorem law121 : law121_statement := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro v h
    by_contra hw
    simp [afterRestore, hw, Phase.isRunning] at h
  · intro v hw
    simp [afterRestore, hw]
  · intro s o
    rfl
  · intro s o idle h
    simp [watchdog, h]
  · intro s o idle h
    have hnot : ¬ idle > watchdogAfter := by omega
    simp [watchdog, hnot]
  · intro v h _
    simp [watchdog]

/-! ### The falsifier, in both directions

The pre-fix predicate is not merely weak; it is wrong in two directions. The drill's frozen node read
current; the healthy leader on a six-height skew read stale. Both are theorems. -/

/-- The frozen node: validated tip 198, peer tip 335, quorum-supported 198 (the nodes it has validated
    all say 198). The pre-fix predicate says current; the law says stale. -/
theorem the_pre_fix_predicate_runs_the_frozen_node :
    ∃ v : View, withinWindowUnchecked v ∧ ¬ withinWindow v := by
  refine ⟨{ own := 198, peerTip := 335, supported := 198 }, ?_, ?_⟩
  · decide
  · decide

/-- The healthy leader on the removed global gate's six-height skew: its own tip and the peer tip are
    both 207, so it is current; the quorum-supported height is 201 because the laggards drag it down.
    The pre-fix predicate would freeze it. -/
theorem the_pre_fix_predicate_freezes_the_healthy_leader :
    ∃ v : View, withinWindow v ∧ ¬ withinWindowUnchecked v := by
  refine ⟨{ own := 207, peerTip := 207, supported := 201 }, ?_, ?_⟩
  · decide
  · decide

/-- The phase without the running proof: the pre-fix type, which could not express the distinction. -/
inductive LoosePhase where
  | starting
  | syncing (anchor : Height)
  | catchingUp (supported own : Height)
  | running
  | blocked

/-- The pre-fix launcher: running whenever the state was restored. -/
def afterRestoreUnchecked (_v : View) : LoosePhase := .running

/-- The mutant: a view outside the window is served. -/
def law121_mutant : Prop :=
  ∃ (v : View), ¬ withinWindow v ∧ afterRestoreUnchecked v = LoosePhase.running

/-- The falsifier: the drill's own view — validated tip 198, peer tip 335, quorum-supported 198. -/
theorem law121_falsifier : law121_mutant := by
  refine ⟨{ own := 198, peerTip := 335, supported := 198 }, ?_, rfl⟩
  simp [withinWindow, catchUpWindow]

end JoinCatchUp
end Rchain
