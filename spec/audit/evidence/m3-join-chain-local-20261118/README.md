# m3-join-chain-local-20261118

**Evidence locality.** `local-devnet`

## Scope

The #428 M3 six-stage chain after the correlation-field change: does `tools/m3-join-chain.py` build a
complete typed chain from a real reset/rejoin's joiner and responder logs, does `check_chain` accept
it, and does the M3 criterion (`join_chain`) refuse the record for locality? This is a local
four-validator Docker devnet. It is **not** the off-host M3 positive and not gate evidence.

## Provenance

| | |
|---|---|
| gate | `G428-M3` |
| source | `46c98b67b3dae3c22104ab737ad7bf4851862b0b` |
| binary | `d98c9a133dd4be551ea6d04ad9e20f16b6a2709eb63da1f37b5273260eac09d5` |
| run | none — local Docker devnet; no Actions URL |
| reference | `spec/audit/runbooks/m3-off-host-20261118.md`; `spec/audit/evidence/m3-m5-pareto-20261101/M3-14-journal.md` |
| packaged | `2026-11-18T22:44:10+00:00` |
| verdict | **INCONCLUSIVE** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The arm reset validator 3 and recovered it passively, then activated its key:

```
chain at {'n457join-bootstrap': 16, 'n457join-validator-1': 16, 'n457join-validator-2': 16}
passive replica caught up in 19.7s; activating its key
(passive-then-activate) in 4.4s: round=18, anchor 13 (de7afc38e50a, held by bootstrap=True);
sync=None; sync failures=0; self-equivocations=0; halted=False; proposed=2
```

`chain_complete` is `true`, and the responder log was captured with `docker logs --since`
(`chain_log_method: since`). The typed chain in `raw/chain.json` carries all six stages:

- `1_request` (joiner: responder key + requested anchor), `2_receipt` and `3_construction`
  (responder: joiner's advertised address, served state hash), `4_transport` (responder:
  `transport outcome delivered`), `5_receive` (joiner: served hash, blocks), `6_apply` (joiner:
  served = received = restored = walked hash, page walk finished, checkpoint read back).
- `python3 tools/m3-join-chain.py check-chain --chain raw/chain.json` → exit 0.
- `python3 tools/m3-join-chain.py check --record <record> --locality local-devnet` → exit 0.
- `python3 tools/gate-check.py --gate G428-M3 --observations observations.json` → exit 1, `FAIL`,
  reason: *"the record declares local-devnet; an off-host criterion requires 'off-host-claim'"*.

The two results are the point: the chain instrument works on a real run, and the gate criterion does
not accept a local record as the off-host positive.

## What this does not establish

- This is **not** an M3 off-host positive: one Docker host, one bridge, one disposable chain. The
  advertised peer is a container address, not a routable observer.
- The chain is a correlation of two logs, not a cryptographic delivery receipt. Stage 4 is the
  joiner's gRPC receiver acknowledgement (the server acks after spawning the dispatch), not handler
  completion.
- Stage 6 records the served hash and the completed, content-addressed page walk. For a certified
  fringe the accepted run root is a different root by design and is recorded separately; the chain
  requires equality only for a genesis anchor.
- Failover (M3-11) is absent and not exercised. No public deployment, real key, transfer or
  disruptive arm ran. `execution_proven` is false.
- No gate is promoted; `G428-M3` remains `blocked`.

## Reproduce

From the repository root at the source pin above, with `rnode:local` built at that commit:

```sh
DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \
DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
DEVNET_NODE_MEMORY=2g tools/devnet.sh up --validators 4 --fresh

DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \
DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
DEVNET_NODE_MEMORY=2g python3 tools/n457-mature-join-arm.py --supported-recovery \
  --min-height 16 --timeout 600 --out target/m3-chain/run

python3 tools/m3-join-chain.py check-chain --chain target/m3-chain/run/chain.json
python3 tools/m3-join-chain.py check --record <record> --locality local-devnet
python3 tools/gate-check.py --gate G428-M3 --observations <record>/observations.json
python3 tools/evidence-pack.py verify --record <record> --require-pins
tools/devnet.sh down -v
```

The arm writes `chain.json` beside `mature-join.json`; `evidence-pack.py` copies the run under
`raw/`, where the checker also looks for the chain.
