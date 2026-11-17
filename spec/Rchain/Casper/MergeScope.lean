import Rchain.Casper.Fringe
import Mathlib.Data.Finset.Basic
import Mathlib.Data.Set.Basic
import Mathlib.Tactic

/-!
# Law 113's scope — the final scope is the trimmed layer, and a fork is rejected

The Rust merge scope (`casper/src/merging.rs`, `MergeScope::from_dag`/`from_fringes`) is the
**trimmed finalised layer** `between(final_fringe, prune_fringe)` plus the conflict scope
`between(merge_fringe, final_fringe)`. The C400 guard replaced the trimmed layer with the whole
finalised closure whenever a conflict-scope block had not seen the final fringe. That widening is
unbounded by construction: `between(final_fringe, ∅)` is every ancestor of the final fringe, so the
merge indexes the whole finalised history. On a checkpoint joiner those pre-states are not hydrated,
and the 2026-11-17 cold-rejoin run failed with `unknown root` on a height-23 block
(`spec/audit/evidence/c344-cold-rejoin-20261117/`).

This module states the replacement rule that the Rust must implement:

* the final scope is the trimmed layer, never the closure;
* a conflict-scope block that does not descend from the final fringe is a **fork**, and a fork's
  chains are rejected rather than merged against a replayed history;
* the scope is a function of the DAG, not of which states the local node happens to hold.

The tuple-space consequence — a rejected slice is not applied — is Law 113
(`Rchain.FinalizedMerge`). This module supplies the block-set half that was previously prose.

## The seen set is constructed, and it contains the message itself

`block-storage/src/dag/message_state.rs:281-285` builds a new message's seen set as
`⋃ justifications.seen ∪ {id}`: the message inserts its own id. The refusal predicate's anchor guard
is therefore the subtraction `seen \ {anchor}`, not a property that the anchor is absent from its own
seen set. The construction is modelled here (`buildsSeen`, `wellFormedSeen`) and
`self_mem_seen` is a theorem, so the anchor boundary follows from the rule rather than from a
hypothesis about arbitrary messages.
-/

namespace Rchain
namespace MergeScope

open Finset

/-- The DAG as the message map the merge reads. `none` is a block this node does not hold; the
port's `between` filters those out (`block-storage/src/dag/message_map.rs:31-50`). -/
abbrev Dag := Nat → Option Message

/-- The seen set of a block, or empty when the DAG does not hold it. -/
def seenOf (dag : Dag) (id : Nat) : Finset Nat :=
  match dag id with
  | some m => m.seen.toFinset
  | none => ∅

/-! ### The construction rule: `seen = ⋃ justifications.seen ∪ {id}` -/

/-- The node's `new_seen` rule (`block-storage/src/dag/message_state.rs:281-285`). -/
def buildsSeen (dag : Dag) (id : Nat) (m : Message) : Prop :=
  m.seen.toFinset = insert id (m.parents.toFinset.biUnion (fun p => seenOf dag p))

/-- A DAG whose every held message obeys the construction rule. -/
def wellFormedSeen (dag : Dag) : Prop :=
  ∀ id m, dag id = some m → buildsSeen dag id m

/-- **A held message sees itself.** This is the rule's other half, and the reason the refusal set is
    `seenOf anchor \ {anchor}`: the anchor is in its own seen set by construction. -/
theorem self_mem_seen (dag : Dag) (id : Nat) (m : Message)
    (h : dag id = some m) (hw : wellFormedSeen dag) : id ∈ seenOf dag id := by
  have hb := hw id m h
  simp only [seenOf, h, buildsSeen] at hb ⊢
  rw [hb]
  exact Finset.mem_insert_self id _

/-- **The height invariant the message state inherits from its justifications.** The node validates
    that a block's height is above its parents'; a seen block therefore sits at or below the seer. -/
def seenDescends (dag : Dag) (heightOf : Nat → Nat) : Prop :=
  ∀ id m x, dag id = some m → x ∈ seenOf dag id → heightOf x ≤ heightOf id

/-- `between upper lower = upper.seen \ lower.seen`, the port's scope construction. -/
def between (dag : Dag) (upper lower : Finset Nat) : Finset Nat :=
  (upper.biUnion (seenOf dag)) \ (lower.biUnion (seenOf dag))

/-- **The descent predicate**: the whole final fringe is in the block's seen set
(`final_fringe.is_subset(m.seen)`, `casper/src/merging.rs`). -/
def descends (dag : Dag) (finalFringe : Finset Nat) (id : Nat) : Prop :=
  finalFringe ⊆ seenOf dag id

/-- **The final scope is the trimmed layer.** No widening case exists in this definition. -/
def finalScope (dag : Dag) (finalFringe pruneFringe : Finset Nat) : Finset Nat :=
  between dag finalFringe pruneFringe

/-- The conflict scope: what the merge must reconcile. -/
def conflictScope (dag : Dag) (mergeFringe finalFringe : Finset Nat) : Finset Nat :=
  between dag mergeFringe finalFringe

/-- **The kept-conflict rule.** A kept set is a subset of the conflict scope, and every
non-descendant conflict block is outside it: a fork's chains are rejected, not merged. -/
def KeptRule
    (dag : Dag) (mergeFringe finalFringe kept : Finset Nat) : Prop :=
  kept ⊆ conflictScope dag mergeFringe finalFringe ∧
    ∀ id ∈ conflictScope dag mergeFringe finalFringe,
      ¬ descends dag finalFringe id → id ∉ kept

/-- **The rule implies every kept conflict block descends from the final fringe.** This is the
property the C400 widening was buying, obtained without reading the closure. -/
theorem kept_chains_descend
    (dag : Dag) (mergeFringe finalFringe kept : Finset Nat)
    (h : KeptRule dag mergeFringe finalFringe kept) :
    ∀ id ∈ kept, descends dag finalFringe id := by
  intro id hid
  by_contra hnd
  exact h.2 id (h.1 hid) hnd hid

/-- **A non-descendant conflict block is not kept.** -/
theorem a_non_descendant_is_not_kept
    (dag : Dag) (mergeFringe finalFringe kept : Finset Nat) (id : Nat)
    (h : KeptRule dag mergeFringe finalFringe kept)
    (hmem : id ∈ conflictScope dag mergeFringe finalFringe)
    (hnd : ¬ descends dag finalFringe id) :
    id ∉ kept :=
  h.2 id hmem hnd

/-- **The final scope is the trimmed layer, definitionally** — the theorem is the anti-regression
witness against reintroducing a widening case. -/
theorem final_scope_is_the_trimmed_layer
    (dag : Dag) (finalFringe pruneFringe : Finset Nat) :
    finalScope dag finalFringe pruneFringe = between dag finalFringe pruneFringe := rfl

/-- **The mutant: widen the final scope to the whole finalised closure.** This is the C400 guard's
`between(final_fringe, ∅)`; the theorem below exhibits a block it reads that the trimmed layer does
not. -/
def finalScopeWidened (dag : Dag) (finalFringe : Finset Nat) : Finset Nat :=
  between dag finalFringe ∅

/-- A fixture: `g ← p ← f`, with `p` the prune frontier and `f` the final fringe. -/
def fixtureDag : Dag
  | 1 => some ⟨1, 0, 0, 0, [], [1]⟩
  | 2 => some ⟨2, 1, 0, 0, [1], [1, 2]⟩
  | 3 => some ⟨3, 2, 0, 0, [2], [1, 2, 3]⟩
  | _ => none

/-- **The widening reads the closure.** The genesis block is in the widened scope and not in the
trimmed layer `between({f}, {p}) = {f}`. This is the unbounded read the live joiner could not
hydrate. -/
theorem widening_reads_the_closure :
    (1 : Nat) ∈ finalScopeWidened fixtureDag {3} ∧
      (1 : Nat) ∉ finalScope fixtureDag {3} {2} := by
  decide

/-! ### Law 109's committed history and the pre-replay refusal set -/

/-- **The committed history of an anchor**: the anchor's seen set. By `buildsSeen` it already
    contains the anchor itself. This is the closure a checkpoint certificate anchors; it is *not*
    every block at or below the anchor's height. -/
def committedHistory (dag : Dag) (anchor : Nat) : Finset Nat :=
  seenOf dag anchor

/-- Membership in the anchor's seen closure. The anchor itself is committed history (it is the
    accepted base); the Rust predicate excludes it separately. -/
def IsCommittedHistory (dag : Dag) (anchor id : Nat) : Prop :=
  id ∈ committedHistory dag anchor

/-- **A same-height sibling is not committed history.** A block the anchor has not seen, however
close its height, is a live message the merge must be able to read — not history a joiner may refuse.
The Rust `is_committed_at_or_below_anchor` keyed on height alone, which refused the post-certificate
siblings and made every height-81 block fail `InvalidPreStateHash`. -/
theorem a_sibling_is_not_committed_history
    (dag : Dag) (anchor sibling : Nat)
    (hsib : sibling ∉ seenOf dag anchor) :
    ¬ IsCommittedHistory dag anchor sibling := by
  simpa [IsCommittedHistory, committedHistory] using hsib

/-- **The pre-replay refusal set.** `is_committed_history`
    (`casper/src/blocks/block_processor.rs:886-906`) returns **false for the anchor itself**: the
    anchor is the accepted base, handled by the separate anchor check. By `buildsSeen` the anchor is
    in its own seen set, so the refusal set is the seen set minus the anchor. -/
def replayRefusedHistory (dag : Dag) (anchor : Nat) : Finset Nat :=
  committedHistory dag anchor \ {anchor}

/-- Membership in the refusal set. -/
def IsReplayRefused (dag : Dag) (anchor id : Nat) : Prop :=
  id ∈ replayRefusedHistory dag anchor

/-- **The anchor is committed history but not refused.** The self-membership is the node's
    construction rule (`self_mem_seen`), not an assumption about arbitrary messages. -/
theorem the_anchor_is_committed_history_but_not_replay_refused
    (dag : Dag) (anchor : Nat) (m : Message)
    (h : dag anchor = some m) (hw : wellFormedSeen dag) :
    IsCommittedHistory dag anchor anchor ∧ ¬ IsReplayRefused dag anchor anchor := by
  have hself := self_mem_seen dag anchor m h hw
  constructor
  · simpa [IsCommittedHistory, committedHistory] using hself
  · simp [IsReplayRefused, replayRefusedHistory, hself]

/-- **The refusal set is the committed history without the anchor**, definitionally: the seen set
    contains the anchor, and the predicate's anchor guard subtracts it. -/
theorem replay_refusal_is_committed_history_without_the_anchor
    (dag : Dag) (anchor : Nat) :
    replayRefusedHistory dag anchor = committedHistory dag anchor \ {anchor} := rfl

/-- The Rust `is_committed_history`'s three arms: not the anchor, at or below the anchor's height,
    and in the anchor's seen set (`casper/src/blocks/block_processor.rs:886-906`). -/
def IsCommittedHistoryArms (dag : Dag) (heightOf : Nat → Nat) (anchor id : Nat) : Prop :=
  id ≠ anchor ∧ heightOf id ≤ heightOf anchor ∧ id ∈ seenOf dag anchor

/-- **The Rust arms are exactly the refusal set.** On a DAG whose seen sets descend in height, the
    height arm removes nothing from the seen set; the anchor arm is the subtraction. -/
theorem the_rust_arms_are_the_refusal_set
    (dag : Dag) (heightOf : Nat → Nat) (anchor : Nat)
    (hd : seenDescends dag heightOf) :
    ∀ id, IsCommittedHistoryArms dag heightOf anchor id ↔ IsReplayRefused dag anchor id := by
  intro id
  constructor
  · rintro ⟨hne, _, hmem⟩
    exact Finset.mem_sdiff.mpr ⟨hmem, by simpa using hne⟩
  · intro h
    have hmem : id ∈ seenOf dag anchor := (Finset.mem_sdiff.mp h).1
    have hne : id ≠ anchor := by
      have hnot : id ∉ ({anchor} : Finset Nat) := (Finset.mem_sdiff.mp h).2
      simpa using hnot
    have hex : ∃ m, dag anchor = some m := by
      by_contra hnone
      have hempty : seenOf dag anchor = ∅ := by
        unfold seenOf
        split
        · rename_i m hm
          exact absurd ⟨m, hm⟩ hnone
        · rfl
      rw [hempty] at hmem
      simp at hmem
    obtain ⟨m, hm⟩ := hex
    exact ⟨hne, hd anchor m id hm hmem, hmem⟩

/-! ### Law 109's row -/

/-- **The mutant proposition**: a well-formed DAG whose anchor is in the pre-replay refusal set.
    The old model refused the anchor because its seen set contains itself; the corrected refusal set
    subtracts it. -/
def law109_mutant : Prop :=
  ∃ (dag : Dag) (anchor : Nat) (m : Message),
    dag anchor = some m ∧ wellFormedSeen dag ∧ anchor ∈ replayRefusedHistory dag anchor

/-- **The falsifier, fired.** A held anchor sees itself, so it is committed history; the refusal
    set subtracts it, and the mutant's membership is impossible. -/
theorem law109_falsifier : ¬ law109_mutant := by
  rintro ⟨dag, anchor, m, hm, hw, href⟩
  have hself := self_mem_seen dag anchor m hm hw
  simp [replayRefusedHistory, committedHistory, hself] at href

/-- **Law 109's statement.** The anchor is committed history by construction and is never refused;
    the refusal set is that history without the anchor; and the Rust predicate's three arms are
    exactly the refusal set on a height-descending DAG. -/
def law109_statement : Prop :=
  (∀ (dag : Dag) (anchor : Nat) (m : Message), dag anchor = some m → wellFormedSeen dag →
    anchor ∈ committedHistory dag anchor ∧ ¬ IsReplayRefused dag anchor anchor) ∧
  (∀ (dag : Dag) (anchor : Nat),
    replayRefusedHistory dag anchor = committedHistory dag anchor \ {anchor}) ∧
  (∀ (dag : Dag) (heightOf : Nat → Nat) (anchor : Nat), seenDescends dag heightOf →
    ∀ id, IsCommittedHistoryArms dag heightOf anchor id ↔ IsReplayRefused dag anchor id) ∧
  ¬ law109_mutant

theorem law109 : law109_statement := by
  refine ⟨?_, ?_, ?_, law109_falsifier⟩
  · intro dag anchor m hm hw
    exact the_anchor_is_committed_history_but_not_replay_refused dag anchor m hm hw
  · intro dag anchor
    exact replay_refusal_is_committed_history_without_the_anchor dag anchor
  · intro dag heightOf anchor hd id
    exact the_rust_arms_are_the_refusal_set dag heightOf anchor hd id

/-- **The scope is a function of the DAG.** Two nodes that hold the same message map derive the same
scope, regardless of which states they have hydrated; a node that lacks a pre-state must refuse,
not derive a different scope. -/
theorem scope_is_a_function_of_the_dag
    {dag dag' : Dag} (h : dag = dag')
    (mergeFringe finalFringe pruneFringe : Finset Nat) :
    finalScope dag finalFringe pruneFringe = finalScope dag' finalFringe pruneFringe ∧
      conflictScope dag mergeFringe finalFringe =
        conflictScope dag' mergeFringe finalFringe := by
  rw [h]
  exact ⟨rfl, rfl⟩

/-- **The fork set**: the conflict-scope blocks that have not seen the whole final fringe. This is
    the membership rule of `MergeScope.fork_blocks` (`casper/src/merging.rs:2054-2062`), the set
    `merge` rejects before the conflict search. A `Set` rather than a `Finset` because the DAG is an
    opaque function here: membership is the predicate the Rust set is built from. -/
def forkBlocks (dag : Dag) (mergeFringe finalFringe : Finset Nat) : Set Nat :=
  { id | id ∈ conflictScope dag mergeFringe finalFringe ∧ ¬ descends dag finalFringe id }

/-- **The fork set is a function of the DAG and the two fringes.** No hydrated state, no arrival
    order: a producer and a joiner that hold the same message map derive the same forks. This is the
    determinism the cold-join fix relies on. -/
theorem fork_blocks_are_a_function_of_the_dag
    {dag dag' : Dag} (h : dag = dag') (mergeFringe finalFringe : Finset Nat) :
    forkBlocks dag mergeFringe finalFringe = forkBlocks dag' mergeFringe finalFringe := by
  rw [h]

/-- A fork block is a conflict-scope block that does not descend from the final fringe. -/
theorem a_fork_block_is_a_non_descendant_conflict_block
    (dag : Dag) (mergeFringe finalFringe : Finset Nat) (id : Nat)
    (h : id ∈ forkBlocks dag mergeFringe finalFringe) :
    id ∈ conflictScope dag mergeFringe finalFringe ∧ ¬ descends dag finalFringe id := by
  simpa [forkBlocks] using h

/-! ### Law 115 — the least closure `with_dependencies` computes -/

/-- The least-closure property: contains the roots, closed under the dependency map, and least
    among the closed supersets. -/
structure DependencyClosure (dep : Nat → Finset Nat) (roots closure : Finset Nat) : Prop where
  contains_roots : roots ⊆ closure
  closed : ∀ x ∈ closure, dep x ⊆ closure
  least : ∀ c, roots ⊆ c → (∀ x ∈ c, dep x ⊆ c) → closure ⊆ c

/-- One level of the Rust loop: the set plus every dependency of its members. -/
def depStep (dep : Nat → Finset Nat) (S : Finset Nat) : Finset Nat :=
  S ∪ S.biUnion dep

/-- `n` rounds of the level-synchronous loop (`sdk/src/dag/merging.rs:31-50`). -/
def closeN (dep : Nat → Finset Nat) : Nat → Finset Nat → Finset Nat
  | 0, S => S
  | n + 1, S => closeN dep n (depStep dep S)

/-- The closure inside a finite universe `U`. `|U|` rounds suffice: every non-final round adds at
    least one element of `U`, and `U` is finite. -/
def close (U : Finset Nat) (dep : Nat → Finset Nat) (S : Finset Nat) : Finset Nat :=
  closeN dep U.card S

theorem depStep_subset (dep : Nat → Finset Nat) (S : Finset Nat) : S ⊆ depStep dep S := by
  intro x hx
  exact Finset.mem_union.mpr (Or.inl hx)

theorem depStep_mono (dep : Nat → Finset Nat) {S T : Finset Nat} (h : S ⊆ T) :
    depStep dep S ⊆ depStep dep T := by
  intro x hx
  rcases Finset.mem_union.mp hx with hx | hx
  · exact Finset.mem_union.mpr (Or.inl (h hx))
  · rcases Finset.mem_biUnion.mp hx with ⟨a, ha, hxa⟩
    exact Finset.mem_union.mpr (Or.inr (Finset.mem_biUnion.mpr ⟨a, h ha, hxa⟩))

theorem depStep_subset_U (dep : Nat → Finset Nat) (U : Finset Nat)
    (hdep : ∀ x, dep x ⊆ U) {S : Finset Nat} (hS : S ⊆ U) : depStep dep S ⊆ U := by
  intro x hx
  rcases Finset.mem_union.mp hx with hx | hx
  · exact hS hx
  · rcases Finset.mem_biUnion.mp hx with ⟨a, _, hxa⟩
    exact hdep a hxa

theorem depStep_subset_c (dep : Nat → Finset Nat) {S c : Finset Nat}
    (hS : S ⊆ c) (hc : ∀ x ∈ c, dep x ⊆ c) : depStep dep S ⊆ c := by
  intro x hx
  rcases Finset.mem_union.mp hx with hx | hx
  · exact hS hx
  · rcases Finset.mem_biUnion.mp hx with ⟨a, ha, hxa⟩
    exact hc a (hS ha) hxa

theorem closeN_mono (dep : Nat → Finset Nat) (n : Nat) {S T : Finset Nat} (h : S ⊆ T) :
    closeN dep n S ⊆ closeN dep n T := by
  induction n generalizing S T with
  | zero => exact h
  | succ _n ih => exact ih (depStep_mono dep h)

theorem closeN_contains (dep : Nat → Finset Nat) (S : Finset Nat) :
    ∀ n, S ⊆ closeN dep n S
  | 0 => by simp [closeN]
  | n + 1 => (depStep_subset dep S).trans (closeN_contains dep (depStep dep S) n)

theorem closeN_subset_U (dep : Nat → Finset Nat) (U : Finset Nat)
    (hdep : ∀ x, dep x ⊆ U) :
    ∀ n (S : Finset Nat), S ⊆ U → closeN dep n S ⊆ U
  | 0, _S, hS => hS
  | n + 1, S, hS =>
      closeN_subset_U dep U hdep n (depStep dep S) (depStep_subset_U dep U hdep hS)

theorem closeN_least (dep : Nat → Finset Nat) {S c : Finset Nat}
    (hS : S ⊆ c) (hc : ∀ x ∈ c, dep x ⊆ c) :
    ∀ n, closeN dep n S ⊆ c
  | 0 => hS
  | n + 1 => closeN_least dep (depStep_subset_c dep hS hc) hc n

theorem closeN_succ_eq (dep : Nat → Finset Nat) (n : Nat) (S : Finset Nat) :
    closeN dep (n + 1) S = depStep dep (closeN dep n S) := by
  induction n generalizing S with
  | zero => rfl
  | succ n ih => simpa [closeN] using ih (depStep dep S)

theorem closeN_fixed (dep : Nat → Finset Nat) {R : Finset Nat}
    (h : depStep dep R = R) :
    ∀ n, closeN dep n R = R
  | 0 => rfl
  | n + 1 => by rw [closeN_succ_eq, closeN_fixed dep h n, h]

theorem closeN_eq_of_fixed (dep : Nat → Finset Nat) (S : Finset Nat) (k : Nat)
    (hfix : depStep dep (closeN dep k S) = closeN dep k S) :
    ∀ m, closeN dep (k + m) S = closeN dep k S := by
  intro m
  induction m with
  | zero => simp
  | succ m ih =>
      rw [Nat.add_succ, closeN_succ_eq, ih, hfix]

/-- **The iteration reaches a fixed point inside `U`.** If it did not, each of the `|U|` rounds
    would add at least one element of `U`, and the final set would have more than `|U|` members. -/
theorem close_is_fixed (U : Finset Nat) (dep : Nat → Finset Nat) (S : Finset Nat)
    (hS : S ⊆ U) (hdep : ∀ x, dep x ⊆ U) :
    depStep dep (close U dep S) = close U dep S := by
  by_contra hne
  change depStep dep (closeN dep U.card S) ≠ closeN dep U.card S at hne
  have hnotfixed : ∀ k ≤ U.card, closeN dep k S ≠ closeN dep (k + 1) S := by
    intro k hk hfix
    have hfix' : depStep dep (closeN dep k S) = closeN dep k S := by
      rw [← closeN_succ_eq]
      exact hfix.symm
    have hfinal : closeN dep U.card S = closeN dep k S := by
      rw [← Nat.add_sub_of_le hk]
      exact closeN_eq_of_fixed dep S k hfix' (U.card - k)
    rw [hfinal] at hne
    exact hne hfix'
  have hstrict : ∀ k < U.card, (closeN dep k S).card < (closeN dep (k + 1) S).card := by
    intro k hk
    have hne_k : closeN dep k S ≠ closeN dep (k + 1) S := hnotfixed k (Nat.le_of_lt hk)
    have hsub_k : closeN dep k S ⊆ closeN dep (k + 1) S := by
      show closeN dep k S ⊆ closeN dep k (depStep dep S)
      exact closeN_mono dep k (depStep_subset dep S)
    exact Finset.card_lt_card (Finset.ssubset_iff_subset_ne.mpr ⟨hsub_k, hne_k⟩)
  have hcard : ∀ k ≤ U.card, S.card + k ≤ (closeN dep k S).card := by
    intro k hk
    induction k with
    | zero => exact le_rfl
    | succ k ih =>
        have hk' : k < U.card := Nat.lt_of_succ_le hk
        have hstep := hstrict k hk'
        have ih' := ih (Nat.le_of_lt hk')
        omega
  have hleU : (closeN dep U.card S).card ≤ U.card :=
    Finset.card_le_card (closeN_subset_U dep U hdep U.card S hS)
  have hcardU := hcard U.card le_rfl
  by_cases hS0 : S.card = 0
  · have hReq : (closeN dep U.card S).card = U.card := by omega
    have hReqU : closeN dep U.card S = U :=
      Finset.eq_of_subset_of_card_le (closeN_subset_U dep U hdep U.card S hS) (by omega)
    have hfixR : depStep dep (closeN dep U.card S) = closeN dep U.card S := by
      rw [hReqU]
      exact Finset.Subset.antisymm
        (depStep_subset_U dep U hdep (by intro x hx; exact hx))
        (depStep_subset dep U)
    exact hne hfixR
  · omega

/-- **The Rust loop computes the least closure.** -/
theorem close_spec (U : Finset Nat) (dep : Nat → Finset Nat) (roots : Finset Nat)
    (hU : roots ⊆ U) (hdep : ∀ x, dep x ⊆ U) :
    DependencyClosure dep roots (close U dep roots) := by
  refine ⟨?_, ?_, ?_⟩
  · exact closeN_contains dep roots U.card
  · intro x hx
    have hfix := close_is_fixed U dep roots hU hdep
    have hsub : dep x ⊆ depStep dep (close U dep roots) := by
      intro y hy
      exact Finset.mem_union.mpr (Or.inr (Finset.mem_biUnion.mpr ⟨x, hx, hy⟩))
    rw [hfix] at hsub
    exact hsub
  · intro c hc hclosed
    exact closeN_least dep hc hclosed U.card

/-- **Rejection is closed under dependency.** A chain in the forced set brings every chain it
    depends on with it: the merge cannot keep a dependent of a rejected fork. -/
theorem rejection_is_closed_under_dependency
    (dep : Nat → Finset Nat) (roots closure : Finset Nat)
    (h : DependencyClosure dep roots closure) :
    ∀ x ∈ closure, dep x ⊆ closure :=
  h.closed

/-- **A fork is rejected in the forced set.** The roots of the closure are the fork blocks' chains,
    so every root is in the set the merge removes from `to_merge`. -/
theorem a_fork_block_is_rejected
    (dep : Nat → Finset Nat) (forks closure : Finset Nat)
    (h : DependencyClosure dep forks closure) :
    forks ⊆ closure :=
  h.contains_roots

/-- **The closure is unique.** Two sets satisfying the least-closure spec are equal, so a producer
    and a joiner that compute the same dependency map reject the same set: the fork rule is
    deterministic, not an artefact of fold order. -/
theorem the_dependency_closure_is_unique
    (dep : Nat → Finset Nat) (roots c₁ c₂ : Finset Nat)
    (h₁ : DependencyClosure dep roots c₁) (h₂ : DependencyClosure dep roots c₂) : c₁ = c₂ := by
  apply Finset.Subset.antisymm
  · exact h₁.least c₂ h₂.contains_roots h₂.closed
  · exact h₂.least c₁ h₁.contains_roots h₁.closed

/-- **The mutant: classify a fork by the anchor's committed closure instead of by descent.** The
    closure rule calls a conflict block that *does* descend from the final fringe a fork — it is
    above the anchor and not in the anchor's seen set — so a legitimate merge participant is
    rejected. The descent rule keeps it. -/
def closureKeyedFork (dag : Dag) (anchor id : Nat) : Prop :=
  id ∉ committedHistory dag anchor

/-- The fork fixtures: `2` is the anchor (seen `{1}`), `6` descends from it, `5` is the merge
    block, and `1` is genesis. -/
def forkFixtureDag : Dag
  | 1 => some ⟨1, 0, 0, 0, [], [1]⟩
  | 2 => some ⟨2, 1, 0, 0, [1], [1, 2]⟩
  | 5 => some ⟨5, 4, 0, 0, [6], [1, 2, 6, 5]⟩
  | 6 => some ⟨6, 3, 0, 0, [1, 2], [1, 2, 6]⟩
  | _ => none

/-- **The falsifier, fired.** Block `6` sees the final fringe `{2}`, so it is not a fork under the
    descent rule; the closure rule calls it one because `6` is not in the anchor's seen set. -/
theorem a_descendant_is_not_a_fork_but_the_closure_rule_calls_it_one :
    (6 : Nat) ∈ conflictScope forkFixtureDag {5} {2} ∧ descends forkFixtureDag {2} 6 ∧
      (6 : Nat) ∉ forkBlocks forkFixtureDag {5} {2} ∧ closureKeyedFork forkFixtureDag 2 6 := by
  simp [conflictScope, between, seenOf, forkFixtureDag, descends, forkBlocks, closureKeyedFork,
    committedHistory]

/-- **Law 115's statement.** The fork set is a function of the DAG; the Rust loop computes the
    least dependency closure of the fork chains; and that closure is unique. -/
def law115_statement : Prop :=
  (∀ (dag dag' : Dag), dag = dag' → ∀ mergeFringe finalFringe : Finset Nat,
    forkBlocks dag mergeFringe finalFringe = forkBlocks dag' mergeFringe finalFringe) ∧
  (∀ (U : Finset Nat) (dep : Nat → Finset Nat) (forks : Finset Nat),
    forks ⊆ U → (∀ x, dep x ⊆ U) →
      DependencyClosure dep forks (close U dep forks)) ∧
  (∀ (dep : Nat → Finset Nat) (roots c₁ c₂ : Finset Nat),
    DependencyClosure dep roots c₁ → DependencyClosure dep roots c₂ → c₁ = c₂)

theorem law115 : law115_statement := by
  refine ⟨?_, ?_, ?_⟩
  · intro dag dag' h mergeFringe finalFringe
    exact fork_blocks_are_a_function_of_the_dag h mergeFringe finalFringe
  · intro U dep forks hU hdep
    exact close_spec U dep forks hU hdep
  · intro dep roots c₁ c₂ h₁ h₂
    exact the_dependency_closure_is_unique dep roots c₁ c₂ h₁ h₂

/-- **The mutant proposition**: a legitimate conflict-scope descendant is classified as a fork by
    the closure rule. -/
def law115_mutant : Prop :=
  ∃ (dag : Dag) (anchor id : Nat),
    id ∈ conflictScope dag {5} {2} ∧ descends dag {2} id ∧ closureKeyedFork dag anchor id

/-- **The falsifier, fired.** The fixture above: `6` descends from the final fringe, so the closure
    rule's classification is false. -/
theorem law115_falsifier : law115_mutant := by
  refine ⟨forkFixtureDag, 2, 6, ?_, ?_, ?_⟩
  · simp [conflictScope, between, seenOf, forkFixtureDag]
  · simp [descends, seenOf, forkFixtureDag]
  · simp [closureKeyedFork, committedHistory, seenOf, forkFixtureDag]

end MergeScope
end Rchain
