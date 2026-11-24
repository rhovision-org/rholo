import Rchain.Laws.Base

/-!
# The law register, chunk 09 (laws 111–128)

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

def lawsChunk09 : List Law := [
  { number := 111, layer := "Sync",
    statement := "**Settlement is determined by the possession-challenge outcome.** A paid period \
      ends with the caller's possession challenge: a pass carries the unused fee and leaves the \
      archive selected; a failure forfeits the period and marks the peer, excluding it from selection \
      until the operator's cooldown. The two outcomes are observably different, so a settlement \
      cannot be confused with the other one, and neither is a consensus event.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Settlement.a_failed_challenge_forfeits,
      `Rchain.Sync.Settlement.a_passed_challenge_refunds,
      `Rchain.Sync.Settlement.settlement_is_determined,
      `Rchain.Sync.Settlement.the_two_outcomes_differ],
    axioms := [],
    rust := ["block-storage/src/history_meter.rs"],
    rustWitness := [
      "block-storage/src/history_meter.rs:a_failed_challenge_forfeits_the_period_and_marks_the_peer",
      "block-storage/src/history_meter.rs:a_passed_challenge_carries_the_unused_credit"],
    witness := [`Rchain.Sync.Settlement.a_failed_challenge_forfeits,
      `Rchain.Sync.Settlement.the_two_outcomes_differ],
    falsifiable := "Make `settle` constant and `the_two_outcomes_differ` fails; swap its branches and \
      both the forfeit and the carry theorems fail. In Rust, the meter tests redden if a failed \
      challenge leaves credit or does not mark the peer, or if a pass forfeits.",
    note := "**Forfeiture is service-level, not a slash.** The direct REV payment is not reversed \
      on-chain; the consequence is non-renewal, credit write-off and selection exclusion. The \
      challenge proves possession at challenge time (Law 107's boundary), so the settlement is about \
      the period the challenge covers, not retention for ever. **What this law does not claim:** the \
      consumer trusts the archive it chose; the settlement key's signature and expiry are verified, \
      not the on-chain transfer." },

  { number := 112, layer := "Sync",
    statement := "**An accepted audit claim is one this node reproduced.** A signed audit \
      attestation names the checkpoint, the state root the replay derived, a replay head over the \
      anchor's canonical ancestor closure, and a challenge nonce; a node accepts it only when its \
      own replay agrees on the root and the head, and the claim extends its signer's chain \
      (`previous`/`sequence`). A mismatched claim is refused rather than stored, and an unchained \
      claim cannot extend a head.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Claim.only_a_reproduced_claim_is_accepted,
      `Rchain.Sync.Claim.a_mismatched_claim_is_refused,
      `Rchain.Sync.Claim.a_reproduced_claim_is_accepted,
      `Rchain.Sync.Claim.an_unchained_claim_is_refused,
      `Rchain.Sync.Claim.a_chained_claim_extends,
      `Rchain.Sync.ReplayState.TargetShape,
      `Rchain.Sync.ReplayState.an_anchored_target_is_the_only_accepted_shape],
    axioms := [],
    rust := ["models/src/casper/protocol/audit.rs", "casper/src/audit.rs",
      "node/src/runtime/audit.rs"],
    rustWitness := [
      "casper/src/audit.rs:a_reproduced_claim_is_accepted",
      "casper/src/audit.rs:a_chained_claim_extends_the_head_and_an_unchained_one_is_refused",
      "node/src/runtime/audit.rs:a_multi_member_fringe_targets_the_anchor_alone"],
    witness := [`Rchain.Sync.Claim.only_a_reproduced_claim_is_accepted,
      `Rchain.Sync.Claim.an_unchained_claim_is_refused],
    falsifiable := "Make `accept` total (always `some`) and `a_mismatched_claim_is_refused` fails; \
      ignore `previous` in `chain` and `an_unchained_claim_is_refused` fails. In Rust, the verifier \
      tests redden if a claim whose derived root or replay head differs from the local replay is \
      accepted, if another key's signature verifies, if a replayed nonce is accepted, or if an \
      unchained claim extends a head.",
    note := "**The statement and its encoding are separate from acceptance.** `AuditAttestation` \
      fixes the claim and its canonical bytes (a golden digest pins the encoding); this law is the \
      acceptance relation over a local reproduction. A signature alone is not acceptance: the \
      accepted store's type has no public constructor, so an unverified claim is not representable. \
      **What this law does not claim:** the replay is correct (Law 106 is the audit's soundness); \
      the nonce's freshness and the request rate limit are the service's (Law 110's accounting), \
      not this law; and a claim is not a zero-knowledge proof — `witnessType` is part of the signed \
      preimage so a future witness can be added, but none is claimed here." },
  { number := 113, layer := "Casper",
    statement := "**A merge above a finalised base does not undo a tuple-space write.** The multi-parent \
      merge applies only the **kept** half of its conflict scope to the finalised base, so a datum \
      present at that base survives unless a kept chain removed it — a later accepted block consumed \
      it — and the rejected half is never applied. **This is the qualified law**: the unqualified \
      reading, that a finalised write is never removed, is false, and \
      `merging_a_tuple_space_rmw_block_duplicates_its_produce` witnesses a kept chain consuming a base \
      datum. **Native absolute writes are not covered**, and the unqualified law is refuted there: a \
      stale conflict-scope block's whole-slot `Put` can overwrite a finalised boundary value. **The \
      guard is bounded** (`Rchain.MergeScope`): the final scope is the trimmed finalised layer \
      `between(final_fringe, prune_fringe)`; a conflict-scope block that does not descend from the \
      final fringe is a **fork**, and its chains are rejected rather than merged against a replayed \
      history. The whole finalised closure is never read, and the scope is a function of the DAG, not \
      of which states the local node holds. The genesis fallback is keyed on the final fringe. \
      **The base is guarded by agreement.** The Rust reader compares the finalisers' \
      `fringe_state_hash` values and refuses a disagreement (`choose_base_claim`); `Agree`/`pickBase` \
      model that guard, and `pickBase_agrees` is the theorem a silently-picked stale base cannot \
      satisfy. **A finalising merge applies no fork rule:** every conflict-scope block of the fringe \
      merge is in `seen(merge_fringe)`, so `guardedForkBlocks true = ∅`; the conflict merge keeps the \
      fork rule. **The ledger excludes what the final scope accepted:** \
      `guardedRejected accepted rejected` is the rejected set minus the accepted set, and \
      `a_finalised_deploy_is_not_in_the_ledger` is the set-level form of the Rust \
      `finalised_floor` refusal. A finalised block is outside `conflictScope` and `forkBlocks` \
      (`FinalisedBy`).",
    status := .provedModel,
    formal := `Rchain.FinalizedMerge.law113_statement,
    mutant := `Rchain.FinalizedMerge.law113_mutant,
    falsifier := `Rchain.FinalizedMerge.law113_falsifier,
    declarations := [`Rchain.FinalizedMerge.State, `Rchain.FinalizedMerge.Change,
      `Rchain.FinalizedMerge.applyKept, `Rchain.FinalizedMerge.law113_statement,
      `Rchain.FinalizedMerge.law113, `Rchain.FinalizedMerge.law113_mutant,
      `Rchain.FinalizedMerge.law113_falsifier,
      `Rchain.FinalizedMerge.Claim, `Rchain.FinalizedMerge.pickClaim,
      `Rchain.FinalizedMerge.pickClaim_mem, `Rchain.FinalizedMerge.Agree,
      `Rchain.FinalizedMerge.pickBase, `Rchain.FinalizedMerge.pickBase_none_of_disagree,
      `Rchain.FinalizedMerge.pickBase_agrees, `Rchain.FinalizedMerge.pickBase_mem,
      `Rchain.FinalizedMerge.guardedRejected,
      `Rchain.FinalizedMerge.a_finalised_deploy_is_not_in_the_ledger,
      `Rchain.FinalizedMerge.unguarded_ledger_can_name_a_finalised_deploy,
      `Rchain.MergeScope.finalScope, `Rchain.MergeScope.KeptRule,
      `Rchain.MergeScope.kept_chains_descend,
      `Rchain.MergeScope.a_non_descendant_is_not_kept,
      `Rchain.MergeScope.final_scope_is_the_trimmed_layer,
      `Rchain.MergeScope.widening_reads_the_closure,
      `Rchain.MergeScope.scope_is_a_function_of_the_dag,
      `Rchain.MergeScope.FinalisedBy,
      `Rchain.MergeScope.a_finalised_block_is_not_in_the_conflict_scope,
      `Rchain.MergeScope.a_finalised_block_is_not_a_fork,
      `Rchain.MergeScope.conflict_scope_is_seen_by_the_merge_fringe,
      `Rchain.MergeScope.guardedForkBlocks,
      `Rchain.MergeScope.finalising_merge_has_no_forks,
      `Rchain.MergeDecision.Decision, `Rchain.MergeDecision.Outcome,
      `Rchain.MergeDecision.Safe, `Rchain.MergeDecision.decide,
      `Rchain.MergeDecision.safe_merge_preserves_every_finalised_deploy,
      `Rchain.MergeDecision.merge_decision_is_total],
    axioms := [],
    rust := ["casper/src/merging.rs", "casper/src/multi_parent_casper.rs",
      "rspace/src/merger/state_change.rs", "rspace/src/merger/state_change_merger.rs"],
    rustWitness := [
      "casper/tests/n420_boundary_merge.rs:a_boundary_round_merge_keeps_the_base_states_user_write",
      "casper/tests/n420_boundary_merge.rs:merging_a_tuple_space_rmw_block_duplicates_its_produce",
      "casper/tests/merge_determinism.rs:system_deploy_ids_are_not_final_scope_rejections",
      "casper/tests/n420_boundary_merge.rs:a_final_scope_system_deploy_chain_is_not_rejected",
      "casper/src/merging.rs:a_non_empty_final_fringe_never_falls_back_to_genesis",
      "casper/src/merging.rs:a_non_descendant_conflict_block_is_a_fork_and_the_final_scope_stays_trimmed",
      "casper/src/merging.rs:a_fringe_merge_never_names_a_fork",
      "casper/src/merging.rs:a_conflict_merge_still_names_a_non_descendant_fork",
      "casper/src/merging.rs:the_finalised_floor_refuses_a_rejected_finalised_host",
      "casper/src/multi_parent_casper.rs:disagreeing_claims_are_refused_not_tie_broken",
      "casper/src/multi_parent_casper.rs:a_certified_base_wins_over_disagreeing_claims",
      "casper/tests/trace_replay.rs:an_agreeing_trace_replays_to_the_agreed_base"],
    witness := [`Rchain.FinalizedMerge.law113_falsifier,
      `Rchain.FinalizedMerge.the_same_fixture_survives_under_the_law,
      `Rchain.FinalizedMerge.pickClaim_mem,
      `Rchain.FinalizedMerge.pickBase_none_of_disagree,
      `Rchain.FinalizedMerge.pickBase_agrees,
      `Rchain.FinalizedMerge.pickBase_mem,
      `Rchain.FinalizedMerge.a_finalised_deploy_is_not_in_the_ledger,
      `Rchain.FinalizedMerge.unguarded_ledger_can_name_a_finalised_deploy,
      `Rchain.MergeScope.final_scope_is_the_trimmed_layer,
      `Rchain.MergeScope.kept_chains_descend,
      `Rchain.MergeScope.a_non_descendant_is_not_kept,
      `Rchain.MergeScope.widening_reads_the_closure,
      `Rchain.MergeScope.scope_is_a_function_of_the_dag,
      `Rchain.MergeScope.a_finalised_block_is_not_in_the_conflict_scope,
      `Rchain.MergeScope.a_finalised_block_is_not_a_fork,
      `Rchain.MergeScope.conflict_scope_is_seen_by_the_merge_fringe,
      `Rchain.MergeScope.finalising_merge_has_no_forks,
      `Rchain.MergeDecision.safe_merge_preserves_every_finalised_deploy,
      `Rchain.MergeDecision.merge_decision_is_total,
      `Rchain.MergeDecision.an_unguarded_decision_can_reject_a_finalised_deploy],
    falsifiable := "`law113_mutant` applies the **rejected** half of the scope as well; \
      `law113_falsifier` exhibits a rejected chain's removal dropping a datum the kept slice preserved. \
      The code applies `to_merge` alone, so the mutation is `compute_merged_state(&conflict_set)`. \
      **The law's own qualifier is falsified by the code's existing consume case**: \
      `merging_a_tuple_space_rmw_block_duplicates_its_produce` has a kept chain remove a base datum, \
      which the law allows and the unqualified reading forbids.",
    note := "**The scope, and what is now enforced.** The tuple-space half is proved: the applied slice \
      is the kept conflict scope (`merging.rs:2049-2071`, `:2122-2129`), the base is its input not an \
      output, and `net_datum_change` cancels a produce-and-consume inside the batch. **The native \
      half is bounded by the scope rule** (C400's successor, `Rchain.MergeScope`): the final scope is \
      the trimmed finalised layer `between(final_fringe, prune_fringe)`; a conflict-scope block that \
      does not descend from the final fringe is a fork, and its chains are rejected before the search. \
      The finalised closure is never indexed. This is the rule the 2026-11-17 cold-rejoin run forced: \
      the C400 widening read the whole certified closure, and a checkpoint joiner could not hydrate \
      its historical pre-states (`unknown root`). The fork set and the scope are functions of the DAG \
      (`seen`), so every producer and joiner derives the same scope; a node that lacks a pre-state \
      refuses rather than deriving a different one. The genesis fallback keys on \
      `final_fringe.is_empty()` (`merging.rs:1916-1922`). **The C344 repair (Phase 1) adds the system-chain rule:** a \
      finaliser's `rejected_deploys` can name a close chain by its 33-byte `blockHash ++ 02` id, and \
      the merge's rejection test reads a chain's first id — so a system id would reject a \
      consensus-mandated close chain and drop its native writes (the `pos:liveness-anchor` slot is \
      the measured case). `final_scope_rejections` does not propagate a system id, and `merge` \
      refuses to reject a system-deploy chain; the two tests above pin the derivation and the merge \
      decision. **The guarded model is the Rust tie:** `guardedForkBlocks` is `MergeMode::Fringe`'s \
      empty fork set, `guardedRejected` is `finalised_floor`'s subtraction, and `pickBase` is \
      `choose_base_claim`'s agreement refusal; `a_fringe_merge_never_names_a_fork`, \
      `the_finalised_floor_refuses_a_rejected_finalised_host` and \
      `disagreeing_claims_are_refused_not_tie_broken` are their Rust witnesses. Native slots remain \
      a code rule over `NativeStoreAction`, and lifting the local invariant to every honest node \
      assumes the same fewer-than-one-third bound as Law 104." },

  { number := 114, layer := "Sync",
    statement := "**The certificate statement determines the join input set, and an incomplete \
      joiner is not representable.** The bodies a joiner must hold are the expanded replay layer — \
      the anchor, the certificate's finalised fringe, the anchor header's own fringe, and every \
      replay-layer member's own fringe — plus every walk header strictly above \
      `anchor_height - DEPLOY_LIFESPAN`, the deploy-duplication window the first post-anchor block's \
      `repeat_deploy` scan reads. The roots are the pre-, post- and fringe-state roots of the \
      expanded replay layer only; the deploy-window bodies are fetched, not hydrated. The set is a \
      function of what the statement commits, not of the chain's arbitrary ancestry. Checkpoint v3 \
      commits the fringe members' headers under `fringeHeadersRoot`, which is how the members' \
      fringes are obtained. Acceptance contains the required bodies and roots. A certificate pinned \
      before its broadcast has exactly one sync target, its anchor; a later broadcast that is not \
      `[anchor]` is ignored.",
    status := .provedModel,
    formal := `Rchain.Sync.JoinInputs.law114_statement,
    mutant := `Rchain.Sync.JoinInputs.law114_mutant,
    falsifier := `Rchain.Sync.JoinInputs.law114_falsifier,
    declarations := [`Rchain.Sync.JoinInputs.Statement,
      `Rchain.Sync.JoinInputs.replayLayer,
      `Rchain.Sync.JoinInputs.expandedReplayLayer,
      `Rchain.Sync.JoinInputs.deployWindowBodies,
      `Rchain.Sync.JoinInputs.requiredBodies,
      `Rchain.Sync.JoinInputs.requiredRoots,
      `Rchain.Sync.JoinInputs.v2RequiredBodies,
      `Rchain.Sync.JoinInputs.Accepted,
      `Rchain.Sync.JoinInputs.an_incomplete_joiner_is_not_representable,
      `Rchain.Sync.JoinInputs.an_unheld_root_is_not_representable,
      `Rchain.Sync.JoinInputs.a_missing_deploy_window_body_is_not_representable,
      `Rchain.Sync.JoinInputs.the_statement_determines_the_join_inputs,
      `Rchain.Sync.JoinInputs.law114_deploy_window_mutant,
      `Rchain.Sync.JoinInputs.law114_deploy_window_falsifier,
      `Rchain.Sync.JoinInputs.the_deploy_window_is_closed_under_parents,
      `Rchain.Sync.JoinInputs.Target,
      `Rchain.Sync.JoinInputs.pin,
      `Rchain.Sync.JoinInputs.onFringeBroadcast,
      `Rchain.Sync.JoinInputs.a_pinned_certificate_targets_exactly_its_anchor,
      `Rchain.Sync.JoinInputs.law114_statement,
      `Rchain.Sync.JoinInputs.law114,
      `Rchain.Sync.JoinInputs.law114_mutant,
      `Rchain.Sync.JoinInputs.law114_falsifier],
    axioms := [],
    rust := ["casper/src/engine/node_syncing.rs", "casper/src/checkpoint.rs",
      "models/src/casper/protocol/checkpoint.rs", "casper/src/validate.rs"],
    rustWitness := [
      "casper/tests/checkpoint_join.rs:a_wiped_joiner_validates_the_first_post_anchor_block_from_the_certificate_and_join_bodies",
      "casper/src/checkpoint.rs:a_v3_statement_commits_the_fringe_headers_and_never_aliases_v2",
      "casper/src/engine/node_syncing.rs:the_join_input_set_is_the_anchor_its_fringe_and_the_deploy_window",
      "casper/src/engine/node_syncing.rs:a_pinned_certificate_does_not_follow_a_later_broadcast_fringe"],
    witness := [`Rchain.Sync.JoinInputs.an_incomplete_joiner_is_not_representable,
      `Rchain.Sync.JoinInputs.an_unheld_root_is_not_representable,
      `Rchain.Sync.JoinInputs.a_missing_deploy_window_body_is_not_representable,
      `Rchain.Sync.JoinInputs.the_statement_determines_the_join_inputs,
      `Rchain.Sync.JoinInputs.the_deploy_window_is_closed_under_parents,
      `Rchain.Sync.JoinInputs.a_pinned_certificate_targets_exactly_its_anchor,
      `Rchain.Sync.JoinInputs.law114_falsifier,
      `Rchain.Sync.JoinInputs.law114_deploy_window_falsifier],
    falsifiable := "Drop the members' fringes from the required set (the v2 statement) and \
      `law114_falsifier` exhibits a body the v3 statement requires and v2 does not; drop the deploy \
      window and `law114_deploy_window_falsifier` exhibits the body the first post-anchor block's \
      `repeat_deploy` scan reads; follow a later broadcast instead of the pinned anchor and \
      `a_pinned_certificate_targets_exactly_its_anchor` keeps the target at the anchor. The join test \
      then fails to validate the first post-anchor block.",
    note := "**Decisions (2026-11-17): the law was under-specified, not the Rust.** The Rust \
      `join_input_set` (`casper/src/engine/node_syncing.rs:1534-1599`) assembles the replay layer, \
      expands every replay-layer member's own fringe, collects the pre/post/fringe roots of the \
      expanded layer, and then adds every walk header above `anchor_height - DEPLOY_LIFESPAN` for the \
      deploy-duplication check. The law now models all of it: the bodies are the expanded replay \
      layer plus the deploy window; the roots are the expanded replay layer only. The deploy window \
      is parent-closed above the floor, which is exactly the `repeat_deploy` scan's stopping rule \
      (`casper/src/validate.rs:525-572`). The v3 `fringeHeadersRoot` is what makes the members' own \
      fringes available to a joiner whose ancestor walk excludes same-height siblings; the v2 \
      statement leaves that body underivable, which is the measured cold-join `unknown root`. The \
      pinned-target rule is the handshake: a certificate fixes the sync target to its anchor and a \
      later broadcast is ignored. `checkpoint_join` validates the first post-anchor block from \
      exactly this set. The bodies' authenticity is Laws 104/109; the quorum is Law 85's." },

  { number := 115, layer := "Casper",
    statement := "**The fork rule is deterministic and dependency-closed.** A conflict-scope block \
      that has not seen the whole final fringe is a fork; its chains are rejected, together with \
      every chain that depends on one. The fork set is a function of the DAG and the two fringes — \
      no hydrated state, no arrival order. The dependency closure is the least set containing the \
      forks and closed under the dependency map, and it is computed by the level-synchronous loop, \
      not assumed: the iteration reaches a fixed point inside a finite universe and the fixed point \
      satisfies the least-closure spec. Two closures of the same roots are equal, so a producer and \
      a joiner reject the same set. A conflict-scope block that does descend from the final fringe \
      is not a fork, and the closure-keyed alternative misclassifies it.",
    status := .provedModel,
    formal := `Rchain.MergeScope.law115_statement,
    mutant := `Rchain.MergeScope.law115_mutant,
    falsifier := `Rchain.MergeScope.law115_falsifier,
    declarations := [`Rchain.MergeScope.forkBlocks,
      `Rchain.MergeScope.fork_blocks_are_a_function_of_the_dag,
      `Rchain.MergeScope.a_fork_block_is_a_non_descendant_conflict_block,
      `Rchain.MergeScope.DependencyClosure,
      `Rchain.MergeScope.depStep,
      `Rchain.MergeScope.closeN,
      `Rchain.MergeScope.close,
      `Rchain.MergeScope.depStep_subset,
      `Rchain.MergeScope.depStep_mono,
      `Rchain.MergeScope.depStep_subset_U,
      `Rchain.MergeScope.depStep_subset_c,
      `Rchain.MergeScope.closeN_mono,
      `Rchain.MergeScope.closeN_contains,
      `Rchain.MergeScope.closeN_subset_U,
      `Rchain.MergeScope.closeN_least,
      `Rchain.MergeScope.closeN_succ_eq,
      `Rchain.MergeScope.closeN_fixed,
      `Rchain.MergeScope.closeN_eq_of_fixed,
      `Rchain.MergeScope.close_is_fixed,
      `Rchain.MergeScope.close_spec,
      `Rchain.MergeScope.rejection_is_closed_under_dependency,
      `Rchain.MergeScope.a_fork_block_is_rejected,
      `Rchain.MergeScope.the_dependency_closure_is_unique,
      `Rchain.MergeScope.closureKeyedFork,
      `Rchain.MergeScope.a_descendant_is_not_a_fork_but_the_closure_rule_calls_it_one,
      `Rchain.MergeScope.law115_statement,
      `Rchain.MergeScope.law115,
      `Rchain.MergeScope.law115_mutant,
      `Rchain.MergeScope.law115_falsifier],
    axioms := [],
    rust := ["casper/src/merging.rs", "sdk/src/dag/merging.rs"],
    rustWitness := [
      "casper/src/merging.rs:a_non_descendant_conflict_block_is_a_fork_and_the_final_scope_stays_trimmed",
      "sdk/src/dag/merging.rs:with_dependencies_collects_transitive_closure"],
    witness := [`Rchain.MergeScope.fork_blocks_are_a_function_of_the_dag,
      `Rchain.MergeScope.close_is_fixed,
      `Rchain.MergeScope.close_spec,
      `Rchain.MergeScope.rejection_is_closed_under_dependency,
      `Rchain.MergeScope.a_fork_block_is_rejected,
      `Rchain.MergeScope.the_dependency_closure_is_unique,
      `Rchain.MergeScope.law115_falsifier],
    falsifiable := "Key the fork set on the anchor's committed closure instead of descent and \
      `law115_falsifier` exhibits a conflict-scope block that descends from the final fringe yet is \
      rejected; the merge then drops a legitimate participant. Make the loop stop before the fixed \
      point and `close_is_fixed` fails; drop an edge from the closure and \
      `close_spec`'s `closed` field fails.",
    note := "The Rust `MergeScope.fork_blocks` (`casper/src/merging.rs:2054-2062`) is the conflict \
      scope minus the blocks that see the whole final fringe. `merge` collects those blocks' deploy \
      chains and applies `with_dependencies` (`sdk/src/dag/merging.rs:31-50`) to remove them and \
      their dependents from `to_merge` and add them to `rejected` (`casper/src/merging.rs:2236-2240`). \
      The Lean model is the loop itself: `depStep` is one level, `close` is `|U|` levels, and \
      `close_is_fixed` proves the iteration stops inside the finite universe; `close_spec` proves the \
      result contains the roots, is closed, and is least, so `the_dependency_closure_is_unique` \
      follows. The C400 widening read the finalised closure instead and made a checkpoint joiner \
      index historical pre-states; the qualified Law 113 records the native-write consequence. The \
      dependency map is the block/chain edge relation the Rust passes; the honest-quorum hypothesis \
      is Law 85's." },

  { number := 116, layer := "Sync",
    statement := "**The pre-replay gate admits exactly two arms, never faults with a window \
      installed, and never condemns a block whose inputs are unavailable.** A block is admitted to \
      replay iff it is not committed history and either its pre-state is the anchor root or a \
      **derived** root (a root whose state this node holds), or its justification set is nonempty and \
      every justification is the anchor or a validated block; or every justification is committed — \
      nonempty, held by the DAG, and at or below the anchor height. A same-height sibling the anchor \
      has not seen is not committed history. When the admitted arms do not apply, a nonempty \
      justification set containing a block that is neither the anchor nor validated is \
      **deferred** (`deferredAncestry`), not condemned: a divergence verdict requires the inputs to \
      be available. The only refusal left is the empty-justification shape. The only retryable fault \
      is the missing window.",
    status := .provedModel,
    formal := `Rchain.Sync.Admission.law116_statement,
    mutant := `Rchain.Sync.Admission.law116_mutant,
    falsifier := `Rchain.Sync.Admission.law116_falsifier,
    declarations := [`Rchain.Sync.Admission.Window,
      `Rchain.Sync.Admission.Verdict,
      `Rchain.Sync.Admission.permits_replay_root,
      `Rchain.Sync.Admission.permits,
      `Rchain.Sync.Admission.permits_justifications,
      `Rchain.Sync.Admission.justificationsAvailable,
      `Rchain.Sync.Admission.permits_block,
      `Rchain.Sync.Admission.justificationsCommitted,
      `Rchain.Sync.Admission.isCommittedHistory,
      `Rchain.Sync.Admission.admission,
      `Rchain.Sync.Admission.admissionRefusingMissing,
      `Rchain.Sync.Admission.admissionFaulting,
      `Rchain.Sync.Admission.no_admission_path_faults,
      `Rchain.Sync.Admission.a_missing_window_is_a_retryable_fault,
      `Rchain.Sync.Admission.admission_replays_iff,
      `Rchain.Sync.Admission.committed_history_is_refused,
      `Rchain.Sync.Admission.a_held_sibling_is_replayable,
      `Rchain.Sync.Admission.a_certified_unheld_root_is_refused_not_faulted,
      `Rchain.Sync.Admission.an_uncertified_root_is_refused,
      `Rchain.Sync.Admission.merged_pre_state_over_validated_justifications_is_replayed,
      `Rchain.Sync.Admission.all_justifications_committed_is_replayed,
      `Rchain.Sync.Admission.permits_is_the_root_or_certified,
      `Rchain.Sync.Admission.an_unavailable_justification_is_deferred,
      `Rchain.Sync.Admission.no_divergence_verdict_without_available_inputs,
      `Rchain.Sync.Admission.law116_statement,
      `Rchain.Sync.Admission.law116,
      `Rchain.Sync.Admission.law116_mutant,
      `Rchain.Sync.Admission.law116_falsifier,
      `Rchain.MergeScope.a_sibling_is_not_committed_history],
    axioms := [],
    rust := ["casper/src/blocks/block_processor.rs", "casper/src/engine/anchor.rs"],
    rustWitness := [
      "casper/src/engine/anchor.rs:a_certified_root_that_is_not_the_anchor_is_not_a_replay_base",
      "casper/src/engine/anchor.rs:a_checkpoint_anchor_admits_its_committed_fringe_as_justifications",
      "casper/src/blocks/block_processor.rs:the_pre_replay_gate_refuses_an_uncertified_historical_pre_state",
      "casper/src/blocks/block_processor.rs:the_pre_replay_gate_admits_a_merged_pre_state_over_validated_justifications",
      "casper/src/blocks/block_processor.rs:the_pre_replay_gate_honours_committed_justifications",
      "casper/src/blocks/block_processor.rs:a_missing_justification_is_deferred_not_failed"],
    witness := [`Rchain.Sync.Admission.no_admission_path_faults,
      `Rchain.Sync.Admission.admission_replays_iff,
      `Rchain.Sync.Admission.committed_history_is_refused,
      `Rchain.Sync.Admission.a_held_sibling_is_replayable,
      `Rchain.Sync.Admission.a_certified_unheld_root_is_refused_not_faulted,
      `Rchain.Sync.Admission.merged_pre_state_over_validated_justifications_is_replayed,
      `Rchain.Sync.Admission.all_justifications_committed_is_replayed,
      `Rchain.Sync.Admission.an_unavailable_justification_is_deferred,
      `Rchain.Sync.Admission.no_divergence_verdict_without_available_inputs,
      `Rchain.MergeScope.a_sibling_is_not_committed_history,
      `Rchain.Sync.Admission.law116_falsifier],
    falsifiable := "Restore the height-keyed pre-replay gate and the sibling is refused as \
      committed history (`a_sibling_is_not_committed_history` is the refutation); return the unheld \
      certified root to the fault branch (`law116_mutant`) and `law116_falsifier` exhibits the \
      internal `unknown root`; use the pre-fix gate for an unavailable justification \
      (`admissionRefusingMissing`) and `law116_falsifier` exhibits the divergence verdict the real \
      gate defers; drop the `justificationsCommitted` arm and \
      `all_justifications_committed_is_replayed` fails.",
    note := "**Decision (2026-11-17): the law was too coarse, not the Rust.** The Rust splits the \
      gate into `is_committed_history` (`casper/src/blocks/block_processor.rs:886-906`), which is \
      false for the anchor and for every block above it, and `permits_block` \
      (`casper/src/engine/anchor.rs:269`), which composes `permits_replay_root` and \
      `permits_justifications`. `pre_replay_refusal` refuses a committed-history block before replay; \
      it admits when `permits_block` is true **or** every justification is committed \
      (`block_processor.rs:862-880`). The law models the three predicates separately: `derived` is \
      the replay base, `certified` is named but not held, and the justifications arm is a height \
      rule. The measured failure was the opposite: a height-keyed gate refused the sibling and the \
      pipeline recorded `unknown root`. \
      **The deferral arm (RC-4 item C, 2026-11-21).** The drill measured the other direction: the \
      bootstrap missed a run of broadcasts (245–260), a later block's justifications were never \
      validated, and the gate recorded blocks 261–262 as `UncertifiedPreState` divergences; their \
      descendants inherited the failed parent and were condemned in turn. The fix is not a new \
      verdict for a real disagreement but the removal of a verdict without inputs: when every \
      justification is the anchor or validated, `permits_justifications` already replays, so the \
      refusal only ever described an empty set or an unavailable input. The Rust now defers that \
      case into the durable ledger and requests the missing ancestry \
      (`a_missing_justification_is_deferred_not_failed`); the pre-fix gate is `admissionRefusingMissing`. \
      The state roots are the quorum claim Law 85 owns." },

  { number := 117, layer := "Sync",
    statement := "**A signed checkpoint statement is honest about every field the joiner \
      verifies.** The statement signs the shard, the anchor, its height and post-state, the \
      finalised fringe and its state, the metadata root, the validation-window root and the \
      fringe-headers root. The joiner re-derives each from the material it fetched and refuses a \
      differing field: the anchor is the max `(height, hash)` member of the certificate's fringe \
      that the walk holds; its post-state equals the walked anchor header's; a present fringe state \
      equals the anchor header's committed fringe state; `metadataRoot` and `historyRoot` are \
      required and equal the commitments of the walked ancestors and window; a present \
      `fringeHeadersRoot` equals the commitment of the held fringe headers, and an absent one is \
      the legitimate v2 statement. A produced statement has a merge base: a nonempty finalised \
      fringe and a non-zero fringe state. The run root is the signed fringe state, not the anchor \
      block's post-state.",
    status := .provedModel,
    formal := `Rchain.Sync.Anchor.law117_statement,
    mutant := `Rchain.Sync.Anchor.law117_mutant,
    falsifier := `Rchain.Sync.Anchor.law117_falsifier,
    declarations := [`Rchain.Sync.Anchor.Header,
      `Rchain.Sync.Anchor.Statement,
      `Rchain.Sync.Anchor.Projection,
      `Rchain.Sync.Anchor.isMaxOf,
      `Rchain.Sync.Anchor.Verifies,
      `Rchain.Sync.Anchor.wellFormed,
      `Rchain.Sync.Anchor.the_anchor_is_the_max_held_fringe_member,
      `Rchain.Sync.Anchor.the_anchor_post_state_is_the_headers,
      `Rchain.Sync.Anchor.the_signed_fringe_state_is_the_headers,
      `Rchain.Sync.Anchor.the_metadata_root_is_the_ancestors_commitment,
      `Rchain.Sync.Anchor.the_history_root_is_the_windows_commitment,
      `Rchain.Sync.Anchor.the_fringe_headers_root_is_the_members_commitment,
      `Rchain.Sync.Anchor.a_differing_metadata_root_is_refused,
      `Rchain.Sync.Anchor.a_differing_history_root_is_refused,
      `Rchain.Sync.Anchor.a_missing_metadata_root_is_refused,
      `Rchain.Sync.Anchor.a_missing_history_root_is_refused,
      `Rchain.Sync.Anchor.a_differing_fringe_state_is_refused,
      `Rchain.Sync.Anchor.a_differing_fringe_headers_root_is_refused,
      `Rchain.Sync.Anchor.an_absent_fringe_headers_root_does_not_refuse,
      `Rchain.Sync.Anchor.an_anchor_without_a_finalised_fringe_is_not_well_formed,
      `Rchain.Sync.Anchor.an_anchor_with_a_zero_fringe_state_is_not_well_formed,
      `Rchain.Sync.Anchor.law117_statement,
      `Rchain.Sync.Anchor.law117,
      `Rchain.Sync.Anchor.law117_mutant,
      `Rchain.Sync.Anchor.law117_falsifier],
    axioms := [],
    rust := ["casper/src/api/block_api_impl.rs", "casper/src/engine/anchor.rs",
      "casper/src/engine/node_syncing.rs"],
    rustWitness := [
      "casper/src/engine/anchor.rs:a_checkpoint_root_is_the_certified_fringe_state",
      "casper/src/checkpoint.rs:a_v3_statement_commits_the_fringe_headers_and_never_aliases_v2",
      "casper/src/checkpoint.rs:a_validation_window_is_bound_and_verifies",
      "casper/src/checkpoint.rs:a_metadata_commitment_is_bound_and_verifies",
      "casper/src/checkpoint.rs:the_fringe_claim_is_committed_per_header"],
    witness := [`Rchain.Sync.Anchor.the_anchor_is_the_max_held_fringe_member,
      `Rchain.Sync.Anchor.the_anchor_post_state_is_the_headers,
      `Rchain.Sync.Anchor.the_signed_fringe_state_is_the_headers,
      `Rchain.Sync.Anchor.the_metadata_root_is_the_ancestors_commitment,
      `Rchain.Sync.Anchor.the_history_root_is_the_windows_commitment,
      `Rchain.Sync.Anchor.the_fringe_headers_root_is_the_members_commitment,
      `Rchain.Sync.Anchor.a_differing_metadata_root_is_refused,
      `Rchain.Sync.Anchor.a_differing_history_root_is_refused,
      `Rchain.Sync.Anchor.a_differing_fringe_headers_root_is_refused,
      `Rchain.Sync.Anchor.law117,
      `Rchain.Sync.Anchor.law117_falsifier],
    falsifiable := "Sign a metadata root, history root or fringe-headers root that does not equal \
      the fetched material's commitment and the corresponding refusal theorem fires; sign the anchor \
      post-state instead of the fringe state (the pre-c405 shape) and the joiner's re-check refuses \
      it. `law117_mutant` is the statement whose signed metadata root differs from its projection, \
      and `law117_falsifier` proves no such statement verifies.",
    note := "`current_checkpoint` (`casper/src/api/block_api_impl.rs:1277-1357`) signs the anchor, \
      its height and post-state, the anchor header's fringe state, the finalised fringe, the \
      metadata root over the anchor's ancestor closure, the validation-window root and the v3 \
      fringe-headers root. The joiner re-checks every field: `verify_metadata_root` and \
      `verify_validation_window` require the roots; `verify_fringe_headers` accepts an absent root \
      (the v2 statement) and otherwise requires every fringe member's header and the matching \
      commitment. The anchor's post-state is identity/audit, not the run root; the run root is the \
      signed fringe state (Law 108d). A produced statement requires a nonempty fringe and a non-zero \
      fringe state — `current_checkpoint`'s 400 is this precondition, and the restore test's \
      readiness poll waits it out. The certificate's quorum is Law 104's; the quorum's honesty is \
      Law 85's." },

  { number := 118, layer := "Storage",
    statement := "**A build opens only a store format it can read.** The data directory carries a \
      `store-format` marker; the gate maps the marker and this build's version to one of five \
      outcomes — stamp an absent marker, accept this build's version, advance an older one, or \
      refuse a newer or unreadable one. The decision is total, and a marker newer than the build is \
      never opened: reading a format the binary does not know is how a downgrade silently becomes \
      an advance. The refusal happens **before any store is opened**, so an old build cannot migrate \
      or read a newer store; a stamp is written durably before the directory is trusted.",
    status := .provedModel,
    formal := `Rchain.Casper.StoreFormat.law118_statement,
    mutant := `Rchain.Casper.StoreFormat.law118_mutant,
    falsifier := `Rchain.Casper.StoreFormat.law118_falsifier,
    declarations := [`Rchain.Casper.StoreFormat.Marker,
      `Rchain.Casper.StoreFormat.Decision,
      `Rchain.Casper.StoreFormat.decide,
      `Rchain.Casper.StoreFormat.opens,
      `Rchain.Casper.StoreFormat.storeSafe,
      `Rchain.Casper.StoreFormat.decide_is_safe,
      `Rchain.Casper.StoreFormat.a_newer_store_is_never_opened,
      `Rchain.Casper.StoreFormat.store_decision_is_total,
      `Rchain.Casper.StoreFormat.opening_a_newer_store_is_the_unsafe_witness,
      `Rchain.Casper.StoreFormat.law118_statement,
      `Rchain.Casper.StoreFormat.law118,
      `Rchain.Casper.StoreFormat.law118_mutant,
      `Rchain.Casper.StoreFormat.law118_falsifier],
    axioms := [],
    rust := ["node/src/runtime/node_runtime.rs", "casper/src/storage.rs"],
    rustWitness := [
      "node/src/runtime/node_runtime.rs:a_store_from_a_newer_build_is_refused_and_this_build_stamps_what_it_opens",
      "node/src/runtime/node_runtime.rs:a_marker_is_written_before_the_directory_is_trusted",
      "casper/src/storage.rs:an_older_format_is_an_advance",
      "casper/src/storage.rs:a_newer_format_is_refused",
      "casper/tests/store_format_trace.rs:every_recorded_format_trace_replays_to_its_decision"],
    witness := [`Rchain.Casper.StoreFormat.decide_is_safe,
      `Rchain.Casper.StoreFormat.a_newer_store_is_never_opened,
      `Rchain.Casper.StoreFormat.store_decision_is_total,
      `Rchain.Casper.StoreFormat.opening_a_newer_store_is_the_unsafe_witness,
      `Rchain.Casper.StoreFormat.law118_falsifier],
    falsifiable := "Open any present marker (`openAny`): a store written by format 5 under a build \
      that writes 4 is accepted, which is `law118_mutant` and `law118_falsifier` exhibits it. In \
      Rust, `decide_store_format(Some(\"5\"), 4)` is `RefuseNewer`, and the probe refuses before \
      `block_store::create`; deleting the probe makes the store open first.",
    note := "The marker is not a schema hash and does not derive a version; the number is a promise \
      about the rows and is bumped by hand (`casper/src/storage.rs`). The gate cannot tell an \
      absent marker from a pre-marker store, so an absent marker is stamped with this build's \
      version and read — the model records that as `Stamp`, not as `Advance`. The \
      v3→v4 disposition-ledger copy is the migration that makes `Advance` safe; it is Laws 66/68 \
      and `block_disposition`'s own tests, not this law." },
  { number := 119, layer := "Casper",
    statement := "A duplicate or effect-free rejection is not a lost deploy: a rejected deploy whose \
      id is also in a kept chain, or whose chain has no state effect, is not in the lossy rejection \
      ledger. The merge kept a copy (or the deploy was an autopropose `Nil`), so no finalised write \
      is dropped.",
    status := .provedModel,
    formal := `Rchain.MergeDecision.law119_statement,
    mutant := `Rchain.MergeDecision.law119_mutant,
    falsifier := `Rchain.MergeDecision.law119_falsifier,
    declarations := [`Rchain.MergeDecision.DeployLedger,
      `Rchain.MergeDecision.lossyRejected,
      `Rchain.MergeDecision.lossyRejectedUnfiltered,
      `Rchain.MergeDecision.law119_statement,
      `Rchain.MergeDecision.law119,
      `Rchain.MergeDecision.law119_mutant,
      `Rchain.MergeDecision.law119_falsifier],
    axioms := [],
    rust := ["casper/src/merging.rs"],
    rustWitness := [
      "casper/src/merging.rs:a_duplicate_rejection_is_not_a_loss",
      "casper/src/merging.rs:a_unique_user_rejection_is_a_loss",
      "casper/src/merging.rs:an_effect_free_rejection_is_not_a_loss",
      "casper/src/merging.rs:a_system_rejection_is_not_a_loss"],
    witness := [`Rchain.MergeDecision.law119,
      `Rchain.MergeDecision.law119_falsifier],
    falsifiable := "Use `lossyRejectedUnfiltered` (the pre-fix ledger): it names a deploy id that is \
      in `kept`, which `law119_mutant` and `law119_falsifier` exhibit. In Rust, the duplicate \
      `rejected_user_hosts` entry fired `finalised_floor` on the RC-2 fringe; the four \
      `rejected_user_loss_hosts` tests keep the unique-loss case and exclude the duplicate, the \
      dummy and the system chain.",
    note := "The law's subject is the **deploy set**, not the chain set. The merge's conflict \
      resolution is unchanged: a shared deploy id remains a conflict, and the losing chain is still \
      rejected. What changes is the loss classification: a rejection whose deploy is kept elsewhere \
      is a dedup, and an effect-free rejection cannot drop a write. The finalised floor remains \
      fail-closed for a unique user deploy with effects; the negative test is the guard." },

  { number := 120, layer := "PoS",
    statement := "**A joining validator is not drawn into the active set until it participates.** At \
      an armed epoch boundary, a bonded validator that is not in the previous active set and is \
      absent from the participation map at the quorum tip (no entry, or strictly more than the \
      absence threshold behind it) is excluded from the active draw, so it is not in the finality \
      denominator. It enters at the first boundary after it participates. An unarmed network \
      (`jailAfter = 0` — the shipped format-2 record) excludes nobody and the draw is unchanged. \
      The bond and the staged withdrawal are untouched. The rule without the absence conjunct — \
      outside the previous active set alone — is the mutant: it excludes a joiner that has already \
      spoken, which is the wrong rule.",
    status := .provedModel,
    formal := `Rchain.Join.law120_statement,
    mutant := `Rchain.Join.law120_mutant,
    falsifier := `Rchain.Join.law120_falsifier,
    declarations := [`Rchain.Join.joining,
      `Rchain.Join.joiningUnfiltered,
      `Rchain.Join.joining_mem,
      `Rchain.Join.law120_statement,
      `Rchain.Join.law120,
      `Rchain.Join.law120_mutant,
      `Rchain.Join.law120_falsifier],
    axioms := [],
    rust := ["rholang/src/native_state.rs"],
    rustWitness := [
      "rholang/src/native_state.rs:a_joining_validator_is_excluded_from_the_active_draw_until_it_participates"],
    witness := [`Rchain.Join.law120, `Rchain.Join.law120_falsifier],
    falsifiable := "Use `joiningUnfiltered` (not-active alone): a validator that bonded and authored \
      a block is outside the previous active set and present, and the unfiltered rule excludes it \
      from the next boundary. `law120_mutant` and `law120_falsifier` exhibit the witness; the Rust \
      arm's first assertion is the joiner excluded while absent, its second is the same joiner \
      drawn after it speaks.",
    note := "**The ladder's second rung, on the existing timing.** Law 89 covers a validator that was \
      active and went silent; this law covers the one that has never been in the draw. \
      `apply_liveness_prelude` counts absence only for the previous active set, so a joiner is not \
      jailed — it must instead be kept out of the draw until it can participate. No new genesis \
      field is added: arming is the five RCHIP #55 parameters, and with them off the record is \
      format 2 and the draw is byte-for-byte the old one. The check reads the pool key set, the \
      previous active set and the quorum-agreed participation map — no local view and no wall \
      clock, so every node classifies the same way." },

  { number := 121, layer := "Sync",
    statement := "**A joined node is never `running` while its own validated tip is behind the peer \
      tip it has observed.** The phase that admits serving carries a proof that `peerTip - own` is \
      within the catch-up window, where `own` is the highest latest message this node has validated \
      and `peerTip` is the highest block number it observed from a connected peer. `supported` — the \
      quorum-supported height over the node's own validated messages — is a diagnostic, not the \
      test: a healthy leader's quorum height sits at a laggard's height. The pre-fix predicate \
      (`own - supported`) therefore both admitted the drill's frozen node (own 198, peer tip 335, \
      supported 198) and froze a healthy leader on a six-height skew (own 207, peer tip 207, \
      supported 201). A view outside the window is `catchingUp supported own`, naming both \
      coordinates; there is no third constructor, so running-and-behind is not writable. The \
      watchdog fails closed to `blocked` after `watchdogAfter` attempts without progress; a running \
      phase is never touched by it (continuity is Law 123). Timing is counted in observed attempts, \
      not wall clock.",
    status := .provedModel,
    formal := `Rchain.JoinCatchUp.law121_statement,
    mutant := `Rchain.JoinCatchUp.law121_mutant,
    falsifier := `Rchain.JoinCatchUp.law121_falsifier,
    declarations := [`Rchain.JoinCatchUp.View,
      `Rchain.JoinCatchUp.withinWindow,
      `Rchain.JoinCatchUp.withinWindowUnchecked,
      `Rchain.JoinCatchUp.Phase,
      `Rchain.JoinCatchUp.Phase.isRunning,
      `Rchain.JoinCatchUp.Phase.blocker,
      `Rchain.JoinCatchUp.afterRestore,
      `Rchain.JoinCatchUp.watchdog,
      `Rchain.JoinCatchUp.the_pre_fix_predicate_runs_the_frozen_node,
      `Rchain.JoinCatchUp.the_pre_fix_predicate_freezes_the_healthy_leader,
      `Rchain.JoinCatchUp.law121_statement,
      `Rchain.JoinCatchUp.law121,
      `Rchain.JoinCatchUp.LoosePhase,
      `Rchain.JoinCatchUp.afterRestoreUnchecked,
      `Rchain.JoinCatchUp.law121_mutant,
      `Rchain.JoinCatchUp.law121_falsifier],
    axioms := [],
    rust := ["casper/src/engine/node_phase.rs", "casper/src/engine/node_launch.rs"],
    rustWitness := [
      "casper/src/engine/catchup.rs:the_catch_up_window_refuses_the_drills_frozen_view",
      "casper/src/engine/catchup.rs:a_leader_with_a_lagging_quorum_is_not_frozen"],
    witness := [`Rchain.JoinCatchUp.law121, `Rchain.JoinCatchUp.law121_falsifier,
      `Rchain.JoinCatchUp.the_pre_fix_predicate_runs_the_frozen_node,
      `Rchain.JoinCatchUp.the_pre_fix_predicate_freezes_the_healthy_leader],
    falsifiable := "The pre-fix launcher — `afterRestoreUnchecked`, the Rust enum without the \
      running proof — returns `running` for the drill's own view; `law121_mutant` and \
      `law121_falsifier` exhibit it. The pre-fix predicate is wrong in both directions: \
      `the_pre_fix_predicate_runs_the_frozen_node` (own 198, peer 335, supported 198) and \
      `the_pre_fix_predicate_freezes_the_healthy_leader` (own 207, peer 207, supported 201). The \
      typed phase cannot express the first value, which is the closure: the bad state is unwritable, \
      not merely refused.",
    note := "**The silent freeze this law removes** (RC-4, 2026-11-21): the light-joined victim at \
      `84d43d8fe` restored the certified state, entered `running` at tip 183 while the other \
      validators were at 306, and stayed there for the harness's whole window — pipeline clean, \
      attestation licensed, nothing named. \
      **The predicate was the defect, and it failed in both directions** (RC-4 item A, #640, \
      2026-11-21). `own` and `supported` both come from the node's validated message state, so a \
      node that has validated nothing above 198 reads a zero gap while its peers are at 335. The \
      same `peerTip - supported` shape used as a *global* proposer gate froze a healthy network on a \
      six-height skew — the leaders' quorum height sat at the laggards' height (drill run \
      `565c8dec6`, removed in `5524f2b25`). The corrected view compares the node's own validated tip \
      with a bounded peer-tip observation (`BlockRange` probe answers), and the Rust witness pins \
      both directions. The continuous re-entry is Law 123; the HAZOP is \
      `spec/audit/evidence/rc4-join-catch-up/hazop.md`." },

  { number := 122, layer := "Sync",
    statement := "**A validator never re-signs a sequence it has signed.** The proposer derives its \
      next sequence from a durable record of the highest sequence this node has ever signed, \
      written before the proposal is broadcast. While the round snapshot is behind that record — \
      the block that carried the signature was dropped by a rewind or a light join — the proposer \
      waits and names the record and the snapshot. A proceed therefore signs strictly above every \
      recorded sequence; the record only rises; and a rewind leaves both the record and the \
      decision unchanged. The snapshot alone is the mutant: it re-signs the sequence the record \
      already holds, which the network reads as an equivocation and slashes.",
    status := .provedModel,
    formal := `Rchain.OwnSeq.law122_statement,
    mutant := `Rchain.OwnSeq.law122_mutant,
    falsifier := `Rchain.OwnSeq.law122_falsifier,
    declarations := [`Rchain.OwnSeq.Record,
      `Rchain.OwnSeq.Snapshot,
      `Rchain.OwnSeq.Decision,
      `Rchain.OwnSeq.nextSeqUnchecked,
      `Rchain.OwnSeq.nextSeq,
      `Rchain.OwnSeq.raise,
      `Rchain.OwnSeq.afterRewind,
      `Rchain.OwnSeq.law122_statement,
      `Rchain.OwnSeq.law122,
      `Rchain.OwnSeq.law122_mutant,
      `Rchain.OwnSeq.law122_falsifier],
    axioms := [],
    rust := ["casper/src/blocks/proposer/block_creator.rs",
      "block-storage/src/dag/dag_storage.rs"],
    rustWitness := [
      "casper/src/blocks/proposer/block_creator.rs:the_own_sequence_record_refuses_a_sequence_already_signed",
      "casper/src/dag.rs:the_own_sequence_record_round_trips_and_clears"],
    witness := [`Rchain.OwnSeq.law122, `Rchain.OwnSeq.law122_falsifier],
    falsifiable := "Use `nextSeqUnchecked` — the round snapshot alone, which is the Rust derivation \
      before this law. With the record at sequence 152 and the snapshot at 151 it returns 152: a \
      sequence already signed. `law122_mutant` and `law122_falsifier` exhibit it, and the Rust \
      witness pins both sides of the guard (record ahead: wait, naming 152/155/151; record \
      reached: proceed).",
    note := "**The drill's own slashing** (RC-4, 2026-11-21): a light-joined validator restored its \
      state, caught up to height 165, and signed block #155 at sequence 152 again. The network \
      slashed its key 100% (Malicious tier); the pool dropped from four validators to three, after \
      which every checkpoint collector refused on bond-set drift and the burst's deploys never \
      resolved. The rewind had dropped the block that carried the signature, and nothing durable \
      remembered it. The record is written before the block leaves `BlockCreator::create`, so a \
      write failure refuses the proposal rather than allowing a signature the node cannot \
      remember; the HAZOP row is J8 in `spec/audit/evidence/rc4-join-catch-up/hazop.md`." },

  { number := 123, layer := "Sync",
    statement := "**Catch-up is continuous, and only a current view proposes.** A supervisor samples \
      the Law 121 view on every tick. A `running` phase whose view leaves the window re-enters \
      `catchingUp` after `enterAfter` consecutive stale observations; the hysteresis is bounded, not \
      a silence. A `catchingUp` phase whose view returns runs again. A `blocked` phase is terminal. \
      The proposer asks only whether the shard is `running`, so a proposal always carries a current \
      view; there is no network-wide window in the decision. The pre-fix one-shot watchdog — run \
      once at boot and never again — is the mutant.",
    status := .provedModel,
    formal := `Rchain.JoinContinuous.law123_statement,
    mutant := `Rchain.JoinContinuous.law123_mutant,
    falsifier := `Rchain.JoinContinuous.law123_falsifier,
    declarations := [`Rchain.JoinContinuous.enterAfter,
      `Rchain.JoinContinuous.supervise,
      `Rchain.JoinContinuous.mayPropose,
      `Rchain.JoinContinuous.a_running_phase_carries_a_current_view,
      `Rchain.JoinContinuous.a_stale_observation_reenters,
      `Rchain.JoinContinuous.the_hysteresis_is_bounded,
      `Rchain.JoinContinuous.a_current_observation_restores,
      `Rchain.JoinContinuous.a_current_observation_is_served,
      `Rchain.JoinContinuous.blocked_is_terminal,
      `Rchain.JoinContinuous.may_propose_is_sound,
      `Rchain.JoinContinuous.law123_statement,
      `Rchain.JoinContinuous.law123,
      `Rchain.JoinContinuous.preFixWatchdog,
      `Rchain.JoinContinuous.law123_mutant,
      `Rchain.JoinContinuous.law123_falsifier],
    axioms := [],
    rust := ["casper/src/engine/catchup.rs", "casper/src/engine/node_launch.rs",
      "node/src/instances/proposer_instance.rs"],
    rustWitness := [
      "casper/src/engine/catchup.rs:a_running_phase_reenters_catching_up_on_a_stale_observation",
      "node/src/instances/proposer_instance.rs:a_catching_up_cell_does_not_propose"],
    witness := [`Rchain.JoinContinuous.law123, `Rchain.JoinContinuous.law123_falsifier,
      `Rchain.JoinContinuous.a_stale_observation_reenters,
      `Rchain.JoinContinuous.a_current_observation_restores,
      `Rchain.JoinContinuous.may_propose_is_sound],
    falsifiable := "Use the pre-fix watchdog — `preFixWatchdog`, which observes once at boot and \
      never again — and the drill's frozen view stays `running` (`law123_mutant`, \
      `law123_falsifier`). Remove the confirmation bound and `the_hysteresis_is_bounded` fails; \
      return a stale observation to `running` and `a_stale_observation_reenters` fails.",
    note := "**The disarm this law removes** (RC-4 item A, #640, 2026-11-21). `spawn_catch_up_watchdog` \
      moved the phase to `Running` once and returned; nothing re-entered `CatchingUp` and nothing \
      restarted the walk. A joined validator finished its walk at 198 while the network ran to 335 \
      (`062b4140c`, `87b9aedf2`, `e3f5195c9`). The supervisor is the shard's single observer after \
      boot: it updates `own` from the validated message state, takes a bounded `BlockRange` probe \
      answer as the peer tip, and re-enters catch-up on the confirmed stale view. The proposer gate \
      already lived in `proposer_instance.rs`; this law gives it a phase that can be entered again. \
      A healthy leader is current even when its quorum-supported height lags (Law 121's second \
      falsifier), so the observation cannot recreate the removed global gate. The Rust witness is a \
      pure step test; the drill supplies the timing." },

  { number := 124, layer := "Sync",
    statement := "**A validator never announces a sequence the network already carries.** Between the \
      signature (Law 122) and the hash announcement the network's copy of this key's block at the \
      same sequence can arrive. The broadcast decision reads the observed claims immediately before \
      the announcement and refuses when a different hash owns that sequence; a same-hash claim does \
      not block. The claim cell is fed only by blocks that pass the signature check, so a peer \
      cannot forge an entry. The guarded broadcast therefore always agrees with the claim it \
      observed; the pre-fix path — the signing-time check only, then an unconditional announcement — \
      is the mutant.",
    status := .provedModel,
    formal := `Rchain.OwnSeqBroadcast.law124_statement,
    mutant := `Rchain.OwnSeqBroadcast.law124_mutant,
    falsifier := `Rchain.OwnSeqBroadcast.law124_falsifier,
    declarations := [`Rchain.OwnSeqBroadcast.Signed,
      `Rchain.OwnSeqBroadcast.Claims,
      `Rchain.OwnSeqBroadcast.mayBroadcast,
      `Rchain.OwnSeqBroadcast.broadcastUnchecked,
      `Rchain.OwnSeqBroadcast.a_broadcast_matches_its_claim,
      `Rchain.OwnSeqBroadcast.a_matching_claim_permits_the_broadcast,
      `Rchain.OwnSeqBroadcast.a_conflicting_claim_refuses_the_broadcast,
      `Rchain.OwnSeqBroadcast.law124_mutant,
      `Rchain.OwnSeqBroadcast.law124_falsifier,
      `Rchain.OwnSeqBroadcast.law124_statement,
      `Rchain.OwnSeqBroadcast.law124],
    axioms := [],
    rust := ["casper/src/blocks/received_claims.rs",
      "casper/src/blocks/proposer/block_creator.rs",
      "casper/src/blocks/proposer/proposer.rs"],
    rustWitness := [
      "casper/src/blocks/received_claims.rs:a_signature_valid_self_block_records_a_claim",
      "casper/src/blocks/proposer/proposer.rs:the_broadcast_guard_refuses_a_claim_that_arrived_after_signing"],
    witness := [`Rchain.OwnSeqBroadcast.law124, `Rchain.OwnSeqBroadcast.law124_falsifier,
      `Rchain.OwnSeqBroadcast.a_broadcast_matches_its_claim,
      `Rchain.OwnSeqBroadcast.a_matching_claim_permits_the_broadcast,
      `Rchain.OwnSeqBroadcast.a_conflicting_claim_refuses_the_broadcast],
    falsifiable := "Use `broadcastUnchecked` — the pre-fix path, where the signing-time check had \
      already passed — and the drill's timing announces sequence 196 again after the network's copy \
      of the pre-crash block arrived (`law124_mutant`, `law124_falsifier`). Read the claims before \
      signing but not before the announcement and the falsifier is the window; drop the signature \
      check at receipt and a peer can forge the claim.",
    note := "**The drill's second slashing** (RC-4 item B, #640, 2026-11-21). With Law 122's \
      signing-time check in place the victim still signed sequence 196 again: the signing-time check \
      ran, the network's copy of its own earlier block at that sequence arrived from gossip during \
      the multi-second replay, and the announcement then made the duplicate network-visible \
      (`e3f5195c9`, 18:38:10). The local gate was correct; the author should never have announced. \
      The claims cell is populated at signature-verified receipt — before validation — so the copy \
      that arrives during replay is visible at the pre-effect check; the block creator also refuses \
      to sign through a claim, and the proposer's insert path refuses with `SelfEquivocation` rather \
      than latching an own-key verdict. The residual race — a block still in transport buffering at \
      the last check — is named in the HAZOP; the H-1 insert gate and the durable own-key verdict \
      remain the backstop." },

  { number := 125, layer := "Casper",
    statement := "**The finalized read is the certified fringe's seen closure, and it is \
      node-local.** `is_finalized(h)` is membership in the union of the latest fringe's `seen` sets, \
      not membership in the fringe's own ids; a block below the tip is finalized because a later \
      member has seen it. The read is node-local: the finality-observation endpoint reports \
      `scope: nodeLocal` and `availability: unknown`, and a local read is not a quorum verdict. The \
      mutant is the read that trusts only the fringe's ids, which drops the seen history.",
    status := .provedModel,
    formal := `Rchain.FinalityRead.law125_statement,
    mutant := `Rchain.FinalityRead.law125_mutant,
    falsifier := `Rchain.FinalityRead.law125_falsifier,
    declarations := [`Rchain.FinalityRead.Msg,
      `Rchain.FinalityRead.fringeSeen,
      `Rchain.FinalityRead.isFinalized,
      `Rchain.FinalityRead.isFinalizedUnchecked,
      `Rchain.FinalityRead.is_finalized_iff_exists_seen,
      `Rchain.FinalityRead.a_seen_member_is_finalized,
      `Rchain.FinalityRead.a_fringe_member_is_finalized,
      `Rchain.FinalityRead.Availability,
      `Rchain.FinalityRead.Observation,
      `Rchain.FinalityRead.localObservation,
      `Rchain.FinalityRead.a_local_observation_does_not_claim_finality,
      `Rchain.FinalityRead.law125_mutant,
      `Rchain.FinalityRead.law125_falsifier,
      `Rchain.FinalityRead.law125_statement,
      `Rchain.FinalityRead.law125],
    axioms := [],
    rust := ["block-storage/src/dag/representation.rs", "node/src/web/http.rs"],
    rustWitness := [
      "block-storage/src/dag/representation.rs:the_finalized_read_is_the_seen_closure_not_the_fringe_ids"],
    witness := [`Rchain.FinalityRead.law125, `Rchain.FinalityRead.law125_falsifier,
      `Rchain.FinalityRead.a_fringe_member_is_finalized,
      `Rchain.FinalityRead.a_local_observation_does_not_claim_finality],
    falsifiable := "Read only the fringe's ids (`isFinalizedUnchecked`) and a block below the tip \
      that every later member has seen stops being finalized (`law125_mutant`, `law125_falsifier`). \
      Treat the local read as a quorum verdict and `a_local_observation_does_not_claim_finality` \
      fails.",
    note := "**The read every settlement answer derives from** (#487, 2026-10-28). The node's \
      `finalized_blocks_set` is the seen closure of the latest fringe (`representation.rs:57-83`), \
      and `is_finalized` is membership in it (`:141-143`). A stale local fringe can lag the network \
      while the node reports `running`; the endpoint's `nodeLocal`/`unknown` label is the boundary, \
      not a quorum claim. The Rust test is the representation read: a seen ancestor is finalized, an \
      unseen block is not, and the last finalized hash is the fringe's highest member. The route \
      reads the same value; the campaign, not this law, measures whether the local view is current." },

  { number := 126, layer := "Casper",
    statement := "**Settlement is finalized inclusion, and the deploy lifecycle is total.** \
      `settlementConfirmed` is true exactly when the deploy's **inclusion block** is finalized and \
      its execution succeeded (`finalizedSuccess`). `ProcessedWithSuccess` is execution, not \
      settlement; `includedUnfinalized` and `finalityUnknown` do not confirm; `finalizedError` is a \
      terminal execution failure, not successful settlement. The stage table is exhaustive and \
      exclusive. The mutant is the pre-#487 reading that treats any processed deploy as settled.",
    status := .provedModel,
    formal := `Rchain.DeploySettlement.law126_statement,
    mutant := `Rchain.DeploySettlement.law126_mutant,
    falsifier := `Rchain.DeploySettlement.law126_falsifier,
    declarations := [`Rchain.DeploySettlement.Exec,
      `Rchain.DeploySettlement.Stage,
      `Rchain.DeploySettlement.stage,
      `Rchain.DeploySettlement.confirmed,
      `Rchain.DeploySettlement.confirmedUnchecked,
      `Rchain.DeploySettlement.option_bool_cases,
      `Rchain.DeploySettlement.confirmed_iff_finalized_success,
      `Rchain.DeploySettlement.processed_success_without_finality_is_not_settlement,
      `Rchain.DeploySettlement.a_finalized_error_is_not_settlement,
      `Rchain.DeploySettlement.the_stage_is_total,
      `Rchain.DeploySettlement.a_processed_deploy_is_included_before_finality,
      `Rchain.DeploySettlement.law126_mutant,
      `Rchain.DeploySettlement.law126_falsifier,
      `Rchain.DeploySettlement.law126_statement,
      `Rchain.DeploySettlement.law126],
    axioms := [],
    rust := ["node/src/web/http.rs", "casper/src/api/block_api_impl.rs"],
    rustWitness := [
      "node/src/web/http.rs:the_settlement_stage_is_total_and_only_finalized_success_confirms"],
    witness := [`Rchain.DeploySettlement.law126, `Rchain.DeploySettlement.law126_falsifier,
      `Rchain.DeploySettlement.confirmed_iff_finalized_success,
      `Rchain.DeploySettlement.processed_success_without_finality_is_not_settlement,
      `Rchain.DeploySettlement.the_stage_is_total],
    falsifiable := "Use `confirmedUnchecked` — any processed deploy is settlement — and the \
      incident's pair (processed success, finality false) confirms (`law126_mutant`, \
      `law126_falsifier`). Swap the `finalizedError` arm into `finalizedSuccess` and the error test \
      fails; drop a stage from the table and `the_stage_is_total` fails.",
    note := "**The #487 contract as a law** (2026-11-22). On block 1781 a native-REV transfer was \
      `ProcessedWithSuccess` while finality was frozen at 1756; a client reading the execution tag \
      or the tip balance could treat it as settled. #552 made `settlementConfirmed` depend on the \
      inclusion block's finality (`http.rs:1556-1659`), and the local campaign at exactly 2/3 live \
      stake passed. This law states the surface's implication and its total stage table; the Rust \
      test enumerates every execution/finality pair and asserts confirmation exactly at \
      `finalizedSuccess`." },

  { number := 127, layer := "Sync",
    statement := "**A block's declared fringe is a claim; the local read uses the recomputation.** \
      A validator that ingests a block recomputes the fringe from the justifications and stores that \
      value in the validated metadata; `is_finalized` reads the validated metadata, not the wire \
      field. A mismatched declared fringe is therefore not the trusted read. The mutant trusts the \
      declared field; the falsifier is a declared `[1]` against a computed `[2]`.",
    status := .provedModel,
    formal := `Rchain.FringeClaim.law127_statement,
    mutant := `Rchain.FringeClaim.law127_mutant,
    falsifier := `Rchain.FringeClaim.law127_falsifier,
    declarations := [`Rchain.FringeClaim.Block,
      `Rchain.FringeClaim.readFringe,
      `Rchain.FringeClaim.trustedRead,
      `Rchain.FringeClaim.the_trusted_read_is_the_computation,
      `Rchain.FringeClaim.a_mismatched_claim_is_not_the_trusted_read,
      `Rchain.FringeClaim.law127_mutant,
      `Rchain.FringeClaim.law127_falsifier,
      `Rchain.FringeClaim.law127_statement,
      `Rchain.FringeClaim.law127],
    axioms := [],
    rust := ["casper/src/interpreter_util.rs", "casper/src/engine/recovery.rs",
      "casper/src/engine/node_launch.rs"],
    rustWitness := [
      "casper/tests/fringe_restore.rs:the_two_paths_disagree_about_the_blocks_own_claim"],
    witness := [`Rchain.FringeClaim.law127, `Rchain.FringeClaim.law127_falsifier,
      `Rchain.FringeClaim.a_mismatched_claim_is_not_the_trusted_read],
    falsifiable := "Trust the declared field (`readFringe b false`) and a block whose declared \
      fringe differs from its computation feeds the wrong set to `is_finalized` (`law127_mutant`, \
      `law127_falsifier`).",
    note := "**The ingest trust boundary** (RCA 2026-11-22). The validated metadata's fringe is the \
      merge's computed pre-state (`interpreter_util.rs:1313,1506`), and the restore/boot paths \
      compare the restored metadata with the validating one (`recovery.rs:361,403`, \
      `node_launch.rs:758-791`). The wire field remains a claim: it is content-addressed with the \
      block, but the local finalized read does not use it. The Rust witness is the fringe-restore \
      comparison, where the restore path's empty claim and the validating path's real fringe are \
      kept apart." },

  { number := 128, layer := "Casper",
    statement := "**Recovery preserves accepted work.** A restart that re-reads the persisted pool \
      leaves every accepted deploy resolved; a reset to a finalized anchor returns every accepted \
      deploy above the anchor to the pool (or leaves it finalized below it); a recovery that \
      empties the pool and the DAG loses it. The invariant is `wellFormed`: an accepted deploy is \
      already pooled, included or finalized. The mutant is the silent drop.",
    status := .provedModel,
    formal := `Rchain.RecoveryPreservation.law128_statement,
    mutant := `Rchain.RecoveryPreservation.law128_mutant,
    falsifier := `Rchain.RecoveryPreservation.law128_falsifier,
    declarations := [`Rchain.RecoveryPreservation.State,
      `Rchain.RecoveryPreservation.resolved,
      `Rchain.RecoveryPreservation.wellFormed,
      `Rchain.RecoveryPreservation.preserves,
      `Rchain.RecoveryPreservation.replay,
      `Rchain.RecoveryPreservation.reset,
      `Rchain.RecoveryPreservation.a_pool_replay_preserves,
      `Rchain.RecoveryPreservation.a_reset_preserves,
      `Rchain.RecoveryPreservation.silentDrop,
      `Rchain.RecoveryPreservation.a_silent_drop_does_not_preserve,
      `Rchain.RecoveryPreservation.law128_mutant,
      `Rchain.RecoveryPreservation.law128_falsifier,
      `Rchain.RecoveryPreservation.law128_statement,
      `Rchain.RecoveryPreservation.law128],
    axioms := [],
    rust := ["casper/src/engine/reset.rs", "casper/src/block_disposition.rs",
      "casper/tests/reset.rs"],
    rustWitness := [
      "casper/tests/reset.rs:a_self_replay_reset_returns_the_deploy_before_purging_its_only_block",
      "casper/tests/reset.rs:a_disk_backed_reset_preserves_the_inventory_across_store_reopen"],
    witness := [`Rchain.RecoveryPreservation.law128, `Rchain.RecoveryPreservation.law128_falsifier,
      `Rchain.RecoveryPreservation.a_pool_replay_preserves,
      `Rchain.RecoveryPreservation.a_reset_preserves],
    falsifiable := "Use `silentDrop` — a recovery that empties the pool and the DAG — and the \
      accepted deploy disappears (`law128_mutant`, `law128_falsifier`). Drop the return arm from \
      `reset` and `a_reset_preserves` fails.",
    note := "**The RC-4/public claim in the register** (#564; #641). The 45-hour freeze's recovery \
      depends on the reset inventory returning unfinalized deploys before the rewind \
      (`return_unfinalised_deploys`, C366/C359/C365), and RC-4/public's pass condition says every \
      deploy accepted by a survivor resolves and its effect is at the post-recovery finalized head. \
      The Rust witnesses are the reset inventory tests: the deploy is returned before its only \
      block is purged, and a disk-backed reset preserves the inventory across a store reopen. The \
      public arm, not this law, measures the live stop and rejoin." },
]

end Laws
end Rchain
