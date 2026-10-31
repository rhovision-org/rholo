import Rchain.Sync.Walk
import Init.Omega

/-!
# Law 81 — the verified-answer join walk

**The defect this file models.** `casper/src/engine/mod.rs`'s `LfsState` is the state machine of the
last-finalized-state block walk. Its `received` accepts a key only in `Requested` status
(`mod.rs:125-136`): an answer that arrives before its request turn is dropped, and an answer whose
body fails `validate::block_hash` has *already* moved the key to `Received` (`mod.rs:146-147`,
`lfs_block_requester.rs:73-108`). `get_next` then re-asks only `Init` — and on a resend, `Requested`
(`mod.rs:102-114`) — never `Received`. A key in `Received` is therefore **in neither set**: no request
selects it and no answer is accepted for it, and `is_finished` (`mod.rs:178-180`), which requires `d`
to be empty, can never hold. A walk stopped at 3 finished keys with 25 outstanding is the shape the
machine admits; the live instance of those counts was inbound reachability (see pass 141 and the
peer-count diagnostic), and this defect stands on its own — a refused body is enough.

**The two machines.** `WrittenStep` is the walk as it is: accept only `Requested`, a failed body
consumes the key to `Received`, and a resend re-asks only `Init`/`Requested`. `Step` is the repair:

* an answer is accepted for a **known** key in any status — the content hash is what is verified, not
  the request slot;
* the body is checked **before** the key is consumed, so a failed check is the identity and leaves the
  key re-requestable;
* `requestTurn true` re-asks every outstanding key (`Init`, `Requested`, **`Received`**), so no key can
  be stranded.

**The falsifier** is `the_written_walk_cannot_finish_from_a_stranded_key`: from a state with one
`Received` key that is still in `latest`, no run of `WrittenStep` reaches `isFinished`, because no
constructor moves `Received` and the goal requires `Done`. The repair is non-vacuous on the same state:
`the_fixed_walk_can_finish_from_a_stranded_key` reaches the goal in two turns.

**What is not claimed.** That answers *arrive*, or that the peer is honest. `a_round_makes_progress`
shows one accepted answer per round raises the finished measure and `doneCount` is bounded by the key
universe; reaching the goal needs both fair answers and a closed, finite key universe (the fringe's
ancestry closure), which the Rust supplies but this model names as hypotheses (`hK`, `hanswer`). The
give-up rule and its fuel are Law 69's; this file is about which key may be answered and which is asked
again.
-/

namespace Rchain

namespace Sync
namespace Join

/-- One key's request status — the same four values as the block leg's `ReqStatus` plus the `Done` the
    requester records in `finished` (`casper/src/engine/mod.rs:37-41`, `:166-175`). `Done` is absorbing
    for the walk's purposes: `requestTurn` and `accept` never move it back. -/
inductive Status where
  | init
  | requested
  | received
  | done
  deriving DecidableEq, Repr

/-- The walk's state: `d` is the requester's map (`none` = never named), `latest` the justification
    frontier that gates requests while it is non-empty (`mod.rs:64-69`).

    `latest` keys are always already in `d` on the Rust's reachable states — `LfsState::new` seeds both
    with the fringe hashes and `received` only removes from `latest` — so the Rust's
    `requests.entry(k).or_insert(Init)` (`mod.rs:96-100`) names no key the map does not already hold.
    The model therefore selects only keys whose status it holds; `none` is never requested. -/
structure State (Key : Type) where
  d : Key → Option Status
  latest : Finset Key

variable {Key : Type} [DecidableEq Key]

/-- A key the walk has named (whether or not it is finished). -/
def known (σ : State Key) (k : Key) : Prop := σ.d k ≠ none

/-- The statuses a request turn may select. **This is the repair's whole content**: a resend re-asks
    `Requested` *and* `Received`, not only `Requested`. -/
@[reducible] def requestable (resend : Bool) (s : Option Status) : Prop :=
  s = some Status.init ∨
    (resend = true ∧ (s = some Status.requested ∨ s = some Status.received))

/-- `getNext`'s selection (`mod.rs:94-122`): a requestable status, gated by `latest` while it is
    non-empty. -/
@[reducible] def selected (resend : Bool) (σ : State Key) (k : Key) : Prop :=
  requestable resend (σ.d k) ∧ (σ.latest = ∅ ∨ k ∈ σ.latest)

/-- The request turn: every selected key becomes `Requested`; every other key is unchanged. -/
def requestTurn (resend : Bool) (σ : State Key) : State Key :=
  { σ with d := fun k => if selected resend σ k then some Status.requested else σ.d k }

/-- The response turn's state effect after a body has been **verified**: a known key becomes `Done`,
    leaves `latest`, and the answered block's justifications are added as `Init` (the Rust's
    `received` + `done` + `add`, `mod.rs:125-175`). A `none` key is untouched (an unknown block is
    dropped) and a `Done` key is absorbing. -/
def accept (k : Key) (next : Finset Key) (σ : State Key) : State Key :=
  match σ.d k with
  | none => σ
  | some Status.done => σ
  | some _ =>
      { d := fun j =>
          if j = k then some Status.done
          else match σ.d j with
               | none => if j ∈ next then some Status.init else none
               | some s => some s
        latest := σ.latest.erase k }

/-- The goal: `is_finished` (`mod.rs:178-180`) — `latest` is empty and every key the map still holds
    is `Done`. (The Rust's `done` removes the key from `d`; keeping it as `Done` here is the same
    predicate, and it makes the measure below additive.) -/
def isFinished (σ : State Key) : Prop :=
  σ.latest = ∅ ∧ ∀ k s, σ.d k = some s → s = Status.done

/-- The number of keys of `K` the walk has finished — the measure the progress theorem counts. -/
def doneCount (K : Finset Key) (σ : State Key) : Nat :=
  (K.filter (fun k => σ.d k = some Status.done)).card

/-- Reachability, as its own inductive so that the falsifier's invariant is an induction over a run
    rather than an appeal to a library's transitive closure. -/
inductive Reach {α : Type} (R : α → α → Prop) : α → α → Prop
  | refl (a : α) : Reach R a a
  | tail {a b c : α} (hab : Reach R a b) (hbc : R b c) : Reach R a c

/-! ### The repaired machine -/

variable {Body : Type}

/-- The **fixed** walk's turns. Every constructor is a Rust event:

* `request` — `request_next` with its resend flag;
* `accept` — a body whose content check passed, for a known, unfinished key;
* `reject` — a body whose content check failed: **the identity**, so the key is not consumed;
* `unknown` — a block for a hash the walk never named: dropped;
* `idle` — a timer round with no answer. -/
inductive Step (valid : Key → Body → Bool) : State Key → State Key → Prop
  | request (resend : Bool) (σ : State Key) : Step valid σ (requestTurn resend σ)
  | accept (k : Key) (b : Body) (next : Finset Key) (σ : State Key)
      (hknown : σ.d k ≠ none) (hnotDone : σ.d k ≠ some Status.done)
      (hvalid : valid k b = true) : Step valid σ (accept k next σ)
  | reject (k : Key) (b : Body) (σ : State Key) (hvalid : valid k b = false) :
      Step valid σ σ
  | unknown (k : Key) (b : Body) (σ : State Key) (hunknown : σ.d k = none) :
      Step valid σ σ
  | idle (σ : State Key) : Step valid σ σ

/-! ### The walk as written -/

/-- `getNext`'s **pre-repair** selection (`mod.rs:104-105`): `Init`, and on a resend `Requested` —
    **never `Received`**. -/
@[reducible] def writtenRequestable (resend : Bool) (s : Option Status) : Prop :=
  s = some Status.init ∨ (resend = true ∧ s = some Status.requested)

def writtenRequestTurn (resend : Bool) (σ : State Key) : State Key :=
  { σ with d := fun k =>
      if writtenRequestable resend (σ.d k) ∧ (σ.latest = ∅ ∨ k ∈ σ.latest)
      then some Status.requested else σ.d k }

/-- A failed body as the Rust processes it: `received` has already moved `Requested` to `Received`
    (`mod.rs:146-147`) and `done` never runs (`mod.rs:166-168`), so the key is consumed without being
    finished. -/
def strand (k : Key) (σ : State Key) : State Key :=
  { σ with d := fun j => if j = k then some Status.received else σ.d j,
           latest := σ.latest.erase k }

/-- The walk **as it is written**: accept only `Requested`, a failed body strands the key, and a
    resend re-asks only `Requested`. -/
inductive WrittenStep (valid : Key → Body → Bool) : State Key → State Key → Prop
  | request (resend : Bool) (σ : State Key) : WrittenStep valid σ (writtenRequestTurn resend σ)
  | accept (k : Key) (b : Body) (next : Finset Key) (σ : State Key)
      (hrequested : σ.d k = some Status.requested) (hvalid : valid k b = true) :
      WrittenStep valid σ (accept k next σ)
  | reject (k : Key) (b : Body) (σ : State Key)
      (hrequested : σ.d k = some Status.requested) (hvalid : valid k b = false) :
      WrittenStep valid σ (strand k σ)
  | unknown (k : Key) (b : Body) (σ : State Key) (hunknown : σ.d k = none) :
      WrittenStep valid σ σ
  | idle (σ : State Key) : WrittenStep valid σ σ

/-! ### Order lemmas -/

theorem requestTurn_d_of_not_selected (resend : Bool) (σ : State Key) (k : Key)
    (h : ¬ selected resend σ k) : (requestTurn resend σ).d k = σ.d k := by
  simp [requestTurn, h]

theorem requestTurn_d_of_selected (resend : Bool) (σ : State Key) (k : Key)
    (h : selected resend σ k) : (requestTurn resend σ).d k = some Status.requested := by
  simp [requestTurn, h]

/-- A request turn never makes a key `Done`, and never un-makes one. -/
theorem requestTurn_d_eq_done (resend : Bool) (σ : State Key) (k : Key) :
    (requestTurn resend σ).d k = some Status.done ↔ σ.d k = some Status.done := by
  by_cases hs : selected resend σ k
  · rw [requestTurn_d_of_selected resend σ k hs]
    constructor
    · intro h
      exact absurd h (by decide)
    · intro h
      obtain ⟨hreq, _⟩ := hs
      unfold requestable at hreq
      rcases hreq with h' | ⟨_, h' | h'⟩ <;> rw [h] at h' <;> exact absurd h' (by decide)
  · rw [requestTurn_d_of_not_selected resend σ k hs]

/-- `accept`'s effect on any key, with the match on the answered key's status exposed. This is the
    definitional shape every lemma below reads. -/
theorem accept_d (k k' : Key) (next : Finset Key) (σ : State Key) :
    (accept k' next σ).d k =
      (match σ.d k' with
       | none => σ.d k
       | some Status.done => σ.d k
       | some _ =>
           if k = k' then some Status.done
           else match σ.d k with
                | none => if k ∈ next then some Status.init else none
                | some s => some s) := by
  unfold accept
  cases hk' : σ.d k' with
  | none => rfl
  | some s => cases s <;> rfl

/-- `accept`'s effect on `latest`: the answered key leaves it; no other key does. -/
theorem accept_latest (k' : Key) (next : Finset Key) (σ : State Key) :
    (accept k' next σ).latest =
      (match σ.d k' with
       | none => σ.latest
       | some Status.done => σ.latest
       | some _ => σ.latest.erase k') := by
  unfold accept
  cases hk' : σ.d k' with
  | none => rfl
  | some s => cases s <;> rfl

/-- `accept` leaves every **other** key out of `Done` unless it already was `Done`. Every new status it
    can write for `k ≠ k'` is `Init` (a justification) or a status copied from `σ`. -/
theorem accept_d_ne_done_of_ne (k k' : Key) (next : Finset Key) (σ : State Key)
    (hne : k ≠ k') (h : σ.d k ≠ some Status.done) :
    (accept k' next σ).d k ≠ some Status.done := by
  rw [accept_d]
  cases hk' : σ.d k' with
  | none => simpa [hk'] using h
  | some s =>
      cases s with
      | done => simpa [hk'] using h
      | init =>
          cases hk : σ.d k with
          | none => simp [hk', hk, hne]
          | some t => cases t <;> simp_all [hk', hk]
      | requested =>
          cases hk : σ.d k with
          | none => simp [hk', hk, hne]
          | some t => cases t <;> simp_all [hk', hk]
      | received =>
          cases hk : σ.d k with
          | none => simp [hk', hk, hne]
          | some t => cases t <;> simp_all [hk', hk]

/-- An accepted answer is the **only** step that marks a key `Done`, and it carries the verification
    that the body's content hash checked out. -/
theorem accepted_answer_is_verified {valid : Key → Body → Bool} {σ σ' : State Key}
    (hstep : Step valid σ σ') {k : Key} (hnew : σ'.d k = some Status.done)
    (hold : σ.d k ≠ some Status.done) : ∃ b : Body, valid k b = true := by
  cases hstep with
  | request resend σ =>
      have hne : (requestTurn resend σ).d k ≠ some Status.done := by
        intro hc
        exact hold ((requestTurn_d_eq_done resend σ k).mp hc)
      exact absurd hnew hne
  | accept k' b next σ hknown hnotDone hvalid =>
      by_cases hk : k = k'
      · subst hk
        exact ⟨b, hvalid⟩
      · exact absurd hnew (accept_d_ne_done_of_ne k k' next σ hk hold)
  | reject k' b σ hvalid => exact absurd hnew hold
  | unknown k' b σ hunknown => exact absurd hnew hold
  | idle σ => exact absurd hnew hold

/-- A rejected body is the identity: the key keeps its status and stays in `latest`, so it can be
    requested and answered again. This is the repair of the consume-before-validate order. -/
theorem a_rejected_body_leaves_the_walk_where_it_was (valid : Key → Body → Bool) (k : Key)
    (b : Body) (σ : State Key) (h : valid k b = false) : Step valid σ σ :=
  Step.reject k b σ h

/-- **The #457 core.** On a resend, every outstanding key — `Init`, `Requested`, **or `Received`** — is
    selected and becomes `Requested`, provided it is inside `latest` (or `latest` is empty). No key the
    walk still owes can fall out of the request set. -/
theorem every_outstanding_key_is_requested_on_a_resend (σ : State Key) (k : Key)
    (hknown : σ.d k ≠ none) (hnotDone : σ.d k ≠ some Status.done)
    (hlatest : σ.latest = ∅ ∨ k ∈ σ.latest) :
    (requestTurn true σ).d k = some Status.requested := by
  apply requestTurn_d_of_selected
  unfold selected requestable
  refine ⟨?_, hlatest⟩
  cases h : σ.d k with
  | none => exact absurd h hknown
  | some s => cases s <;> simp_all

/-! ### The measure

The finished count is monotone and bounded: an accepted answer of a key in `K` adds exactly that key,
and no other turn removes one. -/

theorem doneCount_requestTurn (K : Finset Key) (resend : Bool) (σ : State Key) :
    doneCount K (requestTurn resend σ) = doneCount K σ := by
  unfold doneCount
  congr 1
  ext k
  simp only [Finset.mem_filter]
  rw [requestTurn_d_eq_done]

set_option maxHeartbeats 800000 in
theorem doneCount_accept (K : Finset Key) (k : Key) (next : Finset Key) (σ : State Key)
    (hkK : k ∈ K) (hknown : σ.d k ≠ none) (hnotDone : σ.d k ≠ some Status.done) :
    doneCount K (accept k next σ) = doneCount K σ + 1 := by
  unfold doneCount
  have hset : K.filter (fun j => (accept k next σ).d j = some Status.done) =
      insert k (K.filter (fun j => σ.d j = some Status.done)) := by
    ext j
    simp only [Finset.mem_filter, Finset.mem_insert]
    by_cases hjk : j = k
    · rw [hjk]
      cases hd : σ.d k with
      | none => exact absurd hd hknown
      | some s =>
          cases s with
          | done => exact absurd hd hnotDone
          | init => simp [accept, hd, hkK]
          | requested => simp [accept, hd, hkK]
          | received => simp [accept, hd, hkK]
    · simp only [hjk, false_or]
      have happ : (accept k next σ).d j =
          (match σ.d j with
           | none => if j ∈ next then some Status.init else none
           | some s => some s) := by
        rw [accept_d]
        cases hk : σ.d k with
        | none => exact absurd hk hknown
        | some s =>
            cases s with
            | done => exact absurd hk hnotDone
            | init => rw [if_neg hjk]
            | requested => rw [if_neg hjk]
            | received => rw [if_neg hjk]
      rw [happ]
      cases hdj : σ.d j with
      | none => simp [hdj]
      | some s => cases s <;> simp [hdj]
  have hknot : k ∉ K.filter (fun j => σ.d j = some Status.done) := by
    simp only [Finset.mem_filter, not_and]
    intro _ hd
    exact hnotDone hd
  rw [hset, Finset.card_insert_of_not_mem hknot]

theorem doneCount_le_card (K : Finset Key) (σ : State Key) : doneCount K σ ≤ K.card := by
  unfold doneCount
  exact Finset.card_le_card (Finset.filter_subset _ _)

/-- **One round makes progress.** From a non-finished state whose request set is closed under the
    known keys (`hK`), there is a two-turn run — request, then accept a verified answer — that
    finishes one more key of `K`. The answer's existence is the responder's fairness (`hanswer`); the
    repair is what makes the key *selectable* in the first place.

    This is the theorem that replaces "assume the walk terminates": completion is a finite iteration
    of this round, one key per iteration, with `doneCount_le_card` as the ceiling. -/
theorem a_round_makes_progress (valid : Key → Body → Bool) (K : Finset Key) (σ : State Key)
    (hK : ∀ k, σ.d k ≠ none → k ∈ K)
    (hlatestKnown : ∀ k ∈ σ.latest, σ.d k ≠ none)
    (hlatestNotDone : ∀ k ∈ σ.latest, σ.d k ≠ some Status.done)
    (hnotFin : ¬ isFinished σ)
    (hanswer : ∀ k, σ.d k ≠ none → σ.d k ≠ some Status.done → ∃ b, valid k b = true)
    (next : Key → Finset Key) :
    ∃ σ', Reach (Step valid) σ σ' ∧ doneCount K σ' = doneCount K σ + 1 := by
  classical
  have hexists : ∃ k, σ.d k ≠ none ∧ σ.d k ≠ some Status.done ∧
      (σ.latest = ∅ ∨ k ∈ σ.latest) := by
    by_cases hlatest : σ.latest = ∅
    · have hsome : ∃ k s, σ.d k = some s ∧ s ≠ Status.done := by
        by_contra hnone
        apply hnotFin
        refine ⟨hlatest, ?_⟩
        intro k s hk
        by_contra hs
        exact hnone ⟨k, s, hk, hs⟩
      obtain ⟨k, s, hk, hs⟩ := hsome
      refine ⟨k, ?_, ?_, Or.inl hlatest⟩
      · rw [hk]; exact fun hc => Option.noConfusion hc
      · rw [hk]; exact fun hc => hs (Option.some.inj hc)
    · obtain ⟨k, hk⟩ := Finset.nonempty_iff_ne_empty.mpr hlatest
      exact ⟨k, hlatestKnown k hk, hlatestNotDone k hk, Or.inr hk⟩
  obtain ⟨k, hknown, hnotDone, hlatest⟩ := hexists
  obtain ⟨b, hvalid⟩ := hanswer k hknown hnotDone
  have hsel : selected true σ k := by
    unfold selected requestable
    refine ⟨?_, hlatest⟩
    cases h : σ.d k with
    | none => exact absurd h hknown
    | some s => cases s <;> simp_all
  have hknown' : (requestTurn true σ).d k ≠ none := by
    rw [requestTurn_d_of_selected true σ k hsel]
    simp
  have hnotDone' : (requestTurn true σ).d k ≠ some Status.done := by
    rw [requestTurn_d_of_selected true σ k hsel]
    simp
  refine ⟨accept k (next k) (requestTurn true σ), ?_, ?_⟩
  · exact Reach.tail
      (Reach.tail (Reach.refl σ) (Step.request true σ))
      (Step.accept k b (next k) (requestTurn true σ) hknown' hnotDone' hvalid)
  · rw [doneCount_accept K k (next k) (requestTurn true σ) (hK k hknown) hknown' hnotDone',
        doneCount_requestTurn]

/-! ### The falsifier: a stranded key, and the repair on the same state -/

/-- The stranded shape in miniature: one key, consumed to `Received` by a failed body, still in `latest`.
    `Bool` is the smallest key type with room for "the key" and "another key". -/
def stranded : State Bool where
  d := fun b => if b then some Status.received else none
  latest := {true}

/-- **The repair, non-vacuous on the stranded state.** A resend selects the `Received` key, a verified
    answer finishes it, and `latest` empties. Two turns, and the goal holds. -/
theorem the_fixed_walk_can_finish_from_a_stranded_key :
    ∃ σ', Reach (Step (fun (_ : Bool) (_ : Unit) => true)) stranded σ' ∧ isFinished σ' := by
  have hsel : selected true stranded true := by
    simp [selected, requestable, stranded]
  have hd1 : (requestTurn true stranded).d true = some Status.requested :=
    requestTurn_d_of_selected true stranded true hsel
  have hknown' : (requestTurn true stranded).d true ≠ none := by
    rw [hd1]; simp
  have hnotDone' : (requestTurn true stranded).d true ≠ some Status.done := by
    rw [hd1]; simp
  have hd : (accept true ∅ (requestTurn true stranded)).d true = some Status.done := by
    rw [accept_d]
    simp [hd1]
  have hdf : (accept true ∅ (requestTurn true stranded)).d false = none := by
    rw [accept_d]
    simp [requestTurn, selected, requestable, stranded]
  have hl : (accept true ∅ (requestTurn true stranded)).latest = ∅ := by
    simp only [accept_latest]
    rw [hd1]
    simp [requestTurn, stranded]
  refine ⟨accept true ∅ (requestTurn true stranded),
    Reach.tail (Reach.tail (Reach.refl stranded) (Step.request true stranded))
      (Step.accept true () ∅ (requestTurn true stranded) hknown' hnotDone' rfl), ?_⟩
  refine ⟨hl, ?_⟩
  intro k s hk
  cases k with
  | true => rw [hd] at hk; exact (Option.some.inj hk).symm
  | false => rw [hdf] at hk; exact absurd hk (by simp)

/-- **The falsifier, red on the code as written.** From the stranded state, every `WrittenStep` leaves
    the key `Received`: no request turn selects it (both resend values) and no accept or reject
    constructor applies to it. So no run reaches `isFinished`, whose every-key-`Done` clause is false
    of the state for ever. This is the `(3 finished, 25 outstanding)` shape the code admits. -/
theorem the_written_walk_cannot_finish_from_a_stranded_key
    (valid : Bool → Unit → Bool) :
    ¬ ∃ σ', Reach (WrittenStep valid) stranded σ' ∧ isFinished σ' := by
  have hpreserve : ∀ {σ σ' : State Bool}, σ.d true = some Status.received →
      WrittenStep valid σ σ' → σ'.d true = some Status.received := by
    intro σ σ' hσ hstep
    cases hstep with
    | request resend σ =>
        have hnot : ¬ (writtenRequestable resend (σ.d true) ∧
            (σ.latest = ∅ ∨ true ∈ σ.latest)) := by
          intro hsel
          obtain ⟨hreq, _⟩ := hsel
          unfold writtenRequestable at hreq
          rcases hreq with h | ⟨_, h⟩ <;> rw [hσ] at h <;> exact absurd h (by decide)
        simp [writtenRequestTurn, hnot, hσ]
    | accept k b next σ hrequested hvalid =>
        have hne : true ≠ k := by
          intro h
          subst h
          rw [hσ] at hrequested
          exact absurd hrequested (by decide)
        simp [accept, hrequested, hne, hσ]
    | reject k b σ hrequested hvalid =>
        by_cases hkt : true = k
        · subst hkt
          simp [strand, hσ]
        · simp [strand, hkt, hσ]
    | unknown k b σ hunknown => exact hσ
    | idle σ => exact hσ
  have hreach : ∀ {σ' : State Bool}, Reach (WrittenStep valid) stranded σ' →
      σ'.d true = some Status.received := by
    intro σ' h
    induction h with
    | refl => rfl
    | tail hab hbc ih => exact hpreserve ih hbc
  rintro ⟨σ', hrun, hfin⟩
  obtain ⟨_, hall⟩ := hfin
  have hd := hreach hrun
  have hdone := hall true Status.received hd
  exact absurd hdone (by decide)

theorem the_written_walk_strands_a_received_key :
    stranded.d true = some Status.received ∧ ¬ isFinished stranded := by
  refine ⟨rfl, ?_⟩
  intro hfin
  exact absurd (hfin.2 true Status.received rfl) (by decide)

end Join
end Sync
end Rchain
