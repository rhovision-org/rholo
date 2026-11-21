# rc5-local-20261121-e9508f5

**Evidence locality.** `local-devnet`

## Scope

RC-5, overload refusal: on a disposable devnet at source
`e9508f58c613e87e839de831f08c30cfe3685c59`, the ingress in-flight bound is configured and the
question is whether a request beyond the bound is refused explicitly with the named class and retry
signal, with no 5xx and no accepted work lost, while the unset/default control admits.

## Provenance

| | |
|---|---|
| gate | `RC-5` |
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

**PASS.** At the configured bound the capped lifetime refuses with HTTP `429` and `Retry-After: 1`
(class `inflight`, limit 2); the admission traces carry the refusal coordinates, no 5xx is returned,
the unset control admits, and no accepted deploy is left unresolved (`not_processed` is empty). The
existing `tools/m2-pressure.py::check_pressure` predicate is the judge. Reading: `safe` at
`e9508f58c`.

## What this does not establish

A local single-validator pressure arm with an in-flight bound. It does not exercise the public
endpoint/proxy surfaces, a byte- or deployer-share bound, or admission after a sustained overload —
a finite witness, not a universal liveness claim.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/release-check.py run --claim RC-5 --env local
```
