# n487-live-window-rehearsal-20261118

**Evidence locality.** `local-devnet`

## Scope

The #584 live-window protocol after the collector/live-gate corrections: does a preflight produced
by `release-pin-probe.py merge` carry and require the selected-candidate pins and the typed
deploy-distribution plan, does one signed deploy reach all four validator pools, and does the
1-second sampler observe `pooled` → `included` → `finalizedSuccess` on **every** alias? The run is a
local four-validator Docker devnet; it is a protocol rehearsal, not the live #584 window.

## Provenance

| | |
|---|---|
| gate | `n487-live-window-rehearsal` (a rehearsal component, not G584-G1) |
| source | `4e097cc8d776a2b52cd07aacdd7a09beb9620a1b` (the Rust binary's build commit) |
| binary | `a353225ee1cb219b249f37d22fc66cccc31b5df2be8b2fcb4f0d1e3b75b3310f` |
| tooling | the branch commit under test (the parent of this evidence commit; the only Rust change between it and the binary's source is the `#[cfg(test)]` M2 pressure test, which is not in the release binary) |
| run | none — local Docker devnet; no Actions URL |
| reference | `spec/audit/runbooks/n487-live-postdeployment-20261108.md` and `spec/audit/remaining-gates-path-20261118.md` |
| packaged | `2026-11-18T21:23:39+00:00` |
| verdict | **INCONCLUSIVE** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The worksheet `python3 tools/live-gate.py check` produced:

```
G1: PASS   G2: PASS   G3: PASS   G4: INCOMPLETE   G5: PASS   G6: PENDING
exit 3; overall NOT_A_VERDICT; execution_proven false
```

- **G1 PASS** is the field/agreement contract: the preflight was produced by
  `release-pin-probe.py merge --check` with four records, zero unset observed fields, zero unset
  operator fields, and `checks.ok: true`. The collector carried the three selected pins, cross-checked
  every record against them, and accepted the typed `deploy_relay` plan.
- **G2 PASS** is per-alias: all four aliases observed `pooled`, `includedUnfinalized`/Processed and
  `finalizedSuccess` at the 1-second cadence. One signed deploy was submitted to the bootstrap; the
  deploy relay offered it to the three peers, and the four `/api/v1/deploys` captures are
  byte-identical (`raw/pooled-v1.json` … `raw/pooled-v4.json`). The bootstrap's relay counters read
  `offers=3, failures=0, capacity_refusals=0` (`raw/relay-metrics.txt`).
- **G3 PASS**: the pre-finality settlement samples read not-finalized; the settlement
  `finalizedSuccess`/`settlementConfirmed=true` samples agree with the finality observation, and the
  inclusion block `bbee0145d03c411c8a1cdde0f618604a62222bb99466a222425ef056ae2b4a34` (blockNumber 1)
  finalized on all four.
- **G4 INCOMPLETE** by design: no recovery arm was chosen, and A4 alone is not G4.
- **G5 PASS**: four aliases, per-sample body digests, a covering `SHA256SUMS`, and no raw endpoint
  leak in `run.json`/`plan.public.json`.
- **G6 PENDING**: no independent reviewer verdict was supplied; the tool never records one.

The sampler recorded 1050/1050, 1048/1050, 1048/1050 and 1048/1050 successful responses on the four
aliases over 180 ticks (the few misses are connection errors at the start or end of the run, recorded
as events).

## What this does not establish

- This is **not** the live #584 window and not gate evidence. One host, one Docker bridge, one
  signer; the source is the binary's build commit, not a selected release candidate.
- The four validator records carry **synthetic** boot IDs (`boot_id_source: rehearsal`,
  `synthetic_boot_ids: true`). G1's PASS is the field/agreement contract; it does not establish four
  distinct hosts. A live preflight with synthetic boot IDs is a hard NO-GO in the runbook.
- The operator, observer and independent-reviewer names are rehearsal labels. The published
  preflight is the redacted copy; redaction collapses the operator and reviewer to the same
  `<redacted>` value, so G1 was checked against the private manifest and is not re-runnable from the
  published copy.
- G4 is not exercised and G6 has no reviewer verdict. The live recovery arm and the independent
  verdict remain maintainer inputs.
- The relay is async best-effort. A peer can miss `pooled` before inclusion; the runbook forbids a
  second deploy to manufacture the stage, so that would be INCOMPLETE, not a retry.
- One signed deploy, relayed as a copy, is not four deploys and does not exercise the
  `four-ingress` alternative.
- No public deployment, real key, transfer, disruptive arm, or claim about the supported resource
  tier. `execution_proven` is false.

## Reproduce

From the repository root at the tooling commit, with the `rnode:local` image built at the binary
source above:

```sh
export DEVNET_PREFIX=n487r DEVNET_NETWORK=n487rnet
export DEVNET_GRPC_BASE=46402 DEVNET_HTTP_BASE=46403 DEVNET_ADMIN_BASE=46405
export DEVNET_NODE_MEMORY=2g DEVNET_NODE_ENV=RNODE_DEPLOY_RELAY=1
tools/devnet.sh up --validators 4 --fresh --no-autopropose --no-propose-on-deploy

# One signed deploy; it stays pooled because propose-on-deploy is off.
tools/devnet.sh deploy hello.rho
curl -sS http://localhost:46403/api/v1/deploys   # deploy id, repeated on 47403/48403/49403

# Sample at 1 s, then force one block on each validator.
python3 tools/live-gate.py sample --plan target/n487-rehearsal/plan.json \
  --out target/n487-rehearsal/run --interval 1 --duration 180
tools/devnet.sh cli n487r-bootstrap propose
tools/devnet.sh cli n487r-validator-1 propose
tools/devnet.sh cli n487r-validator-2 propose
tools/devnet.sh cli n487r-validator-3 propose

# Author submission.json / claims.json, regenerate the covering SHA256SUMS, then:
python3 tools/live-gate.py check --run-dir target/n487-rehearsal/run \
  --preflight target/n487-rehearsal/preflight/manifest.json \
  --report target/n487-rehearsal/run/gate-report.json
python3 tools/evidence-pack.py verify --record spec/audit/evidence/n487-live-window-rehearsal-20261118 --require-pins
```

The preflight is built through the collector (`release-pin-probe.py observe` per alias, then
`merge --check`); the records' synthetic boot IDs are disclosed in the manifest.
