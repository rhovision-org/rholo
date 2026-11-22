import Rchain.Pos
import Rchain.Casper.Stake

/-!
# Law 16d — the bonds cache equals the PoS state

The finalizer's gates run on a bonds map — `calculate_fringe`'s stake and `check_min_messages`' sender
coverage both take `bonds_map` (`block-storage/src/dag/finalizer.rs:29,141,216-217`) — and laws 14a/14b
therefore model `bonds : Bonds` as an *argument* to the gate. This module is where that argument's
provenance is modelled, because the port does not assume the map: it *establishes* it, from the PoS
state, at the one site the row's note names.

**The port's three sources**, in `compute_bonds`'s caller (`casper/src/multi_parent_casper.rs:150-172`):

1. **The fringe's state**, when something has finalised — `bonds_map` is read from the PoS contract at
   the fringe's state hash.
2. **The newest justification's post-state**, while nothing has finalised: `newest_justification(…)`'s
   block, its `post_state_hash`, and `runtime.compute_bonds(&state_hash)` on it (`:161-165`).
3. **The maps the justifications carry**, as a *fallback* for an unreadable state (`:170-178`) — and
   there the port **requires them to agree** (`"justifications disagree on the bonds map"`), because
   requiring agreement unconditionally is what wedged a chain permanently on the first bond or
   withdrawal before its first finalisation (#73, the comment's own record).

`compute_bonds` itself is one read — `get_native(PREFIX_POS, pos_active_key())`
(`casper/src/runtime_manager.rs:1374-1380`), i.e. the **`pos:active`** leaf, decoded as a bonds map.

## What the state's own answer is, and the model finding this module rests on

`pos:active` is a `BTreeMap<Validator, NonNegI64>` — **with stakes** (`native_state.rs:796-798`), written
by `select_active(&pool, …)` at a boundary (`:1152-1157`), by genesis installation (`:937`) and by
`slash` (`:1229`) — and **not** by `bond`. So between boundaries the pool's stakes move and the active
map's do not: the map the finalizer's gates read is the *boundary snapshot*, which is law 44's own
"a bond pools but does not activate", seen from the finalizer's side.

**`Rchain/Pos.lean`'s `PosState` can supply that map now**, and the sentence that used to stand here is
worth keeping as the record of what it cost: its `active` was a `List Validator` — ids, no stakes
(`Pos.lean:184`) — with the stakes living in `pool`, so deriving the bonds map from the model's state
would have answered with the *current pool stakes*, which for a validator that bonded after the last
boundary is a **different map from the port's**. That is why this module began by taking the active map
as an explicit `ActiveBonds` input. `PosState.active` now carries the stakes it selected (AUDIT C92's
fix), and with it the state's side is statable: `bondsOfState s.active` is the map the gates read, a
bond between boundaries does **not** move it, and a `pool`-derived reading **would** — that pair below
is the content, because either half alone is a spelling. -/

namespace Rchain

/-- The port's `pos:active` leaf: the consensus set **with the stakes recorded when it was selected**
    (`BTreeMap<Validator, NonNegI64>`, `native_state.rs:796-798`). Not a set of ids: the stakes are the
    whole point, and they are a boundary snapshot rather than a view of `pool`. -/
abbrev ActiveBonds := List (Validator × Nat)

/-- **The state's own answer** — the bonds map the finalizer's gates take, as `compute_bonds` reads it
    from `pos:active` and as `Bonds` (the gates' type) spells it. -/
def bondsOfState (active : ActiveBonds) : Bonds := active.map (fun b => (b.1.id, b.2))

/-- **The inactivity leak** — retire the silent validators from the active snapshot at a boundary.
    This is the map transition the boundary applies before the finality gate is asked again; it is
    the priced instrument Law 52's clause b names for a bond that keeps a quorum unreachable. -/
def leak (active : ActiveBonds) (silent : List Sender) : ActiveBonds :=
  active.filter (fun b => decide (b.1.id ∉ silent))

/-- The leak retires only validators it was told are silent. -/
theorem leak_retires_only_the_silent (active : ActiveBonds) (silent : List Sender) :
    ∀ b ∈ leak active silent, b.1.id ∉ silent := by
  intro b hb
  exact of_decide_eq_true (List.mem_filter.mp hb).2

/-- A leaked sender is no longer in the bonds map the gates read. -/
theorem the_leaked_sender_is_not_in_the_quorum (active : ActiveBonds) (silent : List Sender)
    {v : Sender} (hv : v ∈ silent) :
    v ∉ bondedSenders (bondsOfState (leak active silent)) := by
  intro h
  have hleak : v ∈ (leak active silent).map (fun b => b.1.id) := by
    simpa [bondedSenders, bondsOfState] using h
  simp only [List.mem_map] at hleak
  obtain ⟨b, hb, hbid⟩ := hleak
  exact leak_retires_only_the_silent active silent b hb (by simpa [hbid] using hv)

/-- **The leak never raises the quorum denominator.** -/
theorem totalStake_leak_le (active : ActiveBonds) (silent : List Sender) :
    totalStake (bondsOfState (leak active silent))
      ≤ totalStake (bondsOfState active) := by
  induction active with
  | nil => simp [leak, bondsOfState, totalStake]
  | cons b bs ih =>
    by_cases h : b.1.id ∈ silent
    · simp [leak, h, bondsOfState, totalStake] at ih ⊢
      omega
    · simp [leak, h, bondsOfState, totalStake] at ih ⊢
      omega

/-- A justification, reduced to the two things the bonds sync reads about it: the active map of the
    state at its block's post-state, and the bonds map its block **carried** (the port's
    `Justification.bonds_map`, `casper_message.rs`). `seqNum` is what `newest_justification` orders by
    (the justified block's `sequence_number`, `multi_parent_casper.rs:161`). -/
structure Justification where
  /-- The justified block's sequence number — the port's `newest_justification` key. -/
  seqNum : Nat
  /-- The `pos:active` map of the state at that block's post-state. -/
  active : ActiveBonds
  /-- The bonds map the block carries (its own view at production time). -/
  carried : Bonds

/-- The port's `newest_justification`: the justification with the greatest sequence number, or `none`
    for an empty list. The port's `.ok_or_else(|| "no justifications to read the bonds map from")` is
    the `none` arm — the genesis never reaches it, as its comment records (an empty parent set is
    refused earlier). -/
def newestJustification : List Justification → Option Justification
  | [] => none
  | j :: js =>
    match newestJustification js with
    | none => some j
    | some k => some (if j.seqNum < k.seqNum then k else j)

/-- **Law 16d, the state's side — the map the gates read is the state's *selected* map, and a bond does
    not move it.** `bond` writes `pool` and leaves `active` alone (law 44), and the gates read
    `bondsOfState s.active`, so a validator that bonds between boundaries does **not** enter the map the
    fringe's stake and `checkMinMessages`' coverage are computed from until a boundary selects it. This is
    C92's finding stated: the coarser model, whose only stake-carrying field was `pool`, could not state
    it at all — and the second theorem is what makes this one a claim rather than a spelling. -/
theorem a_bond_does_not_move_the_map_the_gates_read (s : PosState) (v : Validator) (stake : Nat) :
    bondsOfState (bond s v stake).active = bondsOfState s.active := rfl

/-- **…and the reading is load-bearing, not cosmetic**: the map a *pool*-derived answer would give moves
    on the very same bond — by exactly one entry — so the port's choice to read `pos:active` rather than
    `pos:bonds` is doing work. Stated without hypotheses (a bond prepends unconditionally, so the lengths
    differ whatever the pool held), and it is the falsifier of the theorem above: make that one read
    `pool` instead of `active` and the two together are a contradiction. -/
theorem the_pool_derived_map_moves_where_the_states_does_not (s : PosState) (v : Validator) (stake : Nat) :
    bondsOfState (bond s v stake).pool ≠ bondsOfState s.pool := by
  intro h
  have hlen := congrArg List.length h
  -- enough on its own: `bond` prepends, so the two lengths differ by one — the arithmetic is in `simp`'s
  -- own contradiction step, which is why the `omega` a first draft carried had no goal left to close.
  simp [bond, bondsOfState] at hlen

/-- **Source two**: the newest justification's state — what the port reads while nothing has finalised. -/
def bondsFromNewestState (js : List Justification) : Option Bonds :=
  (newestJustification js).map (fun j => bondsOfState j.active)

/-- **Source three, the fallback**: the maps the justifications carry, which **must agree** — `none` is
    the port's `Err("justifications disagree on the bonds map")`, and it is a refusal rather than a
    choice, which is what #73's chain-wedging defect was about. -/
def bondsFromCarried : List Justification → Option Bonds
  | [] => some []
  | j :: rest => if rest.all (fun k => k.carried = j.carried) then some j.carried else none

/-- The newest justification is one of them — so the agreement rule (which ranges over the whole list)
    covers it. -/
theorem newestJustification_mem {js : List Justification} {j : Justification}
    (h : newestJustification js = some j) : j ∈ js := by
  induction js with
  | nil => simp [newestJustification] at h
  | cons k ks ih =>
    simp only [newestJustification] at h
    split at h
    · injection h with h'
      subst h'
      exact List.mem_cons_self ..
    · rename_i m hm
      by_cases hlt : k.seqNum < m.seqNum
      · simp only [hlt, if_true] at h
        injection h with h'
        subst h'
        exact List.mem_cons_of_mem k (ih hm)
      · simp only [hlt, if_false] at h
        injection h with h'
        subst h'
        exact List.mem_cons_self ..

/-- A non-empty list always has a newest justification: the head is a candidate, so the port's
    `no justifications to read the bonds map from` cannot be reached from one. -/
theorem newestJustification_cons_ne_none (j : Justification) (js : List Justification) :
    newestJustification (j :: js) ≠ none := by
  rw [newestJustification]
  split <;> simp

/-- **The two sources agree when the carried maps are honest *and the bond set has not moved across
    them*.** Both hypotheses are load-bearing, and the second is the interesting one:

    - *honest*: each justification's carried map is the map its own state names — the invariant a block
      producer maintains, and the port's own words ("a block's state follows from the DAG alone, so
      honest nodes agree on it by construction");
    - *unmoved*: the justifications' states name the **same** active map. They are different blocks at
      different heights, so their states differ — but a difference that does not touch the bond set
      leaves the map alone, and it is exactly when the bond set *does* move across the justifications
      that the fallback cannot answer (see `a_disagreeing_set_is_refused`). That is why the port reads
      the *state* whenever it can and keeps the carried maps for the unreadable case only.

    Stated for a non-empty list, which is the port's own domain: `newest_justification` is
    `.ok_or_else(|| "no justifications to read the bonds map from")` (`:161`), and genesis — the one
    empty-parent case — is refused before this site (the comment at `:158-160`). -/
theorem the_sources_agree (j : Justification) (js : List Justification)
    (h : ∀ x ∈ j :: js, x.carried = bondsOfState x.active)
    (hmoved : ∀ x ∈ j :: js, ∀ y ∈ j :: js, bondsOfState x.active = bondsOfState y.active) :
    bondsFromCarried (j :: js) = bondsFromNewestState (j :: js) := by
  cases hnew : newestJustification (j :: js) with
  | none => exact absurd hnew (newestJustification_cons_ne_none j js)
  | some n =>
    have hn : n ∈ j :: js := newestJustification_mem hnew
    have hjn : bondsOfState j.active = bondsOfState n.active :=
      hmoved j (List.mem_cons_self ..) n hn
    have hall : js.all (fun k => k.carried = j.carried) = true := by
      apply List.all_eq_true.mpr
      intro m hm
      show decide (m.carried = j.carried) = true
      rw [decide_eq_true_eq]
      calc m.carried = bondsOfState m.active := h m (List.mem_cons_of_mem j hm)
        _ = bondsOfState j.active := hmoved m (List.mem_cons_of_mem j hm) j (List.mem_cons_self ..)
        _ = j.carried := (h j (List.mem_cons_self ..)).symm
    rw [bondsFromCarried, if_pos hall, bondsFromNewestState, hnew]
    show some j.carried = some (bondsOfState n.active)
    rw [h j (List.mem_cons_self ..)]
    exact congrArg some hjn

/-- **A disagreeing set is refused, not silently resolved.** If two justifications carry different maps,
    the fallback answers `none` — the port's error — so the caller cannot run a gate on one node's view
    while believing it is the state's. This is the shape the port's own comment describes (#73): the
    alternative was to pick one, and picking one is how a chain wedges. -/
theorem a_disagreeing_set_is_refused (j k : Justification) (rest : List Justification)
    (hmem : k ∈ rest) (hne : j.carried ≠ k.carried) : bondsFromCarried (j :: rest) = none := by
  rw [bondsFromCarried, if_neg]
  intro hall
  exact hne (decide_eq_true_eq.mp (List.all_eq_true.mp hall k hmem)).symm

/-- **The hypotheses are not decoration, and this is the case they exclude.** Two justifications whose
    states straddle a bond change carry different maps — validator 7 at stake 1 in the older state, 2 in
    the newer, which is what law 44's gate lets happen between boundaries — and then:

    - the **fallback refuses** (`bondsFromCarried … = none`, the port's error), rather than silently
      running a gate on one validator's view;
    - the **state path answers**, with the newest state's map — which is why the port reads the state
      whenever the state is readable and keeps the carried maps for the unreadable case only.

    This is the #73 shape as a theorem: the two paths differ exactly where the bond set moved, so a
    caller that took the first map it found would be running the finalizer on a stale bond set. -/
theorem a_bond_change_between_justifications_is_refused :
    bondsFromCarried [⟨0, [({ id := 7 }, 1)], [(7, 1)]⟩, ⟨1, [({ id := 7 }, 2)], [(7, 2)]⟩] = none ∧
    bondsFromNewestState [⟨0, [({ id := 7 }, 1)], [(7, 1)]⟩, ⟨1, [({ id := 7 }, 2)], [(7, 2)]⟩]
      = some [(7, 2)] := by
  decide

/-- **The gates cannot tell which source answered** — the finalizer's gates take the map as an argument
    (`finalizer.rs:29,141,216-217`), so two equal maps give equal verdicts. This is the step that
    makes laws 14a/14b's `bonds : Bonds` sound *whatever* its provenance, and it composes with
    `the_sources_agree` to say: on an honest DAG the gate's verdict is the same read either way. -/
theorem the_gate_reads_the_same_map {a b : Bonds} (h : a = b) (s : Sender) :
    stakeOf a s = stakeOf b s := by rw [h]

end Rchain
