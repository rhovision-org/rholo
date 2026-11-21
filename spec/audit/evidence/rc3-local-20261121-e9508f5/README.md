# rc3-local-20261121-e9508f5

**Evidence locality.** `local-devnet`

## Scope

RC-3, state agreement across a merge: on a four-validator disposable devnet at source
`e9508f58c613e87e839de831f08c30cfe3685c59`, a bounded window of accepted blocks is sampled and the
question is whether every validator derives the same `preStateHash`, `postStateHash` and
`rejectedDeploys` for each block, and whether a kept marker is readable at the same block on every
validator.

## Provenance

| | |
|---|---|
| gate | `RC-3` |
| source | `e9508f58c613e87e839de831f08c30cfe3685c59` |
| binary | `88a4c7614df98dd3fb6fc0a8039897032f33dd89266321ae21c40bd5eccd61c0` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-21T10:14:42+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**PASS.** Every sampled block carries the same pre-state, post-state and kept/rejected ledger on all
four validators. The marker written by the deploy is present at the same block (`b1e19370…`) on every
validator, and the state traces recompute each claimed pre-state and rejection set. Reading: `safe`
at `e9508f58c`.

## What this does not establish

A bounded local window on one devnet. It does not cover the public fleet, a mixed-version or
multi-host window, or every possible merge shape — a finite witness, not a universal agreement
theorem.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/release-check.py run --claim RC-3 --env local
```
