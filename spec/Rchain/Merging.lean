import Mathlib.Data.Finset.Basic
import Rchain.Cmp
import Rchain.Crypto.Random

/-!
# Law 17 — the merge arithmetic, and the RNG merge at its call site

The port's numeric-channel merge lives in `rholang/src/merging.rs`: `calculate_number_channel_merge`
(`:80-132`) and `calculate_num_channel_diff` (`:299-321`). Both do their arithmetic with the **checked**
`i64` operations, so a value that would leave the signed 64-bit range is *refused* rather than wrapped —
`checked_add` with the message "number channel merge overflow", `checked_sub` with "number channel diff
overflow". This module models exactly those two paths, and nothing else, because the law the catalogue
was carrying here — "numeric channels are non-negative" — was **false of the code**: numeric channels are
signed `i64` and negative diffs are ordinary (`merging.rs:161-166`; the tests at `:349,370` use
`diff: -5` and `-10`). `NonNegI64` is a real newtype in `shared/src/refined.rs:64`, but it types *bonds*
and *heights*, not numeric channels, and the old law conflated them.

## What one of the two paths does *not* do, which the law must therefore not claim

The merge refuses to wrap; the diff *accumulator* does not. Combining two branch index maps adds with a
plain `i64 +=`:

    rspace/src/merger/event_log_index.rs:151      *number_channels.entry(*k).or_insert(0) += *v;
    casper/src/merging.rs:758                     *mergeable_diffs.entry(*k).or_insert(0) += v;

so at the accumulator a debug build panics and a release build wraps. Nothing refuses that addition.
That is a finding about the code rather than a theorem about a model, so it is recorded in
`spec/AUDIT.md` and named in the register's Law 17 row — this module does not dress it up as a law.

## The RNG merge, and why there is no commutativity axiom here

The catalogue used to carry `mergeRandom_comm`. It was false: the primitive's Rust signature is
`Blake2b512Random::merge(children: &[Self])` — n-ary, and order-*sensitive*, pinned by the primitive's
own test (`crypto/src/hash/blake2b512_random.rs:548`, `merge_is_order_sensitive`).

But the *law* the axiom was reaching for is true where it matters: the merge's only caller canonicalizes
first. `merging.rs:121-124` dedups the branch generators and **sorts them by their bytes** before
merging:

    randoms.retain(|r| seen.insert(r.to_bytes()));
    randoms.sort_by_key(|r| r.to_bytes());
    Blake2b512Random::merge(&randoms)

So the merged state is a function of the *set* of branch generators, and `mergeRandoms_perm` below is
that fact proved from `Rchain/Cmp.lean`'s `sortList_perm` — the same sort-then-hash shape as Law 1.
-/

namespace Rchain

/-! ## The checked `i64` arithmetic -/

/-- The signed 64-bit lower bound, as the kernel's `i64::MIN`. -/
def i64Min : Int := -9223372036854775808

/-- The signed 64-bit upper bound, as the kernel's `i64::MAX`. -/
def i64Max : Int := 9223372036854775807

/-- The port's `checked_add`: `none` when the sum leaves the signed 64-bit range, which is how
`calculate_number_channel_merge` refuses to wrap (`rholang/src/merging.rs:102-104`). -/
def checkedAdd (a b : Int) : Option Int :=
  if i64Min ≤ a + b ∧ a + b ≤ i64Max then some (a + b) else none

/-- The port's `checked_sub`, from `calculate_num_channel_diff` (`rholang/src/merging.rs:309-311`). -/
def checkedSub (a b : Int) : Option Int :=
  if i64Min ≤ a - b ∧ a - b ≤ i64Max then some (a - b) else none

/-- The merge **refuses** a sum that leaves `i64` instead of wrapping — the code's
`checked_add(...).ok_or("number channel merge overflow")`. A witness rather than a remark: a
`checkedAdd` that wrapped would satisfy nothing here. -/
theorem checkedAdd_refuses_overflow :
    checkedAdd i64Max 1 = none ∧ checkedAdd i64Min (-1) = none := by
  constructor <;> decide

/-- The same, in the other direction, for the diff computation (`checked_sub`). -/
theorem checkedSub_refuses_overflow :
    checkedSub i64Min 1 = none ∧ checkedSub i64Max (-1) = none := by
  constructor <;> decide

/-- The arithmetic the two functions rely on, in range: the diff and the merge are inverse. Stated over
the kernel's bounds rather than in the abstract, so it is a claim about `checkedAdd`/`checkedSub` and not
about `Int` arithmetic in general. -/
theorem merge_diff_round_trip (init e : Int)
    (hsub : i64Min ≤ e - init ∧ e - init ≤ i64Max)
    (hadd : i64Min ≤ init + (e - init) ∧ init + (e - init) ≤ i64Max) :
    (checkedSub e init).bind (checkedAdd init) = some e := by
  unfold checkedSub
  rw [if_pos hsub]
  simp only [Option.some_bind]
  unfold checkedAdd
  rw [if_pos hadd]
  congr 1
  omega

/-! ## The RNG merge at its call site -/

/-- Merge the branch random generators the way `calculate_number_channel_merge` does: canonicalize
(dedup and sort by bytes) and *then* merge. `Random.state` models the generator's `to_bytes`, so sorting
by `state` is the code's `sort_by_key(|r| r.to_bytes())`.

`noncomputable` because it calls `mergeRandom`, which is an axiom — a modelled primitive has no compiled
code, and this is a definition in the specification rather than something the node executes. -/
noncomputable def mergeRandoms (rs : List Random) : Random :=
  mergeRandom
    ((Comparator.sortList (Comparator.linearOrderComparator Nat) (rs.map (·.state))).eraseDups
      |>.map Random.mk)

/-- **The order-independence the old `mergeRandom_comm` axiom claimed, proved where it is true.** The
primitive is order-sensitive; its caller sorts, so the merged state depends on the *set* of branch
generators and not on the order they arrived in — `sortList_perm` (`Rchain/Cmp.lean:228`) plus
`eraseDups`. -/
theorem mergeRandoms_perm (rs ss : List Random) (h : List.Perm rs ss) :
    mergeRandoms rs = mergeRandoms ss := by
  simp only [mergeRandoms]
  exact congrArg (fun l => mergeRandom (l.eraseDups.map Random.mk))
    (Comparator.sortList_perm (Comparator.linearOrderComparator Nat)
      (List.Perm.map Random.state h))

/-! ## Law 17a — the rejection a conflict set resolves to, and why it is unique

`sdk/src/dag/merging.rs`'s `resolve_conflict_set` (`:395-445`) turns a conflict set into
`(accepted, rejected)`: it closes the conflict map under dependencies, computes the *rejection options*
(settling a conflict means rejecting a branch, together with whatever depends on it), extends them with
whatever an overflow forces, and then picks one — `compute_optimal_rejection` (`:278-295`):

    options.iter().min_by(|a, b| (cost(a), a.len(), a).cmp(&(cost(b), b.len(), b)))

**This row used to say the port "does not choose among candidates, so a claim about a unique
minimum-cost candidate has nothing in the code to be stated against" — that was wrong**, and the line
above is the choice. The law's word *unique* is load-bearing, and not decoration: `min_by` returns an
element of a `BTreeSet`, so the answer is a function of the *set* only if no two distinct options
compare equal. The key `(total cost, size, the sorted set)` is linear, which is why it works —
`optionComparator` below, built from `Rchain.Cmp`'s comparators, whose `eq_iff` is the fact
`the_minimum_is_unique` turns on. -/

/-- A rejection option: the port holds these in a `BTreeSet`, so a **sorted** list here, and a deploy is
    a `Nat` — the resolution never looks inside one. -/
abbrev RejectionOption := List Nat

/-- The total cost of rejecting a set of deploys — the port's `a.iter().map(&target_f).sum()`. Written
    as a `foldl` because this prelude has no `List.sum`. -/
def totalCost (cost : Nat → Int) (o : RejectionOption) : Int :=
  o.foldl (fun acc d => acc + cost d) 0

/-- The port's `min_by` key: total cost, size, and the set itself. -/
def optionKey (cost : Nat → Int) (o : RejectionOption) : Int × Nat × List Nat :=
  (totalCost cost o, o.length, o)

/-- The lexicon on those keys, in the port's order: cost, then size, then the sorted set element-wise. -/
def optionKeyComparator : Comparator (Int × Nat × List Nat) :=
  Comparator.cmpPair (Comparator.linearOrderComparator Int)
    (Comparator.cmpPair (Comparator.linearOrderComparator Nat)
      (Comparator.listComparator (Comparator.linearOrderComparator Nat)))

/-- The port's option order, on options: the key's order pulled back along `optionKey`. The three laws
    come from the key's comparator, `eq_iff` using that the key *contains* the option (its third
    component), which is exactly why no two distinct options can compare equal. -/
def optionComparator (cost : Nat → Int) : Comparator RejectionOption where
  cmp a b := optionKeyComparator.cmp (optionKey cost a) (optionKey cost b)
  eq_iff := by
    intro a b
    constructor
    · intro h
      have hk : optionKey cost a = optionKey cost b := optionKeyComparator.eq_iff.mp h
      exact congrArg (fun t : Int × Nat × List Nat => t.2.2) hk
    · intro h; subst h; exact optionKeyComparator.eq_iff.mpr rfl
  swap := by intro a b; exact optionKeyComparator.swap
  lt_trans := by intro a b c h1 h2; exact optionKeyComparator.lt_trans h1 h2

/-- `compute_optimal_rejection`'s fold. It keeps the *last* of equal minima where Rust's `min_by` keeps
    the first — invisible, because comparing equal under `optionComparator` means being the *same set*
    (`eq_iff`), which is the fact the uniqueness theorem below states. -/
def pickRejection (cost : Nat → Int) : List RejectionOption → Option RejectionOption
  | [] => none
  | o :: os =>
      match pickRejection cost os with
      | none => some o
      | some m => some (if (optionComparator cost).cmp o m = Ordering.lt then o else m)

/-- The fold answers `none` exactly when it was given nothing. -/
theorem pickRejection_eq_none_iff (cost : Nat → Int) (os : List RejectionOption) :
    pickRejection cost os = none ↔ os = [] := by
  constructor
  · intro h
    cases os with
    | nil => rfl
    | cons o os =>
        cases hp : pickRejection cost os with
        | none => exact absurd h (by simp [pickRejection, hp])
        | some m => exact absurd h (by simp [pickRejection, hp])
  · intro h; subst h; rfl

/-- The fold returns an option it was given. -/
theorem pickRejection_mem (cost : Nat → Int) : ∀ (os : List RejectionOption) (m : RejectionOption),
    pickRejection cost os = some m → m ∈ os := by
  intro os
  induction os with
  | nil => intro m h; simp [pickRejection] at h
  | cons o os ih =>
      intro m h
      cases hp : pickRejection cost os with
      | none =>
          simp only [pickRejection, hp] at h
          injection h with h'; subst h'
          exact List.mem_cons_self ..
      | some m' =>
          by_cases hlt : (optionComparator cost).cmp o m' = Ordering.lt
          · simp only [pickRejection, hp, if_pos hlt] at h
            injection h with h'; subst h'
            exact List.mem_cons_self ..
          · simp only [pickRejection, hp, if_neg hlt] at h
            injection h with h'; subst h'
            exact List.mem_cons_of_mem _ (ih m' hp)

/-- **It is a minimum**: no option the fold saw is smaller. -/
theorem pickRejection_minimal (cost : Nat → Int) : ∀ (os : List RejectionOption) (m : RejectionOption),
    pickRejection cost os = some m → ∀ o ∈ os, (optionComparator cost).le m o := by
  intro os
  induction os with
  | nil => intro m h; simp [pickRejection] at h
  | cons o os ih =>
      intro m h
      cases hp : pickRejection cost os with
      | none =>
          have hos : os = [] := (pickRejection_eq_none_iff cost os).mp hp
          subst hos
          simp only [pickRejection, hp] at h
          injection h with h'; subst h'
          intro o' ho'
          rw [List.mem_singleton] at ho'
          subst ho'
          exact Comparator.le_refl _ _
      | some m' =>
          have hmin : ∀ o' ∈ os, (optionComparator cost).le m' o' := ih m' hp
          by_cases hlt : (optionComparator cost).cmp o m' = Ordering.lt
          · simp only [pickRejection, hp, if_pos hlt] at h
            injection h with h'; subst h'
            intro o' ho'
            rcases List.mem_cons.mp ho' with rfl | hmem
            · exact Comparator.le_refl _ _
            · exact Comparator.le_trans _ (Or.inl hlt) (hmin o' hmem)
          · simp only [pickRejection, hp, if_neg hlt] at h
            injection h with h'; subst h'
            intro o' ho'
            rcases List.mem_cons.mp ho' with rfl | hmem
            · exact Comparator.le_of_not_lt _ hlt
            · exact hmin o' hmem

/-- **The key's third component is load-bearing.** Two options can agree on cost *and* size — the key's
    first two components — and the set then separates them. Drop it and the comparison is not linear,
    `min_by` returns whichever the `BTreeSet` happened to yield first, and the resolution stops being a
    function of the conflict set. This is the port's own case (`compute_optimal_rejection_minimizes_
    cost_then_size`, where every deploy costs 1 and `{1}` must win). -/
theorem equal_cost_and_size_do_not_make_equal_options :
    totalCost (fun _ => 1) ([1, 2] : RejectionOption) = totalCost (fun _ => 1) [1, 3] ∧
    ([1, 2] : RejectionOption).length = ([1, 3] : RejectionOption).length ∧
    (optionComparator (fun _ => 1)).cmp [1, 2] [1, 3] = Ordering.lt := by
  refine ⟨?_, ?_, ?_⟩ <;> decide

/-- **Law 17a — the minimum is unique.** Two options that are both minimal for the same set are the
    same option. This is `le_antisymm` of the port's own key, and it is why `min_by` over a `BTreeSet`
    is a function of the set: **the iteration order cannot be observed**. -/
theorem the_minimum_is_unique (cost : Nat → Int) {os : List RejectionOption}
    {m₁ m₂ : RejectionOption} (h₁ : m₁ ∈ os) (hm₁ : ∀ o ∈ os, (optionComparator cost).le m₁ o)
    (h₂ : m₂ ∈ os) (hm₂ : ∀ o ∈ os, (optionComparator cost).le m₂ o) : m₁ = m₂ :=
  Comparator.le_antisymm _ (hm₁ m₂ h₂) (hm₂ m₁ h₁)

/-- **And that is what the port relies on**: two orderings of the same options resolve to the same
    rejection, because each is a minimum of the same set. The proof is `the_minimum_is_unique` — there
    is no appeal to the iteration order anywhere. -/
theorem the_resolution_does_not_depend_on_the_iteration_order (cost : Nat → Int)
    {os os' : List RejectionOption} (h : List.Perm os os') :
    pickRejection cost os = pickRejection cost os' := by
  cases hsome : pickRejection cost os with
  | none =>
      have hos : os = [] := (pickRejection_eq_none_iff cost os).mp hsome
      cases os' with
      | nil => rw [← hsome, hos]
      | cons a as =>
          rw [hos] at h
          exact absurd h.length_eq (by simp)
  | some m =>
      cases hsome' : pickRejection cost os' with
      | none =>
          have hos' : os' = [] := (pickRejection_eq_none_iff cost os').mp hsome'
          have hos : os = [] := by
            cases os with
            | nil => rfl
            | cons a as =>
                rw [hos'] at h
                exact absurd h.length_eq (by simp)
          rw [hos] at hsome
          exact absurd hsome (by simp [pickRejection])
      | some m' =>
          have hmem : m ∈ os := pickRejection_mem cost os m hsome
          have hmin : ∀ o ∈ os, (optionComparator cost).le m o :=
            pickRejection_minimal cost os m hsome
          have hmem' : m' ∈ os' := pickRejection_mem cost os' m' hsome'
          have hmin' : ∀ o ∈ os', (optionComparator cost).le m' o :=
            pickRejection_minimal cost os' m' hsome'
          have heq : m = m' :=
            the_minimum_is_unique cost hmem hmin (h.symm.mem_iff.mp hmem')
              (fun o ho => hmin' o (h.mem_iff.mp ho))
          rw [heq]

/-! ## Law 17c — the native relation is on the chain, not the block

The rows above model the merge's **selection**. This one models the relation the selection runs over,
because that relation is where #280's defect lived and because the choice of *what a rejection unit is*
is not a detail of the search — it decides what can be lost.

The port carries each block's native state changes as `BlockIndex.native_changes`, a flat
`Vec<NativeStoreAction>` (`casper/src/merging.rs`). The relation that decides conflicts reads that
**host block's** set — `NativeRelations::conflicting` takes the union over the block's chains — and the
rejection rule is over the host too: `reject_whole_blocks` removes every chain of a rejected block, on
the stated grounds that "native effects are block-level and were computed with every chain's effects".
At an epoch boundary every block runs `close_block`, so every block's host set holds the epoch's slots
and **every** pair of concurrent blocks conflicts — including the pairs whose user chain wrote no
contended slot at all. The user deploy's effects then die with a contention they had no part in, which is
exactly what the live capture records (`spec/audit/evidence/n280-merge-loses-a-write-results.md`: the
write present at heights 100, 101 and 102, gone from 103, with the boundary blocks' `CloseBlock` ids in
the rejected set).

**What this law does not model.** The rejection-*option enumeration* over the conflict graph — the
maximal conflict-free sets the search returns, and the budget that bounds it — is still not modelled
here; it is the piece law 17a's note names as law 9's and it would be a second model of
`sdk/src/dag/merging.rs`'s `compute_rejection_options`. What is modelled is the relation and the
acceptance predicate, which is what the defect falsified. -/

/-- One deploy chain, as the native relation sees it: the block it rode in on, and the native slots its
    **own** deploys wrote. `host` stands for the port's `host_block`, `slots` for the `(prefix, key)`
    pairs its actions touch (`NativeStoreAction::slot`) folded into a set. -/
structure Chain where
  host : Nat
  slots : Finset Nat
  deriving DecidableEq

/-- The native relation at the **chain** level: two chains of different blocks, sharing a slot, neither
    having seen the other. `sees` is a parameter rather than a field because it comes from the DAG's
    ancestry (`MergeScope::ancestry`) and is not a function of the two chains — the same reason the
    port's `NativeRelations` reads it out of the scope rather than off the chains. -/
def chainConflict (sees : Chain → Chain → Prop) (a b : Chain) : Prop :=
  a.host ≠ b.host ∧ (a.slots ∩ b.slots).Nonempty ∧ ¬ sees a b ∧ ¬ sees b a

/-- The slots written by **any** chain that rode in on block `h` — the port's
    `native_keys: BTreeMap<host, BTreeSet<slot>>`, built by unioning the block's chains. -/
def hostSlots (cs : List Chain) (h : Nat) : Finset Nat :=
  cs.foldl (fun acc c => if c.host = h then acc ∪ c.slots else acc) ∅

/-- The native relation at the **block** level — what the port applied before this law, and what a
    chain is rejected by. Note that it says nothing about *which* chain wrote the shared slot: that is
    the whole of the defect. -/
def hostConflict (cs : List Chain) (a b : Chain) : Prop :=
  a.host ≠ b.host ∧ (hostSlots cs a.host ∩ hostSlots cs b.host).Nonempty

/-- **The incident's shape, as data.** One block (host `1`) carried two chains: its boundary chain,
    which ran `close_block` and wrote the epoch's slots `{1, 2}`, and its user deploy's chain, which
    wrote **no native slot at all** (`∅`). A concurrent sibling (host `2`) wrote the same two slots.
    This is the round the live chain ran at height 100. -/
def incidentChains : List Chain :=
  [⟨1, {1, 2}⟩, ⟨1, ∅⟩, ⟨2, {1, 2}⟩]

/-- **The rule that stood, refuted.** At the block level the user chain conflicts with the sibling —
    different hosts, and the *host* key sets overlap — while at the chain level it does not, because its
    own slot set is empty. So the rule that stood rejects a chain that wrote nothing contended, and that
    is how a user deploy's effects came to die for a slot they never touched. The two sides here are the
    falsifier: `hostConflict` is the shape the port applied and `¬ chainConflict` is the shape this law
    requires, of the *same* pair of chains. -/
theorem host_keys_conflict_where_the_chains_do_not (sees : Chain → Chain → Prop) :
    hostConflict incidentChains ⟨1, ∅⟩ ⟨2, {1, 2}⟩ ∧
      ¬ chainConflict sees ⟨1, ∅⟩ ⟨2, {1, 2}⟩ := by
  constructor
  · unfold hostConflict hostSlots incidentChains
    decide
  · intro h
    exact absurd h.2.1 (by simp)

/-- **The fold's refusal**, as the port computes it: two accepted chains that both wrote one slot and
    that the order cannot sequence. The port sequences two writers of one slot only when one saw the
    other — a descendant's absolute value already includes its ancestor's — or when they are chains of
    **one** block, where the block's own execution order decides. That second arm is what this law's fix
    adds (`first_deploy_ordinal`); the block-level rule had no need of it because it never applied two
    chains of one block separately. -/
def foldRefuses (sees : Chain → Chain → Prop) (kept : List Chain) : Prop :=
  ∃ a ∈ kept, ∃ b ∈ kept, a ≠ b ∧ (a.slots ∩ b.slots).Nonempty ∧
    ¬ sees a b ∧ ¬ sees b a ∧ a.host ≠ b.host

/-- **The resolution's own guarantee**: no two kept chains are in conflict. This is the predicate the
    search hands the fold, and it is witnessed independently on the Rust side
    (`sdk/src/property_tests.rs:law17_the_survivors_of_a_rejection_option_are_conflict_free`), so the two
    predicates below are not two spellings of one thing. -/
def resolves (sees : Chain → Chain → Prop) (kept : List Chain) : Prop :=
  ∀ a ∈ kept, ∀ b ∈ kept, ¬ chainConflict sees a b

/-- **Law 17c's positive half: the resolution leaves the fold nothing to refuse.** `resolves` is stated
    on `chainConflict` and `foldRefuses` on its own shared-slot-and-unsequenced shape, so this is a bridge
    between two independently defined predicates rather than a restatement of either: a kept pair that
    shares a slot and is unsequenced *is* a chain conflict, which `resolves` forbids. It is what lets the
    fold's refusal be made **loud** rather than defensively swallowed — after this law the refusal is
    unreachable on any state the resolution can produce, so a report of one is a defect and not a
    judgement call. -/
theorem the_resolution_leaves_the_fold_nothing_to_refuse (sees : Chain → Chain → Prop)
    {kept : List Chain} (h : resolves sees kept) : ¬ foldRefuses sees kept := by
  rintro ⟨a, ha, b, hb, _, hslots, hsa, hsb, hhost⟩
  exact h a ha b hb ⟨hhost, hslots, hsa, hsb⟩

/-- **And the refusal is not an empty predicate**, which is what makes the theorem above a statement
    about a reachable shape rather than about nothing: two chains of different blocks that wrote one slot
    without either having seen the other are exactly what the fold refuses. (`hostSlotWriters` in the
    port is this pair; the fix's job is to keep such a pair out of the *kept* set, not to make the
    refusal unstatable.) -/
theorem the_fold_refuses_concurrent_writers_of_one_slot :
    foldRefuses (fun _ _ => False) [⟨1, {1}⟩, ⟨2, {1}⟩] := by
  refine ⟨⟨1, {1}⟩, by simp, ⟨2, {1}⟩, by simp, by decide, by decide, by simp, by simp, by decide⟩

/-! ## Law 17e — an unchanged native write is not a write

Law 17c puts the native relation on the chain and on the slots that chain wrote. It leaves one thing
open that the port then got wrong: **which slots count as written**. `NativeStore::put` records a
`Put` even when the bytes equal what the pre-state holds, so `pos:slash-votes`, `pos:vote-windows`
and `pos:liveness` — which the armed liveness prelude rewrites at every advancing boundary — entered
a boundary chain's slot set although the boundary changed nothing.

A no-op write in that set is a conflict partner of a concurrent user chain that did write the leaf.
When the boundary is finalised and the user chain is not, the port's `incompatible_with_final` forces
the user chain out, and its deploy's effect dies with a boundary that wrote the same bytes back. That
is C344's exact-threshold arm losing validator 1's ballot, reduced to
`casper/src/merging.rs::a_finalized_boundary_noop_does_not_reject_a_concurrent_ballot`.

The port's fix is `effective_native_effects` at index construction: walk the deploys in order against
a local view seeded from the pre-state and keep only the writes that change a value. This law states
the value-level fact that rule implements. -/

/-- One native write: its slot, the value the pre-state (or an earlier deploy's write) held, and the
    value it leaves. `Option Nat` is the port's `Option<Vec<u8>>`; `none` is an absent leaf. -/
structure NativeWrite where
  slot : Nat
  before : Option Nat
  after : Option Nat
  deriving DecidableEq

/-- A write that leaves the slot as it found it. -/
def NativeWrite.isNoop (w : NativeWrite) : Prop := w.before = w.after

/-- The slots a write list **effectively** writes: those whose value changed. The port walks the
    writes in deploy order, so a later write's `before` is what an earlier write left — which is why
    a later deploy restoring an earlier value is itself effective and must be kept. -/
def effectiveSlots (ws : List NativeWrite) : Finset Nat :=
  (ws.filter (fun w => w.before != w.after)).foldl (fun s w => s ∪ {w.slot}) ∅

/-- Every slot the list touches, changed or not — the rule that stood before this law. -/
def allSlots (ws : List NativeWrite) : Finset Nat :=
  ws.foldl (fun s w => s ∪ {w.slot}) ∅

/-- **A no-op write is not a conflict partner.** A chain whose only write to a slot leaves the value
    unchanged is not in conflict with a concurrent chain that really writes that slot: its effective
    slot set is empty, so `chainConflict` has nothing to overlap. -/
def law17e_statement : Prop :=
  ∀ (sees : Chain → Chain → Prop) (w : NativeWrite), w.isNoop →
    ¬ chainConflict sees ⟨1, effectiveSlots [w]⟩ ⟨2, allSlots [w]⟩

theorem law17e : law17e_statement := by
  intro sees w h
  simp only [NativeWrite.isNoop] at h
  simp [chainConflict, effectiveSlots, h]

/-- **The rule it replaces, refuted on the same pair of chains.** With the no-op write counted by
    its slot (`allSlots`), the two chains are in conflict; with its effective set (`effectiveSlots`,
    empty), they are not. This is the falsifier: drop the value comparison from `effectiveSlots` and
    the port is back to rejecting a chain for bytes it did not change. -/
theorem all_writes_conflict_where_the_effective_ones_do_not :
    chainConflict (fun _ _ => False)
        ⟨1, allSlots [⟨7, some 1, some 1⟩]⟩
        ⟨2, allSlots [⟨7, some 1, some 1⟩]⟩ ∧
      ¬ chainConflict (fun _ _ => False)
        ⟨1, effectiveSlots [⟨7, some 1, some 1⟩]⟩
        ⟨2, allSlots [⟨7, some 1, some 1⟩]⟩ := by
  constructor
  · simp [chainConflict, allSlots]
  · simp [chainConflict, effectiveSlots, allSlots]

end Rchain
