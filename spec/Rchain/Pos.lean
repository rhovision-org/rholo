import Rchain.Casper.Stake
import Init.Omega

/-!
# Laws 44–47 — Proof-of-Stake: the epoch gate, the reward split, and the dust

The port's native PoS state (`rholang/src/native_state.rs`) implements the validator lifecycle — bond,
withdraw, slash, trust — and **the epoch gate it used to skip now exists**: where `close_block` once
refunded quarantined withdrawers and recomputed `pos:active` on *every* block, never reading
`PosParams.epoch_length`, it now runs those steps only at a boundary, matching the Scala contract it
replaces (`legacy/casper/src/main/resources/Pos.rhox`: `blockNumber % $$epochLength$$ == 0`, :517;
rewards computed at the boundary, `getCurrentEpochRewards` :241-256, and committed,
`commitCurrentEpochRewards` :568-576; only expired quarantines paid, :556-567, :592-621). The port
keeps the contract's *meaning* for a zero epoch length (`native_state.rs:632-647`: the divisor is
`max(epoch_length, 1)`, so a zero means one-block epochs rather than a division fault).

Laws 44 and 47 remain `open` for a different reason: what they name is the **state machine** the gate
guards, and until this module grows one the laws are claims about the Rust evidenced by Rust tests.
That is this file's Programme C item, and the two properties are conservation (an epoch moves value
between the vaults and mints none) and the release rule (a withdrawal is staged, then escrowed out of
the pool, then paid `bond + committed` — and only at a boundary past its quarantine).

## The reward split does not conserve, and the law has to say so

`getCurrentEpochRewards` divides **twice** with integer division:

    reward_i = pot * (bond_i / minimumBond) / (activeBonds / minimumBond)

so `Σ reward_i` is the pot *minus dust* — the sum of the floored shares, re-floored. A conservation law
written as an equality would be **false**, and the Scala's own comment does not say which it means. The
model settles it: `list_sum_div_le` and `sum_rewards_le_pot` below are the inequality, and the instance
at the end is a case where it is *strict* — which is what makes the statement falsifiable rather than a
restatement of an accounting identity.
-/

/-- The sum of a list of naturals. Defined here rather than imported: this file is about the arithmetic
of one fold, and Mathlib's big-operators import would pull a tree the spec does not otherwise need. -/
def nsum : List Nat → Nat
  | [] => 0
  | a :: rest => a + nsum rest

namespace Rchain

/-- The distributable pot of an epoch: `posBalance - totalBond - totalWithdraw - totalCommittedRewards`
(`Pos.rhox:241-256`). The Scala computes it in `Long` and the contract's guards keep it non-negative, so
the model is a `Nat` subtraction — the floor is the *stated* part, not an artefact of the type. -/
def rewardPot (posBalance totalBond totalWithdraw committed : Nat) : Nat :=
  posBalance - totalBond - totalWithdraw - committed

/-- One active validator's epoch reward, exactly as `getCurrentEpochRewards` divides it: the pot scaled
by the validator's bond over `minimumBond`, normalised by the active set's bond over `minimumBond`. -/
def reward (pot minimumBond activeBonds bond : Nat) : Nat :=
  pot * (bond / minimumBond) / (activeBonds / minimumBond)

/-- **The integer division that makes the split conservative**: `⌊a/m⌋ + ⌊b/m⌋ ≤ ⌊(a+b)/m⌋`. Two
floors never add up to more than the floor of the sum, which is why the reward formula leaves a
remainder rather than distributing the pot exactly. -/
theorem div_add_div_le (m a b : Nat) : a / m + b / m ≤ (a + b) / m := by
  rcases Nat.eq_zero_or_pos m with rfl | hm
  · simp
  -- Two floors add up to at most the floor of the sum: each division drops a remainder, and dropping
  -- two of them can only lose. Stated through `div_mul_le_self` rather than left to `omega`, because
  -- `(a / m) * m` is a product of two terms and `omega` is linear in its variables.
  · rw [Nat.le_div_iff_mul_le hm, Nat.add_mul]
    exact Nat.add_le_add (Nat.div_mul_le_self a m) (Nat.div_mul_le_self b m)

/-- The same fact over a list: the sum of the floored shares never exceeds the floor of the sum. The
induction is `div_add_div_le` once per element. -/
theorem list_sum_div_le (m : Nat) : ∀ (l : List Nat), nsum (l.map (· / m)) ≤ nsum l / m
  | [] => by simp [nsum]
  | a :: rest => by
    have ih := list_sum_div_le m rest
    have step := div_add_div_le m a (nsum rest)
    simp only [List.map_cons, nsum] at ih ⊢
    omega

/-- `Σ (pot * x_i) = pot * Σ x_i` — the factor the split multiplies each share by. -/
theorem nsum_map_mul_left (pot : Nat) : ∀ (l : List Nat), nsum (l.map (fun x => pot * x)) = pot * nsum l
  | [] => by simp [nsum]
  | a :: rest => by
    simp only [List.map_cons, nsum, nsum_map_mul_left pot rest]
    rw [Nat.mul_add]

/-- **Law 46 — the epoch's split never pays out more than the pot**, and the remainder is the dust of two
integer divisions. Stated over the list of active bonds rather than a map, because the formula only ever
reads each bond through `bond / minimumBond`.

The hypotheses are the contract's own: the active set's bond total is the sum of its members'
(`hactive`), and the normaliser is positive (`hD`) — which is the only hypothesis the proof needs, and
it is the one that carries `minimumBond > 0` with it: `activeBonds / minimumBond` is positive only if the
divisor is, so a `minimumBond` of zero is already excluded. `activeBonds / minimumBond = 0` would be a
contract that pays every validator nothing, which is the shape the hypothesis refuses. -/
theorem sum_rewards_le_pot (pot minimumBond activeBonds : Nat) (bonds : List Nat)
    (hactive : activeBonds = nsum bonds) (hD : 0 < activeBonds / minimumBond) :
    nsum (bonds.map (fun b => reward pot minimumBond activeBonds b)) ≤ pot := by
  have hstep :
      nsum (bonds.map (fun b => reward pot minimumBond activeBonds b))
        ≤ nsum (bonds.map (fun b => pot * (b / minimumBond))) / (activeBonds / minimumBond) := by
    simpa only [reward, List.map_map, Function.comp_def] using
      list_sum_div_le (activeBonds / minimumBond) (bonds.map (fun b => pot * (b / minimumBond)))
  have hsum : nsum (bonds.map (fun b => pot * (b / minimumBond)))
      = pot * nsum (bonds.map (fun b => b / minimumBond)) := by
    simpa only [List.map_map, Function.comp_def] using
      nsum_map_mul_left pot (bonds.map (fun b => b / minimumBond))
  have hfits : nsum (bonds.map (fun b => b / minimumBond)) ≤ activeBonds / minimumBond := by
    rw [hactive]
    exact list_sum_div_le minimumBond bonds
  have hmono : pot * nsum (bonds.map (fun b => b / minimumBond)) / (activeBonds / minimumBond)
      ≤ pot * (activeBonds / minimumBond) / (activeBonds / minimumBond) :=
    Nat.div_le_div_right (Nat.mul_le_mul_left pot hfits)
  calc nsum (bonds.map (fun b => reward pot minimumBond activeBonds b))
      ≤ nsum (bonds.map (fun b => pot * (b / minimumBond))) / (activeBonds / minimumBond) := hstep
    _ = pot * nsum (bonds.map (fun b => b / minimumBond)) / (activeBonds / minimumBond) := by rw [hsum]
    _ ≤ pot * (activeBonds / minimumBond) / (activeBonds / minimumBond) := hmono
    _ = pot := Nat.mul_div_cancel _ hD

/-- **The inequality is strict, so it is not an accounting identity in disguise.** `minimumBond = 3`,
bonds `[4, 8]`: the normaliser is `12 / 3 = 4`, and the validators' scaled shares are `4/3 = 1` and
`8/3 = 2`, so with a pot of ten they are paid `10 * 1 / 4 = 2` and `10 * 2 / 4 = 5` and the epoch
distributes **7 of 10** — three units of dust, which is the whole content of law 46. A statement that
said `Σ = pot` would be refuted by this line.

**And the shares must differ, which is the only reason the bonds above are 4 and 8** (AUDIT C149). This
witness used to bond `[4, 5]`, where `4/3 = 5/3 = 1`: the proportionality factor `bond / minimumBond`
was the identity for *every* validator in the instance, so a `reward` that dropped the factor entirely —
paying each active validator `pot / (activeBonds / minimumBond)` — satisfied this line exactly as the
real one did, and the module's Rust twin with it. The comment that stood here *stated* that collapse as
the evidence of strictness ("each validator's scaled share is `4/3 = 1` and `5/3 = 1`"), which is the
part worth keeping: a fixture whose parameters make the term under test the identity cannot witness that
term, however exact the assertion about it is, and reasoning carefully about the arithmetic does not
prevent it — this one was reasoned about. `8/3 = 2` is the smallest change that breaks the tie and keeps
the dust strict. -/
theorem the_dust_is_real :
    reward 10 3 12 4 + reward 10 3 12 8 = 7 ∧ (7 : Nat) < 10 := by decide

end Rchain

/-! ## The state machine the arithmetic above sits inside (laws 44 and 47)

The first half of this file models the epoch's *arithmetic* — the pot and the split. What laws 44 and 47
name is the **state machine** around it: the gate that decides whether an epoch happens at all, the four
ordered steps `close_block` runs when it does, and the three stages a withdrawal passes through. That
machine was missing, so both laws were claims about the Rust evidenced only by Rust tests. It is here
now, mirroring `rholang/src/native_state.rs` — field for field where a rule reads the field, and step for
step in `close_block`'s order (`:1083-1158`), because the order carries meaning: the rewards are
committed **before** a leaving validator is moved out of the pool, which is what lets a validator earn in
the epoch it leaves.

## What conservation is, and what it is not

`totalRev` sums the **coin fields only**. The pool, the requests, the claims and the committed map are
*liabilities against the staking vault* — ledger entries, not coins — which is why the Rust's own
`total_value` helper sums exactly the three coin fields (`native_state.rs:915-961`), and why the port's
`slash`-minted-before-`ba9e259a7` bug is this theorem's motivating counterexample: it credited the Coop
vault without debiting the staking vault. Counting the ledgers in the sum would make the theorem false,
not stronger.
-/

namespace Rchain

/-- A validator's claim on the staking vault: the bond escrowed out of the pool, and the block at which it
    may be paid (`Withdrawal`, `native_state.rs:389-404`). The struct stores the **bond only** — the
    reward is read from the committed map at payment time, which is what lets a validator be paid for the
    epoch it left in. -/
structure PosClaim where
  who : Validator
  bond : Nat
  deadline : Nat
deriving DecidableEq

/-- A staged withdrawal: the validator and the block at which its quarantine expires
    (`pos:pending_withdrawers`, `native_state.rs:89-94`). -/
structure PosRequest where
  who : Validator
  deadline : Nat
deriving DecidableEq

/-- A delegation (law 57): `amount` of a delegator's principal attributed to an operator's key
    (`pos:delegations`, `rholang/src/native_state.rs`). The port's record shape is exactly this triple,
    which is why the accrued-reward leaf (`pos:delegated_rewards`) reuses it — the same three fields,
    read as a credit rather than a principal. -/
structure PosDelegation where
  operator : Validator
  delegator : Validator
  amount : Nat
deriving DecidableEq

/-- A staged undelegation (`pos:pending_delegations`, law 57). Like `PosRequest` it records only a
    deadline: the principal stays in the pool, earning, until the boundary that acts on it. -/
structure PosUndelegation where
  operator : Validator
  delegator : Validator
  deadline : Nat
deriving DecidableEq

/-- An escrowed undelegated principal (`pos:delegation_claims`, law 57): out of the pool, held until the
    deadline, and — like `PosClaim` — still reachable by a slash on its operator until then. -/
structure PosDelegationClaim where
  operator : Validator
  delegator : Validator
  amount : Nat
  deadline : Nat
deriving DecidableEq

/-- The PoS state: the three coin fields, the eight ledgers, the active set, and the two parameters the
    transition reads.

    `user` is the **total** over every user vault rather than a map: every transfer in this mechanism
    either moves coins between the staking vault and a user's vault or between the staking and Coop
    vaults, so the total is what conservation is about and a per-address map would only add lookup
    lemmas. `active` is what step 4 recomputes; `pool` is the bond pool (`pos:bonds`), which a bond joins
    immediately but which the active set only follows at a boundary. -/
structure PosState where
  /-- The staking vault (`pos:vault`) — the epoch pot's source. -/
  vault : Nat
  /-- The Coop multisig vault (`pos:coop`) — where slashing confiscates to. -/
  coop : Nat
  /-- The total over every user vault. -/
  user : Nat
  /-- The bond pool (`pos:bonds`): every pooled validator and its stake — the **aggregate** since law 57,
      the operator's own bond plus every delegation to its key, which is what makes a delegated principal
      count in the draw and in what a slash reaches. -/
  pool : List (Validator × Nat)
  /-- The active set (`pos:active`) — recomputed by step 4, never by `bond` — **carrying the stakes it
      selected**, because that is what the port's leaf holds: a `BTreeMap<Validator, NonNegI64>` with
      stakes (`native_state.rs:796-798`), written by `select_active` at a boundary (`:1312-1317`), by
      genesis (`:1061`) and by `slash` (`:1429`). It is deliberately **not** a view of `pool`: between
      boundaries the pool's stakes move and this one does not, which is law 44's own gate seen from the
      side the finalizer's bonds map is read on (AUDIT C92). -/
  active : List (Validator × Nat)
  /-- Staged withdrawal requests (`pos:pending_withdrawers`). -/
  requests : List PosRequest
  /-- Escrowed withdrawal claims (`pos:withdrawers`). -/
  claims : List PosClaim
  /-- Committed rewards (`pos:committed`) — a claim ledger, not coins. -/
  committed : List (Validator × Nat)
  /-- Delegated principals (`pos:delegations`, law 57): a third party's stake attributed to an operator's
      key. The principal itself is **not** here rather than in `pool` — `pool` carries the aggregate — so
      this ledger is the attribution, not a second copy of the stake. -/
  delegations : List PosDelegation
  /-- Staged undelegations (`pos:pending_delegations`, law 57) — a request, like `requests`. -/
  undelegations : List PosUndelegation
  /-- Escrowed undelegated principals (`pos:delegation_claims`, law 57): the amount out of the pool and
      the block it may be paid at. Its operator is recorded for the same reason `claims` keeps its
      `who` — a slash still reaches it until the deadline passes. -/
  delegationClaims : List PosDelegationClaim
  /-- Accrued delegators' rewards (`pos:delegated_rewards`, law 57): the same triple read as a credit
      rather than a principal, so a delegator's share of an epoch is not the operator's to spend. -/
  delegatedRewards : List PosDelegation
  /-- `epochLength` (`PosParams.epoch_length`). -/
  epochLength : Nat
  /-- `quarantineLength` (`PosParams.quarantine_length`). -/
  quarantineLength : Nat
deriving DecidableEq

/-- **The conserved quantity**: every user vault plus the staking vault plus the Coop vault — the sum the
    Rust's `total_value` computes (`native_state.rs:915-961`). The ledgers are deliberately outside it:
    they are claims *against* `vault`, and counting them would count the same REV twice. -/
def totalRev (s : PosState) : Nat := s.vault + s.coop + s.user

/-- A ledger lookup — `0` for an absent key, the port's `unwrap_or(NonNegI64::zero())`
    (`native_state.rs:1257-1260`, `:1290`). -/
def lookup : List (Validator × Nat) → Validator → Nat
  | [], _ => 0
  | (w, x) :: rest, v => if w = v then x else lookup rest v

/-- Set a ledger key (insert or replace) — the port's `committed.insert`. -/
def setKey (l : List (Validator × Nat)) (v : Validator) (x : Nat) : List (Validator × Nat) :=
  if l.any (fun p => p.1 = v) then l.map (fun p => if p.1 = v then (v, x) else p)
  else (v, x) :: l

/-- **The epoch divisor**: `max(epoch_length, 1)` (`epoch_divisor`, `native_state.rs:632-647`). The
    contract divides by `$$epochLength$$` directly and faults on zero; the port reads a zero as "every
    block is a boundary", which is what `epoch_length = 1` means to the contract. -/
def divisor (s : PosState) : Nat := max s.epochLength 1

/-- **The gate** (`is_epoch_boundary`, `native_state.rs:649-653`): an epoch runs exactly when the block
    number is a multiple of the divisor. -/
def isBoundary (s : PosState) (n : Nat) : Bool := n % divisor s == 0

/-- The deadline a withdrawal staged at block `n` is given:
    `quarantineLength + epochLength * (1 + n / divisor)` (`native_state.rs:1179-1184`, `Pos.rhox:381`). -/
def withdrawDeadline (s : PosState) (n : Nat) : Nat :=
  s.quarantineLength + s.epochLength * (1 + n / divisor s)

/-- **The withdrawal request** (`withdraw`, `native_state.rs:1151-1189`): the request records a deadline
    and changes **nothing else** — the validator stays in the pool and in the active set, still earning.
    This is law 47's first stage. -/
def stage (s : PosState) (v : Validator) (n : Nat) : PosState :=
  { s with requests := ⟨v, withdrawDeadline s n⟩ :: s.requests }

/-- **A bond** (`bond`, `native_state.rs:1074-1149`): the stake moves from the user's vault into the
    staking vault and the validator joins the **pool** — and the active set is untouched. Activation is
    the boundary's step 4, which is law 44's "pooled but not activated". -/
def bond (s : PosState) (v : Validator) (stake : Nat) : PosState :=
  { s with user := s.user - stake, vault := s.vault + stake, pool := (v, stake) :: s.pool }

/-- Step 1 of `close_block`: the epoch's rewards are written into the **committed** ledger
    (`native_state.rs:1253-1266`). The amounts are `Rchain.reward`'s (modelled above); the machine takes
    them as given, because no amount moves a coin — and *that* is what conservation claims. -/
def commitRewards (r : Validator → Nat) (s : PosState) : PosState :=
  { s with
    committed := s.pool.foldl
      (fun (l : List (Validator × Nat)) (wb : Validator × Nat) =>
        setKey l wb.1 (lookup l wb.1 + r wb.1)) s.committed }

/-- Step 2: every staged request becomes an escrowed claim — its bond leaves the pool and its deadline is
    recorded (`native_state.rs:1268-1279`). No coin moves: the escrowed bond is still in the vault and the
    validator's own vault is still empty. Law 47's second stage. -/
def movePending (s : PosState) : PosState :=
  { s with
    claims := s.claims ++ s.requests.filterMap (fun r =>
      match s.pool.find? (fun wb => wb.1 = r.who) with
      | some wb => some ⟨r.who, wb.2, r.deadline⟩
      | none => none),
    pool := s.pool.filter (fun wb => !(s.requests.any (fun r => r.who = wb.1))),
    requests := [] }

/-- The claims a boundary pays: those whose quarantine has elapsed (`native_state.rs:1281-1285`, the
    filter `w.deadline <= block_number`). -/
def dueClaims (s : PosState) (n : Nat) : List PosClaim := s.claims.filter (fun c => c.deadline ≤ n)

/-- What one claim is paid: its bond **plus** its committed reward, read from the ledger at payment time
    (`native_state.rs:1290-1291`; the contract's `bonds + committedRewards.getOrElse(pk, 0)`,
    `Pos.rhox:604`). -/
def payoutOf (s : PosState) (c : PosClaim) : Nat := c.bond + lookup s.committed c.who

/-- The total a boundary pays out. -/
def duePayout (s : PosState) (n : Nat) : Nat := nsum ((dueClaims s n).map (payoutOf s))

/-- Step 3: pay every due claim — debit the staking vault, credit the validator's vault, remove the claim
    and its committed entry (`native_state.rs:1286-1302`). Written as one batch rather than a fold, which
    is the same function whenever a validator has at most one claim: the port's `withdrawers` is a *map*,
    so that is an invariant of the mechanism rather than a coincidence.

    **`none` is the port's refusal, not a silent half-payment.** `debit_pos_vault` returns an error when
    the vault cannot cover the transfer and `close_block` propagates it, having persisted nothing (its
    writes come after all four steps, `native_state.rs:1313-1317`). An unguarded `Nat` subtraction would
    instead truncate the debit and *mint* the difference — a model that quietly does the wrong thing,
    which is the failure mode this project's Rust avoids structurally and its models are supposed to as
    well. So the payment is partial on purpose, and the conservation theorem below carries no hypothesis:
    it is stated about whatever the step returned. -/
def payDue (s : PosState) (n : Nat) : Option PosState :=
  if duePayout s n ≤ s.vault then
    some { s with
      vault := s.vault - duePayout s n
      user := s.user + duePayout s n
      claims := s.claims.filter (fun c => !(c.deadline ≤ n))
      committed := s.committed.filter (fun p => !((dueClaims s n).any (fun c => c.who = p.1))) }
  else none

/-- Step 4: the active set for the epoch that starts now — the pool's members **with their stakes**,
    minus anyone whose bond is escrowed in a claim (`select_active`, `native_state.rs:558-618`). The
    stakes are kept rather than dropped: this is the `pos:active` map the finalizer's gates read, and a
    bond that arrives between boundaries must not move it (law 44, and AUDIT C92's finding). -/
def reselect (s : PosState) : PosState :=
  { s with
    active := s.pool.filter (fun wb => !(s.claims.any (fun c => c.who = wb.1))) }

/-- **The epoch transition**, in `close_block`'s order (`native_state.rs:1219-1354`): commit the rewards,
    move the staged withdrawals into claims, pay the claims whose quarantine elapsed, re-select the active
    set. The gate is *outside* this function (`closeBlock`), because a transition that branched on it
    would make "off a boundary nothing changes" a restatement of its own definition. -/
def epochStep (r : Validator → Nat) (s : PosState) (n : Nat) : Option PosState :=
  (payDue (movePending (commitRewards r s)) n).map reselect

/-- `close_block`: at a boundary the epoch runs; off one, **nothing is written at all** — the port returns
    before touching state (`native_state.rs:1224-1225`). -/
def closeBlock (r : Validator → Nat) (s : PosState) (n : Nat) : Option PosState :=
  if isBoundary s n then epochStep r s n else some s

/-! ### Conservation -/

/-- **A payout is a transfer, not a mint** — the property the port's `total_value` helper exists to assert.
    No hypothesis: the guard *is* the port's refusal (`debit_pos_vault` fails a transfer the vault cannot
    cover, `native_state.rs:1292`, rather than half-paying it), so whatever the step returned conserves. -/
theorem payDue_conserves (s : PosState) (n : Nat) {s' : PosState} (h : payDue s n = some s') :
    totalRev s' = totalRev s := by
  unfold payDue at h
  split at h
  · simp only [Option.some.injEq] at h
    subst h
    simp only [totalRev]
    omega
  · exact absurd h (by simp)

/-- Steps 1, 2 and 4 leave the coin fields exactly as they were: they are ledger steps. -/
theorem the_ledger_steps_leave_the_coins (r : Validator → Nat) (s : PosState) :
    totalRev (commitRewards r s) = totalRev s ∧
    totalRev (movePending s) = totalRev s ∧
    totalRev (reselect s) = totalRev s :=
  ⟨rfl, rfl, rfl⟩

/-- **Law 44/47 — conservation of an epoch**: whatever the transition pays, it pays out of the staking
    vault, so the REV held by users, the staking vault and the Coop vault is invariant across an epoch.
    This is the theorem the file could not state before it had a state machine, and the one whose absence
    let the port mint in `slash` until `ba9e259a7`. -/
theorem epochStep_conserves (r : Validator → Nat) (s : PosState) (n : Nat) {s' : PosState}
    (h : epochStep r s n = some s') :
    totalRev s' = totalRev s := by
  -- `reselect` is the outermost step and a ledger step, so it comes off first; then the payment, then
  -- the two ledger steps beneath it. Each `have` is the lemma instantiated at the state it is applied
  -- to, which is what `rw` needs — `totalRev (reselect ((…)))` does not contain `totalRev (payDue …)`
  -- as a subterm, so the rewrites have to peel one layer at a time.
  unfold epochStep at h
  cases hpay : payDue (movePending (commitRewards r s)) n with
  | none => simp [hpay] at h
  | some paid =>
      simp only [hpay, Option.map_some, Option.some.injEq] at h
      cases h
      have hcon := payDue_conserves (movePending (commitRewards r s)) n hpay
      have hres := (the_ledger_steps_leave_the_coins r paid).2.2
      have hmov := (the_ledger_steps_leave_the_coins r (commitRewards r s)).2.1
      have hcom := (the_ledger_steps_leave_the_coins r s).1
      rw [hres, hcon, hmov, hcom]

/-- Off a boundary the transition writes nothing — stated of `closeBlock`, so it is a fact about the gate
    rather than a restatement of a branch inside the transition. -/
theorem closeBlock_off_a_boundary (r : Validator → Nat) (s : PosState) (n : Nat)
    (h : isBoundary s n = false) : closeBlock r s n = some s := by
  simp [closeBlock, h]

/-! ### The release rule (law 47) and the gate's two halves (law 44) -/

/-- **Law 47, stage one — staged, not moved.** A withdrawal records a deadline and changes nothing else:
    the validator is still in the pool, still active, and no coin has moved. -/
theorem a_staged_withdrawal_moves_no_coins (s : PosState) (v : Validator) (n : Nat) :
    (stage s v n).requests = ⟨v, withdrawDeadline s n⟩ :: s.requests ∧
    totalRev (stage s v n) = totalRev s ∧
    (stage s v n).pool = s.pool ∧
    (stage s v n).active = s.active :=
  ⟨rfl, rfl, rfl, rfl⟩

/-- **Law 47, stage two — escrowed, not paid.** At the boundary after the request the bond leaves the pool
    and becomes a claim carrying the recorded deadline, and the payer's vault is still empty: the claim
    stores the **bond only** (the contract stores `allBonds.get(pk)` at `Pos.rhox:582` and adds
    `committedRewards` at `:604`, which is the order the port keeps). -/
theorem the_move_escrows_the_bond_and_pays_nothing (s : PosState) (v : Validator) (stake d : Nat)
    (h : s.pool.find? (fun wb => wb.1 = v) = some (v, stake)) :
    (movePending { s with requests := [⟨v, d⟩] }).claims = s.claims ++ [⟨v, stake, d⟩] ∧
    (movePending { s with requests := [⟨v, d⟩] }).user = s.user := by
  constructor
  · simp [movePending, h, List.filterMap_cons]
  · rfl

/-- **Law 47, stage three — paid `bond + committed`.** What a due claim is paid is its bond plus the
    reward committed to it (the first conjunct is the definition, so a payout that dropped either part
    would fail to type-check here), and a successful payment credits the validator's vault by exactly
    that (the second, under the same guard the port applies). -/
theorem a_due_claim_is_paid_its_bond_plus_its_committed (s : PosState) (c : PosClaim) {s' : PosState}
    (h : payDue s c.deadline = some s') :
    payoutOf s c = c.bond + lookup s.committed c.who ∧ s'.user = s.user + duePayout s c.deadline := by
  refine ⟨rfl, ?_⟩
  unfold payDue at h
  split at h
  · cases h
    rfl
  · exact absurd h (by simp)

/-- **…and a claim before its deadline is not paid**: it is still in the ledger after the boundary's step
    3, which is what "only the expired quarantines are paid" means for the claim itself. -/
theorem a_claim_before_its_deadline_is_not_paid (s : PosState) (c : PosClaim) (n : Nat)
    {s' : PosState} (hc : c ∈ s.claims) (h : ¬ c.deadline ≤ n) (hp : payDue s n = some s') :
    c ∈ s'.claims := by
  unfold payDue at hp
  split at hp
  · simp only [Option.some.injEq] at hp
    subst hp
    simp only [List.mem_filter]
    exact ⟨hc, by simp [h]⟩
  · exact absurd hp (by simp)

/-- **Law 44 — pooled, not activated.** A bond moves the stake into the staking vault and joins the pool,
    and leaves the active set exactly as it was; activation is the boundary's step 4, which is why the
    port's own test must call `close_block` before the validator is in the consensus set. -/
theorem a_bond_pools_but_does_not_activate (s : PosState) (v : Validator) (stake : Nat) :
    (bond s v stake).pool = (v, stake) :: s.pool ∧
    (bond s v stake).active = s.active ∧
    (bond s v stake).vault = s.vault + stake ∧
    (bond s v stake).user = s.user - stake :=
  ⟨rfl, rfl, rfl, rfl⟩

/-- **Law 44 — and the boundary is what activates it.** Step 4 makes the pool — the newly bonded
    validator included, **with the stakes it holds at this boundary** — the active set, minus anyone
    whose bond is escrowed in a claim; so a bond and a boundary in the same block do activate it. This is
    the port's `bond_escrows_the_stake_and_activates_at_the_boundary` in the model.

    **The statement is unconditional now, and that is the stakes' doing.** It used to assume
    `s'.claims = []` because the active set was a list of *ids*: `s'.pool.map (·.1)` could only equal
    step 4's output when the filter removed nobody. Carrying the stakes makes the conclusion *the
    filter itself*, so the hypothesis has nothing left to do — the escrowed-claim case is the same
    theorem with a non-empty claim list, which is strictly more than the no-claim instance the port's
    test exercises. The old hypothesis is not weaker, it is inert. -/
theorem a_boundary_activates_the_pool (r : Validator → Nat) (s : PosState) (n : Nat)
    {s' : PosState} (hp : payDue (movePending (commitRewards r s)) n = some s') :
    ∃ s'', epochStep r s n = some s'' ∧
      s''.active = s'.pool.filter (fun wb => !(s'.claims.any (fun c => c.who = wb.1))) :=
  ⟨reselect s', by simp [epochStep, hp], by simp [reselect]⟩

/-- **The ordering the release rule depends on**: the move of step 2 leaves the committed ledger
    exactly as step 1 wrote it, so a validator that leaves the pool at this boundary is still paid
    against a reward committed for the epoch it was in. The two steps are ordered commit-then-move in
    `epochStep` for this reason (`native_state.rs:1253-1279`, `Pos.rhox:568-588`). -/
theorem the_move_does_not_disturb_the_ledger (r : Validator → Nat) (s : PosState) (v : Validator) :
    lookup (movePending (commitRewards r s)).committed v
      = lookup (commitRewards r s).committed v := by
  simp [movePending]

/-- **…and the reward is *in* that entry**: the same ordering checked on the run the port's own test
    builds — one pooled validator, a withdrawal staged at block 3, the epoch reward committed at the
    boundary. The value is read back from the ledger **after** the move, so the theorem fails if the
    commitment is dropped, reordered after the move, or keyed to the wrong validator. (The general form
    of this — for any pool and any reward function — is a `foldl` induction over `setKey` and is owed;
    this is the instance the port's test exercises, decided rather than described.) -/
theorem the_reward_is_committed_before_the_leave :
    lookup (movePending (commitRewards (fun _ => 5)
      { vault := 40, coop := 0, user := 0, pool := [(⟨0⟩, 40)], active := [(⟨0⟩, 40)],
        requests := [⟨⟨0⟩, 9⟩], claims := [], committed := [(⟨0⟩, 0)],
        delegations := [], undelegations := [], delegationClaims := [], delegatedRewards := [],
        epochLength := 1, quarantineLength := 0 })).committed ⟨0⟩ = 5 := by
  decide

/-! ## The slash, graded by the offence (AUDIT C199)

The transitions above are the port's `close_block`. This section is the port's `slash`, and it is here
because it is the other half of what a validator's stake is exposed to: the epoch says what a validator
*earns*, and this says what it can **lose**.

**Before C199 the model had no slash at all**, which is why the rule could be argued in prose for as long
as it was: the port took the whole bond, the register recorded that it did, and nothing here could state
that a *bounded* loss was even a thing to ask for. The bound is now a theorem, and so is the property
that keeps the offender's balance from shrinking the pot for everyone else (C197). -/

/-- What a validator holds in the PoS system: its **bond**, its **accrued and unwithdrawn rewards**, and
    an **escrowed claim**. The staking vault holds all three, and the port's `slash` clears all three.

    **Delegated stake enters this sum exactly once, and through `pool`.** A delegation's principal is
    *inside* the operator's aggregate `pool` entry — that is what "aggregate" means — so the ledger
    `s.delegations` is deliberately **not** added here: doing so would count a delegator's REV twice and
    overstate what a slash may take. What *is* added is the two delegated holdings that have left the
    pool or never joined it: a delegator's accrued reward (`pos:delegated_rewards`, which is a credit
    against the vault that `committed` does not carry) and an escrowed undelegated principal
    (`pos:delegation_claims`, out of the pool until its deadline passes, like `claims`). -/
def atRisk (s : PosState) (v : Validator) : Nat :=
  lookup s.pool v + lookup s.committed v
    + (s.claims.filter (fun c => c.who = v)).foldl (fun acc c => acc + c.bond) 0
    + (s.delegatedRewards.filter (fun d => d.operator = v)).foldl (fun acc d => acc + d.amount) 0
    + (s.delegationClaims.filter (fun c => c.operator = v)).foldl (fun acc c => acc + c.amount) 0

/-- The tiers, in basis points of everything at risk (`SlashSeverity::basis_points`). -/
def malicious : Nat := 10000

def misdemeanour : Nat := 2500

def honestMistake : Nat := 1000

/-- **A slash, graded by the offence** (`slash`, `native_state.rs`): the validator leaves the pool, the
    active set, the requests, the claims and the committed ledger; `bps` basis points of what it held go
    to the Coop vault, and **the remainder returns to its own vault**.

    `bps = 10000` is the pre-tier rule — it takes everything — and it is what a record written before the
    tiers existed replays to. -/
def slash (s : PosState) (v : Validator) (bps : Nat) : PosState :=
  let risk := atRisk s v
  let taken := risk * bps / 10000
  { vault := s.vault - risk
  , coop := s.coop + taken
  , user := s.user + (risk - taken)
  , pool := s.pool.filter (fun p => p.1 ≠ v)
  , active := s.active.filter (fun p => p.1 ≠ v)
  , requests := s.requests.filter (fun r => r.who ≠ v)
  , claims := s.claims.filter (fun c => c.who ≠ v)
  , committed := s.committed.filter (fun p => p.1 ≠ v)
  , delegations := s.delegations.filter (fun d => d.operator ≠ v)
  , undelegations := s.undelegations.filter (fun u => u.operator ≠ v)
  , delegationClaims := s.delegationClaims.filter (fun c => c.operator ≠ v)
  , delegatedRewards := s.delegatedRewards.filter (fun d => d.operator ≠ v)
  , epochLength := s.epochLength
  , quarantineLength := s.quarantineLength }

/-- A tier never takes more than everything: `bps ≤ 10000` bounds the confiscation by what is at risk. -/
theorem taken_le_risk (s : PosState) (v : Validator) {bps : Nat} (h : bps ≤ 10000) :
    atRisk s v * bps / 10000 ≤ atRisk s v := by
  have h1 : atRisk s v * bps ≤ 10000 * atRisk s v := by
    rw [Nat.mul_comm 10000]
    exact Nat.mul_le_mul_left _ h
  exact Nat.div_le_of_le_mul h1

/-- **A slash is a transfer, not a mint**: the three vaults sum to what they summed to before. This is
    `slash`'s version of the conservation law the epoch's own transitions carry. -/
theorem slash_conserves (s : PosState) (v : Validator) {bps : Nat}
    (hbps : bps ≤ 10000) (hvault : atRisk s v ≤ s.vault) :
    totalRev (slash s v bps) = totalRev s := by
  have htaken := taken_le_risk s v hbps
  simp only [totalRev, slash]
  omega

/-- **A milder tier never takes more** — the user-facing sentence "a punishment befitting the crime",
    stated as an order on the tiers. Without it `bps` would be a number with no meaning. -/
theorem a_milder_tier_takes_no_more (s : PosState) (v : Validator) {b₁ b₂ : Nat} (h : b₁ ≤ b₂) :
    (slash s v b₁).coop ≤ (slash s v b₂).coop := by
  simp only [slash]
  exact Nat.add_le_add_left (Nat.div_le_div_right (Nat.mul_le_mul_left _ h)) _

/-- **Nothing is left behind.** The validator is out of every ledger the slash touches, so no stranded
    entry can go on reducing the pot for every other validator — which is exactly the defect C197
    recorded. A list that was filtered by `≠ v` and is then filtered by `= v` is empty, and that is all
    this says. -/
theorem slash_clears_every_ledger (s : PosState) (v : Validator) (bps : Nat) :
    (slash s v bps).pool.filter (fun p => p.1 = v) = []
      ∧ (slash s v bps).committed.filter (fun p => p.1 = v) = []
      ∧ (slash s v bps).claims.filter (fun c => c.who = v) = [] := by
  refine ⟨?_, ?_, ?_⟩ <;> simp only [slash] <;> simp [List.filter_filter]

/-! ## The block's producer is paid for the work (B2)

The transitions above say what a validator earns *for being drawn*; `slash` says what it can lose.
Neither says anything about **producing a block**, which is the work the protocol actually needs: a
drawn validator earns its share of the epoch pot whether it proposes or not, and a validator that
never proposes is, in the port's own words, "a drag instead".

This is the one production signal the protocol can read without new state — a block's **own signed
sender**. Every deploy's burned phlo passes through the staking vault on its way to the epoch pot
(`pre_charge` in, `refund` out), so paying a share of what was burned to the address that signed the
block which burned it is a transfer inside that same vault. It needs no participation score, no new
leaf, and it cannot be steered by anyone but the signer. -/

/-- The share of a deploy's burned phlo paid to the block's producer
    (`PosParams::executor_share`; 2 500 = a quarter, the shipped default). -/
def executorShare : Nat := 2500

/-- **Pay the block's producer** (`native_state.rs`'s `pay_executor`): `burned * executorShare /
    10000` leaves the staking vault and arrives in the producer's own vault. The pool, the active
    set and every ledger are untouched — this moves income, never stake. -/
def payExecutor (s : PosState) (burned : Nat) : PosState :=
  { s with vault := s.vault - burned * executorShare / 10000,
           user := s.user + burned * executorShare / 10000 }

/-- A producer's share is never more than what the deploy burned: `share ≤ 10000` bounds it. -/
theorem producer_share_le_burned (burned : Nat) {share : Nat} (h : share ≤ 10000) :
    burned * share / 10000 ≤ burned := by
  have h1 : burned * share ≤ 10000 * burned := by
    rw [Nat.mul_comm 10000]
    exact Nat.mul_le_mul_left _ h
  exact Nat.div_le_of_le_mul h1

/-- **The producer's payment is a transfer, not a mint** — the same conservation the epoch's own
    transitions and the slash carry. `burned ≤ vault` is exactly the hypothesis under which the
    subtraction does not saturate: the phlo was charged into this vault, so it is there to pay out. -/
theorem payExecutor_conserves (s : PosState) (burned : Nat) (hburned : burned ≤ s.vault) :
    totalRev (payExecutor s burned) = totalRev s := by
  have hshare := producer_share_le_burned burned (show executorShare ≤ 10000 by decide)
  simp only [totalRev, payExecutor]
  omega

/-- **And it moves income, never stake.** The pool, the active set and the committed ledger are the
    ones it was handed — a producer payment that could touch a bond would be a slash in disguise. -/
theorem payExecutor_leaves_the_stake (s : PosState) (burned : Nat) :
    (payExecutor s burned).pool = s.pool
      ∧ (payExecutor s burned).active = s.active
      ∧ (payExecutor s burned).committed = s.committed := by
  refine ⟨?_, ?_, ?_⟩ <;> simp only [payExecutor]

/-- **The payment is monotone in what was burned**, which is the whole of "producing pays": a deploy
    that burns more phlo pays its producer more. Stated because a share is only a *reward for work*
    if it is a function of work and not a flat grant. -/
theorem producer_pay_is_monotone {a b : Nat} (h : a ≤ b) :
    a * executorShare / 10000 ≤ b * executorShare / 10000 :=
  Nat.div_le_div_right (Nat.mul_le_mul_right _ h)

/-! ## The developer fee reserve (#655)

The producer's share is the one per-deploy split the protocol had, and its payee is fixed: the block's
signed sender. **A share of the burn for developer usage rewards is a second payee in the same move**,
and it has to exist at genesis, because the share is consensus state and no deploy can write it: a
share a genesis does not commit can never be enabled without a hard fork. So the genesis commits a
*ceiling*, and how much of it is paid to developers is policy carried out later by ordinary deploys.

`native_state.rs`'s `pay_executor` pays both in one native step (`fee_split` computes the two amounts,
and the merge's cost-accounting composition calls the same function). The reserve is paid to an
address, so in this model it is a move from `vault` to `user`, like the producer's share. -/

/-- **Pay the developer fee reserve**: `burned * share / 10000` leaves the staking vault and arrives in
    the reserve's address. Like `payExecutor` it moves income and never stake. -/
def payReserve (s : PosState) (burned share : Nat) : PosState :=
  { s with vault := s.vault - burned * share / 10000,
           user := s.user + burned * share / 10000 }

/-- **One deploy's fee split**: the producer's share, then the reserve's — `pay_executor`'s order. -/
def payFees (s : PosState) (burned share : Nat) : PosState :=
  payReserve (payExecutor s burned) burned share

/-- **The two shares never take more than was burned** when they sum to at most the whole, which is
    the genesis rule `check_developer_reserve` enforces (`executor-share + developer-share ≤ 10000`).
    Each share is floored separately, so this is the bound on what the vault pays out per deploy. -/
theorem fee_split_le_burned (burned : Nat) {e d : Nat} (h : e + d ≤ 10000) :
    burned * e / 10000 + burned * d / 10000 ≤ burned := by
  have h1 : burned * e + burned * d ≤ burned * 10000 := by
    rw [← Nat.mul_add]
    exact Nat.mul_le_mul_left _ h
  omega

/-- **And the bound is load-bearing**: two shares above the whole (here 9 000 + 2 000) pay out more
    than the deploy burned, which is why the genesis refuses them rather than trusting the split. -/
theorem shares_above_the_whole_overpay :
    ¬ (1000 * 9000 / 10000 + 1000 * 2000 / 10000 ≤ 1000) := by decide

/-- **The reserve is a transfer, not a mint**, given the genesis bound and that the burn was charged
    into this vault. The producer's share and the reserve together conserve `totalRev`. -/
theorem payFees_conserves (s : PosState) (burned share : Nat) (hburned : burned ≤ s.vault)
    (hshare : executorShare + share ≤ 10000) :
    totalRev (payFees s burned share) = totalRev s := by
  have hsplit := fee_split_le_burned burned hshare
  simp only [totalRev, payFees, payReserve, payExecutor]
  omega

/-- **It moves income, never stake**: the pool, the active set and the committed ledger are untouched,
    for the reason `payExecutor_leaves_the_stake` gives. -/
theorem payFees_leaves_the_stake (s : PosState) (burned share : Nat) :
    (payFees s burned share).pool = s.pool
      ∧ (payFees s burned share).active = s.active
      ∧ (payFees s burned share).committed = s.committed := by
  refine ⟨?_, ?_, ?_⟩ <;> simp only [payFees, payReserve, payExecutor]

/-- **A zero share is no reserve**: the split is exactly the producer's share alone, so a genesis that
    reserves nothing runs the rule it ran before the reserve existed. -/
theorem a_zero_reserve_is_the_old_rule (s : PosState) (burned : Nat) :
    payFees s burned 0 = payExecutor s burned := by
  cases s
  simp [payFees, payReserve, payExecutor]

/-- **A decided instance, so the split is not vacuous**: a burn of 1 000 at the shipped producer share
    and a 500-basis-point reserve pays 250 to the producer and 50 to the reserve, and leaves 700. -/
theorem the_split_is_a_split :
    1000 * executorShare / 10000 = 250 ∧ 1000 * 500 / 10000 = 50
      ∧ 1000 - 1000 * executorShare / 10000 - 1000 * 500 / 10000 = 700 := by decide

/-! ## The absence rule (B4)

The epoch pays for *being drawn*; `payExecutor` pays for *producing*. Neither says anything about a
validator that is drawn and produces nothing — which, on this protocol, is free: an absent validator
keeps its bond, counts in the finality denominator, and until this rule existed collected its share of
the pot.

The rule is stated here as what it is: **a function of an epoch's reward and of how long the validator
has been silent**, and of nothing else. That is the whole of "income only, never the bond" — it cannot
reach a stake because a stake is not one of its arguments. -/

/-- **The participation weight**: basis points of a drawn validator's epoch share that it is paid, as a
    function of how far behind the last finalised fringe its latest message sits.

    `grace` is the lag inside which the weight is the whole share, `knee` the lag at which it reaches
    zero, and the ramp between is linear. **Two parameters rather than one, and that is a property rather
    than a taste**: a rule that pays an honest validator *in full* and reaches zero elsewhere cannot be a
    function of a single threshold, because the ramp would have to begin at lag zero — which puts a
    haircut on every live validator, including one that missed nothing. `grace = knee` reproduces a
    binary threshold rule exactly, so this is the general case and the cliff is its instance. -/
def participationWeight (behind grace knee : Nat) : Nat :=
  if behind ≤ grace then 10000
  else if knee ≤ behind then 0
  else 10000 * (knee - behind) / (knee - grace)

/-- **A weight is never more than the whole share.** This is the fact that carries `sum_rewards_le_pot`
    through the rule: every adjusted share is bounded by the share it adjusts, so an epoch under this
    rule still cannot pay out more than its pot. -/
theorem participationWeight_le (behind grace knee : Nat) :
    participationWeight behind grace knee ≤ 10000 := by
  unfold participationWeight
  split
  · exact Nat.le_refl _
  · split
    · exact Nat.zero_le _
    · refine Nat.div_le_of_le_mul ?_
      rw [Nat.mul_comm (knee - grace) 10000]
      exact Nat.mul_le_mul_left 10000 (by omega : knee - behind ≤ knee - grace)

/-- **The weight is full inside the grace.** "An honest, temporarily-offline validator recovers fully",
    as arithmetic: having been away for less than the grace is indistinguishable from never having been
    away, so the rule has no memory to recover from. -/
theorem participationWeight_full {behind grace knee : Nat} (h : behind ≤ grace) :
    participationWeight behind grace knee = 10000 := by
  simp only [participationWeight, h, if_true]

/-- **And it reaches zero at the knee** — the weight is a penalty with a floor, not a curve that
    asymptotes: past the knee a validator is paid nothing for the epoch it sat out.

    **`grace < behind` is not decoration**: it rules out the full-share branch, and without it the
    statement is false — `participationWeight 5 10 5` is `10000`, not `0`, because a knee below the
    grace is a shape the first branch already answers. So the statement is about the *graded* shape,
    where `grace ≤ knee`; the cliff is `grace = knee`, and there the two branches meet with no ramp
    between them. -/
theorem participationWeight_zero {behind grace knee : Nat}
    (hg : grace < behind) (h : knee ≤ behind) :
    participationWeight behind grace knee = 0 := by
  unfold participationWeight
  rw [if_neg (by omega), if_pos h]

/-- **A validator further behind is paid no more.** Stated because a weight is only a *penalty for
    absence* if it is monotone in the absence; a function that paid a quieter validator more would
    satisfy every other theorem here. -/
theorem participationWeight_antitone {b₁ b₂ grace knee : Nat} (h : b₁ ≤ b₂) :
    participationWeight b₂ grace knee ≤ participationWeight b₁ grace knee := by
  by_cases hfull : b₁ ≤ grace
  · -- The nearer one is already paid in full, so the further one can be at most that.
    rw [participationWeight_full hfull]
    exact participationWeight_le b₂ grace knee
  · by_cases hzero : knee ≤ b₂
    · -- The further one is already at the floor, and a weight is never negative.
      rw [participationWeight_zero (by omega) hzero]
      exact Nat.zero_le _
    · -- Both are on the interior of the ramp: same denominator, and `b₁ ≤ b₂` shrinks the numerator.
      have hnf : ¬ b₂ ≤ grace := fun hb => hfull (h.trans hb)
      rw [participationWeight, participationWeight,
        if_neg hnf, if_neg hzero, if_neg hfull,
        if_neg (show ¬ knee ≤ b₁ by omega)]
      exact Nat.div_le_div_right (Nat.mul_le_mul_left 10000 (by omega))

/-- **The absence rule** (`native_state.rs`'s `apply_weight`): a validator's epoch reward is scaled by
    its participation weight. `knee = 0` is the off switch, and off is `Pos.rhox`'s behaviour: in the
    contract absence costs nothing.

    The rule is still **income only, never the bond**, and structurally so — a stake is not one of its
    arguments. The withheld fraction stays in the staking vault (nothing debits it), so the next epoch
    distributes it and conservation is untouched. -/
def absenceAdjusted (behind grace knee reward : Nat) : Nat :=
  if knee = 0 then reward else reward * participationWeight behind grace knee / 10000

/-- **The rule never raises a reward.** It pays a fraction or the whole, and there is no third thing —
    which is what makes it a penalty rather than a second reward axis. It rests entirely on the weight's
    own bound, which is why that bound is stated before this. -/
theorem absence_never_raises (behind grace knee reward : Nat) :
    absenceAdjusted behind grace knee reward ≤ reward := by
  unfold absenceAdjusted
  split
  · exact Nat.le_refl _
  · have h1 : reward * participationWeight behind grace knee ≤ 10000 * reward := by
      rw [Nat.mul_comm 10000]
      exact Nat.mul_le_mul_left _ (participationWeight_le behind grace knee)
    exact Nat.div_le_of_le_mul h1

/-- **A validator inside the grace is paid in full** — "an honest, temporarily-offline validator
    recovers fully", as arithmetic: having been away for less than the grace is indistinguishable from
    never having been away, so the rule has no memory to recover from. Note the hypothesis is over the
    **grace** and not the knee, which is the whole reason the weight has two parameters. -/
theorem a_returning_validator_is_paid_in_full {behind grace knee reward : Nat}
    (h : behind ≤ grace) :
    absenceAdjusted behind grace knee reward = reward := by
  unfold absenceAdjusted
  by_cases hk : knee = 0
  · simp [hk]
  · rw [if_neg hk, participationWeight_full h, Nat.mul_comm reward 10000]
    exact Nat.mul_div_right reward (by decide : 0 < 10000)

/-- **And the rule moves no stake.** Committing a reward leaves the pool and the active set exactly as
    they were — for *any* reward function, which is what makes the statement about the transition
    rather than about the rule. A validator that was not paid is not a validator that was slashed, and
    that is the plan's condition on this rule stated as a theorem. -/
theorem the_absence_rule_moves_no_stake (r : Validator → Nat) (s : PosState) :
    (commitRewards r s).pool = s.pool ∧ (commitRewards r s).active = s.active :=
  ⟨rfl, rfl⟩

/-- **A partial weight withholds.** The theorems above pin only that the rule never *raises* a reward —
    which an implementation that changed nothing at all would satisfy, and which was for a time exactly
    what the tree had: `absence_never_raises` is a `≤`, `a_returning_validator_is_paid_in_full`
    constrains only the grace, and `the_absence_rule_moves_no_stake` does not mention `absenceAdjusted`
    at all.

    This is the other half, and it is the **graded** statement rather than the binary one: any weight
    strictly below the whole, under a rule that is on, and with something to lose, pays less than the
    share. The hypotheses are each load-bearing — drop `knee ≠ 0` and the off switch returns the reward
    untouched, drop `participationWeight < 10000` and the validator is inside the grace, drop
    `0 < reward` and there is nothing left to prove. -/
theorem absence_withholds {behind grace knee reward : Nat}
    (hon : knee ≠ 0)
    (hpartial : participationWeight behind grace knee < 10000)
    (hreward : 0 < reward) :
    absenceAdjusted behind grace knee reward < reward := by
  unfold absenceAdjusted
  rw [if_neg hon, Nat.div_lt_iff_lt_mul (by decide : 0 < 10000)]
  exact Nat.mul_lt_mul_of_pos_left hpartial hreward

/-- **The binary rule's statement, as the strictest instance of the graded one.** Past the knee the
    weight is zero, so the validator is paid *nothing* for the epoch it sat out — which is the rule the
    tree shipped before this was graded, preserved here as an instance rather than dropped, so a reader
    comparing the two finds the old statement inside the new one. -/
theorem absence_withholds_past_the_knee {behind grace knee reward : Nat}
    (hon : knee ≠ 0) (hgk : grace < knee) (hknee : knee ≤ behind) :
    absenceAdjusted behind grace knee reward = 0 := by
  unfold absenceAdjusted
  rw [if_neg hon, participationWeight_zero (by omega) hknee, Nat.mul_zero, Nat.zero_div]

/-- `Σ zipWith f xs ys ≤ Σ map g xs` when `f` is bounded by `g` pointwise and the lists agree in
    length — the step that composes a per-validator scaling with the split. -/
theorem nsum_zipWith_le_map {bonds weights : List Nat} (hlen : weights.length = bonds.length)
    (f : Nat → Nat → Nat) (g : Nat → Nat) (h : ∀ b w, w ∈ weights → f b w ≤ g b) :
    nsum (List.zipWith f bonds weights) ≤ nsum (bonds.map g) := by
  induction bonds generalizing weights with
  | nil => simp [nsum]
  | cons b rest ih =>
    match weights with
    | [] => simp at hlen
    | w :: wl =>
      have hlen' : wl.length = rest.length := by simp at hlen ⊢; omega
      have hr : ∀ b' w', w' ∈ wl → f b' w' ≤ g b' := fun b' w' hw' => h b' w' (by simp [hw'])
      simp only [List.zipWith_cons_cons, List.map_cons, nsum]
      exact Nat.add_le_add (h b w (by simp)) (ih hlen' hr)

/-- **An epoch under the participation rule still cannot pay out more than its pot** — the composition
    `sum_rewards_le_pot` could not state on its own, because the rule's effect is a per-validator
    scaling that happens *after* the split. It is the clause of #150's close condition that reads "the
    invariant must survive whatever replaces the formula": the split is unchanged and still bounded by
    the pot, and the rule only ever multiplies each of its outputs by something at most the whole, so
    the scaled total is bounded by the total.

    The two hypotheses are the ones the weight supplies: `hw` is `participationWeight_le` at every
    drawn validator, and the other two are `sum_rewards_le_pot`'s own. -/
theorem weighted_rewards_le_pot (pot minimumBond activeBonds : Nat) (bonds weights : List Nat)
    (hlen : weights.length = bonds.length)
    (hactive : activeBonds = nsum bonds) (hD : 0 < activeBonds / minimumBond)
    (hw : ∀ w ∈ weights, w ≤ 10000) :
    nsum (List.zipWith (fun b w => reward pot minimumBond activeBonds b * w / 10000) bonds weights)
      ≤ pot := by
  refine le_trans
    (nsum_zipWith_le_map hlen
      (fun b w => reward pot minimumBond activeBonds b * w / 10000)
      (fun b => reward pot minimumBond activeBonds b) ?_) ?_
  · intro b w hmem
    have h1 : reward pot minimumBond activeBonds b * w
        ≤ 10000 * reward pot minimumBond activeBonds b := by
      rw [Nat.mul_comm 10000]
      exact Nat.mul_le_mul_left _ (hw w hmem)
    exact Nat.div_le_of_le_mul h1
  · exact sum_rewards_le_pot pot minimumBond activeBonds bonds hactive hD

/-- **The ramp is a ramp, decided rather than described.** Four points across the weight's four regions —
    inside the grace, two places on the interior, at the knee and past it — so an implementation that
    ignored the ramp, or the grace, or the knee fails on a computation. A fixture whose weights are all
    equal is the degeneracy AUDIT C149 recorded in this file once already.

    **This is not a companion to `absence_withholds`; it is what makes it bite.** That theorem is an
    implication whose hypothesis is `participationWeight behind grace knee < 10000`, so a weight that was
    *always* `10000` satisfies it **vacuously** — and that is measured rather than argued: replacing the
    weight with the constant `10000` leaves the implication's truth untouched and fails *this*
    declaration, with `decide` reporting that the proposition is false. The implication and the instance
    are therefore a pair, and neither alone is a check. It is the same trap as the absence rule's own,
    one level up: there the theorems were satisfied by a rule that withheld nothing, here by a weight
    that never withholds. -/
theorem the_ramp_is_a_ramp :
    participationWeight 5 5 15 = 10000
      ∧ participationWeight 6 5 15 = 9000
      ∧ participationWeight 10 5 15 = 5000
      ∧ participationWeight 15 5 15 = 0
      ∧ participationWeight 20 5 15 = 0 := by decide

/-! ## Delegated stake (law 57)

Every section above assumes the stake a key carries is the operator's own. This one models the case the
protocol could not previously express: a **delegator** — a key that holds REV, signs its own deploys, and
has no intention of running a node — bonding that REV *through* an operator's key and holding a claim on
what it earns.

The port's shape is one substitution and four ledgers. The substitution is `pos:bonds`: a key's entry
becomes the **aggregate**, the operator's own stake plus every delegation to it, which is exactly what
`delegate` below writes and why `atRisk` reads it without adding `s.delegations` a second time. The
ledgers are `pos:delegations` (who owns what, modelled as `PosDelegation`), `pos:pending_delegations` (a
staged exit), `pos:delegation_claims` (the escrow it becomes) and `pos:delegated_rewards` (a delegator's
accrued share of an epoch, which must not sit in the operator's `committed` map where the operator could
be paid it).

**What this section can and cannot say, and why.** `PosState.user` is the *total* over every user vault
rather than a map — the state's own doc gives the reason, and the withdrawal path already carries the
same simplification. So the arithmetic below pins **how much** a delegator is owed and
`delegate_conserves` pins that no coin is minted, but no theorem here can say *which* vault was credited;
`fanOut`'s per-vault attribution is the Rust's, and it is pinned by a Rust witness rather than restated
here. What is *not* deferred is the part a map is not needed for: that the split of one validator's reward
is **exact** (so a validator with delegators commits the same total it would have alone, and law 46 is not
weakened), that it is the **identity** when nobody has delegated (so a chain that never delegates writes
the state it wrote before this law existed), and that a second delegation from the same delegator adds to
the first rather than replacing it. -/

/-- The principal one operator's key carries on behalf of others — the sum the port's `pos:delegations`
    lookup returns for one operator, and the amount that must already be inside its `pos:bonds` entry.
    Written as a recursion rather than a `foldl` so that the two cons cases below are `simp`, which is
    what makes "a second delegation accumulates" a computation instead of an induction. -/
def delegatedTotal : List PosDelegation → Validator → Nat
  | [], _ => 0
  | d :: rest, op => (if d.operator = op then d.amount else 0) + delegatedTotal rest op

/-- **A second delegation from the same delegator adds to the first.** The port inserts into
    `pos:delegations` with `lookup k + amount`, so a delegator's principal is the **sum** of its
    transfers and not its last one; the model keeps two records and this says the totals agree. Without
    it an implementation that *replaced* the entry would satisfy every pro-rata theorem above, because
    those read a list of amounts and cannot see whether two of them should have been one. -/
theorem a_second_delegation_accumulates (ds : List PosDelegation) (op del : Validator) (a b : Nat) :
    delegatedTotal (⟨op, del, b⟩ :: ⟨op, del, a⟩ :: ds) op = delegatedTotal ds op + a + b := by
  simp only [delegatedTotal, if_true]
  omega

/-- **The total is one operator's, and no other's.** Two delegators of the same operator sum; a third
    party's delegation to a different operator is invisible here — the misattribution an aggregate
    computed over the whole ledger instead of one key would produce. -/
theorem the_total_belongs_to_one_operator :
    delegatedTotal [⟨⟨1⟩, ⟨2⟩, 10⟩, ⟨⟨1⟩, ⟨3⟩, 5⟩, ⟨⟨7⟩, ⟨8⟩, 4⟩] ⟨1⟩ = 15
      ∧ delegatedTotal [⟨⟨1⟩, ⟨2⟩, 10⟩, ⟨⟨1⟩, ⟨3⟩, 5⟩, ⟨⟨7⟩, ⟨8⟩, 4⟩] ⟨7⟩ = 4
      ∧ delegatedTotal [⟨⟨1⟩, ⟨2⟩, 10⟩, ⟨⟨1⟩, ⟨3⟩, 5⟩, ⟨⟨7⟩, ⟨8⟩, 4⟩] ⟨9⟩ = 0 := by decide

/-- **The pro-rata split, with the base that is divided by but not paid.** `amount` is divided across
    `weights` in order; `base` joins the **denominator** without being issued a share. Both halves of
    delegated stake are this function: the reward split has `base` the operator's own stake (it is paid,
    but as the remainder rather than by this formula — see `split_sums_to_the_reward`), and the slash's
    refund has `base = 0`, the principals being the whole of what was at risk. One floor per share, so the
    shares sum to at most `amount` and the difference is the dust the operator keeps. -/
def proRata (amount base : Nat) (weights : List Nat) : List Nat :=
  let total := base + nsum weights
  weights.map (fun w => amount * w / total)

/-- **The shares never exceed what is being shared**, so the operator's remainder is a `Nat` subtraction
    rather than a truncation and no implementation can pay out more than it holds. No hypothesis: the
    base is inside the denominator, so `Σ weights ≤ total` holds by construction, and a zero denominator
    makes every share zero. -/
theorem proRata_sum_le (amount base : Nat) (weights : List Nat) :
    nsum (proRata amount base weights) ≤ amount := by
  have hstep : nsum (proRata amount base weights)
      ≤ nsum (weights.map (fun w => amount * w)) / (base + nsum weights) := by
    simpa only [proRata, List.map_map, Function.comp_def] using
      list_sum_div_le (base + nsum weights) (weights.map (fun w => amount * w))
  have hsum : nsum (weights.map (fun w => amount * w)) = amount * nsum weights :=
    nsum_map_mul_left amount weights
  have hle : nsum weights ≤ base + nsum weights := Nat.le_add_left _ _
  have hmul : amount * nsum weights ≤ (base + nsum weights) * amount := by
    rw [Nat.mul_comm (base + nsum weights) amount]
    exact Nat.mul_le_mul_left amount hle
  calc nsum (proRata amount base weights)
      ≤ nsum (weights.map (fun w => amount * w)) / (base + nsum weights) := hstep
    _ = amount * nsum weights / (base + nsum weights) := by rw [hsum]
    _ ≤ amount := Nat.div_le_of_le_mul hmul

/-- **The split is exact.** The delegators' shares plus the operator's remainder are the reward the key
    was paid — so a validator with delegators commits the same total it would have committed alone, and
    law 46 (`sum_rewards_le_pot`) is a statement about the reward *before* this split and therefore
    survives it rather than needing to be restated. -/
theorem split_sums_to_the_reward (reward own : Nat) (amounts : List Nat) :
    nsum (proRata reward own amounts) + (reward - nsum (proRata reward own amounts)) = reward := by
  have h := proRata_sum_le reward own amounts
  omega

/-- **Dormancy, stated in the model.** With no delegators the split issues no share at all and the
    operator's remainder is the whole reward — so a chain where `delegate` has never been called writes
    the same `pos:committed` entry it wrote before this law existed. This is the arithmetic half of the
    port's byte-identical-post-state requirement; the store half is that the four new leaves are left
    **absent** rather than written empty. -/
theorem split_with_no_delegators_is_the_operator (reward own : Nat) :
    proRata reward own [] = [] ∧ reward - nsum (proRata reward own []) = reward := by
  simp [proRata, nsum]

/-- **The shares are proportional, and the dust is real — decided, not described.** The first clause is
    the reward split: a reward of 100 on an operator holding 30 with principals 10 and 61 divides by 101
    and pays 9 and 60, leaving 31 to the operator — the floor bites, the two shares **differ** from each
    other and from zero, and the operator's remainder is neither the whole reward nor nothing. The second
    is the refund, where `base = 0` and the principals are the whole denominator, so 100 over `[30, 70]`
    is exact. The third keeps a non-zero base with a denominator that does not divide.

    **The fixture is the point, and it was measured rather than argued** (AUDIT C149, three times in this
    file now). Three plausible wrong splits were compiled against these clauses: the operator keeps
    everything (`weights.map (fun _ => 0)`), every delegator gets an equal share
    (`weights.map (fun _ => amount / (base + weights.length))`), and the base is dropped from the
    denominator (`weights.map (fun w => amount * w / nsum weights)`). All three are refuted by the first
    and third clauses. **The second clause refutes none of them and is not meant to** — it is the exact
    end of the range, included to show the dust is not always present, and with `base = 0` it cannot tell
    a rule that divides by the base from one that ignores it, which is the base-drop mutant's own
    degenerate case. **The third clause read `[1, 1, 1]` when it was written, and that was nearly vacuous
    for the same reason**: three equal weights pay `100 / 3` each under the real rule *and* under the
    equal-share mutant, so it distinguished nothing that the first clause did not. `[1, 2]` with base 40
    gives `[2, 4]`, which the equal-share mutant reads as `[2, 2]` and the base-drop mutant as `[33, 66]`.
    A fixture clause that no plausible mutant fails is a clause that is not a check. -/
theorem the_split_is_not_the_identity :
    proRata 100 30 [10, 61] = [9, 60]
      ∧ proRata 100 0 [30, 70] = [30, 70]
      ∧ proRata 100 40 [1, 2] = [2, 4] := by decide

/-- **The slash's refund runs the same function, and its fixture is decided too** — which is the half
    of `proRata` the epoch never reaches. #193's close condition asks for "the delegated-slash refund"
    to be *proved and held by property tests*, and the proof is `proRata_sum_le` plus the exactness
    above; what this adds is the instance, in the shape the refund actually takes.

    The refund's base is a **weight that is paid but not issued a share by this function** — the
    offender's own holdings, which receives the remainder — and its weights are the delegators'
    at-risk totals. So the instance needs three different numbers on three different sides: a returned
    amount, an offender's part, and **two unequal** delegator parts. `100` returned against parts
    `40 / 30 / 60` divides by `130` and pays `23` and `46`, leaving `31` with the offender.

    **The fixture bites for the same measured reason the split's does**: the two shares differ from
    each other and from zero, so an equal-share refund reads `[50, 50]`, a refund that ignores the
    offender's base reads `[33, 66]`, and one that pays the offender everything reads `[]`. -/
theorem the_refund_is_split_pro_rata : proRata 100 40 [30, 60] = [23, 46] := by decide

/-- **A delegation** (`delegate`, `rholang/src/native_state.rs`): the principal leaves the delegator's own
    vault and joins the **staking vault**, and the operator's pool entry becomes the **aggregate** — which
    is the whole of what a delegation changes for consensus, since `compute_bonds` reads the active set
    that `select_active` draws from the pool. It activates exactly as `bond` does: the pool follows at
    once and `active` only at the boundary, so a delegation cannot conjure a slot mid-epoch.

    The principal is **not** recorded in `pool` a second time; the ledger records who owns the part that
    is there, which is why `atRisk` reads the pool entry without adding this ledger. -/
def delegate (s : PosState) (op del : Validator) (amount : Nat) : PosState :=
  { s with
    user := s.user - amount
    vault := s.vault + amount
    pool := setKey s.pool op (lookup s.pool op + amount)
    delegations := ⟨op, del, amount⟩ :: s.delegations }

/-- **A staged undelegation** (`undelegate`): the request records a deadline and moves **nothing** — the
    principal stays in the operator's pool entry and in the ledger, still earning and still at risk, until
    the boundary acts on it. Law 47's first stage, seen from a delegator, and the reason undelegating
    cannot be used to escape a slash that is already in flight. -/
def undelegate (s : PosState) (op del : Validator) (n : Nat) : PosState :=
  { s with undelegations := ⟨op, del, withdrawDeadline s n⟩ :: s.undelegations }

/-- A delegation is a transfer, not a mint: the three vaults sum to what they summed to before. The
    hypothesis is the port's refusal — an unfunded delegation fails rather than truncating the debit. -/
theorem delegate_conserves (s : PosState) (op del : Validator) (amount : Nat)
    (h : amount ≤ s.user) : totalRev (delegate s op del amount) = totalRev s := by
  simp only [delegate, totalRev]
  omega

/-- `setKey` at a key puts that key's value there: the lemma `a_delegation_enlarges_the_pool` needs, and
    the reason `setKey` is the port's insert-or-replace rather than a plain cons. -/
theorem lookup_setKey_self (l : List (Validator × Nat)) (v : Validator) (x : Nat) :
    lookup (setKey l v x) v = x := by
  induction l with
  | nil => simp [setKey, lookup]
  | cons hd tl ih =>
    by_cases hhd : hd.1 = v
    · simp [setKey, lookup, hhd]
    · by_cases htl : tl.any (fun p => p.1 = v)
      · have hany : (hd :: tl).any (fun p => p.1 = v) = true := by simp [List.any_cons, hhd, htl]
        have ih' : lookup (tl.map (fun p => if p.1 = v then (v, x) else p)) v = x := by
          simpa [setKey, htl] using ih
        simp [setKey, hany, List.map_cons, lookup, hhd, ih']
      · have hany : (hd :: tl).any (fun p => p.1 = v) = false := by simp [List.any_cons, hhd, htl]
        simp [setKey, hany, lookup]

/-- **The operator's key carries the delegator's principal**: after a delegation its aggregate pool entry
    is larger by exactly the amount delegated. This is the sentence "a delegator's principal sits inside
    the operator's bond" as a computation, and it is what makes the slash's reach — and the draw's weight
    — a consequence of the aggregate rather than a claim about it. -/
theorem a_delegation_enlarges_the_pool (s : PosState) (op del : Validator) (amount : Nat) :
    lookup (delegate s op del amount).pool op = lookup s.pool op + amount := by
  simp only [delegate]
  exact lookup_setKey_self s.pool op (lookup s.pool op + amount)

/-- An undelegation moves no coin: the request is a deadline and nothing else. -/
theorem undelegate_moves_no_coins (s : PosState) (op del : Validator) (n : Nat) :
    totalRev (undelegate s op del n) = totalRev s := by
  simp [undelegate, totalRev]

/-- **A slash clears the delegation ledgers too.** The offender's delegators are out of the ledger, the
    staged requests, the escrow and the accrued rewards together, so no delegator's entry can go on
    reducing the pot — or stay claimable against a key that no longer carries the stake it was delegated
    to. This is `slash_clears_every_ledger` extended to the four leaves law 57 adds; the per-vault
    attribution of the refund it is paid for is the Rust's, per this section's header. -/
theorem a_slash_clears_the_delegations (s : PosState) (v : Validator) (bps : Nat) :
    (slash s v bps).delegations.filter (fun d => d.operator = v) = []
      ∧ (slash s v bps).undelegations.filter (fun u => u.operator = v) = []
      ∧ (slash s v bps).delegationClaims.filter (fun c => c.operator = v) = []
      ∧ (slash s v bps).delegatedRewards.filter (fun d => d.operator = v) = [] := by
  refine ⟨?_, ?_, ?_, ?_⟩ <;> simp only [slash] <;> simp [List.filter_filter]

end Rchain
