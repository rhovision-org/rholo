# #420 restart/replay arm — 2026-10-24

A validator on the **1 GiB** devnet (below the advised minimum) is SIGKILLed at height ~1,197 and
restarted with the operator's explicit `--allow-under-minimum`. The restart must replay from durable
state, come back serving, survive, and keep the accepted chain.

## Provenance

| | |
|---|---|
| node source | `7878df9e2` (branch `feat/resource-fallback`) |
| image | `rnode:local` `sha256:16ea238f769d25fb6d74ecb635892bd6b8ef35632b31ec8e5c4d830bd8fe9bbf` |
| arm | `tools/resource-restart-arm.py --container n420soak-validator-1` |
| raw record | `restart.json` (sha256 in `manifest.json`) |
| network | the local 1 GiB devnet from `n420-soak-20261024`; no live testnet |

## Commands

```sh
DEVNET_PREFIX=n420soak DEVNET_NODE_MEMORY=1g DEVNET_EXTRA_FLAGS=--allow-under-minimum \
    tools/devnet.sh up --validators 4 --fresh
# ... the soak arm runs ...
DEVNET_PREFIX=n420soak python3 tools/resource-restart-arm.py \
    --container n420soak-validator-1 --out spec/audit/evidence/n420-restart-20261024
```

## Result

| reading | value |
|---|---|
| anchor | block `c7fa52d0…` at height 1,196 (read from the bootstrap; the target was `critical`) |
| restart | SIGKILL + start of `n420soak-validator-1`, same flags (including the override) |
| replay | **11.6 s** to height 1,197 |
| state after replay | `normal` — the fresh process's headroom recovered, and the fallback state is per-process |
| OOM-killed on restart | no |
| anchor block present after replay | yes |

## Non-claims

- One node, one SIGKILL, one run; not a universal recovery theorem.
- The host is below the advised minimum and the override is the operator's explicit acceptance; the
  preflight's *refusal* without it is the A2 arming instrument's subject.
- The replay window is this chain's (~1,200 blocks), not a measured curve at target height.
