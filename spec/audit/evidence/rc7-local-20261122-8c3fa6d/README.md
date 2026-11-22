# rc7-local-20261122-8c3fa6d

**Evidence locality.** `local-devnet`

## Scope

The local arm of RC-7: upgrade compatibility. The populated-store copy/rollback/restart drill, the
refusal of a store written by a newer build, and a mixed N/N+1 fleet in which validator 3 is
replaced by the current image while the other three continue. It answers whether the current
candidate migrates, refuses, and upgrades a populated store without changing the chain.

## Provenance

| | |
|---|---|
| gate | `RC-7` |
| source | `8c3fa6d747ffe9bbfa28a21763015adbe7e3b882` |
| binary | `c971797ad5dd4dba89cf25e3c83c95ff5d5467911bf538ae3d5435f6b255bed7` |
| run | local Docker devnet; no GitHub Actions run |
| reference | the prior image `rnode:pr567-old` built from `ec104fb80` |
| packaged | `2026-11-22T16:06:12+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The claim's own `check(record)` returned `pass`: the migration drill passed, a newer store was
refused, the old and new binaries agreed on the block's state triple, and the upgraded validator
reached the common finalised head with the marker present. The observations and format traces are
under `raw/`.

## What this does not establish

A `local-devnet` pass is not public evidence, not the public fleet's rolling upgrade, and not
release readiness. It is one run on one candidate against one prior image: a finite witness.

## Reproduce

```sh
RNODE_IMAGE=rnode:pr567-current tools/devnet.sh build
docker build --build-arg GIT_HEAD_COMMIT=ec104fb8005406869748566b62fa52d9869fbaa0 \
  -f <old-worktree>/docker/rnode/Dockerfile -t rnode:pr567-old <old-worktree>
RNODE_IMAGE=rnode:pr567-current python3 tools/release-check.py run --claim RC-7 --env local \
  --old-image rnode:pr567-old
```
