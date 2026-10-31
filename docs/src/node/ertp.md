# ERTP: brands, issuers, purses and payments

**Technical reference** for the native ledger. The how-to, with worked examples, is
[Tokens in Rholang: ERTP](../developer/ertp.md).

**ERTP** is the token standard Agoric's stack uses. A token is not a balance in a table — it is a set
of capabilities: a **brand** that identifies the asset, a **mint** that creates it, an **issuer** that
answers how much a holder contains, a **purse** that holds it, and a **payment** for moving it. An
**amount** is a `(brand, value)` pair.

RChain already had a mint (`MakeMint.rho`) whose brand is a private name and whose purses hold number
cells. What it had no notion of is an **issuer** — the balance lived in the purse, not in a ledger the
issuer owns — and so no `amount`, no `amountMath`, no payment distinct from a purse, and only one
token: REV, whose vault is native.

This page describes how ERTP is built into the node. To *use* it from Rholang, see
[Tokens in Rholang: ERTP](../developer/ertp.md).

## Two halves: a native ledger and a Rholang API

Balances are **consensus state, held natively** under `PREFIX_ERTP = 0x0A`, and the contracts are the
API over it. That follows the same split as the registry, PoS and the REV vault: *"a payment is
consumed exactly once"* is an invariant of consensus, not of an interpreter's evaluation order, so the
spend rule belongs in the ledger.

- **`rho:rchain:ertp:ledger`** — the native ledger. Operations: `makeKit`, `makePurse`, `balance`,
  `mint`, `withdraw`, `deposit`, and the four REV operations below (`revBrand`, `revFund`, `revRedeem`,
  `revWithdraw`).
- **`rho:rchain:ertp`** — the object API, installed at genesis from
  `casper/src/genesis/resources/ERTP.rho`. It turns ledger operations into objects with arms.

The two names are deliberately different URIs so they cannot collide: the shorthand a consumer
hardcodes holds the object API, and the longer urn stays bound to the native channel.

## The ledger

The ledger holds two kinds of leaf, told apart by a tag byte in the value:

- a **brand** leaf: `blake2b256(brand) → the bytes of the name that may mint it`;
- a **holding** leaf: `blake2b256(len(brand) ‖ brand ‖ holder)` →
  `tag ‖ u32_le(len(brand)) ‖ brand ‖ i64_le(amount) ‖ live` — the value carries its own brand length,
  because a brand is variable-width and a total over the prefix has to know where it ends.

The brand is in the value as well as the key, which is not redundant — the key is a hash, so a REV
total over the prefix could not be taken without it, and that total is what the supply-conservation
check reads. A leaf read under a key whose brand it does not name is refused rather than read as data.

Identities — the unforgeable names a purse and a payment are keyed by — are **minted natively**, as the
vault does for its per-call handles. A Rholang contract cannot convert its own `new` name to bytes,
and no operation may be added that does: possessing the bytes *is* possessing the capability. The
ledger keys by the hash of the minted name, so the key leaks nothing.

## The authority model

- **A brand belongs to the name that registered it.** Registration is idempotent for the same
  authority and refused for a different one. A brand whose minter can be replaced is a brand whose
  scarcity is not a fact.
- **Only that name may mint or issue.** `mint` and `withdraw` check it.
- **`deposit` checks nothing.** A purse's own name *is* the capability to deposit into it.
- **A holding of one brand is not a holding of another.** An unknown payment is refused, not treated
  as zero — the two are different facts, and flattening them is how a foreign payment would look
  acceptable.

The deposit protocol is the part that does the work. A purse asks a payment for the payment's ledger
token, then calls the ledger with **its own brand and its own token**. It never trusts the payment's
claim: a payment of another brand names a token the ledger does not know under this brand, so the
refusal is structural rather than a check — and because the ledger refuses before consuming anything,
the refused payment stays live.

## REV is an ERTP brand

REV's supply is fixed at genesis and it lives in vaults (`PREFIX_VAULT`), so the ERTP layer presents
REV as a brand whose **issuer has no mint arm**. `getRevIssuer` replies a `(brand, issuer)` pair — a
two-tuple, so the absence of a mint is visible in the shape before any check.

The brand, the authority and the reserve address are all derived from named strings
(`blake2b256("rchain:rev:brand")`, `…:authority`, `…:ertp-reserve`), so anyone can recompute them. A
REV amount is a claim on a reserve that vault REV backs:

- `revBrand(ret)` registers the brand and answers with its **bytes, never the authority** — a caller who
  learned the authority could mint REV, which is the one thing this brand has no arm for;
- `revFund(funder, amount)` moves REV **from the funder's own vault** — derived from the presented
  `deployerId`, never a supplied address — into the reserve, then credits the purse;
- `revRedeem(amount, to)` checks the holding, pays the address out of the reserve, then debits;
- `revWithdraw(amount)` mints a payment out of a purse, so a REV purse is not a one-way door.

**The authority is derived in Rust** (`rev_authority()` = `blake2b256("rchain:rev:authority")`) **and is
never replied on any channel.** No Rholang term can
construct a `GPrivate`, so the authority can never be presented; a byte array of exactly its bytes is
not a name. The reserve address is derived from a name nobody can construct, so no deploy can spend
it. `REV_SUPPLY_IS_CONSERVED` is the instrument over both halves: the vault total is invariant, and the
float is backed — `Σ REV holdings ≤ the reserve`. Funded, redeemed and withdrawn REV is *moved*, never
created; the ERTP layer never makes vault REV.

`revVault` is untouched. Its reply shapes are pinned by wallet and rgov vectors, and its `deposit`
stays refused. REV's ERTP path is new operations, not a branch inside the old ones.

## Amounts are Int-valued, and bounded

An amount is `(brand, value)` with an **`Int`** value, and the ledger holds a `NonNegI64`.
`amountMath` therefore **refuses** a value the ledger could not hold rather than letting one exist that
cannot be deposited: `i64::MAX + 1` promotes to a `BigInt`, and a `BigInt` is refused — never
truncated, never wrapped. This is a deliberate cut against ERTP's arbitrary-precision `NAT`, and it is
visible: an `add` whose sum would overflow replies `(false, …)`.

## Reading a balance

The balance readers reply a **bare `Int`**, not an amount: `getCurrentAmount`, `getAmountOf` and
`deposit` all answer `(true, n)`. The pair-shaped replies are `amountMath`'s constructors and
arithmetic (`make`, `add`, `subtract`), which answer `(true, (brand, n))`. This is a divergence from
Agoric's ERTP. The full reply-shape table is in [the API](../developer/ertp.md#the-api).

## See also

- [Tokens in Rholang: ERTP](../developer/ertp.md) — the API, with worked examples.
- [OCapN interoperability](ocapn.md) — how these capabilities cross to a foreign peer.
- [`spec/RUST-FIRST.md`](../../../spec/RUST-FIRST.md) — the native-state design this ledger follows.
- [`spec/GENESIS.md`](../../../spec/GENESIS.md) — what a blessed contract has to appear in.
