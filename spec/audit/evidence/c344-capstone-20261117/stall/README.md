# stall

**Evidence locality.** `local-devnet`

## Scope

The `stall` arm of `tools/liveness-enforcement-drill.py` on a disposable four-validator devnet
(stakes 400/300/200/100, epoch length 3, absence threshold 1, jail after 2 boundaries, vote window
6), with validators 1, 2 and 3 stopped so the quorum cannot advance. The question: below quorum, do
finality and the liveness counters freeze without jailing anyone?

## Provenance

| | |
|---|---|
| gate | `C344` |
| source | `06405f21c5c2e8c15cf00aa70f34f6acaba6faaf` |
| binary | `ffaf1e068b6af5a284f793fda680a12854be84913ae7b5412e705da84b557e5e` |
| run | local devnet; no Actions URL |
| packaged | `2026-11-17T12:44:27+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

All four checks passed over a 30-second window: the finalized height and anchor were unchanged, the
liveness counters were unchanged, and no validator was jailed. A quorum stall is therefore not read
as absence.

## What this does not establish

The arm freezes finality by stopping the voters; it does not exercise the cold rejoin or the
release/renewal path, and one 30-second window is a finite witness. `execution_proven` is false.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/liveness-enforcement-drill.py --arm stall --out target/c344-capstone/stall
```
