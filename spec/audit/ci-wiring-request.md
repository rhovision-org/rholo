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
python3 tools/check-section6-dispositions.py
```

`tools/check-mainnet-gates.py --check` is now reached by the already-wired
`tests/test_hardfork_51_closeout.py` (see below), so this line is a direct invocation rather than
the only guard.

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
`spec/audit/gate-criteria.json`: malformed input, actor-name independence, coverage against the
external register and adapter binding. The behavioural assertions (a satisfying observation, every
declared control, every missing required input) now live in `tools/gate-selftest.py`, which
`check-mainnet-gates.py::check_criteria` runs. **Correction, 2026-11-19:** the earlier claim that
`validate` runs inside `check-hardfork-51-closeout.py` was false — `gate_binding` returns before
`validate` when the register has no HF51 rows. The wired path is now
`tests/test_hardfork_51_closeout.py::MainnetRegisterTests`, which runs
`check-mainnet-gates.py --check`; that check includes the self-test, the 47 met rows' evidence and
the build comparison. A depth-1 checkout records `register_dev_sha` ancestry as
`unverified-shallow` (a full checkout still enforces it). `gate-collect.py` is a read-only operator
tool; it is not run in CI.

Add `spec/audit/gate-criteria.json` and `spec/audit/gate-principles-20261118.md` to the
path-gated job's paths.

## Gate-closure promotion and the G520 drill (2026-11-18)

Add beside the runs above:

```sh
python3 -m unittest discover -s tests -p test_populated_store_migration_drill.py -v
python3 tools/populated-store-migration-drill.py self-test
python3 tools/check-mainnet-gates.py --run-controls --controls python
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
an operator run rather than a CI step. `check-mainnet-gates.py --run-controls --controls python`
executes the two `::self_test` controls and reports the 45 Rust witness controls as `SKIPPED` by
selection.

**The Rust corpus (2026-11-19).** `--run-controls` now defaults to `--controls all`: each Rust
control applies its declared mutation, runs the witness by name through cargo, and requires the
symbol's own libtest line to fail. That recompiles one crate per control (a warm target makes it
minutes, not the old `SKIPPED`). CI keeps the fast `--controls python` line above; run the full
corpus at release time and record it. `tests/test_rust_controls.py` is reached through the existing
`tests/test_hardfork_51_closeout.py` discovery (the `RustControlTests` wrapper), so no additional
workflow line is needed; its live case builds a dependency-free toy crate, not the corpus.

## The release-claim tracker (2026-11-19)

The 12 old campaign rows were deleted and replaced by the seven claims in
`spec/audit/release-claims.json`, emitted to `spec/audit/release-campaigns.json` and `.md`.
`tests/test_release_campaigns.py` is reached through the existing
`tests/test_hardfork_51_closeout.py` discovery (`ReleaseClaimTests`), so no workflow line is
needed; its CI-reached `--check` validates the generated tracker and the superseded mapping.
`check-release-campaigns.py --assert-ready` (exit 3) is the release checkpoint while any claim or
sub-observation is not closed. The old criteria self-test is not run here: a release claim's
observation and tool are its criterion.

## HF51 provenance (2026-11-19)

`tests/test_hardfork_51_closeout.py` and `tests/test_section6_inventory.py` already run in the
static-gates job; their new cases are reached there, including a live dependency-free toy crate for
the merged-row acceptance runner. Two checks are **release-time**, not CI steps:

```sh
python3 tools/check-section6-inventory.py --verify-pinned-blob
python3 tools/check-hardfork-51-closeout.py --verify-local-git --verify-pinned-blob \
        --run-merged-tests --assert-closeable
```

`--verify-pinned-blob` reads `spec/audit/passes.md` from the ledger's `source_dev_sha`; the test
checkouts are depth 1, so the check records `unverified-shallow` there (exit 4 when requested
explicitly, and never closeable). The full-history job, or an operator checkout, establishes it.
`--run-merged-tests` is 14 cargo invocations (13 rows) and is run at release time; no workflow line
or `.github/workflows/*` edit is proposed.

## Local equivalent

The PR checklist runs the same commands. The gate register is 47 gates, all met, so
`check-mainnet-gates.py --assert-closeable` exits **0**; release is still blocked by
`check-release-campaigns.py --assert-ready` (exit 3, seven claims open or owed) and by
`check-hardfork-51-closeout.py --verify-local-git --assert-closeable` (exit 3).

## The M4 and M2 suites (2026-11-19)

`tests/test_m4_campaign.py` and `tests/test_m2_pressure.py` are reached from
`tests/test_hardfork_51_closeout.py` (`FinalityCampaignTests`, `M2PressureArmTests`), the same
discovered seam the register check and the M1 collector use. No workflow file changes: the
discovery line for `test_hardfork_51_closeout.py` already exists.
