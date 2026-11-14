# c343-proposer-isolation-20261114

**Evidence locality.** `local-devnet`

## Scope

The operational arm `U20-01/07/09/11` named as owed: on a disposable four-validator devnet, stop
validators 1–3 so only the bootstrap remains, and let its tip outrun the last finalized height by the
policy bound (`MAX_PROPOSAL_FINALITY_LAG = 32`). The drill asserts that the proposer latches
isolation, that `/health` and `GET /api/v1/status/operator` name the halt and the reason, that the
halted tip stays flat and an explicit `POST /api/v1/propose` adds no block, that
`POST /api/v1/proposer/resume` clears the latch, and that the next proposal **re-halts** while
finality is still frozen.

## Provenance

| | |
|---|---|
| gate | `C343` |
| source | `6964e875bb878c7d9d22b4360815de2d8ab19674` (the `rnode:local` image's build source) |
| drill | `5f1848ecb` (`tools/proposer_isolation_drill.py`, the run's `source_sha`) |
| binary | `cf186f4d6358a1b007963895187c2ab1634cc2660ff6f91d19f561acaa93980b` |
| run | local devnet; no Actions URL (the tool records no `run.url`) |
| packaged | `2026-11-14T13:50:13+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

All twelve assertions passed. At the halt, `/health` reported
`proposalHalted: true`, `ready: false`, `readyReasons: ["proposal_isolated"]`,
`finalizedLag: 32`, and `proposalHaltReason: "tip 37 is 33 heights beyond last finalized 4
(bound 32)"`. The operator route carried the same gate, the tip stayed flat across the 15 s window,
and an explicit propose added no block. The resume route answered
`{"message": "proposal isolation cleared; the next proposal re-checks the finality bound"}`; the
cleared moment was observed before autopropose re-latched, and the next proposal re-set the latch
because finality was still frozen. The 443 proposal attempts in `raw/result.json` are the round-escape
ticks the lone validator needed to reach the bound (~15 s per block with the round unable to close).

## What this does not establish

This is a single-host `local-devnet` witness, not the live B/C/D evidence C343 still names: the
pipeline phase the live incident was in (NodeSyncing recovery versus the NodeRunning processor fault)
still needs the old hosts' read-only `status/operator` and block-6 journal lines. The isolation bound
is node-local operating policy and is not a consensus rule; the drill does not establish that a peer
accepts the bounded chain or that any other node's view agrees. It is a finite witness of the
propose/attest halt and resume, not a universal safety or liveness theorem, and it does not exercise
`U19-11`'s aggregate backlog surface.

## Reproduce

```sh
tools/devnet.sh build
DEVNET_PREFIX=c343 DEVNET_NETWORK=c343 \
DEVNET_GRPC_BASE=44402 DEVNET_HTTP_BASE=44403 DEVNET_ADMIN_BASE=44405 \
python3 tools/proposer_isolation_drill.py --out target/c343-proposer-isolation
```

The pins above are the source commit and the executable SHA-256 the run used. `tools/devnet.sh` is
disposable: `DEVNET_PREFIX=c343 DEVNET_NETWORK=c343 tools/devnet.sh down -v` removes the network,
containers and volumes.
