# C408 — the `unrestorableRecords` acting rule (F-U26-02, F-U26-03, issue #658)

- **code:** `0c59ff2ab` (branch `feat/658-divergence-halt`, stacked on the census PR #662 and the
  F-U4-01 / row-closure PRs)
- **harness:** `e81cf6790`
- **tests:** `casper/tests/divergence_halt.rs` (4), `casper/src/divergence_halt.rs` (3 unit),
  `tests/test_recovery_harness_controls.py` (62)

## The rule

A spent restore budget is a divergence the node cannot repair without an anchor the network has not
agreed. It now **stops producing and keeps serving**:

1. **Lean first.** Law 76 (`spec/Rchain/Stop.lean`) is amended in place with the safe-halt machine:
   `safeHaltStep` sets `proposing := false` and leaves `serving` unchanged; no safe-halt step restores
   production (`a_safe_halt_has_no_automatic_exit`); only the operator's `operatorResumeStep` does
   (`a_safe_halt_can_be_resumed_by_an_operator`). No new law number.
2. **Durable record.** `DIVERGENCE_HALT_KEY = b"divergence-halt"` in the per-shard control-record
   store, a fixed 56-byte codec (`at_ms`, block hash, seq, episodes), written store-first from
   `note_unrestorable_if_exhausted`. The process registry (`casper/src/divergence_halt.rs`) is noted
   from the record; a failed write still halts and the surface says `durable: false`.
3. **Every trigger.** `Proposer::do_propose` refuses with `ProposeStatus::DivergenceHalted { block,
   seq }` before any creation, so the timer, the tap, the retry and the explicit route all stop.
   `ProposeHealth::park_while_halted` parks the timer. Serving is untouched.
4. **Boot.** `ProposeHealth::seed_divergence_halt_from_store` reads the record before the timer is
   spawned; a read failure is logged as unknown, not as "never halted".
5. **The operator's exit.** `POST /api/v1/autopropose/resume?acknowledgeDivergence=<hex>` requires the
   refused block hash. A missing or different hash is refused and the halt stands; the durable record
   is cleared store-first, then the registry, then the timer is woken.
6. **Store format 5.** A v4 binary refuses the store rather than ignoring the halt and resuming
   production on a divergent view.
7. **Harness.** `tests/consensus-partition-rejoin.py` reports `FAIL_DIVERGENCE_HALTED` naming the
   resume route when the halt stands; an older node's `FAIL_PERMANENT_DIVERGENCE` remains.

## Runs

```
cargo test --locked -p rchain-casper --test divergence_halt          # 4 passed
cargo test --locked -p rchain-casper --lib divergence_halt::         # 3 passed
cargo test --locked -p rchain-casper --lib api::block_api::          # 8 passed
cargo test --locked -p rchain-casper --lib storage::                 # 9 passed
cargo test --locked -p rchain-casper --test store_format_trace       # 4 passed
cargo test --locked -p rchain-node --lib api::conversion::tests      # 11 passed
cargo test --locked -p rchain-node --lib diagnostics::health_contract # 9 passed
cargo test --locked -p rchain-node --test lean_envelope_corpus       # 1 passed
python3 tests/test_recovery_harness_controls.py                      # 62 passed
cargo check --locked --workspace --all-targets                       # clean
```

Formal: `lake build Rchain.Laws` succeeded locally (mathlib packages fetched and cached); the full
`tools/check-lean-conformance.sh` was running when this record was written, and the result is recorded
in the PR. CI's `formal-on-change.yml` re-runs it because `spec/Rchain/**` changed.

## Mutation control

Removing the `divergence_halt::any()` check from `Proposer::do_propose` makes
`a_spent_restore_budget_halts_every_proposal_trigger_and_survives_a_restart` fail with
`NotBonded` where `DivergenceHalted` is required. The mutation is not committed.

## Decisions and limits

- **Store format 5** was chosen over the self-replay precedent: a downgrade must refuse rather than
  silently ignore the halt.
- **The acknowledgement is the admin listener + the block hash**, not a signed acknowledgement. The
  admin listener is loopback-bound by default and `enable-devnet-admin-public` can expose it; a signed
  acknowledgement is a separate change and is not claimed here.
- **The acting decision is conservative across member shards.** The registry is keyed by shard, but a
  proposer has no shard id and consults `any()`; a multi-shard node stops every member's production
  until the named shard is acknowledged. Serving continues on every shard. This is deliberate: a
  divergent shard is a consensus-relevant fault on the node that runs it.
- **No live devnet arm.** `--merge-divergence-injection` was not shown to drive `unrestorableRecords
  > 0` deterministically, so the deterministic tests are the evidence and a live arm is owed.
- The census gained D26-18 for the new key; the emitter now derives 48 identifiers, all classified.
