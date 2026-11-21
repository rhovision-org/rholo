import Rchain.JoinCatchUp

/-!
# Law 123 — catch-up is continuous, and only a current view proposes

Law 121 made the boot-time launcher sound: `running` carries a proof that the node's own validated tip
is within the window of the observed peer tip. The drill then measured the remaining edge: the
watchdog **disarmed at `Running`**. A joined validator finished its walk at 198 while the network ran
to 335; nothing re-entered `CatchingUp`, and nothing restarted the walk (`062b4140c`, `87b9aedf2`,
`e3f5195c9`). The phase was correct at boot and unobserved afterwards.

This module is the observation. A supervisor samples the same `View` on every tick and applies one
rule: a `running` phase whose view leaves the window re-enters `catchingUp` after `enterAfter`
consecutive stale observations; a `catchingUp` phase whose view returns runs again. The proposer does
not read a network-wide window — it reads this shard's phase, and the phase can only be `running`
with a current view. A healthy leader on a lagging quorum is current (Law 121's second falsifier), so
the observation does not freeze it: the global gate's failure is structurally excluded, not merely
avoided.

The mutant is the pre-fix one-shot watchdog (`preFixWatchdog`), which stays `running` for ever.
-/

namespace Rchain
namespace JoinContinuous

open Rchain.JoinCatchUp

/-- How many consecutive stale observations confirm a regression. A single sample is not a phase
    change; the Rust carries the same constant. -/
def enterAfter : Nat := 2

/-- **The supervisor's step.** It is a pure function of the phase, the observed view, and the run of
    consecutive stale observations. `staleTicks` is managed by the caller.

    * `running` + current → `running` (the proof is re-derived for the observed view);
    * `running` + stale, confirmed → `catchingUp` at the observed coordinates;
    * `running` + stale, unconfirmed → the phase is unchanged (bounded hysteresis);
    * `catchingUp` + current → `running`;
    * every other phase → unchanged. -/
def supervise (p : Phase) (v : View) (staleTicks : Nat) : Phase :=
  match p with
  | .running v0 h0 =>
      if h : withinWindow v then
        .running v h
      else if staleTicks ≥ enterAfter then
        .catchingUp v.supported v.own
      else
        .running v0 h0
  | .catchingUp s o =>
      if h : withinWindow v then .running v h else .catchingUp s o
  | other => other

/-- **The proposer's question is the phase, not a window.** A shard proposes exactly when it is
    `running`; the Rust asks the same question in `proposer_instance.rs`. -/
def mayPropose (p : Phase) : Bool := p.isRunning

/-- A running phase carries a current view — the question `mayPropose` answers is sound. -/
theorem a_running_phase_carries_a_current_view (p : Phase) (h : p.isRunning = true) :
    ∃ (v : View) (hv : withinWindow v), p = Phase.running v hv := by
  cases p with
  | starting => simp [Phase.isRunning] at h
  | syncing _ => simp [Phase.isRunning] at h
  | catchingUp _ _ => simp [Phase.isRunning] at h
  | running v hv => exact ⟨v, hv, rfl⟩
  | blocked => simp [Phase.isRunning] at h

/-- **The regression.** A confirmed stale observation takes a running phase out of service. -/
theorem a_stale_observation_reenters (v0 : View) (h0 : withinWindow v0) (v : View)
    (h : ¬ withinWindow v) (n : Nat) (hn : enterAfter ≤ n) :
    (supervise (Phase.running v0 h0) v n).isRunning = false := by
  simp [supervise, h, hn, Phase.isRunning]

/-- **The hysteresis is bounded, not a silence.** Before the confirmation count is reached the phase
    stays running; the bound is exactly `enterAfter`. -/
theorem the_hysteresis_is_bounded (v0 : View) (h0 : withinWindow v0) (v : View)
    (n : Nat) (hn : n < enterAfter) :
    (supervise (Phase.running v0 h0) v n).isRunning = true := by
  by_cases h : withinWindow v
  · simp [supervise, h, Phase.isRunning]
  · have hge : ¬ enterAfter ≤ n := Nat.not_le.mpr hn
    simp [supervise, h, hge, Phase.isRunning]

/-- **Recovery.** A current observation returns a catching-up phase to service. -/
theorem a_current_observation_restores (s o : Nat) (v : View) (h : withinWindow v) (n : Nat) :
    (supervise (Phase.catchingUp s o) v n).isRunning = true := by
  simp [supervise, h, Phase.isRunning]

/-- A current observation renews the running phase's proof for the observed view. -/
theorem a_current_observation_is_served (v0 : View) (h0 : withinWindow v0) (v : View)
    (h : withinWindow v) (n : Nat) :
    supervise (Phase.running v0 h0) v n = Phase.running v h := by
  simp [supervise, h]

/-- **A blocked phase is terminal.** The supervisor never revives it: the fail-closed rung is a stop,
    not another loop. -/
theorem blocked_is_terminal (v : View) (n : Nat) : supervise Phase.blocked v n = Phase.blocked := rfl

/-- **May-propose soundness.** Whatever the supervisor returns, a proposal can only be made on a
    current view. -/
theorem may_propose_is_sound (p : Phase) (v : View) (n : Nat)
    (h : (supervise p v n).isRunning = true) :
    ∃ (v' : View) (hv : withinWindow v'), supervise p v n = Phase.running v' hv :=
  a_running_phase_carries_a_current_view (supervise p v n) h

/-- **Law 123's statement.** A stale observation re-enters catch-up; the hysteresis is bounded; a
    current observation restores the serving phase; a blocked phase is terminal; and a proposing phase
    carries a current view. -/
def law123_statement : Prop :=
  (∀ (v0 : View) (h0 : withinWindow v0) (v : View), ¬ withinWindow v →
      ∀ n, enterAfter ≤ n → (supervise (Phase.running v0 h0) v n).isRunning = false) ∧
  (∀ (v0 : View) (h0 : withinWindow v0) (v : View) (n : Nat), n < enterAfter →
      (supervise (Phase.running v0 h0) v n).isRunning = true) ∧
  (∀ (s o : Nat) (v : View), withinWindow v →
      ∀ n, (supervise (Phase.catchingUp s o) v n).isRunning = true) ∧
  (∀ (v : View) (n : Nat), supervise Phase.blocked v n = Phase.blocked) ∧
  (∀ (p : Phase) (v : View) (n : Nat), (supervise p v n).isRunning = true →
      ∃ (v' : View) (hv : withinWindow v'), supervise p v n = Phase.running v' hv)

theorem law123 : law123_statement := by
  refine ⟨?_, ?_, ?_, ?_, ?_⟩
  · intro v0 h0 v h n hn
    exact a_stale_observation_reenters v0 h0 v h n hn
  · intro v0 h0 v n hn
    exact the_hysteresis_is_bounded v0 h0 v n hn
  · intro s o v h n
    exact a_current_observation_restores s o v h n
  · intro v n
    exact blocked_is_terminal v n
  · intro p v n h
    exact may_propose_is_sound p v n h

/-! ### The falsifier

The pre-fix watchdog observed once, at boot, and never again: `preFixWatchdog` returns the loose
`running` phase for any view and any number of ticks. The drill's frozen view is the witness. -/

/-- The pre-fix supervisor: the phase was set once by the launcher and never observed again. -/
def preFixWatchdog (_v : View) (_ticks : Nat) : LoosePhase := .running

/-- The mutant: a view outside the window stays `running` under the pre-fix watchdog. -/
def law123_mutant : Prop :=
  ∃ (v : View) (n : Nat),
    ¬ withinWindow v ∧ preFixWatchdog v n = LoosePhase.running

/-- The falsifier: validated tip 198, peer tip 335 — the drill's frozen node. -/
theorem law123_falsifier : law123_mutant := by
  refine ⟨{ own := 198, peerTip := 335, supported := 198 }, 1, ?_, rfl⟩
  simp [withinWindow, catchUpWindow]

end JoinContinuous
end Rchain
