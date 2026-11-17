# c357-current-build-20261117

**Evidence locality.** `local-devnet`

## Scope

The three self-replay recovery arms on a disposable three-validator devnet built from the current
source. The question: after a self-replay refusal, does the current build (1) recover by
attestation, (2) recover by reset, catch-up and automatic resume, and (3) halt and then exit only
through the operator resume — with finality advancing in every recovered arm?

## Provenance

| | |
|---|---|
| gate | `C357` |
| source | `89cac44d4c7b665d88d3eefed964818d35cdd016` |
| binary | `sha256:4a656971436902a3e0de863017623c5888dbdc7bb2651e005f46e314e36f28a5` |
| run | none — local devnet; the drill records no Actions URL |
| packaged | `2026-11-17T19:00:17+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**PASS — all three arms.** Arm 1 (`--self-replay-injection deploy:1`) recovered by broadcasting the
attestation-only block and finality advanced. Arm 2 (`all:2`) raised the reset, caught up, resumed
automatically and advanced finality. Arm 3 (`all:2 --no-self-replay-reset`) halted, stayed frozen,
and resumed only through the operator route, then advanced finality. The campaign aggregate is
PASS (3 arms). The no-user-deploy payload, the refused-set exclusion and the one-reset rule are the
unit witnesses in `spec/audit/closeout-phase-space-20261117.md`; this drill is the operational
ladder, not a substitute for them.

## What this does not establish

This is a single-host `local-devnet` witness, `execution_proven: false`. It is not the selected
release, not the section-6 lockstep rollout, not the independent category-A sign-off, and not the
live selected-candidate acceptance. It does not recover the incident's original rejected
block/trace bytes. It is a finite witness of the recovery ladder, not a universal safety or
liveness theorem, and it does not establish that a peer accepts the recovered chain.

## Reproduce

```sh
tools/devnet.sh build
tools/self-replay-drill.sh --timeout 600
tools/devnet.sh down -v
```

The source pin is `89cac44d4c7b665d88d3eefed964818d35cdd016`; the image digest is
`sha256:4a656971436902a3e0de863017623c5888dbdc7bb2651e005f46e314e36f28a5` (`rnode:local`).
