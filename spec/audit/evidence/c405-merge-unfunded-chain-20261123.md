# C405 — a merge refused an unfunded chain and stopped the proposer (2026-11-23)

## Scope

The merge of a bond-leave devnet run on 2026-11-23. A proposal on the height-41 view failed every time
with:

```
cost accounting would take a vault to -1902: the accepted deploys spend more than the merged state
holds, and no sequential execution reaches this
```

The refusal named slot `PREFIX_VAULT` key `abb3e13a…`, current `0`, delta `-1902`, host `8ad77440…`
(height 41). The spend was `leave-fund`'s (height 39, cost 1902, charge 1e6, refund 998098, burned
1902). Its funding block `4f933ca8…` (height 39) was an ancestor of the spender but **absent from
`to_merge`**: conflict resolution had rejected the funding chain whole and kept its descendant. The
spender then composed its burn onto a vault the merge base never funded. The refusal was deterministic,
survived a restart, and was reproducible with `POST /api/v1/propose`, so no block was produced on that
view.

## Result

The merge now treats a cost-accounting refusal as a **rejection**, not a merge failure:

- `compose_cost_accounting` records the negative, non-system hosts per slot and, on a negative
  composition, returns them in `refused_hosts`.
- `MergeScope::merge` drops the offending chains and their `with_dependencies` closure, adds them to
  `rejected`, and recomposes. If nothing can be dropped, it returns the original error — the honest
  terminal, not a silent success.
- A chain carrying a system deploy is never dropped. `refusal_closure` checks the **whole closure**,
  not just the offending host, because `with_dependencies` returns dependents (C344).

## Evidence

- `casper/src/merging.rs::compose_cost_accounting` — the refusal names the hosts and the slot.
- `casper/src/merging.rs::a_spend_whose_funding_chain_lost_is_rejected_rather_than_halting_the_merge`
  — the measured shape: the unfunded spender is rejected, the innocent chain merges, and the
  unfunded vault leaf remains absent.
- `casper/src/merging.rs::a_system_deploy_in_the_dependent_closure_is_not_dropped` — the C344 control:
  a system deploy in the closure makes the closure non-removable; the same graph without it is
  removable.

## Negative control

With `refusal_closure` forced to `None` (the repair disabled), the regression test fails:

```
the merge must reject the unfundable chain, not fail closed:
"cost accounting would take a vault to -500: the accepted deploys spend more than the merged state
holds, and no sequential execution reaches this [slot prefix 5 key f5267e3a… current 0 delta -500
base_state 68aaa201…]"
```

The `-500` is the fixture's burn; the live instance's `-1902` is the same shape. The system-deploy test
carries its own positive control: the same dependency graph without the system chain is removable, so a
guard that checked only the host would fail the test.

## Law

Law 53b, the merge-boundary inverse: `Rchain.MergeRepair` proves that a repair reports every drop, that a
system deploy is not removable, and that a merge with nothing removable fails rather than succeeding
with a silent loss. The proposition is `Rchain.MergeRepair.law53b_statement`; the mutant is the old
whole-merge failure.

## What this does not establish

- No public-fleet run. The evidence is the local merge test and the measured devnet instance; RC-1's
  stall arm is a separate change and the release-claim surface is untouched.
- The repair is conditional: if the surviving set cannot compose, the merge still fails. The law does
  not claim otherwise.
- No readiness claim; `--assert-ready` remains NO-GO.
