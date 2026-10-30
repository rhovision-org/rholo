# Governance-set parameters (plan H2)

**Status: design. Nothing here is implemented.** This is the second of the two items that decide
whether the next genesis can be the last one.

## Why this exists

The proof-of-stake parameters — `min_phlo_price`, `max_number_of_parents`, `executor_share`,
`absence_slack`, `participation_grace`, the bond bounds, and the epoch and quarantine lengths — are
**committed into the genesis** by `PosParams`, carried on `PosGenesis` and installed into native state
at block 0 (`rholang/src/native_state.rs`). The block-version schedule can change a *rule* at a height;
it cannot change a *value*, because the value is part of what block 0's post-state hash commits to.

The consequence is sharp: today, lowering the fee floor by one phlo, or raising the parent bound,
is a **new genesis**. That is not a parameter change, it is a chain replacement — and it is the reason
a "last genesis" needs this path to exist first.

## What the mechanism is

### 1. The record

`pos:params` holds the committed parameter record: **a format byte, then the fields**. The format byte
already exists — format 1 (ten fields) and format 2 (with the developer allocation's field at payload
index 10) are on the wire — so a later format is a scheduled change, not a wire change.

The record is the *only* source of these values. Any consumer that reads a parameter from the node's
configuration, or re-derives it from the genesis files, is a defect: two nodes with different configs
would then disagree about the same block.

### 2. The read: pre-state only (Law 11)

Every consumer reads the record **once per block, from that block's pre-state**, and threads the value
through the execution:

- validation reads the bounds from pre-state (this is what #376 began: the committed leaf, not the
  node's config);
- the runtime manager threads the same record into deploy execution;
- proposing reads it from the block's pre-state too, so a proposer and a validator cannot disagree.

Nothing reads the block's own post-state, a wall clock, or a configuration value. That is what makes a
param set at height *H* visible at *H+1* and deterministic on replay.

### 3. The setter

A `pos` system process — `setParams` — applied as a **system deploy** with a fixed cost, authorized by
the governance authority, carrying:

- the narrowest write: a list of `(field, new value)` pairs, not a whole record;
- `effective_from` — a height strictly greater than the block that carries the deploy;
- `expected_format` — so a set written against format 2 is refused if the chain has moved to format 3.

A narrow write matters: a whole-record write is a race surface (two setters, last one wins) and makes
the diff of a governance action unreadable.

### 4. The activation gate

`setParams` is refused — **invalid, not ignored** — before the schedule version that introduces it.
`SCHEDULE` gains a row; before it the process is inert, so the path ships without changing any block.
After it, the setter exists but only governance can reach it.

### 5. Bounds, so governance cannot brick the chain

Each field has a validity interval and a per-epoch delta bound, enforced at the setter:

| field | why a bound |
|---|---|
| `max_number_of_parents` | `0` or `1` stops merges; a huge value makes blocks unvalidatable |
| `min_phlo_price` | `0` re-opens the free-deploy flood; a huge value makes deploys unaffordable |
| `executor_share` | above 10 000 it is not a share; it changes who is paid |
| `epoch_length`, `quarantine_length` | `0` divides; a change moves every boundary |

A set outside the interval is refused (fail-closed), and so is a set that moves a field by more than
the delta bound in one epoch. Two refused sets are better than a chain that has to be restarted.

## What is *not* settable

- **The block format.** The version schedule, the format byte's meaning, and the field set are not
  parameters: changing them is a shape change, and shape changes are what `SCHEDULE` rows are for.
- **The quorum and finalizer thresholds.** Lowering them changes which blocks are valid *by
  definition*; that is a fork, and it must stay one.
- **The genesis allocation.** Its address and amount are genesis content (Laws 78–79), decided by the
  ceremony, not by a later vote.
- **The registry of blessed contracts.** Contract code changes through the upgrade path
  (`genesis-contract-upgrade-path.md`), which has its own authorization and its own review.

## Determinism, replay and cost

- the record is consensus state, so replay reproduces it; the setter's cost is fixed and charged as a
  system deploy;
- `effective_from` is part of the record, so a set carried by block *N* is inert in *N* and visible in
  *N+1* — including on a node that replays the chain from genesis;
- the post-state hash of every block before the activation height is unchanged.

## Falsifiers — the tests that must exist, and must fail without the mechanism

1. **A set at height *H* applies at *H+1*, and not at *H*.**
2. **Replay is identical:** a fresh node replaying from genesis reaches the same values and the same
   post-state hash at every height.
3. **A set outside the validity interval is refused.**
4. **A set moving a field beyond the per-epoch delta is refused.**
5. **A non-governance signer is refused** even at the activation height.
6. **Before activation the process is invalid** — a proposer including it produces a block validators
   reject.
7. **Evidence gate:** with the path active and unused, every parameter still equals the genesis value —
   the check that "shipped inert" actually shipped inert.

## Open decisions this design needs before code

1. **Authorization.** `pos_multi_sig_public_keys` / `pos_multi_sig_quorum` is the cheapest consistent
   answer and is already the genesis's governance mechanism. Weighted rgov voting is the alternative
   and is a larger change.
2. **Which fields.** Starting with the two that #373 committed and #376 enforces (`min_phlo_price`,
   `max_number_of_parents`) is the smallest honest scope; the reward and bond fields can follow in a
   later format.
3. **Effective height.** `H+1` is simple and testable; epoch-aligned is friendlier to operators but
   couples two mechanisms.
4. **Intervals and delta bounds.** These are consensus constants, so they belong in the schedule's
   release table rather than in a config file — otherwise the bound itself becomes a per-node value.

## What this page does not claim

Not that the mechanism is implemented, not that it is approved, and not that every future parameter
change will be covered: the fields left out of the first format will still need either a format change
(schedulable) or a decision. It claims that with this path, a *value* change stops being a chain
replacement.
