//! **The cold join**: a node with the certificate, the certified roots and the bounded join bodies
//! validates the first post-anchor block.
//!
//! The measured live failure (`restart-expiry-v5`, source `33d735041`) was that a wiped validator
//! accepted the checkpoint and then could never validate the first post-anchor block. The unit
//! fixture below is that node: a fresh DAG of headers, a cold runtime with empty sidecar stores, and
//! a block store holding only the genesis plus the join bodies (the anchor, the fringe it finalised,
//! and those members' own fringes). Before the fix the merge read a genesis state the certified sync
//! never hydrates and fringe bodies it never fetches.
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
use rchain_casper::checkpoint::VerifiedCheckpoint;
use rchain_casper::dag::BlockDagKeyValueStorage;
use rchain_casper::engine::anchor::{AcceptedState, AcceptedWindow, SharedAcceptedWindow};
use rchain_casper::genesis::contracts::{ProofOfStake, Registry, Validator as GenesisValidator};
use rchain_casper::genesis::{build_pos_genesis, create_genesis_block, Genesis};
use rchain_casper::interpreter_util::ShardHealth;
use rchain_casper::merging::BlockIndex;
use rchain_casper::multi_parent_casper::{
    get_pre_state_for_parents, validate, validate_with_window,
};
use rchain_casper::runtime_manager::{MergeableStore, NativeChangesStore, RuntimeManager};
use rchain_casper::validator_identity::ValidatorIdentity;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_models::casper::protocol::checkpoint::Checkpoint;
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_rholang::merging::{DeployMergeableDataCodec, NativeSidecarCodec};
use rchain_rholang::runtime::{ReplayRhoRuntime, RhoRuntime};
use rchain_rholang::scheduler::EffectMode;
use rchain_rholang::storage::RhoMatch;
use rchain_rspace::hot_store::InMemHotStore;
use rchain_rspace::rspace::RSpace;
use rchain_shared::base16;
use rchain_shared::log::{Log, NopLog};
use rchain_shared::refined::NonNegI64;
use rchain_shared::store_manager::{database, InMemoryStoreManager, KeyValueStoreManager};
use rchain_shared::typed_store::{BytesCodec, KeyValueTypedStore, KeyValueTypedStoreCodec};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::RwLock;

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
    // **The durable-records store is attached because `BlockCreator::create` writes the validator's
    // own signed sequence before it returns** (Law 122): the whole point of that write is that a
    // node which cannot record the signature must not produce a block, so a fixture that drives
    // the creator has to give it somewhere to record — the same reason the node attaches it.
    let records: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> =
        Arc::new(KeyValueTypedStoreCodec::new(
            manager.store("records").await.unwrap(),
            Arc::new(BytesCodec),
            Arc::new(BytesCodec),
        ));
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
    .with_reset_marker(records)
}

async fn blocks() -> BlockStore {
    let manager = InMemoryStoreManager::default();
    Arc::new(
        database(
            &manager,
            "blocks",
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        )
        .await
        .unwrap(),
    )
}

/// A verifier runtime with **empty sidecar stores**: the joiner's index regeneration path.
async fn cold_runtime(producer: &RuntimeManager) -> RuntimeManager {
    let history = producer.get_history_repo().clone();
    let reader = history.get_history_reader(history.root()).await;
    let hot = Arc::new(InMemHotStore::new(reader.base()));
    let (play, replay) = RSpace::<
        SortedProc,
        BindPattern,
        ListParWithRandom,
        TaggedContinuation,
    >::create_with_replay(history.clone(), hot, Arc::new(RhoMatch));
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

fn pos() -> ProofOfStake {
    ProofOfStake {
        minimum_bond: NonNegI64::one(),
        maximum_bond: NonNegI64::try_from(100).unwrap(),
        validators: vec![GenesisValidator {
            pk: ValidatorIdentity::from_hex(&"07".repeat(32))
                .unwrap()
                .public_key,
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
    }
}

#[test]
fn a_wiped_joiner_validates_the_first_post_anchor_block_from_the_certificate_and_join_bodies() {
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
                    let pos = pos();
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
                    let source_store = blocks().await;
                    source_store
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
                        received_claims: None,
                    };
                    let mut tip = genesis.block_hash;
                    // The anchor sits beyond `DEPLOY_LIFESPAN` so the wiped-store assertion below
                    // is real: the deploy-duplication window is heights 11..=60, and height 3 is
                    // outside every input this joiner is given.
                    let anchor_height = 60;

                    // The producer runs to the anchor, snapshots the certificate there, then
                    // produces the first post-anchor block.
                    let mut anchor: Option<(
                        BlockMessage,
                        rchain_models::block_metadata::BlockMetadata,
                        BTreeSet<BlockHash>,
                    )> = None;
                    for height in 1..=anchor_height + 1 {
                        let index = |hash| {
                            let source = &source;
                            let store = &source_store;
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
                            &source_store,
                            &runtime,
                            &BTreeSet::from([tip]),
                            &index,
                            None,
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
                            &source,
                            &source_store,
                            &runtime,
                            &block,
                            "/root",
                            0,
                            0,
                            &index,
                            &log,
                            &health,
                        )
                        .await
                        .unwrap();
                        tip = block.block_hash;
                        source_store.put(&[(tip, block.clone())]).await.unwrap();
                        source.insert(meta, block.clone()).await.unwrap();
                        heights.insert(height, BTreeSet::from([tip]));
                        if height == anchor_height {
                            let meta = source.lookup(&tip).await.unwrap().unwrap();
                            let latest: BTreeSet<BlockHash> = source
                                .get_representation()
                                .await
                                .latest_fringe()
                                .iter()
                                .map(|m| m.id)
                                .collect();
                            anchor = Some((block.clone(), meta, latest));
                        }
                        if height == anchor_height + 1 {
                            // The post-anchor block is the one the joiner must validate.
                            break;
                        }
                    }
                    let (anchor_block, anchor_meta, latest_fringe) =
                        anchor.expect("the anchor snapshot was taken at the anchor height");
                    let first_post_anchor = source_store
                        .get(&[tip])
                        .await
                        .unwrap()
                        .into_iter()
                        .flatten()
                        .next()
                        .expect("the first post-anchor block is stored");
                    assert_eq!(i64::from(first_post_anchor.block_number), anchor_height + 1);
                    assert!(
                        !anchor_meta.fringe.is_empty(),
                        "the anchor finalised a fringe"
                    );

                    // **The certificate as the producer would sign it.**
                    let checkpoint = Checkpoint {
                        shard_id: "/root".to_string(),
                        anchor: base16::encode(anchor_block.block_hash.as_bytes()),
                        anchor_height,
                        anchor_post_state: base16::encode(anchor_block.post_state_hash.as_bytes()),
                        fringe_state: Some(base16::encode(
                            anchor_meta.fringe_state_hash.as_bytes(),
                        )),
                        fringe_hashes: latest_fringe
                            .iter()
                            .map(|h| base16::encode(h.as_bytes()))
                            .collect(),
                        parent_digest: None,
                        epoch: anchor_height / 3,
                        metadata_root: None,
                        history_root: None,
                        fringe_headers_root: None,
                    };
                    let verified = VerifiedCheckpoint {
                        checkpoint,
                        digest: Blake2b256Hash::create(&[]),
                        supporting_stake: 100,
                        total_stake: 100,
                    };

                    // **The wiped joiner: headers only, plus the bounded join bodies.**
                    let restored = dag().await;
                    for height in 0..=anchor_height {
                        let hash = *heights.get(&height).unwrap().iter().next().unwrap();
                        let meta = source.lookup(&hash).await.unwrap().unwrap();
                        restored.insert_metadata(meta).await.unwrap();
                    }
                    let joiner_store = blocks().await;
                    let mut join_bodies: BTreeSet<BlockHash> =
                        BTreeSet::from([anchor_block.block_hash]);
                    join_bodies.extend(anchor_meta.fringe.iter().copied());
                    for member in &anchor_meta.fringe {
                        let meta = source.lookup(member).await.unwrap().unwrap();
                        join_bodies.extend(meta.fringe.iter().copied());
                    }
                    // The deploy-duplication window: bodies only, no replay, no roots.
                    let deploy_floor = anchor_height - 50;
                    for (height, hashes) in &heights {
                        if *height > deploy_floor && *height <= anchor_height {
                            join_bodies.extend(hashes.iter().copied());
                        }
                    }
                    for hash in &join_bodies {
                        let body = source_store
                            .get(&[*hash])
                            .await
                            .unwrap()
                            .into_iter()
                            .flatten()
                            .next()
                            .unwrap_or_else(|| {
                                panic!("join body {} is on the producer", hash.to_hex())
                            });
                        joiner_store.put(&[(*hash, body)]).await.unwrap();
                    }
                    // The wipe is real: a historical block outside the join set has no body.
                    let historical = *heights.get(&3).unwrap().iter().next().unwrap();
                    assert!(
                        !join_bodies.contains(&historical),
                        "height 3 is not a join input"
                    );
                    assert!(joiner_store
                        .get(&[historical])
                        .await
                        .unwrap()
                        .into_iter()
                        .flatten()
                        .next()
                        .is_none());

                    // The certified window: every header's three roots (the sync hydrates exactly
                    // the certificate's set; this fixture's shared history holds them).
                    let mut window = BTreeSet::new();
                    for height in 0..=anchor_height {
                        let hash = *heights.get(&height).unwrap().iter().next().unwrap();
                        let meta = source.lookup(&hash).await.unwrap().unwrap();
                        for root in [
                            meta.pre_state_hash,
                            meta.post_state_hash,
                            meta.fringe_state_hash,
                        ] {
                            window.insert(Blake2b256Hash::from_byte_array(root.as_bytes()));
                        }
                    }
                    let accepted =
                        AcceptedState::from_certificate(verified, window).expect("accepted state");
                    let accepted_window = AcceptedWindow::from_accepted(&accepted);
                    let shared: SharedAcceptedWindow = Arc::new(RwLock::new(Some(accepted_window)));
                    let joiner_runtime = cold_runtime(&runtime).await;

                    let index = |hash| {
                        let restored = &restored;
                        let store = &joiner_store;
                        let runtime = &joiner_runtime;
                        async move {
                            let meta = restored.lookup(&hash).await?.unwrap();
                            BlockIndex::get_block_index(
                                runtime,
                                restored,
                                store,
                                hash,
                                Blake2b256Hash::from_byte_array(meta.fringe_state_hash.as_bytes()),
                            )
                            .await
                        }
                    };
                    let actual = validate_with_window(
                        &restored,
                        &joiner_store,
                        &joiner_runtime,
                        &first_post_anchor,
                        "/root",
                        0,
                        0,
                        &index,
                        &log,
                        &health,
                        Some(&shared),
                    )
                    .await
                    .expect("the first post-anchor block must validate on the wiped joiner");
                    assert!(!actual.validation_failed);
                    let producer_meta = source.lookup(&tip).await.unwrap().unwrap();
                    assert_eq!(
                        (actual.fringe, actual.fringe_state_hash),
                        (producer_meta.fringe, producer_meta.fringe_state_hash),
                        "the joiner derives the producer's fringe and fringe state"
                    );

                    // **The deploy-window arm.** The first post-anchor block's `repeat_deploy`
                    // scan walks every parent body above `anchor_height - DEPLOY_LIFESPAN`. A
                    // joiner that fetches only the replay layer cannot validate the block. Remove
                    // one window body and require the failure; the positive above supplies it.
                    let omitted = *heights
                        .get(&(deploy_floor + 1))
                        .unwrap()
                        .iter()
                        .next()
                        .unwrap();
                    let deficient_store = blocks().await;
                    for hash in &join_bodies {
                        if *hash == omitted {
                            continue;
                        }
                        let body = source_store
                            .get(&[*hash])
                            .await
                            .unwrap()
                            .into_iter()
                            .flatten()
                            .next()
                            .unwrap();
                        deficient_store.put(&[(*hash, body)]).await.unwrap();
                    }
                    let deficient_index = |hash| {
                        let restored = &restored;
                        let store = &deficient_store;
                        let runtime = &joiner_runtime;
                        async move {
                            let meta = restored.lookup(&hash).await?.unwrap();
                            BlockIndex::get_block_index(
                                runtime,
                                restored,
                                store,
                                hash,
                                Blake2b256Hash::from_byte_array(meta.fringe_state_hash.as_bytes()),
                            )
                            .await
                        }
                    };
                    let deficient = validate_with_window(
                        &restored,
                        &deficient_store,
                        &joiner_runtime,
                        &first_post_anchor,
                        "/root",
                        0,
                        0,
                        &deficient_index,
                        &log,
                        &health,
                        Some(&shared),
                    )
                    .await;
                    assert!(
                        deficient
                            .as_ref()
                            .map(|meta| meta.validation_failed)
                            .unwrap_or(true),
                        "a joiner missing a deploy-window body must not validate the first \
                         post-anchor block"
                    );
                })
        })
        .unwrap()
        .join()
        .unwrap();
}
