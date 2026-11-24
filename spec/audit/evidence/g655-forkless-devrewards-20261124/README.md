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

## Live four-validator arm, measured (2026-11-24)

Image: `rnode:devrewards`, built from this tree with `GIT_HEAD_COMMIT=1e934101b`. Devnet: **four
validators plus one read-only observer**, production genesis (no `--dev-mode`), `developer-share
500`, a 2-of-2 reserve over validator keys 0 and 1, reporting enabled on the observer, namespaced
containers/ports.

| step | measured |
|---|---|
| genesis + finality | five nodes healthy; the observer finalized block 1 and later 51, 112, 129 |
| install | `devrewards/install.rendered.rho` succeeded as an ordinary signed deploy; `@"devrewards/treasury-address"` = `1111o8xM2GTeLyGC9iipZg3EvgvyqH9mycSzTVzZyoQTssov4NzLk` |
| reserve → treasury | validator 0's `transfer` of the reserve's balance **4511** recorded nonce 0; validator 1's `confirm` returned `(true, (false, "done"))` |
| top-up | 200 000 REV from the deployer's own vault. **Why:** a 5 % reserve is, by construction, a fraction of the installer's own burn, so it cannot cover the installer's full leaf; the top-up is stated rather than hidden |
| beneficiary | validator 0 registered `111121VP3YyBCToBxa5ikc2nSgh3XK9P9F9bc3LmCZpZPpwnG3Subk`; `beneficiaryOf(v0, 51)` returned it |
| observer manifest | the Rust observer (`node/examples/devrewards_observer.rs`) and `tools/devrewards_manifest.py` produced the **same root** `8ea03da9…d9c` and digest at anchor 51 |
| accept | 2-of-2 secp256k1 attestations over the canonical digest were accepted on chain |
| inactive | a claim before activation was refused `treasury inactive` |
| activation | 2-of-2 over `blake2b256("devrewards-activate-v1" \|\| treasuryAddress)` activated the treasury |
| claim | **123 871** paid once to the anchor-resolved beneficiary; the target balance became 123 871 |
| negative arms | a replayed claim refused `already paid`; an over-claim refused `invalid proof`; a conflicting manifest for the epoch refused `conflicting manifest` |
| restart | validator 3 stopped and restarted; after a post-restart deploy finality advanced 112 → 117 → 129 and the target balance stayed 123 871 |
| benchmark | the identical no-op term cost **129** at block 117 and **129** at block 128 |

**The genesis report is refused by the node**: `getEventByHash` on block 0 fails with
`ReplayException: Unused COMM event: replayData multimap has 17 elements left`, and the report API
is served only by a read-only node. The observer therefore attributes post-genesis usage from height
1; genesis deploys are not developer usage.

## Not run — and why G655 stays `owes`

The gate's own pass condition also requires **injected service crashes** (not only a validator
restart) that converge to exactly-once balances, and **one protocol version and one finalized state
on every node**. Neither was measured: the restart arm restarted one validator; no observer crash
between accept and claim, no repeated crash loop, and no five-node state/protocol census. The formal
witness `tools/devrewards-experiment.py::run_gate` also does not yet drive the live flow; the
measurements above were driven by `tools/devrewards_live.py` and the observer example. Promoting the
row on this record would claim more than was measured.

- **The live four-validator production genesis.** The devnet now has `--production-genesis` (no
  `--dev-mode`, the reserve genesis parameters, a funded deployer, no `--deployer-private-key`), and
  the image build was started from this tree. The live arm is recorded by
  `tools/devrewards-experiment.py run`; until it writes `"result": "pass"`, this section stays.
- **The independent observer.** The node exposes the report tree over gRPC —
  `getEventByHash(ReportQuery) returns EventInfoResponse` (`models/proto/deploy_service_v1.proto`,
  served at `node/src/api/grpc/deploy_grpc_service_v1.rs`), which carries `CommEvent.consume.hash`
  and the deploy's `ConsumeEvent.hash` — but **no CLI command wraps it**, so the observer has nothing
  to call yet. A small gRPC client that walks finalized blocks and calls `getEventByHash` is the
  unblocker. The experiment records the arm as `not-run` rather than substituting its own deploy log:
  a self-report is not an independent recomputation.
- **Restart drills, benchmarks, compatibility and the negative live arms.** They depend on the
  observer and are `not-run`.

## Reproduce

`spec/audit/runbooks/g655-forkless-devrewards-20261124.md` has the commands, the expected exit codes,
the NO-GO list and the rollback. The experiment's verdict is the acceptance surface: exit 0 only when
every required arm is `pass`.
