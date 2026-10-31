# Local devnet (Docker)

A local Docker **testnet** for deploying and testing rholang smart contracts, driven by the
`tools/devnet.sh` script. It brings up 1–3 bonded validators (full consensus, autopropose) plus optional
unbonded observers, seeds genesis with a funded deployer wallet, and exposes `deploy`/`query` helpers.

The same script's `up --nodes N` mode is the bare 1–5 node *network-topology* harness (no autopropose,
no deployer wallet, no deploy helpers) — see [Operating the node](operating.md).

## Growth, and the measurement volumes

**Autopropose mints a block every 2 s with no ceiling, deliberately.** On a local testnet the chain
length is the variable being varied — a long measurement chain exists only because a devnet was left
running — so the timer is documented rather than capped (`node/src/runtime/node_runtime.rs`, `AUTOPROPOSE_INTERVAL`). The practical
consequence is that a devnet left up grows ~1,800 blocks an hour, and the cost of that growth is on
`/metrics` rather than hidden: the DAG publishes its own gauges (`rchain_dag_messages`,
`rchain_dag_seen_entries`, `rchain_dag_fringe_states`, `rchain_dag_index_entries`,
`rchain_dag_logical_bytes`), so a node's footprint can be read off the running process.

Volumes, and what each is for:

| volume | what it is |
|---|---|
| `devnet-stale-snapshot` | the **recorded long chain**, mounted by `up --data-volume devnet-stale-snapshot`. It is a live chain: every run that uses it extends it (5,844 blocks when recorded; 6,339 after the 2026-09-24 serving-term runs), so a measurement quoting a height should say which one it measured |
| `devnet-bootstrap-data`, `devnet-validator-{1,2}-data` | the standard working set: `up` reuses them (each rebuilds its accumulated chain) and `--fresh` discards them |
| `devnet-perf-boot` | an earlier measurement's bootstrap store, kept for comparison |
| `perfsync-validator-{1,2}-data` | created by `DEVNET_PREFIX=perfsync up …` for the fresh-peer measurement; **disposable** — `DEVNET_PREFIX=perfsync tools/devnet.sh down -v` removes them and their network |

`up` prints which volumes it is reusing every time, because reusing an accumulated chain silently is how
one measurement is mistaken for a hang.

> **Security.** The validator and deployer keys baked into the script are throwaway dev keys for a
> *local* testnet only. Never reuse them for anything with real value.

## Prereqs

- `docker`
- `openssl` (to read the bootstrap node-id from its generated TLS certificate)

Every container `up` starts is given a **4 GiB memory ceiling with swap off** (`DEVNET_NODE_MEMORY`
overrides the size, an empty value opts out). The ceiling is there so a node that runs away is
OOM-killed inside its own cgroup instead of dragging the host into swap. It **does** bind under load:
at `4g` a four-way deploy storm can OOM-kill nodes (exit 137) while the image sits at ~132 MiB and a
node mid-storm is nothing like it. So a measurement that needs its nodes to *survive* a storm should
raise `DEVNET_NODE_MEMORY`, lower the storm's concurrency, or record the OOM kill as a result. CPU is
left uncapped deliberately: the block timing a measurement observes is the thing under test, and a
storm's rate is a latency phenomenon rather than a capability.

`DEVNET_NODE_ENV` passes extra container environment as comma-separated `KEY=VALUE` pairs — the
allocator knobs (`MALLOC_ARENA_MAX`, `MALLOC_TRIM_THRESHOLD_`) are what it exists for, since a node's
footprint under fork load is allocator-shaped and those are testable without a rebuild, which would
change the binary under test.

## Commands

```text
tools/devnet.sh build                        build the rnode:local image
tools/devnet.sh up --validators N [--observers M]
                                             start N validators (default 1) + M observers (default 0)
tools/devnet.sh deploy <contract.rho>        signed deploy to the bootstrap (file lives in examples/)
tools/devnet.sh eval <file.rho>              thin-client REPL eval of a file on the bootstrap
tools/devnet.sh query <name>                 listen for data at a public name
tools/devnet.sh propose                      force the bootstrap to propose a block
tools/devnet.sh status                       docker ps for the devnet
tools/devnet.sh logs <node>                  tail a node's logs
tools/devnet.sh diagnose                     per-node health check (PASS/FAIL)
tools/devnet.sh stop <node>                  stop ONE node, leaving the rest running
tools/devnet.sh start <node>                 start a node that `stop` stopped (its data survives)
tools/devnet.sh down [-v]                    stop the devnet (+ drop volumes)
```

`stop <node>` is the one-validator death of a liveness measurement — `2` resolves to
`devnet-validator-2`, and `start` brings the same validator back with its chain, which is the other
half of the question (a silent validator that returns). It exists because `down` removes the whole
network and takes no node name, so "kill one validator" had no supported command.

**Autopropose and friends are `up` flags, not build flags** — set them per run:

```text
tools/devnet.sh up --validators 1                    # autopropose ON (default)
tools/devnet.sh up --validators 1 --no-autopropose   # autopropose OFF — blocks only via `propose`
tools/devnet.sh up --validators 1 --no-propose-on-deploy
                                                     # deploy no longer auto-proposes
```

**`--no-autopropose` does not make a multi-validator net quiet.** The dummy-`Nil` injector goes away,
but attestation is still on by default, and each attestation is itself a remote block for the peers,
which attest in turn, so a three-validator net stays busy with autopropose off. On one validator there
is nobody to attest to, so the flag does buy quiet there. The measured storm is in
[Incident history](incident-history.md#the-attestation-storm).

`--admin` (default on) publishes the admin HTTP port `40405`, which exposes only `POST /api/v1/propose`
(*force a block*) — deploys always go through the public `40403` surface or the deploy gRPC, never
`40405`. Run `tools/devnet.sh help` for the full matrix (autopropose / propose-on-deploy / admin /
deployer-key / nodes).

## Worked example

```bash
# 1. Build the image.
tools/devnet.sh build

# 2. Start a single validator (creates genesis and autoproposes).
tools/devnet.sh up --validators 1

# 3. Deploy a contract (examples/hello.rho sends "world" on the public name "hello").
tools/devnet.sh deploy hello.rho

# 4. The bootstrap autoproposes a block containing the deploy; then query the result.
tools/devnet.sh query hello          # -> "world"

# 5. Or a 3-validator + 1-observer network:
tools/devnet.sh up --validators 3 --observers 1
tools/devnet.sh deploy hello.rho
tools/devnet.sh query hello

# 6. Tear down.
tools/devnet.sh down -v
```

## Node roles

- **Validators (1–3)** — each has its own keypair, is bonded in the shared `bonds.txt`, and runs
  `--autopropose`. Validator 0 (`devnet-bootstrap`) is the genesis ceremony-master (`--standalone`);
  validators 1–2 bootstrap from it.
- **Observers (0–3)** — unbonded, no autopropose; they bootstrap and replicate the chain.

Genesis is written to a temp dir and mounted read-only into the bootstrap: `bonds.txt` (one
`<pubkey> <stake>` line per validator) and `wallets.txt` (a funded deployer vault so deploys can pay
phlo). Contracts in `examples/` are mounted read-only at `/contracts` inside every node.

Blocks are produced **on their own**: every node runs with `--dev-mode --deployer-private-key`, so
`--autopropose` injects a signed `Nil` dummy deploy whenever the pool is empty and keeps proposing.
`up` blocks until `latestBlockNumber` is advancing.

## Casper fidelity & gaps

The devnet runs the real CBC-Casper data path — genesis bonding, deploy-driven proposals, a
cross-justified block-DAG, the monotone fringe estimator, and `> 2/3`-stake finality — with these
inputs simplified (theory in [Consensus (Casper)](consensus.md)):

- **Equal stake ⇒ unanimous finality.** The validators have equal stake `100` each (total `300`), so
  the strict `> 2/3` threshold requires *all* validators to attest before a block finalizes (2 of 3 is
  exactly 2/3, not a supermajority). Production's uneven stakes let a proper subset reach `> 2/3` — but
  see the next bullet: the threshold is not the binding constraint on a net where someone is silent.
- **A validator that stops producing does not cap finality.** The partition is the **live weight set**
  (`block-storage/src/dag/liveness.rs`) while the quorum stays the whole bonded set, so survivors can
  finalise and a minority still cannot. The measurement that changed this is in
  [Incident history](incident-history.md#finality-and-the-live-weight-set).
- **A joiner can stall permanently on `missing justification`.** A block can be in the DAG's in-memory
  index before the store that index is built from holds it, so it is queued for validation, fails to
  resolve a justification, and is dropped with nothing to re-queue it; only a restart recovers.
- **Dummy `Nil` deploys** stand in for real user traffic, and **autopropose is a tight loop** (no
  backoff), so the block rate is unbounded rather than a production cadence.
- **Single shard** (`root`); **no Byzantine behavior** (all validators honest — the slash path exists
  but is never exercised); **no partitions/latency** (local Docker bridge); **throwaway keys** with no
  economic security; a small fixed validator set (≤3).

## Genesis content and what a consumer can hardcode

A fresh chain's genesis installs the interpreted contracts a client reaches through
`rho:registry:lookup`, and seeds the shorthand aliases so those lookups resolve — `rho:rchain:revVault`
and `rho:rchain:pos` (native channels), `rho:rchain:makeMint`, `rho:lang:listOps` and
`rho:lang:nonNegativeNumber`. [`spec/GENESIS.md`](../../../spec/GENESIS.md) is the manifest: each
entry with the consumer it unblocks, its `rho:id`, and what is deliberately *not* installed.

For a client:

- A seeded lookup answers **`(9223372036854775807, bundle+{dispatcher})`** — the signed-registration
  shape, destructured `for (@(_, X) <- ch)` (or `for (@(_, *X) <- ch)`), after which `X` is callable.
  A `Nil` reply is what an *unseeded* shorthand looks like; it is silent, since an unmatched `for` is
  not an error.
- The `rho:id`s of the blessed contracts are **constants of this port** (they derive from fixed
  deployer keys) and are listed in the manifest, so they are safe to hardcode. They are not the
  54-char mainnet ids.
- `down -v && up` **regenerates genesis**, which produces a new genesis *address* and invalidates any
  URI recorded from a *deployed* bootstrap (e.g. a governance master URI obtained by deploying the
  contract set at runtime). With the blessed set installed at genesis, that no longer applies to the
  node's own contracts — but anything a deploy registers is still per-chain, because
  `rho:registry:insertArbitrary` derives its URI from a random seed.

## Governance on a devnet (the rgov set)

A fresh chain from `tools/devnet.sh up` already has the rgov governance set installed — no bootstrap
script, no per-chain URI to record. `spec/GENESIS.md` is the manifest; the constants a client
hardcodes are there, and the one a governance client needs first is the master directory's read cap
(`ReadcapURI`, what the wallet's `master-uri.ts` holds).

What is installed, and by whom:

- the eight classes (`Kudos`, `Inbox`, `Directory`, `roll`, `Issue`, `Ballot`, `Chat`, `Group`) with
  upstream's registration shape, each published under a chosen constant key;
- the master directory (upstream's seven slots, rendered with those constants) plus the three slots
  upstream omits (`Chat`, `Ballot`, `Group`);
- the `GetMe`/`SendThem` feature — the first call a governance client makes.

The last three are signed by **the genesis ceremony's key** — the bootstrap validator's own key on a
devnet — written into the node's own data volume as a file and passed as
`--validator-private-key-path`, not as hex on the command line, where `docker inspect` returns it to
anyone who can talk to the daemon. They must share one key, because the template publishes its admin
capability for its own deployer and the feature's registration reads it back. The consequence worth
knowing: that validator also holds the `MasterContractAdmin` locker, so `newMemberDirectory` (which
requires it) is callable by that key and not by others. See "Ceiling of this arrangement" in
`spec/GENESIS.md` before reusing any of it on a public network.

Two things worth knowing when you exercise this by hand:

- **Log what the feature logs, or you will not see where it stopped.** Pointing its log channel at
  `rho:io:stdout` works — stdout takes a list as one datum and is persistent, and the genesis run
  prints the feature's multi-element lines on it in a row — but a *drain* (`for (@_ <= logCh) { Nil }`)
  throws the trace away, and a lost trace is what makes a stall indistinguishable from a broken
  client.
- **`rho:rchain:deployerId` must be a real public key** for anything that derives a REV address from
  it (`getMe` does). A placeholder byte array makes `RevAddress!("fromPublicKey", …)` match nothing
  and the call stalls.
- **A reply is not a value.** `MCAread!("GetMe", …)` answers `Nil` for a key the directory does not
  hold, and a bare `for (GetMe <- ch)` pattern matches `Nil`, so "the directory answered" says nothing
  about whether `GetMe` was registered. Report the value.

A hand probe of the handshake (deploy it, then read the node's own log for the stages):

```rholang
new rl(`rho:registry:lookup`), deployerId(`rho:rchain:deployerId`), out(`rho:io:stdout`),
    capCh, getMeCh, stuffCh in {
  out!("stage:1 lookup sent") |
  rl!(`<masterdict-resolve URI from spec/GENESIS.md>`, *capCh) |
  for (MCAread <- capCh) {
    out!("stage:2 resolve facet resolved") |
    MCAread!("resolve", ["GetMe"], *getMeCh) |
    for (GetMe <- getMeCh) {
      out!(["stage:3 getme-entry", *GetMe]) |     // the *value*, not just that it answered
      new logCh in {
        for (@line <= logCh) { out!(["log", line]) } |   // keep the feature's trace
        GetMe!(*deployerId, *stuffCh, *logCh) |
        for (@_reply <- stuffCh) { out!("stage:4 getMe answered") }
      }
    }
  }
}
```

All four stages pass on a fresh devnet: `getMe` answers, and for a deployer that had none it creates
the member's inbox and dictionary on the way. The historical stall between stages 3 and 4 is in
[Incident history](incident-history.md#the-governance-handshake); it was not the genesis installation
nor the feature's flow. `spec/GENESIS.md` carries the resolved state; the check that a stall is *over*
is `@[*deployerId, "inbox"]` and `@[*deployerId, "dictionary"]` being non-empty after
`newInbox`.

## The consumer's call order

From the wallet's integration suite (which drives these contracts): `newInbox` **first** — it creates
`@[*deployerId, "inbox"]` and `@[*deployerId, "dictionary"]`, and eighteen other snippets read one of
those two lockers. Then, per family: `newChat` → `sendChat`/`readChat`; `newBallot` → `castBallot`;
`newIssue` → `addVoterToIssue`/`castVote`/`displayVote`/`delegateVote`/`tallyVotes`; `newGroup` →
`joinGroup`/`addMember`. `newMemberDirectory` needs the `MasterContractAdmin` locker, which only the
key that deployed the master directory has — the devnet bootstrap validator here, or a client's own
deploy on a public network. Reads (`getRoll`, `peekKudos`, `checkRegistration`, `checkBalance`) go anywhere after their
inputs exist.

## Ports


Deploy is served on gRPC `40401` and Propose+Repl on `40402`; the helpers run the Rust `rnode` client
*inside* a node container (`docker exec`) so they reach both via `localhost`. The public HTTP API
(in-container `40403`) and the admin HTTP API (in-container `40405`) are also published to the host,
so a browser can reach them directly.

The host maps each node's deploy gRPC port to `40402 + 1000·i`, its public HTTP port to
`40403 + 1000·i`, and its admin HTTP port to `40405 + 1000·i` — the bootstrap is `i = 0`, so it
publishes `40402`/`40403`/`40405` directly.

---

**Running a real network:** [Running a node: testnet now, mainnet later](validator-observer-setup.md).
