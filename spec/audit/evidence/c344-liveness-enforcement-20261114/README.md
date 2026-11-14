# c344-liveness-enforcement-20261114

**Evidence locality.** `local-devnet`

## Scope

The `jail-vote` arm of `tools/liveness-enforcement-drill.py`: arm RCHIP #55 / Law 89 on a
four-validator devnet (epoch length 3, absence threshold 1, jail after 2 boundaries, 10-boundary jail,
6-boundary vote window, severity `misdemeanour`; stakes 700/100/100/100), stop validator 3, and wait
for `getLiveness` to report a jail. **The arm failed: no jail was ever imposed while the chain kept
advancing without the absent validator.**

## Provenance

| | |
|---|---|
| gate | `C344` |
| source | `e257f00dcd6f7970019e21edffd78e3cca75b2ad` (the `rnode:local` image's build source) |
| binary | `6f60e106ddb4db75929d22aaac256224425115c85dc509ba2b055341a8811694` |
| run | local devnet; no Actions URL |
| packaged | `2026-11-14T14:52:00+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

Validator 3 was stopped at about height 20. Over the next 420 seconds the surviving three validators
advanced the chain to **`maxObservedHeight: 305`, `finalizedHeight: 302`** (`/health`, recorded in
`raw/result.json`), so finality was not stalled. `getLiveness` returned an **empty map** throughout
(`liveness_at_timeout: {}`), the target's bond stayed at 100 (the jail would not touch it), and no
vote was submitted because the vote window never opened. The drill recorded `jailed: false` and
exited non-zero; the result is a `FAIL`, not an `INCONCLUSIVE`.

**Instrumented follow-up (diagnosis, not part of the retained binary).** A temporary build printed the
prelude's inputs at each boundary. It showed `jail-after=2`, `participation` correctly naming three of
four validators, and **`liveness_anchor()` returning `None` at every boundary after the first armed
one** — even though the prelude sets the anchor at the first armed boundary. The anchor leaf
(`pos:liveness-anchor`) is therefore not visible in the next boundary block's pre-merged state on this
path; the absence counter never advances past the first comparison point, so `jail_after` is never
reached. Whether the block merge drops the close chain's slot or the write never reaches the sidecar is
not established by this record.

The unit tests (`rholang/src/native_state.rs`, `liveness_tests`) use one in-memory store across
boundaries and do not exercise the block merge, which is why the mechanism passes them.

## What this does not establish

This is a `local-devnet` witness that the **jail arm does not work on a live path**, not a diagnosis
of the merge defect (the instrumented build is not retained). It does not establish the live testnet
behaviour, the vote-slash/partition/restart-expiry/exact-threshold arms, or any consensus rule about
when a boundary must carry `CloseBlock`. It does not show that a bond can or cannot be slashed; the
vote was never reached. The finding keeps C344 `in progress`; it cannot be closed before the anchor
persistence is repaired and the live arms run.

## Reproduce

```sh
tools/devnet.sh build
DEVNET_PREFIX=c344c DEVNET_NETWORK=c344c \
DEVNET_GRPC_BASE=54402 DEVNET_HTTP_BASE=54403 DEVNET_ADMIN_BASE=54405 \
python3 tools/liveness-enforcement-drill.py --arm jail-vote \
  --out target/c344-liveness-enforcement --timeout 420
```

`tools/devnet.sh` is disposable: `DEVNET_PREFIX=c344c DEVNET_NETWORK=c344c tools/devnet.sh down -v`
removes the network, containers and volumes.
