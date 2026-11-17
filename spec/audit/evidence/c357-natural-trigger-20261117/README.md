# c357-natural-trigger-20261117

**Evidence locality.** `local-devnet`

## Scope

The natural pending-peek replay-order trigger on a fresh two-validator devnet. The question: on the
current build, does a corrected proposer broadcast the natural pending-peek block, does a second
corrected node strictly replay it at the same per-deploy accounting and post-state, and do both
finalise it — while the pre-#570 image, run as a control, still refuses its own block with
`ReplayCostMismatch`?

## Provenance

| | |
|---|---|
| gate | `C357-natural-trigger` |
| source | `89cac44d4c7b665d88d3eefed964818d35cdd016` |
| binary | `sha256:4a656971436902a3e0de863017623c5888dbdc7bb2651e005f46e314e36f28a5` |
| run | none — local devnet; the drill records no Actions URL |
| packaged | `2026-11-17T19:02:49+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**PASS — both selected arms.** The green arm (`rnode:local`) produced inclusion block
`efe796b44acb422b2bb4444148b2b3ee0ac1e9835d458d69908db4a539f436fe`; both nodes held the same block
with identical per-deploy accounting and post-state and finalised it. The red control
(`rnode:n564-pre568`) refused its own natural-trigger block with `ReplayCostMismatch`, which is the
defect shape the current build fixes. The campaign aggregate is PASS.

## What this does not establish

This is a single-host `local-devnet` witness, `execution_proven: false`. It is not the selected
release, the section-6 lockstep rollout, the independent category-A sign-off, or the live
selected-candidate acceptance. It does not recover the incident's original rejected block/trace
bytes. It is a finite witness of the two-node same-block replay, not a universal safety theorem,
and it does not establish that a peer outside this devnet accepts the chain.

## Reproduce

```sh
tools/devnet.sh build
tools/n564-natural-trigger-drill.sh --green-image rnode:local --timeout 600
tools/devnet.sh down -v
```

The source pin is `89cac44d4c7b665d88d3eefed964818d35cdd016`; the green image digest is
`sha256:4a656971436902a3e0de863017623c5888dbdc7bb2651e005f46e314e36f28a5`. The red control is the
retained `rnode:n564-pre568` image, recorded in the run's `red-image.txt`.
