//! The reporting casper's `trace` — a block replayed for its *events*, over three crates at once.
//!
//! `RhoReporter::trace` was unexecuted, and with it `replay_deploys`, `ReportingRuntime`'s replay path
//! and the reporting space's event collection: 128 missed lines across `casper/src/reporting.rs`,
//! `rholang/src/reporting_runtime.rs` and `rspace/src/reporting_rspace.rs`. The classification table
//! called it `harness-bound` and named the seam — a reporting space over the **same** store the block
//! was produced on, since `trace` resets to the block's pre-state and replays its deploys there.
//!
//! The fixture is therefore `casper/tests/common::build_runtime_manager`'s body with the store manager
//! **kept** rather than dropped, plus a block produced the way `casper/tests/block_index.rs` produces
//! one. What the test asserts is the reporting subsystem's whole correctness claim: the replayed
//! post-state hash equals the block's, and the deploy's events were collected.
//!
//! `mod common` is deliberately not used: that module builds the manager internally and returns only
//! the runtime manager, and the reporting space needs the store the runtime was built over.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rchain_casper::block_random_seed::BlockRandomSeed;
use rchain_casper::genesis::contracts::Vault;
use rchain_casper::reporting::{rho_reporter, ReportingCasper};
use rchain_casper::runtime_manager::{MergeableStore, NativeChangesStore, RuntimeManager};
use rchain_casper::system_deploy::SystemDeploy;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::public_key::PublicKey;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, DeployData, RholangState, SignedDeployData,
};
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_models::validator::Validator;
use rchain_rholang::merging::{DeployMergeableDataCodec, NativeSidecarCodec};
use rchain_rholang::native_state::PosGenesis;
use rchain_rholang::reporting_runtime::create_reporting_rspace;
use rchain_rholang::runtime::{ReplayRhoRuntime, RhoRuntime};
use rchain_rholang::scheduler::EffectMode;
use rchain_rholang::storage::RhoMatch;
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_rspace::factory::create_history_repository;
use rchain_rspace::hot_store::InMemHotStore;
use rchain_rspace::rspace::RSpace;
use rchain_shared::refined::NonNegI64;
use rchain_shared::store_manager::{database, InMemoryStoreManager};
use rchain_shared::typed_store::BytesCodec;

/// The runtime manager **and the store manager it was built over**. The second half is the point:
/// `create_reporting_rspace` builds a reporting space over a store manager, and for the reporter to
/// find the block's pre-state, it has to be the *same* store the runtime wrote to. It is an `Arc`
/// because `rho_reporter` takes an `Fn` that creates a space on every call.
async fn build_runtime_and_manager() -> (RuntimeManager, Arc<InMemoryStoreManager>) {
    let manager = Arc::new(InMemoryStoreManager::default());
    let history = create_history_repository::<
        SortedProc,
        BindPattern,
        ListParWithRandom,
        TaggedContinuation,
    >(&*manager, "rspace")
    .await
    .expect("history repository");
    let reader = history.get_history_reader(history.root()).await;
    let hot = Arc::new(InMemHotStore::new(reader.base()));
    let (play, replay) = RSpace::create_with_replay(history.clone(), hot, Arc::new(RhoMatch));
    let rho = RhoRuntime::create_with_effect_mode(
        play,
        history.clone(),
        SortedProc::default(),
        true,
        EffectMode::Sequential,
    )
    .await
    .expect("rho runtime");
    let replay = ReplayRhoRuntime::create(Arc::new(replay), history.clone(), SortedProc::default())
        .await
        .expect("replay runtime");
    let mergeable: MergeableStore = Arc::new(
        database(
            &*manager,
            "mergeable",
            Arc::new(BytesCodec),
            Arc::new(DeployMergeableDataCodec),
        )
        .await
        .expect("mergeable store"),
    );
    let native_changes: NativeChangesStore = Arc::new(
        database(
            &*manager,
            "native-changes",
            Arc::new(BytesCodec),
            Arc::new(NativeSidecarCodec),
        )
        .await
        .expect("native changes store"),
    );
    (
        RuntimeManager::new(
            rho,
            replay,
            history,
            mergeable,
            native_changes,
            EffectMode::Sequential,
        ),
        manager,
    )
}

/// The state hash of the last finalised fringe as of a synthetic block — the value the block's close
/// system deploy anchors the next epoch's active-set seed to. Supplied to both the play and the replay
/// here, exactly as a node derives it once from its own DAG and gives it to both paths.
fn fringe_state(seed: u8) -> rchain_crypto::hash::blake2b256_hash::Blake2b256Hash {
    rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::create(&[seed])
}

fn deploy(term: &str) -> SignedDeployData {
    SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: term.to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit: 500_000,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: vec![0u8; 65],
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// The deployer's vault — the deploy above is unfunded, so the block's deploy must be payable.
fn seeded_vault() -> Vault {
    Vault {
        rev_address: RevAddress::from_public_key(&PublicKey::new(vec![0u8; 65]))
            .expect("valid rev address"),
        initial_balance: NonNegI64::try_from(1_000_000_000).unwrap(),
    }
}

/// The block shell: the seed the reporter replays with comes from the block, so the hash and height
/// are fixed here and the state is filled in after the deploy has been run under that seed.
fn block_shell() -> BlockMessage {
    BlockMessage {
        version: 1,
        shard_id: "root".to_string(),
        block_hash: BlockHash::new([0x33; 32]),
        block_number: 1.try_into().expect("height 1"),
        sender: Validator::new([0u8; 65]),
        seq_num: 1.try_into().expect("seq 1"),
        pre_state_hash: StateHash::new([0u8; 32]),
        post_state_hash: StateHash::new([0u8; 32]),
        // Non-empty so the replay charges costs, as the play that produced the state did.
        justifications: vec![BlockHash::new([0x44; 32])],
        bonds: BTreeMap::new(),
        rejected_deploys: BTreeSet::new(),
        rejected_blocks: BTreeSet::new(),
        rejected_senders: BTreeSet::new(),
        state: RholangState::default(),
        sig_algorithm: "secp256k1".to_string(),
        sig: Vec::new(),
        timestamp: 0,
    }
}

/// **A block replayed for its events.** The term both produces and consumes, so the report has
/// something to collect in more than one shape.
///
/// **What this can and cannot see.** The system deploy is replayed and reported, and that is the claim
/// here — its *effect* is not: `CloseBlock` writes to the runtime's native store, which is a sidecar
/// beside the trie (like the mergeable store), so a replayed close-block at the wrong height leaves the
/// post-state hash unchanged. Measured, not assumed: planting `block_number + 1` in the replay's
/// `close_block` passes this test, which is why this says *path* rather than *effect*. The op's own
/// behaviour is pinned by `native_state`'s tests, where the write is observable.
///
/// The three assertions are the subsystem's claims:
///
///   * the replay reaches the **block's own post-state hash** — if the reporting replay diverged from
///     the block path, the tool would report events for a state the chain is not in, which is worse
///     than reporting nothing;
///   * one user deploy is reported, with the deploy it processed;
///   * its events were **collected** — the reporting space's whole product, and the reason the
///     transformer and the space exist at all.
#[tokio::test]
async fn the_reporter_replays_a_block_and_collects_its_events() {
    let (rm, manager) = build_runtime_and_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    let (_pre, genesis_post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let mut block = block_shell();
    block.pre_state_hash = StateHash::new(*genesis_post.as_bytes());
    let block_rand =
        BlockRandomSeed::random_generator_from_block(&block).expect("valid block seed");

    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &genesis_post,
            &[deploy(r#"new c in { c!(1) | for (x <- c) { Nil } }"#)],
            // A **system** deploy as well as a user one: the replay of a system deploy is a different
            // path (`RuntimeReplayOps::replay_block_system_deploy`, and the `eval_system_deploy`/
            // `consume_system_result` pair under it), and a block with none leaves it unreachable.
            &[SystemDeploy::close_block(
                1,
                fringe_state(1),
                BTreeMap::new(),
                block_rand.clone(),
            )],
            &block_rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "the deploy must succeed or there is nothing to report: {:?}",
        user_results[0].eval_result.errors
    );

    block.post_state_hash = StateHash::new(*post_state.as_bytes());
    block.state = RholangState {
        deploys: user_results.into_iter().map(|r| r.deploy).collect(),
        system_deploys: sys_results.into_iter().map(|r| r.deploy).collect(),
    };

    // The reporting space over the same store, and the reporter that creates one per trace.
    let reporter = rho_reporter(
        {
            let manager = manager.clone();
            move || {
                let manager = manager.clone();
                async move { create_reporting_rspace(&*manager).await }
            }
        },
        SortedProc::default(),
        // A reporter has no DAG, so the close deploy's two derived inputs are supplied to it — here the
        // same values the play above handed to `close_block`, which is what a node gets from the DAG
        // for that block.
        |_block| async move { Ok((fringe_state(1), BTreeMap::new(), BTreeSet::new())) },
    );

    let result = reporter
        .trace(block.clone())
        .await
        .expect("the reporter replays the block");

    assert_eq!(
        result.post_state_hash, block.post_state_hash,
        "the reporting replay must reach the block's own post-state"
    );
    assert_eq!(
        result.deploy_report_result.len(),
        1,
        "one user deploy, one report"
    );
    let reported = &result.deploy_report_result[0];
    assert_eq!(
        reported.processed_deploy.deploy.deployer, block.state.deploys[0].deploy.deployer,
        "and it is the deploy the block carried"
    );
    assert!(
        !reported.events.is_empty() && reported.events.iter().any(|batch| !batch.is_empty()),
        "the reporting space collected the deploy's events: {:?}",
        reported.events
    );
    assert_eq!(
        result.system_deploy_report_result.len(),
        1,
        "the block's system deploy is replayed and reported too"
    );
}
