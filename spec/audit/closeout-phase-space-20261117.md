# C343 / C357 close-out phase space — 2026-11-17

**Rows stay `in progress`.** This is the failure-tell matrix and the witness-audit record, not a
closure. The remaining C343 evidence is the live B/C/D read-only snapshot; the remaining C357
evidence is the section-6 lockstep acceptance and the independent category-A sign-off.

**Base.** `6d490f9a9` (green dev CI run `44143002504`). This PR's Lean unit is `a333d1a64`; the
Rust surface unit is `e38519d51`. Every gate below runs against the PR's base, not the older
evidence pins (`6964e875` for C343, `1ceabd291` for C357).

## Locality vocabulary

| label | meaning |
|---|---|
| `unit` | the named Rust unit test |
| `integration` | a Rust integration test (no Docker) |
| `local-devnet` | a disposable Docker devnet on one host |
| `single-host` | a pinned-binary qualification on one host |
| `live-host` | a read-only snapshot from hosts B/C/D |
| `external` | a lockstep release/independent-review artefact the tree cannot supply |

**A gate that cannot reach its precondition reports `PRECONDITION`, never `PASS` or `FAIL`.** A
fixture that stages its own precondition can fail for staging reasons and say nothing about the
gate; that is the `checkpoint_restore` lesson.

## C343 — the private-chain surface (Law 88a/88b)

| cell | Lean theorem | Rust witness | gate assertion | a failure means | locality |
|---|---|---|---|---|---|
| audit covers every stored body before Running | `audit_covers`, `startup_clean` | `the_legacy_scan_adopts_every_unaccounted_body_and_writes_its_marker` | `orphanBodies == 0` after the drain | a body reached Running unaccounted; 88a violated | unit + local-devnet |
| the scan marks after the walk | `the_scan_audits_before_marking`, `a_scanned_state_holds_no_orphan` | same test (marker assertion and second-pass no-op) | restart with the marker; no re-adoption, no orphan | marker written before the walk | unit |
| a held body in any stage is visible | `the_aggregate_surface_sees_every_held_body` | `the_queue_sample_reports_every_stage`, `health_reports_readiness_and_pool_capacity` | `blockPipeline.{validatedQueue,incomingQueue}` are numbers; a held body moves one | the surface is `blocks_st`-only; F-U19-11 still open | unit + local-devnet |
| an outstanding fault reports Blocked | `an_outstanding_fault_reports_blocked`, `an_outstanding_fault_blocks_admission` | `an_outstanding_pipeline_fault_reports_blocked`; deploy route `pipeline_faults_outstanding() > 0` (existing); proposer gate (existing) | `/health.phase == "blocked"`, deploy 503 `"phase":"blocked"`, propose refused | the surface disagrees with the gate; a running cell serves work during a fault | unit (phase) + integration (route/gate) |
| a settled fault returns the cell phase | `a_settled_fault_returns_the_cell_phase` | same | `/health.phase` returns to `running`; admission resumes | the fault gate sticks after the verdict settles | unit |
| a shard is gated by its own cell | `a_shard_is_gated_only_by_its_own_cell`, `a_syncing_shard_is_refused_whatever_the_others_do` | `a_syncing_cell_does_not_propose`, `two_cells_hold_independent_phases` | a syncing shard's proposer adds no block | the gate read the wrong phase cell | unit |
| readiness is every shard running | `ready_iff_every_shard_running` | `health_is_ready_only_when_every_shard_runs` | a non-running shard makes `ready:false` and names the reason | the readiness conjunction was dropped | unit + local-devnet (single-shard caveat) |
| the bound gate | `gate_admits_only_in_running_and_within_bound` | `the_proposal_finality_bound_passes_at_the_bound_and_refuses_beyond_it` | at lag 32, `proposalHalted`; an explicit propose adds no block | a private chain is admitted | unit + local-devnet |
| the operator route reads the gate | `gate_refuses_outside_running` | `the_operator_status_carries_every_readiness_gate` | operator status names the halt; propose refused | a second trigger bypasses the gate | unit |
| resume re-checks | `withinProposalFinalityBound` | the isolation drill's resume arm | resume clears; the next propose re-halts while finality is frozen | resume admits beyond the bound | local-devnet |

If the devnet is single-shard, the per-shard cells are unit-witnessed only; the table says so and
the live B/C/D snapshot is where the real phases are read.

## C357 — the self-replay recovery surface (Laws 11, 92, 94, 95a/95b)

| cell | Lean theorem | Rust witness | gate assertion | a failure means | locality |
|---|---|---|---|---|---|
| provenance decides the cause | `law92_statement`, `a_self_replay_failure_is_not_divergence` | `a_self_built_replay_failure_is_classified_as_a_local_fault` | the natural trigger is a local fault, never divergence | the divergence path was taken; Law 92 violated | unit + local-devnet |
| one-shot attestation, no user deploy | `the_attest_only_rebuild_has_no_user_deploy`, `the_refused_deploy_is_not_in_the_rebuild` | `a_self_replay_failure_is_bounded_and_not_counted_as_divergence` | one rebuild; its block has no user deploy; the refused set is excluded at the refusing pre-state | the payload retained the deploy or the same input was rebuilt | unit + local-devnet |
| no unchanged retry | `no_unchanged_retry`, `a_retry_of_the_same_key_is_not_a_new_attempt` | same test (one refusal, one different input) | the same `(preState, deploys, code)` is attempted once | the 45-hour loop is representable | unit |
| the durable record precedes the halt | `an_observable_halt_has_a_durable_record` | `a_self_replay_fault_raises_one_reset_and_records_the_halt` (record order `[Refused, Halted]`) | the record exists at the halt and survives a restart | the transition was observable without a record | unit + integration |
| every trigger reads the latch | `a_halted_proposer_admits_no_trigger` | `a_self_replay_failure_with_no_pool_deploy_is_counted`, `a_self_replay_halt_is_read_by_the_gates_and_cleared_by_the_resume` | autopropose, admin propose and deploy propose all refuse | an uncovered trigger produces a block | unit + local-devnet |
| the reset anchor is finalised, once | `the_reset_anchor_is_finalised`, `at_most_one_reset_per_anchor` | `a_reset_at_an_anchor_already_attempted_is_refused`, `a_self_replay_fault_raises_one_reset_and_records_the_halt` | the reset marker's anchor is finalised; a second reset at it is refused | a reset below the anchor or a reset storm | unit + local-devnet |
| resume waits for catch-up | `resume_requires_catch_up`, `resume_requires_a_finalised_anchor` | `a_self_replay_reset_below_its_anchor_does_not_resume`, `a_caught_up_self_replay_reset_resumes_without_an_operator_post` | a resume before the frontier passes the anchor is refused | resume without catch-up | integration + local-devnet |
| replay order and cost | Law 95a/95b chronological theorems | `pending_peeks_replay_with_exact_cost_and_state` | strict replay at 1860/2606/4844 and the declared post-state | the newest-first build; `ReplayCostMismatch` | integration + local-devnet |
| the lockstep boundary | — | — | selected release, every validator in lockstep (or a coordinated new genesis), independent category-A verdict | mixed-version rollout or an unsigned record | external |

## Witness audit — 2026-11-17

Each row applied a local mutant (not committed), ran the named test, and required it to fail. A
test that passes under its mutant is a tautology or a fixture that does not reach the gate.

| law | mutant applied | witness | result |
|---|---|---|---|
| 92 | `ValidationOrigin::SelfBuilt => Refusal::InvalidState` | `a_self_built_replay_failure_is_classified_as_a_local_fault` | **FAILED (required)** |
| 88a | skip every key in `scan_unaccounted_bodies` (`if true { continue; }`) | `the_legacy_scan_adopts_every_unaccounted_body_and_writes_its_marker` | **FAILED (required)** |
| 88a | drop `validatedQueue`/`incomingQueue` from `blockPipeline` | `health_reports_readiness_and_pool_capacity` | **FAILED (required)** |
| 88b | `within_proposal_finality_bound` returns `true` | `the_proposal_finality_bound_passes_at_the_bound_and_refuses_beyond_it` | **FAILED (required)** |
| 88b | proposer phase check is `if false` | `a_syncing_cell_does_not_propose` | **FAILED (required)** |
| 88b | `all_running` uses `.any` instead of `.all` | `health_is_ready_only_when_every_shard_runs` | **FAILED (required)** |
| 94 | the self-replay latch check is `if false` | `a_self_replay_failure_with_no_pool_deploy_is_counted` | **FAILED (required)** |
| 94 | the recovery build uses `ProposalMode::Normal` | `a_self_replay_failure_is_bounded_and_not_counted_as_divergence` | **FAILED (required)** |
| 94 | `self_replay_resume_is_due` returns `true` | `a_self_replay_reset_below_its_anchor_does_not_resume` | **FAILED (required)** |
| 94 | `reset_is_recurrence` returns `false` | `a_reset_at_an_anchor_already_attempted_is_refused` | **FAILED (required)** |
| 95 | remove `.rev()` from `build_replay_data` | `pending_peeks_replay_with_exact_cost_and_state` | **FAILED (required)**: `ReplayCostMismatch { 4844, 4554 }` |

Direct assertions not mutated: `two_cells_hold_independent_phases` (cell independence),
`the_phase_stays_starting_until_the_drain_closes` (drain before Running), the record-order
assertion in `a_self_replay_fault_raises_one_reset_and_records_the_halt`, and the pace-rule test
`an_attestation_only_build_is_not_withheld_by_the_pace_rule`.

## External contracts

### C343-LIVE — read-only, hosts B/C/D

Collect `GET /health` (`ready`, `readyReasons`, `phase`, `nodePhase`, `shardPhases`,
`pipelineFaults`, `pipelineFaultsOutstanding`, `lastPipelineFault`, `orphanBodies`,
`orphanDetections`, `proposalHalted`, `proposalHaltReason`, `finalizedLag`, `blockPipeline`),
`GET /api/v1/status/operator` (the seven gate keys, `syncPhase`, `sync`, `resetMarker`,
`finalityStalled`, `shards`), and the block-6 journal lines. The verifier checks the phase
discriminator (`syncPhase`/`phase`), the disposition of the block-6 body, and the height gap.

The old hosts may still run the pre-fix build: this snapshot is the incident's phase diagnosis,
not a current-build gate. The current-build evidence is the local/integration arm above.

### C357-ACCEPT — section 6

One of exactly two boundary shapes: (1) unswept history → a coordinated new genesis on the
selected corrected build, every validator in lockstep; or (2) existing-chain continuation → an
independently accepted retained-history sweep through the agreed checkpoint plus coordinated
activation. A mixed-version rollout is refused. The record needs a digest-pinned campaign, the
release owner's selected source/artifact/binary, and an independent reviewer's published verdict.
`tools/check-hardfork-51-closeout.py --assert-closeable` stays exit `3` until those exist.

## Register state

C343 and C357 remain `in progress`. Their `owes` cells name only the external artefacts above;
the local/code-side blockers are closed by this PR.
