# C344 — the integration phase space and its law/test map

This is the acceptance bar for closing C344 in PR #600. Every row names the state the integration
test or drill arm drives, the law theorem that models it, and the executable witness. A row with a
law but no witness, or a witness but no law, is not closed.

Status: **W** = unit/logic witness committed; **I** = in-process integration test committed;
**L** = live drill arm (the final run is recorded under `spec/audit/evidence/c344-closeout-20261117/`).

## Checkpoint / cold join

| # | state / transition | Lean (law) | witness | status |
|---|---|---|---|---|
| C1 | producer publishes only a statement with a merge base (nonempty anchor fringe, non-zero fringe state); the c405 400 is the empty case | Law 117 `wellFormed`, `an_anchor_without_a_finalised_fringe_is_not_well_formed` | `wait_for_certifiable_checkpoint` in `node/tests/checkpoint_restore.rs` | I |
| C2 | pinned certificate target is exactly `[anchor]`; a later broadcast fringe is ignored | Law 114 `Target`, `a_pinned_certificate_targets_exactly_its_anchor` | `casper/src/engine/node_syncing.rs::a_pinned_certificate_does_not_follow_a_later_broadcast_fringe`; restore e2e | W + I |
| C3 | join inputs = anchor + certificate fringe + anchor header's fringe + every member's own fringe | Law 114 `replayLayer`, `expandedReplayLayer`, `requiredBodies` | `casper/src/engine/node_syncing.rs::the_join_input_set_is_the_anchor_its_fringe_and_the_deploy_window`; `checkpoint_join` | W + I |
| C4 | roots cover the expanded replay layer (pre/post/fringe, zero skipped) and not the deploy window | Law 114 `requiredRoots`, `an_unheld_root_is_not_representable` | `checkpoint_join` positive | W + I |
| C5 | deploy-window bodies above `anchor_height - DEPLOY_LIFESPAN`; the `repeat_deploy` scan is parent-closed above the floor | Law 114 `deployWindowBodies`, `a_missing_deploy_window_body_is_not_representable`, `the_deploy_window_is_closed_under_parents`, `law114_deploy_window_falsifier` | `checkpoint_join` positive + the missing-window-body negative arm | W + I |
| C6 | a v2 statement (no member headers) cannot supply a member's own fringe | Law 114 `v2RequiredBodies`, `law114_mutant`, `law114_falsifier`; Law 117 `fringeHeadersRoot` optional arm | `casper/src/checkpoint.rs::a_v3_statement_commits_the_fringe_headers_and_never_aliases_v2` | W |
| C7 | every signed commitment is checked: anchor, post-state, fringe state, metadata root, history root, fringe-headers root; v2 optionality | Law 117 `Verifies`, six projection theorems, six refusal theorems | `casper/src/checkpoint.rs` `a_metadata_commitment_is_bound_and_verifies`, `a_validation_window_is_bound_and_verifies`, `the_fringe_claim_is_committed_per_header` | W |
| C8 | joiner restores the certified state, reaches Running, anchor is a checkpoint, head > anchor, restored dev-vesting leaf | Laws 114/116/117 | `node/tests/checkpoint_restore.rs::a_checkpoint_joiner_restores_the_certified_state` | I |
| C9 | bonded restored joiner proposes; the bootstrap validates and finalises; the bootstrap's autopropose keeps the head moving | Laws 114/116/117 | `node/tests/checkpoint_restore.rs::a_checkpoint_restored_bonded_joiner_proposes_and_the_bootstrap_finalises_it` | I |
| C10 | wiped joiner validates the first post-anchor block from the certificate and join bodies | Laws 114/116 | `casper/tests/checkpoint_join.rs::a_wiped_joiner_validates_the_first_post_anchor_block_from_the_certificate_and_join_bodies` | I |
| C11 | committed history refused; held sibling replayed; certified-unheld root refused (not faulted); merged pre-state over validated justifications replayed; all justifications committed replayed | Law 116 `admission_replays_iff`, `committed_history_is_refused`, `a_held_sibling_is_replayable`, `a_certified_unheld_root_is_refused_not_faulted`, `merged_pre_state_over_validated_justifications_is_replayed`, `all_justifications_committed_is_replayed`; Law 109 `IsReplayRefused` | `casper/src/blocks/block_processor.rs::the_pre_replay_gate_*`; `casper/src/engine/anchor.rs::a_certified_root_that_is_not_the_anchor_is_not_a_replay_base` | W |
| C12 | the anchor's seen set is self-inclusive; the refusal set subtracts the anchor; a sibling is live | Law 109 `buildsSeen`, `self_mem_seen`, `replayRefusedHistory`, `the_rust_arms_are_the_refusal_set`, `a_sibling_is_not_committed_history` | `block-storage/src/dag/message_state.rs::create_message_seen_is_parents_seen_plus_id`; anchor admission units | W |
| C13 | fork: a conflict block that has not seen the whole final fringe is rejected with its proved dependency closure | Law 115 `forkBlocks`, `close_is_fixed`, `close_spec`, `the_dependency_closure_is_unique` | `casper/src/merging.rs::a_non_descendant_conflict_block_is_a_fork_and_the_final_scope_stays_trimmed`; `sdk/src/dag/merging.rs::with_dependencies_collects_transitive_closure`; `merge_determinism`; `n420_boundary_merge` | W + I |

## Liveness

| # | state / transition | Lean (law) | witness | status |
|---|---|---|---|---|
| L1 | presence = max height of a fringe block authored or a deploy signed | Law 89 `participationMap`, `presence_is_the_max_participation_height`, `a_deploy_signer_can_present_a_jailed_validator` | `casper/src/multi_parent_casper.rs::a_deploy_signer_is_presence_at_the_including_height`, `presence_keeps_the_greatest_height` | W |
| L2 | quorum tip = greatest height with strict `3·support > 2·total`; empty/zero => no action | Law 89 `quorumTip`, `quorum_tip_is_the_greatest_height` | `rholang/src/pos_liveness.rs::the_quorum_tip_is_the_height_a_supermajority_reached` | W |
| L3 | absence = missing or strictly more than the threshold behind the tip; exactly threshold is present | Law 89 `isAbsent`, `absence_is_missing_or_behind_the_threshold`, `exactly_the_threshold_is_present` | `rholang/src/pos_liveness.rs::absence_is_missing_or_behind_the_threshold` | W |
| L4 | sustained absence across `N` advancing boundaries jails; one omission does not; bond and withdrawal untouched | Law 89 `newlyJailed`, `a_boundary_at_the_threshold_jails`, `one_lapse_is_not_sustained`, `jail_never_touches_the_bond` | `rholang/src/native_state.rs::a_sustained_absence_jails_without_touching_the_bond`, `one_boundary_omission_does_not_jail`; drill `jail-vote` | W + L |
| L5 | jail removes the target from `active`; release re-enters it | Law 89 `a_jailed_validator_leaves_the_active_set`, `a_released_validator_reenters` | drill `jail-vote` (`target_excluded_from_active_after_jail`), `restart-expiry` | W + L |
| L6 | a window opens only for a jailed target; a vote against an active validator is refused | Law 89 `openWindow`, `a_vote_without_jail_is_not_opened`, `a_vote_without_a_window_is_not_recorded` | `rholang/src/native_state.rs::a_vote_cannot_open_against_an_active_validator`; drill `jail-vote` `active_target_vote_refused` | W + L |
| L7 | exactly 2/3 of the active denominator does not pass; strict > 2/3 does; the denominator excludes the jailed target | Law 89 `passes`, `exactly_two_thirds_does_not_pass`, `a_strict_supermajority_passes` | `rholang/src/native_state.rs::a_vote_below_two_thirds_does_not_slash`; drill `exact-threshold` (900; 600 no slash; 800 slash) | W + L |
| L8 | a passing vote slashes/ejects: bond, active, window, ballots, ban, staged withdrawal | Law 89 `passing`, `slashTarget`, `a_passing_vote_slashes_and_ejects` | `rholang/src/native_state.rs::a_passing_vote_slashes_and_permanently_ejects`, `a_passing_vote_removes_a_staged_withdrawal`; drill `jail-vote` | W + L |
| L9 | a stalled boundary changes nothing; an off-boundary block changes nothing | Law 89 `the_stall_rule_changes_nothing`, `a_stall_freezes_the_jail_clock` | `rholang/src/native_state.rs::a_stall_freezes_the_jail_clock`; drill `stall` (`finalized_height_frozen_below_quorum`, `off_boundary_dag_advanced_without_a_liveness_write`) | W + L |
| L10 | at expiry, presence releases (record/window/ballots cleared); silence renews | Law 89 `expireTarget`, `a_returning_validator_is_released`, `a_still_absent_validator_is_renewed`, `release_clears_the_window_and_ballots` | `rholang/src/native_state.rs::a_jailed_validator_returns_after_the_term`, `a_still_absent_validator_is_renewed`, `a_released_jail_leaves_no_window_or_ballot`; drill `restart-expiry` | W + L |
| L11 | a banished validator cannot rebond | Law 89 `rebond`, `a_banished_validator_cannot_rebond` | `rholang/src/native_state.rs::a_banished_validator_cannot_rebond` | W |
| L12 | a boundary no-op write does not reject the concurrent ballot | Law 17e | `casper/src/merging.rs::a_finalized_boundary_noop_does_not_reject_a_concurrent_ballot` | W |
| L13 | a wiped validator rejoins, catches up, proposes, is finalised, and has no liveness record | Laws 114/116 + Law 89 | drill `cold-rejoin` (`join_contract_running_and_caught_up`, `signed_a_block_after_the_join`, `joined_block_finalized`) | L |

## Known coverage limits, stated rather than hidden

- **C6** has a Lean falsifier and the v3-statement unit test, not a two-node v2 join failure: the
  one-validator cold-join fixture cannot exercise the same-height-sibling body the live multi-parent
  join did. The v3 statement test pins the wire difference; the live `cold-rejoin` arm is the
  end-to-end counterweight.
- **C7** is unit-level because the joiner's verification runs in `node_syncing`/`checkpoint.rs`, not
  in the `checkpoint_join` validation fixture; the e2e restore test exercises the honest path.
- **C9** uses the bootstrap's autopropose to keep the head moving; the cold-rejoin arm's
  `join_contract_running_and_caught_up` is the explicit catch-up criterion.
- The liveness rows L1–L3 have unit witnesses, not live arms: the participation map and quorum tip
  are the observation surface the drill reads; the live arms assert their consequences.
