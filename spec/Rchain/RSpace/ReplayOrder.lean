import Mathlib.Data.List.Basic

/-!
# Law 95 — the replay record is the play's execution order

The 2026-11-08 incident (#564) reached an operator as `ReplayCostMismatch initial_cost=5576
replay_cost=5302`. The mechanism is now established: RSpace **prepends** every event to its log
(`rspace/src/rspace.rs:202-216`, `insert(0, …)`), so the log is newest-first, while
`build_replay_data` (`rspace/src/replay_rspace.rs:129-167`) files each recorded COMM under its
consume/produce refs **in the order it walks that log**. Before the fix the walk was unreversed, so a
later pending request could be offered before the earlier repeated peek's recorded COMM, the peek's
repetition counter (`comm.times_repeated`, compared at `rspace/src/replay_rspace.rs:169-181`) could
not match, work was suppressed, and the replay cost fell by 274. The fix is `log.iter().rev()`.

**The law.** A replay record must file its COMMs in **play (execution) order**. Built chronologically
(`build` below), the candidate list a consume sees is the play's own sequence; built newest-first
(`buildNewestFirst`, the defect's shape), the first matching recorded COMM is a later request's, the
work its continuation performs is different, and the replayed cost differs. The model abstracts
produce/consume identity to `Nat` refs and the continuation to a cost — the law is about the order, not
about hashing — and it carries persistence and the repetition counter because the incident's
suppression is exactly a repeated peek losing its COMM.

**What it does not claim.** It does not model `build_replay_data`'s produce-side map, the matcher, or
the RCHAIN-3505 workaround that drops the reverse half for a failed deploy
(`casper/src/runtime_replay.rs:733-749`); the law fixes the order the data is built in, and a failed
deploy's trace mask remains a separate, recorded hazard (Law 11's note). The incident's 5576/5302 pair
is evidence, not a model witness: the Rust witness
(`casper/tests/replay_trace_order.rs::pending_peeks_replay_with_exact_cost_and_state`) pins the real
costs 1860 / 2606 / 4844, and the Lean witness below is a small decided trace.

Layer `RSpace`.
-/

namespace Rchain
namespace ReplayOrder

/-- A produce/consume reference, abstracted to a number. -/
abbrev Ref := Nat

/-- One recorded COMM: the consume it belongs to, the datum refs it consumes, the datum whose
repetition counter it names (`anchor`/`repeat`), and what it charges. `cost` is the COMM's own charge
and `continuation` the work its continuation performs once it is chosen — the quantity the incident's
inversion suppresses. -/
structure RecordedComm where
  consume : Ref
  requires : List Ref
  anchor : Ref
  repeatIndex : Nat
  cost : Nat
  continuation : Nat
deriving DecidableEq, Repr

/-- An RSpace event, as the log holds them (the `Produce`/`Consume` cases carry no data here: the law
is about the COMM order). -/
inductive Event where
  | produce (r : Ref)
  | consume (r : Ref)
  | comm (c : RecordedComm)
deriving DecidableEq, Repr

/-- The tuple-space facts the selector reads: how many data each ref holds, how many times a
repetition has fired, and whether a datum is persistent. -/
structure Store where
  data : Ref → Nat
  repeats : Ref → Nat
  persistent : Ref → Bool

/-- The recorded COMM can be offered: every datum it requires is present. -/
def Store.available (s : Store) (c : RecordedComm) : Bool :=
  c.requires.all (fun r => decide (0 < s.data r))

/-- The port's `matches`: the required data are present **and** the anchored repeat counter is the one
the record names (`rspace/src/replay_rspace.rs:169-181`). -/
def Store.matches (s : Store) (c : RecordedComm) : Bool :=
  s.available c && (s.repeats c.anchor == c.repeatIndex)

/-- Fire a COMM: consume the non-persistent required data, advance the repetition counters, and keep
the persistence flags. -/
def Store.fire (s : Store) (c : RecordedComm) : Store where
  data := fun r =>
    if c.requires.contains r && (s.persistent r == false) then s.data r - 1 else s.data r
  repeats := fun r => if c.requires.contains r then s.repeats r + 1 else s.repeats r
  persistent := s.persistent

/-- The replay data as the port holds it: per consume ref, the recorded COMMs in the order the builder
filed them. -/
abbrev ReplayData := Ref → List RecordedComm

/-- **The chronological build** (the fix): walk the newest-first log in execution order and file each
COMM under its consume. This is `build_replay_data`'s `log.iter().rev()`. -/
def build (log : List Event) : ReplayData :=
  fun r =>
    log.reverse.filterMap fun e =>
      match e with
      | Event.comm c => if c.consume = r then some c else none
      | _ => none

/-- **The defect's shape**: file the COMMs in the log's own newest-first order. This is the pre-fix
`build_replay_data`, modelled beside the law so the inversion is a *second function* rather than a
comment. -/
def buildNewestFirst (log : List Event) : ReplayData :=
  fun r =>
    log.filterMap fun e =>
      match e with
      | Event.comm c => if c.consume = r then some c else none
      | _ => none

/-- Run a replay: for each request (a consume ref), take the **first** recorded COMM the state can
offer — `get_comm_and_consume_candidates` walks the list in order and returns the first comm with
candidates (`rspace/src/replay_rspace.rs:235-246`) — and charge its cost and continuation. No offer
means the request suspends, exactly as an unmatched consume is silence. -/
def run (s : Store) (d : ReplayData) (requests : List Ref) : Nat × Store :=
  requests.foldl
    (fun (acc : Nat × Store) r =>
      match (d r).find? (fun c => acc.2.matches c) with
      | some c => (acc.1 + c.cost + c.continuation, acc.2.fire c)
      | none => acc)
    (0, s)

/-! ## The witness: two pending requests on one consume, sharing a datum -/

/-- Two data: ref `1` and ref `2`. -/
def witnessStore : Store where
  data := fun r => if r = 1 ∨ r = 2 then 1 else 0
  repeats := fun _ => 0
  persistent := fun _ => false

/-- The earlier request's recorded COMM: it needs only `1` and leaves a continuation worth `5`. -/
def earlier : RecordedComm where
  consume := 0
  requires := [1]
  anchor := 1
  repeatIndex := 0
  cost := 5
  continuation := 0

/-- The later request's recorded COMM: it needs `1` and `2`, and its continuation is worth `3`. -/
def later : RecordedComm where
  consume := 0
  requires := [1, 2]
  anchor := 1
  repeatIndex := 0
  cost := 3
  continuation := 0

/-- The log as RSpace holds it: **newest first**, so the later request's COMM is at the head. -/
def witnessLog : List Event := [Event.comm later, Event.comm earlier]

/-- **The chronological build files the play's order.** -/
theorem the_chronological_build_files_the_play_order :
    (build witnessLog) 0 = [earlier, later] := by
  decide

/-- **The newest-first build files the later COMM first** — the inversion, as data. -/
theorem the_newest_first_build_files_the_later_comm_first :
    (buildNewestFirst witnessLog) 0 = [later, earlier] := by
  decide

/-- The two builders really are different functions on the witness (non-vacuity: this is what makes
the cost theorem below a statement about a reachable pair of records, not about one builder twice). -/
theorem the_two_builds_differ_on_the_witness :
    (build witnessLog) 0 ≠ (buildNewestFirst witnessLog) 0 := by
  decide

/-- **The inversion changes the cost.** In play order the earlier COMM fires, consumes the shared
datum, and the later request has nothing left to match: `5`. Newest-first, the later COMM fires first,
consumes both data, and the earlier request is suppressed: `3`. The 274-phlo class, in a trace the
kernel can decide. -/
theorem the_inversion_changes_the_cost :
    (run witnessStore (build witnessLog) [0, 0]).1 = 5 ∧
      (run witnessStore (buildNewestFirst witnessLog) [0, 0]).1 = 3 := by
  decide

/-- Persistence is carried: a persistent datum survives the fire (a repeated peek keeps its datum). -/
def persistentStore : Store :=
  { witnessStore with persistent := fun r => r == 2 }

theorem a_persistent_datum_survives_a_fire :
    (persistentStore.fire later).data 2 = 1 := by
  decide

/-- And a non-persistent datum is consumed, which is the control that the flag is load-bearing. -/
theorem a_non_persistent_datum_is_consumed :
    (witnessStore.fire later).data 2 = 0 := by
  decide

/-! ## The continuation witness and the formal rung

The cost witnesses above charge `cost` and leave `continuation = 0`. The incident's suppression is
the continuation's work, so the rung adds a second witness whose entire charge is the continuation;
the order inversion still changes it. -/

def earlierCont : RecordedComm := { earlier with cost := 0, continuation := 5 }
def laterCont : RecordedComm := { later with cost := 0, continuation := 3 }
def witnessLogCont : List Event := [Event.comm laterCont, Event.comm earlierCont]

/-- **The continuation is charged**: the same two requests, with every charge in `continuation`,
still cost `5` chronologically and `3` newest-first. -/
theorem the_continuation_is_charged :
    (run witnessStore (build witnessLogCont) [0, 0]).1 = 5 ∧
      (run witnessStore (buildNewestFirst witnessLogCont) [0, 0]).1 = 3 := by
  decide

/-- The order predicate on the witness: the filed list is the play order. -/
def FilesPlayOrder (d : ReplayData) : Prop := d 0 = [earlier, later]

def law95a_statement : Prop :=
  FilesPlayOrder (build witnessLog) ∧ ¬ FilesPlayOrder (buildNewestFirst witnessLog)

def law95a_mutant : Prop :=
  FilesPlayOrder (buildNewestFirst witnessLog)

theorem law95a_falsifier : ¬ law95a_mutant := by
  intro h
  unfold law95a_mutant FilesPlayOrder at h
  have h2 : (buildNewestFirst witnessLog) 0 = [later, earlier] :=
    the_newest_first_build_files_the_later_comm_first
  rw [h2] at h
  exact absurd h (by decide)

theorem law95a : law95a_statement :=
  ⟨the_chronological_build_files_the_play_order, law95a_falsifier⟩

def law95b_statement : Prop :=
  (run witnessStore (build witnessLog) [0, 0]).1 = 5 ∧
  (run witnessStore (buildNewestFirst witnessLog) [0, 0]).1 = 3 ∧
  (run witnessStore (build witnessLogCont) [0, 0]).1 = 5 ∧
  (run witnessStore (buildNewestFirst witnessLogCont) [0, 0]).1 = 3

def law95b_mutant : Prop :=
  (run witnessStore (buildNewestFirst witnessLog) [0, 0]).1 =
    (run witnessStore (build witnessLog) [0, 0]).1

theorem law95b_falsifier : ¬ law95b_mutant := by
  intro h
  unfold law95b_mutant at h
  have hc := the_inversion_changes_the_cost
  rw [hc.1, hc.2] at h
  exact absurd h (by decide)

theorem law95b : law95b_statement := by
  refine ⟨?_, ?_, ?_, ?_⟩
  · exact the_inversion_changes_the_cost.1
  · exact the_inversion_changes_the_cost.2
  · exact the_continuation_is_charged.1
  · exact the_continuation_is_charged.2

end ReplayOrder
end Rchain
