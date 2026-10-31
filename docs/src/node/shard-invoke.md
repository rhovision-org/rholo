# Cross-shard invoke is a remote deploy

> **Decision record (Layer 1).** Cross-shard invoke is a normal, caller-signed deploy submitted to
> the target shard: the primitive needs **no relay, no link table, and no new consensus**. It
> supersedes the "one powerbox process + a link table" framing that preceded it; the history is in
> [Incident history](incident-history.md).

The design companion `CapabilityTransport.md` splits cross-shard coordination into
two layers. This page covers **Layer 1 — the linked invoke**. The delegation layer
(revocable proxies, promise pipelining, gateway routing, OCapN) is Layer 2, tracked
separately outside this repository, and is out of scope here.

## The insight: identity is already the same

A `deployerId` is the caller's secp256k1 public key, and it is **shard-independent**.
The REV address derived from the same key is the same account on every shard. So
invoking a capability on another shard is not a capability-*transport* problem:
nothing has to be delegated, wrapped, or handed off. It is **signed message-passing
where the far shard binds your own `deployerId`** — which is exactly what a deploy
already is.

A deploy is `{ data, deployer, sig, sigAlgorithm }`, where `deployer` is the public
key and `sig` is that key's signature over the canonical `DeployData`. The far node
verifies the signature and binds `rho:rchain:deployerId` to it for the duration of
the reduction (`NormalizerEnv`). A `deployerId`-gated capability on the far shard
therefore sees *exactly* the identity it would see if the caller had deployed there.

There is nothing to relay: the caller already signs the message that carries the
identity. A relay would have to re-sign a *different* payload as the caller — which
the home node cannot do, because it holds only the caller's public key. Any design
that has the home node re-sign is therefore either (a) wrong (the far contract sees
the home node), or (b) a pre-signed envelope, i.e. a relay — and both are unnecessary.

## `rho:shard:invoke` — the expansion

The call site is

```rholang
$at(shard, `rho:id:theOracle`)!("getPrice", "ETH", *ret)
```

For a **link** (Layer 1) this expands, at the client, to a normal signed deploy
whose term is run *on the target shard*:

```rholang
new lookup(`rho:registry:lookup`), cap in {
  lookup!(`rho:id:theOracle`, *cap) |
  for (@(_, target) <- cap) {
    @target!("getPrice", "ETH", `rho:rchain:deployId`)
  }
}
```

- `lookup` resolves the registry URI and replies with `(uri, value)` (or `Nil`).
  A miss returns `Nil`, the `for` does not fire, and the deploy produces nothing —
  surfaced as a `shard-error` (below), **never a hang**.
- `target` is the capability name stored at the URI, invoked with the `@target!`
  deref idiom (the same pattern the genesis `AuthKey.rho` uses to invoke a looked-up
  contract). Only someone who was *handed* the URI can name it at all; the registry
  does not make a capability public.
- The reply channel is `` `rho:rchain:deployId` `` — the deploy's own signature,
  bound by the far node's normalizer environment (`NormalizerEnv`). The client
  listens on it (`listenForDataAtName`); the value produced there *is* the reply.

### The reply is data on the deploy's channel — listen, don't poll

`rho:shard:invoke(linkId, targetUri, method, args, *ret)` in the issue writes the
reply to a caller-local `*ret`. A caller-local name cannot cross shards: it lives
only in the home shard's tuple space, and the far deploy has no handle on it. The
only channel a far deploy can *always* reach is its own `rho:rchain:deployId`.
So:

- **far side:** the invoked capability sends its reply to `` `rho:rchain:deployId` ``;
- **caller side:** the client knows the deploy's signature (id), derives the reply
  channel from it (`reply_channel`), and **listens on that channel** —
  `listenForDataAtName` on the far node — resolving when the reply is produced.
  No `deployStatus` polling: the reply is a channel event, so the caller waits on
  the channel, exactly as a local `for` would.

The node's `listenForDataAtName` is a one-shot query over recent blocks (the same
shape as the Scala oracle, whose client waits with `listenAtNameUntilChanges`), so
the client re-listens on the *channel* until the reply commits. A streaming listen
is a possible transport improvement, not a change to this contract.

### Failure is a value

The listen resolves on the channel's value, or times out to a value — it never
hangs forever:

| reply channel | caller sees |
|---|---|
| data produced on `` `rho:rchain:deployId` `` | `v` |
| no data before the caller's timeout | `("shard-error", "timed out …")` |
| the far node rejects the deploy / the listen errors | `("shard-error", reason)` |

`("shard-error", reason)` is an ordinary `(String, String)` tuple, so it composes
with local rholang. A rejected signature never reaches a block: it fails at the
deploy-admission boundary (the same boundary that already rejects a bad deploy).

### Phlo

Phlo is charged to the **caller's account on the far shard** — which is the same
account, keyed by the same public key, so it is the same REV vault the caller funds
locally. The client sets `phloLimit`/`phloPrice` and `shardId` on the remote deploy
exactly as for a local one.

## What happened to the link table

A link in the original framing was "a mutual registration holding each rnode's API
endpoint and the set of registry URIs that shard exposes to the link". Under the
remote-deploy model it collapses to two things the caller already has:

- **an endpoint** — the target node's deploy service (gRPC 40401, HTTP 40403);
- **the URI** — the capability handle, which only a party that was handed it
  possesses.

"Reachability, not authority" is preserved and simplified: the endpoint is
reachability, and the registry URI is the authority. There is no link state to
replicate, no grant list to keep consistent, and therefore **no new consensus**.
Revocation is whatever the far contract's own authorization says (plus, optionally,
not handing out the URI; a registry entry can be superseded through the normal
registry mechanism). There is no separate "link" to revoke.

## Honest consequence: no single home-shard closure

Routing the invoke *through* the home shard (the issue's original motivation) was
meant to let a remote call compose with local operations into one home-shard
reduction, which is what made a joint-ZFA-closure conservation check meaningful for
the exchange escrow. A remote deploy is a **separate transaction on the far shard**
and cannot participate in the home shard's reduction. Consequences:

- A cross-shard exchange is **client-orchestrated** and **not atomic across
  shards**: each side commits under its own deploy, and each escrow is
  independently conserved. The client orders the two deploys (e.g. remote
  `consume`, then local `deposit`) and a crash between them leaves the two sides
  out of balance until the client retries — which is why each escrow must be
  *individually* sound and idempotent.
- The joint closure is not established by the call graph. It is a **check** the
  contract runs on the two escrows' committed facts, using `rho:qucalc:verify`
  (already exposed by the node). It is a verification, not a transport guarantee.
- Achieving atomic, single-closure composition is Layer 2's job (a gateway peer
  that is a member of both shards, or promise pipelining). A node that *is* a
  member of several shards can now do it: see
  [cross-shard transactions](../formal/cross-shard-transactions.md) for the
  two-phase commit and `POST /api/v1/txn` for the node-side coordinator.
  Promise pipelining and relay to a shard the node does *not* validate remain
  out of scope. See [Exchange support](#exchange-support) and Layer 2 below.

This is the price of "no new consensus", and it is the right trade for Layer 1:
most uses need same-account signed reach, not cross-shard atomicity. `#32`'s
"keep rnode a minimal deterministic reducer" is preserved exactly.

## Exchange support

A bilateral exchange-rate escrow for fungible **non-REV** tokens: one escrow per
shard, pre-funded, with a rate fixed at deploy. It is a **contract on the
primitive**, not a new node feature — see
[`qucalc/examples/shard_exchange.rho`][shard_exchange] and
[the walkthrough](#a-bilateral-escrow-walkthrough).

[shard_exchange]: https://github.com/rhovision-org/rholo/blob/dev/qucalc/examples/shard_exchange.rho

The security is capability security, and it is the whole proof:

- the escrow exposes no method that pays the operator;
- it holds no capability that lets it (or its operator) drain it outside the fixed
  rate;
- the rate is fixed (or governed) at deploy, so there is no ambient authority to
  corrupt.

Each side runs only the half it owns and checks conservation against its own
ledger. When both legs have committed, `rho:qucalc:verify` re-checks the joint
closure of the two escrow capability URIs (the `coupled` reading from quantum-os
`crates/zfa-core/src/coupling.rs`).

No REV ever moves. Cross-shard REV movement is explicitly out of scope: a token
that wants to move across shards exposes a `transfer` method on its own contract
and the caller invokes *that* — nothing here burns or mints the platform token.

### A bilateral escrow walkthrough

Two shards, `A` and `B`, one escrow deployed on each with the same fixed rate,
each pre-funded with its local token. A client holds both escrow URIs.

Moving value A→B is two signed deploys from the client's key:

1. **remote consume on B** (target shard `B`):
   `$at(B, escrowB)!("consume", *deployerId, amountA, *rB)` — B's escrow checks the
   caller's `deployerId` against its configured counterparty, decrements B's
   balance by the rate-equivalent, and replies on B's deploy id.
2. **local deposit on A** (target shard `A`): the client deploys
   `escrowA!("deposit", *deployerId, amountA, *rA)` to A.

If step 2 fails, B's consume has still committed and A's escrow is short; the client
retries step 2 (deposit is idempotent per deploy id). The joint check
`rho:qucalc:verify([escrowA, escrowB])` reads the two committed facts. Because the
two legs are separate transactions, the escrow **must not** treat the joint check as
a precondition for a single atomic move; it is a *monitor* over the pair.

## The Rust primitive

Because only the keyholder can sign, the primitive is a **client-side** helper: it
builds the far-shard term, signs it with the caller's key, and listens on the reply
channel for the response. It lives at [`rchain_casper::shard_invoke`][module].

[module]: https://github.com/rhovision-org/rholo/blob/dev/casper/src/shard_invoke.rs

- `invoke_term(target_uri, method, args: &[Par]) -> String` — the expansion above, with
  `` `rho:rchain:deployId` `` as the reply channel.
- `signed_invoke(term, caller_key, timestamp, phlo_limit, phlo_price, valid_after_block_number,
  shard_id) -> Result<Signed<DeployData>, String>` — an ordinary deploy signed by the
  caller (`shard_id` must be the *target* shard's, or the far node rejects the
  `DeployData.shardId`); `deployerId` on the far shard is `caller_key`'s public key.
- `reply_channel(deploy_id) -> Par` — the reply channel, derived from the deploy id.
- `await_reply(service, deploy_id, listen_interval, timeout) -> ShardOutcome` —
  listens on that channel until the reply appears, mapping it (or a timeout) to
  `Value(Par)` / `Error(String)`; `into_value()` renders the latter as
  `("shard-error", reason)`.
- `reply_outcome(data: &[DataWithBlockInfo]) -> ShardOutcome` — the same mapping over
  an already-fetched listen result (the first value produced, else an error), and
  `shard_error(reason) -> Par` — the `("shard-error", reason)` tuple on its own, for a
  caller that already has a reason string.

There is no server-side `rho:shard:invoke` system process, and none is needed: the
node already accepts signed deploys (`rnode --grpc-host <target> -p 40401 deploy …`,
or the HTTP deploy endpoint on 40403). A thin-client command that performs the
expansion end-to-end is a follow-up, not a node change.

## Non-goals

- Cross-shard REV movement / burn / mint — never.
- Shared state or cross-shard consensus. A cross-shard call is a deploy.
- A relay, a signed-envelope wire protocol, or a replicated link table.
- Delegated / revocable proxies, promise pipelining, three-party handoff, gateway
  routing (relaying a deploy to a shard this node is **not** a member of),
  non-RChain chains — all Layer 2 (`quantum-os#173`), built over this. Serving
  several shards in one node *is* implemented (the gateway): the node has a
  membership list, routes each request to the shard that owns it, and
  coordinates a two-phase commit across its own shards. Relay stays out.
- Light state proofs, arbitrary-pre-state replay — dropped in `#32`.

## Related

- [Operating the node](operating.md) — deploy/propose ports.
- [Building applications on the local devnet](../developer/building-apps.md) — the
  client deploy path.
- `CapabilityTransport.md` (quantum-os) — Layer 1 vs Layer 2, the exchange
  contract, and the OCapN references.
- `quantum-os#173` — the delegation layer. `#32` — keep rnode minimal.
