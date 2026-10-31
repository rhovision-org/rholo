# Running a validator: hardware requirements

## TL;DR

- **Storage- and uptime-bound, not CPU-bound.** The compute is modest; LMDB's synchronous small writes
  and staying online are what bind.
- **Minimal (testnet / low traffic):** 2–4 vCPU, 8 GB RAM, 100–250 GB **NVMe SSD**, 10 Mbit+ stable.
- **Comfortable (mainnet validator):** 8+ vCPU, 16–32 GB RAM, **1 TB+ NVMe SSD**, 100 Mbit+ low-latency.
- **NVMe SSD for the data dir is the single most important choice.** RAM must cover the restart, not
  just steady state; start-up replay grows superlinearly with chain height.
- **Uptime, not horsepower.** An absent validator holds finality back — there is no inactivity leak — so
  a laptop that sleeps is a fine node and a bad validator.
- Sizing is an estimate from the code's storage envelope; there is no in-repo production benchmark.

A validator is **bonded and slashable for invalid behaviour**; absence is not currently slashed, but it
holds finality back. Read the [operator risk notice](operator-risk.md) and the [operator terms](https://github.com/rhovision-org/rholo/blob/dev/TERMS.md) before bonding.

A validator is **storage-bound and uptime-bound**, not CPU-bound. The compute envelope is modest; the
binding constraints are the synchronous-write storage layer (LMDB) and being online continuously. This
page gives a grounded sizing estimate derived from the code's storage envelope and execution model —
there is no in-repo production benchmark, so treat the numbers as an estimate, not a measurement.

## The minimum is per validator; a host running two must size for both

The 8 GB figure above is **per validator**. Memory is not partitioned by the node — every process on
the host competes for the same `MemAvailable` — so a host that runs two validators needs the sum, and
the cgroup must give each one its own share:

| Host layout | Per-host RAM |
|---|---|
| one validator | **8 GB minimal**, 16–32 GB comfortable |
| two validators (the current public testnet) | **16 GB minimal**, 32–64 GB comfortable |

Until 2026-10-29, the public testnet ran **two validators on each `s-2vcpu-4gb` host** — about 2 GiB
per validator. The operator's resize record
now reports 16 GiB RAM on each host, with two validators retained: A has 2 vCPU / 25 GB disk and B
has 4 vCPU / 80 GB disk. The nominal memory share reaches 8 GiB per validator; dedicated cgroup
allocations are not established by that arithmetic. CPU and storage still fall short of the full
per-validator profile above. The network remains **capacity-limited for qualification claims**;
the memory resize alone does not establish application conformance or throughput. The admission changes in
The admission contract keeps such a node serving — the pool is the
throttle, pending work relays to peers, and memory bounds execution permits rather than ingress — but
they do **not** make it a valid surface for application conformance, throughput claims, or a
representative validator benchmark. A node protecting itself is not a substitute for the host meeting
its own minimum.

The earlier incident remains a historical measurement: at height 1688 the host had **547 MiB available**, two
validators were `degraded: throttled`, and the old boolean gate refused every deploy and explore. The
correct reading is not "the guard worked" but "the host is below the floor"; see
[the public testnet](testnet.md) for the current declaration.

This page answers **"how big a box"**. For **"how fast, and what stops it"** — the throughput ceiling,
finality latency, merge cost and the Θ(N²) residency that sets a validator's time-to-ceiling — see
[Scaling and performance limits](scaling.md).

## What drives each dimension

### Storage — the biggest variable

The LMDB environments are sized in `casper/src/storage.rs` (`rnode_db_mapping`):

| Store | LMDB map size |
|---|---|
| `blockstorage` (blocks, and the block-disposition ledger) | 1 TB |
| `dagstorage` (DAG / metadata / deploy index) | 100 GB |
| `rspace/history` + `rspace/cold` (tuple-space trie) | 1 TB each |
| `eval/history` + `eval/cold` (REPL eval store) | 1 TB each |
| `reporting` (event log) | 10 TB |
| `deploypoolstorage` / `transaction` | 1 GB each |

These are **sparse mmap reservations**, not actual usage — an LMDB file only grows as data is written.
Real disk usage scales with chain length, on-chain state size (the RSpace trie grows with contracts,
vaults, and unforgeable names), and deploy event-log volume. The `reporting` store is the largest
reservation but is gated off by default (`enable-reporting = false` in `node/src/configuration/defaults.conf`).

### CPU — replay determinism is the real cost

Every block a node validates is **re-executed**: `replay_compute_state` runs each deploy + system
deploy against the recorded COMM trace, and proposing also executes deploys. Block validation is
parallelized across blocks (cross-block replay parallelism), so this scales with core count — it is the
main reason to give a validator more vCPUs than a first estimate suggests.

### RAM — mmap'd LMDB + in-memory DAG/hot-store

LMDB is mmap-backed (large virtual address space, but resident memory is bounded by the working set).
The genuinely in-memory pieces are the hot store and the DAG representation. The worst case is protocol
message buffering — `max-message-consumers = 400` with blocks up to
`grpc-max-recv-stream-message-size = 256M` is ~100 GB in theory, but steady-state is far lower.

### RAM again — start-up replay is the floor, and it is not a steady-state number

What a node needs *to start* is a different quantity from what it needs *while running*, and it scales
with how long the chain is. Measured on a chain of **1142 blocks whose persisted state is 4.5 MB**
(21 files under the data dir):

| | RSS | API reachable after |
|---|---|---|
| fresh chain (≤6 blocks) | 18–20 MB | 15 s |
| replay of the 1142-block state, read-only | 57 → 284 MB (oscillating, not monotone) | 225 s |
| the same replay with `-s --dev-mode --propose-on-deploy` | 52 → 285 MB | 210 s |
| an isolated chain *producing* blocks (`--autopropose` + dummy deploy) | 20 → 23 MB across 131 produced blocks | — |

So start-up costs roughly **0.25 MB of RSS and ~0.2 s per existing block**, while *producing* blocks
costs almost nothing (~0.02 MB/block). The start-up cost is the in-memory view being rebuilt:
`BlockMetadataStore::create` (`casper/src/block_metadata_store.rs`) reads every block's metadata and
rebuilds the DAG state, `BlockDagKeyValueStorage::create` (`casper/src/dag.rs`) rebuilds the child map,
height map, message graph and fringe states, and `BlockIndex` entries accumulate in a process-global
cache (`casper/src/merging.rs`).

What this means for sizing:

- **A 1 GB host is fine for a few hundred blocks but can be unable to restart a ~1000-block chain** while
  anything else runs on the box: the kernel OOM-kills rnode mid-replay, and since every restart replays
  the same DAG it loops rather than recovering. The unit still reports `active`, so the visible symptom
  is an API that never answers.
- Budget **≥ 2 GB for ~1k blocks, ≥ 4 GB to be comfortable**, and expect roughly **30 s of unavailability
  per 150 blocks** after every restart before the API answers at all.
- Replay is silent: no progress output, no readiness signal. If a node has not answered shortly after
  start on a long chain, check whether the process's RSS is *growing* before treating it as hung — and do
  not tighten the restart loop, which only repeats the replay.
- A chain that produces blocks continuously (`--autopropose` plus the dev-mode injector, as the devnet
  does) reaches a given height in an afternoon, and therefore reaches this limit quickly; a chain that
  only grows with real traffic stays in the safe zone much longer. Steady-state production is cheap — the
  cost is all in start-up.

**Start-up cost is superlinear in chain length, so size for the height you intend to serve.** The
figures above are a rate, not a constant to multiply by a block count. The DAG's per-message ancestry
sets are Θ(N²) in total, so the marginal cost of a block rises with the chain already in the DAG. The
measured points do not lie on a line: 131 produced blocks at ~0.02 MB per block, a fresh
100–300-block devnet at 0.16–0.23 MB, and a pre-`b37410b2e` 3,200-block chain at ~1.8 GB peak and
~17 minutes before the API opened. The 3,200-block row is a property of a tree this repository no
longer builds: `b37410b2e` removed one of the two Θ(N²) terms — the metadata-index rebuild no longer
clones `DagState` per block — and two pre-registered rounds could not reach the old ceiling (0 of 3
crossed on either arm; the loaded rig peaked 647–689 MiB against a 3000 MiB threshold). It is kept as
a bound, not a sizing point. The ancestry term remains, so expect start-up to worsen superlinearly and
treat a few thousand blocks as where a small host stops fitting.

A node logs progress while it indexes (`casper/src/merging.rs`, one line every 250 blocks), but
readiness is still implicit: the API binds after replay, so a client sees connection-refused, which is
indistinguishable from a crash. A paired mature-history measurement of the metadata-index constructor
is in `spec/audit/evidence/n480-startup-metadata-20261029/README.md`; it checks the reconstructed
index and persisted metadata, not full replay, and does not qualify host capacity. The correction
stack and the two rounds are in
[Incident history](incident-history.md#start-up-replay-and-the-3200-block-correction).

### Resident memory is not only what the node holds — glibc's arenas

glibc gives each thread its own malloc **arena**, and an arena keeps the high-water mark of what it has
held; it is not returned to the OS. A node under fork load spreads allocation across its worker threads,
so what the process retains is the *sum* of the per-thread peaks rather than any one peak. Measured on a
3-validator devnet, the anonymous
regions are **29 of exactly 32 MiB** — the worker **stacks** (`thread_stack_size(32 MiB)`), holding
5.7 MiB of RSS between them — and **11 of exactly 64 MiB**, which *are* the arena heaps, while the main
`[heap]` sat at 1.9 MiB and file-backed RSS at 26.8 MiB.

The corrected reading matters: the 32 MiB regions are worker stacks, not arenas, and 29 × 64 MiB would
be 1.86 GiB — more than the run's whole anonymous RSS. The earlier inversion and its correction are
recorded in [Incident history](incident-history.md#devnet-memory-ceiling).

**And what reaches the ceiling is held, not returned.** Read from inside the running node by the
allocator's own accounting, `retained` was **0** in every one of ~1,700 samples across nine node-runs,
with allocated bytes explaining 97–99 % of the cgroup's `anon` at peaks of 3.1–8.2 GiB. So a node that
dies at its limit is dying of memory it still holds, not of a leak and not of purge lag — which is why
the fix space is the code, and why the arena cap below is not the answer. The audit's §27 carries the
instruments, and the one composition question they leave open.

**Capping the arenas is unproven and is deliberately not shipped.** `MALLOC_ARENA_MAX=2` was tried at a
4 GiB ceiling, and the survival counts do not separate it from noise; single runs of this shape
disagree with each other. Do not set it on the strength of this page. The mechanism is the useful
part: a killed node's cgroup is gone, so a probe that samples only survivors reads a peak of 0, and a
node cannot set the cap for itself because `mallopt` and `malloc_trim` are `unsafe` and the crate
graph is `#![forbid(unsafe_code)]`. The only lever is a runtime environment variable, and any claim
for it needs a reproduction whose run-to-run spread is characterised first.

### Network — low bandwidth, latency-sensitive

gRPC + Kademlia discovery over ~20 batch peer connections; blocks are capped at 256 MB streams and
deploys at 16 MB. Bandwidth is modest; consensus wants **stable, low-latency** connectivity more than
raw throughput.

## Recommendation

| | Minimal (testnet / low traffic) | Comfortable (mainnet validator) |
|---|---|---|
| CPU | 2–4 vCPU | 8+ vCPU |
| RAM | 8 GB | 16–32 GB |
| Disk | 100–250 GB SSD | 1 TB+ NVMe SSD |
| Network | 10 Mbit+ stable | 100 Mbit+ low-latency |

The single most important choice is **NVMe SSD for the data dir**: LMDB is synchronous-write-heavy and
the RSpace trie + block store perform many small writes, so a spinning disk dominates the cost.

## In practice

Any modern general-purpose desktop or laptop with an NVMe SSD **and the per-validator floor above** is
enough to *run* a validator — the CPU/RAM needs are modest, but the floor is per validator and a
below-floor host is refused. An observer can be smaller. Four caveats matter more than raw specs:

1. **Uptime, not horsepower.** A validator that sleeps, hibernates, or is shut down falls out of sync
   and stops proposing. That costs it nothing directly — this tree has **no inactivity leak**, so a
   silent validator keeps its bond and, while it is in the drawn active set, its share of the epoch
   ([Validator economics](validator-economics.md)); a slash needs a block that *fails validation*, not a
   node that is absent. What it costs is the network: finality needs the active validators to speak, so
   one absent validator holds everyone back. A laptop runs it fine; a laptop you carry around and close
   is a bad *validator*. That, not the CPU, is the reason for an always-on box or VPS.
2. **Disk capacity grows.** The 1 TB reservations are not needed up front, but a busy shard's RSpace
   trie + block store + deploy event logs accumulate. A 128–256 GB SSD is fine to start and eventually
   fills on a heavily used chain.
3. **RAM has to cover the restart, not just the run.** Sizing on steady-state RSS alone produces a node
   that works fine until it is restarted, and then cannot come back on a small host. See the start-up
   replay section above.
4. **Bare-metal storage beats a nested container.** A Chromebook-class machine (8 threads / 8 GB)
   would *run* the node, but only inside its Linux container behind a hypervisor and an I/O layer —
   exactly the wrong substrate for LMDB's many-small-writes pattern, and it may not expose the NVMe
   directly. A cheap always-on box running bare Linux is a better validator than a faster laptop in a
   container.

## Ringfencing the node

The numbers above are the sizing; the **boundary** is the operator's. A node cannot set its own cgroup
limits, choose the kernel's OOM victim or prevent two validators sharing a host. See
[Ringfencing a node](resource-ringfence.md) for the systemd/Docker memory, CPU, I/O and PID limits, the
dedicated-NVMe data-directory rule and the cgroup signals to alert on. The node-side start-up preflight
is shipped and documented there; memory-aware admission and fallback states are tracked in the
[resource safety case](../spec/resource-safety-case.md) and HAZOP study node U17.

---

**Setting one up:** [Running a node: testnet now, mainnet later](validator-observer-setup.md).
