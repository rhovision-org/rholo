# Resource safety case — inherent safety and ALARP

**Status:** authored 2026-10-24. This is a safety-case record and an evidence contract, not a
certification. It covers the node's own contribution to the #420 failure class: a node that is under
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
| B34 | memory-aware HTTP `503` and gRPC `resource_exhausted` before decode/execution | B |
| B35 | bounded in-flight permits (reuse #426; never a second semaphore) | B |
| B36 | atomic duplicate reservation + byte-based pool accounting | B |
| B37 | `Normal → Throttled → Critical` (+`Unavailable`) fallback state machine with propose/attest gates; `Halt` remains the operator/integrity `HaltRecord`, never automatic on pressure | C |
| B38 | status/metrics visibility: public `degraded`/`acceptsDeploys`, operator headroom/provenance | A1 |
| B39 | idempotent rewind cleanup + start-up derived-index audit/repair | D2 |
| B40 | `CURRENT_ROOT` published only after block acceptance | D1 |
| B41 | memory-pressure and restart/replay evidence arms | E |

**Consequences:** an accepted deploy that is not durable or whose acceptance is ambiguous; API/read
timeouts and clients losing the chain; height divergence and finality lag; a joiner that cannot sync; a
validator losing production and the network losing quorum; an operator unable to see the cause, with a
restart that repeats it.

## Hazard log

Each row is a hazard this programme dispositions. `Evidence` is a real `path::symbol` when the test exists,
or `owed: <unit> — <path::test>` where it is not yet built. `owed:` is visible debt, not a pass.

| ID | Threat | Existing barrier | New barrier | Consequence | Residual | Evidence |
|---|---|---|---|---|---|---|
| H-RS-01 | T-RS-1 a host below the replay floor starts and OOMs during the DAG rebuild | none | B31 | C-RS-1, C-RS-5 | the policy floor is overridable and is not a measured curve above ~5k blocks | node/tests/resource_preflight_arming.rs::the_preflight_refuses_a_validator_warns_an_observer_and_honours_the_override |
| H-RS-02 | T-RS-2 a burst of concurrent decoded/executed deploys and explores | B20 (429), bounded queues | B33, B34, B35 | C-RS-2 | a fast spike can beat the sample interval; body limits bound it | node/src/web/http.rs::a_deploy_is_refused_503_before_the_body_is_decoded |
| H-RS-03 | T-RS-3 pool amplification: count-only caps, duplicate re-parse, body/term asymmetry | B21, B22 | B36 | C-RS-1 | the HTTP body cap still allows a 2 MiB decode before the 64 KiB term bound | casper/src/dag.rs::a_pool_at_its_byte_cap_refuses_before_the_store |
| H-RS-04 | T-RS-4 the replay/merge peak exceeds headroom mid-run | none | B33, B37 | C-RS-3 | replay is superlinear and unmeasured at target height | node/src/resources/fallback.rs::critical_stops_production |
| H-RS-05 | T-RS-5 data-dir/disk exhaustion; LMDB map full | LMDB map sizes, sparse reservations | B31 | C-RS-3 | free space is checked at start, not bounded at runtime | node/src/resources/disk.rs::a_missing_data_dir_is_an_error_not_zero_free |
| H-RS-06 | T-RS-6 no cgroup/ringfence; external host pressure or kernel OOM | devnet memory ceiling (deployment only) | B32, B33, B38 | C-RS-5 | the node cannot set cgroup limits or choose the OOM victim | node/src/resources/cgroup.rs::parses_v2_memory_cpu_and_events |
| H-RS-07 | T-RS-7 co-located validators: one host loss defeats quorum | none | B32, B38 (advisory) | C-RS-5 | host topology is the operator's; containers may hide siblings | node/src/resources/colocation.rs::counts_only_processes_named_rnode |
| H-RS-08 | T-RS-8 CPU-quota starvation; replay/finality lag | `available_parallelism` (workers) | B31, B33 | C-RS-3 | worker capping is deferred; the quota is only reported | node/src/resources/cgroup.rs::parses_v1_memory_and_cpu_limits |
| H-RS-09 | T-RS-9 a degraded node still accepts work or advertises healthy | `is_node_read_only` (config) | B34, B37, B38 | C-RS-1, C-RS-5 | one status snapshot is not an independent read | node/src/resources/fallback.rs::normal_to_throttled_needs_hysteresis |
| H-RS-10 | T-RS-10 a restart replays into the same OOM | supervisor restart | B31, B32 | C-RS-6 | the supervisor can always restart anyway; the preflight must refuse first | spec/audit/evidence/n420-restart-20261024/restart.json |
| H-RS-11 | T-RS-11 ambiguous acceptance, an orphan root or a stale derived index | root-pair write, reset bracket | B39, B40 | C-RS-1 | no cross-store WAL; D1/D2 reduce and repair the known windows | casper/tests/current_root_publication.rs::a_failed_insert_leaves_the_current_root_where_it_was |
| H-RS-12 | T-RS-12 peers cannot route around a degraded validator | none | none — out of node-local scope | C-RS-4 | needs #427/M3 protocol work and peer failover | owed: #427/#428 M3 |

## Barrier independence

These are stated rather than assumed, because several barriers share an input or a surface:

- **B33/B34 share one signal.** If the watchdog is starved, admission cannot see pressure. Mitigation:
  sample on a dedicated `std::thread`, and treat a stale sample as `Unavailable`/throttled, never normal.
- **B34/B35 are two gates on one path.** Admit only if headroom **and** a permit are available; a permit
  is a concurrency bound, not a memory bound, and headroom is not a concurrency bound.
- **B31 is a start-up barrier only**; B33–B37 cover runtime.
- **B32 is advisory only.** The node cannot set cgroups, prevent co-location or choose the OOM victim;
  the operator owns the ringfence ([ringfence page](../node/resource-ringfence.md)).
- **B38 is one surface.** `/api/status` and `/metrics` read the same snapshot; an independent read needs
  external cgroup monitoring.
- **B40 reduces but does not eliminate the cross-store window.** Body, trie, DAG metadata and derived
  indexes remain separate transactions; there is no cross-store WAL. D2 repairs the derived rows at boot.
- **B39 is a repair, not a prevention.** A crash between metadata and index cleanup is still possible; the
  boot audit makes it visible and repairable.

## ALARP argument

The node-local risk is **as low as reasonably practicable** only if these hold:

1. **Cheap inherent measures first.** Refuse provable contradictions at start; cap pool bytes and
   in-flight work; acknowledge a deploy only after it is durable; publish `CURRENT_ROOT` only after the
   block is accepted.
2. **Shed before OOM.** A watchdog with hysteresis gates admission, and the node degrades to read-only
   rather than accepting work it cannot finish. Halt is operator/integrity only, so a restart cannot
   loop into the same replay OOM.
3. **The operator can see it.** The state, the reason and the provenance are on the surfaces; a measured
   fact is distinguishable from a policy threshold.
4. **Residual risk is outside node control or disproportionately expensive.** Host topology, cgroup
   enforcement, the kernel's OOM choice, a measured replay curve at target height, cross-node degraded
   gossip, bootstrap failover, state GC and a cross-store WAL are not added here.

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
status and metrics share one snapshot; `--allow-under-minimum` is a conscious operator risk acceptance.

## Non-claims

- `degraded = normal` is a sample, not a certificate.
- A `503` means "refused before doing the work", not "the node is healthy".
- No OOM in one pressure arm does not establish M1; it is one falsifier at one configuration.
- The policy table is not a measured replay model above the documented range.
- This case makes no consensus-safety, finality-safety or lost-write claim; the merge fix is separate
  (C336).
- Per-block consensus term/byte limits are **not** changed here; they are a hard-fork-class decision
  deferred with a named reason.
- #429's fail-closed rule stands: even an all-PASS evidence set is `PENDING_MAINTAINER_SIGNOFF`, never
  release authorization. #420 remains open.
