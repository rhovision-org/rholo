# C409 — an unreadable halt is a halt, and an acknowledgement names the halt it read

- **code:** `6a581d733` on `finish/658-round` (single PR to `dev`); the stranded round content is the
  merge `0b04c8466`
- **tests:** `casper/tests/divergence_halt.rs` (the negative controls and the state transitions)
- **review:** the independent recovery review of #667 at `8b5b7494a` (Aria), whose two gates this
  record closes.

## The finding

The divergence-halt acting rule (C408) had two fail-open paths:

1. **An unreadable record read as absence.** `seed_divergence_halt_from_store` returned `Err` on a
   store-read failure but installed nothing; `setup_shard_runtime` only logged. On a fresh process
   the registry was empty, so `Proposer::do_propose`'s `any()` could not distinguish an unreadable
   halt record from "no halt on record" — the node proposed on a view whose durable halt it could not
   read. The storeless test cleared the registry after the failed seed and asserted nothing, so it
   could not fail.
2. **An acknowledgement of an older halt could delete a newer one.** `clear_autopropose_halt` did
   read A → acknowledge A → delete → registry-clear in separate awaits. A halt B recorded by
   `note_unrestorable_if_exhausted` between them was deleted by A's acknowledgement.

A third, latent defect was found while designing the fix: when the store held no record but the
registry held a `durable: false` halt (the halt-time write failed), the old clear path skipped the
divergence entirely, so the halt's only exit was unreachable.

## The rule

- **Unknown is not absence.** The registry carries `HaltCell::{Halted, Unreadable}`. A production
  shard is marked `Unreadable` at construction and again before/on a failed boot read; the proposer
  refuses every trigger with `DivergenceHaltUnreadable`; serving continues. A **successful** read
  retires the unknown state; the operator's resume is refused until one succeeds.
- **An acknowledgement names the halt it read.** A per-shard transition lock serialises
  `record_halt` (write → latch) against `acknowledge_divergence_halt` (read → acknowledge → delete →
  forget). The clear acknowledges the newest standing halt, so a newer halt makes the older
  acknowledgement fail rather than delete it. A non-durable registry halt is acknowledged and
  cleared too.
- **Lean first.** Law 76 (`spec/Rchain/Stop.lean`) is amended in place, no renumbering:
  `HaltState` gains `read` and `haltedOn`; `haltUnread`/`haltReadFailedStep` make an unreadable
  record a halt (`an_unreadable_halt_record_stops_production`); `operatorResumeStep` requires the
  acknowledgement to equal the standing halt
  (`an_acknowledgement_of_an_older_halt_cannot_resume_a_newer_one`, witnessed by
  `a_newer_halt_survives_an_acknowledgement_of_the_older`). The register row is amended in
  `spec/Rchain/Laws/Chunk05.lean` (the chunked register `dev` now carries), then re-emitted.

## The surface

`divergenceHalt` is one tagged observation rather than a nullable object: `{"state":"clear"}`,
`{"state":"halted", at, block, seqNum, episodes, durable}`, or `{"state":"unreadable", reason}`. The
four surfaces the U26 census owed are published on the operator status, each with its own `error` so
a read failure is unknown rather than an empty reading:

- `ownSequenceRecord` (`{record: {height, seqNum, block}, error}`),
- `equivocationEvidence` (`{validators, error}`),
- `rspaceRoot` (`{root, error}`),
- `gatewayTxn` (`{total, inFlight, error}`).

## Runs

```
cargo test --locked -p rchain-casper --test divergence_halt
cargo test --locked -p rchain-casper --lib divergence_halt::
cargo test --locked -p rchain-casper --test store_format_trace
cargo test --locked -p rchain-node --lib api::conversion::tests
cargo test --locked -p rchain-node --lib diagnostics::health_contract
cargo test --locked -p rchain-node --test lean_envelope_corpus
python3 tests/test_recovery_harness_controls.py
```

Results (2026-11-24, `finish/658-round`):

```
casper/tests/divergence_halt.rs                                  8 passed (the 2 controls + 6)
casper lib divergence_halt::                                     1 passed
casper lib api::block_api::                                      8 passed
casper lib proposer::                                            69 passed, 1 ignored
casper lib dag::the_equivocation_evidence_survives_a_restart     1 passed (includes the surface)
casper --test store_format_trace                                  4 passed
node lib api::conversion (incl. two new surface tests)           13 passed
node lib diagnostics::health_contract                              9 passed
node --test lean_envelope_corpus                                   1 passed
python3 tests/test_recovery_harness_controls.py                   63 passed
```

Gates: `check-durable-state-census.py --gate` (18 rows, 0 violations),
`check-hazop-worksheet.sh --gate` (26 nodes, 154 rows, 25 falsifiers owed, 44 node-words),
`check-node-diagnostics.py --gate` (0 violations), `emit-lean-counts.sh --check` (128 laws / 187
entries). `cargo fmt --all` applied; clippy is recorded in the PR run.

## Negative controls

Both controls the review asked for are deterministic tests, and each has a mutation control that must
turn it red:

| control | test | mutation |
|---|---|---|
| fresh cell + injected read error refuses every proposal trigger; the resume is refused; a successful read resolves it | `a_failed_boot_read_stops_every_proposal_trigger_until_a_successful_read` | remove the construction-time/read-failure `seed_unreadable` → the proposer reaches `NotBonded` |
| ack A interleaved with halt B preserves B across restart | `an_acknowledgement_cannot_delete_a_halt_recorded_while_it_reads` | remove `transition` from `record_halt` or `acknowledge_divergence_halt` → the clear deletes B |
| a non-durable registry halt is ackable | `a_non_durable_registry_halt_is_acknowledged_and_cleared` | restore the store-`None` skip → the halt is never cleared |
| a successful read retires unknown | `a_readable_empty_record_resolves_an_unreadable_halt` | skip the successful-read resolve → `Unreadable` persists |

The mutations are not committed.

## Decisions and limits

- **A transition lock, not a generation field.** The store API has no atomic delete-if
  (`KeyValueTypedStore` exposes get/put/delete/contains/to_map/keys/count), so a generation field
  would still be check-then-delete racy; extending every backend with a CAS has no safety benefit for
  a single-writer process. The per-shard lock is the transaction.
- **The acknowledgement is the admin listener plus the block hash**, not a signature. That boundary
  is unchanged from C408 and is not claimed as authenticated beyond the listener.
- **The acting rule is still conservative across shards**: the proposer consults any member's halt,
  so a multi-shard node stops every member's production until the named shard is acknowledged. The
  `DivergenceHaltUnreadable { shard }` status names the shard but does not narrow the rule.
- **No live devnet arm.** The deterministic tests and their mutation controls are the evidence;
  `--merge-divergence-injection` was not shown to produce `unrestorableRecords > 0` deterministically.
- **C408's evidence item 4** said a read failure was "logged as unknown, not as never halted". The
  log was true; the latch was not — the registry stayed empty and the proposer read it as absence.
  This record is that correction, and the C408 record is pinned and not rewritten.
