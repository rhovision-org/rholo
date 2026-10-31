# Building applications: local devnet and public testnet

This page is for developers building apps against a running RNode — wallets, block explorers, dapps,
indexers, and tooling. It starts with the local Docker **devnet**, then shows the stricter evidence
boundary for a shared **public testnet**. Both use the public API to **deploy rholang** and **read back
the results**.

The devnet is a throwaway local testnet: 1–3 bonded validators with real consensus and autopropose, a
funded deployer wallet, and every API surface published to `localhost`. It is *not* for production —
the validator and deployer keys are public dev keys baked into the script.

## 1. Bring up the devnet

Prereqs: `docker` (a running daemon) and `openssl`.

```sh
tools/devnet.sh build                 # build the rnode:local image (once)
tools/devnet.sh up --validators 1     # 1 validator, autoproposing (the default)
# or a small multi-validator net:
tools/devnet.sh up --validators 3 --observers 1
# or a bare 1–5 node network topology (no autopropose, manual propose):
tools/devnet.sh up --nodes 3
```

**Autopropose is a `up` flag, not a build flag.** You control it per run:

```sh
tools/devnet.sh up --validators 1                    # autopropose ON (default)
tools/devnet.sh up --validators 1 --no-autopropose   # autopropose OFF — blocks only via `propose`
tools/devnet.sh up --validators 1 --no-propose-on-deploy
                                                     # deploy no longer auto-proposes; autopropose timer still runs
```

The **bootstrap** node (`devnet-bootstrap`) is the one apps should talk to. Tear down with
`tools/devnet.sh down -v` (drop the `-v` to keep the data volumes). Run `tools/devnet.sh help` for the
full flag reference (autopropose / propose-on-deploy / admin / deployer-key / nodes).

## 2. Endpoints

There are two wire surfaces:

- **HTTP (JSON)** — the primary surface for apps. Read-only queries and signed deploys live on the
  *public* API (`40403`); block production lives on the *admin* API (`40405`).
- **gRPC (protobuf)** — the lower-level `DeployService` (external, `40401`) and the
  `ProposeService`/REPL (loopback-only, `40402`).

### Ports (bootstrap = node 0)

| Service | In-container | Host: bootstrap | Host: node *i* |
|---|---|---|---|
| Deploy gRPC (`DeployService`) | 40401 | 40402 | 40402 + 1000·*i* |
| Propose / REPL gRPC | 40402 | — (loopback) | — |
| Public HTTP API | 40403 | 40403 | 40403 + 1000·*i* |
| Admin HTTP API | 40405 | 40405 | 40405 + 1000·*i* |
| protocol (peer TLS) | 40400 | — | — |
| discovery (Kademlia) | 40404 | — | — |

Against the bootstrap: public HTTP at `http://localhost:40403`, admin HTTP at
`http://localhost:40405`, deploy gRPC at `localhost:40402`.

> **There is no "admin deploy".** `40405` is the **admin** API and exposes exactly one operation of
> interest to apps: `POST /api/v1/propose` — *force a block*. It does **not** accept deploys. Deploys
> always go through the **public** surface — `POST /api/v1/deploy` on `40403`, or the `DeployService`
> gRPC on `40401`/`40402`. The admin port is only relevant when you want to trigger block production
> yourself (e.g. `--no-autopropose`).

> **Propose/REPL is loopback-only.** It binds `127.0.0.1:40402` inside the container and is not
> host-mapped; `tools/devnet.sh` reaches it via `docker exec`. From an app, drive block production via
> the admin HTTP `POST /api/v1/propose` instead.

### The OpenAPI document

The v1 HTTP API is described by a hand-written OpenAPI 3.0 document the node serves itself:

```sh
curl -s http://localhost:40403/api/v1/openapi.json | jq
```

## 3. The three operations an app needs

1. **Deploy** — submit signed rholang for inclusion in a block.
2. **Read responses** — observe data a contract produced at a name.
3. **Force a block** — propose (only needed when the node is *not* autoproposing; the devnet
   autoproposes).

### 3.1 Deploy rholang

The fastest path during development is the CLI helper, which signs with the devnet's funded deployer
key and pays phlo:

```sh
tools/devnet.sh deploy hello.rho   # deploys examples/hello.rho to the bootstrap
```

From an app, submit a signed deploy over HTTP:

```http
POST /api/v1/deploy                 # public HTTP 40403
Content-Type: application/json

{
  "data": {
    "term": "@\"hello\"!(\"world\")",
    "timestamp": 1724500000000,
    "phloPrice": 1,
    "phloLimit": 1000000,
    "validAfterBlockNumber": 0,
    "shardId": "/root"
  },
  "deployer": "<base16 secp256k1 public key>",
  "signature": "<base16 secp256k1 signature>",
  "sigAlgorithm": "secp256k1"
}
```

Two fields are the ones a client gets wrong, so they are given the values the node's own tested
fixture uses (`tools/devnet-test.sh`): `shardId` is the **full shard id** — `/root` for the default
shard, `/root/child` for its child — *not* the bare `shard-name` from the config, and a mismatch is
rejected with "Deploy shardId '…' is not as expected network shard '…'". And
`validAfterBlockNumber` must be at or above the current block height minus the 50-block deploy
lifespan: `0` is always valid on a fresh chain, whereas `-1` is *expired* once the node is past
height 50, which is why the faucet anchors its deploys to the tip.

The response is the **deploy signature** (a hex string); keep it to poll status. The `signature` is a
secp256k1 signature over the protobuf-serialized `data` object; `deployer` is the signer's public key
and `signature` its signature, both base16. The devnet's funded deployer is validator 0
(`DEPLOYER_PRIV` in `tools/devnet.sh`); use the CLI to sign during development, and in production sign
with your own key and fund the corresponding wallet in genesis.

**Deploying is identical whether autopropose is on or off** — the same `POST /api/v1/deploy` (or
gRPC `doDeploy`) is used, and the deploy always lands in the deploy pool. Autopropose only changes
*when* it gets included in a block:

- **autopropose ON** (default): the deploy is proposed automatically — immediately (propose-on-deploy)
  or on the next timer tick.
- **autopropose OFF** (`--no-autopropose --no-propose-on-deploy`): the deploy sits in the pool until
  *you* call `propose` (admin `POST /api/v1/propose` on `40405`, or `rnode propose`).

#### Binary attachments (RCHIP #39)

A deploy may carry binary attachments instead of hex-encoding a blob into the `term`. Add an
`attachments` array (hex strings, in order) to `data`. It is part of the signed payload, so the
signature must cover it.

```http
{
  "data": {
    "term": "new out(`rho:io:stdout`), a(`rho:attachment:1`) in { out!(*a) }",
    "timestamp": 1724500000000,
    "phloPrice": 1,
    "phloLimit": 1000000,
    "validAfterBlockNumber": -1,
    "shardId": "/root",
    "attachments": ["deadbeef", "0102030405"]
  },
  "deployer": "<base16 secp256k1 public key>",
  "signature": "<base16 secp256k1 signature>",
  "sigAlgorithm": "secp256k1"
}
```

Inside the deploy, the *i*-th attachment (1-based) is the name `` `rho:attachment:i` ``, which
evaluates to its bytes as a `ByteArray` — send it, match it, hash it, like any other `ByteArray`.

A deploy with no `attachments` is unchanged: the field is omitted from both the JSON and the
protobuf, so existing signatures and deploy ids stay valid. Note that a node **without** this
version ignores the new protobuf field, so it would evaluate such a deploy *without* its attachments
and diverge — every validator must be on this version before attachments are used (a coordinated
upgrade; no genesis reset).

### Finality is not execution success (#487)

`acceptsDeploys` in `GET /api/v1/status` answers only whether the *local pool has space*. It
does not imply that the network can finalize. A successful `POST /api/v1/deploy` is a submission
receipt, and `ProcessedWithSuccess` means execution/inclusion, **not settlement**.

Use `GET /api/v1/finality-observation` for a sampled node-local
`latestBlockNumber`, `finalizedBlockNumber`, `finalizedBlockHash`,
`observedFinalityLag` and `observedAtUnixMs`. The `availability` field is intentionally
`unknown`: one node's frontier and peer count cannot prove live bonded quorum. Missing
readings return HTTP 503 with nullable fields. Observations can be stale; callers must
check timestamps and repeat sampling across validators before making network claims.

For a specific signed deploy, poll
`GET /api/v1/deploy-settlement/{deploy_signature}`. Its `stage` is one of
`notProcessed`, `includedUnfinalized`, `finalityUnknown`,
`finalizedSuccess`, `finalizedError`. Only `finalizedSuccess` sets
`settlementConfirmed=true`. A failed block-finality lookup yields
`finalityUnknown`, never success. All statements are explicitly node-local;
applications requiring independent confirmation should compare finalized hashes and
verify bonded-validator finality attestations across multiple nodes.

gRPC clients can make the corresponding separate queries through the existing
`DeployService` methods: `findDeploy` / deploy status to establish inclusion,
then `isFinalized` for that *inclusion block*, and `lastFinalizedBlock` for a
frontier observation. The existing gRPC `Status` does **not** become a quorum signal.
These reads do not change admission, consensus rules, pool bounds, or retries.

### 3.2 Read responses

> **Coming from `rnode`?** [Porting an app from rnode](porting-a-client.md) has the exact shape of every
> value, the terms this node refuses that a permissive parser accepted, and the four traps that read
> like bugs. Worth five minutes before you debug your parser.

Three ways, in order of increasing specificity:

1. **Poll a deploy's result** — after a deploy, `GET /api/v1/deploy-status/{deploySignature}` returns a
   `DeployExecStatus`: `ProcessedWithSuccess` (with the `deployResult` expression),
   `ProcessedWithError` (with the reducer's `deployError` text — the reason the deploy failed), or
   `NotProcessed` (not yet in a block). **The variant tags are PascalCase and the fields camelCase**,
   which is the shape law 43's envelope catalogue pins (`spec/conformance/envelope.tsv`) — a client
   switching on `processedWithError` matches nothing and errors nowhere.
2. **Run a term and read its result** — `POST /api/v1/explore-deploy` runs a rholang term against the
   current state without signing or persisting a deploy, and returns the reduced expression.
3. **Read data at a name** — subscribe with the CLI, or read point-in-time by block hash over HTTP.

CLI (streaming subscribe — blocks until data arrives):

```sh
tools/devnet.sh query hello          # listens for data at the public name @"hello"
```

HTTP (point-in-time at a block hash; an empty `blockHash` means the current state):

```http
POST /api/v1/data-at-name-by-block-hash
Content-Type: application/json

{ "name": { "ExprString": { "data": "hello" } }, "blockHash": "", "usePreStateHash": false }
```

A public name `@"hello"` is expressed as the rholang expression `{"ExprString": {"data": "hello"}}`
— note the `data` wrapper: every `RhoExpr` arm wraps its payload in a field named `data`, a map's
payload is a JSON **object**, and an unforgeable nests one level deeper
(`{"ExprUnforg":{"data":{"UnforgPrivate":{"data":"…"}}}}`). That is the reference document's shape
(`legacy/docs/rnode-api/rnode-openapi.json`, which is what a client generates its types from); the
served document describes it as `RhoExpr`/`RhoUnforg`, and AUDIT C38 records what it cost to have had
it the other way.

### 3.3 Force a block

```sh
tools/devnet.sh propose
# or over HTTP (admin):
curl -s -X POST http://localhost:40405/api/v1/propose
```

`POST /api/v1/propose` takes **no body** and returns a plain-text string, not JSON. Success is
`Success! Block <hex> created and added.`; a failure is `Failure: <reason> (seqNum <n>)`. It is a
*synchronous* call — it waits for the block to be created and validated, so it is the right path when
you need a block deterministically (e.g. your node is **not** autoproposing). When `--autopropose` or
`--propose-on-deploy` is on — the devnet default — blocks are already produced automatically, and
calling `propose` is redundant; an extra call while one is in flight returns
`Failure: another propose is in progress`, which is harmless.

### 3.4 Faucet — fund an unfunded wallet

The devnet seeds a **funded deployer wallet** at genesis (`wallets.txt`), and every node runs with
`--dev-mode --deployer-private-key`. The faucet transfers `0.3 REV` (`30,000,000` drops) from that
funded wallet to a caller's REV address — it is a real transfer, not newly-minted REV.

```sh
tools/devnet.sh faucet <rev-address>
# or over HTTP (public):
curl -s -X POST http://localhost:40403/api/v1/faucet \
  -H 'Content-Type: application/json' \
  -d '{"address": "<rev-address>"}'
```

The response is `{ "deployId": "<hex>", "amount": 30000000, "to": "<rev-address>", "status": "pending", "deployError": null }`.

**`status` is the outcome, and it is why the response has five fields rather than three.** `pending`
means a deploy was submitted and its result is not yet known; `resubmitted` means the same except the
deploy is the original signed drip replayed after a crash, so `deployId` is unchanged; `failed` means the
drip named by `deployId` was processed **with an error** and nothing was delivered — the reason is in
`deployError`. A `failed` response submits nothing new, so call again to retry (the allocation was
refunded). `deployId` is the deploy signature, so for a `pending` drip poll
`GET /api/v1/deploy-status/{deployId}` for `ProcessedWithSuccess`, then
wait until `last-finalized-block >= blockNumber` before reading the recipient balance. Submission is
not delivery. The endpoint is **dev-mode only**. When the faucet route is not enabled, it is not mounted
and therefore returns `404`. Requests are rate-limited to one drip per second; this change makes recipient
eligibility depend on finalized chain balance, and the public-testnet allocation is bounded to 10,000 REV.

### 3.5 Discover node capabilities (gate UI features without a hardcoded "devnet" flag)

A wallet should decide what to surface (a manual `propose` button, the faucet) from the node itself,
not from a hardcoded flag. `GET /api/v1/capabilities` (public `40403`) returns a stable, camelCase
object:

```sh
curl -s http://localhost:40403/api/v1/capabilities
```

```json
{
  "autopropose": true,
  "proposeOnDeploy": true,
  "manualPropose": false,
  "adminHttp": true,
  "devMode": true,
  "proposer": true,
  "nodeList": false,
  "faucet": true,
  "faucetRemaining": 999970000000
}
```

Semantics:

| Field | Meaning |
|---|---|
| `autopropose` | `--autopropose` — blocks are produced continuously on a timer. |
| `proposeOnDeploy` | `--propose-on-deploy` — a block is proposed immediately after each deploy. |
| `manualPropose` | `!autopropose && !proposeOnDeploy` — **a mode**, not an identity: an observer with neither flag reports `true` too. |
| `proposer` | **this node holds a validator identity and can propose at all.** An unkeyed observer reports `false`, whatever its mode. |
| `adminHttp` | the admin HTTP surface (`POST /api/v1/propose` on `40405`) is published and CORS-enabled (`--admin` + `--api-enable-devnet-cors`). |
| `devMode` | `--dev-mode`. |
| `nodeList` | `GET /api/v1/nodes` is mounted (`--api-publish-node-list`). |
| `faucet` | the `/api/v1/faucet` endpoint is available and has enough remaining budget for another drip. |
| `faucetRemaining` | remaining faucet allocation in REV drops; clients can stop offering a dry faucet. |

Recommended gating:

- show the **PROPOSE** button only when `manualPropose && proposer` — otherwise blocks are produced
  automatically, and on an observer `propose` could not produce one anyway.
- show the **FAUCET** button only when `faucet` is `true`.
- treat a node with `proposer: false` as read-only for writes: it can pool a deploy, but nothing on
  it will include the deploy unless the deploy relay is enabled.

On a default `tools/devnet.sh up` (`--autopropose --propose-on-deploy --admin --deployer-key`) this
reports `autopropose:true, proposeOnDeploy:true, manualPropose:false, proposer:true,
adminHttp:true, devMode:true, nodeList:false, faucet:true`. On the bare topology (`up --nodes 3`) the
bootstrap reports `autopropose:false, proposeOnDeploy:false, manualPropose:true, proposer:true`,
and an observer reports the same mode fields with `proposer:false`.

### 3.5b Discover deploy-capable nodes

A node that opted in (`--api-publish-node-list`) answers `GET /api/v1/nodes` (public `40403`) with
the peers it has probed and found deploy-capable. A wallet can use this to spread **distinct**
deploys over several nodes instead of sending everything to one hardcoded endpoint:

```sh
curl -s http://localhost:40403/api/v1/nodes
```

```json
{
  "networkId": "testnet",
  "primaryShard": "/root",
  "generatedAt": 1761868800123,
  "ownLatestBlockNumber": 128,
  "nodes": [
    {
      "nodeId": "de6eed5d…",
      "httpUrl": "http://203.0.113.10:40403",
      "networkId": "testnet",
      "shardId": "/root",
      "nodeVersion": "0.4.x",
      "latestBlockNumber": 127,
      "proposer": true,
      "acceptsDeploys": true,
      "self": false
    }
  ]
}
```

An entry exists only because the serving node **probed** it: the peer's `rnode://` protocol port
plus 3, over HTTP, answered `/api/v1/status`, named the peer's own node id, reported the same
`networkId` and a shard this node is a member of, and reported `proposer: true`. An observer is never
listed; neither is a peer that did not answer. The entries after `self` are ordered by `nodeId`;
`self` is the first entry when the serving node is itself a proposer.

Rules for a client:

- **404 means the node does not publish a hostlist** — keep using the node you know.
- Treat every field as **advisory**. A peer reports its own `proposer`, height and pool state over
  plain HTTP, so it can lie and an on-path observer can forge a well-formed answer. Verify an entry
  with `GET {httpUrl}/api/v1/status` before relying on it.
- Send **each distinct deploy to one node**, chosen round-robin. Never re-submit the same signed
  deploy to a second node: a signed deploy is already a transaction, and a retry after an ambiguous
  failure can be included twice if two proposers select it from their own pools.
- Take `validAfterBlockNumber` from the **lowest** `latestBlockNumber` among your targets, and
  `phloPrice` at least the **highest** `minPhloPrice`, so no target refuses on height or price.
  Prefer entries with `acceptsDeploys: true`, which is the pool's capacity at the last probe — still
  handle a 429.
- A node is listed only if its HTTP port is reachable at `protocol + 3`; a node behind a reverse
  proxy on 443, or on a non-standard port layout, is not discoverable this way.

### 3.6 List pending (pooled) deploys

A wallet can reconcile its outstanding transactions across sessions/devices with
`GET /api/v1/deploys` (public `40403`) — it returns every deploy that has been accepted but **not yet
included in a block**, most-recent-first:

```sh
curl -s http://localhost:40403/api/v1/deploys
```

```json
{ "deploys": [
  { "deployId": "<base16 sig>", "timestamp": 1724500000000, "deployer": "<hex>",
    "term": "@\"hello\"!(\"world\")", "phloPrice": 1, "phloLimit": 1000000,
    "validAfterBlockNumber": -1 }
] }
```

The `deployId` is the base16 signature — identical to what `POST /api/v1/deploy` returns and
`GET /api/v1/deploy-status/{sig}` accepts. When the pool is empty the response is `{ "deploys": [] }`.

Notes:

- A deploy appears here after `POST /api/v1/deploy` (or the faucet) and disappears once the proposer
  includes it in a block — so with `--autopropose` it empties quickly; with `--no-autopropose` it stays
  until you call `propose`.
- Only *pooled* (not-yet-included) deploys are listed. Expired deploys are pruned from the pool, and a
  signature that was never pooled (or is already in a block) won't appear — use
  `GET /api/v1/deploy-status/{sig}` for a specific deploy's outcome.
- Recommended app pattern: after `POST /api/v1/deploy`, poll `GET /api/v1/deploy-status/{sig}` for the
  terminal state; use `GET /api/v1/deploys` to enumerate anything still pending (e.g. on startup to
  reconcile across sessions/devices).

## 4. End-to-end example (curl)

```bash
# Is the bootstrap up?
curl -s http://localhost:40403/api/v1/status

# Deploy + observe with the CLI helpers (signs + pays phlo for you):
tools/devnet.sh deploy hello.rho
tools/devnet.sh query hello          # -> "world"

# Run a term without a deploy and read the result:
curl -s -X POST http://localhost:40403/api/v1/explore-deploy \
  -H 'Content-Type: application/json' \
  -d '"1 + 1"'

# Inspect the chain:
curl -s http://localhost:40403/api/v1/blocks
curl -s http://localhost:40403/api/v1/block/<blockHash>
```

## 5. Building against a shared public testnet

The public deploy/read API is the same surface an application uses on the local devnet, but a shared
public testnet changes the **trust and evidence boundary**. Treat the network as an independently
operated service: do not assume access to the admin API, do not assume a faucet is enabled, and do not
hardcode one observed block-production mode as a network invariant. The current public-testnet
endpoint and operator notes live in [Public testnet](../node/testnet.md).

Start by observing the node rather than inferring its configuration:

```sh
BASE=https://<public-testnet-node>

curl -s "$BASE/api/v1/status" | jq
curl -s "$BASE/api/v1/capabilities" | jq
curl -s "$BASE/api/last-finalized-block" | jq
```

Use those observations to set the deploy's `shardId`, choose a fresh `validAfterBlockNumber`, and gate
optional UI such as a faucet. A public client should not need `POST /api/v1/propose`: if the network is
idle, block production is an operator/network property, not authority the browser should silently
assume it has.

### 5.1 Keep unknown, false, and stale separate

Public endpoints can degrade independently. For example, status may be reachable while a capability or
finalized-state read fails. Do not turn an unavailable observation into a negative fact such as
`faucet=false`, `devMode=false`, or "node offline". Keep each observation's success/failure boundary
explicit and preserve the last known value as stale only if the UI says that it is stale.

This matters most around finality: a latest height can keep moving while a finalized-height read is
unavailable, and an application must not convert that gap into a finality claim.

### 5.2 Submission, execution, inclusion, finality, and state are different claims

A client should model these as a ladder rather than one boolean "success":

1. **Submission accepted** — `POST /api/v1/deploy` returned a deploy signature. This proves only that
   the node accepted the signed deploy into its submission path.
2. **Execution observed** — `GET /api/v1/deploy-status/{sig}` returned `ProcessedWithSuccess`. The
   response also identifies the block that processed the deploy.
3. **Containing block finalized** — query the exact processing block with
   `GET /api/is-finalized/{blockHash}` and require `true`. In a block DAG, a numerically higher
   finalized height alone does not prove that this particular block finalized.
4. **Application state observed from finalized state** — query the contract/balance/receipt from a
   finalized state view and check the exact postcondition the operation claims.
5. **Transaction-specific evidence** — where another operation could produce the same state change,
   bind the application receipt to something unique to this deploy (for example the deploy id, a unique
   contract receipt/name, or the containing block's deploy set). A balance increase by itself is useful
   corroboration, but it does not prove that one particular transfer caused it.

The distinction is intentional. A faucet or wallet that says "delivered" immediately after submission
can be wrong even when the HTTP request succeeded: the deploy may later execute with an error, or the
processing block may not yet be finalized.

### 5.3 Faucet: pending is not delivered

When the public node advertises `faucet:true`, the faucet response's `deployId` is the deploy signature.
Treat a `pending` response exactly like any other submitted deploy:

```sh
# 1. request test funds
curl -s -X POST "$BASE/api/v1/faucet" \
  -H 'Content-Type: application/json' \
  -d '{"address":"<rev-address>"}'

# 2. poll the returned deployId
curl -s "$BASE/api/v1/deploy-status/<deployId>" | jq

# 3. after ProcessedWithSuccess, verify the exact processing block
curl -s "$BASE/api/is-finalized/<blockHash>" | jq

# 4. use a finalized-state read for the claimed balance/contract postcondition
curl -s "$BASE/api/last-finalized-block" | jq
```

Only after the processing block is finalized should the client use a finalized balance/state read to
claim that the drip landed. `ProcessedWithError` is execution evidence that it did not. An unavailable
status is not success or failure; it is unresolved.

### 5.4 Shared-network hygiene

- Use **disposable keys** and test-value funds. A public testnet can reset and has no production-value
  guarantee.
- Refresh `validAfterBlockNumber` from current network state; a literal copied from a local-devnet
  example eventually expires on a long-running shared chain.
- Keep bounded request timeouts. A half-open public connection should not leave signing or transfer UI
  permanently pending.
- Do not expose an admin/propose control unless the node explicitly provides that authority and the
  application is meant to operate it. Ordinary public-testnet applications should stay on the public
  API.
- Record enough evidence to reproduce a claim: deploy id, inclusion block/hash, finalized observation,
  and the application-specific postcondition. Block heights alone are not stable fixtures because the
  shared testnet advances and may reset.

The same discipline applies to tokens and other capability-based applications: prove the operation the
UI names, and state separately what was submitted, included, finalized, and observed afterward.

## 6. Reference — HTTP routes

All routes are also served without the `/v1` segment (the legacy `/api/…` forms) except where noted.
Requests and responses are JSON (`camelCase` keys).

| Method | Path | Purpose | Body / response |
|---|---|---|---|
| GET | `/api/v1/status` | node version, address, peers, block height | `PublicStatus` |
| GET | `/api/v1/status/operator` | **admin listener only** — divergence records, the safety halt, the report counters, the sync task's state, the reset marker | `OperatorStatus` (a superset of `PublicStatus`) |
| GET | `/api/v1/capabilities` | block-creation mode + dev/admin/faucet surfaces | `NodeCapabilities` |
| GET | `/api/v1/deploys` | the pooled (not-yet-included) deploys | `PooledDeploy[]` |
| POST | `/api/v1/deploy` | submit a signed deploy | `DeployRequest` → deploy signature (string) |
| POST | `/api/v1/faucet` | **dev-mode** — transfer 0.3 REV to an address | `{address}` → `FaucetResponse` |
| GET | `/api/v1/deploy-status/{sig}` | a deploy's execution status | `DeployExecStatus` |
| POST | `/api/v1/explore-deploy` | run a term, return its result | raw JSON string term → `ExploratoryDeployResponse` |
| POST | `/api/v1/explore-deploy-by-block-hash` | run a term at a block hash | `{term, blockHash, usePreStateHash}` |
| POST | `/api/v1/data-at-name-by-block-hash` | data at a name, at a block hash | `{name, blockHash, usePreStateHash}` → `RhoDataResponse` |
| GET | `/api/v1/blocks` | recent blocks | `LightBlockInfo[]` |
| GET | `/api/v1/block/{hash}` | a block and its deploys | `BlockInfo` |
| GET | `/api/v1/openapi.json` | the OpenAPI document | JSON schema |
| POST | `/api/v1/propose` | **admin (40405)** — force a block | string |

The public API also serves `/api/last-finalized-block`, `/api/blocks/{start}/{end}`,
`/api/blocks/{depth}`, `/api/deploy/{deployId}`, `/api/is-finalized/{hash}`, `/version`, `/status`,
`/metrics`, and `/api/transactions/{hash}` (the transactions route answers 404 unless
`api-server.enable-reporting` is on).

## 7. Reference — gRPC services

`DeployService` (external, host `40402`) exposes `doDeploy`, `deployStatus`, `getBlock`, `getBlocks`,
`listenForDataAtName`, `getDataAtName`, `listenForContinuationAtName`, `findDeploy`,
`lastFinalizedBlock`, `isFinalized`, `bondStatus`, `exploratoryDeploy`, `getBlocksByHeights`,
`getEventByHash`, `visualizeDag`, `machineVerifiableDag`, and `status`. `ProposeService` (loopback)
exposes `propose` and `proposeResult`. The protobuf definitions live in
`models/proto/{deploy_service_v1,propose_service_v1,deploy_service_common}.proto`.

## 8. Notes and gotchas

- **Throwaway keys.** The validator/deployer keys in `tools/devnet.sh` are public dev keys; never reuse
  them for anything with real value.
- **CORS.** The admin HTTP API is CORS-restricted by default (`api-server.enable-devnet-cors = false`)
  so a foreign origin cannot trigger block production. The devnet opts in with `--api-enable-devnet-cors`.
- **Rate limit.** Deploy/explore endpoints on the public HTTP API are rate-limited; exceeding the limit
  returns HTTP 429.
- **Phlo.** Deploys pay `phloLimit × phloPrice`; the devnet seeds a funded deployer wallet so the
  helpers work out of the box. In your own genesis, fund the wallet that signs your deploys.
- **Reporting is off.** `api-server.enable-reporting` is `false` by default, so the transaction/event
  reporting routes are unavailable unless you enable them.
- **One node vs many.** Non-bootstrap nodes offset their host ports by `1000·i`, so node *i*'s public
  HTTP is `40403 + 1000·i`, its admin HTTP `40405 + 1000·i`, and its deploy gRPC `40402 + 1000·i`. For a
  single-validator devnet (`--validators 1`) the bootstrap numbers above are the only ones you need.
  In a multi-validator net you still **deploy to the bootstrap** (or a specific node with
  `tools/devnet.sh deploy hello.rho --to N`); the deploy then reaches the other nodes via block
  propagation, and `--autopropose`/`--propose-on-deploy` are per-node flags applied uniformly by `up`.
- **`InvalidStateHash` on propose.** A `Failure: … (seqNum N)` containing `InvalidStateHash` means the
  node rejected its **own** freshly-created block: the post-state hash it computed did not match the
  state it recomputed by replaying the block's deploys. It is a node-side bug, *not* caused by your
  deploy, your signature, or calling `propose`/`explore-deploy`. Read-only requests (data-at-name,
  explore-deploy) run on an isolated fork of the state, so they cannot disturb block production; if
  you see this error, it is a consensus bug to report, not a request you should retry.

## 9. When blocks are created (the formal spec)

CBC-Casper is **deploy-driven**: a block is created **only** when one of three triggers fires on a
node. An idle node with none of them produces no blocks — there is no independent heartbeat.

1. **Propose-on-deploy** (`--propose-on-deploy`) — a submitted deploy triggers an immediate propose so
   it lands in the next block.
2. **Autopropose** (`--autopropose`) — the node keeps proposing on its own: a timer tick, a validated
   peer block, or (with dev-mode) a signed `Nil` **dummy deploy** injected whenever the pool is empty.
3. **Manual propose** — `POST /api/v1/propose` (admin HTTP, `--admin`) or `rnode propose` (gRPC).

The devnet defaults to all three on; the bare topology (`up --nodes N`) is all three off. The full flag
matrix is `tools/devnet.sh help`:

| Script flag | Node flag(s) | Effect |
|---|---|---|
| `--autopropose` (default on) | `--autopropose` | continuous block production (timer + dummy deploy) |
| `--propose-on-deploy` (default on) | `--propose-on-deploy` | propose immediately after a deploy is accepted |
| `--admin` (default on) | `--api-enable-devnet-cors` + publish `40405` | expose the admin `/api/v1/propose` to the host |
| `--deployer-key` (default on) | `--dev-mode --deployer-private-key` | fund the deployer wallet + dummy-deploy keepalive |
| `--nodes N` | (all of the above off) | bare 1–5 node network topology, manual propose via `cli` |

**Gotcha:** if you `deploy` against a node that wasn't started with `--deployer-key` (or `propose
--admin` against one without `--admin`), the request fails — the script surfaces a clear warning/error
instead of a raw connection failure. Run `tools/devnet.sh diagnose` for a per-node PASS/WARN/FAIL
report of syncing, block production, and peer connectivity.

Observe consensus and finality over the public HTTP API (`http://localhost:40403`):

- `GET /api/v1/status` — `latestBlockNumber` climbing on its own.
- `GET /api/v1/blocks` — recent blocks. The `sender` and `justifications` fields show multiple
  proposers cross-justifying each other's blocks (the block-DAG, not a single chain).
- `GET /api/v1/block/{blockHash}` — one block plus the deploys it carries.
- `GET /api/last-finalized-block` — the finalized fringe (a legacy `/api/` route, not under `/v1`).
- `GET /api/is-finalized/{blockHash}` — whether a specific block has passed the `> 2/3` stake
  threshold.

To exercise the real deploy path (submit a signed user deploy and poll its status), use
`tools/devnet.sh deploy hello.rho` and `tools/devnet-test.sh`, which cover it end-to-end.

The devnet is a *reasonable simulation of production, gaps stated*: single shard (`root`), equal stake
⇒ unanimous finality, dummy `Nil` deploys, no Byzantine behavior. The full fidelity note is in
[Local devnet (Docker)](../node/devnet.md).
