# jail-vote

**Evidence locality.** `local-devnet`

## Scope

The `jail-vote` arm of `tools/liveness-enforcement-drill.py` on a disposable four-validator devnet
(stakes 700/100/100/100, epoch length 3, absence threshold 1, jail after 2 boundaries, vote window 6),
with validator 3 stopped. The question: does sustained absence across advancing boundaries jail the
validator without touching its bond, open a vote window only on the jail, and does a passing vote
slash and eject it?

## Provenance

| | |
|---|---|
| gate | `C344` |
| source | `06405f21c5c2e8c15cf00aa70f34f6acaba6faaf` |
| binary | `ffaf1e068b6af5a284f793fda680a12854be84913ae7b5412e705da84b557e5e` |
| run | local devnet; no Actions URL |
| packaged | `2026-11-17T12:41:52+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

All ten checks passed on one binary: the validator was jailed, its bond was unchanged by the jail,
it left the active draw, the vote window opened with remaining time, the vote deploy was committed
and not rejected, its ballot was consumed or carried in its own block, the bond was removed by the
passing vote, the liveness record was cleared, and the vote window closed. This is the reachable
jail-and-vote surface with a block-authored absence signal.

## What this does not establish

It is one arm on one local devnet, not public-testnet behaviour. It says nothing about release or
renewal after a jail (those are the `restart-expiry` arm, which has no clean run), the cold rejoin,
or the exact-threshold boundary beyond the separate arm. `execution_proven` is false.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/liveness-enforcement-drill.py --arm jail-vote --out target/c344-capstone/jail-vote
```
