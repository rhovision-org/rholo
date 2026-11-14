# c339-mature-replay-20261114

**Evidence locality.** `local-devnet`

## Scope

C339's missing measurement: a wiped validator cold-restoring a **2,432-height** chain through
`RecoveryReplay`. The chain was built to 2,436+ observed heights by a four-validator devnet, validator 3
was stopped, its volume discarded, and it was restarted empty at 15:53:25 UTC against the bootstrap.
The harness (`tools/c339_mature_replay.py`) allowed a 45-minute restore budget.

## Provenance

| | |
|---|---|
| gate | `C339` |
| source | `6964e875bb878c7d9d22b4360815de2d8ab19674` (the `rnode:local` image's build source) |
| binary | `cf186f4d6358a1b007963895187c2ab1634cc2660ff6f91d19f561acaa93980b` |
| run | local devnet; no Actions URL |
| packaged | `2026-11-14T16:55:00+00:00` |
| verdict | **INCONCLUSIVE** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The restore **did not complete inside the budget**, and it did not fail either. At the 45-minute
expiry the joiner was still in `NodeSyncing` (`height: 0`, `finalizedHeight: null`, `ready: false`,
`readyReasons: ["shard_not_running"]`). It kept replaying after the budget expired: by 16:52 UTC it had
replayed to `close_block 461` at roughly **3 blocks/min**, with its store at 31.5 MB against the
bootstrap's 47.4 MB, and the bootstrap at height 2,582 / finalized 2,550.

Two facts make this a cost measurement under contention rather than a clean restore:

- the host was shared with another session's `cargo`/`rustc` build and the `darkwow` devnet, with load
  average ~29 on 24 CPUs;
- the joiner logged **eight** `No tuple space state responses for 30s. Resending requests` stalls,
  i.e. the bootstrap did not answer the tuple-space walk for repeated 30-second windows.

The restart and wrong-genesis-pin controls were not reached.

## What this does not establish

This is **not** a PASS: the exact-source joiner did not reach the anchor, so C339 stays `in progress`.
It is also not a FAIL of the code: the incomplete restore is confounded by host contention and by the
bootstrap's unanswered LFS windows, and the observed rate is a lower bound rather than a
mature-chain replay cost. The `n479-native-join-20261029` record remains the completed control
(756-record chain, 133 s replay, restart and wrong-pin controls); this record adds only the attempted
2,432-height scale measurement. No target-finality proof or legacy-store certification is claimed.

## Reproduce

On an otherwise idle host:

```sh
tools/devnet.sh build
python3 tools/c339_mature_replay.py --out target/c339-mature-replay \
  --target-height 2436 --restore-timeout 14400
```

The tool wipes validator 3 at `--validator` (default 3), waits for the cold restore, then runs the
restart and wrong-pin controls. `DEVNET_PREFIX` and the three base ports must be free; the default
campaign uses the `devnet` prefix.
