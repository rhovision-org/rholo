# rc1-local-20261120-2a1bc85

**Evidence locality.** `local-devnet`

## Scope

The RC-1 baseline local arm: a four-validator local devnet, one marker deploy, settlement read
through `/api/v1/deploy-settlement`, and the marker read at every validator's finalised head after
two further epoch boundaries. The question is whether a reported `finalizedSuccess` is a retained
finalised write, and whether a later block's `rejectedDeploys` ever names the deploy.

## Provenance

| | |
|---|---|
| gate | `RC-1` |
| source | `2a1bc85bb3d3b4a49f6599ebaacbbfbb5d6a235b` |
| binary | `bc260d70d03da98dfcf0694a23b143731b6a36c0a7d8ac6fbf2152bc7ccd4db7` |
| run | local harness; no upstream actions run |
| reference | none |
| packaged | `2026-11-20T19:18:23+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The acceptance drill passed. All four validators reported `finalizedSuccess` with
`settlementConfirmed: true` for the deploy id, agreeing on inclusion block
`0bd6d77f…` at height 26. The marker was present at every finalised head (heights 28 and 29 across
the validators), and `is_finalized` was true for the inclusion block on every validator. No
validator's block range named the deploy id in `rejectedDeploys`. The record carries 1,032 merge
traces; `base_ok`, `fork_ok`, `ledger_ok` and `scope_partition_ok` hold on them.

## What this does not establish

A local mechanism check, not the public claim: the four-host public arm is separate. This is a
finite witness for one deploy and one four-validator devnet, not a universal safety theorem. It does
not establish behaviour above the ≤1/3 Byzantine bound, a bond/leave transition, or a lost store.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-1 --arm local
```

with `rnode:local` built at `2a1bc85bb3d3b4a49f6599ebaacbbfbb5d6a235b` (binary
`bc260d70…`). The arm boots its own four-validator devnet and tears it down.
