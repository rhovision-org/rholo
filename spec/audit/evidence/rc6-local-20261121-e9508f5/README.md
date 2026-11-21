# rc6-local-20261121-e9508f5

**Evidence locality.** `local-devnet`

## Scope

RC-6, join and audit: on a four-validator disposable devnet at source
`e9508f58c613e87e839de831f08c30cfe3685c59`, a validator's store is wiped and it rejoins from a
bootstrap, and the question is whether it restores and hydrates the certified anchor and whether
`rnode audit` reproduces the joiner's checkpoint claim. This re-run is after the `build_audit`
command routing, the anchored recovery target and the boot-trace code landed on `dev`.

## Provenance

| | |
|---|---|
| gate | `RC-6` |
| source | `e9508f58c613e87e839de831f08c30cfe3685c59` |
| binary | `88a4c7614df98dd3fb6fc0a8039897032f33dd89266321ae21c40bd5eccd61c0` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-21T10:14:43+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**PASS.** The join half passed (`join_result: pass`, `chain_problems: []`): the passive replicator
reset, caught up, was activated, and the six-stage chain check found no problem. The joiner's
operator status named checkpoint digest `99559a2e…` at anchor height 25. `rnode audit` then exited 0
with verdict **AGREE** over 101 blocks, `checkpointDigest` equal to the joiner's digest, and
`derivedRoot == claimedRoot == 86701b60…`. Reading: `safe` at `e9508f58c`.

## What this does not establish

One local reset/join on one devnet. The audit is a replay agreement for this anchor; it is not a
universal theorem, not the public-fleet join, and not the legacy/no-commitment certificate arm.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/release-check.py run --claim RC-6 --env local
```
