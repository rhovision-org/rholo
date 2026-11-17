# exact-threshold

**Evidence locality.** `local-devnet`

## Scope

The `exact-threshold` arm of `tools/liveness-enforcement-drill.py` on a disposable four-validator
devnet (stakes 600/200/100/100, epoch length 3, absence threshold 1, jail after 2 boundaries, vote
window 20), with validator 3 stopped and a second voter funded. The question: is the vote gate
strictly `3·for > 2·total` — does a first ballot at exactly two thirds leave the bond untouched, and
does the second ballot cross the threshold and slash?

## Provenance

| | |
|---|---|
| gate | `C344` |
| source | `06405f21c5c2e8c15cf00aa70f34f6acaba6faaf` |
| binary | `ffaf1e068b6af5a284f793fda680a12854be84913ae7b5412e705da84b557e5e` |
| run | local devnet; no Actions URL |
| packaged | `2026-11-17T12:43:13+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

All eighteen checks passed: the active denominator was 900, the first ballot was exactly two thirds,
the target's bond was unchanged at exactly two thirds and the window stayed open, the second ballot
was counted in its block, its weight was a strict supermajority, and the second vote slashed the
bond and closed the window. The strictness boundary is therefore observed on both sides.

## What this does not establish

It is one arm on one local devnet. It does not establish the live release/renewal path or the cold
rejoin, and `execution_proven` is false.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/liveness-enforcement-drill.py --arm exact-threshold --out target/c344-capstone/exact-threshold
```
