# c344-anchor-persistence-20261116

**Evidence locality.** `local-devnet`

## Scope

The `jail-vote` arm of `tools/liveness-enforcement-drill.py` on a disposable four-validator devnet
(stakes 700/100/100/100, epoch length 3, absence threshold 1, jail after 2 boundaries), with validator
3 stopped. The question: after the Phase 1 repair to `final_scope_rejections` and `MergeScope::merge`,
does the `pos:liveness-anchor` written by a boundary's close chain survive the next block's merge, so
the absence counter advances and a jail is imposed?

## Provenance

| | |
|---|---|
| gate | `C344` |
| source | `40b4396aab4cb143dc4b899ba83d3a197375263f` |
| binary | `ac608f8c72a19868f6bca5916036c2a17885ee0f6b190c6d00fc19fdf85664fb` |
| run | local devnet; no Actions URL |
| packaged | `2026-11-16T14:45:59+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The chain advanced to `maxObservedHeight: 11`, `height: 12`, but **finality stopped at height 2**. From
block `7e276d01…` onward the bootstrap's sidecar regeneration failed on every propose:

```
regenerated mergeable channels for block 7e276d01… but replay computed bb99bf10…
instead of fb760682… — no block was produced, so the chain will not advance until this is fixed
```

`getLiveness` returned an empty map throughout, the target's bond stayed at 100, and no vote window
opened; the drill recorded `jailed: false` and exited non-zero. **The anchor repair is therefore not
accepted end-to-end by this run** — the arm never reaches a jail.

Two things *are* observed. The run contains **zero** `final scope rejected` lines and **zero**
mentions of the anchor slot `04/688c6e…` among the reported native slots; the pre-repair evidence had
both. The derivation and merge-decision tests
(`system_deploy_ids_are_not_final_scope_rejections`,
`a_final_scope_system_deploy_chain_is_not_rejected`) pin the mechanism those lines exercised.

The same arm on the **unmodified base** `d4b9bfb` stalls identically at height 12 / finalised 2 with
the same `regenerated mergeable channels` error (different block hash); that diagnostic is recorded
under **C402**. The stall is therefore not caused by this repair; it is a pre-existing partition /
replay-sidecar disagreement that blocks the liveness arm. C344 stays `in progress`.

## What this does not establish

It does not show the anchor is present or absent on a chain that advances past the boundary; it does
not establish live-testnet behaviour, the vote-slash/partition/restart-expiry/exact-threshold arms, or
any claim about a bond being slashed. `execution_proven` is false. The only finite claim is the
observed stall and the absence of the final-scope-rejection lines.

## Reproduce

```sh
tools/devnet.sh build
DEVNET_PREFIX=c344f DEVNET_NETWORK=c344f DEVNET_GRPC_BASE=54402 \
DEVNET_HTTP_BASE=54403 DEVNET_ADMIN_BASE=54405 \
python3 tools/liveness-enforcement-drill.py --arm jail-vote \
  --out target/c344-jail-vote-20261116 --timeout 420
```

The base comparison used the same command with the `d4b9bfb` image built from
`git archive d4b9bfb | tar -x -C target/base-src` and `docker build -f target/base-src/docker/rnode/Dockerfile
-t rnode:local target/base-src`.
