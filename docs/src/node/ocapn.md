# OCapN interoperability

**Technical reference** for the node side: listeners, wire shape, code map and limits. The client
how-to is [Talking to a node from another implementation](../developer/ocapn.md).

[OCapN](https://ocapn.org/) (the Object Capability Network, *CapTP*) is the capability transport
Agoric's stack speaks. It lets a foreign peer — an Agoric vat, an `@endo/ocapn` client, any
implementation of the protocol — open a session to a node and hold **live references** to objects on
it. A reference to a Rholang capability is a real remote object: the peer calls it, and the call is
answered from the chain.

This is a *peer* protocol, not a client API. A peer does not post deploys and poll; it holds a
capability and invokes it. What makes that useful here is that the capability can name anything on the
chain — an ERTP issuer ([ERTP](ertp.md)), the REV vault, a contract of your own.

The crate is `ocapn/` (package `rchain-ocapn`). Sessions, wire bytes and answer bookkeeping are
node-local; the only thing that reaches consensus is the signed deploy the bridge produces.

## Turning it on

The node binds **no OCapN listener unless one is configured**. There are four, one key each —
`api-server.ocapn-listen` (the `tcp-testing-only` transport), `ocapn-listen-unix`, `ocapn-listen-noise`
and `ocapn-listen-websocket` — and each is off until it names a bind address. They are independent: a
node may listen on any subset, and a node listening on none is a dial-only node (its dial route says so
rather than dialling into "this node speaks nothing").

| Config key | CLI flag | Meaning |
|---|---|---|
| `api-server.ocapn-listen` | `--ocapn-listen` | `host:port` to bind for `tcp-testing-only`. Unset: no TCP listener. |
| `api-server.ocapn-listen-unix` | `--ocapn-listen-unix` | Socket path to bind for `unix`. Unset: no UDS listener. |
| `api-server.ocapn-listen-noise` | `--ocapn-listen-noise` | `host:port` to bind for `noise`. Unset: no Noise listener. |
| `api-server.ocapn-listen-websocket` | `--ocapn-listen-websocket` | `host:port` to bind for `websocket`. Unset: no WebSocket listener. |
| `api-server.ocapn-identity-key` | `--ocapn-identity-key` | Where this node's OCapN identity is kept; created on first use. Required by the two keys above. |
| `api-server.ocapn-advertised-host` | — | The host peers should dial this node at, when the bind address is not one they can reach. **Required** by a listener bound to `0.0.0.0` or `::`. |
| `api-server.ocapn-deny-local-dial` | — | Refuse to dial loopback and private addresses a peer names. Off by default. |
| `api-server.enable-ocapn-dial` | — | Mount `POST /api/v1/ocapn/dial` on the admin server. Off by default. |

The netlayers implemented are the OCapN project's `tcp-testing-only` — plain TCP, **no encryption and
no authentication**, which the project's own README flags "HIGHLY INSECURE — DO NOT USE IN
PRODUCTION" — `unix`, a Unix domain socket whose authentication is the socket's file mode (`0600`),
and `noise`, which is **the one a peer not on this host can use**: an authenticated, encrypted
handshake with no certificate authority and no daemon. Any of them may be bound, on separate keys:

```hocon
api-server {
  ocapn-listen = "127.0.0.1:22045"            # tcp-testing-only
  ocapn-listen-unix = "/run/rnode/ocapn.sock" # unix
  ocapn-listen-noise = "0.0.0.0:22046"        # noise
  ocapn-listen-websocket = "0.0.0.0:22047"    # websocket
  ocapn-identity-key = "/var/lib/rnode/ocapn-identity.key"
  ocapn-advertised-host = "node.example"      # what peers are told to dial
}
```

**A bind address is not the same as an address a peer can dial**, and `0.0.0.0` is
the case that catches people: it means *every* address on this host, so a node that advertises it hands
out locations where the peer dials **itself**. Same-host dialling happens to work, which makes the
mistake invisible until a peer somewhere else tries to follow a sturdyref or accept a handoff. So a
listener bound to `0.0.0.0` or `::` **must** be told what to advertise, with
`api-server.ocapn-advertised-host`, and the node refuses to start without it rather than handing out
locations nobody can use. The port is always the bound one — `:0` is legal, and the chosen port is the
only useful thing to advertise. For a listener bound to a specific address the key is not *needed* —
that address is its own answer — but **setting it still wins**: a set `ocapn-advertised-host` is
returned whatever the bind, so it overrides a specific address rather than complementing it.

**`websocket` is the transport `@endo/ocapn` speaks**, so it is the one a published Agoric peer can be
pointed at — and it is **weaker than `noise`**: as the reference writes it, `ws://` carries no TLS and
its in-band handshake has only the *server* prove its identity. Reach for `noise` unless the peer
speaks nothing else. It shares the identity file with `noise`: the node signs a challenge with the same
Ed25519 key the Noise handshake names it by.

A node that binds **none** has no transport at all: it does not listen, and the dial route answers
**503** rather than dialing into "this node speaks nothing".

**`noise` is the transport to reach for, and it is the only one of the four that is both reachable
and authenticated.** The handshake is Noise `XX` with X25519, ChaCha20Poly1305 and BLAKE2s; each side
proves it holds an Ed25519 key by signing its own X25519 static public key, and the node's name *is*
that Ed25519 key — a dialler must know it in advance, because the SYN is prefixed with it and a
responder refuses a handshake naming another node before doing any cryptography. It is verified
against Agoric's own implementation (`spec/audit/evidence/ocapn-noise/`). **Keep the identity file**:
a node that loses it comes back under a new name, and peers holding the old one cannot find it.

**One file is one peer, and the name cannot be rotated**. The designator *is* the
Ed25519 key in that file, so two nodes sharing it are **one peer** to everyone else: the crossed-hello
rule compares sessions by that name, so their sessions evict each other and a sturdyref to one resolves
at the other. And there is no rotation — replacing the key, or deleting the file and letting the node
regenerate one, renames the node, which is the same breakage as losing it.

**The node's name is a function of configuration**. Without an identity it derives
from the deployer key; with one it *is* the Ed25519 key. So adding `api-server.ocapn-listen-noise` or
`ocapn-listen-websocket` to a running node — or adding `ocapn-identity-key` — **renames it**, and every
location handed out under the old name stops resolving. Decide the name before peers hold it. (This
also means a key file with no listener does not change the name: the node reads and validates it, and
still does not adopt it.)

`websocket` uses the same identity, and is the transport `@endo/ocapn` speaks — but as the reference
writes it, `ws://` carries no TLS and only the server proves itself, so `noise` is the one to reach
for.

**The node can also start a session of its own.** With `enable-ocapn-dial = true`, `POST
/api/v1/ocapn/dial` on the admin server makes it dial a peer the request names and fetch the object at
the swiss number it gives:

```json
{ "designator": "peer", "transport": "unix",
  "hints": { "path": "/run/peer.sock" }, "swiss": "3c0f…" }
```

It is off by default and rate limited, and it is served on the admin listener — loopback unless
`enable-devnet-admin-public` — because it makes the node act, on a caller's word, against a peer of
that caller's choosing. **A dial the node starts carries no peer origin**, so Law 62's origin rule has
nothing to judge; the guard is the target policy (`ocapn-deny-local-dial`) alone, plus that gate.

**A peer that completes a handshake can do two things, and both are the node's own authority:**

- **Make the node submit a signed deploy.** Every call to a chain-backed capability becomes a deploy
  signed by the node's deployer key. It spends the node's REV, and the chain records the *node* as the
  caller. A peer can therefore reach every arm that trusts `rho:rchain:deployerId`, with the node's
  identity.
- **Make the node dial an address of the peer's choosing.** A sturdyref and a handoff give both carry
  a peer-chosen location, and the node connects to it.

Bind loopback unless the interop genuinely needs otherwise. `ocapn-deny-local-dial = true` closes the
second one for a listener whose peers are not on the same host; link-local (`169.254.0.0/16`,
`fe80::/10`) and the unspecified address are refused whatever this is set to, and a hostname is
resolved and judged by what it resolves to before any connection is attempted.

## What a peer can reach

A peer dials, the two sides handshake, and the peer is given the node's **bootstrap object** at export
0. `fetch(swiss)` on it resolves a swiss number to a capability. The node publishes:

| Swiss number | Object |
|---|---|
| `rho:rchain:revVault/getBalance` | the REV vault balance capability. **The address is an argument the peer supplies**, not the node's own: the deliverable takes one REV address and answers its balance |
| `rho:rchain:ertp` | the ERTP object API — `makeIssuerKit` and `getRevIssuer` |
| the OCapN conformance fixtures | the test suite's objects: echo, the car factory, the promise resolver, the greeter, the sturdyref enlivener |

The first two are published **only when the node has a deployer key to sign with**
(`dev.deployer-private-key`). Without one the bridge cannot reach the chain, and the bootstrap carries
the fixtures alone.

## The bridge

A delivery to a chain-backed capability becomes a **deploy**. The node builds a Rholang term that looks
the capability up by its registry name and calls the method the delivery named, signs it with the
node's key, submits it, and answers the CapTP promise with whatever the call wrote to the deploy's
reply channel.

Four consequences a client feels:

- **Every call costs a block.** The reply arrives when a block carrying the deploy is produced. On a
  node with autopropose off, nothing answers until something proposes.
- **Capabilities cross as descriptors, never as data.** A reply that *returns* an object — a brand, a
  mint, an issuer — arrives at the peer as a CapTP import it can call. The underlying Rholang name
  never leaves the node. A capability can also be passed *as an argument* to an arm that takes one.
- **The node is the caller on chain.** The deploy is signed by the node's deployer key, so the node's
  vault pays the phlo and the chain records the node as `deployerId` for every peer. There is no
  binding between a CapTP session and a deployer key, so the chain cannot tell one peer from another.
- **Bridged deploys are rate-limited** to 4 per second **across every session and capability** — each
  one spends the node's REV, so the total is on the node's spend. A second bound sits *under* that
  total: one capability may take at most 1 per second. The total alone is unfair rather than
  unsafe — the one object a peer loops on can spend the whole allowance while every other capability
  the node exports is refused — and a limiter per capability *instead of* the total would be a bound of
  N times the number, which is the resource-amplification class the row itself is filed under.

## Interop notes

These are the places where the OCapN prose and the reference implementations disagree, and where the
port had to choose. They matter to anyone integrating a new peer.

- **`op:start-session` carries four fields**, not the five the CapTP draft lists: the draft names a
  `crypto-version` that appears nowhere on the wire. The port follows the reference implementations.
- **Messages are netstring-framed.** The netlayer is described as "pure Syrup, no length prefix", but
  every implementation tested — the Python suite and `@endo/ocapn` — wraps each message as
  `<length>:<payload>`. Bare Syrup interoperates with nothing.
- **The swiss number is a byte array to the suite and a string to Endo.** The two reference
  implementations disagree, so the bootstrap accepts either and keys its directory by bytes.
- **A tuple crosses as OCapN's tagged value.** A Syrup record is *labelled* and a Rholang tuple has no
  label, so `(true, 0)` crosses as `<desc:tagged 'rho:tuple' [true, 0]>` — the passable union's own
  extension point — and comes back a tuple. A bare list would not do: a list from the peer does not
  match a contract's `(brand, value)` pattern, which is what had kept the ERTP arms that take an amount
  out of reach (see [ERTP](ertp.md); the wire-shape decision is Law 59).
- **Struct members are ordered by their encoded key bytes**, not by the key string — the reference's
  own sort. The session signature covers a struct, so this is load-bearing.
- The session Public Identifier is two SHA-256 rounds over the session public key; the Session ID is
  `SHA256(SHA256("prot0" ‖ sorted(PI_a, PI_b)))`. Session keys are **Ed25519**, ephemeral and
  off-chain; Ed25519 stays disabled as an on-chain signature algorithm.

## The code

| Module | What it holds |
|---|---|
| `syrup.rs`, `netstring.rs` | the Syrup codec and the length-prefixed framing |
| `locator.rs`, `peer.rs` | the URI and in-band locator forms |
| `session.rs`, `session_id.rs` | `op:start-session`, Public Identifier, Session ID, `op:abort` |
| `netlayer.rs`, `framed.rs`, `tcp_testing_only.rs`, `unix.rs`, `noise.rs`, `websocket.rs` | the netlayer trait, the shared framing, and the four transports |
| `multi.rs` | the dialing dispatcher: a locator's transport name picks the layer |
| `captp.rs`, `conn.rs` | the import/export and answer tables, `op:deliver`, `op:listen`, GC |
| `bootstrap.rs`, `fixtures.rs` | the bootstrap object and the conformance fixtures |
| `owner.rs`, `proxy.rs` | session ownership (a handle and the loop that owns the socket) and cross-session forwarding |
| `handoff.rs`, `enliven.rs` | third-party handoffs and the sturdyref enlivener that dials out |
| `capacity.rs`, `dial_policy.rs` | the peer-facing tables' caps and the dial policy |
| `par_value.rs` | the Rholang `Par` ↔ Syrup translation |

`node/src/api/ocapn.rs` is the node's end: the listener, the chain-backed capability, and the bridge.
`casper/src/shard_invoke.rs` builds the deploy terms.

The wire encodings are pinned by known-answer tests in `ocapn/tests/reference_vectors.rs` against
vectors produced by the reference suite's own encoder, because a codec that round-trips but orders
itself differently from a peer passes every local test and fails every handshake.

## Limits

- **The transports are `tcp-testing-only`, `unix`, `noise` and `websocket`.** `tcp-testing-only` is
  the OCapN project's own test transport, unauthenticated by design; `unix` authenticates by the
  socket's file mode but is local to the host; `noise` is the one a remote peer should use, and its
  handshake is verified against Agoric's implementation; `websocket` is the transport `@endo/ocapn`
  speaks, and is the weaker of the two networked ones — `ws://` has no TLS and only the server
  authenticates. **`onion` (Tor) is not built** — it is the only concrete transport in the OCapN draft,
  and it needs a `tor` daemon on the node — and any other netlayer (libp2p, IBC) implements the same
  two-function trait; nothing above it changes.
- **A bridged call's reply is written to the permanent registry**, because a Rholang value returned to
  a peer has no source literal and must be registered to be reachable. Nothing deletes those entries.
- **The node cannot yet name the peer on chain.** Until a session is bound to a deployer key, binding
  the listener publishes the node's own authority, not identified callers.

## See also

- [Talking to a node from another implementation](../developer/ocapn.md) — the client-side how-to.
- [ERTP](ertp.md) — the object API most peers want to reach.
- [Cross-shard invoke](shard-invoke.md) — the caller-signed deploy the bridge is built on.
