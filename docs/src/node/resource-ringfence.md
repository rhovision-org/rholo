# Ringfencing a node

A node cannot set its own memory, CPU or disk limits — that is the operator's boundary — and a node
that is not bounded competes with everything else on the host for memory. A memory-exhaustion
incident is the worked example ([Incident history](incident-history.md#ingress-admission-and-the-memory-gate)): two validators on each 1 GB host, a deploy burst, host memory exhausted, and the kernel
chose which processes died. This page is the operator half of the
[resource safety case](../spec/resource-safety-case.md). The start-up preflight, the fallback state
machine and the headroom permit budget are shipped (Law 86); memory pressure bounds **execution
concurrency**, not admission, so the limits on this page remain the runtime barrier between a busy node
and the host. A node on an undersized host keeps accepting and relaying work; it does not become
healthy.

One validator per host is the default. Two validators may share a host only when the host is sized for
both and the failure-domain risk is accepted: a host loss then removes **both** validators' stake at
once, which is up to half of a four-validator pool.

## Give the node its own cgroup

On a systemd host, a drop-in or a unit with a dedicated slice is the simplest ringfence. Size the limits
from [the hardware requirements](validator-requirements.md) — including the **restart/replay** headroom,
not just steady-state RSS.

```ini
# /etc/systemd/system/rnode.service.d/resource.conf
[Service]
MemoryHigh=6G
MemoryMax=8G
MemorySwapMax=0
CPUQuota=200%
IOWeight=100
TasksMax=4096
Restart=on-failure
RestartSec=10s
```

- `MemoryHigh` is the throttle point; `MemoryMax` is the hard ceiling. Keeping
  `MemorySwapMax=0` (or equal to `MemoryMax` with Docker) prevents a slow swap death.
- The cgroup limit is also the node's **effective budget**: a limit below the chain's replay floor will
  OOM the node during start-up, before its API opens. The start-up preflight refuses that host rather
  than replay into it (see below).
- `CPUQuota` leaves headroom for the kernel and the page cache; do not give the node every core.
- A hard cgroup limit contains the damage — the cgroup OOM killer takes the node, not the host — but it
  is still a failure. The goal is to shed work before the limit, not to find it.

For a one-off run:

```sh
systemd-run --scope --slice=rnode.slice \
  -p MemoryHigh=6G -p MemoryMax=8G -p MemorySwapMax=0 -p CPUQuota=200% \
  /usr/local/bin/rnode run --profile default ...
```

## Docker

```sh
docker run --name rnode \
  --memory 8g --memory-swap 8g --cpus 2 --pids-limit 4096 \
  -v /srv/rnode:/var/lib/rnode \
  rnode:local run --profile docker ...
```

`--memory-swap` equal to `--memory` disables swap; leaving it unset lets the container swap, which turns
an OOM into a long stall. The devnet tooling already uses this shape (`tools/devnet.sh`).

## The start-up preflight (shipped)

Before the expensive DAG rebuild, the node reads its host: `MemAvailable` capped by the cgroup's
remaining allowance, the data directory's free space, and its own CPU count. A **validator**
(a node holding `casper.validator-private-key`) is refused when it is below the policy floor for the
number of blocks it has persisted; an **observer** is warned and starts, because an observer on a small
host can still serve its user while a starved validator harms quorum.

```hocon
# rnode.conf — the defaults; change only with a measured reason
resource-guard {
  enforce-startup = auto          # auto: validators enforce, observers warn; or enforce / warn
  allow-under-minimum = false     # true accepts a floor breach, logged
  accept-under-minimum-risk = false  # a validator's explicit acceptance; required with the line above
  min-available-mb = 0            # 0 = the chain-length policy table; a positive value is your floor
  min-data-dir-free-mb = 1024
}
```

The policy table is 1 GiB up to 100 persisted blocks, 2 GiB to 1,000, 4 GiB to 5,000, and 4 GiB
(labeled **unmeasured**) above that. It is a conservative floor with its provenance in
`node/src/resources/policy.rs`, **not** a per-block replay model; the refusal names the band it used.
`--allow-under-minimum` (or the config key) is the explicit override and is logged. For a validator it
must be paired with `--accept-under-minimum-risk`; read [validator risk](operator-risk.md) and the
[operator terms](https://github.com/rhovision-org/rholo/blob/dev/TERMS.md). A host known to be adequate
can start without editing the table. A full data directory is refused regardless of role unless overridden, because the next write fails
rather than the next allocation; for a validator the override also needs `--accept-under-minimum-risk`.

## Give the data directory its own disk

LMDB is synchronous-write-heavy and its map sizes are sparse reservations, not immediate usage
(`casper/src/storage.rs`). Put `storage.data-dir` on a **dedicated NVMe mount**, not the root filesystem,
and leave free space for the append-only history and the RSpace trie. A spinning disk or a nested
container's I/O layer dominates the cost even when the CPU and RAM are adequate.

```sh
mount | grep "$(rnode ... --data-dir 2>/dev/null || echo /var/lib/rnode)"   # confirm the filesystem
df -h /var/lib/rnode                                                      # confirm free space
```

The start-up preflight refuses a data directory with less free space than
`resource-guard.min-data-dir-free-mb` (1 GiB by default) unless the override is set. That is a start-up
check, not a runtime bound: a disk that fills while the node runs is still the operator's `df` alert.

## Monitor the boundary, not just the node

Alert on the cgroup and the filesystem, not only on `/api/status`:

```sh
cat /sys/fs/cgroup/rnode.slice/memory.current
cat /sys/fs/cgroup/rnode.slice/memory.peak
cat /sys/fs/cgroup/rnode.slice/memory.events     # low high max oom oom_kill
cat /sys/fs/cgroup/rnode.slice/cpu.stat
df -h /var/lib/rnode
```

The external `testnet.rhobot.net/health` snapshot carries `mem_available_mb`/`disk_free_mb`; the node's
own `/api/status` carries `degraded`/`acceptsDeploys`, and `/api/status/operator` carries the sampled
headroom, floors, recovery boundary, permit limits, source and sample age.

**`acceptsDeploys` is the deploy pool's count and term-byte capacity, not a memory reading** (Law 86).
A throttled node with a free pool slot accepts; `degraded` is the fallback state machine's **advisory**
word, and it is not a readiness signal. A **429 with `Retry-After`** is a real bound — a full pool, a
deployer share or rate, or the explore permit budget — and the work can be retried; a **503** is a node
that cannot serve at all. Neither surface is a certificate: a `degraded = normal` sample means the node
was not reporting pressure at that instant, not that it is healthy.

## What the node cannot do

The node cannot raise a cgroup limit, prevent co-location, or choose the kernel's OOM victim. It can
detect a limit, refuse to start below a policy floor, bound execution concurrency with the headroom
permit budget, hold pending work in the pool, relay it to peers, and report the state and the reason.
Those are the safety-case barriers; this page is the operator barrier they depend on.

---

**Setting one up:** [Running a node: testnet now, mainnet later](validator-observer-setup.md).
