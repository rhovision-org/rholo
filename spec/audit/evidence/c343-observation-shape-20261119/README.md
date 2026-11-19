# c343-observation-shape-20261119

**Evidence locality.** `local-devnet`

## Scope

The C343 collector now writes the per-host `{health, operator, block6}` document that
`gate-principles.py::phase_diagnosis` consumes, and carries the run's declared locality. This
rehearsal asks two questions: does one `c343-live.py collect` on a local devnet produce that shape
from the real `/health` and admin `/api/v1/status/operator` routes, and does the criterion refuse
the resulting local-devnet document for locality? It is **not** the live B/C/D snapshot.

## Provenance

| | |
|---|---|
| gate | `C343` |
| source | `46c98b67b3dae3c22104ab737ad7bf4851862b0b` (the binary's build commit) |
| binary | `d98c9a133dd4be551ea6d04ad9e20f16b6a2709eb63da1f37b5273260eac09d5` |
| tooling | the parent of this evidence commit (the collector bundle writer and locality parameter) |
| run | none — local Docker devnet; no Actions URL |
| reference | `tools/c343-live.py`; `tools/gate-principles.py::phase_diagnosis`; `spec/audit/phase-diagnosis.tsv` |
| packaged | `2026-11-19T09:46:17+00:00` |
| verdict | **INCONCLUSIVE** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict
is sound.

## Result

One validator, admin listener published, plan locality `local-devnet`. `collect` answered both
routes:

```text
c343-live: collected 1 host(s) into target/c343/run; all answered
```

and wrote `raw/host-bootstrap.observation.json` with `gate_id: "C343"`, `locality: "local-devnet"`,
the parsed 14-field health document, the parsed operator document and the journal text as `block6`.
`raw/observations.hosts.json` indexes the per-host documents.

- `python3 tools/c343-live.py verify --run-dir <run> --purpose current-build` → exit 3,
  `PRECONDITION (bootstrap)`: the fresh devnet's 87-line journal carries no block-6 disposition,
  which is the honest reading rather than a PASS.
- `python3 tools/gate-check.py --gate C343 --observations <record>/raw/host-bootstrap.observation.json`
  → exit 1, `FAIL`: *"the observation declares 'local-devnet'; the criterion requires 'live-host'"*.

The second is the point of the locality field: the shape is complete enough to evaluate, and the
criterion does not accept a local rehearsal as the live-host positive.

## What this does not establish

- This is **not** the C343 live B/C/D snapshot. One container, one host, a disposable chain.
- The journal is a fresh devnet's short startup log; it has no block-6 disposition line. A
  `PRECONDITION` here says the local run does not exercise block 6, not that a live host would fail.
- The record binds bytes and the collector's plan; it does not prove execution.
  `execution_proven` is false.
- No gate is promoted; `C343` remains `precondition` and the live operator snapshot is still owed.
  No public deployment, real key, transfer or `.github/workflows/*` edit.

## Reproduce

From the repository root at the tooling commit, with `rnode:local` built at the source pin above:

```sh
DEVNET_PREFIX=c343 DEVNET_NETWORK=c343net \
DEVNET_GRPC_BASE=57402 DEVNET_HTTP_BASE=57403 DEVNET_ADMIN_BASE=57405 \
DEVNET_NODE_MEMORY=2g tools/devnet.sh up --validators 1 --fresh --admin
docker logs c343-bootstrap > target/c343/bootstrap.log
python3 tools/c343-live.py collect --plan target/c343/plan.json --out target/c343/run \
  --locality local-devnet
python3 tools/c343-live.py verify --run-dir target/c343/run --purpose current-build
python3 tools/gate-check.py --gate C343 \
  --observations target/c343/run/raw/host-bootstrap.observation.json
tools/devnet.sh down -v
```

The plan is `target/c343/plan.json` in the run: alias `bootstrap`, public base
`http://127.0.0.1:57403`, operator base `http://127.0.0.1:57405`, and the captured journal.
