# c344-cold-rejoin-20261117

**Evidence locality.** `local-devnet`

## Scope

The `cold-rejoin` arm of `tools/liveness-enforcement-drill.py` on a disposable four-validator
devnet (stakes 100 each, epoch length 3, liveness disabled). The question: after `devnet.sh reset 3`
discards validator 3's container and data volume, does the wiped node join the running chain from its
certificate, reach readiness, and produce a block that finalises?

## Provenance

| | |
|---|---|
| gate | `C344` |
| source | `3a3c372eaca93a02b48239cf861e9a0a4af4dbfd` (the tree the image was built from) |
| binary | `6d66d6c552cfe6f43f7ee07df87632ea03ede52848595b8ae1214d4071935adf` (image `f30845d6c5b9…`) |
| run | local devnet; no Actions URL |
| packaged | `2026-11-17T13:27:48+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

**Provenance note.** `raw/result.json` records `source_sha` `29f229218…` because the drill reads
`git HEAD` when the arm finishes; the `deny.toml` commit landed while the image was already built and
does not change the binary. The pin above is the built tree `3a3c372e…`, confirmed by the node's
`/api/status` version string in `raw/validator-3-status.json`.

## Result

**The wiped node served again but never produced.** The arm's first wait (`finalizedHeight >= 20`)
passed; `devnet.sh reset 3` ran; the node answered `/api/status` (`rejoined_and_serves` was reached);
then it produced no block signed by validator 3 above the reset height in 1200 s. `arm_completed` is
false and the verdict is FAIL.

The node's own status at the end says why the arm could not finish: `latestBlockNumber` **21**,
`checkpoint.anchorHeight` **20**, `syncPhase: running`, while the bootstrap was at height 204 /
finality 200. The joiner took the certificate at the first certifiable anchor and then had to backfill
184 blocks; it did not.

Its log names the fault that stopped it (`raw/validator-3.log`):

```
ERROR [coop.rchain.node.instances.ProposerInstance] propose failed with an internal error:
failed to regenerate mergeable channels for block 034adb13… : InternalError("unknown root")
— no block was produced, so the chain will not advance until this is fixed
```

The same log shows the backfill thrashing —
`Could not retrieve requested block … from devnet-bootstrap. Asking peers again.` and
`backfill ingress full; dropping a body … (it will be re-requested)` — and the collector reporting
`no checkpoint certificate this tick: … 100 of 400 bonded stake signed`.

**Reading.** The unit fix covers the first post-anchor block when the certificate's join inputs are
held: `casper/tests/checkpoint_join.rs` builds the wiped store and validates that block end to end.
The live run exposes the next missing input on the same path: `regenerate_sidecars` forks a replay at
a block's **pre-state root**, and the joiner does not hold the root it needs for the block the
proposal path asks it to index. The certified window hydrates the roots the merge is allowed to read;
it does not hydrate every pre-state a backfilled scope block names. That root is the defect this
FAIL records.

A second, instrument-level fact is recorded rather than acted on: the arm resets as soon as finality
reaches 20, so the certificate it joins from is the first one, and the backfill is at its largest.
A later arm should let the chain mature before the wipe so the join is measured at a recent anchor;
that is a change to the instrument, not a re-run of this result.

## What this does not establish

This is one FAIL on one local devnet, not a claim that the join cannot work, and not a PASS for any
part of it. It says nothing about the `restart-expiry` release/renewal arm, public-testnet behaviour,
or the collector's quorum behaviour. `execution_proven` is false.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/liveness-enforcement-drill.py --arm cold-rejoin \
  --out target/c344/cold-rejoin-v1 --timeout 1200
```
