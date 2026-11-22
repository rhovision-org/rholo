import Mathlib.Tactic

/-!
# Law 126 — settlement is finalized inclusion, and the lifecycle is total

The #487 false-settlement shape: a deploy was `ProcessedWithSuccess` in block 1781 while finality was
frozen at 1756, and a client that read the execution tag or the tip balance could treat the transfer
as settled. The #552 contract made the distinction explicit, and the endpoint
`GET /api/v1/deploy-settlement/{id}` is the only settlement statement
(`node/src/web/http.rs:1556-1659`). The register had no model of that surface; this module is the
model.

The table is small and load-bearing:

* `pooled` and `notProcessed` are the unexecuted states;
* `includedUnfinalized` and `finalityUnknown` say the deploy is in a block that is not (or cannot be
  shown to be) finalized;
* `finalizedSuccess` is the only stage with `settlementConfirmed = true`;
* `finalizedError` is a terminal execution failure, not successful settlement.

The law states the implication in both directions, the total/exclusive stage table, and the two
negative facts the #487 record depends on: execution success without finality is not settlement, and
a finalized error is not successful settlement. The mutant is the pre-#487 reading that treats any
processed deploy as settled; the falsifier is the incident's own pair — a processed success with
finality false.
-/

namespace Rchain
namespace DeploySettlement

/-- The execution view the endpoint receives: not processed, or a processed success/error. -/
inductive Exec where
  | notProcessed
  | success
  | error
  deriving DecidableEq, Fintype

/-- The settlement stages of the #552 contract. -/
inductive Stage where
  | notProcessed
  | pooled
  | includedUnfinalized
  | finalityUnknown
  | finalizedSuccess
  | finalizedError
  deriving DecidableEq

/-- The stage the endpoint reports. `pooled` is a distinct not-processed state; a processed deploy's
    stage is decided by the inclusion block's finality, never by the execution tag alone. -/
def stage (e : Exec) (pooled : Bool) (fin : Option Bool) : Stage :=
  match e with
  | .notProcessed => if pooled then .pooled else .notProcessed
  | .success =>
      match fin with
      | some true => .finalizedSuccess
      | some false => .includedUnfinalized
      | none => .finalityUnknown
  | .error =>
      match fin with
      | some true => .finalizedError
      | some false => .includedUnfinalized
      | none => .finalityUnknown

/-- **The only settlement statement.** True exactly for a successful execution whose inclusion block
    is finalized. -/
def confirmed (e : Exec) (fin : Option Bool) : Bool :=
  match e, fin with
  | .success, some true => true
  | _, _ => false

/-- The pre-#487 reading: any processed deploy is settlement. -/
def confirmedUnchecked (e : Exec) : Bool :=
  match e with
  | .notProcessed => false
  | _ => true

/-- Every `Option Bool` is one of three values, for the finite case split below. -/
theorem option_bool_cases (fin : Option Bool) :
    fin = none ∨ fin = some true ∨ fin = some false := by
  cases fin with
  | none => exact Or.inl rfl
  | some b => cases b with
    | true => exact Or.inr (Or.inl rfl)
    | false => exact Or.inr (Or.inr rfl)

/-- **The implication is exact.** `settlementConfirmed` is true iff the stage is
    `finalizedSuccess`; no other stage can confirm. -/
theorem confirmed_iff_finalized_success (e : Exec) (pooled : Bool) (fin : Option Bool) :
    confirmed e fin = true ↔ stage e pooled fin = .finalizedSuccess := by
  cases pooled <;> rcases option_bool_cases fin with h | h | h <;> rw [h] <;>
    cases e <;> simp [confirmed, stage]

/-- **Execution success is not settlement.** A processed success whose inclusion block is not
    finalized, or whose finality is unknown, does not confirm. -/
theorem processed_success_without_finality_is_not_settlement :
    confirmed .success (some false) = false ∧ confirmed .success none = false := by
  decide

/-- **A finalized execution error is not successful settlement.** -/
theorem a_finalized_error_is_not_settlement :
    confirmed .error (some true) = false := by
  decide

/-- **The stage table is total.** Every execution/finality pair maps to one of the six stages. -/
theorem the_stage_is_total (e : Exec) (pooled : Bool) (fin : Option Bool) :
    stage e pooled fin = .notProcessed ∨ stage e pooled fin = .pooled ∨
    stage e pooled fin = .includedUnfinalized ∨ stage e pooled fin = .finalityUnknown ∨
    stage e pooled fin = .finalizedSuccess ∨ stage e pooled fin = .finalizedError := by
  cases pooled <;> rcases option_bool_cases fin with h | h | h <;> rw [h] <;>
    cases e <;> simp [stage]

/-- **A processed deploy is included before it can be finalized.** The observation that finality is
    false yields `includedUnfinalized`; only a later observation of finality can move it to a
    finalized stage. -/
theorem a_processed_deploy_is_included_before_finality (e : Exec) (pooled : Bool)
    (h : e ≠ .notProcessed) :
    stage e pooled (some false) = .includedUnfinalized := by
  cases e <;> simp_all [stage]

/-- The mutant: any processed deploy confirms. -/
def law126_mutant : Prop :=
  ∃ (e : Exec) (fin : Option Bool), confirmedUnchecked e = true ∧ confirmed e fin = false

/-- The falsifier: the incident's own pair — processed success, finality false. -/
theorem law126_falsifier : law126_mutant := by
  refine ⟨.success, some false, rfl, rfl⟩

/-- **Law 126's statement.** Confirmation is exactly `finalizedSuccess`; execution success without
    finality is not settlement; a finalized error is not successful settlement; the stage table is
    total; and the any-processed reading is refuted by the incident's pair. -/
def law126_statement : Prop :=
  (∀ (e : Exec) (pooled : Bool) (fin : Option Bool),
      confirmed e fin = true ↔ stage e pooled fin = .finalizedSuccess) ∧
  (confirmed .success (some false) = false ∧ confirmed .success none = false) ∧
  confirmed .error (some true) = false ∧
  (∀ (e : Exec) (pooled : Bool) (fin : Option Bool),
      stage e pooled fin = .notProcessed ∨ stage e pooled fin = .pooled ∨
      stage e pooled fin = .includedUnfinalized ∨ stage e pooled fin = .finalityUnknown ∨
      stage e pooled fin = .finalizedSuccess ∨ stage e pooled fin = .finalizedError) ∧
  law126_mutant

theorem law126 : law126_statement := by
  refine ⟨?_, ?_, ?_, ?_, ?_⟩
  · intro e pooled fin
    exact confirmed_iff_finalized_success e pooled fin
  · exact processed_success_without_finality_is_not_settlement
  · exact a_finalized_error_is_not_settlement
  · intro e pooled fin
    exact the_stage_is_total e pooled fin
  · exact law126_falsifier

end DeploySettlement
end Rchain
