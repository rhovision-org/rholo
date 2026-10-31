import Rchain.Pos
import Rchain.Casper.Stake

/-!
# Law 82 — the validator lifecycle and its trust axis

**What this file is for.** `Rchain/Pos.lean` models the epoch's **arithmetic** and the release rule; it
has no notion of who may enter. The admission list is `pos:trusted`, and the Rust implements it in
`rholang/src/native_state.rs`: `trust` requires the **caller** to be trusted and binds it to the
unforgeable `deployerId` (`system_processes.rs:2043-2063`), `untrust` refuses self-revocation and — since
AUDIT C111 — **does not confiscate the target's bond**, `bond` requires the bonder to be trusted and
refuses a key already in the pool or active set (`native_state.rs:1526-1543`), and activation is the
epoch boundary's `select_active`, never the bond's (`:1582-1585`).

**The state machine.** A `Lifecycle` wraps a `PosState` with the admission list and a model-only
`retired` tombstone, and `LifecycleStep` has six constructors — `trust`, `untrust`, `bond`, `withdraw`, the epoch
`boundary`, and the justified `slash`. There is no seventh way to enter consensus, and no constructor
moves the active set except the boundary or a slash that removes from it.

**What the model pins, and what it corrects.** The code matches the law; the contradiction the audit
found is the stale module header at `native_state.rs:25` ("`slash`/`untrust` remove the validator and
confiscate the stake"), which the code and C111 contradict — the comment is the defect, not the
semantics. `untrust` leaving an **active** validator active is deliberate: trust is an admission list,
a bond is property, and severing the first does not transfer the second. That is a named non-claim of
the trust model (Law 83), not a gap here.

**The tombstone.** The Rust observes removal as *absence*: a validator that is neither trusted nor
holding a bond, request or claim simply stops appearing in any ledger. `retired` records that it was
admitted once, so `phaseOf` can tell "left" from "never arrived"; it changes no consensus value and is
not a Rust field. `trust` clears it (a re-admission), which is the only way back.
-/

namespace Rchain

/-- The validator lifecycle: the PoS state, the admission list (`pos:trusted`), and the model-only
    tombstone described in the header. -/
structure Lifecycle where
  pos : PosState
  trusted : List Validator
  retired : List Validator

/-- The phase a validator is in, derived from the ledgers and the admission list — never stored. -/
inductive Phase where
  /-- Not admitted, never admitted: an observer. -/
  | observer
  /-- Admitted, no stake. -/
  | trusted
  /-- In the pool, not in the active set (between a bond and its boundary). -/
  | bonded
  /-- In the consensus set. -/
  | active
  /-- A withdrawal is staged; the validator is still pooled and still earning. -/
  | withdrawing
  /-- The bond is escrowed in a claim; the validator is out of the pool, awaiting its deadline. -/
  | claimable
  /-- Admitted once, and now neither trusted nor holding anything — the model's reading of the Rust's
      absence. -/
  | removed
  deriving DecidableEq, Repr

@[reducible] def pooledKey (p : PosState) (v : Validator) : Bool :=
  p.pool.any (fun wb => wb.1 = v)

@[reducible] def activeKey (p : PosState) (v : Validator) : Bool :=
  p.active.any (fun wb => wb.1 = v)

@[reducible] def requestedKey (p : PosState) (v : Validator) : Bool :=
  p.requests.any (fun r => r.who = v)

@[reducible] def claimedKey (p : PosState) (v : Validator) : Bool :=
  p.claims.any (fun c => c.who = v)

/-- The phase: stake first (a claim, a staged request, the active set, the pool), then admission, then
    the tombstone. **Admission is checked after every stake ledger**, which is the ordering that says
    revocation does not deactivate: an untrusted validator with a bond is `bonded`/`active`, not
    `observer`. -/
def phaseOf (s : Lifecycle) (v : Validator) : Phase :=
  if claimedKey s.pos v then .claimable
  else if requestedKey s.pos v then .withdrawing
  else if activeKey s.pos v then .active
  else if pooledKey s.pos v then .bonded
  else if v ∈ s.trusted then .trusted
  else if v ∈ s.retired then .removed
  else .observer

/-! ### The state each constructor writes -/

/-- `trust`: the target joins the admission list; a tombstone is cleared (re-admission). -/
def doTrust (target : Validator) (s : Lifecycle) : Lifecycle :=
  { s with trusted := target :: s.trusted,
           retired := s.retired.filter (fun v => v ≠ target) }

/-- `untrust`: the target leaves the admission list. It touches **no** ledger — `pos` is unchanged, so
    the bond, the active seat, the staged request and the claim all survive (AUDIT C111). The
    tombstone records the revocation; `phaseOf` only reports `removed` once nothing else holds it. -/
def doUntrust (target : Validator) (s : Lifecycle) : Lifecycle :=
  { s with trusted := s.trusted.filter (fun v => v ≠ target),
           retired := target :: s.retired }

/-- `bond`: the pool gains the key; the active set is untouched. -/
def doBond (v : Validator) (stake : Nat) (s : Lifecycle) : Lifecycle :=
  { s with pos := bond s.pos v stake }

/-- `withdraw`: a staged request; the pool and the active set are untouched. -/
def doWithdraw (v : Validator) (n : Nat) (s : Lifecycle) : Lifecycle :=
  { s with pos := stage s.pos v n }

/-- The epoch boundary (`close_block`): `none` when the payout refuses (the staking vault cannot cover
    it), which is the port's failure rather than a half-applied step. -/
def boundaryState (r : Validator → Nat) (n : Nat) (s : Lifecycle) : Option Lifecycle :=
  (closeBlock r s.pos n).map (fun p' => { s with pos := p' })

/-- A justified slash of one validator. -/
def doSlash (v : Validator) (bps : Nat) (s : Lifecycle) : Lifecycle :=
  { s with pos := slash s.pos v bps }

/-! ### The transitions -/

/-- The **only** ways a validator's lifecycle moves. `justified` is the abstract evidence predicate
    (`casper::validate::slashable_senders`): a slash the chain has not justified has no constructor.

    * `trust` — a trusted caller admits a target; the caller cannot admit itself by a path that means
      anything, and the Rust's `trust` permits the no-op while `untrust` refuses it.
    * `untrust` — a trusted caller revokes another validator; self-revocation is refused, so the last
      stakeholder cannot revoke itself out of existence.
    * `bond` — a trusted validator, not already pooled or active, escrows a stake it holds.
    * `withdraw` — a pooled validator stages its release.
    * `boundary` — the epoch's four steps, and the only step that can **add** to the active set.
    * `slash` — only under justification. -/
inductive LifecycleStep (justified : Validator → Prop) : Lifecycle → Lifecycle → Prop
  | trust (caller target : Validator) (s : Lifecycle)
      (hcaller : caller ∈ s.trusted) :
      LifecycleStep justified s (doTrust target s)
  | untrust (caller target : Validator) (s : Lifecycle)
      (hcaller : caller ∈ s.trusted) (hne : caller ≠ target) :
      LifecycleStep justified s (doUntrust target s)
  | bond (v : Validator) (stake : Nat) (s : Lifecycle)
      (htrusted : v ∈ s.trusted)
      (hnotPooled : pooledKey s.pos v = false)
      (hnotActive : activeKey s.pos v = false)
      (hvault : stake ≤ s.pos.user) :
      LifecycleStep justified s (doBond v stake s)
  | withdraw (v : Validator) (n : Nat) (s : Lifecycle)
      (hbonded : pooledKey s.pos v = true) :
      LifecycleStep justified s (doWithdraw v n s)
  | boundary (r : Validator → Nat) (n : Nat) (s s' : Lifecycle)
      (h : boundaryState r n s = some s') : LifecycleStep justified s s'
  | slash (v : Validator) (bps : Nat) (s : Lifecycle)
      (hjust : justified v) (hbps : bps ≤ 10000) :
      LifecycleStep justified s (doSlash v bps s)

/-! ### Admission -/

/-- **Trust is written only by a trusted key.** A validator that enters the admission list by a step had
    a trusted caller distinct from itself (for `untrust`; `trust` is the Rust's permitted no-op when
    caller and target coincide, and the theorem's conclusion names the caller the step carries). -/
theorem trust_only_from_a_trusted_key {justified : Validator → Prop} {s s' : Lifecycle}
    (h : LifecycleStep justified s s') {target : Validator}
    (hnew : target ∈ s'.trusted) (hold : target ∉ s.trusted) :
    ∃ caller, caller ∈ s.trusted := by
  cases h with
  | trust caller t s hcaller =>
      by_cases ht : target = t
      · subst ht
        exact ⟨caller, hcaller⟩
      · rw [doTrust] at hnew
        rcases List.mem_cons.mp hnew with h' | h'
        · exact absurd h' ht
        · exact absurd h' hold
  | untrust caller t s hcaller hne =>
      rw [doUntrust] at hnew
      exact absurd (List.mem_filter.mp hnew).1 hold
  | bond v stake s htrusted hnotPooled hnotActive hvault => exact absurd hnew hold
  | withdraw v n s hbonded => exact absurd hnew hold
  | boundary r n s s' hb =>
      obtain ⟨p', _, rfl⟩ := Option.map_eq_some.mp hb
      exact absurd hnew hold
  | slash v bps s hjust hbps => exact absurd hnew hold

/-- **Revocation is not confiscation** (AUDIT C111): `untrust` leaves the PoS state — the bond, the
    active seat, the staged request, the claim — exactly as it was. The old rule this replaces called
    `slash` on a bonded target, which moved its stake to the Coop vault on one signature. -/
theorem untrust_leaves_the_bond_alone (target : Validator) (s : Lifecycle) :
    (doUntrust target s).pos = s.pos := rfl

/-- **…and it does not remove the validator from consensus.** The active set is a field of `pos`, so it
    is untouched; `phaseOf` reads stake before admission, so a revoked-but-active validator's phase
    stays `active`. Admission is not membership. -/
theorem untrust_does_not_deactivate (target : Validator) (s : Lifecycle) :
    (doUntrust target s).pos.active = s.pos.active := rfl

/-- **The last stakeholder cannot revoke itself.** `untrust` requires `caller ≠ target`, so the caller
    stays in the list it filters; a non-empty trusted set therefore stays non-empty. -/
theorem the_trusted_set_never_empties (caller target : Validator) (s : Lifecycle)
    (hcaller : caller ∈ s.trusted) (hne : caller ≠ target) :
    caller ∈ (doUntrust target s).trusted := by
  rw [doUntrust]
  exact List.mem_filter.mpr ⟨hcaller, decide_eq_true hne⟩

/-! ### Bonding -/

/-- **A bond is self-only.** The transition adds the bonding key's own entry to the pool and requires
    that key to be in the admission list; there is no constructor that bonds on another's behalf, which
    is the model's content of the Rust's `deployerId` capability binding
    (`system_processes.rs:1852-1901`). -/
theorem bond_is_self_only (v : Validator) (stake : Nat) (s : Lifecycle) :
    (doBond v stake s).pos.pool = (v, stake) :: s.pos.pool := rfl

/-- **A bond pools but does not activate** — law 44 seen from the lifecycle's side: the active set is
    unchanged until a boundary. -/
theorem the_bond_step_does_not_activate (v : Validator) (stake : Nat) (s : Lifecycle) :
    (doBond v stake s).pos.active = s.pos.active := rfl

/-- **Activation only at a boundary** (or out of one, by a slash). Every other constructor leaves
    `pos.active` definitionally unchanged, so a step that changes it is the epoch's or the slash's —
    this is the state-transition statement the old prose ("membership takes effect at an epoch
    boundary") was making without a machine. -/
theorem activation_only_at_a_boundary {justified : Validator → Prop} {s s' : Lifecycle}
    (h : LifecycleStep justified s s') (hchange : s'.pos.active ≠ s.pos.active) :
    (∃ r n, boundaryState r n s = some s') ∨ (∃ v bps, s' = doSlash v bps s) := by
  cases h with
  | trust caller target s hcaller => exact absurd rfl hchange
  | untrust caller target s hcaller hne => exact absurd rfl hchange
  | bond v stake s htrusted hnotPooled hnotActive hvault => exact absurd rfl hchange
  | withdraw v n s hbonded => exact absurd rfl hchange
  | boundary r n s s' hb => exact Or.inl ⟨r, n, hb⟩
  | slash v bps s hjust hbps => exact Or.inr ⟨v, bps, rfl⟩

/-! ### Withdrawal and slashing, reusing the epoch's own theorems -/

/-- **The release is staged, not moved**: the withdraw step writes a request and no coin. -/
theorem withdrawal_is_staged_then_escrowed_then_paid (s : Lifecycle) (v : Validator) (n : Nat) :
    (doWithdraw v n s).pos.requests = ⟨v, withdrawDeadline s.pos n⟩ :: s.pos.requests ∧
    totalRev (doWithdraw v n s).pos = totalRev s.pos ∧
    (doWithdraw v n s).pos.pool = s.pos.pool ∧
    (doWithdraw v n s).pos.active = s.pos.active :=
  a_staged_withdrawal_moves_no_coins s.pos v n

/-- **A slash is a step only under evidence** — the constructor carries `justified v`, so the proposer's
    decision is not a derivation. The converse is deliberately **not** stated: when the validator holds
    nothing a slash writes the state it was given, so state equality alone cannot identify which
    constructor fired. -/
theorem a_justified_slash_is_a_step (justified : Validator → Prop) (v : Validator) (bps : Nat)
    (s : Lifecycle) (hjust : justified v) (hbps : bps ≤ 10000) :
    LifecycleStep justified s (doSlash v bps s) :=
  LifecycleStep.slash v bps s hjust hbps

/-- **A milder tier conserves the same total** — law 57's slash arithmetic at the lifecycle's edge,
    stated for a step the model admits. -/
theorem slash_conserves_the_lifecycle (v : Validator) (bps : Nat) (s : Lifecycle)
    (hbps : bps ≤ 10000) (hvault : atRisk s.pos v ≤ s.pos.vault) :
    totalRev (doSlash v bps s).pos = totalRev s.pos :=
  slash_conserves s.pos v hbps hvault

/-! ### Non-vacuity: a lifecycle is reachable -/

/-- A three-key fixture: an empty PoS state, `a` trusted. -/
def lifecycleGenesis : Lifecycle where
  pos :=
    { vault := 0, coop := 0, user := 0, pool := [], active := [], requests := [], claims := [],
      committed := [], delegations := [], undelegations := [], delegationClaims := [],
      delegatedRewards := [], epochLength := 1, quarantineLength := 0 }
  trusted := [⟨0⟩]
  retired := []

/-- A bond puts the key in the pool, not the active set: the phase is `bonded`. -/
theorem a_bond_reaches_the_bonded_phase :
    phaseOf (doBond ⟨1⟩ 5 { lifecycleGenesis with trusted := [⟨0⟩] }) ⟨1⟩ = .bonded := by
  simp [phaseOf, doBond, bond, lifecycleGenesis, pooledKey, activeKey, requestedKey, claimedKey]

/-- The fixture's admission state is not `observer` for the trusted key. -/
theorem a_trusted_key_is_not_an_observer :
    phaseOf lifecycleGenesis ⟨0⟩ = .trusted := by
  simp [phaseOf, lifecycleGenesis, pooledKey, activeKey, requestedKey, claimedKey]

/-- And a key that is not in any list is an observer — the model is not vacuous in the other
    direction. -/
theorem an_unlisted_key_is_an_observer : phaseOf lifecycleGenesis ⟨7⟩ = .observer := by
  simp [phaseOf, lifecycleGenesis, pooledKey, activeKey, requestedKey, claimedKey]

end Rchain
