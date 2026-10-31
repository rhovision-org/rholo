//! Recovery must derive fringe metadata rather than replay against the peer's claims.
mod common;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMessageCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_block_storage::syntax::insert_genesis;
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_casper::blocks::proposer::block_creator::BlockCreator;
use rchain_casper::blocks::proposer::propose_result::BlockCreatorResult;
use rchain_casper::dag::BlockDagKeyValueStorage;
use rchain_casper::engine::recovery::RecoveryReplay;
use rchain_casper::genesis::contracts::{ProofOfStake, Registry, Validator as GenesisValidator};
use rchain_casper::genesis::{build_pos_genesis, create_genesis_block, Genesis};
use rchain_casper::interpreter_util::ShardHealth;
use rchain_casper::merging::BlockIndex;
use rchain_casper::multi_parent_casper::{get_pre_state_for_parents, validate};
use rchain_casper::runtime_manager::{MergeableStore, NativeChangesStore, RuntimeManager};
use rchain_casper::validator_identity::ValidatorIdentity;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{BlockFringe, FinalizedFringe};
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_rholang::merging::{DeployMergeableDataCodec, NativeSidecarCodec};
use rchain_rholang::runtime::{ReplayRhoRuntime, RhoRuntime};
use rchain_rholang::scheduler::EffectMode;
use rchain_rholang::storage::RhoMatch;
use rchain_rspace::hot_store::InMemHotStore;
use rchain_rspace::rspace::RSpace;
use rchain_shared::log::{Log, NopLog};
use rchain_shared::refined::NonNegI64;
use rchain_shared::store_manager::{database, InMemoryStoreManager};
use rchain_shared::typed_store::BytesCodec;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

async fn dag() -> BlockDagKeyValueStorage {
    let manager = InMemoryStoreManager::default();
    let metadata = database(
        &manager,
        "metadata",
        Arc::new(BlockHashCodec),
        Arc::new(BlockMetadataCodec),
    )
    .await
    .unwrap();
    let index = database(
        &manager,
        "index",
        Arc::new(BytesCodec),
        Arc::new(BlockHashCodec),
    )
    .await
    .unwrap();
    let deploys = database(
        &manager,
        "deploys",
        Arc::new(BytesCodec),
        Arc::new(SignedDeployDataCodec),
    )
    .await
    .unwrap();
    BlockDagKeyValueStorage::create(
        Arc::new(
            BlockMetadataStore::create(Arc::new(metadata))
                .await
                .unwrap(),
        ),
        Arc::new(index),
        Arc::new(deploys),
    )
    .await
    .unwrap()
}

// A separate verifier runtime over the received content, with empty sidecar stores.
async fn cold_runtime(producer: &RuntimeManager) -> RuntimeManager {
    let history = producer.get_history_repo().clone();
    let reader = history.get_history_reader(history.root()).await;
    let hot = Arc::new(InMemHotStore::new(reader.base()));
    let (play,replay) = RSpace::<SortedProc,BindPattern,ListParWithRandom,TaggedContinuation>::create_with_replay(history.clone(),hot,Arc::new(RhoMatch));
    let play = RhoRuntime::create(play, history.clone(), SortedProc::default())
        .await
        .unwrap();
    let replay = ReplayRhoRuntime::create(Arc::new(replay), history.clone(), SortedProc::default())
        .await
        .unwrap();
    let manager = InMemoryStoreManager::default();
    let mergeable: MergeableStore = Arc::new(
        database(
            &manager,
            "mergeable",
            Arc::new(BytesCodec),
            Arc::new(DeployMergeableDataCodec),
        )
        .await
        .unwrap(),
    );
    let native: NativeChangesStore = Arc::new(
        database(
            &manager,
            "native",
            Arc::new(BytesCodec),
            Arc::new(NativeSidecarCodec),
        )
        .await
        .unwrap(),
    );
    RuntimeManager::new(
        play,
        replay,
        history,
        mergeable,
        native,
        EffectMode::Sequential,
    )
    .with_genesis_pos(producer.genesis_pos().clone())
}

#[test]
fn independent_recovery_replays_epoch_boundaries_and_refuses_forged_claims() {
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(32 * 1024 * 1024)
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let identity = ValidatorIdentity::from_hex(&"07".repeat(32)).unwrap();
                    let pos = ProofOfStake {
                        minimum_bond: NonNegI64::one(),
                        maximum_bond: NonNegI64::try_from(100).unwrap(),
                        validators: vec![GenesisValidator {
                            pk: identity.public_key.clone(),
                            stake: NonNegI64::try_from(100).unwrap(),
                        }],
                        epoch_length: 3,
                        quarantine_length: 3,
                        number_of_active_validators: 1,
                        executor_share: NonNegI64::zero(),
                        absence_slack: NonNegI64::zero(),
                        participation_grace: NonNegI64::zero(),
                        min_phlo_price: NonNegI64::zero(),
                        max_number_of_parents: 0,
                        dev_tax_vesting_blocks: NonNegI64::zero(),
                        pos_multi_sig_public_keys: Vec::new(),
                        pos_multi_sig_quorum: 1,
                        pos_vault_pub_key: String::new(),

                        liveness_absence_threshold: 0,
                        liveness_jail_after_boundaries: 0,
                        liveness_jail_epochs: 0,
                        liveness_vote_window: 0,
                        liveness_slash_severity: 0,
                    };
                    let runtime = common::build_runtime_manager()
                        .await
                        .with_genesis_pos(build_pos_genesis(&pos, None));
                    let genesis = create_genesis_block(
                        &identity,
                        &Genesis {
                            sender: identity.public_key.clone(),
                            shard_id: "/root".to_string(),
                            block_number: 0,
                            proof_of_stake: pos,
                            registry: Registry {
                                system_contract_pub_key: String::new(),
                            },
                            vaults: Vec::new(),
                            dev_vesting: None,
                        },
                        &runtime,
                    )
                    .await
                    .unwrap();
                    let manager = InMemoryStoreManager::default();
                    let store: BlockStore = Arc::new(
                        database(
                            &manager,
                            "blocks",
                            Arc::new(BlockHashCodec),
                            Arc::new(BlockMessageCodec),
                        )
                        .await
                        .unwrap(),
                    );
                    store
                        .put(&[(genesis.block_hash, genesis.clone())])
                        .await
                        .unwrap();
                    let source = dag().await;
                    insert_genesis(&source, genesis.clone()).await.unwrap();
                    let mut heights = BTreeMap::from([(0, BTreeSet::from([genesis.block_hash]))]);
                    let log: Arc<dyn Log> = Arc::new(NopLog);
                    let health = ShardHealth::new();
                    let creator = BlockCreator {
                        id: identity,
                        shard_id: "/root".to_string(),
                    };
                    let mut tip = genesis.block_hash;
                    for height in 1..=12 {
                        let index = |hash| {
                            let source = &source;
                            let store = &store;
                            let runtime = &runtime;
                            async move {
                                let meta = source.lookup(&hash).await?.unwrap();
                                BlockIndex::get_block_index(
                                    runtime,
                                    source,
                                    store,
                                    hash,
                                    Blake2b256Hash::from_byte_array(
                                        meta.fringe_state_hash.as_bytes(),
                                    ),
                                )
                                .await
                            }
                        };
                        let pre = get_pre_state_for_parents(
                            &source,
                            &store,
                            &runtime,
                            &BTreeSet::from([tip]),
                            &index,
                        )
                        .await
                        .unwrap();
                        let BlockCreatorResult::Created(block) = creator
                            .create(
                                &runtime,
                                &source,
                                &pre,
                                &[],
                                &BTreeMap::new(),
                                true,
                                false,
                                None,
                            )
                            .await
                            .unwrap()
                        else {
                            panic!("block not created")
                        };
                        let meta = validate(
                            &source, &store, &runtime, &block, "/root", 0, 0, &index, &log, &health,
                        )
                        .await
                        .unwrap();
                        tip = block.block_hash;
                        store.put(&[(tip, block.clone())]).await.unwrap();
                        source.insert(meta, block).await.unwrap();
                        heights.insert(height, BTreeSet::from([tip]));
                    }
                    let mut ancestry = Vec::new();
                    for hash in heights.values().flat_map(|set| set.iter()) {
                        let meta = source.lookup(hash).await.unwrap().unwrap();
                        ancestry.push(BlockFringe {
                            block_hash: *hash,
                            fringe: meta.fringe,
                            fringe_state_hash: meta.fringe_state_hash,
                        });
                    }
                    let target = FinalizedFringe {
                        hashes: vec![tip],
                        state_hash: source
                            .lookup(&tip)
                            .await
                            .unwrap()
                            .unwrap()
                            .fringe_state_hash,
                        ancestry,
                    };
                    BlockIndex::prune_cache(
                        &heights
                            .values()
                            .flat_map(|set| set.iter().copied())
                            .collect::<Vec<_>>(),
                    );
                    let replay = RecoveryReplay {
                        runtime: Arc::new(cold_runtime(&runtime).await),
                        shard_id: "/root".to_string(),
                        min_phlo_price: 0,
                        max_number_of_parents: 0,
                        expected_genesis: Some(genesis.block_hash),
                        anchor: None,
                    };
                    let verified = replay
                        .verify(&store, &heights, &target, &log)
                        .await
                        .unwrap();
                    let restored = dag().await;
                    assert!(restored.get_representation().await.dag_set.is_empty());
                    verified.publish(&restored, &store).await.unwrap();
                    assert_eq!(restored.get_representation().await.dag_set.len(), 13);
                    for hash in heights.values().flat_map(|set| set.iter()) {
                        let a = source.lookup(hash).await.unwrap().unwrap();
                        let b = restored.lookup(hash).await.unwrap().unwrap();
                        assert_eq!(
                            (a.fringe, a.fringe_state_hash),
                            (b.fringe, b.fringe_state_hash)
                        );
                    }
                    let mut lying = target.clone();
                    lying.ancestry.last_mut().unwrap().fringe_state_hash = StateHash::new([9; 32]);
                    assert!(replay
                        .verify(&store, &heights, &lying, &log)
                        .await
                        .unwrap_err()
                        .contains("disagrees"));
                    let mut wrong_fringe = target.clone();
                    let claimed = &mut wrong_fringe.ancestry.last_mut().unwrap().fringe;
                    *claimed = if claimed.is_empty() {
                        BTreeSet::from([genesis.block_hash])
                    } else {
                        BTreeSet::new()
                    };
                    assert!(replay
                        .verify(&store, &heights, &wrong_fringe, &log)
                        .await
                        .unwrap_err()
                        .contains("disagrees"));
                    let mut lying = target.clone();
                    lying.state_hash = StateHash::new([9; 32]);
                    assert!(replay
                        .verify(&store, &heights, &lying, &log)
                        .await
                        .unwrap_err()
                        .contains("target state"));
                    let mut no_metadata = target.clone();
                    no_metadata.ancestry.clear();
                    replay
                        .verify(&store, &heights, &no_metadata, &log)
                        .await
                        .unwrap();
                    let mut wrong_pin = replay.clone();
                    wrong_pin.expected_genesis = Some(BlockHash::new([9; 32]));
                    assert!(wrong_pin
                        .verify(&store, &heights, &target, &log)
                        .await
                        .unwrap_err()
                        .contains("pinned genesis"));
                    let mut missing = heights.clone();
                    missing.remove(&6);
                    assert!(replay
                        .verify(&store, &missing, &no_metadata, &log)
                        .await
                        .is_err());
                    let mut anchored = replay.clone();
                    anchored.anchor = Some(tip);
                    let mut anchor_target = target.clone();
                    anchor_target.state_hash = store
                        .get(&[tip])
                        .await
                        .unwrap()
                        .into_iter()
                        .flatten()
                        .next()
                        .unwrap()
                        .post_state_hash;
                    anchored
                        .verify(&store, &heights, &anchor_target, &log)
                        .await
                        .unwrap();
                    // The producer extends its own DAG; the restored node validates
                    // four new blocks, including the height-15 epoch boundary.
                    let before = restored
                        .get_representation()
                        .await
                        .latest_fringe()
                        .iter()
                        .map(|message| i64::from(message.height))
                        .max()
                        .unwrap_or(0);
                    for _height in 13..=16 {
                        let source_index = |hash| {
                            let source = &source;
                            let store = &store;
                            let runtime = &runtime;
                            async move {
                                let meta = source.lookup(&hash).await?.unwrap();
                                BlockIndex::get_block_index(
                                    runtime,
                                    source,
                                    store,
                                    hash,
                                    Blake2b256Hash::from_byte_array(
                                        meta.fringe_state_hash.as_bytes(),
                                    ),
                                )
                                .await
                            }
                        };
                        let pre = get_pre_state_for_parents(
                            &source,
                            &store,
                            &runtime,
                            &BTreeSet::from([tip]),
                            &source_index,
                        )
                        .await
                        .unwrap();
                        let BlockCreatorResult::Created(next) = creator
                            .create(
                                &runtime,
                                &source,
                                &pre,
                                &[],
                                &BTreeMap::new(),
                                true,
                                false,
                                None,
                            )
                            .await
                            .unwrap()
                        else {
                            panic!("new block not created")
                        };
                        let expected = validate(
                            &source,
                            &store,
                            &runtime,
                            &next,
                            "/root",
                            0,
                            0,
                            &source_index,
                            &log,
                            &health,
                        )
                        .await
                        .unwrap();
                        store.put(&[(next.block_hash, next.clone())]).await.unwrap();
                        source.insert(expected.clone(), next.clone()).await.unwrap();
                        let index = |hash| {
                            let restored = &restored;
                            let store = &store;
                            let runtime = &replay.runtime;
                            async move {
                                let meta = restored.lookup(&hash).await?.unwrap();
                                BlockIndex::get_block_index(
                                    runtime,
                                    restored,
                                    store,
                                    hash,
                                    Blake2b256Hash::from_byte_array(
                                        meta.fringe_state_hash.as_bytes(),
                                    ),
                                )
                                .await
                            }
                        };
                        let actual = validate(
                            &restored,
                            &store,
                            &replay.runtime,
                            &next,
                            "/root",
                            0,
                            0,
                            &index,
                            &log,
                            &health,
                        )
                        .await
                        .unwrap();
                        assert!(!actual.validation_failed);
                        assert_eq!(
                            (actual.fringe.clone(), actual.fringe_state_hash),
                            (expected.fringe, expected.fringe_state_hash)
                        );
                        tip = next.block_hash;
                        restored.insert(actual, next).await.unwrap();
                    }
                    let after = restored
                        .get_representation()
                        .await
                        .latest_fringe()
                        .iter()
                        .map(|message| i64::from(message.height))
                        .max()
                        .unwrap_or(0);
                    assert!(
                        after > before,
                        "the restored node's finalized fringe must advance on new work"
                    );
                });
        })
        .unwrap()
        .join()
        .unwrap();
}
