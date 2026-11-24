# C411 — the launch gate's external clause

Pinned to the release-campaign-readiness branch on `dev@ad9418aac` (PR #670 merged).

## Defect

The `launch-gate` programme action promised to "refuse ready while any external item is open
without a dated disposition", but no checker read `spec/audit/mainnet-gates.external.json`.
`check-release-campaigns.py` examines arms only, so the eight open external rows were invisible to
every readiness command.

## Change

- `spec/audit/launch-gate.dispositions.json` carries decisions only: `gate_id`, `disposition`,
  `dated`, `authority`, `reference`, `note`.
- `tools/check-launch-gate.py` derives the open set from the external register.
  - `--gate`: exit 0 when every open row has exactly one valid dated disposition; exit 3 when
    missing; exit 2 on malformed, duplicate, unknown, superseded, future-dated or empty rows.
  - `--assert-ready`: exit 0 only when `tools/release-check.py --assert-ready` is green and every
    external row is `met` or `superseded`.
- Only the M5 owner decision of 2026-11-15 is recorded; the other seven rows are missing.

## Witnesses

`tests/test_launch_gate.py`: the committed register is missing seven dispositions; a missing row,
a duplicate, a disposition on a superseded row, an unknown id, a future date, an unknown
vocabulary word, an empty authority and a non-https/out-of-tree reference are refused;
`--assert-ready` remains 3 with a valid M5 disposition while M5 is open, and passes only with all
claims green and all external rows closed.

## Mutation controls

Removing the date check accepts a future decision. Removing the superseded-row refusal lets a
disposition rewrite a closed row. Removing the external-open check from `--assert-ready` would call
a fleet ready while M5 is blocked.

## Non-claims

A disposition is not a waiver or a pass. M5 remains blocked. No launch authorization is created;
the aggregate checker is not an independent sign-off.
