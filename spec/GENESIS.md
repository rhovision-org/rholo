# Genesis content — the manifest

Genesis content is **consensus identity**: every node of a network must agree on it, and changing it
later is a genesis change, not a deploy. So this list is deliberately small, and every entry names the
consumer that justifies it. `casper/src/genesis/standard_deploys.rs::GENESIS_ALIASES` is the
machine-readable form of the alias table below; `spec/TEST-COVERAGE.md` pins it with tests.

Before this landed, `default_blessed_terms` returned `Vec::new()`: a fresh chain's native registry was
empty, so `lookup!(\`rho:rchain:revVault\`, *ch)` answered `Nil`. A `Nil` reply is **silent** — an
unmatched `for` is not an error, it just never fires — which is why the failure presented to
consumers as their own bug (and why the rgov family returned `[]` with no diagnostic).

## What genesis installs

| # | Contract | Installed | Shorthand seeded | `rho:id` (hardcodable) | Consumer it unblocks |
|---|---|---|---|---|---|
| 1 | `ListOps.rho` | blessed deploy (`LIST_OPS_PK`) | `rho:lang:listOps` | `rho:id:6fzorimqngeedepkrizgiqms6zjt76zjeciktt1eifequy4osz3o` | rgov `rholang/core/CrowdFund.rho:8` — `lookup!` then `@(_, *ListOps)` then `ListOps!("fold", …)` |
| 2 | `NonNegativeNumber.rho` | blessed deploy (`NON_NEGATIVE_NUMBER_PK`) | `rho:lang:nonNegativeNumber` | `rho:id:hxyadh1ffypra47ry9mk6b8r1i33ar1w9wjsez4khfe9huzrfcyo` | `MakeMint.rho:27` looks this up *during its own deploy*, so it must be seeded for makeMint to install at all |
| 3 | `MakeMint.rho` (adapted, below) | blessed deploy (`MAKE_MINT_PK`) | `rho:rchain:makeMint` | `rho:id:asysrwfgzf8bf7sxkiowp4b3tcsy4f8ombi3w96ysox4u3qdmn1o` | rgov `src/actions/makeMint.rho:13` and wallet `snippets.ts:751` — `lookup!` then `@(nonce, *MakeMint)` then `MakeMint!(*ch)` |
| 4 | `AuthKey.rho` (adapted) | blessed deploy (`AUTH_KEY_PK`) | `rho:rchain:authKey` | — | `MultiSigRevVault.rho:35` looks it up before it can install, and every `deployerAuthKey` is made through it |
| 5 | `MultiSigRevVault.rho` (adapted) | blessed deploy (`MULTI_SIG_REV_VAULT_PK`) | `rho:rchain:multiSigRevVault` | — | multi-signature custody: `lookup!` then `@(_, MultiSigRevVault)` then its `create` / `makeSealerUnsealer` / `deployerAuthKey` methods. **This is AUDIT C114's alternative taken**: the channel used to answer with the single-signer handler and then to refuse, and is now answered by the contract |
| 6 | `ERTP.rho` (ours) | blessed deploy (`ERTP_PK`) | `rho:rchain:ertp` | `rho:id:75fy4nja3qsq958b5k3rmi3sz1g653fa5c1xfkwdx9awpfwybauo` | the ERTP object API itself (issue #249): `lookup!` then `@(_, ERTP)` then `ERTP!("makeIssuerKit", *ch)`. **The shorthand is deliberately *not* the ledger's urn** — `rho:rchain:ertp:ledger` stays bound to the native system process, so the alias tier holds the object API and the native channel holds the ledger it is written over, and neither name changes meaning |

Note on the URIs: they are this port's own zbase32 encoding of `blake2b256(deployer public key)`
(`rholang/src/registry.rs:43`), which deliberately does not reproduce the Scala's CRC14+ZBase32 bit
order (`registry.rs:3-6`). They are **stable across chains of this port** because each blessed deploy's
key is a fixed constant — that is what makes them hardcodable — but they are **not** the 54-char
mainnet values carried in the `.rho` header comments.

## The rgov governance set — the **testnet** setup

Installed from `casper/src/genesis/resources/rgov/` (vendored from `rhovision-org/rgov`; commit,
licence position and every adaptation: `resources/rgov/NOTICE`), so that a governance client on a
fresh chain needs **no bootstrap step and no recorded URI**: it hardcodes the constants below.

Two registrations live in these files and only one keeps upstream's shape:

- a **class** registers with `insertArbitrary!(bundle+{*X}, …)` — the stored value is the bare class,
  which is what every rgov consumer destructures (`for (Dir <- lookCh)`, `for (@(_, *X) <- ch)` for the
  node's own signed contracts). **Do not convert this to `insertSigned`**: its value is a
  `(nonce, value)` tuple, and the master-directory template then stalls *silently* — a deploy that
  reports `processedWithSuccess` and produces nothing. That was tried, and it is why the class keys
  below are chosen keys with a copy behind them rather than derived ones;
- each class therefore **publishes** the URI it was given (`["<name>", <uri>]` on the fixed channel
  `rnode:genesis:rgov-uri`), and genesis copies the entry onto a key *we* choose, which is what makes
  the key independent of the install order (the registered URI is `blake2b256` of the deploy's RNG
  state — deterministic on genesis, but it moves if any deploy is inserted before it).

### The constants a client hardcodes

| Contract (`rgov`) | `rho:id` | Consumer it unblocks |
|---|---|---|
| `Kudos.rho` | `rho:id:6hstcrmii97pxfnnwturmhmhfbtomdnh6q6fc6nrnsgfbuyjogyy` | the directory's `Kudos` slot; `Kudos!("peek"/"award", …)` |
| `Inbox.rho` | `rho:id:cbqr7s4o9yb6trpcitj7ne3qdyci8ph7u8yn8qp1hs1woe59iweo` | the `newInbox`/`sendMail`/`sendChat` snippets |
| `Directory.rho` | `rho:id:atsx1axaqqyjq841y8em3wgtrf8fe9iwm5ykwjyk3j66fxyskt7y` | every per-deployer dictionary |
| `memberIdGovRev.rho` (the "roll") | `rho:id:sff83mg96h5rt3emdncpnkrwgfobh9uuyhgw3tuctkeq17fnykqy` | `MemberDirectory!("make"/"makeFromURI", …)` |
| `Issue.rho` | `rho:id:q5eo8oexygm1yu3ha7g4t55gnau3onyex39198hm4d69n7ifhsto` | `newIssue`, `castVote`, `tallyVotes`, delegations |
| `Ballot.rho` | `rho:id:hpg1dns31bbdwb4yf9u6teabt7doutus6xfnu6uij6rweoszc8qy` | the ballot snippets |
| `Chat.rho` | `rho:id:yaer85qmkisrnr3h7yir389u687jhrzs4p1h67jtqasp4j5fw8sy` | `newChat`, `sendChat`, `readChat` |
| `Group.rho` | `rho:id:4ms51n1oramet9iu94df4483xp88jogfsfcnnsmen6xpraz7gs9o` | `newGroup`, `joinGroup`, `addMember` |
| **`masterdict-resolve`** | `rho:id:fbcb5xks6kygyyeixsuq1ahpcb6jwahpmt5s5byfwfpcmi64bpqo` | the name layer's read side — `resolve`, `resolveAt`, `versionsOf`, `ownerOf`, `sealed`, `targetOf`, `aliases`. **This is where the `MasterURI` a governance client takes now points** |
| **`masterdict-publish`** | `rho:id:3q9ax77mpszucp83yqomxe5c5161tg7u1mqfhw7b4k71j869bpiy` | the self-scoped write side: `publish(path, value)`, `seal(path)`, `grant(path)`, `revoke(path)`. Where the old **grant cap** went: nothing is claimed, so nothing is granted — the owner prefix is derived |
| **`masterdict-root`** | `rho:id:1wk4t7op7kjmpkacgzcwkoeo93buy3pqkshypzca7ntw457r4emo` | the **alias tier's governor**: `alias`, `unalias`, `setRootAuthority`. Held by the ceremony key |

`ballot`, `chat` and `group` are **not** in upstream's deployment order: the master directory has
slots for them here because the wallet's editor asks the directory for those class *names*, and a slot
that was never filled answers `Nil` — which a client cannot tell from "broken".

**The name layer is the rooted master dictionary** (issue #99). It replaced the testnet *template* and
the `readcap`/`grantcap` pair with one term, `resources/rgov/MasterDictionary.rho` — **our own, not
vendored**: it has no upstream. `Directory.rho` remains a *class* (its URI is in the table above, and
`memberIdGovRev` imports it), but nothing installs an *instance* of it at genesis any more.

Two tiers, and the access-control system is entirely the first one:

- **rooted**: a name is `<revAddr>/<path>`, and `publish(path, value)` succeeds iff the path's owner
  prefix equals the caller's derived REV address. The prefix comes from
  `rho:rev:address("fromDeployerId", …)` — never from the caller — so a write outside your own root is
  **inexpressible rather than refused**, and a forged id derives `Nil`, which every verb guards before
  it touches state. `grant(path)` still exists, but it hands out a writekey bound to one path **and one
  epoch**; `revoke(path)` bumps the epoch and retires every key already issued for it.
- **aliases**: the short names are aliases to rooted paths — `"Inbox" -> "<revAddr>/Inbox"` — and
  re-pointing one is a single governed act that never rewrites what an earlier client resolved.
  Versions are append-only (`resolveAt(p, 0)` answers the same thing for ever) and `seal(p)` closes a
  path; the only mutable pointer is the alias.

The dictionary publishes the three facets above under constant keys, parks its own admin handle on
`@[*deployerId, "MasterContractAdmin"]` for the ceremony key, **publishes each class under the
operator's own root** and aliases the names to those paths — so `resolve("Directory")` answers the
class *contract*, exactly what the old slot held. `memberDirectory` publishes `GetMe`/`SendThem` the
same way, at `<rootAddr>/GetMe`.

**The admin handle is written by the dictionary, not parked out of reach.** #71's defect was that the
template parked `{"read","write","grant"}` on `@[*deployerId, "MasterContractAdmin"]` keyed by the
genesis deployer — an identity nothing holds after block 1 — so from genesis onward nothing could write
a name, and an application trying to register got no error and no rejection: law 40 (a call the
directory cannot match does nothing) over law 38 (silence is not failure). The dictionary now writes
that datum *itself*, for the deployer that installs it, and it carries `{"read", "write", "root"}` —
its own three facets. No `grant`: a rooted name is derived, so there is nothing to grant. That the
handle is **operator-held and application-immutable** is unchanged and still measured, by
`only_the_ceremony_key_can_reach_the_admin_handle`: the ceremony key's deploy reads it with a `<<-`
peek, while a stranger's identical probe keys the channel to *its own* id, matches nothing, and finds
nothing.

### The three admission questions, answered

These were recorded as open in #71's thread, and deciding them is what #99 owed. Each is now a property
of the deployed dictionary, pinned by `casper/tests/master_dictionary.rs`:

- **Who may claim a name at block 0, and how that authority rotates.** *Nobody claims one.* A rooted
  name is derived from the caller's identity, so there is no unclaimed namespace to race for and no
  admission policy to freeze — the question dissolves rather than being answered by policy. The one
  governed thing is the **alias tier**, and its governor is the identity that installs the dictionary
  (the ceremony key), which hands the role on with `setRootAuthority`. A non-root `alias` call is
  refused: `("dir-error", "not the root authority")`.
- **Whether an existing name may be overwritten.** *It may not.* `publish` **appends**; a version, once
  written, is answered by `resolveAt(path, v)` for ever. The mutable pointer is the alias, and moving it
  is visible, single-writer and governed. That is the "extend-only" reading #71's thread agreed on; the
  overwrite path `Directory.rho`'s `set` gave a `grant` holder is gone from the name layer entirely.
- **Whether `Group`/`Ballot`/`Chat` belong in genesis content at all.** *The classes stay; the names
  stop being frozen.* Their class URIs are still genesis content — installing less would break the
  wallet's editor — but what the *names* mean is now an alias the root authority can re-point, so a
  governance experiment can change them without a genesis change. That is the compromise the rooted
  design buys: the classes are frozen, the interface is not.

**The disclosure rule, which is a discipline and not a mechanism.** A deployer id is a bearer value:
"Nothing was forged. The identity was disclosed, and disclosure is transfer" (`SECURITY.md`). Holding
one lets its holder act as that identity *anywhere*, for as long as it exists, so a contract that stores
`*deployerId` and hands it out transfers that namespace. Nothing inside the calculus prevents it; the
mitigation is that the dictionary itself never does it, which
`the_deployer_id_is_only_ever_the_address_derivation` asserts over the contract's source — every
occurrence of the binder is the derivation, the binder itself, the admin-handle key, or a call to one
of the dictionary's own facets.

Two of the vendored files carry a behavioural repair, both recorded in
`resources/rgov/NOTICE` with their evidence: `Inbox.rho`'s zero-argument `read` restored a store it
consumed (AUDIT C22 item 1), and `Group.rho`'s `@"new"` read the deployer's dictionary *of its own
registration deploy* rather than the caller's, which genesis never writes — so the class answered
nothing, silently (AUDIT C25). The group row above is usable because of that second repair; before it,
`newGroup` hung and `joinGroup`/`addMember` had no group to reach.

### The key: the genesis ceremony's own

`masterDictionary` and `memberDirectory` are signed by **the key that creates the genesis block** —
`create_genesis_block`'s `ValidatorIdentity`. That is the standard genesis-ceremony arrangement, and it
is what the admin handle requires:

- **They must be one key.** The dictionary parks its admin handle on
  `@[*deployerId, "MasterContractAdmin"]` for *its own* deployer, and the `GetMe` feature's registration
  is gated on reading that handle back — as is the dictionary's **root authority**, which is derived
  from the deployer that installs it. Signed by different keys the gate never opens and the feature
  registers nothing. (This rule was originally argued from a handshake that reached "directory answered
  GetMe" and stopped before `getMe` — an observation later explained by AUDIT C21 rather than by the
  keys. The rule stands on the gate's `deployerId`, not on that sighting: the same key is what makes
  `the_governance_terms_are_signed_by_the_ceremony_key` meaningful.)
- **It must be a key whose private half is not public.** An earlier revision signed them with a key
  derived from a string literal in `rgov.rs`; anyone reading the source could compute it and exercise
  the capability on any network that installed it. The ceremony identity is threaded in for that
  reason, and `spec/TEST-COVERAGE.md` records the change.
- **Nothing a client hardcodes moves because of it.** The eight class keys are the classes' own fixed
  keys, and the dictionary's three facet keys are derived from *named strings*
  (`build_uri(blake2b256("rnode/genesis/masterdict-…"))`) rather than from the deploy's RNG state or the
  signer; `the_published_keys_are_constants` asserts that the three are distinct and hardcodable, and it
  is why the constants above are the same under either key.

## Ceiling of this arrangement, and what a public network needs

### The handshake, end to end (the open item is closed)

On a fresh chain, with the constants above: the resolve facet answers, the name layer answers `GetMe` with
the feature's channel, `getMe` runs for the calling deployer, and it **answers** — creating the member
(inbox + dictionary) on the way if this is the deployer's first call, and writing
`@[*deployerId, "inbox"]` / `@[*deployerId, "dictionary"]` for the key that deployed the feature. That
is the whole of the wallet's first governance step, and it needs no bootstrap script.

There was an open item here — recorded as "stops inside the feature's own `createMe`" — and it was
**wrong about where it stopped**. `getMe` died one line earlier, at
`if (everyone.contains(you) == false)` (`MemberDirectory.rho:78`): the port normalized an `if`'s
condition against the `par` that precedes it, so any `if` that was not the first term of its `par`
reduced to nothing, silently, and `createMe` was never called at all. The fix is one field in
`rholang/src/normalizer.rs::normalize_if`; no vendored `.rho` byte changed. **AUDIT C21** carries the
mechanism, the evidence and the (hard-fork class) consequence — three `if`s in this feature and a
handful more in the node's own `ListOps.rho`/`MultiSigRevVault.rho` were dead with it.

The diagnosis trap is worth keeping: `GetMe` is answered even when nothing was registered, because
`Directory.rho`'s `read` replies `*map.get(key)` = `Nil` for an absent key and a bare
`for (GetMe <- ch)` pattern matches `Nil`. Existence of a reply is not existence of a value — probe the
value, not the receive.

## Testnet vs mainnet

Genesis installing steps 2–4 above is a **testnet** convenience with a real cost, and a public network
must not do it:

- **The ceremony key holds `@[*deployerId, "MasterContractAdmin"]`** and the chain's only `GetMe`
  feature: the handle belongs to whoever ran genesis. That is a real, secret key and an
  identifiable operator — but it is still *one* key over every client's first governance call. A
  network that would rather each client run its own dictionary must install neither of steps 2–3; that is
  a genesis flag to land, not something this arrangement can express. What makes the shared model
  tolerable is verifiability: the class URIs are chain constants, so a client can check what the
  directory hands it against the table above instead of trusting the operator. **The parked capability
  is `write` (and the grant cap it mints); since #71 the ceremony key is no longer the *only* holder
  of a way to write** — any caller may take a one-key writer through the published grant cap, which is
  what makes the two open admission questions above worth answering before a public genesis.
- **A directory slot that was never filled answers `Nil`**, and a consumer cannot distinguish that
  from "broken" — so on mainnet a client must handle an absent class explicitly rather than wait.
- **Class URIs derived from the deploy RNG move when the blessed order changes** (`BLESSED_DEPENDENCIES`
  and the pinned sequence in `casper/src/genesis/mod.rs` are the guard). The *chosen* keys above do
  not move, which is why a client hardcodes them and not the registered URIs.
- **The blessed deploys are free** (`phlo_price 0`, `phlo_limit MAX`) and unbounded in reduce steps
  except by the genesis path's own limits; a production genesis should charge or bound them.
- **Per-deployer state stays runtime**: an inbox, a dictionary, a master directory for a *new* key —
  none of that is genesis content, and the wallet's `newInbox` creates it for its own key.

## Install order

Genesis installs the set **in one order, and only one of the constraints is sharp**:

| Constraint | Why | If violated | Caught by |
|---|---|---|---|
| `non_negative_number` → `make_mint` | `MakeMint.rho:27` looks the counter up (`lookup!(\`rho:lang:nonNegativeNumber\`, …)`) **during its own deploy**, and waits on a reply pattern a `Nil` reply cannot match | the deploy still *succeeds*, `MakeMint` never registers, and `lookup!(\`rho:rchain:makeMint\`)` answers `Nil` forever — silently | the genesis ceremony's completeness check (`missing_genesis_aliases`), pinned by `installing_make_mint_before_its_dependency_is_caught_by_the_genesis_check` |
| `directory`, `inbox` → `roll` | **not a genesis constraint** — `memberIdGovRev` resolves those imports per *call*, not at deploy time, so its position in the list is free (all three are genesis content, so a caller always finds them) | nothing at genesis; a client's `"makeFromURI"` would need them installed, which they are by the time anyone can call | — (a negative test for it is what established this; see `BLESSED_DEPENDENCIES`) |
| `kudos`, `issue`, `ertp` vs anything | independent: each self-registers and reads nothing at deploy time. `ertp` binds `rho:rchain:ertp:ledger`, which is a **native** system process (present on any chain, whatever genesis installs), not a registry lookup | — | — |

The order itself is pinned twice: `genesis::tests::blessed_terms_are_ordered_by_dependency` asserts
the dependency table against the returned list *and* the exact sequence (a change there is a genesis
change), and every entry's *usability* is asserted by the call probes on a fresh chain.

## Native system channels, aliased

The PoS and vault channels are native (`rholang/src/system_processes.rs::definitions`), so only the
*registry alias* was missing. Each resolves to `(9223372036854775807, bundle+{channel})` — the
`(nonce, value)` shape `rho:registry:insertSigned:secp256k1` stores, which is what consumers
destructure.

| Shorthand | Channel | Consumer it unblocks |
|---|---|---|
| `rho:rchain:revVault` | native, arity-1 + remainder | wallet `src/utils/rho.ts:7,18`; rgov `src/actions/transfer.rho:4`, `checkBalance.rho:10` — `lookup!` then `@(_, RevVault)` then the vault methods |
| `rho:rchain:pos` | native, arity-1 + remainder | wallet bonding `src/utils/rho.ts:29` — `lookup!` then `@(_, PoS)` then `PoS!("bond", …)` |

## Excluded, with reasons

Nothing here is excluded for being hard; each is excluded because no consumer reaches it, or because
its source contradicts the port's native design.

| Source | Why not |
|---|---|
| `Registry.rho` | Its bootstrap handshake sends **one** item on `rho:registry:lookup` (`Registry.rho:393`), but this port's native definition is **arity 2** (`system_processes.rs:463-469`), so the handshake cannot match and the registry would stay silently empty. It would also install the interpreted `TreeHashMap` trie that the port replaced with native state (`spec/RUST-FIRST.md`). The aliases are seeded natively instead. |
| `AuthKey.rho` | Registered *by* `Registry.rho`; needed only by the interpreted vault sources. **No consumer in either checkout reads `rho:rchain:authKey`** — the wallet and rgov reach auth keys through `rho:rchain:revVault` `deployerAuthKey`/`unforgeableAuthKey` method calls. |
| `Either.rho` | Zero demand: no occurrence of `rho:lang:either`, `Left`/`Right` in either consumer. Its only in-repo consumer would be `RevVault.rho`, which is not installed (native). |
| `RevVault.rho`, `Pos.rhox` | The vault and PoS **system** contracts are native here (`rholang/src/native_state.rs` + `system_deploy::NativeSystemDeployOp`), and installing the interpreted equivalents would shadow consensus-critical logic. Their sources additionally read `rho:lang:treeHashMap` and `rho:registry:systemContractManager`/`rho:rchain:configPublicKeyCheck`, none of which exist in this port. **`MultiSigRevVault.rho` was in this row and is no longer** — it is installed, adapted, for the reason `AuthKey.rho` is: see row 5 above and the corrected row below. |
| `rho:lang:treeHashMap`, `rho:rchain:configPublicKeyCheck`, `rho:registry:systemContractManager` | Provided only by the interpreted `Registry.rho` (see above); no consumer uses them. |
| ~~`rho:rchain:multiSigRevVault`~~ | **Stale, corrected 2026-09-27.** This row said the urn was not seeded because no consumer looks it up. It **is** seeded now, and to the *contract* rather than the native channel: installing the multi-signature vault made `lookup!` the only path that answers, so `GENESIS_ALIASES` maps the shorthand to `GenesisAliasSource::Contract` (see "What genesis installs" row 5). The native fixed channel still refuses by direct binding, and its refusal names the lookup path — `spec/API-SCHEMA.md`'s row carries the API half. |

## PoS liveness parameters (RCHIP #55, Law 89)

Five genesis parameters arm liveness enforcement. All are `0`/`none` in the shipped default, which is
byte-for-byte the pre-change `PosParams` format-2 record and writes no new native leaf. A network that
arms them starts a **new genesis**; arming an existing chain without one is a separate consensus change.

| key | meaning | default |
|---|---|---|
| `liveness-absence-threshold` | heights behind the quorum-supported tip that count as absent | `0` (off) |
| `liveness-jail-after-boundaries` | consecutive advancing boundaries absent before jail; at least `2` when enabled | `0` |
| `liveness-jail-epochs` | advancing boundaries a jail lasts | `0` |
| `liveness-vote-window` | advancing boundaries a slash vote stays open; must fit inside `quarantine-length` | `0` |
| `liveness-slash-severity` | `none` / `malicious` / `misdemeanour` / `honest-mistake` | `none` |

`PosParams::validate` refuses a half-configured rule at genesis and at decode. See
[RCHIP #55](rchip-55-liveness-enforcement.md) for the mechanism and its residuals.

### Test-only and example sources (explicitly not candidates)

`legacy/rholang/examples/**` (76 files — the `tut-*` teaching programs, `old/**`, `linking/**`,
`vault_demo/**`), `legacy/rholang/src/main/k/**` (28 K-framework test inputs),
`legacy/casper/src/test/resources/**` (23), `legacy/rholang/src/test/resources/**` (12),
`legacy/integration-tests/resources/**` (14), `legacy/rspace-bench/...` (3),
`legacy/casper/src/main/resources/**` (10 — the superseded Scala genesis sources),
`examples/**` (2), `qucalc/rholang/**` + `qucalc/examples/**` (12), `rspace-bench/benches/resources/**`
(3). None is embedded by production Rust; each is consumed only by a test, a benchmark, or the legacy
Scala tree. The only production `include_str!` of rholang is the ten files in
`casper/src/genesis/resources/`.

## The one adapted source

`MakeMint.rho` cannot install verbatim: its epilogue asks `rho:registry:systemContractManager` for a
write-only dispatcher and defines a `securityCheck` arm calling `rho:rchain:configPublicKeyCheck`.
Neither channel exists in this port, so the `for` waiting on them never fires, nothing registers, and
`lookup!(\`rho:rchain:makeMint\`)` would answer `Nil` forever. `standard_deploys.rs::make_mint_source`
registers the contract's own bundle instead and drops the unused arm; both markers are asserted, so a
drift in the vendored source fails the build rather than shipping an unadapted epilogue. No consumer
calls `securityCheck`, and the consumer path — `lookup!` → `(nonce, bundle)` → call it — is unchanged.

## What this changes for a consumer

- **The shorthands resolve.** `rho:rchain:revVault`, `rho:rchain:pos`, `rho:rchain:makeMint`,
  `rho:rchain:ertp`, `rho:lang:listOps` (and `rho:lang:nonNegativeNumber`) answer their lookup with
  `(9223372036854775807, bundle+{dispatcher})`, on a fresh chain, with no bootstrap deploy.
- **`rho:rchain:ertp` is the ERTP object API, not the ledger.** The native issuer ledger stays at
  `rho:rchain:ertp:ledger`; a caller that wants `makeIssuerKit` looks the shorthand up
  (`spec/API-SCHEMA.md` has both rows). `ERTP.rho`'s own key is *derived*
  (`blake2b256("rnode/genesis/ertp")`) rather than pasted from a vendored header, so anyone can
  recompute it — `standard_deploys::tests::the_ertp_key_is_derived_from_a_named_string` does.
- **The `rho:id`s above are constant** for every chain of this port: hardcoding them is safe (and
  means a `down && up` no longer invalidates them for this set). Anything registered *by a deploy*
  still shifts per chain — `rho:registry:insertArbitrary` derives its URI from a random seed
  (`system_processes.rs:1338`), which is exactly why the rgov contracts need `insertSigned` to become
  genesis content (see below).
- **`down && up` still regenerates the genesis address**, so a *recorded* master URI from a deployed
  bootstrap goes stale. With the blessed set installed, this no longer applies to the node's own
  contracts.

## Not in scope here

The rgov **governance** contracts (`Kudos`, `Inbox`, `Directory`, `memberIdGovRev`, `Issue`, and the
master directory) live in `rhovision-org/rgov`, not in this repository, and are not vendored by
this change. Making them genesis content is a separate sourcing decision with its own requirements —
fixed keys/timestamps, `insertSigned` instead of `insertArbitrary` so their URIs stop shifting per
chain, and the dependency markers substituted with those constants.

## The vault handle's dispatch id moved (2026-10-06)

**A consensus-visible change, and it is deliberate.** A vault handle minted by `findOrCreate` used to
carry **two** installed continuations — `balance` at arity 2 and `transfer` at arity 5 — and the space
keeps one per channel, so the second install replaced the first and `balance` answered nothing at all
(AUDIT C219). The handle is now **one** continuation at `arity: 1, remainder: true` dispatching on the
method, which is the shape `rev_vault`, `ertp` and `pos` already use.

The continuation's `body_ref` is `native_body_ref(name, arity)` — a hash of the name *and* the arity —
so a handle minted under the new rule carries a **different** `body_ref` than the old rule would have
given it. Nothing in this tree's genesis content changes (the handle is minted by a deploy, not by the
genesis), and no *existing* state is rewritten: what moves is what a **newly minted** handle's
post-state carries. Chain state produced before this commit stays valid — the RSpace under a handle is
whatever it was — and the wire is unchanged: a 5-field `transfer` send matches the new pattern exactly
as it matched the old one, which `casper/tests/determinism.rs` asserts as a fund *movement* rather
than a reply.
