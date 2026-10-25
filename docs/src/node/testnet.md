# The public testnet — `testnet.rhobot.net`

A small public RChain testnet running this codebase, used by the [r-wallet](https://rhovision-org.github.io/r-wallet) and the [quantum-os](https://github.com/rhovision-org/quantum-os/blob/main/README.md) room agents. **Four bonded validators at equal stake, two per host** — A and D on the first, B and C on the
second — funded dev wallets, and an **idle** chain that produces a block only when a deploy arrives.

The **generalised** procedure — standing up a testnet of your own, from the stake split to a rebuild —
is [Running a public testnet](running-a-public-testnet.md). This page is the concrete instance: the live
hosts, the genesis, the wallets.

> **Status: four bonded validators at 250 each, pool 1000, and the chain finalises.**
> A brand-new key can be funded, deploy, be trusted and bond into the pool. **Losing one validator is
> survivable** — the other three keep finalising at 75 %; losing two leaves 50 %, which is not a quorum
> under the strict supermajority rule, so finality stops until one returns
> ([#214](https://github.com/rhovision-org/rholo/issues/214)). `/api/status`, `/api/explore-deploy`, `getBonds`,
> `getActiveValidators`, `/health`, and `rnode deploy` all work — a CLI deploy needs a **funded** key, or
> it is accepted and mined and then reports `processedWithError` for phlo. The chain is deliberately
> **idle** (no `--autopropose`): blocks appear when a deploy arrives. A continuously-producing chain
> cannot be restarted on a 1 GB host — that is the most important operational
> constraint here. Not production, holds no value, and its chain can be reset at any time.

---

# Part 1 — For users

## What it is

| | |
|---|---|
| Chain | `testnet` network id, shard **`/testnet`**; genesis built from this net's `bonds.txt`, four equal
  stakes. Rebuilt 2026-10-24 from a fresh genesis `c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26` |
| Validators | **four, at equal stake — 250 each, pool 1000**, two per host. Equal stakes are the point: every validator is 25 %, so **any one of them can be lost and the survivors still finalise** — measured 2026-10-04 by stopping A, the genesis master and the endpoint's own node (finality 65 → 90 with the height 69 → 94, three survivors in lockstep), and A rejoined to the tip in under 20 s. Joiners are capped by the chain (`--bond-maximum 250`) and the active set is bounded at 4, so the quarter-share survives growth. See [Recovery](#recovery) |
| Hosts | A `164.90.140.144` (nyc3) and B `167.99.43.214` (**ams3**) — deliberately on two continents, so every 3-of-4 quorum crosses the Atlantic. Both `s-2vcpu-4gb` |
| Cost | 2 × DigitalOcean `s-1vcpu-1gb`, **$12/mo** |
| Binary | `dev` @ `185ed283`, static musl, `sha256:c4948443bb6b…`, **the same build on all four nodes and on the playground** — rebuilt 2026-10-24 |
| Endpoint | **https://testnet.rhobot.net** (nginx → node A's HTTP API) |

Short hashes in this document are the first twelve hex characters of the value they name, and each is
labelled with what it is a digest *of*: `sha256:` is `sha256sum` over the `rnode` artifact as shipped,
so it can be recomputed from the release; the bare ones are block hashes and addresses as the node
prints them. (Before the September 2026 audit the binary's digest was written unlabelled, which made it
indistinguishable from a commit and unverifiable either way.)

The quorum is measured against the **whole bond pool** (including stake sitting in withdrawal
quarantine), with no inactivity leak, no decay and no eviction — so an absent validator's stake goes on
counting. That is why the *shape* is chosen for what it can lose rather than for who proposes: at four
equal stakes each validator is a quarter and any one of them can stop ([Recovery](#recovery)).

**There is no single-proposer mode, and this page no longer claims one.** With
`--propose-on-deploy --attest-on-new-blocks` on each node, any bonded validator that receives a deploy
proposes, and a deploy addressed to **any one** of them finalises. That this net *looks* single-proposer
is the public endpoint's routing, and it has a cost if A dies — see
[Routing, and what it costs if A dies](#routing-and-what-it-costs-if-a-dies).

## Connect

| | |
|---|---|
| HTTP API | **https://testnet.rhobot.net** (nginx → node A's `40403`) |
| Health snapshot | `https://testnet.rhobot.net/health` — JSON, refreshed by each node every 60 s |
| Direct, if you prefer | `GET http://164.90.140.144:40403/api/status`, `POST http://164.90.140.144:40403/api/explore-deploy` (eval a read-only term) |
| Admin | `POST http://164.90.140.144:40405/api/v1/propose` — force a block (keep this one private in general) |

Ports `40400` (protocol) and `40404` (discovery) are open on all four nodes, as are `40401`, `40403` and
`40405` for clients — and the `414xx` family for nodes D and C, which share the hosts — what each is for is in
[running a public testnet § Ports](running-a-public-testnet.md#ports).

## Use it from r-wallet

[r-wallet](https://rhowallet.org) can select this net directly: in the wallet's network list, choose
**RChain testnet**. Balances, transfers and deploys then go to `https://testnet.rhobot.net`, which is
node A's HTTP API.

Two entries in that list are easy to confuse, and they are different chains:

| entry in r-wallet | what it actually is |
|---|---|
| **RChain testnet** | this net — `testnet.rhobot.net` |
| **Rholang playground** | the playground node at `playground.rhobot.net` (formerly `rnodeapi.rhobot.net`) — a dev-mode chain that is *not* this one: one validator on rhobot-2, shard **`/playground`**, genesis `edd277849ea96848f983d854fa8eb92d942fda2e6b9d4937d4827da423a9f8d9`, rebuilt 2026-10-24 from the same image as this net |

Until 2026-09-22 the wallet's only "testnet" entry pointed at the playground, so picking it put you on
the wrong chain, with a dead faucet link; that is fixed in
[rchain-wallet#12](https://github.com/rhovision-org/rchain-wallet/pull/12).

Two practical notes:

- the wallet needs a key holding REV to deploy or transfer — fund that key's address from the **faucet**
  ([Getting REV](#getting-rev--the-faucet) below), rather than importing a funded key that belongs to
  someone else;
- it does not need a node's admin API: both testnet nodes run `--propose-on-deploy`, so the node that
  receives a deploy proposes it.

## Getting REV — the faucet

Test REV comes from a **faucet**, not from an account someone hands you. There are two doors to it, and
both sign the same `revVault` transfer.

**From r-wallet, or any client: the node's own faucet.**

```
POST https://<node>/api/faucet   {"address": "<your REV address>"}
→ {"deployId":"3045…","amount":30000000,"to":"1111…",
   "status":"pending","deployError":null}                    # 30,000,000 drops = 0.3 REV
```

**Read `status`.** A submission is not a delivery, and the response now says which it is: `pending`
(a deploy was submitted, outcome not yet known — poll `deploy-status/{deployId}`), `resubmitted` (the
same, except it is the original signed drip replayed after a crash, so the ID is unchanged), or
`failed` — the drip named by `deployId` was processed **with an error** and nothing was delivered, with
the reason in `deployError`. A `failed` response has submitted nothing new, so call again to retry; the
allocation was refunded either way. This is the field that would have made an **unfunded** signing key
visible: signing with a key whose vault is empty produces a deploy that is accepted and then fails the
phlo pre-charge, which used to be indistinguishable from a drip on its way.

That is the endpoint r-wallet calls against whichever node it is pointed at, and it is all the wallet
needs to fund a fresh address. It is a **dev-mode** endpoint: the node must be started with
`--dev-mode --deployer-private-key`, and it signs the transfer from that key's vault. **Without the key
the route is not mounted at all, so a keyless node answers `404`**, and it reports `faucet: false` in
its capability list. `faucetRemaining` reports the remaining allocation in drops, and a dry faucet
reports `faucet: false`. The 10,000 REV allocation and unresolved deploy reservations
are stored in the node's data directory so ordinary process restarts preserve both. After a crash,
an unresolved drip is retried with its original signed deploy and deploy ID; the reservation remains
until its chain outcome can be reconciled.

| node | faucet |
|---|---|
| `testnet.rhobot.net` (node A) | ✅ **works** — dev-mode plus a funded key (`/etc/rnode/faucet.env`), **0.3 REV per drip, ten drips per address per process**. Verified delivering on the 2026-10-06 chain |
| `playground.rhobot.net` | ✅ **works** — dev-mode plus a deployer key |

Two facts a client needs and neither is obvious: the faucet's **`deployId` is the deploy signature**, so
`GET /api/v1/deploy-status/<signature>` reports what became of the drip (`ProcessedWithSuccess`, or the
error); and a drip is only visible in a **finalised** block, so read the recipient's balance once
`/api/last-finalized-block` has passed the drip's `blockNumber`. The endpoint is also rate-limited —
two quick requests earn `HTTP 429 "faucet rate limit exceeded"`.
**This net's faucet was off until 2026-10-06, and this table said so**; it is on now, signed by a key whose
vault holds REV ([#247](https://github.com/rhovision-org/rholo/issues/247) is the story of the one
that was not). This row describes the currently deployed ten-drips-per-process policy; #246 changes
eligibility to require the recipient's finalized balance to remain below one drip.

**In a room: `/facil faucet`.**

A quantum-os facilitator started with `--key <funded deploy key>` answers

```
/facil faucet <your REV address>     # or just /facil faucet, once it has remembered your address
```

and signs a fixed **10 REV** transfer to it. This facilitator surface is separate from the node HTTP
faucet; do not describe the node faucet as unlimited: its public HTTP surface has a fixed-window rate
limiter and its own address/total budgets. The facilitator refuses to move anything if it was started
without a key. Plain English works too: `/facil ask give me some test rev` routes to the same function,
never to an LLM decision to move funds.

To put that faucet on **this** net, give a facilitator a funded testnet key and point it here; it will
also answer over HTTP, in the same wire shape as the node's own endpoint:

```sh
node scripts/qos-cli/agent.mjs --room <room> --role facilitator \
  --rnode https://testnet.rhobot.net --key <a funded key on this net> --faucet-http 8080
```

**The one thing the faucet needs is your REV address**: `/rholang key show` prints it in the playground,
and r-wallet shows the address of the key you hold. Test REV holds no value — never send a faucet an
address whose key you care about.

## What works today

| | |
|---|---|
| `eval` / `explore-deploy` | ✅ works — the full Rholang/QLF macro surface |
| Reads of chain state (`getBonds`, `getActiveValidators`) | ✅ works |
| `GET /api/status` | ✅ works |
| `/health` monitoring snapshot | ✅ works |
| **Deploys — browser, room agents, `rnode deploy` CLI** | ✅ works (the CLI needed `--valid-after-block-number` before the 2026-09-21 binary) |
| Transfers, including funding a brand-new key | ✅ verified: a fresh key's balance went `0` → `100000000000`, and it could then deploy |
| **Becoming a validator** | ✅ verified end to end on the 2026-09-22 chain: `trust` → `(true)`, `bond` → `(true)`, bond pool 2 → 3, active set 3. The procedure is in [Onboarding an observer into the validator pool](#onboarding-an-observer-into-the-validator-pool) |

## Monitoring

`https://testnet.rhobot.net/health` is refreshed every 60 seconds by each node and answers with
that node's snapshot (node A's, since nginx fronts A):

```json
{ "host": "testnet-a", "ok": true, "rnode_unit": "active", "api_reachable": true,
  "blocks": 135, "blocks_since_last_tick": 0, "peers": 3, "nodes": 4,
  "finalized_fringe": true, "autopropose": false,
  "mem_available_mb": 300, "disk_free_mb": 20000 }
```

`ok: false` means the unit is down, the API is unreachable, or the height is zero. **`finalized_fringe:
false` is a fault on this net**: with four validators attesting, the fringe advances, and a `false` here
means finality has stopped and wants investigating. Note the snapshot is refreshed every 60 s, so it can
lag the chain by up to a minute — read `/api/last-finalized-block` for the current value. DigitalOcean's dashboard also graphs CPU/RAM/disk for both hosts
(`do-agent`).

## Limits to expect

- **Idle chain, by design.** No `--autopropose` and no injected dummy deploy: a block is produced
  when a deploy arrives (`--propose-on-deploy`). Heights stay put while nobody is doing anything.
- **A rebuild resets everything.** Changing `bonds.txt`/`wallets.txt` means a new genesis and a new
  chain.
- Disk now grows with real usage only (~6.6 KB per block) instead of ~140 MB/day.
- **Start-up is the expensive part.** Replay costs about **0.25 MB of RAM and ~0.2 s per existing
  block** before the API opens at all (a 1142-block chain: ~285 MB, ~3.5 minutes), while *producing*
  blocks is nearly free. Budget ≥2 GB for ~1k blocks, ≥4 GB to be comfortable — see

Restart cost is proportional to the length of the chain, not to activity — that is the constraint to
- No SLA, no backups of chain state beyond the genesis files.

---

# Part 2 — For maintainers

## Related pages

Where this page and the generalised ones overlap, prefer those: this one is the record for **this** net —
our hosts, keys, genesis and health checks.

| page | what lives there |
|---|---|
| [Running a public testnet](running-a-public-testnet.md) | the generalised procedure: stake split, genesis ceremony, joining, admitting a validator to a running chain, monitoring, sizing, rebuilds, and the port map |
| [Operating the node](operating.md#deploying-and-block-production) | `--shard-id`, the deploy-anchor rule, reading a term's return value, the block production modes, finality and withdraw behaviour |
| [Running a validator: hardware requirements](validator-requirements.md) | host sizing, including the start-up replay floor () |
| [#60](https://github.com/rhovision-org/rholo/issues/60) / [#68](https://github.com/rhovision-org/rholo/issues/68) | the start-up replay: measurements, reproductions, and what is still unattributed |
| [#39](https://github.com/rhovision-org/rholo/issues/39) | the validator lifecycle; this net's verified transcript is posted there |

The deploy-anchor fix () reached `dev` as
[#58](https://github.com/rhovision-org/rholo/pull/58), and the node docs above came in as
[#61](https://github.com/rhovision-org/rholo/pull/61).

## Topology

```
testnet.rhobot.net ──► node A 164.90.140.144 (10.108.0.3)   genesis master, stake 250  (ports 40400-40405)
                        └─ nginx + Let's Encrypt (cert to 2026-12-20), /health from a timer
                        └─ rnode: -s --dev-mode --propose-on-deploy --attest-on-new-blocks
                       node D 164.90.140.144 (10.108.0.3)   validator, stake 250      (ports 41400-41405)
                        └─ rnode: --dev-mode --propose-on-deploy --attest-on-new-blocks --bootstrap A
                       node B 167.99.43.214 (ams3)         validator, stake 250      (ports 40400-40405)
                        └─ rnode: --dev-mode --propose-on-deploy --attest-on-new-blocks --bootstrap A
                       node C 167.99.43.214 (ams3)         validator, stake 250      (ports 41400-41405)
                        └─ rnode: --dev-mode --propose-on-deploy --attest-on-new-blocks --bootstrap A
```

A is in `nyc3`, in the same VPC as rhobot-2, so A and rhobot-2 can also talk over private addresses
(`10.108.0.0/20`). B is in **`ams3`** — a different region and a different VPC — so every A↔B exchange
crosses the public internet, and every 3-of-4 quorum crosses an ocean. B's two nodes share a host and
are told apart by their port families.

### Where each node is

| node | IP address | protocol | gRPC ext | HTTP API | discovery | shard | role | public name |
|---|---|---|---|---|---|---|---|---|
| **A** `rnode` | `164.90.140.144` (nyc3) | 40400 | 40401 | **40403** | 40404 | `/testnet` | genesis master, validator | **https://testnet.rhobot.net** |
| **A** `rnode-d` | `164.90.140.144` (nyc3) | 41400 | 41401 | **41403** | 41404 | `/testnet` | validator | — |
| **B** `rnode` | `167.99.43.214` (ams3) | 40400 | 40401 | **40403** | 40404 | `/testnet` | validator | — |
| **B** `rnode-c` | `167.99.43.214` (ams3) | 41400 | 41401 | **41403** | 41404 | `/testnet` | validator | — |
| **observer** | `138.197.65.34` (rhobot-2) | 42400 | 42401 | **42403** | 42404 | `/testnet` | observer — follows, never votes | — (firewalled to one client) |
| **playground** | `138.197.65.34` (rhobot-2) | 40400 | 40401 | **40403** | 40404 | `/playground` | its own one-validator chain | **https://playground.rhobot.net** |
| **history** | `138.197.65.34` (rhobot-2) | 43400 | 43401 | **43403** | 43404 | `/history` | genesis-only snapshot, 13,946 wallets | **https://history.rhobot.net** |

Each row is reachable at `IP:port` on that address — node A's HTTP API is
`http://164.90.140.144:40403`, the observer's is `http://138.197.65.34:42403`. Within a row the
ports are fixed offsets from the **protocol** port: `+1` gRPC external, `+2` gRPC internal (not
shown), `+3` HTTP API, `+4` discovery, `+5` admin HTTP. Admin is **disabled on every node**, so
nothing listens on `…+5`.

`…/api/status` and `…/api/last-finalized-block` work on every row, so a single node can always be
diagnosed without going through the public name.

**What is exposed.** A and B publish `40400–40405` and `41400–41405` to the world, so every
validator's HTTP and gRPC API is reachable **without TLS** — use `testnet.rhobot.net` unless you are
diagnosing one node. The observer's `42403` is open to a single client address, and `43403` is
reachable only through its own nginx vhost.

**Why four equal stakes, a cap of 250, and no `--autopropose`.** The split and the cap were both
learned by breaking it; the host lesson is the third item.

1. **Finality needs >⅔ of the whole pool, and the shape is chosen for what it can lose.** The quorum
   denominator is the **whole bonded pool**, with no inactivity leak and no eviction, so an absent
   validator's stake goes on counting. At **four equal stakes of 250**, each validator is 25 % and the
   remaining three are 75 %, so **any single validator can stop and the chain keeps finalising**. That is
   the property no three-validator split can have: three stakes each below ⅓ cannot sum to the whole, and
   three equal stakes fail *exactly* on the boundary, since ⅔ is not `> ⅔`. Measured on this net
   2026-10-04 by stopping **A** — the genesis master and the node the public endpoint routes to — and then
   addressing deploys to D, B and C: finality ran **65 → 90** while the height ran **69 → 94**, all three
   survivors in lockstep, and A rejoined to the tip in **under 20 s** when started again.
2. **`--bond-maximum 250` keeps that property as the pool grows.** It is a **genesis parameter**, so the
   chain refuses a joining bond above it; the active set is bounded at 4. A fifth validator at the cap
   takes the pool to 1250 with everyone at 20 %, which still tolerates any single loss. An earlier shape on
   this net (a single 800 core plus two 100 joiners) tolerated the loss of both joiners but made the core's
   own loss **fatal**; four equal stakes remove that asymmetry, because no single key is worth more than a
   quarter. This is why the *key material* matters more than any one host: any validator can be rebuilt
   from its key and a data directory, and none of them is irreplaceable on its own.
3. **Start-up is the expensive part, so the chain is idle.** Replay costs roughly **0.25 MB and ~0.2 s per
   existing block** before the API opens, and a chain that produces continuously grows that cost with
   wall-clock time rather than with use: on a 1 GB host a long enough chain cannot be restarted at all — the
   kernel kills rnode mid-replay and every restart repeats it. Omitting `--autopropose` and
   `--deployer-private-key` makes blocks arrive only when deploys do, keeping restart cost proportional to
   real usage. Sizing guidance: [Running a validator: hardware requirements](validator-requirements.md).

## Routing, and what it costs if A dies

**Every bonded validator proposes; what looks like one proposer is routing.** Every node runs
`--propose-on-deploy --attest-on-new-blocks`, so any validator that receives a deploy proposes — and
since [#219](https://github.com/rhovision-org/rholo/pull/219) (C209, 2026-10-04) a deploy
addressed to **any one** of them finalises: the acceptance run sent every deploy to a single validator,
one arm per validator, and all three finalised. The public hostname proxies to node A's `40403`, so an
HTTP-only client reaches A. The three **gRPC deploy ports are open, and each one is a valid way in**:

| validator | stake | deploy endpoint |
|---|---|---|
| A | 250 | `164.90.140.144:40401` |
| D | 250 | `164.90.140.144:41401` |
| B | 250 | `167.99.43.214:40401` |
| C | 250 | `167.99.43.214:41401` |

A client that addresses B or C directly gets that validator's proposal, and its block finalises the same
way. Spreading deploys across the three is a supported configuration, not a workaround.

**If A stops, the endpoint stops — the chain does not.** A is a quarter of the pool like the other
three, so its absence costs the chain nothing in finality: measured by stopping A and addressing deploys to
D, B and C, which kept finalising (f 65 → 90). What A's absence *does* cost is the public hostname, which
proxies to A, so an HTTP-only client cannot submit until A is back. That is precisely why the table above
lists all four deploy endpoints: admission should not depend on one node's availability, and finality no
longer does.

The routing point stands on its own merits: publish every validator's deploy port (the table above) or
round-robin the endpoint across them, keeping `--propose-on-deploy` on each. Then a client's *admission*
does not depend on one node's availability, whatever the pool's arithmetic does about finality.

## Recovery

What the shape tolerates, and what the operator does about it. The first row is **measured on this net**;
the others are arithmetic on the same numbers, and are labelled as such.

| what failed | what happens | action |
|---|---|---|
| **any one** validator | **nothing** — the survivors hold 750 of 1000 = 75 % and keep finalising. Measured by stopping A: deploys went to the other three and finality advanced, then A rejoined to the tip in under 20 s | `systemctl start rnode` (or `rnode-d`, `rnode-c`) on the host that holds it. A returning validator is level with the tip in seconds; that rejoin path was broken until [#223](https://github.com/rhovision-org/rholo/issues/223) |
| **two** validators (50 %) | *arithmetic, not yet run:* 500 of 1000 is not `> ⅔`, so the fringe cannot advance however many blocks are produced | **one** of the two coming back restores 75 % and the chain finalises again. Nothing is lost while they are away: the state is on disk, and the pool still counts their stake |
| **all four** | production stops; every node's state sits unchanged on disk | stop and start all four: each replays its own store and the chain resumes with no loss (this exact restart was measured on the previous shape on 2026-10-04: h 59 → 78, finality 53 → 72 after all nodes were restarted) |
| a validator's **key or host** permanently | as above, while its stake still sits in the pool | restore that validator's `validator.key` and data directory, or its host from the provider's backup. Because no stake exceeds a quarter, **no single loss is fatal** — but two simultaneous permanent losses are, since 50 % can never reach a quorum |
| the chain **diverges** — several heads, finality frozen | nothing in-protocol recovers it: `docs/src/spec/testnet-acceptance.md` §TE-1 states the verdict as *"Recovery from the divergent finality itself: none."*, and the live occurrence is witnessed at `spec/audit/evidence/te-1-2026-10-09-four-divergent-heads.md` | **the store-level restore** — see below. It is the only recovery demonstrated for this state |

### Recovering a diverged chain

The last row is the one recovery demonstrated for a chain whose finality has frozen, and it is worth
stating what it does and does not do, because **it is not a sync**.

- **It copies state.** `tools/reconcile-network.sh --restore-from-master` stops the whole network — the
  survivor included, because a filesystem-level copy of a live LMDB is a torn snapshot — and copies the
  survivor's *chain state* (`blockstorage`, `dagstorage`, `rspace/history`, `rspace/cold`) onto the other
  nodes. It **never copies identity**: a joiner holding the survivor's key would be an equivocator, and no
  agreement check would see it until the two later signed conflicting blocks.
- **It is a fiat, and the tool prints that before it applies.** The joiners adopt the survivor's view of the
  chain, including the blocks above the reconciliation point that only the survivor accepted — so running it is
  the operator choosing a winner. That is why the plan is printed first, why `--apply` is required, and why the
  tool refuses outright where it cannot **prove** its point: an attestation that does not verify, an ancestry
  walk that does not close, or a stake fraction that is not a strict supermajority all stop it at the plan
  phase.
- **The anchor is computed: the meet.** Every listed node is asked to **attest** the block it has finalised
  (`/api/v1/finality-attestation`, bound to a nonce the tool draws for the run, signed by the node's validator
  key), and every block the tool touches is fetched as the **preimage of its hash**
  (`/api/v1/block/{hash}/raw`) and verified: the bytes must hash to the claimed hash, and the signature must
  verify under the claimed sender. Both checks are the node's own rules, reached through
  `rnode verify-block` / `rnode verify-attestation`, so nothing here re-derives a rule the node owns — a
  second implementation that disagrees with the first is its own failure class. The reconciliation point is
  then **the deepest block a strict supermajority of stake has finalised** (`3 * stake > 2 * total` on
  integers, so exactly two thirds is not one), which tolerates the ordinary lag between honest nodes —
  measured on this net: three survivors reading finalised 102 / 97 / 110 — and can neither be dragged back by
  a laggard nor vetoed by one. Ancestry is checked by walking **verified** justifications; where the counted
  nodes' blocks are not on one branch, the walk steps down until they meet, bounded, and refuses at the bound
  rather than guessing below it. The tool prints the fraction it computed, the nodes it counted, and the ones
  it left out and why (register row **C272**; before this it required every node to report the *identical*
  finalised block, which is not a property a producing chain has).
- **Where nothing is finalised, you name the block.** A net with no usable finalised block anywhere is
  exactly the state where a computed anchor is impossible — there is nothing to meet *on* — and until
  2026-10-10 the tool refused there and stopped, which left its own recovery path unreachable (register row
  **C269**). It takes `RECONCILE_ANCHOR=<block hash>`: the block you know the net last agreed on. The tool
  checks only that your master holds it, says in the plan that this is **your** root and not a meet it
  computed, reports what each node claimed rather than resolving it, and enumerates what is above your point.
  Configure the joiners with `--sync-anchor <the same hash>` and the wipe path resyncs to it;
  `--restore-from-master` does not need the flag, because it copies the master's chain state. Nothing above
  your anchor is taken on trust either way: every block above it is validated, not installed.
- **Its equivocation check proves what it can.** Section 2 still **reports** a sender with two distinct blocks
  at one height, read from the block API's own `sender` field — one endpoint's word, and nothing is decided on
  it. Section 3 **proves** an equivocation where the artefacts reach: two distinct blocks by one sender at one
  height, *each fetched and verified*, with the sender read from the verified bytes rather than from the
  summary that suggested where to look. A proven equivocator's stake leaves the numerator **and** the
  denominator, which is the only way a pool may legitimately shrink — never by silence, because a threshold
  that drops when a validator goes quiet would let whoever can silence honest nodes finalise with a minority.
  The proof scans a bounded window of heights around the deepest attested block and says which, so the absence
  of a proof is read as a bound rather than a clean bill of health. A validator that is merely quiet, slow or
  absent keeps its weight: silence is not evidence. The on-chain slash is still the node's
  ([#290](https://github.com/rhovision-org/rholo/issues/290)'s part 1).
- **What it does not do.** It does not repair the discarded heads' deploys: they are enumerated **one record
  per unique block** — the block's own hash, the endpoints that served it, its accepted deploy count, and the
  **signatures of the deploys its merge rejected** (`rejectedDeploys`, base16; a deploy's id *is* its
  signature) — and their owners re-submit. The record is per block, not per observation: a block held by every
  node is one line, because `/api/blocks/{h}/{h}` answers per node and counting answers inflated the work list
  by the replication factor. It is also **not** the protocol answer, though the protocol answer now exists
  beside it: a node-side path that syncs to an agreed anchor is **built and measured** — `--sync-anchor`
  names the root *in the request*, the responder builds that block's fringe **and its whole ancestry**
  (register row **C259**(a), the #139 pattern), and the **windowed catch-up** walks the gap upward, a wiped
  joiner 300 heights below the tip reaching it in 98 s with no drops
  (`spec/audit/evidence/n-anchor-drill/run-3-windowed-catchup.txt`). **C259 closed 2026-10-11**: the
  no-finality case — a net with no usable finalised block anywhere — is recovered by
  `--restore-from-master`, measured on that exact state (meet at genesis, one head, identical per-height
  hashes, finality resumed on one hash: `spec/audit/evidence/n-reconcile-drill/restore-disarmed.txt`).
- **Two costs the restore still pays, stated rather than implied.** The LFS restore hydrates the tuple space
  of every block from its root down to genesis, so it is **O(chain)** (`Requesting tuple-space data for 316
  approved block state roots`, ~2.5 minutes on the control) — and a node-side anchor restore pays the same.
  And the **sidecar stores a merge needs** (`mergeable-channel-cache`, `native-changes-cache`) are *not* part
  of the state a sync transfers, so a restored block regenerates them: either by extending the sync or by
  bounding that regeneration once, and that is an open decision rather than a defect. Neither is a recovery
  limit; both are what a restore costs.
- **It verifies the fourth clause, and refuses without it.** After restarting, the tool compares each height's
  **block-hash set** across the nodes (not their heights, where four nodes at zero are equal), and then
  requires every node to have **finalised past the reconciliation point**, on a block the nodes' own DAGs
  hold — a converged chain that cannot finalise is **not** a recovery, and the run exits 9 saying so rather
  than printing a reading that looks like success. Both assertions, and the four clauses, are in the tool's own
  output: `spec/audit/evidence/n-reconcile-drill/run-4-restore.txt`.
- **Nothing is deleted.** Data directories are moved aside or tarred before anything is removed, and the
  genesis inputs are preserved across it — a joiner without them cannot validate what it pulls.

What this shape gives up is nothing structural: with no stake above a quarter, the tolerance is
symmetric — the property a validator set needs before the *join and leave* questions can be answered on
it. [#214](https://github.com/rhovision-org/rholo/issues/214) closed on 2026-10-04; the
residual join/leave work is
[#242](https://github.com/rhovision-org/rholo/issues/242).

## Genesis

Built once with `scripts/localnet/keys.mjs` — which lives in the **quantum-os** repository, not this
one, as do `scripts/qos-cli/agent.mjs` and `scripts/localnet/pk.txt`. The exact files are on each node:

```
/var/lib/rnode/genesis/bonds.txt       4 lines: <65-byte pubkey> <stake>  (four validators at 250 each)
/var/lib/rnode/genesis/wallets.txt     4 funded REV addresses, 1e12 drops = 10,000 REV each
/etc/rnode/validator.key               node A's validator key        (0600 rnode:rnode)
/etc/rnode-d/validator.key             node D's validator key        (host A runs two validators)
/etc/rnode-c/validator.key             node C's validator key        (host B runs two)
/etc/rnode/faucet.env                  FAUCET_KEY=… — the key node A's faucet signs with (dave's, funded)
```

Genesis hash `713c0ebb0eb4ce866ae118aa1177a498b4edb2d431dd8b77d28abcdd4da9ea91` (four bonds at 250).

**Node ids are deliberately not written down here.** A rebuild regenerates them — both changed twice on
2026-09-29 alone — so a page that pins them is wrong within the hour. Read the live ones per host from
`GET /api/status` → `address`.

The genesis hash depends only on the genesis *inputs* — bonds, wallets, parameters **and the genesis
content itself** — not on either node's identity, so it is stable across rebuilds but changes when any of
those change: the 2026-09-26 rebuilds that signed for one validator all produced `e525129d…`, adding B's
bond moved it to `6a6db0db…`, and the 2026-09-29 rebuild — one bond *and* the content change below —
produced `9f09e7a0…`, and the 2026-10-04 rebuild — four bonds at 250, a cap of 250 and an active set of
4 — produced `713c0ebb…`, and the 2026-10-24 rebuild — the shard renamed from `/root` to `/testnet` — produced
`c596bef4…`, which is the current one. **The old single-bond hash is not reachable again**: #71's fix moved the content,
so the same bond set no longer gives the same block.

### Rebuilding: the four things that bit, measured on 2026-10-24

A rebuild is a fresh data directory and a new genesis. These are the steps that are not obvious, each
one from the day's own failures:

1. **The data directory owns the shard.** `/var/lib/rnode/shard-id` is a marker. A directory stamped
   with one shard refuses to start under another — *"data directory /var/lib/rnode belongs to shard
   '/root', but this node is configured for shard '/playground'; remove the directory or fix
   `casper.shards`"*. Move the old directory aside; do not reuse it, and check the marker is gone
   before starting.
2. **A drop-in can override `ExecStart`.** `rnode.service.d/faucet.conf` does `ExecStart=` followed by
   its own line, so editing the base unit changes nothing — the node keeps running the old binary. Edit
   both, and confirm with `systemctl show -p ExecStart --value rnode.service` rather than trusting the file.
3. **The node identity lives in the data directory** (`node.key.pem`, 0600). A fresh directory generates
   a new `address`, and every joiner's `--bootstrap rnode://<id>@…` is then stale — they retry
   `PeerUnavailable` ten times and exit. Either copy `node.key.pem` into the new directory, or re-point
   the joiners.
4. **A fresh chain cannot bootstrap on the faucet.** `POST /api/faucet` anchors on the last *finalized*
   block, and at genesis nothing is finalized: it answers *"Finalized fringe is not available."* Seed the
   first block with a direct deploy — `rnode --grpc-host <host> --grpc-port 40401 deploy --shard-id
   /testnet --phlo-limit … --phlo-price … <term-file>`, where the term is a **file path**, not a term
   string. Once one block exists, the faucet works.

`rnode-health` is a monitor, not a keepalive: it writes a health snapshot and warns, and produces no
deploys, so it does not keep a quiet chain moving.

**The content half of that list was missing until 2026-09-28, and it is the half that bites.** The
blessed contract set and the governance deploys are genesis *state*, so a change to any of them moves the
hash exactly as a bond change does — and unlike a bond change, nothing about it is obvious to a node
operator. #71's fix is the worked example: publishing the master directory's grant capability (see
[`spec/GENESIS.md`](https://github.com/rhovision-org/rholo/blob/dev/spec/GENESIS.md)) adds a
registered capability and a native registry entry, so **every chain built from that commit onward has a
different genesis hash, and the `e525129d…`/`6a6db0db…` hashes above name chains built before it.** An existing net is
untouched — its genesis is already committed history — but a rebuilt data directory is a *different*
chain, and a node pointed at the old bootstrap will not join it. Any change under
`casper/src/genesis/` belongs on the hard-fork tracker before it lands for this reason.

A node id is **not** derived from the validator key — a rebuilt data directory gets a fresh node
identity, so any `--bootstrap` URI pointing at the master has to be updated after a rebuild, and genesis
artefacts are produced once, at genesis. **This page named both ids until 2026-09-29 and stopped after
they changed twice in one day**; expect to retarget B's `--bootstrap` on every rebuild.

Both nodes start with `--pos-multi-sig-public-keys <dave's pubkey> --pos-multi-sig-quorum 1`, which
puts **dave** — a `wallets.txt`-funded key that can actually pay phlo — into the trusted set at
genesis. That is what makes live admission possible (): dave is the key that can `trust` others.
Give the same list to every node, or a joiner's own view of the genesis PoS spec will not match the
chain it is joining.

**This net sets bond parameters explicitly; none of them is a default.** The shipped defaults are
`bond-minimum 1`, `bond-maximum 9223372036854775807`, `number-of-active-validators 100`,
`epoch-length 10000` and `quarantine-length 50000` (`node/src/configuration/defaults.conf`). This net
runs `--bond-minimum 1 --bond-maximum 250 --number-of-active-validators 4 --epoch-length 10
--quarantine-length 10`. The active set (4) equals the number of bonds, so every bonded validator is
active — no top-N truncation to reason about. `--executor-share`, `--absence-slack` and
`--participation-grace` are also **genesis parameters** and are at their defaults here; every node must
agree on all of them or it will not join.

`--validator-private-key-path` (a file, not a flag value) works because the fix merged
2026-09-21; on older binaries it is silently ignored and the key must be passed inline.

### Genesis wallets

`wallets.txt` funds **four** REV addresses with 1,000,000,000,000 drops (10,000 REV) each, so tooling
already wired to those keys works unchanged and a facilitator can be handed a key with REV to give away.
The four funded addresses include **dave's** (`1111pJu4TJaJDNJDTinnftr2fcHvMfnDeTRXRzwgPfwuKmGMa5juj`);
the `deployer` key is **not** among them, and its vault reads `0`.

| key | role on this net |
|---|---|
| `dave` (`7707a3e0…`) | **the faucet's signing key** — funded, supplied through `/etc/rnode/faucet.env` — and the trusted key seeded by `--pos-multi-sig-public-keys`, so it is the key that can `trust` newcomers |
| `deployer` (`3554e876…`) | **not funded on this genesis.** This page used to call it the faucet's key; drips signed with it failed the phlo pre-charge while the endpoint still answered success — root cause and fixes in [#247](https://github.com/rhovision-org/rholo/issues/247) |
| the developer keys in `scripts/localnet/pk.txt` | funded accounts for tooling; not part of the net's operation |

Throwaway development keys, published on purpose. Never use them for anything real. Users are not sent
here — they get REV from the faucet.

## Operating the nodes

```bash
systemctl status rnode                          # node A (also: rnode-d on host A, rnode-c on host B)
systemctl restart rnode
journalctl -u rnode -n 50                       # `systemctl log` is not a command
systemctl list-timers rnode-health.timer        # monitoring
journalctl -t rnode-health -n 20                # health warnings only
```

Rebuild / re-key (the whole network):

```bash
# node A — the genesis master. These are the flags it actually runs (from its unit file);
# the bond parameters are genesis inputs, so changing any of them gives a different genesis.
rnode --profile docker run -s --dev-mode --propose-on-deploy --no-upnp --network-id testnet \
  --host 164.90.140.144 \
  --protocol-port 40400 --api-port-grpc-external 40401 --api-port-grpc-internal 40402 \
  --api-port-http 40403 --discovery-port 40404 --api-port-admin-http 40405 \
  --data-dir /var/lib/rnode \
  --bonds-file /var/lib/rnode/genesis/bonds.txt \
  --wallets-file /var/lib/rnode/genesis/wallets.txt \
  --pos-multi-sig-public-keys <dave pubkey> --pos-multi-sig-quorum 1 \
  --epoch-length 10 --quarantine-length 10 \
  --bond-minimum 1 --bond-maximum 250 --number-of-active-validators 4 \
  --validator-private-key-path /etc/rnode/validator.key
# a joining validator: same flags minus -s, its own ports/data-dir/key, plus
#   --bootstrap rnode://<A's id from /api/status>@164.90.140.144?protocol=40400&discovery=40404
# D: ports 414xx, --data-dir /var/lib/rnode-d, --validator-private-key-path /etc/rnode-d/validator.key
# B: host B, ports 404xx, /var/lib/rnode, /etc/rnode/validator.key
# C: host B, ports 414xx, --data-dir /var/lib/rnode-c, /etc/rnode-c/validator.key
#
# --attest-on-new-blocks is ON by default; the control is the opt-out --no-attest-on-new-blocks.
```

There is **no `--no-autopropose` flag** — you omit `--autopropose`. (`tools/devnet.sh` accepts
`--no-autopropose` because that is *its* CLI; it only omits the node flag.) Passing it makes the
node exit 1 in a restart loop. The production-mode matrix and the finality consequences of an idle chain
are in [Operating the node](operating.md#block-production-modes).

## Adding a node (observer)

An observer is any node without a bonded key. It replicates the chain and can be started
anywhere:

```bash
rnode --profile docker run --host <its-ip> --data-dir /var/lib/rnode \
  --pos-multi-sig-public-keys <dave pubkey> --pos-multi-sig-quorum 1 \
  --bootstrap rnode://<A's node id from /api/status>@164.90.140.144?protocol=40400&discovery=40404
```

(The multi-sig flags must match the genesis master's, or the joiner's own view of the genesis PoS
spec will not match — see [Genesis](#genesis).)

Success looks like this in the log — note the LFS step, which restores from the **approved genesis**
fringe. That is why joining works even though this idle chain has no *last-finalised* fringe:

```
INFO [casper.engine.NodeSyncing] Blocks for approved state added to DAG.
INFO [casper.engine.NodeSyncing] LFS state is successfully restored.
INFO [casper.engine.NodeLaunch] Making a transition to Running state.
```

A join takes about 15 seconds and ~19 MB, measured.

**Measured 2026-10-25: this is what a join looks like when there is nothing to catch up on.** Every
node above was at height 1, so "restoring LFS state" had only the approved genesis fringe to fetch.
Against the chain at height 28, a fresh data directory with `--bootstrap` receives the finalized
fringe, then all 29 history and 235 data store items, then **every block #0–28** — and applies none of
them. It sits at `h=0` while the bootstrap logs `Response sent.` to all 28 block requests, and after
three 30-second idle rounds it reports:

```
ERROR [casper.engine.NodeSyncing] LFS state sync failed (attempt 1 of 3):
      no block finished in 3 consecutive idle rounds of 30s
```

Reproduced on both `185ed283` and `2745dbb3`, so it is the join path, not a regression from the
2026-10-24 merges. **A fresh node cannot join this chain today.**

Until that is fixed, seed the observer from a synced node and let it reconnect:

```bash
# on the synced node (A) — the chain is ~1.3 MB at 109 blocks
tar czf /tmp/seed.tgz -C /var/lib/rnode --exclude=node.key.pem --exclude=node.certificate.pem .
# on the new node
tar xzf /tmp/seed.tgz -C /var/lib/rnode-c
```

Excluding the two identity files is the load-bearing part: keeping them would hand the observer the
seed node's node id, and peers would see two nodes claiming one identity (and every joiner's
`--bootstrap` would then be ambiguous). With a fresh identity it logs `Reconnecting to existing
network… republished CURRENT_ROOT at the latest accepted block <hash>` and follows from there —
verified by a faucet deploy that took the net 28 → 29 and the observer with it.

## Adding a validator — what to watch

Adding one is supported. The three blockers that once justified refusing it are fixed, and the join/leave
rows A2.1–A2.4 pass on this net; the evidence is
[the acceptance specification](../spec/testnet-acceptance.md) §3.2, which owns that question.

What still bites when a validator is added, and is worth knowing before you try:

- **a bonded key needs a running node.** The pool counts its stake whether or not anything is producing
  with it, so a bonded key with no node dilutes everyone who is contributing rather than merely failing to
  help. Run that node with `--propose-on-deploy`.
- **the pool shape decides how much can be lost.** Each of the four holds 250 of 1000; a joiner at the cap
  takes the pool to 1250 with everyone at 20 %, which still tolerates any single loss. There is no
  inactivity leak and no eviction, so an absent validator's stake goes on counting until it speaks again.
- **two funding prerequisites and one ordering rule**, all in
  [Onboarding an observer into the validator pool](#onboarding-an-observer-into-the-validator-pool): the
  trusted key pays for its own `trust` deploy, the newcomer's vault must cover its stake, and only a
  trusted key can confer `trust`.

## Onboarding an observer into the validator pool

The generalised procedure — the admission routes, the funding prerequisites and a verified transcript — is
in [Running a public testnet of your own](running-a-public-testnet.md).
What follows is this net's version, with the keys and addresses actually in play here.

The implementation models the full lifecycle natively (`rholang/src/native_state.rs`):

> **observer** — any key that is not bonded · **trusted** — admission into the validator
> stakeholder group; only a trusted key may bond · **bonded / pool** — a bond within
> `[minimum, maximum]`, deducted from the validator's REV vault · **active** — the consensus set,
> recomputed only at an epoch boundary: the whole eligible pool when it fits under
> `number_of_active_validators`, otherwise a seeded **stake-weighted** draw from it (`select_active` —
> proportional to stake, *not* a ranking) · **withdrawing** — deactivation, stake escrowed until the quarantine deadline ·
> **removed** — `slash`/`untrust`, stake confiscated. What each state earns and risks:
> [Validator economics](validator-economics.md).

Bonding is done through the `rho:rchain:pos` **system process** (native methods `bond`,
`withdraw`, `trust`, `untrust`, `getBonds`, `getActiveValidators`). There is no CLI or HTTP
endpoint for it: it is a deploy. `bond` takes the *caller's own* `rho:rchain:deployerId` as an
unforgeable capability, so **a key can only bond itself** — nobody can bond it on its behalf.

`native_state.rs::bond` enforces, in order:

| check | failure string |
|---|---|
| not already in pool/active | `Public key is already bonded.` |
| **is trusted** | `Validator is not trusted: observer admission is required before bonding.` |
| `minimum ≤ stake ≤ maximum` | `Bond is less than minimum (…)` / `greater than maximum (…)` |
| `vault_balance ≥ stake` | `insufficient funds to bond … (have …)` |

### Two admission routes

**(a) At genesis** — `bonds.txt`, plus `--pos-multi-sig-public-keys` to pre-trust keys that are
not themselves bonded. `trusted` is seeded as *genesis bond keys ∪ that list*. Changing either
means a **new genesis and a new chain**.

**(b) Live, on a running chain** — a trusted key confers trust, then the newcomer bonds:

```
1. a trusted key deploys        pos!("trust", [*deployerId, "<newcomer 65-byte pubkey>".hexToBytes(), *ret])
2. the newcomer deploys         pos!("bond",  [*deployerId, <stake>, *ret])      # 1..250 here
```

Two funding prerequisites, both easy to miss and both **verified working here** (the general form, with
the reasoning, is upstream):

- the **trusting key must hold REV**, because it pays for the `trust` deploy's phlo from its own vault. A
  genesis-trusted key that is not in `wallets.txt` cannot deploy at all — which is why **dave** is the
  trusted key on this net (`--pos-multi-sig-public-keys`);
- the **newcomer must hold REV ≥ stake**, because the bond is deducted from its vault.

Funding either one is an ordinary transfer. There is **no `pos` method to read a vault balance**: the
native dispatcher implements `getBonds`, `getActiveValidators`, `getTrusted`, `getDelegations`,
`bond`, `withdraw`, `trust`, `untrust`, `delegate` and `undelegate` — and **no balance read** — so
`pos!("getBalance", …)` fails with
`pos: unknown method getBalance` (`rholang/src/system_processes.rs`). A balance read has to go through
the REV vault contract, or a client macro that wraps it — not `pos`. The terms this net uses are below.

### The exact terms

Every term must bind the names it uses — a raw deploy/eval does **not** get `return` for free
(the browser and macro path adds it via `wrapProgram`, which merges
`new return, stdout(\`rho:io:stdout\`), …zfa/grant/verify/fuse… in { … }`). Omitting it fails
with `Top level free variables are not allowed`.

```
// read the pool (works today)
new return, pos(`rho:rchain:pos`), ret in {
  pos!("getBonds", [*ret]) | for (@b <- ret) { return!(b) }
}
// → four entries, one per validator, each {"ExprInt":250}

// read the consensus set (works today)
new return, pos(`rho:rchain:pos`), ret in {
  pos!("getActiveValidators", [*ret]) | for (@v <- ret) { return!(v) }
}
// → {"expr":[{"ExprSet":[…four ExprBytes…]}]}

// confer trust on a newcomer (deploy signed by a trusted, funded key)
new return, pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("trust", [*deployerId, "<65-byte hex pubkey>".hexToBytes(), *ret]) |
  for (@r <- ret) { return!(r) }
}

// bond yourself (deploy signed by the newcomer; stake 1..250 on this net)
new return, pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("bond", *deployerId, 100, *ret) | for (@r <- ret) { return!(r) }
}

// withdraw (a request: the bond leaves the pool at the next epoch boundary, then is escrowed)
new return, pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("withdraw", *deployerId, *ret) | for (@r <- ret) { return!(r) }
}
```

Deploy them with:

```bash
# `--grpc-host`/`--grpc-port` are GLOBAL options, so they go BEFORE the subcommand. Put them after
# it and the CLI exits with `unexpected argument '--grpc-host' found` and prints jemalloc statistics
# on the way out, which reads like a crash. On the node's own host, omit both.
rnode --profile docker --grpc-host 164.90.140.144 --grpc-port 40401 deploy \
  --phlo-limit 90000 --phlo-price 1 --shard-id /testnet --private-key <hex> term.rho
# (D: 41401 on host A.  B: 40401, C: 41401, both on host B.)
rnode --profile docker --grpc-host 164.90.140.144 --grpc-port 40401 deploy-status \
  --deploy-signature <deployId>
```

Two easy-to-miss details:

- `--shard-id /testnet`, or the node answers
  `Deploy shardId '' is not a member of this node's shards: [/testnet]`;
- `--valid-after-block-number <current height>` **only on a binary built before 2026-09-21** — the
  current testnet binary is anchored at the node's height automatically (). If `deploy-status`
  answers `notProcessed / Unknown`, the deploy was swept from the pool as expired.

## Status

| | |
|---|---|
| Shape | four bonded validators at 250 each, pool 1000, two per host, all four attesting |
| Losing one | **survivable** — with A stopped the other three held 75 % and kept finalising (f 65 → 90), and A was level with the tip again in under 20 s |
| Losing two | 50 % is not a quorum, so finality stops until one of the two speaks again *(arithmetic on this shape, not measured)* |
| Losing all four | production stops, state sits on disk; start them and the chain resumes |
| Last verified on this chain | 2026-10-06 — rebuilt onto one build, genesis reproduced, producing and finalising, faucet delivering, bond map 4 × 250 |

The measurements behind each row, the blockers that were fixed to make them true, and the regressions
found along the way live in the tracker, not here: [#39](https://github.com/rhovision-org/rholo/issues/39),
[#58](https://github.com/rhovision-org/rholo/pull/58), [#60](https://github.com/rhovision-org/rholo/issues/60),
[#68](https://github.com/rhovision-org/rholo/issues/68), [#70](https://github.com/rhovision-org/rholo/issues/70),
[#105](https://github.com/rhovision-org/rholo/issues/105), [#148](https://github.com/rhovision-org/rholo/issues/148),
[#223](https://github.com/rhovision-org/rholo/issues/223),
[#247](https://github.com/rhovision-org/rholo/issues/247), and
[the acceptance specification](../spec/testnet-acceptance.md).

## Using it from quantum-os

The room agents and the playground both take a node URL, so pointing them at this net is one flag:

```sh
# the qos-cli agents (facilitator, observer) talk to whichever node the room is on
node agent.mjs --rnode https://testnet.rhobot.net …

# the playground's console takes the same thing interactively
/rholang rnode https://testnet.rhobot.net
```

That is the whole integration. The agents sign with a local key and read each term's return value out of
the registry result slot — the same path the verified transcripts on this page were produced with.

## Housekeeping

- Snapshots/rollback: unit files are backed up in place (`rnode.service.bak-<epoch>`), node binaries as
  `/usr/local/bin/rnode.old-<sha>`, and each rebuild left the previous data dir as
  `/var/lib/rnode.bak-<timestamp>`. The genesis files are the source of truth and are tiny; those old
  data dirs (4.5 MB each) can be deleted once the new chain is confirmed.
- Firewall: `ufw` allows `22`, `40400`, `40401`, `40403`, `40404`, `40405`, **the `414xx` family
  (`41400`–`41405`, with `41404/udp`)**, plus `80`/`443` on A. The `414xx` ports are what nodes D and C
  listen on and what the deploy table above publishes.
  Nothing else — the old rhobot box's 36-rule ruleset was pruned to what actually has listeners.
- Certificates renew via `certbot.timer` on A (nginx authenticator), first expiry 2026-12-20.
- To move the testnet to another host: copy `bonds.txt`, `wallets.txt`, the validator key and the
  static musl `rnode` binary. The binary is self-contained (no Docker, no runtime deps).

**K8 — a deploy's write was lost at an epoch boundary, and nothing said so. FIXED 2026-10-08; a hard fork, so this chain needs a rebuild to carry it.**

On 2026-10-08 a deploy landed in block 100, which is an epoch boundary on this net
(`epoch_length = 10`) and therefore a round in which **every** validator's block runs `close_block`.
Its write was `processedWithSuccess` and was in the state at heights 100, 101 and 102 — including the
finalised block — and absent from 103 onward, **on every node at once**. No peer refused a block and
no node disagreed with another, which is why the first hypothesis (C215, two nodes merging the same
justifications to different pre-states) was wrong: the disagreement was between the chain and its own
past, because the same round resolves differently once finality advances past it.

The cause was the merge's **native** relation. It was keyed on the *host block* — the union of every
chain that rode in on it — and a rejection was closed over the whole block
(`reject_whole_blocks`). At a boundary that makes every concurrent pair conflict, including the pairs
whose user chain wrote no contended slot at all, and the user chain then died with its host. The
merge had no logger and the whole-block rule fired with no counter, so the only visible trace was the
deploy being absent from a state that reported it as processed.

**What an operator sees next time.** The relation is now on each chain's own writes, the whole-block
rule is **deleted**, and the merge **returns** a report — the slots a rejected chain would have
written and who won them, plus two invariants that must stay empty — which the validation path logs
whenever the merge is not quiet. First thing to read on a stall or a state surprise:

```sh
journalctl -u rnode | grep 'merge:'     # one line per merge that rejected anything
```

**Two things this fix does not do.** It does not repair the chain it happened on — three of the four
validators agreed on the pre-state that dropped the write, so the write is gone above height 102, and
recovery is a rollback below height 100 or accepting the loss. And it is a **hard fork**:
`rejected_deploys` is part of the hashed block body, so any block whose scope held a contended
boundary hashes differently — the running chain must be rebuilt from genesis to carry it
(`#51`, category A). The account, the raw readings and the diagnosis:
[#280](https://github.com/rhovision-org/rholo/issues/280) and
`spec/audit/evidence/n280-merge-loses-a-write-results.md`.
