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

`test_evidence_locality.py` is already wired; the four tests above are not.

## Local equivalent

The PR checklist runs the same commands; `check-mainnet-gates.py --assert-closeable` and
`check-hardfork-51-closeout.py --verify-local-git --assert-closeable` both exit 3 by design.
