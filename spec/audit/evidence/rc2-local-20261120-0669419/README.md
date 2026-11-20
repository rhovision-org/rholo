# rc2-local-20261120-0669419

**Evidence locality.** `local-devnet`

## Scope

RC-2, safety: after a producing validator is partitioned and rejoined, is every finalised block one
validator holds finalised by every validator at or above its height, with no finalised regression
and no conflicting finalised histories?

## Provenance

| | |
|---|---|
| gate | `RC-2` |
| source | `066941917e1b066b2a704141d1904be8329e1f7a` |
| binary | `d664fc030f1d4bc3d35ddd3414ea39b3aeff8638b14453f2e07460be0e16c0b8` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-20T11:58:00+00:00` |
| verdict | **INCONCLUSIVE** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**NOT RUN.** The partition harness could not install a partition: its first phase is the shared
acceptance drill, and that drill failed at `marker present in finalized state on every validator` /
`validator 0 lost or changed the marker in finalized post-state` — the RC-1 retention defect recorded
in `spec/audit/evidence/rc1-local-20261120-0669419`. The safety observation was therefore never
taken. The harness log is `raw/partition.log`; the full poll stream is
`raw/observations.jsonl.gz` (compressed at rest).

## What this does not establish

Nothing about containment: the run stopped before a partition existed. A future green partition is
still a finite local witness, not a safety proof; equivocation, host loss and the public fleet are
out of scope.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-2 --env local
```

This boots `DEVNET_PREFIX=acceptance DEVNET_NETWORK=acceptance` with autopropose on, waits for the
chain to advance, then runs `tests/consensus-partition-rejoin.py --local --producing-victim`. Once
RC-1 is fixed, the same command produces the containment matrix.
