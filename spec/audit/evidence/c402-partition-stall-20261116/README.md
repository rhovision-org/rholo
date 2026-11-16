# c402-partition-stall-20261116

**Evidence locality.** `local-devnet`

## Scope

An isolation run of the same `jail-vote` arm as
`spec/audit/evidence/c344-anchor-persistence-20261116/`, against the **unmodified merged base**
`d4b9bfbaf529fa754630c0ddfc695a5f2b62e576` (the #596 merge). The question: does the pre-Phase-1
binary also fail to index a received block and stall finality after a validator is stopped?

## Provenance

| | |
|---|---|
| gate | `C402` |
| source | `d4b9bfbaf529fa754630c0ddfc695a5f2b62e576` (image built from `git archive d4b9bfb`) |
| binary | `c4ded5f141b36b475db3b9f634045abc2f5a35d4fc21d8d687e8d4d2f8f32f9a` |
| run | local devnet; no Actions URL |
| packaged | `2026-11-16T14:51:00+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound. The drill's own `result.json` records `source_sha` as the worktree HEAD `40b4396` because it
reads the tree it is launched from; the **binary pin above** is the source of record for this run.

## Result

The base chain advanced to `maxObservedHeight: 11`, `height: 12`, with finality stopped at height 2 —
the same shape as the Phase 1 run. The bootstrap failed sidecar regeneration from block
`ab5b9b84…` onward:

```
regenerated mergeable channels for block ab5b9b84… but replay computed a59dfb5f…
— no block was produced, so the chain will not advance until this is fixed
```

`getLiveness` stayed empty, no jail was imposed, and no vote window opened. **The stall is
pre-existing on the merged base**, not introduced by C344's `final_scope_rejections` filter or the
`info.thn(0)` correction. This run also has zero `final scope rejected` lines, so it does not
reproduce the original anchor-loss log either; the failure is a different partition/replay-sidecar
disagreement.

## What this does not establish

It does not diagnose the root cause (the replay's `participation_for_block` or the sidecar
regeneration path is the suspect), and it does not establish live-testnet behaviour. It is an
isolation control: it shows the same stall without the Phase 1 repair, so the repair is not the cause.
The drill's internal `source_sha` is the launching tree's HEAD, not the image source; the binary
digest is authoritative.

## Reproduce

```sh
git archive d4b9bfbaf529fa754630c0ddfc695a5f2b62e576 | tar -x -C target/base-src
docker build --build-arg GIT_HEAD_COMMIT=d4b9bfbaf529fa754630c0ddfc695a5f2b62e576 \
  -f target/base-src/docker/rnode/Dockerfile -t rnode:local target/base-src
DEVNET_PREFIX=c344b DEVNET_NETWORK=c344b DEVNET_GRPC_BASE=55402 \
DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
python3 tools/liveness-enforcement-drill.py --arm jail-vote \
  --out target/c344-base-drill --timeout 240
```
