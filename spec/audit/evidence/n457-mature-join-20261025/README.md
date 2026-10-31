# #457 mature-join arm — 2026-10-25

A bonded validator is reset to an empty store on a four-validator devnet whose chain is already past
the size at which the live join stalled, and must LFS-restore the bootstrap's approved state.

## Provenance

| | |
|---|---|
| node source | `cdb00670c9922f943a5b485644627c5efe24e4b9` (`feat/n457-trustless-join`) |
| image | `rnode:local` `sha256:39b9b3d085d205a535849c76d03c2296cc24e422d0ee06befaceaacad52c542c` |
| arm | `tools/n457-mature-join-arm.py` |
| raw record | `mature-join.json` (sha256 in `manifest.json`) |
| network | local Docker bridge `n457net`, ports 55402/55403/55405 + 1000·i; disposable, no live testnet |

## Commands

```sh
cargo build --release -p rchain-node
tools/devnet.sh build
DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \
DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
    tools/devnet.sh up --validators 4 --fresh
DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \
DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
    python3 tools/n457-mature-join-arm.py --min-height 28 --timeout 600 \
        --out spec/audit/evidence/n457-mature-join-20261025
```

The arm waits until the three non-reset nodes are at 28+ rounds, records the bootstrap's live
finalized block, runs `tools/devnet.sh reset 3` (which discards the container and its volume and
restarts it with `--bootstrap`), then waits for the reset node to leave `NodeSyncing`.

## Result on this run

| assertion | reading |
|---|---|
| chain mature before the reset | bootstrap 29 rounds, finalised block 26; validators-1/2 at 29 |
| LFS restore completed | reset node left `NodeSyncing` in **39.3 s**; `status.sync` is cleared |
| restored anchor | block **23** `c4ce5b0f…`, held by the bootstrap |
| sync failures | `LFS state sync failed` **0**, `no block finished in` **0** |
| catch-up after restore | `latestBlockNumber` 30 at the reading; the moving tip is the windowed catch-up's |

The failure this arm is about is the one the live chain showed: the block walk finished 3 of 28 and
failed after three idle rounds. Here the walk of the same chain shape completes, the node writes the
approved state and enters `NodeRunning`.

## Non-claims

- **One** four-validator local devnet, one reset. A finite witness, not a universal join theorem, and
  not a certification of #457 on any live chain.
- The live join's 3-finished-of-28 counts were caused by **inbound reachability**, not by this walk
  defect: a controlled A/B on 2026-10-25 joined with every peer allowed inbound and stalled with only
  the bootstrap. This arm exercises the walk with a reachable peer; it does not reproduce that stall.
- The restored anchor (23) is **older than the bootstrap's live finality at capture (26)**: the
  approved-fringe store the bootstrap serves lags its live finality. That is a property of the
  responder, not a defect of this arm; the arm asserts the anchor is a block the bootstrap holds.
- Convergence to the tip is not asserted. Once the restore returns, the windowed catch-up owns the
  gap, and a fast-producing chain can outrun it.
- Fringe metadata is checked as far as Law 83 says: the derived `fringe` and the singleton state pin.
  The fringe's state hashes (root and per-block) are still assumed inputs (**C339**); a singleton pin
  against the named block's pre-/post-state was tried and refuted by the restore tests.
- The live testnet replay remains #421/#429's, and a live re-check of the join is the observation that
  counts.
