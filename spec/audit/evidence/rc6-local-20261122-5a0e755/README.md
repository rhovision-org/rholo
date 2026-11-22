# rc6-local-20261122-5a0e755

**Evidence locality.** `local-devnet`

## Scope

The local arm of RC-6: audit replay target: a genesis join and audit agree on the anchored target. It answers whether that behaviour holds on the current
candidate, on a disposable four-validator Docker devnet.

## Provenance

| | |
|---|---|
| gate | `RC-6` |
| source | `5a0e755be3d92ddf3843aa5fde493a622f58ca7c` |
| binary | `e5ae3e023d7e604175b723eba68a9cf035b898efb46fb169b66f8f5e33fe60ca` |
| run | local Docker devnet; no GitHub Actions run |
| reference | none |
| packaged | `2026-11-22T14:08:02+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The claim's own `check(record)` returned `pass` on this record. The observations and decision
traces are under `raw/`; the harness's own output is in the same directory.

## What this does not establish

A `local-devnet` pass is not public evidence, not a statement about four independent hosts, and not
release readiness. It is one run on one candidate: a finite witness, not a universal safety or
liveness theorem.

## Reproduce

```sh
RNODE_IMAGE=rnode:pr567-current tools/devnet.sh build
python3 tools/release-check.py run --claim RC-6 --env local
```
