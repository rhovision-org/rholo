# rc7-local-20261120-0669419

**Evidence locality.** `local-devnet`

## Scope

RC-7, upgrade compatibility: does a populated store migrate, does a build refuse a store written by
a newer format, and does a mixed-version fleet either keep one finalised chain/state or refuse on
the consensus path?

## Provenance

| | |
|---|---|
| gate | `RC-7` |
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

**NOT RUN**, with two arms recorded:

- the G520 populated-store copy/rollback/restart drill returned **PASS**
  (`raw/migration/raw/result.json`);
- a scratch store stamped `store-format = 5` was **refused** by this build (which writes 4) with
  `the data directory /data was written by a newer build (store format 5, this build writes 4)`
  (`raw/downgrade.log`).

The mixed-version arm did not run: no separately built prior image was supplied. The test accepts
`--old-image` / `RNODE_OLD_IMAGE`; retagging `rnode:local` is not a mixed fleet and is refused by
the test's distinct-binary check.

## What this does not establish

The mixed-fleet half (same state triples or a named consensus refusal, then catch-up after the swap)
was not observed. The migration drill is one local copy/rollback/restart, not a live fleet upgrade.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-7 --env local --old-image rnode:old
```

With an old image built from a prior commit, the test boots `DEVNET_PREFIX=rc7mix
DEVNET_NETWORK=rc7mixnet` with `RNODE_IMAGE_rc7mix_validator_3` set to it, compares the same block's
`preStateHash`/`postStateHash`/`rejectedDeploys` on the two binaries, swaps the old node, and
requires the same finalised head.
