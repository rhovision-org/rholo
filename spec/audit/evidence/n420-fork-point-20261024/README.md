# #420 devnet fork-point arm — 2026-10-24

The incident's own verification: four real validators, one partition, two tips at the same block
number, then a healing multi-parent merge.

## Provenance

| | |
|---|---|
| node source | `63b73d2c5c186410ab0c58bcbc5f75b54575cce8` (branch `feat/resource-fallback`) |
| image | `rnode:local` `sha256:16ea238f769d25fb6d74ecb635892bd6b8ef35632b31ec8e5c4d830bd8fe9bbf` |
| arm | `tools/n420-fork-point.py` |
| raw record | `fork-point.json` (sha256 in `manifest.json`) |
| network | local Docker bridge, disposable; no live testnet |

## Commands

```sh
tools/devnet.sh build
DEVNET_PREFIX=n420fork DEVNET_NODE_MEMORY=2g tools/devnet.sh up --validators 4 --fresh \
    --no-autopropose --epoch-length 10
DEVNET_PREFIX=n420fork python3 tools/n420-fork-point.py \
    --out spec/audit/evidence/n420-fork-point-20261024 --timeout 300
```

The arm partitions `validator-2`/`validator-3` off the bridge, deploys `@"forkA"!("A")` to the
bootstrap and `@"forkB"!("B")` to `validator-2`, proposes on each side, reconnects, and proposes until a
block justifies both tips. It then reads
`for (@x <- @"forkA"; @y <- @"forkB") { @"out"!((x, y)) }` at the merge block's post-state and at the
common base, and restarts one validator to re-read.

## Result on this run

| assertion | reading |
|---|---|
| tips | block 1: `d32306fa…` (group A) and `1c19f0a1…` (group B), distinct |
| merge | `eb0b7ed4…`, block 2, justifications contain both tips |
| (a) same post-state on every node | `3a510521…` on bootstrap and `validator-1` |
| (b) merge pre-state is neither parent's post-state | `3a510521…` ≠ `d32306fa…`'s post-state and ≠ `1c19f0a1…`'s |
| (c) both tips justified | present |
| (d) merged state carries both writers | the tuple `("A", "B")`; the common base carries neither |
| (e) restart re-derives | `validator-1` restarted: same `3a510521…`, same tuple |

## Non-claims

- **One** four-validator devnet, one partition, one merge. A finite witness, not a universal safety
  theorem, and not a certification of #420.
- The merge's own **pre-state** is the merged state of its parents, so it is expected to carry both
  writers; the negative control is the **common base**, which carries neither.
- This says nothing about the live testnet chain: the exact live 840→841 replay remains #421/#429's.
- The earlier runs of this arm (before the read control was corrected) are not evidence and are not
  committed; only this run is.
