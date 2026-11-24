# G655 — forkless developer rewards: the offline record (2026-11-24)

**Verdict: the service is implemented and green offline; the live gate arm is not run.** G655 stays
`owes`. Nothing here is a mainnet, scale or economic-soundness claim.

## What was built (PR-C, ordinary post-genesis deploys)

- `examples/devrewards/BeneficiaryRegistry.rho` — append-only, self-scoped, anchor-versioned.
- `examples/devrewards/Treasury.rho` — a fresh unforgeable vault; inactive means held; activation by
  a threshold of distinct install-time validator keys; `pay` on a private name held only by the
  settlement.
- `examples/devrewards/Settlement.rho` — one accepted manifest per epoch, claim against its Merkle
  root, paid-after-transfer, conflicts recorded, unclaimed carry-forward.
- `examples/devrewards/install.rho` + `tools/devrewards_install.py` — the single install deploy.
- `tools/devrewards_manifest.py` — `devrewards-usage-v1` attribution, canonical leaf/node/digest,
  proofs, `build`/`verify`/`self-test`.
- `tests/fixtures/devrewards_manifest_golden.json` — the shared encoding pin.
- `tools/devrewards-experiment.py` — the witness's verdict engine (`self-test`); it reports `not-run`,
  never a green, for an arm that was not measured.
- `docs/src/node/developer-rewards.md`, `docs/src/spec/failure-hazop.md` §U27.

## Measured (this worktree, 2026-11-24)

| check | result |
|---|---|
| `cargo test -p rchain-casper --test devrewards` | 1 passed |
| `cargo test -p rchain-casper --test devrewards_flows` | 4 passed (registry, settlement, under-funded retry, replayed attestation) |
| `cargo test -p rchain-casper --test devrewards_attribution` | 1 passed (two identical installs have distinct consume hashes; an invocation resolves to exactly one) |
| `cargo test -p rchain-casper --test devrewards_manifest_encoding` | 2 passed (golden leaf/root/digest; proof fold) |
| `python3 -m unittest tests.test_devrewards_manifest` | 15 passed |
| `python3 tools/devrewards_install.py --self-test` | ok |
| `python3 tools/devrewards_manifest.py self-test` | ok |
| `python3 tools/devrewards-experiment.py self-test` | ok |
| `tools/check-hazop-worksheet.sh --gate` | every node dispositions all eleven guide words (27 nodes, 165 rows) |

The settlement flow test also replays the paying claim with `with_cost_accounting = true`; the
replayer refuses a cost mismatch, so the state-hash assertion is not the only check.

## Live smoke, measured on a production genesis (partial arm)

The node image was built from this tree (`rnode:devrewards`, `GIT_HEAD_COMMIT=1e934101b...`). A
**non-dev** devnet was started, namespaced to avoid a peer session's containers and host ports:

```bash
DEVNET_PREFIX=g655 DEVNET_NETWORK=g655 DEVNET_GRPC_BASE=41402 \
DEVNET_HTTP_BASE=41403 DEVNET_ADMIN_BASE=41405 RNODE_IMAGE=rnode:devrewards \
tools/devnet.sh up --validators 1 --fresh --production-genesis --developer-share 500 \
  --developer-reserve-keys <pub0>,<pub1> --developer-reserve-quorum 2
# exit 0; the bootstrap reached Running and produced blocks (latestBlockNumber=8)

tools/devnet.sh deploy devrewards/install.rendered.rho
# Response: Success!  DeployId 3045...fc6   (block 4)
```

Read back from the chain: `@"devrewards/treasury-address"` holds
`11112fZs4p6axeZdLgKViKtYJmFWgFGUEUCMDz9wNM4QvpNnaqSdVn`, published by the install deploy in block 4.
So the plan's main feasibility risk — **a non-dev genesis that funds a deployer and accepts an
ordinary signed deploy** — is measured, and the DevRewards install runs on it.

What this arm does **not** do: it is one validator, not four; it does not fund the treasury from the
reserve, build a manifest from block events, or claim. A first attempt under the default container
prefix collided with another session's `devnet-bootstrap` (the name is shared); the namespaced run
above is the record.

## Not run (and why the gate is not met)

- **The live four-validator production genesis.** The devnet now has `--production-genesis` (no
  `--dev-mode`, the reserve genesis parameters, a funded deployer, no `--deployer-private-key`), and
  the image build was started from this tree. The live arm is recorded by
  `tools/devrewards-experiment.py run`; until it writes `"result": "pass"`, this section stays.
- **The independent observer.** The node CLI in this build has no block-event export
  (`CommEvent.consume.hash`), so the observer cannot recompute the manifest from finalized blocks.
  The experiment records that arm as `not-run` with this reason rather than substituting the
  experiment's own deploy log — a self-report is not an independent recomputation.
- **Restart drills, benchmarks, compatibility and the negative live arms.** They depend on the
  observer and are `not-run`.

## Reproduce

`spec/audit/runbooks/g655-forkless-devrewards-20261124.md` has the commands, the expected exit codes,
the NO-GO list and the rollback. The experiment's verdict is the acceptance surface: exit 0 only when
every required arm is `pass`.
