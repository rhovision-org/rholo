# n8-finality-current-20261118

**Evidence locality.** `local-devnet`

## Scope

The A1.3 arm from `docs/src/spec/testnet-acceptance.md`: at eight equal-stake validators, one
deploy, `--no-autopropose`, `--epoch-length 10`, does the deploy's block finalise? The old verdict
was **fail** on `bbabc39e8` (2026-10-07): 112–120 blocks, all eight senders, and
`last-finalized-block` never advanced because "the supporting stake is not a supermajority — 400 of
800". This run asks whether that failure reproduces on the current tree (`4e097cc8d`, which contains
#408's N=8 finality restoration).

## Provenance

| | |
|---|---|
| gate | `TA-C1-A1.3-LOCAL` (a local component, not the live TA-C1 gate) |
| source | `4e097cc8d776a2b52cd07aacdd7a09beb9620a1b` |
| binary | `a353225ee1cb219b249f37d22fc66cccc31b5df2be8b2fcb4f0d1e3b75b3310f` |
| run | none — local Docker devnet; no Actions URL |
| reference | the old failure, `spec/audit/evidence/n149-blocks/bbabc39e8-20261007T121036Z/` |
| packaged | `2026-11-18T20:01:35+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**PASS, 3 of 3 attempts.** The deploy's block finalises on every attempt; the old permanent
freeze does not reproduce.

| attempt | blocks after deploy | senders | max parents | time to finality |
|---|---|---|---|---|
| 1 | 312 | 8 | 8 | 24 s |
| 2 | 64 | 8 | 8 | 7 s |
| 3 | 64 | 8 | 8 | 7 s |

The deploy-bearing block was included once in each attempt. All eight validators finalized to the
same height (33 in attempt 1). The old refusal string still appears at low tips (0–8), while the
finality layer gathers support; it is not the permanent 400/800 failure, because finality advances
past it. No container was OOM-killed, and no attempt was void.

Two earlier attempts from the same session were VOID at container start (Docker failed to rebind
host port 42402); they are excluded from this record and are not part of the three samples.

## What this does not establish

- This is not the live TA-C1 witness. TA-C1 is `live-host`; this record is `local-devnet`.
- The nodes ran at 2 GiB each, below the documented 8 GiB supported tier. It says nothing about
  the tier, replay headroom or a soak.
- One host, equal stakes, `--no-autopropose`, one deploy. It is not N=8 partition/rejoin, host
  loss, restart/replay, or the M4 envelope.
- Three attempts are a sample, not a rate.
- The source is the current development tree, not the selected release candidate.
- The old failure was real on `bbabc39e8`; this record says the tree changed, not that the old
  measurement was wrong.

## Reproduce

```sh
tools/devnet.sh build
NS="8" ATTEMPTS=3 CONTROL_ATTEMPTS=0 CAP=2g SETTLE_S=60 IDLE_S=30 READ_S=180 \
  DEVNET_WAIT_TRIES=180 spec/audit/evidence/n149-sweep-run.sh
```

The source pin is the branch commit above; the binary pin is the image's
`/usr/local/bin/rnode`. The run directory records the image id and the `memory.max` cgroup value
(`2147483648`) in `series.tsv`'s header.
