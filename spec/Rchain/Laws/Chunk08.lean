import Rchain.Laws.Base

/-!
# The law register, chunk 08 (laws 100–110)

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

def lawsChunk08 : List Law := [
  { number := 100, clause := "a", layer := "Casper",
    statement := "**Membership is admitted before a record opens.** An empty leg list or any leg on \
      a shard this node does not validate opens no record and submits no deploy; every opened \
      record names only members. A transaction for a foreign shard is a caller error, not a \
      guaranteed abort.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.allMembers, `Rchain.Acceptance.openTxn,
      `Rchain.Acceptance.openTxnUnguarded,
      `Rchain.Acceptance.a_non_member_leg_opens_no_record,
      `Rchain.Acceptance.an_empty_leg_list_opens_no_record,
      `Rchain.Acceptance.an_opened_record_names_only_members,
      `Rchain.Acceptance.an_unguarded_open_records_a_foreign_leg],
    axioms := [],
    rust := ["casper/src/gateway/mod.rs"],
    rustWitness := ["casper/tests/gateway_txn.rs:gateway_rejects_a_non_member_shard",
      "casper/tests/gateway_txn.rs:local_deploy_service_rejects_a_non_member_shard",
      "casper/tests/gateway_faults.rs:recover_in_flight_refuses_a_leg_that_left_the_membership"],
    witness := [`Rchain.Acceptance.a_non_member_leg_opens_no_record,
      `Rchain.Acceptance.an_empty_leg_list_opens_no_record,
      `Rchain.Acceptance.an_opened_record_names_only_members,
      `Rchain.Acceptance.an_unguarded_open_records_a_foreign_leg],
    falsifiable := "Drop the membership loop (`openTxnUnguarded`): a foreign leg opens a record and \
      reaches a prepare (`an_unguarded_open_records_a_foreign_leg`). In Rust, the two non-member \
      rejections pin the gateway and the deploy service; the recovery case pins a leg that left the \
      membership between the open and the resume.",
    note := "**What this law does not claim.** A member may still fail — a down member aborts under \
      Law 27 — and no deploy is relayed (an explicit non-goal). The model's membership list is the \
      node's `LocalShardDeployService` map; the gateway checks every leg before the durable `put` \
      (`casper/src/gateway/mod.rs:301-331`)." },

  { number := 100, clause := "b", layer := "Casper",
    statement := "**A phase is anchored at the target shard's head, and an unreadable head refuses \
      it.** The anchor is `current_height(shard)`, so a phase is admissible while the chain is \
      within the deploy lifespan; a head that cannot be read produces a refusal rather than \
      `valid_after = 0`, whose deploy is born expired once `head >= lifespan` (C166/C64). A refused \
      phase cannot vote ready.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.PhaseResult,
      `Rchain.Acceptance.phaseWithHead,
      `Rchain.Acceptance.phaseWithHeadUnguarded,
      `Rchain.Acceptance.phaseAnchor, `Rchain.Acceptance.BornExpired,
      `Rchain.Acceptance.an_unreadable_head_refuses_the_phase,
      `Rchain.Acceptance.a_refused_phase_is_not_a_ready_vote,
      `Rchain.Acceptance.a_readable_head_votes,
      `Rchain.Acceptance.a_readable_head_anchors_at_its_height,
      `Rchain.Acceptance.a_refused_phase_has_no_anchor,
      `Rchain.Acceptance.a_genesis_anchor_is_born_expired,
      `Rchain.Acceptance.the_head_anchor_is_admissible,
      `Rchain.Acceptance.the_unguarded_phase_votes_ready_without_a_head],
    axioms := [],
    rust := ["casper/src/gateway/mod.rs", "casper/src/txn_coordinator.rs",
      "casper/src/validate.rs"],
    rustWitness := ["casper/src/gateway/mod.rs:current_height_refuses_a_head_it_cannot_read",
      "casper/src/gateway/mod.rs:current_height_reads_the_shards_head",
      "casper/tests/gateway_faults.rs:an_unreadable_head_refuses_the_leg_and_aborts",
      "casper/src/txn_coordinator.rs:run_2pc_anchors_each_phase_at_its_legs_height"],
    witness := [`Rchain.Acceptance.an_unreadable_head_refuses_the_phase,
      `Rchain.Acceptance.a_refused_phase_is_not_a_ready_vote,
      `Rchain.Acceptance.a_genesis_anchor_is_born_expired,
      `Rchain.Acceptance.the_head_anchor_is_admissible,
      `Rchain.Acceptance.the_unguarded_phase_votes_ready_without_a_head],
    falsifiable := "Answer a failed head read with `valid_after = 0` \
      (`phaseWithHeadUnguarded`): the phase votes ready on a chain past the lifespan \
      (`the_unguarded_phase_votes_ready_without_a_head`), and \
      `a_genesis_anchor_is_born_expired` is the arithmetic that makes it a silent expiry. In Rust, \
      `current_height_refuses_a_head_it_cannot_read` and the rewritten gateway fault case pin the \
      refusal and the empty deploy list on the unreadable shard.",
    note := "The expiry is the port's `valid_after <= height - lifespan` \
      (`casper/src/validate.rs:77`), i.e. `anchor + lifespan <= head`. **The unknown-shard edge is \
      not a law clause**: `current_height` returns `Ok(0)` for a shard the node does not hold, \
      which is unreachable on a fresh run (membership precedes the phase) and, on recovery after a \
      membership change, the deploy service's `member()` gate refuses the shard and the leg aborts \
      (the recovery witness). A leg already voted ready on a removed shard keeps its escrow locked \
      — a recorded residual, outside the law." },

  { number := 100, clause := "c", layer := "Casper",
    statement := "**Block and deploy ingress is shard-local, so every merge input is one shard.** An \
      accepted block carries the node's shard id and every deploy in it carries that same id; \
      therefore any list of accepted blocks — and hence any merge input — is one shard. This is why \
      the shard-blind DAG and Law 97's per-shard merge accounting are safe: a merge can never mix \
      shards because no foreign block or deploy enters one.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.Deploy, `Rchain.Acceptance.Block,
      `Rchain.Acceptance.blockShardOk, `Rchain.Acceptance.deployShardOk,
      `Rchain.Acceptance.accepted,
      `Rchain.Acceptance.acceptedByBlockShardOnly,
      `Rchain.Acceptance.an_accepted_block_is_the_nodes_shard,
      `Rchain.Acceptance.every_deploy_in_an_accepted_block_is_the_nodes_shard,
      `Rchain.Acceptance.a_merge_input_of_accepted_blocks_is_one_shard,
      `Rchain.Acceptance.a_block_shard_check_alone_accepts_a_foreign_deploy],
    axioms := [],
    rust := ["casper/src/blocks/block_receiver.rs", "casper/src/validate.rs"],
    rustWitness := ["casper/src/blocks/block_receiver.rs:a_block_from_another_shard_is_not_of_interest_at_the_receiver",
      "casper/src/validate.rs:a_block_whose_deploy_is_from_another_shard_is_refused",
      "casper/src/validate.rs:deploy_validators",
      "casper/src/validate.rs:format_of_fields_rejects_non_ascii_shard_id"],
    witness := [`Rchain.Acceptance.an_accepted_block_is_the_nodes_shard,
      `Rchain.Acceptance.every_deploy_in_an_accepted_block_is_the_nodes_shard,
      `Rchain.Acceptance.a_merge_input_of_accepted_blocks_is_one_shard,
      `Rchain.Acceptance.a_block_shard_check_alone_accepts_a_foreign_deploy],
    falsifiable := "Accept on the block's shard alone (`acceptedByBlockShardOnly`): a block whose \
      deploy names another shard passes, which \
      `a_block_shard_check_alone_accepts_a_foreign_deploy` states. At the receiver, \
      `a_block_from_another_shard_is_not_of_interest_at_the_receiver` refuses the foreign block \
      with a same-shard positive control, and \
      `a_block_whose_deploy_is_from_another_shard_is_refused` pins the per-deploy half.",
    note := "Models the shard halves of `check_if_of_interest` \
      (`casper/src/blocks/block_receiver.rs:440-459`), `format_of_fields`' shard-id check and \
      `deploys_shard_identifier` (`casper/src/validate.rs:11-27`, `:96-106`). **It does not model** \
      hash, signature, version, phlo or pool checks (Laws 16/17/48/86), and it does not claim the \
      DAG is otherwise shard-aware — it is not; safety comes from this ingress. Law 97's accounting \
      is per shard and is cited, not reproved." },

  { number := 100, clause := "d", layer := "Node",
    statement := "**A deploy is dispatched to its named member or refused with the membership — never \
      relayed.** The router has no relay outcome: a member deploy goes to exactly its own shard, a \
      non-member is an error naming the membership, and a keyed lookup probes the primary first. A \
      router whose primary is not a member is not constructible.",
    status := .provedModel,
    declarations := [`Rchain.Acceptance.Route, `Rchain.Acceptance.Router,
      `Rchain.Acceptance.mkRouter, `Rchain.Acceptance.Router.isMember,
      `Rchain.Acceptance.routeDeploy, `Rchain.Acceptance.relayRoute,
      `Rchain.Acceptance.probeOrder, `Rchain.Acceptance.firstSome,
      `Rchain.Acceptance.probe, `Rchain.Acceptance.sampleRouter,
      `Rchain.Acceptance.answerChild,
      `Rchain.Acceptance.a_deploy_is_dispatched_only_to_its_named_member,
      `Rchain.Acceptance.a_non_member_deploy_is_refused,
      `Rchain.Acceptance.a_router_needs_a_primary_that_is_a_member,
      `Rchain.Acceptance.a_member_primary_makes_a_router,
      `Rchain.Acceptance.the_primary_is_probed_first,
      `Rchain.Acceptance.a_key_on_a_secondary_is_found_after_the_primary_misses,
      `Rchain.Acceptance.a_relay_route_leaves_the_named_shard],
    axioms := [],
    rust := ["node/src/api/shard_routing.rs"],
    rustWitness := ["node/src/api/shard_routing.rs:deploy_is_dispatched_to_the_named_shard",
      "node/src/api/shard_routing.rs:deploy_to_a_non_member_shard_is_rejected_with_the_membership",
      "node/src/api/shard_routing.rs:every_keyed_lookup_probes_the_members",
      "node/src/api/shard_routing.rs:the_primary_answers_every_unrouted_method",
      "node/src/api/shard_routing.rs:a_router_needs_a_primary_that_is_a_member"],
    witness := [`Rchain.Acceptance.a_deploy_is_dispatched_only_to_its_named_member,
      `Rchain.Acceptance.a_non_member_deploy_is_refused,
      `Rchain.Acceptance.a_router_needs_a_primary_that_is_a_member,
      `Rchain.Acceptance.the_primary_is_probed_first,
      `Rchain.Acceptance.a_relay_route_leaves_the_named_shard],
    falsifiable := "Route a non-primary's deploy to the primary (`relayRoute`): the deploy leaves \
      the named member (`a_relay_route_leaves_the_named_shard`), which is the relay the port \
      refuses. Dropping the membership check makes a foreign deploy reach the primary instead of \
      the membership error; `deploy_to_a_non_member_shard_is_rejected_with_the_membership` reddens.",
    note := "**This row is the router's shape, not a network claim.** No-relay is a statement that \
      the dispatch function has no relay outcome; the only dispatch target is the named member. The \
      primary-first probe and the unrouted-default are the router's policy, mirrored by the Rust \
      table, not a liveness claim, and health fan-out and finality reads are out of the model \
      (`node/src/api/shard_routing.rs:40-113`)." },

  { number := 101, layer := "Sync",
    statement := "**A verified sync answer is peer-independent.** A block or state page is accepted \
      only when its body hashes to the key it was requested under, so two verified answers for one \
      key carry the same body and the announcing peer is not an input to acceptance. A redundant \
      request (a hedge) completes no key of its own, so it is a stutter on the walk's completed \
      measure, and the per-key fan-out bounds the redundant requests.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Redundancy.any_verified_answer_is_the_answer,
      `Rchain.Sync.Redundancy.peer_is_not_an_input,
      `Rchain.Sync.Redundancy.hedging_is_a_stutter,
      `Rchain.Sync.Redundancy.hedging_is_bounded],
    axioms := [],
    rust := ["casper/src/protocol/comm_util.rs",
      "casper/src/engine/lfs_block_requester.rs",
      "casper/src/engine/lfs_tuple_space_requester.rs"],
    rustWitness := [
      "casper/src/engine/lfs_block_requester.rs:a_connected_non_bootstrap_peer_can_serve_a_block_the_bootstrap_ignores",
      "casper/src/engine/lfs_tuple_space_requester.rs:request_next_asks_the_bootstrap_and_a_connected_peer_for_a_page"],
    witness := [`Rchain.Sync.Redundancy.any_verified_answer_is_the_answer,
      `Rchain.Sync.Redundancy.hedging_is_a_stutter],
    falsifiable := "Delete injectivity of the content hash and `any_verified_answer_is_the_answer` \
      is false: two bodies can hash to one key, so the redundant answers differ. Make acceptance \
      read the announcing peer and `peer_is_not_an_input` is false. Make a hedge advance the \
      completed measure and `hedging_is_a_stutter` is false, which would also break Law 69's \
      pacing. The Rust witnesses pin both directions: the block walk completes from a non-bootstrap \
      peer whose answer the bootstrap never sent, and the page walk sends the same path to the \
      bootstrap and a connected peer.",
    note := "**Injectivity is a hypothesis, not a theorem.** `hcf` is the same assumption Law 19 \
      makes about `blake2b256`; it is not an axiom of this file, so the row's `axioms` stays empty \
      and the register's axiom accounting is unchanged. The law does not claim that a second peer \
      answers, that any peer is honest, or that the walk finishes: those are Law 69's fairness \
      hypotheses and Law 83's trust residual. `hedging_is_bounded` is the per-key fan-out \
      (`LFS_BLOCK_FANOUT`/`LFS_PAGE_FANOUT`, each 2), not a global request budget." },

  { number := 102, layer := "Sync",
    statement := "**Replay reconstructs the chain's states; the genesis post-state is the only \
      external one.** A chain is linked when each block's pre-state is the state the previous block \
      produced. On a linked chain replay has a result, and that result implies the chain was linked; \
      a block whose pre-state is not the previous post-state is refused rather than filled from a \
      downloaded state. This is why the audit's replay hydrates only the genesis post-state instead \
      of walking the whole trie for every historical block root. **The sync path never replays to \
      establish trust**: it accepts a certified checkpoint (Laws 104/105) and leaves replay-from-\
      genesis to the audit. It does replay **one bounded segment** — a checkpoint joiner regenerates \
      the deploy-chain indices of the finalised layer behind the anchor, from pre-states the \
      certificate or the walked headers commit, and refuses a post-state mismatch. That replay is a \
      merge input, not an acceptance path.",
    status := .provedModel,
    declarations := [`Rchain.Sync.ReplayState.linked_iff_replay_has_a_result,
      `Rchain.Sync.ReplayState.replay_reconstructs_every_produced_state,
      `Rchain.Sync.ReplayState.a_block_whose_pre_state_is_not_the_previous_post_is_refused,
      `Rchain.Sync.ReplayState.StateDecision, `Rchain.Sync.ReplayState.stateSafe,
      `Rchain.Sync.ReplayState.a_safe_state_decision_is_the_derived_state,
      `Rchain.Sync.ReplayState.same_parents_same_state,
      `Rchain.Sync.ReplayState.state_decision_is_total,
      `Rchain.Sync.ReplayState.an_accepted_mismatch_is_the_unsafe_witness,
      `Rchain.Sync.ReplayState.TargetShape, `Rchain.Sync.ReplayState.targetShapeSafe,
      `Rchain.Sync.ReplayState.an_anchored_target_is_the_only_accepted_shape,
      `Rchain.Sync.ReplayState.a_whole_fringe_target_is_refused,
      `Rchain.Sync.ReplayState.target_shape_is_total],
    axioms := [],
    rust := ["casper/src/engine/recovery.rs", "node/src/runtime/audit.rs"],
    rustWitness := [
      "casper/tests/recovery_replay.rs:independent_recovery_replays_epoch_boundaries_and_refuses_forged_claims",
      "casper/src/engine/recovery.rs:an_anchored_target_is_the_only_accepted_shape"],
    witness := [`Rchain.Sync.ReplayState.linked_iff_replay_has_a_result,
      `Rchain.Sync.ReplayState.a_block_whose_pre_state_is_not_the_previous_post_is_refused,
      `Rchain.Sync.ReplayState.a_safe_state_decision_is_the_derived_state,
      `Rchain.Sync.ReplayState.same_parents_same_state,
      `Rchain.Sync.ReplayState.state_decision_is_total,
      `Rchain.Sync.ReplayState.an_accepted_mismatch_is_the_unsafe_witness,
      `Rchain.Sync.ReplayState.an_anchored_target_is_the_only_accepted_shape,
      `Rchain.Sync.ReplayState.a_whole_fringe_target_is_refused,
      `Rchain.Sync.ReplayState.target_shape_is_total],
    falsifiable := "Break a block's pre-state so it is not the previous post-state: \
      `a_block_whose_pre_state_is_not_the_previous_post_is_refused` fails, and replay would have to \
      fall back to a downloaded value. Give the audit a third external root and the no-fallback half \
      is what fails: replay would have to accept a state it did not derive.",
    note := "**The model is the single-chain projection of the Rust's DAG merge.** The Rust derives a \
      block's pre-state from the merge of its parents' post-states (`get_pre_state_for_parents`); \
      the model's `pre = previous post` is the one-parent case. The merge's correctness is Laws \
      66/68/83, not this file, and the downloaded blocks' signatures and content hashes are checked \
      before replay. `a_block_whose_pre_state_is_not_the_previous_post_is_refused` is the no-fallback \
      half: the recovery replay never substitutes a downloaded root for a state it could not derive. \
      The sync path's staging of the same replay is gone; Law 105 states the separation and \
      `node/src/runtime/audit.rs` is where the trust-establishing replay now runs. **The bounded \
      segment the join does replay (2026-11-17 audit):** `MergeScope::merge` loads a `BlockIndex` — \
      body plus mergeable/native sidecars — for every block in the conflict and final scopes, and a \
      wiped joiner has none of them; `regenerate_sidecars` replays each such block from its own \
      pre-state. The certificate commits state roots and headers, not indices, so this replay is \
      the model's truth and the reason the earlier 'does not replay at all' sentence was false." },

  { number := 103, layer := "Sync",
    statement := "**Concurrent serving preserves answer identity and bounds the pool.** The answer \
      to a store-items request is a read of the immutable trie at the request's path: the announcing \
      peer is not an input, another request does not change the answer's value (reversing the batch \
      reverses the answer list and nothing else), and the in-flight set is bounded by the configured \
      consumer width.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Serve.an_answer_is_a_function_of_its_own_request,
      `Rchain.Sync.Serve.the_peer_is_not_an_input,
      `Rchain.Sync.Serve.reversing_the_requests_reverses_the_answers,
      `Rchain.Sync.Serve.in_flight_is_bounded],
    axioms := [],
    rust := ["casper/src/engine/node_launch.rs", "casper/src/engine/node_running.rs"],
    rustWitness := [
      "casper/src/engine/node_running.rs:a_page_over_the_byte_cap_is_served_as_a_valid_prefix",
      "casper/src/engine/node_running.rs:a_store_items_page_costs_the_dispatch_loop_this_long"],
    witness := [`Rchain.Sync.Serve.in_flight_is_bounded,
      `Rchain.Sync.Serve.the_peer_is_not_an_input],
    falsifiable := "Make `serve` read the peer and `the_peer_is_not_an_input` fails; drop the `min` \
      in `InFlight` and `in_flight_is_bounded` fails; make an answer depend on another request's \
      order and `reversing_the_requests_reverses_the_answers` fails.",
    note := "**Structural, not a lock model.** The trie's immutability is Law 70 and the store's \
      locking is outside this file; what is proved is that the responder's output is a function of \
      the request and the trie, and that the pool cannot exceed its width. The per-`(peer, path)` \
      cursor chain and the coalescing gate are the responder's own tests, not this model. The READ \
      path is the one the consumer pool moved; block ingress and fringe traffic stay ordered on the \
      shard loop." },

  { number := 104, layer := "Sync",
    statement := "**A checkpoint certificate is attributable.** A certificate is quorate when its \
      signers hold strictly more than two thirds of the bonded stake; two quorate certificates for \
      one parent therefore share a signer, so two conflicting checkpoints require an equivocator. The \
      digest binds the anchor, the anchor's post-state, the fringe, **the merge base — the state at \
      the fringe the anchor finalised** — the shard, the parent and the metadata commitment, so a \
      signature over one checkpoint is not a signature over another. An absent parent is not an \
      explicit zero parent, and a negative epoch is not a well-shaped statement.",
    status := .provedModel,
    formal := `Rchain.Sync.Checkpoint.law104_statement,
    mutant := `Rchain.Sync.Checkpoint.law104_threshold_mutant,
    falsifier := `Rchain.Sync.Checkpoint.law104_falsifier,
    declarations := [`Rchain.Sync.Checkpoint.two_quorums_intersect,
      `Rchain.Sync.Checkpoint.a_conflicting_checkpoint_requires_an_equivocator,
      `Rchain.Sync.Checkpoint.the_digest_binds_the_anchor_and_parent,
      `Rchain.Sync.Checkpoint.the_digest_binds_the_fringe_state,
      `Rchain.Sync.Checkpoint.the_absence_of_a_parent_is_not_the_zero_parent,
      `Rchain.Sync.Checkpoint.a_negative_epoch_is_not_well_shaped,
      `Rchain.Sync.Checkpoint.law104_statement,
      `Rchain.Sync.Checkpoint.law104_threshold_mutant,
      `Rchain.Sync.Checkpoint.law104_falsifier,
      `Rchain.Sync.Checkpoint.two_weighted_quorums_intersect,
      `Rchain.Sync.Checkpoint.law104],
    axioms := [],
    rust := ["casper/src/checkpoint.rs", "casper/src/api/block_api_impl.rs"],
    rustWitness := [
      "casper/src/checkpoint.rs:a_certificate_at_exactly_two_thirds_is_refused",
      "casper/src/checkpoint.rs:two_conflicting_checkpoints_share_a_signer_under_a_split",
      "casper/src/checkpoint.rs:an_explicit_zero_parent_is_malformed",
      "casper/src/checkpoint.rs:an_epoch_below_zero_is_malformed",
      "casper/src/checkpoint.rs:the_digest_binds_the_fringe_state",
      "casper/src/checkpoint.rs:a_v1_certificate_over_a_multi_member_fringe_is_refused"],
    corpus := some "checkpoint",
    witness := [`Rchain.Sync.Checkpoint.two_quorums_intersect,
      `Rchain.Sync.Checkpoint.the_digest_binds_the_anchor_and_parent,
      `Rchain.Sync.Checkpoint.the_digest_binds_the_fringe_state,
      `Rchain.Sync.Checkpoint.the_absence_of_a_parent_is_not_the_zero_parent],
    falsifiable := "Make the threshold `>=` and a certificate at exactly two thirds is admitted — \
      `law104_threshold_mutant` is that rule, and `law104_falsifier` shows `{0, 1}` of three signers \
      passes it while `Quorate` refuses it; the Rust witness \
      `a_certificate_at_exactly_two_thirds_is_refused` reddens for the same change. Intersection itself \
      survives `>=` for every signer count, so the falsifiable edge is the admission threshold, not the \
      intersection. Drop the digest's anchor projection and \
      `the_digest_binds_the_anchor_and_parent` fails; `the_digest_separates_two_anchors` is the \
      non-vacuity half. Hash an explicit zero parent and \
      `an_explicit_zero_parent_is_malformed` fails; accept a negative epoch and \
      `an_epoch_below_zero_is_malformed` fails.",
    note := "**The model carries both the weighted rule and its equal-weight projection.** \
      `two_weighted_quorums_intersect` is the law's proposition (two stake-quorate sets meet in \
      positive weight), `two_quorums_intersect` is the projection, and the `checkpoint` corpus pins \
      `rchain_sdk::consensus::is_super_majority`'s arithmetic at the boundary (17 rows). **The corpus \
      does not pin the digest binding** — the Rust's `checkpoint_digest` tests and the benchmark's \
      digest recomputation do; that is why this row stays `proved-model` rather than `proved-tied`. \
      The Rust's `verify_checkpoint_certificate` sums stake in `i128` and deduplicates signers. **The shape \
      rules are part of the law, not an assumption:** `Parent` carries a presence bit, so an absent \
      parent and an explicit zero are different statements, and the Rust refuses the explicit zero \
      before hashing; `WellShaped` requires a nonnegative epoch, which the Rust enforces. **What this \
      law does not claim:** a quorum can sign a wrong root — that residual is Law 85's, and the audit \
      (Law 106) is its counterweight. This build produces **genesis-rooted** checkpoints only: there \
      is no parent chain and no freshness window yet, so the statement's parent binding is the \
      digest's binding, not evidence that a chain of certificates was walked. That is recorded as a \
      residual rather than implied." },

  { number := 105, layer := "Sync",
    statement := "**Sync and verification are separate.** Acceptance is a function of the checkpoint \
      certificate, the metadata walk (including the statement's commitment to the metadata set) and \
      the checkpoint state; the replay input cannot change it. A failed audit is an alarm on an \
      accepted state, not a rewind: it never un-accepts what the quorum certified. Replay-from-genesis \
      is the audit's job (`rnode audit`), not the sync path's.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Separation.sync_reads_only_the_checkpoint_and_metadata,
      `Rchain.Sync.Separation.replay_cannot_change_acceptance,
      `Rchain.Sync.Separation.a_failed_audit_is_an_alarm_not_a_rewind,
      `Rchain.Sync.Separation.sync_refuses_a_metadata_root_that_disagrees,
      `Rchain.Sync.Separation.a_quorate_certificate_with_the_same_metadata_is_accepted],
    axioms := [],
    rust := ["casper/src/engine/node_syncing.rs", "node/src/runtime/audit.rs"],
    rustWitness := [
      "casper/src/engine/node_syncing.rs:an_unsigned_sync_has_no_fallback_path",
      "casper/src/engine/node_syncing.rs:a_certified_metadata_history_carries_the_quorums_claim_on_each_block",
      "casper/src/engine/node_syncing.rs:a_tampered_metadata_header_is_refused_under_the_same_certificate",
      "casper/src/engine/node_syncing.rs:a_metadata_history_without_a_commitment_is_refused"],
    witness := [`Rchain.Sync.Separation.replay_cannot_change_acceptance,
      `Rchain.Sync.Separation.a_failed_audit_is_an_alarm_not_a_rewind,
      `Rchain.Sync.Separation.sync_refuses_a_metadata_root_that_disagrees],
    falsifiable := "Feed the replay input into `accept` and `replay_cannot_change_acceptance` fails; \
      make a failed audit yield a rewind and `a_failed_audit_is_an_alarm_not_a_rewind` fails. In Rust, \
      `an_unsigned_sync_has_no_fallback_path` reddens if sync accepts a fringe without a certificate, \
      `a_certified_metadata_history_carries_the_quorums_claim_on_each_block` if acceptance reads \
      anything but the certified header set, and the tamper test if a walked header that differs from \
      the certificate's commitment is accepted.",
    note := "**Acceptance is quorum attribution, not a state proof.** This law does not claim the \
      certificate is quorate (Law 14a) or honest (Law 83/85); it claims the *shape* of the path. \
      Operator policy may stop a node on a disagreement (`--halt-on-audit-disagree` is a policy the \
      audit does not itself impose); the model's default is to stay accepted and alarmed. **The \
      metadata is now committed by the statement** (`metadata_root`, a set commitment over the \
      anchor's ancestor closure): a joiner recomputes it from the headers it walked, so a peer that \
      serves another header set under a valid certificate is refused. A legacy certificate with no \
      commitment is refused by sync and accepted only by the audit, which reports \
      `metadataCommitted: false`. `hash_block` still covers deploy payloads, so the backfill and the \
      audit remain the checks of a body; neither gates acceptance." },

  { number := 106, layer := "Sync",
    statement := "**Audit equivalence.** The audit's verdict is sound with respect to what it \
      compares: agreement implies the replay derived the claimed root and the certificate's metadata \
      commitment matched the bodies; a covered difference — of the state root or of the metadata \
      commitment — is reported as a disagreement; and a history the audit could not obtain is **not** \
      agreement.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Audit.a_replay_that_agrees_derives_the_checkpoint_root,
      `Rchain.Sync.Audit.a_divergence_is_reported,
      `Rchain.Sync.Audit.a_metadata_commitment_mismatch_is_a_disagreement,
      `Rchain.Sync.Audit.an_incomplete_history_is_not_agreement],
    axioms := [],
    rust := ["node/src/runtime/audit.rs", "casper/src/engine/recovery.rs"],
    rustWitness := [
      "node/src/runtime/audit.rs:a_raw_block_reconstructs_and_a_tampered_preimage_is_refused",
      "node/src/runtime/audit.rs:an_audit_covers_every_height_including_genesis",
      "node/src/runtime/audit.rs:the_audit_closure_ignores_a_sibling_at_a_certified_height",
      "casper/src/checkpoint.rs:a_metadata_commitment_is_bound_and_verifies"],
    witness := [`Rchain.Sync.Audit.a_replay_that_agrees_derives_the_checkpoint_root,
      `Rchain.Sync.Audit.a_metadata_commitment_mismatch_is_a_disagreement,
      `Rchain.Sync.Audit.an_incomplete_history_is_not_agreement],
    falsifiable := "Make `judge` return `agree` on a covered difference and \
      `a_replay_that_agrees_derives_the_checkpoint_root` fails; return `agree` on an uncovered history \
      and `an_incomplete_history_is_not_agreement` fails; ignore the metadata check and \
      `a_metadata_commitment_mismatch_is_a_disagreement` fails. In Rust, the reconstruction test \
      reddens if a body that does not hash to the claimed block is accepted, the height-coverage test \
      if the audit skips a height, the closure test if a sibling at a certified height enters the \
      committed set, and the commitment test if a changed committed header field leaves the root \
      unchanged.",
    note := "**What this law does not claim.** It does not claim the replay is faithful (Law 102 and \
      the Rust's rules), nor that agreement is proof the state is honest: a quorum that signed a wrong \
      root is Law 85's residual, and the audit is the counterweight that makes it detectable, not \
      impossible. The audit checks the certificate's metadata commitment — recomputed over the \
      anchor's ancestor closure, not every body at a height — before the replay; a mismatch is a \
      `DISAGREE`, and a legacy certificate with no commitment is accepted with \
      `metadataCommitted: false`. The Rust report names the checkpoint digest and the claimed root; \
      the derived root is present on agreement and the reason carries the replay's own message (which \
      names the block and both states) on disagreement. Missing evidence is `INCOMPLETE`, never a \
      pass." },

  { number := 107, layer := "Sync",
    statement := "**History availability is a service with a check.** A challenge answer proves the \
      archive held the sampled body at challenge time; a sample that covers a gap detects it; a \
      funded, retained history can reconstruct the bodies a light node lacks; and a nonzero body \
      floor is reported as `unavailable`, never as `available`.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Availability.a_challenge_answer_proves_possession,
      `Rchain.Sync.Availability.a_gap_is_detected_with_probability,
      `Rchain.Sync.Availability.funded_retention_can_reconstruct,
      `Rchain.Sync.Availability.an_unavailable_archive_is_reported],
    axioms := [],
    rust := ["node/src/web/archives.rs", "node/src/web/http.rs",
      "casper/src/api/block_api_impl.rs"],
    rustWitness := [
      "node/src/web/archives.rs:a_challenge_sample_is_deterministic_distinct_and_in_range",
      "node/src/web/archives.rs:the_sample_is_bounded_by_the_count_cap_and_the_height_range"],
    witness := [`Rchain.Sync.Availability.a_challenge_answer_proves_possession,
      `Rchain.Sync.Availability.an_unavailable_archive_is_reported],
    falsifiable := "Make `challenge` answer a height the archive does not hold and \
      `a_challenge_answer_proves_possession` fails; make `availability` report `available` on a \
      nonzero floor and `an_unavailable_archive_is_reported` fails. In Rust, the sampling tests pin \
      determinism, distinctness, range and the per-response cap; a sampler that repeated a height or \
      exceeded the cap reddens them.",
    note := "**The probability bound is hypothesis-shaped.** The model proves the deterministic half \
      — a sample that covers a nonempty gap intersects it — and the sampling law (64 distinct heights \
      catch a 10% gap with about 99.9% probability) is a combinatorial statement about the sampler, \
      cited in `node/src/web/archives.rs` rather than proved here. **Retention is a hypothesis, not an \
      axiom**: `funded_retention_can_reconstruct` assumes the body was retained, and the funding \
      decision (a direct service fee is the minimum viable rail) is an external row. Possession at \
      challenge time is not retention for ever, and a failed challenge is fee forfeiture/reputation, \
      not consensus slashing. Observers enter the tier by staking hardware, not tokens; validators \
      can archive too." },

  { number := 108, layer := "Sync",
    statement := "**A running node is anchored.** The node runs from either the operator's pinned \
      genesis or a verified quorum checkpoint; its current root is the anchor's root, and a failed \
      sync is not running. **For a checkpoint the anchor's root is the state at the fringe the anchor \
      itself finalised** — the merge base a post-anchor block's merge starts from — not the anchor \
      block's own post-state and not the merged state of the finalised set: finality is multi-parent, \
      and the layer above the anchor's fringe merges from that base. The signed value is checked \
      against the anchor header's committed `fringe_state_hash`, so the quorum's statement names the \
      base its anchor claims. A genesis anchor names the pinned genesis, not a bootstrap's unverified \
      claim, and an accepted state cannot carry a root its anchor does not name.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Anchor.running_is_anchored,
      `Rchain.Sync.Anchor.a_refused_sync_is_not_running,
      `Rchain.Sync.Anchor.a_genesis_anchor_is_the_pinned_genesis,
      `Rchain.Sync.Anchor.a_checkpoint_anchors_the_fringe_state,
      `Rchain.Sync.Anchor.a_checkpoint_root_is_not_a_blocks_post_state,
      `Rchain.Sync.Anchor.a_foreign_root_is_not_accepted,
      `Rchain.Sync.Anchor.BootEvidence, `Rchain.Sync.Anchor.BootOutcome,
      `Rchain.Sync.Anchor.BootCoordinate, `Rchain.Sync.Anchor.bootOutcome,
      `Rchain.Sync.Anchor.bootSafe,
      `Rchain.Sync.Anchor.only_an_acquisition_runs_a_non_genesis_boot,
      `Rchain.Sync.Anchor.a_genesis_boot_is_genesis_only,
      `Rchain.Sync.Anchor.a_refusal_names_a_missing_coordinate,
      `Rchain.Sync.Anchor.boot_outcome_is_total,
      `Rchain.Sync.Anchor.an_accepted_boot_below_quorum_is_the_unsafe_witness],
    axioms := [],
    rust := ["casper/src/engine/anchor.rs", "casper/src/engine/node_launch.rs"],
    rustWitness := [
      "casper/src/engine/anchor.rs:an_accepted_state_carries_its_anchors_root",
      "casper/src/engine/anchor.rs:a_checkpoint_root_is_the_certified_fringe_state",
      "casper/src/engine/anchor.rs:a_genesis_anchor_must_match_the_pin",
      "casper/src/engine/node_launch.rs:a_fetched_certificate_is_recorded_and_adopted",
      "casper/src/engine/node_launch.rs:a_stored_certificate_is_reverified_without_a_fetch"],
    witness := [`Rchain.Sync.Anchor.running_is_anchored,
      `Rchain.Sync.Anchor.a_checkpoint_anchors_the_fringe_state,
      `Rchain.Sync.Anchor.a_checkpoint_root_is_not_a_blocks_post_state,
      `Rchain.Sync.Anchor.a_foreign_root_is_not_accepted,
      `Rchain.Sync.Anchor.only_an_acquisition_runs_a_non_genesis_boot,
      `Rchain.Sync.Anchor.a_genesis_boot_is_genesis_only,
      `Rchain.Sync.Anchor.a_refusal_names_a_missing_coordinate,
      `Rchain.Sync.Anchor.boot_outcome_is_total,
      `Rchain.Sync.Anchor.an_accepted_boot_below_quorum_is_the_unsafe_witness],
    falsifiable := "Give `Accepted` a public constructor that omits the root equality and \
      `a_foreign_root_is_not_accepted` fails; let the failed-sync transition reach `running` and \
      `a_refused_sync_is_not_running` fails; identify the checkpoint root with the anchor block's \
      post-state and `a_checkpoint_root_is_not_a_blocks_post_state` fails. In Rust, the anchor tests \
      redden if the accepted state derives a root other than the certificate's **fringe state**, or if \
      the genesis anchor accepts a block other than the pin.",
    note := "**The anchor union is the design.** A node's trust anchor is either the operator's \
      genesis (the manifest/descriptors and the genesis block's post-state) or a quorum checkpoint \
      certificate; there is no third source. The prior sync path could leave `CURRENT_ROOT` at genesis \
      after accepting a checkpoint because the running state's root came from a DAG/tip heuristic; the \
      `AcceptedState` type and this law remove that state rather than patch it. **The 2026-11-17 \
      correction (twice measured):** the checkpoint's root is **the state at the fringe the \
      anchor finalised** (Law 104's `fringeState`), which is the base a post-anchor block's merge \
      starts from. The first cut signed the merged state of the finalised fringe and the live cold \
      rejoin showed why that is one layer too high: a block above the anchor does not merge from the \
      anchor's `fringe_state_hash`'s successor, it merges from the state at the fringe its parents \
      finalised — the anchor's fringe state — because a concurrent sibling at the anchor's height may \
      not be in the anchor's finalized set. The singleton legacy case (no signed `fringeState`) falls \
      back to the anchor post-state and keeps the v1 digest. **What this law does not claim:** the \
      certificate is quorate (Law 14a) or honest (Law 83/85); it is the shape of the running state. \
      The genesis-only case is the anchor's \
      genesis branch: no certificate is \
      requested, because a network with no finalized fringe has no quorum to ask. A cold bonded \
      validator still needs the remaining validators to form a quorum (N≥4 under the strict rule); \
      that liveness boundary is recorded, not claimed." },

  { number := 109, layer := "Sync",
    statement := "**Committed history is the anchor's seen closure, and the refusal set subtracts \
      the anchor.** The node builds a message's seen set as the union of its justifications' seen sets \
      plus its own id (`block-storage/src/dag/message_state.rs:281-285`), so the anchor is in its own \
      seen set by construction. The pre-replay predicate is therefore the anchor's seen set minus the \
      anchor itself: a same-height sibling the anchor has not seen is a live message, not committed \
      history; the anchor is the accepted base and is never refused. On a height-descending DAG the \
      Rust predicate's three arms — not the anchor, at or below the anchor's height, in the anchor's \
      seen set — are exactly that refusal set.",
    status := .provedModel,
    formal := `Rchain.MergeScope.law109_statement,
    mutant := `Rchain.MergeScope.law109_mutant,
    falsifier := `Rchain.MergeScope.law109_falsifier,
    declarations := [`Rchain.MergeScope.buildsSeen,
      `Rchain.MergeScope.wellFormedSeen,
      `Rchain.MergeScope.self_mem_seen,
      `Rchain.MergeScope.seenDescends,
      `Rchain.MergeScope.committedHistory,
      `Rchain.MergeScope.IsCommittedHistory,
      `Rchain.MergeScope.replayRefusedHistory,
      `Rchain.MergeScope.IsReplayRefused,
      `Rchain.MergeScope.IsCommittedHistoryArms,
      `Rchain.MergeScope.the_rust_arms_are_the_refusal_set,
      `Rchain.MergeScope.a_sibling_is_not_committed_history,
      `Rchain.MergeScope.the_anchor_is_committed_history_but_not_replay_refused,
      `Rchain.MergeScope.replay_refusal_is_committed_history_without_the_anchor,
      `Rchain.MergeScope.law109_statement,
      `Rchain.MergeScope.law109,
      `Rchain.MergeScope.law109_mutant,
      `Rchain.MergeScope.law109_falsifier,
      `Rchain.Sync.Window.a_validated_block_reads_only_certified_roots,
      `Rchain.Sync.Window.an_uncertified_root_is_refused,
      `Rchain.Sync.Window.the_history_root_binds_the_window,
      `Rchain.Sync.Window.the_first_post_fringe_block_builds_on_the_certified_root,
      `Rchain.Sync.Window.a_shortened_window_refuses,
      `Rchain.Sync.Window.a_window_root_validates],
    axioms := [],
    rust := ["casper/src/checkpoint.rs", "casper/src/engine/node_syncing.rs",
      "node/src/runtime/audit.rs", "block-storage/src/dag/message_state.rs"],
    rustWitness := [
      "casper/src/checkpoint.rs:a_validation_window_is_bound_and_verifies",
      "casper/src/engine/node_syncing.rs:a_certified_metadata_history_carries_the_quorums_claim_on_each_block",
      "casper/src/engine/node_syncing.rs:the_sync_root_is_the_certified_fringe_state"],
    witness := [`Rchain.MergeScope.self_mem_seen,
      `Rchain.MergeScope.the_rust_arms_are_the_refusal_set,
      `Rchain.MergeScope.a_sibling_is_not_committed_history,
      `Rchain.MergeScope.the_anchor_is_committed_history_but_not_replay_refused,
      `Rchain.MergeScope.replay_refusal_is_committed_history_without_the_anchor,
      `Rchain.MergeScope.law109,
      `Rchain.MergeScope.law109_falsifier,
      `Rchain.Sync.Window.a_validated_block_reads_only_certified_roots,
      `Rchain.Sync.Window.the_first_post_fringe_block_builds_on_the_certified_root,
      `Rchain.Sync.Window.a_shortened_window_refuses],
    falsifiable := "Refuse the anchor by its own seen set (the old model) and \
      `law109_falsifier` proves the anchor is in it by construction; key the refusal on height alone \
      and `a_sibling_is_not_committed_history` exhibits the live sibling it would refuse. In Rust the \
      window test reddens if the digest does not bind `historyRoot`, if a shortened window derives \
      the same commitment, or if a changed header field does not change the window root.",
    note := "**Decision (2026-11-17): the model was wrong, not the Rust.** The old model assumed \
      `anchor ∉ seenOf anchor` as a free hypothesis. The node inserts the message's own id into its \
      seen set (`new_seen.insert(id)`, `block-storage/src/dag/message_state.rs:281-285`), so the \
      hypothesis is false of every held anchor. The corrected model derives self-membership \
      (`self_mem_seen`) from the construction rule and defines the refusal set as the seen set minus \
      the anchor. The Rust's explicit anchor guard is exactly that subtraction. **The window's shape \
      is a protocol constant, not a theorem:** `VALIDATION_WINDOW_HEIGHTS` is 14; the merge depth is \
      the finality lag plus one finality step, and a future builder that reads deeper must change \
      the constant and the commitment together. **The at-or-below-anchor refusal is a closure rule:** \
      a post-certificate sibling at the anchor's height is a latest message the next block's merge \
      reads. The window commits the states of the merge's scope; the deploy-chain indices are a \
      replay product (Law 102). The state roots are the quorum claim Law 85 owns; the audit (Law 106) \
      is the counterweight." },

  { number := 110, layer := "Sync",
    statement := "**A served history range is accounted before it leaves.** A history range is \
      served only when the peer's durable credit pays for its bytes: the debit is exact (the served \
      total grows by the range and the remaining credit is the credit before minus the range), an \
      account short of the range is refused without a partial debit, and a credit invoice or a \
      range nonce is single-use. The meter is the enforcement point; the fee itself is an external \
      decision, not a consensus rule.",
    status := .provedModel,
    declarations := [`Rchain.Sync.Service.served_requires_credit,
      `Rchain.Sync.Service.an_unpaid_range_is_refused,
      `Rchain.Sync.Service.a_served_range_is_accounted,
      `Rchain.Sync.Service.a_paid_range_is_served,
      `Rchain.Sync.Service.a_short_account_is_refused],
    axioms := [],
    rust := ["block-storage/src/history_meter.rs"],
    rustWitness := [
      "block-storage/src/history_meter.rs:a_range_with_credit_is_served_and_debited_exactly",
      "block-storage/src/history_meter.rs:a_range_without_credit_is_refused_before_a_body_is_read"],
    witness := [`Rchain.Sync.Service.a_served_range_is_accounted,
      `Rchain.Sync.Service.an_unpaid_range_is_refused],
    falsifiable := "Remove the credit check from `serve` and `an_unpaid_range_is_refused` fails; \
      debit any amount other than the range's cost and `a_served_range_is_accounted` fails. In Rust, \
      the meter tests redden if a range is debited without credit, if a short account is partially \
      debited, or if a replayed invoice or nonce credits/serves twice.",
    note := "**What is metered is bodies, not headers.** Headers and checkpoint metadata are small, \
      committed by the certificate and needed to join cheaply, so joins are not taxed; the archive's \
      custody and bandwidth are. The funding rail is the direct REV service fee — the decision of \
      record (2026-11-15, owner: Patrick): no consensus change, no new token and no new slashing rule. \
      **What this law does not claim:** the meter does not price the service or prove the on-chain \
      transfer; the receipt's settlement key is the operator's, and on-chain payment proof is future \
      work. Peer-to-peer body replication remains the sync path and is not a substitute for the paid \
      on-demand tier. See Law 111 for the challenge outcome." },
]

end Laws
end Rchain
