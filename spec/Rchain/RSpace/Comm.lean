import Rchain.Cmp
import Rchain.Crypto.Spec

/-!
# Laws 8 & 11 — deterministic COMM, replay determinism

A COMM event is content-addressed by the sorted produce refs (Law 8); replay recomputes COMM events and
must agree with the recorded trace (Law 11). The Scala oracle is `trace/Event.scala:35-39` +
`ReplayRSpace.scala:68-71`; the Rust realization is `rspace/src/trace/event.rs` (the event types and
their hashes) + `rspace/src/space_matcher.rs` (sorted-first selection) + `rspace/src/replay_rspace.rs`.

## What the consolidation pass changed here

This file carried four axioms: `produceRefs : Comm → List Nat`, `comm_content_addressed`,
`replayEvents : Trace → List Comm`, and `replay_comm_subset`. Two of them were claims about *undefined*
functions, so they said nothing —

* `produceRefs` is a **definition** now, mirroring `Comm::apply`'s sort (`event.rs:150-173`), and
  `comm_content_addressed` is a **theorem**: a COMM's identity is canonical because the refs are sorted,
  which is Law 1's canonicalization applied to the event log.
* `replayEvents`/`replay_comm_subset` are **deleted**, and Law 11's row went to `vacuous` with the
  reason written down. The law is real in the code — replay recomputes the log with the same matcher and
  checks membership, and it checks the *reverse* too ("Unused COMM event", `replay_rspace.rs:580-590`) —
  but in that model "recompute" and "record" would be the *same function*, so the subset claim would be
  `rfl`. What gives it content is a model of the recorded store, so that the two are different
  functions that must agree; that is the modelling step below, and it is landed.

## The modelling step, landed (2026-09-23, Programme D unit 9)

The design below is the one the previous attempt worked out and reverted two or three build cycles from
closing, with one change that removes all three of the obstructions it hit: **multiplicity is
`List.count`, not a function of our own.** The earlier version defined `occurrences` by its own
recursion, so every lemma in the proof opened with `if e = c then 1 else 0` under a quantified binder
and had to fight `if_pos`/`if_neg` reduction, variable shadowing, and `0 + x` normalisation. The
prelude's `count_cons_self`/`count_cons_of_ne`/`count_erase_self`/`count_erase_of_ne`/`count_eq_zero`
are exactly those lemmas, already proved, and `List.erase` is already `remove_bindings_for`'s step. The
proof is then `omega`-plus-`simp` and nothing else.

What the model is, and why it answers the note above: the replay's **record is an input** — an arbitrary
list of COMM refs — and the recomputation is another. They are *different* values, so the claim that
they agree can fail, which is precisely what the deleted `replayEvents`/`replay_comm_subset` could not
say. The check the port runs is the replay as a relation over those two inputs: one step per recomputed
COMM, each requiring a recorded occurrence *at that point* (the forward half — `ReplayCommNotInTrace`),
and the relation ends only when the record is empty (the reverse half — `check_replay_data`'s "Unused
COMM event"). The law is then an **equivalence**: the check holds exactly when the two inputs have the
same occurrences.

Two consequences are theorems here rather than claims, and the second is a live hazard:

* `the_forward_half_alone_admits_a_diverging_trace` and
  `the_reverse_half_alone_admits_a_phantom_recomputation` — neither half suffices, each with a witness.
* The port drops the reverse half for a **failed** deploy: `check_replay_data_with_fix`
  (`casper/src/runtime_replay.rs:587-599`) returns `Ok` on an unused-COMM failure when
  `eval_successful` is false, the RCHAIN-3505 workaround. So on that path the check *is* the forward
  half, and `the_forward_half_alone_admits_a_diverging_trace` is the witness that it admits a trace the
  strict check refuses. That is the divergence-masking the plan flagged: a play/replay difference of the
  kind this law is about can pass unremarked behind a failed deploy. Modelled as a theorem so that
  removing the workaround is a change the register can see.
-/

namespace Rchain

/-- A produce as the Rust stores it: its channel's hash, its own content hash, and its persistence flag.
    Equality and order in the code are by content hash (`Produce::apply` gives the hash at
    `rspace/src/trace/event.rs:24-49`, `Ord` is by it at `:67-71`), which is why the ref list below can
    be sorted at all. -/
structure Produce where
  chanHash : Hash
  hash : Hash
  persist : Bool

/-- A consume's identity: the hashes of the channels it joins and its continuation's hash
    (`Consume::apply`, `rspace/src/trace/event.rs:82-104`). -/
structure Consume where
  channels : List Hash
  continuation : Hash

/-- A COMM: a consume matched against the produces it consumed, **in arrival order** — that order is
    exactly what the canonical identity below removes (`Comm::apply`, `event.rs:150-173`). -/
structure Comm where
  consume : Consume
  produces : List Produce

/-- The Rust's produce order: by `(channels_hash, hash, persistent)` (`event.rs:165`). -/
def produceComparator : Comparator (Hash × Hash × Bool) :=
  Comparator.cmpPair (Comparator.listComparator (Comparator.linearOrderComparator Byte))
    (Comparator.cmpPair (Comparator.listComparator (Comparator.linearOrderComparator Byte))
      (Comparator.linearOrderComparator Bool))

/-- The produce refs in canonical order: sorted by the same key the Rust sorts by
    (`produce_refs.sort_by_key`, `event.rs:165`). -/
def produceRefs (c : Comm) : List (Hash × Hash × Bool) :=
  Comparator.sortList produceComparator
    (c.produces.map (fun p => (p.chanHash, p.hash, p.persist)))

/-- A COMM's canonical identity: its consume, and the **sorted** refs. -/
def commId (c : Comm) : Consume × List (Hash × Hash × Bool) := (c.consume, produceRefs c)

/-- **Law 8** — a COMM's identity is canonical: two comms whose produces differ only in *order* are the
    same event, because the identity sorts them. This is Law 1's canonicalization applied to the event
    log, and it is what makes the recorded trace reproducible — a replay that matched the same produces
    in a different order produces the same event, which is the fact Law 11's row then leans on. -/
theorem comm_content_addressed (a b : Comm) (hc : a.consume = b.consume)
    (h : List.Perm a.produces b.produces) : commId a = commId b := by
  simp only [commId]
  rw [hc]
  congr 1
  simp only [produceRefs]
  exact Comparator.sortList_perm produceComparator
    (List.Perm.map (fun p => (p.chanHash, p.hash, p.persist)) h)

/-- A recorded trace of COMM events: the log `ReplayRSpace` replays against. -/
structure Trace where
  events : List Comm

/-! ## Law 11 — the replay's check, as a relation over a record and a recomputation

The record is `rig`'d from the log (`replay_rspace.rs:516-518`) and the replay walks the recomputed
COMMs against it; the two are **separate inputs** here, which is what gives the law content. The key a
recorded event is found under is `commId` — the port compares `Comm` *refs* (`Comm::apply`'s result,
`event.rs:150-173`), never the `Comm` itself, whose produce order Law 8's `comm_content_addressed` has
just shown to be non-canonical. -/

/-- The key the replay's multimap is indexed by: a COMM's canonical identity, with `Consume` written
    out as its two fields. It is a product of `List Byte`, `Byte` and `Bool` rather than a structure of
    its own for a mechanical reason worth recording: `List.count`/`List.erase` — the multiplicity the
    replay walks — need a `LawfulBEq` instance, Lean does **not** derive one for a custom structure,
    and the built-in product already has it. -/
abbrev CommRef := (List Hash × Hash) × List (Hash × Hash × Bool)

/-- A COMM's ref, as the replay keys it — `commId` flattened. -/
def refOf (c : Comm) : CommRef := ((c.consume.channels, c.consume.continuation), produceRefs c)

/-- The refs of a log, in order — the keys a replay looks each recomputed COMM up by. -/
def refsOf (l : List Comm) : List CommRef := l.map refOf

/-- How many times a ref occurs in a trace. Multiplicity, not membership: the record can hold the same
    COMM twice, and the port's `remove_bindings_for` (`replay_rspace.rs:334`) consumes **one**
    occurrence per recomputation. An `abbrev`, so the prelude's `List.count` lemmas rewrite through it
    without every proof opening with a `simp only [occurrences]`. -/
abbrev occurrences (l : List CommRef) (a : CommRef) : Nat := l.count a

/-- The port's `remove_bindings_for`: consume one recorded occurrence of a recomputed COMM. `none` is
    the `ReplayCommNotInTrace` case (`replay_rspace.rs:330-332`). -/
def removeOne (l : List CommRef) (a : CommRef) : Option (List CommRef) :=
  if a ∈ l then some (l.erase a) else none

/-- **The replay as the port runs it.** One step per recomputed COMM, in the order the replay consumes
    them: the COMM must still have a recorded occurrence (the **forward** half — `removeOne` is
    `remove_bindings_for`, and a missing occurrence is `ReplayCommNotInTrace`), and the relation ends
    only with the record **empty** (the **reverse** half — `check_replay_data`'s "Unused COMM event:
    replayData multimap has N elements left", `replay_rspace.rs:576-586`). Both halves are one relation
    because both are one replay. -/
inductive Replays : List CommRef → List CommRef → Prop
  | done : Replays [] []
  | step (a : CommRef) (cs rs : List CommRef) : a ∈ rs → Replays cs (rs.erase a) →
      Replays (a :: cs) rs

/-- A step is exactly a successful `removeOne`: this is the tie between the relation and the Rust call
    it mirrors (`remove_bindings_for`, whose `none` is `ReplayCommNotInTrace`). -/
theorem removeOne_eq_some_iff {l : List CommRef} {a : CommRef} {l' : List CommRef} :
    removeOne l a = some l' ↔ a ∈ l ∧ l' = l.erase a := by
  simp only [removeOne]
  constructor
  · intro h
    by_cases hmem : a ∈ l
    · rw [if_pos hmem] at h; exact ⟨hmem, (Option.some.injEq _ _ ▸ h).symm⟩
    · rw [if_neg hmem] at h; exact absurd h (by simp)
  · rintro ⟨hmem, rfl⟩
    rw [if_pos hmem]

/-- The **forward half on its own**: every recomputed COMM has a recorded occurrence. This is what
    survives on the port's lenient path, and it is not the law. -/
def forwardHolds (recomputed recorded : List CommRef) : Prop :=
  ∀ a ∈ recomputed, a ∈ recorded

/-- The **reverse half on its own**: no recorded COMM is left unconsumed. -/
def reverseHolds (recomputed recorded : List CommRef) : Prop :=
  ∀ a, occurrences recorded a ≤ occurrences recomputed a

/-- A concrete COMM ref, distinguishable by its continuation — the witnesses' vocabulary, so the
    insufficiency theorems are `decide`-checkable rather than prose. -/
def sampleRef (n : Nat) : CommRef := (([], List.replicate n 0), [])

/-! ### The four lemmas the walk needs (the prelude has the rest) -/

/-- `occurrences` is positive exactly when the ref is there. The prelude's `List.count_eq_zero` is the
    other direction, spelled with `∉`. -/
theorem occurrences_pos_iff {a : CommRef} {l : List CommRef} : 0 < occurrences l a ↔ a ∈ l := by
  simp only [occurrences]
  constructor
  · intro h
    by_contra hc
    have : l.count a = 0 := List.count_eq_zero.mpr hc
    omega
  · intro h
    have : l.count a ≠ 0 := fun hz => (List.count_eq_zero.mp hz) h
    omega

/-- A head occurrence, as arithmetic: this is what makes the step cases below `omega` problems rather
    than rewrite chains. -/
theorem occurrences_cons_self (a : CommRef) (l : List CommRef) :
    occurrences (a :: l) a = occurrences l a + 1 := by
  simp [occurrences, List.count_cons_self]

/-- A non-head occurrence is unchanged. -/
theorem occurrences_cons_of_ne {a b : CommRef} (h : b ≠ a) (l : List CommRef) :
    occurrences (a :: l) b = occurrences l b := by
  simp only [occurrences]
  exact List.count_cons_of_ne (a := b) (b := a) h l

/-- Removing an occurrence the trace has raises every count back to what it was: the `List.count`
    lemmas give this modulo subtraction, and the `a ∈ l` bound is what turns it into an equation. -/
theorem occurrences_erase_of_mem {a b : CommRef} {l : List CommRef} (h : a ∈ l) :
    occurrences (l.erase a) b + (if b = a then 1 else 0) = occurrences l b := by
  rcases eq_or_ne b a with hba | hne
  · rw [if_pos hba]
    subst hba
    have hpos : 0 < occurrences l b := occurrences_pos_iff.mpr h
    have h1 : occurrences (l.erase b) b = occurrences l b - 1 :=
      List.count_erase_self (a := b) (l := l)
    omega
  · rw [if_neg hne]
    exact List.count_erase_of_ne (a := b) (b := a) hne l

/-- There is nothing to occur in an empty trace. -/
theorem occurrences_nil (a : CommRef) : occurrences ([] : List CommRef) a = 0 := rfl

/-- A trace with no occurrences anywhere is empty. -/
theorem eq_nil_of_occurrences_eq_zero {l : List CommRef}
    (h : ∀ b, occurrences l b = 0) : l = [] := by
  rw [List.eq_nil_iff_forall_not_mem]
  intro b hb
  have hpos : 0 < occurrences l b := occurrences_pos_iff.mpr hb
  have hzero : occurrences l b = 0 := h b
  omega

/-! ### The law -/

/-- **Law 11 — the port's check is exactly agreement.** A replay of `recomputed` against a `recorded`
    trace succeeds **if and only if** the two have the same occurrences of every COMM. The two sides
    are independent inputs, so this is a claim that can fail — a recomputed COMM the record does not
    have fails the forward half, and a recorded COMM the replay never consumes fails the reverse half.
    What it does *not* model is the recomputation itself: `recomputed` is an input here, and the fact
    that the node recomputes the same log is the reducer's business (law 25), not this check's. -/
theorem replays_iff_same_occurrences (recomputed recorded : List CommRef) :
    Replays recomputed recorded ↔
      ∀ a, occurrences recomputed a = occurrences recorded a := by
  constructor
  · intro h
    induction h with
    | done => intro a; rfl
    | step a cs rs ha _ ih =>
        intro b
        have herase := occurrences_erase_of_mem (a := a) (b := b) ha
        have hic := ih b
        rcases eq_or_ne b a with hba | hne
        · subst hba
          rw [if_pos rfl] at herase
          have hc := occurrences_cons_self b cs
          omega
        · rw [if_neg hne, Nat.add_zero] at herase
          have hc := occurrences_cons_of_ne hne cs
          omega
  · intro h
    induction recomputed generalizing recorded with
    | nil =>
        have hnil : recorded = [] :=
          eq_nil_of_occurrences_eq_zero (fun b => by
            have := h b
            simpa [occurrences] using this.symm)
        subst hnil
        exact Replays.done
    | cons a cs ih =>
        have hmem : a ∈ recorded := by
          have hp : 0 < occurrences recorded a := by
            rw [← h a, occurrences_cons_self]
            omega
          exact occurrences_pos_iff.mp hp
        have hih : Replays cs (recorded.erase a) :=
          ih (recorded.erase a) (fun b => by
            have herase := occurrences_erase_of_mem (a := a) (b := b) hmem
            have hic := h b
            rcases eq_or_ne b a with hba | hne
            · subst hba
              rw [if_pos rfl] at herase
              have hc := occurrences_cons_self b cs
              omega
            · rw [if_neg hne, Nat.add_zero] at herase
              have hc := occurrences_cons_of_ne hne cs
              omega)
        exact Replays.step a cs recorded hmem hih

/-- **The forward half alone is not the law** — and this is not hypothetical: it is exactly what the
    port enforces for a *failed* deploy. `check_replay_data_with_fix` (`runtime_replay.rs:587-599`, the
    RCHAIN-3505 workaround) returns `Ok` when the trace has entries left unconsumed and
    `eval_successful` is false, so the reverse half is dropped there. One recomputed COMM, two
    recorded: every recomputed COMM is in the trace, and the check still fails — the leftover is the
    `Unused COMM event` the guard is swallowing. -/
theorem the_forward_half_alone_admits_a_diverging_trace :
    forwardHolds [sampleRef 0] [sampleRef 0, sampleRef 1] ∧
    ¬ Replays [sampleRef 0] [sampleRef 0, sampleRef 1] := by
  refine ⟨?_, fun h => ?_⟩
  · intro a ha
    simp only [List.mem_singleton] at ha
    subst ha
    simp
  · have hc := (replays_iff_same_occurrences _ _).mp h (sampleRef 1)
    exact absurd hc (by decide)

/-- **And the reverse half alone is not the law either**: no recorded COMM left over, but one
    recomputed COMM with no occurrence to consume — the `ReplayCommNotInTrace` case the forward half
    exists for. -/
theorem the_reverse_half_alone_admits_a_phantom_recomputation :
    reverseHolds [sampleRef 0, sampleRef 0] [sampleRef 0] ∧
    ¬ Replays [sampleRef 0, sampleRef 0] [sampleRef 0] := by
  refine ⟨?_, fun h => ?_⟩
  · intro a
    rcases eq_or_ne a (sampleRef 0) with hba | hne
    · rw [hba]; decide
    · have h0 := occurrences_nil a
      have h1 := occurrences_cons_of_ne (a := sampleRef 0) (b := a) hne ([] : List CommRef)
      have h2 := occurrences_cons_of_ne (a := sampleRef 0) (b := a) hne [sampleRef 0]
      omega
  · have hc := (replays_iff_same_occurrences _ _).mp h (sampleRef 0)
    exact absurd hc (by decide)

/-- **The law, over logs rather than refs** — the statement the register's row carries: a replay of one
    log against another agrees exactly when the two logs have the same COMM occurrences. The two logs
    are different values, which is the content the deleted `replayEvents`/`replay_comm_subset` could not
    have: "recompute" and "record" were the same function there. -/
theorem a_replay_agrees_with_its_record (recomputed recorded : List Comm) :
    Replays (refsOf recomputed) (refsOf recorded) ↔
      ∀ a, occurrences (refsOf recomputed) a = occurrences (refsOf recorded) a :=
  replays_iff_same_occurrences _ _

/-- **Law 11's statement**: the port's replay relation is exactly occurrence agreement. -/
def law11_statement : Prop :=
  ∀ recomputed recorded : List CommRef,
    Replays recomputed recorded ↔
      ∀ a, occurrences recomputed a = occurrences recorded a

/-- **Law 11's mutant**: the forward half alone is treated as the whole check. This is the
    RCHAIN-3505 workaround's shape (`check_replay_data_with_fix`), which drops the reverse half for
    a failed deploy. -/
def law11_mutant : Prop :=
  ∀ recomputed recorded : List CommRef,
    forwardHolds recomputed recorded → Replays recomputed recorded

/-- **The falsifier, fired**: the forward-half-only rule admits a trace the replay refuses. -/
theorem law11_falsifier : ¬ law11_mutant := by
  intro h
  have hf : forwardHolds [sampleRef 0] [sampleRef 0, sampleRef 1] :=
    (the_forward_half_alone_admits_a_diverging_trace).1
  have hr : Replays [sampleRef 0] [sampleRef 0, sampleRef 1] := h _ _ hf
  exact (the_forward_half_alone_admits_a_diverging_trace).2 hr

theorem law11 : law11_statement := replays_iff_same_occurrences

end Rchain
