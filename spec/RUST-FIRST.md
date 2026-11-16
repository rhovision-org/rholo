# Rust-first native system contracts

This page records the **rust-first** replacement of the Scala-legacy genesis bootstrap: the
registry, Proof-of-Stake state, and REV vault are no longer rholang contracts (`Registry.rho`,
`Pos.rhox`, `RevVault.rho`, …) that re-implement a `TreeHashMap` trie *in interpreted rholang*.
They are now **native Rust state + system processes**. Scala serves as a *checklist* of required
behavior only (per the prime directive in [`AGENTS.md`](../AGENTS.md)); the fragile trie and the
registry-bootstrap echo are gone.

## Why

The blessed `Registry.rho` built a depth-4 keccak-256 nybble trie with 16-bit bitmasks, using peek
receives, persistent sends, list-channels (`@[node, *storeToken]`) and tuple-channels
(`@(map, "depth")`). Any interpreter bug made the registry **silently empty** — genesis "succeeded"
while `compute_bonds` returned zero. Native state makes that failure mode unrepresentable: the
bonds/registry/vault live in typed Rust maps, folded into the same content-addressed radix trie as
the tuple space.

## State model

The block state hash *is* the radix-trie root of the tuple space; there is no second state
component. Native state therefore enters the same trie, under dedicated prefixes:

| Prefix | Byte | Content | Leaf key |
|---|---|---|---|
| `PREFIX_REGISTRY` | `0x03` | registry `uri → Par` | `blake2b256(uri)` |
| `PREFIX_POS` | `0x04` | PoS state (see below) | `blake2b256(b"pos:…")` |
| `PREFIX_VAULT` | `0x05` | vault `address → NonNegI64` | `blake2b256(address)` |
| `PREFIX_TXN` | `0x06` | cross-shard 2PC records (`txn-id → TxnRecord`) | `blake2b256(txn-id)` |
| `PREFIX_HTTP` | `0x07` | HTTP claim registry (`url → (value, block)`) | `blake2b256(b"http:records")` |
| `PREFIX_VAULT_NAME` | `0x08` | vault **handle** (`unforgeable-name → base58 rev-address`) | `blake2b256(name)` |
| `PREFIX_VAULT_AUTH` | `0x09` | vault **authority** (`unforgeable-name → base58 rev-address`) | `blake2b256(name)` |
| `PREFIX_ERTP` | `0x0A` | ERTP issuer ledger: a brand's minting authority, and each purse's or payment's holding | `blake2b256(brand)` / `blake2b256(len(brand) ‖ brand ‖ holder)` |

Both leaves under `PREFIX_ERTP` are **self-describing**, because the totals over the prefix are
derivations by *enumeration* rather than running sums (`spec/`'s C109 idiom), and an enumeration that
cannot tell a brand from a holding cannot sum one brand's value:

| Leaf | Content |
|---|---|
| brand (`0x01`) | the bytes of the name that may mint it |
| holding (`0x02`) | `u32_le(brand.len()) ‖ brand ‖ i64_le(amount) ‖ live` — the brand is in the **value**, not only in the key, so a holding's brand survives the hash; a leaf read under a key whose brand it does not name is refused |

The PoS leaves under `PREFIX_POS`:

| Leaf | Content |
|---|---|
| `pos:bonds` | the full bond **pool** (`Validator → NonNegI64`) |
| `pos:active` | the **active** consensus validator set (top-N of the pool) |
| `pos:trusted` | the trusted validator-**stakeholder** set (admission gate) |
| `pos:withdrawers` | escrowed withdrawals (`Validator → (bond, deadline)`), moved here at the boundary |
| `pos:pending_withdrawers` | withdrawal **requests** (`Validator → epoch-boundary deadline`) |
| `pos:committed` | rewards earned but not yet paid (`Validator → NonNegI64`) |
| `pos:params` | immutable PoS parameters (min/max bond, epoch, quarantine, active cap) |
| `pos:coop` | the Coop slashing-vault balance (confiscated stake) |
| `pos:vault` | the staking-vault balance — escrowed bonds + the phlo that funds the rewards |
| `pos:delegations` | delegated principals (`(operator, delegator) → NonNegI64`) — the attribution of the aggregate, not a second copy of the stake |
| `pos:pending_delegations` | undelegation **requests** (`(operator, delegator) → epoch-boundary deadline`) |
| `pos:delegation_claims` | escrowed undelegated principals (`(operator, delegator) → (amount, deadline)`), moved here at the boundary |
| `pos:delegated_rewards` | a delegator's accrued reward (`(operator, delegator) → NonNegI64`), held apart from the operator's `pos:committed` |

**The four `deleg*` leaves are absent on a chain where nobody has delegated**, and that is a
requirement rather than a detail (see *Dormancy* below): a validator's `pos:bonds` entry is the
**aggregate** of its own stake and its delegations, so with no delegations the aggregate is the stake and
the post-state is byte-identical to a chain without the primitive. `pos:last_spoke` and `pos:epoch_seed`
are the two earlier leaves of the same shape and are documented in `spec/audit/passes.md` §6.

Leaves are the new `PersistedData::NativeLeaf(Vec<u8>)` (the previously-free 2-bit tag `3`). The
trie prefix disambiguates registry vs PoS vs vault, so a single leaf kind suffices.

`PREFIX_HTTP` holds the deploy-supplied HTTP **claim registry** (RCHIP #54) in a single leaf,
`http:records`, mapping `url → (value, captured block)`. `record` is **first writer wins**: the value
is asserted inside a signed deploy, so replay/validation never performs a network fetch and later
deploys compare against the record rather than re-fetching. It is not an oracle — any URL is
squattable by the first writer, and the claimant is not recorded — and the leaf is bounded (256
records / 1 KiB URL / 4 KiB value) and charged its encoded size per operation (C301 B1/B2).

The typed layer is `rholang/src/native_state.rs` (`NativeSystemState`), wrapping the byte-oriented
`rspace/src/native_store.rs` (`InMemNativeStore`):

- `InMemNativeStore` is a write-through overlay on a `NativeHistoryReader`: reads fall through to the
  persisted trie, writes buffer in an overlay + tombstone set, and `drain_changes` emits the
  `NativeStoreAction::{Put,Delete}`s folded into the next checkpoint via
  `HistoryRepository::checkpoint_with_native`.
- `NativeSystemState` exposes typed accessors — `bonds()`/`set_bonds()` (the pool),
  `active()`/`set_active()` (the consensus set), `trusted()`/`set_trusted()`,
  `withdrawers()`/`set_withdrawers()`, `pending_withdrawers()`/`set_pending_withdrawers()`,
  `committed_rewards()`/`set_committed_rewards()`, `params()`/`set_params()`,
  `coop_balance()`/`set_coop_balance()`,
  `pos_vault_balance()`/`set_pos_vault_balance()`, `vault_balance()`/`set_vault_balance()`, and
  `registry_lookup()`/`registry_insert()`, with canonical byte encodings (sorted `BTreeMap`,
  fixed-width `Validator` + little-endian stake).

## The staking vault (`pos:vault`)

Every REV movement in the mechanism is a transfer between a user's vault, the **staking vault** (the
contract's `posVault`), and the Coop multisig vault — so nothing is minted and nothing is burned:

| Step | Effect | Scala |
|---|---|---|
| bond | validator vault → staking vault | `Pos.rhox:397-404` (`deposit!(deployerId, amount, posVaultAddr)`) |
| pre-charge | deployer vault → staking vault | same (`chargeDeploy`) |
| refund | staking vault → deployer vault | `Pos.rhox:417-454` (`refundDeploy`) |
| slash | staking vault → Coop vault | `Pos.rhox:470-482` |
| withdrawal | staking vault → validator vault | `Pos.rhox:556-567` (`payWithdrawer`) |
| **delegation** | delegator vault → staking vault | **none** — the contract has no delegation primitive |
| **undelegation payout** | staking vault → delegator vault | **none**, for the same reason |
| **slash fan-out** | staking vault → the offender's vault, **and each delegator's own vault** | **none** — the contract returns the remainder to the offender alone |
| **pot funding** (`fundPot`) | funder vault → staking vault, **with no claim** — so the whole amount is pot at the next boundary | **none** — the contract has no way to add to the pot except burned phlo |

The last three rows are the primitive's whole REV movement, and each is a deviation registered with the
**"Hard fork (#51 category A)"** marking in `spec/audit/passes.md` §6. A delegation debits the
**delegator's own** vault through the delegator's own `deployerId` — which is what keeps it inside the B2
decision below rather than needing a capability the tree declined to model.

The genesis install funds the vault with **exactly** the initial bond sum (`Pos.rhox:167-174`), so at
genesis the distributable *pot* (`vault − bonded − withdrawers − committed rewards`) is zero. The pot
is the phlo the deploys since the last epoch boundary actually burned: the charge goes in and the
unused surplus comes straight back out. A debit the vault cannot cover is a **platform error** (the
deploy fails) rather than a partial payment — the Scala's `payWithdrawer` ignores its failed transfer
and drops the withdrawer anyway (`// FIXME fix transfer in failure case`), which loses the bond.
`install_genesis` also returns `Result` now: a bond sum that does not fit an `i64` is a genesis that
cannot be installed, not one to clamp.

## Dynamic validators

The validator lifecycle is native and on-chain (`rholang/src/native_state.rs`):

1. **observer** — any key that is not active/bonded. Any node may run as an observer (no validator
   key, or a key that is not in the active set); it syncs and serves but does not propose.
2. **trusted** — admission into the validator *stakeholder group* (`pos:trusted`). Only a trusted key
   may bond. A genesis validator is trusted by construction; a trusted stakeholder admits a new key
   via `rho:rchain:pos!("trust", *deployerId, targetPubKey, *ret)`, and revokes via `"untrust"`.
3. **bonded** — `"bond"` checks trust, `[minimum_bond, maximum_bond]`, and the deployer's REV vault,
   moves the stake into the staking vault, and inserts it into the pool (`pos:bonds`). It does **not**
   activate: the contract's `bond` writes only `allBonds` (`Pos.rhox:355`).
4. **active** — the consensus set (`pos:active`), recomputed **only at an epoch boundary** (see below).
   `number_of_active_validators <= 0` means unlimited, and a cap that does not bite is not a selection:
   the whole eligible pool (positive stake, not withdrawing) is active and the draw below never runs.
   Where the cap **does** bite, membership is a **seeded stake-weighted draw without replacement** from
   that pool (`select_active`, `rholang/src/native_state.rs`) — proportional to stake, *not* a ranking —
   with the seed written one boundary ahead from the last finalised fringe (`spec/RUST-VS-SCALA.md` §3
   item 12). It was a uniform draw until 2026-10-02, which paid per **key** rather than per stake and so
   made splitting a stake profitable; the weight is the fix, and the sequential tail is the residual
   (item 12's O3). A validator leaves the active set at once if it is slashed, which the contract also
   does in place (`Pos.rhox:486-495`).
5. **withdrawing** — `"withdraw"` only **stages** the request (`pos:pending_withdrawers`, the
   contract's `pendingWithdrawers`): the validator stays bonded and keeps validating until the next
   epoch boundary, where it is moved out of the pool (`pos:withdrawers`, holding the bond and a
   deadline) and paid `bond + committed rewards` at the first boundary past its quarantine.
6. **removed** — `slash` (consensus, for bonded offenders) and `untrust` (governance) remove the
   validator and confiscate the stake to the Coop vault (`pos:coop`).

**Alongside the lifecycle, not a stage of it: delegated-to.** A bonded validator's pool entry may carry
one or more delegations — third-party stake attributed to its key. It is not a validator state (the
operator does not change state, and a delegator is not a validator at all), so it is a section of its
own below.

## Delegated stake (`pos:delegations`)

A key that holds REV but does not run a node can stake it on one that does. `rho:rchain:pos!("delegate",
*deployerId, operatorPubKey, amount, *ret)` moves `amount` from the **delegator's own** vault into the
staking vault and adds it to the operator's `pos:bonds` entry, so the operator's key carries the
**aggregate** and the ledger `pos:delegations` records which part is whose. There is no commission and no
admission step: any bonded operator is delegable-to, and the integer-division dust the split leaves goes
to the operator.

- **Activation follows `bond`, not a new path.** A delegation enters the pool at once and the *active*
  set only at the next boundary, because `select_active` draws from the pool. So a delegation cannot
  conjure a slot mid-epoch, and it does reach the draw — which is stake-weighted, so a delegator's REV
  buys weight through the operator's key that it could not buy alone.
- **The epoch split.** At a boundary, after `epoch_rewards` and the absence weight, each drawn
  validator's reward is divided pro-rata across its own stake and its delegations. The delegators' shares
  go to `pos:delegated_rewards` — **not** to the operator's `pos:committed`, where the operator could be
  paid what is not its own — and the operator keeps the remainder. So the operator's committed entry
  plus its delegators' entries equal exactly what it would have committed with no delegators at all,
  which is what keeps `sum_rewards_le_pot` true through the split (`spec/Rchain/Pos.lean`, law 57).
  `epoch_pot` subtracts both ledgers, or the pot would distribute a delegator's accrued reward twice.
- **Undelegation mirrors `withdraw`.** `"undelegate"` **stages** the request
  (`pos:pending_delegations`): the principal stays in the operator's pool entry, still earning and still
  at risk, until the boundary moves it into `pos:delegation_claims` with a deadline, and it is paid
  `principal + accrued rewards` to the delegator's own vault at the first boundary past its quarantine.
  Staging rather than paying at once is the point: undelegating cannot be used to dodge a slash already
  in flight.
- **A slash reaches the delegated principal, and returns it to its owner.** The operator's aggregate pool
  entry is what `atRisk` reads, so the tier applies to delegators' stake exactly as to the operator's;
  the remainder is fanned out pro-rata — the operator's own part to its vault, each delegator's part to
  **its own** vault — with the dust to the operator, and every delegation ledger entry for that key is
  removed. This fan-out is the sharpest correctness risk in the primitive: paying the whole remainder to
  the operator would hand a delegator's principal to the party it was delegated to.
- **Two refusals keep the accounting single-valued.** `withdraw` refuses while delegations are
  outstanding, and `delegate` refuses while a withdrawal is pending, so a validator is never
  simultaneously withdrawing and delegated-to. A delegation is also refused when the operator is not in
  the pool, when the delegator names itself, and below `minimum_bond` — the per-delegation floor, which
  is the DoS control on an unbounded ledger.
- **A delegator can read its own position** — `rho:rchain:pos!("getDelegations", delegatorKey, *ret)`
  replies one entry per operator that key has staked with, and `GET /api/v1/pos/delegations?delegator=…`
  renders the same thing for a client that only reaches HTTP — on the **admin** listener, with the rest
  of the operator surface (W-N3-03), because it is a store query keyed on a caller-supplied key rather
  than chain state any peer would read. **Scoped to the key
  asked about**, because the ledger is unbounded in *delegators* and the bounded direction is the one
  keyed by the delegator; the operator-scoped listing is the unbounded direction and is not offered.
  The three numbers — principal, accrued reward, staged deadline — come from three leaves, and a read
  path never writes: an absent leaf reads as an empty map, and a `set_*` of an empty map here would put
  a trie leaf under a chain that has never delegated (see *Dormancy*).

### Dormancy, and where the fork point is

No chain that never calls `delegate` changes state. The four leaves are **not written at genesis** and no
setter is called until its value is non-empty — `set_*` is an unconditional `put`
(`rholang/src/native_state.rs`), so an empty write is a trie leaf and a **different root** from an absent
one. `pos:epoch_seed` is the precedent: genesis leaves it absent and the first boundary writes it. With
the aggregate equal to the stake when nobody has delegated, the post-state of a no-delegation chain is
byte-identical to one produced without the primitive — so the fork point is the **first `delegate`
deploy**, and an unupgraded node diverges there rather than at genesis.

## The epoch (`close_block`)

`close_block` is the epoch transition and it does **nothing at all** off a boundary
(`Pos.rhox:517-519`; with the permissive default parameters every block is one). At a boundary it runs
one sequence, in this order, because the order carries the meaning (`Pos.rhox:528-551`):

1. **reward** — every pooled validator's share of the pot, computed from the state *as it stands*, is
   added to the committed map (`pos:committed`, the contract's `committedRewards`). A validator
   outside the active set gets an entry of zero, so the map has a key for every pooled validator.
2. **move** — each staged withdrawal becomes a claim: `withdrawers[pk] = (pool[pk], deadline)`, and the
   validator leaves the pool. Because step 1 ran first, the epoch it spent its last blocks in still
   paid it.
3. **pay** — every claim whose deadline has passed is paid `bond + committed[pk]` out of the staking
   vault, and both entries are removed.
4. **re-select** the active set — the only place a bonded validator becomes active, and the only place
   one below the active cap can be promoted into it.

The pot is `vault − bonded − withdrawers − committed`, and one validator's share is
`pot * (bond / minimum_bond) / (active_bonds / minimum_bond)` — **two** integer divisions, so the
shares do not sum to the pot. The remainder is not lost: it stays in the pot and the next epoch
distributes it. `spec/Rchain/Pos.lean` states the inequality (`sum_rewards_le_pot`) and decides an
instance where it is strict (`the_dust_is_real`: minimum bond 3, bonds [4, 5], pot 10 — six units
distributed of ten), and `an_epoch_splits_the_pot_and_keeps_the_dust` builds exactly that state and
reads the split back, so the implementation is checked against the arithmetic rather than against a
remembered number. Where the contract's formula is undefined (a zero minimum bond, or a normaliser of
zero) the port pays zero rather than faulting; §6 of `spec/AUDIT.md` records that and the other
epoch-boundary deviations.

`compute_bonds` (used by finality, the supermajority check, and `Validate::bonds_cache`) reads
`pos:active`; `getBonds` returns the pool and `getActiveValidators` the active set.

## Replay determinism

Native mutations are folded into the radix root, so replay reproduces them **only** if they are pure
functions of `(deploy, random_state)` — never wall-clock time or OS entropy. The system-deploy
operations (`pre_charge`/`refund`/`close_block`/`slash`) and the genesis install
(`compute_genesis(…, pos_genesis)`) obey this; `replay_compute_state` re-installs the genesis PoS
state (pool, trusted set, params, derived active set, the Coop vault and the staking vault) and the
genesis vaults on the genesis replay
(`with_cost_accounting == false`) so the replayed root matches the play root. This is
asserted by `casper/tests/consensus.rs::empty_state_hash_fixed_matches_runtime` and
`genesis_deploy_replay_recomputes_state`.

## System processes

The native `rho:*` protocol is installed as ordinary system-process `Definition`s:

- `rho:registry:lookup` / `insertArbitrary` / `insertSigned:secp256k1` — backed by the native
  registry map.
- `rho:rchain:pos` — `getBonds` (pool) → `RhoMap`, `getActiveValidators` (active set) → `RhoSet`,
  `bond` / `withdraw` (validator lifecycle), `delegate` / `undelegate` (delegated stake — no new channel
  and no `SystemDeployData` variant: a new **method** on this channel is reached by an ordinary deploy
  and replays from that deploy's own COMM trace), and `trust` / `untrust` (stakeholder admission) via a
  `remainder` install pattern.
- `rho:rchain:revVault` / `multiSigRevVault` — `getBalance` / `deposit` / `transfer` / `findOrCreate`
  over the vault balance map.
- `rho:block:data` — the current block's number, sender and informational timestamp.
- `rho:io:http` — the deploy-supplied HTTP **claim registry** (RCHIP #54): `record` (first writer
  wins; no fetch), `get`, `check`, `height`, over the bounded and charged `http:records` leaf.

The `bond` (trust + min/max + vault-funds + `(validator, stake)` into the pool and the staking vault),
`withdraw` (staged request, quarantined payout), `trust`/`untrust` (stakeholder
admission/revocation), `slash` (confiscation to the Coop vault) and vault `findOrCreate` methods are
implemented natively, returning the `(Bool, Either)` result the PoS/vault contracts expect.

**Superseded in part (2026-09-27): the capability *is* modelled now — `findOrCreate` returns a minted
handle and a handle spends.** The same B2 reasoning still keeps the classic shape (a balance map keyed
by REV address, `transfer` from the caller's own `deployerId`), which is why nothing below is deleted:
it is the record of why the classic half is what it is, and of what the delegation half cost. The
missing pieces it names — "thread the deploy's random seed and persist `address → unforgeable`" — are
`ContractCall::unapply`'s carried RNG and `PREFIX_VAULT_NAME`.

**Decided (2026-09-23, Programme B item B2): the vault stays a balance map keyed by REV address — the
unforgeable-name capability is *not* modelled, and that is a decision rather than a gap.** The oracle
mints a **purse** capability per vault (`RevVault.rho:103-140`'s `findOrCreate` → `_makeVault`, whose
`MakeMint` purse is what `transfer`/`deposit`/`getBalance` are called *on*, with an auth key derived
from the address's unforgeable), so a purse can be *handed to a contract* that then spends from it. This
port keys balances by address and takes the caller's own `deployerId`:

- **The half that is present, in a different encoding.** A transfer's `from` account is derived from the
  caller's `rho:rchain:deployerId` (`system_processes.rs`'s `transfer` arm — "capability, not data"),
  which the deploy's signature makes unforgeable. So "only the holder of a vault can spend it" holds;
  what is unforgeable is the *deploy's* identity rather than a minted name. `getBalance` takes a string
  address, which the Scala's does not, and costs nothing: a balance is public chain state, and the
  address is derivable from a public key anyway.
- **The half that is lost: delegation.** A contract cannot be handed a vault to spend from; only the key
  that signs a deploy can spend that key's REV. Nothing in this tree needed it (the wallet, the faucet,
  the gateway's txn legs and the genesis ceremony all act as the key itself), which is why the decision
  is to keep the simplification rather than pay for it.
  **Updated 2026-10-02 (#193): the *delegation* this paragraph names is not the delegation that landed,
  and the distinction is why the decision stands.** What is lost here is a contract's ability to spend a
  vault it was *handed* — a third party spending a named account. What landed is a staker spending from
  **its own** vault through **its own** `deployerId` and having the protocol attribute the stake to
  another key (`pos:delegations`, the *Delegated stake* section above). No capability is handed over and
  no minted name is needed, so the "what landing it would cost" item below is still unspent. The
  alternative shape — bonding from a named vault — is the one that would have needed it, and it is the
  direction issue #193 rejected for exactly this reason.
- **What landing it would cost**, for whoever revisits this: the minted unforgeable must be
  *deterministic and replayable* — in the contract it is a `new` name, i.e. drawn from the deploy's
  RNG — so the native call would have to thread the deploy's random seed and persist
  `address → unforgeable` alongside `unforgeable → balance`, and `transfer`/`getBalance` would change
  shape for every client. The reply-shape deviation was recorded where a client would meet it:
  `spec/API-SCHEMA.md`'s `rho:rchain:revVault` row — **❌ open when this paragraph was written, and ✅
  since 2026-09-27**, because the capability landed: `findOrCreate` returns a minted handle and a handle
  spends, while the classic shapes are unchanged so no client moves. Law 39's doc tie keeps that row
  honest.

**Also deferred, and previously unregistered:** the **`revvaultexport`** offline tooling
(`legacy/node/src/main/scala/coop/rchain/node/revvaultexport/`, seven files — the rho-trie traverser, the
balance getter, and the mainnet1 balance/reporting mains). No Rust module and no CLI subcommand exist for
it; `docs/src/contributor/architecture.md` listed it as a done feature until that claim was checked and
corrected. A deferral registered here is a decision; an unregistered one was an omission.

**Decided (2026-09-23, Programme B item B4): it stays unported, and the shape a port would take is
recorded rather than guessed at.** The Scala tool is 322 lines plus two subdirectories, and it does two
things the contract made hard: `RhoTrieTraverser` walks the interpreted `TreeHashMap` that the old
`RevVault.rho` kept its addresses in, and `VaultBalanceGetter` then *calls* each vault's `balance`
through a rholang runtime (with a randomised return name and a phlo budget). **This port's native state
subsumes the first half outright** — a vault is a `PREFIX_VAULT` leaf, `address → NonNegI64`
(`native_state.rs`'s `vault_balance`) — so the port's tool would be a *read*, not an evaluation. What it
would need that does not exist yet: (a) an offline way to open a data directory's rspace store (the
recovery path does this for cross-shard transactions, so there is precedent) and enumerate the leaves
under a prefix — `traverse_history` walks a *root-and-path*, and enumerating every leaf of a prefix is a
different traversal from the exporter's; and (b) for the *reporting* half
(`mainnet1/reporting`), the transaction store and a DAG traversal, which is a larger input surface
again. Its "specification" is a doc plus a CLI surface, not a law: there is no invariant here to state
in the register, and the alternative — forcing a `rnode revvaultexport` row into `spec/INVENTORY.md` —
would make the catalogue's count mean something other than what it means. Registered as a decision with
that shape, so the next reader knows both what it is and what it is not.

`default_blessed_terms` installs only the interpreted contracts a consumer actually reaches through
`rho:registry:lookup` — `ListOps`, `NonNegativeNumber` and `MakeMint` — plus the registry aliases that
make those lookups (and the native `rho:rchain:revVault` / `rho:rchain:pos` channels) resolve. The
vault and PoS **system** contracts are not installed as sources: their state is native
(`install_genesis`), `compute_bonds`/`get_active_validators` read the `pos:active` leaf directly (no
rholang exploratory deploy), and the pre-charge/refund/close-block/slash system deploys carry a
`NativeSystemDeployOp` rather than routing through `rho:registry:lookup` + `Pos.rhox`. Installing the
interpreted equivalents would shadow consensus-critical logic with slower, less auditable copies.
`spec/GENESIS.md` is the manifest (what is installed, why, and what is deliberately excluded).

## Cross-links

- [`spec/AUDIT.md`](AUDIT.md) §9 — the rust-first fragility audit that motivated this.
- [`spec/RHO-CALCULUS.md`](RHO-CALCULUS.md) — the ρ-calculus core this realizes.
- [`spec/TYPE-SYSTEM.md`](TYPE-SYSTEM.md) — the no-silent-partiality discipline.
