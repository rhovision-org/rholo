# m3-join-chain-local-20261119

**Evidence locality.** `local-devnet`

## Scope

The #428 M3 six-stage chain after the review-response changes: does a real reset/rejoin produce a
chain whose `complete` is true, whose stage-4 words match its outcome, and whose quoted lines are
present in the named raw logs and pinned by `SHA256SUMS.json`? Does the local policy accept it while
the M3 criterion refuses it for locality? This is a local four-validator Docker devnet; it is
**not** the off-host M3 positive and not gate evidence.

## Provenance

| | |
|---|---|
| gate | `G428-M3` |
| source | `46c98b67b3dae3c22104ab737ad7bf4851862b0b` (the Rust binary's build commit) |
| binary | `d98c9a133dd4be551ea6d04ad9e20f16b6a2709eb63da1f37b5273260eac09d5` |
| tooling | the parent of this evidence commit (the raw-log binding and completeness changes) |
| run | none — local Docker devnet; no Actions URL |
| reference | `spec/audit/runbooks/m3-off-host-20261118.md`; `spec/audit/evidence/m3-m5-pareto-20261101/M3-14-journal.md` |
| packaged | `2026-11-19T08:23:21+00:00` |
| verdict | **INCONCLUSIVE** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The arm reset validator 3 and recovered it passively, then activated its key:

```text
chain at {'n457join-bootstrap': 17, 'n457join-validator-1': 16, 'n457join-validator-2': 16}
passive replica caught up in 17.5s; activating its key
(passive-then-activate) in 4.4s: round=19, anchor 14 (b56ec0be8cf2, held by bootstrap=True);
sync=None; sync failures=0; self-equivocations=0; halted=False; proposed=2
```

The chain is complete (`complete: true`, empty `incomplete_reasons`) and names its raw logs
(`logs: {joiner: joiner.log, responder: responder.log}`). It carries all six stages, and stage 4 is
the `sent to … transport outcome delivered` form:

- `python3 tools/m3-join-chain.py check-chain --chain raw/chain.json` → exit 0.
- `python3 tools/m3-join-chain.py check --record <record> --locality local-devnet` → exit 0. This
  now verifies that every quoted line is present in `raw/joiner.log` or `raw/responder.log` and that
  both logs match their `SHA256SUMS.json` digests.
- `python3 tools/gate-check.py --gate G428-M3 --observations observations.json` → exit 1, `FAIL`,
  reason: *"the record declares local-devnet; an off-host criterion requires 'off-host-claim'"*.

The two results are the point: the chain is bound to captured bytes and passes its own policy, and
the gate criterion does not accept a local record as the off-host positive.

## What this does not establish

- This is **not** an M3 off-host positive: one Docker host, one bridge, one disposable chain. The
  advertised peer is a container address, not a routable observer.
- The chain is a correlation of two logs, not a cryptographic delivery receipt. The raw-log binding
  catches an edited chain and an edited log that does not match its pin; it does not prove the run
  happened or that the responder's application handler completed.
- Stage 4 is the joiner's gRPC receiver acknowledgement (the server acks after spawning the
  dispatch). Stage 6 records the served hash and the completed, content-addressed page walk; for a
  certified fringe the accepted run root is a different root by design and is recorded separately.
  The chain requires equality only for a genesis anchor.
- Failover (M3-11) is absent and not exercised. No public deployment, real key, transfer or
  disruptive arm ran. `execution_proven` is false.
- No gate is promoted; `G428-M3` remains `blocked`.

## Reproduce

From the repository root at the source/tooling commits above, with `rnode:local` built at the source
pin:

```sh
DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \
DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
DEVNET_NODE_MEMORY=2g tools/devnet.sh up --validators 4 --fresh

DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net \
DEVNET_GRPC_BASE=55402 DEVNET_HTTP_BASE=55403 DEVNET_ADMIN_BASE=55405 \
DEVNET_NODE_MEMORY=2g python3 tools/n457-mature-join-arm.py --supported-recovery \
  --min-height 16 --timeout 600 --out target/m3-chain/run-2

python3 tools/m3-join-chain.py check-chain --chain target/m3-chain/run-2/chain.json
python3 tools/m3-join-chain.py check --record <record> --locality local-devnet
python3 tools/gate-check.py --gate G428-M3 --observations <record>/observations.json
python3 tools/evidence-pack.py verify --record <record> --require-pins
tools/devnet.sh down -v
```

The arm writes `chain.json`, `joiner.log` and `responder.log` beside `mature-join.json`;
`evidence-pack.py` copies the run under `raw/`, where the checker also looks.
