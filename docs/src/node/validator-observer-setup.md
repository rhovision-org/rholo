# Running a node: testnet now, mainnet later — validator and observer

This is the on-ramp: what to install, what to open on the network, how to create or join a chain, and how
to become — or stop being — a validator. It covers a public **testnet** today and a **mainnet** later, and
it separates the two roles: an **observer** follows the chain, a **validator** takes part in consensus.

It links to the detail rather than copying it: sizing in [the hardware page](validator-requirements.md),
the boundary in [ringfencing](resource-ringfence.md), pay and penalties in
[validator economics](validator-economics.md), the live net's record in [the public testnet](testnet.md),
and the join's trust boundary in [what a joining node trusts](../spec/trust-model.md).

## 1. The two roles

| | Observer | Validator |
|---|---|---|
| Has a validator key | optional | **yes** — the key is the identity |
| Bonded stake | none | within `[minimum, maximum]`, from its own vault |
| Proposes / attests | no | yes, once active |
| Counts in the >2/3 denominator | no | **yes** — an active validator that is down is silent stake |
| Can read the chain, serve the API, relay blocks | yes | yes |

Both run the same binary and sync the same way. The difference is admission: a validator must be
**trusted** (admission list, `pos:trusted`) and then **bond itself**. Admission is not consensus
membership — `untrust` does not confiscate a bond or remove an active seat (Law 82); the
boundary re-selects the active set each epoch. The lifecycle is
`observer → trusted → bonded → active → withdrawing → claimable`.

## 2. Before you start

- **Hardware.** The sizing table is [here](validator-requirements.md). Read the distinction it makes:
  the headline numbers include the **start-up replay floor**, and a node replaying a maturing chain needs
  headroom for that replay, not just for steady state. A devnet running in 1–4 GB is a devnet; it is
  below the advised validator minimum.
- **A cgroup and a disk of its own** — [ringfence the node](resource-ringfence.md). The shipped start-up
  preflight refuses a below-minimum **validator** and warns an **observer**. A below-floor validator
  needs both `--allow-under-minimum` and `--accept-under-minimum-risk`; read the
  [operator risk notice](https://github.com/rhovision-org/rholo/blob/dev/TERMS.md) and [validator risk](operator-risk.md) first.
- **A validator key you can keep.** It is a file, not an argument — see §4.
- **A reachable host.** §3 is the part that is usually missed.

## 3. Network and firewall — accept inbound from **every peer**

| Service | Port | Who reaches it |
|---|---|---|
| protocol (TLS peer transport) | 40400 | **every participating node**, inbound and outbound |
| discovery (Kademlia) | 40404 | **every participating node**, inbound and outbound |
| gRPC API — Deploy | 40401 | clients |
| gRPC API — Propose / Repl | 40402 | **loopback only** |
| HTTP API | 40403 | clients |
| admin HTTP | 40405 | private; never expose it |

A second node on one host uses an offset (`41400`–`41405`, discovery UDP `41404`); see
[the testnet's port notes](testnet.md#housekeeping) for the exact rule.

### Publishing the node hostlist (optional)

A node can tell wallets which other nodes can take their deploys: `GET /api/v1/nodes` returns the
live peers it has probed and found deploy-capable. It is **off by default**, because it discloses
peer addresses to any HTTP client:

```hocon
api-server {
  publish-node-list = true
  node-list-max-entries = 32
  node-list-probe-interval = 60 seconds
  node-list-probe-timeout = 2 seconds
}
```

or `--api-publish-node-list`. Two things make a node **listable** by its peers, and both follow from
the probe rather than from a setting:

- its HTTP API (40403) must be reachable from the peers, on the default layout
  (`HTTP = protocol + 3`, e.g. 40400 → 40403, 41400 → 41403);
- it must hold a validator key (`proposer: true` in `/api/v1/status`) — an observer is never served,
  because a deploy sent to it would sit in its pool rather than be included.

A node behind a reverse proxy on 443, or on a non-standard port layout, can publish a list but will
not appear in its peers' lists. The list is advisory: peers report their own state, and a wallet
re-verifies each entry before use.

**The bootstrap is not the only peer that must be able to dial you.** A joiner that admits inbound only
from its `--bootstrap` receives the fringe, the store items and the block gossip, and **applies none of
them**: it sits at height 0 with a healthy peer count and fails after three idle rounds. Measured
2026-10-25 with the same binary and chain, changing only the firewall:

| inbound allowed from | result |
|---|---|
| every peer | joined, reached the tip, followed the next deploy |
| the bootstrap only | stuck at `h=0`, `peers=6`, `LFS state sync failed` |

So open **40400 and 40404 inbound from every validator and observer**, and open outbound to them too.
The bootstrap must additionally be reachable outbound for the fringe request. `peers=N` on
`/api/status` does not mean those peers can answer you. If a host genuinely cannot accept inbound, the
fallback is to **seed it from a synced node** (copy the data directory excluding `node.key.pem` and
`node.certificate.pem`) rather than to admit only the bootstrap.

The join path's fixes are recorded in Law 81; the remaining live arms — the
blocked-inbound regression and the fresh join on the current binary — are tracked in
[Incident history](incident-history.md#testnet-rebuild-and-receiver-exhaustion).

## 4. Validator identity

The key is read from a **file**, never from the command line (an argv hex is visible to any local process
and is gone from the CLI; see [operating](operating.md#standalone-genesis-prerequisites)):

```sh
install -m 600 /dev/null /etc/rnode/validator.key
printf '%s' '<hex private key>' > /etc/rnode/validator.key
rnode ... --validator-private-key-path /etc/rnode/validator.key
```

Keep file permissions at `0600`, keep the admin listener on loopback, and treat the node's data directory
as sensitive: the TLS identity (`node.key.pem`, `node.certificate.pem`) and the validator key live there.
The wargame's key-lifecycle row (in [the bounded adversary](../spec/adversary.md)) is the threat
model for replacement and custody.

## 5. Creating a network (the genesis master)

A genesis is created **once**, by one node, into an empty DAG, and then distributed. `rnode run -s`
(standalone) is that master. Two files are the source of truth and must be kept:

```sh
install -d -m 755 /var/lib/rnode/genesis
echo "<65-byte uncompressed pubkey> 1000" > /var/lib/rnode/genesis/bonds.txt   # the founder
echo "<second founder pubkey> 100"       >> /var/lib/rnode/genesis/bonds.txt   # optional
: > /var/lib/rnode/genesis/wallets.txt    # parsed strictly: must exist, may be empty

rnode --profile docker run -s --propose-on-deploy --no-upnp --host <public-ip> \
  --data-dir /var/lib/rnode \
  --bonds-file /var/lib/rnode/genesis/bonds.txt \
  --wallets-file /var/lib/rnode/genesis/wallets.txt \
  --pos-multi-sig-public-keys <funded governance pubkey> --pos-multi-sig-quorum 1 \
  --validator-private-key-path /etc/rnode/validator.key
```

Give the founding validator **more than two thirds of the pool at genesis** (for example 1000 against
100): a genesis the founders cannot finalise alone is a network that needs every founder up before its
first boundary.

**The master's validator key must be the first entry in `bonds.txt`.** The ceremony's identity — the
block's sender, the blessed deploys' signer and the governance admin — is that bond, so the genesis is a
function of the inputs (Law 84, #456). A node whose key is not first **refuses** to create a genesis and
names both keys instead of starting a private chain.

The command above is a **testnet** ceremony: add `--dev-mode` (as the devnet does) for a disposable
network. A **production** genesis runs without `--dev-mode`; it then credits a developer allocation, and
the recipient is a genesis parameter:

- `casper.genesis-block-data.dev-tax-address` — empty in dev mode (no allocation); outside it an empty
  value resolves to the repository's `PRODUCTION_DEV_TAX_ADDRESS` (`casper/src/genesis/dev_tax.rs`),
  and an explicit value wins for a network that names its own recipient. Every node replaying the
  genesis must use the identical value. The **amount** is derived from the wallets file
  (`max(20 000 000 REV, 2% of its total)`), not configured.
- `dev-tax-vesting-blocks` — the unlock term. 36 months is a *block count* because the block cadence is
  not a constant; set it from the measured cadence at the ceremony.

Two command-line choices decide whether live admission will work later:

- `--pos-multi-sig-public-keys` seeds `trusted` = the genesis bond keys ∪ this list. Make the governance
  key **funded** in `wallets.txt`: it pays phlo for its own `trust` deploy, and a mere bond key is not
  funded. `--pos-multi-sig-quorum` must be ≤ the number of keys.
- **Every node must be given the same genesis parameters** — bonds, wallets, multi-sig list and quorum,
  bond bounds, epoch and quarantine lengths, participation and executor shares. They are written into the
  genesis post-state; a node that omits one computes a different genesis and refuses every block above it.

Record the **genesis hash** and the master's **node id** (`/api/status` → `address`) beside the two
files. They change when the genesis changes.

## 6. Joining a running chain (observer first)

A joiner receives the approved fringe from a bootstrap and restores the state over LFS. It must be given
the **same genesis inputs** as the master (the files and parameters above):

```sh
rnode --profile docker run --propose-on-deploy --no-upnp --host <its-public-ip> \
  --data-dir /var/lib/rnode \
  --bonds-file /var/lib/rnode/genesis/bonds.txt \
  --wallets-file /var/lib/rnode/genesis/wallets.txt \
  --pos-multi-sig-public-keys <same list> --pos-multi-sig-quorum 1 \
  --bootstrap rnode://<master-node-id>@<master-host>?protocol=40400&discovery=40404 \
  --validator-private-key-path /etc/rnode/validator.key
```

Success is these lines, then the node serving:

```
INFO [casper.engine.NodeSyncing] Blocks for approved state added to DAG.
INFO [casper.engine.NodeSyncing] LFS state is successfully restored.
INFO [casper.engine.NodeLaunch] Making a transition to Running state.
```

A fresh chain restores in seconds; a **mature** chain takes longer and needs the inbound rule of §3. If
the sync fails, check the three usual causes in order: inbound reachability from every peer, the
bootstrap's node id and host, and identical genesis inputs. The failed attempt's log now names the
outstanding hashes and the peers they were requested from.

Once running, the node is an **observer**: it follows and serves, but does not vote.

## 7. Becoming a validator (live admission)

**Sync first, then bond.** A bond is a chain-state change, but the node decides whether it may propose
from the newest block in its **own** DAG: one that has not restored the chain is told `ReadOnlyMode`. A
bonded node that is not up is silent stake — it counts in the >2/3 denominator and can stall finality
for everyone, in the worst case until it produces a block.

The order is: start the node → wait for `LFS state is successfully restored` → confirm it follows →
`trust` from a trusted, funded key → fund the newcomer → `bond` from the newcomer → wait for the epoch
boundary → it becomes active. An admission tool that applies this order is
[`scripts/qos-cli/admit.mjs`](https://github.com/rhovision-org/quantum-os/blob/main/scripts/qos-cli/admit.mjs). Everything is a deploy to `rho:rchain:pos`; there is no CLI or HTTP command
for admission.

`bond` is **self-only** — it takes the caller's own `rho:rchain:deployerId`, so nobody can bond another
key. It enforces, in order:

| check | failure |
|---|---|
| not already pooled/active | `Public key is already bonded.` |
| is trusted | `Validator is not trusted: observer admission is required before bonding.` |
| `minimum ≤ stake ≤ maximum` | `Bond is less than minimum (…)` / `greater than maximum (…)` |
| vault covers the stake | `insufficient funds to bond … (have …)` |

The terms (bind every name; a raw deploy does not get `return` for free):

```rholang
// read the pool / the consensus set
new return, pos(`rho:rchain:pos`), ret in {
  pos!("getBonds", [*ret]) | for (@b <- ret) { return!(b) }
}
new return, pos(`rho:rchain:pos`), ret in {
  pos!("getActiveValidators", [*ret]) | for (@v <- ret) { return!(v) }
}

// confer trust (signed by a trusted, funded key)
new return, pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("trust", [*deployerId, "<65-byte hex pubkey>".hexToBytes(), *ret]) |
  for (@r <- ret) { return!(r) }
}

// bond yourself (signed by the newcomer; stake within [minimum, maximum])
new return, pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("bond", [*deployerId, 100, *ret]) | for (@r <- ret) { return!(r) }
}

// withdraw: it **stages** a request. The validator stays bonded and active, still earning, until the
// boundary after the quarantine escrows the bond; a later boundary pays it with the committed rewards.
new return, pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("withdraw", [*deployerId, *ret]) | for (@r <- ret) { return!(r) }
}
```

Funding is where testnets stall: the trusting key pays phlo for `trust`, and the newcomer needs
`REV ≥ stake` for the bond. Fund with `revVault!("transfer", …)`; read with
`revVault!("getBalance", "<address>", *ret)` (the method is `getBalance`, not `balance`). Deploy with the
shard named and read the reply out of the term, not only from `deploy-status`
([operating](operating.md#rnode-deploy-and-deploy-status)).

## 8. Monitoring

- `GET /api/status` — `latestBlockNumber` (rounds, not blocks), `sync` (non-null means the LFS sync task
  is running and names its last failure), `acceptsDeploys` (the deploy pool's count/byte capacity),
  `degraded` (the **advisory** fallback word; it does not gate admission).
- `GET /api/last-finalized-block` — finality, the number that matters for consensus health.
- `GET /health` — **a node route** (200 while serving): `ok`, `degraded`, `acceptsDeploys`, height,
  pool, and the peer-liveness fields `finalityStalled`/`finalityStallEpisodes`. The public testnet's
  `testnet.rhobot.net/health` is a separate host snapshot; do not confuse the two.
- `GET /api/deploy-status/<id>` — submission/execution/inclusion; read the deploy's own reply for
  contract outcomes.
- The **operator** half (`/api/v1/status/operator`, numeric headroom, fallback reason, sync attempts) is on
  the admin listener — keep it private.

Alert on: a finality number that stops while blocks continue, a non-null `sync` that persists, a
`degraded` word sustained below `normal` (a host below the specification), `acceptsDeploys: false`
with pending work that does not drain, and a validator that is active but not producing. Do **not**
alert on `degraded: throttled` alone: a node on an undersized host is expected to say so while it
keeps serving.

**Under memory pressure the pool is the throttle.** A deploy with a free pool slot is accepted even
when `throttled`; a full pool, a deployer share or a rate is **429 + `Retry-After`**; explore has its
own permit budget. A **503** means the node cannot serve at all. If the pool is full, work drains as
blocks include it, and with `RNODE_DEPLOY_RELAY=1` an admitted deploy is offered to peers' deploy
service so a constrained node is not the only proposer of work it holds.

## 9. Restart, upgrade, recovery

- **Restart** replays state before the API opens (measured ~0.2 s and 0.25 MB per block; ~3.5 min at
  1,140 blocks). During that window `systemctl is-active` says the unit is running while the API does not
  answer — that is a replay, not a wedge. Size for the replay floor (§2), not steady state.
- **Upgrade** the binary only on a node whose data directory you can back up; the store has a format
  guard, and an older binary refuses a newer store rather than corrupting it.
- **A mature-chain joiner** needs §3's inbound rule; a host that cannot provide it is seeded from a
  synced node.
- **A diverged chain or a lost node**: the reconciliation utility can restore a node from a master
  (`reconcile-network.sh --restore-from-master`); the live net's runbook is
  [here](testnet.md#recovery). Self-reset (`casper.self-reset`) is an operator switch, off by default,
  and refuses where it cannot name a safe anchor.
- **Never** restart the whole network expecting it to re-derive a genesis: the genesis is the anchor, and
  a node configured to create one where a chain exists is a fork.

## 10. Security

- The validator key and the TLS identity are the node's credentials: file permissions `0600`, no argv
  hex, no public admin listener, no shared keys between nodes.
- Treat testnet keys as disposable; treat mainnet keys as custody. The production developer allocation
  recipient is the repository constant `PRODUCTION_DEV_TAX_ADDRESS` unless the ceremony names one; it
  is a genesis parameter, so every replaying node must use the same value. Dev mode has no allocation.
- Give the node its own cgroup and disk; a host-wide OOM is a consensus event, not an inconvenience.
- Do not accept inbound from the internet to 40402 or 40405.

## 11. Testnet now, mainnet later

| | Public testnet | Future mainnet |
|---|---|---|
| Mode | `--dev-mode` (disposable) | production (no `--dev-mode`) |
| Faucet | available | **none** — REV comes from the genesis wallets file only |
| Developer allocation | usually absent | **required**, repository production recipient unless the ceremony names one |
| Genesis | re-created when the net is rebuilt | **fixed**; joiners must authenticate it (a pinned genesis hash is the planned guard) |
| Keys | throwaway, funded with test REV | custody-grade, offline where possible |
| Hardware | steady-state class is enough | comfortable class; plan for replay headroom |
| Vesting | default block count | set from the measured cadence at the ceremony |
| Governance | test multi-sig key | real multi-sig keys and quorum |

A mainnet genesis **cannot be recomputed from config**. The chain is handed the block and the block is
the anchor; the rebuild-from-config shortcut is not a mainnet procedure.

## 12. Checklist before you call it running

- [ ] inbound 40400/40404 open **from every peer**, outbound open to them
- [ ] validator key at `0600`, path passed, admin on loopback
- [ ] genesis inputs identical on every node (files, multi-sig list/quorum, all parameters)
- [ ] LFS restore reached `LFS state is successfully restored`, then `Running`
- [ ] `/api/status` shows `sync` null and `acceptsDeploys` true (`degraded` may be `throttled` on an
      undersized host; that is the advisory word, not a refusal)
- [ ] `/api/last-finalized-block` advances
- [ ] for a validator: synced before bonding, trust from a funded key, self-bond, active after a boundary
- [ ] the genesis hash and node ids are recorded beside the genesis files

## See also

- [Operating the node](operating.md) — flags, deploy, finality, PoS reads
- [Running a validator: hardware requirements](validator-requirements.md)
- [Ringfencing a node](resource-ringfence.md)
- [Validator economics](validator-economics.md)
- [The public testnet](testnet.md) — the live net's record, recovery and onboarding
- [What a joining node trusts](../spec/trust-model.md)
- [Bounded adversary](../spec/adversary.md) — key custody, host trust, upgrade and DR
- [Local devnet](devnet.md) — the disposable Docker network, not this procedure
