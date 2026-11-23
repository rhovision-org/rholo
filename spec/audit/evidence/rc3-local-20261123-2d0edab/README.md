# rc3-local-20261123-2d0edab

**Evidence locality.** `local-devnet`

## Scope

The local arm of RC-3, state agreement across a merge: every validator reports the same
`preStateHash`, `postStateHash` and `rejectedDeploys` for every accepted block in the window, and a
kept marker is readable at the same block on every validator. It answers whether that holds on the
campaign candidate, on a disposable four-validator Docker devnet, after the liveness/native-state
changes that made the previous record stale.

## Provenance

| | |
|---|---|
| gate | `RC-3` |
| source | `2d0edabc792b839ac42cc03d3a6e47c2a90473e6` |
| binary | `fa4f3d8e4332532fe029f377ad8ce93a07bfc0ae4b7b3b2c5f4689017405e7ba` |
| run | local Docker devnet; no GitHub Actions run |
| reference | none |
| packaged | `2026-11-23T18:21:50+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The claim's own `check(record)` returned `pass` on this record. The observations and decision
traces are under `raw/`; the harness's own output is in the same directory. This record replaces
`rc3-local-20261122-5a0e755`, whose source predates the liveness/native-state changes.

## What this does not establish

A `local-devnet` pass is not public evidence, not a statement about independent hosts, and not
release readiness. It is one run on one candidate: a finite witness.

## Reproduce

```sh
RNODE_IMAGE=rnode:campaign-20261123 tools/devnet.sh build
python3 tools/release-check.py run --claim RC-3 --env local
```
