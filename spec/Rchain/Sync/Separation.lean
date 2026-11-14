import Mathlib.Logic.Function.Basic

/-!
# Law 105 — sync and verification are separate

**The change this law covers.** Sync used to accept a state by downloading every body and replaying
it (`RecoveryReplay::verify` on the sync path). The redesign accepts a **checkpoint certificate** and
builds the DAG from headers (`casper/src/engine/node_syncing.rs::run_approved_state_sync`); replay
moves behind `rnode audit` (`node/src/runtime/audit.rs`).

**What this law says.** Acceptance is a function of the certificate, the metadata and the checkpoint
state only — the replay input cannot change it. A failed audit is an **alarm on an accepted state**,
not a rewind: it never un-accepts what the quorum certified.

**What it does not claim.** It does not claim the certificate is quorate (Law 14a) or honest (Law
83/85). It does not model the audit's equivalence rule, which is Law 106. Operator policy may stop a
node on a disagreement — that is a decision above this model, and the model says the *default* is to
stay accepted and alarmed.

**Why it is not vacuous.** Feed the replay input into `accept` and
`replay_cannot_change_acceptance` fails; make a failed audit yield `rewound` and
`a_failed_audit_is_an_alarm_not_a_rewind` fails.
-/

namespace Rchain

namespace Sync
namespace Separation

/-- The acceptance inputs: the certificate's quorum result, the metadata and the checkpoint state. -/
structure Certificate where
  quorate : Bool

/-- One block header as the DAG stores it (the metadata path has no body). -/
structure Metadata where
  height : Nat

/-- **Sync's acceptance**: the certificate, whether the metadata walk completed, and the checkpoint
    state. The replay (`_replay`) is an input of the *audit*, not of this function. -/
def accept (c : Certificate) (_m : Metadata) (_state : Nat) (_replay : Nat) : Bool :=
  c.quorate

/-- **Law 105a — sync reads only the checkpoint and the metadata.** -/
theorem sync_reads_only_the_checkpoint_and_metadata (c : Certificate) (m : Metadata)
    (state replay : Nat) : accept c m state replay = c.quorate :=
  rfl

/-- **Law 105b — replay cannot change acceptance.** Two audit inputs give the same acceptance. -/
theorem replay_cannot_change_acceptance (c : Certificate) (m : Metadata) (state r1 r2 : Nat) :
    accept c m state r1 = accept c m state r2 :=
  rfl

/-- What the node does: the checkpoint is accepted, and the audit reports. -/
inductive Outcome
  | accepted
  | alarmed
  | rewound

/-- A failed audit on an accepted state leaves the state accepted and raises an alarm. `rewound` is
    not reachable by this function: an audit has no authority over acceptance. -/
def after_audit (accepted : Bool) (auditOk : Bool) : Outcome :=
  if accepted then
    if auditOk then Outcome.accepted else Outcome.alarmed
  else
    Outcome.alarmed

/-- **Law 105c — a failed audit is an alarm, not a rewind.** -/
theorem a_failed_audit_is_an_alarm_not_a_rewind (auditOk : Bool) (h : auditOk = false) :
    after_audit true auditOk = Outcome.alarmed := by
  simp [after_audit, h]

/-- Non-vacuity of 105c: a *passing* audit on an accepted state is `accepted`, so the equality above
    is about the failure case rather than about a function that always alarms. -/
theorem a_passing_audit_keeps_the_accepted_state :
    after_audit true true = Outcome.accepted := by
  simp [after_audit]

end Separation
end Sync

end Rchain
