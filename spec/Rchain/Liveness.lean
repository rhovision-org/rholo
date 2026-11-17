/-!
# Law 89 — liveness enforcement: jail first, a vote for the bond

A validator that stops keeps its bond and counts in the finality denominator for ever. This module
states the two-step response the node now implements (RCHIP #55): at an **advancing** epoch
boundary a sustained absentee is **jailed** — removed from the active set for `J` boundaries and paid
nothing — while its bond and its delegators' principal are untouched. Only a **stake-weighted vote
above two thirds of the active stake**, opened against an already-jailed validator, may **slash** and
permanently eject it.

The model deliberately separates the two steps. `stepJail` never writes `pool`; `voteSlash` is the
only function that does, and only when `passes`. The defect variants are the falsifiers:
`stallStep` (counts absence without finality progress), `burnStep` (jails by removing the bond), and
`simpleMajorityGate` (ejects at exactly two thirds).

What stays with Rust and its tests: the store codecs, the epoch boundary's participation map
(derived from the receiver's finalised fringe), delegation fan-out on a slash, the withdrawal
escrow, and the exact `is_super_majority` (this module uses the same strict `3·for > 2·total`).

**The release/renewal clause is conditional on a presence signal, and the node's signal refutes it.**
`releaseIfPresent` takes `present` as a parameter, so the earlier theorems are true for any predicate.
`presentFromSenders` is the derivation the node actually runs (finalised-fringe senders), and
`gatedSenders` is the proposer gate (a jailed validator is outside the drawn active set and cannot
author a message). Under those two facts `a_jailed_validator_is_never_present`,
`block_presence_cannot_release_a_jailed_validator` and `block_presence_renews_a_jailed_validator`
state the reachable outcome: **renew, never release**. A release needs a signal a jailed validator can
produce — a deploy it signs and an unjailed proposer includes — or a fixed non-renewing term; the
algebra does not supply one.
-/

namespace Rchain

namespace Liveness

/-- Validator identities are opaque numbers in the model; the Rust side uses 65-byte keys. -/
abbrev Validator := Nat

/-- The liveness-relevant part of the PoS state. `pool` is the bond map; `active` is the drawn
    finality set; `absent`/`jailed` are the counters; `windows` are open vote windows; `ballots`
    are `((target, voter), inFavor)`. -/
structure State where
  pool : List (Validator × Nat)
  active : List (Validator × Nat)
  absent : List (Validator × Nat)
  jailed : List (Validator × Nat)
  banned : List Validator
  windows : List (Validator × Nat)
  ballots : List ((Validator × Validator) × Bool)
  anchor : Nat
  deriving Repr, DecidableEq

/-- A validator's value in a `validator → nat` list; `0` when absent. -/
def lookup (entries : List (Validator × Nat)) (validator : Validator) : Nat :=
  (entries.find? (fun entry => entry.1 == validator)).map (fun entry => entry.2) |>.getD 0

def absentCount (s : State) (v : Validator) : Nat := lookup s.absent v
def jailTerm (s : State) (v : Validator) : Nat := lookup s.jailed v
def windowLeft (s : State) (v : Validator) : Nat := lookup s.windows v

/-- Total active stake. -/
def totalStake (s : State) : Nat :=
  s.active.foldl (fun acc entry => acc + entry.2) 0

/-- The active stake whose ballot for `target` is in favour. -/
def forStake (s : State) (target : Validator) : Nat :=
  s.ballots.foldl
    (fun acc ballot =>
      if ballot.2 && ballot.1.1 == target then acc + lookup s.active ballot.1.2 else acc)
    0

/-- **Strictly more than two thirds.** The same integer rule as `is_super_majority`. -/
def passes (s : State) (target : Validator) : Bool :=
  3 * forStake s target > 2 * totalStake s

/-- **The automatic step.** It runs only on an advancing boundary (`advance = true`); every active
    validator's absence counter is reset by a present boundary or incremented by an absent one, and a
    count reaching `N` opens a jail of `J`. Existing jails lose one boundary and are released when a
    present validator reaches the end, renewed while it stays silent. -/
def stepJail (present : Validator → Bool) (advance : Bool) (N J : Nat) (s : State) : State :=
  if advance then
    let counted := s.active.map (fun entry =>
      (entry.1, if present entry.1 then 0 else absentCount s entry.1 + 1))
    let newly := counted.filterMap (fun entry => if entry.2 >= N then some (entry.1, J) else none)
    let kept :=
      s.jailed.filterMap (fun entry =>
        if entry.2 > 1 then some (entry.1, entry.2 - 1)
        else if present entry.1 then none
        else some (entry.1, J))
    { s with absent := counted, jailed := newly ++ kept }
  else s

/-- A boundary gate: nothing is decided off a boundary. -/
def boundaryStep (onBoundary : Bool) (present : Validator → Bool) (advance : Bool) (N J : Nat)
    (s : State) : State :=
  if onBoundary then stepJail present advance N J s else s

/-- **A jail is not a bond change.** The only ledger `stepJail` writes is the counter list. -/
theorem jail_never_touches_the_bond (present : Validator → Bool) (advance : Bool) (N J : Nat)
    (s : State) : (stepJail present advance N J s).pool = s.pool := by
  unfold stepJail
  split <;> rfl

/-- **A stalled boundary changes nothing.** No advance, no counter, jail or window movement. -/
theorem the_stall_rule_changes_nothing (present : Validator → Bool) (N J : Nat) (s : State) :
    stepJail present false N J s = s := by
  simp [stepJail]

/-- **No boundary, no action.** -/
theorem no_boundary_no_action (present : Validator → Bool) (advance : Bool) (N J : Nat)
    (s : State) : boundaryStep false present advance N J s = s := by
  simp [boundaryStep]

/-- One lapse is not sustained absence: with `N = 2`, a single absent boundary leaves the
    validator unjailed. -/
theorem one_lapse_is_not_sustained :
    let s : State :=
      { pool := [(1, 10)], active := [(1, 10)], absent := [], jailed := [], banned := [],
        windows := [], ballots := [], anchor := 0 }
    jailTerm (stepJail (fun _ => false) true 2 5 s) 1 = 0 := by
  decide

/-- **The defect shape, in the tree:** a step that counts absence without checking finality
    progress jails an innocent validator during a stall. -/
def stallStep (present : Validator → Bool) (N J : Nat) (s : State) : State :=
  stepJail present true N J s

theorem a_stall_that_counts_jails_an_innocent :
    let s : State :=
      { pool := [(1, 10)], active := [(1, 10)], absent := [(1, 1)], jailed := [], banned := [],
        windows := [], ballots := [], anchor := 0 }
    jailTerm (stallStep (fun _ => false) 2 5 s) 1 = 5 ∧ stepJail (fun _ => false) false 2 5 s = s := by
  decide

/-- **The vote gate.** A vote changes the pool only when it passes. -/
def voteSlash (s : State) (target : Validator) : State :=
  if passes s target then
    { s with
      pool := s.pool.filter (fun entry => entry.1 != target)
      active := s.active.filter (fun entry => entry.1 != target)
      banned := target :: s.banned
      jailed := s.jailed.filter (fun entry => entry.1 != target)
      windows := s.windows.filter (fun entry => entry.1 != target)
      ballots := s.ballots.filter (fun ballot => ballot.1.1 != target) }
  else s

/-- **Only a passing vote reaches the pool.** -/
theorem vote_admits_only_above_threshold (s : State) (target : Validator)
    (h : (voteSlash s target).pool ≠ s.pool) : passes s target = true := by
  by_cases hp : passes s target = true
  · exact hp
  · simp [voteSlash, hp] at h

/-- **A passing vote slashes and permanently ejects.** -/
theorem a_passing_vote_slashes_and_ejects (s : State) (target : Validator)
    (h : passes s target = true) :
    (voteSlash s target).pool = s.pool.filter (fun entry => entry.1 != target) ∧
      target ∈ (voteSlash s target).banned := by
  simp [voteSlash, h]

/-- **A simple majority is not enough.** Two of three active stake is exactly two thirds, not a
    strict supermajority. -/
theorem a_simple_majority_is_not_enough :
    let s : State :=
      { pool := [(1, 10), (2, 10), (3, 10)], active := [(1, 10), (2, 10), (3, 10)], absent := [], jailed := [],
        banned := [], windows := [], ballots := [((3, 1), true), ((3, 2), true)], anchor := 0 }
    passes s 3 = false := by
  decide

/-- **A vote is not opened against an active validator**: opening requires an existing jail. -/
def openWindow (s : State) (target : Validator) (length : Nat) : State :=
  if jailTerm s target > 0 then { s with windows := (target, length) :: s.windows } else s

theorem a_vote_without_jail_is_not_opened (s : State) (target : Validator) (length : Nat)
    (h : jailTerm s target = 0) : openWindow s target length = s := by
  simp [openWindow, h]

/-- **The defect shape that burns a bond:** a "jail" that removes the pool entry. It is not
    `stepJail`, and the difference is the law. -/
def burnStep (s : State) (target : Validator) : State :=
  { s with pool := s.pool.filter (fun entry => entry.1 != target) }

theorem jail_that_burns_the_bond_reaches_a_stake :
    let s : State :=
      { pool := [(1, 10)], active := [(1, 10)], absent := [], jailed := [], banned := [],
        windows := [], ballots := [], anchor := 0 }
    (burnStep s 1).pool ≠ s.pool ∧ (stepJail (fun _ => false) false 2 5 s).pool = s.pool := by
  decide

/-- The head of a `validator → nat` list is that validator's value. -/
theorem lookup_cons_self (v J : Nat) (entries : List (Validator × Nat)) :
    lookup ((v, J) :: entries) v = J := by
  simp [lookup]

/-- **Continued silence renews**: with one boundary left and no message, the jail is renewed to the
    full term. -/
def renewIfAbsent (present : Validator → Bool) (J : Nat) (s : State) (v : Validator) : State :=
  if jailTerm s v ≤ 1 ∧ present v = false then
    { s with jailed := (v, J) :: s.jailed.filter (fun entry => entry.1 != v) }
  else s

theorem a_still_absent_validator_is_renewed (present : Validator → Bool) (J : Nat) (s : State)
    (v : Validator) (hterm : jailTerm s v ≤ 1) (habsent : present v = false) :
    jailTerm (renewIfAbsent present J s v) v = J := by
  simp only [renewIfAbsent]
  rw [if_pos ⟨hterm, habsent⟩]
  exact lookup_cons_self v J _

/-- **A passing vote closes the target's window and clears its ballots.** `voteSlash` already filters
    both (`:132-133`); this states it, because it is the condition the exact-threshold and jail-vote
    arms assert after the slash (`vote_window_closed_after_slash`,
    `liveness_record_cleared_after_slash`) and, until this theorem, nothing on the Lean side said it.
    The Rust assertion is `rholang/src/native_state.rs::a_passing_vote_slashes_and_permanently_ejects`. -/
theorem a_passing_vote_closes_the_window_and_clears_ballots (s : State) (target : Validator)
    (h : passes s target = true) :
    (voteSlash s target).windows = s.windows.filter (fun entry => entry.1 != target) ∧
      (voteSlash s target).ballots = s.ballots.filter (fun ballot => ballot.1.1 != target) := by
  simp [voteSlash, h]

/-- **A released jail leaves no window or ballot behind.** The Rust release path removes the jail
    record, the window and the target's ballots (`native_state.rs:2694-2699`). The model's
    `releaseIfPresent` removed only the jail entry; this correction makes the model say what the code
    does, and `release_clears_the_window_and_ballots` states it. The Rust witness is
    `rholang/src/native_state.rs::a_released_jail_leaves_no_window_or_ballot`. -/
def releaseIfPresent (present : Validator → Bool) (s : State) (v : Validator) : State :=
  if jailTerm s v ≤ 1 ∧ present v = true then
    { s with
      jailed := s.jailed.filter (fun entry => entry.1 != v)
      windows := s.windows.filter (fun entry => entry.1 != v)
      ballots := s.ballots.filter (fun ballot => ballot.1.1 != v) }
  else s

theorem a_returning_validator_is_released (present : Validator → Bool) (s : State) (v : Validator)
    (hterm : jailTerm s v ≤ 1) (hpresent : present v = true) :
    v ∉ (releaseIfPresent present s v).jailed.map (fun entry => entry.1) := by
  simp [releaseIfPresent, hterm, hpresent]

theorem release_clears_the_window_and_ballots (present : Validator → Bool) (s : State)
    (v : Validator) (hterm : jailTerm s v ≤ 1) (hpresent : present v = true) :
    (releaseIfPresent present s v).windows = s.windows.filter (fun entry => entry.1 != v) ∧
      (releaseIfPresent present s v).ballots =
        s.ballots.filter (fun ballot => ballot.1.1 != v) := by
  simp [releaseIfPresent, hterm, hpresent]

/-- **The node's presence derivation.** A validator is present at a boundary iff it authored a
    message in the finalised fringe. This is the whole of the reachable rule: the boundary's
    participation map is `latest_heights(prev_fringe senders)` in
    `casper/src/multi_parent_casper.rs`, and nothing else is read — no deploy signer enters it. -/
def presentFromSenders (senders : Validator → Bool) (v : Validator) : Bool := senders v

/-- **The proposer gate, as the model needs it.** A jailed validator is removed from the drawn
    active set, and the proposer refuses a sender whose stake is zero in the newest block's bond
    cache; so a jailed validator is never among the finalised fringe's senders. The Rust facts are
    `rholang/src/native_state.rs::select_active_excluding` (the draw excludes the jailed) and
    `casper/src/blocks/proposer/proposer.rs::is_active_validator` (the proposer refuses an
    undrawn sender). -/
def gatedSenders (s : State) (senders : Validator → Bool) : Prop :=
  ∀ v, jailTerm s v > 0 → senders v = false

/-- **Under the node's derivation a jailed validator is never present.** The free-parameter reading
    of `releaseIfPresent` proves a conditional; this theorem supplies the antecedent's negation for
    every reachable boundary, which is what makes the release clause dead code in this build. -/
theorem a_jailed_validator_is_never_present (s : State) (senders : Validator → Bool)
    (hgate : gatedSenders s senders) (v : Validator) (hjailed : jailTerm s v > 0) :
    presentFromSenders senders v = false :=
  hgate v hjailed

/-- **Block-authored presence cannot release a jailed validator.** The release branch needs
    `present v = true`; the derivation and the gate give `false`, so `releaseIfPresent` is the
    identity on the state. C344's release/renewal clause is therefore not proven by the algebra —
    it is refuted as unreachable until the presence signal changes (deploy signers, or a fixed
    non-renewing term). -/
theorem block_presence_cannot_release_a_jailed_validator (s : State) (senders : Validator → Bool)
    (hgate : gatedSenders s senders) (v : Validator) (hjailed : jailTerm s v > 0) :
    releaseIfPresent (presentFromSenders senders) s v = s := by
  have hp : presentFromSenders senders v = false := hgate v hjailed
  simp [releaseIfPresent, hp]

/-- **Block-authored presence renews a jailed validator.** At the expiry boundary the term is
    renewed to the full sentence, which is the reachable arm of the rule in this build. -/
theorem block_presence_renews_a_jailed_validator (s : State) (senders : Validator → Bool)
    (hgate : gatedSenders s senders) (J : Nat) (v : Validator) (hterm : jailTerm s v ≤ 1)
    (hjailed : jailTerm s v > 0) :
    jailTerm (renewIfAbsent (presentFromSenders senders) J s v) v = J :=
  a_still_absent_validator_is_renewed (presentFromSenders senders) J s v hterm
    (hgate v hjailed)

/-- **A banished validator cannot rebond.** The Rust `bond` refuses a banished key before its own
    checks (`native_state.rs:1874-1878`), which is the half that makes a passing vote's ejection
    permanent. This models the guard; `a_banished_validator_cannot_rebond` is its Rust witness. -/
def rebond (s : State) (v : Validator) (stake : Nat) : State :=
  if v ∈ s.banned then s else { s with pool := (v, stake) :: s.pool }

theorem a_banished_validator_cannot_rebond (s : State) (v : Validator) (stake : Nat)
    (h : v ∈ s.banned) : (rebond s v stake).pool = s.pool := by
  simp [rebond, h]

/-- The unguarded shape is what the rule forbids: without the ban check, a banished validator's bond
    re-enters the pool. This is the falsifier that keeps the guard from being an empty predicate. -/
def unguardedRebond (s : State) (v : Validator) (stake : Nat) : State :=
  { s with pool := (v, stake) :: s.pool }

theorem an_unguarded_rebond_reaches_a_banished_validator :
    let s : State :=
      { pool := [], active := [], absent := [], jailed := [], banned := [1],
        windows := [], ballots := [], anchor := 0 }
    (unguardedRebond s 1 10).pool ≠ s.pool := by
  decide

end Liveness

end Rchain
