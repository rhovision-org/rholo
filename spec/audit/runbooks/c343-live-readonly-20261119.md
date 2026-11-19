# C343 live B/C/D read-only phase diagnosis — pre-registered protocol

**Status: DRAFT — NOT EXECUTED.** This runbook fixes the protocol before a run so the record
cannot be assembled around the result. It does not claim that any run has happened.

- Gate: `C343` — private-chain surface: live B/C/D read-only phase diagnosis.
- Evidence destination: `spec/audit/evidence/c343-live-<date>/`.
- Prerequisite reading: `tools/c343-live.py`, `spec/audit/closeout-phase-space-20261117.md`
  (the inline C343-LIVE protocol), and `spec/audit/phase-diagnosis.tsv`.

## 1. What this window must establish

A read-only snapshot of hosts B, C and D: the public `/health` contract, the admin
`/api/v1/status/operator` contract, and each host's block-6 journal text, with the phase
discriminator, block-6 disposition, height gap, fault latch and isolation. The observation's
locality is `live-host`; the criterion is `gate-check.py --gate C343`.

The current installed build returns 4xx on the operator route. A host that cannot serve it is a
NO-GO for that host, not a reason to remove the field.

## 2. Hard NO-GO conditions

- **Any write or disruptive action.** This is GET-only on `/health` and
  `/api/v1/status/operator`.
- **A local-devnet label.** The criterion requires `live-host`; a local rehearsal is a different
  record.
- **A missing admin endpoint.** An installed build that does not serve
  `/api/v1/status/operator` cannot produce the operator half.
- **A fabricated block-6 disposition.** A fresh devnet journal has no block-6 disposition; that is
  a `PRECONDITION`, recorded as such, never filled in.
- **A host set other than B/C/D** for the gate's snapshot.
- **Any gate promotion is out of scope.** None is expected from this run.

## 3. Preflight pins (fill before starting; a missing pin is NO-GO)

| field | value |
|---|---|
| host aliases and endpoints (B, C, D) | |
| admin (operator) endpoints for each | |
| selected source SHA (40-hex) and binary SHA-256 (64-hex) | |
| journal paths for each host | |
| read-only credential source (never committed) | |
| window start / end (UTC) | |
| operator / independent reviewer | |
| evidence destination | `spec/audit/evidence/c343-live-<date>/` |

No public transfer and no key are needed; the access is read-only. The private credentials never
enter Git.

## 4. Procedure

1. Write the plan (schema 1): `campaign`, `locality: "live-host"`, one `hosts` entry per alias
   with `alias`, `base`, `operator_base`, `auth_env`, `source_sha`, `binary_sha256`, and the
   `journals` map.
2. Collect:
   ```sh
   python3 tools/c343-live.py collect --plan <run>/plan.json --out <record> \
     --source-sha <40hex> --binary-sha256 <64hex> --locality live-host
   ```
3. Verify:
   ```sh
   python3 tools/c343-live.py verify --run-dir <record> --purpose current-build \
     --expected-source-sha <40hex> --expected-binary-sha256 <64hex> --require-hosts B,C,D
   ```
   `PRECONDITION` on a missing journal is a recorded gap, not a pass.
4. Write the criterion observation into the record before packing, then pack and verify:
   ```sh
   python3 tools/evidence-pack.py pack --run-dir <record>/raw --record <record> \
     --gate-id C343 --verdict INCONCLUSIVE --locality external \
     --source-sha <40hex> --binary-sha256 <64hex>
   python3 tools/evidence-pack.py verify --record <record> --require-pins
   python3 tools/gate-check.py --gate C343 --observations <record>/raw/observations.json
   ```

## 5. Negative controls (each captured in a labelled record)

- **A missing block-6 disposition.** The criterion returns `PRECONDITION`, not PASS.
- **A contradictory readiness/reason pair.** The criterion FAILs.
- **A local-devnet label.** The criterion refuses the locality.
- **A missing contract field.** The criterion returns `PRECONDITION` for an incomplete contract.
- **A 404 or unparsed operator response.** PRECONDITION, recorded as such.

## 6. What this window does not establish

- A read-only snapshot is not a live verdict beyond the phase contract; it does not prove the
  chain healthy.
- `execution_proven` is false; the digests prove the bytes, not the run.
- It does not promote `C343`; the register's witness/control/evidence binding is a separate
  decision. The gate has no `depends_on`.
