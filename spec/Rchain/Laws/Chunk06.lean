import Rchain.Laws.Base

/-!
# The law register, chunk 06 (laws 80–87)

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

def lawsChunk06 : List Law := [
  { number := 80, layer := "Casper",
    statement := "**`CURRENT_ROOT` names only an accepted block.** A checkpoint may record a candidate \
      post-state as a *known* root, but the store's current-root pointer moves only when a block whose \
      post-state is that root is accepted into the DAG — so no reachable state has the pointer naming a \
      state no accepted block references, and a boot republishes only an accepted root",
    status := .provedModel,
    declarations := [`Rchain.PubState, `Rchain.pubStep, `Rchain.pubSystem,
      `Rchain.a_current_root_is_an_accepted_state, `Rchain.a_boot_republishes_only_an_accepted_root],
    axioms := [],
    rust := ["rspace/src/history/roots_store.rs", "rspace/src/history/history_repository.rs",
      "casper/src/runtime_replay.rs", "casper/src/blocks/block_processor.rs"],
    rustWitness := [],
    witness := [`Rchain.an_early_publish_reaches_an_orphan_root, `Rchain.publication_can_advance],
    falsifiable := "The port's order before the deferred publish: the replay checkpoint records the \
      candidate root and moves `CURRENT_ROOT` before `dag.insert` decides. \
      `an_early_publish_reaches_an_orphan_root` exhibits the reachable state that order admits — a \
      current root no accepted block references — so the law was red on the code as it stood, and \
      `publication_can_advance` keeps it from being vacuous.",
    note := "**Why recording a root is not publishing it.** Validation replays the block's deploys, \
      writes a checkpoint and the root tag, and only then calls `dag.insert` \
      (`casper/src/blocks/block_processor.rs:118`); the replay checkpoint reaches \
      `RootsStore::record_root` (`casper/src/runtime_replay.rs:302-306` → \
      `rspace/src/history/history_repository.rs:317`), which writes the root tag and `CURRENT_ROOT` in \
      one LMDB transaction. A block refused for a bad pre-state hash or dropped by a rewind therefore \
      leaves the pointer naming a post-state no accepted block carries — a durable pointer that is not \
      a function of the chain the node runs. The fix (B40, HAZOP F-U17-10) splits the write: candidate \
      checkpoints record the **known** root only, and the publish runs after acceptance at the insert \
      sites, with a non-decreasing-height guard and a boot reconciliation that republishes the latest \
      accepted root when it is known. `reset`'s `validate_and_set_current_root` keeps publishing: its \
      root is an accepted ancestor's. **What it does not claim**: the cross-store window between the \
      insert and the publish is narrowed, not closed — there is no WAL — and a crash there is repaired \
      at boot, best-effort if the root is unknown. `rustWitness` is owed until the deferred-publish \
      tests land." },

  { number := 81, layer := "Sync",
    statement := "**A join walk accepts a verified answer for any known key, and re-asks every \
      outstanding key.** The block walk's key can reach `Received` without being finished — an answer \
      that arrived before its request turn is dropped, and a body that fails its content check has \
      already consumed the key. As written, no request selects `Received` and no answer is accepted \
      for it, so the key is stranded and `is_finished` can never hold. The repair accepts an answer \
      for a known key whatever the request slot, validates before consuming, and re-requests `Init`, \
      `Requested` and `Received` on a resend; one accepted answer per round raises the finished \
      measure, which is bounded by the walk's key universe",
    status := .provedModel,
    declarations := [`Rchain.Sync.Join.accepted_answer_is_verified,
      `Rchain.Sync.Join.every_outstanding_key_is_requested_on_a_resend,
      `Rchain.Sync.Join.a_round_makes_progress,
      `Rchain.Sync.Join.the_written_walk_cannot_finish_from_a_stranded_key],
    axioms := [],
    rust := ["casper/src/engine/mod.rs", "casper/src/engine/lfs_block_requester.rs"],
    rustWitness := [
      "casper/src/engine/mod.rs:resend_re_requests_a_received_key",
      "casper/src/engine/mod.rs:received_accepts_a_known_key_before_its_request_turn",
      "casper/src/engine/mod.rs:received_still_rejects_an_unknown_key",
      "casper/src/engine/lfs_block_requester.rs:a_forged_body_does_not_consume_the_key_and_is_re_requested",
      "casper/src/engine/lfs_block_requester.rs:a_known_needed_block_is_accepted_before_its_request_is_sent",
      "casper/src/engine/lfs_block_requester.rs:a_mature_chain_join_completes",
      "block-storage/src/dag/codecs.rs:a_decoded_block_still_hashes_to_its_hash"],
    witness := [`Rchain.Sync.Join.the_written_walk_strands_a_received_key,
      `Rchain.Sync.Join.the_fixed_walk_can_finish_from_a_stranded_key,
      `Rchain.Sync.Join.a_rejected_body_leaves_the_walk_where_it_was],
    falsifiable := "A walk state with one `Received` key still in `latest`: \
      `the_written_walk_cannot_finish_from_a_stranded_key` shows every `WrittenStep` leaves it \
      `Received`, so no run reaches `isFinished` — the `(3 finished, 25 outstanding)` shape. The same \
      counts appeared in the live join, but a controlled A/B (2026-10-25) found that instance was inbound \
      reachability, not this defect; the stranded key is what the code admits and what this row fixes. \
      The same state reaches the goal in two turns of the repaired machine \
      (`the_fixed_walk_can_finish_from_a_stranded_key`), so the falsifier is not vacuous and the \
      repair is not a widening of the accepted set to unknown keys: `Step.unknown` still drops them.",
    note := "**Why the missing transition was not the whole defect.** The Rust consumes before it \
      validates (`lfs_block_requester.rs:73-108` calls `received` and only then checks the hash), so a \
      body that fails the check leaves the key `Received`; and `get_next` re-requests only `Init`, or \
      `Requested` on a resend (`mod.rs:102-114`). The model keeps both facts as `WrittenStep`, which is \
      what makes the falsifier the code's own shape rather than an invented one. The repaired machine's \
      fairness hypothesis is explicit: `a_round_makes_progress` needs a verified answer to exist \
      (`hanswer`) and the key universe to be finite and closed (`hK`); the Rust supplies the closure \
      through the fringe's ancestry but this module does not prove the DAG finite. **Law 69 is not \
      replaced**: it models the tuple-space page walk, whose validation failure aborts the attempt and \
      so has no silent stranded key. The witnesses are the 28-block `request_blocks` test and the \
      requester's rejection/early-answer tests; the codec round-trip witness rules out a wire cause \
      of a systematic hash mismatch." },

  { number := 82, layer := "PoS",
    statement := "**A validator's lifecycle has six transitions, and trust is the admission one.** \
      `observer → trusted → bonded → active → withdrawing → claimable`, with `removed` as the model's \
      reading of the Rust's absence: `trust`/`untrust` move the admission list (a trusted caller, \
      never a self-revocation), `bond` is self-only and requires trust and leaves the active set \
      alone, `withdraw` stages without moving a coin, the epoch boundary is the only step that can \
      *add* to the active set, and a slash needs evidence. Revocation is not confiscation: `untrust` \
      changes no stake ledger, so an untrusted-but-bonded validator stays in consensus",
    status := .provedModel,
    declarations := [`Rchain.Lifecycle, `Rchain.LifecycleStep, `Rchain.phaseOf,
      `Rchain.trust_only_from_a_trusted_key, `Rchain.untrust_leaves_the_bond_alone,
      `Rchain.the_trusted_set_never_empties, `Rchain.bond_is_self_only,
      `Rchain.activation_only_at_a_boundary],
    axioms := [],
    rust := ["rholang/src/native_state.rs", "rholang/src/system_processes.rs",
      "casper/src/genesis/mod.rs"],
    rustWitness := [
      "rholang/src/native_state.rs:trust_requires_a_trusted_caller",
      "rholang/src/native_state.rs:untrust_removes_trust_and_leaves_the_bond_alone",
      "rholang/src/native_state.rs:untrust_does_not_change_the_active_set",
      "rholang/src/native_state.rs:bond_escrows_the_stake_and_activates_at_the_boundary",
      "rholang/src/native_state.rs:the_epoch_gate_does_nothing_off_a_boundary",
      "rholang/src/native_state.rs:a_validator_with_delegations_cannot_withdraw",
      "rholang/src/native_state.rs:the_boundary_pays_on_the_participation_it_is_handed"],
    witness := [`Rchain.untrust_does_not_deactivate, `Rchain.the_bond_step_does_not_activate,
      `Rchain.a_justified_slash_is_a_step, `Rchain.a_bond_reaches_the_bonded_phase],
    falsifiable := "The rule this replaces is C111's: `untrust` called `slash` on a bonded target, so \
      one trusted signature moved another validator's entire stake to the Coop vault. In the model that \
      rule would be a second `doUntrust` writing `pos`, and `untrust_leaves_the_bond_alone` would be \
      unprovable by `rfl` — the theorem is the state equality itself, not a description of it. \
      `the_trusted_set_never_empties` excludes the other shape: a caller revoking itself out of the \
      admission list. `activation_only_at_a_boundary` excludes activation by `bond`.",
    note := "**The audit found the code matches the law, with one contradiction that is a comment.** \
      `trust`/`untrust` bind the caller to the unforgeable `deployerId` (`system_processes.rs:2043-2063`), \
      `bond`/`withdraw` bind the actor to the deployer (`:1852-1901`), `untrust` does not confiscate \
      (C111), and activation is `select_active` at a boundary. The stale module header at \
      `rholang/src/native_state.rs:25` says `slash`/`untrust` 'remove the validator and confiscate the \
      stake', which the code and C111 contradict; the comment is fixed, the semantics are not. \
      **What the model does not claim**: `Validator` is an id and the vault is one total, so the \
      per-vault debit of a bond is the Rust's property and not a theorem here; the Rust's min/max bond \
      bounds are policy outside the state machine; and `retired` is model-only — the Rust observes \
      removal as absence. `untrust` leaving an active validator active is deliberate and is named as a \
      non-claim of Law 83's trust model. The witnesses are the lifecycle's own tests: the caller \
      gate, revocation without confiscation, revocation without deactivation, the bond/boundary \
      split, the epoch gate, the delegation guard and the boundary payout." },

  { number := 83, layer := "Sync",
    statement := "**The join is content-trustless, not finality-trustless.** Every join input is \
      genesis-anchored (the configured genesis block/state and its `bond keys ∪ pos-multi-sig` trust \
      set), verified (block content hashes, trie pages by Law 70, ancestry membership and the \
      *derived* `fringe`), or residual (the bootstrap's choice of finalised root, the fringe's state \
      hashes — the root and each per-block `fringe_state_hash` — liveness, and wire \
      signatures/finality proofs). **The checkpoint's fringe state is a signed residual, not an \
      assumed one:** the quorum signs the merged fringe root (Laws 104/108), so a checkpoint joiner \
      hydrates a root the quorum attested; the ordinary fringe reply's member claim remains residual \
      (C339). Two joiners that verify the same block map and hold the same trie \
      read the same value at the claimed root and answer the same block lookup; the residual is an \
      exact, checked enumeration",
    status := .provedModel,
    declarations := [`Rchain.Sync.Trust.Join, `Rchain.Sync.Trust.derivedFringe,
      `Rchain.Sync.Trust.join_is_content_trustless,
      `Rchain.Sync.Trust.a_lying_ancestry_fringe_is_refused,
      `Rchain.Sync.Trust.the_derivation_is_a_function_of_the_verified_inputs,
      `Rchain.Sync.Trust.the_residual_is_exactly_named],
    axioms := [],
    rust := ["casper/src/engine/node_syncing.rs", "casper/src/engine/lfs_block_requester.rs",
      "casper/src/genesis/mod.rs"],
    rustWitness := [
      "casper/src/engine/node_syncing.rs:an_ancestry_entry_for_an_unknown_block_is_refused",
      "casper/src/engine/node_syncing.rs:two_ancestry_entries_for_one_block_are_refused",
      "casper/src/engine/node_syncing.rs:an_ancestry_fringe_that_is_not_the_derived_fringe_is_refused"],
    witness := [`Rchain.Sync.Trust.a_lying_ancestry_state_hash_changes_replay,
      `Rchain.Sync.Trust.genesis_trust_anchor,
      `Rchain.Sync.Trust.a_different_multisig_list_changes_the_anchor,
      `Rchain.Sync.Trust.the_derivation_is_a_function_of_the_verified_inputs],
    falsifiable := "The peer-chosen field the join path could not check: \
      `a_lying_ancestry_state_hash_changes_replay` shows two ancestry rows with the same block and \
      fringe and a different `fringe_state_hash` produce different replay metadata, which \
      `close_block` folds into the epoch seed — so the fringe's state hash is a residual. \
      `a_lying_ancestry_fringe_is_refused` is the field that *is* derivable and therefore refused; \
      `a_different_multisig_list_changes_the_anchor` is the anchor's sensitivity. **A singleton pin \
      was tried and refuted**: it compared the root with the named block's pre- or post-state, and the \
      node's restore suite showed the served root is the state *at* the fringe — one fringe earlier \
      than the block's pre-state — so the check was removed rather than kept as false assurance, and \
      C339 now covers every fringe-state claim.",
    note := "**The exact wording matters.** The proved claim is *content* trustlessness: every state \
      byte and block body is checked against a hash, so two joiners with the same verified inputs \
      cannot read differently. It is **not** finality trustlessness: the joiner trusts the \
      bootstrap's choice of finalised root, its liveness, the per-block `fringe_state_hash` for a \
      multi-member fringe, and it checks neither block signatures nor a quorum finality proof. Those \
      are enumerated in `the_residual_is_exactly_named` and on the trust-model page. The Rust fix this \
      row accompanies verifies ancestry block membership, duplicate-freedom and the derived `fringe`, \
      and refuses a derivable fringe that lies; **C339 stays open** — the fringe's state hashes are \
      not derivable on the join path, so the residual stands. The witnesses are the join path's own \
      tests: membership, duplicate, derived-fringe, and the state-hash residual stated as one. A \
      singleton state pin was attempted and the node's restore suite refuted it; the law records the \
      absence rather than the false check. **Reconciled by Laws 105/108-109 (§168, post-#591/#592): \
      this row models the removed transfer boundary. The checkpoint path no longer trusts the \
      bootstrap's choice of finalised root — acceptance is a verified quorum certificate (Law 105) \
      and the running root is the anchor's (Law 108) — and the per-block `fringe_state_hash` residual \
      is replaced by the committed validation window (Law 109). The content-trustlessness half \
      remains as stated; C339 is superseded by §168.**" },
  { number := 84, layer := "Casper",
    statement := "**The genesis anchor is a function of the genesis inputs, and the ceremony refuses \
      any other key.** The block's sender, the blessed deploys' signer and the governance admin are \
      the **first bond in the bonds file** — an input — so identical inputs give identical bytes \
      (the signing schemes are deterministic) and therefore one genesis hash. A node whose local key \
      is not that bond must refuse to run the ceremony rather than build a private chain. The old \
      rule let each node's own key sign, so identical inputs gave different blocks (#456)",
    status := .provedModel,
    declarations := [`Rchain.GenesisInputs, `Rchain.genesisCreator, `Rchain.mayCreate,
      `Rchain.the_genesis_anchor_is_a_function_of_the_inputs,
      `Rchain.the_genesis_state_is_a_function_of_the_inputs],
    axioms := [],
    rust := ["casper/src/engine/node_launch.rs", "casper/src/genesis/mod.rs",
      "casper/src/bonds_parser.rs"],
    rustWitness := [
      "casper/src/engine/node_launch.rs:the_genesis_ceremony_is_the_first_bond_or_refused"],
    witness := [`Rchain.an_open_ceremony_reaches_two_genesis_hashes,
      `Rchain.the_first_bond_may_create],
    falsifiable := "An **open** ceremony — any local key may sign — reaches two genesis hashes from \
      one bonds file: `an_open_ceremony_reaches_two_genesis_hashes` exhibits it with the smallest \
      input (one bond, two other keys), and `mayCreate` refuses both under the rule. The measured \
      live instance is #456: two nodes, identical inputs, sender and post-state both different.",
    note := "**The rule is the first bond in bonds-file order, and the current network already obeys \
      it**: #456's table shows the network genesis sender is the first bond's key. So a correctly-run \
      ceremony produces the same hash as before; the change is the **refusal**, not a new genesis for \
      nets that followed it. What the model does not restate: that secp256k1 (RFC-6979) and Ed25519 \
      signatures are deterministic, which is what makes identical signed deploys give identical \
      bytes — that is `crypto/`'s property and its own tests. The Rust check is at `create_genesis`: \
      the first bond is read from the file (not the sorted map, which loses file order) and the local \
      key must equal it. The witness is the two-key ceremony test: the first bond is admitted, the \
      second refuses with both keys named, and the autogen path's `.sk` supplies the generated first \
      bond's key for a standalone node." },

  { number := 85, layer := "Casper",
    statement := "**The finalized fringe's root is a member's claim about a fringe it finalises, not a \
      state field of the named block.** The ordinary answer serves the max `(height, hash)` member's \
      `fringe_state_hash`; a recovery anchor and the fresh-shard answer serve a block's \
      `post_state_hash`; and a member's claim is the state at the fringe **it** finalises — one \
      fringe earlier than the set it belongs to. The served root is therefore always some member's \
      claim, but it need not equal either state the block carries, which is why the singleton pin was \
      false and removed. **A checkpoint certificate is different:** it signs a fringe state root \
      itself (Law 104's `fringeState`, Law 108's root) — the anchor's own claim about the fringe it \
      finalised — so the joiner hydrates a quorum-signed root rather than a peer's reply, and the \
      signed value is re-checked against the anchor header's committed `fringe_state_hash`; the \
      legacy case with no signed root uses the member's post-state",
    status := .provedModel,
    declarations := [`Rchain.FringeClaim, `Rchain.ClaimsTheStateAtItsFringe, `Rchain.servedRoot,
      `Rchain.the_served_root_is_a_members_claim,
      `Rchain.the_served_root_is_the_state_at_the_members_fringe,
      `Rchain.singletonState, `Rchain.a_singleton_fringe_state_is_the_member_post_state],
    axioms := [],
    rust := ["casper/src/engine/node_running.rs", "casper/src/multi_parent_casper.rs",
      "casper/src/engine/node_syncing.rs"],
    rustWitness := [],
    witness := [`Rchain.a_members_claim_is_not_necessarily_a_block_state,
      `Rchain.the_served_root_is_a_members_claim],
    falsifiable := "The pin this branch first implemented: compare the served root with the named \
      block's pre- or post-state. `a_members_claim_is_not_necessarily_a_block_state` is its \
      refutation — a member's claim (the state at the empty fringe) is neither the block's pre-state \
      nor its post-state — and the node's `lfs_restore_dev_tax` suite found it in the genesis-branch \
      claim. The three answer branches are the remaining distinctions: they serve three different \
      kinds of value under one field name.",
    note := "**The member/finaliser distinction is the whole of it**, and `multi_parent_casper.rs` \
      records the stall it caused when a reader took a member for a finaliser (the base came out one \
      fringe early and the chain stopped at height 20). The responder's ordinary branch takes the max \
      member of the latest fringe and serves its membership claim anyway; the anchor and fresh-shard \
      branches serve `post_state_hash` instead. The law proves the served value is always a member's \
      claim (never an invented root) and that the claim is the state at that member's fringe; it does \
      **not** supply `stateAt` (the merge), which is why C339 stays open for callers of the ordinary \
      fringe reply. **The checkpoint path no longer relies on those replies:** the certificate \
      signs the anchor's fringe state and the joiner checks the signed value against the anchor \
      header's committed fringe root, so the run root is a quorum-signed, header-checked claim rather \
      than a peer's reply; the residual remains only for the ordinary/recovery replies. The legacy \
      certificate without a signed root uses the anchor post-state." },

  { number := 86, clause := "a", layer := "Admission",
    statement := "**Admission is a bounded queue, and the decision reads only the pool's bounds.** A \
      free slot with a new id admits; a full pool refuses with a named reason. The resource word \
      (`normal`/`throttled`/`critical`/`unavailable`) is not an input to `enqueue`: it is an input to \
      the execution budget (86d). The legacy boolean gate is modelled as `legacyAdmit`, and \
      `the_legacy_gate_refuses_an_empty_slot` is the height-1688 state as a theorem",
    status := .provedTied,
    corpus := some "admission",
    declarations := [`Rchain.Admission.Pool, `Rchain.Admission.enqueue,
      `Rchain.Admission.legacyAdmit, `Rchain.Admission.an_empty_slot_admits,
      `Rchain.Admission.a_full_pool_refuses,
      `Rchain.Admission.the_legacy_gate_refuses_an_empty_slot,
      `Rchain.Admission.AdmissionDecision, `Rchain.Admission.admissionSafe,
      `Rchain.Admission.a_safe_refusal_is_bounded_and_visible,
      `Rchain.Admission.a_safe_refusal_is_not_a_server_error,
      `Rchain.Admission.admission_decision_is_total,
      `Rchain.Admission.a_rate_refusal_without_a_retry_is_unsafe],
    axioms := [],
    rust := ["casper/src/dag.rs", "casper/src/pool.rs", "node/src/resources/fallback.rs",
      "node/src/web/http.rs", "node/src/api/grpc/mod.rs"],
    rustWitness := [
      "casper/tests/admission_trace.rs:a_count_refusal_replays_to_the_same_class",
      "node/src/api/grpc/mod.rs:the_ingress_guard_refuses_over_allowance_as_resource_exhausted"],
    witness := [`Rchain.Admission.an_empty_slot_admits,
      `Rchain.Admission.a_full_pool_refuses,
      `Rchain.Admission.the_legacy_gate_refuses_an_empty_slot,
      `Rchain.Admission.a_safe_refusal_is_bounded_and_visible,
      `Rchain.Admission.admission_decision_is_total,
      `Rchain.Admission.a_rate_refusal_without_a_retry_is_unsafe],
    falsifiable := "The live height-1688 state: an empty pool, a free slot, `throttled`, and a 503. \
      `the_legacy_gate_refuses_an_empty_slot` is that state proved; deleting the resource parameter \
      from `enqueue` is the change that would make it unprovable. A rule that admitted only above the \
      throttle floor would instead fail `an_empty_slot_admits`.",
    note := "The model's `capacity` is the count bound; the Rust's byte and per-deployer bounds are \
      the same `hasSlot` shape and are driven by the admission corpus when it lands. The row is \
      `proved-tied` on `spec/conformance/admission.tsv`; its Rust consumer drives \
      `casper::pool::check_admission`/`check_commit`, the same policy `dag.rs` calls. The corpus's \
      five cases cover the free slot, the count bound, the duplicate, one permit and zero headroom; \
      the rate, share and byte bounds are unit-tested in `casper::pool`. The law does **not** say a \
      node must accept unbounded work: a full pool is a real bound and refuses, with a retry \
      condition." },

  { number := 86, clause := "b", layer := "Admission",
    statement := "**No admitted work is silently lost.** A pool step advances an entry's status \
      (pending → running → done, or pending → expired) and never removes the row; every refusal \
      returns the pool it was given. `no_admitted_work_is_lost` proves every accepted id is in the \
      ledger at every reachable state, so a retirement is observable",
    status := .provedModel,
    declarations := [`Rchain.Admission.Accepted, `Rchain.Admission.StatusStep,
      `Rchain.Admission.Step, `Rchain.Admission.accepted_of_step,
      `Rchain.Admission.no_admitted_work_is_lost,
      `Rchain.Admission.a_dropping_pool_loses_work],
    axioms := [],
    rust := ["casper/src/dag.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.no_admitted_work_is_lost,
      `Rchain.Admission.a_refusal_changes_nothing],
    falsifiable := "`dropOnPressure` is the defect variant: it filters the entry out and \
      `a_dropping_pool_loses_work` exhibits the accepted id that is then absent. A step that removed \
      a row instead of advancing its status would make `no_admitted_work_is_lost` unprovable. The \
      expiry rule is the stated `pending → expired` transition, not an unobserved deletion.",
    note := "The ledger keeps `done` and `expired` rows rather than deleting them, so 'what happened \
      to this deploy?' is answerable after retirement. The Rust pool currently deletes an included \
      deploy on block insertion (`casper/src/dag.rs:1380-1417`) and keeps the deploy index; the model \
      states the property the index supplies, and the note names the difference. A rewind/reset is \
      outside this step relation and is Law 72's subject." },

  { number := 86, clause := "c", layer := "Admission",
    statement := "**Independent work commutes; conflicting work serialises.** At the effect level the \
      criterion is disjoint **closures** (`Independent`), not disjoint footprints; \
      `independent_pair_commutes` is the existing Law 9 theorem, and `independent_prefix_commutes` \
      moves an independent effect across a fold. At the state-change level, \
      `independent_change_commutes_with_a_fold` moves a `NonConflicting` change past a fold and \
      `independent_folds_commute` swaps two independent batches",
    status := .provedModel,
    declarations := [`Rchain.Admission.Independent,
      `Rchain.Admission.independent_pair_commutes,
      `Rchain.Admission.independent_prefix_commutes,
      `Rchain.Admission.independent_change_commutes_with_a_fold,
      `Rchain.Admission.independent_folds_commute,
      `Rchain.Admission.independent_batches_commute],
    axioms := [],
    rust := ["casper/src/runtime_manager.rs", "casper/src/merging.rs",
      "rspace/src/merger/state_change.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.independent_prefix_commutes,
      `Rchain.Admission.independent_change_commutes_with_a_fold,
      `Rchain.Admission.independent_folds_commute],
    falsifiable := "`effectA`/`effectB` have disjoint footprints and overlapping closures; \
      `effect_reorder_diverges` reaches different states in the two orders, so a footprint partition \
      is unsound. `nonConflicting_not_necessary` keeps the criterion honest: the law claims a \
      sufficient condition, not the port's conflict predicate.",
    note := "**The bridge is the obligation.** The Lean model's `NonConflicting` is over the \
      modelled `StateChange`; the Rust executor certifies a batch from its **dynamic** read/write \
      `TouchLog` (channels and native slots, read or written). That the touch log implies \
      `NonConflicting` is a refinement obligation, not a theorem here, which is why this row is \
      `proved-model` and never silently `proved-tied`. The first Rust release keeps \
      `batch.verify = true` (re-execute sequentially and compare state hash and every \
      `ProcessedDeploy`) with a circuit breaker; the differential batch suite is the witness." },

  { number := 86, clause := "d", layer := "Admission",
    statement := "**One permit progresses; capacity changes the schedule, not the queue.** A \
      non-empty pending pool with at least one permit has a step (`one_permit_progresses`), and \
      `next` takes at most `permits` ids (`the_batch_is_bounded_by_the_permits`). The defect shape is \
      `needsTwoPermits`: a non-empty pool with one permit and no step",
    status := .provedTied,
    corpus := some "admission",
    declarations := [`Rchain.Admission.Node, `Rchain.Admission.scheduleBatch,
      `Rchain.Admission.next, `Rchain.Admission.NodeStep,
      `Rchain.Admission.one_permit_progresses,
      `Rchain.Admission.scheduleBatch_length_le_permits,
      `Rchain.Admission.the_batch_is_bounded_by_the_permits,
      `Rchain.Admission.a_scheduler_that_needs_two_permits_stalls],
    axioms := [],
    rust := ["casper/src/budget.rs", "node/src/resources/budget.rs",
      "casper/src/blocks/proposer/proposer.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.one_permit_progresses,
      `Rchain.Admission.the_batch_is_bounded_by_the_permits,
      `Rchain.Admission.a_scheduler_that_needs_two_permits_stalls],
    falsifiable := "`needsTwoPermits` is the defect: a step relation that requires two permits \
      leaves a non-empty pool with one permit with no step, which \
      `a_scheduler_that_needs_two_permits_stalls` exhibits. The live node's boolean gate had the \
      stronger defect — no step even with every permit — and `the_legacy_gate_refuses_an_empty_slot` \
      is that half.",
    note := "The permit count is the headroom budget (64 MiB per execution in the first cut), never \
      a permission to enqueue. `enqueue` has no permit parameter; `next` does, so a smaller budget makes \
      a smaller batch and cannot change an admission answer. Critical/Unavailable may stop block \
      production; they do not stop admission or relay." },

  { number := 86, clause := "e", layer := "Admission",
    statement := "**Pending work is relocatable without loss.** A relay **copies** an entry to a peer \
      and leaves the sender's row alone (`relay_preserves_the_sender`); when the receiver has room \
      the copy is accepted (`relay_copies_to_the_receiver`); both pools keep every accepted id \
      (`relay_preserves_both_pools`). The defect shape is a **moving** relay, which loses work",
    status := .provedModel,
    declarations := [`Rchain.Admission.TwoNodes, `Rchain.Admission.relay,
      `Rchain.Admission.relay_preserves_the_sender,
      `Rchain.Admission.relay_copies_to_the_receiver,
      `Rchain.Admission.relay_preserves_both_pools,
      `Rchain.Admission.a_moving_relay_loses_work],
    axioms := [],
    rust := ["casper/src/dag.rs", "casper/src/api/block_api_impl.rs",
      "node/src/runtime/node_runtime.rs", "casper/src/protocol/client.rs"],
    rustWitness := [],
    witness := [`Rchain.Admission.relay_preserves_the_sender,
      `Rchain.Admission.relay_preserves_both_pools,
      `Rchain.Admission.a_moving_relay_loses_work],
    falsifiable := "`moveRelay` deletes from the sender and drops when the receiver is full; \
      `a_moving_relay_loses_work` exhibits the id absent from both pools. A relay that removed the \
      sender's row would make `relay_preserves_the_sender` unprovable.",
    note := "**The Rust relay is a copy over the deploy service, not a new wire message.** After an \
      admission, `BlockApiImpl` calls the operator-enabled `RelayFunction`, which offers the signed \
      deploy to each connected peer's external deploy service (`GrpcDeployService::deploy`). The \
      receiver runs the same signature, shard, rate, share, count and byte checks as a direct \
      submission, because it is the same `deploy` path; the sender keeps its own pool row. A bounded \
      seen set suppresses a relay circulating; the switch is `RNODE_DEPLOY_RELAY` (default off, API \
      port 40401, `RNODE_DEPLOY_RELAY_PORT` to change it). The model's receiver bound is the count \
      bound; the Rust's is the receiver's own pool." },

  { number := 87, clause := "a", layer := "Decimal",
    statement := "**Retired: the decimal's canonical form is no longer a rule of this tree.** The archived model canonicalised (stripping trailing zeros while the scale is positive, sending every zero to `(0,0)`), bounded the scale **after** canonicalising, and refused a non-canonical pair by name",
    status := .retired,
    axioms := [],
    rust := ["models/src/decimal.rs"],
    note := "**Retired 2026-10-31 by issue #527 (option R: remove the decimal type before mainnet).** The code read was the canonicalisation and the three constructor policies of the archived implementation, and the canonical-form refusals the wire and the literal parser applied. The port has no exact decimal value type: the type, its wire tag (35, now `reserved`) and its sort rank (14) are gone, `models/src/decimal.rs` and `spec/Rchain/Decimal.lean` are archived in place with dated headers, and the row is kept — not deleted — so the decision and the re-landing requirements travel with the number. `declarations`, `witness`, `rustWitness`, `corpus` and `falsifiable` were dropped with the model they named; the `rust` anchor survives because the archived file is the code that was read. Why it went, the evidence, and what a future attempt must do differently (model and corpus first, a versioned activation rather than an ungated pre-mainnet addition, and a wire-tag decision) are in `docs/src/formal/decimal-removed.md`." },
  { number := 87, clause := "b", layer := "Decimal",
    statement := "**Retired: there is no decimal value embedding in this tree.** The scaled-value injection (`scaled_value d = coefficient × 10^(38 − scale)`) and its order were the score the sorter's DECIMAL class used",
    status := .retired,
    axioms := [],
    rust := ["models/src/decimal.rs"],
    note := "**Retired 2026-10-31 by issue #527 (option R: remove the decimal type before mainnet).** The code read was the archived `scaled_value`/`cmp_numeric` pair and the sorter's removed DECIMAL (rank 14) score arm. The port has no exact decimal value type: the type, its wire tag (35, now `reserved`) and its sort rank (14) are gone, `models/src/decimal.rs` and `spec/Rchain/Decimal.lean` are archived in place with dated headers, and the row is kept — not deleted — so the decision and the re-landing requirements travel with the number. `declarations`, `witness`, `rustWitness`, `corpus` and `falsifiable` were dropped with the model they named; the `rust` anchor survives because the archived file is the code that was read. Why it went, the evidence, and what a future attempt must do differently (model and corpus first, a versioned activation rather than an ungated pre-mainnet addition, and a wire-tag decision) are in `docs/src/formal/decimal-removed.md`." },
  { number := 87, clause := "c", layer := "Decimal",
    statement := "**Retired: there is no exact decimal arithmetic in this tree.** Addition, subtraction and multiplication were exact and canonical, and satisfied the scaled identity the value algebra stated",
    status := .retired,
    axioms := [],
    rust := ["models/src/decimal.rs"],
    note := "**Retired 2026-10-31 by issue #527 (option R: remove the decimal type before mainnet).** The code read was the archived `add`/`sub`/`mul` and the reducer arms that routed decimal operands to them. The port has no exact decimal value type: the type, its wire tag (35, now `reserved`) and its sort rank (14) are gone, `models/src/decimal.rs` and `spec/Rchain/Decimal.lean` are archived in place with dated headers, and the row is kept — not deleted — so the decision and the re-landing requirements travel with the number. `declarations`, `witness`, `rustWitness`, `corpus` and `falsifiable` were dropped with the model they named; the `rust` anchor survives because the archived file is the code that was read. Why it went, the evidence, and what a future attempt must do differently (model and corpus first, a versioned activation rather than an ungated pre-mainnet addition, and a wire-tag decision) are in `docs/src/formal/decimal-removed.md`." },
  { number := 87, clause := "d", layer := "Decimal",
    statement := "**Retired: there is no decimal division rule in this tree.** Division was exact or an explicit `InexactDivision`, with a zero divisor refused by name — the limitation that made a fixed-point decimal useless for a ratio",
    status := .retired,
    axioms := [],
    rust := ["models/src/decimal.rs"],
    note := "**Retired 2026-10-31 by issue #527 (option R: remove the decimal type before mainnet).** The code read was the archived division and the reducer's division arm. The port has no exact decimal value type: the type, its wire tag (35, now `reserved`) and its sort rank (14) are gone, `models/src/decimal.rs` and `spec/Rchain/Decimal.lean` are archived in place with dated headers, and the row is kept — not deleted — so the decision and the re-landing requirements travel with the number. `declarations`, `witness`, `rustWitness`, `corpus` and `falsifiable` were dropped with the model they named; the `rust` anchor survives because the archived file is the code that was read. Why it went, the evidence, and what a future attempt must do differently (model and corpus first, a versioned activation rather than an ungated pre-mainnet addition, and a wire-tag decision) are in `docs/src/formal/decimal-removed.md`." },
  { number := 87, clause := "e", layer := "Decimal",
    statement := "**Retired: the `edecimal` payload bridge and sort rank 14 are gone.** The model's value order was the sorter's DECIMAL class order; both the class and the payload are retired, and rank 14 is never reused",
    status := .retired,
    axioms := [],
    rust := ["models/src/decimal.rs"],
    note := "**Retired 2026-10-31 by issue #527 (option R: remove the decimal type before mainnet).** The code read was the archived payload bridge, the removed `Expr.edecimal` constructor and the sorter's DECIMAL score arm; the `sort` corpus rows 43–50. The port has no exact decimal value type: the type, its wire tag (35, now `reserved`) and its sort rank (14) are gone, `models/src/decimal.rs` and `spec/Rchain/Decimal.lean` are archived in place with dated headers, and the row is kept — not deleted — so the decision and the re-landing requirements travel with the number. `declarations`, `witness`, `rustWitness`, `corpus` and `falsifiable` were dropped with the model they named; the `rust` anchor survives because the archived file is the code that was read. Why it went, the evidence, and what a future attempt must do differently (model and corpus first, a versioned activation rather than an ungated pre-mainnet addition, and a wire-tag decision) are in `docs/src/formal/decimal-removed.md`." },
]

end Laws
end Rchain
