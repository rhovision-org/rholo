# rc4-local-20261120-0669419

**Evidence locality.** `local-devnet`

## Scope

RC-4, recovery without loss: after one validator is SIGKILLed mid-burst and restarted, do the
survivors keep finalising, does an accepted deploy's effect survive at the post-recovery finalised
head, and does the victim rejoin the same finalised head without an own-key halt or self-replay?

## Provenance

| | |
|---|---|
| gate | `RC-4` |
| source | `066941917e1b066b2a704141d1904be8329e1f7a` |
| binary | `d664fc030f1d4bc3d35ddd3414ea39b3aeff8638b14453f2e07460be0e16c0b8` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-20T12:05:00+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**FAIL.** The pre-restart marker settled at every finalised head, and the harness killed validator
1. The container restarted (`StartedAt` changed from `11:53:18.659Z` to `11:58:38.837Z`), but the
node's `/health` never answered with a new `bootId` within the 240-second window, so the harness
stopped at `validator 1 did not restart with a new bootId` (`raw/restart/observations.jsonl`,
`raw/claim.json`).

The restarted node's log names the cause in its own words: its `CheckpointCollector` reports
`no checkpoint certificate this tick: the certificate is not quorate: 100 of 400 bonded stake
signed` (and later 200 of 400), and the node never reaches the API. Before the kill, only the
bootstrap held a verified checkpoint certificate; validators 1–3 held none
(`observations.checkpoint_certificates`). A validator with a DAG but no certificate cannot attribute
its accepted state, so the restart path does not complete.

## What this does not establish

The survivors were not read after the harness stopped; the record does not show whether they kept
finalising (the harness aborted before its agreement phase). The deploy-effect read at the
post-recovery head was therefore not taken. This is one local kill, not a host loss; and a restart
that does not complete says nothing about a restart that does.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-4 --env local
```

The test boots `DEVNET_PREFIX=restartload` with 2 GiB per validator, deploys a marker, waits for a
checkpoint certificate where one exists, then runs `tests/consensus-restart-under-load.py
--validators 4 --cycles 2 --window 240 --load-seconds 20`.
