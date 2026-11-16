# Language reference

A compact reference for the rholang surface syntax. The precise abstract grammar and sorts are in
[Grammar and sorts](../formal/grammar-sorts.md); this page is the programmer-facing cheat sheet.

## Core grammar

```
Proc ::=  Name ! ( Name,* )                -- send
       |  Name !! ( Name,* )               -- persistent send
       |  for ( Pattern <- Name ;* ) { Proc }    -- receive (; = join)
       |  for ( Pattern <= Name ;* ) { Proc }    -- persistent receive (peek)
       |  contract Name ( Pattern,* ) = { Proc } -- sugar for a persistent receive
       |  new NameDec,* in { Proc }              -- fresh unforgeable names
       |  match Name { Pattern => Proc ,* }      -- first-match-wins dispatch
       |  if ( Bool ) Proc else Proc             -- sugar for match true/false
       |  Proc | Proc                            -- parallel composition
       |  ! Proc                                 -- replication
       |  Nil

Name ::=  @ Proc                            -- quote a process
       |  * Name                            -- evaluate a name
       |  Ground                            -- Int | BigInt | String | Bool | Uri | ByteArray
       |  Unforgeable                       -- new name | deployerId | deployId | sysAuthToken
       |  Bundle                            -- bundle{+|-|0|} Proc
       |  [ Name,* ] | ( Name,* ) | Set( Name,* ) | { Name : Name ,* }   -- collections
```

`;` in a receive is a **join** (simultaneous receive on every listed channel). `<=` is a peek; `!!` is a
persistent send.

**A `contract` is a persistent receive, so it claims the name it is installed on.** Every later send
to that name is consumed by the contract's body; a second `for` on the same name waits in silence.
`new` names are unforgeable and immune — only a holder can name them — while quoted and
registry-published names are public by construction. Installing a contract on a public name is a
deliberate claim on it, not an accident the runtime can distinguish (Law 38).

## Patterns

```
Pattern ::=  Name                          -- an exact (literal) process
          |  Var                           -- binds
          |  _                             -- wildcard (matches anything, binds nothing)
          |  ~ Pattern                     -- negation (matches anything the pattern does not)
          |  Pattern /\ Pattern            -- and (binds in both)
          |  Pattern \/ Pattern            -- or  (binds in neither)
          |  = Name                        -- exact match (no binding/shadowing)
          |  Var =* Name                   -- match an exact name and bind it
          |  Pattern | Pattern             -- match a parallel composition (greedy)
```

Name equivalence is applied only at the top level of a name ("look through the looking glass once").

## Precedence

From tightest to loosest: postfix method calls and `*`/`@`; arithmetic (`* / %` then `+ -`);
comparisons; `not`; `and`; `or`; then `|` (parallel composition); then `new`/`for`/`match`/`if` bodies.
Parenthesize when in doubt — `( … )` always wins.

## Grounds and operators

| Kind | Literals | Operators / methods |
|---|---|---|
| Integer | `42`, `-7` | `+ - * / %`, `< <= > >= == !=`; **exact** — overflow promotes to BigInt (never wraps); `bitAnd`, `bitOr`, `bitXor`, `bitNot`, `shiftLeft`, `shiftRight`, `shiftRightUnsigned` |
| Big integer | arbitrary precision | same |
| Boolean | `true` `false` | `and`, `or`, `not` |
| String | `"…"` | `++`, `${…}` interpolation, `length`, `slice`, `substring`, `indexOf`, `contains`, `startsWith`, `endsWith`, `toLowerCase`, `toUpperCase`, `capitalize`, `reverse`, `trim`, `isEmpty`, `nonEmpty`, `replace`, `split`, `format`, `toString`, `toUtf8Bytes`, `hexToBytes` |
| URI | `` `rho:…` `` | — |
| Byte array | `"…".hexToBytes()` | `length`, `nth`, `slice`, `toUtf8Bytes`, `bitAnd`, `bitOr`, `bitXor`, `bitNot` (equal lengths) |

Numbers are **exact**: integer arithmetic never wraps — an `Int` whose result leaves the machine range
promotes to `BigInt`, mixed `Int`/`BigInt` operands interoperate, and `2 == 2n` (RCHIP #51; recorded in
[`spec/AUDIT.md`](../../../spec/AUDIT.md) §6). String methods index and count by **character**
(Unicode scalar values), and `toString` on an `Int`/`BigInt`/`Bool`/`Uri` gives its string form
(so `42.toString() ++ " units"` works). The bit operations (RCHIP #12) are methods too, since `|` and
`~` already belong to the grammar — see [Data structures](data-structures.md#bit-operations).

Scalar equality is numeric while structural identity is not: `1 == 1n` is true, but `1` and `1n` are
distinct map keys and a pattern for one does not match the other. Canonical ordering is also
class-major — integers sort before big integers whatever their value — so the canonical form of a
structure is not its numeric order.

**There is no decimal value type.** An exact decimal (written `1.5d`) existed from 19 to 31 October
2026 and was removed before mainnet: it was a fixed-point value isomorphic to a scale-38 integer, and
`1d / 3d` was an error rather than a ratio. A `d`-suffixed literal no longer parses, and a wire term
carrying its reserved tag 35 is refused. The decision, what was archived and what a future attempt must
do differently are in [the decimal type, withdrawn](../formal/decimal-removed.md).

## Collections

| Type | Syntax | Methods |
|---|---|---|
| List | `[a, b, c]` | `nth`, `length`, `slice`, `getOrElse`, `contains`, `++` |
| Tuple | `(a, b, c)` | `nth`, `length` |
| Set | `Set(a, b)` | `contains`, `union`, `diff`, `delete`, `size` |
| Map | `{k: v, …}` | `get`, `getOrElse`, `set`, `keys`, `values`, `contains`, `union`, `diff`, `delete`, `size` |

## System channels

| Channel | Purpose |
|---|---|
| `rho:io:stdout` / `rho:io:stdoutAck` | print a line / its completion ack |
| `rho:io:stderr` / `rho:io:stderrAck` | print to stderr / its ack |
| `rho:registry:lookup` | resolve a name to a value |
| `rho:registry:insertArbitrary` | insert a value (unauthenticated) |
| `rho:registry:insertSigned:secp256k1` | insert a value, signed |
| `rho:rchain:deployerId` | the unforgeable id of the deployer (who invoked the contract) |
| `rho:attachment:N` | the *N*-th (1-based) binary attachment of the deploy, as a `ByteArray` (RCHIP #39) |
| `rho:rchain:revVault` | the REV vault contract |
| `rho:rchain:pos` | the proof-of-stake contract |
| `rho:block:data` | the current block's number, sender and informational timestamp (the proposer's wall clock, unverified — validators never judge it) |
| `rho:io:http` | the deploy-supplied HTTP **claim registry** (no fetch): `record` (first writer wins), `get`, `check`, `height` |
| `rho:txn` | the cross-shard two-phase-commit participant (`prepare` / `commit` / `abort`) |
| `rho:crypto:blake2b256Hash` / `keccak256Hash` | hashing |
| `rho:crypto:secp256k1Verify` / `ed25519Verify` | signature verification |

## Known limitations

Some surface features are known incomplete in the reference semantics (see
[`legacy/rholang/README.md`](https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/rholang/README.md) "what's broken"): guarded patterns, and
certain 0-arity/match-case pre-evaluation edge cases. Treat the K semantics under
[`legacy/rholang/src/main/k/rholang/`](https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/rholang/src/main/k/rholang/) as the executable
reference, and the [laws](../formal/laws.md) as the authoritative invariants.
