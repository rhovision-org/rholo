import Rchain.Effect
import Rchain.RSpace.Merge
import Rchain.Progress
import Rchain.Bounded

/-!
# Law 86 — admission is a bounded queue; execution is a concurrent schedule

The public testnet settled into `throttled` with a free pool and refused every deploy and explore
request with a 503 (issue #477). The defect was not the memory reading: it was a **boolean gate in
front of a queue**. A node that cannot execute now should hold the work and drain it when it can, and
a node that cannot drain it should hand it to a peer; refusing the whole world is neither admission
control nor backpressure.

This module states the contract in five clauses. It is deliberately about *policy shapes*: the
pool's bounds, the statuses an entry may move through, the independence criterion a concurrent batch
must satisfy, the permit budget that bounds execution, and a relay that copies rather than moves.

* **86a** — admission is a bounded queue, and the decision reads only the pool's bounds. A free slot
  admits; a full pool refuses with a reason. The legacy boolean gate is modelled as `legacyAdmit`,
  and `the_legacy_gate_refuses_an_empty_slot` is the height-1688 state as a theorem.
* **86b** — no admitted work is silently lost. A step changes an entry's status; it never removes the
  row. The only retirement is the stated expiry transition, and every refusal changes nothing.
* **86c** — independent work commutes. At the effect level the criterion is disjoint **closures**
  (`effect_commute_of_disjoint_closure`); at the state-change level it is `NonConflicting` and the
  existing `mergeChanges_comm`/`mergeChanges_assoc`. The batch theorems below are the fold forms the
  executor needs.
* **86d** — one permit progresses, and the permit budget bounds the **batch**, not the admission
  answer. The defect shape (`needsTwoPermits`) is a non-empty pool with one permit and no step.
* **86e** — pending work is relocatable: `relay` copies an entry to a peer and leaves the sender's row
  alone, so a refused or lost message cannot lose work.

The bridge from the Rust's dynamic read/write `TouchLog` to 86c's `NonConflicting` is a refinement
obligation, not a Lean theorem; the register's 86c row is `proved-model` and says so.
-/

namespace Rchain
namespace Admission

/-! ## The pool and admission (86a, 86b) -/

/-- A deploy identifier. The Rust's `DeployId` is a hash; a `Nat` is enough for the policy model. -/
abbrev DeployId := Nat

/-- The statuses a pooled entry may carry. `done` and `expired` are terminal **retirements**; neither
    removes the row, so a ledger can answer "what happened to this deploy?" after the fact. -/
inductive PoolStatus where
  | pending
  | running
  | done
  | expired
  deriving DecidableEq, Repr

structure Entry where
  id : DeployId
  status : PoolStatus
  deriving DecidableEq, Repr

/-- A bounded pool. `capacity` is the count bound; the Rust's byte and per-deployer bounds are the
    same shape (a `hasSlot` predicate) and are driven by the conformance corpus, not restated here. -/
structure Pool where
  capacity : Nat
  entries : List Entry
  deriving Repr

/-- The pool holds this id. A `Bool` because `enqueue` is a decision procedure, not a proposition. -/
def Pool.hasId (p : Pool) (id : DeployId) : Bool :=
  p.entries.any (fun e => e.id == id)

/-- There is room for another entry under the count bound. -/
def Pool.hasSlot (p : Pool) : Bool :=
  p.entries.length < p.capacity

/-- Why an admission was refused. `duplicate` is not a loss: the id is already in the pool. -/
inductive Refusal where
  | full
  | duplicate
  | share
  | rate
  deriving DecidableEq, Repr

inductive AdmitResult where
  | admitted (p : Pool)
  | refused (p : Pool) (r : Refusal)
  deriving Repr

/-- The resource word. It is **not** an input to `enqueue`; it is an input to the permit budget
    (86d). `legacyAdmit` below is the gate this law removes. -/
inductive ResourceState where
  | normal
  | throttled
  | critical
  | unavailable
  deriving DecidableEq, Repr

/-- **The admission decision.** Only the pool's own bounds are consulted: an id already present is a
    duplicate, a free slot admits, a full pool refuses. There is no resource-state parameter, and
    that is 86a's structural half. -/
def enqueue (p : Pool) (id : DeployId) : AdmitResult :=
  if p.hasId id then
    .refused p .duplicate
  else if p.hasSlot then
    .admitted { p with entries := p.entries ++ [{ id := id, status := .pending }] }
  else
    .refused p .full

/-- **The defect this law removes**: a boolean resource gate that refuses with a free slot. It is
    kept in the model so 86a is a claim about a *different* rule, not a restatement of `enqueue`. -/
def legacyAdmit (r : ResourceState) (p : Pool) (id : DeployId) : AdmitResult :=
  if r = .normal then enqueue p id else .refused p .full

/-- The id is in the ledger (whatever its status). -/
def Accepted (p : Pool) (id : DeployId) : Prop :=
  ∃ e ∈ p.entries, e.id = id

/-- **86a, the admission half.** A free slot with a new id admits. -/
theorem an_empty_slot_admits (p : Pool) (id : DeployId)
    (hslot : p.hasSlot = true) (hnew : p.hasId id = false) :
    ∃ p', enqueue p id = .admitted p' := by
  refine ⟨{ p with entries := p.entries ++ [{ id := id, status := .pending }] }, ?_⟩
  simp [enqueue, hnew, hslot]

/-- **86a, the refusal half.** A full pool refuses with the named reason. -/
theorem a_full_pool_refuses (p : Pool) (id : DeployId)
    (hfull : p.hasSlot = false) (hnew : p.hasId id = false) :
    enqueue p id = .refused p .full := by
  simp [enqueue, hnew, hfull]

/-- **86a's falsifier, in the tree.** The legacy gate refuses a deploy it could have admitted: an
    empty pool, a free slot, and `throttled` — the height-1688 state, stated as a theorem. -/
theorem the_legacy_gate_refuses_an_empty_slot :
    ∃ (p : Pool) (id : DeployId),
      p.hasSlot = true ∧ p.hasId id = false ∧
        legacyAdmit .throttled p id = .refused p .full := by
  refine ⟨{ capacity := 1, entries := [] }, 0, ?_, ?_, ?_⟩
  · simp [Pool.hasSlot]
  · simp [Pool.hasId]
  · simp [legacyAdmit]

/-- A refusal does not change the pool: `duplicate` and `full` return the pool they were given. This
    is 86b's "a refusal is observable and loses nothing". -/
theorem a_refusal_changes_nothing (p p' : Pool) (id : DeployId) (r : Refusal)
    (h : enqueue p id = .refused p' r) : p' = p := by
  unfold enqueue at h
  split at h
  · cases h
    rfl
  · split at h
    · cases h
    · cases h
      rfl

/-- One entry's status may advance: pending → running → done, or pending → expired. No other change
    is a step, and in particular no status disappears. -/
def StatusStep : PoolStatus → PoolStatus → Prop
  | .pending, .running => True
  | .running, .done => True
  | .pending, .expired => True
  | s, s' => s = s'

/-- A pool step: capacities agree and entries correspond position by position, each advancing by
    `StatusStep`. **No step removes an entry**, which is 86b's mechanism. -/
def Step (p p' : Pool) : Prop :=
  p.capacity = p'.capacity ∧
    List.Forall₂ (fun e e' => e.id = e'.id ∧ StatusStep e.status e'.status) p.entries p'.entries

/-- A `Forall₂` over entry lists preserves accepted ids. Stated over list variables so the
    induction is on a variable index. -/
theorem accepted_of_forall₂ {l l' : List Entry}
    (h : List.Forall₂ (fun e e' => e.id = e'.id ∧ StatusStep e.status e'.status) l l')
    {id : DeployId} (ha : ∃ e ∈ l, e.id = id) : ∃ e' ∈ l', e'.id = id := by
  induction h with
  | nil => rcases ha with ⟨e, he, -⟩; cases he
  | cons hhead _ ih =>
      rcases ha with ⟨e, he, hid⟩
      cases he with
      | head =>
          exact ⟨_, by simp, hhead.1.symm.trans hid⟩
      | tail _ hmem =>
          rcases ih ⟨_, hmem, hid⟩ with ⟨e', he', hid'⟩
          exact ⟨e', by simp [he'], hid'⟩

/-- A step preserves every accepted id. -/
theorem accepted_of_step {p p' : Pool} (h : Step p p') {id : DeployId}
    (ha : Accepted p id) : Accepted p' id := by
  rcases h with ⟨-, hlist⟩
  exact accepted_of_forall₂ hlist ha

/-- **86b.** No admitted work is silently lost: every id accepted at the start is in the ledger at
    every reachable state. The status may have advanced to `done` or `expired`, but the row remains,
    so the retirement is observable. -/
theorem no_admitted_work_is_lost (p0 : Pool) (id : DeployId) :
    Accepted p0 id → ∀ p, Relation.ReflTransGen Step p0 p → Accepted p id := by
  intro ha p hr
  induction hr with
  | refl => exact ha
  | tail _ hstep ih => exact accepted_of_step hstep ih

/-- **86b's falsifier, in the tree.** A pool that *removes* a pending entry on pressure loses work:
    the defect shape the old gate had, stated as a reachable state. -/
def dropOnPressure (p : Pool) (id : DeployId) : Pool :=
  { p with entries := p.entries.filter (fun e => e.id != id) }

theorem a_dropping_pool_loses_work :
    ∃ (p : Pool) (id : DeployId),
      Accepted p id ∧ ¬ Accepted (dropOnPressure p id) id := by
  refine ⟨{ capacity := 1, entries := [{ id := 0, status := .pending }] }, 0, ?_, ?_⟩
  · exact ⟨{ id := 0, status := .pending }, by simp, rfl⟩
  · simp [Accepted, dropOnPressure]

/-! ## The permit budget and progress (86d) -/

/-- A node is its pool and its execution budget. The budget bounds how many entries a step may take;
    it does not appear in `enqueue`, which is the whole of 86d's "capacity changes the schedule, not
    the queue". -/
structure Node where
  pool : Pool
  permits : Nat

/-- The pending ids in ledger order. -/
def pendingIds (p : Pool) : List DeployId :=
  (p.entries.filter (fun e => e.status == .pending)).map (fun e => e.id)

/-- Mark a batch's ids running, leaving every other entry alone. -/
def markRunning (p : Pool) (ids : List DeployId) : Pool :=
  { p with
    entries := p.entries.map (fun e =>
      if ids.contains e.id then { e with status := .running } else e) }

/-- The batch a step would take: up to `permits` pending ids. -/
def scheduleBatch (n : Node) : List DeployId :=
  let ids := pendingIds n.pool
  ids.take (min n.permits ids.length)

/-- One scheduling step: take the pending batch and mark it running. `none` exactly when the batch
    is empty (no pending work, or no permit). -/
def next (n : Node) : Option (List DeployId × Node) :=
  let batch := scheduleBatch n
  if batch = [] then
    none
  else
    some (batch, { n with pool := markRunning n.pool batch })

/-- A node step is a non-empty batch from `next`. -/
def NodeStep (n n' : Node) : Prop :=
  ∃ batch, next n = some (batch, n') ∧ batch ≠ []

/-- **86d, progress.** A non-empty pool with at least one permit has a step: the node cannot sit
    idle with work it could have started. -/
theorem one_permit_progresses (n : Node) (hpending : pendingIds n.pool ≠ [])
    (hpermits : 1 ≤ n.permits) : ∃ n', NodeStep n n' := by
  have hbatch : scheduleBatch n ≠ [] := by
    unfold scheduleBatch
    cases hids : pendingIds n.pool with
    | nil => exact absurd hids hpending
    | cons x xs =>
        have hk : 0 < min n.permits (x :: xs).length :=
          Nat.lt_min.mpr ⟨hpermits, by simp⟩
        cases hmin : min n.permits (x :: xs).length with
        | zero => exact absurd hmin (Nat.ne_of_gt hk)
        | succ k =>
            have hperm0 : n.permits ≠ 0 := by
              have hle : k + 1 ≤ n.permits := by
                rw [← hmin]
                exact Nat.min_le_left _ _
              exact Nat.ne_of_gt (Nat.lt_of_lt_of_le (Nat.succ_pos k) hle)
            simp [List.take_succ_cons, hperm0]
  refine ⟨{ n with pool := markRunning n.pool (scheduleBatch n) }, ?_⟩
  refine ⟨scheduleBatch n, ?_, hbatch⟩
  simp [next, hbatch]

/-- **86d, the budget bounds the batch.** -/
theorem scheduleBatch_length_le_permits (n : Node) :
    (scheduleBatch n).length ≤ n.permits := by
  unfold scheduleBatch
  rw [List.length_take]
  exact le_trans (Nat.min_le_left _ _) (Nat.min_le_left _ _)

/-- **86d, the budget bounds the batch.** Capacity changes how much is scheduled, never whether an
    admission is accepted. -/
theorem the_batch_is_bounded_by_the_permits (n n' : Node) (batch : List DeployId)
    (h : next n = some (batch, n')) : batch.length ≤ n.permits := by
  by_cases hb : scheduleBatch n = []
  · simp [next, hb] at h
  · simp [next, hb] at h
    rw [← h.1]
    exact scheduleBatch_length_le_permits n

/-- **86d's falsifier, in the tree.** A scheduler that needs two permits stalls a non-empty pool
    with one permit — "throttled but nothing can happen", the shape of the live node. -/
def needsTwoPermits (n n' : Node) : Prop :=
  2 ≤ n.permits ∧ NodeStep n n'

theorem a_scheduler_that_needs_two_permits_stalls :
    ∃ n : Node,
      pendingIds n.pool ≠ [] ∧ n.permits = 1 ∧ ¬ ∃ n', needsTwoPermits n n' := by
  refine ⟨{ pool := { capacity := 1, entries := [{ id := 0, status := .pending }] },
            permits := 1 }, ?_, rfl, ?_⟩
  · simp [pendingIds]
  · rintro ⟨n', h2, -⟩
    exact Nat.lt_irrefl 1 (Nat.succ_le_iff.mp h2)

/-! ## Relocation across nodes (86e) -/

/-- Two pools, as a relay sees them. -/
structure TwoNodes where
  left : Pool
  right : Pool

/-- A relay **copies** a pending id to the right pool when it has room and the id is new; the left
    pool is never changed. The condition is a `Bool` so the function is computable. -/
def relay (n : TwoNodes) (id : DeployId) : TwoNodes :=
  if n.left.hasId id && n.right.hasSlot && !n.right.hasId id then
    { n with
      right := { n.right with
        entries := n.right.entries ++ [{ id := id, status := .pending }] } }
  else
    n

/-- **86e, the sender keeps its row.** A relay never removes work from the origin. -/
theorem relay_preserves_the_sender (n : TwoNodes) (relayed : DeployId) (id : DeployId) :
    Accepted (relay n relayed).left id ↔ Accepted n.left id := by
  unfold relay
  split <;> rfl

/-- **86e, the receiver gets a copy** when it has room. -/
theorem relay_copies_to_the_receiver (n : TwoNodes) (id : DeployId)
    (hleft : n.left.hasId id = true) (hslot : n.right.hasSlot = true)
    (hnew : n.right.hasId id = false) :
    Accepted (relay n id).right id := by
  refine ⟨{ id := id, status := .pending }, ?_, rfl⟩
  simp [relay, hleft, hslot, hnew]

/-- **86e, no side loses a row.** A relay leaves both pools' accepted ids intact: the left pool is
    unchanged, and the right is only ever extended. -/
theorem relay_preserves_both_pools (n : TwoNodes) (id : DeployId) :
    (∀ id', Accepted n.left id' → Accepted (relay n id).left id') ∧
      (∀ id', Accepted n.right id' → Accepted (relay n id).right id') := by
  constructor
  · intro id' h
    rw [relay_preserves_the_sender n id id']
    exact h
  · intro id' h
    unfold relay
    split
    · exact ⟨_, List.mem_append_left _ h.choose_spec.1, h.choose_spec.2⟩
    · exact h

/-- **86e's falsifier, in the tree.** A relay that **moves** (deletes from the sender and drops when
    the receiver is full) loses work. The broken variant is the reason the law says copy. -/
def moveRelay (n : TwoNodes) (id : DeployId) : TwoNodes :=
  { left := { n.left with entries := n.left.entries.filter (fun e => e.id != id) }
    right := n.right }

theorem a_moving_relay_loses_work :
    ∃ (n : TwoNodes) (id : DeployId),
      Accepted n.left id ∧ ¬ Accepted (moveRelay n id).left id ∧
        ¬ Accepted (moveRelay n id).right id := by
  refine ⟨{ left := { capacity := 1, entries := [{ id := 0, status := .pending }] },
            right := { capacity := 1, entries := [{ id := 1, status := .pending }] } },
          0, ?_, ?_, ?_⟩
  · exact ⟨{ id := 0, status := .pending }, by simp, rfl⟩
  · simp [moveRelay, Accepted]
  · simp [moveRelay, Accepted]

/-! ## Independence and the batch (86c) -/

/-- The effect-level independence criterion: **disjoint closures**, not disjoint footprints. This is
    the strengthened Law 9 condition the scheduler must enforce. -/
def Independent (e1 e2 : Effect) : Prop := e1.closure ∩ e2.closure = ∅

/-- Two independent effects commute (the existing Law 9 theorem, named at this level). -/
theorem independent_pair_commutes (e1 e2 : Effect) (s : State)
    (h : Independent e1 e2) : e1.apply (e2.apply s) = e2.apply (e1.apply s) :=
  effect_commute_of_disjoint_closure e1 e2 s h

/-- An effect independent of every effect in a list can be moved across the list's fold: the batch
    may schedule it before or after the rest. -/
theorem independent_prefix_commutes (e : Effect) (es : List Effect) (s : State)
    (h : ∀ e' ∈ es, Independent e e') :
    e.apply (es.foldl (fun acc e' => e'.apply acc) s) =
      es.foldl (fun acc e' => e'.apply acc) (e.apply s) := by
  induction es generalizing s with
  | nil => rfl
  | cons e' es ih =>
      simp only [List.foldl_cons]
      rw [ih (e'.apply s) (fun x hx => h x (by simp [hx]))]
      rw [independent_pair_commutes e e' s (h e' (by simp))]

/-- An empty state change: no channel added or removed, no join written. -/
def emptyChange : StateChange :=
  ⟨fun _ => [], fun _ => [], fun _ => none⟩

/-- `NonConflicting` is symmetric: it is a claim about two sets of touched channels. -/
theorem nonConflicting_symm {x y : StateChange} (h : NonConflicting x y) :
    NonConflicting y x := by
  intro c hc
  exact h c ⟨hc.2, hc.1⟩

/-- An independent state change can be moved **past** a fold of changes: applying it after the batch
    reaches the same state as applying it before. This is the state-change form of 86c, and the one
    the Rust executor's certificate is written against. -/
theorem independent_change_commutes_with_a_fold (x : StateChange) (ys : List StateChange)
    (s : StateChange) (h : ∀ y ∈ ys, NonConflicting x y) :
    mergeChanges (ys.foldl mergeChanges s) x =
      ys.foldl mergeChanges (mergeChanges s x) := by
  induction ys generalizing s with
  | nil => rfl
  | cons y ys ih =>
      simp only [List.foldl_cons]
      rw [ih (mergeChanges s y) (fun z hz => h z (by simp [hz]))]
      congr 1
      calc
        mergeChanges (mergeChanges s y) x = mergeChanges s (mergeChanges y x) :=
          mergeChanges_assoc s y x
        _ = mergeChanges s (mergeChanges x y) := by
          rw [mergeChanges_comm y x (nonConflicting_symm (h y (by simp)))]
        _ = mergeChanges (mergeChanges s x) y := (mergeChanges_assoc s x y).symm

/-- Two batches whose members are pairwise non-conflicting commute, with an arbitrary accumulator.
    The executor may therefore merge a certified batch in any order and reach the same state;
    `mergeChanges_assoc` plus this theorem is the fold form the law needs. -/
theorem independent_folds_commute (xs ys : List StateChange) (s : StateChange)
    (h : ∀ x ∈ xs, ∀ y ∈ ys, NonConflicting x y) :
    xs.foldl mergeChanges (ys.foldl mergeChanges s) =
      ys.foldl mergeChanges (xs.foldl mergeChanges s) := by
  induction xs generalizing s with
  | nil => rfl
  | cons x xs ih =>
      simp only [List.foldl_cons]
      rw [independent_change_commutes_with_a_fold x ys s
            (fun y hy => h x (by simp) y hy)]
      rw [ih (mergeChanges s x) (fun z hz y hy => h z (by simp [hz]) y hy)]

/-- The batch corollary: two independent batches from an empty accumulator commute. -/
theorem independent_batches_commute (xs ys : List StateChange)
    (h : ∀ x ∈ xs, ∀ y ∈ ys, NonConflicting x y) :
    xs.foldl mergeChanges (ys.foldl mergeChanges emptyChange) =
      ys.foldl mergeChanges (xs.foldl mergeChanges emptyChange) :=
  independent_folds_commute xs ys emptyChange h

end Admission
end Rchain
