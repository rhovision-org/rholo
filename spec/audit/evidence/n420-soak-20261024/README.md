# #420 1 GiB pressure arm — 2026-10-24

A four-validator devnet, each node capped at **1 GiB** (below the advised validator minimum), under a
10-minute deploy burst. This is **fault injection**, not a supported-tier claim.

## Provenance

| | |
|---|---|
| node source | `7878df9e2` (branch `feat/resource-fallback`) |
| image | `rnode:local` `sha256:16ea238f769d25fb6d74ecb635892bd6b8ef35632b31ec8e5c4d830bd8fe9bbf` |
| arm | `tools/n420-soak.py --seconds 600` |
| raw record | `soak.json` (sha256 in `manifest.json`) |
| network | local Docker bridge, disposable; no live testnet |

## Commands

```sh
DEVNET_PREFIX=n420soak DEVNET_NODE_MEMORY=1g DEVNET_EXTRA_FLAGS=--allow-under-minimum \
    tools/devnet.sh up --validators 4 --fresh
DEVNET_PREFIX=n420soak python3 tools/n420-soak.py \
    --seconds 600 --out spec/audit/evidence/n420-soak-20261024
```

`--allow-under-minimum` is the start-up preflight's explicit override of the 2 GiB floor this host
is below at its height; the refusal without it is the A2 arming instrument's subject.

## Result

| reading | value |
|---|---|
| fallback states visited | `normal`, **`throttled`** |
| deploys attempted / refused | 102 / 6 (refusals recorded, not hidden) |
| OOM-killed containers | none |
| container restarts | none |
| `/api/status` readable at the end | all four |
| autopropose halted | no |
| finality | 837 → 1166 on every validator |

## Non-claims

- One run on one host; a finite witness, not a universal safety theorem.
- 1 GiB is below the advised validator minimum: a pass says the node shed work and stayed observable
  under the barriers, **not** that 1 GiB is a supported validator.
- The run reached `throttled` but not `critical`; the state machine's transitions and the
  propose/attest gates are pinned by unit tests, not claimed from this run.
- No live network was touched.
