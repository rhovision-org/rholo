# G428-M2 public-live pressure campaign — pre-registered protocol

**Status: DRAFT — NOT EXECUTED.** This runbook fixes the protocol before a run so the record
cannot be assembled around the result. It does not claim that any run has happened.

- Gate: `G428-M2` — bounded public ingress and overload behavior.
- Evidence destination: `spec/audit/evidence/m2-public-live-<date>/`.
- Prerequisite reading: `spec/audit/gate-principles-20261118.md` (`pressure_refusal`),
  `tools/m2-pressure.py`, `tools/m2-pressure-arm.py`.

## 1. What this window must establish

On the installed candidate: a capped lifetime that holds every configured permit and refuses the
burst with 429 and `retry-after: 1` at HTTP, direct HTTP and gRPC; an unset lifetime that refuses
nothing; no 5xx; a handler reachable after release; and a record whose endpoint is non-local and
whose every surface is bound to that endpoint. The record is judged by
`gate-check.py --gate G428-M2`; its locality must be `public-live`.

The gRPC half is explicitly a **candidate-code** measurement: the in-process Rust test binds a
loopback listener and exercises `PeerAddressIo`/`DeployIngressGuard`. The record must declare
`origin: candidate-test`, `scope: candidate-code` and `endpoint.used: false`. It may not claim
endpoint transport.

## 2. Hard NO-GO conditions

- **A local, private or loopback endpoint.** `assemble` derives the locality from the endpoint and
  refuses a contradiction; a `local-devnet` record is not gate evidence.
- **A proxy between the observer and the node.** `endpoint.proxy` must be `none`.
- **A 5xx anywhere in the probe transcripts**, or an unset control that refuses.
- **A different candidate** between the two lifetimes, or a gRPC candidate pin that differs from
  the record's `candidate`.
- **A gRPC surface that claims endpoint transport from the in-process test.**
- **A missing host capture.** A non-local phase needs `--inspect` and `--version` captured on that
  host; reading a local container's facts is refused.
- **TLS direct-socket measurement.** The direct arm is HTTP-only; a TLS direct arm is deferred.
- **Any gate promotion is out of scope.** None is expected from this run.

## 3. Preflight pins (fill before starting; a missing pin is NO-GO)

| field | value |
|---|---|
| selected source SHA (40-hex) | |
| selected binary SHA-256 (64-hex) | |
| the validator's HTTP endpoint (non-local) | |
| the validator's gRPC endpoint (same host) | |
| capped lifetime: container config and `rnode --version` captured on the host | |
| unset lifetime: the same two captures | |
| probe host, probe rate and the per-peer ceiling | |
| window start / end (UTC) | |
| operator / independent reviewer | |
| evidence destination | `spec/audit/evidence/m2-public-live-<date>/` |

No public deployment, real key or transfer is required for this arm. If the candidate is on a
public chain, the authorization is recorded here before the run.

## 4. Procedure

1. Start the capped lifetime with `RNODE_HTTP_MAX_INFLIGHT_DEPLOY=<cap>` in the node's
   environment. The cap is read at listener startup, so the capped and unset lifetimes are two
   node lifetimes on the same binary, not a mutation of a running node.
2. Capture the host facts and drive the phase:
   ```sh
   python3 tools/m2-pressure-arm.py phase --mode capped --out <run>/capped \
     --endpoint http://<host>:<http-port> \
     --inspect <run>/host/capped-inspect.json --version <run>/host/capped-version.txt \
     --port <http-port> --cap <cap>
   ```
3. Restart the same binary without the cap and repeat with `--mode unset`.
4. Run the gRPC observation:
   ```sh
   python3 tools/m2-pressure-arm.py grpc --out <run>/grpc \
     --source-sha <40hex> --binary-sha256 <64hex> \
     --endpoint http://<host>:<grpc-port> --locality public-live
   ```
5. Assemble; the locality is derived from the endpoint:
   ```sh
   python3 tools/m2-pressure-arm.py assemble --capped <run>/capped --unset <run>/unset \
     --grpc <run>/grpc --out <record> \
     --endpoint http://<host>:<http-port> \
     --source-sha <40hex> --binary-sha256 <64hex>
   ```
   A refusal is a NO-GO, not a reason to edit the arm output.
6. Judge, then write the criterion observation **into the record before packing**:
   ```sh
   python3 tools/m2-pressure.py check --record <record> --locality public-live
   printf '{"source": {"path": "node/src/runtime/node_runtime.rs"}, "pressure": {"dir": "%s"}}\n' \
     "<record path>" > <record>/raw/observations.json
   python3 tools/evidence-pack.py pack --run-dir <record>/raw --record <record> \
     --gate-id G428-M2 --verdict INCONCLUSIVE --locality external \
     --source-sha <40hex> --binary-sha256 <64hex>
   python3 tools/evidence-pack.py verify --record <record> --require-pins
   python3 tools/gate-check.py --gate G428-M2 --observations <record>/raw/observations.json
   ```

## 5. Negative controls (each captured in a labelled record, never described without bytes)

- **A local endpoint on a public-live claim.** Assemble with `--endpoint http://127.0.0.1:<port>`:
  `assemble` derives `local-devnet` and the criterion refuses the public-live policy.
- **A proxy.** Set `endpoint.proxy` to a non-`none` value: the criterion FAILs.
- **A gRPC candidate from another source or binary.** The criterion FAILs the pin match.
- **A candidate-test surface claiming the endpoint.** The criterion FAILs `endpoint.used`.
- **An HTTP or direct surface from another host.** The criterion FAILs the listener match.
- **An unset lifetime that refuses.** The control fails.

## 6. What this window does not establish

- The gRPC surface is candidate-code, not endpoint transport. A real remote pool-full client is
  deferred: the live pool cannot be deterministically saturated without test hooks.
- It is not a proof that the run happened: `execution_proven` is false; the digests prove the bytes.
- It does not measure multi-validator independence; one node per surface is the M2 subject.
- It does not promote `G428-M2`; the register's witness/control/evidence binding and the
  `G584-G1` dependency are separate decisions.
- TLS direct-socket pressure is not measured.
