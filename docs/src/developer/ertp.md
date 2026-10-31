# Tokens in Rholang: ERTP

**How-to.** The ledger layout, authority model and Int bound are in
[the ERTP technical reference](../node/ertp.md).

How to make and use a token from Rholang. ERTP is a set of capabilities — a brand, a mint, an issuer,
purses and payments — and this page is the API over them. For the ledger and the authority model
underneath, see [ERTP](../node/ertp.md).

## Reach the contract

The ERTP object API is installed at genesis and reached by its registry shorthand,
`rho:rchain:ertp`. Look it up and bind the name:

```rholang
new rl(`rho:registry:lookup`), ch, ret in {
  rl!(`rho:rchain:ertp`, *ch) |
  for (@(_, ERTP) <- ch) {
    ERTP!("makeIssuerKit", *ret)
  }
}
```

The lookup is per deploy. Contracts cannot hold a name across deploys, so every deploy that needs
ERTP looks it up again.

## A worked example

Make a kit, mint ten units, put them in a purse, read the balance.

```rholang
new rl(`rho:registry:lookup`), ERTPCh, kitCh, amCh, amtCh, payCh, purseCh, ret in {
  rl!(`rho:rchain:ertp`, *ERTPCh) |
  for (@(_, ERTP) <- ERTPCh) {
    ERTP!("makeIssuerKit", *kitCh) |
    for (@(brand, mint, issuer) <- kitCh) {
      // An amount is made by the brand's own amountMath.
      issuer!("getAmountMath", *amCh) |
      for (@(true, amountMath) <- amCh) {
        amountMath!("make", brand, 10, *amtCh) |
        for (@(true, amount) <- amtCh) {
          // The mint creates a payment out of nothing.
          mint!("mintPayment", amount, *payCh) |
          for (@(true, payment) <- payCh) {
            issuer!("makeEmptyPurse", *purseCh) |
            for (@(true, purse) <- purseCh) {
              purse!("deposit", payment, *ret)
            }
          }
        }
      }
    }
  }
}
```

The reply on `ret` is `(true, 10)` — a deposit answers the purse's new balance.

A payment is **consumed by its first deposit**. Depositing the same payment twice refuses with
`"that payment has already been deposited"` and moves nothing. And a purse refuses a payment of
another brand without burning it: the refused payment is still live and can be deposited into its own
brand's purse.

## The API

`makeIssuerKit` replies a **three-tuple** `(brand, mint, issuer)`. `getRevIssuer` replies a
**two-tuple** `(brand, issuer)` — no mint. Every other operation replies `(true, value)` or
`(false, reason)`, so a caller has one shape to match.

| Object | Arms |
|---|---|
| contract | `makeIssuerKit` → `(brand, mint, issuer)`; `getRevIssuer` → `(brand, issuer)` |
| issuer | `getBrand`, `getAmountMath`, `makeEmptyPurse`, `getAmountOf(payment)` |
| mint | `mintPayment(amount)` |
| amountMath | `make(brand, value)`, `add`, `subtract`, `getValue`, `getBrand`, `isEqual`, `isEmpty` |
| purse | `deposit(payment)`, `withdraw(amount)`, `getCurrentAmount`, `getDepositFacet` |
| payment | `getAllegedBrand`, `getLedgerToken` — the token is handed to whoever holds the payment, because **holding it is the authority to deposit it**: the token alone does nothing without a purse |
| REV purse | the purse arms, plus `revFund(funder, amount)` and `revRedeem(amount, to)` |

Reply shapes worth pinning:

| Call | Replies |
|---|---|
| `purse!("getCurrentAmount", *ret)` | `(true, 10)` — a bare Int |
| `purse!("deposit", payment, *ret)` | `(true, newBalance)` |
| `purse!("withdraw", amount, *ret)` | `(true, payment)` |
| `mint!("mintPayment", amount, *ret)` | `(true, payment)` |
| `issuer!("getAmountOf", payment, *ret)` | `(true, 10)` |
| `amountMath!("make", brand, 10, *ret)` | `(true, (brand, 10))` |
| `amountMath!("add", a, b, *ret)` | `(true, (brand, n))` |
| `issuer!("getAmountMath", *ret)` | `(true, amountMath)` |
| `issuer!("makeEmptyPurse", *ret)` | `(true, purse)` |

Balances are bare Ints and amounts are bounded by the ledger's `NonNegI64`; the exact replies and the
overflow rule are in [the reference](../node/ertp.md#amounts-are-int-valued-and-bounded).

## Depositing into your own purse

A purse's own name is the authority to deposit into it, so a purse you created is one you can be paid
into. To let someone else deposit without letting them withdraw, hand them the **deposit facet**:

```rholang
purse!("getDepositFacet", *ret)   // -> (true, facet)
```

The facet forwards `deposit` and nothing else. Its holder never sees the purse's ledger token and
cannot withdraw.

## REV

REV is a standard brand whose issuer is in Rust and has no mint. Get it with `getRevIssuer`, which
answers a `(brand, issuer)` pair — the missing mint is visible in the shape.

```rholang
new rl(`rho:registry:lookup`), deployerId(`rho:rchain:deployerId`),
    ch, issCh, amCh, amtCh, purseCh, ret in {
  rl!(`rho:rchain:ertp`, *ch) |
  for (@(_, ERTP) <- ch) {
    ERTP!("getRevIssuer", *issCh) |
    for (@(revBrand, issuer) <- issCh) {
      issuer!("getAmountMath", *amCh) |
      for (@(true, amountMath) <- amCh) {
        amountMath!("make", revBrand, 100, *amtCh) |
        for (@(true, amount) <- amtCh) {
          issuer!("makeEmptyPurse", *purseCh) |
          for (@(true, purse) <- purseCh) {
            // Fund it from the deployer's own vault; the address is derived, not passed.
            purse!("revFund", deployerId, amount, *ret)
          }
        }
      }
    }
  }
}
```

`revFund` moves REV **from the funder's own vault** into the ERTP reserve and credits the purse;
`revRedeem` pays an address out of the reserve and debits; `withdraw` mints a payment out of the purse.
A kit's purse has **no** `revFund` or `revRedeem` arm at all — those ops arrive as a capability the REV
issuer passes, so their absence on an ordinary purse is structural, not a brand check.

The funder's address is derived from the presented `deployerId`, never from an address argument, so
`revFund` cannot be pointed at someone else's vault.

## Gotchas

- **An unmatched `for` in Rholang is silent.** A call that reaches nothing looks like success. During
  development, report each observation on its own `@"tag"` channel and check the tags arrived — a
  missing tag is a failed assertion with a name, which is the only thing that makes a passing test
  mean anything.
- **Reply-shape drift is silent too.** `for (@(true, x) <- ch)` on a refusal does not fire and does
  not error. Match on the shape you expect and let the other case report.
- **Every lookup is per deploy.** Bind the contract inside each deploy that uses it.
- **A kit's purse is not a REV purse.** The REV-only arms are the difference, and it is deliberate.

## See also

- [ERTP](../node/ertp.md) — the ledger, the authority model, and REV's reserve.
- [Building applications on the local devnet](building-apps.md) — how to deploy and read back a term.
- [Talking to a node from another implementation](ocapn.md) — using ERTP from outside Rholang.
