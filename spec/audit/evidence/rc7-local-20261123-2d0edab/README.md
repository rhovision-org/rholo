# rc7-local-20261123-2d0edab

**Evidence locality.** `local-devnet`

## Scope

The local arm of RC-7, upgrade compatibility: the populated-store copy/rollback/restart drill, the
refusal of a store written by a newer build, and a mixed fleet in which validator 3 runs the prior
image `rnode:pr567-old`, finalises the first epoch boundary, installs a persistent ρ contract, and
keeps its store while the candidate takes over. The candidate re-reads the pre-swap block's state
triple and marker, invokes the installed contract, and reaches the common finalised head. This is
the stronger prior-path test from the closed #654 branch; its DE-18 episode is a by-product, not a
HAZOP placement.

## Provenance

| | |
|---|---|
| gate | `RC-7` |
| source | `2d0edabc792b839ac42cc03d3a6e47c2a90473e6` |
| binary | `fa4f3d8e4332532fe029f377ad8ce93a07bfc0ae4b7b3b2c5f4689017405e7ba` |
| run | local Docker devnet; no GitHub Actions run |
| reference | the prior image `rnode:pr567-old` built from `ec104fb80` |
| packaged | `2026-11-23T18:22:10+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The claim's own `check(record)` returned `pass` on this record: the migration drill passed, a newer
store was refused, the two binaries agreed on the block's state triple, the prior path resolved
after the swap, and the upgraded validator reached the common finalised head. The observations and
format traces are under `raw/`. This record replaces `rc7-local-20261122-8c3fa6d`, whose source
predates the liveness/native-state changes and whose test only deployed a marker.

## What this does not establish

A `local-devnet` pass is not public evidence, not the public fleet's rolling upgrade, and not
release readiness. Both images write store format 4, so this run does not cross a format boundary;
the migration half is pinned by the Rust fixtures named in the record's episode, not by this run.
It is one run on one candidate: a finite witness.

## Reproduce

```sh
RNODE_IMAGE=rnode:campaign-20261123 tools/devnet.sh build
python3 tools/release-check.py run --claim RC-7 --env local --old-image rnode:pr567-old
```
