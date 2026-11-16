# C344 (RCHIP #55) — law↔code↔test coverage

**Purpose.** Before the four-arm capstone runs, answer the owner's question: is every behaviour the
arms assert covered by a Lean law that matches the Rust, with a named Rust witness — and what is not?
This record is the answer of 2026-11-16, after the `exact-threshold` arm's lost second ballot was
reduced to a unit fixture.

**Verdict in one paragraph.** The rule-level behaviours are covered by Law 89 (`proved-model`,
`spec/Rchain/Liveness.lean`), with Law 14 (`proved-tied`) pinning the strict `3·stake > 2·total` and
Law 58's corpus tying the live-weight filter. Three behaviours the arms assert had no Lean statement
or no Rust assertion; they are closed here. The merge rule the capstone depends on is Law 17c
(`proved-model`), and the `exact-threshold` failure was a distinct rule — **Law 17e, new here**: a
native write that leaves a slot unchanged is not a write, so it is not a conflict partner. The
failure was reproduced red at `casper/src/merging.rs:4989` and fixed at
`merging.rs:833` (`effective_native_effects`). **This is a hard-fork-class merge-acceptance change**
and is not armed; mainnet arming remains #428.

## Rule coverage

| # | Behaviour the arms assert | Lean (`spec/Rchain/Liveness.lean` unless stated) | Rust | Named Rust test |
|---|---|---|---|---|
| 1 | Sustained absence jails; the bond is untouched | `stepJail`, `jail_never_touches_the_bond` | `native_state.rs::apply_liveness_prelude` step 2 (`:2627-2658`) | `rholang/src/native_state.rs:a_sustained_absence_jails_without_touching_the_bond` |
| 2 | One lapse is not sustained absence | `one_lapse_is_not_sustained` | same, `is_absent` (`pos_liveness.rs:251-261`) | `rholang/src/native_state.rs:one_boundary_omission_does_not_jail` |
| 3 | A stall or an off-boundary block changes nothing | `the_stall_rule_changes_nothing`, `no_boundary_no_action` | stall freeze (`:2551-2559`); non-boundary return (`:2042-2049`) | `rholang/src/native_state.rs:a_stall_freezes_the_jail_clock` |
| 4 | The vote gate is strict `3·stake > 2·total` | `passes`, `vote_admits_only_above_threshold`; Law 14 `is_super_majority` (`sdk/src/consensus.rs:19-21`) | `passes_vote` (`pos_liveness.rs:265-267`); tally (`:2566-2595`) | `rholang/src/native_state.rs:a_vote_below_two_thirds_does_not_slash`; `two_of_three_active_votes_cannot_cross_strict_threshold_via_delegation`; `sdk/src/consensus.rs:two_thirds_is_not_supermajority` |
| 5 | A passing vote slashes and permanently ejects | `voteSlash`, `a_passing_vote_slashes_and_ejects` | step 1 (`:2596-2625`); `slash` (`:2773-2959`); `bond` ban guard (`:1874-1878`) | `rholang/src/native_state.rs:a_passing_vote_slashes_and_permanently_ejects`; `a_banished_validator_cannot_rebond` |
| 6 | A vote window opens only against a jailed target | `openWindow`, `a_vote_without_jail_is_not_opened` | step 2 window (`:2645-2653`); `cast_vote` refusals (`:3029-3057`) | `rholang/src/native_state.rs:a_vote_cannot_open_against_an_active_validator` |
| 7 | One message releases a jail | `releaseIfPresent`, `a_returning_validator_is_released` | step 3 release (`:2694-2699`) | `rholang/src/native_state.rs:a_jailed_validator_returns_after_the_term` |
| 8 | Continued silence renews the jail to the full term | `renewIfAbsent`, `a_still_absent_validator_is_renewed` | step 3 renewal (`:2680-2693`) | `rholang/src/native_state.rs:a_still_absent_validator_is_renewed` |
| 9 | **A passing vote closes the window and clears its ballots** | **new** `a_passing_vote_closes_the_window_and_clears_ballots` (`Liveness.lean:203`) | step 1 `windows.remove` / `ballots.retain` (`:2601-2602`); `slash` tail (`:2942-2957`) | extended `rholang/src/native_state.rs:a_passing_vote_slashes_and_permanently_ejects` |
| 10 | **A release clears the window and ballots** | **new** `releaseIfPresent` correction + `release_clears_the_window_and_ballots` (`Liveness.lean:227`) | step 3 release (`:2695-2697`) | **new** `rholang/src/native_state.rs:a_released_jail_leaves_no_window_or_ballot` |
| 11 | **A banished validator cannot rebond** | **new** `rebond`, `a_banished_validator_cannot_rebond` (`Liveness.lean:237-253`), with `an_unguarded_rebond_reaches_a_banished_validator` as the falsifier | `bond` ban refusal (`:1874-1878`) | `rholang/src/native_state.rs:a_banished_validator_cannot_rebond` |
| 12 | The liveness denominator is the live-weight set | Law 58's liveness corpus (`spec/conformance/liveness.tsv`) | `block-storage/src/dag/liveness.rs::live_weight_set` (`:102-120`) | `node/tests/lean_liveness_corpus.rs:the_node_agrees_with_the_model_on_every_live_weight_set` |
| 13 | **A chain's slots are the writes that changed a value; a no-op is not a conflict partner** | **new Law 17e**: `NativeWrite`, `effectiveSlots`, `law17e`, `all_writes_conflict_where_the_effective_ones_do_not` (`spec/Rchain/Merging.lean:435-477`) | **new** `casper/src/merging.rs::effective_native_effects` (`:833`), called at `BlockIndex::apply` and `RuntimeManager::play_one_on_fork` | `casper/src/merging.rs:a_finalized_boundary_noop_does_not_reject_a_concurrent_ballot`; `a_concurrent_boundary_noop_does_not_overwrite_a_ballot`; `a_boundary_that_changes_the_ballot_map_still_conflicts`; `a_noop_native_write_is_not_a_chain_slot` |
| 14 | The native relation is on the chain, not the block | Law 17c: `chainConflict`, `resolves`, `the_resolution_leaves_the_fold_nothing_to_refuse`, `host_keys_conflict_where_the_chains_do_not` (`Merging.lean:328-413`) | `NativeRelations::conflicting` (`merging.rs:1763-1766`), `fold` (`:1797-1841`) | `casper/src/merging.rs:a_rejected_boundary_chain_does_not_take_its_blocks_other_chains`; `a_boundary_round_does_not_lose_a_contained_deploys_charge`; `the_merge_report_names_every_dropped_native_write` |

## The three closed gaps

1. **A passing vote's window/ballot sweep** had code (`native_state.rs:2601-2602`, `:2942-2957`) but
   no Lean theorem stating it and no Rust assertion. The theorem now exists, and the passing-vote
   test asserts `vote_windows()` and `slash_votes()` no longer name the target.
2. **The release sweep** was in Rust (`:2695-2697`) but the Lean `releaseIfPresent` removed only the
   jail entry. The model now matches the code; `a_released_jail_leaves_no_window_or_ballot` pins the
   state with a vote window as long as the jail term, so the release path is what clears it.
3. **Rebond admission** had a Rust test but no Lean guard. The guard and its falsifier are modelled;
   the test remains the Rust witness.

## The failure this record was written for

The `exact-threshold` arm's second ballot never entered state. The cause, reduced at unit level: the
armed prelude rewrites `pos:slash-votes` at every advancing boundary, `NativeStore::put` records the
`Put` even when the bytes equal the pre-state, and `NativeRelations` therefore counted the unchanged
rewrite as a slot the boundary chain wrote. A conflict-scope vote chain that conflicts with a
**finalised** boundary chain is forced out by `incompatible_with_final`
(`sdk/src/dag/merging.rs:54-72`) — so the ballot died although the boundary changed nothing.

`a_finalized_boundary_noop_does_not_reject_a_concurrent_ballot` was run **red before the fix**: the
vote deploy id `[177]` appeared in `rejected_deploys` and `(host 0xb0, pos:slash-votes)` in
`dropped_native_slots`. After `effective_native_effects` it keeps both ballots. The fix is at index
construction rather than in the store because `BlockIndex::apply` trusts a stored sidecar; filtering
there makes old and regenerated sidecars agree, so indexing is idempotent across an upgrade.

## What this does not cover

- **The capstone itself.** The four-arm run on a live four-validator devnet is operational evidence,
  not a theorem; this record is what licenses making it once.
- **The merge's option search.** Law 17e states the write rule and fixture A–D pin its consequences
  in the final/conflict and conflict/conflict shapes; the enumeration the resolution minimises over
  remains Law 17a's and is not re-modelled here (17c's note says the same).
- **Enforced parameters.** `PosParams::validate` refuses a vote window longer than the jail term
  (`native_state.rs:817-822`), so a window cannot outlive its jail by configuration; the release
  sweep is tested at state level rather than as an independently reachable configuration.
- **Out of scope:** boundary-CloseBlock validity (C301 C5, hard-fork class), mainnet arming (#428),
  and the live public-testnet operation. No deployment, no public validator, no real key use.

## Sources

`spec/Rchain/Liveness.lean`; `spec/Rchain/Merging.lean`; `spec/Rchain/Laws.lean` rows 89 and 17e;
`spec/laws.tsv`; `rholang/src/native_state.rs`; `rholang/src/pos_liveness.rs`;
`casper/src/merging.rs`; `casper/src/runtime_manager.rs`; `rspace/src/native_store.rs`;
`block-storage/src/dag/liveness.rs`; `spec/conformance/liveness.tsv`;
`node/tests/lean_liveness_corpus.rs`.
