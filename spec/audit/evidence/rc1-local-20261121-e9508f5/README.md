# rc1-local-20261121-e9508f5

**Evidence locality.** `local-devnet`

## Scope

RC-1, truthful settlement: on a four-validator disposable devnet at source
`e9508f58c613e87e839de831f08c30cfe3685c59`, a signed marker deploy is submitted and the question is
whether the settlement surface's `finalizedSuccess` + `settlementConfirmed` is truthful and durable —
is the deploy's effect present at the finalised head on every validator, and does it remain present?

## Provenance

| | |
|---|---|
| gate | `RC-1` |
| source | `e9508f58c613e87e839de831f08c30cfe3685c59` |
| binary | `88a4c7614df98dd3fb6fc0a8039897032f33dd89266321ae21c40bd5eccd61c0` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-21T10:14:41+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**PASS.** The settlement response is `finalizedSuccess` with `settlementConfirmed: true` on all four
validators, naming the same inclusion block `860936ed…` at height 25. The marker written by the
deploy reads back at each validator's finalised head (heights 26–27), the inclusion block is
finalised on every validator, and the deploy id appears in no later `rejectedDeploys` on any of the
four. The merge traces carry the four predicates the phase-space evaluator recomputes. Reading:
`safe` at `e9508f58c`.

## What this does not establish

One deploy on one four-validator local devnet. It is not the public fleet, not the M1/M4 topologies,
not a partition or a host loss, and not a proof that every settlement response is truthful — a
finite witness, not a universal safety theorem.

## Reproduce

```sh
tools/devnet.sh build                     # the image label must equal the source pin above
python3 tools/release-check.py run --claim RC-1 --env local
```
