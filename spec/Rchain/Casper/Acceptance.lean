/-!
# C357's section-6 acceptance — the boundary shapes as a checked table

C357's remaining external obligation is section 6: a selected release with every validator
activating in lockstep, or a coordinated new genesis on unswept history; an independently accepted
retained-history sweep for an existing chain; and an independent category-A verdict. A mixed-version
rollout is refused. The incident's original rejected bytes were not preserved; the owner's decision
(2026-11-17) is that a source-pinned reconstructed trace plus the independent reviewer's explicit
acceptance of the reconstruction is the close condition, with the original bytes recorded as a
residual.

**The model.** `AcceptanceRecord` is the record the checker reads; `acceptanceValid` is the
priority-free conjunction the contract requires. The table is emitted to
`spec/audit/c357-acceptance.tsv` and the checker reads it, so the Python cannot invent a requirement
the Lean does not carry.

**What it does not claim.** The Bools are the record's claims; the tool checks the identities,
digests and URLs behind them. The model does not prove that a campaign ran, that a reviewer was
independent in fact, or that the selected binary is safe. Those are the external artefacts.
-/

namespace Rchain.Acceptance

/-- The two boundary shapes the contract admits. There is no third. -/
inductive BoundaryShape where
  | coordinatedNewGenesis
  | retainedHistorySweep
deriving DecidableEq, Repr

def BoundaryShape.label : BoundaryShape → String
  | .coordinatedNewGenesis => "coordinated-new-genesis"
  | .retainedHistorySweep => "retained-history-sweep"

/-- The record the C357 checker validates. -/
structure AcceptanceRecord where
  shape : Option BoundaryShape
  sweepPresent : Bool
  sweepComplete : Bool
  lockstep : Bool
  mixedVersion : Bool
  releaseSelected : Bool
  reviewerIndependent : Bool
  verdictAccepted : Bool
  campaignDigest : Bool
  incidentTracePresent : Bool
  incidentTraceOriginal : Bool
  ownerAcceptedReconstruction : Bool
  reviewerAcceptedReconstruction : Bool
deriving DecidableEq, Repr

/-- The requirements common to both shapes. The incident trace may be the original bytes, or a
reconstruction the owner **and** the independent reviewer both accepted. -/
def commonValid (r : AcceptanceRecord) : Bool :=
  r.campaignDigest && r.releaseSelected && r.lockstep && !r.mixedVersion
    && r.reviewerIndependent && r.verdictAccepted && r.incidentTracePresent
    && (r.incidentTraceOriginal
      || (r.ownerAcceptedReconstruction && r.reviewerAcceptedReconstruction))

/-- **The contract.** Exactly one shape; a new genesis carries no sweep; a retained history carries a
complete sweep. -/
def acceptanceValid (r : AcceptanceRecord) : Bool :=
  match r.shape with
  | none => false
  | some .coordinatedNewGenesis => commonValid r && !r.sweepPresent
  | some .retainedHistorySweep => commonValid r && r.sweepPresent && r.sweepComplete

theorem exactly_one_boundary_shape (r : AcceptanceRecord) (h : acceptanceValid r = true) :
    r.shape = some .coordinatedNewGenesis ∨ r.shape = some .retainedHistorySweep := by
  unfold acceptanceValid at h
  cases hs : r.shape with
  | none => simp [hs] at h
  | some s => cases s <;> simp [hs]

theorem a_mixed_version_rollout_is_refused (r : AcceptanceRecord) (h : r.mixedVersion = true) :
    acceptanceValid r = false := by
  unfold acceptanceValid
  cases r.shape with
  | none => simp
  | some s => cases s <;> simp [commonValid, h]

theorem a_missing_independent_review_is_refused (r : AcceptanceRecord)
    (h : r.reviewerIndependent = false) : acceptanceValid r = false := by
  unfold acceptanceValid
  cases r.shape with
  | none => simp
  | some s => cases s <;> simp [commonValid, h]

theorem an_unselected_candidate_is_refused (r : AcceptanceRecord)
    (h : r.releaseSelected = false) : acceptanceValid r = false := by
  unfold acceptanceValid
  cases r.shape with
  | none => simp
  | some s => cases s <;> simp [commonValid, h]

theorem a_new_genesis_with_a_sweep_is_refused (r : AcceptanceRecord)
    (hs : r.shape = some .coordinatedNewGenesis) (h : r.sweepPresent = true) :
    acceptanceValid r = false := by
  simp [acceptanceValid, hs, h]

theorem a_retained_history_without_a_complete_sweep_is_refused (r : AcceptanceRecord)
    (hs : r.shape = some .retainedHistorySweep) (h : r.sweepComplete = false) :
    acceptanceValid r = false := by
  simp [acceptanceValid, hs, h]

theorem a_reconstructed_trace_without_owner_acceptance_is_refused (r : AcceptanceRecord)
    (ho : r.incidentTraceOriginal = false) (ha : r.ownerAcceptedReconstruction = false) :
    acceptanceValid r = false := by
  unfold acceptanceValid
  cases r.shape with
  | none => simp
  | some s => cases s <;> simp [commonValid, ho, ha]

theorem a_reconstructed_trace_without_reviewer_acceptance_is_refused (r : AcceptanceRecord)
    (ho : r.incidentTraceOriginal = false) (ha : r.reviewerAcceptedReconstruction = false) :
    acceptanceValid r = false := by
  unfold acceptanceValid
  cases r.shape with
  | none => simp
  | some s => cases s <;> simp [commonValid, ho, ha]

theorem the_campaign_must_be_digest_pinned (r : AcceptanceRecord)
    (h : r.campaignDigest = false) : acceptanceValid r = false := by
  unfold acceptanceValid
  cases r.shape with
  | none => simp
  | some s => cases s <;> simp [commonValid, h]

/-- The emitted table: the shape and the requirements the checker enforces. The strings are the
checker's field names, not a second prose contract. -/
structure AcceptanceRule where
  shape : BoundaryShape
  requirements : List String
deriving Repr

def acceptanceTable : List AcceptanceRule :=
  [ { shape := .coordinatedNewGenesis,
      requirements := ["campaignDigest", "releaseSelected", "lockstep", "noMixedVersion",
        "noSweep", "reviewerIndependent", "verdictAccepted", "incidentTrace"] },
    { shape := .retainedHistorySweep,
      requirements := ["campaignDigest", "releaseSelected", "lockstep", "noMixedVersion",
        "sweepPresent", "sweepComplete", "reviewerIndependent", "verdictAccepted",
        "incidentTrace"] } ]

def validNewGenesis : AcceptanceRecord :=
  { shape := some .coordinatedNewGenesis, sweepPresent := false, sweepComplete := false,
    lockstep := true, mixedVersion := false, releaseSelected := true, reviewerIndependent := true,
    verdictAccepted := true, campaignDigest := true, incidentTracePresent := true,
    incidentTraceOriginal := true, ownerAcceptedReconstruction := false,
    reviewerAcceptedReconstruction := false }

theorem a_valid_new_genesis_is_accepted : acceptanceValid validNewGenesis = true := by
  decide

def validRetainedSweep : AcceptanceRecord :=
  { validNewGenesis with shape := some .retainedHistorySweep, sweepPresent := true, sweepComplete := true }

theorem a_valid_retained_sweep_is_accepted : acceptanceValid validRetainedSweep = true := by
  decide

def reconstructedSweep : AcceptanceRecord :=
  { validRetainedSweep with incidentTraceOriginal := false, ownerAcceptedReconstruction := true, reviewerAcceptedReconstruction := true }

theorem a_reconstructed_trace_with_both_acceptances_is_accepted :
    acceptanceValid reconstructedSweep = true := by decide

end Rchain.Acceptance
