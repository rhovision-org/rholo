# local-check1-finality-20261119

**Evidence locality.** `local-devnet`

## Scope

Local release check 1 on the standard `local-4` profile: one host, Docker, four validators, 4 GiB
each (below the documented 8 GiB tier). The question is whether a signed transfer that every
validator reports as finalised is still present in the finalised state after two further epoch
boundaries. This is a mechanism check, not a hosted-runner acceptance run and not testnet evidence.

## Provenance

| | |
|---|---|
| check | `LOCAL-CHECK-1` |
| source | `91cde81905af7b660c23e19e7db6493500eda592` |
| binary | `d664fc030f1d4bc3d35ddd3414ea39b3aeff8638b14453f2e07460be0e16c0b8` |
| image | `32f28ab5b0d9` (built 2026-11-19 from the source above) |
| run | none — local Docker devnet; no Actions URL |
| reference | `spec/audit/release-checks-20261119.md` (stage 1, check 1) |
| packaged | `2026-11-19T20:34:43+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**FAIL: a finalised write disappeared from the finalised state.**

The drill wrote `@"acceptance:5b5487bf…"!(42)` in block
`33507f7aaaca6f83463667a69f05fb057683e1517f9f259f887c6196b48bb695` at height 13 (deploy signature
`304402202bfefb49…`, `errored: false`, cost 228).

- Every validator reported that block as finalised (`/api/is-finalized/33507f7a…` is `true` on all
  four) and the marker read back as `42` at the finalised block `c6f8f074…` at height 14.
- By the finalised head at height 46 (`9b6f8b58…`, the same hash on all four), the marker read back
  as `[]` on every validator.
- The node's own merge log names the deploy signature in the drop:
  `"2 deploy(s) rode on a final-scope rejected chain (304402202bfefb49…, …)"`, with the reason
  `"4 chain(s) rejected as forks: their block did not see the whole final fringe
  (Rchain.MergeScope.KeptRule)"`.

This is the C260 / M5-04 class: the finality oracle reports the block finalised, but the merge's
final-scope rule rejects its chain and the deploy's effect is absent from the later finalised
state. `spec/AUDIT.md`'s C260 row says "**No write is dropped**"; this run is a counterexample to
that sentence as written, unless "dropped" is redefined to exclude final-scope-rejected deploys.
The merge accounting explains the witness; it does not make the user-visible write reappear.

## What this does not establish

- It is one finite run on a local devnet, not a rate, a theorem, or a mainnet observation.
- The four validators ran at 4 GiB each, below the documented tier.
- It does not identify the correct fix. It does not show whether the finality oracle or the merge
  scope is the side to change.
- It does not claim that other deploys, or deploys whose blocks see the whole final fringe, behave
  the same way.
- `execution_proven` is false; the digests prove the bytes, not the run.

## Reproduce

```sh
DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance DEVNET_NODE_MEMORY=4g \
  tools/devnet.sh up --validators 4 --fresh
DEVNET_PREFIX=acceptance python3 tools/consensus-acceptance-drill.py \
  --validators 4 --local --window 240 --out target/rc/check1-n4
DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance tools/devnet.sh down -v
```

The drill is expected to fail on the current tree. The record preserves the raw observations, the
four container records, the causal logs and the failure envelope.
