import Rchain.Liveness

/-!
# Law 121 — the catch-up obligation: a joined node is never `running` while it is behind

The RC-4 drill's light join restores the certified state and then, at `84d43d8fe`, the victim stopped
advancing: tip 183 while the other validators ran to 306. Its `/health` read `running`, its pipeline
was clean and its attestation was licensed. Nothing was false and nothing was named: a live node can
be frozen, and no reader could see it.

The fix is structural. **`running` carries a proof that the node's quorum-supported height is within
the catch-up window of the highest latest message it holds.** A view outside the window is
`catchingUp supported`, a value that names the coordinate a reader compares with the peer tip. There
is no third constructor, so the silent state — running and behind — is not writable (the mutant
below cannot even be expressed over the typed phase; it is expressed over the loose phase the
pre-fix launcher used).

The watchdog is the safe fallback: a catch-up that observes `watchdogAfter` attempts without progress
re-enters `syncing`, which is the node's existing checkpoint-sync path (rewind and re-adopt a newer
certified anchor). Timing is measured in observed attempts, not wall clock, so it is deterministic
and testable.

The Rust side is `casper/src/engine/node_phase.rs` and the launcher's phase decision; the HAZOP is
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

/-- What the node can see of the network's messages. `peerTip` is the highest latest message among
    the active set; `supported` is the quorum-supported height over those messages. Both come from
    the message state, never from the DAG's metadata maximum (a metadata tip counts blocks the node
    has not validated and would hold every proposer closed — measured 2026-11-21). -/
structure View where
  peerTip : Height
  supported : Height

/-- **The view is current** when the quorum-supported height is within the window of the highest
    latest message. -/
def withinWindow (v : View) : Prop := v.peerTip - v.supported ≤ catchUpWindow

instance (v : View) : Decidable (withinWindow v) := by
  unfold withinWindow
  infer_instance

/-- The phase. `running` can only be constructed for a current view; a live, non-running phase names
    the height a reader compares with the peer tip. -/
inductive Phase where
  | starting
  | syncing (anchor : Height)
  | catchingUp (supported : Height)
  | running (v : View) (h : withinWindow v)

/-- Whether this phase is the serving one. -/
def Phase.isRunning : Phase → Bool
  | .running _ _ => true
  | _ => false

/-- **The blocker coordinate** every live non-running phase names. A reader with this and the peer
    tip can say whether the node is progressing; `none` is reserved for `running` (nothing to
    block) and `starting` (launch is deciding, and it is bounded by the launch's own timeout). -/
def Phase.blocker : Phase → Option Height
  | .starting => none
  | .syncing anchor => some anchor
  | .catchingUp supported => some supported
  | .running _ _ => none

/-- **The only launcher of the serving phase.** It cannot produce `running` for a stale view: the
    stale arm is `catchingUp` with its supported height, and the proof cannot be forged. -/
def afterRestore (v : View) : Phase :=
  if h : withinWindow v then .running v h else .catchingUp v.supported

/-- **The watchdog.** A catch-up that has observed more than `watchdogAfter` attempts without
    progress re-enters `syncing`; a running phase is never touched. -/
def watchdog (p : Phase) (idle : Nat) : Phase :=
  match p with
  | .catchingUp _ => if idle > watchdogAfter then .syncing 0 else p
  | other => other

/-- **Law 121's statement.** The launcher never serves a stale view; a stale view is a catch-up
    that names its supported height; the watchdog fires only from a catch-up and only to sync; and a
    running phase is immutable under it. -/
def law121_statement : Prop :=
  (∀ (v : View), (afterRestore v).isRunning = true → withinWindow v) ∧
  (∀ (v : View), ¬ withinWindow v → afterRestore v = Phase.catchingUp v.supported) ∧
  (∀ (s : Height), Phase.blocker (Phase.catchingUp s) = some s) ∧
  (∀ (s : Height) (idle : Nat), idle > watchdogAfter →
      watchdog (Phase.catchingUp s) idle = Phase.syncing 0) ∧
  (∀ (s : Height) (idle : Nat), idle ≤ watchdogAfter →
      watchdog (Phase.catchingUp s) idle = Phase.catchingUp s) ∧
  (∀ (v : View) (h : withinWindow v) (idle : Nat),
      watchdog (Phase.running v h) idle = Phase.running v h)

theorem law121 : law121_statement := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro v h
    by_contra hw
    simp [afterRestore, hw, Phase.isRunning] at h
  · intro v hw
    simp [afterRestore, hw]
  · intro s
    rfl
  · intro s idle h
    simp [watchdog, h]
  · intro s idle h
    have hnot : ¬ idle > watchdogAfter := by omega
    simp [watchdog, hnot]
  · intro v h idle
    simp [watchdog]

/-! ### The falsifier

The pre-fix launcher returned `running` for whatever the sync had restored. Over the typed phase that
value is unwritable for a stale view, so the mutant is stated over a loose phase — the same shape the
Rust enum had before this law. -/

/-- The phase without the running proof: the pre-fix type. -/
inductive LoosePhase where
  | starting
  | syncing (anchor : Height)
  | catchingUp (supported : Height)
  | running

/-- The pre-fix launcher: running whenever the state was restored. -/
def afterRestoreUnchecked (_v : View) : LoosePhase := .running

/-- The mutant: a view outside the window is served. -/
def law121_mutant : Prop :=
  ∃ (v : View), ¬ withinWindow v ∧ afterRestoreUnchecked v = LoosePhase.running

/-- The falsifier: the drill's own view — highest latest message 306, quorum stuck at 181. -/
theorem law121_falsifier : law121_mutant := by
  refine ⟨{ peerTip := 306, supported := 181 }, ?_, rfl⟩
  simp [withinWindow, catchUpWindow]

end JoinCatchUp
end Rchain
