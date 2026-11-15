# c377-anchored-join-20261115

**Evidence locality.** `local-devnet`

## Scope

A cold observer joins a retained contiguous chain through the **certified checkpoint path** and the
run answers four questions: does the joiner accept only the certified state (certificate verifies,
metadata commitment matches, the anchor post-state equals the bootstrap's); does it reach and retain
the bootstrap's finalized values; does it finalize a **new** block after the join; and does it restart
on the retained anchor without replaying? The chain was built by `tools/sync-bench.sh` for the 2,432
target and left producing until the freeze; the measured bootstrap was at height 2,798, finalized
2,795.

**The image predates Laws 108/109.** The pinned source `1f6b842af` carries the metadata commitment,
the measured eight-header close-history hydration and the approved-fringe root preference; it does
**not** carry the `AcceptedState` type or the committed validation window (`historyRoot`), which landed
after this run. This pack is evidence for the certified join, the commitment, finality and restart on
that binary; the anchor type and the window are established by their unit witnesses
(`casper/src/engine/anchor.rs`, `casper/src/checkpoint.rs::a_validation_window_is_bound_and_verifies`),
not by this run.

## Provenance

| | |
|---|---|
| gate | `C377` |
| source | `1f6b842af618c35a7206af8328e3dd367c3c8222` |
| binary | `6574b102640375944dfb496cf86128b1fef7df5527d73fe2c11d937671cf6903` |
| run | local (no hosted run URL; the host checkout at the run was `ed221e9a6`) |
| packaged | `2026-11-15T11:54:31+00:00` |
| verdict | **PASS** (the join predicate below; the latency target is separately missed) |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The U4 acceptance predicate passed with no reasons:

- **Anchor and root.** The certificate verified against the genesis active bonds (100 of 100 stake,
  one validator); `metadataCommitted` is true; the certified anchor post-state equals the bootstrap's
  post-state at that anchor.
- **Finality.** The bootstrap was finalized at 2,795; the joiner's certified anchor was at 2,795 and
  its observed height reached 2,798.
- **Post-join progress.** An `explore-deploy` plus an admin propose produced a block; the joiner
  observed it (2,798) and it finalized (2,795).
- **Restart.** Stop/start returned ready in 11.4 s with the certified anchor height and finalized
  height retained.
- **No replay.** The joiner log contains `replayed none` and no `RecoveryReplay`/replay marker. The
  certifying collector's own transient `active-bonds leaf not found` warnings are not part of the
  join (the observer does not attest); the predicate records them and does not fail on them.

**Measured restore: 817.1 s.** Decomposition: the state walk hydrated 10 roots in 63 s; the metadata
walk moved 2,795 headers in about 750 s, one header per round-trip (the chain is linear, so the
128-hash batch never has more than one hash in its frontier). The pre-registered plan ceiling
p50 ≤ 45.5 s is **missed**; the cause is the walk's per-header round-trip, and the fix is a metadata
height-range request (a named performance unit), not a re-baseline. No re-run is claimed here.

## What this does not establish

- It does not exercise the post-run `AcceptedState`/`historyRoot` code (see Scope); those are unit
  witnesses and Laws 108/109.
- It is one local run, not the plan's three-run p50/p95, and not a hosted CI run.
- It does not establish N4/N8 joins, a cold **bonded** validator (N≥4 is the liveness boundary under
  the strict rule), a partition/rejoin campaign, or the audit's `DISAGREE` on the retained
  divergence chain (the audit's verdict equivalence is Law 106 with its unit witnesses).
- It does not establish bond-set transitions, a wall-clock freshness window, or parent chaining;
  those are recorded design residuals.
- A PASS is a finite witness, not a universal safety theorem.

## Reproduce

The retained volume came from `tools/sync-bench.sh --heights 2432 --rebuild` (one-validator bootstrap,
autopropose), then was left producing to 2,798 and frozen. The measured network:

```sh
DEVNET_PREFIX=syncmeas DEVNET_NETWORK=syncmeas \
DEVNET_GRPC_BASE=62402 DEVNET_HTTP_BASE=62403 DEVNET_ADMIN_BASE=62405 \
  tools/devnet.sh up --validators 1 --observers 1 --fresh --admin \
  --no-autopropose --no-propose-on-deploy --data-volume synbuild20-bootstrap-data

DEVNET_PREFIX=syncmeas DEVNET_NETWORK=syncmeas \
DEVNET_GRPC_BASE=62402 DEVNET_HTTP_BASE=62403 DEVNET_ADMIN_BASE=62405 \
DEVNET_NODE_MEMORY=8g \
  python3 tools/c339_mature_replay.py --out <run-dir> --target-height 2432 --observer \
  --join-mode checkpoint --no-autopropose --no-propose-on-deploy --skip-up --skip-chain \
  --restore-timeout 1800
```
