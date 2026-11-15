import Mathlib.Logic.Function.Basic

/-!
# Law 105 — sync and verification are separate

**The change this law covers.** Sync used to accept a state by downloading every body and replaying
it (`RecoveryReplay::verify` on the sync path). The redesign accepts a **checkpoint certificate** and
builds the DAG from headers (`casper/src/engine/node_syncing.rs::run_approved_state_sync`); replay
moves behind `rnode audit` (`node/src/runtime/audit.rs`).

**What this law says.** Acceptance is a function of the certificate, the metadata (including the
statement's commitment to it) and the checkpoint state only — the replay input cannot change it. A
failed audit is an **alarm on an accepted state**, not a rewind: it never un-accepts what the quorum
certified.

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

/-- The acceptance inputs: the certificate's quorum result and its metadata commitment. -/
structure Certificate where
  quorate : Bool
  metadata_root : Nat

/-- One block header as the DAG stores it (the metadata path has no body), plus the root its set
    derives. -/
structure Metadata where
  height : Nat
  committed_root : Nat

/-- **The certificate commits to the metadata the walk holds.** A statement without a commitment is
    refused on the sync path; a mismatch means the headers are not the set the quorum signed over. -/
def metadata_agrees (c : Certificate) (m : Metadata) : Bool :=
  c.metadata_root = m.committed_root

/-- **Sync's acceptance**: the certificate's quorum, the metadata commitment, and the checkpoint
    state. The replay (`_replay`) is an input of the *audit*, not of this function. -/
def accept (c : Certificate) (m : Metadata) (_state : Nat) (_replay : Nat) : Bool :=
  c.quorate && metadata_agrees c m

/-- **Law 105a — sync reads only the checkpoint and the metadata.** -/
theorem sync_reads_only_the_checkpoint_and_metadata (c : Certificate) (m : Metadata)
    (state replay : Nat) : accept c m state replay = (c.quorate && metadata_agrees c m) :=
  rfl

/-- **Law 105d — a metadata root that disagrees is not accepted.** -/
theorem sync_refuses_a_metadata_root_that_disagrees (c : Certificate) (m : Metadata)
    (state replay : Nat) (h : metadata_agrees c m = false) :
    accept c m state replay = false := by
  simp [accept, h]

/-- Non-vacuity of 105d: a quorate certificate whose metadata root agrees is accepted, so the
    refusal above is about the commitment rather than a function that always refuses. -/
theorem a_quorate_certificate_with_the_same_metadata_is_accepted (m : Metadata)
    (state replay : Nat) :
    accept { quorate := true, metadata_root := m.committed_root } m state replay = true := by
  simp [accept, metadata_agrees]

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
