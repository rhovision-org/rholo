# CI wiring request — owner merge

PR #606 adds Python tests and emitted tables that are not in `.github/workflows/*`. The token
lacks `workflow` scope, so this file records the exact additions for the owner. No workflow file
is edited in the PR.

## `ci.yml` — "Tests and the node's static gates"

Add beside the existing `test_live_gate.py` / `test_evidence_pack.py` / `test_evidence_locality.py`
runs:

```sh
python3 -m unittest discover -s tests -p test_mainnet_gates.py -v
python3 -m unittest discover -s tests -p test_section6_dispositions.py -v
python3 -m unittest discover -s tests -p test_bounded_ingress_queues.py -v
python3 -m unittest discover -s tests -p test_gate_contracts.py -v
python3 -m unittest discover -s tests -p test_gate_criterion.py -v
python3 tools/check-mainnet-gates.py --check
python3 tools/check-section6-dispositions.py
```

Add the register and table paths to any path-gated job that should run on a change:

- `spec/audit/mainnet-gates.json`, `spec/audit/mainnet-gates.md`, `spec/audit/mainnet-gates.external.json`
- `spec/audit/section6-dispositions.json`
- `spec/audit/sources/*.md`
- `spec/audit/*.tsv` (the emitted C343/C357 tables)

`test_evidence_locality.py` is already wired; the five tests above are not. The queue gate
itself (`tools/check-bounded-ingress-queues.sh --gate`) already runs in the lint job; the new
test proves the gate's own absent/unbounded/unpinned/unknown cases.

## Principle-level criteria (2026-11-18)

Add beside the runs above:

```sh
python3 -m unittest discover -s tests -p test_gate_principles.py -v
python3 -m unittest discover -s tests -p test_gate_collect.py -v
python3 tools/gate-check.py --list
```

`test_gate_principles.py` is the generic suite over every entry in
`spec/audit/gate-criteria.json`: a satisfying observation, each declared negative control, each
missing required observation, malformed input, non-vacuity, actor-name independence and coverage
against the external register. `check-mainnet-gates.py::validate` already validates the registry,
and that function runs inside `check-hardfork-51-closeout.py` (which is wired), so a drift in
the criteria coverage fails today even before this test is added. `gate-collect.py` is a
read-only operator tool; it is not run in CI.

Add `spec/audit/gate-criteria.json` and `spec/audit/gate-principles-20261118.md` to the
path-gated job's paths.

## Gate-closure promotion and the G520 drill (2026-11-18)

Add beside the runs above:

```sh
python3 -m unittest discover -s tests -p test_populated_store_migration_drill.py -v
python3 tools/populated-store-migration-drill.py self-test
python3 tools/check-mainnet-gates.py --run-controls
```

Add to the path-gated job:

- `spec/audit/mainnet-gates.bindings.json`
- `spec/audit/s6-test-plan.json`
- `spec/audit/section6-reviews/`
- `spec/audit/evidence/g520-migration-store-drill-*`
- `spec/audit/evidence/s6-*-20261118*`

The S6 test batch adds one Python witness that unittest discovery does not see:

```sh
python3 tests/liveness-enforcement.py self-test
python3 -m unittest discover -s tests -p test_s6_bind.py -v
```

`tests/liveness-enforcement.py` runs the in-process jail test; the live/devnet liveness arms remain
release obligations. `tools/s6-bind.py` is an operator tool and is not run in CI.

`self_test` is hermetic; the full drill (`--gate`) needs Docker and a disposable devnet, so it is
an operator run rather than a CI step. `check-mainnet-gates.py --run-controls` executes the G520
control (`::self_test`) and reports the Rust witness controls as `SKIPPED`; the Rust FAIL records
are committed evidence.

## Local equivalent

The PR checklist runs the same commands; `check-mainnet-gates.py --assert-closeable` and
`check-hardfork-51-closeout.py --verify-local-git --assert-closeable` both exit 3 by design.
