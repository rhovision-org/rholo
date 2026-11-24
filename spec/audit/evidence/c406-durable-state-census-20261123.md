# C406 — the durable-state census (issue #658)

- **revision:** `f9e509038`
- **detail:** `spec/audit/evidence/658-durable-state-census-20261123/results.md`
  (preregistration, derived TSV, per-row derivation and the negative controls are in that directory)
- **derived identifiers:** 47
- **Table D:** 17 rows — 8 `auto-recover`, 7 `operator-action`, 2 `safe-halt`
- **Table F:** 10 explicit exclusions
- **owed:** 0 tests, 5 surfaces (`ownSequenceRecord`, `unrestorableRecords`,
  `equivocationEvidence`, `gatewayTxn`, `rspaceRoot`)

**Gates.** `tools/emit-durable-states.sh --check` (current); `tools/check-durable-state-census.py`
(0 violations); `tools/check-hazop-worksheet.sh` (26 nodes, 152 rows, 30 falsifiers owed, 44
node-words owed); `tools/check-node-diagnostics.py --gate` (0 violations).

**Negative controls.** A removed row, an unknown class, a fabricated `path::symbol`, an uncounted
`owed:` test and an `owed:` surface absent from §5 are each refused; a `#[cfg(test)]` key stays
excluded.

**Later change (C408).** Stage 4 added the divergence-halt key: the emitter now derives 48
identifiers and Table D has 18 rows; the `f9e509038` baseline in this record was 47 / 17. See
`spec/audit/evidence/658-durable-state-census-20261123/results.md`.
