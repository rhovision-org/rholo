# rc4-local-20261121-e9508f5

**Evidence locality.** `local-devnet`

## Scope

RC-4, recovery without loss: on a four-validator disposable devnet at source
`e9508f58c613e87e839de831f08c30cfe3685c59`, a marker deploy settles, one validator is SIGKILLed
mid-burst and restarted, and the question is whether the survivors keep finalising, whether the
marker survives at the post-recovery finalised head, and whether the victim rejoins the same
finalised head without an own-key halt or a spurious self-replay. This re-run is after the boot
decision trace landed on `dev`; the failure scan was also widened to name the terminal-sync line
rather than the collector's per-tick warning.

## Provenance

| | |
|---|---|
| gate | `RC-4` |
| source | `e9508f58c613e87e839de831f08c30cfe3685c59` |
| binary | `88a4c7614df98dd3fb6fc0a8039897032f33dd89266321ae21c40bd5eccd61c0` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-21T10:14:44+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**FAIL.** The marker settled before the kill, and the victim was killed and restarted (its container
start time changed from `10:03:42Z` to `10:09:02Z`), but its `/health` never answered and the harness
stopped at `validator 1 did not restart with a new bootId`.

The victim's own log names the refusal, and the widened scan now keeps it:

```
boot: no checkpoint certificate was stored; fetched and adopted the configured source's verified
certificate bb717e18… (served supporting stake 300 of 400)
boot: the stored checkpoint certificate names another fringe than the accepted fringe (4 certified,
1 accepted) and the accepted fringe does not descend from its anchor; the accepted state is not
re-verified by it
Fatal: NodeLaunch exited with error: this node's DAG holds 784 block(s) but no verified checkpoint
certificate; the accepted state cannot be attributed, so the node refuses to run (a genesis anchor is
only for a genesis-only DAG)
```

The certificate the victim fetched was **quorate** (300 of 400). The defect is that the certificate
is **ahead** of the victim's stored accepted fringe: the victim was down for about five minutes, so
the bootstrap's certified anchor moved from height 189 (the certificate it held before the kill) to
357, while the victim's approved fringe is a single block from before the kill. The boot agreement
handles an accepted fringe that is *newer* than the certificate's anchor, not one that is *older*, and
a node that cannot attribute its accepted state refuses to run (Law 108).

Before the kill only the bootstrap held a certificate (anchor 189); validators 1–3 held none. The
boot trace records the successful acquisition (`accepted: true`, `outcome: certified`) but not the
later fringe-agreement refusal — the trace is written before the agreement decision, which is a model
gap this record exposes. Reading: `unsafe-point` at `e9508f58c`.

## What this does not establish

One local kill on one devnet, not a host loss. The harness aborts on the failed restart, so the
survivors' post-recovery heads and the deploy-effect read were not taken; this record says nothing
about whether a restart that acquires an anchor would lose work. It does not show that every restart
fails — only that a restart whose stored fringe is behind the bootstrap's certified anchor cannot
boot on this build.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/release-check.py run --claim RC-4 --env local
```
