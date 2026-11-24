# Derivation — the durable-state census (issue #658)

Revision `f9e509038`. One line per Table D row: what it resolved, and the judgment that set its class.
The row's full cells are in `docs/src/spec/failure-hazop.md` U26.

| row | resolves | class decision |
|---|---|---|
| D26-01 | hypothesis 1 — the reset marker (`Rewinding`/`Rewound`) | `auto-recover`: `catchup.rs::run_catchup` clears it once the frontier passes the anchor; the crash-repair test drives boot |
| D26-02 | hypothesis 4 — `own-latest` (Law 122) | `auto-recover`: boot reconciliation calls `clear_own_latest`; the unit test proves the round trip and clear. Surface is owed — the RC-4 own-seq floor has no diagnostic row |
| D26-03 | hypothesis 2 — the autopropose halt | `operator-action`: the record is obeyed at boot and cleared only by `POST /api/v1/autopropose/resume`; no automatic exit exists by design |
| D26-04 | hypothesis 12 — the durable health counters | `operator-action`: three monotone counters; clearing one would falsify the shard's history. The resolution is the operator's response to the counter, not a clear |
| D26-05 | hypothesis 9a — the legacy body-scan marker | `auto-recover`: a crash before the marker rescans on the next boot; the scan's own test proves the adopt-and-mark path |
| D26-06 | hypothesis 3 — the self-replay record | `auto-recover`: the post-catch-up `Recovered` stage clears the latch without an operator; the operator POST is the fallback |
| D26-07 | hypothesis 5 + 9b — the disposition ledger and the v3 copy | `auto-recover`: the re-offer loop and the startup audit settle or retry every row; the legacy copy keys are written once and then never consulted as a decision |
| D26-08 | hypothesis 7 — `block-metadata` (`restore_attempts`, `validation_failed`) | `operator-action`: the attempt cap is absorbing for the process lifetime unless a reset/rewind (off by default) runs; the counter is surfaced only as the §5 gap |
| D26-09 | hypothesis 13a — the finalized-fringe store | `auto-recover`: the recovery-pending marker is cleared when the restored checkpoint is persisted and read back; the read-back test is the control |
| D26-10 | hypothesis 13b — the checkpoint stores | `operator-action`: the served head is monotone and certificates are retained evidence; nothing clears them, and an operator owns any archive |
| D26-11 | hypothesis 13c — the audit stores | `operator-action`: accepted claims and conflicts are retained evidence; the batch test proves the atomic write, not a clear |
| D26-12 | hypothesis 9c — block bodies and orphan bodies | `auto-recover`: a body with no DAG entry is adopted by the re-offer/legacy scan; the orphan-body surface is the report |
| D26-13 | hypothesis 10 — the equivocation evidence DB | `operator-action`: append-only offence evidence; deleting it destroys the only copy. No surface exists yet (owed) |
| D26-14 | hypothesis 11 — the gateway transaction records | `operator-action`: coordinator records reach a terminal state and are never deleted. The row is `vacuous:` on the law because it is node-local, not consensus state |
| D26-15 | hypothesis 14 — RSpace roots, history and cold leaves | `auto-recover`: the root is a single pointer, rewritten by reset and republished at boot; the failed-insert test keeps the root where it was. No surface exists yet (owed) |
| D26-16 | hypothesis 8a — the shard-id marker | `safe-halt`: a directory owned by another shard must refuse to open; removal is an operator act |
| D26-17 | hypothesis 8b — the store-format marker | `safe-halt`: a newer format refuses to open; the version decision is the guard |

## Resolved to an exclusion rather than a row

- `deploy-index`, `deployer-index` — derived indexes with repair/backfill, not recovery states.
- `work-seen-store` — a pruned proposer cache.
- `history-meter` — archive accounting.
- `deploy-pool` — deploy work that self-clears on inclusion/expiry.
- `eval-*` — the evaluator's scratch RSpace.
- `mergeable-channel-cache`, `native-changes-cache`, `reporting-cache` — rebuildable caches.
- `transaction` — a mapping no code opens.

## The four examples in the issue

- **F-U4-01** — the retry path exists at this revision; the missing proof is a contract test (Stage 2).
- **F-U7-01** — closed upstream; Stage 3 closes the HAZOP row against the boundary tests.
- **F-U3-01** — structural; classed `operator-action` with the reconcile route, not automatic recovery
  (Stage 3).
- **`unrestorableRecords`** — the live gap. D26-08 and F-U26-02/03 record it; Stage 4 supplies the
  acting rule.

## Added in Stage 4 (C408)

| row | resolves | class decision |
|---|---|---|
| D26-18 | the divergence halt (`DIVERGENCE_HALT_KEY`) | `operator-action`: `note_unrestorable_if_exhausted` writes the record store-first and notes the registry; `clear_autopropose_halt` requires the refused block hash to clear it. There is no automatic exit, and serving continues |
