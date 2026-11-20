# rc3-local-20261120-0669419

**Evidence locality.** `local-devnet`

## Scope

RC-3, state agreement across a merge: on one binary, do four validators derive the same
`preStateHash`, `postStateHash` and `rejectedDeploys` for the same accepted block, and is a marker
written before the window readable at the same inclusion block on all four?

## Provenance

| | |
|---|---|
| gate | `RC-3` |
| source | `066941917e1b066b2a704141d1904be8329e1f7a` |
| binary | `d664fc030f1d4bc3d35ddd3414ea39b3aeff8638b14453f2e07460be0e16c0b8` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-20T11:53:16+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

PASS. The run deployed one marker, waited for it to be visible on every validator, waited for the
settlement route to name its inclusion block, and required that block finalised on all four. It then
compared the three state fields for every block hash held by all four validators. The record names
the inclusion block, the per-validator marker reads, and 34 common blocks; no block disagreed.

## What this does not establish

This is one workload on one local Docker bridge, not a safety proof. It says the four nodes agreed
on the observed blocks on this binary; it does not cover a different binary, a native-write case, an
equivocation, or the public fleet. A local pass is a mechanism check.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-3 --env local
```

The test boots `tools/devnet.sh up --validators 4 --fresh`, deploys a marker, samples
`/api/blocks/48` and `/api/block/{hash}` on every validator, and probes the marker at the
settlement's inclusion block. The source and binary pins are the table above.
