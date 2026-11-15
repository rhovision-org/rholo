# The node's API response schema (the standard)

**Why this file exists.** There was no published statement of *what a client receives*. Every client
guessed, and each guess was only falsified when someone ran it — which is how the node came to wrap
`rho:registry:lookup` replies in `(uri, value)` (C18) while every oracle-era client consumed the
value alone, and how the whole rgov governance contract family came to return `[]` with no
diagnostic. A shape is a published contract: changing one is a breaking change, so every row here
cites the oracle it follows and the test that enforces it.

**This file has been wrong once, and the way it went wrong is worth knowing.** Its rule 1 said the
wire had no `data` envelope and called the envelope "the Scala/OpenAPI *schema* artifact's spelling,
not the wire's" — and, because the only consumer (`r-wallet/`) had been written to that rule, the port
and the wallet agreed with each other and neither with the reference node. When rule 1 was corrected
(AUDIT C38) the wallet's parser broke on 22 of its 36 contracts: **a correction to a published contract
is still a breaking change**, and this one landed without being called out as such. The consumer is
out-of-tree, so nothing in this repository can assert what it expects — which is exactly why the rule
below cites the *reference* rather than a consumer, and why the migration is the client accepting both
forms.

**Scope.** What rholang code and HTTP clients receive. The HTTP DTOs live in the OpenAPI artifact
(`node/src/web/http.rs`, served at `/api/v1/openapi.json`); this file covers what it cannot: the
rholang-typed values, and the reply shapes of the system processes.

## Normative rules

1. **`RhoExpr` is externally tagged and carries the reference's `data` wrapper**:
   `{"ExprInt":{"data":42}}`, `{"ExprString":{"data":"…"}}`, `{"ExprBytes":{"data":"<hex>"}}`,
   `{"ExprUnforg":{"data":{"UnforgDeploy":{"data":"<hex>"}}}}`. Collections:
   `ExprList`/`ExprTuple`/`ExprSet`/`ExprPar` are arrays under `data`; `ExprMap` is a JSON **object**
   under `data`; an unforgeable nests one level deeper.

   **This rule was wrong until AUDIT C38, and the way it was wrong is worth keeping.** It said the
   wire had no `data` envelope, and that the envelope was "the Scala/OpenAPI *schema* artifact's
   spelling, not the wire's" — a claim that would be true only if the document were generated
   separately from the codec. It is not: the reference's own JSON layer derives both from the *same*
   generic instances — `implicit lazy val rhoExprSchema: JsonSchema[RhoExpr] =
   schemaTagged[RhoExpr]` over `genericTagged`/`genericRecord`
   (`legacy/node/src/main/scala/coop/rchain/node/api/json/JsonSchemaDerivation.scala:45-82`), where
   `schemaRecord` is endpoints4s's case-class-to-JSON-object derivation. So the field named `data` in
   `final case class ExprInt(data: Long)` *is* the serialization, and the document is its shadow
   rather than a rival spelling. The port emitted the unwrapped form, this file blessed it, and law 42
   was then written from the port — so code and law agreed with each other and neither with a client.
   The corpus could not catch it: it compares the node to the model.

   **Migration, stated because the correction broke a client.** A consumer written to the bare form
   must accept the envelope (or read `Expr*["data"]`); a consumer written to the *reference* node is
   unaffected, because the reference always emitted the envelope. The wallet is being changed to accept
   both; the envelope is what a fresh chain has emitted since C38.

   Note the lossiness that follows: a set is indistinguishable from a list on the wire.
2. **Terminal results are a list.** A deploy or explore result is always wrapped one level
   (`[42]`, `[]` when the term sent nothing), because the result channel is a `Par`.
3. **Where a reply is read from, in order — and the response says which.** A deploy's `expr` is read
   from these channels, first non-empty wins, and the response's `replySource` names the one that
   answered (`"firstPrivateName"`, `"out"`, or `"none"`):
   - **`POST /api/v1/explore-deploy`** (and `-by-block-hash`):
     1. the term's **first `new`-bound name** — the reference node's own convention, stated in the
        Scala's `BlockApiImpl` as "be sure the first new should be `return`", and the only channel the
        port used to read;
     2. **`@"out"`** — the channel the corpus, the examples and the system-process conformance tests
        use, added by AUDIT C38 because reading only the first meant a term written this way returned
        `{"expr": []}`, indistinguishable from a term that produced nothing.
   - **`GET /api/v1/deploy-status/{sig}`**: the deploy's own `rho:rchain:deployId` channel
     (`rho:id:<sig>`), which is how the reference node reports a deploy's result.
4. **`[]` means the term sent nothing to the channels in rule 3** — not "failed" and not
   "unsupported". On the explore path `replySource: "none"` says it explicitly; a term that must be
   seen sends to one of the channels in rule 3. A reduce error is a 400 with the error text, and a
   deploy that fails is `processedWithError`.
5. **`POST /api/deploy` and `POST /api/propose` return a JSON-encoded string**
   (`"Success!\nDeployId is: <hex>"`), not a JSON object. `deploy-status` returns the
   `DeployExecStatus` enum instead: `{ProcessedWithSuccess:{deployResult,block}}`,
   `{ProcessedWithError:{deployError,block}}`, `{NotProcessed:{status}}` — **the variant tag is
   capitalized and its fields are camelCase**, which is the `serde` default for an externally-tagged
   enum carrying `rename_all_fields`. `NotProcessed.status` is one of `"Pooled"`,
   `"Block not yet available"`, `"Unknown"`.

   *Corrected by the September 2026 audit (F-10).* This rule previously said the **tag** was camelCase
   and cited C16 — which fixed the *fields*, not the tag, so the citation was a misattribution on top
   of the error. Three sources agree on the capitalized tag and none on the old wording: the wire
   (`node/src/api/dto.rs`, which sets `rename_all_fields` and no `rename_all`), the pinned HTTP test
   (`node/tests/api_surface.rs`), and the machine-checked envelope law (`spec/Rchain/Envelope.lean`,
   which rejects the camelCase spelling outright). A consumer coded to the old wording looked for a
   key that is never sent.
6. **`POST /api/explore-deploy` takes a raw JSON string body** (the term), not
   `ExploreDeployRequest`.
8. **A signed service object is verified over canonical bytes, never over its JSON envelope.** The
   history credit, the range authorization and the audit attestation travel as JSON (the statement
   plus `signer`, `sigAlgorithm`, `sig`), but the signature covers a domain-tagged, length-prefixed
   preimage defined by the type (`models/src/casper/protocol/audit.rs`,
   `node/src/web/history_credit.rs`). A verifier must not hash the JSON: key order, whitespace and
   number formatting are not part of the signature, and a parser that accepted a second spelling of
   the same statement would accept a different signed message.


## System-process reply shapes

Oracle order of preference: the legacy Scala source, the genesis `.rho` sources it deploys, the
recorded expected outputs of the Scala-era examples/corpus. "Rust-first" means the process has no
Scala oracle (a documented extension) and this file is its definition.

| urn | arity | reply | oracle | status |
|---|---|---|---|---|
| `rho:registry:lookup` | 2 | **the stored value alone**; `Nil` when unknown | genesis `Registry.rho:397-401` + `legacy/rholang/examples/tut-registry.rho:8,42-47` | **fixed (C18)** — was `(uri, value)` |
| `rho:registry:insertArbitrary` | 2 | a `rho:id:` uri | `Registry.rho:409-426` | ✅ |
| `rho:registry:insertSigned:secp256k1` | 3 | a `rho:id:` uri, or `Nil` | `Registry.rho:433-469` | ✅ (stores `(nonce, data)` as the value) |
| `rho:registry:ops` | 3 | a uri | `SystemProcesses.scala:308-320` | ⚠️ shape ✅, URI encoding differs (z-base-32 vs CRC14) — deliberate, see AUDIT F5 |
| `rho:io:stdout` / `stderr` | 1 | none (prints) | `SystemProcesses.scala:206-222` | ✅ |
| `rho:io:stdoutAck` / `stderrAck` | 2 | `Nil` | `SystemProcesses.scala:211-230` | ✅ |
| `rho:crypto:{sha256,keccak256,blake2b256}Hash` | 2 | `ByteArray` | `SystemProcesses.scala:182-192` | ✅ |
| `rho:crypto:{secp256k1,ed25519}Verify` | 4 | `Bool` | `SystemProcesses.scala:159-180` | ✅ |
| `rho:rev:address` | 3 | `String`, else `Nil` | `SystemProcesses.scala:232-295` | ✅ |
| `rho:rchain:deployerId:ops` | 3 | `ByteArray` | `SystemProcesses.scala:297-306` | ✅ |
| `sys:authToken:ops` | 3 | `Bool` | `SystemProcesses.scala:322-332` | ✅ |
| `rho:block:data` | 1 | **`(blockNumber, sender, timestamp)`** — three, deliberately (pinned as a law-39 catalog row) | oracle sends **two**: `SystemProcesses.scala:355-361` produces `(blockNumber, sender)` from a `(blockNumber, sender, seqNum)` record | **documented extension**, not a defect: `docs/src/rholang/reference.md:93` specifies "number, sender and informational timestamp", and the node's own genesis vault consumes all three (`casper/src/genesis/resources/RevVault.rho:207-209` binds `@blockNumber, @sender, @timestamp`). The port substituted `timestamp` for the oracle's unexposed `seqNum`. **Consequence to know:** a *legacy* two-name consumer (`legacy/casper/src/test/resources/BlockDataContractTest.rho:15-16`) cannot match this and will stall — it must be amended, or the reply versioned, before that corpus is relied on |
| `rho:rchain:revVault` | 1 | `Int`, `Nil`, `(true, addr_string)` | `legacy/casper/src/main/resources/RevVault.rho:103-121,196-204` | ✅ **added 2026-09-27** — `findOrCreate` now returns a *vault handle* (a minted unforgeable name) and `balance`/`transfer` are methods of that handle, authorised by the name; the classic `getBalance(address)`/`transfer(deployerId, …)` shapes are unchanged, so no existing client moves |
| `rho:rchain:multiSigRevVault` | — (a contract, not a fixed reply shape) | **the contract's own API** — `lookup!` then `@(_, MultiSigRevVault)`, then its `create` / `makeSealerUnsealer` / `deployerAuthKey` methods | `MultiSigRevVault.rho` (vendored and adapted, like `MakeMint.rho`) | ✅ **added 2026-09-27** — AUDIT C114's alternative taken: genesis installs the adapted contract and `GENESIS_ALIASES` maps this shorthand to `GenesisAliasSource::Contract`, so `lookup!(\`rho:rchain:multiSigRevVault\`, *ch)` resolves to the **multi-signature** contract, with its quorum, co-signers and confirmation step (`a_multisig_vault_spends_through_the_contract_that_holds_it` drives a delegated spend through it). The native **fixed channel** of the same name answers no method and refuses with a message naming the lookup path: binding the urn as a name used to return single-key custody under a multi-signature name, so that path stays loud rather than silent |
| `rho:rchain:{revVault,pos,makeMint}`, `rho:lang:{listOps,nonNegativeNumber}` | — | `(9223372036854775807, bundle+{dispatcher})` — the signed-registration shape consumers destructure as `@(_, X)` | genesis content + aliases; `Registry.rho:371-379` is the oracle's shorthand aliasing | ✅ **resolved** — genesis now installs the interpreted library contracts and seeds the shorthand aliases natively (`spec/GENESIS.md`), so `lookup!(\`rho:rchain:revVault\`, *ch)` resolves *and* the value answers a call. Native channels keep working by direct binding too |
| `rho:rchain:ertp:ledger` | 1 (+ remainder, so calls are `L!("op", args…, ret)`) | **`(true, value)` / `(false, reason)` for every op**, never a bare value — one shape for a caller to match. `makeKit` alone replies a bare `(brand, auth)` pair, because it cannot fail. Ops and their values: `makeKit(ret)`, `makePurse(brand, ret)`, `balance(brand, holder, ret)`, `mint(brand, authority, value, ret)`, `withdraw(brand, authority, purse, value, ret)`, `deposit(brand, purse, payment, ret)` | **Rust-first, no oracle** (issue #249) | ✅ **added 2026-10-05** — the consensus half of ERTP: a brand's minting authority, and every purse's and payment's holding (`PREFIX_ERTP = 0x0A`). Names are never returned and never accepted as bytes (`ertp_name` refuses a `GByteArray`: no Rholang term can construct a `GPrivate`, which is what makes a ledger key unforgeable rather than merely unguessable). Amounts are `Int`-valued and bounded by the ledger's `NonNegI64` — refused, never truncated |
| `rho:rchain:ertp` | — (a contract, not a fixed reply shape) | **the ERTP object API** — `lookup!` then `@(_, ERTP)`, then `makeIssuerKit` (→ a 3-tuple `(brand, mint, issuer)`) and `getRevIssuer` (→ a 2-tuple `(brand, issuer)`, **no mint**). Everything else is an arm of an object the kit returns, and replies `(true, value)` / `(false, reason)` | our own term (`casper/src/genesis/resources/ERTP.rho`), installed by the blessed deploy at `blake2b256("rnode/genesis/ertp")` | ✅ **added 2026-10-05** — the shorthand is deliberately **not** the ledger's urn, so the alias tier holds the object API while `rho:rchain:ertp:ledger` stays bound to the native channel and neither name changes meaning. REV is reachable here as a standard brand: `revFund`/`revRedeem`/`revWithdraw` are new ledger ops, and `revVault`'s own row above is unchanged |
| `rho:rchain:ertp:ledger`: `revBrand`, `revFund`, `revRedeem`, `revWithdraw` | 1 (+ remainder) | **the same `(true, …)`/`(false, …)` shape.** `revBrand(ret)` → `(true, brand)` and **never the authority**; `revFund(deployerId, value, purse, ret)` → `(true, balance)` after the REV has *left the caller's own vault* for the reserve; `revRedeem(purse, value, to, ret)` → `(true, remaining)` after the reserve has paid `to`; `revWithdraw(purse, value, ret)` → `(true, payment)` | **Rust-first, no oracle** (issue #249) | ✅ **added 2026-10-05** — REV's ERTP path. The four are **new ops rather than a branch inside `withdraw`/`mint` on the brand**: the REV authority is a Rust constant no deploy can present, so "the same call, but REV" would be a call whose authority requirements differ invisibly (the silent downgrade AUDIT C114 exists to stop). `revVault!("deposit", …)` is unchanged, and its refusal still names REV's only mint |
| `rho:rchain:authKey`, `rho:lang:either`, `rho:rchain:systemContractManager`, `rho:rchain:configPublicKeyCheck`, `rho:lang:treeHashMap` | — | **nothing — not seeded** | provided only by the interpreted `Registry.rho`, which this port does not install (its bootstrap handshake cannot match the native arity-2 `rho:registry:lookup`) | **deliberate, no consumer**: no occurrence in either the wallet or rgov consumer trees. Recorded with the reasoning in `spec/GENESIS.md` |
| `rho:rchain:pos` (native) | 1 | `Map`, `Set`, `List`, `(Bool, Nil\|String)` | `Pos.rhox:271-362` | ⚠️ shape ✅; `trust`/`untrust` have no oracle (extensions). **`delegate` / `undelegate` / `getDelegations` are extensions too** (#193, law 57): the oracle has no delegation primitive. Their shapes are `delegate(*deployerId, operatorKey, amount, ret)` and `undelegate(*deployerId, operatorKey, ret)`, each replying `(Bool, Nil\|String)`, and `getDelegations(delegatorKey, ret)` replying a **`List` of 4-tuples** `(operatorKey, amount, accruedRewards, pendingDeadlineOrNil)` — scoped to the key asked about, because the ledger is unbounded in delegator count. The `List` reply shape is new to this channel and is why the column above gained it |
| `rho:txn`, `rho:gov:*`, `rho:qucalc:*`, `rho:io:http` | — | as implemented | — | **Rust-first** — no Scala oracle. `docs/src/qucalc/architecture.md:100-107` already tabulates the qucalc/gov rows |

## Consumer-side consequences (what this standard bought us)

- A client consumes a lookup as `lookup!(uri, *ch) | for (X <- ch) { X!(…) }` — no unwrapping.
  Consumers of *system* contracts destructure `@(_, X)` because those are registered with a
  `(nonce, data)` **value**, which is a property of the stored value, not of the lookup.
- A client that receives `[]` has learned that the term produced nothing; it must not read that as
  success. `r-wallet/scripts/test-output-json.ts` treats `[]` as a failure unless the contract is
  explicitly allow-listed with a reason.
- Result rendering lossiness is a wire property (sets→arrays, unforgeables→bare hex), so a client
  should not round-trip its own output back as input without knowing that.

## Enforcement

- **Catalog (law 39)**: the rows a call can be *spelled* for are data in `spec/Rchain/Protocol.lean`
  (`replyCatalog`), emitted to `spec/conformance/protocol.tsv` and checked against a running node by
  `rholang/tests/lean_protocol_corpus.rs` — each row calls its urn with the arguments it spells and
  classifies every slot of the reply, arity included. This file is the tie on the other side:
  `tools/check-lean-conformance.sh` fails if a catalog urn has no row here. Read a row here as a
  *claim*, and that corpus as the machine-checked form of it; `spec/INVENTORY.md` row 39 names the
  urns the catalog cannot reach yet (the `ByteArray`-argument ones).
- **Node**: `rholang/tests/system_process_conformance.rs` asserts reply *shapes* — and, since C18,
  also **reachability** (`a_looked_up_contract_can_be_called_through_its_lookup_reply`). A shape
  assertion alone can pass while every real client fails, so reachability is part of the standard.
- **Consumer**: `r-wallet/scripts/test-output-json.ts` asserts the shapes it depends on from the
  other side (`check_process_api_schema`), naming the register entry when one drifts.
- **Register**: every change to this file that alters a row is a breaking change to a published
  contract and gets an entry in the `spec/AUDIT.md` check-off.
