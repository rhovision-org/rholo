# #477 live arm — a throttled validator accepts, includes and serves

**What this measures.** The old memory gate refused every deploy and explore with a 503 as soon as
the fallback word was `throttled`, even with a free pool. This arm reproduces the incident's shape on
a disposable two-validator devnet and checks the replacement: a throttled validator **accepts** a
signed deploy, **includes** it in a block, and the chain continues.

## Topology and instrumentation

- Image `rnode:477`, built from `fix/477-pending-work` at `60b636908` (release binary, glibc runtime
  image), `tools/devnet.sh up --validators 2 --fresh` with `DEVNET_PREFIX=arm477`,
  `DEVNET_NETWORK=arm477`, gRPC/HTTP/admin bases `50402/50403/50405`.
- `arm477-bootstrap` (validator 0) at the devnet's default 4 GiB cgroup; `arm477-validator-1`
  constrained **after start** with
  `docker update --memory 320m --memory-swap 320m arm477-validator-1` (the arm's stand-in for the
  testnet's two-validators-on-~4 GiB host). `docker-stats.txt` records the 34.5 MiB / 320 MiB usage
  at the reading.
- The node reads the container cgroup, so `effective_available_mb = min(host, 320 - current)` ≈
  285 MiB. On a 320 MiB limit the policy floor is `max(256, 10%) = 256` and the throttle floor is
  `256 + 64 = 320`, so 285 sits in `[critical, throttle)` → **`degraded: throttled`**.
- The devnet runs `--autopropose --propose-on-deploy`; the batch and relay switches are **off**
  (this arm is the admission/permit path, not the batch/relay path).

## Readings

| file | reading |
|---|---|
| `validator1-status-before.json` | `degraded: throttled`, `acceptsDeploys: true`, `height 105`, pool 1/3 |
| `validator1-health-before.json` | `ok: true`, `degraded: throttled`, `acceptsDeploys: true`, pool 10 000 / 64 MiB |
| `deploy.log` | `deploying hello.rho to arm477-validator-1`; `Success!`; deploy id |
| `deploy-status.json` | `ProcessedWithSuccess`, `blockNumber: 112`, block hash `636469f5…`, sender a validator key |
| `validator1-status-after.json` | `degraded: throttled`, `acceptsDeploys: true`, `height 139`, pool 0/0 |
| `bootstrap-status-after.json` | `degraded: normal`, `height 139`, pool 0/0 |

The deploy was submitted to the **constrained** validator (`devnet.sh deploy hello.rho --to 1`), whose
`degraded` word was `throttled` before and after. It was accepted (`Success!`), included in block 112,
and the chain advanced from 105 to 139; the healthy bootstrap agrees at 139. The constrained node
executed under the permit budget (headroom 29 MiB over the floor gives one execution permit), and its
`/health` reports `ok: true` with the pool capacity, not a low-memory failure.

**The contrast with #477's code:** the old `PressureGate` returned `Refuse` for `ResourceState::Throttled`
before the body was decoded, so the same request would have been a 503 and the deploy would never have
entered a pool. The new path admits it, and the `acceptsDeploys` / `pool` fields say why.

## What this arm does not cover

- The public testnet's exact 4 GiB host with two validators **sharing one host cgroup**: Docker limits
  are per container, so this arm constrains one container rather than two siblings in one host cgroup.
  The node's reading is the same `min(host, cgroup limit - current)` computation; the host total is
  the only term this arm does not reproduce.
- The relay: the deploy was included by the constrained validator itself (propose-on-deploy), not by
  a peer after a relay. The relay is off by default and its end-to-end arm is still owed.
- The batch path: off; this arm runs the sequential executor.
- Finality: the devnet's two equal validators are 50/50; this reading is about admission/inclusion,
  not a finality measurement.
