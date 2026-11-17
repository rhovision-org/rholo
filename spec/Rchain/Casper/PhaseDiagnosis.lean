import Rchain.Disposition

/-!
# C343's phase diagnosis — the operator's read-only snapshot as a checked table

The C343 close-out owes a read-only snapshot of hosts B/C/D: `/health`, the operator status and the
block-6 journal lines. The snapshot is only evidence if its verdict is a **checked** classification,
not a reader's impression. This module is that classification.

**The discriminator.** The incident has two shapes the operator must tell apart: a host that is
`Syncing` and restoring a chain it is not serving (the node is catching up; its stored bodies are
ahead of its DAG), and a host whose launch cell is `Running` while the processor owes a durable
verdict (the fail-closed `Blocked` phase of Law 88a). A proposer that is running but isolated is a
third, and the healthy snapshot is a fourth. Anything else is `unmodelled` and is **not resolved** —
the verifier reports `PRECONDITION`, never a pass.

**The table is the definition.** `diagnose` is a first-match walk over `phaseDiagnosisTable`; the
table is emitted to `spec/audit/phase-diagnosis.tsv` and the verifier reads it. The witness cases
below pin the four named shapes and the unmodelled fallback.

**What it does not claim.** It classifies the observed fields; it does not prove the field values,
the Rust's sampling, or that a host answered. The collector and the read-only guarantee are the
tool's job. The incident's *fix* is Law 88a's audit/readiness model, not this table.
-/

namespace Rchain.PhaseDiagnosis

open Rchain.Disposition

/-- The fields the C343-LIVE contract collects. The tool flattens `/health` and the operator status
into this shape before calling the table. -/
structure PhaseObservation where
  healthPhase : Phase
  syncPhase : Option Phase
  syncActive : Bool
  resetMarker : Bool
  pipelineFaultsOutstanding : Nat
  proposalHalted : Bool
  finalityStalled : Bool
  heightGap : Nat
  shardPhases : List Phase
  orphanBodies : Nat
  ready : Bool
  readyReasons : List String
deriving DecidableEq, Repr

/-- The named outcomes. `unmodelled` is the honest residual. -/
inductive Diagnosis where
  | syncingRecovery
  | processorFaultBlocked
  | proposalIsolation
  | healthy
  | unmodelled
deriving DecidableEq, Repr

def Diagnosis.resolved : Diagnosis → Bool
  | .unmodelled => false
  | _ => true

def Diagnosis.label : Diagnosis → String
  | .syncingRecovery => "syncing-recovery"
  | .processorFaultBlocked => "processor-fault-blocked"
  | .proposalIsolation => "proposal-isolation"
  | .healthy => "healthy"
  | .unmodelled => "unmodelled"

/-- The predicates the table is written in. `anySyncing` is derived from its four components;
`noSyncing` from `anySyncing`. -/
inductive Condition where
  | faultOutstanding
  | phaseBlocked
  | proposalHalted
  | phaseSyncing
  | syncPhaseSyncing
  | syncActive
  | resetMarker
  | anySyncing
  | ready
  | readyReasonsEmpty
  | allShardsRunning
  | finalityStalled
  | noFaultOutstanding
  | noProposalHalted
  | noSyncing
  | notFinalityStalled
deriving DecidableEq, Repr

def Condition.label : Condition → String
  | .faultOutstanding => "fault-outstanding"
  | .phaseBlocked => "phase-blocked"
  | .proposalHalted => "proposal-halted"
  | .phaseSyncing => "phase-syncing"
  | .syncPhaseSyncing => "sync-phase-syncing"
  | .syncActive => "sync-active"
  | .resetMarker => "reset-marker"
  | .anySyncing => "any-syncing"
  | .ready => "ready"
  | .readyReasonsEmpty => "ready-reasons-empty"
  | .allShardsRunning => "all-shards-running"
  | .finalityStalled => "finality-stalled"
  | .noFaultOutstanding => "no-fault-outstanding"
  | .noProposalHalted => "no-proposal-halted"
  | .noSyncing => "no-syncing"
  | .notFinalityStalled => "not-finality-stalled"

/-- The any-of the syncing rows use. -/
def anySyncing (o : PhaseObservation) : Bool :=
  decide (o.healthPhase = .syncing) || decide (o.syncPhase = some .syncing)
    || o.syncActive || o.resetMarker

def holds : Condition → PhaseObservation → Bool
  | .faultOutstanding, o => decide (o.pipelineFaultsOutstanding > 0)
  | .phaseBlocked, o => decide (o.healthPhase = .blocked)
  | .proposalHalted, o => o.proposalHalted
  | .phaseSyncing, o => decide (o.healthPhase = .syncing)
  | .syncPhaseSyncing, o => decide (o.syncPhase = some .syncing)
  | .syncActive, o => o.syncActive
  | .resetMarker, o => o.resetMarker
  | .anySyncing, o => anySyncing o
  | .ready, o => o.ready
  | .readyReasonsEmpty, o => decide (o.readyReasons = [])
  | .allShardsRunning, o => o.shardPhases.all (fun p => decide (p = .running))
  | .finalityStalled, o => o.finalityStalled
  | .noFaultOutstanding, o => decide (o.pipelineFaultsOutstanding = 0)
  | .noProposalHalted, o => decide (o.proposalHalted = false)
  | .noSyncing, o => !(anySyncing o)
  | .notFinalityStalled, o => !o.finalityStalled

structure DiagnosisRow where
  priority : Nat
  diagnosis : Diagnosis
  conditions : List Condition
deriving DecidableEq, Repr

def rowMatches (o : PhaseObservation) (r : DiagnosisRow) : Bool :=
  r.conditions.all (fun c => holds c o)

/-- **The priority-ordered table.** A later row is reached only when every earlier row failed. -/
def phaseDiagnosisTable : List DiagnosisRow :=
  [ { priority := 1, diagnosis := .processorFaultBlocked,
      conditions := [.faultOutstanding, .phaseBlocked] },
    { priority := 2, diagnosis := .proposalIsolation,
      conditions := [.proposalHalted, .noFaultOutstanding] },
    { priority := 3, diagnosis := .syncingRecovery, conditions := [.anySyncing] },
    { priority := 4, diagnosis := .healthy,
      conditions := [.ready, .readyReasonsEmpty, .allShardsRunning, .noFaultOutstanding,
        .noProposalHalted, .noSyncing, .notFinalityStalled] } ]

def diagnose (o : PhaseObservation) : Diagnosis :=
  (phaseDiagnosisTable.find? (rowMatches o)).map (·.diagnosis) |>.getD .unmodelled

/-- **The consistency check** the verifier runs before the table: the ready bit agrees with the
reasons, the reported phase agrees with the fail-closed latch, and the operator `syncPhase` does not
contradict the health phase. An inconsistent observation is a **FAIL**, not a diagnosis. -/
def consistent (o : PhaseObservation) : Bool :=
  (o.ready == decide (o.readyReasons = []))
    && (decide (o.healthPhase = .blocked) == decide (o.pipelineFaultsOutstanding > 0))
    && (match o.syncPhase with
        | none => true
        | some p => decide (o.healthPhase = p) || decide (p = .syncing) || decide (p = .starting))

theorem diagnose_is_total (o : PhaseObservation) : ∃ d, diagnose o = d := ⟨_, rfl⟩

theorem diagnose_matches_its_row (o : PhaseObservation) (r : DiagnosisRow)
    (h : phaseDiagnosisTable.find? (rowMatches o) = some r) :
    diagnose o = r.diagnosis := by
  simp [diagnose, h]

theorem unmodelled_is_not_resolved : Diagnosis.resolved .unmodelled = false := rfl

theorem healthy_is_resolved : Diagnosis.resolved .healthy = true := rfl

/-- The healthy row, named so the consistency theorem can speak about it without an inline record
application. -/
def healthyRow : DiagnosisRow :=
  { priority := 4, diagnosis := .healthy,
    conditions := [.ready, .readyReasonsEmpty, .allShardsRunning, .noFaultOutstanding,
      .noProposalHalted, .noSyncing, .notFinalityStalled] }

theorem an_inconsistent_ready_bit_is_not_the_healthy_row (o : PhaseObservation)
    (h : o.ready = false) (hr : o.readyReasons = []) :
    rowMatches o healthyRow = false := by
  simp [rowMatches, holds, healthyRow, h, hr]

/-! ## The witness cases

Each is a concrete observation the emitted `phase-diagnosis-cases.tsv` names. -/

def incidentObservation : PhaseObservation :=
  { healthPhase := .running, syncPhase := some .running, syncActive := false, resetMarker := false,
    pipelineFaultsOutstanding := 0, proposalHalted := true, finalityStalled := true,
    heightGap := 32, shardPhases := [.running], orphanBodies := 0, ready := false,
    readyReasons := ["proposal_isolated"] }

theorem the_incident_is_proposal_isolation :
    diagnose incidentObservation = .proposalIsolation := by
  decide

def processorFaultObservation : PhaseObservation :=
  { incidentObservation with healthPhase := .blocked, proposalHalted := false, pipelineFaultsOutstanding := 2, ready := false, readyReasons := ["pipeline_fault_outstanding"] }

theorem a_processor_fault_is_blocked :
    diagnose processorFaultObservation = .processorFaultBlocked := by
  decide

def syncingObservation : PhaseObservation :=
  { incidentObservation with healthPhase := .syncing, syncPhase := some .syncing, proposalHalted := false, syncActive := true, finalityStalled := false, ready := false, readyReasons := ["shard_not_running"] }

theorem a_syncing_host_is_recovery :
    diagnose syncingObservation = .syncingRecovery := by
  decide

def healthyObservation : PhaseObservation :=
  { healthPhase := .running, syncPhase := some .running, syncActive := false, resetMarker := false,
    pipelineFaultsOutstanding := 0, proposalHalted := false, finalityStalled := false,
    heightGap := 1, shardPhases := [.running], orphanBodies := 0, ready := true,
    readyReasons := [] }

theorem a_healthy_snapshot_is_healthy :
    diagnose healthyObservation = .healthy := by
  decide

def unmodelledObservation : PhaseObservation :=
  { healthyObservation with finalityStalled := true, ready := false, readyReasons := ["unknown"] }

theorem an_unknown_shape_is_unmodelled :
    diagnose unmodelledObservation = .unmodelled := by
  decide

end Rchain.PhaseDiagnosis
