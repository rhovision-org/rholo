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

**What this law does not cover, and it is the adversary page's open question.** *Absolute* native writes
are whole-slot snapshots (`rspace/src/native_store.rs`), and `NativeRelations::fold` never compares a
kept write against the base. A stale conflict-scope block can therefore overwrite a finalised slot value
computed without it: the stale-fork trace is recorded in `spec/audit/passes.md` §170, and the missing
guard is a base-state comparison or a descent check. That is a **refutation of the unqualified law** for
native absolute writes, not this theorem's claim.
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

end FinalizedMerge
end Rchain
