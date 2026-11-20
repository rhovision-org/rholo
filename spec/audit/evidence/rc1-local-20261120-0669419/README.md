# rc1-local-20261120-0669419

**Evidence locality.** `local-devnet`

## Scope

RC-1, truthful settlement: does a deploy that is finalised, and whose effect reads back at that
finalised head, remain present through two further epoch boundaries and never appear in a later
block's `rejectedDeploys`?

## Provenance

| | |
|---|---|
| gate | `RC-1` |
| source | `066941917e1b066b2a704141d1904be8329e1f7a` |
| binary | `d664fc030f1d4bc3d35ddd3414ea39b3aeff8638b14453f2e07460be0e16c0b8` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-20T11:54:00+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**FAIL.** The drill deployed one marker and observed it in the finalised post-state at block 3
(`e97adf3aedf9ea23f9cbaf5cf014c2aaea3ee983e4857b4c6c073886b09197d6`) on all four validators —
the marker read `42` on each. It then waited for retention at a later finalised head. At block 24
(`e7926197bd9df8b87f1ca00cd91752513ad77744ee1515afbb9959baf7bb89f0`) the marker read empty on
validator 0, and that block's `rejectedDeploys` names the marker deploy
`304402203433f69289b19b5fa80253fa8a90292785b403f5990a9d6b80d5f5cb370cdf410220512d0b8742173fef7745679bd7cd42b2e834fcdcb9b56655238963eebaea3894`.
A write that was finalised at block 3 was dropped from the state and recorded as a rejected deploy
at block 24.

The decisive events are `raw/decisive-events.jsonl`; the full poll stream is
`raw/observations.jsonl.gz`; the drill's failing tail is `raw/drill-tail.log`. This is the check-1
finding on the current build, now produced by the RC-1 test itself. It is recorded, not fixed.

## What this does not establish

The run sees one marker and one validator's read at block 24 before the drill raised; the other
validators were not probed at that head in this run. It does not establish how many deployments are
affected, whether the drop depends on the deploy pool or the merge scope, or whether a restart would
recover it. A local devnet is not the public fleet.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-1 --env local
```

The test boots `DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance tools/devnet.sh up --validators 4
--fresh --no-autopropose --epoch-length 10`, runs `tools/consensus-acceptance-drill.py --local`,
and reads the settlement route and the finalised marker at every validator.
