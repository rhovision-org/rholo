# Results — the durable-state census (issue #658)

- **commit:** `f9e509038` (the reviewed revision; the worktree base)
- **run:** `spec/audit/evidence/658-durable-state-census-20261123/run.sh`
- **identifiers derived:** 47
- **Table D rows:** 17 — `auto-recover` 8, `operator-action` 7, `safe-halt` 2
- **Table F exclusions:** 10
- **census tests owed:** 0
- **census surfaces owed:** 5 (`ownSequenceRecord`, `unrestorableRecords`, `equivocationEvidence`,
  `gatewayTxn`, `rspaceRoot`)

## Gates

| gate | result |
|---|---|
| `tools/emit-durable-states.sh --check` | 47 identifiers, current |
| `python3 tools/check-durable-state-census.py` | 0 violations |
| `tools/check-hazop-worksheet.sh` | 26 nodes, 152 Table A rows, falsifiers owed 30, node-words owed 44 |
| `python3 tools/check-node-diagnostics.py --gate` | passes (pending the PR run) |

## Negative controls

Run against a temporary copy of the page (or, for the last, a temporary append to a scanned file).
Each was expected to be refused; the untouched page was expected to pass.

| # | control | expected | observed |
|---|---|---|---|
| 1 | remove D26-01 | red: `RESET_MARKER_DB` unclassified | red — "durable identifier RESET_MARKER_DB is not classified by Table D or Table F" |
| 2 | class `auto` | red: class vocabulary | red — "class [auto] is not in (auto-recover, operator-action, safe-halt)" |
| 3 | fabricated test symbol | red: symbol does not resolve | red — "casper/tests/reset.rs::a_test_that_does_not_exist does not resolve" |
| 4 | `owed:` test without a debt update | red: counted 1, declared 0 | red — "declared census tests owed 0 but counted 1" |
| 5 | `owed:` surface absent from §5 | red: no §5 row | red — "owed surface notInSection5 has no node-diagnostics §5 row" |
| 6 | `#[cfg(test)]` key in a scanned file | green: test-only keys are excluded | green — the key was not emitted |
| 0 | untouched page | green | green, 0 violations |

Controls 3 and 4 first needed the control to be stated in the page's own grammar (an unbackticked
`owed:` for a debt test); the first attempt at control 3 replaced a string that occurred in another
row first. The final controls above are the ones recorded.

## What the census re-derived from the issue's hypotheses

- **Hypothesis 6 (key-equivocation halt) is not durable.** `OWN_KEY_EQUIVOCATION` is a process-global
  cell (`casper/src/blocks/block_processor.rs`), and the durable equivocation evidence is a separate
  store (D26-13). The halt itself is RC-4's in-memory latch; the census excludes it from the durable
  set and names the evidence store instead.
- **Hypotheses 15 and 16 are mostly in-memory.** The durable half of LFS/catch-up is the reset marker
  (D26-01); the durable half of the proposer self-validation halt is the autopropose halt record
  (D26-03). The rest is RC-4.
- **The first pass's line numbers were re-derived** against `f9e509038`. The two examples that had
  moved — F-U4-01's drop arm and F-U7-01's expiry boundary — are dispositioned in Stages 2 and 3 of
  #658, not carried as live Terminal rows.
- **Additions the issue's list did not name:** the v3 disposition-ledger copy keys (D26-07), the
  checkpoint and audit stores (D26-10/11), the RSpace roots (D26-15), the two boot markers
  (D26-16/17), and the gateway ledger (D26-14).

## Limits and non-claims

- The scan covers the store files named in U26's recipe. It does not discover a durable store added in
  a new module; that module is added to the recipe deliberately.
- A test named in a row proves the writer/clearer contract it is written against, not an end-to-end
  crash at every persistence boundary. Crash injection at those boundaries is a separate, held arm.
- The census adds no release claim and changes no consensus rule. F-U26-02/F-U26-03 are `owed:` until
  Stage 4; F-U3-01's live-testnet drill remains a release-campaign item.
- No live devnet or testnet arm was run for this pass. The census is a document-and-tree claim, and
  its two checkers are the evidence.
