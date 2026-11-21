# rc2-local-20261121-e9508f5

**Evidence locality.** `local-devnet`

## Scope

RC-2, safety: on a four-validator disposable devnet at source
`e9508f58c613e87e839de831f08c30cfe3685c59`, two partition/rejoin cycles are driven with a producing
victim, and the question is whether any finalised block is held by one validator but not finalised by
every validator at or above its height, and whether a finalised (height, hash) ever regresses.

## Provenance

| | |
|---|---|
| gate | `RC-2` |
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

**PASS** (`PASS_BOUNDED_WORKLOAD_PARTITION_REJOIN`). Both cycles retained the anchor on all four
validators, crossed an epoch boundary, produced from the victim, and rejoined. Containment holds at
or above every sampled height on every validator; no finalised (height, hash) regressed and no two
conflicting finalised histories were observed. The finality traces carry the whole bonded-map
denominator and the strict-supermajority predicate. Reading: `safe` at `e9508f58c`.

## What this does not establish

A local four-validator devnet with a bounded workload partition — not a host loss, not a declared
multi-host or M1/M4 topology, and not the public fleet. It is a finite witness over two cycles, not
a universal safety theorem.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/release-check.py run --claim RC-2 --env local
```
