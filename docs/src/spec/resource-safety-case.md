# Resource safety case — inherent safety and ALARP

**Status:** authored 2026-10-24; load-bearing review 2026-10-29. This is a safety-case record and an evidence contract, not a
certification. The operator-facing contract is [validator risk](../node/operator-risk.md) and
[`TERMS.md`](https://github.com/rhovision-org/rholo/blob/dev/TERMS.md). It covers the node's own contribution to the #420 failure class: a node that is under
resourced, or that accepts work it cannot finish, degrades chain health rather than failing alone. The
merge duplicate-produce defect found during that investigation is separate and fixed (PR #434, C336).

## Top event

**TE-RS — an under-resourced node degrades chain health.** This is the node-local specialisation of
**TE-4** ("state, disk or memory exhaustion halts the net") in
[failure-mode HAZOP](failure-hazop.md); its consequences cross TE-1 (chain halt), TE-3 (a joiner cannot
safely join) and TE-5 (node wedge). The harm is network-level: a validator that cannot serve blocks,
fringe or finality is indistinguishable from an absent one, and co-locating two validators on one host
turns that host failure into half of the bound pool.

The incident that motivates this case (#420): two 1 GB hosts, two 250-stake validators each; a 17-deploy
burst; host memory exhausted; the kernel OOM-killed RNode; HTTP and the fringe service stopped answering;
tips diverged and finality lagged; a well-resourced observer could not bootstrap from any validator.

## Bow-tie

**Threats** (why control is lost): start below the replay floor; a burst of decoded/executed work; pool
amplification (count-only caps, duplicate re-parses, body/term asymmetry); the replay/merge peak; disk
fill; no cgroup or ringfence; co-located validators; CPU-quota starvation; a degraded node still
accepting work; a restart that replays into the same OOM; ambiguous acceptance, an orphan root or a
stale derived index; peers unable to route around a degraded validator (outside node control).

**Barriers — existing:** per-peer 429 before JSON decode; gRPC 100/s; pool count/share/rate caps; the
64 KiB term bound and 2 MiB/16 MiB decode caps; four bounded ingress queues; the atomic root-pair write
and `reset_volatile`; the reset bracket; replay validation; finality-stall counters; the durable
autopropose halt; operator health counters; durable deploy acceptance (LMDB pool before HTTP 200);
jemalloc purge and the #117 retention test; the devnet/Docker memory ceiling.

**Barriers — new** (the units in this programme):

| id | barrier | unit |
|---|---|---|
| B31 | start-up host facts + chain-length policy floor + data-dir/cgroup checks, warn/refuse with an explicit override | A2 |
| B32 | ringfence and co-location advisories, operator guidance | A2 / [ringfence page](../node/resource-ringfence.md) |
| B33 | independent watchdog thread with hysteresis and OOM-event observation | A1/C |
| B34 | **real bounds only**: pool count/bytes/share/rate → HTTP **429** + `Retry-After`, gRPC `RESOURCE_EXHAUSTED`; memory is never a 503 (Law 86a) | B / #477 |
| B35 | headroom-derived permit budgets: exploration 429s when spent; the proposer acquires the **execution** permits and caps the batch with them (C341). When batching is off, execution is already serial and the permit is a zero-headroom gate | B / #477; C341 |
| B36 | atomic duplicate reservation + byte-based pool accounting | B |
| B37 | `Normal → Throttled → Critical` (+`Unavailable`) fallback state machine with propose/attest gates; `Halt` remains the operator/integrity `HaltRecord`, never automatic on pressure | C |
| B38 | status visibility: public `acceptsDeploys` = pool capacity, `degraded` = advisory word; operator headroom/floors/recovery and permit limits on `/metrics`; peer `/health` carries `finalityStalled`/episodes | A1 / #477; #486 |
| B39 | idempotent rewind cleanup + start-up derived-index audit/repair | D2 |
| B40 | `CURRENT_ROOT` published only after block acceptance | D1 |
| B41 | memory-pressure and restart/replay evidence arms | E |
| B42 | **pending-work relay**: an admitted deploy is copied to peers' deploy service, so a constrained node does not bottleneck the network; opt-in, bounded seen set (Law 86e) | #477 |
| B43 | **operator risk notice**: `TERMS.md` plus the validator start-up log notice; the notice is informational, not a verdict | F |
| B44 | **explicit below-minimum acceptance**: a validator below the memory floor needs `--allow-under-minimum` and `--accept-under-minimum-risk`; observers remain allowed below the floor | F |
| B45 | **execution-permit wiring**: the proposer acquires the execution permits and passes the batch cap; a `Throttled` host makes a smaller batch when batching is enabled and withholds at zero | #486 / C341 |
| B46 | **block disposition and repair**: a stored body has a DAG record or a live pending lease; a fault is recorded and retried; a boot/periodic audit re-offers or quarantines an orphan; **readiness** (`/health`, `orphan_bodies`) requires no unresolved orphan — the phase does not, and `Running` may be entered with one pending (C352) | U19 |
| B47 | **phase/readiness and isolation halt**: `Syncing`/`Blocked` refuse deploy admission and report not-ready; a proposer beyond the finality bound stops propose and attest and needs an operator resume | U20 |
| B48 | **liveness jail**: at advancing boundaries a sustained absentee is removed from the active set for J boundaries and paid nothing; the bond and delegations are untouched, the jail expires for a returning validator and renews for a silent one | U21 / RCHIP #55 |
| B49 | **vote-gated slash/eject**: a slash vote opens only against a jailed validator; strictly more than two thirds of the active stake must vote for it; a passing vote slashes at the configured tier and permanently bans the key | U21 / RCHIP #55 |

**Consequences:** an accepted deploy that is not durable or whose acceptance is ambiguous; API/read
timeouts and clients losing the chain; height divergence and finality lag; a joiner that cannot sync; a
validator losing production and the network losing quorum; an operator unable to see the cause, with a
restart that repeats it; an operator without an accepted hardware/absence risk (C-RS-7); execution
concurrency that is not bounded by the permit budget (C-RS-8).

## Hazard log

Each row is a hazard this programme dispositions. `Evidence` is a real `path::symbol` when the test exists,
or `owed: <unit> — <path::test>` where it is not yet built. `owed:` is visible debt, not a pass.

| ID | Threat | Existing barrier | New barrier | Consequence | Residual | Evidence |
|---|---|---|---|---|---|---|
| H-RS-01 | T-RS-1 a host below the replay floor starts and OOMs during the DAG rebuild | none | B31 | C-RS-1, C-RS-5 | the policy floor is overridable; a validator now needs the explicit acceptance (B44); the floor is not a measured curve above ~5k blocks | node/tests/resource_preflight_arming.rs::the_preflight_refuses_a_validator_warns_an_observer_and_honours_the_override |
| H-RS-02 | T-RS-2 a burst of concurrent decoded/executed deploys and explores | B20 (429), bounded queues | B33, B34, B35, B37 | C-RS-2 | a fast spike can beat the sample interval; the body/term caps and the pool byte bound still hold; the execution permit caps the batch, not the rate of a sequential block | node/src/web/http.rs::a_bounded_deploy_refusal_is_429_with_retry_after |
| H-RS-03 | T-RS-3 pool amplification: count-only caps, duplicate re-parse, body/term asymmetry | B21, B22 | B36 | C-RS-1 | the HTTP body cap still allows a 2 MiB decode before the 64 KiB term bound | casper/src/dag.rs::a_pool_at_its_byte_cap_refuses_before_the_store |
| H-RS-04 | T-RS-4 the replay/merge peak exceeds headroom mid-run | none | B33, B35, B37 | C-RS-3 | replay is superlinear and unmeasured at target height; the execution permit bounds a batched proposal, not replay | node/src/resources/fallback.rs::critical_stops_production |
| H-RS-05 | T-RS-5 data-dir/disk exhaustion; LMDB map full | LMDB map sizes, sparse reservations | B31 | C-RS-3 | free space is checked at start, not bounded at runtime | node/src/resources/disk.rs::a_missing_data_dir_is_an_error_not_zero_free |
| H-RS-06 | T-RS-6 no cgroup/ringfence; external host pressure or kernel OOM | devnet memory ceiling (deployment only) | B32, B33, B38 | C-RS-5 | the node cannot set cgroup limits or choose the OOM victim | node/src/resources/cgroup.rs::parses_v2_memory_cpu_and_events |
| H-RS-07 | T-RS-7 co-located validators: one host loss defeats quorum | none | B32, B38 (advisory) | C-RS-5 | host topology is the operator's; containers may hide siblings | node/src/resources/colocation.rs::counts_only_processes_named_rnode |
| H-RS-08 | T-RS-8 CPU-quota starvation; replay/finality lag | `available_parallelism` (workers) | B31, B33 | C-RS-3 | worker capping is deferred; the quota is only reported | node/src/resources/cgroup.rs::parses_v1_memory_and_cpu_limits |
| H-RS-09 | T-RS-9 a degraded node advertises that it is healthy, or silently loses work | `is_node_read_only` (config) | B34, B37, B38 | C-RS-1, C-RS-5 | accepting work under pressure is **correct** now; the hazard is a false readiness claim or a silent drop, and one status snapshot is not an independent read | node/src/api/conversion.rs::degraded_is_advisory_and_accepts_deploys_is_pool_capacity |
| H-RS-10 | T-RS-10 a restart replays into the same OOM | supervisor restart | B31, B32 | C-RS-6 | the supervisor can always restart anyway; the preflight must refuse first | spec/audit/evidence/n420-restart-20261024/restart.json |
| H-RS-11 | T-RS-11 ambiguous acceptance, an orphan root or a stale derived index | root-pair write, reset bracket | B39, B40 | C-RS-1 | no cross-store WAL; D1/D2 reduce and repair the known windows | casper/tests/current_root_publication.rs::a_failed_insert_leaves_the_current_root_where_it_was |
| H-RS-12 | T-RS-12 peers cannot route around a constrained validator | none | B42 | C-RS-4 | the relay is opt-in and uses the peer's external deploy API port, not a protocol-level routing guarantee; the four-validator relay arm records a critical node's deploy included by a healthy peer, and the shared-host cgroup topology remains owed | spec/audit/evidence/n477-relay-arm-20261028/README.md |
| H-RS-13 | T-RS-14 the execution permit half of B35 was never acquired by the proposer, so `Throttled` kept full-rate execution | B33 (fallback), B37 (Critical gate) | B45 | C-RS-8 | the execution budget caps a batched proposal and withholds at zero; a sequential block is inherently one-at-a-time; the fallback still withholds at `Critical` | node/src/instances/proposer_instance.rs::a_throttled_proposer_takes_a_smaller_batch_and_releases_the_lease |
| H-RS-14 | T-RS-13 an operator bonds without accepting the hardware/absence risk, or runs below the floor without a notice | B31 (preflight), B37 (Critical gate) | B43, B44 | C-RS-7 | under the shipped default absence is not slashed and is paid by default; a network that arms RCHIP #55 can jail an absentee and a >2/3 vote can slash it, and the operator notice now says so; the notice/acceptance does not make the host adequate | node/tests/resource_preflight_arming.rs::the_preflight_refuses_a_validator_warns_an_observer_and_honours_the_override |
| H-RS-17 | T-RS-17 a silent validator is paid and holds finality for ever, or a liveness rule punishes a partitioned minority and takes a bond through no fault | the income-only participation rule (off by default) and the whole-active-stake denominator | B48, B49 | C-RS-9, C-RS-7 | jail is bond-preserving and expires; only a >2/3 vote reaches the bond; a stalled chain freezes both clocks; a proponent can present a stale reachable fringe; a boundary block can omit `CloseBlock` (no validity rule yet); no live testnet evidence | owed: the partition arm and the live jail/vote-slash drill; the jail, threshold and return/renewal tests exist in `rholang/src/native_state.rs` |
| H-RS-15 | T-RS-15 a received block is stored but never accepted or refused: an `Internal`/insert fault is dropped, the receiver retains it as pending, and no audit re-offers it; children wait and the height stops | B39/B40 repair the derived-index windows, not a body with no DAG entry | B46 | C-RS-1, C-RS-5 | the durable ledger, the start-up audit before `Running`, bounded retry, the marker-gated **keys-only** pre-ledger scan and the periodic count audit are implemented; an undecodable body and a persistent fault still need an operator reset, and the fault/orphan counters remain node-wide | casper/src/block_disposition.rs::the_legacy_scan_adopts_every_unaccounted_body_and_writes_its_marker; casper/src/block_disposition.rs::the_phase_stays_starting_until_the_drain_closes |
| H-RS-16 | T-RS-16 a validator whose peers reject its chain keeps proposing; the tip advances against frozen finality and the node looks healthy | none | B47 | C-RS-4, C-RS-5 | the height-gap gate is a local policy, not a proof of peer acceptance; the phase is per shard and node readiness requires every shard Running, so one syncing shard no longer looks like a healthy node. The attestation endpoint and the operator resume remain separate controls | owed: the operator halt/resume drill; the bound, phase-gate (`a_syncing_cell_does_not_propose`) and all-shards-ready (`health_is_ready_only_when_every_shard_runs`) tests exist |

## Barrier independence

These are stated rather than assumed, because several barriers share an input or a surface:

- **B33/B35 share one signal.** If the watchdog is starved, the permit budgets cannot see pressure;
  admission still works (it reads the pool), and a stale sample is advisory. Both halves of B35 are
  wired now; the execution half caps a batched proposal and withholds at zero. The watchdog runs on a
  dedicated `std::thread` so the executor it protects cannot starve it.
- **B34/B35 are different questions on one path.** B34 admits into the bounded pool (count/bytes/
  share/rate); B35 bounds how much executes at once. A deploy is admitted by B34 and executed under
  B35; neither is the other, and memory is an input only to B35.
- **B31 is a start-up barrier only**; B33–B37 cover runtime.
- **B32 is advisory only.** The node cannot set cgroups, prevent co-location or choose the OOM victim;
  the operator owns the ringfence ([ringfence page](../node/resource-ringfence.md)).
- **B38 is one surface plus the peer route.** `/api/status` and `/metrics` read the same snapshot; the
  peer `/health` publishes only the finality stall bits, not the operator diagnosis; an independent
  host read still needs external cgroup monitoring.
- **B40 reduces but does not eliminate the cross-store window.** Body, trie, DAG metadata and derived
  indexes remain separate transactions; there is no cross-store WAL. D2 repairs the derived rows at boot.
- **B39 is a repair, not a prevention.** A crash between metadata and index cleanup is still possible; the
  boot audit makes it visible and repairable.
- **B46 is a disposition/repair barrier, not a proof of acceptance.** The ledger and audit make an
  orphan visible and retryable; a persistent fault still requires the operator reset, and the
  single-process task supervisor is not an independent process.
- **B48 and B49 share the finalised-fringe participation reading.** B48 is automatic and bond-preserving; B49 applies only to an already-jailed target and is the only path to the pool. Neither steps without finality progress.
- **B47 is a local isolation policy.** The height gap is computed from this node's DAG; a peer's
  rejection is inferred from frozen finality, not observed directly. A network with two independent
  partitions can still have two local views; the operator resume is the recovery contract.

## ALARP argument

The node-local risk is **as low as reasonably practicable** only if these hold (B45 now acquires the execution permits):

1. **Cheap inherent measures first.** Refuse provable contradictions at start; cap pool bytes and
   in-flight work; acknowledge a deploy only after it is durable; publish `CURRENT_ROOT` only after the
   block is accepted.
2. **Shed before OOM.** A watchdog with hysteresis gates admission, and the node degrades to read-only
   rather than accepting work it cannot finish. Halt is operator/integrity only, so a restart cannot
   loop into the same replay OOM.
3. **The operator can see it.** The state, the reason and the provenance are on the surfaces; a measured
   fact is distinguishable from a policy threshold.
4. **No silent acceptance and no private-chain production.** A stored body has a disposition or a
   live lease, an orphan is retried/reported, and a proposer beyond the finality bound stops
   proposing and attesting (B46/B47). The remaining exposure is an operator action or a peer set the
   node cannot observe directly.
5. **Residual risk is outside node control or disproportionately expensive.** Host topology, cgroup
   enforcement, the kernel's OOM choice, a measured replay curve at target height, cross-node degraded
   gossip, bootstrap failover, state GC and a cross-store WAL are not added here. A liveness jail or slash of an honest validator during a
partition, a jail that moves a bond, or a vote below two thirds would invalidate this claim.

**What would invalidate this claim:** an OOM inside the declared supported tier; a preflight that
false-passes a 1 GiB / 1k-block host; or a degraded node losing an accepted deploy. Any of those changes
the thresholds, the ringfence guidance or the state machine before the claim is repeated.

**Network honesty.** Node-local changes cannot make a four-validator network with two validators per host
ALARP: the failure-domain math is **#428 M1**. This case makes the node's own contribution defensible; it
does not certify the network.

## Verification matrix

The evidence rows above are the contract. The programme's units supply them in order:
A2 (preflight refusal and disk/cgroup facts), A1 (parsers, status), B (503/refusal, duplicate, byte
pool, gRPC), C (state machine, propose gate, admin resume), D (deferred root publication, index audit and
repair), E (1 GiB-versus-adequate pressure arm, restart/replay arm). The arms record binary/source SHA,
cgroup facts, chain height, deploy IDs, status/503 counts, `memory.peak`/`oom_kill`, finality/height and
the exact commands, under `target/resource-pressure/`; no live-testnet mutation is performed.

## Residual risk

Watchdog sampling can miss a fast spike (bounded by permits, bytes and the body cap); `MemAvailable` and
`memory.current` include reclaimable cache, so threshold false negatives and false positives remain; the
replay policy table is a floor, not a measured model above the documented range; disk can fill after
start; host/kernel/cgroup/co-location are outside node control; a `Critical` validator stops proposing
and attesting, so enough degraded stake can stall finality (a deliberate, observable safety/liveness
trade); peers cannot route around a degraded validator until #427/M3; there is no cross-store WAL; the
status and metrics share one snapshot; `--allow-under-minimum` is a conscious operator risk acceptance
and now requires `--accept-under-minimum-risk` for a validator; the execution permit caps a batched
proposal, not a sequential block's rate; absence is not slashed and is paid by default (#149), and a
network that wants to price it must choose a genesis participation rule; the peer `/health` liveness
fields are a signal, not an alarm — the operator owns the alert.

## Non-claims

- `degraded = normal` is a sample, not a certificate.
- A `503` means "refused before doing the work", not "the node is healthy".
- No OOM in one pressure arm does not establish M1; it is one falsifier at one configuration.
- The policy table is not a measured replay model above the documented range.
- B35 bounds a batched proposal's execution; it does not rate-limit a sequential block, and a zero permit
withholds rather than making the node degrade silently.
- The operator notice/acceptance allocates risk; it does not make an inadequate host adequate, and
absence remains un-slashed under the shipped defaults.
- This case makes no consensus-safety, finality-safety or lost-write claim; the merge fix is separate
  (C336).
- Per-block consensus term/byte limits are **not** changed here; they are a hard-fork-class decision
  deferred with a named reason.
- #429's fail-closed rule stands: even an all-PASS evidence set is `PENDING_MAINTAINER_SIGNOFF`, never
  release authorization. #420 remains open.
