import Rchain.Laws.Base

/-!
# The law register, chunk 07 (laws 88–99)

One module of the register catalog. The register is split across modules because elaborating it as a
single module peaked at ~15 GiB on the 16 GiB CI runner (measured 2026-11-23); a module boundary is a
process boundary, so each chunk's elaboration memory is bounded. The register's numbering check
(`1..lawCeiling`) and `entryCeiling` make a dropped or duplicated row across chunk boundaries a build
failure. See `Rchain/Laws.lean` for the assembly and the measurement.
-/

set_option autoImplicit false
set_option maxHeartbeats 4000000

namespace Rchain
namespace Laws

def lawsChunk07 : List Law := [
  { number := 88, clause := "a", layer := "Disposition",
    statement := "**A stored body is accounted for or visible.** A body in the block store is \
      either a DAG block (its metadata is the durable verdict) or carries an unsettled \
      `Pending`/`Faulted` row; the start-up audit adopts every unaccounted body before the phase \
      becomes `Running`, so that transition cannot skip a stored body, and the scan writes its \
      durable marker only after the walk. The law says **visible**, not **settled**: a body whose \
      justifications are not in the DAG stays `Pending` and visible, and the bounded start-up \
      drain does not wait for it. Visibility is the **aggregate** ingest backlog: a body held in \
      any of the `blocks_st`, validated or incoming stages is seen, not only the capped queue. \
      An outstanding pipeline fault is a fail-closed `Blocked` phase: the reported phase, deploy \
      admission and the proposal gate all refuse until the fault settles, and the shard phase cell \
      keeps its single writer. The deploy route is the cell's own admission answer conjoined with \
      the three latches the Rust route checks (pipeline fault, proposal isolation, orphan body); \
      `Starting` admits work, and the proposer's wait is a separate `PhaseDecision`",
    status := .provedModel,
    formal := `Rchain.Disposition.law88a_statement,
    mutant := `Rchain.Disposition.law88a_mutant,
    falsifier := `Rchain.Disposition.law88a_falsifier,
    declarations := [`Rchain.Disposition.State,
      `Rchain.Disposition.Disposition, `Rchain.Disposition.audit,
      `Rchain.Disposition.adoptBody, `Rchain.Disposition.startup,
      `Rchain.Disposition.audit_covers,
      `Rchain.Disposition.audit_makes_an_unbacked_body_visible,
      `Rchain.Disposition.audit_does_not_change_dag,
      `Rchain.Disposition.startup_clean,
      `Rchain.Disposition.startup_without_audit_leaves_an_orphan,
      `Rchain.Disposition.a_dropped_row_is_an_orphan,
      `Rchain.Disposition.scan,
      `Rchain.Disposition.the_scan_audits_before_marking,
      `Rchain.Disposition.a_scanned_state_holds_no_orphan,
      `Rchain.Disposition.markedWithoutAudit,
      `Rchain.Disposition.a_marker_without_the_audit_leaves_an_orphan,
      `Rchain.Disposition.Stage,
      `Rchain.Disposition.aggregate,
      `Rchain.Disposition.blocksStOnlySurface,
      `Rchain.Disposition.the_aggregate_surface_sees_every_held_body,
      `Rchain.Disposition.a_body_held_in_a_validated_queue_is_invisible_to_a_blocks_st_only_surface,
      `Rchain.Disposition.effectivePhase,
      `Rchain.Disposition.admitsWork,
      `Rchain.Disposition.rawPhaseOnly,
      `Rchain.Disposition.admitsWorkRaw,
      `Rchain.Disposition.proposalGate,
      `Rchain.Disposition.an_outstanding_fault_reports_blocked,
      `Rchain.Disposition.a_settled_fault_returns_the_cell_phase,
      `Rchain.Disposition.an_outstanding_fault_blocks_admission,
      `Rchain.Disposition.an_outstanding_fault_refuses_the_proposal_gate,
      `Rchain.Disposition.a_raw_phase_admits_work_during_an_outstanding_fault,
      `Rchain.Disposition.law88a_mutant,
      `Rchain.Disposition.law88a_falsifier,
      `Rchain.Disposition.law88a_statement,
      `Rchain.Disposition.law88a,
      `Rchain.Disposition.cellAdmitsWork, `Rchain.Disposition.targetAdmits, `Rchain.Disposition.routeAdmits, `Rchain.Disposition.a_starting_cell_admits_work, `Rchain.Disposition.an_unknown_target_skips_the_cell_check, `Rchain.Disposition.the_deploy_route_is_the_cell_and_the_latch, `Rchain.Disposition.an_isolated_cell_is_refused, `Rchain.Disposition.an_orphan_body_refuses_admission
      ],
    axioms := [],
    rust := ["casper/src/block_disposition.rs", "casper/src/engine/node_launch.rs",
      "node/src/runtime/node_runtime.rs", "casper/src/dag.rs", "shared/src/typed_store.rs"],
    rustWitness := ["shared/src/typed_store.rs:keys_decodes_only_the_key_and_survives_an_undecodable_value",
      "casper/src/block_disposition.rs:the_legacy_scan_adopts_every_unaccounted_body_and_writes_its_marker",
      "casper/src/block_disposition.rs:a_stored_body_without_a_dag_entry_is_reoffered",
      "casper/src/block_disposition.rs:the_phase_stays_starting_until_the_drain_closes"],
    witness := [`Rchain.Disposition.audit_covers,
      `Rchain.Disposition.audit_makes_an_unbacked_body_visible,
      `Rchain.Disposition.audit_does_not_change_dag,
      `Rchain.Disposition.startup_clean,
      `Rchain.Disposition.startup_without_audit_leaves_an_orphan,
      `Rchain.Disposition.a_dropped_row_is_an_orphan,
      `Rchain.Disposition.the_scan_audits_before_marking,
      `Rchain.Disposition.a_scanned_state_holds_no_orphan,
      `Rchain.Disposition.a_marker_without_the_audit_leaves_an_orphan,
      `Rchain.Disposition.the_aggregate_surface_sees_every_held_body,
      `Rchain.Disposition.a_body_held_in_a_validated_queue_is_invisible_to_a_blocks_st_only_surface,
      `Rchain.Disposition.an_outstanding_fault_reports_blocked,
      `Rchain.Disposition.a_settled_fault_returns_the_cell_phase,
      `Rchain.Disposition.an_outstanding_fault_blocks_admission,
      `Rchain.Disposition.an_outstanding_fault_refuses_the_proposal_gate,
      `Rchain.Disposition.a_raw_phase_admits_work_during_an_outstanding_fault,
      `Rchain.Disposition.law88a_falsifier,
      `Rchain.Disposition.a_starting_cell_admits_work, `Rchain.Disposition.an_unknown_target_skips_the_cell_check, `Rchain.Disposition.the_deploy_route_is_the_cell_and_the_latch, `Rchain.Disposition.an_isolated_cell_is_refused, `Rchain.Disposition.an_orphan_body_refuses_admission
      ],
    falsifiable := "`skipAudit` flips the phase without adopting: \
      `startup_without_audit_leaves_an_orphan` exhibits the stored body 7 with no DAG entry and no \
      row, which is the live incident's shape. Deleting a row without a DAG record \
      (`a_dropped_row_is_an_orphan`) is the other defect shape; an audit that removed rows instead \
      of adding them fails `audit_covers`. The marker without the walk \
      (`a_marker_without_the_audit_leaves_an_orphan`), the blocks-st-only surface \
      (`a_body_held_in_a_validated_queue_is_invisible_to_a_blocks_st_only_surface`) and the raw \
      phase that admits during a fault (`a_raw_phase_admits_work_during_an_outstanding_fault`) are \
      the three added defect shapes.",
    note := "Node-local durable ingestion, not consensus state, so `proved-model` rather than a \
      corpus-tied row: there is no wire or derived value to tie. This row promotes C343's \
      `candidate:block-disposition-and-isolation`. The model assumes readable bodies; the Rust scan \
      skips a body whose bytes do not decode, that body remains an orphan, and the periodic \
      stored-body count audit (`orphanBodies`) withholds readiness. The bounded drain is a \
      liveness policy, not a state predicate: `Running` follows an audit attempt, and a \
      missing-parent row stays `Pending` by design." },

  { number := 88, clause := "b", layer := "Disposition",
    statement := "**A proposal is admitted only in `Running` and within the finality bound.** The \
      gate refuses outside the running phase and one height beyond `MAX_PROPOSAL_FINALITY_LAG`, \
      which is what bounds a private chain when no peer accepts it. The gate is **total in the \
      anchor**: a view with no finalised block measures from genesis, so `no anchor` is not `no \
      bound`. The phase is per-shard: a shard's proposal gate reads only its own cell. Readiness is \
      the Rust registry's: the primary cell defaults to `Starting` when absent, a member is running \
      only when its own cell is present and `Running`, and an empty member list is governed by the \
      primary rather than vacuously true. `Starting` is a 120-second wait, not an immediate refusal",
    status := .provedModel,
    formal := `Rchain.Disposition.law88b_statement,
    mutant := `Rchain.Disposition.law88b_mutant,
    falsifier := `Rchain.Disposition.law88b_falsifier,
    declarations := [`Rchain.Disposition.Phase,
      `Rchain.Disposition.MAX_PROPOSAL_FINALITY_LAG,
      `Rchain.Disposition.withinProposalFinalityBound,
      `Rchain.Disposition.gate, `Rchain.Disposition.Refusal,
      `Rchain.Disposition.GateResult,
      `Rchain.Disposition.gate_admits_only_in_running_and_within_bound,
      `Rchain.Disposition.gate_admits_at_the_bound,
      `Rchain.Disposition.gate_refuses_beyond_the_bound,
      `Rchain.Disposition.gate_refuses_outside_running,
      `Rchain.Disposition.a_gate_without_the_phase_admits_a_syncing_node,
      `Rchain.Disposition.a_gate_without_the_bound_admits_a_private_chain,
      `Rchain.Disposition.finalizedFloor, `Rchain.Disposition.totalGate,
      `Rchain.Disposition.floor_never_exceeds_the_anchor,
      `Rchain.Disposition.skipWithoutAnchor,
      `Rchain.Disposition.a_gate_that_skips_the_bound_without_an_anchor_admits_a_private_chain,
      `Rchain.Disposition.a_view_without_a_finalized_block_is_still_bounded,
      `Rchain.Disposition.the_anchor_is_passed_through,
      `Rchain.Disposition.ShardPhases,
      `Rchain.Disposition.ready,
      `Rchain.Disposition.shardGate,
      `Rchain.Disposition.a_shard_is_gated_only_by_its_own_cell,
      `Rchain.Disposition.a_syncing_shard_is_refused_whatever_the_others_do,
      `Rchain.Disposition.ready_iff_every_shard_running,
      `Rchain.Disposition.anyShardRunning,
      `Rchain.Disposition.a_readiness_that_accepts_any_shard_admits_a_syncing_node,
      `Rchain.Disposition.law88b_mutant,
      `Rchain.Disposition.law88b_falsifier,
      `Rchain.Disposition.law88b_statement,
      `Rchain.Disposition.law88b,
      `Rchain.Disposition.primaryPhase, `Rchain.Disposition.memberRunning, `Rchain.Disposition.primary_phase_defaults_to_starting, `Rchain.Disposition.an_empty_member_list_is_governed_by_the_primary, `Rchain.Disposition.an_empty_member_list_with_a_running_primary_is_ready, `Rchain.Disposition.an_empty_member_list_with_a_starting_primary_is_not_ready, `Rchain.Disposition.an_absent_member_is_not_running, `Rchain.Disposition.an_unknown_shard_is_not_gated_by_another_shards_cell, `Rchain.Disposition.readyIgnoringPrimary, `Rchain.Disposition.a_readiness_that_ignores_the_primary_admits_an_empty_registry, `Rchain.Disposition.absentMemberRunning, `Rchain.Disposition.an_absent_member_treated_as_running_is_not_ready, `Rchain.Disposition.PhaseDecision, `Rchain.Disposition.phaseDecisionWait, `Rchain.Disposition.proposeAdmission, `Rchain.Disposition.a_starting_proposer_waits_then_refuses, `Rchain.Disposition.a_running_proposer_is_admitted, `Rchain.Disposition.a_syncing_proposer_is_refused, `Rchain.Disposition.the_propose_decision_is_total, `Rchain.PhaseDiagnosis.PhaseObservation, `Rchain.PhaseDiagnosis.Diagnosis, `Rchain.PhaseDiagnosis.phaseDiagnosisTable, `Rchain.PhaseDiagnosis.diagnose, `Rchain.PhaseDiagnosis.the_incident_is_proposal_isolation, `Rchain.PhaseDiagnosis.a_processor_fault_is_blocked, `Rchain.PhaseDiagnosis.a_syncing_host_is_recovery, `Rchain.PhaseDiagnosis.a_healthy_snapshot_is_healthy, `Rchain.PhaseDiagnosis.an_unknown_shape_is_unmodelled
      ],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs",
      "node/src/instances/proposer_instance.rs", "casper/src/engine/node_phase.rs",
      "node/src/web/http.rs"],
    rustWitness := ["casper/src/blocks/proposer/proposer.rs:the_proposal_finality_bound_passes_at_the_bound_and_refuses_beyond_it",
      "casper/src/blocks/proposer/proposer.rs:a_view_without_a_finalized_block_still_trips_the_bound",
      "casper/src/engine/node_phase.rs:two_cells_hold_independent_phases",
      "node/src/instances/proposer_instance.rs:a_syncing_cell_does_not_propose",
      "node/src/web/http.rs:health_is_ready_only_when_every_shard_runs"],
    witness := [`Rchain.Disposition.gate_admits_only_in_running_and_within_bound,
      `Rchain.Disposition.gate_admits_at_the_bound,
      `Rchain.Disposition.gate_refuses_beyond_the_bound,
      `Rchain.Disposition.gate_refuses_outside_running,
      `Rchain.Disposition.a_gate_without_the_phase_admits_a_syncing_node,
      `Rchain.Disposition.a_gate_without_the_bound_admits_a_private_chain,
      `Rchain.Disposition.a_gate_that_skips_the_bound_without_an_anchor_admits_a_private_chain,
      `Rchain.Disposition.a_view_without_a_finalized_block_is_still_bounded,
      `Rchain.Disposition.a_shard_is_gated_only_by_its_own_cell,
      `Rchain.Disposition.a_syncing_shard_is_refused_whatever_the_others_do,
      `Rchain.Disposition.ready_iff_every_shard_running,
      `Rchain.Disposition.a_readiness_that_accepts_any_shard_admits_a_syncing_node,
      `Rchain.Disposition.law88b_falsifier,
      `Rchain.Disposition.primary_phase_defaults_to_starting, `Rchain.Disposition.an_empty_member_list_is_governed_by_the_primary, `Rchain.Disposition.an_empty_member_list_with_a_running_primary_is_ready, `Rchain.Disposition.an_empty_member_list_with_a_starting_primary_is_not_ready, `Rchain.Disposition.an_absent_member_is_not_running, `Rchain.Disposition.an_unknown_shard_is_not_gated_by_another_shards_cell, `Rchain.Disposition.a_readiness_that_ignores_the_primary_admits_an_empty_registry, `Rchain.Disposition.an_absent_member_treated_as_running_is_not_ready, `Rchain.Disposition.a_starting_proposer_waits_then_refuses, `Rchain.Disposition.a_running_proposer_is_admitted, `Rchain.Disposition.a_syncing_proposer_is_refused, `Rchain.Disposition.the_propose_decision_is_total, `Rchain.PhaseDiagnosis.the_incident_is_proposal_isolation, `Rchain.PhaseDiagnosis.a_processor_fault_is_blocked, `Rchain.PhaseDiagnosis.a_syncing_host_is_recovery, `Rchain.PhaseDiagnosis.a_healthy_snapshot_is_healthy, `Rchain.PhaseDiagnosis.an_unknown_shape_is_unmodelled
      ],
    falsifiable := "`legacyGate` admits a syncing node \
      (`a_gate_without_the_phase_admits_a_syncing_node`) and `unboundedGate` admits a tip beyond \
      the bound (`a_gate_without_the_bound_admits_a_private_chain`). A gate that reads the wrong \
      phase, or drops the bound, fails those rows. **And the third shape, which the Rust had** \
      (C351): `skipWithoutAnchor` answers `.allow` for every tip when the view has no fringe, which \
      `a_gate_that_skips_the_bound_without_an_anchor_admits_a_private_chain` exhibits; \
      `a_view_without_a_finalized_block_is_still_bounded` is total, and \
      `a_view_without_a_finalized_block_still_trips_the_bound` pins that the Rust now answers it \
      the same way. The any-shard readiness is the third added defect shape \
      (`a_readiness_that_accepts_any_shard_admits_a_syncing_node`).",
    note := "The bound is policy (`32`), not a measured optimum, and the Rust split is: the phase \
      gate in `proposer_instance`, the height gate in `proposer.rs`. This row promotes C343's \
      `candidate:block-disposition-and-isolation`. **The anchor is total since C351**: the gate was \
      wrapped in `if let Some(finalized)`, so a view with no finalised block skipped both the bound \
      and the isolation latch. Genesis is the floor (`finalized_floor`), which is the answer \
      `RewindPlan` already gives and the one `finalizedFloor` states; the model took a plain `Nat` \
      all along, so the Rust was the outlier. Normal operation is unchanged (at genesis the gap is \
      1 of 32). The model does not prove a peer accepted the chain — it bounds the private chain a \
      node can build; the finality algorithm and slashing are outside it." },

  { number := 88, clause := "c", layer := "Disposition",
    statement := "**An overflow refusal is a deferral, not an orphan.** The receiver stores the \
      body and writes the `Pending` row before it can refuse for capacity, so the refusal changes \
      no row: the obligation survives by construction. A runtime sweep re-offers an aged `Stored` \
      row once every justification is in the DAG; a row already in flight (`Validating`/`Retry`) is \
      never eligible; a held body's claim is taken once; and a re-offered row moves to `Retry`, so \
      a second sweep dispatches nothing",
    status := .provedTied,
    corpus := some "recovery",
    declarations := [`Rchain.Disposition.Recovery.Row, `Rchain.Disposition.Recovery.eligible,
      `Rchain.Disposition.Recovery.refuse,
      `Rchain.Disposition.Recovery.an_overflow_refusal_keeps_the_row,
      `Rchain.Disposition.Recovery.Receiver, `Rchain.Disposition.Recovery.claim,
      `Rchain.Disposition.Recovery.a_claim_is_taken_once,
      `Rchain.Disposition.Recovery.reoffer,
      `Rchain.Disposition.Recovery.reoffer_is_not_eligible,
      `Rchain.Disposition.Recovery.the_age_floor_gates_eligibility,
      `Rchain.Disposition.Recovery.an_inflight_row_is_not_eligible],
    axioms := [],
    rust := ["casper/src/block_disposition.rs", "casper/src/blocks/block_receiver.rs",
      "node/src/runtime/node_runtime.rs"],
    rustWitness := ["casper/src/block_disposition.rs:a_refused_stored_body_is_reoffered_by_the_runtime_sweep",
      "casper/src/block_disposition.rs:a_pending_body_waits_for_its_parent",
      "casper/src/block_disposition.rs:the_sweep_is_idempotent",
      "casper/src/blocks/block_receiver.rs:a_reoffer_claim_is_taken_once_and_never_steals_an_inflight_block"],
    witness := [`Rchain.Disposition.Recovery.an_overflow_refusal_keeps_the_row,
      `Rchain.Disposition.Recovery.a_claim_is_taken_once,
      `Rchain.Disposition.Recovery.reoffer_is_not_eligible,
      `Rchain.Disposition.Recovery.the_age_floor_gates_eligibility,
      `Rchain.Disposition.Recovery.an_inflight_row_is_not_eligible],
    falsifiable := "Removing the age floor makes the sweep race the receiver's own release and the \
      boot sweep (`the_age_floor_gates_eligibility` flips); dropping the `Validating`/`Retry` \
      exclusion makes a second sweep duplicate an in-flight body (`an_inflight_row_is_not_eligible`, \
      `the_sweep_is_idempotent`); dropping the claim lets the sweep and the receiver dispatch the \
      same held body (`a_reoffer_claim_is_taken_once...`); and a sweep that does not move the row \
      in flight fails `a_refused_stored_body_is_reoffered_by_the_runtime_sweep`'s `Retry` \
      assertion.",
    note := "The code was adopted from candidate `ac41fe7d`/`4d9883a4`/`20ef2505` and re-homed on \
      #499's `NodePhaseCell` (the candidate calls the pre-#499 free `wait_until_running`), with the \
      receiver claim added: the candidate's sweep could race the receiver's own release, because the \
      in-memory state is shared through the `Arc<Mutex<…>>`. **Liveness only** — the sweep dispatches \
      the same `block_processor` validation path as the boot audit, and no validity, state-hash or \
      wire rule changes. The model abstracts the store I/O, the missing-body arm and the queue. \
      Named residual: a just-released block is briefly neither held nor in the DAG; the 60 s floor \
      makes the window small, and the restart-under-load arm is the place to look for duplicate \
      proposals before adding an in-flight hash set." },

  { number := 88, clause := "d", layer := "Disposition",
    statement := "**An unresolvable justification is named, not silently passed into an \
      unclassified fault.** The regression check cannot be evaluated when a justification is absent \
      from the DAG message map; the current rule **passes** (the Scala's own `None`), and that \
      verdict is deliberate — only a detected regression refuses. What the outcome must carry is its \
      name: `Unresolved` is distinct from `Pass`, `NoOwnJustification` from both, and a later \
      height/sequence failure is recorded under the stage `missing justification`, not \
      `validation internal`",
    status := .provedTied,
    corpus := some "recovery",
    declarations := [`Rchain.Disposition.Recovery.Outcome,
      `Rchain.Disposition.Recovery.classify, `Rchain.Disposition.Recovery.refuses,
      `Rchain.Disposition.Recovery.outcomeName,
      `Rchain.Disposition.Recovery.unresolved_does_not_refuse,
      `Rchain.Disposition.Recovery.regression_refuses,
      `Rchain.Disposition.Recovery.pass_does_not_refuse],
    axioms := [],
    rust := ["casper/src/validate.rs", "casper/src/blocks/block_processor.rs"],
    rustWitness := ["casper/src/validate.rs:an_unknown_justification_passes_the_regression_check_and_is_named",
      "casper/src/blocks/block_processor.rs:a_missing_justification_is_recorded_under_its_own_stage"],
    witness := [`Rchain.Disposition.Recovery.unresolved_does_not_refuse,
      `Rchain.Disposition.Recovery.regression_refuses,
      `Rchain.Disposition.Recovery.pass_does_not_refuse],
    falsifiable := "Making `Unresolved` a refusal flips `unresolved_does_not_refuse` and the \
      corpus `classify false true unresolved` row — and is a consensus change, which is why the test \
      `an_unknown_justification_passes_the_regression_check_and_is_named` pins the passing verdict. \
      Folding the reason back into `validation internal` fails \
      `a_missing_justification_is_recorded_under_its_own_stage`.",
    note := "The verdicts are **unchanged**: a block that passed the regression check still passes \
      it, and one that failed still refuses with `JustificationRegression`. The later failure is the \
      height/sequence check's `Err(\"missing justification <hex>\")`, which the processor now records \
      under its own stage; the retry semantics are the same durable `Faulted` row. This is \
      classification and observability, not a validity rule — a design pass corrected an earlier \
      reading that called it a live drop (the processor already records and retries the fault)." },

  { number := 89, layer := "PoS",
    statement := "**Liveness enforcement is jail first; only a vote reaches the bond.** A validator \
      absent across advancing epoch boundaries is removed from the active set for `J` boundaries and \
      paid nothing, while its bond and its delegators' principal are untouched. A stake-weighted \
      vote of strictly more than two thirds of the active stake, opened only against an already-jailed \
      validator, may slash it at the configured tier and permanently eject it; a passing vote closes \
      the target's window and clears its ballots, and a banished validator cannot rebond. **Presence \
      is derived, not fed:** a validator is present at a boundary when it authored a finalised-fringe \
      block **or signed a deploy included in one**, at the member's height; `participationMap` takes \
      the maximum height per validator. The boundary's quorum tip is the greatest presence height \
      whose active supporting stake is strictly more than two thirds of the active total; absence is \
      no entry or strictly more than the threshold behind the tip. A stalled boundary \
      (`tip <= anchor`), an off-boundary block, or a zero tip changes nothing. A jailed validator \
      leaves `active`, so the vote denominator excludes it (the exact-threshold arm's denominator is \
      900, not 1000); release re-enters it. The jail and expiry paths never write `pool` or \
      `withdrawals`; only the tally and the slash do, and only for a target whose vote passed. The \
      authors-only presence is the mutant (`presentFromSenders`), under which every jail is \
      permanent. **Amended 2026-11-22: the inactivity leak.** A validator that *speaks* but whose \
      message does not cover the finality partition (`NoAdvance::Support.not_full`) is the one \
      keeping a layer from filling, and the jail cannot reach it (it is present). After \
      `liveness-jail-after-boundaries` consecutive boundaries of that silence the boundary \
      **retires** it: the validator leaves the pool and the active snapshot and its bond is \
      forfeited to the Coop vault (`SlashSeverity::Malicious` through `slash`). `leak active silent` \
      retires exactly the named senders, never raises the denominator, and makes the bond-leave \
      layer advance: 100/100/50/50 with sender 1 not-full refuses at 200 of 300 — exactly two \
      thirds — and advances at 200 of 200 once sender 1 is leaked. `Coverage.missing` (absence by \
      height) is deliberately not the trigger; the jail remains its response.",
    status := .provedModel,
    formal := `Rchain.law89_with_leak_statement,
    mutant := `Rchain.Liveness.law89_mutant,
    falsifier := `Rchain.Liveness.law89_falsifier,
    declarations := [`Rchain.Liveness.State,
      `Rchain.Liveness.Params,
      `Rchain.Liveness.stakeOf,
      `Rchain.Liveness.jailed,
      `Rchain.Liveness.totalStake,
      `Rchain.Liveness.forStake,
      `Rchain.Liveness.passes,
      `Rchain.Liveness.setNat,
      `Rchain.Liveness.heights,
      `Rchain.Liveness.participationMap,
      `Rchain.Liveness.presentFromAuthorsAndDeployers,
      `Rchain.Liveness.present,
      `Rchain.Liveness.presentFromSenders,
      `Rchain.Liveness.gatedSenders,
      `Rchain.Liveness.isAbsent,
      `Rchain.Liveness.atOrAbove,
      `Rchain.Liveness.support,
      `Rchain.Liveness.isQuorumHeightBool,
      `Rchain.Liveness.isQuorumHeight,
      `Rchain.Liveness.candidateHeights,
      `Rchain.Liveness.quorumTip,
      `Rchain.Liveness.heights_mem,
      `Rchain.Liveness.participationMap_some,
      `Rchain.Liveness.participationMap_mem,
      `Rchain.Liveness.presence_is_the_max_participation_height,
      `Rchain.Liveness.present_of_mem,
      `Rchain.Liveness.a_deploy_signer_can_present_a_jailed_validator,
      `Rchain.Liveness.a_jailed_validator_is_never_present,
      `Rchain.Liveness.absence_is_missing_or_behind_the_threshold,
      `Rchain.Liveness.presence_is_not_absent,
      `Rchain.Liveness.exactly_the_threshold_is_present,
      `Rchain.Liveness.quorum_tip_is_the_greatest_height,
      `Rchain.Liveness.advances,
      `Rchain.Liveness.countedAbsent,
      `Rchain.Liveness.newlyJailed,
      `Rchain.Liveness.jailNew,
      `Rchain.Liveness.releaseTarget,
      `Rchain.Liveness.renewTarget,
      `Rchain.Liveness.expireTarget,
      `Rchain.Liveness.expireAll,
      `Rchain.Liveness.closeWindow,
      `Rchain.Liveness.decrementWindow,
      `Rchain.Liveness.ageWindow,
      `Rchain.Liveness.ageAll,
      `Rchain.Liveness.slashTarget,
      `Rchain.Liveness.passing,
      `Rchain.Liveness.tally,
      `Rchain.Liveness.step,
      `Rchain.Liveness.castVote,
      `Rchain.Liveness.openWindow,
      `Rchain.Liveness.the_stall_rule_changes_nothing,
      `Rchain.Liveness.a_stall_freezes_the_jail_clock,
      `Rchain.Liveness.jail_never_touches_the_bond,
      `Rchain.Liveness.a_jailed_validator_leaves_the_active_set,
      `Rchain.Liveness.a_released_validator_reenters,
      `Rchain.Liveness.release_clears_the_window_and_ballots,
      `Rchain.Liveness.a_returning_validator_is_released,
      `Rchain.Liveness.a_still_absent_validator_is_renewed,
      `Rchain.Liveness.exactly_two_thirds_does_not_pass,
      `Rchain.Liveness.a_strict_supermajority_passes,
      `Rchain.Liveness.a_passing_vote_slashes_and_ejects,
      `Rchain.Liveness.a_vote_without_a_window_is_not_recorded,
      `Rchain.Liveness.a_vote_without_jail_is_not_opened,
      `Rchain.Liveness.a_boundary_at_the_threshold_jails,
      `Rchain.Liveness.one_lapse_is_not_sustained,
      `Rchain.Liveness.rebond,
      `Rchain.Liveness.unguardedRebond,
      `Rchain.Liveness.a_banished_validator_cannot_rebond,
      `Rchain.Liveness.an_unguarded_rebond_reaches_a_banished_validator,
      `Rchain.Liveness.lapseState,
      `Rchain.Liveness.lapseParams,
      `Rchain.Liveness.law89_statement,
      `Rchain.Liveness.law89,
      `Rchain.Liveness.law89_mutant,
      `Rchain.Liveness.law89_falsifier,
      `Rchain.ActiveBonds, `Rchain.bondsOfState, `Rchain.leak,
      `Rchain.leak_retires_only_the_silent, `Rchain.the_leaked_sender_is_not_in_the_quorum,
      `Rchain.totalStake_leak_le, `Rchain.bondsJoinActive,
      `Rchain.the_join_layer_advances_after_leak, `Rchain.a_speaker_within_the_window_does_not_leak,
      `Rchain.law89Leak_statement, `Rchain.law89Leak_statement_holds, `Rchain.law89Leak_mutant,
      `Rchain.law89Leak_falsifier, `Rchain.law89_with_leak_statement,
      `Rchain.law89_with_leak_statement_holds],
    axioms := [],
    rust := ["rholang/src/native_state.rs", "rholang/src/pos_liveness.rs",
      "rholang/src/system_processes.rs", "casper/src/genesis/mod.rs",
      "block-storage/src/dag/liveness.rs", "casper/src/multi_parent_casper.rs",
      "casper/src/system_deploy.rs", "casper/src/blocks/proposer/block_creator.rs"],
    rustWitness := ["rholang/src/native_state.rs:a_sustained_absence_jails_without_touching_the_bond",
      "rholang/src/native_state.rs:one_boundary_omission_does_not_jail",
      "rholang/src/native_state.rs:a_stall_freezes_the_jail_clock",
      "rholang/src/native_state.rs:a_vote_below_two_thirds_does_not_slash",
      "rholang/src/native_state.rs:a_passing_vote_slashes_and_permanently_ejects",
      "rholang/src/native_state.rs:a_vote_cannot_open_against_an_active_validator",
      "rholang/src/native_state.rs:a_jailed_validator_returns_after_the_term",
      "rholang/src/native_state.rs:a_released_jail_leaves_no_window_or_ballot",
      "rholang/src/native_state.rs:a_still_absent_validator_is_renewed",
      "rholang/src/native_state.rs:a_passing_vote_removes_a_staged_withdrawal",
      "rholang/src/native_state.rs:a_banished_validator_cannot_rebond",
      "rholang/src/native_state.rs:a_delegation_cannot_buy_slash_voting_power_mid_epoch",
      "rholang/src/native_state.rs:two_of_three_active_votes_cannot_cross_strict_threshold_via_delegation",
      "casper/src/multi_parent_casper.rs:a_deploy_signer_is_presence_at_the_including_height",
      "casper/src/multi_parent_casper.rs:presence_keeps_the_greatest_height",
      "rholang/src/system_processes.rs:pos_vote_slash_is_authorised_by_the_deployer_id",
      "rholang/src/native_state.rs:a_validator_that_does_not_cover_the_partition_is_leaked_after_the_window",
      "rholang/src/native_state.rs:a_boundary_that_covers_the_partition_clears_the_silence_counter",
      "casper/src/runtime_manager.rs:a_boundarys_layer_missing_reaches_play_and_replay_alike",
      "casper/src/blocks/proposer/block_creator.rs:the_proposer_attaches_the_list_the_replay_will_read"],
    witness := [`Rchain.Liveness.presence_is_the_max_participation_height,
      `Rchain.Liveness.a_deploy_signer_can_present_a_jailed_validator,
      `Rchain.Liveness.a_jailed_validator_is_never_present,
      `Rchain.Liveness.absence_is_missing_or_behind_the_threshold,
      `Rchain.Liveness.exactly_the_threshold_is_present,
      `Rchain.Liveness.quorum_tip_is_the_greatest_height,
      `Rchain.Liveness.the_stall_rule_changes_nothing,
      `Rchain.Liveness.a_stall_freezes_the_jail_clock,
      `Rchain.Liveness.one_lapse_is_not_sustained,
      `Rchain.Liveness.a_boundary_at_the_threshold_jails,
      `Rchain.Liveness.jail_never_touches_the_bond,
      `Rchain.Liveness.a_jailed_validator_leaves_the_active_set,
      `Rchain.Liveness.a_released_validator_reenters,
      `Rchain.Liveness.release_clears_the_window_and_ballots,
      `Rchain.Liveness.a_returning_validator_is_released,
      `Rchain.Liveness.a_still_absent_validator_is_renewed,
      `Rchain.Liveness.exactly_two_thirds_does_not_pass,
      `Rchain.Liveness.a_strict_supermajority_passes,
      `Rchain.Liveness.a_passing_vote_slashes_and_ejects,
      `Rchain.Liveness.a_vote_without_a_window_is_not_recorded,
      `Rchain.Liveness.a_vote_without_jail_is_not_opened,
      `Rchain.Liveness.a_banished_validator_cannot_rebond,
      `Rchain.Liveness.an_unguarded_rebond_reaches_a_banished_validator,
      `Rchain.Liveness.law89,
      `Rchain.Liveness.law89_falsifier,
      `Rchain.leak_retires_only_the_silent, `Rchain.the_leaked_sender_is_not_in_the_quorum,
      `Rchain.totalStake_leak_le, `Rchain.the_join_layer_advances_after_leak,
      `Rchain.a_speaker_within_the_window_does_not_leak, `Rchain.law89Leak_statement_holds,
      `Rchain.law89Leak_falsifier, `Rchain.law89_with_leak_statement_holds],
    falsifiable := "`one_lapse_is_not_sustained` shows one absent boundary does not jail with \
      `jailAfter = 2`; `a_boundary_at_the_threshold_jails` shows the sustained count does. \
      `exactly_two_thirds_does_not_pass` shows 600 of 900 is not a strict supermajority and \
      `a_strict_supermajority_passes` shows the crossing; `a_vote_without_a_window_is_not_recorded` \
      and `a_vote_without_jail_is_not_opened` are the refusal of a vote against an active validator; \
      `a_stall_freezes_the_jail_clock` and `the_stall_rule_changes_nothing` are the stall and \
      off-boundary no-ops; `a_passing_vote_slashes_and_ejects` is the bond/eject/clear arm. \
      **The retired authors-only presence** leaves a deploy-presented jail in place \
      (`law89_mutant`/`law89_falsifier`), while `presentFromAuthorsAndDeployers` releases it \
      (`a_deploy_signer_can_present_a_jailed_validator`). **The leak's arms**: \
      `the_join_layer_advances_after_leak` is the 200/200 advance that the 200/300 refusal could not \
      reach, `a_speaker_within_the_window_does_not_leak` is the empty silent set, and \
      `law89Leak_falsifier` constructs the mutant leak that retires every sender and empties the \
      quorum — a lockstep chain cannot finalise, so the mutant is not a refinement.",
    note := "RCHIP #55. The model derives presence from the participation map: `participationMap` is \
      the maximum height per validator over fringe authors and included deploy signers \
      (`casper/src/multi_parent_casper.rs:770-818`); the boundary's `quorumTip` is the greatest \
      height whose active supporting stake is strictly more than two thirds \
      (`rholang/src/pos_liveness.rs:221-246`); absence is no entry or `tip - height > threshold`, so \
      exactly the threshold behind is present. `step` runs the tally against the active snapshot \
      first, then counts absence and jails, then expires (renew while absent, release while present), \
      then ages windows. A newly jailed validator is removed from `active`, which is the vote \
      denominator; release re-enters it. `jailNew` and the expiry paths never write `pool` or \
      `withdrawals`; only `tally`/`slashTarget` do, and only for a passing target. **The 2026-11-17 \
      audit found the presence signal one-armed and the model now records the correction:** the \
      boundary's participation map read only finalised-fringe block senders, and a jailed validator \
      is filtered out of the drawn active set and refused by the proposer \
      (`select_active_excluding`, `is_active_validator`), so every jail was permanent. The code now \
      unions **deploy signers** with the authors (`participation_map`/`participation_from_fringe`, \
      `deploy_signers`), which is the signal a jailed key can still produce: it signs a funded \
      deploy an unjailed proposer includes. `presentFromAuthorsAndDeployers` is that rule; \
      `presentFromSenders` is the retired authors-only model, kept as the mutant. The slash's \
      distribution to delegators is Law 57's subject. **Amended 2026-11-22 with the priced \
      inactivity leak**, superseding the 2026-10-02 refusal: the bond-leave measurement showed a \
      layer at 100/100/50/50 whose 200 of 300 is exactly two thirds, where no predicate, window or \
      round-clock change advances the chain; retirement of the sender the gate names as not-full is \
      the escape, and the round-clock alternative was refuted by the N=8 invariants. The trigger is \
      the gate's own refusal, derived by every node from the block's justification set and never \
      carried on the wire (`CloseBlock.layer_missing`, `pos:silence`), so a proposer cannot choose \
      whom to leak; `Coverage.missing` stays the jail's. The leak runs **before** the frozen-tip \
      guard, because a frozen tip is the condition it exists to break. The bond forfeiture reuses \
      `slash` (`Malicious` takes the whole tier), so the validator leaves the pool, the active set \
      and every delegation ledger by the same rule a proven offence uses. Default off; a network \
      arms it at genesis. What it does **not** claim: that a live quorum produces a full layer in \
      bounded rounds — that remains Law 58's schedule hypothesis, measured by the campaign. The \
      boundary-skip hole (no validity rule requires a `CloseBlock` at a boundary) is a separate \
      residual." },

  { number := 90, clause := "a", layer := "Node",
    statement := "**A served hostlist entry is earned.** A peer appears in `GET /api/v1/nodes` only \
      when a probe of its derived client endpoint (`protocol + 3`) completed in the current cycle \
      with the serving node's `networkId`, a `shardId` in its membership, `proposer = true`, and an \
      `address` naming that peer's node id. A peer that is not dialable, does not answer, answers for \
      another network, shard or node, or does not claim a proposer is not served; the serving node \
      itself is served only when it can propose",
    status := .provedTied,
    corpus := some "hostlist",
    declarations := [`Rchain.HostList.valid, `Rchain.HostList.peerPart,
      `Rchain.HostList.servedIds, `Rchain.HostList.every_served_entry_is_valid,
      `Rchain.HostList.every_served_peer_is_a_proposer,
      `Rchain.HostList.a_failed_single_probe_is_absent,
      `Rchain.HostList.peer_part_mem],
    axioms := [],
    rust := ["node/src/web/node_list.rs", "casper/src/api/block_api_impl.rs"],
    rustWitness := ["node/src/web/node_list.rs:an_observer_is_not_served",
      "node/src/web/node_list.rs:a_mismatched_network_is_not_served",
      "node/src/web/node_list.rs:a_peer_on_another_shard_is_not_served",
      "node/src/web/node_list.rs:a_peer_answering_for_another_node_id_is_not_served",
      "node/src/web/node_list.rs:an_unreachable_peer_is_not_served",
      "node/src/web/node_list.rs:a_non_json_answer_is_refused",
      "casper/src/api/block_api_impl.rs:proposer_is_the_validator_identity_not_the_mode"],
    witness := [`Rchain.HostList.every_served_entry_is_valid,
      `Rchain.HostList.every_served_peer_is_a_proposer,
      `Rchain.HostList.a_failed_single_probe_is_absent],
    falsifiable := "Dropping `proposer` from `valid` serves an observer that reports \
      `manualPropose: true` (`an_observer_is_not_served`, and `an_observer_is_not_served` in the \
      corpus); dropping `networkId` serves another chain's node; dropping the address check serves a \
      co-hosted validator under its neighbour's identity; treating a failed probe as success serves a \
      peer that did not answer. The `proposer` field itself is falsified at the source: \
      `manual_propose` is `!autopropose && !proposeOnDeploy`, so an unkeyed observer reports it \
      `true`, exactly like a manual-mode validator — the test pins that both report the same mode and \
      only the key separates them.",
    note := "**What the model does not claim.** `proposer` is a peer's own claim over plain HTTP, so \
      an on-path observer can forge a well-formed answer; the list is advisory and a client \
      re-verifies. Host-class filtering (a public node never probes a private peer, Law 62's rule), \
      DNS pinning, the body and field bounds, and the redirect refusal are Rust-tested \
      implementation facts rather than modelled. The probe adds no new host to the node's egress: it \
      dials a port on a host the node already dials under mutual TLS, and the request carries no \
      caller- or peer-supplied URL. **Rollout:** a node older than the `proposer` field lacks it and \
      is excluded until upgraded; safe, temporarily a shorter list." },

  { number := 90, clause := "b", layer := "Node",
    statement := "**The list is bounded, deduplicated and ordered.** At most `maxEntries` entries, at \
      most one per node id, the serving node first when it can propose, and the rest ascending by \
      node id — the response is a function of the bound, not of the network's size, and its order is \
      stable across cycles",
    status := .provedTied,
    corpus := some "hostlist",
    declarations := [`Rchain.HostList.servedIds, `Rchain.HostList.peerPart,
      `Rchain.HostList.the_list_is_bounded, `Rchain.HostList.self_is_first,
      `Rchain.HostList.self_is_present_when_a_proposer],
    axioms := [],
    rust := ["node/src/web/node_list.rs"],
    rustWitness := ["node/src/web/node_list.rs:the_list_is_sorted_and_bounded",
      "node/src/web/node_list.rs:a_duplicate_connection_is_served_once",
      "node/src/web/node_list.rs:self_is_first_and_flagged",
      "node/src/web/node_list.rs:self_is_absent_when_not_a_proposer"],
    witness := [`Rchain.HostList.the_list_is_bounded, `Rchain.HostList.self_is_first,
      `Rchain.HostList.self_is_present_when_a_proposer],
    falsifiable := "Removing the `take` makes the list a function of the peer count; sorting after \
      truncation drops self or reorders it; skipping the id dedupe serves a peer twice when two \
      connections name it. `truncation_self_first` and `duplicate_ids` in the corpus pin the first \
      and third.",
    note := "Ascending order is **by construction** — the peer part is walked over ascending ids — so \
      it needs no sort theorem; the corpus's expected id list is what ties the order to the Rust's \
      lexicographic `BTreeMap` order, which coincides for the fixed-length hex ids the node uses. \
      `latestBlockNumber` and `acceptsDeploys` are served and deliberately **not** filtered on: a \
      lagging or full proposer is still deploy-capable, and there is no oracle for a lag threshold." },

  { number := 90, clause := "c", layer := "Node",
    statement := "**The list is fresh.** It is derived only from the probes of the cycle that \
      produced it; a peer whose probe fails in a cycle is absent from that cycle's list, whatever the \
      previous cycle served. There is no cache that could revive an entry",
    status := .provedTied,
    corpus := some "hostlist",
    declarations := [`Rchain.HostList.peerPart, `Rchain.HostList.servedIds,
      `Rchain.HostList.a_failed_single_probe_is_absent],
    axioms := [],
    rust := ["node/src/web/node_list.rs"],
    rustWitness := ["node/src/web/node_list.rs:an_unreachable_peer_is_not_served",
      "node/src/web/node_list.rs:a_public_node_does_not_probe_a_private_peer"],
    witness := [`Rchain.HostList.a_failed_single_probe_is_absent],
    falsifiable := "Keeping a per-peer success cache across cycles revives a peer that has gone away; \
      treating a timed-out probe as success serves it immediately. The `unreachable` corpus case pins \
      the derivation; the provider rebuilds its snapshot from each cycle's outcomes and never reads \
      the previous one.",
    note := "The only freshness window is the previous snapshot being served while the next cycle \
      runs, bounded by one probe interval and documented. The provider's snapshot is swapped whole; \
      no lock is held across a probe, and the route takes no caller input." },

  { number := 91, clause := "a", layer := "Rholang",
    statement := "**A canonicalisation or a union charges the elements it canonicalises.** A \
      collection the reducer builds (`toSet`, `toMap`, `set`, `keys`) charges three phlo per element \
      of the output it is about to build, charged from the operand size before the work; a set or \
      map union charges both operands, so the charge is a function of the total element count and \
      does not depend on which operand is which",
    status := .provedModel,
    declarations := [`Rchain.canonicalizationCost, `Rchain.unionCost,
      `Rchain.canonicalizationCost_covers_the_operand, `Rchain.unionCost_is_the_sum_of_its_operands,
      `Rchain.unionCost_symmetric, `Rchain.rightOnlyUnionCost,
      `Rchain.rightOnlyUnionCost_is_not_symmetric],
    axioms := [],
    rust := ["rholang/src/reduce.rs", "rholang/src/accounting.rs"],
    rustWitness := ["rholang/src/reduce.rs:to_set_charges_its_elements",
      "rholang/src/reduce.rs:to_map_charges_its_elements",
      "rholang/src/reduce.rs:keys_charges_its_output",
      "rholang/src/reduce.rs:set_charges_the_map_it_rebuilds",
      "rholang/src/reduce.rs:a_set_add_charges_the_set_it_rebuilds",
      "rholang/src/reduce.rs:a_union_charges_both_operands_alike",
      "rholang/src/accounting.rs:collection_and_censure_charges_are_the_models"],
    witness := [`Rchain.canonicalizationCost_covers_the_operand, `Rchain.unionCost_symmetric],
    falsifiable := "The charge this replaced was flat — 18 phlo for a `toSet` of 500 elements and \
      of 40 000, while the sort's wall time grew from 7 ms to 724 ms — and the union charged the \
      right operand only, which `rightOnlyUnionCost_is_not_symmetric` exhibits as asymmetric at one \
      pair. Reverting either makes the corresponding witness test fail: `to_set_charges_its_elements` \
      sees a zero marginal charge, and `a_union_charges_both_operands_alike` sees the two arms \
      differ.",
    note := "**The log factor is named rather than hidden.** A canonicalisation sorts, so the work \
      is `O(n log n)` and the charge is linear: a phlo buys work bounded by `charge · log charge`, \
      which is bounded — the defect C345 names is *unbounded* work for a flat charge — but it is not \
      an exact comparison count, and this row does not claim one. **Not charged, and deliberately \
      so**: a collection literal the evaluator builds stays uncharged, because its bytes were \
      charged at parse and the create path is not attacker-sized. The tie is prose plus the Rust \
      witness, not a corpus: the cost is a closed form, so a corpus of the same numbers would \
      restate the witness. Consensus-visible (#51 category A): the charge moves refund and burned \
      amounts, hence every post-state hash. **The row was true and the code was not, in one arm**: \
      `set.add`/`set + x` charged a flat 3 phlo for the same re-canonicalisation until the cost \
      register's first census found it (C353). The statement did not need to change — the arm needed \
      to join it, which is what a law that is checked against a census can say and a law checked \
      against its own witnesses cannot." },

  { number := 91, clause := "b", layer := "Rholang",
    statement := "**The censure charge bounds the cubic fold.** `rho:gov:censure` charges `(U+1)³` \
      phlo for a member universe of `U` — a charge that covers every member and grows with the \
      universe — where the fold's fixed point runs at most `U+1` rounds over `U` members",
    status := .provedModel,
    declarations := [`Rchain.censureCost, `Rchain.censureCost_exceeds_the_universe,
      `Rchain.censureCost_monotone, `Rchain.censureCost_is_not_the_square],
    axioms := [],
    rust := ["rholang/src/system_processes.rs", "rholang/src/accounting.rs"],
    rustWitness := ["rholang/src/system_processes.rs:censure_is_charged_the_cube_of_its_universe",
      "rholang/src/accounting.rs:collection_and_censure_charges_are_the_models"],
    witness := [`Rchain.censureCost_exceeds_the_universe, `Rchain.censureCost_monotone],
    falsifiable := "The charge this replaced was `U²` for a `Θ(U³)` fixed point; \
      `censureCost_is_not_the_square` exhibits a universe at which the two differ, and \
      `censure_is_charged_the_cube_of_its_universe` reads the handler's own cost cell at two \
      universes, so a revert to the quadratic table fails there rather than in an estimate.",
    note := "**The bound is measured, not chosen.** `MAX_GOV_UNIVERSE` is 512, and the cubic path at \
      it was timed in release (`rholang/tests/gas_amplification.rs`): 33 ms for a 512-member levels \
      map, 1–5 ms for the three mixed shapes — so the constant stays and the `(512+1)³` = 1.35e8 phlo \
      it charges is the same order as the time it buys under the table's one-phlo-per-operation \
      calibration. The bound is on the **sum** of censures, levels and vouchers, which is what stops \
      a deep cascade: a discredit needs a quorum of about two thirds of the eligible set, and the \
      universe counts the censure pairs that must supply it. **The other three `rho:gov:*` folds \
      keep the squared charge**, and their work shapes were measured afterwards (C354): `trustLevels` \
      converges in at most five rounds because a conferral cannot raise a level past the rater's, and \
      `resolveWeights` walks each member's delegation chain — both quadratic, so the squared charge \
      matches the shape. The measurement was recorded in the cost register (`spec/COST.md`) rather \
      than left as a residual." },

  { number := 91, clause := "c", layer := "Rholang",
    statement := "**A tally charges the option lists it scans.** A ranked tally charges the worst \
      case — one scan per ranked entry per round, at most `longest` rounds — and an approval tally \
      charges every approved entry. The member maps keep the universe bound; the lists are priced \
      rather than refused",
    status := .provedModel,
    declarations := [`Rchain.tallyCost, `Rchain.tallyCost_covers_the_entries,
      `Rchain.tallyCost_is_the_worst_case],
    axioms := [],
    rust := ["rholang/src/system_processes.rs", "rholang/src/accounting.rs"],
    rustWitness := ["rholang/src/system_processes.rs:a_tally_charges_the_option_lists_it_scans",
      "rholang/src/accounting.rs:the_tally_charges_are_the_models"],
    witness := [`Rchain.tallyCost_covers_the_entries, `Rchain.tallyCost_is_the_worst_case],
    falsifiable := "The charge this replaced was `(ballots + weights)²`, a function the option lists \
      do not appear in at all; `a_tally_charges_the_option_lists_it_scans` reads the handler's own \
      cost cell for the same election at two option-list sizes and the difference is zero under that \
      charge, so a revert fails there. The probe measures the work the old charge ignored: 34 ms for \
      a ranked tally of 100 ballots over 4 000 options and 68 ms for an approval tally over 4 000.",
    note := "**Priced, not refused.** The member maps keep `MAX_GOV_UNIVERSE`; the option lists are \
      bounded by the charge itself — a ballot map whose lists are a million entries deep is priced \
      past the block phlo cap instead of being run — because a refusal here would reject a legitimate \
      election (a few hundred ballots over ten options) to stop one that the phlo budget already \
      stops. The ranked worst case is `longest × entries`: every round scans every entry, and a round \
      may eliminate only one option, so `longest` bounds the rounds. The floor is the approval side. \
      Consensus-visible (#51 category A)." },
  { number := 92, layer := "Casper",
    statement := "**A replay failure's cause depends on the block's provenance.** An inbound block's \
      replay disagreement may be a view difference, so it is `Divergence` and C173's restoring rule \
      stays keyed on it. A block this node *built* has no peer view to differ from — its play and \
      replay are two executions of this node's own state — so the same failure is a local fault and \
      is never `Divergence`. The decision table is `Rchain.provenanceCause` over \
      `ProvenanceOrigin` and `ProvenanceRefusal`; it is the Rust's 2×3: `matched` is no refusal, \
      `postStateMismatch` is `InvalidState` for either origin, and only a failed replay is \
      origin-sensitive. The Rust's `ValidationOrigin` and `ValidateError::SelfReplayFailed` are \
      that table's implementation. The 2026-11-08 incident (#564) is the negative case: a self-built block's \
      `ReplayCostMismatch` was collapsed into `InvalidStateHash` → `Divergence`, so the proposer's \
      C270 arm neither counted, surfaced nor bounded it and rebuilt the identical block until \
      finality froze.",
    status := .provedModel,
    formal := `Rchain.law92_statement,
    mutant := `Rchain.law92_mutant,
    falsifier := `Rchain.law92_falsifier,
    declarations := [`Rchain.ProvenanceOrigin, `Rchain.ProvenanceRefusal, `Rchain.ProvenanceCause,
      `Rchain.provenanceCause,
      `Rchain.an_inbound_replay_failure_is_divergence,
      `Rchain.a_self_replay_failure_is_not_divergence,
      `Rchain.the_two_origins_disagree_about_the_same_failure,
      `Rchain.a_self_replay_failure_is_a_local_fault,
      `Rchain.provenanceCauseIgnoringOrigin,
      `Rchain.law92_mutant,
      `Rchain.law92_falsifier,
      `Rchain.law92_statement,
      `Rchain.law92,
      `Rchain.a_post_state_mismatch_is_invalid_state, `Rchain.a_matched_replay_has_no_refusal, `Rchain.the_two_origins_agree_about_a_post_state_mismatch
      ],
    axioms := [],
    rust := ["casper/src/interpreter_util.rs", "casper/src/multi_parent_casper.rs",
      "casper/src/blocks/proposer/proposer.rs"],
    rustWitness := ["casper/src/multi_parent_casper.rs:a_self_built_replay_failure_is_classified_as_a_local_fault"],
    witness := [`Rchain.an_inbound_replay_failure_is_divergence,
      `Rchain.a_self_replay_failure_is_not_divergence,
      `Rchain.the_two_origins_disagree_about_the_same_failure,
      `Rchain.a_self_replay_failure_is_a_local_fault,
      `Rchain.law92_falsifier,
      `Rchain.a_post_state_mismatch_is_invalid_state, `Rchain.a_matched_replay_has_no_refusal
      ],
    falsifiable := "**Delete the origin arm and map every `ReplayVerdict::Failed` to \
      `InvalidStateHash`** — `a_self_replay_failure_is_not_divergence` cannot be proved (`decide` \
      fails on the resulting equality), and the Rust witness reddens because the self-built case \
      returns the divergence status again. The live negative case is the incident: the same replay \
      refusal reached the proposer as `Divergence` and the loop was unbounded for 45 hours. The \
      inbound half is falsified by the existing restoring-rule tests, which require a peer replay \
      refusal to remain `Divergence` so C173's inverse can fire.",
    note := "**What this law does not claim.** It does not say the replay was wrong, which half \
      (play or replay) diverged, or that a deploy set is invalid — that is RC1, and the falsifier in \
      `casper/tests/determinism.rs` is the open measurement. It also does not make the bounded-retry \
      policy a consensus rule: holding a deploy set out of selection is proposer-local operating \
      policy (C357), another validator may include the same deploy, and the quarantine is invisible \
      to block validity. The Rust witness is the classification test, not the live arm." },
  { number := 93, layer := "Casper",
    statement := "**A cost verdict is not a state verdict.** A replay's evidence has two independent \
      components: what it learned about the declared post-state (`matched`, `mismatched`, or \
      `notComputed` when the enforcing path stopped at the cost check) and whether the recorded cost \
      matched. A cost mismatch does not imply a state mismatch; a state-correct block can carry a cost \
      mismatch. The enforcing path's `ReplayCostMismatch` is an accounting verdict taken before the \
      checkpoint, so it is not evidence that the state failed. The Rust shadow replay computes the \
      missing verdict on a bounded second pass and the operator record carries both. The incident \
      (#564) is the live case: a cost-only refusal was read as a divergence and the state was never \
      compared.",
    status := .provedModel,
    declarations := [`Rchain.StateVerdict, `Rchain.ReplayEvidence, `Rchain.StateCorrect,
      `Rchain.cost_mismatch_does_not_imply_state_mismatch,
      `Rchain.every_cost_verdict_admits_a_state_correct_replay,
      `Rchain.not_computed_is_not_a_mismatch],
    axioms := [],
    rust := ["casper/src/runtime_replay.rs", "casper/src/interpreter_util.rs",
      "casper/src/rholang.rs"],
    rustWitness := ["casper/tests/determinism.rs:a_cost_only_tamper_is_refused_before_the_state_hash_is_compared"],
    witness := [`Rchain.cost_mismatch_does_not_imply_state_mismatch,
      `Rchain.every_cost_verdict_admits_a_state_correct_replay,
      `Rchain.not_computed_is_not_a_mismatch],
    falsifiable := "**Make the state verdict a function of the cost verdict** — e.g. return \
      `StateVerdict.mismatched` whenever the cost mismatches. Then \
      `cost_mismatch_does_not_imply_state_mismatch` is false by construction (`decide` fails), and the \
      Rust witness reddens because the shadow can no longer report `state_hash_matched = true` for a \
      cost-only difference. The incident is the negative case: the enforcing path returned the cost \
      mismatch and never compared `8ae7d4de…`.",
    note := "**What this law does not decide.** It does not say whether a state-correct block with a \
      cost mismatch should be accepted; that is a protocol decision (the Scala oracle refuses it), \
      recorded in `spec/audit/evidence/n564-self-replay/cost-criticality.md`. It also does not say the \
      incident's block was state-correct: the refused body, the terms and A's store are unavailable, so \
      the shadow's verdict is a future-occurrence instrument, not a retroactive one. The Rust witness \
      is the cost-only tamper plus the shadow, which proves the mechanism, not the incident's state." },
  { number := 94, layer := "Casper",
    statement := "**A local fault has a bounded exit.** From the refused state there is **no \
      transition back to itself**: a local fault either changes the proposal input (the refused set is \
      held out and the immediate rebuild is an attestation with no user deploy), moves to a reset of a \
      **finalised** anchor, or halts. An unrecognised step is a halt, the safe fallback. Every fault \
      therefore reaches a safe state in one transition, and the incident's 45-hour loop — the same \
      deterministic block rebuilt and refused — has no representation to write. **The reset is not \
      terminal:** once the catch-up walk passes the finalised anchor it rewound to, \
      `resumeAfterCatchup` returns the node to healthy. The concrete arms are the model's \
      implementation: `attestOnly` carries no user deploy, `changeInput` holds the refused set out, \
      `latchAfter` makes the latch a transition state read by all four Rust propose sources \
      (`autoproposeTimer`, `attestOnNewBlocks`, `notDueRetry`, `explicitPropose`), \
      `resetResumeIsDue` is the Rust's `self_replay_resume_is_due` (the `Rewound` marker, a frontier \
      strictly past its height, the record's reset/halt stage, its one attempt and the same anchor), \
      `resetIsRecurrence` is the Rust's `reset_is_recurrence` against a finalised anchor, and \
      `wellFormedTrace` makes the durable write precede each observable transition.",
    status := .provedModel,
    formal := `Rchain.law94_statement,
    mutant := `Rchain.law94_mutant,
    falsifier := `Rchain.law94_falsifier,
    declarations := [`Rchain.RecoveryState, `Rchain.RecoveryStep, `Rchain.nextState,
      `Rchain.RecoverySafe, `Rchain.no_unchanged_retry, `Rchain.every_fault_reaches_safe,
      `Rchain.a_caught_up_reset_resumes, `Rchain.resume_requires_a_reset,
      `Rchain.resetAdmitted, `Rchain.a_non_finalised_anchor_is_refused,
      `Rchain.a_finalised_anchor_is_admitted,
      `Rchain.Deploy, `Rchain.Input, `Rchain.attestOnly,
      `Rchain.the_attest_only_rebuild_has_no_user_deploy,
      `Rchain.changeInput, `Rchain.the_refused_deploy_is_not_in_the_rebuild,
      `Rchain.AttemptKey, `Rchain.keyOf, `Rchain.a_retry_of_the_same_key_is_not_a_new_attempt,
      `Rchain.Trigger, `Rchain.triggerAdmitted, `Rchain.a_halted_proposer_admits_no_trigger,
      `Rchain.triggerSkipsLatch, `Rchain.a_trigger_that_skips_the_latch_admits_a_block,
      `Rchain.Transition, `Rchain.transitionStep, `Rchain.an_observable_halt_has_a_durable_record,
      `Rchain.a_record_after_the_transition_is_not_a_bracket,
      `Rchain.Reset, `Rchain.canReset, `Rchain.the_reset_anchor_is_finalised,
      `Rchain.at_most_one_reset_per_anchor,
      `Rchain.resumeAdmitted, `Rchain.resume_requires_catch_up,
      `Rchain.resume_requires_a_finalised_anchor,
      `Rchain.RecoveryStepMutant, `Rchain.nextStateMutant,
      `Rchain.law94_mutant, `Rchain.law94_falsifier,
      `Rchain.law94_statement, `Rchain.law94,
      `Rchain.LatchState, `Rchain.latchAfter, `Rchain.a_halt_latches, `Rchain.a_resume_clears_the_latch, `Rchain.a_non_resume_step_does_not_clear_the_latched_state, `Rchain.TriggerPath, `Rchain.admitted, `Rchain.rustTriggerSources, `Rchain.the_four_triggers_are_the_rust_sources, `Rchain.every_trigger_reads_the_latch, `Rchain.explicitProposeSkipsLatch, `Rchain.the_latch_skipping_path_admits, `Rchain.ResumePhase, `Rchain.ResetMarker, `Rchain.SelfReplayStage, `Rchain.SelfReplayRecord, `Rchain.ResumeState, `Rchain.resetResumeIsDue, `Rchain.resume_requires_a_rewound_marker, `Rchain.resume_requires_a_record, `Rchain.resume_requires_the_reset_attempt, `Rchain.resume_requires_the_same_anchor, `Rchain.a_resume_below_the_anchor_is_refused, `Rchain.a_marker_from_another_anchors_reset_does_not_resume, `Rchain.ResetAttemptRecord, `Rchain.resetIsRecurrence, `Rchain.resetAnchorFinalised, `Rchain.a_second_reset_at_the_same_anchor_is_refused, `Rchain.a_reset_at_a_different_anchor_is_not_a_recurrence, `Rchain.a_recurrence_names_the_attempted_anchor, `Rchain.raiseReset, `Rchain.raiseReset_records_the_anchor, `Rchain.TraceItem, `Rchain.wellFormedTrace, `Rchain.noAnchorTrace, `Rchain.resetTrace, `Rchain.badTrace, `Rchain.a_well_formed_trace_has_no_step_before_a_write, `Rchain.the_no_anchor_trace_records_before_the_halt, `Rchain.the_reset_trace_records_before_each_transition, `Rchain.the_bad_trace_is_not_well_formed, `Rchain.BoundedRecovery, `Rchain.law94_recovery_layer, `Rchain.law94_recovery_layer_proof,
      `Rchain.Acceptance.BoundaryShape, `Rchain.Acceptance.AcceptanceRecord,
      `Rchain.Acceptance.acceptanceValid, `Rchain.Acceptance.acceptanceTable,
      `Rchain.Acceptance.exactly_one_boundary_shape,
      `Rchain.Acceptance.a_mixed_version_rollout_is_refused,
      `Rchain.Acceptance.a_missing_independent_review_is_refused,
      `Rchain.Acceptance.a_new_genesis_with_a_sweep_is_refused,
      `Rchain.Acceptance.a_retained_history_without_a_complete_sweep_is_refused,
      `Rchain.Acceptance.a_reconstructed_trace_without_owner_acceptance_is_refused,
      `Rchain.Acceptance.a_reconstructed_trace_without_reviewer_acceptance_is_refused,
      `Rchain.Acceptance.a_reconstructed_trace_with_both_acceptances_is_accepted
      ],
    axioms := [],
    rust := ["casper/src/blocks/proposer/proposer.rs", "casper/src/api/block_api.rs",
      "casper/src/engine/reset.rs", "casper/src/engine/catchup.rs",
      "block-storage/src/dag/dag_storage.rs"],
    rustWitness := ["casper/src/blocks/proposer/proposer.rs:a_self_replay_failure_is_bounded_and_not_counted_as_divergence",
      "casper/src/blocks/proposer/proposer.rs:a_self_replay_fault_raises_one_reset_and_records_the_halt",
      "casper/src/blocks/proposer/proposer.rs:a_reset_at_an_anchor_already_attempted_is_refused",
      "casper/tests/reset.rs:a_caught_up_self_replay_reset_resumes_without_an_operator_post",
      "casper/tests/reset.rs:a_self_replay_reset_below_its_anchor_does_not_resume"],
    witness := [`Rchain.no_unchanged_retry, `Rchain.every_fault_reaches_safe,
      `Rchain.a_caught_up_reset_resumes, `Rchain.a_non_finalised_anchor_is_refused,
      `Rchain.the_attest_only_rebuild_has_no_user_deploy,
      `Rchain.the_refused_deploy_is_not_in_the_rebuild,
      `Rchain.a_retry_of_the_same_key_is_not_a_new_attempt,
      `Rchain.a_halted_proposer_admits_no_trigger,
      `Rchain.a_trigger_that_skips_the_latch_admits_a_block,
      `Rchain.an_observable_halt_has_a_durable_record,
      `Rchain.a_record_after_the_transition_is_not_a_bracket,
      `Rchain.the_reset_anchor_is_finalised,
      `Rchain.at_most_one_reset_per_anchor,
      `Rchain.resume_requires_catch_up,
      `Rchain.resume_requires_a_finalised_anchor,
      `Rchain.law94_falsifier,
      `Rchain.a_halt_latches, `Rchain.a_resume_clears_the_latch, `Rchain.a_non_resume_step_does_not_clear_the_latched_state, `Rchain.the_four_triggers_are_the_rust_sources, `Rchain.every_trigger_reads_the_latch, `Rchain.the_latch_skipping_path_admits, `Rchain.resume_requires_catch_up, `Rchain.resume_requires_a_rewound_marker, `Rchain.resume_requires_a_record, `Rchain.resume_requires_the_reset_attempt, `Rchain.resume_requires_the_same_anchor, `Rchain.the_reset_anchor_is_finalised, `Rchain.at_most_one_reset_per_anchor, `Rchain.a_second_reset_at_the_same_anchor_is_refused, `Rchain.a_recurrence_names_the_attempted_anchor, `Rchain.the_no_anchor_trace_records_before_the_halt, `Rchain.the_reset_trace_records_before_each_transition, `Rchain.the_bad_trace_is_not_well_formed, `Rchain.law94_recovery_layer_proof
      ],
    falsifiable := "**Add a transition from `refused` to `refused`** (the incident's retry). Then \
      `no_unchanged_retry` cannot be proved (`decide` fails on the equality) and the proposer witness \
      reddens, because the second build is the refused block again rather than an attestation. The \
      live negative case is the incident: the same block, the same refusal, 45 hours. The reset half \
      is falsified by admitting a non-finalised anchor — `a_non_finalised_anchor_is_refused` fails to \
      decide — which is the state `rewind_plan`'s finalised arm and the pre-flight's absent-anchor \
      refusal exist to prevent. The resume half is falsified by admitting `resumeAfterCatchup` from \
      `refused` (`resume_requires_a_reset` fails to decide) and in Rust by resuming below the anchor \
      or at another anchor (`a_self_replay_reset_below_its_anchor_does_not_resume`, \
      `a_marker_from_another_anchors_reset_does_not_clear_the_self_replay_fault`). The state-derived \
      rung adds the mutants the prose named: a latch-skipping path \
      (`the_latch_skipping_path_admits`), a resumed marker that is not `Rewound`, a record with no \
      attempt or another anchor, a second reset at the same anchor, and `badTrace` \
      (`the_bad_trace_is_not_well_formed`).",
    note := "**What this law does not claim.** It does not model the replay's cause (RC-B, open), the \
      cost/state ordering (law 93), or the storage-level crash bracket of the record (laws \
      72/73/76); `wellFormedTrace` makes the record-before-observable-transition rule a property of \
      the trace, while the storage-level crash window remains the Rust's (laws 72/73/76). It says \
      that the fault state has an exit, that the exit is not a no-op, and that a completed reset \
      returns the node to healthy. The one-reset-per-finalised-anchor bound is \
      operating policy rather than a consensus invariant; `at_most_one_reset_per_anchor` states it \
      and the Rust witness refuses a second reset at the same anchor; the operator's explicit resume \
      remains the fallback when no anchor was named, the reset was disabled, or the anchor has \
      already had its attempt." },

  { number := 95, clause := "a", layer := "RSpace",
    statement := "**The replay record is built in the play's execution order.** RSpace prepends \
      every event to its log, so the log is newest-first (`rspace/src/rspace.rs:202-216`), while \
      `build_replay_data` files each recorded COMM under its consume/produce refs in the order it \
      walks that log (`rspace/src/replay_rspace.rs:129-167`). A chronological build makes the \
      candidate list a consume sees the play's own sequence; the newest-first builder is the \
      defect's shape. The law fixes the order; the cost consequence is clause b. The rung is \
      `FilesPlayOrder` over the witness, with the newest-first build as its mutant and \
      `law95a_falsifier` as the refusal.",
    status := .provedModel,
    formal := `Rchain.ReplayOrder.law95a_statement,
    mutant := `Rchain.ReplayOrder.law95a_mutant,
    falsifier := `Rchain.ReplayOrder.law95a_falsifier,
    declarations := [`Rchain.ReplayOrder.Event, `Rchain.ReplayOrder.ReplayData,
      `Rchain.ReplayOrder.build, `Rchain.ReplayOrder.buildNewestFirst,
      `Rchain.ReplayOrder.the_chronological_build_files_the_play_order,
      `Rchain.ReplayOrder.the_newest_first_build_files_the_later_comm_first,
      `Rchain.ReplayOrder.FilesPlayOrder, `Rchain.ReplayOrder.law95a_statement, `Rchain.ReplayOrder.law95a_mutant, `Rchain.ReplayOrder.law95a_falsifier, `Rchain.ReplayOrder.law95a
      ],
    axioms := [],
    rust := ["rspace/src/replay_rspace.rs", "rspace/src/rspace.rs"],
    rustWitness := ["casper/tests/replay_trace_order.rs:pending_peeks_replay_with_exact_cost_and_state"],
    witness := [`Rchain.ReplayOrder.the_chronological_build_files_the_play_order,
      `Rchain.ReplayOrder.the_newest_first_build_files_the_later_comm_first,
      `Rchain.ReplayOrder.law95a_falsifier
      ],
    falsifiable := "Delete `.rev()` from `build_replay_data` — that is exactly the newest-first \
      builder — and the chronological theorem is false of the code: the witness list is \
      `[later, earlier]` and the Rust witness returns `ReplayCostMismatch` again. The model's two \
      builder functions are the two code shapes; the `[5, 3]` trace is the smallest in which they \
      disagree.",
    note := "**What this law does not claim.** It does not model the produce-side map, the matcher, \
      or the RCHAIN-3505 workaround that drops the reverse half for a failed deploy \
      (`casper/src/runtime_replay.rs:733-749`); that mask is Law 11's recorded hazard, not this \
      law's. The incident's 5576/5302 pair is evidence, not a model witness: the Lean witness is a \
      decided trace, and the Rust test pins 1860 / 2606 / 4844. The model abstracts \
      produce/consume identity to `Nat` refs because the law is about order, not hashing." },

  { number := 95, clause := "b", layer := "RSpace",
    statement := "**The inverted order changes the binding and the cost.** On a two-request witness \
      sharing one persistent consume's datum, the chronological list offers the earlier COMM (cost \
      5) and the later request is suppressed; the newest-first list offers the later COMM (cost 3) \
      and the earlier request is suppressed. The record is the same; the order is not. The \
      continuation-bearing witness (`the_continuation_is_charged`) puts the whole charge in the \
      continuation, which is the quantity the incident suppressed; the exact 5576/5302 and \
      1860/2606/4844 costs remain the Rust test's, not the model's.",
    status := .provedModel,
    formal := `Rchain.ReplayOrder.law95b_statement,
    mutant := `Rchain.ReplayOrder.law95b_mutant,
    falsifier := `Rchain.ReplayOrder.law95b_falsifier,
    declarations := [`Rchain.ReplayOrder.Store, `Rchain.ReplayOrder.run,
      `Rchain.ReplayOrder.the_two_builds_differ_on_the_witness,
      `Rchain.ReplayOrder.the_inversion_changes_the_cost,
      `Rchain.ReplayOrder.a_persistent_datum_survives_a_fire,
      `Rchain.ReplayOrder.a_non_persistent_datum_is_consumed,
      `Rchain.ReplayOrder.earlierCont, `Rchain.ReplayOrder.laterCont, `Rchain.ReplayOrder.witnessLogCont, `Rchain.ReplayOrder.the_continuation_is_charged, `Rchain.ReplayOrder.law95b_statement, `Rchain.ReplayOrder.law95b_mutant, `Rchain.ReplayOrder.law95b_falsifier, `Rchain.ReplayOrder.law95b
      ],
    axioms := [],
    rust := ["rspace/src/replay_rspace.rs"],
    rustWitness := ["casper/tests/replay_trace_order.rs:pending_peeks_replay_with_exact_cost_and_state"],
    witness := [`Rchain.ReplayOrder.the_inversion_changes_the_cost,
      `Rchain.ReplayOrder.the_two_builds_differ_on_the_witness,
      `Rchain.ReplayOrder.a_persistent_datum_survives_a_fire,
      `Rchain.ReplayOrder.the_continuation_is_charged, `Rchain.ReplayOrder.a_non_persistent_datum_is_consumed, `Rchain.ReplayOrder.law95b_falsifier
      ],
    falsifiable := "Make the build the unreversed walk: `run` then charges the later COMM first, \
      the first conjunct of the cost theorem decides to 3 instead of 5, and the Rust witness reddens \
      with the cost mismatch. The persistence control is falsified by making the datum persistent \
      always true, which changes the second request's outcome and fails the non-persistent control.",
    note := "The model's `run` is the selector's order-sensitivity, not a full replay: it charges \
      the cost and continuation of the **first** offered COMM and suspends when none is offered, \
      which is what `get_comm_and_consume_candidates` does (`rspace/src/replay_rspace.rs:235-246`). \
      The 274-phlo class is the shape; the incident's exact deploy set is not modelled." },

  { number := 96, clause := "a", layer := "Scheduler",
    statement := "**Verification is not optional.** The effective batch policy — what the runtime \
      manager stores after `with_batch_policy`/`set_batch_policy` — has no state with an enabled \
      batch and verification off; the canonicaliser forces `verify` \
      (`casper/src/runtime_manager.rs:501-513`).",
    status := .provedModel,
    declarations := [`Rchain.Split.BatchPolicy, `Rchain.Split.canonicalize,
      `Rchain.Split.the_canonicaliser_forces_verification,
      `Rchain.Split.an_unverified_policy_is_not_canonical],
    axioms := [],
    rust := ["casper/src/runtime_manager.rs"],
    rustWitness := ["casper/src/runtime_manager.rs:a_batch_policy_without_verification_is_not_representable"],
    witness := [`Rchain.Split.the_canonicaliser_forces_verification,
      `Rchain.Split.an_unverified_policy_is_not_canonical],
    falsifiable := "Delete the `verify` forcing from `canonical_batch_policy`; the Rust witness \
      reddens because the runtime then accepts an enabled unverified policy. The raw struct literal \
      is constructible — the law is about the effective policy, which is why the Rust test drives \
      the canonicaliser rather than the fields.",
    note := "The model's `canonicalize` is `casper/src/runtime_manager.rs:508-513`; the theorem is \
      the statement that the canonicaliser leaves no unverified enabled batch." },

  { number := 96, clause := "b", layer := "Scheduler",
    statement := "**A certified split equals the fold.** For a canonical policy the published \
      per-deploy cost, COMM log, failure flag and post-state are the sequential oracle's; a failed \
      verification is the whole-block sequential fallback. An unverified split can record a \
      different cost and state — C360's `[986, 984, 984, 984]` and the different post-state.",
    status := .provedModel,
    declarations := [`Rchain.Split.Outcome, `Rchain.Split.publishedBatch,
      `Rchain.Split.a_canonical_policy_publishes_the_oracle,
      `Rchain.Split.an_unverified_policy_can_change_the_cost],
    axioms := [],
    rust := ["casper/src/runtime_manager.rs"],
    rustWitness := ["casper/tests/determinism.rs:play_and_replay_agree_per_deploy_for_same_key_multi_transfer"],
    witness := [`Rchain.Split.a_canonical_policy_publishes_the_oracle,
      `Rchain.Split.an_unverified_policy_can_change_the_cost],
    falsifiable := "Disable the verification comparison in the batched path (or run it with \
      verification off): the batched costs `[986, 984, 984, 984]` and the post-state differ from \
      the sequential oracle (C360, matrix M08), which is exactly the unverified witness; the Rust \
      test that compares the two play paths reddens.",
    note := "The state half is Law 25's and Law 86c's; this row adds the per-deploy carrier (cost, \
      log, failure flag). The model's `speculative` is a `+1` stand-in for C360's observed \
      difference; the Rust witness carries the real numbers." },

  { number := 96, clause := "c", layer := "Scheduler",
    statement := "**Concurrent validation of a dependency-free batch agrees with serial \
      validation.** Each block replays on its own fork seeded at its committed pre-state and \
      insertion remains serial (`casper/src/blocks/block_processor.rs:536-620`, \
      `casper/src/runtime_manager.rs:670-688`); a shared replay runtime would change a verdict.",
    status := .provedModel,
    declarations := [`Rchain.Split.Block, `Rchain.Split.Verdict,
      `Rchain.Split.serialValidation, `Rchain.Split.concurrentValidation,
      `Rchain.Split.sharedValidation,
      `Rchain.Split.concurrent_validation_agrees_with_serial,
      `Rchain.Split.a_shared_replay_runtime_changes_a_verdict],
    axioms := [],
    rust := ["casper/src/blocks/block_processor.rs", "casper/src/runtime_manager.rs"],
    rustWitness := ["casper/src/blocks/block_processor.rs:the_parallel_validation_cap_is_never_zero",
      "casper/src/runtime_manager.rs:fork_replay_runtime_replays_empty",
      "casper/tests/determinism.rs:concurrent_forked_replays_agree_with_serial_replays"],
    witness := [`Rchain.Split.concurrent_validation_agrees_with_serial,
      `Rchain.Split.a_shared_replay_runtime_changes_a_verdict],
    falsifiable := "Validate a batch against one shared replay runtime instead of a fork per block: \
      the shared-runtime witness is that shape, and the per-block fork is what makes the equality \
      true. `concurrent_forked_replays_agree_with_serial_replays` reddens if the concurrent replay \
      does not reach the serial verdict; the cap test pins the concurrent path and the fork test \
      pins the isolation.",
    note := "The equality is a consequence of each block's replay being a function of its own \
      committed pre-state; the dependency-free/antichain hypothesis is the processor's batch, not \
      arbitrary blocks. It does not claim a speedup or a capacity result (M1/M2). The direct Rust \
      witness replays two dependency-free blocks concurrently on per-block forks and compares each \
      verdict with its serial replay." },

  { number := 96, clause := "d", layer := "Scheduler",
    statement := "**A cross-node verdict disagreement is not published.** Two verdicts are \
      reconciled only by agreement; a difference publishes nothing and is attributed by provenance \
      (Law 92) and bounded by the recovery state machine (Law 94), never silently reconciled.",
    status := .provedModel,
    declarations := [`Rchain.Split.publishAgreed, `Rchain.Split.publishLocal,
      `Rchain.Split.a_disagreement_publishes_nothing,
      `Rchain.Split.an_agreement_publishes_it,
      `Rchain.Split.the_local_rule_publishes_a_disagreement],
    axioms := [],
    rust := ["casper/src/interpreter_util.rs", "casper/src/blocks/proposer/proposer.rs"],
    rustWitness := ["casper/tests/determinism.rs:a_cost_only_tamper_is_refused_before_the_state_hash_is_compared"],
    witness := [`Rchain.Split.a_disagreement_publishes_nothing,
      `Rchain.Split.an_agreement_publishes_it,
      `Rchain.Split.the_local_rule_publishes_a_disagreement],
    falsifiable := "Publish the local verdict despite the peer's disagreement — the local rule \
      exhibits it, and the incident's divergence reading was that shape; the agreed rule is what \
      the node must keep. The Rust cost-only tamper test pins the refusal; \
      `a_disagreement_publishes_nothing` is the local-rule theorem.",
    note := "This is the composition of Law 92 (a self-built replay failure is a local fault) and \
      Law 94 (a local fault has a bounded exit). It does not say the raw verdicts are equal — that \
      is 96b's canonical-path statement; it says a difference is not reconciled into a \
      publication." },

  { number := 97, clause := "a", layer := "Casper",
    statement := "**Every conflict-scope chain is classified, and every rejection has a reason.** A \
      chain is kept, rejected by the conflict resolution, or rejected by a final-scope decision; a \
      kept chain is neither resolved nor forced out; a rejected conflict-scope chain carries a \
      non-zero reason (I1: it conflicts with a kept chain or depends on a rejected one). The \
      final-scope ledger is named separately (`final_scope_rejected_chains`/`_deploys`).",
    status := .provedModel,
    declarations := [`Rchain.MergeAccounting.Verdict, `Rchain.MergeAccounting.classify,
      `Rchain.MergeAccounting.every_chain_is_classified,
      `Rchain.MergeAccounting.a_kept_chain_is_neither_resolved_nor_forced,
      `Rchain.MergeAccounting.every_rejection_has_a_reason],
    axioms := [],
    rust := ["casper/src/merging.rs", "sdk/src/dag/merging.rs"],
    rustWitness := ["casper/tests/n420_boundary_merge.rs:a_boundary_round_merge_keeps_the_base_states_user_write"],
    witness := [`Rchain.MergeAccounting.every_chain_is_classified,
      `Rchain.MergeAccounting.a_kept_chain_is_neither_resolved_nor_forced,
      `Rchain.MergeAccounting.every_rejection_has_a_reason],
    falsifiable := "Drop a chain from the classification (the defect's shape: a rejection visible \
      nowhere, #280) — the totality theorem has no fourth state, and the kept theorem fails if a \
      resolved chain is reported kept. Make the reason always zero — the non-zero conclusion of the \
      reason theorem fails. The Rust witness asserts the report's partition and the I1/I2 \
      emptiness; the totality theorem is `every_chain_is_classified`.",
    note := "**The fixture identity is not the general theorem.** \
      `resolve_conflict_set_with_census` returns `kept = conflictSet \\ enforce_rejected \\ \
      resolved` and `rejected = resolved ∪ enforce_rejected` (`sdk/src/dag/merging.rs:926-978`), \
      so `kept + rejected = conflict` holds only when the final-scope decisions lie inside the \
      conflict set. The n420 assertion is that fixture's control; this row states the set relation \
      and the per-chain classification, which are general." },

  { number := 97, clause := "b", layer := "Casper",
    statement := "**No kept effect is silently lost.** The coverage check is multiset-sensitive: \
      every occurrence a kept chain added or removed must occur in the combined change, so the \
      duplicate-occurrence case (the #420 incident's shape) cannot pass as a set check.",
    status := .provedModel,
    declarations := [`Rchain.MergeAccounting.covers,
      `Rchain.MergeAccounting.coverage_is_reflexive,
      `Rchain.MergeAccounting.a_dropped_occurrence_breaks_coverage,
      `Rchain.MergeAccounting.duplicate_occurrences_are_counted],
    axioms := [],
    rust := ["casper/src/merging.rs"],
    rustWitness := ["casper/src/merging.rs:coverage_preserves_duplicate_occurrences_within_and_across_kept_chains"],
    witness := [`Rchain.MergeAccounting.coverage_is_reflexive,
      `Rchain.MergeAccounting.a_dropped_occurrence_breaks_coverage,
      `Rchain.MergeAccounting.duplicate_occurrences_are_counted],
    falsifiable := "Replace the multiset coverage with a set or `contains` check: the dropped \
      duplicate is the case it misses, and the Rust duplicate-occurrence witness reddens. The \
      positive is the reflexive coverage; the two decided instances keep it from passing on a \
      single occurrence.",
    note := "The coverage check is computed from the kept chains' `StateChange`s against the \
      combined change (`casper/src/merging.rs:350-403`); it is an observation, not an input to the \
      state — the report re-runs identically." },

  { number := 97, clause := "c", layer := "Casper",
    statement := "**A cost move is applied at most once.** A move whose `(payer, slot)` is already \
      applied is skipped; a composition that would take a vault below zero is refused rather than \
      invented — the fail-closed branch reachable when two accepted blocks spend one payer's REV \
      without seeing each other.",
    status := .provedModel,
    declarations := [`Rchain.MergeAccounting.CostMove, `Rchain.MergeAccounting.applies,
      `Rchain.MergeAccounting.applyOne,
      `Rchain.MergeAccounting.a_seen_move_is_not_reapplied,
      `Rchain.MergeAccounting.an_unseen_move_is_applied,
      `Rchain.MergeAccounting.compose,
      `Rchain.MergeAccounting.an_overdraft_is_refused,
      `Rchain.MergeAccounting.a_funded_spend_is_composed],
    axioms := [],
    rust := ["casper/src/merging.rs"],
    rustWitness := ["casper/tests/n420_boundary_merge.rs:sibling_hot_cell_merges_keep_one_value_and_name_a_rejection"],
    witness := [`Rchain.MergeAccounting.a_seen_move_is_not_reapplied,
      `Rchain.MergeAccounting.an_unseen_move_is_applied,
      `Rchain.MergeAccounting.an_overdraft_is_refused,
      `Rchain.MergeAccounting.a_funded_spend_is_composed],
    falsifiable := "Apply every move unconditionally (drop the `seen` check): the seen-move \
      theorem fails. Return a negative balance instead of refusing: the overdraft theorem fails. \
      The Rust sibling-hot-cell witness pins the one-value/one-slot outcome; the parallel-register \
      witness pins the parallel case.",
    note := "The refusal is a merge-level `Err` (a local drop), not a block-validity verdict \
      (`casper/src/merging.rs:1616-1630`). A rejected chain's effects are not applied; the \
      rejection is recorded (`rejected_cost_accounted_chains`). It does not decide the fee/refund \
      question (Law 48)." },

  { number := 97, clause := "d", layer := "Casper",
    statement := "**Independent chains commute and their charges add.** The state half is Law 86c's \
      (independent `StateChange`s commute); the cost half is this law's: the merge's per-chain move \
      totals are additive over the kept set.",
    status := .provedModel,
    declarations := [`Rchain.MergeAccounting.moveTotal, `Rchain.MergeAccounting.totalCost,
      `Rchain.MergeAccounting.chainA, `Rchain.MergeAccounting.chainB,
      `Rchain.MergeAccounting.independent_merges_commute_and_add],
    axioms := [],
    rust := ["casper/src/merging.rs", "rspace/src/merger/state_change.rs"],
    rustWitness := ["casper/tests/n420_boundary_merge.rs:parallel_register_shaped_rmws_leave_a_countable_channel"],
    witness := [`Rchain.MergeAccounting.independent_merges_commute_and_add],
    falsifiable := "Make the cost total non-additive (for example, subtract a shared fee once per \
      merge): the additive theorem fails. The state half's falsifier is Law 86c's, which reddens if \
      the merge orders dependent chains. The Rust parallel-register witness pins the state \
      outcome; the additive theorem is `independent_merges_commute_and_add`.",
    note := "The state half is cited to Law 86c, not reproved; this row adds the cost carrier. The \
      model's two chains are a concrete disjoint pair; the general independence criterion is Law \
      17c's `chainConflict`/`resolves`." },

  { number := 98, clause := "a", layer := "Casper",
    statement := "**A cross-shard leg's prepare escrows exactly, and conserves the leg total.** \
      `txn_prepare` debits the payer by exactly the amount, leaves the payee untouched when the two \
      accounts differ, records a `prepared` record carrying that amount, and the payer + payee + \
      escrow total is unchanged. The debit happens after the balance check, so a refused prepare \
      moves nothing.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.escrow, `Rchain.Acceptance.recordTotal,
      `Rchain.Acceptance.preparedRecord, `Rchain.Acceptance.prepareWithoutDebit,
      `Rchain.Acceptance.prepare_escrows_exactly,
      `Rchain.Acceptance.prepare_conserves_the_leg_total,
      `Rchain.Acceptance.a_prepare_that_does_not_debit_creates_value],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    rustWitness := ["rholang/src/native_state.rs:txn_prepare_commit_round_trip",
      "casper/tests/gateway_txn.rs:gateway_moves_the_escrow_on_every_leg"],
    witness := [`Rchain.Acceptance.prepare_escrows_exactly,
      `Rchain.Acceptance.prepare_conserves_the_leg_total,
      `Rchain.Acceptance.a_prepare_that_does_not_debit_creates_value],
    falsifiable := "Record the transaction without debiting the payer (`prepareWithoutDebit`): the \
      escrow-inclusive total is larger by exactly the amount, which is \
      `a_prepare_that_does_not_debit_creates_value`; a double debit fails the exact-debit \
      conjunct. In Rust, the `100 -> 60` payer balance is `txn_prepare_commit_round_trip` and the \
      two-shard `100 -> 70 / 100 -> 60` debits are `gateway_moves_the_escrow_on_every_leg`.",
    note := "**What this law does not claim.** Idempotence is Law 28's, cited \
      (`Rchain.txnPrepare_idempotent`), not reproved. The gateway's boundary does not validate the \
      amount — Law 26 records that `admitLeg` admits a negative amount and the ledger's `NonNegI64` \
      catches it later. The model's `Ledger` is the participant's `native_state` store, and \
      `src = dst` is excluded from the untouched-payee half because the pair total is not the right \
      carrier for a self-payment." },

  { number := 98, clause := "b", layer := "Casper",
    statement := "**The terminal verbs release exactly the escrow, never more and never twice.** \
      `txn_commit` credits the payee by exactly the record's amount and leaves the payer untouched \
      when the accounts differ; `txn_abort` refunds the payer by exactly that amount and leaves the \
      payee untouched; each moves the record to a terminal state, so the escrow-inclusive total is \
      unchanged. Commit is not a mint and abort is not a second payment.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.commit_credits_the_payee_exactly,
      `Rchain.Acceptance.commit_conserves_the_leg_total,
      `Rchain.Acceptance.abort_refunds_the_payer_exactly,
      `Rchain.Acceptance.abort_conserves_the_leg_total,
      `Rchain.Acceptance.commitThatKeepsTheEscrow,
      `Rchain.Acceptance.commitWithCredit,
      `Rchain.Acceptance.a_commit_that_keeps_the_escrow_creates_value,
      `Rchain.Acceptance.an_overcredit_creates_value],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    rustWitness := ["rholang/src/native_state.rs:txn_prepare_commit_round_trip",
      "rholang/src/native_state.rs:txn_prepare_abort_returns_escrow",
      "casper/tests/gateway_txn.rs:gateway_aborts_every_leg_when_one_cannot_prepare",
      "casper/tests/cross_shard_txn.rs:two_shard_2pc_commits_all"],
    witness := [`Rchain.Acceptance.commit_credits_the_payee_exactly,
      `Rchain.Acceptance.commit_conserves_the_leg_total,
      `Rchain.Acceptance.abort_refunds_the_payer_exactly,
      `Rchain.Acceptance.abort_conserves_the_leg_total,
      `Rchain.Acceptance.a_commit_that_keeps_the_escrow_creates_value,
      `Rchain.Acceptance.an_overcredit_creates_value],
    falsifiable := "Credit `amount + 1` and `an_overcredit_creates_value` holds — one unit is \
      created; credit the payee while leaving the record `prepared` and \
      `a_commit_that_keeps_the_escrow_creates_value` holds — the escrow is counted twice. In Rust, \
      the `100 -> 60 -> 40` commit and the `100 -> 70 -> 100` abort are the two round-trip \
      witnesses, and the abort path restores shard A while leaving the destination empty.",
    note := "**What this law does not claim.** Idempotence is Law 28's, cited \
      (`Rchain.txnCommit_idempotent`, `Rchain.txnAbort_idempotent`); the `checked_add` overflow \
      refusal is a Rust refinement not modelled. The account-untouched half needs `src != dst`, \
      because a self-payment's commit credits the payer's own balance." },

  { number := 98, clause := "c", layer := "Casper",
    statement := "**The instance is the sum of its legs, under one decision.** A committed instance \
      credits each destination by its leg's escrow; an aborted instance refunds each source; the \
      summed totals equal the per-leg totals, so no leg's move is counted twice and no value is \
      created or lost. The one decision is Law 27's, cited.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.Book, `Rchain.Acceptance.Book.commit,
      `Rchain.Acceptance.Book.abort, `Rchain.Acceptance.Book.total,
      `Rchain.Acceptance.listSum, `Rchain.Acceptance.a_commit_conserves_one_leg,
      `Rchain.Acceptance.a_committed_instance_conserves_the_totals,
      `Rchain.Acceptance.a_committed_instance_credits_the_leg_amounts,
      `Rchain.Acceptance.an_abort_conserves_one_leg,
      `Rchain.Acceptance.an_aborted_instance_conserves_the_totals,
      `Rchain.Acceptance.an_aborted_instance_refunds_the_leg_amounts,
      `Rchain.Acceptance.Decision, `Rchain.Acceptance.mixedDecision,
      `Rchain.Acceptance.a_mixed_instance_is_not_one_decision,
      `Rchain.Acceptance.badCommit, `Rchain.Acceptance.a_double_credit_creates_value],
    axioms := [],
    rust := ["casper/src/gateway/mod.rs"],
    rustWitness := ["casper/tests/cross_shard_txn.rs:two_shard_2pc_commits_all",
      "casper/tests/cross_shard_txn.rs:two_shard_2pc_aborts_all_when_a_leg_fails",
      "casper/tests/gateway_txn.rs:gateway_moves_the_escrow_on_every_leg"],
    witness := [`Rchain.Acceptance.a_committed_instance_conserves_the_totals,
      `Rchain.Acceptance.a_committed_instance_credits_the_leg_amounts,
      `Rchain.Acceptance.an_aborted_instance_conserves_the_totals,
      `Rchain.Acceptance.an_aborted_instance_refunds_the_leg_amounts,
      `Rchain.Acceptance.a_mixed_instance_is_not_one_decision,
      `Rchain.Acceptance.a_double_credit_creates_value],
    falsifiable := "Apply commit to one leg and abort to another (`mixedDecision`): \
      `a_mixed_instance_is_not_one_decision` is the decided counterexample, and the instance is no \
      longer one decision's. Credit both accounts from the escrow (`badCommit`): the instance's \
      total grows by the escrow, which `a_double_credit_creates_value` states. The Rust \
      commit-all and abort-all witnesses pin the two arms across the two shards.",
    note := "The `Book` is the accounting shape after 98a/98b's bridge from the participant verbs; \
      it does not model phase-two delivery failure or the coordinator's retry loop (99 covers the \
      ordering; C161 records the delivery residual). The one decision is Law 27's \
      (`Rchain.every_leg_reaches_the_one_decision`), and the once-only move is Law 28's, both \
      cited rather than reproved." },

  { number := 99, clause := "a", layer := "Casper",
    statement := "**No phase-two effect without a terminal decision.** An undecided coordinator \
      record applies nothing; a decided one applies the single verb to exactly the legs that voted \
      ready. The decision is durable before the effect (`drive` writes the record, then \
      `apply_phase_two`), so a crash between them cannot deliver a decision that was never \
      recorded.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.hasVote, `Rchain.Acceptance.voteFor,
      `Rchain.Acceptance.phaseTwo, `Rchain.Acceptance.phaseTwoUnguarded,
      `Rchain.Acceptance.readyRecord, `Rchain.Acceptance.pendingRecord,
      `Rchain.Acceptance.an_undecided_record_applies_nothing,
      `Rchain.Acceptance.a_decided_record_applies_the_ready_legs,
      `Rchain.Acceptance.a_phase_two_without_the_terminal_guard_applies_before_a_decision],
    axioms := [],
    rust := ["casper/src/gateway/mod.rs", "casper/src/gateway/ledger.rs"],
    rustWitness := ["casper/src/gateway/mod.rs:apply_phase_two_returns_early_for_an_undecided_record",
      "casper/src/gateway/mod.rs:apply_phase_two_skips_legs_that_did_not_vote_ready",
      "casper/tests/gateway_faults.rs:a_decision_that_cannot_be_written_sends_no_phase_two"],
    witness := [`Rchain.Acceptance.an_undecided_record_applies_nothing,
      `Rchain.Acceptance.a_decided_record_applies_the_ready_legs,
      `Rchain.Acceptance.a_phase_two_without_the_terminal_guard_applies_before_a_decision],
    falsifiable := "Drop the `state.IsTerminal` guard: `phaseTwoUnguarded` applies a verb to a \
      `prepared` record's ready legs before a decision exists \
      (`a_phase_two_without_the_terminal_guard_applies_before_a_decision`). In Rust, the ledger \
      write that would carry the decision is failed in \
      `a_decision_that_cannot_be_written_sends_no_phase_two`, which asserts the prepare deploy \
      happened and no commit deploy did.",
    note := "The model's `phaseTwo` returns the shards a verb is sent to, not the participant \
      outcomes; it does not claim a failed phase-two delivery is retried (C161's residual). It \
      cites Law 29's absorbing terminal record for why the durable decision cannot be moved after \
      it is written." },

  { number := 99, clause := "b", layer := "Casper",
    statement := "**Every drive reaches a terminal state.** Phase one records a vote per leg; a \
      record still undecided afterwards — including a record with no legs — aborts, so `drive` never \
      returns a `prepared` record. The fallback is the branch that makes recovery total.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.prepareLoop, `Rchain.Acceptance.prepareAll,
      `Rchain.Acceptance.decideRecord, `Rchain.Acceptance.drive,
      `Rchain.Acceptance.driveNoFallback, `Rchain.Acceptance.leglessRecord,
      `Rchain.Acceptance.a_drive_reaches_a_terminal_state,
      `Rchain.Acceptance.a_legless_drive_aborts,
      `Rchain.Acceptance.a_drive_without_the_fallback_can_stay_undecided],
    axioms := [],
    rust := ["casper/src/gateway/mod.rs", "casper/src/gateway/ledger.rs"],
    rustWitness := ["casper/tests/gateway_faults.rs:drive_aborts_a_legless_record",
      "casper/tests/gateway_faults.rs:recover_in_flight_aborts_when_a_leg_errors"],
    witness := [`Rchain.Acceptance.a_drive_reaches_a_terminal_state,
      `Rchain.Acceptance.a_legless_drive_aborts,
      `Rchain.Acceptance.a_drive_without_the_fallback_can_stay_undecided],
    falsifiable := "Remove the fallback (`driveNoFallback`): a legless record stays `prepared`, \
      which `a_drive_without_the_fallback_can_stay_undecided` states. The Rust \
      `drive_aborts_a_legless_record` asserts the abort and its reason.",
    note := "The fallback is `drive`'s `record.state = Aborted` branch \
      (`casper/src/gateway/mod.rs:394-401`), reached only by a record no leg list explains. The \
      theorem is a branch property; its non-vacuity is the unguarded contrast, and Law 29's \
      absorbing record supplies `a decided record is not re-decided`." },

  { number := 99, clause := "c", layer := "Casper",
    statement := "**A terminal record is returned untouched and contacts nobody.** `resume`/`run` \
      on a decided transaction returns the record it found with an empty send list, so a retried \
      request or a restart recovery cannot move funds again. The participant balance is unchanged \
      because no verb is sent, which is Law 28's idempotence at the second line of defence.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.resume, `Rchain.Acceptance.resumeUnguarded,
      `Rchain.Acceptance.a_terminal_record_is_returned_untouched,
      `Rchain.Acceptance.a_terminal_resume_contacts_nobody,
      `Rchain.Acceptance.a_resume_without_the_terminal_guard_contacts_a_decided_transaction],
    axioms := [],
    rust := ["casper/src/gateway/mod.rs"],
    rustWitness := ["casper/tests/gateway_txn.rs:gateway_is_idempotent_under_txn_id",
      "casper/tests/gateway_faults.rs:a_failed_phase_two_leaves_the_decision_intact",
      "casper/tests/gateway_txn.rs:gateway_recovers_an_in_flight_transaction_from_the_ledger"],
    witness := [`Rchain.Acceptance.a_terminal_record_is_returned_untouched,
      `Rchain.Acceptance.a_terminal_resume_contacts_nobody,
      `Rchain.Acceptance.a_resume_without_the_terminal_guard_contacts_a_decided_transaction],
    falsifiable := "Without the terminal return, `resumeUnguarded` returns a non-empty send list \
      for a decided record (`a_resume_without_the_terminal_guard_contacts_a_decided_transaction`). \
      In Rust, `gateway_is_idempotent_under_txn_id` asserts the shard height does not move on a \
      third run.",
    note := "The lever is `run_under_lock`'s `existing.state.is_terminal() -> return Ok(existing)` \
      (`casper/src/gateway/mod.rs:292-296`); the model returns a send list rather than balances, so \
      the balance half is Law 28's participant idempotence, cited. `a_failed_phase_two...` shows \
      the complementary direction: a failed delivery does not undo a durable decision." },

  { number := 99, clause := "d", layer := "Casper",
    statement := "**A resume never re-prepares a leg that already voted.** The phase-one loop skips \
      a shard already in the vote set, so votes are monotone and a crash mid-phase-one cannot escrow \
      a second time. The leg that already voted is not contacted; only the pending legs are.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.prepareLoop, `Rchain.Acceptance.sentBy,
      `Rchain.Acceptance.sentByUnguarded,
      `Rchain.Acceptance.prepareLoop_never_sends_initial_voted,
      `Rchain.Acceptance.a_voted_leg_is_never_prepared,
      `Rchain.Acceptance.a_resume_does_not_re_prepare_the_voted_leg,
      `Rchain.Acceptance.a_resume_sends_only_the_pending_legs,
      `Rchain.Acceptance.an_unguarded_prepare_re_prepares_a_voted_leg],
    axioms := [],
    rust := ["casper/src/gateway/mod.rs"],
    rustWitness := ["casper/tests/gateway_faults.rs:run_resumes_an_existing_non_terminal_record",
      "casper/tests/gateway_txn.rs:concurrent_calls_for_one_txn_id_open_the_transaction_once"],
    witness := [`Rchain.Acceptance.prepareLoop_never_sends_initial_voted,
      `Rchain.Acceptance.a_voted_leg_is_never_prepared,
      `Rchain.Acceptance.a_resume_does_not_re_prepare_the_voted_leg,
      `Rchain.Acceptance.a_resume_sends_only_the_pending_legs,
      `Rchain.Acceptance.an_unguarded_prepare_re_prepares_a_voted_leg],
    falsifiable := "Remove the skip (`sentByUnguarded`): a voted shard is sent a prepare again \
      (`an_unguarded_prepare_re_prepares_a_voted_leg`). In Rust, \
      `run_resumes_an_existing_non_terminal_record` asserts the already-voted leg's deploy list \
      contains no `prepare` while the pending leg gets prepare then commit.",
    note := "The loop carries the voted set explicitly rather than reading it back from \
      `recordVote`, and the skipped branch returns the record unchanged, so a vote is never \
      rewritten. The general invariant is \
      `prepareLoop_never_sends_initial_voted`; the two decided positives are the finite witnesses \
      the Rust test mirrors." },
]

end Laws
end Rchain
