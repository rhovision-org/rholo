# The cost register — measurements, 2026-10-30

Evidence for `spec/audit/passes.md` §150 (the cost register, C353 and C354), and for the three
`rho:gov:*` folds C346 left unmeasured. Branch `cost-register`, off `dev@78efd3fa1`.

## `fold-probe.log`

`RNODE_GAS_PROBE=1 cargo test -p rchain-rholang --test gas_amplification --release -- --nocapture
--test-threads=1`, release, 2026-10-30. The `censure_*` rows are §149's, re-run on this tree; the
`trust_levels` / `resolve_weights` / `tally_*` rows are this pass's.

| case | universe / shape | charged phlo | measured |
|---|---|---|---|
| `trust_levels`, chain of 511 | 512 | 262 144 | **0 ms** |
| `resolve_weights`, chain of 511 | 512 | 262 144 | **41–42 ms** |
| `tally_ranked`, 100 ballots × 4 000 options | 200 | **40 000** | **21–34 ms** |
| `tally_approval`, 128 ballots × 4 000 approvals | 256 | **65 536** | **68 ms** |

What the numbers say:

- **`trustLevels` is quadratic, not cubic.** A conferral cannot raise a level past the rater's, so the
  monotone relaxation converges in at most five rounds whatever the chain's depth — 0 ms at the bound.
  The squared charge matches the shape.
- **`resolveWeights` is quadratic in shape and in charge.** Every member walks the delegation chain, so
  the work is `U · L`; the 42 ms at the bound is the table's one-phlo-per-step applied to a
  string-keyed map lookup, which is a *constant* mismatch (≈600× the table's per-operation rate), not
  an unbounded one. Recorded in `spec/COST.md` as `quadratic` / `measured` rather than repriced.
- **Both tallies charged by the ballot and weight count alone.** The option lists were not in the
  universe at all: 4 000 options per ballot bought 21–68 ms for 40 000–65 536 phlo, the same charge
  the same election would pay with eight options a ballot. This is C354, and the charge now grows
  with the entries (Law 91c).

## Tests

```text
cargo test -p rchain-rholang --test cost_shapes
python3 tools/check-cost-register.py
python3 -m unittest tests.test_cost_register
cargo test -p rchain-rholang --lib -- a_set_add_charges_the_set_it_rebuilds a_tally_charges_the_option_lists_it_scans
```

The two charge tests were shown to bite: with the previous charges restored in the tree they fail with
`0` against `3` (the set insertion) and `0` against `54` (the ranked tally).

## What this does not show

No populated-store migration, no second validator binary, no pre-fork replay refusal — those are the
consensus-change acceptance this unit does **not** claim, and the ledger item carries them. The gate
reads declarations, not work; `trustLevels`/`resolveWeights` being measured is a record, not a proof
that no other fold's constant is off.
