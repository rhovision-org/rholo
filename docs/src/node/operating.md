# Operating the node: REPL and the Docker multi-node network

Two thin-client surfaces ship with the `rnode` binary: the interactive rholang **REPL** and the
scripted **Docker multi-node network**. This page is the operation guide for both.

---

## Shard memberships

A node validates one shard by default (`casper.shard-name = root` under `casper.parent-shard-id = /`,
giving the full id `/root`). A **gateway** node validates several — it produces and validates each
independently, routes every request to the shard that owns it, and can coordinate a cross-shard
transaction across its own shards ([cross-shard transactions](../formal/cross-shard-transactions.md)).

```hocon
casper {
  # One entry per membership; the FIRST is the primary shard (the default target for requests that
  # do not name a shard). Each entry may override `genesis-block-data` field by field.
  shards = [
    { shard-name = root, parent-shard-id = / }
    { shard-name = child, parent-shard-id = /root,
      genesis-block-data { wallets-file = /var/lib/rnode/genesis-child/wallets.txt } }
  ]
}
```

Notes for operators:

- **File-configured.** The command line cannot address array entries, so per-shard genesis data
  (bonds and wallets) is expressed in `rnode.conf`; `--shard-name`/`--parent-shard-id` still work and
  define the one-element default. Setting both the array and those scalar keys is an error.
- **Storage.** The primary membership keeps the data directory itself; each additional shard nests
  under `<data-dir>/shard/<shard-id segments>/`. A directory records the shard that owns it
  (`shard-id`), and a mismatch is a startup error rather than a node silently running the wrong
  chain — so keep a shard's position in the list stable.
- **Serving state.** `disable-state-exporter = true` makes this node refuse a peer's store-items
  (state-sync) request before it walks the trie — the operator's switch for who may pull this node's
  state (`NodeRunning.scala:314-320`; the port serves the request otherwise, bounded by a per-peer
  rate limit and a `take` cap). A node that refuses still *receives* state from others: the flag
  gates the serving half only.
- **Sync.** LFS sync is not shard-aware, so a multi-shard node must reach its shards from their own
  genesis (genesis master / `--standalone`) or from existing local state.
- **Cross-shard transactions.** Set `api-server.enable-txn-api = true` on a gateway to serve
  `POST /api/v1/txn` (plus `GET /api/v1/txn` and `/api/v1/txn/{txnId}`). A single-shard node, or one
  without a validator key, answers 404. The legs are ordinary deploys, so they take effect when a
  block includes them — enable `--propose-on-deploy` or `--autopropose`, or the transaction will
  time out waiting.
  **These routes are on the *admin* HTTP server (port 40405), not the public one (40403)** — the
  coordinator signs every leg with this node's validator key and escrows out of **that key's own REV
  account**, so the surface is privileged in the same way `/api/propose` is, and it is deliberately not
  reachable on the API port a remote client can reach. The admin server binds
  `127.0.0.1` unless `api-server.enable-devnet-admin-public = true`, so a wallet on another host needs
  that opt-in — and should weigh it: publishing the admin port publishes an unauthenticated
  spend-this-node's-REV route as well. A transaction may name at most 32 legs, and the coordinator
  ledger holds at most 10,000 records; past either bound a new transaction is refused rather than
  queued.
  **A leg's amount is not capped by this API** (register C348). It must be non-negative, and the
  escrow can only move what the coordinator key's own vault holds — but a caller who can reach the
  route can ask it to escrow that whole vault across shards. That is a deliberate boundary rather than
  a missing check: a sub-balance cap would be a node-local spending policy layered on the chain's, and
  the route is already behind three opt-ins (a multi-shard gateway with a signing key,
  `api-server.enable-txn-api`, and an admin listener published beyond loopback). **If you serve it to
  callers you do not trust, cap the coordinator key's balance, not the request.**
- `GET /api/v1/shards` lists the memberships, primary first, each with its own chain height. The
  membership is deliberately *not* folded into `/api/status`, whose `shardId` field existing tooling
  parses on its own.
- **There is a fourth kind of listener, and every one of them is off unless you ask for one.** The
  OCapN listeners are where a foreign peer — an Agoric vat, say — can hold live references
  to objects on this node: `api-server.ocapn-listen` is the conformance suite's plaintext
  `tcp-testing-only` port, `ocapn-listen-unix` a domain socket authenticated by its `0600` mode,
  `ocapn-listen-noise` the one a remote peer should use (authenticated *and* encrypted), and
  `ocapn-listen-websocket` the one `@endo/ocapn` speaks and the weakest of the networked two. They are
  **not** part of the API above, and when `dev.deployer-private-key` is set a peer that
  reaches one can make this node **submit deploys signed with that key** and cause it to **dial any
  address the peer names**. Read [OCapN interoperability](ocapn.md) before enabling it; the short
  version is that binding it
  publishes the node's own authority, so bind loopback unless every peer that can reach the address is
  one you trust.

---

## The REPL

`rnode repl` is a thin gRPC client: the interactive loop runs on your machine and forwards each line
to a running node, which parses, normalizes, and evaluates it and returns the rendered result. The
evaluation happens on the node — inside its **isolated `eval-*` store** — so REPL terms never touch
the node's live chain state (`rspace-*`).

### Start a node

```sh
# Local standalone node (creates genesis on first boot):
cargo run --release -p rchain-node --bin rnode -- run -s
```

The node serves **Deploy** on the external gRPC port **40401** and **Propose + Repl** on the internal
loopback port **40402** (propose/repl are not network-reachable — `spec/AUDIT.md` C1).

### Standalone genesis prerequisites

`run -s` makes the node the **genesis master**, and it needs two things the bare command above does
not set up for you:

1. **A validator key.** The genesis block must be signed, so provide a secp256k1 private key (32
   bytes, base16) as a **file**, through `--validator-private-key-path`:

   ```sh
   rnode run -s --validator-private-key-path /var/lib/rnode/validator.hex
   ```

   **There is no flag that takes the hex itself.** `--validator-private-key <hex>` existed and was
   removed because a secret in `argv` is readable by any local process through
   `/proc/<pid>/cmdline`, and hiding the flag from `--help` hides it from the operator, not from the
   host. A command line that still passes it is refused at parse time, naming this route.

   Without a key the node exits with `To create genesis block node must provide validator private key`.

   The file may hold either a
   64-character base16 scalar (surrounding whitespace ignored) or an unencrypted PKCS#8 `PRIVATE KEY`
   PEM, as produced by `openssl ecparam -name secp256k1 -genkey`:

   ```sh
   install -m 600 /dev/stdin /etc/rnode/validator.hex <<< "$KEY"
   rnode run -s --validator-private-key-path /etc/rnode/validator.hex
   ```

   The distinction matters: the hex flag puts the secret in the process argument list, where any local
   process can read it from `ps` or `/proc/<pid>/cmdline`, whereas only the path is exposed. If both are
   given the hex flag wins and the path is not read. A path that is set but unreadable, or that holds no
   secp256k1 key, is a startup error rather than a silent fallback — otherwise the node would run with
   no validator identity and never propose. *Encrypted* PEMs are not accepted here (they need a
   passphrase; `Secp256k1.parse_pem_file` covers that shape).

2. **A wallets file.** The genesis ceremony parses `~/.rnode/genesis/wallets.txt` *strictly*, so the
   file must exist — an empty file is fine. `bonds.txt` is auto-generated when absent, but to be
   bonded as a validator, provide a `<public_key> <stake>` line (public key = uncompressed 65-byte
   point, base16) matching your key:

   ```sh
   mkdir -p ~/.rnode/genesis
   touch ~/.rnode/genesis/wallets.txt
   echo "04c591a8ff19ac9c4e4e5793673b83123437e975285e7b442f4ee2654dffca5e2d2103ed494718c697ac9aebcfd19612e224db46661011863ed2fc54e71861e2a6 100" \
     > ~/.rnode/genesis/bonds.txt
   ```

   A missing wallets file exits with `FAILED PARSING WALLETS FILE: … No such file or directory`.

### Bind to loopback (localhost-only)

The API server builds its listen address with `SocketAddr::from_str`, so `--api-host` must be a
**literal IP**, not a hostname — `--api-host localhost` fails with `invalid socket address syntax`.
For a localhost-only node, use `127.0.0.1` and skip the UPnP probe and external-IP guessing:

```sh
rnode run -s \
  --validator-private-key-path /var/lib/rnode/validator.hex \
  --host 127.0.0.1 --api-host 127.0.0.1 --no-upnp
```

`--host 127.0.0.1` sets the advertised protocol address to loopback and also stops the node probing
external services to guess its public IP (otherwise it logs `guessing your external IP address…`);
`--no-upnp` disables the gateway probe. All node data lives under `~/.rnode` by default.

### Run the REPL

```sh
# In a second terminal, against the local node (defaults: localhost:40402):
cargo run --release -p rchain-node --bin rnode -- repl

# Against a remote node:
rnode --grpc-host <host> --grpc-port 40402 repl
```

The prompt is `rholang $ ` with line-editing, history, and tab-completion over the REPL keywords
(`stdout`, `stdoutack`, `stderr`, `stderrack`, `for`, `!!`). Each submitted term:

1. is parsed and normalized; a syntax error surfaces as `Error: …` without evaluating;
2. on success, the normalized term is echoed on the **node console** as an `Evaluating:` line;
3. is evaluated, and the result is printed:

```
Deployment cost: 33
Storage Contents:
@{Unforgeable(0x…)}!(0) |
for( … ) { Nil } | …
```

`:q` (or EOF/`Ctrl-D`) quits. A single evaluation error stops the loop — matching the Scala
thin-client REPL.

### Evaluate files

```sh
rnode eval file.rho [--print-unmatched-sends-only]
```

`eval` reads the files client-side, sends each whole program to the node, and prints a `Result for
<file>:` header per file.

### REPL internals (short)

| Piece | File |
|---|---|
| client loop (`:q`, coloring) | `node/src/runtime/repl_runtime.rs` |
| rustyline console (prompt, history, completion) | `node/src/effects/console_io.rs` |
| gRPC client | `node/src/effects/repl_client.rs` |
| server eval (`Evaluating:` echo + result) | `node/src/api/grpc/repl_grpc_service.rs` |
| isolated eval store | `node/src/runtime/node_runtime.rs` (`"eval"` prefix → `rspace/src/factory.rs`) |

`rho:io:stdout` / `rho:io:stderr` print on the **node** process (server side), matching the Scala
thin-client model — not on the REPL client.

---

## Deploying, and block production

### `rnode deploy` and `deploy-status`

Deploys reach a node through the **external gRPC** port (40401 by default) or `POST /api/deploy`
(40403). Three things bite operators:

- **The shard must be named.** The client sends an empty shard id unless told otherwise, and a node on the
  `root` shard rejects it outright:

  ```
  Deploy shardId '' is not a member of this node's shards: [/root]
  ```

  Pass `--shard-id /root` (or whatever `/api/status` reports as `shardId`).

- **A CLI deploy that does not set an anchor is dropped.** The client leaves
  `valid_after_block_number` at `-1`, and a deploy is expired once
  `height - valid_after_block_number > DEPLOY_LIFESPAN` (50) — enforced by `dag::expire_deploys`, which
  deletes it from the pool, and again by the proposer. On a chain taller than ~49 blocks such a deploy is
  accepted (`Response: Success!` with a DeployId) and then never proposed:
  `deploy-status` answers `notProcessed / Unknown`, and the proposer logs
  `No pooled deploys; injecting dummy deploy for block #NNN`. Pass
  `--valid-after-block-number <current height>` on a binary that predates the anchor fix; the CLI
  resolves the anchor from the node's own height the way the faucet, the browser client and
  `txn_coordinator` do.
  Short chains hide this: below the lifespan, `-1` is not yet "expired".

- **`deploy-status` reports the outcome *and the reason*.** A success returns
  `{"ProcessedWithSuccess": {"deployResult": […], "block": …}}`; a failure returns
  `{"ProcessedWithError": {"deployError": "<the reducer's reason>", "block": …}}` — for phlo exhaustion,
  `"Computation ran out of phlogistons."`; a deploy still in the pool returns
  `{"NotProcessed": {"status": "Pooled"}}`; and a deploy the node has no record of returns
  `"Unknown"`. **The variant tags are PascalCase and the fields camelCase**, which is what law 43's
  envelope catalogue declares (`spec/conformance/envelope.tsv`) — a client switching on
  `processedWithError` matches nothing, and nothing errors.

  The reason comes from the block's recorded failure, so it is available on any node
  holding the block. The placeholder
  `"<deploy error message not available in cache or deploy executed on another node>"` survives for one
  case only: a deploy whose record carries no message at all (a block from a node that predates
  the fix), which is also the only case the Scala's per-node execution tracker could answer.

  To read a term's **return value** — usually what you actually want — have the term send its result
  somewhere it can be read back, using the registry result-slot pattern that the browser client and
  `scripts/qos-cli/rholang-client.mjs` use: `wrapProgram` binds `return` and inserts the value under a
  nonce through `rho:registry:insertSigned:secp256k1`, which can then be looked up. A raw deploy or eval
  does **not** get `return` for free, so a term written for the browser fails as
  `Top level free variables are not allowed` when deployed directly.

### Block production modes

| mode | flags | behaviour |
|---|---|---|
| continuous | `--autopropose` | proposes on a fixed cadence. The devnet adds `--dev-mode --deployer-private-key` so an injected dummy `Nil` deploy keeps the pool non-empty |
| on demand | `--propose-on-deploy` | proposes when a deploy arrives |
| idle | neither | nothing is proposed; the DAG advances only via the admin API's `POST /api/v1/propose` or another node |
| attesting | *(default)* | a validator proposes an **empty attestation** when a remote block arrives, even with nothing of its own to include |

The last row is the one that matters for a *network*: a validator that holds no deploys still has to move
its latest message, or its stake never reaches the 2/3 finality threshold. Attestation is therefore **on
by default** (`--no-attest-on-new-blocks` turns it off) and a validator needs **no `--autopropose`** to be
live — including a validator on a small host. `--autopropose`
remains a dev/CI tool for continuous block production; its dummy-`Nil` injector is not a liveness
mechanism.

There is **no `--no-autopropose` flag** on the node — you omit `--autopropose`. (`tools/devnet.sh`
accepts `--no-autopropose` because that is *its* CLI, and it simply omits the node flag. Passing it to
`rnode run` exits 1 in a restart loop.)

### Admission, memory and the relay

A deploy with a free pool slot is accepted whatever the host's memory state. `degraded` is an
**advisory** word; `acceptsDeploys` is the pool's count/byte capacity. Only a real bound refuses:

- a full pool, a deployer's share or its rate: **429 + `Retry-After`** (HTTP) or
  `RESOURCE_EXHAUSTED` (gRPC);
- a malformed request, bad signature, wrong shard or expired anchor: **400**.

Explore has its own headroom permit budget and a **429** when it is spent; it is never gated by the
deploy pool. A **503** means the node cannot serve at all. The operator status
(`/api/v1/status/operator`, admin listener) carries the headroom, the critical/throttle/recovery
floors, the process RSS and the permit limits; `/metrics` carries the same as `rchain.resource.*`
gauges; the node's own `GET /health` is a readiness body (200 while serving) and carries
`finalityStalled`/`finalityStallEpisodes` as the peer-liveness signal.

**Readiness is not pool capacity.** `acceptsDeploys` remains the Law 86a pool answer; `/health` also
carries `ready`, `phase`, `nodePhase`, `shardPhases`, `pipelineFaults`, `orphanBodies`,
`orphanDetections`, and `proposalHalted`. **The phase is per shard**: `phase`/`nodePhase` are the
primary shard's label and `shardPhases` maps every member shard to its own. `ready` requires
**every** member shard Running (plus no pipeline fault, no isolation and no confirmed orphan);
a node with one serving and one syncing shard is not ready. Deploy admission is judged by the shard
the deploy names: `Syncing`/`Blocked` refuse it with a 503 even when the pool has room, while
`Starting` may pool it (the node is seconds from deciding, and `/health` is still not ready there).
A block-pipeline fault sets `ready: false`; a validator beyond the finality bound sets
`proposalHalted` and stops proposing **and** attesting. The operator action is
`POST /api/v1/proposer/resume` on the admin listener; the next proposal re-checks the bound, so a
node that is still isolated latches again.

**What launch does before it says Running.** A shard moves `Starting` → `Syncing` (if it restores
state) → the **start-up disposition audit** → `Running`. The audit runs a one-time **keys-only**
scan for bodies written before the ledger (the `legacy-body-scan` marker in the shard's durable
records; it adopts orphan hashes as `Pending` and decodes one body at a time), re-offers every
unsettled row, and waits a **bounded 30 s** for the rows it actually offered. A row whose
justification is not in the DAG was never offered, stays `Pending` and visible, and does not hold
boot; an audit error is logged and boot proceeds. The marker is written only after the scan, so a
crash rescans. `Running` therefore means "the audit was attempted and its window closed", not "the
ledger is empty".

**A block the node cannot settle is durable and retried.** Each stored body has a disposition row
while the node owes it an outcome; a fault (an internal replay error, an insert error, a validation
past its 10-minute deadline) is recorded with its stage and attempt count and re-offered on a
doubling backoff (30 s to 10 min, eight attempts) before it is quarantined. `pipelineFaults` is the
process counter and does not fall when a retry succeeds; a restart is the clean reset for a fault
whose cause is gone. A quarantined row and an unresolved orphan are both visible on `/health` and
both withhold `ready`. `orphanBodies` is **zero until the periodic audit has seen the same
block-store/DAG gap for three consecutive samples**, so an in-flight block does not flap readiness.
A sustained non-zero value means a body the DAG does not account for. A body written by a build
before the ledger is adopted by the start-up scan on the next start; a body whose bytes cannot be
decoded is logged by hash and remains this count audit's signal, so the recovery is an operator
reset or a peer re-delivery.

**The body and its row are one write.** They live in the same LMDB environment and are committed in
one transaction, so a crash cannot leave a stored body with no row — the shape that needed the
start-up scan in the first place. The ledger moved into the block-storage environment in the build
that carries the October 2026 remediation, which bumped the store-format marker: an **upgrade** from
an earlier build re-adopts the ledger with the start-up scan on first boot (a `Faulted` row on an
unsettled body comes back as `Pending`, so the block is retried rather than dropped), and a
**downgrade** to an earlier build is refused rather than run over a store it cannot read.

**The same build changes the gas charges, so its genesis post-state hash differs from the previous
build's.** A network upgrading must regenerate any pinned `--expected-genesis-hash` and move together
(validators and observers alike): a pre-change block containing a collection conversion or a `censure`
does not replay under the new charges, and a mixed network would refuse it.

| Switch | Default | Effect |
|---|---|---|
| `RNODE_DEPLOY_RELAY` | off | offer each admitted deploy to connected peers' external deploy service (`_PORT`, default 40401; `_FANOUT` to cap) |
| `RNODE_CROSS_DEPLOY_BATCHING` | off | execute independent deploys on forked runtimes and merge them (`_VERIFY` on, `_MAX` default 4) |
| `RNODE_HTTP_MAX_INFLIGHT_DEPLOY` / `_EXPLORE` | off | operator concurrency caps, 1–1024 |

The batch and relay are opt-in because both are new paths: the batch re-executes a block
sequentially (`RNODE_CROSS_DEPLOY_BATCH_VERIFY=1`) and falls back on any mismatch; the relay assumes
the peers expose their external deploy port. Neither changes the block format or the consensus rules.

### Finality, and why an idle chain can report no fringe

Finality requires **more than 2/3 of the whole bond pool** to attest, *and* someone to propose. Two
consequences operators run into:

- If the only proposer holds ≤ 2/3 of the pool, blocks keep extending the DAG but nothing finalises:
  `GET /api/last-finalized-block` answers `"Finalized fringe is not available."`, and a health check that
  treats a missing fringe as a failure will report an idle chain as unhealthy. Joining nodes can still
  sync — they restore from the **approved** fringe, persisted at the genesis ceremony by
  `put_approved_block` — but a node needing a *later* fringe to catch up quickly has none available.
- `withdraw` does **not** remove stake from the pool at once, and does not deactivate the validator
  either: the request only stages a deadline (`pendingWithdrawers`), so the validator keeps validating,
  keeps earning, and its stake keeps counting against the 2/3 threshold until the next epoch boundary.
  The boundary moves the stake out of the pool into an escrowed claim, and the claim is paid once its
  quarantine has elapsed — at a later boundary (`rholang/src/native_state.rs`, `close_block`; the
  boundary is `blockNumber % epochLength == 0`).

A net that must keep finalising while otherwise idle should either give its founding validator more than
2/3 of the pool as a genesis stake, or run a second validator — attestation is on by default, so it adds
its attestations without `--autopropose` (and without a deployer key). [The setup
guide](validator-observer-setup.md) works both through.

---

## Reading a PoS position, including a delegated one

Two reads, and the difference between them matters:

- **`GET /api/v1/pos`** — one shard's state in one object: the epoch, the boundary countdown, the
  active set, and the pending withdrawals with their countdowns. Nothing about delegation, deliberately:
  the delegation ledger is unbounded in how many people have delegated, and hanging it here would make a
  single status call's size a function of that.
- **`GET /api/v1/pos/delegations?delegator=<65-byte hex>`** — **one delegator's positions**, across
  every operator it has staked with. Scoped to the key you ask about, which is the bounded direction.
  **On the admin listener (40405), not the public one**: it is a store query keyed on a
  caller-supplied key, not chain state a peer would read, and it answered any caller's question about
  any delegator's portfolio while it was public.

  ```sh
  curl -s "http://localhost:40405/api/v1/pos/delegations?delegator=04f700a4…" | jq
  ```

  Each entry is `{operator, amount, accruedRewards, pendingUndelegation}`:
  - `amount` is the principal still attributed to you on that operator's key. It is **not** the
    operator's bond — `getBonds` answers that, and it is the *aggregate* of every delegation to it, so
    it is attributable to no single delegator;
  - `accruedRewards` is what that delegation has earned and not yet been paid. It is held apart from the
    operator's own committed rewards, which is what makes it yours to see;
  - `pendingUndelegation` is `null`, or `{deadline, blocksRemaining}` when you have staged an exit.
    **`deadline` is a block height, not the height you requested at** — it already contains the
    quarantine — and `blocksRemaining` is the distance to it, floored at zero.
  A malformed key answers `400` rather than an empty list: an empty list is a *true* answer about a
  delegator with no positions, and a caller who mistyped its own key must not be told that.

The same two reads are reachable from rholang, for a contract rather than an operator:
`rho:rchain:pos!("getDelegations", delegatorKeyBytes, *ret)` replies a list of
`(operatorKey, amount, accruedRewards, pendingDeadlineOrNil)` tuples — see
[`spec/API-SCHEMA.md`](../../../spec/API-SCHEMA.md)'s `rho:rchain:pos` row. `examples/pos-delegations.rho`
is a runnable probe.

**What a delegator is exposed to** is [Validator economics](validator-economics.md)'s subject rather than
this page's, but the short form is: your principal sits inside the operator's bond, so it is at the same
slash tier as the operator's own stake, and the operator's participation scales the reward your share is
drawn from.


---

## Has a key signed a deploy? The deployer index

**`GET /api/v1/deployer/<key>`** answers whether a key has signed a deploy that is in a block. Once
it has, the key is public, which is what a wallet's quantum key-hygiene check needs to know (the
[post-quantum plan](../contributor/post-quantum-plan.md) §16.1). `<key>` is the hex `blake2b256` hash
of the 65-byte uncompressed public key, or the key itself. **A wallet should send the hash**: asking
about a key that has never signed by sending the key would hand it to the node (and its logs), which
is the exposure the check exists to avoid. The hash does not reveal the key, and it is not the REV
address either, so it cannot be computed from an address.

```sh
curl -s http://localhost:40403/api/v1/deployer/<64 hex chars> | jq
# {"block": {…LightBlockInfo…} | null, "indexedFromHeight": 0}
```

The answer is about **this node's blocks**: a `null` says no block it holds and has indexed carries a
deploy from the key, not that the key was never shown to anyone (a deploy sent elsewhere and never
included, or a signature made off-chain, is outside what any node can see).

- `block` is the first block this node inserted that carries a deploy from the key, or `null`.
- `indexedFromHeight` is the height the index reaches down to. `0` means every stored block is
  indexed, so `null` means "in no block this node holds". A node upgraded onto an existing chain
  starts above `0` and backfills the older blocks once, in the background (`deployer index backfilled
  from N stored blocks` in the log). Until then a `null` only covers the heights from there up.
  A node that joined by last-finalized-state sync never stored the blocks below the fringe, so it
  reports the lowest height it holds and stays above `0`; ask a node that has the whole chain for a
  full answer.
- On a multi-shard node the lookup searches every member shard. `block` is the first hit (the primary
  shard first), and `indexedFromHeight` is the highest of the members' values, so `0` still means
  "complete on every shard".
- Every block is indexed, including blocks that failed validation: a deploy in one still published
  the key.
- Anything other than 32 or 65 hex-encoded bytes answers `400`.

The index lives in the shard's `deployer-index` store: one entry per distinct signing key (a 32-byte
hash of the key → a 32-byte block hash) plus a height marker. It can never hold more entries than the
`deploy-index` store, which has one per deploy. Code: `casper/src/dag.rs` (`with_deployer_index`,
`backfill_deployer_index`, `lookup_by_deployer`).
---

## The Docker multi-node network (bare topology)

`tools/devnet.sh up --nodes N` boots a bare **1–5 node** network (one bootstrap + `N-1` unbonded peers)
with no autopropose and no deployer wallet — the network-*topology* mode for exercising sync/gossip,
driven manually via `cli`. (The same script's default mode is the contract *devnet* — see
[Local devnet](devnet.md).) The image is built from
[`docker/rnode/Dockerfile`](../../../docker/rnode/Dockerfile).

### Prereqs

- `docker` (a running daemon)
- `openssl` (to read the bootstrap node-id from its generated TLS cert)

### Commands

```sh
tools/devnet.sh build                       # build the rnode:local image
tools/devnet.sh up --nodes 3                # bootstrap + 2 peers (N in 1..5)
tools/devnet.sh status                      # docker ps for the network
tools/devnet.sh logs <node>                 # tail a node's logs
tools/devnet.sh cli <node> <subcommand…>    # run the Rust client against <node>
tools/devnet.sh down                        # stop the network
tools/devnet.sh down -v                     # stop + delete the data volumes
```

### Topology and genesis

`up --nodes N` starts:

1. **`devnet-bootstrap`** — `rnode run -s` (standalone). It generates its own TLS cert, **creates the
   genesis block**, and is bonded as the sole validator via a fixed validator key and a generated
   `bonds.txt`/`wallets.txt` mounted read-only.
2. **`devnet-observer-1` … `devnet-observer-N-1`** — `rnode run --bootstrap rnode://<bootstrap-id>@devnet-bootstrap?protocol=40400&discovery=40404`.
   Each peer gets its own data volume (so its node identity — the TLS cert — is stable), connects to
   the bootstrap over the **TLS protocol transport** (port 40400), and syncs the finalized fringe.

The bootstrap's node-id is read from its generated certificate (the cert CommonName is the base16
keccak-20 address). The script waits for the cert, extracts the id with `openssl`, and passes it into
each peer's bootstrap URL.

### Ports

Each node binds the same in-container ports; `up` maps each node's **deploy** gRPC port to a distinct
host port so you can also reach a node from the host. Propose/repl bind loopback-only
(`127.0.0.1:40402`), so they are not host-mapped. In bare (`--nodes`) mode the admin HTTP port is not
published (`--no-admin`):

| Service | In-container | Host (bootstrap / peerN) |
|---|---|---|
| protocol (TLS peer transport) | 40400 | not mapped (network-internal) |
| discovery (Kademlia) | 40404 | not mapped |
| gRPC API — Deploy | 40401 | 40402 / 40402 + 1000·N |
| gRPC API — Propose/Repl | 40402 | not mapped (loopback-only) |
| HTTP | 40403 | 40403 / 40403 + 1000·N |
| admin HTTP | 40405 | not published |

### Interacting from the CLI

`cli <node> <subcommand…>` runs the Rust client **inside the node container** via `docker exec`, so it
can reach both the deploy server (external port **40401**) and the loopback-only propose/repl server
(`127.0.0.1:40402`):

```sh
tools/devnet.sh cli devnet-bootstrap status
tools/devnet.sh cli devnet-bootstrap repl       # interactive REPL against the bootstrap
tools/devnet.sh cli devnet-observer-1 status
tools/devnet.sh cli devnet-bootstrap propose   # force a block (bare mode has no autopropose)
```

No `--grpc-port` is passed: the client defaults to the right port per subcommand (`deploy`/`status`/
`show-block`/… → 40401; `repl`/`eval`/`propose` → 40402), matching the node's port split.

`deploy` / `eval` read a file at a path inside the container; copy a local file in first:

```sh
docker cp demo.rho devnet-bootstrap:/tmp/demo.rho
tools/devnet.sh cli devnet-bootstrap deploy /tmp/demo.rho
```
