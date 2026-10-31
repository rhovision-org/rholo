# The cost model, and the gate that keeps it honest

The gas table is a **contract about work**: a phlo is meant to buy a bounded amount of it. The
contract's failure mode is specific and has now occurred twice — an operation whose cost to the node
grows faster than the charge the deployer pays, so a phlo buys unbounded work and the per-block phlo
cap stops bounding anything. September 2026 found it in `Set.union`'s dedup; October 2026 found it
again in the collection conversions, the union asymmetry and `rho:gov:censure`. **Both times it was
found by measurement**, because no gate in the tree could see a charge that does not track its work.

This page is the instrument that closes that gap, what it can and cannot see, and the shapes the
charges are supposed to have.

## The register

[`spec/COST.md`](../../../spec/COST.md) is rendered from [`spec/cost-register.tsv`](../../../spec/cost-register.tsv):
one row per priced operation, with

- **the work** it does, in a phrase (`clone the set and re-canonicalise it`);
- **the shape** of its charge — `constant`, `linear`, `product`, `quadratic`, `cubic`, `capped`, or
  `identity` (no charge and no work beyond the operand's own evaluation);
- **the basis** the charge is computed from (an operand count, a serialized size, a channel count);
- **the constructor** that charges it (`Costs::canonicalization_cost`), or the helper whose body does;
- **evidence** — the test that measures it — and any **residual**.

`capped` is the honest shape for work that is bounded by a constant rather than priced by phlo: the
pattern matcher's subset enumeration is capped by the product bound C124 introduced, so it cannot be
made to run unboundedly and does not need a charge. A capped operation is not the class defect; the
defect is *unbounded* work for a flat charge.

## The gate

`tools/check-cost-register.py` runs in the `test` job beside the spawn and type-system censuses, and
fails on:

- **an arm with no row.** Every method arm of `reduce.rs::eval_method` and every `Expr::E*` arm of
  `reduce.rs::eval_arm` must be declared. This is the half that matters: both defects arrived as a new
  or newly-read arm charging a constant, and silence is what let them.
- **a row for nothing.** An operation the dispatch no longer has, or a builtin symbol that no longer
  exists, fails as stale — a register that keeps rows for old code stops meaning anything.
- **a declared charge that is not where the row says.** A row's constructors (or helper) must occur in
  the arm's own region, so a row and an arm cannot drift apart.
- **an `identity` or `capped` row that names a charge**, a row with a shape outside the vocabulary, or
  a row missing a field.
- **evidence that resolves to nothing** — every `path::symbol` in the evidence column must exist in the
  tree.

`rholang/tests/cost_shapes.rs` is the other half and runs in `cargo test --workspace`: for each shape
family it evaluates the same operation at two sizes and asserts the **charged phlo** grows at the
declared rate. Charged phlo is deterministic — wall time is not — so this is a test rather than a
benchmark. It is what a charge that regresses to a constant fails even while its constructor is still
called.

Neither half needs a special build. `rholang/tests/gas_amplification.rs` remains the release-mode probe
for measuring *work* when a finding needs a number; it is not a gate.

## What the gate cannot see

- **The work, only the declaration.** A row saying `linear` over work that is `O(n log n)` passes. That
  is the recorded **log-factor residual**: a phlo buys work bounded by `charge · log charge`, which is
  bounded and is not the defect, but is not an exact price of a comparison.
- **A wrong declaration.** A new arm that charges a constant and declares `constant` passes. The
  difference from before is that the claim is now written down beside the work, in one table, and a
  reviewer reads it — not that a static check can judge it.
- **An undeclared *cost* that is not a charge site.** A builtin that never calls `charge` at all is a
  `capped` or `identity` row; if it is neither, the row is wrong, and that is a reading, not a check.
- **The constant.** One phlo per operation assumes operands cost about the same; a fold over a
  `BTreeMap` of strings steps slower than an integer addition. `rho:gov:resolveWeights` is the measured
  example (41 ms at the universe bound for a quadratic charge that matches its quadratic shape). The
  shapes are what this gate is about, not the calibration.

## The shapes, and where they came from

| shape | the work | an instance |
|---|---|---|
| `constant` | `O(1)`, whatever the operands | `nth`, a method dispatch |
| `linear` | one pass over an operand | `toSet` (canonicalisation), `toLowerCase`, storage |
| `product` | a scan per element, or two operands multiplied | `diff`, `indexOf`, `format`, a tally's rounds |
| `quadratic` | all pairs of a member set | the `rho:gov:*` folds' member walks |
| `cubic` | a fixed point over a universe | `rho:gov:censure` |
| `capped` | bounded by a named constant, not priced | the pattern matcher (C124) |
| `identity` | nothing beyond the operand's own evaluation | a collection literal, `isEmpty` |

The properties are stated as **Law 91** (`spec/Rchain/Cost.lean`): a canonicalisation charges the
elements it canonicalises, a union charges both operands, the censure fold charges the cube of its
universe, and a tally charges the option lists it scans. The gate is not a law — it is the instrument
that makes a new arm's silence a build failure, in the same way the spawn census makes a bare spawned
task a build failure.

## The record

- Pass §150 of [`spec/audit/passes.md`](../../../spec/audit/passes.md) — the instrument, and the two
  live instances its first census found (`set.add`, and the tallies' uncounted option lists).
- [`docs/src/node/security-audit.md`](../node/security-audit.md) §5 and §8 — the finding that motivated
  it, and the two questions that remain open (the log factor, and the constant).
- [`spec/RUST-AFFORDANCES.md`](../../../spec/RUST-AFFORDANCES.md) §6 — why this shape of cost model is
  available to Rust and was not to the JVM reference.
