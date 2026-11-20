# rc5-local-20261120-0669419

**Evidence locality.** `local-devnet`

## Scope

RC-5, overload refusal: with `RNODE_HTTP_MAX_INFLIGHT_DEPLOY=2`, does the listener refuse further
work in the named class (429/RESOURCE_EXHAUSTED) with a retry signal, return no 5xx, keep a handler
reachable after release, and admit when the cap is unset — and does every deploy it accepted remain
readable or explicitly failed?

## Provenance

| | |
|---|---|
| gate | `RC-5` |
| source | `066941917e1b066b2a704141d1904be8329e1f7a` |
| binary | `d664fc030f1d4bc3d35ddd3414ea39b3aeff8638b14453f2e07460be0e16c0b8` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-20T11:53:38+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

PASS. Two devnet lifetimes drove `tools/m2-pressure-arm.py phase` (capped and unset), the Rust gRPC
observation ran against the same listener shape, and `tools/m2-pressure.py check --locality
local-devnet` accepted the assembled record. The record carries the raw socket transcripts, the two
container configs, the version output naming `91cde81905af7b660c23e19e7db6493500eda592`, and the
capped/unset pressure counts.

## What this does not establish

The opt-in in-flight cap is not the shipping default; the pool's count/byte/term/share/rate bounds
are the intended throttle. This is a local listener, not a public proxy; transport correctness
across HTTP, direct HTTP, gRPC and a proxy is out of scope for this claim.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-5 --env local
```

The test boots two single-validator devnets at `DEVNET_PREFIX=m2cap` / ports 56402–56405, runs the
two phases and the gRPC observation, assembles `record/pressure.json`, and judges it with
`tools/m2-pressure.py check`.
