# #477 relay arm — a critical validator hands work to a healthy one

**What this measures.** The two-validator arm (`n477-live-arm-20261028`) showed a throttled node
accepting and including its own work. This arm puts the constrained node at **`critical`** (where it
stops producing) and turns the pending-work relay on, so the deploy's only route into a block is a
peer's `deploy` service.

## Topology

- Image `rnode:477` from `fix/477-pending-work` at `60b636908`; `tools/devnet.sh up --validators 4
  --fresh` with `DEVNET_PREFIX=relay477`, `DEVNET_NETWORK=relay477`, gRPC/HTTP/admin bases
  `60402/60403/60405`, and `DEVNET_NODE_ENV='RNODE_DEPLOY_RELAY=1'` on every node.
- `relay477-validator-1` constrained after start to a 280 MiB cgroup
  (`docker update --memory 280m --memory-swap 280m`): limit `293601280`, current ~42 MiB →
  effective available ~250 MiB, below the 256 MiB critical floor → **`degraded: critical`**, and
  `stops_production()` is true, so this node does not propose.
- Four equal validators (100 stake each); the other three are healthy and finalise (75 %).

## Readings

| file | reading |
|---|---|
| `validator1-status-before.json` | `degraded: critical`, `acceptsDeploys: true`, height 41 |
| `validator1-operator-before.json` | the same word, with RSS/headroom/floors |
| `deploy.log` | `deploying hello.rho to relay477-validator-1`; `Success!`; deploy id |
| `deploy-status-validator1.json` | `ProcessedWithSuccess`, **block 44, sender `04ea3ce0…` = validator 3**, not validator-1 |
| `validator1-status-after.json` | `degraded: critical`, `acceptsDeploys: true`, height 57, pool 0/0 |
| `bootstrap-status-after.json` | `degraded: normal`, height 58 |
| `bootstrap-finalized-after.json` | finalized block 49, sender `04d8b6c3…` = validator 2 |
| `docker-stats.txt` | the per-container memory at the reading |

The deploy returned `Success!` **on the critical validator** and was included in block 44 by validator
3. The critical node did not propose (`stops_production`): the relay offered the signed deploy to its
connected peers, and a healthy validator pooled and proposed it. Finality advanced (block 49
finalised at height 58) with three of four validators, so the constrained node neither refused the
work nor held the chain back.

**The contrast with #477's code:** the old gate refused at `throttled`, let alone `critical`; the new
`acceptsDeploys` is the pool's capacity, and the relay is what lets a node that cannot execute pass
the work on.

## What this arm does not cover

- The public testnet's shared-host topology (two validators in one ~4 GiB host cgroup); Docker limits
  are per container, so both arms constrain one container.
- The **batch** path: off for this reading; the batch equivalence is covered by
  `runtime_manager::tests::a_certified_batch_matches_the_sequential_fold` and the conflicting pair.
- The exact public-testnet stake/keys; this is a disposable devnet with throwaway keys.
