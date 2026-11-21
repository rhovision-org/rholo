# rc4-local-20261121-20501f4

**Evidence locality.** `local-devnet`

## Scope

RC-4, recovery without loss: the same four-validator kill-and-restart drill as
`rc4-local-20261121-e9508f5`, re-run at source
`20501f4820df02f8d58d33d6cb9dba19769cae6c` after the boot trace learned to record the **agreement
decision** and the boot learned to ask the source for a certificate at an anchor from its own
accepted fringe. The question is whether the victim, restarted behind its peers, can attribute its
accepted state and run.

## Provenance

| | |
|---|---|
| gate | `RC-4` |
| source | `20501f4820df02f8d58d33d6cb9dba19769cae6c` |
| binary | `b6ada1e3278f17d640400a762b42913ab93a65a1208095f0ff7d0ac2f68ac7cf` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-21T10:48:53+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**FAIL, now as `unsafe-point` rather than `model-incomplete`.** The victim restarted (container
start time changed) and `/health` never answered. The decision trace carries the agreement the
evaluator needs:

```json
{"stage": "decision", "accepted": false, "fringe_agreement": "mismatch",
 "fringe_agrees": false, "anchor_closure_present": false,
 "accepted_fringe": 1, "certified_fringe": 4,
 "refusal_coordinate": "fringe-mismatch",
 "served_supporting_stake": 300, "served_total_stake": 400}
```

The certificate the victim fetched was quorate (300/400) but its anchor is **ahead** of the victim's
recorded fringe; the boot agreement accepts only a fringe that descends from the anchor. The boot
also asked the source for a certificate at the victim's own recorded anchor first; the peer holds
certificates at anchors 190 and 330 and neither anchors this fence, so the head was used.

**The root cause the run establishes:** before the kill only the bootstrap held a certificate
(anchor 190); validators 1–3 held none, because the collector mints only when its own latest-fringe
statement reaches quorum, and under continuous production the live nodes' latest-fringe digests
differ (the victim's own collector logs `0/400`, then `200/400`). A validator therefore restarts
with no stored certificate, and the only certificate its peer can serve is ahead of the fence it
held.

The next unit decides the liveness rule: restore the lagging node from the network's certified
anchor through the checkpoint-sync path a fresh joiner uses, or make the certificate statement
common to live validators so each stores one anchored to its own history. This record closes the
model gap; it does not claim the restart works.

## What this does not establish

One local kill on one devnet, not a host loss. The harness aborts on the failed restart, so the
survivors' post-recovery heads and the deploy-effect read were not taken. The anchor-named fetch is
exercised (it found no certificate for this fence) but this record does not show that a fence whose
block *is* an anchor boots; that is only unit-tested.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/release-check.py run --claim RC-4 --env local
```
