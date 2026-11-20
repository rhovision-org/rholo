import Mathlib.Data.List.Basic
import Mathlib.Tactic

/-!
# Law 113 — a merge above a finalised base does not undo a tuple-space write

The multi-parent merge applies the **kept** half of its conflict scope to the finalised base
(`casper/src/merging.rs:2049-2129`): the final scope is a constraint and a rejection ledger, never an
applied change. The law is the consequence at the tuple-space level — a datum present at the finalised
base survives unless a kept chain removed it, i.e. unless a later accepted block consumed it. The
qualifier is load-bearing: the unqualified reading ("never removed") is false, and
`casper/tests/n420_boundary_merge.rs::merging_a_tuple_space_rmw_block_duplicates_its_produce` is the
existing witness of a kept chain consuming a base datum.

Scope: tuple-space datums. Absolute native writes are a different state model
(`rspace/src/native_store.rs`) and are not this law's subject.
-/

namespace Rchain
namespace FinalizedMerge

abbrev Datum := Nat
abbrev Chan := Nat

/-- The applied state: each channel holds a multiset of datums, as counts. -/
abbrev State := Chan → Datum → Nat

/-- One chain's observable change: what it added and removed at each channel. -/
structure Change where
  added : Chan → List Datum
  removed : Chan → List Datum

/-- The per-channel concatenation `StateChange::combine` performs. -/
def combined (cs : List Change) (sel : Change → Chan → List Datum) (c : Chan) : List Datum :=
  (cs.map fun ch => sel ch c).join

/-- `net_datum_change`: cancel one addition against one removal of the same datum inside the batch,
    so a produce-and-consume within the merge leaves the base untouched (`state_change_merger.rs:61-77`). -/
def cancel : List Datum → List Datum → List Datum × List Datum
  | a, [] => (a, [])
  | a, y :: ys =>
      let (a', r') := cancel a ys
      if y ∈ a' then (a'.erase y, r') else (a', y :: r')

/-- The batch's net `(added, removed)` at a channel. -/
def net (cs : List Change) (c : Chan) : List Datum × List Datum :=
  cancel (combined cs Change.added c) (combined cs Change.removed c)

/-- Apply the kept slice to the base: `seq_diff(base, net.removed) ++ net.added`
    (`state_change_merger.rs:44-48`). -/
def applyKept (base : State) (cs : List Change) : State :=
  fun c d => base c d - (net cs c).2.count d + (net cs c).1.count d

/-- A removal in the net is a removal in the batch. Cancellation only removes a removal against an
    addition of the same datum; it never invents one. -/
theorem mem_cancel_removed {a r : List Datum} {d : Datum} :
    d ∈ (cancel a r).2 → d ∈ r := by
  induction r generalizing a with
  | nil => simp [cancel]
  | cons y ys ih =>
      simp only [cancel]
      split
      · intro h
        exact List.mem_cons.mpr (Or.inr (ih h))
      · intro h
        rcases List.mem_cons.mp h with rfl | h'
        · exact List.mem_cons.mpr (Or.inl rfl)
        · exact List.mem_cons.mpr (Or.inr (ih h'))

/-- **Law 113 — a finalised tuple-space datum survives unless a kept chain removes it.** -/
def law113_statement : Prop :=
  ∀ (base : State) (cs : List Change) (c : Chan) (d : Datum),
    base c d > 0 →
    (∀ ch ∈ cs, d ∉ ch.removed c) →
    applyKept base cs c d > 0

theorem law113 : law113_statement := by
  intro base cs c d hbase hnot
  unfold applyKept
  have hzero : (net cs c).2.count d = 0 := by
    apply List.count_eq_zero.mpr
    intro hmem
    have hbatch : d ∈ combined cs Change.removed c := mem_cancel_removed hmem
    rcases List.mem_join.mp hbatch with ⟨l, hl, hdl⟩
    rcases List.mem_map.mp hl with ⟨ch, hch, rfl⟩
    exact hnot ch hch hdl
  rw [hzero]
  omega

/-- **The mutant**: apply the whole conflict scope, kept and rejected — a merge that treats a
    rejection decision as advisory. The code applies `to_merge` alone (`merging.rs:2049-2071`,
    `:2122-2129`), so this is the mutation `compute_merged_state(&to_merge)` →
    `compute_merged_state(&conflict_set)`. -/
def applyUnscoped (base : State) (kept rejected : List Change) : State :=
  applyKept base (kept ++ rejected)

/-- The mutant proposition: a rejected chain's removal drops a datum the kept slice preserved. -/
def law113_mutant : Prop :=
  ∃ (base : State) (kept rejected : List Change) (c : Chan) (d : Datum),
    base c d > 0 ∧
    (∀ ch ∈ kept, d ∉ ch.removed c) ∧
    (∃ ch, ch ∈ rejected ∧ d ∈ ch.removed c) ∧
    applyUnscoped base kept rejected c d = 0

/-- **The falsifier, fired.** Base holds `7` at channel `0`; the kept slice is empty; a rejected chain
    removes `7`. The scoped rule keeps it, the unscoped mutant drops it. -/
theorem law113_falsifier : law113_mutant := by
  let base : State := fun _ _ => 1
  let rejected : Change := ⟨fun _ => [], fun c' => if c' = 0 then [7] else []⟩
  refine ⟨base, [], [rejected], 0, 7, by simp [base], by simp, ?_, ?_⟩
  · exact ⟨rejected, by simp, by simp [rejected]⟩
  · simp [applyUnscoped, applyKept, net, cancel, combined, rejected]

/-- Non-vacuity on the same fixture: the scoped rule keeps what the mutant drops. -/
theorem the_same_fixture_survives_under_the_law :
    applyKept (fun _ _ => 1) [] 0 7 > 0 := by
  simp [applyKept, net, cancel, combined]

/-! ### The base selector: agreement is the guard

The Rust reader compares the finalisers' `fringe_state_hash` values and refuses a disagreement
(`choose_base_claim`, `casper/src/multi_parent_casper.rs`). The model compares the hash, which is
what the implementation can compare; a disagreement has no `pickBase` result. -/

/-- One finaliser's claim: height, block id, and the state hash it reports for its fringe. -/
structure Claim where
  height : Nat
  id : Nat
  hash : Nat

/-- The picker's order: greater height wins, then greater id. -/
def better (a b : Claim) : Bool :=
  decide (a.height > b.height) ||
    (decide (a.height = b.height) && decide (a.id > b.id))

/-- The greatest claim under `better`. -/
def pickClaim : List Claim → Option Claim
  | [] => none
  | c :: cs =>
      match pickClaim cs with
      | none => some c
      | some best => if better c best then some c else some best

/-- **The chosen claim is one of the claims.** Selection cannot invent a claim. -/
theorem pickClaim_mem : ∀ (cs : List Claim) (c : Claim),
    pickClaim cs = some c → c ∈ cs := by
  intro cs
  induction cs with
  | nil =>
      intro c h
      simp [pickClaim] at h
  | cons x xs ih =>
      intro c h
      cases hpc : pickClaim xs with
      | none =>
          simp [pickClaim, hpc] at h
          subst h
          exact List.mem_cons_self x xs
      | some best =>
          simp only [pickClaim, hpc] at h
          split at h
          · simp only [Option.some.injEq] at h
            subst h
            exact List.mem_cons_self x xs
          · simp only [Option.some.injEq] at h
            subst h
            exact List.mem_cons_of_mem x (ih best hpc)

/-- Every claim in the tail reports `h`. -/
def allSame (h : Nat) : List Claim → Bool
  | [] => true
  | c :: cs => (c.hash == h) && allSame h cs

theorem allSame_sound : ∀ {h : Nat} {cs : List Claim}, allSame h cs = true →
    ∀ x ∈ cs, x.hash = h := by
  intro h cs
  induction cs with
  | nil =>
      intro _ x hx
      simp at hx
  | cons c cs ih =>
      intro hb x hx
      simp only [allSame] at hb
      rw [Bool.and_eq_true] at hb
      rcases hb with ⟨hc, hrest⟩
      rw [beq_iff_eq] at hc
      rcases List.mem_cons.mp hx with rfl | hx
      · exact hc
      · exact ih hrest x hx

/-- **The agreement check.** Every claim's hash equals the first claim's. -/
def agreeB : List Claim → Bool
  | [] => true
  | c :: cs => allSame c.hash cs

/-- Every claim reports the same state hash. -/
def Agree (claims : List Claim) : Prop := agreeB claims = true

/-- **The guarded selector.** The agreement guard is in the definition: a disagreeing claim list
has no selected base. This is the function the Rust `choose_base_claim` implements. -/
def pickBase (claims : List Claim) : Option Claim :=
  match agreeB claims with
  | true => pickClaim claims
  | false => none

/-- The boolean agreement check implies equality of every pair of claim hashes. -/
theorem agreeB_sound : ∀ {claims : List Claim}, agreeB claims = true →
    ∀ x ∈ claims, ∀ y ∈ claims, x.hash = y.hash := by
  intro claims
  induction claims with
  | nil =>
      intro _ x hx
      simp at hx
  | cons c cs _ih =>
      intro hb x hx y hy
      simp only [agreeB] at hb
      have hx_c : x.hash = c.hash := by
        rcases List.mem_cons.mp hx with rfl | hx
        · rfl
        · exact allSame_sound hb x hx
      have hy_c : y.hash = c.hash := by
        rcases List.mem_cons.mp hy with rfl | hy
        · rfl
        · exact allSame_sound hb y hy
      exact hx_c.trans hy_c.symm

/-- `pickBase` is `none` on disagreement. -/
theorem pickBase_none_of_disagree {claims : List Claim} (h : ¬ Agree claims) :
    pickBase claims = none := by
  unfold Agree at h
  cases hb : agreeB claims with
  | true => exact absurd hb h
  | false => simp [pickBase, hb]

/-- **A selected base agrees with every claim.** This is the theorem the stale-base mutant cannot
satisfy: the guard, not selection, establishes it. -/
theorem pickBase_agrees {claims : List Claim} {c : Claim}
    (h : pickBase claims = some c) : ∀ x ∈ claims, x.hash = c.hash := by
  cases hb : agreeB claims with
  | false => simp [pickBase, hb] at h
  | true =>
      have hsel : pickClaim claims = some c := by simpa [pickBase, hb] using h
      have hmem := pickClaim_mem claims c hsel
      exact fun x hx => agreeB_sound hb x hx c hmem

/-- The selected claim is one of the claims. -/
theorem pickBase_mem {claims : List Claim} {c : Claim}
    (h : pickBase claims = some c) : c ∈ claims := by
  cases hb : agreeB claims with
  | false => simp [pickBase, hb] at h
  | true =>
      have hsel : pickClaim claims = some c := by simpa [pickBase, hb] using h
      exact pickClaim_mem claims c hsel

/-! ### The rejection ledger

The merge subtracts what the final scope accepted from the rejected set; a finalised deploy is then
not a value of the guarded ledger. The Rust `finalised_floor` refuses the host-level overlap. -/

/-- The guarded ledger: rejected minus accepted. -/
def guardedRejected (accepted rejected : Finset Nat) : Finset Nat :=
  rejected \ accepted

/-- **A finalised deploy is not in the guarded ledger.** -/
theorem a_finalised_deploy_is_not_in_the_ledger (accepted rejected : Finset Nat) (d : Nat)
    (h : d ∈ accepted) : d ∉ guardedRejected accepted rejected := by
  simp [guardedRejected, h]

/-- **The boundary**: without the subtraction, a rejected set that names an accepted deploy is a
value; the guard removes it. -/
theorem unguarded_ledger_can_name_a_finalised_deploy :
    ∃ (accepted rejected : Finset Nat) (d : Nat),
      d ∈ accepted ∧ d ∈ rejected ∧ d ∉ guardedRejected accepted rejected := by
  refine ⟨{7}, {7}, 7, by simp, by simp, ?_⟩
  simp [guardedRejected]

end FinalizedMerge
end Rchain
