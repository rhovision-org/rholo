# C345–C352 remediation — 2026-10-30

The measurements and tests behind `spec/audit/passes.md` §149, which remediates the eight findings of
the October 2026 re-audit (§148). Branch `audit/security-2026-10` (PR #514), on top of the audit commit
`75badc4d6`.

## `gas-probe-after.log`

`RNODE_GAS_PROBE=1 cargo test -p rchain-rholang --test gas_amplification --release -- --nocapture
--test-threads=1`, release, run 2026-10-30 on the remediated tree. The same instrument as §148's
`spec/audit/evidence/n20261030-reaudit/gas-probe.log`, so the two logs are comparable cell by cell.

What moved:

| case | before (n20261030-reaudit) | after |
|---|---|---|
| `toSet` delta, N = 500 | 18 phlo | 1 518 phlo |
| `toSet` delta, N = 40 000 | 18 phlo | 120 018 phlo (wall 453 → 652 ms, unchanged) |
| `set` delta | 22 flat | 18 + 3N |
| `keys` delta | 10 flat | 18 + 3N |
| `++` (two N-element sets) | `3·N` for one operand | `3·2N` — both operands |
| `censure`, 512-member universe | `N²` charged for Θ(U³) | `(U+1)³` = 1.35e8 phlo; **33 ms** measured in the members-only shape, 1–5 ms in the three mixed shapes |

The `toMap`/`keys` rows' *deltas* are negative in both logs because the baseline is a list literal and
the converted term is a map — the storage charge differs between the two shapes and dominates. The
comparable quantity is the per-element slope, which moved by exactly +3 phlo per element across the two
runs (−10 → −7 for `toMap`); the exact charge is pinned by
`rholang/src/reduce.rs::to_set_charges_its_elements` and its siblings, not by this log.

The censure timings set the bound rather than the other way round: the four shapes a 512-member
universe admits (the bound is on the **sum** of censures, levels and vouchers) are 33 / 5 / 1 / 1 ms, so
`MAX_GOV_UNIVERSE` stays 512 and the cubic charge is the same order as the time it buys.

## Tests

Each finding's falsifier is named in §149's table. Run individually:

```text
cargo test -p rchain-rholang --lib collection_and_censure_charges_are_the_models
cargo test -p rchain-rholang --lib -- to_set_charges to_map_charges keys_charges set_charges a_union_charges
cargo test -p rchain-rholang --lib censure_is_charged_the_cube_of_its_universe
cargo test -p rchain-ocapn --lib a_dial_slot_is_taken_from_the_shared_budget_and_released_with_the_permit
cargo test -p rchain-ocapn --lib every_dialing_fixture_draws_from_the_shared_budget
cargo test -p rchain-ocapn --lib a_silent_client_does_not_hold_the_accept
cargo test -p rchain-shared --features lmdb --lib put_across_writes_both_or_neither_and_refuses_two_environments
cargo test -p rchain-casper --lib the_writer_stores_the_body_and_its_row_together
cargo test -p rchain-casper --lib a_view_without_a_finalized_block_still_trips_the_bound
cargo test -p rchain-casper --lib mapping_has_expected_databases
```

## What this evidence does not show

No live multi-node attack, no populated-store migration (the harness uses a fresh temporary LMDB
directory), no end-to-end greeter dial (the call site is pinned by the source ratchet; the dial itself
is the reference suite's), and no demonstration that the cost class is closed — the linear charge leaves
a logarithmic factor by construction, which §149 states.
