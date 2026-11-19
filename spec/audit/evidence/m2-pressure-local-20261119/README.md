# m2-pressure-local-20261119

**Evidence locality.** `local-devnet`

## Scope

The #428 M2 pressure record after the criterion was made pressure-aware: does the capped HTTP
listener refuse a concurrent burst with 429 and `retry-after: 1` and no 5xx while the unset default
admits the same burst, do the gRPC ingress and pool bounds answer `RESOURCE_EXHAUSTED`, and does
the criterion refuse the resulting local record for locality? This is one Docker host. It is
**not** the public-live M2 positive and not gate evidence.

## Provenance

| | |
|---|---|
| gate | `G428-M2` |
| source | `46c98b67b3dae3c22104ab737ad7bf4851862b0b` (the binary's build commit) |
| binary | `d98c9a133dd4be551ea6d04ad9e20f16b6a2709eb63da1f37b5273260eac09d5` |
| tooling | the parent of this evidence commit (the pressure criterion and arm) |
| run | none — local Docker devnet; no Actions URL |
| reference | `spec/audit/gate-criteria.json` G428-M2; `tools/m2-pressure.py`; `tools/m2-pressure-arm.py` |
| packaged | `2026-11-19T09:42:46+00:00` |
| verdict | **INCONCLUSIVE** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict
is sound.

## Result

Two devnet lifetimes, one with `RNODE_HTTP_MAX_INFLIGHT_DEPLOY=2` and one unset. The capped run
held both permits with partial-body POSTs and fired four complete invalid-JSON probes:

```text
capped: held=2 probe=4 admitted=0 refused=4 retry_after=4 server_errors=0
        after-release request reaches the extractor (400)
unset:  held=0 probe=6 admitted=6 refused=0 server_errors=0
```

The raw-socket view of the same listener carries `HTTP/1.1 429` with `retry-after: 1` while the
permits are held and `HTTP/1.1 400` after release; without the cap it carries `HTTP/1.1 400`.
The gRPC observation (through `PeerAddressIo` and `DeployIngressGuard` over a real tonic listener)
records one admitted and one `RESOURCE_EXHAUSTED` call under a one-request allowance, the pool
bound's `RESOURCE_EXHAUSTED` with `retry-after: 1`, and a permissive control with two admissions
and no refusal.

- `python3 tools/m2-pressure.py check --record <this record> --locality local-devnet` → exit 0.
- `python3 tools/gate-check.py --gate G428-M2 --observations <this record>/observations.json` →
  exit 1, `FAIL`: *"the record declares 'local-devnet'; the criterion requires 'public-live'"* and
  *"endpoint host '127.0.0.1' is local, not public-live"*.

The two results are the point: the record is internally bound and passes its own local policy, and
the gate criterion does not accept a local run as the public-live positive.

## What this does not establish

- This is **not** the M2 public-live positive: one Docker host, one bridge, a loopback listener.
  The criterion requires a `public-live` label and a non-local endpoint.
- The record binds captured bytes and container creation configuration; it does not prove
  execution. `execution_proven` is false and a coordinated operator can still lie.
- The gRPC ingress-rate refusal has no `retry-after` metadata; it is recorded as its own arm, and
  the criterion does not treat it as the pool bound's retry signal.
- The node has one HTTP listener; `http` is the client-library transcript and `direct_http` is the
  raw-socket view of the same listener, not two network paths.
- No gate is promoted; `G428-M2` remains `owes` (`depends_on: G584-G1`). No public deployment, real
  key, transfer or `.github/workflows/*` edit.

## Reproduce

From the repository root at the tooling commit, with `rnode:local` built at the source pin above:

```sh
DEVNET_PREFIX=m2cap DEVNET_NETWORK=m2net \
DEVNET_GRPC_BASE=56402 DEVNET_HTTP_BASE=56403 DEVNET_ADMIN_BASE=56405 \
DEVNET_NODE_MEMORY=2g DEVNET_NODE_ENV=RNODE_HTTP_MAX_INFLIGHT_DEPLOY=2 \
  tools/devnet.sh up --validators 1 --fresh
python3 tools/m2-pressure-arm.py phase --mode capped --out target/m2/capped \
  --container m2cap-bootstrap --port 56403 --cap 2 --probe 4
tools/devnet.sh down -v

DEVNET_PREFIX=m2cap DEVNET_NETWORK=m2net \
DEVNET_GRPC_BASE=56402 DEVNET_HTTP_BASE=56403 DEVNET_ADMIN_BASE=56405 \
DEVNET_NODE_MEMORY=2g tools/devnet.sh up --validators 1 --fresh
python3 tools/m2-pressure-arm.py phase --mode unset --out target/m2/unset \
  --container m2cap-bootstrap --port 56403 --probe 6
tools/devnet.sh down -v

python3 tools/m2-pressure-arm.py grpc --out target/m2/grpc
python3 tools/m2-pressure-arm.py assemble --capped target/m2/capped --unset target/m2/unset \
  --grpc target/m2/grpc --out target/m2/run --locality local-devnet --port 56403
python3 tools/m2-pressure.py check --record <this record> --locality local-devnet
python3 tools/gate-check.py --gate G428-M2 --observations <this record>/observations.json
```
