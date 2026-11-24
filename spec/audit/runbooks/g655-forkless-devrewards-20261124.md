# G655 runbook — forkless developer rewards on a local production genesis

**Scope.** Run the DevRewards service on a four-validator **production-genesis** devnet and prove the
gate's arms. This is a local-devnet witness, not mainnet-scale evidence.

**Status of the arm.** The offline arms (contracts, settlement, registry, attribution, encoding,
HAZOP) are green and listed in
`spec/audit/evidence/g655-forkless-devrewards-20261124/README.md`. A **one-validator production-genesis
smoke is green**: the genesis commits and the DevRewards install lands as an ordinary deploy (block 4;
the treasury address is published). The full arm is **not** green: the node exposes the report tree
over gRPC (`getEventByHash`) but no CLI command wraps it, so the independent observer has no client
yet; the four-validator run, the reserve funding, the claim and the restart drills are owed. The
observer client is the unblocker. G655 stays `owes`; do not promote it from this runbook alone.

## Preconditions

- Docker with the node image built from the tree under test:
  `RNODE_IMAGE=rnode:devrewards tools/devnet.sh build --fresh`
- `rnode` CLI in the image; the devnet's four throwaway validator keys (published at the top of
  `tools/devnet.sh`) are the reserve signers and the attestors.
- 16 GB available; the four-validator default is the highest this runbook tests.

## Run

```bash
# 1. Production genesis: no --dev-mode, a 5% reserve, 2-of-2 over two validator keys, funded deployer.
RNODE_IMAGE=rnode:devrewards tools/devnet.sh up --validators 4 --fresh \
  --production-genesis --deployer \
  --developer-share 500 \
  --developer-reserve-keys <pub0>,<pub1> --developer-reserve-quorum 2

# 2. Finality.
tools/devnet.sh cli devnet-validator-1 last-finalized-block

# 3. The service is an ordinary deploy. Attestors = two validator keys, threshold 2.
python3 tools/devrewards_install.py --attestor <pub0> --attestor <pub1> --threshold 2 \
  > examples/devrewards/install.rendered.rho
tools/devnet.sh deploy devrewards/install.rendered.rho

# 4. Fund the treasury from the reserve (quorum transfer), register the beneficiary, build the
#    manifest, accept it, activate, claim. The exact deploy terms are in
#    `docs/src/node/developer-rewards.md`; the orchestration is
#    `tools/devrewards-experiment.py run`.
```

## Expected

- `up` prints a genesis accepted, four nodes, and the bootstrap's autopropose produces blocks.
- `last-finalized-block` prints a block before the 300 s deadline.
- The install deploy exits 0 and `@"devrewards/treasury-address"` holds a REV address.
- The experiment writes `verdict.json` with `"result": "pass"` **only** when every required arm is
  `pass`. Exit 0 means pass; 1 means fail or not-run; 2 means the harness could not start.

## NO-GO

- Any arm `fail` — stop; do not promote G655.
- Any arm `not-run` (including the independent observer, the restart drills and the benchmarks) — the
  gate is **not** met. `not-run` is not a pass.
- `check-mainnet-gates.py --check` not green, or the launch gate exiting anything other than 3 on the
  other open rows.

## Rollback

Stop the devnet: `tools/devnet.sh down --volumes`. This is a disposable local chain; there is no
chain state to restore. The reserve is held by the genesis-created `MultiSigRevVault`; if it was
funded in the run, the funds are on the disposable chain.

## What this does not claim

No mainnet-scale, timing or economic-soundness claim; no public validation; Docker is never sole
evidence (the Rust integration suites and the Python encoding tests are in the record). Promoting
G655 does not close #428 and does not authorize a mainnet date.
