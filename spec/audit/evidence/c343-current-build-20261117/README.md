# c343-current-build-20261117

**Evidence locality.** `local-devnet`

## Scope

The U20-01/07/09/11 isolation halt/resume arm on a disposable four-validator devnet built from the
current source. The question: with validators 1–3 stopped, does the surviving bootstrap halt at
`MAX_PROPOSAL_FINALITY_LAG = 32`, name the bound on `/health` and `/api/v1/status/operator`, refuse
an explicit propose, clear on the operator resume, and re-halt while finality is still frozen? The
run also records the current `/health` aggregate `blockPipeline` fields.

## Provenance

| | |
|---|---|
| gate | `C343` |
| source | `89cac44d4c7b665d88d3eefed964818d35cdd016` |
| binary | `sha256:4a656971436902a3e0de863017623c5888dbdc7bb2651e005f46e314e36f28a5` |
| run | none — local devnet; the drill records no Actions URL |
| packaged | `2026-11-17T19:00:32+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**PASS — every isolation assertion.** At the halt, `/health` reported `proposalHalted: true`,
`ready: false`, `readyReasons: ["proposal_isolated"]` and
`proposalHaltReason: "tip 36 is 33 heights beyond last finalized 3 (bound 32)"`; the tip stayed
flat across the window, an explicit `POST /api/v1/propose` added no block, the operator route
carried the same gate, the resume cleared the latch, and the next proposal re-latched while
finality was frozen. The run needed 451 proposal attempts to reach the bound. The captured
`/health` also carries `blockPipeline.{validatedQueue,incomingQueue}` (both 0 at rest), which is the
aggregate backlog surface the current build adds. The fail-closed pipeline-fault phase is not
exercised here: the devnet has no pipeline-fault injection, so that cell is the `node_phase` unit
test plus the existing deploy/propose gates.

## What this does not establish

This is a single-host `local-devnet` witness, `execution_proven: false`. It is not the live B/C/D
incident diagnosis: the old hosts' read-only `/health` and `/api/v1/status/operator` snapshots and
the block-6 journal lines are still owed, and they may still run a pre-fix build. The devnet is
single-shard, so the per-shard phase cells are unit-witnessed, not live-witnessed. No deployment,
restart or key use occurred. It is a finite witness of the halt/resume contract, not a consensus
safety theorem.

## Reproduce

```sh
tools/devnet.sh build
DEVNET_PREFIX=c343 DEVNET_NETWORK=c343 DEVNET_GRPC_BASE=44402 \
DEVNET_HTTP_BASE=44403 DEVNET_ADMIN_BASE=44405 \
python3 tools/proposer_isolation_drill.py --out target/c343-proposer-isolation
DEVNET_PREFIX=c343 DEVNET_NETWORK=c343 tools/devnet.sh down -v
```

The source pin is `89cac44d4c7b665d88d3eefed964818d35cdd016`; the image digest is
`sha256:4a656971436902a3e0de863017623c5888dbdc7bb2651e005f46e314e36f28a5` (`rnode:local`).
